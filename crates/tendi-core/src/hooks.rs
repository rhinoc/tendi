use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex, MutexGuard},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use toml::Value as TomlValue;

use crate::{
    fsutil::{atomic_write, sha256_file, sha256_text},
    skills::AgentKind,
};

static HOOK_MUTATION_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn lock_hook_mutation() -> Result<MutexGuard<'static, ()>> {
    HOOK_MUTATION_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("hook mutation authority is unavailable"))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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
    pub needs_review: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_reason: Option<String>,
    #[serde(skip)]
    pub(crate) provider_review_key: Option<String>,
    #[serde(skip)]
    pub(crate) provider_current_hash: Option<String>,
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
    pub agent: AgentKind,
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
    pub agent: AgentKind,
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

#[derive(Debug, Clone, Deserialize)]
pub struct HookReviewRequest {
    pub agent: AgentKind,
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

fn tendi_hook_review_state_path() -> Option<PathBuf> {
    dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Library/Application Support")))
        .map(|base| base.join("tendi/hook-reviews.json"))
}

fn load_tendi_hook_review_states() -> HashMap<String, String> {
    let Some(path) = tendi_hook_review_state_path() else {
        return HashMap::new();
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn hook_review_identity(hook: &HookRecord) -> String {
    serde_json::to_string(&(
        hook.agent,
        hook.path.to_string_lossy(),
        &hook.event,
        &hook.matcher,
        &hook.hook_type,
        &hook.command,
        &hook.url,
        &hook.prompt,
        &hook.filter,
        &hook.status_message,
    ))
    .unwrap_or_default()
}

fn hook_source_is_managed(hook: &HookRecord) -> bool {
    crate::providers::agent_provider(hook.agent).managed_hook_path(&hook.path)
}

fn apply_tendi_hook_review_states(hooks: &mut [HookRecord], states: &HashMap<String, String>) {
    for hook in hooks {
        if !crate::providers::agent_provider(hook.agent).uses_tendi_hook_review_state()
            || hook_source_is_managed(hook)
        {
            continue;
        }
        hook.needs_review = states
            .get(&hook_review_identity(hook))
            .is_none_or(|trusted_hash| trusted_hash != &hook.trust_hash);
    }
}

pub fn scan_hooks(cwd: &Path) -> Result<HookScan> {
    let mut hooks = Vec::new();
    let mut warnings = Vec::new();
    let mut scanned_files = HashSet::new();
    let context = crate::providers::ProviderContext::new(cwd);
    let tendi_hook_review_states = load_tendi_hook_review_states();

    for provider in crate::providers::agent_providers() {
        provider.scan_hooks(&context, &mut scanned_files, &mut hooks, &mut warnings);
        provider.apply_hook_review_states(&mut hooks, &context);
    }

    apply_tendi_hook_review_states(&mut hooks, &tendi_hook_review_states);

    Ok(HookScan { hooks, warnings })
}

pub fn delete_hook(request: HookDeleteRequest) -> Result<()> {
    delete_hooks(vec![request])
}

pub fn delete_hooks(requests: Vec<HookDeleteRequest>) -> Result<()> {
    let _mutation = lock_hook_mutation()?;
    let mut requests_by_path = HashMap::<PathBuf, Vec<HookDeleteRequest>>::new();
    for request in requests {
        requests_by_path
            .entry(request.path.clone())
            .or_default()
            .push(request);
    }

    let mut writes = Vec::new();
    for (path, requests) in requests_by_path {
        let agent = requests[0].agent;
        if requests.iter().any(|request| request.agent != agent) {
            bail!("hook delete requests disagree about the provider");
        }
        ensure_deletable_hook_path(agent, &path)?;
        let expected_hash = &requests[0].expected_trust_hash;
        if requests
            .iter()
            .any(|request| request.expected_trust_hash != *expected_hash)
        {
            bail!("hook delete requests disagree about the source hash");
        }
        let text = fs::read_to_string(&path)?;
        if sha256_text(&text) != *expected_hash {
            bail!("hook source changed");
        }

        let after = crate::providers::agent_provider(agent).delete_hooks(&requests, &text)?;
        writes.push((path, text, after));
    }

    let mut committed = Vec::new();
    for (path, before, after) in &writes {
        let current = fs::read_to_string(path)?;
        if current != *before {
            rollback_hook_writes(&committed)?;
            bail!("hook source changed");
        }
        if let Err(error) = atomic_write(path, after) {
            rollback_hook_writes(&committed)?;
            return Err(error);
        }
        committed.push((path.clone(), before.clone()));
    }
    Ok(())
}

fn rollback_hook_writes(committed: &[(PathBuf, String)]) -> Result<()> {
    for (path, before) in committed.iter().rev() {
        atomic_write(path, before)
            .with_context(|| format!("failed to roll back hook source {}", path.display()))?;
    }
    Ok(())
}

pub fn set_hook_enabled(request: HookSetEnabledRequest) -> Result<()> {
    let _mutation = lock_hook_mutation()?;
    ensure_deletable_hook_path(request.agent, &request.path)?;
    let text = fs::read_to_string(&request.path)?;
    if sha256_text(&text) != request.expected_trust_hash {
        bail!("hook source changed");
    }

    let after =
        crate::providers::agent_provider(request.agent).set_hook_enabled(&request, &text)?;
    atomic_write(&request.path, &after)
}

fn hook_matches_delete_request(hook: &HookRecord, request: &HookDeleteRequest) -> bool {
    hook.path == request.path
        && hook.trust_hash == request.expected_trust_hash
        && hook.event == request.event
        && hook.matcher == request.matcher
        && hook.hook_type == request.hook_type
        && hook.command == request.command
        && hook.url == request.url
        && hook.prompt == request.prompt
        && hook.filter == request.filter
        && hook.status_message == request.status_message
}

fn hook_matches_set_enabled_request(hook: &HookRecord, request: &HookSetEnabledRequest) -> bool {
    hook.path == request.path
        && hook.trust_hash == request.expected_trust_hash
        && hook.event == request.event
        && hook.matcher == request.matcher
        && hook.hook_type == request.hook_type
        && hook.command == request.command
        && hook.url == request.url
        && hook.prompt == request.prompt
        && hook.filter == request.filter
        && hook.status_message == request.status_message
}

fn hook_matches_review_request(hook: &HookRecord, request: &HookReviewRequest) -> bool {
    hook.agent == request.agent
        && crate::providers::agent_provider(request.agent).discoverable()
        && hook.path == request.path
        && hook.trust_hash == request.expected_trust_hash
        && hook.event == request.event
        && hook.matcher == request.matcher
        && hook.hook_type == request.hook_type
        && hook.command == request.command
        && hook.url == request.url
        && hook.prompt == request.prompt
        && hook.filter == request.filter
        && hook.status_message == request.status_message
}

fn scan_hook_source_for_review(path: &Path, agent: AgentKind) -> HookScan {
    let mut hooks = Vec::new();
    let mut warnings = Vec::new();
    if !crate::providers::agent_provider(agent).scan_hook_source_for_review(
        path,
        &mut hooks,
        &mut warnings,
    ) {
        scan_hook_file(path, agent, &mut hooks, &mut warnings);
    }
    HookScan { hooks, warnings }
}

fn refresh_hook_states(cwd: &Path, hooks: &mut [HookRecord]) {
    let context = crate::providers::ProviderContext::new(cwd);
    for provider in crate::providers::agent_providers() {
        provider.apply_hook_review_states(hooks, &context);
    }
    let states = load_tendi_hook_review_states();
    apply_tendi_hook_review_states(hooks, &states);
}

fn refresh_hook_hashes(cwd: &Path, hooks: &mut [HookRecord], paths: &[PathBuf]) -> Result<()> {
    for hook in hooks.iter_mut().filter(|hook| paths.contains(&hook.path)) {
        hook.trust_hash = sha256_file(&hook.path)?;
    }
    refresh_hook_states(cwd, hooks);
    Ok(())
}

pub fn refresh_hook_scan_after_delete(
    cwd: &Path,
    mut scan: HookScan,
    requests: &[HookDeleteRequest],
) -> Result<HookScan> {
    let paths = requests
        .iter()
        .map(|request| request.path.clone())
        .collect::<HashSet<_>>();
    scan.hooks.retain(|hook| {
        !requests
            .iter()
            .any(|request| hook_matches_delete_request(hook, request))
    });
    refresh_hook_hashes(cwd, &mut scan.hooks, &paths.into_iter().collect::<Vec<_>>())?;
    Ok(scan)
}

pub fn refresh_hook_scan_after_set_enabled(
    cwd: &Path,
    mut scan: HookScan,
    request: &HookSetEnabledRequest,
) -> Result<HookScan> {
    let mut matched = false;
    for hook in &mut scan.hooks {
        if hook_matches_set_enabled_request(hook, request) {
            hook.enabled = request.enabled;
            matched = true;
        }
    }
    if !matched {
        bail!("matching hook was not found");
    }
    refresh_hook_hashes(cwd, &mut scan.hooks, std::slice::from_ref(&request.path))?;
    Ok(scan)
}

/// Review one hook against an already-loaded projection.
///
/// The target source is re-read so the expected hash and provider-specific
/// review metadata are current; other hook sources are not traversed.
pub fn review_hook_from_scan(scan: HookScan, request: HookReviewRequest) -> Result<HookScan> {
    let _mutation = lock_hook_mutation()?;
    review_hook_from_scan_inner(scan, request)
}

fn review_hook_from_scan_inner(mut scan: HookScan, request: HookReviewRequest) -> Result<HookScan> {
    let hook_index = scan
        .hooks
        .iter()
        .position(|hook| hook_matches_review_request(hook, &request))
        .context("matching hook was not found")?;
    let source_scan = scan_hook_source_for_review(&request.path, request.agent);
    let hook = source_scan
        .hooks
        .iter()
        .find(|hook| hook_matches_review_request(hook, &request))
        .context("matching hook source was not found")?;
    if hook_source_is_managed(hook) {
        bail!("managed hook sources cannot be reviewed here");
    }
    crate::providers::agent_provider(request.agent).review_hook(hook)?;
    scan.hooks[hook_index].needs_review = false;
    Ok(scan)
}

pub(crate) fn review_hook_with_tendi_state(hook: &HookRecord) -> Result<()> {
    let path = tendi_hook_review_state_path().context("Tendi data directory is unavailable")?;
    let mut states = load_tendi_hook_review_states();
    states.insert(hook_review_identity(hook), hook.trust_hash.clone());
    atomic_write(
        &path,
        &format!("{}\n", serde_json::to_string_pretty(&states)?),
    )
}

pub fn read_hook_source(
    cwd: &Path,
    path: &Path,
    expected_trust_hash: Option<&str>,
    hook_match: Option<&HookSourceMatch>,
) -> Result<HookSourceContent> {
    let scan = scan_hooks(cwd)?;
    let Some(hook) = scan.hooks.iter().find(|hook| hook.path == path) else {
        bail!("refusing to read unknown hook source {}", path.display());
    };

    read_hook_source_at_path(path, hook.agent, expected_trust_hash, hook_match)
}

pub fn read_hook_source_at_path(
    path: &Path,
    agent: AgentKind,
    expected_trust_hash: Option<&str>,
    hook_match: Option<&HookSourceMatch>,
) -> Result<HookSourceContent> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let sha256 = sha256_file(path)?;
    if expected_trust_hash.is_some_and(|expected| !expected.is_empty() && expected != sha256) {
        bail!("hook source changed");
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
    let read_only_reason = crate::providers::agent_provider(agent)
        .hook_read_only_reason(path)
        .map(str::to_string);

    Ok(HookSourceContent {
        path: path.to_path_buf(),
        content,
        sha256,
        source_type: path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
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

fn ensure_deletable_hook_path(agent: AgentKind, path: &Path) -> Result<()> {
    if let Some(reason) = crate::providers::agent_provider(agent).hook_read_only_reason(path) {
        bail!("{reason}");
    }
    Ok(())
}

fn canonical_scan_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn yaml_frontmatter(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

pub(crate) fn scan_hook_file_once(
    path: &Path,
    agent: AgentKind,
    scanned_files: &mut HashSet<PathBuf>,
    hooks: &mut Vec<HookRecord>,
    warnings: &mut Vec<String>,
) {
    scan_file_once(
        path,
        scanned_files,
        hooks,
        warnings,
        |path, hooks, warnings| scan_hook_file(path, agent, hooks, warnings),
    );
}

pub(crate) fn scan_file_once(
    path: &Path,
    scanned_files: &mut HashSet<PathBuf>,
    hooks: &mut Vec<HookRecord>,
    warnings: &mut Vec<String>,
    scanner: impl FnOnce(&Path, &mut Vec<HookRecord>, &mut Vec<String>),
) {
    if !path.is_file() {
        return;
    }
    if !scanned_files.insert(canonical_scan_key(path)) {
        return;
    }
    scanner(path, hooks, warnings);
}

pub(crate) fn scan_hook_file(
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
    if crate::providers::agent_provider(agent).parse_hook_file(path, &trust_hash, hooks, warnings) {
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

pub(crate) fn delete_json_hooks(requests: &[HookDeleteRequest], source: &str) -> Result<String> {
    let mut value = serde_json::from_str::<Value>(source)?;
    for request in requests {
        if !remove_json_hook_from_value(&mut value, request) {
            bail!("matching hook was not found");
        }
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

pub(crate) fn delete_toml_hooks(requests: &[HookDeleteRequest], source: &str) -> Result<String> {
    let mut value = toml::from_str::<TomlValue>(source)?;
    for request in requests {
        if !remove_toml_hook_from_value(&mut value, request) {
            bail!("matching hook was not found");
        }
    }
    Ok(toml::to_string_pretty(&value)?)
}

pub(crate) fn set_json_hook_enabled(
    request: &HookSetEnabledRequest,
    source: &str,
) -> Result<String> {
    let mut value = serde_json::from_str::<Value>(source)?;
    if !set_json_hook_enabled_in_value(&mut value, request) {
        bail!("matching hook was not found");
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

pub(crate) fn set_toml_hook_enabled(
    request: &HookSetEnabledRequest,
    source: &str,
) -> Result<String> {
    let mut value = toml::from_str::<TomlValue>(source)?;
    if !set_toml_hook_enabled_in_value(&mut value, request) {
        bail!("matching hook was not found");
    }
    Ok(toml::to_string_pretty(&value)?)
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

pub(crate) fn collect_hooks_from_value(
    agent: AgentKind,
    path: &Path,
    trust_hash: &str,
    value: &Value,
    hooks: &mut Vec<HookRecord>,
) {
    let Some(hook_map) = value.get("hooks").and_then(Value::as_object) else {
        return;
    };

    let start = hooks.len();
    for (event, specs) in hook_map {
        collect_event_hooks(agent, path, trust_hash, event, 0, specs, hooks);
    }
    if crate::providers::agent_provider(agent).disables_hooks_from_config(value) {
        for hook in &mut hooks[start..] {
            hook.enabled = false;
        }
    }
}

pub(crate) fn collect_event_hooks(
    agent: AgentKind,
    path: &Path,
    trust_hash: &str,
    event: &str,
    group_index: usize,
    specs: &Value,
    hooks: &mut Vec<HookRecord>,
) {
    if let Some(array) = specs.as_array() {
        for (group_index, item) in array.iter().enumerate() {
            collect_event_hooks(agent, path, trust_hash, event, group_index, item, hooks);
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
            group_index,
            0,
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
        group_index,
        0,
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
    group_index: usize,
    handler_index: usize,
    matcher: Option<&str>,
    group_enabled: bool,
    specs: &Value,
    hooks: &mut Vec<HookRecord>,
) {
    if let Some(array) = specs.as_array() {
        for (handler_index, item) in array.iter().enumerate() {
            collect_group_hooks(
                agent,
                path,
                trust_hash,
                event,
                group_index,
                handler_index,
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
            group_index,
            handler_index,
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
        group_index,
        handler_index,
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
    group_index: usize,
    handler_index: usize,
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

    let status_message = object
        .get("statusMessage")
        .or_else(|| object.get("status_message"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let (provider_review_key, provider_current_hash) = crate::providers::agent_provider(agent)
        .hook_review_metadata(
            path,
            event,
            group_index,
            handler_index,
            matcher,
            command.as_deref(),
            object.get("timeout").and_then(Value::as_u64),
            object
                .get("async")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            status_message.as_deref(),
            object
                .get("additionalContextLimit")
                .or_else(|| object.get("additional_context_limit"))
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
        );

    hooks.push(HookRecord {
        agent,
        event: event.to_string(),
        matcher: matcher.map(str::to_string),
        hook_type,
        command,
        url,
        prompt,
        filter: object.get("if").and_then(Value::as_str).map(str::to_string),
        status_message,
        enabled,
        path: path.to_path_buf(),
        trust_hash: trust_hash.to_string(),
        needs_review: false,
        read_only_reason: crate::providers::agent_provider(agent)
            .hook_read_only_reason(path)
            .map(str::to_string),
        provider_review_key,
        provider_current_hash,
    });
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        HookReviewRequest, HookScan, apply_tendi_hook_review_states, hook_review_identity,
        scan_hook_file,
    };
    use crate::{
        providers::{claude::scan_claude_component_file, codex::scan_codex_config_hooks},
        skills::AgentKind,
    };


    #[test]
    fn marks_codex_untrusted_and_modified_hooks_for_review() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-review-{}-{}",
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
      { "hooks": [{ "type": "command", "command": "/bin/echo trusted" }] },
      { "hooks": [{ "type": "command", "command": "/bin/echo modified" }] }
    ]
  }
}"#,
        )
        .expect("write hooks");

        let mut hooks = Vec::new();
        let mut warnings = Vec::new();
        super::scan_hook_file(&path, AgentKind::Codex, &mut hooks, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        let trusted_key = hooks[0].provider_review_key.clone().expect("trusted key");
        let trusted_hash = hooks[0]
            .provider_current_hash
            .clone()
            .expect("trusted hash");
        let modified_key = hooks[1].provider_review_key.clone().expect("modified key");
        crate::providers::codex::apply_hook_review_states(
            &mut hooks,
            &std::collections::HashMap::from([
                (trusted_key, trusted_hash),
                (modified_key, "sha256:old".to_string()),
            ]),
        );

        assert!(!hooks[0].needs_review);
        assert!(hooks[1].needs_review);
        let _ = fs::remove_dir_all(root);
    }

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
    fn marks_claude_hooks_disabled_when_disable_all_hooks_is_set() {
        let root = std::env::temp_dir().join(format!(
            "tendi-claude-disabled-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("settings.json");
        fs::write(
            &path,
            r#"{
  "disableAllHooks": true,
  "hooks": {
    "PreToolUse": [{ "hooks": [{ "type": "command", "command": "/bin/echo checked" }] }]
  }
}"#,
        )
        .expect("write settings");

        let mut hooks = Vec::new();
        let mut warnings = Vec::new();
        scan_hook_file(&path, AgentKind::Claude, &mut hooks, &mut warnings);

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(hooks.len(), 1);
        assert!(!hooks[0].enabled);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn treats_cursor_macos_managed_hooks_as_read_only() {
        assert_eq!(
            crate::providers::agent_provider(AgentKind::Cursor).hook_read_only_reason(
                std::path::Path::new("/Library/Application Support/Cursor/hooks.json"),
            ),
            Some("this hook source is read-only")
        );
    }

    #[test]
    fn writes_codex_trusted_hash_without_touching_other_states() {
        let root = std::env::temp_dir().join(format!(
            "tendi-codex-state-{}-{}",
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
            "[hooks.state.\"/tmp/hooks.json:pre_tool_use:0:0\"]\ntrusted_hash = \"sha256:old\"\n\n[notice]\nhide = true\n",
        )
        .expect("write config");

        crate::providers::codex::write_trusted_hash(
            &path,
            "/tmp/hooks.json:pre_tool_use:0:0",
            "sha256:new",
        )
        .expect("write trusted hash");
        let text = fs::read_to_string(&path).expect("read config");
        assert!(text.contains("trusted_hash = \"sha256:new\""));
        assert!(text.contains("[notice]\nhide = true"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn marks_unreviewed_cursor_hooks_and_accepts_matching_tendi_state() {
        let root = std::env::temp_dir().join(format!(
            "tendi-cursor-review-{}-{}",
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
    "beforeSubmitPrompt": [{ "command": "/bin/echo review" }],
    "stop": [{ "command": "/bin/echo trusted" }]
  }
}"#,
        )
        .expect("write hooks");
        let mut hooks = Vec::new();
        let mut warnings = Vec::new();
        scan_hook_file(&path, AgentKind::Cursor, &mut hooks, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        let trusted_identity = hook_review_identity(&hooks[1]);
        let states = HashMap::from([(trusted_identity, hooks[1].trust_hash.clone())]);
        apply_tendi_hook_review_states(&mut hooks, &states);
        assert!(hooks[0].needs_review);
        assert!(!hooks[1].needs_review);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn review_projection_reads_only_the_requested_hook_source() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-targeted-review-{}-{}",
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
    "SessionStart": [{
      "hooks": [{ "type": "prompt", "prompt": "review this" }]
    }]
  }
}"#,
        )
        .expect("write hooks");

        let scan = super::scan_hook_source_for_review(&path, AgentKind::Codex);
        assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);
        assert_eq!(scan.hooks.len(), 1);
        assert_eq!(scan.hooks[0].path, path);
        assert_eq!(scan.hooks[0].prompt.as_deref(), Some("review this"));
        assert!(scan.hooks[0].provider_review_key.is_some());
        assert!(scan.hooks[0].provider_current_hash.is_none());

        let hook = scan.hooks[0].clone();
        let error = super::review_hook_from_scan(
            HookScan {
                hooks: scan.hooks,
                warnings: Vec::new(),
            },
            HookReviewRequest {
                agent: AgentKind::Codex,
                path: hook.path,
                expected_trust_hash: hook.trust_hash,
                event: hook.event,
                matcher: hook.matcher,
                hook_type: hook.hook_type,
                command: hook.command,
                url: hook.url,
                prompt: hook.prompt,
                filter: hook.filter,
                status_message: hook.status_message,
            },
        )
        .expect_err("prompt hooks must not be reviewed");
        assert!(error.to_string().contains("this hook type does not support review"));

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

        scan_codex_config_hooks(&path, &mut hooks, &mut warnings);

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

        scan_codex_config_hooks(&path, &mut hooks, &mut warnings);

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
    fn deletes_multiple_hooks_from_one_json_source() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-delete-many-json-{}-{}",
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
      { "matcher": "Bash", "hooks": [{ "type": "command", "command": "one" }] },
      { "matcher": "Read", "hooks": [{ "type": "command", "command": "two" }] }
    ]
  }
}"#,
        )
        .expect("write hooks");
        let trust_hash = super::sha256_file(&path).expect("hash");
        let request = |matcher: &str, command: &str| super::HookDeleteRequest {
            agent: AgentKind::Codex,
            path: path.clone(),
            expected_trust_hash: trust_hash.clone(),
            event: "PreToolUse".to_string(),
            matcher: Some(matcher.to_string()),
            hook_type: Some("command".to_string()),
            command: Some(command.to_string()),
            url: None,
            prompt: None,
            filter: None,
            status_message: None,
        };

        super::delete_hooks(vec![request("Bash", "one"), request("Read", "two")])
            .expect("delete hooks");

        let mut hooks = Vec::new();
        let mut warnings = Vec::new();
        scan_hook_file(&path, AgentKind::Codex, &mut hooks, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(hooks.is_empty(), "{hooks:?}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_hook_mutations_accept_only_one_stale_hash() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-concurrent-mutation-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("hooks.json");
        fs::write(
            &path,
            r#"{"hooks":{"Stop":[{"type":"command","command":"one"},{"type":"command","command":"two"}]}}"#,
        )
        .unwrap();
        let trust_hash = super::sha256_file(&path).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles = ["one", "two"].map(|command| {
            let path = path.clone();
            let trust_hash = trust_hash.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                super::delete_hook(super::HookDeleteRequest {
                    agent: AgentKind::Codex,
                    path,
                    expected_trust_hash: trust_hash,
                    event: "Stop".to_string(),
                    matcher: None,
                    hook_type: Some("command".to_string()),
                    command: Some(command.to_string()),
                    url: None,
                    prompt: None,
                    filter: None,
                    status_message: None,
                })
            })
        });
        let results = handles.map(|handle| handle.join().unwrap());

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        let text = fs::read_to_string(&path).unwrap();
        assert_ne!(text.contains("one"), text.contains("two"));
        assert!(fs::read_dir(&root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("tendi-tmp")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_delete_validates_every_source_before_writing() {
        let root = std::env::temp_dir().join(format!(
            "tendi-hooks-delete-validate-all-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let first_path = root.join("first.json");
        let second_path = root.join("second.json");
        let hook = |command: &str| {
            format!(r#"{{"hooks":{{"Stop":[{{"type":"command","command":"{command}"}}]}}}}"#)
        };
        let first_before = hook("first");
        let second_before = hook("second");
        fs::write(&first_path, &first_before).unwrap();
        fs::write(&second_path, &second_before).unwrap();
        let request =
            |path: PathBuf, expected_trust_hash: String, command: &str| super::HookDeleteRequest {
                agent: AgentKind::Codex,
                path,
                expected_trust_hash,
                event: "Stop".to_string(),
                matcher: None,
                hook_type: Some("command".to_string()),
                command: Some(command.to_string()),
                url: None,
                prompt: None,
                filter: None,
                status_message: None,
            };

        let result = super::delete_hooks(vec![
            request(
                first_path.clone(),
                super::sha256_file(&first_path).unwrap(),
                "first",
            ),
            request(second_path, "stale-hash".to_string(), "second"),
        ]);

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(first_path).unwrap(), first_before);
        fs::remove_dir_all(root).unwrap();
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
            agent: AgentKind::Codex,
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
        scan_codex_config_hooks(&path, &mut hooks, &mut warnings);
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
            agent: AgentKind::Codex,
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
            agent: AgentKind::Codex,
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
        scan_codex_config_hooks(&path, &mut hooks, &mut warnings);
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

        scan_claude_component_file(&path, &mut hooks, &mut warnings);

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
