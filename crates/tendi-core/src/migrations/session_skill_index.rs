use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use crate::{runtime_contract::ScopeKey, storage::Store};

const INDEX_VERSION: &str = "8";
const MIGRATION_KEY_PREFIX: &str = "session_skill_index_version:";

pub(super) fn ensure_current_version(store: &Store, scope_key: &ScopeKey) -> Result<bool> {
    let meta_key = format!("{MIGRATION_KEY_PREFIX}{}", scope_key.as_str());
    if current_version_matches(&store.conn, &meta_key)? {
        return Ok(false);
    }

    store.with_named_write_transaction("migration.session_skill_index_version", |tx| {
        if current_version_matches(tx, &meta_key)? {
            return Ok(false);
        }

        tx.execute(
            "DELETE FROM scoped_session_skill_links WHERE scope_key = ?1",
            [scope_key.as_str()],
        )?;
        tx.execute(
            "DELETE FROM scoped_session_skill_index WHERE scope_key = ?1",
            [scope_key.as_str()],
        )?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![meta_key, INDEX_VERSION],
        )?;
        Ok(true)
    })
}

pub(super) fn invalidate_previous_versions(conn: &rusqlite::Connection) -> Result<()> {
    for table in ["scoped_session_skill_links", "scoped_session_skill_index"] {
        conn.execute(
            &format!(
                "DELETE FROM {table} WHERE NOT EXISTS(SELECT 1 FROM meta
             WHERE key=?1 || {table}.scope_key AND value=?2)"
            ),
            params![MIGRATION_KEY_PREFIX, INDEX_VERSION],
        )?;
    }
    Ok(())
}

fn current_version_matches(conn: &rusqlite::Connection, meta_key: &str) -> Result<bool> {
    let current = conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [meta_key], |row| {
            row.get::<_, String>(0)
        })
        .optional()?;
    Ok(current.as_deref() == Some(INDEX_VERSION))
}

#[cfg(test)]
#[path = "session_skill_index_tests.rs"]
mod tests;
