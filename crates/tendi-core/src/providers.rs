use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use toml::Value as TomlValue;
use walkdir::WalkDir;

use crate::{
    rules::{self, RuleRecord},
    sessions::{self, SessionRecord, SessionScanCache},
    skills::{AgentKind, SkillRoot},
};

pub(crate) struct ProviderContext {
    pub home: Option<PathBuf>,
    project_dirs: Vec<PathBuf>,
}

impl ProviderContext {
    pub(crate) fn new(cwd: &Path) -> Self {
        Self {
            home: dirs::home_dir(),
            project_dirs: project_dirs(cwd),
        }
    }

    pub(crate) fn project_dirs(&self) -> &[PathBuf] {
        &self.project_dirs
    }
}

pub(crate) trait AgentProvider: Sync {
    fn kind(&self) -> AgentKind;

    fn display_name(&self) -> Option<&'static str> {
        None
    }

    fn executable_names(&self) -> &'static [&'static str] {
        &[]
    }

    fn config_dir(&self, _ctx: &ProviderContext) -> Option<PathBuf> {
        None
    }

    fn skill_roots(&self, _ctx: &ProviderContext) -> Vec<SkillRoot> {
        Vec::new()
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

struct SharedProvider;
struct CodexProvider;
struct CursorProvider;
struct ClaudeProvider;

static SHARED: SharedProvider = SharedProvider;
static CODEX: CodexProvider = CodexProvider;
static CURSOR: CursorProvider = CursorProvider;
static CLAUDE: ClaudeProvider = ClaudeProvider;

pub(crate) fn all_providers() -> Vec<&'static dyn AgentProvider> {
    vec![&SHARED, &CODEX, &CURSOR, &CLAUDE]
}

pub(crate) fn agent_providers() -> Vec<&'static dyn AgentProvider> {
    all_providers()
        .into_iter()
        .filter(|provider| provider.display_name().is_some())
        .collect()
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

impl AgentProvider for SharedProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Shared
    }

    fn skill_roots(&self, ctx: &ProviderContext) -> Vec<SkillRoot> {
        let mut roots = Vec::new();
        if let Some(home) = &ctx.home {
            push_skill_root(
                &mut roots,
                home.join(".agents/skills"),
                "global",
                self.kind(),
            );
        }
        for dir in ctx.project_dirs() {
            push_skill_root(
                &mut roots,
                dir.join(".agents/skills"),
                "project",
                self.kind(),
            );
        }
        roots
    }
}

impl AgentProvider for CodexProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("Codex")
    }

    fn executable_names(&self) -> &'static [&'static str] {
        &["codex"]
    }

    fn config_dir(&self, ctx: &ProviderContext) -> Option<PathBuf> {
        Some(codex_home(ctx))
    }

    fn skill_roots(&self, ctx: &ProviderContext) -> Vec<SkillRoot> {
        let home = codex_home(ctx);
        codex_skill_roots(&home, ctx.project_dirs(), self.kind())
    }

    fn scan_sessions(
        &self,
        ctx: &ProviderContext,
        sessions_out: &mut Vec<SessionRecord>,
        warnings: &mut Vec<String>,
        cache: Option<&SessionScanCache>,
    ) -> Result<()> {
        let root = codex_home(ctx);
        sessions::scan_codex_index(&root.join("session_index.jsonl"), sessions_out, warnings)?;
        sessions::scan_codex_jsonl(&root.join("sessions"), sessions_out, cache);
        sessions::scan_codex_jsonl(&root.join("archived_sessions"), sessions_out, cache);
        Ok(())
    }

    fn session_roots(&self, ctx: &ProviderContext) -> Vec<PathBuf> {
        let root = codex_home(ctx);
        vec![
            root.join("session_index.jsonl"),
            root.join("sessions"),
            root.join("archived_sessions"),
        ]
    }

    fn scan_rules(
        &self,
        ctx: &ProviderContext,
        rules_out: &mut Vec<RuleRecord>,
        warnings: &mut Vec<String>,
        order: &mut usize,
    ) {
        let codex_home = codex_home(ctx);
        rules::add_first_rule_file(
            rules_out,
            warnings,
            order,
            self.kind(),
            "global",
            vec![
                (
                    "AGENTS.override.md".to_string(),
                    codex_home.join("AGENTS.override.md"),
                ),
                ("AGENTS.md".to_string(), codex_home.join("AGENTS.md")),
            ],
        );

        let fallback_names = codex_project_doc_fallbacks(ctx);
        for dir in ctx.project_dirs() {
            let mut candidates = vec![
                (
                    "AGENTS.override.md".to_string(),
                    dir.join("AGENTS.override.md"),
                ),
                ("AGENTS.md".to_string(), dir.join("AGENTS.md")),
            ];
            candidates.extend(
                fallback_names
                    .iter()
                    .map(|name| (name.clone(), dir.join(name))),
            );
            rules::add_first_rule_file(
                rules_out,
                warnings,
                order,
                self.kind(),
                "project",
                candidates,
            );
        }
    }

    fn resume_session_command(&self, session: &SessionRecord) -> Option<SessionCommand> {
        let project = absolute_project(session);
        let mut args = Vec::new();
        if let Some(project) = project.as_ref() {
            args.push("-C".to_string());
            args.push(project.display().to_string());
        }
        args.push("resume".to_string());
        args.push(session.id.clone());
        Some(SessionCommand {
            executable: "codex".to_string(),
            args,
            cwd: project,
            env: Vec::new(),
        })
    }
}

impl AgentProvider for CursorProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Cursor
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("Cursor")
    }

    fn executable_names(&self) -> &'static [&'static str] {
        &["cursor"]
    }

    fn config_dir(&self, ctx: &ProviderContext) -> Option<PathBuf> {
        ctx.home.as_ref().map(|home| home.join(".cursor"))
    }

    fn skill_roots(&self, ctx: &ProviderContext) -> Vec<SkillRoot> {
        let mut roots = Vec::new();
        if let Some(home) = &ctx.home {
            push_skill_root(
                &mut roots,
                home.join(".cursor/skills"),
                "global",
                self.kind(),
            );
        }
        for dir in ctx.project_dirs() {
            push_skill_root(
                &mut roots,
                dir.join(".cursor/skills"),
                "project",
                self.kind(),
            );
        }
        roots
    }

    fn scan_sessions(
        &self,
        ctx: &ProviderContext,
        sessions_out: &mut Vec<SessionRecord>,
        _warnings: &mut Vec<String>,
        cache: Option<&SessionScanCache>,
    ) -> Result<()> {
        if let Some(home) = &ctx.home {
            sessions::scan_cursor_meta(
                &home.join(".cursor/acp-sessions"),
                sessions_out,
                self.kind(),
                cache,
            );
            sessions::scan_cursor_meta(
                &home.join(".cursor/chats"),
                sessions_out,
                self.kind(),
                cache,
            );
            sessions::scan_cursor_agent_transcripts(
                &home.join(".cursor/projects"),
                sessions_out,
                cache,
            );
        }
        Ok(())
    }

    fn session_roots(&self, ctx: &ProviderContext) -> Vec<PathBuf> {
        let Some(home) = &ctx.home else {
            return Vec::new();
        };
        vec![
            home.join(".cursor/acp-sessions"),
            home.join(".cursor/chats"),
            home.join(".cursor/projects"),
        ]
    }

    fn scan_rules(
        &self,
        ctx: &ProviderContext,
        rules_out: &mut Vec<RuleRecord>,
        warnings: &mut Vec<String>,
        order: &mut usize,
    ) {
        for dir in ctx.project_dirs() {
            rules::add_rule_file(
                rules_out,
                warnings,
                order,
                self.kind(),
                "AGENTS.md",
                "project",
                dir.join("AGENTS.md"),
            );
            rules::add_rule_tree(
                rules_out,
                warnings,
                order,
                self.kind(),
                "cursor-rule",
                "project",
                &dir.join(".cursor/rules"),
                Some("mdc"),
                6,
            );
        }
    }

    fn resume_session_command(&self, session: &SessionRecord) -> Option<SessionCommand> {
        let project = absolute_project(session);
        let mut args = vec![
            "agent".to_string(),
            "--resume".to_string(),
            session.id.clone(),
        ];
        if let Some(project) = project.as_ref() {
            args.push("--workspace".to_string());
            args.push(project.display().to_string());
        }
        Some(SessionCommand {
            executable: "cursor".to_string(),
            args,
            cwd: project,
            env: Vec::new(),
        })
    }
}

impl AgentProvider for ClaudeProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("Claude Code")
    }

    fn executable_names(&self) -> &'static [&'static str] {
        &["claude"]
    }

    fn config_dir(&self, ctx: &ProviderContext) -> Option<PathBuf> {
        ctx.home.as_ref().map(|home| home.join(".claude"))
    }

    fn skill_roots(&self, ctx: &ProviderContext) -> Vec<SkillRoot> {
        let mut roots = Vec::new();
        if let Some(home) = &ctx.home {
            push_skill_root(
                &mut roots,
                home.join(".claude/skills"),
                "global",
                self.kind(),
            );
        }
        for dir in ctx.project_dirs() {
            push_skill_root(
                &mut roots,
                dir.join(".claude/skills"),
                "project",
                self.kind(),
            );
        }
        roots
    }

    fn scan_sessions(
        &self,
        ctx: &ProviderContext,
        sessions_out: &mut Vec<SessionRecord>,
        _warnings: &mut Vec<String>,
        cache: Option<&SessionScanCache>,
    ) -> Result<()> {
        if let Some(home) = &ctx.home {
            sessions::scan_claude_projects(&home.join(".claude/projects"), sessions_out, cache);
        }
        Ok(())
    }

    fn session_roots(&self, ctx: &ProviderContext) -> Vec<PathBuf> {
        ctx.home
            .as_ref()
            .map(|home| vec![home.join(".claude/projects")])
            .unwrap_or_default()
    }

    fn scan_rules(
        &self,
        ctx: &ProviderContext,
        rules_out: &mut Vec<RuleRecord>,
        warnings: &mut Vec<String>,
        order: &mut usize,
    ) {
        if let Some(home) = &ctx.home {
            rules::add_rule_file(
                rules_out,
                warnings,
                order,
                self.kind(),
                "CLAUDE.md",
                "global",
                home.join(".claude/CLAUDE.md"),
            );
            rules::add_rule_tree(
                rules_out,
                warnings,
                order,
                self.kind(),
                "claude-rule",
                "global",
                &home.join(".claude/rules"),
                Some("md"),
                6,
            );
        }

        for dir in ctx.project_dirs() {
            rules::add_rule_file(
                rules_out,
                warnings,
                order,
                self.kind(),
                "CLAUDE.md",
                "project",
                dir.join("CLAUDE.md"),
            );
            rules::add_rule_file(
                rules_out,
                warnings,
                order,
                self.kind(),
                ".claude/CLAUDE.md",
                "project",
                dir.join(".claude/CLAUDE.md"),
            );
            rules::add_rule_file(
                rules_out,
                warnings,
                order,
                self.kind(),
                "CLAUDE.local.md",
                "local",
                dir.join("CLAUDE.local.md"),
            );
            rules::add_rule_tree(
                rules_out,
                warnings,
                order,
                self.kind(),
                "claude-rule",
                "project",
                &dir.join(".claude/rules"),
                Some("md"),
                6,
            );
        }

        rules::add_rule_file(
            rules_out,
            warnings,
            order,
            self.kind(),
            "managed-CLAUDE.md",
            "managed",
            PathBuf::from("/Library/Application Support/ClaudeCode/CLAUDE.md"),
        );
    }

    fn resume_session_command(&self, session: &SessionRecord) -> Option<SessionCommand> {
        let project = absolute_project(session);
        Some(SessionCommand {
            executable: "claude".to_string(),
            args: vec!["--resume".to_string(), session.id.clone()],
            cwd: project,
            env: Vec::new(),
        })
    }
}

fn absolute_project(session: &SessionRecord) -> Option<PathBuf> {
    session
        .project
        .as_ref()
        .filter(|path| path.is_absolute())
        .cloned()
}

fn agent_display_name(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Codex => "Codex",
        AgentKind::Cursor => "Cursor",
        AgentKind::Claude => "Claude Code",
        AgentKind::Shared => "Shared",
        AgentKind::Unknown => "Unknown",
    }
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

fn codex_home(ctx: &ProviderContext) -> PathBuf {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| ctx.home.as_ref().map(|home| home.join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

fn codex_skill_roots(home: &Path, project_dirs: &[PathBuf], agent: AgentKind) -> Vec<SkillRoot> {
    let mut roots = Vec::new();
    push_skill_root(&mut roots, home.join("skills"), "global", agent);
    let plugin_enabled = codex_plugin_enabled_by_id(home);
    for root in codex_plugin_skill_roots(home) {
        let plugin_id = codex_plugin_id_for_skill_root(home, &root);
        let enabled = plugin_id
            .as_ref()
            .and_then(|id| plugin_enabled.get(id).copied());
        push_skill_root_with_plugin(&mut roots, root, "plugin", agent, plugin_id, enabled);
    }
    for dir in project_dirs {
        push_skill_root(&mut roots, dir.join(".codex/skills"), "project", agent);
    }
    roots
}

fn codex_plugin_enabled_by_id(codex_home: &Path) -> BTreeMap<String, bool> {
    let Ok(text) = fs::read_to_string(codex_home.join("config.toml")) else {
        return BTreeMap::new();
    };
    let Ok(value) = toml::from_str::<TomlValue>(&text) else {
        return BTreeMap::new();
    };
    let Some(plugins) = value.get("plugins").and_then(TomlValue::as_table) else {
        return BTreeMap::new();
    };
    plugins
        .iter()
        .filter_map(|(id, value)| {
            value
                .get("enabled")
                .and_then(TomlValue::as_bool)
                .map(|enabled| (id.to_string(), enabled))
        })
        .collect()
}

fn codex_plugin_id_for_skill_root(codex_home: &Path, skill_root: &Path) -> Option<String> {
    let relative = skill_root
        .strip_prefix(codex_home.join("plugins/cache"))
        .ok()?;
    let mut parts = relative
        .components()
        .filter_map(|part| part.as_os_str().to_str());
    let marketplace = parts.next()?;
    let plugin = parts.next()?;
    Some(format!("{plugin}@{marketplace}"))
}

fn codex_plugin_skill_roots(codex_home: &Path) -> Vec<PathBuf> {
    let cache = codex_home.join("plugins/cache");
    if !cache.is_dir() {
        return Vec::new();
    }

    let mut roots = WalkDir::new(cache)
        .follow_links(false)
        .max_depth(5)
        .into_iter()
        .filter_entry(|entry| !is_skipped_plugin_entry(entry.path()))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_dir() && entry.file_name() == "skills")
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    roots.sort();
    roots
}

fn is_skipped_plugin_entry(path: &Path) -> bool {
    path.components().any(|part| {
        part.as_os_str().to_str().is_some_and(|value| {
            matches!(
                value,
                ".git" | "node_modules" | "dist" | "build" | "__pycache__"
            )
        })
    })
}

fn codex_project_doc_fallbacks(ctx: &ProviderContext) -> Vec<String> {
    let mut values = Vec::new();
    collect_codex_fallbacks_from_config(&codex_home(ctx).join("config.toml"), &mut values);
    for dir in ctx.project_dirs() {
        collect_codex_fallbacks_from_config(&dir.join(".codex/config.toml"), &mut values);
    }
    values
}

fn collect_codex_fallbacks_from_config(path: &Path, values: &mut Vec<String>) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = toml::from_str::<TomlValue>(&text) else {
        return;
    };
    let Some(items) = value
        .get("project_doc_fallback_filenames")
        .and_then(TomlValue::as_array)
    else {
        return;
    };
    values.clear();
    values.extend(
        items
            .iter()
            .filter_map(TomlValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.contains('/'))
            .map(str::to_string),
    );
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

    use super::{
        AgentKind, SessionRecord, codex_plugin_enabled_by_id, codex_plugin_id_for_skill_root,
        codex_plugin_skill_roots, codex_skill_roots, plan_session_resume, project_dirs,
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

        let roots = codex_skill_roots(&codex_home, &project_dirs(&root), AgentKind::Codex);

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
            codex_plugin_id_for_skill_root(&codex_home, &plugin_skills).as_deref(),
            Some("browser@openai-bundled")
        );
        assert_eq!(
            codex_plugin_enabled_by_id(&codex_home)
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

        let roots = codex_plugin_skill_roots(&codex_home);

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
}
