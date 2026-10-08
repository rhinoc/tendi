use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, VecDeque},
    fs,
    io::{BufRead, BufReader},
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

    let mut cache_hits = 0;
    let mut cache_misses = 0;
    let mut miss_reasons = BTreeMap::<&'static str, usize>::new();

    for entry in WalkDir::new(root)
        .follow_links(true)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "meta.json")
    {
        let path = entry.into_path();
        if scan_cursor_meta_file(&path, sessions, agent, cache) {
            cache_hits += 1;
        } else {
            cache_misses += 1;
            *miss_reasons
                .entry(cursor_cache_miss_reason(&path, agent, cache))
                .or_default() += 1;
        }
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
        if scan_cursor_store_file(&path, sessions, agent, cache) {
            cache_hits += 1;
        } else {
            cache_misses += 1;
            *miss_reasons
                .entry(cursor_cache_miss_reason(&path, agent, cache))
                .or_default() += 1;
        }
    }
    crate::logging::global().info(
        "cursor store scan completed",
        serde_json::json!({"cacheHits": cache_hits, "cacheMisses": cache_misses, "missReasons": miss_reasons}),
    );
}

fn cursor_cache_miss_reason(
    path: &Path,
    agent: AgentKind,
    cache: Option<&SessionScanCache>,
) -> &'static str {
    let Some(cache) = cache else {
        return "cache_absent";
    };
    let Some(id) = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|id| id.to_str())
    else {
        return "id_unavailable";
    };
    cache.session_id_cache_miss_reason(agent, id)
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
) -> bool {
    let Some(id) = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
    else {
        return false;
    };
    if cache.is_some_and(|cache| cache.empty_source_if_current(agent, path)) {
        return true;
    }
    if let Some(session) = cache.and_then(|cache| cache.session_if_current_id(agent, &id)) {
        sessions.push(session);
        return true;
    }
    let source_states = sessions::scan_source_states(agent, path);
    let value = fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let file_updated_at = sessions::file_modified_iso(path);
    let store_path = path.parent().map(|parent| parent.join("store.db"));
    let store_available = store_path.as_ref().is_some_and(|path| path.is_file());
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
        if value.is_some() && (!store_available || store_meta.scan_complete) {
            if let Some(cache) = cache {
                cache.record_empty_source(agent, path, source_states);
            }
        }
        return false;
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
    false
}

pub(crate) fn scan_cursor_store_file(
    path: &Path,
    sessions: &mut Vec<SessionRecord>,
    agent: AgentKind,
    cache: Option<&SessionScanCache>,
) -> bool {
    let Some(id) = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
    else {
        return false;
    };
    if cache.is_some_and(|cache| cache.empty_source_if_current(agent, path)) {
        return true;
    }
    if let Some(session) = cache.and_then(|cache| cache.session_if_current_id(agent, &id)) {
        sessions.push(session);
        return true;
    }
    let source_states = sessions::scan_source_states(agent, path);
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
        if store_meta.scan_complete {
            if let Some(cache) = cache {
                cache.record_empty_source(agent, path, source_states);
            }
        }
        return false;
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
    false
}

pub(crate) fn session_scan_source_paths(path: &Path) -> Vec<PathBuf> {
    let mut paths = vec![path.to_path_buf()];
    let store_paths = match path.file_name().and_then(|name| name.to_str()) {
        Some("store.db") => vec![path.to_path_buf()],
        Some("meta.json") => path
            .parent()
            .map(|parent| parent.join("store.db"))
            .into_iter()
            .collect(),
        _ if cursor_transcript_project_dir(path).is_some() => native_session_meta_paths(path)
            .into_iter()
            .map(|path| path.with_file_name("store.db"))
            .collect(),
        _ => find_cursor_store_db(path).into_iter().collect(),
    };
    for store_path in store_paths {
        if !paths.contains(&store_path) {
            paths.push(store_path.clone());
        }
        let meta_path = store_path.with_file_name("meta.json");
        if !paths.contains(&meta_path) {
            paths.push(meta_path);
        }
        for suffix in ["-wal", "-journal"] {
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
    scan_complete: bool,
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
    journal: Option<CursorStoreFileMetadata>,
}

pub(crate) fn cursor_store_source_state_current(path: &Path, cached_size: i64) -> Option<bool> {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("store.db-shm") => Some(true),
        Some("store.db-wal") if cached_size == 0 => {
            Some(fs::metadata(path).map_or(true, |metadata| metadata.len() == 0))
        }
        _ => None,
    }
}

pub(crate) fn cursor_store_source_index_state(path: &Path) -> Option<(i64, i64)> {
    (path.file_name().and_then(|name| name.to_str()) == Some("store.db-wal")
        && fs::metadata(path).map_or(true, |metadata| metadata.len() == 0))
    .then_some((0, 0))
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
        wal: cursor_store_file_metadata(&cursor_store_sidecar(path, "-wal"))
            .filter(|metadata| metadata.size > 0),
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

    let stored_meta_result =
        connection.query_row("select value from meta where key = '0'", [], |row| {
            row.get::<_, String>(0)
        });
    let stored_meta_valid = match &stored_meta_result {
        Ok(text) => parse_cursor_store_value(text).is_some(),
        Err(rusqlite::Error::QueryReturnedNoRows) => true,
        Err(_) => false,
    };
    let stored_meta = stored_meta_result
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
    let mut scan_complete = false;
    if let Ok(mut statement) = connection.prepare("select data from blobs") {
        if let Ok(rows) = statement.query_map([], |row| row.get::<_, Vec<u8>>(0)) {
            scan_complete = stored_meta_valid;
            for bytes in rows {
                let Ok(bytes) = bytes else {
                    scan_complete = false;
                    continue;
                };
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
        scan_complete,
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
    if scan_complete {
        cursor_store_cache_put(&path, version, meta.clone());
    }
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
        if item_type != Some("tool-call") {
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
        if let Some(catalog_start) = text[..=start].rfind("<available_skills") {
            let catalog_closed = text[catalog_start..start].contains("</available_skills>");
            if !catalog_closed {
                let Some(end) = text[start..].find("</available_skills>") else {
                    break;
                };
                search_from = start + end + "</available_skills>".len();
                continue;
            }
        }
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
    let project_dir = cursor_transcript_project_dir(path)?;
    memoized_project(project_dir, || resolve_cursor_project(project_dir))
}

fn resolve_cursor_project(project_dir: &Path) -> Option<PathBuf> {
    let key = project_dir.file_name()?.to_str()?;
    let terminals = project_dir.join("terminals");
    let mut sources = vec![terminals.clone()];
    let terminal_files = cursor_directory_files(&terminals, "txt");
    sources.extend(terminal_files.iter().cloned());
    let workspace_root = super::cursor::cursor_state_db_path(project_dir.parent()?)?
        .parent()?
        .parent()?
        .join("workspaceStorage");
    sources.push(workspace_root.clone());
    let workspaces: Vec<_> = fs::read_dir(&workspace_root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("workspace.json"))
        .filter(|path| path.is_file())
        .collect();
    sources.extend(workspaces.iter().cloned());
    sources.sort();
    let version: Vec<_> = sources
        .into_iter()
        .map(|path| {
            let state = cursor_store_file_metadata(&path);
            (path, state)
        })
        .collect();
    let mut cache = CURSOR_PROJECT_CACHE.lock().ok()?;
    if let Some((cached_version, project)) = cache.get(project_dir) {
        if cached_version == &version {
            return project.clone();
        }
    }
    let mut candidates = std::collections::BTreeSet::new();
    for path in terminal_files {
        if let Some(project) = cursor_terminal_cwd(&path) {
            for ancestor in project.ancestors() {
                if cursor_project_key(ancestor) == key {
                    candidates.insert(ancestor.to_path_buf());
                }
            }
        }
    }
    for path in workspaces {
        let project = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| {
                value
                    .get("folder")
                    .and_then(Value::as_str)
                    .and_then(|uri| url::Url::parse(uri).ok())
                    .and_then(|url| url.to_file_path().ok())
            });
        if let Some(project) = project {
            if cursor_project_key(&project) == key {
                candidates.insert(project);
            }
        }
    }
    // The directory key is lossy. Only unambiguous original paths are evidence.
    let project = (candidates.len() == 1)
        .then(|| candidates.into_iter().next())
        .flatten();
    if cache.len() >= 128 {
        cache.clear();
    }
    cache.insert(project_dir.to_path_buf(), (version, project.clone()));
    project
}

type CursorProjectVersion = Vec<(PathBuf, Option<CursorStoreFileMetadata>)>;
type CursorProjectCache = HashMap<PathBuf, (CursorProjectVersion, Option<PathBuf>)>;
static CURSOR_PROJECT_CACHE: LazyLock<Mutex<CursorProjectCache>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

type CursorScanProjects = HashMap<PathBuf, Option<PathBuf>>;
type CursorNativeSources = HashMap<PathBuf, HashMap<String, Vec<PathBuf>>>;
thread_local! {
    static CURSOR_SCAN_PROJECTS: RefCell<Option<CursorScanProjects>> = const { RefCell::new(None) };
    static CURSOR_SCAN_NATIVE_SOURCES: RefCell<Option<CursorNativeSources>> = const { RefCell::new(None) };
}

pub(super) struct CursorProjectScanCache {
    projects: Option<CursorScanProjects>,
    native_sources: Option<CursorNativeSources>,
    entered: bool,
}

pub(super) fn project_scan_cache() -> CursorProjectScanCache {
    if CURSOR_SCAN_PROJECTS.with(|cache| cache.borrow().is_some()) {
        return CursorProjectScanCache {
            projects: None,
            native_sources: None,
            entered: false,
        };
    }
    CursorProjectScanCache {
        projects: CURSOR_SCAN_PROJECTS.with(|cache| cache.replace(Some(HashMap::new()))),
        native_sources: CURSOR_SCAN_NATIVE_SOURCES
            .with(|cache| cache.replace(Some(HashMap::new()))),
        entered: true,
    }
}

impl Drop for CursorProjectScanCache {
    fn drop(&mut self) {
        if !self.entered {
            return;
        }
        CURSOR_SCAN_PROJECTS.with(|cache| cache.replace(self.projects.take()));
        CURSOR_SCAN_NATIVE_SOURCES.with(|cache| cache.replace(self.native_sources.take()));
    }
}

fn native_session_meta_paths(path: &Path) -> Vec<PathBuf> {
    let Some(root) = path
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == ".cursor"))
    else {
        return Vec::new();
    };
    let Some(id) = path.file_stem().and_then(|id| id.to_str()) else {
        return Vec::new();
    };
    let cached = CURSOR_SCAN_NATIVE_SOURCES.with(|cache| {
        cache
            .borrow()
            .as_ref()
            .and_then(|cache| cache.get(root))
            .map(|index| index.get(id).cloned().unwrap_or_default())
    });
    if let Some(paths) = cached {
        return paths;
    }
    let mut index = HashMap::<String, Vec<PathBuf>>::new();
    // Match provider scan order: ACP metadata precedes chat metadata.
    for directory in ["acp-sessions", "chats"] {
        let mut paths: Vec<_> = WalkDir::new(root.join(directory))
            .follow_links(true)
            .max_depth(4)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_type().is_file()
                    && matches!(entry.file_name().to_str(), Some("meta.json" | "store.db"))
            })
            .map(|entry| entry.into_path().with_file_name("meta.json"))
            .collect();
        paths.sort();
        paths.dedup();
        for meta in paths {
            if let Some(id) = meta
                .parent()
                .and_then(Path::file_name)
                .and_then(|id| id.to_str())
            {
                index.entry(id.to_string()).or_default().push(meta);
            }
        }
    }
    let paths = index.get(id).cloned().unwrap_or_default();
    CURSOR_SCAN_NATIVE_SOURCES.with(|cache| {
        if let Some(cache) = cache.borrow_mut().as_mut() {
            cache.insert(root.to_path_buf(), index);
        }
    });
    paths
}

pub(super) fn session_project(path: &Path, explicit: Option<PathBuf>) -> Option<PathBuf> {
    native_session_project(path)
        .or(explicit)
        .or_else(|| cursor_project_from_transcript_path(path))
}

fn native_session_project(path: &Path) -> Option<PathBuf> {
    native_session_meta_paths(path)
        .into_iter()
        .find_map(|meta| {
            memoized_project(&meta, || {
                let value = fs::read_to_string(&meta)
                    .ok()
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok());
                cursor_project_from_meta(value.as_ref())
            })
        })
}

fn memoized_project(path: &Path, resolve: impl FnOnce() -> Option<PathBuf>) -> Option<PathBuf> {
    if let Some(project) = CURSOR_SCAN_PROJECTS.with(|cache| {
        cache
            .borrow()
            .as_ref()
            .and_then(|cache| cache.get(path).cloned())
    }) {
        return project;
    }
    let project = resolve();
    CURSOR_SCAN_PROJECTS.with(|cache| {
        if let Some(cache) = cache.borrow_mut().as_mut() {
            cache.insert(path.to_path_buf(), project.clone());
        }
    });
    project
}

fn cursor_transcript_project_dir(path: &Path) -> Option<&Path> {
    path.ancestors().find(|path| {
        path.parent().is_some_and(|parent| {
            parent.file_name().is_some_and(|name| name == "projects")
                && parent
                    .parent()
                    .is_some_and(|cursor| cursor.file_name().is_some_and(|name| name == ".cursor"))
        })
    })
}

fn cursor_project_key(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .filter(|ch| *ch != '.')
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_start_matches('-')
        .to_string()
}

fn cursor_directory_files(path: &Path, extension: &str) -> Vec<PathBuf> {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == extension))
        .collect()
}

fn cursor_terminal_cwd(path: &Path) -> Option<PathBuf> {
    let mut lines = BufReader::new(fs::File::open(path).ok()?).lines();
    if lines.next()?.ok()?.trim() != "---" {
        return None;
    }
    for line in lines.take(16).map_while(Result::ok) {
        if line.trim() == "---" {
            break;
        }
        if let Some(value) = line.strip_prefix("cwd:") {
            let value: serde_yaml::Value = serde_yaml::from_str(value.trim()).ok()?;
            let path = PathBuf::from(value.as_str()?);
            return path.is_absolute().then_some(path);
        }
    }
    None
}

pub(crate) fn cached_project_is_current(session: &SessionRecord) -> bool {
    if cursor_transcript_project_dir(&session.path).is_none() {
        return true;
    }
    if let Some(project) = native_session_project(&session.path) {
        return session.project.as_ref() == Some(&project);
    }
    let project = cursor_project_from_transcript_path(&session.path);
    if project == session.project {
        return true;
    }
    // Session metadata and explicit transcript cwd precede project-wide evidence.
    let explicit = memoized_project(&session.path, || {
        let version = vec![(
            session.path.clone(),
            cursor_store_file_metadata(&session.path),
        )];
        if let Ok(mut cache) = CURSOR_PROJECT_CACHE.lock() {
            if let Some((cached_version, explicit)) = cache.get(&session.path) {
                if cached_version == &version {
                    return explicit.clone();
                }
            }
            let explicit =
                sessions::scan_jsonl_meta_for_agent(&session.path, Some(AgentKind::Cursor)).project;
            if cache.len() >= 128 {
                cache.clear();
            }
            cache.insert(session.path.clone(), (version, explicit.clone()));
            return explicit;
        }
        None
    });
    explicit.or(project) == session.project
}

#[cfg(test)]
#[path = "cursor_sessions_tests.rs"]
mod tests;
