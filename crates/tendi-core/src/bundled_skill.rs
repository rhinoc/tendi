use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::{
    fsutil::{atomic_write, sha256_text},
    skills::{AgentKind, ChangeSet, FileChange, apply_changes, global_agent_skill_root},
    storage::default_db_path,
};

const SKILL_NAME: &str = "tendi";
const SKILL_MARKDOWN: &str = include_str!("../../../skills/tendi/SKILL.md");
const OPENAI_YAML: &str = include_str!("../../../skills/tendi/agents/openai.yaml");
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

pub fn status(agent: AgentKind) -> Result<BundledSkillStatus> {
    let target = global_agent_skill_root(agent)?.join(SKILL_NAME);
    status_at(&target, &prompt_marker_path()?)
}

pub fn plan_install(agent: AgentKind) -> Result<BundledSkillInstallPlan> {
    let target = global_agent_skill_root(agent)?.join(SKILL_NAME);
    plan_install_at(&target)
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
        status: status_at(&plan.target, &marker)?,
        plan,
    })
}

pub fn dismiss_prompt() -> Result<()> {
    mark_prompt_handled_at(&prompt_marker_path()?)
}

fn prompt_marker_path() -> Result<PathBuf> {
    let db = default_db_path()?;
    let parent = db.parent().context("Tendi data directory is unavailable")?;
    Ok(parent.join(PROMPT_MARKER))
}

fn status_at(target: &Path, marker: &Path) -> Result<BundledSkillStatus> {
    let skill = read_optional(&target.join("SKILL.md"))?;
    let openai = read_optional(&target.join("agents/openai.yaml"))?;
    let installed = skill.is_some() || openai.is_some();
    let current =
        skill.as_deref() == Some(SKILL_MARKDOWN) && openai.as_deref() == Some(OPENAI_YAML);
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

fn plan_install_at(target: &Path) -> Result<BundledSkillInstallPlan> {
    let desired = [
        (target.join("SKILL.md"), SKILL_MARKDOWN),
        (target.join("agents/openai.yaml"), OPENAI_YAML),
    ];
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
        GUIDE_MARKDOWN, OPENAI_YAML, SKILL_MARKDOWN, mark_prompt_handled_at, plan_install_at,
        status_at,
    };
    use crate::skills::apply_changes;

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
        let plan = plan_install_at(&target).unwrap();
        assert_eq!(plan.action, "install");
        assert!(!plan.requires_overwrite);
        apply_changes(&plan.changes).unwrap();
        mark_prompt_handled_at(&marker).unwrap();

        let status = status_at(&target, &marker).unwrap();
        assert!(status.installed);
        assert!(status.current);
        assert!(status.prompt_handled);
        assert!(!status.should_prompt);
        assert_eq!(
            fs::read_to_string(target.join("SKILL.md")).unwrap(),
            SKILL_MARKDOWN
        );
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

        let plan = plan_install_at(&target).unwrap();
        assert_eq!(plan.action, "replace");
        assert!(plan.requires_overwrite);
        let status = status_at(&target, &root.join("prompt")).unwrap();
        assert!(status.installed);
        assert!(!status.current);
        assert!(!status.should_prompt);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discovery_stub_matches_the_versioned_guide() {
        fn frontmatter(text: &str) -> &str {
            let rest = text.strip_prefix("---\n").unwrap();
            let end = 4 + rest.find("\n---\n").unwrap() + 5;
            &text[..end]
        }

        assert_eq!(frontmatter(SKILL_MARKDOWN), frontmatter(GUIDE_MARKDOWN));
        assert!(SKILL_MARKDOWN.len() < GUIDE_MARKDOWN.len());
        assert!(SKILL_MARKDOWN.contains("TENDI skills guide"));
        for command in [
            "tendi sessions search",
            "tendi sessions transcript",
            "tendi skills list",
            "tendi skills add",
            "tendi setup skills",
        ] {
            assert!(GUIDE_MARKDOWN.contains(command), "missing {command}");
        }
    }
}
