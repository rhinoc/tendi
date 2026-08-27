use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{providers::agent_provider, skills::AgentKind};

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
    #[serde(rename = "callId", skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(skip)]
    pub started_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptScan {
    pub items: Vec<TranscriptItem>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptLocatorItem {
    pub index: usize,
    pub label: String,
    pub response: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptPage {
    pub items: Vec<TranscriptItem>,
    pub locator_items: Vec<TranscriptLocatorItem>,
    pub warnings: Vec<String>,
    pub next_cursor: Option<String>,
    pub done: bool,
    pub source_version: String,
    pub restart_required: bool,
    pub unchanged: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptLocatorPage {
    pub locator_items: Vec<TranscriptLocatorItem>,
    pub warnings: Vec<String>,
    pub source_version: String,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSearchScopes {
    pub user: bool,
    pub assistant: bool,
    pub system: bool,
    pub tool: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSearchHit {
    pub group_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSearchResult {
    pub hits: Vec<TranscriptSearchHit>,
    pub warnings: Vec<String>,
    pub source_version: String,
}

const TRANSCRIPT_PAGE_DEFAULT_LIMIT: usize = 160;
const TRANSCRIPT_PAGE_MAX_LIMIT: usize = 400;
const TRANSCRIPT_PAGE_MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
const TRANSCRIPT_PAGE_MAX_SOURCE_LINES: usize = 2_000;
const TRANSCRIPT_PAGE_MAX_LINE_BYTES: usize = 2 * 1024 * 1024;
const TRANSCRIPT_SEARCH_CACHE_MAX_ENTRIES: usize = 16;
const TRANSCRIPT_SEARCH_CACHE_MAX_BYTES: usize = 512 * 1024;
const TRANSCRIPT_CHUNK_CACHE_MAX_ENTRIES: usize = 256;
const TRANSCRIPT_CHUNK_CACHE_MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TranscriptChunkOffset {
    start: u64,
    end: u64,
}

#[derive(Debug, Clone)]
struct TranscriptOffsetIndex {
    chunks: Vec<TranscriptChunkOffset>,
    valid: bool,
}

impl TranscriptOffsetIndex {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            valid: true,
        }
    }

    fn record(&mut self, start: u64, end: u64) {
        let contiguous = self
            .chunks
            .last()
            .map_or(start == 0, |chunk| chunk.end == start);
        if !contiguous || end < start {
            self.valid = false;
            return;
        }
        self.chunks.push(TranscriptChunkOffset { start, end });
    }

    fn is_complete(&self, source_size: u64) -> bool {
        if !self.valid {
            return false;
        }
        if source_size == 0 {
            return self.chunks == [TranscriptChunkOffset { start: 0, end: 0 }];
        }
        self.chunks.first().is_some_and(|chunk| chunk.start == 0)
            && self
                .chunks
                .last()
                .is_some_and(|chunk| chunk.end == source_size)
            && self
                .chunks
                .windows(2)
                .all(|chunks| chunks[0].end == chunks[1].start)
    }
}

#[derive(Debug, Clone)]
struct CachedTranscriptSearch {
    path: PathBuf,
    agent: AgentKind,
    query: String,
    scopes: TranscriptSearchScopes,
    source_version: String,
    offset_index: TranscriptOffsetIndex,
    result: TranscriptSearchResult,
    weight: usize,
}

#[derive(Debug, Default)]
struct TranscriptSearchCache {
    entries: VecDeque<CachedTranscriptSearch>,
    bytes: usize,
}

static TRANSCRIPT_SEARCH_CACHE: LazyLock<Mutex<TranscriptSearchCache>> =
    LazyLock::new(|| Mutex::new(TranscriptSearchCache::default()));

#[derive(Debug, Clone)]
struct CachedTranscriptChunk {
    path: PathBuf,
    agent: AgentKind,
    source_version: String,
    start: u64,
    end: u64,
    next_cursor: Option<String>,
    done: bool,
    items: Vec<TranscriptItem>,
    warnings: Vec<String>,
    weight: usize,
    #[cfg(test)]
    hits: usize,
}

#[derive(Debug, Default)]
struct TranscriptChunkCache {
    entries: VecDeque<CachedTranscriptChunk>,
    bytes: usize,
}

static TRANSCRIPT_CHUNK_CACHE: LazyLock<Mutex<TranscriptChunkCache>> =
    LazyLock::new(|| Mutex::new(TranscriptChunkCache::default()));

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct TranscriptCursor {
    offset: u64,
    line: usize,
    source: TranscriptSourceIdentity,
    boundary_hash: u64,
    source_size: u64,
    source_modified_ns: u128,
}

impl TranscriptCursor {
    fn parse(value: &str) -> Result<Self> {
        let encoded = value
            .strip_prefix("v1-")
            .with_context(|| "unsupported transcript cursor")?;
        if encoded.len() > 2_048 {
            anyhow::bail!("transcript cursor is too large");
        }
        let bytes = decode_hex(encoded).with_context(|| "invalid transcript cursor")?;
        let cursor: Self =
            serde_json::from_slice(&bytes).with_context(|| "invalid transcript cursor")?;
        if cursor.source.prefix_len > 4 * 1024 || cursor.line > 1_000_000_000 {
            anyhow::bail!("transcript cursor fields exceed their bounds");
        }
        Ok(cursor)
    }

    fn encode(&self) -> Result<String> {
        Ok(format!("v1-{}", encode_hex(&serde_json::to_vec(self)?)))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct TranscriptSourceIdentity {
    device: u64,
    inode: u64,
    prefix_len: usize,
    prefix_hash: u64,
}

#[derive(Debug, Clone)]
struct TranscriptSourceSnapshot {
    identity: TranscriptSourceIdentity,
    size: u64,
    modified_ns: u128,
}

impl TranscriptSourceSnapshot {
    fn version(&self) -> String {
        format!(
            "v1-{}-{}-{:016x}-{}-{}",
            self.identity.device,
            self.identity.inode,
            self.identity.prefix_hash,
            self.size,
            self.modified_ns,
        )
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        anyhow::bail!("odd cursor encoding length");
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char)
                .to_digit(16)
                .with_context(|| "invalid cursor encoding")?;
            let low = (pair[1] as char)
                .to_digit(16)
                .with_context(|| "invalid cursor encoding")?;
            Ok(((high << 4) | low) as u8)
        })
        .collect()
}

fn transcript_source_snapshot(
    file: &mut fs::File,
    prefix_len: Option<usize>,
) -> Result<TranscriptSourceSnapshot> {
    let metadata = file.metadata()?;
    let size = metadata.len();
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let (device, inode) = transcript_file_identity(&metadata);
    file.seek(SeekFrom::Start(0))?;
    let prefix_len = prefix_len
        .unwrap_or_else(|| usize::try_from(size.min(4 * 1024)).unwrap_or_default())
        .min(usize::try_from(size).unwrap_or(usize::MAX));
    let mut prefix = vec![0u8; prefix_len];
    file.read_exact(&mut prefix)?;
    file.seek(SeekFrom::Start(0))?;
    let prefix_hash = prefix.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    Ok(TranscriptSourceSnapshot {
        identity: TranscriptSourceIdentity {
            device,
            inode,
            prefix_len,
            prefix_hash,
        },
        size,
        modified_ns,
    })
}

fn transcript_boundary_hash(path: &Path, offset: u64) -> Result<u64> {
    if offset == 0 {
        return Ok(0xcbf29ce484222325);
    }
    let mut file = fs::File::open(path)?;
    let start = offset.saturating_sub(4 * 1024);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0u8; usize::try_from(offset - start).unwrap_or_default()];
    file.read_exact(&mut bytes)?;
    Ok(bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    }))
}

#[cfg(unix)]
fn transcript_file_identity(metadata: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn transcript_file_identity(_metadata: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

pub fn parse_transcript_page(
    path: &Path,
    agent: AgentKind,
    cursor: Option<&str>,
    limit: Option<usize>,
) -> Result<TranscriptPage> {
    parse_transcript_page_with_known_source_version_options(
        path, agent, cursor, limit, None, None, false, true,
    )
}

pub fn parse_transcript_page_if_changed(
    path: &Path,
    agent: AgentKind,
    cursor: Option<&str>,
    limit: Option<usize>,
    known_source_version: Option<&str>,
) -> Result<TranscriptPage> {
    parse_transcript_page_with_known_source_version_options(
        path,
        agent,
        cursor,
        limit,
        None,
        known_source_version,
        false,
        cursor.is_some(),
    )
}

pub fn parse_transcript_locator_page(
    path: &Path,
    agent: AgentKind,
) -> Result<TranscriptLocatorPage> {
    let mut locator_builder = TranscriptLocatorBuilder::default();
    let warnings = for_each_transcript_item(path, agent, |item| {
        locator_builder.push(&item);
    })?;
    Ok(TranscriptLocatorPage {
        locator_items: locator_builder.finish(),
        warnings,
        source_version: transcript_source_version(path)?,
    })
}

pub fn transcript_source_version(path: &Path) -> Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(transcript_source_snapshot(&mut file, None)?.version())
}

#[cfg(test)]
fn parse_transcript_page_with_store(
    path: &Path,
    agent: AgentKind,
    cursor: Option<&str>,
    limit: Option<usize>,
    cursor_store: Option<&Path>,
) -> Result<TranscriptPage> {
    parse_transcript_page_with_known_source_version_options(
        path,
        agent,
        cursor,
        limit,
        cursor_store,
        None,
        false,
        true,
    )
}

fn parse_transcript_page_with_known_source_version_options(
    path: &Path,
    agent: AgentKind,
    cursor: Option<&str>,
    limit: Option<usize>,
    cursor_store: Option<&Path>,
    known_source_version: Option<&str>,
    include_locator: bool,
    include_metadata: bool,
) -> Result<TranscriptPage> {
    parse_transcript_page_with_snapshot(
        path,
        agent,
        cursor,
        limit,
        cursor_store,
        known_source_version,
        None,
        include_locator,
        include_metadata,
    )
}

fn parse_transcript_page_at_snapshot(
    path: &Path,
    agent: AgentKind,
    cursor: Option<&str>,
    limit: Option<usize>,
    snapshot: &TranscriptSourceSnapshot,
) -> Result<TranscriptPage> {
    parse_transcript_page_with_snapshot(
        path,
        agent,
        cursor,
        limit,
        None,
        None,
        Some(snapshot),
        false,
        false,
    )
}

fn parse_transcript_page_with_snapshot(
    path: &Path,
    agent: AgentKind,
    cursor: Option<&str>,
    limit: Option<usize>,
    cursor_store: Option<&Path>,
    known_source_version: Option<&str>,
    search_snapshot: Option<&TranscriptSourceSnapshot>,
    include_locator: bool,
    include_metadata: bool,
) -> Result<TranscriptPage> {
    let cursor = cursor.map(TranscriptCursor::parse).transpose()?;
    let mut file =
        fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut inherited_history_start_ordinal = if cursor.is_some() {
        transcript_inherited_history_start_ordinal(path, agent)?
    } else {
        None
    };
    let source = transcript_source_snapshot(
        &mut file,
        search_snapshot
            .map(|snapshot| snapshot.identity.prefix_len)
            .or_else(|| cursor.as_ref().map(|cursor| cursor.source.prefix_len)),
    )
    .with_context(|| format!("failed to inspect {}", path.display()))?;
    let source_version =
        search_snapshot.map_or_else(|| source.version(), TranscriptSourceSnapshot::version);
    let locator_requested = include_locator
        && cursor.is_none()
        && known_source_version != Some(source_version.as_str());
    if cursor.is_none() && known_source_version == Some(source_version.as_str()) {
        return Ok(TranscriptPage {
            items: Vec::new(),
            locator_items: Vec::new(),
            warnings: Vec::new(),
            next_cursor: None,
            done: true,
            source_version,
            restart_required: false,
            unchanged: true,
        });
    }
    let cursor_stale = if let Some(cursor) = cursor.as_ref() {
        if let Some(snapshot) = search_snapshot {
            source.identity != snapshot.identity
                || source.size < snapshot.size
                || cursor.source != snapshot.identity
                || cursor.offset > snapshot.size
                || cursor.source_size != snapshot.size
                || transcript_boundary_hash(path, cursor.offset)? != cursor.boundary_hash
        } else {
            cursor.source != source.identity
                || cursor.offset > source.size
                || cursor.source_size != source.size
                || cursor.source_modified_ns != source.modified_ns
                || transcript_boundary_hash(path, cursor.offset)? != cursor.boundary_hash
        }
    } else {
        search_snapshot.is_some_and(|snapshot| {
            source.identity != snapshot.identity || source.size < snapshot.size
        })
    };
    if cursor_stale {
        return Ok(TranscriptPage {
            items: Vec::new(),
            locator_items: Vec::new(),
            warnings: vec!["transcript source changed; restart from the first page".to_string()],
            next_cursor: None,
            done: false,
            source_version,
            restart_required: true,
            unchanged: false,
        });
    }
    let cursor_source = search_snapshot.unwrap_or(&source);
    let cursor = match cursor {
        Some(cursor) => cursor,
        None => TranscriptCursor {
            offset: 0,
            line: 0,
            source: cursor_source.identity.clone(),
            boundary_hash: transcript_boundary_hash(path, 0)?,
            source_size: cursor_source.size,
            source_modified_ns: cursor_source.modified_ns,
        },
    };

    let limit = limit
        .unwrap_or(TRANSCRIPT_PAGE_DEFAULT_LIMIT)
        .clamp(1, TRANSCRIPT_PAGE_MAX_LIMIT);
    let mut reader = BufReader::new(file);
    reader
        .seek(SeekFrom::Start(cursor.offset))
        .with_context(|| format!("failed to seek {}", path.display()))?;
    let mut items = Vec::new();
    let mut warnings = Vec::new();
    let mut line_number = cursor.line;
    let mut source_bytes = 0u64;
    let mut snapshot_remaining =
        search_snapshot.map(|snapshot| snapshot.size.saturating_sub(cursor.offset));
    let mut page_complete = false;
    let mut page_offset = None;
    let mut page_line = None;
    let mut locator_builder = locator_requested.then(TranscriptLocatorBuilder::default);

    while !page_complete || locator_requested {
        if !locator_requested
            && (line_number.saturating_sub(cursor.line) >= TRANSCRIPT_PAGE_MAX_SOURCE_LINES
                || source_bytes >= TRANSCRIPT_PAGE_MAX_SOURCE_BYTES)
        {
            break;
        }
        let Some(line) = read_bounded_jsonl_line(
            &mut reader,
            TRANSCRIPT_PAGE_MAX_LINE_BYTES,
            snapshot_remaining,
        )?
        else {
            break;
        };
        line_number += 1;
        source_bytes = source_bytes.saturating_add(line.consumed);
        if let Some(remaining) = snapshot_remaining.as_mut() {
            *remaining = remaining.saturating_sub(line.consumed);
        }
        if !line.complete {
            break;
        }
        if line.truncated {
            warnings.push(format!(
                "{}:{} exceeds {} bytes and was skipped",
                path.display(),
                line_number,
                TRANSCRIPT_PAGE_MAX_LINE_BYTES,
            ));
            continue;
        }
        let line = match String::from_utf8(line.bytes) {
            Ok(line) => line,
            Err(err) => {
                warnings.push(format!("{}:{}: {err}", path.display(), line_number));
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let value = match serde_json::from_str::<Value>(&line) {
            Ok(value) => value,
            Err(err) => {
                warnings.push(format!("{}:{}: {err}", path.display(), line_number));
                continue;
            }
        };
        if inherited_history_start_ordinal.is_none() {
            inherited_history_start_ordinal =
                agent_provider(agent).transcript_inherited_history_start_ordinal(&value);
        }
        if is_inherited_transcript_value(&value, inherited_history_start_ordinal) {
            continue;
        }
        if !page_complete {
            let item_start = items.len();
            collect_transcript_value(&value, agent, &mut items);
            if let Some(locator_builder) = locator_builder.as_mut() {
                for item in &items[item_start..] {
                    locator_builder.push(item);
                }
            }
            if items.len() >= limit {
                page_complete = true;
                page_offset = Some(reader.stream_position()?);
                page_line = Some(line_number);
            }
        } else if let Some(locator_builder) = locator_builder.as_mut() {
            let mut parsed_items = Vec::new();
            collect_transcript_value(&value, agent, &mut parsed_items);
            for item in &parsed_items {
                locator_builder.push(item);
            }
        }
    }

    let end_offset = reader.stream_position()?;
    let offset = page_offset.unwrap_or(end_offset);
    let done = if search_snapshot.is_some() || locator_requested {
        offset >= cursor_source.size
    } else {
        reader.fill_buf()?.is_empty()
    };
    if done && include_metadata && !agent_provider(agent).transcript_cacheable() {
        if let Some(store_path) = cursor_store
            .map(Path::to_path_buf)
            .or_else(|| agent_provider(agent).transcript_metadata_store_path(path))
        {
            agent_provider(agent).append_transcript_metadata_from_store(&store_path, &mut items)?;
        }
    }
    let next_cursor = if done {
        None
    } else {
        Some(
            TranscriptCursor {
                offset,
                line: page_line.unwrap_or(line_number),
                source: cursor_source.identity.clone(),
                boundary_hash: transcript_boundary_hash(path, offset)?,
                source_size: cursor_source.size,
                source_modified_ns: cursor_source.modified_ns,
            }
            .encode()?,
        )
    };
    Ok(TranscriptPage {
        items,
        locator_items: locator_builder
            .map(TranscriptLocatorBuilder::finish)
            .unwrap_or_default(),
        warnings,
        next_cursor,
        done,
        source_version,
        restart_required: false,
        unchanged: false,
    })
}

struct BoundedJsonlLine {
    bytes: Vec<u8>,
    consumed: u64,
    truncated: bool,
    complete: bool,
}

fn read_bounded_jsonl_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
    source_remaining: Option<u64>,
) -> std::io::Result<Option<BoundedJsonlLine>> {
    let mut bytes = Vec::with_capacity(max_bytes.min(16 * 1024));
    let mut consumed = 0u64;
    let mut truncated = false;
    let mut saw_data = false;
    let mut source_remaining = source_remaining;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(saw_data.then_some(BoundedJsonlLine {
                bytes,
                consumed,
                truncated,
                complete: true,
            }));
        }
        saw_data = true;
        let visible_len = source_remaining.map_or(available.len(), |remaining| {
            available
                .len()
                .min(usize::try_from(remaining).unwrap_or(usize::MAX))
        });
        if visible_len == 0 {
            return Ok(Some(BoundedJsonlLine {
                bytes,
                consumed,
                truncated,
                complete: false,
            }));
        }
        let visible = &available[..visible_len];
        let chunk_len = visible
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(visible_len, |index| index + 1);
        let line_complete = visible.get(chunk_len.saturating_sub(1)) == Some(&b'\n');
        let source_has_more_bytes = visible_len < available.len();
        let remaining = max_bytes.saturating_sub(bytes.len());
        let copy_len = remaining.min(chunk_len);
        bytes.extend_from_slice(&visible[..copy_len]);
        if copy_len < chunk_len {
            truncated = true;
        }
        reader.consume(chunk_len);
        consumed = consumed.saturating_add(chunk_len as u64);
        let source_exhausted = source_remaining
            .is_some_and(|remaining| u64::try_from(chunk_len).unwrap_or(u64::MAX) >= remaining);
        if let Some(remaining) = source_remaining.as_mut() {
            *remaining = remaining.saturating_sub(chunk_len as u64);
        }
        if line_complete {
            return Ok(Some(BoundedJsonlLine {
                bytes,
                consumed,
                truncated,
                complete: true,
            }));
        }
        if source_exhausted && source_has_more_bytes {
            return Ok(Some(BoundedJsonlLine {
                bytes,
                consumed,
                truncated,
                complete: false,
            }));
        }
    }
}

fn collect_transcript_value(value: &Value, agent: AgentKind, items: &mut Vec<TranscriptItem>) {
    agent_provider(agent).parse_transcript_value(value, items);
}

pub(crate) fn transcript_inherited_history_start_ordinal(
    path: &Path,
    agent: AgentKind,
) -> Result<Option<u64>> {
    let file =
        fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    for line in BufReader::new(file).lines().take(64) {
        let Ok(line) = line else {
            break;
        };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(start_ordinal) =
            agent_provider(agent).transcript_inherited_history_start_ordinal(&value)
        {
            return Ok(Some(start_ordinal));
        }
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            break;
        }
    }
    Ok(None)
}

pub(crate) fn is_inherited_transcript_value(value: &Value, start_ordinal: Option<u64>) -> bool {
    start_ordinal.is_some_and(|start_ordinal| {
        value
            .get("ordinal")
            .and_then(Value::as_u64)
            .is_some_and(|ordinal| ordinal < start_ordinal)
    })
}

pub fn parse_transcript(path: &Path, agent: AgentKind) -> Result<TranscriptScan> {
    let file =
        fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let inherited_history_start_ordinal = transcript_inherited_history_start_ordinal(path, agent)?;
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

        if is_inherited_transcript_value(&value, inherited_history_start_ordinal) {
            continue;
        }
        collect_transcript_value(&value, agent, &mut items);
    }

    agent_provider(agent).append_transcript_metadata(path, &mut items)?;

    Ok(TranscriptScan { items, warnings })
}

#[derive(Default)]
struct TranscriptLocatorBuilder {
    items: Vec<TranscriptLocatorItem>,
    pending_response: Option<usize>,
    grouped_index: usize,
    previous_was_tool: bool,
}

impl TranscriptLocatorBuilder {
    fn push(&mut self, item: &TranscriptItem) {
        let kind = item.kind.as_str();
        let item_index = if kind == "tool" && self.previous_was_tool {
            self.grouped_index.saturating_sub(1)
        } else {
            let index = self.grouped_index;
            self.grouped_index += 1;
            index
        };

        if kind == "user" {
            self.items.push(TranscriptLocatorItem {
                index: item_index,
                label: item.body.trim().to_string(),
                response: String::new(),
            });
            self.pending_response = Some(self.items.len() - 1);
        } else if kind == "assistant" {
            if let Some(locator_index) = self.pending_response.take() {
                self.items[locator_index].response = item.body.trim().to_string();
            }
        }
        self.previous_was_tool = kind == "tool";
    }

    fn finish(self) -> Vec<TranscriptLocatorItem> {
        self.items
    }
}

fn for_each_transcript_item<F>(path: &Path, agent: AgentKind, mut visit: F) -> Result<Vec<String>>
where
    F: FnMut(TranscriptItem),
{
    let file =
        fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let inherited_history_start_ordinal = transcript_inherited_history_start_ordinal(path, agent)?;
    let mut warnings = Vec::new();
    let mut items = Vec::new();

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
        if is_inherited_transcript_value(&value, inherited_history_start_ordinal) {
            continue;
        }
        let item_start = items.len();
        collect_transcript_value(&value, agent, &mut items);
        for item in items[item_start..].iter().cloned() {
            visit(item);
        }
        items.retain(|item| item.kind == "tool" && item.result.is_none());
    }

    Ok(warnings)
}

#[cfg(test)]
pub(crate) fn parse_search_transcript(path: &Path, agent: AgentKind) -> Result<TranscriptScan> {
    let mut items = Vec::new();
    let warnings = for_each_search_item(path, agent, |item| items.push(item))?;
    Ok(TranscriptScan { items, warnings })
}

pub(crate) fn for_each_search_item<F>(
    path: &Path,
    agent: AgentKind,
    mut visit: F,
) -> Result<Vec<String>>
where
    F: FnMut(TranscriptItem),
{
    let file =
        fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let inherited_history_start_ordinal = transcript_inherited_history_start_ordinal(path, agent)?;
    let mut warnings = Vec::new();

    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                warnings.push(format!("{}:{}: {err}", path.display(), index + 1));
                continue;
            }
        };
        if line.trim().is_empty() || !agent_provider(agent).transcript_search_hint(&line) {
            continue;
        }
        let value = match serde_json::from_str::<Value>(&line) {
            Ok(value) => value,
            Err(err) => {
                warnings.push(format!("{}:{}: {err}", path.display(), index + 1));
                continue;
            }
        };

        if is_inherited_transcript_value(&value, inherited_history_start_ordinal) {
            continue;
        }

        let mut items = Vec::new();
        collect_transcript_value(&value, agent, &mut items);
        for item in items {
            if matches!(item.kind.as_str(), "user" | "assistant") {
                visit(item);
            }
        }
    }
    Ok(warnings)
}

fn transcript_cursor_offset(cursor: Option<&str>) -> Result<u64> {
    Ok(cursor
        .map(TranscriptCursor::parse)
        .transpose()?
        .map_or(0, |cursor| cursor.offset))
}

fn transcript_item_weight(item: &TranscriptItem) -> usize {
    item.kind
        .len()
        .saturating_add(item.body.len())
        .saturating_add(item.tag.as_deref().map_or(0, str::len))
        .saturating_add(item.command.as_deref().map_or(0, str::len))
        .saturating_add(item.result.as_deref().map_or(0, str::len))
        .saturating_add(item.time.as_deref().map_or(0, str::len))
        .saturating_add(item.linked_session_id.as_deref().map_or(0, str::len))
        .saturating_add(item.model.as_deref().map_or(0, str::len))
        .saturating_add(item.effort.as_deref().map_or(0, str::len))
        .saturating_add(item.call_id.as_deref().map_or(0, str::len))
        .saturating_add(std::mem::size_of::<TranscriptItem>())
}

fn transcript_chunk_weight(items: &[TranscriptItem], warnings: &[String]) -> usize {
    items
        .iter()
        .map(transcript_item_weight)
        .chain(warnings.iter().map(String::len))
        .sum()
}

fn transcript_chunk_cache_get(
    path: &Path,
    agent: AgentKind,
    source_version: &str,
    start: u64,
    source_size: u64,
) -> Option<CachedTranscriptChunk> {
    let mut cache = TRANSCRIPT_CHUNK_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = cache.entries.iter().position(|entry| {
        entry.path == path
            && entry.agent == agent
            && entry.source_version == source_version
            && entry.start == start
            && entry.end <= source_size
    })?;
    let entry = cache.entries.remove(index)?;
    #[cfg(test)]
    let entry = {
        let mut entry = entry;
        entry.hits = entry.hits.saturating_add(1);
        entry
    };
    cache.entries.push_front(entry.clone());
    Some(entry)
}

fn transcript_chunk_cache_put(
    path: &Path,
    agent: AgentKind,
    source_version: &str,
    source_size: u64,
    start: u64,
    end: u64,
    next_cursor: Option<String>,
    done: bool,
    items: Vec<TranscriptItem>,
    warnings: Vec<String>,
) {
    if end < start || end > source_size {
        return;
    }
    let weight = transcript_chunk_weight(&items, &warnings);
    if weight > TRANSCRIPT_CHUNK_CACHE_MAX_BYTES {
        return;
    }

    let mut cache = TRANSCRIPT_CHUNK_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let mut retained = VecDeque::with_capacity(cache.entries.len());
    while let Some(entry) = cache.entries.pop_front() {
        if entry.path == path && entry.agent == agent && entry.source_version != source_version {
            cache.bytes = cache.bytes.saturating_sub(entry.weight);
        } else {
            retained.push_back(entry);
        }
    }
    cache.entries = retained;

    if let Some(index) = cache.entries.iter().position(|entry| {
        entry.path == path
            && entry.agent == agent
            && entry.source_version == source_version
            && entry.start == start
    }) {
        if let Some(entry) = cache.entries.remove(index) {
            cache.bytes = cache.bytes.saturating_sub(entry.weight);
        }
    }
    while cache.entries.len() >= TRANSCRIPT_CHUNK_CACHE_MAX_ENTRIES
        || cache.bytes.saturating_add(weight) > TRANSCRIPT_CHUNK_CACHE_MAX_BYTES
    {
        let Some(entry) = cache.entries.pop_back() else {
            break;
        };
        cache.bytes = cache.bytes.saturating_sub(entry.weight);
    }

    cache.bytes = cache.bytes.saturating_add(weight);
    cache.entries.push_front(CachedTranscriptChunk {
        path: path.to_path_buf(),
        agent,
        source_version: source_version.to_string(),
        start,
        end,
        next_cursor,
        done,
        items,
        warnings,
        weight,
        #[cfg(test)]
        hits: 0,
    });
}

fn transcript_search_cache_get(
    path: &Path,
    agent: AgentKind,
    query: &str,
    scopes: &TranscriptSearchScopes,
    source_version: &str,
) -> Option<TranscriptSearchResult> {
    let source_size = fs::metadata(path).ok()?.len();
    let mut cache = TRANSCRIPT_SEARCH_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = cache.entries.iter().position(|entry| {
        entry.path == path
            && entry.agent == agent
            && entry.query == query
            && entry.scopes == *scopes
            && entry.source_version == source_version
    })?;
    let entry = cache.entries.remove(index)?;
    if !entry.offset_index.is_complete(source_size) {
        cache.bytes = cache.bytes.saturating_sub(entry.weight);
        return None;
    }
    let result = entry.result.clone();
    cache.entries.push_front(entry);
    Some(result)
}

fn transcript_search_result_weight(
    query: &str,
    result: &TranscriptSearchResult,
    offset_index: &TranscriptOffsetIndex,
) -> usize {
    query
        .len()
        .saturating_add(result.hits.len().saturating_mul(24))
        .saturating_add(result.warnings.iter().map(String::len).sum::<usize>())
        .saturating_add(offset_index.chunks.len().saturating_mul(16))
}

fn transcript_search_cache_put(
    path: &Path,
    agent: AgentKind,
    query: &str,
    scopes: &TranscriptSearchScopes,
    source_version: &str,
    source_size: u64,
    offset_index: TranscriptOffsetIndex,
    result: TranscriptSearchResult,
) {
    if !offset_index.is_complete(source_size) {
        return;
    }
    let weight = transcript_search_result_weight(query, &result, &offset_index);
    if weight > TRANSCRIPT_SEARCH_CACHE_MAX_BYTES {
        return;
    }

    let mut cache = TRANSCRIPT_SEARCH_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(index) = cache.entries.iter().position(|entry| {
        entry.path == path
            && entry.agent == agent
            && entry.query == query
            && entry.scopes == *scopes
    }) {
        if let Some(entry) = cache.entries.remove(index) {
            cache.bytes = cache.bytes.saturating_sub(entry.weight);
        }
    }
    while cache.entries.len() >= TRANSCRIPT_SEARCH_CACHE_MAX_ENTRIES
        || cache.bytes.saturating_add(weight) > TRANSCRIPT_SEARCH_CACHE_MAX_BYTES
    {
        let Some(entry) = cache.entries.pop_back() else {
            break;
        };
        cache.bytes = cache.bytes.saturating_sub(entry.weight);
    }
    cache.bytes = cache.bytes.saturating_add(weight);
    cache.entries.push_front(CachedTranscriptSearch {
        path: path.to_path_buf(),
        agent,
        query: query.to_string(),
        scopes: scopes.clone(),
        source_version: source_version.to_string(),
        offset_index,
        result,
        weight,
    });
}

#[cfg(test)]
fn transcript_search_cache_offsets(
    path: &Path,
    agent: AgentKind,
    query: &str,
    scopes: &TranscriptSearchScopes,
    source_version: &str,
) -> Option<Vec<(u64, u64)>> {
    let cache = TRANSCRIPT_SEARCH_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache
        .entries
        .iter()
        .find(|entry| {
            entry.path == path
                && entry.agent == agent
                && entry.query == query
                && entry.scopes == *scopes
                && entry.source_version == source_version
        })
        .map(|entry| {
            entry
                .offset_index
                .chunks
                .iter()
                .map(|chunk| (chunk.start, chunk.end))
                .collect()
        })
}

#[cfg(test)]
fn transcript_chunk_cache_offsets(
    path: &Path,
    agent: AgentKind,
    source_version: &str,
) -> Vec<(u64, u64)> {
    let cache = TRANSCRIPT_CHUNK_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache
        .entries
        .iter()
        .filter(|entry| {
            entry.path == path && entry.agent == agent && entry.source_version == source_version
        })
        .map(|entry| (entry.start, entry.end))
        .collect()
}

#[cfg(test)]
fn transcript_chunk_cache_hits(path: &Path, agent: AgentKind, source_version: &str) -> usize {
    let cache = TRANSCRIPT_CHUNK_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache
        .entries
        .iter()
        .filter(|entry| {
            entry.path == path && entry.agent == agent && entry.source_version == source_version
        })
        .map(|entry| entry.hits)
        .sum()
}

pub fn search_transcript(
    path: &Path,
    agent: AgentKind,
    query: &str,
    scopes: &TranscriptSearchScopes,
) -> Result<TranscriptSearchResult> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(TranscriptSearchResult {
            hits: Vec::new(),
            warnings: Vec::new(),
            source_version: transcript_source_version(path)?,
        });
    }

    let initial_source = transcript_search_snapshot(path)?;
    let initial_source_version = initial_source.version();
    let source_size = initial_source.size;
    let initial_boundary_hash = transcript_boundary_hash(path, source_size)?;
    let cache_source_version = agent_provider(agent)
        .transcript_cacheable()
        .then_some(initial_source_version.as_str());
    if let Some(source_version) = cache_source_version
        && let Some(result) =
            transcript_search_cache_get(path, agent, &needle, scopes, source_version)
    {
        return Ok(result);
    }

    let mut cursor = None;
    let mut group_index = 0usize;
    let mut previous_tool_group = None;
    let mut next_tool_index = 0usize;
    let mut tool_groups = HashMap::<String, (usize, usize)>::new();
    let mut hits = Vec::new();
    let mut warnings = Vec::new();
    let mut offset_index = TranscriptOffsetIndex::new();
    let source_version = loop {
        let chunk_start = transcript_cursor_offset(cursor.as_deref())?;
        let cached_chunk = if !agent_provider(agent).transcript_cacheable() {
            None
        } else {
            transcript_chunk_cache_get(
                path,
                agent,
                &initial_source_version,
                chunk_start,
                source_size,
            )
        };
        let (items, page_warnings, page_source_version, page_done, page_next_cursor, chunk_end) =
            if let Some(chunk) = cached_chunk {
                (
                    chunk.items,
                    chunk.warnings,
                    chunk.source_version,
                    chunk.done,
                    chunk.next_cursor,
                    chunk.end,
                )
            } else {
                let page = parse_transcript_page_at_snapshot(
                    path,
                    agent,
                    cursor.as_deref(),
                    Some(TRANSCRIPT_PAGE_DEFAULT_LIMIT),
                    &initial_source,
                )?;
                if page.restart_required {
                    anyhow::bail!("transcript source changed during search")
                }
                let page_source_version = page.source_version.clone();
                if page_source_version != initial_source_version {
                    anyhow::bail!("transcript source changed during search")
                }
                let chunk_end = match page.next_cursor.as_deref() {
                    Some(next_cursor) => transcript_cursor_offset(Some(next_cursor))?,
                    None => source_size,
                };
                if agent_provider(agent).transcript_cacheable() {
                    transcript_chunk_cache_put(
                        path,
                        agent,
                        &initial_source_version,
                        source_size,
                        chunk_start,
                        chunk_end,
                        page.next_cursor.clone(),
                        page.done,
                        page.items.clone(),
                        page.warnings.clone(),
                    );
                }
                (
                    page.items,
                    page.warnings,
                    page_source_version,
                    page.done,
                    page.next_cursor,
                    chunk_end,
                )
            };
        if page_source_version != initial_source_version {
            anyhow::bail!("transcript source changed during search")
        }
        warnings.extend(page_warnings);
        offset_index.record(chunk_start, chunk_end);

        for item in &items {
            let kind = item.kind.as_str();
            let mapped_tool = if kind == "tool_result" {
                item.call_id
                    .as_ref()
                    .and_then(|call_id| tool_groups.get(call_id).copied())
            } else {
                None
            };
            let (item_group_index, tool_index, is_tool_scope) =
                if let Some((group, index)) = mapped_tool {
                    (group, Some(index), true)
                } else if kind == "tool" {
                    let group = previous_tool_group.unwrap_or_else(|| {
                        let current = group_index;
                        group_index += 1;
                        current
                    });
                    let index = if previous_tool_group == Some(group) {
                        next_tool_index
                    } else {
                        0
                    };
                    previous_tool_group = Some(group);
                    next_tool_index = index + 1;
                    if let Some(call_id) = item.call_id.as_ref() {
                        tool_groups.insert(call_id.clone(), (group, index));
                    }
                    (group, Some(index), true)
                } else {
                    previous_tool_group = None;
                    next_tool_index = 0;
                    let current = group_index;
                    group_index += 1;
                    (current, None, false)
                };

            if !scope_enabled(kind, is_tool_scope, scopes) {
                continue;
            }
            if transcript_item_contains(item, &needle) {
                hits.push(TranscriptSearchHit {
                    group_index: item_group_index,
                    tool_index,
                });
            }
        }

        if page_done {
            break page_source_version;
        }
        cursor = page_next_cursor;
        if cursor.is_none() {
            break page_source_version;
        }
    };

    hits.sort_by_key(|hit| (hit.group_index, hit.tool_index.unwrap_or(0)));
    if !transcript_search_snapshot_is_compatible(path, &initial_source, initial_boundary_hash)? {
        anyhow::bail!("transcript source changed during search")
    }
    let result = TranscriptSearchResult {
        hits,
        warnings,
        source_version,
    };
    if agent_provider(agent).transcript_cacheable() {
        transcript_search_cache_put(
            path,
            agent,
            &needle,
            scopes,
            &result.source_version,
            source_size,
            offset_index,
            result.clone(),
        );
    }
    Ok(result)
}

fn transcript_search_snapshot(path: &Path) -> Result<TranscriptSourceSnapshot> {
    let mut file =
        fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    transcript_source_snapshot(&mut file, None)
}

fn transcript_search_snapshot_is_compatible(
    path: &Path,
    snapshot: &TranscriptSourceSnapshot,
    boundary_hash: u64,
) -> Result<bool> {
    let mut file = fs::File::open(path)?;
    let current = transcript_source_snapshot(&mut file, Some(snapshot.identity.prefix_len))?;
    Ok(current.identity == snapshot.identity
        && current.size >= snapshot.size
        && (current.size > snapshot.size || current.modified_ns == snapshot.modified_ns)
        && transcript_boundary_hash(path, snapshot.size)? == boundary_hash)
}

fn scope_enabled(kind: &str, mapped_tool: bool, scopes: &TranscriptSearchScopes) -> bool {
    if mapped_tool || kind == "tool" || kind == "toolGroup" {
        return scopes.tool;
    }
    match kind {
        "user" | "notification" => scopes.user,
        "context" | "compaction" | "model_config" => scopes.system,
        _ => scopes.assistant,
    }
}

fn transcript_item_contains(item: &TranscriptItem, needle: &str) -> bool {
    [
        item.body.as_str(),
        item.tag.as_deref().unwrap_or_default(),
        item.command.as_deref().unwrap_or_default(),
        item.result.as_deref().unwrap_or_default(),
        item.time.as_deref().unwrap_or_default(),
    ]
    .into_iter()
    .any(|value| value.to_lowercase().contains(needle))
}

const SEARCH_MESSAGE_HINT_BYTES: usize = 16 * 1024;

pub(crate) fn search_json_hint(line: &str) -> &str {
    let mut end = line.len().min(SEARCH_MESSAGE_HINT_BYTES);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    &line[..end]
}

pub(crate) fn json_string_hint<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
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

pub(crate) fn collect_generic_item(value: &Value, items: &mut Vec<TranscriptItem>) {
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
    collect_message_content(content, items, kind, time.clone());

    if let Some(Value::Array(content_items)) = content {
        for item in content_items {
            if item.get("type").and_then(Value::as_str) == Some("tool_use") {
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                    .map(str::to_string);
                push_tool_item(
                    items,
                    "tool",
                    summarize_tool_call(item),
                    name,
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

pub(crate) fn extract_content_text(value: Option<&Value>) -> Option<String> {
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
        Value::Object(_) => value
            .get("text")
            .and_then(Value::as_str)
            .and_then(clean_body)
            .or_else(|| extract_content_text(value.get("content")))
            .or_else(|| value.get("message").and_then(Value::as_str).and_then(clean_body)),
        _ => None,
    }
}

pub(crate) fn extract_raw_content_text(value: Option<&Value>) -> Option<String> {
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

pub(crate) type InternalContextMarker = (&'static str, &'static str, Option<&'static str>);

pub(crate) fn collect_message_content(
    content: Option<&Value>,
    items: &mut Vec<TranscriptItem>,
    role: &str,
    time: Option<String>,
) {
    collect_message_content_with_markers(content, items, role, time, &[]);
}

pub(crate) fn collect_message_content_with_markers(
    content: Option<&Value>,
    items: &mut Vec<TranscriptItem>,
    role: &str,
    time: Option<String>,
    extra_markers: &[InternalContextMarker],
) {
    let Some(content) = content else {
        return;
    };
    let content_items = match content {
        Value::Array(content_items) => content_items.as_slice(),
        _ => std::slice::from_ref(content),
    };
    let mut pending_body = Vec::new();

    for content_item in content_items {
        if is_non_message_content_item(content_item) {
            continue;
        }
        let Some(raw_body) = extract_raw_content_text(Some(content_item)) else {
            continue;
        };
        for (label, segment) in
            split_internal_context_segments_with_markers(&raw_body, extra_markers)
        {
            if let Some(label) = label {
                push_message_body(&mut pending_body, items, role, time.clone());
                push_item(
                    items,
                    "context",
                    segment,
                    Some(label.to_string()),
                    time.clone(),
                );
            } else if let Some(body) = clean_body(&segment) {
                pending_body.push(body);
            }
        }
    }
    push_message_body(&mut pending_body, items, role, time);
}

fn push_message_body(
    pending_body: &mut Vec<String>,
    items: &mut Vec<TranscriptItem>,
    role: &str,
    time: Option<String>,
) {
    if pending_body.is_empty() {
        return;
    }
    let body = pending_body.join("\n");
    pending_body.clear();
    let item_kind = if role == "user" && is_subagent_notification(&body) {
        "notification"
    } else {
        role
    };
    let tag = (item_kind == "notification").then(|| "Subagent".to_string());
    push_item(items, item_kind, body, tag, time);
}

pub(crate) fn extract_thinking_text(value: Option<&Value>) -> Option<String> {
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

pub(crate) fn is_thinking_content_item(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some("thinking" | "reasoning" | "summary_text")
    )
}

pub(crate) fn is_non_message_content_item(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some(
            "tool_result"
                | "tool_use"
                | "function_call"
                | "function_call_output"
                | "custom_tool_call"
                | "custom_tool_call_output"
        )
    )
}

pub(crate) fn clean_body(text: &str) -> Option<String> {
    let text = split_internal_context_segments(text)
        .into_iter()
        .filter_map(|(label, segment)| label.is_none().then_some(segment))
        .collect::<Vec<_>>()
        .join("\n");
    let mut text = text.trim();
    if text.is_empty() {
        return None;
    }

    if let Some(inner) = extract_tag_body(text, "user_query") {
        text = inner;
    }

    Some(text.to_string())
}

pub(crate) fn split_internal_context_segments(text: &str) -> Vec<(Option<&'static str>, String)> {
    split_internal_context_segments_with_markers(text, &[])
}

pub(crate) fn split_internal_context_segments_with_markers(
    text: &str,
    extra_markers: &[InternalContextMarker],
) -> Vec<(Option<&'static str>, String)> {
    let mut segments = Vec::new();
    let mut cursor = 0;
    while let Some((start, label, prefix, closing)) =
        find_internal_context_marker(text, cursor, extra_markers)
    {
        if start > cursor {
            segments.push((None, text[cursor..start].to_string()));
        }
        let block_end = closing
            .and_then(|closing| {
                text[start..]
                    .find(closing)
                    .map(|offset| start + offset + closing.len())
            })
            .or_else(|| {
                find_internal_context_marker(text, start + prefix.len(), extra_markers)
                    .map(|(next_start, _, _, _)| next_start)
            })
            .unwrap_or(text.len());
        if block_end <= start {
            break;
        }
        segments.push((Some(label), text[start..block_end].trim().to_string()));
        cursor = block_end;
    }
    if cursor < text.len() {
        segments.push((None, text[cursor..].to_string()));
    }
    if segments.is_empty() && !text.trim().is_empty() {
        segments.push((None, text.trim().to_string()));
    }
    segments
}

pub(crate) fn is_subagent_notification(text: &str) -> bool {
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

fn find_internal_context_marker(
    text: &str,
    offset: usize,
    extra_markers: &[InternalContextMarker],
) -> Option<(usize, &'static str, &'static str, Option<&'static str>)> {
    INTERNAL_CONTEXT_MARKERS
        .iter()
        .copied()
        .chain(extra_markers.iter().copied())
        .filter_map(|(prefix, label, closing)| {
            let mut search_from = offset;
            while let Some(relative_start) = text[search_from..].find(prefix) {
                let start = search_from + relative_start;
                if start == 0 || text.as_bytes().get(start.wrapping_sub(1)) == Some(&b'\n') {
                    return Some((start, label, prefix, closing));
                }
                search_from = start + prefix.len();
            }
            None
        })
        .min_by_key(|(start, _, _, _)| *start)
}

const INTERNAL_CONTEXT_MARKERS: [(&str, &str, Option<&str>); 18] = [
    (
        "# AGENTS.md instructions",
        "AGENTS.md",
        Some("</INSTRUCTIONS>"),
    ),
    (
        "<recommended_plugins>",
        "Recommended plugins",
        Some("</recommended_plugins>"),
    ),
    (
        "<environment_context>",
        "Environment",
        Some("</environment_context>"),
    ),
    (
        "<permissions instructions>",
        "Permissions",
        Some("</permissions instructions>"),
    ),
    ("<app-context>", "App context", Some("</app-context>")),
    (
        "<collaboration_mode>",
        "Collaboration",
        Some("</collaboration_mode>"),
    ),
    (
        "<skills_instructions>",
        "Skills",
        Some("</skills_instructions>"),
    ),
    (
        "<plugins_instructions>",
        "Plugins",
        Some("</plugins_instructions>"),
    ),
    (
        "<system-reminder>",
        "System reminder",
        Some("</system-reminder>"),
    ),
    (
        "<available_subagent_types>",
        "Subagent types",
        Some("</available_subagent_types>"),
    ),
    (
        "<user_instructions>",
        "User instructions",
        Some("</user_instructions>"),
    ),
    (
        "<local-command-caveat>",
        "Local command",
        Some("</local-command-caveat>"),
    ),
    ("<command-name>", "Command", Some("</command-name>")),
    (
        "<local-command-stdout>",
        "Command output",
        Some("</local-command-stdout>"),
    ),
    (
        "<task-notification>",
        "Task notification",
        Some("</task-notification>"),
    ),
    (
        "<subagent_notification>",
        "Subagent",
        Some("</subagent_notification>"),
    ),
    ("<turn_aborted>", "Turn aborted", Some("</turn_aborted>")),
    (
        "<in-app-browser-context",
        "Browser context",
        Some("</in-app-browser-context>"),
    ),
];

pub(crate) fn summarize_tool_call(payload: &Value) -> String {
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

    String::new()
}

pub(crate) fn push_item(
    items: &mut Vec<TranscriptItem>,
    kind: &str,
    body: String,
    tag: Option<String>,
    time: Option<String>,
) {
    push_tool_item(items, kind, body, tag, time, None, None, None, None, None);
}

pub(crate) fn push_tool_item(
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

pub(crate) fn attach_tool_result(
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

pub(crate) fn extract_call_id(payload: &Value) -> Option<String> {
    payload
        .get("call_id")
        .or_else(|| payload.get("callId"))
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub(crate) fn extract_tool_command(payload: &Value) -> Option<String> {
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

pub(crate) fn extract_tool_result(payload: &Value) -> Option<String> {
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

pub(crate) fn extract_duration_ms(payload: &Value, output: Option<&str>) -> Option<u64> {
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

pub(crate) fn parse_timestamp_ms(value: &str) -> Option<i64> {
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

pub(crate) fn compact_time(value: &str) -> String {
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
        fmt::Write as _,
        fs,
        io::Write as _,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use rusqlite::Connection;
    use serde_json::json;

    use super::{
        TranscriptSearchScopes, collect_generic_item, parse_search_transcript,
        parse_transcript,
        parse_transcript_locator_page, parse_transcript_page, parse_transcript_page_at_snapshot,
        search_transcript, summarize_tool_call, transcript_search_cache_offsets, transcript_source_version,
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
        let locator =
            parse_transcript_locator_page(&path, crate::skills::AgentKind::Codex).unwrap();
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

        let locator =
            parse_transcript_locator_page(&path, crate::skills::AgentKind::Codex).unwrap();

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
    fn transcript_cursor_requests_restart_when_the_file_appends() {
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

        assert!(stale.restart_required);
        assert!(stale.items.is_empty());
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

        assert_eq!(
            summarize_tool_call(&payload),
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
        assert_eq!(items[2].time.as_deref(), Some("10:12"));
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
}
