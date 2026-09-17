use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{scan_rules, scan_rules_for_project_roots};
use crate::skills::AgentKind;

#[test]
fn scans_provider_declared_project_rules() {
    let root = std::env::temp_dir().join(format!(
        "tendi-rules-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    ));
    let nested = root.join("repo/crate/src");
    fs::create_dir_all(&nested).expect("create nested cwd");
    fs::create_dir_all(root.join("repo/.git")).expect("create git root");
    fs::write(root.join("repo/AGENTS.md"), "root agents").expect("write AGENTS");
    fs::write(root.join("repo/AGENTS.override.md"), "override agents")
        .expect("write override AGENTS");
    fs::create_dir_all(root.join("repo/.codex/rules")).expect("create codex rules");
    fs::write(
        root.join("repo/.codex/config.toml"),
        "project_doc_fallback_filenames = [\"TEAM_GUIDE.md\"]",
    )
    .expect("write codex config");
    fs::write(
        root.join("repo/.codex/rules/default.rules"),
        "prefix_rule(pattern=[\"gh\"])",
    )
    .expect("write codex policy rule");
    fs::write(root.join("repo/crate/TEAM_GUIDE.md"), "crate fallback")
        .expect("write fallback AGENTS");
    fs::write(root.join("repo/CLAUDE.md"), "root claude").expect("write CLAUDE");
    fs::write(root.join("repo/CLAUDE.local.md"), "local claude").expect("write local CLAUDE");
    fs::create_dir_all(root.join("repo/.claude/rules")).expect("create claude rules");
    fs::write(root.join("repo/.claude/CLAUDE.md"), "dot claude").expect("write .claude CLAUDE");
    fs::write(root.join("repo/.claude/rules/testing.md"), "claude rule")
        .expect("write claude rule");
    fs::create_dir_all(root.join("repo/.cursor/rules")).expect("create cursor rules");
    fs::write(root.join("repo/.cursor/rules/project.mdc"), "cursor rule")
        .expect("write cursor rule");
    fs::write(
        root.join("repo/.cursor/rules/ignored.md"),
        "ignored cursor markdown",
    )
    .expect("write ignored cursor rule");

    let scan = scan_rules(&nested).expect("scan rules");
    let project_rules = scan
        .rules
        .iter()
        .filter(|rule| rule.path.starts_with(&root))
        .map(|rule| {
            (
                rule.agents.clone(),
                rule.kind.as_str(),
                rule.scope.as_str(),
                rule.path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .to_string(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        project_rules,
        vec![
            (
                vec![AgentKind::Codex],
                "AGENTS.override.md",
                "project",
                "repo/AGENTS.override.md".to_string()
            ),
            (
                vec![AgentKind::Codex],
                "TEAM_GUIDE.md",
                "project",
                "repo/crate/TEAM_GUIDE.md".to_string()
            ),
            (
                vec![AgentKind::Cursor],
                "AGENTS.md",
                "project",
                "repo/AGENTS.md".to_string()
            ),
            (
                vec![AgentKind::Cursor, AgentKind::Claude],
                "CLAUDE.md",
                "project",
                "repo/CLAUDE.md".to_string()
            ),
            (
                vec![AgentKind::Cursor],
                "cursor-rule",
                "project",
                "repo/.cursor/rules/project.mdc".to_string()
            ),
            (
                vec![AgentKind::Claude],
                ".claude/CLAUDE.md",
                "project",
                "repo/.claude/CLAUDE.md".to_string()
            ),
            (
                vec![AgentKind::Claude],
                "CLAUDE.local.md",
                "local",
                "repo/CLAUDE.local.md".to_string()
            ),
            (
                vec![AgentKind::Claude],
                "claude-rule",
                "project",
                "repo/.claude/rules/testing.md".to_string()
            ),
        ]
    );
    assert!(
        !project_rules
            .iter()
            .any(|(_, _, _, path)| path == "repo/.codex/rules/default.rules"),
        "Codex exec-policy .rules files are not prompt rules"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scans_rules_from_additional_project_roots() {
    let root = std::env::temp_dir().join(format!(
        "tendi-rules-additional-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    ));
    let cwd = root.join("cwd");
    let project = root.join("project");
    fs::create_dir_all(&cwd).expect("create cwd");
    fs::create_dir_all(project.join(".git")).expect("create git root");
    fs::write(project.join("AGENTS.md"), "project rule").expect("write project rule");
    let project_root = project.canonicalize().expect("canonicalize project root");

    let scan = scan_rules_for_project_roots(&cwd, std::slice::from_ref(&project_root))
        .expect("scan additional project rules");

    assert!(
        scan.rules
            .iter()
            .any(|rule| rule.path == project_root.join("AGENTS.md")),
        "rules: {:#?}",
        scan.rules
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scans_same_rule_once_with_all_applicable_agents() {
    let root = std::env::temp_dir().join(format!(
        "tendi-rules-shared-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    ));
    let project = root.join("repo");
    fs::create_dir_all(project.join(".git")).expect("create git root");
    fs::write(project.join("AGENTS.md"), "shared agents").expect("write AGENTS");

    let scan = scan_rules(&project).expect("scan rules");
    let project_rules = scan
        .rules
        .iter()
        .filter(|rule| rule.path == project.join("AGENTS.md"))
        .collect::<Vec<_>>();

    assert_eq!(project_rules.len(), 1);
    assert_eq!(
        project_rules[0].agents,
        vec![AgentKind::Codex, AgentKind::Cursor]
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn deletes_only_scanned_rules() {
    let root = std::env::temp_dir().join(format!(
        "tendi-rules-delete-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    ));
    let project = root.join("repo");
    fs::create_dir_all(project.join(".git")).expect("create git root");
    let rule_path = project.join("AGENTS.md");
    fs::write(&rule_path, "delete me").expect("write rule");

    let scan = scan_rules(&project).expect("scan rules");
    let rule_path = scan
        .rules
        .iter()
        .find(|rule| rule.path == rule_path)
        .map(|rule| rule.path.clone())
        .expect("rule should be scanned");
    super::delete_rule_files_for_project_roots(&project, std::slice::from_ref(&rule_path), &[])
        .expect("known rule should be deleted");
    assert!(!rule_path.exists());
    assert!(
        super::delete_rule_files_for_project_roots(&project, &[project.join("other.md")], &[])
            .is_err()
    );

    let _ = fs::remove_dir_all(root);
}
