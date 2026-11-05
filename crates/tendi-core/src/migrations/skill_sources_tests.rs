use std::{fs, time::SystemTime};

use super::read_skills_cli_lock;

#[test]
fn global_skills_cli_v3_lock_uses_source_url_and_tree_hash() {
    let root = std::env::temp_dir().join(format!(
        "tendi-global-skills-cli-lock-migration-{}",
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let lock_path = root.join(".skill-lock.json");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &lock_path,
        r#"{
  "version": 3,
  "skills": {
    "demo": {
      "source": "example/agent-skills",
      "sourceType": "github",
      "sourceUrl": "https://github.com/example/agent-skills.git",
      "ref": "main",
      "skillPath": "skills/demo/SKILL.md",
      "skillFolderHash": "tree-hash",
      "installedAt": "2026-08-01T00:00:00Z",
      "updatedAt": "2026-08-01T00:00:00Z"
    }
  }
}
"#,
    )
    .unwrap();

    let mut warnings = Vec::new();
    let lock = read_skills_cli_lock(&lock_path, 3, &mut warnings).unwrap();
    let entry = &lock.skills["demo"];
    assert!(warnings.is_empty());
    assert_eq!(
        entry.source(),
        "https://github.com/example/agent-skills.git"
    );
    assert_eq!(entry.source_type, "github");
    assert_eq!(entry.skill_folder_hash.as_deref(), Some("tree-hash"));
    assert_eq!(entry.r#ref.as_deref(), Some("main"));

    fs::remove_dir_all(root).unwrap();
}

struct WellKnownImport {
    root: std::path::PathBuf,
    store: crate::storage::Store,
    scan: crate::skills::SkillScan,
    digest: String,
}

impl WellKnownImport {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        crate::test_support::ensure_isolated_environment();
        let root = std::env::temp_dir().join(format!(
            "tendi-well-known-import-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join(".git")).unwrap();
        let skill = root.join(".agents/skills/import-demo");
        fs::create_dir_all(&skill).unwrap();
        let original = "---\nname: import-demo\ndescription: Demo\n---\n\n# Original\n";
        fs::write(skill.join("SKILL.md"), original).unwrap();
        let digest = crate::skill_source::skill_files_digest(
            &crate::skill_source::read_skill_files(&skill).unwrap(),
        );
        fs::write(
            skill.join("SKILL.md"),
            original.replace(
                "description: Demo",
                "description: Demo\ndisable-model-invocation: true",
            ),
        )
        .unwrap();
        fs::create_dir_all(skill.join("agents")).unwrap();
        fs::write(
            skill.join("agents/openai.yaml"),
            "policy:\n  allow_implicit_invocation: false\n",
        )
        .unwrap();
        fs::write(
            root.join("skills-lock.json"),
            serde_json::json!({
                "version": 1, "skills": {"import-demo": {
                    "source": "https://example.test/.well-known/skills/import-demo/SKILL.md",
                    "sourceType": "well-known", "wellKnownDigest": digest,
                    "computedHash": "", "skillFolderHash": ""
                }}
            })
            .to_string(),
        )
        .unwrap();
        let store = crate::storage::Store::open(root.join("store.sqlite3")).unwrap();
        let mut scan =
            crate::skills::scan_skills_for_workspace_initialization(&root, &store, &[]).unwrap();
        scan.skills.retain(|skill| {
            skill.name == "import-demo"
                && skill.paths.iter().any(|path| path.path.starts_with(&root))
        });
        assert_eq!(scan.skills.len(), 1);
        Self {
            root,
            store,
            scan,
            digest,
        }
    }

    fn import(&mut self) -> bool {
        super::migrate_scan(&self.store, &self.root, &mut self.scan).unwrap()
    }

    fn record(&self, version: Option<&str>) {
        let path = &self.scan.skills[0].paths[0];
        self.store
            .upsert_skill_source_records_for_workspace(
                &self.root,
                &[crate::skills::SkillSourceRecord {
                    skill_name: "import-demo".to_string(),
                    skill_path: path.path.clone(),
                    source_kind: "well-known".to_string(),
                    source: Some(
                        "https://example.test/.well-known/skills/import-demo/SKILL.md".to_string(),
                    ),
                    source_ref: None,
                    source_version: version.map(str::to_string),
                    source_relative_path: None,
                    update_status: "tracked".to_string(),
                    origin: "projection-scan".to_string(),
                }],
            )
            .unwrap();
        super::super::mark_migration_completed(
            &self.store,
            &super::migration_key_for_workspace(&self.root).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for WellKnownImport {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn well_known_lock_import_captures_only_a_digest_verified_baseline() {
    let mut fixture = WellKnownImport::new();
    assert!(fixture.import());
    let path = &fixture.scan.skills[0].paths[0].path;
    let records = fixture
        .store
        .skill_source_records_for_workspace(&fixture.root)
        .unwrap();
    assert_eq!(
        records[0].source_version.as_deref(),
        Some(fixture.digest.as_str())
    );
    let snapshot = fixture
        .store
        .skill_snapshot_for_workspace(&fixture.root, path)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.source_version, fixture.digest);
    let manifest = snapshot
        .files
        .iter()
        .find(|file| file.relative_path == "SKILL.md")
        .unwrap();
    assert!(
        std::str::from_utf8(&manifest.content)
            .unwrap()
            .contains("disable-model-invocation: true")
    );
    assert!(super::migration_completed_for_workspace(&fixture.store, &fixture.root).unwrap());
    assert!(!fixture.import());
}

#[test]
fn well_known_digest_import_repairs_a_previously_completed_source_import() {
    let mut fixture = WellKnownImport::new();
    fixture.record(None);
    assert!(!super::migration_completed_for_workspace(&fixture.store, &fixture.root).unwrap());
    assert!(fixture.import());
    let path = &fixture.scan.skills[0].paths[0].path;
    let snapshot = fixture
        .store
        .skill_snapshot_for_workspace(&fixture.root, path)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.source_version, fixture.digest);
    assert!(!fixture.import());
}

#[test]
fn well_known_digest_import_preserves_an_already_updated_source() {
    let mut fixture = WellKnownImport::new();
    fixture.record(Some("already-updated-version"));
    assert!(!fixture.import());
    let records = fixture
        .store
        .skill_source_records_for_workspace(&fixture.root)
        .unwrap();
    assert_eq!(
        records[0].source_version.as_deref(),
        Some("already-updated-version")
    );
    assert!(super::migration_completed_for_workspace(&fixture.store, &fixture.root).unwrap());
}

#[test]
fn well_known_digest_import_does_not_adopt_modified_content_as_the_base() {
    let mut fixture = WellKnownImport::new();
    let path = fixture.scan.skills[0].paths[0].path.clone();
    let content = fs::read_to_string(path.join("SKILL.md")).unwrap();
    fs::write(
        path.join("SKILL.md"),
        content.replace("Original", "User edit"),
    )
    .unwrap();
    assert!(fixture.import());
    assert!(
        fixture
            .store
            .skill_snapshot_for_workspace(&fixture.root, &path)
            .unwrap()
            .is_none()
    );
}
