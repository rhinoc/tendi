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
