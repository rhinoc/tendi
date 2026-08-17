mod claude_official;
mod clawhub;
mod http;
mod skillsmp;
mod source;

use anyhow::{Result, bail};

use source::{MarketplaceSearchResult, merge_marketplace_sources};

#[derive(Clone, Copy)]
struct ProviderSpec {
    search: fn(&str) -> Result<Vec<MarketplaceSource>>,
}

const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        search: skillsmp::search,
    },
    ProviderSpec {
        search: clawhub::search,
    },
    ProviderSpec {
        search: claude_official::search,
    },
];

pub use source::{MarketplaceEntryKind, MarketplaceSource};

pub fn search(query: &str) -> Result<MarketplaceSearchResult> {
    let query = query.trim();
    if query.chars().count() < 2 {
        bail!("marketplace search needs at least 2 characters");
    }

    search_all(query)
}

fn search_all(query: &str) -> Result<MarketplaceSearchResult> {
    std::thread::scope(|scope| {
        let handles = PROVIDERS
            .iter()
            .map(|provider| {
                let search = provider.search;
                scope.spawn(move || search(query))
            })
            .collect::<Vec<_>>();

        let mut items = Vec::new();
        let mut warnings = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(Ok(mut result)) => items.append(&mut result),
                Ok(Err(_)) | Err(_) => {
                    warnings.push("Some marketplaces are unavailable.".to_string())
                }
            }
        }

        if items.is_empty() && !warnings.is_empty() {
            bail!("all skill marketplaces are unavailable");
        }

        warnings.dedup();
        Ok(MarketplaceSearchResult {
            items: merge_marketplace_sources(items),
            warnings,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_results_by_source_and_keeps_official_results_first() {
        let make_source = |provider: &'static str, source: &str, name: &str| MarketplaceSource {
            provider,
            id: name.to_string(),
            name: name.to_string(),
            description: None,
            source: source.to_string(),
            url: String::new(),
            version: None,
            metric: None,
            metric_label: None,
            trust_label: None,
            kind: MarketplaceEntryKind::Skill,
        };
        let result = merge_marketplace_sources(vec![
            make_source("skillsmp", "https://example.com/shared", "community"),
            make_source(
                "claude-official",
                "https://example.com/official",
                "official",
            ),
            make_source("clawhub", "https://example.com/shared", "duplicate"),
        ]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "official");
    }
}
