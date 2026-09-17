use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    AgentProvider, CursorProvider, ProviderContext, cursor_event_timestamp, cursor_runtime_tools,
    merge_cursor_plugin_enrichment, parse_transcript,
};
use crate::mcp::{McpEnrichment, McpTool};
use crate::skills::AgentKind;
use rusqlite::Connection;
use serde_json::json;

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "tendi-cursor-rules-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    ))
}

#[test]
fn parses_embedded_cursor_timestamp_for_transcript_items() {
    let user = json!({
        "role": "user",
        "message": {
            "content": [{
                "type": "text",
                "text": "<timestamp>Thursday, Aug 27, 2026, 11:01 PM (UTC+8)</timestamp>\n<user_query>Why did sending fail?</user_query>"
            }]
        }
    });
    assert_eq!(
        cursor_event_timestamp(&user).as_deref(),
        Some("2026-08-27T23:01:00+08:00")
    );

    let mut items = Vec::new();
    parse_transcript(&user, &mut items);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].body, "Why did sending fail?");
    assert_eq!(items[0].time.as_deref(), Some("23:01"));
}

#[test]
fn scans_global_and_project_cursor_rules_with_their_own_scopes() {
    let root = temp_dir();
    let home = root.join("home");
    let project = root.join("project");
    let global_rules = home.join(".cursor/rules");
    let project_rules = project.join(".cursor/rules");
    fs::create_dir_all(global_rules.join("nested")).expect("create global rules");
    fs::create_dir_all(&project_rules).expect("create project rules");
    fs::write(global_rules.join("global.mdc"), "global rule").expect("write global rule");
    fs::write(global_rules.join("nested/deep.mdc"), "nested global rule")
        .expect("write nested global rule");
    fs::write(global_rules.join("ignored.md"), "not a Cursor rule")
        .expect("write ignored global rule");
    fs::write(project_rules.join("project.mdc"), "project rule").expect("write project rule");
    fs::write(project.join(".cursorrules"), "legacy project rule")
        .expect("write legacy project rule");
    fs::write(project.join("CLAUDE.md"), "Claude-compatible project rule")
        .expect("write Claude-compatible project rule");

    let context = ProviderContext {
        home: Some(home),
        project_dirs: vec![project.clone()],
    };
    let mut rules = Vec::new();
    let mut warnings = Vec::new();
    let mut order = 0;
    CursorProvider.scan_rules(&context, &mut rules, &mut warnings, &mut order);

    assert!(warnings.is_empty(), "warnings: {warnings:#?}");
    assert_eq!(
        rules
            .iter()
            .map(|rule| (
                rule.path.strip_prefix(&root).unwrap().to_path_buf(),
                rule.scope.as_str(),
                rule.agents.clone(),
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                PathBuf::from("home/.cursor/rules/global.mdc"),
                "global",
                vec![AgentKind::Cursor],
            ),
            (
                PathBuf::from("home/.cursor/rules/nested/deep.mdc"),
                "global",
                vec![AgentKind::Cursor],
            ),
            (
                PathBuf::from("project/CLAUDE.md"),
                "project",
                vec![AgentKind::Cursor],
            ),
            (
                PathBuf::from("project/.cursorrules"),
                "project",
                vec![AgentKind::Cursor],
            ),
            (
                PathBuf::from("project/.cursor/rules/project.mdc"),
                "project",
                vec![AgentKind::Cursor],
            ),
        ]
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scans_cursor_global_mcp_file() {
    let root = temp_dir();
    let home = root.join("home");
    let path = home.join(".cursor/mcp.json");
    fs::create_dir_all(path.parent().expect("mcp parent")).expect("create mcp directory");
    fs::write(
        &path,
        r#"{"mcpServers":{"global-server":{"command":"demo"}}}"#,
    )
    .expect("write global MCP");

    let context = ProviderContext {
        home: Some(home),
        project_dirs: Vec::new(),
    };
    let mut servers = Vec::new();
    let mut warnings = Vec::new();
    let mut probe_cache = crate::mcp::McpProbeCache::default();
    CursorProvider
        .scan_mcp(&context, &mut servers, &mut warnings, &mut probe_cache)
        .expect("scan Cursor MCP");

    assert!(warnings.is_empty(), "warnings: {warnings:#?}");
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "global-server");
    assert_eq!(servers[0].scope, "global");
    assert_eq!(servers[0].path, path);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn reads_cursor_runtime_tool_arguments_for_parameter_table() {
    let root = temp_dir();
    let metadata_path = root.join("SERVER_METADATA.json");
    let tools_dir = root.join("tools");
    fs::create_dir_all(&tools_dir).expect("create Cursor tool metadata directory");
    fs::write(
        &metadata_path,
        r#"{"serverIdentifier":"cursor-ide-browser"}"#,
    )
    .expect("write Cursor server metadata");
    fs::write(
        tools_dir.join("browser_click.json"),
        serde_json::to_string(&json!({
            "name": "browser_click",
            "arguments": {
                "type": "object",
                "properties": {"ref": {"type": "string"}},
                "required": ["ref"]
            }
        }))
        .expect("serialize Cursor tool metadata"),
    )
    .expect("write Cursor tool metadata");

    let tools = cursor_runtime_tools(&metadata_path);
    assert_eq!(
        tools[0]
            .input_schema
            .as_ref()
            .and_then(|schema| schema.pointer("/properties/ref/type"))
            .and_then(serde_json::Value::as_str),
        Some("string")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn preserves_cursor_parameter_descriptions_when_live_schema_omits_them() {
    let metadata = McpEnrichment {
        tools: vec![McpTool {
            name: "accessibility_action".to_string(),
            title: None,
            description: Some("Invoke an accessibility action".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "The action to perform"
                    }
                }
            })),
            icons: Vec::new(),
        }],
        ..McpEnrichment::default()
    };
    let live = McpEnrichment {
        probe_succeeded: true,
        tools: vec![McpTool {
            name: "accessibility_action".to_string(),
            title: None,
            description: None,
            input_schema: Some(json!({
                "type": "object",
                "properties": {"action": {"type": "string"}}
            })),
            icons: Vec::new(),
        }],
        ..McpEnrichment::default()
    };

    let merged = merge_cursor_plugin_enrichment(metadata, live);
    assert_eq!(
        merged.tools[0]
            .input_schema
            .as_ref()
            .and_then(|schema| schema.pointer("/properties/action/description"))
            .and_then(serde_json::Value::as_str),
        Some("The action to perform")
    );
}

#[test]
fn enriches_cursor_tool_items_with_store_arguments_and_results() {
    let root = temp_dir();
    fs::create_dir_all(&root).expect("create Cursor store directory");
    let store_path = root.join("store.db");
    let connection = Connection::open(&store_path).expect("open Cursor store");
    connection
        .execute_batch(
            "CREATE TABLE blobs (data BLOB); \
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);",
        )
        .expect("create Cursor store tables");

    let read_args = json!({ "path": "/tmp/example.txt" });
    let replace_args = json!({
        "path": "/tmp/example.txt",
        "old_string": "before",
        "new_string": "after"
    });
    let blobs = [
        json!({
            "role": "assistant",
            "content": [{
                "type": "tool-call",
                "toolCallId": "read-call",
                "toolName": "Read",
                "args": read_args
            }]
        }),
        json!({
            "role": "tool",
            "content": [{
                "type": "tool-result",
                "toolCallId": "read-call",
                "toolName": "Read",
                "result": "file contents"
            }]
        }),
        json!({
            "role": "assistant",
            "content": [{
                "type": "tool-call",
                "toolCallId": "replace-call",
                "toolName": "StrReplace",
                "args": replace_args
            }]
        }),
        json!({
            "role": "tool",
            "content": [{
                "type": "tool-result",
                "toolCallId": "replace-call",
                "toolName": "StrReplace",
                "result": "file updated"
            }]
        }),
    ];
    for blob in blobs {
        let data = blob.to_string();
        connection
            .execute("INSERT INTO blobs (data) VALUES (?1)", [data.as_bytes()])
            .expect("insert Cursor store blob");
    }
    drop(connection);

    let transcript = json!({
        "role": "assistant",
        "message": {
            "content": [
                {
                    "type": "tool_use",
                    "id": "read-call",
                    "name": "Read",
                    "input": { "path": "/tmp/example.txt" }
                },
                {
                    "type": "tool_use",
                    "id": "replace-call",
                    "name": "StrReplace",
                    "input": {
                        "path": "/tmp/example.txt",
                        "old_string": "before",
                        "new_string": "after"
                    }
                }
            ]
        }
    });
    let mut items = Vec::new();
    parse_transcript(&transcript, &mut items);

    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|item| item.command.is_some()));
    let stored_tool_calls =
        crate::providers::cursor_sessions::cursor_store_tool_calls_for_path(&store_path);
    assert_eq!(
        stored_tool_calls
            .iter()
            .map(|call| (call.id.as_str(), call.result.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            ("read-call", Some("file contents")),
            ("replace-call", Some("file updated")),
        ]
    );
    super::enrich_transcript_tools_from_store(&store_path, &mut items)
        .expect("enrich Cursor tool items");

    assert_eq!(items[0].call_id.as_deref(), Some("read-call"));
    assert_eq!(items[0].result.as_deref(), Some("file contents"));
    assert_eq!(items[1].call_id.as_deref(), Some("replace-call"));
    assert_eq!(items[1].result.as_deref(), Some("file updated"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(items[0].command.as_deref().unwrap())
            .expect("Read arguments JSON"),
        json!({ "path": "/tmp/example.txt" })
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(items[1].command.as_deref().unwrap())
            .expect("StrReplace arguments JSON"),
        json!({
            "path": "/tmp/example.txt",
            "old_string": "before",
            "new_string": "after"
        })
    );

    let _ = fs::remove_dir_all(root);
}
