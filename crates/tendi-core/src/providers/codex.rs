use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    env,
    fs::{self, OpenOptions, TryLockError},
    path::{Component, Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use serde_yaml::Value as YamlValue;
use sha2::Digest;
use toml::Value as TomlValue;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};
use walkdir::WalkDir;

use crate::transcript::{
    InternalContextMarker, TranscriptItem, attach_tool_result,
    collect_message_content_with_markers, compact_time, extract_call_id, extract_duration_ms,
    extract_raw_content_text, extract_thinking_text, extract_tool_command, extract_tool_result,
    push_item, push_tool_item, summarize_tool_call,
};

use crate::time::timestamp_ms;

use super::*;

pub(super) struct CodexProvider;

const CODEX_EXTERNAL_SOURCE_PREFIX: &str = "__codex_external__/";
const CODEX_PLUGIN_READ_ONLY_REASON: &str = "Codex plugin MCP is managed by the plugin";

fn infer_mcp_transport(spec: &Value) -> Option<String> {
    spec.get("transport")
        .or_else(|| spec.get("type"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            spec.get("command")
                .and_then(Value::as_str)
                .map(|_| "stdio".to_string())
        })
        .or_else(|| {
            spec.get("url")
                .and_then(Value::as_str)
                .map(|url| if url.contains("/sse") { "sse" } else { "http" }.to_string())
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

fn enrich_mcp_spec(
    name: &str,
    spec: &Value,
    transport: &str,
    enabled: bool,
    base_dir: Option<&Path>,
    probe_cache: &mut crate::mcp::McpProbeCache,
) -> crate::mcp::McpEnrichment {
    if !probe_cache.allows_probe() {
        return crate::mcp::McpEnrichment::default();
    }
    let probe_spec = codex_plugin_probe_spec(spec, base_dir);
    let headers = codex_mcp_auth_headers(name, spec, transport);
    let extra_env = codex_mcp_probe_env(spec, base_dir);
    crate::mcp::enrich_json_mcp_spec_with_headers_at_dir_and_options(
        &probe_spec,
        transport,
        enabled,
        &headers,
        &extra_env,
        codex_mcp_probe_timeout(spec),
        base_dir,
        probe_cache,
    )
}

fn enrich_mcp_toml_spec(
    name: &str,
    spec: &TomlValue,
    transport: &str,
    enabled: bool,
    base_dir: Option<&Path>,
    probe_cache: &mut crate::mcp::McpProbeCache,
) -> crate::mcp::McpEnrichment {
    if !probe_cache.allows_probe() {
        return crate::mcp::McpEnrichment::default();
    }
    let headers = codex_mcp_auth_headers_from_toml(name, spec, transport);
    crate::mcp::enrich_toml_mcp_spec_with_headers_at_dir_and_options(
        spec,
        transport,
        enabled,
        &headers,
        &BTreeMap::new(),
        codex_mcp_probe_timeout_from_toml(spec),
        base_dir,
        probe_cache,
    )
}

fn codex_mcp_probe_timeout(spec: &Value) -> Option<Duration> {
    spec.get("startup_timeout_sec")
        .and_then(Value::as_u64)
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
}

fn codex_mcp_probe_timeout_from_toml(spec: &TomlValue) -> Option<Duration> {
    spec.get("startup_timeout_sec")
        .and_then(TomlValue::as_integer)
        .and_then(|seconds| u64::try_from(seconds).ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
}

fn codex_home_from_mcp_base_dir(base_dir: Option<&Path>) -> Option<PathBuf> {
    base_dir
        .and_then(|path| {
            path.ancestors().find(|ancestor| {
                ancestor.file_name().and_then(|value| value.to_str()) == Some("plugins")
            })
        })
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

fn codex_plugin_icon(manifest: &Value, plugin_root: &Path) -> Option<crate::mcp::McpIcon> {
    let interface = manifest
        .get("interface")
        .or_else(|| manifest.pointer("/extensions/com.openai/interface"));
    interface
        .and_then(|value| {
            ["composerIcon", "logo", "logoDark"]
                .iter()
                .find_map(|key| value.get(*key).and_then(Value::as_str))
        })
        .and_then(|relative| crate::mcp::mcp_icon_from_file(&plugin_root.join(relative)))
}

fn codex_repl_command(codex_home: &Path) -> Option<String> {
    codex_repl_server(codex_home)?
        .get("command")
        .and_then(TomlValue::as_str)
        .map(str::to_string)
}

fn codex_repl_server(codex_home: &Path) -> Option<toml::map::Map<String, TomlValue>> {
    let text = fs::read_to_string(codex_home.join("config.toml")).ok()?;
    let value = toml::from_str::<TomlValue>(&text).ok()?;
    value
        .get("mcp_servers")
        .and_then(TomlValue::as_table)
        .and_then(|servers| {
            servers.values().find_map(|server| {
                let server = server.as_table()?;
                let environment = server.get("env").and_then(TomlValue::as_table)?;
                environment
                    .contains_key("NODE_REPL_NODE_PATH")
                    .then(|| server.clone())
            })
        })
}

fn codex_repl_environment(codex_home: &Path) -> BTreeMap<String, String> {
    codex_repl_server(codex_home)
        .and_then(|server| server.get("env").cloned())
        .and_then(|env| env.as_table().cloned())
        .map(|env| {
            env.into_iter()
                .filter_map(|(name, value)| value.as_str().map(|value| (name, value.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn codex_repl_node_path(codex_home: &Path) -> Option<String> {
    codex_repl_server(codex_home).and_then(|server| {
        server
            .get("env")
            .and_then(TomlValue::as_table)
            .and_then(|env| env.get("NODE_REPL_NODE_PATH"))
            .and_then(TomlValue::as_str)
            .map(str::to_string)
    })
}

fn is_codex_plugin_launcher_spec(spec: &Value, base_dir: Option<&Path>) -> bool {
    base_dir.is_some_and(is_codex_plugin_path)
        && spec
            .get("args")
            .and_then(Value::as_array)
            .is_some_and(|args| {
                args.iter()
                    .filter_map(Value::as_str)
                    .any(|arg| arg.ends_with("scripts/launch.mjs"))
            })
}

fn codex_plugin_probe_spec(spec: &Value, base_dir: Option<&Path>) -> Value {
    if !is_codex_plugin_launcher_spec(spec, base_dir) {
        return spec.clone();
    }

    let mut normalized = spec.clone();
    let Some(object) = normalized.as_object_mut() else {
        return normalized;
    };
    if !object.contains_key("cwd") {
        if let Some(base_dir) = base_dir {
            object.insert(
                "cwd".to_string(),
                Value::String(base_dir.display().to_string()),
            );
        }
    }
    if let Some(codex_home) =
        base_dir.and_then(|base_dir| codex_home_from_mcp_base_dir(Some(base_dir)))
        && let Some(node_path) = codex_repl_node_path(&codex_home)
    {
        object.insert("command".to_string(), Value::String(node_path));
    }
    normalized
}

fn codex_mcp_probe_env(spec: &Value, base_dir: Option<&Path>) -> BTreeMap<String, String> {
    let mut env_values = BTreeMap::new();
    if let Some(names) = spec.get("env_vars").and_then(Value::as_array) {
        for name in names.iter().filter_map(Value::as_str) {
            if let Ok(value) = env::var(name) {
                env_values.insert(name.to_string(), value);
            }
        }
    }

    let codex_home = codex_home_from_mcp_base_dir(base_dir);
    if let Some(codex_home) = codex_home.as_ref() {
        env_values
            .entry("CODEX_HOME".to_string())
            .or_insert_with(|| codex_home.display().to_string());
        if is_codex_plugin_launcher_spec(spec, base_dir) {
            for (name, value) in codex_repl_environment(codex_home) {
                env_values.entry(name).or_insert(value);
            }
            if let Some(value) = codex_repl_command(codex_home) {
                env_values.insert("CUA_REPL_NODE_REPL_PATH".to_string(), value);
            } else if let Ok(value) = env::var("CUA_REPL_NODE_REPL_PATH") {
                env_values
                    .entry("CUA_REPL_NODE_REPL_PATH".to_string())
                    .or_insert(value);
            }
        }
    }
    env_values
}

const CODEX_MCP_OAUTH_SERVICE: &str = "Codex MCP Credentials";
const CODEX_MCP_OAUTH_REFRESH_SKEW_MS: u64 = 30_000;

#[cfg(target_os = "macos")]
static CODEX_MCP_KEYRING_BATCH_CACHE: LazyLock<Mutex<Option<HashMap<String, String>>>> =
    LazyLock::new(|| Mutex::new(None));

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum CodexMcpOAuthStore {
    Auto,
    File,
    Keyring,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CodexMcpAccessTokenCacheKey {
    store: CodexMcpOAuthStore,
    name: String,
    url: String,
}

static CODEX_MCP_ACCESS_TOKEN_CACHE: LazyLock<
    Mutex<HashMap<CodexMcpAccessTokenCacheKey, Option<String>>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn codex_mcp_auth_headers(name: &str, spec: &Value, transport: &str) -> BTreeMap<String, String> {
    let Some(url) = spec
        .get("url")
        .or_else(|| spec.get("serverUrl"))
        .and_then(Value::as_str)
    else {
        return BTreeMap::new();
    };
    codex_mcp_auth_headers_for_url(name, url, transport)
}

fn codex_mcp_auth_headers_from_toml(
    name: &str,
    spec: &TomlValue,
    transport: &str,
) -> BTreeMap<String, String> {
    let Some(url) = spec
        .get("url")
        .or_else(|| spec.get("server_url"))
        .and_then(TomlValue::as_str)
    else {
        return BTreeMap::new();
    };
    codex_mcp_auth_headers_for_url(name, url, transport)
}

fn codex_mcp_auth_headers_for_url(
    name: &str,
    url: &str,
    transport: &str,
) -> BTreeMap<String, String> {
    if transport == "stdio" {
        return BTreeMap::new();
    }
    let Some(access_token) = load_codex_mcp_access_token(name, url) else {
        return BTreeMap::new();
    };
    BTreeMap::from([(
        "Authorization".to_string(),
        format!("Bearer {access_token}"),
    )])
}

fn codex_mcp_oauth_store() -> CodexMcpOAuthStore {
    let Some(config_path) = codex_home_from_system().map(|home| home.join("config.toml")) else {
        return CodexMcpOAuthStore::Auto;
    };
    let Ok(text) = fs::read_to_string(config_path) else {
        return CodexMcpOAuthStore::Auto;
    };
    match toml::from_str::<TomlValue>(&text)
        .ok()
        .and_then(|value| value.get("mcp_oauth_credentials_store").cloned())
        .and_then(|value| value.as_str().map(str::to_ascii_lowercase))
        .as_deref()
    {
        Some("file") => CodexMcpOAuthStore::File,
        Some("keyring") => CodexMcpOAuthStore::Keyring,
        _ => CodexMcpOAuthStore::Auto,
    }
}

fn load_codex_mcp_access_token(name: &str, url: &str) -> Option<String> {
    let store = codex_mcp_oauth_store();
    load_codex_mcp_access_token_cached(
        CodexMcpAccessTokenCacheKey {
            store,
            name: name.to_string(),
            url: url.to_string(),
        },
        || load_codex_mcp_access_token_uncached(name, url, store),
    )
}

fn load_codex_mcp_access_token_cached<F>(
    key: CodexMcpAccessTokenCacheKey,
    load: F,
) -> Option<String>
where
    F: FnOnce() -> Option<String>,
{
    let Ok(mut cache) = CODEX_MCP_ACCESS_TOKEN_CACHE.lock() else {
        return load();
    };
    if let Some(token) = cache.get(&key) {
        return token.clone();
    }

    // Keep the mutex held while reading the credential. Concurrent MCP probes
    // must not all enter the macOS Keychain prompt before the first lookup is
    // cached. Cache misses too, so a denied/missing item does not re-prompt on
    // every projection refresh.
    let token = load();
    cache.insert(key, token.clone());
    token
}

fn load_codex_mcp_access_token_uncached(
    name: &str,
    url: &str,
    store: CodexMcpOAuthStore,
) -> Option<String> {
    match store {
        CodexMcpOAuthStore::File => load_codex_mcp_file_token(name, url),
        CodexMcpOAuthStore::Keyring => load_codex_mcp_keyring_token(name, url),
        CodexMcpOAuthStore::Auto => {
            load_codex_mcp_keyring_token(name, url).or_else(|| load_codex_mcp_file_token(name, url))
        }
    }
}

fn load_codex_mcp_file_token(name: &str, url: &str) -> Option<String> {
    let home = codex_home_from_system()?;
    load_codex_mcp_file_token_from_home(&home, name, url)
}

fn load_codex_mcp_file_token_from_home(home: &Path, name: &str, url: &str) -> Option<String> {
    let text = fs::read_to_string(home.join(".credentials.json")).ok()?;
    let store = serde_json::from_str::<BTreeMap<String, Value>>(&text).ok()?;
    store.values().find_map(|entry| {
        let server_url = entry.get("server_url").and_then(Value::as_str)?;
        let server_name = entry.get("server_name").and_then(Value::as_str)?;
        if server_url != url || !codex_server_names_match(name, server_name) {
            return None;
        }
        let access_token = entry.get("access_token").and_then(Value::as_str)?;
        token_if_current(access_token, entry.get("expires_at"))
    })
}

fn load_codex_mcp_keyring_token(name: &str, url: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let keys = codex_mcp_store_keys(name, url);
        if let Ok(cache) = CODEX_MCP_KEYRING_BATCH_CACHE.lock() {
            if let Some(credentials) = cache.as_ref() {
                return keys.iter().find_map(|key| {
                    credentials
                        .get(key)
                        .and_then(|serialized| parse_codex_mcp_keyring_token(name, url, serialized))
                });
            }
        }

        for key in keys {
            let Ok(entry) = keyring::Entry::new(CODEX_MCP_OAUTH_SERVICE, &key) else {
                continue;
            };
            let Ok(serialized) = entry.get_password() else {
                continue;
            };
            if let Some(token) = parse_codex_mcp_keyring_token(name, url, &serialized) {
                return Some(token);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn parse_codex_mcp_keyring_token(name: &str, url: &str, serialized: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(serialized).ok()?;
    let server_url = value.get("url").and_then(Value::as_str).unwrap_or_default();
    let server_name = value
        .get("server_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if server_url != url || !codex_server_names_match(name, server_name) {
        return None;
    }
    let access_token = value
        .get("token_response")
        .and_then(|token| token.get("access_token"))
        .and_then(Value::as_str)?;
    token_if_current(access_token, value.get("expires_at"))
}

#[cfg(target_os = "macos")]
fn prefetch_codex_mcp_keyring_tokens() {
    let Ok(mut cache) = CODEX_MCP_KEYRING_BATCH_CACHE.lock() else {
        return;
    };
    if cache.is_some() {
        return;
    }
    let Some(credentials) = load_codex_mcp_keyring_tokens() else {
        return;
    };
    *cache = Some(credentials);
}

#[cfg(target_os = "macos")]
fn load_codex_mcp_keyring_tokens() -> Option<HashMap<String, String>> {
    use security_framework::{
        item::{ItemClass, ItemSearchOptions, Limit},
        os::macos::keychain::{SecKeychain, SecPreferencesDomain},
    };

    let keychain = SecKeychain::default_for_domain(SecPreferencesDomain::User).ok()?;
    let mut options = ItemSearchOptions::new();
    options
        .keychains(std::slice::from_ref(&keychain))
        .class(ItemClass::generic_password())
        .service(CODEX_MCP_OAUTH_SERVICE)
        .load_attributes(true)
        .load_data(true)
        .limit(Limit::All);

    let items = options.search().ok()?;
    let mut credentials = HashMap::new();
    for item in items {
        let Some(attributes) = item.simplify_dict() else {
            continue;
        };
        let Some(account) = attributes.get("acct") else {
            continue;
        };
        let Some(serialized) = attributes.get("v_Data") else {
            continue;
        };
        credentials.insert(account.clone(), serialized.clone());
    }
    Some(credentials)
}

fn codex_server_names_match(config_name: &str, stored_name: &str) -> bool {
    config_name == stored_name
        || config_name.strip_prefix("local:") == Some(stored_name)
        || stored_name.strip_prefix("local:") == Some(config_name)
}

fn token_if_current(access_token: &str, expires_at: Option<&Value>) -> Option<String> {
    if access_token.trim().is_empty() {
        return None;
    }
    let current = expires_at.and_then(Value::as_u64).is_none_or(|expires_at| {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now.saturating_add(CODEX_MCP_OAUTH_REFRESH_SKEW_MS) < expires_at
    });
    current.then(|| access_token.to_string())
}

#[cfg(target_os = "macos")]
fn codex_mcp_store_keys(name: &str, url: &str) -> Vec<String> {
    let name = name.strip_prefix("local:").unwrap_or(name);
    let encoded_url = serde_json::to_string(url).unwrap_or_else(|_| format!("\"{url}\""));
    let payloads = [
        format!(r#"{{"type":"http","url":{encoded_url},"headers":{{}}}}"#),
        format!(r#"{{"headers":{{}},"type":"http","url":{encoded_url}}}"#),
    ];
    payloads
        .into_iter()
        .map(|payload| {
            let digest = sha2::Sha256::digest(payload.as_bytes());
            let hash = digest
                .iter()
                .take(8)
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            format!("{name}|{hash}")
        })
        .collect()
}

fn infer_mcp_toml_transport(spec: &TomlValue) -> Option<String> {
    spec.get("transport")
        .or_else(|| spec.get("type"))
        .and_then(TomlValue::as_str)
        .map(str::to_string)
        .or_else(|| {
            spec.get("command")
                .and_then(TomlValue::as_str)
                .map(|_| "stdio".to_string())
        })
        .or_else(|| {
            spec.get("url")
                .and_then(TomlValue::as_str)
                .map(|url| if url.contains("/sse") { "sse" } else { "http" }.to_string())
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

fn update_mcp_toml_server(spec: &mut dyn toml_edit::TableLike, enabled: bool) -> bool {
    let has_disabled = spec.contains_key("disabled");
    let has_enabled = spec.contains_key("enabled");
    if has_disabled {
        spec.insert("disabled", toml_edit::value(!enabled));
    } else if has_enabled {
        spec.insert("enabled", toml_edit::value(enabled));
    } else {
        spec.insert("enabled", toml_edit::value(enabled));
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
const CODEX_GOAL_CONTEXT_PREFIX: &str = "<codex_internal_context source=\"goal\">";
const CODEX_CONTEXT_CLOSE: &str = "</codex_internal_context>";
const CODEX_SELECTED_SKILL_KIND: &str = "skills.selected_skill_instructions";

fn is_codex_selected_skill(value: &Value) -> bool {
    if value.pointer("/payload/type").and_then(Value::as_str) != Some("message")
        || value.pointer("/payload/role").and_then(Value::as_str) != Some("user")
    {
        return false;
    }

    let has_skill_content_kind = value
        .pointer("/payload/internal_chat_message_metadata_passthrough/content_item_kinds")
        .and_then(Value::as_array)
        .is_some_and(|kinds| {
            kinds
                .iter()
                .any(|kind| kind.as_str() == Some(CODEX_SELECTED_SKILL_KIND))
        });
    if has_skill_content_kind {
        return true;
    }

    extract_raw_content_text(value.pointer("/payload/content"))
        .is_some_and(|text| is_codex_skill_wrapper(&text))
}

fn codex_selected_skill_candidate(value: &Value) -> Option<SkillEvidenceCandidate> {
    if !is_codex_selected_skill(value) {
        return None;
    }
    let text = extract_raw_content_text(value.pointer("/payload/content"))?;
    let name = xml_tag_value(&text, "name");
    let path = xml_tag_value(&text, "path");
    (name.is_some() || path.is_some()).then(|| SkillEvidenceCandidate {
        name,
        path,
        evidence: Evidence {
            kind: "explicit_skill".to_string(),
            text,
            time: value
                .get("timestamp")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        confidence: "explicit",
    })
}

fn xml_tag_value(text: &str, tag: &str) -> Option<String> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = text.find(&start_tag)? + start_tag.len();
    let end = text[start..].find(&end_tag)? + start;
    let value = text[start..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn is_codex_skill_wrapper(text: &str) -> bool {
    let text = text.trim();
    text.starts_with("<skill>")
        && text.ends_with("</skill>")
        && xml_tag_value(text, "name").is_some()
        && xml_tag_value(text, "path").is_some()
}

fn extract_goal_objective(value: &Value) -> Option<String> {
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("message")
        || payload.get("role").and_then(Value::as_str) != Some("user")
    {
        return None;
    }
    let content = extract_raw_content_text(payload.get("content"))?;
    let context_start = content.find(CODEX_GOAL_CONTEXT_PREFIX)? + CODEX_GOAL_CONTEXT_PREFIX.len();
    let context = &content[context_start..];
    let context_end = context.find(CODEX_CONTEXT_CLOSE)?;
    let context = &context[..context_end];
    let objective_start = context.find("<objective>")? + "<objective>".len();
    let objective = &context[objective_start..];
    let objective_end = objective.find("</objective>")?;
    let objective = objective[..objective_end].trim();
    (!objective.is_empty()).then(|| objective.to_string())
}

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
    let newline = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut updated = lines.join(newline);
    if original.ends_with(newline) {
        updated.push_str(newline);
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
    #[serde(rename = "SessionEnd", default)]
    session_end: Vec<Value>,
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
    fn event_groups(&self) -> [(&str, &Vec<Value>); 11] {
        [
            ("PreToolUse", &self.pre_tool_use),
            ("PermissionRequest", &self.permission_request),
            ("PostToolUse", &self.post_tool_use),
            ("PreCompact", &self.pre_compact),
            ("PostCompact", &self.post_compact),
            ("SessionStart", &self.session_start),
            ("SessionEnd", &self.session_end),
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

fn render_codex_policy(before: Option<&str>, visibility: SkillVisibility) -> Result<String> {
    if visibility == SkillVisibility::Mixed {
        bail!("mixed visibility is a scan summary and cannot be written to Codex policy");
    }
    let desired = matches!(visibility, SkillVisibility::Auto);
    let Some(before) = before else {
        return Ok(if desired {
            String::new()
        } else {
            "policy:\n  allow_implicit_invocation: false\n".to_string()
        });
    };
    let root = serde_yaml::from_str::<YamlValue>(before)?;
    let current = root
        .get("policy")
        .and_then(|policy| policy.get("allow_implicit_invocation"))
        .and_then(YamlValue::as_bool);
    if current == Some(desired) {
        return Ok(before.to_string());
    }
    if !matches!(root, YamlValue::Mapping(_)) {
        bail!("Codex policy document root must be a YAML mapping");
    }
    update_codex_policy_text(before, desired)
}

fn update_codex_policy_text(before: &str, desired: bool) -> Result<String> {
    let newline = if before.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let trailing_newline = before.ends_with('\n');
    let mut lines = before
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect::<Vec<_>>();
    if trailing_newline {
        lines.pop();
    }
    if lines.len() == 1 && lines[0].trim().is_empty() {
        lines.clear();
    }

    let policy = lines.iter().enumerate().find_map(|(index, line)| {
        yaml_key_line(line, "policy")
            .filter(|(indent, _)| *indent == 0)
            .map(|(_, colon)| (index, colon))
    });
    let value = if desired { "true" } else { "false" };

    let Some((policy_index, policy_colon)) = policy else {
        lines.push("policy:".to_string());
        lines.push(format!("  allow_implicit_invocation: {value}"));
        return Ok(render_codex_policy_lines(
            &lines,
            newline,
            trailing_newline || before.is_empty(),
        ));
    };

    let policy_tail = lines[policy_index][policy_colon + 1..].trim();
    if !policy_tail.is_empty() && !policy_tail.starts_with('#') {
        bail!("Codex policy must use a block YAML mapping");
    }

    let policy_indent = yaml_key_line(&lines[policy_index], "policy")
        .map(|(indent, _)| indent)
        .unwrap_or(0);
    let section_end = lines
        .iter()
        .enumerate()
        .skip(policy_index + 1)
        .find(|(_, line)| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with('#')
                && leading_yaml_indent(line) <= policy_indent
        })
        .map(|(index, _)| index)
        .unwrap_or(lines.len());

    for line in lines.iter_mut().take(section_end).skip(policy_index + 1) {
        let Some((indent, colon)) = yaml_key_line(line, "allow_implicit_invocation") else {
            continue;
        };
        if indent > policy_indent {
            *line = replace_yaml_bool_value(line, colon, value);
            return Ok(render_codex_policy_lines(&lines, newline, trailing_newline));
        }
    }

    let child_indent = lines
        .iter()
        .take(section_end)
        .skip(policy_index + 1)
        .find_map(|line| {
            let trimmed = line.trim();
            (!trimmed.is_empty() && !trimmed.starts_with('#'))
                .then(|| leading_yaml_indent(line))
                .filter(|indent| *indent > policy_indent)
        })
        .unwrap_or(policy_indent + 2);
    lines.insert(
        section_end,
        format!(
            "{}allow_implicit_invocation: {value}",
            " ".repeat(child_indent)
        ),
    );
    Ok(render_codex_policy_lines(&lines, newline, trailing_newline))
}

fn leading_yaml_indent(line: &str) -> usize {
    line.bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

fn yaml_key_line(line: &str, key: &str) -> Option<(usize, usize)> {
    let indent = leading_yaml_indent(line);
    let content = &line[indent..];
    let colon = content.find(':')?;
    (content[..colon].trim() == key).then_some((indent, indent + colon))
}

fn replace_yaml_bool_value(line: &str, colon: usize, value: &str) -> String {
    let suffix = &line[colon + 1..];
    let comment = suffix.find(" #").unwrap_or(suffix.len());
    let value_part = &suffix[..comment];
    let leading = value_part.len() - value_part.trim_start().len();
    let trailing = value_part.len() - value_part.trim_end().len();
    let between = &value_part[..leading];
    let ending = &value_part[value_part.len().saturating_sub(trailing)..];
    format!(
        "{}{}{}{}{}",
        &line[..colon + 1],
        between,
        value,
        ending,
        &suffix[comment..]
    )
}

fn render_codex_policy_lines(lines: &[String], newline: &str, trailing_newline: bool) -> String {
    let mut rendered = lines.join(newline);
    if trailing_newline {
        rendered.push_str(newline);
    }
    rendered
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
    Ok(crate::fsutil::preserve_newline_style(
        before,
        doc.to_string(),
    ))
}

fn legacy_codex_skill_target(
    path: &Path,
    legacy_skill_root: &Path,
    canonical_skill_root: &Path,
) -> Option<PathBuf> {
    let relative = path.strip_prefix(legacy_skill_root).ok()?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        || path.exists()
    {
        return None;
    }
    let target = canonical_skill_root.join(relative);
    target.is_file().then_some(target)
}

fn migrate_codex_skill_config_paths(
    before: &str,
    legacy_skill_root: &Path,
    canonical_skill_root: &Path,
) -> Result<String> {
    let mut doc = if before.trim().is_empty() {
        DocumentMut::new()
    } else {
        before.parse::<DocumentMut>()?
    };
    let Some(skills) = doc.get_mut("skills").and_then(Item::as_table_mut) else {
        return Ok(before.to_string());
    };
    let Some(configs) = skills
        .get_mut("config")
        .and_then(Item::as_array_of_tables_mut)
    else {
        return Ok(before.to_string());
    };

    let original = configs.iter().cloned().collect::<Vec<_>>();
    let original_paths = original
        .iter()
        .filter_map(|config| config.get("path").and_then(Item::as_str))
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let mut migrated_targets = BTreeSet::new();
    let mut changed = false;
    let mut migrated = Vec::with_capacity(original.len());

    for mut config in original {
        let Some(path) = config.get("path").and_then(Item::as_str) else {
            migrated.push(config);
            continue;
        };
        let Some(target) =
            legacy_codex_skill_target(Path::new(path), legacy_skill_root, canonical_skill_root)
        else {
            migrated.push(config);
            continue;
        };

        let target_keys = path_lookup_keys(&target);
        let has_canonical_entry = original_paths.iter().any(|existing| {
            path_lookup_keys(existing)
                .iter()
                .any(|key| target_keys.iter().any(|target_key| key == target_key))
        });
        let target_key = target
            .canonicalize()
            .unwrap_or_else(|_| target.to_path_buf());
        if has_canonical_entry || !migrated_targets.insert(target_key) {
            changed = true;
            continue;
        }

        config["path"] = value(target.to_string_lossy().to_string());
        changed = true;
        migrated.push(config);
    }

    if !changed {
        return Ok(before.to_string());
    }
    *configs = migrated.into_iter().collect();
    Ok(crate::fsutil::preserve_newline_style(
        before,
        doc.to_string(),
    ))
}

pub(crate) fn migrate_legacy_global_skill_config() -> Result<()> {
    let Some(home) = dirs::home_dir() else {
        return Ok(());
    };
    let Some(codex_home) = codex_home_from_system() else {
        return Ok(());
    };
    let config_path = codex_home.join("config.toml");
    let before = match fs::read_to_string(&config_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read Codex config {}", config_path.display()));
        }
    };
    let after = migrate_codex_skill_config_paths(
        &before,
        &codex_home.join("skills"),
        &home.join(".agents/skills"),
    )?;
    if before == after {
        return Ok(());
    }
    crate::fsutil::atomic_write(&config_path, &after)
        .with_context(|| format!("failed to migrate Codex config {}", config_path.display()))
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
    Ok(
        serde_yaml::from_str::<YamlValue>(&expected)?
            == serde_yaml::from_str::<YamlValue>(current)?,
    )
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
    const MARKERS: [&[u8]; 12] = [
        b"\"session_meta\"",
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

    if entry_type == "session_meta" {
        if let Some(model) = extract_model(&value) {
            crate::analytics::set_model(record, &model);
        }
        return;
    }
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
            crate::analytics::start_open_run(record, stamp);
        }
        return;
    }
    if payload_type == "task_complete" {
        if payload
            .get("last_agent_message")
            .is_some_and(Value::is_null)
        {
            crate::analytics::discard_open_run(record);
        } else {
            crate::analytics::close_open_run(record, timestamp.as_deref().unwrap_or(""), true);
        }
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
        Some("session_meta") => value
            .pointer("/payload/base_instructions/provenance/model")
            .or_else(|| value.pointer("/payload/provenance/model")),
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
        if let Some(title) = extract_goal_objective(&value)
            .and_then(|objective| crate::sessions::clean_title(&objective))
        {
            return provider_title.or(Some(title));
        }
        if let Some(title) =
            crate::sessions::extract_session_title_for_agent(AgentKind::Codex, &value)
        {
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
        Err(TryLockError::WouldBlock) => Ok(Some(SessionWriter { lock_path })),
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
            value.as_table().map(|value| {
                (
                    id.to_string(),
                    value
                        .get("enabled")
                        .and_then(TomlValue::as_bool)
                        .unwrap_or(true),
                )
            })
        })
        .collect()
}

fn codex_plugin_mcp_enabled_by_id(codex_home: &Path) -> BTreeMap<(String, String), bool> {
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
        .flat_map(|(plugin_id, plugin)| {
            plugin
                .get("mcp_servers")
                .and_then(TomlValue::as_table)
                .into_iter()
                .flat_map(move |servers| {
                    servers.iter().filter_map(move |(server_name, server)| {
                        server
                            .get("enabled")
                            .and_then(TomlValue::as_bool)
                            .map(|enabled| {
                                ((plugin_id.to_string(), server_name.to_string()), enabled)
                            })
                    })
                })
        })
        .collect()
}

fn codex_plugin_root(manifest_path: &Path) -> Option<&Path> {
    let parent = manifest_path.parent()?;
    match parent.file_name().and_then(|value| value.to_str()) {
        Some(".codex-plugin") => parent.parent(),
        Some(".claude-plugin") => None,
        _ => Some(parent),
    }
}

fn codex_plugin_id(codex_home: &Path, plugin_root: &Path, manifest: &Value) -> Option<String> {
    let relative = plugin_root
        .strip_prefix(codex_home.join("plugins/cache"))
        .ok()?;
    let mut parts = relative
        .components()
        .filter_map(|part| part.as_os_str().to_str());
    let marketplace = parts.next()?;
    let plugin_directory = parts.next()?;
    let plugin_name = manifest
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(plugin_directory);
    Some(format!("{plugin_name}@{marketplace}"))
}

fn codex_plugin_directory_id(codex_home: &Path, plugin_root: &Path) -> Option<String> {
    let relative = plugin_root
        .strip_prefix(codex_home.join("plugins/cache"))
        .ok()?;
    let mut parts = relative
        .components()
        .filter_map(|part| part.as_os_str().to_str());
    let marketplace = parts.next()?;
    let plugin_directory = parts.next()?;
    Some(format!("{plugin_directory}@{marketplace}"))
}

fn codex_plugin_manifest_paths(codex_home: &Path) -> Vec<PathBuf> {
    let cache = codex_home.join("plugins/cache");
    if !cache.is_dir() {
        return Vec::new();
    }
    let mut paths = WalkDir::new(&cache)
        .follow_links(false)
        .max_depth(6)
        .into_iter()
        .filter_entry(|entry| !is_skipped_plugin_entry(entry.path()))
        .filter_map(Result::ok)
        .filter(|entry| {
            if entry.file_type().is_dir() {
                return true;
            }
            if entry.file_name() != "plugin.json" {
                return false;
            }
            let Ok(relative) = entry.path().strip_prefix(&cache) else {
                return false;
            };
            let components = relative.components().collect::<Vec<_>>();
            (components.len() == 4 && components[3].as_os_str() == "plugin.json")
                || (components.len() == 5
                    && components[3].as_os_str() == ".codex-plugin"
                    && components[4].as_os_str() == "plugin.json")
        })
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| {
        if path
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            == Some(".codex-plugin")
        {
            1
        } else {
            0
        }
    });
    paths
}

fn read_codex_plugin_mcp_source(
    path: &Path,
    warnings: &mut Vec<String>,
) -> Option<(PathBuf, String, Value)> {
    let Ok(text) = fs::read_to_string(path) else {
        return None;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        warnings.push(format!("{}: invalid Codex plugin MCP JSON", path.display()));
        return None;
    };
    Some((path.to_path_buf(), text, value))
}

fn codex_plugin_mcp_sources(
    manifest_path: &Path,
    manifest_text: &str,
    manifest: &Value,
    plugin_root: &Path,
    plugin_name: &str,
    warnings: &mut Vec<String>,
) -> Vec<(PathBuf, String, Value)> {
    let Some(declaration) = manifest.get("mcpServers") else {
        return [plugin_root.join("mcp.json"), plugin_root.join(".mcp.json")]
            .into_iter()
            .find_map(|path| read_codex_plugin_mcp_source(&path, warnings))
            .into_iter()
            .collect();
    };
    match declaration {
        Value::String(relative) => {
            read_codex_plugin_mcp_source(&plugin_root.join(relative), warnings)
                .into_iter()
                .collect()
        }
        Value::Object(object) => {
            let value = if is_mcp_server_spec(object) {
                json!({"mcpServers": {plugin_name: declaration}})
            } else {
                json!({"mcpServers": declaration})
            };
            vec![(
                manifest_path.to_path_buf(),
                manifest_text.to_string(),
                value,
            )]
        }
        Value::Array(entries) => entries
            .iter()
            .flat_map(|entry| match entry {
                Value::String(relative) => {
                    read_codex_plugin_mcp_source(&plugin_root.join(relative), warnings)
                        .into_iter()
                        .collect::<Vec<_>>()
                }
                Value::Object(object) => {
                    let value = if is_mcp_server_spec(object) {
                        json!({"mcpServers": {plugin_name: entry}})
                    } else {
                        json!({"mcpServers": entry})
                    };
                    vec![(
                        manifest_path.to_path_buf(),
                        manifest_text.to_string(),
                        value,
                    )]
                }
                _ => Vec::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn is_mcp_server_spec(object: &serde_json::Map<String, Value>) -> bool {
    ["command", "url", "serverUrl", "type", "transport"]
        .iter()
        .any(|key| object.contains_key(*key))
}

fn codex_plugin_mcp_entries(
    value: &Value,
    fallback_name: &str,
) -> Vec<(String, Value, Vec<String>)> {
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    if let Some(servers) = object.get("mcpServers").and_then(Value::as_object) {
        if is_mcp_server_spec(servers) {
            return vec![(
                fallback_name.to_string(),
                Value::Object(servers.clone()),
                vec!["mcpServers".to_string()],
            )];
        }
        return servers
            .iter()
            .map(|(name, spec)| (name.clone(), spec.clone(), vec!["mcpServers".to_string()]))
            .collect();
    }
    object
        .iter()
        .filter(|(name, spec)| name.as_str() != "$schema" && spec.is_object())
        .map(|(name, spec)| (name.clone(), spec.clone(), Vec::new()))
        .collect()
}

fn codex_plugin_enrichment(
    manifest: &Value,
    plugin_root: &Path,
    plugin_name: &str,
    server_name: &str,
    spec: &Value,
) -> crate::mcp::McpEnrichment {
    let interface = manifest
        .get("interface")
        .or_else(|| manifest.pointer("/extensions/com.openai/interface"));
    let display_name = interface
        .and_then(|value| value.get("displayName"))
        .and_then(Value::as_str)
        .or_else(|| manifest.get("displayName").and_then(Value::as_str))
        .or_else(|| manifest.get("name").and_then(Value::as_str));
    let description = interface
        .and_then(|value| value.get("longDescription"))
        .and_then(Value::as_str)
        .or_else(|| manifest.get("description").and_then(Value::as_str));
    let website = interface
        .and_then(|value| value.get("websiteURL"))
        .and_then(Value::as_str)
        .or_else(|| manifest.get("homepage").and_then(Value::as_str));
    let icon = codex_plugin_icon(manifest, plugin_root);
    crate::mcp::McpEnrichment {
        plugin_name: Some(plugin_name.to_string()),
        server_name: Some(server_name.to_string()),
        server_title: display_name.map(str::to_string),
        server_version: manifest
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string),
        server_description: description.map(str::to_string),
        server_website_url: website.map(str::to_string),
        icons: icon.into_iter().collect(),
        tools: spec
            .get("enabled_tools")
            .and_then(Value::as_array)
            .into_iter()
            .flat_map(|tools| tools.iter())
            .filter_map(Value::as_str)
            .map(|name| crate::mcp::McpTool {
                name: name.to_string(),
                title: None,
                description: None,
                input_schema: None,
                icons: Vec::new(),
            })
            .take(512)
            .collect(),
        ..Default::default()
    }
}

pub(crate) fn scan_codex_plugin_mcp(
    codex_home: &Path,
    servers: &mut Vec<McpServerRecord>,
    warnings: &mut Vec<String>,
) {
    let plugin_enabled = codex_plugin_enabled_by_id(codex_home);
    let server_enabled = codex_plugin_mcp_enabled_by_id(codex_home);
    let mut scanned_plugins = BTreeSet::new();
    for manifest_path in codex_plugin_manifest_paths(codex_home) {
        let Ok(manifest_text) = fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<Value>(&manifest_text) else {
            continue;
        };
        let Some(plugin_root) = codex_plugin_root(&manifest_path) else {
            continue;
        };
        let Some(plugin_id) = codex_plugin_id(codex_home, plugin_root, &manifest) else {
            continue;
        };
        let enabled = plugin_enabled.get(&plugin_id).copied().or_else(|| {
            codex_plugin_directory_id(codex_home, plugin_root)
                .and_then(|id| plugin_enabled.get(&id).copied())
        });
        let Some(plugin_enabled) = enabled else {
            continue;
        };
        let Some(plugin_name) = manifest.get("name").and_then(Value::as_str) else {
            continue;
        };
        let sources = codex_plugin_mcp_sources(
            &manifest_path,
            &manifest_text,
            &manifest,
            plugin_root,
            plugin_name,
            warnings,
        );
        if sources.is_empty() || !scanned_plugins.insert(plugin_id.clone()) {
            continue;
        }
        for (path, text, value) in sources {
            for (server_name, spec, server_path) in codex_plugin_mcp_entries(&value, plugin_name) {
                let Some(transport) = infer_mcp_transport(&spec) else {
                    warnings.push(format!(
                        "{}: MCP server {server_name} has no recognized transport",
                        path.display()
                    ));
                    continue;
                };
                let effective_enabled = plugin_enabled
                    && server_enabled
                        .get(&(plugin_id.clone(), server_name.clone()))
                        .copied()
                        .unwrap_or_else(|| infer_mcp_enabled(&spec));
                let enrichment = codex_plugin_enrichment(
                    &manifest,
                    plugin_root,
                    plugin_name,
                    &server_name,
                    &spec,
                );
                servers.push(crate::mcp::build_mcp_server_record(
                    &path,
                    AgentKind::Codex,
                    "global",
                    &text,
                    &server_path,
                    &server_name,
                    transport,
                    effective_enabled,
                    if effective_enabled {
                        "configured".to_string()
                    } else {
                        "disabled".to_string()
                    },
                    enrichment,
                    Some(CODEX_PLUGIN_READ_ONLY_REASON.to_string()),
                ));
            }
        }
    }
}

fn filter_codex_config_servers_shadowed_by_plugins(
    servers: &mut Vec<McpServerRecord>,
    plugin_servers: &[McpServerRecord],
) {
    let plugin_names = plugin_servers
        .iter()
        .map(|server| server.name.as_str())
        .collect::<BTreeSet<_>>();
    servers.retain(|server| !plugin_names.contains(server.name.as_str()));
}

fn is_codex_plugin_path(path: &Path) -> bool {
    let components = path.components().collect::<Vec<_>>();
    components.windows(2).any(|window| {
        window[0].as_os_str() == std::ffi::OsStr::new("plugins")
            && window[1].as_os_str() == std::ffi::OsStr::new("cache")
    })
}

fn preserve_codex_plugin_identity(
    current: &McpServerRecord,
    mut updated: McpServerRecord,
) -> McpServerRecord {
    let is_plugin = current.read_only_reason.as_deref() == Some(CODEX_PLUGIN_READ_ONLY_REASON);
    if !is_plugin {
        return updated;
    }

    updated.server_name = current.server_name.clone();
    updated.server_title = current.server_title.clone();
    updated.server_version = current.server_version.clone();
    updated.server_description = current.server_description.clone();
    updated.server_website_url = current.server_website_url.clone();
    if updated.icons.is_empty() {
        updated.icons = current.icons.clone();
    }
    updated
}

fn codex_mcp_base_dir(path: &Path) -> Option<&Path> {
    let parent = path.parent()?;
    if parent.file_name().and_then(|value| value.to_str()) == Some(".codex-plugin") {
        parent.parent()
    } else {
        Some(parent)
    }
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

fn codex_model_instructions_file(path: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(path).ok()?;
    let value = toml::from_str::<TomlValue>(&text).ok()?;
    let configured = value
        .get("model_instructions_file")
        .and_then(TomlValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let configured = PathBuf::from(configured);
    Some(if configured.is_absolute() {
        configured
    } else {
        path.parent()
            .unwrap_or_else(|| Path::new("."))
            .join(configured)
    })
}

fn codex_model_instructions_files(ctx: &ProviderContext) -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    let candidates = std::iter::once(("global".to_string(), codex_home(ctx).join("config.toml")))
        .chain(
            ctx.project_dirs()
                .iter()
                .map(|dir| ("project".to_string(), dir.join(".codex/config.toml"))),
        );
    for (scope, config_path) in candidates {
        let Some(instructions_path) = codex_model_instructions_file(&config_path) else {
            continue;
        };
        if seen.insert(instructions_path.clone()) {
            files.push((scope, instructions_path));
        }
    }
    files
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
        .and_then(timestamp_ms);

    match payload.get("type").and_then(Value::as_str) {
        Some("message") => {
            let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
            let content = payload.get("content");
            if is_codex_selected_skill(value) {
                if let Some(body) = extract_raw_content_text(content) {
                    push_item(items, "context", body, Some("Skill".to_string()), time);
                }
                return;
            }
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
            if role == "user" {
                if let Some(objective) = extract_goal_objective(value) {
                    push_item(items, "user", objective, None, time);
                    return;
                }
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
            let command = extract_codex_tool_command(payload);
            let summary = command
                .as_deref()
                .map(|command| command.chars().take(220).collect())
                .unwrap_or_else(|| summarize_tool_call(payload));
            push_tool_item(
                items,
                "tool",
                summary,
                Some(kind.to_string()),
                time,
                command,
                None,
                extract_duration_ms(payload, None),
                extract_call_id(payload),
                timestamp_ms,
            );
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

    fn backup_global_source_key(&self, path: &Path) -> Option<String> {
        let home = dirs::home_dir()?;
        let codex_root = self.global_backup_root(&home)?;
        if let Some(relative) = path.strip_prefix(&codex_root).ok() {
            return relative.to_str().map(str::to_string);
        }
        path.strip_prefix(&home)
            .ok()
            .and_then(|relative| relative.to_str())
            .map(|relative| format!("{CODEX_EXTERNAL_SOURCE_PREFIX}{relative}"))
    }

    fn restore_global_source_path(&self, relative: &str) -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        if let Some(relative) = relative.strip_prefix(CODEX_EXTERNAL_SOURCE_PREFIX) {
            let relative = Path::new(relative);
            if relative.as_os_str().is_empty()
                || relative.is_absolute()
                || relative
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                return None;
            }
            return Some(home.join(relative));
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

    fn normalize_skill_file_for_merge(
        &self,
        relative_path: &str,
        local: Option<&str>,
        base: Option<&str>,
        incoming: Option<&str>,
        visibility: SkillVisibility,
    ) -> Option<(Option<String>, Option<String>, Option<String>)> {
        if !self.is_managed_skill_file(relative_path) {
            return None;
        }
        let normalize = |text: Option<&str>| {
            text.map(|text| render_codex_policy(Some(text), visibility))
                .transpose()
                .ok()?
        };
        Some((normalize(local), normalize(base), normalize(incoming)))
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
        for (scope, path) in codex_model_instructions_files(ctx) {
            rules::add_rule_file(
                rules_out,
                warnings,
                order,
                self.kind(),
                "model-instructions-file",
                &scope,
                path,
            );
        }
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

    fn assistant_ask_command(&self, workspace: &Path, prompt: &str) -> Option<SessionCommand> {
        Some(SessionCommand {
            executable: "codex".to_string(),
            args: vec![
                "-C".to_string(),
                workspace.display().to_string(),
                "exec".to_string(),
                "--ephemeral".to_string(),
                "--json".to_string(),
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                prompt.to_string(),
            ],
            cwd: Some(workspace.to_path_buf()),
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

    fn session_user_message(&self, value: &Value) -> Option<String> {
        extract_goal_objective(value)
    }

    fn session_transcript_title_overrides_index(&self) -> bool {
        true
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

    fn session_message_kind(&self, value: &Value) -> Option<SessionMessageKind> {
        if is_codex_selected_skill(value) {
            Some(SessionMessageKind::Context)
        } else {
            default_session_message_kind(value)
        }
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
            duration: true,
            rate_limit_history: true,
        }
    }

    fn parse_analytics_line(&self, line: &str, record: &mut SessionAnalyticsRecord) {
        parse_analytics_line(line, record);
    }

    fn extract_skill_tool_payloads<'a>(&self, value: &'a Value) -> Vec<(&'a Value, Evidence)> {
        tool_payloads(value)
    }

    fn extract_session_skill_evidence(&self, value: &Value) -> Vec<SkillEvidenceCandidate> {
        codex_selected_skill_candidate(value).into_iter().collect()
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
        probe_cache: &mut crate::mcp::McpProbeCache,
    ) -> Result<()> {
        if let Some(home) = &ctx.home {
            let root = codex_home(ctx);
            let mut config_servers = Vec::new();
            crate::mcp::scan_toml_mcp(
                &root.join("config.toml"),
                self.kind(),
                "global",
                "mcp_servers",
                infer_mcp_toml_transport,
                infer_mcp_toml_enabled,
                infer_mcp_toml_status,
                enrich_mcp_toml_spec,
                probe_cache,
                &mut config_servers,
                warnings,
            );
            let plugin_start = servers.len();
            scan_codex_plugin_mcp(&root, servers, warnings);
            filter_codex_config_servers_shadowed_by_plugins(
                &mut config_servers,
                &servers[plugin_start..],
            );
            servers.extend(config_servers);
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
                enrich_mcp_spec,
                probe_cache,
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
                enrich_mcp_toml_spec,
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
        probe_cache: &mut crate::mcp::McpProbeCache,
    ) -> Result<McpServerRecord> {
        let updated = match request.path.extension().and_then(|value| value.to_str()) {
            Some("toml") => crate::mcp::probe_toml_mcp_server(
                request,
                current,
                "mcp_servers",
                infer_mcp_toml_transport,
                infer_mcp_toml_enabled,
                infer_mcp_toml_status,
                enrich_mcp_toml_spec,
                probe_cache,
            ),
            Some("json") => crate::mcp::probe_json_mcp_server_at_dir(
                request,
                current,
                infer_mcp_transport,
                infer_mcp_enabled,
                infer_mcp_status,
                enrich_mcp_spec,
                codex_mcp_base_dir(&request.path),
                probe_cache,
            ),
            _ => bail!("Codex MCP source must be JSON or TOML"),
        }?;
        Ok(preserve_codex_plugin_identity(current, updated))
    }

    fn prepare_mcp_probe(&self, servers: &[McpServerRecord]) {
        #[cfg(target_os = "macos")]
        if servers.iter().any(|server| {
            matches!(
                server.transport.as_str(),
                "http" | "sse" | "streamable-http"
            )
        }) {
            prefetch_codex_mcp_keyring_tokens();
        }
        #[cfg(not(target_os = "macos"))]
        let _ = servers;
    }

    fn set_mcp_enabled(&self, request: &McpSetEnabledRequest) -> Result<()> {
        if is_codex_plugin_path(&request.path) {
            bail!(CODEX_PLUGIN_READ_ONLY_REASON);
        }
        match request.path.extension().and_then(|value| value.to_str()) {
            Some("toml") => {
                crate::mcp::set_toml_server_enabled(request, "mcp_servers", update_mcp_toml_server)
            }
            Some("json") => crate::mcp::set_json_server_enabled(
                request,
                &["mcpServers"],
                update_mcp_json_server,
            ),
            _ => bail!("Codex MCP source must be JSON or TOML"),
        }
    }

    fn backup_mcp_entry(&self, path: &Path, server_path: &[String], name: &str) -> Result<Value> {
        match path.extension().and_then(|value| value.to_str()) {
            Some("toml") => crate::mcp::read_toml_server_entry(path, "mcp_servers", name),
            Some("json") => crate::mcp::read_json_server_entry_at_path(path, server_path, name),
            _ => bail!("Codex MCP source must be JSON or TOML"),
        }
    }

    fn restore_mcp_entry(
        &self,
        path: &Path,
        server_path: &[String],
        name: &str,
        entry: &Value,
    ) -> Result<String> {
        match path.extension().and_then(|value| value.to_str()) {
            Some("toml") => crate::mcp::merge_toml_server_entry(path, "mcp_servers", name, entry),
            Some("json") => {
                crate::mcp::merge_json_server_entry_at_path(path, server_path, name, entry)
            }
            _ => bail!("Codex MCP source must be JSON or TOML"),
        }
    }

    fn mcp_status_after_toggle(&self, enabled: bool) -> &'static str {
        if enabled { "configured" } else { "disabled" }
    }

    fn delete_hooks(&self, requests: &[HookDeleteRequest], source: &str) -> Result<String> {
        match requests[0]
            .path
            .extension()
            .and_then(|value| value.to_str())
        {
            Some("json") => crate::hooks::delete_json_hooks(requests, source),
            Some("toml") => crate::hooks::delete_toml_hooks(requests, source),
            _ => bail!("Codex hook source must be JSON or TOML"),
        }
    }

    fn set_hook_enabled(&self, request: &HookSetEnabledRequest, source: &str) -> Result<String> {
        match request.path.extension().and_then(|value| value.to_str()) {
            Some("json") => crate::hooks::set_json_hook_enabled(request, source),
            Some("toml") => crate::hooks::set_toml_hook_enabled(request, source),
            _ => bail!("Codex hook source must be JSON or TOML"),
        }
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

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::{
        fs,
        path::PathBuf,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    use toml::Value as TomlValue;

    use crate::{
        mcp::{McpProbeState, McpServerRecord},
        providers::SessionMessageKind,
        skills::AgentKind,
    };

    use super::{
        AgentProvider, CODEX_MCP_ACCESS_TOKEN_CACHE, CODEX_PLUGIN_READ_ONLY_REASON,
        CodexMcpAccessTokenCacheKey, CodexMcpOAuthStore, CodexProvider, ProviderContext,
        SkillVisibility, codex_mcp_probe_env, codex_mcp_probe_timeout, codex_plugin_probe_spec,
        filter_codex_config_servers_shadowed_by_plugins, hook_current_hash,
        load_codex_mcp_access_token_cached, parse_codex_hook_file, preserve_codex_plugin_identity,
        render_codex_policy,
    };

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "tendi-codex-rules-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ))
    }

    #[test]
    fn selected_skill_message_is_context_and_emits_skill_evidence() {
        let value = json!({
            "type": "response_item",
            "timestamp": "2026-06-24T10:00:00Z",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "<skill>\n<name>foo</name>\n<path>/tmp/foo/SKILL.md</path>\n</skill>"
                }],
                "internal_chat_message_metadata_passthrough": {
                    "content_item_kinds": ["skills.selected_skill_instructions"]
                }
            }
        });

        assert_eq!(
            CodexProvider.session_message_kind(&value),
            Some(SessionMessageKind::Context)
        );
        let evidence = super::codex_selected_skill_candidate(&value).expect("skill evidence");
        assert_eq!(evidence.name.as_deref(), Some("foo"));
        assert_eq!(evidence.path.as_deref(), Some("/tmp/foo/SKILL.md"));

        let mut items = Vec::new();
        super::parse_transcript(&value, &mut items);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, "context");
        assert_eq!(items[0].tag.as_deref(), Some("Skill"));
    }

    #[test]
    fn skill_wrapper_without_metadata_is_context_and_emits_skill_evidence() {
        let value = json!({
            "type": "response_item",
            "timestamp": "2026-08-31T06:32:20.291Z",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "<skill>\n<name>datafinder</name>\n<path>/tmp/datafinder/SKILL.md</path>\n---\nname: datafinder\n---</skill>"
                }]
            }
        });

        assert_eq!(
            CodexProvider.session_message_kind(&value),
            Some(SessionMessageKind::Context)
        );
        let evidence = super::codex_selected_skill_candidate(&value).expect("skill evidence");
        assert_eq!(evidence.name.as_deref(), Some("datafinder"));
        assert_eq!(evidence.path.as_deref(), Some("/tmp/datafinder/SKILL.md"));

        let mut items = Vec::new();
        super::parse_transcript(&value, &mut items);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, "context");
        assert_eq!(items[0].tag.as_deref(), Some("Skill"));
    }

    #[test]
    fn scans_model_instructions_file_declared_by_codex_config() {
        let root = temp_dir();
        let home = root.join("home");
        let codex_home = home.join(".codex");
        let instructions = codex_home.join("instructions.md");
        fs::create_dir_all(&codex_home).expect("create Codex home");
        fs::write(
            codex_home.join("config.toml"),
            "model_instructions_file = \"instructions.md\"\n",
        )
        .expect("write Codex config");
        fs::write(&instructions, "custom Codex instructions").expect("write instructions");

        let context = ProviderContext {
            home: Some(home),
            project_dirs: Vec::new(),
        };
        let mut rules = Vec::new();
        let mut warnings = Vec::new();
        let mut order = 0;
        CodexProvider.scan_rules(&context, &mut rules, &mut warnings, &mut order);

        assert!(warnings.is_empty(), "warnings: {warnings:#?}");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].kind, "model-instructions-file");
        assert_eq!(rules[0].scope, "global");
        assert_eq!(rules[0].path, instructions);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_plugin_probe_inherits_runtime_environment_and_timeout() {
        let root = temp_dir();
        let codex_home = root.join(".codex");
        let plugin_root = codex_home.join("plugins/cache/openai-bundled/demo/1.0");
        fs::create_dir_all(&plugin_root).expect("create plugin root");
        fs::write(
            codex_home.join("config.toml"),
            "[mcp_servers.runtime]\ncommand = \"/tmp/node-repl\"\n\n[mcp_servers.runtime.env]\nNODE_REPL_NODE_PATH = \"/tmp/node\"\nNODE_REPL_NODE_MODULE_DIRS = \"/tmp/node-modules\"\n",
        )
        .expect("write Codex config");
        let spec = serde_json::json!({
            "command": "node",
            "args": ["scripts/launch.mjs"],
            "env_vars": ["PATH"],
            "startup_timeout_sec": 120
        });

        let env = codex_mcp_probe_env(&spec, Some(&plugin_root));
        assert_eq!(
            env.get("CODEX_HOME"),
            Some(&codex_home.display().to_string())
        );
        assert_eq!(
            env.get("CUA_REPL_NODE_REPL_PATH"),
            Some(&"/tmp/node-repl".to_string())
        );
        assert_eq!(
            env.get("NODE_REPL_NODE_MODULE_DIRS"),
            Some(&"/tmp/node-modules".to_string())
        );
        let path = std::env::var("PATH").expect("PATH is available in the test process");
        assert_eq!(env.get("PATH"), Some(&path));
        assert_eq!(
            codex_mcp_probe_timeout(&spec),
            Some(Duration::from_secs(120))
        );
        let normalized = codex_plugin_probe_spec(&spec, Some(&plugin_root));
        assert_eq!(normalized["command"], "/tmp/node");
        assert_eq!(normalized["cwd"], plugin_root.display().to_string());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_config_entry_is_shadowed_by_matching_plugin_server() {
        let server = |name: &str| McpServerRecord {
            agent: AgentKind::Codex,
            name: name.to_string(),
            scope: "global".to_string(),
            transport: "stdio".to_string(),
            enabled: false,
            status: "disabled".to_string(),
            path: PathBuf::new(),
            trust_hash: "hash".to_string(),
            probe_cache_version: 0,
            probe_state: McpProbeState::Unknown,
            server_path: vec!["mcp_servers".to_string()],
            read_only_reason: None,
            server_name: None,
            server_title: None,
            server_version: None,
            server_description: None,
            server_website_url: None,
            probe_error: None,
            icons: Vec::new(),
            tools: Vec::new(),
        };
        let mut config_servers = vec![server("plugin_server"), server("custom")];
        filter_codex_config_servers_shadowed_by_plugins(
            &mut config_servers,
            &[server("plugin_server")],
        );
        assert_eq!(
            config_servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
            vec!["custom"]
        );
    }

    #[test]
    fn codex_plugin_probe_keeps_manifest_identity() {
        let current = McpServerRecord {
            agent: AgentKind::Codex,
            name: "server".to_string(),
            scope: "global".to_string(),
            transport: "stdio".to_string(),
            enabled: true,
            status: "configured".to_string(),
            path: PathBuf::from("/tmp/.codex/plugins/cache/demo/server/1/.mcp.json"),
            trust_hash: "hash".to_string(),
            probe_cache_version: 0,
            probe_state: McpProbeState::Unknown,
            server_path: vec!["mcpServers".to_string()],
            read_only_reason: Some(CODEX_PLUGIN_READ_ONLY_REASON.to_string()),
            server_name: Some("server".to_string()),
            server_title: Some("Plugin".to_string()),
            server_version: Some("1".to_string()),
            server_description: Some("Plugin description".to_string()),
            server_website_url: Some("https://example.com".to_string()),
            probe_error: None,
            icons: Vec::new(),
            tools: Vec::new(),
        };
        let mut probed = current.clone();
        probed.server_name = Some("rmcp".to_string());
        probed.server_title = None;
        probed.server_version = Some("1.5.0".to_string());

        let normalized = preserve_codex_plugin_identity(&current, probed);

        assert_eq!(normalized.server_name.as_deref(), Some("server"));
        assert_eq!(normalized.server_title.as_deref(), Some("Plugin"));
        assert_eq!(normalized.server_version.as_deref(), Some("1"));
        assert_eq!(
            normalized.server_description.as_deref(),
            Some("Plugin description")
        );
    }

    #[test]
    fn scans_codex_session_end_hooks() {
        let root = temp_dir();
        let hooks_path = root.join("hooks.json");
        fs::create_dir_all(&root).expect("create hook root");
        fs::write(
            &hooks_path,
            r#"{
  "hooks": {
    "SessionEnd": [
      { "hooks": [{ "type": "command", "command": "cleanup" }] }
    ]
  }
}"#,
        )
        .expect("write SessionEnd hooks");

        let trust_hash = crate::fsutil::sha256_file(&hooks_path).expect("hash hooks");
        let mut hooks = Vec::new();
        let mut warnings = Vec::new();
        assert!(parse_codex_hook_file(
            &hooks_path,
            &trust_hash,
            &mut hooks,
            &mut warnings,
        ));

        assert!(warnings.is_empty(), "warnings: {warnings:?}");
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].event, "SessionEnd");
        assert_eq!(hooks[0].command.as_deref(), Some("cleanup"));
        assert_eq!(hooks[0].matcher, None);
        assert_eq!(
            hooks[0].provider_current_hash.as_deref(),
            hook_current_hash("SessionEnd", None, "cleanup", 1, false, None, None).as_deref()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_policy_update_preserves_unrelated_yaml_formatting() {
        let before = "interface:\n  display_name: \"Wait What\"\n  short_description: \"Re-pitch that — simpler, with the context I'm missing\"\npolicy:\n  allow_implicit_invocation: false\n";
        let expected = "interface:\n  display_name: \"Wait What\"\n  short_description: \"Re-pitch that — simpler, with the context I'm missing\"\npolicy:\n  allow_implicit_invocation: true\n";

        assert_eq!(
            render_codex_policy(Some(before), SkillVisibility::Auto).unwrap(),
            expected
        );
        assert_eq!(
            render_codex_policy(Some(expected), SkillVisibility::Manual).unwrap(),
            before
        );
    }

    #[test]
    fn codex_merge_normalizes_only_visibility_policy() {
        let base = "interface:\n  display_name: \"Better Typography\"\n  short_description: \"Web typography from fonts to spacing and wrapping\"\npolicy:\n  allow_implicit_invocation: true\n";
        let local = "interface:\n  display_name: \"Better Typography\"\n  short_description: \"Web typography from fonts to spacing and wrapping\"\npolicy:\n  allow_implicit_invocation: false\n";
        let incoming = "interface:\n  display_name: \"Better Typography\"\n  short_description: \"Fonts, type scales, spacing and wrapping\"\npolicy:\n  allow_implicit_invocation: true\n";

        let (normalized_local, normalized_base, normalized_incoming) = CodexProvider
            .normalize_skill_file_for_merge(
                "agents/openai.yaml",
                Some(local),
                Some(base),
                Some(incoming),
                SkillVisibility::Manual,
            )
            .expect("Codex provider file");

        assert_eq!(normalized_local, normalized_base);
        let normalized_incoming = normalized_incoming.expect("incoming Codex provider file");
        assert!(normalized_incoming.contains("Fonts, type scales, spacing and wrapping"));
        assert!(normalized_incoming.contains("allow_implicit_invocation: false"));
    }

    #[test]
    fn codex_policy_update_adds_missing_policy_without_reformatting_existing_yaml() {
        let before = "interface:\n  display_name: \"Wait What\"\n";
        let expected = "interface:\n  display_name: \"Wait What\"\npolicy:\n  allow_implicit_invocation: false\n";

        assert_eq!(
            render_codex_policy(Some(before), SkillVisibility::Manual).unwrap(),
            expected
        );
    }

    #[test]
    fn codex_skill_config_update_preserves_crlf_line_endings() {
        let before = "# keep\r\n[other]\r\nvalue = 'keep'\r\n";
        let after = super::render_codex_skill_config(
            before,
            std::path::Path::new("/tmp/demo/SKILL.md"),
            true,
        )
        .unwrap();

        assert!(after.contains("value = 'keep'\r\n"));
        assert!(after.contains("enabled = true\r\n"));
        assert!(!after.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn codex_skill_config_migration_removes_stale_legacy_entry() {
        let root = temp_dir();
        let legacy_root = root.join(".codex/skills");
        let canonical_root = root.join(".agents/skills");
        let legacy_file = legacy_root.join("bugfix-loop/SKILL.md");
        let canonical_file = canonical_root.join("bugfix-loop/SKILL.md");
        fs::create_dir_all(canonical_file.parent().expect("canonical parent"))
            .expect("create canonical skill");
        fs::write(&canonical_file, "---\nname: bugfix-loop\n---\n").expect("write skill");
        let before = format!(
            "[[skills.config]]\npath = {:?}\nenabled = true\n\n[[skills.config]]\npath = {:?}\nenabled = false\n",
            legacy_file.display().to_string(),
            canonical_file.display().to_string(),
        );

        let after = super::migrate_codex_skill_config_paths(&before, &legacy_root, &canonical_root)
            .expect("migrate Codex skill config");
        let value = toml::from_str::<TomlValue>(&after).expect("parse migrated config");
        let configs = value
            .get("skills")
            .and_then(|skills| skills.get("config"))
            .and_then(TomlValue::as_array)
            .expect("migrated skill config entries");
        let canonical_path = canonical_file.to_string_lossy().into_owned();
        assert_eq!(configs.len(), 1);
        assert_eq!(
            configs[0].get("path").and_then(TomlValue::as_str),
            Some(canonical_path.as_str())
        );
        assert_eq!(
            configs[0].get("enabled").and_then(TomlValue::as_bool),
            Some(false)
        );
        assert!(!after.contains(legacy_file.to_string_lossy().as_ref()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reuses_codex_file_mcp_oauth_token_for_matching_server() {
        let root = temp_dir();
        let codex_home = root.join(".codex");
        fs::create_dir_all(&codex_home).expect("create Codex home");
        fs::write(
            codex_home.join(".credentials.json"),
            r#"{
  "figma|credential-key": {
    "server_name": "figma",
    "server_url": "https://mcp.figma.com/mcp",
    "client_id": "client",
    "access_token": "access-token"
  }
}"#,
        )
        .expect("write OAuth credentials");

        assert_eq!(
            super::load_codex_mcp_file_token_from_home(
                &codex_home,
                "figma",
                "https://mcp.figma.com/mcp",
            )
            .as_deref(),
            Some("access-token")
        );
        assert!(
            super::load_codex_mcp_file_token_from_home(
                &codex_home,
                "other",
                "https://mcp.figma.com/mcp",
            )
            .is_none()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn caches_codex_mcp_access_token_lookup_result() {
        let key = CodexMcpAccessTokenCacheKey {
            store: CodexMcpOAuthStore::Keyring,
            name: format!("cache-test-{}", std::process::id()),
            url: "https://cache-test.invalid/mcp".to_string(),
        };
        CODEX_MCP_ACCESS_TOKEN_CACHE
            .lock()
            .expect("lock Codex MCP credential cache")
            .remove(&key);

        let lookups = std::cell::Cell::new(0);
        let first = load_codex_mcp_access_token_cached(key.clone(), || {
            lookups.set(lookups.get() + 1);
            Some("cached-token".to_string())
        });
        let second = load_codex_mcp_access_token_cached(key.clone(), || {
            lookups.set(lookups.get() + 1);
            Some("unexpected-second-token".to_string())
        });

        assert_eq!(first.as_deref(), Some("cached-token"));
        assert_eq!(second.as_deref(), Some("cached-token"));
        assert_eq!(lookups.get(), 1);
        CODEX_MCP_ACCESS_TOKEN_CACHE
            .lock()
            .expect("lock Codex MCP credential cache")
            .remove(&key);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parses_codex_mcp_keyring_token_from_batch_item() {
        let serialized = serde_json::json!({
            "server_name": "figma",
            "url": "https://mcp.figma.com/mcp",
            "token_response": { "access_token": "access-token" }
        })
        .to_string();

        assert_eq!(
            super::parse_codex_mcp_keyring_token(
                "figma",
                "https://mcp.figma.com/mcp",
                &serialized,
            )
            .as_deref(),
            Some("access-token")
        );
    }
}
