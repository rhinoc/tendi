use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

const DATABASE_SUFFIXES: [&str; 4] = ["", "-wal", "-shm", "-journal"];

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn marker_path(path: &Path) -> PathBuf {
    sibling(path, ".reset-pending")
}

pub(super) fn storage_bytes(path: &Path) -> io::Result<u64> {
    let mut bytes = 0_u64;
    for suffix in DATABASE_SUFFIXES {
        match fs::metadata(sibling(path, suffix)) {
            Ok(metadata) => bytes = bytes.saturating_add(metadata.len()),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(bytes)
}

pub(super) fn schedule_reset(path: &Path) -> io::Result<()> {
    File::create(marker_path(path))?.sync_all()
}

pub(super) fn apply_pending_reset(path: &Path) -> io::Result<bool> {
    let marker = marker_path(path);
    match fs::metadata(&marker) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    }
    for suffix in DATABASE_SUFFIXES {
        match fs::remove_file(sibling(path, suffix)) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    fs::remove_file(marker)?;
    Ok(true)
}

#[cfg(test)]
#[path = "database_storage_tests.rs"]
mod tests;
