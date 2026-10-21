use super::*;

const CHECK_INTERVAL: Duration = Duration::from_secs(3);
const EVENT_DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Debug)]
pub(super) struct OwnershipRuntime {
    pub snapshot: Mutex<runtime_schema::SessionOwnershipSnapshot>,
}

impl OwnershipRuntime {
    pub fn new() -> Self {
        Self {
            snapshot: Mutex::new(runtime_schema::SessionOwnershipSnapshot {
                epoch: format!(
                    "{}:{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                ),
                revision: 0,
                held: Vec::new(),
            }),
        }
    }
}

pub(super) fn run(daemon: Daemon) {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
        if let Ok(event) = event {
            let _ = tx.send(event);
        }
    })
    .ok();
    let mut watched = BTreeSet::new();
    let mut source_paths = BTreeSet::new();
    let mut dirty_providers = BTreeSet::new();
    let mut held_by_provider = BTreeMap::new();
    let mut process_monitor = tendi_core::session_ownership::ProcessMonitor::default();
    let mut last_check = Instant::now() - CHECK_INTERVAL;
    let mut dirty_since = None;
    while !daemon.is_shutting_down() {
        if process_monitor.changed() {
            dirty_providers.extend(held_by_provider.keys().copied());
            dirty_since.get_or_insert_with(Instant::now);
        }
        if last_check.elapsed() >= CHECK_INTERVAL
            || dirty_since.is_some_and(|since: Instant| since.elapsed() >= EVENT_DEBOUNCE)
        {
            let roots = daemon
                .state
                .session_runtime
                .watcher
                .lock()
                .ok()
                .and_then(|state| state.watch_plan.clone())
                .map(|plan| {
                    plan.targets
                        .into_iter()
                        .map(|target| target.path)
                        .chain(plan.dynamic_roots)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let full_check = last_check.elapsed() >= CHECK_INTERVAL;
            let probe = tendi_core::session_ownership::probe_selected(
                &daemon.state.cwd,
                &roots,
                if full_check {
                    None
                } else {
                    Some(&dirty_providers)
                },
            );
            if let Some(pids) = probe.process_ids {
                process_monitor.update(pids);
            }
            if full_check {
                source_paths.clear();
            }
            source_paths.extend(probe.watch_paths);

            for (agent, result) in probe.providers {
                match result {
                    Ok(held) => {
                        held_by_provider.insert(agent, held);
                    }
                    Err(error) => {
                        held_by_provider.remove(&agent);
                        tendi_core::logging::global().warn(
                            "session ownership probe failed",
                            json!({ "agent": agent.label(), "error": error.to_string() }),
                        );
                    }
                }
            }
            let held = held_by_provider
                .values()
                .flat_map(|held| held.iter())
                .map(|held| json!({ "id": held.id, "agent": held.agent }))
                .collect::<Vec<_>>();
            if let Ok(held) = serde_json::from_value(json!(held)) {
                publish(&daemon, held);
            }
            if let Some(watcher) = watcher.as_mut() {
                let paths = source_paths
                    .iter()
                    .filter_map(|(_, path)| {
                        path.ancestors()
                            .find(|parent| parent.is_dir())
                            .map(Path::to_path_buf)
                    })
                    .collect::<BTreeSet<_>>();
                for path in watched.difference(&paths) {
                    let _ = watcher.unwatch(path);
                }
                let mut next = BTreeSet::new();
                for path in paths {
                    if watched.contains(&path)
                        || watcher.watch(&path, RecursiveMode::NonRecursive).is_ok()
                    {
                        next.insert(path);
                    }
                }
                watched = next;
            }
            if full_check {
                last_check = Instant::now();
            }
            dirty_since = None;
            dirty_providers.clear();
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => {
                for event in std::iter::once(event).chain(rx.try_iter()) {
                    for (agent, source) in &source_paths {
                        if event
                            .paths
                            .iter()
                            .any(|path| path.starts_with(source) || source.starts_with(path))
                        {
                            dirty_providers.insert(*agent);
                        }
                    }
                }
                if !dirty_providers.is_empty() {
                    dirty_since.get_or_insert_with(Instant::now);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn publish(daemon: &Daemon, held: Vec<runtime_schema::SessionOwnershipIdentity>) {
    let changed = daemon
        .state
        .session_ownership
        .snapshot
        .lock()
        .ok()
        .and_then(|mut snapshot| {
            if snapshot.revision > 0 && snapshot.held == held {
                return None;
            }
            snapshot.revision += 1;
            snapshot.held = held;
            Some(snapshot.clone())
        });
    if let Some(snapshot) = changed {
        daemon.emit_event(
            SESSION_OWNERSHIP_EVENT,
            runtime_schema::RuntimeEventPayload::SessionsOwnership(snapshot),
        );
    }
}

#[cfg(test)]
mod tests {
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
}
