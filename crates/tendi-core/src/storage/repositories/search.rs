//! search persistence through the database-owned transaction boundary.
use super::super::*;

impl Store {
    pub(in crate::storage) fn session_search_candidate_join(
        &self,
        candidates: Option<&[SessionIdentity]>,
    ) -> Result<String> {
        // Keep the large candidate set indexed on this connection. A JSON
        // CTE would avoid SQLite's expression-depth limit but scan every
        // candidate for every FTS hit.
        let Some(candidates) = candidates else {
            return Ok(String::new());
        };
        self.conn.execute_batch(&format!(
            "CREATE TEMP TABLE IF NOT EXISTS {SESSION_SEARCH_CANDIDATE_TABLE} (
                session_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_path TEXT NOT NULL,
                PRIMARY KEY (session_id, agent, session_path)
            )"
        ))?;
        self.conn.execute(
            &format!("DELETE FROM temp.{SESSION_SEARCH_CANDIDATE_TABLE}"),
            [],
        )?;
        let candidates_json =
            serde_json::to_string(candidates).context("serialize session search candidates")?;
        self.conn.execute(
            &format!(
                "INSERT OR IGNORE INTO temp.{SESSION_SEARCH_CANDIDATE_TABLE}
                 SELECT
                    json_extract(value, '$.id'),
                    json_extract(value, '$.agent'),
                    json_extract(value, '$.path')
                 FROM json_each(?1)"
            ),
            [candidates_json],
        )?;
        Ok(format!(
            "JOIN temp.{SESSION_SEARCH_CANDIDATE_TABLE} AS candidate
               ON candidate.session_id = search.session_id
              AND candidate.agent = search.agent
              AND candidate.session_path = search.session_path"
        ))
    }

    pub(in crate::storage) fn session_search_matches(&self, query: &str) -> Result<()> {
        self.conn.execute_batch(&format!(
            "DROP TABLE IF EXISTS temp.{SESSION_SEARCH_MATCH_TABLE};
             CREATE TEMP TABLE {SESSION_SEARCH_MATCH_TABLE} (
                record_id INTEGER PRIMARY KEY,
                bm25_score REAL NOT NULL
            )"
        ))?;
        self.conn.execute(
            &format!(
                "INSERT INTO temp.{SESSION_SEARCH_MATCH_TABLE}(record_id, bm25_score)
                 SELECT rowid,
                        -bm25(scoped_session_search_fts, 0.5, 10.0, 5.0, 6.0, 3.0)
                 FROM scoped_session_search_fts
                 WHERE scoped_session_search_fts MATCH ?1"
            ),
            [query],
        )?;
        Ok(())
    }

    pub(in crate::storage) fn session_search_candidate_order(
        candidates: &[SessionIdentity],
    ) -> HashMap<(String, String, PathBuf), usize> {
        candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                (
                    (
                        candidate.id.clone(),
                        agent_label(candidate.agent).to_owned(),
                        candidate.path.clone(),
                    ),
                    index,
                )
            })
            .collect()
    }

    pub(in crate::storage) fn session_search_hit_key(
        session: &SessionRecord,
    ) -> (String, String, PathBuf) {
        (
            session.id.clone(),
            agent_label(session.agent).to_owned(),
            session.path.clone(),
        )
    }

    /// Search the workspace-owned session projection through its own scoped
    /// FTS index. The legacy global FTS tables are never consulted here.
    pub fn search_sessions_for_scope(
        &self,
        scope_key: &ScopeKey,
        query: &str,
        candidates: Option<&[SessionIdentity]>,
    ) -> Result<Vec<SessionSearchHit>> {
        with_database_read_lock_retry(|| {
            self.search_sessions_for_scope_once(scope_key, query, candidates)
        })
    }

    pub(in crate::storage) fn search_sessions_for_scope_once(
        &self,
        scope_key: &ScopeKey,
        query: &str,
        candidates: Option<&[SessionIdentity]>,
    ) -> Result<Vec<SessionSearchHit>> {
        let terms = session_search_terms(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        if terms.iter().any(|term| term.chars().count() < 3) {
            return self.search_scoped_sessions_by_contains(scope_key, &terms, candidates);
        }
        self.search_scoped_sessions_by_fts(scope_key, &terms, candidates)
    }

    pub(in crate::storage) fn search_scoped_sessions_by_fts(
        &self,
        scope_key: &ScopeKey,
        terms: &[String],
        candidates: Option<&[SessionIdentity]>,
    ) -> Result<Vec<SessionSearchHit>> {
        let query = session_search_query(terms);
        self.session_search_matches(&query)?;
        let sql_params = vec![SqlValue::Text(scope_key.as_str().to_string())];
        let candidate_join = self.session_search_candidate_join(candidates)?;
        let mut stmt = self.conn.prepare(&format!(
            "WITH ranked AS (
                SELECT
                    search.scope_key,
                    search.session_id,
                    search.agent,
                    search.session_path,
                    search.id AS record_id,
                    matches.bm25_score,
                    ROW_NUMBER() OVER (
                        PARTITION BY search.scope_key, search.session_id,
                                     search.agent, search.session_path
                        ORDER BY matches.bm25_score DESC, search.id ASC
                    ) AS row_number
                FROM temp.{SESSION_SEARCH_MATCH_TABLE} AS matches
                JOIN scoped_session_search_records AS search ON search.id = matches.record_id
                JOIN scoped_session_search_index AS published
                  ON published.scope_key = search.scope_key AND published.session_id = search.session_id
                 AND published.agent = search.agent AND published.session_path = search.session_path
                 AND published.search_index_version = {SESSION_SEARCH_INDEX_VERSION}
                {candidate_join}
                WHERE search.scope_key = ?1
            ), matched AS (
                SELECT scope_key, session_id, agent, session_path, record_id, bm25_score
                FROM ranked
                WHERE row_number = 1
            )
            SELECT
                sessions.data_json,
                search.metadata_text,
                search.title,
                search.project,
                search.user_text,
                search.assistant_text,
                matched.bm25_score
             FROM matched
             JOIN scoped_session_search_records AS search
               ON search.id = matched.record_id
             JOIN scoped_sessions AS sessions
               ON sessions.scope_key = matched.scope_key
              AND sessions.id = matched.session_id
              AND sessions.agent = matched.agent
              AND sessions.path = matched.session_path"
        ))?;
        let rows = stmt.query_map(params_from_iter(sql_params.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                SessionSearchDocument {
                    metadata_text: row.get(1)?,
                    title: row.get(2)?,
                    project: row.get(3)?,
                    user_text: row.get(4)?,
                    assistant_text: row.get(5)?,
                },
                row.get::<_, f64>(6)?,
            ))
        })?;
        let candidate_order = candidates.map(Self::session_search_candidate_order);
        let mut hits: Vec<SessionSearchHit> = Vec::new();
        let mut seen = HashSet::new();
        for row in rows {
            let (data_json, document, search_score) = row?;
            let session = Self::normalize_cached_session(
                serde_json::from_str::<SessionRecord>(&data_json)
                    .context("invalid cached scoped session search row")?,
            );
            if !seen.insert(Self::session_search_hit_key(&session)) {
                continue;
            }
            hits.push(SessionSearchHit {
                session,
                search_score,
                search_snippet: contains_search_snippet(&document, terms),
            });
        }
        if let Some(candidate_order) = candidate_order {
            hits.sort_by_key(|hit| {
                candidate_order
                    .get(&Self::session_search_hit_key(&hit.session))
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        } else {
            hits.sort_by(Self::compare_session_search_hits);
        }
        Ok(hits)
    }

    pub(in crate::storage) fn search_scoped_sessions_by_contains(
        &self,
        scope_key: &ScopeKey,
        terms: &[String],
        candidates: Option<&[SessionIdentity]>,
    ) -> Result<Vec<SessionSearchHit>> {
        let where_clause = terms
            .iter()
            .map(|_| {
                "LOWER(
                    COALESCE(search.title, '') || ' ' ||
                    COALESCE(search.project, '') || ' ' ||
                    COALESCE(search.metadata_text, '') || ' ' ||
                    COALESCE(search.user_text, '') || ' ' ||
                    COALESCE(search.assistant_text, '')
                ) LIKE ? ESCAPE '\\'"
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut sql_params = vec![SqlValue::Text(scope_key.as_str().to_string())];
        sql_params.extend(
            terms
                .iter()
                .map(|term| SqlValue::Text(format!("%{}%", escape_like(term)))),
        );
        let candidate_join = self.session_search_candidate_join(candidates)?;
        let sql = format!(
            "WITH matched AS (
                SELECT
                    search.scope_key,
                    search.session_id,
                    search.agent,
                    search.session_path,
                    MIN(search.id) AS record_id
                FROM scoped_session_search_records AS search
                JOIN scoped_session_search_index AS published
                  ON published.scope_key = search.scope_key AND published.session_id = search.session_id
                 AND published.agent = search.agent AND published.session_path = search.session_path
                 AND published.search_index_version = {SESSION_SEARCH_INDEX_VERSION}
                {candidate_join}
                WHERE search.scope_key = ?1 AND {where_clause}
                GROUP BY search.scope_key, search.session_id, search.agent, search.session_path
            )
            SELECT
                sessions.data_json,
                search.metadata_text,
                search.title,
                search.project,
                search.user_text,
                search.assistant_text
             FROM matched
             JOIN scoped_session_search_records AS search
               ON search.id = matched.record_id
             JOIN scoped_sessions AS sessions
               ON sessions.scope_key = search.scope_key
              AND sessions.id = search.session_id
              AND sessions.agent = search.agent
              AND sessions.path = search.session_path
             "
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(sql_params.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                SessionSearchDocument {
                    metadata_text: row.get(1)?,
                    title: row.get(2)?,
                    project: row.get(3)?,
                    user_text: row.get(4)?,
                    assistant_text: row.get(5)?,
                },
            ))
        })?;
        let candidate_order = candidates.map(Self::session_search_candidate_order);
        let mut hits: Vec<SessionSearchHit> = Vec::new();
        let mut hit_indexes: HashMap<(String, String, PathBuf), usize> = HashMap::new();
        for row in rows {
            let (data_json, document) = row?;
            let session = Self::normalize_cached_session(
                serde_json::from_str::<SessionRecord>(&data_json)
                    .context("invalid cached scoped session search row")?,
            );
            let key = Self::session_search_hit_key(&session);
            let hit = SessionSearchHit {
                search_score: contains_search_score(&document, terms),
                search_snippet: contains_search_snippet(&document, terms),
                session,
            };
            if let Some(index) = hit_indexes.get(&key).copied() {
                if hit.search_score > hits[index].search_score {
                    hits[index] = hit;
                }
            } else {
                hit_indexes.insert(key, hits.len());
                hits.push(hit);
            }
        }
        if let Some(candidate_order) = candidate_order {
            hits.sort_by_key(|hit| {
                candidate_order
                    .get(&Self::session_search_hit_key(&hit.session))
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        } else {
            hits.sort_by(Self::compare_session_search_hits);
        }
        Ok(hits)
    }

    pub(in crate::storage) fn compare_session_search_hits(
        left: &SessionSearchHit,
        right: &SessionSearchHit,
    ) -> std::cmp::Ordering {
        right
            .search_score
            .total_cmp(&left.search_score)
            .then_with(|| Self::compare_session_updated_at(&left.session, &right.session))
    }
}
