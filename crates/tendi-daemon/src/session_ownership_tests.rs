use super::*;

#[test]
fn ownership_worker_refreshes_released_locks_without_history_or_database_work() {
    let root = std::env::temp_dir().join(format!(
        "tendi-ownership-worker-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let locks = root.join("thread-writer-locks");
    fs::create_dir_all(&locks).unwrap();
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(locks.join("live.lock"))
        .unwrap();
    file.try_lock().unwrap();
    let database = root.join("state.sqlite3");
    let daemon = Daemon::with_database(root.clone(), database.clone(), false);
    daemon
        .state
        .session_runtime
        .watcher
        .lock()
        .unwrap()
        .watch_plan = Some(tendi_core::sessions::SessionWatchPlan {
        targets: vec![tendi_core::sessions::SessionWatchTarget {
            path: root.join("sessions"),
            recursive: true,
        }],
        dynamic_roots: Vec::new(),
    });
    let subscription = daemon.subscribe_events();
    let worker_daemon = daemon.clone();
    daemon
        .lifecycle
        .workers
        .lock()
        .unwrap()
        .push(thread::spawn(move || run(worker_daemon)));
    subscription.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        daemon
            .sessions_ownership_snapshot()
            .unwrap()
            .held
            .iter()
            .any(|session| session.id == "live")
    );
    file.unlock().unwrap();
    subscription.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        !daemon
            .sessions_ownership_snapshot()
            .unwrap()
            .held
            .iter()
            .any(|session| session.id == "live")
    );
    assert!(!database.exists());
    daemon.shutdown();
    drop(file);
    drop(daemon);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ownership_publishes_only_changes_and_snapshot_has_latest_state() {
    let root = std::env::temp_dir().join(format!(
        "tendi-ownership-events-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let daemon = Daemon::with_database(root.clone(), root.join("state.sqlite3"), false);
    let subscription = daemon.subscribe_events();
    let held = vec![runtime_schema::SessionOwnershipIdentity {
        id: "live".into(),
        agent: runtime_schema::AgentKind::Codex,
    }];
    publish(&daemon, held.clone());
    let event = subscription.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(event.event.as_str(), SESSION_OWNERSHIP_EVENT);
    assert_eq!(daemon.sessions_ownership_snapshot().unwrap().held, held);
    for _ in 0..100 {
        publish(&daemon, held.clone());
    }
    assert!(
        subscription
            .recv_timeout(Duration::from_millis(10))
            .is_err()
    );
    let revision = daemon.sessions_ownership_snapshot().unwrap().revision;
    publish(&daemon, Vec::new());
    subscription.recv_timeout(Duration::from_secs(1)).unwrap();
    let snapshot = daemon.sessions_ownership_snapshot().unwrap();
    assert!(snapshot.held.is_empty());
    assert_eq!(snapshot.revision, revision + 1);
    daemon.shutdown();
    drop(daemon);
    fs::remove_dir_all(root).unwrap();
}
