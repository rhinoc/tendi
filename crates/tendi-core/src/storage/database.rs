//! One writable connection for each canonical database in this process.
use super::{
    database_writer::{WritePriority, WriterQueue},
    transaction,
};
use anyhow::{Context, Result};
use rusqlite::{Connection, Transaction};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, Weak},
    time::Duration,
};

type DatabaseSlot = Arc<Mutex<Weak<DatabaseWriter>>>;
static DATABASES: LazyLock<Mutex<HashMap<PathBuf, DatabaseSlot>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) struct DatabaseWriter {
    path: PathBuf,
    queue: WriterQueue,
    connection: Mutex<Connection>,
}

impl DatabaseWriter {
    fn open_connection(path: &Path) -> Result<Connection> {
        let connection = Connection::open(path)
            .with_context(|| format!("failed to open database writer {}", path.display()))?;
        connection.busy_timeout(Duration::from_secs(30))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        Ok(connection)
    }

    pub(super) fn open(path: &Path) -> Result<Arc<Self>> {
        let slot = {
            let mut databases = DATABASES
                .lock()
                .map_err(|_| anyhow::anyhow!("database registry poisoned"))?;
            databases.retain(|_, slot| {
                Arc::strong_count(slot) > 1
                    || slot
                        .try_lock()
                        .map_or(true, |database| database.strong_count() > 0)
            });
            databases.entry(path.to_path_buf()).or_default().clone()
        };
        // Only opens for this database wait for its initialization. Opening an
        // unrelated database never waits behind another database's SQLite lock.
        let mut current = slot
            .lock()
            .map_err(|_| anyhow::anyhow!("database initialization poisoned"))?;
        if let Some(database) = current.upgrade() {
            return Ok(database);
        }
        let connection = Self::open_connection(path)?;
        let database = Arc::new(Self {
            path: path.to_path_buf(),
            queue: WriterQueue::default(),
            connection: Mutex::new(connection),
        });
        *current = Arc::downgrade(&database);
        Ok(database)
    }

    pub(super) fn recover(&self) -> Result<()> {
        let queued = std::time::Instant::now();
        let _turn = self
            .queue
            .acquire_for(Duration::from_secs(2), WritePriority::Interactive)
            .map_err(|error| {
                error.context(format!(
                    "database connection recovery could not enter writer queue for {}",
                    self.path.display()
                ))
            })?;
        let connection = Self::open_connection(&self.path)?;
        let queue_wait = queued.elapsed();
        let mut current = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("database writer connection poisoned"))?;
        *current = connection;
        crate::logging::global().warn(
            "database connection recovered",
            serde_json::json!({
                "database": self.path,
                "queueWaitMs": queue_wait.as_secs_f64() * 1000.0,
            }),
        );
        Ok(())
    }

    pub(super) fn write<T>(
        &self,
        operation: &str,
        write: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.write_with_priority(operation, WritePriority::Interactive, write)
    }

    pub(super) fn write_background<T>(
        &self,
        operation: &str,
        write: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.write_with_priority(operation, WritePriority::Background, write)
    }

    fn write_with_priority<T>(
        &self,
        operation: &str,
        priority: WritePriority,
        write: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let queued = std::time::Instant::now();
        let _turn = self.queue.acquire_for(Duration::from_secs(30), priority).map_err(|error| {
            crate::logging::global().warn("database write rejected", serde_json::json!({"operation": operation, "database": self.path, "queueWaitMs": queued.elapsed().as_secs_f64() * 1000.0, "reason": error.to_string()}));
            error.context(format!("database operation {operation} could not enter writer queue"))
        })?;
        let queue_wait = queued.elapsed();
        let lease_started = std::time::Instant::now();
        let _database_lease = acquire_database_lease(&self.path, queued)?;
        let lease_wait = lease_started.elapsed();
        if lease_wait >= Duration::from_millis(1) {
            crate::logging::global().info(
                "database writer cross-process lease acquired",
                serde_json::json!({
                    "database": self.path,
                    "operation": operation,
                    "queueWaitMs": queue_wait.as_secs_f64() * 1000.0,
                    "leaseWaitMs": lease_wait.as_secs_f64() * 1000.0,
                }),
            );
        }
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("database writer connection poisoned"))?;
        let remaining = Duration::from_secs(30).saturating_sub(queued.elapsed());
        if remaining.is_zero() {
            return Err(super::database_writer::AdmissionError::Deadline.into());
        }
        transaction::write(
            &mut connection,
            &self.path,
            operation,
            priority,
            queue_wait,
            remaining,
            write,
        )
    }
}

fn acquire_database_lease(
    path: &Path,
    started: std::time::Instant,
) -> Result<crate::coordination::ResourceLease> {
    const LEASE_KEY: &str = "database-writer";
    loop {
        let remaining = Duration::from_secs(30).saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(super::database_writer::AdmissionError::Deadline.into());
        }
        if let Some(lease) = crate::coordination::ResourceLease::try_acquire(path, LEASE_KEY)? {
            return Ok(lease);
        }
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}

#[cfg(test)]
#[path = "database_tests.rs"]
mod tests;
