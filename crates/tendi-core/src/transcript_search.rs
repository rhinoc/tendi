//! Durable JSONL search cursor. Provider parsers own message semantics; this
//! module owns byte boundaries, provisional EOF records and append validation.
//!
//! Append reuse assumes an immutable historical prefix. Identity, same-size
//! writes, truncation and bounded prefix/boundary anchors reject observed
//! rewrites. An arbitrary middle overwrite combined with growth cannot be
//! detected without reading that middle; callers with known rewrite evidence
//! must discard the checkpoint. This is not a whole-file integrity checksum.

use super::*;
use sha2::{Digest, Sha256};

const ANCHOR_BYTES: u64 = 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SearchCheckpoint {
    pub parser_version: String,
    identity: String,
    source_size: u64,
    source_modified_ns: u128,
    pub committed_offset: u64,
    /// Next searchable record after the committed complete lines (metadata is 0).
    pub next_record_order: usize,
    line_count: u64,
    inherited_ordinal: Option<u64>,
    prefix_hash: String,
    boundary_hash: String,
}

pub(crate) struct SearchDelta {
    pub checkpoint: SearchCheckpoint,
    pub start_record_order: usize,
    pub items: Vec<TranscriptItem>,
    pub warnings: Vec<String>,
    pub bytes_read: u64,
    pub validation_bytes_read: u64,
}

impl SearchCheckpoint {
    pub(crate) fn matches_source(&self, path: &Path) -> Result<bool> {
        let metadata = fs::metadata(path)?;
        Ok(self.identity == file_identity(&metadata)
            && self.source_size == metadata.len()
            && self.source_modified_ns == modified_ns(&metadata))
    }
}

fn file_identity(metadata: &fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        format!("{:?}", metadata.created().ok())
    }
}

fn modified_ns(metadata: &fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos())
}

fn anchor(file: &mut fs::File, offset: u64, length: u64, bytes: &mut u64) -> Result<String> {
    file.seek(SeekFrom::Start(offset))?;
    let mut content = vec![0; length as usize];
    file.read_exact(&mut content)?;
    *bytes += length;
    Ok(format!("{:x}", Sha256::digest(content)))
}

pub(crate) fn read_search_delta(
    path: &Path,
    agent: AgentKind,
    previous: Option<&SearchCheckpoint>,
    should_stop: &dyn Fn() -> bool,
) -> Result<Option<SearchDelta>> {
    let mut file =
        fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let metadata = file.metadata()?;
    let size = metadata.len();
    let provider = agent_provider(agent);
    let version = provider.transcript_search_append_version();
    let identity = file_identity(&metadata);
    let modified = modified_ns(&metadata);
    let mut validation_bytes_read = 0;
    let mut reusable = previous.filter(|old| {
        version == Some(old.parser_version.as_str())
            && old.identity == identity
            && size >= old.source_size
            && old.committed_offset <= old.source_size
            && (size > old.source_size || modified == old.source_modified_ns)
    });
    if let Some(old) = reusable {
        let width = old.committed_offset.min(ANCHOR_BYTES);
        if anchor(&mut file, 0, width, &mut validation_bytes_read)? != old.prefix_hash
            || anchor(
                &mut file,
                old.committed_offset - width,
                width,
                &mut validation_bytes_read,
            )? != old.boundary_hash
        {
            reusable = None;
        }
    }
    let mut checkpoint = reusable.cloned().unwrap_or(SearchCheckpoint {
        parser_version: version.unwrap_or("full-search-v1").to_owned(),
        identity,
        source_size: size,
        source_modified_ns: modified,
        committed_offset: 0,
        next_record_order: 1,
        line_count: 0,
        inherited_ordinal: None,
        prefix_hash: String::new(),
        boundary_hash: String::new(),
    });
    let start_record_order = checkpoint.next_record_order;
    let start_offset = checkpoint.committed_offset;
    if reusable.is_none() {
        // Full-reader semantics resolve inherited history before any messages,
        // even when a provider header is preceded by other JSONL records.
        file.seek(SeekFrom::Start(0))?;
        let mut header_reader = BufReader::new((&mut file).take(size));
        let mut header = Vec::new();
        for _ in 0..64 {
            if should_stop() {
                return Ok(None);
            }
            header.clear();
            let count = header_reader.read_until(b'\n', &mut header)?;
            validation_bytes_read += count as u64;
            if count == 0 {
                break;
            }
            if let Ok(value) = serde_json::from_slice::<Value>(&header) {
                if let Some(ordinal) = provider.transcript_inherited_history_start_ordinal(&value) {
                    checkpoint.inherited_ordinal = Some(ordinal);
                    break;
                }
                if value.get("type").and_then(Value::as_str) == Some("session_meta") {
                    break;
                }
            }
        }
    }
    file.seek(SeekFrom::Start(start_offset))?;
    let mut reader = BufReader::new((&mut file).take(size - start_offset));
    let mut raw_line = Vec::new();
    let mut items = Vec::new();
    let mut warnings = Vec::new();
    let mut bytes_read = 0;
    loop {
        if should_stop() {
            return Ok(None);
        }
        raw_line.clear();
        let count = reader.read_until(b'\n', &mut raw_line)?;
        if count == 0 {
            break;
        }
        bytes_read += count as u64;
        let complete = raw_line.ends_with(b"\n");
        let line = match std::str::from_utf8(&raw_line) {
            Ok(line) => line,
            Err(_) if !complete => break, // A provider can split a UTF-8 codepoint at EOF.
            Err(error) => return Err(error.into()),
        };
        let before = items.len();
        // Header metadata is examined before the message hint, just as the
        // provider's full reader does. It is persisted, not rescanned on append.
        if !line.trim().is_empty()
            && (checkpoint.line_count < 64 || provider.transcript_search_hint(&line))
        {
            match serde_json::from_str::<Value>(&line) {
                Ok(value) => {
                    if checkpoint.line_count < 64 {
                        if let Some(ordinal) =
                            provider.transcript_inherited_history_start_ordinal(&value)
                        {
                            checkpoint.inherited_ordinal = Some(ordinal);
                        }
                    }
                    if provider.transcript_search_hint(&line)
                        && !is_inherited_transcript_value(&value, checkpoint.inherited_ordinal)
                    {
                        let mut parsed = Vec::new();
                        collect_transcript_value(&value, agent, &mut parsed);
                        items.extend(
                            parsed
                                .into_iter()
                                .filter(|item| matches!(item.kind.as_str(), "user" | "assistant")),
                        );
                    }
                }
                Err(error) if complete => {
                    warnings.push(format!("line {}: {error}", checkpoint.line_count + 1))
                }
                Err(_) => {} // An incomplete EOF line remains uncommitted.
            }
        }
        if complete {
            checkpoint.committed_offset += count as u64;
            checkpoint.next_record_order += items.len() - before;
            checkpoint.line_count += 1;
        }
        // A valid unterminated JSON record is provisionally searchable. Keep
        // its byte/order boundary before it so the next pass replaces that tail.
        if !complete {
            break;
        }
    }
    drop(reader);
    checkpoint.source_size = size;
    checkpoint.source_modified_ns = modified;
    let width = checkpoint.committed_offset.min(ANCHOR_BYTES);
    checkpoint.prefix_hash = anchor(&mut file, 0, width, &mut validation_bytes_read)?;
    checkpoint.boundary_hash = anchor(
        &mut file,
        checkpoint.committed_offset - width,
        width,
        &mut validation_bytes_read,
    )?;
    Ok(Some(SearchDelta {
        checkpoint,
        start_record_order,
        items,
        warnings,
        bytes_read,
        validation_bytes_read,
    }))
}

#[cfg(test)]
#[path = "transcript_search_tests.rs"]
mod tests;
