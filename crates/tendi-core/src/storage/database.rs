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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn recovery_reopens_writer_connection_without_replaying_work() {
        let path = std::env::temp_dir().join(format!(
            "tendi-database-recovery-{}-{}.sqlite3",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let writer = DatabaseWriter::open(&path).unwrap();
        writer
            .write("recovery.setup", |tx| {
                tx.execute_batch("CREATE TABLE recovery_probe (value TEXT NOT NULL)")?;
                Ok(())
            })
            .unwrap();

        writer.recover().unwrap();
        writer
            .write("recovery.write", |tx| {
                tx.execute("INSERT INTO recovery_probe (value) VALUES ('ok')", [])?;
                Ok(())
            })
            .unwrap();
        drop(writer);

        let connection = Connection::open(&path).unwrap();
        let value: String = connection
            .query_row("SELECT value FROM recovery_probe", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "ok");

        for suffix in ["", "-wal", "-shm"] {
            let mut target = path.as_os_str().to_os_string();
            target.push(suffix);
            let _ = fs::remove_file(target);
        }
    }
}
