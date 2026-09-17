use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{SKILL_MARKDOWN, mark_prompt_handled_at, plan_install_at, remove_at, status_at};
use crate::skills::{AgentKind, apply_changes};

const OPENAI_YAML: &str = include_str!("../../../skills/tendi/agents/openai.yaml");

fn temp_dir(name: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("tendi-bundled-skill-{name}-{nonce}"));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn installs_bundled_files_and_reports_current() {
    let root = temp_dir("install");
    let target = root.join("skills/tendi");
    let marker = root.join("prompt");
    let plan = plan_install_at(&target, AgentKind::Shared).unwrap();
    assert_eq!(plan.action, "install");
    assert!(!plan.requires_overwrite);
    apply_changes(&plan.changes).unwrap();
    mark_prompt_handled_at(&marker).unwrap();

    let status = status_at(&target, &marker, AgentKind::Shared).unwrap();
    assert!(status.installed);
    assert!(status.current);
    assert!(status.prompt_handled);
    assert!(!status.should_prompt);
    assert_eq!(
        fs::read_to_string(target.join("SKILL.md")).unwrap(),
        SKILL_MARKDOWN
    );
    assert!(!target.join("agents/openai.yaml").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn codex_bundle_includes_provider_metadata() {
    let root = temp_dir("codex-metadata");
    let target = root.join("skills/tendi");
    let plan = plan_install_at(&target, AgentKind::Codex).unwrap();

    assert!(
        plan.changes
            .changes
            .iter()
            .any(|change| change.path.ends_with("agents/openai.yaml"))
    );
    apply_changes(&plan.changes).unwrap();
    assert_eq!(
        fs::read_to_string(target.join("agents/openai.yaml")).unwrap(),
        OPENAI_YAML
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn refuses_to_treat_different_skill_as_current() {
    let root = temp_dir("conflict");
    let target = root.join("skills/tendi");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("SKILL.md"), "user content\n").unwrap();

    let plan = plan_install_at(&target, AgentKind::Shared).unwrap();
    assert_eq!(plan.action, "replace");
    assert!(plan.requires_overwrite);
    let status = status_at(&target, &root.join("prompt"), AgentKind::Shared).unwrap();
    assert!(status.installed);
    assert!(!status.current);
    assert!(!status.should_prompt);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn removes_only_unchanged_bundled_files() {
    let root = temp_dir("remove");
    let target = root.join("skills/tendi");
    let marker = root.join("prompt");
    let plan = plan_install_at(&target, AgentKind::Shared).unwrap();
    apply_changes(&plan.changes).unwrap();
    mark_prompt_handled_at(&marker).unwrap();

    remove_at(&target, AgentKind::Shared).unwrap();

    let status = status_at(&target, &marker, AgentKind::Shared).unwrap();
    assert!(!status.installed);
    assert!(!status.current);
    assert!(!target.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn refuses_to_remove_changed_bundled_files() {
    let root = temp_dir("remove-conflict");
    let target = root.join("skills/tendi");
    fs::create_dir_all(target.join("agents")).unwrap();
    fs::write(target.join("SKILL.md"), "user content\n").unwrap();
    fs::write(target.join("agents/openai.yaml"), OPENAI_YAML).unwrap();

    let error = remove_at(&target, AgentKind::Codex)
        .unwrap_err()
        .to_string();

    assert!(error.contains("refusing to remove"));
    assert_eq!(
        fs::read_to_string(target.join("SKILL.md")).unwrap(),
        "user content\n"
    );
    fs::remove_dir_all(root).unwrap();
}
