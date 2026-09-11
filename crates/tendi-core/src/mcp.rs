use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{ChildStdin, Command, Stdio},
    sync::{LazyLock, Mutex, MutexGuard, mpsc},
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

static MCP_MUTATION_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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

pub(crate) fn set_json_server_enabled(
    request: &McpSetEnabledRequest,
    server_keys: &[&str],
    update_server: fn(&mut serde_json::Map<String, Value>, bool) -> bool,
) -> Result<()> {
    let _mutation = lock_mcp_mutation()?;
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
    let _mutation = lock_mcp_mutation()?;
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

fn lock_mcp_mutation() -> Result<MutexGuard<'static, ()>> {
    MCP_MUTATION_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("MCP mutation authority is unavailable"))
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
mod tests {
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        McpProbeCache, McpProbeRequest, McpProbeState, McpServerRecord, McpSetEnabledRequest,
        enrichment_from_responses, merge_json_server_entry_at_path, merge_toml_server_entry,
        probe_server, resolve_sse_endpoint, response_value, scan_mcp_for_project_roots,
        scan_toml_mcp, set_server_enabled,
    };
    use crate::{fsutil::sha256_text, skills::AgentKind};
    use rusqlite::params;
    use toml::Value as TomlValue;

    #[test]
    fn reads_standard_server_metadata_icons_and_tools() {
        let initialize = serde_json::json!({
            "id": 1,
            "result": {
                "serverInfo": {
                    "name": "figma",
                    "title": "Figma",
                    "version": "2.2.107",
                    "description": "Design context",
                    "websiteUrl": "https://figma.com",
                    "icons": [
                        {"src": "http://example.com/figma.svg", "mimeType": "image/svg+xml", "sizes": ["any"]}
                    ]
                }
            }
        });
        let tools = serde_json::json!({
            "id": 2,
            "result": {
                "tools": [
                    {
                        "name": "get_design_context",
                        "title": "Get Design Context",
                        "description": "Read a selection",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "fileKey": {"type": "string"}
                            },
                            "required": ["fileKey"]
                        }
                    }
                ]
            }
        });

        let enrichment = enrichment_from_responses(&initialize, Some(&tools));
        assert_eq!(enrichment.server_name.as_deref(), Some("figma"));
        assert_eq!(enrichment.server_title.as_deref(), Some("Figma"));
        assert_eq!(enrichment.server_version.as_deref(), Some("2.2.107"));
        assert_eq!(
            enrichment.icons[0].mime_type.as_deref(),
            Some("image/svg+xml")
        );
        assert_eq!(enrichment.tools[0].name, "get_design_context");
        assert_eq!(
            enrichment.tools[0].title.as_deref(),
            Some("Get Design Context")
        );
        assert_eq!(
            enrichment.tools[0]
                .input_schema
                .as_ref()
                .and_then(|schema| schema.get("properties"))
                .and_then(|properties| properties.get("fileKey"))
                .and_then(|file_key| file_key.get("type"))
                .and_then(serde_json::Value::as_str),
            Some("string")
        );
    }

    #[test]
    fn probe_without_icons_preserves_provider_icon() {
        let current = McpServerRecord {
            agent: AgentKind::Codex,
            name: "computer-use".to_string(),
            scope: "global".to_string(),
            transport: "stdio".to_string(),
            enabled: true,
            status: "configured".to_string(),
            path: std::path::PathBuf::from("/tmp/mcp.json"),
            trust_hash: "hash".to_string(),
            probe_cache_version: super::MCP_PROBE_CACHE_VERSION,
            probe_state: McpProbeState::Unknown,
            server_path: vec!["mcpServers".to_string()],
            read_only_reason: None,
            server_name: Some("computer-use".to_string()),
            server_title: Some("Computer Use".to_string()),
            server_version: None,
            server_description: None,
            server_website_url: None,
            probe_error: None,
            icons: vec![super::McpIcon {
                src: "data:image/png;base64,icon".to_string(),
                mime_type: Some("image/png".to_string()),
                sizes: vec!["any".to_string()],
                theme: None,
            }],
            tools: Vec::new(),
        };
        let updated = super::apply_probe_enrichment(
            &current,
            "stdio".to_string(),
            true,
            "configured".to_string(),
            super::McpEnrichment {
                probe_succeeded: true,
                tools: vec![super::McpTool {
                    name: "computer".to_string(),
                    title: None,
                    description: None,
                    input_schema: None,
                    icons: Vec::new(),
                }],
                ..Default::default()
            },
        );

        assert_eq!(updated.icons.len(), 1);
        assert_eq!(updated.tools[0].name, "computer");
    }

    #[test]
    fn probes_disabled_mcp_once_and_reuses_enrichment() {
        let marker = temp_root("probe-cache");
        let script = r#"
if [ -e "$1" ]; then exit 1; fi
touch "$1"
initialize='{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"figma","icons":[{"src":"http://example.com/figma.svg"}]}}}'
tools='{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}'
printf 'Content-Length: %s\r\n\r\n%s' "${#initialize}" "$initialize"
printf 'Content-Length: %s\r\n\r\n%s' "${#tools}" "$tools"
sleep 1
"#;
        let spec = serde_json::json!({
            "command": "/bin/sh",
            "args": ["-c", script, "mcp-probe", marker.display().to_string()],
        });
        let mut probe_cache = McpProbeCache::explicit();
        let first = super::enrich_json_mcp_spec(&spec, "stdio", false, &mut probe_cache);
        let second = super::enrich_json_mcp_spec(&spec, "http", true, &mut probe_cache);

        assert_eq!(first.server_name.as_deref(), Some("figma"));
        assert_eq!(first.icons.len(), 1);
        assert_eq!(second.server_name.as_deref(), Some("figma"));
        assert_eq!(second.icons.len(), 1);
        assert!(marker.is_file());
        let _ = fs::remove_file(marker);
    }

    #[test]
    fn metadata_only_scan_does_not_start_mcp_process() {
        let marker = temp_root("metadata-only-probe");
        let script = r#"touch "$1""#;
        let spec = serde_json::json!({
            "command": "/bin/sh",
            "args": ["-c", script, "mcp-probe", marker.display().to_string()],
        });
        let mut probe_cache = McpProbeCache::metadata_only();

        let enrichment = super::enrich_json_mcp_spec(&spec, "stdio", true, &mut probe_cache);

        assert!(!enrichment.probe_succeeded);
        assert!(!marker.exists());
    }

    #[test]
    fn static_mcp_metadata_without_tools_stays_unresolved() {
        let metadata = super::McpEnrichment {
            plugin_name: Some("demo".to_string()),
            server_name: Some("demo".to_string()),
            server_title: Some("Demo".to_string()),
            ..Default::default()
        };

        assert_eq!(
            super::probe_state_for_enrichment(&metadata),
            McpProbeState::Unknown
        );
    }

    #[test]
    fn invalidates_legacy_ready_without_tools_cache() {
        let server = McpServerRecord {
            agent: AgentKind::Codex,
            name: "legacy".to_string(),
            scope: "global".to_string(),
            transport: "stdio".to_string(),
            enabled: true,
            status: "configured".to_string(),
            path: std::path::PathBuf::from("/tmp/mcp.json"),
            trust_hash: "hash".to_string(),
            probe_cache_version: super::MCP_PROBE_CACHE_VERSION,
            probe_state: McpProbeState::Ready,
            server_path: vec!["mcpServers".to_string()],
            read_only_reason: None,
            server_name: Some("legacy".to_string()),
            server_title: Some("Legacy".to_string()),
            server_version: None,
            server_description: None,
            server_website_url: None,
            probe_error: None,
            icons: Vec::new(),
            tools: Vec::new(),
        };

        assert!(!super::has_cached_probe(&server));
        assert!(super::mcp_server_requires_probe(&server));
        let mut confirmed_empty = server;
        confirmed_empty.probe_state = McpProbeState::ReadyEmpty;
        assert!(super::has_cached_probe(&confirmed_empty));
        assert!(!super::mcp_server_requires_probe(&confirmed_empty));
    }

    #[test]
    fn stdio_probe_resolves_cwd_relative_to_source_directory() {
        let root = temp_root("stdio-cwd");
        fs::create_dir_all(&root).expect("create probe cwd");
        let script = r#"
initialize='{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"cwd-demo"}}}'
tools='{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}'
touch cwd-marker
printf 'Content-Length: %s\r\n\r\n%s' "${#initialize}" "$initialize"
printf 'Content-Length: %s\r\n\r\n%s' "${#tools}" "$tools"
sleep 1
"#;
        let spec = serde_json::json!({
            "command": "/bin/sh",
            "args": ["-c", script, "mcp-probe"],
            "cwd": ".",
        });
        let mut probe_cache = McpProbeCache::explicit();

        let enrichment = super::enrich_json_mcp_spec_with_headers_at_dir(
            &spec,
            "stdio",
            true,
            &std::collections::BTreeMap::new(),
            Some(&root),
            &mut probe_cache,
        );

        assert!(enrichment.probe_succeeded);
        assert_eq!(enrichment.server_name.as_deref(), Some("cwd-demo"));
        assert!(root.join("cwd-marker").is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stdio_probe_supports_json_lines_servers() {
        let script = r#"
read -r request
case "$request" in
  \{*) ;;
  *) exit 1 ;;
esac
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"json-lines"},"capabilities":{"tools":{}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"json_lines_tool"}]}}'
sleep 1
"#;
        let spec = serde_json::json!({
            "command": "/bin/sh",
            "args": ["-c", script, "mcp-probe"],
        });
        let mut probe_cache = McpProbeCache::explicit();

        let enrichment = super::enrich_json_mcp_spec(&spec, "stdio", true, &mut probe_cache);

        assert!(enrichment.probe_succeeded);
        assert_eq!(enrichment.server_name.as_deref(), Some("json-lines"));
        assert_eq!(enrichment.tools[0].name, "json_lines_tool");
    }

    #[test]
    fn stdio_probe_falls_back_to_content_length_servers() {
        let script = r#"
IFS= read -r first
case "$first" in
  'Content-Length:'*)
    IFS= read -r blank
    initialize='{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"content-length"},"capabilities":{"tools":{}}}}'
    tools='{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"content_length_tool"}]}}'
    printf 'Content-Length: %s\r\n\r\n%s' "${#initialize}" "$initialize"
    printf 'Content-Length: %s\r\n\r\n%s' "${#tools}" "$tools"
    sleep 1
    ;;
  *) exit 1 ;;
esac
"#;
        let spec = serde_json::json!({
            "command": "/bin/sh",
            "args": ["-c", script, "mcp-probe"],
        });
        let mut probe_cache = McpProbeCache::explicit();

        let enrichment = super::enrich_json_mcp_spec(&spec, "stdio", true, &mut probe_cache);

        assert!(enrichment.probe_succeeded);
        assert_eq!(enrichment.server_name.as_deref(), Some("content-length"));
        assert_eq!(enrichment.tools[0].name, "content_length_tool");
    }

    #[test]
    fn explicit_probe_reads_mcp_server_metadata() {
        let root = temp_root("explicit-probe");
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("mcp.json");
        let script = r#"initialize='{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"demo","version":"1"}}}'; tools='{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}'; printf 'Content-Length: %s\r\n\r\n%s' "$(printf %s "$initialize" | wc -c)" "$initialize"; printf 'Content-Length: %s\r\n\r\n%s' "$(printf %s "$tools" | wc -c)" "$tools"; sleep 1"#;
        let value = serde_json::json!({
            "mcpServers": {
                "demo": {
                    "command": "/bin/sh",
                    "args": ["-c", script, "mcp-probe"]
                }
            }
        });
        let text = serde_json::to_string(&value).expect("serialize MCP config");
        fs::write(&path, &text).expect("write MCP config");
        let current = McpServerRecord {
            agent: AgentKind::Codex,
            name: "demo".to_string(),
            scope: "global".to_string(),
            transport: "stdio".to_string(),
            enabled: false,
            status: "configured".to_string(),
            path: path.clone(),
            trust_hash: sha256_text(&text),
            probe_cache_version: super::MCP_PROBE_CACHE_VERSION,
            probe_state: McpProbeState::Unknown,
            server_path: vec!["mcpServers".to_string()],
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

        let updated = probe_server(
            McpProbeRequest {
                agent: AgentKind::Codex,
                path,
                expected_trust_hash: current.trust_hash.clone(),
                name: "demo".to_string(),
                server_path: vec!["mcpServers".to_string()],
            },
            current,
        )
        .expect("explicit MCP probe");

        assert_eq!(updated.server_name.as_deref(), Some("demo"));
        assert_eq!(updated.server_version.as_deref(), Some("1"));
        assert_eq!(updated.probe_state, McpProbeState::ReadyEmpty);
        assert_eq!(updated.status, "configured");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn marks_http_unauthorized_mcp_as_need_login() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind unauthorized MCP server");
        let address = listener
            .local_addr()
            .expect("read unauthorized MCP address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept MCP probe");
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                let read = stream.read(&mut chunk).expect("read MCP probe request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
                let Some(header_end) = request.windows(4).position(|value| value == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers.lines().find_map(|line| {
                    line.strip_prefix("Content-Length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                });
                if request.len() >= header_end + 4 + content_length.unwrap_or(0) {
                    break;
                }
            }
            stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Type: text/plain\r\nContent-Length: 12\r\nConnection: close\r\n\r\nUnauthorized",
                )
                .expect("write MCP unauthorized response");
            stream.flush().expect("flush MCP unauthorized response");
        });

        let spec = serde_json::json!({
            "url": format!("http://{address}/mcp"),
        });
        let mut probe_cache = McpProbeCache::explicit();
        let enrichment = super::enrich_json_mcp_spec(&spec, "http", false, &mut probe_cache);

        let needs_login = enrichment.needs_login;
        let probe_error = enrichment.probe_error.clone();
        server.join().expect("join unauthorized MCP server");
        assert!(
            needs_login,
            "expected authentication error, got {probe_error:?}"
        );
        assert_eq!(
            super::mcp_status_for_enrichment("disabled".to_string(), &enrichment),
            "need-login"
        );
    }

    #[test]
    fn accepts_json_and_sse_mcp_responses() {
        let json = response_value(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).unwrap();
        assert_eq!(json["id"], 1);
        let sse = response_value(
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n\n",
        )
        .unwrap();
        assert_eq!(sse["id"], 2);
        assert_eq!(
            resolve_sse_endpoint("https://example.com/sse", "/messages?id=1"),
            "https://example.com/messages?id=1"
        );
        assert_eq!(
            resolve_sse_endpoint("https://example.com/sse", "messages?id=1"),
            "https://example.com/messages?id=1"
        );
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "tendi-mcp-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ))
    }

    #[test]
    fn scans_codex_toml_mcp_servers() {
        let root = temp_root("codex");
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("config.toml");
        fs::write(
            &path,
            r#"
[mcp_servers.node_repl]
command = "/bin/node-repl"

[mcp_servers.remote]
url = "https://example.com/mcp"
enabled = false

[mcp_servers.invalid]
enabled = true
"#,
        )
        .expect("write config");
        let mut servers = Vec::new();
        let mut warnings = Vec::new();
        let mut probe_cache = McpProbeCache::default();

        scan_toml_mcp(
            &path,
            AgentKind::Codex,
            "global",
            "mcp_servers",
            |spec| {
                spec.get("command")
                    .and_then(TomlValue::as_str)
                    .map(|_| "stdio".to_string())
                    .or_else(|| {
                        spec.get("url")
                            .and_then(TomlValue::as_str)
                            .map(|_| "http".to_string())
                    })
            },
            |spec| {
                spec.get("enabled")
                    .and_then(TomlValue::as_bool)
                    .unwrap_or(true)
            },
            |spec| {
                (!spec
                    .get("enabled")
                    .and_then(TomlValue::as_bool)
                    .unwrap_or(true))
                .then(|| "disabled".to_string())
                .unwrap_or_else(|| "configured".to_string())
            },
            |_name, spec, transport, enabled, _base_dir, probe_cache| {
                super::enrich_toml_mcp_spec(spec, transport, enabled, probe_cache)
            },
            &mut probe_cache,
            &mut servers,
            &mut warnings,
        );

        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].name, "node_repl");
        assert_eq!(servers[0].scope, "global");
        assert_eq!(servers[0].transport, "stdio");
        assert!(servers[0].enabled);
        assert_eq!(servers[0].status, "configured");
        assert_eq!(servers[1].name, "remote");
        assert_eq!(servers[1].transport, "http");
        assert!(!servers[1].enabled);
        assert_eq!(servers[1].status, "disabled");
        assert_eq!(servers[1].probe_state, McpProbeState::Unknown);
        assert!(super::mcp_server_requires_probe(&servers[1]));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scans_codex_portable_plugin_mcp_with_metadata() {
        let root = temp_root("codex-plugin");
        let codex_home = root.join(".codex");
        let plugin_root = codex_home.join("plugins/cache/acme/portable/1.0.0");
        fs::create_dir_all(&plugin_root).expect("create Codex plugin root");
        fs::write(
            codex_home.join("config.toml"),
            "[plugins.\"portable@acme\"]\n",
        )
        .expect("write Codex plugin config");
        fs::write(
            plugin_root.join("plugin.json"),
            r#"{"name":"portable","version":"1.0.0","description":"Portable plugin","extensions":{"com.openai":{"interface":{"displayName":"Portable","logo":"./logo.png","websiteURL":"https://example.com"}}}}"#,
        )
        .expect("write Codex portable manifest");
        fs::write(plugin_root.join("logo.png"), [137_u8, 80, 78, 71])
            .expect("write Codex plugin logo");
        fs::write(
            plugin_root.join("mcp.json"),
            r#"{"mcpServers":{"docs":{"type":"http","url":"https://example.com/mcp","enabled_tools":["read","write"]}}}"#,
        )
        .expect("write Codex portable MCP source");

        let mut servers = Vec::new();
        let mut warnings = Vec::new();
        crate::providers::codex::scan_codex_plugin_mcp(&codex_home, &mut servers, &mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "docs");
        assert_eq!(servers[0].scope, "global");
        assert_eq!(servers[0].transport, "http");
        assert_eq!(servers[0].status, "configured");
        assert_eq!(servers[0].probe_state, McpProbeState::Ready);
        assert_eq!(
            servers[0]
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["read", "write"]
        );
        assert_eq!(servers[0].server_title.as_deref(), Some("Portable"));
        assert_eq!(servers[0].server_version.as_deref(), Some("1.0.0"));
        assert_eq!(
            servers[0].server_website_url.as_deref(),
            Some("https://example.com")
        );
        assert!(
            servers[0].icons[0]
                .src
                .starts_with("data:image/png;base64,")
        );
        assert_eq!(
            servers[0].read_only_reason.as_deref(),
            Some("Codex plugin MCP is managed by the plugin")
        );
        let error = set_server_enabled(McpSetEnabledRequest {
            agent: AgentKind::Codex,
            path: servers[0].path.clone(),
            expected_trust_hash: servers[0].trust_hash.clone(),
            name: servers[0].name.clone(),
            enabled: false,
            server_path: servers[0].server_path.clone(),
        })
        .expect_err("Codex plugin MCP should be read-only");
        assert!(error.to_string().contains("managed by the plugin"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scans_codex_legacy_plugin_mcp_and_server_policy() {
        let root = temp_root("codex-legacy-plugin");
        let codex_home = root.join(".codex");
        let plugin_root = codex_home.join("plugins/cache/acme/legacy/1.0.0");
        fs::create_dir_all(plugin_root.join(".codex-plugin"))
            .expect("create Codex legacy plugin root");
        fs::write(
            codex_home.join("config.toml"),
            "[plugins.\"legacy@acme\"]\nenabled = true\n\n[plugins.\"legacy@acme\".mcp_servers.docs]\nenabled = false\n",
        )
        .expect("write Codex plugin config");
        fs::write(
            plugin_root.join(".codex-plugin/plugin.json"),
            r#"{"name":"legacy","mcpServers":"./.mcp.json"}"#,
        )
        .expect("write Codex legacy manifest");
        fs::write(
            plugin_root.join(".mcp.json"),
            r#"{"mcpServers":{"docs":{"url":"https://example.com/mcp"}}}"#,
        )
        .expect("write Codex legacy MCP source");

        let mut servers = Vec::new();
        let mut warnings = Vec::new();
        crate::providers::codex::scan_codex_plugin_mcp(&codex_home, &mut servers, &mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "docs");
        assert!(!servers[0].enabled);
        assert_eq!(servers[0].status, "disabled");
        assert!(servers[0].path.ends_with(".mcp.json"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scans_claude_enabled_plugin_mcp_with_project_scope() {
        let root = temp_root("claude-plugin");
        let home = root.join("home");
        let project = root.join("workspace");
        let plugin_root = home.join(".claude/plugins/marketplaces/acme/external_plugins/remote");
        fs::create_dir_all(plugin_root.join(".claude-plugin")).expect("create Claude plugin root");
        fs::create_dir_all(project.join(".claude")).expect("create Claude project settings");
        fs::write(
            project.join(".claude/settings.json"),
            r#"{"enabledPlugins":{"remote@acme":true}}"#,
        )
        .expect("write Claude plugin config");
        fs::write(
            plugin_root.join(".claude-plugin/plugin.json"),
            r#"{"name":"remote","version":"2.0.0","description":"Remote tools","logo":"./logo.svg","homepage":"https://example.com"}"#,
        )
        .expect("write Claude plugin manifest");
        fs::write(
            plugin_root.join("logo.svg"),
            "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
        )
        .expect("write Claude plugin logo");
        fs::write(
            plugin_root.join(".mcp.json"),
            r#"{"remote":{"type":"http","url":"https://example.com/mcp"}}"#,
        )
        .expect("write Claude direct MCP source");

        let mut servers = Vec::new();
        let mut warnings = Vec::new();
        crate::providers::claude::scan_claude_plugin_mcp(
            &home,
            std::slice::from_ref(&project),
            &mut servers,
            &mut warnings,
        );

        assert!(warnings.is_empty());
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "remote");
        assert_eq!(servers[0].scope, project.display().to_string());
        assert_eq!(servers[0].transport, "http");
        assert_eq!(servers[0].status, "configured");
        assert_eq!(servers[0].server_version.as_deref(), Some("2.0.0"));
        assert_eq!(
            servers[0].server_website_url.as_deref(),
            Some("https://example.com")
        );
        assert!(
            servers[0].icons[0]
                .src
                .starts_with("data:image/svg+xml;base64,")
        );
        assert_eq!(
            servers[0].read_only_reason.as_deref(),
            Some("Claude plugin MCP is managed by the plugin")
        );
        let error = set_server_enabled(McpSetEnabledRequest {
            agent: AgentKind::Claude,
            path: servers[0].path.clone(),
            expected_trust_hash: servers[0].trust_hash.clone(),
            name: servers[0].name.clone(),
            enabled: false,
            server_path: servers[0].server_path.clone(),
        })
        .expect_err("Claude plugin MCP should be read-only");
        assert!(error.to_string().contains("managed by the plugin"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn toggles_json_mcp_server_and_rejects_stale_source() {
        let root = temp_root("toggle-json");
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("mcp.json");
        let text = r#"{"mcpServers":{"demo":{"command":"demo","enabled":false}}}"#;
        fs::write(&path, text).expect("write config");

        set_server_enabled(McpSetEnabledRequest {
            agent: AgentKind::Claude,
            path: path.clone(),
            expected_trust_hash: sha256_text(text),
            name: "demo".to_string(),
            enabled: true,
            server_path: Vec::new(),
        })
        .expect("enable MCP server");
        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).expect("parse updated JSON");
        assert_eq!(value["mcpServers"]["demo"]["enabled"], true);

        let stale = fs::read_to_string(&path).expect("read updated config");
        fs::write(&path, format!("{stale}\n")).expect("change config");
        let error = set_server_enabled(McpSetEnabledRequest {
            agent: AgentKind::Claude,
            path: path.clone(),
            expected_trust_hash: sha256_text(&stale),
            name: "demo".to_string(),
            enabled: false,
            server_path: Vec::new(),
        })
        .expect_err("stale source should be rejected");
        assert!(error.to_string().contains("MCP source changed"));
        let unchanged = fs::read_to_string(&path).unwrap();
        assert!(unchanged.ends_with('\n'));
        assert!(!unchanged.ends_with("\n\n"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn toggles_json_mcp_server_without_reformatting_crlf_source() {
        let root = temp_root("toggle-json-format");
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("mcp.json");
        let source = "{\r\n  \"mcpServers\": {\r\n    \"demo\": { \"command\": \"demo\", \"enabled\": false },\r\n    \"kept\": {\"url\": \"https://example.com\"}\r\n  },\r\n  \"other\": true\r\n}\r\n";
        fs::write(&path, source).expect("write config");

        set_server_enabled(McpSetEnabledRequest {
            agent: AgentKind::Claude,
            path: path.clone(),
            expected_trust_hash: sha256_text(source),
            name: "demo".to_string(),
            enabled: true,
            server_path: Vec::new(),
        })
        .expect("enable MCP server");

        assert_eq!(
            fs::read_to_string(&path).expect("read updated config"),
            source.replace("\"enabled\": false", "\"enabled\": true")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn toggles_toml_mcp_server_without_reformatting_crlf_source() {
        let root = temp_root("toggle-toml-format");
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("config.toml");
        let source = "# keep\r\n[mcp_servers.demo]\r\ncommand = 'demo' # command\r\ndisabled = true\r\n\r\n[other]\r\nvalue = 1\r\n";
        fs::write(&path, source).expect("write config");

        set_server_enabled(McpSetEnabledRequest {
            agent: AgentKind::Codex,
            path: path.clone(),
            expected_trust_hash: sha256_text(source),
            name: "demo".to_string(),
            enabled: true,
            server_path: Vec::new(),
        })
        .expect("enable MCP server");

        assert_eq!(
            fs::read_to_string(&path).expect("read updated config"),
            source.replace("disabled = true", "disabled = false")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn merges_one_json_mcp_server_without_dropping_other_servers() {
        let root = temp_root("merge-json");
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("mcp.json");
        fs::write(
            &path,
            r#"{
  "other": true,
  "mcpServers": {
    "selected": {"command": "old"},
    "kept": {"url": "https://example.com/mcp"}
  }
}"#,
        )
        .expect("write config");

        let merged = merge_json_server_entry_at_path(
            &path,
            &["mcpServers".to_string()],
            "selected",
            &serde_json::json!({"command": "new"}),
        )
        .expect("merge selected server");
        fs::write(&path, merged).expect("write merged config");
        let value = serde_json::from_str::<serde_json::Value>(
            &fs::read_to_string(&path).expect("read merged config"),
        )
        .expect("parse merged config");

        assert_eq!(value["other"], true);
        assert_eq!(value["mcpServers"]["selected"]["command"], "new");
        assert_eq!(
            value["mcpServers"]["kept"]["url"],
            "https://example.com/mcp"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn merges_nested_json_mcp_server_without_dropping_other_projects() {
        let root = temp_root("merge-nested-json");
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("claude.json");
        fs::write(
            &path,
            r#"{
  "projects": {
    "/work/demo": {"mcpServers": {"selected": {"command": "old"}}},
    "/work/other": {"mcpServers": {"kept": {"command": "other"}}}
  }
}"#,
        )
        .expect("write config");

        let merged = super::merge_json_server_entry_at_path(
            &path,
            &[
                "projects".to_string(),
                "/work/demo".to_string(),
                "mcpServers".to_string(),
            ],
            "selected",
            &serde_json::json!({"command": "new"}),
        )
        .expect("merge nested server");
        let value = serde_json::from_str::<serde_json::Value>(&merged).expect("parse merged JSON");

        assert_eq!(
            value["projects"]["/work/demo"]["mcpServers"]["selected"]["command"],
            "new"
        );
        assert_eq!(
            value["projects"]["/work/other"]["mcpServers"]["kept"]["command"],
            "other"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn toggles_nested_claude_project_mcp_server_only() {
        let root = temp_root("toggle-nested-json");
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("claude.json");
        let text = r#"{
  "mcpServers": {"personal": {"command": "personal"}},
  "projects": {
    "/work/demo": {"mcpServers": {"project": {"command": "project"}}}
  }
}"#;
        fs::write(&path, text).expect("write config");

        set_server_enabled(McpSetEnabledRequest {
            agent: AgentKind::Claude,
            path: path.clone(),
            expected_trust_hash: sha256_text(text),
            name: "project".to_string(),
            enabled: false,
            server_path: vec![
                "projects".to_string(),
                "/work/demo".to_string(),
                "mcpServers".to_string(),
            ],
        })
        .expect("disable nested MCP server");

        let value = serde_json::from_str::<serde_json::Value>(
            &fs::read_to_string(&path).expect("read updated config"),
        )
        .expect("parse updated config");
        assert_eq!(
            value["mcpServers"]["personal"]["disabled"],
            serde_json::Value::Null
        );
        assert_eq!(
            value["projects"]["/work/demo"]["mcpServers"]["project"]["disabled"],
            true
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn merges_one_toml_mcp_server_without_dropping_other_servers() {
        let root = temp_root("merge-toml");
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("config.toml");
        fs::write(
            &path,
            "other = true\n\n[mcp_servers.selected]\ncommand = \"old\"\n\n[mcp_servers.kept]\nurl = \"https://example.com/mcp\"\n",
        )
        .expect("write config");

        let merged = merge_toml_server_entry(
            &path,
            "mcp_servers",
            "selected",
            &serde_json::json!({"command": "new"}),
        )
        .expect("merge selected server");
        let value = toml::from_str::<TomlValue>(&merged).expect("parse merged config");

        assert_eq!(value["other"], TomlValue::Boolean(true));
        assert_eq!(
            value["mcp_servers"]["selected"]["command"],
            TomlValue::String("new".to_string())
        );
        assert_eq!(
            value["mcp_servers"]["kept"]["url"],
            TomlValue::String("https://example.com/mcp".to_string())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn merges_toml_mcp_server_without_reformatting_unchanged_fields() {
        let root = temp_root("merge-toml-format");
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("config.toml");
        let source = "# keep\r\n[mcp_servers.selected]\r\ncommand = 'old'\r\ntype = 'stdio' # keep this field\r\n\r\n[mcp_servers.kept]\r\nurl = 'https://example.com/mcp'\r\n";
        fs::write(&path, source).expect("write config");

        let merged = merge_toml_server_entry(
            &path,
            "mcp_servers",
            "selected",
            &serde_json::json!({"command": "new", "type": "stdio"}),
        )
        .expect("merge selected server");

        assert_eq!(
            merged,
            source.replace("command = 'old'", "command = \"new\"")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn toggles_toml_mcp_server() {
        let root = temp_root("toggle-toml");
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("config.toml");
        let text = "[mcp_servers.demo]\ncommand = \"demo\"\n";
        fs::write(&path, text).expect("write config");

        set_server_enabled(McpSetEnabledRequest {
            agent: AgentKind::Codex,
            path: path.clone(),
            expected_trust_hash: sha256_text(text),
            name: "demo".to_string(),
            enabled: false,
            server_path: Vec::new(),
        })
        .expect("disable MCP server");
        let updated = fs::read_to_string(&path).expect("read updated config");
        let value = toml::from_str::<TomlValue>(&updated).expect("parse updated TOML");
        assert_eq!(
            value["mcp_servers"]["demo"]["enabled"],
            TomlValue::Boolean(false)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn toggles_cursor_json_mcp_server_and_rejects_plugin_metadata() {
        let root = temp_root("toggle-cursor");
        fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("mcp.json");
        let text = r#"{"mcpServers":{"demo":{"command":"demo"}}}"#;
        fs::write(&path, text).expect("write config");

        set_server_enabled(McpSetEnabledRequest {
            agent: AgentKind::Cursor,
            path: path.clone(),
            expected_trust_hash: sha256_text(text),
            name: "demo".to_string(),
            enabled: false,
            server_path: Vec::new(),
        })
        .expect("disable Cursor MCP server");
        let updated = fs::read_to_string(&path).expect("read updated config");
        let value = serde_json::from_str::<serde_json::Value>(&updated).expect("parse JSON");
        assert_eq!(value["mcpServers"]["demo"]["disabled"], true);

        let metadata_path = root.join("SERVER_METADATA.json");
        let metadata = r#"{"serverIdentifier":"demo"}"#;
        fs::write(&metadata_path, metadata).expect("write plugin metadata");
        let error = set_server_enabled(McpSetEnabledRequest {
            agent: AgentKind::Cursor,
            path: metadata_path,
            expected_trust_hash: sha256_text(metadata),
            name: "demo".to_string(),
            enabled: false,
            server_path: Vec::new(),
        })
        .expect_err("Cursor plugin metadata should be read-only");
        assert!(error.to_string().contains("read-only"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scans_mcp_from_additional_project_roots() {
        let cwd = temp_root("project-cwd");
        let project = temp_root("project-root");
        let marker = cwd.join("mcp-probed");
        fs::create_dir_all(&cwd).expect("create cwd");
        fs::create_dir_all(&project).expect("create project root");
        let config = project.join(".mcp.json");
        let value = serde_json::json!({
            "mcpServers": {
                "project-server": {
                    "command": "/bin/sh",
                    "args": ["-c", "touch \"$1\"", "mcp-probe", marker.display().to_string()]
                }
            }
        });
        fs::write(
            &config,
            serde_json::to_string(&value).expect("serialize project mcp"),
        )
        .expect("write project mcp");

        let scan = scan_mcp_for_project_roots(&cwd, std::slice::from_ref(&project)).unwrap();
        let config = config.canonicalize().unwrap();
        assert!(
            scan.servers
                .iter()
                .any(|server| server.path == config && server.name == "project-server")
        );
        assert!(!marker.exists());

        let _ = fs::remove_file(marker);
        let _ = fs::remove_dir_all(cwd);
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn scans_cursor_plugin_metadata_status() {
        let root = temp_root("cursor");
        let server_dir = root.join("project-alpha/mcps/plugin-figma-figma");
        fs::create_dir_all(&server_dir).expect("create server dir");
        let metadata = server_dir.join("SERVER_METADATA.json");
        fs::write(
            &metadata,
            r#"{"serverIdentifier":"plugin-figma-figma","serverName":"figma"}"#,
        )
        .expect("write metadata");
        fs::write(
            server_dir.join("STATUS.md"),
            "The MCP server needs authentication.",
        )
        .expect("write status");
        fs::create_dir_all(server_dir.join("tools")).expect("create runtime tools directory");
        fs::write(
            server_dir.join("tools/figma_tool.json"),
            r#"{"name":"figma_tool","description":"Figma runtime tool"}"#,
        )
        .expect("write runtime tool");
        let mut servers = Vec::new();
        let mut warnings = Vec::new();
        let mut probe_cache = McpProbeCache::default();

        crate::providers::cursor::scan_project_mcp(
            &root,
            &mut probe_cache,
            &mut servers,
            &mut warnings,
        );

        assert!(warnings.is_empty());
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "figma");
        assert_eq!(servers[0].scope, "project-alpha");
        assert_eq!(servers[0].transport, "cursor-plugin");
        assert_eq!(servers[0].status, "needs-auth");
        assert_eq!(servers[0].tools[0].name, "figma_tool");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scans_cursor_plugin_icon_from_mcp_manifest() {
        let root = temp_root("cursor-plugin-icon");
        let projects = root.join(".cursor/projects");
        let server_dir = projects.join("project-alpha/mcps/plugin-figma-figma");
        let duplicate_server_dir = projects.join("project-beta/mcps/plugin-figma-figma");
        fs::create_dir_all(&server_dir).expect("create server dir");
        fs::create_dir_all(&duplicate_server_dir).expect("create duplicate server dir");
        fs::write(
            server_dir.join("SERVER_METADATA.json"),
            r#"{"serverIdentifier":"plugin-figma-figma","serverName":"figma"}"#,
        )
        .expect("write metadata");
        fs::write(
            duplicate_server_dir.join("SERVER_METADATA.json"),
            r#"{"serverIdentifier":"plugin-figma-figma","serverName":"plugin-figma-figma"}"#,
        )
        .expect("write duplicate metadata");

        let state_db =
            root.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb");
        fs::create_dir_all(state_db.parent().expect("state database parent"))
            .expect("create state database directory");
        let connection = rusqlite::Connection::open(&state_db).expect("open Cursor state database");
        connection
            .execute_batch(
                "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB); \
                 CREATE TABLE cursorDiskKV (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB);",
            )
            .expect("create Cursor state tables");
        connection
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                params![
                    "cursor.plugins.installedIds.no-team|no-workspace",
                    r#"[{"id":"657","sources":["user"]}]"#
                ],
            )
            .expect("write installed plugin scope");
        let slash_menu = serde_json::json!([{
            "pluginAttribution": { "displayName": "Figma" },
            "slashSubject": { "kind": "plugin", "scope": "plugin", "scopeId": "657" }
        }]);
        connection
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                params![
                    "slashMenuItems/v7/local.glass.empty-window",
                    format!("v2:1\n{slash_menu}")
                ],
            )
            .expect("write plugin attribution");

        let plugin_root = root.join(".cursor/plugins/cache/cursor-public/figma/hash");
        fs::create_dir_all(plugin_root.join(".cursor-plugin")).expect("create plugin root");
        fs::create_dir_all(plugin_root.join(".claude-plugin")).expect("create Claude plugin root");
        fs::write(
            plugin_root.join(".claude-plugin/plugin.json"),
            r#"{"name":"figma","version":"2.2.107"}"#,
        )
        .expect("write Claude plugin manifest");
        fs::write(
            plugin_root.join(".cursor-plugin/plugin.json"),
            r#"{"name":"figma","displayName":"Figma","version":"2.2.107","description":"Figma MCP server","homepage":"https://github.com/figma/mcp-server-guide","logo":"./plugin-logo.svg","mcpServers":"./.mcp.json"}"#,
        )
        .expect("write plugin manifest");
        fs::write(
            plugin_root.join("plugin-logo.svg"),
            "<svg id=\"plugin-logo\" xmlns=\"http://www.w3.org/2000/svg\"/>",
        )
        .expect("write plugin logo");
        fs::write(
            plugin_root.join(".mcp.json"),
            r#"{"mcpServers":{"figma":{"url":"https://mcp.figma.com/mcp","_meta":{"ideToolIconPath":"./tool-icon.svg","ideToolTitles":{"get_design_context":"Get Design Context"}}}}}"#,
        )
        .expect("write mcp manifest");
        fs::write(
            plugin_root.join("tool-icon.svg"),
            "<svg id=\"tool-icon\" xmlns=\"http://www.w3.org/2000/svg\"/>",
        )
        .expect("write tool icon");

        let mut servers = Vec::new();
        let mut warnings = Vec::new();
        let mut probe_cache = McpProbeCache::default();
        crate::providers::cursor::scan_project_mcp(
            &projects,
            &mut probe_cache,
            &mut servers,
            &mut warnings,
        );

        assert!(warnings.is_empty());
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "figma");
        assert_eq!(servers[0].scope, "global");
        assert_eq!(servers[0].server_title.as_deref(), Some("Figma"));
        assert_eq!(servers[0].server_version.as_deref(), Some("2.2.107"));
        assert_eq!(servers[0].status, "configured");
        assert_eq!(servers[0].tools[0].name, "get_design_context");
        assert_eq!(
            servers[0].tools[0].title.as_deref(),
            Some("Get Design Context")
        );
        assert_eq!(servers[0].icons.len(), 1);
        assert!(servers[0].icons[0].src.starts_with("data:image/svg+xml,"));
        assert!(servers[0].icons[0].src.contains("plugin-logo"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scans_cursor_global_plugin_without_runtime_metadata() {
        let root = temp_root("cursor-plugin-without-runtime-metadata");
        let projects = root.join(".cursor/projects");
        fs::create_dir_all(&projects).expect("create Cursor projects directory");
        let state_db =
            root.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb");
        fs::create_dir_all(state_db.parent().expect("state database parent"))
            .expect("create state database directory");
        let connection = rusqlite::Connection::open(&state_db).expect("open Cursor state database");
        connection
            .execute_batch(
                "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB); \
                 CREATE TABLE cursorDiskKV (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB);",
            )
            .expect("create Cursor state tables");
        connection
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                params![
                    "cursor.plugins.installedIds.no-team|no-workspace",
                    r#"[{"id":"657","sources":["user"]}]"#
                ],
            )
            .expect("write installed plugin scope");
        let slash_menu = serde_json::json!([{
            "pluginAttribution": { "displayName": "Figma" },
            "slashSubject": { "kind": "plugin", "scope": "plugin", "scopeId": "657" }
        }]);
        connection
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                params![
                    "slashMenuItems/v7/local.glass.empty-window",
                    format!("v2:1\n{slash_menu}")
                ],
            )
            .expect("write plugin attribution");

        let plugin_root = root.join(".cursor/plugins/cache/cursor-public/figma/hash");
        fs::create_dir_all(plugin_root.join(".cursor-plugin")).expect("create plugin root");
        fs::write(
            plugin_root.join(".cursor-plugin/plugin.json"),
            r#"{"name":"figma","displayName":"Figma","version":"2.2.107","logo":"./plugin-logo.svg","mcpServers":"./.mcp.json"}"#,
        )
        .expect("write plugin manifest");
        fs::write(
            plugin_root.join("plugin-logo.svg"),
            "<svg id=\"plugin-logo\" xmlns=\"http://www.w3.org/2000/svg\"/>",
        )
        .expect("write plugin logo");
        fs::write(
            plugin_root.join(".mcp.json"),
            r#"{"mcpServers":{"figma":{"url":"https://mcp.figma.com/mcp","_meta":{"ideToolTitles":{"get_design_context":"Get Design Context"}}}}}"#,
        )
        .expect("write plugin MCP manifest");

        let mut servers = Vec::new();
        let mut warnings = Vec::new();
        let mut probe_cache = McpProbeCache::default();
        crate::providers::cursor::scan_project_mcp(
            &projects,
            &mut probe_cache,
            &mut servers,
            &mut warnings,
        );

        assert!(warnings.is_empty());
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "figma");
        assert_eq!(servers[0].scope, "global");
        assert_eq!(servers[0].status, "configured");
        assert!(servers[0].path.ends_with(".mcp.json"));
        assert_eq!(servers[0].tools[0].name, "get_design_context");
        assert_eq!(servers[0].icons.len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cursor_project_mcp_skips_metadata_without_server_name() {
        let root = temp_root("cursor-project-scope");
        let server_dir = root.join("project-alpha/mcps/cursor-app-control");
        fs::create_dir_all(&server_dir).expect("create server dir");
        fs::write(
            server_dir.join("SERVER_METADATA.json"),
            r#"{"serverIdentifier":"cursor-app-control"}"#,
        )
        .expect("write metadata");
        let mut servers = Vec::new();
        let mut warnings = Vec::new();
        let mut probe_cache = McpProbeCache::default();

        crate::providers::cursor::scan_project_mcp(
            &root,
            &mut probe_cache,
            &mut servers,
            &mut warnings,
        );

        assert!(warnings.is_empty());
        assert!(servers.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn deduplicates_repeated_cursor_runtime_mcp_metadata_as_global() {
        let root = temp_root("cursor-repeated-runtime-mcp");
        for project in ["project-alpha", "project-beta"] {
            for server in ["runtime-service-alpha", "runtime-service-beta"] {
                let server_dir = root.join(project).join("mcps").join(server);
                fs::create_dir_all(&server_dir).expect("create server directory");
                fs::write(
                    server_dir.join("SERVER_METADATA.json"),
                    format!(r#"{{"serverIdentifier":"{server}","serverName":"{server}"}}"#),
                )
                .expect("write metadata");
            }
        }

        let mut servers = Vec::new();
        let mut warnings = Vec::new();
        let mut probe_cache = McpProbeCache::default();
        crate::providers::cursor::scan_project_mcp(
            &root,
            &mut probe_cache,
            &mut servers,
            &mut warnings,
        );

        assert!(warnings.is_empty());
        assert_eq!(servers.len(), 2);
        assert!(servers.iter().all(|server| server.scope == "global"));
        assert!(
            servers
                .iter()
                .any(|server| server.name == "runtime-service-alpha")
        );
        assert!(
            servers
                .iter()
                .any(|server| server.name == "runtime-service-beta")
        );
        let _ = fs::remove_dir_all(root);
    }
}
