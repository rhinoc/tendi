use super::*;
use tendi_core::generated::runtime_contract::CommandName;

#[test]
fn projection_refresh_retries_only_the_pure_scan_after_concurrent_invalidation() {
    let root = std::env::temp_dir().join(format!(
        "tendi-cli-projection-cas-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let store = tendi_core::storage::Store::open(root.join("test.sqlite3")).unwrap();
    let scans = std::cell::Cell::new(0);
    let publications = std::cell::Cell::new(0);
    let result = refresh_projection(
        &store,
        &root,
        "rules",
        || {
            scans.set(scans.get() + 1);
            if scans.get() == 1 {
                store.invalidate_projection("rules", &root)?;
            }
            Ok(tendi_core::RuleScan {
                rules: vec![],
                warnings: vec![],
            })
        },
        |scan, captured| {
            let applied = store.save_rules_for_workspace_if_revision(&root, scan, captured)?;
            if applied {
                publications.set(publications.get() + 1);
            }
            Ok(applied)
        },
    )
    .unwrap();
    assert!(result.rules.is_empty());
    assert_eq!(scans.get(), 2);
    assert_eq!(publications.get(), 1);
    let state = store
        .read_projection_refresh_state::<tendi_core::RuleScan>("rules", &root)
        .unwrap();
    assert!(!state.full_refresh);
    assert!(state.resources.is_empty());
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn projection_refresh_contention_leaves_durable_work_pending() {
    let root = std::env::temp_dir().join(format!(
        "tendi-cli-projection-pending-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let store = tendi_core::storage::Store::open(root.join("test.sqlite3")).unwrap();
    let result = refresh_projection(
        &store,
        &root,
        "rules",
        || {
            store.invalidate_projection("rules", &root)?;
            Ok(tendi_core::RuleScan {
                rules: vec![],
                warnings: vec![],
            })
        },
        |scan, captured| store.save_rules_for_workspace_if_revision(&root, scan, captured),
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("refresh remains pending")
    );
    let state = store
        .read_projection_refresh_state::<tendi_core::RuleScan>("rules", &root)
        .unwrap();
    assert!(state.full_refresh);
    assert!(state.snapshot.is_none());
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn standalone_search_drains_committed_dirty_sessions_without_a_daemon() {
    let root = std::env::temp_dir().join(format!(
        "tendi-cli-search-pending-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let store = tendi_core::storage::Store::open(root.join("test.sqlite3")).unwrap();
    let scope = workspace_scope_key(&root).unwrap();
    let path = root.join("session.jsonl");
    std::fs::write(&path, "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"standaloneneedle\"}]}}\n").unwrap();
    let session: tendi_core::SessionRecord = serde_json::from_value(serde_json::json!({
        "id":"cli-search", "agent":"codex", "path":path,
    }))
    .unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session], &[])
        .unwrap();
    assert_eq!(
        store.pending_session_search_scopes().unwrap(),
        [scope.clone()]
    );
    refresh_session_search(&store, &scope).unwrap();
    assert!(store.pending_session_search_scopes().unwrap().is_empty());
    assert_eq!(
        store
            .search_sessions_for_scope(&scope, "standaloneneedle", None)
            .unwrap()
            .len(),
        1
    );
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn skill_add_accepts_extended_target_and_project_scope() {
    let cli = Cli::try_parse_from([
        "tendi", "skills", "add", "./repo", "--to", "opencode", "--scope", "project",
    ])
    .unwrap();
    let Command::Skills {
        command: SkillCommand::Add { to, scope, .. },
    } = cli.command
    else {
        panic!("unexpected command");
    };
    assert_eq!(to.id(), "opencode");
    assert_eq!(scope, tendi_core::SkillInstallScope::Project);
}

#[test]
fn skill_link_keeps_legacy_shared_target() {
    let cli =
        Cli::try_parse_from(["tendi", "skills", "link", "./skill", "--to", "shared"]).unwrap();
    let Command::Skills {
        command: SkillCommand::Link { to, scope, .. },
    } = cli.command
    else {
        panic!("unexpected command");
    };
    assert_eq!(to.id(), "shared");
    assert_eq!(scope, tendi_core::SkillInstallScope::Global);
}

#[test]
fn skill_restore_accepts_dry_run_and_yes() {
    let cli = Cli::try_parse_from(["tendi", "skills", "restore", "--dry-run", "--yes"]).unwrap();
    let Command::Skills {
        command: SkillCommand::Restore { dry_run, yes },
    } = cli.command
    else {
        panic!("unexpected command");
    };
    assert!(dry_run);
    assert!(yes);
}

#[test]
fn skill_backup_configure_accepts_a_remote_without_a_device_label() {
    let cli = Cli::try_parse_from([
        "tendi",
        "skills",
        "sync",
        "configure",
        "git@github.com:example/skills.git",
    ])
    .unwrap();
    let Command::Skills {
        command:
            SkillCommand::Sync {
                command: BackupCommand::Configure { remote_url, .. },
            },
    } = cli.command
    else {
        panic!("unexpected command");
    };
    assert_eq!(remote_url, "git@github.com:example/skills.git");
}

#[test]
fn generated_runtime_client_emits_json_rpc_method_and_params() {
    let mut client = RuntimeClient::new();
    let request =
        client.sessions_snapshot(tendi_core::generated::runtime_contract::EmptyRequest::default());
    assert_eq!(request.jsonrpc, "2.0");
    assert_eq!(request.method, "sessions_snapshot");
    assert_eq!(request.params, serde_json::json!({}));
}

#[test]
fn generated_runtime_client_decodes_json_rpc_success_and_error() {
    let result = RuntimeClient::decode_response(
        CommandName::SessionsSnapshot,
        &serde_json::json!("cli-1"),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "cli-1",
            "result": {
                "scopeKey": "workspace:/repo",
                "domain": "sessions",
                "revision": 0,
                "schemaVersion": 1,
                "snapshotId": "snapshot-1",
                "payload": []
            }
        }),
    )
    .unwrap();
    assert_eq!(result["domain"], "sessions");

    let error = RuntimeClient::decode_response(
        CommandName::SessionsSnapshot,
        &serde_json::json!("cli-1"),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "cli-1",
            "error": { "code": -32002, "message": "conflict", "data": { "kind": "CONFLICT" } }
        }),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "CONFLICT: conflict");
}
