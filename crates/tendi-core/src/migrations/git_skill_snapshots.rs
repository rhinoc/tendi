use anyhow::Result;
use rusqlite::{Connection, params};

use crate::storage::Store;

const MIGRATION_KEY: &str = "storage.git_skill_snapshots_removed_v1";

pub(super) fn purge(store: &Store) -> Result<()> {
    if super::migration_completed(store, MIGRATION_KEY)? {
        return Ok(());
    }

    store.with_named_write_transaction("migration.purge_git_skill_snapshots", |tx| {
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key = ?1)",
            [MIGRATION_KEY],
            |row| row.get::<_, bool>(0),
        )? {
            return Ok(());
        }

        let mut deleted_rows = 0;
        for (scope_key, skill_path, source_kind) in git_skill_snapshot_keys(tx)? {
            deleted_rows += tx.execute(
                "DELETE FROM scoped_skill_snapshots
                 WHERE scope_key = ?1 AND skill_path = ?2
                   AND EXISTS (
                       SELECT 1 FROM scoped_skill_sources
                       WHERE scope_key = ?1 AND skill_path = ?2 AND source_kind = ?3
                )",
                params![scope_key, skill_path, source_kind],
            )?;
        }
        if deleted_rows > 0 {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES ('storage.compaction.pending', '1')
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [],
            )?;
        }
        super::mark_migration_completed_in_tx(tx, MIGRATION_KEY)
    })
}

fn git_skill_snapshot_keys(conn: &Connection) -> Result<Vec<(String, String, String)>> {
    let mut statement = conn.prepare(
        "SELECT DISTINCT sources.scope_key, sources.skill_path, sources.source_kind
         FROM scoped_skill_sources AS sources
         JOIN scoped_skill_snapshots AS snapshots
           ON snapshots.scope_key = sources.scope_key
          AND snapshots.skill_path = sources.skill_path",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter(|(_, _, source_kind)| crate::skills::is_git_source_kind(source_kind))
        .collect())
}
