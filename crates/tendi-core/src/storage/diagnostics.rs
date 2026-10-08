use serde_json::{Value, json};
use std::{
    fs::{self, File, Metadata},
    io::Read,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

fn sidecar_path(database: &Path, suffix: &str) -> PathBuf {
    let mut path = database.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

fn file_state(path: &Path) -> Value {
    match fs::metadata(path) {
        Ok(metadata) => json!({
            "exists": true,
            "bytes": metadata.len(),
            "modifiedMs": metadata.modified().ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|elapsed| elapsed.as_millis()),
            "inode": inode(&metadata),
            "device": device(&metadata),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({"exists": false}),
        Err(error) => json!({"error": error.to_string()}),
    }
}

#[cfg(unix)]
fn inode(metadata: &Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.ino())
}

#[cfg(unix)]
fn device(metadata: &Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.dev())
}

#[cfg(not(unix))]
fn inode(_: &Metadata) -> Option<u64> {
    None
}

#[cfg(not(unix))]
fn device(_: &Metadata) -> Option<u64> {
    None
}

fn wal_state(path: &Path) -> Value {
    let mut state = file_state(path);
    if state.get("exists") != Some(&Value::Bool(true)) {
        return state;
    }
    let mut header = [0u8; 32];
    let read = File::open(path).and_then(|mut file| file.read_exact(&mut header));
    if let Value::Object(fields) = &mut state {
        match read {
            Ok(()) => {
                let field =
                    |start| u32::from_be_bytes(header[start..start + 4].try_into().unwrap());
                fields.insert("magic".into(), json!(format!("0x{:08x}", field(0))));
                fields.insert("formatVersion".into(), json!(field(4)));
                fields.insert("pageSize".into(), json!(field(8)));
                fields.insert("checkpointSequence".into(), json!(field(12)));
                fields.insert("salt1".into(), json!(field(16)));
                fields.insert("salt2".into(), json!(field(20)));
            }
            Err(error) => {
                fields.insert("headerError".into(), json!(error.to_string()));
            }
        }
    }
    state
}

pub fn database_file_diagnostics(database: &Path) -> Value {
    json!({
        "sqliteVersion": rusqlite::version(),
        "executable": std::env::current_exe().ok(),
        "databaseFile": file_state(database),
        "walFile": wal_state(&sidecar_path(database, "-wal")),
        "shmFile": file_state(&sidecar_path(database, "-shm")),
    })
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
