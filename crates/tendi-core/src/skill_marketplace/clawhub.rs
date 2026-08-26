use anyhow::Result;
use serde::Deserialize;

use super::{
    http::{encode_query, get_json},
    source::MarketplaceSource,
};

const SEARCH_URL: &str = "https://clawhub.ai/api/v1/search";

#[derive(Debug, Deserialize)]
struct Response {
    results: Vec<Skill>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Skill {
    slug: String,
    display_name: String,
    summary: Option<String>,
    version: Option<String>,
    owner_handle: Option<String>,
}

pub(super) fn search(query: &str) -> Result<Vec<MarketplaceSource>> {
    let url = format!(
        "{SEARCH_URL}?q={}&limit=50&nonSuspiciousOnly=true",
        encode_query(query)
    );
    let response: Response = get_json(&url)?;

    Ok(response
        .results
        .into_iter()
        .map(|skill| {
            let mut source = format!(
                "https://clawhub.ai/api/v1/download?slug={}",
                encode_query(&skill.slug)
            );
            if let Some(version) = skill.version.as_deref() {
                source.push_str("&version=");
                source.push_str(&encode_query(version));
            }
            let owner = skill.owner_handle.clone().unwrap_or_default();
            let url = if owner.is_empty() {
                format!("https://clawhub.ai/skills/{}", skill.slug)
            } else {
                format!("https://clawhub.ai/{owner}/skills/{}", skill.slug)
            };
            MarketplaceSource {
                provider: "clawhub",
                id: if owner.is_empty() {
                    skill.slug.clone()
                } else {
                    format!("{owner}/{}", skill.slug)
                },
                name: skill.display_name,
                description: skill.summary,
                source,
                url: Some(url),
                version: skill.version,
                metric: None,
                metric_label: None,
                trust_label: Some("Security filter".to_string()),
                kind: super::MarketplaceEntryKind::Skill,
            }
        })
        .collect())
}
