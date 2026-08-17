use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use toml::Value as TomlValue;
use walkdir::WalkDir;

use crate::skills::AgentKind;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerRecord {
    pub agent: AgentKind,
    pub name: String,
    pub scope: String,
    pub transport: String,
    pub status: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpScan {
    pub servers: Vec<McpServerRecord>,
    pub warnings: Vec<String>,
}

pub fn scan_mcp(cwd: &Path) -> Result<McpScan> {
    let mut servers = Vec::new();
    let mut warnings = Vec::new();

    let context = crate::providers::ProviderContext::new(cwd);
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

pub(crate) fn scan_project_mcp(
    root: &Path,
    agent: AgentKind,
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
        if entry.file_name() == "mcp-auth.json" {
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
        if entry.file_name() == "mcp.json" || entry.file_name() == ".mcp.json" {
            scan_json_mcp(entry.path(), agent, scope, servers, warnings);
        }
    }
}

pub(crate) fn scan_toml_mcp(
    path: &Path,
    agent: AgentKind,
    scope: &str,
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

    let Some(map) = value.get("mcp_servers").and_then(TomlValue::as_table) else {
        return;
    };
    for (name, spec) in map {
        servers.push(McpServerRecord {
            agent,
            name: name.to_string(),
            scope: scope.to_string(),
            transport: infer_toml_transport(spec),
            status: infer_toml_status(spec),
            path: path.to_path_buf(),
        });
    }
}

pub(crate) fn scan_json_mcp(
    path: &Path,
    agent: AgentKind,
    scope: &str,
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

    if let Some(map) = value
        .get("mcpServers")
        .or_else(|| value.get("mcp_servers"))
        .or_else(|| value.get("servers"))
        .and_then(Value::as_object)
    {
        for (name, spec) in map {
            servers.push(McpServerRecord {
                agent,
                name: name.to_string(),
                scope: scope.to_string(),
                transport: infer_transport(spec),
                status: infer_status(spec),
                path: path.to_path_buf(),
            });
        }
    }
}

fn infer_transport(spec: &Value) -> String {
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
        .unwrap_or_else(|| "unknown".to_string())
}

fn infer_toml_transport(spec: &TomlValue) -> String {
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
        .unwrap_or_else(|| "unknown".to_string())
}

fn infer_status(spec: &Value) -> String {
    let disabled = spec
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let enabled = spec.get("enabled").and_then(Value::as_bool).unwrap_or(true);

    if disabled || !enabled {
        "disabled".to_string()
    } else {
        "configured".to_string()
    }
}

fn infer_toml_status(spec: &TomlValue) -> String {
    let disabled = spec
        .get("disabled")
        .and_then(TomlValue::as_bool)
        .unwrap_or(false);
    let enabled = spec
        .get("enabled")
        .and_then(TomlValue::as_bool)
        .unwrap_or(true);

    if disabled || !enabled {
        "disabled".to_string()
    } else {
        "configured".to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::scan_toml_mcp;
    use crate::skills::AgentKind;

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
"#,
        )
        .expect("write config");
        let mut servers = Vec::new();
        let mut warnings = Vec::new();

        scan_toml_mcp(
            &path,
            AgentKind::Codex,
            "global",
            &mut servers,
            &mut warnings,
        );

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].name, "node_repl");
        assert_eq!(servers[0].scope, "global");
        assert_eq!(servers[0].transport, "stdio");
        assert_eq!(servers[0].status, "configured");
        assert_eq!(servers[1].name, "remote");
        assert_eq!(servers[1].transport, "http");
        assert_eq!(servers[1].status, "disabled");
        let _ = fs::remove_dir_all(root);
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
    fn cursor_project_mcp_uses_project_directory_as_scope() {
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
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].scope, "project-alpha");
        let _ = fs::remove_dir_all(root);
    }
}
