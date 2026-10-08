use super::*;

fn fixture() -> (PathBuf, Store, ScopeKey) {
    let root = std::env::temp_dir().join(format!(
        "tendi-recall-{}-{}",
        std::process::id(),
        unix_now_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let store = Store::open(root.join("test.sqlite3")).unwrap();
    (root, store, ScopeKey::new("workspace:recall-test").unwrap())
}

fn unix_now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn message(agent: AgentKind, role: &str, body: &str) -> String {
    let content = serde_json::json!([{ "type": if role == "user" { "input_text" } else { "output_text" }, "text": body }]);
    let value = match agent {
        AgentKind::Codex => {
            serde_json::json!({"type":"response_item","timestamp":"2026-06-23T15:48:04Z","payload":{"type":"message","role":role,"content":content}})
        }
        AgentKind::Claude => {
            serde_json::json!({"type":role,"timestamp":"2026-06-23T15:48:04Z","message":{"role":role,"content":body}})
        }
        AgentKind::Cursor => {
            serde_json::json!({"role":role,"message":{"content":[{"type":"text","text":body}]}})
        }
        _ => unreachable!(),
    };
    value.to_string() + "\n"
}

fn save(
    root: &Path,
    store: &Store,
    scope: &ScopeKey,
    id: &str,
    agent: AgentKind,
    project: &str,
    time: &str,
    body: &str,
) -> SessionRecord {
    let session: SessionRecord = serde_json::from_value(serde_json::json!({
        "id":id,"agent":agent,"title":id,"path":root.join(format!("{id}.jsonl")),"project":project,"started_at":time
    })).unwrap();
    fs::write(&session.path, body).unwrap();
    store
        .apply_session_changes_for_scope(scope, &[session.clone()], &[])
        .unwrap();
    let (_, errors, pending) = store
        .refresh_pending_session_search_for_scope(scope)
        .unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(!pending);
    session
}

#[test]
fn provider_roles_dates_cwd_and_pagination_are_filtered_in_core() {
    let (root, store, scope) = fixture();
    for (i, agent) in [AgentKind::Codex, AgentKind::Claude, AgentKind::Cursor]
        .into_iter()
        .enumerate()
    {
        save(
            &root,
            &store,
            &scope,
            &format!("provider-{i}"),
            agent,
            "/history/douvo/worktree",
            &format!("2026-06-2{}T12:00:00Z", i + 1),
            &(message(agent, "user", "引入叭哥说登录")
                + &message(agent, "assistant", "assistant-only needle")),
        );
    }
    save(
        &root,
        &store,
        &scope,
        "sibling",
        AgentKind::Codex,
        "/history/douvo-other",
        "2026-06-22T12:00:00Z",
        &message(AgentKind::Codex, "user", "引入叭哥说登录"),
    );
    let options = SessionRecallOptions {
        query: "叭哥说".into(),
        cwd: vec!["/history/douvo".into()],
        since: Some("2026-06-22".into()),
        until: Some("2026-06-24T00:00:00+08:00".into()),
        role: Some(SessionRecallRole::User),
        sort: SessionRecallSort::TimeAsc,
        limit: 1,
        ..Default::default()
    };
    let page = store.recall_sessions(&options).unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.hits[0].id, "provider-1");
    assert_eq!(page.hits[0].role, "user");
    assert!(page.hits[0].record_order > 0);
    assert_eq!(
        store
            .recall_sessions(&SessionRecallOptions {
                offset: 1,
                ..options.clone()
            })
            .unwrap()
            .hits[0]
            .id,
        "provider-2"
    );
    assert!(
        store
            .recall_sessions(&SessionRecallOptions {
                exact_cwd: true,
                ..options.clone()
            })
            .unwrap()
            .hits
            .is_empty()
    );
    assert!(
        store
            .recall_sessions(&SessionRecallOptions {
                query: "assistant-only".into(),
                ..options.clone()
            })
            .unwrap()
            .hits
            .is_empty()
    );
    assert_eq!(
        store
            .recall_sessions(&SessionRecallOptions {
                query: "assistant-only".into(),
                role: Some(SessionRecallRole::Assistant),
                ..options
            })
            .unwrap()
            .total,
        2
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn same_message_matching_phrase_and_frequency_saturation() {
    let (root, store, scope) = fixture();
    let agent = AgentKind::Codex;
    save(
        &root,
        &store,
        &scope,
        "original",
        agent,
        "/project",
        "2026-06-23T00:00:00Z",
        &message(agent, "user", "增加 prompt tab，保存复制"),
    );
    save(
        &root,
        &store,
        &scope,
        "review",
        agent,
        "/project",
        "2026-09-23T00:00:00Z",
        &message(agent, "assistant", &"prompt tab ".repeat(500)),
    );
    save(
        &root,
        &store,
        &scope,
        "separated",
        agent,
        "/project",
        "2026-09-23T00:00:00Z",
        &(message(agent, "user", "prompt") + &message(agent, "user", "tab")),
    );
    let options = SessionRecallOptions {
        query: "prompt tab".into(),
        ..Default::default()
    };
    let page = store.recall_sessions(&options).unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.hits[0].id, "original");
    assert!(page.hits[0].snippet.contains("⟦prompt⟧"));
    assert!(page.hits[0].snippet.contains("⟦tab⟧"));
    assert_eq!(
        store
            .recall_sessions(&SessionRecallOptions {
                query: "prompt   tab".into(),
                phrase: true,
                ..options.clone()
            })
            .unwrap()
            .total,
        0
    );
    assert_eq!(
        store
            .recall_sessions(&SessionRecallOptions {
                phrase: true,
                ..options
            })
            .unwrap()
            .total,
        2
    );
    assert!(
        store
            .recall_sessions(&SessionRecallOptions {
                query: "needle".into(),
                since: Some("oops".into()),
                ..Default::default()
            })
            .is_err()
    );
    assert!(
        store
            .recall_sessions(&SessionRecallOptions {
                query: "needle".into(),
                since: Some("2026-06-24".into()),
                until: Some("2026-06-23".into()),
                ..Default::default()
            })
            .is_err()
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn published_source_content_is_reused_across_scopes_and_invalidated_on_append() {
    use std::io::Write;
    let (root, store, scope) = fixture();
    let session = save(
        &root,
        &store,
        &scope,
        "shared",
        AgentKind::Codex,
        "/project",
        "2026-06-23T00:00:00Z",
        &message(AgentKind::Codex, "user", "cachedneedle"),
    );
    let other = ScopeKey::new("workspace:new-subdirectory").unwrap();
    store
        .apply_session_changes_for_scope(&other, &[session.clone()], &[])
        .unwrap();
    let (_, errors, pending) = store
        .refresh_pending_session_search_for_scope(&other)
        .unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(!pending);
    let checkpoints: Vec<String> = store
        .conn
        .prepare("SELECT search_checkpoint FROM scoped_session_search_index ORDER BY scope_key")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(checkpoints.len(), 2);
    assert_eq!(checkpoints[0], checkpoints[1]);
    let contents: i64=store.conn.query_row("SELECT count(DISTINCT content_id) FROM scoped_session_search_entries WHERE record_order > 0",[],|row|row.get(0)).unwrap();
    assert_eq!(contents, 1);
    let options = SessionRecallOptions {
        query: "cachedneedle".into(),
        ..Default::default()
    };
    assert_eq!(store.recall_sessions(&options).unwrap().total, 1);
    fs::OpenOptions::new()
        .append(true)
        .open(&session.path)
        .unwrap()
        .write_all(message(AgentKind::Codex, "user", "appendedneedle").as_bytes())
        .unwrap();
    let third = ScopeKey::new("workspace:after-append").unwrap();
    store
        .apply_session_changes_for_scope(&third, &[session], &[])
        .unwrap();
    let (_, errors, pending) = store
        .refresh_pending_session_search_for_scope(&third)
        .unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(!pending);
    assert_eq!(
        store
            .recall_sessions(&SessionRecallOptions {
                query: "appendedneedle".into(),
                ..options
            })
            .unwrap()
            .total,
        1
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn shared_scan_cache_preserves_source_bundles_and_provider_identity() {
    let (root, store, scope) = fixture();
    let session = save(
        &root,
        &store,
        &scope,
        "scan-cache",
        AgentKind::Codex,
        "/project",
        "2026-06-23T00:00:00Z",
        &message(AgentKind::Codex, "user", "cache"),
    );
    let cache = store.shared_session_scan_cache().unwrap();
    assert!(
        cache
            .session_if_current(AgentKind::Codex, &session.path)
            .is_some()
    );
    assert!(
        cache
            .session_if_current(AgentKind::Claude, &session.path)
            .is_none()
    );
    fs::write(
        &session.path,
        message(AgentKind::Codex, "user", "modified cache source"),
    )
    .unwrap();
    assert!(
        cache
            .session_if_current(AgentKind::Codex, &session.path)
            .is_none()
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
