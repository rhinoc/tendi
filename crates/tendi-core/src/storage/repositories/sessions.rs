//! sessions persistence through the database-owned transaction boundary.
use super::super::*;
use crate::{sessions::SessionTokenUsage, time::parse_timestamp};

pub(in crate::storage) struct PreparedSessionSource {
    sources: Vec<SessionScanSourceState>,
    search_needs_refresh: bool,
}

#[derive(Debug, Clone)]
struct SessionListProjectionRow {
    id: String,
    agent: AgentKind,
    title: Option<String>,
    project: Option<PathBuf>,
    repository: Option<PathBuf>,
    repository_url: Option<String>,
    logical_project_id: Option<String>,
    logical_project_name: Option<String>,
    path: PathBuf,
    started_at: Option<String>,
    updated_at: Option<String>,
    message_count: Option<usize>,
    turn_count: Option<usize>,
    parent_session_id: Option<String>,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
}

impl SessionListProjectionRow {
    fn into_session(self) -> SessionRecord {
        let token_usage =
            self.input_tokens
                .or(self.cached_input_tokens)
                .map(|_| SessionTokenUsage {
                    input_tokens: self.input_tokens.unwrap_or_default(),
                    cached_input_tokens: self.cached_input_tokens.unwrap_or_default(),
                    output_tokens: 0,
                    reasoning_output_tokens: 0,
                    total_tokens: 0,
                });
        SessionRecord {
            id: self.id,
            agent: self.agent,
            title: clean_session_title(self.title),
            project: self.project,
            repository: self.repository,
            repository_url: self.repository_url,
            logical_project_id: self.logical_project_id,
            logical_project_name: self.logical_project_name,
            path: self.path,
            started_at: self.started_at,
            updated_at: self.updated_at,
            message_count: self.message_count,
            first_user_message: None,
            last_user_message: None,
            last_assistant_message: None,
            turn_count: self.turn_count,
            model: None,
            mode: None,
            approval_mode: None,
            is_run_everything: None,
            parent_session_id: self.parent_session_id,
            token_usage,
        }
    }
}

struct SessionListSqlPageSnapshot {
    page_payloads: Vec<String>,
    total: usize,
    child_session_count: usize,
    page: usize,
    page_count: usize,
    page_start: usize,
    page_end: usize,
}

enum SessionListPageDbSnapshot {
    Simple {
        page: SessionListSqlPageSnapshot,
        project_options: Vec<SessionListProjectOption>,
    },
    Complex {
        settings: AppSettings,
        projects: Vec<ProjectRecord>,
        projection_rows: Vec<SessionListProjectionRow>,
        search_hits: Vec<SessionSearchHit>,
    },
}

struct SessionListProjectionValues {
    repository: Option<String>,
    repository_url: Option<String>,
    logical_project_id: Option<String>,
    logical_project_name: Option<String>,
    started_at_ms: Option<i64>,
    updated_at_ms: Option<i64>,
    turn_count: Option<i64>,
    parent_session_id: Option<String>,
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
}

fn session_list_projection_values(session: &SessionRecord) -> Result<SessionListProjectionValues> {
    let token_usage = session.token_usage.as_ref();
    Ok(SessionListProjectionValues {
        repository: session
            .repository
            .as_ref()
            .map(|path| path.display().to_string()),
        repository_url: session.repository_url.clone(),
        logical_project_id: session.logical_project_id.clone(),
        logical_project_name: session.logical_project_name.clone(),
        started_at_ms: session
            .started_at
            .as_deref()
            .and_then(parse_timestamp)
            .map(|value| value.timestamp_millis()),
        updated_at_ms: session
            .updated_at
            .as_deref()
            .and_then(parse_timestamp)
            .map(|value| value.timestamp_millis()),
        turn_count: session
            .turn_count
            .map(|value| i64::try_from(value).context("session turn count is too large"))
            .transpose()?,
        parent_session_id: session.parent_session_id.clone(),
        input_tokens: token_usage
            .map(|usage| i64::try_from(usage.input_tokens))
            .transpose()
            .context("session input token count is too large")?,
        cached_input_tokens: token_usage
            .map(|usage| i64::try_from(usage.cached_input_tokens))
            .transpose()
            .context("session cached input token count is too large")?,
    })
}

/// Persist deletion tombstones before canonical membership is removed. The
/// worker then cleans exactly these identities, including after a restart.
fn mark_obsolete_session_search_in_tx(tx: &Transaction<'_>, scope: &ScopeKey) -> Result<()> {
    let payloads = tx
        .prepare(
            "SELECT data_json FROM scoped_sessions s WHERE scope_key = ?1
         AND NOT EXISTS(SELECT 1 FROM current_sessions c
             WHERE c.id = s.id AND c.agent = s.agent AND c.path = s.path)",
        )?
        .query_map([scope.as_str()], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for payload in payloads {
        let session: SessionRecord = serde_json::from_str(&payload)?;
        session_search_storage::mark_session_key_pending_in_tx(tx, scope, &session)?;
    }
    Ok(())
}

fn load_session_projects(
    tx: &Transaction<'_>,
    scope_key: &ScopeKey,
) -> Result<HashMap<String, ProjectState>> {
    let mut stmt = tx.prepare(
        "SELECT id, name, name_custom, last_seen_at
         FROM session_projects
         WHERE scope_key = ?1",
    )?;
    let rows = stmt.query_map([scope_key.as_str()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            ProjectState {
                name: row.get(1)?,
                name_custom: row.get(2)?,
                last_seen_at: row.get(3)?,
            },
        ))
    })?;
    rows.collect::<std::result::Result<HashMap<_, _>, _>>()
        .map_err(Into::into)
}

fn load_session_project_aliases(
    tx: &Transaction<'_>,
    scope_key: &ScopeKey,
) -> Result<HashMap<SessionProjectAlias, String>> {
    let mut stmt = tx.prepare(
        "SELECT kind, value, project_id
         FROM session_project_aliases
         WHERE scope_key = ?1",
    )?;
    let rows = stmt.query_map([scope_key.as_str()], |row| {
        Ok(((row.get(0)?, row.get(1)?), row.get(2)?))
    })?;
    rows.collect::<std::result::Result<HashMap<_, _>, _>>()
        .map_err(Into::into)
}

fn merge_session_project_rows(
    tx: &Transaction<'_>,
    target_project_id: &str,
    source_project_id: &str,
    scope_key: &ScopeKey,
) -> Result<()> {
    if target_project_id == source_project_id {
        return Ok(());
    }
    tx.execute(
        "UPDATE session_project_aliases SET project_id = ?1
         WHERE scope_key = ?3 AND project_id = ?2",
        params![target_project_id, source_project_id, scope_key.as_str()],
    )?;
    tx.execute(
        "DELETE FROM session_projects WHERE scope_key = ?2 AND id = ?1",
        params![source_project_id, scope_key.as_str()],
    )?;
    Ok(())
}

type SessionStorageKey = (String, String, String);

fn session_delta_storage_keys_in_tx(
    tx: &Transaction<'_>,
    scope_key: &ScopeKey,
    sessions: &[SessionRecord],
) -> Result<BTreeSet<SessionStorageKey>> {
    let mut keys = BTreeSet::new();
    let mut statement = tx.prepare(
        "SELECT id, agent, path
         FROM scoped_sessions
         WHERE scope_key = ?1 AND id = ?2 AND agent = ?3",
    )?;
    for session in sessions {
        let agent = agent_label(session.agent).to_string();
        keys.insert((
            session.id.clone(),
            agent.clone(),
            session.path.display().to_string(),
        ));
        let existing =
            statement.query_map(params![scope_key.as_str(), session.id, agent], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
        for key in existing {
            keys.insert(key?);
        }
    }
    Ok(keys)
}

fn cleanup_stale_scoped_session_rows_for_keys(
    tx: &Transaction<'_>,
    scope_key: &ScopeKey,
    keys: &BTreeSet<SessionStorageKey>,
) -> Result<()> {
    for (session_id, agent, session_path) in keys {
        tx.execute(
            "DELETE FROM scoped_session_skill_links
             WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4
               AND NOT EXISTS (
                SELECT 1 FROM scoped_sessions
                WHERE scoped_sessions.scope_key = scoped_session_skill_links.scope_key
                  AND scoped_sessions.id = scoped_session_skill_links.session_id
                  AND scoped_sessions.agent = scoped_session_skill_links.agent
                  AND scoped_sessions.path = scoped_session_skill_links.session_path
             )",
            params![scope_key.as_str(), session_id, agent, session_path],
        )?;
        tx.execute(
            "DELETE FROM scoped_session_skill_index
             WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4
               AND NOT EXISTS (
                SELECT 1 FROM scoped_sessions
                WHERE scoped_sessions.scope_key = scoped_session_skill_index.scope_key
                  AND scoped_sessions.id = scoped_session_skill_index.session_id
                  AND scoped_sessions.agent = scoped_session_skill_index.agent
                  AND scoped_sessions.path = scoped_session_skill_index.session_path
             )",
            params![scope_key.as_str(), session_id, agent, session_path],
        )?;
        tx.execute(
            &format!(
                "DELETE FROM {SCOPED_SESSION_SCAN_SOURCE_TABLE}
                 WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4
                   AND NOT EXISTS (
                    SELECT 1 FROM scoped_sessions
                    WHERE scoped_sessions.scope_key = {SCOPED_SESSION_SCAN_SOURCE_TABLE}.scope_key
                      AND scoped_sessions.id = {SCOPED_SESSION_SCAN_SOURCE_TABLE}.session_id
                      AND scoped_sessions.agent = {SCOPED_SESSION_SCAN_SOURCE_TABLE}.agent
                      AND scoped_sessions.path = {SCOPED_SESSION_SCAN_SOURCE_TABLE}.session_path
                 )"
            ),
            params![scope_key.as_str(), session_id, agent, session_path],
        )?;
    }
    Ok(())
}

fn replace_scoped_session_scan_sources(
    conn: &rusqlite::Transaction<'_>,
    scope_key: &ScopeKey,
    session: &SessionRecord,
    prepared: &PreparedSessionSources,
) -> Result<()> {
    let prepared_source = prepared
        .get(&session.path)
        .context("session source changed during preparation; retry the scan")?;
    let sources = &prepared_source.sources;
    let existing = {
        let mut statement = conn.prepare("SELECT source_path, file_mtime, file_size, parser_version FROM scoped_session_scan_sources WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4")?;
        statement
            .query_map(
                params![
                    scope_key.as_str(),
                    session.id,
                    agent_label(session.agent),
                    session.path.display().to_string()
                ],
                |row| {
                    Ok((
                        PathBuf::from(row.get::<_, String>(0)?),
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?
    };
    let incoming = sources
        .iter()
        .map(|source| {
            (
                source.path.clone(),
                source.file_mtime,
                source.file_size,
                SESSION_SCAN_CACHE_PARSER_VERSION.to_string(),
            )
        })
        .collect::<BTreeSet<_>>();
    if existing == incoming && !prepared_source.search_needs_refresh {
        return Ok(());
    }
    session_search_storage::mark_session_key_pending_in_tx(conn, scope_key, session)?;
    if existing == incoming {
        return Ok(());
    }
    conn.execute(
        &format!(
            "DELETE FROM {SCOPED_SESSION_SCAN_SOURCE_TABLE}
             WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3"
        ),
        params![scope_key.as_str(), session.id, agent_label(session.agent)],
    )?;
    let session_path = session.path.display().to_string();
    for source in sources {
        conn.execute(
            &format!(
                "INSERT INTO {SCOPED_SESSION_SCAN_SOURCE_TABLE}
                 (scope_key, session_id, agent, session_path, source_path, file_mtime, file_size, parser_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
            ),
            params![
                scope_key.as_str(),
                session.id,
                agent_label(session.agent),
                session_path,
                source.path.display().to_string(),
                source.file_mtime,
                source.file_size,
                SESSION_SCAN_CACHE_PARSER_VERSION,
            ],
        )?;
    }
    Ok(())
}

fn cleanup_stale_scoped_session_scan_source_rows(
    conn: &rusqlite::Transaction<'_>,
    scope_key: &ScopeKey,
) -> Result<()> {
    conn.execute(
        &format!(
            "DELETE FROM {SCOPED_SESSION_SCAN_SOURCE_TABLE}
             WHERE scope_key = ?1
               AND NOT EXISTS (
                SELECT 1 FROM scoped_sessions
                WHERE scoped_sessions.scope_key = {SCOPED_SESSION_SCAN_SOURCE_TABLE}.scope_key
                  AND scoped_sessions.id = {SCOPED_SESSION_SCAN_SOURCE_TABLE}.session_id
                  AND scoped_sessions.agent = {SCOPED_SESSION_SCAN_SOURCE_TABLE}.agent
                  AND scoped_sessions.path = {SCOPED_SESSION_SCAN_SOURCE_TABLE}.session_path
             )"
        ),
        [scope_key.as_str()],
    )?;
    Ok(())
}

impl Store {
    pub fn session_page_with_revision_for_scope(
        &self,
        scope_key: &ScopeKey,
        query: SessionListQuery,
    ) -> Result<(Revision, SessionListPage)> {
        let snapshot_query = query.clone();
        let (revision, snapshot) = self.read_session_revisioned(scope_key, || {
            self.read_session_page_snapshot(scope_key, &snapshot_query)
        })?;
        Ok((
            revision,
            self.finish_session_page_snapshot(scope_key, query, snapshot)?,
        ))
    }

    pub fn list_session_page_for_scope(
        &self,
        scope_key: &ScopeKey,
        query: SessionListQuery,
    ) -> Result<SessionListPage> {
        let snapshot = self.read_session_page_snapshot(scope_key, &query)?;
        self.finish_session_page_snapshot(scope_key, query, snapshot)
    }

    fn read_session_page_snapshot(
        &self,
        scope_key: &ScopeKey,
        query: &SessionListQuery,
    ) -> Result<SessionListPageDbSnapshot> {
        if Self::is_simple_session_list_query(query) {
            // Keep the fast path ahead of all session hydration and project
            // resolution. The snapshot contains only SQL values; JSON and
            // filesystem work happen after the revisioned read transaction.
            let page = self.list_session_page_from_table(scope_key, query)?;
            let settings = self.app_settings_once()?;
            let projects = self.list_projects_once()?;
            return Ok(SessionListPageDbSnapshot::Simple {
                page,
                project_options: self.list_session_list_project_options_from_table(
                    scope_key, query, &settings, &projects,
                )?,
            });
        }

        let normalized_query = query.query.trim();
        let search_hits = if normalized_query.is_empty() {
            Vec::new()
        } else {
            self.search_sessions_for_scope_once(scope_key, normalized_query, None)?
        };
        Ok(SessionListPageDbSnapshot::Complex {
            settings: self.app_settings_once()?,
            projects: self.list_projects_once()?,
            projection_rows: self.list_session_list_projection_rows(scope_key)?,
            search_hits,
        })
    }

    fn finish_session_page_snapshot(
        &self,
        scope_key: &ScopeKey,
        query: SessionListQuery,
        snapshot: SessionListPageDbSnapshot,
    ) -> Result<SessionListPage> {
        let (settings, projects, projection_rows, search_hits) = match snapshot {
            SessionListPageDbSnapshot::Simple {
                page,
                project_options,
            } => {
                return Ok(SessionListPage {
                    rows: Self::hydrate_session_list_payloads(page.page_payloads),
                    project_options,
                    total: page.total,
                    child_session_count: page.child_session_count,
                    page: page.page,
                    page_count: page.page_count,
                    page_start: page.page_start,
                    page_end: page.page_end,
                    group_count: None,
                });
            }
            SessionListPageDbSnapshot::Complex {
                settings,
                projects,
                projection_rows,
                search_hits,
            } => (settings, projects, projection_rows, search_hits),
        };
        let all_sessions = projection_rows
            .into_iter()
            .map(SessionListProjectionRow::into_session)
            .collect::<Vec<_>>();
        let session_projects = Self::session_project_summaries_from_sessions(&all_sessions);
        let project_options = Self::session_list_project_options(
            &all_sessions,
            &query,
            &settings,
            &projects,
            &session_projects,
        );

        let base_sessions = all_sessions
            .iter()
            .filter(|session| query.agent.is_none_or(|agent| session.agent == agent))
            .cloned()
            .collect::<Vec<_>>();
        let normalized_query = query.query.trim();
        let mut rows = if normalized_query.is_empty() {
            base_sessions
                .into_iter()
                .map(|session| SessionListRow {
                    session,
                    search_score: None,
                    search_snippet: None,
                })
                .collect::<Vec<_>>()
        } else {
            search_hits
                .into_iter()
                .filter(|hit| query.agent.is_none_or(|agent| hit.session.agent == agent))
                .map(|hit| SessionListRow {
                    session: hit.session,
                    search_score: Some(hit.search_score),
                    search_snippet: Some(hit.search_snippet),
                })
                .collect::<Vec<_>>()
        };

        let selected_projects = query
            .selected_project_keys
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        if !selected_projects.is_empty() {
            rows.retain(|row| {
                resolve_session_list_project(
                    &row.session,
                    &settings.missing_session_project_policy,
                    &session_projects,
                    &projects,
                )
                .is_some_and(|project| selected_projects.contains(project.key.as_str()))
            });
        }
        let child_session_count = rows
            .iter()
            .filter(|row| row.session.parent_session_id.is_some())
            .count();
        if !query.show_child_sessions {
            rows.retain(|row| row.session.parent_session_id.is_none());
        }
        rows.sort_by(|left, right| {
            compare_session_list_rows(left, right, &query.sort_key, &query.sort_direction)
        });

        let total = rows.len();
        let locate_key = query.locate.as_ref().map(|session| {
            format!(
                "{}\0{}\0{}",
                session.agent.label(),
                session.id,
                session.path.display(),
            )
        });
        if let Some(group_by) = query.group_by.as_deref() {
            let pages = build_session_list_group_pages(rows, group_by, query.page_size);
            let located_page = locate_key.as_deref().and_then(|target| {
                pages.iter().position(|page| {
                    page.rows
                        .iter()
                        .any(|row| session_list_identity(&row.session) == target)
                })
            });
            let page_count = pages.len().max(1);
            let page = located_page.unwrap_or(query.page).min(page_count - 1);
            let selected = &pages[page];
            let rows = self.hydrate_session_list_rows(scope_key, selected.rows.clone())?;
            return Ok(SessionListPage {
                rows,
                project_options,
                total,
                child_session_count,
                page,
                page_count,
                page_start: selected.start,
                page_end: selected.start + selected.rows.len(),
                group_count: Some(selected.group_count),
            });
        }

        let page_count = total.div_ceil(query.page_size).max(1);
        let located_page = locate_key.as_deref().and_then(|target| {
            rows.iter()
                .position(|row| session_list_identity(&row.session) == target)
                .map(|index| index / query.page_size)
        });
        let page = located_page.unwrap_or(query.page).min(page_count - 1);
        let page_start = if total == 0 {
            0
        } else {
            page * query.page_size
        };
        let page_end = (page_start + query.page_size).min(total);
        let rows =
            self.hydrate_session_list_rows(scope_key, rows[page_start..page_end].to_vec())?;
        Ok(SessionListPage {
            rows,
            project_options,
            total,
            child_session_count,
            page,
            page_count,
            page_start,
            page_end,
            group_count: None,
        })
    }

    fn is_simple_session_list_query(query: &SessionListQuery) -> bool {
        query.query.trim().is_empty()
            && query.selected_project_keys.is_empty()
            && query.group_by.is_none()
            && query.locate.is_none()
            && Self::session_list_sql_order(&query.sort_key, &query.sort_direction).is_some()
    }

    fn session_list_project_options(
        all_sessions: &[SessionRecord],
        query: &SessionListQuery,
        settings: &AppSettings,
        projects: &[ProjectRecord],
        session_projects: &[SessionProjectSummary],
    ) -> Vec<SessionListProjectOption> {
        let project_source = all_sessions
            .iter()
            .filter(|session| query.agent.is_none_or(|agent| session.agent == agent))
            .filter(|session| query.show_child_sessions || session.parent_session_id.is_none());
        let mut project_options = HashMap::<String, SessionListProjectOption>::new();
        for session in project_source {
            let Some(option) = resolve_session_list_project(
                session,
                &settings.missing_session_project_policy,
                session_projects,
                projects,
            ) else {
                continue;
            };
            project_options
                .entry(option.key.clone())
                .and_modify(|current| current.count += 1)
                .or_insert(SessionListProjectOption {
                    key: option.key,
                    label: option.label,
                    title: option.title,
                    count: 1,
                });
        }
        let mut project_options = project_options.into_values().collect::<Vec<_>>();
        project_options.sort_by(|left, right| {
            left.label
                .to_lowercase()
                .cmp(&right.label.to_lowercase())
                .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
        });
        project_options
    }

    pub fn save_sessions_at_for_scope(
        &self,
        scope_key: &ScopeKey,
        sessions: &SessionScan,
        scanned_at: u64,
    ) -> Result<()> {
        self.save_sessions_at_with_scope(sessions, scanned_at, scope_key)
    }

    pub(in crate::storage) fn save_sessions_at_with_scope(
        &self,
        sessions: &SessionScan,
        scanned_at: u64,
        scope_key: &ScopeKey,
    ) -> Result<()> {
        let sources = self.prepare_session_sources(scope_key, &sessions.sessions)?;
        self.with_named_write_transaction("save_sessions_at_with_scope", |tx| {
            self.save_sessions_at_with_scope_in_tx(tx, sessions, scanned_at, scope_key, &sources)
        })?;
        Ok(())
    }

    pub(in crate::storage) fn save_sessions_at_with_scope_in_tx(
        &self,
        tx: &Transaction<'_>,
        sessions: &SessionScan,
        scanned_at: u64,
        scope_key: &ScopeKey,
        sources: &PreparedSessionSources,
    ) -> Result<()> {
        tx.execute_batch(
            "
            CREATE TEMP TABLE IF NOT EXISTS current_sessions (
                id TEXT NOT NULL,
                agent TEXT NOT NULL,
                path TEXT NOT NULL,
                PRIMARY KEY (id, agent, path)
            );
            DELETE FROM current_sessions;
            ",
        )?;

        for session in &sessions.sessions {
            let agent = agent_label(session.agent);
            let path = session.path.display().to_string();
            let title = clean_session_title(session.title.clone());
            let projection = session_list_projection_values(session)?;
            let data_json = Self::session_metadata_json(session)?;
            tx.execute(
                "INSERT INTO current_sessions (id, agent, path)
                 VALUES (?1, ?2, ?3)",
                params![session.id, agent, path],
            )?;
            let changed = tx.execute(
                &format!(
                    "INSERT INTO {SCOPED_SESSION_TABLE}
                    (scope_key, id, agent, title, project, path, started_at, updated_at, message_count, first_user_message, last_user_message, last_assistant_message, repository, repository_url, logical_project_id, logical_project_name, started_at_ms, updated_at_ms, turn_count, parent_session_id, input_tokens, cached_input_tokens, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
                 ON CONFLICT(scope_key, id, agent, path) DO UPDATE SET
                    title = excluded.title,
                    project = excluded.project,
                    started_at = excluded.started_at,
                    updated_at = excluded.updated_at,
                    message_count = excluded.message_count,
                    first_user_message = excluded.first_user_message,
                    last_user_message = excluded.last_user_message,
                    last_assistant_message = excluded.last_assistant_message,
                    repository = excluded.repository,
                    repository_url = excluded.repository_url,
                    logical_project_id = excluded.logical_project_id,
                    logical_project_name = excluded.logical_project_name,
                    started_at_ms = excluded.started_at_ms,
                    updated_at_ms = excluded.updated_at_ms,
                    turn_count = excluded.turn_count,
                    parent_session_id = excluded.parent_session_id,
                    input_tokens = excluded.input_tokens,
                    cached_input_tokens = excluded.cached_input_tokens,
                    data_json = excluded.data_json
                 WHERE {SCOPED_SESSION_TABLE}.data_json IS NOT excluded.data_json
                    OR {SCOPED_SESSION_TABLE}.title IS NOT excluded.title
                    OR {SCOPED_SESSION_TABLE}.project IS NOT excluded.project
                    OR {SCOPED_SESSION_TABLE}.started_at IS NOT excluded.started_at
                    OR {SCOPED_SESSION_TABLE}.updated_at IS NOT excluded.updated_at
                    OR {SCOPED_SESSION_TABLE}.message_count IS NOT excluded.message_count
                    OR {SCOPED_SESSION_TABLE}.first_user_message IS NOT excluded.first_user_message
                    OR {SCOPED_SESSION_TABLE}.last_user_message IS NOT excluded.last_user_message
                    OR {SCOPED_SESSION_TABLE}.last_assistant_message IS NOT excluded.last_assistant_message
                    OR {SCOPED_SESSION_TABLE}.repository IS NOT excluded.repository
                    OR {SCOPED_SESSION_TABLE}.repository_url IS NOT excluded.repository_url
                    OR {SCOPED_SESSION_TABLE}.logical_project_id IS NOT excluded.logical_project_id
                    OR {SCOPED_SESSION_TABLE}.logical_project_name IS NOT excluded.logical_project_name
                    OR {SCOPED_SESSION_TABLE}.started_at_ms IS NOT excluded.started_at_ms
                    OR {SCOPED_SESSION_TABLE}.updated_at_ms IS NOT excluded.updated_at_ms
                    OR {SCOPED_SESSION_TABLE}.turn_count IS NOT excluded.turn_count
                    OR {SCOPED_SESSION_TABLE}.parent_session_id IS NOT excluded.parent_session_id
                    OR {SCOPED_SESSION_TABLE}.input_tokens IS NOT excluded.input_tokens
                    OR {SCOPED_SESSION_TABLE}.cached_input_tokens IS NOT excluded.cached_input_tokens"
                ),
                params![
                    scope_key.as_str(),
                    session.id,
                    agent,
                    title,
                    session.project.as_ref().map(|path| path.display().to_string()),
                    session.path.display().to_string(),
                    session.started_at,
                    session.updated_at,
                    session.message_count.map(|value| value as i64),
                    bound_session_preview(session.first_user_message.clone()),
                    bound_session_preview(session.last_user_message.clone()),
                    bound_session_preview(session.last_assistant_message.clone()),
                    projection.repository,
                    projection.repository_url,
                    projection.logical_project_id,
                    projection.logical_project_name,
                    projection.started_at_ms,
                    projection.updated_at_ms,
                    projection.turn_count,
                    projection.parent_session_id,
                    projection.input_tokens,
                    projection.cached_input_tokens,
                    data_json,
                ],
            )?;
            if changed > 0 {
                session_search_storage::mark_session_key_pending_in_tx(tx, scope_key, session)?;
            }
            replace_scoped_session_scan_sources(tx, scope_key, session, sources)?;
        }

        if sessions.warnings.is_empty() {
            mark_obsolete_session_search_in_tx(tx, scope_key)?;
            tx.execute(
                &format!(
                    "DELETE FROM {SCOPED_SESSION_TABLE}
                     WHERE scope_key = ?1
                       AND NOT EXISTS (
                        SELECT 1 FROM current_sessions
                        WHERE current_sessions.id = {SCOPED_SESSION_TABLE}.id
                          AND current_sessions.agent = {SCOPED_SESSION_TABLE}.agent
                          AND current_sessions.path = {SCOPED_SESSION_TABLE}.path
                       )"
                ),
                params![scope_key.as_str()],
            )?;
            cleanup_stale_scoped_session_skill_rows(&tx, scope_key)?;
            cleanup_stale_scoped_session_scan_source_rows(&tx, scope_key)?;
        }
        tx.execute("DELETE FROM current_sessions", [])?;
        let key = format!("sessions_last_scan_at:{}", scope_key.as_str());
        tx.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, scanned_at.to_string()],
        )?;
        advance_projection_head_in_tx(&tx, scope_key, "sessions", None, "ready")?;
        Ok(())
    }

    /// Completes a full session scan after its batches have been persisted.
    ///
    /// The scan data is already upserted by
    /// `apply_session_delta_and_resolve_projects_for_scope`; this transaction
    /// only removes sessions that disappeared and advances the scan marker.
    /// Keeping search indexing out of this transaction lets other domains
    /// acquire the database write lock between scan batches.
    pub fn finalize_session_scan_for_scope(
        &self,
        scope_key: &ScopeKey,
        sessions: &SessionScan,
        scanned_at: u64,
    ) -> Result<()> {
        self.with_named_write_transaction("finalize_session_scan_for_scope", |tx| {
            tx.execute_batch(
                "
            CREATE TEMP TABLE IF NOT EXISTS current_sessions (
                id TEXT NOT NULL,
                agent TEXT NOT NULL,
                path TEXT NOT NULL,
                PRIMARY KEY (id, agent, path)
            );
            DELETE FROM current_sessions;
            CREATE TEMP TABLE IF NOT EXISTS removed_sessions (
                id TEXT NOT NULL,
                agent TEXT NOT NULL,
                path TEXT NOT NULL,
                PRIMARY KEY (id, agent, path)
            );
            DELETE FROM removed_sessions;
            ",
            )?;
            for session in &sessions.sessions {
                tx.execute(
                    "INSERT INTO current_sessions (id, agent, path)
                 VALUES (?1, ?2, ?3)",
                    params![
                        session.id,
                        agent_label(session.agent),
                        session.path.display().to_string(),
                    ],
                )?;
            }

            if sessions.warnings.is_empty() {
                tx.execute(
                    &format!(
                        "INSERT INTO removed_sessions (id, agent, path)
                     SELECT scoped.id, scoped.agent, scoped.path
                     FROM {SCOPED_SESSION_TABLE} AS scoped
                     WHERE scoped.scope_key = ?1
                       AND NOT EXISTS (
                        SELECT 1 FROM current_sessions
                        WHERE current_sessions.id = scoped.id
                          AND current_sessions.agent = scoped.agent
                          AND current_sessions.path = scoped.path
                       )"
                    ),
                    params![scope_key.as_str()],
                )?;
                mark_obsolete_session_search_in_tx(tx, scope_key)?;
                tx.execute(
                    &format!(
                        "DELETE FROM {SCOPED_SESSION_TABLE}
                     WHERE scope_key = ?1
                       AND EXISTS (
                        SELECT 1 FROM removed_sessions
                        WHERE removed_sessions.id = {SCOPED_SESSION_TABLE}.id
                          AND removed_sessions.agent = {SCOPED_SESSION_TABLE}.agent
                          AND removed_sessions.path = {SCOPED_SESSION_TABLE}.path
                       )"
                    ),
                    params![scope_key.as_str()],
                )?;
            }
            // Delta batches only clean identities they touched. The complete
            // orphan sweep belongs here, once per finalized full scan.
            cleanup_stale_scoped_session_skill_rows(&tx, scope_key)?;
            cleanup_stale_scoped_session_scan_source_rows(&tx, scope_key)?;
            tx.execute("DELETE FROM current_sessions", [])?;
            tx.execute("DELETE FROM removed_sessions", [])?;
            let key = format!("sessions_last_scan_at:{}", scope_key.as_str());
            tx.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, scanned_at.to_string()],
            )?;
            advance_projection_head_in_tx(&tx, scope_key, "sessions", None, "ready")?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn apply_session_changes_for_scope(
        &self,
        scope_key: &ScopeKey,
        sessions: &[SessionRecord],
        removed_paths: &[PathBuf],
    ) -> Result<(Vec<SessionRecord>, Vec<crate::sessions::SessionIdentity>)> {
        let sources = self.prepare_session_sources(scope_key, sessions)?;
        let aliases = prepare_project_aliases(sessions);
        let result =
            self.with_named_write_transaction("apply_session_changes_for_scope", |tx| {
                let mut changed =
                    Self::apply_session_delta_in_tx(&tx, sessions, scope_key, &sources)?;
                self.resolve_session_projects_in_tx(&tx, &mut changed, scope_key, &aliases)?;
                let existing = if removed_paths.is_empty() {
                    Vec::new()
                } else {
                    Self::list_sessions_in_tx(tx, scope_key)?
                };
                let removed = existing
                    .into_iter()
                    .filter(|session| {
                        removed_paths
                            .iter()
                            .any(|path| session.path == *path || session.path.starts_with(path))
                    })
                    .collect::<Vec<_>>();
                for session in &removed {
                    session_search_storage::mark_session_key_pending_in_tx(tx, scope_key, session)?;
                    tx.execute(
                        &format!(
                            "DELETE FROM {SCOPED_SESSION_TABLE}
                     WHERE scope_key = ?1 AND id = ?2 AND agent = ?3 AND path = ?4"
                        ),
                        params![
                            scope_key.as_str(),
                            session.id,
                            agent_label(session.agent),
                            session.path.display().to_string(),
                        ],
                    )?;
                }
                if !changed.is_empty() || !removed.is_empty() {
                    advance_projection_head_in_tx(&tx, scope_key, "sessions", None, "ready")?;
                }
                if !changed.is_empty() || !removed.is_empty() {
                    cleanup_stale_scoped_session_skill_rows(&tx, scope_key)?;
                    cleanup_stale_scoped_session_scan_source_rows(&tx, scope_key)?;
                }

                Ok((
                    changed,
                    removed
                        .iter()
                        .map(crate::sessions::SessionIdentity::from)
                        .collect::<Vec<_>>(),
                ))
            })?;
        crate::logging::global().info(
            "session projection write completed",
            serde_json::json!({
                "scopeKey": scope_key.as_str(),
                "inputSessionCount": sessions.len(),
                "changedSessionCount": result.0.len(),
                "removedSessionCount": result.1.len(),
                "changedSessions": result
                    .0
                    .iter()
                    .map(|session| {
                        serde_json::json!({
                            "id": &session.id,
                            "agent": session.agent.label(),
                            "path": &session.path,
                            "messageCount": session.message_count,
                            "userLastPresent": session
                                .last_user_message
                                .as_ref()
                                .is_some_and(|message| !message.is_empty()),
                            "assistantLastPresent": session
                                .last_assistant_message
                                .as_ref()
                                .is_some_and(|message| !message.is_empty()),
                        })
                    })
                    .collect::<Vec<_>>(),
            }),
        );
        Ok(result)
    }

    pub(in crate::storage) fn apply_session_delta_in_tx(
        tx: &Transaction<'_>,
        sessions: &[SessionRecord],
        scope_key: &ScopeKey,
        sources: &PreparedSessionSources,
    ) -> Result<Vec<SessionRecord>> {
        let mut changed = Vec::new();
        for session in sessions {
            let agent = agent_label(session.agent);
            let existing_session = tx
                .query_row(
                    "SELECT data_json FROM scoped_sessions
                     WHERE scope_key = ?1 AND id = ?2 AND agent = ?3 LIMIT 1",
                    params![scope_key.as_str(), session.id, agent],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .and_then(|data_json| serde_json::from_str::<SessionRecord>(&data_json).ok());
            let mut canonical = session.clone();
            if let Some(existing) = existing_session.as_ref() {
                let existing_is_transcript = existing
                    .path
                    .extension()
                    .is_some_and(|extension| extension == "jsonl");
                let incoming_is_transcript = canonical
                    .path
                    .extension()
                    .is_some_and(|extension| extension == "jsonl");
                if existing_is_transcript && !incoming_is_transcript {
                    canonical.path = existing.path.clone();
                    if canonical.token_usage.is_none() {
                        canonical.token_usage = existing.token_usage.clone();
                    }
                }
            }
            canonical.title = clean_session_title(canonical.title.take());
            let path = canonical.path.display().to_string();
            let projection = session_list_projection_values(&canonical)?;
            let data_json = Self::session_metadata_json(&canonical)?;
            let title = canonical.title.clone();
            let project = canonical
                .project
                .as_ref()
                .map(|path| path.display().to_string());
            let started_at = canonical.started_at.clone();
            let updated_at = canonical.updated_at.clone();
            let message_count = canonical.message_count.map(|value| value as i64);
            let first_user_message = bound_session_preview(canonical.first_user_message.clone());
            let last_user_message = bound_session_preview(canonical.last_user_message.clone());
            let last_assistant_message =
                bound_session_preview(canonical.last_assistant_message.clone());
            let current = tx
                .query_row(
                    "SELECT data_json, title, project, started_at, updated_at, message_count,
                            first_user_message, last_user_message, last_assistant_message
                     FROM scoped_sessions
                     WHERE scope_key = ?1 AND id = ?2 AND agent = ?3 AND path = ?4",
                    params![scope_key.as_str(), canonical.id, agent, path],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<i64>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, Option<String>>(8)?,
                        ))
                    },
                )
                .optional()?;
            if current.as_ref().is_some_and(|current| {
                current.0 == data_json
                    && current.1 == title
                    && current.2 == project
                    && current.3 == started_at
                    && current.4 == updated_at
                    && current.5 == message_count
                    && current.6 == first_user_message
                    && current.7 == last_user_message
                    && current.8 == last_assistant_message
            }) {
                replace_scoped_session_scan_sources(tx, scope_key, &canonical, sources)?;
                continue;
            }
            let replaced = tx.prepare(
                "SELECT data_json FROM scoped_sessions WHERE scope_key = ?1 AND id = ?2 AND agent = ?3 AND path <> ?4"
            )?.query_map(params![scope_key.as_str(), canonical.id, agent, path], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for payload in replaced {
                let old: SessionRecord = serde_json::from_str(&payload)?;
                session_search_storage::mark_session_key_pending_in_tx(tx, scope_key, &old)?;
            }
            tx.execute(
                "DELETE FROM scoped_sessions
                 WHERE scope_key = ?1 AND id = ?2 AND agent = ?3 AND path <> ?4",
                params![scope_key.as_str(), canonical.id, agent, path],
            )?;
            tx.execute(
                "INSERT INTO scoped_sessions
                (scope_key, id, agent, title, project, path, started_at, updated_at, message_count, first_user_message, last_user_message, last_assistant_message, repository, repository_url, logical_project_id, logical_project_name, started_at_ms, updated_at_ms, turn_count, parent_session_id, input_tokens, cached_input_tokens, data_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
             ON CONFLICT(scope_key, id, agent, path) DO UPDATE SET
                title = excluded.title,
                project = excluded.project,
                started_at = excluded.started_at,
                updated_at = excluded.updated_at,
                message_count = excluded.message_count,
                first_user_message = excluded.first_user_message,
                last_user_message = excluded.last_user_message,
                last_assistant_message = excluded.last_assistant_message,
                repository = excluded.repository,
                repository_url = excluded.repository_url,
                logical_project_id = excluded.logical_project_id,
                logical_project_name = excluded.logical_project_name,
                started_at_ms = excluded.started_at_ms,
                updated_at_ms = excluded.updated_at_ms,
                turn_count = excluded.turn_count,
                parent_session_id = excluded.parent_session_id,
                input_tokens = excluded.input_tokens,
                cached_input_tokens = excluded.cached_input_tokens,
                data_json = excluded.data_json",
                params![
                    scope_key.as_str(),
                    canonical.id,
                    agent,
                    title,
                    project,
                    path,
                    started_at,
                    updated_at,
                    message_count,
                    first_user_message,
                    last_user_message,
                    last_assistant_message,
                    projection.repository,
                    projection.repository_url,
                    projection.logical_project_id,
                    projection.logical_project_name,
                    projection.started_at_ms,
                    projection.updated_at_ms,
                    projection.turn_count,
                    projection.parent_session_id,
                    projection.input_tokens,
                    projection.cached_input_tokens,
                    data_json,
                ],
            )?;
            replace_scoped_session_scan_sources(tx, scope_key, &canonical, sources)?;
            session_search_storage::mark_session_key_pending_in_tx(tx, scope_key, &canonical)?;
            changed.push(canonical);
        }
        Ok(changed)
    }

    pub(in crate::storage) fn list_sessions_in_tx(
        tx: &Transaction<'_>,
        scope_key: &ScopeKey,
    ) -> Result<Vec<SessionRecord>> {
        let mut stmt = tx.prepare("SELECT data_json FROM scoped_sessions WHERE scope_key = ?1")?;
        let mut sessions = Vec::new();
        let mut rows = stmt.query([scope_key.as_str()])?;
        while let Some(row) = rows.next()? {
            if let Ok(session) = serde_json::from_str::<SessionRecord>(&row.get::<_, String>(0)?) {
                sessions.push(Self::normalize_cached_session(session));
            }
        }
        Ok(sessions)
    }

    pub fn remove_sessions_for_paths_for_scope(
        &self,
        scope_key: &ScopeKey,
        paths: &[PathBuf],
    ) -> Result<Vec<crate::sessions::SessionIdentity>> {
        Ok(self
            .apply_session_changes_for_scope(scope_key, &[], paths)?
            .1)
    }

    pub fn session_scan_cache_for_scope(&self, scope_key: &ScopeKey) -> Result<SessionScanCache> {
        let sessions = self.list_sessions_for_scope(scope_key)?.sessions;
        let mut source_states_by_session: HashMap<
            (String, String, String),
            Vec<SessionScanSourceState>,
        > = HashMap::new();
        let mut statement = self.conn.prepare(&format!(
            "SELECT session_id, agent, session_path, source_path, file_mtime, file_size,
                    parser_version
             FROM {SCOPED_SESSION_SCAN_SOURCE_TABLE}
             WHERE scope_key = ?1"
        ))?;
        let rows = statement.query_map([scope_key.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                SessionScanSourceState {
                    path: PathBuf::from(row.get::<_, String>(3)?),
                    file_mtime: row.get(4)?,
                    file_size: row.get(5)?,
                },
                row.get::<_, String>(6)?,
            ))
        })?;
        for row in rows {
            let (session_id, agent, session_path, source, parser_version) = row?;
            if parser_version != SESSION_SCAN_CACHE_PARSER_VERSION {
                continue;
            }
            source_states_by_session
                .entry((session_id, agent, session_path))
                .or_default()
                .push(source);
        }
        drop(statement);

        let entries = sessions.into_iter().filter_map(|session| {
            let key = (
                session.id.clone(),
                agent_label(session.agent).to_string(),
                session.path.display().to_string(),
            );
            let source_states = source_states_by_session.remove(&key)?;
            let session_path = session.path.clone();
            let primary = source_states
                .iter()
                .find(|source| source.path == session_path)?;
            let file_mtime = primary.file_mtime;
            let file_size = primary.file_size;
            Some(SessionScanCacheEntry {
                session,
                file_mtime,
                file_size,
                additional_file_states: source_states
                    .into_iter()
                    .filter(|source| source.path != session_path)
                    .collect(),
            })
        });
        Ok(SessionScanCache::from_entries(entries))
    }

    pub fn sessions_last_scan_at_for_scope(&self, scope_key: &ScopeKey) -> Result<Option<u64>> {
        self.sessions_last_scan_at_for_key(&format!("sessions_last_scan_at:{}", scope_key.as_str()))
    }

    pub(in crate::storage) fn sessions_last_scan_at_for_key(
        &self,
        key: &str,
    ) -> Result<Option<u64>> {
        self.conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| {
                value
                    .parse::<u64>()
                    .context("invalid sessions scan timestamp")
            })
            .transpose()
    }

    pub fn apply_session_delta_for_scope(
        &self,
        scope_key: &ScopeKey,
        sessions: &[SessionRecord],
    ) -> Result<Vec<SessionRecord>> {
        let sources = self.prepare_session_sources(scope_key, sessions)?;
        self.with_named_write_transaction("apply_session_delta_for_scope", |tx| {
            let affected_keys = session_delta_storage_keys_in_tx(tx, scope_key, sessions)?;
            let changed = Self::apply_session_delta_in_tx(&tx, sessions, scope_key, &sources)?;
            if !changed.is_empty() {
                advance_projection_head_in_tx(&tx, scope_key, "sessions", None, "ready")?;
            }
            cleanup_stale_scoped_session_rows_for_keys(tx, scope_key, &affected_keys)?;
            Ok(changed)
        })
    }

    /// Commit session metadata before preparing and updating its search index.
    pub fn apply_session_delta_and_resolve_projects_for_scope(
        &self,
        scope_key: &ScopeKey,
        sessions: &[SessionRecord],
    ) -> Result<Vec<SessionRecord>> {
        let sources = self.prepare_session_sources(scope_key, sessions)?;
        let aliases = prepare_project_aliases(sessions);
        let changed = self.with_named_write_transaction(
            "apply_session_delta_and_resolve_projects_for_scope",
            |tx| {
                let affected_keys = session_delta_storage_keys_in_tx(tx, scope_key, sessions)?;
                let mut changed =
                    Self::apply_session_delta_in_tx(tx, sessions, scope_key, &sources)?;
                self.resolve_session_projects_in_tx(tx, &mut changed, scope_key, &aliases)?;
                if !changed.is_empty() {
                    advance_projection_head_in_tx(tx, scope_key, "sessions", None, "ready")?;
                }
                cleanup_stale_scoped_session_rows_for_keys(tx, scope_key, &affected_keys)?;
                Ok(changed)
            },
        )?;
        Ok(changed)
    }

    pub(in crate::storage) fn compare_session_updated_at(
        left: &SessionRecord,
        right: &SessionRecord,
    ) -> std::cmp::Ordering {
        compare_timestamps(right.updated_at.as_deref(), left.updated_at.as_deref())
            .then_with(|| left.id.cmp(&right.id))
    }

    pub(in crate::storage) fn normalize_cached_session(
        mut session: SessionRecord,
    ) -> SessionRecord {
        session.title = clean_session_title(session.title.take());
        session.first_user_message = bound_session_preview(session.first_user_message.take());
        session.last_user_message = bound_session_preview(session.last_user_message.take());
        session.last_assistant_message =
            bound_session_preview(session.last_assistant_message.take());
        session
    }

    pub(crate) fn session_metadata_json(session: &SessionRecord) -> Result<String> {
        let mut metadata = session.clone();
        metadata.title = clean_session_title(metadata.title.take());
        metadata.first_user_message = bound_session_preview(metadata.first_user_message.take());
        metadata.last_user_message = bound_session_preview(metadata.last_user_message.take());
        metadata.last_assistant_message =
            bound_session_preview(metadata.last_assistant_message.take());
        Ok(serde_json::to_string(&metadata)?)
    }

    pub fn resolve_session_projects_for_scope(
        &self,
        scope_key: &ScopeKey,
        sessions: &mut [SessionRecord],
    ) -> Result<()> {
        normalize_session_projects(sessions);
        let aliases = prepare_project_aliases(sessions);
        self.with_named_write_transaction("resolve_session_projects_for_scope", |tx| {
            self.resolve_session_projects_in_tx(tx, sessions, scope_key, &aliases)
        })?;
        Ok(())
    }

    pub(in crate::storage) fn resolve_session_projects_in_tx(
        &self,
        tx: &Transaction<'_>,
        sessions: &mut [SessionRecord],
        scope_key: &ScopeKey,
        prepared_aliases: &PreparedProjectAliases,
    ) -> Result<()> {
        let mut projects = load_session_projects(tx, scope_key)?;
        let mut aliases = load_session_project_aliases(tx, scope_key)?;

        for session in sessions.iter_mut() {
            let evidence = prepared_aliases
                .get(&(session.id.clone(), agent_label(session.agent).to_string()))
                .context("session project evidence must be prepared before writing")?
                .clone();
            if evidence.is_empty() {
                session.logical_project_id = None;
                session.logical_project_name = None;
                continue;
            }

            let mut candidates = evidence
                .iter()
                .filter_map(|alias| aliases.get(alias).cloned())
                .collect::<BTreeSet<_>>();
            let project_id = if candidates.is_empty() {
                let seed = evidence
                    .iter()
                    .find(|(kind, _)| kind == "repository_url")
                    .unwrap_or(&evidence[0]);
                let Some(name) = suggested_project_name(session) else {
                    session.logical_project_id = None;
                    session.logical_project_name = None;
                    continue;
                };
                let project_id = format!(
                    "project-{}",
                    &sha256_text(&format!("{}:{}", seed.0, seed.1))[..24]
                );
                let last_seen_at = session_project_seen_at(session);
                tx.execute(
                    "INSERT OR IGNORE INTO session_projects
                        (scope_key, id, name, name_custom, last_seen_at)
                     VALUES (?1, ?2, ?3, 0, ?4)",
                    params![scope_key.as_str(), project_id, name, last_seen_at],
                )?;
                projects.entry(project_id.clone()).or_insert(ProjectState {
                    name,
                    name_custom: false,
                    last_seen_at,
                });
                project_id
            } else {
                let target = candidates
                    .iter()
                    .max_by(|left, right| {
                        let left_seen_at = projects
                            .get(*left)
                            .map(|project| project.last_seen_at.as_str());
                        let right_seen_at = projects
                            .get(*right)
                            .map(|project| project.last_seen_at.as_str());
                        compare_timestamps(left_seen_at, right_seen_at)
                    })
                    .cloned()
                    .context("session project alias references a missing project")?;
                candidates.remove(&target);
                for source in candidates {
                    merge_session_project_rows(tx, &target, &source, scope_key)?;
                    for alias_project_id in aliases.values_mut() {
                        if *alias_project_id == source {
                            *alias_project_id = target.clone();
                        }
                    }
                    projects.remove(&source);
                }
                target
            };

            for alias in evidence {
                if aliases.get(&alias) != Some(&project_id) {
                    tx.execute(
                        "INSERT OR REPLACE INTO session_project_aliases
                            (scope_key, project_id, kind, value)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![scope_key.as_str(), project_id, alias.0, alias.1],
                    )?;
                    aliases.insert(alias, project_id.clone());
                }
            }

            let seen_at = session_project_seen_at(session);
            let suggested_name = suggested_project_name(session);
            if let Some(project) = projects.get_mut(&project_id) {
                if compare_timestamps(Some(seen_at.as_str()), Some(project.last_seen_at.as_str()))
                    .is_gt()
                {
                    project.last_seen_at = seen_at.clone();
                    if !project.name_custom {
                        if let Some(suggested_name) = suggested_name {
                            project.name = suggested_name;
                        }
                    }
                    tx.execute(
                        "UPDATE session_projects
                         SET name = ?3, last_seen_at = ?4
                         WHERE scope_key = ?1 AND id = ?2",
                        params![
                            scope_key.as_str(),
                            project_id,
                            project.name,
                            project.last_seen_at
                        ],
                    )?;
                }
                session.logical_project_id = Some(project_id);
                session.logical_project_name = Some(project.name.clone());
            }
        }

        for session in sessions.iter() {
            let projection = session_list_projection_values(session)?;
            let data_json = Self::session_metadata_json(session)?;
            let changed = tx.execute(
                &format!(
                    "UPDATE {SCOPED_SESSION_TABLE}
                     SET repository = ?1,
                         repository_url = ?2,
                         logical_project_id = ?3,
                         logical_project_name = ?4,
                         started_at_ms = ?5,
                         updated_at_ms = ?6,
                         turn_count = ?7,
                         parent_session_id = ?8,
                         input_tokens = ?9,
                         cached_input_tokens = ?10,
                         data_json = ?11
                     WHERE scope_key = ?12 AND id = ?13 AND agent = ?14 AND path = ?15
                       AND data_json IS NOT ?11"
                ),
                params![
                    projection.repository,
                    projection.repository_url,
                    projection.logical_project_id,
                    projection.logical_project_name,
                    projection.started_at_ms,
                    projection.updated_at_ms,
                    projection.turn_count,
                    projection.parent_session_id,
                    projection.input_tokens,
                    projection.cached_input_tokens,
                    data_json,
                    scope_key.as_str(),
                    session.id,
                    agent_label(session.agent),
                    session.path.display().to_string(),
                ],
            )?;
            if changed > 0 {
                session_search_storage::mark_session_key_pending_in_tx(tx, scope_key, session)?;
            }
        }
        Ok(())
    }

    pub(in crate::storage) fn list_sessions_from_table(
        &self,
        scope_key: &ScopeKey,
    ) -> Result<SessionScan> {
        // `data_json` is the SessionRecord authority. The scalar session columns are
        // denormalized projections retained for compatibility and write-side indexing.
        let updated_order = Self::session_list_sql_order("updatedAt", "desc")
            .context("updatedAt is not SQL-orderable")?;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT data_json FROM scoped_sessions
                 WHERE scope_key = ?1
                 ORDER BY {updated_order}, id ASC",
        ))?;
        let mut sessions = Vec::new();
        let mut warnings = Vec::new();
        let mut rows = stmt.query(params![scope_key.as_str()])?;
        while let Some(row) = rows.next()? {
            let data_json = row.get::<_, String>(0)?;
            match serde_json::from_str::<SessionRecord>(&data_json) {
                Ok(session) => sessions.push(Self::normalize_cached_session(session)),
                Err(err) => warnings.push(format!("invalid cached session row: {err}")),
            }
        }

        Ok(SessionScan { sessions, warnings })
    }

    /// Return logical projects observed in one workspace from its canonical
    /// snapshot or scoped index.
    pub fn list_session_projects_for_scope(
        &self,
        scope_key: &ScopeKey,
    ) -> Result<Vec<SessionProjectSummary>> {
        with_database_read_lock_retry(|| self.list_session_projects_for_scope_once(scope_key))
    }

    pub(in crate::storage) fn list_session_projects_for_scope_once(
        &self,
        scope_key: &ScopeKey,
    ) -> Result<Vec<SessionProjectSummary>> {
        self.session_project_summaries_for_scope(scope_key)
    }

    pub(in crate::storage) fn session_project_summaries_for_scope(
        &self,
        scope_key: &ScopeKey,
    ) -> Result<Vec<SessionProjectSummary>> {
        let sessions = self.list_session_list_projections(scope_key)?;
        Ok(Self::session_project_summaries_from_sessions(&sessions))
    }

    pub(in crate::storage) fn session_project_summaries_from_sessions(
        sessions: &[SessionRecord],
    ) -> Vec<SessionProjectSummary> {
        let mut summaries = BTreeMap::<String, SessionProjectSummary>::new();
        for session in sessions {
            let Some(id) = session.logical_project_id.as_ref() else {
                continue;
            };
            let entry = summaries
                .entry(id.clone())
                .or_insert_with(|| SessionProjectSummary {
                    id: id.clone(),
                    name: session
                        .logical_project_name
                        .clone()
                        .unwrap_or_else(|| "Unnamed project".to_string()),
                    missing: true,
                    paths: Vec::new(),
                });
            if let Some(path) = &session.project {
                entry.paths.push(path.clone());
            }
            if let Some(name) = &session.logical_project_name {
                entry.name = name.clone();
            }
        }
        for summary in summaries.values_mut() {
            summary.paths.sort();
            summary.paths.dedup();
            summary.missing = !summary.paths.iter().any(|path| path.is_dir());
        }
        summaries.into_values().collect()
    }

    fn list_session_list_projection_rows(
        &self,
        scope_key: &ScopeKey,
    ) -> Result<Vec<SessionListProjectionRow>> {
        let mut sessions = Vec::new();
        self.for_each_session_list_projection_row(scope_key, |session| {
            sessions.push(session);
            Ok(())
        })?;
        Ok(sessions)
    }

    fn for_each_session_list_projection_row(
        &self,
        scope_key: &ScopeKey,
        mut visit: impl FnMut(SessionListProjectionRow) -> Result<()>,
    ) -> Result<()> {
        let mut statement = self.conn.prepare(
            "SELECT id, agent, title, project, path, started_at, updated_at, message_count,
                    repository, repository_url, logical_project_id, logical_project_name,
                    turn_count, parent_session_id, input_tokens, cached_input_tokens
             FROM scoped_sessions
             WHERE scope_key = ?1 AND json_valid(data_json)",
        )?;
        let rows = statement.query_map([scope_key.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<i64>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, Option<i64>>(14)?,
                row.get::<_, Option<i64>>(15)?,
            ))
        })?;
        for row in rows {
            let (
                id,
                agent,
                title,
                project,
                path,
                started_at,
                updated_at,
                message_count,
                repository,
                repository_url,
                logical_project_id,
                logical_project_name,
                turn_count,
                parent_session_id,
                input_tokens,
                cached_input_tokens,
            ) = row?;
            let Some(agent) = parse_agent_label(&agent) else {
                continue;
            };
            let session = SessionListProjectionRow {
                id,
                agent,
                title,
                project: project.map(PathBuf::from),
                repository: repository.map(PathBuf::from),
                repository_url,
                logical_project_id,
                logical_project_name,
                path: PathBuf::from(path),
                started_at,
                updated_at,
                message_count: message_count.and_then(|value| usize::try_from(value).ok()),
                turn_count: turn_count.and_then(|value| usize::try_from(value).ok()),
                parent_session_id,
                input_tokens: input_tokens.map(|value| value.max(0) as u64),
                cached_input_tokens: cached_input_tokens.map(|value| value.max(0) as u64),
            };
            visit(session)?;
        }
        Ok(())
    }

    fn session_project_summaries_from_table(
        &self,
        scope_key: &ScopeKey,
    ) -> Result<Vec<SessionProjectSummary>> {
        let mut summaries = BTreeMap::<String, SessionProjectSummary>::new();
        self.for_each_session_list_projection_row(scope_key, |session| {
            let Some(id) = session.logical_project_id.as_ref() else {
                return Ok(());
            };
            let entry = summaries
                .entry(id.clone())
                .or_insert_with(|| SessionProjectSummary {
                    id: id.clone(),
                    name: session
                        .logical_project_name
                        .clone()
                        .unwrap_or_else(|| "Unnamed project".to_string()),
                    missing: true,
                    paths: Vec::new(),
                });
            if let Some(path) = session.project {
                entry.paths.push(path);
            }
            if let Some(name) = session.logical_project_name {
                entry.name = name;
            }
            Ok(())
        })?;
        for summary in summaries.values_mut() {
            summary.paths.sort();
            summary.paths.dedup();
            summary.missing = !summary.paths.iter().any(|path| path.is_dir());
        }
        Ok(summaries.into_values().collect())
    }

    fn list_session_list_project_options_from_table(
        &self,
        scope_key: &ScopeKey,
        query: &SessionListQuery,
        settings: &AppSettings,
        projects: &[ProjectRecord],
    ) -> Result<Vec<SessionListProjectOption>> {
        let session_projects = self.session_project_summaries_from_table(scope_key)?;
        let mut project_options = HashMap::<String, SessionListProjectOption>::new();
        self.for_each_session_list_projection_row(scope_key, |projection| {
            if query.agent.is_some_and(|agent| projection.agent != agent)
                || (!query.show_child_sessions && projection.parent_session_id.is_some())
            {
                return Ok(());
            }
            let session = projection.into_session();
            let Some(option) = resolve_session_list_project(
                &session,
                &settings.missing_session_project_policy,
                &session_projects,
                projects,
            ) else {
                return Ok(());
            };
            project_options
                .entry(option.key.clone())
                .and_modify(|current| current.count += 1)
                .or_insert(SessionListProjectOption {
                    key: option.key,
                    label: option.label,
                    title: option.title,
                    count: 1,
                });
            Ok(())
        })?;
        let mut project_options = project_options.into_values().collect::<Vec<_>>();
        project_options.sort_by(|left, right| {
            left.label
                .to_lowercase()
                .cmp(&right.label.to_lowercase())
                .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
        });
        Ok(project_options)
    }

    fn list_session_list_projections(&self, scope_key: &ScopeKey) -> Result<Vec<SessionRecord>> {
        Ok(self
            .list_session_list_projection_rows(scope_key)?
            .into_iter()
            .map(SessionListProjectionRow::into_session)
            .collect())
    }

    fn hydrate_session_list_payloads(page_payloads: Vec<String>) -> Vec<SessionListRow> {
        page_payloads
            .into_iter()
            .filter_map(|data_json| {
                serde_json::from_str::<SessionRecord>(&data_json)
                    .ok()
                    .map(Self::normalize_cached_session)
            })
            .map(|session| SessionListRow {
                session,
                search_score: None,
                search_snippet: None,
            })
            .collect()
    }

    fn hydrate_session_list_rows(
        &self,
        scope_key: &ScopeKey,
        mut rows: Vec<SessionListRow>,
    ) -> Result<Vec<SessionListRow>> {
        let mut statement = self.conn.prepare(
            "SELECT data_json FROM scoped_sessions
             WHERE scope_key = ?1 AND id = ?2 AND agent = ?3 AND path = ?4",
        )?;
        for row in &mut rows {
            let data_json = statement.query_row(
                params![
                    scope_key.as_str(),
                    row.session.id,
                    agent_label(row.session.agent),
                    row.session.path.display().to_string(),
                ],
                |record| record.get::<_, String>(0),
            )?;
            let session = serde_json::from_str::<SessionRecord>(&data_json).with_context(|| {
                format!(
                    "invalid cached session row {}:{}",
                    row.session.agent.label(),
                    row.session.id
                )
            })?;
            row.session = Self::normalize_cached_session(session);
        }
        Ok(rows)
    }

    fn session_list_sql_order(sort_key: &str, sort_direction: &str) -> Option<String> {
        let direction = if sort_direction == "desc" {
            "DESC"
        } else {
            "ASC"
        };
        Some(match sort_key {
            "startedAt" => format!(
                "CASE WHEN started_at IS NULL THEN 0 WHEN started_at_ms IS NULL THEN 1 ELSE 2 END {direction}, \
                 CASE WHEN started_at_ms IS NOT NULL THEN started_at_ms END {direction}, \
                 CASE WHEN started_at_ms IS NULL THEN started_at END {direction}"
            ),
            "updatedAt" => format!(
                "CASE WHEN updated_at IS NULL THEN 0 WHEN updated_at_ms IS NULL THEN 1 ELSE 2 END {direction}, \
                 CASE WHEN updated_at_ms IS NOT NULL THEN updated_at_ms END {direction}, \
                 CASE WHEN updated_at_ms IS NULL THEN updated_at END {direction}"
            ),
            "messages" => format!("COALESCE(message_count, 0) {direction}"),
            "turns" => format!("COALESCE(turn_count, 0) {direction}"),
            "cacheRate" => format!(
                "CASE WHEN input_tokens > 0 THEN COALESCE(cached_input_tokens, 0) * 1.0 / input_tokens ELSE -1.0 END {direction}"
            ),
            _ => return None,
        })
    }

    fn list_session_page_from_table(
        &self,
        scope_key: &ScopeKey,
        query: &SessionListQuery,
    ) -> Result<SessionListSqlPageSnapshot> {
        let mut filters = vec!["scope_key = ?1", "json_valid(data_json)"];
        let mut sql_params = vec![SqlValue::Text(scope_key.as_str().to_string())];
        if let Some(agent) = query.agent {
            filters.push("agent = ?2");
            sql_params.push(SqlValue::Text(agent_label(agent).to_string()));
        }
        let common_where = filters.join(" AND ");
        let total_where = if query.show_child_sessions {
            common_where.clone()
        } else {
            format!("{common_where} AND parent_session_id IS NULL")
        };
        let total = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM scoped_sessions WHERE {total_where}"),
            params_from_iter(sql_params.iter()),
            |row| row.get::<_, i64>(0),
        )?;
        let child_session_count = self.conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM scoped_sessions WHERE {common_where}
                 AND parent_session_id IS NOT NULL"
            ),
            params_from_iter(sql_params.iter()),
            |row| row.get::<_, i64>(0),
        )?;
        let total = usize::try_from(total.max(0)).context("session total is too large")?;
        let child_session_count = usize::try_from(child_session_count.max(0))
            .context("child session count is too large")?;
        let page_count = total.div_ceil(query.page_size).max(1);
        let page = query.page.min(page_count - 1);
        let page_start = if total == 0 {
            0
        } else {
            page.checked_mul(query.page_size)
                .context("session page offset is too large")?
        };
        let page_end = (page_start + query.page_size).min(total);
        let order = Self::session_list_sql_order(&query.sort_key, &query.sort_direction)
            .context("unsupported SQL session list sort key")?;
        let updated_tie_order = Self::session_list_sql_order("updatedAt", "desc")
            .context("updatedAt is not SQL-orderable")?;
        let limit = i64::try_from(query.page_size).context("session page size is too large")?;
        let offset = i64::try_from(page_start).context("session page offset is too large")?;
        let mut page_params = sql_params;
        page_params.push(SqlValue::Integer(limit));
        page_params.push(SqlValue::Integer(offset));
        let mut statement = self.conn.prepare(&format!(
            "SELECT data_json FROM scoped_sessions
             WHERE {total_where}
             ORDER BY {order}, {updated_tie_order}, id ASC
             LIMIT ?{} OFFSET ?{}",
            page_params.len() - 1,
            page_params.len(),
        ))?;
        let rows = statement.query_map(params_from_iter(page_params.iter()), |row| {
            row.get::<_, String>(0)
        })?;
        let mut page_payloads = Vec::new();
        for row in rows {
            page_payloads.push(row?);
        }
        Ok(SessionListSqlPageSnapshot {
            page_payloads,
            total,
            child_session_count,
            page,
            page_count,
            page_start,
            page_end,
        })
    }

    pub(in crate::storage) fn prepare_session_sources(
        &self,
        scope_key: &ScopeKey,
        sessions: &[SessionRecord],
    ) -> Result<PreparedSessionSources> {
        let mut sources = PreparedSessionSources::new();
        let mut statement = self.conn.prepare("SELECT data_json FROM scoped_sessions WHERE scope_key = ?1 AND id = ?2 AND agent = ?3 LIMIT 1")?;
        for session in sessions {
            if !sources.contains_key(&session.path) {
                sources.insert(
                    session.path.clone(),
                    self.prepare_session_source(scope_key, session)?,
                );
            }
            let current = statement
                .query_row(
                    params![scope_key.as_str(), session.id, agent_label(session.agent)],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if let Some(current) = current {
                let current: SessionRecord = serde_json::from_str(&current)?;
                if !sources.contains_key(&current.path) {
                    sources.insert(
                        current.path.clone(),
                        self.prepare_session_source(scope_key, &current)?,
                    );
                }
            }
        }
        Ok(sources)
    }

    fn prepare_session_source(
        &self,
        scope: &ScopeKey,
        session: &SessionRecord,
    ) -> Result<PreparedSessionSource> {
        // File identity checks belong to preparation, outside the writer lease.
        // mtime/size alone misses rename-over replacements preserving timestamps.
        let current = self.conn.query_row(
            "SELECT search_index_version, search_checkpoint, file_mtime, file_size
             FROM scoped_session_search_index WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
            params![scope.as_str(), session.id, agent_label(session.agent), session.path.to_string_lossy()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?))
        ).optional()?;
        let search_needs_refresh =
            current
                .as_ref()
                .is_none_or(|(version, encoded, mtime, size)| {
                    if *version != SESSION_SEARCH_INDEX_VERSION {
                        return true;
                    }
                    if let Some(checkpoint) = encoded.as_deref().and_then(|encoded| {
                        serde_json::from_str::<transcript::SearchCheckpoint>(encoded).ok()
                    }) {
                        let parser = crate::providers::agent_provider(session.agent)
                            .transcript_search_append_version()
                            .unwrap_or("full-search-v1");
                        checkpoint.parser_version != parser
                            || !checkpoint.matches_source(&session.path).unwrap_or(false)
                    } else {
                        !(*mtime == 0
                            && *size == 0
                            && fs::metadata(&session.path)
                                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound))
                    }
                });
        Ok(PreparedSessionSource {
            sources: session_scan_source_states(session),
            search_needs_refresh,
        })
    }

    pub fn list_sessions_for_scope(&self, scope_key: &ScopeKey) -> Result<SessionScan> {
        with_database_read_lock_retry(|| self.list_sessions_for_scope_once(scope_key))
    }

    pub fn session_snapshot_for_scope(
        &self,
        scope_key: &ScopeKey,
    ) -> Result<(Revision, SessionScan)> {
        self.read_session_revisioned(scope_key, || self.list_sessions_for_scope(scope_key))
    }

    pub(in crate::storage) fn read_session_revisioned<T>(
        &self,
        scope_key: &ScopeKey,
        read: impl FnOnce() -> Result<T>,
    ) -> Result<(Revision, T)> {
        // A WAL read transaction keeps the revision and rows on the same snapshot
        // without acquiring the application database write lock.
        let tx = Transaction::new_unchecked(&self.conn, rusqlite::TransactionBehavior::Deferred)?;
        let revision = self
            .projection_head(scope_key, "sessions")?
            .map(|head| head.revision)
            .unwrap_or(Revision::ZERO);
        let value = read()?;
        tx.commit()?;
        Ok((revision, value))
    }

    pub(in crate::storage) fn list_sessions_for_scope_once(
        &self,
        scope_key: &ScopeKey,
    ) -> Result<SessionScan> {
        self.list_sessions_from_table(scope_key)
    }
}
