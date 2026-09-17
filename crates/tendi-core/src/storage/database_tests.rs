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
