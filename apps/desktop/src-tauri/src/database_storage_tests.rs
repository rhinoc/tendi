use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn reset_removes_database_files_before_startup_and_keeps_other_data() {
    let directory = std::env::temp_dir().join(format!(
        "tendi-database-reset-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    let database = directory.join("tendi.sqlite3");
    for suffix in DATABASE_SUFFIXES {
        fs::write(sibling(&database, suffix), b"database").unwrap();
    }
    let logs = directory.join("tendi.log");
    fs::write(&logs, b"log").unwrap();

    assert_eq!(storage_bytes(&database).unwrap(), 32);
    schedule_reset(&database).unwrap();
    assert!(apply_pending_reset(&database).unwrap());
    assert!(!apply_pending_reset(&database).unwrap());
    assert_eq!(storage_bytes(&database).unwrap(), 0);
    let store = tendi_core::storage::Store::open(&database).unwrap();
    assert!(store.list_projects().unwrap().is_empty());
    drop(store);
    assert!(storage_bytes(&database).unwrap() > 0);
    assert_eq!(fs::read(logs).unwrap(), b"log");

    fs::remove_dir_all(directory).unwrap();
}
