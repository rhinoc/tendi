use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::{
    fsutil::{atomic_write, sha256_text},
    providers::agent_provider,
    skills::{AgentKind, ChangeSet, FileChange, apply_changes, global_agent_skill_root},
    storage::default_db_path,
};

const SKILL_NAME: &str = "tendi";
pub const INSTALL_SOURCE: &str = "tendi://bundled";
const SKILL_MARKDOWN: &str = include_str!("../../../skills/tendi/SKILL.md");
const GUIDE_MARKDOWN: &str = include_str!("../../../skill-guides/tendi.md");
const PROMPT_MARKER: &str = "bundled-skill-prompt-v1";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundledSkillStatus {
    pub name: &'static str,
    pub target: PathBuf,
    pub installed: bool,
    pub current: bool,
    pub prompt_handled: bool,
    pub should_prompt: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundledSkillInstallPlan {
    pub name: &'static str,
    pub target: PathBuf,
    pub action: &'static str,
    pub requires_overwrite: bool,
    pub changes: ChangeSet,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundledSkillInstallReport {
    pub plan: BundledSkillInstallPlan,
    pub applied: bool,
    pub status: BundledSkillStatus,
}

pub fn guide_markdown() -> &'static str {
    GUIDE_MARKDOWN
}

pub fn install_source_path() -> Result<PathBuf> {
    let db = default_db_path()?;
    let parent = db.parent().context("Tendi data directory is unavailable")?;
    let source = parent.join("sources/tendi-bundled");
    for (relative, content) in desired_files(AgentKind::Shared) {
        let path = source.join(relative);
        if read_optional(&path)?.is_none() {
            atomic_write(&path, content)?;
        }
    }
    Ok(source)
}

pub fn status(agent: AgentKind) -> Result<BundledSkillStatus> {
    let target = global_agent_skill_root(agent)?.join(SKILL_NAME);
    status_at(&target, &prompt_marker_path()?, agent)
}

pub fn plan_install(agent: AgentKind) -> Result<BundledSkillInstallPlan> {
    let target = global_agent_skill_root(agent)?.join(SKILL_NAME);
    plan_install_at(&target, agent)
}

pub fn install(
    agent: AgentKind,
    overwrite: bool,
    dry_run: bool,
) -> Result<BundledSkillInstallReport> {
    let marker = prompt_marker_path()?;
    let plan = plan_install(agent)?;
    if plan.requires_overwrite && !overwrite {
        bail!(
            "{} contains different content; inspect it and rerun with --overwrite to replace the bundled files",
            plan.target.display()
        );
    }
    if !dry_run {
        apply_changes(&plan.changes)?;
        mark_prompt_handled_at(&marker)?;
    }
    Ok(BundledSkillInstallReport {
        applied: !dry_run && !plan.changes.changes.is_empty(),
        status: status_at(&plan.target, &marker, agent)?,
        plan,
    })
}

pub fn remove(agent: AgentKind) -> Result<BundledSkillStatus> {
    let target = global_agent_skill_root(agent)?.join(SKILL_NAME);
    let marker = prompt_marker_path()?;
    remove_at(&target, agent)?;
    status_at(&target, &marker, agent)
}

pub fn dismiss_prompt() -> Result<()> {
    mark_prompt_handled_at(&prompt_marker_path()?)
}

fn prompt_marker_path() -> Result<PathBuf> {
    let db = default_db_path()?;
    let parent = db.parent().context("Tendi data directory is unavailable")?;
    Ok(parent.join(PROMPT_MARKER))
}

fn desired_files(agent: AgentKind) -> Vec<(PathBuf, &'static str)> {
    let mut files = vec![(PathBuf::from("SKILL.md"), SKILL_MARKDOWN)];
    files.extend(
        agent_provider(agent)
            .bundled_skill_files()
            .iter()
            .map(|(path, content)| (PathBuf::from(path), *content)),
    );
    files
}

fn status_at(target: &Path, marker: &Path, agent: AgentKind) -> Result<BundledSkillStatus> {
    let states = desired_files(agent)
        .iter()
        .map(|(relative, expected)| {
            read_optional(&target.join(relative)).map(|actual| (actual, *expected))
        })
        .collect::<Result<Vec<_>>>()?;
    let installed = states.iter().any(|(actual, _)| actual.is_some());
    let current = states
        .iter()
        .all(|(actual, expected)| actual.as_deref() == Some(*expected));
    let prompt_handled = marker.is_file();
    Ok(BundledSkillStatus {
        name: SKILL_NAME,
        target: target.to_path_buf(),
        installed,
        current,
        prompt_handled,
        should_prompt: !installed && !prompt_handled,
    })
}

fn plan_install_at(target: &Path, agent: AgentKind) -> Result<BundledSkillInstallPlan> {
    let desired = desired_files(agent)
        .into_iter()
        .map(|(relative, content)| (target.join(relative), content))
        .collect::<Vec<_>>();
    let mut changes = Vec::new();
    let mut requires_overwrite = false;
    let mut target_exists = false;
    for (path, after) in desired {
        let before = read_optional(&path)?;
        if before.as_deref() == Some(after) {
            continue;
        }
        target_exists |= before.is_some();
        requires_overwrite |= before.is_some();
        changes.push(FileChange {
            before_sha256: before.as_deref().map(sha256_text),
            before,
            path,
            after: after.to_string(),
        });
    }
    let action = if changes.is_empty() {
        "none"
    } else if requires_overwrite {
        "replace"
    } else if target_exists || target.exists() {
        "complete"
    } else {
        "install"
    };
    Ok(BundledSkillInstallPlan {
        name: SKILL_NAME,
        target: target.to_path_buf(),
        action,
        requires_overwrite,
        changes: ChangeSet { changes },
    })
}

fn remove_at(target: &Path, agent: AgentKind) -> Result<()> {
    let desired = desired_files(agent)
        .into_iter()
        .map(|(relative, content)| (target.join(relative), content))
        .collect::<Vec<_>>();
    for (path, expected) in &desired {
        if let Some(before) = read_optional(path)? {
            if before != *expected {
                bail!(
                    "{} contains different content; refusing to remove it",
                    path.display()
                );
            }
        }
    }
    for (path, _) in desired {
        if read_optional(&path)?.is_some() {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
    }
    for directory in [target.join("agents"), target.to_path_buf()] {
        match fs::remove_dir(&directory) {
            Ok(()) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
            Err(error) => {
                return Err(error).with_context(|| format!("remove {}", directory.display()));
            }
        }
    }
    Ok(())
}

fn mark_prompt_handled_at(path: &Path) -> Result<()> {
    atomic_write(path, "handled\n")
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        SKILL_MARKDOWN, mark_prompt_handled_at, plan_install_at, remove_at,
        status_at,
    };
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

}
