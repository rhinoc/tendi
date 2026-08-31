use std::{fs, path::Path};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct FixtureManifest {
    fixtures: Vec<FixtureEntry>,
}

#[derive(Debug, Deserialize)]
struct FixtureEntry {
    scope: String,
    provider: String,
    path: String,
}

#[test]
fn skill_fixture_manifest_covers_scope_and_provider_identity() {
    let manifest = serde_json::from_str::<FixtureManifest>(include_str!(
        "../testdata/skills/fixture-manifest.json"
    ))
    .unwrap();
    assert_eq!(manifest.fixtures.len(), 3);

    for fixture in manifest.fixtures {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/skills")
            .join(&fixture.path);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("---\nname: demo\n"));
        assert!(!fixture.scope.is_empty());
        assert!(!fixture.provider.is_empty());
    }
}
