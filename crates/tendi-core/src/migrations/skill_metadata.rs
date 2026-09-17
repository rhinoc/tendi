use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_yaml::Value;

use crate::skills::{ChangeSet, FileChange, SkillScan, SkillVisibility};
use crate::storage::{Store, canonical_workspace_root};

const MIGRATION_KEY: &str = "skill_visibility_database_migrated_v1";
const LEGACY_RELATIVE_PATH: &str = "agents/tendi.yaml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyMetadataFile {
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    visibility: SkillVisibility,
}

fn default_schema_version() -> u32 {
    1
}

pub(super) fn migrate_scan(store: &Store, cwd: &Path, scan: &SkillScan) -> Result<bool> {
    let workspace_root = canonical_workspace_root(cwd);
    let migration_key = format!("{MIGRATION_KEY}:{}", workspace_root.display());
    if super::migration_completed(store, &migration_key)? {
        return Ok(false);
    }

    let persisted = store.skill_visibilities_for_workspace(&workspace_root)?;
    let mut values = Vec::<(PathBuf, SkillVisibility)>::new();
    let mut legacy_files = Vec::new();
    let mut changes = Vec::new();
    for skill in &scan.skills {
        for path in &skill.paths {
            let key = path
                .path
                .canonicalize()
                .unwrap_or_else(|_| path.path.clone());
            let sidecar = read_legacy_visibility(&path.path)?;
            let skill_file = path.path.join("SKILL.md");
            let skill_text = fs::read_to_string(&skill_file).ok();
            let legacy_frontmatter = skill_text
                .as_deref()
                .and_then(crate::skills::parse_frontmatter)
                .and_then(|frontmatter| parse_legacy_frontmatter_visibility(&frontmatter));
            let visibility = persisted
                .get(&key)
                .copied()
                .or(sidecar)
                .or(legacy_frontmatter)
                .unwrap_or(path.effective_visibility);
            if !persisted.contains_key(&key) {
                values.push((key, visibility));
            }
            if sidecar.is_some() {
                legacy_files.push(path.path.join(LEGACY_RELATIVE_PATH));
            }
            if let Some(skill_text) = skill_text {
                let cleaned = remove_legacy_frontmatter_visibility(&skill_text)?;
                if cleaned != skill_text {
                    changes.push(FileChange {
                        path: skill_file,
                        before_sha256: Some(crate::fsutil::sha256_text(&skill_text)),
                        before: Some(skill_text),
                        after: cleaned,
                    });
                }
            }
        }
    }

    values.sort_by(|left, right| left.0.cmp(&right.0));
    values.dedup_by(|left, right| left.0 == right.0);
    if !values.is_empty() {
        store.upsert_skill_visibilities_for_workspace(&workspace_root, &values)?;
    }
    let had_changes = !changes.is_empty();
    if had_changes {
        crate::skills::apply_changes(&ChangeSet {
            changes: crate::skills::dedupe_changes(changes),
        })?;
    }
    let had_legacy_files = !legacy_files.is_empty();
    for path in legacy_files {
        remove_legacy_file(&path)?;
    }

    let changed = !values.is_empty() || had_legacy_files || had_changes;
    if scan.warnings.is_empty() {
        super::mark_migration_completed(store, &migration_key)?;
    }
    Ok(changed)
}

fn read_legacy_visibility(skill_dir: &Path) -> Result<Option<SkillVisibility>> {
    let path = skill_dir.join(LEGACY_RELATIVE_PATH);
    let Some(text) = fs::read_to_string(&path).ok() else {
        return Ok(None);
    };
    let metadata = serde_yaml::from_str::<LegacyMetadataFile>(&text)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if metadata.schema_version != 1 {
        bail!(
            "unsupported legacy Tendi skill metadata schema version {} in {}",
            metadata.schema_version,
            path.display()
        );
    }
    if metadata.visibility == SkillVisibility::Mixed {
        bail!(
            "mixed visibility cannot be migrated from {}",
            path.display()
        );
    }
    Ok(Some(metadata.visibility))
}

fn parse_legacy_frontmatter_visibility(frontmatter: &Value) -> Option<SkillVisibility> {
    let value = frontmatter
        .get("tendi")
        .and_then(|tendi| tendi.get("visibility"))
        .or_else(|| frontmatter.get("tendi.visibility"))?
        .as_str()?;
    match value.to_ascii_lowercase().as_str() {
        "auto" => Some(SkillVisibility::Auto),
        "manual" => Some(SkillVisibility::Manual),
        "off" => Some(SkillVisibility::Off),
        _ => None,
    }
}

fn remove_legacy_frontmatter_visibility(text: &str) -> Result<String> {
    let mut doc = crate::skills::MarkdownDoc::parse(text)?;
    let tendi_key = Value::String("tendi".to_string());
    let flat_key = Value::String("tendi.visibility".to_string());
    let mut changed = doc.meta.remove(&flat_key).is_some();
    if let Some(tendi) = doc.meta.get_mut(&tendi_key).and_then(Value::as_mapping_mut) {
        let visibility_key = Value::String("visibility".to_string());
        changed |= tendi.remove(&visibility_key).is_some();
        if tendi.is_empty() {
            doc.meta.remove(&tendi_key);
        }
    }
    if changed {
        doc.render()
    } else {
        Ok(text.to_string())
    }
}

fn remove_legacy_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            fs::remove_file(path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
        Ok(_) => bail!(
            "refusing to remove non-file legacy metadata {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::skills::{AgentKind, SkillPath, SkillRecord, SkillRoot};

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "tendi-skill-visibility-migration-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn scan_for(skill_dir: &Path) -> SkillScan {
        SkillScan {
            roots: vec![SkillRoot {
                path: skill_dir.parent().unwrap().to_path_buf(),
                scope: "global".to_string(),
                agent: AgentKind::Shared,
                plugin_id: None,
                plugin_enabled: None,
            }],
            skills: vec![SkillRecord {
                id: "skill@test".to_string(),
                installation_id: "skill@test".to_string(),
                name: "demo".to_string(),
                description: Some("Demo".to_string()),
                tags: Vec::new(),
                dependencies: Vec::new(),
                dependents: Vec::new(),
                dependency_ids: Vec::new(),
                dependent_ids: Vec::new(),
                is_wrapper: false,
                visibility: SkillVisibility::Manual,
                agents: vec![AgentKind::Shared],
                paths: vec![SkillPath {
                    path: skill_dir.to_path_buf(),
                    root: skill_dir.parent().unwrap().to_path_buf(),
                    scope: "global".to_string(),
                    agent: AgentKind::Shared,
                    install_target: "shared".to_string(),
                    source_kind: "local".to_string(),
                    source: None,
                    source_ref: None,
                    source_version: None,
                    source_relative_path: None,
                    symlink_status: "direct".to_string(),
                    update_status: "local".to_string(),
                    sha256: "sha".to_string(),
                    tags: Vec::new(),
                    tendi_visibility: None,
                    effective_visibility: SkillVisibility::Auto,
                    provider_allow_implicit_invocation: None,
                    provider_skill_enabled: None,
                    provider_disable_model_invocation: None,
                    plugin_id: None,
                    plugin_enabled: None,
                }],
                source_summary: "local".to_string(),
                install_targets: vec!["shared".to_string()],
                update_status: "local".to_string(),
                is_system: false,
                ctime: None,
                mtime: None,
            }],
            warnings: Vec::new(),
        }
    }

    #[test]
    fn migrates_legacy_visibility_once_into_database() {
        let root = temp_dir();
        let skill_dir = root.join(".agents/skills/demo");
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ntendi:\n  visibility: off\n---\n\n# Demo\n",
        )
        .unwrap();
        fs::write(
            skill_dir.join(LEGACY_RELATIVE_PATH),
            "schema_version: 1\nvisibility: manual\n",
        )
        .unwrap();

        let store = Store::open(root.join("tendi.sqlite3")).unwrap();
        let scan = scan_for(&skill_dir);
        assert!(migrate_scan(&store, &root, &scan).unwrap());
        assert!(!skill_dir.join(LEGACY_RELATIVE_PATH).exists());
        assert!(
            !fs::read_to_string(skill_dir.join("SKILL.md"))
                .unwrap()
                .contains("tendi:")
        );
        assert_eq!(
            store
                .skill_visibilities_for_workspace(&root)
                .unwrap()
                .get(&skill_dir.canonicalize().unwrap())
                .copied(),
            Some(SkillVisibility::Manual)
        );
        assert!(!migrate_scan(&store, &root, &scan).unwrap());

        fs::remove_dir_all(root).unwrap();
    }
}
