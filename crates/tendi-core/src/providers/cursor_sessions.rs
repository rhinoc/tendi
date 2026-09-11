use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::UNIX_EPOCH,
};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use walkdir::WalkDir;

use crate::{
    providers::cursor::find_cursor_store_db,
    session_skills::{Evidence, SkillEvidenceCandidate},
    sessions,
};

use super::*;

pub(crate) fn scan_cursor_meta(
    root: &Path,
    sessions: &mut Vec<SessionRecord>,
    agent: AgentKind,
    cache: Option<&SessionScanCache>,
) {
    if !root.is_dir() {
        return;
    }

    for entry in WalkDir::new(root)
        .follow_links(true)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "meta.json")
    {
        let path = entry.into_path();
        scan_cursor_meta_file(&path, sessions, agent, cache);
    }

    for entry in WalkDir::new(root)
        .follow_links(true)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "store.db")
    {
        let path = entry.into_path();
        if path.with_file_name("meta.json").is_file() {
            continue;
        }
        scan_cursor_store_file(&path, sessions, agent, cache);
    }
}

pub(crate) fn is_cursor_meta_file(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("schemaVersion").and_then(Value::as_u64))
        .is_some()
}

pub(crate) fn scan_cursor_meta_file(
    path: &Path,
    sessions: &mut Vec<SessionRecord>,
    agent: AgentKind,
    cache: Option<&SessionScanCache>,
) {
    let Some(id) = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
    else {
        return;
    };
    if let Some(session) = cache.and_then(|cache| cache.session_if_current_id(agent, &id))
        && is_cursor_transcript_path(&session.path)
    {
        sessions.push(session);
        return;
    }
    let value = fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let file_updated_at = sessions::file_modified_iso(path);
    let store_path = path.parent().map(|parent| parent.join("store.db"));
    let store_meta = scan_cursor_store_db(store_path);
    let explicit_title = sessions::clean_session_title(sessions::string_field(
        value
            .as_ref()
            .and_then(|value| value.get("name").or_else(|| value.get("title"))),
    ))
    .or_else(|| store_meta.title.clone())
    .filter(|title| {
        store_meta.parent_session_id.is_none() || !title.eq_ignore_ascii_case("New Agent")
    });
    if explicit_title.is_none()
        && store_meta.message_count.is_none()
        && store_meta.model.is_none()
        && store_meta.mode.is_none()
        && store_meta.approval_mode.is_none()
        && store_meta.is_run_everything.is_none()
        && store_meta.parent_session_id.is_none()
        && store_meta.first_user_message.is_none()
        && store_meta.last_user_message.is_none()
        && store_meta.last_assistant_message.is_none()
    {
        return;
    }
    sessions.push(SessionRecord {
        id,
        agent,
        title: explicit_title,
        project: cursor_project_from_meta(value.as_ref()),
        repository: None,
        repository_url: None,
        logical_project_id: None,
        logical_project_name: None,
        path: path.to_path_buf(),
        started_at: cursor_time_field(
            value.as_ref(),
            &["createdAt", "created_at", "startedAt", "started_at"],
        )
        .or(store_meta.started_at.clone())
        .or_else(|| file_updated_at.clone()),
        updated_at: cursor_time_field(value.as_ref(), &["updatedAt", "updated_at"])
            .or(store_meta.updated_at)
            .or(file_updated_at),
        message_count: store_meta.message_count,
        first_user_message: store_meta.first_user_message,
        last_user_message: store_meta.last_user_message,
        last_assistant_message: store_meta.last_assistant_message,
        turn_count: store_meta.turn_count,
        model: store_meta.model.clone(),
        mode: store_meta.mode.clone(),
        approval_mode: store_meta.approval_mode.clone(),
        is_run_everything: store_meta.is_run_everything,
        parent_session_id: store_meta.parent_session_id,
        token_usage: None,
    });
}

pub(crate) fn scan_cursor_store_file(
    path: &Path,
    sessions: &mut Vec<SessionRecord>,
    agent: AgentKind,
    cache: Option<&SessionScanCache>,
) {
    let Some(id) = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
    else {
        return;
    };
    if let Some(session) = cache.and_then(|cache| cache.session_if_current_id(agent, &id)) {
        if is_cursor_transcript_path(&session.path) {
            sessions.push(session);
            return;
        }
    }
    let file_updated_at = sessions::file_modified_iso(path);
    let store_meta = scan_cursor_store_db(Some(path.to_path_buf()));
    if store_meta.title.is_none()
        && store_meta.message_count.is_none()
        && store_meta.model.is_none()
        && store_meta.mode.is_none()
        && store_meta.approval_mode.is_none()
        && store_meta.is_run_everything.is_none()
        && store_meta.parent_session_id.is_none()
        && store_meta.first_user_message.is_none()
        && store_meta.last_user_message.is_none()
        && store_meta.last_assistant_message.is_none()
    {
        return;
    }
    let title = store_meta.title.filter(|title| {
        store_meta.parent_session_id.is_none() || !title.eq_ignore_ascii_case("New Agent")
    });
    sessions.push(SessionRecord {
        id,
        agent,
        title,
        project: None,
        repository: None,
        repository_url: None,
        logical_project_id: None,
        logical_project_name: None,
        path: path.to_path_buf(),
        started_at: store_meta.started_at,
        updated_at: store_meta.updated_at.or(file_updated_at),
        message_count: store_meta.message_count,
        first_user_message: store_meta.first_user_message,
        last_user_message: store_meta.last_user_message,
        last_assistant_message: store_meta.last_assistant_message,
        turn_count: store_meta.turn_count,
        model: store_meta.model,
        mode: store_meta.mode,
        approval_mode: store_meta.approval_mode,
        is_run_everything: store_meta.is_run_everything,
        parent_session_id: store_meta.parent_session_id,
        token_usage: None,
    });
}

fn is_cursor_transcript_path(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl")
        && path
            .components()
            .any(|component| component.as_os_str() == "agent-transcripts")
}

pub(crate) fn session_scan_source_paths(path: &Path) -> Vec<PathBuf> {
    let mut paths = vec![path.to_path_buf()];
    let store_path = match path.file_name().and_then(|name| name.to_str()) {
        Some("store.db") => Some(path.to_path_buf()),
        Some("meta.json") => path.parent().map(|parent| parent.join("store.db")),
        _ => find_cursor_store_db(path),
    };
    if let Some(store_path) = store_path {
        if !paths.contains(&store_path) {
            paths.push(store_path.clone());
        }
        let meta_path = store_path.with_file_name("meta.json");
        if !paths.contains(&meta_path) {
            paths.push(meta_path);
        }
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = cursor_store_sidecar(&store_path, suffix);
            if !paths.contains(&sidecar) {
                paths.push(sidecar);
            }
        }
    }
    paths
}

const CURSOR_STORE_CACHE_MAX_ENTRIES: usize = 128;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CursorStoreMeta {
    pub(crate) message_count: Option<usize>,
    pub(crate) first_user_message: Option<String>,
    pub(crate) last_user_message: Option<String>,
    pub(crate) last_assistant_message: Option<String>,
    pub(crate) turn_count: Option<usize>,
    pub(crate) started_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) models: Vec<String>,
    pub(crate) mode: Option<String>,
    pub(crate) approval_mode: Option<String>,
    pub(crate) is_run_everything: Option<bool>,
    pub(crate) parent_session_id: Option<String>,
    pub(crate) tool_calls: Vec<CursorToolCall>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CursorToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) args: Value,
    pub(crate) result: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CursorStoreFileMetadata {
    size: u64,
    modified_ns: u128,
    device: u64,
    inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CursorStoreVersion {
    database: CursorStoreFileMetadata,
    wal: Option<CursorStoreFileMetadata>,
    shm: Option<CursorStoreFileMetadata>,
    journal: Option<CursorStoreFileMetadata>,
}

#[derive(Debug, Clone)]
struct CursorStoreCacheEntry {
    path: PathBuf,
    version: CursorStoreVersion,
    meta: CursorStoreMeta,
    #[cfg(test)]
    full_scans: usize,
}

#[derive(Debug, Default)]
struct CursorStoreCache {
    entries: VecDeque<CursorStoreCacheEntry>,
}

static CURSOR_STORE_CACHE: LazyLock<Mutex<CursorStoreCache>> =
    LazyLock::new(|| Mutex::new(CursorStoreCache::default()));

fn cursor_store_file_metadata(path: &Path) -> Option<CursorStoreFileMetadata> {
    let metadata = fs::metadata(path).ok()?;
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let (device, inode) = cursor_store_file_identity(&metadata);
    Some(CursorStoreFileMetadata {
        size: metadata.len(),
        modified_ns,
        device,
        inode,
    })
}

fn cursor_store_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut file_name = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("store.db"))
        .to_os_string();
    file_name.push(suffix);
    path.with_file_name(file_name)
}

fn cursor_store_version(path: &Path) -> Option<CursorStoreVersion> {
    Some(CursorStoreVersion {
        database: cursor_store_file_metadata(path)?,
        wal: cursor_store_file_metadata(&cursor_store_sidecar(path, "-wal")),
        shm: cursor_store_file_metadata(&cursor_store_sidecar(path, "-shm")),
        journal: cursor_store_file_metadata(&cursor_store_sidecar(path, "-journal")),
    })
}

#[cfg(unix)]
fn cursor_store_file_identity(metadata: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;

    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn cursor_store_file_identity(_metadata: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

fn cursor_store_cache_get(path: &Path, version: &CursorStoreVersion) -> Option<CursorStoreMeta> {
    let mut cache = CURSOR_STORE_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = cache
        .entries
        .iter()
        .position(|entry| entry.path == path && entry.version == *version)?;
    let entry = cache.entries.remove(index)?;
    let meta = entry.meta.clone();
    cache.entries.push_front(entry);
    Some(meta)
}

fn cursor_store_cache_put(path: &Path, version: CursorStoreVersion, meta: CursorStoreMeta) {
    let mut cache = CURSOR_STORE_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    #[cfg(test)]
    let full_scans = cache
        .entries
        .iter()
        .find(|entry| entry.path == path)
        .map_or(0, |entry| entry.full_scans)
        .saturating_add(1);

    cache.entries.retain(|entry| entry.path != path);
    while cache.entries.len() >= CURSOR_STORE_CACHE_MAX_ENTRIES {
        cache.entries.pop_back();
    }
    cache.entries.push_front(CursorStoreCacheEntry {
        path: path.to_path_buf(),
        version,
        meta,
        #[cfg(test)]
        full_scans,
    });
}

pub(crate) fn scan_cursor_store_db(path: Option<PathBuf>) -> CursorStoreMeta {
    let Some(path) = path else {
        return CursorStoreMeta::default();
    };
    if !path.is_file() {
        return CursorStoreMeta::default();
    }

    let Some(version) = cursor_store_version(&path) else {
        return CursorStoreMeta::default();
    };
    if let Some(meta) = cursor_store_cache_get(&path, &version) {
        return meta;
    }

    let Ok(connection) = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return CursorStoreMeta::default();
    };

    let stored_meta = connection
        .query_row("select value from meta where key = '0'", [], |row| {
            row.get::<_, String>(0)
        })
        .ok()
        .and_then(|text| parse_cursor_store_value(&text));
    let mut title = stored_meta.as_ref().and_then(|value| {
        sessions::clean_session_title(sessions::string_field(
            value.get("name").or_else(|| value.get("title")),
        ))
    });
    let mut model = stored_meta
        .as_ref()
        .and_then(|value| cursor_store_model(value));
    let mode = stored_meta
        .as_ref()
        .and_then(|value| sessions::string_field(value.get("mode")));
    let approval_mode = stored_meta.as_ref().and_then(|value| {
        sessions::string_field(
            value
                .get("approvalMode")
                .or_else(|| value.get("approval_mode")),
        )
    });
    let is_run_everything = stored_meta.as_ref().and_then(|value| {
        value
            .get("isRunEverything")
            .or_else(|| value.get("is_run_everything"))
            .and_then(Value::as_bool)
    });
    let started_at = stored_meta.as_ref().and_then(|value| {
        cursor_time_field(
            Some(value),
            &["createdAt", "created_at", "startedAt", "started_at"],
        )
    });
    let parent_session_id = stored_meta
        .as_ref()
        .and_then(|value| value.pointer("/subagentInfo/parentAgentId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);

    let mut message_count = 0usize;
    let mut turn_count = 0usize;
    let mut first_user_message = None;
    let mut last_user_message = None;
    let mut last_assistant_message = None;
    let mut models = Vec::new();
    let mut tool_calls = Vec::new();
    let mut pending_tool_results = HashMap::new();
    if let Ok(mut statement) = connection.prepare("select data from blobs") {
        if let Ok(rows) = statement.query_map([], |row| row.get::<_, Vec<u8>>(0)) {
            for bytes in rows.filter_map(Result::ok) {
                let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                    continue;
                };
                if value.get("role").and_then(Value::as_str).is_some() {
                    message_count += 1;
                }
                if sessions::extract_session_title_for_agent(AgentKind::Cursor, &value).is_some() {
                    turn_count += 1;
                }
                if title.is_none() {
                    title = sessions::extract_session_title_for_agent(AgentKind::Cursor, &value);
                }
                if let Some(blob_model) = extract_cursor_blob_model(&value) {
                    if model.is_none() {
                        model = Some(blob_model.clone());
                    }
                    models.push(blob_model);
                }
                if let Some((role, body)) =
                    sessions::extract_session_message_for_agent(AgentKind::Cursor, &value)
                {
                    if let Some(body) = sessions::clean_preview_text(&body) {
                        match role {
                            "user" => {
                                if first_user_message.is_none() {
                                    first_user_message = Some(body.clone());
                                }
                                last_user_message = Some(body);
                            }
                            "assistant" => last_assistant_message = Some(body),
                            _ => {}
                        }
                    }
                }
                if let Some(content) = value.get("content").and_then(Value::as_array) {
                    for item in content {
                        match item.get("type").and_then(Value::as_str) {
                            Some("tool-call") => {
                                let Some(id) = item
                                    .get("toolCallId")
                                    .or_else(|| item.get("tool_call_id"))
                                    .and_then(Value::as_str)
                                    .filter(|id| !id.trim().is_empty())
                                else {
                                    continue;
                                };
                                let Some(name) = item
                                    .get("toolName")
                                    .or_else(|| item.get("tool_name"))
                                    .and_then(Value::as_str)
                                    .filter(|name| !name.trim().is_empty())
                                else {
                                    continue;
                                };
                                tool_calls.push(CursorToolCall {
                                    id: id.to_string(),
                                    name: name.to_string(),
                                    args: item.get("args").cloned().unwrap_or(Value::Null),
                                    result: pending_tool_results.remove(id),
                                });
                            }
                            Some("tool-result") => {
                                let Some(id) = item
                                    .get("toolCallId")
                                    .or_else(|| item.get("tool_call_id"))
                                    .and_then(Value::as_str)
                                    .filter(|id| !id.trim().is_empty())
                                else {
                                    continue;
                                };
                                let Some(result) = cursor_store_result_text(
                                    item.get("result").or_else(|| item.get("content")),
                                ) else {
                                    continue;
                                };
                                if let Some(call) =
                                    tool_calls.iter_mut().rev().find(|call| call.id == id)
                                {
                                    call.result = Some(result);
                                } else {
                                    pending_tool_results.insert(id.to_string(), result);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    let meta = CursorStoreMeta {
        message_count: (message_count > 0).then_some(message_count),
        first_user_message,
        last_user_message,
        last_assistant_message,
        turn_count: (turn_count > 0).then_some(turn_count),
        started_at,
        updated_at: None,
        title,
        model,
        models,
        mode,
        approval_mode,
        is_run_everything,
        parent_session_id,
        tool_calls,
    };
    cursor_store_cache_put(&path, version, meta.clone());
    meta
}

pub(crate) fn cursor_store_models_for_path(path: &Path) -> Vec<String> {
    let store_path = (path.is_file()
        && path.file_name().and_then(|name| name.to_str()) == Some("store.db"))
    .then(|| path.to_path_buf())
    .or_else(|| find_cursor_store_db(path));
    store_path
        .map(|store_path| scan_cursor_store_db(Some(store_path)).models)
        .unwrap_or_default()
}

pub(crate) fn cursor_store_tool_calls_for_path(path: &Path) -> Vec<CursorToolCall> {
    scan_cursor_store_db(Some(path.to_path_buf())).tool_calls
}

pub(crate) fn cursor_store_skill_evidence_for_path(path: &Path) -> Vec<SkillEvidenceCandidate> {
    if path.file_name().and_then(|name| name.to_str()) != Some("store.db") || !path.is_file() {
        return Vec::new();
    }
    let Ok(connection) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let Ok(mut statement) = connection.prepare("select data from blobs") else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], |row| row.get::<_, Vec<u8>>(0)) else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    for bytes in rows.filter_map(Result::ok) {
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        collect_cursor_skill_evidence(&value, &mut candidates);
    }
    candidates
}

fn collect_cursor_skill_evidence(value: &Value, out: &mut Vec<SkillEvidenceCandidate>) {
    let time = crate::providers::cursor::cursor_event_timestamp(value);
    let mut strings = Vec::new();
    collect_value_strings(value, &mut strings);
    for text in strings.iter().copied() {
        for path in cursor_agent_skill_paths(text) {
            out.push(SkillEvidenceCandidate {
                name: None,
                path: Some(path),
                evidence: Evidence {
                    kind: "agent_skill".to_string(),
                    text: text.to_string(),
                    time: time.clone(),
                },
                confidence: "explicit",
            });
        }
    }

    let Some(content) = value.get("content").and_then(Value::as_array) else {
        return;
    };
    for item in content {
        let item_type = item.get("type").and_then(Value::as_str);
        if !matches!(item_type, Some("tool-call" | "tool-result")) {
            continue;
        }
        let tool_name = item
            .get("toolName")
            .or_else(|| item.get("tool_name"))
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("tool")
            .to_string();
        let evidence_text = serde_json::to_string(item).unwrap_or_default();
        let mut item_strings = Vec::new();
        collect_value_strings(item, &mut item_strings);
        for text in item_strings {
            for path in cursor_skill_file_paths(text) {
                out.push(SkillEvidenceCandidate {
                    name: None,
                    path: Some(path),
                    evidence: Evidence {
                        kind: tool_name.clone(),
                        text: evidence_text.clone(),
                        time: time.clone(),
                    },
                    confidence: "observed",
                });
            }
        }
    }
}

fn cursor_skill_file_paths(text: &str) -> Vec<String> {
    let mut paths = crate::session_skills::skill_file_candidate_paths(text);
    let mut search_from = 0;
    while let Some(relative_start) = text[search_from..].find("SKILL.md") {
        let end = search_from + relative_start + "SKILL.md".len();
        let mut start = search_from + relative_start;
        while start > 0 {
            let previous = text[..start].chars().next_back().unwrap_or_default();
            if previous.is_whitespace()
                || matches!(
                    previous,
                    '"' | '\''
                        | '`'
                        | '<'
                        | '>'
                        | '|'
                        | ';'
                        | '&'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                )
            {
                break;
            }
            start -= previous.len_utf8();
        }
        let candidate = text[start..end].trim_matches(|ch| {
            matches!(
                ch,
                '"' | '\'' | '`' | '<' | '>' | '[' | ']' | '(' | ')' | '{' | '}'
            )
        });
        if candidate.contains('/') && !paths.iter().any(|path| path == candidate) {
            paths.push(candidate.to_string());
        }
        search_from = end;
    }
    paths
}

fn collect_value_strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => out.push(text),
        Value::Array(items) => {
            for item in items {
                collect_value_strings(item, out);
            }
        }
        Value::Object(object) => {
            for value in object.values() {
                collect_value_strings(value, out);
            }
        }
        _ => {}
    }
}

fn cursor_agent_skill_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut search_from = 0;
    while let Some(relative_start) = text[search_from..].find("<agent_skill") {
        let start = search_from + relative_start;
        let Some(relative_end) = text[start..].find('>') else {
            break;
        };
        let tag = &text[start..start + relative_end];
        for attribute in ["fullPath", "full_path", "path"] {
            if let Some(path) =
                xml_attribute_value(tag, attribute).filter(|path| path.ends_with("SKILL.md"))
            {
                if !paths.contains(&path) {
                    paths.push(path);
                }
                break;
            }
        }
        search_from = start + relative_end + 1;
    }
    paths
}

fn xml_attribute_value(text: &str, attribute: &str) -> Option<String> {
    let marker = format!("{attribute}=");
    let start = text.find(&marker)? + marker.len();
    let quote = text.as_bytes().get(start).copied()?;
    if !matches!(quote, b'\"' | b'\'') {
        return None;
    }
    let value_start = start + 1;
    let value_end = text[value_start..].find(quote as char)? + value_start;
    let value = text[value_start..value_end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn cursor_store_result_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    let text = match value {
        Value::String(text) => text.trim().to_string(),
        Value::Object(_) | Value::Array(_) => serde_json::to_string_pretty(value).ok()?,
        _ => value.to_string(),
    };
    (!text.is_empty()).then_some(text)
}

pub(crate) fn cursor_store_model(value: &Value) -> Option<String> {
    sessions::string_field(value.get("lastUsedModel").or_else(|| value.get("model")))
        .filter(|model| !model.eq_ignore_ascii_case("default"))
}

pub(crate) fn extract_cursor_blob_model(value: &Value) -> Option<String> {
    if let Some(content) = value.get("content").and_then(Value::as_array) {
        for item in content.iter().rev() {
            if let Some(model) = item
                .pointer("/providerOptions/cursor/modelName")
                .and_then(|model| sessions::string_field(Some(model)))
            {
                return Some(model);
            }
        }
    }
    value
        .pointer("/providerOptions/cursor/modelName")
        .and_then(|model| sessions::string_field(Some(model)))
}

fn parse_cursor_store_value(text: &str) -> Option<Value> {
    serde_json::from_str(text).ok().or_else(|| {
        if !text.len().is_multiple_of(2) {
            return None;
        }
        let bytes = text
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let pair = std::str::from_utf8(pair).ok()?;
                u8::from_str_radix(pair, 16).ok()
            })
            .collect::<Option<Vec<_>>>()?;
        serde_json::from_slice(&bytes).ok()
    })
}

pub(crate) fn cursor_project_from_meta(value: Option<&Value>) -> Option<PathBuf> {
    value
        .and_then(|value| {
            value
                .get("cwd")
                .or_else(|| value.get("workspace"))
                .or_else(|| value.get("workspacePath"))
                .or_else(|| value.get("folder"))
                .or_else(|| value.get("folderPath"))
        })
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

pub(crate) fn cursor_time_field(value: Option<&Value>, keys: &[&str]) -> Option<String> {
    let value = value?;
    for key in keys {
        if let Some(timestamp) = value.get(*key).and_then(Value::as_str) {
            return Some(timestamp.to_string());
        }
        if let Some(timestamp) = value.get(format!("{key}Ms")).and_then(Value::as_i64) {
            return sessions::unix_ms_to_iso(timestamp);
        }
    }
    value
        .get("createdAtMs")
        .filter(|_| keys.iter().any(|key| key.starts_with("created")))
        .and_then(Value::as_i64)
        .and_then(sessions::unix_ms_to_iso)
        .or_else(|| {
            value
                .get("updatedAtMs")
                .filter(|_| keys.iter().any(|key| key.starts_with("updated")))
                .and_then(Value::as_i64)
                .and_then(sessions::unix_ms_to_iso)
        })
}

pub(crate) fn cursor_project_from_transcript_path(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    while let Some(component) = components.next() {
        if component.as_os_str() == "projects" {
            let project = components.next()?.as_os_str().to_str()?;
            return decode_cursor_project_dir(project);
        }
    }
    None
}

pub(crate) fn decode_cursor_project_dir(value: &str) -> Option<PathBuf> {
    let parts = value
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    if parts[0] == "Users" {
        return Some(PathBuf::from(format!("/{}", parts.join("/"))));
    }
    Some(PathBuf::from(parts.join("/")))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use rusqlite::Connection;
    use serde_json::json;

    use super::*;

    #[test]
    fn extracts_skill_evidence_from_cursor_store() {
        let root = std::env::temp_dir().join(format!(
            "tendi-cursor-skill-evidence-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp directory");
        let skill_path = root.join(".ctx/plans/e2e/SKILL.md");
        fs::create_dir_all(skill_path.parent().expect("skill parent")).expect("create skill");
        let store_path = root.join("store.db");
        let connection = Connection::open(&store_path).expect("open store");
        connection
            .execute("CREATE TABLE blobs (data BLOB)", [])
            .expect("create blobs table");
        for value in [
            json!({
                "role": "assistant",
                "content": [{
                    "type": "tool-call",
                    "toolCallId": "read-1",
                    "toolName": "Read",
                    "args": { "path": skill_path }
                }]
            }),
            json!({
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": format!("<agent_skill fullPath=\"{}\">", skill_path.display())
                }]
            }),
        ] {
            let data = value.to_string();
            connection
                .execute("INSERT INTO blobs (data) VALUES (?1)", [data.as_bytes()])
                .expect("insert blob");
        }
        drop(connection);

        let evidence = cursor_store_skill_evidence_for_path(&store_path);
        assert!(evidence.iter().any(|candidate| {
            candidate.path.as_deref() == Some(skill_path.to_str().expect("skill path"))
                && candidate.evidence.kind == "Read"
                && candidate.confidence == "observed"
        }));
        assert!(evidence.iter().any(|candidate| {
            candidate.path.as_deref() == Some(skill_path.to_str().expect("skill path"))
                && candidate.evidence.kind == "agent_skill"
                && candidate.confidence == "explicit"
        }));

        fs::remove_dir_all(root).expect("remove temp directory");
    }
}
