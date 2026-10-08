use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::{Transaction, params};

use crate::storage::{Store, canonical_workspace_root};

const MIGRATION_KEY: &str = "skill_visibility_data_migrated_v1";
const INSTALLATION_SCOPE_KEY: &str = "installation:default";

pub(super) fn migrate_legacy_data(store: &Store) -> Result<()> {
    if super::migration_completed(store, MIGRATION_KEY)? {
        return Ok(());
    }

    store.with_named_write_transaction("migration.skill_visibility_data", |tx| {
        if migration_completed_in_tx(tx)? {
            return Ok(());
        }

        tx.execute("DELETE FROM scoped_skill_visibility WHERE locked = 0", [])?;
        migrate_global_skill_visibility_scopes(tx)?;
        // Skills projections are owned by workspace scopes. Older builds
        // accidentally created an installation-scope dirty receipt while
        // persisting global visibility; it has no worker that can consume it.
        tx.execute(
            "DELETE FROM projection_dirty_resources
             WHERE scope_key = ?1 AND domain = 'skills'",
            params![INSTALLATION_SCOPE_KEY],
        )?;
        super::mark_migration_completed_in_tx(tx, MIGRATION_KEY)
    })
}

fn migration_completed_in_tx(tx: &Transaction<'_>) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key = ?1)",
        [MIGRATION_KEY],
        |row| row.get(0),
    )?)
}

fn canonical_skill_visibility_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn migrate_global_skill_visibility_scopes(tx: &Transaction<'_>) -> Result<()> {
    let mut statement = tx.prepare(
        "SELECT scope_key, skill_path, visibility, locked
         FROM scoped_skill_visibility
         WHERE scope_key LIKE 'workspace:%'
         ORDER BY scope_key, skill_path",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    for (scope_key, skill_path, visibility, locked) in rows {
        let Some(workspace) = scope_key.strip_prefix("workspace:") else {
            continue;
        };
        let workspace = PathBuf::from(workspace);
        let skill_path_buf = PathBuf::from(&skill_path);
        let canonical_workspace = canonical_workspace_root(&workspace);
        let canonical_skill_path = canonical_skill_visibility_path(&skill_path_buf);
        if canonical_skill_path.starts_with(&canonical_workspace) {
            continue;
        }
        tx.execute(
            "INSERT INTO scoped_skill_visibility (scope_key, skill_path, visibility, locked)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(scope_key, skill_path) DO UPDATE SET
                visibility = CASE
                    WHEN excluded.locked AND NOT scoped_skill_visibility.locked
                    THEN excluded.visibility
                    ELSE scoped_skill_visibility.visibility
                END,
                locked = MAX(scoped_skill_visibility.locked, excluded.locked)",
            params![
                INSTALLATION_SCOPE_KEY,
                canonical_skill_path.display().to_string(),
                visibility,
                locked,
            ],
        )?;
        tx.execute(
            "DELETE FROM scoped_skill_visibility
             WHERE scope_key = ?1 AND skill_path = ?2",
            params![scope_key, skill_path],
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "skill_visibility_tests.rs"]
mod tests;
