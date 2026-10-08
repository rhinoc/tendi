//! Skills CLI lock file integration for skills installed by Tendi.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::{
    fsutil::atomic_write,
    skill_targets::SkillInstallScope,
    skills::{SkillAddApplyReport, is_git_source_kind},
};

const GLOBAL_VERSION: u64 = 3;
const PROJECT_VERSION: u64 = 1;

pub fn lock_path(scope: SkillInstallScope, cwd: &Path) -> Result<PathBuf> {
    match scope {
        SkillInstallScope::Global => global_lock_path(),
        SkillInstallScope::Project => Ok(project_root(cwd).join("skills-lock.json")),
    }
}

fn global_lock_path() -> Result<PathBuf> {
    if let Some(state_home) = env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home).join("skills/.skill-lock.json"));
    }
    Ok(dirs::home_dir()
        .context("home directory is unavailable")?
        .join(".agents/.skill-lock.json"))
}

fn project_root(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

/// Merge successfully installed remote skills into the Skills CLI lock file.
/// Lock write errors are returned to the caller so the installation boundary can log them.
pub fn record_install(report: &SkillAddApplyReport, cwd: &Path) -> Result<()> {
    if !matches!(
        report.plan.source_kind.as_str(),
        "well-known" | "github" | "git" | "gitlab" | "huggingface"
    ) {
        return Ok(());
    }

    let lock_path = lock_path(report.plan.scope, cwd)?;
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut document = read_lock(&lock_path, report.plan.scope)?;
    let skills = document
        .as_object_mut()
        .context("Skills CLI lock file root must be an object")?
        .entry("skills")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("Skills CLI lock file skills field must be an object")?;

    for (skill, result) in report.plan.selected.iter().zip(&report.results) {
        if !result.applied || !result.target.is_dir() {
            continue;
        }
        let entry = make_entry(report, skill, &result.target, skills.get(&skill.name), &now)?;
        skills.insert(skill.name.clone(), entry);
    }

    if let Some(object) = document.as_object_mut() {
        object.entry("version").or_insert_with(|| {
            json!(match report.plan.scope {
                SkillInstallScope::Global => GLOBAL_VERSION,
                SkillInstallScope::Project => PROJECT_VERSION,
            })
        });
    }
    atomic_write(
        &lock_path,
        &format!("{}\n", serde_json::to_string_pretty(&document)?),
    )
    .with_context(|| {
        format!(
            "failed to write Skills CLI lock file {}",
            lock_path.display()
        )
    })
}

fn read_lock(path: &Path, scope: SkillInstallScope) -> Result<Value> {
    let version = match scope {
        SkillInstallScope::Global => GLOBAL_VERSION,
        SkillInstallScope::Project => PROJECT_VERSION,
    };
    match fs::read_to_string(path) {
        Ok(content) => {
            let mut value: Value = serde_json::from_str(&content)
                .with_context(|| format!("invalid Skills CLI lock file {}", path.display()))?;
            let root = value
                .as_object_mut()
                .context("Skills CLI lock file root must be an object")?;
            let existing_version = root.get("version").and_then(Value::as_u64);
            if existing_version.is_none() || !root.get("skills").is_some_and(Value::is_object) {
                bail!(
                    "unsupported Skills CLI lock file structure at {}",
                    path.display()
                );
            }
            // Preserve newer lock versions and their unrecognized fields.
            if existing_version.unwrap_or(version) < version {
                root.insert("version".into(), json!(version));
            }
            Ok(value)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({
            "version": version,
            "skills": {},
        })),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn make_entry(
    report: &SkillAddApplyReport,
    skill: &crate::skills::InstallableSkill,
    installed_path: &Path,
    previous: Option<&Value>,
    now: &str,
) -> Result<Value> {
    let source_kind = report.plan.source_kind.as_str();
    let discovered_skill_path = normalized_skill_path(&skill.relative_path);
    let relative_skill_path = if is_git_source_kind(source_kind) {
        git_relative_skill_path(&skill.path).unwrap_or(discovered_skill_path)
    } else {
        discovered_skill_path
    };
    let computed_hash = content_hash(installed_path)?;
    let well_known_metadata = (source_kind == "well-known")
        .then(|| well_known_metadata(&report.plan.source_root, &skill.name))
        .transpose()?
        .flatten();
    let well_known_digest = if source_kind == "well-known" {
        Some(
            well_known_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("digest").and_then(Value::as_str))
                .map(str::to_string)
                .unwrap_or(well_known_digest(installed_path)?),
        )
    } else {
        None
    };

    let mut entry = match report.plan.scope {
        SkillInstallScope::Global => {
            let source = if source_kind == "well-known" {
                source_host(&report.plan.source)
            } else {
                global_source(source_kind, &report.plan.source)
            };
            let mut entry = json!({
                "source": source,
                "sourceType": source_kind,
                "sourceUrl": well_known_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("sourceUrl"))
                    .cloned()
                    .unwrap_or_else(|| json!(report.plan.source)),
                "skillFolderHash": git_tree_hash(&skill.path, &relative_skill_path).unwrap_or_default(),
                "installedAt": previous
                    .and_then(|value| value.get("installedAt"))
                    .cloned()
                    .unwrap_or_else(|| json!(now)),
                "updatedAt": now,
            });
            if let Some(ref_name) = &report.plan.source_ref {
                entry["ref"] = json!(ref_name);
            }
            if is_git_source_kind(source_kind) && !relative_skill_path.is_empty() {
                entry["skillPath"] = json!(relative_skill_path);
            }
            if source_kind == "well-known" {
                entry["sourceBaseUrl"] = json!(report.plan.source);
                entry["wellKnownDigest"] = json!(well_known_digest.unwrap_or_default());
            }
            entry
        }
        SkillInstallScope::Project => {
            let source = if source_kind == "well-known" {
                source_host(&report.plan.source)
            } else {
                global_source(source_kind, &report.plan.source)
            };
            let mut entry = json!({
                "source": source,
                "sourceType": source_kind,
                "computedHash": computed_hash,
            });
            if source_kind == "well-known"
                || matches!(source_kind, "git" | "gitlab" | "huggingface")
            {
                entry["sourceUrl"] = json!(report.plan.source);
            }
            if let Some(ref_name) = &report.plan.source_ref {
                entry["ref"] = json!(ref_name);
            }
            if is_git_source_kind(source_kind) && !relative_skill_path.is_empty() {
                entry["skillPath"] = json!(relative_skill_path);
            }
            if source_kind == "well-known" {
                entry["wellKnownDigest"] = json!(well_known_digest.unwrap_or_default());
            }
            entry
        }
    };

    // Retain CLI-owned optional fields Tendi does not currently produce.
    if let (Some(current), Some(previous)) =
        (entry.as_object_mut(), previous.and_then(Value::as_object))
    {
        for (key, value) in previous {
            if !current.contains_key(key) && matches!(key.as_str(), "pluginName" | "subagents") {
                current.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(entry)
}

fn global_source(source_kind: &str, source: &str) -> String {
    if source_kind != "github" {
        return source.to_string();
    }
    let cleaned = source.trim_end_matches('/').trim_end_matches(".git");
    if let Some(path) = cleaned
        .strip_prefix("https://github.com/")
        .or_else(|| cleaned.strip_prefix("http://github.com/"))
    {
        return path.to_string();
    }
    source.to_string()
}

fn source_host(source: &str) -> String {
    url::Url::parse(source)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host.trim_start_matches("www.").to_string())
        })
        .unwrap_or_else(|| source.to_string())
}

fn normalized_skill_path(path: &str) -> String {
    let path = path.trim_matches('/').replace('\\', "/");
    if path.is_empty() || path.ends_with("SKILL.md") {
        path
    } else {
        format!("{path}/SKILL.md")
    }
}

fn git_tree_hash(skill_path: &Path, _relative_skill_path: &str) -> Option<String> {
    let skill_dir = skill_path.canonicalize().ok()?;
    let repo = skill_dir
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists())?;
    let tree_path = skill_dir
        .strip_prefix(repo)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    let spec = if tree_path.is_empty() {
        "HEAD^{tree}".to_string()
    } else {
        format!("HEAD:{tree_path}")
    };
    let output = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "rev-parse", &spec])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_relative_skill_path(skill_path: &Path) -> Option<String> {
    let skill_dir = skill_path.canonicalize().ok()?;
    let repo = skill_dir
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists())?;
    let relative = skill_dir
        .strip_prefix(repo)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    Some(if relative.is_empty() {
        "SKILL.md".to_string()
    } else {
        format!("{relative}/SKILL.md")
    })
}

fn content_hash(root: &Path) -> Result<String> {
    let mut files = WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| {
            !entry.file_type().is_dir()
                || !matches!(entry.file_name().to_str(), Some(".git" | "node_modules"))
        })
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            let relative = entry
                .path()
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            Ok((relative, fs::read(entry.path())?))
        })
        .collect::<Result<Vec<_>>>()?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hash = Sha256::new();
    for (path, content) in files {
        hash.update(path.as_bytes());
        hash.update(content);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn well_known_digest(root: &Path) -> Result<String> {
    let mut files = WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| {
            !entry.file_type().is_dir()
                || !matches!(entry.file_name().to_str(), Some(".git" | "node_modules"))
        })
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            let relative = entry
                .path()
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            Ok((relative, fs::read(entry.path())?))
        })
        .collect::<Result<Vec<_>>>()?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hash = Sha256::new();
    for (path, content) in files {
        hash.update(path.as_bytes());
        hash.update([0]);
        hash.update(content);
        hash.update([0]);
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}

fn well_known_metadata(source_root: &Path, skill_name: &str) -> Result<Option<Value>> {
    let path = source_root.join(".tendi-well-known-lock.json");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let value: Value = serde_json::from_str(&content)
        .with_context(|| format!("invalid well-known lock metadata {}", path.display()))?;
    Ok(value
        .get("skills")
        .and_then(Value::as_object)
        .and_then(|skills| skills.get(skill_name))
        .cloned())
}
