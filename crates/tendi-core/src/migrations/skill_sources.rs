use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use serde::Deserialize;

use crate::skills::{SkillScan, SkillSourceRecord};

use crate::storage::{Store, canonical_workspace_root, workspace_scope_key};

const SKILLS_CLI_SOURCE_MIGRATION_KEY: &str = "skills_cli_source_records_migrated_v1";

#[derive(Debug, Default, Deserialize)]
struct SkillsCliLockFile {
    version: u64,
    #[serde(default)]
    skills: BTreeMap<String, SkillsCliLockEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillsCliLockEntry {
    source: String,
    source_type: String,
    source_url: Option<String>,
    r#ref: Option<String>,
    skill_path: Option<String>,
    skill_folder_hash: Option<String>,
    computed_hash: Option<String>,
}

#[derive(Debug, Default)]
struct SkillsCliLockDatabase {
    global: BTreeMap<String, SkillsCliLockEntry>,
    projects: Vec<(PathBuf, BTreeMap<String, SkillsCliLockEntry>)>,
}

/// Import source provenance from Skills CLI lock files into the scoped source table.
///
/// This is deliberately a migration operation. Runtime skill scanning must only read the
/// scoped source table; it must not fall back to lock files after this import has completed.
pub(super) fn migrate_scan(store: &Store, cwd: &Path, scan: &mut SkillScan) -> Result<bool> {
    let workspace_root = canonical_workspace_root(cwd);
    let scope_key = workspace_scope_key(&workspace_root)?;
    let migration_key = format!("{SKILLS_CLI_SOURCE_MIGRATION_KEY}:{}", scope_key.as_str());
    if super::migration_completed(store, &migration_key)? {
        return Ok(false);
    }

    let existing_paths = store
        .skill_source_records_for_workspace(cwd)?
        .into_iter()
        .map(|record| record.skill_path)
        .collect::<BTreeSet<_>>();
    let (locks, lock_warnings) = SkillsCliLockDatabase::load(cwd);
    let has_lock_warnings = !lock_warnings.is_empty();
    scan.warnings.extend(lock_warnings);
    let mut records = Vec::new();
    let mut seen_paths = existing_paths;

    for skill in &scan.skills {
        for path in &skill.paths {
            if seen_paths.contains(&path.path) {
                continue;
            }
            let Some(entry) = locks.entry(&skill.name, &path.scope, &path.path) else {
                continue;
            };
            records.push(SkillSourceRecord {
                skill_name: skill.name.clone(),
                skill_path: path.path.clone(),
                source_kind: entry.source_type.trim().to_ascii_lowercase(),
                source: Some(normalize_locked_source(
                    entry.source(),
                    entry.source_type.as_str(),
                )),
                source_ref: non_empty(entry.r#ref.as_deref()),
                source_version: entry
                    .skill_folder_hash
                    .as_deref()
                    .and_then(|value| non_empty(Some(value)))
                    .or_else(|| non_empty(entry.computed_hash.as_deref())),
                source_relative_path: non_empty(entry.skill_path.as_deref()),
                update_status: "tracked".to_string(),
                origin: "skills-cli-lock".to_string(),
            });
            seen_paths.insert(path.path.clone());
        }
    }

    if records.is_empty() {
        if !has_lock_warnings {
            super::mark_migration_completed(store, &migration_key)?;
        }
        return Ok(false);
    }

    store.with_named_write_transaction("migration.skill_sources", |tx| {
        store
            .insert_skill_source_records_if_missing_for_workspace_in_tx(tx, &scope_key, &records)?;
        if !has_lock_warnings {
            super::mark_migration_completed_in_tx(tx, &migration_key)?;
        }
        Ok(())
    })?;
    Ok(true)
}

impl SkillsCliLockDatabase {
    fn load(cwd: &Path) -> (Self, Vec<String>) {
        let mut database = Self::default();
        let mut warnings = Vec::new();

        if let Some(path) = global_skills_cli_lock_path() {
            if let Some(lock) = read_skills_cli_lock(&path, 3, &mut warnings) {
                database.global = lock.skills;
            }
        }

        for project_dir in skill_project_dirs(cwd) {
            let path = project_dir.join("skills-lock.json");
            if let Some(lock) = read_skills_cli_lock(&path, 1, &mut warnings) {
                database.projects.push((project_dir, lock.skills));
            }
        }

        (database, warnings)
    }

    fn entry(&self, name: &str, scope: &str, skill_path: &Path) -> Option<&SkillsCliLockEntry> {
        match scope {
            "global" => matching_skills_cli_lock_entry(&self.global, name),
            "project" => self
                .projects
                .iter()
                .filter(|(root, _)| skill_path.starts_with(root))
                .max_by_key(|(root, _)| root.components().count())
                .and_then(|(_, skills)| matching_skills_cli_lock_entry(skills, name)),
            _ => None,
        }
    }
}

impl SkillsCliLockEntry {
    fn source(&self) -> &str {
        self.source_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(self.source.as_str())
    }
}

fn read_skills_cli_lock(
    path: &Path,
    expected_version: u64,
    warnings: &mut Vec<String>,
) -> Option<SkillsCliLockFile> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            warnings.push(format!(
                "{}: failed to read skills CLI lock: {error}",
                path.display()
            ));
            return None;
        }
    };
    let lock = match serde_json::from_str::<SkillsCliLockFile>(&text) {
        Ok(lock) => lock,
        Err(error) => {
            warnings.push(format!(
                "{}: invalid skills CLI lock: {error}",
                path.display()
            ));
            return None;
        }
    };
    if lock.version != expected_version {
        warnings.push(format!(
            "{}: unsupported skills CLI lock version {}; expected {}",
            path.display(),
            lock.version,
            expected_version
        ));
        return None;
    }
    Some(lock)
}

fn global_skills_cli_lock_path() -> Option<PathBuf> {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .map(|root| root.join("skills/.skill-lock.json"))
        .or_else(|| dirs::home_dir().map(|home| home.join(".agents/.skill-lock.json")))
}

fn skill_project_dirs(cwd: &Path) -> Vec<PathBuf> {
    let root = cwd
        .ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(cwd);
    let mut dirs = cwd
        .ancestors()
        .take_while(|path| *path != root)
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    dirs.push(root.to_path_buf());
    dirs
}

fn matching_skills_cli_lock_entry<'a>(
    skills: &'a BTreeMap<String, SkillsCliLockEntry>,
    name: &str,
) -> Option<&'a SkillsCliLockEntry> {
    skills.get(name).or_else(|| {
        let normalized_name = normalize_skill_match_name(name);
        skills.iter().find_map(|(candidate, entry)| {
            (normalize_skill_match_name(candidate) == normalized_name).then_some(entry)
        })
    })
}

fn normalize_skill_match_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalize_locked_source(source: &str, kind: &str) -> String {
    let source = source.trim();
    if kind.trim().eq_ignore_ascii_case("github")
        && source.split('/').count() == 2
        && !source.contains(':')
        && !source.contains('.')
    {
        return format!("https://github.com/{source}.git");
    }
    source.to_string()
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
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
}
