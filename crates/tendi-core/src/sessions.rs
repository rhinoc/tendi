use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Local};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

use crate::skills::AgentKind;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SessionTokenUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: String,
    pub agent: AgentKind,
    pub title: Option<String>,
    pub project: Option<PathBuf>,
    /// Main Git checkout for grouping; `project` remains the session's actual workspace/worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<PathBuf>,
    /// Repository remote captured by the session or the live checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_url: Option<String>,
    /// Stable logical project resolved by storage aliases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_project_name: Option<String>,
    pub path: PathBuf,
    pub started_at: Option<String>,
    pub updated_at: Option<String>,
    pub message_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_user_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_user_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_assistant_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_run_everything: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<SessionTokenUsage>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SessionIdentity {
    pub id: String,
    pub agent: AgentKind,
    pub path: PathBuf,
}

impl From<&SessionRecord> for SessionIdentity {
    fn from(session: &SessionRecord) -> Self {
        Self {
            id: session.id.clone(),
            agent: session.agent,
            path: session.path.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionScan {
    pub sessions: Vec<SessionRecord>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SessionScanCacheEntry {
    pub session: SessionRecord,
    pub file_mtime: i64,
    pub file_size: i64,
}

#[derive(Debug, Clone, Default)]
pub struct SessionScanCache {
    entries: BTreeMap<(AgentKind, PathBuf), SessionScanCacheEntry>,
    #[cfg(unix)]
    entries_by_file: BTreeMap<(AgentKind, SessionFileIdentity), PathBuf>,
}

impl SessionScanCache {
    pub fn from_entries(entries: impl IntoIterator<Item = SessionScanCacheEntry>) -> Self {
        let mut cache = Self::default();
        for entry in entries {
            #[cfg(unix)]
            if let Some(identity) = session_file_identity(&entry.session.path) {
                cache
                    .entries_by_file
                    .insert((entry.session.agent, identity), entry.session.path.clone());
            }
            cache
                .entries
                .insert((entry.session.agent, entry.session.path.clone()), entry);
        }
        cache
    }

    fn session_if_current(&self, agent: AgentKind, path: &Path) -> Option<SessionRecord> {
        let entry = self.entry_for_path(agent, path)?;
        let (file_mtime, file_size) = file_state(path)?;
        (entry.file_mtime == file_mtime && entry.file_size == file_size)
            .then(|| entry.session.clone())
    }

    fn session_if_current_id(&self, agent: AgentKind, id: &str) -> Option<SessionRecord> {
        self.entries.values().find_map(|entry| {
            if entry.session.agent != agent || entry.session.id != id {
                return None;
            }
            let (file_mtime, file_size) = file_state(&entry.session.path)?;
            (entry.file_mtime == file_mtime && entry.file_size == file_size)
                .then(|| entry.session.clone())
        })
    }

    fn session_if_appended(&self, agent: AgentKind, path: &Path) -> Option<(SessionRecord, u64)> {
        let entry = self.entry_for_path(agent, path)?;
        let (file_mtime, file_size) = file_state(path)?;
        let cached_size = u64::try_from(entry.file_size).ok()?;
        let current_size = u64::try_from(file_size).ok()?;
        if current_size <= cached_size
            || file_mtime < entry.file_mtime
            || !is_line_boundary(path, cached_size)
        {
            return None;
        }
        Some((entry.session.clone(), cached_size))
    }

    fn entry_for_path(&self, agent: AgentKind, path: &Path) -> Option<&SessionScanCacheEntry> {
        if let Some(entry) = self.entries.get(&(agent, path.to_path_buf())) {
            return Some(entry);
        }
        #[cfg(unix)]
        if let Some(identity) = session_file_identity(path) {
            let cached_path = self.entries_by_file.get(&(agent, identity))?;
            return self.entries.get(&(agent, cached_path.clone()));
        }
        None
    }

    pub fn changed_sessions(&self, sessions: &[SessionRecord]) -> Vec<SessionRecord> {
        sessions
            .iter()
            .filter(|session| {
                self.entries
                    .get(&(session.agent, session.path.clone()))
                    .is_none_or(|entry| entry.session != **session)
            })
            .cloned()
            .collect()
    }

    fn agent_for_path(&self, path: &Path) -> Option<AgentKind> {
        let direct = self
            .entries
            .iter()
            .find_map(|((agent, cached_path), _)| (cached_path == path).then_some(*agent));
        if direct.is_some() {
            return direct;
        }
        #[cfg(unix)]
        if let Some(identity) = session_file_identity(path) {
            return self
                .entries_by_file
                .keys()
                .find_map(|(agent, cached_identity)| {
                    (*cached_identity == identity).then_some(*agent)
                });
        }
        None
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SessionFileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
fn session_file_identity(path: &Path) -> Option<SessionFileIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::metadata(path).ok()?;
    Some(SessionFileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

pub fn scan_sessions(cwd: &Path) -> Result<SessionScan> {
    scan_sessions_with_additional_roots(cwd, &[])
}

pub fn scan_sessions_with_additional_roots(
    cwd: &Path,
    additional_session_roots: &[PathBuf],
) -> Result<SessionScan> {
    scan_sessions_with_additional_roots_with_cache(cwd, additional_session_roots, None)
}

pub fn scan_sessions_with_additional_roots_cached(
    cwd: &Path,
    additional_session_roots: &[PathBuf],
    cache: &SessionScanCache,
) -> Result<SessionScan> {
    scan_sessions_with_additional_roots_with_cache(cwd, additional_session_roots, Some(cache))
}

fn scan_sessions_with_additional_roots_with_cache(
    cwd: &Path,
    additional_session_roots: &[PathBuf],
    cache: Option<&SessionScanCache>,
) -> Result<SessionScan> {
    let mut sessions = Vec::new();
    let mut warnings = Vec::new();

    let ctx = crate::providers::ProviderContext::new(cwd);
    for provider in crate::providers::agent_providers() {
        if let Err(err) = provider.scan_sessions(&ctx, &mut sessions, &mut warnings, cache) {
            warnings.push(format!("{:?}: {err:#}", provider.kind()));
        }
    }
    scan_additional_session_roots(additional_session_roots, &mut sessions, cache);

    let mut sessions = merge_sessions(sessions);
    sessions.retain(|session| !is_index_path(&session.path));
    normalize_session_projects(&mut sessions);
    enrich_session_repositories(&mut sessions);
    sessions.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    Ok(SessionScan { sessions, warnings })
}

pub fn session_watch_roots(cwd: &Path, additional_session_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = crate::providers::session_roots(cwd);
    roots.extend(additional_session_roots.iter().cloned());
    roots.sort();
    roots.dedup();
    roots
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWatchTarget {
    pub path: PathBuf,
    pub recursive: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionWatchPlan {
    pub targets: Vec<SessionWatchTarget>,
    pub tutti_run_roots: Vec<PathBuf>,
}

pub fn session_watch_plan(cwd: &Path, additional_session_roots: &[PathBuf]) -> SessionWatchPlan {
    let mut targets = Vec::new();
    let mut tutti_run_roots = Vec::new();
    for root in session_watch_roots(cwd, additional_session_roots) {
        if is_tutti_run_root(&root) {
            tutti_run_roots.push(root.clone());
            targets.push(SessionWatchTarget {
                path: root.clone(),
                recursive: false,
            });
            targets.extend(tutti_run_session_roots(&root).into_iter().map(|path| {
                SessionWatchTarget {
                    path,
                    recursive: true,
                }
            }));
        } else {
            targets.push(SessionWatchTarget {
                recursive: root.is_dir(),
                path: root,
            });
        }
    }
    targets.sort_by(|left, right| left.path.cmp(&right.path));
    targets.dedup_by(|left, right| left.path == right.path);
    tutti_run_roots.sort();
    tutti_run_roots.dedup();
    SessionWatchPlan {
        targets,
        tutti_run_roots,
    }
}

pub fn recent_session_paths(
    cwd: &Path,
    additional_session_roots: &[PathBuf],
    since_unix_seconds: Option<u64>,
) -> Vec<PathBuf> {
    let mut candidates = BTreeMap::<PathBuf, SystemTime>::new();
    for root in session_watch_roots(cwd, additional_session_roots) {
        for (path, modified) in recent_session_paths_in_root_with_time(&root, since_unix_seconds) {
            candidates.insert(path, modified);
        }
    }
    sorted_recent_paths(candidates)
}

pub fn recent_session_paths_in_root(root: &Path, since_unix_seconds: Option<u64>) -> Vec<PathBuf> {
    sorted_recent_paths(
        recent_session_paths_in_root_with_time(root, since_unix_seconds)
            .into_iter()
            .collect(),
    )
}

fn recent_session_paths_in_root_with_time(
    root: &Path,
    since_unix_seconds: Option<u64>,
) -> Vec<(PathBuf, SystemTime)> {
    let since_seconds = since_unix_seconds.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(24 * 60 * 60)
    });
    let since = UNIX_EPOCH + Duration::from_secs(since_seconds);
    let since_local = DateTime::<Local>::from(since);
    let since_date = (since_local.year(), since_local.month(), since_local.day());
    let mut candidates = BTreeMap::<PathBuf, SystemTime>::new();

    if is_tutti_run_root(root) {
        let mut paths = BTreeSet::new();
        collect_tutti_run_session_paths(root, &mut paths);
        for path in paths {
            collect_recent_candidate(&path, since, &mut candidates);
        }
    } else if root.file_name().and_then(|name| name.to_str()) == Some("sessions")
        && root.parent().is_some_and(|parent| {
            parent.file_name().and_then(|name| name.to_str()) == Some(".codex")
        })
    {
        collect_recent_codex_paths(root, since_date, since, &mut candidates);
    } else {
        collect_recent_paths(root, since, &mut candidates);
    }

    candidates.into_iter().collect()
}

fn sorted_recent_paths(candidates: BTreeMap<PathBuf, SystemTime>) -> Vec<PathBuf> {
    let mut candidates = candidates.into_iter().collect::<Vec<_>>();
    candidates.sort_by(|(left_path, left_time), (right_path, right_time)| {
        right_time
            .cmp(left_time)
            .then_with(|| left_path.cmp(right_path))
    });
    candidates.into_iter().map(|(path, _)| path).collect()
}

pub fn is_session_candidate_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "jsonl")
        || matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("meta.json" | "store.db")
        )
}

pub fn scan_session_paths(paths: &[PathBuf], cache: &SessionScanCache) -> SessionScan {
    let mut sessions = Vec::new();
    let warnings = Vec::new();
    for path in paths.iter().filter(|path| path.is_file()) {
        match path.file_name().and_then(|name| name.to_str()) {
            Some("meta.json") if is_cursor_meta_file(path) => {
                scan_cursor_meta_file(path, &mut sessions, AgentKind::Cursor, Some(cache));
            }
            Some("store.db") => {
                scan_cursor_store_file(path, &mut sessions, AgentKind::Cursor, Some(cache));
            }
            _ if path
                .extension()
                .is_some_and(|extension| extension == "jsonl") =>
            {
                scan_detected_jsonl_session(path, &mut sessions, Some(cache));
            }
            _ => {}
        }
    }
    let mut sessions = merge_sessions(sessions);
    normalize_session_projects(&mut sessions);
    enrich_session_repositories(&mut sessions);
    sessions.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    SessionScan { sessions, warnings }
}

pub fn enrich_session_repositories(sessions: &mut [SessionRecord]) {
    let mut resolver = SessionRepositoryResolver::default();
    for session in sessions {
        let Some(project) = session
            .project
            .as_ref()
            .filter(|path| path.is_absolute() && path.is_dir())
        else {
            continue;
        };
        let (repository, repository_url) = resolver.resolve(project);
        session.repository = repository;
        if session.repository_url.is_none() {
            session.repository_url = repository_url;
        }
    }
}

pub fn normalize_session_projects(sessions: &mut [SessionRecord]) {
    for session in sessions {
        let Some(project) = session.project.as_ref() else {
            continue;
        };
        let Some(root) = ephemeral_chat_root(project) else {
            continue;
        };
        session.project = Some(root);
    }
}

fn ephemeral_chat_root(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let parent_name = parent.file_name()?.to_str()?;
    let root = parent.parent()?;

    if parent_name == "tutti"
        && root.file_name()?.to_str()? == "Documents"
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.strip_prefix("session-").is_some_and(is_uuid))
    {
        return Some(parent.to_path_buf());
    }

    if root.file_name()?.to_str()? == "Codex"
        && root.parent()?.file_name()?.to_str()? == "Documents"
        && is_date_directory(parent_name)
        && !path.file_name()?.to_str()?.is_empty()
    {
        return Some(root.to_path_buf());
    }

    None
}

fn is_date_directory(value: &str) -> bool {
    value.len() == 10
        && value.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

fn is_uuid(value: &str) -> bool {
    let lengths = [8, 4, 4, 4, 12];
    let mut parts = value.split('-');
    lengths.into_iter().all(|length| {
        parts.next().is_some_and(|part| {
            part.len() == length && part.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }) && parts.next().is_none()
}

#[derive(Debug, Default)]
struct SessionRepositoryResolver {
    workspaces: BTreeMap<PathBuf, (Option<PathBuf>, Option<String>)>,
    repositories: BTreeMap<PathBuf, (Option<PathBuf>, Option<String>)>,
    ancestor_boundaries: BTreeMap<PathBuf, Option<PathBuf>>,
    #[cfg(test)]
    metadata_probes: usize,
}

impl SessionRepositoryResolver {
    fn resolve(&mut self, workspace: &Path) -> (Option<PathBuf>, Option<String>) {
        if let Some(repository) = self.workspaces.get(workspace) {
            return repository.clone();
        }
        let repository = self.resolve_uncached(workspace);
        self.workspaces
            .insert(workspace.to_path_buf(), repository.clone());
        repository
    }

    fn resolve_uncached(&mut self, workspace: &Path) -> (Option<PathBuf>, Option<String>) {
        let canonical = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        let Some(boundary) = self.repository_boundary(&canonical) else {
            return (None, None);
        };
        self.repositories
            .entry(boundary.clone())
            .or_insert_with(|| {
                (
                    git_repository_root(&boundary),
                    git_repository_url(&boundary),
                )
            })
            .clone()
    }

    fn repository_boundary(&mut self, workspace: &Path) -> Option<PathBuf> {
        let mut visited = Vec::new();
        let boundary = workspace.ancestors().find_map(|ancestor| {
            if let Some(boundary) = self.ancestor_boundaries.get(ancestor) {
                return Some(boundary.clone());
            }
            #[cfg(test)]
            {
                self.metadata_probes += 1;
            }
            visited.push(ancestor.to_path_buf());
            fs::symlink_metadata(ancestor.join(".git"))
                .is_ok()
                .then(|| Some(ancestor.to_path_buf()))
        });
        let boundary = boundary.flatten();
        for ancestor in visited {
            self.ancestor_boundaries.insert(ancestor, boundary.clone());
        }
        boundary
    }
}

fn git_repository_url(workspace: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["config", "--get", "remote.origin.url"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn git_repository_root(workspace: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
        ])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let mut lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let top_level = PathBuf::from(lines.next()?);
    let git_dir = PathBuf::from(lines.next()?);
    let common_dir = PathBuf::from(lines.next()?);
    let canonical_git_dir = fs::canonicalize(&git_dir).unwrap_or(git_dir);
    let canonical_common_dir = fs::canonicalize(&common_dir).unwrap_or(common_dir.clone());

    if canonical_git_dir != canonical_common_dir
        && canonical_common_dir
            .file_name()
            .and_then(|name| name.to_str())
            == Some(".git")
    {
        return canonical_common_dir.parent().map(Path::to_path_buf);
    }

    Some(fs::canonicalize(&top_level).unwrap_or(top_level))
}

fn collect_recent_codex_paths(
    root: &Path,
    since_date: (i32, u32, u32),
    since: SystemTime,
    candidates: &mut BTreeMap<PathBuf, SystemTime>,
) {
    let Ok(years) = fs::read_dir(root) else {
        return;
    };
    for year in years.filter_map(Result::ok) {
        let Some(year_number) = year
            .file_name()
            .to_str()
            .and_then(|value| value.parse::<i32>().ok())
        else {
            continue;
        };
        let Ok(months) = fs::read_dir(year.path()) else {
            continue;
        };
        for month in months.filter_map(Result::ok) {
            let Some(month_number) = month
                .file_name()
                .to_str()
                .and_then(|value| value.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(days) = fs::read_dir(month.path()) else {
                continue;
            };
            for day in days.filter_map(Result::ok) {
                let Some(day_number) = day
                    .file_name()
                    .to_str()
                    .and_then(|value| value.parse::<u32>().ok())
                else {
                    continue;
                };
                if (year_number, month_number, day_number) < since_date {
                    continue;
                }
                collect_recent_paths(&day.path(), since, candidates);
            }
        }
    }
}

fn collect_recent_paths(
    root: &Path,
    since: SystemTime,
    candidates: &mut BTreeMap<PathBuf, SystemTime>,
) {
    if root.is_file() {
        collect_recent_candidate(root, since, candidates);
        return;
    }
    let max_depth = if root.to_string_lossy().contains("/.claude/projects") {
        3
    } else if root.to_string_lossy().contains("/.cursor/projects") {
        4
    } else {
        10
    };
    for entry in WalkDir::new(root)
        .follow_links(false)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        collect_recent_candidate(entry.path(), since, candidates);
    }
}

fn collect_recent_candidate(
    path: &Path,
    since: SystemTime,
    candidates: &mut BTreeMap<PathBuf, SystemTime>,
) {
    if !is_session_candidate_path(path) {
        return;
    }
    let Some(modified) = fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
    else {
        return;
    };
    if modified >= since {
        candidates.insert(path.to_path_buf(), modified);
    }
}

fn file_state(path: &Path) -> Option<(i64, i64)> {
    let metadata = fs::metadata(path).ok()?;
    let file_mtime = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())?;
    let file_size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
    Some((file_mtime, file_size))
}

fn is_line_boundary(path: &Path, offset: u64) -> bool {
    if offset == 0 {
        return true;
    }
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    if file.seek(SeekFrom::Start(offset - 1)).is_err() {
        return false;
    }
    let mut byte = [0; 1];
    file.read_exact(&mut byte).is_ok_and(|_| byte[0] == b'\n')
}

pub fn infer_session_project(path: &Path, agent: AgentKind) -> Option<PathBuf> {
    let project = if path.extension().is_some_and(|ext| ext == "jsonl") {
        let meta = scan_jsonl_meta(path);
        meta.project.or_else(|| match agent {
            AgentKind::Cursor => cursor_project_from_transcript_path(path),
            _ => None,
        })
    } else if path.file_name().and_then(|name| name.to_str()) == Some("meta.json") {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| cursor_project_from_meta(Some(&value)))
    } else {
        None
    };

    let project = project.filter(|path| path.is_absolute());
    if agent == AgentKind::Codex {
        project.map(|path| ephemeral_chat_root(&path).unwrap_or(path))
    } else {
        project
    }
}

pub(crate) fn scan_codex_index(
    path: &Path,
    sessions: &mut Vec<SessionRecord>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };

    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(value) => {
                if let Some(id) = value.get("id").and_then(Value::as_str) {
                    sessions.push(SessionRecord {
                        id: id.to_string(),
                        agent: AgentKind::Codex,
                        title: value
                            .get("thread_name")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        project: None,
                        repository: None,
                        repository_url: None,
                        logical_project_id: None,
                        logical_project_name: None,
                        path: path.to_path_buf(),
                        started_at: value
                            .get("started_at")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        updated_at: value
                            .get("updated_at")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        message_count: None,
                        first_user_message: None,
                        last_user_message: None,
                        last_assistant_message: None,
                        turn_count: None,
                        model: None,
                        mode: None,
                        approval_mode: None,
                        is_run_everything: None,
                        parent_session_id: None,
                        token_usage: None,
                    });
                }
            }
            Err(err) => warnings.push(format!("{}:{}: {err}", path.display(), index + 1)),
        }
    }

    Ok(())
}

pub(crate) fn scan_codex_jsonl(
    root: &Path,
    sessions: &mut Vec<SessionRecord>,
    cache: Option<&SessionScanCache>,
) {
    if !root.is_dir() {
        return;
    }

    for entry in WalkDir::new(root)
        .follow_links(true)
        .max_depth(6)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().is_some_and(|ext| ext == "jsonl")
        })
    {
        let path = entry.into_path();
        let raw_id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("codex-session")
            .trim_start_matches("rollout-");
        let id = if raw_id.len() >= 36 {
            raw_id[raw_id.len() - 36..].to_string()
        } else {
            raw_id.to_string()
        };
        if let Some(session) = cache
            .and_then(|cache| cache.session_if_current(AgentKind::Codex, &path))
            .filter(session_is_known_non_empty)
        {
            sessions.push(session);
            continue;
        }
        if let Some((session, offset)) =
            cache.and_then(|cache| cache.session_if_appended(AgentKind::Codex, &path))
        {
            if let Some(session) = scan_jsonl_meta_from_offset(&path, offset, session) {
                sessions.push(session);
                continue;
            }
        }
        let meta = scan_jsonl_meta(&path);
        if !meta.has_content {
            continue;
        }
        sessions.push(SessionRecord {
            id,
            agent: AgentKind::Codex,
            title: meta.title,
            project: meta.project,
            repository: None,
            repository_url: meta.repository_url,
            logical_project_id: None,
            logical_project_name: None,
            path,
            started_at: meta.started_at,
            updated_at: meta.updated_at,
            message_count: meta.message_count,
            first_user_message: meta.first_user_message,
            last_user_message: meta.last_user_message,
            last_assistant_message: meta.last_assistant_message,
            turn_count: meta.turn_count,
            model: meta.model,
            mode: None,
            approval_mode: None,
            is_run_everything: None,
            parent_session_id: meta.parent_session_id,
            token_usage: meta.token_usage,
        });
    }
    normalize_session_projects(sessions);
}

pub(crate) fn scan_additional_session_roots(
    roots: &[PathBuf],
    sessions: &mut Vec<SessionRecord>,
    cache: Option<&SessionScanCache>,
) {
    let mut session_paths = BTreeSet::new();
    for root in roots {
        if root.is_file() {
            session_paths.insert(root.clone());
            continue;
        }
        if is_tutti_run_root(root) {
            collect_tutti_run_session_paths(root, &mut session_paths);
            continue;
        }
        for entry in WalkDir::new(root)
            .follow_links(false)
            .max_depth(10)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_type().is_file()
                    && (entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "jsonl")
                        || entry.file_name() == "meta.json")
            })
        {
            session_paths.insert(entry.into_path());
        }
    }
    for path in session_paths {
        if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            scan_detected_jsonl_session(&path, sessions, cache);
        } else if is_cursor_meta_file(&path) {
            scan_cursor_meta_file(&path, sessions, AgentKind::Cursor, cache);
        }
    }
}

fn is_tutti_run_root(root: &Path) -> bool {
    root.file_name().and_then(|name| name.to_str()) == Some("runs")
        && root
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some("agent")
}

fn collect_tutti_run_session_paths(root: &Path, session_paths: &mut BTreeSet<PathBuf>) {
    for sessions in tutti_run_session_roots(root) {
        for entry in WalkDir::new(sessions)
            .follow_links(false)
            .max_depth(6)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_type().is_file()
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "jsonl")
            })
        {
            session_paths.insert(entry.into_path());
        }
    }
}

fn tutti_run_session_roots(root: &Path) -> Vec<PathBuf> {
    let Ok(runs) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut roots = runs
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|run| run.path().join("codex-home/sessions"))
        .filter(|sessions| sessions.is_dir())
        .collect::<Vec<_>>();
    roots.sort();
    roots
}

fn scan_detected_jsonl_session(
    path: &Path,
    sessions: &mut Vec<SessionRecord>,
    cache: Option<&SessionScanCache>,
) {
    let Some(agent) = cache
        .and_then(|cache| cache.agent_for_path(path))
        .or_else(|| detect_jsonl_agent(path))
    else {
        return;
    };
    if let Some(session) = cache
        .and_then(|cache| cache.session_if_current(agent, path))
        .filter(session_is_known_non_empty)
    {
        sessions.push(session);
        return;
    }
    if matches!(agent, AgentKind::Codex | AgentKind::Cursor) {
        if let Some((session, offset)) =
            cache.and_then(|cache| cache.session_if_appended(agent, path))
        {
            if let Some(session) = scan_jsonl_meta_from_offset(path, offset, session) {
                sessions.push(session);
                return;
            }
        }
    }
    let meta = scan_jsonl_meta(path);
    if !meta.has_content {
        return;
    }
    let file_updated_at = file_modified_iso(path);
    let project = match agent {
        AgentKind::Claude => meta
            .project
            .or_else(|| path.parent().map(Path::to_path_buf)),
        AgentKind::Cursor => meta
            .project
            .or_else(|| cursor_project_from_transcript_path(path)),
        _ => meta.project,
    };
    let id = match agent {
        AgentKind::Codex => codex_session_id_from_path(path),
        _ => path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("session")
            .to_string(),
    };
    sessions.push(SessionRecord {
        id,
        agent,
        title: meta.title,
        project,
        repository: None,
        repository_url: meta.repository_url,
        logical_project_id: None,
        logical_project_name: None,
        path: path.to_path_buf(),
        started_at: meta.started_at.or_else(|| file_updated_at.clone()),
        updated_at: meta.updated_at.or(file_updated_at),
        message_count: meta.message_count,
        first_user_message: meta.first_user_message,
        last_user_message: meta.last_user_message,
        last_assistant_message: meta.last_assistant_message,
        turn_count: meta.turn_count,
        model: meta.model,
        mode: None,
        approval_mode: None,
        is_run_everything: None,
        parent_session_id: meta.parent_session_id,
        token_usage: meta.token_usage,
    });
}

fn detect_jsonl_agent(path: &Path) -> Option<AgentKind> {
    let file = fs::File::open(path).ok()?;
    for line in BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .take(64)
    {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let record_type = value.get("type").and_then(Value::as_str);
        if matches!(
            record_type,
            Some("session_meta" | "response_item" | "event_msg" | "turn_context" | "compacted")
        ) {
            return Some(AgentKind::Codex);
        }
        if value.get("sessionId").is_some() && record_type.is_some() {
            return Some(AgentKind::Claude);
        }
        if value.get("role").and_then(Value::as_str).is_some() && value.get("message").is_some() {
            return Some(AgentKind::Cursor);
        }
    }
    None
}

fn codex_session_id_from_path(path: &Path) -> String {
    let raw_id = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("codex-session")
        .trim_start_matches("rollout-");
    if raw_id.len() >= 36 {
        raw_id[raw_id.len() - 36..].to_string()
    } else {
        raw_id.to_string()
    }
}

pub(crate) fn scan_claude_projects(
    root: &Path,
    sessions: &mut Vec<SessionRecord>,
    cache: Option<&SessionScanCache>,
) {
    if !root.is_dir() {
        return;
    }

    for entry in WalkDir::new(root)
        .follow_links(true)
        .max_depth(3)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().is_some_and(|ext| ext == "jsonl")
        })
    {
        let path = entry.into_path();
        let id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("claude-session")
            .to_string();
        if let Some(session) = cache
            .and_then(|cache| cache.session_if_current(AgentKind::Claude, &path))
            .filter(session_is_known_non_empty)
        {
            sessions.push(session);
            continue;
        }
        let meta = scan_jsonl_meta(&path);
        if !meta.has_content {
            continue;
        }
        sessions.push(SessionRecord {
            id,
            agent: AgentKind::Claude,
            title: meta.title,
            project: meta
                .project
                .or_else(|| path.parent().map(Path::to_path_buf)),
            repository: None,
            repository_url: meta.repository_url,
            logical_project_id: None,
            logical_project_name: None,
            path,
            started_at: meta.started_at,
            updated_at: meta.updated_at,
            message_count: meta.message_count,
            first_user_message: meta.first_user_message,
            last_user_message: meta.last_user_message,
            last_assistant_message: meta.last_assistant_message,
            turn_count: meta.turn_count,
            model: meta.model,
            mode: None,
            approval_mode: None,
            is_run_everything: None,
            parent_session_id: meta.parent_session_id,
            token_usage: meta.token_usage,
        });
    }
}

fn merge_sessions(sessions: Vec<SessionRecord>) -> Vec<SessionRecord> {
    let mut by_key: BTreeMap<(AgentKind, String), SessionRecord> = BTreeMap::new();

    for session in sessions {
        by_key
            .entry((session.agent, session.id.clone()))
            .and_modify(|existing| merge_session(existing, &session))
            .or_insert(session);
    }

    by_key.into_values().collect()
}

fn merge_session(existing: &mut SessionRecord, incoming: &SessionRecord) {
    if existing.title.is_none() {
        existing.title = incoming.title.clone();
    }
    if existing.project.is_none() {
        existing.project = incoming.project.clone();
    }
    if let Some(started_at) = &incoming.started_at {
        if existing
            .started_at
            .as_deref()
            .is_none_or(|current| started_at.as_str() < current)
        {
            existing.started_at = Some(started_at.clone());
        }
    }
    if existing.updated_at.is_none() || incoming.updated_at > existing.updated_at {
        existing.updated_at = incoming.updated_at.clone();
    }
    if existing.message_count.is_none() {
        existing.message_count = incoming.message_count;
    }
    if existing.first_user_message.is_none() {
        existing.first_user_message = incoming.first_user_message.clone();
    }
    if incoming.last_user_message.is_some() {
        existing.last_user_message = incoming.last_user_message.clone();
    }
    if incoming.last_assistant_message.is_some() {
        existing.last_assistant_message = incoming.last_assistant_message.clone();
    }
    if existing.turn_count.is_none() {
        existing.turn_count = incoming.turn_count;
    }
    if existing.model.is_none() {
        existing.model = incoming.model.clone();
    }
    if existing.mode.is_none() {
        existing.mode = incoming.mode.clone();
    }
    if existing.approval_mode.is_none() {
        existing.approval_mode = incoming.approval_mode.clone();
    }
    if existing.is_run_everything.is_none() {
        existing.is_run_everything = incoming.is_run_everything;
    }
    if existing.parent_session_id.is_none() {
        existing.parent_session_id = incoming.parent_session_id.clone();
    }
    if existing.token_usage.is_none() {
        existing.token_usage = incoming.token_usage.clone();
    }
    if should_replace_session_path(&existing.path, &incoming.path) {
        existing.path = incoming.path.clone();
    }
}

fn should_replace_session_path(existing: &Path, incoming: &Path) -> bool {
    if is_transcript_path(incoming) && !is_transcript_path(existing) {
        return true;
    }

    is_index_path(existing) && !is_metadata_path(incoming)
}

fn is_transcript_path(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl") && !is_index_path(path)
}

fn is_metadata_path(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("meta.json" | "session_index.jsonl")
    )
}

fn is_index_path(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some("session_index.jsonl")
}

#[derive(Default)]
struct JsonlMeta {
    has_content: bool,
    message_count: Option<usize>,
    first_user_message: Option<String>,
    last_user_message: Option<String>,
    last_assistant_message: Option<String>,
    turn_count: Option<usize>,
    started_at: Option<String>,
    updated_at: Option<String>,
    project: Option<PathBuf>,
    repository_url: Option<String>,
    title: Option<String>,
    model: Option<String>,
    parent_session_id: Option<String>,
    token_usage: Option<SessionTokenUsage>,
}

fn scan_jsonl_meta(path: &Path) -> JsonlMeta {
    let Ok(file) = fs::File::open(path) else {
        return JsonlMeta::default();
    };
    let mut meta = JsonlMeta {
        has_content: false,
        message_count: Some(0),
        turn_count: Some(0),
        ..Default::default()
    };
    let mut claude_message_usage = BTreeMap::new();

    scan_jsonl_meta_lines(
        BufReader::new(file).lines().map_while(Result::ok),
        &mut meta,
        &mut claude_message_usage,
    );

    if meta.token_usage.is_none() && !claude_message_usage.is_empty() {
        meta.token_usage = sum_token_usage(claude_message_usage.values());
    }

    meta
}

fn scan_jsonl_meta_from_offset(
    path: &Path,
    offset: u64,
    session: SessionRecord,
) -> Option<SessionRecord> {
    let mut file = fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;

    let mut meta = JsonlMeta {
        has_content: session_is_known_non_empty(&session),
        message_count: session.message_count,
        first_user_message: session.first_user_message,
        last_user_message: session.last_user_message,
        last_assistant_message: session.last_assistant_message,
        turn_count: session.turn_count,
        started_at: session.started_at,
        updated_at: session.updated_at,
        project: session.project,
        repository_url: session.repository_url,
        title: session.title,
        model: session.model,
        parent_session_id: session.parent_session_id,
        token_usage: session.token_usage,
    };
    let mut claude_message_usage = BTreeMap::new();
    scan_jsonl_meta_lines(
        BufReader::new(file).lines().map_while(Result::ok),
        &mut meta,
        &mut claude_message_usage,
    );

    if !meta.has_content {
        return None;
    }

    Some(SessionRecord {
        title: meta.title,
        project: meta.project,
        repository_url: meta.repository_url,
        started_at: meta.started_at,
        updated_at: meta.updated_at,
        message_count: meta.message_count,
        first_user_message: meta.first_user_message,
        last_user_message: meta.last_user_message,
        last_assistant_message: meta.last_assistant_message,
        turn_count: meta.turn_count,
        model: meta.model,
        parent_session_id: meta.parent_session_id,
        token_usage: meta.token_usage,
        ..session
    })
}

fn scan_jsonl_meta_lines<I, S>(
    lines: I,
    meta: &mut JsonlMeta,
    claude_message_usage: &mut BTreeMap<String, SessionTokenUsage>,
) where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    for line in lines {
        let line = line.as_ref();
        if line.trim().is_empty() {
            continue;
        }
        meta.message_count = meta.message_count.map(|count| count + 1);
        let prefix = metadata_hint_prefix(line);
        meta.has_content |= line_has_session_content(prefix);
        if !line_requires_full_metadata_parse(prefix, meta) {
            if let Some(timestamp) = json_string_field(prefix, "\"timestamp\"") {
                apply_time_bounds(meta, timestamp);
            }
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if extract_session_title(&value).is_some() {
            meta.turn_count = meta.turn_count.map(|count| count + 1);
        }
        if let Some((role, body)) = extract_session_message(&value) {
            if let Some(body) = clean_preview_text(&body) {
                match role {
                    "user" => {
                        if meta.first_user_message.is_none() {
                            meta.first_user_message = Some(body.clone());
                        }
                        meta.last_user_message = Some(body);
                    }
                    "assistant" => meta.last_assistant_message = Some(body),
                    _ => {}
                }
            }
        }
        if let Some(timestamp) =
            json_str(&value, &["timestamp"]).or_else(|| json_str(&value, &["payload", "timestamp"]))
        {
            apply_time_bounds(meta, timestamp);
        }
        if meta.project.is_none() {
            if let Some(cwd) =
                json_str(&value, &["cwd"]).or_else(|| json_str(&value, &["payload", "cwd"]))
            {
                meta.project = Some(PathBuf::from(cwd));
            }
        }
        if meta.repository_url.is_none() {
            meta.repository_url = value
                .pointer("/payload/git/repository_url")
                .or_else(|| value.pointer("/git/repository_url"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(str::to_string);
        }
        if meta.title.is_none() {
            meta.title = extract_session_title(&value);
        }
        if let Some(model) = extract_session_model(&value) {
            meta.model = Some(model);
        }
        if meta.parent_session_id.is_none() {
            meta.parent_session_id = extract_parent_session_id(&value);
        }
        if let Some(token_usage) = extract_codex_token_usage(&value) {
            meta.token_usage = Some(token_usage);
        }
        if let Some((message_id, token_usage)) = extract_claude_token_usage(&value) {
            claude_message_usage.insert(message_id, token_usage);
        }
    }
}

const METADATA_HINT_PREFIX_BYTES: usize = 16 * 1024;

fn metadata_hint_prefix(line: &str) -> &str {
    let mut prefix_end = line.len().min(METADATA_HINT_PREFIX_BYTES);
    while !line.is_char_boundary(prefix_end) {
        prefix_end -= 1;
    }
    &line[..prefix_end]
}

fn line_requires_full_metadata_parse(prefix: &str, meta: &JsonlMeta) -> bool {
    match json_string_field(prefix, "\"type\"") {
        Some("session_meta" | "turn_context" | "assistant" | "user") => true,
        Some("event_msg") => {
            prefix.contains("\"thread_settings_applied\"")
                || prefix.contains("\"token_count\"")
                || line_has_message_role(prefix)
        }
        Some("response_item") => line_has_message_role(prefix),
        _ => {
            line_has_message_role(prefix)
                || (meta.project.is_none() && prefix.contains("\"cwd\""))
                || (meta.repository_url.is_none() && prefix.contains("\"repository_url\""))
        }
    }
}

fn line_has_session_content(prefix: &str) -> bool {
    match json_string_field(prefix, "\"type\"") {
        Some("session_meta" | "turn_context" | "world_state") => false,
        Some("response_item") => !matches!(
            json_string_field(prefix, "\"role\""),
            Some("developer" | "system")
        ),
        Some("event_msg") => [
            "user_message",
            "agent_message",
            "agent_reasoning",
            "sub_agent_activity",
            "context_compacted",
        ]
        .iter()
        .any(|kind| line_contains_json_string_value(prefix, kind)),
        Some("compacted" | "user" | "assistant") => true,
        _ => matches!(
            json_string_field(prefix, "\"role\""),
            Some("user" | "assistant")
        ),
    }
}

fn line_contains_json_string_value(line: &str, expected: &str) -> bool {
    let marker = "\"type\"";
    let mut search_from = 0;
    while let Some(offset) = line[search_from..].find(marker) {
        let start = search_from + offset;
        search_from = start + marker.len();
        if is_escaped_at(line, start) {
            continue;
        }
        let value = line[search_from..].trim_start();
        let Some(value) = value.strip_prefix(':').map(str::trim_start) else {
            continue;
        };
        let Some(value) = value.strip_prefix('"') else {
            continue;
        };
        let Some(end) = value.find('"') else {
            continue;
        };
        if !value[..end].contains('\\') && &value[..end] == expected {
            return true;
        }
    }
    false
}

fn session_is_known_non_empty(session: &SessionRecord) -> bool {
    session.title.is_some()
        || session.turn_count.is_some_and(|count| count > 0)
        || session.token_usage.is_some()
}

fn line_has_user_role(prefix: &str) -> bool {
    prefix.contains("\"role\"") && prefix.contains("\"user\"")
}

fn line_has_message_role(prefix: &str) -> bool {
    line_has_user_role(prefix) || (prefix.contains("\"role\"") && prefix.contains("\"assistant\""))
}

fn is_escaped_at(line: &str, start: usize) -> bool {
    line[..start]
        .chars()
        .rev()
        .take_while(|ch| *ch == '\\')
        .count()
        % 2
        == 1
}

fn json_string_field<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let mut search_from = 0;
    while let Some(offset) = line[search_from..].find(marker) {
        let start = search_from + offset;
        search_from = start + marker.len();
        if is_escaped_at(line, start) {
            continue;
        }
        let value = line[search_from..].trim_start();
        let value = value.strip_prefix(':')?.trim_start();
        let value = value.strip_prefix('"')?;
        let end = value.find('"')?;
        if !value[..end].contains('\\') {
            return Some(&value[..end]);
        }
    }
    None
}

fn extract_session_model(value: &Value) -> Option<String> {
    let model = match value.get("type").and_then(Value::as_str) {
        Some("turn_context") => value.pointer("/payload/model"),
        Some("event_msg")
            if value.pointer("/payload/type").and_then(Value::as_str)
                == Some("thread_settings_applied") =>
        {
            value.pointer("/payload/thread_settings/model")
        }
        Some("assistant") => value.pointer("/message/model"),
        _ => None,
    };
    model
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

fn extract_parent_session_id(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    value
        .pointer("/payload/parent_thread_id")
        .or_else(|| value.pointer("/payload/source/subagent/thread_spawn/parent_thread_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn extract_codex_token_usage(value: &Value) -> Option<SessionTokenUsage> {
    if value.pointer("/payload/type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let usage = value.pointer("/payload/info/total_token_usage")?;
    let total_tokens = usage.get("total_tokens")?.as_u64()?;
    if total_tokens == 0 {
        return None;
    }
    Some(SessionTokenUsage {
        input_tokens: usage.get("input_tokens")?.as_u64()?,
        cached_input_tokens: usage.get("cached_input_tokens")?.as_u64()?,
        output_tokens: usage.get("output_tokens")?.as_u64()?,
        reasoning_output_tokens: usage
            .get("reasoning_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        total_tokens,
    })
}

fn extract_claude_token_usage(value: &Value) -> Option<(String, SessionTokenUsage)> {
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let message = value.get("message")?;
    let message_id = message.get("id")?.as_str()?.to_string();
    let usage = message.get("usage")?;
    let direct_input_tokens = usage.get("input_tokens")?.as_u64()?;
    let cache_creation_input_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached_input_tokens = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage.get("output_tokens")?.as_u64()?;
    let input_tokens = direct_input_tokens
        .checked_add(cache_creation_input_tokens)?
        .checked_add(cached_input_tokens)?;
    let total_tokens = input_tokens.checked_add(output_tokens)?;
    if total_tokens == 0 {
        return None;
    }
    Some((
        message_id,
        SessionTokenUsage {
            input_tokens,
            cached_input_tokens,
            output_tokens,
            reasoning_output_tokens: 0,
            total_tokens,
        },
    ))
}

fn sum_token_usage<'a>(
    usages: impl Iterator<Item = &'a SessionTokenUsage>,
) -> Option<SessionTokenUsage> {
    let mut total = SessionTokenUsage {
        input_tokens: 0,
        cached_input_tokens: 0,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 0,
    };
    for usage in usages {
        total.input_tokens = total.input_tokens.checked_add(usage.input_tokens)?;
        total.cached_input_tokens = total
            .cached_input_tokens
            .checked_add(usage.cached_input_tokens)?;
        total.output_tokens = total.output_tokens.checked_add(usage.output_tokens)?;
        total.reasoning_output_tokens = total
            .reasoning_output_tokens
            .checked_add(usage.reasoning_output_tokens)?;
        total.total_tokens = total.total_tokens.checked_add(usage.total_tokens)?;
    }
    Some(total)
}

fn json_str<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_str()
}

fn apply_time_bounds(meta: &mut JsonlMeta, timestamp: &str) {
    if meta
        .started_at
        .as_deref()
        .is_none_or(|current| timestamp < current)
    {
        meta.started_at = Some(timestamp.to_string());
    }
    if meta
        .updated_at
        .as_deref()
        .is_none_or(|current| timestamp > current)
    {
        meta.updated_at = Some(timestamp.to_string());
    }
}

pub(crate) fn scan_cursor_meta(
    root: &Path,
    sessions: &mut Vec<SessionRecord>,
    agent: AgentKind,
    cache: Option<&SessionScanCache>,
) {
    if !root.is_dir() {
        return;
    }

    for entry in WalkDir::new(root)
        .follow_links(true)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "meta.json")
    {
        let path = entry.into_path();
        scan_cursor_meta_file(&path, sessions, agent, cache);
    }

    for entry in WalkDir::new(root)
        .follow_links(true)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "store.db")
    {
        let path = entry.into_path();
        if path.with_file_name("meta.json").is_file() {
            continue;
        }
        scan_cursor_store_file(&path, sessions, agent, cache);
    }
}

fn is_cursor_meta_file(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("schemaVersion").and_then(Value::as_u64))
        .is_some()
}

fn scan_cursor_meta_file(
    path: &Path,
    sessions: &mut Vec<SessionRecord>,
    agent: AgentKind,
    cache: Option<&SessionScanCache>,
) {
    let id = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("cursor-session")
        .to_string();
    if let Some(session) = cache.and_then(|cache| cache.session_if_current_id(agent, &id))
        && is_cursor_transcript_path(&session.path)
    {
        sessions.push(session);
        return;
    }
    let value = fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let file_updated_at = file_modified_iso(path);
    let store_path = path.parent().map(|parent| parent.join("store.db"));
    let store_meta = scan_cursor_store_db(store_path);
    let explicit_title = string_field(
        value
            .as_ref()
            .and_then(|value| value.get("name").or_else(|| value.get("title"))),
    )
    .or_else(|| store_meta.title.clone())
    .filter(|title| {
        store_meta.parent_session_id.is_none() || !title.eq_ignore_ascii_case("New Agent")
    });
    if explicit_title.is_none()
        && store_meta.message_count.is_none()
        && store_meta.model.is_none()
        && store_meta.mode.is_none()
        && store_meta.approval_mode.is_none()
        && store_meta.is_run_everything.is_none()
        && store_meta.parent_session_id.is_none()
        && store_meta.first_user_message.is_none()
        && store_meta.last_user_message.is_none()
        && store_meta.last_assistant_message.is_none()
    {
        return;
    }
    sessions.push(SessionRecord {
        id,
        agent,
        title: explicit_title.or_else(|| {
            cursor_project_from_meta(value.as_ref()).and_then(|path| title_from_project_path(&path))
        }),
        project: cursor_project_from_meta(value.as_ref()),
        repository: None,
        repository_url: None,
        logical_project_id: None,
        logical_project_name: None,
        path: path.to_path_buf(),
        started_at: cursor_time_field(
            value.as_ref(),
            &["createdAt", "created_at", "startedAt", "started_at"],
        )
        .or(store_meta.started_at.clone())
        .or_else(|| file_updated_at.clone()),
        updated_at: cursor_time_field(value.as_ref(), &["updatedAt", "updated_at"])
            .or(store_meta.updated_at)
            .or(file_updated_at),
        message_count: store_meta.message_count.or(Some(0)),
        first_user_message: store_meta.first_user_message,
        last_user_message: store_meta.last_user_message,
        last_assistant_message: store_meta.last_assistant_message,
        turn_count: store_meta.turn_count.or(Some(0)),
        model: store_meta.model.clone(),
        mode: store_meta.mode.clone(),
        approval_mode: store_meta.approval_mode.clone(),
        is_run_everything: store_meta.is_run_everything,
        parent_session_id: store_meta.parent_session_id,
        token_usage: None,
    });
}

fn scan_cursor_store_file(
    path: &Path,
    sessions: &mut Vec<SessionRecord>,
    agent: AgentKind,
    cache: Option<&SessionScanCache>,
) {
    let id = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("cursor-session")
        .to_string();
    if let Some(session) = cache.and_then(|cache| cache.session_if_current_id(agent, &id)) {
        if is_cursor_transcript_path(&session.path) {
            sessions.push(session);
            return;
        }
    }
    let file_updated_at = file_modified_iso(path);
    let store_meta = scan_cursor_store_db(Some(path.to_path_buf()));
    if store_meta.title.is_none()
        && store_meta.message_count.is_none()
        && store_meta.model.is_none()
        && store_meta.mode.is_none()
        && store_meta.approval_mode.is_none()
        && store_meta.is_run_everything.is_none()
        && store_meta.parent_session_id.is_none()
        && store_meta.first_user_message.is_none()
        && store_meta.last_user_message.is_none()
        && store_meta.last_assistant_message.is_none()
    {
        return;
    }
    let title = store_meta.title.filter(|title| {
        store_meta.parent_session_id.is_none() || !title.eq_ignore_ascii_case("New Agent")
    });
    sessions.push(SessionRecord {
        id,
        agent,
        title,
        project: None,
        repository: None,
        repository_url: None,
        logical_project_id: None,
        logical_project_name: None,
        path: path.to_path_buf(),
        started_at: store_meta.started_at,
        updated_at: store_meta.updated_at.or(file_updated_at),
        message_count: store_meta.message_count,
        first_user_message: store_meta.first_user_message,
        last_user_message: store_meta.last_user_message,
        last_assistant_message: store_meta.last_assistant_message,
        turn_count: store_meta.turn_count,
        model: store_meta.model,
        mode: store_meta.mode,
        approval_mode: store_meta.approval_mode,
        is_run_everything: store_meta.is_run_everything,
        parent_session_id: store_meta.parent_session_id,
        token_usage: None,
    });
}

fn is_cursor_transcript_path(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl")
        && path
            .components()
            .any(|component| component.as_os_str() == "agent-transcripts")
}

pub(crate) fn scan_cursor_agent_transcripts(
    root: &Path,
    sessions: &mut Vec<SessionRecord>,
    cache: Option<&SessionScanCache>,
) {
    if !root.is_dir() {
        return;
    }

    for entry in WalkDir::new(root)
        .follow_links(true)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().is_some_and(|ext| ext == "jsonl")
                && entry
                    .path()
                    .components()
                    .any(|component| component.as_os_str() == "agent-transcripts")
        })
    {
        let path = entry.into_path();
        let id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("cursor-session")
            .to_string();
        if let Some(session) = cache
            .and_then(|cache| cache.session_if_current(AgentKind::Cursor, &path))
            .filter(session_is_known_non_empty)
        {
            sessions.push(session);
            continue;
        }
        if let Some((session, offset)) =
            cache.and_then(|cache| cache.session_if_appended(AgentKind::Cursor, &path))
        {
            if let Some(session) = scan_jsonl_meta_from_offset(&path, offset, session) {
                sessions.push(session);
                continue;
            }
        }
        let file_updated_at = file_modified_iso(&path);
        let meta = scan_jsonl_meta(&path);
        if !meta.has_content {
            continue;
        }
        sessions.push(SessionRecord {
            id,
            agent: AgentKind::Cursor,
            title: meta.title,
            project: meta
                .project
                .or_else(|| cursor_project_from_transcript_path(&path)),
            repository: None,
            repository_url: meta.repository_url,
            logical_project_id: None,
            logical_project_name: None,
            path,
            started_at: meta.started_at.or_else(|| file_updated_at.clone()),
            updated_at: meta.updated_at.or(file_updated_at),
            message_count: meta.message_count,
            first_user_message: meta.first_user_message,
            last_user_message: meta.last_user_message,
            last_assistant_message: meta.last_assistant_message,
            turn_count: meta.turn_count,
            model: meta.model,
            mode: None,
            approval_mode: None,
            is_run_everything: None,
            parent_session_id: meta.parent_session_id,
            token_usage: meta.token_usage,
        });
    }
}

#[derive(Default)]
struct CursorStoreMeta {
    message_count: Option<usize>,
    first_user_message: Option<String>,
    last_user_message: Option<String>,
    last_assistant_message: Option<String>,
    turn_count: Option<usize>,
    started_at: Option<String>,
    updated_at: Option<String>,
    title: Option<String>,
    model: Option<String>,
    mode: Option<String>,
    approval_mode: Option<String>,
    is_run_everything: Option<bool>,
    parent_session_id: Option<String>,
}

fn scan_cursor_store_db(path: Option<PathBuf>) -> CursorStoreMeta {
    let Some(path) = path else {
        return CursorStoreMeta::default();
    };
    if !path.is_file() {
        return CursorStoreMeta::default();
    }

    let Ok(connection) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return CursorStoreMeta::default();
    };

    let stored_meta = connection
        .query_row("select value from meta where key = '0'", [], |row| {
            row.get::<_, String>(0)
        })
        .ok()
        .and_then(|text| parse_cursor_store_value(&text));
    let mut title = stored_meta
        .as_ref()
        .and_then(|value| string_field(value.get("name").or_else(|| value.get("title"))));
    let mut model = stored_meta
        .as_ref()
        .and_then(|value| cursor_store_model(value));
    let mode = stored_meta
        .as_ref()
        .and_then(|value| string_field(value.get("mode")));
    let approval_mode = stored_meta.as_ref().and_then(|value| {
        string_field(
            value
                .get("approvalMode")
                .or_else(|| value.get("approval_mode")),
        )
    });
    let is_run_everything = stored_meta.as_ref().and_then(|value| {
        value
            .get("isRunEverything")
            .or_else(|| value.get("is_run_everything"))
            .and_then(Value::as_bool)
    });
    let started_at = stored_meta.as_ref().and_then(|value| {
        cursor_time_field(
            Some(value),
            &["createdAt", "created_at", "startedAt", "started_at"],
        )
    });
    let parent_session_id = stored_meta
        .as_ref()
        .and_then(|value| value.pointer("/subagentInfo/parentAgentId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);

    let mut message_count = 0usize;
    let mut turn_count = 0usize;
    let mut first_user_message = None;
    let mut last_user_message = None;
    let mut last_assistant_message = None;
    if let Ok(mut statement) = connection.prepare("select data from blobs") {
        if let Ok(rows) = statement.query_map([], |row| row.get::<_, Vec<u8>>(0)) {
            for bytes in rows.filter_map(Result::ok) {
                let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                    continue;
                };
                if value.get("role").and_then(Value::as_str).is_some() {
                    message_count += 1;
                }
                if extract_session_title(&value).is_some() {
                    turn_count += 1;
                }
                if title.is_none() {
                    title = extract_session_title(&value);
                }
                if model.is_none() {
                    model = extract_cursor_blob_model(&value);
                }
                if let Some((role, body)) = extract_session_message(&value) {
                    if let Some(body) = clean_preview_text(&body) {
                        match role {
                            "user" => {
                                if first_user_message.is_none() {
                                    first_user_message = Some(body.clone());
                                }
                                last_user_message = Some(body);
                            }
                            "assistant" => last_assistant_message = Some(body),
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    CursorStoreMeta {
        message_count: (message_count > 0).then_some(message_count),
        first_user_message,
        last_user_message,
        last_assistant_message,
        turn_count: (turn_count > 0).then_some(turn_count),
        started_at,
        updated_at: None,
        title,
        model,
        mode,
        approval_mode,
        is_run_everything,
        parent_session_id,
    }
}

fn cursor_store_model(value: &Value) -> Option<String> {
    string_field(value.get("lastUsedModel").or_else(|| value.get("model")))
        .filter(|model| !model.eq_ignore_ascii_case("default"))
}

pub(crate) fn extract_cursor_blob_model(value: &Value) -> Option<String> {
    if let Some(content) = value.get("content").and_then(Value::as_array) {
        for item in content.iter().rev() {
            if let Some(model) = item
                .pointer("/providerOptions/cursor/modelName")
                .and_then(|model| string_field(Some(model)))
            {
                return Some(model);
            }
        }
    }
    value
        .pointer("/providerOptions/cursor/modelName")
        .and_then(|model| string_field(Some(model)))
}

fn parse_cursor_store_value(text: &str) -> Option<Value> {
    serde_json::from_str(text).ok().or_else(|| {
        if !text.len().is_multiple_of(2) {
            return None;
        }
        let bytes = text
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let pair = std::str::from_utf8(pair).ok()?;
                u8::from_str_radix(pair, 16).ok()
            })
            .collect::<Option<Vec<_>>>()?;
        serde_json::from_slice(&bytes).ok()
    })
}

fn cursor_project_from_meta(value: Option<&Value>) -> Option<PathBuf> {
    value
        .and_then(|value| {
            value
                .get("cwd")
                .or_else(|| value.get("workspace"))
                .or_else(|| value.get("workspacePath"))
                .or_else(|| value.get("folder"))
                .or_else(|| value.get("folderPath"))
        })
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

fn cursor_time_field(value: Option<&Value>, keys: &[&str]) -> Option<String> {
    let value = value?;
    for key in keys {
        if let Some(timestamp) = value.get(*key).and_then(Value::as_str) {
            return Some(timestamp.to_string());
        }
        if let Some(timestamp) = value.get(format!("{key}Ms")).and_then(Value::as_i64) {
            return unix_ms_to_iso(timestamp);
        }
    }
    value
        .get("createdAtMs")
        .filter(|_| keys.iter().any(|key| key.starts_with("created")))
        .and_then(Value::as_i64)
        .and_then(unix_ms_to_iso)
        .or_else(|| {
            value
                .get("updatedAtMs")
                .filter(|_| keys.iter().any(|key| key.starts_with("updated")))
                .and_then(Value::as_i64)
                .and_then(unix_ms_to_iso)
        })
}

fn cursor_project_from_transcript_path(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    while let Some(component) = components.next() {
        if component.as_os_str() == "projects" {
            let project = components.next()?.as_os_str().to_str()?;
            return decode_cursor_project_dir(project);
        }
    }
    None
}

fn decode_cursor_project_dir(value: &str) -> Option<PathBuf> {
    let parts = value
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    if parts[0] == "Users" {
        return Some(PathBuf::from(format!("/{}", parts.join("/"))));
    }
    Some(PathBuf::from(parts.join("/")))
}

fn title_from_project_path(path: &Path) -> Option<String> {
    let parts = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    for marker in ["installations", "workspaces"] {
        if let Some(index) = parts.iter().position(|part| *part == marker) {
            if let Some(name) = parts.get(index + 1) {
                return Some((*name).to_string());
            }
        }
    }
    if let Some(index) = parts.iter().position(|part| *part == "apps") {
        if index > 0 {
            return Some(parts[index - 1].to_string());
        }
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
}

fn extract_session_title(value: &Value) -> Option<String> {
    let role = json_str(value, &["role"])
        .or_else(|| json_str(value, &["message", "role"]))
        .or_else(|| json_str(value, &["payload", "role"]));
    if role != Some("user") {
        return None;
    }
    let text = extract_user_text(
        value
            .get("message")
            .and_then(|message| message.get("content"))
            .or_else(|| value.get("content"))
            .or_else(|| value.pointer("/payload/content")),
    )?;
    clean_title(&text)
}

fn extract_session_message(value: &Value) -> Option<(&'static str, String)> {
    let role = json_str(value, &["role"])
        .or_else(|| json_str(value, &["message", "role"]))
        .or_else(|| json_str(value, &["payload", "role"]))?;
    let role = match role {
        "user" => "user",
        "assistant" => "assistant",
        _ => return None,
    };
    let content = value
        .pointer("/message/content")
        .or_else(|| value.get("content"))
        .or_else(|| value.pointer("/payload/content"))
        .or_else(|| value.get("message"));
    Some((role, extract_user_text(content)?))
}

pub(crate) const SESSION_PREVIEW_MAX_CHARS: usize = 256;

pub(crate) fn bound_session_preview(value: Option<String>) -> Option<String> {
    value.and_then(|value| bound_session_preview_text(&value))
}

fn bound_session_preview_text(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut chars = text.chars();
    let mut value: String = chars.by_ref().take(SESSION_PREVIEW_MAX_CHARS).collect();
    if chars.next().is_some() {
        value.push('…');
    }
    Some(value)
}

fn clean_preview_text(text: &str) -> Option<String> {
    let mut text = text.trim();
    if let Some(inner) = extract_tag_body(text, "user_query") {
        text = inner.trim();
    }
    if text.is_empty()
        || [
            "# AGENTS.md instructions",
            "<codex_internal_context",
            "<local-command-caveat>",
            "<command-name>",
            "<local-command-stdout>",
            "<task-notification>",
            "<environment_context>",
            "<permissions instructions>",
            "<app-context>",
            "<collaboration_mode>",
            "<skills_instructions>",
            "<plugins_instructions>",
            "<system-reminder>",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
    {
        return None;
    }
    bound_session_preview_text(text)
}

fn extract_user_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Array(items) => {
            let text = items
                .iter()
                .filter(|item| {
                    !matches!(
                        item.get("type").and_then(Value::as_str),
                        Some("tool_result" | "tool_use" | "function_call" | "function_call_output")
                    )
                })
                .filter_map(|item| {
                    item.get("text")
                        .or_else(|| item.get("content"))
                        .and_then(Value::as_str)
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then_some(text)
        }
        value => extract_text(Some(value)),
    }
}

fn extract_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| {
                    item.get("text")
                        .or_else(|| item.get("content"))
                        .and_then(Value::as_str)
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then_some(text)
        }
        Value::Object(_) => None,
        _ => None,
    }
}

fn clean_title(text: &str) -> Option<String> {
    let mut text = text.trim();
    if text.starts_with("# AGENTS.md instructions")
        || text.starts_with("<codex_internal_context")
        || text.starts_with("<local-command-caveat>")
        || text.starts_with("<command-name>")
        || text.starts_with("<local-command-stdout>")
        || text.starts_with("<task-notification>")
        || text.starts_with("<environment_context>")
        || text.starts_with("<permissions instructions>")
        || text.starts_with("<app-context>")
        || text.starts_with("<collaboration_mode>")
        || text.starts_with("<skills_instructions>")
        || text.starts_with("<plugins_instructions>")
        || text.starts_with("<system-reminder>")
    {
        return None;
    }
    if let Some(inner) = extract_tag_body(text, "user_query") {
        text = inner.trim();
    }
    let title = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("---"))?;
    Some(title.chars().take(96).collect())
}

fn extract_tag_body<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = text.find(&start_tag)? + start_tag.len();
    let end = text[start..].find(&end_tag)? + start;
    let inner = text[start..end].trim();
    (!inner.is_empty()).then_some(inner)
}

fn string_field(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).and_then(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    })
}

fn unix_ms_to_iso(value: i64) -> Option<String> {
    let seconds = value.div_euclid(1000);
    let millis = value.rem_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z"
    ))
}

fn file_modified_iso(path: &Path) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    system_time_to_iso(modified)
}

fn system_time_to_iso(value: SystemTime) -> Option<String> {
    let duration = value.duration_since(UNIX_EPOCH).ok()?;
    let millis = i64::try_from(duration.as_millis()).ok()?;
    unix_ms_to_iso(millis)
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use rusqlite::Connection;
    use serde_json::json;

    use crate::skills::AgentKind;

    use super::{
        SESSION_PREVIEW_MAX_CHARS, SessionRecord, SessionRepositoryResolver, SessionScanCache,
        SessionScanCacheEntry, clean_preview_text, clean_title, collect_recent_codex_paths,
        collect_tutti_run_session_paths, cursor_store_model, cursor_time_field,
        decode_cursor_project_dir, extract_cursor_blob_model, extract_parent_session_id,
        extract_session_title, file_state, git_repository_root, infer_session_project,
        merge_sessions, normalize_session_projects, scan_additional_session_roots,
        scan_codex_jsonl, scan_cursor_agent_transcripts, scan_cursor_meta, scan_jsonl_meta,
        session_watch_plan, should_replace_session_path, title_from_project_path, unix_ms_to_iso,
    };

    fn temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn git_repository_groups_linked_worktree_under_main_checkout() {
        let root = temp_dir("tendi-session-repository-test");
        let repo = root.join("repo");
        let linked = root.join("linked");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.email", "test@tendi.invalid"]);
        git(&repo, &["config", "user.name", "Tendi Test"]);
        fs::write(repo.join("README.md"), "seed\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "--quiet", "-m", "seed"]);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "feature",
                linked.to_str().unwrap(),
            ],
        );

        let expected = fs::canonicalize(&repo).unwrap();
        assert_eq!(
            git_repository_root(&repo).as_deref(),
            Some(expected.as_path())
        );
        assert_eq!(
            git_repository_root(&linked).as_deref(),
            Some(expected.as_path())
        );
        let mut resolver = SessionRepositoryResolver::default();
        assert_eq!(
            resolver.resolve(&repo).0.as_deref(),
            Some(expected.as_path())
        );
        assert_eq!(
            resolver.resolve(&linked).0.as_deref(),
            Some(expected.as_path())
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_repository_resolver_skips_non_git_and_caches_repository() {
        let root = temp_dir("tendi-session-repository-cache-test");
        let plain = root.join("plain/project");
        let plain_sibling = root.join("plain/sibling");
        let repo = root.join("repo");
        let alpha = repo.join("packages/alpha");
        let beta = repo.join("packages/beta");
        fs::create_dir_all(&plain).unwrap();
        fs::create_dir_all(&plain_sibling).unwrap();
        fs::create_dir_all(&alpha).unwrap();
        fs::create_dir_all(&beta).unwrap();
        git(&repo, &["init", "--quiet"]);
        git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/example/tendi-test.git",
            ],
        );

        let mut resolver = SessionRepositoryResolver::default();
        assert_eq!(resolver.resolve(&plain), (None, None));
        assert!(resolver.repositories.is_empty());
        let probes_after_plain = resolver.metadata_probes;
        assert_eq!(resolver.resolve(&plain_sibling), (None, None));
        assert_eq!(resolver.metadata_probes, probes_after_plain + 1);

        let alpha_result = resolver.resolve(&alpha);
        assert_eq!(resolver.resolve(&alpha), alpha_result);
        let probes_after_alpha = resolver.metadata_probes;
        let beta_result = resolver.resolve(&beta);
        assert_eq!(resolver.metadata_probes, probes_after_alpha + 1);
        assert_eq!(resolver.workspaces.len(), 4);
        assert_eq!(resolver.repositories.len(), 1);
        assert_eq!(alpha_result, beta_result);
        assert_eq!(alpha_result.0, Some(fs::canonicalize(&repo).unwrap()));
        assert_eq!(
            alpha_result.1.as_deref(),
            Some("https://github.com/example/tendi-test.git")
        );

        let nested_repo = repo.join("vendor/nested");
        let nested_workspace = nested_repo.join("src");
        fs::create_dir_all(&nested_workspace).unwrap();
        git(&nested_repo, &["init", "--quiet"]);
        let nested_result = resolver.resolve(&nested_workspace);
        assert_eq!(
            nested_result.0,
            Some(fs::canonicalize(&nested_repo).unwrap())
        );
        assert_ne!(nested_result, alpha_result);

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let alpha_link = root.join("alpha-link");
            symlink(&alpha, &alpha_link).unwrap();
            let probes_before_link = resolver.metadata_probes;
            assert_eq!(resolver.resolve(&alpha_link), alpha_result);
            assert_eq!(resolver.metadata_probes, probes_before_link);
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cursor_millis_timestamps_are_formatted_as_iso() {
        let value = json!({
            "createdAtMs": 1781621545175_i64,
            "updatedAtMs": 1781623096363_i64
        });

        assert_eq!(
            cursor_time_field(Some(&value), &["createdAt", "created_at"]),
            Some("2026-06-16T14:52:25.175Z".to_string())
        );
        assert_eq!(
            cursor_time_field(Some(&value), &["updatedAt", "updated_at"]),
            Some("2026-06-16T15:18:16.363Z".to_string())
        );
    }

    #[test]
    fn cursor_store_model_reads_last_used_model() {
        let value = json!({ "lastUsedModel": "composer-2.5" });

        assert_eq!(cursor_store_model(&value).as_deref(), Some("composer-2.5"));
    }

    #[test]
    fn cursor_blob_model_reads_provider_message_metadata() {
        let value = json!({
            "content": [{
                "providerOptions": {
                    "cursor": { "modelName": "claude-fable-5-thinking-high" }
                }
            }]
        });

        assert_eq!(
            extract_cursor_blob_model(&value).as_deref(),
            Some("claude-fable-5-thinking-high")
        );
    }

    #[test]
    fn cursor_project_folder_decodes_to_workspace_path() {
        assert_eq!(
            decode_cursor_project_dir("Users-test-dev-example-nextop").as_deref(),
            Some(Path::new("/Users/test/dev/example/nextop"))
        );
    }

    #[test]
    fn cursor_project_title_prefers_app_or_installation_name() {
        assert_eq!(
            title_from_project_path(Path::new("/Users/test/dev/example/group-chat/apps/server")),
            Some("group-chat".to_string())
        );
        assert_eq!(
            title_from_project_path(Path::new(
                "/Users/test/.tutti-dev/apps/installations/group-chat/abc/runtime"
            )),
            Some("group-chat".to_string())
        );
    }

    #[test]
    fn scans_additional_roots_by_transcript_format() {
        let root = temp_dir("tendi-additional-session-scan-test");
        let id = "5dcc0f66-1234-4f39-8e9d-123456789abc";
        let codex_session = root
            .join("private-run/state")
            .join(format!("rollout-{id}.jsonl"));
        let claude_session = root.join("private-run/claude-session.jsonl");
        let cursor_session = root.join("private-run/cursor-session.jsonl");
        fs::create_dir_all(codex_session.parent().unwrap()).unwrap();
        fs::write(
            &codex_session,
            concat!(
                r#"{"timestamp":"2026-07-30T10:00:00Z","type":"session_meta","payload":{"cwd":"/Users/test/dev/example-workspace","git":{"repository_url":"https://github.com/tutti-os/tutti.git"}}}"#,
                "\n",
                r#"{"timestamp":"2026-07-30T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Hello"}]}}"#
            ),
        )
        .unwrap();
        fs::write(
            &claude_session,
            r#"{"timestamp":"2026-07-30T10:00:00Z","type":"user","sessionId":"claude-session","message":{"role":"user","content":"Hello"}}"#,
        )
        .unwrap();
        fs::write(
            &cursor_session,
            r#"{"role":"user","message":{"content":"Hello"}}"#,
        )
        .unwrap();

        let mut sessions = Vec::new();
        scan_additional_session_roots(&[root.clone()], &mut sessions, None);

        assert_eq!(sessions.len(), 3);
        let codex = sessions
            .iter()
            .find(|session| session.agent == AgentKind::Codex)
            .unwrap();
        assert_eq!(codex.id, id);
        assert_eq!(
            codex.project.as_deref(),
            Some(Path::new("/Users/test/dev/example-workspace"))
        );
        assert_eq!(
            codex.repository_url.as_deref(),
            Some("https://github.com/tutti-os/tutti.git")
        );
        assert!(
            sessions
                .iter()
                .any(|session| session.agent == AgentKind::Claude)
        );
        assert!(
            sessions
                .iter()
                .any(|session| session.agent == AgentKind::Cursor)
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_codex_session_with_only_session_metadata() {
        let root = temp_dir("tendi-empty-codex-session-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-empty-session-id.jsonl");
        fs::write(
            &path,
            r#"{"timestamp":"2026-08-13T11:58:34.979Z","type":"session_meta","payload":{"cwd":"/Users/test/dev/tendi"}}"#,
        )
        .unwrap();

        let mut sessions = Vec::new();
        scan_codex_jsonl(&root, &mut sessions, None);

        assert!(sessions.is_empty());
        assert!(!scan_jsonl_meta(&path).has_content);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_empty_codex_session_is_rechecked_instead_of_reused() {
        let root = temp_dir("tendi-cached-empty-codex-session-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-empty-session-id.jsonl");
        fs::write(
            &path,
            r#"{"timestamp":"2026-08-13T11:58:34.979Z","type":"session_meta","payload":{"cwd":"/Users/test/dev/tendi"}}"#,
        )
        .unwrap();
        let (file_mtime, file_size) = file_state(&path).unwrap();
        let cache = SessionScanCache::from_entries([SessionScanCacheEntry {
            session: SessionRecord {
                id: "empty-session-id".to_string(),
                agent: AgentKind::Codex,
                title: None,
                project: None,
                repository: None,
                repository_url: None,
                logical_project_id: None,
                logical_project_name: None,
                path: path.clone(),
                started_at: None,
                updated_at: None,
                message_count: Some(1),
                first_user_message: None,
                last_user_message: None,
                last_assistant_message: None,
                turn_count: Some(0),
                model: None,
                mode: None,
                approval_mode: None,
                is_run_everything: None,
                parent_session_id: None,
                token_usage: None,
            },
            file_mtime,
            file_size,
        }]);

        let mut sessions = Vec::new();
        scan_codex_jsonl(&root, &mut sessions, Some(&cache));

        assert!(sessions.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tutti_run_discovery_only_reads_the_run_session_directory() {
        let root = temp_dir("tendi-tutti-run-discovery-test").join("agent/runs");
        let expected = root.join(
            "run-one/codex-home/sessions/2026/08/12/rollout-12345678-1234-1234-1234-123456789012.jsonl",
        );
        let fixture = root.join(
            "run-one/codex-home/.tmp/plugins/fixtures/rollout-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.jsonl",
        );
        fs::create_dir_all(expected.parent().unwrap()).unwrap();
        fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        fs::write(&expected, "{}\n").unwrap();
        fs::write(&fixture, "{}\n").unwrap();

        let mut paths = std::collections::BTreeSet::new();
        collect_tutti_run_session_paths(&root, &mut paths);

        assert_eq!(paths.into_iter().collect::<Vec<_>>(), vec![expected]);
        fs::remove_dir_all(root.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn tutti_watch_plan_only_recurses_into_session_directories() {
        let base = temp_dir("tendi-tutti-watch-plan-test");
        let root = base.join("agent/runs");
        let sessions = root.join("run-one/codex-home/sessions");
        let unrelated = root.join("run-one/workspace/node_modules");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(&unrelated).unwrap();

        let plan = session_watch_plan(&base, std::slice::from_ref(&root));

        assert!(plan.tutti_run_roots.contains(&root));
        assert!(
            plan.targets
                .iter()
                .any(|target| { target.path == root && !target.recursive })
        );
        assert!(
            plan.targets
                .iter()
                .any(|target| { target.path == sessions && target.recursive })
        );
        assert!(
            !plan
                .targets
                .iter()
                .any(|target| { target.path == unrelated })
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn cached_codex_transcript_reuses_unchanged_metadata_and_invalidates_on_append() {
        let root = temp_dir("tendi-session-scan-cache-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-12345678-1234-1234-1234-123456789012.jsonl");
        let initial = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"First question"}]}}"#;
        fs::write(&path, initial).unwrap();

        let mut first_scan = Vec::new();
        scan_codex_jsonl(&root, &mut first_scan, None);
        assert_eq!(first_scan.len(), 1);
        assert_eq!(first_scan[0].message_count, Some(1));

        let (file_mtime, file_size) = file_state(&path).unwrap();
        let cache = SessionScanCache::from_entries([SessionScanCacheEntry {
            session: first_scan[0].clone(),
            file_mtime,
            file_size,
        }]);

        let mut reused_scan = Vec::new();
        scan_codex_jsonl(&root, &mut reused_scan, Some(&cache));
        assert_eq!(reused_scan[0].message_count, Some(1));
        assert_eq!(reused_scan[0].title, first_scan[0].title);

        fs::write(
            &path,
            format!(
                "{initial}\n{}",
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Answer"}]}}"#
            ),
        )
        .unwrap();

        let mut invalidated_scan = Vec::new();
        scan_codex_jsonl(&root, &mut invalidated_scan, Some(&cache));
        assert_eq!(invalidated_scan[0].message_count, Some(2));

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn session_scan_cache_reuses_hard_linked_transcript_across_roots() {
        let root = temp_dir("tendi-session-hard-link-cache-test");
        let primary = root.join("primary/session.jsonl");
        let alias = root.join("alias/session.jsonl");
        fs::create_dir_all(primary.parent().unwrap()).unwrap();
        fs::create_dir_all(alias.parent().unwrap()).unwrap();
        fs::write(&primary, "{}\n").unwrap();
        fs::hard_link(&primary, &alias).unwrap();
        let (file_mtime, file_size) = file_state(&primary).unwrap();
        let cached_session = SessionRecord {
            id: "session-id".to_string(),
            agent: AgentKind::Codex,
            title: Some("Cached title".to_string()),
            project: None,
            repository: None,
            repository_url: None,
            logical_project_id: None,
            logical_project_name: None,
            path: primary.clone(),
            started_at: None,
            updated_at: None,
            message_count: Some(1),
            first_user_message: None,
            last_user_message: None,
            last_assistant_message: None,
            turn_count: Some(1),
            model: None,
            mode: None,
            approval_mode: None,
            is_run_everything: None,
            parent_session_id: None,
            token_usage: None,
        };
        let cache = SessionScanCache::from_entries([SessionScanCacheEntry {
            session: cached_session.clone(),
            file_mtime,
            file_size,
        }]);

        assert_eq!(
            cache.session_if_current(AgentKind::Codex, &alias),
            Some(cached_session)
        );
        assert_eq!(cache.agent_for_path(&alias), Some(AgentKind::Codex));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recent_codex_discovery_skips_days_before_watermark() {
        let root = temp_dir("tendi-recent-codex-sessions");
        for day in ["04", "05", "06"] {
            let dir = root.join("2026/08").join(day);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(format!("rollout-{day}.jsonl")), "{}\n").unwrap();
        }

        let mut candidates = BTreeMap::new();
        collect_recent_codex_paths(&root, (2026, 8, 5), UNIX_EPOCH, &mut candidates);

        let paths = candidates.keys().collect::<Vec<_>>();
        assert_eq!(paths.len(), 2);
        assert!(
            paths
                .iter()
                .all(|path| !path.to_string_lossy().contains("/04/"))
        );
        assert!(
            paths
                .iter()
                .any(|path| path.to_string_lossy().contains("/05/"))
        );
        assert!(
            paths
                .iter()
                .any(|path| path.to_string_lossy().contains("/06/"))
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_titles_from_user_messages() {
        let cursor = json!({
            "role": "user",
            "message": {
                "content": [{ "type": "text", "text": "<user_query>\nFix the install progress UI\n</user_query>" }]
            }
        });
        let codex = json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "sessions 页面修一下标题" }]
            }
        });

        assert_eq!(
            extract_session_title(&cursor),
            Some("Fix the install progress UI".to_string())
        );
        assert_eq!(
            extract_session_title(&codex),
            Some("sessions 页面修一下标题".to_string())
        );
        assert_eq!(
            clean_title(
                "<user_info>Ryan</user_info>\n<timestamp>today</timestamp>\n<user_query>\nUse the Cursor transcript title\n</user_query>"
            ),
            Some("Use the Cursor transcript title".to_string())
        );
        assert_eq!(
            clean_preview_text(
                "<environment_context>hidden</environment_context>\n<user_query>\nShow the session preview\n</user_query>"
            ),
            Some("Show the session preview".to_string())
        );
        let oversized_preview = "界".repeat(SESSION_PREVIEW_MAX_CHARS + 1);
        let bounded_preview = clean_preview_text(&oversized_preview).unwrap();
        assert_eq!(
            bounded_preview.chars().count(),
            SESSION_PREVIEW_MAX_CHARS + 1
        );
        assert!(bounded_preview.ends_with('…'));
        assert_eq!(clean_title("# AGENTS.md instructions\nhidden"), None);
    }

    #[test]
    fn counts_only_real_user_turns() {
        let root = temp_dir("tendi-session-turn-count-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session.jsonl");
        fs::write(
            &path,
            [
                r##"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions\nhidden"}]}}"##,
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"First question"}]}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"First answer"}]}}"#,
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"tool output"}]}}"#,
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"<user_query>\nSecond question\n</user_query>"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let meta = scan_jsonl_meta(&path);

        assert_eq!(meta.message_count, Some(5));
        assert_eq!(meta.turn_count, Some(2));
        assert_eq!(meta.first_user_message.as_deref(), Some("First question"));
        assert_eq!(meta.last_user_message.as_deref(), Some("Second question"));
        assert_eq!(meta.last_assistant_message.as_deref(), Some("First answer"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn metadata_fast_path_keeps_time_bounds_and_user_turns() {
        let root = temp_dir("tendi-session-metadata-fast-path-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session.jsonl");
        let tool_output = "x".repeat(super::METADATA_HINT_PREFIX_BYTES * 2);
        fs::write(
            &path,
            [
                format!(
                    r#"{{"timestamp":"2026-08-12T10:00:00Z","type":"response_item","payload":{{"type":"custom_tool_call_output","output":"{tool_output}"}}}}"#
                ),
                r#"{"timestamp":"2026-08-12T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Check CPU usage"}]}}"#.to_string(),
            ]
            .join("\n"),
        )
        .unwrap();

        let meta = scan_jsonl_meta(&path);

        assert_eq!(meta.started_at.as_deref(), Some("2026-08-12T10:00:00Z"));
        assert_eq!(meta.updated_at.as_deref(), Some("2026-08-12T10:00:01Z"));
        assert_eq!(meta.message_count, Some(2));
        assert_eq!(meta.turn_count, Some(1));
        assert_eq!(meta.title.as_deref(), Some("Check CPU usage"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_latest_session_model() {
        let root = temp_dir("tendi-session-model-test");
        fs::create_dir_all(&root).unwrap();
        let codex_path = root.join("codex.jsonl");
        fs::write(
            &codex_path,
            [
                r#"{"type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
                r#"{"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"model":"gpt-5.6-sol"}}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let claude_path = root.join("claude.jsonl");
        fs::write(
            &claude_path,
            r#"{"type":"assistant","message":{"model":"claude-opus-4-1"}}"#,
        )
        .unwrap();

        assert_eq!(
            scan_jsonl_meta(&codex_path).model.as_deref(),
            Some("gpt-5.6-sol")
        );
        assert_eq!(
            scan_jsonl_meta(&claude_path).model.as_deref(),
            Some("claude-opus-4-1")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_codex_parent_session_id() {
        let direct = json!({
            "type": "session_meta",
            "payload": { "parent_thread_id": "parent-direct" }
        });
        let nested = json!({
            "type": "session_meta",
            "payload": {
                "source": {
                    "subagent": {
                        "thread_spawn": { "parent_thread_id": "parent-nested" }
                    }
                }
            }
        });

        assert_eq!(
            extract_parent_session_id(&direct).as_deref(),
            Some("parent-direct")
        );
        assert_eq!(
            extract_parent_session_id(&nested).as_deref(),
            Some("parent-nested")
        );
    }

    #[test]
    fn cursor_meta_skips_project_only_empty_sessions() {
        let root = temp_dir("tendi-cursor-empty-meta-test");
        let session_dir = root.join("session-id");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("meta.json"),
            r#"{"schemaVersion":1,"cwd":"/Users/test/.tutti-dev/apps/installations/ai-slide/f18a5113a098dcd2/runtime"}"#,
        )
        .unwrap();

        let mut sessions = Vec::new();
        scan_cursor_meta(&root, &mut sessions, AgentKind::Cursor, None);

        assert!(sessions.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cursor_meta_keeps_explicit_title_without_messages() {
        let root = temp_dir("tendi-cursor-titled-meta-test");
        let session_dir = root.join("session-id");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("meta.json"),
            r#"{"schemaVersion":1,"name":"Pinned Cursor session","cwd":"/Users/test/dev/tendi"}"#,
        )
        .unwrap();

        let mut sessions = Vec::new();
        scan_cursor_meta(&root, &mut sessions, AgentKind::Cursor, None);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title.as_deref(), Some("Pinned Cursor session"));
        assert_eq!(sessions[0].message_count, Some(0));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cursor_store_metadata_links_subagent_transcript_to_parent() {
        let root = temp_dir("tendi-cursor-subagent-test");
        let chats_root = root.join("chats");
        let transcripts_root = root.join("projects");
        let child_id = "3207a1c6-bfb3-41e6-bf81-cb557da65f35";
        let parent_id = "f524c2b4-7841-43f7-b177-ccd35f440bc1";
        let child_dir = chats_root.join("workspace").join(child_id);
        let transcript_dir = transcripts_root
            .join("workspace/agent-transcripts")
            .join(child_id);
        fs::create_dir_all(&child_dir).unwrap();
        fs::create_dir_all(&transcript_dir).unwrap();

        let connection = Connection::open(child_dir.join("store.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);",
            )
            .unwrap();
        let metadata = json!({
            "agentId": child_id,
            "name": "New Agent",
            "createdAt": 1785420350441_i64,
            "mode": "default",
            "approvalMode": "unrestricted",
            "isRunEverything": false,
            "subagentInfo": {
                "parentAgentId": parent_id,
                "rootParentAgentId": parent_id,
                "toolCallId": "toolu_test"
            }
        })
        .to_string();
        let encoded = metadata
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        connection
            .execute("INSERT INTO meta (key, value) VALUES ('0', ?1)", [&encoded])
            .unwrap();
        let message_blob = json!({
            "role": "assistant",
            "content": [{
                "type": "reasoning",
                "providerOptions": {
                    "cursor": { "modelName": "claude-fable-5-thinking-high" }
                }
            }]
        })
        .to_string();
        connection
            .execute(
                "INSERT INTO blobs (id, data) VALUES ('message', ?1)",
                [message_blob.as_bytes()],
            )
            .unwrap();
        drop(connection);
        fs::write(
            transcript_dir.join(format!("{child_id}.jsonl")),
            r#"{"role":"user","message":{"content":[{"type":"text","text":"Investigate queue replay"}]}}"#,
        )
        .unwrap();

        let mut sessions = Vec::new();
        scan_cursor_meta(&chats_root, &mut sessions, AgentKind::Cursor, None);
        scan_cursor_agent_transcripts(&transcripts_root, &mut sessions, None);
        let sessions = merge_sessions(sessions);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, child_id);
        assert_eq!(
            sessions[0].title.as_deref(),
            Some("Investigate queue replay")
        );
        assert_eq!(sessions[0].parent_session_id.as_deref(), Some(parent_id));
        assert_eq!(
            sessions[0].model.as_deref(),
            Some("claude-fable-5-thinking-high")
        );
        assert_eq!(sessions[0].mode.as_deref(), Some("default"));
        assert_eq!(sessions[0].approval_mode.as_deref(), Some("unrestricted"));
        assert_eq!(sessions[0].is_run_everything, Some(false));
        assert_eq!(
            sessions[0].path,
            transcript_dir.join(format!("{child_id}.jsonl"))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cursor_transcript_path_replaces_metadata_path() {
        assert!(should_replace_session_path(
            Path::new("/Users/test/.cursor/chats/session-id/meta.json"),
            Path::new(
                "/Users/test/.cursor/projects/project/agent-transcripts/session-id/session-id.jsonl"
            )
        ));
        assert!(!should_replace_session_path(
            Path::new(
                "/Users/test/.cursor/projects/project/agent-transcripts/session-id/session-id.jsonl"
            ),
            Path::new("/Users/test/.cursor/chats/session-id/meta.json")
        ));
    }

    #[test]
    fn codex_transcript_project_fills_index_session() {
        let sessions = merge_sessions(vec![
            SessionRecord {
                id: "session-id".to_string(),
                agent: AgentKind::Codex,
                title: Some("Index title".to_string()),
                project: None,
                repository: None,
                repository_url: None,
                logical_project_id: None,
                logical_project_name: None,
                path: PathBuf::from("/Users/test/.codex/session_index.jsonl"),
                started_at: None,
                updated_at: Some("2026-06-23T14:54:04.092592Z".to_string()),
                message_count: None,
                first_user_message: None,
                last_user_message: None,
                last_assistant_message: None,
                turn_count: None,
                model: None,
                mode: None,
                approval_mode: None,
                is_run_everything: None,
                parent_session_id: None,
                token_usage: None,
            },
            SessionRecord {
                id: "session-id".to_string(),
                agent: AgentKind::Codex,
                title: None,
                project: Some(PathBuf::from("/Users/test/dev/tendi")),
                repository: None,
                repository_url: None,
                logical_project_id: None,
                logical_project_name: None,
                path: PathBuf::from(
                    "/Users/test/.codex/archived_sessions/rollout-session-id.jsonl",
                ),
                started_at: Some("2026-06-23T14:53:56.489Z".to_string()),
                updated_at: Some("2026-06-23T14:53:56.489Z".to_string()),
                message_count: Some(8),
                first_user_message: None,
                last_user_message: None,
                last_assistant_message: None,
                turn_count: Some(2),
                model: Some("gpt-5.6-sol".to_string()),
                mode: None,
                approval_mode: None,
                is_run_everything: None,
                parent_session_id: Some("parent-session".to_string()),
                token_usage: None,
            },
        ]);

        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].project.as_deref(),
            Some(Path::new("/Users/test/dev/tendi"))
        );
        assert_eq!(sessions[0].title.as_deref(), Some("Index title"));
        assert_eq!(sessions[0].message_count, Some(8));
        assert_eq!(sessions[0].turn_count, Some(2));
        assert_eq!(sessions[0].model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(
            sessions[0].parent_session_id.as_deref(),
            Some("parent-session")
        );
        assert_eq!(
            sessions[0].path,
            PathBuf::from("/Users/test/.codex/archived_sessions/rollout-session-id.jsonl")
        );
    }

    #[test]
    fn infers_session_project_from_codex_jsonl_cwd() {
        let root = temp_dir("tendi-session-project-infer-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-session-id.jsonl");
        fs::write(
            &path,
            r#"{"timestamp":"2026-06-22T11:21:33.250Z","type":"session_meta","payload":{"id":"session-id","cwd":"/Users/test/dev/_scripts"}}"#,
        )
        .unwrap();

        assert_eq!(
            infer_session_project(&path, AgentKind::Codex).as_deref(),
            Some(Path::new("/Users/test/dev/_scripts"))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn normalizes_ephemeral_chat_projects_without_touching_real_workspaces() {
        let mut sessions = vec![
            SessionRecord {
                id: "agent-chat".to_string(),
                agent: AgentKind::Claude,
                title: None,
                project: Some(PathBuf::from(
                    "/Users/test/Documents/tutti/session-c98d1ced-5371-43cc-8173-9416c349a776",
                )),
                repository: None,
                repository_url: None,
                logical_project_id: None,
                logical_project_name: None,
                path: PathBuf::from("/tmp/codex-chat.jsonl"),
                started_at: None,
                updated_at: None,
                message_count: None,
                first_user_message: None,
                last_user_message: None,
                last_assistant_message: None,
                turn_count: None,
                model: None,
                mode: None,
                approval_mode: None,
                is_run_everything: None,
                parent_session_id: None,
                token_usage: None,
            },
            SessionRecord {
                id: "real-workspace".to_string(),
                agent: AgentKind::Codex,
                title: None,
                project: Some(PathBuf::from("/Users/test/dev/tendi")),
                repository: None,
                repository_url: None,
                logical_project_id: None,
                logical_project_name: None,
                path: PathBuf::from("/tmp/real-workspace.jsonl"),
                started_at: None,
                updated_at: None,
                message_count: None,
                first_user_message: None,
                last_user_message: None,
                last_assistant_message: None,
                turn_count: None,
                model: None,
                mode: None,
                approval_mode: None,
                is_run_everything: None,
                parent_session_id: None,
                token_usage: None,
            },
        ];

        let mut archive = sessions[0].clone();
        archive.id = "codex-archive".to_string();
        archive.agent = AgentKind::Codex;
        archive.project = Some(PathBuf::from("/Users/test/Documents/Codex/2026-08-09/c"));
        archive.path = PathBuf::from("/tmp/codex-archive.jsonl");
        sessions.push(archive);
        normalize_session_projects(&mut sessions);

        assert_eq!(
            sessions[0].project.as_deref(),
            Some(Path::new("/Users/test/Documents/tutti"))
        );
        assert_eq!(
            sessions[1].project.as_deref(),
            Some(Path::new("/Users/test/dev/tendi"))
        );
        assert_eq!(
            sessions[2].project.as_deref(),
            Some(Path::new("/Users/test/Documents/Codex"))
        );
    }

    #[test]
    fn extracts_latest_codex_session_cache_usage() {
        let root = temp_dir("tendi-session-token-usage-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-session-id.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":40,\"output_tokens\":20,\"reasoning_output_tokens\":5,\"total_tokens\":120}}}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":250,\"cached_input_tokens\":175,\"output_tokens\":50,\"reasoning_output_tokens\":10,\"total_tokens\":300}}}}\n"
            ),
        )
        .unwrap();

        let usage = scan_jsonl_meta(&path).token_usage.unwrap();

        assert_eq!(usage.input_tokens, 250);
        assert_eq!(usage.cached_input_tokens, 175);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.reasoning_output_tokens, 10);
        assert_eq!(usage.total_tokens, 300);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_deduplicated_claude_session_usage() {
        let root = temp_dir("tendi-claude-token-usage-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session-id.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"assistant\",\"message\":{\"id\":\"msg-1\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"msg-1\",\"usage\":{\"input_tokens\":10,\"cache_creation_input_tokens\":20,\"cache_read_input_tokens\":30,\"output_tokens\":5}}}\n",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"msg-2\",\"usage\":{\"input_tokens\":5,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":95,\"output_tokens\":10}}}\n",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"msg-2\",\"usage\":{\"input_tokens\":5,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":95,\"output_tokens\":10}}}\n",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"msg-empty\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n"
            ),
        )
        .unwrap();

        let usage = scan_jsonl_meta(&path).token_usage.unwrap();

        assert_eq!(usage.input_tokens, 160);
        assert_eq!(usage.cached_input_tokens, 125);
        assert_eq!(usage.output_tokens, 15);
        assert_eq!(usage.reasoning_output_tokens, 0);
        assert_eq!(usage.total_tokens, 175);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unix_millis_to_iso_handles_utc_dates() {
        assert_eq!(
            unix_ms_to_iso(0),
            Some("1970-01-01T00:00:00.000Z".to_string())
        );
    }
}
