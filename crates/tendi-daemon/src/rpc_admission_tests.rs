use super::*;

#[test]
fn remote_skill_update_check_does_not_claim_projection_writer() {
    assert!(!skill_projection_writer("skills_updates"));
    assert!(skill_projection_writer("skills_update"));
}

#[test]
fn skill_reconciliation_projection_resources_are_scoped() {
    let root = std::env::temp_dir().join(format!(
        "tendi-reconciliation-resource-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let store_path = root.join("state.sqlite3");
    let store = tendi_core::storage::Store::open(&store_path).unwrap();
    let first = skills_projection_scope_resource(&store, Path::new("/workspace/first"));
    let second = skills_projection_scope_resource(&store, Path::new("/workspace/second"));
    let same = skills_projection_scope_resource(&store, Path::new("/workspace/first"));
    let shared = skills_projection_resource(&store);

    assert!(!first.conflicts_with(&second).unwrap());
    assert!(first.conflicts_with(&same).unwrap());
    assert!(!first.conflicts_with(&shared).unwrap());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cache_owned_skill_operations_do_not_pre_refresh_the_projection() {
    for method in [
        "skills_refresh",
        "skills_updates",
        "skills_backup_now",
        "skills_backup_status",
    ] {
        assert!(
            projection_dependencies(method).is_empty(),
            "{method} should own its cache/refresh policy"
        );
    }
}

#[test]
fn update_check_emits_one_terminal_event_for_success_queued_cancel_and_closed_scheduler() {
    for scenario in ["success", "cancel", "closed"] {
        let root = std::env::temp_dir().join(format!(
            "tendi-update-terminal-{scenario}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let store = tendi_core::storage::Store::open(&root.join("state.db")).unwrap();
        let daemon = Daemon::with_database(root.clone(), root.join("state.db"), false);
        let events = daemon.subscribe_events();
        let mut blockers = Vec::new();
        let mut releases = Vec::new();
        if scenario == "cancel" {
            for _ in 0..2 {
                let (started, ready) = std::sync::mpsc::channel();
                let (release, released) = std::sync::mpsc::channel();
                blockers.push(
                    daemon
                        .state
                        .requests
                        .submit(
                            tendi_core::OperationId::new("prepare-blocker").unwrap(),
                            Step::acquire(Workload::Prepare, vec![], move || {
                                started.send(()).unwrap();
                                released.recv().unwrap();
                                Ok(Step::Complete(()))
                            }),
                            Arc::new(AtomicBool::new(false)),
                        )
                        .unwrap(),
                );
                ready.recv_timeout(Duration::from_secs(2)).unwrap();
                releases.push(release);
            }
        } else if scenario == "closed" {
            daemon.state.requests.shutdown();
        }
        // An empty, explicitly supplied scan has no filesystem/Git resources
        // and invokes no discovery, default Store, or remote endpoint.
        let status = daemon.start_skill_update_check(
            tendi_core::skills::SkillScan {
                roots: vec![],
                skills: vec![],
                warnings: vec![],
            },
            tendi_core::Revision::ZERO,
        );
        assert_eq!(
            status,
            if scenario == "closed" {
                "unavailable"
            } else {
                "started"
            }
        );
        if scenario == "cancel" {
            assert!(daemon.state.skill_update.cancel());
        }
        let event = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(event.event, SKILL_UPDATE_EVENT);
        assert_eq!(
            event.payload["status"],
            if scenario == "success" {
                "completed"
            } else {
                "failed"
            }
        );
        assert!(
            daemon.state.skill_update.start().is_some(),
            "terminal event must release running state"
        );
        for release in releases {
            release.send(()).unwrap();
        }
        for blocker in blockers {
            blocker
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap();
        }
        daemon.shutdown();
        assert!(
            events.recv_timeout(Duration::from_millis(30)).is_err(),
            "terminal event duplicated"
        );
        drop(daemon);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn temp_database_real_rule_requests_wait_outside_workers_and_settings_still_complete() {
    let root = std::env::temp_dir().join(format!(
        "tendi-rpc-admission-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("AGENTS.md");
    fs::write(&path, "original\n").unwrap();
    let initial = tendi_core::rules::read_rule_file_at_path(&path).unwrap();
    let store = tendi_core::storage::Store::open(&root.join("state.db")).unwrap();
    let scan = tendi_core::rules::RuleScan {
        rules: vec![tendi_core::rules::RuleRecord {
            agents: vec![tendi_core::AgentKind::Shared],
            kind: "instruction".into(),
            scope: "project".into(),
            path: path.clone(),
            order: 0,
            sha256: initial.sha256.clone(),
        }],
        warnings: vec![],
    };
    assert!(
        store
            .save_rules_for_workspace_if_revision(&root, &scan, tendi_core::Revision::ZERO)
            .unwrap()
    );
    let daemon = Daemon::with_database(root.clone(), root.join("state.db"), false);
    let held = tendi_core::coordination::acquire_file_resources(&[path.clone()]).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut workers = Vec::new();
    for id in 0..8 {
        let daemon = daemon.clone();
        let sender = sender.clone();
        let params =
            json!({"path": path, "expectedSha256": initial.sha256, "content": "updated\n"});
        workers.push(thread::spawn(move || {
            sender
                .send(daemon.handle_json_rpc(json!({"jsonrpc":"2.0", "id":id,
                    "method":"rule_file_save", "params":params})))
                .unwrap();
        }));
    }
    let settings_daemon = daemon.clone();
    let (done, response) = std::sync::mpsc::channel();
    let settings = thread::spawn(move || {
        done.send(
            settings_daemon.handle_json_rpc(json!({"jsonrpc":"2.0", "id":99,
                "method":"settings_save", "params":{"appearance":"dark"}})),
        )
        .unwrap();
    });
    let response = response.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(response["result"]["appearance"], "dark");
    assert!(receiver.try_recv().is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "original\n");
    drop(held);
    let responses = (0..8)
        .map(|_| receiver.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        responses
            .iter()
            .filter(|response| response.get("error").is_none())
            .count(),
        1,
        "{responses:?}"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "updated\n");
    settings.join().unwrap();
    for worker in workers {
        worker.join().unwrap();
    }
    daemon.shutdown();
    drop(daemon);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
