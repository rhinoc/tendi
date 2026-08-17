use std::{
    env, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// A filesystem-backed source projected into the Tendi SQLite store.
///
/// `source_kind` and `path` form the logical identity.  The path is stored as
/// the caller-provided stable string; callers should pass the same normalized
/// path representation for subsequent scans.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsManifestEntry {
    pub source_kind: String,
    pub path: PathBuf,
    pub root: PathBuf,
    pub agent: Option<String>,
    pub scope: Option<String>,
    pub mtime_ns: Option<i64>,
    pub size: Option<i64>,
    pub inode: Option<i64>,
    pub device: Option<i64>,
    pub sha256: Option<String>,
    pub parser_version: String,
    pub last_seen_at: i64,
    pub parse_status: String,
}

/// Return the stable workspace identity used by projection context rows.
///
/// Existing workspaces are canonicalized so symlinked paths do not create
/// separate projection contexts. A not-yet-existing path is kept absolute so
/// callers can still use the same identity during setup and tests.
pub fn canonical_workspace_root(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(path)
}
