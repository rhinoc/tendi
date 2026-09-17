use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    McpProbeCache, McpProbeRequest, McpProbeState, McpServerRecord, McpSetEnabledRequest,
    enrichment_from_responses, mcp_server_id, mcp_server_matches_id,
    merge_json_server_entry_at_path, merge_toml_server_entry, probe_server, resolve_sse_endpoint,
    response_value, scan_mcp_for_project_roots, scan_toml_mcp, set_server_enabled,
};
use crate::{fsutil::sha256_text, skills::AgentKind};
use rusqlite::params;
use toml::Value as TomlValue;

fn identity_test_server() -> McpServerRecord {
    McpServerRecord {
        agent: AgentKind::Codex,
        name: "docs".to_string(),
        scope: "global".to_string(),
        transport: "stdio".to_string(),
        enabled: true,
        status: "configured".to_string(),
        path: std::path::PathBuf::from("/tmp/codex/config.toml"),
        trust_hash: "source-v1".to_string(),
        probe_cache_version: super::MCP_PROBE_CACHE_VERSION,
        probe_state: McpProbeState::Ready,
        server_path: vec!["mcp_servers".to_string()],
        read_only_reason: None,
        server_name: Some("Docs".to_string()),
        server_title: Some("Documentation".to_string()),
        server_version: Some("1.0.0".to_string()),
        server_description: Some("A docs server".to_string()),
        server_website_url: Some("https://example.com".to_string()),
        probe_error: None,
        icons: Vec::new(),
        tools: Vec::new(),
    }
}

#[test]
fn mcp_server_id_is_stable_and_ignores_mutable_metadata() {
    let server = identity_test_server();
    let id = mcp_server_id(&server);
    let mut changed = server.clone();
    changed.enabled = false;
    changed.status = "need-login".to_string();
    changed.trust_hash = "source-v2".to_string();
    changed.probe_cache_version = 0;
    changed.probe_state = McpProbeState::Failed;
    changed.read_only_reason = Some("managed by provider".to_string());
    changed.server_name = Some("Changed display name".to_string());
    changed.server_title = Some("Changed title".to_string());
    changed.server_version = Some("2.0.0".to_string());
    changed.server_description = Some("Changed description".to_string());
    changed.server_website_url = Some("https://changed.example.com".to_string());
    changed.probe_error = Some("connection failed".to_string());
    changed.icons.push(super::McpIcon {
        src: "https://example.com/icon.svg".to_string(),
        mime_type: None,
        sizes: Vec::new(),
        theme: None,
    });
    changed.tools.push(super::McpTool {
        name: "search".to_string(),
        title: None,
        description: None,
        input_schema: None,
        icons: Vec::new(),
    });

    assert_eq!(mcp_server_id(&server), id);
    assert_eq!(mcp_server_id(&changed), id);
    assert!(mcp_server_matches_id(&server, &id));
    assert!(mcp_server_matches_id(&changed, &id));
    assert!(!mcp_server_matches_id(&server, " "));
}

#[test]
fn mcp_server_id_distinguishes_each_identity_component() {
    let server = identity_test_server();
    let id = mcp_server_id(&server);

    let mut different_agent = server.clone();
    different_agent.agent = AgentKind::Claude;
    let mut different_path = server.clone();
    different_path.path = std::path::PathBuf::from("/tmp/claude/config.json");
    let mut different_server_path = server.clone();
    different_server_path.server_path = vec!["projects".to_string(), "mcp".to_string()];
    let mut different_name = server.clone();
    different_name.name = "search".to_string();

    for different in [
        different_agent,
        different_path,
        different_server_path,
        different_name,
    ] {
        assert_ne!(mcp_server_id(&different), id);
        assert!(!mcp_server_matches_id(&different, &id));
    }
}

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
            let Some(header_end) = request.windows(4).position(|value| value == b"\r\n\r\n") else {
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
    let sse =
        response_value("event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n\n")
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
    fs::write(plugin_root.join("logo.png"), [137_u8, 80, 78, 71]).expect("write Codex plugin logo");
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
    fs::create_dir_all(plugin_root.join(".codex-plugin")).expect("create Codex legacy plugin root");
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

    let state_db = root.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb");
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
    let state_db = root.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb");
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
