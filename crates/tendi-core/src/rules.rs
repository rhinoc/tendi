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

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
pub struct RuleFileWriteResult {
    pub path: PathBuf,
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
) -> Result<RuleFileWriteResult> {
    save_rule_file_for_project_roots(cwd, path, expected_sha256, content, &[])
}

pub fn save_rule_file_for_project_roots(
    cwd: &Path,
    path: &Path,
    expected_sha256: &str,
    content: &str,
    project_roots: &[PathBuf],
) -> Result<RuleFileWriteResult> {
    ensure_known_rule_for_project_roots(cwd, path, project_roots)?;
    save_rule_file_at_path(path, expected_sha256, content)
}

pub fn save_rule_file_at_path(
    path: &Path,
    expected_sha256: &str,
    content: &str,
) -> Result<RuleFileWriteResult> {
    let _resources = crate::coordination::acquire_file_resources(&[path.to_path_buf()])?;
    {
        let before = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let current_sha = sha256_text(&before);
        if current_sha != expected_sha256 {
            bail!("refusing to overwrite changed file {}", path.display());
        }
    }
    atomic_write(path, content)?;
    Ok(RuleFileWriteResult {
        path: path.to_path_buf(),
        sha256: sha256_text(content),
    })
}

pub fn delete_rule_files(paths: &[PathBuf]) -> Result<()> {
    let _resources = crate::coordination::acquire_file_resources(paths)?;
    for path in paths {
        fs::remove_file(path).with_context(|| format!("failed to delete {}", path.display()))?;
    }
    Ok(())
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
    delete_rule_files(paths)
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
#[path = "rules_tests.rs"]
mod tests;
