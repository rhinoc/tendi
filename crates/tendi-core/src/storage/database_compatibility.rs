//! Hold a database-wide SQLite engine version lock for the lifetime of a writer.
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const LOCK_TIMEOUT: Duration = Duration::from_secs(2);

fn sidecar_path(database: &Path, suffix: &str) -> PathBuf {
    let mut path = database.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

fn open_lock(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("open SQLite compatibility lock {}", path.display()))
}

fn wait_for_exclusive(file: &File, path: &Path) -> Result<()> {
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(fs::TryLockError::WouldBlock) => {
                ensure!(
                    started.elapsed() < LOCK_TIMEOUT,
                    "timed out acquiring SQLite compatibility gate {}",
                    path.display()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(fs::TryLockError::Error(error)) => {
                return Err(error).with_context(|| format!("lock {}", path.display()));
            }
        }
    }
}

pub(super) fn acquire(database: &Path) -> Result<File> {
    ensure!(
        rusqlite::version_number() >= 3_051_003,
        "SQLite {} lacks the WAL reset fix required to open {}",
        rusqlite::version(),
        database.display()
    );
    let version = format!(
        "SQLite {}; Tendi {}; schema {}",
        rusqlite::version(),
        env!("CARGO_PKG_VERSION"),
        super::STORAGE_SCHEMA_VERSION
    );
    acquire_for_version(database, &version)
}

fn acquire_for_version(database: &Path, requested: &str) -> Result<File> {
    let gate_path = sidecar_path(database, ".sqlite-version.gate");
    let gate = open_lock(&gate_path)?;
    wait_for_exclusive(&gate, &gate_path)?;

    let version_path = sidecar_path(database, ".sqlite-version.lock");
    let mut version_file = open_lock(&version_path)?;
    match version_file.try_lock() {
        Ok(()) => {
            version_file.set_len(0)?;
            version_file.seek(SeekFrom::Start(0))?;
            version_file.write_all(requested.as_bytes())?;
            version_file.sync_data()?;
            version_file.lock_shared().with_context(|| {
                format!(
                    "hold shared SQLite compatibility lock {}",
                    version_path.display()
                )
            })?;
        }
        Err(fs::TryLockError::WouldBlock) => {
            version_file.try_lock_shared().with_context(|| {
                format!(
                    "join shared SQLite compatibility lock {}",
                    version_path.display()
                )
            })?;
            let mut active = String::new();
            version_file.read_to_string(&mut active)?;
            if active != requested {
                crate::logging::global().error(
                    "database version conflict",
                    serde_json::json!({
                        "database": database,
                        "activeVersion": active,
                        "requestedVersion": requested,
                        "processId": std::process::id(),
                        "executable": std::env::current_exe().ok(),
                    }),
                );
                bail!(
                    "database {} is open with {}; this process uses {}. Close the other Tendi process before opening this database",
                    database.display(),
                    active,
                    requested
                );
            }
        }
        Err(fs::TryLockError::Error(error)) => {
            return Err(error)
                .with_context(|| format!("lock SQLite version file {}", version_path.display()));
        }
    }
    drop(gate);
    Ok(version_file)
}

#[cfg(test)]
#[path = "database_compatibility_tests.rs"]
mod tests;
