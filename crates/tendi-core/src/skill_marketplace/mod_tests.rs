use super::*;

#[test]
fn merges_results_by_source_and_keeps_official_results_first() {
    let make_source = |provider: &'static str, source: &str, name: &str| MarketplaceSource {
        provider,
        id: name.to_string(),
        name: name.to_string(),
        description: None,
        source: source.to_string(),
        url: None,
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
