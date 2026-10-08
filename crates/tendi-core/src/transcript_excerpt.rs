use super::*;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct TranscriptExcerptOptions {
    pub query: Option<String>,
    pub roles: Vec<String>,
    pub offset: usize,
    pub limit: usize,
    pub around: usize,
}

impl Default for TranscriptExcerptOptions {
    fn default() -> Self {
        Self {
            query: None,
            roles: Vec::new(),
            offset: 0,
            limit: 20,
            around: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptExcerptItem {
    pub index: usize,
    pub matched: bool,
    #[serde(flatten)]
    pub item: TranscriptItem,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptExcerpt {
    pub items: Vec<TranscriptExcerptItem>,
    pub warnings: Vec<String>,
    pub source_version: String,
    pub next_offset: Option<usize>,
}

/// Reuse bounded provider pages, retaining only selected messages and nearby
/// context. Offset is a normalized item index, not a raw JSONL line number.
pub fn read_transcript_excerpt(
    path: &Path,
    agent: AgentKind,
    options: &TranscriptExcerptOptions,
) -> Result<TranscriptExcerpt> {
    anyhow::ensure!(
        (1..=1000).contains(&options.limit),
        "limit must be between 1 and 1000"
    );
    anyhow::ensure!(options.around <= 20, "around must be between 0 and 20");
    let terms: Vec<_> = options
        .query
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    if options.query.is_some() {
        anyhow::ensure!(!terms.is_empty(), "match query must not be empty");
    }
    let mut cursor = None;
    let mut source_version = None;
    let mut warnings = Vec::new();
    let mut selected = BTreeMap::<usize, TranscriptExcerptItem>::new();
    let mut previous = VecDeque::<TranscriptExcerptItem>::new();
    let mut index = 0;
    let mut centers = 0;
    let mut last_center = None;
    loop {
        let page = parse_transcript_page(path, agent, cursor.as_deref(), Some(400))?;
        anyhow::ensure!(
            !page.restart_required,
            "transcript changed during reading; restart the query"
        );
        if let Some(version) = source_version.as_ref() {
            anyhow::ensure!(
                version == &page.source_version,
                "transcript changed during reading; restart the query"
            );
        } else {
            source_version = Some(page.source_version.clone());
        }
        warnings.extend(page.warnings);
        let page_len = page.items.len();
        for (position, item) in page.items.into_iter().enumerate() {
            let eligible = index >= options.offset
                && centers < options.limit
                && (options.roles.is_empty() || options.roles.contains(&item.kind));
            let matched = eligible
                && (terms.is_empty() || {
                    let body = item.body.to_lowercase();
                    terms.iter().all(|term| body.contains(term))
                });
            let entry = TranscriptExcerptItem {
                index,
                matched,
                item,
            };
            if matched {
                for context in &previous {
                    selected
                        .entry(context.index)
                        .or_insert_with(|| context.clone());
                }
                selected.insert(index, entry.clone());
                centers += 1;
                last_center = Some(index);
            } else if last_center
                .is_some_and(|center| index <= center.saturating_add(options.around))
            {
                selected.entry(index).or_insert_with(|| entry.clone());
            }
            previous.push_back(entry);
            while previous.len() > options.around {
                previous.pop_front();
            }
            let complete = centers == options.limit
                && last_center.is_some_and(|center| index >= center.saturating_add(options.around));
            index += 1;
            if complete {
                let more = position + 1 < page_len || !page.done;
                return Ok(TranscriptExcerpt {
                    items: selected.into_values().collect(),
                    warnings,
                    source_version: source_version.unwrap_or_default(),
                    next_offset: more.then(|| last_center.unwrap() + 1),
                });
            }
        }
        if page.done {
            break;
        }
        anyhow::ensure!(
            page.next_cursor.is_some(),
            "transcript reader did not advance"
        );
        cursor = page.next_cursor;
    }
    Ok(TranscriptExcerpt {
        items: selected.into_values().collect(),
        warnings,
        source_version: source_version.unwrap_or_default(),
        next_offset: None,
    })
}

#[cfg(test)]
#[path = "transcript_excerpt_tests.rs"]
mod tests;
