use std::{
    collections::BTreeMap,
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

/// Import source provenance from Skills CLI lock files into the scoped source table when the
/// lock inputs change. Runtime skill scanning reads only the scoped source table.
pub(super) fn migrate_scan(store: &Store, cwd: &Path, scan: &mut SkillScan) -> Result<bool> {
    let migration_key = migration_key_for_workspace(cwd)?;
    let existing_records = store
        .skill_source_records_for_workspace(cwd)?
        .into_iter()
        .map(|record| (record.skill_path.clone(), record))
        .collect::<BTreeMap<_, _>>();
    let (locks, lock_warnings) = SkillsCliLockDatabase::load(cwd);
    let has_lock_warnings = !lock_warnings.is_empty();
    scan.warnings.extend(lock_warnings);
    let mut records = Vec::new();

    for skill in &scan.skills {
        for path in &skill.paths {
            let Some(entry) = locks.entry(&skill.name, &path.scope, &path.path) else {
                continue;
            };
            let existing = existing_records.get(&path.path);
            let source_kind = entry.source_type.trim().to_ascii_lowercase();
            // Lock entries are authoritative for paths they identify. Upgrade inferred local
            // provenance and refresh records previously imported from the lock, while leaving
            // paths already attributed to a different source kind untouched.
            if existing.is_some_and(|record| {
                record.source_kind != "local" && record.source_kind != source_kind
            }) {
                continue;
            }
            let record = SkillSourceRecord {
                skill_name: skill.name.clone(),
                skill_path: path.path.clone(),
                source_kind,
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
                origin: existing
                    .map(|record| record.origin.clone())
                    .unwrap_or_else(|| "skills-cli-lock".to_string()),
            };
            if existing.is_none_or(|existing| !same_source_record(existing, &record)) {
                records.push(record);
            }
        }
    }

    if records.is_empty() {
        if !has_lock_warnings && !super::migration_completed(store, &migration_key)? {
            super::mark_migration_completed(store, &migration_key)?;
        }
        return Ok(false);
    }

    let changed = store.upsert_skill_source_records_for_workspace(cwd, &records)? > 0;
    if !has_lock_warnings && !super::migration_completed(store, &migration_key)? {
        super::mark_migration_completed(store, &migration_key)?;
    }
    Ok(changed)
}

fn same_source_record(left: &SkillSourceRecord, right: &SkillSourceRecord) -> bool {
    left.skill_name == right.skill_name
        && left.skill_path == right.skill_path
        && left.source_kind == right.source_kind
        && left.source == right.source
        && left.source_ref == right.source_ref
        && left.source_version == right.source_version
        && left.source_relative_path == right.source_relative_path
        && left.update_status == right.update_status
}

pub(super) fn migration_completed_for_workspace(store: &Store, cwd: &Path) -> Result<bool> {
    let migration_key = migration_key_for_workspace(cwd)?;
    super::migration_completed(store, &migration_key)
}

pub(super) fn migration_key_for_workspace(cwd: &Path) -> Result<String> {
    let workspace_root = canonical_workspace_root(cwd);
    let scope_key = workspace_scope_key(&workspace_root)?;
    let mut lock_inputs = String::new();
    for (path, _) in skills_cli_lock_paths(cwd) {
        lock_inputs.push_str(&path.display().to_string());
        lock_inputs.push('\n');
        match fs::read_to_string(&path) {
            Ok(contents) => {
                lock_inputs.push_str("present\n");
                lock_inputs.push_str(&contents);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                lock_inputs.push_str("missing\n");
            }
            Err(error) => {
                lock_inputs.push_str("unreadable:");
                lock_inputs.push_str(&error.kind().to_string());
                lock_inputs.push('\n');
            }
        }
        lock_inputs.push('\n');
    }
    let lock_fingerprint = crate::fsutil::sha256_text(&lock_inputs);
    Ok(format!(
        "{SKILLS_CLI_SOURCE_MIGRATION_KEY}:{}:{lock_fingerprint}",
        scope_key.as_str(),
    ))
}

impl SkillsCliLockDatabase {
    fn load(cwd: &Path) -> (Self, Vec<String>) {
        let mut database = Self::default();
        let mut warnings = Vec::new();

        for (path, expected_version) in skills_cli_lock_paths(cwd) {
            if let Some(lock) = read_skills_cli_lock(&path, expected_version, &mut warnings) {
                if expected_version == 3 {
                    database.global = lock.skills;
                } else if let Some(project_dir) = path.parent() {
                    database
                        .projects
                        .push((project_dir.to_path_buf(), lock.skills));
                }
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

fn skills_cli_lock_paths(cwd: &Path) -> Vec<(PathBuf, u64)> {
    let mut paths = global_skills_cli_lock_path()
        .into_iter()
        .map(|path| (path, 3))
        .collect::<Vec<_>>();
    paths.extend(
        skill_project_dirs(cwd)
            .into_iter()
            .map(|directory| (directory.join("skills-lock.json"), 1)),
    );
    paths
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
#[path = "skill_sources_tests.rs"]
mod tests;
