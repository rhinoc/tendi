//! operations persistence through the database-owned transaction boundary.
use super::super::*;

impl Store {
    pub fn record_operation(&self, operation: &OperationRecord) -> Result<()> {
        self.with_named_write_transaction("record_operation", |tx| {
            let now = i64::try_from(unix_now()).context("invalid operation timestamp")?;
            tx.execute(
                "INSERT INTO operation_journal
                (operation_id, kind, scope_key, status, input_revision, source_version,
                 checkpoint_json, error, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
             ON CONFLICT(operation_id) DO UPDATE SET
                kind = excluded.kind,
                scope_key = excluded.scope_key,
                status = excluded.status,
                input_revision = excluded.input_revision,
                source_version = excluded.source_version,
                checkpoint_json = excluded.checkpoint_json,
                error = excluded.error,
                updated_at = excluded.updated_at",
                params![
                    operation.operation_id.as_str(),
                    operation.kind.as_str(),
                    operation.scope_key.as_str(),
                    operation.status.as_str(),
                    operation.input_revision.value(),
                    operation.source_version.as_ref().map(SourceVersion::as_str),
                    operation.checkpoint_json.as_deref(),
                    operation.error.as_deref(),
                    now,
                ],
            )?;
            Ok(())
        })
    }

    pub fn update_operation(
        &self,
        operation_id: &OperationId,
        status: OperationStatus,
        checkpoint_json: Option<&str>,
        error: Option<&str>,
    ) -> Result<bool> {
        self.with_named_write_transaction("update_operation", |tx| {
            let updated = tx.execute(
                "UPDATE operation_journal
             SET status = ?2, checkpoint_json = ?3, error = ?4, updated_at = ?5
             WHERE operation_id = ?1",
                params![
                    operation_id.as_str(),
                    status.as_str(),
                    checkpoint_json,
                    error,
                    i64::try_from(unix_now()).context("invalid operation timestamp")?,
                ],
            )?;
            Ok(updated > 0)
        })
    }

    pub fn recover_inflight_operations(&self) -> Result<usize> {
        self.with_named_write_transaction("recover_inflight_operations", |tx| {
            let updated = tx.execute(
                "UPDATE operation_journal
             SET status = 'failed',
                 error = COALESCE(error, ?1),
                 updated_at = ?2
             WHERE status IN ('queued', 'running', 'committing')",
                params![
                    "daemon restarted before the operation completed",
                    i64::try_from(unix_now()).context("invalid operation timestamp")?,
                ],
            )?;
            Ok(updated)
        })
    }

    pub fn operation(&self, operation_id: &OperationId) -> Result<Option<OperationRecord>> {
        let row = self
            .conn
            .query_row(
                "SELECT operation_id, kind, scope_key, status, input_revision,
                        source_version, checkpoint_json, error
                 FROM operation_journal
                 WHERE operation_id = ?1",
                params![operation_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .optional()?;
        row.map(
            |(
                operation_id,
                kind,
                scope_key,
                status,
                input_revision,
                source_version,
                checkpoint_json,
                error,
            )| {
                let kind = match kind.as_str() {
                    "scan" => crate::runtime_contract::OperationKind::Scan,
                    "watch" => crate::runtime_contract::OperationKind::Watch,
                    "backfill" => crate::runtime_contract::OperationKind::Backfill,
                    "analytics" => crate::runtime_contract::OperationKind::Analytics,
                    "skill_update" => crate::runtime_contract::OperationKind::SkillUpdate,
                    "projection" => crate::runtime_contract::OperationKind::Projection,
                    other => anyhow::bail!("unknown operation kind: {other}"),
                };
                let status = match status.as_str() {
                    "queued" => OperationStatus::Queued,
                    "running" => OperationStatus::Running,
                    "committing" => OperationStatus::Committing,
                    "committed" => OperationStatus::Committed,
                    "published" => OperationStatus::Published,
                    "failed" => OperationStatus::Failed,
                    "cancelled" => OperationStatus::Cancelled,
                    "timed_out" => OperationStatus::TimedOut,
                    "stale" => OperationStatus::Stale,
                    other => anyhow::bail!("unknown operation status: {other}"),
                };
                Ok(OperationRecord {
                    operation_id: OperationId::new(operation_id)
                        .map_err(|error| anyhow::anyhow!(error))?,
                    kind,
                    scope_key: ScopeKey::new(scope_key).map_err(|error| anyhow::anyhow!(error))?,
                    status,
                    input_revision: Revision::new(
                        u64::try_from(input_revision).context("invalid operation revision")?,
                    ),
                    source_version: source_version
                        .map(SourceVersion::new)
                        .transpose()
                        .map_err(|error| anyhow::anyhow!(error))?,
                    checkpoint_json,
                    error,
                })
            },
        )
        .transpose()
    }
}
