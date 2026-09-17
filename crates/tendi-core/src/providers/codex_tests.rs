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
    let expected =
        "interface:\n  display_name: \"Wait What\"\npolicy:\n  allow_implicit_invocation: false\n";

    assert_eq!(
        render_codex_policy(Some(before), SkillVisibility::Manual).unwrap(),
        expected
    );
}

#[test]
fn codex_skill_config_update_preserves_crlf_line_endings() {
    let before = "# keep\r\n[other]\r\nvalue = 'keep'\r\n";
    let after =
        super::render_codex_skill_config(before, std::path::Path::new("/tmp/demo/SKILL.md"), true)
            .unwrap();

    assert!(after.contains("value = 'keep'\r\n"));
    assert!(after.contains("enabled = true\r\n"));
    assert!(!after.replace("\r\n", "").contains('\n'));
}

#[test]
fn codex_skill_config_rewrite_removes_stale_legacy_entry() {
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

    let after = super::rewrite_codex_skill_config_paths(&before, &legacy_root, &canonical_root)
        .expect("rewrite Codex skill config");
    let value = toml::from_str::<TomlValue>(&after).expect("parse rewritten config");
    let configs = value
        .get("skills")
        .and_then(|skills| skills.get("config"))
        .and_then(TomlValue::as_array)
        .expect("rewritten skill config entries");
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
        super::parse_codex_mcp_keyring_token("figma", "https://mcp.figma.com/mcp", &serialized,)
            .as_deref(),
        Some("access-token")
    );
}
