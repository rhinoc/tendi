use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub use crate::generated::runtime_contract::AppSettingsPatch;
use anyhow::{Context, Result, bail};
use chrono::Local;
use rusqlite::{
    Connection, OptionalExtension, Transaction, params, params_from_iter,
    types::{Type, Value as SqlValue},
};
use serde::{Deserialize, Serialize};

#[path = "fs_manifest.rs"]
mod fs_manifest;
pub use fs_manifest::{FsManifestEntry, canonical_workspace_root};

#[path = "session_search_storage.rs"]
mod session_search_storage;
pub use session_search_storage::SessionSearchPublication;

#[path = "storage/repositories/mod.rs"]
mod repositories;
pub use repositories::ProjectionRefreshState;
use repositories::{advance_projection_head_in_tx, cleanup_stale_scoped_session_skill_rows};

#[path = "storage/writer.rs"]
mod database_writer;

#[path = "storage/database.rs"]
mod database;
#[path = "migrations/mod.rs"]
pub(crate) mod migrations;
#[path = "storage/transaction.rs"]
mod transaction;

use crate::{
    HookScan, McpScan, RuleRecord, RuleScan, ScanReport,
    analytics::{
        self, AnalyticsCapabilities, AnalyticsCoverage, AnalyticsProviderCapability,
        AnalyticsRefreshProgress, AnalyticsRefreshReport, OverviewAnalytics,
        SessionAnalyticsOverviewRecord, SessionAnalyticsRecord,
    },
    assistant::{AssistantChatSession, AssistantMessage, AssistantSessionLink},
    fsutil::sha256_text,
    projects::{self, ProjectRecord, ProjectScanResult, ProjectScanScope},
    runtime_contract::{
        OperationId, OperationRecord, OperationStatus, ProjectionHead, Revision, ScopeKey,
        SourceVersion,
    },
    session_skills::{SessionFileState, SessionSkillIndexStatus, SessionSkillLink},
    sessions::{
        SessionIdentity, SessionRecord, SessionScan, SessionScanCache, SessionScanCacheEntry,
        SessionScanSourceState, bound_session_preview, clean_session_title,
        normalize_session_projects,
    },
    skills::{AgentKind, SkillScan, SkillSnapshot, SkillSnapshotFile, SkillSourceRecord},
    time::compare_timestamps,
    transcript,
};

#[derive(Debug, Clone)]
struct ProjectState {
    name: String,
    name_custom: bool,
    last_seen_at: String,
}

const SESSION_ANALYTICS_BATCH_SIZE: usize = 64;
// The current schema is squashed; all development revisions before this
// release were never published as compatibility boundaries.
pub(crate) const STORAGE_SCHEMA_VERSION: i64 = 1;
const SESSION_SEARCH_INDEX_VERSION: i64 = 2;
pub(crate) const PROJECTION_PARSER_VERSION: &str = "scan-v8";
const DATABASE_READ_LOCK_ATTEMPTS: usize = 100;
const DATABASE_READ_LOCK_RETRY: Duration = Duration::from_millis(50);
const SCOPED_SESSION_TABLE: &str = "scoped_sessions";
const SCOPED_SESSION_SCAN_SOURCE_TABLE: &str = "scoped_session_scan_sources";
const SESSION_SCAN_CACHE_PARSER_VERSION: &str = "scan-v10";
const SESSION_SEARCH_CANDIDATE_TABLE: &str = "tendi_session_search_candidates";
const SESSION_SEARCH_MATCH_TABLE: &str = "tendi_session_search_matches";
const DEFAULT_SCOPE_KEY: &str = "installation:default";
const NORMALIZED_SNAPSHOT_TABLE: &str = "normalized_snapshots";
const SCOPED_PROJECTION_CONTEXT_TABLE: &str = "scoped_projection_contexts";

const PROJECTION_DOMAINS: [&str; 5] = ["agents", "skills", "rules", "hooks", "mcp"];

fn log_skill_source_record_write(table: &str, scope_key: Option<&str>, record: &SkillSourceRecord) {
    crate::logging::global().debug(
        "skill source record write",
        serde_json::json!({
            "operation": "skill_source_record_write",
            "table": table,
            "scopeKey": scope_key,
            "skillName": &record.skill_name,
            "skillPath": &record.skill_path,
            "source": &record.source,
            "sourceRelativePath": &record.source_relative_path,
            "sourceVersion": &record.source_version,
            "updateStatus": &record.update_status,
            "origin": &record.origin,
        }),
    );
}

pub(crate) fn is_database_lock_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<rusqlite::Error>()
            .is_some_and(|error| {
                matches!(
                    error,
                    rusqlite::Error::SqliteFailure(code, _)
                        if code.code == rusqlite::ErrorCode::DatabaseBusy
                            || code.code == rusqlite::ErrorCode::DatabaseLocked
                )
            })
    })
}

/// SQLite reports WAL/file-descriptor failures as `SQLITE_IOERR` (including
/// extended codes such as `SQLITE_IOERR_SHORT_READ`). These errors are
/// recoverable at the connection boundary; replaying the business operation
/// is not safe.
pub fn is_database_io_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<rusqlite::Error>()
            .is_some_and(|error| {
                matches!(
                    error,
                    rusqlite::Error::SqliteFailure(code, _)
                        if code.code == rusqlite::ErrorCode::SystemIoFailure
                )
            })
    })
}

/// Some daemon boundaries currently carry only a serialized error message.
/// Keep the fallback matcher narrow and tied to SQLite's IOERR diagnostics.
pub fn is_database_io_error_message(message: &str) -> bool {
    message.contains("disk I/O error") || message.contains("Error code 522")
}

pub fn recover_database(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let path = fs::canonicalize(path)
        .with_context(|| format!("failed to resolve sqlite database {}", path.display()))?;
    database::DatabaseWriter::open(&path)?.recover()
}

fn with_database_read_lock_retry<T, F>(mut read: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    for attempt in 0..DATABASE_READ_LOCK_ATTEMPTS {
        match read() {
            Ok(value) => return Ok(value),
            Err(error)
                if attempt + 1 < DATABASE_READ_LOCK_ATTEMPTS && is_database_lock_error(&error) =>
            {
                std::thread::sleep(DATABASE_READ_LOCK_RETRY);
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("database read retry loop always returns");
}

fn session_scan_source_states(session: &SessionRecord) -> Vec<SessionScanSourceState> {
    crate::providers::agent_provider(session.agent)
        .session_scan_source_paths(&session.path)
        .into_iter()
        .map(|path| {
            let (file_mtime, file_size) = crate::session_skills::session_file_state(&path)
                .map(|state| (state.file_mtime, state.file_size))
                .unwrap_or((0, 0));
            SessionScanSourceState {
                path,
                file_mtime,
                file_size,
            }
        })
        .collect()
}

type PreparedSessionSources = HashMap<PathBuf, repositories::PreparedSessionSource>;
type PreparedProjectAliases = HashMap<(String, String), Vec<SessionProjectAlias>>;

fn prepare_project_aliases(sessions: &[SessionRecord]) -> PreparedProjectAliases {
    sessions
        .iter()
        .map(|session| {
            (
                (session.id.clone(), agent_label(session.agent).to_string()),
                session_project_aliases(session),
            )
        })
        .collect()
}

pub fn workspace_scope_key(workspace_root: &Path) -> Result<ScopeKey> {
    ScopeKey::new(format!(
        "workspace:{}",
        canonical_workspace_root(workspace_root).display()
    ))
    .map_err(|error| anyhow::anyhow!(error))
}

fn workspace_root_for_manifest_root(root: &Path) -> PathBuf {
    let is_agent_dir = |path: &Path| {
        matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".agents" | ".codex" | ".claude" | ".cursor")
        )
    };
    if root.file_name().and_then(|name| name.to_str()) == Some("skills")
        && root.parent().is_some_and(is_agent_dir)
    {
        return root
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.to_path_buf());
    }
    root.to_path_buf()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionStatus {
    Fresh,
    Missing,
    Stale,
    Refreshing,
}

fn analytics_refresh_progress(
    report: &AnalyticsRefreshReport,
    completed: usize,
) -> AnalyticsRefreshProgress {
    AnalyticsRefreshProgress {
        total: report.total,
        completed,
        parsed: report.parsed,
        appended: report.appended,
        skipped: report.skipped,
        failed: report.failed,
    }
}

pub struct Store {
    conn: Connection,
    path: PathBuf,
    writer: std::sync::Arc<database::DatabaseWriter>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default = "default_appearance")]
    pub appearance: String,
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_color_theme")]
    pub light_theme: String,
    #[serde(default = "default_color_theme")]
    pub dark_theme: String,
    #[serde(default = "default_app_icon")]
    pub app_icon: String,
    pub terminal: String,
    #[serde(default = "default_session_resume_target")]
    pub session_resume_target: String,
    #[serde(default = "default_missing_session_project_policy")]
    pub missing_session_project_policy: String,
    #[serde(default = "default_editor")]
    pub editor: String,
    #[serde(default)]
    pub developer_mode: bool,
    #[serde(default)]
    pub additional_session_roots: Vec<String>,
    #[serde(default)]
    pub config_profiles: BTreeMap<String, String>,
}

fn default_appearance() -> String {
    "system".to_string()
}

fn default_font_family() -> String {
    "manrope".to_string()
}

fn default_color_theme() -> String {
    "vercel".to_string()
}

fn default_app_icon() -> String {
    "gruvbox".to_string()
}

fn default_editor() -> String {
    "vscode".to_string()
}

fn default_session_resume_target() -> String {
    "auto".to_string()
}

fn default_missing_session_project_policy() -> String {
    "show".to_string()
}

fn normalize_appearance(value: &str) -> Result<String> {
    let appearance = value.trim().to_ascii_lowercase();
    if matches!(appearance.as_str(), "system" | "light" | "dark") {
        Ok(appearance)
    } else {
        anyhow::bail!("invalid appearance setting: {value}")
    }
}

fn normalize_font_family(value: &str) -> Result<String> {
    let font_family = value.trim().to_ascii_lowercase();
    if matches!(
        font_family.as_str(),
        "geist"
            | "manrope"
            | "inter"
            | "ibm-plex-sans"
            | "instrument-sans"
            | "plus-jakarta-sans"
            | "bricolage-grotesque"
    ) {
        Ok(font_family)
    } else {
        anyhow::bail!("invalid font family setting: {value}")
    }
}

fn normalize_color_theme(value: &str) -> Result<String> {
    let theme = value.trim().to_ascii_lowercase();
    if matches!(
        theme.as_str(),
        "sakura-pop" | "gruvbox" | "dracula" | "nord" | "catppuccin" | "tokyo-night" | "vercel"
    ) {
        Ok(theme)
    } else {
        anyhow::bail!("invalid color theme setting: {value}")
    }
}

fn normalize_session_resume_target(value: &str) -> Result<String> {
    let target = value.trim().to_ascii_lowercase();
    match target.as_str() {
        "auto" | "terminal" => Ok(target),
        "app" => Ok("app".to_string()),
        _ => anyhow::bail!("invalid session resume target: {value}"),
    }
}

fn normalize_missing_session_project_policy(value: &str) -> Result<String> {
    let policy = value.trim().to_ascii_lowercase();
    if matches!(policy.as_str(), "show" | "hide" | "merge-by-name") {
        Ok(policy)
    } else {
        anyhow::bail!("invalid missing session project policy: {value}")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PromptRecord {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PromptWrite {
    pub id: Option<String>,
    pub title: String,
    pub tags: Vec<String>,
    pub body: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionSearchHit {
    #[serde(flatten)]
    pub session: SessionRecord,
    pub search_score: f64,
    pub search_snippet: String,
}

#[derive(Debug, Clone)]
pub struct SessionListQuery {
    pub query: String,
    pub agent: Option<AgentKind>,
    pub sort_key: String,
    pub sort_direction: String,
    pub group_by: Option<String>,
    pub page: usize,
    pub page_size: usize,
    pub show_child_sessions: bool,
    pub selected_project_keys: Vec<String>,
    pub locate: Option<SessionIdentity>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionListRow {
    #[serde(flatten)]
    pub session: SessionRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListProjectOption {
    pub key: String,
    pub label: String,
    pub title: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListPage {
    pub rows: Vec<SessionListRow>,
    pub project_options: Vec<SessionListProjectOption>,
    pub total: usize,
    pub child_session_count: usize,
    pub page: usize,
    pub page_count: usize,
    pub page_start: usize,
    pub page_end: usize,
    pub group_count: Option<usize>,
}

#[derive(Debug, Clone)]
struct ResolvedSessionProject {
    key: String,
    label: String,
    title: String,
}

#[derive(Debug)]
struct SessionListGroupPage {
    rows: Vec<SessionListRow>,
    start: usize,
    group_count: usize,
}

fn session_list_text(value: Option<&str>) -> String {
    value.unwrap_or_default().trim().to_string()
}

fn session_list_path(value: Option<&PathBuf>) -> String {
    value
        .map(|path| {
            path.to_string_lossy()
                .trim_end_matches(['/', '\\'])
                .to_string()
        })
        .unwrap_or_default()
}

fn session_list_project_label(session: &SessionRecord) -> String {
    let logical_name = session_list_text(session.logical_project_name.as_deref());
    if !logical_name.is_empty() {
        return logical_name;
    }
    session
        .repository
        .as_ref()
        .or(session.project.as_ref())
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn session_list_identity(session: &SessionRecord) -> String {
    format!(
        "{}\0{}\0{}",
        session.agent.label(),
        session.id,
        session.path.display()
    )
}

fn session_list_project_group_key(session: &SessionRecord) -> String {
    if let Some(project_id) = session
        .logical_project_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        return serde_json::to_string(&(
            "logical-project",
            project_id,
            session_list_project_label(session).as_str(),
        ))
        .unwrap_or_default();
    }
    let repository = session_list_path(session.repository.as_ref());
    if repository.is_empty() {
        session_list_path(session.project.as_ref())
    } else {
        repository
    }
}

fn session_list_project_for_path<'a>(
    path: Option<&PathBuf>,
    projects: &'a [ProjectRecord],
) -> Option<&'a ProjectRecord> {
    let path = path?;
    projects
        .iter()
        .filter(|project| path == &project.root_path || path.starts_with(&project.root_path))
        .max_by_key(|project| project.root_path.components().count())
}

fn resolve_session_list_project(
    session: &SessionRecord,
    policy: &str,
    session_projects: &[SessionProjectSummary],
    projects: &[ProjectRecord],
) -> Option<ResolvedSessionProject> {
    let logical_id = session
        .logical_project_id
        .as_deref()
        .unwrap_or_default()
        .trim();
    let workspace_path = session_list_path(session.project.as_ref());
    let summary = session_projects.iter().find(|project| {
        (!logical_id.is_empty() && project.id.trim() == logical_id)
            || (!workspace_path.is_empty()
                && project
                    .paths
                    .iter()
                    .any(|path| session_list_path(Some(path)) == workspace_path))
    });
    if summary.is_some_and(|project| project.missing) && policy == "hide" {
        return None;
    }

    let label = session_list_project_label(session);
    let title = session
        .repository_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or(workspace_path);
    if policy != "merge-by-name" {
        return Some(ResolvedSessionProject {
            key: session_list_project_group_key(session),
            label,
            title,
        });
    }

    let normalized_name = label.trim().to_lowercase();
    let same_name_projects = projects
        .iter()
        .filter(|project| {
            let root_name = project
                .root_path
                .file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            !normalized_name.is_empty()
                && (project.name.trim().to_lowercase() == normalized_name
                    || root_name == normalized_name)
        })
        .collect::<Vec<_>>();
    let scanned_project = session_list_project_for_path(session.project.as_ref(), projects)
        .or_else(|| session_list_project_for_path(session.repository.as_ref(), projects))
        .or_else(|| {
            (summary.is_some_and(|project| project.missing) && same_name_projects.len() == 1)
                .then_some(same_name_projects[0])
        });
    let Some(project) = scanned_project else {
        return Some(ResolvedSessionProject {
            key: session_list_project_group_key(session),
            label,
            title,
        });
    };
    Some(ResolvedSessionProject {
        key: serde_json::to_string(&("scanned-project", project.id.as_str())).unwrap_or_default(),
        label: project.name.trim().to_string(),
        title: project
            .remote_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| project.root_path.to_string_lossy().into_owned()),
    })
}

fn compare_session_list_rows(
    left: &SessionListRow,
    right: &SessionListRow,
    key: &str,
    direction: &str,
) -> std::cmp::Ordering {
    let sessions = (&left.session, &right.session);
    let ordering = match key {
        "startedAt" => compare_timestamps(
            sessions.0.started_at.as_deref(),
            sessions.1.started_at.as_deref(),
        ),
        "updatedAt" => compare_timestamps(
            sessions.0.updated_at.as_deref(),
            sessions.1.updated_at.as_deref(),
        ),
        "messages" => sessions
            .0
            .message_count
            .unwrap_or(0)
            .cmp(&sessions.1.message_count.unwrap_or(0)),
        "turns" => sessions
            .0
            .turn_count
            .unwrap_or(0)
            .cmp(&sessions.1.turn_count.unwrap_or(0)),
        "cacheRate" => {
            let rate = |session: &SessionRecord| {
                session
                    .token_usage
                    .as_ref()
                    .filter(|usage| usage.input_tokens > 0)
                    .map(|usage| usage.cached_input_tokens as f64 / usage.input_tokens as f64)
                    .unwrap_or(-1.0)
            };
            rate(sessions.0).total_cmp(&rate(sessions.1))
        }
        "searchScore" => left
            .search_score
            .unwrap_or(f64::NEG_INFINITY)
            .total_cmp(&right.search_score.unwrap_or(f64::NEG_INFINITY)),
        "title" => session_list_text(sessions.0.title.as_deref())
            .to_lowercase()
            .cmp(&session_list_text(sessions.1.title.as_deref()).to_lowercase()),
        "agent" => sessions
            .0
            .agent
            .label()
            .to_lowercase()
            .cmp(&sessions.1.agent.label().to_lowercase()),
        "project" => session_list_project_label(sessions.0)
            .to_lowercase()
            .cmp(&session_list_project_label(sessions.1).to_lowercase()),
        _ => std::cmp::Ordering::Equal,
    };
    if direction == "desc" {
        ordering.reverse()
    } else {
        ordering
    }
}

fn session_list_group_key(row: &SessionListRow, group_by: &str) -> String {
    match group_by {
        "agent" => row.session.agent.label().to_string(),
        "project" => session_list_project_group_key(&row.session),
        "startedAt" => row
            .session
            .started_at
            .as_deref()
            .and_then(|value| value.get(..10))
            .unwrap_or_default()
            .to_string(),
        "updatedAt" => row
            .session
            .updated_at
            .as_deref()
            .and_then(|value| value.get(..10))
            .unwrap_or_default()
            .to_string(),
        "title" => session_list_text(row.session.title.as_deref()),
        "messages" => row.session.message_count.unwrap_or(0).to_string(),
        "turns" => row.session.turn_count.unwrap_or(0).to_string(),
        "cacheRate" => row
            .session
            .token_usage
            .as_ref()
            .filter(|usage| usage.input_tokens > 0)
            .map(|usage| {
                (usage.cached_input_tokens as f64 / usage.input_tokens as f64 * 100.0).to_string()
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn build_session_list_group_pages(
    rows: Vec<SessionListRow>,
    group_by: &str,
    page_size: usize,
) -> Vec<SessionListGroupPage> {
    let mut group_indexes = HashMap::<String, usize>::new();
    let mut groups = Vec::<Vec<SessionListRow>>::new();
    for row in rows {
        let key = session_list_group_key(&row, group_by);
        let index = *group_indexes.entry(key).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[index].push(row);
    }
    let mut pages = Vec::new();
    let mut page_rows = Vec::new();
    let mut page_start = 0;
    let mut page_group_count = 0;
    for group in groups {
        if !page_rows.is_empty() && page_rows.len() >= page_size {
            let row_count = page_rows.len();
            pages.push(SessionListGroupPage {
                rows: page_rows,
                start: page_start,
                group_count: page_group_count,
            });
            page_start += row_count;
            page_rows = Vec::new();
            page_group_count = 0;
        }
        page_rows.extend(group);
        page_group_count += 1;
    }
    if !page_rows.is_empty() || pages.is_empty() {
        pages.push(SessionListGroupPage {
            rows: page_rows,
            start: page_start,
            group_count: page_group_count,
        });
    }
    pages
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionProjectSummary {
    pub id: String,
    pub name: String,
    pub missing: bool,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SessionSearchDocument {
    metadata_text: String,
    title: String,
    project: String,
    user_text: String,
    assistant_text: String,
}

impl Store {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Enter the named database-owned writer once, committing or rolling back together.
    fn with_named_write_transaction<T>(
        &self,
        operation: &str,
        write: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.writer.write(operation, write)
    }

    /// Rebuildable derived data with source/generation checks at commit time.
    fn with_background_write_transaction<T>(
        &self,
        operation: &str,
        write: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.writer.write_background(operation, write)
    }
}

fn normalize_setting_value(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

type SessionProjectAlias = (String, String);

fn session_project_aliases(session: &SessionRecord) -> Vec<SessionProjectAlias> {
    let mut aliases = Vec::new();
    if let Some(url) = session
        .repository_url
        .as_deref()
        .and_then(normalize_repository_url)
    {
        aliases.push(("repository_url".to_string(), url));
    }
    if let Some(path) = session.repository.as_deref() {
        aliases.push(("repository_path".to_string(), normalize_project_path(path)));
    }
    if let Some(path) = session.project.as_deref() {
        let path = normalize_project_path(path);
        aliases.push(("workspace_path".to_string(), path.clone()));
        for alias in crate::providers::agent_provider(session.agent).session_project_aliases(&path)
        {
            aliases.push(("workspace_path".to_string(), alias));
        }
    }
    aliases.sort();
    aliases.dedup();
    aliases
}

fn normalize_repository_url(value: &str) -> Option<String> {
    let mut value = value
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string();
    if value.is_empty() {
        return None;
    }
    if let Some((user_host, path)) = value.split_once(':')
        && user_host.contains('@')
        && !user_host.contains("//")
    {
        let host = user_host.rsplit('@').next()?.to_ascii_lowercase();
        return Some(format!("{host}/{}", path.trim_start_matches('/')));
    }
    if let Some((_, rest)) = value.split_once("://") {
        value = rest.to_string();
    }
    let value = value.trim_start_matches('/');
    let (host, path) = value.split_once('/')?;
    let host = host.rsplit('@').next()?;
    (!host.is_empty() && !path.is_empty())
        .then(|| format!("{}/{}", host.to_ascii_lowercase(), path))
}

fn normalize_project_path(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .trim_end_matches('/')
        .to_string()
}

fn suggested_project_name(session: &SessionRecord) -> Option<String> {
    session
        .repository_url
        .as_deref()
        .and_then(normalize_repository_url)
        .and_then(|url| url.rsplit('/').next().map(str::to_string))
        .or_else(|| {
            session
                .repository
                .as_deref()
                .or(session.project.as_deref())
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .filter(|name| !name.is_empty())
}

fn session_project_seen_at(session: &SessionRecord) -> String {
    session
        .updated_at
        .as_deref()
        .or(session.started_at.as_deref())
        .unwrap_or("")
        .to_string()
}

fn normalize_additional_session_roots(values: Vec<String>) -> Result<Vec<String>> {
    let mut roots = BTreeSet::new();
    for value in values {
        for line in value.lines() {
            let value = line.trim();
            if value.is_empty() {
                continue;
            }
            let path = if value == "~" {
                dirs::home_dir().context("could not resolve home directory")?
            } else if let Some(relative) = value.strip_prefix("~/") {
                dirs::home_dir()
                    .context("could not resolve home directory")?
                    .join(relative)
            } else {
                PathBuf::from(value)
            };
            if !path.is_absolute() {
                anyhow::bail!("Additional session root must be an absolute path: {value}");
            }
            roots.insert(path.to_string_lossy().into_owned());
        }
    }
    Ok(roots.into_iter().collect())
}

pub fn default_db_path() -> Result<PathBuf> {
    let base = dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Library/Application Support")))
        .context("could not resolve application support directory")?;
    Ok(base.join("tendi/tendi.sqlite3"))
}

#[cfg(test)]
pub(crate) fn test_default_db_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "tendi-unit-test-default-{}.sqlite3",
        std::process::id()
    ))
}

pub(crate) fn agent_label(agent: AgentKind) -> &'static str {
    crate::providers::agent_provider(agent).storage_key()
}

fn primary_rule_agent(rule: &RuleRecord) -> Option<AgentKind> {
    rule.agents.first().copied()
}

fn parse_agent_label(value: &str) -> Option<AgentKind> {
    crate::providers::parse_agent(value).ok()
}

fn fs_manifest_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FsManifestEntry> {
    Ok(FsManifestEntry {
        source_kind: row.get(0)?,
        path: PathBuf::from(row.get::<_, String>(1)?),
        root: PathBuf::from(row.get::<_, String>(2)?),
        agent: row.get(3)?,
        scope: row.get(4)?,
        mtime_ns: row.get(5)?,
        size: row.get(6)?,
        inode: row.get(7)?,
        device: row.get(8)?,
        sha256: row.get(9)?,
        parser_version: row.get(10)?,
        last_seen_at: row.get(11)?,
        parse_status: row.get(12)?,
        resource_path: row.get::<_, Option<String>>(13)?.map(PathBuf::from),
    })
}

fn ensure_projection_domain(domain: &str) -> Result<()> {
    if PROJECTION_DOMAINS.contains(&domain) {
        Ok(())
    } else {
        anyhow::bail!("unknown projection domain: {domain}")
    }
}

fn manifest_source_kinds(domain: &str) -> Result<Vec<&'static str>> {
    ensure_projection_domain(domain)?;
    Ok(match domain {
        "agents" => vec!["agent", "agent-dir", "agent-candidate", "agent-root"],
        "skills" => vec![
            "skill",
            "skill-dir",
            "skill-candidate",
            "skill-root",
            "skill-install-root",
        ],
        "rules" => vec!["rule", "rule-dir", "rule-candidate", "rule-root"],
        "hooks" => vec!["hook", "hook-dir", "hook-candidate", "hook-root"],
        "mcp" => vec!["mcp", "mcp-dir", "mcp-candidate", "mcp-root"],
        _ => unreachable!("validated projection domain"),
    })
}

fn manifest_entry_is_current(entry: &FsManifestEntry) -> bool {
    let Ok(metadata) = fs::metadata(&entry.path) else {
        return entry.mtime_ns.is_none() && entry.size.is_none();
    };
    metadata_mtime_ns(&metadata) == entry.mtime_ns
        && i64::try_from(metadata.len()).ok() == entry.size
}

fn metadata_mtime_ns(metadata: &fs::Metadata) -> Option<i64> {
    metadata.modified().ok().and_then(|mtime| {
        let duration = mtime.duration_since(UNIX_EPOCH).ok()?;
        i64::try_from(duration.as_nanos())
            .ok()
            .or_else(|| i64::try_from(duration.as_millis()).ok())
            .or_else(|| i64::try_from(duration.as_secs()).ok())
    })
}

fn manifest_entry_for_path(
    source_kind: &str,
    path: &Path,
    root: &Path,
    agent: Option<String>,
    scope: Option<String>,
    sha256: Option<String>,
    parser_version: &str,
) -> FsManifestEntry {
    let metadata = fs::metadata(path).ok();
    FsManifestEntry {
        source_kind: source_kind.to_string(),
        resource_path: crate::coordination::canonical_resource_path(path).ok(),
        path: path.to_path_buf(),
        root: root.to_path_buf(),
        agent,
        scope,
        mtime_ns: metadata.as_ref().and_then(metadata_mtime_ns),
        size: metadata
            .as_ref()
            .and_then(|value| i64::try_from(value.len()).ok()),
        inode: None,
        device: None,
        sha256,
        parser_version: parser_version.to_string(),
        last_seen_at: unix_now() as i64,
        parse_status: if metadata.is_some() {
            "ok".to_string()
        } else {
            "missing".to_string()
        },
    }
}

fn append_manifest_candidates(
    entries: &mut Vec<FsManifestEntry>,
    domain: &str,
    workspace_root: &Path,
) {
    let kind = match domain {
        "agents" => "agent",
        "skills" => "skill",
        "rules" => "rule",
        "hooks" => "hook",
        "mcp" => "mcp",
        _ => return,
    };
    let workspace_root = canonical_workspace_root(workspace_root);
    let mut candidates = BTreeSet::new();
    candidates.insert(workspace_root.clone());
    if let Some(home) = dirs::home_dir() {
        for path in domain_candidate_files(domain, &home) {
            if let Some(parent) = path.parent() {
                candidates.insert(parent.to_path_buf());
            }
            candidates.insert(path);
        }
    }
    for path in domain_candidate_files(domain, &workspace_root) {
        if let Some(parent) = path.parent() {
            candidates.insert(parent.to_path_buf());
        }
        candidates.insert(path);
    }
    for entry in entries.iter() {
        let mut current = entry.path.parent();
        while let Some(path) = current {
            if path.starts_with(&workspace_root) {
                candidates.insert(path.to_path_buf());
            } else {
                candidates.insert(path.to_path_buf());
                break;
            }
            if path == workspace_root {
                break;
            }
            current = path.parent();
        }
    }

    let existing = entries
        .iter()
        .map(|entry| (entry.source_kind.clone(), entry.path.clone()))
        .collect::<BTreeSet<_>>();
    for path in candidates {
        let source_kind = if path == workspace_root {
            format!("{kind}-root")
        } else if path.extension().is_some()
            || crate::providers::all_providers()
                .into_iter()
                .any(|provider| provider.projection_candidate_is_file(&path))
        {
            format!("{kind}-candidate")
        } else {
            format!("{kind}-dir")
        };
        if existing.contains(&(source_kind.clone(), path.clone())) {
            continue;
        }
        entries.push(manifest_entry_for_path(
            &source_kind,
            &path,
            &workspace_root,
            None,
            None,
            None,
            PROJECTION_PARSER_VERSION,
        ));
    }
}

fn domain_candidate_files(domain: &str, root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let ancestors = root.ancestors().collect::<Vec<_>>();
    for ancestor in ancestors {
        for provider in crate::providers::all_providers() {
            paths.extend(provider.projection_candidate_files(domain, ancestor));
        }
    }
    paths
}

fn manifest_entries_for_agents(
    scan: &crate::agents::AgentScan,
    workspace_root: &Path,
) -> Vec<FsManifestEntry> {
    let mut entries = Vec::new();
    for agent in &scan.agents {
        if let Some(path) = agent.config_dir.as_deref() {
            entries.push(manifest_entry_for_path(
                "agent",
                path,
                path.parent().unwrap_or(workspace_root),
                Some(agent_label(agent.kind).to_string()),
                None,
                None,
                PROJECTION_PARSER_VERSION,
            ));
        }
    }
    append_manifest_candidates(&mut entries, "agents", workspace_root);
    entries
}

pub(crate) fn skill_source_records_from_scan(scan: &SkillScan) -> Vec<SkillSourceRecord> {
    let mut records_by_path = BTreeMap::new();
    let logger = crate::logging::global();
    for skill in &scan.skills {
        for path in &skill.paths {
            if logger.debug_enabled() {
                logger.debug(
                    "skill source observed during projection scan",
                    serde_json::json!({
                        "operation": "skill_source_projection_observed",
                        "skillName": &skill.name,
                        "skillPath": &path.path,
                        "skillSha256": &path.sha256,
                        "source": &path.source,
                        "sourceRelativePath": &path.source_relative_path,
                        "sourceVersion": &path.source_version,
                        "updateStatus": &path.update_status,
                        "scope": &path.scope,
                        "agent": path.agent,
                    }),
                );
            }
            let record = SkillSourceRecord {
                skill_name: skill.name.clone(),
                skill_path: path.path.clone(),
                source_kind: path.source_kind.clone(),
                source: path.source.clone(),
                source_ref: path.source_ref.clone(),
                source_version: path.source_version.clone(),
                source_relative_path: path.source_relative_path.clone(),
                update_status: path.update_status.clone(),
                origin: "projection-scan".to_string(),
            };
            records_by_path
                .entry(record.skill_path.clone())
                .or_insert(record);
        }
    }
    records_by_path.into_values().collect()
}

fn manifest_entries_for_skills(scan: &SkillScan, workspace_root: &Path) -> Vec<FsManifestEntry> {
    let mut entries = Vec::new();
    for root in &scan.roots {
        entries.push(manifest_entry_for_path(
            "skill-install-root",
            &root.path,
            &root.path,
            Some(agent_label(root.agent).to_owned()),
            Some(root.scope.clone()),
            None,
            PROJECTION_PARSER_VERSION,
        ));
    }
    for skill in &scan.skills {
        for path in &skill.paths {
            entries.push(manifest_entry_for_path(
                "skill",
                &path.path.join("SKILL.md"),
                &path.root,
                Some(agent_label(path.agent).to_string()),
                Some(path.scope.clone()),
                (!path.sha256.is_empty()).then(|| path.sha256.clone()),
                PROJECTION_PARSER_VERSION,
            ));
        }
    }
    append_manifest_candidates(&mut entries, "skills", workspace_root);
    entries
}

fn manifest_entries_for_rules(scan: &RuleScan, workspace_root: &Path) -> Vec<FsManifestEntry> {
    let mut entries = Vec::new();
    for rule in &scan.rules {
        let Some(agent) = primary_rule_agent(rule) else {
            continue;
        };
        entries.push(manifest_entry_for_path(
            "rule",
            &rule.path,
            rule.path.parent().unwrap_or(workspace_root),
            Some(agent_label(agent).to_string()),
            Some(rule.scope.clone()),
            (!rule.sha256.is_empty()).then(|| rule.sha256.clone()),
            PROJECTION_PARSER_VERSION,
        ));
    }
    append_manifest_candidates(&mut entries, "rules", workspace_root);
    entries
}

fn manifest_entries_for_hooks(scan: &HookScan, workspace_root: &Path) -> Vec<FsManifestEntry> {
    let mut entries = Vec::new();
    for hook in &scan.hooks {
        entries.push(manifest_entry_for_path(
            "hook",
            &hook.path,
            hook.path.parent().unwrap_or(workspace_root),
            Some(agent_label(hook.agent).to_string()),
            None,
            (!hook.trust_hash.is_empty()).then(|| hook.trust_hash.clone()),
            PROJECTION_PARSER_VERSION,
        ));
    }
    append_manifest_candidates(&mut entries, "hooks", workspace_root);
    entries
}

fn manifest_entries_for_mcp(scan: &McpScan, workspace_root: &Path) -> Vec<FsManifestEntry> {
    let mut entries = Vec::new();
    for server in &scan.servers {
        entries.push(manifest_entry_for_path(
            "mcp",
            &server.path,
            server.path.parent().unwrap_or(workspace_root),
            Some(agent_label(server.agent).to_string()),
            Some(server.scope.clone()),
            None,
            PROJECTION_PARSER_VERSION,
        ));
    }
    append_manifest_candidates(&mut entries, "mcp", workspace_root);
    entries
}

fn session_search_metadata_document(session: &SessionRecord) -> SessionSearchDocument {
    SessionSearchDocument {
        metadata_text: format!(
            "{} {} {} {} {} {} {}",
            session.id,
            agent_label(session.agent),
            session.path.display(),
            session.model.as_deref().unwrap_or_default(),
            session.mode.as_deref().unwrap_or_default(),
            session.approval_mode.as_deref().unwrap_or_default(),
            session
                .is_run_everything
                .map(|value| value.to_string())
                .unwrap_or_default()
        ),
        title: session.title.clone().unwrap_or_default(),
        project: session
            .project
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        ..SessionSearchDocument::default()
    }
}

fn session_search_metadata(session: &SessionRecord) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        agent_label(session.agent),
        session.title.as_deref().unwrap_or(""),
        session
            .project
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        session.path.display(),
        session.model.as_deref().unwrap_or_default(),
        session.mode.as_deref().unwrap_or_default(),
        session.approval_mode.as_deref().unwrap_or_default(),
        session
            .is_run_everything
            .map(|value| value.to_string())
            .unwrap_or_default(),
    )
}

fn session_search_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|term| term.trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .collect()
}

fn session_search_query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn compact_search_snippet(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

const SEARCH_SNIPPET_LEADING_CONTEXT_CHARS: usize = 0;
const SEARCH_SNIPPET_TRAILING_CONTEXT_CHARS: usize = 80;

fn snippet_context_start(value: &str, end: usize) -> usize {
    if SEARCH_SNIPPET_LEADING_CONTEXT_CHARS == 0 {
        return end;
    }
    value[..end]
        .char_indices()
        .rev()
        .nth(SEARCH_SNIPPET_LEADING_CONTEXT_CHARS.saturating_sub(1))
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn snippet_context_end(value: &str, start: usize) -> usize {
    value[start..]
        .char_indices()
        .nth(SEARCH_SNIPPET_TRAILING_CONTEXT_CHARS)
        .map(|(index, _)| start + index)
        .unwrap_or(value.len())
}

fn contains_search_score(document: &SessionSearchDocument, terms: &[String]) -> f64 {
    terms
        .iter()
        .map(|term| {
            [
                (document.metadata_text.as_str(), 0.5),
                (document.title.as_str(), 10.0),
                (document.user_text.as_str(), 6.0),
                (document.project.as_str(), 5.0),
                (document.assistant_text.as_str(), 3.0),
            ]
            .into_iter()
            .find_map(|(value, weight)| value.to_lowercase().contains(term).then_some(weight))
            .unwrap_or(0.0)
        })
        .sum()
}

fn contains_search_snippet(document: &SessionSearchDocument, terms: &[String]) -> String {
    let candidates = [
        &document.title,
        &document.user_text,
        &document.project,
        &document.assistant_text,
        &document.metadata_text,
    ];
    for term in terms {
        for candidate in candidates {
            if let Some(snippet) = highlight_contains_match(candidate, term) {
                return compact_search_snippet(&snippet);
            }
        }
    }
    String::new()
}

fn highlight_contains_match(value: &str, term: &str) -> Option<String> {
    let match_start = value.to_lowercase().find(term)?;
    let match_end = match_start + term.len();
    if !value.is_char_boundary(match_start) || !value.is_char_boundary(match_end) {
        return Some(value.chars().take(160).collect());
    }

    let start = snippet_context_start(value, match_start);
    let end = snippet_context_end(value, match_end);
    Some(format!(
        "{}{}⟦{}⟧{}{}",
        if start > 0 && start < match_start {
            "… "
        } else {
            ""
        },
        &value[start..match_start],
        &value[match_start..match_end],
        &value[match_end..end],
        if end < value.len() { " …" } else { "" },
    ))
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn new_prompt_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("prompt-{nanos}")
}

fn normalize_prompt_tags(tags: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for tag in tags {
        let trimmed = tag.trim();
        if trimmed.is_empty() || normalized.iter().any(|value| value == trimmed) {
            continue;
        }
        normalized.push(trimmed.to_string());
    }
    normalized
}

fn parse_prompt_tags(tags_json: &str) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str::<Vec<String>>(tags_json)
        .map(normalize_prompt_tags)
        .map_err(|error| rusqlite::Error::FromSqlConversionFailure(2, Type::Text, Box::new(error)))
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
