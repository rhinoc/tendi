//! projects persistence through the database-owned transaction boundary.
use super::super::*;

struct PreparedProjectScan {
    scopes: Vec<ProjectScanScope>,
    projects: Vec<(ProjectRecord, String)>,
    warnings: Vec<String>,
}

#[cfg(test)]
#[path = "projects_tests.rs"]
mod tests;

impl Store {
    pub(in crate::storage) fn list_projects_on(conn: &Connection) -> Result<Vec<ProjectRecord>> {
        let mut statement = conn.prepare(
            "SELECT data_json FROM projects
             WHERE status = 'ready'
             ORDER BY name COLLATE NOCASE ASC, root_path ASC",
        )?;
        let rows = statement.query_map([], |row| {
            let data: String = row.get(0)?;
            serde_json::from_str::<ProjectRecord>(&data).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn scan_projects(&self) -> Result<ProjectScanResult> {
        self.scan_projects_inner(None)
    }

    pub fn scan_projects_for_workspace(&self, workspace_root: &Path) -> Result<ProjectScanResult> {
        let scope_key = workspace_scope_key(workspace_root)?;
        self.scan_projects_inner(Some(&scope_key))
    }

    pub(in crate::storage) fn scan_projects_inner(
        &self,
        invalidate_scope: Option<&ScopeKey>,
    ) -> Result<ProjectScanResult> {
        // Own catalog ordering here so CLI and daemon scans share one boundary.
        // Editing scan configuration remains independent and invalidates this scan.
        let _catalog = crate::coordination::ResourceLease::acquire(self.path(), "project-catalog")?;
        let scan = self.prepare_project_scan()?;
        self.commit_project_scan(scan, invalidate_scope)
    }

    fn prepare_project_scan(&self) -> Result<PreparedProjectScan> {
        let scopes = self.project_scan_scopes()?;
        let exclusion_matcher = projects::build_exclusion_matcher(&scopes)?;
        let mut scanned_projects = BTreeMap::new();
        let mut warnings = Vec::new();
        for scope in scopes
            .iter()
            .filter(|scope| scope.enabled && !scope.excluded)
        {
            let (projects, scope_warnings) =
                projects::scan_scope(&scope.path, &scope.id, &exclusion_matcher);
            for project in projects {
                scanned_projects
                    .entry(project.id.clone())
                    .or_insert(project);
            }
            warnings.extend(scope_warnings);
        }
        let projects = scanned_projects
            .into_values()
            .map(|project| Ok((project.clone(), serde_json::to_string(&project)?)))
            .collect::<Result<Vec<_>>>()?;
        Ok(PreparedProjectScan {
            scopes,
            projects,
            warnings,
        })
    }

    fn commit_project_scan(
        &self,
        scan: PreparedProjectScan,
        invalidate_scope: Option<&ScopeKey>,
    ) -> Result<ProjectScanResult> {
        let PreparedProjectScan {
            scopes,
            projects,
            warnings,
        } = scan;
        let exclusion_matcher = projects::build_exclusion_matcher(&scopes)?;

        self.with_named_write_transaction("scan_projects", |tx| {
            let mut statement =
                tx.prepare("SELECT id, path, enabled FROM project_scan_scopes ORDER BY path ASC")?;
            let current = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, bool>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let expected = scopes
                .iter()
                .map(|scope| {
                    let path = if scope.excluded {
                        format!("!{}", scope.path.display())
                    } else {
                        scope.path.to_string_lossy().into_owned()
                    };
                    (scope.id.clone(), path, scope.enabled)
                })
                .collect::<Vec<_>>();
            anyhow::ensure!(
                current == expected,
                "project scan configuration changed while scanning; scan again"
            );
            let existing_projects = Self::list_projects_on(tx)?;
            for scope in scopes
                .iter()
                .filter(|scope| scope.enabled && !scope.excluded)
            {
                tx.execute(
                    "UPDATE projects SET status = 'missing'
                 WHERE scope_id = ?1 AND status = 'ready'",
                    [&scope.id],
                )?;
            }
            for project in existing_projects.iter().filter(|project| {
                projects::path_is_excluded(&exclusion_matcher, &project.root_path, true)
            }) {
                tx.execute(
                    "UPDATE projects SET status = 'out-of-scope' WHERE id = ?1",
                    [&project.id],
                )?;
            }
            for scope in scopes.iter().filter(|scope| !scope.enabled) {
                tx.execute(
                    "UPDATE projects SET status = 'out-of-scope'
                 WHERE scope_id = ?1 AND status != 'out-of-scope'",
                    [&scope.id],
                )?;
            }
            for (project, data_json) in &projects {
                tx.execute(
                    "INSERT INTO projects (
                    id, root_path, name, remote_url, scope_id, status,
                    last_scanned_at, data_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                    root_path = excluded.root_path,
                    name = excluded.name,
                    remote_url = excluded.remote_url,
                    scope_id = excluded.scope_id,
                    status = excluded.status,
                    last_scanned_at = excluded.last_scanned_at,
                    data_json = excluded.data_json",
                    params![
                        project.id,
                        project.root_path.to_string_lossy(),
                        project.name,
                        project.remote_url,
                        project.scope_id,
                        project.status,
                        project.last_scanned_at,
                        data_json,
                    ],
                )?;
            }
            for scope in scopes.iter().filter(|scope| scope.enabled) {
                tx.execute(
                    "UPDATE project_scan_scopes
                 SET last_scanned_at = ?2
                 WHERE id = ?1",
                    params![scope.id, Local::now().to_rfc3339()],
                )?;
            }
            if let Some(scope_key) = invalidate_scope {
                for domain in ["skills", "rules", "mcp"] {
                    self.set_projection_context_in_tx(tx, scope_key, domain, "stale", None)?;
                }
            }
            Ok(())
        })?;

        Ok(ProjectScanResult {
            projects: self.list_projects()?,
            scopes: self.project_scan_scopes()?,
            warnings,
        })
    }

    pub fn project_scan_scopes(&self) -> Result<Vec<ProjectScanScope>> {
        with_database_read_lock_retry(|| self.project_scan_scopes_once())
    }

    pub(in crate::storage) fn project_scan_scopes_once(&self) -> Result<Vec<ProjectScanScope>> {
        Self::project_scan_scopes_on(&self.conn)
    }

    fn project_scan_scopes_on(conn: &Connection) -> Result<Vec<ProjectScanScope>> {
        let mut statement = conn.prepare(
            "SELECT s.id, s.path, s.enabled, s.last_scanned_at,
                    (SELECT COUNT(*) FROM projects p WHERE p.scope_id = s.id AND p.status = 'ready')
             FROM project_scan_scopes s
             ORDER BY s.path ASC",
        )?;
        let rows = statement.query_map([], |row| {
            let id: String = row.get(0)?;
            let stored_path: String = row.get(1)?;
            let excluded = stored_path.starts_with('!');
            let path = PathBuf::from(stored_path.strip_prefix('!').unwrap_or(&stored_path));
            Ok(ProjectScanScope {
                id,
                path,
                excluded,
                enabled: row.get::<_, i64>(2)? != 0,
                last_scanned_at: row.get(3)?,
                project_count: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn save_project_scan_scopes(&self, values: Vec<String>) -> Result<Vec<ProjectScanScope>> {
        let paths = projects::normalize_scope_paths(values)?;
        self.with_named_write_transaction("save_project_scan_scopes", |tx| {
            tx.execute("UPDATE project_scan_scopes SET enabled = 0", [])?;
            for scope in paths {
                let id = projects::scope_id_for_path(&scope.path, scope.excluded);
                let stored_path = if scope.excluded {
                    format!("!{}", scope.path.display())
                } else {
                    scope.path.to_string_lossy().into_owned()
                };
                tx.execute(
                    "INSERT INTO project_scan_scopes (id, path, enabled)
                 VALUES (?1, ?2, 1)
                 ON CONFLICT(id) DO UPDATE SET path = excluded.path, enabled = 1",
                    params![id, stored_path],
                )?;
            }
            Ok(())
        })?;
        self.project_scan_scopes()
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRecord>> {
        with_database_read_lock_retry(|| self.list_projects_once())
    }

    pub(in crate::storage) fn list_projects_once(&self) -> Result<Vec<ProjectRecord>> {
        Self::list_projects_on(&self.conn)
    }
}
