use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use anyhow::bail;
use chrono::{DateTime, SecondsFormat};
use serde_json::Value;
use walkdir::WalkDir;

use crate::transcript::{
    TranscriptItem, collect_cursor_item_with_timestamp, extract_raw_content_text,
};

use super::*;

pub(super) struct CursorProvider;

const CURSOR_SKILL_FRONTMATTER_KEY: &str = "disable-model-invocation";

#[cfg(test)]
pub(crate) fn plan_skill_frontmatter(
    path: PathBuf,
    visibility: SkillVisibility,
) -> Result<FileChange> {
    crate::skills::plan_skill_frontmatter_for_agent(path, AgentKind::Cursor, visibility)
}

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
    normalized == "cursor"
}

fn profile_paths_for_root(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() || !path.join("cli-config.json").is_file() {
                return None;
            }
            let name = entry.file_name().to_str()?.to_string();
            crate::config::validate_profile_name(&name)
                .ok()
                .map(|_| name)
        })
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|profile| root.join(profile).join("cli-config.json"))
        .collect()
}

pub(crate) fn scan_project_mcp(
    root: &Path,
    servers: &mut Vec<McpServerRecord>,
    warnings: &mut Vec<String>,
) {
    crate::mcp::scan_project_mcp(
        root,
        AgentKind::Cursor,
        &["mcp.json", ".mcp.json"],
        &["mcp-auth.json"],
        &["mcpServers"],
        infer_mcp_transport,
        infer_mcp_enabled,
        infer_mcp_status,
        servers,
        warnings,
    );
    if !root.is_dir() {
        return;
    }
    for entry in WalkDir::new(root)
        .follow_links(true)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "SERVER_METADATA.json")
    {
        let path = entry.path();
        let Some(scope) = entry_scope(root, path) else {
            continue;
        };
        scan_cursor_metadata_mcp(path, &scope, servers, warnings);
    }
}

fn entry_scope(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .and_then(|path| path.components().next())
        .and_then(|component| component.as_os_str().to_str())
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(str::to_string)
}

fn scan_cursor_metadata_mcp(
    path: &Path,
    scope: &str,
    servers: &mut Vec<McpServerRecord>,
    warnings: &mut Vec<String>,
) {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return;
        }
    };
    let value = match serde_json::from_str::<Value>(&text) {
        Ok(value) => value,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return;
        }
    };
    if value
        .get("serverIdentifier")
        .and_then(Value::as_str)
        .is_none()
    {
        return;
    }
    let Some(name) = value
        .get("serverName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
    else {
        return;
    };
    let status_path = path.parent().map(|parent| parent.join("STATUS.md"));
    let status = status_path
        .as_ref()
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|text| {
            if text.to_ascii_lowercase().contains("needs authentication") {
                "needs-auth"
            } else {
                "configured"
            }
        })
        .unwrap_or_default();
    servers.push(McpServerRecord {
        agent: AgentKind::Cursor,
        name: name.to_string(),
        scope: scope.to_string(),
        transport: "cursor-plugin".to_string(),
        enabled: true,
        status: status.to_string(),
        path: path.to_path_buf(),
        trust_hash: crate::fsutil::sha256_text(&text),
        server_path: Vec::new(),
        read_only_reason: Some("Cursor plugin metadata cannot be changed".to_string()),
    });
}

pub(super) fn apply_config_profile(command: &mut SessionCommand, profile: &str) -> Result<()> {
    let path = crate::config::config_profile_path(crate::skills::AgentKind::Cursor, profile)?;
    if !path.is_file() {
        bail!("Cursor profile not found: {profile}");
    }
    let config_dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cursor profile directory is unavailable"))?;
    command.env.push((
        "CURSOR_CONFIG_DIR".to_string(),
        config_dir.display().to_string(),
    ));
    Ok(())
}

pub(super) fn parse_transcript(value: &Value, items: &mut Vec<TranscriptItem>) {
    collect_cursor_item_with_timestamp(value, items, cursor_event_timestamp(value));
}

pub(crate) fn cursor_event_timestamp(value: &Value) -> Option<String> {
    let content = value
        .pointer("/message/content")
        .or_else(|| value.get("content"))
        .or_else(|| value.get("message"));
    let text = extract_raw_content_text(content)?;
    let start = text.find("<timestamp>")? + "<timestamp>".len();
    let end = text[start..].find("</timestamp>")? + start;
    let value = text[start..end].trim();
    let normalized = normalize_cursor_timestamp(value)?;
    DateTime::parse_from_str(&normalized, "%A, %b %-d, %Y, %-I:%M %p %:z")
        .ok()
        .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn normalize_cursor_timestamp(value: &str) -> Option<String> {
    let (date_time, zone) = value.rsplit_once(" (")?;
    let zone = zone.strip_suffix(')')?.strip_prefix("UTC")?;
    let offset = normalize_cursor_offset(zone)?;
    Some(format!("{date_time} {offset}"))
}

fn normalize_cursor_offset(value: &str) -> Option<String> {
    if value.is_empty() {
        return Some("+00:00".to_string());
    }
    let sign = value
        .chars()
        .next()
        .filter(|sign| matches!(sign, '+' | '-'))?;
    let digits = value[sign.len_utf8()..].replace(':', "");
    if !digits.chars().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    let (hours, minutes) = match digits.len() {
        1 | 2 => (digits.parse::<u32>().ok()?, 0),
        4 => (
            digits.get(..2)?.parse::<u32>().ok()?,
            digits.get(2..)?.parse::<u32>().ok()?,
        ),
        _ => return None,
    };
    (hours <= 23 && minutes <= 59).then(|| format!("{sign}{hours:02}:{minutes:02}"))
}

pub(super) fn append_transcript_metadata(
    path: &Path,
    items: &mut Vec<TranscriptItem>,
) -> Result<()> {
    let Some(store_path) = find_cursor_store_db(path) else {
        return Ok(());
    };
    append_transcript_metadata_from_store(&store_path, items)?;
    Ok(())
}

pub(super) fn append_transcript_metadata_from_store(
    path: &Path,
    items: &mut Vec<TranscriptItem>,
) -> Result<()> {
    let models = cursor_sessions::cursor_store_models_for_path(path);
    insert_cursor_model_configs(items, &models);
    Ok(())
}

pub(super) fn enrich_transcript_tools_from_store(
    path: &Path,
    items: &mut Vec<TranscriptItem>,
) -> Result<()> {
    let tool_calls = cursor_sessions::cursor_store_tool_calls_for_path(path);
    if tool_calls.is_empty() {
        return Ok(());
    }
    let mut used = HashSet::new();
    for item in items.iter_mut().filter(|item| item.kind == "tool") {
        let Some(command) = item.command.as_deref() else {
            continue;
        };
        let Some((index, tool_call)) = tool_calls.iter().enumerate().find(|(index, tool_call)| {
            !used.contains(index)
                && (item.call_id.as_deref() == Some(tool_call.id.as_str())
                    || (item.tag.as_deref() == Some(tool_call.name.as_str())
                        && cursor_tool_call_matches(command, &tool_call.args)))
        }) else {
            continue;
        };
        used.insert(index);
        item.call_id = Some(tool_call.id.clone());
        if let Some(result) = &tool_call.result {
            item.result = Some(result.clone());
        }
    }
    Ok(())
}

fn cursor_tool_call_matches(command: &str, args: &Value) -> bool {
    if serde_json::from_str::<Value>(command).ok().as_ref() == Some(args) {
        return true;
    }
    args.get("command")
        .or_else(|| args.get("cmd"))
        .and_then(Value::as_str)
        == Some(command)
}

#[cfg(test)]
pub(crate) fn append_transcript_metadata_from_store_for_test(
    path: &Path,
    items: &mut Vec<TranscriptItem>,
) {
    let _ = append_transcript_metadata_from_store(path, items);
}

pub(super) fn may_contain_search_message(line: &str) -> bool {
    let hint = crate::transcript::search_json_hint(line);
    matches!(
        crate::transcript::json_string_hint(hint, "\"role\"")
            .or_else(|| crate::transcript::json_string_hint(hint, "\"type\"")),
        Some("user" | "assistant")
    )
}

pub(crate) fn find_cursor_store_db(path: &Path) -> Option<std::path::PathBuf> {
    let session_id = path.file_stem()?.to_str()?.trim();
    if session_id.is_empty() {
        return None;
    }
    let chats_root = dirs::home_dir()?.join(".cursor/chats");
    let entries = std::fs::read_dir(chats_root).ok()?;
    entries.filter_map(Result::ok).find_map(|entry| {
        let candidate = entry.path().join(session_id).join("store.db");
        candidate.is_file().then_some(candidate)
    })
}

fn insert_cursor_model_configs(items: &mut Vec<TranscriptItem>, models: &[String]) {
    if models.is_empty() {
        return;
    }
    let assistant_positions = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| (item.kind == "assistant").then_some(index))
        .collect::<Vec<_>>();
    let mut previous_model: Option<&str> = None;
    let model_count = models.len();
    let assistant_count = assistant_positions.len();
    let mut insertions = Vec::new();
    for (sample_index, model) in models.iter().enumerate() {
        if previous_model == Some(model.as_str()) {
            continue;
        }
        previous_model = Some(model);
        let assistant_index = sample_index
            .saturating_mul(assistant_count)
            .checked_div(model_count)
            .unwrap_or(0);
        let item_index = assistant_positions
            .get(assistant_index)
            .copied()
            .unwrap_or(items.len());
        insertions.push((item_index, model.clone()));
    }
    let mut offset = 0usize;
    for (item_index, model) in insertions {
        items.insert(
            item_index + offset,
            model_config_item(Some(model), None, None),
        );
        offset += 1;
    }
}

fn model_config_item(
    model: Option<String>,
    effort: Option<String>,
    time: Option<String>,
) -> TranscriptItem {
    let body = [
        model.as_ref().map(|value| format!("Model: {value}")),
        effort.as_ref().map(|value| format!("Effort: {value}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    TranscriptItem {
        kind: "model_config".to_string(),
        body,
        tag: None,
        time,
        command: None,
        result: None,
        duration_ms: None,
        linked_session_id: None,
        model,
        effort,
        call_id: None,
        started_at_ms: None,
    }
}
impl super::AgentProvider for CursorProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Cursor
    }

    fn storage_key(&self) -> &'static str {
        "cursor"
    }

    fn discoverable(&self) -> bool {
        true
    }

    fn global_skill_root(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".cursor/skills"))
    }

    fn project_skill_root(&self, cwd: &Path) -> Option<PathBuf> {
        Some(cwd.join(".cursor/skills"))
    }

    fn skill_target(&self) -> Option<ProviderSkillTarget> {
        Some(ProviderSkillTarget {
            id: "cursor",
            display_name: "Cursor",
        })
    }

    fn config_profile_path(&self, home: &Path, _agent_home: &Path, name: &str) -> Option<PathBuf> {
        Some(
            home.join(".cursor/tendi-profiles")
                .join(name)
                .join("cli-config.json"),
        )
    }

    fn config_profile_format(&self) -> Option<&'static str> {
        Some("json")
    }

    fn config_files(&self, home: &Path, agent_home: &Path) -> Vec<crate::config::AgentConfigFile> {
        let base_path = home.join(".cursor/cli-config.json");
        let mut configs = vec![
            self.config_file_for_path(home, agent_home, &base_path)
                .expect("Cursor provider must resolve its base config path"),
        ];
        configs.extend(
            profile_paths_for_root(&home.join(".cursor/tendi-profiles"))
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
        let base_path = home.join(".cursor/cli-config.json");
        if path == base_path {
            return Some(crate::config::AgentConfigFile {
                agent: self.kind(),
                label: "Cursor".to_string(),
                path: base_path.clone(),
                format: "json".to_string(),
                exists: base_path.is_file(),
                updated_at: None,
                profile: None,
            });
        }

        if path.file_name()?.to_str()? != "cli-config.json" {
            return None;
        }
        let profile = path.parent()?.file_name()?.to_str()?;
        crate::config::validate_profile_name(profile).ok()?;
        let expected_path = home
            .join(".cursor/tendi-profiles")
            .join(profile)
            .join("cli-config.json");
        (path == expected_path).then_some(crate::config::AgentConfigFile {
            agent: self.kind(),
            label: format!("Cursor / {profile}"),
            path: expected_path.clone(),
            format: "json".to_string(),
            exists: expected_path.is_file(),
            updated_at: None,
            profile: Some(profile.to_string()),
        })
    }

    fn config_order(&self) -> usize {
        2
    }

    fn matches_name(&self, normalized: &str) -> bool {
        matches_name(normalized)
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("Cursor")
    }

    fn app_bundle_path(&self) -> Option<&'static str> {
        Some("/Applications/Cursor.app")
    }

    fn executable_names(&self) -> &'static [&'static str] {
        &["cursor"]
    }

    fn config_dir(&self, ctx: &ProviderContext) -> Option<PathBuf> {
        ctx.home.as_ref().map(|home| home.join(".cursor"))
    }

    fn config_home(&self, home: &Path) -> PathBuf {
        home.join(".cursor")
    }

    fn projection_directories(&self) -> &'static [&'static str] {
        &[".cursor"]
    }

    fn projection_candidate_files(&self, domain: &str, ancestor: &Path) -> Vec<PathBuf> {
        match domain {
            "rules" => vec![ancestor.join(".cursorrules")],
            "hooks" => vec![ancestor.join(".cursor/hooks.json")],
            "mcp" => vec![
                ancestor.join(".cursor/mcp.json"),
                ancestor.join(".cursor/cli-config.json"),
            ],
            "skills" => vec![ancestor.join(".cursor/skills")],
            _ => Vec::new(),
        }
    }

    fn projection_candidate_is_file(&self, path: &Path) -> bool {
        path.file_name().and_then(|name| name.to_str()) == Some(".cursorrules")
    }

    fn skill_visibility_metadata(
        &self,
        _skill_dir: &Path,
        _skill_file: &Path,
        frontmatter: Option<&serde_yaml::Value>,
    ) -> Result<SkillProviderMetadata> {
        Ok(SkillProviderMetadata {
            allow_implicit_invocation: None,
            enabled: None,
            disable_model_invocation: frontmatter
                .and_then(|value| value.get(CURSOR_SKILL_FRONTMATTER_KEY))
                .and_then(serde_yaml::Value::as_bool),
            provider_visibility: if frontmatter
                .and_then(|value| value.get(CURSOR_SKILL_FRONTMATTER_KEY))
                .and_then(serde_yaml::Value::as_bool)
                == Some(true)
            {
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
            Some(CURSOR_SKILL_FRONTMATTER_KEY),
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
            Some(CURSOR_SKILL_FRONTMATTER_KEY),
        )
    }

    fn skill_frontmatter_visibility_key(&self) -> Option<&'static str> {
        Some(CURSOR_SKILL_FRONTMATTER_KEY)
    }

    fn session_scan_priority(&self, root: &Path) -> Option<u8> {
        root.to_string_lossy().contains("/.cursor/").then_some(1)
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
            cursor_sessions::scan_cursor_meta(
                &home.join(".cursor/acp-sessions"),
                sessions_out,
                self.kind(),
                cache,
            );
            cursor_sessions::scan_cursor_meta(
                &home.join(".cursor/chats"),
                sessions_out,
                self.kind(),
                cache,
            );
            sessions::scan_jsonl_sessions(
                &home.join(".cursor/projects"),
                self.kind(),
                4,
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
        if let Some(home) = &ctx.home {
            rules::add_rule_tree(
                rules_out,
                warnings,
                order,
                self.kind(),
                "cursor-rule",
                "global",
                &home.join(".cursor/rules"),
                Some("mdc"),
                6,
            );
        }
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
                ".cursorrules",
                "project",
                dir.join(".cursorrules"),
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

    fn session_requires_rescan(&self, session: &SessionRecord) -> Option<bool> {
        if session.started_at.is_some() && session.updated_at.is_some() {
            return None;
        }
        if session
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("jsonl")
        {
            return None;
        }
        fs::read_to_string(&session.path)
            .ok()
            .is_some_and(|text| text.contains("<timestamp>") && text.contains("</timestamp>"))
            .then_some(true)
    }

    fn update_session_metadata(
        &self,
        value: &Value,
        meta: &mut SessionMetadata,
        _deduplicated_usage: &mut BTreeMap<String, SessionTokenUsage>,
    ) {
        if let Some(timestamp) = cursor_event_timestamp(value) {
            sessions::apply_time_bounds(meta, &timestamp);
        }
    }

    fn parse_transcript_value(&self, value: &Value, items: &mut Vec<TranscriptItem>) {
        parse_transcript(value, items);
    }

    fn config_profile_key(&self) -> Option<&'static str> {
        Some("cursor")
    }

    fn apply_config_profile(&self, command: &mut SessionCommand, profile: &str) -> Result<()> {
        apply_config_profile(command, profile)
    }

    fn append_transcript_metadata(
        &self,
        path: &Path,
        items: &mut Vec<TranscriptItem>,
    ) -> Result<()> {
        append_transcript_metadata(path, items)
    }

    fn append_transcript_metadata_from_store(
        &self,
        path: &Path,
        items: &mut Vec<TranscriptItem>,
    ) -> Result<()> {
        append_transcript_metadata_from_store(path, items)
    }

    fn enrich_transcript_tools_from_store(
        &self,
        path: &Path,
        items: &mut Vec<TranscriptItem>,
    ) -> Result<()> {
        enrich_transcript_tools_from_store(path, items)
    }

    fn transcript_metadata_store_path(&self, path: &Path) -> Option<PathBuf> {
        find_cursor_store_db(path)
    }

    fn transcript_search_hint(&self, line: &str) -> bool {
        may_contain_search_message(line)
    }

    fn transcript_cacheable(&self) -> bool {
        false
    }

    fn recognizes_transcript(&self, value: &Value) -> bool {
        value.get("role").and_then(Value::as_str).is_some() && value.get("message").is_some()
    }

    fn session_supports_append_cache(&self) -> bool {
        true
    }

    fn analytics_model_hint(&self, session: &SessionRecord) -> Option<String> {
        let models = cursor_sessions::cursor_store_models_for_path(&session.path);
        if models.is_empty() {
            return session
                .model
                .as_deref()
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string);
        }
        let first = models.first()?.trim();
        (models.iter().all(|model| model.trim() == first)).then(|| first.to_string())
    }

    fn parse_analytics_line(&self, line: &str, record: &mut SessionAnalyticsRecord) {
        let timestamp = serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|value| cursor_event_timestamp(&value));
        crate::analytics::parse_message_line_with_timestamp(line, record, timestamp);
    }

    fn extract_skill_tool_payloads<'a>(&self, value: &'a Value) -> Vec<(&'a Value, Evidence)> {
        crate::providers::shared::shared_tool_payloads(value)
    }

    fn infer_session_project(&self, path: &Path, project: Option<PathBuf>) -> Option<PathBuf> {
        project.or_else(|| cursor_sessions::cursor_project_from_transcript_path(path))
    }

    fn infer_meta_project(&self, value: &Value) -> Option<PathBuf> {
        cursor_sessions::cursor_project_from_meta(Some(value))
    }

    fn is_session_candidate_path(&self, path: &Path) -> bool {
        matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("meta.json" | "store.db")
        )
    }

    fn session_path_role(&self, path: &Path) -> SessionPathRole {
        if path.file_name().and_then(|name| name.to_str()) == Some("meta.json") {
            SessionPathRole::Metadata
        } else if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            SessionPathRole::Transcript
        } else {
            SessionPathRole::Other
        }
    }

    fn session_scan_source_paths(&self, path: &Path) -> Vec<PathBuf> {
        cursor_sessions::session_scan_source_paths(path)
    }

    fn session_project_aliases(&self, path: &str) -> Vec<String> {
        let home = dirs::home_dir().map(|home| home.to_string_lossy().into_owned());
        let Some(home) = home else {
            return Vec::new();
        };
        let prefix = format!("{home}/codex/worktrees/");
        path.strip_prefix(&prefix)
            .map(|suffix| format!("{home}/.codex/worktrees/{suffix}"))
            .into_iter()
            .collect()
    }

    fn scan_explicit_session_path(
        &self,
        path: &Path,
        sessions: &mut Vec<SessionRecord>,
        cache: Option<&SessionScanCache>,
    ) -> bool {
        if cursor_sessions::is_cursor_meta_file(path) {
            cursor_sessions::scan_cursor_meta_file(path, sessions, self.kind(), cache);
            true
        } else if path.file_name().and_then(|name| name.to_str()) == Some("store.db") {
            cursor_sessions::scan_cursor_store_file(path, sessions, self.kind(), cache);
            true
        } else {
            false
        }
    }

    fn scan_mcp(
        &self,
        ctx: &ProviderContext,
        servers: &mut Vec<McpServerRecord>,
        warnings: &mut Vec<String>,
    ) -> Result<()> {
        if let Some(home) = &ctx.home {
            crate::mcp::scan_json_mcp(
                &home.join(".cursor/mcp.json"),
                self.kind(),
                "global",
                &["mcpServers"],
                infer_mcp_transport,
                infer_mcp_enabled,
                infer_mcp_status,
                servers,
                warnings,
            );
            crate::mcp::scan_json_mcp(
                &home.join(".cursor/cli-config.json"),
                self.kind(),
                "global",
                &["mcpServers"],
                infer_mcp_transport,
                infer_mcp_enabled,
                infer_mcp_status,
                servers,
                warnings,
            );
            scan_project_mcp(&home.join(".cursor/projects"), servers, warnings);
        }
        for ancestor in ctx.project_dirs() {
            let scope = ancestor.display().to_string();
            crate::mcp::scan_json_mcp(
                &ancestor.join(".cursor/mcp.json"),
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
        if request.path.file_name().and_then(|value| value.to_str()) == Some("SERVER_METADATA.json")
        {
            bail!("Cursor plugin MCP metadata is read-only");
        }
        if request.path.extension().and_then(|value| value.to_str()) != Some("json") {
            bail!("Cursor MCP source must be JSON");
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
            bail!("Cursor MCP source must be JSON");
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
            bail!("Cursor MCP source must be JSON");
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
            bail!("Cursor hook source must be JSON");
        }
        crate::hooks::delete_json_hooks(requests, source)
    }

    fn set_hook_enabled(
        &self,
        request: &HookSetEnabledRequest,
        source: &str,
    ) -> Result<String> {
        if request.path.extension().and_then(|value| value.to_str()) != Some("json") {
            bail!("Cursor hook source must be JSON");
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
        path.starts_with("/etc/cursor/") || path.starts_with("/Library/Application Support/Cursor/")
    }

    fn is_global_hook_path(&self, path: &Path) -> bool {
        dirs::home_dir().is_some_and(|home| path.starts_with(home.join(".cursor")))
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
                &home.join(".cursor/hooks.json"),
                self.kind(),
                scanned_files,
                hooks,
                warnings,
            );
        }
        for path in [
            PathBuf::from("/etc/cursor/hooks.json"),
            PathBuf::from("/Library/Application Support/Cursor/hooks.json"),
        ] {
            crate::hooks::scan_hook_file_once(&path, self.kind(), scanned_files, hooks, warnings);
        }
        for ancestor in ctx.project_dirs() {
            crate::hooks::scan_hook_file_once(
                &ancestor.join(".cursor/hooks.json"),
                self.kind(),
                scanned_files,
                hooks,
                warnings,
            );
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

    use super::{
        AgentProvider, CursorProvider, ProviderContext, cursor_event_timestamp, parse_transcript,
    };
    use crate::skills::AgentKind;
    use rusqlite::Connection;
    use serde_json::json;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "tendi-cursor-rules-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ))
    }

    #[test]
    fn parses_embedded_cursor_timestamp_for_transcript_items() {
        let user = json!({
            "role": "user",
            "message": {
                "content": [{
                    "type": "text",
                    "text": "<timestamp>Thursday, Aug 27, 2026, 11:01 PM (UTC+8)</timestamp>\n<user_query>Why did sending fail?</user_query>"
                }]
            }
        });
        assert_eq!(
            cursor_event_timestamp(&user).as_deref(),
            Some("2026-08-27T23:01:00+08:00")
        );

        let mut items = Vec::new();
        parse_transcript(&user, &mut items);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].body, "Why did sending fail?");
        assert_eq!(items[0].time.as_deref(), Some("23:01"));
    }

    #[test]
    fn scans_global_and_project_cursor_rules_with_their_own_scopes() {
        let root = temp_dir();
        let home = root.join("home");
        let project = root.join("project");
        let global_rules = home.join(".cursor/rules");
        let project_rules = project.join(".cursor/rules");
        fs::create_dir_all(global_rules.join("nested")).expect("create global rules");
        fs::create_dir_all(&project_rules).expect("create project rules");
        fs::write(global_rules.join("global.mdc"), "global rule").expect("write global rule");
        fs::write(global_rules.join("nested/deep.mdc"), "nested global rule")
            .expect("write nested global rule");
        fs::write(global_rules.join("ignored.md"), "not a Cursor rule")
            .expect("write ignored global rule");
        fs::write(project_rules.join("project.mdc"), "project rule")
            .expect("write project rule");
        fs::write(project.join(".cursorrules"), "legacy project rule")
            .expect("write legacy project rule");
        fs::write(project.join("CLAUDE.md"), "Claude-compatible project rule")
            .expect("write Claude-compatible project rule");

        let context = ProviderContext {
            home: Some(home),
            project_dirs: vec![project.clone()],
        };
        let mut rules = Vec::new();
        let mut warnings = Vec::new();
        let mut order = 0;
        CursorProvider.scan_rules(&context, &mut rules, &mut warnings, &mut order);

        assert!(warnings.is_empty(), "warnings: {warnings:#?}");
        assert_eq!(
            rules
                .iter()
                .map(|rule| (
                    rule.path.strip_prefix(&root).unwrap().to_path_buf(),
                    rule.scope.as_str(),
                    rule.agents.clone(),
                ))
                .collect::<Vec<_>>(),
            vec![
                (
                    PathBuf::from("home/.cursor/rules/global.mdc"),
                    "global",
                    vec![AgentKind::Cursor],
                ),
                (
                    PathBuf::from("home/.cursor/rules/nested/deep.mdc"),
                    "global",
                    vec![AgentKind::Cursor],
                ),
                (
                    PathBuf::from("project/CLAUDE.md"),
                    "project",
                    vec![AgentKind::Cursor],
                ),
                (
                    PathBuf::from("project/.cursorrules"),
                    "project",
                    vec![AgentKind::Cursor],
                ),
                (
                    PathBuf::from("project/.cursor/rules/project.mdc"),
                    "project",
                    vec![AgentKind::Cursor],
                ),
            ]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scans_cursor_global_mcp_file() {
        let root = temp_dir();
        let home = root.join("home");
        let path = home.join(".cursor/mcp.json");
        fs::create_dir_all(path.parent().expect("mcp parent")).expect("create mcp directory");
        fs::write(
            &path,
            r#"{"mcpServers":{"global-server":{"command":"demo"}}}"#,
        )
        .expect("write global MCP");

        let context = ProviderContext {
            home: Some(home),
            project_dirs: Vec::new(),
        };
        let mut servers = Vec::new();
        let mut warnings = Vec::new();
        CursorProvider
            .scan_mcp(&context, &mut servers, &mut warnings)
            .expect("scan Cursor MCP");

        assert!(warnings.is_empty(), "warnings: {warnings:#?}");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "global-server");
        assert_eq!(servers[0].scope, "global");
        assert_eq!(servers[0].path, path);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn enriches_cursor_tool_items_with_store_arguments_and_results() {
        let root = temp_dir();
        fs::create_dir_all(&root).expect("create Cursor store directory");
        let store_path = root.join("store.db");
        let connection = Connection::open(&store_path).expect("open Cursor store");
        connection
            .execute_batch(
                "CREATE TABLE blobs (data BLOB); \
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);",
            )
            .expect("create Cursor store tables");

        let read_args = json!({ "path": "/tmp/example.txt" });
        let replace_args = json!({
            "path": "/tmp/example.txt",
            "old_string": "before",
            "new_string": "after"
        });
        let blobs = [
            json!({
                "role": "assistant",
                "content": [{
                    "type": "tool-call",
                    "toolCallId": "read-call",
                    "toolName": "Read",
                    "args": read_args
                }]
            }),
            json!({
                "role": "tool",
                "content": [{
                    "type": "tool-result",
                    "toolCallId": "read-call",
                    "toolName": "Read",
                    "result": "file contents"
                }]
            }),
            json!({
                "role": "assistant",
                "content": [{
                    "type": "tool-call",
                    "toolCallId": "replace-call",
                    "toolName": "StrReplace",
                    "args": replace_args
                }]
            }),
            json!({
                "role": "tool",
                "content": [{
                    "type": "tool-result",
                    "toolCallId": "replace-call",
                    "toolName": "StrReplace",
                    "result": "file updated"
                }]
            }),
        ];
        for blob in blobs {
            let data = blob.to_string();
            connection
                .execute("INSERT INTO blobs (data) VALUES (?1)", [data.as_bytes()])
                .expect("insert Cursor store blob");
        }
        drop(connection);

        let transcript = json!({
            "role": "assistant",
            "message": {
                "content": [
                    {
                        "type": "tool_use",
                        "id": "read-call",
                        "name": "Read",
                        "input": { "path": "/tmp/example.txt" }
                    },
                    {
                        "type": "tool_use",
                        "id": "replace-call",
                        "name": "StrReplace",
                        "input": {
                            "path": "/tmp/example.txt",
                            "old_string": "before",
                            "new_string": "after"
                        }
                    }
                ]
            }
        });
        let mut items = Vec::new();
        parse_transcript(&transcript, &mut items);

        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.command.is_some()));
        let stored_tool_calls =
            crate::providers::cursor_sessions::cursor_store_tool_calls_for_path(&store_path);
        assert_eq!(
            stored_tool_calls
                .iter()
                .map(|call| (call.id.as_str(), call.result.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("read-call", Some("file contents")),
                ("replace-call", Some("file updated")),
            ]
        );
        super::enrich_transcript_tools_from_store(&store_path, &mut items)
            .expect("enrich Cursor tool items");

        assert_eq!(items[0].call_id.as_deref(), Some("read-call"));
        assert_eq!(items[0].result.as_deref(), Some("file contents"));
        assert_eq!(items[1].call_id.as_deref(), Some("replace-call"));
        assert_eq!(items[1].result.as_deref(), Some("file updated"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(items[0].command.as_deref().unwrap())
                .expect("Read arguments JSON"),
            json!({ "path": "/tmp/example.txt" })
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(items[1].command.as_deref().unwrap())
                .expect("StrReplace arguments JSON"),
            json!({
                "path": "/tmp/example.txt",
                "old_string": "before",
                "new_string": "after"
            })
        );

        let _ = fs::remove_dir_all(root);
    }

}
