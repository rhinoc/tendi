use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;
use serde_json::json;

use super::*;

fn project_fixture(project: &Path) -> (PathBuf, PathBuf) {
    let home = std::env::temp_dir().join(format!(
        "tendi-cursor-project-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let root = home
        .join(".cursor/projects")
        .join(cursor_project_key(project));
    let path = root.join("agent-transcripts/session/session.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"排队时取消\"}]}}\n").unwrap();
    (home, path)
}

fn terminal_fixture(path: &Path, name: &str, cwd: &Path) {
    let root = cursor_transcript_project_dir(path)
        .unwrap()
        .join("terminals");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join(name),
        format!("---\npid: 1\ncwd: {}\n---\n", json!(cwd)),
    )
    .unwrap();
}

#[test]
fn deleted_worktree_cwd_is_recovered_without_splitting_hyphens() {
    let project = Path::new("/Users/test/dev/tutti-os/tutti-agent-session-replay-cases");
    let (home, path) = project_fixture(project);
    terminal_fixture(
        &path,
        "other.txt",
        Path::new("/Users/test/dev/tutti-os/tutti"),
    );
    terminal_fixture(&path, "matching.txt", project);
    assert_eq!(
        cursor_project_from_transcript_path(&path).as_deref(),
        Some(project)
    );
    let mut records = Vec::new();
    sessions::scan_jsonl_sessions(
        &home.join(".cursor/projects"),
        AgentKind::Cursor,
        4,
        &mut records,
        None,
    );
    assert_eq!(records[0].project.as_deref(), Some(project));
    let store = crate::storage::Store::open(home.join("tendi.sqlite3")).unwrap();
    let scope = crate::ScopeKey::new("workspace:cursor-cwd-test").unwrap();
    store
        .apply_session_changes_for_scope(&scope, &records, &[])
        .unwrap();
    let (_, warnings, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert!(warnings.is_empty());
    assert!(!pending);
    let page = store
        .recall_sessions(&crate::storage::SessionRecallOptions {
            query: "排队时取消".into(),
            cwd: vec![PathBuf::from("/Users/test/dev/tutti-os")],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.total, 1);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn workspace_uri_preserves_percent_encoded_paths() {
    let project = Path::new("/Users/test/dev/tutti-lab/测试 project");
    let (home, path) = project_fixture(project);
    let workspace = super::super::cursor::cursor_state_db_path(&home.join(".cursor/projects"))
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("workspaceStorage/abc/workspace.json");
    fs::create_dir_all(workspace.parent().unwrap()).unwrap();
    fs::write(
        &workspace,
        json!({"folder":url::Url::from_file_path(project).unwrap().as_str()}).to_string(),
    )
    .unwrap();
    assert_eq!(
        cursor_project_from_transcript_path(&path).as_deref(),
        Some(project)
    );
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn hidden_worktree_and_terminal_subdirectory_use_the_original_workspace() {
    let project = Path::new("/Users/test/dev/tutti-lab/.worktrees/queue-edit");
    assert_eq!(
        cursor_project_key(project),
        "Users-test-dev-tutti-lab-worktrees-queue-edit"
    );
    let (home, path) = project_fixture(project);
    terminal_fixture(&path, "cwd.txt", &project.join("apps/web"));
    assert_eq!(
        cursor_project_from_transcript_path(&path).as_deref(),
        Some(project)
    );
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn ambiguous_keys_and_unrelated_terminal_paths_are_not_invented() {
    let project = Path::new("/Users/test/dev/a-b/c");
    let (home, path) = project_fixture(project);
    terminal_fixture(&path, "unrelated.txt", Path::new("/Users/test/dev/other"));
    assert_eq!(cursor_project_from_transcript_path(&path), None);
    terminal_fixture(&path, "one.txt", project);
    assert_eq!(
        cursor_project_from_transcript_path(&path).as_deref(),
        Some(project)
    );
    terminal_fixture(&path, "two.txt", Path::new("/Users/test/dev/a/b-c"));
    assert_eq!(cursor_project_from_transcript_path(&path), None);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn project_resolution_is_shared_within_scan_and_refreshed_on_next_scan() {
    let project = Path::new("/Users/test/dev/tutti-lab/tutti");
    let (home, path) = project_fixture(project);
    {
        let _scan = project_scan_cache();
        assert_eq!(cursor_project_from_transcript_path(&path), None);
        terminal_fixture(&path, "cwd.txt", project);
        assert_eq!(cursor_project_from_transcript_path(&path), None);
    }
    {
        let _scan = project_scan_cache();
        assert_eq!(
            cursor_project_from_transcript_path(&path).as_deref(),
            Some(project)
        );
    }
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn cached_guessed_project_is_reparsed_and_explicit_metadata_keeps_precedence() {
    let project = Path::new("/Users/test/dev/tutti-os/tutti");
    let (home, path) = project_fixture(project);
    terminal_fixture(&path, "cwd.txt", project);
    let mut records = Vec::new();
    sessions::scan_jsonl_sessions(
        &home.join(".cursor/projects"),
        AgentKind::Cursor,
        4,
        &mut records,
        None,
    );
    let mut session = records.remove(0);
    session.project = Some(PathBuf::from("/Users/test/dev/tutti/os/tutti"));
    let metadata = fs::metadata(&path).unwrap();
    let cache = SessionScanCache::from_entries([crate::sessions::SessionScanCacheEntry {
        session: session.clone(),
        file_mtime: metadata
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
        file_size: metadata.len() as i64,
        additional_file_states: vec![],
    }]);
    records.clear();
    sessions::scan_jsonl_sessions(
        &home.join(".cursor/projects"),
        AgentKind::Cursor,
        4,
        &mut records,
        Some(&cache),
    );
    assert_eq!(records[0].project.as_deref(), Some(project));
    assert!(cached_project_is_current(&records[0]));
    let explicit = Path::new("/different/explicit-workspace");
    fs::write(
        &path,
        format!(
            "{}\n",
            json!({"role":"user","cwd":explicit,"content":"queue"})
        ),
    )
    .unwrap();
    records.clear();
    sessions::scan_jsonl_sessions(
        &home.join(".cursor/projects"),
        AgentKind::Cursor,
        4,
        &mut records,
        None,
    );
    assert_eq!(records[0].project.as_deref(), Some(explicit));
    assert!(cached_project_is_current(&records[0]));
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn native_cwd_is_shared_by_scanning_and_cache_validation() {
    let project = Path::new("/Users/test/dev/tendi");
    let (home, path) = project_fixture(project);
    terminal_fixture(&path, "cwd.txt", project);
    let meta = home.join(".cursor/chats/workspace/session/meta.json");
    fs::create_dir_all(meta.parent().unwrap()).unwrap();
    let cwd = project.join("apps/desktop");
    fs::write(
        &meta,
        json!({"schemaVersion":1,"cwd":cwd,"name":"Work"}).to_string(),
    )
    .unwrap();
    let store = crate::storage::Store::open(home.join("tendi.sqlite3")).unwrap();
    let scope = crate::ScopeKey::new("workspace:native-cwd").unwrap();
    let mut records = Vec::new();
    {
        let _scan = project_scan_cache();
        sessions::scan_jsonl_sessions(
            &home.join(".cursor/projects"),
            AgentKind::Cursor,
            4,
            &mut records,
            None,
        );
        assert_eq!(records[0].project.as_ref(), Some(&cwd));
        assert!(cached_project_is_current(&records[0]));
        store
            .apply_session_changes_for_scope(&scope, &records, &[])
            .unwrap();
    }
    let cache = store.shared_session_scan_cache().unwrap();
    {
        let _scan = project_scan_cache();
        assert!(cache.session_if_current(AgentKind::Cursor, &path).is_some());
        assert!(scan_cursor_meta_file(
            &meta,
            &mut Vec::new(),
            AgentKind::Cursor,
            Some(&cache)
        ));
    }
    let moved = project.join("apps/web");
    fs::write(
        &meta,
        json!({"schemaVersion":1,"cwd":moved,"name":"Moved workspace"}).to_string(),
    )
    .unwrap();
    {
        let _scan = project_scan_cache();
        assert!(cache.session_if_current(AgentKind::Cursor, &path).is_none());
        records.clear();
        sessions::scan_jsonl_sessions(
            &home.join(".cursor/projects"),
            AgentKind::Cursor,
            4,
            &mut records,
            Some(&cache),
        );
        assert_eq!(records[0].project.as_ref(), Some(&moved));
    }
    let newer_scope = crate::ScopeKey::new("workspace:newer-cwd").unwrap();
    store
        .persist_session_scan_for_scope(
            &newer_scope,
            &crate::SessionScan {
                sessions: records,
                warnings: Vec::new(),
            },
            &cache,
            2,
        )
        .unwrap();
    {
        let _scan = project_scan_cache();
        let shared = store.shared_session_scan_cache().unwrap();
        let current = shared.session_if_current(AgentKind::Cursor, &path).unwrap();
        assert_eq!(current.project.as_ref(), Some(&moved));
    }
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn native_cwd_follows_acp_then_chat_scan_order() {
    let (home, path) = project_fixture(Path::new("/repo"));
    for (relative, cwd) in [
        (".cursor/acp-sessions/session/meta.json", "/repo/acp"),
        (".cursor/chats/workspace/session/meta.json", "/repo/chat"),
    ] {
        let meta = home.join(relative);
        fs::create_dir_all(meta.parent().unwrap()).unwrap();
        fs::write(meta, json!({"cwd":cwd}).to_string()).unwrap();
    }
    let _scan = project_scan_cache();
    assert_eq!(
        session_project(&path, Some(PathBuf::from("/repo/explicit"))),
        Some(PathBuf::from("/repo/acp"))
    );
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn empty_cursor_sources_survive_restart_and_wal_changes_invalidate_them() {
    let (home, _) = project_fixture(Path::new("/repo"));
    let meta = home.join(".cursor/chats/workspace/empty/meta.json");
    fs::create_dir_all(meta.parent().unwrap()).unwrap();
    fs::write(&meta, json!({"schemaVersion":1}).to_string()).unwrap();
    let source = Connection::open(meta.with_file_name("store.db")).unwrap();
    source.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE meta(key TEXT, value TEXT); CREATE TABLE blobs(data BLOB); PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    let database = home.join("tendi.sqlite3");
    let scope = crate::ScopeKey::new("workspace:empty-source").unwrap();
    let empty = crate::SessionScan {
        sessions: Vec::new(),
        warnings: Vec::new(),
    };
    {
        let store = crate::storage::Store::open(&database).unwrap();
        let cache = store.shared_session_scan_cache().unwrap();
        let mut records = Vec::new();
        assert!(!scan_cursor_meta_file(
            &meta,
            &mut records,
            AgentKind::Cursor,
            Some(&cache)
        ));
        assert!(records.is_empty());
        assert_eq!(cache.empty_source_entries().len(), 1);
        store
            .persist_session_scan_for_scope(&scope, &empty, &cache, 1)
            .unwrap();
    }
    let store = crate::storage::Store::open(&database).unwrap();
    let cache = store.shared_session_scan_cache().unwrap();
    let mut records = Vec::new();
    assert!(scan_cursor_meta_file(
        &meta,
        &mut records,
        AgentKind::Cursor,
        Some(&cache)
    ));
    assert!(records.is_empty());
    source
        .execute(
            "INSERT INTO blobs VALUES (?1)",
            [json!({"role":"user","content":"queue"})
                .to_string()
                .into_bytes()],
        )
        .unwrap();
    assert!(!scan_cursor_meta_file(
        &meta,
        &mut records,
        AgentKind::Cursor,
        Some(&cache)
    ));
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].first_user_message.as_deref(), Some("queue"));
    store
        .persist_session_scan_for_scope(
            &scope,
            &crate::SessionScan {
                sessions: records,
                warnings: Vec::new(),
            },
            &cache,
            2,
        )
        .unwrap();
    assert!(
        store
            .shared_session_scan_cache()
            .unwrap()
            .empty_source_entries()
            .is_empty()
    );
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn failed_cursor_store_reads_are_not_cached_as_empty() {
    let (home, _) = project_fixture(Path::new("/repo"));
    let meta = home.join(".cursor/chats/workspace/broken/meta.json");
    fs::create_dir_all(meta.parent().unwrap()).unwrap();
    fs::write(&meta, json!({"schemaVersion":1}).to_string()).unwrap();
    fs::write(meta.with_file_name("store.db"), b"invalid sqlite database").unwrap();
    let cache = SessionScanCache::default();
    assert!(!scan_cursor_meta_file(
        &meta,
        &mut Vec::new(),
        AgentKind::Cursor,
        Some(&cache)
    ));
    assert!(cache.empty_source_entries().is_empty());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_database_created_after_an_empty_meta_scan_invalidates_the_cache() {
    let (home, _) = project_fixture(Path::new("/repo"));
    let meta = home.join(".cursor/acp-sessions/new/meta.json");
    fs::create_dir_all(meta.parent().unwrap()).unwrap();
    fs::write(&meta, json!({"schemaVersion":1}).to_string()).unwrap();
    let cache = SessionScanCache::default();
    let mut records = Vec::new();
    assert!(!scan_cursor_meta_file(
        &meta,
        &mut records,
        AgentKind::Cursor,
        Some(&cache)
    ));
    assert!(scan_cursor_meta_file(
        &meta,
        &mut records,
        AgentKind::Cursor,
        Some(&cache)
    ));
    assert!(records.is_empty());
    let source = Connection::open(meta.with_file_name("store.db")).unwrap();
    source
        .execute_batch("CREATE TABLE meta(key TEXT, value TEXT); CREATE TABLE blobs(data BLOB);")
        .unwrap();
    source
        .execute(
            "INSERT INTO blobs VALUES (?1)",
            [json!({"role":"user","content":"new request"})
                .to_string()
                .into_bytes()],
        )
        .unwrap();
    assert!(!scan_cursor_meta_file(
        &meta,
        &mut records,
        AgentKind::Cursor,
        Some(&cache)
    ));
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].first_user_message.as_deref(),
        Some("new request")
    );
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn available_skill_catalog_is_not_usage_evidence() {
    let text = concat!(
        "<agent_skills><available_skills description=\"Skills the agent can use\">",
        "<agent_skill fullPath=\"/skills/available/SKILL.md\">Available</agent_skill>",
        "</available_skills></agent_skills>",
        "<agent_skill fullPath=\"/skills/selected/SKILL.md\">Selected</agent_skill>"
    );
    let mut candidates = Vec::new();
    collect_cursor_skill_evidence(&json!({"role":"system","content":text}), &mut candidates);
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].path.as_deref(),
        Some("/skills/selected/SKILL.md")
    );
    assert!(
        cursor_agent_skill_paths(
            "<available_skills><agent_skill fullPath=\"/skills/available/SKILL.md\">"
        )
        .is_empty()
    );
}

#[test]
fn tool_results_do_not_turn_catalog_paths_into_skill_reads() {
    let mut candidates = Vec::new();
    collect_cursor_skill_evidence(
        &json!({"role":"tool","content":[{
            "type":"tool-result", "toolName":"Read",
            "result":"<available_skills><agent_skill fullPath=\"/skills/available/SKILL.md\">Available</agent_skill></available_skills>"
        }]}),
        &mut candidates,
    );
    assert!(candidates.is_empty());
}

#[test]
fn extracts_skill_evidence_from_cursor_store() {
    let root = std::env::temp_dir().join(format!(
        "tendi-cursor-skill-evidence-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("create temp directory");
    let skill_path = root.join(".ctx/plans/e2e/SKILL.md");
    fs::create_dir_all(skill_path.parent().expect("skill parent")).expect("create skill");
    let store_path = root.join("store.db");
    let connection = Connection::open(&store_path).expect("open store");
    connection
        .execute("CREATE TABLE blobs (data BLOB)", [])
        .expect("create blobs table");
    for value in [
        json!({
            "role": "assistant",
            "content": [{
                "type": "tool-call",
                "toolCallId": "read-1",
                "toolName": "Read",
                "args": { "path": skill_path }
            }]
        }),
        json!({
            "role": "user",
            "content": [{
                "type": "text",
                "text": format!("<agent_skill fullPath=\"{}\">", skill_path.display())
            }]
        }),
    ] {
        let data = value.to_string();
        connection
            .execute("INSERT INTO blobs (data) VALUES (?1)", [data.as_bytes()])
            .expect("insert blob");
    }
    drop(connection);

    let evidence = cursor_store_skill_evidence_for_path(&store_path);
    assert!(evidence.iter().any(|candidate| {
        candidate.path.as_deref() == Some(skill_path.to_str().expect("skill path"))
            && candidate.evidence.kind == "Read"
            && candidate.confidence == "observed"
    }));
    assert!(evidence.iter().any(|candidate| {
        candidate.path.as_deref() == Some(skill_path.to_str().expect("skill path"))
            && candidate.evidence.kind == "agent_skill"
            && candidate.confidence == "explicit"
    }));

    fs::remove_dir_all(root).expect("remove temp directory");
}
