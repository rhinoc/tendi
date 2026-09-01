use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use serde_json::Value;

use crate::{
    analytics::{AnalyticsCapabilities, SessionAnalyticsRecord},
    hooks::{HookDeleteRequest, HookRecord, HookSetEnabledRequest, HookSourceMatch},
    mcp::{McpServerRecord, McpSetEnabledRequest},
    rules::{self, RuleRecord},
    session_skills::Evidence,
    sessions::{
        self, SessionMetadata, SessionRecord, SessionScanCache, SessionTokenUsage,
        SessionWatchExpansion, SessionWatchTarget,
    },
    skills::{AgentKind, FileChange, SkillPath, SkillRoot, SkillVisibility},
    transcript::TranscriptItem,
};

pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod cursor;
pub(crate) mod cursor_sessions;
pub(crate) mod shared;
mod unknown;

pub(crate) struct ProviderContext {
    pub home: Option<PathBuf>,
    project_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProviderSkillTarget {
    pub id: &'static str,
    pub display_name: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionPathRole {
    Other,
    Transcript,
    Metadata,
    Index,
}

impl ProviderContext {
    pub(crate) fn new(cwd: &Path) -> Self {
        Self::with_additional_project_dirs(cwd, &[])
    }

    pub(crate) fn with_additional_project_dirs(
        cwd: &Path,
        additional_project_dirs: &[PathBuf],
    ) -> Self {
        let mut project_dirs = project_dirs(cwd);
        for directory in additional_project_dirs {
            let directory = directory
                .canonicalize()
                .unwrap_or_else(|_| directory.to_path_buf());
            if !project_dirs.contains(&directory) {
                project_dirs.push(directory);
            }
        }
        Self {
            home: dirs::home_dir(),
            project_dirs,
        }
    }

    pub(crate) fn project_dirs(&self) -> &[PathBuf] {
        &self.project_dirs
    }
}

pub(crate) trait AgentProvider: Sync {
    fn kind(&self) -> AgentKind;

    fn storage_key(&self) -> &'static str;

    fn discoverable(&self) -> bool {
        false
    }

    fn matches_name(&self, _normalized: &str) -> bool {
        false
    }

    fn display_name(&self) -> Option<&'static str> {
        None
    }

    fn app_bundle_path(&self) -> Option<&'static str> {
        None
    }

    fn executable_names(&self) -> &'static [&'static str] {
        &[]
    }

    fn config_dir(&self, _ctx: &ProviderContext) -> Option<PathBuf> {
        None
    }

    fn config_home(&self, home: &Path) -> PathBuf {
        home.to_path_buf()
    }

    #[cfg(test)]
    fn config_home_for_test(&self, home: &Path, _override_home: &Path) -> PathBuf {
        self.config_home(home)
    }

    fn projection_directories(&self) -> &'static [&'static str] {
        &[]
    }

    fn projection_candidate_files(&self, _domain: &str, _ancestor: &Path) -> Vec<PathBuf> {
        Vec::new()
    }

    fn projection_candidate_is_file(&self, _path: &Path) -> bool {
        false
    }

    fn session_scan_priority(&self, _root: &Path) -> Option<u8> {
        None
    }

    fn is_session_candidate_path(&self, _path: &Path) -> bool {
        false
    }

    fn session_path_role(&self, path: &Path) -> SessionPathRole {
        if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            SessionPathRole::Transcript
        } else {
            SessionPathRole::Other
        }
    }

    fn session_scan_source_paths(&self, path: &Path) -> Vec<PathBuf> {
        vec![path.to_path_buf()]
    }

    fn skill_visibility_metadata(
        &self,
        _skill_dir: &Path,
        _skill_file: &Path,
        _frontmatter: Option<&serde_yaml::Value>,
    ) -> Result<SkillProviderMetadata> {
        Ok(SkillProviderMetadata::default())
    }

    fn effective_skill_visibility(
        &self,
        tendi_visibility: Option<SkillVisibility>,
        provider_visibility: SkillVisibility,
        _root: &SkillRoot,
    ) -> SkillVisibility {
        crate::skills::combine_skill_visibility(tendi_visibility, provider_visibility)
    }

    fn skill_backup_exclusion_reason(&self, _path: &SkillPath) -> Option<&'static str> {
        None
    }

    fn skill_frontmatter_satisfies(
        &self,
        meta: &serde_yaml::Mapping,
        visibility: SkillVisibility,
    ) -> bool {
        crate::skills::skill_frontmatter_satisfies_with_provider_key(meta, visibility, None)
    }

    fn render_skill_frontmatter(
        &self,
        before: &str,
        visibility: SkillVisibility,
    ) -> Result<String> {
        crate::skills::render_skill_frontmatter_with_provider_key(before, visibility, None)
    }

    fn skill_frontmatter_visibility_key(&self) -> Option<&'static str> {
        None
    }

    fn plan_skill_visibility(
        &self,
        _skill_dir: &Path,
        _visibility: SkillVisibility,
        _update_provider_config: bool,
    ) -> Result<Vec<FileChange>> {
        Ok(Vec::new())
    }

    fn is_managed_skill_file(&self, _relative_path: &str) -> bool {
        false
    }

    fn normalize_skill_file_for_merge(
        &self,
        _relative_path: &str,
        _local: Option<&str>,
        _base: Option<&str>,
        _incoming: Option<&str>,
        _visibility: SkillVisibility,
    ) -> Option<(Option<String>, Option<String>, Option<String>)> {
        None
    }

    fn skill_roots(&self, _ctx: &ProviderContext) -> Vec<SkillRoot> {
        Vec::new()
    }

    fn global_skill_root(&self, _home: &Path) -> Option<PathBuf> {
        None
    }

    fn global_backup_root(&self, home: &Path) -> Option<PathBuf> {
        self.global_skill_root(home)
            .and_then(|path| path.parent().map(Path::to_path_buf))
    }

    fn backup_global_source_key(&self, path: &Path) -> Option<String> {
        self.global_backup_root(&dirs::home_dir()?)
            .and_then(|root| path.strip_prefix(root).ok())
            .and_then(|relative| relative.to_str())
            .map(str::to_string)
    }

    fn restore_global_source_path(&self, relative: &str) -> Option<PathBuf> {
        let relative = Path::new(relative);
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return None;
        }
        Some(self.global_backup_root(&dirs::home_dir()?)?.join(relative))
    }

    fn project_skill_root(&self, _cwd: &Path) -> Option<PathBuf> {
        None
    }

    fn skill_target(&self) -> Option<ProviderSkillTarget> {
        None
    }

    fn bundled_skill_files(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }

    fn config_profile_path(
        &self,
        _home: &Path,
        _agent_home: &Path,
        _name: &str,
    ) -> Option<PathBuf> {
        None
    }

    fn config_profile_format(&self) -> Option<&'static str> {
        None
    }

    fn config_files(
        &self,
        _home: &Path,
        _agent_home: &Path,
    ) -> Vec<crate::config::AgentConfigFile> {
        Vec::new()
    }

    fn config_file_for_path(
        &self,
        _home: &Path,
        _agent_home: &Path,
        _path: &Path,
    ) -> Option<crate::config::AgentConfigFile> {
        None
    }

    fn config_order(&self) -> usize {
        usize::MAX
    }

    fn scan_sessions(
        &self,
        _ctx: &ProviderContext,
        _sessions: &mut Vec<SessionRecord>,
        _warnings: &mut Vec<String>,
        _cache: Option<&SessionScanCache>,
    ) -> Result<()> {
        Ok(())
    }

    fn session_roots(&self, _ctx: &ProviderContext) -> Vec<PathBuf> {
        Vec::new()
    }

    fn session_watch_targets(&self, _root: &Path) -> Option<(Vec<SessionWatchTarget>, bool)> {
        None
    }

    fn session_watch_expansion(
        &self,
        _dynamic_roots: &[PathBuf],
        _event_path: &Path,
    ) -> Option<SessionWatchExpansion> {
        None
    }

    fn collect_additional_session_paths(
        &self,
        _root: &Path,
        _session_paths: &mut std::collections::BTreeSet<PathBuf>,
    ) -> bool {
        false
    }

    fn scan_rules(
        &self,
        _ctx: &ProviderContext,
        _rules: &mut Vec<RuleRecord>,
        _warnings: &mut Vec<String>,
        _order: &mut usize,
    ) {
    }

    fn resume_session_command(&self, _session: &SessionRecord) -> Option<SessionCommand> {
        None
    }

    fn resume_target_from_transcript_value(&self, _value: &Value) -> Option<&'static str> {
        None
    }

    fn accepts_session_app_url(&self, _url: &str) -> bool {
        false
    }

    fn active_session_writer(&self, _session: &SessionRecord) -> Result<Option<SessionWriter>> {
        Ok(None)
    }

    fn validate_session_resume(&self, _session: &SessionRecord) -> Result<()> {
        Ok(())
    }

    fn session_requires_rescan(&self, _session: &SessionRecord) -> Option<bool> {
        None
    }

    fn session_line_has_content(&self, _prefix: &str) -> Option<bool> {
        None
    }

    fn session_line_requires_metadata_parse(
        &self,
        _prefix: &str,
        _meta: &SessionMetadata,
    ) -> Option<bool> {
        None
    }

    fn update_session_metadata(
        &self,
        _value: &Value,
        _meta: &mut SessionMetadata,
        _deduplicated_usage: &mut BTreeMap<String, SessionTokenUsage>,
    ) {
    }

    fn config_profile_key(&self) -> Option<&'static str> {
        None
    }

    fn skill_target_aliases(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }

    fn apply_config_profile(&self, _command: &mut SessionCommand, _profile: &str) -> Result<()> {
        Ok(())
    }

    fn parse_transcript_value(&self, value: &Value, items: &mut Vec<TranscriptItem>);

    fn transcript_internal_context_markers(
        &self,
    ) -> &'static [crate::transcript::InternalContextMarker] {
        &[]
    }

    fn transcript_inherited_history_start_ordinal(&self, _value: &Value) -> Option<u64> {
        None
    }

    fn append_transcript_metadata(
        &self,
        _path: &Path,
        _items: &mut Vec<TranscriptItem>,
    ) -> Result<()> {
        Ok(())
    }

    fn append_transcript_metadata_from_store(
        &self,
        path: &Path,
        items: &mut Vec<TranscriptItem>,
    ) -> Result<()> {
        self.append_transcript_metadata(path, items)
    }

    fn enrich_transcript_tools_from_store(
        &self,
        _path: &Path,
        _items: &mut Vec<TranscriptItem>,
    ) -> Result<()> {
        Ok(())
    }

    fn transcript_metadata_store_path(&self, _path: &Path) -> Option<PathBuf> {
        None
    }

    fn transcript_search_hint(&self, _line: &str) -> bool {
        true
    }

    fn transcript_cacheable(&self) -> bool {
        true
    }

    fn recognizes_transcript(&self, _value: &Value) -> bool {
        false
    }

    fn session_supports_append_cache(&self) -> bool {
        false
    }

    fn infer_session_project(&self, _path: &Path, project: Option<PathBuf>) -> Option<PathBuf> {
        project
    }

    fn normalize_session_project(&self, project: PathBuf) -> PathBuf {
        project
    }

    fn infer_meta_project(&self, _value: &Value) -> Option<PathBuf> {
        None
    }

    fn session_id_from_path(&self, path: &Path) -> Option<String> {
        path.file_stem()
            .and_then(|name| name.to_str())
            .map(str::to_string)
    }

    fn session_project_aliases(&self, _path: &str) -> Vec<String> {
        Vec::new()
    }

    fn uses_shared_skill_layout(&self) -> bool {
        false
    }

    fn scan_explicit_session_path(
        &self,
        _path: &Path,
        _sessions: &mut Vec<SessionRecord>,
        _cache: Option<&SessionScanCache>,
    ) -> bool {
        false
    }

    fn analytics_capabilities(&self) -> AnalyticsCapabilities {
        AnalyticsCapabilities {
            token_usage: false,
            reasoning_tokens: false,
            explicit_runs: false,
            duration: false,
            rate_limit_history: false,
        }
    }

    fn analytics_model_hint(&self, _session: &SessionRecord) -> Option<String> {
        None
    }

    fn parse_analytics_line(&self, _line: &str, _record: &mut SessionAnalyticsRecord) {}

    fn extract_skill_tool_payloads<'a>(&self, _value: &'a Value) -> Vec<(&'a Value, Evidence)> {
        Vec::new()
    }

    fn scan_mcp(
        &self,
        _ctx: &ProviderContext,
        _servers: &mut Vec<McpServerRecord>,
        _warnings: &mut Vec<String>,
    ) -> Result<()> {
        Ok(())
    }

    fn set_mcp_enabled(&self, _request: &McpSetEnabledRequest) -> Result<()> {
        bail!(
            "MCP enable/disable is not supported for {}",
            self.storage_key()
        )
    }

    fn backup_mcp_entry(
        &self,
        _path: &Path,
        _server_path: &[String],
        _name: &str,
    ) -> Result<Value> {
        bail!("MCP entry sync is not supported for {}", self.storage_key())
    }

    fn restore_mcp_entry(
        &self,
        _path: &Path,
        _server_path: &[String],
        _name: &str,
        _entry: &Value,
    ) -> Result<String> {
        bail!("MCP entry restore is not supported for {}", self.storage_key())
    }

    fn mcp_status_after_toggle(&self, enabled: bool) -> &'static str;

    fn delete_hooks(
        &self,
        _requests: &[HookDeleteRequest],
        _source: &str,
    ) -> Result<String> {
        bail!("hook deletion is not supported for {}", self.storage_key())
    }

    fn set_hook_enabled(
        &self,
        _request: &HookSetEnabledRequest,
        _source: &str,
    ) -> Result<String> {
        bail!("hook enable/disable is not supported for {}", self.storage_key())
    }

    fn backup_hook_entry(&self, _path: &Path, _identity: &HookSourceMatch) -> Result<Value> {
        bail!("hook entry sync is not supported for {}", self.storage_key())
    }

    fn restore_hook_entry(
        &self,
        _path: &Path,
        _identity: &HookSourceMatch,
        _entry: &Value,
    ) -> Result<String> {
        bail!("hook entry restore is not supported for {}", self.storage_key())
    }

    fn hook_read_only_reason(&self, path: &Path) -> Option<&'static str> {
        self.managed_hook_path(path)
            .then_some("this hook source is read-only")
    }

    fn scan_hooks(
        &self,
        _ctx: &ProviderContext,
        _scanned_files: &mut HashSet<PathBuf>,
        _hooks: &mut Vec<HookRecord>,
        _warnings: &mut Vec<String>,
    ) {
    }

    fn scan_hook_source_for_review(
        &self,
        _path: &Path,
        _hooks: &mut Vec<HookRecord>,
        _warnings: &mut Vec<String>,
    ) -> bool {
        false
    }

    fn apply_hook_review_states(&self, _hooks: &mut [HookRecord], _ctx: &ProviderContext) {}

    fn managed_hook_path(&self, _path: &Path) -> bool {
        false
    }

    fn is_global_hook_path(&self, _path: &Path) -> bool {
        false
    }

    fn uses_tendi_hook_review_state(&self) -> bool {
        true
    }

    fn disables_hooks_from_config(&self, _value: &Value) -> bool {
        false
    }

    fn parse_hook_file(
        &self,
        _path: &Path,
        _trust_hash: &str,
        _hooks: &mut Vec<HookRecord>,
        _warnings: &mut Vec<String>,
    ) -> bool {
        false
    }

    fn hook_review_metadata(
        &self,
        _path: &Path,
        _event: &str,
        _group_index: usize,
        _handler_index: usize,
        _matcher: Option<&str>,
        _command: Option<&str>,
        _configured_timeout: Option<u64>,
        _is_async: bool,
        _status_message: Option<&str>,
        _additional_context_limit: Option<usize>,
    ) -> (Option<String>, Option<String>) {
        (None, None)
    }

    fn review_hook(&self, _hook: &HookRecord) -> Result<()> {
        bail!("this hook does not support review")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SkillProviderMetadata {
    pub allow_implicit_invocation: Option<bool>,
    pub enabled: Option<bool>,
    pub disable_model_invocation: Option<bool>,
    pub provider_visibility: SkillVisibility,
}

impl Default for SkillProviderMetadata {
    fn default() -> Self {
        Self {
            allow_implicit_invocation: None,
            enabled: None,
            disable_model_invocation: None,
            provider_visibility: SkillVisibility::Auto,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionCommand {
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionResumePlan {
    pub agent: AgentKind,
    pub display_name: String,
    pub command: SessionCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWriter {
    pub lock_path: PathBuf,
}

static SHARED: shared::SharedProvider = shared::SharedProvider;
static CODEX: codex::CodexProvider = codex::CodexProvider;
static CURSOR: cursor::CursorProvider = cursor::CursorProvider;
static CLAUDE: claude::ClaudeProvider = claude::ClaudeProvider;
static UNKNOWN: unknown::UnknownProvider = unknown::UnknownProvider;

pub(crate) fn all_providers() -> Vec<&'static dyn AgentProvider> {
    vec![&SHARED, &CODEX, &CURSOR, &CLAUDE, &UNKNOWN]
}

pub(crate) fn agent_provider(agent: AgentKind) -> &'static dyn AgentProvider {
    all_providers()
        .into_iter()
        .find(|provider| provider.kind() == agent)
        .unwrap_or(&UNKNOWN)
}

pub fn session_root_priority(root: &Path) -> u8 {
    all_providers()
        .into_iter()
        .find_map(|provider| provider.session_scan_priority(root))
        .unwrap_or(3)
}

pub(crate) fn agent_providers() -> Vec<&'static dyn AgentProvider> {
    all_providers()
        .into_iter()
        .filter(|provider| provider.discoverable())
        .collect()
}

pub(crate) fn skill_target_uses_shared_layout(target: &str) -> bool {
    if matches!(target, "shared" | "universal") {
        return true;
    }
    let canonical = canonical_skill_target_alias(target).unwrap_or(target);
    all_providers()
        .into_iter()
        .find(|provider| {
            provider
                .skill_target()
                .is_some_and(|skill_target| skill_target.id == canonical)
        })
        .is_some_and(|provider| provider.uses_shared_skill_layout())
}

pub(crate) fn session_roots(cwd: &Path) -> Vec<PathBuf> {
    let ctx = ProviderContext::new(cwd);
    agent_providers()
        .into_iter()
        .flat_map(|provider| provider.session_roots(&ctx))
        .collect()
}

pub fn plan_session_resume(session: &SessionRecord) -> Result<SessionResumePlan> {
    let Some(provider) = agent_providers()
        .into_iter()
        .find(|provider| provider.kind() == session.agent)
    else {
        bail!(
            "{} sessions cannot be resumed from a terminal",
            agent_display_name(session.agent)
        );
    };
    provider.validate_session_resume(session)?;
    let Some(command) = provider.resume_session_command(session) else {
        bail!(
            "{} does not expose a supported CLI resume command",
            provider
                .display_name()
                .unwrap_or_else(|| agent_display_name(session.agent))
        );
    };
    Ok(SessionResumePlan {
        agent: session.agent,
        display_name: provider
            .display_name()
            .unwrap_or_else(|| agent_display_name(session.agent))
            .to_string(),
        command,
    })
}

pub fn active_session_writer(session: &SessionRecord) -> Result<Option<SessionWriter>> {
    agent_provider(session.agent).active_session_writer(session)
}

pub fn accepts_session_app_url(url: &str) -> bool {
    agent_providers()
        .into_iter()
        .any(|provider| provider.accepts_session_app_url(url))
}

pub fn config_profile_key(agent: AgentKind) -> Option<&'static str> {
    agent_provider(agent).config_profile_key()
}

pub(crate) fn canonical_skill_target_alias(value: &str) -> Option<&'static str> {
    agent_providers().into_iter().find_map(|provider| {
        provider
            .skill_target_aliases()
            .iter()
            .find_map(|(alias, target)| (*alias == value).then_some(*target))
    })
}

pub fn apply_session_config_profile(
    agent: AgentKind,
    command: &mut SessionCommand,
    profile: &str,
) -> Result<()> {
    agent_provider(agent).apply_config_profile(command, profile)
}

pub fn parse_agent(value: &str) -> Result<AgentKind> {
    let normalized = value
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>();
    all_providers()
        .into_iter()
        .find(|provider| provider.matches_name(&normalized))
        .map(|provider| provider.kind())
        .ok_or_else(|| anyhow::anyhow!("unknown agent target {value}"))
}

fn absolute_project(session: &SessionRecord) -> Option<PathBuf> {
    session
        .project
        .as_ref()
        .filter(|path| path.is_absolute())
        .cloned()
}

fn agent_display_name(agent: AgentKind) -> &'static str {
    agent_provider(agent).display_name().unwrap_or_default()
}

fn push_skill_root(roots: &mut Vec<SkillRoot>, path: PathBuf, scope: &str, agent: AgentKind) {
    push_skill_root_with_plugin(roots, path, scope, agent, None, None);
}

fn push_skill_root_with_plugin(
    roots: &mut Vec<SkillRoot>,
    path: PathBuf,
    scope: &str,
    agent: AgentKind,
    plugin_id: Option<String>,
    plugin_enabled: Option<bool>,
) {
    if path.is_dir() && !roots.iter().any(|root| root.path == path) {
        roots.push(SkillRoot {
            path,
            scope: scope.to_string(),
            agent,
            plugin_id,
            plugin_enabled,
        });
    }
}

fn project_dirs(cwd: &Path) -> Vec<PathBuf> {
    let ancestors = cwd.ancestors().collect::<Vec<_>>();
    let root = ancestors
        .iter()
        .find(|path| path.join(".git").exists())
        .copied()
        .unwrap_or(cwd);

    let mut dirs = Vec::new();
    for ancestor in cwd.ancestors() {
        dirs.push(ancestor.to_path_buf());
        if ancestor == root {
            break;
        }
    }
    dirs.reverse();
    dirs
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{AgentKind, SessionRecord, codex, plan_session_resume, project_dirs};

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
    fn codex_provider_includes_plugin_skill_roots() {
        let root = temp_dir("tendi-codex-plugin-skills-test");
        let codex_home = root.join(".codex");
        let global_skills = codex_home.join("skills");
        let plugin_skills =
            codex_home.join("plugins/cache/openai-primary-runtime/documents/26.622.11653/skills");
        fs::create_dir_all(&global_skills).unwrap();
        fs::create_dir_all(plugin_skills.join("documents")).unwrap();
        fs::write(
            codex_home.join("config.toml"),
            "[plugins.\"documents@openai-primary-runtime\"]\nenabled = false\n",
        )
        .unwrap();
        fs::write(
            plugin_skills.join("documents/SKILL.md"),
            "---\nname: documents\ndescription: Documents\n---\n",
        )
        .unwrap();

        let roots = codex::codex_skill_roots(&codex_home, &project_dirs(&root), AgentKind::Codex);

        assert!(roots.iter().any(|root| {
            root.path == global_skills && root.scope == "global" && root.agent == AgentKind::Codex
        }));
        assert!(roots.iter().any(|root| {
            root.path == plugin_skills
                && root.scope == "plugin"
                && root.agent == AgentKind::Codex
                && root.plugin_id.as_deref() == Some("documents@openai-primary-runtime")
                && root.plugin_enabled == Some(false)
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_plugin_config_maps_enabled_state_by_plugin_id() {
        let root = temp_dir("tendi-codex-plugin-config-test");
        let codex_home = root.join(".codex");
        let plugin_skills = codex_home.join("plugins/cache/openai-bundled/browser/1.0.0/skills");
        fs::create_dir_all(&plugin_skills).unwrap();
        fs::write(
            codex_home.join("config.toml"),
            "[plugins.\"browser@openai-bundled\"]\nenabled = false\n",
        )
        .unwrap();

        assert_eq!(
            codex::codex_plugin_id_for_skill_root(&codex_home, &plugin_skills).as_deref(),
            Some("browser@openai-bundled")
        );
        assert_eq!(
            codex::codex_plugin_enabled_by_id(&codex_home)
                .get("browser@openai-bundled")
                .copied(),
            Some(false)
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_plugin_skill_roots_ignores_dependency_dirs() {
        let root = temp_dir("tendi-codex-plugin-skip-test");
        let codex_home = root.join(".codex");
        let plugin_skills = codex_home.join("plugins/cache/openai-bundled/browser/1.0.0/skills");
        let dependency_skills = codex_home
            .join("plugins/cache/openai-bundled/browser/1.0.0/scripts/node_modules/pkg/skills");
        fs::create_dir_all(&plugin_skills).unwrap();
        fs::create_dir_all(&dependency_skills).unwrap();

        let roots = codex::codex_plugin_skill_roots(&codex_home);

        assert_eq!(roots, vec![plugin_skills]);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resume_command_ignores_relative_project_paths() {
        let session = SessionRecord {
            id: "019eef10-7054-7a63-b9c8-fd16cc70cd53".to_string(),
            agent: AgentKind::Codex,
            title: None,
            project: Some(PathBuf::from("tendi")),
            repository: None,
            repository_url: None,
            logical_project_id: None,
            logical_project_name: None,
            path: PathBuf::from("/Users/test/.codex/sessions/session.jsonl"),
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
        };

        let plan = plan_session_resume(&session).unwrap();

        assert_eq!(plan.command.cwd, None);
        assert_eq!(
            plan.command.args,
            vec!["resume", "019eef10-7054-7a63-b9c8-fd16cc70cd53"]
        );
    }

    #[test]
    fn codex_writer_lock_path_supports_live_and_archived_sessions() {
        assert_eq!(
            codex::codex_thread_writer_lock_path(
                PathBuf::from("/Users/test/.codex/sessions/2026/08/24/session.jsonl").as_path(),
                "session-id"
            ),
            Some(PathBuf::from(
                "/Users/test/.codex/thread-writer-locks/session-id.lock"
            ))
        );
        assert_eq!(
            codex::codex_thread_writer_lock_path(
                PathBuf::from("/Users/test/.codex/archived_sessions/session.jsonl").as_path(),
                "session-id"
            ),
            Some(PathBuf::from(
                "/Users/test/.codex/thread-writer-locks/session-id.lock"
            ))
        );
        assert_eq!(
            codex::codex_thread_writer_lock_path(
                PathBuf::from("/Users/test/session.jsonl").as_path(),
                "session-id"
            ),
            None
        );
    }

    #[test]
    fn plan_session_resume_rejects_an_active_codex_writer() {
        let root = temp_dir("tendi-codex-resume-lock-test");
        let session_path = root.join(".codex/sessions/2026/08/24/session-id.jsonl");
        fs::create_dir_all(session_path.parent().unwrap()).unwrap();
        fs::write(&session_path, "").unwrap();
        let session = SessionRecord {
            id: "session-id".to_string(),
            agent: AgentKind::Codex,
            title: None,
            project: None,
            repository: None,
            repository_url: None,
            logical_project_id: None,
            logical_project_name: None,
            path: session_path,
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
        };
        let lock_path = codex::codex_thread_writer_lock_path(&session.path, &session.id).unwrap();
        fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        lock_file.try_lock().unwrap();

        let writer = codex::active_session_writer(&session)
            .unwrap()
            .expect("the held lock should be reported as an active writer");
        assert_eq!(writer.lock_path, lock_path);

        let error = plan_session_resume(&session).unwrap_err();

        assert!(error.to_string().contains("already has an active writer"));
        lock_file.unlock().unwrap();
        drop(lock_file);
        assert!(plan_session_resume(&session).is_ok());
        let _ = fs::remove_dir_all(root);
    }
}
