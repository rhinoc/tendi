use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::{
    sessions::SessionRecord,
    skills::AgentKind,
    transcript::{TranscriptScan, collect_cursor_item_with_timestamp},
};

pub(super) fn desktop_database(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    let user_data = home.join("Library/Application Support/Cursor");
    #[cfg(target_os = "windows")]
    let user_data = home.join("AppData/Roaming/Cursor");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let user_data = home.join(".config/Cursor");
    user_data.join("User/globalStorage/state.vscdb")
}

pub(super) fn is_desktop(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "state.vscdb")
        && path.parent().is_some_and(|parent| {
            parent
                .file_name()
                .is_some_and(|name| name == "globalStorage")
        })
}

pub(super) fn is_sdk(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "index.db")
        && path.ancestors().any(|part| {
            part.file_name()
                .is_some_and(|name| name == "sdk-agent-store")
        })
}

pub(super) fn desktop_database_for_export(path: &Path) -> Option<PathBuf> {
    if !path
        .extension()
        .is_some_and(|extension| extension == "jsonl")
    {
        return None;
    }
    let home = path
        .ancestors()
        .find(|part| part.file_name().is_some_and(|name| name == ".cursor"))?
        .parent()?;
    Some(desktop_database(home))
}

pub(super) fn is_cli_store(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("meta.json" | "store.db")
    ) && path.ancestors().any(|part| {
        matches!(
            part.file_name().and_then(|name| name.to_str()),
            Some("chats" | "acp-sessions")
        )
    })
}

pub(super) fn sdk_index_for_blob_store(path: &Path) -> Option<PathBuf> {
    let path = path
        .ancestors()
        .find(|part| part.file_name().is_some_and(|name| name == "agents"))?
        .parent()?
        .join("index.db");
    is_sdk(&path).then_some(path)
}

fn open(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("failed to read Cursor session store {}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(5))?;
    Ok(connection)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CursorDatabaseFileVersion {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
}

type CursorDatabaseVersion = Vec<(PathBuf, Option<CursorDatabaseFileVersion>)>;

struct CursorDesktopArchiveSnapshot {
    version: CursorDatabaseVersion,
    loaded_at: Instant,
    states: Option<Arc<HashMap<String, bool>>>,
}

static CURSOR_DESKTOP_ARCHIVE_SNAPSHOTS: LazyLock<
    Mutex<HashMap<PathBuf, CursorDesktopArchiveSnapshot>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn database_version(path: &Path) -> CursorDatabaseVersion {
    database_sources(path)
        .into_iter()
        .map(|source| {
            let version = fs::metadata(&source).ok().map(|metadata| {
                #[cfg(unix)]
                use std::os::unix::fs::MetadataExt;

                CursorDatabaseFileVersion {
                    len: metadata.len(),
                    modified: metadata.modified().ok(),
                    #[cfg(unix)]
                    device: metadata.dev(),
                    #[cfg(unix)]
                    inode: metadata.ino(),
                    #[cfg(unix)]
                    changed_seconds: metadata.ctime(),
                    #[cfg(unix)]
                    changed_nanoseconds: metadata.ctime_nsec(),
                }
            });
            (source, version)
        })
        .collect()
}

fn desktop_archive_states(path: &Path) -> Option<Arc<HashMap<String, bool>>> {
    let version = database_version(path);
    if version.first().is_none_or(|(_, version)| version.is_none()) {
        return None;
    }

    let mut snapshots = CURSOR_DESKTOP_ARCHIVE_SNAPSHOTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(snapshot) = snapshots
        .get(path)
        .filter(|snapshot| snapshot.version == version)
    {
        if let Some(states) = &snapshot.states {
            return Some(Arc::clone(states));
        }
        if snapshot.loaded_at.elapsed() < Duration::from_secs(1) {
            return None;
        }
    }

    let states = (|| {
        let connection = open(path).ok()?;
        let mut query = connection
            .prepare("SELECT composerId, isArchived FROM composerHeaders")
            .ok()?;
        let states = query
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
            })
            .ok()?
            .collect::<rusqlite::Result<HashMap<_, _>>>()
            .ok()?;
        Some(Arc::new(states))
    })();
    let current_version = database_version(path);
    if current_version == version {
        snapshots.insert(
            path.to_path_buf(),
            CursorDesktopArchiveSnapshot {
                version,
                loaded_at: Instant::now(),
                states: states.as_ref().map(Arc::clone),
            },
        );
    }
    states
}

pub(super) fn database_sources(path: &Path) -> Vec<PathBuf> {
    [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
    ]
    .into_iter()
    .collect()
}

pub(super) fn scan(home: &Path, out: &mut Vec<SessionRecord>, warnings: &mut Vec<String>) {
    let desktop = desktop_database(home);
    if desktop.is_file() {
        if let Err(error) = scan_database(&desktop, out) {
            warnings.push(error.to_string());
        }
    }
    for entry in WalkDir::new(home.join(".cursor/projects"))
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_type().is_file() && is_sdk(entry.path()) {
            if let Err(error) = scan_database(entry.path(), out) {
                warnings.push(error.to_string());
            }
        }
    }
}

pub(super) fn scan_database(path: &Path, out: &mut Vec<SessionRecord>) -> Result<()> {
    let connection = open(path)?;
    if is_desktop(path) {
        let mut query =
            connection.prepare("SELECT composerId, value, isArchived FROM composerHeaders")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?;
        for row in rows {
            let (id, value, archived) = row?;
            let header: Value = serde_json::from_str(&value)?;
            if header.get("isDraft").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let transcript = read_desktop(&connection, &id)?;
            if transcript.items.is_empty()
                && header
                    .get("name")
                    .and_then(Value::as_str)
                    .is_none_or(|name| name.trim().is_empty())
            {
                continue;
            }
            let project = header
                .pointer("/workspaceIdentifier/uri/fsPath")
                .and_then(Value::as_str)
                .or_else(|| {
                    header
                        .pointer("/trackedGitRepos/0/repoPath")
                        .and_then(Value::as_str)
                })
                .map(PathBuf::from);
            out.push(record(
                path,
                id,
                &header,
                project,
                archived,
                false,
                &transcript,
            ));
        }
    } else if is_sdk(path) {
        let mut query = connection.prepare(
            "SELECT agent_id, workspace_ref, status, name, created_at, updated_at FROM agents",
        )?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        for row in rows {
            let (id, workspace, status, name, created, updated) = row?;
            let transcript = read_sdk(path, &connection, &id)?;
            let header = json!({"name":name,"createdAt":created,"lastUpdatedAt":updated});
            out.push(record(
                path,
                id,
                &header,
                Some(PathBuf::from(workspace)),
                status == "ARCHIVED",
                true,
                &transcript,
            ));
        }
    } else {
        bail!("unsupported Cursor session store: {}", path.display());
    }
    Ok(())
}

fn timestamp(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => {
            chrono::DateTime::from_timestamp_millis(number.as_i64()?).map(|time| time.to_rfc3339())
        }
        _ => None,
    }
}

fn record(
    path: &Path,
    id: String,
    header: &Value,
    project: Option<PathBuf>,
    archived: bool,
    archive_supported: bool,
    transcript: &TranscriptScan,
) -> SessionRecord {
    let users = transcript
        .items
        .iter()
        .filter(|item| item.kind == "user")
        .collect::<Vec<_>>();
    let assistants = transcript
        .items
        .iter()
        .filter(|item| item.kind == "assistant")
        .collect::<Vec<_>>();
    SessionRecord {
        id,
        agent: AgentKind::Cursor,
        title: crate::sessions::clean_session_title(
            header
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
        project,
        repository: None,
        repository_url: None,
        logical_project_id: None,
        logical_project_name: None,
        path: path.to_path_buf(),
        archived: Some(archived),
        archive_supported,
        resumable: false,
        started_at: timestamp(header.get("createdAt")),
        updated_at: timestamp(header.get("lastUpdatedAt")),
        message_count: Some(users.len() + assistants.len()),
        turn_count: Some(users.len()),
        first_user_message: users.first().map(|item| item.body.clone()),
        last_user_message: users.last().map(|item| item.body.clone()),
        last_assistant_message: assistants.last().map(|item| item.body.clone()),
        model: header
            .pointer("/modelConfig/modelName")
            .and_then(Value::as_str)
            .map(str::to_owned),
        mode: header
            .get("unifiedMode")
            .and_then(Value::as_str)
            .map(str::to_owned),
        approval_mode: None,
        is_run_everything: None,
        parent_session_id: header
            .pointer("/subagentInfo/parentComposerId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        token_usage: None,
    }
}

pub(super) fn archive_state(path: &Path, id: &str) -> Option<bool> {
    if path
        .extension()
        .is_some_and(|extension| extension == "jsonl")
    {
        return archive_state(&desktop_database_for_export(path)?, id);
    }
    if !is_desktop(path) && !is_sdk(path) {
        return None;
    }
    if is_desktop(path) {
        if let Some(cached) = super::cursor_sessions::cached_desktop_archive_states(path) {
            return cached?.get(id).copied();
        }
        let Some(states) = desktop_archive_states(path) else {
            super::cursor_sessions::cache_desktop_archive_states(path, None);
            return None;
        };
        let state = states.get(id).copied();
        super::cursor_sessions::cache_desktop_archive_states(path, Some(states));
        return state;
    }
    let connection = open(path).ok()?;
    if is_sdk(path) {
        connection
            .query_row(
                "SELECT status='ARCHIVED' FROM agents WHERE agent_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
    } else {
        None
    }
}

pub(super) fn set_archived(session: &SessionRecord, archived: bool) -> Result<PathBuf> {
    if !is_sdk(&session.path) {
        bail!("native archiving is not supported for this Cursor session source");
    }
    let mut connection =
        Connection::open_with_flags(&session.path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let (status, active): (String, Option<String>) = transaction.query_row(
        "SELECT status, active_run_id FROM agents WHERE agent_id=?1",
        [&session.id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if (status == "ARCHIVED") == archived {
        return Ok(session.path.clone());
    }
    if status == "RUNNING" || active.is_some() {
        bail!("Cursor SDK agent is running; stop the run before archiving");
    }
    transaction.execute(
        "UPDATE agents SET status=?1, updated_at=?2 WHERE agent_id=?3",
        rusqlite::params![
            if archived { "ARCHIVED" } else { "IDLE" },
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            session.id
        ],
    )?;
    transaction.commit()?;
    Ok(session.path.clone())
}

pub(super) fn read(path: &Path, id: &str) -> Result<TranscriptScan> {
    if (is_desktop(path) || is_sdk(path)) && id.is_empty() {
        bail!("sessionId is required for this Cursor session store");
    }
    if is_cli_store(path) {
        let store_path = path.with_file_name("store.db");
        if !store_path.is_file() {
            return Ok(TranscriptScan {
                items: Vec::new(),
                warnings: Vec::new(),
            });
        }
        let store = open(&store_path)?;
        let raw: String =
            store.query_row("SELECT value FROM meta WHERE key='0'", [], |row| row.get(0))?;
        let meta = super::cursor_sessions::parse_cursor_store_value(&raw)
            .context("invalid Cursor store metadata")?;
        let Some(root) = meta
            .get("latestRootBlobId")
            .and_then(Value::as_str)
            .filter(|root| !root.is_empty())
        else {
            return Ok(TranscriptScan {
                items: Vec::new(),
                warnings: Vec::new(),
            });
        };
        let bytes = store.query_row("SELECT data FROM blobs WHERE id=?1", [root], |row| {
            row.get::<_, Vec<u8>>(0)
        })?;
        return read_state(&bytes, |blob_id| {
            Ok(store
                .query_row("SELECT data FROM blobs WHERE id=?1", [blob_id], |row| {
                    row.get(0)
                })
                .optional()?)
        });
    }
    let connection = open(path)?;
    if is_desktop(path) {
        read_desktop(&connection, id)
    } else if is_sdk(path) {
        read_sdk(path, &connection, id)
    } else {
        bail!("unsupported Cursor session store");
    }
}

fn read_desktop(connection: &Connection, id: &str) -> Result<TranscriptScan> {
    let raw: Option<Vec<u8>> = connection
        .query_row(
            "SELECT CAST(value AS BLOB) FROM cursorDiskKV WHERE key=?1",
            [format!("composerData:{id}")],
            |row| row.get(0),
        )
        .optional()?;
    let Some(raw) = raw else {
        return Ok(TranscriptScan {
            items: Vec::new(),
            warnings: Vec::new(),
        });
    };
    let composer: Value = serde_json::from_slice(&raw)?;
    let mut scan = TranscriptScan {
        items: Vec::new(),
        warnings: Vec::new(),
    };
    if let Some(state) = composer
        .get("conversationState")
        .and_then(Value::as_str)
        .and_then(|state| state.strip_prefix('~'))
    {
        let bytes = STANDARD.decode(state)?;
        if !bytes.is_empty() {
            return read_state(&bytes, |blob_id| {
                Ok(connection
                    .query_row(
                        "SELECT CAST(value AS BLOB) FROM cursorDiskKV WHERE key=?1",
                        [format!("agentKv:blob:{blob_id}")],
                        |row| row.get(0),
                    )
                    .optional()?)
            });
        }
    }
    if let Some(headers) = composer
        .get("fullConversationHeadersOnly")
        .and_then(Value::as_array)
    {
        for header in headers {
            let Some(bubble_id) = header.get("bubbleId").and_then(Value::as_str) else {
                continue;
            };
            let raw: Option<Vec<u8>> = connection
                .query_row(
                    "SELECT CAST(value AS BLOB) FROM cursorDiskKV WHERE key=?1",
                    [format!("bubbleId:{id}:{bubble_id}")],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(raw) = raw {
                let bubble: Value = serde_json::from_slice(&raw)?;
                let role = match bubble.get("type").and_then(Value::as_u64) {
                    Some(1) => "user",
                    Some(2) => "assistant",
                    _ => continue,
                };
                let message = json!({"role":role,"content":bubble.get("text")});
                collect_cursor_item_with_timestamp(
                    &message,
                    &mut scan.items,
                    timestamp(bubble.get("createdAt")),
                );
            } else {
                scan.warnings
                    .push(format!("Cursor message {bubble_id} is unavailable"));
            }
        }
    }
    Ok(scan)
}

fn sdk_store(path: &Path, id: &str) -> PathBuf {
    path.parent()
        .unwrap_or(path)
        .join("agents")
        .join(format!("agent-{:x}", Sha256::digest(id.as_bytes())))
        .join("store.db")
}

fn read_sdk(path: &Path, connection: &Connection, id: &str) -> Result<TranscriptScan> {
    let checkpoint: Option<String> = connection.query_row(
        "SELECT latest_checkpoint_ref_json FROM agents WHERE agent_id=?1",
        [id],
        |row| row.get(0),
    )?;
    let scan = TranscriptScan {
        items: Vec::new(),
        warnings: Vec::new(),
    };
    let Some(checkpoint) = checkpoint else {
        return Ok(scan);
    };
    let checkpoint: Value = serde_json::from_str(&checkpoint)?;
    let root = checkpoint
        .get("blobId")
        .and_then(Value::as_str)
        .context("Cursor SDK checkpoint has no root blob")?;
    let store = open(&sdk_store(path, id))?;
    let bytes: Vec<u8> = store.query_row("SELECT data FROM blobs WHERE id=?1", [root], |row| {
        row.get(0)
    })?;
    read_state(&bytes, |blob_id| {
        Ok(store
            .query_row("SELECT data FROM blobs WHERE id=?1", [blob_id], |row| {
                row.get(0)
            })
            .optional()?)
    })
}

fn read_state(
    bytes: &[u8],
    mut blob: impl FnMut(&str) -> Result<Option<Vec<u8>>>,
) -> Result<TranscriptScan> {
    let fields = proto_fields(bytes)?;
    let mut scan = TranscriptScan {
        items: Vec::new(),
        warnings: Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    for (_, archive) in fields.iter().filter(|(field, _)| matches!(field, 11 | 13)) {
        let id = hex(archive);
        if let Some(archive) = blob(&id)? {
            for message in prompt_blob_ids(&archive)? {
                if seen.insert(message.clone()) {
                    append_blob(blob(&message)?, &message, &mut scan);
                }
            }
        } else {
            scan.warnings
                .push(format!("Cursor history archive {id} is unavailable"));
        }
    }
    for message in prompt_blob_ids(bytes)? {
        if seen.insert(message.clone()) {
            append_blob(blob(&message)?, &message, &mut scan);
        }
    }
    for (_, turn_id) in fields.iter().filter(|(field, _)| *field == 8) {
        let id = hex(turn_id);
        let turn = blob(&id)?.with_context(|| format!("Cursor turn {id} is unavailable"))?;
        for (variant, turn) in proto_fields(&turn)? {
            if variant != 1 {
                scan.warnings
                    .push("Cursor native shell-turn details are unavailable".to_owned());
                continue;
            }
            for (field, value) in proto_fields(turn)? {
                if !matches!(field, 1 | 2) {
                    continue;
                }
                let id = hex(value);
                let message = blob(&id)?
                    .with_context(|| format!("Cursor turn message {id} is unavailable"))?;
                if field == 1 {
                    if let Some((_, text)) = proto_fields(&message)?
                        .into_iter()
                        .find(|(field, _)| *field == 1)
                    {
                        collect_cursor_item_with_timestamp(
                            &json!({"role":"user","content":std::str::from_utf8(text)?}),
                            &mut scan.items,
                            None,
                        );
                    }
                } else {
                    for (variant, message) in proto_fields(&message)? {
                        if variant == 1 {
                            if let Some((_, text)) = proto_fields(message)?
                                .into_iter()
                                .find(|(field, _)| *field == 1)
                            {
                                collect_cursor_item_with_timestamp(
                                    &json!({"role":"assistant","content":std::str::from_utf8(text)?}),
                                    &mut scan.items,
                                    None,
                                );
                            }
                        } else if variant == 2 {
                            scan.warnings
                                .push("Cursor native tool-step details are unavailable".to_owned());
                        }
                    }
                }
            }
        }
    }
    scan.warnings.sort();
    scan.warnings.dedup();
    Ok(scan)
}

fn hex(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn append_blob(raw: Option<Vec<u8>>, id: &str, scan: &mut TranscriptScan) {
    let Some(raw) = raw else {
        scan.warnings
            .push(format!("Cursor message {id} is unavailable"));
        return;
    };
    let mut message: Value = match serde_json::from_slice(&raw) {
        Ok(message) => message,
        Err(error) => {
            scan.warnings.push(format!("Cursor message {id}: {error}"));
            return;
        }
    };
    if let Some(parts) = message.get_mut("content").and_then(Value::as_array_mut) {
        for part in parts {
            match part.get("type").and_then(Value::as_str) {
                Some("tool-call") => {
                    *part = json!({"type":"tool_use","id":part.get("toolCallId"),"name":part.get("toolName"),"input":part.get("args")});
                }
                Some("tool-result") => {
                    *part = json!({"type":"tool_result","tool_use_id":part.get("toolCallId"),"content":part.get("result")});
                }
                _ => {}
            }
        }
    }
    if message.get("role").and_then(Value::as_str) == Some("tool") {
        message["role"] = json!("user");
    }
    let time = super::cursor::cursor_event_timestamp(&message);
    collect_cursor_item_with_timestamp(&message, &mut scan.items, time);
}

fn prompt_blob_ids(bytes: &[u8]) -> Result<Vec<String>> {
    Ok(proto_fields(bytes)?
        .into_iter()
        .filter(|(field, _)| *field == 1)
        .map(|(_, value)| hex(value))
        .collect())
}

fn proto_fields(bytes: &[u8]) -> Result<Vec<(u64, &[u8])>> {
    let mut remaining = bytes;
    let mut ids = Vec::new();
    while !remaining.is_empty() {
        let tag = varint(&mut remaining)?;
        match tag & 7 {
            0 => {
                varint(&mut remaining)?;
            }
            1 => {
                take(&mut remaining, 8)?;
            }
            2 => {
                let length = usize::try_from(varint(&mut remaining)?)?;
                let value = take(&mut remaining, length)?;
                ids.push((tag >> 3, value));
            }
            5 => {
                take(&mut remaining, 4)?;
            }
            _ => bail!("unsupported Cursor conversation protobuf field"),
        }
    }
    Ok(ids)
}

fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8]> {
    if length > bytes.len() {
        bail!("truncated Cursor conversation checkpoint");
    }
    let (value, rest) = bytes.split_at(length);
    *bytes = rest;
    Ok(value)
}

fn varint(bytes: &mut &[u8]) -> Result<u64> {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let byte = take(bytes, 1)?[0];
        value |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    bail!("invalid Cursor conversation protobuf integer")
}

#[cfg(test)]
#[path = "cursor_native_sessions_tests.rs"]
mod tests;
