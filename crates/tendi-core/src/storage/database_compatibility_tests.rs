use super::*;
use std::{
    fs,
    process::{Child, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

const CHILD_DATABASE: &str = "TENDI_SQLITE_LOCK_TEST_DATABASE";
const CHILD_READY: &str = "TENDI_SQLITE_LOCK_TEST_READY";

fn old_version() -> String {
    format!(
        "SQLite 3.46.0; Tendi {}; schema {}",
        env!("CARGO_PKG_VERSION"),
        super::super::STORAGE_SCHEMA_VERSION
    )
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn holds_old_sqlite_version_in_child_process() {
    let Ok(database) = std::env::var(CHILD_DATABASE) else {
        return;
    };
    let ready = std::env::var(CHILD_READY).unwrap();
    let _lock = acquire_for_version(Path::new(&database), &old_version()).unwrap();
    fs::write(ready, b"ready").unwrap();
    std::thread::sleep(Duration::from_secs(10));
}

#[test]
fn rejects_different_sqlite_version_before_opening_database() {
    let directory = std::env::temp_dir().join(format!(
        "tendi-sqlite-version-lock-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    let database = directory.join("database.sqlite3");
    let ready = directory.join("child-ready");
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "storage::database_compatibility::tests::holds_old_sqlite_version_in_child_process",
        ])
        .env(CHILD_DATABASE, &database)
        .env(CHILD_READY, &ready)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let child = ChildGuard(child);
    let started = Instant::now();
    while !ready.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "child lock was not acquired"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let same_version = acquire_for_version(&database, &old_version()).unwrap();
    let error = match super::super::Store::open(&database) {
        Ok(_) => panic!("different SQLite version was allowed to open the database"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("SQLite 3.46.0"));
    assert!(!database.exists(), "SQLite opened before the version check");
    drop(same_version);
    drop(child);

    let writer = super::super::Store::open(&database).unwrap();
    assert!(database.exists());
    drop(writer);
    fs::remove_dir_all(directory).unwrap();
}
