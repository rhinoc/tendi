use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::Value;
use toml::Value as TomlValue;
use walkdir::WalkDir;

use crate::transcript::{
    TranscriptItem, attach_tool_result, collect_message_content, compact_time, extract_call_id,
    extract_duration_ms, extract_raw_content_text, extract_thinking_text, extract_tool_command,
    extract_tool_result, parse_timestamp_ms, push_item, push_tool_item, summarize_tool_call,
};

use super::*;

pub(super) struct CodexProvider;

pub(super) fn matches_name(normalized: &str) -> bool {
    normalized == "codex"
}

/// Codex lifecycle hook events from the official `HookEventsToml` schema.
/// `hooks.state` is runtime metadata and is intentionally excluded.
#[derive(Debug, Default, Deserialize)]
struct CodexHookEvents {
    #[serde(rename = "PreToolUse", default)]
    pre_tool_use: Vec<Value>,
    #[serde(rename = "PermissionRequest", default)]
    permission_request: Vec<Value>,
    #[serde(rename = "PostToolUse", default)]
    post_tool_use: Vec<Value>,
    #[serde(rename = "PreCompact", default)]
    pre_compact: Vec<Value>,
    #[serde(rename = "PostCompact", default)]
    post_compact: Vec<Value>,
    #[serde(rename = "SessionStart", default)]
    session_start: Vec<Value>,
    #[serde(rename = "UserPromptSubmit", default)]
    user_prompt_submit: Vec<Value>,
    #[serde(rename = "SubagentStart", default)]
    subagent_start: Vec<Value>,
    #[serde(rename = "SubagentStop", default)]
    subagent_stop: Vec<Value>,
    #[serde(rename = "Stop", default)]
    stop: Vec<Value>,
}

impl CodexHookEvents {
    fn event_groups(&self) -> [(&str, &Vec<Value>); 10] {
        [
            ("PreToolUse", &self.pre_tool_use),
            ("PermissionRequest", &self.permission_request),
            ("PostToolUse", &self.post_tool_use),
            ("PreCompact", &self.pre_compact),
            ("PostCompact", &self.post_compact),
            ("SessionStart", &self.session_start),
            ("UserPromptSubmit", &self.user_prompt_submit),
            ("SubagentStart", &self.subagent_start),
            ("SubagentStop", &self.subagent_stop),
            ("Stop", &self.stop),
        ]
    }

    fn is_empty(&self) -> bool {
        self.event_groups()
            .into_iter()
            .all(|(_, groups)| groups.is_empty())
    }
}

#[derive(Debug, Deserialize)]
struct CodexHooksFile {
    hooks: CodexHookEvents,
}

fn collect_codex_hook_events(
    path: &Path,
    trust_hash: &str,
    events: &CodexHookEvents,
    hooks: &mut Vec<HookRecord>,
) {
    for (event, groups) in events.event_groups() {
        if groups.is_empty() {
            continue;
        }
        for (group_index, group) in groups.iter().enumerate() {
            crate::hooks::collect_event_hooks(
                AgentKind::Codex,
                path,
                trust_hash,
                event,
                group_index,
                group,
                hooks,
            );
        }
    }
}

pub(crate) fn parse_codex_hook_file(
    path: &Path,
    trust_hash: &str,
    hooks: &mut Vec<HookRecord>,
    warnings: &mut Vec<String>,
) -> bool {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return true;
        }
    };
    let parsed = match serde_json::from_str::<CodexHooksFile>(&text) {
        Ok(parsed) => parsed,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return true;
        }
    };
    collect_codex_hook_events(path, trust_hash, &parsed.hooks, hooks);
    true
}

pub(crate) fn scan_codex_config_hooks(
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
    let trust_hash = match crate::fsutil::sha256_file(path) {
        Ok(hash) => hash,
        Err(err) => {
            warnings.push(format!("{}: {err:#}", path.display()));
            return;
        }
    };
    let toml_value = match toml::from_str::<TomlValue>(&text) {
        Ok(value) => value,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return;
        }
    };
    let Some(hooks_table) = toml_value.get("hooks") else {
        return;
    };
    let events: CodexHookEvents = match hooks_table.clone().try_into() {
        Ok(events) => events,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return;
        }
    };
    if !events.is_empty() {
        collect_codex_hook_events(path, &trust_hash, &events, hooks);
    }
}

fn codex_home(ctx: &ProviderContext) -> PathBuf {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| ctx.home.as_ref().map(|home| home.join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

pub(super) fn codex_skill_roots(
    home: &Path,
    project_dirs: &[PathBuf],
    agent: AgentKind,
) -> Vec<SkillRoot> {
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

pub(super) fn codex_plugin_enabled_by_id(codex_home: &Path) -> BTreeMap<String, bool> {
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

pub(super) fn codex_plugin_id_for_skill_root(
    codex_home: &Path,
    skill_root: &Path,
) -> Option<String> {
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

pub(super) fn codex_plugin_skill_roots(codex_home: &Path) -> Vec<PathBuf> {
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

pub(super) fn apply_config_profile(command: &mut SessionCommand, profile: &str) -> Result<()> {
    if !crate::config::config_profile_exists(crate::skills::AgentKind::Codex, profile)? {
        bail!("Codex profile not found: {profile}");
    }
    command
        .args
        .splice(0..0, ["--profile".to_string(), profile.to_string()]);
    Ok(())
}

pub(super) fn parse_transcript(value: &Value, items: &mut Vec<TranscriptItem>) {
    collect_codex_item(value, items);
}

pub(super) fn tool_payloads(value: &Value) -> Vec<(&Value, Evidence)> {
    if value.get("type").and_then(Value::as_str) != Some("response_item") {
        return Vec::new();
    }
    let Some(payload) = value.get("payload") else {
        return Vec::new();
    };
    if !matches!(
        payload.get("type").and_then(Value::as_str),
        Some("function_call" | "custom_tool_call" | "local_shell_call")
    ) {
        return Vec::new();
    }
    let name = payload
        .get("name")
        .or_else(|| payload.pointer("/action/type"))
        .and_then(Value::as_str)
        .unwrap_or("tool_call");
    vec![(
        payload,
        Evidence {
            kind: name.to_string(),
            text: crate::session_skills::summarize_evidence(payload, name),
            time: value
                .get("timestamp")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
    )]
}

pub(super) fn may_contain_search_message(line: &str) -> bool {
    let hint = crate::transcript::search_json_hint(line);
    crate::transcript::json_string_hint(hint, "\"type\"") == Some("response_item")
        && matches!(
            crate::transcript::json_string_hint(hint, "\"role\""),
            Some("user" | "assistant")
        )
        && hint.contains("\"message\"")
}

fn collect_codex_item(value: &Value, items: &mut Vec<TranscriptItem>) {
    let record_type = value.get("type").and_then(Value::as_str);
    if record_type == Some("compacted") {
        push_codex_compaction(items, value);
        return;
    }
    if record_type == Some("event_msg") {
        if value.pointer("/payload/type").and_then(Value::as_str) == Some("context_compacted") {
            push_codex_compaction(items, value);
            return;
        }
        if value.pointer("/payload/type").and_then(Value::as_str) == Some("thread_settings_applied")
        {
            push_codex_model_config(items, value);
            return;
        }
        attach_codex_subagent_session(value, items);
        return;
    }
    if record_type == Some("turn_context") {
        push_codex_model_config(items, value);
        return;
    }
    if record_type != Some("response_item") {
        return;
    }

    let Some(payload) = value.get("payload") else {
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

    match payload.get("type").and_then(Value::as_str) {
        Some("message") => {
            let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
            let content = payload.get("content");
            if role == "developer" || role == "system" {
                if let Some(body) = extract_raw_content_text(content) {
                    push_item(
                        items,
                        "context",
                        body,
                        Some(
                            if role == "system" {
                                "System"
                            } else {
                                "Developer"
                            }
                            .to_string(),
                        ),
                        time,
                    );
                }
                return;
            }
            if role != "user" && role != "assistant" {
                return;
            }
            collect_message_content(content, items, role, time);
        }
        Some("reasoning") | Some("thinking") => {
            let kind = payload
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("reasoning");
            if let Some(body) = extract_thinking_text(
                payload
                    .get("summary")
                    .or_else(|| payload.get("content"))
                    .or(Some(payload)),
            ) {
                push_item(items, kind, body, None, time);
            }
        }
        Some("function_call") | Some("custom_tool_call") | Some("local_shell_call") => {
            let name = payload
                .get("name")
                .or_else(|| payload.pointer("/action/type"))
                .and_then(Value::as_str)
                .unwrap_or("tool_call");
            push_tool_item(
                items,
                "tool",
                summarize_tool_call(payload, name),
                Some(name.to_string()),
                time,
                extract_tool_command(payload),
                None,
                extract_duration_ms(payload, None),
                extract_call_id(payload),
                timestamp_ms,
            );
        }
        Some("function_call_output") | Some("custom_tool_call_output") => {
            let result = extract_tool_result(payload);
            let call_id = extract_call_id(payload);
            if !attach_tool_result(
                items,
                call_id.as_deref(),
                result.clone(),
                extract_duration_ms(payload, result.as_deref()),
                timestamp_ms,
            ) {
                push_tool_item(
                    items,
                    "tool_result",
                    "Tool result".to_string(),
                    Some("tool_result".to_string()),
                    time,
                    None,
                    result,
                    extract_duration_ms(payload, None),
                    call_id,
                    timestamp_ms,
                );
            }
        }
        Some("web_search_call") | Some("image_generation_call") => {
            let kind = payload
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("tool_call");
            push_item(
                items,
                "tool",
                kind.replace('_', " "),
                Some(kind.to_string()),
                time,
            );
        }
        _ => {}
    }
}

fn push_codex_compaction(items: &mut Vec<TranscriptItem>, value: &Value) {
    let time = value
        .get("timestamp")
        .and_then(Value::as_str)
        .map(compact_time);
    if items
        .last()
        .is_some_and(|item| item.kind == "compaction" && item.time == time)
    {
        return;
    }
    push_item(
        items,
        "compaction",
        "Context compacted".to_string(),
        None,
        time,
    );
}

fn push_codex_model_config(items: &mut Vec<TranscriptItem>, value: &Value) {
    let settings = if value.get("type").and_then(Value::as_str) == Some("turn_context") {
        value.get("payload")
    } else {
        value.pointer("/payload/thread_settings")
    };
    let previous = items.iter().rev().find(|item| item.kind == "model_config");
    let model = settings
        .and_then(|settings| settings.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| previous.and_then(|item| item.model.clone()));
    let effort = settings
        .and_then(|settings| {
            settings
                .get("effort")
                .or_else(|| settings.get("reasoning_effort"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| previous.and_then(|item| item.effort.clone()));
    let time = value
        .get("timestamp")
        .and_then(Value::as_str)
        .map(compact_time);
    push_model_config(items, model, effort, time);
}

fn push_model_config(
    items: &mut Vec<TranscriptItem>,
    model: Option<String>,
    effort: Option<String>,
    time: Option<String>,
) {
    let previous = items.iter().rev().find(|item| item.kind == "model_config");
    if model.is_none() && effort.is_none()
        || previous.is_some_and(|item| item.model == model && item.effort == effort)
    {
        return;
    }

    items.push(model_config_item(model, effort, time));
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

fn attach_codex_subagent_session(value: &Value, items: &mut [TranscriptItem]) -> bool {
    let Some(payload) = value.get("payload") else {
        return false;
    };
    if payload.get("type").and_then(Value::as_str) != Some("sub_agent_activity")
        || payload.get("kind").and_then(Value::as_str) != Some("started")
    {
        return false;
    }
    let Some(event_id) = payload.get("event_id").and_then(Value::as_str) else {
        return false;
    };
    let Some(session_id) = payload.get("agent_thread_id").and_then(Value::as_str) else {
        return false;
    };
    let Some(item) = items.iter_mut().rev().find(|item| {
        item.kind == "tool"
            && item.tag.as_deref() == Some("spawn_agent")
            && item.call_id.as_deref() == Some(event_id)
    }) else {
        return false;
    };
    item.linked_session_id = Some(session_id.to_string());
    true
}
impl super::AgentProvider for CodexProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn storage_key(&self) -> &'static str {
        "codex"
    }

    fn discoverable(&self) -> bool {
        true
    }

    fn global_skill_root(&self, home: &Path) -> Option<PathBuf> {
        Some(
            codex_home(&ProviderContext {
                home: Some(home.to_path_buf()),
                project_dirs: Vec::new(),
            })
            .join("skills"),
        )
    }

    fn config_profile_path(&self, _home: &Path, codex_home: &Path, name: &str) -> Option<PathBuf> {
        Some(codex_home.join(format!("{name}.config.toml")))
    }

    fn config_profile_format(&self) -> Option<&'static str> {
        Some("toml")
    }

    fn config_files(&self, home: &Path, codex_home: &Path) -> Vec<crate::config::AgentConfigFile> {
        let base_path = codex_home.join("config.toml");
        let mut configs = vec![self
            .config_file_for_path(home, codex_home, &base_path)
            .expect("Codex provider must resolve its base config path")];
        configs.extend(
            crate::config::profile_paths_for_root(codex_home, ".config.toml")
                .into_iter()
                .filter_map(|path| self.config_file_for_path(home, codex_home, &path)),
        );
        configs
    }

    fn config_file_for_path(
        &self,
        _home: &Path,
        codex_home: &Path,
        path: &Path,
    ) -> Option<crate::config::AgentConfigFile> {
        let base_path = codex_home.join("config.toml");
        if path == base_path {
            return Some(crate::config::AgentConfigFile {
                agent: self.kind(),
                label: "Codex".to_string(),
                path: base_path.clone(),
                format: "toml".to_string(),
                exists: base_path.is_file(),
                updated_at: None,
                profile: None,
            });
        }

        let profile = path.file_name()?.to_str()?.strip_suffix(".config.toml")?;
        crate::config::validate_profile_name(profile).ok()?;
        let expected_path = codex_home.join(format!("{profile}.config.toml"));
        (path == expected_path).then_some(crate::config::AgentConfigFile {
            agent: self.kind(),
            label: format!("Codex / {profile}"),
            path: expected_path.clone(),
            format: "toml".to_string(),
            exists: expected_path.is_file(),
            updated_at: None,
            profile: Some(profile.to_string()),
        })
    }

    fn config_order(&self) -> usize {
        0
    }

    fn materialized_skill_is_shared_or_codex(&self) -> bool {
        true
    }

    fn matches_name(&self, normalized: &str) -> bool {
        matches_name(normalized)
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

    fn parse_transcript_value(&self, value: &Value, items: &mut Vec<TranscriptItem>) {
        parse_transcript(value, items);
    }

    fn transcript_inherited_history_start_ordinal(&self, value: &Value) -> Option<u64> {
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            return None;
        }
        let payload = value.get("payload")?;
        if payload.get("thread_source").and_then(Value::as_str) != Some("subagent") {
            return None;
        }
        payload
            .get("subagent_history_start_ordinal")
            .and_then(Value::as_u64)
    }

    fn config_profile_key(&self) -> Option<&'static str> {
        Some("codex")
    }

    fn apply_config_profile(&self, command: &mut SessionCommand, profile: &str) -> Result<()> {
        apply_config_profile(command, profile)
    }

    fn transcript_search_hint(&self, line: &str) -> bool {
        may_contain_search_message(line)
    }

    fn recognizes_transcript(&self, value: &Value) -> bool {
        matches!(
            value.get("type").and_then(Value::as_str),
            Some("session_meta" | "response_item" | "event_msg" | "turn_context" | "compacted")
        )
    }

    fn session_supports_append_cache(&self) -> bool {
        true
    }

    fn analytics_capabilities(&self) -> AnalyticsCapabilities {
        AnalyticsCapabilities {
            token_usage: true,
            reasoning_tokens: true,
            explicit_runs: true,
            rate_limit_history: true,
        }
    }

    fn parse_analytics_line(&self, line: &str, record: &mut SessionAnalyticsRecord) {
        crate::analytics::parse_codex_line(line, record);
    }

    fn extract_skill_tool_payloads<'a>(&self, value: &'a Value) -> Vec<(&'a Value, Evidence)> {
        tool_payloads(value)
    }

    fn infer_session_project(&self, _path: &Path, project: Option<PathBuf>) -> Option<PathBuf> {
        project.and_then(|path| sessions::ephemeral_chat_root(&path).or(Some(path)))
    }

    fn session_id_from_path(&self, path: &Path) -> String {
        sessions::codex_session_id_from_path(path)
    }

    fn scan_mcp(
        &self,
        ctx: &ProviderContext,
        servers: &mut Vec<McpServerRecord>,
        warnings: &mut Vec<String>,
    ) -> Result<()> {
        if let Some(home) = &ctx.home {
            let root = codex_home(ctx);
            crate::mcp::scan_toml_mcp(
                &root.join("config.toml"),
                self.kind(),
                "global",
                servers,
                warnings,
            );
            let _ = home;
        }
        for ancestor in ctx.project_dirs() {
            let scope = ancestor.display().to_string();
            crate::mcp::scan_json_mcp(
                &ancestor.join(".codex/mcp.json"),
                self.kind(),
                &scope,
                servers,
                warnings,
            );
            crate::mcp::scan_toml_mcp(
                &ancestor.join(".codex/config.toml"),
                self.kind(),
                &scope,
                servers,
                warnings,
            );
        }
        Ok(())
    }

    fn uses_tendi_hook_review_state(&self) -> bool {
        false
    }

    fn scan_hooks(
        &self,
        ctx: &ProviderContext,
        scanned_files: &mut HashSet<PathBuf>,
        hooks: &mut Vec<HookRecord>,
        warnings: &mut Vec<String>,
    ) {
        let root = codex_home(ctx);
        crate::hooks::scan_hook_file_once(
            &root.join("hooks.json"),
            self.kind(),
            scanned_files,
            hooks,
            warnings,
        );
        crate::hooks::scan_file_once(
            &root.join("config.toml"),
            scanned_files,
            hooks,
            warnings,
            scan_codex_config_hooks,
        );
        for ancestor in ctx.project_dirs() {
            crate::hooks::scan_hook_file_once(
                &ancestor.join(".codex/hooks.json"),
                self.kind(),
                scanned_files,
                hooks,
                warnings,
            );
            crate::hooks::scan_file_once(
                &ancestor.join(".codex/config.toml"),
                scanned_files,
                hooks,
                warnings,
                scan_codex_config_hooks,
            );
        }
    }

    fn parse_hook_file(
        &self,
        path: &Path,
        trust_hash: &str,
        hooks: &mut Vec<HookRecord>,
        warnings: &mut Vec<String>,
    ) -> bool {
        parse_codex_hook_file(path, trust_hash, hooks, warnings)
    }

    fn codex_hook_metadata(
        &self,
        path: &Path,
        event: &str,
        group_index: usize,
        handler_index: usize,
        matcher: Option<&str>,
        command: Option<&str>,
        configured_timeout: Option<u64>,
        is_async: bool,
        status_message: Option<&str>,
        additional_context_limit: Option<usize>,
    ) -> (Option<String>, Option<String>) {
        let key = crate::hooks::codex_hook_key(path, event, group_index, handler_index);
        let hash = command.and_then(|command| {
            crate::hooks::codex_hook_hash(
                event,
                matcher,
                command,
                crate::hooks::codex_hook_timeout(event, configured_timeout),
                is_async,
                status_message,
                additional_context_limit,
            )
        });
        (Some(key), hash)
    }

    fn review_hook(&self, hook: &HookRecord) -> Result<()> {
        let key = hook
            .codex_hook_key
            .as_deref()
            .context("this hook does not support review")?;
        let current_hash = hook
            .codex_current_hash
            .as_deref()
            .filter(|hash| *hash != "unsupported")
            .context("this hook type does not support review")?;
        let home = dirs::home_dir().context("home directory is unavailable")?;
        crate::hooks::write_codex_trusted_hash(
            &codex_home(&ProviderContext {
                home: Some(home),
                project_dirs: Vec::new(),
            })
            .join("config.toml"),
            key,
            current_hash,
        )
    }
}
