use super::*;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

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
