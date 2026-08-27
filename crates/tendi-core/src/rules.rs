use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::{
    fsutil::{atomic_write, sha256_file, sha256_text},
    skills::AgentKind,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RuleRecord {
    pub agents: Vec<AgentKind>,
    pub kind: String,
    pub scope: String,
    pub path: PathBuf,
    pub order: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleScan {
    pub rules: Vec<RuleRecord>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleFileContent {
    pub path: PathBuf,
    pub content: String,
    pub sha256: String,
}

pub fn scan_rules(cwd: &Path) -> Result<RuleScan> {
    scan_rules_for_project_roots(cwd, &[])
}

pub fn scan_rules_for_project_roots(cwd: &Path, project_roots: &[PathBuf]) -> Result<RuleScan> {
    let mut rules = Vec::new();
    let mut warnings = Vec::new();
    let mut order = 0;

    let ctx = crate::providers::ProviderContext::with_additional_project_dirs(cwd, project_roots);
    for provider in crate::providers::agent_providers() {
        provider.scan_rules(&ctx, &mut rules, &mut warnings, &mut order);
    }

    Ok(RuleScan {
        rules: merge_rules_by_path(rules),
        warnings,
    })
}

pub(crate) fn merge_rules_by_path(rules: Vec<RuleRecord>) -> Vec<RuleRecord> {
    let mut merged: Vec<RuleRecord> = Vec::new();
    let mut indexes: BTreeMap<PathBuf, usize> = BTreeMap::new();

    for mut rule in rules {
        rule.agents.sort();
        rule.agents.dedup();
        if let Some(index) = indexes.get(&rule.path).copied() {
            let existing = &mut merged[index];
            existing.agents.extend(rule.agents);
            existing.agents.sort();
            existing.agents.dedup();
        } else {
            indexes.insert(rule.path.clone(), merged.len());
            merged.push(rule);
        }
    }

    merged
}

pub fn read_rule_file(cwd: &Path, path: &Path) -> Result<RuleFileContent> {
    read_rule_file_for_project_roots(cwd, path, &[])
}

pub fn read_rule_file_for_project_roots(
    cwd: &Path,
    path: &Path,
    project_roots: &[PathBuf],
) -> Result<RuleFileContent> {
    ensure_known_rule_for_project_roots(cwd, path, project_roots)?;
    read_rule_file_at_path(path)
}

pub fn read_rule_file_at_path(path: &Path) -> Result<RuleFileContent> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(RuleFileContent {
        path: path.to_path_buf(),
        sha256: sha256_text(&content),
        content,
    })
}

pub fn save_rule_file(
    cwd: &Path,
    path: &Path,
    expected_sha256: &str,
    content: &str,
) -> Result<RuleFileContent> {
    save_rule_file_for_project_roots(cwd, path, expected_sha256, content, &[])
}

pub fn save_rule_file_for_project_roots(
    cwd: &Path,
    path: &Path,
    expected_sha256: &str,
    content: &str,
    project_roots: &[PathBuf],
) -> Result<RuleFileContent> {
    ensure_known_rule_for_project_roots(cwd, path, project_roots)?;
    save_rule_file_at_path(path, expected_sha256, content)
}

pub fn save_rule_file_at_path(
    path: &Path,
    expected_sha256: &str,
    content: &str,
) -> Result<RuleFileContent> {
    let before =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let current_sha = sha256_text(&before);
    if current_sha != expected_sha256 {
        bail!("refusing to overwrite changed file {}", path.display());
    }
    atomic_write(path, content)?;
    Ok(RuleFileContent {
        path: path.to_path_buf(),
        sha256: sha256_text(content),
        content: content.to_string(),
    })
}

pub fn delete_rule_files_for_project_roots(
    cwd: &Path,
    paths: &[PathBuf],
    project_roots: &[PathBuf],
) -> Result<()> {
    let scan = scan_rules_for_project_roots(cwd, project_roots)?;
    for path in paths {
        if !scan.rules.iter().any(|rule| rule.path == *path) {
            bail!("refusing to delete unknown rule {}", path.display());
        }
    }
    for path in paths {
        fs::remove_file(path).with_context(|| format!("failed to delete {}", path.display()))?;
    }
    Ok(())
}

fn ensure_known_rule_for_project_roots(
    cwd: &Path,
    path: &Path,
    project_roots: &[PathBuf],
) -> Result<()> {
    let scan = scan_rules_for_project_roots(cwd, project_roots)?;
    if scan.rules.iter().any(|rule| rule.path == path) {
        return Ok(());
    }
    bail!("refusing to edit unknown rule {}", path.display())
}

pub(crate) fn add_rule_file(
    rules: &mut Vec<RuleRecord>,
    warnings: &mut Vec<String>,
    order: &mut usize,
    agent: AgentKind,
    kind: &str,
    scope: &str,
    path: PathBuf,
) {
    if !path.is_file() {
        return;
    }

    match sha256_file(&path) {
        Ok(sha256) => {
            rules.push(RuleRecord {
                agents: vec![agent],
                kind: kind.to_string(),
                scope: scope.to_string(),
                path,
                order: *order,
                sha256,
            });
            *order += 1;
        }
        Err(err) => warnings.push(format!("{}: {err:#}", path.display())),
    }
}

pub(crate) fn add_first_rule_file(
    rules: &mut Vec<RuleRecord>,
    warnings: &mut Vec<String>,
    order: &mut usize,
    agent: AgentKind,
    scope: &str,
    candidates: Vec<(String, PathBuf)>,
) {
    for (kind, path) in candidates {
        if !path.is_file() {
            continue;
        }
        add_rule_file(rules, warnings, order, agent, &kind, scope, path);
        return;
    }
}

pub(crate) fn add_rule_tree(
    rules: &mut Vec<RuleRecord>,
    warnings: &mut Vec<String>,
    order: &mut usize,
    agent: AgentKind,
    kind: &str,
    scope: &str,
    root: &Path,
    extension: Option<&str>,
    max_depth: usize,
) {
    if !root.is_dir() {
        return;
    }

    let mut files = WalkDir::new(root)
        .follow_links(true)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            extension.is_none_or(|extension| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|value| value == extension)
            })
        })
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    files.sort();

    for path in files {
        add_rule_file(rules, warnings, order, agent, kind, scope, path);
    }
}

#[cfg(test)]
mod tests {
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
                    vec![AgentKind::Cursor],
                    "cursor-rule",
                    "project",
                    "repo/.cursor/rules/project.mdc".to_string()
                ),
                (
                    vec![AgentKind::Claude],
                    "CLAUDE.md",
                    "project",
                    "repo/CLAUDE.md".to_string()
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
        assert!(super::delete_rule_files_for_project_roots(&project, &[project.join("other.md")], &[]).is_err());

        let _ = fs::remove_dir_all(root);
    }
}
