use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::generated::runtime_contract::AssistantStreamEvent;
use crate::{SessionCommand, parse_agent, plan_assistant_ask};

const ASSISTANT_OUTPUT_IDLE_TIMEOUT: Duration = Duration::from_secs(3 * 60);
const MAX_ASSISTANT_PROMPT_BYTES: usize = 120_000;

pub type AssistantStreamSink = Arc<dyn Fn(AssistantStreamEvent) + Send + Sync>;
pub type CommandOutputLineSink = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssistantSessionLink {
    pub id: String,
    pub agent: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssistantChatSession {
    pub id: String,
    pub messages: Vec<AssistantMessage>,
    pub linked_session: Option<AssistantSessionLink>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantAskRequest {
    pub conversation_id: String,
    pub request_id: String,
    pub message: String,
    #[serde(default)]
    pub history: Vec<AssistantMessage>,
    pub context: Value,
    pub agent: String,
    #[serde(default)]
    pub workspace: String,
    pub persist_user_message: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantAskResponse {
    pub answer: String,
    pub status: String,
    pub usage: AssistantUsage,
    pub error: Option<String>,
}

impl AssistantAskResponse {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            answer: String::new(),
            status: "error".to_string(),
            usage: AssistantUsage::default(),
            error: Some(message.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcessOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
}

#[derive(Clone)]
struct OutputActivity {
    clock: Instant,
    last_output_ms: Arc<AtomicU64>,
}

impl OutputActivity {
    fn new() -> Self {
        Self {
            clock: Instant::now(),
            last_output_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    fn mark_output(&self) {
        self.last_output_ms
            .store(self.clock.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    fn idle_for(&self) -> Duration {
        self.clock.elapsed().saturating_sub(Duration::from_millis(
            self.last_output_ms.load(Ordering::Relaxed),
        ))
    }
}

pub fn run_command_with_timeout(
    command: Command,
    input: &[u8],
    timeout: Duration,
) -> Result<ProcessOutput> {
    run_command_with_timeout_streaming(command, input, timeout, None, None)
}

pub fn run_command_with_timeout_streaming(
    mut command: Command,
    input: &[u8],
    timeout: Duration,
    stdout_line_sink: Option<CommandOutputLineSink>,
    cancellation: Option<Arc<AtomicBool>>,
) -> Result<ProcessOutput> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn assistant process")?;

    let stdin = child.stdin.take();
    let stdin_input = input.to_vec();
    let stdin_thread = thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(&stdin_input);
        }
    });
    let stdout = child.stdout.take().context("capture assistant stdout")?;
    let stderr = child.stderr.take().context("capture assistant stderr")?;
    let output_activity = OutputActivity::new();
    let stdout_output_activity = output_activity.clone();
    let stdout_thread = thread::spawn(move || {
        read_bytes_streaming(stdout, stdout_line_sink, Some(stdout_output_activity))
    });
    let stderr_thread = thread::spawn(move || read_bytes(stderr));

    let (status, timed_out, cancelled) = loop {
        match child.try_wait().context("wait for assistant process")? {
            Some(status) => break (status, false, false),
            None if cancellation
                .as_ref()
                .is_some_and(|token| token.load(Ordering::Acquire)) =>
            {
                let _ = child.kill();
                let status = child
                    .wait()
                    .context("wait for cancelled assistant process")?;
                break (status, false, true);
            }
            None if output_activity.idle_for() < timeout => {
                thread::sleep(Duration::from_millis(25))
            }
            None => {
                let _ = child.kill();
                let status = child
                    .wait()
                    .context("wait for timed-out assistant process")?;
                break (status, true, false);
            }
        }
    };

    let _ = stdin_thread.join();
    let stdout = stdout_thread
        .join()
        .map_err(|_| anyhow::anyhow!("assistant stdout reader panicked"))??;
    let stderr = stderr_thread
        .join()
        .map_err(|_| anyhow::anyhow!("assistant stderr reader panicked"))??;
    Ok(ProcessOutput {
        success: status.success(),
        code: status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        timed_out,
        cancelled,
    })
}

fn read_bytes(mut reader: impl Read) -> Result<Vec<u8>> {
    read_bytes_streaming(&mut reader, None, None)
}

fn read_bytes_streaming(
    mut reader: impl Read,
    line_sink: Option<CommandOutputLineSink>,
    output_activity: Option<OutputActivity>,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut line = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = reader.read(&mut chunk).context("read assistant output")?;
        if read == 0 {
            if !line.is_empty() {
                if let Some(line_sink) = line_sink.as_ref() {
                    let line_text = String::from_utf8_lossy(&line);
                    line_sink(line_text.trim_end_matches(['\r', '\n']));
                }
            }
            break;
        }
        if let Some(output_activity) = output_activity.as_ref() {
            output_activity.mark_output();
        }
        bytes.extend_from_slice(&chunk[..read]);
        line.extend_from_slice(&chunk[..read]);
        while let Some(newline_index) = line.iter().position(|byte| *byte == b'\n') {
            let complete_line = line.drain(..=newline_index).collect::<Vec<_>>();
            if let Some(line_sink) = line_sink.as_ref() {
                let line_text = String::from_utf8_lossy(&complete_line);
                line_sink(line_text.trim_end_matches(['\r', '\n']));
            }
        }
    }
    Ok(bytes)
}

#[derive(Default)]
struct AssistantStreamParser {
    agent_message_item_ids: HashSet<String>,
    tool_item_ids: HashSet<String>,
    tool_names_by_id: HashMap<String, String>,
}

impl AssistantStreamParser {
    fn events(&mut self, conversation_id: &str, line: &str) -> Vec<AssistantStreamEvent> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return vec![stream_event(
                conversation_id,
                "delta",
                Some(line.to_string()),
                None,
            )];
        };
        let Some(object) = value.as_object() else {
            return Vec::new();
        };
        let event_type = object
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| object.get("role").and_then(Value::as_str))
            .unwrap_or("");

        if matches!(event_type, "stream_event" | "content_block_delta") {
            let event = object.get("event").unwrap_or(&value);
            let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
            if event_type == "content_block_start"
                && event.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use")
            {
                if let Some(tool) = event.get("content_block") {
                    let label = tool_label(tool);
                    if let Some(id) = tool_id(tool) {
                        self.tool_names_by_id.insert(id, label.clone());
                    }
                    return vec![stream_event(
                        conversation_id,
                        "tool-call",
                        tool_input(tool),
                        Some(label),
                    )];
                }
            }
            if event_type == "content_block_delta"
                && event.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta")
                && let Some(text) = event.pointer("/delta/text").and_then(Value::as_str)
                && !text.is_empty()
            {
                return vec![stream_event(
                    conversation_id,
                    "delta",
                    Some(text.to_string()),
                    None,
                )];
            }
            if event_type == "content_block_start"
                && event.pointer("/content_block/type").and_then(Value::as_str) == Some("text")
                && let Some(text) = event.pointer("/content_block/text").and_then(Value::as_str)
                && !text.is_empty()
            {
                return vec![stream_event(
                    conversation_id,
                    "delta",
                    Some(text.to_string()),
                    None,
                )];
            }
            if !event_type.is_empty() {
                return vec![stream_event(
                    conversation_id,
                    "progress",
                    None,
                    Some(event_type.to_string()),
                )];
            }
        }

        if event_type == "assistant" {
            let events = self.assistant_message_events(conversation_id, object.get("message"));
            if !events.is_empty() {
                return events;
            }
        }

        if event_type == "user" {
            let mut events = Vec::new();
            if let Some(content) = object
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(Value::as_array)
            {
                for item in content {
                    if item.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let id = tool_id(item);
                    let label = id
                        .as_ref()
                        .and_then(|id| self.tool_names_by_id.get(id))
                        .cloned()
                        .unwrap_or_else(|| "Tool result".to_string());
                    events.push(stream_event(
                        conversation_id,
                        "tool-result",
                        item.get("content")
                            .or_else(|| item.get("result"))
                            .and_then(value_text),
                        Some(label),
                    ));
                }
            }
            if let Some(result) = object.get("toolUseResult").and_then(value_text) {
                let label = object
                    .get("toolUseID")
                    .or_else(|| object.get("tool_use_id"))
                    .and_then(Value::as_str)
                    .and_then(|id| self.tool_names_by_id.get(id))
                    .cloned()
                    .unwrap_or_else(|| "Tool result".to_string());
                events.push(stream_event(
                    conversation_id,
                    "tool-result",
                    Some(result),
                    Some(label),
                ));
            }
            if !events.is_empty() {
                return events;
            }
        }

        if matches!(
            event_type,
            "item.started" | "item.updated" | "item.completed"
        ) {
            if let Some(item) = object.get("item").and_then(Value::as_object) {
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
                if is_tool_item_type(item_type) {
                    let id = tool_id(&Value::Object(item.clone()));
                    let label = tool_label(&Value::Object(item.clone()));
                    let is_new = id
                        .as_ref()
                        .map(|id| self.tool_item_ids.insert(id.clone()))
                        .unwrap_or(event_type == "item.started");
                    if let Some(id) = id {
                        self.tool_names_by_id.insert(id, label.clone());
                    }
                    let mut events = Vec::new();
                    if is_new {
                        events.push(stream_event(
                            conversation_id,
                            "tool-call",
                            tool_input(&Value::Object(item.clone())),
                            Some(label.clone()),
                        ));
                    }
                    if matches!(event_type, "item.updated" | "item.completed") {
                        if let Some(result) = tool_output(&Value::Object(item.clone())) {
                            events.push(stream_event(
                                conversation_id,
                                "tool-result",
                                Some(result),
                                Some(label.clone()),
                            ));
                        } else if event_type == "item.completed" && tool_item_has_result(item_type)
                        {
                            events.push(stream_event(
                                conversation_id,
                                "tool-result",
                                None,
                                Some(label),
                            ));
                        }
                    }
                    if !events.is_empty() {
                        return events;
                    }
                }
                if item_type == "agent_message" {
                    if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                        self.agent_message_item_ids.insert(item_id.to_string());
                    }
                    if matches!(event_type, "item.updated" | "item.completed") {
                        if let Some(text) = item.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                return vec![stream_event(
                                    conversation_id,
                                    "replace",
                                    Some(text.to_string()),
                                    None,
                                )];
                            }
                        }
                    }
                }
            }
        }

        if event_type == "response_item"
            && let Some(payload) = object.get("payload")
        {
            let payload_type = payload.get("type").and_then(Value::as_str).unwrap_or("");
            if is_tool_item_type(payload_type) {
                let label = tool_label(payload);
                if let Some(id) = tool_id(payload) {
                    self.tool_names_by_id.insert(id, label.clone());
                }
                return vec![stream_event(
                    conversation_id,
                    "tool-call",
                    tool_input(payload),
                    Some(label),
                )];
            }
            if is_tool_result_item_type(payload_type) {
                let label = tool_id(payload)
                    .as_ref()
                    .and_then(|id| self.tool_names_by_id.get(id))
                    .cloned()
                    .unwrap_or_else(|| "Tool result".to_string());
                return vec![stream_event(
                    conversation_id,
                    "tool-result",
                    tool_output(payload),
                    Some(label),
                )];
            }
        }

        if event_type == "item.delta" {
            let item_id = object
                .get("item_id")
                .or_else(|| object.get("itemId"))
                .and_then(Value::as_str);
            let item_is_agent_message = object
                .get("item")
                .and_then(Value::as_object)
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
                == Some("agent_message");
            if (item_id.is_some_and(|id| self.agent_message_item_ids.contains(id))
                || item_is_agent_message)
                && let Some(delta) = object.get("delta").and_then(Value::as_str)
                && !delta.is_empty()
            {
                return vec![stream_event(
                    conversation_id,
                    "delta",
                    Some(delta.to_string()),
                    None,
                )];
            }
        }

        if event_type == "result" {
            if let Some(text) = object
                .get("result")
                .or_else(|| object.get("text"))
                .and_then(Value::as_str)
                && !text.is_empty()
            {
                return vec![stream_event(
                    conversation_id,
                    "replace",
                    Some(text.to_string()),
                    None,
                )];
            }
        }

        if matches!(
            event_type,
            "turn.started" | "turn.completed" | "item.started" | "item.updated" | "item.completed"
        ) {
            let detail = object
                .get("item")
                .and_then(Value::as_object)
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
                .map(|item_type| format!("{event_type}: {item_type}"))
                .unwrap_or_else(|| event_type.to_string());
            return vec![stream_event(
                conversation_id,
                "progress",
                None,
                Some(detail),
            )];
        }

        Vec::new()
    }

    fn assistant_message_events(
        &mut self,
        conversation_id: &str,
        message: Option<&Value>,
    ) -> Vec<AssistantStreamEvent> {
        let Some(message) = message else {
            return Vec::new();
        };
        let content = message.get("content").unwrap_or(message);
        let Some(content) = content.as_array() else {
            return assistant_message_text(Some(message))
                .filter(|text| !text.is_empty())
                .map(|text| stream_event(conversation_id, "delta", Some(text), None))
                .into_iter()
                .collect();
        };
        let mut events = Vec::new();
        for item in content {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = item.get("text").and_then(Value::as_str)
                        && !text.is_empty()
                    {
                        events.push(stream_event(
                            conversation_id,
                            "delta",
                            Some(text.to_string()),
                            None,
                        ));
                    }
                }
                Some("tool_use") => {
                    let label = tool_label(item);
                    if let Some(id) = tool_id(item) {
                        self.tool_names_by_id.insert(id, label.clone());
                    }
                    events.push(stream_event(
                        conversation_id,
                        "tool-call",
                        tool_input(item),
                        Some(label),
                    ));
                }
                _ => {}
            }
        }
        events
    }
}

fn is_tool_item_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "command_execution"
            | "function_call"
            | "custom_tool_call"
            | "local_shell_call"
            | "mcp_tool_call"
            | "web_search_call"
            | "image_generation_call"
            | "tool_use"
    )
}

fn is_tool_result_item_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "command_execution_output"
            | "function_call_output"
            | "custom_tool_call_output"
            | "local_shell_call_output"
            | "mcp_tool_call_output"
            | "tool_result"
    )
}

fn tool_item_has_result(item_type: &str) -> bool {
    matches!(
        item_type,
        "command_execution" | "mcp_tool_call" | "web_search_call" | "image_generation_call"
    )
}

fn tool_id(value: &Value) -> Option<String> {
    ["id", "call_id", "callId", "tool_use_id", "toolUseID"]
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
}

fn tool_label(value: &Value) -> String {
    if let Some(label) = ["name", "tool_name", "toolName"]
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .filter(|label| !label.trim().is_empty())
    {
        return label.to_string();
    }
    match value.get("type").and_then(Value::as_str).unwrap_or("tool") {
        "command_execution" | "local_shell_call" => "Shell command".to_string(),
        "mcp_tool_call" => "MCP tool".to_string(),
        "web_search_call" => "Web search".to_string(),
        "image_generation_call" => "Image generation".to_string(),
        item_type => item_type.replace('_', " "),
    }
}

fn tool_input(value: &Value) -> Option<String> {
    for key in ["command", "cmd", "query", "prompt"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            return truncate_tool_text(text.to_string(), 4_000);
        }
    }
    for key in ["arguments", "input", "action"] {
        let Some(input) = value.get(key) else {
            continue;
        };
        if let Some(text) = input.as_str() {
            if let Ok(parsed) = serde_json::from_str::<Value>(text)
                && let Some(command) = tool_input(&parsed)
            {
                return Some(command);
            }
            return truncate_tool_text(text.to_string(), 4_000);
        }
        if let Some(command) = tool_input(input) {
            return Some(command);
        }
        if let Some(text) = value_text(input) {
            return truncate_tool_text(text, 4_000);
        }
    }
    None
}

fn tool_output(value: &Value) -> Option<String> {
    [
        "aggregated_output",
        "output",
        "result",
        "content",
        "response",
    ]
    .iter()
    .find_map(|key| value.get(*key).and_then(value_text))
    .and_then(|text| truncate_tool_text(text, 12_000))
}

fn value_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(value_text)
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then_some(text)
        }
        Value::Object(object) => ["text", "output", "result", "content", "message"]
            .iter()
            .find_map(|key| object.get(*key).and_then(value_text))
            .or_else(|| serde_json::to_string_pretty(value).ok()),
        _ => None,
    }
}

fn truncate_tool_text(value: String, limit: usize) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let mut chars = value.chars();
    let text = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        Some(format!("{text}…"))
    } else {
        Some(text)
    }
}

fn assistant_message_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    let content = value.get("content").unwrap_or(value);
    match content {
        Value::String(text) => Some(text.to_string()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<String>();
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn stream_event(
    conversation_id: &str,
    kind: &str,
    text: Option<String>,
    detail: Option<String>,
) -> AssistantStreamEvent {
    AssistantStreamEvent {
        conversation_id: conversation_id.to_string(),
        request_id: String::new(),
        kind: kind.to_string(),
        text,
        detail,
    }
}

fn emit_stream_event(
    stream_sink: &AssistantStreamSink,
    mut event: AssistantStreamEvent,
    request_id: &str,
) {
    if !event.conversation_id.is_empty() && !request_id.is_empty() {
        event.request_id = request_id.to_string();
        stream_sink(event);
    }
}

pub fn ask(request: AssistantAskRequest) -> AssistantAskResponse {
    ask_with_stream(request, Arc::new(|_| {}), Arc::new(AtomicBool::new(false)))
}

pub fn ask_with_stream(
    mut request: AssistantAskRequest,
    stream_sink: AssistantStreamSink,
    cancellation: Arc<AtomicBool>,
) -> AssistantAskResponse {
    let started_at = Instant::now();
    let conversation_id = request.conversation_id.trim().to_string();
    let request_id = if request.request_id.trim().is_empty() {
        conversation_id.clone()
    } else {
        request.request_id.trim().to_string()
    };
    request.request_id = request_id.clone();
    let agent = request.agent.clone();
    let history_count = request.history.len();
    emit_stream_event(
        &stream_sink,
        stream_event(&conversation_id, "started", None, None),
        &request_id,
    );
    crate::logging::global().info(
        "assistant request started",
        serde_json::json!({
            "conversationId": conversation_id.as_str(),
            "agent": agent.as_str(),
            "historyCount": history_count,
            "persistUserMessage": request.persist_user_message,
        }),
    );

    match ask_inner(request, &stream_sink, &cancellation, &request_id) {
        Ok(response) => {
            emit_stream_event(
                &stream_sink,
                stream_event(&conversation_id, "completed", None, None),
                &request_id,
            );
            crate::logging::global().info(
                "assistant request finished",
                serde_json::json!({
                    "conversationId": conversation_id.as_str(),
                    "agent": agent.as_str(),
                    "status": response.status.as_str(),
                    "durationMs": started_at.elapsed().as_secs_f64() * 1000.0,
                    "answerBytes": response.answer.len(),
                }),
            );
            response
        }
        Err(error) => {
            let detail = format!("{error:#}");
            emit_stream_event(
                &stream_sink,
                stream_event(&conversation_id, "error", None, Some(log_detail(&detail))),
                &request_id,
            );
            crate::logging::global().error(
                "assistant request failed",
                serde_json::json!({
                    "conversationId": conversation_id.as_str(),
                    "agent": agent.as_str(),
                    "durationMs": started_at.elapsed().as_secs_f64() * 1000.0,
                    "error": log_detail(&detail),
                }),
            );
            AssistantAskResponse::error(detail)
        }
    }
}

fn ask_inner(
    request: AssistantAskRequest,
    stream_sink: &AssistantStreamSink,
    cancellation: &Arc<AtomicBool>,
    request_id: &str,
) -> Result<AssistantAskResponse> {
    let message = request.message.trim();
    if message.is_empty() {
        bail!("assistant message is empty")
    }
    let conversation_id = request.conversation_id.trim();
    if conversation_id.is_empty() {
        bail!("assistant conversation id is empty")
    }
    let agent = parse_agent(&request.agent).context("resolve assistant agent")?;
    let workspace = workspace_path(&request.workspace)?;
    emit_stream_event(
        stream_sink,
        stream_event(
            conversation_id,
            "progress",
            None,
            Some("preparing assistant".to_string()),
        ),
        request_id,
    );
    if cancellation.load(Ordering::Acquire) {
        bail!("assistant request cancelled")
    }
    let linked_session = context_session_link(&request.context);
    let storage_started_at = Instant::now();
    let store = crate::storage::Store::open_default()?;
    crate::logging::global().info(
        "assistant storage ready",
        serde_json::json!({
            "conversationId": conversation_id,
            "durationMs": storage_started_at.elapsed().as_secs_f64() * 1000.0,
        }),
    );
    if request.persist_user_message {
        let persist_started_at = Instant::now();
        persist_chat_message(
            &store,
            conversation_id,
            &workspace,
            linked_session.as_ref(),
            AssistantMessage {
                role: "user".to_string(),
                content: message.to_string(),
            },
        )?;
        crate::logging::global().info(
            "assistant user message persisted",
            serde_json::json!({
                "conversationId": conversation_id,
                "durationMs": persist_started_at.elapsed().as_secs_f64() * 1000.0,
            }),
        );
    }
    let prompt = build_prompt(message, &request.history, &request.context);
    if prompt.len() > MAX_ASSISTANT_PROMPT_BYTES {
        bail!(
            "assistant prompt exceeds the {} byte limit",
            MAX_ASSISTANT_PROMPT_BYTES
        )
    }
    let plan_started_at = Instant::now();
    let mut plan = plan_assistant_ask(agent, &workspace, &prompt)?;
    crate::logging::global().info(
        "assistant provider plan ready",
        serde_json::json!({
            "conversationId": conversation_id,
            "durationMs": plan_started_at.elapsed().as_secs_f64() * 1000.0,
        }),
    );
    let settings_started_at = Instant::now();
    let settings = store.app_settings()?;
    crate::logging::global().info(
        "assistant settings loaded",
        serde_json::json!({
            "conversationId": conversation_id,
            "durationMs": settings_started_at.elapsed().as_secs_f64() * 1000.0,
        }),
    );
    if let Some(profile) = crate::config_profile_key(agent)
        .and_then(|key| settings.config_profiles.get(key))
        .map(String::as_str)
    {
        crate::apply_session_config_profile(agent, &mut plan, profile)?;
    }
    crate::logging::global().info(
        "assistant provider started",
        serde_json::json!({
            "conversationId": conversation_id,
            "agent": request.agent.as_str(),
            "promptBytes": prompt.len(),
        }),
    );
    emit_stream_event(
        stream_sink,
        stream_event(
            conversation_id,
            "progress",
            None,
            Some(format!("running {}", request.agent)),
        ),
        request_id,
    );
    let provider_started_at = Instant::now();
    let output = run_session_command(
        plan,
        &[],
        ASSISTANT_OUTPUT_IDLE_TIMEOUT,
        conversation_id,
        stream_sink,
        cancellation.clone(),
        request_id,
    )?;
    crate::logging::global().info(
        "assistant provider finished",
        serde_json::json!({
            "conversationId": conversation_id,
            "durationMs": provider_started_at.elapsed().as_secs_f64() * 1000.0,
            "success": output.success,
            "code": output.code,
            "timedOut": output.timed_out,
            "stdoutBytes": output.stdout.len(),
            "stderrBytes": output.stderr.len(),
        }),
    );
    if output.timed_out {
        bail!("assistant agent timed out")
    }
    if output.cancelled {
        bail!("assistant request cancelled")
    }
    if !output.success {
        let detail = output.stderr.trim();
        if detail.is_empty() {
            bail!(
                "assistant agent exited with status {}",
                output
                    .code
                    .map_or_else(|| "unknown".to_string(), |code| code.to_string())
            )
        }
        bail!("assistant agent failed: {detail}")
    }
    let answer = extract_answer(&output.stdout)
        .ok_or_else(|| anyhow::anyhow!("assistant agent returned no answer"))?;
    let persist_started_at = Instant::now();
    persist_chat_message(
        &store,
        conversation_id,
        &workspace,
        linked_session.as_ref(),
        AssistantMessage {
            role: "assistant".to_string(),
            content: answer.clone(),
        },
    )?;
    crate::logging::global().info(
        "assistant response persisted",
        serde_json::json!({
            "conversationId": conversation_id,
            "durationMs": persist_started_at.elapsed().as_secs_f64() * 1000.0,
        }),
    );
    Ok(AssistantAskResponse {
        answer,
        status: "completed".to_string(),
        usage: extract_usage(&output.stdout).unwrap_or_default(),
        error: None,
    })
}

fn log_detail(value: &str) -> String {
    let mut chars = value.chars();
    let detail = chars.by_ref().take(512).collect::<String>();
    if chars.next().is_some() {
        format!("{detail}…")
    } else {
        detail
    }
}

fn persist_chat_message(
    store: &crate::storage::Store,
    conversation_id: &str,
    workspace: &PathBuf,
    linked_session: Option<&AssistantSessionLink>,
    message: AssistantMessage,
) -> Result<()> {
    store.with_database_write_lock_retry(|| {
        store.append_assistant_chat_message(conversation_id, workspace, linked_session, &message)
    })
}

fn context_session_link(context: &Value) -> Option<AssistantSessionLink> {
    let session = context.get("session")?.as_object()?;
    let id = session.get("id")?.as_str()?.trim();
    let agent = session.get("agent")?.as_str()?.trim();
    let path = session.get("path")?.as_str()?.trim();
    if id.is_empty() || agent.is_empty() || path.is_empty() {
        return None;
    }
    Some(AssistantSessionLink {
        id: id.to_string(),
        agent: agent.to_string(),
        path: path.to_string(),
    })
}

fn workspace_path(value: &str) -> Result<PathBuf> {
    let path = if value.trim().is_empty() {
        std::env::current_dir().context("resolve assistant workspace")?
    } else {
        PathBuf::from(value.trim())
    };
    if !path.is_absolute() {
        bail!("assistant workspace must be an absolute path")
    }
    Ok(path)
}

fn build_prompt(message: &str, history: &[AssistantMessage], context: &Value) -> String {
    let history = history
        .iter()
        .filter(|item| matches!(item.role.as_str(), "user" | "assistant"))
        .rev()
        .take(20)
        .collect::<Vec<_>>();
    let mut prompt = String::from(
        "You are the Tendi in-app assistant. Answer the user's question directly. ".to_string(),
    );
    prompt.push_str("You may use the agent's available tools when they help answer the question. ");
    prompt.push_str(
        "For page-specific questions, treat tendi.currentPage.rows as the authoritative visible data sample, use rowCount and shownRowCount to understand its bounds, name exact rows when relevant, and do not invent facts that are absent from the context.\n\n",
    );
    prompt.push_str("<tendi-context>\n");
    prompt.push_str(&serde_json::to_string_pretty(context).unwrap_or_else(|_| "{}".to_string()));
    prompt.push_str("\n</tendi-context>\n\n");
    if !history.is_empty() {
        prompt.push_str("<conversation>\n");
        for item in history.into_iter().rev() {
            prompt.push_str(if item.role == "user" {
                "User: "
            } else {
                "Assistant: "
            });
            prompt.push_str(&item.content);
            prompt.push('\n');
        }
        prompt.push_str("</conversation>\n\n");
    }
    prompt.push_str("User: ");
    prompt.push_str(message);
    prompt
}

fn run_session_command(
    plan: SessionCommand,
    input: &[u8],
    timeout: Duration,
    conversation_id: &str,
    stream_sink: &AssistantStreamSink,
    cancellation: Arc<AtomicBool>,
    request_id: &str,
) -> Result<ProcessOutput> {
    let mut command = Command::new(plan.executable);
    command.args(plan.args);
    if let Some(cwd) = plan.cwd {
        command.current_dir(cwd);
    }
    command.envs(plan.env);
    let parser = Arc::new(Mutex::new(AssistantStreamParser::default()));
    let parser_sink = Arc::clone(&parser);
    let stream_sink = Arc::clone(stream_sink);
    let conversation_id = conversation_id.to_string();
    let request_id = request_id.to_string();
    let stdout_line_sink: CommandOutputLineSink = Arc::new(move |line| {
        let events = parser_sink
            .lock()
            .map(|mut parser| parser.events(&conversation_id, line))
            .unwrap_or_default();
        for event in events {
            emit_stream_event(&stream_sink, event, &request_id);
        }
    });
    run_command_with_timeout_streaming(
        command,
        input,
        timeout,
        Some(stdout_line_sink),
        Some(cancellation),
    )
}

fn extract_answer(output: &str) -> Option<String> {
    let mut answers = Vec::new();
    let mut parsed_json = false;
    if let Ok(value) = serde_json::from_str::<Value>(output.trim()) {
        parsed_json = true;
        collect_answer_values(&value, &mut answers);
    } else {
        for line in output.lines() {
            if let Ok(value) = serde_json::from_str::<Value>(line.trim()) {
                parsed_json = true;
                collect_answer_values(&value, &mut answers);
            }
        }
    }
    answers
        .into_iter()
        .rev()
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
        .or_else(|| {
            let value = output.trim();
            (!value.is_empty() && !parsed_json).then_some(value.to_string())
        })
}

fn collect_answer_values(value: &Value, answers: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            const ANSWER_KEYS: &[&str] =
                &["answer", "result", "text", "message", "output", "content"];
            for key in ANSWER_KEYS {
                if let Some(value) = object.get(*key) {
                    match value {
                        Value::String(text) => answers.push(text.clone()),
                        _ => collect_answer_values(value, answers),
                    }
                }
            }
            for (key, value) in object {
                if !ANSWER_KEYS.contains(&key.as_str()) && key != "type" {
                    collect_answer_values(value, answers);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_answer_values(item, answers);
            }
        }
        _ => {}
    }
}

fn extract_usage(output: &str) -> Option<AssistantUsage> {
    let values = if let Ok(value) = serde_json::from_str::<Value>(output.trim()) {
        vec![value]
    } else {
        output
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
            .collect()
    };
    values.iter().rev().find_map(find_usage)
}

fn find_usage(value: &Value) -> Option<AssistantUsage> {
    match value {
        Value::Object(object) => {
            if let Some(usage) = object.get("usage").and_then(Value::as_object) {
                let read = |keys: &[&str]| {
                    keys.iter()
                        .find_map(|key| usage.get(*key).and_then(Value::as_u64))
                };
                return Some(AssistantUsage {
                    input_tokens: read(&["input_tokens", "inputTokens"]),
                    cached_input_tokens: read(&[
                        "cache_read_input_tokens",
                        "cached_input_tokens",
                        "cachedInputTokens",
                    ]),
                    output_tokens: read(&["output_tokens", "outputTokens"]),
                    reasoning_output_tokens: read(&[
                        "reasoning_output_tokens",
                        "reasoningOutputTokens",
                    ]),
                    total_tokens: read(&["total_tokens", "totalTokens"]),
                });
            }
            object.values().find_map(find_usage)
        }
        Value::Array(items) => items.iter().rev().find_map(find_usage),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
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
            extract_answer("{\"type\":\"item.completed\",\"item\":{\"text\":\"done\"}}\n")
                .as_deref(),
            Some("done")
        );
        assert_eq!(extract_answer(r#"{"type":"turn.completed"}"#), None);
    }

    #[test]
    fn extracts_usage_from_nested_json() {
        let usage =
            extract_usage(r#"{"result":"done","usage":{"input_tokens":2,"output_tokens":3}}"#)
                .unwrap();
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

        let events = parser.events(
            "conversation",
            r#"{"type":"item.updated","item":{"id":"tool-1","type":"command_execution","aggregated_output":"fixture"}}"#,
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "tool-result");
        assert_eq!(events[0].text.as_deref(), Some("fixture"));
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

        let events = parser.events(
            "conversation",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"fixture"}]}}"#,
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "tool-result");
        assert_eq!(events[0].detail.as_deref(), Some("Bash"));
        assert_eq!(events[0].text.as_deref(), Some("fixture"));
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
            "printf 'first'; sleep 0.15; printf 'second'; sleep 0.15",
        ]);

        let output = run_command_with_timeout_streaming(
            command,
            &[],
            Duration::from_millis(250),
            None,
            None,
        )
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
}
