use super::*;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn bundled_sqlite_includes_wal_reset_fix() {
    assert!(
        rusqlite::version_number() >= 3_051_003,
        "SQLite {} lacks the WAL reset fix",
        rusqlite::version()
    );
}

#[test]
fn recovery_reopens_writer_connection_without_replaying_work() {
    let path = std::env::temp_dir().join(format!(
        "tendi-database-recovery-{}-{}.sqlite3",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let writer = DatabaseWriter::open(&path).unwrap();
    writer
        .write("recovery.setup", |tx| {
            tx.execute_batch("CREATE TABLE recovery_probe (value TEXT NOT NULL)")?;
            Ok(())
        })
        .unwrap();

    writer.recover().unwrap();
    writer
        .write("recovery.write", |tx| {
            tx.execute("INSERT INTO recovery_probe (value) VALUES ('ok')", [])?;
            Ok(())
        })
        .unwrap();
    drop(writer);

    let connection = Connection::open(&path).unwrap();
    let value: String = connection
        .query_row("SELECT value FROM recovery_probe", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, "ok");

    for suffix in ["", "-wal", "-shm"] {
        let mut target = path.as_os_str().to_os_string();
        target.push(suffix);
        let _ = fs::remove_file(target);
    }
}

#[test]
fn rebuilding_corrupt_session_skill_links_preserves_session_rows() {
    let path = std::env::temp_dir().join(format!(
        "tendi-database-skill-links-rebuild-{}-{}.sqlite3",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let writer = DatabaseWriter::open(&path).unwrap();
    writer
        .write("rebuild.setup", |tx| {
            tx.execute_batch(
                "CREATE TABLE scoped_sessions (id TEXT PRIMARY KEY);
                 INSERT INTO scoped_sessions (id) VALUES ('session-1');
                 CREATE TABLE scoped_session_skill_index (
                     scope_key TEXT NOT NULL,
                     session_id TEXT NOT NULL
                 );
                 INSERT INTO scoped_session_skill_index (scope_key, session_id)
                     VALUES ('scope-1', 'session-1');
                 CREATE TABLE scoped_session_skill_links (
                     scope_key TEXT NOT NULL,
                     session_id TEXT NOT NULL,
                     skill_path TEXT NOT NULL,
                     PRIMARY KEY (scope_key, session_id, skill_path)
                 );
                 INSERT INTO scoped_session_skill_links (
                     scope_key, session_id, skill_path
                 ) VALUES ('scope-1', 'session-1', '/skill');",
            )?;
            Ok(())
        })
        .unwrap();

    let connection = Connection::open(&path).unwrap();
    crate::migrations::rebuild_corrupt_session_skill_links(&connection).unwrap();
    DatabaseWriter::probe_connection(&connection, false).unwrap();
    let session_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM scoped_sessions", [], |row| row.get(0))
        .unwrap();
    let index_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM scoped_session_skill_index",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let link_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM scoped_session_skill_links",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(session_count, 1);
    assert_eq!(index_count, 0);
    assert_eq!(link_count, 0);
    drop(connection);
    drop(writer);

    for suffix in ["", "-wal", "-shm"] {
        let mut target = path.as_os_str().to_os_string();
        target.push(suffix);
        let _ = fs::remove_file(target);
    }
}

#[test]
fn rebuilding_corrupt_fs_manifest_preserves_data_and_queues_full_refreshes() {
    let directory = std::env::temp_dir().join(format!(
        "tendi-fs-manifest-recovery-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("database.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    connection
        .execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE fs_manifest (
                 scope_key TEXT NOT NULL DEFAULT 'installation:default',
                 source_kind TEXT NOT NULL,
                 path TEXT NOT NULL,
                 root TEXT NOT NULL,
                 agent TEXT,
                 scope TEXT,
                 mtime_ns INTEGER,
                 size INTEGER,
                 inode INTEGER,
                 device INTEGER,
                 sha256 TEXT,
                 parser_version TEXT NOT NULL,
                 last_seen_at INTEGER NOT NULL,
                 parse_status TEXT NOT NULL,
                 resource_path TEXT,
                 PRIMARY KEY (scope_key, source_kind, path)
             );
             CREATE INDEX idx_fs_manifest_root_kind_path
                 ON fs_manifest(scope_key, root, source_kind, path);
             CREATE INDEX idx_fs_manifest_resource_scope
                 ON fs_manifest(source_kind, resource_path, scope_key);
             CREATE TABLE scoped_projection_contexts (
                 scope_key TEXT NOT NULL,
                 domain TEXT NOT NULL,
                 state TEXT NOT NULL,
                 scanned_at INTEGER,
                 error TEXT,
                 parser_version TEXT NOT NULL,
                 PRIMARY KEY (scope_key, domain)
             );
             CREATE TABLE projection_heads (
                 scope_key TEXT NOT NULL,
                 domain TEXT NOT NULL,
                 revision INTEGER NOT NULL,
                 source_version TEXT,
                 schema_version INTEGER NOT NULL DEFAULT 1,
                 status TEXT NOT NULL,
                 updated_at INTEGER NOT NULL,
                 PRIMARY KEY (scope_key, domain)
             );
             CREATE TABLE projection_dirty_resources (
                 scope_key TEXT NOT NULL,
                 domain TEXT NOT NULL,
                 resource_key TEXT NOT NULL,
                 generation INTEGER NOT NULL,
                 reconcile_generation INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (scope_key, domain, resource_key)
             );
             CREATE TABLE preserved_rows (value TEXT NOT NULL);
             INSERT INTO meta (key, value) VALUES ('keep', 'yes');
             INSERT INTO fs_manifest (
                 scope_key, source_kind, path, root, parser_version, last_seen_at, parse_status
             ) VALUES (
                 'workspace:/workspace', 'rule', '/workspace/rule.md', '/workspace',
                 'scan-v8', 1, 'ready'
             );
             INSERT INTO scoped_projection_contexts (
                 scope_key, domain, state, scanned_at, parser_version
             ) VALUES ('workspace:/workspace', 'rules', 'ready', 1, 'scan-v8');
             INSERT INTO projection_heads (
                 scope_key, domain, revision, source_version, status, updated_at
             ) VALUES ('workspace:/workspace', 'rules', 7, 'source-v1', 'ready', 1);
             INSERT INTO preserved_rows (value) VALUES ('keep');",
        )
        .unwrap();
    let root_page: i64 = connection
        .query_row(
            "SELECT rootpage FROM sqlite_master WHERE name = 'fs_manifest'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let error = anyhow::anyhow!(
        "database health probe quick_check returned *** in database main ***\nTree {root_page} page {root_page}: btreeInitPage() returns error code 11"
    );

    assert!(DatabaseWriter::repairs_fs_manifest(&connection, &path, &error).unwrap());
    DatabaseWriter::probe_connection(&connection, false).unwrap();

    let manifest_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM fs_manifest", [], |row| row.get(0))
        .unwrap();
    let preserved: String = connection
        .query_row("SELECT value FROM preserved_rows", [], |row| row.get(0))
        .unwrap();
    let (state, revision, status): (String, i64, String) = connection
        .query_row(
            "SELECT c.state, h.revision, h.status
             FROM scoped_projection_contexts c
             JOIN projection_heads h USING (scope_key, domain)
             WHERE c.scope_key = 'workspace:/workspace' AND c.domain = 'rules'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let (resource_key, generation): (String, i64) = connection
        .query_row(
            "SELECT resource_key, generation FROM projection_dirty_resources
             WHERE scope_key = 'workspace:/workspace' AND domain = 'rules'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let backup_exists = fs::read_dir(&directory)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("database.sqlite3.corrupt-backup-")
        });

    assert_eq!(manifest_count, 0);
    assert_eq!(preserved, "keep");
    assert_eq!(state, "stale");
    assert_eq!(revision, 8);
    assert_eq!(status, "stale");
    assert_eq!(resource_key, "");
    assert_eq!(generation, 8);
    assert!(backup_exists);
    let marker_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM meta WHERE key = 'storage.fs_manifest_rebuild_pending'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(marker_count, 0);

    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}
