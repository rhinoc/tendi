//! Business-resource leases. These never represent a database transaction.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};

#[derive(Debug)]
pub struct ResourceLease {
    // Rc deliberately makes a lease !Send. Nested ownership belongs to the
    // current operation thread and must not leak to a different executor.
    _state: Rc<LeaseState>,
}

#[derive(Debug, Clone)]
struct LeaseState {
    _files: Arc<Vec<File>>,
    namespace: PathBuf,
    paths: Vec<PathBuf>,
    reserved_keys: Vec<String>,
}

/// A declaration can cross threads. It does not own any resource yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceRequest {
    Paths {
        namespace: PathBuf,
        paths: Vec<PathBuf>,
    },
    Named {
        namespace: PathBuf,
        key: String,
    },
}

impl ResourceRequest {
    pub fn files(paths: Vec<PathBuf>) -> Result<Self> {
        Ok(Self::Paths {
            namespace: filesystem_namespace()?,
            paths,
        })
    }
    pub fn named(namespace: &Path, key: impl Into<String>) -> Self {
        Self::Named {
            namespace: namespace.to_path_buf(),
            key: key.into(),
        }
    }

    pub fn conflicts_with(&self, other: &Self) -> Result<bool> {
        fn parts(request: &ResourceRequest) -> (&Path, Option<&str>, &[PathBuf]) {
            match request {
                ResourceRequest::Named { namespace, key } => (namespace, Some(key), &[]),
                ResourceRequest::Paths { namespace, paths } => (namespace, None, paths),
            }
        }
        let (namespace, key, paths) = parts(self);
        let (other_namespace, other_key, other_paths) = parts(other);
        if canonical_resource_path(namespace)? != canonical_resource_path(other_namespace)? {
            return Ok(false);
        }
        match (key, other_key) {
            (Some(a), Some(b)) => Ok(a == b),
            (None, None) => {
                let a = resource_paths(paths)?;
                let b = resource_paths(other_paths)?;
                Ok(a.iter()
                    .any(|a| b.iter().any(|b| a.starts_with(b) || b.starts_with(a))))
            }
            _ => Ok(false),
        }
    }
}

/// An admission pump owns OS locks without claiming thread-local execution.
/// Only `enter` converts the Send reservation into thread-affine ownership.
#[derive(Debug)]
pub struct ResourceReservation {
    states: Vec<LeaseState>,
}

#[derive(Debug)]
pub struct EnteredResources {
    _leases: Vec<ResourceLease>,
}

impl ResourceReservation {
    pub fn acquire(requests: &[ResourceRequest]) -> Result<EnteredResources> {
        let started = Instant::now();
        loop {
            if started.elapsed() >= RESOURCE_WAIT_BUDGET {
                anyhow::bail!("resource acquisition deadline exceeded before operation started");
            }
            if let Some(reservation) = Self::try_acquire(requests)? {
                return Ok(reservation.enter());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn try_acquire(requests: &[ResourceRequest]) -> Result<Option<Self>> {
        let mut groups = BTreeMap::<PathBuf, (Vec<PathBuf>, Vec<String>)>::new();
        for request in requests {
            match request {
                ResourceRequest::Paths { namespace, paths } => groups
                    .entry(canonical_resource_path(namespace)?)
                    .or_default()
                    .0
                    .extend(paths.iter().cloned()),
                ResourceRequest::Named { namespace, key } => groups
                    .entry(canonical_resource_path(namespace)?)
                    .or_default()
                    .1
                    .push(key.clone()),
            }
        }
        let mut states = Vec::new();
        for (namespace, (original, mut keys)) in groups {
            let paths = resource_paths(&original)?;
            keys.sort();
            keys.dedup();
            let mut locks = BTreeMap::<String, bool>::new();
            for path in &paths {
                for ancestor in path.ancestors().skip(1) {
                    locks
                        .entry(format!("path:{}", ancestor.display()))
                        .or_insert(false);
                }
                locks.insert(format!("path:{}", path.display()), true);
            }
            for key in &keys {
                locks.insert(key.clone(), true);
            }
            let mut files = Vec::new();
            for (key, exclusive) in locks {
                let file = ResourceLease::open(&namespace, &key)?;
                match if exclusive {
                    file.try_lock()
                } else {
                    file.try_lock_shared()
                } {
                    Ok(()) => files.push(file),
                    Err(fs::TryLockError::WouldBlock) => return Ok(None),
                    Err(fs::TryLockError::Error(error)) => {
                        return Err(error).context("resource reservation failed");
                    }
                }
            }
            if resource_paths(&original)? != paths {
                return Ok(None);
            }
            states.push(LeaseState {
                _files: Arc::new(files),
                namespace,
                paths,
                reserved_keys: keys,
            });
        }
        Ok(Some(Self { states }))
    }

    pub fn enter(self) -> EnteredResources {
        let leases = self
            .states
            .into_iter()
            .map(|state| {
                let state = Rc::new(state);
                PATH_LEASES.with(|leases| {
                    let mut leases = leases.borrow_mut();
                    leases.retain(|lease| lease.strong_count() > 0);
                    leases.push(Rc::downgrade(&state));
                });
                ResourceLease { _state: state }
            })
            .collect();
        EnteredResources { _leases: leases }
    }
}

thread_local! {
    static PATH_LEASES: RefCell<Vec<Weak<LeaseState>>> = const { RefCell::new(Vec::new()) };
}

const RESOURCE_WAIT_BUDGET: Duration = Duration::from_secs(30);

impl ResourceLease {
    fn open(namespace: &Path, key: &str) -> Result<File> {
        let namespace = canonical_resource_path(namespace)
            .with_context(|| format!("invalid coordination namespace {}", namespace.display()))?;
        fs::create_dir_all(namespace.parent().context("namespace requires a parent")?)?;
        let name = namespace
            .file_name()
            .context("coordination namespace requires a file name")?;
        let key = crate::fsutil::sha256_text(key);
        let path =
            namespace.with_file_name(format!("{}.resource-{key}.lock", name.to_string_lossy()));
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to open resource lease {}", path.display()))
    }

    pub fn try_acquire(namespace: &Path, key: &str) -> Result<Option<Self>> {
        let namespace = canonical_resource_path(namespace)?;
        if let Some(state) = PATH_LEASES.with(|leases| {
            leases
                .borrow()
                .iter()
                .filter_map(Weak::upgrade)
                .find(|state| {
                    state.namespace == namespace
                        && state.reserved_keys.iter().any(|held| held == key)
                })
        }) {
            return Ok(Some(Self { _state: state }));
        }
        let file = Self::open(&namespace, key)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self {
                _state: Rc::new(LeaseState {
                    _files: Arc::new(vec![file]),
                    namespace,
                    paths: Vec::new(),
                    reserved_keys: Vec::new(),
                }),
            })),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(error)) => {
                Err(error).context("failed to acquire resource lease")
            }
        }
    }

    pub fn acquire(namespace: &Path, key: &str) -> Result<Self> {
        wait_for_lease(|| Self::try_acquire(namespace, key))
    }

    /// Canonical physical paths, not workspace IDs, identify shared resources.
    /// Ancestors use shared intention locks; targets use exclusive locks. Thus
    /// replacing a directory conflicts with editing a child, while independent
    /// siblings remain concurrent. Failed multi-resource attempts release all
    /// acquired locks before waiting; they never hold A while waiting for B.
    pub fn acquire_paths(namespace: &Path, paths: &[PathBuf]) -> Result<Self> {
        wait_for_lease(|| Self::try_acquire_paths(namespace, paths))
    }

    pub fn try_acquire_paths(namespace: &Path, paths: &[PathBuf]) -> Result<Option<Self>> {
        let namespace = canonical_resource_path(namespace)?;
        let resolved = resource_paths(paths)?;
        Self::try_acquire_resolved_paths(namespace, paths, resolved)
    }

    fn try_acquire_resolved_paths(
        namespace: PathBuf,
        original_paths: &[PathBuf],
        paths: Vec<PathBuf>,
    ) -> Result<Option<Self>> {
        if paths.is_empty() {
            return Ok(Some(Self {
                _state: Rc::new(LeaseState {
                    _files: Arc::new(Vec::new()),
                    namespace,
                    paths,
                    reserved_keys: Vec::new(),
                }),
            }));
        }
        // The outer compound operation owns filesystem apply, DB commit and
        // rollback together. Inner helpers may reuse a covered subset only.
        let inherited = PATH_LEASES.with(|leases| -> Result<Option<Rc<LeaseState>>> {
            let mut leases = leases.borrow_mut();
            leases.retain(|lease| lease.strong_count() > 0);
            let active = leases
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|lease| lease.namespace == namespace)
                .collect::<Vec<_>>();
            if let Some(owner) = active.iter().find(|owner| {
                paths
                    .iter()
                    .all(|path| owner.paths.iter().any(|held| path.starts_with(held)))
            }) {
                return Ok(Some(Rc::clone(owner)));
            }
            if !active.is_empty() {
                anyhow::bail!(
                    "nested resource acquisition cannot expand the outer operation's resource set"
                );
            }
            Ok(None)
        })?;
        if let Some(state) = inherited {
            return Ok(Some(Self { _state: state }));
        }

        let mut locks = BTreeMap::<PathBuf, bool>::new();
        for path in &paths {
            for ancestor in path.ancestors().skip(1) {
                locks.entry(ancestor.to_path_buf()).or_insert(false);
            }
            locks.insert(path.clone(), true);
        }
        let mut files = Vec::with_capacity(locks.len());
        for (path, exclusive) in locks {
            let file = Self::open(&namespace, &format!("path:{}", path.display()))?;
            let acquired = if exclusive {
                file.try_lock()
            } else {
                file.try_lock_shared()
            };
            match acquired {
                Ok(()) => {}
                Err(fs::TryLockError::WouldBlock) => return Ok(None),
                Err(fs::TryLockError::Error(error)) => {
                    return Err(error)
                        .with_context(|| format!("failed to acquire resource {}", path.display()));
                }
            }
            files.push(file);
        }
        // Resolving a symlink and locking its name are separate filesystem steps.
        // If another owner retargeted it between them, these locks protect the
        // old target, not the path the caller will use. Drop the entire attempt.
        if resource_paths(original_paths)? != paths {
            return Ok(None);
        }
        let state = Rc::new(LeaseState {
            _files: Arc::new(files),
            namespace,
            paths,
            reserved_keys: Vec::new(),
        });
        PATH_LEASES.with(|leases| leases.borrow_mut().push(Rc::downgrade(&state)));
        Ok(Some(Self { _state: state }))
    }
}

fn wait_for_lease(
    mut attempt: impl FnMut() -> Result<Option<ResourceLease>>,
) -> Result<ResourceLease> {
    let started = Instant::now();
    loop {
        if started.elapsed() >= RESOURCE_WAIT_BUDGET {
            anyhow::bail!("resource acquisition deadline exceeded before operation started");
        }
        if let Some(lease) = attempt()? {
            if started.elapsed() >= RESOURCE_WAIT_BUDGET {
                anyhow::bail!("resource acquisition deadline exceeded before operation started");
            }
            return Ok(lease);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn resource_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut resolved = Vec::new();
    for path in paths {
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            std::env::current_dir()?.join(path)
        };
        resolved.push(canonical_resource_path(&absolute)?);
        // Protect the named link as well as its physical target: another Tendi
        // operation cannot replace a symlink while its target is being edited.
        if let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) {
            resolved.push(canonical_resource_path(parent)?.join(name));
        }
    }
    resolved.sort();
    resolved.dedup();
    let mut roots: Vec<PathBuf> = Vec::new();
    for path in resolved {
        if !roots.iter().any(|root| path.starts_with(root)) {
            roots.push(path);
        }
    }
    Ok(roots)
}

pub fn canonical_resource_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return canonical_resource_path(&std::env::current_dir()?.join(path));
    }
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().context("resource path requires a parent")?;
            let name = path
                .file_name()
                .context("resource path requires a file name")?;
            Ok(canonical_resource_path(parent)?.join(name))
        }
        Err(error) => {
            Err(error).with_context(|| format!("failed to resolve resource {}", path.display()))
        }
    }
}

/// All Tendi filesystem mutations share this namespace, including CLI calls
/// without a Store. Resolving it does not open or initialize a database.
pub fn filesystem_namespace() -> Result<PathBuf> {
    let database = crate::storage::default_db_path()?;
    Ok(database
        .parent()
        .context("application directory is missing")?
        .join("coordination/filesystem"))
}

pub fn acquire_file_resources(paths: &[PathBuf]) -> Result<ResourceLease> {
    ResourceLease::acquire_paths(&filesystem_namespace()?, paths)
}

pub fn try_acquire_file_resources(paths: &[PathBuf]) -> Result<Option<ResourceLease>> {
    ResourceLease::try_acquire_paths(&filesystem_namespace()?, paths)
}

/// Explicitly delegate a covered subset to a scoped child task. The child must
/// enter the reservation; unrelated threads never inherit ownership implicitly.
pub fn fork_current_file_resources(paths: &[PathBuf]) -> Result<Option<ResourceReservation>> {
    let namespace = canonical_resource_path(&filesystem_namespace()?)?;
    let paths = resource_paths(paths)?;
    PATH_LEASES.with(|leases| {
        let active = leases
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|state| state.namespace == namespace)
            .collect::<Vec<_>>();
        if active.is_empty() {
            return Ok(None);
        }
        let owner = active
            .iter()
            .find(|state| {
                paths
                    .iter()
                    .all(|path| state.paths.iter().any(|held| path.starts_with(held)))
            })
            .context("child resource delegation cannot expand parent ownership")?;
        let mut state = (**owner).clone();
        state.paths = paths;
        state.reserved_keys.clear();
        Ok(Some(ResourceReservation {
            states: vec![state],
        }))
    })
}

pub fn projection_key(domain: &str, workspace: &Path) -> String {
    format!(
        "projection:{domain}:{}",
        crate::storage::canonical_workspace_root(workspace).display()
    )
}

/// Skills are projected from installations shared by multiple workspaces.
/// Their refresh and reconciliation writers therefore need one lease across
/// workspace scopes; a workspace-specific lease allows the same physical
/// installation to advance another scope's CAS revision concurrently.
pub fn shared_projection_key(domain: &str) -> String {
    format!("projection:{domain}:shared")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_projection_key_is_independent_of_workspace() {
        assert_eq!(shared_projection_key("skills"), "projection:skills:shared");
        assert_ne!(
            shared_projection_key("skills"),
            projection_key("skills", Path::new("/workspace"))
        );
    }

    #[test]
    fn reservation_transfers_ownership_once_and_enters_named_and_path_subsets() {
        let (root, namespace) = fixture("reservation-transfer");
        let path = root.join("installation");
        let requests = vec![
            ResourceRequest::Paths {
                namespace: namespace.clone(),
                paths: vec![path.clone()],
            },
            ResourceRequest::named(&namespace, "projection"),
        ];
        let reservation = ResourceReservation::try_acquire(&requests)
            .unwrap()
            .unwrap();
        assert!(
            ResourceReservation::try_acquire(&requests)
                .unwrap()
                .is_none()
        );
        let worker_namespace = namespace.clone();
        let worker_path = path.clone();
        std::thread::spawn(move || {
            let entered = reservation.enter();
            let named = ResourceLease::try_acquire(&worker_namespace, "projection")
                .unwrap()
                .unwrap();
            let subset = ResourceLease::try_acquire_paths(
                &worker_namespace,
                &[worker_path.join("SKILL.md")],
            )
            .unwrap()
            .unwrap();
            drop(entered);
            assert!(
                ResourceReservation::try_acquire(&[ResourceRequest::named(
                    &worker_namespace,
                    "projection"
                )])
                .unwrap()
                .is_none()
            );
            drop(named);
            drop(subset);
        })
        .join()
        .unwrap();
        assert!(
            ResourceReservation::try_acquire(&requests)
                .unwrap()
                .is_some()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reservation_failure_releases_every_namespace_and_prevents_partial_admission() {
        let (root, namespace) = fixture("reservation-atomic");
        let other = root.join("other");
        let held = ResourceLease::acquire(&other, "busy").unwrap();
        let requests = vec![
            ResourceRequest::named(&namespace, "available"),
            ResourceRequest::named(&other, "busy"),
        ];
        assert!(
            ResourceReservation::try_acquire(&requests)
                .unwrap()
                .is_none()
        );
        assert!(
            ResourceLease::try_acquire(&namespace, "available")
                .unwrap()
                .is_some()
        );
        drop(held);
        assert!(
            ResourceReservation::try_acquire(&requests)
                .unwrap()
                .is_some()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_child_delegation_retains_file_locks_after_parent_exits() {
        let (root, _) = fixture("child-delegation");
        let repository = root.join("repository");
        let request = ResourceRequest::files(vec![repository.clone()]).unwrap();
        let entered = ResourceReservation::try_acquire(&[request.clone()])
            .unwrap()
            .unwrap()
            .enter();
        let child_path = repository.join(".git");
        let child = fork_current_file_resources(&[child_path.clone()])
            .unwrap()
            .unwrap();
        assert!(fork_current_file_resources(&[root.join("unrelated")]).is_err());
        let (ready, started) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _entered = child.enter();
            let _nested = acquire_file_resources(&[child_path]).unwrap();
            ready.send(()).unwrap();
            released.recv().unwrap();
        });
        started.recv().unwrap();
        drop(entered);
        assert!(
            ResourceReservation::try_acquire(&[request.clone()])
                .unwrap()
                .is_none()
        );
        release.send(()).unwrap();
        worker.join().unwrap();
        assert!(
            ResourceReservation::try_acquire(&[request])
                .unwrap()
                .is_some()
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn fixture(name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "tendi-resource-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir_all(&root).unwrap();
        (root.clone(), root.join("namespace"))
    }

    #[test]
    fn directory_exclusion_conflicts_with_children_but_not_siblings() {
        let (root, namespace) = fixture("hierarchy");
        let directory = root.join("skill");
        fs::create_dir(&directory).unwrap();
        let child = directory.join("SKILL.md");
        let held = ResourceLease::acquire_paths(&namespace, &[directory.clone()]).unwrap();
        let other_namespace = namespace.clone();
        let other_child = child.clone();
        assert!(
            std::thread::spawn(move || ResourceLease::try_acquire_paths(
                &other_namespace,
                &[other_child]
            )
            .unwrap()
            .is_none())
            .join()
            .unwrap()
        );
        drop(held);
        let held = ResourceLease::acquire_paths(&namespace, &[child]).unwrap();
        let other_namespace = namespace.clone();
        let sibling = directory.join("README.md");
        assert!(
            std::thread::spawn(move || ResourceLease::try_acquire_paths(
                &other_namespace,
                &[sibling]
            )
            .unwrap()
            .is_some())
            .join()
            .unwrap()
        );
        let other_namespace = namespace;
        assert!(
            std::thread::spawn(move || ResourceLease::try_acquire_paths(
                &other_namespace,
                &[directory]
            )
            .unwrap()
            .is_none())
            .join()
            .unwrap()
        );
        drop(held);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nested_subset_retains_outer_ownership_and_expansion_is_rejected() {
        let (root, namespace) = fixture("nested");
        let installation = root.join("installation");
        let outer = ResourceLease::acquire_paths(&namespace, &[installation.clone()]).unwrap();
        let inner =
            ResourceLease::acquire_paths(&namespace, &[installation.join("SKILL.md")]).unwrap();
        assert!(
            ResourceLease::try_acquire_paths(&namespace, &[root.join("unrelated")])
                .unwrap_err()
                .to_string()
                .contains("cannot expand")
        );
        drop(outer);
        let other_namespace = namespace.clone();
        let other_installation = installation.clone();
        assert!(
            std::thread::spawn(move || ResourceLease::try_acquire_paths(
                &other_namespace,
                &[other_installation]
            )
            .unwrap()
            .is_none())
            .join()
            .unwrap()
        );
        drop(inner);
        assert!(
            ResourceLease::try_acquire_paths(&namespace, &[installation])
                .unwrap()
                .is_some()
        );
        assert!(
            !namespace.exists(),
            "coordination never creates a database or namespace data file"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_multi_resource_acquisition_releases_partial_locks() {
        let (root, namespace) = fixture("all-or-none");
        let first = root.join("a");
        let second = root.join("b");
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let other_namespace = namespace.clone();
        let other_second = second.clone();
        let worker = std::thread::spawn(move || {
            let _lease = ResourceLease::acquire_paths(&other_namespace, &[other_second]).unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        ready_rx.recv().unwrap();
        assert!(
            ResourceLease::try_acquire_paths(&namespace, &[first.clone(), second.clone()])
                .unwrap()
                .is_none()
        );
        // If the unsuccessful group retained A, this independent owner could not acquire it.
        assert!(
            ResourceLease::try_acquire_paths(&namespace, &[first])
                .unwrap()
                .is_some()
        );
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(
            ResourceLease::try_acquire_paths(&namespace, &[second])
                .unwrap()
                .is_some()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn alias_retargeted_after_resolution_retries_without_retaining_old_locks() {
        for parent_alias in [false, true] {
            let (root, namespace) = fixture(if parent_alias {
                "retarget-parent"
            } else {
                "retarget-leaf"
            });
            let old_directory = root.join("old");
            let new_directory = root.join("new");
            fs::create_dir(&old_directory).unwrap();
            fs::create_dir(&new_directory).unwrap();
            let old_file = old_directory.join("config");
            let new_file = new_directory.join("config");
            fs::write(&old_file, "old").unwrap();
            fs::write(&new_file, "new").unwrap();
            let alias = root.join("alias");
            let old_target = if parent_alias {
                &old_directory
            } else {
                &old_file
            };
            let new_target = if parent_alias {
                &new_directory
            } else {
                &new_file
            };
            std::os::unix::fs::symlink(old_target, &alias).unwrap();
            let original = vec![if parent_alias {
                alias.join("config")
            } else {
                alias.clone()
            }];
            let resolved = resource_paths(&original).unwrap();

            // Another cooperating owner completes a retarget between the two
            // actual production phases: resolution and lock acquisition.
            let replacement = ResourceLease::acquire_paths(&namespace, &[alias.clone()]).unwrap();
            fs::remove_file(&alias).unwrap();
            std::os::unix::fs::symlink(new_target, &alias).unwrap();
            drop(replacement);
            assert!(
                ResourceLease::try_acquire_resolved_paths(namespace.clone(), &original, resolved)
                    .unwrap()
                    .is_none()
            );
            assert!(
                ResourceLease::try_acquire_paths(&namespace, &[old_file])
                    .unwrap()
                    .is_some()
            );

            let current = ResourceLease::acquire_paths(&namespace, &original).unwrap();
            let other_namespace = namespace.clone();
            assert!(
                std::thread::spawn(move || ResourceLease::try_acquire_paths(
                    &other_namespace,
                    &[new_file]
                )
                .unwrap()
                .is_none())
                .join()
                .unwrap()
            );
            drop(current);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn physical_path_aliases_share_one_lease_across_workspaces() {
        let root = std::env::temp_dir().join(format!(
            "tendi-resource-alias-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let namespace = root.join("test.sqlite3");
        fs::write(&namespace, "").unwrap();
        let shared = root.join("shared-skill");
        fs::create_dir(&shared).unwrap();
        let alias = root.join("workspace-skill");
        std::os::unix::fs::symlink(&shared, &alias).unwrap();
        let lease = ResourceLease::acquire_paths(&namespace, &[alias, shared.clone()]).unwrap();
        let key = format!("path:{}", fs::canonicalize(shared).unwrap().display());
        assert!(
            ResourceLease::try_acquire(&namespace, &key)
                .unwrap()
                .is_none()
        );
        drop(lease);
        assert!(
            ResourceLease::try_acquire(&namespace, &key)
                .unwrap()
                .is_some()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn leases_are_resource_scoped_and_release_on_drop() {
        let root = std::env::temp_dir().join(format!(
            "tendi-resource-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let namespace = root.join("test.sqlite3");
        fs::write(&namespace, "").unwrap();
        let lease = ResourceLease::try_acquire(&namespace, "sessions")
            .unwrap()
            .unwrap();
        assert!(
            ResourceLease::try_acquire(&namespace, "sessions")
                .unwrap()
                .is_none()
        );
        assert!(
            ResourceLease::try_acquire(&namespace, "skills")
                .unwrap()
                .is_some()
        );
        drop(lease);
        assert!(
            ResourceLease::try_acquire(&namespace, "sessions")
                .unwrap()
                .is_some()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
