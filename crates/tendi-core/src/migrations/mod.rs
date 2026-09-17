use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};

#[cfg(not(test))]
use crate::storage::default_db_path;
use crate::{
    skills::{SkillPath, SkillScan, SkillSourceRecord},
    storage::{PROJECTION_PARSER_VERSION, STORAGE_SCHEMA_VERSION, Store},
};

mod schema;
mod skill_metadata;
mod skill_sources;

const CODEX_GLOBAL_SKILL_CONFIG_MIGRATION_KEY: &str = "codex_global_skill_config_migrated_v1";
const GIT_SOURCE_VERSION_LONG_SHA_MIGRATION_KEY: &str = "git_source_version_long_sha_migrated_v1";
const MAX_SQUASHED_DEVELOPMENT_SCHEMA_VERSION: i64 = 3;

impl Store {
    pub fn open_default() -> Result<Self> {
        #[cfg(test)]
        let path = crate::storage::test_default_db_path();
        #[cfg(not(test))]
        let path = default_db_path()?;
        Self::open(path)
    }

    /// Normalize Git source revisions explicitly. This operates on the current
    /// scoped projection tables; legacy per-field tables are not part of the
    /// database contract anymore.
    pub fn normalize_git_source_versions(&self) -> Result<()> {
        migrate_git_source_versions(self)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to create sqlite database {}", path.display()))?;
        let path = fs::canonicalize(&path)
            .with_context(|| format!("failed to resolve sqlite database {}", path.display()))?;
        let writer = super::database::DatabaseWriter::open(&path)?;
        let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("failed to open sqlite reader {}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(30))?;
        let store = Self { conn, path, writer };
        let schema_version = store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
        let schema_needs_initialization =
            schema_version != STORAGE_SCHEMA_VERSION || schema::needs_current_shape(&store.conn)?;
        if schema_needs_initialization {
            store.with_named_write_transaction("schema.initialize", |tx| {
                let current: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
                if current != STORAGE_SCHEMA_VERSION || schema::needs_current_shape(tx)? {
                    anyhow::ensure!(
                        (0..=MAX_SQUASHED_DEVELOPMENT_SCHEMA_VERSION).contains(&current),
                        "unsupported database schema version {current}; this build only supports schema versions 0 through {MAX_SQUASHED_DEVELOPMENT_SCHEMA_VERSION}"
                    );
                    schema::bootstrap(tx)?;
                    schema::run(tx)?;
                }
                Ok(())
            })?;
        }
        Ok(store)
    }
}

pub(crate) fn run_workspace(store: &Store, cwd: &Path, project_roots: &[PathBuf]) -> Result<()> {
    store.ensure_skill_visibility_table()?;
    migrate_codex_global_skill_config(store)?;
    invalidate_old_projection_contexts(store)?;

    let mut scan =
        crate::skills::scan_skills_for_workspace_initialization(cwd, store, project_roots)?;
    loop {
        if crate::skills::materialize_tendi_cache_links(&scan)? {
            scan =
                crate::skills::scan_skills_for_workspace_initialization(cwd, store, project_roots)?;
            continue;
        }
        let metadata_changed = skill_metadata::migrate_scan(store, cwd, &scan)?;
        let source_changed = skill_sources::migrate_scan(store, cwd, &mut scan)?;
        if !metadata_changed && !source_changed {
            return Ok(());
        }
        scan = crate::skills::scan_skills_for_workspace_initialization(cwd, store, project_roots)?;
    }
}

fn migrate_codex_global_skill_config(store: &Store) -> Result<()> {
    if migration_completed(store, CODEX_GLOBAL_SKILL_CONFIG_MIGRATION_KEY)? {
        return Ok(());
    }
    crate::providers::codex::rewrite_legacy_global_skill_config()
        .context("failed to rewrite legacy Codex global skill config")?;
    mark_migration_completed(store, CODEX_GLOBAL_SKILL_CONFIG_MIGRATION_KEY)
}

fn invalidate_old_projection_contexts(store: &Store) -> Result<()> {
    let needs_invalidation = store.conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM scoped_projection_contexts WHERE parser_version != ?1
         )",
        params![PROJECTION_PARSER_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if !needs_invalidation {
        return Ok(());
    }

    store.with_named_write_transaction("migration.invalidate_old_projection_contexts", |tx| {
        let mut statement = tx.prepare(
            "SELECT scope_key, domain FROM scoped_projection_contexts
             WHERE parser_version != ?1",
        )?;
        let outdated = statement
            .query_map([PROJECTION_PARSER_VERSION], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for (scope, domain) in outdated {
            store.set_projection_context_in_tx(
                tx,
                &crate::ScopeKey::new(scope)?,
                &domain,
                "stale",
                None,
            )?;
        }
        tx.execute(
            "UPDATE scoped_projection_contexts
             SET state = 'stale', scanned_at = NULL, error = NULL,
                 parser_version = ?1
             WHERE parser_version != ?1",
            params![PROJECTION_PARSER_VERSION],
        )?;
        Ok(())
    })?;
    Ok(())
}

#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd)]
struct GitSourceVersionKey {
    skill_path: String,
    source: Option<String>,
    source_ref: Option<String>,
    source_version: String,
}

pub(super) fn migrate_git_source_versions(store: &Store) -> Result<()> {
    if migration_completed(store, GIT_SOURCE_VERSION_LONG_SHA_MIGRATION_KEY)? {
        return verify_git_source_versions(store);
    }

    let candidates = collect_git_source_version_candidates(store)?;
    let mut replacements = BTreeMap::new();
    let mut unresolved = Vec::new();
    for record in candidates {
        let Some(key) = git_source_version_key(&record) else {
            continue;
        };
        let Some(full_revision) = crate::skills::resolve_git_source_version(&record) else {
            unresolved.push(format!(
                "{} at {}",
                key.source_version,
                record.skill_path.display()
            ));
            continue;
        };
        if !crate::skills::is_full_git_revision(&full_revision) {
            unresolved.push(format!(
                "{} at {} resolved to invalid revision {}",
                key.source_version,
                record.skill_path.display(),
                full_revision
            ));
            continue;
        }
        if let Some(existing) = replacements.insert(key.clone(), full_revision.clone())
            && existing != full_revision
        {
            bail!(
                "ambiguous Git source revision {} at {} resolved to both {} and {}",
                key.source_version,
                key.skill_path,
                existing,
                full_revision
            );
        }
    }

    if !unresolved.is_empty() {
        let details = unresolved
            .into_iter()
            .take(20)
            .collect::<Vec<_>>()
            .join(", ");
        bail!("cannot migrate all Git source revisions to full SHA; unresolved: {details}");
    }

    store.with_named_write_transaction("migration.migrate_git_source_versions", |tx| {
        migrate_source_records(tx, &replacements)?;
        migrate_normalized_skill_snapshots(tx, &replacements)?;
        migrate_skill_snapshots(tx, &replacements)?;
        Ok(())
    })?;

    verify_git_source_versions(store)?;
    mark_migration_completed(store, GIT_SOURCE_VERSION_LONG_SHA_MIGRATION_KEY)
}

fn verify_git_source_versions(store: &Store) -> Result<()> {
    let remaining = collect_git_source_version_candidates(store)?;
    if !remaining.is_empty() {
        let details = remaining
            .into_iter()
            .take(20)
            .map(|record| {
                format!(
                    "{} at {}",
                    record.source_version.unwrap_or_default(),
                    record.skill_path.display()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        bail!("Git source revision migration left short SHAs: {details}");
    }
    let remaining_snapshots = short_git_snapshot_versions(store)?;
    if !remaining_snapshots.is_empty() {
        let details = remaining_snapshots
            .into_iter()
            .take(20)
            .collect::<Vec<_>>()
            .join(", ");
        bail!("Git snapshot revision migration left short SHAs: {details}");
    }
    Ok(())
}

fn collect_git_source_version_candidates(store: &Store) -> Result<Vec<SkillSourceRecord>> {
    let mut candidates = BTreeMap::<GitSourceVersionKey, SkillSourceRecord>::new();
    let data_json = {
        let mut statement = store
            .conn
            .prepare("SELECT data_json FROM scoped_skill_sources")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for data_json in data_json {
        let record = serde_json::from_str::<SkillSourceRecord>(&data_json)
            .context("invalid Git source record in scoped_skill_sources")?;
        add_git_source_candidate(&mut candidates, record);
    }

    let normalized_skills = {
        let mut statement = store
            .conn
            .prepare("SELECT payload_json FROM normalized_snapshots WHERE domain = 'skills'")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for payload_json in normalized_skills {
        let scan = serde_json::from_str::<SkillScan>(&payload_json)
            .context("invalid skill snapshot during Git source migration")?;
        for skill in scan.skills {
            for path in skill.paths {
                add_git_source_candidate(
                    &mut candidates,
                    source_record_from_path(&skill.name, &path),
                );
            }
        }
    }
    Ok(candidates.into_values().collect())
}

fn add_git_source_candidate(
    candidates: &mut BTreeMap<GitSourceVersionKey, SkillSourceRecord>,
    record: SkillSourceRecord,
) {
    if let Some(key) = git_source_version_key(&record) {
        candidates.entry(key).or_insert(record);
    }
}

fn git_source_version_key(record: &SkillSourceRecord) -> Option<GitSourceVersionKey> {
    let source_version = record.source_version.as_deref()?;
    (crate::skills::is_git_source_kind(&record.source_kind)
        && crate::skills::is_abbreviated_git_revision(source_version))
    .then(|| GitSourceVersionKey {
        skill_path: record.skill_path.display().to_string(),
        source: record.source.clone(),
        source_ref: record.source_ref.clone(),
        source_version: source_version.to_string(),
    })
}

fn source_record_from_path(skill_name: &str, path: &SkillPath) -> SkillSourceRecord {
    SkillSourceRecord {
        skill_name: skill_name.to_string(),
        skill_path: path.path.clone(),
        source_kind: path.source_kind.clone(),
        source: path.source.clone(),
        source_ref: path.source_ref.clone(),
        source_version: path.source_version.clone(),
        source_relative_path: path.source_relative_path.clone(),
        update_status: path.update_status.clone(),
        origin: "git-source-version-migration".to_string(),
    }
}

fn replacement_for_record<'a>(
    record: &SkillSourceRecord,
    replacements: &'a BTreeMap<GitSourceVersionKey, String>,
) -> Option<&'a String> {
    replacements.get(&git_source_version_key(record)?)
}

fn replacement_for_path<'a>(
    path: &SkillPath,
    replacements: &'a BTreeMap<GitSourceVersionKey, String>,
) -> Option<&'a String> {
    replacements
        .iter()
        .find(|(key, _)| {
            key.skill_path == path.path.display().to_string()
                && key.source == path.source
                && key.source_ref == path.source_ref
                && path.source_version.as_deref() == Some(key.source_version.as_str())
        })
        .map(|(_, revision)| revision)
}

fn migrate_source_records(
    tx: &rusqlite::Transaction<'_>,
    replacements: &BTreeMap<GitSourceVersionKey, String>,
) -> Result<()> {
    let rows = {
        let mut statement =
            tx.prepare("SELECT scope_key, skill_path, data_json FROM scoped_skill_sources")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (scope_key, skill_path, data_json) in rows {
        let mut record = serde_json::from_str::<SkillSourceRecord>(&data_json)
            .context("invalid Git source record in scoped_skill_sources")?;
        let Some(full_revision) = replacement_for_record(&record, replacements) else {
            continue;
        };
        record.source_version = Some(full_revision.clone());
        tx.execute(
            "UPDATE scoped_skill_sources
             SET source_version = ?1, data_json = ?2
             WHERE scope_key = ?3 AND skill_path = ?4",
            params![
                full_revision,
                serde_json::to_string(&record)?,
                scope_key,
                skill_path
            ],
        )?;
    }
    Ok(())
}

fn migrate_normalized_skill_snapshots(
    tx: &rusqlite::Transaction<'_>,
    replacements: &BTreeMap<GitSourceVersionKey, String>,
) -> Result<()> {
    let rows = {
        let mut statement = tx.prepare(
            "SELECT scope_key, payload_json FROM normalized_snapshots WHERE domain = 'skills'",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (scope_key, payload_json) in rows {
        let mut scan = serde_json::from_str::<SkillScan>(&payload_json)
            .context("invalid skill snapshot during Git source migration")?;
        let mut changed = false;
        for skill in &mut scan.skills {
            for path in &mut skill.paths {
                if let Some(full_revision) = replacement_for_path(path, replacements) {
                    path.source_version = Some(full_revision.clone());
                    changed = true;
                }
            }
        }
        if changed {
            tx.execute(
                "UPDATE normalized_snapshots SET payload_json = ?1
                 WHERE scope_key = ?2 AND domain = 'skills'",
                params![serde_json::to_string(&scan)?, scope_key],
            )?;
        }
    }
    Ok(())
}

fn migrate_skill_snapshots(
    tx: &rusqlite::Transaction<'_>,
    replacements: &BTreeMap<GitSourceVersionKey, String>,
) -> Result<()> {
    for key in replacements.keys() {
        tx.execute(
            "UPDATE scoped_skill_snapshots SET source_version = ?1
             WHERE skill_path = ?2 AND source_version = ?3",
            params![replacements[key], key.skill_path, key.source_version],
        )?;
    }
    Ok(())
}

fn short_git_snapshot_versions(store: &Store) -> Result<Vec<String>> {
    let mut leftovers = Vec::new();
    let mut statement = store.conn.prepare(
        "SELECT ss.skill_path, ss.source_version
         FROM scoped_skill_snapshots ss
         JOIN scoped_skill_sources s
           ON s.scope_key = ss.scope_key AND s.skill_path = ss.skill_path
         WHERE s.source_kind IN ('git', 'github', 'gitlab', 'huggingface')",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (skill_path, source_version) = row?;
        if crate::skills::is_abbreviated_git_revision(&source_version) {
            leftovers.push(format!("{} at {}", source_version, skill_path));
        }
    }
    Ok(leftovers)
}

fn migration_completed(store: &Store, key: &str) -> Result<bool> {
    Ok(store
        .conn
        .query_row(
            "SELECT 1 FROM meta WHERE key = ?1 LIMIT 1",
            params![key],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some())
}

fn mark_migration_completed(store: &Store, key: &str) -> Result<()> {
    store.with_named_write_transaction("migration.mark_completed", |tx| {
        mark_migration_completed_in_tx(tx, key)?;
        Ok(())
    })
}

pub(super) fn mark_migration_completed_in_tx(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO meta (key, value)
         VALUES (?1, '1')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key],
    )?;
    Ok(())
}
