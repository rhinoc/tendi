//! session_skills persistence through the database-owned transaction boundary.
use super::super::*;
use std::collections::BTreeMap;

pub(in crate::storage) fn cleanup_stale_scoped_session_skill_rows(
    conn: &rusqlite::Transaction<'_>,
    scope_key: &ScopeKey,
) -> Result<()> {
    conn.execute(
        "DELETE FROM scoped_session_skill_links
         WHERE scope_key = ?1
           AND NOT EXISTS (
            SELECT 1 FROM scoped_sessions
            WHERE scoped_sessions.scope_key = scoped_session_skill_links.scope_key
              AND scoped_sessions.id = scoped_session_skill_links.session_id
              AND scoped_sessions.agent = scoped_session_skill_links.agent
              AND scoped_sessions.path = scoped_session_skill_links.session_path
         )",
        [scope_key.as_str()],
    )?;
    conn.execute(
        "DELETE FROM scoped_session_skill_index
         WHERE scope_key = ?1
           AND NOT EXISTS (
            SELECT 1 FROM scoped_sessions
            WHERE scoped_sessions.scope_key = scoped_session_skill_index.scope_key
              AND scoped_sessions.id = scoped_session_skill_index.session_id
              AND scoped_sessions.agent = scoped_session_skill_index.agent
              AND scoped_sessions.path = scoped_session_skill_index.session_path
         )",
        [scope_key.as_str()],
    )?;
    Ok(())
}

impl Store {
    pub fn session_skill_index_status_for_scope(
        &self,
        scope_key: &ScopeKey,
        running: bool,
    ) -> Result<SessionSkillIndexStatus> {
        with_database_read_lock_retry(|| {
            let total = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM scoped_sessions WHERE scope_key = ?1",
                    [scope_key.as_str()],
                    |row| row.get::<_, i64>(0),
                )?
                .max(0) as usize;
            let indexed = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM scoped_session_skill_index
                     WHERE scope_key = ?1 AND status = 'indexed'",
                    [scope_key.as_str()],
                    |row| row.get::<_, i64>(0),
                )?
                .max(0) as usize;
            let failed = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM scoped_session_skill_index
                     WHERE scope_key = ?1 AND status = 'failed'",
                    [scope_key.as_str()],
                    |row| row.get::<_, i64>(0),
                )?
                .max(0) as usize;
            let last_indexed_at = self.conn.query_row(
                "SELECT MAX(indexed_at) FROM scoped_session_skill_index
                 WHERE scope_key = ?1 AND status = 'indexed'",
                [scope_key.as_str()],
                |row| row.get::<_, Option<String>>(0),
            )?;
            Ok(SessionSkillIndexStatus {
                total,
                indexed,
                pending: total.saturating_sub(indexed + failed),
                failed,
                running,
                last_indexed_at,
            })
        })
    }

    pub fn session_skill_links_for_scope(
        &self,
        scope_key: &ScopeKey,
        session_id: &str,
        agent: AgentKind,
    ) -> Result<Vec<SessionSkillLink>> {
        with_database_read_lock_retry(|| {
            self.query_session_skill_links_from(
                "scoped_session_skill_links",
                "scoped_sessions",
                "WHERE links.scope_key = ?1 AND links.session_id = ?2 AND links.agent = ?3",
                params![scope_key.as_str(), session_id, agent_label(agent)],
            )
        })
    }

    pub fn skill_session_links_for_scope(
        &self,
        scope_key: &ScopeKey,
        skill_paths: &[PathBuf],
    ) -> Result<Vec<SessionSkillLink>> {
        if skill_paths.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (2..=skill_paths.len() + 1)
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let where_clause = format!(
            "WHERE links.scope_key = ?1 AND links.skill_path IN ({placeholders})
               AND (sessions.data_json IS NULL
                    OR json_extract(sessions.data_json, '$.parent_session_id') IS NULL)"
        );
        let mut values = Vec::with_capacity(skill_paths.len() + 1);
        values.push(scope_key.as_str().to_string());
        values.extend(skill_paths.iter().map(|path| path.display().to_string()));
        with_database_read_lock_retry(|| {
            self.query_session_skill_links_from(
                "scoped_session_skill_links",
                "scoped_sessions",
                &where_clause,
                rusqlite::params_from_iter(values.iter()),
            )
        })
    }

    pub(in crate::storage) fn query_session_skill_links_from<P>(
        &self,
        links_table: &str,
        sessions_table: &str,
        where_clause: &str,
        params: P,
    ) -> Result<Vec<SessionSkillLink>>
    where
        P: rusqlite::Params,
    {
        // Read only the Session fields Linked Sessions needs from data_json.
        // Scalar session columns are projections, not authority (see list_sessions).
        // ORDER BY mirrors the prior in-memory sort keys; compare_timestamps
        // still re-sorts so RFC3339 offsets and invalid stamps stay equivalent.
        let sql = format!(
            "SELECT
                links.session_id,
                links.agent,
                links.session_path,
                CASE
                  WHEN sessions.data_json IS NULL THEN 0
                  WHEN json_valid(sessions.data_json) THEN 0
                  ELSE 1
                END,
                json_extract(sessions.data_json, '$.title'),
                json_extract(sessions.data_json, '$.project'),
                json_extract(sessions.data_json, '$.started_at'),
                json_extract(sessions.data_json, '$.updated_at'),
                json_extract(sessions.data_json, '$.message_count'),
                links.skill_name,
                links.skill_path,
                links.skill_agent,
                links.skill_scope,
                links.evidence_kind,
                links.evidence_text,
                links.evidence_time,
                links.confidence
             FROM {links_table} links
             LEFT JOIN {sessions_table} sessions
               ON sessions.id = links.session_id
              AND sessions.agent = links.agent
              AND sessions.path = links.session_path
              AND sessions.scope_key = links.scope_key
             {where_clause}
             ORDER BY
                json_extract(sessions.data_json, '$.updated_at') DESC,
                links.evidence_time DESC,
                links.skill_name ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, String>(13)?,
                row.get::<_, String>(14)?,
                row.get::<_, Option<String>>(15)?,
                row.get::<_, String>(16)?,
            ))
        })?;
        let mut links = Vec::new();
        for row in rows {
            let (
                session_id,
                agent,
                session_path,
                invalid_session_json,
                session_title,
                session_project,
                session_started_at,
                session_updated_at,
                session_message_count,
                skill_name,
                skill_path,
                skill_agent,
                skill_scope,
                evidence_kind,
                evidence_text,
                evidence_time,
                confidence,
            ) = row?;
            let agent = parse_agent_label(&agent)
                .ok_or_else(|| anyhow::anyhow!("invalid session skill link agent: {agent}"))?;
            if invalid_session_json != 0 {
                anyhow::bail!(
                    "invalid cached session row for skill link {}:{}",
                    agent_label(agent),
                    session_id
                );
            }
            let skill_agent = skill_agent
                .map(|agent| {
                    parse_agent_label(&agent).ok_or_else(|| {
                        anyhow::anyhow!("invalid session skill link skill agent: {agent}")
                    })
                })
                .transpose()?;
            let message_count = session_message_count
                .map(|count| usize::try_from(count))
                .transpose()
                .with_context(|| {
                    format!(
                        "invalid session message_count for skill link {}:{}",
                        agent_label(agent),
                        session_id
                    )
                })?;
            let link = SessionSkillLink {
                session_id,
                agent,
                session_path: PathBuf::from(session_path),
                session_title: clean_session_title(session_title),
                session_project: session_project.map(PathBuf::from),
                session_started_at,
                session_updated_at,
                session_message_count: message_count,
                skill_name,
                skill_path: PathBuf::from(skill_path),
                skill_agent,
                skill_scope,
                evidence_kind,
                evidence_text,
                evidence_time,
                confidence,
            };
            links.push(link);
        }
        links.sort_by(|left, right| {
            compare_timestamps(
                right.session_updated_at.as_deref(),
                left.session_updated_at.as_deref(),
            )
            .then_with(|| {
                compare_timestamps(
                    right.evidence_time.as_deref(),
                    left.evidence_time.as_deref(),
                )
            })
            .then_with(|| left.skill_name.cmp(&right.skill_name))
        });
        Ok(links)
    }

    pub fn clear_session_skill_index_for_scope(&self, scope_key: &ScopeKey) -> Result<()> {
        self.with_named_write_transaction("clear_session_skill_index_for_scope", |tx| {
            tx.execute(
                "DELETE FROM scoped_session_skill_links WHERE scope_key = ?1",
                [scope_key.as_str()],
            )?;
            tx.execute(
                "DELETE FROM scoped_session_skill_index WHERE scope_key = ?1",
                [scope_key.as_str()],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn ensure_session_skill_index_version_for_scope(
        &self,
        scope_key: &ScopeKey,
        version: &str,
    ) -> Result<bool> {
        let meta_key = format!("session_skill_index_version:{}", scope_key.as_str());
        self.with_named_write_transaction("ensure_session_skill_index_version_for_scope", |tx| {
            let current = tx
                .query_row(
                    "SELECT value FROM meta WHERE key = ?1",
                    [&meta_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if current.as_deref() == Some(version) {
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
                params![meta_key, version],
            )?;
            Ok(true)
        })
    }

    pub fn session_skill_index_is_current_for_scope(
        &self,
        scope_key: &ScopeKey,
        session: &SessionRecord,
        file_mtime: i64,
        file_size: i64,
    ) -> Result<bool> {
        let status = self
            .conn
            .query_row(
                "SELECT status FROM scoped_session_skill_index
                 WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3
                   AND session_path = ?4 AND file_mtime = ?5 AND file_size = ?6",
                params![
                    scope_key.as_str(),
                    session.id,
                    agent_label(session.agent),
                    session.path.display().to_string(),
                    file_mtime,
                    file_size,
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(status.as_deref() == Some("indexed"))
    }

    pub fn session_skill_index_states_for_scope(
        &self,
        scope_key: &ScopeKey,
    ) -> Result<BTreeMap<(String, String, String), (i64, i64)>> {
        with_database_read_lock_retry(|| {
            let mut statement = self.conn.prepare(
                "SELECT session_id, agent, session_path, file_mtime, file_size
                 FROM scoped_session_skill_index
                 WHERE scope_key = ?1 AND status = 'indexed'",
            )?;
            let mut states = BTreeMap::new();
            let rows = statement.query_map([scope_key.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?;
            for row in rows {
                let (session_id, agent, session_path, file_mtime, file_size) = row?;
                states.insert((session_id, agent, session_path), (file_mtime, file_size));
            }
            Ok(states)
        })
    }

    pub fn replace_session_skill_links_for_scope(
        &self,
        scope_key: &ScopeKey,
        session: &SessionRecord,
        state: &SessionFileState,
        links: &[SessionSkillLink],
    ) -> Result<()> {
        self.with_named_write_transaction("replace_session_skill_links_for_scope", |tx| {
            let agent = agent_label(session.agent);
            let session_path = session.path.display().to_string();
            tx.execute(
                "DELETE FROM scoped_session_skill_links
             WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
                params![scope_key.as_str(), session.id, agent, session_path],
            )?;
            for link in links {
                tx.execute(
                    "INSERT INTO scoped_session_skill_links (
                    scope_key, session_id, agent, session_path, skill_name, skill_path,
                    skill_agent, skill_scope, evidence_kind, evidence_text,
                    evidence_time, confidence
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        scope_key.as_str(),
                        link.session_id,
                        agent_label(link.agent),
                        link.session_path.display().to_string(),
                        link.skill_name,
                        link.skill_path.display().to_string(),
                        link.skill_agent.map(agent_label),
                        link.skill_scope,
                        link.evidence_kind,
                        link.evidence_text,
                        link.evidence_time,
                        link.confidence,
                    ],
                )?;
            }
            tx.execute(
                "INSERT INTO scoped_session_skill_index (
                scope_key, session_id, agent, session_path, file_mtime, file_size,
                indexed_at, status, error
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'indexed', NULL)
             ON CONFLICT(scope_key, session_id, agent, session_path) DO UPDATE SET
                file_mtime = excluded.file_mtime,
                file_size = excluded.file_size,
                indexed_at = excluded.indexed_at,
                status = excluded.status,
                error = NULL",
                params![
                    scope_key.as_str(),
                    session.id,
                    agent,
                    session_path,
                    state.file_mtime,
                    state.file_size,
                    unix_now().to_string(),
                ],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn mark_session_skill_index_failed_for_scope(
        &self,
        scope_key: &ScopeKey,
        session: &SessionRecord,
        file_mtime: i64,
        file_size: i64,
        error: &str,
    ) -> Result<()> {
        self.with_named_write_transaction("mark_session_skill_index_failed_for_scope", |tx| {
            tx.execute(
                "INSERT INTO scoped_session_skill_index (
                scope_key, session_id, agent, session_path, file_mtime, file_size,
                indexed_at, status, error
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'failed', ?8)
             ON CONFLICT(scope_key, session_id, agent, session_path) DO UPDATE SET
                file_mtime = excluded.file_mtime,
                file_size = excluded.file_size,
                indexed_at = excluded.indexed_at,
                status = excluded.status,
                error = excluded.error",
                params![
                    scope_key.as_str(),
                    session.id,
                    agent_label(session.agent),
                    session.path.display().to_string(),
                    file_mtime,
                    file_size,
                    unix_now().to_string(),
                    error,
                ],
            )?;
            Ok(())
        })
    }
}
