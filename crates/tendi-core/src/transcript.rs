use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;

use crate::{sessions::extract_cursor_blob_model, skills::AgentKind};

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptItem {
    pub kind: String,
    pub body: String,
    pub tag: Option<String>,
    pub time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip)]
    pub call_id: Option<String>,
    #[serde(skip)]
    pub started_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptScan {
    pub items: Vec<TranscriptItem>,
    pub warnings: Vec<String>,
}

pub fn parse_transcript(path: &Path, agent: AgentKind) -> Result<TranscriptScan> {
    let file =
        fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut items = Vec::new();
    let mut warnings = Vec::new();

    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                warnings.push(format!("{}:{}: {err}", path.display(), index + 1));
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let value = match serde_json::from_str::<Value>(&line) {
            Ok(value) => value,
            Err(err) => {
                warnings.push(format!("{}:{}: {err}", path.display(), index + 1));
                continue;
            }
        };

        match agent {
            AgentKind::Codex => collect_codex_item(&value, &mut items),
            AgentKind::Claude => collect_claude_item(&value, &mut items),
            AgentKind::Cursor | AgentKind::Shared | AgentKind::Unknown => {
                collect_generic_item(&value, &mut items)
            }
        }
    }

    if agent == AgentKind::Cursor {
        append_cursor_model_configs(path, &mut items);
    }

    Ok(TranscriptScan { items, warnings })
}

pub(crate) fn parse_search_transcript(path: &Path, agent: AgentKind) -> Result<TranscriptScan> {
    let file =
        fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut items = Vec::new();
    let mut warnings = Vec::new();

    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                warnings.push(format!("{}:{}: {err}", path.display(), index + 1));
                continue;
            }
        };
        if line.trim().is_empty() || !line_may_contain_search_message(&line, agent) {
            continue;
        }
        let value = match serde_json::from_str::<Value>(&line) {
            Ok(value) => value,
            Err(err) => {
                warnings.push(format!("{}:{}: {err}", path.display(), index + 1));
                continue;
            }
        };

        match agent {
            AgentKind::Codex => collect_codex_item(&value, &mut items),
            AgentKind::Claude => collect_claude_item(&value, &mut items),
            AgentKind::Cursor | AgentKind::Shared | AgentKind::Unknown => {
                collect_generic_item(&value, &mut items)
            }
        }
    }
    items.retain(|item| matches!(item.kind.as_str(), "user" | "assistant"));
    Ok(TranscriptScan { items, warnings })
}

const SEARCH_MESSAGE_HINT_BYTES: usize = 16 * 1024;

fn line_may_contain_search_message(line: &str, agent: AgentKind) -> bool {
    let mut end = line.len().min(SEARCH_MESSAGE_HINT_BYTES);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    let hint = &line[..end];
    let role = json_string_hint(hint, "\"role\"");
    match agent {
        AgentKind::Codex => {
            json_string_hint(hint, "\"type\"") == Some("response_item")
                && matches!(role, Some("user" | "assistant"))
                && hint.contains("\"message\"")
        }
        AgentKind::Claude => {
            matches!(
                json_string_hint(hint, "\"type\""),
                Some("user" | "assistant")
            ) && !hint.contains("\"tool_result\"")
        }
        AgentKind::Cursor | AgentKind::Shared | AgentKind::Unknown => matches!(
            role.or_else(|| json_string_hint(hint, "\"type\"")),
            Some("user" | "assistant")
        ),
    }
}

fn json_string_hint<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let start = line.find(marker)? + marker.len();
    let value = line[start..].trim_start().strip_prefix(':')?.trim_start();
    let value = value.strip_prefix('"')?;
    let end = value.find('"')?;
    if value[..end].contains('\\') {
        None
    } else {
        Some(&value[..end])
    }
}

fn collect_codex_item(value: &Value, items: &mut Vec<TranscriptItem>) {
    let record_type = value.get("type").and_then(Value::as_str);
    if record_type == Some("compacted") {
        push_codex_compaction(items, value);
        return;
    }
    if record_type == Some("event_msg") {
        if value.pointer("/payload/type").and_then(Value::as_str) == Some("context_compacted") {
            push_codex_compaction(items, value);
            return;
        }
        if value.pointer("/payload/type").and_then(Value::as_str) == Some("thread_settings_applied")
        {
            push_codex_model_config(items, value);
            return;
        }
        attach_codex_subagent_session(value, items);
        return;
    }
    if record_type == Some("turn_context") {
        push_codex_model_config(items, value);
        return;
    }
    if record_type != Some("response_item") {
        return;
    }

    let Some(payload) = value.get("payload") else {
        return;
    };
    let time = value
        .get("timestamp")
        .and_then(Value::as_str)
        .map(compact_time);
    let timestamp_ms = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_timestamp_ms);

    match payload.get("type").and_then(Value::as_str) {
        Some("message") => {
            let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
            let content = payload.get("content");
            if role == "developer" || role == "system" {
                if let Some(body) = extract_raw_content_text(content) {
                    push_item(
                        items,
                        "context",
                        body,
                        Some(
                            if role == "system" {
                                "System"
                            } else {
                                "Developer"
                            }
                            .to_string(),
                        ),
                        time,
                    );
                }
                return;
            }
            if role != "user" && role != "assistant" {
                return;
            }
            if role == "user" {
                collect_internal_context_items(content, items, time.clone());
            }
            if let Some(body) = extract_content_text(content) {
                let item_kind = if role == "user" && is_subagent_notification(&body) {
                    "notification"
                } else {
                    role
                };
                let tag = (item_kind == "notification").then(|| "Subagent".to_string());
                push_item(items, item_kind, body, tag, time);
            }
        }
        Some("reasoning") | Some("thinking") => {
            let kind = payload
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("reasoning");
            if let Some(body) = extract_thinking_text(
                payload
                    .get("summary")
                    .or_else(|| payload.get("content"))
                    .or(Some(payload)),
            ) {
                push_item(items, kind, body, None, time);
            }
        }
        Some("function_call") | Some("custom_tool_call") | Some("local_shell_call") => {
            let name = payload
                .get("name")
                .or_else(|| payload.pointer("/action/type"))
                .and_then(Value::as_str)
                .unwrap_or("tool_call");
            push_tool_item(
                items,
                "tool",
                summarize_tool_call(payload, name),
                Some(name.to_string()),
                time,
                extract_tool_command(payload),
                None,
                extract_duration_ms(payload, None),
                extract_call_id(payload),
                timestamp_ms,
            );
        }
        Some("function_call_output") | Some("custom_tool_call_output") => {
            let result = extract_tool_result(payload);
            attach_tool_result(
                items,
                extract_call_id(payload).as_deref(),
                result.clone(),
                extract_duration_ms(payload, result.as_deref()),
                timestamp_ms,
            );
        }
        Some("web_search_call") | Some("image_generation_call") => {
            let kind = payload
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("tool_call");
            push_item(
                items,
                "tool",
                kind.replace('_', " "),
                Some(kind.to_string()),
                time,
            );
        }
        _ => {}
    }
}

fn push_codex_compaction(items: &mut Vec<TranscriptItem>, value: &Value) {
    let time = value
        .get("timestamp")
        .and_then(Value::as_str)
        .map(compact_time);
    if items
        .last()
        .is_some_and(|item| item.kind == "compaction" && item.time == time)
    {
        return;
    }
    push_item(
        items,
        "compaction",
        "Context compacted".to_string(),
        None,
        time,
    );
}

fn push_codex_model_config(items: &mut Vec<TranscriptItem>, value: &Value) {
    let settings = if value.get("type").and_then(Value::as_str) == Some("turn_context") {
        value.get("payload")
    } else {
        value.pointer("/payload/thread_settings")
    };
    let previous = items.iter().rev().find(|item| item.kind == "model_config");
    let model = settings
        .and_then(|settings| settings.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| previous.and_then(|item| item.model.clone()));
    let effort = settings
        .and_then(|settings| {
            settings
                .get("effort")
                .or_else(|| settings.get("reasoning_effort"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| previous.and_then(|item| item.effort.clone()));
    let time = value
        .get("timestamp")
        .and_then(Value::as_str)
        .map(compact_time);
    push_model_config(items, model, effort, time);
}

fn push_model_config(
    items: &mut Vec<TranscriptItem>,
    model: Option<String>,
    effort: Option<String>,
    time: Option<String>,
) {
    let previous = items.iter().rev().find(|item| item.kind == "model_config");
    if model.is_none() && effort.is_none()
        || previous.is_some_and(|item| item.model == model && item.effort == effort)
    {
        return;
    }

    items.push(model_config_item(model, effort, time));
}

fn model_config_item(
    model: Option<String>,
    effort: Option<String>,
    time: Option<String>,
) -> TranscriptItem {
    let body = [
        model.as_ref().map(|value| format!("Model: {value}")),
        effort.as_ref().map(|value| format!("Effort: {value}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    TranscriptItem {
        kind: "model_config".to_string(),
        body,
        tag: None,
        time,
        command: None,
        result: None,
        duration_ms: None,
        linked_session_id: None,
        model,
        effort,
        call_id: None,
        started_at_ms: None,
    }
}

fn append_cursor_model_configs(path: &Path, items: &mut Vec<TranscriptItem>) {
    let Some(store_path) = find_cursor_store_db(path) else {
        return;
    };
    append_cursor_model_configs_from_store(&store_path, items);
}

fn find_cursor_store_db(path: &Path) -> Option<PathBuf> {
    let session_id = path.file_stem()?.to_str()?.trim();
    if session_id.is_empty() {
        return None;
    }
    let chats_root = dirs::home_dir()?.join(".cursor/chats");
    let entries = fs::read_dir(chats_root).ok()?;
    entries.filter_map(Result::ok).find_map(|entry| {
        let candidate = entry.path().join(session_id).join("store.db");
        candidate.is_file().then_some(candidate)
    })
}

fn append_cursor_model_configs_from_store(path: &Path, items: &mut Vec<TranscriptItem>) {
    let Ok(connection) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return;
    };
    let Ok(mut statement) = connection.prepare(
        "select data from blobs
         where instr(data, 'modelName') > 0
         order by rowid",
    ) else {
        return;
    };
    let Ok(rows) = statement.query_map([], |row| row.get::<_, Vec<u8>>(0)) else {
        return;
    };

    let mut models = Vec::new();
    for bytes in rows.filter_map(Result::ok) {
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let Some(model) = extract_cursor_blob_model(&value) else {
            continue;
        };
        models.push(model);
    }

    insert_cursor_model_configs(items, &models);
}

fn insert_cursor_model_configs(items: &mut Vec<TranscriptItem>, models: &[String]) {
    if models.is_empty() {
        return;
    }
    let assistant_positions = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| (item.kind == "assistant").then_some(index))
        .collect::<Vec<_>>();
    let mut previous_model: Option<&str> = None;
    let model_count = models.len();
    let assistant_count = assistant_positions.len();
    let mut insertions = Vec::new();
    for (sample_index, model) in models.iter().enumerate() {
        if previous_model == Some(model.as_str()) {
            continue;
        }
        previous_model = Some(model);
        let assistant_index = sample_index
            .saturating_mul(assistant_count)
            .checked_div(model_count)
            .unwrap_or(0);
        let item_index = assistant_positions
            .get(assistant_index)
            .copied()
            .unwrap_or(items.len());
        insertions.push((item_index, model.clone()));
    }
    let mut offset = 0usize;
    for (item_index, model) in insertions {
        items.insert(
            item_index + offset,
            model_config_item(Some(model), None, None),
        );
        offset += 1;
    }
}

fn attach_codex_subagent_session(value: &Value, items: &mut [TranscriptItem]) -> bool {
    let Some(payload) = value.get("payload") else {
        return false;
    };
    if payload.get("type").and_then(Value::as_str) != Some("sub_agent_activity")
        || payload.get("kind").and_then(Value::as_str) != Some("started")
    {
        return false;
    }
    let Some(event_id) = payload.get("event_id").and_then(Value::as_str) else {
        return false;
    };
    let Some(session_id) = payload.get("agent_thread_id").and_then(Value::as_str) else {
        return false;
    };
    let Some(item) = items.iter_mut().rev().find(|item| {
        item.kind == "tool"
            && item.tag.as_deref() == Some("spawn_agent")
            && item.call_id.as_deref() == Some(event_id)
    }) else {
        return false;
    };
    item.linked_session_id = Some(session_id.to_string());
    true
}

fn collect_claude_item(value: &Value, items: &mut Vec<TranscriptItem>) {
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return;
    };
    let time = value
        .get("timestamp")
        .and_then(Value::as_str)
        .map(compact_time);
    let timestamp_ms = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_timestamp_ms);

    if kind == "user" || kind == "assistant" {
        let content = value
            .get("message")
            .and_then(|message| message.get("content"));
        if kind == "user" {
            collect_internal_context_items(content, items, time.clone());
        }
        if kind == "user"
            && attach_claude_tool_results(value, content, items, time.clone(), timestamp_ms)
        {
            return;
        }
        if kind == "assistant" {
            if let Some(body) = extract_thinking_text(content) {
                push_item(items, "thinking", body, None, time.clone());
            }
        }
        if let Some(body) = value
            .get("message")
            .and_then(|message| extract_content_text(message.get("content")))
        {
            let item_kind = if kind == "user" && is_subagent_notification(&body) {
                "notification"
            } else {
                kind
            };
            let tag = (item_kind == "notification").then(|| "Subagent".to_string());
            push_item(items, item_kind, body, tag, time.clone());
        }

        if let Some(Value::Array(content_items)) = content {
            for item in content_items {
                if item.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool_call");
                    push_tool_item(
                        items,
                        "tool",
                        summarize_tool_call(item, name),
                        Some(name.to_string()),
                        time.clone(),
                        extract_tool_command(item),
                        None,
                        extract_duration_ms(item, None),
                        item.get("id").and_then(Value::as_str).map(str::to_string),
                        timestamp_ms,
                    );
                }
            }
        }
    }

    if let Some(result) = value.get("toolUseResult") {
        let body = extract_content_text(Some(result)).unwrap_or_else(|| "tool result".to_string());
        let call_id = value
            .get("toolUseID")
            .or_else(|| value.get("tool_use_id"))
            .or_else(|| value.get("toolUseId"))
            .and_then(Value::as_str);
        if !attach_tool_result(
            items,
            call_id,
            Some(body.clone()),
            extract_duration_ms(value, None),
            timestamp_ms,
        ) {
            push_item(items, "tool", body, Some("tool_result".to_string()), time);
        }
    }
}

fn attach_claude_tool_results(
    value: &Value,
    content: Option<&Value>,
    items: &mut Vec<TranscriptItem>,
    time: Option<String>,
    timestamp_ms: Option<i64>,
) -> bool {
    let Some(Value::Array(content_items)) = content else {
        return false;
    };
    let mut handled = false;
    for item in content_items {
        if item.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        handled = true;
        let body = extract_content_text(item.get("content"))
            .or_else(|| {
                value
                    .get("toolUseResult")
                    .and_then(|result| extract_content_text(Some(result)))
            })
            .unwrap_or_else(|| "tool result".to_string());
        let call_id = item
            .get("tool_use_id")
            .or_else(|| item.get("toolUseID"))
            .or_else(|| item.get("toolUseId"))
            .and_then(Value::as_str);
        if !attach_tool_result(
            items,
            call_id,
            Some(body.clone()),
            extract_duration_ms(value, Some(&body)),
            timestamp_ms,
        ) {
            push_item(
                items,
                "tool",
                body,
                Some("tool_result".to_string()),
                time.clone(),
            );
        }
    }
    handled
}

fn collect_generic_item(value: &Value, items: &mut Vec<TranscriptItem>) {
    let kind = value
        .get("role")
        .or_else(|| value.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if kind == "developer" || kind == "system" {
        let content = value
            .pointer("/message/content")
            .or_else(|| value.get("content"))
            .or_else(|| value.get("message"));
        if let Some(body) = extract_raw_content_text(content) {
            push_item(
                items,
                "context",
                body,
                Some(
                    if kind == "system" {
                        "System"
                    } else {
                        "Developer"
                    }
                    .to_string(),
                ),
                value
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .map(compact_time),
            );
        }
        return;
    }
    if kind != "user" && kind != "assistant" {
        return;
    }

    let time = value
        .get("timestamp")
        .and_then(Value::as_str)
        .map(compact_time);
    let content = value
        .pointer("/message/content")
        .or_else(|| value.get("content"))
        .or_else(|| value.get("message"));
    if kind == "user" {
        collect_internal_context_items(content, items, time.clone());
    }
    if let Some(body) = extract_content_text(content) {
        let item_kind = if kind == "user" && is_subagent_notification(&body) {
            "notification"
        } else {
            kind
        };
        let tag = (item_kind == "notification").then(|| "Subagent".to_string());
        push_item(items, item_kind, body, tag, time.clone());
    }

    if let Some(Value::Array(content_items)) = content {
        for item in content_items {
            if item.get("type").and_then(Value::as_str) == Some("tool_use") {
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool_call");
                push_tool_item(
                    items,
                    "tool",
                    summarize_tool_call(item, name),
                    Some(name.to_string()),
                    time.clone(),
                    extract_tool_command(item),
                    None,
                    extract_duration_ms(item, None),
                    item.get("id").and_then(Value::as_str).map(str::to_string),
                    value
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .and_then(parse_timestamp_ms),
                );
            }
        }
    }
}

fn extract_content_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::String(text) => clean_body(text),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter(|item| !is_thinking_content_item(item))
                .filter_map(|item| {
                    item.get("text")
                        .or_else(|| item.get("content"))
                        .and_then(Value::as_str)
                        .and_then(clean_body)
                })
                .collect::<Vec<_>>()
                .join("\n");
            clean_body(&text)
        }
        Value::Object(_) if is_thinking_content_item(value) => None,
        Value::Object(_) => serde_json::to_string(value)
            .ok()
            .and_then(|text| clean_body(&text)),
        _ => None,
    }
}

fn extract_raw_content_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    let text = match value {
        Value::String(text) => text.trim().to_string(),
        Value::Array(items) => items
            .iter()
            .filter(|item| !is_thinking_content_item(item))
            .filter_map(|item| {
                item.get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| extract_raw_content_text(item.get("content")))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(_) if is_thinking_content_item(value) => String::new(),
        Value::Object(_) => value
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| extract_raw_content_text(value.get("content")))
            .unwrap_or_default(),
        _ => String::new(),
    };
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

fn collect_internal_context_items(
    content: Option<&Value>,
    items: &mut Vec<TranscriptItem>,
    time: Option<String>,
) {
    let Some(content) = content else {
        return;
    };
    let content_items = match content {
        Value::Array(content_items) => content_items.as_slice(),
        _ => std::slice::from_ref(content),
    };
    for content_item in content_items {
        let Some(body) = extract_raw_content_text(Some(content_item)) else {
            continue;
        };
        let Some(label) = internal_context_label(&body) else {
            continue;
        };
        push_item(
            items,
            "context",
            body,
            Some(label.to_string()),
            time.clone(),
        );
    }
}

fn extract_thinking_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::String(text) => clean_body(text),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter(|item| is_thinking_content_item(item))
                .filter_map(|item| {
                    item.get("thinking")
                        .or_else(|| item.get("text"))
                        .and_then(Value::as_str)
                        .and_then(clean_body)
                        .or_else(|| extract_thinking_text(item.get("content")))
                })
                .collect::<Vec<_>>()
                .join("\n");
            clean_body(&text)
        }
        Value::Object(_) => value
            .get("thinking")
            .or_else(|| value.get("text"))
            .and_then(Value::as_str)
            .and_then(clean_body)
            .or_else(|| extract_thinking_text(value.get("summary")))
            .or_else(|| extract_thinking_text(value.get("content"))),
        _ => None,
    }
}

fn is_thinking_content_item(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some("thinking" | "reasoning" | "summary_text")
    )
}

fn clean_body(text: &str) -> Option<String> {
    let mut text = text.trim();
    if text.is_empty()
        || text.starts_with("<local-command-caveat>")
        || text.starts_with("<command-name>")
        || text.starts_with("<local-command-stdout>")
        || text.starts_with("<task-notification>")
        || internal_context_label(text).is_some()
    {
        return None;
    }

    if let Some(inner) = extract_tag_body(text, "user_query") {
        text = inner;
    }

    Some(text.to_string())
}

fn is_subagent_notification(text: &str) -> bool {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized
        == "Briefly inform the user about the task result and perform any follow-up actions (if needed)."
        || normalized.starts_with(
            "The beginning of the above subagent result is already visible to the user. Perform any follow-up actions (if needed).",
        )
}

fn extract_tag_body<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = text.find(&start_tag)? + start_tag.len();
    let end = text[start..].find(&end_tag)? + start;
    let inner = text[start..end].trim();
    (!inner.is_empty()).then_some(inner)
}

fn internal_context_label(text: &str) -> Option<&'static str> {
    [
        ("# AGENTS.md instructions", "AGENTS.md"),
        ("<codex_internal_context", "Codex internal"),
        ("<environment_context>", "Environment"),
        ("<permissions instructions>", "Permissions"),
        ("<app-context>", "App context"),
        ("<collaboration_mode>", "Collaboration"),
        ("<skills_instructions>", "Skills"),
        ("<plugins_instructions>", "Plugins"),
        ("<system-reminder>", "System reminder"),
        ("<available_subagent_types>", "Subagent types"),
    ]
    .into_iter()
    .find_map(|(prefix, label)| text.starts_with(prefix).then_some(label))
}

fn summarize_tool_call(payload: &Value, name: &str) -> String {
    if let Some(command) = extract_tool_command(payload) {
        return command.chars().take(220).collect();
    }

    if let Some(arguments) = payload.get("arguments").and_then(Value::as_str) {
        if let Ok(value) = serde_json::from_str::<Value>(arguments) {
            if let Some(command) = value.get("cmd").and_then(Value::as_str) {
                return command.chars().take(220).collect();
            }
        }
        if !arguments.trim().is_empty() {
            return arguments.chars().take(220).collect();
        }
    }

    if let Some(input) = payload.get("input") {
        if let Ok(text) = serde_json::to_string(input) {
            if !text.trim().is_empty() {
                return text.chars().take(220).collect();
            }
        }
    }

    name.to_string()
}

fn push_item(
    items: &mut Vec<TranscriptItem>,
    kind: &str,
    body: String,
    tag: Option<String>,
    time: Option<String>,
) {
    push_tool_item(items, kind, body, tag, time, None, None, None, None, None);
}

fn push_tool_item(
    items: &mut Vec<TranscriptItem>,
    kind: &str,
    body: String,
    tag: Option<String>,
    time: Option<String>,
    command: Option<String>,
    result: Option<String>,
    duration_ms: Option<u64>,
    call_id: Option<String>,
    started_at_ms: Option<i64>,
) {
    items.push(TranscriptItem {
        kind: kind.to_string(),
        body,
        tag,
        time,
        command,
        result,
        duration_ms,
        linked_session_id: None,
        model: None,
        effort: None,
        call_id,
        started_at_ms,
    });
}

fn attach_tool_result(
    items: &mut [TranscriptItem],
    call_id: Option<&str>,
    result: Option<String>,
    duration_ms: Option<u64>,
    ended_at_ms: Option<i64>,
) -> bool {
    let Some(result) = result else {
        return false;
    };
    let matched = items.iter_mut().rev().find(|item| {
        item.kind == "tool"
            && match call_id {
                Some(call_id) => item.call_id.as_deref() == Some(call_id),
                None => item.result.is_none(),
            }
    });
    let Some(item) = matched else {
        return false;
    };
    item.result = Some(truncate_text(result.trim(), 12_000));
    let elapsed_ms = item
        .started_at_ms
        .zip(ended_at_ms)
        .and_then(|(start, end)| u64::try_from(end - start).ok());
    let measured_duration = match duration_ms {
        Some(0) | None => elapsed_ms.or(duration_ms),
        Some(duration_ms) => Some(duration_ms),
    };
    if item.duration_ms.is_none() || item.duration_ms == Some(0) {
        item.duration_ms = measured_duration;
    }
    true
}

fn extract_call_id(payload: &Value) -> Option<String> {
    payload
        .get("call_id")
        .or_else(|| payload.get("callId"))
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn extract_tool_command(payload: &Value) -> Option<String> {
    if let Some(command) = payload
        .pointer("/arguments/cmd")
        .or_else(|| payload.pointer("/arguments/command"))
        .or_else(|| payload.pointer("/action/command"))
        .or_else(|| payload.pointer("/input/command"))
        .or_else(|| payload.pointer("/input/cmd"))
        .and_then(Value::as_str)
    {
        return Some(truncate_text(command.trim(), 4_000));
    }

    let arguments = payload.get("arguments").and_then(Value::as_str)?;
    let value = serde_json::from_str::<Value>(arguments).ok()?;
    value
        .get("cmd")
        .or_else(|| value.get("command"))
        .and_then(Value::as_str)
        .map(|command| truncate_text(command.trim(), 4_000))
}

fn extract_tool_result(payload: &Value) -> Option<String> {
    payload
        .get("output")
        .or_else(|| payload.get("result"))
        .or_else(|| payload.get("content"))
        .and_then(|value| match value {
            Value::String(text) => Some(text.clone()),
            Value::Object(_) | Value::Array(_) => serde_json::to_string_pretty(value).ok(),
            _ => None,
        })
        .map(|text| truncate_text(text.trim(), 12_000))
        .filter(|text| !text.is_empty())
}

fn extract_duration_ms(payload: &Value, output: Option<&str>) -> Option<u64> {
    for pointer in ["/duration_ms", "/durationMs", "/elapsed_ms", "/elapsedMs"] {
        if let Some(value) = payload.pointer(pointer) {
            if let Some(ms) = value.as_u64() {
                return Some(ms);
            }
            if let Some(ms) = value.as_f64() {
                return Some(ms.max(0.0).round() as u64);
            }
        }
    }

    output.and_then(parse_wall_time_ms)
}

fn parse_wall_time_ms(output: &str) -> Option<u64> {
    output.lines().find_map(|line| {
        let value = line.trim().strip_prefix("Wall time: ")?;
        let seconds = value.strip_suffix(" seconds").unwrap_or(value).trim();
        seconds
            .parse::<f64>()
            .ok()
            .map(|seconds| (seconds.max(0.0) * 1000.0).round() as u64)
    })
}

fn parse_timestamp_ms(value: &str) -> Option<i64> {
    let text = value.trim();
    let year = text.get(0..4)?.parse::<i32>().ok()?;
    let month = text.get(5..7)?.parse::<u32>().ok()?;
    let day = text.get(8..10)?.parse::<u32>().ok()?;
    let hour = text.get(11..13)?.parse::<u32>().ok()?;
    let minute = text.get(14..16)?.parse::<u32>().ok()?;
    let second = text.get(17..19)?.parse::<u32>().ok()?;
    let millis = text
        .get(19..)
        .and_then(|rest| rest.strip_prefix('.'))
        .map(|fraction| {
            fraction
                .chars()
                .take_while(|char| char.is_ascii_digit())
                .take(3)
                .collect::<String>()
        })
        .filter(|fraction| !fraction.is_empty())
        .and_then(|fraction| format!("{fraction:0<3}").parse::<i64>().ok())
        .unwrap_or(0);

    let days = days_from_civil(year, month, day)?;
    Some(
        days * 86_400_000
            + i64::from(hour) * 3_600_000
            + i64::from(minute) * 60_000
            + i64::from(second) * 1000
            + millis,
    )
}

fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i32::try_from(month).ok()?;
    let day = i32::try_from(day).ok()?;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(i64::from(era) * 146_097 + i64::from(day_of_era) - 719_468)
}

fn truncate_text(value: &str, limit: usize) -> String {
    let mut text: String = value.chars().take(limit).collect();
    if value.chars().count() > limit {
        text.push_str("\n... truncated");
    }
    text
}

fn compact_time(value: &str) -> String {
    value
        .split('T')
        .nth(1)
        .and_then(|time| time.get(0..5))
        .unwrap_or(value)
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use rusqlite::Connection;
    use serde_json::json;

    use super::{
        append_cursor_model_configs_from_store, collect_claude_item, collect_codex_item,
        collect_generic_item, parse_search_transcript, summarize_tool_call,
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

        assert_eq!(
            summarize_tool_call(&payload, "exec_command"),
            "rg -n \"needle\" src"
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
        assert_eq!(items[0].time.as_deref(), Some("06:19"));
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
        assert_eq!(items[1].time.as_deref(), Some("10:11"));
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
    fn skips_claude_task_notifications() {
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

        assert!(items.is_empty());
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
                        "input": { "command": "cat src/App.jsx" }
                    }
                ]
            }
        });
        let mut items = Vec::new();

        collect_generic_item(&user, &mut items);
        collect_generic_item(&assistant, &mut items);

        assert_eq!(items.len(), 3);
        assert_eq!(items[0].kind, "user");
        assert_eq!(items[0].body, "Fix Cursor detail");
        assert_eq!(items[1].kind, "assistant");
        assert_eq!(items[1].body, "I will inspect it.");
        assert_eq!(items[2].kind, "tool");
        assert_eq!(items[2].tag.as_deref(), Some("Read"));
        assert_eq!(items[2].command.as_deref(), Some("cat src/App.jsx"));
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

        collect_generic_item(&notification, &mut items);
        collect_generic_item(&notification_with_details, &mut items);
        collect_generic_item(&internal, &mut items);
        collect_generic_item(&user, &mut items);

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

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, "context");
        assert_eq!(items[0].tag.as_deref(), Some("AGENTS.md"));
        assert!(
            items[0]
                .body
                .contains("<INSTRUCTIONS>hidden</INSTRUCTIONS>")
        );
        assert_eq!(items[1].kind, "user");
        assert_eq!(items[1].body, "What happened in this session?");
        assert_eq!(items[1].time.as_deref(), Some("10:12"));
    }
}
