use std::{
    fmt::Write as _,
    fs,
    io::Write as _,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;
use serde_json::{Value, json};

use super::{
    TranscriptSearchScopes, collect_shared_item, parse_search_transcript, parse_transcript,
    parse_transcript_locator_page, parse_transcript_page, parse_transcript_page_at_snapshot,
    search_transcript, summarize_tool_call, transcript_search_cache_offsets,
    transcript_source_version,
};

use crate::providers::{
    claude::collect_transcript_item as collect_claude_item,
    codex::collect_transcript_item as collect_codex_item,
    cursor::append_transcript_metadata_from_store_for_test as append_cursor_model_configs_from_store,
};

fn temp_path(prefix: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{}-{suffix}", std::process::id()))
}

fn cursor_model_blob(model: &str) -> Vec<u8> {
    json!({
        "role": "assistant",
        "content": [{
            "providerOptions": {
                "cursor": { "modelName": model }
            }
        }]
    })
    .to_string()
    .into_bytes()
}

fn codex_message(role: &str, body: &str) -> String {
    json!({
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": role,
            "content": [{ "type": "input_text", "text": body }]
        }
    })
    .to_string()
}

#[test]
fn codex_child_transcript_skips_inherited_history_before_start_ordinal() {
    let path = temp_path("tendi-codex-child-transcript-boundary-test.jsonl");
    let lines = [
        r#"{"ordinal":0,"type":"session_meta","payload":{"thread_source":"subagent","subagent_history_start_ordinal":3}}"#,
        r#"{"ordinal":1,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"parent context"}]}}"#,
        r#"{"ordinal":2,"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"parent answer"}]}}"#,
        r#"{"ordinal":3,"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"child answer"}]}}"#,
    ];
    fs::write(&path, lines.join("\n")).unwrap();

    let scan = parse_transcript(&path, crate::skills::AgentKind::Codex).unwrap();
    assert_eq!(
        scan.items
            .iter()
            .map(|item| item.body.as_str())
            .collect::<Vec<_>>(),
        ["child answer"]
    );

    let page =
        parse_transcript_page(&path, crate::skills::AgentKind::Codex, None, Some(1)).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].body, "child answer");
    assert!(page.locator_items.is_empty());

    let search = parse_search_transcript(&path, crate::skills::AgentKind::Codex).unwrap();
    assert_eq!(search.items.len(), 1);
    assert_eq!(search.items[0].body, "child answer");

    fs::remove_file(path).unwrap();
}

#[test]
fn codex_goal_message_becomes_user_transcript_item() {
    let path = temp_path("tendi-codex-goal-user-transcript-test.jsonl");
    fs::write(
            &path,
            [
                codex_message(
                    "user",
                    "<codex_internal_context source=\"goal\"><objective>Goal objective\nwith details</objective></codex_internal_context>",
                ),
                codex_message("assistant", "Started"),
            ]
            .join("\n"),
        )
        .unwrap();

    let scan = parse_transcript(&path, crate::skills::AgentKind::Codex).unwrap();

    assert_eq!(scan.items.len(), 2);
    assert_eq!(scan.items[0].kind, "user");
    assert_eq!(scan.items[0].body, "Goal objective\nwith details");
    assert_eq!(scan.items[1].kind, "assistant");
    assert_eq!(scan.items[1].body, "Started");

    fs::remove_file(path).unwrap();
}

#[test]
fn transcript_pages_use_backend_byte_cursors_without_duplicates() {
    let path = temp_path("tendi-transcript-page-test.jsonl");
    fs::write(
        &path,
        [
            codex_message("user", "one"),
            codex_message("assistant", "two"),
            codex_message("user", "three"),
            codex_message("assistant", "four"),
            codex_message("user", "five"),
        ]
        .join("\n"),
    )
    .unwrap();

    let first =
        parse_transcript_page(&path, crate::skills::AgentKind::Codex, None, Some(2)).unwrap();
    assert_eq!(first.items.len(), 2);
    assert!(first.locator_items.is_empty());
    let locator = parse_transcript_locator_page(&path, crate::skills::AgentKind::Codex).unwrap();
    assert_eq!(locator.locator_items.len(), 3);
    assert_eq!(locator.locator_items[0].index, 0);
    assert_eq!(locator.locator_items[0].label, "one");
    assert_eq!(locator.locator_items[0].response, "two");
    assert!(!first.done);
    let second = parse_transcript_page(
        &path,
        crate::skills::AgentKind::Codex,
        first.next_cursor.as_deref(),
        Some(2),
    )
    .unwrap();
    assert_eq!(second.items.len(), 2);
    assert!(!second.done);
    let third = parse_transcript_page(
        &path,
        crate::skills::AgentKind::Codex,
        second.next_cursor.as_deref(),
        Some(2),
    )
    .unwrap();
    assert_eq!(third.items.len(), 1);
    assert!(third.done);
    assert_eq!(
        first
            .items
            .iter()
            .chain(&second.items)
            .chain(&third.items)
            .map(|item| item.body.as_str())
            .collect::<Vec<_>>(),
        ["one", "two", "three", "four", "five"],
    );

    fs::remove_file(path).unwrap();
}

#[test]
fn transcript_locator_keeps_tool_result_with_tool_group() {
    let path = temp_path("tendi-transcript-locator-tool-test.jsonl");
    let call = json!({
        "type": "response_item",
        "payload": {
            "type": "function_call",
            "call_id": "call_locator",
            "name": "exec_command",
            "arguments": "{\"cmd\":\"cargo test\"}"
        }
    });
    let output = json!({
        "type": "response_item",
        "payload": {
            "type": "function_call_output",
            "call_id": "call_locator",
            "output": "passed"
        }
    });
    fs::write(
        &path,
        [
            codex_message("user", "one"),
            call.to_string(),
            output.to_string(),
            codex_message("assistant", "two"),
            codex_message("user", "three"),
            codex_message("assistant", "four"),
        ]
        .join("\n"),
    )
    .unwrap();

    let locator = parse_transcript_locator_page(&path, crate::skills::AgentKind::Codex).unwrap();

    assert_eq!(locator.locator_items.len(), 2);
    assert_eq!(locator.locator_items[0].index, 0);
    assert_eq!(locator.locator_items[0].label, "one");
    assert_eq!(locator.locator_items[0].response, "two");
    assert_eq!(locator.locator_items[1].index, 3);
    assert_eq!(locator.locator_items[1].label, "three");
    assert_eq!(locator.locator_items[1].response, "four");
    fs::remove_file(path).unwrap();
}

#[test]
fn transcript_locator_skips_codex_selected_skill_context() {
    let path = temp_path("tendi-transcript-locator-skill-context-test.jsonl");
    fs::write(
        &path,
        [
            codex_message(
                "user",
                "<skill>\n<name>datafinder</name>\n<path>/tmp/datafinder/SKILL.md</path>\n</skill>",
            ),
            codex_message("user", "one"),
            codex_message("assistant", "two"),
        ]
        .join("\n"),
    )
    .unwrap();

    let locator = parse_transcript_locator_page(&path, crate::skills::AgentKind::Codex).unwrap();

    assert_eq!(locator.locator_items.len(), 1);
    assert_eq!(locator.locator_items[0].index, 1);
    assert_eq!(locator.locator_items[0].label, "one");
    assert_eq!(locator.locator_items[0].response, "two");
    fs::remove_file(path).unwrap();
}

#[test]
fn transcript_cursor_preserves_utf8_line_boundaries() {
    let path = temp_path("tendi-transcript-page-utf8-offset-test.jsonl");
    fs::write(
        &path,
        [
            codex_message("user", "第一条🙂"),
            codex_message("assistant", "第二条之后"),
        ]
        .join("\n"),
    )
    .unwrap();

    let first =
        parse_transcript_page(&path, crate::skills::AgentKind::Codex, None, Some(1)).unwrap();
    let second = parse_transcript_page(
        &path,
        crate::skills::AgentKind::Codex,
        first.next_cursor.as_deref(),
        Some(1),
    )
    .unwrap();

    assert_eq!(first.items[0].body, "第一条🙂");
    assert_eq!(second.items[0].body, "第二条之后");
    assert!(second.done);
    fs::remove_file(path).unwrap();
}

#[test]
fn transcript_cursor_continues_when_the_file_appends() {
    let path = temp_path("tendi-transcript-page-append-test.jsonl");
    fs::write(
        &path,
        [
            codex_message("user", "one"),
            codex_message("assistant", "two"),
        ]
        .join("\n"),
    )
    .unwrap();
    let first =
        parse_transcript_page(&path, crate::skills::AgentKind::Codex, None, Some(1)).unwrap();
    assert!(!first.done);
    use std::io::Write;
    writeln!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "\n{}",
        codex_message("user", "three"),
    )
    .unwrap();

    let stale = parse_transcript_page(
        &path,
        crate::skills::AgentKind::Codex,
        first.next_cursor.as_deref(),
        Some(10),
    )
    .unwrap();

    assert!(!stale.restart_required);
    assert_eq!(
        stale
            .items
            .iter()
            .map(|item| item.body.as_str())
            .collect::<Vec<_>>(),
        ["two", "three"],
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn transcript_cursor_rejects_unbounded_source_prefix() {
    let path = temp_path("tendi-transcript-page-cursor-bounds-test.jsonl");
    fs::write(&path, codex_message("user", "one")).unwrap();
    let cursor = super::TranscriptCursor {
        offset: 0,
        line: 0,
        source: super::TranscriptSourceIdentity {
            device: 0,
            inode: 0,
            prefix_len: 10 * 1024 * 1024,
            prefix_hash: 0,
        },
        boundary_hash: 0,
        source_size: 0,
        source_modified_ns: 0,
    }
    .encode()
    .unwrap();

    let error = parse_transcript_page(
        &path,
        crate::skills::AgentKind::Codex,
        Some(&cursor),
        Some(1),
    )
    .unwrap_err();

    assert!(format!("{error:#}").contains("fields exceed their bounds"));
    fs::remove_file(path).unwrap();
}

#[test]
fn transcript_cursor_requests_restart_after_rewrite() {
    let path = temp_path("tendi-transcript-page-rewrite-test.jsonl");
    fs::write(
        &path,
        [
            codex_message("user", "original-one"),
            codex_message("assistant", "original-two"),
        ]
        .join("\n"),
    )
    .unwrap();
    let first =
        parse_transcript_page(&path, crate::skills::AgentKind::Codex, None, Some(1)).unwrap();
    fs::write(
        &path,
        [
            codex_message("user", "rewritten-one"),
            codex_message("assistant", "rewritten-two"),
        ]
        .join("\n"),
    )
    .unwrap();

    let stale = parse_transcript_page(
        &path,
        crate::skills::AgentKind::Codex,
        first.next_cursor.as_deref(),
        Some(10),
    )
    .unwrap();

    assert!(stale.restart_required);
    assert!(!stale.done);
    assert!(stale.items.is_empty());
    fs::remove_file(path).unwrap();
}

#[test]
fn transcript_page_drops_unmatched_cross_boundary_tool_result() {
    let path = temp_path("tendi-transcript-page-cross-tool-test.jsonl");
    let call = json!({
        "type": "response_item",
        "payload": {
            "type": "function_call",
            "call_id": "call_cross_page",
            "name": "exec_command",
            "arguments": "{\"cmd\":\"cargo test\"}"
        }
    });
    let output = json!({
        "type": "response_item",
        "payload": {
            "type": "function_call_output",
            "call_id": "call_cross_page",
            "output": "cross-page-result"
        }
    });
    let mut lines = vec![call.to_string()];
    lines.extend(
        (0..super::TRANSCRIPT_PAGE_MAX_SOURCE_LINES)
            .map(|index| json!({ "type": "metadata", "index": index }).to_string()),
    );
    lines.push(output.to_string());
    fs::write(&path, lines.join("\n")).unwrap();

    let first =
        parse_transcript_page(&path, crate::skills::AgentKind::Codex, None, Some(1)).unwrap();
    assert_eq!(first.items[0].call_id.as_deref(), Some("call_cross_page"));
    assert!(first.items[0].result.is_none());
    assert!(!first.done);
    let mut cursor = first.next_cursor;
    loop {
        let page = parse_transcript_page(
            &path,
            crate::skills::AgentKind::Codex,
            cursor.as_deref(),
            Some(1),
        )
        .unwrap();
        assert!(page.items.iter().all(|item| item.kind != "tool_result"));
        if page.done {
            break;
        }
        cursor = page.next_cursor;
        assert!(cursor.is_some());
    }

    fs::remove_file(path).unwrap();
}

#[test]
fn transcript_page_skips_an_oversized_line_with_a_warning() {
    let path = temp_path("tendi-transcript-page-large-line-test.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n{}",
            "x".repeat(super::TRANSCRIPT_PAGE_MAX_LINE_BYTES + 1),
            codex_message("user", "after-large-line"),
        ),
    )
    .unwrap();

    let page =
        parse_transcript_page(&path, crate::skills::AgentKind::Codex, None, Some(10)).unwrap();

    assert!(page.done);
    assert_eq!(page.items[0].body, "after-large-line");
    assert_eq!(page.warnings.len(), 1);
    assert!(page.warnings[0].contains("exceeds"));
    fs::remove_file(path).unwrap();
}

#[test]
fn transcript_page_reports_bad_lines_and_reaches_eof() {
    let path = temp_path("tendi-transcript-page-warning-test.jsonl");
    fs::write(
        &path,
        format!("not-json\n{}", codex_message("user", "valid")),
    )
    .unwrap();

    let page =
        parse_transcript_page(&path, crate::skills::AgentKind::Codex, None, Some(2)).unwrap();

    assert!(page.done);
    assert!(page.next_cursor.is_none());
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.warnings.len(), 1);
    assert!(page.warnings[0].contains(":1:"));
    fs::remove_file(path).unwrap();
}

#[test]
fn transcript_page_attaches_tool_result_within_same_page() {
    let path = temp_path("tendi-transcript-page-same-tool-test.jsonl");
    let call = json!({
        "type": "response_item",
        "payload": {
            "type": "function_call",
            "call_id": "call_same_page",
            "name": "exec_command",
            "arguments": "{\"cmd\":\"cargo test\"}"
        }
    });
    let output = json!({
        "type": "response_item",
        "payload": {
            "type": "function_call_output",
            "call_id": "call_same_page",
            "output": "passed"
        }
    });
    fs::write(&path, format!("{call}\n{output}")).unwrap();

    let page =
        parse_transcript_page(&path, crate::skills::AgentKind::Codex, None, Some(2)).unwrap();

    assert!(page.done);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].kind, "tool");
    assert_eq!(page.items[0].call_id.as_deref(), Some("call_same_page"));
    assert_eq!(page.items[0].result.as_deref(), Some("passed"));
    fs::remove_file(path).unwrap();
}

#[test]
fn search_transcript_skips_tool_results_and_keeps_messages() {
    let path = temp_path("tendi-search-transcript-test.jsonl");
    let large_output = "tool-output-only ".repeat(2_000);
    fs::write(
        &path,
        [
            json!({
                "type": "user",
                "message": { "role": "user", "content": "Find the CPU regression" }
            })
            .to_string(),
            json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "content": large_output
                    }]
                }
            })
            .to_string(),
            json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": "The scan was duplicated" }
            })
            .to_string(),
        ]
        .join("\n"),
    )
    .unwrap();

    let scan = parse_search_transcript(&path, crate::skills::AgentKind::Claude).unwrap();

    assert_eq!(scan.items.len(), 2);
    assert_eq!(scan.items[0].body, "Find the CPU regression");
    assert_eq!(scan.items[1].body, "The scan was duplicated");
    assert!(
        scan.items
            .iter()
            .all(|item| !item.body.contains("tool-output-only"))
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn transcript_search_reads_only_the_start_snapshot_when_source_appends() {
    let path = temp_path("tendi-transcript-search-snapshot-test.jsonl");
    let initial = format!("{}\n", codex_message("user", "initial message"));
    fs::write(&path, &initial).unwrap();
    let snapshot = super::transcript_search_snapshot(&path).unwrap();

    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(codex_message("user", "appended message").as_bytes())
        .unwrap();

    let page = parse_transcript_page_at_snapshot(
        &path,
        crate::skills::AgentKind::Codex,
        None,
        Some(10),
        &snapshot,
    )
    .unwrap();

    assert!(page.done);
    assert_eq!(page.source_version, snapshot.version());
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].body, "initial message");
    fs::remove_file(path).unwrap();
}

#[test]
fn transcript_search_reuses_unchanged_offset_index_and_invalidates_on_append() {
    let path = temp_path("tendi-transcript-search-offset-index-test.jsonl");
    let mut contents = String::new();
    for index in 0..900 {
        writeln!(
            contents,
            "{}",
            codex_message(
                "user",
                if index == 700 {
                    "needle-before-cache"
                } else {
                    "ordinary-message"
                },
            )
        )
        .unwrap();
    }
    fs::write(&path, contents).unwrap();

    let scopes = TranscriptSearchScopes {
        user: true,
        assistant: false,
        system: false,
        tool: false,
    };
    let first = search_transcript(
        &path,
        crate::skills::AgentKind::Codex,
        "needle-before-cache",
        &scopes,
    )
    .unwrap();
    let offsets = transcript_search_cache_offsets(
        &path,
        crate::skills::AgentKind::Codex,
        "needle-before-cache",
        &scopes,
        &first.source_version,
    )
    .unwrap();
    assert!(offsets.len() >= 3);
    assert_eq!(offsets.first().copied(), Some((0, offsets[0].1)));
    assert_eq!(
        offsets.last().map(|(_, end)| *end),
        Some(fs::metadata(&path).unwrap().len())
    );
    assert!(offsets.windows(2).all(|chunks| chunks[0].1 == chunks[1].0));

    let cached = search_transcript(
        &path,
        crate::skills::AgentKind::Codex,
        "needle-before-cache",
        &scopes,
    )
    .unwrap();
    assert_eq!(cached, first);

    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(format!("\n{}", codex_message("user", "needle-after-append")).as_bytes())
        .unwrap();
    let after_append = search_transcript(
        &path,
        crate::skills::AgentKind::Codex,
        "needle-before-cache",
        &scopes,
    )
    .unwrap();
    assert_eq!(after_append.hits, first.hits);
    assert_ne!(after_append.source_version, first.source_version);
    assert_eq!(
        transcript_source_version(&path).unwrap(),
        after_append.source_version
    );

    fs::remove_file(path).unwrap();
}

#[test]
fn inserts_cursor_model_markers_in_store_order() {
    let root = temp_path("tendi-cursor-model-history-test");
    fs::create_dir_all(&root).unwrap();
    let store_path = root.join("store.db");
    let connection = Connection::open(&store_path).unwrap();
    connection
        .execute("CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB)", [])
        .unwrap();
    for (index, model) in [
        "claude-fable-5-thinking-high",
        "claude-fable-5-thinking-high",
        "cursor-grok-4.5-high-fast",
    ]
    .iter()
    .enumerate()
    {
        connection
            .execute(
                "INSERT INTO blobs (id, data) VALUES (?1, ?2)",
                rusqlite::params![index.to_string(), cursor_model_blob(model)],
            )
            .unwrap();
    }
    drop(connection);

    let mut items = vec![
        super::TranscriptItem {
            kind: "user".to_string(),
            body: "start".to_string(),
            tag: None,
            time: None,
            command: None,
            result: None,
            duration_ms: None,
            linked_session_id: None,
            model: None,
            effort: None,
            call_id: None,
            started_at_ms: None,
        },
        super::TranscriptItem {
            kind: "assistant".to_string(),
            body: "first".to_string(),
            tag: None,
            time: None,
            command: None,
            result: None,
            duration_ms: None,
            linked_session_id: None,
            model: None,
            effort: None,
            call_id: None,
            started_at_ms: None,
        },
        super::TranscriptItem {
            kind: "assistant".to_string(),
            body: "second".to_string(),
            tag: None,
            time: None,
            command: None,
            result: None,
            duration_ms: None,
            linked_session_id: None,
            model: None,
            effort: None,
            call_id: None,
            started_at_ms: None,
        },
    ];

    append_cursor_model_configs_from_store(&store_path, &mut items);

    let markers = items
        .iter()
        .filter(|item| item.kind == "model_config")
        .collect::<Vec<_>>();
    assert_eq!(markers.len(), 2);
    assert_eq!(
        markers[0].model.as_deref(),
        Some("claude-fable-5-thinking-high")
    );
    assert_eq!(
        markers[1].model.as_deref(),
        Some("cursor-grok-4.5-high-fast")
    );
    assert_eq!(items[1].kind, "model_config");
    assert_eq!(items[3].kind, "model_config");
    assert!(items.iter().all(|item| item.time.is_none()));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn summarizes_codex_function_call_arguments_string() {
    let payload = json!({
        "type": "function_call",
        "name": "exec_command",
        "arguments": "{\"cmd\":\"rg -n \\\"needle\\\" src\"}"
    });

    assert_eq!(summarize_tool_call(&payload), "rg -n \"needle\" src");
}

#[test]
fn extracts_codex_special_tool_actions() {
    let web_search = json!({
        "type": "response_item",
        "payload": {
            "type": "web_search_call",
            "action": { "type": "search", "query": "Cursor transcript" }
        }
    });
    let image_generation = json!({
        "type": "response_item",
        "payload": {
            "type": "image_generation_call",
            "action": { "type": "image_generation", "prompt": "A small icon" }
        }
    });
    let mut items = Vec::new();

    collect_codex_item(&web_search, &mut items);
    collect_codex_item(&image_generation, &mut items);

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].tag.as_deref(), Some("web_search_call"));
    assert_eq!(
        serde_json::from_str::<Value>(items[0].command.as_deref().unwrap())
            .expect("web search action JSON"),
        json!({ "type": "search", "query": "Cursor transcript" })
    );
    assert_eq!(items[1].tag.as_deref(), Some("image_generation_call"));
    assert_eq!(
        serde_json::from_str::<Value>(items[1].command.as_deref().unwrap())
            .expect("image generation action JSON"),
        json!({ "type": "image_generation", "prompt": "A small icon" })
    );
}

#[test]
fn attaches_codex_function_call_output_and_duration() {
    let call = json!({
        "type": "response_item",
        "timestamp": "2026-06-19T10:11:12.000Z",
        "payload": {
            "type": "function_call",
            "call_id": "call_1",
            "name": "exec_command",
            "arguments": "{\"cmd\":\"cargo test\",\"workdir\":\"/tmp/project\"}"
        }
    });
    let output = json!({
        "type": "response_item",
        "timestamp": "2026-06-19T10:11:13.250Z",
        "payload": {
            "type": "function_call_output",
            "call_id": "call_1",
            "output": "Chunk ID: abc\nWall time: 1.245 seconds\nProcess exited with code 0\nOutput:\nok\n"
        }
    });
    let mut items = Vec::new();

    collect_codex_item(&call, &mut items);
    collect_codex_item(&output, &mut items);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, "tool");
    assert_eq!(items[0].body, "cargo test");
    assert_eq!(items[0].command.as_deref(), Some("cargo test"));
    assert_eq!(items[0].duration_ms, Some(1245));
    assert!(items[0].result.as_deref().unwrap_or("").contains("ok"));
}

#[test]
fn preserves_timezone_offsets_when_measuring_tool_duration() {
    let call = json!({
        "type": "response_item",
        "timestamp": "2026-08-28T11:49:00+08:00",
        "payload": {
            "type": "function_call",
            "call_id": "call_1",
            "name": "exec_command",
            "arguments": "{\"cmd\":\"pwd\"}"
        }
    });
    let output = json!({
        "type": "response_item",
        "timestamp": "2026-08-28T03:57:03.099Z",
        "payload": {
            "type": "function_call_output",
            "call_id": "call_1",
            "output": "done"
        }
    });
    let mut items = Vec::new();

    collect_codex_item(&call, &mut items);
    collect_codex_item(&output, &mut items);

    assert_eq!(items[0].duration_ms, Some(483_099));
}

#[test]
fn links_codex_spawn_agent_call_to_child_session() {
    let call = json!({
        "type": "response_item",
        "payload": {
            "type": "function_call",
            "name": "spawn_agent",
            "call_id": "call_spawn",
            "arguments": "{\"task_name\":\"child\"}"
        }
    });
    let activity = json!({
        "type": "event_msg",
        "payload": {
            "type": "sub_agent_activity",
            "event_id": "call_spawn",
            "kind": "started",
            "agent_thread_id": "child-session-id"
        }
    });
    let mut items = Vec::new();

    collect_codex_item(&call, &mut items);
    collect_codex_item(&activity, &mut items);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].tag.as_deref(), Some("spawn_agent"));
    assert_eq!(
        items[0].linked_session_id.as_deref(),
        Some("child-session-id")
    );
}

#[test]
fn renders_paired_codex_compaction_events_once() {
    let compacted = json!({
        "type": "compacted",
        "timestamp": "2026-07-24T06:19:25.075Z",
        "payload": { "replacement_history": [] }
    });
    let event = json!({
        "type": "event_msg",
        "timestamp": "2026-07-24T06:19:25.079Z",
        "payload": { "type": "context_compacted" }
    });
    let mut items = Vec::new();

    collect_codex_item(&compacted, &mut items);
    collect_codex_item(&event, &mut items);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, "compaction");
    assert_eq!(items[0].body, "Context compacted");
    assert_eq!(items[0].time.as_deref(), Some("2026-07-24T06:19:25.075Z"));
}

#[test]
fn renders_codex_model_config_only_when_it_changes() {
    let initial = json!({
        "type": "turn_context",
        "timestamp": "2026-07-24T03:50:12.778Z",
        "payload": { "model": "gpt-5.6-sol", "effort": "high" }
    });
    let duplicate = json!({
        "type": "event_msg",
        "timestamp": "2026-07-24T04:03:02.685Z",
        "payload": {
            "type": "thread_settings_applied",
            "thread_settings": {
                "model": "gpt-5.6-sol",
                "reasoning_effort": "high"
            }
        }
    });
    let changed = json!({
        "type": "turn_context",
        "timestamp": "2026-07-24T04:03:02.690Z",
        "payload": { "model": "gpt-5.6-sol", "effort": "xhigh" }
    });
    let mut items = Vec::new();

    collect_codex_item(&initial, &mut items);
    collect_codex_item(&duplicate, &mut items);
    collect_codex_item(&changed, &mut items);

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].kind, "model_config");
    assert_eq!(items[0].model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(items[0].effort.as_deref(), Some("high"));
    assert_eq!(items[1].body, "Model: gpt-5.6-sol\nEffort: xhigh");
}

#[test]
fn attaches_codex_custom_tool_call_output() {
    let call = json!({
        "type": "response_item",
        "payload": {
            "type": "custom_tool_call",
            "call_id": "call_1",
            "name": "exec",
            "input": { "command": "cargo test" }
        }
    });
    let output = json!({
        "type": "response_item",
        "payload": {
            "type": "custom_tool_call_output",
            "call_id": "call_1",
            "output": [{ "type": "text", "text": "all tests passed" }]
        }
    });
    let mut items = Vec::new();

    collect_codex_item(&call, &mut items);
    collect_codex_item(&output, &mut items);

    assert_eq!(items.len(), 1);
    assert!(
        items[0]
            .result
            .as_deref()
            .unwrap_or("")
            .contains("all tests passed")
    );
}

#[test]
fn extracts_codex_custom_tool_call_string_input_as_command() {
    let input = r#"const result = await tools.exec_command({cmd: "cargo test"});"#;
    let call = json!({
        "type": "response_item",
        "payload": {
            "type": "custom_tool_call",
            "call_id": "call_1",
            "name": "exec",
            "input": input
        }
    });
    let mut items = Vec::new();

    collect_codex_item(&call, &mut items);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].command.as_deref(), Some(input));
    assert_eq!(items[0].body, input);
}

#[test]
fn unmatched_tool_results_are_ignored() {
    let codex_output = json!({
        "type": "response_item",
        "payload": {
            "type": "function_call_output",
            "call_id": "missing-codex-call",
            "output": "orphaned"
        }
    });
    let claude_content_result = json!({
        "type": "user",
        "message": {
            "content": [{
                "type": "tool_result",
                "tool_use_id": "missing-claude-call",
                "content": "orphaned"
            }]
        }
    });
    let claude_legacy_result = json!({
        "type": "user",
        "toolUseID": "missing-claude-legacy-call",
        "toolUseResult": { "stdout": "orphaned" }
    });

    let mut items = Vec::new();
    collect_codex_item(&codex_output, &mut items);
    assert!(items.is_empty());

    collect_claude_item(&claude_content_result, &mut items);
    assert!(items.is_empty());

    collect_claude_item(&claude_legacy_result, &mut items);
    assert!(items.is_empty());
}

#[test]
fn tool_results_without_call_ids_do_not_bind_to_the_latest_tool() {
    let call = json!({
        "type": "response_item",
        "payload": {
            "type": "function_call",
            "call_id": "call-1",
            "name": "exec",
            "arguments": {"cmd": "true"}
        }
    });
    let result = json!({
        "type": "response_item",
        "payload": {
            "type": "function_call_output",
            "output": "should remain orphaned"
        }
    });
    let mut items = Vec::new();
    collect_codex_item(&call, &mut items);
    collect_codex_item(&result, &mut items);
    assert_eq!(items.len(), 1);
    assert!(items[0].result.is_none());
}

#[test]
fn uses_event_timestamps_when_wall_time_is_zero() {
    let call = json!({
        "type": "response_item",
        "timestamp": "2026-06-19T10:11:12.000Z",
        "payload": {
            "type": "function_call",
            "call_id": "call_1",
            "name": "exec_command",
            "arguments": "{\"cmd\":\"pwd\"}"
        }
    });
    let output = json!({
        "type": "response_item",
        "timestamp": "2026-06-19T10:11:12.180Z",
        "payload": {
            "type": "function_call_output",
            "call_id": "call_1",
            "output": "Chunk ID: abc\nWall time: 0.0000 seconds\nProcess exited with code 0\nOutput:\n/tmp\n"
        }
    });
    let mut items = Vec::new();

    collect_codex_item(&call, &mut items);
    collect_codex_item(&output, &mut items);

    assert_eq!(items[0].duration_ms, Some(180));
}

#[test]
fn keeps_full_codex_message_body() {
    let long_body = format!("{}tail-marker", "x".repeat(1_600));
    let value = json!({
        "type": "response_item",
        "timestamp": "2026-06-19T10:11:12.000Z",
        "payload": {
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": long_body }]
        }
    });
    let mut items = Vec::new();

    collect_codex_item(&value, &mut items);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].body, long_body);
    assert!(items[0].body.ends_with("tail-marker"));
}

#[test]
fn extracts_codex_reasoning_summary() {
    let value = json!({
        "type": "response_item",
        "timestamp": "2026-06-19T10:11:12.000Z",
        "payload": {
            "type": "reasoning",
            "summary": [{ "type": "summary_text", "text": "Need inspect parser." }]
        }
    });
    let mut items = Vec::new();

    collect_codex_item(&value, &mut items);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, "reasoning");
    assert_eq!(items[0].body, "Need inspect parser.");
}

#[test]
fn extracts_claude_tool_use_as_tool_item() {
    let value = json!({
        "type": "assistant",
        "timestamp": "2026-06-19T10:11:12.000Z",
        "message": {
            "content": [
                { "type": "text", "text": "I will inspect the file." },
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": {
                        "command": "cat src/main.rs",
                        "description": "Read main file"
                    }
                }
            ]
        }
    });
    let mut items = Vec::new();

    collect_claude_item(&value, &mut items);

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].kind, "assistant");
    assert_eq!(items[0].body, "I will inspect the file.");
    assert_eq!(items[1].kind, "tool");
    assert_eq!(items[1].body, "cat src/main.rs");
    assert_eq!(items[1].tag.as_deref(), Some("Bash"));
    assert_eq!(items[1].time.as_deref(), Some("2026-06-19T10:11:12.000Z"));
    assert_eq!(items[1].command.as_deref(), Some("cat src/main.rs"));
}

#[test]
fn extracts_claude_thinking_separately() {
    let value = json!({
        "type": "assistant",
        "timestamp": "2026-06-19T10:11:12.000Z",
        "message": {
            "content": [
                { "type": "thinking", "thinking": "Need inspect parser." },
                { "type": "text", "text": "I will inspect the file." }
            ]
        }
    });
    let mut items = Vec::new();

    collect_claude_item(&value, &mut items);

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].kind, "thinking");
    assert_eq!(items[0].body, "Need inspect parser.");
    assert_eq!(items[1].kind, "assistant");
    assert_eq!(items[1].body, "I will inspect the file.");
}

#[test]
fn attaches_claude_tool_result_without_user_message() {
    let call = json!({
        "type": "assistant",
        "timestamp": "2026-06-19T10:11:12.000Z",
        "message": {
            "content": [{
                "type": "tool_use",
                "id": "toolu_1",
                "name": "Bash",
                "input": { "command": "pwd" }
            }]
        }
    });
    let result = json!({
        "type": "user",
        "timestamp": "2026-06-19T10:11:13.250Z",
        "message": {
            "role": "user",
            "content": [{
                "tool_use_id": "toolu_1",
                "type": "tool_result",
                "content": "/tmp/project",
                "is_error": false
            }]
        },
        "toolUseResult": {
            "stdout": "/tmp/project",
            "stderr": "",
            "interrupted": false
        }
    });
    let mut items = Vec::new();

    collect_claude_item(&call, &mut items);
    collect_claude_item(&result, &mut items);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, "tool");
    assert_eq!(items[0].tag.as_deref(), Some("Bash"));
    assert_eq!(items[0].result.as_deref(), Some("/tmp/project"));
}

#[test]
fn classifies_claude_task_notifications_as_context() {
    let value = json!({
        "type": "user",
        "timestamp": "2026-06-19T10:11:12.000Z",
        "origin": { "kind": "task-notification" },
        "message": {
            "role": "user",
            "content": "<task-notification>\n<task-id>a2b6cf50aca587d06</task-id>\n<tool-use-id>toolu_01PnfnDLkqR6rLxGaJ8cPXRE</tool-use-id>\n<status>completed</status>\n</task-notification>"
        }
    });
    let mut items = Vec::new();

    collect_claude_item(&value, &mut items);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, "context");
    assert_eq!(items[0].tag.as_deref(), Some("Task notification"));
}

#[test]
fn extracts_cursor_message_content_and_tool_use() {
    let user = json!({
        "role": "user",
        "message": {
            "content": [{
                "type": "text",
                "text": "<user_info>Ryan</user_info>\n<timestamp>today</timestamp>\n<user_query>\nFix Cursor detail\n</user_query>"
            }]
        }
    });
    let assistant = json!({
        "role": "assistant",
        "message": {
            "content": [
                { "type": "text", "text": "I will inspect it." },
                {
                    "type": "tool_use",
                    "name": "Read",
                    "input": { "path": "src/App.jsx" }
                }
            ]
        }
    });
    let mut items = Vec::new();

    collect_shared_item(&user, &mut items);
    collect_shared_item(&assistant, &mut items);

    assert_eq!(items.len(), 3);
    assert_eq!(items[0].kind, "user");
    assert_eq!(items[0].body, "Fix Cursor detail");
    assert_eq!(items[1].kind, "assistant");
    assert_eq!(items[1].body, "I will inspect it.");
    assert_eq!(items[2].kind, "tool");
    assert_eq!(items[2].tag.as_deref(), Some("Read"));
    assert_eq!(
        serde_json::from_str::<Value>(items[2].command.as_deref().unwrap())
            .expect("Read arguments JSON"),
        json!({ "path": "src/App.jsx" })
    );
}

#[test]
fn ignores_timestamp_only_user_messages() {
    let value = json!({
        "role": "user",
        "message": {
            "content": [{
                "type": "text",
                "text": "<timestamp>Tuesday, Sep 1, 2026, 10:37 AM (UTC+8)</timestamp>"
            }]
        }
    });
    let mut items = Vec::new();

    collect_shared_item(&value, &mut items);

    assert!(items.is_empty());
}

#[test]
fn classifies_cursor_subagent_notifications_and_context() {
    let notification = json!({
        "role": "user",
        "message": {
            "content": [{
                "type": "text",
                "text": "<timestamp>today</timestamp>\n<user_query>Briefly inform the user about the task result and perform any follow-up actions (if needed).</user_query>"
            }]
        }
    });
    let notification_with_details = json!({
        "role": "user",
        "message": {
            "content": [{
                "type": "text",
                "text": "<timestamp>today</timestamp>\n<user_query>The beginning of the above subagent result is already visible to the user. Perform any follow-up actions (if needed). DO NOT repeat the same confirmation.</user_query>"
            }]
        }
    });
    let internal = json!({
        "role": "user",
        "message": {
            "content": [{
                "type": "text",
                "text": "<available_subagent_types>\nAvailable subagent_types: generalPurpose\n</available_subagent_types>"
            }]
        }
    });
    let user = json!({
        "role": "user",
        "message": {
            "content": [{
                "type": "text",
                "text": "<timestamp>today</timestamp>\n<user_query>开始</user_query>"
            }]
        }
    });
    let mut items = Vec::new();

    collect_shared_item(&notification, &mut items);
    collect_shared_item(&notification_with_details, &mut items);
    collect_shared_item(&internal, &mut items);
    collect_shared_item(&user, &mut items);

    assert_eq!(items.len(), 4);
    assert_eq!(items[0].kind, "notification");
    assert_eq!(items[0].tag.as_deref(), Some("Subagent"));
    assert_eq!(items[1].kind, "notification");
    assert_eq!(items[2].kind, "context");
    assert_eq!(items[2].tag.as_deref(), Some("Subagent types"));
    assert_eq!(items[3].kind, "user");
    assert_eq!(items[3].body, "开始");
}

#[test]
fn extracts_codex_internal_context_as_context_item() {
    let internal = json!({
        "type": "response_item",
        "timestamp": "2026-06-19T10:11:12.000Z",
        "payload": {
            "type": "message",
            "role": "user",
            "content": [
                {
                    "type": "input_text",
                    "text": "# AGENTS.md instructions\n\n<INSTRUCTIONS>hidden</INSTRUCTIONS>"
                },
                {
                    "type": "input_text",
                    "text": "<recommended_plugins>\nplugin metadata\n</recommended_plugins>"
                }
            ]
        }
    });
    let user = json!({
        "type": "response_item",
        "timestamp": "2026-06-19T10:12:12.000Z",
        "payload": {
            "type": "message",
            "role": "user",
            "content": [
                {
                    "type": "input_text",
                    "text": "What happened in this session?"
                }
            ]
        }
    });
    let mut items = Vec::new();

    collect_codex_item(&internal, &mut items);
    collect_codex_item(&user, &mut items);

    assert_eq!(items.len(), 3);
    assert_eq!(items[0].kind, "context");
    assert_eq!(items[0].tag.as_deref(), Some("AGENTS.md"));
    assert!(
        items[0]
            .body
            .contains("<INSTRUCTIONS>hidden</INSTRUCTIONS>")
    );
    assert_eq!(items[1].kind, "context");
    assert_eq!(items[1].tag.as_deref(), Some("Recommended plugins"));
    assert!(items[1].body.contains("plugin metadata"));
    assert_eq!(items[2].kind, "user");
    assert_eq!(items[2].body, "What happened in this session?");
    assert_eq!(items[2].time.as_deref(), Some("2026-06-19T10:12:12.000Z"));
}

#[test]
fn splits_embedded_codex_lifecycle_context_from_user_text() {
    let value = json!({
        "type": "response_item",
        "timestamp": "2026-06-19T10:11:12.000Z",
        "payload": {
            "type": "message",
            "role": "user",
            "content": "Real request\n<subagent_notification>\n{\"status\":\"shutdown\"}\n</subagent_notification>\n<turn_aborted>\nThe user interrupted the previous turn on purpose.\n</turn_aborted>\n<in-app-browser-context source=\"ambient-ui-state\">\nUI state\n</in-app-browser-context>\nNext request"
        }
    });
    let mut items = Vec::new();

    collect_codex_item(&value, &mut items);

    assert_eq!(items.len(), 5);
    assert_eq!(items[0].kind, "user");
    assert_eq!(items[0].body, "Real request");
    assert_eq!(items[1].kind, "context");
    assert_eq!(items[1].tag.as_deref(), Some("Subagent"));
    assert_eq!(items[2].kind, "context");
    assert_eq!(items[2].tag.as_deref(), Some("Turn aborted"));
    assert_eq!(items[3].kind, "context");
    assert_eq!(items[3].tag.as_deref(), Some("Browser context"));
    assert_eq!(items[4].kind, "user");
    assert_eq!(items[4].body, "Next request");
}
