use super::*;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionRecallSort {
    #[default]
    Relevance,
    TimeAsc,
    TimeDesc,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionRecallRole {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct SessionRecallOptions {
    pub query: String,
    pub cwd: Vec<PathBuf>,
    pub exact_cwd: bool,
    pub since: Option<String>,
    pub until: Option<String>,
    pub agent: Option<AgentKind>,
    pub role: Option<SessionRecallRole>,
    pub phrase: bool,
    pub sort: SessionRecallSort,
    pub limit: usize,
    pub offset: usize,
}

impl Default for SessionRecallOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            cwd: Vec::new(),
            exact_cwd: false,
            since: None,
            until: None,
            agent: None,
            role: None,
            phrase: false,
            sort: SessionRecallSort::Relevance,
            limit: 20,
            offset: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionRecallStatus {
    pub mode: &'static str,
    pub indexed_sessions: usize,
    pub pending_sessions: usize,
    pub scopes: usize,
    pub last_scan_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionRecallHit {
    pub id: String,
    pub agent: AgentKind,
    pub title: Option<String>,
    pub project: Option<PathBuf>,
    pub path: PathBuf,
    pub started_at: Option<String>,
    pub score: f64,
    pub snippet: String,
    pub role: String,
    /// Position in the provider's searchable user/assistant message stream.
    pub record_order: usize,
    #[serde(skip)]
    pub session: SessionRecord,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionRecallPage {
    pub hits: Vec<SessionRecallHit>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    pub status: SessionRecallStatus,
}

fn date_bound(value: &str) -> Result<String> {
    if let Ok(date) = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Ok(format!("{date}T00:00:00Z"));
    }
    Ok(chrono::DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("invalid date {value}; use YYYY-MM-DD (UTC) or RFC3339"))?
        .to_rfc3339())
}

impl Store {
    pub fn session_recall_status(&self) -> Result<SessionRecallStatus> {
        let indexed_sessions = self.conn.query_row(
            "SELECT count(*) FROM (SELECT session_id, agent, session_path
             FROM scoped_session_search_index WHERE search_index_version = ?1
             GROUP BY session_id, agent, session_path)",
            [SESSION_SEARCH_INDEX_VERSION],
            |row| row.get(0),
        )?;
        let pending_sessions = self.conn.query_row(
            "SELECT count(*) FROM (SELECT session_id, agent, session_path
             FROM scoped_session_search_work GROUP BY session_id, agent, session_path)",
            [],
            |row| row.get(0),
        )?;
        let scopes = self.conn.query_row(
            "SELECT count(DISTINCT scope_key) FROM scoped_session_search_index",
            [],
            |row| row.get(0),
        )?;
        let last_scan_at = self.conn.query_row(
            "SELECT max(CAST(value AS INTEGER)) FROM meta WHERE key LIKE 'sessions_last_scan_at:%'",
            [], |row| row.get::<_, Option<i64>>(0))?.map(|time| time.to_string());
        Ok(SessionRecallStatus {
            mode: "persistent-index",
            indexed_sessions,
            pending_sessions,
            scopes,
            last_scan_at,
        })
    }

    pub fn session_by_id(&self, id: &str, agent: Option<AgentKind>) -> Result<SessionRecord> {
        let mut stmt = self.conn.prepare(
            "SELECT data_json FROM scoped_sessions WHERE id = ?1 AND (?2 IS NULL OR agent = ?2)
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![id, agent.map(agent_label)], |row| {
            row.get::<_, String>(0)
        })?;
        let mut sessions = BTreeMap::new();
        for row in rows {
            let session: SessionRecord = serde_json::from_str(&row?)?;
            sessions
                .entry((agent_label(session.agent).to_string(), session.path.clone()))
                .or_insert(session);
        }
        if sessions.len() > 1 {
            bail!(
                "session {id} has multiple provider/source identities; use --agent or an explicit path"
            );
        }
        sessions
            .into_values()
            .next()
            .with_context(|| format!("session {id} is not indexed; run tendi sessions refresh"))
    }

    /// Query published records across workspaces without scanning provider files.
    /// Workspace projections stay separate; identical source identities are deduplicated.
    pub fn recall_sessions(&self, options: &SessionRecallOptions) -> Result<SessionRecallPage> {
        if options.limit == 0 || options.limit > 1000 {
            bail!("limit must be between 1 and 1000");
        }
        let terms = if options.phrase {
            vec![options.query.trim().to_lowercase()]
        } else {
            session_search_terms(&options.query)
        };
        if terms.is_empty() || terms.iter().any(String::is_empty) {
            bail!("search query must not be empty");
        }
        let since = options.since.as_deref().map(date_bound).transpose()?;
        let until = options.until.as_deref().map(date_bound).transpose()?;
        if let (Some(since), Some(until)) = (&since, &until) {
            if crate::time::compare_timestamps(Some(since), Some(until)).is_ge() {
                bail!("--until must be later than --since");
            }
        }
        with_database_read_lock_retry(|| {
            self.recall_sessions_once(options, &terms, since.as_deref(), until.as_deref())
        })
    }

    fn recall_sessions_once(
        &self,
        options: &SessionRecallOptions,
        terms: &[String],
        since: Option<&str>,
        until: Option<&str>,
    ) -> Result<SessionRecallPage> {
        let text = match options.role {
            Some(SessionRecallRole::User) => "content.user_text",
            Some(SessionRecallRole::Assistant) => "content.assistant_text",
            None => {
                "(search.metadata_text || ' ' || search.title || ' ' || search.project || ' ' || content.user_text || ' ' || content.assistant_text)"
            }
        };
        let mut predicates = vec![format!(
            "published.search_index_version = {SESSION_SEARCH_INDEX_VERSION}"
        )];
        let mut values = Vec::<SqlValue>::new();
        for term in terms {
            predicates.push(format!("instr(lower({text}), ?) > 0"));
            values.push(SqlValue::Text(term.clone()));
        }
        if let Some(agent) = options.agent {
            predicates.push("sessions.agent = ?".into());
            values.push(SqlValue::Text(agent_label(agent).into()));
        }
        for (bound, comparison) in [(since, ">="), (until, "<")] {
            if let Some(bound) = bound {
                predicates.push(format!(
                    "julianday(sessions.started_at) {comparison} julianday(?)"
                ));
                values.push(SqlValue::Text(bound.into()));
            }
        }
        if !options.cwd.is_empty() {
            let mut paths = Vec::new();
            for cwd in &options.cwd {
                let path = canonical_workspace_root(cwd)
                    .to_string_lossy()
                    .trim_end_matches('/')
                    .to_string();
                if options.exact_cwd {
                    paths.push("sessions.project = ?".to_string());
                    values.push(SqlValue::Text(if path.is_empty() {
                        "/".into()
                    } else {
                        path
                    }));
                } else {
                    paths.push(
                        "(sessions.project = ? OR sessions.project LIKE ? ESCAPE '\\')".into(),
                    );
                    values.push(SqlValue::Text(path.clone()));
                    values.push(SqlValue::Text(format!("{}/%", escape_like(&path))));
                }
            }
            predicates.push(format!("({})", paths.join(" OR ")));
        }
        // Long literals use the existing shared trigram index. Short terms are
        // verified against normalized records; no provider parsing happens here.
        let fts = session_search_query(terms);
        let (with_matches, from_matches) = if fts.is_empty() {
            (
                String::new(),
                "FROM scoped_session_search_entries search".to_string(),
            )
        } else {
            values.splice(0..0, [SqlValue::Text(fts.clone()), SqlValue::Text(fts)]);
            ("WITH matches AS MATERIALIZED (
                SELECT entry.id FROM session_search_content_fts
                JOIN scoped_session_search_entries entry ON entry.content_id = session_search_content_fts.rowid
                WHERE session_search_content_fts MATCH ?
                UNION SELECT rowid FROM scoped_session_search_metadata_fts
                WHERE scoped_session_search_metadata_fts MATCH ?
              )".to_string(),
             "FROM matches CROSS JOIN scoped_session_search_entries search ON search.id = matches.id".to_string())
        };
        let mut stmt = self.conn.prepare(&format!(
            "{with_matches} SELECT sessions.data_json, search.record_order, search.metadata_text,
                    search.title, search.project, content.user_text, content.assistant_text
             {from_matches}
             JOIN session_search_content_records content ON content.id = search.content_id
             JOIN scoped_session_search_index published ON published.scope_key = search.scope_key
               AND published.session_id = search.session_id AND published.agent = search.agent
               AND published.session_path = search.session_path
             JOIN scoped_sessions sessions ON sessions.scope_key = search.scope_key
               AND sessions.id = search.session_id AND sessions.agent = search.agent AND sessions.path = search.session_path
             WHERE {}", predicates.join(" AND ")))?;
        let rows = stmt.query_map(params_from_iter(values.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, usize>(1)?,
                SessionSearchDocument {
                    metadata_text: row.get(2)?,
                    title: row.get(3)?,
                    project: row.get(4)?,
                    user_text: row.get(5)?,
                    assistant_text: row.get(6)?,
                },
            ))
        })?;
        let mut hits: BTreeMap<(String, String, PathBuf), SessionRecallHit> = BTreeMap::new();
        for row in rows {
            let (encoded, record_order, document) = row?;
            let session: SessionRecord = serde_json::from_str(&encoded)?;
            let role = if !document.user_text.is_empty() {
                "user"
            } else if !document.assistant_text.is_empty() {
                "assistant"
            } else {
                "metadata"
            };
            let title_matches = session.title.as_deref().is_some_and(|title| {
                let title = title.to_lowercase();
                terms.iter().all(|term| title.contains(term))
            });
            let score =
                contains_search_score(&document, terms) + if title_matches { 30.0 } else { 0.0 };
            let hit = SessionRecallHit {
                id: session.id.clone(),
                agent: session.agent,
                title: session.title.clone(),
                project: session.project.clone(),
                path: session.path.clone(),
                started_at: session.started_at.clone(),
                score,
                snippet: contains_search_snippet(&document, terms),
                role: role.into(),
                record_order,
                session,
            };
            let key = (
                hit.id.clone(),
                agent_label(hit.agent).to_string(),
                hit.path.clone(),
            );
            if hits.get(&key).is_none_or(|existing| {
                hit.score > existing.score
                    || (hit.score == existing.score && hit.record_order < existing.record_order)
            }) {
                hits.insert(key, hit);
            }
        }
        let mut hits: Vec<_> = hits.into_values().collect();
        hits.sort_by(|a, b| {
            match (&a.started_at, &b.started_at) {
                (None, Some(_)) => return std::cmp::Ordering::Greater,
                (Some(_), None) => return std::cmp::Ordering::Less,
                _ => {}
            }
            let time =
                crate::time::compare_timestamps(a.started_at.as_deref(), b.started_at.as_deref());
            match options.sort {
                SessionRecallSort::Relevance => b.score.total_cmp(&a.score).then(time.reverse()),
                SessionRecallSort::TimeAsc => time,
                SessionRecallSort::TimeDesc => time.reverse(),
            }
            .then(a.id.cmp(&b.id))
            .then(a.path.cmp(&b.path))
        });
        let total = hits.len();
        let hits = hits
            .into_iter()
            .skip(options.offset)
            .take(options.limit)
            .collect();
        Ok(SessionRecallPage {
            hits,
            total,
            limit: options.limit,
            offset: options.offset,
            status: self.session_recall_status()?,
        })
    }
}

#[cfg(test)]
#[path = "session_recall_tests.rs"]
mod tests;
