use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    HookReviewRequest, HookScan, HookSourceMatch, apply_tendi_hook_review_states, hook_matches_id,
    hook_record_id, hook_review_identity, merge_hook_entry, read_hook_entry, scan_hook_file,
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
fn hook_record_id_is_stable_for_state_changes_and_changes_for_structure() {
    let root = std::env::temp_dir().join(format!(
        "tendi-hook-id-{}-{}",
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
    "PreToolUse": [{
      "matcher": "Bash",
      "hooks": [{ "type": "command", "command": "/bin/echo one" }]
    }]
  }
}"#,
    )
    .expect("write hooks");

    let mut hooks = Vec::new();
    let mut warnings = Vec::new();
    scan_hook_file(&path, AgentKind::Codex, &mut hooks, &mut warnings);
    assert!(warnings.is_empty(), "{warnings:?}");
    let hook = hooks.into_iter().next().expect("scan one hook");
    let id = hook_record_id(&hook);
    assert!(hook_matches_id(&hook, &id));

    let mut state_changed = hook.clone();
    state_changed.enabled = !state_changed.enabled;
    state_changed.trust_hash = "sha256:changed".to_string();
    assert_eq!(hook_record_id(&state_changed), id);
    assert!(hook_matches_id(&state_changed, &id));

    let mut handler_changed = hook.clone();
    handler_changed.command = Some("/bin/echo two".to_string());
    assert_ne!(hook_record_id(&handler_changed), id);

    let mut event_changed = hook.clone();
    event_changed.event = "PostToolUse".to_string();
    assert_ne!(hook_record_id(&event_changed), id);

    let mut path_changed = hook;
    path_changed.path = root.join("other-hooks.json");
    assert_ne!(hook_record_id(&path_changed), id);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn syncs_one_json_hook_without_dropping_other_hooks() {
    let root = std::env::temp_dir().join(format!(
        "tendi-hooks-sync-entry-{}-{}",
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
  "other": true,
  "hooks": {
    "PreToolUse": [
      {"matcher": "Bash", "hooks": [{"type": "command", "command": "one"}]},
      {"matcher": "Read", "hooks": [{"type": "command", "command": "two"}]}
    ]
  }
}"#,
    )
    .expect("write hooks");
    let identity = HookSourceMatch {
        event: "PreToolUse".to_string(),
        matcher: Some("Bash".to_string()),
        hook_type: Some("command".to_string()),
        command: Some("one".to_string()),
        url: None,
        prompt: None,
        filter: None,
        status_message: None,
        enabled: Some(true),
    };

    let entry = read_hook_entry(&path, &identity).expect("read selected hook");
    assert_eq!(entry["hook"]["command"], "one");
    let merged = merge_hook_entry(
        &path,
        &identity,
        &serde_json::json!({
            "event": "PreToolUse",
            "matcher": "Bash",
            "enabled": true,
            "hook": {"type": "command", "command": "updated"}
        }),
    )
    .expect("merge selected hook");
    let value = serde_json::from_str::<serde_json::Value>(&merged).expect("parse hooks");

    assert_eq!(value["other"], true);
    assert_eq!(
        value["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "updated"
    );
    assert_eq!(
        value["hooks"]["PreToolUse"][1]["hooks"][0]["command"],
        "two"
    );
    assert_eq!(
        merged,
        r#"{
  "other": true,
  "hooks": {
    "PreToolUse": [
      {"matcher": "Bash", "hooks": [{"type": "command", "command": "updated"}]},
      {"matcher": "Read", "hooks": [{"type": "command", "command": "two"}]}
    ]
  }
}"#
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn syncs_one_toml_hook_without_reformatting_other_entries() {
    let root = std::env::temp_dir().join(format!(
        "tendi-hooks-sync-toml-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("create temp root");
    let path = root.join("config.toml");
    let source = "# keep\r\n[[hooks.PreToolUse]]\r\nmatcher = 'Bash'\r\n\r\n[[hooks.PreToolUse.hooks]]\r\ntype = 'command'\r\ncommand = 'one'\r\n\r\n[[hooks.PreToolUse]]\r\nmatcher = 'Read'\r\n\r\n[[hooks.PreToolUse.hooks]]\r\ntype = 'command'\r\ncommand = 'two'\r\n";
    fs::write(&path, source).expect("write hooks");
    let identity = HookSourceMatch {
        event: "PreToolUse".to_string(),
        matcher: Some("Bash".to_string()),
        hook_type: Some("command".to_string()),
        command: Some("one".to_string()),
        url: None,
        prompt: None,
        filter: None,
        status_message: None,
        enabled: Some(true),
    };

    let merged = merge_hook_entry(
        &path,
        &identity,
        &serde_json::json!({
            "event": "PreToolUse",
            "matcher": "Bash",
            "enabled": true,
            "hook": {"type": "command", "command": "updated"}
        }),
    )
    .expect("merge selected hook");

    assert_eq!(
        merged,
        source.replace("command = 'one'", "command = \"updated\"")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn syncs_markdown_hook_without_reformatting_unrelated_frontmatter() {
    let root = std::env::temp_dir().join(format!(
        "tendi-hooks-sync-markdown-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("create temp root");
    let path = root.join("hooks.md");
    let source = "---\r\nname: demo\r\ndescription: \"keep this\"\r\nother: [one, two]\r\nhooks:\r\n  PreToolUse:\r\n    - matcher: Bash\r\n      hooks:\r\n        - type: command\r\n          command: one\r\n---\r\n\r\n# body\r\n";
    fs::write(&path, source).expect("write hooks");
    let identity = HookSourceMatch {
        event: "PreToolUse".to_string(),
        matcher: Some("Bash".to_string()),
        hook_type: Some("command".to_string()),
        command: Some("one".to_string()),
        url: None,
        prompt: None,
        filter: None,
        status_message: None,
        enabled: Some(true),
    };

    let merged = merge_hook_entry(
        &path,
        &identity,
        &serde_json::json!({
            "event": "PreToolUse",
            "matcher": "Bash",
            "enabled": true,
            "hook": {"type": "command", "command": "updated"}
        }),
    )
    .expect("merge markdown hook");

    assert!(merged.contains("name: demo\r\ndescription: \"keep this\""));
    assert!(merged.contains("other: [one, two]"));
    assert!(merged.contains("command: updated"));
    assert!(merged.contains("---\r\n\r\n# body\r\n"));
    assert!(!merged.replace("\r\n", "").contains('\n'));
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
fn writes_codex_trusted_hash_without_changing_crlf_line_endings() {
    let root = std::env::temp_dir().join(format!(
        "tendi-codex-crlf-state-{}-{}",
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
            "[hooks.state.\"/tmp/hooks.json:pre_tool_use:0:0\"]\r\ntrusted_hash = \"sha256:old\"\r\n\r\n[notice]\r\nhide = true\r\n",
        )
        .expect("write config");

    crate::providers::codex::write_trusted_hash(
        &path,
        "/tmp/hooks.json:pre_tool_use:0:0",
        "sha256:new",
    )
    .expect("write trusted hash");
    let text = fs::read_to_string(&path).expect("read config");
    assert!(text.contains("trusted_hash = \"sha256:new\"\r\n"));
    assert!(text.contains("[notice]\r\nhide = true\r\n"));
    assert!(!text.replace("\r\n", "").contains('\n'));
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
    assert!(
        error
            .to_string()
            .contains("this hook type does not support review")
    );

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
fn scans_codex_session_end_hooks() {
    let root = std::env::temp_dir().join(format!(
        "tendi-hooks-session-end-{}-{}",
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
[[hooks.SessionEnd]]
[[hooks.SessionEnd.hooks]]
type = "command"
command = "/bin/echo session-ended"
timeout = 2
"#,
    )
    .expect("write hooks");
    let mut hooks = Vec::new();
    let mut warnings = Vec::new();

    scan_codex_config_hooks(&path, &mut hooks, &mut warnings);

    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0].event, "SessionEnd");
    assert_eq!(hooks[0].command.as_deref(), Some("/bin/echo session-ended"));
    assert_eq!(
        hooks[0].provider_current_hash,
        crate::providers::codex::hook_current_hash(
            "SessionEnd",
            None,
            "/bin/echo session-ended",
            2,
            false,
            None,
            None,
        ),
    );
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
fn deletes_hooks_from_multiple_json_event_members() {
    let source = r#"{
  "hooks": {
    "Keep": [{ "type": "command", "command": "keep" }],
    "DeleteBeforeLast": [{ "type": "command", "command": "before-last" }],
    "DeleteLast": [{ "type": "command", "command": "last" }]
  }
}"#;
    let request = |event: &str, command: &str| super::HookDeleteRequest {
        agent: AgentKind::Codex,
        path: PathBuf::from("hooks.json"),
        expected_trust_hash: String::new(),
        event: event.to_string(),
        matcher: None,
        hook_type: Some("command".to_string()),
        command: Some(command.to_string()),
        url: None,
        prompt: None,
        filter: None,
        status_message: None,
    };

    let result = super::delete_json_hooks(
        &[
            request("DeleteBeforeLast", "before-last"),
            request("DeleteLast", "last"),
        ],
        source,
    )
    .expect("delete hooks");

    assert_eq!(
        result,
        r#"{
  "hooks": {
    "Keep": [{ "type": "command", "command": "keep" }]
  }
}"#
    );
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
    let source = r#"{
  "hooks": {
    "PreToolUse": [
      {
        "type": "command",
        "command": "/bin/echo toggle-me"
      }
    ]
  }
}"#;
    fs::write(&path, source).expect("write hooks");
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

    assert_eq!(
        fs::read_to_string(&path).expect("read updated hooks"),
        source.replace(
            "        \"command\": \"/bin/echo toggle-me\"",
            "        \"command\": \"/bin/echo toggle-me\",\n        \"enabled\": false",
        )
    );

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
    let source = r#"
[[hooks.PreToolUse]]
matcher = "Bash"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "/bin/echo toggle-me"
enabled = false
"#;
    fs::write(&path, source).expect("write hooks");
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

    assert_eq!(
        fs::read_to_string(&path).expect("read updated hooks"),
        source.replace("enabled = false", "enabled = true")
    );

    let mut hooks = Vec::new();
    let mut warnings = Vec::new();
    scan_codex_config_hooks(&path, &mut hooks, &mut warnings);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(hooks.len(), 1);
    assert!(hooks[0].enabled);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn batch_toggles_codex_hooks_in_one_toml_source() {
    let root = std::env::temp_dir().join(format!(
        "tendi-hook-batch-toggle-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("config.toml");
    fs::write(
        &path,
        r#"
[[hooks.PreToolUse]]
matcher = "Bash"
[[hooks.PreToolUse.hooks]]
type = "command"
command = "/bin/echo one"
enabled = false
[[hooks.PreToolUse.hooks]]
type = "command"
command = "/bin/echo two"
enabled = false
"#,
    )
    .unwrap();
    let expected_trust_hash = super::sha256_file(&path).unwrap();
    let requests = ["/bin/echo one", "/bin/echo two"]
        .into_iter()
        .map(|command| super::HookSetEnabledRequest {
            agent: AgentKind::Codex,
            path: path.clone(),
            expected_trust_hash: expected_trust_hash.clone(),
            event: "PreToolUse".to_string(),
            matcher: Some("Bash".to_string()),
            hook_type: Some("command".to_string()),
            command: Some(command.to_string()),
            url: None,
            prompt: None,
            filter: None,
            status_message: None,
            enabled: true,
        })
        .collect();
    super::set_hooks_enabled(requests).unwrap();
    let mut hooks = Vec::new();
    let mut warnings = Vec::new();
    scan_codex_config_hooks(&path, &mut hooks, &mut warnings);
    assert!(warnings.is_empty());
    assert_eq!(hooks.len(), 2);
    assert!(hooks.iter().all(|hook| hook.enabled));
    fs::remove_dir_all(root).unwrap();
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
