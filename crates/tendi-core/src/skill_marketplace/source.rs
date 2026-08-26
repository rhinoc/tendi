use std::collections::BTreeSet;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceSource {
    #[serde(skip)]
    pub(crate) provider: &'static str,
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub version: Option<String>,
    pub metric: Option<u64>,
    pub metric_label: Option<String>,
    pub trust_label: Option<String>,
    pub kind: MarketplaceEntryKind,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MarketplaceEntryKind {
    Skill,
    Plugin,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceSearchResult {
    pub items: Vec<MarketplaceSource>,
    pub warnings: Vec<String>,
}

pub(crate) fn merge_marketplace_sources(sources: Vec<MarketplaceSource>) -> Vec<MarketplaceSource> {
    let mut seen = BTreeSet::new();
    let mut sources = sources
        .into_iter()
        .filter(|source| seen.insert(source.source.to_ascii_lowercase()))
        .collect::<Vec<_>>();
    sources.sort_by(|left, right| {
        provider_order(left.provider)
            .cmp(&provider_order(right.provider))
            .then_with(|| right.metric.cmp(&left.metric))
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
    });
    sources
}

fn provider_order(provider: &str) -> u8 {
    match provider {
        "claude-official" => 0,
        "skillsmp" => 1,
        "clawhub" => 2,
        _ => 3,
    }
}
