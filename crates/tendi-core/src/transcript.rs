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

use crate::{providers::agent_provider, skills::AgentKind, time::timestamp_ms};

#[path = "transcript_search.rs"]
mod search;
pub(crate) use search::{SearchCheckpoint, read_search_delta};

#[path = "transcript_excerpt.rs"]
mod excerpt;
pub use excerpt::{TranscriptExcerpt, TranscriptExcerptItem, TranscriptExcerptOptions, read_transcript_excerpt};

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

fn transcript_source_identity_append_compatible(
    path: &Path,
    previous: &TranscriptSourceIdentity,
    current: &TranscriptSourceIdentity,
) -> Result<bool> {
    if previous.device != current.device
        || previous.inode != current.inode
        || previous.prefix_len > current.prefix_len
    {
        return Ok(false);
    }
    if previous.prefix_len == current.prefix_len {
        return Ok(previous.prefix_hash == current.prefix_hash);
    }

    let mut file = fs::File::open(path)?;
    let mut prefix = vec![0u8; previous.prefix_len];
    file.read_exact(&mut prefix)?;
    let prefix_hash = prefix.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    Ok(prefix_hash == previous.prefix_hash)
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
        Ok(())
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
            // Live agent transcripts grow by appending complete JSONL records. Keep the
            // cursor valid when the consumed prefix is unchanged so pagination can follow
            // the live tail without rescanning from the beginning.
            let append_compatible = transcript_source_identity_append_compatible(
                path,
                &cursor.source,
                &source.identity,
            )? && cursor.offset <= source.size
                && source.size >= cursor.source_size
                && (source.size > cursor.source_size
                    || source.modified_ns == cursor.source_modified_ns)
                && transcript_boundary_hash(path, cursor.offset)? == cursor.boundary_hash;
            !append_compatible
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
    if let Some(store_path) = cursor_store
        .map(Path::to_path_buf)
        .or_else(|| agent_provider(agent).transcript_metadata_store_path(path))
    {
        agent_provider(agent).enrich_transcript_tools_from_store(&store_path, &mut items)?;
        if done && include_metadata && !agent_provider(agent).transcript_cacheable() {
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

/// Writes transcript items without retaining the complete transcript in memory.
/// Providers that enrich items after parsing keep the existing materialized path.
pub fn write_transcript_json<W: std::io::Write>(
    path: &Path,
    agent: AgentKind,
    writer: &mut W,
) -> Result<Vec<String>> {
    if !agent_provider(agent).transcript_cacheable() {
        let transcript = parse_transcript(path, agent)?;
        serde_json::to_writer(&mut *writer, &transcript.items)?;
        return Ok(transcript.warnings);
    }

    writer.write_all(b"[")?;
    let mut first = true;
    let warnings = for_each_transcript_item(path, agent, |item| {
        if !first {
            writer.write_all(b",")?;
        }
        first = false;
        serde_json::to_writer(&mut *writer, &item)?;
        Ok(())
    })?;
    writer.write_all(b"]")?;
    Ok(warnings)
}

enum TranscriptLocatorPendingGroup {
    CommandRun { item_count: usize, has_exec: bool },
    ToolRun { item_count: usize, all_exec: bool },
}

#[derive(Default)]
struct TranscriptLocatorBuilder {
    items: Vec<TranscriptLocatorItem>,
    pending_response: Option<usize>,
    grouped_index: usize,
    pending_group: Option<TranscriptLocatorPendingGroup>,
}

impl TranscriptLocatorBuilder {
    fn push(&mut self, item: &TranscriptItem) {
        let kind = item.kind.as_str();
        match kind {
            "reasoning" | "thinking" => self.push_command_component(false),
            "tool"
                if item
                    .tag
                    .as_deref()
                    .is_some_and(|tag| tag.trim().eq_ignore_ascii_case("exec")) =>
            {
                self.push_command_component(true)
            }
            "tool" => self.push_tool(),
            _ => {
                self.flush_pending_group();
                let item_index = self.grouped_index;
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
                self.grouped_index += 1;
            }
        }
    }

    fn push_command_component(&mut self, is_exec: bool) {
        self.pending_group = Some(match self.pending_group.take() {
            None => TranscriptLocatorPendingGroup::CommandRun {
                item_count: 1,
                has_exec: is_exec,
            },
            Some(TranscriptLocatorPendingGroup::CommandRun {
                item_count,
                has_exec,
            }) => TranscriptLocatorPendingGroup::CommandRun {
                item_count: item_count + 1,
                has_exec: has_exec || is_exec,
            },
            Some(TranscriptLocatorPendingGroup::ToolRun {
                item_count,
                all_exec: true,
            }) => TranscriptLocatorPendingGroup::CommandRun {
                item_count: item_count + 1,
                has_exec: true,
            },
            Some(TranscriptLocatorPendingGroup::ToolRun { .. }) => {
                self.grouped_index += 1;
                TranscriptLocatorPendingGroup::CommandRun {
                    item_count: 1,
                    has_exec: is_exec,
                }
            }
        });
    }

    fn push_tool(&mut self) {
        self.pending_group = Some(match self.pending_group.take() {
            None => TranscriptLocatorPendingGroup::ToolRun {
                item_count: 1,
                all_exec: false,
            },
            Some(TranscriptLocatorPendingGroup::CommandRun {
                item_count,
                has_exec: true,
            }) => {
                if item_count > 1 {
                    self.grouped_index += 1;
                    TranscriptLocatorPendingGroup::ToolRun {
                        item_count: 1,
                        all_exec: false,
                    }
                } else {
                    TranscriptLocatorPendingGroup::ToolRun {
                        item_count: item_count + 1,
                        all_exec: false,
                    }
                }
            }
            Some(TranscriptLocatorPendingGroup::CommandRun {
                item_count,
                has_exec: false,
            }) => {
                self.grouped_index += item_count;
                TranscriptLocatorPendingGroup::ToolRun {
                    item_count: 1,
                    all_exec: false,
                }
            }
            Some(TranscriptLocatorPendingGroup::ToolRun { item_count, .. }) => {
                TranscriptLocatorPendingGroup::ToolRun {
                    item_count: item_count + 1,
                    all_exec: false,
                }
            }
        });
    }

    fn flush_pending_group(&mut self) {
        match self.pending_group.take() {
            Some(TranscriptLocatorPendingGroup::CommandRun {
                item_count,
                has_exec: true,
            }) if item_count > 1 => self.grouped_index += 1,
            Some(TranscriptLocatorPendingGroup::CommandRun { item_count, .. }) => {
                self.grouped_index += item_count
            }
            Some(TranscriptLocatorPendingGroup::ToolRun { .. }) => self.grouped_index += 1,
            None => {}
        }
    }

    fn finish(mut self) -> Vec<TranscriptLocatorItem> {
        self.flush_pending_group();
        self.items
    }
}

fn for_each_transcript_item<F>(path: &Path, agent: AgentKind, mut visit: F) -> Result<Vec<String>>
where
    F: FnMut(TranscriptItem) -> Result<()>,
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
            visit(item)?;
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

#[cfg(test)]
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
    let source_size = file.metadata()?.len();
    let inherited_history_start_ordinal = transcript_inherited_history_start_ordinal(path, agent)?;
    let mut warnings = Vec::new();

    for (index, line) in BufReader::new(file.take(source_size)).lines().enumerate() {
        let line =
            line.with_context(|| format!("failed to read {}:{}", path.display(), index + 1))?;
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

pub(crate) fn collect_shared_item(value: &Value, items: &mut Vec<TranscriptItem>) {
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_string);
    collect_shared_format_item_with_timestamp(value, items, timestamp);
}

pub(crate) fn collect_cursor_item_with_timestamp(
    value: &Value,
    items: &mut Vec<TranscriptItem>,
    timestamp: Option<String>,
) {
    collect_shared_format_item_with_timestamp(value, items, timestamp);
}

fn collect_shared_format_item_with_timestamp(
    value: &Value,
    items: &mut Vec<TranscriptItem>,
    timestamp: Option<String>,
) {
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
                timestamp,
            );
        }
        return;
    }
    if kind != "user" && kind != "assistant" {
        return;
    }

    let time = timestamp.clone();
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
                    timestamp.as_deref().and_then(timestamp_ms),
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
            .or_else(|| {
                value
                    .get("message")
                    .and_then(Value::as_str)
                    .and_then(clean_body)
            }),
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
    let text = strip_timestamp_tags(text);
    let text = split_internal_context_segments(&text)
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

fn strip_timestamp_tags(text: &str) -> String {
    const OPEN: &str = "<timestamp>";
    const CLOSE: &str = "</timestamp>";
    let mut result = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(relative_start) = text[cursor..].find(OPEN) {
        let start = cursor + relative_start;
        result.push_str(&text[cursor..start]);
        let content_start = start + OPEN.len();
        let Some(relative_end) = text[content_start..].find(CLOSE) else {
            result.push_str(&text[start..]);
            return result;
        };
        cursor = content_start + relative_end + CLOSE.len();
    }
    result.push_str(&text[cursor..]);
    result
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
    let Some(call_id) = call_id.filter(|call_id| !call_id.trim().is_empty()) else {
        return false;
    };
    let matched = items
        .iter_mut()
        .rev()
        .find(|item| item.kind == "tool" && item.call_id.as_deref() == Some(call_id));
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

    if let Some(arguments) = payload.get("arguments") {
        if let Some(arguments) = arguments.as_str() {
            if let Ok(value) = serde_json::from_str::<Value>(arguments) {
                if let Some(command) = value
                    .get("cmd")
                    .or_else(|| value.get("command"))
                    .and_then(Value::as_str)
                {
                    return Some(truncate_text(command.trim(), 4_000));
                }
            }
        }
        if let Some(text) = serialize_tool_input(arguments) {
            return Some(truncate_text(&text, 4_000));
        }
    }

    payload
        .get("action")
        .and_then(serialize_tool_input)
        .map(|action| truncate_text(&action, 4_000))
        .or_else(|| {
            payload
                .get("input")
                .and_then(serialize_tool_input)
                .map(|input| truncate_text(&input, 4_000))
        })
}

fn serialize_tool_input(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => text.trim().to_string(),
        Value::Object(_) | Value::Array(_) => serde_json::to_string(value).ok()?,
        Value::Null => return None,
        _ => value.to_string(),
    };
    (!text.is_empty()).then_some(text)
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

fn truncate_text(value: &str, limit: usize) -> String {
    let mut text: String = value.chars().take(limit).collect();
    if value.chars().count() > limit {
        text.push_str("\n... truncated");
    }
    text
}

pub fn compact_time(value: &str) -> String {
    value
        .split('T')
        .nth(1)
        .and_then(|time| time.get(0..5))
        .unwrap_or(value)
        .to_string()
}

pub fn compact_local_time(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|timestamp| {
            timestamp
                .with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| compact_time(value))
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
