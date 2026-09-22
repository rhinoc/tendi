use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    AgentScan, HookScan, McpScan, RuleRecord, RuleScan, ScanReport, SessionRecord, SessionScan,
    SkillRecord, SkillScan, SkillVisibility,
    analytics::{
        AnalyticsParserState, AnalyticsResponseUsage, AnalyticsTokenUsage, SessionAnalytics,
        SessionAnalyticsRecord,
    },
    assistant::{AssistantMessage, AssistantSessionLink},
    runtime_contract::{
        OperationId, OperationKind, OperationRecord, OperationStatus, Revision, ScopeKey,
        SourceVersion,
    },
    session_skills::{SessionFileState, SessionSkillLink},
    sessions::SessionIdentity,
    skills::{
        AgentKind, SkillPath, SkillRoot, SkillSnapshot, SkillSnapshotFile, SkillSourceRecord,
    },
};
use chrono::Local;
use rusqlite::{Connection, params};

use super::{
    ANALYTICS_JSON_ENCODING, AppSettings, PromptWrite, SESSION_SEARCH_CANDIDATE_TABLE,
    SessionListQuery, SessionListRow, Store, compare_session_list_rows, decompress_analytics_json,
    highlight_contains_match, is_database_io_error, is_database_io_error_message,
    normalize_repository_url,
};

#[test]
fn sqlite_ioerr_classification_includes_extended_codes() {
    let error = anyhow::Error::new(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR_SHORT_READ),
        None,
    ));
    assert!(is_database_io_error(&error));
    assert!(is_database_io_error_message(&error.to_string()));
    assert!(!is_database_io_error_message("database is locked"));
}

#[test]
fn repository_urls_normalize_https_and_ssh_forms() {
    assert_eq!(
        normalize_repository_url("https://github.com/tutti-os/tutti.git/"),
        Some("github.com/tutti-os/tutti".to_string())
    );
    assert_eq!(
        normalize_repository_url("git@github.com:tutti-os/tutti.git"),
        Some("github.com/tutti-os/tutti".to_string())
    );
    assert_eq!(
        normalize_repository_url("ssh://git@github.com/tutti-os/tutti.git"),
        Some("github.com/tutti-os/tutti".to_string())
    );
}

#[test]
fn search_snippet_context_counts_cjk_characters() {
    let value = format!("{}命中{}", "前".repeat(81), "后".repeat(81));
    let snippet = highlight_contains_match(&value, "命中").unwrap();

    assert!(!snippet.starts_with("… "));
    assert!(snippet.ends_with(" …"));
    assert!(snippet.contains("⟦命中⟧"));
    let marker_start = snippet.find('⟦').unwrap();
    assert_eq!(snippet[..marker_start].chars().count(), 0);
    assert_eq!(snippet.chars().count(), 86);
}

#[test]
fn search_score_sort_precedes_updated_time() {
    let mut rows = vec![
        SessionListRow {
            session: session("newer", "Newer"),
            search_score: Some(6.0),
            search_snippet: Some("single match".to_string()),
        },
        SessionListRow {
            session: session("older", "Older"),
            search_score: Some(20.0),
            search_snippet: Some("multiple matches".to_string()),
        },
    ];
    rows.sort_by(|left, right| compare_session_list_rows(left, right, "searchScore", "desc"));

    assert_eq!(rows[0].session.id, "older");
    assert_eq!(rows[1].session.id, "newer");
}

#[test]
fn session_scan_cache_uses_persisted_source_state() {
    let temp = temp_dir("tendi-session-scan-source-state");
    fs::create_dir_all(&temp).unwrap();
    let transcript = temp.join("session.jsonl");
    fs::write(&transcript, "initial\n").unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let scope = ScopeKey::new("workspace:/repo").unwrap();
    let session = SessionRecord {
        id: "session-id".to_string(),
        agent: AgentKind::Codex,
        title: Some("Session".to_string()),
        project: None,
        repository: None,
        repository_url: None,
        logical_project_id: None,
        logical_project_name: None,
        path: transcript.clone(),
        started_at: None,
        updated_at: None,
        message_count: Some(1),
        first_user_message: Some("initial".to_string()),
        last_user_message: Some("initial".to_string()),
        last_assistant_message: Some("answer".to_string()),
        turn_count: Some(1),
        model: None,
        mode: None,
        approval_mode: None,
        is_run_everything: None,
        parent_session_id: None,
        token_usage: None,
    };

    store
        .apply_session_delta_for_scope(&scope, std::slice::from_ref(&session))
        .unwrap();
    let cache = store.session_scan_cache_for_scope(&scope).unwrap();
    assert!(
        cache
            .session_if_current(AgentKind::Codex, &transcript)
            .is_some()
    );

    fs::write(&transcript, "changed source\n").unwrap();
    let cache = store.session_scan_cache_for_scope(&scope).unwrap();
    assert!(
        cache
            .session_if_current(AgentKind::Codex, &transcript)
            .is_none()
    );

    Connection::open(store.path())
        .unwrap()
        .execute(
            "UPDATE scoped_session_scan_sources SET parser_version = 'legacy'",
            [],
        )
        .unwrap();
    fs::write(&transcript, "initial\n").unwrap();
    let cache = store.session_scan_cache_for_scope(&scope).unwrap();
    assert!(
        cache
            .session_if_current(AgentKind::Codex, &transcript)
            .is_none()
    );

    store
        .apply_session_delta_for_scope(&scope, std::slice::from_ref(&session))
        .unwrap();
    let cache = store.session_scan_cache_for_scope(&scope).unwrap();
    assert!(
        cache
            .session_if_current(AgentKind::Codex, &transcript)
            .is_some()
    );

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn finalizing_a_batched_session_scan_prunes_stale_rows() {
    let temp = temp_dir("tendi-session-scan-finalize");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let scope = ScopeKey::new("workspace:/repo").unwrap();
    let keep = session("keep", "Keep");
    let remove = session("remove", "Remove");

    store
        .save_sessions_at_for_scope(
            &scope,
            &SessionScan {
                sessions: vec![keep.clone(), remove],
                warnings: Vec::new(),
            },
            1,
        )
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&scope)
        .unwrap();
    let stale_search_rows_before: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM scoped_session_search_records
                 WHERE scope_key = ?1 AND session_id = 'remove'",
            [scope.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(stale_search_rows_before > 0);
    store
        .apply_session_delta_and_resolve_projects_for_scope(&scope, std::slice::from_ref(&keep))
        .unwrap();
    store
        .finalize_session_scan_for_scope(
            &scope,
            &SessionScan {
                sessions: vec![keep.clone()],
                warnings: Vec::new(),
            },
            2,
        )
        .unwrap();

    let sessions = store.list_sessions_for_scope(&scope).unwrap().sessions;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "keep");
    assert_eq!(
        store.sessions_last_scan_at_for_scope(&scope).unwrap(),
        Some(2)
    );
    let (_, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert!(errors.is_empty());
    assert!(!pending);
    let stale_search_rows_after: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM scoped_session_search_records
                 WHERE scope_key = ?1 AND session_id = 'remove'",
            [scope.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stale_search_rows_after, 0);

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn project_scan_scope_excludes_bang_prefixed_paths() {
    let temp = temp_dir("tendi-project-scope-exclude");
    let scope = temp.join("dev");
    let included = scope.join("included");
    let excluded = scope.join("nested").join("excluded");
    fs::create_dir_all(included.join(".git")).unwrap();
    fs::create_dir_all(excluded.join(".git")).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

    store
        .save_project_scan_scopes(vec![
            scope.to_string_lossy().into_owned(),
            format!("!{}/**/excluded", scope.display()),
        ])
        .unwrap();
    let result = store.scan_projects().unwrap();

    assert!(result.scopes.iter().any(|scope| scope.excluded));
    assert!(
        result
            .projects
            .iter()
            .any(|project| project.root_path == included.canonicalize().unwrap())
    );
    assert!(
        !result
            .projects
            .iter()
            .any(|project| { project.root_path == excluded.canonicalize().unwrap() })
    );

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn scoped_session_search_rebuild_repairs_existing_projection_rows() {
    let temp = temp_dir("tendi-storage-scoped-search-backfill");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let scope = ScopeKey::new("workspace:backfill").unwrap();
    let path = temp.join("session.jsonl");
    fs::write(
        &path,
        include_str!("../testdata/transcripts/codex.jsonl")
            .replace("Inspect the fixture parser", "backfill-private-marker"),
    )
    .unwrap();
    let mut record = session("backfill", "Backfill session");
    record.path = path;
    let candidate = SessionIdentity::from(&record);
    let candidates = std::iter::once(candidate.clone())
        .chain((0..1_199).map(|index| SessionIdentity {
            id: format!("unrelated-{index}"),
            agent: AgentKind::Codex,
            path: PathBuf::from(format!("/tmp/unrelated-{index}.jsonl")),
        }))
        .collect::<Vec<_>>();
    store
        .save_sessions_at_for_scope(
            &scope,
            &SessionScan {
                sessions: vec![record],
                warnings: Vec::new(),
            },
            1,
        )
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&scope)
        .unwrap();
    assert!(
        !store
            .ensure_scoped_session_search_for_scope(&scope)
            .unwrap()
    );
    Connection::open(store.path())
        .unwrap()
        .execute(
            "DELETE FROM scoped_session_search_records WHERE scope_key = ?1",
            params![scope.as_str()],
        )
        .unwrap();

    assert!(
        store
            .search_sessions_for_scope(&scope, "backfill-private-marker", None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .ensure_scoped_session_search_for_scope(&scope)
            .unwrap(),
        true
    );
    assert_eq!(
        store
            .search_sessions_for_scope(&scope, "backfill-private-marker", Some(&candidates),)
            .unwrap()
            .len(),
        1
    );

    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn session_search_candidate_table_handles_large_and_empty_sets() {
    let candidates = (0..1_200)
        .map(|index| SessionIdentity {
            id: format!("session-{index}"),
            agent: AgentKind::Codex,
            path: PathBuf::from(format!("/tmp/session-{index}.jsonl")),
        })
        .collect::<Vec<_>>();
    let temp = temp_dir("tendi-storage-session-search-candidates");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

    let join = store
        .session_search_candidate_join(Some(&candidates))
        .unwrap();
    assert!(join.contains(SESSION_SEARCH_CANDIDATE_TABLE));
    let matched: i64 = store
        .conn
        .query_row(
            &format!("SELECT COUNT(*) FROM temp.{SESSION_SEARCH_CANDIDATE_TABLE}"),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(matched, 1_200);

    store.session_search_candidate_join(Some(&[])).unwrap();
    let matched_empty: i64 = store
        .conn
        .query_row(
            &format!("SELECT COUNT(*) FROM temp.{SESSION_SEARCH_CANDIDATE_TABLE}"),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(matched_empty, 0);

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn session_list_page_filters_sorts_pages_searches_and_locates_in_storage() {
    let temp = temp_dir("tendi-storage-session-list-page");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let scope = ScopeKey::new("workspace:session-list-page").unwrap();
    let mut sessions = [
        session("oldest", "Oldest session"),
        session("middle", "Needle in title"),
        session("newest", "Newest session"),
        session("other", "Other project"),
        session("child", "Child session"),
    ];
    for (index, record) in sessions.iter_mut().enumerate() {
        record.updated_at = Some(format!("2026-06-23T10:{index:02}:00Z"));
    }
    sessions[3].project = Some(PathBuf::from("/tmp/another-project"));
    sessions[3].path = PathBuf::from("/tmp/another-project/other.jsonl");
    sessions[3].first_user_message = Some("retained list preview".to_string());
    sessions[3].model = Some("retained-model".to_string());
    sessions[4].parent_session_id = Some("newest".to_string());
    store
        .save_sessions_at_for_scope(
            &scope,
            &SessionScan {
                sessions: sessions.to_vec(),
                warnings: Vec::new(),
            },
            1,
        )
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&scope)
        .unwrap();

    let base_query = SessionListQuery {
        query: String::new(),
        agent: None,
        sort_key: "updatedAt".to_string(),
        sort_direction: "desc".to_string(),
        group_by: None,
        page: 0,
        page_size: 2,
        show_child_sessions: false,
        selected_project_keys: Vec::new(),
        locate: None,
    };
    let first_page = store
        .list_session_page_for_scope(&scope, base_query.clone())
        .unwrap();
    assert_eq!(first_page.total, 4);
    assert_eq!(first_page.child_session_count, 1);
    assert_eq!(first_page.page_count, 2);
    assert_eq!(
        first_page
            .rows
            .iter()
            .map(|row| row.session.id.as_str())
            .collect::<Vec<_>>(),
        vec!["other", "newest"],
    );
    let mut contract_value = serde_json::to_value(&first_page).unwrap();
    contract_value["revision"] = serde_json::json!(1);
    let contract_page = serde_json::from_value::<
        crate::generated::runtime_contract::SessionsListResponse,
    >(contract_value)
    .unwrap();
    assert_eq!(contract_page.rows.len(), 2);

    let located = store
        .list_session_page_for_scope(
            &scope,
            SessionListQuery {
                locate: Some(SessionIdentity::from(&sessions[0])),
                ..base_query.clone()
            },
        )
        .unwrap();
    assert_eq!(located.page, 1);
    assert!(located.rows.iter().any(|row| row.session.id == "oldest"));

    let searched = store
        .list_session_page_for_scope(
            &scope,
            SessionListQuery {
                query: "needle".to_string(),
                page_size: 50,
                ..base_query.clone()
            },
        )
        .unwrap();
    assert_eq!(searched.total, 1);
    assert_eq!(searched.rows[0].session.id, "middle");
    assert!(searched.rows[0].search_score.is_some());

    let filtered = store
        .list_session_page_for_scope(
            &scope,
            SessionListQuery {
                selected_project_keys: vec!["/tmp/another-project".to_string()],
                page_size: 50,
                ..base_query.clone()
            },
        )
        .unwrap();
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.rows[0].session.id, "other");
    assert_eq!(
        filtered.rows[0].session.first_user_message.as_deref(),
        Some("retained list preview")
    );
    assert_eq!(
        filtered.rows[0].session.model.as_deref(),
        Some("retained-model")
    );

    let grouped = store
        .list_session_page_for_scope(
            &scope,
            SessionListQuery {
                sort_direction: "asc".to_string(),
                group_by: Some("project".to_string()),
                locate: Some(SessionIdentity::from(&sessions[3])),
                ..base_query
            },
        )
        .unwrap();
    assert_eq!(grouped.page_count, 2);
    assert_eq!(grouped.page, 1);
    assert_eq!(grouped.group_count, Some(1));
    assert_eq!(grouped.rows[0].session.id, "other");

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn session_list_uses_derived_numeric_projection_for_sql_page_ordering() {
    let temp = temp_dir("tendi-storage-session-list-projection");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let scope = ScopeKey::new("workspace:session-list-projection").unwrap();
    let mut sessions = vec![
        session("low", "Low"),
        session("high", "High"),
        session("middle", "Middle"),
    ];
    sessions[0].updated_at = Some("2026-06-23T10:00:00+02:00".to_string());
    sessions[1].updated_at = Some("2026-06-23T10:00:00Z".to_string());
    sessions[2].updated_at = Some("2026-06-23T10:00:00-02:00".to_string());
    sessions[0].turn_count = Some(1);
    sessions[1].turn_count = Some(20);
    sessions[2].turn_count = Some(10);
    sessions[0].token_usage = Some(crate::sessions::SessionTokenUsage {
        input_tokens: 100,
        cached_input_tokens: 10,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 0,
    });
    sessions[1].token_usage = Some(crate::sessions::SessionTokenUsage {
        input_tokens: 100,
        cached_input_tokens: 80,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 0,
    });
    store
        .save_sessions_at_for_scope(
            &scope,
            &SessionScan {
                sessions: sessions.clone(),
                warnings: Vec::new(),
            },
            1,
        )
        .unwrap();

    let turns_page = store
        .list_session_page_for_scope(
            &scope,
            SessionListQuery {
                query: String::new(),
                agent: None,
                sort_key: "turns".to_string(),
                sort_direction: "desc".to_string(),
                group_by: None,
                page: 0,
                page_size: 1,
                show_child_sessions: false,
                selected_project_keys: Vec::new(),
                locate: None,
            },
        )
        .unwrap();
    assert_eq!(turns_page.total, 3);
    assert_eq!(turns_page.rows[0].session.id, "high");

    let updated_page = store
        .list_session_page_for_scope(
            &scope,
            SessionListQuery {
                query: String::new(),
                agent: None,
                sort_key: "updatedAt".to_string(),
                sort_direction: "desc".to_string(),
                group_by: None,
                page: 0,
                page_size: 3,
                show_child_sessions: false,
                selected_project_keys: Vec::new(),
                locate: None,
            },
        )
        .unwrap();
    assert_eq!(
        updated_page
            .rows
            .iter()
            .map(|row| row.session.id.as_str())
            .collect::<Vec<_>>(),
        vec!["middle", "high", "low"]
    );

    let cache_page = store
        .list_session_page_for_scope(
            &scope,
            SessionListQuery {
                query: String::new(),
                agent: None,
                sort_key: "cacheRate".to_string(),
                sort_direction: "desc".to_string(),
                group_by: None,
                page: 0,
                page_size: 1,
                show_child_sessions: false,
                selected_project_keys: Vec::new(),
                locate: None,
            },
        )
        .unwrap();
    assert_eq!(cache_page.rows[0].session.id, "high");

    let projection = Connection::open(store.path())
        .unwrap()
        .query_row(
            "SELECT turn_count, input_tokens, cached_input_tokens
             FROM scoped_sessions WHERE scope_key = ?1 AND id = 'high'",
            [scope.as_str()],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(projection, (Some(20), Some(100), Some(80)));

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn session_search_ranks_hits_with_fts_bm25() {
    let temp = temp_dir("tendi-storage-session-search-bm25");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let scope = ScopeKey::new("workspace:session-search-bm25").unwrap();
    let sessions = vec![
        session("single-hit", "needle"),
        session("repeated-hit", "needle needle needle"),
    ];

    store
        .save_sessions_at_for_scope(
            &scope,
            &SessionScan {
                sessions,
                warnings: Vec::new(),
            },
            1,
        )
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&scope)
        .unwrap();

    let hits = store
        .search_sessions_for_scope(&scope, "needle", None)
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].session.id, "repeated-hit");
    assert!(hits[0].search_score > hits[1].search_score);
    assert!(hits[1].search_score > 0.0);

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn session_search_preserves_long_substrings_and_rejects_split_trigrams() {
    let temp = temp_dir("tendi-storage-session-search-trigram-filter");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let scope = ScopeKey::new("workspace:session-search-trigram-filter").unwrap();
    store
        .save_sessions_at_for_scope(
            &scope,
            &SessionScan {
                sessions: vec![
                    session("exact", "abcdef"),
                    session("split", "abc xxx bcd xxx cde xxx def"),
                ],
                warnings: Vec::new(),
            },
            1,
        )
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&scope)
        .unwrap();

    let hits = store
        .search_sessions_for_scope(&scope, "abcdef", None)
        .unwrap();
    assert_eq!(
        hits.iter()
            .map(|hit| hit.session.id.as_str())
            .collect::<Vec<_>>(),
        ["exact"]
    );

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn session_search_fts_optimize_resets_due_mutation_counter() {
    let temp = temp_dir("tendi-storage-session-search-optimize");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    store
        .with_named_write_transaction("test.session_search_maintenance_due", |tx| {
            tx.execute(
                "UPDATE scoped_session_search_maintenance
                 SET pending_mutations = 2000, last_optimized_at = 0 WHERE id = 1",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    assert!(store.optimize_session_search_fts_if_due().unwrap());
    let state: (i64, i64) = store
        .conn
        .query_row(
            "SELECT pending_mutations, last_optimized_at
             FROM scoped_session_search_maintenance WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state.0, 0);
    assert!(state.1 > 0);

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn schema_v1_storage_migrates_fts_and_analytics_cache() {
    let temp = temp_dir("tendi-storage-v1-compaction-migration");
    fs::create_dir_all(&temp).unwrap();
    let db = temp.join("tendi.sqlite3");
    let analytics_json = r#"{"sessionId":"legacy","agent":"codex","responses":[]}"#;
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE scoped_session_analytics (
                 scope_key TEXT NOT NULL, session_id TEXT NOT NULL, agent TEXT NOT NULL,
                 session_path TEXT NOT NULL, file_mtime INTEGER NOT NULL, file_size INTEGER NOT NULL,
                 indexed_at TEXT NOT NULL, analytics_json TEXT NOT NULL, parser_state_json TEXT NOT NULL,
                 event_min_date TEXT, event_max_date TEXT, has_activity INTEGER NOT NULL DEFAULT 0,
                 capability_token_usage INTEGER NOT NULL DEFAULT 0,
                 capability_reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                 capability_explicit_runs INTEGER NOT NULL DEFAULT 0,
                 capability_rate_limit_history INTEGER NOT NULL DEFAULT 0,
                 overview_indexed INTEGER NOT NULL DEFAULT 1, overview_index_error TEXT,
                 PRIMARY KEY (scope_key, session_id, agent, session_path)
             );
             CREATE TABLE scoped_session_search_records (
                 scope_key TEXT NOT NULL, id INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id TEXT NOT NULL, agent TEXT NOT NULL, session_path TEXT NOT NULL,
                 record_order INTEGER NOT NULL, metadata_text TEXT NOT NULL DEFAULT '',
                 title TEXT NOT NULL DEFAULT '', project TEXT NOT NULL DEFAULT '',
                 user_text TEXT NOT NULL, assistant_text TEXT NOT NULL,
                 UNIQUE (scope_key, session_id, agent, session_path, record_order)
             );
             CREATE VIRTUAL TABLE scoped_session_search_fts USING fts5(
                 metadata_text, title, project, user_text, assistant_text,
                 content = 'scoped_session_search_records', content_rowid = 'id',
                 tokenize = 'trigram case_sensitive 0'
             );
             PRAGMA user_version = 1;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO scoped_session_search_records(
                scope_key, session_id, agent, session_path, record_order,
                title, user_text, assistant_text
             ) VALUES ('legacy', 'session', 'codex', '/tmp/session.jsonl', 0,
                       'Legacy title', 'needle', '')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO scoped_session_search_fts(
                rowid, metadata_text, title, project, user_text, assistant_text
             ) SELECT id, metadata_text, title, project, user_text, assistant_text
               FROM scoped_session_search_records",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO scoped_session_analytics(
                scope_key, session_id, agent, session_path, file_mtime, file_size,
                indexed_at, analytics_json, parser_state_json
             ) VALUES ('legacy', 'session', 'codex', '/tmp/session.jsonl', 0, 0,
                       '', ?1, '{}')",
            [analytics_json],
        )
        .unwrap();
    }

    let store = Store::open(&db).unwrap();
    store.run_pending_storage_migrations().unwrap();
    let definition: String = store
        .conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'scoped_session_search_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(definition.contains("contentless_delete = 1"));
    assert!(definition.contains("detail = column"));
    let rowid: i64 = store
        .conn
        .query_row(
            "SELECT rowid FROM scoped_session_search_fts
             WHERE scoped_session_search_fts MATCH 'nee AND eed AND edl AND dle'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rowid, 1);
    let compressed: Vec<u8> = store
        .conn
        .query_row(
            "SELECT analytics_json FROM scoped_session_analytics",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(compressed.starts_with(ANALYTICS_JSON_ENCODING.as_bytes()));
    assert_eq!(
        decompress_analytics_json(&compressed).unwrap(),
        analytics_json
    );
    assert!(
        store
            .conn
            .query_row(
                "SELECT value = '1' FROM meta WHERE key = 'storage.compaction.pending'",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    );
    assert!(store.run_pending_storage_maintenance().unwrap());
    assert!(
        store
            .conn
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM meta WHERE key = 'storage.compaction.pending')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    );

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn reads_remain_available_during_uncommitted_writer() {
    let temp = temp_dir("tendi-session-skill-read-lock");
    fs::create_dir_all(&temp).unwrap();
    let db = temp.join("tendi.sqlite3");
    let store = Store::open(&db).unwrap();
    store.conn.busy_timeout(Duration::ZERO).unwrap();

    let scope = ScopeKey::new("workspace:session-skill-read-lock").unwrap();
    let status = read_after_transient_exclusive_lock(&db, || {
        store.session_skill_index_status_for_scope(&scope, false)
    });
    assert_eq!(status.total, 0);

    let settings = read_after_transient_exclusive_lock(&db, || store.app_settings());
    assert_eq!(settings.appearance, "system");
    let _ = read_after_transient_exclusive_lock(&db, || store.project_scan_scopes());
    let _ = read_after_transient_exclusive_lock(&db, || store.list_projects());
    let _ = read_after_transient_exclusive_lock(&db, || store.list_sessions_for_scope(&scope));
    let _ =
        read_after_transient_exclusive_lock(&db, || store.list_session_projects_for_scope(&scope));
    let _ = read_after_transient_exclusive_lock(&db, || store.analytics_revision());
    let _ = read_after_transient_exclusive_lock(&db, || {
        store.search_sessions_for_scope(&scope, "session", None)
    });
    let _ = read_after_transient_exclusive_lock(&db, || store.list_prompts());

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

fn read_after_transient_exclusive_lock<T>(
    db: &Path,
    read: impl FnOnce() -> anyhow::Result<T>,
) -> T {
    let blocker = Connection::open(db).unwrap();
    blocker.busy_timeout(Duration::ZERO).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let value = read().unwrap();
    blocker.execute_batch("ROLLBACK;").unwrap();
    value
}

#[test]
fn app_settings_store_normalized_additional_session_roots() {
    let temp = temp_dir("tendi-storage-additional-session-settings");
    fs::create_dir_all(&temp).unwrap();
    let db = temp.join("tendi.sqlite3");
    let store = Store::open(&db).unwrap();
    assert_eq!(store.app_settings().unwrap().appearance, "system");
    assert_eq!(store.app_settings().unwrap().font_family, "manrope");
    assert_eq!(store.app_settings().unwrap().light_theme, "vercel");
    assert_eq!(store.app_settings().unwrap().dark_theme, "vercel");
    assert_eq!(store.app_settings().unwrap().app_icon, "gruvbox");
    assert_eq!(store.app_settings().unwrap().session_resume_target, "auto");
    assert_eq!(
        store.app_settings().unwrap().missing_session_project_policy,
        "show"
    );
    assert_eq!(store.app_settings().unwrap().editor, "vscode");
    assert!(!store.app_settings().unwrap().developer_mode);

    let saved = store
        .save_app_settings(AppSettings {
            appearance: "dark".to_string(),
            font_family: "geist".to_string(),
            light_theme: "nord".to_string(),
            dark_theme: "tokyo-night".to_string(),
            app_icon: "dracula".to_string(),
            terminal: "Warp".to_string(),
            session_resume_target: "app".to_string(),
            missing_session_project_policy: "hide".to_string(),
            editor: "zed".to_string(),
            developer_mode: true,
            additional_session_roots: vec![
                "/tmp/tendi-additional-sessions".to_string(),
                "\n/tmp/tendi-additional-sessions\n".to_string(),
            ],
            config_profiles: BTreeMap::from([
                ("codex".to_string(), "deep-review".to_string()),
                ("claude".to_string(), "safe-mode".to_string()),
                ("cursor".to_string(), "safe-mode".to_string()),
            ]),
        })
        .unwrap();

    assert_eq!(saved.appearance, "dark");
    assert_eq!(saved.font_family, "geist");
    assert_eq!(saved.light_theme, "nord");
    assert_eq!(saved.dark_theme, "tokyo-night");
    assert_eq!(saved.app_icon, "dracula");
    assert_eq!(saved.terminal, "Warp");
    assert_eq!(saved.session_resume_target, "app");
    assert_eq!(saved.missing_session_project_policy, "hide");
    assert_eq!(saved.editor, "zed");
    assert!(saved.developer_mode);
    assert_eq!(saved.config_profiles["codex"], "deep-review");
    assert_eq!(saved.config_profiles["claude"], "safe-mode");
    assert_eq!(saved.config_profiles["cursor"], "safe-mode");
    assert_eq!(
        saved.additional_session_roots,
        vec!["/tmp/tendi-additional-sessions"]
    );
    assert_eq!(
        store.app_settings().unwrap().additional_session_roots,
        saved.additional_session_roots
    );
    assert!(store.app_settings().unwrap().developer_mode);
    assert_eq!(
        store.app_settings().unwrap().config_profiles,
        saved.config_profiles
    );

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn workspace_scan_warning_keeps_the_last_good_canonical_snapshot() {
    let temp = temp_dir("tendi-storage-scan-warning");
    fs::create_dir_all(&temp).unwrap();
    let workspace = temp.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let clean = report_with_skill("old", SkillVisibility::Auto);
    store.save_scan_for_workspace(&workspace, &clean).unwrap();

    let mut failed = report_with_skill("new", SkillVisibility::Manual);
    failed
        .skills
        .warnings
        .push("partial provider scan".to_string());
    store.save_scan_for_workspace(&workspace, &failed).unwrap();

    let cached = store
        .list_skills_cached_for_workspace(&workspace)
        .unwrap()
        .unwrap();
    assert_eq!(cached.skills[0].name, "old");

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn skill_snapshots_round_trip_and_are_removed_with_source_records() {
    let temp = temp_dir("tendi-storage-skill-snapshot");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let skill_path = temp.join("skills/demo");
    store
        .replace_skill_snapshots(&[SkillSnapshot {
            skill_path: skill_path.clone(),
            source_version: "base".to_string(),
            files: vec![SkillSnapshotFile {
                relative_path: "SKILL.md".to_string(),
                content: b"base".to_vec(),
            }],
        }])
        .unwrap();

    let snapshot = store.skill_snapshot(&skill_path).unwrap().unwrap();
    assert_eq!(snapshot.source_version, "base");
    assert_eq!(snapshot.files[0].content, b"base");
    store
        .delete_skill_source_records(std::slice::from_ref(&skill_path))
        .unwrap();
    assert!(store.skill_snapshot(&skill_path).unwrap().is_none());

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn projection_manifest_invalidates_on_new_edit_and_delete() {
    let temp = temp_dir("tendi-projection-freshness");
    let workspace = temp.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let rule_path = workspace.join("AGENTS.md");
    fs::write(&rule_path, "old").unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

    store
        .save_rules_for_workspace(
            &workspace,
            &RuleScan {
                rules: vec![RuleRecord {
                    agents: vec![AgentKind::Shared],
                    kind: "agents".to_string(),
                    scope: "project".to_string(),
                    path: rule_path.clone(),
                    order: 0,
                    sha256: "old".to_string(),
                }],
                warnings: Vec::new(),
            },
        )
        .unwrap();
    assert!(
        store
            .list_rules_for_workspace(&workspace)
            .unwrap()
            .is_some()
    );

    fs::write(&rule_path, "edited with a different size").unwrap();
    assert!(
        store
            .list_rules_for_workspace(&workspace)
            .unwrap()
            .is_none()
    );

    fs::remove_file(&rule_path).unwrap();
    assert!(
        store
            .list_rules_for_workspace(&workspace)
            .unwrap()
            .is_none()
    );

    store
        .save_rules_for_workspace(
            &workspace,
            &RuleScan {
                rules: Vec::new(),
                warnings: Vec::new(),
            },
        )
        .unwrap();
    assert!(
        store
            .list_rules_for_workspace(&workspace)
            .unwrap()
            .is_some()
    );
    fs::write(&rule_path, "new file").unwrap();
    assert!(
        store
            .list_rules_for_workspace(&workspace)
            .unwrap()
            .is_none()
    );

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn mcp_projection_ignores_unrelated_skill_directory_changes() {
    let temp = temp_dir("tendi-mcp-projection-freshness");
    let workspace = temp.join("workspace");
    let skill_dir = workspace.join(".agents/skills/demo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "old").unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

    store
        .save_mcp_for_workspace(
            &workspace,
            &McpScan {
                servers: Vec::new(),
                warnings: Vec::new(),
            },
        )
        .unwrap();
    assert!(store.list_mcp_for_workspace(&workspace).unwrap().is_some());

    fs::write(skill_dir.join("SKILL.md"), "edited skill content").unwrap();
    assert!(store.list_mcp_for_workspace(&workspace).unwrap().is_some());

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn skill_projection_invalidates_when_skill_file_changes() {
    let temp = temp_dir("tendi-skill-projection-freshness");
    let workspace = temp.join("workspace");
    let skill_dir = workspace.join(".agents/skills/demo");
    let skill_file = skill_dir.join("SKILL.md");
    let skills_lock = workspace.join("skills-lock.json");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(&skill_file, "old").unwrap();
    fs::write(&skills_lock, "old lock").unwrap();

    let mut scan = report_with_skill("demo", SkillVisibility::Auto).skills;
    scan.roots[0].path = workspace.join(".agents/skills");
    scan.skills[0].paths[0].path = skill_dir;
    scan.skills[0].paths[0].root = workspace.join(".agents/skills");
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

    store.save_skills_for_workspace(&workspace, &scan).unwrap();
    assert!(
        store
            .list_skills_for_workspace(&workspace)
            .unwrap()
            .is_some()
    );
    let manifest = store
        .list_fs_manifest_for_root(&workspace.canonicalize().unwrap())
        .unwrap();
    assert!(!manifest.iter().any(|entry| {
        entry.source_kind == "skill-candidate" && entry.path == skills_lock.canonicalize().unwrap()
    }));

    Connection::open(store.path())
        .unwrap()
        .execute(
            "UPDATE fs_manifest SET parser_version = 'scan-v3' WHERE source_kind = 'skill'",
            [],
        )
        .unwrap();
    assert!(
        store
            .list_skills_for_workspace(&workspace)
            .unwrap()
            .is_none()
    );
    store.save_skills_for_workspace(&workspace, &scan).unwrap();

    fs::write(&skill_file, "edited skill content").unwrap();
    assert!(
        store
            .list_skills_for_workspace(&workspace)
            .unwrap()
            .is_none()
    );
    store.save_skills_for_workspace(&workspace, &scan).unwrap();

    fs::write(&skills_lock, "edited lock content").unwrap();
    assert!(
        store
            .list_skills_for_workspace(&workspace)
            .unwrap()
            .is_some()
    );

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn cached_skill_projection_reads_stale_rows_without_rescanning() {
    let temp = temp_dir("tendi-skill-projection-cached");
    let workspace = temp.join("workspace");
    let skill_dir = workspace.join(".agents/skills/demo");
    let skill_file = skill_dir.join("SKILL.md");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(&skill_file, "old").unwrap();

    let mut scan = report_with_skill("demo", SkillVisibility::Auto).skills;
    scan.roots[0].path = workspace.join(".agents/skills");
    scan.skills[0].paths[0].path = skill_dir;
    scan.skills[0].paths[0].root = workspace.join(".agents/skills");
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    store.save_skills_for_workspace(&workspace, &scan).unwrap();
    fs::write(&skill_file, "edited skill content").unwrap();

    let cached = store
        .list_skills_cached_for_workspace(&workspace)
        .unwrap()
        .unwrap();
    assert_eq!(cached.skills.len(), 1);
    assert_eq!(cached.skills[0].name, "demo");
    assert!(
        store
            .list_skills_for_workspace(&workspace)
            .unwrap()
            .is_none()
    );

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn projection_context_does_not_mix_workspaces_and_retries_empty_domain() {
    let temp = temp_dir("tendi-projection-context");
    let first = temp.join("first");
    let second = temp.join("second");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    let first_path = first.join("AGENTS.md");
    let second_path = second.join("AGENTS.md");
    fs::write(&first_path, "first").unwrap();
    fs::write(&second_path, "second").unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

    let scan = |path: PathBuf, sha256: &str| RuleScan {
        rules: vec![RuleRecord {
            agents: vec![AgentKind::Shared],
            kind: "agents".to_string(),
            scope: "project".to_string(),
            path,
            order: 0,
            sha256: sha256.to_string(),
        }],
        warnings: Vec::new(),
    };
    store
        .save_rules_for_workspace(&first, &scan(first_path, "first"))
        .unwrap();
    assert_eq!(
        store
            .list_rules_for_workspace(&first)
            .unwrap()
            .unwrap()
            .rules[0]
            .sha256,
        "first"
    );
    assert!(store.list_rules_for_workspace(&second).unwrap().is_none());

    store
        .save_rules_for_workspace(&second, &scan(second_path, "second"))
        .unwrap();
    assert_eq!(
        store
            .list_rules_for_workspace(&second)
            .unwrap()
            .unwrap()
            .rules[0]
            .sha256,
        "second"
    );
    assert_eq!(
        store
            .list_rules_for_workspace(&first)
            .unwrap()
            .unwrap()
            .rules[0]
            .sha256,
        "first"
    );

    store
        .save_rules_for_workspace(
            &first,
            &RuleScan {
                rules: Vec::new(),
                warnings: vec!["temporary scan failure".to_string()],
            },
        )
        .unwrap();
    assert!(store.list_rules_for_workspace(&first).unwrap().is_none());
    store
        .save_rules_for_workspace(
            &first,
            &RuleScan {
                rules: Vec::new(),
                warnings: Vec::new(),
            },
        )
        .unwrap();
    assert_eq!(
        store
            .list_rules_for_workspace(&first)
            .unwrap()
            .unwrap()
            .rules
            .len(),
        0
    );

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

fn test_skill_source(name: &str, path: &Path) -> SkillSourceRecord {
    SkillSourceRecord {
        skill_name: name.to_string(),
        skill_path: path.to_path_buf(),
        source_kind: "github".to_string(),
        source: Some(format!("https://github.com/example/{name}.git")),
        source_ref: Some("main".to_string()),
        source_version: Some("abc123".to_string()),
        source_relative_path: Some(format!("skills/{name}")),
        update_status: "tracked".to_string(),
        origin: "tendi-install".to_string(),
    }
}

#[test]
fn skill_source_records_are_scoped_by_workspace() {
    let temp = temp_dir("tendi-storage-skill-source-scope");
    let workspace_a = temp.join("workspace-a");
    let workspace_b = temp.join("workspace-b");
    fs::create_dir_all(&workspace_a).unwrap();
    fs::create_dir_all(&workspace_b).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let source_a = test_skill_source("alpha", &workspace_a.join("skills/alpha"));
    let source_b = test_skill_source("beta", &workspace_b.join("skills/beta"));

    store
        .upsert_skill_source_records_for_workspace(&workspace_a, std::slice::from_ref(&source_a))
        .unwrap();
    store
        .upsert_skill_source_records_for_workspace(&workspace_b, std::slice::from_ref(&source_b))
        .unwrap();

    assert_eq!(
        store
            .skill_source_records_for_workspace(&workspace_a)
            .unwrap()
            .iter()
            .map(|record| record.skill_name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha"]
    );
    assert_eq!(
        store
            .skill_source_records_for_workspace(&workspace_b)
            .unwrap()
            .iter()
            .map(|record| record.skill_name.as_str())
            .collect::<Vec<_>>(),
        vec!["beta"]
    );
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn installation_source_records_do_not_leak_into_workspace_scope() {
    let temp = temp_dir("tendi-storage-skill-source-installation-scope");
    let workspace = temp.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

    let legacy_path = temp.join("shared-skills/legacy");
    let shared_path = temp.join("shared-skills/shared");
    let legacy = test_skill_source("legacy", &legacy_path);
    let legacy_shared = test_skill_source("legacy-shared", &shared_path);
    store
        .upsert_skill_source_records(&[legacy, legacy_shared])
        .unwrap();

    let mut scoped_shared = test_skill_source("scoped-shared", &shared_path);
    scoped_shared.source_kind = "local".to_string();
    scoped_shared.source = None;
    store
        .upsert_skill_source_records_for_workspace(&workspace, &[scoped_shared])
        .unwrap();

    let records = store
        .skill_source_records_for_workspace(&workspace)
        .unwrap();
    assert_eq!(
        records
            .iter()
            .map(|record| record.skill_name.as_str())
            .collect::<Vec<_>>(),
        vec!["scoped-shared"]
    );
    assert_eq!(records[0].source_kind, "local");

    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn saving_skills_persists_scanned_source_records_for_workspace() {
    let temp = temp_dir("tendi-storage-skill-source-scan-persistence");
    let workspace = temp.join("workspace");
    let skill_path = workspace.join(".agents/skills/remote");
    fs::create_dir_all(&workspace).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

    let mut scan = report_with_skill("remote", SkillVisibility::Auto).skills;
    let path = &mut scan.skills[0].paths[0];
    path.path = skill_path.clone();
    path.root = skill_path.parent().unwrap().to_path_buf();
    path.scope = "global".to_string();
    path.source_kind = "github".to_string();
    path.source = Some("https://github.com/example/remote-skills.git".to_string());
    path.source_relative_path = Some("skills/remote".to_string());
    path.update_status = "checkable".to_string();

    store.save_skills_for_workspace(&workspace, &scan).unwrap();

    let records = store
        .skill_source_records_for_workspace(&workspace)
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source_kind, "github");
    assert_eq!(records[0].origin, "projection-scan");

    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn scoped_skill_snapshots_round_trip_without_cross_workspace_merge() {
    let temp = temp_dir("tendi-storage-scoped-skill-snapshot");
    let workspace_a = temp.join("workspace-a");
    let workspace_b = temp.join("workspace-b");
    fs::create_dir_all(&workspace_a).unwrap();
    fs::create_dir_all(&workspace_b).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let path = temp.join("shared-path/skill");
    let snapshot_a = SkillSnapshot {
        skill_path: path.clone(),
        source_version: "a".to_string(),
        files: vec![SkillSnapshotFile {
            relative_path: "SKILL.md".to_string(),
            content: b"workspace-a".to_vec(),
        }],
    };
    let snapshot_b = SkillSnapshot {
        skill_path: path.clone(),
        source_version: "b".to_string(),
        files: vec![SkillSnapshotFile {
            relative_path: "SKILL.md".to_string(),
            content: b"workspace-b".to_vec(),
        }],
    };
    store
        .replace_skill_snapshots_for_workspace(&workspace_a, std::slice::from_ref(&snapshot_a))
        .unwrap();
    store
        .replace_skill_snapshots_for_workspace(&workspace_b, std::slice::from_ref(&snapshot_b))
        .unwrap();

    assert!(store.skill_snapshot(&path).unwrap().is_none());

    let snapshot_a = store
        .skill_snapshot_for_workspace(&workspace_a, &path)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot_a.source_version, "a");
    assert_eq!(snapshot_a.files[0].content, b"workspace-a");
    let snapshot_b = store
        .skill_snapshot_for_workspace(&workspace_b, &path)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot_b.source_version, "b");
    assert_eq!(snapshot_b.files[0].content, b"workspace-b");

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn checked_skill_persistence_rejects_a_stale_source_version() {
    let temp = temp_dir("tendi-storage-stale-skill-source");
    let workspace = temp.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let path = workspace.join("skills/demo");
    let mut current = test_skill_source("demo", &path);
    current.source_version = Some("version-a".to_string());
    store
        .upsert_skill_source_records_for_workspace(&workspace, std::slice::from_ref(&current))
        .unwrap();

    let mut next = current.clone();
    next.source_version = Some("version-b".to_string());
    store
        .persist_skill_update_persistence_for_workspace_checked(
            &workspace,
            &[(path.clone(), Some("version-a".to_string()))],
            std::slice::from_ref(&next),
            &[],
        )
        .unwrap();

    let mut stale = next.clone();
    stale.source_version = Some("version-c".to_string());
    let error = store
        .persist_skill_update_persistence_for_workspace_checked(
            &workspace,
            &[(path, Some("version-a".to_string()))],
            std::slice::from_ref(&stale),
            &[],
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("changed after the update preview")
    );

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn checked_skill_persistence_rejects_a_mismatched_snapshot_version() {
    let temp = temp_dir("tendi-storage-mismatched-skill-snapshot");
    let workspace = temp.join("workspace");
    let path = workspace.join("skills/demo");
    fs::create_dir_all(&path).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let mut record = test_skill_source("demo", &path);
    record.source_version = Some("version-b".to_string());

    let error = store
        .persist_skill_update_persistence_for_workspace_checked(
            &workspace,
            &[(path.clone(), Some("version-a".to_string()))],
            std::slice::from_ref(&record),
            &[SkillSnapshot {
                skill_path: path,
                source_version: "version-a".to_string(),
                files: vec![SkillSnapshotFile {
                    relative_path: "SKILL.md".to_string(),
                    content: b"base".to_vec(),
                }],
            }],
        )
        .unwrap_err();
    assert!(error.to_string().contains("source version"));

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn stale_skill_preflight_leaves_filesystem_untouched() {
    let temp = temp_dir("tendi-storage-stale-skill-preflight");
    let workspace = temp.join("workspace");
    let skill_path = workspace.join("skills/demo/SKILL.md");
    fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
    fs::write(&skill_path, "before\n").unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let source_path = skill_path.parent().unwrap().to_path_buf();
    let mut current = test_skill_source("demo", &source_path);
    current.source_version = Some("version-b".to_string());
    store
        .upsert_skill_source_records_for_workspace(&workspace, std::slice::from_ref(&current))
        .unwrap();

    let error = store
        .validate_skill_source_versions_for_workspace(
            &workspace,
            &[(source_path, Some("version-a".to_string()))],
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("changed after the update preview")
    );
    assert_eq!(fs::read_to_string(&skill_path).unwrap(), "before\n");

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

fn report_with_skill(name: &str, visibility: SkillVisibility) -> ScanReport {
    let root = PathBuf::from("/tmp/tendi-test/.agents/skills");
    let path = root.join(name);
    ScanReport {
        agents: AgentScan {
            agents: Vec::new(),
            warnings: Vec::new(),
        },
        skills: SkillScan {
            roots: vec![SkillRoot {
                path: root.clone(),
                scope: "project".to_string(),
                agent: AgentKind::Shared,
                plugin_id: None,
                plugin_enabled: None,
            }],
            skills: vec![SkillRecord {
                id: name.to_string(),
                installation_id: name.to_string(),
                name: name.to_string(),
                description: Some("demo".to_string()),
                tags: Vec::new(),
                dependencies: Vec::new(),
                dependents: Vec::new(),
                dependency_ids: Vec::new(),
                dependent_ids: Vec::new(),
                is_wrapper: false,
                visibility,
                agents: vec![AgentKind::Shared],
                paths: vec![SkillPath {
                    path,
                    root,
                    scope: "project".to_string(),
                    agent: AgentKind::Shared,
                    install_target: "shared:/tmp/tendi-test/.agents/skills".to_string(),
                    source_kind: "local".to_string(),
                    source: Some("local:/tmp/tendi-test/.agents/skills".to_string()),
                    source_ref: None,
                    source_version: None,
                    source_relative_path: None,
                    symlink_status: "local".to_string(),
                    update_status: "local".to_string(),
                    sha256: "abc123".to_string(),
                    tags: Vec::new(),
                    tendi_visibility: Some(visibility),
                    effective_visibility: visibility,
                    provider_allow_implicit_invocation: None,
                    provider_skill_enabled: None,
                    provider_disable_model_invocation: None,
                    plugin_id: None,
                    plugin_enabled: None,
                }],
                source_summary: "local:/tmp/tendi-test/.agents/skills".to_string(),
                install_targets: vec!["shared:/tmp/tendi-test/.agents/skills".to_string()],
                update_status: "local".to_string(),
                is_system: false,
                ctime: None,
                mtime: None,
            }],
            warnings: Vec::new(),
        },
        sessions: SessionScan {
            sessions: Vec::new(),
            warnings: Vec::new(),
        },
        rules: RuleScan {
            rules: Vec::new(),
            warnings: Vec::new(),
        },
        hooks: HookScan {
            hooks: Vec::new(),
            warnings: Vec::new(),
        },
        mcp: McpScan {
            servers: Vec::new(),
            warnings: Vec::new(),
        },
    }
}

#[test]
fn scoped_session_projection_keeps_workspaces_isolated() {
    let temp = temp_dir("tendi-storage-scoped-sessions");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let first_scope = ScopeKey::new("workspace:first").unwrap();
    let second_scope = ScopeKey::new("workspace:second").unwrap();
    let first_path = temp.join("first.jsonl");
    let second_path = temp.join("second.jsonl");
    let transcript = |marker: &str| {
        include_str!("../testdata/transcripts/codex.jsonl")
            .replace("Inspect the fixture parser", marker)
    };
    fs::write(&first_path, transcript("first-private-marker")).unwrap();
    fs::write(&second_path, transcript("second-private-marker")).unwrap();
    let mut first_session = session("shared", "First workspace");
    first_session.path = first_path;
    let mut second_session = session("shared", "Second workspace");
    second_session.path = second_path;

    store
        .save_sessions_at_for_scope(
            &first_scope,
            &SessionScan {
                sessions: vec![first_session],
                warnings: Vec::new(),
            },
            1,
        )
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&first_scope)
        .unwrap();
    store
        .save_sessions_at_for_scope(
            &second_scope,
            &SessionScan {
                sessions: vec![second_session],
                warnings: Vec::new(),
            },
            2,
        )
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&second_scope)
        .unwrap();

    let first = store.list_sessions_for_scope(&first_scope).unwrap();
    let second = store.list_sessions_for_scope(&second_scope).unwrap();
    assert_eq!(first.sessions[0].title.as_deref(), Some("First workspace"));
    assert_eq!(
        second.sessions[0].title.as_deref(),
        Some("Second workspace")
    );
    assert_eq!(
        store.sessions_last_scan_at_for_scope(&first_scope).unwrap(),
        Some(1)
    );
    assert_eq!(
        store
            .sessions_last_scan_at_for_scope(&second_scope)
            .unwrap(),
        Some(2)
    );
    assert_eq!(
        store
            .search_sessions_for_scope(&first_scope, "first", None)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .search_sessions_for_scope(&second_scope, "first", None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .search_sessions_for_scope(&first_scope, "first-private-marker", None)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .search_sessions_for_scope(&second_scope, "first-private-marker", None)
            .unwrap()
            .is_empty()
    );
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn scoped_analytics_ignores_records_from_another_workspace() {
    let temp = temp_dir("tendi-storage-scoped-analytics");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let first_scope = ScopeKey::new("workspace:first").unwrap();
    let second_scope = ScopeKey::new("workspace:second").unwrap();
    let mut first_session = session("same", "First");
    first_session.path = PathBuf::from("/tmp/scope-first.jsonl");
    let mut second_session = session("same", "Second");
    second_session.path = PathBuf::from("/tmp/scope-second.jsonl");
    store
        .save_sessions_at_for_scope(
            &first_scope,
            &SessionScan {
                sessions: vec![first_session.clone()],
                warnings: Vec::new(),
            },
            1,
        )
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&first_scope)
        .unwrap();
    store
        .save_sessions_at_for_scope(
            &second_scope,
            &SessionScan {
                sessions: vec![second_session.clone()],
                warnings: Vec::new(),
            },
            1,
        )
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&second_scope)
        .unwrap();
    let mut first_analytics =
        analytics_record("same", AgentKind::Codex, "2026-08-28T10:00:00Z", 11);
    first_analytics.analytics.session_path = first_session.path;
    let mut second_analytics =
        analytics_record("same", AgentKind::Codex, "2026-08-28T10:00:00Z", 99);
    second_analytics.analytics.session_path = second_session.path;
    store
        .save_session_analytics_records_for_scope(&first_scope, &[first_analytics])
        .unwrap();
    store
        .save_session_analytics_records_for_scope(&second_scope, &[second_analytics])
        .unwrap();

    let overview = store
        .overview_analytics_for_scope(&first_scope, None, 30, 30)
        .unwrap();
    assert_eq!(overview.coverage.total_sessions, 1);
    assert_eq!(overview.summary.usage.total_tokens, 11);
    assert_eq!(overview.summary.sessions, 1);

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn scoped_analytics_indexing_count_ignores_sessions_outside_requested_range() {
    let temp = temp_dir("tendi-storage-analytics-coverage");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let scope = ScopeKey::new("workspace:coverage").unwrap();
    let mut recent_session = session("recent", "Recent");
    recent_session.path = PathBuf::from("/tmp/scope-recent.jsonl");
    let mut old_session = session("old", "Old");
    old_session.path = PathBuf::from("/tmp/scope-old.jsonl");
    store
        .save_sessions_at_for_scope(
            &scope,
            &SessionScan {
                sessions: vec![recent_session.clone(), old_session.clone()],
                warnings: Vec::new(),
            },
            1,
        )
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&scope)
        .unwrap();

    let mut recent_analytics =
        analytics_record("recent", AgentKind::Codex, &Local::now().to_rfc3339(), 11);
    recent_analytics.analytics.session_path = recent_session.path;
    let mut old_analytics = analytics_record("old", AgentKind::Codex, "2020-01-01T10:00:00Z", 99);
    old_analytics.analytics.session_path = old_session.path;
    store
        .save_session_analytics_records_for_scope(&scope, &[recent_analytics, old_analytics])
        .unwrap();

    let overview = store
        .overview_analytics_for_scope(&scope, None, 30, 30)
        .unwrap();
    assert_eq!(overview.coverage.total_sessions, 2);
    assert_eq!(overview.coverage.first.as_deref(), Some("2020-01-01"));
    assert_eq!(overview.coverage.analyzed_sessions, 2);
    assert_eq!(overview.coverage.indexing_sessions, 0);
    assert_eq!(overview.summary.sessions, 1);

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn concurrent_store_open_serializes_schema_transitions() {
    let temp = temp_dir("tendi-storage-concurrent-open");
    fs::create_dir_all(&temp).unwrap();
    let db = temp.join("tendi.sqlite3");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles = (0..2)
        .map(|_| {
            let db = db.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                Store::open(db).map(|store| drop(store))
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    let store = Store::open(&db).unwrap();
    let current_tables = store
        .conn
        .prepare(
            "SELECT name FROM sqlite_master
                 WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(current_tables.contains(&"scoped_sessions".to_string()));
    for removed in [
        "agents",
        "skills",
        "skill_paths",
        "skill_sources",
        "skill_snapshots",
        "projection_contexts",
        "sessions",
        "session_skill_index",
        "session_skill_links",
        "rules",
        "hooks",
        "mcp_servers",
    ] {
        assert!(
            !current_tables.iter().any(|table| table == removed),
            "{removed}"
        );
    }
    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn concurrent_writers_complete_one_hundred_rounds_without_sqlite_lock_errors() {
    let temp = temp_dir("tendi-storage-writer-soak");
    fs::create_dir_all(&temp).unwrap();
    let db = temp.join("tendi.sqlite3");
    Store::open(&db).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let handles = (0..4)
        .map(|worker| {
            let db = db.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let store = Store::open(&db).unwrap();
                let baseline = store.app_settings().unwrap();
                barrier.wait();
                for round in 0..100 {
                    let mut settings = baseline.clone();
                    settings.terminal = format!("worker-{worker}-{round}");
                    let saved = store.save_app_settings(settings).unwrap();
                    assert_eq!(saved.terminal, format!("worker-{worker}-{round}"));
                }
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().unwrap();
    }
    drop(Store::open(&db).unwrap());
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn store_handles_share_one_writer_and_main_database_reads_cannot_write() {
    let temp = temp_dir("tendi-storage-connection-ownership");
    fs::create_dir_all(&temp).unwrap();
    let first = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let second = Store::open(temp.join(".").join("tendi.sqlite3")).unwrap();
    assert!(std::sync::Arc::ptr_eq(&first.writer, &second.writer));
    let error = first
        .conn
        .execute(
            "INSERT INTO meta (key, value) VALUES ('illegal-write', '1')",
            [],
        )
        .unwrap_err();
    assert_eq!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::ReadOnly)
    );
    // Read connections can still materialize search candidates in TEMP.
    first.conn.execute_batch("CREATE TEMP TABLE read_candidates (id INTEGER); INSERT INTO read_candidates VALUES (1);").unwrap();
    drop((first, second));
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn nested_write_rejects_once_and_rolls_back_the_outer_transaction() {
    let temp = temp_dir("tendi-storage-nested-write");
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let calls = std::cell::Cell::new(0);
    let error = store
        .with_named_write_transaction("outer-test", |tx| {
            calls.set(calls.get() + 1);
            tx.execute(
                "INSERT INTO meta (key, value) VALUES ('rolled-back', '1')",
                [],
            )?;
            store.record_operation(&OperationRecord {
                operation_id: OperationId::new("nested-test").unwrap(),
                kind: crate::runtime_contract::OperationKind::Scan,
                scope_key: ScopeKey::new("workspace:nested").unwrap(),
                status: OperationStatus::Queued,
                input_revision: Revision::ZERO,
                source_version: None,
                checkpoint_json: None,
                error: None,
            })
        })
        .unwrap_err();
    assert!(error.chain().any(|error| {
        error.downcast_ref::<super::database_writer::AdmissionError>()
            == Some(&super::database_writer::AdmissionError::Reentrant)
    }));
    assert_eq!(calls.get(), 1);
    assert_eq!(
        store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM meta WHERE key = 'rolled-back'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn prompts_can_be_saved_updated_and_deleted() {
    let temp = temp_dir("tendi-storage-prompts");
    fs::create_dir_all(&temp).unwrap();
    let db = temp.join("tendi.sqlite3");
    let store = Store::open(&db).unwrap();

    let saved = store
        .save_prompt(PromptWrite {
            id: None,
            title: "Review".to_string(),
            tags: vec!["Code".to_string(), "Review".to_string()],
            body: "Review this diff".to_string(),
        })
        .unwrap();

    assert_eq!(store.list_prompts().unwrap().len(), 1);
    assert_eq!(
        store.list_prompts().unwrap()[0].tags,
        vec!["Code", "Review"]
    );

    let updated = store
        .save_prompt(PromptWrite {
            id: Some(saved.id.clone()),
            title: "Review updated".to_string(),
            tags: vec!["Code".to_string(), "Deep".to_string()],
            body: "Review this diff carefully".to_string(),
        })
        .unwrap();

    assert_eq!(updated.id, saved.id);
    assert_eq!(updated.created_at, saved.created_at);
    let prompts = store.list_prompts().unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].title, "Review updated");
    assert_eq!(prompts[0].tags, vec!["Code", "Deep"]);

    assert_eq!(store.delete_prompts(&[saved.id]).unwrap(), 1);
    assert!(store.list_prompts().unwrap().is_empty());

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn assistant_chat_messages_persist_without_empty_sessions() {
    let temp = temp_dir("tendi-storage-assistant-chat");
    let db = temp.join("tendi.sqlite3");
    let workspace = temp.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let store = Store::open(&db).unwrap();

    assert!(store.list_assistant_chat_sessions().unwrap().is_empty());

    let linked_session = AssistantSessionLink {
        id: "transcript-1".to_string(),
        agent: "codex".to_string(),
        path: workspace.join("transcript.jsonl").display().to_string(),
    };
    store
        .append_assistant_chat_message(
            "assistant-chat-1",
            &workspace,
            Some(&linked_session),
            &AssistantMessage {
                role: "user".to_string(),
                content: "Summarize this session".to_string(),
            },
        )
        .unwrap();
    store
        .append_assistant_chat_message(
            "assistant-chat-1",
            &workspace,
            None,
            &AssistantMessage {
                role: "assistant".to_string(),
                content: "The session is ready to review.".to_string(),
            },
        )
        .unwrap();

    let sessions = Store::open(&db)
        .unwrap()
        .list_assistant_chat_sessions()
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "assistant-chat-1");
    assert_eq!(sessions[0].messages.len(), 2);
    assert_eq!(sessions[0].messages[0].role, "user");
    assert_eq!(sessions[0].messages[1].role, "assistant");
    assert_eq!(sessions[0].linked_session, Some(linked_session));

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn prompts_reject_empty_titles_before_writing() {
    let temp = temp_dir("tendi-storage-prompts-empty-title");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

    for (index, title) in ["", "  \n\t"].into_iter().enumerate() {
        let error = store
            .save_prompt(PromptWrite {
                id: Some(format!("prompt-empty-title-{index}")),
                title: title.to_string(),
                tags: Vec::new(),
                body: "Body".to_string(),
            })
            .expect_err("empty prompt title must be rejected");
        assert_eq!(error.to_string(), "prompt title is required");
    }

    assert!(store.list_prompts().unwrap().is_empty());
    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn malformed_prompt_tags_are_not_replaced_by_category() {
    let temp = temp_dir("tendi-storage-prompts-invalid-tags");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    store
        .save_prompt(PromptWrite {
            id: Some("prompt-test".to_string()),
            title: "Review".to_string(),
            tags: vec!["Code".to_string()],
            body: "Review this diff".to_string(),
        })
        .unwrap();
    Connection::open(store.path()).unwrap().execute(
                "UPDATE prompts SET category = 'legacy', tags_json = 'not-json' WHERE id = 'prompt-test'",
                [],
            )
            .unwrap();

    assert!(store.list_prompts().is_err());

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

fn session(id: &str, title: &str) -> SessionRecord {
    SessionRecord {
        id: id.to_string(),
        agent: AgentKind::Codex,
        title: Some(title.to_string()),
        project: Some(PathBuf::from("/tmp/tendi-test")),
        repository: None,
        repository_url: None,
        logical_project_id: None,
        logical_project_name: None,
        path: PathBuf::from(format!("/tmp/tendi-test/{id}.jsonl")),
        started_at: Some("2026-06-23T10:00:00Z".to_string()),
        updated_at: Some(format!("2026-06-23T10:0{}:00Z", id.len())),
        message_count: Some(id.len()),
        first_user_message: None,
        last_user_message: None,
        last_assistant_message: None,
        turn_count: Some(id.len()),
        model: None,
        mode: None,
        approval_mode: None,
        is_run_everything: None,
        parent_session_id: None,
        token_usage: None,
    }
}

fn analytics_record(
    id: &str,
    agent: AgentKind,
    timestamp: &str,
    total_tokens: u64,
) -> SessionAnalyticsRecord {
    let usage = AnalyticsTokenUsage {
        input_tokens: total_tokens,
        total_tokens,
        ..AnalyticsTokenUsage::default()
    };
    SessionAnalyticsRecord {
        analytics: SessionAnalytics {
            session_id: id.to_string(),
            agent,
            session_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            responses: vec![AnalyticsResponseUsage {
                index: 1,
                timestamp: timestamp.to_string(),
                model: "test-model".to_string(),
                usage,
                cumulative: usage,
            }],
            ..SessionAnalytics::default()
        },
        state: AnalyticsParserState::default(),
        file_mtime: 0,
        file_size: 0,
    }
}

#[test]
fn scoped_session_skill_links_do_not_cross_workspace_boundaries() {
    let temp = temp_dir("tendi-storage-scoped-session-skills");
    fs::create_dir_all(&temp).unwrap();
    let db = temp.join("tendi.sqlite3");
    let transcript = temp.join("same.jsonl");
    fs::write(&transcript, "{}\n").unwrap();
    let store = Store::open(&db).unwrap();
    let first_scope = ScopeKey::new("workspace:/first").unwrap();
    let second_scope = ScopeKey::new("workspace:/second").unwrap();
    let mut session = session("same", "Same session");
    session.path = transcript;
    let scan = SessionScan {
        sessions: vec![session.clone()],
        warnings: Vec::new(),
    };
    store
        .save_sessions_at_for_scope(&first_scope, &scan, 1)
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&first_scope)
        .unwrap();
    store
        .save_sessions_at_for_scope(&second_scope, &scan, 1)
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&second_scope)
        .unwrap();
    let state = SessionFileState {
        file_mtime: 1,
        file_size: 3,
    };

    store
        .replace_session_skill_links_for_scope(
            &first_scope,
            &session,
            &state,
            &[link(&session, "first-skill")],
        )
        .unwrap();
    store
        .replace_session_skill_links_for_scope(
            &second_scope,
            &session,
            &state,
            &[link(&session, "second-skill")],
        )
        .unwrap();

    assert_eq!(
        store
            .skill_session_links_for_scope(&first_scope, &[skill_path("first-skill")])
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .skill_session_links_for_scope(&first_scope, &[skill_path("second-skill")])
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .session_skill_index_status_for_scope(&first_scope, false)
            .unwrap()
            .indexed,
        1
    );
    assert_eq!(
        store
            .session_skill_index_status_for_scope(&second_scope, false)
            .unwrap()
            .indexed,
        1
    );

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn skill_session_links_exclude_child_sessions() {
    let temp = temp_dir("tendi-storage-skill-links-no-children");
    fs::create_dir_all(&temp).unwrap();
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let scope = ScopeKey::new("workspace:/skill-links").unwrap();
    let parent = session("parent", "Parent session");
    let mut child = session("child", "Child session");
    child.parent_session_id = Some(parent.id.clone());
    let scan = SessionScan {
        sessions: vec![parent.clone(), child.clone()],
        warnings: Vec::new(),
    };
    store.save_sessions_at_for_scope(&scope, &scan, 1).unwrap();
    store
        .ensure_scoped_session_search_for_scope(&scope)
        .unwrap();
    let state = SessionFileState {
        file_mtime: 1,
        file_size: 1,
    };
    store
        .replace_session_skill_links_for_scope(
            &scope,
            &parent,
            &state,
            &[link(&parent, "shared-skill")],
        )
        .unwrap();
    store
        .replace_session_skill_links_for_scope(
            &scope,
            &child,
            &state,
            &[link(&child, "shared-skill")],
        )
        .unwrap();

    let links = store
        .skill_session_links_for_scope(&scope, &[skill_path("shared-skill")])
        .unwrap();
    assert_eq!(
        links
            .iter()
            .map(|item| item.session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["parent"]
    );

    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

fn link(session: &SessionRecord, skill_name: &str) -> SessionSkillLink {
    SessionSkillLink {
        session_id: session.id.clone(),
        agent: session.agent,
        session_path: session.path.clone(),
        session_title: session.title.clone(),
        session_project: session.project.clone(),
        session_started_at: session.started_at.clone(),
        session_updated_at: session.updated_at.clone(),
        session_message_count: session.message_count,
        skill_name: skill_name.to_string(),
        skill_path: skill_path(skill_name),
        skill_agent: Some(AgentKind::Codex),
        skill_scope: Some("global".to_string()),
        evidence_kind: "exec_command".to_string(),
        evidence_text: format!("cat /tmp/tendi-test/.codex/skills/{skill_name}/SKILL.md"),
        evidence_time: Some("2026-06-23T10:00:00Z".to_string()),
        confidence: "observed".to_string(),
    }
}

fn skill_path(skill_name: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/tendi-test/.codex/skills/{skill_name}"))
}

#[test]
fn projection_heads_are_monotonic_per_scope_and_domain() {
    let temp = temp_dir("tendi-projection-head");
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let scope = ScopeKey::new("workspace:/repo").unwrap();
    let source = SourceVersion::new("sha-1").unwrap();

    assert!(store.projection_head(&scope, "sessions").unwrap().is_none());
    let first = store
        .advance_projection_head(&scope, "sessions", Some(&source), "ready")
        .unwrap();
    let second = store
        .advance_projection_head(&scope, "sessions", Some(&source), "ready")
        .unwrap();

    assert_eq!(first.revision, Revision::new(1));
    assert_eq!(second.revision, Revision::new(2));
    assert_eq!(
        store
            .projection_head(&scope, "sessions")
            .unwrap()
            .unwrap()
            .revision,
        Revision::new(2)
    );
}

#[test]
fn operation_journal_round_trips_terminal_state_and_error() {
    let temp = temp_dir("tendi-operation-journal");
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let operation = OperationRecord {
        operation_id: OperationId::new("op-1").unwrap(),
        kind: OperationKind::Scan,
        scope_key: ScopeKey::new("workspace:/repo").unwrap(),
        status: OperationStatus::Running,
        input_revision: Revision::new(4),
        source_version: Some(SourceVersion::new("sha-1").unwrap()),
        checkpoint_json: Some("{\"offset\":12}".to_string()),
        error: None,
    };
    store.record_operation(&operation).unwrap();
    assert_eq!(
        store.operation(&operation.operation_id).unwrap(),
        Some(operation.clone())
    );

    assert!(
        store
            .update_operation(
                &operation.operation_id,
                OperationStatus::Failed,
                operation.checkpoint_json.as_deref(),
                Some("provider failed"),
            )
            .unwrap()
    );
    let saved = store.operation(&operation.operation_id).unwrap().unwrap();
    assert_eq!(saved.status, OperationStatus::Failed);
    assert_eq!(saved.error.as_deref(), Some("provider failed"));
}

#[test]
fn recovery_marks_unfinished_operations_as_failed() {
    let temp = temp_dir("tendi-operation-recovery");
    let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
    let scope = ScopeKey::new("workspace:/repo").unwrap();
    for (id, status) in [
        ("queued-op", OperationStatus::Queued),
        ("running-op", OperationStatus::Running),
        ("committing-op", OperationStatus::Committing),
        ("committed-op", OperationStatus::Committed),
    ] {
        store
            .record_operation(&OperationRecord {
                operation_id: OperationId::new(id).unwrap(),
                kind: OperationKind::Projection,
                scope_key: scope.clone(),
                status,
                input_revision: Revision::ZERO,
                source_version: None,
                checkpoint_json: None,
                error: None,
            })
            .unwrap();
    }

    assert_eq!(store.recover_inflight_operations().unwrap(), 3);
    for id in ["queued-op", "running-op", "committing-op"] {
        let operation = store
            .operation(&OperationId::new(id).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(operation.status, OperationStatus::Failed);
        assert_eq!(
            operation.error.as_deref(),
            Some("daemon restarted before the operation completed")
        );
    }
    assert_eq!(
        store
            .operation(&OperationId::new("committed-op").unwrap())
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::Committed
    );
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
}
