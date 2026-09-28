use anyhow::{Context, Result, bail};

use crate::storage::{ANALYTICS_JSON_ENCODING, STORAGE_SCHEMA_VERSION, compress_analytics_json};
use rusqlite::{Connection, OptionalExtension, params, types::Value};

pub(super) const SESSION_LIST_MIGRATION_KEY: &str = "storage.scoped_session_list.v2";
pub(super) const ANALYTICS_MIGRATION_KEY: &str = "storage.analytics_json.v2";
const SESSION_LIST_MIGRATION_BATCH_SIZE: i64 = 128;
const ANALYTICS_MIGRATION_BATCH_SIZE: i64 = 32;
pub(super) const FS_MANIFEST_REBUILD_PENDING_KEY: &str = "storage.fs_manifest_rebuild_pending";

pub(super) fn bootstrap(conn: &Connection) -> Result<()> {
    let fs_manifest_missing = !table_exists(conn, "fs_manifest")?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS storage_migrations (
            key TEXT PRIMARY KEY,
            state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'completed', 'failed')),
            cursor INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0,
            last_error TEXT
        );
        CREATE TABLE IF NOT EXISTS scoped_skill_sources (
            scope_key TEXT NOT NULL,
            skill_path TEXT NOT NULL,
            skill_name TEXT NOT NULL,
            source_kind TEXT NOT NULL,
            source TEXT,
            source_ref TEXT,
            source_version TEXT,
            source_relative_path TEXT,
            update_status TEXT NOT NULL,
            origin TEXT NOT NULL,
            data_json TEXT NOT NULL,
            PRIMARY KEY (scope_key, skill_path)
        );
        CREATE TABLE IF NOT EXISTS scoped_skill_snapshots (
            scope_key TEXT NOT NULL,
            skill_path TEXT NOT NULL,
            source_version TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            content BLOB NOT NULL,
            PRIMARY KEY (scope_key, skill_path, relative_path)
        );
        CREATE INDEX IF NOT EXISTS idx_scoped_skill_sources_name
            ON scoped_skill_sources(scope_key, skill_name);
        CREATE INDEX IF NOT EXISTS idx_scoped_skill_snapshots_path
            ON scoped_skill_snapshots(scope_key, skill_path);
        CREATE TABLE IF NOT EXISTS scoped_skill_visibility (
            scope_key TEXT NOT NULL,
            skill_path TEXT NOT NULL,
            visibility TEXT NOT NULL,
            locked INTEGER NOT NULL DEFAULT 0 CHECK (locked IN (0, 1)),
            PRIMARY KEY (scope_key, skill_path)
        );
        CREATE INDEX IF NOT EXISTS idx_scoped_skill_visibility_path
            ON scoped_skill_visibility(skill_path);
        CREATE TABLE IF NOT EXISTS normalized_snapshots (
            scope_key TEXT NOT NULL,
            domain TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            source_version TEXT,
            revision INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (scope_key, domain)
        );
        CREATE TABLE IF NOT EXISTS scoped_projection_contexts (
            scope_key TEXT NOT NULL,
            domain TEXT NOT NULL,
            state TEXT NOT NULL,
            scanned_at INTEGER,
            error TEXT,
            parser_version TEXT NOT NULL,
            PRIMARY KEY (scope_key, domain)
        );
        CREATE INDEX IF NOT EXISTS idx_normalized_snapshots_domain
            ON normalized_snapshots(domain, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_scoped_projection_contexts_domain
            ON scoped_projection_contexts(domain, state, scanned_at DESC);
        CREATE TABLE IF NOT EXISTS projection_heads (
            scope_key TEXT NOT NULL,
            domain TEXT NOT NULL,
            revision INTEGER NOT NULL,
            source_version TEXT,
            schema_version INTEGER NOT NULL DEFAULT 1,
            status TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (scope_key, domain)
        );
        CREATE TABLE IF NOT EXISTS operation_journal (
            operation_id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            scope_key TEXT NOT NULL,
            status TEXT NOT NULL,
            input_revision INTEGER NOT NULL,
            source_version TEXT,
            checkpoint_json TEXT,
            error TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_operation_journal_scope_status
            ON operation_journal(scope_key, status, updated_at);
        CREATE TABLE IF NOT EXISTS scoped_sessions (
            scope_key TEXT NOT NULL,
            id TEXT NOT NULL,
            agent TEXT NOT NULL,
            title TEXT,
            project TEXT,
            path TEXT NOT NULL,
            started_at TEXT,
            updated_at TEXT,
            message_count INTEGER,
            first_user_message TEXT,
            last_user_message TEXT,
            last_assistant_message TEXT,
            repository TEXT,
            repository_url TEXT,
            logical_project_id TEXT,
            logical_project_name TEXT,
            started_at_ms INTEGER,
            updated_at_ms INTEGER,
            turn_count INTEGER,
            parent_session_id TEXT,
            input_tokens INTEGER,
            cached_input_tokens INTEGER,
            data_json TEXT NOT NULL,
            PRIMARY KEY (scope_key, id, agent, path)
        );
        CREATE INDEX IF NOT EXISTS idx_scoped_sessions_updated_id
            ON scoped_sessions(updated_at DESC, id ASC);
        CREATE INDEX IF NOT EXISTS idx_scoped_sessions_agent_updated_id
            ON scoped_sessions(agent, updated_at DESC, id ASC);
        CREATE INDEX IF NOT EXISTS idx_scoped_sessions_path
            ON scoped_sessions(path);
        CREATE TABLE IF NOT EXISTS scoped_session_scan_sources (
            scope_key TEXT NOT NULL,
            session_id TEXT NOT NULL,
            agent TEXT NOT NULL,
            session_path TEXT NOT NULL,
            source_path TEXT NOT NULL,
            file_mtime INTEGER NOT NULL,
            file_size INTEGER NOT NULL,
            parser_version TEXT NOT NULL,
            PRIMARY KEY (scope_key, session_id, agent, session_path, source_path)
        );
        CREATE INDEX IF NOT EXISTS idx_scoped_session_scan_sources_source
            ON scoped_session_scan_sources(scope_key, source_path);
        CREATE TABLE IF NOT EXISTS session_projects (
            scope_key TEXT NOT NULL DEFAULT 'installation:default',
            id TEXT NOT NULL,
            name TEXT NOT NULL,
            name_custom INTEGER NOT NULL DEFAULT 0,
            last_seen_at TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (scope_key, id)
        );
        CREATE TABLE IF NOT EXISTS session_project_aliases (
            scope_key TEXT NOT NULL DEFAULT 'installation:default',
            project_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            value TEXT NOT NULL,
            PRIMARY KEY (scope_key, kind, value)
        );
        CREATE INDEX IF NOT EXISTS idx_session_project_aliases_project
            ON session_project_aliases(project_id);
        CREATE TABLE IF NOT EXISTS project_scan_scopes (
            scope_key TEXT NOT NULL DEFAULT 'installation:default',
            id TEXT PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            enabled INTEGER NOT NULL DEFAULT 1,
            last_scanned_at TEXT
        );
        CREATE TABLE IF NOT EXISTS projects (
            scope_key TEXT NOT NULL DEFAULT 'installation:default',
            id TEXT PRIMARY KEY,
            root_path TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            remote_url TEXT,
            scope_id TEXT NOT NULL,
            status TEXT NOT NULL,
            last_scanned_at TEXT NOT NULL,
            data_json TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_projects_scope_status
            ON projects(scope_id, status);
        CREATE TABLE IF NOT EXISTS scoped_session_skill_index (
            scope_key TEXT NOT NULL,
            session_id TEXT NOT NULL,
            agent TEXT NOT NULL,
            session_path TEXT NOT NULL,
            file_mtime INTEGER NOT NULL,
            file_size INTEGER NOT NULL,
            indexed_at TEXT,
            status TEXT NOT NULL,
            error TEXT,
            PRIMARY KEY (scope_key, session_id, agent, session_path)
        );
        CREATE TABLE IF NOT EXISTS scoped_session_skill_links (
            scope_key TEXT NOT NULL,
            session_id TEXT NOT NULL,
            agent TEXT NOT NULL,
            session_path TEXT NOT NULL,
            skill_name TEXT NOT NULL,
            skill_path TEXT NOT NULL,
            skill_agent TEXT,
            skill_scope TEXT,
            evidence_kind TEXT NOT NULL,
            evidence_text TEXT NOT NULL,
            evidence_time TEXT,
            confidence TEXT NOT NULL,
            PRIMARY KEY (scope_key, session_id, agent, session_path, skill_path)
        );
        CREATE TABLE IF NOT EXISTS scoped_session_analytics (
            scope_key TEXT NOT NULL,
            session_id TEXT NOT NULL,
            agent TEXT NOT NULL,
            session_path TEXT NOT NULL,
            file_mtime INTEGER NOT NULL,
            file_size INTEGER NOT NULL,
            indexed_at TEXT NOT NULL,
            analytics_json BLOB NOT NULL,
            parser_state_json TEXT NOT NULL,
            event_min_date TEXT,
            event_max_date TEXT,
            has_activity INTEGER NOT NULL DEFAULT 0,
            capability_token_usage INTEGER NOT NULL DEFAULT 0,
            capability_reasoning_tokens INTEGER NOT NULL DEFAULT 0,
            capability_explicit_runs INTEGER NOT NULL DEFAULT 0,
            capability_rate_limit_history INTEGER NOT NULL DEFAULT 0,
            overview_indexed INTEGER NOT NULL DEFAULT 1,
            overview_index_error TEXT,
            PRIMARY KEY (scope_key, session_id, agent, session_path)
        );
        CREATE TABLE IF NOT EXISTS scoped_session_analytics_overview (
            scope_key TEXT NOT NULL,
            session_id TEXT NOT NULL,
            agent TEXT NOT NULL,
            session_path TEXT NOT NULL,
            event_min_date TEXT,
            event_max_date TEXT,
            has_activity INTEGER NOT NULL DEFAULT 0,
            overview_json TEXT NOT NULL,
            PRIMARY KEY (scope_key, session_id, agent, session_path)
        );
        CREATE TABLE IF NOT EXISTS scoped_session_search_index (
            scope_key TEXT NOT NULL,
            session_id TEXT NOT NULL,
            agent TEXT NOT NULL,
            session_path TEXT NOT NULL,
            file_mtime INTEGER NOT NULL,
            file_size INTEGER NOT NULL,
            indexed_at TEXT NOT NULL,
            search_metadata TEXT NOT NULL DEFAULT '',
            search_index_version INTEGER NOT NULL DEFAULT 1,
            search_checkpoint TEXT,
            PRIMARY KEY (scope_key, session_id, agent, session_path)
        );
        CREATE TABLE IF NOT EXISTS scoped_session_search_pending (
            scope_key TEXT PRIMARY KEY NOT NULL,
            generation INTEGER NOT NULL,
            requested_at TEXT NOT NULL,
            last_error TEXT
        );
        CREATE TABLE IF NOT EXISTS scoped_session_search_maintenance (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            pending_mutations INTEGER NOT NULL DEFAULT 0,
            last_optimized_at INTEGER NOT NULL DEFAULT 0
        );
        INSERT OR IGNORE INTO scoped_session_search_maintenance(
            id, pending_mutations, last_optimized_at
        ) VALUES (1, 0, 0);
        CREATE TABLE IF NOT EXISTS scoped_session_search_work (
            scope_key TEXT NOT NULL,
            session_id TEXT NOT NULL,
            agent TEXT NOT NULL,
            session_path TEXT NOT NULL,
            generation INTEGER NOT NULL,
            requested_at TEXT NOT NULL,
            last_error TEXT,
            PRIMARY KEY (scope_key, session_id, agent, session_path)
        );
        CREATE INDEX IF NOT EXISTS idx_session_search_work_requested
            ON scoped_session_search_work(scope_key, requested_at);
        CREATE TABLE IF NOT EXISTS projection_dirty_resources (
            scope_key TEXT NOT NULL,
            domain TEXT NOT NULL,
            resource_key TEXT NOT NULL,
            generation INTEGER NOT NULL,
            reconcile_generation INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (scope_key, domain, resource_key)
        );
        CREATE INDEX IF NOT EXISTS idx_projection_dirty_domain_scope
            ON projection_dirty_resources(domain, scope_key);
        CREATE TABLE IF NOT EXISTS session_search_content_records (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content_hash TEXT NOT NULL UNIQUE,
            user_text TEXT NOT NULL,
            assistant_text TEXT NOT NULL
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS session_search_content_fts USING fts5(
            user_text,
            assistant_text,
            content = '',
            contentless_delete = 1,
            detail = column,
            tokenize = 'trigram case_sensitive 0'
        );
        CREATE TABLE IF NOT EXISTS scoped_session_search_entries (
            scope_key TEXT NOT NULL,
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            agent TEXT NOT NULL,
            session_path TEXT NOT NULL,
            record_order INTEGER NOT NULL,
            metadata_text TEXT NOT NULL DEFAULT '',
            title TEXT NOT NULL DEFAULT '',
            project TEXT NOT NULL DEFAULT '',
            content_id INTEGER NOT NULL,
            UNIQUE (scope_key, session_id, agent, session_path, record_order)
        );
        CREATE INDEX IF NOT EXISTS idx_scoped_session_search_entries_content
            ON scoped_session_search_entries(content_id);
        CREATE VIRTUAL TABLE IF NOT EXISTS scoped_session_search_metadata_fts USING fts5(
            metadata_text,
            title,
            project,
            content = '',
            contentless_delete = 1,
            detail = column,
            tokenize = 'trigram case_sensitive 0'
        );
        CREATE INDEX IF NOT EXISTS idx_scoped_session_skill_links_session
            ON scoped_session_skill_links(scope_key, session_id, agent);
        CREATE INDEX IF NOT EXISTS idx_scoped_session_skill_links_skill
            ON scoped_session_skill_links(scope_key, skill_name);
        CREATE TABLE IF NOT EXISTS prompts (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            category TEXT NOT NULL,
            tags_json TEXT NOT NULL DEFAULT '[]',
            body TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS assistant_chat_sessions (
            id TEXT PRIMARY KEY,
            workspace TEXT NOT NULL,
            linked_session_id TEXT,
            linked_session_agent TEXT,
            linked_session_path TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS assistant_chat_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
            content TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (session_id) REFERENCES assistant_chat_sessions(id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS app_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )?;
    ensure_fs_manifest(conn)?;
    if fs_manifest_missing {
        mark_all_fs_manifest_projections_stale(conn)?;
        conn.execute(
            "DELETE FROM meta WHERE key = ?1",
            [FS_MANIFEST_REBUILD_PENDING_KEY],
        )?;
    }
    Ok(())
}

fn ensure_fs_manifest(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS fs_manifest (
            scope_key TEXT NOT NULL DEFAULT 'installation:default',
            source_kind TEXT NOT NULL,
            path TEXT NOT NULL,
            root TEXT NOT NULL,
            agent TEXT,
            scope TEXT,
            mtime_ns INTEGER,
            size INTEGER,
            inode INTEGER,
            device INTEGER,
            sha256 TEXT,
            parser_version TEXT NOT NULL,
            last_seen_at INTEGER NOT NULL,
            parse_status TEXT NOT NULL,
            resource_path TEXT,
            PRIMARY KEY (scope_key, source_kind, path)
        );
        CREATE INDEX IF NOT EXISTS idx_fs_manifest_root_kind_path
            ON fs_manifest(scope_key, root, source_kind, path);
        CREATE INDEX IF NOT EXISTS idx_fs_manifest_resource_scope
            ON fs_manifest(source_kind, resource_path, scope_key);",
    )?;
    Ok(())
}

pub(super) fn rebuild_corrupt_fs_manifest(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO meta (key, value) VALUES (?1, '1')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [FS_MANIFEST_REBUILD_PENDING_KEY],
    )?;
    tx.commit()?;

    conn.execute_batch(
        "PRAGMA writable_schema = ON;
         DELETE FROM sqlite_master WHERE tbl_name = 'fs_manifest';
         PRAGMA writable_schema = OFF;
         VACUUM;",
    )?;
    ensure_fs_manifest(conn)?;
    finish_pending_fs_manifest_rebuild(conn)?;
    Ok(())
}

pub(super) fn finish_pending_fs_manifest_rebuild(conn: &Connection) -> Result<bool> {
    let pending: bool = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM meta WHERE key = ?1 AND value = '1'
         )",
        [FS_MANIFEST_REBUILD_PENDING_KEY],
        |row| row.get(0),
    )?;
    if !pending {
        return Ok(false);
    }
    let tx = conn.unchecked_transaction()?;
    mark_all_fs_manifest_projections_stale(&tx)?;
    tx.execute(
        "DELETE FROM meta WHERE key = ?1",
        [FS_MANIFEST_REBUILD_PENDING_KEY],
    )?;
    tx.commit()?;
    Ok(true)
}

fn mark_all_fs_manifest_projections_stale(conn: &Connection) -> Result<()> {
    let projections = {
        let mut statement = conn.prepare(
            "SELECT scope_key, domain FROM scoped_projection_contexts
             UNION
             SELECT scope_key, domain FROM projection_heads",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (scope_key, domain) in projections {
        conn.execute(
            "UPDATE scoped_projection_contexts
             SET state = 'stale', scanned_at = NULL, error = NULL
             WHERE scope_key = ?1 AND domain = ?2",
            rusqlite::params![scope_key, domain],
        )?;
        conn.execute(
            "INSERT INTO projection_heads (
                scope_key, domain, revision, source_version, schema_version, status, updated_at
             ) VALUES (
                ?1, ?2, 1, NULL, 1, 'stale', CAST(strftime('%s', 'now') AS INTEGER)
             )
             ON CONFLICT(scope_key, domain) DO UPDATE SET
                revision = projection_heads.revision + 1,
                source_version = NULL,
                status = 'stale',
                updated_at = excluded.updated_at",
            rusqlite::params![scope_key, domain],
        )?;
        let revision: i64 = conn.query_row(
            "SELECT revision FROM projection_heads WHERE scope_key = ?1 AND domain = ?2",
            rusqlite::params![scope_key, domain],
            |row| row.get(0),
        )?;
        conn.execute(
            "INSERT INTO projection_dirty_resources (
                scope_key, domain, resource_key, generation, reconcile_generation
             ) VALUES (?1, ?2, '', ?3, 0)
             ON CONFLICT(scope_key, domain, resource_key) DO UPDATE SET
                generation = excluded.generation",
            rusqlite::params![scope_key, domain, revision],
        )?;
    }
    Ok(())
}

pub(super) fn rebuild_scoped_session_skill_links(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA writable_schema = ON;
         DELETE FROM sqlite_master
          WHERE name IN (
              'scoped_session_skill_links',
              'sqlite_autoindex_scoped_session_skill_links_1',
              'idx_scoped_session_skill_links_session',
              'idx_scoped_session_skill_links_skill'
          );
         PRAGMA writable_schema = OFF;
         VACUUM;
         CREATE TABLE scoped_session_skill_links (
             scope_key TEXT NOT NULL,
             session_id TEXT NOT NULL,
             agent TEXT NOT NULL,
             session_path TEXT NOT NULL,
             skill_name TEXT NOT NULL,
             skill_path TEXT NOT NULL,
             skill_agent TEXT,
             skill_scope TEXT,
             evidence_kind TEXT NOT NULL,
             evidence_text TEXT NOT NULL,
             evidence_time TEXT,
             confidence TEXT NOT NULL,
             PRIMARY KEY (scope_key, session_id, agent, session_path, skill_path)
         );
         CREATE INDEX idx_scoped_session_skill_links_session
             ON scoped_session_skill_links(scope_key, session_id, agent);
         CREATE INDEX idx_scoped_session_skill_links_skill
             ON scoped_session_skill_links(scope_key, skill_name);",
    )?;
    conn.execute("DELETE FROM scoped_session_skill_index", [])?;
    Ok(())
}

pub(super) fn needs_current_shape(conn: &Connection) -> Result<bool> {
    Ok(!table_exists(conn, "storage_migrations")?
        || !table_exists(conn, "fs_manifest")?
        || !column_exists(conn, "scoped_session_search_index", "search_checkpoint")?
        || !column_exists(conn, "scoped_sessions", "cached_input_tokens")?
        || !column_exists(conn, "scoped_sessions", "started_at_ms")?
        || !column_exists(conn, "scoped_sessions", "updated_at_ms")?
        || !table_exists(conn, "session_search_content_records")?
        || !table_exists(conn, "session_search_content_fts")?
        || !table_exists(conn, "scoped_session_search_entries")?
        || !table_exists(conn, "scoped_session_search_metadata_fts")?
        || !column_exists(conn, "scoped_skill_visibility", "locked")?
        || table_exists(conn, "scoped_session_search_records")?)
}

pub(super) fn has_user_schema(conn: &Connection) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master
             WHERE type IN ('table', 'index', 'trigger', 'view')
               AND name NOT LIKE 'sqlite_%'
         )",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

/// Upgrade the unversioned schema shipped in v0.1.x directly to the current
/// shape. Unreleased scoped-schema revisions are not migration boundaries.
/// New scoped projections start empty and are populated by normal source scans.
pub(super) fn run(conn: &Connection, existing_schema: bool) -> Result<()> {
    let previous_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let analytics_migration_needed = analytics_json_needs_migration(conn)?;
    let legacy_search_records_exist = table_exists(conn, "scoped_session_search_records")?;
    let reset_session_search = existing_schema
        && (previous_version < STORAGE_SCHEMA_VERSION || legacy_search_records_exist);
    ensure_prompt_tags_column(conn)?;
    ensure_skill_visibility_lock_column(conn)?;
    ensure_session_search_index_version_column(conn)?;
    ensure_scoped_session_search_checkpoint_column(conn)?;
    let list_columns_added = ensure_scoped_session_list_columns(conn)?;
    ensure_storage_indexes(conn)?;
    recreate_shared_session_search_triggers(conn)?;
    if reset_session_search {
        drop_legacy_session_search_cache(conn)?;
        queue_session_search_rebuild(conn)?;
    }
    initialize_migration_state(
        conn,
        SESSION_LIST_MIGRATION_KEY,
        existing_schema && (previous_version < STORAGE_SCHEMA_VERSION || list_columns_added),
    )?;
    initialize_migration_state(conn, ANALYTICS_MIGRATION_KEY, analytics_migration_needed)?;
    conn.execute(
        "UPDATE storage_migrations
         SET state = 'completed', cursor = 0,
             updated_at = CAST(strftime('%s', 'now') AS INTEGER), last_error = NULL
         WHERE key IN ('storage.session_search_fts.v2',
                       'storage.session_search_shared_content.v1')",
        [],
    )?;
    if !analytics_migration_needed {
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('storage.analytics_json_encoding', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [ANALYTICS_JSON_ENCODING],
        )?;
    }
    if existing_schema
        && (previous_version < STORAGE_SCHEMA_VERSION
            || analytics_migration_needed
            || reset_session_search)
    {
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('storage.compaction.pending', '1')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [],
        )?;
    }
    conn.execute_batch(&format!(
        "PRAGMA user_version = {};",
        STORAGE_SCHEMA_VERSION
    ))?;
    Ok(())
}

fn ensure_skill_visibility_lock_column(conn: &Connection) -> Result<()> {
    if column_exists(conn, "scoped_skill_visibility", "locked")? {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE scoped_skill_visibility
         ADD COLUMN locked INTEGER NOT NULL DEFAULT 0 CHECK (locked IN (0, 1));",
    )?;
    Ok(())
}

fn drop_legacy_session_search_cache(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS scoped_session_search_records_ai;
         DROP TRIGGER IF EXISTS scoped_session_search_records_ad;
         DROP TRIGGER IF EXISTS scoped_session_search_records_au;
         DROP TRIGGER IF EXISTS scoped_session_search_records_shared_ai;
         DROP TRIGGER IF EXISTS scoped_session_search_records_shared_ad;
         DROP TRIGGER IF EXISTS scoped_session_search_records_shared_au;
         DROP TABLE IF EXISTS scoped_session_search_fts_migration_v2;
         DROP TABLE IF EXISTS scoped_session_search_fts;
         DROP TABLE IF EXISTS scoped_session_search_records;
         DELETE FROM scoped_session_search_entries;
         DELETE FROM session_search_content_records;",
    )?;
    Ok(())
}

fn queue_session_search_rebuild(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "UPDATE scoped_session_search_index SET search_index_version = 0;
         INSERT INTO scoped_session_search_work(
             scope_key, session_id, agent, session_path, generation,
             requested_at, last_error
         )
         SELECT scope_key, session_id, agent, session_path, 1,
                CAST(strftime('%s', 'now') AS TEXT), NULL
         FROM (
             SELECT scope_key, id AS session_id, agent, path AS session_path
             FROM scoped_sessions
             UNION
             SELECT scope_key, session_id, agent, session_path
             FROM scoped_session_search_index
         ) WHERE true
         ON CONFLICT(scope_key, session_id, agent, session_path) DO UPDATE SET
             generation = generation + 1,
             requested_at = excluded.requested_at,
             last_error = NULL;
         DELETE FROM scoped_session_search_pending
         WHERE NOT EXISTS (
             SELECT 1 FROM scoped_session_search_work
             WHERE scoped_session_search_work.scope_key = scoped_session_search_pending.scope_key
         );
         INSERT INTO scoped_session_search_pending(
             scope_key, generation, requested_at, last_error
         )
         SELECT scope_key, 1, CAST(strftime('%s', 'now') AS TEXT), NULL
         FROM scoped_session_search_work
         WHERE true
         GROUP BY scope_key
         ON CONFLICT(scope_key) DO UPDATE SET
             generation = generation + 1,
             requested_at = excluded.requested_at,
             last_error = NULL;",
    )?;
    Ok(())
}

fn ensure_storage_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_prompts_updated_title
            ON prompts(updated_at DESC, title ASC);
        CREATE INDEX IF NOT EXISTS idx_assistant_chat_sessions_updated
            ON assistant_chat_sessions(updated_at DESC, id ASC);
        CREATE INDEX IF NOT EXISTS idx_assistant_chat_messages_session
            ON assistant_chat_messages(session_id, id ASC);
        CREATE INDEX IF NOT EXISTS idx_scoped_sessions_scope_updated_id
            ON scoped_sessions(scope_key, updated_at DESC, id ASC);
        CREATE INDEX IF NOT EXISTS idx_scoped_sessions_scope_agent_updated_id
            ON scoped_sessions(scope_key, agent, updated_at DESC, id ASC);
        CREATE INDEX IF NOT EXISTS idx_scoped_sessions_scope_started_id
            ON scoped_sessions(scope_key, started_at DESC, id ASC);
        CREATE INDEX IF NOT EXISTS idx_scoped_sessions_scope_message_id
            ON scoped_sessions(scope_key, message_count DESC, id ASC);
        CREATE INDEX IF NOT EXISTS idx_scoped_sessions_scope_turn_id
            ON scoped_sessions(scope_key, turn_count DESC, id ASC);
        DROP INDEX IF EXISTS idx_scoped_session_search_records_session;
        ",
    )?;
    Ok(())
}

fn ensure_scoped_session_list_columns(conn: &Connection) -> Result<bool> {
    let mut added = false;
    for (name, definition) in [
        ("repository", "TEXT"),
        ("repository_url", "TEXT"),
        ("logical_project_id", "TEXT"),
        ("logical_project_name", "TEXT"),
        ("started_at_ms", "INTEGER"),
        ("updated_at_ms", "INTEGER"),
        ("turn_count", "INTEGER"),
        ("parent_session_id", "TEXT"),
        ("input_tokens", "INTEGER"),
        ("cached_input_tokens", "INTEGER"),
    ] {
        if !column_exists(conn, "scoped_sessions", name)? {
            conn.execute(
                &format!("ALTER TABLE scoped_sessions ADD COLUMN {name} {definition}"),
                [],
            )?;
            added = true;
        }
    }
    Ok(added)
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    Ok(stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .any(|name| name == column))
}

fn ensure_session_search_index_version_column(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare("PRAGMA table_info(scoped_session_search_index)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if !columns
        .iter()
        .any(|column| column == "search_index_version")
    {
        conn.execute(
            "ALTER TABLE scoped_session_search_index
             ADD COLUMN search_index_version INTEGER NOT NULL DEFAULT 1",
            [],
        )?;
    }
    Ok(())
}

fn ensure_scoped_session_search_checkpoint_column(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "scoped_session_search_index", "search_checkpoint")? {
        conn.execute(
            "ALTER TABLE scoped_session_search_index ADD COLUMN search_checkpoint TEXT",
            [],
        )?;
    }
    Ok(())
}

fn ensure_prompt_tags_column(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(prompts)")?;
    let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut has_tags_json = false;
    for column in columns {
        if column? == "tags_json" {
            has_tags_json = true;
            break;
        }
    }
    if !has_tags_json {
        conn.execute(
            "ALTER TABLE prompts ADD COLUMN tags_json TEXT NOT NULL DEFAULT '[]'",
            [],
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MigrationBatch {
    pub processed: usize,
    pub complete: bool,
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1
         )",
        [table],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn analytics_json_needs_migration(conn: &Connection) -> Result<bool> {
    let declared_type: Option<String> = conn
        .query_row(
            "SELECT type FROM pragma_table_info('scoped_session_analytics')
             WHERE name = 'analytics_json'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(declared_type) = declared_type else {
        return Ok(false);
    };
    let encoded = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'storage.analytics_json_encoding'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(encoded.as_deref() != Some(ANALYTICS_JSON_ENCODING)
        && !declared_type.eq_ignore_ascii_case("BLOB"))
}

fn recreate_shared_session_search_triggers(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS session_search_content_records_ai;
         DROP TRIGGER IF EXISTS session_search_content_records_ad;
         DROP TRIGGER IF EXISTS session_search_content_records_au;
         DROP TRIGGER IF EXISTS scoped_session_search_entries_ai;
         DROP TRIGGER IF EXISTS scoped_session_search_entries_ad;
         DROP TRIGGER IF EXISTS scoped_session_search_entries_au;
         CREATE TRIGGER session_search_content_records_ai
         AFTER INSERT ON session_search_content_records BEGIN
             INSERT INTO session_search_content_fts(
                 rowid, user_text, assistant_text
             ) VALUES (new.id, new.user_text, new.assistant_text);
         END;
         CREATE TRIGGER session_search_content_records_ad
         AFTER DELETE ON session_search_content_records BEGIN
             DELETE FROM session_search_content_fts WHERE rowid = old.id;
         END;
         CREATE TRIGGER session_search_content_records_au
         AFTER UPDATE ON session_search_content_records BEGIN
             DELETE FROM session_search_content_fts WHERE rowid = old.id;
             INSERT INTO session_search_content_fts(
                 rowid, user_text, assistant_text
             ) VALUES (new.id, new.user_text, new.assistant_text);
         END;
         CREATE TRIGGER scoped_session_search_entries_ai
         AFTER INSERT ON scoped_session_search_entries BEGIN
             INSERT INTO scoped_session_search_metadata_fts(
                 rowid, metadata_text, title, project
             ) VALUES (new.id, new.metadata_text, new.title, new.project);
         END;
         CREATE TRIGGER scoped_session_search_entries_ad
         AFTER DELETE ON scoped_session_search_entries BEGIN
             DELETE FROM scoped_session_search_metadata_fts WHERE rowid = old.id;
             DELETE FROM session_search_content_records
              WHERE id = old.content_id
                AND NOT EXISTS (
                    SELECT 1 FROM scoped_session_search_entries
                    WHERE content_id = old.content_id
                );
         END;
         CREATE TRIGGER scoped_session_search_entries_au
         AFTER UPDATE ON scoped_session_search_entries BEGIN
             DELETE FROM scoped_session_search_metadata_fts WHERE rowid = old.id;
             INSERT INTO scoped_session_search_metadata_fts(
                 rowid, metadata_text, title, project
             ) VALUES (new.id, new.metadata_text, new.title, new.project);
             DELETE FROM session_search_content_records
              WHERE id = old.content_id
                AND old.content_id != new.content_id
                AND NOT EXISTS (
                    SELECT 1 FROM scoped_session_search_entries
                    WHERE content_id = old.content_id
                );
         END;",
    )?;
    Ok(())
}

fn initialize_migration_state(conn: &Connection, key: &str, pending: bool) -> Result<()> {
    let state = if pending { "pending" } else { "completed" };
    conn.execute(
        "INSERT OR IGNORE INTO storage_migrations(key, state, cursor, updated_at, last_error)
         VALUES (?1, ?2, 0, CAST(strftime('%s', 'now') AS INTEGER), NULL)",
        params![key, state],
    )?;
    Ok(())
}

fn migration_state(tx: &rusqlite::Transaction<'_>, key: &str) -> Result<Option<(String, i64)>> {
    tx.query_row(
        "SELECT state, cursor FROM storage_migrations WHERE key = ?1",
        [key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(Into::into)
}

fn update_migration_state(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    state: &str,
    cursor: i64,
) -> Result<()> {
    tx.execute(
        "UPDATE storage_migrations
         SET state = ?1, cursor = ?2, updated_at = CAST(strftime('%s', 'now') AS INTEGER),
             last_error = NULL
         WHERE key = ?3",
        params![state, cursor, key],
    )?;
    Ok(())
}

pub(super) fn migrate_session_list_batch(tx: &rusqlite::Transaction<'_>) -> Result<MigrationBatch> {
    let Some((state, cursor)) = migration_state(tx, SESSION_LIST_MIGRATION_KEY)? else {
        return Ok(MigrationBatch {
            processed: 0,
            complete: true,
        });
    };
    if state == "completed" {
        return Ok(MigrationBatch {
            processed: 0,
            complete: true,
        });
    }
    let rows = {
        let mut statement = tx.prepare(
            "SELECT rowid, data_json FROM scoped_sessions
             WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
        )?;
        statement
            .query_map(params![cursor, SESSION_LIST_MIGRATION_BATCH_SIZE], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    if rows.is_empty() {
        update_migration_state(tx, SESSION_LIST_MIGRATION_KEY, "completed", cursor)?;
        return Ok(MigrationBatch {
            processed: 0,
            complete: true,
        });
    }

    let mut next_cursor = cursor;
    for (rowid, data_json) in &rows {
        let value = serde_json::from_str::<serde_json::Value>(data_json).ok();
        let repository = json_string_field(value.as_ref(), "repository");
        let repository_url = json_string_field(value.as_ref(), "repository_url");
        let logical_project_id = json_string_field(value.as_ref(), "logical_project_id");
        let logical_project_name = json_string_field(value.as_ref(), "logical_project_name");
        let started_at = json_string_field(value.as_ref(), "started_at");
        let updated_at = json_string_field(value.as_ref(), "updated_at");
        let started_at_ms = started_at
            .as_deref()
            .and_then(crate::time::parse_timestamp)
            .map(|value| value.timestamp_millis());
        let updated_at_ms = updated_at
            .as_deref()
            .and_then(crate::time::parse_timestamp)
            .map(|value| value.timestamp_millis());
        let turn_count = json_i64_field(value.as_ref(), "turn_count");
        let parent_session_id = json_string_field(value.as_ref(), "parent_session_id");
        let input_tokens = json_i64_nested_field(value.as_ref(), &["token_usage", "input_tokens"]);
        let cached_input_tokens =
            json_i64_nested_field(value.as_ref(), &["token_usage", "cached_input_tokens"]);
        tx.execute(
            "UPDATE scoped_sessions SET
                repository = ?1, repository_url = ?2,
                logical_project_id = ?3, logical_project_name = ?4,
                started_at_ms = ?5, updated_at_ms = ?6, turn_count = ?7,
                parent_session_id = ?8, input_tokens = ?9, cached_input_tokens = ?10
             WHERE rowid = ?11",
            params![
                repository,
                repository_url,
                logical_project_id,
                logical_project_name,
                started_at_ms,
                updated_at_ms,
                turn_count,
                parent_session_id,
                input_tokens,
                cached_input_tokens,
                rowid,
            ],
        )?;
        next_cursor = *rowid;
    }
    update_migration_state(tx, SESSION_LIST_MIGRATION_KEY, "pending", next_cursor)?;
    Ok(MigrationBatch {
        processed: rows.len(),
        complete: false,
    })
}

fn json_string_field(value: Option<&serde_json::Value>, key: &str) -> Option<String> {
    value
        .and_then(|value| value.get(key))
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
}

fn json_i64_field(value: Option<&serde_json::Value>, key: &str) -> Option<i64> {
    value
        .and_then(|value| value.get(key))
        .and_then(serde_json::Value::as_i64)
}

fn json_i64_nested_field(value: Option<&serde_json::Value>, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .fold(value, |value, key| value.and_then(|value| value.get(*key)))
        .and_then(serde_json::Value::as_i64)
}

pub(super) fn migrate_analytics_json_batch(
    tx: &rusqlite::Transaction<'_>,
) -> Result<MigrationBatch> {
    let Some((state, cursor)) = migration_state(tx, ANALYTICS_MIGRATION_KEY)? else {
        return Ok(MigrationBatch {
            processed: 0,
            complete: true,
        });
    };
    if state == "completed" {
        return Ok(MigrationBatch {
            processed: 0,
            complete: true,
        });
    }
    let rows = {
        let mut statement = tx.prepare(
            "SELECT rowid, analytics_json FROM scoped_session_analytics
             WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
        )?;
        statement
            .query_map(params![cursor, ANALYTICS_MIGRATION_BATCH_SIZE], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Value>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    if rows.is_empty() {
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('storage.analytics_json_encoding', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [ANALYTICS_JSON_ENCODING],
        )?;
        update_migration_state(tx, ANALYTICS_MIGRATION_KEY, "completed", cursor)?;
        return Ok(MigrationBatch {
            processed: 0,
            complete: true,
        });
    }

    let processed = rows.len();
    let mut next_cursor = cursor;
    for (rowid, value) in rows {
        let encoded = match value {
            Value::Text(value) => compress_analytics_json(&value)
                .with_context(|| format!("compress analytics cache row {rowid}"))?,
            Value::Blob(value) if value.starts_with(ANALYTICS_JSON_ENCODING.as_bytes()) => value,
            Value::Blob(value) => {
                let value = String::from_utf8(value)
                    .with_context(|| format!("analytics cache row {rowid} is not UTF-8"))?;
                compress_analytics_json(&value)
                    .with_context(|| format!("compress analytics cache row {rowid}"))?
            }
            other => bail!("analytics cache row {rowid} has unsupported SQLite value {other:?}"),
        };
        tx.execute(
            "UPDATE scoped_session_analytics SET analytics_json = ?1 WHERE rowid = ?2",
            params![encoded, rowid],
        )?;
        next_cursor = rowid;
    }
    update_migration_state(tx, ANALYTICS_MIGRATION_KEY, "pending", next_cursor)?;
    Ok(MigrationBatch {
        processed,
        complete: false,
    })
}
