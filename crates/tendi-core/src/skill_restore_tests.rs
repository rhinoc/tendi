use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{apply_project_skill_restore, plan_project_skill_restore};
use crate::{skills::SkillSourceRecord, storage::Store};

fn temp_dir(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn init_source(root: &Path) {
    let skill = root.join("skills/demo");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: demo\ndescription: Demo\n---\n\n# Demo\n",
    )
    .unwrap();
}

#[test]
fn restores_local_project_lock_and_persists_source_without_changing_lock() {
    let root = temp_dir("tendi-project-restore");
    let source = root.join("source");
    fs::create_dir_all(&root).unwrap();
    init_source(&source);
    let lock_path = root.join("skills-lock.json");
    let lock = r#"{
  "version": 1,
  "skills": {
    "demo": {
      "source": "source",
      "sourceType": "local",
      "skillPath": "skills/demo/SKILL.md",
      "computedHash": "content-hash"
    }
  }
}"#;
    fs::write(&lock_path, lock).unwrap();
    let store = Store::open(root.join("tendi.sqlite3")).unwrap();

    let plan = plan_project_skill_restore(&root, &store).unwrap();
    assert_eq!(plan.operations[0].status, "planned");
    assert!(!root.join(".agents/skills/demo").exists());
    let report = apply_project_skill_restore(&plan, &store).unwrap();
    assert_eq!(report.operations[0].status, "restored");
    assert!(root.join(".agents/skills/demo/SKILL.md").is_file());
    let record = store
        .skill_source_record_for_workspace(&plan.project_root, &plan.target_root.join("demo"))
        .unwrap()
        .unwrap();
    assert_eq!(
        record.source_relative_path.as_deref(),
        Some("skills/demo/SKILL.md")
    );
    assert_eq!(record.source_version.as_deref(), Some("content-hash"));
    assert_eq!(fs::read_to_string(lock_path).unwrap(), lock);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn database_record_skips_invalid_lock_entry_without_overwriting_it() {
    let root = temp_dir("tendi-project-restore-database");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("skills-lock.json"),
        r#"{
  "version": 1,
  "skills": {
    "demo": "this entry is intentionally invalid and must not be read"
  }
}"#,
    )
    .unwrap();
    let store = Store::open(root.join("tendi.sqlite3")).unwrap();
    let target = root.join(".agents/skills/demo");
    let database = SkillSourceRecord {
        skill_name: "demo".to_string(),
        skill_path: target.clone(),
        source_kind: "github".to_string(),
        source: Some("https://github.com/database/demo.git".to_string()),
        source_ref: Some("main".to_string()),
        source_version: Some("database".to_string()),
        source_relative_path: Some("skills/demo/SKILL.md".to_string()),
        update_status: "tracked".to_string(),
        origin: "tendi-install".to_string(),
    };
    store
        .upsert_skill_source_records_for_workspace(&root, &[database.clone()])
        .unwrap();

    let plan = plan_project_skill_restore(&root, &store).unwrap();
    assert_eq!(plan.operations[0].status, "skipped-database");
    let persisted = store
        .skill_source_record_for_workspace(&root, &target)
        .unwrap()
        .unwrap();
    assert_eq!(persisted.source, database.source);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_node_modules_and_rejects_ambiguous_gitlab_source() {
    let root = temp_dir("tendi-project-restore-skips");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("skills-lock.json"),
        r#"{
  "version": 1,
  "skills": {
    "from-package": {
      "source": "node_modules/pkg",
      "sourceType": "local",
      "skillPath": "skills/from-package/SKILL.md"
    },
    "gitlab-demo": {
      "source": "group/repo",
      "sourceType": "gitlab",
      "skillPath": "skills/demo/SKILL.md"
    }
  }
}"#,
    )
    .unwrap();
    let store = Store::open(root.join("tendi.sqlite3")).unwrap();

    let plan = plan_project_skill_restore(&root, &store).unwrap();
    assert_eq!(plan.operations[0].status, "skipped-node-modules");
    assert_eq!(plan.operations[1].status, "error");
    assert!(
        plan.operations[1]
            .message
            .as_deref()
            .unwrap()
            .contains("require sourceUrl")
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn finds_project_root_lock_from_nested_cwd() {
    let root = temp_dir("tendi-project-restore-root");
    let nested = root.join("nested/worktree");
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        root.join("skills-lock.json"),
        r#"{"version":1,"skills":{}}"#,
    )
    .unwrap();
    Command::new("git").arg("init").arg(&root).output().unwrap();
    let store = Store::open(root.join("tendi.sqlite3")).unwrap();

    let plan = plan_project_skill_restore(&nested, &store).unwrap();
    assert_eq!(plan.project_root, root);

    fs::remove_dir_all(root).unwrap();
}
