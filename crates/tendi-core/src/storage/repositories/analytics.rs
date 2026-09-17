//! analytics persistence through the database-owned transaction boundary.
use super::super::*;

impl Store {
    pub fn analytics_revision(&self) -> Result<u64> {
        with_database_read_lock_retry(|| self.analytics_revision_once())
    }

    pub(in crate::storage) fn analytics_revision_once(&self) -> Result<u64> {
        self.conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'analytics_revision'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| value.parse::<u64>().context("invalid analytics revision"))
            .transpose()
            .map(|value| value.unwrap_or(0))
    }

    pub(in crate::storage) fn analytics_overview_index_warnings_for_scope(
        &self,
        scope_key: &ScopeKey,
        agent: Option<AgentKind>,
    ) -> Result<Vec<String>> {
        let agent_value = agent.map(agent_label);
        let mut stmt = self.conn.prepare(
            "SELECT session_path, overview_index_error
             FROM scoped_session_analytics
             WHERE overview_index_error IS NOT NULL
               AND scope_key = ?1
               AND (?2 IS NULL OR agent = ?2)",
        )?;
        stmt.query_map(params![scope_key.as_str(), agent_value], |row| {
            Ok(format!(
                "analytics index unavailable for {}: {}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
    }

    pub fn refresh_session_analytics_for_scope_with_progress<F>(
        &self,
        scope_key: &ScopeKey,
        sessions: &[SessionRecord],
        on_progress: F,
    ) -> Result<AnalyticsRefreshReport>
    where
        F: FnMut(AnalyticsRefreshProgress),
    {
        self.refresh_session_analytics_for_scope_with_progress_inner(
            scope_key,
            sessions,
            on_progress,
        )
    }

    pub(in crate::storage) fn refresh_session_analytics_for_scope_with_progress_inner<F>(
        &self,
        scope_key: &ScopeKey,
        sessions: &[SessionRecord],
        mut on_progress: F,
    ) -> Result<AnalyticsRefreshReport>
    where
        F: FnMut(AnalyticsRefreshProgress),
    {
        let mut warnings = Vec::new();
        let mut report = AnalyticsRefreshReport {
            total: sessions.len(),
            parsed: 0,
            appended: 0,
            skipped: 0,
            failed: 0,
            warnings: Vec::new(),
        };
        on_progress(analytics_refresh_progress(&report, 0));

        for (batch_index, batch) in sessions.chunks(SESSION_ANALYTICS_BATCH_SIZE).enumerate() {
            let mut updates = Vec::new();
            {
                let mut state_stmt = self.conn.prepare(
                    "SELECT file_mtime, file_size, parser_state_json
                     FROM scoped_session_analytics
                     WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
                )?;
                let mut cache_stmt = self.conn.prepare(
                    "SELECT file_mtime, file_size, analytics_json, parser_state_json
                     FROM scoped_session_analytics
                     WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
                )?;

                for session in batch {
                    let session_path = session.path.display().to_string();
                    let cached_state = state_stmt
                        .query_row(
                            params![
                                scope_key.as_str(),
                                session.id,
                                agent_label(session.agent),
                                session_path,
                            ],
                            |row| {
                                Ok((
                                    row.get::<_, i64>(0)?,
                                    row.get::<_, i64>(1)?,
                                    row.get::<_, String>(2)?,
                                ))
                            },
                        )
                        .optional()?;
                    let file_state = match crate::session_skills::session_file_state(&session.path)
                    {
                        Ok(state) => state,
                        Err(err) => {
                            report.failed += 1;
                            warnings.push(format!(
                                "analytics skipped {} {}: {err}",
                                agent_label(session.agent),
                                session.path.display()
                            ));
                            continue;
                        }
                    };
                    if cached_state.is_some_and(|(file_mtime, file_size, parser_state_json)| {
                        file_mtime == file_state.file_mtime
                            && file_size == file_state.file_size
                            && analytics::parser_state_is_current(&parser_state_json)
                    }) {
                        report.skipped += 1;
                        continue;
                    }
                    let cached_row = cache_stmt
                        .query_row(
                            params![
                                scope_key.as_str(),
                                session.id,
                                agent_label(session.agent),
                                session.path.display().to_string(),
                            ],
                            |row| {
                                Ok((
                                    row.get::<_, i64>(0)?,
                                    row.get::<_, i64>(1)?,
                                    row.get::<_, String>(2)?,
                                    row.get::<_, String>(3)?,
                                ))
                            },
                        )
                        .optional()?;
                    let cached = cached_row.and_then(
                        |(file_mtime, file_size, analytics_json, parser_state_json)| match (
                            serde_json::from_str(&analytics_json),
                            serde_json::from_str(&parser_state_json),
                        ) {
                            (Ok(analytics), Ok(state)) => Some(SessionAnalyticsRecord {
                                analytics,
                                state,
                                file_mtime,
                                file_size,
                            }),
                            (Err(err), _) | (_, Err(err)) => {
                                warnings.push(format!(
                                    "invalid scoped analytics cache row for {}: {err}",
                                    session.path.display()
                                ));
                                None
                            }
                        },
                    );
                    let was_append = cached.as_ref().is_some_and(|record| {
                        record.file_size >= 0 && record.file_size < file_state.file_size
                    });
                    match analytics::analyze_session(session, cached.as_ref()) {
                        Ok(record) => {
                            report.parsed += 1;
                            report.appended += usize::from(was_append);
                            updates.push(record);
                        }
                        Err(err) => {
                            report.failed += 1;
                            warnings.push(format!(
                                "analytics failed {} {}: {err}",
                                agent_label(session.agent),
                                session.path.display()
                            ));
                        }
                    }
                }
            }

            self.save_session_analytics_records_for_scope(scope_key, &updates)?;
            let completed = ((batch_index + 1) * SESSION_ANALYTICS_BATCH_SIZE).min(sessions.len());
            on_progress(analytics_refresh_progress(&report, completed));
        }

        let has_stale_rows = self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM scoped_session_analytics_overview overview
                WHERE overview.scope_key = ?1
                  AND NOT EXISTS (
                      SELECT 1 FROM scoped_sessions
                      WHERE scoped_sessions.scope_key = overview.scope_key
                        AND scoped_sessions.id = overview.session_id
                        AND scoped_sessions.agent = overview.agent
                        AND scoped_sessions.path = overview.session_path
                  )
            ) OR EXISTS(
                SELECT 1 FROM scoped_session_analytics analytics
                WHERE analytics.scope_key = ?1
                  AND NOT EXISTS (
                      SELECT 1 FROM scoped_sessions
                      WHERE scoped_sessions.scope_key = analytics.scope_key
                        AND scoped_sessions.id = analytics.session_id
                        AND scoped_sessions.agent = analytics.agent
                        AND scoped_sessions.path = analytics.session_path
                  )
            )",
            [scope_key.as_str()],
            |row| row.get::<_, bool>(0),
        )?;
        if has_stale_rows {
            self.with_named_write_transaction("prune_session_analytics", |tx| {
                tx.execute(
                    "DELETE FROM scoped_session_analytics_overview
                 WHERE scope_key = ?1
                   AND NOT EXISTS (
                       SELECT 1 FROM scoped_sessions
                       WHERE scoped_sessions.scope_key = scoped_session_analytics_overview.scope_key
                         AND scoped_sessions.id = scoped_session_analytics_overview.session_id
                         AND scoped_sessions.agent = scoped_session_analytics_overview.agent
                         AND scoped_sessions.path = scoped_session_analytics_overview.session_path
                   )",
                    [scope_key.as_str()],
                )?;
                tx.execute(
                    "DELETE FROM scoped_session_analytics
                 WHERE scope_key = ?1
                   AND NOT EXISTS (
                       SELECT 1 FROM scoped_sessions
                       WHERE scoped_sessions.scope_key = scoped_session_analytics.scope_key
                         AND scoped_sessions.id = scoped_session_analytics.session_id
                         AND scoped_sessions.agent = scoped_session_analytics.agent
                         AND scoped_sessions.path = scoped_session_analytics.session_path
                   )",
                    [scope_key.as_str()],
                )?;
                Ok(())
            })?;
        }
        report.warnings = warnings;
        Ok(report)
    }

    pub(in crate::storage) fn save_session_analytics_records_for_scope(
        &self,
        scope_key: &ScopeKey,
        records: &[SessionAnalyticsRecord],
    ) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        struct PreparedAnalytics<'a> {
            record: &'a SessionAnalyticsRecord,
            overview: analytics::AnalyticsOverviewIndex,
            overview_record: SessionAnalyticsOverviewRecord,
            session_path: String,
            analytics_json: String,
            parser_state_json: String,
            overview_json: String,
        }
        // Aggregation and JSON encoding scale with the transcript, not the row
        // count. Complete them before entering the shared database writer.
        let prepared = records
            .iter()
            .map(|record| {
                let overview_record = analytics::overview_record(record);
                Ok(PreparedAnalytics {
                    record,
                    overview: record.analytics.overview_index(&record.state),
                    session_path: record.analytics.session_path.display().to_string(),
                    analytics_json: serde_json::to_string(&record.analytics)?,
                    parser_state_json: serde_json::to_string(&record.state)?,
                    overview_json: serde_json::to_string(&overview_record)?,
                    overview_record,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let source_version = SourceVersion::new(PROJECTION_PARSER_VERSION)
            .map_err(|error| anyhow::anyhow!(error))?;
        let indexed_at = unix_now().to_string();
        self.with_named_write_transaction("save_session_analytics_records_for_scope", |tx| {
        for prepared in &prepared {
            let record = prepared.record;
            let overview = &prepared.overview;
            let overview_record = &prepared.overview_record;
            tx.execute(
                "INSERT INTO scoped_session_analytics (
                    scope_key, session_id, agent, session_path, file_mtime, file_size,
                    indexed_at, analytics_json, parser_state_json,
                    event_min_date, event_max_date, has_activity,
                    capability_token_usage, capability_reasoning_tokens,
                    capability_explicit_runs, capability_rate_limit_history,
                    overview_indexed, overview_index_error
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 1, NULL)
                 ON CONFLICT(scope_key, session_id, agent, session_path) DO UPDATE SET
                    file_mtime = excluded.file_mtime,
                    file_size = excluded.file_size,
                    indexed_at = excluded.indexed_at,
                    analytics_json = excluded.analytics_json,
                    parser_state_json = excluded.parser_state_json,
                    event_min_date = excluded.event_min_date,
                    event_max_date = excluded.event_max_date,
                    has_activity = excluded.has_activity,
                    capability_token_usage = excluded.capability_token_usage,
                    capability_reasoning_tokens = excluded.capability_reasoning_tokens,
                    capability_explicit_runs = excluded.capability_explicit_runs,
                    capability_rate_limit_history = excluded.capability_rate_limit_history,
                    overview_indexed = 1,
                    overview_index_error = NULL",
                params![
                    scope_key.as_str(),
                    record.analytics.session_id,
                    agent_label(record.analytics.agent),
                    prepared.session_path,
                    record.file_mtime,
                    record.file_size,
                    indexed_at,
                    prepared.analytics_json,
                    prepared.parser_state_json,
                    overview.first,
                    overview.last,
                    overview.has_activity,
                    overview.capabilities.token_usage,
                    overview.capabilities.reasoning_tokens,
                    overview.capabilities.explicit_runs,
                    overview.capabilities.rate_limit_history,
                ],
            )?;
            tx.execute(
                "INSERT INTO scoped_session_analytics_overview (
                    scope_key, session_id, agent, session_path, event_min_date,
                    event_max_date, has_activity, overview_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(scope_key, session_id, agent, session_path) DO UPDATE SET
                    event_min_date = excluded.event_min_date,
                    event_max_date = excluded.event_max_date,
                    has_activity = excluded.has_activity,
                    overview_json = excluded.overview_json",
                params![
                    scope_key.as_str(),
                    &overview_record.session_id,
                    agent_label(overview_record.agent),
                    prepared.session_path,
                    &overview_record.first,
                    &overview_record.last,
                    overview_record.has_activity,
                    prepared.overview_json,
                ],
            )?;
        }
        advance_projection_head_in_tx(tx, scope_key, "analytics", Some(&source_version), "ready")?;
            Ok(())
        })?;
        Ok(())
    }

    /// Return analytics for one workspace only. Scoped analytics are stored in
    /// their own composite-key projection, so the workspace boundary is
    /// enforced by SQL rather than by filtering a global cache in memory.
    pub fn overview_analytics_for_scope(
        &self,
        scope_key: &ScopeKey,
        agent: Option<AgentKind>,
        days: u32,
        rank_days: u32,
    ) -> Result<OverviewAnalytics> {
        self.overview_analytics_for_scope_until(scope_key, agent, days, rank_days, None)
    }

    pub fn overview_analytics_for_scope_until(
        &self,
        scope_key: &ScopeKey,
        agent: Option<AgentKind>,
        days: u32,
        rank_days: u32,
        end_date: Option<&str>,
    ) -> Result<OverviewAnalytics> {
        let scoped_sessions = self.list_sessions_for_scope(scope_key)?.sessions;
        let allowed = scoped_sessions
            .iter()
            .filter(|session| agent.is_none_or(|expected| session.agent == expected))
            .map(|session| (session.id.clone(), session.agent, session.path.clone()))
            .collect::<HashSet<_>>();
        let today = end_date
            .map(|value| {
                chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                    .with_context(|| format!("invalid analytics end date: {value}"))
            })
            .transpose()?
            .unwrap_or_else(|| chrono::Local::now().date_naive());
        let days = days.clamp(1, 365);
        let rank_days = rank_days.clamp(1, 730);
        let since = today - chrono::Duration::days(i64::from(days.saturating_sub(1)));
        let rank_since = today - chrono::Duration::days(i64::from(rank_days.saturating_sub(1)));
        let cutoff = since.min(rank_since).to_string();
        let (records, warnings) = self.load_session_analytics_overview_records_for_scope(
            scope_key, agent, &cutoff, end_date,
        )?;
        let mut overview = analytics::aggregate_overview_records_until(
            &records,
            days,
            rank_days,
            Some(today),
            warnings,
        );
        let mut capabilities = BTreeMap::<AgentKind, AnalyticsCapabilities>::new();
        for record in &records {
            let provider_capabilities = AnalyticsCapabilities::for_agent(record.agent);
            let entry = capabilities
                .entry(record.agent)
                .or_insert(provider_capabilities);
            entry.token_usage |= provider_capabilities.token_usage;
            entry.reasoning_tokens |= provider_capabilities.reasoning_tokens;
            entry.explicit_runs |= provider_capabilities.explicit_runs;
            entry.duration |= provider_capabilities.duration;
            entry.rate_limit_history |= provider_capabilities.rate_limit_history;
        }
        let total_sessions = allowed.len();
        let agent_value = agent.map(agent_label);
        let (first, last, indexed_sessions, analyzed_sessions) = self.conn.query_row(
            "SELECT MIN(overview.event_min_date),
                    MAX(overview.event_max_date),
                    COUNT(*),
                    COALESCE(SUM(CASE WHEN overview.has_activity THEN 1 ELSE 0 END), 0)
             FROM scoped_session_analytics_overview AS overview
             WHERE overview.scope_key = ?1
               AND (?2 IS NULL OR overview.agent = ?2)
               AND EXISTS (
                   SELECT 1 FROM scoped_sessions AS session
                   WHERE session.scope_key = overview.scope_key
                     AND session.id = overview.session_id
                     AND session.agent = overview.agent
                     AND session.path = overview.session_path
               )",
            params![scope_key.as_str(), agent_value],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)? as usize,
                    row.get::<_, i64>(3)? as usize,
                ))
            },
        )?;
        overview.revision = self
            .projection_head(scope_key, "analytics")?
            .map(|head| head.revision.value())
            .unwrap_or(0);
        overview.coverage = AnalyticsCoverage {
            first,
            last,
            total_sessions,
            analyzed_sessions,
            indexing_sessions: total_sessions.saturating_sub(indexed_sessions),
        };
        overview.capabilities = capabilities
            .into_iter()
            .map(|(agent, capabilities)| AnalyticsProviderCapability {
                agent,
                capabilities,
            })
            .collect();
        overview
            .warnings
            .extend(self.analytics_overview_index_warnings_for_scope(scope_key, agent)?);
        Ok(overview)
    }

    pub(in crate::storage) fn load_session_analytics_overview_records_for_scope(
        &self,
        scope_key: &ScopeKey,
        agent: Option<AgentKind>,
        cutoff: &str,
        end_date: Option<&str>,
    ) -> Result<(Vec<SessionAnalyticsOverviewRecord>, Vec<String>)> {
        let agent_value = agent.map(agent_label);
        let mut records = Vec::new();
        let mut warnings = Vec::new();
        let mut invalid = false;
        let mut stmt = self.conn.prepare(
            "SELECT overview_json
             FROM scoped_session_analytics_overview
             WHERE scope_key = ?1
               AND (?2 IS NULL OR agent = ?2)
               AND event_max_date >= ?3
               AND (?4 IS NULL OR event_min_date <= ?4)",
        )?;
        let rows = stmt.query_map(
            params![scope_key.as_str(), agent_value, cutoff, end_date],
            |row| row.get::<_, String>(0),
        )?;
        for row in rows {
            match serde_json::from_str::<SessionAnalyticsOverviewRecord>(&row?) {
                Ok(record) => records.push(record),
                Err(error) => {
                    invalid = true;
                    warnings.push(format!(
                        "invalid scoped analytics overview cache row: {error}"
                    ));
                }
            }
        }
        drop(stmt);

        if invalid {
            records.clear();
            let mut stmt = self.conn.prepare(
                "SELECT analytics_json, parser_state_json
                 FROM scoped_session_analytics
                 WHERE scope_key = ?1
                   AND (?2 IS NULL OR agent = ?2)
                   AND overview_indexed = 1
                   AND event_max_date >= ?3",
            )?;
            let rows = stmt.query_map(params![scope_key.as_str(), agent_value, cutoff], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (analytics_json, parser_state_json) = row?;
                match (
                    serde_json::from_str::<crate::analytics::SessionAnalytics>(&analytics_json),
                    serde_json::from_str::<crate::analytics::AnalyticsParserState>(
                        &parser_state_json,
                    ),
                ) {
                    (Ok(analytics), Ok(state)) => {
                        records.push(analytics::overview_record(&SessionAnalyticsRecord {
                            analytics,
                            state,
                            file_mtime: 0,
                            file_size: 0,
                        }));
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        warnings.push(format!("invalid scoped analytics cache row: {error}"));
                    }
                }
            }
        }
        Ok((records, warnings))
    }
}
