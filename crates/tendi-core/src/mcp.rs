use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use anyhow::Result;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use toml::Value as TomlValue;
use toml_edit::{DocumentMut, Item, Table, TableLike, Value as EditValue};
use walkdir::WalkDir;

use crate::{
    fsutil::{atomic_write, sha256_text},
    skills::AgentKind,
};

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpProbeState {
    #[default]
    Unknown,
    Ready,
    ReadyEmpty,
    NeedsAuth,
    Failed,
}

impl McpProbeState {
    fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

// Bumped when provider-owned MCP probe or enrichment semantics change, so
// persisted rows cannot mask the updated projection behind stale metadata.
pub const MCP_PROBE_CACHE_VERSION: u8 = 8;

fn is_zero_probe_cache_version(value: &u8) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerRecord {
    pub agent: AgentKind,
    pub name: String,
    pub scope: String,
    pub transport: String,
    pub enabled: bool,
    pub status: String,
    pub path: PathBuf,
    pub trust_hash: String,
    #[serde(default, skip_serializing_if = "is_zero_probe_cache_version")]
    pub probe_cache_version: u8,
    #[serde(default, skip_serializing_if = "McpProbeState::is_unknown")]
    pub probe_state: McpProbeState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub server_path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_website_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub icons: Vec<McpIcon>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<McpTool>,
}

/// Returns the stable identity of an MCP server record.
///
/// The JSON tuple deliberately contains only the provider, configured server
/// name, source file, and nested server path. It matches the existing
/// desktop row-key encoding so callers can adopt this core-owned ID
/// without changing persisted locators. Mutable configuration and probe
/// metadata are intentionally excluded.
pub fn mcp_server_id(server: &McpServerRecord) -> String {
    serde_json::to_string(&(
        server.agent,
        &server.name,
        server.path.to_string_lossy(),
        &server.server_path,
    ))
    .expect("MCP server identity fields must be serializable")
}

/// Returns whether `id` identifies `server`.
pub fn mcp_server_matches_id(server: &McpServerRecord, id: &str) -> bool {
    !id.trim().is_empty() && mcp_server_id(server) == id
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpIcon {
    pub src: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sizes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub icons: Vec<McpIcon>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct McpEnrichment {
    pub(crate) plugin_name: Option<String>,
    pub(crate) server_name: Option<String>,
    pub(crate) server_title: Option<String>,
    pub(crate) server_version: Option<String>,
    pub(crate) server_description: Option<String>,
    pub(crate) server_website_url: Option<String>,
    pub(crate) icons: Vec<McpIcon>,
    pub(crate) tools: Vec<McpTool>,
    pub(crate) needs_login: bool,
    pub(crate) probe_succeeded: bool,
    pub(crate) probe_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct McpConnectionSpec {
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub url: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub transport: String,
    pub timeout: Duration,
}

impl PartialEq for McpConnectionSpec {
    fn eq(&self, other: &Self) -> bool {
        self.command == other.command
            && self.args == other.args
            && self.cwd == other.cwd
            && self.env == other.env
            && self.url == other.url
            && self.headers == other.headers
            && self.transport == other.transport
            && self.timeout == other.timeout
    }
}

impl Eq for McpConnectionSpec {}

impl std::hash::Hash for McpConnectionSpec {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.command.hash(state);
        self.args.hash(state);
        self.cwd.hash(state);
        self.env.hash(state);
        self.url.hash(state);
        self.headers.hash(state);
        self.transport.hash(state);
        self.timeout.hash(state);
    }
}

pub(crate) struct McpProbeCache {
    enrichments: HashMap<McpConnectionSpec, McpEnrichment>,
    allow_probe: bool,
}

impl Default for McpProbeCache {
    fn default() -> Self {
        Self::metadata_only()
    }
}

impl McpProbeCache {
    pub(crate) fn explicit() -> Self {
        Self {
            enrichments: HashMap::new(),
            allow_probe: true,
        }
    }

    pub(crate) fn metadata_only() -> Self {
        Self {
            enrichments: HashMap::new(),
            allow_probe: false,
        }
    }

    pub(crate) fn allows_probe(&self) -> bool {
        self.allow_probe
    }

    fn enrich(&mut self, connection: &McpConnectionSpec) -> McpEnrichment {
        if !self.allow_probe {
            return McpEnrichment::default();
        }
        if let Some(enrichment) = self.enrichments.get(connection) {
            return enrichment.clone();
        }

        let enrichment = probe_mcp(connection);
        self.enrichments
            .insert(connection.clone(), enrichment.clone());
        enrichment
    }
}

const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const MCP_PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const MCP_MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
const MCP_MAX_TOOL_PAGES: usize = 32;

#[derive(Debug)]
struct McpAuthenticationRequired;

impl std::fmt::Display for McpAuthenticationRequired {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MCP server authentication is required")
    }
}

impl std::error::Error for McpAuthenticationRequired {}

fn json_string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|map| map.iter())
        .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
        .collect()
}

fn probe_transport(command: Option<&str>, transport: &str) -> String {
    if command.is_some() {
        "stdio".to_string()
    } else if transport == "sse" {
        "sse".to_string()
    } else {
        "http".to_string()
    }
}

pub(crate) fn connection_spec_from_json_with_base_dir(
    spec: &Value,
    transport: &str,
    base_dir: Option<&Path>,
) -> Option<McpConnectionSpec> {
    let command = spec
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let url = spec
        .get("url")
        .or_else(|| spec.get("serverUrl"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if command.is_none() && url.is_none() {
        return None;
    }
    let args = spec
        .get("args")
        .and_then(Value::as_array)
        .map(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let transport = probe_transport(command.as_deref(), transport);
    Some(McpConnectionSpec {
        command,
        args,
        cwd: json_path(spec.get("cwd"), base_dir),
        env: json_string_map(spec.get("env")),
        url,
        headers: json_string_map(spec.get("headers")),
        transport,
        timeout: MCP_PROBE_TIMEOUT,
    })
}

pub(crate) fn connection_spec_from_toml_with_base_dir(
    spec: &TomlValue,
    transport: &str,
    base_dir: Option<&Path>,
) -> Option<McpConnectionSpec> {
    let command = spec
        .get("command")
        .and_then(TomlValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let url = spec
        .get("url")
        .or_else(|| spec.get("server_url"))
        .and_then(TomlValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if command.is_none() && url.is_none() {
        return None;
    }
    let args = spec
        .get("args")
        .and_then(TomlValue::as_array)
        .map(|args| {
            args.iter()
                .filter_map(TomlValue::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let env = spec
        .get("env")
        .and_then(TomlValue::as_table)
        .into_iter()
        .flat_map(|map| map.iter())
        .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
        .collect();
    let headers = spec
        .get("headers")
        .and_then(TomlValue::as_table)
        .into_iter()
        .flat_map(|map| map.iter())
        .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
        .collect();
    let transport = probe_transport(command.as_deref(), transport);
    Some(McpConnectionSpec {
        command,
        args,
        cwd: toml_path(spec.get("cwd"), base_dir),
        env,
        url,
        headers,
        transport,
        timeout: MCP_PROBE_TIMEOUT,
    })
}

fn resolve_path(value: &str, base_dir: Option<&Path>) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base_dir.map_or(path.clone(), |base| base.join(path))
    }
}

fn json_path(value: Option<&Value>, base_dir: Option<&Path>) -> Option<PathBuf> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| resolve_path(value, base_dir))
}

fn toml_path(value: Option<&TomlValue>, base_dir: Option<&Path>) -> Option<PathBuf> {
    value
        .and_then(TomlValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| resolve_path(value, base_dir))
}

#[cfg(test)]
pub(crate) fn enrich_json_mcp_spec(
    spec: &Value,
    transport: &str,
    _enabled: bool,
    probe_cache: &mut McpProbeCache,
) -> McpEnrichment {
    enrich_json_mcp_spec_with_headers(spec, transport, _enabled, &BTreeMap::new(), probe_cache)
}

#[cfg(test)]
pub(crate) fn enrich_json_mcp_spec_with_headers(
    spec: &Value,
    transport: &str,
    _enabled: bool,
    extra_headers: &BTreeMap<String, String>,
    probe_cache: &mut McpProbeCache,
) -> McpEnrichment {
    enrich_json_mcp_spec_with_headers_at_dir(
        spec,
        transport,
        _enabled,
        extra_headers,
        None,
        probe_cache,
    )
}

pub(crate) fn enrich_json_mcp_spec_with_headers_at_dir(
    spec: &Value,
    transport: &str,
    _enabled: bool,
    extra_headers: &BTreeMap<String, String>,
    base_dir: Option<&Path>,
    probe_cache: &mut McpProbeCache,
) -> McpEnrichment {
    enrich_json_mcp_spec_with_headers_at_dir_and_options(
        spec,
        transport,
        _enabled,
        extra_headers,
        &BTreeMap::new(),
        None,
        base_dir,
        probe_cache,
    )
}

pub(crate) fn enrich_json_mcp_spec_with_headers_at_dir_and_options(
    spec: &Value,
    transport: &str,
    _enabled: bool,
    extra_headers: &BTreeMap<String, String>,
    extra_env: &BTreeMap<String, String>,
    timeout: Option<Duration>,
    base_dir: Option<&Path>,
    probe_cache: &mut McpProbeCache,
) -> McpEnrichment {
    if !probe_cache.allows_probe() {
        return McpEnrichment::default();
    }
    connection_spec_from_json_with_base_dir(spec, transport, base_dir)
        .map(|mut connection| {
            merge_probe_headers(&mut connection.headers, extra_headers);
            merge_probe_env(&mut connection.env, extra_env);
            if let Some(timeout) = timeout {
                connection.timeout = timeout;
            }
            probe_cache.enrich(&connection)
        })
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) fn enrich_toml_mcp_spec(
    spec: &TomlValue,
    transport: &str,
    _enabled: bool,
    probe_cache: &mut McpProbeCache,
) -> McpEnrichment {
    enrich_toml_mcp_spec_with_headers(spec, transport, _enabled, &BTreeMap::new(), probe_cache)
}

#[cfg(test)]
pub(crate) fn enrich_toml_mcp_spec_with_headers(
    spec: &TomlValue,
    transport: &str,
    _enabled: bool,
    extra_headers: &BTreeMap<String, String>,
    probe_cache: &mut McpProbeCache,
) -> McpEnrichment {
    enrich_toml_mcp_spec_with_headers_at_dir(
        spec,
        transport,
        _enabled,
        extra_headers,
        None,
        probe_cache,
    )
}

#[cfg(test)]
pub(crate) fn enrich_toml_mcp_spec_with_headers_at_dir(
    spec: &TomlValue,
    transport: &str,
    _enabled: bool,
    extra_headers: &BTreeMap<String, String>,
    base_dir: Option<&Path>,
    probe_cache: &mut McpProbeCache,
) -> McpEnrichment {
    enrich_toml_mcp_spec_with_headers_at_dir_and_options(
        spec,
        transport,
        _enabled,
        extra_headers,
        &BTreeMap::new(),
        None,
        base_dir,
        probe_cache,
    )
}

pub(crate) fn enrich_toml_mcp_spec_with_headers_at_dir_and_options(
    spec: &TomlValue,
    transport: &str,
    _enabled: bool,
    extra_headers: &BTreeMap<String, String>,
    extra_env: &BTreeMap<String, String>,
    timeout: Option<Duration>,
    base_dir: Option<&Path>,
    probe_cache: &mut McpProbeCache,
) -> McpEnrichment {
    if !probe_cache.allows_probe() {
        return McpEnrichment::default();
    }
    connection_spec_from_toml_with_base_dir(spec, transport, base_dir)
        .map(|mut connection| {
            merge_probe_headers(&mut connection.headers, extra_headers);
            merge_probe_env(&mut connection.env, extra_env);
            if let Some(timeout) = timeout {
                connection.timeout = timeout;
            }
            probe_cache.enrich(&connection)
        })
        .unwrap_or_default()
}

fn merge_probe_headers(
    headers: &mut BTreeMap<String, String>,
    extra_headers: &BTreeMap<String, String>,
) {
    for (name, value) in extra_headers {
        if headers
            .keys()
            .any(|existing| existing.eq_ignore_ascii_case(name))
        {
            continue;
        }
        headers.insert(name.clone(), value.clone());
    }
}

fn merge_probe_env(env: &mut BTreeMap<String, String>, extra_env: &BTreeMap<String, String>) {
    for (name, value) in extra_env {
        env.entry(name.clone()).or_insert_with(|| value.clone());
    }
}

fn valid_icon_src(src: &str) -> bool {
    if src.starts_with("http://") || src.starts_with("https://") {
        return true;
    }
    let Some(image) = src.strip_prefix("data:image/") else {
        return false;
    };
    let Some((metadata, _data)) = image.split_once(',') else {
        return false;
    };
    let mime_type = metadata
        .split(';')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        mime_type.as_str(),
        "png" | "jpeg" | "jpg" | "svg+xml" | "webp"
    )
}

pub(crate) fn mcp_icon_from_file(path: &Path) -> Option<McpIcon> {
    let bytes = fs::read(path).ok()?;
    let extension = path.extension().and_then(|value| value.to_str())?;
    let mime_type = match extension.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => return None,
    };
    Some(McpIcon {
        src: format!(
            "data:{mime_type};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ),
        mime_type: Some(mime_type.to_string()),
        sizes: vec!["any".to_string()],
        theme: None,
    })
}

fn parse_icons(value: Option<&Value>) -> Vec<McpIcon> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flat_map(|icons| icons.iter())
        .filter_map(|value| {
            let object = value.as_object()?;
            let src = object.get("src")?.as_str()?.trim();
            if !valid_icon_src(src) {
                return None;
            }
            Some(McpIcon {
                src: src.to_string(),
                mime_type: object
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                sizes: object
                    .get("sizes")
                    .and_then(Value::as_array)
                    .map(|sizes| {
                        sizes
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                theme: object
                    .get("theme")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .take(4)
        .collect()
}

fn parse_tools(value: Option<&Value>) -> Vec<McpTool> {
    value
        .and_then(|value| value.get("tools"))
        .and_then(Value::as_array)
        .into_iter()
        .flat_map(|tools| tools.iter())
        .filter_map(|value| {
            let object = value.as_object()?;
            let name = object.get("name")?.as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            Some(McpTool {
                name: name.to_string(),
                title: object
                    .get("title")
                    .or_else(|| {
                        object
                            .get("annotations")
                            .and_then(Value::as_object)
                            .and_then(|annotations| annotations.get("title"))
                    })
                    .and_then(Value::as_str)
                    .map(str::to_string),
                description: object
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                input_schema: object
                    .get("inputSchema")
                    .or_else(|| object.get("input_schema"))
                    .cloned(),
                icons: parse_icons(object.get("icons")),
            })
        })
        .take(512)
        .collect()
}

fn enrichment_from_initialize(value: &Value) -> McpEnrichment {
    let info = value
        .get("result")
        .and_then(|value| value.get("serverInfo"));
    McpEnrichment {
        plugin_name: None,
        server_name: info
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        server_title: info
            .and_then(|value| value.get("title"))
            .and_then(Value::as_str)
            .map(str::to_string),
        server_version: info
            .and_then(|value| value.get("version"))
            .and_then(Value::as_str)
            .map(str::to_string),
        server_description: info
            .and_then(|value| value.get("description"))
            .and_then(Value::as_str)
            .map(str::to_string),
        server_website_url: info
            .and_then(|value| value.get("websiteUrl"))
            .and_then(Value::as_str)
            .map(str::to_string),
        icons: parse_icons(info.and_then(|value| value.get("icons"))),
        tools: Vec::new(),
        needs_login: false,
        probe_succeeded: false,
        probe_error: None,
    }
}

fn rpc_request(id: u64, method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
}

fn rpc_notification(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    })
}

fn initialize_request() -> Value {
    rpc_request(
        1,
        "initialize",
        json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "tendi", "version": env!("CARGO_PKG_VERSION") },
        }),
    )
}

fn tools_list_request(cursor: Option<&str>) -> Value {
    rpc_request(
        2,
        "tools/list",
        cursor.map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor })),
    )
}

fn response_matches(value: &Value, id: u64) -> bool {
    value.get("id").and_then(Value::as_u64) == Some(id)
        && value.get("error").is_none()
        && value.get("result").is_some()
}

fn response_error(value: &Value, id: u64) -> Option<String> {
    if value.get("id").and_then(Value::as_u64) != Some(id) {
        return None;
    }
    value.get("error").map(|error| {
        error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| error.to_string())
    })
}

fn initialize_declares_no_tools(value: &Value) -> bool {
    value
        .pointer("/result/capabilities")
        .and_then(Value::as_object)
        .is_some_and(|capabilities| !capabilities.contains_key("tools"))
}

#[cfg(test)]
fn enrichment_from_responses(initialize: &Value, tools: Option<&Value>) -> McpEnrichment {
    let mut enrichment = enrichment_from_initialize(initialize);
    if let Some(tools) = tools {
        append_tools_page(&mut enrichment, tools);
    }
    enrichment
}

fn append_tools_page(enrichment: &mut McpEnrichment, value: &Value) -> Option<String> {
    let remaining = 512usize.saturating_sub(enrichment.tools.len());
    enrichment
        .tools
        .extend(parse_tools(value.get("result")).into_iter().take(remaining));
    value
        .get("result")
        .and_then(|result| result.get("nextCursor"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|cursor| !cursor.is_empty())
}

fn read_limited_response(response: ureq::Response) -> Result<String> {
    let mut body = String::new();
    response
        .into_reader()
        .take(MCP_MAX_RESPONSE_BYTES)
        .read_to_string(&mut body)?;
    Ok(body)
}

fn response_value(body: &str) -> Result<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(body.trim()) {
        return Ok(value);
    }
    for line in body.lines().rev() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        if let Ok(value) = serde_json::from_str::<Value>(data.trim()) {
            return Ok(value);
        }
    }
    anyhow::bail!("MCP response did not contain a JSON-RPC result")
}

fn http_request(
    agent: &ureq::Agent,
    spec: &McpConnectionSpec,
    session_id: Option<&str>,
    request: &Value,
) -> Result<(Option<Value>, Option<String>)> {
    let url = spec
        .url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("MCP HTTP URL is missing"))?;
    let mut builder = agent
        .post(url)
        .set("Accept", "application/json, text/event-stream")
        .set("Content-Type", "application/json")
        .set("MCP-Protocol-Version", MCP_PROTOCOL_VERSION);
    for (key, value) in &spec.headers {
        builder = builder.set(key, value);
    }
    if let Some(session_id) = session_id {
        builder = builder.set("Mcp-Session-Id", session_id);
    }
    match builder.send_string(&serde_json::to_string(request)?) {
        Ok(response) => {
            let response_session_id = response.header("Mcp-Session-Id").map(str::to_string);
            let body = read_limited_response(response)?;
            if body.trim().is_empty() {
                Ok((None, response_session_id))
            } else {
                Ok((Some(response_value(&body)?), response_session_id))
            }
        }
        Err(ureq::Error::Status(status, response)) => {
            if status == 401 {
                return Err(anyhow::Error::new(McpAuthenticationRequired));
            }
            anyhow::bail!(
                "MCP HTTP request failed with status {status}: {}",
                response.status_text()
            )
        }
        Err(error) => Err(error.into()),
    }
}

fn probe_http(spec: &McpConnectionSpec) -> Result<McpEnrichment> {
    let agent = ureq::AgentBuilder::new().timeout(spec.timeout).build();
    let (initialize, initial_session_id) = http_request(&agent, spec, None, &initialize_request())?;
    let initialize =
        initialize.ok_or_else(|| anyhow::anyhow!("MCP initialize response is empty"))?;
    let mut session_id = initial_session_id;
    let _ = http_request(
        &agent,
        spec,
        session_id.as_deref(),
        &rpc_notification("notifications/initialized", json!({})),
    );
    let mut enrichment = enrichment_from_initialize(&initialize);
    let mut cursor = None;
    for _ in 0..MCP_MAX_TOOL_PAGES {
        let (tools, response_session_id) = http_request(
            &agent,
            spec,
            session_id.as_deref(),
            &tools_list_request(cursor.as_deref()),
        )?;
        if let Some(tools) = tools.as_ref() {
            if let Some(error) = response_error(tools, 2) {
                anyhow::bail!("MCP response failed: {error}")
            }
        }
        if let Some(response_session_id) = response_session_id {
            session_id = Some(response_session_id);
        }
        let Some(tools) = tools else {
            break;
        };
        let next_cursor = append_tools_page(&mut enrichment, &tools);
        if next_cursor.as_deref() == cursor.as_deref() {
            break;
        }
        let Some(next_cursor) = next_cursor else {
            break;
        };
        cursor = Some(next_cursor);
    }
    Ok(enrichment)
}

enum SseEvent {
    Endpoint(String),
    Message(Value),
}

fn read_sse_event(reader: &mut BufReader<impl Read>) -> Result<Option<SseEvent>> {
    let mut event_name = None;
    let mut data = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if data.is_empty() {
                continue;
            }
            let body = data.join("\n");
            return if event_name.as_deref() == Some("endpoint") {
                Ok(Some(SseEvent::Endpoint(body)))
            } else {
                Ok(Some(SseEvent::Message(serde_json::from_str(&body)?)))
            };
        }
        if let Some(value) = line.strip_prefix("event:") {
            event_name = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.trim_start().to_string());
        }
    }
}

fn resolve_sse_endpoint(base: &str, endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return endpoint.to_string();
    }
    let Some(scheme_end) = base.find("://") else {
        return endpoint.to_string();
    };
    let authority_start = scheme_end + 3;
    let authority_end = base[authority_start..]
        .find('/')
        .map(|offset| authority_start + offset)
        .unwrap_or(base.len());
    if endpoint.starts_with('/') {
        return format!("{}{}", &base[..authority_end], endpoint);
    }
    let base_directory = if authority_end == base.len() {
        format!("{base}/")
    } else {
        base[..base.rfind('/').unwrap_or(authority_end) + 1].to_string()
    };
    format!("{base_directory}{endpoint}")
}

fn post_sse_request(
    agent: &ureq::Agent,
    spec: &McpConnectionSpec,
    endpoint: &str,
    request: &Value,
) -> Result<()> {
    let mut builder = agent
        .post(endpoint)
        .set("Accept", "application/json, text/event-stream")
        .set("Content-Type", "application/json")
        .set("MCP-Protocol-Version", MCP_PROTOCOL_VERSION);
    for (key, value) in &spec.headers {
        builder = builder.set(key, value);
    }
    match builder.send_string(&serde_json::to_string(request)?) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, response)) => {
            if status == 401 {
                return Err(anyhow::Error::new(McpAuthenticationRequired));
            }
            anyhow::bail!(
                "MCP SSE request failed with status {status}: {}",
                response.status_text()
            )
        }
        Err(error) => Err(error.into()),
    }
}

fn receive_sse_response(
    receiver: &mpsc::Receiver<Result<Option<SseEvent>>>,
    id: u64,
    timeout: Duration,
) -> Result<Value> {
    loop {
        let event = receiver
            .recv_timeout(timeout)
            .map_err(|_| anyhow::anyhow!("MCP SSE probe timed out"))??;
        let Some(event) = event else {
            anyhow::bail!("MCP SSE server closed its event stream")
        };
        if let SseEvent::Message(value) = event {
            if let Some(error) = response_error(&value, id) {
                anyhow::bail!("MCP response failed: {error}")
            }
            if response_matches(&value, id) {
                return Ok(value);
            }
        }
    }
}

fn probe_sse(spec: &McpConnectionSpec) -> Result<McpEnrichment> {
    let url = spec
        .url
        .clone()
        .ok_or_else(|| anyhow::anyhow!("MCP SSE URL is missing"))?;
    let headers = spec.headers.clone();
    let agent = ureq::AgentBuilder::new().timeout(spec.timeout).build();
    let (sender, receiver) = mpsc::channel();
    let stream_agent = agent.clone();
    thread::spawn(move || {
        let mut request = stream_agent.get(&url).set("Accept", "text/event-stream");
        for (key, value) in headers {
            request = request.set(&key, &value);
        }
        let result = (|| {
            let response = match request.call() {
                Ok(response) => response,
                Err(ureq::Error::Status(status, _response)) if status == 401 => {
                    return Err(anyhow::Error::new(McpAuthenticationRequired));
                }
                Err(error) => return Err(error.into()),
            };
            let mut reader = BufReader::new(response.into_reader());
            loop {
                match read_sse_event(&mut reader) {
                    Ok(Some(event)) => {
                        if sender.send(Ok(Some(event))).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ = sender.send(Ok(None));
                        break;
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
            Ok::<(), anyhow::Error>(())
        })();
        if let Err(error) = result {
            let _ = sender.send(Err(error));
        }
    });
    let endpoint = loop {
        let event = receiver
            .recv_timeout(spec.timeout)
            .map_err(|_| anyhow::anyhow!("MCP SSE endpoint discovery timed out"))??;
        let Some(event) = event else {
            anyhow::bail!("MCP SSE server closed before publishing its endpoint")
        };
        if let SseEvent::Endpoint(endpoint) = event {
            break resolve_sse_endpoint(&spec.url.clone().unwrap_or_default(), &endpoint);
        }
    };
    post_sse_request(&agent, spec, &endpoint, &initialize_request())?;
    let initialize = receive_sse_response(&receiver, 1, spec.timeout)?;
    post_sse_request(
        &agent,
        spec,
        &endpoint,
        &rpc_notification("notifications/initialized", json!({})),
    )?;
    let mut enrichment = enrichment_from_initialize(&initialize);
    if initialize_declares_no_tools(&initialize) {
        return Ok(enrichment);
    }
    let mut cursor = None;
    for _ in 0..MCP_MAX_TOOL_PAGES {
        post_sse_request(
            &agent,
            spec,
            &endpoint,
            &tools_list_request(cursor.as_deref()),
        )?;
        let tools = receive_sse_response(&receiver, 2, spec.timeout)?;
        let next_cursor = append_tools_page(&mut enrichment, &tools);
        if next_cursor.as_deref() == cursor.as_deref() {
            break;
        }
        let Some(next_cursor) = next_cursor else {
            break;
        };
        cursor = Some(next_cursor);
    }
    Ok(enrichment)
}

#[derive(Clone, Copy, Debug)]
enum StdioMessageFormat {
    JsonLines,
    ContentLength,
}

fn write_stdio_json(
    stdin: &mut ChildStdin,
    value: &Value,
    format: StdioMessageFormat,
) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    match format {
        StdioMessageFormat::JsonLines => {
            stdin.write_all(&body)?;
            stdin.write_all(b"\n")?;
        }
        StdioMessageFormat::ContentLength => {
            write!(stdin, "Content-Length: {}\r\n\r\n", body.len())?;
            stdin.write_all(&body)?;
        }
    }
    stdin.flush()?;
    Ok(())
}

fn read_stdio_json(reader: &mut BufReader<impl Read>) -> Result<Option<Value>> {
    let mut first_line = String::new();
    loop {
        first_line.clear();
        if reader.read_line(&mut first_line)? == 0 {
            return Ok(None);
        }
        if !first_line.trim().is_empty() {
            break;
        }
    }
    if first_line.trim_start().starts_with('{') {
        return Ok(Some(serde_json::from_str(first_line.trim())?));
    }
    let Some(length) = first_line
        .strip_prefix("Content-Length:")
        .and_then(|value| value.trim().parse::<usize>().ok())
    else {
        anyhow::bail!("MCP stdio response has no Content-Length")
    };
    let mut header = String::new();
    loop {
        header.clear();
        reader.read_line(&mut header)?;
        if header.trim().is_empty() {
            break;
        }
    }
    if length > MCP_MAX_RESPONSE_BYTES as usize {
        anyhow::bail!("MCP stdio response is too large")
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body)?))
}

fn receive_stdio_response(
    receiver: &mpsc::Receiver<Result<Option<Value>>>,
    request_id: u64,
    timeout: Duration,
) -> Result<Value> {
    loop {
        let value = receiver
            .recv_timeout(timeout)
            .map_err(|_| anyhow::anyhow!("MCP stdio probe timed out"))??
            .ok_or_else(|| anyhow::anyhow!("MCP stdio server closed its output"))?;
        if response_matches(&value, request_id) {
            return Ok(value);
        }
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| error.to_string());
            anyhow::bail!("MCP response failed: {message}")
        }
    }
}

fn probe_stdio_once(spec: &McpConnectionSpec, format: StdioMessageFormat) -> Result<McpEnrichment> {
    let command = spec
        .command
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("MCP stdio command is missing"))?;
    let mut command = Command::new(command);
    command.args(&spec.args).envs(&spec.env);
    if let Some(cwd) = spec.cwd.as_deref() {
        command.current_dir(cwd);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("MCP stdio stdin is unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("MCP stdio stdout is unavailable"))?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_stdio_json(&mut reader) {
                Ok(Some(value)) => {
                    if sender.send(Ok(Some(value))).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = sender.send(Ok(None));
                    break;
                }
                Err(error) => {
                    let _ = sender.send(Err(error));
                    break;
                }
            }
        }
    });
    let result = (|| {
        write_stdio_json(&mut stdin, &initialize_request(), format)?;
        let initialize = receive_stdio_response(&receiver, 1, spec.timeout)?;
        write_stdio_json(
            &mut stdin,
            &rpc_notification("notifications/initialized", json!({})),
            format,
        )?;
        let mut enrichment = enrichment_from_initialize(&initialize);
        if initialize_declares_no_tools(&initialize) {
            return Ok(enrichment);
        }
        let mut cursor = None;
        for _ in 0..MCP_MAX_TOOL_PAGES {
            write_stdio_json(&mut stdin, &tools_list_request(cursor.as_deref()), format)?;
            let tools = receive_stdio_response(&receiver, 2, spec.timeout)?;
            let next_cursor = append_tools_page(&mut enrichment, &tools);
            if next_cursor.as_deref() == cursor.as_deref() {
                break;
            }
            let Some(next_cursor) = next_cursor else {
                break;
            };
            cursor = Some(next_cursor);
        }
        Ok(enrichment)
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn probe_stdio(spec: &McpConnectionSpec) -> Result<McpEnrichment> {
    match probe_stdio_once(spec, StdioMessageFormat::JsonLines) {
        Ok(enrichment) => Ok(enrichment),
        Err(json_lines_error) => probe_stdio_once(spec, StdioMessageFormat::ContentLength).map_err(
            |content_length_error| {
                anyhow::anyhow!(
                    "MCP stdio probe failed with JSON Lines ({json_lines_error}); Content-Length fallback failed ({content_length_error})"
                )
            },
        ),
    }
}

fn probe_connection(spec: &McpConnectionSpec) -> Result<McpEnrichment> {
    if spec.command.is_some() {
        return probe_stdio(spec);
    }
    if spec.transport == "sse" {
        return probe_sse(spec);
    }
    probe_http(spec)
}

fn probe_mcp(spec: &McpConnectionSpec) -> McpEnrichment {
    match probe_connection(spec) {
        Ok(mut enrichment) => {
            enrichment.probe_succeeded = true;
            enrichment
        }
        Err(error) => {
            crate::logging::global().warn(
                "MCP connection probe failed",
                json!({
                    "transport": spec.transport,
                    "command": spec.command,
                    "error": error.to_string(),
                }),
            );
            McpEnrichment {
                needs_login: error.downcast_ref::<McpAuthenticationRequired>().is_some(),
                probe_error: Some(error.to_string()),
                ..McpEnrichment::default()
            }
        }
    }
}

fn mcp_status_for_enrichment(status: String, enrichment: &McpEnrichment) -> String {
    if enrichment.needs_login {
        "need-login".to_string()
    } else {
        status
    }
}

pub(crate) fn probe_state_for_enrichment(enrichment: &McpEnrichment) -> McpProbeState {
    if enrichment.probe_succeeded {
        if enrichment.tools.is_empty() {
            McpProbeState::ReadyEmpty
        } else {
            McpProbeState::Ready
        }
    } else if enrichment.needs_login {
        McpProbeState::NeedsAuth
    } else if !enrichment.tools.is_empty() {
        // A plugin can provide a complete static tool list without a live
        // protocol connection. Other metadata alone is not a probe result.
        McpProbeState::Ready
    } else {
        McpProbeState::Unknown
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSetEnabledRequest {
    pub agent: AgentKind,
    pub path: PathBuf,
    pub expected_trust_hash: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub server_path: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpProbeRequest {
    pub agent: AgentKind,
    pub path: PathBuf,
    pub expected_trust_hash: String,
    pub name: String,
    #[serde(default)]
    pub server_path: Vec<String>,
}

fn validate_probe_identity(request: &McpProbeRequest, current: &McpServerRecord) -> Result<()> {
    if current.agent != request.agent
        || current.path != request.path
        || current.name != request.name
        || current.server_path != request.server_path
    {
        anyhow::bail!("MCP server identity changed; refresh MCP before checking its connection")
    }
    if request.expected_trust_hash.trim().is_empty() {
        anyhow::bail!("MCP source hash is unavailable; refresh MCP before checking its connection")
    }
    Ok(())
}

pub(crate) fn apply_probe_enrichment(
    current: &McpServerRecord,
    transport: String,
    enabled: bool,
    status: String,
    enrichment: McpEnrichment,
) -> McpServerRecord {
    let mut updated = current.clone();
    updated.transport = transport;
    updated.enabled = enabled;
    updated.status = mcp_status_for_enrichment(status, &enrichment);
    updated.probe_cache_version = MCP_PROBE_CACHE_VERSION;
    updated.probe_error = enrichment.probe_error.clone();
    updated.probe_state = if enrichment.probe_succeeded {
        if enrichment.tools.is_empty() {
            McpProbeState::ReadyEmpty
        } else {
            McpProbeState::Ready
        }
    } else if enrichment.needs_login {
        McpProbeState::NeedsAuth
    } else {
        McpProbeState::Failed
    };
    if enrichment.probe_succeeded {
        updated.server_name = enrichment.server_name;
        updated.server_title = enrichment.server_title;
        updated.server_version = enrichment.server_version;
        updated.server_description = enrichment.server_description;
        updated.server_website_url = enrichment.server_website_url;
        if !enrichment.icons.is_empty() {
            updated.icons = enrichment.icons;
        }
        updated.tools = enrichment.tools;
    }
    updated
}

pub(crate) fn probe_json_mcp_server(
    request: &McpProbeRequest,
    current: &McpServerRecord,
    infer_transport: fn(&Value) -> Option<String>,
    infer_enabled: fn(&Value) -> bool,
    infer_status: fn(&Value) -> String,
    enrich: fn(&str, &Value, &str, bool, Option<&Path>, &mut McpProbeCache) -> McpEnrichment,
    probe_cache: &mut McpProbeCache,
) -> Result<McpServerRecord> {
    probe_json_mcp_server_at_dir(
        request,
        current,
        infer_transport,
        infer_enabled,
        infer_status,
        enrich,
        request.path.parent(),
        probe_cache,
    )
}

pub(crate) fn probe_json_mcp_server_at_dir(
    request: &McpProbeRequest,
    current: &McpServerRecord,
    infer_transport: fn(&Value) -> Option<String>,
    infer_enabled: fn(&Value) -> bool,
    infer_status: fn(&Value) -> String,
    enrich: fn(&str, &Value, &str, bool, Option<&Path>, &mut McpProbeCache) -> McpEnrichment,
    base_dir: Option<&Path>,
    probe_cache: &mut McpProbeCache,
) -> Result<McpServerRecord> {
    validate_probe_identity(request, current)?;
    let text = fs::read_to_string(&request.path)?;
    if sha256_text(&text) != request.expected_trust_hash {
        anyhow::bail!("MCP source changed; refresh MCP before checking its connection")
    }
    let value = serde_json::from_str::<Value>(&text)?;
    let spec = json_object_at_path(&value, &request.server_path)
        .and_then(|servers| servers.get(&request.name))
        .ok_or_else(|| anyhow::anyhow!("matching MCP server was not found"))?;
    let transport = infer_transport(spec)
        .ok_or_else(|| anyhow::anyhow!("MCP server has no recognized transport"))?;
    let enabled = infer_enabled(spec);
    let enrichment = enrich(
        &request.name,
        spec,
        &transport,
        enabled,
        base_dir,
        probe_cache,
    );
    Ok(apply_probe_enrichment(
        current,
        transport,
        enabled,
        infer_status(spec),
        enrichment,
    ))
}

pub(crate) fn probe_toml_mcp_server(
    request: &McpProbeRequest,
    current: &McpServerRecord,
    server_key: &str,
    infer_transport: fn(&TomlValue) -> Option<String>,
    infer_enabled: fn(&TomlValue) -> bool,
    infer_status: fn(&TomlValue) -> String,
    enrich: fn(&str, &TomlValue, &str, bool, Option<&Path>, &mut McpProbeCache) -> McpEnrichment,
    probe_cache: &mut McpProbeCache,
) -> Result<McpServerRecord> {
    validate_probe_identity(request, current)?;
    if request.server_path != [server_key.to_string()] {
        anyhow::bail!("MCP server path is not supported by this source")
    }
    let text = fs::read_to_string(&request.path)?;
    if sha256_text(&text) != request.expected_trust_hash {
        anyhow::bail!("MCP source changed; refresh MCP before checking its connection")
    }
    let value = toml::from_str::<TomlValue>(text.trim_start())?;
    let spec = value
        .get(server_key)
        .and_then(TomlValue::as_table)
        .and_then(|servers| servers.get(&request.name))
        .ok_or_else(|| anyhow::anyhow!("matching MCP server was not found"))?;
    let transport = infer_transport(spec)
        .ok_or_else(|| anyhow::anyhow!("MCP server has no recognized transport"))?;
    let enabled = infer_enabled(spec);
    let enrichment = enrich(
        &request.name,
        spec,
        &transport,
        enabled,
        request.path.parent(),
        probe_cache,
    );
    Ok(apply_probe_enrichment(
        current,
        transport,
        enabled,
        infer_status(spec),
        enrichment,
    ))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpScan {
    pub servers: Vec<McpServerRecord>,
    pub warnings: Vec<String>,
}

pub fn scan_mcp(cwd: &Path) -> Result<McpScan> {
    scan_mcp_for_project_roots(cwd, &[])
}

pub fn scan_mcp_for_project_roots(cwd: &Path, project_roots: &[PathBuf]) -> Result<McpScan> {
    let mut servers = Vec::new();
    let mut warnings = Vec::new();
    let mut probe_cache = McpProbeCache::metadata_only();

    let context =
        crate::providers::ProviderContext::with_additional_project_dirs(cwd, project_roots);
    for provider in crate::providers::agent_providers() {
        provider.scan_mcp(&context, &mut servers, &mut warnings, &mut probe_cache)?;
    }

    servers.sort_by(|a, b| {
        a.agent
            .cmp(&b.agent)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.server_path.cmp(&b.server_path))
    });
    servers.dedup_by(|a, b| {
        a.agent == b.agent && a.name == b.name && a.path == b.path && a.server_path == b.server_path
    });
    Ok(McpScan { servers, warnings })
}

/// Scan MCP metadata and restore probe data already represented by the
/// persisted projection. Live probes are explicit user actions.
pub fn scan_mcp_for_project_roots_with_cached(
    cwd: &Path,
    project_roots: &[PathBuf],
    cached: Option<&McpScan>,
) -> Result<McpScan> {
    let mut scan = scan_mcp_for_project_roots(cwd, project_roots)?;

    for server in &mut scan.servers {
        if let Some(cached_server) = cached.and_then(|cached| {
            cached
                .servers
                .iter()
                .find(|candidate| same_mcp_source(candidate, server) && has_cached_probe(candidate))
        }) {
            restore_cached_probe(server, cached_server);
        }
    }

    Ok(scan)
}

fn same_mcp_source(left: &McpServerRecord, right: &McpServerRecord) -> bool {
    left.agent == right.agent
        && left.name == right.name
        && left.path == right.path
        && left.server_path == right.server_path
        && left.trust_hash == right.trust_hash
}

fn has_cached_probe(server: &McpServerRecord) -> bool {
    if server.probe_cache_version < MCP_PROBE_CACHE_VERSION {
        return false;
    }
    match server.probe_state {
        McpProbeState::Unknown => {
            server.status == "need-login"
                || server.status == "needs-auth"
                || !server.tools.is_empty()
        }
        // `ready` without tools was produced by the old metadata-only
        // interpretation and must be re-probed once.
        McpProbeState::Ready => !server.tools.is_empty(),
        McpProbeState::ReadyEmpty | McpProbeState::NeedsAuth | McpProbeState::Failed => true,
    }
}

fn restore_cached_probe(current: &mut McpServerRecord, cached: &McpServerRecord) {
    current.probe_cache_version = cached.probe_cache_version;
    current.server_name = cached.server_name.clone();
    current.server_title = cached.server_title.clone();
    current.server_version = cached.server_version.clone();
    current.server_description = cached.server_description.clone();
    current.server_website_url = cached.server_website_url.clone();
    if !cached.icons.is_empty() {
        current.icons = cached.icons.clone();
    }
    current.probe_error = cached.probe_error.clone();
    current.tools = cached.tools.clone();
    current.probe_state = if cached.probe_state != McpProbeState::Unknown {
        cached.probe_state
    } else if cached.status == "need-login" || cached.status == "needs-auth" {
        McpProbeState::NeedsAuth
    } else {
        McpProbeState::Ready
    };
    if current.probe_state == McpProbeState::NeedsAuth {
        current.status = "need-login".to_string();
    }
}

pub fn mcp_server_requires_probe(server: &McpServerRecord) -> bool {
    matches!(
        server.transport.as_str(),
        "stdio" | "http" | "sse" | "streamable-http" | "cursor-plugin"
    ) && matches!(
        server.probe_state,
        McpProbeState::Unknown | McpProbeState::Ready
    ) && !has_cached_probe(server)
}

pub fn set_server_enabled(request: McpSetEnabledRequest) -> Result<String> {
    let _resources =
        crate::coordination::acquire_file_resources(std::slice::from_ref(&request.path))?;
    crate::providers::agent_provider(request.agent).set_mcp_enabled(&request)?;
    let text = fs::read_to_string(&request.path)?;
    Ok(sha256_text(&text))
}

pub fn mcp_status_after_toggle(agent: AgentKind, enabled: bool) -> &'static str {
    crate::providers::agent_provider(agent).mcp_status_after_toggle(enabled)
}

pub fn probe_server(request: McpProbeRequest, current: McpServerRecord) -> Result<McpServerRecord> {
    let provider = crate::providers::agent_provider(request.agent);
    provider.prepare_mcp_probe(std::slice::from_ref(&current));
    let mut probe_cache = McpProbeCache::explicit();
    provider.probe_mcp(&request, &current, &mut probe_cache)
}

/// Validate the preparation input immediately before publishing a probe result.
/// The publishing owner retains the source resource lease through its commit.
pub fn verify_probe_source(path: &Path, expected: &str) -> Result<()> {
    if crate::fsutil::sha256_file(path)? != expected {
        anyhow::bail!("MCP source changed while probing; probe the current configuration again");
    }
    Ok(())
}

pub(crate) fn set_json_server_enabled(
    request: &McpSetEnabledRequest,
    server_keys: &[&str],
    update_server: fn(&mut serde_json::Map<String, Value>, bool) -> bool,
) -> Result<()> {
    let _resources =
        crate::coordination::acquire_file_resources(std::slice::from_ref(&request.path))?;
    let text = fs::read_to_string(&request.path)?;
    if request.expected_trust_hash.is_empty() || sha256_text(&text) != request.expected_trust_hash {
        anyhow::bail!("MCP source changed");
    }

    let mut value = serde_json::from_str::<Value>(&text)?;
    let before = value.clone();
    let updated = if request.server_path.is_empty() {
        update_json_server(
            &mut value,
            server_keys,
            &request.name,
            request.enabled,
            update_server,
        )
    } else {
        update_json_server_at_path(
            &mut value,
            &request.server_path,
            &request.name,
            request.enabled,
            update_server,
        )
    };
    if !updated {
        anyhow::bail!("matching MCP server was not found");
    }
    let after = crate::json_edit::patch_json_text(&text, &before, &value)?;

    atomic_write(&request.path, &after)
}

pub(crate) fn set_toml_server_enabled(
    request: &McpSetEnabledRequest,
    server_key: &str,
    update_server: fn(&mut dyn TableLike, bool) -> bool,
) -> Result<()> {
    let _resources =
        crate::coordination::acquire_file_resources(std::slice::from_ref(&request.path))?;
    let text = fs::read_to_string(&request.path)?;
    if request.expected_trust_hash.is_empty() || sha256_text(&text) != request.expected_trust_hash {
        anyhow::bail!("MCP source changed");
    }

    let mut value = text.parse::<DocumentMut>()?;
    if !update_toml_server(
        &mut value,
        server_key,
        &request.name,
        request.enabled,
        update_server,
    ) {
        anyhow::bail!("matching MCP server was not found");
    }
    let after = crate::fsutil::preserve_newline_style(&text, value.to_string());

    atomic_write(&request.path, &after)
}

pub(crate) fn read_json_server_entry_at_path(
    path: &Path,
    server_path: &[String],
    name: &str,
) -> Result<Value> {
    let text = fs::read_to_string(path)?;
    let value = serde_json::from_str::<Value>(&text)?;
    json_object_at_path(&value, server_path)
        .and_then(|servers| servers.get(name))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("MCP server {name} was not found"))
}

pub(crate) fn merge_json_server_entry_at_path(
    path: &Path,
    server_path: &[String],
    name: &str,
    entry: &Value,
) -> Result<String> {
    let (source, mut value) = if path.is_file() {
        let source = fs::read_to_string(path)?;
        (
            Some(source.clone()),
            serde_json::from_str::<Value>(&source)?,
        )
    } else {
        (None, Value::Object(serde_json::Map::new()))
    };
    let before = value.clone();
    let servers = json_object_at_path_mut_or_create(&mut value, server_path)
        .ok_or_else(|| anyhow::anyhow!("MCP server collection must be a JSON object"))?;
    servers.insert(name.to_string(), entry.clone());
    match source {
        Some(source) => crate::json_edit::patch_json_text(&source, &before, &value),
        None => Ok(format!("{}\n", serde_json::to_string_pretty(&value)?)),
    }
}

pub(crate) fn read_toml_server_entry(path: &Path, server_key: &str, name: &str) -> Result<Value> {
    let text = fs::read_to_string(path)?;
    let value = toml::from_str::<TomlValue>(&text)?;
    value
        .get(server_key)
        .and_then(TomlValue::as_table)
        .and_then(|servers| servers.get(name))
        .cloned()
        .map(|entry| serde_json::to_value(entry))
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("MCP server {name} was not found"))
}

pub(crate) fn merge_toml_server_entry(
    path: &Path,
    server_key: &str,
    name: &str,
    entry: &Value,
) -> Result<String> {
    let (source, mut value) = if path.is_file() {
        let source = fs::read_to_string(path)?;
        (Some(source.clone()), source.parse::<DocumentMut>()?)
    } else {
        (None, DocumentMut::new())
    };
    let root = value.as_table_mut();
    let servers = root
        .entry(server_key)
        .or_insert(Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or_else(|| anyhow::anyhow!("MCP server collection must be a TOML table"))?;
    let replacement =
        toml_edit::ser::to_document(&TomlValue::try_from(entry.clone()).map_err(|error| {
            anyhow::anyhow!("MCP server entry is not TOML-compatible: {error}")
        })?)?
        .into_item();
    if let Some(existing) = servers.get_mut(name) {
        merge_toml_item(existing, &replacement);
    } else {
        servers.insert(name, replacement);
    }
    Ok(source.map_or_else(
        || value.to_string(),
        |source| crate::fsutil::preserve_newline_style(&source, value.to_string()),
    ))
}

fn merge_toml_item(target: &mut Item, replacement: &Item) {
    if let (Some(target), Some(replacement)) =
        (target.as_table_like_mut(), replacement.as_table_like())
    {
        merge_toml_table_like(target, replacement);
    } else if !toml_items_equal(target, replacement) {
        *target = replacement.clone();
    }
}

fn merge_toml_table_like(target: &mut dyn TableLike, replacement: &dyn TableLike) {
    let replacement_items = replacement
        .iter()
        .map(|(key, item)| (key.to_string(), item.clone()))
        .collect::<Vec<_>>();
    let replacement_keys = replacement_items
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<std::collections::HashSet<_>>();
    let existing_keys = target
        .iter()
        .map(|(key, _)| key.to_string())
        .collect::<Vec<_>>();
    for key in existing_keys {
        if !replacement_keys.contains(key.as_str()) {
            target.remove(&key);
        }
    }
    for (key, item) in replacement_items {
        if let Some(existing) = target.get_mut(&key) {
            merge_toml_item(existing, &item);
        } else {
            target.insert(&key, item);
        }
    }
}

fn toml_items_equal(left: &Item, right: &Item) -> bool {
    match (left, right) {
        (Item::Value(left), Item::Value(right)) => toml_values_equal(left, right),
        (Item::Table(left), Item::Table(right)) => toml_tables_equal(left, right),
        (Item::ArrayOfTables(left), Item::ArrayOfTables(right)) => {
            left.len() == right.len()
                && (0..left.len()).all(|index| {
                    left.get(index)
                        .zip(right.get(index))
                        .is_some_and(|(left, right)| toml_tables_equal(left, right))
                })
        }
        _ => false,
    }
}

fn toml_values_equal(left: &EditValue, right: &EditValue) -> bool {
    match (left, right) {
        (EditValue::String(left), EditValue::String(right)) => left.value() == right.value(),
        (EditValue::Integer(left), EditValue::Integer(right)) => left.value() == right.value(),
        (EditValue::Float(left), EditValue::Float(right)) => left.value() == right.value(),
        (EditValue::Boolean(left), EditValue::Boolean(right)) => left.value() == right.value(),
        (EditValue::Datetime(left), EditValue::Datetime(right)) => left.value() == right.value(),
        (EditValue::Array(left), EditValue::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| toml_values_equal(left, right))
        }
        (EditValue::InlineTable(left), EditValue::InlineTable(right)) => {
            toml_tables_equal(left, right)
        }
        _ => false,
    }
}

fn toml_tables_equal(left: &dyn TableLike, right: &dyn TableLike) -> bool {
    left.len() == right.len()
        && left.iter().all(|(key, item)| {
            right
                .get(key)
                .is_some_and(|other| toml_items_equal(item, other))
        })
}

fn update_json_server(
    value: &mut Value,
    server_keys: &[&str],
    name: &str,
    enabled: bool,
    update_server: fn(&mut serde_json::Map<String, Value>, bool) -> bool,
) -> bool {
    for key in server_keys {
        let Some(servers) = value.get_mut(key).and_then(Value::as_object_mut) else {
            continue;
        };
        let Some(spec) = servers.get_mut(name).and_then(Value::as_object_mut) else {
            continue;
        };
        return update_server(spec, enabled);
    }
    false
}

fn update_json_server_at_path(
    value: &mut Value,
    server_path: &[String],
    name: &str,
    enabled: bool,
    update_server: fn(&mut serde_json::Map<String, Value>, bool) -> bool,
) -> bool {
    let Some(servers) = json_object_at_path_mut(value, server_path) else {
        return false;
    };
    let Some(spec) = servers.get_mut(name).and_then(Value::as_object_mut) else {
        return false;
    };
    update_server(spec, enabled)
}

fn json_object_at_path<'a>(
    value: &'a Value,
    path: &[String],
) -> Option<&'a serde_json::Map<String, Value>> {
    let mut current = value;
    for component in path {
        current = current.get(component)?;
    }
    current.as_object()
}

fn json_object_at_path_mut<'a>(
    value: &'a mut Value,
    path: &[String],
) -> Option<&'a mut serde_json::Map<String, Value>> {
    let mut current = value;
    for component in path {
        current = current.get_mut(component)?;
    }
    current.as_object_mut()
}

fn json_object_at_path_mut_or_create<'a>(
    value: &'a mut Value,
    path: &[String],
) -> Option<&'a mut serde_json::Map<String, Value>> {
    let mut current = value;
    for component in path {
        let object = current.as_object_mut()?;
        current = object
            .entry(component.clone())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
    }
    current.as_object_mut()
}

fn update_toml_server(
    value: &mut DocumentMut,
    server_key: &str,
    name: &str,
    enabled: bool,
    update_server: fn(&mut dyn TableLike, bool) -> bool,
) -> bool {
    let Some(servers) = value
        .as_table_mut()
        .get_mut(server_key)
        .and_then(Item::as_table_like_mut)
    else {
        return false;
    };
    let Some(spec) = servers.get_mut(name).and_then(Item::as_table_like_mut) else {
        return false;
    };
    update_server(spec, enabled)
}

pub(crate) fn scan_project_mcp(
    root: &Path,
    agent: AgentKind,
    file_names: &[&str],
    ignored_file_names: &[&str],
    server_keys: &[&str],
    infer_transport: fn(&Value) -> Option<String>,
    infer_enabled: fn(&Value) -> bool,
    infer_status: fn(&Value) -> String,
    enrich: fn(&str, &Value, &str, bool, Option<&Path>, &mut McpProbeCache) -> McpEnrichment,
    probe_cache: &mut McpProbeCache,
    servers: &mut Vec<McpServerRecord>,
    warnings: &mut Vec<String>,
) {
    if !root.is_dir() {
        return;
    }

    for entry in WalkDir::new(root)
        .follow_links(true)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| ignored_file_names.contains(&name))
        {
            continue;
        }
        let Some(scope) = entry
            .path()
            .strip_prefix(root)
            .ok()
            .and_then(|path| path.components().next())
            .and_then(|component| component.as_os_str().to_str())
        else {
            continue;
        };
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| file_names.contains(&name))
        {
            scan_json_mcp(
                entry.path(),
                agent,
                scope,
                server_keys,
                infer_transport,
                infer_enabled,
                infer_status,
                enrich,
                probe_cache,
                servers,
                warnings,
            );
        }
    }
}

pub(crate) fn scan_toml_mcp(
    path: &Path,
    agent: AgentKind,
    scope: &str,
    server_key: &str,
    infer_transport: fn(&TomlValue) -> Option<String>,
    infer_enabled: fn(&TomlValue) -> bool,
    infer_status: fn(&TomlValue) -> String,
    enrich: fn(&str, &TomlValue, &str, bool, Option<&Path>, &mut McpProbeCache) -> McpEnrichment,
    probe_cache: &mut McpProbeCache,
    servers: &mut Vec<McpServerRecord>,
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
    let value = match toml::from_str::<TomlValue>(text.trim_start()) {
        Ok(value) => value,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return;
        }
    };

    let Some(map) = value.get(server_key).and_then(TomlValue::as_table) else {
        return;
    };
    for (name, spec) in map {
        let Some(transport) = infer_transport(spec) else {
            warnings.push(format!(
                "{}: MCP server {name} has no recognized transport",
                path.display()
            ));
            continue;
        };
        let enrichment = enrich(
            name,
            spec,
            &transport,
            infer_enabled(spec),
            path.parent(),
            probe_cache,
        );
        servers.push(McpServerRecord {
            agent,
            name: name.to_string(),
            scope: scope.to_string(),
            transport,
            enabled: infer_enabled(spec),
            status: mcp_status_for_enrichment(infer_status(spec), &enrichment),
            path: path.to_path_buf(),
            trust_hash: sha256_text(&text),
            probe_cache_version: MCP_PROBE_CACHE_VERSION,
            probe_state: probe_state_for_enrichment(&enrichment),
            server_path: vec![server_key.to_string()],
            read_only_reason: None,
            server_name: enrichment.server_name,
            server_title: enrichment.server_title,
            server_version: enrichment.server_version,
            server_description: enrichment.server_description,
            server_website_url: enrichment.server_website_url,
            probe_error: enrichment.probe_error,
            icons: enrichment.icons,
            tools: enrichment.tools,
        });
    }
}

pub(crate) fn scan_json_mcp(
    path: &Path,
    agent: AgentKind,
    scope: &str,
    server_keys: &[&str],
    infer_transport: fn(&Value) -> Option<String>,
    infer_enabled: fn(&Value) -> bool,
    infer_status: fn(&Value) -> String,
    enrich: fn(&str, &Value, &str, bool, Option<&Path>, &mut McpProbeCache) -> McpEnrichment,
    probe_cache: &mut McpProbeCache,
    servers: &mut Vec<McpServerRecord>,
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
    let value = match serde_json::from_str::<Value>(&text) {
        Ok(value) => value,
        Err(_) => return,
    };

    for key in server_keys {
        let Some(map) = value.get(key).and_then(Value::as_object) else {
            continue;
        };
        scan_json_mcp_map(
            path,
            scope,
            &text,
            agent,
            &[key.to_string()],
            map,
            infer_transport,
            infer_enabled,
            infer_status,
            enrich,
            probe_cache,
            servers,
            warnings,
        );
    }
}

pub(crate) fn scan_json_mcp_at_path(
    path: &Path,
    agent: AgentKind,
    scope: &str,
    server_path: &[String],
    infer_transport: fn(&Value) -> Option<String>,
    infer_enabled: fn(&Value) -> bool,
    infer_status: fn(&Value) -> String,
    enrich: fn(&str, &Value, &str, bool, Option<&Path>, &mut McpProbeCache) -> McpEnrichment,
    probe_cache: &mut McpProbeCache,
    servers: &mut Vec<McpServerRecord>,
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
    let value = match serde_json::from_str::<Value>(&text) {
        Ok(value) => value,
        Err(_) => return,
    };
    let Some(map) = json_object_at_path(&value, server_path) else {
        return;
    };
    scan_json_mcp_map(
        path,
        scope,
        &text,
        agent,
        server_path,
        map,
        infer_transport,
        infer_enabled,
        infer_status,
        enrich,
        probe_cache,
        servers,
        warnings,
    );
}

fn scan_json_mcp_map(
    path: &Path,
    scope: &str,
    text: &str,
    agent: AgentKind,
    server_path: &[String],
    map: &serde_json::Map<String, Value>,
    infer_transport: fn(&Value) -> Option<String>,
    infer_enabled: fn(&Value) -> bool,
    infer_status: fn(&Value) -> String,
    enrich: fn(&str, &Value, &str, bool, Option<&Path>, &mut McpProbeCache) -> McpEnrichment,
    probe_cache: &mut McpProbeCache,
    servers: &mut Vec<McpServerRecord>,
    warnings: &mut Vec<String>,
) {
    for (name, spec) in map {
        let Some(transport) = infer_transport(spec) else {
            warnings.push(format!(
                "{}: MCP server {name} has no recognized transport",
                path.display()
            ));
            continue;
        };
        let enrichment = enrich(
            name,
            spec,
            &transport,
            infer_enabled(spec),
            path.parent(),
            probe_cache,
        );
        servers.push(build_mcp_server_record(
            path,
            agent,
            scope,
            text,
            server_path,
            name,
            transport,
            infer_enabled(spec),
            mcp_status_for_enrichment(infer_status(spec), &enrichment),
            enrichment,
            None,
        ));
    }
}

pub(crate) fn build_mcp_server_record(
    path: &Path,
    agent: AgentKind,
    scope: &str,
    text: &str,
    server_path: &[String],
    name: &str,
    transport: String,
    enabled: bool,
    status: String,
    enrichment: McpEnrichment,
    read_only_reason: Option<String>,
) -> McpServerRecord {
    McpServerRecord {
        agent,
        name: name.to_string(),
        scope: scope.to_string(),
        transport,
        enabled,
        status,
        path: path.to_path_buf(),
        trust_hash: sha256_text(text),
        probe_cache_version: MCP_PROBE_CACHE_VERSION,
        probe_state: probe_state_for_enrichment(&enrichment),
        server_path: server_path.to_vec(),
        read_only_reason,
        server_name: enrichment.server_name,
        server_title: enrichment.server_title,
        server_version: enrichment.server_version,
        server_description: enrichment.server_description,
        server_website_url: enrichment.server_website_url,
        probe_error: enrichment.probe_error,
        icons: enrichment.icons,
        tools: enrichment.tools,
    }
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
