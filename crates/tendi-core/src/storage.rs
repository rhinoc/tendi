use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};
use serde::{Deserialize, Serialize};

#[path = "fs_manifest.rs"]
mod fs_manifest;
pub use fs_manifest::{FsManifestEntry, canonical_workspace_root};

use crate::{
    HookRecord, HookScan, McpScan, McpServerRecord, RuleRecord, RuleScan, ScanReport,
    analytics::{
        self, AnalyticsCapabilities, AnalyticsCoverage, AnalyticsProviderCapability,
        AnalyticsRefreshProgress, AnalyticsRefreshReport, OverviewAnalytics,
        SessionAnalyticsRecord,
    },
    fsutil::sha256_text,
    rules::merge_rules_by_path,
    session_skills::{SessionFileState, SessionSkillIndexStatus, SessionSkillLink},
    sessions::{
        SESSION_PREVIEW_MAX_CHARS, SessionRecord, SessionScan, SessionScanCache,
        SessionScanCacheEntry, bound_session_preview, clean_session_title,
        normalize_session_projects,
    },
    skills::{
        AgentKind, SkillRecord, SkillScan, SkillSnapshot, SkillSnapshotFile, SkillSourceRecord,
    },
    transcript,
};

#[derive(Debug, Clone)]
struct ProjectState {
    name: String,
    name_custom: bool,
    last_seen_at: String,
}

const SESSION_SEARCH_RECORD_TEXT_LIMIT: usize = 64 * 1024;
const SESSION_ANALYTICS_BATCH_SIZE: usize = 64;
const PROJECTION_PARSER_VERSION: &str = "scan-v4";
const PROJECTION_REFRESH_LOCK_TTL_SECS: i64 = 300;
static PROJECTION_REFRESH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const PROJECTION_DOMAINS: [&str; 5] = ["agents", "skills", "rules", "hooks", "mcp"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionStatus {
    Fresh,
    Missing,
    Stale,
    Refreshing,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProjectionCount {
    pub rows: usize,
    pub secondary: usize,
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
    "sakura-pop".to_string()
}

fn default_app_icon() -> String {
    "sakura-pop".to_string()
}

fn default_editor() -> String {
    "vscode".to_string()
}

fn default_session_resume_target() -> String {
    "terminal".to_string()
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
        "sakura-pop"
            | "gruvbox"
            | "dracula"
            | "nord"
            | "catppuccin"
            | "tokyo-night"
            | "vercel"
    ) {
        Ok(theme)
    } else {
        anyhow::bail!("invalid color theme setting: {value}")
    }
}

fn normalize_session_resume_target(value: &str) -> Result<String> {
    let target = value.trim().to_ascii_lowercase();
    match target.as_str() {
        "terminal" => Ok(target),
        "app" | "codex" => Ok("app".to_string()),
        _ => anyhow::bail!("invalid session resume target: {value}"),
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

#[derive(Debug, Clone, Serialize)]
pub struct SessionSearchHit {
    #[serde(flatten)]
    pub session: SessionRecord,
    pub search_score: f64,
    pub search_snippet: String,
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
        for attempt in 0..5 {
            match store.init() {
                Ok(()) => break,
                Err(error)
                    if attempt < 4
                        && error
                            .downcast_ref::<rusqlite::Error>()
                            .is_some_and(|error| {
                                matches!(
                                    error,
                                    rusqlite::Error::SqliteFailure(code, _)
                                        if code.code == rusqlite::ErrorCode::DatabaseBusy
                                            || code.code == rusqlite::ErrorCode::DatabaseLocked
                                )
                            }) =>
                {
                    std::thread::yield_now();
                }
                Err(error) => return Err(error),
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
        let mut config_profiles = self
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
        for (legacy_key, agent) in [("codex_profile", "codex"), ("claude_profile", "claude")] {
            let legacy_profile = self
                .conn
                .query_row(
                    "SELECT value FROM app_settings WHERE key = ?1",
                    [legacy_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if let Some(profile) = legacy_profile {
                config_profiles.entry(agent.to_string()).or_insert(profile);
            }
        }
        Ok(AppSettings {
            appearance,
            font_family,
            light_theme,
            dark_theme,
            app_icon,
            terminal,
            session_resume_target,
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
        self.conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('appearance', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![appearance],
        )?;
        self.conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('font_family', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![font_family],
        )?;
        self.conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('light_theme', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![light_theme],
        )?;
        self.conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('dark_theme', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![dark_theme],
        )?;
        self.conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('app_icon', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![app_icon],
        )?;
        self.conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('terminal', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![terminal],
        )?;
        self.conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('session_resume_target', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![session_resume_target],
        )?;
        self.conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('editor', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![editor],
        )?;
        self.conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('developer_mode', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![serde_json::to_string(&developer_mode)?],
        )?;
        self.conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('additional_session_roots', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![serde_json::to_string(&additional_session_roots)?],
        )?;
        self.conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('config_profiles', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![serde_json::to_string(&config_profiles)?],
        )?;
        Ok(AppSettings {
            appearance,
            font_family,
            light_theme,
            dark_theme,
            app_icon,
            terminal,
            session_resume_target,
            editor,
            developer_mode,
            additional_session_roots,
            config_profiles,
        })
    }

    pub fn save_scan(&self, report: &ScanReport) -> Result<()> {
        self.save_scan_inner(report)?;
        self.upsert_fs_manifest_entries(&fs_manifest_entries_from_scan(report))?;
        Ok(())
    }

    /// Persist a complete scan and bind all filesystem-backed projections to
    /// one canonical workspace context.
    pub fn save_scan_for_workspace(
        &self,
        workspace_root: &Path,
        report: &ScanReport,
    ) -> Result<()> {
        self.save_scan_inner(report)?;
        self.finalize_projection_scan(workspace_root, report)
    }

    fn save_scan_inner(&self, report: &ScanReport) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(
            "
            DELETE FROM agents;
            DELETE FROM skills;
            DELETE FROM skill_paths;
            CREATE TEMP TABLE IF NOT EXISTS current_sessions (
                id TEXT NOT NULL,
                agent TEXT NOT NULL,
                path TEXT NOT NULL,
                PRIMARY KEY (id, agent, path)
            );
            DELETE FROM current_sessions;
            DELETE FROM rules;
            DELETE FROM hooks;
            DELETE FROM mcp_servers;
            ",
        )?;

        for agent in &report.agents.agents {
            tx.execute(
                "INSERT INTO agents (kind, name, installed, config_dir, executable, version, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    agent_label(agent.kind),
                    agent.name,
                    agent.installed,
                    agent.config_dir.as_ref().map(|path| path.display().to_string()),
                    agent.executable,
                    agent.version,
                    serde_json::to_string(agent)?,
                ],
            )?;
        }

        for skill in &report.skills.skills {
            tx.execute(
                "INSERT INTO skills (name, visibility, agents_json, description, is_system, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    skill.name,
                    format!("{:?}", skill.visibility).to_lowercase(),
                    serde_json::to_string(&skill.agents)?,
                    skill.description,
                    skill.is_system,
                    serde_json::to_string(skill)?,
                ],
            )?;
            for path in &skill.paths {
                tx.execute(
                    "INSERT INTO skill_paths (skill_name, path, root, scope, agent, sha256, data_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(skill_name, path) DO UPDATE SET
                        root = excluded.root,
                        scope = excluded.scope,
                        agent = excluded.agent,
                        sha256 = excluded.sha256,
                        data_json = excluded.data_json",
                    params![
                        skill.name,
                        path.path.display().to_string(),
                        path.root.display().to_string(),
                        path.scope,
                        agent_label(path.agent),
                        path.sha256,
                        serde_json::to_string(path)?,
                    ],
                )?;
            }
        }

        for session in &report.sessions.sessions {
            let title = clean_session_title(session.title.clone());
            let first_user_message = bound_session_preview(session.first_user_message.clone());
            let last_user_message = bound_session_preview(session.last_user_message.clone());
            let last_assistant_message =
                bound_session_preview(session.last_assistant_message.clone());
            tx.execute(
                "INSERT INTO current_sessions (id, agent, path)
                 VALUES (?1, ?2, ?3)",
                params![
                    session.id,
                    agent_label(session.agent),
                    session.path.display().to_string(),
                ],
            )?;
            tx.execute(
                "INSERT INTO sessions (id, agent, title, project, path, started_at, updated_at, message_count, first_user_message, last_user_message, last_assistant_message, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id, agent, path) DO UPDATE SET
                    title = excluded.title,
                    project = excluded.project,
                    started_at = excluded.started_at,
                    updated_at = excluded.updated_at,
                    message_count = excluded.message_count,
                    first_user_message = excluded.first_user_message,
                    last_user_message = excluded.last_user_message,
                    last_assistant_message = excluded.last_assistant_message,
                    data_json = excluded.data_json
                 WHERE sessions.data_json IS NOT excluded.data_json
                    OR sessions.first_user_message IS NOT excluded.first_user_message
                    OR sessions.last_user_message IS NOT excluded.last_user_message
                    OR sessions.last_assistant_message IS NOT excluded.last_assistant_message",
                params![
                    session.id,
                    agent_label(session.agent),
                    title,
                    session.project.as_ref().map(|path| path.display().to_string()),
                    session.path.display().to_string(),
                    session.started_at,
                    session.updated_at,
                    session.message_count.map(|value| value as i64),
                    first_user_message,
                    last_user_message,
                    last_assistant_message,
                    Self::session_metadata_json(session)?,
                ],
            )?;
            index_session_search_document_best_effort(&tx, session);
        }
        if report.sessions.warnings.is_empty() {
            tx.execute(
                "DELETE FROM sessions
                 WHERE NOT EXISTS (
                    SELECT 1 FROM current_sessions
                    WHERE current_sessions.id = sessions.id
                      AND current_sessions.agent = sessions.agent
                      AND current_sessions.path = sessions.path
                 )",
                [],
            )?;
            cleanup_stale_session_skill_rows(&tx)?;
            cleanup_stale_session_search_rows(&tx)?;
            cleanup_stale_session_analytics_rows(&tx)?;
        }
        tx.execute("DELETE FROM current_sessions", [])?;

        for rule in &report.rules.rules {
            tx.execute(
                "INSERT INTO rules (agent, kind, scope, path, effective_order, sha256, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    agent_label(primary_rule_agent(rule)),
                    rule.kind,
                    rule.scope,
                    rule.path.display().to_string(),
                    rule.order as i64,
                    rule.sha256,
                    serde_json::to_string(rule)?,
                ],
            )?;
        }

        for hook in &report.hooks.hooks {
            tx.execute(
                "INSERT INTO hooks (agent, event, command, enabled, path, trust_hash, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    agent_label(hook.agent),
                    hook.event,
                    hook.command,
                    hook.enabled,
                    hook.path.display().to_string(),
                    hook.trust_hash,
                    serde_json::to_string(hook)?,
                ],
            )?;
        }

        for server in &report.mcp.servers {
            tx.execute(
                "INSERT INTO mcp_servers (agent, name, transport, status, path, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    agent_label(server.agent),
                    server.name,
                    server.transport,
                    server.status,
                    server.path.display().to_string(),
                    serde_json::to_string(server)?,
                ],
            )?;
        }

        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('last_scan_at', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![unix_now().to_string()],
        )?;
        cleanup_stale_session_skill_rows(&tx)?;
        cleanup_stale_skill_link_rows(&tx)?;
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
                    source_kind, path, root, agent, scope, mtime_ns, size, inode, device,
                    sha256, parser_version, last_seen_at, parse_status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(source_kind, path) DO UPDATE SET
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
                 WHERE source_kind = ?1 AND path = ?2",
                params![source_kind, path.display().to_string()],
                fs_manifest_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_fs_manifest_for_root(&self, root: &Path) -> Result<Vec<FsManifestEntry>> {
        let mut statement = self.conn.prepare(
            "SELECT source_kind, path, root, agent, scope, mtime_ns, size, inode, device,
                    sha256, parser_version, last_seen_at, parse_status
             FROM fs_manifest
             WHERE root = ?1
             ORDER BY source_kind ASC, path ASC",
        )?;
        let rows =
            statement.query_map(params![root.display().to_string()], fs_manifest_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn delete_fs_manifest_entry(&self, source_kind: &str, path: &Path) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM fs_manifest WHERE source_kind = ?1 AND path = ?2",
            params![source_kind, path.display().to_string()],
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
                 WHERE source_kind = ?1 AND root = ?2",
            )?;
            let rows = statement.query_map(params![source_kind, root_key], |row| {
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
                     WHERE source_kind = ?1 AND path = ?2 AND root = ?3",
                    params![source_kind, path, root_key],
                )?;
            }
        }
        tx.commit()?;
        Ok(deleted)
    }

    fn list_fs_manifest_for_domain(&self, domain: &str) -> Result<Vec<FsManifestEntry>> {
        let kinds = manifest_source_kinds(domain)?;
        let placeholders = std::iter::repeat_n("?", kinds.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT source_kind, path, root, agent, scope, mtime_ns, size, inode, device,
                    sha256, parser_version, last_seen_at, parse_status
             FROM fs_manifest
             WHERE source_kind IN ({placeholders})
             ORDER BY source_kind ASC, path ASC"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(kinds.iter()), fs_manifest_from_row)?;
        let entries = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    fn finalize_projection_scan(&self, workspace_root: &Path, report: &ScanReport) -> Result<()> {
        let workspace_root = canonical_workspace_root(workspace_root);
        self.finalize_projection_domain(
            "agents",
            &workspace_root,
            &manifest_entries_for_agents(&report.agents, &workspace_root),
            report.agents.warnings.is_empty(),
            (!report.agents.warnings.is_empty()).then(|| report.agents.warnings.join("; ")),
        )?;
        self.finalize_projection_domain(
            "skills",
            &workspace_root,
            &manifest_entries_for_skills(&report.skills, &workspace_root),
            report.skills.warnings.is_empty(),
            (!report.skills.warnings.is_empty()).then(|| report.skills.warnings.join("; ")),
        )?;
        self.finalize_projection_domain(
            "rules",
            &workspace_root,
            &manifest_entries_for_rules(&report.rules, &workspace_root),
            report.rules.warnings.is_empty(),
            (!report.rules.warnings.is_empty()).then(|| report.rules.warnings.join("; ")),
        )?;
        self.finalize_projection_domain(
            "hooks",
            &workspace_root,
            &manifest_entries_for_hooks(&report.hooks, &workspace_root),
            report.hooks.warnings.is_empty(),
            (!report.hooks.warnings.is_empty()).then(|| report.hooks.warnings.join("; ")),
        )?;
        self.finalize_projection_domain(
            "mcp",
            &workspace_root,
            &manifest_entries_for_mcp(&report.mcp, &workspace_root),
            report.mcp.warnings.is_empty(),
            (!report.mcp.warnings.is_empty()).then(|| report.mcp.warnings.join("; ")),
        )?;
        Ok(())
    }

    fn finalize_projection_domain(
        &self,
        domain: &str,
        workspace_root: &Path,
        entries: &[FsManifestEntry],
        ready: bool,
        error: Option<String>,
    ) -> Result<()> {
        let kinds = manifest_source_kinds(domain)?;
        let placeholders = std::iter::repeat_n("?", kinds.len())
            .collect::<Vec<_>>()
            .join(", ");
        let delete_sql = format!("DELETE FROM fs_manifest WHERE source_kind IN ({placeholders})");
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(&delete_sql, params_from_iter(kinds.iter()))?;
        for entry in entries {
            tx.execute(
                "INSERT INTO fs_manifest (
                    source_kind, path, root, agent, scope, mtime_ns, size, inode, device,
                    sha256, parser_version, last_seen_at, parse_status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(source_kind, path) DO UPDATE SET
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
        self.set_projection_context(
            domain,
            &canonical_workspace_root(workspace_root),
            if ready { "ready" } else { "failed" },
            error,
        )
    }

    fn set_projection_context(
        &self,
        domain: &str,
        workspace_root: &Path,
        state: &str,
        error: Option<String>,
    ) -> Result<()> {
        ensure_projection_domain(domain)?;
        self.conn.execute(
            "INSERT INTO projection_contexts
                (domain, workspace_root, state, scanned_at, error, parser_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(domain) DO UPDATE SET
                workspace_root = excluded.workspace_root,
                state = excluded.state,
                scanned_at = excluded.scanned_at,
                error = excluded.error,
                parser_version = excluded.parser_version",
            params![
                domain,
                canonical_workspace_root(workspace_root)
                    .display()
                    .to_string(),
                state,
                (state == "ready").then(|| unix_now() as i64),
                error,
                PROJECTION_PARSER_VERSION,
            ],
        )?;
        Ok(())
    }

    fn try_acquire_database_write_lock(&self, owner: &str) -> Result<bool> {
        let now = unix_now() as i64;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO projection_refresh_lock (id, owner, started_at)
             VALUES (1, ?1, ?2)",
            params![owner, now],
        )?;
        let (current_owner, started_at) = tx.query_row(
            "SELECT owner, started_at FROM projection_refresh_lock WHERE id = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )?;
        let acquired = if current_owner == owner
            || started_at.saturating_add(PROJECTION_REFRESH_LOCK_TTL_SECS) < now
        {
            tx.execute(
                "UPDATE projection_refresh_lock
                 SET owner = ?1, started_at = ?2
                 WHERE id = 1",
                params![owner, now],
            )?;
            true
        } else {
            false
        };
        tx.commit()?;
        Ok(acquired)
    }

    fn release_database_write_lock(&self, owner: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM projection_refresh_lock WHERE id = 1 AND owner = ?1",
            params![owner],
        )?;
        Ok(())
    }

    pub fn list_sessions(&self) -> Result<SessionScan> {
        let mut stmt = self.conn.prepare(
            "SELECT data_json, first_user_message, last_user_message, last_assistant_message
             FROM sessions
             ORDER BY updated_at DESC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut sessions = Vec::new();
        let mut warnings = Vec::new();

        for row in rows {
            let (data_json, first_user_message, last_user_message, last_assistant_message) = row?;
            match serde_json::from_str::<SessionRecord>(&data_json) {
                Ok(mut session) => {
                    Self::apply_session_preview_columns(
                        &mut session,
                        first_user_message,
                        last_user_message,
                        last_assistant_message,
                    );
                    sessions.push(session);
                }
                Err(err) => warnings.push(format!("invalid cached session row: {err}")),
            }
        }

        Ok(SessionScan { sessions, warnings })
    }

    pub fn list_agents(&self) -> anyhow::Result<crate::agents::AgentScan> {
        #[derive(Deserialize)]
        struct CachedAgentRecord {
            kind: AgentKind,
            name: String,
            installed: bool,
            config_dir: Option<PathBuf>,
            executable: Option<String>,
            version: Option<String>,
        }

        let mut stmt = self
            .conn
            .prepare("SELECT kind, data_json FROM agents ORDER BY kind")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut agents = Vec::new();
        let mut warnings = Vec::new();

        for row in rows {
            let (kind, data_json) = row?;
            match serde_json::from_str::<CachedAgentRecord>(&data_json) {
                Ok(record) => agents.push(crate::agents::AgentRecord {
                    kind: record.kind,
                    name: record.name,
                    installed: record.installed,
                    config_dir: record.config_dir,
                    executable: record.executable,
                    version: record.version,
                }),
                Err(err) => warnings.push(format!("invalid cached agent row ({kind}): {err}")),
            }
        }

        Ok(crate::agents::AgentScan { agents, warnings })
    }

    pub fn list_skills(&self) -> anyhow::Result<SkillScan> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, data_json FROM skills ORDER BY name")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut skills = Vec::new();
        let mut warnings = Vec::new();
        for row in rows {
            let (name, data_json) = row?;
            match serde_json::from_str::<serde_json::Value>(&data_json) {
                Ok(value) => match serde_json::from_value(value) {
                    Ok(skill) => skills.push(skill),
                    Err(err) => warnings.push(format!("invalid cached skill row ({name}): {err}")),
                },
                Err(err) => warnings.push(format!("invalid cached skill row ({name}): {err}")),
            }
        }
        Ok(SkillScan {
            roots: Vec::new(),
            skills,
            warnings,
        })
    }

    pub fn list_rules(&self) -> anyhow::Result<RuleScan> {
        let mut stmt = self.conn.prepare("SELECT agent, kind, scope, path, effective_order, data_json FROM rules ORDER BY agent, kind, scope, path, effective_order")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut rules = Vec::new();
        let mut warnings = Vec::new();
        for row in rows {
            let (agent, kind, scope, path, order, data_json) = row?;
            match serde_json::from_str::<RuleRecord>(&data_json) {
                Ok(mut rule) => {
                    if rule.agents.is_empty() {
                        rule.agents.push(parse_agent_label(&agent));
                    }
                    rules.push(rule);
                }
                Err(err) => warnings.push(format!(
                    "invalid cached rule row ({agent}/{kind}/{scope}/{path}/{order}): {err}"
                )),
            }
        }
        Ok(RuleScan {
            rules: merge_rules_by_path(rules),
            warnings,
        })
    }

    pub fn list_hooks(&self) -> anyhow::Result<HookScan> {
        let mut stmt = self.conn.prepare(
            "SELECT agent, event, path, data_json FROM hooks ORDER BY agent, event, path",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut hooks = Vec::new();
        let mut warnings = Vec::new();
        for row in rows {
            let (agent, event, path, data_json) = row?;
            match serde_json::from_str::<HookRecord>(&data_json) {
                Ok(hook) => hooks.push(hook),
                Err(err) => warnings.push(format!(
                    "invalid cached hook row ({agent}/{event}/{path}): {err}"
                )),
            }
        }
        Ok(HookScan { hooks, warnings })
    }

    pub fn list_mcp(&self) -> anyhow::Result<McpScan> {
        let mut stmt = self.conn.prepare("SELECT agent, name, transport, status, path, data_json FROM mcp_servers ORDER BY agent, name, transport, status, path")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut servers = Vec::new();
        let mut warnings = Vec::new();
        for row in rows {
            let (agent, name, transport, status, path, data_json) = row?;
            match serde_json::from_str::<McpServerRecord>(&data_json) {
                Ok(server) => servers.push(server),
                Err(err) => warnings.push(format!(
                    "invalid cached mcp row ({agent}/{name}/{transport}/{status}/{path}): {err}"
                )),
            }
        }
        Ok(McpScan { servers, warnings })
    }

    pub fn projection_status(
        &self,
        domain: &str,
        workspace_root: &Path,
    ) -> Result<ProjectionStatus> {
        ensure_projection_domain(domain)?;
        let workspace_root = canonical_workspace_root(workspace_root);
        let context = self
            .conn
            .query_row(
                "SELECT workspace_root, state
                 FROM projection_contexts
                 WHERE domain = ?1",
                params![domain],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((stored_root, state)) = context else {
            return Ok(ProjectionStatus::Missing);
        };
        if state == "refreshing" {
            return Ok(ProjectionStatus::Refreshing);
        }
        if stored_root != workspace_root.display().to_string() || state != "ready" {
            return Ok(ProjectionStatus::Stale);
        }

        let entries = self.list_fs_manifest_for_domain(domain)?;
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
        self.list_agents().map(Some)
    }

    pub fn list_skills_for_workspace(&self, workspace_root: &Path) -> Result<Option<SkillScan>> {
        if self.projection_status("skills", workspace_root)? != ProjectionStatus::Fresh {
            return Ok(None);
        }
        self.list_skills().map(Some)
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
        let context = self
            .conn
            .query_row(
                "SELECT workspace_root, state, parser_version
                 FROM projection_contexts
                 WHERE domain = 'skills'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((stored_root, state, parser_version)) = context else {
            return Ok(None);
        };
        if stored_root != workspace_root.display().to_string()
            || state != "ready"
            || parser_version != PROJECTION_PARSER_VERSION
        {
            return Ok(None);
        }

        let entries = self.list_fs_manifest_for_domain("skills")?;
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

        let scan = self.list_skills()?;
        let selected = scan
            .skills
            .iter()
            .filter(|skill| selected_names.contains(&skill.name))
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
        self.list_rules().map(Some)
    }

    pub fn list_hooks_for_workspace(&self, workspace_root: &Path) -> Result<Option<HookScan>> {
        if self.projection_status("hooks", workspace_root)? != ProjectionStatus::Fresh {
            return Ok(None);
        }
        self.list_hooks().map(Some)
    }

    pub fn list_mcp_for_workspace(&self, workspace_root: &Path) -> Result<Option<McpScan>> {
        if self.projection_status("mcp", workspace_root)? != ProjectionStatus::Fresh {
            return Ok(None);
        }
        self.list_mcp().map(Some)
    }

    pub fn count_projection_for_workspace(
        &self,
        domain: &str,
        workspace_root: &Path,
        agent: Option<AgentKind>,
    ) -> Result<Option<ProjectionCount>> {
        if self.projection_status(domain, workspace_root)? != ProjectionStatus::Fresh {
            return Ok(None);
        }
        let agent_label = agent.map(agent_label);
        let count = match domain {
            "agents" => ProjectionCount {
                rows: self.count_rows("agents", "kind", agent_label.as_deref())?,
                secondary: 0,
            },
            "skills" => {
                let scan = self.list_skills()?;
                let skills = scan
                    .skills
                    .into_iter()
                    .filter(|skill| agent.is_none_or(|expected| skill.agents.contains(&expected)));
                let mut count = ProjectionCount::default();
                for skill in skills {
                    count.rows += 1;
                    if skill.update_status == "update-available" {
                        count.secondary += 1;
                    }
                }
                count
            }
            "rules" => ProjectionCount {
                rows: self.count_rows("rules", "agent", agent_label.as_deref())?,
                secondary: 0,
            },
            "hooks" => {
                let scan = self.list_hooks()?;
                let hooks = scan
                    .hooks
                    .into_iter()
                    .filter(|hook| agent.is_none_or(|expected| hook.agent == expected));
                let mut count = ProjectionCount::default();
                for hook in hooks {
                    count.rows += 1;
                    if hook.needs_review {
                        count.secondary += 1;
                    }
                }
                count
            }
            "mcp" => ProjectionCount {
                rows: self.count_rows("mcp_servers", "agent", agent_label.as_deref())?,
                secondary: 0,
            },
            _ => anyhow::bail!("unknown projection domain: {domain}"),
        };
        Ok(Some(count))
    }

    pub fn count_sessions(&self, agent: Option<AgentKind>) -> Result<usize> {
        let agent_label = agent.map(agent_label);
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE (?1 IS NULL OR agent = ?1)",
                params![agent_label],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count as usize)
            .map_err(Into::into)
    }

    pub fn count_prompts(&self) -> Result<usize> {
        self.conn
            .query_row("SELECT COUNT(*) FROM prompts", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|count| count as usize)
            .map_err(Into::into)
    }

    fn count_rows(&self, table: &str, column: &str, agent: Option<&str>) -> Result<usize> {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE (?1 IS NULL OR {column} = ?1)");
        self.conn
            .query_row(&sql, params![agent], |row| row.get::<_, i64>(0))
            .map(|count| count as usize)
            .map_err(Into::into)
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
    pub fn with_database_write_lock<T, F>(&self, write: F) -> Result<Option<T>>
    where
        F: FnOnce() -> Result<T>,
    {
        let owner = format!(
            "{}-{}",
            std::process::id(),
            PROJECTION_REFRESH_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        if !self.try_acquire_database_write_lock(&owner)? {
            return Ok(None);
        }

        let result = write();
        let release_result = self.release_database_write_lock(&owner);
        release_result?;
        result.map(Some)
    }

    /// Coordinate cold-start and targeted scans across Tauri and CLI
    /// processes. All projection domains share the database write lock.
    pub fn with_projection_refresh_lock<T, F>(&self, domain: &str, refresh: F) -> Result<Option<T>>
    where
        F: FnOnce() -> Result<T>,
    {
        ensure_projection_domain(domain)?;
        self.with_database_write_lock(refresh)
    }

    pub fn save_agents_for_workspace(
        &self,
        workspace_root: &Path,
        scan: &crate::agents::AgentScan,
    ) -> Result<()> {
        let workspace_root = canonical_workspace_root(workspace_root);
        if !scan.warnings.is_empty() {
            return self.set_projection_context(
                "agents",
                &workspace_root,
                "failed",
                Some(scan.warnings.join("; ")),
            );
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM agents", [])?;
        for agent in &scan.agents {
            tx.execute(
                "INSERT INTO agents (kind, name, installed, config_dir, executable, version, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    agent_label(agent.kind),
                    agent.name,
                    agent.installed,
                    agent.config_dir.as_ref().map(|path| path.display().to_string()),
                    agent.executable,
                    agent.version,
                    serde_json::to_string(agent)?,
                ],
            )?;
        }
        tx.commit()?;
        self.finalize_projection_domain(
            "agents",
            &workspace_root,
            &manifest_entries_for_agents(scan, &workspace_root),
            scan.warnings.is_empty(),
            (!scan.warnings.is_empty()).then(|| scan.warnings.join("; ")),
        )
    }

    pub fn save_skills_for_workspace(&self, workspace_root: &Path, scan: &SkillScan) -> Result<()> {
        let workspace_root = canonical_workspace_root(workspace_root);
        self.save_skills(scan)?;
        self.finalize_projection_domain(
            "skills",
            &workspace_root,
            &manifest_entries_for_skills(scan, &workspace_root),
            scan.warnings.is_empty(),
            (!scan.warnings.is_empty()).then(|| scan.warnings.join("; ")),
        )
    }

    pub fn save_rules_for_workspace(&self, workspace_root: &Path, scan: &RuleScan) -> Result<()> {
        let workspace_root = canonical_workspace_root(workspace_root);
        if !scan.warnings.is_empty() {
            return self.set_projection_context(
                "rules",
                &workspace_root,
                "failed",
                Some(scan.warnings.join("; ")),
            );
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM rules", [])?;
        for rule in &scan.rules {
            tx.execute(
                "INSERT INTO rules (agent, kind, scope, path, effective_order, sha256, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    agent_label(primary_rule_agent(rule)),
                    rule.kind,
                    rule.scope,
                    rule.path.display().to_string(),
                    rule.order as i64,
                    rule.sha256,
                    serde_json::to_string(rule)?,
                ],
            )?;
        }
        tx.commit()?;
        self.finalize_projection_domain(
            "rules",
            &workspace_root,
            &manifest_entries_for_rules(scan, &workspace_root),
            true,
            None,
        )
    }

    pub fn save_hooks_for_workspace(&self, workspace_root: &Path, scan: &HookScan) -> Result<()> {
        let workspace_root = canonical_workspace_root(workspace_root);
        if !scan.warnings.is_empty() {
            return self.set_projection_context(
                "hooks",
                &workspace_root,
                "failed",
                Some(scan.warnings.join("; ")),
            );
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM hooks", [])?;
        for hook in &scan.hooks {
            tx.execute(
                "INSERT INTO hooks (agent, event, command, enabled, path, trust_hash, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    agent_label(hook.agent),
                    hook.event,
                    hook.command,
                    hook.enabled,
                    hook.path.display().to_string(),
                    hook.trust_hash,
                    serde_json::to_string(hook)?,
                ],
            )?;
        }
        tx.commit()?;
        self.finalize_projection_domain(
            "hooks",
            &workspace_root,
            &manifest_entries_for_hooks(scan, &workspace_root),
            true,
            None,
        )
    }

    pub fn save_mcp_for_workspace(&self, workspace_root: &Path, scan: &McpScan) -> Result<()> {
        let workspace_root = canonical_workspace_root(workspace_root);
        if !scan.warnings.is_empty() {
            return self.set_projection_context(
                "mcp",
                &workspace_root,
                "failed",
                Some(scan.warnings.join("; ")),
            );
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM mcp_servers", [])?;
        for server in &scan.servers {
            tx.execute(
                "INSERT INTO mcp_servers (agent, name, transport, status, path, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    agent_label(server.agent),
                    server.name,
                    server.transport,
                    server.status,
                    server.path.display().to_string(),
                    serde_json::to_string(server)?,
                ],
            )?;
        }
        tx.commit()?;
        self.finalize_projection_domain(
            "mcp",
            &workspace_root,
            &manifest_entries_for_mcp(scan, &workspace_root),
            true,
            None,
        )
    }

    fn apply_session_preview_columns(
        session: &mut SessionRecord,
        first_user_message: Option<String>,
        last_user_message: Option<String>,
        last_assistant_message: Option<String>,
    ) {
        session.title = clean_session_title(session.title.take());
        session.first_user_message =
            bound_session_preview(session.first_user_message.take().or(first_user_message));
        session.last_user_message =
            bound_session_preview(session.last_user_message.take().or(last_user_message));
        session.last_assistant_message = bound_session_preview(
            session
                .last_assistant_message
                .take()
                .or(last_assistant_message),
        );
    }

    fn session_metadata_json(session: &SessionRecord) -> Result<String> {
        let mut metadata = session.clone();
        metadata.title = clean_session_title(metadata.title.take());
        metadata.first_user_message = None;
        metadata.last_user_message = None;
        metadata.last_assistant_message = None;
        Ok(serde_json::to_string(&metadata)?)
    }

    pub fn resolve_session_projects(&self, sessions: &mut [SessionRecord]) -> Result<()> {
        normalize_session_projects(sessions);
        let tx = self.conn.unchecked_transaction()?;
        let mut projects = load_session_projects(&tx)?;
        let mut aliases = load_session_project_aliases(&tx)?;

        for session in sessions {
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
                let project_id = format!(
                    "project-{}",
                    &sha256_text(&format!("{}:{}", seed.0, seed.1))[..24]
                );
                let name = suggested_project_name(session);
                let last_seen_at = session_project_seen_at(session);
                tx.execute(
                    "INSERT OR IGNORE INTO session_projects (id, name, name_custom, last_seen_at)
                     VALUES (?1, ?2, 0, ?3)",
                    params![project_id, name, last_seen_at],
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
                    .max_by_key(|project_id| {
                        projects
                            .get(*project_id)
                            .map(|project| project.last_seen_at.as_str())
                            .unwrap_or("")
                    })
                    .cloned()
                    .context("session project alias references a missing project")?;
                candidates.remove(&target);
                for source in candidates {
                    merge_session_project_rows(&tx, &target, &source)?;
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
                        "INSERT INTO session_project_aliases (project_id, kind, value)
                         VALUES (?1, ?2, ?3)
                         ON CONFLICT(kind, value) DO UPDATE SET project_id = excluded.project_id",
                        params![project_id, alias.0, alias.1],
                    )?;
                    aliases.insert(alias, project_id.clone());
                }
            }

            let seen_at = session_project_seen_at(session);
            let suggested_name = suggested_project_name(session);
            if let Some(project) = projects.get_mut(&project_id) {
                if seen_at > project.last_seen_at {
                    project.last_seen_at = seen_at.clone();
                    if !project.name_custom {
                        project.name = suggested_name;
                    }
                    tx.execute(
                        "UPDATE session_projects
                         SET name = ?2, last_seen_at = ?3
                         WHERE id = ?1",
                        params![project_id, project.name, project.last_seen_at],
                    )?;
                }
                session.logical_project_id = Some(project_id);
                session.logical_project_name = Some(project.name.clone());
            }
        }

        tx.commit()?;
        Ok(())
    }

    pub fn refresh_session_analytics(
        &self,
        sessions: &[SessionRecord],
    ) -> Result<AnalyticsRefreshReport> {
        self.refresh_session_analytics_with_progress(sessions, |_| {})
    }

    pub fn refresh_session_analytics_with_progress<F>(
        &self,
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
                     FROM session_analytics
                     WHERE session_id = ?1 AND agent = ?2 AND session_path = ?3",
                )?;
                let mut cache_stmt = self.conn.prepare(
                    "SELECT file_mtime, file_size, analytics_json, parser_state_json
                     FROM session_analytics
                     WHERE session_id = ?1 AND agent = ?2 AND session_path = ?3",
                )?;

                for session in batch {
                    let cached_state = state_stmt
                        .query_row(
                            params![
                                session.id,
                                agent_label(session.agent),
                                session.path.display().to_string(),
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
                                    "invalid analytics cache row for {}: {err}",
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

            self.save_session_analytics_records(&updates)?;
            let completed = ((batch_index + 1) * SESSION_ANALYTICS_BATCH_SIZE).min(sessions.len());
            on_progress(analytics_refresh_progress(&report, completed));
        }

        let tx = self.conn.unchecked_transaction()?;
        cleanup_stale_session_analytics_rows(&tx)?;
        tx.commit()?;
        report.warnings = warnings;
        Ok(report)
    }

    fn save_session_analytics_records(&self, records: &[SessionAnalyticsRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        for record in records {
            let overview = record.analytics.overview_index(&record.state);
            tx.execute(
                "INSERT INTO session_analytics (
                    session_id, agent, session_path, file_mtime, file_size,
                    indexed_at, analytics_json, parser_state_json,
                    event_min_date, event_max_date, has_activity,
                    capability_token_usage, capability_reasoning_tokens,
                    capability_explicit_runs, capability_rate_limit_history,
                    overview_indexed, overview_index_error
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 1, NULL)
                 ON CONFLICT(session_id, agent, session_path) DO UPDATE SET
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
        }
        bump_analytics_revision(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn overview_analytics(
        &self,
        agent: Option<AgentKind>,
        days: u32,
        rank_days: u32,
    ) -> Result<OverviewAnalytics> {
        let today = chrono::Local::now().date_naive();
        let days = days.clamp(1, 365);
        let rank_days = rank_days.clamp(1, 730);
        let since = today - chrono::Duration::days(i64::from(days.saturating_sub(1)));
        let rank_since = today - chrono::Duration::days(i64::from(rank_days.saturating_sub(1)));
        let cutoff = since.min(rank_since).to_string();
        let (records, warnings) = self.load_session_analytics_records(agent, Some(&cutoff))?;
        let mut overview = analytics::aggregate_overview(&records, days, rank_days, warnings);
        let (coverage, capabilities) = self.analytics_overview_metadata(agent)?;
        overview.revision = self.analytics_revision()?;
        overview.coverage = coverage;
        overview.capabilities = capabilities;
        overview
            .warnings
            .extend(self.analytics_overview_index_warnings(agent)?);
        Ok(overview)
    }

    fn load_session_analytics_records(
        &self,
        agent: Option<AgentKind>,
        event_cutoff: Option<&str>,
    ) -> Result<(Vec<SessionAnalyticsRecord>, Vec<String>)> {
        let mut records = Vec::new();
        let mut warnings = Vec::new();
        let sql = if agent.is_some() {
            "SELECT file_mtime, file_size, analytics_json, parser_state_json
             FROM session_analytics
             WHERE agent = ?1
               AND overview_indexed = 1
               AND (?2 IS NULL OR event_max_date >= ?2)"
        } else {
            "SELECT file_mtime, file_size, analytics_json, parser_state_json
             FROM session_analytics
             WHERE ?1 IS NULL
               AND overview_indexed = 1
               AND (?2 IS NULL OR event_max_date >= ?2)"
        };
        let agent_value = agent.map(agent_label);
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![agent_value, event_cutoff], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (file_mtime, file_size, analytics_json, parser_state_json) = row?;
            let analytics =
                serde_json::from_str::<crate::analytics::SessionAnalytics>(&analytics_json);
            let state =
                serde_json::from_str::<crate::analytics::AnalyticsParserState>(&parser_state_json);
            match (analytics, state) {
                (Ok(analytics), Ok(state)) => records.push(SessionAnalyticsRecord {
                    analytics,
                    state,
                    file_mtime,
                    file_size,
                }),
                (Err(err), _) | (_, Err(err)) => {
                    warnings.push(format!("invalid analytics cache row: {err}"));
                }
            }
        }
        Ok((records, warnings))
    }

    fn analytics_overview_metadata(
        &self,
        agent: Option<AgentKind>,
    ) -> Result<(AnalyticsCoverage, Vec<AnalyticsProviderCapability>)> {
        let agent_value = agent.map(agent_label);
        let mut coverage = self.conn.query_row(
            "SELECT MIN(event_min_date), MAX(event_max_date), COUNT(*),
                    COALESCE(SUM(has_activity), 0)
             FROM session_analytics
             WHERE (?1 IS NULL OR agent = ?1) AND overview_indexed = 1",
            [agent_value],
            |row| {
                Ok(AnalyticsCoverage {
                    first: row.get(0)?,
                    last: row.get(1)?,
                    total_sessions: row.get::<_, i64>(2)? as usize,
                    analyzed_sessions: row.get::<_, i64>(3)? as usize,
                    indexing_sessions: 0,
                })
            },
        )?;
        coverage.total_sessions = self.conn.query_row(
            "SELECT COUNT(*) FROM session_analytics WHERE ?1 IS NULL OR agent = ?1",
            [agent_value],
            |row| row.get::<_, i64>(0),
        )? as usize;
        coverage.indexing_sessions = self.conn.query_row(
            "SELECT COUNT(*) FROM session_analytics
             WHERE overview_indexed = 0 AND (?1 IS NULL OR agent = ?1)",
            [agent_value],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let mut stmt = self.conn.prepare(
            "SELECT agent,
                    MAX(capability_token_usage),
                    MAX(capability_reasoning_tokens),
                    MAX(capability_explicit_runs),
                    MAX(capability_rate_limit_history)
             FROM session_analytics
             WHERE (?1 IS NULL OR agent = ?1) AND overview_indexed = 1
             GROUP BY agent",
        )?;
        let capabilities = stmt
            .query_map([agent_value], |row| {
                Ok(AnalyticsProviderCapability {
                    agent: parse_agent_label(&row.get::<_, String>(0)?),
                    capabilities: AnalyticsCapabilities {
                        token_usage: row.get(1)?,
                        reasoning_tokens: row.get(2)?,
                        explicit_runs: row.get(3)?,
                        rate_limit_history: row.get(4)?,
                    },
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok((coverage, capabilities))
    }

    pub fn analytics_revision(&self) -> Result<u64> {
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

    pub fn session_overview_metadata(
        &self,
        agent: Option<AgentKind>,
    ) -> Result<(usize, Vec<AgentKind>)> {
        let agent_value = agent.map(agent_label);
        let total = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE ?1 IS NULL OR agent = ?1",
            [agent_value],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT agent FROM sessions
             WHERE ?1 IS NULL OR agent = ?1
             ORDER BY agent",
        )?;
        let agents = stmt
            .query_map([agent_value], |row| {
                Ok(parse_agent_label(&row.get::<_, String>(0)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok((total, agents))
    }

    fn analytics_overview_index_warnings(&self, agent: Option<AgentKind>) -> Result<Vec<String>> {
        let agent_value = agent.map(agent_label);
        let mut stmt = self.conn.prepare(
            "SELECT session_path, overview_index_error
             FROM session_analytics
             WHERE overview_index_error IS NOT NULL
               AND (?1 IS NULL OR agent = ?1)",
        )?;
        stmt.query_map([agent_value], |row| {
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

    pub fn session_scan_cache(&self) -> Result<SessionScanCache> {
        let mut stmt = self.conn.prepare(
            "SELECT sessions.data_json,
                    sessions.first_user_message,
                    sessions.last_user_message,
                    sessions.last_assistant_message,
                    search.file_mtime,
                    search.file_size
             FROM sessions
             JOIN session_search_index AS search
               ON search.session_id = sessions.id
              AND search.agent = sessions.agent
              AND search.session_path = sessions.path",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;
        let entries = rows.filter_map(Result::ok).filter_map(
            |(
                data_json,
                first_user_message,
                last_user_message,
                last_assistant_message,
                file_mtime,
                file_size,
            )| {
                serde_json::from_str::<SessionRecord>(&data_json)
                    .ok()
                    .map(|mut session| {
                        Self::apply_session_preview_columns(
                            &mut session,
                            first_user_message,
                            last_user_message,
                            last_assistant_message,
                        );
                        SessionScanCacheEntry {
                            session,
                            file_mtime,
                            file_size,
                        }
                    })
            },
        );
        Ok(SessionScanCache::from_entries(entries))
    }

    pub fn sessions_last_scan_at(&self) -> Result<Option<u64>> {
        self.conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'sessions_last_scan_at'",
                [],
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

    pub fn apply_session_delta(&self, sessions: &[SessionRecord]) -> Result<Vec<SessionRecord>> {
        let tx = self.conn.unchecked_transaction()?;
        let mut changed = Vec::new();
        for session in sessions {
            let agent = agent_label(session.agent);
            let existing_session = tx
                .query_row(
                    "SELECT data_json FROM sessions WHERE id = ?1 AND agent = ?2 LIMIT 1",
                    params![session.id, agent],
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
            let first_user_message = bound_session_preview(canonical.first_user_message.clone());
            let last_user_message = bound_session_preview(canonical.last_user_message.clone());
            let last_assistant_message =
                bound_session_preview(canonical.last_assistant_message.clone());
            let current = tx
                .query_row(
                    "SELECT data_json, first_user_message, last_user_message, last_assistant_message
                     FROM sessions WHERE id = ?1 AND agent = ?2 AND path = ?3",
                    params![canonical.id, agent, path],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .optional()?;
            if current.as_ref().is_some_and(
                |(current_json, current_first, current_last, current_assistant)| {
                    current_json == &data_json
                        && current_first == &first_user_message
                        && current_last == &last_user_message
                        && current_assistant == &last_assistant_message
                },
            ) {
                continue;
            }
            tx.execute(
                "DELETE FROM sessions WHERE id = ?1 AND agent = ?2 AND path <> ?3",
                params![canonical.id, agent, path],
            )?;
            tx.execute(
                "INSERT INTO sessions (id, agent, title, project, path, started_at, updated_at, message_count, first_user_message, last_user_message, last_assistant_message, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id, agent, path) DO UPDATE SET
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
                    canonical.id,
                    agent,
                    canonical.title,
                    canonical
                        .project
                        .as_ref()
                        .map(|path| path.display().to_string()),
                    path,
                    canonical.started_at,
                    canonical.updated_at,
                    canonical.message_count.map(|value| value as i64),
                    first_user_message,
                    last_user_message,
                    last_assistant_message,
                    data_json,
                ],
            )?;
            changed.push(canonical);
        }
        cleanup_stale_session_skill_rows(&tx)?;
        cleanup_stale_session_search_rows(&tx)?;
        cleanup_stale_session_analytics_rows(&tx)?;
        tx.commit()?;
        Ok(changed)
    }

    pub fn index_session_delta(&self, sessions: &[SessionRecord]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for session in sessions {
            index_session_search_document_best_effort(&tx, session);
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove_sessions_for_paths(
        &self,
        paths: &[PathBuf],
    ) -> Result<Vec<crate::sessions::SessionIdentity>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let existing = self.list_sessions()?.sessions;
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
                "DELETE FROM sessions WHERE id = ?1 AND agent = ?2 AND path = ?3",
                params![
                    session.id,
                    agent_label(session.agent),
                    session.path.display().to_string(),
                ],
            )?;
        }
        cleanup_stale_session_skill_rows(&tx)?;
        cleanup_stale_session_search_rows(&tx)?;
        cleanup_stale_session_analytics_rows(&tx)?;
        tx.commit()?;
        Ok(removed
            .iter()
            .map(crate::sessions::SessionIdentity::from)
            .collect())
    }

    pub fn search_sessions(&self, query: &str) -> Result<Vec<SessionSearchHit>> {
        let terms = session_search_terms(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        if terms.iter().any(|term| term.chars().count() < 3) {
            return self.search_sessions_by_contains(&terms);
        }
        let query = session_search_query(&terms);

        let mut stmt = self.conn.prepare(
            "SELECT
                sessions.data_json,
                sessions.first_user_message,
                sessions.last_user_message,
                sessions.last_assistant_message,
                bm25(
                    session_search_fts,
                    0.5, 10.0, 5.0, 6.0, 3.0
                ) AS score,
                CASE
                    WHEN instr(highlight(session_search_fts, 1, '⟦', '⟧'), '⟦') > 0
                        THEN highlight(session_search_fts, 1, '⟦', '⟧')
                    WHEN instr(snippet(session_search_fts, 3, '⟦', '⟧', ' … ', 32), '⟦') > 0
                        THEN snippet(session_search_fts, 3, '⟦', '⟧', ' … ', 32)
                    WHEN instr(snippet(session_search_fts, 4, '⟦', '⟧', ' … ', 32), '⟦') > 0
                        THEN snippet(session_search_fts, 4, '⟦', '⟧', ' … ', 32)
                    WHEN instr(highlight(session_search_fts, 2, '⟦', '⟧'), '⟦') > 0
                        THEN highlight(session_search_fts, 2, '⟦', '⟧')
                    ELSE highlight(session_search_fts, 0, '⟦', '⟧')
                END AS snippet
             FROM session_search_fts
             JOIN session_search_records AS search
               ON search.rowid = session_search_fts.rowid
             JOIN sessions
               ON sessions.id = search.session_id
              AND sessions.agent = search.agent
              AND sessions.path = search.session_path
             WHERE session_search_fts MATCH ?1
             ORDER BY score ASC, sessions.updated_at DESC, sessions.id ASC",
        )?;
        let rows = stmt.query_map([query], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut hits = Vec::new();
        let mut seen = HashSet::new();
        for row in rows {
            let (
                data_json,
                first_user_message,
                last_user_message,
                last_assistant_message,
                raw_score,
                snippet,
            ) = row?;
            let mut session = serde_json::from_str::<SessionRecord>(&data_json)
                .context("invalid cached session search row")?;
            Self::apply_session_preview_columns(
                &mut session,
                first_user_message,
                last_user_message,
                last_assistant_message,
            );
            let key = (
                session.id.clone(),
                agent_label(session.agent),
                session.path.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            hits.push(SessionSearchHit {
                session,
                search_score: -raw_score,
                search_snippet: compact_search_snippet(&snippet),
            });
        }
        Ok(hits)
    }

    fn search_sessions_by_contains(&self, terms: &[String]) -> Result<Vec<SessionSearchHit>> {
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
        let sql = format!(
            "SELECT
                sessions.data_json,
                sessions.first_user_message,
                sessions.last_user_message,
                sessions.last_assistant_message,
                search.metadata_text,
                search.title,
                search.project,
                search.user_text,
                search.assistant_text
             FROM session_search_records AS search
             JOIN sessions
               ON sessions.id = search.session_id
              AND sessions.agent = search.agent
              AND sessions.path = search.session_path
             WHERE {where_clause}",
        );
        let patterns = terms
            .iter()
            .map(|term| format!("%{}%", escape_like(term)))
            .collect::<Vec<_>>();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(patterns.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                SessionSearchDocument {
                    metadata_text: row.get(4)?,
                    title: row.get(5)?,
                    project: row.get(6)?,
                    user_text: row.get(7)?,
                    assistant_text: row.get(8)?,
                },
            ))
        })?;
        let mut hits: Vec<SessionSearchHit> = Vec::new();
        let mut hit_indexes: HashMap<(String, &'static str, PathBuf), usize> = HashMap::new();
        for row in rows {
            let (
                data_json,
                first_user_message,
                last_user_message,
                last_assistant_message,
                document,
            ) = row?;
            let mut session = serde_json::from_str::<SessionRecord>(&data_json)
                .context("invalid cached session search row")?;
            Self::apply_session_preview_columns(
                &mut session,
                first_user_message,
                last_user_message,
                last_assistant_message,
            );
            let key = (
                session.id.clone(),
                agent_label(session.agent),
                session.path.clone(),
            );
            let hit = SessionSearchHit {
                search_score: contains_search_score(&document, terms),
                search_snippet: contains_search_snippet(&session, &document, terms),
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
        hits.sort_by(|left, right| {
            right
                .search_score
                .total_cmp(&left.search_score)
                .then_with(|| {
                    right
                        .session
                        .updated_at
                        .cmp(&left.session.updated_at)
                        .then_with(|| left.session.id.cmp(&right.session.id))
                })
        });
        Ok(hits)
    }

    pub fn save_sessions(&self, sessions: &SessionScan) -> Result<()> {
        self.save_sessions_at(sessions, unix_now())
    }

    pub fn save_sessions_at(&self, sessions: &SessionScan, scanned_at: u64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
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
                "INSERT INTO sessions (id, agent, title, project, path, started_at, updated_at, message_count, first_user_message, last_user_message, last_assistant_message, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id, agent, path) DO UPDATE SET
                    title = excluded.title,
                    project = excluded.project,
                    started_at = excluded.started_at,
                    updated_at = excluded.updated_at,
                    message_count = excluded.message_count,
                    first_user_message = excluded.first_user_message,
                    last_user_message = excluded.last_user_message,
                    last_assistant_message = excluded.last_assistant_message,
                    data_json = excluded.data_json
                 WHERE sessions.data_json IS NOT excluded.data_json
                    OR sessions.first_user_message IS NOT excluded.first_user_message
                    OR sessions.last_user_message IS NOT excluded.last_user_message
                    OR sessions.last_assistant_message IS NOT excluded.last_assistant_message",
                params![
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
            index_session_search_document_best_effort(&tx, session);
        }

        if sessions.warnings.is_empty() {
            tx.execute(
                "DELETE FROM sessions
                 WHERE NOT EXISTS (
                    SELECT 1 FROM current_sessions
                    WHERE current_sessions.id = sessions.id
                      AND current_sessions.agent = sessions.agent
                      AND current_sessions.path = sessions.path
                 )",
                [],
            )?;
            cleanup_stale_session_skill_rows(&tx)?;
            cleanup_stale_session_search_rows(&tx)?;
            cleanup_stale_session_analytics_rows(&tx)?;
        }
        tx.execute("DELETE FROM current_sessions", [])?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('sessions_last_scan_at', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![scanned_at.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn clear_session_skill_index(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            DELETE FROM session_skill_links;
            DELETE FROM session_skill_index;
            ",
        )?;
        Ok(())
    }

    pub fn ensure_session_skill_index_version(&self, version: &str) -> Result<bool> {
        let current = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'session_skill_index_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if current.as_deref() == Some(version) {
            return Ok(false);
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(
            "
            DELETE FROM session_skill_links;
            DELETE FROM session_skill_index;
            ",
        )?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('session_skill_index_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [version],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn session_skill_index_is_current(
        &self,
        session: &SessionRecord,
        file_mtime: i64,
        file_size: i64,
    ) -> Result<bool> {
        let status = self
            .conn
            .query_row(
                "SELECT status FROM session_skill_index
                 WHERE session_id = ?1 AND agent = ?2 AND session_path = ?3
                   AND file_mtime = ?4 AND file_size = ?5",
                params![
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

    pub fn replace_session_skill_links(
        &self,
        session: &SessionRecord,
        state: &SessionFileState,
        links: &[SessionSkillLink],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let agent = agent_label(session.agent);
        let session_path = session.path.display().to_string();
        tx.execute(
            "DELETE FROM session_skill_links
             WHERE session_id = ?1 AND agent = ?2 AND session_path = ?3",
            params![session.id, agent, session_path],
        )?;
        for link in links {
            tx.execute(
                "INSERT INTO session_skill_links (
                    session_id, agent, session_path, skill_name, skill_path,
                    skill_agent, skill_scope, evidence_kind, evidence_text,
                    evidence_time, confidence
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
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
            "INSERT INTO session_skill_index (
                session_id, agent, session_path, file_mtime, file_size,
                indexed_at, status, error
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'indexed', NULL)
             ON CONFLICT(session_id, agent, session_path) DO UPDATE SET
                file_mtime = excluded.file_mtime,
                file_size = excluded.file_size,
                indexed_at = excluded.indexed_at,
                status = excluded.status,
                error = NULL",
            params![
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

    pub fn mark_session_skill_index_failed(
        &self,
        session: &SessionRecord,
        file_mtime: i64,
        file_size: i64,
        error: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO session_skill_index (
                session_id, agent, session_path, file_mtime, file_size,
                indexed_at, status, error
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'failed', ?7)
             ON CONFLICT(session_id, agent, session_path) DO UPDATE SET
                file_mtime = excluded.file_mtime,
                file_size = excluded.file_size,
                indexed_at = excluded.indexed_at,
                status = excluded.status,
                error = excluded.error",
            params![
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

    pub fn session_skill_index_status(&self, running: bool) -> Result<SessionSkillIndexStatus> {
        let total = self
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| {
                row.get::<_, i64>(0)
            })?
            .max(0) as usize;
        let indexed = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_skill_index WHERE status = 'indexed'",
                [],
                |row| row.get::<_, i64>(0),
            )?
            .max(0) as usize;
        let failed = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_skill_index WHERE status = 'failed'",
                [],
                |row| row.get::<_, i64>(0),
            )?
            .max(0) as usize;
        let last_indexed_at = self.conn.query_row(
            "SELECT MAX(indexed_at) FROM session_skill_index WHERE status = 'indexed'",
            [],
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
    }

    pub fn session_skill_links(
        &self,
        session_id: &str,
        agent: AgentKind,
    ) -> Result<Vec<SessionSkillLink>> {
        self.query_session_skill_links(
            "WHERE links.session_id = ?1 AND links.agent = ?2",
            params![session_id, agent_label(agent)],
        )
    }

    pub fn skill_session_links(&self, skill_name: &str) -> Result<Vec<SessionSkillLink>> {
        self.query_session_skill_links("WHERE links.skill_name = ?1", params![skill_name])
    }

    fn query_session_skill_links<P>(
        &self,
        where_clause: &str,
        params: P,
    ) -> Result<Vec<SessionSkillLink>>
    where
        P: rusqlite::Params,
    {
        let sql = format!(
            "SELECT
                links.session_id,
                links.agent,
                links.session_path,
                sessions.title,
                sessions.project,
                sessions.updated_at,
                sessions.started_at,
                sessions.message_count,
                links.skill_name,
                links.skill_path,
                links.skill_agent,
                links.skill_scope,
                links.evidence_kind,
                links.evidence_text,
                links.evidence_time,
                links.confidence
             FROM session_skill_links links
             LEFT JOIN sessions
               ON sessions.id = links.session_id
              AND sessions.agent = links.agent
              AND sessions.path = links.session_path
             {where_clause}
             ORDER BY sessions.updated_at DESC, links.evidence_time DESC, links.skill_name ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params, |row| {
            Ok(SessionSkillLink {
                session_id: row.get(0)?,
                agent: parse_agent_label(&row.get::<_, String>(1)?),
                session_path: PathBuf::from(row.get::<_, String>(2)?),
                session_title: clean_session_title(row.get(3)?),
                session_project: row.get::<_, Option<String>>(4)?.map(PathBuf::from),
                session_updated_at: row.get(5)?,
                session_started_at: row.get(6)?,
                session_message_count: row
                    .get::<_, Option<i64>>(7)?
                    .map(|value| value.max(0) as usize),
                skill_name: row.get(8)?,
                skill_path: PathBuf::from(row.get::<_, String>(9)?),
                skill_agent: row
                    .get::<_, Option<String>>(10)?
                    .map(|agent| parse_agent_label(&agent)),
                skill_scope: row.get(11)?,
                evidence_kind: row.get(12)?,
                evidence_text: row.get(13)?,
                evidence_time: row.get(14)?,
                confidence: row.get(15)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn skill_source_records(&self) -> Result<Vec<SkillSourceRecord>> {
        let mut statement = self
            .conn
            .prepare("SELECT data_json FROM skill_sources ORDER BY skill_name, skill_path")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
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
        let mut statement = self.conn.prepare(
            "SELECT source_version, relative_path, content
             FROM skill_snapshots
             WHERE skill_path = ?1
             ORDER BY relative_path",
        )?;
        let mut rows = statement.query(params![skill_path.display().to_string()])?;
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

    pub fn save_skills(&self, skills: &SkillScan) -> Result<()> {
        if !skills.warnings.is_empty() {
            return Ok(());
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(
            "
            DELETE FROM skills;
            DELETE FROM skill_paths;
            ",
        )?;

        for skill in &skills.skills {
            tx.execute(
                "INSERT INTO skills (name, visibility, agents_json, description, is_system, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    skill.name,
                    format!("{:?}", skill.visibility).to_lowercase(),
                    serde_json::to_string(&skill.agents)?,
                    skill.description,
                    skill.is_system,
                    serde_json::to_string(skill)?,
                ],
            )?;
            for path in &skill.paths {
                tx.execute(
                    "INSERT INTO skill_paths (skill_name, path, root, scope, agent, sha256, data_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(skill_name, path) DO UPDATE SET
                        root = excluded.root,
                        scope = excluded.scope,
                        agent = excluded.agent,
                        sha256 = excluded.sha256,
                        data_json = excluded.data_json",
                    params![
                        skill.name,
                        path.path.display().to_string(),
                        path.root.display().to_string(),
                        path.scope,
                        agent_label(path.agent),
                        path.sha256,
                        serde_json::to_string(path)?,
                    ],
                )?;
            }
        }

        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('skills_last_scan_at', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![unix_now().to_string()],
        )?;
        cleanup_stale_skill_link_rows(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_skills(&self, names: &[String]) -> Result<()> {
        if names.is_empty() {
            return Ok(());
        }

        let tx = self.conn.unchecked_transaction()?;
        for name in names {
            tx.execute("DELETE FROM skills WHERE name = ?1", params![name])?;
            tx.execute(
                "DELETE FROM skill_paths WHERE skill_name = ?1",
                params![name],
            )?;
        }
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('skills_last_scan_at', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![unix_now().to_string()],
        )?;
        cleanup_stale_skill_link_rows(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn save_skill_delta(&self, upserts: &[SkillRecord], removed: &[String]) -> Result<()> {
        if upserts.is_empty() && removed.is_empty() {
            return Ok(());
        }

        let tx = self.conn.unchecked_transaction()?;
        for name in removed {
            tx.execute("DELETE FROM skills WHERE name = ?1", params![name])?;
            tx.execute(
                "DELETE FROM skill_paths WHERE skill_name = ?1",
                params![name],
            )?;
        }
        for skill in upserts {
            tx.execute(
                "INSERT INTO skills (name, visibility, agents_json, description, is_system, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(name) DO UPDATE SET
                   visibility = excluded.visibility,
                   agents_json = excluded.agents_json,
                   description = excluded.description,
                   is_system = excluded.is_system,
                   data_json = excluded.data_json",
                params![
                    skill.name,
                    format!("{:?}", skill.visibility).to_lowercase(),
                    serde_json::to_string(&skill.agents)?,
                    skill.description,
                    skill.is_system,
                    serde_json::to_string(skill)?,
                ],
            )?;
            tx.execute(
                "DELETE FROM skill_paths WHERE skill_name = ?1",
                params![skill.name],
            )?;
            for path in &skill.paths {
                tx.execute(
                    "INSERT INTO skill_paths (skill_name, path, root, scope, agent, sha256, data_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(skill_name, path) DO UPDATE SET
                        root = excluded.root,
                        scope = excluded.scope,
                        agent = excluded.agent,
                        sha256 = excluded.sha256,
                        data_json = excluded.data_json",
                    params![
                        skill.name,
                        path.path.display().to_string(),
                        path.root.display().to_string(),
                        path.scope,
                        agent_label(path.agent),
                        path.sha256,
                        serde_json::to_string(path)?,
                    ],
                )?;
            }
        }
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('skills_last_scan_at', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![unix_now().to_string()],
        )?;
        cleanup_stale_skill_link_rows(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn list_prompts(&self) -> Result<Vec<PromptRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, category, tags_json, body, created_at, updated_at
             FROM prompts
             ORDER BY updated_at DESC, title ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let category = row.get::<_, String>(2)?;
            let tags_json = row.get::<_, String>(3)?;
            Ok(PromptRecord {
                id: row.get(0)?,
                title: row.get(1)?,
                tags: parse_prompt_tags(&tags_json, &category),
                body: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
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
            .unwrap_or_else(|_| now.clone());

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
                name TEXT PRIMARY KEY,
                visibility TEXT NOT NULL,
                agents_json TEXT NOT NULL,
                description TEXT,
                is_system INTEGER NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS agents (
                kind TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                installed INTEGER NOT NULL,
                config_dir TEXT,
                executable TEXT,
                version TEXT,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS skill_paths (
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
                skill_path TEXT NOT NULL,
                source_version TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                content BLOB NOT NULL,
                PRIMARY KEY (skill_path, relative_path)
            );
            CREATE INDEX IF NOT EXISTS idx_skill_sources_name
                ON skill_sources(skill_name);
            CREATE TABLE IF NOT EXISTS fs_manifest (
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
                PRIMARY KEY (source_kind, path)
            );
            CREATE INDEX IF NOT EXISTS idx_fs_manifest_root_kind_path
                ON fs_manifest(root, source_kind, path);
            CREATE TABLE IF NOT EXISTS projection_contexts (
                domain TEXT PRIMARY KEY,
                workspace_root TEXT NOT NULL,
                state TEXT NOT NULL,
                scanned_at INTEGER,
                error TEXT,
                parser_version TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_projection_contexts_workspace
                ON projection_contexts(workspace_root, domain);
            CREATE TABLE IF NOT EXISTS projection_refresh_lock (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                owner TEXT NOT NULL,
                started_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS projection_refresh_locks (
                domain TEXT PRIMARY KEY,
                owner TEXT NOT NULL,
                started_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS sessions (
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
            CREATE TABLE IF NOT EXISTS session_projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                name_custom INTEGER NOT NULL DEFAULT 0,
                last_seen_at TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS session_project_aliases (
                project_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY (kind, value)
            );
            CREATE INDEX IF NOT EXISTS idx_session_project_aliases_project
                ON session_project_aliases(project_id);
            CREATE TABLE IF NOT EXISTS session_skill_index (
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
            CREATE TABLE IF NOT EXISTS session_search_index (
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                file_mtime INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                indexed_at TEXT NOT NULL,
                search_metadata TEXT NOT NULL DEFAULT '',
                PRIMARY KEY (session_id, agent, session_path)
            );
            CREATE TABLE IF NOT EXISTS session_analytics (
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
            CREATE TABLE IF NOT EXISTS session_search_records (
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
            CREATE INDEX IF NOT EXISTS idx_session_skill_links_session
                ON session_skill_links(session_id, agent);
            CREATE INDEX IF NOT EXISTS idx_session_skill_links_skill
                ON session_skill_links(skill_name);
            CREATE INDEX IF NOT EXISTS idx_session_skill_links_path
                ON session_skill_links(skill_path);
            CREATE INDEX IF NOT EXISTS idx_session_search_index_session
                ON session_search_index(session_id, agent);
            CREATE INDEX IF NOT EXISTS idx_session_search_records_session
                ON session_search_records(session_id, agent, session_path);
            CREATE TABLE IF NOT EXISTS rules (
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
        self.ensure_prompt_tags_column()?;
        self.ensure_skill_source_ref_column()?;
        self.ensure_session_preview_columns()?;
        self.ensure_session_analytics_overview_columns()?;
        self.ensure_storage_indexes()?;
        self.ensure_session_search_fts()?;
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
             ON session_analytics(agent, event_max_date);",
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

fn load_session_projects(tx: &Transaction<'_>) -> Result<HashMap<String, ProjectState>> {
    let mut stmt =
        tx.prepare("SELECT id, name, name_custom, last_seen_at FROM session_projects")?;
    let rows = stmt.query_map([], |row| {
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
) -> Result<HashMap<SessionProjectAlias, String>> {
    let mut stmt = tx.prepare("SELECT kind, value, project_id FROM session_project_aliases")?;
    let rows = stmt.query_map([], |row| Ok(((row.get(0)?, row.get(1)?), row.get(2)?)))?;
    rows.collect::<std::result::Result<HashMap<_, _>, _>>()
        .map_err(Into::into)
}

fn merge_session_project_rows(
    tx: &Transaction<'_>,
    target_project_id: &str,
    source_project_id: &str,
) -> Result<()> {
    if target_project_id == source_project_id {
        return Ok(());
    }
    tx.execute(
        "UPDATE session_project_aliases SET project_id = ?1 WHERE project_id = ?2",
        params![target_project_id, source_project_id],
    )?;
    tx.execute(
        "DELETE FROM session_projects WHERE id = ?1",
        [source_project_id],
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

pub(crate) fn cursor_codex_worktree_alias(path: &str) -> Option<String> {
    let home = dirs::home_dir()?.to_string_lossy().to_string();
    let prefix = format!("{home}/codex/worktrees/");
    path.strip_prefix(&prefix)
        .map(|suffix| format!("{home}/.codex/worktrees/{suffix}"))
}

fn suggested_project_name(session: &SessionRecord) -> String {
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
        .unwrap_or_else(|| "Unknown".to_string())
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

fn primary_rule_agent(rule: &RuleRecord) -> AgentKind {
    rule.agents.first().copied().unwrap_or(AgentKind::Unknown)
}

fn parse_agent_label(value: &str) -> AgentKind {
    crate::providers::parse_agent(value).unwrap_or(AgentKind::Unknown)
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

fn fs_manifest_entries_from_scan(report: &ScanReport) -> Vec<FsManifestEntry> {
    let last_seen_at = unix_now() as i64;
    let mut entries = Vec::new();

    let mut add = |source_kind: &str,
                   path: &Path,
                   root: &Path,
                   agent: Option<String>,
                   scope: Option<String>,
                   sha256: Option<String>| {
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };
        let mtime_ns = metadata.modified().ok().and_then(|mtime| {
            let duration = mtime.duration_since(UNIX_EPOCH).ok()?;
            i64::try_from(duration.as_nanos())
                .ok()
                .or_else(|| i64::try_from(duration.as_millis()).ok())
                .or_else(|| i64::try_from(duration.as_secs()).ok())
        });
        entries.push(FsManifestEntry {
            source_kind: source_kind.to_string(),
            path: path.to_path_buf(),
            root: root.to_path_buf(),
            agent,
            scope,
            mtime_ns,
            size: i64::try_from(metadata.len()).ok(),
            inode: None,
            device: None,
            sha256,
            parser_version: PROJECTION_PARSER_VERSION.to_string(),
            last_seen_at,
            parse_status: "ok".to_string(),
        });
    };

    for session in &report.sessions.sessions {
        add(
            "session",
            &session.path,
            session.path.parent().unwrap_or(Path::new(".")),
            Some(agent_label(session.agent).to_string()),
            None,
            None,
        );
    }
    for skill in &report.skills.skills {
        for path in &skill.paths {
            add(
                "skill",
                &path.path.join("SKILL.md"),
                &path.root,
                Some(agent_label(path.agent).to_string()),
                Some(path.scope.clone()),
                (!path.sha256.is_empty()).then(|| path.sha256.clone()),
            );
        }
    }
    for rule in &report.rules.rules {
        add(
            "rule",
            &rule.path,
            rule.path.parent().unwrap_or(Path::new(".")),
            Some(agent_label(primary_rule_agent(rule)).to_string()),
            Some(rule.scope.clone()),
            (!rule.sha256.is_empty()).then(|| rule.sha256.clone()),
        );
    }
    for hook in &report.hooks.hooks {
        add(
            "hook",
            &hook.path,
            hook.path.parent().unwrap_or(Path::new(".")),
            Some(agent_label(hook.agent).to_string()),
            None,
            (!hook.trust_hash.is_empty()).then(|| hook.trust_hash.clone()),
        );
    }
    for server in &report.mcp.servers {
        add(
            "mcp",
            &server.path,
            server.path.parent().unwrap_or(Path::new(".")),
            Some(agent_label(server.agent).to_string()),
            Some(server.scope.clone()),
            None,
        );
    }
    entries
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
        for directory in [".codex", ".cursor", ".claude", ".agents"] {
            candidates.insert(ancestor.join(directory));
        }
    }
    if let Some(home) = dirs::home_dir() {
        for directory in [".codex", ".cursor", ".claude", ".agents"] {
            candidates.insert(home.join(directory));
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
            || path.file_name().is_some_and(|name| {
                matches!(
                    name.to_str(),
                    Some("AGENTS.md")
                        | Some("CLAUDE.md")
                        | Some(".cursorrules")
                        | Some("hooks.json")
                        | Some("settings.json")
                        | Some("config.toml")
                        | Some("mcp.json")
                        | Some("cli-config.json")
                )
            })
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
    match domain {
        "rules" => {
            for ancestor in ancestors {
                paths.extend([
                    ancestor.join("AGENTS.md"),
                    ancestor.join("CLAUDE.md"),
                    ancestor.join(".cursorrules"),
                    ancestor.join(".github/copilot-instructions.md"),
                ]);
            }
        }
        "hooks" => {
            for ancestor in ancestors {
                paths.extend([
                    ancestor.join(".codex/hooks.json"),
                    ancestor.join(".codex/config.toml"),
                    ancestor.join(".cursor/hooks.json"),
                    ancestor.join(".claude/settings.json"),
                    ancestor.join(".claude/settings.local.json"),
                ]);
            }
        }
        "mcp" => {
            for ancestor in ancestors {
                paths.extend([
                    ancestor.join(".mcp.json"),
                    ancestor.join(".cursor/mcp.json"),
                    ancestor.join(".cursor/cli-config.json"),
                    ancestor.join(".codex/mcp.json"),
                    ancestor.join(".codex/config.toml"),
                ]);
            }
        }
        "skills" => {
            for ancestor in ancestors {
                paths.extend([
                    ancestor.join(".agents/skills"),
                    ancestor.join(".claude/skills"),
                    ancestor.join(".cursor/skills"),
                    ancestor.join(".codex/skills"),
                ]);
            }
        }
        "agents" => {}
        _ => {}
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
        entries.push(manifest_entry_for_path(
            "rule",
            &rule.path,
            rule.path.parent().unwrap_or(workspace_root),
            Some(agent_label(primary_rule_agent(rule)).to_string()),
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

fn cleanup_stale_session_skill_rows(conn: &rusqlite::Transaction<'_>) -> Result<()> {
    conn.execute(
        "DELETE FROM session_skill_links
         WHERE NOT EXISTS (
            SELECT 1 FROM sessions
            WHERE sessions.id = session_skill_links.session_id
              AND sessions.agent = session_skill_links.agent
              AND sessions.path = session_skill_links.session_path
         )",
        [],
    )?;
    conn.execute(
        "DELETE FROM session_skill_index
         WHERE NOT EXISTS (
            SELECT 1 FROM sessions
            WHERE sessions.id = session_skill_index.session_id
              AND sessions.agent = session_skill_index.agent
              AND sessions.path = session_skill_index.session_path
         )",
        [],
    )?;
    Ok(())
}

fn cleanup_stale_session_search_rows(conn: &rusqlite::Transaction<'_>) -> Result<()> {
    conn.execute(
        "DELETE FROM session_search_records
         WHERE NOT EXISTS (
            SELECT 1 FROM sessions
            WHERE sessions.id = session_search_records.session_id
              AND sessions.agent = session_search_records.agent
              AND sessions.path = session_search_records.session_path
         )",
        [],
    )?;
    conn.execute(
        "DELETE FROM session_search_index
         WHERE NOT EXISTS (
            SELECT 1 FROM sessions
            WHERE sessions.id = session_search_index.session_id
              AND sessions.agent = session_search_index.agent
              AND sessions.path = session_search_index.session_path
         )",
        [],
    )?;
    Ok(())
}

fn cleanup_stale_session_analytics_rows(conn: &rusqlite::Transaction<'_>) -> Result<()> {
    let deleted = conn.execute(
        "DELETE FROM session_analytics
         WHERE NOT EXISTS (
            SELECT 1 FROM sessions
            WHERE sessions.id = session_analytics.session_id
              AND sessions.agent = session_analytics.agent
              AND sessions.path = session_analytics.session_path
         )",
        [],
    )?;
    if deleted > 0 {
        bump_analytics_revision(conn)?;
    }
    Ok(())
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

fn cleanup_stale_skill_link_rows(conn: &rusqlite::Transaction<'_>) -> Result<()> {
    conn.execute(
        "DELETE FROM session_skill_links
         WHERE NOT EXISTS (
            SELECT 1 FROM skill_paths
            WHERE skill_paths.skill_name = session_skill_links.skill_name
              AND skill_paths.path = session_skill_links.skill_path
         )",
        [],
    )?;
    Ok(())
}

fn index_session_search_document(tx: &Transaction<'_>, session: &SessionRecord) -> Result<()> {
    let agent = agent_label(session.agent);
    let session_path = session.path.display().to_string();
    let search_metadata = session_search_metadata(session);
    let state = match crate::session_skills::session_file_state(&session.path) {
        Ok(state) => state,
        Err(_) => {
            tx.execute(
                "DELETE FROM session_search_records
                 WHERE session_id = ?1 AND agent = ?2 AND session_path = ?3",
                params![session.id, agent, session_path],
            )?;
            tx.execute(
                "DELETE FROM session_search_index
                 WHERE session_id = ?1 AND agent = ?2 AND session_path = ?3",
                params![session.id, agent, session_path],
            )?;
            return Ok(());
        }
    };

    let current = tx
        .query_row(
            "SELECT
                file_mtime,
                file_size,
                search_metadata
             FROM session_search_index
             WHERE session_id = ?1 AND agent = ?2 AND session_path = ?3",
            params![session.id, agent, session_path],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    if current.as_ref().is_some_and(|current| {
        current.0 == state.file_mtime
            && current.1 == state.file_size
            && current.2 == search_metadata
    }) {
        return Ok(());
    }

    if current
        .as_ref()
        .is_some_and(|current| current.0 == state.file_mtime && current.1 == state.file_size)
    {
        let document = session_search_metadata_document(session);
        tx.execute(
            "UPDATE session_search_records
             SET metadata_text = ?4, title = ?5, project = ?6
             WHERE session_id = ?1
               AND agent = ?2
               AND session_path = ?3
               AND record_order = 0",
            params![
                session.id,
                agent,
                session_path,
                document.metadata_text,
                document.title,
                document.project,
            ],
        )?;
        tx.execute(
            "UPDATE session_search_index
             SET indexed_at = ?4, search_metadata = ?5
             WHERE session_id = ?1 AND agent = ?2 AND session_path = ?3",
            params![
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
        "DELETE FROM session_search_records
         WHERE session_id = ?1 AND agent = ?2 AND session_path = ?3",
        params![session.id, agent, session_path],
    )?;
    insert_session_search_record(tx, session, 0, &session_search_metadata_document(session))?;

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
        truncate_to_char_boundary(&mut document.user_text, SESSION_SEARCH_RECORD_TEXT_LIMIT);
        truncate_to_char_boundary(
            &mut document.assistant_text,
            SESSION_SEARCH_RECORD_TEXT_LIMIT,
        );
        if let Err(error) = insert_session_search_record(tx, session, record_order, &document) {
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
            "DELETE FROM session_search_records
             WHERE session_id = ?1 AND agent = ?2 AND session_path = ?3 AND record_order > 0",
            params![session.id, agent, session_path],
        )?;
    }
    tx.execute(
        "INSERT INTO session_search_index (
            session_id, agent, session_path, file_mtime, file_size, indexed_at,
            search_metadata
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(session_id, agent, session_path) DO UPDATE SET
            file_mtime = excluded.file_mtime,
            file_size = excluded.file_size,
            indexed_at = excluded.indexed_at,
            search_metadata = excluded.search_metadata",
        params![
            session.id,
            agent,
            session_path,
            state.file_mtime,
            state.file_size,
            unix_now().to_string(),
            search_metadata,
        ],
    )?;
    Ok(())
}

fn insert_session_search_record(
    tx: &Transaction<'_>,
    session: &SessionRecord,
    record_order: usize,
    document: &SessionSearchDocument,
) -> Result<()> {
    tx.execute(
        "INSERT INTO session_search_records (
            session_id, agent, session_path, record_order,
            metadata_text, title, project, user_text, assistant_text
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
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

fn index_session_search_document_best_effort(tx: &Transaction<'_>, session: &SessionRecord) {
    if let Err(err) = index_session_search_document(tx, session) {
        crate::logging::global().warn(
            "failed to index session search text",
            serde_json::json!({
                "agent": agent_label(session.agent),
                "session_id": session.id,
                "error": format!("{err:#}"),
            }),
        );
    }
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

fn truncate_to_char_boundary(text: &mut String, max_len: usize) {
    if text.len() <= max_len {
        return;
    }
    let mut boundary = max_len;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text.truncate(boundary);
}

fn session_search_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|term| term.trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .take(8)
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

fn contains_search_snippet(
    session: &SessionRecord,
    document: &SessionSearchDocument,
    terms: &[String],
) -> String {
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
    session.title.clone().unwrap_or_default()
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

fn parse_prompt_tags(tags_json: &str, category: &str) -> Vec<String> {
    let tags = serde_json::from_str::<Vec<String>>(tags_json)
        .map(normalize_prompt_tags)
        .unwrap_or_default();
    if !tags.is_empty() {
        return tags;
    }
    normalize_prompt_tags(vec![category.to_string()])
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fmt::Write as _,
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{
        AgentScan, HookScan, McpScan, RuleRecord, RuleScan, ScanReport, SessionRecord, SessionScan,
        SkillRecord, SkillScan, SkillVisibility,
        analytics::{
            AnalyticsParserState, AnalyticsResponseUsage, AnalyticsTokenUsage, SessionAnalytics,
            SessionAnalyticsRecord,
        },
        session_skills::{SessionFileState, SessionSkillLink},
        sessions::SESSION_PREVIEW_MAX_CHARS,
        skills::{
            AgentKind, SkillPath, SkillRoot, SkillSnapshot, SkillSnapshotFile, SkillSourceRecord,
        },
    };
    use chrono::{Local, TimeZone};
    use rusqlite::{Connection, params};

    use super::{
        AppSettings, FsManifestEntry, PromptWrite, SESSION_SEARCH_RECORD_TEXT_LIMIT, Store,
        normalize_color_theme, normalize_repository_url, truncate_to_char_boundary,
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
    fn session_projects_join_worktrees_and_repository_renames_without_joining_basenames() {
        let temp = temp_dir("tendi-storage-session-projects");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let mut current = session("current", "Current");
        current.project = Some(PathBuf::from("/Users/test/dev/example/nextop"));
        current.repository_url = Some("https://github.com/tutti-os/tutti.git".to_string());
        current.updated_at = Some("2026-06-11T06:52:03Z".to_string());
        let mut legacy = session("legacy", "Legacy");
        legacy.project = current.project.clone();
        legacy.repository_url = Some("https://github.com/nextop-os/nextop.git".to_string());
        legacy.updated_at = Some("2026-06-11T06:50:19Z".to_string());
        let mut worktree = session("worktree", "Worktree");
        worktree.project = Some(PathBuf::from("/Users/test/.codex/worktrees/624a/nextop"));
        worktree.repository_url = current.repository_url.clone();
        let mut unrelated = session("unrelated", "Unrelated");
        unrelated.project = Some(PathBuf::from("/Users/test/dev/other/nextop"));
        unrelated.repository_url = Some("https://github.com/example/nextop.git".to_string());

        let mut sessions = vec![current, legacy, worktree, unrelated];
        store.resolve_session_projects(&mut sessions).unwrap();

        let project_id = sessions[0].logical_project_id.clone().unwrap();
        assert_eq!(sessions[0].logical_project_name.as_deref(), Some("tutti"));
        assert_eq!(
            sessions[1].logical_project_id.as_deref(),
            Some(project_id.as_str())
        );
        assert_eq!(
            sessions[2].logical_project_id.as_deref(),
            Some(project_id.as_str())
        );
        assert_ne!(
            sessions[3].logical_project_id.as_deref(),
            Some(project_id.as_str())
        );

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn resolving_projects_normalizes_legacy_codex_chat_workspaces() {
        let temp = temp_dir("tendi-storage-codex-chat-project");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let mut chat = session("chat", "Chat");
        chat.project = Some(PathBuf::from(
            "/Users/test/Documents/tutti/session-c98d1ced-5371-43cc-8173-9416c349a776",
        ));

        store
            .resolve_session_projects(std::slice::from_mut(&mut chat))
            .unwrap();

        assert_eq!(
            chat.project.as_deref(),
            Some(Path::new("/Users/test/Documents/tutti"))
        );
        assert_eq!(chat.logical_project_name.as_deref(), Some("tutti"));

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn app_settings_store_normalized_additional_session_roots() {
        let temp = temp_dir("tendi-storage-additional-session-settings");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let store = Store::open(&db).unwrap();
        assert_eq!(store.app_settings().unwrap().appearance, "system");
        assert_eq!(store.app_settings().unwrap().font_family, "manrope");
        assert_eq!(store.app_settings().unwrap().light_theme, "sakura-pop");
        assert_eq!(store.app_settings().unwrap().dark_theme, "sakura-pop");
        assert_eq!(store.app_settings().unwrap().app_icon, "sakura-pop");
        assert_eq!(
            store.app_settings().unwrap().session_resume_target,
            "terminal"
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
    fn skill_backup_config_round_trips_and_can_be_cleared() {
        let temp = temp_dir("tendi-storage-skill-backup-config");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let config = crate::skill_backup::BackupConfig::new(
            "git@github.com:example/tendi-skills.git",
            temp.join("backup"),
            "Test Mac",
        );

        let store = Store::open(&db).unwrap();
        assert!(store.skill_backup_config().unwrap().is_none());
        assert_eq!(
            store.save_skill_backup_config(&config).unwrap().remote_url,
            config.remote_url
        );
        drop(store);

        let store = Store::open(&db).unwrap();
        let saved = store.skill_backup_config().unwrap().unwrap();
        assert_eq!(saved.remote_url, config.remote_url);
        assert_eq!(saved.checkout_path, config.checkout_path);
        assert_eq!(saved.device_label, config.device_label);
        assert!(store.clear_skill_backup_config().unwrap());
        assert!(store.skill_backup_config().unwrap().is_none());

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn save_scan_records_existing_skill_manifest_path() {
        let temp = temp_dir("tendi-storage-scan-manifest");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let report = report_with_skill("manifest-skill", SkillVisibility::Auto);
        let skill_path = &report.skills.skills[0].paths[0].path;
        fs::create_dir_all(skill_path).unwrap();
        fs::write(
            skill_path.join("SKILL.md"),
            "---\nname: manifest-skill\n---\n",
        )
        .unwrap();

        store.save_scan(&report).unwrap();

        let entries = store
            .list_fs_manifest_for_root(skill_path.parent().unwrap())
            .unwrap();
        assert!(entries.iter().any(|entry| {
            entry.source_kind == "skill" && entry.path == skill_path.join("SKILL.md")
        }));

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn save_scan_replaces_stale_rows_with_latest_file_scan() {
        let temp = temp_dir("tendi-storage-replace");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let store = Store::open(&db).unwrap();

        store
            .save_scan(&report_with_skill("old", SkillVisibility::Auto))
            .unwrap();
        assert_eq!(count(&store, "skills"), 1);
        assert_eq!(count(&store, "skill_paths"), 1);
        assert_eq!(skill_names(&store), vec!["old"]);

        store
            .save_scan(&report_with_skill("new", SkillVisibility::Manual))
            .unwrap();

        assert_eq!(count(&store, "skills"), 1);
        assert_eq!(count(&store, "skill_paths"), 1);
        assert_eq!(skill_names(&store), vec!["new"]);
        assert_eq!(skill_visibility(&store, "new"), "manual");

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn remove_skills_cleans_skill_rows() {
        let temp = temp_dir("tendi-storage-remove-skills");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let store = Store::open(&db).unwrap();

        store
            .save_scan(&report_with_skill("remove-me", SkillVisibility::Auto))
            .unwrap();
        store.remove_skills(&["remove-me".to_string()]).unwrap();

        assert_eq!(count(&store, "skills"), 0);
        assert_eq!(count(&store, "skill_paths"), 0);

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn skill_source_lifecycle_upserts_looks_up_and_deletes_by_path() {
        let temp = temp_dir("tendi-storage-skill-source-lifecycle");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let skill_path = temp.join("skills/demo");
        let migrated = SkillSourceRecord {
            skill_name: "demo".to_string(),
            skill_path: skill_path.clone(),
            source_kind: "github".to_string(),
            source: Some("https://github.com/old/demo.git".to_string()),
            source_ref: Some("main".to_string()),
            source_version: Some("old".to_string()),
            source_relative_path: Some("skills/demo".to_string()),
            update_status: "tracked".to_string(),
            origin: "skills-cli-lock".to_string(),
        };
        assert_eq!(
            store
                .insert_skill_source_records_if_missing(std::slice::from_ref(&migrated))
                .unwrap(),
            1
        );

        let installed = SkillSourceRecord {
            source: Some("https://github.com/new/demo.git".to_string()),
            source_ref: Some("release".to_string()),
            source_version: Some("new".to_string()),
            origin: "tendi-install".to_string(),
            ..migrated
        };
        assert_eq!(
            store
                .upsert_skill_source_records(std::slice::from_ref(&installed))
                .unwrap(),
            1
        );
        let stored = store.skill_source_record(&skill_path).unwrap().unwrap();
        assert_eq!(stored.source, installed.source);
        assert_eq!(stored.source_ref, installed.source_ref);
        assert_eq!(stored.source_version, installed.source_version);
        assert_eq!(stored.origin, "tendi-install");

        assert_eq!(
            store
                .delete_skill_source_records(std::slice::from_ref(&skill_path))
                .unwrap(),
            1
        );
        assert!(store.skill_source_record(&skill_path).unwrap().is_none());

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
    fn storage_initializes_performance_indexes_and_manifest_table() {
        let temp = temp_dir("tendi-storage-performance-indexes");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let store = Store::open(&db).unwrap();

        for name in [
            "idx_sessions_updated_id",
            "idx_sessions_agent_updated_id",
            "idx_sessions_path",
            "ux_skill_paths_name_path",
            "idx_skill_sources_name_path",
            "idx_rules_agent_order",
            "idx_rules_agent_kind_scope_path_order",
            "idx_rules_path",
            "idx_hooks_agent_event_enabled",
            "idx_hooks_agent_event_path",
            "idx_hooks_path",
            "idx_mcp_servers_agent_status",
            "idx_mcp_servers_agent_name_path",
            "idx_mcp_servers_agent_name_transport_status_path",
            "idx_prompts_updated_title",
            "idx_fs_manifest_root_kind_path",
        ] {
            assert_eq!(
                store
                    .conn
                    .query_row(
                        "SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1",
                        params![name],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                1,
                "missing index {name}"
            );
        }

        let manifest_columns = store
            .conn
            .prepare("PRAGMA table_info(fs_manifest)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            manifest_columns,
            vec![
                "source_kind",
                "path",
                "root",
                "agent",
                "scope",
                "mtime_ns",
                "size",
                "inode",
                "device",
                "sha256",
                "parser_version",
                "last_seen_at",
                "parse_status",
            ]
        );

        drop(store);
        let reopened = Store::open(&db).unwrap();
        assert_eq!(
            reopened
                .conn
                .query_row("SELECT COUNT(*) FROM fs_manifest", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );

        drop(reopened);
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
    fn fs_manifest_upsert_list_and_delete_missing_entries() {
        let temp = temp_dir("tendi-storage-fs-manifest");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let root = temp.join("sessions");
        let first_path = root.join("first.jsonl");
        let second_path = root.join("second.jsonl");
        let mut first = FsManifestEntry {
            source_kind: "session".to_string(),
            path: first_path.clone(),
            root: root.clone(),
            agent: Some("codex".to_string()),
            scope: Some("global".to_string()),
            mtime_ns: Some(10),
            size: Some(100),
            inode: Some(11),
            device: Some(12),
            sha256: Some("old".to_string()),
            parser_version: "session-v1".to_string(),
            last_seen_at: 20,
            parse_status: "ok".to_string(),
        };
        store.upsert_fs_manifest(&first).unwrap();
        first.mtime_ns = Some(30);
        first.sha256 = Some("new".to_string());
        store.upsert_fs_manifest(&first).unwrap();

        let second = FsManifestEntry {
            path: second_path.clone(),
            mtime_ns: Some(40),
            last_seen_at: 50,
            ..first.clone()
        };
        assert_eq!(store.upsert_fs_manifest_entries(&[second]).unwrap(), 1);
        let skill_entry = FsManifestEntry {
            source_kind: "skill".to_string(),
            ..first.clone()
        };
        store.upsert_fs_manifest(&skill_entry).unwrap();
        assert_eq!(store.list_fs_manifest_for_root(&root).unwrap().len(), 3);
        assert_eq!(
            store
                .fs_manifest_entry("session", &first_path)
                .unwrap()
                .unwrap()
                .sha256,
            Some("new".to_string())
        );

        assert_eq!(
            store
                .delete_fs_manifest_missing_under_root("session", &root, &[first_path.clone()])
                .unwrap(),
            1
        );
        assert_eq!(store.list_fs_manifest_for_root(&root).unwrap().len(), 2);
        assert!(
            store
                .fs_manifest_entry("skill", &first_path)
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .delete_fs_manifest_entry("session", &first_path)
                .unwrap()
        );
        assert!(
            store
                .fs_manifest_entry("session", &first_path)
                .unwrap()
                .is_none()
        );

        drop(store);
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
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(&skill_file, "old").unwrap();

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
        assert!(store.list_rules_for_workspace(&first).unwrap().is_none());

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
    fn projection_refresh_lock_serializes_different_domains() {
        let temp = temp_dir("tendi-projection-lock-domains");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let result = store
            .with_projection_refresh_lock("rules", || {
                store.with_projection_refresh_lock("hooks", || Ok::<_, anyhow::Error>(42))
            })
            .unwrap();
        assert_eq!(result, Some(None));
        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn performance_indexes_preserve_save_scan_and_list_sessions() {
        let temp = temp_dir("tendi-storage-indexed-scan");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let mut report = report_with_skill("demo", SkillVisibility::Auto);
        let duplicate_path = report.skills.skills[0].paths[0].clone();
        report.skills.skills[0].paths.push(duplicate_path);
        report.sessions.sessions.push(session("one", "First"));

        store.save_scan(&report).unwrap();
        assert_eq!(store.list_sessions().unwrap().sessions.len(), 1);
        assert_eq!(count(&store, "skill_paths"), 1);

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn skill_delta_retains_source_records_as_database_authority() {
        let temp = temp_dir("tendi-storage-skill-source-delta");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let mut report = report_with_skill("demo", SkillVisibility::Auto).skills;
        let retained_path = report.skills[0].paths[0].path.clone();
        let stale_path = retained_path.with_file_name("demo-old");
        let other_path = retained_path.with_file_name("other");
        let records = [
            test_skill_source("demo", &retained_path),
            test_skill_source("demo", &stale_path),
            test_skill_source("other", &other_path),
        ];
        store.upsert_skill_source_records(&records).unwrap();

        store.save_skill_delta(&report.skills, &[]).unwrap();

        assert!(store.skill_source_record(&retained_path).unwrap().is_some());
        assert!(store.skill_source_record(&stale_path).unwrap().is_some());
        assert!(store.skill_source_record(&other_path).unwrap().is_some());

        report.skills.clear();
        store.save_skill_delta(&[], &["demo".to_string()]).unwrap();
        assert!(store.skill_source_record(&retained_path).unwrap().is_some());
        assert!(store.skill_source_record(&stale_path).unwrap().is_some());
        assert!(store.skill_source_record(&other_path).unwrap().is_some());

        drop(store);
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
    fn skill_delta_does_not_overwrite_unrelated_rows() {
        let temp = temp_dir("tendi-storage-skill-delta");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let mut alpha = report_with_skill("alpha", SkillVisibility::Auto).skills;
        let beta = report_with_skill("beta", SkillVisibility::Auto)
            .skills
            .skills
            .remove(0);
        alpha.skills.push(beta.clone());
        store.save_skills(&alpha).unwrap();

        let mut externally_updated_beta = beta;
        externally_updated_beta.description = Some("external beta".to_string());
        store
            .save_skill_delta(&[externally_updated_beta], &[])
            .unwrap();
        alpha.skills[0].description = Some("changed alpha".to_string());
        store.save_skill_delta(&alpha.skills[..1], &[]).unwrap();

        let beta_json = store
            .conn
            .query_row(
                "SELECT data_json FROM skills WHERE name = 'beta'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        let beta_json = serde_json::from_str::<serde_json::Value>(&beta_json).unwrap();
        assert_eq!(beta_json["description"], "external beta");

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
            skills: SkillScan {
                roots: vec![SkillRoot {
                    path: root.clone(),
                    scope: "project".to_string(),
                    agent: AgentKind::Shared,
                    plugin_id: None,
                    plugin_enabled: None,
                }],
                skills: vec![SkillRecord {
                    name: name.to_string(),
                    description: Some("demo".to_string()),
                    tags: Vec::new(),
                    dependencies: Vec::new(),
                    dependents: Vec::new(),
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
                        codex_allow_implicit_invocation: None,
                        codex_skill_enabled: None,
                        cursor_disable_model_invocation: None,
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
    fn save_sessions_upserts_and_removes_stale_rows() {
        let temp = temp_dir("tendi-storage-sessions");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let store = Store::open(&db).unwrap();

        store
            .save_sessions(&SessionScan {
                sessions: vec![session("one", "First"), session("two", "Second")],
                warnings: Vec::new(),
            })
            .unwrap();
        assert_eq!(store.list_sessions().unwrap().sessions.len(), 2);

        store
            .save_sessions(&SessionScan {
                sessions: vec![session("one", "First updated")],
                warnings: Vec::new(),
            })
            .unwrap();
        let cached = store.list_sessions().unwrap();

        assert_eq!(cached.sessions.len(), 1);
        assert_eq!(cached.sessions[0].id, "one");
        assert_eq!(cached.sessions[0].title.as_deref(), Some("First updated"));

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn session_preview_columns_round_trip_with_cached_sessions() {
        let temp = temp_dir("tendi-storage-session-previews");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let store = Store::open(&db).unwrap();
        let mut record = session("preview", "Cached title");
        record.first_user_message = Some("First question".to_string());
        record.last_user_message = Some("Last question".to_string());
        record.last_assistant_message = Some("Last answer".to_string());

        store
            .save_sessions(&SessionScan {
                sessions: vec![record],
                warnings: Vec::new(),
            })
            .unwrap();

        let cached = store.list_sessions().unwrap();
        assert_eq!(
            cached.sessions[0].first_user_message.as_deref(),
            Some("First question")
        );
        assert_eq!(
            cached.sessions[0].last_user_message.as_deref(),
            Some("Last question")
        );
        assert_eq!(
            cached.sessions[0].last_assistant_message.as_deref(),
            Some("Last answer")
        );
        let data_json = store
            .conn
            .query_row(
                "SELECT data_json FROM sessions WHERE id = 'preview'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        let data_json = serde_json::from_str::<serde_json::Value>(&data_json).unwrap();
        assert!(data_json.get("first_user_message").is_none());
        assert!(data_json.get("last_user_message").is_none());
        assert!(data_json.get("last_assistant_message").is_none());

        let mut preview_update = cached.sessions[0].clone();
        preview_update.last_user_message = Some("Updated by full save".to_string());
        store
            .save_sessions(&SessionScan {
                sessions: vec![preview_update.clone()],
                warnings: Vec::new(),
            })
            .unwrap();
        assert_eq!(
            store.list_sessions().unwrap().sessions[0]
                .last_user_message
                .as_deref(),
            Some("Updated by full save")
        );

        preview_update.last_user_message = Some("Updated by delta".to_string());
        assert_eq!(
            store.apply_session_delta(&[preview_update]).unwrap().len(),
            1
        );
        assert_eq!(
            store.list_sessions().unwrap().sessions[0]
                .last_user_message
                .as_deref(),
            Some("Updated by delta")
        );

        let oversized = "界".repeat(SESSION_PREVIEW_MAX_CHARS + 1);
        let mut oversized_record = session("oversized-preview", "Cached title");
        oversized_record.last_user_message = Some(oversized);
        store
            .save_sessions(&SessionScan {
                sessions: vec![oversized_record],
                warnings: Vec::new(),
            })
            .unwrap();
        let bounded = store.list_sessions().unwrap();
        assert_eq!(
            bounded.sessions[0]
                .last_user_message
                .as_deref()
                .unwrap()
                .chars()
                .count(),
            SESSION_PREVIEW_MAX_CHARS + 1
        );

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn session_storage_canonicalizes_titles_and_previews() {
        let temp = temp_dir("tendi-storage-session-canonicalization");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let store = Store::open(&db).unwrap();
        let mut record = session(
            "canonicalized",
            "<image name=[Image #1] path=\"/tmp/image.png\"> Cached title",
        );
        record.first_user_message =
            Some("<image name=[Image #1] path=\"/tmp/image.png\">\nFirst question".to_string());
        record.last_user_message =
            Some("<image name=[Image #1] path=\"/tmp/image.png\">".to_string());

        store
            .save_sessions(&SessionScan {
                sessions: vec![record],
                warnings: Vec::new(),
            })
            .unwrap();

        let cached = store.list_sessions().unwrap();
        assert_eq!(cached.sessions[0].title.as_deref(), Some("Cached title"));
        assert_eq!(
            cached.sessions[0].first_user_message.as_deref(),
            Some("First question")
        );
        assert_eq!(
            cached.sessions[0].last_user_message.as_deref(),
            Some("Image")
        );
        let stored = store
            .conn
            .query_row(
                "SELECT title, first_user_message, last_user_message, data_json
                 FROM sessions WHERE id = 'canonicalized'",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(stored.0.as_deref(), Some("Cached title"));
        assert_eq!(stored.1.as_deref(), Some("First question"));
        assert_eq!(stored.2.as_deref(), Some("Image"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&stored.3)
                .unwrap()
                .get("title")
                .and_then(serde_json::Value::as_str),
            Some("Cached title")
        );

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn session_analytics_cache_skips_unchanged_appends_and_cleans_stale_rows() {
        use std::{fs::OpenOptions, io::Write};

        let temp = temp_dir("tendi-storage-analytics");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let path = temp.join("rollout-analytics.jsonl");
        let timestamp = chrono::Local::now().to_rfc3339();
        fs::write(
            &path,
            format!(
                "{{\"timestamp\":\"{timestamp}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":10,\"total_tokens\":10}}}}}}}}\n"
            ),
        )
        .unwrap();
        let store = Store::open(&db).unwrap();
        let mut record = session("analytics", "Analytics");
        record.path = path.clone();
        store
            .save_sessions(&SessionScan {
                sessions: vec![record.clone()],
                warnings: Vec::new(),
            })
            .unwrap();
        let initial_revision = store.analytics_revision().unwrap();

        let first = store
            .refresh_session_analytics(std::slice::from_ref(&record))
            .unwrap();
        assert_eq!((first.parsed, first.skipped, first.failed), (1, 0, 0));
        let first_revision = store.analytics_revision().unwrap();
        assert!(first_revision > initial_revision);
        let unchanged = store
            .refresh_session_analytics(std::slice::from_ref(&record))
            .unwrap();
        assert_eq!((unchanged.parsed, unchanged.skipped), (0, 1));
        assert_eq!(store.analytics_revision().unwrap(), first_revision);

        store
            .conn
            .execute("UPDATE session_analytics SET parser_state_json = '{}'", [])
            .unwrap();
        let reparsed = store
            .refresh_session_analytics(std::slice::from_ref(&record))
            .unwrap();
        assert_eq!(
            (reparsed.parsed, reparsed.skipped, reparsed.failed),
            (1, 0, 0)
        );
        let reparsed_revision = store.analytics_revision().unwrap();
        assert!(reparsed_revision > first_revision);

        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            file,
            "{{\"timestamp\":\"{timestamp}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":25,\"total_tokens\":25}}}}}}}}"
        )
        .unwrap();
        let appended = store
            .refresh_session_analytics(std::slice::from_ref(&record))
            .unwrap();
        assert_eq!((appended.parsed, appended.appended), (1, 1));
        assert!(store.analytics_revision().unwrap() > reparsed_revision);
        let overview = store.overview_analytics(None, 1, 1).unwrap();
        assert_eq!(overview.summary.usage.total_tokens, 25);

        store
            .save_sessions(&SessionScan {
                sessions: Vec::new(),
                warnings: Vec::new(),
            })
            .unwrap();
        assert_eq!(count(&store, "session_analytics"), 0);

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn unchanged_analytics_skips_before_parsing_corrupt_cached_json() {
        let temp = temp_dir("tendi-storage-analytics-skip-corrupt");
        fs::create_dir_all(&temp).unwrap();
        let path = temp.join("rollout.jsonl");
        let timestamp = Local::now().to_rfc3339();
        fs::write(
            &path,
            format!(
                "{{\"timestamp\":\"{timestamp}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":10,\"total_tokens\":10}}}}}}}}\n"
            ),
        )
        .unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let mut record = session("skip-corrupt", "Skip corrupt");
        record.path = path;
        store
            .save_sessions(&SessionScan {
                sessions: vec![record.clone()],
                warnings: Vec::new(),
            })
            .unwrap();
        store
            .refresh_session_analytics(std::slice::from_ref(&record))
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE session_analytics SET analytics_json = 'not-json'",
                [],
            )
            .unwrap();

        let refresh = store
            .refresh_session_analytics(std::slice::from_ref(&record))
            .unwrap();

        assert_eq!((refresh.parsed, refresh.skipped, refresh.failed), (0, 1, 0));
        assert!(refresh.warnings.is_empty());
        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn overview_filters_json_by_local_day_without_shrinking_full_history_metadata() {
        let temp = temp_dir("tendi-storage-overview-window");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let today = Local::now().date_naive();
        let yesterday = today.pred_opt().unwrap();
        let today_timestamp = Local
            .from_local_datetime(&today.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap()
            .to_rfc3339();
        let yesterday_timestamp = Local
            .from_local_datetime(&yesterday.and_hms_opt(23, 59, 0).unwrap())
            .single()
            .unwrap()
            .to_rfc3339();
        store
            .save_session_analytics_records(&[
                analytics_record("old", AgentKind::Claude, &yesterday_timestamp, 10),
                analytics_record("current", AgentKind::Codex, &today_timestamp, 25),
            ])
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE session_analytics SET analytics_json = 'not-json' WHERE session_id = 'old'",
                [],
            )
            .unwrap();

        let overview = store.overview_analytics(None, 1, 1).unwrap();

        assert_eq!(overview.summary.usage.total_tokens, 25);
        assert_eq!(overview.days.len(), 1);
        assert_eq!(overview.days[0].date, today.to_string());
        let today = today.to_string();
        let yesterday = yesterday.to_string();
        assert_eq!(overview.coverage.first.as_deref(), Some(yesterday.as_str()));
        assert_eq!(overview.coverage.last.as_deref(), Some(today.as_str()));
        assert_eq!(overview.coverage.total_sessions, 2);
        assert_eq!(overview.coverage.analyzed_sessions, 2);
        assert!(
            overview
                .capabilities
                .iter()
                .any(|entry| entry.agent == AgentKind::Claude)
        );
        assert!(
            overview
                .capabilities
                .iter()
                .any(|entry| entry.agent == AgentKind::Codex)
        );

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
    fn overview_hot_path_does_not_parse_unindexed_legacy_json() {
        let temp = temp_dir("tendi-storage-overview-unindexed");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        store
            .conn
            .execute(
                "INSERT INTO session_analytics (
                    session_id, agent, session_path, file_mtime, file_size,
                    indexed_at, analytics_json, parser_state_json, overview_indexed
                 ) VALUES ('legacy', 'codex', '/tmp/legacy.jsonl', 0, 0, '0', 'not-json', '{}', 0)",
                [],
            )
            .unwrap();

        let overview = store.overview_analytics(None, 30, 30).unwrap();

        assert_eq!(overview.coverage.indexing_sessions, 1);
        assert!(overview.warnings.is_empty());
        drop(store);
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
    fn session_analytics_reports_progress_at_durable_batch_boundaries() {
        let temp = temp_dir("tendi-storage-analytics-progress");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let timestamp = chrono::Local::now().to_rfc3339();
        let mut sessions = Vec::new();
        for index in 0..65 {
            let path = temp.join(format!("rollout-{index}.jsonl"));
            fs::write(
                &path,
                format!(
                    "{{\"timestamp\":\"{timestamp}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":10,\"total_tokens\":10}}}}}}}}\n"
                ),
            )
            .unwrap();
            let mut record = session(&format!("analytics-{index}"), "Analytics");
            record.path = path;
            sessions.push(record);
        }
        store
            .save_sessions(&SessionScan {
                sessions: sessions.clone(),
                warnings: Vec::new(),
            })
            .unwrap();

        let mut progress = Vec::new();
        let report = store
            .refresh_session_analytics_with_progress(&sessions, |event| {
                progress.push((event.completed, count(&store, "session_analytics")))
            })
            .unwrap();

        assert_eq!((report.total, report.parsed, report.failed), (65, 65, 0));
        assert_eq!(progress, vec![(0, 0), (64, 64), (65, 65)]);
        assert_eq!(count(&store, "session_analytics"), 65);

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn search_sessions_matches_indexed_transcript_text() {
        let temp = temp_dir("tendi-storage-session-search");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let transcript = temp.join("one.jsonl");
        fs::write(
            &transcript,
            concat!(
                r#"{"timestamp":"2026-06-23T09:59:00Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"助手也提到脱敏"}]}}"#,
                "\n",
                r#"{"timestamp":"2026-06-23T10:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Find the hidden voltaic marker，确认脱敏逻辑"}]}}"#,
            ),
        )
        .unwrap();
        let store = Store::open(&db).unwrap();
        let mut searchable = session("one", "First");
        searchable.path = transcript;

        store
            .save_sessions(&SessionScan {
                sessions: vec![searchable],
                warnings: Vec::new(),
            })
            .unwrap();

        let matches = store.search_sessions("voltaic").unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].session.id, "one");
        assert!(matches[0].search_snippet.contains("⟦voltaic⟧"));
        let short_matches = store.search_sessions("脱敏").unwrap();
        assert_eq!(short_matches.len(), 1);
        assert!(short_matches[0].search_snippet.contains("确认⟦脱敏⟧逻辑"));
        assert!(store.search_sessions("missing").unwrap().is_empty());

        store
            .save_sessions(&SessionScan {
                sessions: Vec::new(),
                warnings: Vec::new(),
            })
            .unwrap();
        assert!(store.search_sessions("voltaic").unwrap().is_empty());
        let integrity = store
            .conn
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap();
        assert_eq!(integrity, "ok");

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn search_sessions_indexes_late_user_text_after_large_tool_results() {
        let temp = temp_dir("tendi-storage-session-search-late-user");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let transcript = temp.join("one.jsonl");
        let mut transcript_text = String::new();
        let large_result = "tool_result_only_marker ".repeat(600);
        for index in 0..48 {
            writeln!(
                transcript_text,
                r#"{{"timestamp":"2026-06-23T10:00:00Z","type":"response_item","payload":{{"type":"function_call","name":"exec_command","arguments":"{{}}","call_id":"call_{index}"}}}}"#
            )
            .unwrap();
            writeln!(
                transcript_text,
                r#"{{"timestamp":"2026-06-23T10:00:01Z","type":"response_item","payload":{{"type":"function_call_output","call_id":"call_{index}","output":"{large_result}"}}}}"#
            )
            .unwrap();
        }
        writeln!(
            transcript_text,
            r#"{{"timestamp":"2026-06-23T10:01:00Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"另外为什么网络正确是让vm跑tailscale？"}}]}}}}"#
        )
        .unwrap();
        fs::write(&transcript, transcript_text).unwrap();

        let store = Store::open(&db).unwrap();
        let mut searchable = session("one", "Network investigation");
        searchable.path = transcript;
        store
            .save_sessions(&SessionScan {
                sessions: vec![searchable],
                warnings: Vec::new(),
            })
            .unwrap();

        let matches = store.search_sessions("让vm跑tailscale").unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].session.id, "one");
        assert!(matches[0].search_snippet.contains("⟦让vm跑tailscale⟧"));
        assert!(
            store
                .search_sessions("tool_result_only_marker")
                .unwrap()
                .is_empty()
        );
        let (record_count, largest_record) = store
            .conn
            .query_row(
                "SELECT
                    COUNT(*),
                    MAX(
                        length(CAST(user_text AS BLOB)) +
                        length(CAST(assistant_text AS BLOB))
                    )
                 FROM session_search_records
                 WHERE session_id = 'one'",
                [],
                |row| Ok((row.get::<_, usize>(0)?, row.get::<_, usize>(1)?)),
            )
            .unwrap();
        assert_eq!(record_count, 2);
        assert!(largest_record <= SESSION_SEARCH_RECORD_TEXT_LIMIT);

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn search_sessions_uses_bm25_field_weights() {
        let temp = temp_dir("tendi-storage-session-search-rank");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let title_transcript = temp.join("title.jsonl");
        let body_transcript = temp.join("body.jsonl");
        let assistant_transcript = temp.join("assistant.jsonl");
        fs::write(
            &title_transcript,
            r#"{"timestamp":"2026-06-23T10:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Routine release notes"}]}}"#,
        )
        .unwrap();
        fs::write(
            &body_transcript,
            r#"{"timestamp":"2026-06-23T10:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Investigate the voltaic failure"}]}}"#,
        )
        .unwrap();
        fs::write(
            &assistant_transcript,
            r#"{"timestamp":"2026-06-23T10:00:00Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Investigate the voltaic failure"}]}}"#,
        )
        .unwrap();
        let store = Store::open(&db).unwrap();
        let mut title_match = session("title", "Voltaic release");
        title_match.path = title_transcript;
        let mut body_match = session("body", "Routine failure");
        body_match.path = body_transcript;
        let mut assistant_match = session("assistant", "Routine logs");
        assistant_match.path = assistant_transcript;
        store
            .save_sessions(&SessionScan {
                sessions: vec![body_match, assistant_match, title_match],
                warnings: Vec::new(),
            })
            .unwrap();

        let matches = store.search_sessions("voltaic").unwrap();
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].session.id, "body");
        assert!(matches[0].search_score > matches[2].search_score);
        assert_eq!(matches[1].session.id, "title");
        assert_eq!(matches[2].session.id, "assistant");
        assert_eq!(matches[1].search_snippet, "⟦Voltaic⟧ release");

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn search_sessions_refreshes_changed_metadata_without_reparsing() {
        let temp = temp_dir("tendi-storage-session-search-metadata");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let transcript = temp.join("one.jsonl");
        fs::write(
            &transcript,
            r#"{"timestamp":"2026-06-23T10:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Routine notes"}]}}"#,
        )
        .unwrap();
        let store = Store::open(&db).unwrap();
        let mut searchable = session("one", "Original heading");
        searchable.path = transcript;
        searchable.mode = Some("alpha".to_string());
        store
            .save_sessions(&SessionScan {
                sessions: vec![searchable.clone()],
                warnings: Vec::new(),
            })
            .unwrap();

        searchable.title = Some("Renamed voltaic heading".to_string());
        searchable.mode = Some("beta".to_string());
        store
            .save_sessions(&SessionScan {
                sessions: vec![searchable],
                warnings: Vec::new(),
            })
            .unwrap();

        assert!(store.search_sessions("original").unwrap().is_empty());
        assert!(store.search_sessions("alpha").unwrap().is_empty());
        assert_eq!(
            store.search_sessions("voltaic").unwrap()[0].session.id,
            "one"
        );
        assert_eq!(store.search_sessions("beta").unwrap()[0].session.id, "one");

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn session_delta_only_returns_changes_and_persists_scan_watermark() {
        let temp = temp_dir("tendi-storage-session-delta");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let transcript = temp.join("one.jsonl");
        fs::write(&transcript, "{}\n").unwrap();
        let store = Store::open(&db).unwrap();
        let mut record = session("one", "First");
        record.path = transcript.clone();

        assert_eq!(
            store.apply_session_delta(&[record.clone()]).unwrap().len(),
            1
        );
        assert!(
            store
                .apply_session_delta(&[record.clone()])
                .unwrap()
                .is_empty()
        );

        record.title = Some("Updated".to_string());
        assert_eq!(
            store.apply_session_delta(&[record.clone()]).unwrap().len(),
            1
        );
        let mut metadata_update = record.clone();
        metadata_update.path = temp.join("meta.json");
        metadata_update.title = Some("Metadata title".to_string());
        let canonical = store.apply_session_delta(&[metadata_update]).unwrap();
        assert_eq!(canonical[0].path, transcript);
        assert_eq!(store.list_sessions().unwrap().sessions.len(), 1);
        store.index_session_delta(&canonical).unwrap();
        assert_eq!(
            store.search_sessions("metadata").unwrap()[0].session.path,
            transcript
        );
        store
            .save_sessions_at(
                &SessionScan {
                    sessions: canonical,
                    warnings: Vec::new(),
                },
                123,
            )
            .unwrap();
        assert_eq!(store.sessions_last_scan_at().unwrap(), Some(123));

        assert_eq!(store.last_scan_at().unwrap(), None);
        store
            .conn
            .execute(
                "INSERT INTO meta (key, value) VALUES ('last_scan_at', '456')",
                [],
            )
            .unwrap();
        assert_eq!(store.last_scan_at().unwrap(), Some(456));
        store
            .conn
            .execute(
                "UPDATE meta SET value = 'invalid' WHERE key = 'last_scan_at'",
                [],
            )
            .unwrap();
        assert!(store.last_scan_at().is_err());

        let removed = store.remove_sessions_for_paths(&[transcript]).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(store.list_sessions().unwrap().sessions.is_empty());

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn search_text_truncation_uses_utf8_boundary() {
        let max_len = 10;
        let mut text = format!("{}你", "a".repeat(max_len - 1));
        truncate_to_char_boundary(&mut text, max_len);
        assert!(text.len() <= max_len);
        assert!(text.is_char_boundary(text.len()));
        assert_eq!(text.len(), max_len - 1);
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
    fn session_skill_links_query_both_directions_and_cleanup_stale_sessions() {
        let temp = temp_dir("tendi-storage-session-skills");
        fs::create_dir_all(&temp).unwrap();
        let db = temp.join("tendi.sqlite3");
        let store = Store::open(&db).unwrap();
        let session_one = session("one", "First");
        let session_two = session("two", "Second");

        store
            .save_sessions(&SessionScan {
                sessions: vec![session_one.clone(), session_two.clone()],
                warnings: Vec::new(),
            })
            .unwrap();
        store
            .replace_session_skill_links(
                &session_one,
                &SessionFileState {
                    file_mtime: 1,
                    file_size: 10,
                },
                &[link(&session_one, "foo")],
            )
            .unwrap();
        store
            .replace_session_skill_links(
                &session_two,
                &SessionFileState {
                    file_mtime: 1,
                    file_size: 10,
                },
                &[link(&session_two, "foo")],
            )
            .unwrap();

        assert_eq!(
            store
                .session_skill_links("one", AgentKind::Codex)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(store.skill_session_links("foo").unwrap().len(), 2);
        assert!(
            store
                .session_skill_index_is_current(&session_one, 1, 10)
                .unwrap()
        );

        store
            .save_sessions(&SessionScan {
                sessions: vec![session_one.clone()],
                warnings: Vec::new(),
            })
            .unwrap();

        assert_eq!(store.skill_session_links("foo").unwrap().len(), 1);
        assert_eq!(count(&store, "session_skill_index"), 1);

        drop(store);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn session_skill_index_version_invalidates_once() {
        let temp = temp_dir("tendi-storage-session-skills-version");
        fs::create_dir_all(&temp).unwrap();
        let store = Store::open(temp.join("tendi.sqlite3")).unwrap();
        let session = session("versioned", "Versioned");

        store
            .save_sessions(&SessionScan {
                sessions: vec![session.clone()],
                warnings: Vec::new(),
            })
            .unwrap();
        store
            .replace_session_skill_links(
                &session,
                &SessionFileState {
                    file_mtime: 1,
                    file_size: 10,
                },
                &[link(&session, "foo")],
            )
            .unwrap();

        assert!(store.ensure_session_skill_index_version("2").unwrap());
        assert!(store.skill_session_links("foo").unwrap().is_empty());
        assert_eq!(count(&store, "session_skill_index"), 0);
        assert!(!store.ensure_session_skill_index_version("2").unwrap());

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

    fn count(store: &Store, table: &str) -> i64 {
        store
            .conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    fn skill_names(store: &Store) -> Vec<String> {
        let mut statement = store
            .conn
            .prepare("SELECT name FROM skills ORDER BY name")
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn skill_visibility(store: &Store, name: &str) -> String {
        store
            .conn
            .query_row(
                "SELECT visibility FROM skills WHERE name = ?1",
                [name],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn accepts_vercel_color_theme() {
        assert_eq!(normalize_color_theme(" VERCEL ").unwrap(), "vercel");
    }

    #[test]
    fn accepts_sakura_pop_color_theme() {
        assert_eq!(
            normalize_color_theme(" SAKURA-POP ").unwrap(),
            "sakura-pop"
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
