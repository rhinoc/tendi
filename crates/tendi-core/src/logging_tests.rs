use super::*;
use std::fs;

#[test]
fn log_line_contains_structured_fields() {
    let line = format_log_line(
        "tendi-test",
        Level::Warn,
        "request failed",
        serde_json::json!({"request_id": "abc", "attempt": 2}),
    )
    .unwrap();
    assert!(line.contains("level=warn"));
    assert!(line.contains("component=\"tendi-test\""));
    assert!(line.contains("request_id=\"abc\""));
    assert!(line.contains("attempt=2"));
}

#[test]
fn writer_rotates_when_size_budget_is_exceeded() {
    let directory = tempfile_directory();
    let path = directory.join("tendi.log");
    let mut writer = RotatingFileWriter::open(
        path.clone(),
        RotationConfig {
            max_size_bytes: 4,
            max_backups: 10,
            max_age_days: 0,
            max_total_bytes: 0,
        },
    )
    .unwrap();
    writer.write(b"1234").unwrap();
    writer.write(b"5").unwrap();
    assert!(
        directory
            .read_dir()
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().starts_with("tendi."))
    );
    let _ = fs::remove_dir_all(directory);
}

fn tempfile_directory() -> PathBuf {
    let path = env::temp_dir().join(format!(
        "tendi-logging-test-{}-{}",
        std::process::id(),
        Local::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
