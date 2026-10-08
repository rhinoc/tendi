//! One writable connection for each canonical database in this process.
use super::{
    database_writer::{WritePriority, WriterQueue},
    transaction,
};
use anyhow::{Context, Result};
use rusqlite::{Connection, MAIN_DB, Transaction};
use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

type DatabaseSlot = Arc<Mutex<Weak<DatabaseWriter>>>;
static DATABASES: LazyLock<Mutex<HashMap<PathBuf, DatabaseSlot>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) struct DatabaseWriter {
    path: PathBuf,
    queue: WriterQueue,
    connection: Mutex<Connection>,
    // Rust drops fields in declaration order: close SQLite before releasing this lock.
    _sqlite_compatibility: File,
}

impl DatabaseWriter {
    fn open_connection_unleased(path: &Path) -> Result<Connection> {
        let connection = Connection::open(path)
            .with_context(|| format!("failed to open database writer {}", path.display()))?;
        super::shared_cache::register_functions(&connection)?;
        connection.busy_timeout(Duration::from_secs(30))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        Ok(connection)
    }

    fn probe_connection(connection: &Connection, require_application_schema: bool) -> Result<()> {
        let quick_check: String = connection
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
            .context("database health probe quick_check failed")?;
        anyhow::ensure!(
            quick_check.eq_ignore_ascii_case("ok"),
            "database health probe quick_check returned {quick_check}"
        );

        let _: i64 = connection
            .query_row("PRAGMA schema_version", [], |row| row.get(0))
            .context("database health probe could not read schema_version")?;
        let _: i64 = connection
            .query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get(0))
            .context("database health probe could not read sqlite_master")?;
        if require_application_schema {
            let meta_exists: bool = connection.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = 'meta'
                )",
                [],
                |row| row.get(0),
            )?;
            anyhow::ensure!(
                meta_exists,
                "database health probe found no application schema"
            );
        }
        Ok(())
    }

    fn open_and_probe(path: &Path, require_application_schema: bool) -> Result<()> {
        let _sqlite_compatibility = super::database_compatibility::acquire(path)?;
        let started = std::time::Instant::now();
        let _database_lease = acquire_database_lease(path, started)?;
        let connection = Self::open_connection_unleased(path)?;
        Self::probe_or_repair_connection(&connection, path, require_application_schema)?;
        drop(connection);
        Ok(())
    }

    fn open_with_lease(path: &Path) -> Result<Connection> {
        let started = std::time::Instant::now();
        let _database_lease = acquire_database_lease(path, started)?;
        Self::open_connection_unleased(path)
    }

    fn probe_or_repair_connection(
        connection: &Connection,
        path: &Path,
        require_application_schema: bool,
    ) -> Result<()> {
        let mut repair_count = 0;
        loop {
            match Self::probe_connection(connection, require_application_schema) {
                Ok(()) => return Ok(()),
                Err(error) if repair_count < 2 => {
                    let repaired = Self::repairs_fs_manifest(connection, path, &error)?
                        || Self::repairs_session_skill_links(connection, path, &error)?;
                    if repaired {
                        repair_count += 1;
                        continue;
                    }
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn repairs_fs_manifest(
        connection: &Connection,
        path: &Path,
        error: &anyhow::Error,
    ) -> Result<bool> {
        let message = format!("{error:#}");
        if !message.contains("database health probe quick_check returned") {
            return Ok(false);
        }
        let mut statement = connection.prepare(
            "SELECT rootpage FROM sqlite_master
             WHERE tbl_name = 'fs_manifest' AND rootpage > 0",
        )?;
        let root_pages = statement
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let tree_corruption = root_pages.iter().any(|root_page| {
            message
                .lines()
                .any(|line| line.contains(&format!("Tree {root_page} page ")))
        });
        if !tree_corruption && !message.contains("fs_manifest") {
            return Ok(false);
        }

        let backup_path = Self::backup_corrupt_database(connection, path)?;
        crate::migrations::rebuild_corrupt_fs_manifest(connection).with_context(|| {
            format!(
                "rebuild corrupt filesystem manifest; backup={}",
                backup_path.display()
            )
        })?;
        crate::logging::global().warn(
            "rebuilt corrupt filesystem manifest",
            serde_json::json!({
                "database": path,
                "backup": backup_path,
            }),
        );
        Ok(true)
    }

    fn repairs_session_skill_links(
        connection: &Connection,
        path: &Path,
        error: &anyhow::Error,
    ) -> Result<bool> {
        let message = format!("{error:#}");
        if !message.contains("database health probe quick_check returned") {
            return Ok(false);
        }
        let mut statement = connection.prepare(
            "SELECT rootpage FROM sqlite_master
             WHERE name IN (
                 'scoped_session_skill_links',
                 'scoped_session_skill_links_storage',
                 'sqlite_autoindex_scoped_session_skill_links_storage_1',
                 'sqlite_autoindex_scoped_session_skill_links_1',
                 'idx_scoped_session_skill_links_session',
                 'idx_scoped_session_skill_links_skill'
             )",
        )?;
        let root_pages = statement
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let corrupted = root_pages
            .iter()
            .any(|root_page| message.contains(&format!("Tree {root_page} page {root_page}")));
        if !corrupted {
            return Ok(false);
        }

        let backup_path = Self::backup_corrupt_database(connection, path)?;
        crate::migrations::rebuild_corrupt_session_skill_links(connection).with_context(|| {
            format!(
                "rebuild corrupt session skill links; backup={}",
                backup_path.display()
            )
        })?;
        crate::logging::global().warn(
            "rebuilt corrupt session skill links",
            serde_json::json!({
                "database": path,
                "backup": backup_path,
            }),
        );
        Ok(true)
    }

    fn backup_corrupt_database(connection: &Connection, path: &Path) -> Result<PathBuf> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let file_name = path
            .file_name()
            .context("database path has no file name")?
            .to_string_lossy();
        let backup_path =
            path.with_file_name(format!("{file_name}.corrupt-backup-{timestamp}.sqlite3"));
        connection
            .backup(MAIN_DB, &backup_path, None)
            .with_context(|| format!("backup corrupt database to {}", backup_path.display()))?;
        Ok(backup_path)
    }

    fn registry_slot(path: &Path) -> Result<DatabaseSlot> {
        let mut databases = DATABASES
            .lock()
            .map_err(|_| anyhow::anyhow!("database registry poisoned"))?;
        databases.retain(|_, slot| {
            Arc::strong_count(slot) > 1
                || slot
                    .try_lock()
                    .map_or(true, |database| database.strong_count() > 0)
        });
        Ok(databases.entry(path.to_path_buf()).or_default().clone())
    }

    pub(super) fn open(path: &Path) -> Result<Arc<Self>> {
        let slot = Self::registry_slot(path)?;
        // Only opens for this database wait for its initialization. Opening an
        // unrelated database never waits behind another database's SQLite lock.
        let mut current = slot
            .lock()
            .map_err(|_| anyhow::anyhow!("database initialization poisoned"))?;
        if let Some(database) = current.upgrade() {
            return Ok(database);
        }
        let sqlite_compatibility = super::database_compatibility::acquire(path)?;
        let connection = Self::open_with_lease(path)?;
        let journal_mode =
            connection.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0));
        let wal_autocheckpoint =
            connection.query_row("PRAGMA wal_autocheckpoint", [], |row| row.get::<_, i64>(0));
        let database = Arc::new(Self {
            path: path.to_path_buf(),
            queue: WriterQueue::default(),
            connection: Mutex::new(connection),
            _sqlite_compatibility: sqlite_compatibility,
        });
        crate::logging::global().debug(
            "database writer opened",
            serde_json::json!({
                "database": path,
                "files": super::database_file_diagnostics(path),
                "journalMode": journal_mode.as_ref().ok(),
                "journalModeError": journal_mode.as_ref().err().map(ToString::to_string),
                "walAutocheckpointPages": wal_autocheckpoint.as_ref().ok(),
                "walAutocheckpointError": wal_autocheckpoint.as_ref().err().map(ToString::to_string),
            }),
        );
        *current = Arc::downgrade(&database);
        Ok(database)
    }

    pub(super) fn recover_path(path: &Path) -> Result<()> {
        anyhow::ensure!(
            path.exists(),
            "database recovery refused missing database {}",
            path.display()
        );
        let slot = Self::registry_slot(path)?;
        let database = slot
            .lock()
            .map_err(|_| anyhow::anyhow!("database initialization poisoned"))?
            .upgrade();
        if let Some(database) = database {
            return database.recover();
        }

        // There is no live writer in this process to replace. Opening and
        // probing under the same cross-process lease still proves that a
        // subsequent Store::open may safely create its writer; it never
        // creates or substitutes an empty database.
        Self::open_and_probe(path, true)?;
        Ok(())
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
        let _database_lease = acquire_database_lease(&self.path, queued)?;
        let connection = Self::open_connection_unleased(&self.path)?;
        // DatabaseWriter is also used by low-level storage tests and by the
        // schema bootstrap path before the application tables exist. The
        // recovery entry point validates the application schema when it has
        // no live writer; replacing an existing writer only needs the SQLite
        // health probe here.
        Self::probe_or_repair_connection(&connection, &self.path, false)?;
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

    pub(super) fn vacuum(&self) -> Result<()> {
        let queued = std::time::Instant::now();
        let _turn = self
            .queue
            .acquire_for(Duration::from_secs(30), WritePriority::Background)?;
        let _database_lease = acquire_database_lease(&self.path, queued)?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("database writer connection poisoned"))?;
        connection.busy_timeout(Duration::from_secs(30))?;
        let started = std::time::Instant::now();
        connection.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE)")?;
        crate::logging::global().info(
            "database vacuum completed",
            serde_json::json!({
                "database": self.path,
                "durationMs": started.elapsed().as_secs_f64() * 1000.0,
            }),
        );
        Ok(())
    }

    /// Run compaction only when this process is currently idle and the
    /// cross-process writer lease is immediately available. A caller that gets
    /// `false` must leave its durable maintenance marker in place and retry
    /// from a later maintenance opportunity.
    pub(super) fn vacuum_if_idle(&self) -> Result<bool> {
        let Some(_turn) = self.queue.try_acquire(WritePriority::Background)? else {
            return Ok(false);
        };
        let Some(_database_lease) =
            crate::coordination::ResourceLease::try_acquire(&self.path, "database-writer")?
        else {
            return Ok(false);
        };
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("database writer connection poisoned"))?;
        connection.busy_timeout(Duration::from_millis(1))?;
        let started = std::time::Instant::now();
        match connection.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE)") {
            Ok(()) => {
                crate::logging::global().info(
                    "database idle vacuum completed",
                    serde_json::json!({
                        "database": self.path,
                        "durationMs": started.elapsed().as_secs_f64() * 1000.0,
                    }),
                );
                Ok(true)
            }
            Err(error)
                if matches!(error, rusqlite::Error::SqliteFailure(ref code, _)
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                )) =>
            {
                Ok(false)
            }
            Err(error) => Err(error.into()),
        }
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

    pub(super) fn write_background_until<T>(
        &self,
        operation: &str,
        deadline: std::time::Instant,
        write: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<Option<T>> {
        let queued = std::time::Instant::now();
        let Some(_turn) = self.queue.try_acquire(WritePriority::Background)? else {
            return Ok(None);
        };
        let Some(_database_lease) =
            crate::coordination::ResourceLease::try_acquire(&self.path, "database-writer")?
        else {
            return Ok(None);
        };
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("database writer connection poisoned"))?;
        let result = transaction::write(
            &mut connection,
            &self.path,
            operation,
            WritePriority::Background,
            queued.elapsed(),
            remaining,
            write,
        )?;
        Ok(Some(result))
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
