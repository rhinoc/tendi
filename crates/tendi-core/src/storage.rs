use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

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

use crate::{
    HookScan, McpScan, RuleRecord, RuleScan, ScanReport,
    analytics::{
        self, AnalyticsCapabilities, AnalyticsCoverage, AnalyticsProviderCapability,
        AnalyticsRefreshProgress, AnalyticsRefreshReport, OverviewAnalytics,
        SessionAnalyticsOverviewRecord, SessionAnalyticsRecord,
    },
    fsutil::sha256_text,
    projects::{self, ProjectRecord, ProjectScanResult, ProjectScanScope},
    runtime_contract::{
        OperationId, OperationRecord, OperationStatus, ProjectionHead, Revision, ScopeKey,
        SourceVersion,
    },
    session_skills::{SessionFileState, SessionSkillIndexStatus, SessionSkillLink},
    sessions::{
        SESSION_PREVIEW_MAX_CHARS, SessionIdentity, SessionRecord, SessionScan, SessionScanCache,
        SessionScanCacheEntry, SessionScanSourceState, bound_session_preview, clean_session_title,
        normalize_session_projects,
    },
    skills::{
        AgentKind, SkillScan, SkillSnapshot, SkillSnapshotFile, SkillSourceRecord,
    },
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
const STORAGE_SCHEMA_VERSION: i64 = 1;
const SESSION_SEARCH_INDEX_VERSION: i64 = 2;
const PROJECTION_PARSER_VERSION: &str = "scan-v5";
const SESSION_PAYLOAD_PREVIEWS_MIGRATION_KEY: &str = "session_payload_previews_backfilled_v1";
const DATABASE_WRITE_LOCK_ATTEMPTS: usize = 100;
const DATABASE_WRITE_LOCK_RETRY: Duration = Duration::from_millis(50);
const DATABASE_READ_LOCK_ATTEMPTS: usize = 100;
const DATABASE_READ_LOCK_RETRY: Duration = Duration::from_millis(50);
const SCOPED_SESSION_TABLE: &str = "scoped_sessions";
const SCOPED_SESSION_SCAN_SOURCE_TABLE: &str = "scoped_session_scan_sources";
const SESSION_SCAN_CACHE_PARSER_VERSION: &str = "scan-v7";
const SESSION_SEARCH_CANDIDATE_TABLE: &str = "tendi_session_search_candidates";
const SESSION_SEARCH_MATCH_TABLE: &str = "tendi_session_search_matches";
const DEFAULT_SCOPE_KEY: &str = "installation:default";
const NORMALIZED_SNAPSHOT_TABLE: &str = "normalized_snapshots";
const SCOPED_PROJECTION_CONTEXT_TABLE: &str = "scoped_projection_contexts";

const PROJECTION_DOMAINS: [&str; 5] = ["agents", "skills", "rules", "hooks", "mcp"];

fn is_database_lock_error(error: &anyhow::Error) -> bool {
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

fn with_database_read_lock_retry<T, F>(mut read: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    for attempt in 0..DATABASE_READ_LOCK_ATTEMPTS {
        match read() {
            Ok(value) => return Ok(value),
            Err(error)
                if attempt + 1 < DATABASE_READ_LOCK_ATTEMPTS
                    && is_database_lock_error(&error) =>
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

fn workspace_scope_key(workspace_root: &Path) -> Result<ScopeKey> {
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
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnalyticsOverviewBackfillReport {
    pub processed: usize,
    pub failed: usize,
    pub remaining: usize,
    pub revision: u64,
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
    "sakura-pop".to_string()
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionProjectSummary {
    pub id: String,
    pub name: String,
    pub missing: bool,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
struct SessionSearchDocument {
    metadata_text: String,
    title: String,
    project: String,
    user_text: String,
    assistant_text: String,
}

impl Store {
    pub fn open_default() -> Result<Self> {
        Self::open(default_db_path()?)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let conn = Connection::open(&path)
            .with_context(|| format!("failed to open sqlite database {}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(30))?;
        let store = Self { conn, path };
        // Schema setup is a migration boundary, not part of every RPC open.
        // Re-running it made read requests contend with active scan writers.
        let schema_version = store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
        if schema_version != STORAGE_SCHEMA_VERSION {
            for attempt in 0..5 {
                match store.init() {
                    Ok(()) => break,
                    Err(error) if attempt < 4 && is_database_lock_error(&error) => {
                        std::thread::yield_now();
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn skill_backup_config(&self) -> Result<Option<crate::skill_backup::BackupConfig>> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'skill_backup_config'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        value
            .map(|value| serde_json::from_str(&value).context("invalid skill backup configuration"))
            .transpose()
    }

    pub fn save_skill_backup_config(
        &self,
        config: &crate::skill_backup::BackupConfig,
    ) -> Result<crate::skill_backup::BackupConfig> {
        config.validate()?;
        self.conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('skill_backup_config', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![serde_json::to_string(config)?],
        )?;
        Ok(config.clone())
    }

    pub fn clear_skill_backup_config(&self) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM app_settings WHERE key = 'skill_backup_config'", [])?
            > 0)
    }

    pub fn app_settings(&self) -> Result<AppSettings> {
        with_database_read_lock_retry(|| self.app_settings_once())
    }

    fn app_settings_once(&self) -> Result<AppSettings> {
        let appearance = self
            .conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'appearance'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(default_appearance);
        let appearance = normalize_appearance(&appearance)?;
        let font_family = self
            .conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'font_family'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(default_font_family);
        let font_family = normalize_font_family(&font_family)?;
        let light_theme = self
            .conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'light_theme'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(default_color_theme);
        let light_theme = normalize_color_theme(&light_theme)?;
        let dark_theme = self
            .conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'dark_theme'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(default_color_theme);
        let dark_theme = normalize_color_theme(&dark_theme)?;
        let app_icon = self
            .conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'app_icon'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(default_app_icon);
        let app_icon = normalize_color_theme(&app_icon)?;
        let terminal = self
            .conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'terminal'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| "auto".to_string());
        let session_resume_target = self
            .conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'session_resume_target'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| normalize_session_resume_target(&value))
            .transpose()?
            .unwrap_or_else(default_session_resume_target);
        let missing_session_project_policy = self
            .conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'missing_session_project_policy'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| normalize_missing_session_project_policy(&value))
            .transpose()?
            .unwrap_or_else(default_missing_session_project_policy);
        let editor = self
            .conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'editor'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(default_editor);
        let developer_mode = self
            .conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'developer_mode'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| serde_json::from_str::<bool>(&value))
            .transpose()
            .context("invalid developer mode setting")?
            .unwrap_or(false);
        let additional_session_roots = self
            .conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'additional_session_roots'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| serde_json::from_str::<Vec<String>>(&value))
            .transpose()
            .context("invalid additional session roots setting")?
            .unwrap_or_default();
        let config_profiles = self
            .conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'config_profiles'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| serde_json::from_str::<BTreeMap<String, String>>(&value))
            .transpose()
            .context("invalid config profiles setting")?
            .unwrap_or_default();
        Ok(AppSettings {
            appearance,
            font_family,
            light_theme,
            dark_theme,
            app_icon,
            terminal,
            session_resume_target,
            missing_session_project_policy,
            editor,
            developer_mode,
            additional_session_roots,
            config_profiles,
        })
    }

    pub fn save_app_settings(&self, settings: AppSettings) -> Result<AppSettings> {
        let appearance = normalize_appearance(&settings.appearance)?;
        let font_family = normalize_font_family(&settings.font_family)?;
        let light_theme = normalize_color_theme(&settings.light_theme)?;
        let dark_theme = normalize_color_theme(&settings.dark_theme)?;
        let app_icon = normalize_color_theme(&settings.app_icon)?;
        let terminal = normalize_setting_value(&settings.terminal, "auto");
        let session_resume_target =
            normalize_session_resume_target(&settings.session_resume_target)?;
        let missing_session_project_policy =
            normalize_missing_session_project_policy(&settings.missing_session_project_policy)?;
        let editor = normalize_setting_value(&settings.editor, "vscode");
        let developer_mode = settings.developer_mode;
        let additional_session_roots =
            normalize_additional_session_roots(settings.additional_session_roots)?;
        let config_profiles = settings
            .config_profiles
            .into_iter()
            .filter_map(|(agent, profile)| {
                let agent = agent.trim();
                let profile = profile.trim();
                (!agent.is_empty() && !profile.is_empty())
                    .then(|| (agent.to_string(), profile.to_string()))
            })
            .collect::<BTreeMap<_, _>>();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES ('appearance', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![appearance],
        )?;
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES ('font_family', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![font_family],
        )?;
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES ('light_theme', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![light_theme],
        )?;
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES ('dark_theme', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![dark_theme],
        )?;
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES ('app_icon', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![app_icon],
        )?;
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES ('terminal', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![terminal],
        )?;
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES ('session_resume_target', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![session_resume_target],
        )?;
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES ('missing_session_project_policy', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![missing_session_project_policy],
        )?;
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES ('editor', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![editor],
        )?;
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES ('developer_mode', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![serde_json::to_string(&developer_mode)?],
        )?;
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES ('additional_session_roots', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![serde_json::to_string(&additional_session_roots)?],
        )?;
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES ('config_profiles', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![serde_json::to_string(&config_profiles)?],
        )?;
        tx.commit()?;
        Ok(AppSettings {
            appearance,
            font_family,
            light_theme,
            dark_theme,
            app_icon,
            terminal,
            session_resume_target,
            missing_session_project_policy,
            editor,
            developer_mode,
            additional_session_roots,
            config_profiles,
        })
    }

    pub fn project_scan_scopes(&self) -> Result<Vec<ProjectScanScope>> {
        with_database_read_lock_retry(|| self.project_scan_scopes_once())
    }

    fn project_scan_scopes_once(&self) -> Result<Vec<ProjectScanScope>> {
        let mut statement = self.conn.prepare(
            "SELECT id, path, enabled, last_scanned_at
             FROM project_scan_scopes
             ORDER BY path ASC",
        )?;
        let rows = statement.query_map([], |row| {
            let id: String = row.get(0)?;
            let stored_path: String = row.get(1)?;
            let excluded = stored_path.starts_with('!');
            let path = PathBuf::from(stored_path.strip_prefix('!').unwrap_or(&stored_path));
            let project_count = self.conn.query_row(
                "SELECT COUNT(*) FROM projects WHERE scope_id = ?1 AND status = 'ready'",
                [&id],
                |count| count.get::<_, usize>(0),
            )?;
            Ok(ProjectScanScope {
                id,
                path,
                excluded,
                enabled: row.get::<_, i64>(2)? != 0,
                last_scanned_at: row.get(3)?,
                project_count,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn save_project_scan_scopes(&self, values: Vec<String>) -> Result<Vec<ProjectScanScope>> {
        let paths = projects::normalize_scope_paths(values)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("UPDATE project_scan_scopes SET enabled = 0", [])?;
        for scope in paths {
            let id = projects::scope_id_for_path(&scope.path, scope.excluded);
            let stored_path = if scope.excluded {
                format!("!{}", scope.path.display())
            } else {
                scope.path.to_string_lossy().into_owned()
            };
            tx.execute(
                "INSERT INTO project_scan_scopes (id, path, enabled)
                 VALUES (?1, ?2, 1)
                 ON CONFLICT(id) DO UPDATE SET path = excluded.path, enabled = 1",
                params![id, stored_path],
            )?;
        }
        tx.commit()?;
        self.project_scan_scopes()
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRecord>> {
        with_database_read_lock_retry(|| self.list_projects_once())
    }

    fn list_projects_once(&self) -> Result<Vec<ProjectRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT data_json FROM projects
             WHERE status = 'ready'
             ORDER BY name COLLATE NOCASE ASC, root_path ASC",
        )?;
        let rows = statement.query_map([], |row| {
            let data: String = row.get(0)?;
            serde_json::from_str::<ProjectRecord>(&data).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn scan_projects(&self) -> Result<ProjectScanResult> {
        let scopes = self.project_scan_scopes()?;
        let exclusion_matcher = projects::build_exclusion_matcher(&scopes)?;
        let mut scanned_projects = BTreeMap::new();
        let mut warnings = Vec::new();
        for scope in scopes
            .iter()
            .filter(|scope| scope.enabled && !scope.excluded)
        {
            let (projects, scope_warnings) =
                projects::scan_scope(&scope.path, &scope.id, &exclusion_matcher);
            for project in projects {
                scanned_projects
                    .entry(project.id.clone())
                    .or_insert(project);
            }
            warnings.extend(scope_warnings);
        }
        let scanned_projects = scanned_projects.into_values().collect::<Vec<_>>();

        let tx = self.conn.unchecked_transaction()?;
        let existing_projects = self.list_projects()?;
        for scope in scopes
            .iter()
            .filter(|scope| scope.enabled && !scope.excluded)
        {
            tx.execute(
                "UPDATE projects SET status = 'missing'
                 WHERE scope_id = ?1 AND status = 'ready'",
                [&scope.id],
            )?;
        }
        for project in existing_projects.iter().filter(|project| {
            projects::path_is_excluded(&exclusion_matcher, &project.root_path, true)
        }) {
            tx.execute(
                "UPDATE projects SET status = 'out-of-scope' WHERE id = ?1",
                [&project.id],
            )?;
        }
        for scope in scopes.iter().filter(|scope| !scope.enabled) {
            tx.execute(
                "UPDATE projects SET status = 'out-of-scope'
                 WHERE scope_id = ?1 AND status != 'out-of-scope'",
                [&scope.id],
            )?;
        }
        for project in &scanned_projects {
            tx.execute(
                "INSERT INTO projects (
                    id, root_path, name, remote_url, scope_id, status,
                    last_scanned_at, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                    root_path = excluded.root_path,
                    name = excluded.name,
                    remote_url = excluded.remote_url,
                    scope_id = excluded.scope_id,
                    status = excluded.status,
                    last_scanned_at = excluded.last_scanned_at,
                    data_json = excluded.data_json",
                params![
                    project.id,
                    project.root_path.to_string_lossy(),
                    project.name,
                    project.remote_url,
                    project.scope_id,
                    project.status,
                    project.last_scanned_at,
                    serde_json::to_string(project)?,
                ],
            )?;
        }
        for scope in scopes.iter().filter(|scope| scope.enabled) {
            tx.execute(
                "UPDATE project_scan_scopes
                 SET last_scanned_at = ?2
                 WHERE id = ?1",
                params![scope.id, Local::now().to_rfc3339()],
            )?;
        }
        tx.commit()?;

        Ok(ProjectScanResult {
            projects: self.list_projects()?,
            scopes: self.project_scan_scopes()?,
            warnings,
        })
    }

    /// Persist a complete scan and bind all filesystem-backed projections to
    /// one canonical workspace context.
    pub fn save_scan_for_workspace(
        &self,
        workspace_root: &Path,
        report: &ScanReport,
    ) -> Result<()> {
        let workspace_root = canonical_workspace_root(workspace_root);
        let scope_key = workspace_scope_key(&workspace_root)?;
        let tx = self.conn.unchecked_transaction()?;
        if report.skills.warnings.is_empty() {
            self.insert_skill_source_records_if_missing_for_workspace_in_tx(
                &tx,
                &scope_key,
                &report.skill_source_migrations,
            )?;
            self.write_normalized_snapshot_in_tx(&tx, &scope_key, "skills", &report.skills)?;
        }
        if report.agents.warnings.is_empty() {
            self.write_normalized_snapshot_in_tx(&tx, &scope_key, "agents", &report.agents)?;
        }
        if report.rules.warnings.is_empty() {
            self.write_normalized_snapshot_in_tx(&tx, &scope_key, "rules", &report.rules)?;
        }
        if report.hooks.warnings.is_empty() {
            self.write_normalized_snapshot_in_tx(&tx, &scope_key, "hooks", &report.hooks)?;
        }
        if report.mcp.warnings.is_empty() {
            self.write_normalized_snapshot_in_tx(&tx, &scope_key, "mcp", &report.mcp)?;
        }
        if report.sessions.warnings.is_empty() {
            self.save_sessions_at_with_scope_in_tx(
                &tx,
                &report.sessions,
                unix_now(),
                &scope_key,
            )?;
            let normalized_sessions = SessionScan {
                sessions: report
                    .sessions
                    .sessions
                    .iter()
                    .cloned()
                    .map(Self::normalize_cached_session)
                    .collect(),
                warnings: report.sessions.warnings.clone(),
            };
            self.write_normalized_snapshot_in_tx(&tx, &scope_key, "sessions", &normalized_sessions)?;
            let session_projects = Self::session_project_summaries_from_sessions(
                &normalized_sessions.sessions,
            );
            self.write_normalized_snapshot_in_tx(&tx, &scope_key, "session_projects", &session_projects)?;
        }
        for (domain, ready, error) in [
            (
                "agents",
                report.agents.warnings.is_empty(),
                report.agents.warnings.join("; "),
            ),
            (
                "skills",
                report.skills.warnings.is_empty(),
                report.skills.warnings.join("; "),
            ),
            (
                "rules",
                report.rules.warnings.is_empty(),
                report.rules.warnings.join("; "),
            ),
            (
                "hooks",
                report.hooks.warnings.is_empty(),
                report.hooks.warnings.join("; "),
            ),
            (
                "mcp",
                report.mcp.warnings.is_empty(),
                report.mcp.warnings.join("; "),
            ),
        ] {
            let entries = match domain {
                "agents" => manifest_entries_for_agents(&report.agents, &workspace_root),
                "skills" => manifest_entries_for_skills(&report.skills, &workspace_root),
                "rules" => manifest_entries_for_rules(&report.rules, &workspace_root),
                "hooks" => manifest_entries_for_hooks(&report.hooks, &workspace_root),
                "mcp" => manifest_entries_for_mcp(&report.mcp, &workspace_root),
                _ => unreachable!("projection domain is validated by the match above"),
            };
            self.finalize_projection_domain_in_tx(
                &tx,
                domain,
                &workspace_root,
                &entries,
                ready,
                (!ready).then_some(error),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_fs_manifest(&self, entry: &FsManifestEntry) -> Result<()> {
        self.upsert_fs_manifest_entries(std::slice::from_ref(entry))?;
        Ok(())
    }

    pub fn upsert_fs_manifest_entries(&self, entries: &[FsManifestEntry]) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }

        let tx = self.conn.unchecked_transaction()?;
        for entry in entries {
            tx.execute(
                "INSERT INTO fs_manifest (
                    scope_key, source_kind, path, root, agent, scope, mtime_ns, size, inode, device,
                    sha256, parser_version, last_seen_at, parse_status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(scope_key, source_kind, path) DO UPDATE SET
                    root = excluded.root,
                    agent = excluded.agent,
                    scope = excluded.scope,
                    mtime_ns = excluded.mtime_ns,
                    size = excluded.size,
                    inode = excluded.inode,
                    device = excluded.device,
                    sha256 = excluded.sha256,
                    parser_version = excluded.parser_version,
                    last_seen_at = excluded.last_seen_at,
                    parse_status = excluded.parse_status",
                params![
                    DEFAULT_SCOPE_KEY,
                    entry.source_kind,
                    entry.path.display().to_string(),
                    entry.root.display().to_string(),
                    entry.agent,
                    entry.scope,
                    entry.mtime_ns,
                    entry.size,
                    entry.inode,
                    entry.device,
                    entry.sha256,
                    entry.parser_version,
                    entry.last_seen_at,
                    entry.parse_status,
                ],
            )?;
        }
        tx.commit()?;
        Ok(entries.len())
    }


    pub fn fs_manifest_entry(
        &self,
        source_kind: &str,
        path: &Path,
    ) -> Result<Option<FsManifestEntry>> {
        self.conn
            .query_row(
                "SELECT source_kind, path, root, agent, scope, mtime_ns, size, inode, device,
                        sha256, parser_version, last_seen_at, parse_status
                 FROM fs_manifest
                 WHERE scope_key = ?1 AND source_kind = ?2 AND path = ?3",
                params![DEFAULT_SCOPE_KEY, source_kind, path.display().to_string()],
                fs_manifest_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_fs_manifest_for_root(&self, root: &Path) -> Result<Vec<FsManifestEntry>> {
        let scope_key = workspace_scope_key(&workspace_root_for_manifest_root(root))?;
        let mut statement = self.conn.prepare(
            "SELECT source_kind, path, root, agent, scope, mtime_ns, size, inode, device,
                    sha256, parser_version, last_seen_at, parse_status
             FROM fs_manifest
             WHERE scope_key = ?1 AND root = ?2
             ORDER BY source_kind ASC, path ASC",
        )?;
        let rows = statement.query_map(
            params![scope_key.as_str(), root.display().to_string()],
            fs_manifest_from_row,
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn delete_fs_manifest_entry(&self, source_kind: &str, path: &Path) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM fs_manifest WHERE scope_key = ?1 AND source_kind = ?2 AND path = ?3",
            params![DEFAULT_SCOPE_KEY, source_kind, path.display().to_string()],
        )? > 0)
    }

    pub fn delete_fs_manifest_missing_under_root(
        &self,
        source_kind: &str,
        root: &Path,
        seen_paths: &[PathBuf],
    ) -> Result<usize> {
        let root_key = root.display().to_string();
        let existing_paths = {
            let mut statement = self.conn.prepare(
                "SELECT path
                 FROM fs_manifest
                 WHERE scope_key = ?1 AND source_kind = ?2 AND root = ?3",
            )?;
            let rows = statement.query_map(params![DEFAULT_SCOPE_KEY, source_kind, root_key], |row| {
                row.get::<_, String>(0)
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let seen_paths = seen_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<HashSet<_>>();

        let tx = self.conn.unchecked_transaction()?;
        let mut deleted = 0;
        for path in existing_paths {
            if !seen_paths.contains(&path) {
                deleted += tx.execute(
                    "DELETE FROM fs_manifest
                     WHERE scope_key = ?1 AND source_kind = ?2 AND path = ?3 AND root = ?4",
                    params![DEFAULT_SCOPE_KEY, source_kind, path, root_key],
                )?;
            }
        }
        tx.commit()?;
        Ok(deleted)
    }

    fn list_fs_manifest_for_domain(
        &self,
        domain: &str,
        scope_key: &ScopeKey,
    ) -> Result<Vec<FsManifestEntry>> {
        let kinds = manifest_source_kinds(domain)?;
        let placeholders = std::iter::repeat_n("?", kinds.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT source_kind, path, root, agent, scope, mtime_ns, size, inode, device,
                    sha256, parser_version, last_seen_at, parse_status
             FROM fs_manifest
             WHERE scope_key = ?1
               AND source_kind IN ({placeholders})
             ORDER BY source_kind ASC, path ASC"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let mut values = vec![SqlValue::Text(scope_key.as_str().to_string())];
        values.extend(kinds.iter().map(|kind| SqlValue::Text((*kind).to_string())));
        let rows = statement.query_map(params_from_iter(values.iter()), fs_manifest_from_row)?;
        let entries = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    fn finalize_projection_domain_in_tx(
        &self,
        tx: &Transaction<'_>,
        domain: &str,
        workspace_root: &Path,
        entries: &[FsManifestEntry],
        ready: bool,
        error: Option<String>,
    ) -> Result<()> {
        let kinds = manifest_source_kinds(domain)?;
        let scope_key = workspace_scope_key(workspace_root)?;
        let delete_sql = format!(
            "DELETE FROM fs_manifest
             WHERE scope_key = ?1 AND source_kind IN ({})",
            std::iter::repeat_n("?", kinds.len())
                .enumerate()
                .map(|(index, _)| format!("?{}", index + 2))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut delete_params = vec![SqlValue::Text(scope_key.as_str().to_string())];
        delete_params.extend(kinds.iter().map(|kind| SqlValue::Text((*kind).to_string())));
        tx.execute(&delete_sql, params_from_iter(delete_params.iter()))?;
        for entry in entries {
            tx.execute(
                "INSERT INTO fs_manifest (
                    scope_key, source_kind, path, root, agent, scope, mtime_ns, size, inode, device,
                    sha256, parser_version, last_seen_at, parse_status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(scope_key, source_kind, path) DO UPDATE SET
                    root = excluded.root,
                    agent = excluded.agent,
                    scope = excluded.scope,
                    mtime_ns = excluded.mtime_ns,
                    size = excluded.size,
                    inode = excluded.inode,
                    device = excluded.device,
                    sha256 = excluded.sha256,
                    parser_version = excluded.parser_version,
                    last_seen_at = excluded.last_seen_at,
                    parse_status = excluded.parse_status",
                params![
                    scope_key.as_str(),
                    entry.source_kind,
                    entry.path.display().to_string(),
                    entry.root.display().to_string(),
                    entry.agent,
                    entry.scope,
                    entry.mtime_ns,
                    entry.size,
                    entry.inode,
                    entry.device,
                    entry.sha256,
                    entry.parser_version,
                    entry.last_seen_at,
                    entry.parse_status,
                ],
            )?;
        }
        self.set_projection_context_in_tx(
            tx,
            &scope_key,
            domain,
            if ready { "ready" } else { "failed" },
            error,
        )?;
        let source_version = SourceVersion::new(PROJECTION_PARSER_VERSION)
            .map_err(|error| anyhow::anyhow!(error))?;
        advance_projection_head_in_tx(
            tx,
            &scope_key,
            domain,
            Some(&source_version),
            if ready { "ready" } else { "failed" },
        )?;
        self.sync_normalized_snapshot_revision_in_tx(tx, &scope_key, domain)?;
        Ok(())
    }

    fn set_projection_context(
        &self,
        domain: &str,
        workspace_root: &Path,
        state: &str,
        error: Option<String>,
    ) -> Result<()> {
        ensure_projection_domain(domain)?;
        let workspace_root = canonical_workspace_root(workspace_root);
        let scope_key = ScopeKey::new(format!("workspace:{}", workspace_root.display()))
            .map_err(|error| anyhow::anyhow!(error))?;
        let tx = self.conn.unchecked_transaction()?;
        self.set_projection_context_in_tx(&tx, &scope_key, domain, state, error)?;
        tx.commit()?;
        Ok(())
    }

    fn set_projection_context_in_tx(
        &self,
        tx: &Transaction<'_>,
        scope_key: &ScopeKey,
        domain: &str,
        state: &str,
        error: Option<String>,
    ) -> Result<()> {
        ensure_projection_domain(domain)?;
        tx.execute(
            &format!("INSERT INTO {SCOPED_PROJECTION_CONTEXT_TABLE}
                (scope_key, domain, state, scanned_at, error, parser_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(scope_key, domain) DO UPDATE SET
                state = excluded.state,
                scanned_at = excluded.scanned_at,
                error = excluded.error,
                parser_version = excluded.parser_version"),
            params![
                scope_key.as_str(),
                domain,
                state,
                (state == "ready").then(|| unix_now() as i64),
                error,
                PROJECTION_PARSER_VERSION,
            ],
        )?;
        Ok(())
    }

    fn write_normalized_snapshot<T: Serialize>(
        &self,
        scope_key: &ScopeKey,
        domain: &str,
        payload: &T,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        self.write_normalized_snapshot_in_tx(&tx, scope_key, domain, payload)?;
        tx.commit()?;
        Ok(())
    }

    fn write_normalized_snapshot_in_tx<T: Serialize>(
        &self,
        tx: &Transaction<'_>,
        scope_key: &ScopeKey,
        domain: &str,
        payload: &T,
    ) -> Result<()> {
        let payload_json = serde_json::to_string(payload)?;
        let revision = tx
            .query_row(
                "SELECT revision FROM projection_heads
                 WHERE scope_key = ?1 AND domain = ?2",
                params![scope_key.as_str(), domain],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(Revision::ZERO.value() as i64);
        tx.execute(
            &format!("INSERT INTO {NORMALIZED_SNAPSHOT_TABLE}
                (scope_key, domain, payload_json, source_version, revision, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(scope_key, domain) DO UPDATE SET
                payload_json = excluded.payload_json,
                source_version = excluded.source_version,
                revision = excluded.revision,
                updated_at = excluded.updated_at"),
            params![
                scope_key.as_str(),
                domain,
                payload_json,
                PROJECTION_PARSER_VERSION,
                revision,
                unix_now() as i64,
            ],
        )?;
        Ok(())
    }

    fn sync_normalized_snapshot_revision_in_tx(
        &self,
        tx: &Transaction<'_>,
        scope_key: &ScopeKey,
        domain: &str,
    ) -> Result<()> {
        let Some(revision) = tx
            .query_row(
                "SELECT revision FROM projection_heads
                 WHERE scope_key = ?1 AND domain = ?2",
                params![scope_key.as_str(), domain],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        else {
            return Ok(());
        };
        tx.execute(
            &format!(
                "UPDATE {NORMALIZED_SNAPSHOT_TABLE}
                 SET revision = ?1, updated_at = ?2
                 WHERE scope_key = ?3 AND domain = ?4"
            ),
            params![revision, unix_now() as i64, scope_key.as_str(), domain],
        )?;
        Ok(())
    }

    fn read_normalized_snapshot<T: for<'de> Deserialize<'de>>(
        &self,
        scope_key: &ScopeKey,
        domain: &str,
    ) -> Result<Option<T>> {
        with_database_read_lock_retry(|| self.read_normalized_snapshot_once(scope_key, domain))
    }

    fn read_normalized_snapshot_once<T: for<'de> Deserialize<'de>>(
        &self,
        scope_key: &ScopeKey,
        domain: &str,
    ) -> Result<Option<T>> {
        self.conn
            .query_row(
                &format!("SELECT payload_json FROM {NORMALIZED_SNAPSHOT_TABLE}
                 WHERE scope_key = ?1 AND domain = ?2"),
                params![scope_key.as_str(), domain],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|payload| {
                serde_json::from_str(&payload)
                    .with_context(|| format!("invalid {domain} normalized snapshot"))
            })
            .transpose()
    }

    fn lock_file_path(&self, name: &str) -> PathBuf {
        let database_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("tendi.sqlite3");
        self.path
            .with_file_name(format!("{database_name}.{name}.lock"))
    }

    fn try_acquire_file_lock(&self, name: &str) -> Result<Option<File>> {
        let path = self.lock_file_path(name);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to open lock file {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => {
                Err(error).with_context(|| format!("failed to lock file {}", path.display()))
            }
        }
    }

    pub fn list_sessions_for_scope(&self, scope_key: &ScopeKey) -> Result<SessionScan> {
        with_database_read_lock_retry(|| self.list_sessions_for_scope_once(scope_key))
    }

    fn list_sessions_for_scope_once(&self, scope_key: &ScopeKey) -> Result<SessionScan> {
        if let Some(snapshot) = self.read_normalized_snapshot(scope_key, "sessions")? {
            return Ok(snapshot);
        }
        self.list_sessions_from_table(scope_key)
    }

    pub fn normalized_snapshot_json_for_scope(
        &self,
        scope_key: &ScopeKey,
        domain: &str,
    ) -> Result<Option<String>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT payload_json FROM {NORMALIZED_SNAPSHOT_TABLE}
                     WHERE scope_key = ?1 AND domain = ?2"
                ),
                params![scope_key.as_str(), domain],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(Into::into)
    }

    fn refresh_scoped_session_snapshot(&self, scope_key: &ScopeKey) -> Result<()> {
        let snapshot = self.list_sessions_from_table(scope_key)?;
        self.write_normalized_snapshot(scope_key, "sessions", &snapshot)
    }

    fn list_sessions_from_table(&self, scope_key: &ScopeKey) -> Result<SessionScan> {
        // `data_json` is the SessionRecord authority. The scalar session columns are
        // denormalized projections retained for compatibility and write-side indexing.
        let mut stmt = self.conn.prepare(
            "SELECT data_json FROM scoped_sessions WHERE scope_key = ?1",
        )?;
        let mut sessions = Vec::new();
        let mut warnings = Vec::new();
        let mut rows = stmt.query(params![scope_key.as_str()])?;
        while let Some(row) = rows.next()? {
            let data_json = row.get::<_, String>(0)?;
            match serde_json::from_str::<SessionRecord>(&data_json) {
                Ok(session) => sessions.push(Self::normalize_cached_session(session)),
                Err(err) => warnings.push(format!("invalid cached session row: {err}")),
            }
        }

        sessions.sort_by(Self::compare_session_updated_at);
        Ok(SessionScan { sessions, warnings })
    }

    /// Return logical projects observed in one workspace. The serialized
    /// summary is a workspace snapshot; the old global project tables are only
    /// consulted to enrich paths for the migration period.
    pub fn list_session_projects_for_scope(
        &self,
        scope_key: &ScopeKey,
    ) -> Result<Vec<SessionProjectSummary>> {
        with_database_read_lock_retry(|| self.list_session_projects_for_scope_once(scope_key))
    }

    fn list_session_projects_for_scope_once(
        &self,
        scope_key: &ScopeKey,
    ) -> Result<Vec<SessionProjectSummary>> {
        if let Some(snapshot) = self.read_normalized_snapshot(scope_key, "session_projects")? {
            return Ok(snapshot);
        }
        self.session_project_summaries_for_scope(scope_key)
    }

    fn session_project_summaries_for_scope(
        &self,
        scope_key: &ScopeKey,
    ) -> Result<Vec<SessionProjectSummary>> {
        let sessions = self
            .list_sessions_from_table(scope_key)?
            .sessions;
        Ok(Self::session_project_summaries_from_sessions(&sessions))
    }

    fn session_project_summaries_from_sessions(
        sessions: &[SessionRecord],
    ) -> Vec<SessionProjectSummary> {
        let mut summaries = BTreeMap::<String, SessionProjectSummary>::new();
        for session in sessions {
            let Some(id) = session.logical_project_id.as_ref() else {
                continue;
            };
            let entry = summaries.entry(id.clone()).or_insert_with(|| SessionProjectSummary {
                id: id.clone(),
                name: session
                    .logical_project_name
                    .clone()
                    .unwrap_or_else(|| "Unnamed project".to_string()),
                missing: true,
                paths: Vec::new(),
            });
            if let Some(path) = &session.project {
                entry.paths.push(path.clone());
            }
            if let Some(name) = &session.logical_project_name {
                entry.name = name.clone();
            }
        }
        for summary in summaries.values_mut() {
            summary.paths.sort();
            summary.paths.dedup();
            summary.missing = !summary.paths.iter().any(|path| path.is_dir());
        }
        summaries.into_values().collect()
    }

    fn refresh_scoped_session_project_snapshot(&self, scope_key: &ScopeKey) -> Result<()> {
        let summaries = self.session_project_summaries_for_scope(scope_key)?;
        self.write_normalized_snapshot(scope_key, "session_projects", &summaries)
    }

    pub fn projection_status(
        &self,
        domain: &str,
        workspace_root: &Path,
    ) -> Result<ProjectionStatus> {
        ensure_projection_domain(domain)?;
        let workspace_root = canonical_workspace_root(workspace_root);
        let scope_key = ScopeKey::new(format!("workspace:{}", workspace_root.display()))
            .map_err(|error| anyhow::anyhow!(error))?;
        let context = self
            .conn
            .query_row(
                &format!("SELECT state
                 FROM {SCOPED_PROJECTION_CONTEXT_TABLE}
                 WHERE scope_key = ?1 AND domain = ?2"),
                params![scope_key.as_str(), domain],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(state) = context else {
            return Ok(ProjectionStatus::Missing);
        };
        if state == "refreshing" {
            return Ok(ProjectionStatus::Refreshing);
        }
        if state != "ready"
            || self
                .read_normalized_snapshot::<serde_json::Value>(&scope_key, domain)?
                .is_none()
        {
            return Ok(ProjectionStatus::Stale);
        }

        let entries = self.list_fs_manifest_for_domain(domain, &scope_key)?;
        if entries.is_empty() {
            return Ok(ProjectionStatus::Missing);
        }
        if domain == "skills"
            && entries
                .iter()
                .any(|entry| entry.parser_version != PROJECTION_PARSER_VERSION)
        {
            Ok(ProjectionStatus::Stale)
        } else if entries.iter().all(manifest_entry_is_current) {
            Ok(ProjectionStatus::Fresh)
        } else {
            Ok(ProjectionStatus::Stale)
        }
    }

    pub fn list_agents_for_workspace(
        &self,
        workspace_root: &Path,
    ) -> Result<Option<crate::agents::AgentScan>> {
        if self.projection_status("agents", workspace_root)? != ProjectionStatus::Fresh {
            return Ok(None);
        }
        let scope_key = workspace_scope_key(workspace_root)?;
        self.read_normalized_snapshot(&scope_key, "agents")
    }

    pub fn list_skills_for_workspace(&self, workspace_root: &Path) -> Result<Option<SkillScan>> {
        if self.projection_status("skills", workspace_root)? != ProjectionStatus::Fresh {
            return Ok(None);
        }
        let scope_key = workspace_scope_key(workspace_root)?;
        self.read_normalized_snapshot(&scope_key, "skills")
    }

    /// Return the last cached skill projection for this workspace without checking
    /// filesystem freshness. Read-only surfaces use this while a refresh is owned
    /// by the startup or explicit refresh lifecycle.
    pub fn list_skills_cached_for_workspace(
        &self,
        workspace_root: &Path,
    ) -> Result<Option<SkillScan>> {
        let workspace_root = canonical_workspace_root(workspace_root);
        let scope_key = workspace_scope_key(&workspace_root)?;
        let context = self
            .conn
            .query_row(
                &format!("SELECT state, parser_version
                 FROM {SCOPED_PROJECTION_CONTEXT_TABLE}
                 WHERE scope_key = ?1 AND domain = 'skills'"),
                params![scope_key.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                    ))
                },
            )
            .optional()?;
        let Some((state, parser_version)) = context else {
            return Ok(None);
        };
        if !matches!(state.as_str(), "ready" | "stale" | "failed")
            || parser_version != PROJECTION_PARSER_VERSION
        {
            return Ok(None);
        }
        self.read_normalized_snapshot(&scope_key, "skills")
    }

    /// Return the cached skill projection when the explicitly selected skills
    /// are still fresh. Unselected skill files are intentionally ignored here:
    /// callers using this path only need a stable update plan for `names`.
    pub fn list_skills_for_names_if_current(
        &self,
        workspace_root: &Path,
        names: &[String],
    ) -> Result<Option<SkillScan>> {
        let selected_names = names.iter().cloned().collect::<BTreeSet<_>>();
        if selected_names.is_empty() {
            return Ok(None);
        }

        let workspace_root = canonical_workspace_root(workspace_root);
        let scope_key = workspace_scope_key(&workspace_root)?;
        let context = self
            .conn
            .query_row(
                &format!("SELECT state, parser_version
                 FROM {SCOPED_PROJECTION_CONTEXT_TABLE}
                 WHERE scope_key = ?1 AND domain = 'skills'"),
                params![scope_key.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                    ))
                },
            )
            .optional()?;
        let Some((state, parser_version)) = context else {
            return Ok(None);
        };
        if state != "ready"
            || parser_version != PROJECTION_PARSER_VERSION
        {
            return Ok(None);
        }

        let entries = self.list_fs_manifest_for_domain("skills", &scope_key)?;
        if entries.is_empty() {
            return Ok(None);
        }
        if entries.iter().any(|entry| {
            entry.source_kind != "skill"
                && (entry.parser_version != PROJECTION_PARSER_VERSION
                    || !manifest_entry_is_current(entry))
        }) {
            return Ok(None);
        }

        let Some(scan) = self.read_normalized_snapshot::<SkillScan>(&scope_key, "skills")? else {
            return Ok(None);
        };
        let selected = scan
            .skills
            .iter()
            .filter(|skill| {
                selected_names
                    .iter()
                    .any(|name| crate::skills::skill_matches_selector(skill, name))
            })
            .collect::<Vec<_>>();
        if selected.len() != selected_names.len() {
            return Ok(None);
        }

        for skill in selected {
            for path in &skill.paths {
                let skill_file = path.path.join("SKILL.md");
                let Some(entry) = entries
                    .iter()
                    .find(|entry| entry.source_kind == "skill" && entry.path == skill_file)
                else {
                    return Ok(None);
                };
                if entry.parser_version != PROJECTION_PARSER_VERSION
                    || !manifest_entry_is_current(entry)
                {
                    return Ok(None);
                }
            }
        }

        Ok(Some(scan))
    }

    pub fn list_rules_for_workspace(&self, workspace_root: &Path) -> Result<Option<RuleScan>> {
        if self.projection_status("rules", workspace_root)? != ProjectionStatus::Fresh {
            return Ok(None);
        }
        let scope_key = workspace_scope_key(workspace_root)?;
        self.read_normalized_snapshot(&scope_key, "rules")
    }

    pub fn list_hooks_for_workspace(&self, workspace_root: &Path) -> Result<Option<HookScan>> {
        if self.projection_status("hooks", workspace_root)? != ProjectionStatus::Fresh {
            return Ok(None);
        }
        let scope_key = workspace_scope_key(workspace_root)?;
        self.read_normalized_snapshot(&scope_key, "hooks")
    }

    pub fn list_mcp_for_workspace(&self, workspace_root: &Path) -> Result<Option<McpScan>> {
        if self.projection_status("mcp", workspace_root)? != ProjectionStatus::Fresh {
            return Ok(None);
        }
        let scope_key = workspace_scope_key(workspace_root)?;
        self.read_normalized_snapshot(&scope_key, "mcp")
    }

    /// Mark one domain stale after an external writer changed its source.
    /// The next list call will run only that domain's scanner.
    pub fn invalidate_projection(&self, domain: &str, workspace_root: &Path) -> Result<()> {
        ensure_projection_domain(domain)?;
        self.set_projection_context(
            domain,
            &canonical_workspace_root(workspace_root),
            "stale",
            None,
        )
    }

    /// Coordinate all database writers across Tauri and CLI processes.
    /// `None` means another process owns the database write lock.
    /// Projection refreshes use their domain-specific lock instead.
    pub fn with_database_write_lock<T, F>(&self, write: F) -> Result<Option<T>>
    where
        F: FnOnce() -> Result<T>,
    {
        let lock_started = Instant::now();
        let Some(lock_file) = self.try_acquire_file_lock("database-write")? else {
            return Ok(None);
        };
        crate::logging::global().debug(
            "database write lock acquired",
            serde_json::json!({
                "lockWaitMs": lock_started.elapsed().as_secs_f64() * 1000.0,
                "lock": "database-write",
            }),
        );

        let result = write();
        drop(lock_file);
        result.map(Some)
    }

    pub(crate) fn with_database_write_lock_retry<T, F>(&self, mut write: F) -> Result<T>
    where
        F: FnMut() -> Result<T>,
    {
        for attempt in 0..DATABASE_WRITE_LOCK_ATTEMPTS {
            if let Some(value) = self.with_database_write_lock(&mut write)? {
                return Ok(value);
            }
            if attempt + 1 < DATABASE_WRITE_LOCK_ATTEMPTS {
                std::thread::sleep(DATABASE_WRITE_LOCK_RETRY);
            }
        }
        anyhow::bail!("timed out waiting for the database write lock")
    }

    /// Coordinate cold-start and targeted scans across Tauri and CLI
    /// processes. Each projection domain has an independent OS-backed lock.
    /// The callback runs outside the lock transaction; its persistence methods
    /// own their short database write transactions. The lock file handle is
    /// tied to this process, so an interrupted refresh releases the lock when
    /// the process exits.
    pub fn with_projection_refresh_lock<T, F>(&self, domain: &str, refresh: F) -> Result<Option<T>>
    where
        F: FnOnce() -> Result<T>,
    {
        ensure_projection_domain(domain)?;
        let Some(lock_file) = self.try_acquire_file_lock(&format!("projection-{domain}"))? else {
            return Ok(None);
        };

        let result = refresh();
        drop(lock_file);
        result.map(Some)
    }

    pub fn save_agents_for_workspace(
        &self,
        workspace_root: &Path,
        scan: &crate::agents::AgentScan,
    ) -> Result<()> {
        let workspace_root = canonical_workspace_root(workspace_root);
        self.save_projection_domain_snapshot(
            &workspace_root,
            "agents",
            scan.warnings.is_empty().then_some(scan),
            &manifest_entries_for_agents(scan, &workspace_root),
            scan.warnings.is_empty(),
            (!scan.warnings.is_empty()).then(|| scan.warnings.join("; ")),
            &[],
        )
    }

    pub fn save_skills_for_workspace(&self, workspace_root: &Path, scan: &SkillScan) -> Result<()> {
        let workspace_root = canonical_workspace_root(workspace_root);
        self.save_projection_domain_snapshot(
            &workspace_root,
            "skills",
            scan.warnings.is_empty().then_some(scan),
            &manifest_entries_for_skills(scan, &workspace_root),
            scan.warnings.is_empty(),
            (!scan.warnings.is_empty()).then(|| scan.warnings.join("; ")),
            &[],
        )
    }

    pub fn save_skills_for_workspace_with_source_migrations(
        &self,
        workspace_root: &Path,
        scan: &SkillScan,
        source_migrations: &[SkillSourceRecord],
    ) -> Result<()> {
        let workspace_root = canonical_workspace_root(workspace_root);
        self.save_projection_domain_snapshot(
            &workspace_root,
            "skills",
            scan.warnings.is_empty().then_some(scan),
            &manifest_entries_for_skills(scan, &workspace_root),
            scan.warnings.is_empty(),
            (!scan.warnings.is_empty()).then(|| scan.warnings.join("; ")),
            source_migrations,
        )
    }

    pub fn save_rules_for_workspace(&self, workspace_root: &Path, scan: &RuleScan) -> Result<()> {
        let workspace_root = canonical_workspace_root(workspace_root);
        self.save_projection_domain_snapshot(
            &workspace_root,
            "rules",
            scan.warnings.is_empty().then_some(scan),
            &manifest_entries_for_rules(scan, &workspace_root),
            scan.warnings.is_empty(),
            (!scan.warnings.is_empty()).then(|| scan.warnings.join("; ")),
            &[],
        )
    }

    pub fn save_hooks_for_workspace(&self, workspace_root: &Path, scan: &HookScan) -> Result<()> {
        let workspace_root = canonical_workspace_root(workspace_root);
        self.save_projection_domain_snapshot(
            &workspace_root,
            "hooks",
            scan.warnings.is_empty().then_some(scan),
            &manifest_entries_for_hooks(scan, &workspace_root),
            scan.warnings.is_empty(),
            (!scan.warnings.is_empty()).then(|| scan.warnings.join("; ")),
            &[],
        )
    }

    pub fn save_mcp_for_workspace(&self, workspace_root: &Path, scan: &McpScan) -> Result<()> {
        let workspace_root = canonical_workspace_root(workspace_root);
        self.save_projection_domain_snapshot(
            &workspace_root,
            "mcp",
            scan.warnings.is_empty().then_some(scan),
            &manifest_entries_for_mcp(scan, &workspace_root),
            scan.warnings.is_empty(),
            (!scan.warnings.is_empty()).then(|| scan.warnings.join("; ")),
            &[],
        )
    }

    fn save_projection_domain_snapshot<T: Serialize>(
        &self,
        workspace_root: &Path,
        domain: &str,
        snapshot: Option<&T>,
        entries: &[FsManifestEntry],
        ready: bool,
        error: Option<String>,
        source_migrations: &[SkillSourceRecord],
    ) -> Result<()> {
        let scope_key = workspace_scope_key(workspace_root)?;
        let tx = self.conn.unchecked_transaction()?;
        self.insert_skill_source_records_if_missing_for_workspace_in_tx(
            &tx,
            &scope_key,
            source_migrations,
        )?;
        if let Some(snapshot) = snapshot {
            self.write_normalized_snapshot_in_tx(&tx, &scope_key, domain, snapshot)?;
        }
        self.finalize_projection_domain_in_tx(
            &tx,
            domain,
            workspace_root,
            entries,
            ready,
            error,
        )?;
        tx.commit()?;
        Ok(())
    }


    fn compare_session_updated_at(
        left: &SessionRecord,
        right: &SessionRecord,
    ) -> std::cmp::Ordering {
        compare_timestamps(right.updated_at.as_deref(), left.updated_at.as_deref())
            .then_with(|| left.id.cmp(&right.id))
    }

    fn normalize_cached_session(mut session: SessionRecord) -> SessionRecord {
        session.title = clean_session_title(session.title.take());
        session.first_user_message = bound_session_preview(session.first_user_message.take());
        session.last_user_message = bound_session_preview(session.last_user_message.take());
        session.last_assistant_message =
            bound_session_preview(session.last_assistant_message.take());
        session
    }

    fn session_metadata_json(session: &SessionRecord) -> Result<String> {
        let mut metadata = session.clone();
        metadata.title = clean_session_title(metadata.title.take());
        metadata.first_user_message = bound_session_preview(metadata.first_user_message.take());
        metadata.last_user_message = bound_session_preview(metadata.last_user_message.take());
        metadata.last_assistant_message =
            bound_session_preview(metadata.last_assistant_message.take());
        Ok(serde_json::to_string(&metadata)?)
    }

    pub fn resolve_session_projects_for_scope(
        &self,
        scope_key: &ScopeKey,
        sessions: &mut [SessionRecord],
    ) -> Result<()> {
        normalize_session_projects(sessions);
        let tx = self.conn.unchecked_transaction()?;
        self.resolve_session_projects_in_tx(&tx, sessions, scope_key)?;
        tx.commit()?;
        self.refresh_scoped_session_snapshot(scope_key)?;
        self.refresh_scoped_session_project_snapshot(scope_key)?;
        Ok(())
    }

    fn resolve_session_projects_in_tx(
        &self,
        tx: &Transaction<'_>,
        sessions: &mut [SessionRecord],
        scope_key: &ScopeKey,
    ) -> Result<()> {
        let mut projects = load_session_projects(tx, scope_key)?;
        let mut aliases = load_session_project_aliases(tx, scope_key)?;

        for session in sessions.iter_mut() {
            let evidence = session_project_aliases(session);
            if evidence.is_empty() {
                session.logical_project_id = None;
                session.logical_project_name = None;
                continue;
            }

            let mut candidates = evidence
                .iter()
                .filter_map(|alias| aliases.get(alias).cloned())
                .collect::<BTreeSet<_>>();
            let project_id = if candidates.is_empty() {
                let seed = evidence
                    .iter()
                    .find(|(kind, _)| kind == "repository_url")
                    .unwrap_or(&evidence[0]);
                let Some(name) = suggested_project_name(session) else {
                    session.logical_project_id = None;
                    session.logical_project_name = None;
                    continue;
                };
                let project_id = format!(
                    "project-{}",
                    &sha256_text(&format!("{}:{}", seed.0, seed.1))[..24]
                );
                let last_seen_at = session_project_seen_at(session);
                tx.execute(
                    "INSERT OR IGNORE INTO session_projects
                        (scope_key, id, name, name_custom, last_seen_at)
                     VALUES (?1, ?2, ?3, 0, ?4)",
                    params![scope_key.as_str(), project_id, name, last_seen_at],
                )?;
                projects.entry(project_id.clone()).or_insert(ProjectState {
                    name,
                    name_custom: false,
                    last_seen_at,
                });
                project_id
            } else {
                let target = candidates
                    .iter()
                    .max_by(|left, right| {
                        let left_seen_at = projects
                            .get(*left)
                            .map(|project| project.last_seen_at.as_str());
                        let right_seen_at = projects
                            .get(*right)
                            .map(|project| project.last_seen_at.as_str());
                        compare_timestamps(left_seen_at, right_seen_at)
                    })
                    .cloned()
                    .context("session project alias references a missing project")?;
                candidates.remove(&target);
                for source in candidates {
                    merge_session_project_rows(tx, &target, &source, scope_key)?;
                    for alias_project_id in aliases.values_mut() {
                        if *alias_project_id == source {
                            *alias_project_id = target.clone();
                        }
                    }
                    projects.remove(&source);
                }
                target
            };

            for alias in evidence {
                if aliases.get(&alias) != Some(&project_id) {
                    tx.execute(
                        "INSERT OR REPLACE INTO session_project_aliases
                            (scope_key, project_id, kind, value)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![scope_key.as_str(), project_id, alias.0, alias.1],
                    )?;
                    aliases.insert(alias, project_id.clone());
                }
            }

            let seen_at = session_project_seen_at(session);
            let suggested_name = suggested_project_name(session);
            if let Some(project) = projects.get_mut(&project_id) {
                if compare_timestamps(
                    Some(seen_at.as_str()),
                    Some(project.last_seen_at.as_str()),
                )
                .is_gt()
                {
                    project.last_seen_at = seen_at.clone();
                    if !project.name_custom {
                        if let Some(suggested_name) = suggested_name {
                            project.name = suggested_name;
                        }
                    }
                    tx.execute(
                        "UPDATE session_projects
                         SET name = ?3, last_seen_at = ?4
                         WHERE scope_key = ?1 AND id = ?2",
                        params![
                            scope_key.as_str(),
                            project_id,
                            project.name,
                            project.last_seen_at
                        ],
                    )?;
                }
                session.logical_project_id = Some(project_id);
                session.logical_project_name = Some(project.name.clone());
            }
        }

        for session in sessions.iter() {
            let data_json = Self::session_metadata_json(session)?;
            tx.execute(
                &format!(
                    "UPDATE {SCOPED_SESSION_TABLE}
                     SET data_json = ?1
                     WHERE scope_key = ?2 AND id = ?3 AND agent = ?4 AND path = ?5"
                ),
                params![
                    data_json,
                    scope_key.as_str(),
                    session.id,
                    agent_label(session.agent),
                    session.path.display().to_string(),
                ],
            )?;
            index_scoped_session_search_document_best_effort(tx, scope_key, session);
        }
        Ok(())
    }

    pub fn refresh_session_analytics_for_scope_with_progress<F>(
        &self,
        scope_key: &ScopeKey,
        sessions: &[SessionRecord],
        mut on_progress: F,
    ) -> Result<AnalyticsRefreshReport>
    where
        F: FnMut(AnalyticsRefreshProgress),
    {
        let mut warnings = Vec::new();
        let mut report = AnalyticsRefreshReport {
            total: sessions.len(),
            parsed: 0,
            appended: 0,
            skipped: 0,
            failed: 0,
            warnings: Vec::new(),
        };
        on_progress(analytics_refresh_progress(&report, 0));

        for (batch_index, batch) in sessions.chunks(SESSION_ANALYTICS_BATCH_SIZE).enumerate() {
            let mut updates = Vec::new();
            {
                let mut state_stmt = self.conn.prepare(
                    "SELECT file_mtime, file_size, parser_state_json
                     FROM scoped_session_analytics
                     WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
                )?;
                let mut cache_stmt = self.conn.prepare(
                    "SELECT file_mtime, file_size, analytics_json, parser_state_json
                     FROM scoped_session_analytics
                     WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
                )?;

                for session in batch {
                    let session_path = session.path.display().to_string();
                    let cached_state = state_stmt
                        .query_row(
                            params![
                                scope_key.as_str(),
                                session.id,
                                agent_label(session.agent),
                                session_path,
                            ],
                            |row| {
                                Ok((
                                    row.get::<_, i64>(0)?,
                                    row.get::<_, i64>(1)?,
                                    row.get::<_, String>(2)?,
                                ))
                            },
                        )
                        .optional()?;
                    let file_state = match crate::session_skills::session_file_state(&session.path)
                    {
                        Ok(state) => state,
                        Err(err) => {
                            report.failed += 1;
                            warnings.push(format!(
                                "analytics skipped {} {}: {err}",
                                agent_label(session.agent),
                                session.path.display()
                            ));
                            continue;
                        }
                    };
                    if cached_state.is_some_and(|(file_mtime, file_size, parser_state_json)| {
                        file_mtime == file_state.file_mtime
                            && file_size == file_state.file_size
                            && analytics::parser_state_is_current(&parser_state_json)
                    }) {
                        report.skipped += 1;
                        continue;
                    }
                    let cached_row = cache_stmt
                        .query_row(
                            params![
                                scope_key.as_str(),
                                session.id,
                                agent_label(session.agent),
                                session.path.display().to_string(),
                            ],
                            |row| {
                                Ok((
                                    row.get::<_, i64>(0)?,
                                    row.get::<_, i64>(1)?,
                                    row.get::<_, String>(2)?,
                                    row.get::<_, String>(3)?,
                                ))
                            },
                        )
                        .optional()?;
                    let cached = cached_row.and_then(
                        |(file_mtime, file_size, analytics_json, parser_state_json)| match (
                            serde_json::from_str(&analytics_json),
                            serde_json::from_str(&parser_state_json),
                        ) {
                            (Ok(analytics), Ok(state)) => Some(SessionAnalyticsRecord {
                                analytics,
                                state,
                                file_mtime,
                                file_size,
                            }),
                            (Err(err), _) | (_, Err(err)) => {
                                warnings.push(format!(
                                    "invalid scoped analytics cache row for {}: {err}",
                                    session.path.display()
                                ));
                                None
                            }
                        },
                    );
                    let was_append = cached.as_ref().is_some_and(|record| {
                        record.file_size >= 0 && record.file_size < file_state.file_size
                    });
                    match analytics::analyze_session(session, cached.as_ref()) {
                        Ok(record) => {
                            report.parsed += 1;
                            report.appended += usize::from(was_append);
                            updates.push(record);
                        }
                        Err(err) => {
                            report.failed += 1;
                            warnings.push(format!(
                                "analytics failed {} {}: {err}",
                                agent_label(session.agent),
                                session.path.display()
                            ));
                        }
                    }
                }
            }

            self.save_session_analytics_records_for_scope(scope_key, &updates)?;
            let completed = ((batch_index + 1) * SESSION_ANALYTICS_BATCH_SIZE).min(sessions.len());
            on_progress(analytics_refresh_progress(&report, completed));
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM scoped_session_analytics_overview
             WHERE scope_key = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM scoped_sessions
                   WHERE scoped_sessions.scope_key = scoped_session_analytics_overview.scope_key
                     AND scoped_sessions.id = scoped_session_analytics_overview.session_id
                     AND scoped_sessions.agent = scoped_session_analytics_overview.agent
                     AND scoped_sessions.path = scoped_session_analytics_overview.session_path
               )",
            [scope_key.as_str()],
        )?;
        tx.execute(
            "DELETE FROM scoped_session_analytics
             WHERE scope_key = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM scoped_sessions
                   WHERE scoped_sessions.scope_key = scoped_session_analytics.scope_key
                     AND scoped_sessions.id = scoped_session_analytics.session_id
                     AND scoped_sessions.agent = scoped_session_analytics.agent
                     AND scoped_sessions.path = scoped_session_analytics.session_path
               )",
            [scope_key.as_str()],
        )?;
        tx.commit()?;
        report.warnings = warnings;
        Ok(report)
    }

    fn save_session_analytics_records_for_scope(
        &self,
        scope_key: &ScopeKey,
        records: &[SessionAnalyticsRecord],
    ) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        for record in records {
            let overview = record.analytics.overview_index(&record.state);
            let overview_record = analytics::overview_record(record);
            tx.execute(
                "INSERT INTO scoped_session_analytics (
                    scope_key, session_id, agent, session_path, file_mtime, file_size,
                    indexed_at, analytics_json, parser_state_json,
                    event_min_date, event_max_date, has_activity,
                    capability_token_usage, capability_reasoning_tokens,
                    capability_explicit_runs, capability_rate_limit_history,
                    overview_indexed, overview_index_error
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 1, NULL)
                 ON CONFLICT(scope_key, session_id, agent, session_path) DO UPDATE SET
                    file_mtime = excluded.file_mtime,
                    file_size = excluded.file_size,
                    indexed_at = excluded.indexed_at,
                    analytics_json = excluded.analytics_json,
                    parser_state_json = excluded.parser_state_json,
                    event_min_date = excluded.event_min_date,
                    event_max_date = excluded.event_max_date,
                    has_activity = excluded.has_activity,
                    capability_token_usage = excluded.capability_token_usage,
                    capability_reasoning_tokens = excluded.capability_reasoning_tokens,
                    capability_explicit_runs = excluded.capability_explicit_runs,
                    capability_rate_limit_history = excluded.capability_rate_limit_history,
                    overview_indexed = 1,
                    overview_index_error = NULL",
                params![
                    scope_key.as_str(),
                    record.analytics.session_id,
                    agent_label(record.analytics.agent),
                    record.analytics.session_path.display().to_string(),
                    record.file_mtime,
                    record.file_size,
                    unix_now().to_string(),
                    serde_json::to_string(&record.analytics)?,
                    serde_json::to_string(&record.state)?,
                    overview.first,
                    overview.last,
                    overview.has_activity,
                    overview.capabilities.token_usage,
                    overview.capabilities.reasoning_tokens,
                    overview.capabilities.explicit_runs,
                    overview.capabilities.rate_limit_history,
                ],
            )?;
            tx.execute(
                "INSERT INTO scoped_session_analytics_overview (
                    scope_key, session_id, agent, session_path, event_min_date,
                    event_max_date, has_activity, overview_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(scope_key, session_id, agent, session_path) DO UPDATE SET
                    event_min_date = excluded.event_min_date,
                    event_max_date = excluded.event_max_date,
                    has_activity = excluded.has_activity,
                    overview_json = excluded.overview_json",
                params![
                    scope_key.as_str(),
                    &overview_record.session_id,
                    agent_label(overview_record.agent),
                    overview_record.session_path.display().to_string(),
                    &overview_record.first,
                    &overview_record.last,
                    overview_record.has_activity,
                    serde_json::to_string(&overview_record)?,
                ],
            )?;
        }
        let source_version = SourceVersion::new(PROJECTION_PARSER_VERSION)
            .map_err(|error| anyhow::anyhow!(error))?;
        advance_projection_head_in_tx(
            &tx,
            scope_key,
            "analytics",
            Some(&source_version),
            "ready",
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Return analytics for one workspace only. Scoped analytics are stored in
    /// their own composite-key projection, so the workspace boundary is
    /// enforced by SQL rather than by filtering a global cache in memory.
    pub fn overview_analytics_for_scope(
        &self,
        scope_key: &ScopeKey,
        agent: Option<AgentKind>,
        days: u32,
        rank_days: u32,
    ) -> Result<OverviewAnalytics> {
        let scoped_sessions = self.list_sessions_for_scope(scope_key)?.sessions;
        let allowed = scoped_sessions
            .iter()
            .filter(|session| agent.is_none_or(|expected| session.agent == expected))
            .map(|session| {
                (
                    session.id.clone(),
                    session.agent,
                    session.path.clone(),
                )
            })
            .collect::<HashSet<_>>();
        let today = chrono::Local::now().date_naive();
        let days = days.clamp(1, 365);
        let rank_days = rank_days.clamp(1, 730);
        let since = today - chrono::Duration::days(i64::from(days.saturating_sub(1)));
        let rank_since = today - chrono::Duration::days(i64::from(rank_days.saturating_sub(1)));
        let cutoff = since.min(rank_since).to_string();
        let (records, warnings) = self.load_session_analytics_overview_records_for_scope(
            scope_key,
            agent,
            &cutoff,
        )?;
        let mut overview =
            analytics::aggregate_overview_records(&records, days, rank_days, warnings);
        let mut capabilities = BTreeMap::<AgentKind, AnalyticsCapabilities>::new();
        for record in &records {
            let provider_capabilities = AnalyticsCapabilities::for_agent(record.agent);
            let entry = capabilities
                .entry(record.agent)
                .or_insert(provider_capabilities);
            entry.token_usage |= provider_capabilities.token_usage;
            entry.reasoning_tokens |= provider_capabilities.reasoning_tokens;
            entry.explicit_runs |= provider_capabilities.explicit_runs;
            entry.duration |= provider_capabilities.duration;
            entry.rate_limit_history |= provider_capabilities.rate_limit_history;
        }
        let total_sessions = allowed.len();
        let agent_value = agent.map(agent_label);
        let (first, last, indexed_sessions, analyzed_sessions) = self.conn.query_row(
            "SELECT MIN(overview.event_min_date),
                    MAX(overview.event_max_date),
                    COUNT(*),
                    COALESCE(SUM(CASE WHEN overview.has_activity THEN 1 ELSE 0 END), 0)
             FROM scoped_session_analytics_overview AS overview
             WHERE overview.scope_key = ?1
               AND (?2 IS NULL OR overview.agent = ?2)
               AND EXISTS (
                   SELECT 1 FROM scoped_sessions AS session
                   WHERE session.scope_key = overview.scope_key
                     AND session.id = overview.session_id
                     AND session.agent = overview.agent
                     AND session.path = overview.session_path
               )",
            params![scope_key.as_str(), agent_value],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)? as usize,
                    row.get::<_, i64>(3)? as usize,
                ))
            },
        )?;
        overview.revision = self
            .projection_head(scope_key, "analytics")?
            .map(|head| head.revision.value())
            .unwrap_or(0);
        overview.coverage = AnalyticsCoverage {
            first,
            last,
            total_sessions,
            analyzed_sessions,
            indexing_sessions: total_sessions.saturating_sub(indexed_sessions),
        };
        overview.capabilities = capabilities
            .into_iter()
            .map(|(agent, capabilities)| AnalyticsProviderCapability {
                agent,
                capabilities,
            })
            .collect();
        overview.warnings.extend(
            self.analytics_overview_index_warnings_for_scope(scope_key, agent)?,
        );
        Ok(overview)
    }

    fn load_session_analytics_overview_records_for_scope(
        &self,
        scope_key: &ScopeKey,
        agent: Option<AgentKind>,
        cutoff: &str,
    ) -> Result<(Vec<SessionAnalyticsOverviewRecord>, Vec<String>)> {
        let agent_value = agent.map(agent_label);
        let mut records = Vec::new();
        let mut warnings = Vec::new();
        let mut invalid = false;
        let mut stmt = self.conn.prepare(
            "SELECT overview_json
             FROM scoped_session_analytics_overview
             WHERE scope_key = ?1
               AND (?2 IS NULL OR agent = ?2)
               AND event_max_date >= ?3",
        )?;
        let rows = stmt.query_map(params![scope_key.as_str(), agent_value, cutoff], |row| {
            row.get::<_, String>(0)
        })?;
        for row in rows {
            match serde_json::from_str::<SessionAnalyticsOverviewRecord>(&row?) {
                Ok(record) => records.push(record),
                Err(error) => {
                    invalid = true;
                    warnings.push(format!("invalid scoped analytics overview cache row: {error}"));
                }
            }
        }
        drop(stmt);

        if invalid {
            records.clear();
            let mut stmt = self.conn.prepare(
                "SELECT analytics_json, parser_state_json
                 FROM scoped_session_analytics
                 WHERE scope_key = ?1
                   AND (?2 IS NULL OR agent = ?2)
                   AND overview_indexed = 1
                   AND event_max_date >= ?3",
            )?;
            let rows = stmt.query_map(params![scope_key.as_str(), agent_value, cutoff], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (analytics_json, parser_state_json) = row?;
                match (
                    serde_json::from_str::<crate::analytics::SessionAnalytics>(&analytics_json),
                    serde_json::from_str::<crate::analytics::AnalyticsParserState>(&parser_state_json),
                ) {
                    (Ok(analytics), Ok(state)) => {
                        records.push(analytics::overview_record(&SessionAnalyticsRecord {
                            analytics,
                            state,
                            file_mtime: 0,
                            file_size: 0,
                        }));
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        warnings.push(format!("invalid scoped analytics cache row: {error}"));
                    }
                }
            }
        }
        Ok((records, warnings))
    }

    pub fn analytics_revision(&self) -> Result<u64> {
        with_database_read_lock_retry(|| self.analytics_revision_once())
    }

    fn analytics_revision_once(&self) -> Result<u64> {
        self.conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'analytics_revision'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| value.parse::<u64>().context("invalid analytics revision"))
            .transpose()
            .map(|value| value.unwrap_or(0))
    }

    fn analytics_overview_index_warnings_for_scope(
        &self,
        scope_key: &ScopeKey,
        agent: Option<AgentKind>,
    ) -> Result<Vec<String>> {
        let agent_value = agent.map(agent_label);
        let mut stmt = self.conn.prepare(
            "SELECT session_path, overview_index_error
             FROM scoped_session_analytics
             WHERE overview_index_error IS NOT NULL
               AND scope_key = ?1
               AND (?2 IS NULL OR agent = ?2)",
        )?;
        stmt.query_map(params![scope_key.as_str(), agent_value], |row| {
            Ok(format!(
                "analytics index unavailable for {}: {}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
    }

    pub fn backfill_session_analytics_overview_index_batch(
        &self,
        limit: usize,
    ) -> Result<AnalyticsOverviewBackfillReport> {
        let limit = limit.max(1).min(256) as i64;
        let mut stmt = self.conn.prepare(
            "SELECT session_id, agent, session_path, analytics_json, parser_state_json
             FROM session_analytics
             WHERE overview_indexed = 0
             ORDER BY session_id, agent, session_path
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        let tx = self.conn.unchecked_transaction()?;
        let mut processed = 0;
        let mut failed = 0;
        for (session_id, agent, session_path, analytics_json, parser_state_json) in rows {
            let indexed = serde_json::from_str(&analytics_json).and_then(
                |analytics: crate::analytics::SessionAnalytics| {
                    serde_json::from_str(&parser_state_json)
                        .map(|state| analytics.overview_index(&state))
                },
            );
            let changed = match indexed {
                Ok(index) => tx.execute(
                    "UPDATE session_analytics
                     SET event_min_date = ?1, event_max_date = ?2, has_activity = ?3,
                         capability_token_usage = ?4, capability_reasoning_tokens = ?5,
                         capability_explicit_runs = ?6,
                         capability_rate_limit_history = ?7,
                         overview_indexed = 1, overview_index_error = NULL
                     WHERE session_id = ?8 AND agent = ?9 AND session_path = ?10
                       AND overview_indexed = 0",
                    params![
                        index.first,
                        index.last,
                        index.has_activity,
                        index.capabilities.token_usage,
                        index.capabilities.reasoning_tokens,
                        index.capabilities.explicit_runs,
                        index.capabilities.rate_limit_history,
                        session_id,
                        agent,
                        session_path,
                    ],
                )?,
                Err(error) => {
                    failed += 1;
                    tx.execute(
                        "UPDATE session_analytics
                         SET overview_indexed = 1, overview_index_error = ?1
                         WHERE session_id = ?2 AND agent = ?3 AND session_path = ?4
                           AND overview_indexed = 0",
                        params![error.to_string(), session_id, agent, session_path],
                    )?
                }
            };
            processed += changed;
        }
        if processed > 0 {
            bump_analytics_revision(&tx)?;
        }
        tx.commit()?;
        let remaining = self.conn.query_row(
            "SELECT COUNT(*) FROM session_analytics WHERE overview_indexed = 0",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        Ok(AnalyticsOverviewBackfillReport {
            processed,
            failed,
            remaining,
            revision: self.analytics_revision()?,
        })
    }

    pub fn backfill_session_analytics_overview_batch(
        &self,
        limit: usize,
    ) -> Result<AnalyticsOverviewBackfillReport> {
        let limit = limit.max(1).min(256) as i64;
        let mut stmt = self.conn.prepare(
            "SELECT sa.file_mtime, sa.file_size, sa.analytics_json, sa.parser_state_json
             FROM session_analytics AS sa
             LEFT JOIN session_analytics_overview AS overview
               ON overview.session_id = sa.session_id
              AND overview.agent = sa.agent
              AND overview.session_path = sa.session_path
             WHERE overview.session_id IS NULL
             ORDER BY sa.session_id, sa.agent, sa.session_path
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        let tx = self.conn.unchecked_transaction()?;
        let mut processed = 0;
        let mut failed = 0;
        for (file_mtime, file_size, analytics_json, parser_state_json) in rows {
            let parsed = (
                serde_json::from_str::<crate::analytics::SessionAnalytics>(&analytics_json),
                serde_json::from_str::<crate::analytics::AnalyticsParserState>(&parser_state_json),
            );
            let overview = match parsed {
                (Ok(analytics), Ok(state)) => analytics::overview_record(&SessionAnalyticsRecord {
                    analytics,
                    state,
                    file_mtime,
                    file_size,
                }),
                (Err(_), _) | (_, Err(_)) => {
                    failed += 1;
                    continue;
                }
            };
            tx.execute(
                "INSERT INTO session_analytics_overview (
                    session_id, agent, session_path, event_min_date, event_max_date,
                    has_activity, overview_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(session_id, agent, session_path) DO UPDATE SET
                    event_min_date = excluded.event_min_date,
                    event_max_date = excluded.event_max_date,
                    has_activity = excluded.has_activity,
                    overview_json = excluded.overview_json",
                params![
                    &overview.session_id,
                    agent_label(overview.agent),
                    overview.session_path.display().to_string(),
                    &overview.first,
                    &overview.last,
                    overview.has_activity,
                    serde_json::to_string(&overview)?,
                ],
            )?;
            processed += 1;
        }
        tx.commit()?;
        let remaining = self.conn.query_row(
            "SELECT COUNT(*) FROM session_analytics AS sa
             LEFT JOIN session_analytics_overview AS overview
               ON overview.session_id = sa.session_id
              AND overview.agent = sa.agent
              AND overview.session_path = sa.session_path
             WHERE overview.session_id IS NULL",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        Ok(AnalyticsOverviewBackfillReport {
            processed,
            failed,
            remaining,
            revision: self.analytics_revision()?,
        })
    }


    pub fn session_scan_cache_for_scope(&self, scope_key: &ScopeKey) -> Result<SessionScanCache> {
        let sessions = self.list_sessions_for_scope(scope_key)?.sessions;
        let mut source_states_by_session: HashMap<
            (String, String, String),
            Vec<SessionScanSourceState>,
        > = HashMap::new();
        let mut statement = self.conn.prepare(&format!(
            "SELECT session_id, agent, session_path, source_path, file_mtime, file_size,
                    parser_version
             FROM {SCOPED_SESSION_SCAN_SOURCE_TABLE}
             WHERE scope_key = ?1"
        ))?;
        let rows = statement.query_map([scope_key.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                SessionScanSourceState {
                    path: PathBuf::from(row.get::<_, String>(3)?),
                    file_mtime: row.get(4)?,
                    file_size: row.get(5)?,
                },
                row.get::<_, String>(6)?,
            ))
        })?;
        for row in rows {
            let (session_id, agent, session_path, source, parser_version) = row?;
            if parser_version != SESSION_SCAN_CACHE_PARSER_VERSION {
                continue;
            }
            source_states_by_session
                .entry((session_id, agent, session_path))
                .or_default()
                .push(source);
        }
        drop(statement);

        let entries = sessions.into_iter().filter_map(|session| {
            let key = (
                session.id.clone(),
                agent_label(session.agent).to_string(),
                session.path.display().to_string(),
            );
            let source_states = source_states_by_session.remove(&key)?;
            let session_path = session.path.clone();
            let primary = source_states.iter().find(|source| source.path == session_path)?;
            let file_mtime = primary.file_mtime;
            let file_size = primary.file_size;
            Some(SessionScanCacheEntry {
                session,
                file_mtime,
                file_size,
                additional_file_states: source_states
                    .into_iter()
                    .filter(|source| source.path != session_path)
                    .collect(),
            })
        });
        Ok(SessionScanCache::from_entries(entries))
    }


    pub fn sessions_last_scan_at_for_scope(&self, scope_key: &ScopeKey) -> Result<Option<u64>> {
        self.sessions_last_scan_at_for_key(&format!("sessions_last_scan_at:{}", scope_key.as_str()))
    }

    fn sessions_last_scan_at_for_key(&self, key: &str) -> Result<Option<u64>> {
        self.conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| {
                value
                    .parse::<u64>()
                    .context("invalid sessions scan timestamp")
            })
            .transpose()
    }

    pub fn last_scan_at(&self) -> Result<Option<u64>> {
        self.conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_scan_at'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| value.parse::<u64>().context("invalid scan timestamp"))
            .transpose()
    }

    pub fn projection_head(
        &self,
        scope_key: &ScopeKey,
        domain: &str,
    ) -> Result<Option<ProjectionHead>> {
        let row = self
            .conn
            .query_row(
                "SELECT scope_key, domain, revision, source_version, schema_version, status
                 FROM projection_heads
                 WHERE scope_key = ?1 AND domain = ?2",
                params![scope_key.as_str(), domain],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(scope, domain, revision, source_version, schema_version, status)| {
            Ok(ProjectionHead {
                scope_key: ScopeKey::new(scope).map_err(|error| anyhow::anyhow!(error))?,
                domain,
                revision: Revision::new(
                    u64::try_from(revision).context("invalid projection revision")?,
                ),
                source_version: source_version
                    .map(SourceVersion::new)
                    .transpose()
                    .map_err(|error| anyhow::anyhow!(error))?,
                schema_version: u32::try_from(schema_version)
                    .context("invalid projection schema version")?,
                status,
            })
        })
        .transpose()
    }

    pub fn advance_projection_head(
        &self,
        scope_key: &ScopeKey,
        domain: &str,
        source_version: Option<&SourceVersion>,
        status: &str,
    ) -> Result<ProjectionHead> {
        if domain.trim().is_empty() {
            anyhow::bail!("projection domain must not be empty");
        }
        if status.trim().is_empty() {
            anyhow::bail!("projection status must not be empty");
        }
        let tx = self.conn.unchecked_transaction()?;
        let head = advance_projection_head_in_tx(
            &tx,
            scope_key,
            domain,
            source_version,
            status,
        )?;
        tx.commit()?;
        Ok(head)
    }

    pub fn record_operation(&self, operation: &OperationRecord) -> Result<()> {
        let now = i64::try_from(unix_now()).context("invalid operation timestamp")?;
        self.conn.execute(
            "INSERT INTO operation_journal
                (operation_id, kind, scope_key, status, input_revision, source_version,
                 checkpoint_json, error, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
             ON CONFLICT(operation_id) DO UPDATE SET
                kind = excluded.kind,
                scope_key = excluded.scope_key,
                status = excluded.status,
                input_revision = excluded.input_revision,
                source_version = excluded.source_version,
                checkpoint_json = excluded.checkpoint_json,
                error = excluded.error,
                updated_at = excluded.updated_at",
            params![
                operation.operation_id.as_str(),
                operation.kind.as_str(),
                operation.scope_key.as_str(),
                operation.status.as_str(),
                operation.input_revision.value(),
                operation.source_version.as_ref().map(SourceVersion::as_str),
                operation.checkpoint_json.as_deref(),
                operation.error.as_deref(),
                now,
            ],
        )?;
        Ok(())
    }

    pub fn update_operation(
        &self,
        operation_id: &OperationId,
        status: OperationStatus,
        checkpoint_json: Option<&str>,
        error: Option<&str>,
    ) -> Result<bool> {
        let updated = self.conn.execute(
            "UPDATE operation_journal
             SET status = ?2, checkpoint_json = ?3, error = ?4, updated_at = ?5
             WHERE operation_id = ?1",
            params![
                operation_id.as_str(),
                status.as_str(),
                checkpoint_json,
                error,
                i64::try_from(unix_now()).context("invalid operation timestamp")?,
            ],
        )?;
        Ok(updated > 0)
    }

    pub fn recover_inflight_operations(&self) -> Result<usize> {
        let updated = self.conn.execute(
            "UPDATE operation_journal
             SET status = 'failed',
                 error = COALESCE(error, ?1),
                 updated_at = ?2
             WHERE status IN ('queued', 'running', 'committing')",
            params![
                "daemon restarted before the operation completed",
                i64::try_from(unix_now()).context("invalid operation timestamp")?,
            ],
        )?;
        Ok(updated)
    }

    pub fn operation(&self, operation_id: &OperationId) -> Result<Option<OperationRecord>> {
        let row = self
            .conn
            .query_row(
                "SELECT operation_id, kind, scope_key, status, input_revision,
                        source_version, checkpoint_json, error
                 FROM operation_journal
                 WHERE operation_id = ?1",
                params![operation_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(
            operation_id,
            kind,
            scope_key,
            status,
            input_revision,
            source_version,
            checkpoint_json,
            error,
        )| {
            let kind = match kind.as_str() {
                "scan" => crate::runtime_contract::OperationKind::Scan,
                "watch" => crate::runtime_contract::OperationKind::Watch,
                "backfill" => crate::runtime_contract::OperationKind::Backfill,
                "analytics" => crate::runtime_contract::OperationKind::Analytics,
                "skill_update" => crate::runtime_contract::OperationKind::SkillUpdate,
                "projection" => crate::runtime_contract::OperationKind::Projection,
                other => anyhow::bail!("unknown operation kind: {other}"),
            };
            let status = match status.as_str() {
                "queued" => OperationStatus::Queued,
                "running" => OperationStatus::Running,
                "committing" => OperationStatus::Committing,
                "committed" => OperationStatus::Committed,
                "published" => OperationStatus::Published,
                "failed" => OperationStatus::Failed,
                "cancelled" => OperationStatus::Cancelled,
                "timed_out" => OperationStatus::TimedOut,
                "stale" => OperationStatus::Stale,
                other => anyhow::bail!("unknown operation status: {other}"),
            };
            Ok(OperationRecord {
                operation_id: OperationId::new(operation_id)
                    .map_err(|error| anyhow::anyhow!(error))?,
                kind,
                scope_key: ScopeKey::new(scope_key).map_err(|error| anyhow::anyhow!(error))?,
                status,
                input_revision: Revision::new(
                    u64::try_from(input_revision).context("invalid operation revision")?,
                ),
                source_version: source_version
                    .map(SourceVersion::new)
                    .transpose()
                    .map_err(|error| anyhow::anyhow!(error))?,
                checkpoint_json,
                error,
            })
        })
        .transpose()
    }


    pub fn apply_session_delta_for_scope(
        &self,
        scope_key: &ScopeKey,
        sessions: &[SessionRecord],
    ) -> Result<Vec<SessionRecord>> {
        let tx = self.conn.unchecked_transaction()?;
        let changed = Self::apply_session_delta_in_tx(
            &tx,
            sessions,
            scope_key,
        )?;
        if !changed.is_empty() {
            advance_projection_head_in_tx(&tx, scope_key, "sessions", None, "ready")?;
        }
        cleanup_stale_scoped_session_skill_rows(&tx, scope_key)?;
        cleanup_stale_scoped_session_scan_source_rows(&tx, scope_key)?;
        tx.commit()?;
        self.refresh_scoped_session_snapshot(scope_key)?;
        Ok(changed)
    }


    pub fn apply_session_delta_and_resolve_projects_for_scope(
        &self,
        scope_key: &ScopeKey,
        sessions: &[SessionRecord],
    ) -> Result<Vec<SessionRecord>> {
        let tx = self.conn.unchecked_transaction()?;
        let mut changed = Self::apply_session_delta_in_tx(
            &tx,
            sessions,
            scope_key,
        )?;
        self.resolve_session_projects_in_tx(
            &tx,
            &mut changed,
            scope_key,
        )?;
        if !changed.is_empty() {
            advance_projection_head_in_tx(&tx, scope_key, "sessions", None, "ready")?;
        }
        cleanup_stale_scoped_session_skill_rows(&tx, scope_key)?;
        cleanup_stale_scoped_session_scan_source_rows(&tx, scope_key)?;
        tx.commit()?;
        self.refresh_scoped_session_snapshot(scope_key)?;
        Ok(changed)
    }

    pub fn apply_session_changes_for_scope(
        &self,
        scope_key: &ScopeKey,
        sessions: &[SessionRecord],
        removed_paths: &[PathBuf],
    ) -> Result<(
        Vec<SessionRecord>,
        Vec<crate::sessions::SessionIdentity>,
    )> {
        let tx = self.conn.unchecked_transaction()?;
        let mut changed = Self::apply_session_delta_in_tx(
            &tx,
            sessions,
            scope_key,
        )?;
        self.resolve_session_projects_in_tx(
            &tx,
            &mut changed,
            scope_key,
        )?;
        let existing = Self::list_sessions_in_tx(&tx, scope_key)?;
        let removed = existing
            .into_iter()
            .filter(|session| {
                removed_paths
                    .iter()
                    .any(|path| session.path == *path || session.path.starts_with(path))
            })
            .collect::<Vec<_>>();
        for session in &removed {
            tx.execute(
                &format!(
                    "DELETE FROM {SCOPED_SESSION_TABLE}
                     WHERE scope_key = ?1 AND id = ?2 AND agent = ?3 AND path = ?4"
                ),
                params![
                    scope_key.as_str(),
                    session.id,
                    agent_label(session.agent),
                    session.path.display().to_string(),
                ],
            )?;
        }
        if !changed.is_empty() || !removed.is_empty() {
            advance_projection_head_in_tx(&tx, scope_key, "sessions", None, "ready")?;
        }
        cleanup_stale_scoped_session_skill_rows(&tx, scope_key)?;
        cleanup_stale_scoped_session_scan_source_rows(&tx, scope_key)?;
        tx.commit()?;
        self.refresh_scoped_session_snapshot(scope_key)?;
        Ok((
            changed,
            removed
                .iter()
                .map(crate::sessions::SessionIdentity::from)
                .collect(),
        ))
    }

    fn apply_session_delta_in_tx(
        tx: &Transaction<'_>,
        sessions: &[SessionRecord],
        scope_key: &ScopeKey,
    ) -> Result<Vec<SessionRecord>> {
        let mut changed = Vec::new();
        for session in sessions {
            let agent = agent_label(session.agent);
            let existing_session = tx
                .query_row(
                    "SELECT data_json FROM scoped_sessions
                     WHERE scope_key = ?1 AND id = ?2 AND agent = ?3 LIMIT 1",
                    params![scope_key.as_str(), session.id, agent],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .and_then(|data_json| serde_json::from_str::<SessionRecord>(&data_json).ok());
            let mut canonical = session.clone();
            if let Some(existing) = existing_session.as_ref() {
                let existing_is_transcript = existing
                    .path
                    .extension()
                    .is_some_and(|extension| extension == "jsonl");
                let incoming_is_transcript = canonical
                    .path
                    .extension()
                    .is_some_and(|extension| extension == "jsonl");
                if existing_is_transcript && !incoming_is_transcript {
                    canonical.path = existing.path.clone();
                    if canonical.token_usage.is_none() {
                        canonical.token_usage = existing.token_usage.clone();
                    }
                }
            }
            canonical.title = clean_session_title(canonical.title.take());
            let path = canonical.path.display().to_string();
            let data_json = Self::session_metadata_json(&canonical)?;
            let title = canonical.title.clone();
            let project = canonical
                .project
                .as_ref()
                .map(|path| path.display().to_string());
            let started_at = canonical.started_at.clone();
            let updated_at = canonical.updated_at.clone();
            let message_count = canonical.message_count.map(|value| value as i64);
            let first_user_message = bound_session_preview(canonical.first_user_message.clone());
            let last_user_message = bound_session_preview(canonical.last_user_message.clone());
            let last_assistant_message =
                bound_session_preview(canonical.last_assistant_message.clone());
            let current = tx
                .query_row(
                    "SELECT data_json, title, project, started_at, updated_at, message_count,
                            first_user_message, last_user_message, last_assistant_message
                     FROM scoped_sessions
                     WHERE scope_key = ?1 AND id = ?2 AND agent = ?3 AND path = ?4",
                    params![scope_key.as_str(), canonical.id, agent, path],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<i64>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, Option<String>>(8)?,
                        ))
                    },
                )
                .optional()?;
            if current.as_ref().is_some_and(|current| {
                current.0 == data_json
                    && current.1 == title
                    && current.2 == project
                    && current.3 == started_at
                    && current.4 == updated_at
                    && current.5 == message_count
                    && current.6 == first_user_message
                    && current.7 == last_user_message
                    && current.8 == last_assistant_message
            }) {
                replace_scoped_session_scan_sources(tx, scope_key, &canonical)?;
                continue;
            }
            tx.execute(
                "DELETE FROM scoped_sessions
                 WHERE scope_key = ?1 AND id = ?2 AND agent = ?3 AND path <> ?4",
                params![scope_key.as_str(), canonical.id, agent, path],
            )?;
            tx.execute(
                "INSERT INTO scoped_sessions
                (scope_key, id, agent, title, project, path, started_at, updated_at, message_count, first_user_message, last_user_message, last_assistant_message, data_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(scope_key, id, agent, path) DO UPDATE SET
                title = excluded.title,
                project = excluded.project,
                started_at = excluded.started_at,
                updated_at = excluded.updated_at,
                message_count = excluded.message_count,
                first_user_message = excluded.first_user_message,
                last_user_message = excluded.last_user_message,
                last_assistant_message = excluded.last_assistant_message,
                data_json = excluded.data_json",
                params![
                    scope_key.as_str(),
                    canonical.id,
                    agent,
                    title,
                    project,
                    path,
                    started_at,
                    updated_at,
                    message_count,
                    first_user_message,
                    last_user_message,
                    last_assistant_message,
                    data_json,
                ],
            )?;
            replace_scoped_session_scan_sources(tx, scope_key, &canonical)?;
            changed.push(canonical);
        }
        Ok(changed)
    }

    fn list_sessions_in_tx(
        tx: &Transaction<'_>,
        scope_key: &ScopeKey,
    ) -> Result<Vec<SessionRecord>> {
        let mut stmt = tx.prepare(
            "SELECT data_json FROM scoped_sessions WHERE scope_key = ?1",
        )?;
        let mut sessions = Vec::new();
        let mut rows = stmt.query([scope_key.as_str()])?;
        while let Some(row) = rows.next()? {
            if let Ok(session) = serde_json::from_str::<SessionRecord>(&row.get::<_, String>(0)?) {
                sessions.push(Self::normalize_cached_session(session));
            }
        }
        Ok(sessions)
    }


    fn ensure_scoped_fs_manifest(&self) -> Result<()> {
        let mut stmt = self.conn.prepare("PRAGMA table_info(fs_manifest)")?;
        let columns = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let scope_is_primary = columns
            .iter()
            .any(|(name, primary)| name == "scope_key" && *primary > 0);
        if scope_is_primary {
            return Ok(());
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(
            "
            DROP INDEX IF EXISTS idx_fs_manifest_root_kind_path;
            ALTER TABLE fs_manifest RENAME TO fs_manifest_legacy;
            CREATE TABLE fs_manifest (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                source_kind TEXT NOT NULL,
                path TEXT NOT NULL,
                root TEXT NOT NULL,
                agent TEXT,
                scope TEXT,
                mtime_ns INTEGER,
                size INTEGER,
                inode INTEGER,
                device INTEGER,
                sha256 TEXT,
                parser_version TEXT NOT NULL,
                last_seen_at INTEGER NOT NULL,
                parse_status TEXT NOT NULL,
                PRIMARY KEY (scope_key, source_kind, path)
            );
            INSERT INTO fs_manifest (
                scope_key, source_kind, path, root, agent, scope, mtime_ns, size,
                inode, device, sha256, parser_version, last_seen_at, parse_status
            )
            SELECT scope_key, source_kind, path, root, agent, scope, mtime_ns, size,
                   inode, device, sha256, parser_version, last_seen_at, parse_status
            FROM fs_manifest_legacy;
            DROP TABLE fs_manifest_legacy;
            CREATE INDEX idx_fs_manifest_root_kind_path
                ON fs_manifest(scope_key, root, source_kind, path);
            ",
        )?;
        tx.commit()?;
        Ok(())
    }


    pub fn remove_sessions_for_paths_for_scope(
        &self,
        scope_key: &ScopeKey,
        paths: &[PathBuf],
    ) -> Result<Vec<crate::sessions::SessionIdentity>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let existing = self.list_sessions_for_scope(scope_key)?.sessions;
        let removed = existing
            .into_iter()
            .filter(|session| {
                paths
                    .iter()
                    .any(|path| session.path == *path || session.path.starts_with(path))
            })
            .collect::<Vec<_>>();
        if removed.is_empty() {
            return Ok(Vec::new());
        }

        let tx = self.conn.unchecked_transaction()?;
        for session in &removed {
            tx.execute(
                &format!(
                    "DELETE FROM {SCOPED_SESSION_TABLE}
                     WHERE scope_key = ?1 AND id = ?2 AND agent = ?3 AND path = ?4"
                ),
                params![
                    scope_key.as_str(),
                    session.id,
                    agent_label(session.agent),
                    session.path.display().to_string(),
                ],
            )?;
        }
        cleanup_stale_scoped_session_skill_rows(&tx, scope_key)?;
        cleanup_stale_scoped_session_scan_source_rows(&tx, scope_key)?;
        cleanup_stale_scoped_session_search_rows(&tx, scope_key)?;
        advance_projection_head_in_tx(&tx, scope_key, "sessions", None, "ready")?;
        tx.commit()?;
        self.refresh_scoped_session_snapshot(scope_key)?;
        Ok(removed
            .iter()
            .map(crate::sessions::SessionIdentity::from)
            .collect())
    }


    /// Search the workspace-owned session projection through its own scoped
    /// FTS index. The legacy global FTS tables are never consulted here.
    pub fn search_sessions_for_scope(
        &self,
        scope_key: &ScopeKey,
        query: &str,
        candidates: Option<&[SessionIdentity]>,
    ) -> Result<Vec<SessionSearchHit>> {
        with_database_read_lock_retry(|| {
            self.search_sessions_for_scope_once(scope_key, query, candidates)
        })
    }

    fn search_sessions_for_scope_once(
        &self,
        scope_key: &ScopeKey,
        query: &str,
        candidates: Option<&[SessionIdentity]>,
    ) -> Result<Vec<SessionSearchHit>> {
        let terms = session_search_terms(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        if terms.iter().any(|term| term.chars().count() < 3) {
            return self.search_scoped_sessions_by_contains(scope_key, &terms, candidates);
        }
        self.search_scoped_sessions_by_fts(scope_key, &terms, candidates)
    }

    fn search_scoped_sessions_by_fts(
        &self,
        scope_key: &ScopeKey,
        terms: &[String],
        candidates: Option<&[SessionIdentity]>,
    ) -> Result<Vec<SessionSearchHit>> {
        let query = session_search_query(terms);
        self.session_search_matches(&query)?;
        let sql_params = vec![SqlValue::Text(scope_key.as_str().to_string())];
        let candidate_join = self.session_search_candidate_join(candidates)?;
        let mut stmt = self.conn.prepare(&format!(
            "WITH matched AS (
                SELECT
                    search.scope_key,
                    search.session_id,
                    search.agent,
                    search.session_path,
                    MIN(search.id) AS record_id
                FROM temp.{SESSION_SEARCH_MATCH_TABLE} AS matches
                JOIN scoped_session_search_records AS search ON search.id = matches.record_id
                {candidate_join}
                WHERE search.scope_key = ?1
                GROUP BY search.scope_key, search.session_id, search.agent, search.session_path
            )
            SELECT
                sessions.data_json,
                search.metadata_text,
                search.title,
                search.project,
                search.user_text,
                search.assistant_text
             FROM matched
             JOIN scoped_session_search_records AS search
               ON search.id = matched.record_id
             JOIN scoped_sessions AS sessions
               ON sessions.scope_key = matched.scope_key
              AND sessions.id = matched.session_id
              AND sessions.agent = matched.agent
              AND sessions.path = matched.session_path"
        ))?;
        let rows = stmt.query_map(params_from_iter(sql_params.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                SessionSearchDocument {
                    metadata_text: row.get(1)?,
                    title: row.get(2)?,
                    project: row.get(3)?,
                    user_text: row.get(4)?,
                    assistant_text: row.get(5)?,
                },
            ))
        })?;
        let candidate_order = candidates.map(Self::session_search_candidate_order);
        let mut hits: Vec<SessionSearchHit> = Vec::new();
        let mut seen = HashSet::new();
        for row in rows {
            let (data_json, document) = row?;
            let session = Self::normalize_cached_session(
                serde_json::from_str::<SessionRecord>(&data_json)
                    .context("invalid cached scoped session search row")?,
            );
            if !seen.insert(Self::session_search_hit_key(&session)) {
                continue;
            }
            hits.push(SessionSearchHit {
                session,
                search_score: contains_search_score(&document, terms),
                search_snippet: contains_search_snippet(&document, terms),
            });
        }
        if let Some(candidate_order) = candidate_order {
            hits.sort_by_key(|hit| {
                candidate_order
                    .get(&Self::session_search_hit_key(&hit.session))
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        } else {
            hits.sort_by(Self::compare_session_search_hits);
        }
        Ok(hits)
    }

    fn search_scoped_sessions_by_contains(
        &self,
        scope_key: &ScopeKey,
        terms: &[String],
        candidates: Option<&[SessionIdentity]>,
    ) -> Result<Vec<SessionSearchHit>> {
        let where_clause = terms
            .iter()
            .map(|_| {
                "LOWER(
                    COALESCE(search.title, '') || ' ' ||
                    COALESCE(search.project, '') || ' ' ||
                    COALESCE(search.metadata_text, '') || ' ' ||
                    COALESCE(search.user_text, '') || ' ' ||
                    COALESCE(search.assistant_text, '')
                ) LIKE ? ESCAPE '\\'"
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut sql_params = vec![SqlValue::Text(scope_key.as_str().to_string())];
        sql_params.extend(
            terms
                .iter()
                .map(|term| SqlValue::Text(format!("%{}%", escape_like(term)))),
        );
        let candidate_join = self.session_search_candidate_join(candidates)?;
        let sql = format!(
            "WITH matched AS (
                SELECT
                    search.scope_key,
                    search.session_id,
                    search.agent,
                    search.session_path,
                    MIN(search.id) AS record_id
                FROM scoped_session_search_records AS search
                {candidate_join}
                WHERE search.scope_key = ?1 AND {where_clause}
                GROUP BY search.scope_key, search.session_id, search.agent, search.session_path
            )
            SELECT
                sessions.data_json,
                search.metadata_text,
                search.title,
                search.project,
                search.user_text,
                search.assistant_text
             FROM matched
             JOIN scoped_session_search_records AS search
               ON search.id = matched.record_id
             JOIN scoped_sessions AS sessions
               ON sessions.scope_key = search.scope_key
              AND sessions.id = search.session_id
              AND sessions.agent = search.agent
              AND sessions.path = search.session_path
             "
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(sql_params.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                SessionSearchDocument {
                    metadata_text: row.get(1)?,
                    title: row.get(2)?,
                    project: row.get(3)?,
                    user_text: row.get(4)?,
                    assistant_text: row.get(5)?,
                },
            ))
        })?;
        let candidate_order = candidates.map(Self::session_search_candidate_order);
        let mut hits: Vec<SessionSearchHit> = Vec::new();
        let mut hit_indexes: HashMap<(String, String, PathBuf), usize> = HashMap::new();
        for row in rows {
            let (data_json, document) = row?;
            let session = Self::normalize_cached_session(
                serde_json::from_str::<SessionRecord>(&data_json)
                    .context("invalid cached scoped session search row")?,
            );
            let key = Self::session_search_hit_key(&session);
            let hit = SessionSearchHit {
                search_score: contains_search_score(&document, terms),
                search_snippet: contains_search_snippet(&document, terms),
                session,
            };
            if let Some(index) = hit_indexes.get(&key).copied() {
                if hit.search_score > hits[index].search_score {
                    hits[index] = hit;
                }
            } else {
                hit_indexes.insert(key, hits.len());
                hits.push(hit);
            }
        }
        if let Some(candidate_order) = candidate_order {
            hits.sort_by_key(|hit| {
                candidate_order
                    .get(&Self::session_search_hit_key(&hit.session))
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        } else {
            hits.sort_by(Self::compare_session_search_hits);
        }
        Ok(hits)
    }



    fn compare_session_search_hits(
        left: &SessionSearchHit,
        right: &SessionSearchHit,
    ) -> std::cmp::Ordering {
        right
            .search_score
            .total_cmp(&left.search_score)
            .then_with(|| Self::compare_session_updated_at(&left.session, &right.session))
    }


    fn session_search_candidate_join(
        &self,
        candidates: Option<&[SessionIdentity]>,
    ) -> Result<String> {
        // Keep the large candidate set indexed on this connection. A JSON
        // CTE would avoid SQLite's expression-depth limit but scan every
        // candidate for every FTS hit.
        let Some(candidates) = candidates else {
            return Ok(String::new());
        };
        self.conn.execute_batch(&format!(
            "CREATE TEMP TABLE IF NOT EXISTS {SESSION_SEARCH_CANDIDATE_TABLE} (
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                PRIMARY KEY (session_id, agent, session_path)
            )"
        ))?;
        self.conn.execute(
            &format!("DELETE FROM temp.{SESSION_SEARCH_CANDIDATE_TABLE}"),
            [],
        )?;
        let candidates_json = serde_json::to_string(candidates)
            .context("serialize session search candidates")?;
        self.conn.execute(
            &format!(
                "INSERT OR IGNORE INTO temp.{SESSION_SEARCH_CANDIDATE_TABLE}
                 SELECT
                    json_extract(value, '$.id'),
                    json_extract(value, '$.agent'),
                    json_extract(value, '$.path')
                 FROM json_each(?1)"
            ),
            [candidates_json],
        )?;
        Ok(format!(
            "JOIN temp.{SESSION_SEARCH_CANDIDATE_TABLE} AS candidate
               ON candidate.session_id = search.session_id
              AND candidate.agent = search.agent
              AND candidate.session_path = search.session_path"
        ))
    }

    fn session_search_matches(&self, query: &str) -> Result<()> {
        self.conn.execute_batch(&format!(
            "CREATE TEMP TABLE IF NOT EXISTS {SESSION_SEARCH_MATCH_TABLE} (
                record_id INTEGER PRIMARY KEY
            )"
        ))?;
        self.conn.execute(
            &format!("DELETE FROM temp.{SESSION_SEARCH_MATCH_TABLE}"),
            [],
        )?;
        self.conn.execute(
            &format!(
                "INSERT INTO temp.{SESSION_SEARCH_MATCH_TABLE}(record_id)
                 SELECT rowid
                 FROM scoped_session_search_fts
                 WHERE scoped_session_search_fts MATCH ?1"
            ),
            [query],
        )?;
        Ok(())
    }

    fn session_search_candidate_order(
        candidates: &[SessionIdentity],
    ) -> HashMap<(String, String, PathBuf), usize> {
        candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                (
                    (
                        candidate.id.clone(),
                        agent_label(candidate.agent).to_owned(),
                        candidate.path.clone(),
                    ),
                    index,
                )
            })
            .collect()
    }

    fn session_search_hit_key(session: &SessionRecord) -> (String, String, PathBuf) {
        (
            session.id.clone(),
            agent_label(session.agent).to_owned(),
            session.path.clone(),
        )
    }


    pub fn save_sessions_at_for_scope(
        &self,
        scope_key: &ScopeKey,
        sessions: &SessionScan,
        scanned_at: u64,
    ) -> Result<()> {
        self.save_sessions_at_with_scope(sessions, scanned_at, scope_key)
    }


    /// Rebuild the derived scoped search index from the canonical session
    /// projection. The index is disposable; search itself never falls back to
    /// another workspace or treats stale index rows as session data.
    pub fn ensure_scoped_session_search_for_scope(&self, scope_key: &ScopeKey) -> Result<bool> {
        let mut statement = self.conn.prepare(
            "SELECT sessions.data_json
             FROM scoped_sessions AS sessions
             WHERE sessions.scope_key = ?1
               AND (
                    NOT EXISTS (
                        SELECT 1
                        FROM scoped_session_search_index AS search_index
                        WHERE search_index.scope_key = sessions.scope_key
                          AND search_index.session_id = sessions.id
                          AND search_index.agent = sessions.agent
                          AND search_index.session_path = sessions.path
                          AND search_index.search_index_version = ?2
                    )
                    OR NOT EXISTS (
                        SELECT 1
                        FROM scoped_session_search_records AS records
                        WHERE records.scope_key = sessions.scope_key
                          AND records.session_id = sessions.id
                          AND records.agent = sessions.agent
                          AND records.session_path = sessions.path
                          AND records.record_order = 0
                    )
               )",
        )?;
        let missing_sessions = statement
            .query_map(
                params![scope_key.as_str(), SESSION_SEARCH_INDEX_VERSION],
                |row| row.get::<_, String>(0),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        let stale_rows: i64 = self.conn.query_row(
            "SELECT
                (SELECT COUNT(*)
                 FROM scoped_session_search_records AS records
                 WHERE records.scope_key = ?1
                   AND NOT EXISTS (
                        SELECT 1
                        FROM scoped_sessions AS sessions
                        WHERE sessions.scope_key = records.scope_key
                          AND sessions.id = records.session_id
                          AND sessions.agent = records.agent
                          AND sessions.path = records.session_path
                   ))
                +
                (SELECT COUNT(*)
                 FROM scoped_session_search_index AS search_index
                 WHERE search_index.scope_key = ?1
                   AND NOT EXISTS (
                        SELECT 1
                        FROM scoped_sessions AS sessions
                        WHERE sessions.scope_key = search_index.scope_key
                          AND sessions.id = search_index.session_id
                          AND sessions.agent = search_index.agent
                          AND sessions.path = search_index.session_path
                   ))",
            params![scope_key.as_str()],
            |row| row.get(0),
        )?;
        if missing_sessions.is_empty() && stale_rows == 0 {
            return Ok(false);
        }
        let tx = self.conn.unchecked_transaction()?;
        for data_json in missing_sessions {
            let Ok(session) = serde_json::from_str::<SessionRecord>(&data_json) else {
                continue;
            };
            index_scoped_session_search_document(
                &tx,
                scope_key,
                &Self::normalize_cached_session(session),
            )?;
        }
        cleanup_stale_scoped_session_search_rows(&tx, scope_key)?;
        tx.commit()?;
        Ok(true)
    }

    pub fn rebuild_scoped_session_search_for_scope(&self, scope_key: &ScopeKey) -> Result<usize> {
        let sessions = self.list_sessions_from_table(scope_key)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM scoped_session_search_records WHERE scope_key = ?1",
            params![scope_key.as_str()],
        )?;
        tx.execute(
            "DELETE FROM scoped_session_search_index WHERE scope_key = ?1",
            params![scope_key.as_str()],
        )?;
        for session in &sessions.sessions {
            index_scoped_session_search_document(&tx, scope_key, session)?;
        }
        tx.commit()?;
        Ok(sessions.sessions.len())
    }

    fn save_sessions_at_with_scope(
        &self,
        sessions: &SessionScan,
        scanned_at: u64,
        scope_key: &ScopeKey,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        self.save_sessions_at_with_scope_in_tx(&tx, sessions, scanned_at, scope_key)?;
        tx.commit()?;
        self.refresh_scoped_session_snapshot(scope_key)?;
        self.refresh_scoped_session_project_snapshot(scope_key)?;
        Ok(())
    }

    fn save_sessions_at_with_scope_in_tx(
        &self,
        tx: &Transaction<'_>,
        sessions: &SessionScan,
        scanned_at: u64,
        scope_key: &ScopeKey,
    ) -> Result<()> {
        tx.execute_batch(
            "
            CREATE TEMP TABLE IF NOT EXISTS current_sessions (
                id TEXT NOT NULL,
                agent TEXT NOT NULL,
                path TEXT NOT NULL,
                PRIMARY KEY (id, agent, path)
            );
            DELETE FROM current_sessions;
            ",
        )?;

        for session in &sessions.sessions {
            let agent = agent_label(session.agent);
            let path = session.path.display().to_string();
            let title = clean_session_title(session.title.clone());
            let data_json = Self::session_metadata_json(session)?;
            tx.execute(
                "INSERT INTO current_sessions (id, agent, path)
                 VALUES (?1, ?2, ?3)",
                params![session.id, agent, path],
            )?;
            tx.execute(
                &format!(
                    "INSERT INTO {SCOPED_SESSION_TABLE}
                    (scope_key, id, agent, title, project, path, started_at, updated_at, message_count, first_user_message, last_user_message, last_assistant_message, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(scope_key, id, agent, path) DO UPDATE SET
                    title = excluded.title,
                    project = excluded.project,
                    started_at = excluded.started_at,
                    updated_at = excluded.updated_at,
                    message_count = excluded.message_count,
                    first_user_message = excluded.first_user_message,
                    last_user_message = excluded.last_user_message,
                    last_assistant_message = excluded.last_assistant_message,
                    data_json = excluded.data_json
                 WHERE {SCOPED_SESSION_TABLE}.data_json IS NOT excluded.data_json
                    OR {SCOPED_SESSION_TABLE}.title IS NOT excluded.title
                    OR {SCOPED_SESSION_TABLE}.project IS NOT excluded.project
                    OR {SCOPED_SESSION_TABLE}.started_at IS NOT excluded.started_at
                    OR {SCOPED_SESSION_TABLE}.updated_at IS NOT excluded.updated_at
                    OR {SCOPED_SESSION_TABLE}.message_count IS NOT excluded.message_count
                    OR {SCOPED_SESSION_TABLE}.first_user_message IS NOT excluded.first_user_message
                    OR {SCOPED_SESSION_TABLE}.last_user_message IS NOT excluded.last_user_message
                    OR {SCOPED_SESSION_TABLE}.last_assistant_message IS NOT excluded.last_assistant_message"
                ),
                params![
                    scope_key.as_str(),
                    session.id,
                    agent,
                    title,
                    session.project.as_ref().map(|path| path.display().to_string()),
                    session.path.display().to_string(),
                    session.started_at,
                    session.updated_at,
                    session.message_count.map(|value| value as i64),
                    bound_session_preview(session.first_user_message.clone()),
                    bound_session_preview(session.last_user_message.clone()),
                    bound_session_preview(session.last_assistant_message.clone()),
                    data_json,
                ],
            )?;
            replace_scoped_session_scan_sources(&tx, scope_key, session)?;
            index_scoped_session_search_document_best_effort(&tx, scope_key, session);
        }

        if sessions.warnings.is_empty() {
            tx.execute(
                &format!(
                    "DELETE FROM {SCOPED_SESSION_TABLE}
                     WHERE scope_key = ?1
                       AND NOT EXISTS (
                        SELECT 1 FROM current_sessions
                        WHERE current_sessions.id = {SCOPED_SESSION_TABLE}.id
                          AND current_sessions.agent = {SCOPED_SESSION_TABLE}.agent
                          AND current_sessions.path = {SCOPED_SESSION_TABLE}.path
                       )"
                ),
                params![scope_key.as_str()],
            )?;
            cleanup_stale_scoped_session_skill_rows(&tx, scope_key)?;
            cleanup_stale_scoped_session_scan_source_rows(&tx, scope_key)?;
            cleanup_stale_scoped_session_search_rows(&tx, scope_key)?;
        }
        tx.execute("DELETE FROM current_sessions", [])?;
        let key = format!("sessions_last_scan_at:{}", scope_key.as_str());
        tx.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, scanned_at.to_string()],
        )?;
        advance_projection_head_in_tx(&tx, scope_key, "sessions", None, "ready")?;
        Ok(())
    }


    pub fn clear_session_skill_index_for_scope(&self, scope_key: &ScopeKey) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM scoped_session_skill_links WHERE scope_key = ?1",
            [scope_key.as_str()],
        )?;
        tx.execute(
            "DELETE FROM scoped_session_skill_index WHERE scope_key = ?1",
            [scope_key.as_str()],
        )?;
        tx.commit()?;
        Ok(())
    }


    pub fn ensure_session_skill_index_version_for_scope(
        &self,
        scope_key: &ScopeKey,
        version: &str,
    ) -> Result<bool> {
        let meta_key = format!("session_skill_index_version:{}", scope_key.as_str());
        let current = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                [&meta_key],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if current.as_deref() == Some(version) {
            return Ok(false);
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM scoped_session_skill_links WHERE scope_key = ?1",
            [scope_key.as_str()],
        )?;
        tx.execute(
            "DELETE FROM scoped_session_skill_index WHERE scope_key = ?1",
            [scope_key.as_str()],
        )?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![meta_key, version],
        )?;
        tx.commit()?;
        Ok(true)
    }


    pub fn session_skill_index_is_current_for_scope(
        &self,
        scope_key: &ScopeKey,
        session: &SessionRecord,
        file_mtime: i64,
        file_size: i64,
    ) -> Result<bool> {
        let status = self
            .conn
            .query_row(
                "SELECT status FROM scoped_session_skill_index
                 WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3
                   AND session_path = ?4 AND file_mtime = ?5 AND file_size = ?6",
                params![
                    scope_key.as_str(),
                    session.id,
                    agent_label(session.agent),
                    session.path.display().to_string(),
                    file_mtime,
                    file_size,
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(status.as_deref() == Some("indexed"))
    }


    pub fn replace_session_skill_links_for_scope(
        &self,
        scope_key: &ScopeKey,
        session: &SessionRecord,
        state: &SessionFileState,
        links: &[SessionSkillLink],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let agent = agent_label(session.agent);
        let session_path = session.path.display().to_string();
        tx.execute(
            "DELETE FROM scoped_session_skill_links
             WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
            params![scope_key.as_str(), session.id, agent, session_path],
        )?;
        for link in links {
            tx.execute(
                "INSERT INTO scoped_session_skill_links (
                    scope_key, session_id, agent, session_path, skill_name, skill_path,
                    skill_agent, skill_scope, evidence_kind, evidence_text,
                    evidence_time, confidence
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    scope_key.as_str(),
                    link.session_id,
                    agent_label(link.agent),
                    link.session_path.display().to_string(),
                    link.skill_name,
                    link.skill_path.display().to_string(),
                    link.skill_agent.map(agent_label),
                    link.skill_scope,
                    link.evidence_kind,
                    link.evidence_text,
                    link.evidence_time,
                    link.confidence,
                ],
            )?;
        }
        tx.execute(
            "INSERT INTO scoped_session_skill_index (
                scope_key, session_id, agent, session_path, file_mtime, file_size,
                indexed_at, status, error
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'indexed', NULL)
             ON CONFLICT(scope_key, session_id, agent, session_path) DO UPDATE SET
                file_mtime = excluded.file_mtime,
                file_size = excluded.file_size,
                indexed_at = excluded.indexed_at,
                status = excluded.status,
                error = NULL",
            params![
                scope_key.as_str(),
                session.id,
                agent,
                session_path,
                state.file_mtime,
                state.file_size,
                unix_now().to_string(),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }


    pub fn mark_session_skill_index_failed_for_scope(
        &self,
        scope_key: &ScopeKey,
        session: &SessionRecord,
        file_mtime: i64,
        file_size: i64,
        error: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO scoped_session_skill_index (
                scope_key, session_id, agent, session_path, file_mtime, file_size,
                indexed_at, status, error
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'failed', ?8)
             ON CONFLICT(scope_key, session_id, agent, session_path) DO UPDATE SET
                file_mtime = excluded.file_mtime,
                file_size = excluded.file_size,
                indexed_at = excluded.indexed_at,
                status = excluded.status,
                error = excluded.error",
            params![
                scope_key.as_str(),
                session.id,
                agent_label(session.agent),
                session.path.display().to_string(),
                file_mtime,
                file_size,
                unix_now().to_string(),
                error,
            ],
        )?;
        Ok(())
    }


    pub fn session_skill_index_status_for_scope(
        &self,
        scope_key: &ScopeKey,
        running: bool,
    ) -> Result<SessionSkillIndexStatus> {
        with_database_read_lock_retry(|| {
            let total = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM scoped_sessions WHERE scope_key = ?1",
                    [scope_key.as_str()],
                    |row| row.get::<_, i64>(0),
                )?
                .max(0) as usize;
            let indexed = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM scoped_session_skill_index
                     WHERE scope_key = ?1 AND status = 'indexed'",
                    [scope_key.as_str()],
                    |row| row.get::<_, i64>(0),
                )?
                .max(0) as usize;
            let failed = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM scoped_session_skill_index
                     WHERE scope_key = ?1 AND status = 'failed'",
                    [scope_key.as_str()],
                    |row| row.get::<_, i64>(0),
                )?
                .max(0) as usize;
            let last_indexed_at = self.conn.query_row(
                "SELECT MAX(indexed_at) FROM scoped_session_skill_index
                 WHERE scope_key = ?1 AND status = 'indexed'",
                [scope_key.as_str()],
                |row| row.get::<_, Option<String>>(0),
            )?;
            Ok(SessionSkillIndexStatus {
                total,
                indexed,
                pending: total.saturating_sub(indexed + failed),
                failed,
                running,
                last_indexed_at,
            })
        })
    }


    pub fn session_skill_links_for_scope(
        &self,
        scope_key: &ScopeKey,
        session_id: &str,
        agent: AgentKind,
    ) -> Result<Vec<SessionSkillLink>> {
        with_database_read_lock_retry(|| self.query_session_skill_links_from(
            "scoped_session_skill_links",
            "scoped_sessions",
            "WHERE links.scope_key = ?1 AND links.session_id = ?2 AND links.agent = ?3",
            params![scope_key.as_str(), session_id, agent_label(agent)],
        ))
    }

    pub fn skill_session_links_for_scope(
        &self,
        scope_key: &ScopeKey,
        skill_name: &str,
    ) -> Result<Vec<SessionSkillLink>> {
        with_database_read_lock_retry(|| self.query_session_skill_links_from(
            "scoped_session_skill_links",
            "scoped_sessions",
            "WHERE links.scope_key = ?1 AND links.skill_name = ?2",
            params![scope_key.as_str(), skill_name],
        ))
    }


    fn query_session_skill_links_from<P>(
        &self,
        links_table: &str,
        sessions_table: &str,
        where_clause: &str,
        params: P,
    ) -> Result<Vec<SessionSkillLink>>
    where
        P: rusqlite::Params,
    {
        // Read only the Session fields Linked Sessions needs from data_json.
        // Scalar session columns are projections, not authority (see list_sessions).
        // ORDER BY mirrors the prior in-memory sort keys; compare_timestamps
        // still re-sorts so RFC3339 offsets and invalid stamps stay equivalent.
        let sql = format!(
            "SELECT
                links.session_id,
                links.agent,
                links.session_path,
                CASE
                  WHEN sessions.data_json IS NULL THEN 0
                  WHEN json_valid(sessions.data_json) THEN 0
                  ELSE 1
                END,
                json_extract(sessions.data_json, '$.title'),
                json_extract(sessions.data_json, '$.project'),
                json_extract(sessions.data_json, '$.started_at'),
                json_extract(sessions.data_json, '$.updated_at'),
                json_extract(sessions.data_json, '$.message_count'),
                links.skill_name,
                links.skill_path,
                links.skill_agent,
                links.skill_scope,
                links.evidence_kind,
                links.evidence_text,
                links.evidence_time,
                links.confidence
             FROM {links_table} links
             LEFT JOIN {sessions_table} sessions
               ON sessions.id = links.session_id
              AND sessions.agent = links.agent
              AND sessions.path = links.session_path
              AND sessions.scope_key = links.scope_key
             {where_clause}
             ORDER BY
                json_extract(sessions.data_json, '$.updated_at') DESC,
                links.evidence_time DESC,
                links.skill_name ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, String>(13)?,
                row.get::<_, String>(14)?,
                row.get::<_, Option<String>>(15)?,
                row.get::<_, String>(16)?,
            ))
        })?;
        let mut links = Vec::new();
        for row in rows {
            let (
                session_id,
                agent,
                session_path,
                invalid_session_json,
                session_title,
                session_project,
                session_started_at,
                session_updated_at,
                session_message_count,
                skill_name,
                skill_path,
                skill_agent,
                skill_scope,
                evidence_kind,
                evidence_text,
                evidence_time,
                confidence,
            ) = row?;
            let agent = parse_agent_label(&agent)
                .ok_or_else(|| anyhow::anyhow!("invalid session skill link agent: {agent}"))?;
            if invalid_session_json != 0 {
                anyhow::bail!(
                    "invalid cached session row for skill link {}:{}",
                    agent_label(agent),
                    session_id
                );
            }
            let skill_agent = skill_agent
                .map(|agent| {
                    parse_agent_label(&agent).ok_or_else(|| {
                        anyhow::anyhow!("invalid session skill link skill agent: {agent}")
                    })
                })
                .transpose()?;
            let message_count = session_message_count
                .map(|count| usize::try_from(count))
                .transpose()
                .with_context(|| {
                    format!(
                        "invalid session message_count for skill link {}:{}",
                        agent_label(agent),
                        session_id
                    )
                })?;
            let link = SessionSkillLink {
                session_id,
                agent,
                session_path: PathBuf::from(session_path),
                session_title: clean_session_title(session_title),
                session_project: session_project.map(PathBuf::from),
                session_started_at,
                session_updated_at,
                session_message_count: message_count,
                skill_name,
                skill_path: PathBuf::from(skill_path),
                skill_agent,
                skill_scope,
                evidence_kind,
                evidence_text,
                evidence_time,
                confidence,
            };
            links.push(link);
        }
        links.sort_by(|left, right| {
            compare_timestamps(
                right.session_updated_at.as_deref(),
                left.session_updated_at.as_deref(),
            )
            .then_with(|| {
                compare_timestamps(
                    right.evidence_time.as_deref(),
                    left.evidence_time.as_deref(),
                )
            })
            .then_with(|| left.skill_name.cmp(&right.skill_name))
        });
        Ok(links)
    }

    pub fn skill_source_records(&self) -> Result<Vec<SkillSourceRecord>> {
        let mut records = Vec::new();
        for table in ["skill_sources", "scoped_skill_sources"] {
            let mut statement = self
                .conn
                .prepare(&format!("SELECT data_json FROM {table}"))?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                let data_json = row?;
                records.push(serde_json::from_str::<SkillSourceRecord>(&data_json).map_err(
                    |error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    },
                )?);
            }
        }
        records.sort_by(|left, right| {
            left.skill_name
                .cmp(&right.skill_name)
                .then_with(|| left.skill_path.cmp(&right.skill_path))
        });
        records.dedup_by(|left, right| left.skill_path == right.skill_path);
        Ok(records)
    }

    pub fn skill_source_records_for_workspace(
        &self,
        workspace_root: &Path,
    ) -> Result<Vec<SkillSourceRecord>> {
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let mut statement = self.conn.prepare(
            "SELECT data_json FROM scoped_skill_sources
             WHERE scope_key = ?1
             ORDER BY skill_name, skill_path",
        )?;
        let rows = statement.query_map(params![scope_key.as_str()], |row| row.get::<_, String>(0))?;
        rows.map(|row| {
            let data_json = row?;
            serde_json::from_str::<SkillSourceRecord>(&data_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
    }

    pub fn skill_source_record(&self, skill_path: &Path) -> Result<Option<SkillSourceRecord>> {
        let data_json = self
            .conn
            .query_row(
                "SELECT data_json FROM skill_sources WHERE skill_path = ?1",
                params![skill_path.display().to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let data_json = match data_json {
            Some(data_json) => Some(data_json),
            None => self
                .conn
                .query_row(
                    "SELECT data_json FROM scoped_skill_sources WHERE skill_path = ?1
                     ORDER BY scope_key LIMIT 1",
                    params![skill_path.display().to_string()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?,
        };
        data_json
            .map(|data_json| serde_json::from_str(&data_json).map_err(Into::into))
            .transpose()
    }

    pub fn skill_source_record_for_workspace(
        &self,
        workspace_root: &Path,
        skill_path: &Path,
    ) -> Result<Option<SkillSourceRecord>> {
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let data_json = self
            .conn
            .query_row(
                "SELECT data_json FROM scoped_skill_sources
                 WHERE scope_key = ?1 AND skill_path = ?2",
                params![scope_key.as_str(), skill_path.display().to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        data_json
            .map(|data_json| serde_json::from_str(&data_json).map_err(Into::into))
            .transpose()
    }

    pub fn insert_skill_source_records_if_missing(
        &self,
        records: &[SkillSourceRecord],
    ) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut inserted = 0;
        for record in records {
            inserted += tx.execute(
                "INSERT OR IGNORE INTO skill_sources (
                    skill_path, skill_name, source_kind, source, source_ref, source_version,
                    source_relative_path, update_status, origin, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    record.skill_path.display().to_string(),
                    record.skill_name,
                    record.source_kind,
                    record.source,
                    record.source_ref,
                    record.source_version,
                    record.source_relative_path,
                    record.update_status,
                    record.origin,
                    serde_json::to_string(record)?,
                ],
            )?;
        }
        tx.commit()?;
        Ok(inserted)
    }

    pub fn insert_skill_source_records_if_missing_for_workspace(
        &self,
        workspace_root: &Path,
        records: &[SkillSourceRecord],
    ) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let tx = self.conn.unchecked_transaction()?;
        let inserted = self.insert_skill_source_records_if_missing_for_workspace_in_tx(
            &tx,
            &scope_key,
            records,
        )?;
        tx.commit()?;
        Ok(inserted)
    }

    fn insert_skill_source_records_if_missing_for_workspace_in_tx(
        &self,
        tx: &Transaction<'_>,
        scope_key: &ScopeKey,
        records: &[SkillSourceRecord],
    ) -> Result<usize> {
        let mut inserted = 0;
        for record in records {
            inserted += tx.execute(
                "INSERT OR IGNORE INTO scoped_skill_sources (
                    scope_key, skill_path, skill_name, source_kind, source, source_ref, source_version,
                    source_relative_path, update_status, origin, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    scope_key.as_str(),
                    record.skill_path.display().to_string(),
                    record.skill_name,
                    record.source_kind,
                    record.source,
                    record.source_ref,
                    record.source_version,
                    record.source_relative_path,
                    record.update_status,
                    record.origin,
                    serde_json::to_string(record)?,
                ],
            )?;
        }
        Ok(inserted)
    }

    pub fn upsert_skill_source_records(&self, records: &[SkillSourceRecord]) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut changed = 0;
        for record in records {
            changed += tx.execute(
                "INSERT INTO skill_sources (
                    skill_path, skill_name, source_kind, source, source_ref, source_version,
                    source_relative_path, update_status, origin, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(skill_path) DO UPDATE SET
                    skill_name = excluded.skill_name,
                    source_kind = excluded.source_kind,
                    source = excluded.source,
                    source_ref = excluded.source_ref,
                    source_version = excluded.source_version,
                    source_relative_path = excluded.source_relative_path,
                    update_status = excluded.update_status,
                    origin = excluded.origin,
                    data_json = excluded.data_json",
                params![
                    record.skill_path.display().to_string(),
                    record.skill_name,
                    record.source_kind,
                    record.source,
                    record.source_ref,
                    record.source_version,
                    record.source_relative_path,
                    record.update_status,
                    record.origin,
                    serde_json::to_string(record)?,
                ],
            )?;
        }
        tx.commit()?;
        Ok(changed)
    }

    pub fn upsert_skill_source_records_for_workspace(
        &self,
        workspace_root: &Path,
        records: &[SkillSourceRecord],
    ) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let tx = self.conn.unchecked_transaction()?;
        let mut changed = 0;
        for record in records {
            changed += tx.execute(
                "INSERT INTO scoped_skill_sources (
                    scope_key, skill_path, skill_name, source_kind, source, source_ref, source_version,
                    source_relative_path, update_status, origin, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(scope_key, skill_path) DO UPDATE SET
                    skill_name = excluded.skill_name,
                    source_kind = excluded.source_kind,
                    source = excluded.source,
                    source_ref = excluded.source_ref,
                    source_version = excluded.source_version,
                    source_relative_path = excluded.source_relative_path,
                    update_status = excluded.update_status,
                    origin = excluded.origin,
                    data_json = excluded.data_json",
                params![
                    scope_key.as_str(),
                    record.skill_path.display().to_string(),
                    record.skill_name,
                    record.source_kind,
                    record.source,
                    record.source_ref,
                    record.source_version,
                    record.source_relative_path,
                    record.update_status,
                    record.origin,
                    serde_json::to_string(record)?,
                ],
            )?;
        }
        tx.commit()?;
        Ok(changed)
    }

    pub fn delete_skill_source_records(&self, skill_paths: &[PathBuf]) -> Result<usize> {
        if skill_paths.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut deleted = 0;
        for skill_path in skill_paths {
            deleted += tx.execute(
                "DELETE FROM skill_sources WHERE skill_path = ?1",
                params![skill_path.display().to_string()],
            )?;
            tx.execute(
                "DELETE FROM skill_snapshots WHERE skill_path = ?1",
                params![skill_path.display().to_string()],
            )?;
        }
        tx.commit()?;
        Ok(deleted)
    }

    pub fn delete_skill_source_records_for_workspace(
        &self,
        workspace_root: &Path,
        skill_paths: &[PathBuf],
    ) -> Result<usize> {
        if skill_paths.is_empty() {
            return Ok(0);
        }
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let tx = self.conn.unchecked_transaction()?;
        let mut deleted = 0;
        for skill_path in skill_paths {
            deleted += tx.execute(
                "DELETE FROM scoped_skill_sources WHERE scope_key = ?1 AND skill_path = ?2",
                params![scope_key.as_str(), skill_path.display().to_string()],
            )?;
            tx.execute(
                "DELETE FROM scoped_skill_snapshots WHERE scope_key = ?1 AND skill_path = ?2",
                params![scope_key.as_str(), skill_path.display().to_string()],
            )?;
        }
        tx.commit()?;
        Ok(deleted)
    }

    pub fn delete_skill_source_records_for_names(&self, names: &[String]) -> Result<usize> {
        if names.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut deleted = 0;
        for name in names {
            tx.execute(
                "DELETE FROM skill_snapshots
                 WHERE skill_path IN (
                    SELECT skill_path FROM skill_sources WHERE skill_name = ?1
                 )",
                params![name],
            )?;
            deleted += tx.execute(
                "DELETE FROM skill_sources WHERE skill_name = ?1",
                params![name],
            )?;
        }
        tx.commit()?;
        Ok(deleted)
    }

    pub fn skill_snapshot(&self, skill_path: &Path) -> Result<Option<SkillSnapshot>> {
        let path = skill_path.display().to_string();
        let read_snapshot = |sql: &str| -> Result<Option<SkillSnapshot>> {
            let mut statement = self.conn.prepare(sql)?;
            let mut rows = statement.query(params![&path])?;
            let Some(first) = rows.next()? else {
                return Ok(None);
            };
            let source_version = first.get::<_, String>(0)?;
            let mut files = vec![SkillSnapshotFile {
                relative_path: first.get(1)?,
                content: first.get(2)?,
            }];
            while let Some(row) = rows.next()? {
                files.push(SkillSnapshotFile {
                    relative_path: row.get(1)?,
                    content: row.get(2)?,
                });
            }
            Ok(Some(SkillSnapshot {
                skill_path: skill_path.to_path_buf(),
                source_version,
                files,
            }))
        };

        if let Some(snapshot) = read_snapshot(
            "SELECT source_version, relative_path, content
             FROM skill_snapshots
             WHERE skill_path = ?1
             ORDER BY relative_path",
        )? {
            return Ok(Some(snapshot));
        }
        read_snapshot(
            "SELECT source_version, relative_path, content
             FROM scoped_skill_snapshots
             WHERE skill_path = ?1
               AND scope_key = (
                   SELECT scope_key FROM scoped_skill_snapshots
                   WHERE skill_path = ?1
                   ORDER BY scope_key LIMIT 1
               )
             ORDER BY relative_path",
        )
    }

    pub fn replace_skill_snapshots(&self, snapshots: &[SkillSnapshot]) -> Result<()> {
        if snapshots.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        for snapshot in snapshots {
            tx.execute(
                "DELETE FROM skill_snapshots WHERE skill_path = ?1",
                params![snapshot.skill_path.display().to_string()],
            )?;
            for file in &snapshot.files {
                tx.execute(
                    "INSERT INTO skill_snapshots (
                        skill_path, source_version, relative_path, content
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        snapshot.skill_path.display().to_string(),
                        snapshot.source_version,
                        file.relative_path,
                        file.content,
                    ],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn replace_skill_snapshots_for_workspace(
        &self,
        workspace_root: &Path,
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        if snapshots.is_empty() {
            return Ok(());
        }
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let tx = self.conn.unchecked_transaction()?;
        for snapshot in snapshots {
            tx.execute(
                "DELETE FROM scoped_skill_snapshots
                 WHERE scope_key = ?1 AND skill_path = ?2",
                params![scope_key.as_str(), snapshot.skill_path.display().to_string()],
            )?;
            for file in &snapshot.files {
                tx.execute(
                    "INSERT INTO scoped_skill_snapshots (
                        scope_key, skill_path, source_version, relative_path, content
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        scope_key.as_str(),
                        snapshot.skill_path.display().to_string(),
                        snapshot.source_version,
                        file.relative_path,
                        file.content,
                    ],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn persist_skill_update_persistence(
        &self,
        source_records: &[SkillSourceRecord],
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        if source_records.is_empty() && snapshots.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        for record in source_records {
            tx.execute(
                "INSERT INTO skill_sources (
                    skill_path, skill_name, source_kind, source, source_ref, source_version,
                    source_relative_path, update_status, origin, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(skill_path) DO UPDATE SET
                    skill_name = excluded.skill_name,
                    source_kind = excluded.source_kind,
                    source = excluded.source,
                    source_ref = excluded.source_ref,
                    source_version = excluded.source_version,
                    source_relative_path = excluded.source_relative_path,
                    update_status = excluded.update_status,
                    origin = excluded.origin,
                    data_json = excluded.data_json",
                params![
                    record.skill_path.display().to_string(),
                    record.skill_name,
                    record.source_kind,
                    record.source,
                    record.source_ref,
                    record.source_version,
                    record.source_relative_path,
                    record.update_status,
                    record.origin,
                    serde_json::to_string(record)?,
                ],
            )?;
        }
        for snapshot in snapshots {
            tx.execute(
                "DELETE FROM skill_snapshots WHERE skill_path = ?1",
                params![snapshot.skill_path.display().to_string()],
            )?;
            for file in &snapshot.files {
                tx.execute(
                    "INSERT INTO skill_snapshots (
                        skill_path, source_version, relative_path, content
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        snapshot.skill_path.display().to_string(),
                        snapshot.source_version,
                        file.relative_path,
                        file.content,
                    ],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn persist_skill_update_persistence_checked(
        &self,
        expected_source_versions: &[(PathBuf, Option<String>)],
        source_records: &[SkillSourceRecord],
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        self.validate_skill_source_versions(expected_source_versions)?;
        self.persist_skill_update_persistence(source_records, snapshots)
    }

    pub fn validate_skill_source_versions(
        &self,
        expected_source_versions: &[(PathBuf, Option<String>)],
    ) -> Result<()> {
        if expected_source_versions.is_empty() {
            return Ok(());
        }
        for (path, expected_version) in expected_source_versions {
            let current_version = self
                .conn
                .query_row(
                    "SELECT source_version FROM skill_sources WHERE skill_path = ?1",
                    params![path.display().to_string()],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?;
            if current_version.flatten() != expected_version.clone() {
                bail!(
                    "skill source {} changed after the update preview; preview the update again",
                    path.display()
                );
            }
        }
        Ok(())
    }

    pub fn persist_skill_update_persistence_for_workspace(
        &self,
        workspace_root: &Path,
        source_records: &[SkillSourceRecord],
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        self.persist_skill_update_persistence_for_workspace_with_deleted(
            workspace_root,
            &[],
            source_records,
            snapshots,
        )
    }

    pub fn persist_skill_update_persistence_for_workspace_checked(
        &self,
        workspace_root: &Path,
        expected_source_versions: &[(PathBuf, Option<String>)],
        source_records: &[SkillSourceRecord],
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        self.persist_skill_update_persistence_for_workspace_with_deleted_checked(
            workspace_root,
            &[],
            expected_source_versions,
            source_records,
            snapshots,
        )
    }

    pub fn persist_skill_update_persistence_for_workspace_with_deleted(
        &self,
        workspace_root: &Path,
        deleted_paths: &[PathBuf],
        source_records: &[SkillSourceRecord],
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        self.persist_skill_update_persistence_for_workspace_with_deleted_checked(
            workspace_root,
            deleted_paths,
            &[],
            source_records,
            snapshots,
        )
    }

    pub fn persist_skill_update_persistence_for_workspace_with_deleted_checked(
        &self,
        workspace_root: &Path,
        deleted_paths: &[PathBuf],
        expected_source_versions: &[(PathBuf, Option<String>)],
        source_records: &[SkillSourceRecord],
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        if deleted_paths.is_empty() && source_records.is_empty() && snapshots.is_empty() {
            return Ok(());
        }
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let tx = self.conn.unchecked_transaction()?;
        for (path, expected_version) in expected_source_versions {
            let current_version = tx
                .query_row(
                    "SELECT source_version FROM scoped_skill_sources
                     WHERE scope_key = ?1 AND skill_path = ?2",
                    params![scope_key.as_str(), path.display().to_string()],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?;
            if current_version.flatten() != expected_version.clone() {
                bail!(
                    "skill source {} changed after the update preview; preview the update again",
                    path.display()
                );
            }
        }
        for path in deleted_paths {
            tx.execute(
                "DELETE FROM scoped_skill_sources
                 WHERE scope_key = ?1 AND skill_path = ?2",
                params![scope_key.as_str(), path.display().to_string()],
            )?;
            tx.execute(
                "DELETE FROM scoped_skill_snapshots
                 WHERE scope_key = ?1 AND skill_path = ?2",
                params![scope_key.as_str(), path.display().to_string()],
            )?;
        }
        for record in source_records {
            tx.execute(
                "INSERT INTO scoped_skill_sources (
                    scope_key, skill_path, skill_name, source_kind, source, source_ref, source_version,
                    source_relative_path, update_status, origin, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(scope_key, skill_path) DO UPDATE SET
                    skill_name = excluded.skill_name,
                    source_kind = excluded.source_kind,
                    source = excluded.source,
                    source_ref = excluded.source_ref,
                    source_version = excluded.source_version,
                    source_relative_path = excluded.source_relative_path,
                    update_status = excluded.update_status,
                    origin = excluded.origin,
                    data_json = excluded.data_json",
                params![
                    scope_key.as_str(),
                    record.skill_path.display().to_string(),
                    record.skill_name,
                    record.source_kind,
                    record.source,
                    record.source_ref,
                    record.source_version,
                    record.source_relative_path,
                    record.update_status,
                    record.origin,
                    serde_json::to_string(record)?,
                ],
            )?;
        }
        for snapshot in snapshots {
            tx.execute(
                "DELETE FROM scoped_skill_snapshots
                 WHERE scope_key = ?1 AND skill_path = ?2",
                params![scope_key.as_str(), snapshot.skill_path.display().to_string()],
            )?;
            for file in &snapshot.files {
                tx.execute(
                    "INSERT INTO scoped_skill_snapshots (
                        scope_key, skill_path, source_version, relative_path, content
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        scope_key.as_str(),
                        snapshot.skill_path.display().to_string(),
                        snapshot.source_version,
                        file.relative_path,
                        file.content,
                    ],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn validate_skill_source_versions_for_workspace(
        &self,
        workspace_root: &Path,
        expected_source_versions: &[(PathBuf, Option<String>)],
    ) -> Result<()> {
        if expected_source_versions.is_empty() {
            return Ok(());
        }
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let tx = self.conn.unchecked_transaction()?;
        for (path, expected_version) in expected_source_versions {
            let current_version = tx
                .query_row(
                    "SELECT source_version FROM scoped_skill_sources
                     WHERE scope_key = ?1 AND skill_path = ?2",
                    params![scope_key.as_str(), path.display().to_string()],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?;
            if current_version.flatten() != expected_version.clone() {
                bail!(
                    "skill source {} changed after the update preview; preview the update again",
                    path.display()
                );
            }
        }
        tx.rollback()?;
        Ok(())
    }

    pub fn delete_skill_snapshots(&self, skill_paths: &[PathBuf]) -> Result<()> {
        if skill_paths.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        for skill_path in skill_paths {
            tx.execute(
                "DELETE FROM skill_snapshots WHERE skill_path = ?1",
                params![skill_path.display().to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }


    pub fn list_prompts(&self) -> Result<Vec<PromptRecord>> {
        with_database_read_lock_retry(|| self.list_prompts_once())
    }

    fn list_prompts_once(&self) -> Result<Vec<PromptRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, tags_json, body, created_at, updated_at
             FROM prompts
             ORDER BY updated_at DESC, title ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let tags_json = row.get::<_, String>(2)?;
            Ok(PromptRecord {
                id: row.get(0)?,
                title: row.get(1)?,
                tags: parse_prompt_tags(&tags_json)?,
                body: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn save_prompt(&self, prompt: PromptWrite) -> Result<PromptRecord> {
        let now = unix_now().to_string();
        let id = prompt
            .id
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(new_prompt_id);
        let title = prompt.title.trim().to_string();
        if title.is_empty() {
            anyhow::bail!("prompt title is required");
        }
        let tags = normalize_prompt_tags(prompt.tags);
        let category = tags.first().cloned().unwrap_or_default();
        let tags_json = serde_json::to_string(&tags)?;
        let body = prompt.body.trim_end().to_string();
        let created_at = self
            .conn
            .query_row(
                "SELECT created_at FROM prompts WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| now.clone());

        self.conn.execute(
            "INSERT INTO prompts (id, title, category, tags_json, body, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                category = excluded.category,
                tags_json = excluded.tags_json,
                body = excluded.body,
                updated_at = excluded.updated_at",
            params![id, title, category, tags_json, body, created_at, now],
        )?;

        Ok(PromptRecord {
            id,
            title,
            tags,
            body,
            created_at,
            updated_at: now,
        })
    }

    pub fn delete_prompts(&self, ids: &[String]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let mut deleted = 0;
        for id in ids {
            deleted += tx.execute("DELETE FROM prompts WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        Ok(deleted)
    }

    fn init(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS skills (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                name TEXT PRIMARY KEY,
                visibility TEXT NOT NULL,
                agents_json TEXT NOT NULL,
                description TEXT,
                is_system INTEGER NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS agents (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                kind TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                installed INTEGER NOT NULL,
                config_dir TEXT,
                executable TEXT,
                version TEXT,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS skill_paths (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                skill_name TEXT NOT NULL,
                path TEXT NOT NULL,
                root TEXT NOT NULL,
                scope TEXT NOT NULL,
                agent TEXT NOT NULL,
                sha256 TEXT NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS skill_sources (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                skill_path TEXT PRIMARY KEY,
                skill_name TEXT NOT NULL,
                source_kind TEXT NOT NULL,
                source TEXT,
                source_ref TEXT,
                source_version TEXT,
                source_relative_path TEXT,
                update_status TEXT NOT NULL,
                origin TEXT NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS skill_snapshots (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                skill_path TEXT NOT NULL,
                source_version TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                content BLOB NOT NULL,
                PRIMARY KEY (skill_path, relative_path)
            );
            CREATE TABLE IF NOT EXISTS scoped_skill_sources (
                scope_key TEXT NOT NULL,
                skill_path TEXT NOT NULL,
                skill_name TEXT NOT NULL,
                source_kind TEXT NOT NULL,
                source TEXT,
                source_ref TEXT,
                source_version TEXT,
                source_relative_path TEXT,
                update_status TEXT NOT NULL,
                origin TEXT NOT NULL,
                data_json TEXT NOT NULL,
                PRIMARY KEY (scope_key, skill_path)
            );
            CREATE TABLE IF NOT EXISTS scoped_skill_snapshots (
                scope_key TEXT NOT NULL,
                skill_path TEXT NOT NULL,
                source_version TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                content BLOB NOT NULL,
                PRIMARY KEY (scope_key, skill_path, relative_path)
            );
            CREATE INDEX IF NOT EXISTS idx_scoped_skill_sources_name
                ON scoped_skill_sources(scope_key, skill_name);
            CREATE INDEX IF NOT EXISTS idx_scoped_skill_snapshots_path
                ON scoped_skill_snapshots(scope_key, skill_path);
            CREATE INDEX IF NOT EXISTS idx_skill_sources_name
                ON skill_sources(skill_name);
            CREATE TABLE IF NOT EXISTS fs_manifest (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                source_kind TEXT NOT NULL,
                path TEXT NOT NULL,
                root TEXT NOT NULL,
                agent TEXT,
                scope TEXT,
                mtime_ns INTEGER,
                size INTEGER,
                inode INTEGER,
                device INTEGER,
                sha256 TEXT,
                parser_version TEXT NOT NULL,
                last_seen_at INTEGER NOT NULL,
                parse_status TEXT NOT NULL,
                PRIMARY KEY (scope_key, source_kind, path)
            );
            CREATE INDEX IF NOT EXISTS idx_fs_manifest_root_kind_path
                ON fs_manifest(scope_key, root, source_kind, path);
            CREATE TABLE IF NOT EXISTS projection_contexts (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                domain TEXT PRIMARY KEY,
                workspace_root TEXT NOT NULL,
                state TEXT NOT NULL,
                scanned_at INTEGER,
                error TEXT,
                parser_version TEXT NOT NULL
            );
            -- Workspace projections are canonical payloads. The older per-field
            -- tables remain write-side indexes for legacy APIs, while all
            -- workspace reads use these scope-keyed rows.
            CREATE TABLE IF NOT EXISTS normalized_snapshots (
                scope_key TEXT NOT NULL,
                domain TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                source_version TEXT,
                revision INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (scope_key, domain)
            );
            CREATE TABLE IF NOT EXISTS scoped_projection_contexts (
                scope_key TEXT NOT NULL,
                domain TEXT NOT NULL,
                state TEXT NOT NULL,
                scanned_at INTEGER,
                error TEXT,
                parser_version TEXT NOT NULL,
                PRIMARY KEY (scope_key, domain)
            );
            CREATE INDEX IF NOT EXISTS idx_normalized_snapshots_domain
                ON normalized_snapshots(domain, updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_scoped_projection_contexts_domain
                ON scoped_projection_contexts(domain, state, scanned_at DESC);
            CREATE INDEX IF NOT EXISTS idx_projection_contexts_workspace
                ON projection_contexts(workspace_root, domain);
            CREATE TABLE IF NOT EXISTS projection_heads (
                scope_key TEXT NOT NULL,
                domain TEXT NOT NULL,
                revision INTEGER NOT NULL,
                source_version TEXT,
                schema_version INTEGER NOT NULL DEFAULT 1,
                status TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (scope_key, domain)
            );
            CREATE TABLE IF NOT EXISTS operation_journal (
                operation_id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                scope_key TEXT NOT NULL,
                status TEXT NOT NULL,
                input_revision INTEGER NOT NULL,
                source_version TEXT,
                checkpoint_json TEXT,
                error TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_operation_journal_scope_status
                ON operation_journal(scope_key, status, updated_at);
            CREATE TABLE IF NOT EXISTS sessions (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                id TEXT NOT NULL,
                agent TEXT NOT NULL,
                title TEXT,
                project TEXT,
                path TEXT NOT NULL,
                started_at TEXT,
                updated_at TEXT,
                message_count INTEGER,
                first_user_message TEXT,
                last_user_message TEXT,
                last_assistant_message TEXT,
                data_json TEXT NOT NULL,
                PRIMARY KEY (id, agent, path)
            );
            -- Session rows are scoped because more than one daemon/workspace can
            -- share the same SQLite file. The legacy `sessions` table remains the
            -- unscoped API used by CLI and migration code.
            CREATE TABLE IF NOT EXISTS scoped_sessions (
                scope_key TEXT NOT NULL,
                id TEXT NOT NULL,
                agent TEXT NOT NULL,
                title TEXT,
                project TEXT,
                path TEXT NOT NULL,
                started_at TEXT,
                updated_at TEXT,
                message_count INTEGER,
                first_user_message TEXT,
                last_user_message TEXT,
                last_assistant_message TEXT,
                data_json TEXT NOT NULL,
                PRIMARY KEY (scope_key, id, agent, path)
            );
            CREATE INDEX IF NOT EXISTS idx_scoped_sessions_updated_id
                ON scoped_sessions(updated_at DESC, id ASC);
            CREATE INDEX IF NOT EXISTS idx_scoped_sessions_agent_updated_id
                ON scoped_sessions(agent, updated_at DESC, id ASC);
            CREATE INDEX IF NOT EXISTS idx_scoped_sessions_path
                ON scoped_sessions(path);
            CREATE TABLE IF NOT EXISTS scoped_session_scan_sources (
                scope_key TEXT NOT NULL,
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                source_path TEXT NOT NULL,
                file_mtime INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                parser_version TEXT NOT NULL,
                PRIMARY KEY (scope_key, session_id, agent, session_path, source_path)
            );
            CREATE INDEX IF NOT EXISTS idx_scoped_session_scan_sources_source
                ON scoped_session_scan_sources(scope_key, source_path);
            CREATE TABLE IF NOT EXISTS session_projects (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                id TEXT NOT NULL,
                name TEXT NOT NULL,
                name_custom INTEGER NOT NULL DEFAULT 0,
                last_seen_at TEXT NOT NULL DEFAULT '',
                PRIMARY KEY (scope_key, id)
            );
            CREATE TABLE IF NOT EXISTS session_project_aliases (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                project_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY (scope_key, kind, value)
            );
            CREATE INDEX IF NOT EXISTS idx_session_project_aliases_project
                ON session_project_aliases(project_id);
            CREATE TABLE IF NOT EXISTS project_scan_scopes (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                id TEXT PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                enabled INTEGER NOT NULL DEFAULT 1,
                last_scanned_at TEXT
            );
            CREATE TABLE IF NOT EXISTS projects (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                id TEXT PRIMARY KEY,
                root_path TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                remote_url TEXT,
                scope_id TEXT NOT NULL,
                status TEXT NOT NULL,
                last_scanned_at TEXT NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_projects_scope_status
                ON projects(scope_id, status);
            CREATE TABLE IF NOT EXISTS session_skill_index (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                file_mtime INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                indexed_at TEXT,
                status TEXT NOT NULL,
                error TEXT,
                PRIMARY KEY (session_id, agent, session_path)
            );
            CREATE TABLE IF NOT EXISTS session_skill_links (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                skill_name TEXT NOT NULL,
                skill_path TEXT NOT NULL,
                skill_agent TEXT,
                skill_scope TEXT,
                evidence_kind TEXT NOT NULL,
                evidence_text TEXT NOT NULL,
                evidence_time TEXT,
                confidence TEXT NOT NULL,
                PRIMARY KEY (session_id, agent, session_path, skill_path)
            );
            CREATE TABLE IF NOT EXISTS scoped_session_skill_index (
                scope_key TEXT NOT NULL,
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                file_mtime INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                indexed_at TEXT,
                status TEXT NOT NULL,
                error TEXT,
                PRIMARY KEY (scope_key, session_id, agent, session_path)
            );
            CREATE TABLE IF NOT EXISTS scoped_session_skill_links (
                scope_key TEXT NOT NULL,
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                skill_name TEXT NOT NULL,
                skill_path TEXT NOT NULL,
                skill_agent TEXT,
                skill_scope TEXT,
                evidence_kind TEXT NOT NULL,
                evidence_text TEXT NOT NULL,
                evidence_time TEXT,
                confidence TEXT NOT NULL,
                PRIMARY KEY (scope_key, session_id, agent, session_path, skill_path)
            );
            CREATE TABLE IF NOT EXISTS session_search_index (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                file_mtime INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                indexed_at TEXT NOT NULL,
                search_metadata TEXT NOT NULL DEFAULT '',
                search_index_version INTEGER NOT NULL DEFAULT 1,
                PRIMARY KEY (session_id, agent, session_path)
            );
            CREATE TABLE IF NOT EXISTS session_analytics (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                file_mtime INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                indexed_at TEXT NOT NULL,
                analytics_json TEXT NOT NULL,
                parser_state_json TEXT NOT NULL,
                event_min_date TEXT,
                event_max_date TEXT,
                has_activity INTEGER NOT NULL DEFAULT 0,
                capability_token_usage INTEGER NOT NULL DEFAULT 0,
                capability_reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                capability_explicit_runs INTEGER NOT NULL DEFAULT 0,
                capability_rate_limit_history INTEGER NOT NULL DEFAULT 0,
                overview_indexed INTEGER NOT NULL DEFAULT 1,
                overview_index_error TEXT,
                PRIMARY KEY (session_id, agent, session_path)
            );
            CREATE TABLE IF NOT EXISTS session_analytics_overview (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                event_min_date TEXT,
                event_max_date TEXT,
                has_activity INTEGER NOT NULL DEFAULT 0,
                overview_json TEXT NOT NULL,
                PRIMARY KEY (session_id, agent, session_path)
            );
            CREATE TABLE IF NOT EXISTS scoped_session_analytics (
                scope_key TEXT NOT NULL,
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                file_mtime INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                indexed_at TEXT NOT NULL,
                analytics_json TEXT NOT NULL,
                parser_state_json TEXT NOT NULL,
                event_min_date TEXT,
                event_max_date TEXT,
                has_activity INTEGER NOT NULL DEFAULT 0,
                capability_token_usage INTEGER NOT NULL DEFAULT 0,
                capability_reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                capability_explicit_runs INTEGER NOT NULL DEFAULT 0,
                capability_rate_limit_history INTEGER NOT NULL DEFAULT 0,
                overview_indexed INTEGER NOT NULL DEFAULT 1,
                overview_index_error TEXT,
                PRIMARY KEY (scope_key, session_id, agent, session_path)
            );
            CREATE TABLE IF NOT EXISTS scoped_session_analytics_overview (
                scope_key TEXT NOT NULL,
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                event_min_date TEXT,
                event_max_date TEXT,
                has_activity INTEGER NOT NULL DEFAULT 0,
                overview_json TEXT NOT NULL,
                PRIMARY KEY (scope_key, session_id, agent, session_path)
            );
            CREATE TABLE IF NOT EXISTS session_search_records (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                record_order INTEGER NOT NULL,
                metadata_text TEXT NOT NULL DEFAULT '',
                title TEXT NOT NULL DEFAULT '',
                project TEXT NOT NULL DEFAULT '',
                user_text TEXT NOT NULL,
                assistant_text TEXT NOT NULL,
                UNIQUE (session_id, agent, session_path, record_order)
            );
            CREATE TABLE IF NOT EXISTS scoped_session_search_index (
                scope_key TEXT NOT NULL,
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                file_mtime INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                indexed_at TEXT NOT NULL,
                search_metadata TEXT NOT NULL DEFAULT '',
                search_index_version INTEGER NOT NULL DEFAULT 1,
                PRIMARY KEY (scope_key, session_id, agent, session_path)
            );
            CREATE TABLE IF NOT EXISTS scoped_session_search_records (
                scope_key TEXT NOT NULL,
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                record_order INTEGER NOT NULL,
                metadata_text TEXT NOT NULL DEFAULT '',
                title TEXT NOT NULL DEFAULT '',
                project TEXT NOT NULL DEFAULT '',
                user_text TEXT NOT NULL,
                assistant_text TEXT NOT NULL,
                UNIQUE (scope_key, session_id, agent, session_path, record_order)
            );
            CREATE INDEX IF NOT EXISTS idx_session_skill_links_session
                ON session_skill_links(session_id, agent);
            CREATE INDEX IF NOT EXISTS idx_session_skill_links_skill
                ON session_skill_links(skill_name);
            CREATE INDEX IF NOT EXISTS idx_session_skill_links_path
                ON session_skill_links(skill_path);
            CREATE INDEX IF NOT EXISTS idx_scoped_session_skill_links_session
                ON scoped_session_skill_links(scope_key, session_id, agent);
            CREATE INDEX IF NOT EXISTS idx_scoped_session_skill_links_skill
                ON scoped_session_skill_links(scope_key, skill_name);
            CREATE INDEX IF NOT EXISTS idx_session_search_index_session
                ON session_search_index(session_id, agent);
            CREATE INDEX IF NOT EXISTS idx_session_search_records_session
                ON session_search_records(session_id, agent, session_path);
            CREATE INDEX IF NOT EXISTS idx_scoped_session_search_index_session
                ON scoped_session_search_index(scope_key, session_id, agent, session_path);
            CREATE INDEX IF NOT EXISTS idx_scoped_session_search_records_session
                ON scoped_session_search_records(scope_key, session_id, agent, session_path);
            CREATE TABLE IF NOT EXISTS rules (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent TEXT NOT NULL,
                kind TEXT NOT NULL,
                scope TEXT NOT NULL,
                path TEXT NOT NULL,
                effective_order INTEGER NOT NULL,
                sha256 TEXT NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS hooks (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent TEXT NOT NULL,
                event TEXT NOT NULL,
                command TEXT,
                enabled INTEGER NOT NULL,
                path TEXT NOT NULL,
                trust_hash TEXT NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS mcp_servers (
                scope_key TEXT NOT NULL DEFAULT 'installation:default',
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent TEXT NOT NULL,
                name TEXT NOT NULL,
                transport TEXT NOT NULL,
                status TEXT NOT NULL,
                path TEXT NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prompts (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                category TEXT NOT NULL,
                tags_json TEXT NOT NULL DEFAULT '[]',
                body TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;
        self.ensure_scope_columns()?;
        self.ensure_prompt_tags_column()?;
        self.ensure_skill_source_ref_column()?;
        self.ensure_session_preview_columns()?;
        self.ensure_session_payload_previews()?;
        self.ensure_session_scan_source_version_column()?;
        self.ensure_session_analytics_overview_columns()?;
        self.ensure_session_search_index_version_column()?;
        self.ensure_storage_indexes()?;
        self.ensure_session_search_fts()?;
        self.conn
            .execute_batch(&format!("PRAGMA user_version = {STORAGE_SCHEMA_VERSION};"))?;
        Ok(())
    }

    /// Older databases predate workspace-scoped projection rows. Keep the
    /// migration additive so opening one never drops user data; all new writes
    /// still use the canonical scoped snapshot tables.
    fn ensure_scope_columns(&self) -> Result<()> {
        let tables = [
            "skills",
            "agents",
            "skill_paths",
            "skill_sources",
            "skill_snapshots",
            "fs_manifest",
            "projection_contexts",
            "sessions",
            "session_projects",
            "session_project_aliases",
            "project_scan_scopes",
            "projects",
            "session_skill_index",
            "session_skill_links",
            "session_search_index",
            "session_analytics",
            "session_analytics_overview",
            "session_search_records",
            "rules",
            "hooks",
            "mcp_servers",
        ];
        let tx = self.conn.unchecked_transaction()?;
        for table in tables {
            let has_scope = tx
                .prepare(&format!("PRAGMA table_info({table})"))?
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .any(|column| column == "scope_key");
            if !has_scope {
                tx.execute(
                    &format!(
                        "ALTER TABLE {table} ADD COLUMN scope_key TEXT NOT NULL DEFAULT 'installation:default'"
                    ),
                    [],
                )?;
            }
        }
        tx.commit()?;
        self.ensure_scoped_fs_manifest()?;
        Ok(())
    }

    fn ensure_storage_indexes(&self) -> Result<()> {
        self.ensure_skill_path_unique_index()?;
        self.conn.execute_batch(
            "
            CREATE INDEX IF NOT EXISTS idx_sessions_updated_id
                ON sessions(updated_at DESC, id ASC);
            CREATE INDEX IF NOT EXISTS idx_sessions_agent_updated_id
                ON sessions(agent, updated_at DESC, id ASC);
            CREATE INDEX IF NOT EXISTS idx_sessions_path
                ON sessions(path);
            CREATE INDEX IF NOT EXISTS idx_skill_sources_name_path
                ON skill_sources(skill_name, skill_path);
            CREATE INDEX IF NOT EXISTS idx_rules_agent_order
                ON rules(agent, effective_order);
            CREATE INDEX IF NOT EXISTS idx_rules_agent_kind_scope_path_order
                ON rules(agent, kind, scope, path, effective_order);
            CREATE INDEX IF NOT EXISTS idx_rules_path
                ON rules(path);
            CREATE INDEX IF NOT EXISTS idx_hooks_agent_event_enabled
                ON hooks(agent, event, enabled);
            CREATE INDEX IF NOT EXISTS idx_hooks_agent_event_path
                ON hooks(agent, event, path);
            CREATE INDEX IF NOT EXISTS idx_hooks_path
                ON hooks(path);
            CREATE INDEX IF NOT EXISTS idx_mcp_servers_agent_status
                ON mcp_servers(agent, status);
            CREATE INDEX IF NOT EXISTS idx_mcp_servers_agent_name_path
                ON mcp_servers(agent, name, path);
            CREATE INDEX IF NOT EXISTS idx_mcp_servers_agent_name_transport_status_path
                ON mcp_servers(agent, name, transport, status, path);
            CREATE INDEX IF NOT EXISTS idx_prompts_updated_title
                ON prompts(updated_at DESC, title ASC);
            ",
        )?;
        Ok(())
    }

    fn ensure_skill_path_unique_index(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM skill_paths
             WHERE id NOT IN (
                 SELECT MAX(id)
                 FROM skill_paths
                 GROUP BY skill_name, path
             )",
            [],
        )?;
        tx.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS ux_skill_paths_name_path
             ON skill_paths(skill_name, path);",
        )?;
        tx.commit()?;
        Ok(())
    }

    fn ensure_skill_source_ref_column(&self) -> Result<()> {
        let mut statement = self.conn.prepare("PRAGMA table_info(skill_sources)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if !columns.iter().any(|column| column == "source_ref") {
            self.conn
                .execute("ALTER TABLE skill_sources ADD COLUMN source_ref TEXT", [])?;
        }
        Ok(())
    }

    fn ensure_session_analytics_overview_columns(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let mut stmt = tx.prepare("PRAGMA table_info(session_analytics)")?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<HashSet<_>, _>>()?;
        drop(stmt);
        for (name, definition) in [
            ("event_min_date", "TEXT"),
            ("event_max_date", "TEXT"),
            ("has_activity", "INTEGER NOT NULL DEFAULT 0"),
            ("capability_token_usage", "INTEGER NOT NULL DEFAULT 0"),
            ("capability_reasoning_tokens", "INTEGER NOT NULL DEFAULT 0"),
            ("capability_explicit_runs", "INTEGER NOT NULL DEFAULT 0"),
            (
                "capability_rate_limit_history",
                "INTEGER NOT NULL DEFAULT 0",
            ),
            ("overview_indexed", "INTEGER NOT NULL DEFAULT 0"),
            ("overview_index_error", "TEXT"),
        ] {
            if !columns.contains(name) {
                tx.execute(
                    &format!("ALTER TABLE session_analytics ADD COLUMN {name} {definition}"),
                    [],
                )?;
            }
        }
        tx.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_session_analytics_agent_max_date
             ON session_analytics(agent, event_max_date);
             CREATE INDEX IF NOT EXISTS idx_session_analytics_overview_agent_max_date
             ON session_analytics_overview(agent, event_max_date);
             CREATE INDEX IF NOT EXISTS idx_scoped_session_analytics_scope_date
             ON scoped_session_analytics(scope_key, agent, event_max_date);
             CREATE INDEX IF NOT EXISTS idx_scoped_session_analytics_overview_scope_date
             ON scoped_session_analytics_overview(scope_key, agent, event_max_date);",
        )?;
        tx.commit()?;
        Ok(())
    }

    fn ensure_session_preview_columns(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let mut stmt = tx.prepare("PRAGMA table_info(sessions)")?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for (name, definition) in [
            ("first_user_message", "TEXT"),
            ("last_user_message", "TEXT"),
            ("last_assistant_message", "TEXT"),
        ] {
            if !columns.iter().any(|column| column == name) {
                tx.execute(
                    &format!("ALTER TABLE sessions ADD COLUMN {name} {definition}"),
                    [],
                )?;
            }
        }

        let already_backfilled = tx
            .query_row(
                "SELECT 1 FROM meta WHERE key = 'session_previews_backfilled' LIMIT 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if !already_backfilled {
            tx.execute_batch(&format!(
                "UPDATE sessions AS target
                 SET first_user_message = COALESCE(
                         NULLIF(target.first_user_message, ''),
                         (SELECT substr(records.user_text, 1, {SESSION_PREVIEW_MAX_CHARS})
                          FROM session_search_records AS records
                          WHERE records.session_id = target.id
                            AND records.agent = target.agent
                            AND records.session_path = target.path
                            AND records.user_text <> ''
                          ORDER BY records.record_order ASC
                          LIMIT 1)
                     ),
                     last_user_message = COALESCE(
                         NULLIF(target.last_user_message, ''),
                         (SELECT substr(records.user_text, 1, {SESSION_PREVIEW_MAX_CHARS})
                          FROM session_search_records AS records
                          WHERE records.session_id = target.id
                            AND records.agent = target.agent
                            AND records.session_path = target.path
                            AND records.user_text <> ''
                          ORDER BY records.record_order DESC
                          LIMIT 1)
                     ),
                     last_assistant_message = COALESCE(
                         NULLIF(target.last_assistant_message, ''),
                         (SELECT substr(records.assistant_text, 1, {SESSION_PREVIEW_MAX_CHARS})
                          FROM session_search_records AS records
                          WHERE records.session_id = target.id
                            AND records.agent = target.agent
                            AND records.session_path = target.path
                            AND records.assistant_text <> ''
                          ORDER BY records.record_order DESC
                          LIMIT 1)
                     )
                 WHERE target.first_user_message IS NULL
                    OR target.last_user_message IS NULL
                    OR target.last_assistant_message IS NULL;
                 INSERT INTO meta (key, value)
                 VALUES ('session_previews_backfilled', '1')
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value;"
            ))?;
        }
        tx.commit()?;
        Ok(())
    }

    fn ensure_session_scan_source_version_column(&self) -> Result<()> {
        let mut statement = self
            .conn
            .prepare("PRAGMA table_info(scoped_session_scan_sources)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if !columns.iter().any(|column| column == "parser_version") {
            self.conn.execute(
                "ALTER TABLE scoped_session_scan_sources
                 ADD COLUMN parser_version TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        Ok(())
    }

    fn non_empty_session_preview(value: Option<String>) -> Option<String> {
        value.filter(|value| !value.trim().is_empty())
    }

    fn session_search_preview(
        tx: &Transaction<'_>,
        id: &str,
        agent: &str,
        path: &str,
        column: &str,
        descending: bool,
    ) -> Result<Option<String>> {
        let column = match column {
            "user_text" | "assistant_text" => column,
            _ => unreachable!("session preview search column is fixed by the caller"),
        };
        let order = if descending { "DESC" } else { "ASC" };
        tx.query_row(
            &format!(
                "SELECT {column}
                 FROM session_search_records
                 WHERE session_id = ?1 AND agent = ?2 AND session_path = ?3
                   AND trim({column}) <> ''
                 ORDER BY record_order {order}
                 LIMIT 1"
            ),
            params![id, agent, path],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(Into::into)
    }

    fn session_preview_from_legacy_or_search(
        tx: &Transaction<'_>,
        id: &str,
        agent: &str,
        path: &str,
        legacy: Option<String>,
        column: &str,
        descending: bool,
    ) -> Result<Option<String>> {
        if let Some(value) = Self::non_empty_session_preview(legacy) {
            return Ok(Some(value));
        }
        Self::session_search_preview(tx, id, agent, path, column, descending)
    }

    fn ensure_session_payload_previews(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let already_migrated = tx
            .query_row(
                "SELECT 1 FROM meta WHERE key = ?1 LIMIT 1",
                params![SESSION_PAYLOAD_PREVIEWS_MIGRATION_KEY],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if already_migrated {
            tx.commit()?;
            return Ok(());
        }

        let rows = {
            let mut statement = tx.prepare(
                "SELECT id, agent, path, data_json,
                        first_user_message, last_user_message, last_assistant_message
                 FROM sessions",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        for (id, agent, path, data_json, scalar_first, scalar_last_user, scalar_last_assistant) in
            rows
        {
            let Ok(mut session) = serde_json::from_str::<SessionRecord>(&data_json) else {
                continue;
            };

            if session
                .first_user_message
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                session.first_user_message = Self::session_preview_from_legacy_or_search(
                    &tx,
                    &id,
                    &agent,
                    &path,
                    scalar_first.clone(),
                    "user_text",
                    false,
                )?;
            }
            if session
                .last_user_message
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                session.last_user_message = Self::session_preview_from_legacy_or_search(
                    &tx,
                    &id,
                    &agent,
                    &path,
                    scalar_last_user.clone(),
                    "user_text",
                    true,
                )?;
            }
            if session
                .last_assistant_message
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                session.last_assistant_message = Self::session_preview_from_legacy_or_search(
                    &tx,
                    &id,
                    &agent,
                    &path,
                    scalar_last_assistant.clone(),
                    "assistant_text",
                    true,
                )?;
            }

            let first_user_message = bound_session_preview(session.first_user_message.clone());
            let last_user_message = bound_session_preview(session.last_user_message.clone());
            let last_assistant_message =
                bound_session_preview(session.last_assistant_message.clone());
            session.first_user_message = first_user_message.clone();
            session.last_user_message = last_user_message.clone();
            session.last_assistant_message = last_assistant_message.clone();
            let migrated_data_json = Self::session_metadata_json(&session)?;
            if data_json == migrated_data_json
                && scalar_first == first_user_message
                && scalar_last_user == last_user_message
                && scalar_last_assistant == last_assistant_message
            {
                continue;
            }
            tx.execute(
                "UPDATE sessions
                 SET first_user_message = ?1,
                     last_user_message = ?2,
                     last_assistant_message = ?3,
                     data_json = ?4
                 WHERE id = ?5 AND agent = ?6 AND path = ?7",
                params![
                    first_user_message,
                    last_user_message,
                    last_assistant_message,
                    migrated_data_json,
                    id,
                    agent,
                    path,
                ],
            )?;
        }

        tx.execute(
            "INSERT INTO meta (key, value)
             VALUES (?1, '1')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![SESSION_PAYLOAD_PREVIEWS_MIGRATION_KEY],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn ensure_session_search_index_version_column(&self) -> Result<()> {
        let mut statement = self
            .conn
            .prepare("PRAGMA table_info(session_search_index)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if !columns
            .iter()
            .any(|column| column == "search_index_version")
        {
            self.conn.execute(
                "ALTER TABLE session_search_index
                 ADD COLUMN search_index_version INTEGER NOT NULL DEFAULT 1",
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_prompt_tags_column(&self) -> Result<()> {
        let mut stmt = self.conn.prepare("PRAGMA table_info(prompts)")?;
        let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut has_tags_json = false;
        for column in columns {
            if column? == "tags_json" {
                has_tags_json = true;
                break;
            }
        }
        if !has_tags_json {
            self.conn.execute(
                "ALTER TABLE prompts ADD COLUMN tags_json TEXT NOT NULL DEFAULT '[]'",
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_session_search_fts(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE VIRTUAL TABLE IF NOT EXISTS session_search_fts USING fts5(
                metadata_text,
                title,
                project,
                user_text,
                assistant_text,
                content = 'session_search_records',
                content_rowid = 'rowid',
                tokenize = 'trigram case_sensitive 0'
            );
            CREATE TRIGGER IF NOT EXISTS session_search_records_ai
            AFTER INSERT ON session_search_records BEGIN
                INSERT INTO session_search_fts(
                    rowid, metadata_text, title, project,
                    user_text, assistant_text
                )
                VALUES (
                    new.rowid, new.metadata_text, new.title, new.project,
                    new.user_text, new.assistant_text
                );
            END;
            CREATE TRIGGER IF NOT EXISTS session_search_records_ad
            AFTER DELETE ON session_search_records BEGIN
                INSERT INTO session_search_fts(
                    session_search_fts, rowid, metadata_text, title, project,
                    user_text, assistant_text
                )
                VALUES (
                    'delete', old.rowid, old.metadata_text, old.title, old.project,
                    old.user_text, old.assistant_text
                );
            END;
            CREATE TRIGGER IF NOT EXISTS session_search_records_au
            AFTER UPDATE ON session_search_records BEGIN
                INSERT INTO session_search_fts(
                    session_search_fts, rowid, metadata_text, title, project,
                    user_text, assistant_text
                )
                VALUES (
                    'delete', old.rowid, old.metadata_text, old.title, old.project,
                    old.user_text, old.assistant_text
                );
                INSERT INTO session_search_fts(
                    rowid, metadata_text, title, project,
                    user_text, assistant_text
                )
                VALUES (
                    new.rowid, new.metadata_text, new.title, new.project,
                    new.user_text, new.assistant_text
                );
            END;
            CREATE VIRTUAL TABLE IF NOT EXISTS scoped_session_search_fts USING fts5(
                metadata_text,
                title,
                project,
                user_text,
                assistant_text,
                content = 'scoped_session_search_records',
                content_rowid = 'id',
                tokenize = 'trigram case_sensitive 0'
            );
            CREATE TRIGGER IF NOT EXISTS scoped_session_search_records_ai
            AFTER INSERT ON scoped_session_search_records BEGIN
                INSERT INTO scoped_session_search_fts(
                    rowid, metadata_text, title, project,
                    user_text, assistant_text
                )
                VALUES (
                    new.id, new.metadata_text, new.title, new.project,
                    new.user_text, new.assistant_text
                );
            END;
            CREATE TRIGGER IF NOT EXISTS scoped_session_search_records_ad
            AFTER DELETE ON scoped_session_search_records BEGIN
                INSERT INTO scoped_session_search_fts(
                    scoped_session_search_fts, rowid, metadata_text, title, project,
                    user_text, assistant_text
                )
                VALUES (
                    'delete', old.id, old.metadata_text, old.title, old.project,
                    old.user_text, old.assistant_text
                );
            END;
            CREATE TRIGGER IF NOT EXISTS scoped_session_search_records_au
            AFTER UPDATE ON scoped_session_search_records BEGIN
                INSERT INTO scoped_session_search_fts(
                    scoped_session_search_fts, rowid, metadata_text, title, project,
                    user_text, assistant_text
                )
                VALUES (
                    'delete', old.id, old.metadata_text, old.title, old.project,
                    old.user_text, old.assistant_text
                );
                INSERT INTO scoped_session_search_fts(
                    rowid, metadata_text, title, project,
                    user_text, assistant_text
                )
                VALUES (
                    new.id, new.metadata_text, new.title, new.project,
                    new.user_text, new.assistant_text
                );
            END;
            ",
        )?;
        Ok(())
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

fn load_session_projects(
    tx: &Transaction<'_>,
    scope_key: &ScopeKey,
) -> Result<HashMap<String, ProjectState>> {
    let mut stmt = tx.prepare(
        "SELECT id, name, name_custom, last_seen_at
         FROM session_projects
         WHERE scope_key = ?1",
    )?;
    let rows = stmt.query_map([scope_key.as_str()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            ProjectState {
                name: row.get(1)?,
                name_custom: row.get(2)?,
                last_seen_at: row.get(3)?,
            },
        ))
    })?;
    rows.collect::<std::result::Result<HashMap<_, _>, _>>()
        .map_err(Into::into)
}

fn load_session_project_aliases(
    tx: &Transaction<'_>,
    scope_key: &ScopeKey,
) -> Result<HashMap<SessionProjectAlias, String>> {
    let mut stmt = tx.prepare(
        "SELECT kind, value, project_id
         FROM session_project_aliases
         WHERE scope_key = ?1",
    )?;
    let rows = stmt.query_map([scope_key.as_str()], |row| {
        Ok(((row.get(0)?, row.get(1)?), row.get(2)?))
    })?;
    rows.collect::<std::result::Result<HashMap<_, _>, _>>()
        .map_err(Into::into)
}

fn merge_session_project_rows(
    tx: &Transaction<'_>,
    target_project_id: &str,
    source_project_id: &str,
    scope_key: &ScopeKey,
) -> Result<()> {
    if target_project_id == source_project_id {
        return Ok(());
    }
    tx.execute(
        "UPDATE session_project_aliases SET project_id = ?1
         WHERE scope_key = ?3 AND project_id = ?2",
        params![target_project_id, source_project_id, scope_key.as_str()],
    )?;
    tx.execute(
        "DELETE FROM session_projects WHERE scope_key = ?2 AND id = ?1",
        params![source_project_id, scope_key.as_str()],
    )?;
    Ok(())
}

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

fn agent_label(agent: AgentKind) -> &'static str {
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
        "skills" => vec!["skill", "skill-dir", "skill-candidate", "skill-root"],
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
    for ancestor in workspace_root.ancestors() {
        for provider in crate::providers::all_providers() {
            for directory in provider.projection_directories() {
                candidates.insert(ancestor.join(directory));
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        for provider in crate::providers::all_providers() {
            for directory in provider.projection_directories() {
                candidates.insert(home.join(directory));
            }
        }
        for path in domain_candidate_files(domain, &home) {
            candidates.insert(path);
        }
    }
    for path in domain_candidate_files(domain, &workspace_root) {
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
    if domain == "skills" {
        paths.extend(crate::skills::skill_provenance_candidate_files(root));
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

fn manifest_entries_for_skills(scan: &SkillScan, workspace_root: &Path) -> Vec<FsManifestEntry> {
    let mut entries = Vec::new();
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

fn advance_projection_head_in_tx(
    tx: &Transaction<'_>,
    scope_key: &ScopeKey,
    domain: &str,
    source_version: Option<&SourceVersion>,
    status: &str,
) -> Result<ProjectionHead> {
    if domain.trim().is_empty() {
        anyhow::bail!("projection domain must not be empty");
    }
    if status.trim().is_empty() {
        anyhow::bail!("projection status must not be empty");
    }
    let current = tx
        .query_row(
            "SELECT revision FROM projection_heads
             WHERE scope_key = ?1 AND domain = ?2",
            params![scope_key.as_str(), domain],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0);
    let revision = current
        .checked_add(1)
        .context("projection revision overflow")?;
    let updated_at = i64::try_from(unix_now()).context("invalid projection timestamp")?;
    tx.execute(
        "INSERT INTO projection_heads
            (scope_key, domain, revision, source_version, schema_version, status, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(scope_key, domain) DO UPDATE SET
            revision = excluded.revision,
            source_version = excluded.source_version,
            schema_version = excluded.schema_version,
            status = excluded.status,
            updated_at = excluded.updated_at",
        params![
            scope_key.as_str(),
            domain,
            revision,
            source_version.map(SourceVersion::as_str),
            1_i64,
            status,
            updated_at,
        ],
    )?;
    Ok(ProjectionHead {
        scope_key: scope_key.clone(),
        domain: domain.to_string(),
        revision: Revision::new(u64::try_from(revision)?),
        source_version: source_version.cloned(),
        schema_version: 1,
        status: status.to_string(),
    })
}


fn bump_analytics_revision(conn: &rusqlite::Transaction<'_>) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('analytics_revision', '1')
         ON CONFLICT(key) DO UPDATE
         SET value = CAST(CAST(meta.value AS INTEGER) + 1 AS TEXT)",
        [],
    )?;
    Ok(())
}

fn cleanup_stale_scoped_session_skill_rows(
    conn: &rusqlite::Transaction<'_>,
    scope_key: &ScopeKey,
) -> Result<()> {
    conn.execute(
        "DELETE FROM scoped_session_skill_links
         WHERE scope_key = ?1
           AND NOT EXISTS (
            SELECT 1 FROM scoped_sessions
            WHERE scoped_sessions.scope_key = scoped_session_skill_links.scope_key
              AND scoped_sessions.id = scoped_session_skill_links.session_id
              AND scoped_sessions.agent = scoped_session_skill_links.agent
              AND scoped_sessions.path = scoped_session_skill_links.session_path
         )",
        [scope_key.as_str()],
    )?;
    conn.execute(
        "DELETE FROM scoped_session_skill_index
         WHERE scope_key = ?1
           AND NOT EXISTS (
            SELECT 1 FROM scoped_sessions
            WHERE scoped_sessions.scope_key = scoped_session_skill_index.scope_key
              AND scoped_sessions.id = scoped_session_skill_index.session_id
              AND scoped_sessions.agent = scoped_session_skill_index.agent
              AND scoped_sessions.path = scoped_session_skill_index.session_path
         )",
        [scope_key.as_str()],
    )?;
    Ok(())
}

fn replace_scoped_session_scan_sources(
    conn: &rusqlite::Transaction<'_>,
    scope_key: &ScopeKey,
    session: &SessionRecord,
) -> Result<()> {
    conn.execute(
        &format!(
            "DELETE FROM {SCOPED_SESSION_SCAN_SOURCE_TABLE}
             WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3"
        ),
        params![scope_key.as_str(), session.id, agent_label(session.agent)],
    )?;
    let session_path = session.path.display().to_string();
    for source in session_scan_source_states(session) {
        conn.execute(
            &format!(
                "INSERT INTO {SCOPED_SESSION_SCAN_SOURCE_TABLE}
                 (scope_key, session_id, agent, session_path, source_path, file_mtime, file_size, parser_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
            ),
            params![
                scope_key.as_str(),
                session.id,
                agent_label(session.agent),
                session_path,
                source.path.display().to_string(),
                source.file_mtime,
                source.file_size,
                SESSION_SCAN_CACHE_PARSER_VERSION,
            ],
        )?;
    }
    Ok(())
}

fn cleanup_stale_scoped_session_scan_source_rows(
    conn: &rusqlite::Transaction<'_>,
    scope_key: &ScopeKey,
) -> Result<()> {
    conn.execute(
        &format!(
            "DELETE FROM {SCOPED_SESSION_SCAN_SOURCE_TABLE}
             WHERE scope_key = ?1
               AND NOT EXISTS (
                SELECT 1 FROM scoped_sessions
                WHERE scoped_sessions.scope_key = {SCOPED_SESSION_SCAN_SOURCE_TABLE}.scope_key
                  AND scoped_sessions.id = {SCOPED_SESSION_SCAN_SOURCE_TABLE}.session_id
                  AND scoped_sessions.agent = {SCOPED_SESSION_SCAN_SOURCE_TABLE}.agent
                  AND scoped_sessions.path = {SCOPED_SESSION_SCAN_SOURCE_TABLE}.session_path
             )"
        ),
        [scope_key.as_str()],
    )?;
    Ok(())
}


fn cleanup_stale_scoped_session_search_rows(
    conn: &rusqlite::Transaction<'_>,
    scope_key: &ScopeKey,
) -> Result<()> {
    conn.execute(
        "DELETE FROM scoped_session_search_records
         WHERE scope_key = ?1
           AND NOT EXISTS (
            SELECT 1 FROM scoped_sessions
            WHERE scoped_sessions.scope_key = scoped_session_search_records.scope_key
              AND scoped_sessions.id = scoped_session_search_records.session_id
              AND scoped_sessions.agent = scoped_session_search_records.agent
              AND scoped_sessions.path = scoped_session_search_records.session_path
         )",
        [scope_key.as_str()],
    )?;
    conn.execute(
        "DELETE FROM scoped_session_search_index
         WHERE scope_key = ?1
           AND NOT EXISTS (
            SELECT 1 FROM scoped_sessions
            WHERE scoped_sessions.scope_key = scoped_session_search_index.scope_key
              AND scoped_sessions.id = scoped_session_search_index.session_id
              AND scoped_sessions.agent = scoped_session_search_index.agent
              AND scoped_sessions.path = scoped_session_search_index.session_path
         )",
        [scope_key.as_str()],
    )?;
    Ok(())
}




fn index_scoped_session_search_document_best_effort(
    tx: &Transaction<'_>,
    scope_key: &ScopeKey,
    session: &SessionRecord,
) {
    let _ = index_scoped_session_search_document(tx, scope_key, session);
}

fn index_scoped_session_search_document(
    tx: &Transaction<'_>,
    scope_key: &ScopeKey,
    session: &SessionRecord,
) -> Result<()> {
    let agent = agent_label(session.agent);
    let session_path = session.path.display().to_string();
    let search_metadata = session_search_metadata(session);
    let state = match crate::session_skills::session_file_state(&session.path) {
        Ok(state) => state,
        Err(_) => {
            tx.execute(
                "DELETE FROM scoped_session_search_records
                 WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
                params![scope_key.as_str(), session.id, agent, session_path],
            )?;
            insert_scoped_session_search_record(
                tx,
                scope_key,
                session,
                0,
                &session_search_metadata_document(session),
            )?;
            tx.execute(
                "INSERT INTO scoped_session_search_index (
                    scope_key, session_id, agent, session_path, file_mtime, file_size,
                    indexed_at, search_metadata, search_index_version
                 ) VALUES (?1, ?2, ?3, ?4, 0, 0, ?5, ?6, ?7)
                 ON CONFLICT(scope_key, session_id, agent, session_path) DO UPDATE SET
                    indexed_at = excluded.indexed_at,
                    search_metadata = excluded.search_metadata,
                    search_index_version = excluded.search_index_version",
                params![
                    scope_key.as_str(),
                    session.id,
                    agent,
                    session_path,
                    unix_now().to_string(),
                    search_metadata,
                    SESSION_SEARCH_INDEX_VERSION,
                ],
            )?;
            return Ok(());
        }
    };
    let current = tx
        .query_row(
            "SELECT file_mtime, file_size, search_metadata, search_index_version,
                    EXISTS (
                        SELECT 1
                        FROM scoped_session_search_records AS records
                        WHERE records.scope_key = ?1
                          AND records.session_id = ?2
                          AND records.agent = ?3
                          AND records.session_path = ?4
                          AND records.record_order = 0
                    )
             FROM scoped_session_search_index
             WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
            params![scope_key.as_str(), session.id, agent, session_path],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            },
        )
        .optional()?;
    if current.as_ref().is_some_and(|current| {
        current.0 == state.file_mtime
            && current.1 == state.file_size
            && current.2 == search_metadata
            && current.3 == SESSION_SEARCH_INDEX_VERSION
            && current.4
    }) {
        return Ok(());
    }
    if current.as_ref().is_some_and(|current| {
        current.0 == state.file_mtime
            && current.1 == state.file_size
            && current.3 == SESSION_SEARCH_INDEX_VERSION
            && current.4
    }) {
        let document = session_search_metadata_document(session);
        tx.execute(
            "UPDATE scoped_session_search_records
             SET metadata_text = ?5, title = ?6, project = ?7
             WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3
               AND session_path = ?4 AND record_order = 0",
            params![
                scope_key.as_str(),
                session.id,
                agent,
                session_path,
                document.metadata_text,
                document.title,
                document.project,
            ],
        )?;
        tx.execute(
            "UPDATE scoped_session_search_index
             SET indexed_at = ?5, search_metadata = ?6
             WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
            params![
                scope_key.as_str(),
                session.id,
                agent,
                session_path,
                unix_now().to_string(),
                search_metadata,
            ],
        )?;
        return Ok(());
    }

    tx.execute(
        "DELETE FROM scoped_session_search_records
         WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
        params![scope_key.as_str(), session.id, agent, session_path],
    )?;
    insert_scoped_session_search_record(
        tx,
        scope_key,
        session,
        0,
        &session_search_metadata_document(session),
    )?;
    let mut record_order = 1usize;
    let mut insert_error = None;
    let parse_result = transcript::for_each_search_item(&session.path, session.agent, |item| {
        if insert_error.is_some() {
            return;
        }
        let mut document = SessionSearchDocument::default();
        match item.kind.as_str() {
            "user" => document.user_text = item.body,
            "assistant" => document.assistant_text = item.body,
            _ => return,
        }
        if let Err(error) = insert_scoped_session_search_record(
            tx,
            scope_key,
            session,
            record_order,
            &document,
        ) {
            insert_error = Some(error);
        } else {
            record_order += 1;
        }
    });
    if let Some(error) = insert_error {
        return Err(error);
    }
    if parse_result.is_err() {
        tx.execute(
            "DELETE FROM scoped_session_search_records
             WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3
               AND session_path = ?4 AND record_order > 0",
            params![scope_key.as_str(), session.id, agent, session_path],
        )?;
    }
    tx.execute(
        "INSERT INTO scoped_session_search_index (
            scope_key, session_id, agent, session_path, file_mtime, file_size,
            indexed_at, search_metadata, search_index_version
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(scope_key, session_id, agent, session_path) DO UPDATE SET
            file_mtime = excluded.file_mtime,
            file_size = excluded.file_size,
            indexed_at = excluded.indexed_at,
            search_metadata = excluded.search_metadata,
            search_index_version = excluded.search_index_version",
        params![
            scope_key.as_str(),
            session.id,
            agent,
            session_path,
            state.file_mtime,
            state.file_size,
            unix_now().to_string(),
            search_metadata,
            SESSION_SEARCH_INDEX_VERSION,
        ],
    )?;
    Ok(())
}

fn insert_scoped_session_search_record(
    tx: &Transaction<'_>,
    scope_key: &ScopeKey,
    session: &SessionRecord,
    record_order: usize,
    document: &SessionSearchDocument,
) -> Result<()> {
    tx.execute(
        "INSERT INTO scoped_session_search_records (
            scope_key, session_id, agent, session_path, record_order,
            metadata_text, title, project, user_text, assistant_text
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            scope_key.as_str(),
            session.id,
            agent_label(session.agent),
            session.path.display().to_string(),
            record_order as i64,
            document.metadata_text,
            document.title,
            document.project,
            document.user_text,
            document.assistant_text,
        ],
    )?;
    Ok(())
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

    let mut start = match_start.saturating_sub(80);
    while start > 0 && !value.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (match_end + 80).min(value.len());
    while end < value.len() && !value.is_char_boundary(end) {
        end += 1;
    }
    Some(format!(
        "{}{}⟦{}⟧{}{}",
        if start > 0 { "… " } else { "" },
        &value[start..match_start],
        &value[match_start..match_end],
        &value[match_end..end],
        if end < value.len() { " …" } else { "" },
    ))
}

fn unix_now() -> u64 {
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
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        sync::mpsc,
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use crate::{
        AgentScan, HookScan, McpScan, RuleRecord, RuleScan, ScanReport, SessionRecord, SessionScan,
        SkillRecord, SkillScan, SkillVisibility,
        analytics::{
            AnalyticsParserState, AnalyticsResponseUsage, AnalyticsTokenUsage, SessionAnalytics,
            SessionAnalyticsRecord,
        },
        session_skills::{SessionFileState, SessionSkillLink},
        sessions::SessionIdentity,
        runtime_contract::{OperationId, OperationKind, OperationRecord, OperationStatus, Revision, ScopeKey, SourceVersion},
        skills::{
            AgentKind, SkillPath, SkillRoot, SkillSnapshot, SkillSnapshotFile, SkillSourceRecord,
        },
    };
    use chrono::Local;
    use rusqlite::{Connection, params};

    use super::{
        AppSettings, PromptWrite, SESSION_SEARCH_CANDIDATE_TABLE, Store, normalize_repository_url,
    };

    #[test]
    fn repository_urls_normalize_https_and_ssh_forms() {
        assert_eq!(
            normalize_repository_url("https://github.com/tutti-os/tutti.git/"),
            Some("github.com/tutti-os/tutti".to_string())
        );
        assert_eq!(
            normalize_repository_url("git@github.com:tutti-os/tutti.git"),
            Some("github.com/tutti-os/tutti".to_string())
        );
        assert_eq!(
            normalize_repository_url("ssh://git@github.com/tutti-os/tutti.git"),
            Some("github.com/tutti-os/tutti".to_string())
        );
    }

    #[test]
    fn session_scan_cache_uses_persisted_source_state() {
        let temp = temp_dir("tendi-session-scan-source-state");
        fs::create_dir_all(&temp).unwrap();
        let transcript = temp.join("session.jsonl");
        fs::write(&transcript, "initial\n").unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let scope = ScopeKey::new("workspace:/repo").unwrap();
        let session = SessionRecord {
            id: "session-id".to_string(),
            agent: AgentKind::Codex,
            title: Some("Session".to_string()),
            project: None,
            repository: None,
            repository_url: None,
            logical_project_id: None,
            logical_project_name: None,
            path: transcript.clone(),
            started_at: None,
            updated_at: None,
            message_count: Some(1),
            first_user_message: Some("initial".to_string()),
            last_user_message: Some("initial".to_string()),
            last_assistant_message: Some("answer".to_string()),
            turn_count: Some(1),
            model: None,
            mode: None,
            approval_mode: None,
            is_run_everything: None,
            parent_session_id: None,
            token_usage: None,
        };

        store
            .apply_session_delta_for_scope(&scope, std::slice::from_ref(&session))
            .unwrap();
        let cache = store.session_scan_cache_for_scope(&scope).unwrap();
        assert!(cache.session_if_current(AgentKind::Codex, &transcript).is_some());

        fs::write(&transcript, "changed source\n").unwrap();
        let cache = store.session_scan_cache_for_scope(&scope).unwrap();
        assert!(cache.session_if_current(AgentKind::Codex, &transcript).is_none());

        store
            .conn
            .execute(
                "UPDATE scoped_session_scan_sources SET parser_version = 'legacy'",
                [],
            )
            .unwrap();
        fs::write(&transcript, "initial\n").unwrap();
        let cache = store.session_scan_cache_for_scope(&scope).unwrap();
        assert!(cache.session_if_current(AgentKind::Codex, &transcript).is_none());

        store
            .apply_session_delta_for_scope(&scope, std::slice::from_ref(&session))
            .unwrap();
        let cache = store.session_scan_cache_for_scope(&scope).unwrap();
        assert!(cache.session_if_current(AgentKind::Codex, &transcript).is_some());

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn project_scan_scope_excludes_bang_prefixed_paths() {
        let temp = temp_dir("tendi-project-scope-exclude");
        let scope = temp.join("dev");
        let included = scope.join("included");
        let excluded = scope.join("nested").join("excluded");
        fs::create_dir_all(included.join(".git")).unwrap();
        fs::create_dir_all(excluded.join(".git")).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

        store
            .save_project_scan_scopes(vec![
                scope.to_string_lossy().into_owned(),
                format!("!{}/**/excluded", scope.display()),
            ])
            .unwrap();
        let result = store.scan_projects().unwrap();

        assert!(result.scopes.iter().any(|scope| scope.excluded));
        assert!(
            result
                .projects
                .iter()
                .any(|project| project.root_path == included.canonicalize().unwrap())
        );
        assert!(
            !result
                .projects
                .iter()
                .any(|project| { project.root_path == excluded.canonicalize().unwrap() })
        );

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn scoped_session_search_rebuild_repairs_existing_projection_rows() {
        let temp = temp_dir("tendi-storage-scoped-search-backfill");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let scope = ScopeKey::new("workspace:backfill").unwrap();
        let path = temp.join("session.jsonl");
        fs::write(
            &path,
            include_str!("../testdata/transcripts/codex.jsonl")
                .replace("Inspect the fixture parser", "backfill-private-marker"),
        )
        .unwrap();
        let mut record = session("backfill", "Backfill session");
        record.path = path;
        let candidate = SessionIdentity::from(&record);
        let candidates = std::iter::once(candidate.clone())
            .chain((0..1_199).map(|index| SessionIdentity {
                id: format!("unrelated-{index}"),
                agent: AgentKind::Codex,
                path: PathBuf::from(format!("/tmp/unrelated-{index}.jsonl")),
            }))
            .collect::<Vec<_>>();
        store
            .save_sessions_at_for_scope(
                &scope,
                &SessionScan {
                    sessions: vec![record],
                    warnings: Vec::new(),
                },
                1,
            )
            .unwrap();
        assert!(!store
            .ensure_scoped_session_search_for_scope(&scope)
            .unwrap());
        store
            .conn
            .execute(
                "DELETE FROM scoped_session_search_records WHERE scope_key = ?1",
                params![scope.as_str()],
            )
            .unwrap();

        assert!(
            store
                .search_sessions_for_scope(&scope, "backfill-private-marker", None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.ensure_scoped_session_search_for_scope(&scope).unwrap(),
            true
        );
        assert_eq!(
            store
                .search_sessions_for_scope(
                    &scope,
                    "backfill-private-marker",
                    Some(&candidates),
                )
                .unwrap()
                .len(),
            1
        );

        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn session_search_candidate_table_handles_large_and_empty_sets() {
        let candidates = (0..1_200)
            .map(|index| SessionIdentity {
                id: format!("session-{index}"),
                agent: AgentKind::Codex,
                path: PathBuf::from(format!("/tmp/session-{index}.jsonl")),
            })
            .collect::<Vec<_>>();
        let temp = temp_dir("tendi-storage-session-search-candidates");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

        let join = store
            .session_search_candidate_join(Some(&candidates))
            .unwrap();
        assert!(join.contains(SESSION_SEARCH_CANDIDATE_TABLE));
        let matched: i64 = store
            .conn
            .query_row(
                &format!("SELECT COUNT(*) FROM temp.{SESSION_SEARCH_CANDIDATE_TABLE}"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(matched, 1_200);

        store.session_search_candidate_join(Some(&[])).unwrap();
        let matched_empty: i64 = store
            .conn
            .query_row(
                &format!("SELECT COUNT(*) FROM temp.{SESSION_SEARCH_CANDIDATE_TABLE}"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(matched_empty, 0);

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn session_skill_reads_retry_transient_database_lock() {
        let temp = temp_dir("tendi-session-skill-read-lock");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let store = Store::open(&db).unwrap();
        store
            .conn
            .execute_batch("PRAGMA journal_mode = DELETE;")
            .unwrap();
        store.conn.busy_timeout(Duration::ZERO).unwrap();

        let scope = ScopeKey::new("workspace:session-skill-read-lock").unwrap();
        let status = read_after_transient_exclusive_lock(&db, || {
            store.session_skill_index_status_for_scope(&scope, false)
        });
        assert_eq!(status.total, 0);

        let settings = read_after_transient_exclusive_lock(&db, || store.app_settings());
        assert_eq!(settings.appearance, "system");
        let _ = read_after_transient_exclusive_lock(&db, || store.project_scan_scopes());
        let _ = read_after_transient_exclusive_lock(&db, || store.list_projects());
        let _ = read_after_transient_exclusive_lock(&db, || store.list_sessions_for_scope(&scope));
        let _ = read_after_transient_exclusive_lock(&db, || {
            store.list_session_projects_for_scope(&scope)
        });
        let _ = read_after_transient_exclusive_lock(&db, || store.analytics_revision());
        let _ = read_after_transient_exclusive_lock(&db, || {
            store.search_sessions_for_scope(&scope, "session", None)
        });
        let _ = read_after_transient_exclusive_lock(&db, || store.list_prompts());

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    fn read_after_transient_exclusive_lock<T>(
        db: &Path,
        read: impl FnOnce() -> anyhow::Result<T>,
    ) -> T {
        let blocker = Connection::open(db).unwrap();
        blocker.busy_timeout(Duration::ZERO).unwrap();
        blocker
            .execute_batch("PRAGMA journal_mode = DELETE; BEGIN EXCLUSIVE;")
            .unwrap();
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(125));
            blocker.execute_batch("ROLLBACK;").unwrap();
        });

        let value = read().unwrap();
        release.join().unwrap();
        value
    }


    #[test]
    fn app_settings_store_normalized_additional_session_roots() {
        let temp = temp_dir("tendi-storage-additional-session-settings");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let store = Store::open(&db).unwrap();
        assert_eq!(store.app_settings().unwrap().appearance, "system");
        assert_eq!(store.app_settings().unwrap().font_family, "manrope");
        assert_eq!(store.app_settings().unwrap().light_theme, "vercel");
        assert_eq!(store.app_settings().unwrap().dark_theme, "vercel");
        assert_eq!(store.app_settings().unwrap().app_icon, "sakura-pop");
        assert_eq!(store.app_settings().unwrap().session_resume_target, "auto");
        assert_eq!(
            store.app_settings().unwrap().missing_session_project_policy,
            "show"
        );
        assert_eq!(store.app_settings().unwrap().editor, "vscode");
        assert!(!store.app_settings().unwrap().developer_mode);

        let saved = store
            .save_app_settings(AppSettings {
                appearance: "dark".to_string(),
                font_family: "geist".to_string(),
                light_theme: "nord".to_string(),
                dark_theme: "tokyo-night".to_string(),
                app_icon: "dracula".to_string(),
                terminal: "Warp".to_string(),
                session_resume_target: "app".to_string(),
                missing_session_project_policy: "hide".to_string(),
                editor: "zed".to_string(),
                developer_mode: true,
                additional_session_roots: vec![
                    "/tmp/tendi-additional-sessions".to_string(),
                    "\n/tmp/tendi-additional-sessions\n".to_string(),
                ],
                config_profiles: BTreeMap::from([
                    ("codex".to_string(), "deep-review".to_string()),
                    ("claude".to_string(), "safe-mode".to_string()),
                    ("cursor".to_string(), "safe-mode".to_string()),
                ]),
            })
            .unwrap();

        assert_eq!(saved.appearance, "dark");
        assert_eq!(saved.font_family, "geist");
        assert_eq!(saved.light_theme, "nord");
        assert_eq!(saved.dark_theme, "tokyo-night");
        assert_eq!(saved.app_icon, "dracula");
        assert_eq!(saved.terminal, "Warp");
        assert_eq!(saved.session_resume_target, "app");
        assert_eq!(saved.missing_session_project_policy, "hide");
        assert_eq!(saved.editor, "zed");
        assert!(saved.developer_mode);
        assert_eq!(saved.config_profiles["codex"], "deep-review");
        assert_eq!(saved.config_profiles["claude"], "safe-mode");
        assert_eq!(saved.config_profiles["cursor"], "safe-mode");
        assert_eq!(
            saved.additional_session_roots,
            vec!["/tmp/tendi-additional-sessions"]
        );
        assert_eq!(
            store.app_settings().unwrap().additional_session_roots,
            saved.additional_session_roots
        );
        assert!(store.app_settings().unwrap().developer_mode);
        assert_eq!(
            store.app_settings().unwrap().config_profiles,
            saved.config_profiles
        );

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }



    #[test]
    fn workspace_scan_warning_keeps_the_last_good_canonical_snapshot() {
        let temp = temp_dir("tendi-storage-scan-warning");
        fs::create_dir_all(&temp).unwrap();
        let workspace = temp.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let clean = report_with_skill("old", SkillVisibility::Auto);
        store.save_scan_for_workspace(&workspace, &clean).unwrap();

        let mut failed = report_with_skill("new", SkillVisibility::Manual);
        failed.skills.warnings.push("partial provider scan".to_string());
        store.save_scan_for_workspace(&workspace, &failed).unwrap();

        let cached = store
            .list_skills_cached_for_workspace(&workspace)
            .unwrap()
            .unwrap();
        assert_eq!(cached.skills[0].name, "old");

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }




    #[test]
    fn skill_snapshots_round_trip_and_are_removed_with_source_records() {
        let temp = temp_dir("tendi-storage-skill-snapshot");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let skill_path = temp.join("skills/demo");
        store
            .replace_skill_snapshots(&[SkillSnapshot {
                skill_path: skill_path.clone(),
                source_version: "base".to_string(),
                files: vec![SkillSnapshotFile {
                    relative_path: "SKILL.md".to_string(),
                    content: b"base".to_vec(),
                }],
            }])
            .unwrap();

        let snapshot = store.skill_snapshot(&skill_path).unwrap().unwrap();
        assert_eq!(snapshot.source_version, "base");
        assert_eq!(snapshot.files[0].content, b"base");
        store
            .delete_skill_source_records(std::slice::from_ref(&skill_path))
            .unwrap();
        assert!(store.skill_snapshot(&skill_path).unwrap().is_none());

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn opening_existing_database_adds_skill_source_ref_column() {
        let temp = temp_dir("tendi-storage-skill-source-ref-migration");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE skill_sources (
                    skill_path TEXT PRIMARY KEY,
                    skill_name TEXT NOT NULL,
                    source_kind TEXT NOT NULL,
                    source TEXT,
                    source_version TEXT,
                    source_relative_path TEXT,
                    update_status TEXT NOT NULL,
                    origin TEXT NOT NULL,
                    data_json TEXT NOT NULL
                );",
            )
            .unwrap();
        drop(connection);

        let store = Store::open(&db).unwrap();
        let columns = store
            .conn
            .prepare("PRAGMA table_info(skill_sources)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "source_ref"));

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn opening_existing_database_adds_session_search_index_version_column() {
        let temp = temp_dir("tendi-storage-session-search-version-migration");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session_search_index (
                    session_id TEXT NOT NULL,
                    agent TEXT NOT NULL,
                    session_path TEXT NOT NULL,
                    file_mtime INTEGER NOT NULL,
                    file_size INTEGER NOT NULL,
                    indexed_at TEXT NOT NULL,
                    search_metadata TEXT NOT NULL DEFAULT '',
                    PRIMARY KEY (session_id, agent, session_path)
                );",
            )
            .unwrap();
        drop(connection);

        let store = Store::open(&db).unwrap();
        let columns = store
            .conn
            .prepare("PRAGMA table_info(session_search_index)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "search_index_version"));

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }


    #[test]
    fn existing_duplicate_skill_paths_are_deduplicated_before_unique_index() {
        let temp = temp_dir("tendi-storage-skill-path-migration");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let connection = Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE skill_paths (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    skill_name TEXT NOT NULL,
                    path TEXT NOT NULL,
                    root TEXT NOT NULL,
                    scope TEXT NOT NULL,
                    agent TEXT NOT NULL,
                    sha256 TEXT NOT NULL,
                    data_json TEXT NOT NULL
                );
                INSERT INTO skill_paths
                    (id, skill_name, path, root, scope, agent, sha256, data_json)
                VALUES
                    (1, 'demo', '/tmp/demo', '/tmp', 'global', 'shared', 'old', '{}'),
                    (2, 'demo', '/tmp/demo', '/tmp', 'global', 'shared', 'new', '{\"latest\":true}');",
            )
            .unwrap();
        drop(connection);

        let store = Store::open(&db).unwrap();
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM skill_paths WHERE skill_name = 'demo' AND path = '/tmp/demo'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT sha256 FROM skill_paths WHERE skill_name = 'demo' AND path = '/tmp/demo'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "new"
        );
        drop(store);

        let reopened = Store::open(&db).unwrap();
        assert_eq!(
            reopened
                .conn
                .query_row("SELECT COUNT(*) FROM skill_paths", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
        assert!(
            reopened
                .conn
                .execute(
                    "INSERT INTO skill_paths
                    (skill_name, path, root, scope, agent, sha256, data_json)
                 VALUES ('demo', '/tmp/demo', '/tmp', 'global', 'shared', 'again', '{}')",
                    [],
                )
                .is_err()
        );

        drop(reopened);
        fs::remove_dir_all(temp).unwrap();
    }


    #[test]
    fn projection_manifest_invalidates_on_new_edit_and_delete() {
        let temp = temp_dir("tendi-projection-freshness");
        let workspace = temp.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let rule_path = workspace.join("AGENTS.md");
        fs::write(&rule_path, "old").unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

        store
            .save_rules_for_workspace(
                &workspace,
                &RuleScan {
                    rules: vec![RuleRecord {
                        agents: vec![AgentKind::Shared],
                        kind: "agents".to_string(),
                        scope: "project".to_string(),
                        path: rule_path.clone(),
                        order: 0,
                        sha256: "old".to_string(),
                    }],
                    warnings: Vec::new(),
                },
            )
            .unwrap();
        assert!(
            store
                .list_rules_for_workspace(&workspace)
                .unwrap()
                .is_some()
        );

        fs::write(&rule_path, "edited with a different size").unwrap();
        assert!(
            store
                .list_rules_for_workspace(&workspace)
                .unwrap()
                .is_none()
        );

        fs::remove_file(&rule_path).unwrap();
        assert!(
            store
                .list_rules_for_workspace(&workspace)
                .unwrap()
                .is_none()
        );

        store
            .save_rules_for_workspace(
                &workspace,
                &RuleScan {
                    rules: Vec::new(),
                    warnings: Vec::new(),
                },
            )
            .unwrap();
        assert!(
            store
                .list_rules_for_workspace(&workspace)
                .unwrap()
                .is_some()
        );
        fs::write(&rule_path, "new file").unwrap();
        assert!(
            store
                .list_rules_for_workspace(&workspace)
                .unwrap()
                .is_none()
        );

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn skill_projection_invalidates_when_skill_file_changes() {
        let temp = temp_dir("tendi-skill-projection-freshness");
        let workspace = temp.join("workspace");
        let skill_dir = workspace.join(".agents/skills/demo");
        let skill_file = skill_dir.join("SKILL.md");
        let skills_lock = workspace.join("skills-lock.json");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(&skill_file, "old").unwrap();
        fs::write(&skills_lock, "old lock").unwrap();

        let mut scan = report_with_skill("demo", SkillVisibility::Auto).skills;
        scan.roots[0].path = workspace.join(".agents/skills");
        scan.skills[0].paths[0].path = skill_dir;
        scan.skills[0].paths[0].root = workspace.join(".agents/skills");
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

        store.save_skills_for_workspace(&workspace, &scan).unwrap();
        assert!(
            store
                .list_skills_for_workspace(&workspace)
                .unwrap()
                .is_some()
        );
        let manifest = store
            .list_fs_manifest_for_root(&workspace.canonicalize().unwrap())
            .unwrap();
        assert!(
            manifest
                .iter()
                .any(|entry| {
                    entry.source_kind == "skill-candidate"
                        && entry.path == skills_lock.canonicalize().unwrap()
                })
        );

        store
            .conn
            .execute(
                "UPDATE fs_manifest SET parser_version = 'scan-v3' WHERE source_kind = 'skill'",
                [],
            )
            .unwrap();
        assert!(
            store
                .list_skills_for_workspace(&workspace)
                .unwrap()
                .is_none()
        );
        store.save_skills_for_workspace(&workspace, &scan).unwrap();

        fs::write(&skill_file, "edited skill content").unwrap();
        assert!(
            store
                .list_skills_for_workspace(&workspace)
                .unwrap()
                .is_none()
        );
        store.save_skills_for_workspace(&workspace, &scan).unwrap();

        fs::write(&skills_lock, "edited lock content").unwrap();
        assert!(
            store
                .list_skills_for_workspace(&workspace)
                .unwrap()
                .is_none()
        );

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn cached_skill_projection_reads_stale_rows_without_rescanning() {
        let temp = temp_dir("tendi-skill-projection-cached");
        let workspace = temp.join("workspace");
        let skill_dir = workspace.join(".agents/skills/demo");
        let skill_file = skill_dir.join("SKILL.md");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(&skill_file, "old").unwrap();

        let mut scan = report_with_skill("demo", SkillVisibility::Auto).skills;
        scan.roots[0].path = workspace.join(".agents/skills");
        scan.skills[0].paths[0].path = skill_dir;
        scan.skills[0].paths[0].root = workspace.join(".agents/skills");
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        store.save_skills_for_workspace(&workspace, &scan).unwrap();
        fs::write(&skill_file, "edited skill content").unwrap();

        let cached = store
            .list_skills_cached_for_workspace(&workspace)
            .unwrap()
            .unwrap();
        assert_eq!(cached.skills.len(), 1);
        assert_eq!(cached.skills[0].name, "demo");
        assert!(
            store
                .list_skills_for_workspace(&workspace)
                .unwrap()
                .is_none()
        );

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn projection_context_does_not_mix_workspaces_and_retries_empty_domain() {
        let temp = temp_dir("tendi-projection-context");
        let first = temp.join("first");
        let second = temp.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let first_path = first.join("AGENTS.md");
        let second_path = second.join("AGENTS.md");
        fs::write(&first_path, "first").unwrap();
        fs::write(&second_path, "second").unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

        let scan = |path: PathBuf, sha256: &str| RuleScan {
            rules: vec![RuleRecord {
                agents: vec![AgentKind::Shared],
                kind: "agents".to_string(),
                scope: "project".to_string(),
                path,
                order: 0,
                sha256: sha256.to_string(),
            }],
            warnings: Vec::new(),
        };
        store
            .save_rules_for_workspace(&first, &scan(first_path, "first"))
            .unwrap();
        assert_eq!(
            store
                .list_rules_for_workspace(&first)
                .unwrap()
                .unwrap()
                .rules[0]
                .sha256,
            "first"
        );
        assert!(store.list_rules_for_workspace(&second).unwrap().is_none());

        store
            .save_rules_for_workspace(&second, &scan(second_path, "second"))
            .unwrap();
        assert_eq!(
            store
                .list_rules_for_workspace(&second)
                .unwrap()
                .unwrap()
                .rules[0]
                .sha256,
            "second"
        );
        assert_eq!(
            store
                .list_rules_for_workspace(&first)
                .unwrap()
                .unwrap()
                .rules[0]
                .sha256,
            "first"
        );

        store
            .save_rules_for_workspace(
                &first,
                &RuleScan {
                    rules: Vec::new(),
                    warnings: vec!["temporary scan failure".to_string()],
                },
            )
            .unwrap();
        assert!(store.list_rules_for_workspace(&first).unwrap().is_none());
        store
            .save_rules_for_workspace(
                &first,
                &RuleScan {
                    rules: Vec::new(),
                    warnings: Vec::new(),
                },
            )
            .unwrap();
        assert_eq!(
            store
                .list_rules_for_workspace(&first)
                .unwrap()
                .unwrap()
                .rules
                .len(),
            0
        );

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn projection_refresh_lock_serializes_nested_refreshes() {
        let temp = temp_dir("tendi-projection-lock");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let nested = store
            .with_projection_refresh_lock("rules", || {
                assert!(
                    store
                        .with_projection_refresh_lock("rules", || Ok::<_, anyhow::Error>(()))
                        .unwrap()
                        .is_none()
                );
                Ok::<_, anyhow::Error>(42)
            })
            .unwrap();
        assert_eq!(nested, Some(42));
        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }


    #[test]
    fn projection_refresh_lock_serializes_same_domain_across_connections() {
        let temp = temp_dir("tendi-projection-lock-cross-connection");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let store_a = Store::open(&db).unwrap();
        let store_b = Store::open(&db).unwrap();
        let sql_lock_table_count = store_a
            .conn
            .query_row(
                "SELECT COUNT(*)
                 FROM sqlite_master
                 WHERE type = 'table'
                   AND name IN ('projection_refresh_lock', 'projection_refresh_locks')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(sql_lock_table_count, 0);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let holder = thread::spawn(move || {
            store_a
                .with_projection_refresh_lock("rules", || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok::<_, anyhow::Error>(())
                })
                .unwrap()
        });
        started_rx.recv().unwrap();

        let contender = store_b
            .with_projection_refresh_lock("rules", || Ok::<_, anyhow::Error>(42))
            .unwrap();
        assert_eq!(contender, None);

        release_tx.send(()).unwrap();
        assert_eq!(holder.join().unwrap(), Some(()));
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn projection_refresh_lock_allows_different_domains_across_connections() {
        let temp = temp_dir("tendi-projection-lock-cross-domain");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let store_a = Store::open(&db).unwrap();
        let store_b = Store::open(&db).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let holder = thread::spawn(move || {
            store_a
                .with_projection_refresh_lock("rules", || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok::<_, anyhow::Error>(())
                })
                .unwrap()
        });
        started_rx.recv().unwrap();

        let contender = store_b
            .with_projection_refresh_lock("hooks", || Ok::<_, anyhow::Error>(42))
            .unwrap();
        assert_eq!(contender, Some(42));

        release_tx.send(()).unwrap();
        assert_eq!(holder.join().unwrap(), Some(()));
        fs::remove_dir_all(temp).unwrap();
    }





    fn test_skill_source(name: &str, path: &Path) -> SkillSourceRecord {
        SkillSourceRecord {
            skill_name: name.to_string(),
            skill_path: path.to_path_buf(),
            source_kind: "github".to_string(),
            source: Some(format!("https://github.com/example/{name}.git")),
            source_ref: Some("main".to_string()),
            source_version: Some("abc123".to_string()),
            source_relative_path: Some(format!("skills/{name}")),
            update_status: "tracked".to_string(),
            origin: "tendi-install".to_string(),
        }
    }

    #[test]
    fn skill_source_records_are_scoped_by_workspace() {
        let temp = temp_dir("tendi-storage-skill-source-scope");
        let workspace_a = temp.join("workspace-a");
        let workspace_b = temp.join("workspace-b");
        fs::create_dir_all(&workspace_a).unwrap();
        fs::create_dir_all(&workspace_b).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let source_a = test_skill_source("alpha", &workspace_a.join("skills/alpha"));
        let source_b = test_skill_source("beta", &workspace_b.join("skills/beta"));

        store
            .upsert_skill_source_records_for_workspace(&workspace_a, std::slice::from_ref(&source_a))
            .unwrap();
        store
            .upsert_skill_source_records_for_workspace(&workspace_b, std::slice::from_ref(&source_b))
            .unwrap();

        assert_eq!(
            store
                .skill_source_records_for_workspace(&workspace_a)
                .unwrap()
                .iter()
                .map(|record| record.skill_name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha"]
        );
        assert_eq!(
            store
                .skill_source_records_for_workspace(&workspace_b)
                .unwrap()
                .iter()
                .map(|record| record.skill_name.as_str())
                .collect::<Vec<_>>(),
            vec!["beta"]
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn scoped_skill_snapshots_round_trip_without_cross_workspace_merge() {
        let temp = temp_dir("tendi-storage-scoped-skill-snapshot");
        let workspace_a = temp.join("workspace-a");
        let workspace_b = temp.join("workspace-b");
        fs::create_dir_all(&workspace_a).unwrap();
        fs::create_dir_all(&workspace_b).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let path = temp.join("shared-path/skill");
        let snapshot_a = SkillSnapshot {
            skill_path: path.clone(),
            source_version: "a".to_string(),
            files: vec![SkillSnapshotFile {
                relative_path: "SKILL.md".to_string(),
                content: b"workspace-a".to_vec(),
            }],
        };
        let snapshot_b = SkillSnapshot {
            skill_path: path.clone(),
            source_version: "b".to_string(),
            files: vec![SkillSnapshotFile {
                relative_path: "SKILL.md".to_string(),
                content: b"workspace-b".to_vec(),
            }],
        };
        store
            .replace_skill_snapshots_for_workspace(&workspace_a, std::slice::from_ref(&snapshot_a))
            .unwrap();
        store
            .replace_skill_snapshots_for_workspace(&workspace_b, std::slice::from_ref(&snapshot_b))
            .unwrap();

        let snapshot = store.skill_snapshot(&path).unwrap().unwrap();
        assert_eq!(snapshot.source_version, "a");
        assert_eq!(snapshot.files[0].content, b"workspace-a");

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn checked_skill_persistence_rejects_a_stale_source_version() {
        let temp = temp_dir("tendi-storage-stale-skill-source");
        let workspace = temp.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let path = workspace.join("skills/demo");
        let mut current = test_skill_source("demo", &path);
        current.source_version = Some("version-a".to_string());
        store
            .upsert_skill_source_records_for_workspace(&workspace, std::slice::from_ref(&current))
            .unwrap();

        let mut next = current.clone();
        next.source_version = Some("version-b".to_string());
        store
            .persist_skill_update_persistence_for_workspace_checked(
                &workspace,
                &[(path.clone(), Some("version-a".to_string()))],
                std::slice::from_ref(&next),
                &[],
            )
            .unwrap();

        let mut stale = next.clone();
        stale.source_version = Some("version-c".to_string());
        let error = store
            .persist_skill_update_persistence_for_workspace_checked(
                &workspace,
                &[(path, Some("version-a".to_string()))],
                std::slice::from_ref(&stale),
                &[],
            )
            .unwrap_err();
        assert!(error.to_string().contains("changed after the update preview"));

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn stale_skill_preflight_leaves_filesystem_untouched() {
        let temp = temp_dir("tendi-storage-stale-skill-preflight");
        let workspace = temp.join("workspace");
        let skill_path = workspace.join("skills/demo/SKILL.md");
        fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
        fs::write(&skill_path, "before\n").unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let source_path = skill_path.parent().unwrap().to_path_buf();
        let mut current = test_skill_source("demo", &source_path);
        current.source_version = Some("version-b".to_string());
        store
            .upsert_skill_source_records_for_workspace(&workspace, std::slice::from_ref(&current))
            .unwrap();

        let error = store
            .validate_skill_source_versions_for_workspace(
                &workspace,
                &[(source_path, Some("version-a".to_string()))],
            )
            .unwrap_err();
        assert!(error.to_string().contains("changed after the update preview"));
        assert_eq!(fs::read_to_string(&skill_path).unwrap(), "before\n");

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }


    fn report_with_skill(name: &str, visibility: SkillVisibility) -> ScanReport {
        let root = PathBuf::from("/tmp/tendi-test/.agents/skills");
        let path = root.join(name);
        ScanReport {
            agents: AgentScan {
                agents: Vec::new(),
                warnings: Vec::new(),
            },
            skill_source_migrations: Vec::new(),
            skills: SkillScan {
                roots: vec![SkillRoot {
                    path: root.clone(),
                    scope: "project".to_string(),
                    agent: AgentKind::Shared,
                    plugin_id: None,
                    plugin_enabled: None,
                }],
                skills: vec![SkillRecord {
                    id: name.to_string(),
                    installation_id: name.to_string(),
                    name: name.to_string(),
                    description: Some("demo".to_string()),
                    tags: Vec::new(),
                    dependencies: Vec::new(),
                    dependents: Vec::new(),
                    dependency_ids: Vec::new(),
                    dependent_ids: Vec::new(),
                    visibility,
                    agents: vec![AgentKind::Shared],
                    paths: vec![SkillPath {
                        path,
                        root,
                        scope: "project".to_string(),
                        agent: AgentKind::Shared,
                        install_target: "shared:/tmp/tendi-test/.agents/skills".to_string(),
                        source_kind: "local".to_string(),
                        source: Some("local:/tmp/tendi-test/.agents/skills".to_string()),
                        source_ref: None,
                        source_version: None,
                        source_relative_path: None,
                        symlink_status: "local".to_string(),
                        update_status: "local".to_string(),
                        sha256: "abc123".to_string(),
                        tags: Vec::new(),
                        tendi_visibility: Some(visibility),
                        effective_visibility: visibility,
                        provider_allow_implicit_invocation: None,
                        provider_skill_enabled: None,
                        provider_disable_model_invocation: None,
                        plugin_id: None,
                        plugin_enabled: None,
                    }],
                    source_summary: "local:/tmp/tendi-test/.agents/skills".to_string(),
                    install_targets: vec!["shared:/tmp/tendi-test/.agents/skills".to_string()],
                    update_status: "local".to_string(),
                    is_system: false,
                    ctime: None,
                    mtime: None,
                }],
                warnings: Vec::new(),
            },
            sessions: SessionScan {
                sessions: Vec::new(),
                warnings: Vec::new(),
            },
            rules: RuleScan {
                rules: Vec::new(),
                warnings: Vec::new(),
            },
            hooks: HookScan {
                hooks: Vec::new(),
                warnings: Vec::new(),
            },
            mcp: McpScan {
                servers: Vec::new(),
                warnings: Vec::new(),
            },
        }
    }


    #[test]
    fn scoped_session_projection_keeps_workspaces_isolated() {
        let temp = temp_dir("tendi-storage-scoped-sessions");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let first_scope = ScopeKey::new("workspace:first").unwrap();
        let second_scope = ScopeKey::new("workspace:second").unwrap();
        let first_path = temp.join("first.jsonl");
        let second_path = temp.join("second.jsonl");
        let transcript = |marker: &str| {
            include_str!("../testdata/transcripts/codex.jsonl")
                .replace("Inspect the fixture parser", marker)
        };
        fs::write(&first_path, transcript("first-private-marker")).unwrap();
        fs::write(&second_path, transcript("second-private-marker")).unwrap();
        let mut first_session = session("shared", "First workspace");
        first_session.path = first_path;
        let mut second_session = session("shared", "Second workspace");
        second_session.path = second_path;

        store
            .save_sessions_at_for_scope(
                &first_scope,
                &SessionScan {
                    sessions: vec![first_session],
                    warnings: Vec::new(),
                },
                1,
            )
            .unwrap();
        store
            .save_sessions_at_for_scope(
                &second_scope,
                &SessionScan {
                    sessions: vec![second_session],
                    warnings: Vec::new(),
                },
                2,
            )
            .unwrap();

        let first = store.list_sessions_for_scope(&first_scope).unwrap();
        let second = store.list_sessions_for_scope(&second_scope).unwrap();
        assert_eq!(first.sessions[0].title.as_deref(), Some("First workspace"));
        assert_eq!(second.sessions[0].title.as_deref(), Some("Second workspace"));
        assert_eq!(store.sessions_last_scan_at_for_scope(&first_scope).unwrap(), Some(1));
        assert_eq!(store.sessions_last_scan_at_for_scope(&second_scope).unwrap(), Some(2));
        assert_eq!(
            store
                .search_sessions_for_scope(&first_scope, "first", None)
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .search_sessions_for_scope(&second_scope, "first", None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .search_sessions_for_scope(&first_scope, "first-private-marker", None)
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .search_sessions_for_scope(&second_scope, "first-private-marker", None)
                .unwrap()
                .is_empty()
        );
        let legacy_session_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(legacy_session_count, 0);

        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn scoped_analytics_ignores_records_from_another_workspace() {
        let temp = temp_dir("tendi-storage-scoped-analytics");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let first_scope = ScopeKey::new("workspace:first").unwrap();
        let second_scope = ScopeKey::new("workspace:second").unwrap();
        let mut first_session = session("same", "First");
        first_session.path = PathBuf::from("/tmp/scope-first.jsonl");
        let mut second_session = session("same", "Second");
        second_session.path = PathBuf::from("/tmp/scope-second.jsonl");
        store
            .save_sessions_at_for_scope(
                &first_scope,
                &SessionScan {
                    sessions: vec![first_session.clone()],
                    warnings: Vec::new(),
                },
                1,
            )
            .unwrap();
        store
            .save_sessions_at_for_scope(
                &second_scope,
                &SessionScan {
                    sessions: vec![second_session.clone()],
                    warnings: Vec::new(),
                },
                1,
            )
            .unwrap();
        let mut first_analytics = analytics_record("same", AgentKind::Codex, "2026-08-28T10:00:00Z", 11);
        first_analytics.analytics.session_path = first_session.path;
        let mut second_analytics = analytics_record("same", AgentKind::Codex, "2026-08-28T10:00:00Z", 99);
        second_analytics.analytics.session_path = second_session.path;
        store
            .save_session_analytics_records_for_scope(&first_scope, &[first_analytics])
            .unwrap();
        store
            .save_session_analytics_records_for_scope(&second_scope, &[second_analytics])
            .unwrap();

        let overview = store
            .overview_analytics_for_scope(&first_scope, None, 30, 30)
            .unwrap();
        assert_eq!(overview.coverage.total_sessions, 1);
        assert_eq!(overview.summary.usage.total_tokens, 11);
        assert_eq!(overview.summary.sessions, 1);

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn scoped_analytics_indexing_count_ignores_sessions_outside_requested_range() {
        let temp = temp_dir("tendi-storage-analytics-coverage");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let scope = ScopeKey::new("workspace:coverage").unwrap();
        let mut recent_session = session("recent", "Recent");
        recent_session.path = PathBuf::from("/tmp/scope-recent.jsonl");
        let mut old_session = session("old", "Old");
        old_session.path = PathBuf::from("/tmp/scope-old.jsonl");
        store
            .save_sessions_at_for_scope(
                &scope,
                &SessionScan {
                    sessions: vec![recent_session.clone(), old_session.clone()],
                    warnings: Vec::new(),
                },
                1,
            )
            .unwrap();

        let mut recent_analytics = analytics_record(
            "recent",
            AgentKind::Codex,
            &Local::now().to_rfc3339(),
            11,
        );
        recent_analytics.analytics.session_path = recent_session.path;
        let mut old_analytics = analytics_record(
            "old",
            AgentKind::Codex,
            "2020-01-01T10:00:00Z",
            99,
        );
        old_analytics.analytics.session_path = old_session.path;
        store
            .save_session_analytics_records_for_scope(
                &scope,
                &[recent_analytics, old_analytics],
            )
            .unwrap();

        let overview = store
            .overview_analytics_for_scope(&scope, None, 30, 30)
            .unwrap();
        assert_eq!(overview.coverage.total_sessions, 2);
        assert_eq!(overview.coverage.first.as_deref(), Some("2020-01-01"));
        assert_eq!(overview.coverage.analyzed_sessions, 2);
        assert_eq!(overview.coverage.indexing_sessions, 0);
        assert_eq!(overview.summary.sessions, 1);

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }







    #[test]
    fn analytics_overview_index_migrates_old_schema_in_resumable_batches() {
        let temp = temp_dir("tendi-storage-overview-migration");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let timestamp = Local::now().to_rfc3339();
        let records = (0..3)
            .map(|index| {
                analytics_record(
                    &format!("legacy-{index}"),
                    AgentKind::Codex,
                    &timestamp,
                    10 + index,
                )
            })
            .collect::<Vec<_>>();
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session_analytics (
                    session_id TEXT NOT NULL,
                    agent TEXT NOT NULL,
                    session_path TEXT NOT NULL,
                    file_mtime INTEGER NOT NULL,
                    file_size INTEGER NOT NULL,
                    indexed_at TEXT NOT NULL,
                    analytics_json TEXT NOT NULL,
                    parser_state_json TEXT NOT NULL,
                    PRIMARY KEY (session_id, agent, session_path)
                );",
            )
            .unwrap();
        for record in &records {
            connection
                .execute(
                "INSERT INTO session_analytics
                 (session_id, agent, session_path, file_mtime, file_size, indexed_at, analytics_json, parser_state_json)
                 VALUES (?1, 'codex', ?2, 0, 0, '0', ?3, ?4)",
                params![
                    record.analytics.session_id,
                    record.analytics.session_path.display().to_string(),
                    serde_json::to_string(&record.analytics).unwrap(),
                    serde_json::to_string(&record.state).unwrap(),
                ],
                )
                .unwrap();
        }
        drop(connection);

        let store = Store::open(&db).unwrap();
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT SUM(overview_indexed) FROM session_analytics",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
        );
        let first = store
            .backfill_session_analytics_overview_index_batch(1)
            .unwrap();
        assert_eq!((first.processed, first.remaining), (1, 2));
        let indexed_date = store
            .conn
            .query_row(
                "SELECT event_max_date FROM session_analytics WHERE overview_indexed = 1 LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(indexed_date, Local::now().date_naive().to_string());
        store
            .conn
            .execute(
                "UPDATE session_analytics SET analytics_json = 'not-json' WHERE overview_indexed = 1",
                [],
            )
            .unwrap();
        drop(store);

        let reopened = Store::open(&db).unwrap();
        let second = reopened
            .backfill_session_analytics_overview_index_batch(1)
            .unwrap();
        assert_eq!((second.processed, second.remaining), (1, 1));
        let third = reopened
            .backfill_session_analytics_overview_index_batch(1)
            .unwrap();
        assert_eq!((third.processed, third.remaining), (1, 0));
        drop(reopened);
        fs::remove_dir_all(temp).unwrap();
    }


    #[test]
    fn concurrent_store_open_serializes_schema_migrations() {
        let temp = temp_dir("tendi-storage-concurrent-open");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let db = db.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    Store::open(db).map(|store| drop(store))
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().unwrap().unwrap();
        }
        let store = Store::open(&db).unwrap();
        let columns = store
            .conn
            .prepare("PRAGMA table_info(session_analytics)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "overview_indexed"));
        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn concurrent_writers_complete_one_hundred_rounds_without_sqlite_lock_errors() {
        let temp = temp_dir("tendi-storage-writer-soak");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        Store::open(&db).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let handles = (0..4)
            .map(|worker| {
                let db = db.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    let store = Store::open(&db).unwrap();
                    let baseline = store.app_settings().unwrap();
                    barrier.wait();
                    for round in 0..100 {
                        let mut settings = baseline.clone();
                        settings.terminal = format!("worker-{worker}-{round}");
                        let saved = store
                            .with_database_write_lock_retry(|| store.save_app_settings(settings.clone()))
                            .unwrap();
                        assert_eq!(saved.terminal, format!("worker-{worker}-{round}"));
                    }
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap();
        }
        drop(Store::open(&db).unwrap());
        fs::remove_dir_all(temp).unwrap();
    }




    #[test]
    fn prompts_can_be_saved_updated_and_deleted() {
        let temp = temp_dir("tendi-storage-prompts");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let store = Store::open(&db).unwrap();

        let saved = store
            .save_prompt(PromptWrite {
                id: None,
                title: "Review".to_string(),
                tags: vec!["Code".to_string(), "Review".to_string()],
                body: "Review this diff".to_string(),
            })
            .unwrap();

        assert_eq!(store.list_prompts().unwrap().len(), 1);
        assert_eq!(
            store.list_prompts().unwrap()[0].tags,
            vec!["Code", "Review"]
        );

        let updated = store
            .save_prompt(PromptWrite {
                id: Some(saved.id.clone()),
                title: "Review updated".to_string(),
                tags: vec!["Code".to_string(), "Deep".to_string()],
                body: "Review this diff carefully".to_string(),
            })
            .unwrap();

        assert_eq!(updated.id, saved.id);
        assert_eq!(updated.created_at, saved.created_at);
        let prompts = store.list_prompts().unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].title, "Review updated");
        assert_eq!(prompts[0].tags, vec!["Code", "Deep"]);

        assert_eq!(store.delete_prompts(&[saved.id]).unwrap(), 1);
        assert!(store.list_prompts().unwrap().is_empty());

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn prompts_reject_empty_titles_before_writing() {
        let temp = temp_dir("tendi-storage-prompts-empty-title");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();

        for (index, title) in ["", "  \n\t"].into_iter().enumerate() {
            let error = store
                .save_prompt(PromptWrite {
                    id: Some(format!("prompt-empty-title-{index}")),
                    title: title.to_string(),
                    tags: Vec::new(),
                    body: "Body".to_string(),
                })
                .expect_err("empty prompt title must be rejected");
            assert_eq!(error.to_string(), "prompt title is required");
        }

        assert!(store.list_prompts().unwrap().is_empty());
        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn malformed_prompt_tags_are_not_replaced_by_category() {
        let temp = temp_dir("tendi-storage-prompts-invalid-tags");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        store
            .save_prompt(PromptWrite {
                id: Some("prompt-test".to_string()),
                title: "Review".to_string(),
                tags: vec!["Code".to_string()],
                body: "Review this diff".to_string(),
            })
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE prompts SET category = 'legacy', tags_json = 'not-json' WHERE id = 'prompt-test'",
                [],
            )
            .unwrap();

        assert!(store.list_prompts().is_err());

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }


    fn session(id: &str, title: &str) -> SessionRecord {
        SessionRecord {
            id: id.to_string(),
            agent: AgentKind::Codex,
            title: Some(title.to_string()),
            project: Some(PathBuf::from("/tmp/tendi-test")),
            repository: None,
            repository_url: None,
            logical_project_id: None,
            logical_project_name: None,
            path: PathBuf::from(format!("/tmp/tendi-test/{id}.jsonl")),
            started_at: Some("2026-06-23T10:00:00Z".to_string()),
            updated_at: Some(format!("2026-06-23T10:0{}:00Z", id.len())),
            message_count: Some(id.len()),
            first_user_message: None,
            last_user_message: None,
            last_assistant_message: None,
            turn_count: Some(id.len()),
            model: None,
            mode: None,
            approval_mode: None,
            is_run_everything: None,
            parent_session_id: None,
            token_usage: None,
        }
    }

    fn analytics_record(
        id: &str,
        agent: AgentKind,
        timestamp: &str,
        total_tokens: u64,
    ) -> SessionAnalyticsRecord {
        let usage = AnalyticsTokenUsage {
            input_tokens: total_tokens,
            total_tokens,
            ..AnalyticsTokenUsage::default()
        };
        SessionAnalyticsRecord {
            analytics: SessionAnalytics {
                session_id: id.to_string(),
                agent,
                session_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
                responses: vec![AnalyticsResponseUsage {
                    index: 1,
                    timestamp: timestamp.to_string(),
                    model: "test-model".to_string(),
                    usage,
                    cumulative: usage,
                }],
                ..SessionAnalytics::default()
            },
            state: AnalyticsParserState::default(),
            file_mtime: 0,
            file_size: 0,
        }
    }

    #[test]
    fn scoped_session_skill_links_do_not_cross_workspace_boundaries() {
        let temp = temp_dir("tendi-storage-scoped-session-skills");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let transcript = temp.join("same.jsonl");
        fs::write(&transcript, "{}\n").unwrap();
        let store = Store::open(&db).unwrap();
        let first_scope = ScopeKey::new("workspace:/first").unwrap();
        let second_scope = ScopeKey::new("workspace:/second").unwrap();
        let mut session = session("same", "Same session");
        session.path = transcript;
        let scan = SessionScan {
            sessions: vec![session.clone()],
            warnings: Vec::new(),
        };
        store
            .save_sessions_at_for_scope(&first_scope, &scan, 1)
            .unwrap();
        store
            .save_sessions_at_for_scope(&second_scope, &scan, 1)
            .unwrap();
        let state = SessionFileState {
            file_mtime: 1,
            file_size: 3,
        };

        store
            .replace_session_skill_links_for_scope(
                &first_scope,
                &session,
                &state,
                &[link(&session, "first-skill")],
            )
            .unwrap();
        store
            .replace_session_skill_links_for_scope(
                &second_scope,
                &session,
                &state,
                &[link(&session, "second-skill")],
            )
            .unwrap();

        assert_eq!(
            store
                .skill_session_links_for_scope(&first_scope, "first-skill")
                .unwrap()
                .len(),
            1
        );
        assert!(store
            .skill_session_links_for_scope(&first_scope, "second-skill")
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .session_skill_index_status_for_scope(&first_scope, false)
                .unwrap()
                .indexed,
            1
        );
        assert_eq!(
            store
                .session_skill_index_status_for_scope(&second_scope, false)
                .unwrap()
                .indexed,
            1
        );

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    fn link(session: &SessionRecord, skill_name: &str) -> SessionSkillLink {
        SessionSkillLink {
            session_id: session.id.clone(),
            agent: session.agent,
            session_path: session.path.clone(),
            session_title: session.title.clone(),
            session_project: session.project.clone(),
            session_started_at: session.started_at.clone(),
            session_updated_at: session.updated_at.clone(),
            session_message_count: session.message_count,
            skill_name: skill_name.to_string(),
            skill_path: PathBuf::from(format!("/tmp/tendi-test/.codex/skills/{skill_name}")),
            skill_agent: Some(AgentKind::Codex),
            skill_scope: Some("global".to_string()),
            evidence_kind: "exec_command".to_string(),
            evidence_text: format!("cat /tmp/tendi-test/.codex/skills/{skill_name}/SKILL.md"),
            evidence_time: Some("2026-06-23T10:00:00Z".to_string()),
            confidence: "observed".to_string(),
        }
    }

    #[test]
    fn projection_heads_are_monotonic_per_scope_and_domain() {
        let temp = temp_dir("tendi-projection-head");
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let scope = ScopeKey::new("workspace:/repo").unwrap();
        let source = SourceVersion::new("sha-1").unwrap();

        assert!(store.projection_head(&scope, "sessions").unwrap().is_none());
        let first = store
            .advance_projection_head(&scope, "sessions", Some(&source), "ready")
            .unwrap();
        let second = store
            .advance_projection_head(&scope, "sessions", Some(&source), "ready")
            .unwrap();

        assert_eq!(first.revision, Revision::new(1));
        assert_eq!(second.revision, Revision::new(2));
        assert_eq!(
            store
                .projection_head(&scope, "sessions")
                .unwrap()
                .unwrap()
                .revision,
            Revision::new(2)
        );
    }

    #[test]
    fn operation_journal_round_trips_terminal_state_and_error() {
        let temp = temp_dir("tendi-operation-journal");
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let operation = OperationRecord {
            operation_id: OperationId::new("op-1").unwrap(),
            kind: OperationKind::Scan,
            scope_key: ScopeKey::new("workspace:/repo").unwrap(),
            status: OperationStatus::Running,
            input_revision: Revision::new(4),
            source_version: Some(SourceVersion::new("sha-1").unwrap()),
            checkpoint_json: Some("{\"offset\":12}".to_string()),
            error: None,
        };
        store.record_operation(&operation).unwrap();
        assert_eq!(store.operation(&operation.operation_id).unwrap(), Some(operation.clone()));

        assert!(store
            .update_operation(
                &operation.operation_id,
                OperationStatus::Failed,
                operation.checkpoint_json.as_deref(),
                Some("provider failed"),
            )
            .unwrap());
        let saved = store.operation(&operation.operation_id).unwrap().unwrap();
        assert_eq!(saved.status, OperationStatus::Failed);
        assert_eq!(saved.error.as_deref(), Some("provider failed"));
    }

    #[test]
    fn recovery_marks_unfinished_operations_as_failed() {
        let temp = temp_dir("tendi-operation-recovery");
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let scope = ScopeKey::new("workspace:/repo").unwrap();
        for (id, status) in [
            ("queued-op", OperationStatus::Queued),
            ("running-op", OperationStatus::Running),
            ("committing-op", OperationStatus::Committing),
            ("committed-op", OperationStatus::Committed),
        ] {
            store
                .record_operation(&OperationRecord {
                    operation_id: OperationId::new(id).unwrap(),
                    kind: OperationKind::Projection,
                    scope_key: scope.clone(),
                    status,
                    input_revision: Revision::ZERO,
                    source_version: None,
                    checkpoint_json: None,
                    error: None,
                })
                .unwrap();
        }

        assert_eq!(store.recover_inflight_operations().unwrap(), 3);
        for id in ["queued-op", "running-op", "committing-op"] {
            let operation = store
                .operation(&OperationId::new(id).unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(operation.status, OperationStatus::Failed);
            assert_eq!(
                operation.error.as_deref(),
                Some("daemon restarted before the operation completed")
            );
        }
        assert_eq!(
            store
                .operation(&OperationId::new("committed-op").unwrap())
                .unwrap()
                .unwrap()
                .status,
            OperationStatus::Committed
        );
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
    }
}
