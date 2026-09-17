//! Transaction ownership and diagnostics, independent of domain SQL.
use anyhow::Result;
use rusqlite::{Connection, Transaction, TransactionBehavior};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(super) fn write<T>(
    connection: &mut Connection,
    database: &Path,
    operation: &str,
    priority: super::database_writer::WritePriority,
    queue_wait: Duration,
    remaining: Duration,
    write: impl FnOnce(&Transaction<'_>) -> Result<T>,
) -> Result<T> {
    connection.busy_timeout(remaining)?;
    let begin = Instant::now();
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate);
    let sqlite_wait = begin.elapsed();
    let executing = Instant::now();
    let result = transaction
        .map_err(anyhow::Error::from)
        .and_then(|transaction| {
            if begin.elapsed() >= remaining {
                anyhow::bail!("database write admission expired before callback execution");
            }
            // Execute once. Only SQLite acquisition waits, never replay business work.
            let value = write(&transaction)?;
            transaction.commit()?;
            Ok(value)
        });
    let transaction_time = executing.elapsed();
    let logger = crate::logging::global();
    let slow = queue_wait + sqlite_wait + transaction_time >= Duration::from_millis(500);
    if slow || result.is_err() || logger.debug_enabled() {
        let fields = serde_json::json!({"operation": operation, "priority": format!("{priority:?}"), "database": database, "queueWaitMs": queue_wait.as_secs_f64() * 1000.0, "sqliteWaitMs": sqlite_wait.as_secs_f64() * 1000.0, "transactionMs": transaction_time.as_secs_f64() * 1000.0, "succeeded": result.is_ok(), "error": result.as_ref().err().map(ToString::to_string)});
        if slow || result.is_err() {
            logger.warn("database write completed", fields);
        } else {
            logger.debug("database write completed", fields);
        }
    }
    result
}

#[cfg(test)]
#[path = "transaction_tests.rs"]
mod tests;
