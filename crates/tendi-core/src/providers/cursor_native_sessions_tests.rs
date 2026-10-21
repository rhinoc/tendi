use super::*;
use crate::{
    providers::{AgentProvider, cursor::CursorProvider},
    transcript::{self, TranscriptSearchScopes},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tendi-cursor-native-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn field(number: u8, data: &[u8]) -> Vec<u8> {
    let mut result = vec![(number << 3) | 2];
    let mut length = data.len();
    while length >= 128 {
        result.push((length as u8 & 127) | 128);
        length >>= 7;
    }
    result.push(length as u8);
    result.extend(data);
    result
}

fn desktop(fixture: &Fixture) -> (PathBuf, Connection) {
    let path = desktop_database(&fixture.0);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE composerHeaders(composerId TEXT PRIMARY KEY, value TEXT, isArchived INTEGER); CREATE TABLE cursorDiskKV(key TEXT PRIMARY KEY,value BLOB); PRAGMA journal_mode=WAL;").unwrap();
    (path, db)
}

fn desktop_chat(db: &Connection, id: &str, text: &str, archived: bool, blob: u8) {
    let state = field(1, &[blob; 32]);
    let header = json!({"name":id,"createdAt":1780000000000_i64,"lastUpdatedAt":1780000010000_i64,"isDraft":false,"workspaceIdentifier":{"uri":{"fsPath":"/tmp/cursor-native-workspace"}}});
    db.execute(
        "INSERT INTO composerHeaders VALUES(?1,?2,?3)",
        rusqlite::params![id, header.to_string(), archived],
    )
    .unwrap();
    db.execute(
        "INSERT INTO cursorDiskKV VALUES(?1,?2)",
        rusqlite::params![
            format!("composerData:{id}"),
            json!({"conversationState":format!("~{}",STANDARD.encode(state))}).to_string()
        ],
    )
    .unwrap();
    db.execute(
        "INSERT INTO cursorDiskKV VALUES(?1,?2)",
        rusqlite::params![
            format!("agentKv:blob:{}", hex(&[blob; 32])),
            json!({"role":"user","content":text}).to_string()
        ],
    )
    .unwrap();
}

#[test]
fn cursor_native_desktop_selects_only_own_messages_and_exposes_read_only_archive_state() {
    let fixture = Fixture::new();
    let (path, db) = desktop(&fixture);
    desktop_chat(&db, "one", "first isolated message", true, 1);
    desktop_chat(&db, "two", "second isolated message", false, 2);
    let mut rows = Vec::new();
    scan_database(&path, &mut rows).unwrap();
    assert_eq!(rows.len(), 2);
    let one = rows.iter().find(|row| row.id == "one").unwrap();
    assert_eq!(one.archived, Some(true));
    assert!(!one.archive_supported);
    assert!(!one.resumable);
    assert_eq!(
        one.project.as_deref(),
        Some(Path::new("/tmp/cursor-native-workspace"))
    );
    assert!(CursorProvider.resume_session_command(one).is_none());
    assert!(set_archived(one, false).is_err());
    let page = transcript::parse_session_transcript_page_if_changed(
        &path,
        AgentKind::Cursor,
        Some("one"),
        None,
        Some(1),
        None,
    )
    .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].body, "first isolated message");
    let scopes = TranscriptSearchScopes {
        user: true,
        ..Default::default()
    };
    assert!(
        transcript::search_session_transcript(
            &path,
            AgentKind::Cursor,
            Some("one"),
            "second",
            &scopes
        )
        .unwrap()
        .hits
        .is_empty()
    );
    assert_eq!(
        transcript::search_session_transcript(
            &path,
            AgentKind::Cursor,
            Some("two"),
            "second",
            &scopes
        )
        .unwrap()
        .hits
        .len(),
        1
    );
    let export = fixture
        .0
        .join(".cursor/projects/work/agent-transcripts/one/one.jsonl");
    std::fs::create_dir_all(export.parent().unwrap()).unwrap();
    std::fs::write(&export, "").unwrap();
    assert_eq!(archive_state(&export, "one"), Some(true));
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    assert_eq!(CursorProvider.session_watch_source_path(&wal), Some(path));
}

fn sdk(fixture: &Fixture) -> (PathBuf, Connection) {
    let path = fixture
        .0
        .join(".cursor/projects/work/sdk-agent-store/workspace/index.db");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE agents(agent_id TEXT PRIMARY KEY,workspace_ref TEXT,status TEXT,active_run_id TEXT,latest_checkpoint_ref_json TEXT,name TEXT,metadata_json TEXT,created_at TEXT,updated_at TEXT); PRAGMA journal_mode=WAL;").unwrap();
    (path, db)
}

fn sdk_chat(path: &Path, db: &Connection, id: &str, text: &str) {
    let root = hex(&[8; 32]);
    let message = hex(&[9; 32]);
    db.execute("INSERT INTO agents VALUES(?1,'/tmp/sdk-workspace','IDLE',NULL,?2,?1,'{}','2026-05-28T20:26:40.000Z','2026-05-28T20:26:40.000Z')",rusqlite::params![id,json!({"storeKind":"local-agent-store","blobId":root}).to_string()]).unwrap();
    let store = sdk_store(path, id);
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    let store = Connection::open(store).unwrap();
    store.execute_batch("CREATE TABLE blobs(id TEXT PRIMARY KEY,data BLOB); CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT);").unwrap();
    store
        .execute(
            "INSERT INTO blobs VALUES(?1,?2)",
            rusqlite::params![root, field(1, &[9; 32])],
        )
        .unwrap();
    store
        .execute(
            "INSERT INTO blobs VALUES(?1,?2)",
            rusqlite::params![
                message,
                serde_json::to_vec(&json!({"role":"user","content":text})).unwrap()
            ],
        )
        .unwrap();
}

#[test]
fn cursor_native_sdk_archives_only_selected_agent_and_rejects_active_run() {
    let fixture = Fixture::new();
    let (path, db) = sdk(&fixture);
    sdk_chat(&path, &db, "one", "SDK first message");
    sdk_chat(&path, &db, "two", "SDK second message");
    let mut rows = Vec::new();
    scan_database(&path, &mut rows).unwrap();
    let one = rows.iter().find(|row| row.id == "one").unwrap();
    assert!(one.archive_supported);
    assert!(!one.resumable);
    assert_eq!(one.first_user_message.as_deref(), Some("SDK first message"));
    let full = transcript::parse_session_transcript(&path, AgentKind::Cursor, Some("two")).unwrap();
    assert_eq!(full.items[0].body, "SDK second message");
    let mut exported = Vec::new();
    transcript::write_session_transcript_json(&path, AgentKind::Cursor, Some("two"), &mut exported)
        .unwrap();
    let exported: Value = serde_json::from_slice(&exported).unwrap();
    assert_eq!(exported[0]["body"], "SDK second message");
    let excerpt = transcript::read_session_transcript_excerpt(
        &path,
        AgentKind::Cursor,
        Some("two"),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(excerpt.items[0].item.body, "SDK second message");
    set_archived(one, true).unwrap();
    assert_eq!(archive_state(&path, "one"), Some(true));
    assert_eq!(archive_state(&path, "two"), Some(false));
    set_archived(one, true).unwrap();
    set_archived(one, false).unwrap();
    assert_eq!(archive_state(&path, "one"), Some(false));
    db.execute(
        "UPDATE agents SET status='RUNNING',active_run_id='run-one' WHERE agent_id='one'",
        [],
    )
    .unwrap();
    assert!(
        set_archived(one, true)
            .unwrap_err()
            .to_string()
            .contains("running")
    );
    assert_eq!(archive_state(&path, "one"), Some(false));
    let store = sdk_store(&path, "one");
    assert!(!CursorProvider.is_session_candidate_path(&store));
    assert_eq!(
        CursorProvider
            .session_watch_source_path(&PathBuf::from(format!("{}-wal", store.display()))),
        Some(path)
    );
}

#[test]
fn cursor_native_history_includes_archives_root_messages_and_pending_turns() {
    let mut blobs = std::collections::HashMap::new();
    let mut put = |id: u8, data: Vec<u8>| {
        blobs.insert(hex(&[id; 32]), data);
    };
    put(
        1,
        serde_json::to_vec(&json!({"role":"user","content":"older message"})).unwrap(),
    );
    put(2, field(1, &[1; 32]));
    put(
        3,
        serde_json::to_vec(&json!({"role":"assistant","content":"root response"})).unwrap(),
    );
    put(4, field(1, b"new user turn"));
    put(5, field(1, &field(1, b"new assistant response")));
    put(
        6,
        field(1, &[field(1, &[4; 32]), field(2, &[5; 32])].concat()),
    );
    let state = [
        field(13, &[2; 32]),
        field(1, &[1; 32]),
        field(1, &[3; 32]),
        field(8, &[6; 32]),
    ]
    .concat();
    let scan = read_state(&state, |id| Ok(blobs.get(id).cloned())).unwrap();
    assert!(scan.warnings.is_empty());
    assert_eq!(
        scan.items
            .iter()
            .map(|item| item.body.as_str())
            .collect::<Vec<_>>(),
        [
            "older message",
            "root response",
            "new user turn",
            "new assistant response"
        ]
    );
}

#[test]
fn cursor_native_pagination_isolated_by_session_and_restarts_after_message_changes() {
    let fixture = Fixture::new();
    let (path, db) = desktop(&fixture);
    desktop_chat(&db, "one", "user", false, 1);
    desktop_chat(&db, "two", "other", false, 2);
    let state = [field(1, &[1; 32]), field(1, &[3; 32])].concat();
    db.execute(
        "UPDATE cursorDiskKV SET value=?1 WHERE key='composerData:one'",
        [json!({"conversationState":format!("~{}",STANDARD.encode(state))}).to_string()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO cursorDiskKV VALUES(?1,?2)",
        rusqlite::params![
            format!("agentKv:blob:{}", hex(&[3; 32])),
            json!({"role":"assistant","content":"response"}).to_string()
        ],
    )
    .unwrap();
    let first = transcript::parse_session_transcript_page_if_changed(
        &path,
        AgentKind::Cursor,
        Some("one"),
        None,
        Some(1),
        None,
    )
    .unwrap();
    assert!(!first.done);
    let second = transcript::parse_session_transcript_page_if_changed(
        &path,
        AgentKind::Cursor,
        Some("one"),
        first.next_cursor.as_deref(),
        Some(1),
        None,
    )
    .unwrap();
    assert!(second.done);
    assert_eq!(second.items[0].body, "response");
    let wrong = transcript::parse_session_transcript_page_if_changed(
        &path,
        AgentKind::Cursor,
        Some("two"),
        first.next_cursor.as_deref(),
        Some(1),
        None,
    )
    .unwrap();
    assert!(wrong.restart_required);
    let unchanged = transcript::parse_session_transcript_page_if_changed(
        &path,
        AgentKind::Cursor,
        Some("one"),
        None,
        None,
        Some(&first.source_version),
    )
    .unwrap();
    assert!(unchanged.unchanged);
    db.execute(
        "UPDATE cursorDiskKV SET value=?1 WHERE key=?2",
        rusqlite::params![
            json!({"role":"assistant","content":"changed response"}).to_string(),
            format!("agentKv:blob:{}", hex(&[3; 32]))
        ],
    )
    .unwrap();
    let changed = transcript::parse_session_transcript_page_if_changed(
        &path,
        AgentKind::Cursor,
        Some("one"),
        first.next_cursor.as_deref(),
        None,
        None,
    )
    .unwrap();
    assert!(changed.restart_required);
    let locator =
        transcript::parse_session_transcript_locator_page(&path, AgentKind::Cursor, Some("one"))
            .unwrap();
    assert_eq!(locator.locator_items.len(), 1);
}

#[test]
fn cursor_native_shared_database_keeps_independent_scan_cache_and_search_indexes() {
    let fixture = Fixture::new();
    let (path, db) = sdk(&fixture);
    sdk_chat(&path, &db, "one", "firstalpha content");
    sdk_chat(&path, &db, "two", "firstbeta content");
    let store = crate::storage::Store::open(fixture.0.join("tendi.db")).unwrap();
    let scope = crate::ScopeKey::new("workspace:native-search").unwrap();
    let scan = crate::sessions::scan_session_paths(&[path.clone()], &Default::default());
    assert!(scan.warnings.is_empty());
    store.save_sessions_at_for_scope(&scope, &scan, 1).unwrap();
    let (_, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(!pending);
    let cache = store.session_scan_cache_for_scope(&scope).unwrap();
    assert!(
        cache.changed_sessions(&scan.sessions).is_empty(),
        "scan: {:?}, cache: {cache:?}",
        scan.sessions
    );
    let query = |text: &str| crate::storage::SessionListQuery {
        query: text.to_owned(),
        agent: None,
        sort_key: "updated".to_owned(),
        sort_direction: "desc".to_owned(),
        group_by: None,
        page: 1,
        page_size: 50,
        show_child_sessions: true,
        selected_project_keys: Vec::new(),
        locate: None,
    };
    let page = store
        .list_session_page_for_scope(&scope, query("firstbeta"))
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.rows[0].session.id, "two");
    let second_store = Connection::open(sdk_store(&path, "two")).unwrap();
    second_store
        .execute(
            "UPDATE blobs SET data=?1 WHERE id=?2",
            rusqlite::params![
                serde_json::to_vec(&json!({"role":"user","content":"secondbeta content"})).unwrap(),
                hex(&[9; 32])
            ],
        )
        .unwrap();
    let next = crate::sessions::scan_session_paths(&[path.clone()], &cache);
    store
        .apply_session_changes_for_scope(&scope, &next.sessions, &[])
        .unwrap();
    let (_, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(!pending);
    assert_eq!(
        store
            .list_session_page_for_scope(&scope, query("firstbeta"))
            .unwrap()
            .total,
        0
    );
    let page = store
        .list_session_page_for_scope(&scope, query("secondbeta"))
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.rows[0].session.id, "two");
    let page = store
        .list_session_page_for_scope(&scope, query("firstalpha"))
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.rows[0].session.id, "one");
}

#[test]
fn cursor_native_deep_desktop_subagents_keep_parent_and_archive_state() {
    let fixture = Fixture::new();
    let (database, db) = desktop(&fixture);
    desktop_chat(&db, "child", "child content", true, 1);
    db.execute(
        "UPDATE composerHeaders SET value=?1 WHERE composerId='child'",
        [
            json!({"name":"Child","isDraft":false,"subagentInfo":{"parentComposerId":"parent"}})
                .to_string(),
        ],
    )
    .unwrap();
    let path = fixture
        .0
        .join(".cursor/projects/work/agent-transcripts/parent/subagents/child.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "{\"role\":\"user\",\"message\":{\"content\":\"child content\"}}\n",
    )
    .unwrap();
    let context = crate::providers::ProviderContext {
        home: Some(fixture.0.clone()),
        project_dirs: Vec::new(),
    };
    let mut rows = Vec::new();
    let mut warnings = Vec::new();
    CursorProvider
        .scan_sessions(&context, &mut rows, &mut warnings, None)
        .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let export = rows.iter().find(|row| row.path == path).unwrap();
    assert_eq!(export.parent_session_id.as_deref(), Some("parent"));
    assert_eq!(export.archived, Some(true));
    assert!(!export.resumable);
    let native = rows.iter().find(|row| row.path == database).unwrap();
    assert_eq!(native.parent_session_id.as_deref(), Some("parent"));
    assert!(
        CursorProvider
            .session_scan_source_paths(&path)
            .contains(&database)
    );
}
