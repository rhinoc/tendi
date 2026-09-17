//! Search preparation never runs inside the shared database write lock.
//! A scope lock serializes index builders; bounded transactions yield the database
//! to unrelated writers. An index is published only after every batch commits.

use super::*;

const SEARCH_WRITE_BATCH_BYTES: usize = 256 * 1024;
const SEARCH_WRITE_BATCH_ROWS: usize = 128;
// A cooperative SQL work budget, not a hard deadline: one statement and COMMIT
// finish atomically. Large FTS rows can exceed it and remain visible in metrics.
const SEARCH_WRITE_BATCH_TARGET: Duration = Duration::from_millis(10);

/// The exact metadata and revision boundary committed by one index publication.
/// Callers must not widen this span across unrelated metadata transactions.
#[derive(Debug, Clone)]
pub struct SessionSearchPublication {
    pub session: SessionRecord,
    pub base_revision: Revision,
    pub revision: Revision,
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

    fn refresh_session_search(
        &self,
        scope: &ScopeKey,
        session: &SessionRecord,
        payload: &str,
        should_stop: &dyn Fn() -> bool,
    ) -> Result<(Option<SessionSearchPublication>, bool)> {
        let key = params![
            scope.as_str(),
            session.id,
            agent_label(session.agent),
            session.path.to_string_lossy()
        ];
        let state = search_file_state(&session.path)?;
        let metadata = session_search_metadata(session);
        let current = self
            .conn
            .query_row(
                "SELECT file_mtime, file_size, search_metadata, search_index_version,
                    EXISTS (SELECT 1 FROM scoped_session_search_records r
                            WHERE r.scope_key = ?1 AND r.session_id = ?2 AND r.agent = ?3
                              AND r.session_path = ?4 AND r.record_order = 0), search_checkpoint
             FROM scoped_session_search_index
             WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4",
                key,
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
            .optional()?;
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
        let mut statement = self.conn.prepare(
            "SELECT record_order, metadata_text, title, project, user_text, assistant_text
             FROM scoped_session_search_records
             WHERE scope_key = ?1 AND session_id = ?2 AND agent = ?3 AND session_path = ?4
               AND (record_order = 0 OR (?5 = 0 AND record_order >= ?6)) ORDER BY record_order",
        )?;
        let existing = statement
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
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
        drop(statement);
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
            let processed = self.with_background_write_transaction("session_search.upsert_batch", |tx| {
                ensure_session_unchanged(tx, scope, session, payload)?;
                let started = std::time::Instant::now();
                let mut processed = 0;
                for (order, document) in &updates[start..end] {
                    tx.execute(
                        "INSERT INTO scoped_session_search_records
                         (scope_key, session_id, agent, session_path, record_order,
                          metadata_text, title, project, user_text, assistant_text)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                         ON CONFLICT(scope_key, session_id, agent, session_path, record_order)
                         DO UPDATE SET metadata_text = excluded.metadata_text, title = excluded.title,
                            project = excluded.project, user_text = excluded.user_text,
                            assistant_text = excluded.assistant_text",
                        params![scope.as_str(), session.id, agent_label(session.agent), session.path.to_string_lossy(),
                            *order as i64, document.metadata_text, document.title, document.project,
                            document.user_text, document.assistant_text],
                    )?;
                    processed += 1;
                    if started.elapsed() >= SEARCH_WRITE_BATCH_TARGET { break; }
                }
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
                        "DELETE FROM scoped_session_search_records WHERE scope_key = ?1
                         AND session_id = ?2 AND agent = ?3 AND session_path = ?4 AND record_order = ?5",
                        params![scope.as_str(), session.id, agent_label(session.agent), session.path.to_string_lossy(), *order as i64],
                    )?;
                    processed += 1;
                    if started.elapsed() >= SEARCH_WRITE_BATCH_TARGET { break; }
                }
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
                let head = advance_projection_head_in_tx(tx, scope, "sessions", None, "ready")?;
                Ok(SessionSearchPublication {
                    session: session.clone(),
                    base_revision: Revision::new(head.revision.value() - 1),
                    revision: head.revision,
                })
            })?;
        Ok((Some(publication), source_advanced))
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
            let ids = self
                .conn
                .prepare(
                    "SELECT r.id FROM scoped_session_search_records r WHERE r.scope_key = ?1
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
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if ids.is_empty() {
                break;
            }
            check_search_cancellation(should_stop)?;
            self.with_background_write_transaction("session_search.prune_batch", |tx| {
                let started = std::time::Instant::now();
                for id in &ids {
                    tx.execute(
                        "DELETE FROM scoped_session_search_records WHERE id = ?1
                        AND NOT EXISTS (SELECT 1 FROM scoped_sessions s
                            WHERE s.scope_key = scoped_session_search_records.scope_key
                              AND s.id = scoped_session_search_records.session_id
                              AND s.agent = scoped_session_search_records.agent
                              AND s.path = scoped_session_search_records.session_path)",
                        [id],
                    )?;
                    if started.elapsed() >= SEARCH_WRITE_BATCH_TARGET {
                        break;
                    }
                }
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
           UNION SELECT session_id, agent, session_path FROM scoped_session_search_records WHERE scope_key = ?1
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
mod tests {
    use super::*;
    use std::fs::OpenOptions;

    fn fixture(name: &str) -> (PathBuf, Store, ScopeKey, SessionRecord) {
        let root = std::env::temp_dir().join(format!(
            "tendi-search-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let store = Store::open(root.join("test.sqlite3")).unwrap();
        let session = serde_json::from_value(serde_json::json!({
            "id": "search-test", "agent": "codex", "path": root.join("session.jsonl"), "title": "Search test"
        })).unwrap();
        (
            root,
            store,
            ScopeKey::new("workspace:test").unwrap(),
            session,
        )
    }

    fn message(text: &str) -> String {
        format!(
            "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":{}}}]}}}}\n",
            serde_json::to_string(text).unwrap()
        )
    }

    #[test]
    fn per_session_work_survives_restart_without_visiting_clean_sessions() {
        use std::io::Write;
        let (root, store, scope, dirty) = fixture("dirty-keys");
        let mut clean = dirty.clone();
        clean.id = "clean-session".into();
        clean.path = root.join("clean.jsonl");
        fs::write(&dirty.path, message("original")).unwrap();
        fs::write(&clean.path, message("untouched")).unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[dirty.clone(), clean.clone()], &[])
            .unwrap();
        store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        // An unreadable clean input proves the worker does not enumerate/read
        // every canonical session when just one source has changed.
        fs::remove_file(&clean.path).unwrap();
        fs::create_dir(&clean.path).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&dirty.path)
            .unwrap()
            .write_all(message("appendneedle").as_bytes())
            .unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[dirty.clone()], &[])
            .unwrap();
        let keys: Vec<String> = store
            .conn
            .prepare("SELECT session_id FROM scoped_session_search_work")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(keys, [dirty.id]);
        drop(store);
        let store = Store::open(root.join("test.sqlite3")).unwrap();
        let (published, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert_eq!(published.len(), 1);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(!pending);
        assert_eq!(
            store
                .search_sessions_for_scope(&scope, "appendneedle", None)
                .unwrap()
                .len(),
            1
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn successful_old_generation_cannot_acknowledge_new_session_work() {
        use std::cell::Cell;
        let (root, store, scope, session) = fixture("generation");
        fs::write(&session.path, message("generationneedle")).unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
            .unwrap();
        let advanced = Cell::new(false);
        let (published, errors, pending) = store.refresh_pending_session_search_for_scope_until(&scope, || {
            let building: bool = store.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM scoped_session_search_index WHERE search_index_version = 0)", [], |row| row.get(0)
            ).unwrap();
            if building && !advanced.replace(true) {
                store.with_named_write_transaction("test.new_search_generation", |tx|
                    mark_session_key_pending_in_tx(tx, &scope, &session)).unwrap();
            }
            false
        }).unwrap();
        assert!(advanced.get());
        assert_eq!(published.len(), 1);
        assert!(errors.is_empty());
        assert!(pending);
        let (published, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert!(published.is_empty());
        assert!(errors.is_empty());
        assert!(!pending);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn durable_provisional_tail_is_replaced_not_duplicated() {
        use std::io::Write;
        let (root, store, scope, session) = fixture("durable-tail");
        let first = message("firsttailneedle");
        fs::write(&session.path, first.trim_end()).unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
            .unwrap();
        store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert_eq!(
            store
                .search_sessions_for_scope(&scope, "firsttailneedle", None)
                .unwrap()
                .len(),
            1
        );
        drop(store);
        fs::OpenOptions::new()
            .append(true)
            .open(&session.path)
            .unwrap()
            .write_all(("\n".to_owned() + &message("secondtailneedle") + "{\"type\":").as_bytes())
            .unwrap();
        let store = Store::open(root.join("test.sqlite3")).unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[session], &[])
            .unwrap();
        let (_, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert!(errors.is_empty());
        assert!(!pending);
        let count: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM scoped_session_search_records",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);
        let encoded: String = store
            .conn
            .query_row(
                "SELECT search_checkpoint FROM scoped_session_search_index",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let checkpoint: transcript::SearchCheckpoint = serde_json::from_str(&encoded).unwrap();
        assert_eq!(checkpoint.next_record_order, 3);
        assert_eq!(
            checkpoint.committed_offset,
            first.len() as u64 + message("secondtailneedle").len() as u64
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deletion_tombstone_is_scoped_to_the_complete_session_identity() {
        let (root, store, scope, session) = fixture("scoped-delete");
        let other = ScopeKey::new("workspace:other-delete").unwrap();
        fs::write(&session.path, message("retainedscope")).unwrap();
        for scope in [&scope, &other] {
            store
                .apply_session_changes_for_scope(scope, &[session.clone()], &[])
                .unwrap();
            store
                .refresh_pending_session_search_for_scope(scope)
                .unwrap();
        }
        store
            .apply_session_changes_for_scope(&scope, &[], &[session.path.clone()])
            .unwrap();
        drop(store);
        let store = Store::open(root.join("test.sqlite3")).unwrap();
        let (_, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert!(errors.is_empty());
        assert!(!pending);
        assert!(
            store
                .search_sessions_for_scope(&scope, "retainedscope", None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .search_sessions_for_scope(&other, "retainedscope", None)
                .unwrap()
                .len(),
            1
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_size_replacement_with_preserved_mtime_rebuilds_the_index() {
        let (root, store, scope, session) = fixture("identity-reset");
        fs::write(&session.path, message("oldneedle")).unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
            .unwrap();
        store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        let modified = fs::metadata(&session.path).unwrap().modified().unwrap();
        let replacement = root.join("replacement.jsonl");
        fs::write(&replacement, message("newneedle")).unwrap();
        fs::File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        fs::rename(&replacement, &session.path).unwrap();
        // The ordinary metadata/source owner must enqueue the replacement even
        // when its legacy mtime/size fingerprint and metadata are unchanged.
        store
            .apply_session_changes_for_scope(&scope, &[session], &[])
            .unwrap();
        let (published, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert_eq!(published.len(), 1);
        assert!(errors.is_empty());
        assert!(!pending);
        assert!(
            store
                .search_sessions_for_scope(&scope, "oldneedle", None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .search_sessions_for_scope(&scope, "newneedle", None)
                .unwrap()
                .len(),
            1
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn readded_identity_is_not_pruned_by_an_old_deletion_tombstone() {
        let (root, store, scope, session) = fixture("delete-readd");
        fs::write(&session.path, message("readdedneedle")).unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
            .unwrap();
        store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[], &[session.path.clone()])
            .unwrap();
        let readded = std::cell::Cell::new(false);
        let missing: bool = store
            .conn
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM scoped_sessions)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(missing);
        // The worker has observed the tombstone and selected its prune owner.
        // Re-add at that owner's cancellation boundary before deletion SQL.
        store
            .prune_session_search_key(
                &scope,
                &session.id,
                agent_label(session.agent),
                &session.path.to_string_lossy(),
                &|| {
                    if !readded.get() {
                        readded.set(true);
                        store
                            .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
                            .unwrap();
                    }
                    false
                },
            )
            .unwrap();
        assert!(readded.get());
        let (_, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert!(errors.is_empty());
        if pending {
            store
                .refresh_pending_session_search_for_scope(&scope)
                .unwrap();
        }
        assert_eq!(
            store
                .search_sessions_for_scope(&scope, "readdedneedle", None)
                .unwrap()
                .len(),
            1
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn metadata_commit_survives_index_failure_and_pending_work_recovers_after_restart() {
        let (root, store, scope, mut broken) = fixture("pending-recovery");
        let obstructing_file = root.join("not-a-directory");
        fs::write(&obstructing_file, "obstruction").unwrap();
        broken.path = obstructing_file.join("session.jsonl");
        let mut healthy = broken.clone();
        healthy.id = "healthy".into();
        healthy.path = root.join("healthy.jsonl");
        fs::write(&healthy.path, message("healthyneedle")).unwrap();

        let (committed, _) = store
            .apply_session_changes_for_scope(&scope, &[broken.clone(), healthy], &[])
            .unwrap();
        assert_eq!(committed.len(), 2);
        let committed_revision = store
            .projection_head(&scope, "sessions")
            .unwrap()
            .unwrap()
            .revision;
        assert_eq!(
            store.pending_session_search_scopes().unwrap(),
            vec![scope.clone()]
        );
        let (published, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].session.id, "healthy");
        assert_eq!(published[0].base_revision, committed_revision);
        assert_eq!(
            published[0].revision.value(),
            committed_revision.value() + 1
        );
        assert_eq!(errors.len(), 1);
        assert!(pending);
        let last_error: String = store
            .conn
            .query_row(
                "SELECT last_error FROM scoped_session_search_pending WHERE scope_key = ?1",
                [scope.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(last_error.contains(&broken.id));
        assert_eq!(
            store
                .list_sessions_for_scope(&scope)
                .unwrap()
                .sessions
                .len(),
            2
        );
        drop(store);

        fs::remove_file(&obstructing_file).unwrap();
        fs::create_dir(&obstructing_file).unwrap();
        fs::write(&broken.path, message("recoveredneedle")).unwrap();
        let store = Store::open(root.join("test.sqlite3")).unwrap();
        let (published, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].session.id, broken.id);
        assert!(errors.is_empty());
        assert!(!pending);
        assert!(store.pending_session_search_scopes().unwrap().is_empty());
        assert_eq!(
            store
                .search_sessions_for_scope(&scope, "recoveredneedle", None)
                .unwrap()
                .len(),
            1
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unchanged_metadata_and_source_do_not_schedule_another_index_pass() {
        let (root, store, scope, session) = fixture("unchanged");
        fs::write(&session.path, message("stablebody")).unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
            .unwrap();
        let (published, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert_eq!(published.len(), 1);
        assert!(errors.is_empty());
        assert!(!pending);
        let revision = store
            .projection_head(&scope, "sessions")
            .unwrap()
            .unwrap()
            .revision;
        store
            .apply_session_changes_for_scope(&scope, &[session], &[])
            .unwrap();
        assert!(store.pending_session_search_scopes().unwrap().is_empty());
        let (published, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert!(published.is_empty());
        assert!(errors.is_empty());
        assert!(!pending);
        assert_eq!(
            store
                .projection_head(&scope, "sessions")
                .unwrap()
                .unwrap()
                .revision,
            revision
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_bookkeeping_without_search_changes_does_not_advance_sessions_revision() {
        let (root, store, scope, session) = fixture("bookkeeping");
        let transcript = message("stablebody");
        fs::write(&session.path, &transcript).unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
            .unwrap();
        store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        let revision = store
            .projection_head(&scope, "sessions")
            .unwrap()
            .unwrap()
            .revision;
        fs::write(
            &session.path,
            transcript + "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\"}}\n",
        )
        .unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[session], &[])
            .unwrap();
        assert_eq!(
            store.pending_session_search_scopes().unwrap(),
            vec![scope.clone()]
        );
        let (published, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert!(published.is_empty());
        assert!(errors.is_empty());
        assert!(!pending);
        assert_eq!(
            store
                .projection_head(&scope, "sessions")
                .unwrap()
                .unwrap()
                .revision,
            revision
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rolled_back_metadata_does_not_leave_a_pending_request() {
        let (root, store, scope, _session) = fixture("pending-rollback");
        let result: Result<()> =
            store.with_named_write_transaction("test.aborted_metadata", |tx| {
                mark_session_search_pending_in_tx(tx, &scope)?;
                bail!("abort metadata transaction")
            });
        assert!(result.is_err());
        assert!(store.pending_session_search_scopes().unwrap().is_empty());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn busy_scope_is_deferred_without_blocking_other_scopes() {
        let (root, store, scope, session) = fixture("scope-busy");
        fs::write(&session.path, message("busyneedle")).unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
            .unwrap();
        let other_scope = ScopeKey::new("workspace:other").unwrap();
        store
            .apply_session_changes_for_scope(&other_scope, &[session], &[])
            .unwrap();
        let lease = store.lock_session_search(&scope).unwrap();
        let (published, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert!(published.is_empty());
        assert!(errors.is_empty());
        assert!(pending);
        let (published, errors, pending) = store
            .refresh_pending_session_search_for_scope(&other_scope)
            .unwrap();
        assert_eq!(published.len(), 1);
        assert!(errors.is_empty());
        assert!(!pending);
        drop(lease);
        let (published, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert_eq!(published.len(), 1);
        assert!(errors.is_empty());
        assert!(!pending);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellation_preserves_publications_and_pending_work_for_resume() {
        let (root, store, scope, first) = fixture("cancel-resume");
        let mut second = first.clone();
        second.id = "second-session".into();
        second.path = root.join("second.jsonl");
        fs::write(&first.path, message("firstneedle")).unwrap();
        fs::write(&second.path, message("secondneedle")).unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[first, second], &[])
            .unwrap();
        // Stop once a real publication has committed, not after a predetermined
        // number of callback invocations or a timing-dependent sleep.
        let (published, errors, pending) = store
            .refresh_pending_session_search_for_scope_until(&scope, || {
                store
                    .conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM scoped_session_search_index
                 WHERE scope_key = ?1 AND search_index_version = ?2)",
                        params![scope.as_str(), SESSION_SEARCH_INDEX_VERSION],
                        |row| row.get(0),
                    )
                    .unwrap()
            })
            .unwrap();
        assert_eq!(published.len(), 1);
        assert!(errors.is_empty());
        assert!(pending);
        let last_error: Option<String> = store
            .conn
            .query_row(
                "SELECT last_error FROM scoped_session_search_pending WHERE scope_key = ?1",
                [scope.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(last_error.is_none());
        let (remaining, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_ne!(remaining[0].session.id, published[0].session.id);
        assert!(errors.is_empty());
        assert!(!pending);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deletion_is_committed_before_independent_index_cleanup() {
        let (root, store, scope, session) = fixture("deletion");
        fs::write(&session.path, message("deletedneedle")).unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
            .unwrap();
        store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[], &[session.path.clone()])
            .unwrap();
        assert!(
            store
                .list_sessions_for_scope(&scope)
                .unwrap()
                .sessions
                .is_empty()
        );
        assert!(
            store
                .search_sessions_for_scope(&scope, "deletedneedle", None)
                .unwrap()
                .is_empty()
        );
        let (_, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert!(errors.is_empty());
        assert!(!pending);
        let remaining: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM scoped_session_search_records",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn append_retains_existing_fts_rows_even_when_session_metadata_is_unchanged() {
        let (root, store, scope, session) = fixture("append");
        fs::write(&session.path, message("originalneedle")).unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
            .unwrap();
        store
            .ensure_scoped_session_search_for_scope(&scope)
            .unwrap();
        let original: i64 = store
            .conn
            .query_row(
                "SELECT id FROM scoped_session_search_records WHERE record_order = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        fs::write(
            &session.path,
            message("originalneedle") + &message("appendedneedle"),
        )
        .unwrap();
        let changed = store
            .apply_session_changes_for_scope(&scope, &[session], &[])
            .unwrap();
        // Metadata commits independently; the background publication has its own delta.
        assert!(changed.0.is_empty());
        let (indexed, errors, pending) = store
            .refresh_pending_session_search_for_scope(&scope)
            .unwrap();
        assert_eq!(indexed.len(), 1);
        assert!(errors.is_empty());
        assert!(!pending);
        let retained: i64 = store
            .conn
            .query_row(
                "SELECT id FROM scoped_session_search_records WHERE record_order = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(original, retained);
        assert_eq!(
            store
                .search_sessions_for_scope(&scope, "appendedneedle", None)
                .unwrap()
                .len(),
            1
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rewrite_and_truncation_remove_obsolete_search_results() {
        let (root, store, scope, session) = fixture("rewrite");
        fs::write(
            &session.path,
            message("oldfirstneedle") + &message("oldsecondneedle"),
        )
        .unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[session.clone()], &[])
            .unwrap();
        store
            .ensure_scoped_session_search_for_scope(&scope)
            .unwrap();
        fs::write(&session.path, message("replacementneedle")).unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[session], &[])
            .unwrap();
        store
            .ensure_scoped_session_search_for_scope(&scope)
            .unwrap();
        assert!(
            store
                .search_sessions_for_scope(&scope, "oldsecondneedle", None)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .search_sessions_for_scope(&scope, "oldfirstneedle", None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .search_sessions_for_scope(&scope, "replacementneedle", None)
                .unwrap()
                .len(),
            1
        );
        let count: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM scoped_session_search_records",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transcript_search_finishes_at_the_initial_file_boundary() {
        use std::io::Write;

        let (root, store, _scope, session) = fixture("live-append");
        fs::write(&session.path, message("initialneedle")).unwrap();
        let mut items = Vec::new();
        transcript::for_each_search_item(&session.path, session.agent, |item| {
            items.push(item);
            // Simulate an active provider appending while the indexer is parsing.
            if items.len() == 1 {
                let mut file = OpenOptions::new().append(true).open(&session.path).unwrap();
                file.write_all(message("laterneedle").as_bytes()).unwrap();
            }
        })
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            transcript::parse_search_transcript(&session.path, session.agent)
                .unwrap()
                .items
                .len(),
            2
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transcript_read_failure_is_an_error_instead_of_an_endless_warning_loop() {
        let (root, store, _scope, session) = fixture("read-error");
        fs::create_dir(&session.path).unwrap();
        assert!(transcript::for_each_search_item(&session.path, session.agent, |_| {}).is_err());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_revision_and_rows_share_a_read_snapshot_while_writers_commit() {
        let (root, store, scope, session) = fixture("read-snapshot");
        store
            .apply_session_delta_for_scope(&scope, &[session.clone()])
            .unwrap();
        let before = store
            .projection_head(&scope, "sessions")
            .unwrap()
            .unwrap()
            .revision;
        let (revision, scan) = store
            .read_session_revisioned(&scope, || {
                let writer = Store::open(root.join("test.sqlite3"))?;
                let mut updated = session.clone();
                updated.title = Some("Changed while reading".into());
                writer.apply_session_delta_for_scope(&scope, &[updated])?;
                store.list_sessions_for_scope(&scope)
            })
            .unwrap();
        assert_eq!(revision, before);
        assert_eq!(scan.sessions[0].title, session.title);
        let (after, scan) = store.session_snapshot_for_scope(&scope).unwrap();
        assert!(after.value() > before.value());
        assert_eq!(
            scan.sessions[0].title.as_deref(),
            Some("Changed while reading")
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unpublished_index_is_hidden_and_repaired_after_reopening() {
        let (root, store, scope, session) = fixture("recovery");
        fs::write(&session.path, message("recoverneedle 中文")).unwrap();
        store
            .apply_session_changes_for_scope(&scope, &[session], &[])
            .unwrap();
        store
            .ensure_scoped_session_search_for_scope(&scope)
            .unwrap();
        store
            .with_named_write_transaction("test.interrupt_search", |tx| {
                tx.execute(
                    "UPDATE scoped_session_search_index SET search_index_version = 0",
                    [],
                )?;
                mark_session_search_pending_in_tx(tx, &scope)
            })
            .unwrap();
        drop(store);
        let store = Store::open(root.join("test.sqlite3")).unwrap();
        for query in ["recoverneedle", "中文"] {
            assert!(
                store
                    .search_sessions_for_scope(&scope, query, None)
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(
            store
                .ensure_scoped_session_search_for_scope(&scope)
                .unwrap()
        );
        for query in ["recoverneedle", "中文"] {
            assert_eq!(
                store
                    .search_sessions_for_scope(&scope, query, None)
                    .unwrap()
                    .len(),
                1
            );
        }
        assert!(
            !store
                .ensure_scoped_session_search_for_scope(&scope)
                .unwrap()
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_reads_ignore_obsolete_full_snapshots() {
        let (root, store, scope, session) = fixture("snapshot");
        store
            .write_normalized_snapshot(
                &scope,
                "sessions",
                &SessionScan {
                    sessions: vec![],
                    warnings: vec![],
                },
            )
            .unwrap();
        store
            .apply_session_delta_for_scope(&scope, &[session])
            .unwrap();
        assert_eq!(
            store
                .list_sessions_for_scope(&scope)
                .unwrap()
                .sessions
                .len(),
            1
        );
        let json = store
            .normalized_snapshot_json_for_scope(&scope, "sessions")
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<SessionScan>(&json)
                .unwrap()
                .sessions
                .len(),
            1
        );
        let stored: String = store
            .conn
            .query_row(
                "SELECT payload_json FROM normalized_snapshots WHERE domain='sessions'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            serde_json::from_str::<SessionScan>(&stored)
                .unwrap()
                .sessions
                .is_empty()
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
