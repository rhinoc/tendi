use super::*;
use crate::storage::Store;
use rusqlite::params;
use serde_json::json;

struct TestDir(std::path::PathBuf);
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn store() -> (TestDir, Store) {
    let path = std::env::temp_dir().join(format!(
        "tendi-shared-cache-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = Store::open(path.join("cache.sqlite3")).unwrap();
    (TestDir(path), store)
}

#[test]
fn scoped_values_share_content_and_release_only_the_last_reference() {
    let (_dir, store) = store();
    store
        .with_named_write_transaction("test.shared_cache", |tx| {
            for scope in ["workspace:/a", "workspace:/b"] {
                tx.execute(
                    "INSERT INTO scoped_sessions(scope_key,id,agent,path,data_json)
                VALUES(?1,'session','codex','/session.jsonl','{}')",
                    [scope],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let count = |sql: &str| {
        store
            .conn
            .query_row(sql, [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    assert_eq!(count("SELECT count(*) FROM cache_sessions"), 1);
    assert_eq!(
        count("SELECT count(*) FROM shared_cache_values WHERE kind='data_json'"),
        1
    );
    assert_eq!(
        count("SELECT ref_count FROM shared_cache_values WHERE kind='data_json'"),
        2
    );
    store
        .with_named_write_transaction("test.shared_cache_mutation", |tx| {
            assert_eq!(
                execute_changed(
                    tx,
                    "INSERT INTO scoped_sessions(scope_key,id,agent,path,data_json)
            VALUES('workspace:/a','session','codex','/session.jsonl','{}')",
                    []
                )?,
                0
            );
            tx.execute(
                "DELETE FROM scoped_sessions WHERE scope_key='workspace:/a'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        count("SELECT ref_count FROM shared_cache_values WHERE kind='data_json'"),
        1
    );
    store
        .with_named_write_transaction("test.shared_cache_delete", |tx| {
            tx.execute("DELETE FROM scoped_sessions", [])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(count("SELECT count(*) FROM shared_cache_values"), 0);
}

#[test]
fn compressed_analytics_share_source_data_and_keep_scoped_projects() {
    let (_dir, store) = store();
    let mut state =
        serde_json::to_value(crate::analytics::AnalyticsParserState::default()).unwrap();
    state["parserVersion"] = json!(12);
    state["seenToolIds"] = json!(["id-1", "id-2"]);
    let state = state.to_string();
    store.with_named_write_transaction("test.shared_analytics", |tx| {
        for scope in ["workspace:/a", "workspace:/b"] {
            let analytics = json!({"responses":[],"project":{"name":scope}}).to_string();
            tx.execute("INSERT INTO scoped_session_analytics(scope_key,session_id,agent,session_path,
                file_mtime,file_size,indexed_at,analytics_json,parser_state_json)
                VALUES(?1,'s','codex','/s.jsonl',1,100,'1',?2,?3)",
                params![scope, compress_analytics_json(&analytics)?, state])?;
            tx.execute("INSERT INTO scoped_session_analytics_overview(scope_key,session_id,agent,session_path,
                has_activity,overview_json) VALUES(?1,'s','codex','/s.jsonl',0,?2)",params![scope, analytics])?;
        }
        Ok(())
    }).unwrap();
    let payloads: i64 = store
        .conn
        .query_row("SELECT count(*) FROM shared_cache_values", [], |r| r.get(0))
        .unwrap();
    assert_eq!(payloads, 3);
    for scope in ["workspace:/a", "workspace:/b"] {
        let (encoded, restored_state, version, overview): (Vec<u8>, String, u32, String) = store.conn.query_row(
            "SELECT a.analytics_json,a.parser_state_json,a.parser_version,o.overview_json
             FROM scoped_session_analytics a JOIN scoped_session_analytics_overview o USING(scope_key,session_id,agent,session_path)
             WHERE scope_key=?1", [scope], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        let decoded: serde_json::Value =
            serde_json::from_str(&decompress_analytics_json(&encoded).unwrap()).unwrap();
        assert_eq!(decoded["project"]["name"], scope);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&overview).unwrap(),
            decoded
        );
        assert_eq!(restored_state, state);
        assert_eq!(version, 12);
    }
    let blob_count: i64 = store
        .conn
        .query_row(
            "SELECT count(*) FROM shared_cache_values WHERE typeof(value)='blob'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(blob_count, 2);
    let overview_count: i64 = store
        .conn
        .query_row(
            "SELECT count(*) FROM shared_cache_values
             WHERE kind='overview_json' AND typeof(value)='text'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(overview_count, 1);
}

#[test]
fn projection_upgrade_converts_compressed_overview_payloads() {
    let (_dir, store) = store();
    store
        .with_named_write_transaction("test.shared_overview_upgrade_seed", |tx| {
            tx.execute(
                "INSERT INTO scoped_session_analytics_overview(
                    scope_key,session_id,agent,session_path,has_activity,overview_json
                 ) VALUES('workspace:/a','s','codex','/s.jsonl',0,'{\"project\":{\"name\":\"old\"}}')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let path = store.path().to_owned();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    register_functions(&conn).unwrap();
    let overview_json: String = conn
        .query_row(
            "SELECT CAST(value AS TEXT) FROM shared_cache_values WHERE kind='overview_json'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE shared_cache_values SET value=?1 WHERE kind='overview_json'",
        [compress_analytics_json(&overview_json).unwrap()],
    )
    .unwrap();
    conn.pragma_update(None, "user_version", 5).unwrap();
    drop(conn);

    let store = Store::open(path).unwrap();
    let schema_version: i64 = store
        .conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(schema_version, 6);
    let value_type: String = store
        .conn
        .query_row(
            "SELECT typeof(value) FROM shared_cache_values WHERE kind='overview_json'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(value_type, "text");
    let restored: String = store
        .conn
        .query_row(
            "SELECT overview_json FROM scoped_session_analytics_overview",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&restored).unwrap(),
        json!({"project": {"name": "old"}})
    );
}

#[test]
fn skill_evidence_is_shared_and_duplicate_links_do_not_leak_values() {
    let (_dir, store) = store();
    store.with_named_write_transaction("test.shared_evidence", |tx| {
        for scope in ["workspace:/a", "workspace:/b"] {
            for skill in ["/skills/one", "/skills/two"] {
                tx.execute("INSERT INTO scoped_session_skill_links(scope_key,session_id,agent,session_path,
                    skill_name,skill_path,evidence_kind,evidence_text,confidence)
                    VALUES(?1,'s','claude','/s.jsonl','skill',?2,'Read','same evidence','observed')", params![scope, skill])?;
            }
        }
        tx.execute("INSERT INTO scoped_session_skill_links(scope_key,session_id,agent,session_path,
            skill_name,skill_path,evidence_kind,evidence_text,confidence)
            VALUES('workspace:/a','s','claude','/s.jsonl','skill','/skills/one','Read','ignored evidence','observed')", [])?;
        Ok(())
    }).unwrap();
    let (count, references): (i64, i64) = store
        .conn
        .query_row(
            "SELECT count(*),sum(ref_count) FROM shared_cache_values WHERE kind='evidence_text'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((count, references), (1, 4));
}

fn fingerprint(conn: &Connection, table: &str, fields: &str) -> (u64, Vec<u8>) {
    let mut query = conn
        .prepare(&format!(
            "SELECT rowid,{fields} FROM {table} ORDER BY rowid"
        ))
        .unwrap();
    let mut rows = query.query([]).unwrap();
    let mut digest = Sha256::new();
    let mut count = 0;
    while let Some(row) = rows.next().unwrap() {
        for index in 0..row.as_ref().column_count() {
            let name = row.as_ref().column_name(index).unwrap();
            let bytes = match row.get_ref(index).unwrap() {
                ValueRef::Null => vec![0],
                ValueRef::Integer(value) => value.to_le_bytes().to_vec(),
                ValueRef::Real(value) => value.to_le_bytes().to_vec(),
                ValueRef::Text(value) | ValueRef::Blob(value)
                    if matches!(name, "analytics_json" | "overview_json") =>
                {
                    let text =
                        if value.starts_with(crate::storage::ANALYTICS_JSON_ENCODING.as_bytes()) {
                            decompress_analytics_json(value).unwrap()
                        } else {
                            String::from_utf8(value.to_vec()).unwrap()
                        };
                    serde_json::from_str::<serde_json::Value>(&text)
                        .map(|v| v.to_string())
                        .unwrap_or(text)
                        .into_bytes()
                }
                ValueRef::Text(value) | ValueRef::Blob(value) => value.to_vec(),
            };
            digest.update(bytes.len().to_le_bytes());
            digest.update(bytes);
        }
        count += 1;
    }
    (count, digest.finalize().to_vec())
}

/// Validates a consistent backup of a populated database, never migrates the source.
#[test]
#[ignore = "set TENDI_STORAGE_PROBE_SOURCE and TENDI_STORAGE_PROBE_DEST to validate a real database backup"]
fn populated_database_migration_preserves_cache_and_measures_space() {
    let source_path = std::env::var("TENDI_STORAGE_PROBE_SOURCE").unwrap();
    let destination = std::env::var("TENDI_STORAGE_PROBE_DEST").unwrap();
    assert!(!std::path::Path::new(&destination).exists());
    let source =
        Connection::open_with_flags(source_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    register_functions(&source).unwrap();
    let mut copy = Connection::open(&destination).unwrap();
    register_functions(&copy).unwrap();
    rusqlite::backup::Backup::new(&source, &mut copy)
        .unwrap()
        .run_to_completion(1024, std::time::Duration::from_millis(10), None)
        .unwrap();
    drop(source);
    let before = std::fs::metadata(&destination).unwrap().len();
    let mut snapshots = Vec::new();
    for layout in LAYOUTS
        .iter()
        .filter(|l| !l.table.starts_with("scoped_session_skill"))
    {
        let fields = columns(&copy, layout.table)
            .unwrap()
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>()
            .join(",");
        let snapshot = fingerprint(&copy, layout.table, &fields);
        snapshots.push((layout.table, fields, snapshot));
    }
    drop(copy);
    let started = std::time::Instant::now();
    let store = Store::open(&destination).unwrap();
    eprintln!("migrationSeconds={:.2}", started.elapsed().as_secs_f64());
    for (table, fields, expected) in snapshots {
        assert_eq!(
            fingerprint(&store.conn, table, &fields),
            expected,
            "{table}"
        );
        eprintln!("preserved {table}: {} rows", expected.0);
    }
    store.run_pending_storage_migrations().unwrap();
    store.run_pending_storage_maintenance().unwrap();
    let after = std::fs::metadata(&destination).unwrap().len();
    eprintln!(
        "beforeBytes={before} afterBytes={after} savedBytes={}",
        before - after
    );
    assert!(after < before);
    let check: String = store
        .conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(check, "ok");
    let orphan_count: i64 = store
        .conn
        .query_row(
            "SELECT count(*) FROM shared_cache_values WHERE ref_count=0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphan_count, 0);
}

#[test]
fn empty_metadata_has_no_fts_document_and_content_lives_until_last_scope_deletes_it() {
    let (_dir, store) = store();
    store.with_named_write_transaction("test.shared_search", |tx| {
        tx.execute("INSERT INTO session_search_content_records(content_hash,user_text,assistant_text)
            VALUES('hash','shared message','')", [])?;
        for scope in ["workspace:/a", "workspace:/b"] {
            tx.execute("INSERT INTO scoped_session_search_entries(scope_key,session_id,agent,session_path,record_order,content_id)
                VALUES(?1,'s','codex','/s.jsonl',0,1)", [scope])?;
        }
        Ok(())
    }).unwrap();
    let count = |sql: &str| {
        store
            .conn
            .query_row(sql, [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    assert_eq!(
        count("SELECT count(*) FROM scoped_session_search_metadata_fts"),
        0
    );
    store.with_named_write_transaction("test.search_metadata_update", |tx| {
        tx.execute("UPDATE scoped_session_search_entries SET title='searchable title' WHERE scope_key='workspace:/a'", [])?;
        Ok(())
    }).unwrap();
    assert_eq!(
        count(
            "SELECT count(*) FROM scoped_session_search_metadata_fts WHERE scoped_session_search_metadata_fts MATCH 'sea'"
        ),
        1
    );
    store
        .with_named_write_transaction("test.search_scope_delete", |tx| {
            tx.execute(
                "DELETE FROM scoped_session_search_entries WHERE scope_key='workspace:/a'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        count("SELECT count(*) FROM scoped_session_search_metadata_fts"),
        0
    );
    assert_eq!(
        count("SELECT count(*) FROM session_search_content_records"),
        1
    );
    store
        .with_named_write_transaction("test.search_final_delete", |tx| {
            tx.execute("DELETE FROM scoped_session_search_entries", [])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        count("SELECT count(*) FROM session_search_content_records"),
        0
    );
    assert_eq!(count("SELECT count(*) FROM session_search_content_fts"), 0);
}

#[test]
fn skill_cache_rebuild_keeps_other_payloads_and_releases_evidence() {
    let (_dir, store) = store();
    store.with_named_write_transaction("test.rebuild_seed", |tx| {
        tx.execute("INSERT INTO scoped_sessions(scope_key,id,agent,path,data_json)
            VALUES('workspace:/a','s','codex','/s.jsonl','{}')", [])?;
        tx.execute("INSERT INTO scoped_session_skill_links(scope_key,session_id,agent,session_path,
            skill_name,skill_path,evidence_kind,evidence_text,confidence)
            VALUES('workspace:/a','s','codex','/s.jsonl','skill','/skill','Read','evidence','observed')", [])?;
        Ok(())
    }).unwrap();
    let conn = Connection::open(store.path()).unwrap();
    register_functions(&conn).unwrap();
    rebuild_skill_links(&conn).unwrap();
    let session_count: i64 = store
        .conn
        .query_row("SELECT count(*) FROM scoped_sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(session_count, 1);
    let values: i64 = store
        .conn
        .query_row("SELECT count(*) FROM shared_cache_values", [], |r| r.get(0))
        .unwrap();
    assert_eq!(values, 1);
    assert_eq!(
        store
            .conn
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

#[test]
fn search_upsert_preserves_record_identity() {
    let (_dir, store) = store();
    store.with_named_write_transaction("test.search_identity", |tx| {
        tx.execute("INSERT INTO session_search_content_records(content_hash,user_text,assistant_text)
            VALUES('hash','message','')", [])?;
        let sql = "INSERT INTO scoped_session_search_entries(scope_key,session_id,agent,session_path,record_order,title,content_id)
            VALUES('workspace:/a','s','codex','/s.jsonl',0,?1,1)";
        tx.execute(sql, ["first"])?;
        let before: i64 = tx.query_row("SELECT id FROM scoped_session_search_entries", [], |r|r.get(0))?;
        tx.execute(sql, ["updated"])?;
        let after: i64 = tx.query_row("SELECT id FROM scoped_session_search_entries", [], |r|r.get(0))?;
        assert_eq!(before, after);
        Ok(())
    }).unwrap();
}

#[test]
fn projection_upgrade_preserves_payloads_and_refreshes_write_definitions() {
    let (_dir, store) = store();
    store
        .with_named_write_transaction("test.projection_upgrade_seed", |tx| {
            tx.execute(
                "INSERT INTO scoped_sessions(scope_key,id,agent,path,data_json)
            VALUES('workspace:/a','s','codex','/s.jsonl','{}')",
                [],
            )?;
            tx.pragma_update(None, "user_version", 4)?;
            Ok(())
        })
        .unwrap();
    let path = store.path().to_owned();
    drop(store);
    let store = Store::open(path).unwrap();
    assert_eq!(
        store
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        crate::storage::STORAGE_SCHEMA_VERSION
    );
    assert_eq!(
        store
            .conn
            .query_row("SELECT data_json FROM scoped_sessions", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "{}"
    );
    assert_eq!(
        store
            .conn
            .query_row("SELECT ref_count FROM shared_cache_values", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let mut statement = store
        .conn
        .prepare("SELECT analytics_text FROM scoped_session_analytics")
        .unwrap();
    assert!(!statement.exists([]).unwrap());
}
