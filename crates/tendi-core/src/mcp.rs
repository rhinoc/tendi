use std::{
    fs,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex, MutexGuard},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use toml::Value as TomlValue;
use walkdir::WalkDir;

use crate::{
    fsutil::{atomic_write, sha256_text},
    skills::AgentKind,
};

static MCP_MUTATION_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSetEnabledRequest {
    pub agent: AgentKind,
    pub path: PathBuf,
    pub expected_trust_hash: String,
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
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

    let context =
        crate::providers::ProviderContext::with_additional_project_dirs(cwd, project_roots);
    for provider in crate::providers::agent_providers() {
        provider.scan_mcp(&context, &mut servers, &mut warnings)?;
    }

    servers.sort_by(|a, b| {
        a.agent
            .cmp(&b.agent)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
    servers.dedup_by(|a, b| a.agent == b.agent && a.name == b.name && a.path == b.path);
    Ok(McpScan { servers, warnings })
}

pub fn set_server_enabled(request: McpSetEnabledRequest) -> Result<String> {
    crate::providers::agent_provider(request.agent).set_mcp_enabled(&request)?;
    let text = fs::read_to_string(&request.path)?;
    Ok(sha256_text(&text))
}

pub fn mcp_status_after_toggle(agent: AgentKind, enabled: bool) -> &'static str {
    crate::providers::agent_provider(agent).mcp_status_after_toggle(enabled)
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
    if !update_json_server(
        &mut value,
        server_keys,
        &request.name,
        request.enabled,
        update_server,
    ) {
        anyhow::bail!("matching MCP server was not found");
    }
    let after = format!("{}\n", serde_json::to_string_pretty(&value)?);

    atomic_write(&request.path, &after)
}

pub(crate) fn set_toml_server_enabled(
    request: &McpSetEnabledRequest,
    server_key: &str,
    update_server: fn(&mut toml::map::Map<String, TomlValue>, bool) -> bool,
) -> Result<()> {
    let _mutation = lock_mcp_mutation()?;
    let text = fs::read_to_string(&request.path)?;
    if request.expected_trust_hash.is_empty() || sha256_text(&text) != request.expected_trust_hash {
        anyhow::bail!("MCP source changed");
    }

    let mut value = toml::from_str::<TomlValue>(&text)?;
    if !update_toml_server(
        &mut value,
        server_key,
        &request.name,
        request.enabled,
        update_server,
    ) {
        anyhow::bail!("matching MCP server was not found");
    }
    let after = toml::to_string_pretty(&value)?;

    atomic_write(&request.path, &after)
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

fn update_toml_server(
    value: &mut TomlValue,
    server_key: &str,
    name: &str,
    enabled: bool,
    update_server: fn(&mut toml::map::Map<String, TomlValue>, bool) -> bool,
) -> bool {
    let Some(root) = value.as_table_mut() else {
        return false;
    };
    let Some(servers) = root.get_mut(server_key).and_then(TomlValue::as_table_mut) else {
        return false;
    };
    let Some(spec) = servers.get_mut(name).and_then(TomlValue::as_table_mut) else {
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
        servers.push(McpServerRecord {
            agent,
            name: name.to_string(),
            scope: scope.to_string(),
            transport,
            enabled: infer_enabled(spec),
            status: infer_status(spec),
            path: path.to_path_buf(),
            trust_hash: sha256_text(&text),
            read_only_reason: None,
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
        for (name, spec) in map {
            let Some(transport) = infer_transport(spec) else {
                warnings.push(format!(
                    "{}: MCP server {name} has no recognized transport",
                    path.display()
                ));
                continue;
            };
            servers.push(McpServerRecord {
                agent,
                name: name.to_string(),
                scope: scope.to_string(),
                transport,
                enabled: infer_enabled(spec),
                status: infer_status(spec),
                path: path.to_path_buf(),
                trust_hash: sha256_text(&text),
                read_only_reason: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        McpSetEnabledRequest, scan_mcp_for_project_roots, scan_toml_mcp, set_server_enabled,
    };
    use crate::{fsutil::sha256_text, skills::AgentKind};
    use toml::Value as TomlValue;

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
        })
        .expect_err("stale source should be rejected");
        assert!(error.to_string().contains("MCP source changed"));
        assert!(fs::read_to_string(&path).unwrap().ends_with("\n\n"));
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
        })
        .expect_err("Cursor plugin metadata should be read-only");
        assert!(error.to_string().contains("read-only"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scans_mcp_from_additional_project_roots() {
        let cwd = temp_root("project-cwd");
        let project = temp_root("project-root");
        fs::create_dir_all(&cwd).expect("create cwd");
        fs::create_dir_all(&project).expect("create project root");
        let config = project.join(".mcp.json");
        fs::write(
            &config,
            r#"{"mcpServers":{"project-server":{"command":"demo"}}}"#,
        )
        .expect("write project mcp");

        let scan = scan_mcp_for_project_roots(&cwd, std::slice::from_ref(&project)).unwrap();
        let config = config.canonicalize().unwrap();
        assert!(
            scan.servers
                .iter()
                .any(|server| server.path == config && server.name == "project-server")
        );

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
        let mut servers = Vec::new();
        let mut warnings = Vec::new();

        crate::providers::cursor::scan_project_mcp(&root, &mut servers, &mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "figma");
        assert_eq!(servers[0].scope, "project-alpha");
        assert_eq!(servers[0].transport, "cursor-plugin");
        assert_eq!(servers[0].status, "needs-auth");
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

        crate::providers::cursor::scan_project_mcp(&root, &mut servers, &mut warnings);

        assert!(warnings.is_empty());
        assert!(servers.is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
