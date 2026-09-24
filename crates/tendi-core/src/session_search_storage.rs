//! Search preparation never runs inside the shared database write lock.
//! A scope lock serializes index builders; bounded transactions yield the database
//! to unrelated writers. An index is published only after every batch commits.

use super::*;

const SEARCH_WRITE_BATCH_BYTES: usize = 256 * 1024;
const SEARCH_WRITE_BATCH_ROWS: usize = 128;
// A cooperative SQL work budget, not a hard deadline: one statement and COMMIT
// finish atomically. Large FTS rows can exceed it and remain visible in metrics.
const SEARCH_WRITE_BATCH_TARGET: Duration = Duration::from_millis(10);
const SESSION_SEARCH_PROJECTION_DOMAIN: &str = "session-search";

type CurrentSessionSearchState = (i64, i64, String, i64, bool, Option<String>);

/// The exact metadata and revision boundary committed by one index publication.
/// Callers must not widen this span across unrelated metadata transactions.
#[derive(Debug, Clone)]
pub struct SessionSearchPublication {
    pub session: SessionRecord,
    /// Revisions from the dedicated `session-search` projection stream.
    pub base_revision: Revision,
    pub revision: Revision,
}

impl SessionSearchPublication {
    /// This receipt is internal to the search index lifecycle. It is not a
    /// sessions scan event and must not be published through that contract.
    pub const DOMAIN: &'static str = SESSION_SEARCH_PROJECTION_DOMAIN;
}

#[derive(Debug)]
struct SearchRefreshCancelled;

impl std::fmt::Display for SearchRefreshCancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("session search refresh cancelled")
    }
}

impl std::error::Error for SearchRefreshCancelled {}

fn check_search_cancellation(should_stop: &dyn Fn() -> bool) -> Result<()> {
    if should_stop() {
        return Err(SearchRefreshCancelled.into());
    }
    Ok(())
}

impl Store {
    fn session_search_current_state(
        &self,
        scope: &ScopeKey,
        session: &SessionRecord,
    ) -> Result<Option<CurrentSessionSearchState>> {
        self.conn
            .query_row(
                "SELECT file_mtime, file_size, search_metadata, search_index_version,
                    EXISTS (SELECT 1 FROM scoped_session_search_entries r
                            WHERE r.scope_key = ?1 AND r.session_id = ?2 AND r.agent = ?3
                              AND r.session_path = ?4 AND r.record_order = 0), search_checkpoint
                 FROM scoped_session_search_index
                 WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
                params![
                    scope.as_str(),
                    session.id,
                    agent_label(session.agent),
                    session.path.to_string_lossy()
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, bool>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(Into::into)
    }

    fn existing_session_search_documents(
        &self,
        scope: &ScopeKey,
        session: &SessionRecord,
        content_current: bool,
        start_record_order: usize,
    ) -> Result<BTreeMap<usize, SessionSearchDocument>> {
        let mut statement = self.conn.prepare(
            "SELECT entry.record_order, entry.metadata_text, entry.title, entry.project,
                    content.user_text, content.assistant_text
             FROM scoped_session_search_entries AS entry
             JOIN session_search_content_records AS content ON content.id = entry.content_id
             WHERE entry.scope_key = ?1 AND entry.session_id = ?2 AND entry.agent = ?3
               AND entry.session_path = ?4
               AND (entry.record_order = 0 OR (?5 = 0 AND entry.record_order >= ?6))
             ORDER BY entry.record_order",
        )?;
        statement
            .query_map(
                params![
                    scope.as_str(),
                    session.id,
                    agent_label(session.agent),
                    session.path.to_string_lossy(),
                    content_current,
                    start_record_order as i64
                ],
                |row| {
                    Ok((
                        row.get::<_, usize>(0)?,
                        SessionSearchDocument {
                            metadata_text: row.get(1)?,
                            title: row.get(2)?,
                            project: row.get(3)?,
                            user_text: row.get(4)?,
                            assistant_text: row.get(5)?,
                        },
                    ))
                },
            )?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()
            .map_err(Into::into)
    }

    fn orphan_session_search_record_ids(
        &self,
        scope: &ScopeKey,
        session_id: &str,
        agent: &str,
        path: &str,
    ) -> Result<Vec<i64>> {
        self.conn
            .prepare(
                "SELECT r.id FROM scoped_session_search_entries r WHERE r.scope_key = ?1
                 AND r.session_id = ?2 AND r.agent = ?3 AND r.session_path = ?4
                 AND NOT EXISTS (SELECT 1 FROM scoped_sessions s WHERE s.scope_key = r.scope_key
                    AND s.id = r.session_id AND s.agent = r.agent AND s.path = r.session_path)
                 LIMIT ?5",
            )?
            .query_map(
                params![
                    scope.as_str(),
                    session_id,
                    agent,
                    path,
                    SEARCH_WRITE_BATCH_ROWS as i64
                ],
                |row| row.get::<_, i64>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    fn lock_session_search(&self, scope: &ScopeKey) -> Result<crate::coordination::ResourceLease> {
        crate::coordination::ResourceLease::acquire(self.path(), &session_search_lock_key(scope))
    }

    /// Persistent scopes survive a stopped worker or a failed transcript read.
    pub fn pending_session_search_scopes(&self) -> Result<Vec<ScopeKey>> {
        let mut statement = self.conn.prepare(
            "SELECT scope_key FROM scoped_session_search_work GROUP BY scope_key ORDER BY min(requested_at), scope_key",
        )?;
        let scopes = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        scopes
            .into_iter()
            .map(|scope| Ok(ScopeKey::new(scope)?))
            .collect()
    }

    /// Returns successful publications, individual failures, and whether another
    /// pass is required. A failed item never hides another item's committed event.
    pub fn refresh_pending_session_search_for_scope(
        &self,
        scope: &ScopeKey,
    ) -> Result<(Vec<SessionSearchPublication>, Vec<String>, bool)> {
        self.refresh_pending_session_search_for_scope_until(scope, || false)
    }

    /// Cooperative shutdown stops between preparation and short write batches.
    /// Already-committed publications are returned; unacknowledged work persists.
    pub fn refresh_pending_session_search_for_scope_until(
        &self,
        scope: &ScopeKey,
        should_stop: impl Fn() -> bool,
    ) -> Result<(Vec<SessionSearchPublication>, Vec<String>, bool)> {
        if should_stop() {
            return Ok((Vec::new(), Vec::new(), true));
        }
        let Some(_search_lock) = crate::coordination::ResourceLease::try_acquire(
            self.path(),
            &session_search_lock_key(scope),
        )?
        else {
            // A builder in another process owns this scope. Do not block every
            // other pending scope or the daemon's shutdown on its transcript.
            return Ok((Vec::new(), Vec::new(), true));
        };
        self.refresh_pending_session_search_locked(scope, &should_stop)
    }

    fn refresh_pending_session_search_locked(
        &self,
        scope: &ScopeKey,
        should_stop: &dyn Fn() -> bool,
    ) -> Result<(Vec<SessionSearchPublication>, Vec<String>, bool)> {
        // Snapshot only durable dirty identities, never the full workspace.
        let work = self
            .conn
            .prepare(
                "SELECT session_id, agent, session_path, generation FROM scoped_session_search_work
             WHERE scope_key = ?1 ORDER BY requested_at, session_id, agent, session_path",
            )?
            .query_map([scope.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut changed = Vec::new();
        let mut failures = Vec::new();
        for (id, agent, path, generation) in work {
            if should_stop() {
                return Ok((changed, failures, true));
            }
            let outcome = (|| -> Result<bool> {
                let payload: Option<String> = self
                    .conn
                    .query_row(
                        "SELECT data_json FROM scoped_sessions
                     WHERE scope_key = ?1 AND id = ?2 AND agent = ?3 AND path = ?4",
                        params![scope.as_str(), id, agent, path],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(payload) = payload {
                    let session: SessionRecord = serde_json::from_str(&payload)?;
                    let (publication, advanced) =
                        self.refresh_session_search(scope, &session, &payload, should_stop)?;
                    if let Some(publication) = publication {
                        changed.push(publication);
                    }
                    Ok(advanced)
                } else {
                    self.prune_session_search_key(scope, &id, &agent, &path, should_stop)?;
                    Ok(false)
                }
            })();
            let error = match &outcome {
                Err(error) if error.is::<SearchRefreshCancelled>() => {
                    return Ok((changed, failures, true));
                }
                Err(error) => Some(format!("{id}: {error:#}")),
                _ => None,
            };
            let retain = error.is_some() || matches!(outcome, Ok(true));
            if let Some(error) = &error {
                failures.push(error.clone());
            }
            if let Err(error) = self.with_background_write_transaction("session_search.acknowledge", |tx| {
                if retain {
                    tx.execute(
                        "UPDATE scoped_session_search_work SET last_error = ?6
                         WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4 AND generation = ?5",
                        params![scope.as_str(), id, agent, path, generation, error]
                    )?;
                } else {
                    tx.execute(
                        "DELETE FROM scoped_session_search_work
                         WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4 AND generation = ?5",
                        params![scope.as_str(), id, agent, path, generation]
                    )?;
                }
                Ok(())
            }) { failures.push(format!("search acknowledgement: {error:#}")); }
        }
        // Aggregate status is not a source of work: new generations can only be
        // acknowledged by the pass which actually observed that session key.
        if let Err(error) =
            self.with_background_write_transaction("session_search.scope_status", |tx| {
                tx.execute(
                    "DELETE FROM scoped_session_search_pending WHERE scope_key = ?1
                 AND NOT EXISTS(SELECT 1 FROM scoped_session_search_work WHERE scope_key = ?1)",
                    [scope.as_str()],
                )?;
                tx.execute(
                    "UPDATE scoped_session_search_pending SET last_error = ?2 WHERE scope_key = ?1",
                    params![
                        scope.as_str(),
                        (!failures.is_empty()).then(|| failures.join("; "))
                    ],
                )?;
                Ok(())
            })
        {
            failures.push(format!("search scope status: {error:#}"));
        }
        let pending = match self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM scoped_session_search_work WHERE scope_key = ?1)",
            [scope.as_str()],
            |row| row.get::<_, bool>(0),
        ) {
            Ok(pending) => pending,
            Err(error) => {
                failures.push(format!("search status read: {error:#}"));
                true
            }
        };
        Ok((changed, failures, pending))
    }

    /// Explicit synchronous index work; metadata APIs deliberately do not call it.
    pub fn ensure_scoped_session_search_for_scope(&self, scope: &ScopeKey) -> Result<bool> {
        self.with_background_write_transaction("session_search.request", |tx| {
            mark_session_search_pending_in_tx(tx, scope)
        })?;
        let _search_lock = self.lock_session_search(scope)?;
        let (changed, errors, _) = self.refresh_pending_session_search_locked(scope, &|| false)?;
        if !errors.is_empty() {
            bail!("session search refresh failed: {}", errors.join("; "));
        }
        Ok(!changed.is_empty())
    }

    pub fn rebuild_scoped_session_search_for_scope(&self, scope: &ScopeKey) -> Result<usize> {
        let _search_lock = self.lock_session_search(scope)?;
        self.with_background_write_transaction("session_search.rebuild", |tx| {
            tx.execute(
                "UPDATE scoped_session_search_index SET search_index_version = 0 WHERE scope_key = ?1",
                [scope.as_str()],
            )?;
            mark_session_search_pending_in_tx(tx, scope)
        })?;
        let (changed, errors, _) = self.refresh_pending_session_search_locked(scope, &|| false)?;
        if !errors.is_empty() {
            bail!("session search rebuild failed: {}", errors.join("; "));
        }
        Ok(changed.len())
    }

    pub fn optimize_session_search_fts_if_due(&self) -> Result<bool> {
        let Some(_maintenance_lease) = crate::coordination::ResourceLease::try_acquire(
            self.path(),
            "session-search-fts-optimize",
        )?
        else {
            // FTS5 optimize is one SQLite command and its transaction cannot be
            // safely split here. Serialize it across Store instances so two
            // processes never hold the canonical writer for the same optimize.
            return Ok(false);
        };
        let now = unix_now();
        let due = self.conn.query_row(
            "SELECT pending_mutations, last_optimized_at
             FROM scoped_session_search_maintenance WHERE id = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, u64>(1)?)),
        )?;
        let interval_due = now.saturating_sub(due.1) >= SESSION_SEARCH_FTS_OPTIMIZE_INTERVAL;
        if due.0 < SESSION_SEARCH_FTS_OPTIMIZE_MUTATIONS
            && (!interval_due || due.0 < SESSION_SEARCH_FTS_OPTIMIZE_MIN_MUTATIONS)
        {
            return Ok(false);
        }
        let pending_work: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM scoped_session_search_work)",
            [],
            |row| row.get(0),
        )?;
        if pending_work {
            return Ok(false);
        }
        self.with_background_write_transaction("session_search.fts_optimize", |tx| {
            tx.execute(
                "INSERT INTO session_search_content_fts(session_search_content_fts)
                 VALUES ('optimize')",
                [],
            )?;
            tx.execute(
                "INSERT INTO scoped_session_search_metadata_fts(scoped_session_search_metadata_fts)
                 VALUES ('optimize')",
                [],
            )?;
            tx.execute(
                "UPDATE scoped_session_search_maintenance
                 SET pending_mutations = 0, last_optimized_at = ?1 WHERE id = 1",
                [now],
            )?;
            Ok(())
        })?;
        Ok(true)
    }

    fn refresh_session_search(
        &self,
        scope: &ScopeKey,
        session: &SessionRecord,
        payload: &str,
        should_stop: &dyn Fn() -> bool,
    ) -> Result<(Option<SessionSearchPublication>, bool)> {
        let state = search_file_state(&session.path)?;
        let metadata = session_search_metadata(session);
        let current = self.session_search_current_state(scope, session)?;
        let mut checkpoint = current
            .as_ref()
            .and_then(|row| row.5.as_deref())
            .and_then(|value| serde_json::from_str::<transcript::SearchCheckpoint>(value).ok());
        let parser_current = checkpoint.as_ref().is_some_and(|checkpoint| {
            crate::providers::agent_provider(session.agent).transcript_search_append_version()
                == Some(checkpoint.parser_version.as_str())
        });
        let source_current = state != (0, 0)
            && checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.matches_source(&session.path))
                .transpose()?
                .unwrap_or(false);
        let content_current = current.as_ref().is_some_and(|row| {
            (row.0, row.1) == state
                && row.3 == SESSION_SEARCH_INDEX_VERSION
                && row.4
                && ((parser_current && source_current) || state == (0, 0))
        });
        if content_current && current.as_ref().is_some_and(|row| row.2 == metadata) {
            return Ok((None, false));
        }

        let mut documents = vec![(0, session_search_metadata_document(session))];
        let mut start_record_order = 1;
        check_search_cancellation(should_stop)?;
        if !content_current && state != (0, 0) {
            let previous = current
                .as_ref()
                .filter(|row| row.3 == SESSION_SEARCH_INDEX_VERSION && row.4)
                .and(checkpoint.as_ref());
            let delta =
                transcript::read_search_delta(&session.path, session.agent, previous, should_stop)?
                    .ok_or(SearchRefreshCancelled)?;
            start_record_order = delta.start_record_order;
            for (index, item) in delta.items.into_iter().enumerate() {
                let mut document = SessionSearchDocument::default();
                match item.kind.as_str() {
                    "user" => document.user_text = item.body,
                    "assistant" => document.assistant_text = item.body,
                    _ => continue,
                }
                documents.push((start_record_order + index, document));
            }
            crate::logging::global().debug(
                "session search source read",
                serde_json::json!({
                    "sessionId": session.id, "bytesRead": delta.bytes_read,
                    "validationBytesRead": delta.validation_bytes_read,
                    "startRecordOrder": start_record_order,
                }),
            );
            checkpoint = Some(delta.checkpoint);
            if !delta.warnings.is_empty() {
                crate::logging::global().warn(
                    "session search parse warnings",
                    serde_json::json!({
                        "sessionId": session.id, "warningCount": delta.warnings.len(),
                    }),
                );
            }
        }
        if state == (0, 0) {
            checkpoint = None;
        }
        let checkpoint_json = checkpoint.as_ref().map(serde_json::to_string).transpose()?;

        check_search_cancellation(should_stop)?;

        // Compare before acquiring the database lock. Unchanged records retain
        // their row IDs and FTS entries, including when a transcript is appended.
        let existing = self.existing_session_search_documents(
            scope,
            session,
            content_current,
            start_record_order,
        )?;
        let updates = documents
            .iter()
            .filter(|(order, document)| existing.get(order) != Some(document))
            .map(|(order, document)| (*order, document))
            .collect::<Vec<_>>();
        let removed = if content_current {
            Vec::new()
        } else {
            existing
                .keys()
                .filter(|order| **order >= start_record_order + documents.len() - 1)
                .copied()
                .collect::<Vec<_>>()
        };
        let results_changed = !updates.is_empty()
            || !removed.is_empty()
            || current
                .as_ref()
                .is_none_or(|row| row.3 != SESSION_SEARCH_INDEX_VERSION || !row.4);
        check_search_cancellation(should_stop)?;
        if !results_changed {
            // A provider can append bookkeeping without changing any searchable
            // item. Acknowledge the new source version without hiding results or
            // advancing the user-visible sessions revision.
            let source_advanced = search_file_state(&session.path)? != state
                || checkpoint.as_ref().is_some_and(|checkpoint| {
                    !checkpoint.matches_source(&session.path).unwrap_or(false)
                });
            self.with_background_write_transaction("session_search.source_version", |tx| {
                ensure_session_unchanged(tx, scope, session, payload)?;
                tx.execute(
                    "UPDATE scoped_session_search_index SET file_mtime = ?5, file_size = ?6, indexed_at = ?7, search_metadata = ?8, search_checkpoint = ?9
                     WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
                    params![scope.as_str(), session.id, agent_label(session.agent),
                        session.path.to_string_lossy(), state.0, state.1, unix_now().to_string(), metadata, checkpoint_json],
                )?;
                Ok(())
            })?;
            return Ok((None, source_advanced));
        }
        self.with_background_write_transaction("session_search.begin", |tx| {
            ensure_session_unchanged(tx, scope, session, payload)?;
            // A failed or interrupted build remains unpublished and is repaired
            // by the next refresh, even if metadata was already committed.
            tx.execute(
                "INSERT INTO scoped_session_search_index
                 (scope_key, session_id, agent, session_path, file_mtime, file_size,
                  indexed_at, search_metadata, search_index_version)
                 VALUES (?1, ?2, ?3, ?4, 0, 0, '', '', 0)
                 ON CONFLICT(scope_key, session_id, agent, session_path)
                 DO UPDATE SET search_index_version = 0",
                params![
                    scope.as_str(),
                    session.id,
                    agent_label(session.agent),
                    session.path.to_string_lossy()
                ],
            )?;
            Ok(())
        })?;

        let mut start = 0;
        while start < updates.len() {
            check_search_cancellation(should_stop)?;
            let mut end = start;
            let mut bytes = 0;
            while end < updates.len() && end - start < SEARCH_WRITE_BATCH_ROWS {
                bytes += document_bytes(updates[end].1);
                end += 1;
                if bytes >= SEARCH_WRITE_BATCH_BYTES {
                    break;
                }
            }
            let processed =
                self.with_background_write_transaction("session_search.upsert_batch", |tx| {
                    ensure_session_unchanged(tx, scope, session, payload)?;
                    let started = std::time::Instant::now();
                    let mut processed = 0;
                    for (order, document) in &updates[start..end] {
                        upsert_shared_session_search_record(tx, scope, session, *order, document)?;
                        processed += 1;
                        if started.elapsed() >= SEARCH_WRITE_BATCH_TARGET {
                            break;
                        }
                    }
                    record_session_search_mutations_in_tx(tx, processed)?;
                    Ok(processed)
                })?;
            start += processed;
        }
        let mut removed_start = 0;
        while removed_start < removed.len() {
            check_search_cancellation(should_stop)?;
            let removed_end = (removed_start + SEARCH_WRITE_BATCH_ROWS).min(removed.len());
            let processed = self.with_background_write_transaction("session_search.remove_batch", |tx| {
                ensure_session_unchanged(tx, scope, session, payload)?;
                let started = std::time::Instant::now();
                let mut processed = 0;
                for order in &removed[removed_start..removed_end] {
                    tx.execute(
                        "DELETE FROM scoped_session_search_entries WHERE scope_key = ?1
                         AND session_id = ?2 AND agent = ?3 AND session_path = ?4 AND record_order = ?5",
                        params![scope.as_str(), session.id, agent_label(session.agent), session.path.to_string_lossy(), *order as i64],
                    )?;
                    processed += 1;
                    if started.elapsed() >= SEARCH_WRITE_BATCH_TARGET { break; }
                }
                record_session_search_mutations_in_tx(tx, processed)?;
                Ok(processed)
            })?;
            removed_start += processed;
        }
        check_search_cancellation(should_stop)?;
        let source_advanced = search_file_state(&session.path)? != state
            || checkpoint.as_ref().is_some_and(|checkpoint| {
                !checkpoint.matches_source(&session.path).unwrap_or(false)
            });
        // Publish the source version captured before parsing, not a later stat.
        // Appends during parsing remain stale and are picked up on the next pass.
        let publication =
            self.with_background_write_transaction("session_search.publish", |tx| {
                ensure_session_unchanged(tx, scope, session, payload)?;
                tx.execute(
                    "UPDATE scoped_session_search_index SET file_mtime = ?5, file_size = ?6,
                 search_metadata = ?7, search_index_version = ?8, indexed_at = ?9, search_checkpoint = ?10
                 WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
                    params![
                        scope.as_str(),
                        session.id,
                        agent_label(session.agent),
                        session.path.to_string_lossy(),
                        state.0,
                        state.1,
                        metadata,
                        SESSION_SEARCH_INDEX_VERSION,
                        unix_now().to_string(),
                        checkpoint_json
                    ],
                )?;
                // Search publication has its own revision stream. Seed it from
                // the canonical sessions head once so old workspaces preserve
                // monotonic ordering without allowing search work to advance
                // the sessions projection.
                Self::align_session_search_revision_in_tx(tx, scope)?;
                let head = advance_projection_head_in_tx(
                    tx,
                    scope,
                    SESSION_SEARCH_PROJECTION_DOMAIN,
                    None,
                    "ready",
                )?;
                Ok(SessionSearchPublication {
                    session: session.clone(),
                    base_revision: Revision::new(head.revision.value() - 1),
                    revision: head.revision,
                })
            })?;
        Ok((Some(publication), source_advanced))
    }

    fn align_session_search_revision_in_tx(tx: &Transaction<'_>, scope: &ScopeKey) -> Result<()> {
        let sessions_revision = tx
            .query_row(
                "SELECT revision FROM projection_heads
                 WHERE scope_key = ?1 AND domain = 'sessions'",
                [scope.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let Some(sessions_revision) = sessions_revision else {
            return Ok(());
        };
        let search_revision = tx
            .query_row(
                "SELECT revision FROM projection_heads
                 WHERE scope_key = ?1 AND domain = ?2",
                params![scope.as_str(), SESSION_SEARCH_PROJECTION_DOMAIN],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if search_revision.is_some_and(|revision| revision >= sessions_revision) {
            return Ok(());
        }
        let updated_at = i64::try_from(unix_now()).context("invalid projection timestamp")?;
        tx.execute(
            "INSERT INTO projection_heads
                (scope_key, domain, revision, source_version, schema_version, status, updated_at)
             VALUES (?1, ?2, ?3, NULL, 1, 'ready', ?4)
             ON CONFLICT(scope_key, domain) DO UPDATE SET
                revision = MAX(projection_heads.revision, excluded.revision),
                updated_at = excluded.updated_at",
            params![
                scope.as_str(),
                SESSION_SEARCH_PROJECTION_DOMAIN,
                sessions_revision,
                updated_at,
            ],
        )?;
        Ok(())
    }

    fn prune_session_search_key(
        &self,
        scope: &ScopeKey,
        session_id: &str,
        agent: &str,
        path: &str,
        should_stop: &dyn Fn() -> bool,
    ) -> Result<()> {
        loop {
            check_search_cancellation(should_stop)?;
            let ids = self.orphan_session_search_record_ids(scope, session_id, agent, path)?;
            if ids.is_empty() {
                break;
            }
            check_search_cancellation(should_stop)?;
            self.with_background_write_transaction("session_search.prune_batch", |tx| {
                let started = std::time::Instant::now();
                let mut processed = 0;
                for id in &ids {
                    tx.execute(
                        "DELETE FROM scoped_session_search_entries WHERE id = ?1
                        AND NOT EXISTS (SELECT 1 FROM scoped_sessions s
                            WHERE s.scope_key = scoped_session_search_entries.scope_key
                              AND s.id = scoped_session_search_entries.session_id
                              AND s.agent = scoped_session_search_entries.agent
                              AND s.path = scoped_session_search_entries.session_path)",
                        [id],
                    )?;
                    processed += 1;
                    if started.elapsed() >= SEARCH_WRITE_BATCH_TARGET {
                        break;
                    }
                }
                record_session_search_mutations_in_tx(tx, processed)?;
                Ok(())
            })?;
        }
        check_search_cancellation(should_stop)?;
        self.with_background_write_transaction("session_search.prune_index", |tx| {
            tx.execute(
                "DELETE FROM scoped_session_search_index WHERE scope_key = ?1
                AND session_id = ?2 AND agent = ?3 AND session_path = ?4
                AND NOT EXISTS (SELECT 1 FROM scoped_sessions s
                    WHERE s.scope_key = scoped_session_search_index.scope_key
                      AND s.id = scoped_session_search_index.session_id
                      AND s.agent = scoped_session_search_index.agent
                      AND s.path = scoped_session_search_index.session_path)",
                params![scope.as_str(), session_id, agent, path],
            )?;
            Ok(())
        })
    }
}

fn session_search_lock_key(scope: &ScopeKey) -> String {
    format!("session-search-{}", &sha256_text(scope.as_str())[..24])
}

fn upsert_shared_session_search_record(
    tx: &Transaction<'_>,
    scope: &ScopeKey,
    session: &SessionRecord,
    record_order: usize,
    document: &SessionSearchDocument,
) -> Result<()> {
    let content_hash = session_search_content_hash(&document.user_text, &document.assistant_text);
    tx.execute(
        "INSERT OR IGNORE INTO session_search_content_records(
             content_hash, user_text, assistant_text
         ) VALUES (?1, ?2, ?3)",
        params![content_hash, document.user_text, document.assistant_text],
    )?;
    let content_id = tx.query_row(
        "SELECT id FROM session_search_content_records WHERE content_hash = ?1",
        [&content_hash],
        |row| row.get::<_, i64>(0),
    )?;
    tx.execute(
        "INSERT INTO scoped_session_search_entries(
             scope_key, session_id, agent, session_path, record_order,
             metadata_text, title, project, content_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(scope_key, session_id, agent, session_path, record_order)
         DO UPDATE SET metadata_text = excluded.metadata_text,
                       title = excluded.title,
                       project = excluded.project,
                       content_id = excluded.content_id",
        params![
            scope.as_str(),
            session.id,
            agent_label(session.agent),
            session.path.to_string_lossy(),
            record_order as i64,
            document.metadata_text,
            document.title,
            document.project,
            content_id,
        ],
    )?;
    Ok(())
}

fn record_session_search_mutations_in_tx(tx: &Transaction<'_>, mutations: usize) -> Result<()> {
    if mutations == 0 {
        return Ok(());
    }
    tx.execute(
        "UPDATE scoped_session_search_maintenance
         SET pending_mutations = pending_mutations + ?1 WHERE id = 1",
        [mutations as i64],
    )?;
    Ok(())
}

/// Called in the same transaction that changes canonical metadata or membership.
pub(super) fn mark_session_search_pending_in_tx(
    tx: &Transaction<'_>,
    scope: &ScopeKey,
) -> Result<()> {
    tx.execute(
        "INSERT INTO scoped_session_search_work(scope_key, session_id, agent, session_path, generation, requested_at, last_error)
         SELECT ?1, id, agent, path, 1, ?2, NULL FROM (
           SELECT id, agent, path FROM scoped_sessions WHERE scope_key = ?1
           UNION SELECT session_id, agent, session_path FROM scoped_session_search_index WHERE scope_key = ?1
           UNION SELECT session_id, agent, session_path FROM scoped_session_search_entries WHERE scope_key = ?1
         ) WHERE true ON CONFLICT(scope_key, session_id, agent, session_path) DO UPDATE SET
         generation = generation + 1, requested_at = excluded.requested_at, last_error = NULL",
        params![scope.as_str(), unix_now().to_string()],
    )?;
    mark_search_scope_pending_in_tx(tx, scope)
}

pub(super) fn mark_session_key_pending_in_tx(
    tx: &Transaction<'_>,
    scope: &ScopeKey,
    session: &SessionRecord,
) -> Result<()> {
    tx.execute(
        "INSERT INTO scoped_session_search_work(scope_key, session_id, agent, session_path, generation, requested_at, last_error)
         VALUES (?1, ?2, ?3, ?4, 1, ?5, NULL)
         ON CONFLICT(scope_key, session_id, agent, session_path) DO UPDATE SET
         generation = generation + 1, requested_at = excluded.requested_at, last_error = NULL",
        params![scope.as_str(), session.id, agent_label(session.agent), session.path.to_string_lossy(), unix_now().to_string()],
    )?;
    mark_search_scope_pending_in_tx(tx, scope)
}

fn mark_search_scope_pending_in_tx(tx: &Transaction<'_>, scope: &ScopeKey) -> Result<()> {
    tx.execute(
        "INSERT INTO scoped_session_search_pending(scope_key, generation, requested_at, last_error)
         VALUES (?1, 1, ?2, NULL) ON CONFLICT(scope_key) DO UPDATE SET
         generation = generation + 1, requested_at = excluded.requested_at, last_error = NULL",
        params![scope.as_str(), unix_now().to_string()],
    )?;
    Ok(())
}

fn search_file_state(path: &Path) -> Result<(i64, i64)> {
    match fs::metadata(path) {
        Ok(_) => {
            let state = crate::session_skills::session_file_state(path)?;
            Ok((state.file_mtime, state.file_size))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((0, 0)),
        Err(error) => Err(error.into()),
    }
}

fn ensure_session_unchanged(
    tx: &Transaction<'_>,
    scope: &ScopeKey,
    session: &SessionRecord,
    payload: &str,
) -> Result<()> {
    let current: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM scoped_sessions WHERE scope_key = ?1 AND id = ?2
            AND agent = ?3 AND path = ?4 AND data_json = ?5)",
        params![
            scope.as_str(),
            session.id,
            agent_label(session.agent),
            session.path.to_string_lossy(),
            payload
        ],
        |row| row.get(0),
    )?;
    if !current {
        bail!("session changed while updating its search index");
    }
    Ok(())
}

fn document_bytes(document: &SessionSearchDocument) -> usize {
    document.metadata_text.len()
        + document.title.len()
        + document.project.len()
        + document.user_text.len()
        + document.assistant_text.len()
}

#[cfg(test)]
#[path = "session_search_storage_tests.rs"]
mod tests;
