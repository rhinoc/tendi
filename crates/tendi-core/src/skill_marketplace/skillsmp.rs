use anyhow::{Result, bail};
use serde::Deserialize;

use super::{
    http::{encode_query, get_json},
    source::MarketplaceSource,
};

const SEARCH_URL: &str = "https://skillsmp.com/api/v1/skills/search";

#[derive(Debug, Deserialize)]
struct Response {
    success: bool,
    data: Data,
}

#[derive(Debug, Deserialize)]
struct Data {
    skills: Vec<Skill>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Skill {
    id: String,
    name: String,
    description: Option<String>,
    github_url: Option<String>,
    skill_url: Option<String>,
    stars: Option<u64>,
}

pub(super) fn search(query: &str) -> Result<Vec<MarketplaceSource>> {
    let url = format!(
        "{SEARCH_URL}?q={}&limit=50&sortBy=stars",
        encode_query(query)
    );
    let response: Response = get_json(&url)?;
    if !response.success {
        bail!("SkillsMP returned an unsuccessful response");
    }

    Ok(response
        .data
        .skills
        .into_iter()
        .filter_map(|skill| {
            let source = skill.github_url?.trim().to_string();
            if source.is_empty() {
                return None;
            }
            Some(MarketplaceSource {
                provider: "skillsmp",
                id: skill.id,
                name: skill.name,
                description: skill.description,
                source,
                url: skill.skill_url.unwrap_or_default(),
                version: None,
                metric: skill.stars,
                metric_label: Some("GitHub stars".to_string()),
                trust_label: Some("Community".to_string()),
                kind: super::MarketplaceEntryKind::Skill,
            })
        })
        .collect())
}
