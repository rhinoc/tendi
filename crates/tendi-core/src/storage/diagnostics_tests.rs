use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn reports_database_and_wal_state_without_opening_sqlite() {
    let path = std::env::temp_dir().join(format!(
        "tendi-diagnostics-{}-{}.sqlite3",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&path, b"database").unwrap();
    let wal = sidecar_path(&path, "-wal");
    let mut header = [0u8; 32];
    header[0..4].copy_from_slice(&0x377f0682_u32.to_be_bytes());
    header[4..8].copy_from_slice(&3_007_000_u32.to_be_bytes());
    header[8..12].copy_from_slice(&4096_u32.to_be_bytes());
    header[12..16].copy_from_slice(&7_u32.to_be_bytes());
    header[16..20].copy_from_slice(&11_u32.to_be_bytes());
    header[20..24].copy_from_slice(&13_u32.to_be_bytes());
    fs::write(&wal, header).unwrap();

    let snapshot = database_file_diagnostics(&path);
    assert_eq!(snapshot["databaseFile"]["bytes"], 8);
    assert_eq!(snapshot["walFile"]["magic"], "0x377f0682");
    assert_eq!(snapshot["walFile"]["checkpointSequence"], 7);
    assert_eq!(snapshot["walFile"]["salt1"], 11);
    assert_eq!(snapshot["walFile"]["salt2"], 13);
    assert_eq!(snapshot["shmFile"]["exists"], false);
    assert_eq!(snapshot["sqliteVersion"], rusqlite::version());

    fs::remove_file(wal).unwrap();
    fs::remove_file(path).unwrap();
}
