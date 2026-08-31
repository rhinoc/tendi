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

    pub(crate) fn session_if_current(
        &self,
        agent: AgentKind,
        path: &Path,
    ) -> Option<SessionRecord> {
        let entry = self.entry_for_path(agent, path)?;
        let (file_mtime, file_size) = file_state(path)?;
        (entry.file_mtime == file_mtime
            && entry.file_size == file_size
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
                && !session_requires_rescan(&entry.session))
            .then(|| entry.session.clone())
        })
    }

    pub(crate) fn session_if_appended(
        &self,
        agent: AgentKind,
        path: &Path,
    ) -> Option<(SessionRecord, u64)> {
        let entry = self.entry_for_path(agent, path)?;
        let (file_mtime, file_size) = file_state(path)?;
        let cached_size = u64::try_from(entry.file_size).ok()?;
        let current_size = u64::try_from(file_size).ok()?;
        if current_size <= cached_size
            || file_mtime < entry.file_mtime
            || !is_line_boundary(path, cached_size)
            || session_requires_rescan(&entry.session)
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
    if crate::providers::agent_provider(agent).session_supports_append_cache() {
        if let Some((session, offset)) =
            cache.and_then(|cache| cache.session_if_appended(agent, path))
        {
            if let Some(session) = scan_jsonl_meta_from_offset(path, offset, session) {
                sessions.push(session);
                return;
            }
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
    if existing.title.is_none() {
        existing.title = incoming.title.clone();
    }
    if existing.project.is_none() {
        existing.project = incoming.project.clone();
    }
    if let Some(started_at) = &incoming.started_at {
        if existing.started_at.as_deref().is_none_or(|current| {
            compare_timestamps(Some(started_at), Some(current)).is_lt()
        })
        {
            existing.started_at = Some(started_at.clone());
        }
    }
    if existing.updated_at.is_none()
        || compare_timestamps(incoming.updated_at.as_deref(), existing.updated_at.as_deref())
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
        meta.has_content |= provider
            .and_then(|provider| provider.session_line_has_content(prefix))
            .unwrap_or_else(|| fallback_line_has_session_content(prefix));
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
        if crate::transcript::is_inherited_transcript_value(&value, inherited_history_start_ordinal)
        {
            continue;
        }
        if let Some(title) = extract_session_title(&value) {
            meta.turn_count = meta.turn_count.map(|count| count + 1);
            meta.title_candidates.push(title);
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
    if meta.started_at.as_deref().is_none_or(|current| {
        compare_timestamps(Some(timestamp), Some(current)).is_lt()
    }) {
        meta.started_at = Some(timestamp.to_string());
    }
    if meta.updated_at.as_deref().is_none_or(|current| {
        compare_timestamps(Some(timestamp), Some(current)).is_gt()
    }) {
        meta.updated_at = Some(timestamp.to_string());
    }
}

pub(crate) fn extract_session_title(value: &Value) -> Option<String> {
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

pub(crate) fn extract_session_message(value: &Value) -> Option<(&'static str, String)> {
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
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use rusqlite::Connection;
    use serde_json::json;

    use crate::{
        git,
        providers::{codex::scan_session_index as scan_codex_index, cursor_sessions},
        skills::AgentKind,
    };
    use crate::providers::codex::scan_jsonl_sessions_for_test as scan_codex_jsonl;

    use super::{
        SESSION_PREVIEW_MAX_CHARS, SessionRecord, SessionRepositoryResolver, SessionScanCache,
        SessionScanCacheEntry, clean_preview_text, clean_title, extract_session_title, file_state,
        compare_timestamps, infer_session_project, infer_session_resume_target,
        is_session_candidate_path,
        merge_sessions, normalize_session_projects,
        repository_from_git_snapshot, scan_additional_session_roots, scan_jsonl_meta_for_agent,
        scan_jsonl_sessions, session_requires_rescan,
        session_watch_plan, should_replace_session_path,
    };

    use cursor_sessions::{
        decode_cursor_project_dir, scan_cursor_meta,
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

    #[test]
    fn compares_session_timestamps_by_instant_across_offsets() {
        assert_eq!(
            compare_timestamps(
                Some("2026-08-28T11:49:00+08:00"),
                Some("2026-08-28T03:57:03.099Z"),
            ),
            std::cmp::Ordering::Less,
        );
        assert_eq!(
            compare_timestamps(
                Some("2026-08-28T03:57:03.099Z"),
                Some("2026-08-28T11:49:00+08:00"),
            ),
            std::cmp::Ordering::Greater,
        );
    }

    #[test]
    fn infers_session_resume_target_from_codex_session_meta() {
        let root = temp_dir("tendi-session-resume-target");
        fs::create_dir_all(&root).unwrap();
        let terminal = root.join("terminal.jsonl");
        fs::write(
            &terminal,
            r#"{"type":"session_meta","source":"cli","originator":"codex-tui"}"#,
        )
        .unwrap();
        assert_eq!(
            infer_session_resume_target(&terminal, AgentKind::Codex),
            Some("terminal")
        );

        let app = root.join("app.jsonl");
        fs::write(
            &app,
            r#"{"type":"session_meta","source":"vscode","originator":"Codex Desktop"}"#,
        )
        .unwrap();
        assert_eq!(
            infer_session_resume_target(&app, AgentKind::Codex),
            Some("app")
        );

        let unknown = root.join("unknown.jsonl");
        fs::write(&unknown, r#"{"type":"session_meta","source":"other"}"#).unwrap();
        assert_eq!(
            infer_session_resume_target(&unknown, AgentKind::Codex),
            None
        );
        fs::remove_dir_all(root).unwrap();
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
            repository_from_git_snapshot(
                git::local_repository_snapshot(&repo, git::never_cancelled()).unwrap(),
            )
            .0
            .as_deref(),
            Some(expected.as_path())
        );
        assert_eq!(
            repository_from_git_snapshot(
                git::local_repository_snapshot(&linked, git::never_cancelled()).unwrap(),
            )
            .0
            .as_deref(),
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
    fn session_repository_resolver_reuses_local_snapshot_cache_across_scans() {
        let root = temp_dir("tendi-session-repository-snapshot-cache-test");
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "--quiet"]);
        git(
            &root,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/example/tendi-before.git",
            ],
        );

        let mut first_resolver = SessionRepositoryResolver::default();
        let first = first_resolver.resolve(&root);
        assert_eq!(
            first.1.as_deref(),
            Some("https://github.com/example/tendi-before.git")
        );

        git(
            &root,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/example/tendi-after.git",
            ],
        );

        let mut second_resolver = SessionRepositoryResolver::default();
        assert_eq!(second_resolver.resolve(&root), first);

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
    fn cursor_project_folder_decodes_to_workspace_path() {
        assert_eq!(
            decode_cursor_project_dir("Users-test-dev-example-nextop").as_deref(),
            Some(Path::new("/Users/test/dev/example/nextop"))
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
        let cursor_session = root.join("private-run/cursor-fixture.jsonl");
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
        assert!(!scan_jsonl_meta_for_agent(&path, None).has_content);

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
        crate::providers::codex::collect_tutti_run_session_paths(&root, &mut paths);

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

        assert!(plan.dynamic_roots.contains(&root));
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
    fn session_candidate_discovery_uses_provider_owned_files() {
        assert!(is_session_candidate_path(Path::new("/tmp/session.jsonl")));
        assert!(is_session_candidate_path(Path::new(
            "/tmp/cursor/meta.json"
        )));
        assert!(is_session_candidate_path(Path::new("/tmp/cursor/store.db")));
        assert!(!is_session_candidate_path(Path::new(
            "/tmp/cursor/state.json"
        )));
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
        let codex_with_injected_context = json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "<recommended_plugins>\nhidden\n</recommended_plugins>" },
                    { "type": "input_text", "text": "<environment_context>hidden</environment_context>" },
                    { "type": "input_text", "text": "The real user request" }
                ]
            }
        });
        assert_eq!(
            extract_session_title(&codex_with_injected_context),
            Some("The real user request".to_string())
        );
        let codex_with_embedded_context = json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": "Real request\n<turn_aborted>\nThe previous turn was interrupted.\n</turn_aborted>\nNext request"
            }
        });
        assert_eq!(
            extract_session_title(&codex_with_embedded_context),
            Some("Real request".to_string())
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
        assert_eq!(
            clean_preview_text(
                "<image name=[Image #1] path=\"/tmp/image.png\">\nShow the session preview"
            ),
            Some("Show the session preview".to_string())
        );
        assert_eq!(
            clean_preview_text("<image name=[Image #1] path=\"/tmp/image.png\">"),
            None
        );
        assert_eq!(clean_preview_text("<recommended_plugins>hidden"), None);
        assert_eq!(
            clean_title("<image name=[Image #1] path=\"/tmp/image.png\">"),
            None
        );
        assert_eq!(
            clean_title("<image name=[Image #1] path=\"/tmp/image.png\""),
            None
        );
        assert_eq!(
            clean_title("<image name=[Image #1] path=\"/tmp/image.png\">\nFollow-up request"),
            Some("Follow-up request".to_string())
        );
        assert_eq!(
            clean_title("<image name=[Image #1] path=\"/tmp/image.png\"> Follow-up request"),
            Some("Follow-up request".to_string())
        );
        assert_eq!(
            clean_title("![Image #1](/tmp/image.png) Follow-up request"),
            Some("Follow-up request".to_string())
        );
        let long_title = format!(
            "<image name=[Image #1] path=\"/tmp/image.png\">\n{}",
            "x".repeat(120)
        );
        let parsed_long_title = clean_title(&long_title).unwrap();
        assert_eq!(parsed_long_title.chars().count(), 96);
        assert!(parsed_long_title.chars().all(|character| character == 'x'));
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
    fn cursor_embedded_timestamps_define_session_bounds() {
        let root = temp_dir("tendi-cursor-embedded-timestamp-session");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session.jsonl");
        let first = json!({
            "role": "user",
            "message": {
                "content": [{
                    "type": "text",
                    "text": "<timestamp>Thursday, Aug 27, 2026, 11:01 PM (UTC+8)</timestamp>\n<user_query>First</user_query>"
                }]
            }
        });
        let second = json!({
            "role": "user",
            "message": {
                "content": [{
                    "type": "text",
                    "text": "<timestamp>Friday, Aug 28, 2026, 12:05 AM (UTC+8)</timestamp>\n<user_query>Second</user_query>"
                }]
            }
        });
        fs::write(&path, format!("{first}\n{second}\n")).unwrap();

        let meta = scan_jsonl_meta_for_agent(&path, Some(AgentKind::Cursor));

        assert_eq!(
            meta.started_at.as_deref(),
            Some("2026-08-27T23:01:00+08:00")
        );
        assert_eq!(
            meta.updated_at.as_deref(),
            Some("2026-08-28T00:05:00+08:00")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn child_jsonl_title_skips_inherited_history_without_spawn_task_name() {
        let root = temp_dir("tendi-child-title-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-12345678-1234-1234-1234-123456789012.jsonl");
        fs::write(
            &path,
            [
                r#"{"ordinal":0,"type":"session_meta","payload":{"parent_thread_id":"parent-id","thread_source":"subagent","subagent_history_start_ordinal":3}}"#,
                r#"{"ordinal":1,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Parent orchestration"}]}}"#,
                r#"{"ordinal":2,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Inherited label"}]}}"#,
                r#"{"ordinal":3,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Child-specific task"}]}}"#,
                r#"{"ordinal":4,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Follow-up"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        assert_eq!(
            scan_jsonl_meta_for_agent(&path, Some(AgentKind::Codex))
                .title
                .as_deref(),
            Some("Child-specific task")
        );

        let mut first_scan = Vec::new();
        scan_codex_jsonl(&root, &mut first_scan, None);
        assert_eq!(
            crate::providers::codex::session_title(&path).as_deref(),
            Some("Child-specific task")
        );
        let (file_mtime, file_size) = file_state(&path).unwrap();
        let mut cached_session = first_scan[0].clone();
        cached_session.title = Some("Follow-up".to_string());
        cached_session.first_user_message = Some("Inherited label".to_string());
        assert!(session_requires_rescan(&cached_session));
        let cache = SessionScanCache::from_entries([SessionScanCacheEntry {
            session: cached_session,
            file_mtime,
            file_size,
        }]);

        let mut rescanned = Vec::new();
        scan_codex_jsonl(&root, &mut rescanned, Some(&cache));

        assert_eq!(rescanned[0].title.as_deref(), Some("Child-specific task"));
        assert_eq!(
            rescanned[0].first_user_message.as_deref(),
            Some("Child-specific task")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn child_jsonl_title_uses_spawn_task_name_and_refreshes_cached_parent_title() {
        let root = temp_dir("tendi-child-spawn-task-title-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-12345678-1234-1234-1234-123456789012.jsonl");
        fs::write(
            &path,
            [
                r#"{"type":"session_meta","payload":{"parent_thread_id":"parent-id","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-id","task_name":"composer_connection_reuse","agent_path":"/root/legacy_name","agent_nickname":"Schrodinger"}}},"agent_path":"/root/legacy_name"}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Parent orchestration"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        assert_eq!(
            scan_jsonl_meta_for_agent(&path, None).title.as_deref(),
            Some("composer_connection_reuse")
        );

        let mut first_scan = Vec::new();
        scan_codex_jsonl(&root, &mut first_scan, None);
        let (file_mtime, file_size) = file_state(&path).unwrap();
        let mut cached_session = first_scan[0].clone();
        cached_session.title = Some("Parent orchestration".to_string());
        let cache = SessionScanCache::from_entries([SessionScanCacheEntry {
            session: cached_session,
            file_mtime,
            file_size,
        }]);

        let mut rescanned = Vec::new();
        scan_codex_jsonl(&root, &mut rescanned, Some(&cache));

        assert_eq!(
            rescanned[0].title.as_deref(),
            Some("composer_connection_reuse")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn child_jsonl_cache_clears_inherited_preview_without_child_user_message() {
        let root = temp_dir("tendi-child-preview-boundary-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-12345678-1234-1234-1234-123456789012.jsonl");
        fs::write(
            &path,
            [
                r#"{"ordinal":0,"type":"session_meta","payload":{"parent_thread_id":"parent-id","thread_source":"subagent","subagent_history_start_ordinal":2,"source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-id","task_name":"child_without_user"}}}}}"#,
                r#"{"ordinal":1,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Parent user message"}]}}"#,
                r#"{"ordinal":2,"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Child response"}]}}"#,
                r#"{"ordinal":3,"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Child follow-up"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let mut first_scan = Vec::new();
        scan_codex_jsonl(&root, &mut first_scan, None);
        assert_eq!(first_scan[0].title.as_deref(), Some("child_without_user"));
        assert_eq!(first_scan[0].first_user_message, None);
        assert_eq!(first_scan[0].last_user_message, None);
        assert_eq!(
            first_scan[0].last_assistant_message.as_deref(),
            Some("Child follow-up")
        );

        let (file_mtime, file_size) = file_state(&path).unwrap();
        let mut cached_session = first_scan[0].clone();
        cached_session.first_user_message = Some("Parent user message".to_string());
        cached_session.last_user_message = Some("Parent user message".to_string());
        assert!(session_requires_rescan(&cached_session));
        let cache = SessionScanCache::from_entries([SessionScanCacheEntry {
            session: cached_session,
            file_mtime,
            file_size,
        }]);

        let mut rescanned = Vec::new();
        scan_codex_jsonl(&root, &mut rescanned, Some(&cache));

        assert_eq!(rescanned[0].first_user_message, None);
        assert_eq!(rescanned[0].last_user_message, None);
        assert_eq!(
            rescanned[0].last_assistant_message.as_deref(),
            Some("Child follow-up")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_index_ignores_injected_thread_names() {
        let root = temp_dir("tendi-codex-index-injected-title-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session_index.jsonl");
        fs::write(
            &path,
            r#"{"id":"session-id","thread_name":"<recommended_plugins>"}"#,
        )
        .unwrap();

        let mut sessions = Vec::new();
        let mut warnings = Vec::new();
        scan_codex_index(&path, &mut sessions, &mut warnings).unwrap();

        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, None);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn counts_only_real_user_turns() {
        let root = temp_dir("tendi-session-turn-count-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session.jsonl");
        fs::write(
            &path,
            [
                r##"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<recommended_plugins>\nhidden\n</recommended_plugins>"},{"type":"input_text","text":"# AGENTS.md instructions\nhidden"},{"type":"input_text","text":"<environment_context>hidden</environment_context>"}]}}"##,
                r##"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions\nhidden"}]}}"##,
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"First question"}]}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"First answer"}]}}"#,
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"tool output"}]}}"#,
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"<user_query>\nSecond question\n</user_query>"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let meta = scan_jsonl_meta_for_agent(&path, None);

        assert_eq!(meta.message_count, Some(6));
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

        let meta = scan_jsonl_meta_for_agent(&path, None);

        assert_eq!(meta.started_at.as_deref(), Some("2026-08-12T10:00:00Z"));
        assert_eq!(meta.updated_at.as_deref(), Some("2026-08-12T10:00:01Z"));
        assert_eq!(meta.message_count, Some(2));
        assert_eq!(meta.turn_count, Some(1));
        assert_eq!(meta.title.as_deref(), Some("Check CPU usage"));
        let _ = fs::remove_dir_all(root);
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
            r#"{"schemaVersion":1,"name":"<image name=[Image #1] path=\"/tmp/image.png\"> Pinned Cursor session","cwd":"/Users/test/dev/tendi"}"#,
        )
        .unwrap();

        let mut sessions = Vec::new();
        scan_cursor_meta(&root, &mut sessions, AgentKind::Cursor, None);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title.as_deref(), Some("Pinned Cursor session"));
        assert_eq!(sessions[0].message_count, None);

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
        scan_jsonl_sessions(&transcripts_root, AgentKind::Cursor, 4, &mut sessions, None);
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
            AgentKind::Cursor,
            Path::new("/Users/test/.cursor/chats/session-id/meta.json"),
            Path::new(
                "/Users/test/.cursor/projects/project/agent-transcripts/session-id/session-id.jsonl"
            )
        ));
        assert!(!should_replace_session_path(
            AgentKind::Cursor,
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
        let mut tutti = sessions[0].clone();
        tutti.id = "codex-tutti".to_string();
        tutti.agent = AgentKind::Codex;
        tutti.project = Some(PathBuf::from(
            "/Users/test/Documents/tutti/session-c98d1ced-5371-43cc-8173-9416c349a776",
        ));
        tutti.path = PathBuf::from("/tmp/codex-tutti.jsonl");
        sessions.push(tutti);
        normalize_session_projects(&mut sessions);

        assert_eq!(
            sessions[0].project.as_deref(),
            Some(Path::new(
                "/Users/test/Documents/tutti/session-c98d1ced-5371-43cc-8173-9416c349a776",
            ))
        );
        assert_eq!(
            sessions[1].project.as_deref(),
            Some(Path::new("/Users/test/dev/tendi"))
        );
        assert_eq!(
            sessions[2].project.as_deref(),
            Some(Path::new("/Users/test/Documents/Codex"))
        );
        assert_eq!(
            sessions[3].project.as_deref(),
            Some(Path::new("/Users/test/Documents/tutti"))
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

        let usage = scan_jsonl_meta_for_agent(&path, None).token_usage.unwrap();

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

        let usage = scan_jsonl_meta_for_agent(&path, None).token_usage.unwrap();

        assert_eq!(usage.input_tokens, 160);
        assert_eq!(usage.cached_input_tokens, 125);
        assert_eq!(usage.output_tokens, 15);
        assert_eq!(usage.reasoning_output_tokens, 0);
        assert_eq!(usage.total_tokens, 175);
        let _ = fs::remove_dir_all(root);
    }

}
