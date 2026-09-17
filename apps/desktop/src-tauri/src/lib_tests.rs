use super::log_export_files;
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn log_export_files_includes_active_and_valid_rotated_logs() {
    let directory = temp_directory();
    for name in [
        "tendi.log",
        "tendi.2026-08-17.log",
        "tendi.2026-08-18.1.log",
        "tendi.invalid.log",
        "other.log",
    ] {
        fs::write(directory.join(name), name).unwrap();
    }

    let files = log_export_files(&directory.join("tendi.log")).unwrap();
    let names = files
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "tendi.2026-08-17.log",
            "tendi.2026-08-18.1.log",
            "tendi.log",
        ]
    );
    fs::remove_dir_all(directory).unwrap();
}

fn temp_directory() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("tendi-log-export-test-{timestamp}"));
    fs::create_dir_all(&path).unwrap();
    path
}
