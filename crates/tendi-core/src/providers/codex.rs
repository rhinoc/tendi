use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    env,
    fs::{self, OpenOptions, TryLockError},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_yaml::Value as YamlValue;
use sha2::Digest;
use toml::Value as TomlValue;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};
use walkdir::WalkDir;

use crate::transcript::{
    InternalContextMarker, TranscriptItem, attach_tool_result,
    collect_message_content_with_markers, compact_time, extract_call_id, extract_duration_ms,
    extract_raw_content_text, extract_thinking_text, extract_tool_command, extract_tool_result,
    parse_timestamp_ms, push_item, push_tool_item, summarize_tool_call,
};

use super::*;

pub(super) struct CodexProvider;

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

fn infer_mcp_toml_transport(spec: &TomlValue) -> Option<String> {
    spec.get("transport")
        .or_else(|| spec.get("type"))
        .and_then(TomlValue::as_str)
        .map(str::to_string)
        .or_else(|| spec.get("command").and_then(TomlValue::as_str).map(|_| "stdio".to_string()))
        .or_else(|| {
            spec.get("url").and_then(TomlValue::as_str).map(|url| {
                if url.contains("/sse") { "sse" } else { "http" }.to_string()
            })
        })
}

fn infer_mcp_toml_status(spec: &TomlValue) -> String {
    if infer_mcp_toml_enabled(spec) {
        "configured"
    } else {
        "disabled"
    }
    .to_string()
}

fn infer_mcp_toml_enabled(spec: &TomlValue) -> bool {
    !spec
        .get("disabled")
        .and_then(TomlValue::as_bool)
        .unwrap_or(false)
        && spec
            .get("enabled")
            .and_then(TomlValue::as_bool)
            .unwrap_or(true)
}

fn update_mcp_json_server(spec: &mut serde_json::Map<String, Value>, enabled: bool) -> bool {
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

fn update_mcp_toml_server(spec: &mut toml::map::Map<String, TomlValue>, enabled: bool) -> bool {
    let has_disabled = spec.contains_key("disabled");
    let has_enabled = spec.contains_key("enabled");
    if has_disabled {
        spec.insert("disabled".to_string(), TomlValue::Boolean(!enabled));
    } else if has_enabled {
        spec.insert("enabled".to_string(), TomlValue::Boolean(enabled));
    } else {
        spec.insert("enabled".to_string(), TomlValue::Boolean(enabled));
    }
    true
}

const CODEX_BUNDLED_SKILL_FILES: [(&str, &str); 1] = [(
    "agents/openai.yaml",
    include_str!("../../../../skills/tendi/agents/openai.yaml"),
)];

const CODEX_INTERNAL_CONTEXT_MARKERS: [InternalContextMarker; 1] = [(
    "<codex_internal_context",
    "Codex internal",
    Some("</codex_internal_context>"),
)];

pub(crate) fn scan_session_index(
    path: &Path,
    sessions: &mut Vec<SessionRecord>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };

    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(value) => {
                if let Some(id) = value.get("id").and_then(Value::as_str) {
                    sessions.push(SessionRecord {
                        id: id.to_string(),
                        agent: AgentKind::Codex,
                        title: value
                            .get("thread_name")
                            .and_then(Value::as_str)
                            .and_then(crate::sessions::clean_title),
                        project: None,
                        repository: None,
                        repository_url: None,
                        logical_project_id: None,
                        logical_project_name: None,
                        path: path.to_path_buf(),
                        started_at: value
                            .get("started_at")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        updated_at: value
                            .get("updated_at")
                            .and_then(Value::as_str)
                            .map(str::to_string),
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
                    });
                }
            }
            Err(err) => warnings.push(format!("{}:{}: {err}", path.display(), index + 1)),
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn scan_jsonl_sessions_for_test(
    root: &Path,
    sessions: &mut Vec<SessionRecord>,
    cache: Option<&SessionScanCache>,
) {
    crate::sessions::scan_jsonl_sessions(root, AgentKind::Codex, 6, sessions, cache);
}

pub(crate) fn session_id_from_path(path: &Path) -> Option<String> {
    let raw_id = path
        .file_stem()
        .and_then(|name| name.to_str())
        .map(|name| name.trim_start_matches("rollout-"))?;
    if raw_id.is_empty() {
        return None;
    }
    Some(if raw_id.len() >= 36 {
        raw_id[raw_id.len() - 36..].to_string()
    } else {
        raw_id.to_string()
    })
}

fn normalize_ephemeral_chat_root(path: PathBuf) -> PathBuf {
    let Some(parent) = path.parent() else {
        return path;
    };
    let Some(parent_name) = parent.file_name().and_then(|name| name.to_str()) else {
        return path;
    };
    let Some(root) = parent.parent() else {
        return path;
    };
    if root.file_name().and_then(|name| name.to_str()) == Some("Codex")
        && root
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            == Some("Documents")
        && is_date_directory(parent_name)
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| !name.is_empty())
    {
        return root.to_path_buf();
    }
    path
}

fn is_date_directory(value: &str) -> bool {
    value.len() == 10
        && value.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

fn normalize_tutti_session_root(path: PathBuf) -> PathBuf {
    let Some(parent) = path.parent() else {
        return path;
    };
    let Some(parent_name) = parent.file_name().and_then(|name| name.to_str()) else {
        return path;
    };
    let Some(root) = parent.parent() else {
        return path;
    };
    if parent_name == "tutti"
        && root.file_name().and_then(|name| name.to_str()) == Some("Documents")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.strip_prefix("session-").is_some_and(is_uuid))
    {
        return parent.to_path_buf();
    }
    path
}

fn is_uuid(value: &str) -> bool {
    let lengths = [8, 4, 4, 4, 12];
    let mut parts = value.split('-');
    lengths.into_iter().all(|length| {
        parts.next().is_some_and(|part| {
            part.len() == length && part.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }) && parts.next().is_none()
}

fn is_tutti_run_root(root: &Path) -> bool {
    root.file_name().and_then(|name| name.to_str()) == Some("runs")
        && root
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some("agent")
}

fn tutti_run_session_roots(root: &Path) -> Vec<PathBuf> {
    let Ok(runs) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut roots = runs
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|run| run.path().join("codex-home/sessions"))
        .filter(|sessions| sessions.is_dir())
        .collect::<Vec<_>>();
    roots.sort();
    roots
}

pub(crate) fn collect_tutti_run_session_paths(root: &Path, session_paths: &mut BTreeSet<PathBuf>) {
    for sessions in tutti_run_session_roots(root) {
        for entry in WalkDir::new(sessions)
            .follow_links(false)
            .max_depth(6)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_type().is_file()
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "jsonl")
            })
        {
            session_paths.insert(entry.into_path());
        }
    }
}

#[derive(Debug, Serialize)]
struct NormalizedHookIdentity {
    event_name: String,
    #[serde(flatten)]
    group: NormalizedMatcherGroup,
}

#[derive(Debug, Serialize)]
struct NormalizedMatcherGroup {
    #[serde(default)]
    matcher: Option<String>,
    #[serde(default)]
    hooks: Vec<NormalizedHookHandler>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum NormalizedHookHandler {
    #[serde(rename = "command")]
    Command {
        command: String,
        #[serde(default, rename = "commandWindows")]
        command_windows: Option<String>,
        #[serde(default, rename = "timeout")]
        timeout_sec: Option<u64>,
        #[serde(default)]
        r#async: bool,
        #[serde(default, rename = "statusMessage")]
        status_message: Option<String>,
        #[serde(
            default,
            rename = "additionalContextLimit",
            skip_serializing_if = "Option::is_none"
        )]
        additional_context_limit: Option<usize>,
    },
}

pub(super) fn matches_name(normalized: &str) -> bool {
    normalized == "codex"
}

fn valid_thread_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn load_hook_review_states(path: &Path) -> HashMap<String, String> {
    let Ok(text) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    let mut states = HashMap::new();
    let mut current_key = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            current_key = line
                .strip_prefix("[hooks.state.\"")
                .and_then(|key| key.strip_suffix("\"]"))
                .map(|key| key.replace("\\\"", "\"").replace("\\\\", "\\"));
            continue;
        }
        let Some(key) = current_key.as_ref() else {
            continue;
        };
        let Some(value) = line
            .strip_prefix("trusted_hash = ")
            .or_else(|| line.strip_prefix("trustedHash = "))
        else {
            continue;
        };
        if let Ok(trusted_hash) = serde_json::from_str::<String>(value) {
            states.insert(key.clone(), trusted_hash);
        }
    }
    states
}

fn hook_event_key_label(event: &str) -> String {
    let mut label = String::with_capacity(event.len() + 4);
    for (index, character) in event.chars().enumerate() {
        if character.is_uppercase() && index > 0 {
            label.push('_');
        }
        label.extend(character.to_lowercase());
    }
    label
}

fn hook_review_key(path: &Path, event: &str, group_index: usize, handler_index: usize) -> String {
    format!(
        "{}:{}:{group_index}:{handler_index}",
        path.display(),
        hook_event_key_label(event),
    )
}

fn hook_timeout(event: &str, configured: Option<u64>) -> u64 {
    if event == "SessionEnd" {
        configured.unwrap_or(1).clamp(1, 3)
    } else {
        configured.unwrap_or(600).max(1)
    }
}

fn hook_matcher(event: &str, matcher: Option<&str>) -> Option<String> {
    match event {
        "UserPromptSubmit" | "Stop" => None,
        _ => matcher.map(str::to_string),
    }
}

fn hook_additional_context_limit(event: &str, value: Option<usize>) -> Option<usize> {
    let supported = matches!(
        event,
        "PreToolUse" | "PostToolUse" | "SessionStart" | "UserPromptSubmit" | "SubagentStart"
    );
    supported
        .then_some(value)
        .flatten()
        .filter(|limit| *limit != 2_500)
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted = serde_json::Map::new();
            let mut keys = map.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            for key in keys {
                if let Some(value) = map.get(&key) {
                    sorted.insert(key, canonical_json(value));
                }
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical_json).collect()),
        other => other.clone(),
    }
}

pub(crate) fn hook_current_hash(
    event: &str,
    matcher: Option<&str>,
    command: &str,
    timeout: u64,
    is_async: bool,
    status_message: Option<&str>,
    additional_context_limit: Option<usize>,
) -> Option<String> {
    let identity = NormalizedHookIdentity {
        event_name: hook_event_key_label(event),
        group: NormalizedMatcherGroup {
            matcher: hook_matcher(event, matcher),
            hooks: vec![NormalizedHookHandler::Command {
                command: command.to_string(),
                command_windows: None,
                timeout_sec: Some(timeout),
                r#async: is_async,
                status_message: status_message.map(str::to_string),
                additional_context_limit: hook_additional_context_limit(
                    event,
                    additional_context_limit,
                ),
            }],
        },
    };
    let value = toml::Value::try_from(identity).ok()?;
    let canonical = canonical_json(&serde_json::to_value(value).ok()?);
    let serialized = serde_json::to_vec(&canonical).ok()?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(serialized);
    Some(format!("sha256:{:x}", hasher.finalize()))
}

pub(crate) fn write_trusted_hash(path: &Path, key: &str, trusted_hash: &str) -> Result<()> {
    let original = fs::read_to_string(path)
        .with_context(|| format!("failed to read Codex config {}", path.display()))?;
    let escaped_key = key.replace('\\', "\\\\").replace('"', "\\\"");
    let header = format!(r#"[hooks.state."{escaped_key}"]"#);
    let mut lines = Vec::new();
    let mut in_target = false;
    let mut found_target = false;
    let mut wrote_hash = false;
    for line in original.lines() {
        if line.trim_start().starts_with('[') && line.trim_end().ends_with(']') {
            if in_target && !wrote_hash {
                lines.push(format!("trusted_hash = {trusted_hash:?}"));
                wrote_hash = true;
            }
            in_target = line.trim() == header;
            found_target |= in_target;
        }
        if in_target && line.trim_start().starts_with("trusted_hash =") {
            lines.push(format!("trusted_hash = {trusted_hash:?}"));
            wrote_hash = true;
        } else if in_target && line.trim_start().starts_with("trustedHash =") {
            lines.push(format!("trusted_hash = {trusted_hash:?}"));
            wrote_hash = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if in_target && !wrote_hash {
        lines.push(format!("trusted_hash = {trusted_hash:?}"));
    }
    if !found_target {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(header);
        lines.push(format!("trusted_hash = {trusted_hash:?}"));
    }
    let mut updated = lines.join("\n");
    if original.ends_with('\n') {
        updated.push('\n');
    }
    crate::fsutil::atomic_write(path, &updated)
        .with_context(|| format!("failed to write Codex config {}", path.display()))
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

fn codex_home_from_system() -> Option<PathBuf> {
    dirs::home_dir().map(|home| {
        codex_home(&ProviderContext {
            home: Some(home),
            project_dirs: Vec::new(),
        })
    })
}

fn codex_skill_policy(path: &Path) -> Result<Option<bool>> {
    let Some(text) = fs::read_to_string(path).ok() else {
        return Ok(None);
    };
    let root = serde_yaml::from_str::<YamlValue>(&text)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(root
        .get("policy")
        .and_then(|policy| policy.get("allow_implicit_invocation"))
        .and_then(YamlValue::as_bool))
}

fn path_lookup_keys(path: &Path) -> Vec<PathBuf> {
    let mut keys = vec![path.to_path_buf()];
    if let Ok(canonical) = path.canonicalize() {
        if !keys.iter().any(|key| key == &canonical) {
            keys.push(canonical);
        }
    }
    keys
}

fn codex_skill_enabled_for_path(config_path: &Path, skill_file: &Path) -> Option<bool> {
    let text = fs::read_to_string(config_path).ok()?;
    let value = toml::from_str::<TomlValue>(&text).ok()?;
    let configs = value
        .get("skills")
        .and_then(|skills| skills.get("config"))
        .and_then(TomlValue::as_array)?;
    configs.iter().find_map(|config| {
        let path = config.get("path").and_then(TomlValue::as_str)?;
        let enabled = config.get("enabled").and_then(TomlValue::as_bool)?;
        path_lookup_keys(Path::new(path))
            .into_iter()
            .any(|key| path_lookup_keys(skill_file).contains(&key))
            .then_some(enabled)
    })
}

fn ensure_yaml_mapping_child<'a>(
    root: &'a mut YamlValue,
    key: &str,
) -> &'a mut serde_yaml::Mapping {
    if !matches!(root, YamlValue::Mapping(_)) {
        *root = YamlValue::Mapping(Default::default());
    }
    let map = root.as_mapping_mut().expect("root was made a mapping");
    let key = YamlValue::String(key.to_string());
    if !matches!(map.get(&key), Some(YamlValue::Mapping(_))) {
        map.insert(key.clone(), YamlValue::Mapping(Default::default()));
    }
    map.get_mut(&key)
        .and_then(YamlValue::as_mapping_mut)
        .expect("child was made a mapping")
}

fn render_codex_policy(before: Option<&str>, visibility: SkillVisibility) -> Result<String> {
    if visibility == SkillVisibility::Mixed {
        bail!("mixed visibility is a scan summary and cannot be written to Codex policy");
    }
    let desired = matches!(visibility, SkillVisibility::Auto);
    let mut root = match before {
        Some(text) => serde_yaml::from_str::<YamlValue>(text)?,
        None => YamlValue::Mapping(Default::default()),
    };
    let current = root
        .get("policy")
        .and_then(|policy| policy.get("allow_implicit_invocation"))
        .and_then(YamlValue::as_bool);
    if current == Some(desired) || (before.is_none() && desired) {
        return Ok(before.unwrap_or_default().to_string());
    }
    let policy = ensure_yaml_mapping_child(&mut root, "policy");
    policy.insert(
        YamlValue::String("allow_implicit_invocation".to_string()),
        YamlValue::Bool(desired),
    );
    Ok(serde_yaml::to_string(&root)?)
}

fn plan_codex_policy(skill_dir: &Path, visibility: SkillVisibility) -> Result<FileChange> {
    let path = skill_dir.join("agents/openai.yaml");
    let before = fs::read_to_string(&path).ok();
    let after = render_codex_policy(before.as_deref(), visibility)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(FileChange {
        path,
        before_sha256: before.as_deref().map(crate::fsutil::sha256_text),
        before,
        after,
    })
}

fn codex_skill_config_matches_path(config: &Table, skill_file: &Path) -> bool {
    let Some(path) = config.get("path").and_then(Item::as_str) else {
        return false;
    };
    let skill_keys = path_lookup_keys(skill_file);
    path_lookup_keys(Path::new(path))
        .into_iter()
        .any(|key| skill_keys.iter().any(|skill_key| skill_key == &key))
}

fn render_codex_skill_config(before: &str, skill_file: &Path, enabled: bool) -> Result<String> {
    let mut doc = if before.trim().is_empty() {
        DocumentMut::new()
    } else {
        before.parse::<DocumentMut>()?
    };
    if !doc.as_table().contains_key("skills") {
        doc["skills"] = Item::Table(Table::new());
    }
    let skills = doc["skills"]
        .as_table_mut()
        .context("skills config root was not a table")?;
    if !skills.contains_key("config") {
        skills["config"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    let configs = skills["config"]
        .as_array_of_tables_mut()
        .context("skills.config was not an array of tables")?;
    if let Some(config) = configs
        .iter_mut()
        .find(|config| codex_skill_config_matches_path(config, skill_file))
    {
        config["enabled"] = value(enabled);
    } else {
        let mut config = Table::new();
        config["path"] = value(skill_file.to_string_lossy().to_string());
        config["enabled"] = value(enabled);
        configs.push(config);
    }
    Ok(doc.to_string())
}

fn plan_codex_skill_config(
    skill_dir: &Path,
    visibility: SkillVisibility,
) -> Result<Option<FileChange>> {
    if visibility == SkillVisibility::Mixed {
        bail!("mixed visibility is a scan summary and cannot be written to Codex config");
    }
    let Some(config_home) = codex_home_from_system() else {
        return Ok(None);
    };
    let config_path = config_home.join("config.toml");
    let desired_enabled = visibility != SkillVisibility::Off;
    let before = fs::read_to_string(&config_path).ok();
    if before.is_none() && desired_enabled {
        return Ok(None);
    }
    let before_text = before.as_deref().unwrap_or("");
    let skill_file = skill_dir.join("SKILL.md");
    let after = render_codex_skill_config(before_text, &skill_file, desired_enabled)
        .with_context(|| format!("failed to update {}", config_path.display()))?;
    if before.as_deref() == Some(after.as_str()) {
        return Ok(None);
    }
    Ok(Some(FileChange {
        path: config_path,
        before_sha256: before.as_deref().map(crate::fsutil::sha256_text),
        before,
        after,
    }))
}

#[cfg(test)]
pub(crate) fn plan_skill_policy_file(
    path: PathBuf,
    visibility: SkillVisibility,
) -> Result<FileChange> {
    let before = fs::read_to_string(&path).ok();
    let after = render_codex_policy(before.as_deref(), visibility)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(FileChange {
        path,
        before_sha256: before.as_deref().map(crate::fsutil::sha256_text),
        before,
        after,
    })
}

#[cfg(test)]
pub(crate) fn skill_policy_path(skill_dir: &Path) -> PathBuf {
    skill_dir.join("agents/openai.yaml")
}

#[cfg(test)]
pub(crate) fn collect_transcript_item(value: &Value, items: &mut Vec<TranscriptItem>) {
    collect_codex_item(value, items);
}

#[cfg(test)]
pub(crate) fn policy_matches_visibility_change(
    baseline: Option<&str>,
    current: Option<&str>,
    visibility: SkillVisibility,
) -> Result<bool> {
    let expected = render_codex_policy(baseline, visibility)?;
    let Some(current) = current else {
        return Ok(expected.is_empty());
    };
    if expected.is_empty() || current.is_empty() {
        return Ok(expected == current);
    }
    Ok(serde_yaml::from_str::<YamlValue>(&expected)?
        == serde_yaml::from_str::<YamlValue>(current)?)
}

#[cfg(test)]
pub(crate) fn plan_skill_config_at(
    config_path: PathBuf,
    skill_file: PathBuf,
    visibility: SkillVisibility,
) -> Result<Option<FileChange>> {
    if visibility == SkillVisibility::Mixed {
        bail!("mixed visibility is a scan summary and cannot be written to Codex config");
    }
    let desired_enabled = visibility != SkillVisibility::Off;
    let before = fs::read_to_string(&config_path).ok();
    if before.is_none() && desired_enabled {
        return Ok(None);
    }
    let before_text = before.as_deref().unwrap_or("");
    let after = render_codex_skill_config(before_text, &skill_file, desired_enabled)
        .with_context(|| format!("failed to update {}", config_path.display()))?;
    if before.as_deref() == Some(after.as_str()) {
        return Ok(None);
    }
    Ok(Some(FileChange {
        path: config_path,
        before_sha256: before.as_deref().map(crate::fsutil::sha256_text),
        before,
        after,
    }))
}

fn codex_usage(value: &Value) -> crate::analytics::AnalyticsTokenUsage {
    crate::analytics::AnalyticsTokenUsage {
        input_tokens: value
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cached_input_tokens: value
            .get("cached_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_write_input_tokens: value
            .get("cache_write_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: value
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        reasoning_output_tokens: value
            .get("reasoning_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        total_tokens: value
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

fn usage_signature(usage: &crate::analytics::AnalyticsTokenUsage) -> String {
    format!(
        "{},{},{},{},{},{}",
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.cache_write_input_tokens,
        usage.output_tokens,
        usage.reasoning_output_tokens,
        usage.total_tokens,
    )
}

pub(crate) fn parse_analytics_line(line: &str, record: &mut SessionAnalyticsRecord) {
    let head = &line.as_bytes()[..line.len().min(1024)];
    const MARKERS: [&[u8]; 11] = [
        b"\"turn_context\"",
        b"\"thread_settings_applied\"",
        b"\"token_count\"",
        b"\"task_started\"",
        b"\"task_complete\"",
        b"\"turn_aborted\"",
        b"\"context_compacted\"",
        b"\"function_call\"",
        b"\"custom_tool_call\"",
        b"\"local_shell_call\"",
        b"\"mcp_tool_call_end\"",
    ];
    if !MARKERS
        .iter()
        .any(|marker| crate::analytics::bytes_contains(head, marker))
    {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        record.analytics.malformed_lines += 1;
        return;
    };
    let timestamp = crate::analytics::string_at(&value, &["timestamp"]);
    if let Some(timestamp) = timestamp
        .as_deref()
        .filter(|timestamp| !timestamp.is_empty())
    {
        record.state.last_timestamp = timestamp.to_string();
    }
    let entry_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    let payload = value.get("payload").unwrap_or(&Value::Null);
    let payload_type = payload.get("type").and_then(Value::as_str).unwrap_or("");

    if entry_type == "turn_context" {
        if let Some(model) = payload.get("model").and_then(Value::as_str) {
            crate::analytics::set_model(record, model);
        }
        return;
    }
    if payload_type == "thread_settings_applied" {
        if let Some(model) = payload
            .pointer("/thread_settings/model")
            .and_then(Value::as_str)
        {
            crate::analytics::set_model(record, model);
        }
        return;
    }
    if matches!(
        payload_type,
        "function_call" | "custom_tool_call" | "local_shell_call"
    ) {
        crate::analytics::record_tool_call(payload, timestamp.as_deref().unwrap_or(""), record);
        return;
    }
    if payload_type == "mcp_tool_call_end" {
        if let (Some(server), Some(tool)) = (
            payload
                .pointer("/invocation/server")
                .and_then(Value::as_str),
            payload.pointer("/invocation/tool").and_then(Value::as_str),
        ) {
            if let Some(call) = record
                .analytics
                .tools
                .iter_mut()
                .rev()
                .find(|call| call.name == tool && call.server.is_empty())
            {
                call.server = server.to_string();
            }
        }
        return;
    }
    if payload_type == "turn_aborted" {
        let stamp = timestamp.unwrap_or_default();
        if !stamp.is_empty() {
            record.analytics.aborts.push(stamp.clone());
        }
        crate::analytics::close_open_run(record, &stamp, false);
        return;
    }
    if payload_type == "context_compacted" {
        if let Some(stamp) = timestamp.filter(|stamp| !stamp.is_empty()) {
            record.analytics.compactions.push(stamp);
        }
        return;
    }
    if payload_type == "task_started" {
        let stamp = timestamp.unwrap_or_default();
        crate::analytics::close_open_run(record, &stamp, false);
        if !stamp.is_empty() {
            record.state.open_run = Some(stamp);
        }
        return;
    }
    if payload_type == "task_complete" {
        crate::analytics::close_open_run(record, timestamp.as_deref().unwrap_or(""), true);
        return;
    }
    if payload_type != "token_count" {
        return;
    }

    crate::analytics::record_rate_limits(payload, timestamp.as_deref().unwrap_or(""), record);
    let Some(raw_usage) = payload.pointer("/info/total_token_usage") else {
        return;
    };
    let current = codex_usage(raw_usage);
    if current.total_tokens == 0 {
        return;
    }
    let last = payload
        .pointer("/info/last_token_usage")
        .map(codex_usage)
        .filter(|usage| usage.total_tokens > 0);
    let usage_key = format!(
        "{}|{}",
        usage_signature(&current),
        last.as_ref().map(usage_signature).unwrap_or_default()
    );
    if record.state.last_usage_key == usage_key {
        return;
    }
    record.state.last_usage_key = usage_key;
    let usage =
        last.unwrap_or_else(|| crate::analytics::diff_usage(record.state.previous_usage, current));
    record.state.previous_usage = current;
    if usage.total_tokens == 0 {
        return;
    }
    record.state.cumulative_usage.add_assign(usage);
    record.state.response_index += 1;
    record
        .analytics
        .responses
        .push(crate::analytics::AnalyticsResponseUsage {
            index: record.state.response_index,
            timestamp: timestamp.unwrap_or_default(),
            model: record.state.current_model.clone(),
            usage,
            cumulative: record.state.cumulative_usage,
        });
}

pub(super) fn resume_target_from_transcript_value(value: &Value) -> Option<&'static str> {
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let source = value
        .get("source")
        .or_else(|| value.pointer("/payload/source"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let originator = value
        .get("originator")
        .or_else(|| value.pointer("/payload/originator"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if source == "vscode"
        || originator == "codex desktop"
        || originator.contains("desktop")
        || originator.contains("work_desktop")
    {
        return Some("app");
    }
    if source == "cli"
        || source == "exec"
        || originator.contains("tui")
        || originator.contains("exec")
    {
        return Some("terminal");
    }
    None
}

fn session_line_has_content(prefix: &str) -> bool {
    match crate::sessions::json_string_field(prefix, "\"type\"") {
        Some("session_meta" | "turn_context" | "world_state") => false,
        Some("response_item") => !matches!(
            crate::sessions::json_string_field(prefix, "\"role\""),
            Some("developer" | "system")
        ),
        Some("event_msg") => [
            "user_message",
            "agent_message",
            "agent_reasoning",
            "sub_agent_activity",
            "context_compacted",
        ]
        .iter()
        .any(|kind| crate::sessions::line_contains_json_string_value(prefix, kind)),
        Some("compacted") => true,
        _ => crate::sessions::line_has_message_role(prefix),
    }
}

fn session_line_requires_metadata_parse(
    prefix: &str,
    meta: &crate::sessions::SessionMetadata,
) -> bool {
    match crate::sessions::json_string_field(prefix, "\"type\"") {
        Some("session_meta" | "turn_context") => true,
        Some("event_msg") => {
            prefix.contains("\"thread_settings_applied\"")
                || prefix.contains("\"token_count\"")
                || crate::sessions::line_has_message_role(prefix)
        }
        Some("response_item") => crate::sessions::line_has_message_role(prefix),
        _ => {
            crate::sessions::line_has_message_role(prefix)
                || (meta.project.is_none() && prefix.contains("\"cwd\""))
                || (meta.repository_url.is_none() && prefix.contains("\"repository_url\""))
        }
    }
}

fn extract_model(value: &Value) -> Option<String> {
    let model = match value.get("type").and_then(Value::as_str) {
        Some("turn_context") => value.pointer("/payload/model"),
        Some("event_msg")
            if value.pointer("/payload/type").and_then(Value::as_str)
                == Some("thread_settings_applied") =>
        {
            value.pointer("/payload/thread_settings/model")
        }
        _ => None,
    }?;
    model
        .as_str()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

pub(crate) fn extract_parent_session_id(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    value
        .pointer("/payload/parent_thread_id")
        .or_else(|| value.pointer("/payload/source/subagent/thread_spawn/parent_thread_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

pub(crate) fn apply_hook_review_states(hooks: &mut [HookRecord], states: &HashMap<String, String>) {
    for hook in hooks
        .iter_mut()
        .filter(|hook| hook.agent == AgentKind::Codex)
    {
        let (Some(key), Some(current_hash)) =
            (&hook.provider_review_key, &hook.provider_current_hash)
        else {
            continue;
        };
        hook.needs_review = states
            .get(key)
            .is_none_or(|trusted_hash| trusted_hash != current_hash);
    }
}

pub(crate) fn extract_provider_title(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let task_name = value
        .pointer("/payload/source/subagent/thread_spawn/task_name")
        .or_else(|| value.pointer("/payload/task_name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|task_name| !task_name.is_empty())
        .map(str::to_string);
    if task_name.is_some() {
        return task_name;
    }
    value
        .pointer("/payload/source/subagent/thread_spawn/agent_path")
        .or_else(|| value.pointer("/payload/agent_path"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|agent_path| !agent_path.is_empty())
        .and_then(|agent_path| agent_path.rsplit('/').find(|part| !part.is_empty()))
        .map(str::to_string)
}

fn extract_token_usage(value: &Value) -> Option<crate::sessions::SessionTokenUsage> {
    if value.pointer("/payload/type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let usage = value.pointer("/payload/info/total_token_usage")?;
    let total_tokens = usage.get("total_tokens")?.as_u64()?;
    if total_tokens == 0 {
        return None;
    }
    Some(crate::sessions::SessionTokenUsage {
        input_tokens: usage.get("input_tokens")?.as_u64()?,
        cached_input_tokens: usage.get("cached_input_tokens")?.as_u64()?,
        output_tokens: usage.get("output_tokens")?.as_u64()?,
        reasoning_output_tokens: usage
            .get("reasoning_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        total_tokens,
    })
}

fn session_metadata(path: &Path) -> Option<crate::sessions::SessionMetadata> {
    path.is_file()
        .then(|| crate::sessions::scan_jsonl_metadata(path, AgentKind::Codex))
}

pub(crate) fn session_title(path: &Path) -> Option<String> {
    if path
        .extension()
        .is_none_or(|extension| extension != "jsonl")
    {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    let inherited_history_start_ordinal =
        crate::transcript::transcript_inherited_history_start_ordinal(path, AgentKind::Codex)
            .ok()
            .flatten();
    let mut provider_title = None;
    for line in std::io::BufRead::lines(std::io::BufReader::new(file)).map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if provider_title.is_none() {
            provider_title = extract_provider_title(&value);
        }
        if crate::transcript::is_inherited_transcript_value(&value, inherited_history_start_ordinal)
        {
            continue;
        }
        if let Some(title) = crate::sessions::extract_session_title(&value) {
            return provider_title.or(Some(title));
        }
    }
    provider_title
}

pub(super) fn codex_thread_writer_lock_path(
    session_path: &Path,
    session_id: &str,
) -> Option<PathBuf> {
    if session_id.is_empty() {
        return None;
    }
    session_path.ancestors().find_map(|ancestor| {
        let name = ancestor.file_name()?.to_str()?;
        if !matches!(name, "sessions" | "archived_sessions") {
            return None;
        }
        Some(
            ancestor
                .parent()?
                .join("thread-writer-locks")
                .join(format!("{session_id}.lock")),
        )
    })
}

pub(super) fn active_session_writer(session: &SessionRecord) -> Result<Option<SessionWriter>> {
    let Some(lock_path) = codex_thread_writer_lock_path(&session.path, &session.id) else {
        return Ok(None);
    };
    let lock_file = match OpenOptions::new().read(true).write(true).open(&lock_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to inspect Codex writer lock {}",
                    lock_path.display()
                )
            });
        }
    };

    match lock_file.try_lock() {
        Ok(()) => {
            lock_file.unlock().with_context(|| {
                format!(
                    "failed to release Codex writer lock {}",
                    lock_path.display()
                )
            })?;
            Ok(None)
        }
        Err(TryLockError::WouldBlock) => {
            Ok(Some(SessionWriter { lock_path }))
        }
        Err(TryLockError::Error(error)) => Err(error).with_context(|| {
            format!(
                "failed to inspect Codex writer lock {}",
                lock_path.display()
            )
        }),
    }
}

pub(super) fn validate_session_writer(session: &SessionRecord) -> Result<()> {
    if let Some(writer) = active_session_writer(session)? {
        bail!(
            "Codex session {} already has an active writer ({})",
            session.id,
            writer.lock_path.display()
        );
    }
    Ok(())
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
    let Some(name) = payload
        .get("name")
        .or_else(|| payload.pointer("/action/type"))
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
    else {
        return Vec::new();
    };
    vec![(
        payload,
        Evidence {
            kind: name.to_string(),
            text: crate::session_skills::summarize_evidence(payload),
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
            collect_message_content_with_markers(
                content,
                items,
                role,
                time,
                &CODEX_INTERNAL_CONTEXT_MARKERS,
            );
        }
        Some(kind @ ("reasoning" | "thinking")) => {
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
                .filter(|name| !name.trim().is_empty())
                .map(str::to_string);
            let command = extract_codex_tool_command(payload);
            let summary = command
                .as_deref()
                .map(|command| command.chars().take(220).collect())
                .unwrap_or_else(|| summarize_tool_call(payload));
            push_tool_item(
                items,
                "tool",
                summary,
                name,
                time,
                command,
                None,
                extract_duration_ms(payload, None),
                extract_call_id(payload),
                timestamp_ms,
            );
        }
        Some("function_call_output") | Some("custom_tool_call_output") => {
            let result = extract_tool_result(payload);
            let call_id = extract_call_id(payload);
            let duration_ms = extract_duration_ms(payload, result.as_deref());
            attach_tool_result(items, call_id.as_deref(), result, duration_ms, timestamp_ms);
        }
        Some(kind @ ("web_search_call" | "image_generation_call")) => {
            push_item(items, "tool", String::new(), Some(kind.to_string()), time);
        }
        _ => {}
    }
}

fn extract_codex_tool_command(payload: &Value) -> Option<String> {
    if let Some(command) = extract_tool_command(payload) {
        return Some(command);
    }
    payload
        .get("input")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(|command| command.chars().take(4_000).collect())
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

    fn project_skill_root(&self, cwd: &Path) -> Option<PathBuf> {
        Some(cwd.join(".codex/skills"))
    }

    fn skill_target(&self) -> Option<ProviderSkillTarget> {
        Some(ProviderSkillTarget {
            id: "codex",
            display_name: "Codex",
        })
    }

    fn bundled_skill_files(&self) -> &'static [(&'static str, &'static str)] {
        &CODEX_BUNDLED_SKILL_FILES
    }

    fn config_profile_path(&self, _home: &Path, codex_home: &Path, name: &str) -> Option<PathBuf> {
        Some(codex_home.join(format!("{name}.config.toml")))
    }

    fn config_profile_format(&self) -> Option<&'static str> {
        Some("toml")
    }

    fn config_files(&self, home: &Path, codex_home: &Path) -> Vec<crate::config::AgentConfigFile> {
        let base_path = codex_home.join("config.toml");
        let mut configs = vec![
            self.config_file_for_path(home, codex_home, &base_path)
                .expect("Codex provider must resolve its base config path"),
        ];
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

    fn uses_shared_skill_layout(&self) -> bool {
        true
    }

    fn matches_name(&self, normalized: &str) -> bool {
        matches_name(normalized)
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("Codex")
    }

    fn app_bundle_path(&self) -> Option<&'static str> {
        Some("/Applications/Codex.app")
    }

    fn executable_names(&self) -> &'static [&'static str] {
        &["codex"]
    }

    fn config_dir(&self, ctx: &ProviderContext) -> Option<PathBuf> {
        Some(codex_home(ctx))
    }

    fn config_home(&self, home: &Path) -> PathBuf {
        codex_home(&ProviderContext {
            home: Some(home.to_path_buf()),
            project_dirs: Vec::new(),
        })
    }

    #[cfg(test)]
    fn config_home_for_test(&self, _home: &Path, override_home: &Path) -> PathBuf {
        override_home.to_path_buf()
    }

    fn projection_directories(&self) -> &'static [&'static str] {
        &[".codex"]
    }

    fn projection_candidate_files(&self, domain: &str, ancestor: &Path) -> Vec<PathBuf> {
        match domain {
            "rules" => vec![ancestor.join("AGENTS.md")],
            "hooks" => vec![
                ancestor.join(".codex/hooks.json"),
                ancestor.join(".codex/config.toml"),
            ],
            "mcp" => vec![
                ancestor.join(".codex/mcp.json"),
                ancestor.join(".codex/config.toml"),
            ],
            "skills" => vec![ancestor.join(".codex/skills")],
            _ => Vec::new(),
        }
    }

    fn skill_visibility_metadata(
        &self,
        skill_dir: &Path,
        skill_file: &Path,
        _frontmatter: Option<&serde_yaml::Value>,
    ) -> Result<SkillProviderMetadata> {
        let allow_implicit_invocation = codex_skill_policy(&skill_dir.join("agents/openai.yaml"))?;
        let enabled = codex_home_from_system()
            .map(|home| codex_skill_enabled_for_path(&home.join("config.toml"), skill_file))
            .flatten();
        let provider_visibility = if enabled == Some(false) {
            SkillVisibility::Off
        } else if allow_implicit_invocation == Some(false) {
            SkillVisibility::Manual
        } else {
            SkillVisibility::Auto
        };
        Ok(SkillProviderMetadata {
            allow_implicit_invocation,
            enabled,
            disable_model_invocation: None,
            provider_visibility,
        })
    }

    fn effective_skill_visibility(
        &self,
        tendi_visibility: Option<SkillVisibility>,
        provider_visibility: SkillVisibility,
        root: &SkillRoot,
    ) -> SkillVisibility {
        if root.plugin_enabled == Some(false) {
            SkillVisibility::Off
        } else {
            crate::skills::combine_skill_visibility(tendi_visibility, provider_visibility)
        }
    }

    fn skill_backup_exclusion_reason(&self, path: &SkillPath) -> Option<&'static str> {
        path.plugin_id.is_some().then_some("plugin-skill")
    }

    fn plan_skill_visibility(
        &self,
        skill_dir: &Path,
        visibility: SkillVisibility,
        update_provider_config: bool,
    ) -> Result<Vec<FileChange>> {
        let mut changes = vec![plan_codex_policy(skill_dir, visibility)?];
        if update_provider_config {
            if let Some(change) = plan_codex_skill_config(skill_dir, visibility)? {
                changes.push(change);
            }
        }
        Ok(changes)
    }

    fn is_managed_skill_file(&self, relative_path: &str) -> bool {
        relative_path == "agents/openai.yaml" || relative_path.ends_with("/agents/openai.yaml")
    }

    fn session_scan_priority(&self, root: &Path) -> Option<u8> {
        let path = root.to_string_lossy();
        if path.contains("/.codex/sessions") {
            Some(0)
        } else if path.contains("/.codex/archived_sessions")
            || path.ends_with("/.codex/session_index.jsonl")
        {
            Some(4)
        } else {
            None
        }
    }

    fn session_path_role(&self, path: &Path) -> SessionPathRole {
        if path.file_name().and_then(|name| name.to_str()) == Some("session_index.jsonl") {
            SessionPathRole::Index
        } else if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            SessionPathRole::Transcript
        } else {
            SessionPathRole::Other
        }
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
        scan_session_index(&root.join("session_index.jsonl"), sessions_out, warnings)?;
        sessions::scan_jsonl_sessions(&root.join("sessions"), self.kind(), 6, sessions_out, cache);
        sessions::scan_jsonl_sessions(
            &root.join("archived_sessions"),
            self.kind(),
            6,
            sessions_out,
            cache,
        );
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

    fn session_watch_targets(
        &self,
        root: &Path,
    ) -> Option<(Vec<crate::sessions::SessionWatchTarget>, bool)> {
        if !is_tutti_run_root(root) {
            return None;
        }
        let mut targets = vec![crate::sessions::SessionWatchTarget {
            path: root.to_path_buf(),
            recursive: false,
        }];
        targets.extend(tutti_run_session_roots(root).into_iter().map(|path| {
            crate::sessions::SessionWatchTarget {
                path,
                recursive: true,
            }
        }));
        Some((targets, true))
    }

    fn session_watch_expansion(
        &self,
        dynamic_roots: &[PathBuf],
        event_path: &Path,
    ) -> Option<crate::sessions::SessionWatchExpansion> {
        let root = dynamic_roots
            .iter()
            .find(|root| event_path.starts_with(root))?;
        let run_name = event_path.strip_prefix(root).ok()?.components().next()?;
        let run_dir = root.join(run_name.as_os_str());
        Some(crate::sessions::SessionWatchExpansion {
            agent_home: run_dir.join("codex-home"),
            session_root: run_dir.join("codex-home/sessions"),
            run_dir,
        })
    }

    fn collect_additional_session_paths(
        &self,
        root: &Path,
        session_paths: &mut BTreeSet<PathBuf>,
    ) -> bool {
        if !is_tutti_run_root(root) {
            return false;
        }
        collect_tutti_run_session_paths(root, session_paths);
        true
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

    fn validate_session_resume(&self, session: &SessionRecord) -> Result<()> {
        validate_session_writer(session)
    }

    fn active_session_writer(&self, session: &SessionRecord) -> Result<Option<SessionWriter>> {
        active_session_writer(session)
    }

    fn session_requires_rescan(&self, session: &SessionRecord) -> Option<bool> {
        if session.parent_session_id.is_none() {
            return None;
        }
        if let Some(meta) = session_metadata(&session.path) {
            return Some(
                meta.title != session.title
                    || meta.first_user_message != session.first_user_message
                    || meta.last_user_message != session.last_user_message
                    || meta.last_assistant_message != session.last_assistant_message
                    || meta.parent_session_id != session.parent_session_id,
            );
        }
        session_title(&session.path).map(|title| Some(title) != session.title)
    }

    fn session_line_has_content(&self, prefix: &str) -> Option<bool> {
        Some(session_line_has_content(prefix))
    }

    fn session_line_requires_metadata_parse(
        &self,
        prefix: &str,
        meta: &crate::sessions::SessionMetadata,
    ) -> Option<bool> {
        Some(session_line_requires_metadata_parse(prefix, meta))
    }

    fn update_session_metadata(
        &self,
        value: &Value,
        meta: &mut crate::sessions::SessionMetadata,
        _deduplicated_usage: &mut BTreeMap<String, crate::sessions::SessionTokenUsage>,
    ) {
        if meta.provider_title.is_none() {
            meta.provider_title = extract_provider_title(value);
        }
        if meta.parent_session_id.is_none() {
            meta.parent_session_id = extract_parent_session_id(value);
        }
        if let Some(model) = extract_model(value) {
            meta.model = Some(model);
        }
        if let Some(token_usage) = extract_token_usage(value) {
            meta.token_usage = Some(token_usage);
        }
    }

    fn resume_target_from_transcript_value(&self, value: &Value) -> Option<&'static str> {
        resume_target_from_transcript_value(value)
    }

    fn accepts_session_app_url(&self, url: &str) -> bool {
        url.strip_prefix("codex://threads/")
            .is_some_and(valid_thread_id)
    }

    fn parse_transcript_value(&self, value: &Value, items: &mut Vec<TranscriptItem>) {
        parse_transcript(value, items);
    }

    fn transcript_internal_context_markers(
        &self,
    ) -> &'static [crate::transcript::InternalContextMarker] {
        &CODEX_INTERNAL_CONTEXT_MARKERS
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
        parse_analytics_line(line, record);
    }

    fn extract_skill_tool_payloads<'a>(&self, value: &'a Value) -> Vec<(&'a Value, Evidence)> {
        tool_payloads(value)
    }

    fn infer_session_project(&self, _path: &Path, project: Option<PathBuf>) -> Option<PathBuf> {
        project
    }

    fn normalize_session_project(&self, project: PathBuf) -> PathBuf {
        normalize_ephemeral_chat_root(normalize_tutti_session_root(project))
    }

    fn session_id_from_path(&self, path: &Path) -> Option<String> {
        session_id_from_path(path)
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
                "mcp_servers",
                infer_mcp_toml_transport,
                infer_mcp_toml_enabled,
                infer_mcp_toml_status,
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
                &["mcpServers"],
                infer_mcp_transport,
                infer_mcp_enabled,
                infer_mcp_status,
                servers,
                warnings,
            );
            crate::mcp::scan_toml_mcp(
                &ancestor.join(".codex/config.toml"),
                self.kind(),
                &scope,
                "mcp_servers",
                infer_mcp_toml_transport,
                infer_mcp_toml_enabled,
                infer_mcp_toml_status,
                servers,
                warnings,
            );
        }
        Ok(())
    }

    fn set_mcp_enabled(&self, request: &McpSetEnabledRequest) -> Result<()> {
        match request.path.extension().and_then(|value| value.to_str()) {
            Some("toml") => crate::mcp::set_toml_server_enabled(
                request,
                "mcp_servers",
                update_mcp_toml_server,
            ),
            Some("json") => crate::mcp::set_json_server_enabled(
                request,
                &["mcpServers"],
                update_mcp_json_server,
            ),
            _ => bail!("Codex MCP source must be JSON or TOML"),
        }
    }

    fn mcp_status_after_toggle(&self, enabled: bool) -> &'static str {
        if enabled { "configured" } else { "disabled" }
    }

    fn delete_hooks(
        &self,
        requests: &[HookDeleteRequest],
        source: &str,
    ) -> Result<String> {
        match requests[0].path.extension().and_then(|value| value.to_str()) {
            Some("json") => crate::hooks::delete_json_hooks(requests, source),
            Some("toml") => crate::hooks::delete_toml_hooks(requests, source),
            _ => bail!("Codex hook source must be JSON or TOML"),
        }
    }

    fn set_hook_enabled(
        &self,
        request: &HookSetEnabledRequest,
        source: &str,
    ) -> Result<String> {
        match request.path.extension().and_then(|value| value.to_str()) {
            Some("json") => crate::hooks::set_json_hook_enabled(request, source),
            Some("toml") => crate::hooks::set_toml_hook_enabled(request, source),
            _ => bail!("Codex hook source must be JSON or TOML"),
        }
    }

    fn uses_tendi_hook_review_state(&self) -> bool {
        false
    }

    fn is_global_hook_path(&self, path: &Path) -> bool {
        codex_home_from_system().is_some_and(|root| path.starts_with(root))
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

    fn scan_hook_source_for_review(
        &self,
        path: &Path,
        hooks: &mut Vec<HookRecord>,
        warnings: &mut Vec<String>,
    ) -> bool {
        if path.file_name().and_then(|name| name.to_str()) != Some("config.toml") {
            return false;
        }
        scan_codex_config_hooks(path, hooks, warnings);
        true
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

    fn hook_review_metadata(
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
        let key = hook_review_key(path, event, group_index, handler_index);
        let hash = command.and_then(|command| {
            hook_current_hash(
                event,
                matcher,
                command,
                hook_timeout(event, configured_timeout),
                is_async,
                status_message,
                additional_context_limit,
            )
        });
        (Some(key), hash)
    }

    fn apply_hook_review_states(&self, hooks: &mut [HookRecord], ctx: &ProviderContext) {
        let Some(home) = self.config_dir(ctx) else {
            return;
        };
        let states = load_hook_review_states(&home.join("config.toml"));
        apply_hook_review_states(hooks, &states);
    }

    fn review_hook(&self, hook: &HookRecord) -> Result<()> {
        let key = hook
            .provider_review_key
            .as_deref()
            .context("this hook does not support review")?;
        let current_hash = hook
            .provider_current_hash
            .as_deref()
            .filter(|hash| *hash != "unsupported")
            .context("this hook type does not support review")?;
        let home = dirs::home_dir().context("home directory is unavailable")?;
        write_trusted_hash(
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
