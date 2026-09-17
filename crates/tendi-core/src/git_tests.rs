use std::{
    ffi::OsStr,
    fs,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::{
    CommandFailure, GitRepositorySnapshotCache, GitRepositorySnapshotError, LOCAL_COMMAND_TIMEOUT,
    local_repository_snapshot, run_git, run_local_git_query, run_program,
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
fn linked_worktrees_share_mutation_resources() {
    let root = create_git_repo(
        "tendi-git-shared-mutation",
        "https://example.test/shared.git",
        "shared",
    );
    let linked = root.join("linked");
    git_success(
        &root,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            linked.to_str().unwrap(),
        ],
    );
    let namespace = root.join("resource-locks");
    let main_resources = super::mutation_resource_paths(&root).unwrap();
    let linked_resources = super::mutation_resource_paths(&linked).unwrap();
    let common = fs::canonicalize(root.join(".git")).unwrap();
    assert!(
        linked_resources
            .iter()
            .any(|path| { fs::canonicalize(path).ok().as_ref() == Some(&common) })
    );
    let lease =
        crate::coordination::ResourceLease::acquire_paths(&namespace, &main_resources).unwrap();
    let waiting_namespace = namespace.clone();
    let waiting_resources = linked_resources.clone();
    assert!(
        thread::spawn(move || {
            crate::coordination::ResourceLease::try_acquire_paths(
                &waiting_namespace,
                &waiting_resources,
            )
            .unwrap()
            .is_none()
        })
        .join()
        .unwrap()
    );
    drop(lease);
    let lease =
        crate::coordination::ResourceLease::try_acquire_paths(&namespace, &linked_resources)
            .unwrap();
    assert!(lease.is_some());
    drop(lease);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn metadata_snapshot_resolves_local_remote_to_repository_origin() {
    let mirror = create_git_repo(
        "tendi-git-local-remote-mirror",
        "https://example.test/mirror.git",
        "mirror",
    );
    let checkout = create_git_repo(
        "tendi-git-local-remote-checkout",
        mirror.to_str().unwrap(),
        "checkout",
    );
    let cache = GitRepositorySnapshotCache::new(Duration::ZERO);

    let snapshot = cache
        .metadata_snapshot(&checkout, &AtomicBool::new(false))
        .unwrap();

    assert_eq!(
        snapshot.remote_url.as_deref(),
        Some("https://example.test/mirror.git")
    );

    fs::remove_dir_all(checkout).unwrap();
    fs::remove_dir_all(mirror).unwrap();
}

#[test]
fn metadata_snapshot_drops_local_remote_without_repository_origin() {
    let local_remote = temp_dir("tendi-git-local-remote-without-origin");
    fs::create_dir_all(&local_remote).unwrap();
    git_success(&local_remote, &["init", "--quiet"]);
    let checkout = create_git_repo(
        "tendi-git-local-remote-no-origin-checkout",
        local_remote.to_str().unwrap(),
        "checkout",
    );
    let cache = GitRepositorySnapshotCache::new(Duration::ZERO);

    let snapshot = cache
        .metadata_snapshot(&checkout, &AtomicBool::new(false))
        .unwrap();

    assert_eq!(snapshot.remote_url, None);

    fs::remove_dir_all(checkout).unwrap();
    fs::remove_dir_all(local_remote).unwrap();
}

#[test]
fn metadata_snapshot_keeps_network_remote_unchanged() {
    let root = create_git_repo(
        "tendi-git-network-remote",
        "https://example.test/network.git",
        "content",
    );
    let cache = GitRepositorySnapshotCache::new(Duration::ZERO);

    let snapshot = cache
        .metadata_snapshot(&root, &AtomicBool::new(false))
        .unwrap();

    assert_eq!(
        snapshot.remote_url.as_deref(),
        Some("https://example.test/network.git")
    );

    fs::remove_dir_all(root).unwrap();
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
    cache.metadata_snapshot(&root, &cancelled).unwrap();
    let queries = super::local_git_query_trace();

    assert!(
        queries
            .iter()
            .all(|args| { args.first().map(String::as_str) != Some("status") })
    );
    assert!(
        queries
            .iter()
            .any(|args| { args == &vec!["rev-parse".to_string(), "--show-toplevel".to_string()] })
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn successful_mutation_invalidates_process_metadata_cache() {
    let root = create_git_repo(
        "tendi-git-process-invalidation",
        "https://example.test/invalidation.git",
        "before",
    );
    let cancelled = AtomicBool::new(false);

    let first_snapshot = local_repository_snapshot(&root, &cancelled).unwrap();
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
    let refreshed_snapshot = local_repository_snapshot(&root, &cancelled).unwrap();
    assert_ne!(refreshed_snapshot.head_oid, first_snapshot.head_oid);

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

    let first = cache.metadata_snapshot(&root, &cancelled).unwrap();
    fs::write(root.join("tracked.txt"), "after").unwrap();

    let cached = cache.metadata_snapshot(&root, &cancelled).unwrap();
    assert_eq!(cached, first);

    cache.invalidate(&root, None).unwrap();
    let refreshed = cache.metadata_snapshot(&root, &cancelled).unwrap();
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

    let first = cache.metadata_snapshot(&first_root, &cancelled).unwrap();
    let second = cache.metadata_snapshot(&second_root, &cancelled).unwrap();

    assert_ne!(first.repo_root, second.repo_root);
    assert_ne!(first.remote_url, second.remote_url);
    assert_eq!(
        cache.metadata_snapshot(&first_root, &cancelled).unwrap(),
        first
    );
    assert_eq!(
        cache.metadata_snapshot(&second_root, &cancelled).unwrap(),
        second
    );

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
    let successful = cache.metadata_snapshot(&root, &cancelled).unwrap();

    fs::rename(root.join(".git"), root.join(".git-hidden")).unwrap();
    let failed = cache.metadata_snapshot(&root, &cancelled).unwrap();
    fs::rename(root.join(".git-hidden"), root.join(".git")).unwrap();
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
