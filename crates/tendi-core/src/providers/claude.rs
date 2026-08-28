use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Result, bail};
use serde_json::Value;
use walkdir::WalkDir;

use crate::transcript::{
    TranscriptItem, attach_tool_result, collect_message_content, compact_time,
    extract_content_text, extract_duration_ms, extract_thinking_text, extract_tool_command,
    parse_timestamp_ms, push_item, push_tool_item, summarize_tool_call,
};
use crate::{
    analytics::AnalyticsCapabilities,
    skills::SkillVisibility,
};

use super::*;

pub(super) struct ClaudeProvider;

const CLAUDE_USER_MCP_SOURCE_KEY: &str = "__claude_user_mcp__.json";
const CLAUDE_SKILL_FRONTMATTER_KEY: &str = "disable-model-invocation";

fn infer_mcp_transport(spec: &Value) -> Option<String> {
    spec.get("transport")
        .or_else(|| spec.get("type"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| spec.get("command").and_then(Value::as_str).map(|_| "stdio".to_string()))
        .or_else(|| {
            spec.get("url").and_then(Value::as_str).map(|url| {
                if url.contains("/sse") { "sse" } else { "http" }.to_string()
            })
        })
}

fn infer_mcp_status(spec: &Value) -> String {
    if infer_mcp_enabled(spec) {
        "configured"
    } else {
        "disabled"
    }
    .to_string()
}

fn infer_mcp_enabled(spec: &Value) -> bool {
    !spec
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && spec.get("enabled").and_then(Value::as_bool).unwrap_or(true)
}

fn scan_claude_project_mcp(
    path: &Path,
    servers: &mut Vec<McpServerRecord>,
    warnings: &mut Vec<String>,
) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    let Some(projects) = value.get("projects").and_then(Value::as_object) else {
        return;
    };
    for project in projects.keys() {
        crate::mcp::scan_json_mcp_at_path(
            path,
            AgentKind::Claude,
            project,
            &[
                "projects".to_string(),
                project.clone(),
                "mcpServers".to_string(),
            ],
            infer_mcp_transport,
            infer_mcp_enabled,
            infer_mcp_status,
            servers,
            warnings,
        );
    }
}

fn update_mcp_server(spec: &mut serde_json::Map<String, Value>, enabled: bool) -> bool {
    let has_disabled = spec.contains_key("disabled");
    let has_enabled = spec.contains_key("enabled");
    if has_disabled || !has_enabled {
        spec.insert("disabled".to_string(), Value::Bool(!enabled));
    }
    if has_enabled {
        spec.insert("enabled".to_string(), Value::Bool(enabled));
    }
    true
}

pub(super) fn matches_name(normalized: &str) -> bool {
    matches!(normalized, "claude" | "claudecode")
}

fn extract_model(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    value
        .pointer("/message/model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

fn extract_token_usage(value: &Value) -> Option<(String, crate::sessions::SessionTokenUsage)> {
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
    (total_tokens > 0).then_some((
        message_id,
        crate::sessions::SessionTokenUsage {
            input_tokens,
            cached_input_tokens,
            output_tokens,
            reasoning_output_tokens: 0,
            total_tokens,
        },
    ))
}

fn valid_resume_query(value: &str) -> bool {
    let mut has_session = false;
    for parameter in value.split('&') {
        let Some((key, value)) = parameter.split_once('=') else {
            return false;
        };
        let valid_value = !value.is_empty()
            && value.len() <= 4096
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'%' | b'+')
            });
        if !valid_value {
            return false;
        }
        match key {
            "session" if value.len() <= 128 => has_session = true,
            "cwd" => {}
            _ => return false,
        }
    }
    has_session
}

pub(crate) fn scan_claude_plugin_hooks(
    root: &Path,
    hooks: &mut Vec<HookRecord>,
    warnings: &mut Vec<String>,
) {
    if !root.is_dir() {
        return;
    }
    for entry in WalkDir::new(root)
        .follow_links(true)
        .max_depth(7)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "hooks.json")
    {
        crate::hooks::scan_hook_file(entry.path(), AgentKind::Claude, hooks, warnings);
    }
}

pub(crate) fn scan_claude_managed_dropins(
    root: &Path,
    hooks: &mut Vec<HookRecord>,
    warnings: &mut Vec<String>,
) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut paths = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        crate::hooks::scan_hook_file(&path, AgentKind::Claude, hooks, warnings);
    }
}

pub(crate) fn scan_claude_component_hooks(
    root: &Path,
    hooks: &mut Vec<HookRecord>,
    warnings: &mut Vec<String>,
) {
    if !root.is_dir() {
        return;
    }
    for entry in WalkDir::new(root)
        .follow_links(true)
        .max_depth(7)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().and_then(|value| value.to_str()) == Some("md")
        })
    {
        scan_claude_component_file(entry.path(), hooks, warnings);
    }
}

pub(crate) fn scan_claude_component_file(
    path: &Path,
    hooks: &mut Vec<HookRecord>,
    warnings: &mut Vec<String>,
) {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return;
        }
    };
    let Some(frontmatter) = yaml_frontmatter(&text) else {
        return;
    };
    let value = match serde_yaml::from_str::<serde_yaml::Value>(frontmatter)
        .ok()
        .and_then(|value| serde_json::to_value(value).ok())
    {
        Some(value) => value,
        None => return,
    };
    if value.get("hooks").is_none() {
        return;
    }
    let trust_hash = match crate::fsutil::sha256_file(path) {
        Ok(hash) => hash,
        Err(err) => {
            warnings.push(format!("{}: {err:#}", path.display()));
            return;
        }
    };
    crate::hooks::collect_hooks_from_value(AgentKind::Claude, path, &trust_hash, &value, hooks);
}

fn yaml_frontmatter(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

pub(super) fn apply_config_profile(command: &mut SessionCommand, profile: &str) -> Result<()> {
    let path = crate::config::config_profile_path(crate::skills::AgentKind::Claude, profile)?;
    if !path.is_file() {
        bail!("Claude Code profile not found: {profile}");
    }
    command
        .args
        .splice(0..0, ["--settings".to_string(), path.display().to_string()]);
    Ok(())
}

pub(super) fn parse_transcript(value: &Value, items: &mut Vec<TranscriptItem>) {
    collect_claude_item(value, items);
}

pub(super) fn tool_payloads(value: &Value) -> Vec<(&Value, Evidence)> {
    let Some(content) = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    content
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|item| {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())?;
            Some((
                item,
                Evidence {
                    kind: name.to_string(),
                    text: crate::session_skills::summarize_evidence(item),
                    time: value
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
            ))
        })
        .collect()
}

pub(super) fn may_contain_search_message(line: &str) -> bool {
    let hint = crate::transcript::search_json_hint(line);
    matches!(
        crate::transcript::json_string_hint(hint, "\"type\""),
        Some("user" | "assistant")
    ) && !hint.contains("\"tool_result\"")
}

fn collect_claude_item(value: &Value, items: &mut Vec<TranscriptItem>) {
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return;
    };
    let time = value
        .get("timestamp")
        .and_then(Value::as_str)
        .map(compact_time);
    let timestamp_ms = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_timestamp_ms);

    if kind == "user" || kind == "assistant" {
        let content = value
            .get("message")
            .and_then(|message| message.get("content"));
        if kind == "user" && attach_claude_tool_results(value, content, items, timestamp_ms) {
            return;
        }
        if kind == "assistant" {
            if let Some(body) = extract_thinking_text(content) {
                push_item(items, "thinking", body, None, time.clone());
            }
        }
        collect_message_content(content, items, kind, time.clone());

        if let Some(Value::Array(content_items)) = content {
            for item in content_items {
                if item.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|name| !name.trim().is_empty())
                        .map(str::to_string);
                    push_tool_item(
                        items,
                        "tool",
                        summarize_tool_call(item),
                        name,
                        time.clone(),
                        extract_tool_command(item),
                        None,
                        extract_duration_ms(item, None),
                        item.get("id").and_then(Value::as_str).map(str::to_string),
                        timestamp_ms,
                    );
                }
            }
        }
    }

    if let Some(result) = value.get("toolUseResult") {
        let Some(body) = extract_content_text(Some(result)) else {
            return;
        };
        let call_id = value
            .get("toolUseID")
            .or_else(|| value.get("tool_use_id"))
            .or_else(|| value.get("toolUseId"))
            .and_then(Value::as_str);
        let duration_ms = extract_duration_ms(value, None);
        attach_tool_result(items, call_id, Some(body), duration_ms, timestamp_ms);
    }
}

#[cfg(test)]
pub(crate) fn collect_transcript_item(value: &Value, items: &mut Vec<TranscriptItem>) {
    collect_claude_item(value, items);
}

fn attach_claude_tool_results(
    value: &Value,
    content: Option<&Value>,
    items: &mut Vec<TranscriptItem>,
    timestamp_ms: Option<i64>,
) -> bool {
    let Some(Value::Array(content_items)) = content else {
        return false;
    };
    let mut handled = false;
    for item in content_items {
        if item.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        handled = true;
        let Some(body) = extract_content_text(item.get("content"))
            .or_else(|| {
                value
                    .get("toolUseResult")
                    .and_then(|result| extract_content_text(Some(result)))
            })
        else {
            continue;
        };
        let call_id = item
            .get("tool_use_id")
            .or_else(|| item.get("toolUseID"))
            .or_else(|| item.get("toolUseId"))
            .and_then(Value::as_str);
        let duration_ms = extract_duration_ms(value, Some(&body));
        attach_tool_result(items, call_id, Some(body), duration_ms, timestamp_ms);
    }
    handled
}
impl super::AgentProvider for ClaudeProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn storage_key(&self) -> &'static str {
        "claude"
    }

    fn discoverable(&self) -> bool {
        true
    }

    fn global_skill_root(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".claude/skills"))
    }

    fn backup_global_source_key(&self, path: &Path) -> Option<String> {
        let home = dirs::home_dir()?;
        if path == home.join(".claude.json") {
            return Some(CLAUDE_USER_MCP_SOURCE_KEY.to_string());
        }
        self.global_backup_root(&home)
            .and_then(|root| path.strip_prefix(root).ok())
            .and_then(|relative| relative.to_str())
            .map(str::to_string)
    }

    fn restore_global_source_path(&self, relative: &str) -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        if relative == CLAUDE_USER_MCP_SOURCE_KEY {
            return Some(home.join(".claude.json"));
        }
        let relative = Path::new(relative);
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return None;
        }
        Some(self.global_backup_root(&home)?.join(relative))
    }

    fn project_skill_root(&self, cwd: &Path) -> Option<PathBuf> {
        Some(cwd.join(".claude/skills"))
    }

    fn skill_target(&self) -> Option<ProviderSkillTarget> {
        Some(ProviderSkillTarget {
            id: "claude-code",
            display_name: "Claude Code",
        })
    }

    fn config_profile_path(&self, home: &Path, _agent_home: &Path, name: &str) -> Option<PathBuf> {
        Some(
            home.join(".claude/tendi-profiles")
                .join(format!("{name}.settings.json")),
        )
    }

    fn config_profile_format(&self) -> Option<&'static str> {
        Some("json")
    }

    fn config_files(&self, home: &Path, agent_home: &Path) -> Vec<crate::config::AgentConfigFile> {
        let base_path = home.join(".claude/settings.json");
        let mut configs = vec![
            self.config_file_for_path(home, agent_home, &base_path)
                .expect("Claude provider must resolve its base config path"),
        ];
        configs.extend(
            crate::config::profile_paths_for_root(
                &home.join(".claude/tendi-profiles"),
                ".settings.json",
            )
            .into_iter()
            .filter_map(|path| self.config_file_for_path(home, agent_home, &path)),
        );
        configs
    }

    fn config_file_for_path(
        &self,
        home: &Path,
        _agent_home: &Path,
        path: &Path,
    ) -> Option<crate::config::AgentConfigFile> {
        let base_path = home.join(".claude/settings.json");
        if path == base_path {
            return Some(crate::config::AgentConfigFile {
                agent: self.kind(),
                label: "Claude".to_string(),
                path: base_path.clone(),
                format: "json".to_string(),
                exists: base_path.is_file(),
                updated_at: None,
                profile: None,
            });
        }

        let profile = path.file_name()?.to_str()?.strip_suffix(".settings.json")?;
        crate::config::validate_profile_name(profile).ok()?;
        let expected_path = home
            .join(".claude/tendi-profiles")
            .join(format!("{profile}.settings.json"));
        (path == expected_path).then_some(crate::config::AgentConfigFile {
            agent: self.kind(),
            label: format!("Claude Code / {profile}"),
            path: expected_path.clone(),
            format: "json".to_string(),
            exists: expected_path.is_file(),
            updated_at: None,
            profile: Some(profile.to_string()),
        })
    }

    fn config_order(&self) -> usize {
        1
    }

    fn matches_name(&self, normalized: &str) -> bool {
        matches_name(normalized)
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("Claude Code")
    }

    fn skill_visibility_metadata(
        &self,
        _skill_dir: &Path,
        _skill_file: &Path,
        frontmatter: Option<&serde_yaml::Value>,
    ) -> Result<SkillProviderMetadata> {
        let disable_model_invocation = frontmatter
            .and_then(|value| value.get(CLAUDE_SKILL_FRONTMATTER_KEY))
            .and_then(serde_yaml::Value::as_bool);
        Ok(SkillProviderMetadata {
            allow_implicit_invocation: None,
            enabled: None,
            disable_model_invocation,
            provider_visibility: if disable_model_invocation == Some(true) {
                SkillVisibility::Manual
            } else {
                SkillVisibility::Auto
            },
        })
    }

    fn skill_frontmatter_satisfies(
        &self,
        meta: &serde_yaml::Mapping,
        visibility: SkillVisibility,
    ) -> bool {
        crate::skills::skill_frontmatter_satisfies_with_provider_key(
            meta,
            visibility,
            Some(CLAUDE_SKILL_FRONTMATTER_KEY),
        )
    }

    fn render_skill_frontmatter(
        &self,
        before: &str,
        visibility: SkillVisibility,
    ) -> Result<String> {
        crate::skills::render_skill_frontmatter_with_provider_key(
            before,
            visibility,
            Some(CLAUDE_SKILL_FRONTMATTER_KEY),
        )
    }

    fn app_bundle_path(&self) -> Option<&'static str> {
        Some("/Applications/Claude.app")
    }

    fn executable_names(&self) -> &'static [&'static str] {
        &["claude"]
    }

    fn config_dir(&self, ctx: &ProviderContext) -> Option<PathBuf> {
        ctx.home.as_ref().map(|home| home.join(".claude"))
    }

    fn config_home(&self, home: &Path) -> PathBuf {
        home.join(".claude")
    }

    fn projection_directories(&self) -> &'static [&'static str] {
        &[".claude"]
    }

    fn projection_candidate_files(&self, domain: &str, ancestor: &Path) -> Vec<PathBuf> {
        match domain {
            "rules" => vec![ancestor.join("CLAUDE.md")],
            "hooks" => vec![
                ancestor.join(".claude/settings.json"),
                ancestor.join(".claude/settings.local.json"),
            ],
            "mcp" => vec![ancestor.join(".mcp.json"), ancestor.join(".claude.json")],
            "skills" => vec![ancestor.join(".claude/skills")],
            _ => Vec::new(),
        }
    }

    fn session_scan_priority(&self, root: &Path) -> Option<u8> {
        root.to_string_lossy().contains("/.claude/").then_some(2)
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
            sessions::scan_jsonl_sessions(
                &home.join(".claude/projects"),
                self.kind(),
                3,
                sessions_out,
                cache,
            );
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

    fn accepts_session_app_url(&self, url: &str) -> bool {
        url.strip_prefix("claude://resume?")
            .is_some_and(valid_resume_query)
    }

    fn update_session_metadata(
        &self,
        value: &Value,
        meta: &mut crate::sessions::SessionMetadata,
        deduplicated_usage: &mut BTreeMap<String, crate::sessions::SessionTokenUsage>,
    ) {
        if let Some(model) = extract_model(value) {
            meta.model = Some(model);
        }
        if let Some((message_id, token_usage)) = extract_token_usage(value) {
            deduplicated_usage.insert(message_id, token_usage);
        }
    }

    fn parse_transcript_value(&self, value: &Value, items: &mut Vec<TranscriptItem>) {
        parse_transcript(value, items);
    }

    fn config_profile_key(&self) -> Option<&'static str> {
        Some("claude")
    }

    fn skill_target_aliases(&self) -> &'static [(&'static str, &'static str)] {
        &[("claude", "claude-code")]
    }

    fn apply_config_profile(&self, command: &mut SessionCommand, profile: &str) -> Result<()> {
        apply_config_profile(command, profile)
    }

    fn transcript_search_hint(&self, line: &str) -> bool {
        may_contain_search_message(line)
    }

    fn recognizes_transcript(&self, value: &Value) -> bool {
        (value.get("type").and_then(Value::as_str) == Some("assistant")
            && value.get("message").is_some())
            || (value.get("type").and_then(Value::as_str) == Some("user")
                && value.get("sessionId").is_some()
                && value.get("message").is_some())
    }

    fn analytics_capabilities(&self) -> AnalyticsCapabilities {
        AnalyticsCapabilities {
            token_usage: true,
            reasoning_tokens: false,
            explicit_runs: false,
            rate_limit_history: false,
        }
    }

    fn parse_analytics_line(&self, line: &str, record: &mut SessionAnalyticsRecord) {
        crate::analytics::parse_message_line(line, record);
    }

    fn extract_skill_tool_payloads<'a>(&self, value: &'a Value) -> Vec<(&'a Value, Evidence)> {
        tool_payloads(value)
    }

    fn infer_session_project(&self, path: &Path, project: Option<PathBuf>) -> Option<PathBuf> {
        project.or_else(|| path.parent().map(Path::to_path_buf))
    }

    fn scan_mcp(
        &self,
        ctx: &ProviderContext,
        servers: &mut Vec<McpServerRecord>,
        warnings: &mut Vec<String>,
    ) -> Result<()> {
        if let Some(home) = &ctx.home {
            crate::mcp::scan_json_mcp(
                &home.join(".claude.json"),
                self.kind(),
                "global",
                &["mcpServers"],
                infer_mcp_transport,
                infer_mcp_enabled,
                infer_mcp_status,
                servers,
                warnings,
            );
            scan_claude_project_mcp(&home.join(".claude.json"), servers, warnings);
            crate::mcp::scan_json_mcp(
                &home.join(".claude/settings.json"),
                self.kind(),
                "global",
                &["mcpServers"],
                infer_mcp_transport,
                infer_mcp_enabled,
                infer_mcp_status,
                servers,
                warnings,
            );
        }
        for ancestor in ctx.project_dirs() {
            let scope = ancestor.display().to_string();
            crate::mcp::scan_json_mcp(
                &ancestor.join(".mcp.json"),
                self.kind(),
                &scope,
                &["mcpServers"],
                infer_mcp_transport,
                infer_mcp_enabled,
                infer_mcp_status,
                servers,
                warnings,
            );
        }
        Ok(())
    }

    fn set_mcp_enabled(&self, request: &McpSetEnabledRequest) -> Result<()> {
        if request.path.extension().and_then(|value| value.to_str()) != Some("json") {
            bail!("Claude Code MCP source must be JSON");
        }
        crate::mcp::set_json_server_enabled(request, &["mcpServers"], update_mcp_server)
    }

    fn backup_mcp_entry(
        &self,
        path: &Path,
        server_path: &[String],
        name: &str,
    ) -> Result<Value> {
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            bail!("Claude Code MCP source must be JSON");
        }
        crate::mcp::read_json_server_entry_at_path(path, server_path, name)
    }

    fn restore_mcp_entry(
        &self,
        path: &Path,
        server_path: &[String],
        name: &str,
        entry: &Value,
    ) -> Result<String> {
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            bail!("Claude Code MCP source must be JSON");
        }
        crate::mcp::merge_json_server_entry_at_path(path, server_path, name, entry)
    }

    fn mcp_status_after_toggle(&self, enabled: bool) -> &'static str {
        if enabled { "configured" } else { "disabled" }
    }

    fn delete_hooks(
        &self,
        requests: &[HookDeleteRequest],
        source: &str,
    ) -> Result<String> {
        if requests[0].path.extension().and_then(|value| value.to_str()) != Some("json") {
            bail!("Claude Code hook source must be JSON");
        }
        crate::hooks::delete_json_hooks(requests, source)
    }

    fn set_hook_enabled(
        &self,
        request: &HookSetEnabledRequest,
        source: &str,
    ) -> Result<String> {
        if request.path.extension().and_then(|value| value.to_str()) != Some("json") {
            bail!("Claude Code hook source must be JSON");
        }
        crate::hooks::set_json_hook_enabled(request, source)
    }

    fn backup_hook_entry(&self, path: &Path, identity: &HookSourceMatch) -> Result<Value> {
        crate::hooks::read_hook_entry(path, identity)
    }

    fn restore_hook_entry(
        &self,
        path: &Path,
        identity: &HookSourceMatch,
        entry: &Value,
    ) -> Result<String> {
        crate::hooks::merge_hook_entry(path, identity, entry)
    }

    fn managed_hook_path(&self, path: &Path) -> bool {
        let path = path.to_string_lossy();
        path.starts_with("/etc/claude-code/")
            || path.starts_with("/Library/Application Support/ClaudeCode/")
            || path.contains("/.claude/plugins/")
    }

    fn is_global_hook_path(&self, path: &Path) -> bool {
        dirs::home_dir().is_some_and(|home| path.starts_with(home.join(".claude")))
    }

    fn disables_hooks_from_config(&self, value: &Value) -> bool {
        value
            .get("disableAllHooks")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    fn scan_hooks(
        &self,
        ctx: &ProviderContext,
        scanned_files: &mut HashSet<PathBuf>,
        hooks: &mut Vec<HookRecord>,
        warnings: &mut Vec<String>,
    ) {
        if let Some(home) = &ctx.home {
            crate::hooks::scan_hook_file_once(
                &home.join(".claude/settings.json"),
                self.kind(),
                scanned_files,
                hooks,
                warnings,
            );
            scan_claude_plugin_hooks(&home.join(".claude/plugins"), hooks, warnings);
            scan_claude_component_hooks(&home.join(".claude/skills"), hooks, warnings);
            scan_claude_component_hooks(&home.join(".claude/agents"), hooks, warnings);
        }
        for path in [
            PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json"),
            PathBuf::from("/etc/claude-code/managed-settings.json"),
        ] {
            crate::hooks::scan_hook_file_once(&path, self.kind(), scanned_files, hooks, warnings);
        }
        scan_claude_managed_dropins(
            &PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.d"),
            hooks,
            warnings,
        );
        scan_claude_managed_dropins(
            &PathBuf::from("/etc/claude-code/managed-settings.d"),
            hooks,
            warnings,
        );
        for ancestor in ctx.project_dirs() {
            for relative in [".claude/settings.json", ".claude/settings.local.json"] {
                crate::hooks::scan_hook_file_once(
                    &ancestor.join(relative),
                    self.kind(),
                    scanned_files,
                    hooks,
                    warnings,
                );
            }
            scan_claude_component_hooks(&ancestor.join(".claude/skills"), hooks, warnings);
            scan_claude_component_hooks(&ancestor.join(".claude/agents"), hooks, warnings);
        }
    }

    fn review_hook(&self, hook: &HookRecord) -> Result<()> {
        crate::hooks::review_hook_with_tendi_state(hook)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{AgentProvider, ClaudeProvider, ProviderContext};
    use crate::{
        analytics::AnalyticsCapabilities,
        skills::SkillVisibility,
    };

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "tendi-claude-mcp-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ))
    }

    #[test]
    fn scans_personal_claude_json_mcp_file() {
        let root = temp_dir();
        let home = root.join("home");
        let path = home.join(".claude.json");
        fs::create_dir_all(&home).expect("create home");
        fs::write(
            &path,
            r#"{"mcpServers":{"personal-server":{"command":"demo"}}}"#,
        )
        .expect("write personal MCP");

        let context = ProviderContext {
            home: Some(home),
            project_dirs: Vec::new(),
        };
        let mut servers = Vec::new();
        let mut warnings = Vec::new();
        ClaudeProvider
            .scan_mcp(&context, &mut servers, &mut warnings)
            .expect("scan Claude MCP");

        assert!(warnings.is_empty(), "warnings: {warnings:#?}");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "personal-server");
        assert_eq!(servers[0].scope, "global");
        assert_eq!(servers[0].path, path);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scans_project_mcp_servers_nested_in_personal_claude_json() {
        let root = temp_dir();
        let home = root.join("home");
        let path = home.join(".claude.json");
        fs::create_dir_all(&home).expect("create home");
        fs::write(
            &path,
            r#"{
  "mcpServers": {"personal": {"command": "demo"}},
  "projects": {
    "/work/demo": {"mcpServers": {"project-server": {"command": "project"}}}
  }
}"#,
        )
        .expect("write Claude state");

        let context = ProviderContext {
            home: Some(home),
            project_dirs: Vec::new(),
        };
        let mut servers = Vec::new();
        let mut warnings = Vec::new();
        ClaudeProvider
            .scan_mcp(&context, &mut servers, &mut warnings)
            .expect("scan Claude MCP");

        assert!(warnings.is_empty(), "warnings: {warnings:#?}");
        assert_eq!(servers.len(), 2);
        let project_server = servers
            .iter()
            .find(|server| server.name == "project-server")
            .expect("nested project server");
        assert_eq!(project_server.scope, "/work/demo");
        assert_eq!(
            project_server.server_path,
            vec!["projects", "/work/demo", "mcpServers"]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn maps_claude_skill_invocation_frontmatter_to_visibility() {
        let frontmatter: serde_yaml::Value = serde_yaml::from_str(
            "name: demo\ndisable-model-invocation: true\n",
        )
        .expect("parse skill frontmatter");
        let metadata = ClaudeProvider
            .skill_visibility_metadata(
                PathBuf::from(".claude/skills/demo").as_path(),
                PathBuf::from(".claude/skills/demo/SKILL.md").as_path(),
                Some(&frontmatter),
            )
            .expect("read Claude skill metadata");

        assert_eq!(metadata.disable_model_invocation, Some(true));
        assert_eq!(metadata.provider_visibility, SkillVisibility::Manual);

        let meta = frontmatter.as_mapping().expect("mapping frontmatter");
        assert!(!ClaudeProvider.skill_frontmatter_satisfies(meta, SkillVisibility::Auto));
        assert!(ClaudeProvider.skill_frontmatter_satisfies(meta, SkillVisibility::Manual));

        let rendered = ClaudeProvider
            .render_skill_frontmatter(
                "---\nname: demo\n---\n\n# Demo\n",
                SkillVisibility::Manual,
            )
            .expect("render Claude skill frontmatter");
        assert!(rendered.contains("disable-model-invocation: true"));
        assert!(rendered.contains("visibility: manual"));
    }

    #[test]
    fn exposes_claude_token_usage_analytics_capability() {
        assert_eq!(
            ClaudeProvider.analytics_capabilities(),
            AnalyticsCapabilities {
                token_usage: true,
                reasoning_tokens: false,
                explicit_runs: false,
                rate_limit_history: false,
            }
        );
    }

}
