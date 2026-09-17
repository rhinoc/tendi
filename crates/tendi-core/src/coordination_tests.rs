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
        let subset =
            ResourceLease::try_acquire_paths(&worker_namespace, &[worker_path.join("SKILL.md")])
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
        std::thread::spawn(
            move || ResourceLease::try_acquire_paths(&other_namespace, &[sibling])
                .unwrap()
                .is_some()
        )
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
    let inner = ResourceLease::acquire_paths(&namespace, &[installation.join("SKILL.md")]).unwrap();
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
