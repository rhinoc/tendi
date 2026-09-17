use super::*;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

static TEST_LOCK: Mutex<()> = Mutex::new(());

struct ShortWriter {
    bytes: Vec<u8>,
    max_write: usize,
}

impl Write for ShortWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = bytes.len().min(self.max_write);
        self.bytes.extend_from_slice(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn temp_workspace() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "tendi-daemon-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join(".agents/skills/demo")).unwrap();
    fs::write(
        root.join(".agents/skills/demo/SKILL.md"),
        "---\nname: demo\ndescription: Demo\n---\n\n# Demo\n",
    )
    .unwrap();
    root
}

fn test_daemon(cwd: PathBuf) -> Daemon {
    fs::create_dir_all(&cwd).unwrap();
    let database_path = cwd.with_extension("sqlite3");
    let mut daemon = Daemon::with_database(cwd, database_path.clone(), true);
    daemon.test_database_path = Some(database_path);
    daemon
}

fn test_daemon_without_background(cwd: PathBuf) -> Daemon {
    fs::create_dir_all(&cwd).unwrap();
    let database_path = cwd.with_extension("sqlite3");
    let mut daemon = Daemon::with_database(cwd, database_path.clone(), false);
    daemon.test_database_path = Some(database_path);
    daemon
}

fn test_store(daemon: &Daemon) -> tendi_core::storage::Store {
    tendi_core::storage::Store::open(&daemon.state.database_path).unwrap()
}

fn listed_skill_id(response: &Value, name: &str) -> String {
    response
        .as_array()
        .and_then(|skills| skills.iter().find(|skill| skill["name"] == name))
        .and_then(|skill| skill["id"].as_str())
        .unwrap_or_else(|| panic!("skill {name} was not present in listing: {response}"))
        .to_string()
}

#[test]
fn skill_update_preview_is_not_recorded_as_a_runtime_operation() {
    assert!(!should_record_runtime_operation(
        "skills_update_many",
        &json!({ "dryRun": true })
    ));
    assert!(should_record_runtime_operation(
        "skills_update_many",
        &json!({ "dryRun": false })
    ));
    assert!(should_record_runtime_operation(
        "skills_delete_many",
        &json!({})
    ));
}

#[test]
fn skill_watcher_ignores_tendi_atomic_write_temporary_paths() {
    assert!(is_skill_watcher_transient_path(Path::new(
        "/tmp/demo/.SKILL.md.tendi-tmp-123-1"
    )));
    assert!(is_skill_watcher_transient_path(Path::new(
        "/tmp/demo/agents/.openai.yaml.tendi-tmp-123-2"
    )));
    assert!(!is_skill_watcher_transient_path(Path::new(
        "/tmp/demo/SKILL.md"
    )));
}

#[test]
fn skill_reconciliation_failure_backoff_is_scoped_and_event_resettable() {
    let root = temp_workspace();
    let other = root.join("other-workspace");
    fs::create_dir_all(&other).unwrap();
    let daemon = test_daemon_without_background(root.clone());
    let workspace = tendi_core::storage::canonical_workspace_root(&root);
    let other_workspace = tendi_core::storage::canonical_workspace_root(&other);

    daemon.record_skill_reconciliation_failure(&workspace, "invalid skill metadata");
    assert!(!daemon.skill_reconciliation_retry_ready(&workspace));
    assert!(daemon.skill_reconciliation_retry_ready(&other_workspace));
    let first_delay = daemon
        .state
        .skill_reconciliation_backoff
        .lock()
        .unwrap()
        .get(&workspace)
        .unwrap()
        .delay;

    daemon.record_skill_reconciliation_failure(&workspace, "invalid skill metadata");
    let second_delay = daemon
        .state
        .skill_reconciliation_backoff
        .lock()
        .unwrap()
        .get(&workspace)
        .unwrap()
        .delay;
    assert!(second_delay > first_delay);

    daemon.invalidate_skill_projection(&[]).unwrap();
    assert!(daemon.skill_reconciliation_retry_ready(&workspace));
    daemon.shutdown();
    drop(daemon);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn permanent_skill_parse_failure_waits_for_a_filesystem_event() {
    let root = temp_workspace();
    let daemon = test_daemon_without_background(root.clone());
    let workspace = tendi_core::storage::canonical_workspace_root(&root);

    daemon.record_skill_reconciliation_failure(
        &workspace,
        "failed to parse /tmp/SKILL.md: invalid frontmatter",
    );
    assert!(!daemon.skill_reconciliation_retry_ready(&workspace));

    daemon.invalidate_skill_projection(&[]).unwrap();
    assert!(daemon.skill_reconciliation_retry_ready(&workspace));
    daemon.shutdown();
    drop(daemon);
    fs::remove_dir_all(root).unwrap();
}

fn run_method(daemon: &Daemon, method: &str, params: Value) -> Result<Value, DaemonError> {
    daemon.execute_method(method, &params)
}

fn projection_domain_for_method(method: &str) -> Option<&'static str> {
    match method {
        "agents_list" => Some("agents"),
        "skills_list" => Some("skills"),
        "rules_list" => Some("rules"),
        "hooks_list" => Some("hooks"),
        "mcp_list" => Some("mcp"),
        _ => None,
    }
}

fn run_method_ok(daemon: &Daemon, method: &str, params: Value) -> Value {
    let subscription = projection_domain_for_method(method).map(|_| daemon.subscribe_events());
    let result = run_method(daemon, method, params.clone())
        .unwrap_or_else(|error| panic!("test command failed: {error:?}"));
    let Some(domain) = projection_domain_for_method(method) else {
        return result;
    };
    let store = test_store(&daemon);
    let status = store.projection_status(domain, daemon.cwd()).unwrap();
    if status == tendi_core::storage::ProjectionStatus::Fresh {
        return result;
    }
    let Some(subscription) = subscription else {
        return result;
    };
    for _ in 0..300 {
        let _ = subscription.recv_timeout(Duration::from_millis(100));
        if store.projection_status(domain, daemon.cwd()).unwrap()
            == tendi_core::storage::ProjectionStatus::Fresh
        {
            return run_method(daemon, method, params)
                .unwrap_or_else(|error| panic!("test command failed: {error:?}"));
        }
    }
    result
}

#[test]
fn prompt_save_rejects_empty_title_as_invalid_argument() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());
    let error = run_method(
        &daemon,
        "prompt_save",
        json!({
            "title": "  \n\t",
            "tags": [],
            "body": "Body"
        }),
    )
    .expect_err("empty title should fail");
    assert_eq!(error.code, "INVALID_ARGUMENT");
    assert_eq!(error.message, "missing or empty argument: title");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn bundled_skill_install_uses_requested_agent_and_defaults_to_shared() {
    assert_eq!(
        bundled_skill_agent(Some(runtime_schema::AgentKind::Claude)),
        tendi_core::AgentKind::Claude
    );
    assert_eq!(
        bundled_skill_agent(Some(runtime_schema::AgentKind::Codex)),
        tendi_core::AgentKind::Codex
    );
    assert_eq!(bundled_skill_agent(None), tendi_core::AgentKind::Shared);
}

#[cfg(unix)]
#[test]
fn skills_set_materializes_a_read_only_skill_before_provider_writes() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let source = root.join("vendor/example");
    let target = root.join(".agents/skills/example");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: example\ndescription: Example\n---\n\n# Example\n",
    )
    .unwrap();
    symlink(&source, &target).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();
    let source_before = fs::read_to_string(source.join("SKILL.md")).unwrap();

    let daemon = test_daemon(root.clone());
    let listed = run_method_ok(&daemon, "skills_list", json!({}));
    let skill_id = listed_skill_id(&listed, "example");
    run_method(
        &daemon,
        "skills_set",
        json!({
            "skillIds": [skill_id],
            "visibility": "manual",
            "dryRun": false
        }),
    )
    .unwrap();

    assert!(fs::symlink_metadata(&target).unwrap().is_dir());
    assert_eq!(
        fs::read_to_string(source.join("SKILL.md")).unwrap(),
        source_before
    );
    assert_ne!(
        fs::read_to_string(target.join("SKILL.md")).unwrap(),
        source_before
    );
    assert!(target.join("agents/openai.yaml").is_file());

    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn scheduled_backup_claim_consumes_dirty_and_releases_when_clean() {
    let dirty = AtomicBool::new(true);
    let running = AtomicBool::new(false);

    assert!(claim_scheduled_skill_backup(&dirty, &running));
    assert!(!dirty.load(Ordering::Acquire));
    assert!(running.load(Ordering::Acquire));

    running.store(false, Ordering::Release);
    assert!(!claim_scheduled_skill_backup(&dirty, &running));
    assert!(!running.load(Ordering::Acquire));
}

#[test]
fn scheduled_backup_claim_keeps_dirty_when_another_backup_is_running() {
    let dirty = AtomicBool::new(true);
    let running = AtomicBool::new(true);

    assert!(!claim_scheduled_skill_backup(&dirty, &running));
    assert!(dirty.load(Ordering::Acquire));
    assert!(running.load(Ordering::Acquire));
}

#[test]
fn sessions_scan_start_marks_current_scan_as_not_started() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());
    daemon
        .state
        .session_runtime
        .generation
        .store(7, Ordering::SeqCst);
    daemon
        .state
        .session_runtime
        .watch_revision
        .store(3, Ordering::Release);
    daemon
        .state
        .session_runtime
        .completed_revision
        .store(3, Ordering::Release);

    let result = daemon.sessions_scan_start().unwrap();

    assert_eq!(result.generation, 7);
    assert!(!result.started);
    let _ = fs::remove_dir_all(root);
}

fn hold_database_write_lock(daemon: &Daemon) -> (mpsc::Sender<()>, thread::JoinHandle<()>) {
    let store = test_store(&daemon);
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let holder = thread::spawn(move || {
        let conn = rusqlite::Connection::open(store.path()).unwrap();
        conn.busy_timeout(Duration::from_secs(5)).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        acquired_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        conn.execute_batch("COMMIT").unwrap();
    });
    acquired_rx.recv().unwrap();
    (release_tx, holder)
}

#[test]
fn skill_file_round_trip_and_conflict_are_protocol_errors() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());
    let listed = run_method_ok(&daemon, "skills_list", json!({}));
    let demo_id = listed_skill_id(&listed, "demo");
    assert!(listed[0]["paths"][0]["locationId"].as_str().is_some());
    let _files = run_method_ok(
        &daemon,
        "skill_files",
        json!({ "skillId": demo_id.clone() }),
    );
    let read = run_method_ok(
        &daemon,
        "skill_file_read",
        json!({ "skillId": demo_id.clone(), "relativePath": "SKILL.md" }),
    );
    let sha = read["sha256"].as_str().unwrap().to_string();
    let saved = run_method_ok(
        &daemon,
        "skill_file_save",
        json!({ "skillId": demo_id.clone(), "relativePath": "SKILL.md", "expectedSha256": sha, "content": "updated" }),
    );
    assert!(saved["content"].is_null());
    assert!(saved["skills"].is_array());
    assert_eq!(saved["sha256"].as_str().unwrap().len(), 64);
    let notes = run_method_ok(
        &daemon,
        "skill_file_create",
        json!({ "skillId": demo_id.clone(), "relativePath": "notes.md" }),
    );
    assert!(notes["files"].is_array());
    assert!(notes["content"].is_null());
    assert!(notes["skills"].is_null());
    let conflict = run_method(
            &daemon,
            "skill_file_save",
            json!({ "skillId": demo_id, "relativePath": "SKILL.md", "expectedSha256": "stale", "content": "bad" }),
        )
            .expect_err("stale skill file save should conflict");
    assert_eq!(conflict.code, "CONFLICT");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_file_reads_refresh_when_selected_skill_is_added_after_listing() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());
    let _listed = run_method_ok(&daemon, "skills_list", json!({}));

    let added_skill = root.join(".agents/skills/added");
    fs::create_dir_all(&added_skill).unwrap();
    fs::write(
        added_skill.join("SKILL.md"),
        "---\nname: added\ndescription: Added\n---\n\n# Added\n",
    )
    .unwrap();
    let added_id = format!(
        "skill@path:{}",
        added_skill.canonicalize().unwrap().display()
    );

    let files = run_method_ok(&daemon, "skill_files", json!({ "skillId": added_id }));
    assert!(
        files
            .as_array()
            .is_some_and(|files| { files.iter().any(|file| file["relative_path"] == "SKILL.md") }),
        "unexpected files response: {files}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_file_changes_emit_a_runtime_event() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());
    let _listed = run_method_ok(&daemon, "skills_list", json!({}));
    let subscription = daemon.subscribe_events();
    let skill_file = root.join(".agents/skills/demo/SKILL.md");
    fs::write(
        &skill_file,
        "---\nname: demo\ndescription: Changed\n---\n\n# Changed\n",
    )
    .unwrap();

    let event = subscription
        .recv_timeout(Duration::from_secs(3))
        .expect("skill file change should emit a runtime event");
    assert_eq!(event.event, SKILL_CHANGED_EVENT);
    let skill_dir = skill_file.parent().unwrap();
    let canonical_skill_dir = skill_dir.canonicalize().unwrap();
    assert!(
        event.payload["paths"].as_array().is_some_and(|paths| {
            paths.iter().filter_map(|path| path.as_str()).any(|path| {
                Path::new(path)
                    .canonicalize()
                    .is_ok_and(|path| path.starts_with(&canonical_skill_dir))
            })
        }),
        "unexpected skill change paths: {}",
        event.payload["paths"]
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn direct_reads_do_not_wait_for_database_write_lock() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());
    let listed = run_method_ok(&daemon, "skills_list", json!({}));
    let demo_id = listed_skill_id(&listed, "demo");
    let requests = [
        ("settings_get", json!({})),
        ("skill_session_links", json!({ "skillId": demo_id.clone() })),
        ("skills_targets", json!({})),
        ("skill_files", json!({ "skillId": demo_id.clone() })),
        (
            "skill_file_read",
            json!({
                "skillId": demo_id,
                "relativePath": "SKILL.md",
            }),
        ),
    ];
    for (method, params) in &requests {
        let response = run_method(&daemon, method, params.clone());
        assert!(response.is_ok(), "warm-up response: {response:?}");
    }
    let (release_tx, holder) = hold_database_write_lock(&daemon);

    for (method, params) in requests {
        let request_daemon = daemon.clone();
        let (response_tx, response_rx) = mpsc::channel();
        let request_thread = thread::spawn(move || {
            response_tx
                .send(run_method(&request_daemon, method, params))
                .unwrap();
        });
        let response = response_rx.recv_timeout(Duration::from_secs(5));
        request_thread.join().unwrap();
        let response = response.unwrap_or_else(|error| {
            panic!("request {method} waited for the database authority: {error}")
        });
        assert!(response.is_ok(), "response: {response:?}");
    }

    release_tx.send(()).unwrap();
    holder.join().unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn http_connections_join_when_daemon_shuts_down() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server_daemon = daemon.clone();
    let server = thread::spawn(move || run_http(server_daemon, listener, None));

    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.contains("\"ok\":true"));
    assert!(response.contains(&format!(
        "\"contractFingerprint\":\"{}\"",
        runtime_schema::RUNTIME_CONTRACT_FINGERPRINT
    )));

    daemon.shutdown();
    server.join().unwrap().unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn accepted_http_connections_switch_back_to_blocking_mode() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server_daemon = daemon.clone();
    let server = thread::spawn(move || run_http(server_daemon, listener, None));

    let mut stream = TcpStream::connect(address).unwrap();
    stream.write_all(b"GET /health HTTP/1.1\r\n").unwrap();
    thread::sleep(Duration::from_millis(50));
    stream
        .write_all(b"Host: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();

    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.contains("\"ok\":true"));

    daemon.shutdown();
    server.join().unwrap().unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn http_response_writes_all_bytes_after_short_writes() {
    let body = "x".repeat(1024 * 1024);
    let mut writer = ShortWriter {
        bytes: Vec::new(),
        max_write: 3,
    };

    write_http(&mut writer, 200, &body).unwrap();

    let response = String::from_utf8(writer.bytes).unwrap();
    assert!(response.ends_with(&body));
    assert!(response.contains(&format!("content-length: {}", body.len())));
}

#[test]
fn skill_update_preview_ignores_unrelated_skill_changes() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let unrelated_skill_file = root.join(".agents/skills/.system/unrelated/SKILL.md");
    let unrelated_user_skill_file = root.join(".agents/skills/unrelated-user/SKILL.md");
    fs::create_dir_all(unrelated_skill_file.parent().unwrap()).unwrap();
    fs::create_dir_all(unrelated_user_skill_file.parent().unwrap()).unwrap();
    fs::write(
        &unrelated_skill_file,
        "---\nname: unrelated\ndescription: Unrelated system skill\n---\n\n# Unrelated\n",
    )
    .unwrap();
    fs::write(
        &unrelated_user_skill_file,
        "---\nname: unrelated-user\ndescription: Unrelated user skill\n---\n\n# Unrelated user\n",
    )
    .unwrap();
    let daemon = test_daemon(root.clone());
    let listed = run_method_ok(&daemon, "skills_list", json!({}));
    let demo_id = listed_skill_id(&listed, "demo");

    fs::write(
        &unrelated_skill_file,
        "---\nname: unrelated\ndescription: Externally updated system skill\n---\n\n# Unrelated\n",
    )
    .unwrap();
    fs::write(
            &unrelated_user_skill_file,
            "---\nname: unrelated-user\ndescription: Externally updated user skill\n---\n\n# Unrelated user\n",
        )
        .unwrap();
    let preview = run_method_ok(
        &daemon,
        "skills_update_many",
        json!({ "skillIds": [demo_id.clone()], "dryRun": true }),
    );

    assert_eq!(preview["canApply"], false);
    assert!(preview["previewId"].is_null());
    let apply = run_method(
        &daemon,
        "skills_update_many",
        json!({ "skillIds": [demo_id], "dryRun": false }),
    )
    .expect_err("unrelated skill changes should conflict");
    assert_eq!(apply.code, "CONFLICT");
    let store = test_store(&daemon);
    for _ in 0..100 {
        if store.projection_status("skills", &root).unwrap()
            == tendi_core::storage::ProjectionStatus::Stale
        {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        store.projection_status("skills", &root).unwrap(),
        tendi_core::storage::ProjectionStatus::Stale
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_update_preview_refreshes_when_selected_skill_changes() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let selected_skill_file = root.join(".agents/skills/demo/SKILL.md");
    let daemon = test_daemon(root.clone());
    let listed = run_method_ok(&daemon, "skills_list", json!({}));
    let demo_id = listed_skill_id(&listed, "demo");

    fs::write(
        &selected_skill_file,
        "---\nname: demo\ndescription: Selected skill changed\n---\n\n# Demo\n",
    )
    .unwrap();
    let _preview = run_method_ok(
        &daemon,
        "skills_update_many",
        json!({ "skillIds": [demo_id], "dryRun": true }),
    );

    let store = test_store(&daemon);
    for _ in 0..100 {
        if store.projection_status("skills", &root).unwrap()
            == tendi_core::storage::ProjectionStatus::Stale
        {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        store.projection_status("skills", &root).unwrap(),
        tendi_core::storage::ProjectionStatus::Stale
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_delete_many_applies_without_a_preview() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let skill_dir = root.join(".agents/skills/demo");
    let daemon = test_daemon(root.clone());
    let listed = run_method_ok(&daemon, "skills_list", json!({}));
    let demo_id = listed_skill_id(&listed, "demo");

    let response = run_method_ok(
        &daemon,
        "skills_delete_many",
        json!({ "skillIds": [demo_id.clone()] }),
    );
    assert!(!skill_dir.exists());
    assert_eq!(response["deleted"], json!([demo_id]));
    assert!(response["skills"].is_null());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_delete_many_refreshes_stale_projection_before_mutation() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());
    let _listed = run_method_ok(&daemon, "skills_list", json!({}));

    let added_skill = root.join(".agents/skills/added");
    fs::create_dir_all(&added_skill).unwrap();
    fs::write(
        added_skill.join("SKILL.md"),
        "---\nname: added\ndescription: Added\n---\n\n# Added\n",
    )
    .unwrap();
    let added_id = format!(
        "skill@path:{}",
        added_skill.canonicalize().unwrap().display()
    );

    let response = run_method_ok(
        &daemon,
        "skills_delete_many",
        json!({ "skillIds": [added_id.clone()] }),
    );

    assert!(!added_skill.exists());
    assert_eq!(response["deleted"], json!([added_id]));
    daemon.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn rule_file_delete_many_refreshes_the_projection() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let rule_path = root.join("AGENTS.md");
    fs::write(&rule_path, "delete me").expect("write rule");
    let daemon = test_daemon(root.clone());

    let _listed = run_method_ok(&daemon, "rules_list", json!({}));
    let response = run_method_ok(
        &daemon,
        "rule_file_delete_many",
        json!({ "paths": [rule_path] }),
    );

    assert!(!rule_path.exists());
    let rule_path_text = rule_path.to_string_lossy();
    assert_eq!(response["deleted"], json!([rule_path_text.as_ref()]));
    let listed_after = run_method_ok(&daemon, "rules_list", json!({}));
    assert!(
        !listed_after
            .as_array()
            .expect("rules response should be an array")
            .iter()
            .any(|rule| rule["path"].as_str() == Some(rule_path_text.as_ref()))
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_distribution_moves_multiple_skills_in_one_preview() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let second = root.join(".agents/skills/second");
    fs::create_dir_all(&second).unwrap();
    fs::write(
        second.join("SKILL.md"),
        "---\nname: second\ndescription: Second\n---\n\n# Second\n",
    )
    .unwrap();
    let first_source = root.join(".agents/skills/demo");
    let second_source = second.clone();
    let daemon = test_daemon(root.clone());
    let _listed = run_method_ok(&daemon, "skills_list", json!({}));
    let preview = run_method_ok(
        &daemon,
        "skills_distribute",
        json!({
                "sourcePaths": [first_source, second_source],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "dryRun": true
        }),
    );
    assert_eq!(preview["plans"].as_array().unwrap().len(), 2);
    let preview_id = preview["previewId"].as_str().unwrap();

    let applied = run_method_ok(
        &daemon,
        "skills_distribute",
        json!({
                "sourcePaths": [root.join(".agents/skills/demo"), second],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "previewId": preview_id,
                "dryRun": false
        }),
    );
    assert_eq!(applied["results"].as_array().unwrap().len(), 2);
    assert!(!root.join(".agents/skills/demo").exists());
    assert!(!root.join(".agents/skills/second").exists());
    assert!(root.join(".claude/skills/demo/SKILL.md").is_file());
    assert!(root.join(".claude/skills/second/SKILL.md").is_file());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_distribution_moves_once_and_links_additional_targets() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let source = root.join(".agents/skills/demo");
    let codex = root.join(".codex/skills/demo");
    let cursor = root.join(".cursor/skills/demo");
    let daemon = test_daemon(root.clone());
    let _listed = run_method_ok(&daemon, "skills_list", json!({}));

    let applied = run_method_ok(
        &daemon,
        "skills_distribute",
        json!({
            "sourcePaths": [source.clone()],
            "targets": ["codex", "cursor"],
            "scope": "project",
            "mode": "move",
            "dryRun": false
        }),
    );

    assert_eq!(applied["results"].as_array().unwrap().len(), 2);
    assert!(!source.exists());
    assert!(codex.join("SKILL.md").is_file());
    assert!(
        !fs::symlink_metadata(&codex)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(cursor.join("SKILL.md").is_file());
    assert!(
        fs::symlink_metadata(&cursor)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        cursor.canonicalize().unwrap(),
        codex.canonicalize().unwrap()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_distribution_applies_without_preview() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let source = root.join(".agents/skills/demo");
    let daemon = test_daemon(root.clone());
    let _listed = run_method_ok(&daemon, "skills_list", json!({}));

    let applied = run_method_ok(
        &daemon,
        "skills_distribute",
        json!({
                "sourcePaths": [source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "dryRun": false
        }),
    );

    assert_eq!(applied["plans"][0]["mode"], "move");
    assert!(!source.exists());
    assert!(root.join(".claude/skills/demo/SKILL.md").is_file());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_remove_locations_deletes_only_selected_target() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let source = root.join(".agents/skills/demo");
    let target = root.join(".claude/skills/demo");
    let daemon = test_daemon(root.clone());
    let _listed = run_method_ok(&daemon, "skills_list", json!({}));

    let distributed = run_method_ok(
        &daemon,
        "skills_distribute",
        json!({
                "sourcePaths": [source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "copy",
                "dryRun": false
        }),
    );
    assert!(source.exists());
    assert!(target.exists());
    assert!(
        distributed["updated"]
            .as_array()
            .unwrap()
            .iter()
            .any(|skill| skill["name"] == "demo"),
        "distribution response did not include demo: {distributed}"
    );
    let target_path = target.to_string_lossy();
    let target_id = distributed["updated"]
        .as_array()
        .and_then(|skills| {
            skills.iter().find(|skill| {
                skill["paths"].as_array().is_some_and(|paths| {
                    paths
                        .iter()
                        .any(|path| path["path"].as_str() == Some(target_path.as_ref()))
                })
            })
        })
        .and_then(|skill| skill["id"].as_str())
        .expect("distribution should expose the target installation id")
        .to_string();
    let removed = run_method_ok(
        &daemon,
        "skills_remove_locations",
        json!({
                "skillIds": [target_id],
                "targets": ["claude-code"],
                "scope": "project"
        }),
    );
    assert!(source.exists());
    assert!(!target.exists(), "remove response: {removed}");
    assert_eq!(removed["plan"]["targets"].as_array().unwrap().len(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_remove_locations_rehomes_canonical_installation() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let source = root.join(".agents/skills/demo");
    let codex = root.join(".codex/skills/demo");
    let cursor = root.join(".cursor/skills/demo");
    let daemon = test_daemon(root.clone());
    let _listed = run_method_ok(&daemon, "skills_list", json!({}));

    run_method_ok(
        &daemon,
        "skills_distribute",
        json!({
            "sourcePaths": [source.clone()],
            "targets": ["codex", "cursor"],
            "scope": "project",
            "mode": "symlink",
            "dryRun": false
        }),
    );
    assert!(source.join("SKILL.md").is_file());
    assert!(
        fs::symlink_metadata(&codex)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        fs::symlink_metadata(&cursor)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let listed = run_method_ok(&daemon, "skills_list", json!({}));
    let skill_id = listed_skill_id(&listed, "demo");
    let removed = run_method_ok(
        &daemon,
        "skills_remove_locations",
        json!({
            "skillIds": [skill_id],
            "targets": ["shared"],
            "scope": "project"
        }),
    );

    assert!(!source.exists(), "remove response: {removed}");
    assert!(codex.join("SKILL.md").is_file());
    assert!(
        !fs::symlink_metadata(&codex)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(cursor.join("SKILL.md").is_file());
    assert!(
        fs::symlink_metadata(&cursor)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        cursor.canonicalize().unwrap(),
        codex.canonicalize().unwrap()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn wrapper_syncs_child_content_location_and_deletion() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let child_dir = root.join(".agents/skills/child");
    let wrapper_dir = root.join(".agents/skills/wrapper");
    fs::create_dir_all(&child_dir).unwrap();
    fs::create_dir_all(&wrapper_dir).unwrap();
    fs::write(
        child_dir.join("SKILL.md"),
        "---\nname: child\ndescription: Original child\n---\n\n# Child\n",
    )
    .unwrap();
    fs::write(
            wrapper_dir.join("SKILL.md"),
            format!(
                "---\nname: wrapper\ndescription: Wrapper\n---\n\n# Wrapper\n\n## Route\n\n- [`child`](<{}>): Original child\n",
                child_dir.join("SKILL.md").display()
            ),
        )
        .unwrap();

    let daemon = test_daemon(root.clone());
    let listed = run_method_ok(&daemon, "skills_list", json!({}));
    let wrapper = listed
        .as_array()
        .and_then(|skills| skills.iter().find(|skill| skill["name"] == "wrapper"))
        .expect("wrapper should be listed");
    assert_eq!(wrapper["is_wrapper"], json!(true));
    let child_id = listed_skill_id(&listed, "child");
    let read = run_method_ok(
        &daemon,
        "skill_file_read",
        json!({ "skillId": child_id.clone(), "relativePath": "SKILL.md" }),
    );
    run_method_ok(
        &daemon,
        "skill_file_save",
        json!({
            "skillId": child_id,
            "relativePath": "SKILL.md",
            "expectedSha256": read["sha256"],
            "content": "---\nname: child\ndescription: Updated child\n---\n\n# Child\n"
        }),
    );
    let wrapper_file = wrapper_dir.join("SKILL.md");
    let wrapper_after_save = fs::read_to_string(&wrapper_file).unwrap();
    assert!(wrapper_after_save.contains("Updated child"));

    let distributed = run_method_ok(
        &daemon,
        "skills_distribute",
        json!({
            "sourcePaths": [child_dir.clone()],
            "target": "claude-code",
            "scope": "project",
            "mode": "move",
            "dryRun": false
        }),
    );
    let target_dir = root.join(".claude/skills/child");
    assert!(!child_dir.exists());
    assert!(target_dir.join("SKILL.md").is_file());
    let target_path = target_dir.to_string_lossy();
    let target_id = distributed["updated"]
        .as_array()
        .and_then(|skills| {
            skills.iter().find(|skill| {
                skill["paths"].as_array().is_some_and(|paths| {
                    paths
                        .iter()
                        .any(|path| path["path"].as_str() == Some(target_path.as_ref()))
                })
            })
        })
        .and_then(|skill| skill["id"].as_str())
        .expect("distribution should expose the moved child installation id")
        .to_string();
    let wrapper_after_move = fs::read_to_string(&wrapper_file).unwrap();
    assert!(
        wrapper_after_move.contains(&target_dir.join("SKILL.md").display().to_string()),
        "wrapper after move: {wrapper_after_move}"
    );
    assert!(!wrapper_after_move.contains(&child_dir.join("SKILL.md").display().to_string()));
    assert!(wrapper_after_move.contains("Updated child"));

    run_method_ok(
        &daemon,
        "skills_remove_locations",
        json!({
            "skillIds": [target_id],
            "targets": ["claude-code"],
            "scope": "project"
        }),
    );
    assert!(!target_dir.exists());
    let wrapper_after_delete = fs::read_to_string(wrapper_file).unwrap();
    assert!(!wrapper_after_delete.contains("[`child`]"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_distribution_allows_mode_change_after_preview() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let source = root.join(".agents/skills/demo");
    let daemon = test_daemon(root.clone());
    let _listed = run_method_ok(&daemon, "skills_list", json!({}));

    let preview = run_method_ok(
        &daemon,
        "skills_distribute",
        json!({
                "sourcePaths": [source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "symlink",
                "dryRun": true
        }),
    );
    let preview_id = preview["previewId"].as_str().unwrap();

    let applied = run_method_ok(
        &daemon,
        "skills_distribute",
        json!({
                "sourcePaths": [source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "previewId": preview_id,
                "dryRun": false
        }),
    );
    assert_eq!(applied["plans"][0]["mode"], "move");
    assert!(!source.exists());
    assert!(root.join(".claude/skills/demo/SKILL.md").is_file());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_distribution_allows_same_path_alongside_moved_skill() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let same_path = root.join(".claude/skills/demo");
    fs::create_dir_all(&same_path).unwrap();
    fs::write(
        same_path.join("SKILL.md"),
        "---\nname: demo\ndescription: Demo\n---\n\n# Demo\n",
    )
    .unwrap();
    let moved_source = root.join(".agents/skills/second");
    fs::create_dir_all(&moved_source).unwrap();
    fs::write(
        moved_source.join("SKILL.md"),
        "---\nname: second\ndescription: Second\n---\n\n# Second\n",
    )
    .unwrap();
    let daemon = test_daemon(root.clone());
    let _listed = run_method_ok(&daemon, "skills_list", json!({}));

    let preview = run_method_ok(
        &daemon,
        "skills_distribute",
        json!({
                "sourcePaths": [same_path.clone(), moved_source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "dryRun": true
        }),
    );
    let plans = preview["plans"].as_array().unwrap();
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0]["status"], "already-at-destination");
    assert_eq!(plans[1]["status"], "ready");
    let preview_id = preview["previewId"].as_str().unwrap();

    let _applied = run_method_ok(
        &daemon,
        "skills_distribute",
        json!({
                "sourcePaths": [same_path.clone(), moved_source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "previewId": preview_id,
                "dryRun": false
        }),
    );
    assert!(!moved_source.exists());
    assert!(same_path.join("SKILL.md").is_file());
    assert!(root.join(".claude/skills/second/SKILL.md").is_file());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_mutations_refresh_the_projection_without_a_full_rescan() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let skill_dir = root.join(".agents/skills/demo");
    let daemon = test_daemon(root.clone());
    let listed = run_method_ok(&daemon, "skills_list", json!({}));
    let demo_id = listed_skill_id(&listed, "demo");

    let _visibility = run_method_ok(
        &daemon,
        "skills_set",
        json!({ "skillIds": [demo_id], "visibility": "manual" }),
    );
    let store = test_store(&daemon);
    let visibility = store
        .skill_visibilities_for_workspace(&root)
        .unwrap()
        .get(&skill_dir.canonicalize().unwrap())
        .copied();
    assert_eq!(visibility, Some(tendi_core::SkillVisibility::Manual));
    assert!(
        fs::read_to_string(skill_dir.join("SKILL.md"))
            .unwrap()
            .contains("disable-model-invocation: true")
    );
    assert!(
        fs::read_to_string(skill_dir.join("agents/openai.yaml"))
            .unwrap()
            .contains("allow_implicit_invocation: false")
    );

    let _folder = run_method_ok(
        &daemon,
        "skill_folder_create",
        json!({ "skillId": demo_id.clone(), "relativePath": "references" }),
    );
    let _file = run_method_ok(
        &daemon,
        "skill_file_create",
        json!({ "skillId": demo_id.clone(), "relativePath": "references/notes.md" }),
    );
    let _renamed = run_method_ok(
        &daemon,
        "skill_path_rename",
        json!({
                "skillId": demo_id.clone(),
                "fromRelativePath": "references/notes.md",
                "toRelativePath": "references/renamed.md"
        }),
    );
    let _deleted = run_method_ok(
        &daemon,
        "skill_path_delete",
        json!({ "skillId": demo_id, "relativePath": "references/renamed.md" }),
    );
    assert!(!skill_dir.join("references/renamed.md").exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn mcp_toggle_updates_selected_cached_row_without_full_rescan() {
    let path = PathBuf::from("/tmp/tendi-mcp-config.json");
    let other_path = PathBuf::from("/tmp/tendi-other-mcp-config.json");
    let mut scan = tendi_core::mcp::McpScan {
        servers: vec![
            tendi_core::mcp::McpServerRecord {
                agent: tendi_core::AgentKind::Claude,
                name: "demo".to_string(),
                scope: "global".to_string(),
                transport: "stdio".to_string(),
                enabled: true,
                status: "configured".to_string(),
                path: path.clone(),
                trust_hash: "old-demo".to_string(),
                probe_cache_version: tendi_core::mcp::MCP_PROBE_CACHE_VERSION,
                probe_state: tendi_core::mcp::McpProbeState::Unknown,
                server_path: Vec::new(),
                read_only_reason: None,
                server_name: None,
                server_title: None,
                server_version: None,
                server_description: None,
                server_website_url: None,
                probe_error: None,
                icons: Vec::new(),
                tools: Vec::new(),
            },
            tendi_core::mcp::McpServerRecord {
                agent: tendi_core::AgentKind::Claude,
                name: "other".to_string(),
                scope: "global".to_string(),
                transport: "stdio".to_string(),
                enabled: true,
                status: "configured".to_string(),
                path: other_path,
                trust_hash: "old-other".to_string(),
                probe_cache_version: tendi_core::mcp::MCP_PROBE_CACHE_VERSION,
                probe_state: tendi_core::mcp::McpProbeState::Unknown,
                server_path: Vec::new(),
                read_only_reason: None,
                server_name: None,
                server_title: None,
                server_version: None,
                server_description: None,
                server_website_url: None,
                probe_error: None,
                icons: Vec::new(),
                tools: Vec::new(),
            },
        ],
        warnings: Vec::new(),
    };
    let request = tendi_core::mcp::McpSetEnabledRequest {
        agent: tendi_core::AgentKind::Claude,
        path,
        expected_trust_hash: "old-demo".to_string(),
        name: "demo".to_string(),
        enabled: false,
        server_path: Vec::new(),
    };

    update_mcp_projection_for_toggle(&mut scan, &request, "new-demo".to_string()).unwrap();

    assert_eq!(scan.servers[0].status, "disabled");
    assert!(!scan.servers[0].enabled);
    assert_eq!(scan.servers[0].trust_hash, "new-demo");
    assert_eq!(scan.servers[1].status, "configured");
    assert!(scan.servers[1].enabled);
    assert_eq!(scan.servers[1].trust_hash, "old-other");
}

#[test]
fn unknown_method_is_explicit() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let daemon = test_daemon(temp_workspace());
    let error =
        run_method(&daemon, "not_implemented", json!({})).expect_err("unknown command should fail");
    assert_eq!(error.code, "METHOD_NOT_FOUND");
}

#[test]
fn event_subscription_preserves_shared_envelope() {
    let hub = EventHub {
        next_id: Arc::new(AtomicU64::new(0)),
        state: Arc::new(Mutex::new(EventHubState::default())),
    };
    let subscription = hub.subscribe();
    hub.publish_with_metadata(
        "analytics://revision",
        json!({ "scopeKey": "test", "revision": 42 }),
        None,
        None,
        None,
        None,
        None,
        None,
    );
    let event = subscription
        .recv_timeout(Duration::from_secs(1))
        .expect("event should be delivered");
    assert_eq!(event.id, 1);
    assert_eq!(event.event, "analytics://revision");
    assert_eq!(event.payload["revision"], 42);
}

#[test]
fn event_subscription_replays_events_after_last_event_id() {
    let hub = EventHub {
        next_id: Arc::new(AtomicU64::new(0)),
        state: Arc::new(Mutex::new(EventHubState::default())),
    };
    hub.publish_with_metadata(
        "analytics://revision",
        json!({ "scopeKey": "test", "revision": 1 }),
        None,
        None,
        None,
        None,
        None,
        None,
    );
    hub.publish_with_metadata(
        "analytics://revision",
        json!({ "scopeKey": "test", "revision": 2 }),
        None,
        None,
        None,
        None,
        None,
        None,
    );

    let subscription = hub.subscribe_from(Some(1));
    let event = subscription
        .recv_timeout(Duration::from_secs(1))
        .expect("the missed event should be replayed");
    assert_eq!(event.id, 2);
    assert_eq!(event.payload["revision"], 2);
    assert!(matches!(
        subscription.recv_timeout(Duration::from_millis(20)),
        Err(RecvTimeoutError::Timeout)
    ));
}

#[test]
fn projection_refresh_event_carries_domain_metadata() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());
    let subscription = daemon.subscribe_events();
    daemon.emit_event(
        PROJECTION_CHANGED_EVENT,
        runtime_event(
            PROJECTION_CHANGED_EVENT,
            json!({ "domain": "rules", "error": Value::Null }),
        ),
    );

    let event = subscription
        .recv_timeout(Duration::from_secs(1))
        .expect("projection refresh event should be delivered");
    assert_eq!(event.event, PROJECTION_CHANGED_EVENT);
    assert_eq!(event.domain.as_deref(), Some("rules"));
    assert_eq!(event.payload["domain"], "rules");
    daemon.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_watch_retry_state_keeps_dirty_paths_until_success() {
    let (watch_tx, _watch_rx) = mpsc::channel();
    let (analytics_tx, _analytics_rx) = mpsc::channel();
    let runtime = SessionRuntime {
        generation: AtomicU64::new(0),
        scan_running: AtomicBool::new(false),
        watch_revision: AtomicU64::new(0),
        completed_revision: AtomicU64::new(0),
        watcher: Mutex::new(SessionWatcherState::default()),
        retry: Mutex::new(SessionWatchRetryState::default()),
        watch_tx,
        analytics_tx,
    };
    let path = PathBuf::from("/tmp/tendi-session-watch-retry.jsonl");

    schedule_session_watch_retry(&runtime, std::slice::from_ref(&path));
    {
        let retry = runtime.retry.lock().unwrap();
        assert!(retry.paths.contains(&path));
        assert_eq!(retry.delay, SESSION_WATCH_RETRY_INITIAL * 2);
    }

    runtime.retry.lock().unwrap().retry_at = Some(Instant::now());
    assert_eq!(
        take_due_session_watch_retries(&runtime),
        Some(vec![path.clone()])
    );
    complete_session_watch_paths(&runtime, std::slice::from_ref(&path));

    let retry = runtime.retry.lock().unwrap();
    assert!(retry.paths.is_empty());
    assert!(retry.retry_at.is_none());
    assert_eq!(retry.delay, SESSION_WATCH_RETRY_INITIAL);
}

#[test]
fn live_session_watch_preview_includes_assistant_reply() {
    let root = temp_workspace();
    let path = root.join("rollout-12345678-1234-1234-1234-123456789012.jsonl");
    fs::write(
            &path,
            [
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Question"}]}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Answer"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

    let sessions = live_session_watch_previews(
        std::slice::from_ref(&path),
        &tendi_core::sessions::SessionScanCache::default(),
    );

    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].last_assistant_message.as_deref(),
        Some("Answer")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn analytics_refresh_completes_while_session_lane_is_blocked() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());
    let scope_key = daemon_scope_key(&daemon).unwrap();
    let warmed = run_method_ok(&daemon, "skills_backup_status", json!({}));
    assert!(warmed.is_object());
    let subscription = daemon.subscribe_events();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    daemon
        .state
        .session_operations
        .submit(
            tendi_core::OperationId::new("test-session-blocker")
                .expect("test operation id is valid"),
            move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            },
        )
        .unwrap();
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("session lane blocker should start");

    let analytics_daemon = daemon.clone();
    let analytics_scope = scope_key.clone();
    let (done_tx, done_rx) = mpsc::channel();
    let analytics = thread::spawn(move || {
        let result =
            refresh_session_analytics_serialized(&analytics_daemon, "test", &analytics_scope, &[]);
        done_tx.send(result).unwrap();
    });
    let progress = subscription
        .recv_timeout(Duration::from_secs(1))
        .expect("analytics refresh should start before waiting on its lane");
    assert_eq!(progress.event, ANALYTICS_PROGRESS_EVENT);

    let completed_before_session_release = done_rx.recv_timeout(Duration::from_secs(10));
    release_tx.send(()).unwrap();
    analytics.join().unwrap();
    let report = completed_before_session_release
        .expect("analytics should complete while session lane is blocked")
        .expect("analytics refresh should succeed");
    assert_eq!(report.total, 0);
    daemon.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn foreground_rpc_runs_while_analytics_lane_is_blocked() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());
    let scope_key = daemon_scope_key(&daemon).unwrap();
    let subscription = daemon.subscribe_events();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    daemon
        .state
        .analytics_operations
        .submit(
            tendi_core::OperationId::new("test-analytics-blocker")
                .expect("test operation id is valid"),
            move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            },
        )
        .unwrap();
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("analytics lane blocker should start");

    let analytics_daemon = daemon.clone();
    let analytics_scope = scope_key.clone();
    let (done_tx, done_rx) = mpsc::channel();
    let analytics = thread::spawn(move || {
        let result =
            refresh_session_analytics_serialized(&analytics_daemon, "test", &analytics_scope, &[]);
        done_tx.send(result).unwrap();
    });
    let progress = subscription
        .recv_timeout(Duration::from_secs(1))
        .expect("analytics refresh should start before waiting on its lane");
    assert_eq!(progress.event, ANALYTICS_PROGRESS_EVENT);

    let foreground = run_method_ok(&daemon, "skills_backup_status", json!({}));
    assert!(foreground.is_object());

    release_tx.send(()).unwrap();
    let report = done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("analytics refresh should finish after release")
        .expect("analytics refresh should complete");
    analytics.join().unwrap();
    assert_eq!(report.total, 0);

    daemon.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn foreground_rpc_runs_while_maintenance_lane_is_blocked() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());
    let warmed = run_method_ok(&daemon, "skills_backup_status", json!({}));
    assert!(warmed.is_object());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    daemon
        .state
        .requests
        .submit(
            tendi_core::OperationId::new("test-maintenance-blocker")
                .expect("test operation id is valid"),
            request_scheduler::Step::acquire(
                request_scheduler::Workload::ExternalIo,
                Vec::new(),
                move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(request_scheduler::Step::Complete(()))
                },
            ),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("maintenance lane blocker should start");

    let foreground = run_method_ok(&daemon, "skills_backup_status", json!({}));
    assert!(foreground.is_object());

    release_tx.send(()).unwrap();
    daemon.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn projection_read_does_not_block_foreground_rpc() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());
    let warmed = run_method_ok(&daemon, "skills_backup_status", json!({}));
    assert!(warmed.is_object());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    daemon
        .state
        .requests
        .submit(
            tendi_core::OperationId::new("test-projection-blocker")
                .expect("test operation id is valid"),
            request_scheduler::Step::acquire(
                request_scheduler::Workload::Compute,
                Vec::new(),
                move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(request_scheduler::Step::Complete(()))
                },
            ),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("projection lane blocker should start");

    let read_daemon = daemon.clone();
    let (read_tx, read_rx) = mpsc::channel();
    let projection_read = thread::spawn(move || {
        read_tx
            .send(
                run_method(&read_daemon, "agents_list", json!({}))
                    .unwrap_or_else(|error| panic!("test command failed: {error:?}")),
            )
            .unwrap();
    });

    let projection_result = read_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("cached projection read should not wait for its refresh lane");
    assert!(projection_result.is_array());

    let foreground = run_method_ok(&daemon, "skills_backup_status", json!({}));
    assert!(foreground.is_object());

    release_tx.send(()).unwrap();
    projection_read.join().unwrap();
    daemon.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn store_write_waits_for_another_sqlite_transaction() {
    let root = temp_workspace();
    let db = root.join("tendi.sqlite3");
    let store_a = tendi_core::storage::Store::open(&db).unwrap();
    let store_b = tendi_core::storage::Store::open(&db).unwrap();
    let settings = store_b.app_settings().unwrap();
    let expected_appearance = settings.appearance.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let holder = thread::spawn(move || {
        let conn = rusqlite::Connection::open(store_a.path()).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        conn.execute_batch("COMMIT").unwrap();
    });
    started_rx.recv().unwrap();

    let contender = thread::spawn(move || store_b.save_app_settings(settings).unwrap());
    thread::sleep(Duration::from_millis(100));
    assert!(
        !contender.is_finished(),
        "write must wait for the SQLite owner"
    );
    release_tx.send(()).unwrap();

    assert_eq!(contender.join().unwrap().appearance, expected_appearance);
    holder.join().unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn transcript_read_does_not_wait_for_database_write_lock() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = temp_workspace();
    let transcript = root.join("session.jsonl");
    fs::write(
            &transcript,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}"#,
        )
        .unwrap();
    let daemon = test_daemon_without_background(root.clone());
    let store = test_store(&daemon);
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let holder = thread::spawn(move || {
        let conn = rusqlite::Connection::open(store.path()).unwrap();
        conn.busy_timeout(Duration::from_secs(5)).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        acquired_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        conn.execute_batch("COMMIT").unwrap();
    });
    acquired_rx.recv().unwrap();

    let request_daemon = daemon.clone();
    let path = transcript.display().to_string();
    let (response_tx, response_rx) = mpsc::channel();
    let request = thread::spawn(move || {
        let response = run_method(
            &request_daemon,
            "session_transcript",
            json!({ "path": path, "agent": "codex", "limit": 1 }),
        );
        response_tx.send(response).unwrap();
    });
    let response = response_rx.recv_timeout(Duration::from_millis(300));
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    request.join().unwrap();

    let response = response
        .expect("transcript read should not wait for database write lock")
        .expect("transcript read should succeed");
    assert_eq!(response["items"].as_array().unwrap().len(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn json_rpc_boundary_uses_generated_envelope_and_numeric_errors() {
    let root = temp_workspace();
    let daemon = test_daemon(root.clone());

    let response = daemon.handle_json_rpc(json!({
        "jsonrpc": "2.0",
        "id": "test-1",
        "method": "unknown_method",
        "params": {}
    }));
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], "test-1");
    assert_eq!(response["error"]["code"], -32601);
    assert_eq!(response["error"]["data"]["kind"], "METHOD_NOT_FOUND");
    assert!(response.get("ok").is_none());

    let response = daemon.handle_json_rpc(json!({
        "jsonrpc": "2.0",
        "id": "test-2",
        "method": "sessions_snapshot",
        "params": { "unexpected": true }
    }));
    assert_eq!(response["error"]["code"], -32602);
    assert_eq!(response["error"]["data"]["kind"], "INVALID_PARAMS");

    let response = daemon.handle_json_rpc(json!({
        "jsonrpc": "2.0",
        "id": "test-3",
        "method": "sessions_snapshot"
    }));
    assert_eq!(response["error"]["code"], -32600);
    assert_eq!(response["error"]["data"]["kind"], "INVALID_REQUEST");

    let response = daemon.handle_json_rpc(json!({
        "jsonrpc": "2.0",
        "id": ["invalid"],
        "method": "sessions_snapshot",
        "params": {}
    }));
    assert!(response["id"].is_null());
    assert_eq!(response["error"]["code"], -32600);
    daemon.shutdown();
    let _ = fs::remove_dir_all(root);
}
