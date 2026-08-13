use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use toml::Value as TomlValue;
use walkdir::WalkDir;

use crate::{fsutil::sha256_file, skills::AgentKind};

#[derive(Debug, Clone, Serialize)]
pub struct HookRecord {
    pub agent: AgentKind,
    pub event: String,
    pub matcher: Option<String>,
    pub hook_type: Option<String>,
    pub command: Option<String>,
    pub url: Option<String>,
    pub prompt: Option<String>,
    pub filter: Option<String>,
    pub status_message: Option<String>,
    pub enabled: bool,
    pub path: PathBuf,
    pub trust_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HookScan {
    pub hooks: Vec<HookRecord>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HookSourceContent {
    pub path: PathBuf,
    pub content: String,
    pub sha256: String,
    pub source_type: String,
    pub preview_scope: String,
    pub source_line: Option<u32>,
    pub read_only_reason: Option<String>,
    pub supports_delete: bool,
    pub size_bytes: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HookSourceMatch {
    pub event: String,
    pub matcher: Option<String>,
    pub hook_type: Option<String>,
    pub command: Option<String>,
    pub url: Option<String>,
    pub prompt: Option<String>,
    pub filter: Option<String>,
    pub status_message: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HookDeleteRequest {
    pub path: PathBuf,
    pub expected_trust_hash: String,
    pub event: String,
    pub matcher: Option<String>,
    pub hook_type: Option<String>,
    pub command: Option<String>,
    pub url: Option<String>,
    pub prompt: Option<String>,
    pub filter: Option<String>,
    pub status_message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HookSetEnabledRequest {
    pub path: PathBuf,
    pub expected_trust_hash: String,
    pub event: String,
    pub matcher: Option<String>,
    pub hook_type: Option<String>,
    pub command: Option<String>,
    pub url: Option<String>,
    pub prompt: Option<String>,
    pub filter: Option<String>,
    pub status_message: Option<String>,
    pub enabled: bool,
}

pub fn scan_hooks(cwd: &Path) -> Result<HookScan> {
    let mut hooks = Vec::new();
    let mut warnings = Vec::new();
    let mut scanned_files = HashSet::new();

    if let Some(home) = dirs::home_dir() {
        let codex_home = codex_home(&home);
        scan_hook_file_once(
            &codex_home.join("hooks.json"),
            AgentKind::Codex,
            &mut scanned_files,
            &mut hooks,
            &mut warnings,
        );
        scan_codex_config_hooks_once(
            &codex_home.join("config.toml"),
            &mut scanned_files,
            &mut hooks,
            &mut warnings,
        );
        scan_hook_file_once(
            &home.join(".cursor/hooks.json"),
            AgentKind::Cursor,
            &mut scanned_files,
            &mut hooks,
            &mut warnings,
        );
        scan_hook_file_once(
            &home.join(".claude/settings.json"),
            AgentKind::Claude,
            &mut scanned_files,
            &mut hooks,
            &mut warnings,
        );
        scan_claude_plugin_hooks(&home.join(".claude/plugins"), &mut hooks, &mut warnings);
        scan_claude_component_hooks(&home.join(".claude/skills"), &mut hooks, &mut warnings);
        scan_claude_component_hooks(&home.join(".claude/agents"), &mut hooks, &mut warnings);
    }

    scan_hook_file_once(
        &PathBuf::from("/etc/cursor/hooks.json"),
        AgentKind::Cursor,
        &mut scanned_files,
        &mut hooks,
        &mut warnings,
    );
    scan_hook_file_once(
        &PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json"),
        AgentKind::Claude,
        &mut scanned_files,
        &mut hooks,
        &mut warnings,
    );
    scan_claude_managed_dropins(
        &PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.d"),
        &mut hooks,
        &mut warnings,
    );
    scan_hook_file_once(
        &PathBuf::from("/etc/claude-code/managed-settings.json"),
        AgentKind::Claude,
        &mut scanned_files,
        &mut hooks,
        &mut warnings,
    );
    scan_claude_managed_dropins(
        &PathBuf::from("/etc/claude-code/managed-settings.d"),
        &mut hooks,
        &mut warnings,
    );

    for ancestor in cwd.ancestors() {
        scan_hook_file_once(
            &ancestor.join(".codex/hooks.json"),
            AgentKind::Codex,
            &mut scanned_files,
            &mut hooks,
            &mut warnings,
        );
        scan_codex_config_hooks_once(
            &ancestor.join(".codex/config.toml"),
            &mut scanned_files,
            &mut hooks,
            &mut warnings,
        );
        scan_hook_file_once(
            &ancestor.join(".cursor/hooks.json"),
            AgentKind::Cursor,
            &mut scanned_files,
            &mut hooks,
            &mut warnings,
        );
        scan_hook_file_once(
            &ancestor.join(".claude/settings.json"),
            AgentKind::Claude,
            &mut scanned_files,
            &mut hooks,
            &mut warnings,
        );
        scan_hook_file_once(
            &ancestor.join(".claude/settings.local.json"),
            AgentKind::Claude,
            &mut scanned_files,
            &mut hooks,
            &mut warnings,
        );
        scan_claude_component_hooks(&ancestor.join(".claude/skills"), &mut hooks, &mut warnings);
        scan_claude_component_hooks(&ancestor.join(".claude/agents"), &mut hooks, &mut warnings);
    }

    Ok(HookScan { hooks, warnings })
}

pub fn delete_hook(request: HookDeleteRequest) -> Result<()> {
    ensure_deletable_hook_path(&request.path)?;
    let current_hash = sha256_file(&request.path)?;
    if current_hash != request.expected_trust_hash {
        bail!("hook source changed; refresh hooks before deleting");
    }

    match request.path.extension().and_then(|value| value.to_str()) {
        Some("json") => delete_json_hook(&request),
        Some("toml") => delete_toml_hook(&request),
        _ => bail!("deleting hooks from this source type is not supported"),
    }
}

pub fn set_hook_enabled(request: HookSetEnabledRequest) -> Result<()> {
    ensure_deletable_hook_path(&request.path)?;
    let current_hash = sha256_file(&request.path)?;
    if current_hash != request.expected_trust_hash {
        bail!("hook source changed; refresh hooks before updating");
    }

    match request.path.extension().and_then(|value| value.to_str()) {
        Some("json") => set_json_hook_enabled(&request),
        Some("toml") => set_toml_hook_enabled(&request),
        _ => bail!("updating hooks from this source type is not supported"),
    }
}

pub fn read_hook_source(
    cwd: &Path,
    path: &Path,
    expected_trust_hash: Option<&str>,
    hook_match: Option<&HookSourceMatch>,
) -> Result<HookSourceContent> {
    let scan = scan_hooks(cwd)?;
    if !scan.hooks.iter().any(|hook| hook.path == path) {
        bail!("refusing to read unknown hook source {}", path.display());
    }

    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let sha256 = sha256_file(path)?;
    if expected_trust_hash.is_some_and(|expected| !expected.is_empty() && expected != sha256) {
        bail!("hook source changed; refresh hooks before viewing source");
    }

    let size_bytes = fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(content.len() as u64);

    let source_line = hook_match.and_then(|request| hook_source_line(&content, request));
    let snippet = hook_match.and_then(|request| hook_source_snippet(path, &content, request));
    let (content, preview_scope, truncated) = if let Some(snippet) = snippet {
        (snippet, "hook".to_string(), false)
    } else {
        let truncated = content.len() > 128 * 1024;
        let content = if truncated {
            let mut preview = content.chars().take(128 * 1024).collect::<String>();
            preview.push_str("\n\n[truncated]");
            preview
        } else {
            content
        };
        (content, "source".to_string(), truncated)
    };
    let read_only_reason = hook_management_disabled_reason(path).map(str::to_string);

    Ok(HookSourceContent {
        path: path.to_path_buf(),
        content,
        sha256,
        source_type: path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("unknown")
            .to_string(),
        preview_scope,
        source_line,
        supports_delete: read_only_reason.is_none(),
        read_only_reason,
        size_bytes,
        truncated,
    })
}

fn hook_source_line(content: &str, request: &HookSourceMatch) -> Option<u32> {
    if let Some(line) = request
        .command
        .as_deref()
        .and_then(|needle| line_number_for_needle(content, needle))
    {
        return Some(line);
    }
    if let Some(line) = request
        .url
        .as_deref()
        .and_then(|needle| line_number_for_needle(content, needle))
    {
        return Some(line);
    }
    if let Some(line) = request
        .prompt
        .as_deref()
        .and_then(|needle| line_number_for_needle(content, needle))
    {
        return Some(line);
    }
    line_number_for_needle(content, &format!("\"{}\"", request.event))
}

fn line_number_for_needle(content: &str, needle: &str) -> Option<u32> {
    content
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(needle))
        .map(|(index, _)| (index + 1) as u32)
}

fn hook_source_snippet(path: &Path, content: &str, request: &HookSourceMatch) -> Option<String> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("json") => {
            let value = serde_json::from_str::<Value>(content).ok()?;
            find_json_hook_snippet(&value, request)
                .and_then(|snippet| serde_json::to_string_pretty(&snippet).ok())
        }
        Some("toml") => {
            let value = toml::from_str::<TomlValue>(content).ok()?;
            find_toml_hook_snippet(&value, request)
                .and_then(|snippet| serde_json::to_string_pretty(&snippet).ok())
        }
        Some("md") => {
            let frontmatter = yaml_frontmatter(content)?;
            let value = serde_yaml::from_str::<serde_yaml::Value>(frontmatter).ok()?;
            let value = serde_json::to_value(value).ok()?;
            find_json_hook_snippet(&value, request)
                .and_then(|snippet| serde_json::to_string_pretty(&snippet).ok())
        }
        _ => None,
    }
}

fn find_json_hook_snippet(value: &Value, request: &HookSourceMatch) -> Option<Value> {
    let specs = value
        .get("hooks")
        .and_then(Value::as_object)?
        .get(&request.event)?;
    find_json_event_hook_snippet(specs, request, None, true)
}

fn find_json_event_hook_snippet(
    specs: &Value,
    request: &HookSourceMatch,
    matcher: Option<&str>,
    group_enabled: bool,
) -> Option<Value> {
    if let Some(array) = specs.as_array() {
        for item in array {
            if let Some(snippet) =
                find_json_event_hook_snippet(item, request, matcher, group_enabled)
            {
                return Some(snippet);
            }
        }
        return None;
    }

    let object = specs.as_object()?;
    let next_matcher = object.get("matcher").and_then(Value::as_str).or(matcher);
    let next_enabled = group_enabled
        && object
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);

    if let Some(nested) = object.get("hooks") {
        return find_json_event_hook_snippet(nested, request, next_matcher, next_enabled);
    }

    if !hook_object_matches_source_request(
        next_matcher,
        next_enabled,
        object
            .get("command")
            .or_else(|| object.get("script"))
            .and_then(Value::as_str),
        object.get("url").and_then(Value::as_str),
        object.get("prompt").and_then(Value::as_str),
        object.get("type").and_then(Value::as_str),
        object.get("if").and_then(Value::as_str),
        object
            .get("statusMessage")
            .or_else(|| object.get("status_message"))
            .and_then(Value::as_str),
        request,
    ) {
        return None;
    }

    Some(json!({
        "event": request.event,
        "matcher": next_matcher.unwrap_or("*"),
        "enabled": next_enabled,
        "hook": object,
    }))
}

fn find_toml_hook_snippet(value: &TomlValue, request: &HookSourceMatch) -> Option<Value> {
    let specs = value
        .get("hooks")
        .and_then(TomlValue::as_table)?
        .get(&request.event)?;
    find_toml_event_hook_snippet(specs, request, None, true)
}

fn find_toml_event_hook_snippet(
    specs: &TomlValue,
    request: &HookSourceMatch,
    matcher: Option<String>,
    group_enabled: bool,
) -> Option<Value> {
    if let Some(array) = specs.as_array() {
        for item in array {
            if let Some(snippet) =
                find_toml_event_hook_snippet(item, request, matcher.clone(), group_enabled)
            {
                return Some(snippet);
            }
        }
        return None;
    }

    let table = specs.as_table()?;
    let next_matcher = table
        .get("matcher")
        .and_then(TomlValue::as_str)
        .map(str::to_string)
        .or(matcher);
    let next_enabled = group_enabled
        && table
            .get("enabled")
            .and_then(TomlValue::as_bool)
            .unwrap_or(true);

    if let Some(nested) = table.get("hooks") {
        return find_toml_event_hook_snippet(nested, request, next_matcher, next_enabled);
    }

    if !hook_object_matches_source_request(
        next_matcher.as_deref(),
        next_enabled,
        table
            .get("command")
            .or_else(|| table.get("script"))
            .and_then(TomlValue::as_str),
        table.get("url").and_then(TomlValue::as_str),
        table.get("prompt").and_then(TomlValue::as_str),
        table.get("type").and_then(TomlValue::as_str),
        table.get("if").and_then(TomlValue::as_str),
        table
            .get("statusMessage")
            .or_else(|| table.get("status_message"))
            .and_then(TomlValue::as_str),
        request,
    ) {
        return None;
    }

    let hook = serde_json::to_value(table).ok()?;
    Some(json!({
        "event": request.event,
        "matcher": next_matcher.as_deref().unwrap_or("*"),
        "enabled": next_enabled,
        "hook": hook,
    }))
}

fn hook_object_matches_source_request(
    matcher: Option<&str>,
    enabled: bool,
    command: Option<&str>,
    url: Option<&str>,
    prompt: Option<&str>,
    explicit_hook_type: Option<&str>,
    filter: Option<&str>,
    status_message: Option<&str>,
    request: &HookSourceMatch,
) -> bool {
    let hook_type = explicit_hook_type.or_else(|| {
        if command.is_some() {
            Some("command")
        } else if url.is_some() {
            Some("http")
        } else if prompt.is_some() {
            Some("prompt")
        } else {
            None
        }
    });

    request.matcher.as_deref() == matcher
        && request.hook_type.as_deref() == hook_type
        && request.command.as_deref() == command
        && request.url.as_deref() == url
        && request.prompt.as_deref() == prompt
        && request.filter.as_deref() == filter
        && request.status_message.as_deref() == status_message
        && request.enabled.is_none_or(|expected| expected == enabled)
}

fn ensure_deletable_hook_path(path: &Path) -> Result<()> {
    if let Some(reason) = hook_management_disabled_reason(path) {
        bail!("{reason}");
    }
    Ok(())
}

fn hook_management_disabled_reason(path: &Path) -> Option<&'static str> {
    let text = path.to_string_lossy();
    if text.starts_with("/etc/cursor/")
        || text.starts_with("/etc/claude-code/")
        || text.starts_with("/Library/Application Support/ClaudeCode/")
        || text.contains("/.claude/plugins/")
    {
        return Some("this hook source is read-only");
    }
    if !matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("json" | "toml")
    ) {
        return Some("managing hooks from this source type is not supported");
    }
    None
}

fn delete_json_hook(request: &HookDeleteRequest) -> Result<()> {
    let text = fs::read_to_string(&request.path)?;
    let mut value = serde_json::from_str::<Value>(&text)?;
    if !remove_json_hook_from_value(&mut value, request) {
        bail!("matching hook was not found");
    }
    fs::write(
        &request.path,
        format!("{}\n", serde_json::to_string_pretty(&value)?),
    )?;
    Ok(())
}

fn delete_toml_hook(request: &HookDeleteRequest) -> Result<()> {
    let text = fs::read_to_string(&request.path)?;
    let mut value = toml::from_str::<TomlValue>(&text)?;
    if !remove_toml_hook_from_value(&mut value, request) {
        bail!("matching hook was not found");
    }
    fs::write(&request.path, toml::to_string_pretty(&value)?)?;
    Ok(())
}

fn set_json_hook_enabled(request: &HookSetEnabledRequest) -> Result<()> {
    let text = fs::read_to_string(&request.path)?;
    let mut value = serde_json::from_str::<Value>(&text)?;
    if !set_json_hook_enabled_in_value(&mut value, request) {
        bail!("matching hook was not found");
    }
    fs::write(
        &request.path,
        format!("{}\n", serde_json::to_string_pretty(&value)?),
    )?;
    Ok(())
}

fn set_toml_hook_enabled(request: &HookSetEnabledRequest) -> Result<()> {
    let text = fs::read_to_string(&request.path)?;
    let mut value = toml::from_str::<TomlValue>(&text)?;
    if !set_toml_hook_enabled_in_value(&mut value, request) {
        bail!("matching hook was not found");
    }
    fs::write(&request.path, toml::to_string_pretty(&value)?)?;
    Ok(())
}

fn codex_home(home: &Path) -> PathBuf {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
}

fn canonical_scan_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn scan_hook_file_once(
    path: &Path,
    agent: AgentKind,
    scanned_files: &mut HashSet<PathBuf>,
    hooks: &mut Vec<HookRecord>,
    warnings: &mut Vec<String>,
) {
    if !path.is_file() {
        return;
    }
    if !scanned_files.insert(canonical_scan_key(path)) {
        return;
    }
    scan_hook_file(path, agent, hooks, warnings);
}

fn scan_codex_config_hooks_once(
    path: &Path,
    scanned_files: &mut HashSet<PathBuf>,
    hooks: &mut Vec<HookRecord>,
    warnings: &mut Vec<String>,
) {
    if !path.is_file() {
        return;
    }
    if !scanned_files.insert(canonical_scan_key(path)) {
        return;
    }
    scan_codex_config_hooks(path, hooks, warnings);
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
        for group in groups {
            collect_event_hooks(AgentKind::Codex, path, trust_hash, event, group, hooks);
        }
    }
}

fn scan_hook_file(
    path: &Path,
    agent: AgentKind,
    hooks: &mut Vec<HookRecord>,
    warnings: &mut Vec<String>,
) {
    if !path.is_file() {
        return;
    }

    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return;
        }
    };
    let trust_hash = match sha256_file(path) {
        Ok(hash) => hash,
        Err(err) => {
            warnings.push(format!("{}: {err:#}", path.display()));
            return;
        }
    };
    if agent == AgentKind::Codex {
        let parsed = match serde_json::from_str::<CodexHooksFile>(&text) {
            Ok(parsed) => parsed,
            Err(err) => {
                warnings.push(format!("{}: {err}", path.display()));
                return;
            }
        };
        collect_codex_hook_events(path, &trust_hash, &parsed.hooks, hooks);
        return;
    }

    let value = match serde_json::from_str::<Value>(&text) {
        Ok(value) => value,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return;
        }
    };

    collect_hooks_from_value(agent, path, &trust_hash, &value, hooks);
}

fn scan_codex_config_hooks(path: &Path, hooks: &mut Vec<HookRecord>, warnings: &mut Vec<String>) {
    if !path.is_file() {
        return;
    }

    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return;
        }
    };
    let trust_hash = match sha256_file(path) {
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
    if events.is_empty() {
        return;
    }

    collect_codex_hook_events(path, &trust_hash, &events, hooks);
}

fn scan_claude_plugin_hooks(root: &Path, hooks: &mut Vec<HookRecord>, warnings: &mut Vec<String>) {
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
        scan_hook_file(entry.path(), AgentKind::Claude, hooks, warnings);
    }
}

fn scan_claude_managed_dropins(
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
        scan_hook_file(&path, AgentKind::Claude, hooks, warnings);
    }
}

fn scan_claude_component_hooks(
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
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().and_then(|value| value.to_str()) == Some("md")
        })
    {
        scan_claude_component_file(entry.path(), hooks, warnings);
    }
}

fn scan_claude_component_file(
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
    let value = match serde_yaml::from_str::<serde_yaml::Value>(frontmatter) {
        Ok(value) => value,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return;
        }
    };
    let value = match serde_json::to_value(value) {
        Ok(value) => value,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return;
        }
    };
    if value.get("hooks").is_none() {
        return;
    }
    let trust_hash = match sha256_file(path) {
        Ok(hash) => hash,
        Err(err) => {
            warnings.push(format!("{}: {err:#}", path.display()));
            return;
        }
    };
    collect_hooks_from_value(AgentKind::Claude, path, &trust_hash, &value, hooks);
}

fn yaml_frontmatter(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

fn remove_json_hook_from_value(value: &mut Value, request: &HookDeleteRequest) -> bool {
    let Some(hook_map) = value.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let Some(specs) = hook_map.get_mut(&request.event) else {
        return false;
    };
    let removed = remove_json_event_hook(specs, request, None, true);
    if json_is_empty_hook_specs(specs) {
        hook_map.remove(&request.event);
    }
    removed
}

fn remove_json_event_hook(
    specs: &mut Value,
    request: &HookDeleteRequest,
    matcher: Option<&str>,
    group_enabled: bool,
) -> bool {
    if let Some(array) = specs.as_array_mut() {
        let mut index = 0;
        while index < array.len() {
            let removed =
                remove_json_event_hook(&mut array[index], request, matcher, group_enabled);
            let remove_current = json_is_empty_hook_specs(&array[index])
                || (removed
                    && !array[index]
                        .as_object()
                        .is_some_and(|object| object.contains_key("hooks")));
            if remove_current {
                array.remove(index);
            } else {
                index += 1;
            }
            if removed {
                return true;
            }
        }
        return false;
    }

    let Some(object) = specs.as_object_mut() else {
        return false;
    };
    let next_matcher = object
        .get("matcher")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| matcher.map(str::to_string));
    let next_enabled = group_enabled
        && object
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);

    if let Some(nested) = object.get_mut("hooks") {
        let removed =
            remove_json_event_hook(nested, request, next_matcher.as_deref(), next_enabled);
        if json_is_empty_hook_specs(nested) {
            object.remove("hooks");
        }
        return removed;
    }

    hook_object_matches_json(
        &request.event,
        next_matcher.as_deref(),
        next_enabled,
        object,
        request,
    )
}

fn json_is_empty_hook_specs(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.is_empty(),
        Value::Object(object) => object.get("hooks").is_some_and(json_is_empty_hook_specs),
        _ => false,
    }
}

fn hook_object_matches_json(
    event: &str,
    matcher: Option<&str>,
    group_enabled: bool,
    object: &serde_json::Map<String, Value>,
    request: &HookDeleteRequest,
) -> bool {
    let command = object
        .get("command")
        .or_else(|| object.get("script"))
        .and_then(Value::as_str);
    let url = object.get("url").and_then(Value::as_str);
    let prompt = object.get("prompt").and_then(Value::as_str);
    let hook_type = object.get("type").and_then(Value::as_str).or_else(|| {
        if command.is_some() {
            Some("command")
        } else if url.is_some() {
            Some("http")
        } else if prompt.is_some() {
            Some("prompt")
        } else {
            None
        }
    });
    let filter = object.get("if").and_then(Value::as_str);
    let status_message = object
        .get("statusMessage")
        .or_else(|| object.get("status_message"))
        .and_then(Value::as_str);
    let enabled = group_enabled
        && object
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);

    request.event == event
        && request.matcher.as_deref() == matcher
        && request.hook_type.as_deref() == hook_type
        && request.command.as_deref() == command
        && request.url.as_deref() == url
        && request.prompt.as_deref() == prompt
        && request.filter.as_deref() == filter
        && request.status_message.as_deref() == status_message
        && enabled
}

fn set_json_hook_enabled_in_value(value: &mut Value, request: &HookSetEnabledRequest) -> bool {
    let Some(hook_map) = value.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let Some(specs) = hook_map.get_mut(&request.event) else {
        return false;
    };
    set_json_event_hook_enabled(specs, request, None)
}

fn set_json_event_hook_enabled(
    specs: &mut Value,
    request: &HookSetEnabledRequest,
    matcher: Option<&str>,
) -> bool {
    if let Some(array) = specs.as_array_mut() {
        return array
            .iter_mut()
            .any(|item| set_json_event_hook_enabled(item, request, matcher));
    }

    let Some(object) = specs.as_object_mut() else {
        return false;
    };
    let next_matcher = object
        .get("matcher")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| matcher.map(str::to_string));

    if let Some(nested) = object.get_mut("hooks") {
        return set_json_event_hook_enabled(nested, request, next_matcher.as_deref());
    }

    if !hook_object_matches_enabled_request(next_matcher.as_deref(), object, request) {
        return false;
    }
    object.insert("enabled".to_string(), Value::Bool(request.enabled));
    true
}

fn hook_object_matches_enabled_request(
    matcher: Option<&str>,
    object: &serde_json::Map<String, Value>,
    request: &HookSetEnabledRequest,
) -> bool {
    let command = object
        .get("command")
        .or_else(|| object.get("script"))
        .and_then(Value::as_str);
    let url = object.get("url").and_then(Value::as_str);
    let prompt = object.get("prompt").and_then(Value::as_str);
    let hook_type = object.get("type").and_then(Value::as_str).or_else(|| {
        if command.is_some() {
            Some("command")
        } else if url.is_some() {
            Some("http")
        } else if prompt.is_some() {
            Some("prompt")
        } else {
            None
        }
    });
    let filter = object.get("if").and_then(Value::as_str);
    let status_message = object
        .get("statusMessage")
        .or_else(|| object.get("status_message"))
        .and_then(Value::as_str);

    request.matcher.as_deref() == matcher
        && request.hook_type.as_deref() == hook_type
        && request.command.as_deref() == command
        && request.url.as_deref() == url
        && request.prompt.as_deref() == prompt
        && request.filter.as_deref() == filter
        && request.status_message.as_deref() == status_message
}

fn remove_toml_hook_from_value(value: &mut TomlValue, request: &HookDeleteRequest) -> bool {
    let Some(hook_map) = value.get_mut("hooks").and_then(TomlValue::as_table_mut) else {
        return false;
    };
    let Some(specs) = hook_map.get_mut(&request.event) else {
        return false;
    };
    let removed = remove_toml_event_hook(specs, request, None, true);
    if toml_is_empty_hook_specs(specs) {
        hook_map.remove(&request.event);
    }
    removed
}

fn remove_toml_event_hook(
    specs: &mut TomlValue,
    request: &HookDeleteRequest,
    matcher: Option<String>,
    group_enabled: bool,
) -> bool {
    if let Some(array) = specs.as_array_mut() {
        let mut index = 0;
        while index < array.len() {
            let removed =
                remove_toml_event_hook(&mut array[index], request, matcher.clone(), group_enabled);
            let remove_current = toml_is_empty_hook_specs(&array[index])
                || (removed
                    && !array[index]
                        .as_table()
                        .is_some_and(|table| table.contains_key("hooks")));
            if remove_current {
                array.remove(index);
            } else {
                index += 1;
            }
            if removed {
                return true;
            }
        }
        return false;
    }

    let Some(table) = specs.as_table_mut() else {
        return false;
    };
    let next_matcher = table
        .get("matcher")
        .and_then(TomlValue::as_str)
        .map(str::to_string)
        .or(matcher);
    let next_enabled = group_enabled
        && table
            .get("enabled")
            .and_then(TomlValue::as_bool)
            .unwrap_or(true);

    if let Some(nested) = table.get_mut("hooks") {
        let removed = remove_toml_event_hook(nested, request, next_matcher.clone(), next_enabled);
        if toml_is_empty_hook_specs(nested) {
            table.remove("hooks");
        }
        return removed;
    }

    hook_object_matches_toml(
        &request.event,
        next_matcher.as_deref(),
        next_enabled,
        table,
        request,
    )
}

fn toml_is_empty_hook_specs(value: &TomlValue) -> bool {
    match value {
        TomlValue::Array(items) => items.is_empty(),
        TomlValue::Table(table) => table.get("hooks").is_some_and(toml_is_empty_hook_specs),
        _ => false,
    }
}

fn hook_object_matches_toml(
    event: &str,
    matcher: Option<&str>,
    group_enabled: bool,
    table: &toml::map::Map<String, TomlValue>,
    request: &HookDeleteRequest,
) -> bool {
    let command = table
        .get("command")
        .or_else(|| table.get("script"))
        .and_then(TomlValue::as_str);
    let url = table.get("url").and_then(TomlValue::as_str);
    let prompt = table.get("prompt").and_then(TomlValue::as_str);
    let hook_type = table.get("type").and_then(TomlValue::as_str).or_else(|| {
        if command.is_some() {
            Some("command")
        } else if url.is_some() {
            Some("http")
        } else if prompt.is_some() {
            Some("prompt")
        } else {
            None
        }
    });
    let filter = table.get("if").and_then(TomlValue::as_str);
    let status_message = table
        .get("statusMessage")
        .or_else(|| table.get("status_message"))
        .and_then(TomlValue::as_str);
    let enabled = group_enabled
        && table
            .get("enabled")
            .and_then(TomlValue::as_bool)
            .unwrap_or(true);

    request.event == event
        && request.matcher.as_deref() == matcher
        && request.hook_type.as_deref() == hook_type
        && request.command.as_deref() == command
        && request.url.as_deref() == url
        && request.prompt.as_deref() == prompt
        && request.filter.as_deref() == filter
        && request.status_message.as_deref() == status_message
        && enabled
}

fn set_toml_hook_enabled_in_value(value: &mut TomlValue, request: &HookSetEnabledRequest) -> bool {
    let Some(hook_map) = value.get_mut("hooks").and_then(TomlValue::as_table_mut) else {
        return false;
    };
    let Some(specs) = hook_map.get_mut(&request.event) else {
        return false;
    };
    set_toml_event_hook_enabled(specs, request, None)
}

fn set_toml_event_hook_enabled(
    specs: &mut TomlValue,
    request: &HookSetEnabledRequest,
    matcher: Option<String>,
) -> bool {
    if let Some(array) = specs.as_array_mut() {
        return array
            .iter_mut()
            .any(|item| set_toml_event_hook_enabled(item, request, matcher.clone()));
    }

    let Some(table) = specs.as_table_mut() else {
        return false;
    };
    let next_matcher = table
        .get("matcher")
        .and_then(TomlValue::as_str)
        .map(str::to_string)
        .or(matcher);

    if let Some(nested) = table.get_mut("hooks") {
        return set_toml_event_hook_enabled(nested, request, next_matcher);
    }

    if !hook_table_matches_enabled_request(next_matcher.as_deref(), table, request) {
        return false;
    }
    table.insert("enabled".to_string(), TomlValue::Boolean(request.enabled));
    true
}

fn hook_table_matches_enabled_request(
    matcher: Option<&str>,
    table: &toml::map::Map<String, TomlValue>,
    request: &HookSetEnabledRequest,
) -> bool {
    let command = table
        .get("command")
        .or_else(|| table.get("script"))
        .and_then(TomlValue::as_str);
    let url = table.get("url").and_then(TomlValue::as_str);
    let prompt = table.get("prompt").and_then(TomlValue::as_str);
    let hook_type = table.get("type").and_then(TomlValue::as_str).or_else(|| {
        if command.is_some() {
            Some("command")
        } else if url.is_some() {
            Some("http")
        } else if prompt.is_some() {
            Some("prompt")
        } else {
            None
        }
    });
    let filter = table.get("if").and_then(TomlValue::as_str);
    let status_message = table
        .get("statusMessage")
        .or_else(|| table.get("status_message"))
        .and_then(TomlValue::as_str);

    request.matcher.as_deref() == matcher
        && request.hook_type.as_deref() == hook_type
        && request.command.as_deref() == command
        && request.url.as_deref() == url
        && request.prompt.as_deref() == prompt
        && request.filter.as_deref() == filter
        && request.status_message.as_deref() == status_message
}

fn collect_hooks_from_value(
    agent: AgentKind,
    path: &Path,
    trust_hash: &str,
    value: &Value,
    hooks: &mut Vec<HookRecord>,
) {
    let Some(hook_map) = value.get("hooks").and_then(Value::as_object) else {
        return;
    };

    for (event, specs) in hook_map {
        collect_event_hooks(agent, path, trust_hash, event, specs, hooks);
    }
}

fn collect_event_hooks(
    agent: AgentKind,
    path: &Path,
    trust_hash: &str,
    event: &str,
    specs: &Value,
    hooks: &mut Vec<HookRecord>,
) {
    if let Some(array) = specs.as_array() {
        for item in array {
            collect_event_hooks(agent, path, trust_hash, event, item, hooks);
        }
        return;
    }

    let Some(object) = specs.as_object() else {
        return;
    };

    let matcher = object
        .get("matcher")
        .and_then(Value::as_str)
        .map(str::to_string);
    let group_enabled = object
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    if let Some(nested) = object.get("hooks") {
        collect_group_hooks(
            agent,
            path,
            trust_hash,
            event,
            matcher.as_deref(),
            group_enabled,
            nested,
            hooks,
        );
        return;
    }

    push_hook_record(
        agent,
        path,
        trust_hash,
        event,
        matcher.as_deref(),
        group_enabled,
        object,
        hooks,
    );
}

fn collect_group_hooks(
    agent: AgentKind,
    path: &Path,
    trust_hash: &str,
    event: &str,
    matcher: Option<&str>,
    group_enabled: bool,
    specs: &Value,
    hooks: &mut Vec<HookRecord>,
) {
    if let Some(array) = specs.as_array() {
        for item in array {
            collect_group_hooks(
                agent,
                path,
                trust_hash,
                event,
                matcher,
                group_enabled,
                item,
                hooks,
            );
        }
        return;
    }

    let Some(object) = specs.as_object() else {
        return;
    };

    if let Some(nested) = object.get("hooks") {
        let next_matcher = object.get("matcher").and_then(Value::as_str).or(matcher);
        let next_enabled = group_enabled
            && object
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
        collect_group_hooks(
            agent,
            path,
            trust_hash,
            event,
            next_matcher,
            next_enabled,
            nested,
            hooks,
        );
        return;
    }

    push_hook_record(
        agent,
        path,
        trust_hash,
        event,
        matcher,
        group_enabled,
        object,
        hooks,
    );
}

fn push_hook_record(
    agent: AgentKind,
    path: &Path,
    trust_hash: &str,
    event: &str,
    matcher: Option<&str>,
    group_enabled: bool,
    object: &serde_json::Map<String, Value>,
    hooks: &mut Vec<HookRecord>,
) {
    let command = object
        .get("command")
        .or_else(|| object.get("script"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let url = object
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_string);
    let prompt = object
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::to_string);
    let hook_type = object
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| {
            if command.is_some() {
                Some("command")
            } else if url.is_some() {
                Some("http")
            } else if prompt.is_some() {
                Some("prompt")
            } else {
                None
            }
        })
        .map(str::to_string);
    let enabled = group_enabled
        && object
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);

    if hook_type.is_none() && command.is_none() && url.is_none() && prompt.is_none() {
        return;
    }

    hooks.push(HookRecord {
        agent,
        event: event.to_string(),
        matcher: matcher.map(str::to_string),
        hook_type,
        command,
        url,
        prompt,
        filter: object.get("if").and_then(Value::as_str).map(str::to_string),
        status_message: object
            .get("statusMessage")
            .or_else(|| object.get("status_message"))
            .and_then(Value::as_str)
            .map(str::to_string),
        enabled,
        path: path.to_path_buf(),
        trust_hash: trust_hash.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::scan_hook_file;
    use crate::skills::AgentKind;

    #[test]
    fn scans_codex_nested_command_hooks_without_running_them() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("hooks.json");
        fs::write(
            &path,
            r#"{
  "hooks": {
    "PreToolUse": [
      {
        "hooks": [
          {
            "command": "/bin/echo checked",
            "type": "command",
            "enabled": false
          }
        ]
      }
    ]
  }
}"#,
        )
        .expect("write hooks");
        let mut hooks = Vec::new();
        let mut warnings = Vec::new();

        scan_hook_file(&path, AgentKind::Codex, &mut hooks, &mut warnings);

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].agent, AgentKind::Codex);
        assert_eq!(hooks[0].event, "PreToolUse");
        assert_eq!(hooks[0].hook_type.as_deref(), Some("command"));
        assert_eq!(hooks[0].command.as_deref(), Some("/bin/echo checked"));
        assert!(!hooks[0].enabled);
        assert!(!hooks[0].trust_hash.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scans_codex_inline_toml_hooks() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-toml-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("config.toml");
        fs::write(
            &path,
            r#"
[[hooks.PreToolUse]]
matcher = "^Bash$"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "/bin/echo inline"
statusMessage = "Checking"
"#,
        )
        .expect("write hooks");
        let mut hooks = Vec::new();
        let mut warnings = Vec::new();

        super::scan_codex_config_hooks(&path, &mut hooks, &mut warnings);

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].agent, AgentKind::Codex);
        assert_eq!(hooks[0].event, "PreToolUse");
        assert_eq!(hooks[0].matcher.as_deref(), Some("^Bash$"));
        assert_eq!(hooks[0].command.as_deref(), Some("/bin/echo inline"));
        assert_eq!(hooks[0].status_message.as_deref(), Some("Checking"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ignores_codex_hooks_state_metadata_in_config_toml() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-state-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("config.toml");
        fs::write(
            &path,
            r#"
[hooks.state."/tmp/hooks.json:PreToolUse:0:0"]
trusted_hash = "sha256:abc"

[[hooks.SessionStart]]
[[hooks.SessionStart.hooks]]
type = "command"
command = "/bin/echo hello"
"#,
        )
        .expect("write hooks");
        let mut hooks = Vec::new();
        let mut warnings = Vec::new();

        super::scan_codex_config_hooks(&path, &mut hooks, &mut warnings);

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].event, "SessionStart");
        assert_eq!(hooks[0].command.as_deref(), Some("/bin/echo hello"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_hooks_deduplicates_codex_config_layers() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-dedupe-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        let codex_dir = root.join(".codex");
        fs::create_dir_all(&codex_dir).expect("create codex dir");
        let config_path = codex_dir.join("config.toml");
        fs::write(
            &config_path,
            r#"
[[hooks.Stop]]
[[hooks.Stop.hooks]]
type = "command"
command = "/bin/echo stop"
"#,
        )
        .expect("write config");

        let scan = super::scan_hooks(&root).expect("scan hooks");
        let stop_hooks = scan
            .hooks
            .into_iter()
            .filter(|hook| hook.path == config_path && hook.event == "Stop")
            .collect::<Vec<_>>();

        assert_eq!(stop_hooks.len(), 1);
        assert_eq!(stop_hooks[0].command.as_deref(), Some("/bin/echo stop"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn deletes_matching_json_hook() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-delete-json-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("hooks.json");
        fs::write(
            &path,
            r#"{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "/bin/echo delete-me",
            "statusMessage": "Checking"
          }
        ]
      }
    ]
  }
}"#,
        )
        .expect("write hooks");
        let trust_hash = super::sha256_file(&path).expect("hash");

        super::delete_hook(super::HookDeleteRequest {
            path: path.clone(),
            expected_trust_hash: trust_hash,
            event: "PreToolUse".to_string(),
            matcher: Some("Bash".to_string()),
            hook_type: Some("command".to_string()),
            command: Some("/bin/echo delete-me".to_string()),
            url: None,
            prompt: None,
            filter: None,
            status_message: Some("Checking".to_string()),
        })
        .expect("delete hook");

        let mut hooks = Vec::new();
        let mut warnings = Vec::new();
        scan_hook_file(&path, AgentKind::Codex, &mut hooks, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(hooks.is_empty(), "{hooks:?}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn deletes_matching_codex_toml_hook() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-delete-toml-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("config.toml");
        fs::write(
            &path,
            r#"
[[hooks.PreToolUse]]
matcher = "Bash"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "/bin/echo delete-me"
"#,
        )
        .expect("write hooks");
        let trust_hash = super::sha256_file(&path).expect("hash");

        super::delete_hook(super::HookDeleteRequest {
            path: path.clone(),
            expected_trust_hash: trust_hash,
            event: "PreToolUse".to_string(),
            matcher: Some("Bash".to_string()),
            hook_type: Some("command".to_string()),
            command: Some("/bin/echo delete-me".to_string()),
            url: None,
            prompt: None,
            filter: None,
            status_message: None,
        })
        .expect("delete hook");

        let mut hooks = Vec::new();
        let mut warnings = Vec::new();
        super::scan_codex_config_hooks(&path, &mut hooks, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(hooks.is_empty(), "{hooks:?}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn toggles_matching_json_hook_enabled_state() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-enable-json-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("hooks.json");
        fs::write(
            &path,
            r#"{
  "hooks": {
    "PreToolUse": [
      {
        "type": "command",
        "command": "/bin/echo toggle-me"
      }
    ]
  }
}"#,
        )
        .expect("write hooks");
        let trust_hash = super::sha256_file(&path).expect("hash");

        super::set_hook_enabled(super::HookSetEnabledRequest {
            path: path.clone(),
            expected_trust_hash: trust_hash,
            event: "PreToolUse".to_string(),
            matcher: None,
            hook_type: Some("command".to_string()),
            command: Some("/bin/echo toggle-me".to_string()),
            url: None,
            prompt: None,
            filter: None,
            status_message: None,
            enabled: false,
        })
        .expect("toggle hook");

        let mut hooks = Vec::new();
        let mut warnings = Vec::new();
        scan_hook_file(&path, AgentKind::Codex, &mut hooks, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(hooks.len(), 1);
        assert!(!hooks[0].enabled);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn toggles_matching_toml_hook_enabled_state() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-enable-toml-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("config.toml");
        fs::write(
            &path,
            r#"
[[hooks.PreToolUse]]
matcher = "Bash"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "/bin/echo toggle-me"
enabled = false
"#,
        )
        .expect("write hooks");
        let trust_hash = super::sha256_file(&path).expect("hash");

        super::set_hook_enabled(super::HookSetEnabledRequest {
            path: path.clone(),
            expected_trust_hash: trust_hash,
            event: "PreToolUse".to_string(),
            matcher: Some("Bash".to_string()),
            hook_type: Some("command".to_string()),
            command: Some("/bin/echo toggle-me".to_string()),
            url: None,
            prompt: None,
            filter: None,
            status_message: None,
            enabled: true,
        })
        .expect("toggle hook");

        let mut hooks = Vec::new();
        let mut warnings = Vec::new();
        super::scan_codex_config_hooks(&path, &mut hooks, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(hooks.len(), 1);
        assert!(hooks[0].enabled);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reads_known_hook_source_with_management_metadata() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-read-source-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        let codex_dir = root.join(".codex");
        fs::create_dir_all(&codex_dir).expect("create codex dir");
        let path = codex_dir.join("hooks.json");
        fs::write(
            &path,
            r#"{
  "hooks": {
    "PreToolUse": [
      {
        "type": "command",
        "command": "/bin/echo inspect"
      },
      {
        "type": "command",
        "command": "/bin/echo other"
      }
    ]
  }
}"#,
        )
        .expect("write hooks");
        let trust_hash = super::sha256_file(&path).expect("hash");

        let source = super::read_hook_source(
            &root,
            &path,
            Some(&trust_hash),
            Some(&super::HookSourceMatch {
                event: "PreToolUse".to_string(),
                matcher: None,
                hook_type: Some("command".to_string()),
                command: Some("/bin/echo inspect".to_string()),
                url: None,
                prompt: None,
                filter: None,
                status_message: None,
                enabled: Some(true),
            }),
        )
        .expect("read source");

        assert_eq!(source.path, path);
        assert_eq!(source.sha256, trust_hash);
        assert_eq!(source.source_type, "json");
        assert_eq!(source.preview_scope, "hook");
        assert_eq!(source.source_line, Some(6));
        assert!(source.content.contains("/bin/echo inspect"));
        assert!(!source.content.contains("/bin/echo other"));
        assert!(source.supports_delete);
        assert!(source.read_only_reason.is_none());
        assert!(!source.truncated);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refuses_to_read_unknown_hook_source() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-read-unknown-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("not-a-hook.json");
        fs::write(&path, "{}").expect("write file");

        let error = super::read_hook_source(&root, &path, None, None)
            .expect_err("unknown hook source should be rejected")
            .to_string();

        assert!(error.contains("refusing to read unknown hook source"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scans_claude_skill_frontmatter_hooks() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-frontmatter-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("SKILL.md");
        fs::write(
            &path,
            r#"---
name: secure-operations
description: Security checks
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: command
          command: "./scripts/security-check.sh"
---

# Skill
"#,
        )
        .expect("write skill");
        let mut hooks = Vec::new();
        let mut warnings = Vec::new();

        super::scan_claude_component_file(&path, &mut hooks, &mut warnings);

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].agent, AgentKind::Claude);
        assert_eq!(hooks[0].event, "PreToolUse");
        assert_eq!(hooks[0].matcher.as_deref(), Some("Bash"));
        assert_eq!(
            hooks[0].command.as_deref(),
            Some("./scripts/security-check.sh")
        );
        let _ = fs::remove_dir_all(root);
    }
}
