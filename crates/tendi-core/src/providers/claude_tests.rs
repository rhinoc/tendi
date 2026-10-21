use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{AgentProvider, ClaudeProvider, ProviderContext};
use crate::{analytics::AnalyticsCapabilities, skills::SkillVisibility};

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "tendi-claude-mcp-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    ))
}

#[test]
fn assistant_ask_enables_claude_streaming_output() {
    let workspace = PathBuf::from("/tmp/tendi-claude-stream-test");
    let command = ClaudeProvider
        .assistant_ask_command(&workspace, "prompt")
        .expect("Claude assistant command");

    assert!(command.args.iter().any(|arg| arg == "--verbose"));
    assert!(
        command
            .args
            .iter()
            .any(|arg| arg == "--include-partial-messages")
    );
}

#[test]
fn scans_personal_claude_json_mcp_file() {
    let root = temp_dir();
    let home = root.join("home");
    let path = home.join(".claude.json");
    fs::create_dir_all(&home).expect("create home");
    fs::write(
        &path,
        r#"{"mcpServers":{"personal-server":{"command":"demo"}}}"#,
    )
    .expect("write personal MCP");

    let context = ProviderContext {
        home: Some(home),
        project_dirs: Vec::new(),
    };
    let mut servers = Vec::new();
    let mut warnings = Vec::new();
    let mut probe_cache = crate::mcp::McpProbeCache::default();
    ClaudeProvider
        .scan_mcp(&context, &mut servers, &mut warnings, &mut probe_cache)
        .expect("scan Claude MCP");

    assert!(warnings.is_empty(), "warnings: {warnings:#?}");
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "personal-server");
    assert_eq!(servers[0].scope, "global");
    assert_eq!(servers[0].path, path);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scans_project_mcp_servers_nested_in_personal_claude_json() {
    let root = temp_dir();
    let home = root.join("home");
    let path = home.join(".claude.json");
    fs::create_dir_all(&home).expect("create home");
    fs::write(
        &path,
        r#"{
  "mcpServers": {"personal": {"command": "demo"}},
  "projects": {
    "/work/demo": {"mcpServers": {"project-server": {"command": "project"}}}
  }
}"#,
    )
    .expect("write Claude state");

    let context = ProviderContext {
        home: Some(home),
        project_dirs: Vec::new(),
    };
    let mut servers = Vec::new();
    let mut warnings = Vec::new();
    let mut probe_cache = crate::mcp::McpProbeCache::default();
    ClaudeProvider
        .scan_mcp(&context, &mut servers, &mut warnings, &mut probe_cache)
        .expect("scan Claude MCP");

    assert!(warnings.is_empty(), "warnings: {warnings:#?}");
    assert_eq!(servers.len(), 2);
    let project_server = servers
        .iter()
        .find(|server| server.name == "project-server")
        .expect("nested project server");
    assert_eq!(project_server.scope, "/work/demo");
    assert_eq!(
        project_server.server_path,
        vec!["projects", "/work/demo", "mcpServers"]
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn maps_claude_skill_invocation_frontmatter_to_visibility() {
    let frontmatter: serde_yaml::Value =
        serde_yaml::from_str("name: demo\ndisable-model-invocation: true\n")
            .expect("parse skill frontmatter");
    let metadata = ClaudeProvider
        .skill_visibility_metadata(
            PathBuf::from(".claude/skills/demo").as_path(),
            PathBuf::from(".claude/skills/demo/SKILL.md").as_path(),
            Some(&frontmatter),
        )
        .expect("read Claude skill metadata");

    assert_eq!(metadata.disable_model_invocation, Some(true));
    assert_eq!(metadata.provider_visibility, SkillVisibility::Manual);
}

#[test]
fn exposes_claude_token_usage_analytics_capability() {
    assert_eq!(
        ClaudeProvider.analytics_capabilities(),
        AnalyticsCapabilities {
            token_usage: true,
            reasoning_tokens: false,
            explicit_runs: false,
            duration: true,
            rate_limit_history: false,
        }
    );
}

#[test]
fn ignores_synthetic_model_in_session_metadata() {
    let value = serde_json::json!({
        "type": "assistant",
        "message": { "model": "<synthetic>" }
    });
    let mut metadata = crate::sessions::SessionMetadata::default();
    let mut deduplicated_usage = BTreeMap::new();

    ClaudeProvider.update_session_metadata(&value, &mut metadata, &mut deduplicated_usage);

    assert_eq!(metadata.model, None);
}

#[cfg(target_os = "macos")]
#[test]
fn ownership_registry_rejects_reused_and_exited_pids() {
    let pid = std::process::id() as i32;
    let start = crate::session_ownership::processes::start(pid).unwrap();
    let stamp = chrono::DateTime::from_timestamp(start as i64, 0).unwrap().format("%a %b %e %H:%M:%S %Y").to_string();
    let mut value = serde_json::json!({"pid":pid,"procStart":stamp,"sessionId":"live-session"});
    assert_eq!(super::held_registry_session(&value).as_deref(), Some("live-session"));
    value["sessionId"] = serde_json::json!("switched-session");
    assert_eq!(super::held_registry_session(&value).as_deref(), Some("switched-session"));
    value["procStart"] = serde_json::json!("Thu Jan  1 00:00:00 1970");
    assert!(super::held_registry_session(&value).is_none());
    value["procStart"] = serde_json::json!(stamp);
    value["pid"] = serde_json::json!(i32::MAX);
    assert!(super::held_registry_session(&value).is_none());
}
