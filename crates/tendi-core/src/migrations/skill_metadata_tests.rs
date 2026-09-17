use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use crate::skills::{AgentKind, SkillPath, SkillRecord, SkillRoot};

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "tendi-skill-visibility-migration-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn scan_for(skill_dir: &Path) -> SkillScan {
    SkillScan {
        roots: vec![SkillRoot {
            path: skill_dir.parent().unwrap().to_path_buf(),
            scope: "global".to_string(),
            agent: AgentKind::Shared,
            plugin_id: None,
            plugin_enabled: None,
        }],
        skills: vec![SkillRecord {
            id: "skill@test".to_string(),
            installation_id: "skill@test".to_string(),
            name: "demo".to_string(),
            description: Some("Demo".to_string()),
            tags: Vec::new(),
            dependencies: Vec::new(),
            dependents: Vec::new(),
            dependency_ids: Vec::new(),
            dependent_ids: Vec::new(),
            is_wrapper: false,
            visibility: SkillVisibility::Manual,
            agents: vec![AgentKind::Shared],
            paths: vec![SkillPath {
                path: skill_dir.to_path_buf(),
                root: skill_dir.parent().unwrap().to_path_buf(),
                scope: "global".to_string(),
                agent: AgentKind::Shared,
                install_target: "shared".to_string(),
                source_kind: "local".to_string(),
                source: None,
                source_ref: None,
                source_version: None,
                source_relative_path: None,
                symlink_status: "direct".to_string(),
                update_status: "local".to_string(),
                sha256: "sha".to_string(),
                tags: Vec::new(),
                tendi_visibility: None,
                effective_visibility: SkillVisibility::Auto,
                provider_allow_implicit_invocation: None,
                provider_skill_enabled: None,
                provider_disable_model_invocation: None,
                plugin_id: None,
                plugin_enabled: None,
            }],
            source_summary: "local".to_string(),
            install_targets: vec!["shared".to_string()],
            update_status: "local".to_string(),
            is_system: false,
            ctime: None,
            mtime: None,
        }],
        warnings: Vec::new(),
    }
}

#[test]
fn migrates_legacy_visibility_once_into_database() {
    let root = temp_dir();
    let skill_dir = root.join(".agents/skills/demo");
    fs::create_dir_all(skill_dir.join("agents")).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: demo\ntendi:\n  visibility: off\n---\n\n# Demo\n",
    )
    .unwrap();
    fs::write(
        skill_dir.join(LEGACY_RELATIVE_PATH),
        "schema_version: 1\nvisibility: manual\n",
    )
    .unwrap();

    let store = Store::open(root.join("tendi.sqlite3")).unwrap();
    let scan = scan_for(&skill_dir);
    assert!(migrate_scan(&store, &root, &scan).unwrap());
    assert!(!skill_dir.join(LEGACY_RELATIVE_PATH).exists());
    assert!(
        !fs::read_to_string(skill_dir.join("SKILL.md"))
            .unwrap()
            .contains("tendi:")
    );
    assert_eq!(
        store
            .skill_visibilities_for_workspace(&root)
            .unwrap()
            .get(&skill_dir.canonicalize().unwrap())
            .copied(),
        Some(SkillVisibility::Manual)
    );
    assert!(!migrate_scan(&store, &root, &scan).unwrap());

    fs::remove_dir_all(root).unwrap();
}
