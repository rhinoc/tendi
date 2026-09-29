use super::{
    AssistantMessage, AssistantStreamParser, AssistantUsage, build_prompt, extract_answer,
    extract_usage, run_command_with_timeout_streaming,
};
use serde_json::json;
use std::{
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex},
    time::Duration,
};

#[test]
fn prompt_contains_context_history_and_message() {
    let prompt = build_prompt(
        "How many tokens?",
        &[AssistantMessage {
            role: "user".to_string(),
            content: "Earlier".to_string(),
        }],
        &json!({"session":{"tokenUsage":{"totalTokens":42}}}),
    );
    assert!(prompt.contains("totalTokens"));
    assert!(prompt.contains("authoritative visible data"));
    assert!(prompt.contains("Earlier"));
    assert!(prompt.contains("How many tokens?"));
}

#[test]
fn extracts_json_and_jsonl_answers() {
    assert_eq!(
        extract_answer(r#"{"result":"done"}"#).as_deref(),
        Some("done")
    );
    assert_eq!(
        extract_answer("{\"type\":\"item.completed\",\"item\":{\"text\":\"done\"}}\n").as_deref(),
        Some("done")
    );
    assert_eq!(extract_answer(r#"{"type":"turn.completed"}"#), None);
}

#[test]
fn extracts_usage_from_nested_json() {
    let usage =
        extract_usage(r#"{"result":"done","usage":{"input_tokens":2,"output_tokens":3}}"#).unwrap();
    assert_eq!(usage.input_tokens, Some(2));
    assert_eq!(usage.output_tokens, Some(3));
}

#[test]
fn omits_unknown_usage_values_from_json() {
    assert_eq!(
        serde_json::to_value(AssistantUsage::default()).unwrap(),
        json!({})
    );
}

#[test]
fn streams_codex_agent_message_deltas_but_not_reasoning() {
    let mut parser = AssistantStreamParser::default();
    assert!(parser
            .events(
                "conversation",
                r#"{"type":"item.started","item":{"id":"message-1","type":"agent_message","text":""}}"#,
            )
            .iter()
            .all(|event| event.kind == "progress"));
    let events = parser.events(
        "conversation",
        r#"{"type":"item.delta","item_id":"reasoning-1","delta":"internal"}"#,
    );
    assert!(events.is_empty());
    let events = parser.events(
        "conversation",
        r#"{"type":"item.delta","item_id":"message-1","delta":"hello"}"#,
    );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "delta");
    assert_eq!(events[0].text.as_deref(), Some("hello"));
}

#[test]
fn streams_claude_and_cursor_text_events() {
    let mut parser = AssistantStreamParser::default();
    let events = parser.events(
            "conversation",
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hello"}}}"#,
        );
    assert_eq!(events[0].text.as_deref(), Some("hello"));
    let events = parser.events(
        "conversation",
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":" world"}]}}"#,
    );
    assert_eq!(events[0].text.as_deref(), Some(" world"));
}

#[test]
fn streams_codex_tool_calls_and_results() {
    let mut parser = AssistantStreamParser::default();
    let events = parser.events(
            "conversation",
            r#"{"type":"item.started","item":{"id":"tool-1","type":"command_execution","command":"printf fixture"}}"#,
        );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "tool-call");
    assert_eq!(events[0].detail.as_deref(), Some("Shell command"));
    assert_eq!(events[0].text.as_deref(), Some("printf fixture"));
    assert_eq!(events[0].tool_call_id.as_deref(), Some("tool-1"));

    let events = parser.events(
            "conversation",
            r#"{"type":"item.updated","item":{"id":"tool-1","type":"command_execution","aggregated_output":"fixture"}}"#,
        );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "tool-result");
    assert_eq!(events[0].text.as_deref(), Some("fixture"));
    assert_eq!(events[0].tool_call_id.as_deref(), Some("tool-1"));
}

#[test]
fn streams_claude_tool_calls_and_results() {
    let mut parser = AssistantStreamParser::default();
    let events = parser.events(
            "conversation",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"printf fixture"}}]}}"#,
        );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "tool-call");
    assert_eq!(events[0].detail.as_deref(), Some("Bash"));
    assert_eq!(events[0].text.as_deref(), Some("printf fixture"));
    assert_eq!(events[0].tool_call_id.as_deref(), Some("tool-1"));

    let events = parser.events(
            "conversation",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"fixture"}]}}"#,
        );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "tool-result");
    assert_eq!(events[0].detail.as_deref(), Some("Bash"));
    assert_eq!(events[0].text.as_deref(), Some("fixture"));
    assert_eq!(events[0].tool_call_id.as_deref(), Some("tool-1"));
}

#[test]
fn streams_claude_tool_input_json_deltas() {
    let mut parser = AssistantStreamParser::default();
    let events = parser.events(
            "conversation",
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool-1","name":"Bash","input":{}}}}"#,
        );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "tool-call");
    assert_eq!(events[0].detail.as_deref(), Some("Bash"));
    assert_eq!(events[0].text, None);
    assert_eq!(events[0].tool_call_id.as_deref(), Some("tool-1"));

    let events = parser.events(
            "conversation",
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"printf "}}}"#,
        );
    assert!(events.is_empty());

    let events = parser.events(
            "conversation",
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"fixture\"}"}}}"#,
        );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "tool-call");
    assert_eq!(events[0].detail.as_deref(), Some("Bash"));
    assert_eq!(events[0].text.as_deref(), Some("printf fixture"));
    assert_eq!(events[0].tool_call_id.as_deref(), Some("tool-1"));
}

#[test]
fn streams_cursor_tool_calls_from_role_events() {
    let mut parser = AssistantStreamParser::default();
    let events = parser.events(
            "conversation",
            r#"{"role":"assistant","message":{"content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{"path":"src/App.tsx"}}]}}"#,
        );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "tool-call");
    assert_eq!(events[0].detail.as_deref(), Some("Read"));
    assert!(
        events[0]
            .text
            .as_deref()
            .is_some_and(|text| text.contains("src/App.tsx"))
    );

    let events = parser.events(
            "conversation",
            r#"{"role":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"file contents"}]}}"#,
        );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "tool-result");
    assert_eq!(events[0].detail.as_deref(), Some("Read"));
    assert_eq!(events[0].text.as_deref(), Some("file contents"));
}

#[test]
fn forwards_output_lines_before_process_completion_and_cancels() {
    let cancellation = Arc::new(AtomicBool::new(false));
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink_cancellation = Arc::clone(&cancellation);
    let sink_lines = Arc::clone(&lines);
    let sink = Arc::new(move |line: &str| {
        sink_lines.lock().unwrap().push(line.to_string());
        sink_cancellation.store(true, Ordering::Release);
    });
    let mut command = Command::new("sh");
    command.args(["-c", "printf 'first\\n'; exec sleep 5"]);

    let output = run_command_with_timeout_streaming(
        command,
        &[],
        Duration::from_secs(1),
        Some(sink),
        Some(cancellation),
    )
    .unwrap();

    assert!(output.cancelled);
    assert_eq!(lines.lock().unwrap().as_slice(), ["first"]);
}

#[test]
fn resets_timeout_when_stdout_produces_output() {
    let mut command = Command::new("sh");
    command.args([
        "-c",
        "printf 'first'; sleep 1.5; printf 'second'; sleep 1.5",
    ]);

    let output =
        run_command_with_timeout_streaming(command, &[], Duration::from_millis(2_500), None, None)
            .unwrap();

    assert!(!output.timed_out);
    assert_eq!(output.stdout, "firstsecond");
}

#[test]
fn times_out_after_stdout_stops() {
    let mut command = Command::new("sh");
    command.args(["-c", "printf 'first'; sleep 1"]);

    let output =
        run_command_with_timeout_streaming(command, &[], Duration::from_millis(50), None, None)
            .unwrap();

    assert!(output.timed_out);
    assert_eq!(output.stdout, "first");
}
