use std::fs;

use anyhow::{Context, ensure};
use serde_json::json;

use super::{antigravity_sessions as agy_sessions, *};
use crate::{hooks, mcp};

pub(super) struct AntigravityProvider;

const APP_BUNDLE: &str = "/Applications/Antigravity.app";
const CLI_BINARY: &str = ".local/bin/agy";
const CONFIG_HOME: &str = ".gemini/config";
const CLI_SKILLS: &str = ".gemini/antigravity-cli/skills";
const GEMINI_SKILLS: &str = ".gemini/skills";
const PROJECT_CUSTOMIZATIONS: &[&str] = &[".agents", ".agent", "_agents", "_agent"];
const INSTRUCTION_FILES: &[&str] = &["AGENTS.md", "GEMINI.md"];
const MCP_FILE: &str = "mcp_config.json";
const MCP_KEY: &str = "mcpServers";
const HOOK_FILE: &str = "hooks.json";

impl AgentProvider for AntigravityProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Antigravity
    }

    fn storage_key(&self) -> &'static str {
        "antigravity"
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("Antigravity")
    }

    fn matches_name(&self, normalized: &str) -> bool {
        matches!(normalized, "antigravity" | "antigravitycli" | "agy")
    }

    fn discoverable(&self) -> bool {
        true
    }

    fn app_bundle_path(&self) -> Option<&'static str> {
        Some(APP_BUNDLE)
    }

    fn executable_names(&self) -> &'static [&'static str] {
        &["agy"]
    }

    fn default_executable_path(&self, ctx: &ProviderContext) -> Option<PathBuf> {
        Some(ctx.home.as_ref()?.join(CLI_BINARY))
    }

    fn config_indicates_installation(&self) -> bool {
        false
    }

    fn config_dir(&self, ctx: &ProviderContext) -> Option<PathBuf> {
        Some(ctx.home.as_ref()?.join(CONFIG_HOME))
    }

    fn global_backup_root(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".gemini"))
    }

    fn projection_directories(&self) -> &'static [&'static str] {
        PROJECT_CUSTOMIZATIONS
    }

    fn projection_candidate_files(&self, domain: &str, ancestor: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if domain == "rules" {
            paths.extend(INSTRUCTION_FILES.iter().map(|name| ancestor.join(name)));
        }
        for directory in PROJECT_CUSTOMIZATIONS {
            let root = ancestor.join(directory);
            match domain {
                "skills" => paths.push(root.join("skills")),
                "rules" => {
                    paths.extend(INSTRUCTION_FILES.iter().map(|name| root.join(name)));
                    paths.push(root.join("rules"));
                }
                "hooks" => paths.push(root.join(HOOK_FILE)),
                "mcp" => paths.push(root.join(MCP_FILE)),
                _ => {}
            }
        }
        paths
    }

    fn global_projection_candidate_files(&self, domain: &str, home: &Path) -> Vec<PathBuf> {
        let config = home.join(CONFIG_HOME);
        match domain {
            "agents" => vec![
                config,
                home.join(CLI_BINARY),
                Path::new(APP_BUNDLE).join("Contents/Info.plist"),
            ],
            "skills" => vec![
                config.join("skills"),
                home.join(CLI_SKILLS),
                home.join(GEMINI_SKILLS),
            ],
            "rules" => INSTRUCTION_FILES
                .iter()
                .flat_map(|name| [home.join(".gemini").join(name), config.join(name)])
                .chain(std::iter::once(config.join("rules")))
                .collect(),
            "hooks" => vec![config.join(HOOK_FILE)],
            "mcp" => vec![config.join(MCP_FILE)],
            _ => Vec::new(),
        }
    }

    fn projection_candidate_is_file(&self, path: &Path) -> bool {
        path.ends_with(CLI_BINARY)
    }

    fn skill_targets(&self) -> &'static [ProviderSkillTarget] {
        &[
            ProviderSkillTarget {
                id: "antigravity",
                display_name: "Antigravity",
                listed: true,
            },
            // Keep explicit CLI commands targeting this directory working.
            ProviderSkillTarget {
                id: "antigravity-cli",
                display_name: "Antigravity CLI",
                listed: false,
            },
        ]
    }

    fn global_skill_root(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(CONFIG_HOME).join("skills"))
    }

    fn global_skill_root_for_target(&self, target: &str, home: &Path) -> Option<PathBuf> {
        if target == "antigravity-cli" {
            return Some(home.join(CLI_SKILLS));
        }
        self.global_skill_root(home)
    }

    fn project_skill_root(&self, cwd: &Path) -> Option<PathBuf> {
        Some(cwd.join(".agents/skills"))
    }

    fn skill_scope_for_root(&self, root: &Path, home: Option<&Path>) -> Option<&'static str> {
        if home.is_some_and(|home| {
            self.global_skill_root(home).as_deref() == Some(root)
                || root == home.join(CLI_SKILLS)
                || root == home.join(GEMINI_SKILLS)
        }) {
            return Some("global");
        }
        if root.parent().is_some_and(|parent| {
            PROJECT_CUSTOMIZATIONS[1..]
                .iter()
                .any(|name| parent.ends_with(name))
        }) {
            return Some("project");
        }
        None
    }

    fn skill_scan_depth(&self) -> usize {
        2
    }

    fn skill_install_target_id(&self, root: &Path) -> &'static str {
        if root.ends_with(CLI_SKILLS) || root.ends_with(GEMINI_SKILLS) {
            return "antigravity-cli";
        }
        self.storage_key()
    }

    fn supports_skill_visibility(&self) -> bool {
        false
    }

    fn skill_roots(&self, ctx: &ProviderContext) -> Vec<SkillRoot> {
        let mut roots = Vec::new();
        if let Some(home) = &ctx.home {
            for root in [
                home.join(CLI_SKILLS),
                home.join(GEMINI_SKILLS),
                home.join(CONFIG_HOME).join("skills"),
            ] {
                push_skill_root(&mut roots, root, "global", self.kind());
            }
        }
        for (_, root) in project_roots(ctx) {
            if !root.ends_with(".agents") {
                push_skill_root(&mut roots, root.join("skills"), "project", self.kind());
            }
        }
        roots
    }

    fn scan_rules(
        &self,
        ctx: &ProviderContext,
        rules_out: &mut Vec<RuleRecord>,
        warnings: &mut Vec<String>,
        order: &mut usize,
    ) {
        let mut scan_instructions = |root: &Path, scope| {
            for name in INSTRUCTION_FILES {
                rules::add_rule_file(
                    rules_out,
                    warnings,
                    order,
                    self.kind(),
                    name,
                    scope,
                    root.join(name),
                );
            }
            scan_modular_rules(&root.join("rules"), scope, rules_out, warnings, order);
        };
        if let Some(home) = &ctx.home {
            scan_instructions(&home.join(CONFIG_HOME), "global");
        }
        for (_, root) in project_roots(ctx) {
            scan_instructions(&root, "project");
        }
        for (root, scope) in ctx
            .home
            .iter()
            .map(|home| (home.join(".gemini"), "global"))
            .chain(
                ctx.project_dirs()
                    .iter()
                    .cloned()
                    .map(|path| (path, "project")),
            )
        {
            for name in INSTRUCTION_FILES {
                rules::add_rule_file(
                    rules_out,
                    warnings,
                    order,
                    self.kind(),
                    name,
                    scope,
                    root.join(name),
                );
            }
        }
    }

    fn scan_mcp(
        &self,
        ctx: &ProviderContext,
        servers: &mut Vec<McpServerRecord>,
        warnings: &mut Vec<String>,
        probe_cache: &mut McpProbeCache,
    ) -> Result<()> {
        let sources = self
            .config_dir(ctx)
            .into_iter()
            .map(|root| ("global".to_string(), root))
            .chain(project_roots(ctx).map(|(project, root)| (project.display().to_string(), root)));
        for (scope, root) in sources {
            mcp::scan_json_mcp(
                &root.join(MCP_FILE),
                self.kind(),
                &scope,
                &[MCP_KEY],
                mcp_transport,
                mcp_enabled,
                mcp_status,
                enrich_mcp,
                probe_cache,
                servers,
                warnings,
            );
        }
        Ok(())
    }

    fn probe_mcp(
        &self,
        request: &McpProbeRequest,
        current: &McpServerRecord,
        probe_cache: &mut McpProbeCache,
    ) -> Result<McpServerRecord> {
        validate_source(&request.path, MCP_FILE)?;
        mcp::probe_json_mcp_server(
            request,
            current,
            mcp_transport,
            mcp_enabled,
            mcp_status,
            enrich_mcp,
            probe_cache,
        )
    }

    fn set_mcp_enabled(&self, request: &McpSetEnabledRequest) -> Result<()> {
        validate_source(&request.path, MCP_FILE)?;
        ensure!(
            request.server_path.is_empty() || request.server_path == [MCP_KEY],
            "invalid Antigravity MCP server path"
        );
        mcp::set_json_server_enabled(request, &[MCP_KEY], |server, enabled| {
            server.insert("disabled".to_string(), Value::Bool(!enabled));
            true
        })
    }

    fn mcp_status_after_toggle(&self, enabled: bool) -> &'static str {
        if enabled { "configured" } else { "disabled" }
    }

    fn backup_mcp_entry(&self, path: &Path, server_path: &[String], name: &str) -> Result<Value> {
        validate_source(path, MCP_FILE)?;
        mcp::read_json_server_entry_at_path(path, server_path, name)
    }

    fn restore_mcp_entry(
        &self,
        path: &Path,
        server_path: &[String],
        name: &str,
        entry: &Value,
    ) -> Result<String> {
        validate_source(path, MCP_FILE)?;
        mcp::merge_json_server_entry_at_path(path, server_path, name, entry)
    }

    fn remove_mcp_entry(&self, path: &Path, server_path: &[String], name: &str) -> Result<String> {
        validate_source(path, MCP_FILE)?;
        mcp::remove_json_server_entry_at_path(path, server_path, name)
    }

    fn scan_hooks(
        &self,
        ctx: &ProviderContext,
        scanned_files: &mut HashSet<PathBuf>,
        hooks: &mut Vec<HookRecord>,
        warnings: &mut Vec<String>,
    ) {
        for root in self
            .config_dir(ctx)
            .into_iter()
            .chain(project_roots(ctx).map(|(_, root)| root))
        {
            hooks::scan_hook_file_once(
                &root.join(HOOK_FILE),
                self.kind(),
                scanned_files,
                hooks,
                warnings,
            );
        }
    }

    fn parse_hook_file(
        &self,
        path: &Path,
        source: &str,
        trust_hash: &str,
        hooks: &mut Vec<HookRecord>,
        warnings: &mut Vec<String>,
    ) -> bool {
        match hook_groups(source) {
            Ok(groups) => {
                hooks.extend(
                    groups.into_iter().filter_map(|(name, value)| {
                        named_hook_record(path, trust_hash, name, &value)
                    }),
                );
            }
            Err(error) => warnings.push(format!("{}: {error}", path.display())),
        }
        true
    }

    fn set_hooks_enabled(
        &self,
        requests: &[HookSetEnabledRequest],
        source: &str,
    ) -> Result<String> {
        validate_source(&requests[0].path, HOOK_FILE)?;
        edit_hooks(source, |groups| {
            for request in requests {
                let name = request
                    .name
                    .as_deref()
                    .context("Antigravity hook name is missing")?;
                let group = groups
                    .get_mut(name)
                    .and_then(Value::as_object_mut)
                    .with_context(|| format!("Antigravity hook {name} was not found"))?;
                group.insert("enabled".to_string(), Value::Bool(request.enabled));
            }
            Ok(())
        })
    }

    fn delete_hooks(&self, requests: &[HookDeleteRequest], source: &str) -> Result<String> {
        validate_source(&requests[0].path, HOOK_FILE)?;
        edit_hooks(source, |groups| {
            for request in requests {
                let name = request
                    .name
                    .as_deref()
                    .context("Antigravity hook name is missing")?;
                groups
                    .remove(name)
                    .with_context(|| format!("Antigravity hook {name} was not found"))?;
            }
            Ok(())
        })
    }

    fn is_global_hook_path(&self, path: &Path) -> bool {
        dirs::home_dir().is_some_and(|home| path == home.join(CONFIG_HOME).join(HOOK_FILE))
    }

    fn hook_source_preview(
        &self,
        source: &str,
        identity: &HookSourceMatch,
    ) -> Option<(Option<u32>, String)> {
        let name = identity.name.as_deref()?;
        let groups = hook_groups(source).ok()?;
        let group = groups.get(name)?;
        let needle = serde_json::to_string(name).ok()?;
        let line = hooks::line_number_for_needle(source, &needle);
        Some((
            line,
            serde_json::to_string_pretty(&json!({ name: group })).ok()?,
        ))
    }

    fn backup_hook_entry(&self, path: &Path, identity: &HookSourceMatch) -> Result<Value> {
        validate_source(path, HOOK_FILE)?;
        let name = identity
            .name
            .as_deref()
            .context("Antigravity hook name is missing")?;
        hook_groups(&fs::read_to_string(path)?)?
            .remove(name)
            .ok_or_else(|| {
                MissingSyncEntry(format!("Antigravity hook {name} was not found")).into()
            })
    }

    fn restore_hook_entry(
        &self,
        path: &Path,
        identity: &HookSourceMatch,
        entry: &Value,
    ) -> Result<String> {
        validate_source(path, HOOK_FILE)?;
        let name = identity
            .name
            .as_deref()
            .context("Antigravity hook name is missing")?;
        let source = if path.is_file() {
            fs::read_to_string(path)?
        } else {
            "{}\n".to_string()
        };
        edit_hooks(&source, |groups| {
            groups.insert(name.to_string(), entry.clone());
            Ok(())
        })
    }

    fn hook_review_resource_paths(&self, source: &Path) -> Result<Vec<PathBuf>> {
        let state =
            hooks::tendi_hook_review_state_path().context("Tendi data directory is unavailable")?;
        Ok(vec![source.to_path_buf(), state])
    }

    fn review_hook(&self, hook: &HookRecord) -> Result<()> {
        hooks::review_hook_with_tendi_state(hook)
    }

    fn scan_sessions(
        &self,
        ctx: &ProviderContext,
        records: &mut Vec<SessionRecord>,
        warnings: &mut Vec<String>,
        cache: Option<&SessionScanCache>,
    ) -> Result<()> {
        if let Some(home) = &ctx.home {
            for root in agy_sessions::data_roots(home) {
                agy_sessions::scan_root(&root, records, warnings, cache);
            }
        }
        Ok(())
    }

    fn session_roots(&self, ctx: &ProviderContext) -> Vec<PathBuf> {
        ctx.home
            .iter()
            .flat_map(|home| agy_sessions::data_roots(home))
            .flat_map(|root| [root.join("brain"), agy_sessions::summary_path(&root)])
            .collect()
    }

    fn is_session_candidate_path(&self, path: &Path) -> bool {
        agy_sessions::is_native_summary(path)
    }

    fn is_session_transcript_path(&self, path: &Path) -> bool {
        agy_sessions::Source::from_path(path).is_some()
    }

    fn session_path_role(&self, path: &Path) -> SessionPathRole {
        if self.is_session_transcript_path(path) {
            SessionPathRole::Transcript
        } else {
            SessionPathRole::Other
        }
    }

    fn session_watch_source_path(&self, path: &Path) -> Option<PathBuf> {
        agy_sessions::summary_watch_source(path)
    }

    fn scan_explicit_session_path(
        &self,
        path: &Path,
        records: &mut Vec<SessionRecord>,
        cache: Option<&SessionScanCache>,
    ) -> Result<bool> {
        if agy_sessions::is_native_summary(path) {
            let mut warnings = Vec::new();
            agy_sessions::scan_root(
                path.parent().expect("summary paths have a parent"),
                records,
                &mut warnings,
                cache,
            );
            if !warnings.is_empty() {
                bail!("{}", warnings.join("\n"));
            }
            return Ok(true);
        }
        agy_sessions::scan_path(path, records, cache)
    }

    fn session_id_from_path(&self, path: &Path) -> Option<String> {
        Some(agy_sessions::Source::from_path(path)?.id.to_owned())
    }

    fn recognizes_transcript(&self, value: &Value) -> bool {
        agy_sessions::recognizes(value)
    }

    fn session_line_requires_metadata_parse(
        &self,
        _prefix: &str,
        _meta: &SessionMetadata,
    ) -> Option<bool> {
        Some(true)
    }

    fn session_user_message(&self, value: &Value) -> Option<String> {
        agy_sessions::user_message(value).map(str::to_owned)
    }

    fn session_message_kind(&self, value: &Value) -> Option<SessionMessageKind> {
        agy_sessions::assistant_message(value).then_some(SessionMessageKind::Assistant)
    }

    fn update_session_metadata(
        &self,
        value: &Value,
        meta: &mut SessionMetadata,
        _usage: &mut BTreeMap<String, SessionTokenUsage>,
    ) {
        if let Some(time) = agy_sessions::timestamp(value) {
            sessions::apply_time_bounds(meta, time);
        }
    }

    fn analytics_capabilities(&self) -> AnalyticsCapabilities {
        AnalyticsCapabilities {
            token_usage: true,
            reasoning_tokens: false,
            explicit_runs: false,
            duration: false,
            rate_limit_history: false,
        }
    }

    fn analytics_parser_version(&self) -> u32 {
        // Reindex cached usage with the current model attribution rules.
        crate::analytics::ANALYTICS_PARSER_VERSION + 2
    }

    fn analytics_infers_single_model(&self) -> bool {
        // Checkpoints may report usage without identifying their model.
        false
    }

    fn parse_analytics_line(&self, line: &str, record: &mut SessionAnalyticsRecord) {
        agy_sessions::parse_analytics_line(line, record);
    }

    fn current_session_env_key(&self) -> Option<&'static str> {
        Some("ANTIGRAVITY_CONVERSATION_ID")
    }

    fn current_session_transcript(
        &self,
        ctx: &ProviderContext,
        id: &str,
        env: &BTreeMap<String, String>,
    ) -> Result<Option<PathBuf>> {
        ensure!(
            agy_sessions::valid_id(id),
            "invalid Antigravity conversation ID"
        );
        if let Some(root) = env
            .get("ANTIGRAVITY_APP_DATA_DIR")
            .map(Path::new)
            .filter(|root| root.is_absolute())
        {
            let path = agy_sessions::transcript_path(root, id);
            return Ok(path.is_file().then_some(path));
        }
        Ok(ctx
            .home
            .iter()
            .flat_map(|home| agy_sessions::data_roots(home))
            .map(|root| agy_sessions::transcript_path(&root, id))
            .find(|path| path.is_file()))
    }

    fn session_resumable(&self, path: &Path, id: &str) -> bool {
        agy_sessions::Source::from_path(path).is_some_and(|source| {
            source.id == id && dirs::home_dir().is_some_and(|home| source.is_native_cli(&home))
        })
    }

    fn resume_session_command(&self, session: &SessionRecord) -> Option<SessionCommand> {
        if !self.session_resumable(&session.path, &session.id) {
            return None;
        }
        let ctx = ProviderContext::new(session.project.as_deref().unwrap_or(Path::new(".")));
        Some(SessionCommand {
            executable: self.executable_path(&ctx)?.to_string_lossy().into_owned(),
            args: vec!["--conversation".to_owned(), session.id.clone()],
            cwd: absolute_project(session),
            env: Vec::new(),
        })
    }

    fn parse_transcript_value(&self, value: &Value, items: &mut Vec<TranscriptItem>) {
        agy_sessions::parse_step(value, items);
    }
}

fn project_roots(ctx: &ProviderContext) -> impl Iterator<Item = (&Path, PathBuf)> {
    ctx.project_dirs().iter().flat_map(|dir| {
        PROJECT_CUSTOMIZATIONS
            .iter()
            .map(|name| (dir.as_path(), dir.join(name)))
    })
}

fn scan_modular_rules(
    root: &Path,
    scope: &str,
    rules_out: &mut Vec<RuleRecord>,
    warnings: &mut Vec<String>,
    order: &mut usize,
) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md") && path.is_file())
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                warnings.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        let meta = crate::skills::parse_frontmatter(&text);
        let trigger = meta
            .as_ref()
            .and_then(|meta| meta.get("trigger"))
            .and_then(serde_yaml::Value::as_str);
        if matches!(
            trigger,
            Some("always_on" | "model_decision" | "glob" | "manual")
        ) {
            rules::add_rule_file(
                rules_out,
                warnings,
                order,
                AgentKind::Antigravity,
                "antigravity-rule",
                scope,
                path,
            );
        }
    }
}

fn validate_source(path: &Path, file: &str) -> Result<()> {
    let supported = path.parent().is_some_and(|parent| {
        parent.ends_with(CONFIG_HOME)
            || PROJECT_CUSTOMIZATIONS
                .iter()
                .any(|name| parent.ends_with(name))
    });
    ensure!(
        supported && path.file_name().is_some_and(|name| name == file),
        "invalid Antigravity source: {}",
        path.display()
    );
    Ok(())
}

fn mcp_transport(spec: &Value) -> Option<String> {
    if spec
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| !command.trim().is_empty())
    {
        return Some("stdio".to_string());
    }
    spec.get("serverUrl")
        .or_else(|| spec.get("url"))
        .and_then(Value::as_str)
        .filter(|url| !url.trim().is_empty())
        .map(|_| "http".to_string())
}

fn mcp_enabled(spec: &Value) -> bool {
    spec.get("disabled").and_then(Value::as_bool) != Some(true)
}

fn mcp_status(spec: &Value) -> String {
    AntigravityProvider
        .mcp_status_after_toggle(mcp_enabled(spec))
        .to_string()
}

fn enrich_mcp(
    _name: &str,
    spec: &Value,
    transport: &str,
    enabled: bool,
    base_dir: Option<&Path>,
    cache: &mut McpProbeCache,
) -> mcp::McpEnrichment {
    if !enabled {
        return mcp::McpEnrichment::default();
    }
    let mut enrichment = mcp::enrich_json_mcp_spec_with_headers_at_dir(
        spec,
        transport,
        enabled,
        &BTreeMap::new(),
        base_dir,
        cache,
    );
    let enabled_tools = spec
        .get("enabledTools")
        .and_then(Value::as_array)
        .filter(|tools| !tools.is_empty());
    let disabled_tools = spec.get("disabledTools").and_then(Value::as_array);
    enrichment.tools.retain(|tool| {
        enabled_tools.is_none_or(|tools| tools.iter().any(|name| name.as_str() == Some(&tool.name)))
            && disabled_tools
                .is_none_or(|tools| !tools.iter().any(|name| name.as_str() == Some(&tool.name)))
    });
    enrichment
}

fn hook_groups(source: &str) -> Result<serde_json::Map<String, Value>> {
    serde_json::from_str(source).context("Antigravity hooks must be a JSON object")
}

fn edit_hooks(
    source: &str,
    edit: impl FnOnce(&mut serde_json::Map<String, Value>) -> Result<()>,
) -> Result<String> {
    let mut groups = hook_groups(source)?;
    let before = Value::Object(groups.clone());
    edit(&mut groups)?;
    crate::json_edit::patch_json_text(source, &before, &Value::Object(groups))
}

fn named_hook_record(
    path: &Path,
    trust_hash: &str,
    name: String,
    value: &Value,
) -> Option<HookRecord> {
    let events = [
        "SessionStart",
        "PreToolUse",
        "PostToolUse",
        "PreInvocation",
        "PostInvocation",
        "Stop",
    ];
    let mut configured_events = Vec::new();
    let mut commands = Vec::new();
    let mut matchers = Vec::new();
    for event in events {
        let Some(groups) = value.get(event).and_then(Value::as_array) else {
            continue;
        };
        let before = commands.len();
        for group in groups {
            let mut collect = |handler: &Value| {
                if let Some(command) = handler.get("command").and_then(Value::as_str) {
                    commands.push(command.to_string());
                }
            };
            if let Some(handlers) = group.get("hooks").and_then(Value::as_array) {
                if let Some(matcher) = group
                    .get("matcher")
                    .and_then(Value::as_str)
                    .filter(|matcher| !matcher.is_empty())
                {
                    matchers.push(matcher);
                }
                handlers.iter().for_each(collect);
            } else {
                collect(group);
            }
        }
        if commands.len() > before {
            configured_events.push(event);
        }
    }
    if configured_events.is_empty() {
        return None;
    }
    Some(HookRecord {
        agent: AgentKind::Antigravity,
        name: Some(name),
        event: configured_events.join(", "),
        matcher: (!matchers.is_empty()).then(|| matchers.join(", ")),
        hook_type: Some("command".to_string()),
        command: Some(commands.join("\n")),
        url: None,
        prompt: None,
        filter: None,
        status_message: None,
        enabled: value
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        path: path.to_path_buf(),
        trust_hash: trust_hash.to_string(),
        needs_review: false,
        read_only_reason: None,
        provider_review_key: None,
        provider_current_hash: None,
    })
}
