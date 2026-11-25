use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::{
    analytics::{AnalyticsTokenUsage, SessionAnalyticsRecord},
    sessions::{self, SessionRecord, SessionScanCache},
    skills::AgentKind,
    transcript::{self, TranscriptItem},
};

const SURFACES: &[&str] = &["antigravity-cli", "antigravity"];
const T3_PROFILES: &str = ".t3/userdata/providers/antigravity";
const T3_APP_DATA: &str = "antigravity-acp";
const SUMMARY_FILE: &str = "conversation_summaries.db";
const LOG_SUFFIX: &str = ".system_generated/logs/transcript.jsonl";

pub(super) fn data_roots(home: &Path) -> impl Iterator<Item = PathBuf> {
    let t3_roots = fs::read_dir(home.join(T3_PROFILES))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|profile| profile.path().join(T3_APP_DATA))
        .filter(|root| root.is_dir());
    SURFACES
        .iter()
        .map(|surface| home.join(".gemini").join(surface))
        .chain(t3_roots)
}

pub(super) struct Source<'a> {
    pub id: &'a str,
    root: &'a Path,
}

impl<'a> Source<'a> {
    pub fn from_path(path: &'a Path) -> Option<Self> {
        if !path.ends_with(LOG_SUFFIX) {
            return None;
        }
        let conversation = path.ancestors().nth(3)?;
        let id = conversation.file_name()?.to_str()?;
        let brain = conversation.parent()?;
        if brain.file_name()? != "brain" || !valid_id(id) {
            return None;
        }
        Some(Self {
            id,
            root: brain.parent()?,
        })
    }

    pub fn is_native_cli(&self, home: &Path) -> bool {
        self.root == home.join(".gemini/antigravity-cli")
    }
}

pub(super) fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, byte)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

pub(super) fn transcript_path(root: &Path, id: &str) -> PathBuf {
    root.join("brain").join(id).join(LOG_SUFFIX)
}

pub(super) fn summary_path(root: &Path) -> PathBuf {
    root.join(SUMMARY_FILE)
}

pub(super) fn is_native_summary(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == SUMMARY_FILE)
        && path.parent().is_some_and(|root| {
            SURFACES.iter().any(|surface| root.ends_with(surface)) || t3_userdata(root).is_some()
        })
}

fn t3_userdata(root: &Path) -> Option<&Path> {
    let profiles = root.ancestors().nth(2)?;
    if root.ends_with(T3_APP_DATA) && profiles.ends_with(T3_PROFILES) {
        profiles.parent()?.parent()
    } else {
        None
    }
}

pub(super) fn summary_watch_source(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let basename = name
        .strip_suffix("-wal")
        .or_else(|| name.strip_suffix("-shm"))
        .unwrap_or(name);
    let db = path.with_file_name(basename);
    is_native_summary(&db).then_some(db)
}

pub(super) fn scan_root(
    root: &Path,
    records: &mut Vec<SessionRecord>,
    warnings: &mut Vec<String>,
    cache: Option<&SessionScanCache>,
) {
    let brain = root.join("brain");
    let entries = match fs::read_dir(&brain) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            warnings.push(format!("{}: {error}", brain.display()));
            return;
        }
    };
    let start = records.len();
    for entry in entries.flatten() {
        let path = entry.path().join(LOG_SUFFIX);
        if path.is_file() && Source::from_path(&path).is_some() {
            sessions::scan_jsonl_session(&path, AgentKind::Antigravity, records, cache);
        }
    }
    if records.len() == start {
        return;
    }
    match read_summaries(root, None) {
        Ok(metadata) => {
            for record in &mut records[start..] {
                if let Some(summary) = metadata.get(&record.id) {
                    summary.apply(record);
                }
            }
        }
        Err(error) => warnings.push(format!("{error:#}")),
    }
}

pub(super) fn scan_path(
    path: &Path,
    records: &mut Vec<SessionRecord>,
    cache: Option<&SessionScanCache>,
) -> Result<bool> {
    let Some(source) = Source::from_path(path) else {
        return Ok(false);
    };
    let start = records.len();
    sessions::scan_jsonl_session(path, AgentKind::Antigravity, records, cache);
    if records.len() > start {
        // Transcript data is still readable if the optional SQLite index is busy.
        if let Ok(summaries) = read_summaries(source.root, Some(source.id))
            && let Some(summary) = summaries.get(source.id)
        {
            summary.apply(&mut records[start]);
        }
    }
    Ok(true)
}

#[derive(Debug)]
struct Summary {
    title: Option<String>,
    project: Option<PathBuf>,
    parent: Option<String>,
}

impl Summary {
    fn apply(&self, record: &mut SessionRecord) {
        if let Some(title) = &self.title {
            record.title = Some(title.clone());
        }
        if let Some(project) = &self.project {
            record.project = Some(project.clone());
        }
        if let Some(parent) = &self.parent {
            record.parent_session_id = Some(parent.clone());
        }
    }
}

fn read_summaries(root: &Path, id: Option<&str>) -> Result<BTreeMap<String, Summary>> {
    let path = summary_path(root);
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("failed to open Antigravity summary {}", path.display()))?;
    db.busy_timeout(Duration::from_millis(200))?;
    let base = "SELECT conversation_id, title, workspace_uris, parent_conversation_id
                FROM conversation_summaries";
    let mut stmt = if id.is_some() {
        db.prepare(&format!("{base} WHERE conversation_id = ?1"))
    } else {
        db.prepare(base)
    }
    .with_context(|| format!("failed to read Antigravity summary {}", path.display()))?;
    let mut rows = if let Some(id) = id {
        stmt.query([id])?
    } else {
        stmt.query([])?
    };
    let mut summaries = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let title: Option<String> = row.get(1)?;
        let workspaces: Option<String> = row.get(2)?;
        let parent: Option<String> = row.get(3)?;
        summaries.insert(
            id,
            Summary {
                title: sessions::clean_session_title(title),
                project: workspaces.as_deref().and_then(workspace_path),
                parent: parent.filter(|id| valid_id(id)),
            },
        );
    }
    if let Some(userdata) = t3_userdata(root) {
        for (id, title) in read_t3_titles(&userdata.join("statev2.sqlite"), id)? {
            summaries
                .entry(id)
                .and_modify(|summary| summary.title = Some(title.clone()))
                .or_insert(Summary {
                    title: Some(title),
                    project: None,
                    parent: None,
                });
        }
    }
    Ok(summaries)
}

fn read_t3_titles(path: &Path, id: Option<&str>) -> Result<BTreeMap<String, String>> {
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("failed to open T3 session index {}", path.display()))?;
    db.busy_timeout(Duration::from_millis(200))?;
    let mut statement = db.prepare(
        "SELECT json_extract(p.payload_json, '$.nativeThreadRef.nativeId') AS native_id, t.title
         FROM orchestration_v2_projection_provider_threads p
         JOIN orchestration_v2_projection_threads t ON t.thread_id = p.thread_id
         WHERE p.driver = 'antigravity' AND t.deleted_at IS NULL
           AND native_id IS NOT NULL AND (?1 IS NULL OR native_id = ?1)
         ORDER BY (t.active_provider_thread_id = p.provider_thread_id) DESC, t.updated_at DESC, t.thread_id",
    )?;
    let mut rows = statement.query([id])?;
    let mut titles = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        if let Some(title) = sessions::clean_session_title(Some(row.get(1)?)) {
            titles.entry(id).or_insert(title);
        }
    }
    Ok(titles)
}

fn workspace_path(value: &str) -> Option<PathBuf> {
    serde_json::from_str::<Vec<String>>(value)
        .ok()?
        .iter()
        .filter_map(|uri| url::Url::parse(uri).ok())
        .find_map(|uri| uri.to_file_path().ok())
}

pub(super) fn recognizes(value: &Value) -> bool {
    value.get("step_index").and_then(Value::as_u64).is_some()
        && value.get("type").and_then(Value::as_str).is_some()
        && value.get("source").and_then(Value::as_str).is_some()
        && value.get("status").and_then(Value::as_str).is_some()
}

pub(super) fn user_message(value: &Value) -> Option<&str> {
    if step_type(value)? != "USER_INPUT" {
        return None;
    }
    let text = content(value, "content")?;
    let request = text
        .strip_prefix("<USER_REQUEST>")
        .and_then(|request| request.split_once("</USER_REQUEST>"))
        .map_or(text, |(request, _)| request.trim());
    (!request.is_empty()).then_some(request)
}

pub(super) fn assistant_message(value: &Value) -> bool {
    step_type(value) == Some("PLANNER_RESPONSE") && content(value, "content").is_some()
}

pub(super) fn timestamp(value: &Value) -> Option<&str> {
    value.get("created_at").and_then(Value::as_str)
}

pub(super) fn parse_analytics_line(line: &str, record: &mut SessionAnalyticsRecord) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        record.analytics.malformed_lines += 1;
        return;
    };
    if !matches!(step_type(&value), Some("PLANNER_RESPONSE" | "CHECKPOINT")) {
        return;
    }
    let Some(time) = timestamp(&value) else {
        return;
    };
    let tokens = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or_default();
    // Antigravity records cache reads separately from fresh input.
    let cached_input_tokens = tokens("cache_read_tokens");
    let input_tokens = tokens("input_tokens").saturating_add(cached_input_tokens);
    let output_tokens = tokens("output_tokens");
    record.record_response_usage(
        time.to_owned(),
        content(&value, "model").unwrap_or_default().to_owned(),
        AnalyticsTokenUsage {
            input_tokens,
            cached_input_tokens,
            output_tokens,
            total_tokens: input_tokens.saturating_add(output_tokens),
            ..AnalyticsTokenUsage::default()
        },
    );
}

fn step_type(value: &Value) -> Option<&str> {
    value.get("type").and_then(Value::as_str)
}

fn content<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

pub(super) fn parse_step(value: &Value, items: &mut Vec<TranscriptItem>) {
    let Some(kind) = step_type(value) else { return };
    let time = timestamp(value).map(str::to_owned);
    match kind {
        "USER_INPUT" => {
            if let Some(text) = user_message(value) {
                transcript::push_item(items, "user", text.to_owned(), None, time);
            }
        }
        "PLANNER_RESPONSE" => {
            if let Some(thinking) = content(value, "thinking") {
                transcript::push_item(items, "thinking", thinking.to_owned(), None, time.clone());
            }
            if let Some(text) = content(value, "content") {
                transcript::push_item(items, "assistant", text.to_owned(), None, time.clone());
            }
            for call in value
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(name) = content(call, "name") else {
                    continue;
                };
                transcript::push_tool_item(
                    items,
                    "tool",
                    name.to_owned(),
                    Some(name.to_owned()),
                    time.clone(),
                    transcript::extract_tool_command(
                        &serde_json::json!({"arguments":call.get("args")}),
                    ),
                    None,
                    None,
                    None,
                    None,
                );
            }
        }
        "CONVERSATION_HISTORY" | "CHECKPOINT" | "SYSTEM_MESSAGE" | "GENERIC" => {}
        _ => {
            if value.get("source").and_then(Value::as_str) == Some("MODEL")
                && let Some(text) = content(value, "content")
            {
                let name = kind.to_ascii_lowercase();
                transcript::push_tool_item(
                    items,
                    "tool",
                    name.clone(),
                    Some(name),
                    time,
                    None,
                    Some(transcript::truncate_text(text, 12_000)),
                    None,
                    None,
                    None,
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "antigravity_sessions_tests.rs"]
mod tests;
