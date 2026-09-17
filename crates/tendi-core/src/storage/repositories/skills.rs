//! skills persistence through the database-owned transaction boundary.
use super::super::*;
use crate::skills::SkillVisibility;

fn parse_skill_visibility(value: &str) -> Option<SkillVisibility> {
    match value {
        "auto" => Some(SkillVisibility::Auto),
        "manual" => Some(SkillVisibility::Manual),
        "off" => Some(SkillVisibility::Off),
        _ => None,
    }
}

fn canonical_skill_visibility_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

const INSTALLATION_SCOPE_KEY: &str = "installation:default";

impl Store {
    pub(crate) fn ensure_skill_visibility_table(&self) -> Result<()> {
        self.with_named_write_transaction("ensure_skill_visibility_table", |tx| {
            tx.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS scoped_skill_visibility (
                    scope_key TEXT NOT NULL,
                    skill_path TEXT NOT NULL,
                    visibility TEXT NOT NULL,
                    PRIMARY KEY (scope_key, skill_path)
                );
                CREATE INDEX IF NOT EXISTS idx_scoped_skill_visibility_path
                    ON scoped_skill_visibility(skill_path);
                ",
            )?;
            Ok(())
        })
    }

    pub fn skill_visibilities_for_workspace(
        &self,
        workspace_root: &Path,
    ) -> Result<BTreeMap<PathBuf, SkillVisibility>> {
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let mut statement = self.conn.prepare(
            "SELECT skill_path, visibility
             FROM scoped_skill_visibility
             WHERE scope_key = ?1
             ORDER BY skill_path",
        )?;
        let rows = statement.query_map(params![scope_key.as_str()], |row| {
            let path = PathBuf::from(row.get::<_, String>(0)?);
            let raw_visibility = row.get::<_, String>(1)?;
            let visibility =
                parse_skill_visibility(&raw_visibility).ok_or(rusqlite::Error::InvalidQuery)?;
            Ok((path, visibility))
        })?;
        rows.collect::<std::result::Result<BTreeMap<_, _>, _>>()
            .map_err(Into::into)
    }

    pub fn upsert_skill_visibilities_for_workspace(
        &self,
        workspace_root: &Path,
        values: &[(PathBuf, SkillVisibility)],
    ) -> Result<usize> {
        self.write_skill_visibilities_for_workspace(workspace_root, values, false)
    }

    /// Seed discovered defaults without replacing a concurrent explicit choice.
    pub fn initialize_skill_visibilities_for_workspace(
        &self,
        workspace_root: &Path,
        values: &[(PathBuf, SkillVisibility)],
    ) -> Result<usize> {
        self.write_skill_visibilities_for_workspace(workspace_root, values, true)
    }

    fn write_skill_visibilities_for_workspace(
        &self,
        workspace_root: &Path,
        values: &[(PathBuf, SkillVisibility)],
        initialize_only: bool,
    ) -> Result<usize> {
        if values.is_empty() {
            return Ok(0);
        }
        if values
            .iter()
            .any(|(_, value)| *value == SkillVisibility::Mixed)
        {
            anyhow::bail!("mixed visibility cannot be persisted");
        }
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let values = values
            .iter()
            .map(|(path, visibility)| (canonical_skill_visibility_path(path), *visibility))
            .collect::<Vec<_>>();
        let resources = values
            .iter()
            .map(|(path, _)| crate::coordination::canonical_resource_path(path))
            .collect::<Result<Vec<_>>>()?;
        self.with_named_write_transaction("write_skill_visibilities_for_workspace", |tx| {
            let mut changed = 0;
            for (path, visibility) in &values {
                changed += tx.execute(
                    "INSERT INTO scoped_skill_visibility (scope_key, skill_path, visibility)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(scope_key, skill_path) DO UPDATE SET
                        visibility = excluded.visibility
                     WHERE NOT ?4 AND scoped_skill_visibility.visibility != excluded.visibility",
                    params![
                        scope_key.as_str(),
                        path.display().to_string(),
                        visibility.label(),
                        initialize_only
                    ],
                )?;
            }
            if changed > 0 {
                self.mark_projection_resources_in_tx(
                    tx, &scope_key, "skills", &resources, false, true,
                )?;
            }
            Ok(changed)
        })
    }

    pub fn delete_skill_visibilities_for_workspace(
        &self,
        workspace_root: &Path,
        skill_paths: &[PathBuf],
    ) -> Result<usize> {
        if skill_paths.is_empty() {
            return Ok(0);
        }
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let skill_paths = skill_paths
            .iter()
            .map(|path| canonical_skill_visibility_path(path))
            .collect::<Vec<_>>();
        let resources = skill_paths
            .iter()
            .map(|path| crate::coordination::canonical_resource_path(path))
            .collect::<Result<Vec<_>>>()?;
        self.with_named_write_transaction("delete_skill_visibilities_for_workspace", |tx| {
            let mut deleted = 0;
            for path in &skill_paths {
                deleted += tx.execute(
                    "DELETE FROM scoped_skill_visibility
                     WHERE scope_key = ?1 AND skill_path = ?2",
                    params![scope_key.as_str(), path.display().to_string()],
                )?;
            }
            if deleted > 0 {
                self.mark_projection_resources_in_tx(
                    tx, &scope_key, "skills", &resources, false, true,
                )?;
            }
            Ok(deleted)
        })
    }

    pub fn copy_skill_visibility_for_workspace(
        &self,
        workspace_root: &Path,
        source: &Path,
        destination: &Path,
        remove_source: bool,
    ) -> Result<bool> {
        let workspace_root = canonical_workspace_root(workspace_root);
        let scope_key = workspace_scope_key(&workspace_root)?;
        let source = canonical_skill_visibility_path(source);
        let destination = canonical_skill_visibility_path(destination);
        let resources = [&source, &destination]
            .into_iter()
            .map(|path| crate::coordination::canonical_resource_path(path))
            .collect::<Result<Vec<_>>>()?;
        self.with_named_write_transaction("copy_skill_visibility_for_workspace", |tx| {
            let Some(visibility) = tx.query_row("SELECT visibility FROM scoped_skill_visibility WHERE scope_key = ?1 AND skill_path = ?2", params![scope_key.as_str(), source.display().to_string()], |row| row.get::<_, String>(0)).optional()? else { return Ok(false); };
            tx.execute(
                "INSERT INTO scoped_skill_visibility (scope_key, skill_path, visibility)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(scope_key, skill_path) DO UPDATE SET
                    visibility = excluded.visibility",
                params![scope_key.as_str(), destination.display().to_string(), visibility],
            )?;
            if remove_source && source != destination {
                tx.execute(
                    "DELETE FROM scoped_skill_visibility
                     WHERE scope_key = ?1 AND skill_path = ?2",
                    params![scope_key.as_str(), source.display().to_string()],
                )?;
            }
            self.mark_projection_resources_in_tx(tx, &scope_key, "skills", &resources, false, true)?;
            Ok(true)
        })
    }

    pub fn delete_skill_snapshots(&self, skill_paths: &[PathBuf]) -> Result<()> {
        if skill_paths.is_empty() {
            return Ok(());
        }
        self.with_named_write_transaction("delete_skill_snapshots", |tx| {
            for skill_path in skill_paths {
                tx.execute(
                    "DELETE FROM scoped_skill_snapshots
                     WHERE scope_key = ?1 AND skill_path = ?2",
                    params![INSTALLATION_SCOPE_KEY, skill_path.display().to_string()],
                )?;
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn persist_skill_update_persistence_for_workspace(
        &self,
        workspace_root: &Path,
        source_records: &[SkillSourceRecord],
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        self.persist_skill_update_persistence_for_workspace_with_deleted(
            workspace_root,
            &[],
            source_records,
            snapshots,
        )
    }

    pub fn persist_skill_update_persistence_for_workspace_checked(
        &self,
        workspace_root: &Path,
        expected_source_versions: &[(PathBuf, Option<String>)],
        source_records: &[SkillSourceRecord],
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        self.persist_skill_update_persistence_for_workspace_with_deleted_checked(
            workspace_root,
            &[],
            expected_source_versions,
            source_records,
            snapshots,
        )
    }

    pub fn persist_skill_update_persistence_for_workspace_with_deleted(
        &self,
        workspace_root: &Path,
        deleted_paths: &[PathBuf],
        source_records: &[SkillSourceRecord],
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        self.persist_skill_update_persistence_for_workspace_with_deleted_checked(
            workspace_root,
            deleted_paths,
            &[],
            source_records,
            snapshots,
        )
    }

    pub fn persist_skill_update_persistence_for_workspace_with_deleted_checked(
        &self,
        workspace_root: &Path,
        deleted_paths: &[PathBuf],
        expected_source_versions: &[(PathBuf, Option<String>)],
        source_records: &[SkillSourceRecord],
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        if deleted_paths.is_empty() && source_records.is_empty() && snapshots.is_empty() {
            return Ok(());
        }
        let source_versions = source_records
            .iter()
            .map(|record| (&record.skill_path, record.source_version.as_deref()))
            .collect::<BTreeMap<_, _>>();
        for snapshot in snapshots {
            let Some(source_version) = source_versions.get(&snapshot.skill_path) else {
                bail!(
                    "skill snapshot {} has no matching source record",
                    snapshot.skill_path.display()
                );
            };
            if *source_version != Some(snapshot.source_version.as_str()) {
                crate::logging::global().warn(
                    "skill snapshot source version mismatch",
                    serde_json::json!({
                        "operation": "persist_skill_update_persistence_for_workspace_checked",
                        "skillPath": snapshot.skill_path,
                        "sourceVersion": source_version,
                        "snapshotVersion": snapshot.source_version,
                    }),
                );
                bail!(
                    "skill snapshot {} has source version {}, expected {:?}",
                    snapshot.skill_path.display(),
                    snapshot.source_version,
                    source_version
                );
            }
        }
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let resources = deleted_paths
            .iter()
            .chain(source_records.iter().map(|record| &record.skill_path))
            .chain(snapshots.iter().map(|snapshot| &snapshot.skill_path))
            .map(|path| crate::coordination::canonical_resource_path(path))
            .collect::<Result<Vec<_>>>()?;
        self.with_named_write_transaction("persist_skill_update_persistence_for_workspace_with_deleted_checked", |tx| {
        for (path, expected_version) in expected_source_versions {
            let current_version = tx
                .query_row(
                    "SELECT source_version FROM scoped_skill_sources
                     WHERE scope_key = ?1 AND skill_path = ?2",
                    params![scope_key.as_str(), path.display().to_string()],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?;
            let current_version = current_version.flatten();
            if current_version != expected_version.clone() {
                crate::logging::global().warn(
                    "skill source version validation failed",
                    serde_json::json!({
                        "operation": "persist_skill_update_persistence_for_workspace_checked",
                        "table": "scoped_skill_sources",
                        "scopeKey": scope_key.as_str(),
                        "skillPath": path,
                        "expectedSourceVersion": expected_version,
                        "currentSourceVersion": current_version,
                    }),
                );
                bail!(
                    "skill source {} changed after the update preview; preview the update again",
                    path.display()
                );
            }
        }
        for path in deleted_paths {
            tx.execute(
                "DELETE FROM scoped_skill_sources
                 WHERE scope_key = ?1 AND skill_path = ?2",
                params![scope_key.as_str(), path.display().to_string()],
            )?;
            tx.execute(
                "DELETE FROM scoped_skill_snapshots
                 WHERE scope_key = ?1 AND skill_path = ?2",
                params![scope_key.as_str(), path.display().to_string()],
            )?;
        }
        for record in source_records {
            log_skill_source_record_write("scoped_skill_sources", Some(scope_key.as_str()), record);
            tx.execute(
                "INSERT INTO scoped_skill_sources (
                    scope_key, skill_path, skill_name, source_kind, source, source_ref, source_version,
                    source_relative_path, update_status, origin, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(scope_key, skill_path) DO UPDATE SET
                    skill_name = excluded.skill_name,
                    source_kind = excluded.source_kind,
                    source = excluded.source,
                    source_ref = excluded.source_ref,
                    source_version = excluded.source_version,
                    source_relative_path = excluded.source_relative_path,
                    update_status = excluded.update_status,
                    origin = excluded.origin,
                    data_json = excluded.data_json",
                params![
                    scope_key.as_str(),
                    record.skill_path.display().to_string(),
                    record.skill_name,
                    record.source_kind,
                    record.source,
                    record.source_ref,
                    record.source_version,
                    record.source_relative_path,
                    record.update_status,
                    record.origin,
                    serde_json::to_string(record)?,
                ],
            )?;
        }
        for snapshot in snapshots {
            tx.execute(
                "DELETE FROM scoped_skill_snapshots
                 WHERE scope_key = ?1 AND skill_path = ?2",
                params![
                    scope_key.as_str(),
                    snapshot.skill_path.display().to_string()
                ],
            )?;
            for file in &snapshot.files {
                tx.execute(
                    "INSERT INTO scoped_skill_snapshots (
                        scope_key, skill_path, source_version, relative_path, content
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        scope_key.as_str(),
                        snapshot.skill_path.display().to_string(),
                        snapshot.source_version,
                        file.relative_path,
                        file.content,
                    ],
                )?;
            }
        }
            self.mark_projection_resources_in_tx(tx, &scope_key, "skills", &resources, false, true)?;
            Ok(())
        })?;
        crate::logging::global().debug(
            "skill update persistence committed",
            serde_json::json!({
                "operation": "persist_skill_update_persistence_for_workspace_with_deleted_checked",
                "table": "scoped_skill_sources",
                "scopeKey": scope_key.as_str(),
                "deletedCount": deleted_paths.len(),
                "recordCount": source_records.len(),
                "snapshotCount": snapshots.len(),
            }),
        );
        Ok(())
    }

    pub fn validate_skill_source_versions_for_workspace(
        &self,
        workspace_root: &Path,
        expected_source_versions: &[(PathBuf, Option<String>)],
    ) -> Result<()> {
        if expected_source_versions.is_empty() {
            return Ok(());
        }
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let tx = self.conn.unchecked_transaction()?;
        for (path, expected_version) in expected_source_versions {
            let current_version = tx
                .query_row(
                    "SELECT source_version FROM scoped_skill_sources
                     WHERE scope_key = ?1 AND skill_path = ?2",
                    params![scope_key.as_str(), path.display().to_string()],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?;
            let current_version = current_version.flatten();
            if current_version != expected_version.clone() {
                crate::logging::global().warn(
                    "skill source version validation failed",
                    serde_json::json!({
                        "operation": "validate_skill_source_versions_for_workspace",
                        "table": "scoped_skill_sources",
                        "scopeKey": scope_key.as_str(),
                        "skillPath": path,
                        "expectedSourceVersion": expected_version,
                        "currentSourceVersion": current_version,
                    }),
                );
                bail!(
                    "skill source {} changed after the update preview; preview the update again",
                    path.display()
                );
            }
        }
        tx.rollback()?;
        Ok(())
    }

    pub fn replace_skill_snapshots(&self, snapshots: &[SkillSnapshot]) -> Result<()> {
        if snapshots.is_empty() {
            return Ok(());
        }
        self.with_named_write_transaction("replace_skill_snapshots", |tx| {
            for snapshot in snapshots {
                tx.execute(
                    "DELETE FROM scoped_skill_snapshots
                     WHERE scope_key = ?1 AND skill_path = ?2",
                    params![
                        INSTALLATION_SCOPE_KEY,
                        snapshot.skill_path.display().to_string()
                    ],
                )?;
                for file in &snapshot.files {
                    tx.execute(
                        "INSERT INTO scoped_skill_snapshots (
                        scope_key, skill_path, source_version, relative_path, content
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            INSTALLATION_SCOPE_KEY,
                            snapshot.skill_path.display().to_string(),
                            snapshot.source_version,
                            file.relative_path,
                            file.content,
                        ],
                    )?;
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn replace_skill_snapshots_for_workspace(
        &self,
        workspace_root: &Path,
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        if snapshots.is_empty() {
            return Ok(());
        }
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        self.with_named_write_transaction("replace_skill_snapshots_for_workspace", |tx| {
            for snapshot in snapshots {
                tx.execute(
                    "DELETE FROM scoped_skill_snapshots
                 WHERE scope_key = ?1 AND skill_path = ?2",
                    params![
                        scope_key.as_str(),
                        snapshot.skill_path.display().to_string()
                    ],
                )?;
                for file in &snapshot.files {
                    tx.execute(
                        "INSERT INTO scoped_skill_snapshots (
                        scope_key, skill_path, source_version, relative_path, content
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            scope_key.as_str(),
                            snapshot.skill_path.display().to_string(),
                            snapshot.source_version,
                            file.relative_path,
                            file.content,
                        ],
                    )?;
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn persist_skill_update_persistence(
        &self,
        source_records: &[SkillSourceRecord],
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        if source_records.is_empty() && snapshots.is_empty() {
            return Ok(());
        }
        self.with_named_write_transaction("persist_skill_update_persistence", |tx| {
            Self::persist_skill_update_in_tx(
                tx,
                INSTALLATION_SCOPE_KEY,
                source_records,
                snapshots,
            )?;
            Ok(())
        })?;
        crate::logging::global().debug(
            "skill update persistence committed",
            serde_json::json!({
                "operation": "persist_skill_update_persistence",
                "table": "scoped_skill_sources",
                "scopeKey": INSTALLATION_SCOPE_KEY,
                "recordCount": source_records.len(),
                "snapshotCount": snapshots.len(),
            }),
        );
        Ok(())
    }

    fn persist_skill_update_in_tx(
        tx: &Transaction<'_>,
        scope_key: &str,
        source_records: &[SkillSourceRecord],
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        for record in source_records {
            log_skill_source_record_write("scoped_skill_sources", Some(scope_key), record);
            tx.execute(
                "INSERT INTO scoped_skill_sources (
                    scope_key, skill_path, skill_name, source_kind, source, source_ref, source_version,
                    source_relative_path, update_status, origin, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(scope_key, skill_path) DO UPDATE SET
                    skill_name = excluded.skill_name,
                    source_kind = excluded.source_kind,
                    source = excluded.source,
                    source_ref = excluded.source_ref,
                    source_version = excluded.source_version,
                    source_relative_path = excluded.source_relative_path,
                    update_status = excluded.update_status,
                    origin = excluded.origin,
                    data_json = excluded.data_json",
                params![
                    scope_key,
                    record.skill_path.display().to_string(),
                    record.skill_name,
                    record.source_kind,
                    record.source,
                    record.source_ref,
                    record.source_version,
                    record.source_relative_path,
                    record.update_status,
                    record.origin,
                    serde_json::to_string(record)?,
                ],
            )?;
        }
        for snapshot in snapshots {
            tx.execute(
                "DELETE FROM scoped_skill_snapshots
                 WHERE scope_key = ?1 AND skill_path = ?2",
                params![scope_key, snapshot.skill_path.display().to_string()],
            )?;
            for file in &snapshot.files {
                tx.execute(
                    "INSERT INTO scoped_skill_snapshots (
                        scope_key, skill_path, source_version, relative_path, content
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        scope_key,
                        snapshot.skill_path.display().to_string(),
                        snapshot.source_version,
                        file.relative_path,
                        file.content,
                    ],
                )?;
            }
        }
        Ok(())
    }

    pub fn persist_skill_update_persistence_checked(
        &self,
        expected_source_versions: &[(PathBuf, Option<String>)],
        source_records: &[SkillSourceRecord],
        snapshots: &[SkillSnapshot],
    ) -> Result<()> {
        self.with_named_write_transaction("persist_skill_update_persistence_checked", |tx| {
            Self::validate_skill_source_versions_on(tx, expected_source_versions)?;
            Self::persist_skill_update_in_tx(tx, INSTALLATION_SCOPE_KEY, source_records, snapshots)
        })
    }

    pub fn validate_skill_source_versions(
        &self,
        expected_source_versions: &[(PathBuf, Option<String>)],
    ) -> Result<()> {
        Self::validate_skill_source_versions_on(&self.conn, expected_source_versions)
    }

    fn validate_skill_source_versions_on(
        conn: &Connection,
        expected_source_versions: &[(PathBuf, Option<String>)],
    ) -> Result<()> {
        if expected_source_versions.is_empty() {
            return Ok(());
        }
        for (path, expected_version) in expected_source_versions {
            let current_version = conn
                .query_row(
                    "SELECT source_version FROM scoped_skill_sources
                     WHERE scope_key = ?1 AND skill_path = ?2",
                    params![INSTALLATION_SCOPE_KEY, path.display().to_string()],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?;
            let current_version = current_version.flatten();
            if current_version != expected_version.clone() {
                crate::logging::global().warn(
                    "skill source version validation failed",
                    serde_json::json!({
                        "operation": "validate_skill_source_versions",
                        "table": "scoped_skill_sources",
                        "scopeKey": INSTALLATION_SCOPE_KEY,
                        "skillPath": path,
                        "expectedSourceVersion": expected_version,
                        "currentSourceVersion": current_version,
                    }),
                );
                bail!(
                    "skill source {} changed after the update preview; preview the update again",
                    path.display()
                );
            }
        }
        Ok(())
    }

    pub fn upsert_skill_source_records_for_workspace(
        &self,
        workspace_root: &Path,
        records: &[SkillSourceRecord],
    ) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let prepared = records
            .iter()
            .map(|record| {
                log_skill_source_record_write(
                    "scoped_skill_sources",
                    Some(scope_key.as_str()),
                    record,
                );
                Ok((record, serde_json::to_string(record)?))
            })
            .collect::<Result<Vec<_>>>()?;
        let resources = records
            .iter()
            .map(|record| crate::coordination::canonical_resource_path(&record.skill_path))
            .collect::<Result<Vec<_>>>()?;
        let changed = self.with_named_write_transaction("upsert_skill_source_records_for_workspace", |tx| {
        let mut changed = 0;
        for (record, data_json) in &prepared {
            changed += tx.execute(
                "INSERT INTO scoped_skill_sources (
                    scope_key, skill_path, skill_name, source_kind, source, source_ref, source_version,
                    source_relative_path, update_status, origin, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(scope_key, skill_path) DO UPDATE SET
                    skill_name = excluded.skill_name,
                    source_kind = excluded.source_kind,
                    source = excluded.source,
                    source_ref = excluded.source_ref,
                    source_version = excluded.source_version,
                    source_relative_path = excluded.source_relative_path,
                    update_status = excluded.update_status,
                    origin = excluded.origin,
                    data_json = excluded.data_json",
                params![
                    scope_key.as_str(),
                    record.skill_path.display().to_string(),
                    record.skill_name,
                    record.source_kind,
                    record.source,
                    record.source_ref,
                    record.source_version,
                    record.source_relative_path,
                    record.update_status,
                    record.origin,
                    data_json,
                ],
            )?;
        }
            if changed > 0 {
                self.mark_projection_resources_in_tx(tx, &scope_key, "skills", &resources, false, true)?;
            }
            Ok(changed)
        })?;
        crate::logging::global().debug(
            "skill source records committed",
            serde_json::json!({
                "operation": "upsert_skill_source_records_for_workspace",
                "table": "scoped_skill_sources",
                "scopeKey": scope_key.as_str(),
                "recordCount": records.len(),
            }),
        );
        Ok(changed)
    }

    pub fn delete_skill_source_records(&self, skill_paths: &[PathBuf]) -> Result<usize> {
        if skill_paths.is_empty() {
            return Ok(0);
        }
        let deleted = self.with_named_write_transaction("delete_skill_source_records", |tx| {
            let mut deleted = 0;
            for skill_path in skill_paths {
                deleted += tx.execute(
                    "DELETE FROM scoped_skill_sources
                     WHERE scope_key = ?1 AND skill_path = ?2",
                    params![INSTALLATION_SCOPE_KEY, skill_path.display().to_string()],
                )?;
                tx.execute(
                    "DELETE FROM scoped_skill_snapshots
                     WHERE scope_key = ?1 AND skill_path = ?2",
                    params![INSTALLATION_SCOPE_KEY, skill_path.display().to_string()],
                )?;
            }
            Ok(deleted)
        })?;
        Ok(deleted)
    }

    pub fn delete_skill_source_records_for_workspace(
        &self,
        workspace_root: &Path,
        skill_paths: &[PathBuf],
    ) -> Result<usize> {
        if skill_paths.is_empty() {
            return Ok(0);
        }
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let resources = skill_paths
            .iter()
            .map(|path| crate::coordination::canonical_resource_path(path))
            .collect::<Result<Vec<_>>>()?;
        let deleted =
            self.with_named_write_transaction("delete_skill_source_records_for_workspace", |tx| {
                let mut deleted = 0;
                for skill_path in skill_paths {
                    deleted += tx.execute(
                        "DELETE FROM scoped_skill_sources WHERE scope_key = ?1 AND skill_path = ?2",
                        params![scope_key.as_str(), skill_path.display().to_string()],
                    )?;
                    tx.execute(
                "DELETE FROM scoped_skill_snapshots WHERE scope_key = ?1 AND skill_path = ?2",
                params![scope_key.as_str(), skill_path.display().to_string()],
            )?;
                }
                if deleted > 0 {
                    self.mark_projection_resources_in_tx(
                        tx, &scope_key, "skills", &resources, false, true,
                    )?;
                }
                Ok(deleted)
            })?;
        Ok(deleted)
    }

    pub fn skill_snapshot(&self, skill_path: &Path) -> Result<Option<SkillSnapshot>> {
        let path = skill_path.display().to_string();
        let read_snapshot = |sql: &str| -> Result<Option<SkillSnapshot>> {
            let mut statement = self.conn.prepare(sql)?;
            let mut rows = statement.query(params![&path])?;
            let Some(first) = rows.next()? else {
                return Ok(None);
            };
            let source_version = first.get::<_, String>(0)?;
            let mut files = vec![SkillSnapshotFile {
                relative_path: first.get(1)?,
                content: first.get(2)?,
            }];
            while let Some(row) = rows.next()? {
                files.push(SkillSnapshotFile {
                    relative_path: row.get(1)?,
                    content: row.get(2)?,
                });
            }
            Ok(Some(SkillSnapshot {
                skill_path: skill_path.to_path_buf(),
                source_version,
                files,
            }))
        };

        read_snapshot(
            "SELECT source_version, relative_path, content
             FROM scoped_skill_snapshots
             WHERE scope_key = 'installation:default' AND skill_path = ?1
             ORDER BY relative_path",
        )
    }

    pub fn skill_snapshot_for_workspace(
        &self,
        workspace_root: &Path,
        skill_path: &Path,
    ) -> Result<Option<SkillSnapshot>> {
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let path = skill_path.display().to_string();
        let mut statement = self.conn.prepare(
            "SELECT source_version, relative_path, content
             FROM scoped_skill_snapshots
             WHERE scope_key = ?1 AND skill_path = ?2
             ORDER BY relative_path",
        )?;
        let mut rows = statement.query(params![scope_key.as_str(), &path])?;
        let Some(first) = rows.next()? else {
            return Ok(None);
        };
        let source_version = first.get::<_, String>(0)?;
        let mut files = vec![SkillSnapshotFile {
            relative_path: first.get(1)?,
            content: first.get(2)?,
        }];
        while let Some(row) = rows.next()? {
            files.push(SkillSnapshotFile {
                relative_path: row.get(1)?,
                content: row.get(2)?,
            });
        }
        Ok(Some(SkillSnapshot {
            skill_path: skill_path.to_path_buf(),
            source_version,
            files,
        }))
    }

    pub fn insert_skill_source_records_if_missing(
        &self,
        records: &[SkillSourceRecord],
    ) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        let inserted =
            self.with_named_write_transaction("insert_skill_source_records_if_missing", |tx| {
                let mut inserted = 0;
                for record in records {
                    log_skill_source_record_write(
                        "scoped_skill_sources",
                        Some(INSTALLATION_SCOPE_KEY),
                        record,
                    );
                    inserted += tx.execute(
                        "INSERT OR IGNORE INTO scoped_skill_sources (
                    scope_key, skill_path, skill_name, source_kind, source, source_ref, source_version,
                    source_relative_path, update_status, origin, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                        params![
                            INSTALLATION_SCOPE_KEY,
                            record.skill_path.display().to_string(),
                            record.skill_name,
                            record.source_kind,
                            record.source,
                            record.source_ref,
                            record.source_version,
                            record.source_relative_path,
                            record.update_status,
                            record.origin,
                            serde_json::to_string(record)?,
                        ],
                    )?;
                }
                Ok(inserted)
            })?;
        Ok(inserted)
    }

    pub fn insert_skill_source_records_if_missing_for_workspace(
        &self,
        workspace_root: &Path,
        records: &[SkillSourceRecord],
    ) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let inserted = self.with_named_write_transaction(
            "insert_skill_source_records_if_missing_for_workspace",
            |tx| {
                let inserted = self.insert_skill_source_records_if_missing_for_workspace_in_tx(
                    &tx, &scope_key, records,
                )?;
                Ok(inserted)
            },
        )?;
        Ok(inserted)
    }

    pub(crate) fn insert_skill_source_records_if_missing_for_workspace_in_tx(
        &self,
        tx: &Transaction<'_>,
        scope_key: &ScopeKey,
        records: &[SkillSourceRecord],
    ) -> Result<usize> {
        let mut inserted = 0;
        for record in records {
            log_skill_source_record_write("scoped_skill_sources", Some(scope_key.as_str()), record);
            inserted += tx.execute(
                "INSERT OR IGNORE INTO scoped_skill_sources (
                    scope_key, skill_path, skill_name, source_kind, source, source_ref, source_version,
                    source_relative_path, update_status, origin, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    scope_key.as_str(),
                    record.skill_path.display().to_string(),
                    record.skill_name,
                    record.source_kind,
                    record.source,
                    record.source_ref,
                    record.source_version,
                    record.source_relative_path,
                    record.update_status,
                    record.origin,
                    serde_json::to_string(record)?,
                ],
            )?;
        }
        Ok(inserted)
    }

    /// Persist the source rows produced by a complete skill projection. This
    /// is normal projection synchronization, not a compatibility transition:
    /// the successful scan is authoritative for the workspace's current paths.
    pub(in crate::storage) fn replace_skill_source_projection_in_tx(
        &self,
        tx: &Transaction<'_>,
        scope_key: &ScopeKey,
        records: &[SkillSourceRecord],
    ) -> Result<()> {
        let mut records_by_path = BTreeMap::new();
        for record in records {
            records_by_path
                .entry(record.skill_path.clone())
                .or_insert(record);
        }
        for record in records_by_path.values() {
            log_skill_source_record_write("scoped_skill_sources", Some(scope_key.as_str()), record);
            tx.execute(
                "INSERT INTO scoped_skill_sources (
                    scope_key, skill_path, skill_name, source_kind, source, source_ref, source_version,
                    source_relative_path, update_status, origin, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(scope_key, skill_path) DO UPDATE SET
                    skill_name = excluded.skill_name,
                    source_kind = excluded.source_kind,
                    source = excluded.source,
                    source_ref = excluded.source_ref,
                    source_version = excluded.source_version,
                    source_relative_path = excluded.source_relative_path,
                    update_status = excluded.update_status,
                    origin = excluded.origin,
                    data_json = excluded.data_json",
                params![
                    scope_key.as_str(),
                    record.skill_path.display().to_string(),
                    record.skill_name,
                    record.source_kind,
                    record.source,
                    record.source_ref,
                    record.source_version,
                    record.source_relative_path,
                    record.update_status,
                    record.origin,
                    serde_json::to_string(record)?,
                ],
            )?;
        }

        let current_paths = records_by_path
            .keys()
            .map(|path| path.display().to_string())
            .collect::<BTreeSet<_>>();
        let existing_paths = {
            let mut statement =
                tx.prepare("SELECT skill_path FROM scoped_skill_sources WHERE scope_key = ?1")?;
            let rows = statement.query_map(params![scope_key.as_str()], |row| row.get(0))?;
            rows.collect::<std::result::Result<Vec<String>, _>>()?
        };
        for skill_path in existing_paths {
            if current_paths.contains(&skill_path) {
                continue;
            }
            tx.execute(
                "DELETE FROM scoped_skill_sources
                 WHERE scope_key = ?1 AND skill_path = ?2",
                params![scope_key.as_str(), skill_path],
            )?;
            tx.execute(
                "DELETE FROM scoped_skill_snapshots
                 WHERE scope_key = ?1 AND skill_path = ?2",
                params![scope_key.as_str(), skill_path],
            )?;
        }
        Ok(())
    }

    pub fn upsert_skill_source_records(&self, records: &[SkillSourceRecord]) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        let changed = self.with_named_write_transaction("upsert_skill_source_records", |tx| {
            let mut changed = 0;
            for record in records {
                log_skill_source_record_write(
                    "scoped_skill_sources",
                    Some(INSTALLATION_SCOPE_KEY),
                    record,
                );
                changed += tx.execute(
                    "INSERT INTO scoped_skill_sources (
                    scope_key, skill_path, skill_name, source_kind, source, source_ref, source_version,
                    source_relative_path, update_status, origin, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(scope_key, skill_path) DO UPDATE SET
                    skill_name = excluded.skill_name,
                    source_kind = excluded.source_kind,
                    source = excluded.source,
                    source_ref = excluded.source_ref,
                    source_version = excluded.source_version,
                    source_relative_path = excluded.source_relative_path,
                    update_status = excluded.update_status,
                    origin = excluded.origin,
                    data_json = excluded.data_json",
                    params![
                        INSTALLATION_SCOPE_KEY,
                        record.skill_path.display().to_string(),
                        record.skill_name,
                        record.source_kind,
                        record.source,
                        record.source_ref,
                        record.source_version,
                        record.source_relative_path,
                        record.update_status,
                        record.origin,
                        serde_json::to_string(record)?,
                    ],
                )?;
            }
            Ok(changed)
        })?;
        crate::logging::global().debug(
            "skill source records committed",
            serde_json::json!({
                "operation": "upsert_skill_source_records",
                "table": "scoped_skill_sources",
                "scopeKey": INSTALLATION_SCOPE_KEY,
                "recordCount": records.len(),
            }),
        );
        Ok(changed)
    }

    /// Commit only the deleted installation metadata. The subsequent pure scan
    /// publishes separately using its captured projection revision, so unrelated
    /// concurrent installations cannot be overwritten by an old whole snapshot.
    pub fn delete_skill_sources_for_workspace(
        &self,
        workspace_root: &Path,
        deleted_paths: &[PathBuf],
        deleted_visibility_paths: &[PathBuf],
    ) -> Result<()> {
        let workspace_root = canonical_workspace_root(workspace_root);
        let scope_key = workspace_scope_key(&workspace_root)?;
        let deleted_visibility_paths = deleted_visibility_paths
            .iter()
            .map(|path| canonical_skill_visibility_path(path))
            .collect::<Vec<_>>();
        let resources = deleted_paths
            .iter()
            .chain(&deleted_visibility_paths)
            .map(|path| crate::coordination::canonical_resource_path(path))
            .collect::<Result<Vec<_>>>()?;
        self.with_named_write_transaction("delete_skill_sources_for_workspace", |tx| {
            for path in &deleted_visibility_paths {
                tx.execute(
                    "DELETE FROM scoped_skill_visibility WHERE scope_key = ?1 AND skill_path = ?2",
                    params![scope_key.as_str(), path.display().to_string()],
                )?;
            }
            for path in deleted_paths {
                tx.execute(
                    "DELETE FROM scoped_skill_sources WHERE scope_key = ?1 AND skill_path = ?2",
                    params![scope_key.as_str(), path.display().to_string()],
                )?;
                tx.execute(
                    "DELETE FROM scoped_skill_snapshots WHERE scope_key = ?1 AND skill_path = ?2",
                    params![scope_key.as_str(), path.display().to_string()],
                )?;
            }
            self.mark_projection_resources_in_tx(tx, &scope_key, "skills", &resources, false, true)
        })
    }

    pub fn skill_source_records(&self) -> Result<Vec<SkillSourceRecord>> {
        let mut records = Vec::new();
        let mut statement = self.conn.prepare(
            "SELECT data_json FROM scoped_skill_sources
             WHERE scope_key = ?1",
        )?;
        let rows = statement.query_map([INSTALLATION_SCOPE_KEY], |row| row.get::<_, String>(0))?;
        for row in rows {
            let data_json = row?;
            records.push(
                serde_json::from_str::<SkillSourceRecord>(&data_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
            );
        }
        records.sort_by(|left, right| {
            left.skill_name
                .cmp(&right.skill_name)
                .then_with(|| left.skill_path.cmp(&right.skill_path))
        });
        records.dedup_by(|left, right| left.skill_path == right.skill_path);
        Ok(records)
    }

    pub fn skill_source_records_for_workspace(
        &self,
        workspace_root: &Path,
    ) -> Result<Vec<SkillSourceRecord>> {
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let mut records_by_path = BTreeMap::new();
        let mut statement = self.conn.prepare(
            "SELECT data_json FROM scoped_skill_sources
             WHERE scope_key = ?1
             ORDER BY skill_name, skill_path",
        )?;
        let rows =
            statement.query_map(params![scope_key.as_str()], |row| row.get::<_, String>(0))?;
        for row in rows {
            let data_json = row?;
            let record =
                serde_json::from_str::<SkillSourceRecord>(&data_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
            records_by_path.insert(record.skill_path.clone(), record);
        }

        let mut records = records_by_path.into_values().collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.skill_name
                .cmp(&right.skill_name)
                .then_with(|| left.skill_path.cmp(&right.skill_path))
        });
        Ok(records)
    }

    pub fn skill_source_record(&self, skill_path: &Path) -> Result<Option<SkillSourceRecord>> {
        let data_json = self
            .conn
            .query_row(
                "SELECT data_json FROM scoped_skill_sources
                 WHERE scope_key = ?1 AND skill_path = ?2",
                params![INSTALLATION_SCOPE_KEY, skill_path.display().to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        data_json
            .map(|data_json| serde_json::from_str(&data_json).map_err(Into::into))
            .transpose()
    }

    pub fn skill_source_record_for_workspace(
        &self,
        workspace_root: &Path,
        skill_path: &Path,
    ) -> Result<Option<SkillSourceRecord>> {
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let data_json = self
            .conn
            .query_row(
                "SELECT data_json FROM scoped_skill_sources
                 WHERE scope_key = ?1 AND skill_path = ?2",
                params![scope_key.as_str(), skill_path.display().to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        data_json
            .map(|data_json| serde_json::from_str(&data_json).map_err(Into::into))
            .transpose()
    }
}
