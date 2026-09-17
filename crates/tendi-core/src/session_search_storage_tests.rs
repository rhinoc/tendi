use super::*;
use std::fs::OpenOptions;

fn fixture(name: &str) -> (PathBuf, Store, ScopeKey, SessionRecord) {
    let root = std::env::temp_dir().join(format!(
        "tendi-search-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let store = Store::open(root.join("test.sqlite3")).unwrap();
    let session = serde_json::from_value(serde_json::json!({
            "id": "search-test", "agent": "codex", "path": root.join("session.jsonl"), "title": "Search test"
        })).unwrap();
    (
        root,
        store,
        ScopeKey::new("workspace:test").unwrap(),
        session,
    )
}

fn message(text: &str) -> String {
    format!(
        "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":{}}}]}}}}\n",
        serde_json::to_string(text).unwrap()
    )
}

#[test]
fn per_session_work_survives_restart_without_visiting_clean_sessions() {
    use std::io::Write;
    let (root, store, scope, dirty) = fixture("dirty-keys");
    let mut clean = dirty.clone();
    clean.id = "clean-session".into();
    clean.path = root.join("clean.jsonl");
    fs::write(&dirty.path, message("original")).unwrap();
    fs::write(&clean.path, message("untouched")).unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[dirty.clone(), clean.clone()], &[])
        .unwrap();
    store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    // An unreadable clean input proves the worker does not enumerate/read
    // every canonical session when just one source has changed.
    fs::remove_file(&clean.path).unwrap();
    fs::create_dir(&clean.path).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&dirty.path)
        .unwrap()
        .write_all(message("appendneedle").as_bytes())
        .unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[dirty.clone()], &[])
        .unwrap();
    let keys: Vec<String> = store
        .conn
        .prepare("SELECT session_id FROM scoped_session_search_work")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(keys, [dirty.id]);
    drop(store);
    let store = Store::open(root.join("test.sqlite3")).unwrap();
    let (published, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert_eq!(published.len(), 1);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(!pending);
    assert_eq!(
        store
            .search_sessions_for_scope(&scope, "appendneedle", None)
            .unwrap()
            .len(),
        1
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn successful_old_generation_cannot_acknowledge_new_session_work() {
    use std::cell::Cell;
    let (root, store, scope, session) = fixture("generation");
    fs::write(&session.path, message("generationneedle")).unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
        .unwrap();
    let advanced = Cell::new(false);
    let (published, errors, pending) = store.refresh_pending_session_search_for_scope_until(&scope, || {
            let building: bool = store.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM scoped_session_search_index WHERE search_index_version = 0)", [], |row| row.get(0)
            ).unwrap();
            if building && !advanced.replace(true) {
                store.with_named_write_transaction("test.new_search_generation", |tx|
                    mark_session_key_pending_in_tx(tx, &scope, &session)).unwrap();
            }
            false
        }).unwrap();
    assert!(advanced.get());
    assert_eq!(published.len(), 1);
    assert!(errors.is_empty());
    assert!(pending);
    let (published, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert!(published.is_empty());
    assert!(errors.is_empty());
    assert!(!pending);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn durable_provisional_tail_is_replaced_not_duplicated() {
    use std::io::Write;
    let (root, store, scope, session) = fixture("durable-tail");
    let first = message("firsttailneedle");
    fs::write(&session.path, first.trim_end()).unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
        .unwrap();
    store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert_eq!(
        store
            .search_sessions_for_scope(&scope, "firsttailneedle", None)
            .unwrap()
            .len(),
        1
    );
    drop(store);
    fs::OpenOptions::new()
        .append(true)
        .open(&session.path)
        .unwrap()
        .write_all(("\n".to_owned() + &message("secondtailneedle") + "{\"type\":").as_bytes())
        .unwrap();
    let store = Store::open(root.join("test.sqlite3")).unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session], &[])
        .unwrap();
    let (_, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert!(errors.is_empty());
    assert!(!pending);
    let count: i64 = store
        .conn
        .query_row(
            "SELECT count(*) FROM scoped_session_search_records",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 3);
    let encoded: String = store
        .conn
        .query_row(
            "SELECT search_checkpoint FROM scoped_session_search_index",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let checkpoint: transcript::SearchCheckpoint = serde_json::from_str(&encoded).unwrap();
    assert_eq!(checkpoint.next_record_order, 3);
    assert_eq!(
        checkpoint.committed_offset,
        first.len() as u64 + message("secondtailneedle").len() as u64
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn deletion_tombstone_is_scoped_to_the_complete_session_identity() {
    let (root, store, scope, session) = fixture("scoped-delete");
    let other = ScopeKey::new("workspace:other-delete").unwrap();
    fs::write(&session.path, message("retainedscope")).unwrap();
    for scope in [&scope, &other] {
        store
            .apply_session_changes_for_scope(scope, &[session.clone()], &[])
            .unwrap();
        store
            .refresh_pending_session_search_for_scope(scope)
            .unwrap();
    }
    store
        .apply_session_changes_for_scope(&scope, &[], &[session.path.clone()])
        .unwrap();
    drop(store);
    let store = Store::open(root.join("test.sqlite3")).unwrap();
    let (_, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert!(errors.is_empty());
    assert!(!pending);
    assert!(
        store
            .search_sessions_for_scope(&scope, "retainedscope", None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .search_sessions_for_scope(&other, "retainedscope", None)
            .unwrap()
            .len(),
        1
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn same_size_replacement_with_preserved_mtime_rebuilds_the_index() {
    let (root, store, scope, session) = fixture("identity-reset");
    fs::write(&session.path, message("oldneedle")).unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
        .unwrap();
    store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    let modified = fs::metadata(&session.path).unwrap().modified().unwrap();
    let replacement = root.join("replacement.jsonl");
    fs::write(&replacement, message("newneedle")).unwrap();
    fs::File::options()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    fs::rename(&replacement, &session.path).unwrap();
    // The ordinary metadata/source owner must enqueue the replacement even
    // when its legacy mtime/size fingerprint and metadata are unchanged.
    store
        .apply_session_changes_for_scope(&scope, &[session], &[])
        .unwrap();
    let (published, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert_eq!(published.len(), 1);
    assert!(errors.is_empty());
    assert!(!pending);
    assert!(
        store
            .search_sessions_for_scope(&scope, "oldneedle", None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .search_sessions_for_scope(&scope, "newneedle", None)
            .unwrap()
            .len(),
        1
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn readded_identity_is_not_pruned_by_an_old_deletion_tombstone() {
    let (root, store, scope, session) = fixture("delete-readd");
    fs::write(&session.path, message("readdedneedle")).unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
        .unwrap();
    store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[], &[session.path.clone()])
        .unwrap();
    let readded = std::cell::Cell::new(false);
    let missing: bool = store
        .conn
        .query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM scoped_sessions)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(missing);
    // The worker has observed the tombstone and selected its prune owner.
    // Re-add at that owner's cancellation boundary before deletion SQL.
    store
        .prune_session_search_key(
            &scope,
            &session.id,
            agent_label(session.agent),
            &session.path.to_string_lossy(),
            &|| {
                if !readded.get() {
                    readded.set(true);
                    store
                        .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
                        .unwrap();
                }
                false
            },
        )
        .unwrap();
    assert!(readded.get());
    let (_, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert!(errors.is_empty());
    if pending {
        store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
    }
    assert_eq!(
        store
            .search_sessions_for_scope(&scope, "readdedneedle", None)
            .unwrap()
            .len(),
        1
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn metadata_commit_survives_index_failure_and_pending_work_recovers_after_restart() {
    let (root, store, scope, mut broken) = fixture("pending-recovery");
    let obstructing_file = root.join("not-a-directory");
    fs::write(&obstructing_file, "obstruction").unwrap();
    broken.path = obstructing_file.join("session.jsonl");
    let mut healthy = broken.clone();
    healthy.id = "healthy".into();
    healthy.path = root.join("healthy.jsonl");
    fs::write(&healthy.path, message("healthyneedle")).unwrap();

    let (committed, _) = store
        .apply_session_changes_for_scope(&scope, &[broken.clone(), healthy], &[])
        .unwrap();
    assert_eq!(committed.len(), 2);
    let committed_revision = store
        .projection_head(&scope, "sessions")
        .unwrap()
        .unwrap()
        .revision;
    assert_eq!(
        store.pending_session_search_scopes().unwrap(),
        vec![scope.clone()]
    );
    let (published, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].session.id, "healthy");
    assert_eq!(published[0].base_revision, committed_revision);
    assert_eq!(
        published[0].revision.value(),
        committed_revision.value() + 1
    );
    assert_eq!(errors.len(), 1);
    assert!(pending);
    let last_error: String = store
        .conn
        .query_row(
            "SELECT last_error FROM scoped_session_search_pending WHERE scope_key = ?1",
            [scope.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(last_error.contains(&broken.id));
    assert_eq!(
        store
            .list_sessions_for_scope(&scope)
            .unwrap()
            .sessions
            .len(),
        2
    );
    drop(store);

    fs::remove_file(&obstructing_file).unwrap();
    fs::create_dir(&obstructing_file).unwrap();
    fs::write(&broken.path, message("recoveredneedle")).unwrap();
    let store = Store::open(root.join("test.sqlite3")).unwrap();
    let (published, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].session.id, broken.id);
    assert!(errors.is_empty());
    assert!(!pending);
    assert!(store.pending_session_search_scopes().unwrap().is_empty());
    assert_eq!(
        store
            .search_sessions_for_scope(&scope, "recoveredneedle", None)
            .unwrap()
            .len(),
        1
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unchanged_metadata_and_source_do_not_schedule_another_index_pass() {
    let (root, store, scope, session) = fixture("unchanged");
    fs::write(&session.path, message("stablebody")).unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
        .unwrap();
    let (published, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert_eq!(published.len(), 1);
    assert!(errors.is_empty());
    assert!(!pending);
    let revision = store
        .projection_head(&scope, "sessions")
        .unwrap()
        .unwrap()
        .revision;
    store
        .apply_session_changes_for_scope(&scope, &[session], &[])
        .unwrap();
    assert!(store.pending_session_search_scopes().unwrap().is_empty());
    let (published, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert!(published.is_empty());
    assert!(errors.is_empty());
    assert!(!pending);
    assert_eq!(
        store
            .projection_head(&scope, "sessions")
            .unwrap()
            .unwrap()
            .revision,
        revision
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_bookkeeping_without_search_changes_does_not_advance_sessions_revision() {
    let (root, store, scope, session) = fixture("bookkeeping");
    let transcript = message("stablebody");
    fs::write(&session.path, &transcript).unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
        .unwrap();
    store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    let revision = store
        .projection_head(&scope, "sessions")
        .unwrap()
        .unwrap()
        .revision;
    fs::write(
        &session.path,
        transcript + "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\"}}\n",
    )
    .unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session], &[])
        .unwrap();
    assert_eq!(
        store.pending_session_search_scopes().unwrap(),
        vec![scope.clone()]
    );
    let (published, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert!(published.is_empty());
    assert!(errors.is_empty());
    assert!(!pending);
    assert_eq!(
        store
            .projection_head(&scope, "sessions")
            .unwrap()
            .unwrap()
            .revision,
        revision
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rolled_back_metadata_does_not_leave_a_pending_request() {
    let (root, store, scope, _session) = fixture("pending-rollback");
    let result: Result<()> = store.with_named_write_transaction("test.aborted_metadata", |tx| {
        mark_session_search_pending_in_tx(tx, &scope)?;
        bail!("abort metadata transaction")
    });
    assert!(result.is_err());
    assert!(store.pending_session_search_scopes().unwrap().is_empty());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn busy_scope_is_deferred_without_blocking_other_scopes() {
    let (root, store, scope, session) = fixture("scope-busy");
    fs::write(&session.path, message("busyneedle")).unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
        .unwrap();
    let other_scope = ScopeKey::new("workspace:other").unwrap();
    store
        .apply_session_changes_for_scope(&other_scope, &[session], &[])
        .unwrap();
    let lease = store.lock_session_search(&scope).unwrap();
    let (published, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert!(published.is_empty());
    assert!(errors.is_empty());
    assert!(pending);
    let (published, errors, pending) = store
        .refresh_pending_session_search_for_scope(&other_scope)
        .unwrap();
    assert_eq!(published.len(), 1);
    assert!(errors.is_empty());
    assert!(!pending);
    drop(lease);
    let (published, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert_eq!(published.len(), 1);
    assert!(errors.is_empty());
    assert!(!pending);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancellation_preserves_publications_and_pending_work_for_resume() {
    let (root, store, scope, first) = fixture("cancel-resume");
    let mut second = first.clone();
    second.id = "second-session".into();
    second.path = root.join("second.jsonl");
    fs::write(&first.path, message("firstneedle")).unwrap();
    fs::write(&second.path, message("secondneedle")).unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[first, second], &[])
        .unwrap();
    // Stop once a real publication has committed, not after a predetermined
    // number of callback invocations or a timing-dependent sleep.
    let (published, errors, pending) = store
        .refresh_pending_session_search_for_scope_until(&scope, || {
            store
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM scoped_session_search_index
                 WHERE scope_key = ?1 AND search_index_version = ?2)",
                    params![scope.as_str(), SESSION_SEARCH_INDEX_VERSION],
                    |row| row.get(0),
                )
                .unwrap()
        })
        .unwrap();
    assert_eq!(published.len(), 1);
    assert!(errors.is_empty());
    assert!(pending);
    let last_error: Option<String> = store
        .conn
        .query_row(
            "SELECT last_error FROM scoped_session_search_pending WHERE scope_key = ?1",
            [scope.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(last_error.is_none());
    let (remaining, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert_eq!(remaining.len(), 1);
    assert_ne!(remaining[0].session.id, published[0].session.id);
    assert!(errors.is_empty());
    assert!(!pending);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn deletion_is_committed_before_independent_index_cleanup() {
    let (root, store, scope, session) = fixture("deletion");
    fs::write(&session.path, message("deletedneedle")).unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
        .unwrap();
    store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[], &[session.path.clone()])
        .unwrap();
    assert!(
        store
            .list_sessions_for_scope(&scope)
            .unwrap()
            .sessions
            .is_empty()
    );
    assert!(
        store
            .search_sessions_for_scope(&scope, "deletedneedle", None)
            .unwrap()
            .is_empty()
    );
    let (_, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert!(errors.is_empty());
    assert!(!pending);
    let remaining: i64 = store
        .conn
        .query_row(
            "SELECT count(*) FROM scoped_session_search_records",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn append_retains_existing_fts_rows_even_when_session_metadata_is_unchanged() {
    let (root, store, scope, session) = fixture("append");
    fs::write(&session.path, message("originalneedle")).unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&scope)
        .unwrap();
    let original: i64 = store
        .conn
        .query_row(
            "SELECT id FROM scoped_session_search_records WHERE record_order = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    fs::write(
        &session.path,
        message("originalneedle") + &message("appendedneedle"),
    )
    .unwrap();
    let changed = store
        .apply_session_changes_for_scope(&scope, &[session], &[])
        .unwrap();
    // Metadata commits independently; the background publication has its own delta.
    assert!(changed.0.is_empty());
    let (indexed, errors, pending) = store
        .refresh_pending_session_search_for_scope(&scope)
        .unwrap();
    assert_eq!(indexed.len(), 1);
    assert!(errors.is_empty());
    assert!(!pending);
    let retained: i64 = store
        .conn
        .query_row(
            "SELECT id FROM scoped_session_search_records WHERE record_order = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(original, retained);
    assert_eq!(
        store
            .search_sessions_for_scope(&scope, "appendedneedle", None)
            .unwrap()
            .len(),
        1
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rewrite_and_truncation_remove_obsolete_search_results() {
    let (root, store, scope, session) = fixture("rewrite");
    fs::write(
        &session.path,
        message("oldfirstneedle") + &message("oldsecondneedle"),
    )
    .unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&scope)
        .unwrap();
    fs::write(&session.path, message("replacementneedle")).unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session], &[])
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&scope)
        .unwrap();
    assert!(
        store
            .search_sessions_for_scope(&scope, "oldsecondneedle", None)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .search_sessions_for_scope(&scope, "oldfirstneedle", None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .search_sessions_for_scope(&scope, "replacementneedle", None)
            .unwrap()
            .len(),
        1
    );
    let count: i64 = store
        .conn
        .query_row(
            "SELECT count(*) FROM scoped_session_search_records",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn transcript_search_finishes_at_the_initial_file_boundary() {
    use std::io::Write;

    let (root, store, _scope, session) = fixture("live-append");
    fs::write(&session.path, message("initialneedle")).unwrap();
    let mut items = Vec::new();
    transcript::for_each_search_item(&session.path, session.agent, |item| {
        items.push(item);
        // Simulate an active provider appending while the indexer is parsing.
        if items.len() == 1 {
            let mut file = OpenOptions::new().append(true).open(&session.path).unwrap();
            file.write_all(message("laterneedle").as_bytes()).unwrap();
        }
    })
    .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        transcript::parse_search_transcript(&session.path, session.agent)
            .unwrap()
            .items
            .len(),
        2
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn transcript_read_failure_is_an_error_instead_of_an_endless_warning_loop() {
    let (root, store, _scope, session) = fixture("read-error");
    fs::create_dir(&session.path).unwrap();
    assert!(transcript::for_each_search_item(&session.path, session.agent, |_| {}).is_err());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_revision_and_rows_share_a_read_snapshot_while_writers_commit() {
    let (root, store, scope, session) = fixture("read-snapshot");
    store
        .apply_session_delta_for_scope(&scope, &[session.clone()])
        .unwrap();
    let before = store
        .projection_head(&scope, "sessions")
        .unwrap()
        .unwrap()
        .revision;
    let (revision, scan) = store
        .read_session_revisioned(&scope, || {
            let writer = Store::open(root.join("test.sqlite3"))?;
            let mut updated = session.clone();
            updated.title = Some("Changed while reading".into());
            writer.apply_session_delta_for_scope(&scope, &[updated])?;
            store.list_sessions_for_scope(&scope)
        })
        .unwrap();
    assert_eq!(revision, before);
    assert_eq!(scan.sessions[0].title, session.title);
    let (after, scan) = store.session_snapshot_for_scope(&scope).unwrap();
    assert!(after.value() > before.value());
    assert_eq!(
        scan.sessions[0].title.as_deref(),
        Some("Changed while reading")
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unpublished_index_is_hidden_and_repaired_after_reopening() {
    let (root, store, scope, session) = fixture("recovery");
    fs::write(&session.path, message("recoverneedle 中文")).unwrap();
    store
        .apply_session_changes_for_scope(&scope, &[session], &[])
        .unwrap();
    store
        .ensure_scoped_session_search_for_scope(&scope)
        .unwrap();
    store
        .with_named_write_transaction("test.interrupt_search", |tx| {
            tx.execute(
                "UPDATE scoped_session_search_index SET search_index_version = 0",
                [],
            )?;
            mark_session_search_pending_in_tx(tx, &scope)
        })
        .unwrap();
    drop(store);
    let store = Store::open(root.join("test.sqlite3")).unwrap();
    for query in ["recoverneedle", "中文"] {
        assert!(
            store
                .search_sessions_for_scope(&scope, query, None)
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        store
            .ensure_scoped_session_search_for_scope(&scope)
            .unwrap()
    );
    for query in ["recoverneedle", "中文"] {
        assert_eq!(
            store
                .search_sessions_for_scope(&scope, query, None)
                .unwrap()
                .len(),
            1
        );
    }
    assert!(
        !store
            .ensure_scoped_session_search_for_scope(&scope)
            .unwrap()
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_reads_ignore_obsolete_full_snapshots() {
    let (root, store, scope, session) = fixture("snapshot");
    store
        .write_normalized_snapshot(
            &scope,
            "sessions",
            &SessionScan {
                sessions: vec![],
                warnings: vec![],
            },
        )
        .unwrap();
    store
        .apply_session_delta_for_scope(&scope, &[session])
        .unwrap();
    assert_eq!(
        store
            .list_sessions_for_scope(&scope)
            .unwrap()
            .sessions
            .len(),
        1
    );
    let json = store
        .normalized_snapshot_json_for_scope(&scope, "sessions")
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<SessionScan>(&json)
            .unwrap()
            .sessions
            .len(),
        1
    );
    let stored: String = store
        .conn
        .query_row(
            "SELECT payload_json FROM normalized_snapshots WHERE domain='sessions'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        serde_json::from_str::<SessionScan>(&stored)
            .unwrap()
            .sessions
            .is_empty()
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
