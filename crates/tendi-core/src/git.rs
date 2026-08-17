use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

pub(crate) const LOCAL_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const NETWORK_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

static NEVER_CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum CommandFailure {
    Cancelled,
    TimedOut,
    Spawn,
    Wait,
}

#[derive(Debug)]
pub(crate) struct CommandError {
    pub(crate) kind: CommandFailure,
    program: OsString,
    args: Vec<OsString>,
    cwd: PathBuf,
    timeout: Duration,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let command = std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(
                self.args
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned()),
            )
            .collect::<Vec<_>>()
            .join(" ");
        let detail = match self.kind {
            CommandFailure::Cancelled => "cancelled".to_string(),
            CommandFailure::TimedOut => format!("timed out after {}ms", self.timeout.as_millis()),
            CommandFailure::Spawn => "failed to spawn".to_string(),
            CommandFailure::Wait => "failed while waiting".to_string(),
        };
        write!(formatter, "{command} in {} {detail}", self.cwd.display())
    }
}

impl std::error::Error for CommandError {}

pub(crate) fn never_cancelled() -> &'static AtomicBool {
    &NEVER_CANCELLED
}

pub(crate) fn run_git<I, S>(
    cwd: &Path,
    args: I,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect::<Vec<_>>();
    let mutating_command = args
        .first()
        .is_some_and(|command| is_local_repository_mutation(command));
    let output = run_program("git", cwd, &args, timeout, cancelled)?;
    if output.status.success() && mutating_command {
        let _ = invalidate_local_repository_snapshot(cwd, None);
    }
    Ok(output)
}

fn is_local_repository_mutation(command: &OsStr) -> bool {
    matches!(
        command.to_str(),
        Some(
            "fetch"
                | "pull"
                | "merge"
                | "reset"
                | "checkout"
                | "switch"
                | "commit"
                | "rebase"
                | "cherry-pick"
                | "clone"
        )
    )
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum GitRepositoryStatus {
    Clean,
    Dirty,
    NotRepository,
    NotChecked,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct GitRepositorySnapshot {
    pub(crate) workspace: PathBuf,
    pub(crate) repo_root: Option<PathBuf>,
    pub(crate) git_dir: Option<PathBuf>,
    pub(crate) common_dir: Option<PathBuf>,
    pub(crate) remote_url: Option<String>,
    pub(crate) head_oid: Option<String>,
    pub(crate) local_checked_at: SystemTime,
    pub(crate) status: GitRepositoryStatus,
    pub(crate) error: Option<String>,
}

#[derive(Debug)]
pub(crate) enum GitRepositorySnapshotError {
    Command(CommandError),
    QueryFailed {
        operation: &'static str,
        detail: String,
    },
    UnsupportedCommand {
        command: String,
    },
    CachePoisoned,
}

impl std::fmt::Display for GitRepositorySnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command(error) => error.fmt(formatter),
            Self::QueryFailed { operation, detail } => {
                write!(formatter, "git {operation} failed: {detail}")
            }
            Self::UnsupportedCommand { command } => {
                write!(
                    formatter,
                    "git command is not a local snapshot query: {command}"
                )
            }
            Self::CachePoisoned => formatter.write_str("git repository snapshot cache is poisoned"),
        }
    }
}

impl std::error::Error for GitRepositorySnapshotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Command(error) => Some(error),
            Self::QueryFailed { .. } | Self::UnsupportedCommand { .. } | Self::CachePoisoned => {
                None
            }
        }
    }
}

const DEFAULT_REPOSITORY_SNAPSHOT_TTL: Duration = Duration::from_secs(2);

static PROCESS_REPOSITORY_SNAPSHOT_CACHE: OnceLock<GitRepositorySnapshotCache> = OnceLock::new();

pub(crate) fn local_repository_snapshot(
    workspace: &Path,
    cancelled: &AtomicBool,
) -> Result<GitRepositorySnapshot, GitRepositorySnapshotError> {
    process_repository_snapshot_cache().metadata_snapshot(workspace, cancelled)
}

// Dirty state is deliberately opt-in. Metadata consumers must not pay for a
// worktree scan just because a snapshot type can also carry status.
#[allow(dead_code)]
pub(crate) fn local_repository_status(
    workspace: &Path,
    cancelled: &AtomicBool,
) -> Result<GitRepositoryStatus, GitRepositorySnapshotError> {
    process_repository_snapshot_cache().status(workspace, cancelled)
}

pub(crate) fn invalidate_local_repository_snapshot(
    workspace: &Path,
    repo_root: Option<&Path>,
) -> Result<(), GitRepositorySnapshotError> {
    process_repository_snapshot_cache().invalidate(workspace, repo_root)
}

fn process_repository_snapshot_cache() -> &'static GitRepositorySnapshotCache {
    PROCESS_REPOSITORY_SNAPSHOT_CACHE
        .get_or_init(|| GitRepositorySnapshotCache::new(DEFAULT_REPOSITORY_SNAPSHOT_TTL))
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
struct GitRepositoryCacheKey {
    workspace: PathBuf,
    repo_root: PathBuf,
}

#[derive(Debug, Clone)]
struct CachedGitRepositorySnapshot {
    snapshot: GitRepositorySnapshot,
    cached_at: Instant,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct CachedGitRepositoryStatus {
    status: GitRepositoryStatus,
    cached_at: Instant,
    checked_at: SystemTime,
}

pub(crate) struct GitRepositorySnapshotCache {
    ttl: Duration,
    entries: Mutex<HashMap<GitRepositoryCacheKey, CachedGitRepositorySnapshot>>,
    status_entries: Mutex<HashMap<GitRepositoryCacheKey, CachedGitRepositoryStatus>>,
}

impl GitRepositorySnapshotCache {
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
            status_entries: Mutex::new(HashMap::new()),
        }
    }

    // Preserve the existing complete-snapshot API for callers that need status.
    // The resolver-facing local_repository_snapshot uses metadata_snapshot.
    #[allow(dead_code)]
    pub(crate) fn snapshot(
        &self,
        workspace: &Path,
        cancelled: &AtomicBool,
    ) -> Result<GitRepositorySnapshot, GitRepositorySnapshotError> {
        let metadata = self.metadata_snapshot(workspace, cancelled)?;
        let status = self.status_for_metadata(&metadata, cancelled)?;
        let mut snapshot = metadata;
        snapshot.local_checked_at = status.checked_at;
        snapshot.status = status.status;
        self.store_snapshot(snapshot.clone())?;
        Ok(snapshot)
    }

    pub(crate) fn metadata_snapshot(
        &self,
        workspace: &Path,
        cancelled: &AtomicBool,
    ) -> Result<GitRepositorySnapshot, GitRepositorySnapshotError> {
        let workspace = normalize_path(workspace);
        if cancelled.load(Ordering::Acquire) {
            return Err(GitRepositorySnapshotError::Command(CommandError {
                kind: CommandFailure::Cancelled,
                program: OsString::from("git"),
                args: Vec::new(),
                cwd: workspace,
                timeout: LOCAL_COMMAND_TIMEOUT,
            }));
        }

        if let Some(snapshot) = self.fresh_snapshot(&workspace)? {
            let mut metadata = snapshot;
            if metadata.status != GitRepositoryStatus::NotRepository {
                metadata.status = GitRepositoryStatus::NotChecked;
            }
            return Ok(metadata);
        }

        let snapshot = collect_repository_snapshot(&workspace, cancelled)?;
        if snapshot.error.is_none() {
            self.store_snapshot(snapshot.clone())?;
        }
        Ok(snapshot)
    }

    #[allow(dead_code)]
    pub(crate) fn status(
        &self,
        workspace: &Path,
        cancelled: &AtomicBool,
    ) -> Result<GitRepositoryStatus, GitRepositorySnapshotError> {
        let metadata = self.metadata_snapshot(workspace, cancelled)?;
        Ok(self.status_for_metadata(&metadata, cancelled)?.status)
    }

    #[allow(dead_code)]
    fn status_for_metadata(
        &self,
        metadata: &GitRepositorySnapshot,
        cancelled: &AtomicBool,
    ) -> Result<CachedGitRepositoryStatus, GitRepositorySnapshotError> {
        if metadata.status == GitRepositoryStatus::NotRepository {
            return Ok(CachedGitRepositoryStatus {
                status: GitRepositoryStatus::NotRepository,
                cached_at: Instant::now(),
                checked_at: metadata.local_checked_at,
            });
        }
        let Some(repo_root) = metadata.repo_root.as_ref() else {
            return Err(GitRepositorySnapshotError::QueryFailed {
                operation: "status",
                detail: "metadata snapshot has no repository root".to_string(),
            });
        };
        let key = GitRepositoryCacheKey {
            workspace: metadata.workspace.clone(),
            repo_root: repo_root.clone(),
        };
        if !self.ttl.is_zero() {
            let status_entries = self
                .status_entries
                .lock()
                .map_err(|_| GitRepositorySnapshotError::CachePoisoned)?;
            if let Some(entry) = status_entries
                .get(&key)
                .filter(|entry| entry.cached_at.elapsed() < self.ttl)
            {
                return Ok(entry.clone());
            }
        }

        let status_output = query_required(
            &metadata.workspace,
            &["status", "--porcelain", "--untracked-files=normal"],
            cancelled,
            "status",
        )?;
        let entry = CachedGitRepositoryStatus {
            status: if status_output.stdout.is_empty() {
                GitRepositoryStatus::Clean
            } else {
                GitRepositoryStatus::Dirty
            },
            cached_at: Instant::now(),
            checked_at: SystemTime::now(),
        };
        let mut status_entries = self
            .status_entries
            .lock()
            .map_err(|_| GitRepositorySnapshotError::CachePoisoned)?;
        status_entries.insert(key, entry.clone());
        Ok(entry)
    }

    pub(crate) fn invalidate(
        &self,
        workspace: &Path,
        repo_root: Option<&Path>,
    ) -> Result<(), GitRepositorySnapshotError> {
        let workspace = normalize_path(workspace);
        let repo_root = repo_root.map(normalize_path);
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| GitRepositorySnapshotError::CachePoisoned)?;
        entries.retain(|key, _| {
            if key.workspace != workspace {
                return true;
            }
            repo_root
                .as_ref()
                .is_some_and(|repo_root| key.repo_root != *repo_root)
        });
        let mut status_entries = self
            .status_entries
            .lock()
            .map_err(|_| GitRepositorySnapshotError::CachePoisoned)?;
        status_entries.retain(|key, _| {
            if key.workspace != workspace {
                return true;
            }
            repo_root
                .as_ref()
                .is_some_and(|repo_root| key.repo_root != *repo_root)
        });
        Ok(())
    }

    fn fresh_snapshot(
        &self,
        workspace: &Path,
    ) -> Result<Option<GitRepositorySnapshot>, GitRepositorySnapshotError> {
        if self.ttl.is_zero() {
            return Ok(None);
        }
        let entries = self
            .entries
            .lock()
            .map_err(|_| GitRepositorySnapshotError::CachePoisoned)?;
        Ok(entries
            .iter()
            .find(|(key, entry)| key.workspace == workspace && entry.cached_at.elapsed() < self.ttl)
            .map(|(_, entry)| entry.snapshot.clone()))
    }

    fn store_snapshot(
        &self,
        snapshot: GitRepositorySnapshot,
    ) -> Result<(), GitRepositorySnapshotError> {
        let Some(repo_root) = snapshot.repo_root.as_ref() else {
            return Ok(());
        };
        let key = GitRepositoryCacheKey {
            workspace: snapshot.workspace.clone(),
            repo_root: repo_root.clone(),
        };
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| GitRepositorySnapshotError::CachePoisoned)?;
        entries.retain(|existing_key, _| existing_key.workspace != key.workspace);
        entries.insert(
            key,
            CachedGitRepositorySnapshot {
                snapshot,
                cached_at: Instant::now(),
            },
        );
        Ok(())
    }
}

fn collect_repository_snapshot(
    workspace: &Path,
    cancelled: &AtomicBool,
) -> Result<GitRepositorySnapshot, GitRepositorySnapshotError> {
    let root_output = run_local_git_query(workspace, &["rev-parse", "--show-toplevel"], cancelled)?;
    if !root_output.status.success() {
        return Ok(GitRepositorySnapshot {
            workspace: workspace.to_path_buf(),
            repo_root: None,
            git_dir: None,
            common_dir: None,
            remote_url: None,
            head_oid: None,
            local_checked_at: SystemTime::now(),
            status: GitRepositoryStatus::NotRepository,
            error: Some(git_output_detail(&root_output)),
        });
    }

    let repo_root = resolve_git_path(workspace, &root_output, "show-toplevel")?;
    let git_dir = resolve_git_path(
        workspace,
        &query_required(workspace, &["rev-parse", "--git-dir"], cancelled, "git-dir")?,
        "git-dir",
    )?;
    let common_dir = resolve_git_path(
        workspace,
        &query_required(
            workspace,
            &["rev-parse", "--git-common-dir"],
            cancelled,
            "git-common-dir",
        )?,
        "git-common-dir",
    )?;

    let remote_url = query_optional(
        workspace,
        &["config", "--get", "remote.origin.url"],
        cancelled,
    )?;
    let head_oid = query_optional(workspace, &["rev-parse", "HEAD"], cancelled)?;

    Ok(GitRepositorySnapshot {
        workspace: workspace.to_path_buf(),
        repo_root: Some(repo_root),
        git_dir: Some(git_dir),
        common_dir: Some(common_dir),
        remote_url,
        head_oid,
        local_checked_at: SystemTime::now(),
        status: GitRepositoryStatus::NotChecked,
        error: None,
    })
}

fn query_required(
    cwd: &Path,
    args: &[&str],
    cancelled: &AtomicBool,
    operation: &'static str,
) -> Result<Output, GitRepositorySnapshotError> {
    let output = run_local_git_query(cwd, args, cancelled)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(GitRepositorySnapshotError::QueryFailed {
            operation,
            detail: git_output_detail(&output),
        })
    }
}

fn query_optional(
    cwd: &Path,
    args: &[&str],
    cancelled: &AtomicBool,
) -> Result<Option<String>, GitRepositorySnapshotError> {
    let output = run_local_git_query(cwd, args, cancelled)?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty()))
}

fn resolve_git_path(
    workspace: &Path,
    output: &Output,
    operation: &'static str,
) -> Result<PathBuf, GitRepositorySnapshotError> {
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if value.is_empty() {
        return Err(GitRepositorySnapshotError::QueryFailed {
            operation,
            detail: "git returned an empty path".to_string(),
        });
    }
    let path = PathBuf::from(value);
    let path = if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    };
    Ok(normalize_path(&path))
}

fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(path) = fs::canonicalize(path) {
        return path;
    }
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current_dir| current_dir.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn run_local_git_query(
    cwd: &Path,
    args: &[&str],
    cancelled: &AtomicBool,
) -> Result<Output, GitRepositorySnapshotError> {
    #[cfg(test)]
    record_local_git_query(args);
    if !is_local_git_query(args) {
        return Err(GitRepositorySnapshotError::UnsupportedCommand {
            command: args.join(" "),
        });
    }
    run_git(cwd, args, LOCAL_COMMAND_TIMEOUT, cancelled)
        .map_err(GitRepositorySnapshotError::Command)
}

#[cfg(test)]
thread_local! {
    static LOCAL_GIT_QUERY_TRACE: std::cell::RefCell<Vec<Vec<String>>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

#[cfg(test)]
fn record_local_git_query(args: &[&str]) {
    LOCAL_GIT_QUERY_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .push(args.iter().map(|arg| (*arg).to_string()).collect());
    });
}

#[cfg(test)]
fn reset_local_git_query_trace() {
    LOCAL_GIT_QUERY_TRACE.with(|trace| trace.borrow_mut().clear());
}

#[cfg(test)]
fn local_git_query_trace() -> Vec<Vec<String>> {
    LOCAL_GIT_QUERY_TRACE.with(|trace| trace.borrow().clone())
}

fn is_local_git_query(args: &[&str]) -> bool {
    matches!(
        args,
        ["rev-parse", "--show-toplevel"]
            | ["rev-parse", "--git-dir"]
            | ["rev-parse", "--git-common-dir"]
            | ["rev-parse", "HEAD"]
            | ["config", "--get", "remote.origin.url"]
            | ["status", "--porcelain", "--untracked-files=normal"]
    )
}

fn git_output_detail(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !stdout.is_empty() {
        return stdout;
    }
    output
        .status
        .code()
        .map(|code| format!("git exited with status {code}"))
        .unwrap_or_else(|| "git exited without a status code".to_string())
}

pub(crate) fn run_program<I, S, P>(
    program: P,
    cwd: &Path,
    args: I,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
    P: AsRef<OsStr>,
{
    let program = program.as_ref().to_os_string();
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect::<Vec<_>>();
    if cancelled.load(Ordering::Acquire) {
        return Err(CommandError {
            kind: CommandFailure::Cancelled,
            program,
            args,
            cwd: cwd.to_path_buf(),
            timeout,
        });
    }

    let mut command = Command::new(&program);
    command
        .args(&args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|_| CommandError {
        kind: CommandFailure::Spawn,
        program: program.clone(),
        args: args.clone(),
        cwd: cwd.to_path_buf(),
        timeout,
    })?;
    let stdout = spawn_reader(child.stdout.take());
    let stderr = spawn_reader(child.stderr.take());
    let started = Instant::now();

    let status = loop {
        if cancelled.load(Ordering::Acquire) {
            terminate(&mut child);
            let _ = child.wait();
            return Err(CommandError {
                kind: CommandFailure::Cancelled,
                program,
                args,
                cwd: cwd.to_path_buf(),
                timeout,
            });
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                terminate(&mut child);
                let _ = child.wait();
                return Err(CommandError {
                    kind: CommandFailure::TimedOut,
                    program,
                    args,
                    cwd: cwd.to_path_buf(),
                    timeout,
                });
            }
            // Keep cancellation and timeout checks responsive without adding a
            // full 20ms of latency to every short-lived local git command.
            Ok(None) => thread::sleep(Duration::from_millis(2)),
            Err(_) => {
                terminate(&mut child);
                let _ = child.wait();
                return Err(CommandError {
                    kind: CommandFailure::Wait,
                    program,
                    args,
                    cwd: cwd.to_path_buf(),
                    timeout,
                });
            }
        }
    };

    Ok(Output {
        status,
        stdout: join_reader(stdout),
        stderr: join_reader(stderr),
    })
}

fn spawn_reader(reader: Option<impl Read + Send + 'static>) -> Option<thread::JoinHandle<Vec<u8>>> {
    reader.map(|mut reader| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = reader.read_to_end(&mut bytes);
            bytes
        })
    })
}

fn join_reader(reader: Option<thread::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    reader
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &format!("-{}", child.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsStr,
        fs,
        path::Path,
        sync::{Arc, atomic::AtomicBool},
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    use super::{
        CommandFailure, GitRepositorySnapshotCache, GitRepositorySnapshotError,
        GitRepositoryStatus, LOCAL_COMMAND_TIMEOUT, local_repository_snapshot,
        local_repository_status, run_git, run_local_git_query, run_program,
    };

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn git_success(cwd: &Path, args: &[&str]) {
        let output = run_git(cwd, args, LOCAL_COMMAND_TIMEOUT, &AtomicBool::new(false)).unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn create_git_repo(prefix: &str, remote_url: &str, content: &str) -> std::path::PathBuf {
        let root = temp_dir(prefix);
        fs::create_dir_all(&root).unwrap();
        git_success(&root, &["init", "--quiet"]);
        git_success(&root, &["config", "user.email", "tendi-test@example.com"]);
        git_success(&root, &["config", "user.name", "Tendi Test"]);
        fs::write(root.join("tracked.txt"), content).unwrap();
        git_success(&root, &["add", "tracked.txt"]);
        git_success(&root, &["commit", "--quiet", "-m", "initial"]);
        git_success(&root, &["remote", "add", "origin", remote_url]);
        root
    }

    #[test]
    fn metadata_snapshot_does_not_query_worktree_status() {
        let root = create_git_repo(
            "tendi-git-metadata-only",
            "https://example.test/metadata.git",
            "content",
        );
        let cache = GitRepositorySnapshotCache::new(Duration::from_secs(60));
        let cancelled = AtomicBool::new(false);

        super::reset_local_git_query_trace();
        let snapshot = cache.metadata_snapshot(&root, &cancelled).unwrap();
        let queries = super::local_git_query_trace();

        assert_eq!(snapshot.status, GitRepositoryStatus::NotChecked);
        assert!(
            queries
                .iter()
                .all(|args| { args.first().map(String::as_str) != Some("status") })
        );
        assert!(
            queries.iter().any(|args| {
                args == &vec!["rev-parse".to_string(), "--show-toplevel".to_string()]
            })
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repository_status_api_is_cached_and_refreshes_after_invalidation() {
        let root = create_git_repo(
            "tendi-git-explicit-status",
            "https://example.test/status.git",
            "before",
        );
        let cache = GitRepositorySnapshotCache::new(Duration::from_secs(60));
        let cancelled = AtomicBool::new(false);

        super::reset_local_git_query_trace();
        assert_eq!(
            cache.status(&root, &cancelled).unwrap(),
            GitRepositoryStatus::Clean
        );
        let first_query_count = super::local_git_query_trace().len();

        fs::write(root.join("tracked.txt"), "after").unwrap();
        assert_eq!(
            cache.status(&root, &cancelled).unwrap(),
            GitRepositoryStatus::Clean
        );
        assert_eq!(super::local_git_query_trace().len(), first_query_count);

        cache.invalidate(&root, None).unwrap();
        assert_eq!(
            cache.status(&root, &cancelled).unwrap(),
            GitRepositoryStatus::Dirty
        );
        assert!(super::local_git_query_trace().len() > first_query_count);

        let uncached = GitRepositorySnapshotCache::new(Duration::ZERO);
        assert_eq!(
            uncached.status(&root, &cancelled).unwrap(),
            GitRepositoryStatus::Dirty
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn successful_mutation_invalidates_process_metadata_and_status_caches() {
        let root = create_git_repo(
            "tendi-git-process-invalidation",
            "https://example.test/invalidation.git",
            "before",
        );
        let cancelled = AtomicBool::new(false);

        let first_snapshot = local_repository_snapshot(&root, &cancelled).unwrap();
        assert_eq!(first_snapshot.status, GitRepositoryStatus::NotChecked);
        assert_eq!(
            local_repository_status(&root, &cancelled).unwrap(),
            GitRepositoryStatus::Clean
        );
        let normalized_root = super::normalize_path(&root);
        let process_cache = super::process_repository_snapshot_cache();
        assert!(
            process_cache
                .entries
                .lock()
                .unwrap()
                .keys()
                .any(|key| key.workspace == normalized_root)
        );
        assert!(
            process_cache
                .status_entries
                .lock()
                .unwrap()
                .keys()
                .any(|key| key.workspace == normalized_root)
        );

        fs::write(root.join("tracked.txt"), "after").unwrap();
        git_success(&root, &["commit", "--quiet", "-am", "changed"]);

        assert!(
            !process_cache
                .entries
                .lock()
                .unwrap()
                .keys()
                .any(|key| key.workspace == normalized_root)
        );
        assert!(
            !process_cache
                .status_entries
                .lock()
                .unwrap()
                .keys()
                .any(|key| key.workspace == normalized_root)
        );

        let refreshed_snapshot = local_repository_snapshot(&root, &cancelled).unwrap();
        assert_ne!(refreshed_snapshot.head_oid, first_snapshot.head_oid);
        assert_eq!(
            local_repository_status(&root, &cancelled).unwrap(),
            GitRepositoryStatus::Clean
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn command_uses_requested_working_directory() {
        let root = temp_dir("tendi-git-command-cwd");
        fs::create_dir_all(&root).unwrap();
        let cancelled = AtomicBool::new(false);
        let output = run_program(
            "/bin/pwd",
            &root,
            std::iter::empty::<&str>(),
            Duration::from_secs(1),
            &cancelled,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            root.canonicalize().unwrap().to_string_lossy()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn command_times_out_and_kills_child_process_group() {
        let cancelled = AtomicBool::new(false);
        let started = Instant::now();
        let error = run_program(
            "/bin/sh",
            Path::new("/tmp"),
            ["-c", "sleep 5"],
            Duration::from_millis(100),
            &cancelled,
        )
        .unwrap_err();
        assert_eq!(error.kind, CommandFailure::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn command_cancellation_interrupts_running_process() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let trigger = Arc::clone(&cancelled);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            trigger.store(true, std::sync::atomic::Ordering::Release);
        });
        let error = run_program(
            "/bin/sh",
            Path::new("/tmp"),
            ["-c", "sleep 5"],
            Duration::from_secs(2),
            &cancelled,
        )
        .unwrap_err();
        assert_eq!(error.kind, CommandFailure::Cancelled);
    }

    #[test]
    fn repository_snapshot_reuses_same_repo_within_ttl_and_invalidates() {
        let root = create_git_repo(
            "tendi-git-cache-reuse",
            "https://example.test/reuse.git",
            "before",
        );
        let cache = GitRepositorySnapshotCache::new(Duration::from_secs(60));
        let cancelled = AtomicBool::new(false);

        let first = cache.snapshot(&root, &cancelled).unwrap();
        assert_eq!(first.status, GitRepositoryStatus::Clean);
        fs::write(root.join("tracked.txt"), "after").unwrap();

        let cached = cache.snapshot(&root, &cancelled).unwrap();
        assert_eq!(cached, first);

        cache.invalidate(&root, None).unwrap();
        let refreshed = cache.snapshot(&root, &cancelled).unwrap();
        assert_eq!(refreshed.status, GitRepositoryStatus::Dirty);
        assert_ne!(refreshed.local_checked_at, first.local_checked_at);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repository_snapshot_keys_do_not_mix_different_repositories() {
        let first_root = create_git_repo(
            "tendi-git-cache-first",
            "https://example.test/first.git",
            "first",
        );
        let second_root = create_git_repo(
            "tendi-git-cache-second",
            "https://example.test/second.git",
            "second",
        );
        let cache = GitRepositorySnapshotCache::new(Duration::from_secs(60));
        let cancelled = AtomicBool::new(false);

        let first = cache.snapshot(&first_root, &cancelled).unwrap();
        let second = cache.snapshot(&second_root, &cancelled).unwrap();

        assert_ne!(first.repo_root, second.repo_root);
        assert_ne!(first.remote_url, second.remote_url);
        assert_eq!(cache.snapshot(&first_root, &cancelled).unwrap(), first);
        assert_eq!(cache.snapshot(&second_root, &cancelled).unwrap(), second);

        fs::remove_dir_all(first_root).unwrap();
        fs::remove_dir_all(second_root).unwrap();
    }

    #[test]
    fn failed_snapshot_does_not_replace_successful_cache_entry() {
        let root = create_git_repo(
            "tendi-git-cache-failure",
            "https://example.test/failure.git",
            "stable",
        );
        let cache = GitRepositorySnapshotCache::new(Duration::ZERO);
        let cancelled = AtomicBool::new(false);
        let successful = cache.snapshot(&root, &cancelled).unwrap();

        fs::rename(root.join(".git"), root.join(".git-hidden")).unwrap();
        let failed = cache.snapshot(&root, &cancelled).unwrap();
        fs::rename(root.join(".git-hidden"), root.join(".git")).unwrap();
        assert_eq!(failed.status, GitRepositoryStatus::NotRepository);
        assert!(failed.error.is_some());

        let entries = cache.entries.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries.values().next().unwrap().snapshot, successful);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fetch_and_mutating_commands_are_rejected_from_local_query_path() {
        let commands: &[&[&str]] = &[
            &["fetch", "origin"],
            &["pull"],
            &["merge", "main"],
            &["push", "origin", "main"],
            &["reset", "--hard"],
        ];

        for args in commands {
            let error = run_local_git_query(
                Path::new("/path/that/must/not/be/used"),
                args,
                &AtomicBool::new(false),
            )
            .unwrap_err();
            assert!(matches!(
                error,
                GitRepositorySnapshotError::UnsupportedCommand { .. }
            ));
        }
    }

    #[test]
    fn repository_mutation_detection_covers_writes_but_not_queries() {
        for command in [
            "fetch",
            "pull",
            "merge",
            "reset",
            "checkout",
            "switch",
            "commit",
            "rebase",
            "cherry-pick",
            "clone",
        ] {
            assert!(super::is_local_repository_mutation(OsStr::new(command)));
        }
        for command in ["rev-parse", "config", "status"] {
            assert!(!super::is_local_repository_mutation(OsStr::new(command)));
        }
    }
}
