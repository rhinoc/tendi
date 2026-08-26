use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    SkillInstallScope, SkillTarget,
    skills::{
        SkillAddOptions, SkillAddPlan, SkillSourceRecord, SkillVisibility, apply_skill_add_preview,
        plan_skill_add,
    },
    storage::Store,
};

#[derive(Debug, Clone, Serialize)]
pub struct SkillRestorePlan {
    pub lock_path: PathBuf,
    pub project_root: PathBuf,
    pub target_root: PathBuf,
    pub operations: Vec<SkillRestoreOperation>,
    #[serde(skip)]
    executable: Vec<RestoreExecutable>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillRestoreReport {
    pub lock_path: PathBuf,
    pub project_root: PathBuf,
    pub target_root: PathBuf,
    pub operations: Vec<SkillRestoreOperation>,
}

#[derive(Debug, Clone)]
pub struct SkillRestoreApplyResult {
    pub report: SkillRestoreReport,
    pub source_records: Vec<SkillSourceRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillRestoreOperation {
    pub name: String,
    pub target: PathBuf,
    pub status: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
struct RestoreExecutable {
    operation_index: usize,
    options: SkillAddOptions,
    add_plan: SkillAddPlan,
    source_record: SkillSourceRecord,
}

#[derive(Debug, Deserialize)]
struct ProjectLock {
    version: u64,
    #[serde(default)]
    skills: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectLockEntry {
    source: String,
    source_type: String,
    source_url: Option<String>,
    r#ref: Option<String>,
    skill_path: Option<String>,
    skill_folder_hash: Option<String>,
    computed_hash: Option<String>,
}

pub fn plan_project_skill_restore(cwd: &Path, store: &Store) -> Result<SkillRestorePlan> {
    let (project_root, lock_path) = project_lock_path(cwd)?;
    let text = fs::read_to_string(&lock_path)
        .with_context(|| format!("failed to read {}", lock_path.display()))?;
    let lock: ProjectLock = serde_json::from_str(&text)
        .with_context(|| format!("invalid skills lock {}", lock_path.display()))?;
    if lock.version != 1 {
        bail!(
            "unsupported skills lock version {} in {}; expected 1",
            lock.version,
            lock_path.display()
        );
    }

    let target: SkillTarget = "universal".parse()?;
    let target_root = crate::skill_targets::skill_target_root(
        &project_root,
        &target,
        SkillInstallScope::Project,
    )?;
    let mut operations = Vec::new();
    let mut executable = Vec::new();

    for (name, entry_value) in lock.skills {
        let target_path = target_root.join(sanitize_restore_name(&name)?);
        if store.skill_source_record(&target_path)?.is_some() {
            operations.push(SkillRestoreOperation {
                name,
                target: target_path,
                status: "skipped-database".to_string(),
                message: Some("skill source database is authoritative".to_string()),
            });
            continue;
        }
        let entry = match serde_json::from_value::<ProjectLockEntry>(entry_value) {
            Ok(entry) => entry,
            Err(error) => {
                operations.push(SkillRestoreOperation {
                    name,
                    target: target_path,
                    status: "error".to_string(),
                    message: Some(format!("invalid lock entry: {error}")),
                });
                continue;
            }
        };
        if is_node_modules_entry(&entry) {
            operations.push(SkillRestoreOperation {
                name,
                target: target_path,
                status: "skipped-node-modules".to_string(),
                message: Some("node_modules restore is not supported".to_string()),
            });
            continue;
        }

        let operation_index = operations.len();
        match plan_entry_restore(&project_root, &target_path, &name, &entry, target.clone()) {
            Ok((options, add_plan, source_record)) => {
                let operation = &add_plan.operations[0];
                let (status, message) = if operation.status == "planned" {
                    ("planned".to_string(), None)
                } else {
                    (
                        format!("skipped-{}", operation.status),
                        operation.message.clone(),
                    )
                };
                operations.push(SkillRestoreOperation {
                    name,
                    target: target_path,
                    status,
                    message,
                });
                if operation.status == "planned" {
                    executable.push(RestoreExecutable {
                        operation_index,
                        options,
                        add_plan,
                        source_record,
                    });
                }
            }
            Err(error) => operations.push(SkillRestoreOperation {
                name,
                target: target_path,
                status: "error".to_string(),
                message: Some(format!("{error:#}")),
            }),
        }
    }

    Ok(SkillRestorePlan {
        lock_path,
        project_root,
        target_root,
        operations,
        executable,
    })
}

pub fn apply_project_skill_restore(
    plan: &SkillRestorePlan,
    store: &Store,
) -> Result<SkillRestoreReport> {
    let result = apply_project_skill_restore_without_database(plan)?;
    store.upsert_skill_source_records(&result.source_records)?;
    Ok(result.report)
}

pub fn apply_project_skill_restore_without_database(
    plan: &SkillRestorePlan,
) -> Result<SkillRestoreApplyResult> {
    let mut operations = plan.operations.clone();
    let mut source_records = Vec::new();
    for executable in &plan.executable {
        let operation = &mut operations[executable.operation_index];
        match apply_skill_add_preview(&executable.add_plan, &executable.options) {
            Ok(report) => {
                let Some(result) = report.results.first() else {
                    operation.status = "error".to_string();
                    operation.message = Some("restore produced no installation result".to_string());
                    continue;
                };
                let mut source_record = executable.source_record.clone();
                source_record.skill_path = result.target.clone();
                source_records.push(source_record);
                operation.status = "restored".to_string();
                operation.message = None;
            }
            Err(error) => {
                operation.status = "error".to_string();
                operation.message = Some(format!("{error:#}"));
            }
        }
    }
    Ok(SkillRestoreApplyResult {
        report: SkillRestoreReport {
            lock_path: plan.lock_path.clone(),
            project_root: plan.project_root.clone(),
            target_root: plan.target_root.clone(),
            operations,
        },
        source_records,
    })
}

fn plan_entry_restore(
    project_root: &Path,
    target_path: &Path,
    name: &str,
    entry: &ProjectLockEntry,
    target: SkillTarget,
) -> Result<(SkillAddOptions, SkillAddPlan, SkillSourceRecord)> {
    let source_input = restore_source_input(project_root, entry)?;
    let expected_relative_path = normalized_skill_dir(
        entry
            .skill_path
            .as_deref()
            .context("lock entry is missing skillPath; refusing an ambiguous restore")?,
    )?;
    let mut options = SkillAddOptions {
        source: source_input,
        target,
        scope: SkillInstallScope::Project,
        skills: vec![name.to_string()],
        copy: true,
        overwrite: false,
        visibility: SkillVisibility::Auto,
    };
    let add_plan = plan_skill_add(project_root, &options)?;
    options.source = add_plan.source.clone();
    if add_plan.selected.len() != 1 || add_plan.operations.len() != 1 {
        bail!("lock entry did not resolve to exactly one skill named {name}");
    }
    let selected_relative = normalized_skill_dir(&add_plan.selected[0].relative_path)?;
    if selected_relative != expected_relative_path {
        bail!(
            "lock skillPath {} resolved to {}, refusing a different skill with the same name",
            expected_relative_path.display(),
            selected_relative.display()
        );
    }
    if add_plan.operations[0].target != target_path {
        bail!("restore target did not match the project shared skill path");
    }

    let source_kind = add_plan.source_kind.clone();
    let source = add_plan.source.clone();
    let source_version = entry
        .skill_folder_hash
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            entry
                .computed_hash
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        })
        .map(str::to_string);
    let source_record = SkillSourceRecord {
        skill_name: name.to_string(),
        skill_path: target_path.to_path_buf(),
        source_kind,
        source: Some(source),
        source_ref: add_plan.source_ref.clone(),
        source_version,
        source_relative_path: entry.skill_path.clone(),
        update_status: "tracked".to_string(),
        origin: "skills-cli-restore".to_string(),
    };
    Ok((options, add_plan, source_record))
}

fn restore_source_input(project_root: &Path, entry: &ProjectLockEntry) -> Result<String> {
    let source_kind = entry.source_type.trim().to_ascii_lowercase();
    if matches!(source_kind.as_str(), "git" | "gitlab")
        && entry
            .source_url
            .as_deref()
            .is_none_or(|source| source.trim().is_empty())
    {
        bail!("{source_kind} lock entries require sourceUrl; shorthand is ambiguous");
    }
    let source = entry
        .source_url
        .as_deref()
        .filter(|source| !source.trim().is_empty())
        .unwrap_or(entry.source.as_str())
        .trim();
    if source.is_empty() {
        bail!("lock entry source is empty");
    }
    let source = if source_kind == "local" {
        let path = PathBuf::from(source);
        if path.is_absolute() {
            path
        } else {
            project_root.join(path)
        }
        .display()
        .to_string()
    } else {
        source.to_string()
    };
    match entry
        .r#ref
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(git_ref) if source_kind != "local" => Ok(format!("{source}#{git_ref}")),
        _ => Ok(source),
    }
}

fn project_lock_path(cwd: &Path) -> Result<(PathBuf, PathBuf)> {
    let cwd = cwd.to_path_buf();
    let direct = cwd.join("skills-lock.json");
    if direct.is_file() {
        return Ok((cwd, direct));
    }
    let project_root = cwd
        .ancestors()
        .find(|path| path.join(".git").exists())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cwd.clone());
    let lock_path = project_root.join("skills-lock.json");
    if !lock_path.is_file() {
        bail!("skills-lock.json not found in {}", project_root.display());
    }
    Ok((project_root, lock_path))
}

fn normalized_skill_dir(value: &str) -> Result<PathBuf> {
    let path = Path::new(value.trim());
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!("unsafe skillPath {value:?}");
    }
    if value.contains('\\') {
        bail!("unsafe skillPath {value:?}");
    }
    let path = if path.file_name().is_some_and(|name| name == "SKILL.md") {
        path.parent().unwrap_or(Path::new("."))
    } else {
        path
    };
    Ok(path.to_path_buf())
}

fn sanitize_restore_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains(['/', '\\'])
        || trimmed.chars().any(char::is_control)
    {
        bail!("unsafe skill name {name:?}");
    }
    let sanitized = trimmed
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .collect::<Vec<_>>()
        .join("-")
        .trim_matches(['.', '-'])
        .chars()
        .take(255)
        .collect::<String>();
    if sanitized.is_empty() {
        bail!("unsafe skill name {name:?}");
    }
    Ok(sanitized)
}

fn is_node_modules_entry(entry: &ProjectLockEntry) -> bool {
    entry.source_type.eq_ignore_ascii_case("node_modules")
        || [&entry.source, entry.source_url.as_deref().unwrap_or("")]
            .into_iter()
            .chain(entry.skill_path.as_deref())
            .any(|value| {
                value
                    .replace('\\', "/")
                    .split('/')
                    .any(|part| part == "node_modules")
            })
}

#[cfg(test)]
mod tests {
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
            .skill_source_record(&plan.target_root.join("demo"))
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
            .upsert_skill_source_records(&[database.clone()])
            .unwrap();

        let plan = plan_project_skill_restore(&root, &store).unwrap();
        assert_eq!(plan.operations[0].status, "skipped-database");
        let persisted = store.skill_source_record(&target).unwrap().unwrap();
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
}
