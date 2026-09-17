use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

use crate::{
    git,
    runtime_contract::{SessionKey, SourceLocator},
    skills::AgentKind,
    time::compare_timestamps,
};

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

impl SessionRecord {
    pub fn session_key(&self) -> Option<SessionKey> {
        SessionKey::new(self.agent, self.agent.label(), self.id.clone()).ok()
    }

    pub fn source_locator(&self) -> Option<SourceLocator> {
        SourceLocator::new(
            self.agent,
            self.path.display().to_string(),
            Some(self.id.clone()),
        )
        .ok()
    }
}

impl SessionIdentity {
    pub fn session_key(&self) -> Option<SessionKey> {
        SessionKey::new(self.agent, self.agent.label(), self.id.clone()).ok()
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
    pub additional_file_states: Vec<SessionScanSourceState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionScanSourceState {
    pub path: PathBuf,
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
            #[cfg(unix)]
            for source in &entry.additional_file_states {
                if let Some(identity) = session_file_identity(&source.path) {
                    cache
                        .entries_by_file
                        .insert((entry.session.agent, identity), entry.session.path.clone());
                }
            }
            cache
                .entries
                .insert((entry.session.agent, entry.session.path.clone()), entry);
        }
        cache
    }

    pub(crate) fn session_if_current(
        &self,
        agent: AgentKind,
        path: &Path,
    ) -> Option<SessionRecord> {
        let entry = self.entry_for_path(agent, path)?;
        let (file_mtime, file_size) = file_state(path)?;
        (entry.file_mtime == file_mtime
            && entry.file_size == file_size
            && self.additional_file_states_current(entry)
            && !session_requires_rescan(&entry.session))
        .then(|| entry.session.clone())
    }

    pub(crate) fn session_if_current_id(
        &self,
        agent: AgentKind,
        id: &str,
    ) -> Option<SessionRecord> {
        self.entries.values().find_map(|entry| {
            if entry.session.agent != agent || entry.session.id != id {
                return None;
            }
            let (file_mtime, file_size) = file_state(&entry.session.path)?;
            (entry.file_mtime == file_mtime
                && entry.file_size == file_size
                && self.additional_file_states_current(entry)
                && !session_requires_rescan(&entry.session))
            .then(|| entry.session.clone())
        })
    }

    pub(crate) fn session_if_appended(
        &self,
        agent: AgentKind,
        path: &Path,
    ) -> Option<(SessionRecord, u64)> {
        let logger = crate::logging::global();
        let Some(entry) = self.entry_for_path(agent, path) else {
            logger.debug(
                "session scan append cache decision",
                serde_json::json!({
                    "agent": agent.label(),
                    "path": path,
                    "eligible": false,
                    "reason": "no_cache_entry",
                }),
            );
            return None;
        };
        let Some((file_mtime, file_size)) = file_state(path) else {
            logger.debug(
                "session scan append cache decision",
                serde_json::json!({
                    "agent": agent.label(),
                    "path": path,
                    "eligible": false,
                    "reason": "file_state_unavailable",
                    "cachedMtime": entry.file_mtime,
                    "cachedSize": entry.file_size,
                }),
            );
            return None;
        };
        let Some(cached_size) = u64::try_from(entry.file_size).ok() else {
            logger.debug(
                "session scan append cache decision",
                serde_json::json!({
                    "agent": agent.label(),
                    "path": path,
                    "eligible": false,
                    "reason": "invalid_cached_size",
                    "cachedMtime": entry.file_mtime,
                    "cachedSize": entry.file_size,
                    "currentMtime": file_mtime,
                    "currentSize": file_size,
                }),
            );
            return None;
        };
        let current_size = u64::try_from(file_size).ok()?;
        let line_boundary = is_line_boundary(path, cached_size);
        let additional_files_current = self.additional_file_states_current(entry);
        let requires_rescan = session_requires_rescan(&entry.session);
        let reason = if current_size <= cached_size {
            "not_appended"
        } else if file_mtime < entry.file_mtime {
            "mtime_regressed"
        } else if !line_boundary {
            "cached_offset_not_on_line_boundary"
        } else if !additional_files_current {
            "additional_source_changed"
        } else if requires_rescan {
            "session_requires_rescan"
        } else {
            "eligible"
        };
        let fields = serde_json::json!({
            "agent": agent.label(),
            "path": path,
            "sessionId": entry.session.id,
            "eligible": reason == "eligible",
            "reason": reason,
            "cachedMtime": entry.file_mtime,
            "cachedSize": cached_size,
            "currentMtime": file_mtime,
            "currentSize": current_size,
            "lineBoundary": line_boundary,
            "additionalFilesCurrent": additional_files_current,
            "requiresRescan": requires_rescan,
        });
        if reason != "eligible" && current_size > cached_size {
            logger.info("session scan append cache rejected", fields);
        } else {
            logger.debug("session scan append cache decision", fields);
        }
        if reason != "eligible" {
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

    fn additional_file_states_current(&self, entry: &SessionScanCacheEntry) -> bool {
        entry.additional_file_states.iter().all(|source| {
            file_state(&source.path).unwrap_or((0, 0)) == (source.file_mtime, source.file_size)
        })
    }

    pub fn changed_sessions(&self, sessions: &[SessionRecord]) -> Vec<SessionRecord> {
        sessions
            .iter()
            .filter(|session| {
                self.entries
                    .get(&(session.agent, session.path.clone()))
                    .is_none_or(|entry| {
                        entry.session != **session
                            || file_state(&session.path).unwrap_or((0, 0))
                                != (entry.file_mtime, entry.file_size)
                            || !self.additional_file_states_current(entry)
                    })
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
    sessions.retain(|session| !is_index_path(session.agent, &session.path));
    normalize_session_projects(&mut sessions);
    enrich_session_repositories(&mut sessions);
    sessions.sort_by(|a, b| {
        compare_timestamps(b.updated_at.as_deref(), a.updated_at.as_deref())
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
    pub dynamic_roots: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWatchExpansion {
    pub run_dir: PathBuf,
    pub agent_home: PathBuf,
    pub session_root: PathBuf,
}

pub fn session_watch_plan(cwd: &Path, additional_session_roots: &[PathBuf]) -> SessionWatchPlan {
    let mut targets = Vec::new();
    let mut dynamic_roots = Vec::new();
    for root in session_watch_roots(cwd, additional_session_roots) {
        let mut handled = false;
        for provider in crate::providers::all_providers() {
            if let Some((provider_targets, is_nested_root)) = provider.session_watch_targets(&root)
            {
                handled = true;
                if is_nested_root {
                    dynamic_roots.push(root.clone());
                }
                targets.extend(provider_targets);
                break;
            }
        }
        if !handled {
            targets.push(SessionWatchTarget {
                recursive: root.is_dir(),
                path: root,
            });
        }
    }
    targets.sort_by(|left, right| left.path.cmp(&right.path));
    targets.dedup_by(|left, right| left.path == right.path);
    dynamic_roots.sort();
    dynamic_roots.dedup();
    SessionWatchPlan {
        targets,
        dynamic_roots,
    }
}

pub fn session_watch_expansion(
    dynamic_roots: &[PathBuf],
    event_path: &Path,
) -> Option<SessionWatchExpansion> {
    crate::providers::all_providers()
        .into_iter()
        .find_map(|provider| provider.session_watch_expansion(dynamic_roots, event_path))
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
    let mut candidates = BTreeMap::<PathBuf, SystemTime>::new();

    collect_recent_paths(root, since, &mut candidates);

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
        || crate::providers::all_providers()
            .into_iter()
            .any(|provider| provider.is_session_candidate_path(path))
}

pub fn scan_session_paths(paths: &[PathBuf], cache: &SessionScanCache) -> SessionScan {
    let mut sessions = Vec::new();
    let warnings = Vec::new();
    for path in paths.iter().filter(|path| path.is_file()) {
        let handled = crate::providers::all_providers()
            .into_iter()
            .any(|provider| provider.scan_explicit_session_path(path, &mut sessions, Some(cache)));
        if !handled
            && path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        {
            scan_detected_jsonl_session(path, &mut sessions, Some(cache));
        }
    }
    let mut sessions = merge_sessions(sessions);
    normalize_session_projects(&mut sessions);
    enrich_session_repositories(&mut sessions);
    sessions.sort_by(|left, right| {
        compare_timestamps(right.updated_at.as_deref(), left.updated_at.as_deref())
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
        if let Some(project) = session.project.take() {
            session.project = Some(
                crate::providers::agent_provider(session.agent).normalize_session_project(project),
            );
        }
    }
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
                git::local_repository_snapshot(&boundary, git::never_cancelled())
                    .map(repository_from_git_snapshot)
                    .unwrap_or((None, None))
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

fn repository_from_git_snapshot(
    snapshot: git::GitRepositorySnapshot,
) -> (Option<PathBuf>, Option<String>) {
    (git::logical_repository_root(&snapshot), snapshot.remote_url)
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
    for entry in WalkDir::new(root)
        .follow_links(false)
        .max_depth(10)
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
    let provider = crate::providers::agent_provider(agent);
    let project =
        if provider.session_path_role(path) == crate::providers::SessionPathRole::Transcript {
            let meta = scan_jsonl_meta_for_agent(path, Some(agent));
            meta.project
        } else if provider.session_path_role(path) == crate::providers::SessionPathRole::Metadata {
            fs::read_to_string(path)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|value| provider.infer_meta_project(&value))
        } else {
            None
        };

    provider
        .infer_session_project(path, project)
        .filter(|path| path.is_absolute())
}

pub fn infer_session_resume_target(path: &Path, agent: AgentKind) -> Option<&'static str> {
    if !is_transcript_path(agent, path) {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().map_while(Result::ok).take(16) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(target) =
            crate::providers::agent_provider(agent).resume_target_from_transcript_value(&value)
        {
            return Some(target);
        }
    }
    None
}

pub(crate) fn scan_jsonl_sessions(
    root: &Path,
    agent: AgentKind,
    max_depth: usize,
    sessions: &mut Vec<SessionRecord>,
    cache: Option<&SessionScanCache>,
) {
    if !root.is_dir() {
        return;
    }

    for entry in WalkDir::new(root)
        .follow_links(true)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().is_some_and(|ext| ext == "jsonl")
        })
    {
        let path = entry.into_path();
        let provider = crate::providers::agent_provider(agent);
        if let Some(session) = cache
            .and_then(|cache| cache.session_if_current(agent, &path))
            .filter(session_is_known_non_empty)
        {
            sessions.push(session);
            continue;
        }
        if let Some((session, offset)) =
            cache.and_then(|cache| cache.session_if_appended(agent, &path))
        {
            if let Some(session) = scan_jsonl_meta_from_offset(&path, offset, session) {
                sessions.push(session);
                continue;
            }
        }
        let meta = scan_jsonl_meta_for_agent(&path, Some(agent));
        if !meta.has_content {
            continue;
        }
        let Some(id) = provider.session_id_from_path(&path) else {
            continue;
        };
        sessions.push(SessionRecord {
            id,
            agent,
            title: meta.title,
            project: provider.infer_session_project(&path, meta.project),
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
        if crate::providers::all_providers()
            .into_iter()
            .any(|provider| provider.collect_additional_session_paths(root, &mut session_paths))
        {
            continue;
        }
        for entry in WalkDir::new(root)
            .follow_links(false)
            .max_depth(10)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file() && is_session_candidate_path(entry.path()))
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
        } else {
            for provider in crate::providers::all_providers() {
                if provider.scan_explicit_session_path(&path, sessions, cache) {
                    break;
                }
            }
        }
    }
}

fn scan_detected_jsonl_session(
    path: &Path,
    sessions: &mut Vec<SessionRecord>,
    cache: Option<&SessionScanCache>,
) {
    let logger = crate::logging::global();
    let Some(agent) = cache
        .and_then(|cache| cache.agent_for_path(path))
        .or_else(|| detect_jsonl_agent(path))
    else {
        logger.debug(
            "session scan skipped unrecognized jsonl",
            serde_json::json!({ "path": path }),
        );
        return;
    };
    if let Some(session) = cache
        .and_then(|cache| cache.session_if_current(agent, path))
        .filter(session_is_known_non_empty)
    {
        logger.debug(
            "session scan cache hit",
            serde_json::json!({
                "agent": agent.label(),
                "path": path,
                "sessionId": &session.id,
                "strategy": "cached",
                "messageCount": session.message_count,
                "assistantLastPresent": session
                    .last_assistant_message
                    .as_ref()
                    .is_some_and(|message| !message.is_empty()),
            }),
        );
        sessions.push(session);
        return;
    }
    if crate::providers::agent_provider(agent).session_supports_append_cache() {
        if let Some((session, offset)) =
            cache.and_then(|cache| cache.session_if_appended(agent, path))
        {
            let session_id = session.id.clone();
            logger.info(
                "session scan append started",
                serde_json::json!({
                    "agent": agent.label(),
                    "path": path,
                    "sessionId": &session_id,
                    "offset": offset,
                }),
            );
            if let Some(session) = scan_jsonl_meta_from_offset(path, offset, session) {
                logger.info(
                    "session scan append completed",
                    serde_json::json!({
                        "agent": agent.label(),
                        "path": path,
                        "sessionId": &session.id,
                        "messageCount": session.message_count,
                        "assistantLastPresent": session
                            .last_assistant_message
                            .as_ref()
                            .is_some_and(|message| !message.is_empty()),
                    }),
                );
                sessions.push(session);
                return;
            }
            logger.warn(
                "session scan append produced no session",
                serde_json::json!({
                    "agent": agent.label(),
                    "path": path,
                    "sessionId": &session_id,
                    "offset": offset,
                }),
            );
        }
    }
    let meta = scan_jsonl_meta_for_agent(path, Some(agent));
    if !meta.has_content {
        return;
    }
    let file_updated_at = file_modified_iso(path);
    let provider = crate::providers::agent_provider(agent);
    let Some(id) = provider.session_id_from_path(path) else {
        return;
    };
    let project = provider.infer_session_project(path, meta.project);
    let session = SessionRecord {
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
    };
    logger.debug(
        "session scan full parse completed",
        serde_json::json!({
            "agent": agent.label(),
            "path": path,
            "sessionId": &session.id,
            "messageCount": session.message_count,
            "assistantLastPresent": session
                .last_assistant_message
                .as_ref()
                .is_some_and(|message| !message.is_empty()),
        }),
    );
    sessions.push(session);
}

fn detect_jsonl_agent(path: &Path) -> Option<AgentKind> {
    let file = fs::File::open(path).ok()?;
    let mut candidates = BTreeSet::new();
    for line in BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .take(64)
    {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        for provider in crate::providers::all_providers() {
            if provider.recognizes_transcript(&value) {
                candidates.insert(provider.kind());
            }
        }
        if candidates.len() > 1 {
            return None;
        }
    }
    return candidates.into_iter().next();
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
    let transcript_title_overrides_index = incoming.title.is_some()
        && is_index_path(existing.agent, &existing.path)
        && is_transcript_path(existing.agent, &incoming.path)
        && crate::providers::agent_provider(existing.agent)
            .session_transcript_title_overrides_index();
    if existing.title.is_none() || transcript_title_overrides_index {
        existing.title = incoming.title.clone();
    }
    if existing.project.is_none() {
        existing.project = incoming.project.clone();
    }
    if let Some(started_at) = &incoming.started_at {
        if existing
            .started_at
            .as_deref()
            .is_none_or(|current| compare_timestamps(Some(started_at), Some(current)).is_lt())
        {
            existing.started_at = Some(started_at.clone());
        }
    }
    if existing.updated_at.is_none()
        || compare_timestamps(
            incoming.updated_at.as_deref(),
            existing.updated_at.as_deref(),
        )
        .is_gt()
    {
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
    if should_replace_session_path(existing.agent, &existing.path, &incoming.path) {
        existing.path = incoming.path.clone();
    }
}

fn should_replace_session_path(agent: AgentKind, existing: &Path, incoming: &Path) -> bool {
    if is_transcript_path(agent, incoming) && !is_transcript_path(agent, existing) {
        return true;
    }

    is_index_path(agent, existing) && !is_metadata_path(agent, incoming)
}

fn is_transcript_path(agent: AgentKind, path: &Path) -> bool {
    crate::providers::agent_provider(agent).session_path_role(path)
        == crate::providers::SessionPathRole::Transcript
}

fn is_metadata_path(agent: AgentKind, path: &Path) -> bool {
    matches!(
        crate::providers::agent_provider(agent).session_path_role(path),
        crate::providers::SessionPathRole::Metadata | crate::providers::SessionPathRole::Index
    )
}

fn is_index_path(agent: AgentKind, path: &Path) -> bool {
    crate::providers::agent_provider(agent).session_path_role(path)
        == crate::providers::SessionPathRole::Index
}

#[derive(Default)]
pub(crate) struct SessionMetadata {
    pub(crate) has_content: bool,
    pub(crate) message_count: Option<usize>,
    pub(crate) first_user_message: Option<String>,
    pub(crate) last_user_message: Option<String>,
    pub(crate) last_assistant_message: Option<String>,
    pub(crate) turn_count: Option<usize>,
    pub(crate) started_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) project: Option<PathBuf>,
    pub(crate) repository_url: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) title_candidates: Vec<String>,
    pub(crate) provider_title: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) parent_session_id: Option<String>,
    pub(crate) token_usage: Option<SessionTokenUsage>,
}

pub(crate) fn scan_jsonl_meta_for_agent(path: &Path, agent: Option<AgentKind>) -> SessionMetadata {
    let Ok(file) = fs::File::open(path) else {
        return SessionMetadata::default();
    };
    let mut meta = SessionMetadata {
        has_content: false,
        message_count: Some(0),
        turn_count: Some(0),
        ..Default::default()
    };
    let mut deduplicated_usage = BTreeMap::new();
    let provider = agent.map(crate::providers::agent_provider);
    let inherited_history_start_ordinal = agent.and_then(|agent| {
        crate::transcript::transcript_inherited_history_start_ordinal(path, agent)
            .ok()
            .flatten()
    });

    scan_jsonl_meta_lines(
        BufReader::new(file).lines().map_while(Result::ok),
        &mut meta,
        &mut deduplicated_usage,
        inherited_history_start_ordinal,
        provider,
    );
    finalize_jsonl_title(&mut meta);

    if meta.token_usage.is_none() && !deduplicated_usage.is_empty() {
        meta.token_usage = sum_token_usage(deduplicated_usage.values());
    }

    meta
}

pub(crate) fn scan_jsonl_metadata(path: &Path, agent: AgentKind) -> SessionMetadata {
    scan_jsonl_meta_for_agent(path, Some(agent))
}

pub(crate) fn scan_jsonl_meta_from_offset(
    path: &Path,
    offset: u64,
    session: SessionRecord,
) -> Option<SessionRecord> {
    let mut file = fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;

    let mut meta = SessionMetadata {
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
        title_candidates: Vec::new(),
        provider_title: None,
        model: session.model,
        parent_session_id: session.parent_session_id,
        token_usage: session.token_usage,
    };
    let mut deduplicated_usage = BTreeMap::new();
    let provider = Some(crate::providers::agent_provider(session.agent));
    let inherited_history_start_ordinal =
        crate::transcript::transcript_inherited_history_start_ordinal(path, session.agent)
            .ok()
            .flatten();
    scan_jsonl_meta_lines(
        BufReader::new(file).lines().map_while(Result::ok),
        &mut meta,
        &mut deduplicated_usage,
        inherited_history_start_ordinal,
        provider,
    );
    finalize_jsonl_title(&mut meta);

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
    meta: &mut SessionMetadata,
    deduplicated_usage: &mut BTreeMap<String, SessionTokenUsage>,
    inherited_history_start_ordinal: Option<u64>,
    provider: Option<&dyn crate::providers::AgentProvider>,
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
        let line_has_content = provider
            .and_then(|provider| provider.session_line_has_content(prefix))
            .unwrap_or_else(|| fallback_line_has_session_content(prefix));
        meta.has_content |= line_has_content;
        if !provider
            .and_then(|provider| provider.session_line_requires_metadata_parse(prefix, meta))
            .unwrap_or_else(|| {
                provider.is_none() || fallback_line_requires_metadata_parse(prefix, meta)
            })
        {
            if let Some(timestamp) = json_string_field(prefix, "\"timestamp\"") {
                apply_time_bounds(meta, timestamp);
            }
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(provider) = provider {
            provider.update_session_metadata(&value, meta, deduplicated_usage);
        } else {
            for provider in crate::providers::all_providers() {
                if provider.recognizes_transcript(&value) {
                    provider.update_session_metadata(&value, meta, deduplicated_usage);
                }
            }
        }
        let user_message = provider.and_then(|provider| provider.session_user_message(&value));
        let message = provider
            .map(|provider| extract_session_message_for_agent(provider.kind(), &value))
            .unwrap_or_else(|| extract_session_message(&value));
        meta.has_content |= user_message.is_some() || message.is_some();
        if crate::transcript::is_inherited_transcript_value(&value, inherited_history_start_ordinal)
        {
            continue;
        }
        if let Some(body) = user_message {
            record_user_session_metadata(meta, &body);
        }
        if let Some((role, body)) = message {
            match role {
                "user" => record_user_session_metadata(meta, &body),
                "assistant" => {
                    if let Some(body) = clean_preview_text(&body) {
                        meta.last_assistant_message = Some(body);
                    }
                }
                _ => {}
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
    }
}

fn record_user_session_metadata(meta: &mut SessionMetadata, body: &str) {
    if let Some(title) = clean_title(body) {
        meta.turn_count = meta.turn_count.map(|count| count + 1);
        meta.title_candidates.push(title);
    }
    if let Some(body) = clean_preview_text(body) {
        if meta.first_user_message.is_none() {
            meta.first_user_message = Some(body.clone());
        }
        meta.last_user_message = Some(body);
    }
}

fn finalize_jsonl_title(meta: &mut SessionMetadata) {
    if meta.title.is_some() {
        return;
    }
    if meta.parent_session_id.is_some() {
        if let Some(provider_title) = meta.provider_title.clone() {
            meta.title = Some(provider_title);
            return;
        }
    }
    meta.title = meta.title_candidates.first().cloned();
}

const METADATA_HINT_PREFIX_BYTES: usize = 16 * 1024;

fn metadata_hint_prefix(line: &str) -> &str {
    let mut prefix_end = line.len().min(METADATA_HINT_PREFIX_BYTES);
    while !line.is_char_boundary(prefix_end) {
        prefix_end -= 1;
    }
    &line[..prefix_end]
}

fn fallback_line_requires_metadata_parse(prefix: &str, meta: &SessionMetadata) -> bool {
    match json_string_field(prefix, "\"type\"") {
        Some("assistant" | "user") => true,
        _ => {
            line_has_message_role(prefix)
                || (meta.project.is_none() && prefix.contains("\"cwd\""))
                || (meta.repository_url.is_none() && prefix.contains("\"repository_url\""))
        }
    }
}

fn fallback_line_has_session_content(prefix: &str) -> bool {
    matches!(
        json_string_field(prefix, "\"role\""),
        Some("user" | "assistant")
    )
}

pub(crate) fn line_contains_json_string_value(line: &str, expected: &str) -> bool {
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

pub(crate) fn session_is_known_non_empty(session: &SessionRecord) -> bool {
    session.title.is_some()
        || session.turn_count.is_some_and(|count| count > 0)
        || session.token_usage.is_some()
}

fn line_has_user_role(prefix: &str) -> bool {
    prefix.contains("\"role\"") && prefix.contains("\"user\"")
}

pub(crate) fn line_has_message_role(prefix: &str) -> bool {
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

pub(crate) fn json_string_field<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
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

pub(crate) fn apply_time_bounds(meta: &mut SessionMetadata, timestamp: &str) {
    if meta
        .started_at
        .as_deref()
        .is_none_or(|current| compare_timestamps(Some(timestamp), Some(current)).is_lt())
    {
        meta.started_at = Some(timestamp.to_string());
    }
    if meta
        .updated_at
        .as_deref()
        .is_none_or(|current| compare_timestamps(Some(timestamp), Some(current)).is_gt())
    {
        meta.updated_at = Some(timestamp.to_string());
    }
}

pub(crate) fn extract_session_title_for_agent(agent: AgentKind, value: &Value) -> Option<String> {
    if crate::providers::agent_provider(agent).session_message_kind(value)
        != Some(crate::providers::SessionMessageKind::User)
    {
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

fn infer_session_value_agent(value: &Value) -> AgentKind {
    if value.get("type").and_then(Value::as_str) == Some("response_item") {
        AgentKind::Codex
    } else {
        AgentKind::Unknown
    }
}

pub(crate) fn extract_session_message(value: &Value) -> Option<(&'static str, String)> {
    extract_session_message_for_agent(infer_session_value_agent(value), value)
}

pub(crate) fn extract_session_message_for_agent(
    agent: AgentKind,
    value: &Value,
) -> Option<(&'static str, String)> {
    let role = match crate::providers::agent_provider(agent).session_message_kind(value)? {
        crate::providers::SessionMessageKind::User => "user",
        crate::providers::SessionMessageKind::Assistant => "assistant",
        crate::providers::SessionMessageKind::Context => return None,
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
    value.and_then(|value| {
        if contains_internal_context(&value) || contains_image_marker(&value) {
            clean_preview_text(&value)
        } else {
            bound_session_preview_text(&value)
        }
    })
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

const INTERNAL_CONTEXT_MARKERS: [(&str, Option<&str>); 18] = [
    ("# AGENTS.md instructions", Some("</INSTRUCTIONS>")),
    ("<local-command-caveat>", Some("</local-command-caveat>")),
    ("<command-name>", Some("</command-name>")),
    ("<local-command-stdout>", Some("</local-command-stdout>")),
    ("<task-notification>", Some("</task-notification>")),
    ("<recommended_plugins>", Some("</recommended_plugins>")),
    ("<environment_context>", Some("</environment_context>")),
    (
        "<permissions instructions>",
        Some("</permissions instructions>"),
    ),
    ("<app-context>", Some("</app-context>")),
    ("<collaboration_mode>", Some("</collaboration_mode>")),
    ("<skills_instructions>", Some("</skills_instructions>")),
    ("<plugins_instructions>", Some("</plugins_instructions>")),
    ("<system-reminder>", Some("</system-reminder>")),
    (
        "<available_subagent_types>",
        Some("</available_subagent_types>"),
    ),
    ("<user_instructions>", Some("</user_instructions>")),
    ("<subagent_notification>", Some("</subagent_notification>")),
    ("<turn_aborted>", Some("</turn_aborted>")),
    ("<in-app-browser-context", Some("</in-app-browser-context>")),
];

fn find_internal_context_marker(
    text: &str,
    offset: usize,
) -> Option<(usize, &'static str, Option<&'static str>)> {
    let provider_markers = crate::providers::all_providers()
        .into_iter()
        .flat_map(|provider| {
            provider
                .transcript_internal_context_markers()
                .iter()
                .map(|(prefix, _, closing)| (*prefix, *closing))
        })
        .collect::<Vec<_>>();
    INTERNAL_CONTEXT_MARKERS
        .iter()
        .copied()
        .chain(provider_markers)
        .filter_map(|(prefix, closing)| {
            let mut search_from = offset;
            while let Some(relative_start) = text[search_from..].find(prefix) {
                let start = search_from + relative_start;
                if start == 0 || text.as_bytes().get(start.wrapping_sub(1)) == Some(&b'\n') {
                    return Some((start, prefix, closing));
                }
                search_from = start + prefix.len();
            }
            None
        })
        .min_by_key(|(start, _, _)| *start)
}

fn split_internal_context_segments(text: &str) -> Vec<(bool, String)> {
    let mut segments = Vec::new();
    let mut cursor = 0;
    while let Some((start, prefix, closing)) = find_internal_context_marker(text, cursor) {
        if start > cursor {
            segments.push((false, text[cursor..start].to_string()));
        }
        let block_end = closing
            .and_then(|closing| {
                text[start..]
                    .find(closing)
                    .map(|offset| start + offset + closing.len())
            })
            .or_else(|| {
                find_internal_context_marker(text, start + prefix.len())
                    .map(|(next_start, _, _)| next_start)
            })
            .unwrap_or(text.len());
        if block_end <= start {
            break;
        }
        segments.push((true, text[start..block_end].trim().to_string()));
        cursor = block_end;
    }
    if cursor < text.len() {
        segments.push((false, text[cursor..].to_string()));
    }
    if segments.is_empty() && !text.trim().is_empty() {
        segments.push((false, text.trim().to_string()));
    }
    segments
}

fn contains_internal_context(text: &str) -> bool {
    split_internal_context_segments(text)
        .into_iter()
        .any(|(is_context, _)| is_context)
}

fn contains_image_marker(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("<image") || text.contains("![")
}

fn clean_user_content_part(text: &str) -> Option<String> {
    let mut text = split_internal_context_segments(text)
        .into_iter()
        .filter_map(|(is_context, segment)| (!is_context).then_some(segment))
        .collect::<Vec<_>>()
        .join("\n");
    text = text.trim().to_string();
    if let Some(inner) = extract_tag_body(&text, "user_query") {
        text = inner.trim().to_string();
    }
    if text.is_empty() {
        return None;
    }
    Some(text.to_string())
}

pub(crate) fn clean_preview_text(text: &str) -> Option<String> {
    let text = clean_user_content_part(text)?;
    let text = strip_image_markers(&text);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    bound_session_preview_text(&text)
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
                .filter_map(clean_user_content_part)
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then_some(text)
        }
        value => extract_text(Some(value)).and_then(|text| clean_user_content_part(&text)),
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

pub(crate) fn clean_title(text: &str) -> Option<String> {
    let text = clean_user_content_part(text)?;
    let text = strip_image_markers(&text);
    let title = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("---"));
    title.map(|title| title.chars().take(96).collect())
}

pub(crate) fn clean_session_title(value: Option<String>) -> Option<String> {
    value.and_then(|value| clean_title(&value))
}

fn strip_image_markers(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut cursor = 0;
    let mut scan_from = 0;
    let mut stripped = String::with_capacity(text.len());
    while let Some((start, is_tag)) = next_image_marker(&lower, scan_from) {
        let Some(end) = image_marker_end(text, start, is_tag) else {
            scan_from = start + 2;
            continue;
        };
        stripped.push_str(&text[cursor..start]);
        cursor = end;
        scan_from = end;
    }

    stripped.push_str(&text[cursor..]);
    stripped
}

fn next_image_marker(text: &str, offset: usize) -> Option<(usize, bool)> {
    let open_tag = text[offset..]
        .find("<image")
        .map(|relative| offset + relative);
    let open_tag = open_tag.filter(|start| {
        text.as_bytes()
            .get(start + "<image".len())
            .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
    });
    let close_tag = text[offset..]
        .find("</image")
        .map(|relative| offset + relative);
    let close_tag = close_tag.filter(|start| {
        text.as_bytes()
            .get(start + "</image".len())
            .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
    });
    let markdown = text[offset..].find("![").map(|relative| offset + relative);

    let tag = [open_tag, close_tag].into_iter().flatten().min();
    match (tag, markdown) {
        (Some(tag), Some(markdown)) if tag < markdown => Some((tag, true)),
        (Some(tag), Some(_)) => Some((tag, true)),
        (Some(tag), None) => Some((tag, true)),
        (None, Some(markdown)) => Some((markdown, false)),
        (None, None) => None,
    }
}

fn image_marker_end(text: &str, start: usize, is_tag: bool) -> Option<usize> {
    if is_tag {
        return Some(
            text[start..]
                .find('>')
                .map(|relative| start + relative + 1)
                .unwrap_or(text.len()),
        );
    }
    let url_start = start + text[start..].find("](")? + 2;
    text[url_start..]
        .find(')')
        .map(|relative| url_start + relative + 1)
}

fn session_preview_requires_rescan(session: &SessionRecord) -> bool {
    [
        session.title.as_deref(),
        session.first_user_message.as_deref(),
        session.last_user_message.as_deref(),
        session.last_assistant_message.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|text| contains_internal_context(text) || contains_image_marker(text))
}

fn session_requires_rescan(session: &SessionRecord) -> bool {
    if session_preview_requires_rescan(session) {
        return true;
    }
    if let Some(result) =
        crate::providers::agent_provider(session.agent).session_requires_rescan(session)
    {
        return result;
    }
    generic_session_title_requires_rescan(session)
}

fn generic_session_title_requires_rescan(session: &SessionRecord) -> bool {
    let Some(title) = session.title.as_deref() else {
        return session.parent_session_id.is_some()
            || session.turn_count.is_some_and(|count| count > 0);
    };
    if session.parent_session_id.is_none() {
        return false;
    }
    session
        .first_user_message
        .as_deref()
        .and_then(|message| clean_title(message))
        .is_some_and(|first_title| first_title != title)
}

fn extract_tag_body<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = text.find(&start_tag)? + start_tag.len();
    let end = text[start..].find(&end_tag)? + start;
    let inner = text[start..end].trim();
    (!inner.is_empty()).then_some(inner)
}

pub(crate) fn string_field(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).and_then(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    })
}

pub(crate) fn unix_ms_to_iso(value: i64) -> Option<String> {
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

pub(crate) fn file_modified_iso(path: &Path) -> Option<String> {
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
#[path = "sessions_tests.rs"]
mod tests;
