use anyhow::Result;

use crate::storage::STORAGE_SCHEMA_VERSION;
use rusqlite::Connection;

pub(super) fn bootstrap(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
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
            PRIMARY KEY (scope_key, skill_path)
        );
        CREATE INDEX IF NOT EXISTS idx_scoped_skill_visibility_path
            ON scoped_skill_visibility(skill_path);
        CREATE TABLE IF NOT EXISTS fs_manifest (
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
            analytics_json TEXT NOT NULL,
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
        CREATE TABLE IF NOT EXISTS scoped_session_search_records (
            scope_key TEXT NOT NULL,
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            agent TEXT NOT NULL,
            session_path TEXT NOT NULL,
            record_order INTEGER NOT NULL,
            metadata_text TEXT NOT NULL DEFAULT '',
            title TEXT NOT NULL DEFAULT '',
            project TEXT NOT NULL DEFAULT '',
            user_text TEXT NOT NULL,
            assistant_text TEXT NOT NULL,
            UNIQUE (scope_key, session_id, agent, session_path, record_order)
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
    Ok(())
}

pub(super) fn needs_current_shape(conn: &Connection) -> Result<bool> {
    Ok(!column_exists(
        conn,
        "scoped_session_search_index",
        "search_checkpoint",
    )?)
}

/// Upgrade the unversioned schema shipped in v0.1.x directly to the current
/// shape. Unreleased scoped-schema revisions are not migration boundaries.
/// New scoped projections start empty and are populated by normal source scans.
pub(super) fn run(conn: &Connection) -> Result<()> {
    ensure_prompt_tags_column(conn)?;
    ensure_session_search_index_version_column(conn)?;
    ensure_scoped_session_search_checkpoint_column(conn)?;
    ensure_storage_indexes(conn)?;
    ensure_session_search_fts(conn)?;
    conn.execute_batch(&format!(
        "PRAGMA user_version = {};",
        STORAGE_SCHEMA_VERSION
    ))?;
    Ok(())
}

fn ensure_storage_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_fs_manifest_resource_scope
            ON fs_manifest(source_kind, resource_path, scope_key);
        CREATE INDEX IF NOT EXISTS idx_prompts_updated_title
            ON prompts(updated_at DESC, title ASC);
        CREATE INDEX IF NOT EXISTS idx_assistant_chat_sessions_updated
            ON assistant_chat_sessions(updated_at DESC, id ASC);
        CREATE INDEX IF NOT EXISTS idx_assistant_chat_messages_session
            ON assistant_chat_messages(session_id, id ASC);
        ",
    )?;
    Ok(())
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

fn ensure_session_search_fts(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS scoped_session_search_fts USING fts5(
            metadata_text,
            title,
            project,
            user_text,
            assistant_text,
            content = 'scoped_session_search_records',
            content_rowid = 'id',
            tokenize = 'trigram case_sensitive 0'
        );
        CREATE TRIGGER IF NOT EXISTS scoped_session_search_records_ai
        AFTER INSERT ON scoped_session_search_records BEGIN
            INSERT INTO scoped_session_search_fts(
                rowid, metadata_text, title, project,
                user_text, assistant_text
            )
            VALUES (
                new.id, new.metadata_text, new.title, new.project,
                new.user_text, new.assistant_text
            );
        END;
        CREATE TRIGGER IF NOT EXISTS scoped_session_search_records_ad
        AFTER DELETE ON scoped_session_search_records BEGIN
            INSERT INTO scoped_session_search_fts(
                scoped_session_search_fts, rowid, metadata_text, title, project,
                user_text, assistant_text
            )
            VALUES (
                'delete', old.id, old.metadata_text, old.title, old.project,
                old.user_text, old.assistant_text
            );
        END;
        CREATE TRIGGER IF NOT EXISTS scoped_session_search_records_au
        AFTER UPDATE ON scoped_session_search_records BEGIN
            INSERT INTO scoped_session_search_fts(
                scoped_session_search_fts, rowid, metadata_text, title, project,
                user_text, assistant_text
            )
            VALUES (
                'delete', old.id, old.metadata_text, old.title, old.project,
                old.user_text, old.assistant_text
            );
            INSERT INTO scoped_session_search_fts(
                rowid, metadata_text, title, project,
                user_text, assistant_text
            )
            VALUES (
                new.id, new.metadata_text, new.title, new.project,
                new.user_text, new.assistant_text
            );
        END;
        ",
    )?;
    Ok(())
}
