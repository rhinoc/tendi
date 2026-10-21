use super::*;
use sha2::{Digest, Sha256};

fn snapshot(
    path: &Path,
    agent: AgentKind,
    id: Option<&str>,
) -> Result<Option<(TranscriptScan, String)>> {
    let Some(result) = agent_provider(agent).read_session_transcript(path, id.unwrap_or("")) else {
        return Ok(None);
    };
    let scan = result?;
    let version = source_version(id, &scan)?;
    Ok(Some((scan, version)))
}

pub fn parse_session_transcript(
    path: &Path,
    agent: AgentKind,
    id: Option<&str>,
) -> Result<TranscriptScan> {
    match agent_provider(agent).read_session_transcript(path, id.unwrap_or("")) {
        Some(scan) => scan,
        None => parse_transcript(path, agent),
    }
}

pub fn write_session_transcript_json<W: std::io::Write>(
    path: &Path,
    agent: AgentKind,
    id: Option<&str>,
    writer: &mut W,
) -> Result<Vec<String>> {
    if let Some(scan) = agent_provider(agent).read_session_transcript(path, id.unwrap_or("")) {
        let scan = scan?;
        serde_json::to_writer(writer, &scan.items)?;
        Ok(scan.warnings)
    } else {
        write_transcript_json(path, agent, writer)
    }
}

pub(super) fn source_version(id: Option<&str>, scan: &TranscriptScan) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(id, scan))?)
    ))
}

pub fn parse_session_transcript_page_if_changed(
    path: &Path,
    agent: AgentKind,
    id: Option<&str>,
    cursor: Option<&str>,
    limit: Option<usize>,
    known_version: Option<&str>,
) -> Result<TranscriptPage> {
    let Some((scan, source_version)) = snapshot(path, agent, id)? else {
        return parse_transcript_page_if_changed(path, agent, cursor, limit, known_version);
    };
    native_page(scan, source_version, cursor, limit, known_version)
}

pub fn parse_session_transcript_page(
    path: &Path,
    agent: AgentKind,
    id: Option<&str>,
    cursor: Option<&str>,
    limit: Option<usize>,
) -> Result<TranscriptPage> {
    let Some((scan, source_version)) = snapshot(path, agent, id)? else {
        return parse_transcript_page(path, agent, cursor, limit);
    };
    native_page(scan, source_version, cursor, limit, None)
}

fn native_page(
    scan: TranscriptScan,
    source_version: String,
    cursor: Option<&str>,
    limit: Option<usize>,
    known_version: Option<&str>,
) -> Result<TranscriptPage> {
    let unchanged = cursor.is_none() && known_version == Some(source_version.as_str());
    let mut offset = 0;
    let mut restart_required = false;
    if let Some(cursor) = cursor {
        let (version, index) = cursor
            .strip_prefix("native:")
            .and_then(|cursor| cursor.split_once(':'))
            .context("invalid native transcript cursor")?;
        offset = index
            .parse::<usize>()
            .context("invalid native transcript offset")?;
        restart_required = version != source_version || offset > scan.items.len();
    }
    let end = offset
        .saturating_add(
            limit
                .unwrap_or(TRANSCRIPT_PAGE_DEFAULT_LIMIT)
                .clamp(1, TRANSCRIPT_PAGE_MAX_LIMIT),
        )
        .min(scan.items.len());
    let done = unchanged || (!restart_required && end == scan.items.len());
    Ok(TranscriptPage {
        items: if unchanged || restart_required {
            Vec::new()
        } else {
            scan.items[offset..end].to_vec()
        },
        locator_items: Vec::new(),
        warnings: scan.warnings,
        next_cursor: (!unchanged && !restart_required && !done)
            .then(|| format!("native:{source_version}:{end}")),
        done,
        source_version,
        restart_required,
        unchanged,
    })
}

pub fn parse_session_transcript_locator_page(
    path: &Path,
    agent: AgentKind,
    id: Option<&str>,
) -> Result<TranscriptLocatorPage> {
    let Some((scan, source_version)) = snapshot(path, agent, id)? else {
        return parse_transcript_locator_page(path, agent);
    };
    let mut builder = TranscriptLocatorBuilder::default();
    for item in &scan.items {
        builder.push(item);
    }
    Ok(TranscriptLocatorPage {
        locator_items: builder.finish(),
        warnings: scan.warnings,
        source_version,
    })
}

pub fn search_session_transcript(
    path: &Path,
    agent: AgentKind,
    id: Option<&str>,
    query: &str,
    scopes: &TranscriptSearchScopes,
) -> Result<TranscriptSearchResult> {
    let Some((scan, source_version)) = snapshot(path, agent, id)? else {
        return search_transcript(path, agent, query, scopes);
    };
    let needle = query.trim().to_lowercase();
    let mut builder = TranscriptSearchBuilder::default();
    if !needle.is_empty() {
        for item in &scan.items {
            builder.push(item, &needle, scopes);
        }
    }
    Ok(TranscriptSearchResult {
        hits: builder.finish(),
        warnings: scan.warnings,
        source_version,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptItemMatch {
    pub record_order: usize,
    pub role: &'static str,
    pub score: f64,
    pub snippet: String,
}

pub fn search_session_scoped_match(
    path: &Path,
    agent: AgentKind,
    id: Option<&str>,
    query: &str,
    scopes: &TranscriptSearchScopes,
    phrase: bool,
) -> Result<Option<TranscriptItemMatch>> {
    let query = query.trim().to_lowercase();
    let terms: Vec<String> = if phrase {
        (!query.is_empty())
            .then_some(query.clone())
            .into_iter()
            .collect()
    } else {
        query
            .split_whitespace()
            .map(str::to_string)
            .filter(|term| !term.is_empty())
            .collect()
    };
    if terms.is_empty() {
        return Ok(None);
    }

    if let Some(result) = agent_provider(agent).read_session_transcript(path, id.unwrap_or("")) {
        let scan = result?;
        return Ok(best_scoped_match(&scan.items, 0, &terms, scopes));
    }

    let mut cursor = None;
    let mut record_order = 0;
    let mut best = None;
    loop {
        let page = parse_session_transcript_page(path, agent, id, cursor.as_deref(), Some(400))?;
        for item in &page.items {
            if let Some(candidate) = transcript_item_match(item, record_order, &terms, scopes) {
                if best.as_ref().is_none_or(|current: &TranscriptItemMatch| {
                    candidate.score > current.score
                        || (candidate.score == current.score
                            && candidate.record_order < current.record_order)
                }) {
                    best = Some(candidate);
                }
            }
            record_order += 1;
        }
        if page.done {
            break;
        }
        anyhow::ensure!(
            page.next_cursor.is_some(),
            "transcript reader did not advance during transcript search"
        );
        cursor = page.next_cursor;
    }
    Ok(best)
}

fn best_scoped_match(
    items: &[TranscriptItem],
    start_record_order: usize,
    terms: &[String],
    scopes: &TranscriptSearchScopes,
) -> Option<TranscriptItemMatch> {
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            transcript_item_match(item, start_record_order + index, terms, scopes)
        })
        .max_by(|left, right| {
            left.score
                .total_cmp(&right.score)
                .then_with(|| right.record_order.cmp(&left.record_order))
        })
}

fn transcript_item_match(
    item: &TranscriptItem,
    record_order: usize,
    terms: &[String],
    scopes: &TranscriptSearchScopes,
) -> Option<TranscriptItemMatch> {
    if !super::scope_enabled(&item.kind, false, scopes) {
        return None;
    }
    let fields = [
        item.command.as_deref().unwrap_or_default(),
        item.body.as_str(),
        item.tag.as_deref().unwrap_or_default(),
        item.result.as_deref().unwrap_or_default(),
    ];
    let lower = fields.map(str::to_lowercase);
    if !terms
        .iter()
        .all(|term| lower.iter().any(|field| field.contains(term)))
    {
        return None;
    }
    let best_field = lower
        .iter()
        .enumerate()
        .max_by_key(|(_, field)| terms.iter().filter(|term| field.contains(*term)).count())
        .map(|(index, _)| fields[index])
        .unwrap_or_default();
    let same_field_matches = lower
        .iter()
        .map(|field| terms.iter().filter(|term| field.contains(*term)).count())
        .max()
        .unwrap_or_default();
    let phrase_match = lower.iter().any(|field| field.contains(&terms.join(" ")));
    let score = terms.len() as f64 * 8.0
        + same_field_matches as f64 * 4.0
        + if phrase_match { 20.0 } else { 0.0 };
    let snippet = format_transcript_search_snippet(best_field, &terms.join(" "));
    Some(TranscriptItemMatch {
        record_order,
        role: match item.kind.as_str() {
            "tool" | "toolGroup" => "Tool",
            "context" | "compaction" | "model_config" => "System",
            _ => "Assistant",
        },
        score,
        snippet,
    })
}

fn format_transcript_search_snippet(value: &str, query: &str) -> String {
    const MAX_CHARS: usize = 280;
    let char_count = value.chars().count();
    if char_count <= MAX_CHARS {
        return value.to_string();
    }
    let mut lower = String::new();
    let mut lower_char_sources = Vec::new();
    for (byte_index, character) in value.char_indices() {
        let lowered = character.to_lowercase().collect::<String>();
        lower_char_sources.extend(std::iter::repeat_n(byte_index, lowered.chars().count()));
        lower.push_str(&lowered);
    }
    let match_byte = lower
        .find(&query.to_lowercase())
        .map(|lower_byte| lower[..lower_byte].chars().count())
        .and_then(|lower_char| lower_char_sources.get(lower_char).copied())
        .unwrap_or_default();
    let match_char = value[..match_byte].chars().count();
    let start = match_char.saturating_sub(MAX_CHARS / 3);
    let end = (start + MAX_CHARS).min(char_count);
    let excerpt = value
        .chars()
        .skip(start)
        .take(end - start)
        .collect::<String>();
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        excerpt,
        if end < char_count { "…" } else { "" }
    )
}
