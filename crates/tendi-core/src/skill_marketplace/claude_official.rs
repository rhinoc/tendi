use anyhow::Result;
use serde::Deserialize;

use super::{MarketplaceEntryKind, http::get_json, source::MarketplaceSource};

const MARKETPLACE_URL: &str = "https://raw.githubusercontent.com/anthropics/claude-plugins-official/main/.claude-plugin/marketplace.json";

#[derive(Debug, Deserialize)]
struct Marketplace {
    plugins: Vec<Plugin>,
}

#[derive(Debug, Deserialize)]
struct Plugin {
    name: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    description: Option<String>,
    source: PluginSource,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PluginSource {
    Path(String),
    Remote(RemoteSource),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteSource {
    source: Option<String>,
    repo: Option<String>,
    url: Option<String>,
    path: Option<String>,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
}

pub(super) fn search(query: &str) -> Result<Vec<MarketplaceSource>> {
    let catalog: Marketplace = get_json(MARKETPLACE_URL)?;
    let query = query.to_ascii_lowercase();
    Ok(catalog
        .plugins
        .into_iter()
        .filter_map(|plugin| {
            let display_name = plugin
                .display_name
                .clone()
                .unwrap_or_else(|| plugin.name.clone());
            let haystack = format!(
                "{} {} {}",
                plugin.name,
                display_name,
                plugin.description.as_deref().unwrap_or_default()
            )
            .to_ascii_lowercase();
            if !haystack.contains(&query) {
                return None;
            }
            let source = plugin_source(&plugin.source)?;
            Some(MarketplaceSource {
                provider: "claude-official",
                id: plugin.name,
                name: display_name,
                description: plugin.description,
                source,
                url: Some("https://claude.com/plugins".to_string()),
                version: None,
                metric: None,
                metric_label: None,
                trust_label: Some("Anthropic official catalog".to_string()),
                kind: MarketplaceEntryKind::Plugin,
            })
        })
        .collect())
}

fn plugin_source(source: &PluginSource) -> Option<String> {
    match source {
        PluginSource::Path(path) => {
            let path = path.strip_prefix("./").unwrap_or(path);
            Some(format!(
                "https://github.com/anthropics/claude-plugins-official/tree/main/{path}"
            ))
        }
        PluginSource::Remote(source) => {
            let git_ref = source.git_ref.as_deref().unwrap_or("main");
            let path = source.path.as_deref().unwrap_or_default();
            let repo = source
                .repo
                .as_deref()
                .or_else(|| source.url.as_deref())?
                .trim_end_matches(".git");
            if repo.starts_with("https://github.com/") {
                let repo = repo.trim_start_matches("https://github.com/");
                if path.is_empty() {
                    Some(format!("https://github.com/{repo}/tree/{git_ref}"))
                } else {
                    Some(format!("https://github.com/{repo}/tree/{git_ref}/{path}"))
                }
            } else if source.source.as_deref() == Some("github") && path.is_empty() {
                Some(format!("https://github.com/{repo}/tree/{git_ref}"))
            } else if path.is_empty() {
                Some(repo.to_string())
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_relative_plugin_source() {
        let source = PluginSource::Path("./plugins/example".to_string());
        assert_eq!(
            plugin_source(&source).as_deref(),
            Some("https://github.com/anthropics/claude-plugins-official/tree/main/plugins/example")
        );
    }
}
