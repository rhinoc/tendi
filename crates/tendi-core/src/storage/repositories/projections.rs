//! projections persistence through the database-owned transaction boundary.
use super::super::*;

/// One durable work receipt. Revisions cover both publication and reconciliation.
#[derive(Debug)]
pub struct ProjectionRefreshState<T> {
    pub revision: Revision,
    pub snapshot: Option<T>,
    pub resources: Vec<PathBuf>,
    pub full_refresh: bool,
    pub reconcile_resources: Vec<PathBuf>,
    pub reconcile_full: bool,
}

#[cfg(test)]
#[path = "projections_tests.rs"]
mod concurrency_tests;

pub(in crate::storage) fn advance_projection_head_in_tx(
    tx: &Transaction<'_>,
    scope_key: &ScopeKey,
    domain: &str,
    source_version: Option<&SourceVersion>,
    status: &str,
) -> Result<ProjectionHead> {
    if domain.trim().is_empty() {
        anyhow::bail!("projection domain must not be empty");
    }
    if status.trim().is_empty() {
        anyhow::bail!("projection status must not be empty");
    }
    let current = tx
        .query_row(
            "SELECT revision FROM projection_heads
             WHERE scope_key = ?1 AND domain = ?2",
            params![scope_key.as_str(), domain],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0);
    let revision = current
        .checked_add(1)
        .context("projection revision overflow")?;
    let updated_at = i64::try_from(unix_now()).context("invalid projection timestamp")?;
    tx.execute(
        "INSERT INTO projection_heads
            (scope_key, domain, revision, source_version, schema_version, status, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(scope_key, domain) DO UPDATE SET
            revision = excluded.revision,
            source_version = excluded.source_version,
            schema_version = excluded.schema_version,
            status = excluded.status,
            updated_at = excluded.updated_at",
        params![
            scope_key.as_str(),
            domain,
            revision,
            source_version.map(SourceVersion::as_str),
            1_i64,
            status,
            updated_at,
        ],
    )?;
    Ok(ProjectionHead {
        scope_key: scope_key.clone(),
        domain: domain.to_string(),
        revision: Revision::new(u64::try_from(revision)?),
        source_version: source_version.cloned(),
        schema_version: 1,
        status: status.to_string(),
    })
}

impl Store {
    pub fn invalidate_projection_resources(
        &self,
        domain: &str,
        workspace_root: &Path,
        paths: &[PathBuf],
        reconcile: bool,
    ) -> Result<()> {
        ensure_projection_domain(domain)?;
        let scope = workspace_scope_key(workspace_root)?;
        let resources = paths
            .iter()
            .map(|path| crate::coordination::canonical_resource_path(path))
            .collect::<Result<Vec<_>>>()?;
        self.with_named_write_transaction("invalidate_projection_resources", |tx| {
            self.mark_projection_resources_in_tx(tx, &scope, domain, &resources, reconcile, true)
        })
    }

    pub(in crate::storage) fn mark_projection_resources_in_tx(
        &self,
        tx: &Transaction<'_>,
        scope: &ScopeKey,
        domain: &str,
        resources: &[PathBuf],
        reconcile: bool,
        shared: bool,
    ) -> Result<()> {
        let keys = if resources.is_empty() {
            BTreeSet::from([String::new()])
        } else {
            resources
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect()
        };
        let mut scopes = BTreeMap::from([(scope.as_str().to_owned(), keys.clone())]);
        if shared && !resources.is_empty() {
            let kind = manifest_source_kinds(domain)?[0];
            let install_root_kind = format!("{kind}-install-root");
            let directory_kind = format!("{kind}-dir");
            let provider_directories = crate::providers::all_providers()
                .into_iter()
                .flat_map(|provider| {
                    provider
                        .projection_directories()
                        .iter()
                        .map(|value| value.to_string())
                        .collect::<Vec<_>>()
                })
                .collect::<BTreeSet<_>>();
            for resource in resources {
                let key = resource.to_string_lossy().into_owned();
                let ancestors = resource
                    .ancestors()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                let placeholders = (6..6 + ancestors.len())
                    .map(|index| format!("?{index}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                // Each UNION branch is an indexed exact or prefix range probe;
                // no scan over unrelated scopes' manifest rows is necessary.
                let sql = format!("SELECT scope_key, source_kind, path FROM fs_manifest WHERE source_kind IN (?1, ?2, ?3) AND resource_path IN ({placeholders}) AND scope_key LIKE 'workspace:%'
                    UNION SELECT scope_key, source_kind, path FROM fs_manifest WHERE source_kind IN (?1, ?2, ?3) AND resource_path >= ?4 AND resource_path < ?5 AND scope_key LIKE 'workspace:%'");
                let separator = std::path::MAIN_SEPARATOR;
                let prefix = key.trim_end_matches(separator);
                let upper_separator = char::from_u32(separator as u32 + 1)
                    .expect("path separator has a lexical successor");
                let mut values = vec![
                    SqlValue::Text(kind.into()),
                    SqlValue::Text(install_root_kind.clone()),
                    SqlValue::Text(directory_kind.clone()),
                    SqlValue::Text(format!("{prefix}{separator}")),
                    SqlValue::Text(format!("{prefix}{upper_separator}")),
                ];
                values.extend(ancestors.into_iter().map(SqlValue::Text));
                let mut statement = tx.prepare(&sql)?;
                let references = statement.query_map(params_from_iter(values.iter()), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        PathBuf::from(row.get::<_, String>(2)?),
                    ))
                })?;
                for reference in references {
                    let (owner, source_kind, logical) = reference?;
                    // Generic ancestor directories in the manifest detect external
                    // changes, but do not declare ownership of every descendant.
                    if source_kind == directory_kind
                        && !provider_directories
                            .iter()
                            .any(|directory| logical.ends_with(directory))
                    {
                        continue;
                    }
                    scopes.entry(owner).or_default().insert(key.clone());
                }
            }
        }
        for (scope, keys) in scopes {
            let scope = ScopeKey::new(scope)?;
            self.write_projection_context_in_tx(tx, &scope, domain, "stale", None)?;
            let generation = advance_projection_head_in_tx(tx, &scope, domain, None, "stale")?
                .revision
                .value();
            for key in &keys {
                tx.execute("INSERT INTO projection_dirty_resources(scope_key, domain, resource_key, generation, reconcile_generation)
                    VALUES (?1, ?2, ?3, ?4, ?5)
                    ON CONFLICT(scope_key, domain, resource_key) DO UPDATE SET generation = excluded.generation,
                    reconcile_generation = CASE WHEN excluded.reconcile_generation > 0 THEN excluded.reconcile_generation ELSE projection_dirty_resources.reconcile_generation END",
                    params![scope.as_str(), domain, key, generation, if reconcile { generation } else { 0 }])?;
            }
        }
        Ok(())
    }

    pub fn read_projection_refresh_state<T: for<'de> Deserialize<'de>>(
        &self,
        domain: &str,
        workspace_root: &Path,
    ) -> Result<ProjectionRefreshState<T>> {
        let scope = workspace_scope_key(workspace_root)?;
        // Both queries use this read-only connection's same snapshot.
        let read = self.conn.unchecked_transaction()?;
        let (revision, snapshot) =
            self.read_cached_projection_with_revision(domain, workspace_root)?;
        let mut statement = read.prepare("SELECT resource_key, generation, reconcile_generation FROM projection_dirty_resources WHERE scope_key = ?1 AND domain = ?2 ORDER BY resource_key")?;
        let rows = statement
            .query_map(params![scope.as_str(), domain], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        read.commit()?;
        let mut state = ProjectionRefreshState {
            revision,
            full_refresh: snapshot.is_none(),
            snapshot,
            resources: Vec::new(),
            reconcile_resources: Vec::new(),
            reconcile_full: false,
        };
        for (key, generation, reconcile_generation) in rows {
            if generation > 0 {
                if key.is_empty() {
                    state.full_refresh = true;
                } else {
                    state.resources.push(PathBuf::from(&key));
                }
            }
            if reconcile_generation > 0 {
                if key.is_empty() {
                    state.reconcile_full = true;
                } else {
                    state.reconcile_resources.push(PathBuf::from(key));
                }
            }
        }
        Ok(state)
    }

    pub fn pending_projection_scopes(&self, domain: &str) -> Result<Vec<PathBuf>> {
        let mut statement = self.conn.prepare("SELECT DISTINCT scope_key FROM projection_dirty_resources WHERE domain = ?1 AND (generation > 0 OR reconcile_generation > 0) ORDER BY scope_key")?;
        let scopes = statement
            .query_map([domain], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut pending = Vec::new();
        let mut orphaned = Vec::new();
        for scope in scopes {
            let Some(path) = scope.strip_prefix("workspace:").map(PathBuf::from) else {
                continue;
            };
            if path.is_dir() {
                pending.push(path);
            } else {
                orphaned.push(scope);
            }
        }
        if !orphaned.is_empty() {
            self.with_named_write_transaction("prune_orphaned_projection_dirty_resources", |tx| {
                for scope in &orphaned {
                    tx.execute(
                        "DELETE FROM projection_dirty_resources WHERE scope_key = ?1 AND domain = ?2",
                        params![scope, domain],
                    )?;
                }
                Ok(())
            })?;
        }
        Ok(pending)
    }

    pub fn acknowledge_projection_reconciliation(
        &self,
        domain: &str,
        workspace_root: &Path,
        captured: Revision,
    ) -> Result<()> {
        let scope = workspace_scope_key(workspace_root)?;
        self.with_named_write_transaction("acknowledge_projection_reconciliation", |tx| {
            tx.execute("UPDATE projection_dirty_resources SET reconcile_generation = 0 WHERE scope_key = ?1 AND domain = ?2 AND reconcile_generation <= ?3", params![scope.as_str(), domain, captured.value()])?;
            tx.execute("DELETE FROM projection_dirty_resources WHERE scope_key = ?1 AND domain = ?2 AND generation = 0 AND reconcile_generation = 0", params![scope.as_str(), domain])?;
            Ok(())
        })
    }

    pub fn save_hooks_for_workspace_if_revision(
        &self,
        workspace_root: &Path,
        scan: &HookScan,
        expected: Revision,
    ) -> Result<bool> {
        self.save_hooks_for_workspace_with_revision(workspace_root, scan, Some(expected))
    }

    fn save_hooks_for_workspace_with_revision(
        &self,
        workspace_root: &Path,
        scan: &HookScan,
        expected: Option<Revision>,
    ) -> Result<bool> {
        let workspace_root = canonical_workspace_root(workspace_root);
        self.save_projection_domain_snapshot_if_revision(
            &workspace_root,
            "hooks",
            scan.warnings.is_empty().then_some(scan),
            &manifest_entries_for_hooks(scan, &workspace_root),
            scan.warnings.is_empty(),
            (!scan.warnings.is_empty()).then(|| scan.warnings.join("; ")),
            &[],
            expected,
        )
    }

    pub fn save_mcp_for_workspace_if_revision(
        &self,
        workspace_root: &Path,
        scan: &McpScan,
        expected: Revision,
    ) -> Result<bool> {
        self.save_mcp_for_workspace_with_revision(workspace_root, scan, Some(expected))
    }

    fn save_mcp_for_workspace_with_revision(
        &self,
        workspace_root: &Path,
        scan: &McpScan,
        expected: Option<Revision>,
    ) -> Result<bool> {
        let workspace_root = canonical_workspace_root(workspace_root);
        self.save_projection_domain_snapshot_if_revision(
            &workspace_root,
            "mcp",
            scan.warnings.is_empty().then_some(scan),
            &manifest_entries_for_mcp(scan, &workspace_root),
            scan.warnings.is_empty(),
            (!scan.warnings.is_empty()).then(|| scan.warnings.join("; ")),
            &[],
            expected,
        )
    }

    pub fn save_agents_for_workspace_if_revision(
        &self,
        workspace_root: &Path,
        scan: &crate::agents::AgentScan,
        expected: Revision,
    ) -> Result<bool> {
        self.save_agents_for_workspace_with_revision(workspace_root, scan, Some(expected))
    }

    fn save_agents_for_workspace_with_revision(
        &self,
        workspace_root: &Path,
        scan: &crate::agents::AgentScan,
        expected: Option<Revision>,
    ) -> Result<bool> {
        let workspace_root = canonical_workspace_root(workspace_root);
        self.save_projection_domain_snapshot_if_revision(
            &workspace_root,
            "agents",
            scan.warnings.is_empty().then_some(scan),
            &manifest_entries_for_agents(scan, &workspace_root),
            scan.warnings.is_empty(),
            (!scan.warnings.is_empty()).then(|| scan.warnings.join("; ")),
            &[],
            expected,
        )
    }

    pub fn save_skills_for_workspace_if_revision(
        &self,
        workspace_root: &Path,
        scan: &SkillScan,
        expected: Revision,
    ) -> Result<bool> {
        self.save_skills_for_workspace_with_revision(workspace_root, scan, Some(expected))
    }

    fn save_skills_for_workspace_with_revision(
        &self,
        workspace_root: &Path,
        scan: &SkillScan,
        expected: Option<Revision>,
    ) -> Result<bool> {
        let workspace_root = canonical_workspace_root(workspace_root);
        let source_records = skill_source_records_from_scan(scan);
        self.save_projection_domain_snapshot_if_revision(
            &workspace_root,
            "skills",
            scan.warnings.is_empty().then_some(scan),
            &manifest_entries_for_skills(scan, &workspace_root),
            scan.warnings.is_empty(),
            (!scan.warnings.is_empty()).then(|| scan.warnings.join("; ")),
            &source_records,
            expected,
        )
    }

    pub fn save_rules_for_workspace_if_revision(
        &self,
        workspace_root: &Path,
        scan: &RuleScan,
        expected: Revision,
    ) -> Result<bool> {
        self.save_rules_for_workspace_with_revision(workspace_root, scan, Some(expected))
    }

    fn save_rules_for_workspace_with_revision(
        &self,
        workspace_root: &Path,
        scan: &RuleScan,
        expected: Option<Revision>,
    ) -> Result<bool> {
        let workspace_root = canonical_workspace_root(workspace_root);
        self.save_projection_domain_snapshot_if_revision(
            &workspace_root,
            "rules",
            scan.warnings.is_empty().then_some(scan),
            &manifest_entries_for_rules(scan, &workspace_root),
            scan.warnings.is_empty(),
            (!scan.warnings.is_empty()).then(|| scan.warnings.join("; ")),
            &[],
            expected,
        )
    }

    pub fn advance_projection_head(
        &self,
        scope_key: &ScopeKey,
        domain: &str,
        source_version: Option<&SourceVersion>,
        status: &str,
    ) -> Result<ProjectionHead> {
        if domain.trim().is_empty() {
            anyhow::bail!("projection domain must not be empty");
        }
        if status.trim().is_empty() {
            anyhow::bail!("projection status must not be empty");
        }
        let head = self.with_named_write_transaction("advance_projection_head", |tx| {
            let head =
                advance_projection_head_in_tx(&tx, scope_key, domain, source_version, status)?;
            Ok(head)
        })?;
        Ok(head)
    }

    #[cfg(test)]
    pub fn save_hooks_for_workspace(&self, workspace_root: &Path, scan: &HookScan) -> Result<()> {
        self.save_hooks_for_workspace_with_revision(workspace_root, scan, None)
            .map(|_| ())
    }

    #[cfg(test)]
    pub fn save_mcp_for_workspace(&self, workspace_root: &Path, scan: &McpScan) -> Result<()> {
        self.save_mcp_for_workspace_with_revision(workspace_root, scan, None)
            .map(|_| ())
    }

    fn save_projection_domain_snapshot_if_revision<T: Serialize>(
        &self,
        workspace_root: &Path,
        domain: &str,
        snapshot: Option<&T>,
        entries: &[FsManifestEntry],
        ready: bool,
        error: Option<String>,
        source_records: &[SkillSourceRecord],
        expected: Option<Revision>,
    ) -> Result<bool> {
        let scope_key = workspace_scope_key(workspace_root)?;
        let snapshot_json = snapshot.map(serde_json::to_string).transpose()?;
        let committed = self.with_named_write_transaction("save_projection_domain_snapshot", |tx| {
            if let Some(expected) = expected {
                let current = tx.query_row("SELECT revision FROM projection_heads WHERE scope_key = ?1 AND domain = ?2", params![scope_key.as_str(), domain], |row| row.get::<_, u64>(0)).optional()?.unwrap_or(0);
                if current != expected.value() { return Ok(false); }
            }
            if domain == "skills" && ready {
                self.replace_skill_source_projection_in_tx(&tx, &scope_key, source_records)?;
            } else {
                self.insert_skill_source_records_if_missing_for_workspace_in_tx(
                    &tx,
                    &scope_key,
                    source_records,
                )?;
            }
            if let Some(snapshot_json) = snapshot_json.as_deref() {
                self.write_normalized_snapshot_json_in_tx(tx, &scope_key, domain, snapshot_json)?;
            }
            if ready {
                if let Some(captured) = expected {
                    tx.execute("UPDATE projection_dirty_resources SET generation = 0 WHERE scope_key = ?1 AND domain = ?2 AND generation <= ?3", params![scope_key.as_str(), domain, captured.value()])?;
                    tx.execute("DELETE FROM projection_dirty_resources WHERE scope_key = ?1 AND domain = ?2 AND generation = 0 AND reconcile_generation = 0", params![scope_key.as_str(), domain])?;
                }
            }
        let previous_revision = tx
            .query_row(
                "SELECT revision FROM projection_heads WHERE scope_key = ?1 AND domain = ?2",
                params![scope_key.as_str(), domain],
                |row| row.get::<_, u64>(0),
            )
            .optional()?
            .unwrap_or(0);
        self.finalize_projection_domain_in_tx(&tx, domain, &scope_key, entries, ready, error)?;
        if domain == "skills" {
            crate::logging::global().info(
                "skill projection revision advanced",
                serde_json::json!({
                    "scopeKey": scope_key.as_str(),
                    "previousRevision": previous_revision,
                    "revision": previous_revision.saturating_add(1),
                    "status": if ready { "ready" } else { "failed" },
                }),
            );
        }
        Ok(true)
        })?;
        if committed && domain == "skills" && ready {
            crate::logging::global().debug(
                "skill projection persistence committed",
                serde_json::json!({
                    "operation": "save_projection_domain_snapshot",
                    "table": "scoped_skill_sources",
                    "scopeKey": scope_key.as_str(),
                    "sourceRecordCount": source_records.len(),
                }),
            );
        }
        Ok(committed)
    }

    pub fn last_scan_at(&self) -> Result<Option<u64>> {
        self.conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_scan_at'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| value.parse::<u64>().context("invalid scan timestamp"))
            .transpose()
    }

    pub fn projection_head(
        &self,
        scope_key: &ScopeKey,
        domain: &str,
    ) -> Result<Option<ProjectionHead>> {
        let row = self
            .conn
            .query_row(
                "SELECT scope_key, domain, revision, source_version, schema_version, status
                 FROM projection_heads
                 WHERE scope_key = ?1 AND domain = ?2",
                params![scope_key.as_str(), domain],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?;
        row.map(
            |(scope, domain, revision, source_version, schema_version, status)| {
                Ok(ProjectionHead {
                    scope_key: ScopeKey::new(scope).map_err(|error| anyhow::anyhow!(error))?,
                    domain,
                    revision: Revision::new(
                        u64::try_from(revision).context("invalid projection revision")?,
                    ),
                    source_version: source_version
                        .map(SourceVersion::new)
                        .transpose()
                        .map_err(|error| anyhow::anyhow!(error))?,
                    schema_version: u32::try_from(schema_version)
                        .context("invalid projection schema version")?,
                    status,
                })
            },
        )
        .transpose()
    }

    pub fn list_mcp_for_workspace(&self, workspace_root: &Path) -> Result<Option<McpScan>> {
        if self.projection_status("mcp", workspace_root)? != ProjectionStatus::Fresh {
            return Ok(None);
        }
        let scope_key = workspace_scope_key(workspace_root)?;
        self.read_normalized_snapshot(&scope_key, "mcp")
    }

    /// Mark one domain stale after an external writer changed its source.
    /// The next list call will run only that domain's scanner.
    pub fn invalidate_projection(&self, domain: &str, workspace_root: &Path) -> Result<()> {
        ensure_projection_domain(domain)?;
        self.set_projection_context(
            domain,
            &canonical_workspace_root(workspace_root),
            "stale",
            None,
        )
    }

    #[cfg(test)]
    pub fn save_agents_for_workspace(
        &self,
        workspace_root: &Path,
        scan: &crate::agents::AgentScan,
    ) -> Result<()> {
        self.save_agents_for_workspace_with_revision(workspace_root, scan, None)
            .map(|_| ())
    }

    #[cfg(test)]
    pub fn save_skills_for_workspace(&self, workspace_root: &Path, scan: &SkillScan) -> Result<()> {
        self.save_skills_for_workspace_with_revision(workspace_root, scan, None)
            .map(|_| ())
    }

    #[cfg(test)]
    pub fn save_rules_for_workspace(&self, workspace_root: &Path, scan: &RuleScan) -> Result<()> {
        self.save_rules_for_workspace_with_revision(workspace_root, scan, None)
            .map(|_| ())
    }

    pub fn list_skills_for_workspace(&self, workspace_root: &Path) -> Result<Option<SkillScan>> {
        if self.projection_status("skills", workspace_root)? != ProjectionStatus::Fresh {
            return Ok(None);
        }
        let scope_key = workspace_scope_key(workspace_root)?;
        self.read_normalized_snapshot(&scope_key, "skills")
    }

    /// Return the last cached skill projection for this workspace without checking
    /// filesystem freshness. Read-only surfaces use this while a refresh is owned
    /// by the startup or explicit refresh lifecycle.
    pub fn list_skills_cached_for_workspace(
        &self,
        workspace_root: &Path,
    ) -> Result<Option<SkillScan>> {
        let workspace_root = canonical_workspace_root(workspace_root);
        let scope_key = workspace_scope_key(&workspace_root)?;
        let context = self
            .conn
            .query_row(
                &format!(
                    "SELECT state, parser_version
                 FROM {SCOPED_PROJECTION_CONTEXT_TABLE}
                 WHERE scope_key = ?1 AND domain = 'skills'"
                ),
                params![scope_key.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((state, parser_version)) = context else {
            return Ok(None);
        };
        if !matches!(state.as_str(), "ready" | "stale" | "failed")
            || parser_version != PROJECTION_PARSER_VERSION
        {
            return Ok(None);
        }
        self.read_normalized_snapshot(&scope_key, "skills")
    }

    /// Return the cached skill projection when the explicitly selected skills
    /// are still fresh. Unselected skill files are intentionally ignored here:
    /// callers using this path only need a stable update plan for `skill_ids`.
    pub fn list_skills_for_ids_if_current(
        &self,
        workspace_root: &Path,
        skill_ids: &[String],
    ) -> Result<Option<SkillScan>> {
        let selected_ids = skill_ids.iter().cloned().collect::<BTreeSet<_>>();
        if selected_ids.is_empty() {
            return Ok(None);
        }

        let workspace_root = canonical_workspace_root(workspace_root);
        let scope_key = workspace_scope_key(&workspace_root)?;
        let context = self
            .conn
            .query_row(
                &format!(
                    "SELECT state, parser_version
                 FROM {SCOPED_PROJECTION_CONTEXT_TABLE}
                 WHERE scope_key = ?1 AND domain = 'skills'"
                ),
                params![scope_key.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((state, parser_version)) = context else {
            return Ok(None);
        };
        if state != "ready" || parser_version != PROJECTION_PARSER_VERSION {
            return Ok(None);
        }

        let entries = self.list_fs_manifest_for_domain("skills", &scope_key)?;
        if entries.is_empty() {
            return Ok(None);
        }
        if entries.iter().any(|entry| {
            entry.source_kind != "skill"
                && (entry.parser_version != PROJECTION_PARSER_VERSION
                    || !manifest_entry_is_current(entry))
        }) {
            return Ok(None);
        }

        let Some(scan) = self.read_normalized_snapshot::<SkillScan>(&scope_key, "skills")? else {
            return Ok(None);
        };
        let selected = scan
            .skills
            .iter()
            .filter(|skill| {
                selected_ids
                    .iter()
                    .any(|id| crate::skills::skill_matches_id(skill, id))
            })
            .collect::<Vec<_>>();
        if selected.len() != selected_ids.len() {
            return Ok(None);
        }

        for skill in selected {
            for path in &skill.paths {
                let skill_file = path.path.join("SKILL.md");
                let Some(entry) = entries
                    .iter()
                    .find(|entry| entry.source_kind == "skill" && entry.path == skill_file)
                else {
                    return Ok(None);
                };
                if entry.parser_version != PROJECTION_PARSER_VERSION
                    || !manifest_entry_is_current(entry)
                {
                    return Ok(None);
                }
            }
        }

        Ok(Some(scan))
    }

    pub fn list_rules_for_workspace(&self, workspace_root: &Path) -> Result<Option<RuleScan>> {
        if self.projection_status("rules", workspace_root)? != ProjectionStatus::Fresh {
            return Ok(None);
        }
        let scope_key = workspace_scope_key(workspace_root)?;
        self.read_normalized_snapshot(&scope_key, "rules")
    }

    pub fn list_hooks_for_workspace(&self, workspace_root: &Path) -> Result<Option<HookScan>> {
        if self.projection_status("hooks", workspace_root)? != ProjectionStatus::Fresh {
            return Ok(None);
        }
        let scope_key = workspace_scope_key(workspace_root)?;
        self.read_normalized_snapshot(&scope_key, "hooks")
    }

    /// Read the last persisted projection without inspecting or refreshing its
    /// source files. Callers use this on request paths; freshness is owned by
    /// the projection refresh coordinator.
    pub fn read_cached_projection<T: for<'de> Deserialize<'de>>(
        &self,
        domain: &str,
        workspace_root: &Path,
    ) -> Result<Option<T>> {
        ensure_projection_domain(domain)?;
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        self.read_normalized_snapshot(&scope_key, domain)
    }

    /// Return the publication revision and cached payload from one SQLite snapshot.
    pub fn read_cached_projection_with_revision<T: for<'de> Deserialize<'de>>(
        &self,
        domain: &str,
        workspace_root: &Path,
    ) -> Result<(Revision, Option<T>)> {
        ensure_projection_domain(domain)?;
        let scope_key = workspace_scope_key(&canonical_workspace_root(workspace_root))?;
        let (revision, payload) = self.conn.query_row(
            &format!("SELECT COALESCE(h.revision, 0), s.payload_json
                FROM (SELECT ?1 AS scope_key, ?2 AS domain) requested
                LEFT JOIN projection_heads h ON h.scope_key = requested.scope_key AND h.domain = requested.domain
                LEFT JOIN {NORMALIZED_SNAPSHOT_TABLE} s ON s.scope_key = requested.scope_key AND s.domain = requested.domain"),
            params![scope_key.as_str(), domain],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, Option<String>>(1)?)),
        )?;
        let snapshot = payload
            .map(|payload| {
                serde_json::from_str(&payload)
                    .with_context(|| format!("invalid {domain} normalized snapshot"))
            })
            .transpose()?;
        Ok((Revision::new(revision), snapshot))
    }

    pub(in crate::storage) fn read_normalized_snapshot_once<T: for<'de> Deserialize<'de>>(
        &self,
        scope_key: &ScopeKey,
        domain: &str,
    ) -> Result<Option<T>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT payload_json FROM {NORMALIZED_SNAPSHOT_TABLE}
                 WHERE scope_key = ?1 AND domain = ?2"
                ),
                params![scope_key.as_str(), domain],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|payload| {
                serde_json::from_str(&payload)
                    .with_context(|| format!("invalid {domain} normalized snapshot"))
            })
            .transpose()
    }

    pub fn normalized_snapshot_json_for_scope(
        &self,
        scope_key: &ScopeKey,
        domain: &str,
    ) -> Result<Option<String>> {
        if domain == "sessions" {
            if self.projection_head(scope_key, "sessions")?.is_none() {
                return Ok(None);
            }
            return Ok(Some(serde_json::to_string(
                &self.list_sessions_for_scope(scope_key)?,
            )?));
        }
        if domain == "session_projects" {
            return Ok(Some(serde_json::to_string(
                &self.list_session_projects_for_scope(scope_key)?,
            )?));
        }
        self.conn
            .query_row(
                &format!(
                    "SELECT payload_json FROM {NORMALIZED_SNAPSHOT_TABLE}
                     WHERE scope_key = ?1 AND domain = ?2"
                ),
                params![scope_key.as_str(), domain],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn projection_status(
        &self,
        domain: &str,
        workspace_root: &Path,
    ) -> Result<ProjectionStatus> {
        ensure_projection_domain(domain)?;
        let workspace_root = canonical_workspace_root(workspace_root);
        let scope_key = ScopeKey::new(format!("workspace:{}", workspace_root.display()))
            .map_err(|error| anyhow::anyhow!(error))?;
        let context = self
            .conn
            .query_row(
                &format!(
                    "SELECT state
                 FROM {SCOPED_PROJECTION_CONTEXT_TABLE}
                 WHERE scope_key = ?1 AND domain = ?2"
                ),
                params![scope_key.as_str(), domain],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(state) = context else {
            return Ok(ProjectionStatus::Missing);
        };
        if state == "refreshing" {
            return Ok(ProjectionStatus::Refreshing);
        }
        let snapshot_exists = self
            .conn
            .query_row(
                &format!(
                    "SELECT 1 FROM {NORMALIZED_SNAPSHOT_TABLE}
                     WHERE scope_key = ?1 AND domain = ?2"
                ),
                params![scope_key.as_str(), domain],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if state != "ready" || !snapshot_exists {
            return Ok(ProjectionStatus::Stale);
        }

        let entries = self.list_fs_manifest_for_domain(domain, &scope_key)?;
        if entries.is_empty() {
            // An empty source set is a valid, persisted projection. The
            // context and snapshot above already distinguish it from a
            // projection that has never been initialized.
            return Ok(ProjectionStatus::Fresh);
        }
        if matches!(domain, "skills" | "mcp")
            && entries
                .iter()
                .any(|entry| entry.parser_version != PROJECTION_PARSER_VERSION)
        {
            Ok(ProjectionStatus::Stale)
        } else if entries.iter().all(manifest_entry_is_current) {
            Ok(ProjectionStatus::Fresh)
        } else {
            Ok(ProjectionStatus::Stale)
        }
    }

    pub fn list_agents_for_workspace(
        &self,
        workspace_root: &Path,
    ) -> Result<Option<crate::agents::AgentScan>> {
        if self.projection_status("agents", workspace_root)? != ProjectionStatus::Fresh {
            return Ok(None);
        }
        let scope_key = workspace_scope_key(workspace_root)?;
        self.read_normalized_snapshot(&scope_key, "agents")
    }

    pub(in crate::storage) fn set_projection_context_in_tx(
        &self,
        tx: &Transaction<'_>,
        scope_key: &ScopeKey,
        domain: &str,
        state: &str,
        error: Option<String>,
    ) -> Result<()> {
        if state == "stale" {
            return self.mark_projection_resources_in_tx(tx, scope_key, domain, &[], false, false);
        }
        self.write_projection_context_in_tx(tx, scope_key, domain, state, error)
    }

    fn write_projection_context_in_tx(
        &self,
        tx: &Transaction<'_>,
        scope_key: &ScopeKey,
        domain: &str,
        state: &str,
        error: Option<String>,
    ) -> Result<()> {
        ensure_projection_domain(domain)?;
        tx.execute(
            &format!(
                "INSERT INTO {SCOPED_PROJECTION_CONTEXT_TABLE}
                (scope_key, domain, state, scanned_at, error, parser_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(scope_key, domain) DO UPDATE SET
                state = excluded.state,
                scanned_at = excluded.scanned_at,
                error = excluded.error,
                parser_version = excluded.parser_version"
            ),
            params![
                scope_key.as_str(),
                domain,
                state,
                (state == "ready").then(|| unix_now() as i64),
                error,
                PROJECTION_PARSER_VERSION,
            ],
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn write_normalized_snapshot<T: Serialize>(
        &self,
        scope_key: &ScopeKey,
        domain: &str,
        payload: &T,
    ) -> Result<()> {
        let payload_json = serde_json::to_string(payload)?;
        self.with_named_write_transaction("write_normalized_snapshot", |tx| {
            self.write_normalized_snapshot_json_in_tx(tx, scope_key, domain, &payload_json)
        })
    }

    pub(in crate::storage) fn write_normalized_snapshot_json_in_tx(
        &self,
        tx: &Transaction<'_>,
        scope_key: &ScopeKey,
        domain: &str,
        payload_json: &str,
    ) -> Result<()> {
        let revision = tx
            .query_row(
                "SELECT revision FROM projection_heads
                 WHERE scope_key = ?1 AND domain = ?2",
                params![scope_key.as_str(), domain],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(Revision::ZERO.value() as i64);
        tx.execute(
            &format!(
                "INSERT INTO {NORMALIZED_SNAPSHOT_TABLE}
                (scope_key, domain, payload_json, source_version, revision, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(scope_key, domain) DO UPDATE SET
                payload_json = excluded.payload_json,
                source_version = excluded.source_version,
                revision = excluded.revision,
                updated_at = excluded.updated_at"
            ),
            params![
                scope_key.as_str(),
                domain,
                payload_json,
                PROJECTION_PARSER_VERSION,
                revision,
                unix_now() as i64,
            ],
        )?;
        Ok(())
    }

    pub(in crate::storage) fn sync_normalized_snapshot_revision_in_tx(
        &self,
        tx: &Transaction<'_>,
        scope_key: &ScopeKey,
        domain: &str,
    ) -> Result<()> {
        let Some(revision) = tx
            .query_row(
                "SELECT revision FROM projection_heads
                 WHERE scope_key = ?1 AND domain = ?2",
                params![scope_key.as_str(), domain],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        else {
            return Ok(());
        };
        tx.execute(
            &format!(
                "UPDATE {NORMALIZED_SNAPSHOT_TABLE}
                 SET revision = ?1, updated_at = ?2
                 WHERE scope_key = ?3 AND domain = ?4"
            ),
            params![revision, unix_now() as i64, scope_key.as_str(), domain],
        )?;
        Ok(())
    }

    pub(in crate::storage) fn read_normalized_snapshot<T: for<'de> Deserialize<'de>>(
        &self,
        scope_key: &ScopeKey,
        domain: &str,
    ) -> Result<Option<T>> {
        with_database_read_lock_retry(|| self.read_normalized_snapshot_once(scope_key, domain))
    }

    pub fn delete_fs_manifest_entry(&self, source_kind: &str, path: &Path) -> Result<bool> {
        self.with_named_write_transaction("delete_fs_manifest_entry", |tx| {
            Ok(tx.execute(
                "DELETE FROM fs_manifest WHERE scope_key = ?1 AND source_kind = ?2 AND path = ?3",
                params![DEFAULT_SCOPE_KEY, source_kind, path.display().to_string()],
            )? > 0)
        })
    }

    pub fn delete_fs_manifest_missing_under_root(
        &self,
        source_kind: &str,
        root: &Path,
        seen_paths: &[PathBuf],
    ) -> Result<usize> {
        let root_key = root.display().to_string();
        let existing_paths = {
            let mut statement = self.conn.prepare(
                "SELECT path
                 FROM fs_manifest
                 WHERE scope_key = ?1 AND source_kind = ?2 AND root = ?3",
            )?;
            let rows = statement
                .query_map(params![DEFAULT_SCOPE_KEY, source_kind, root_key], |row| {
                    row.get::<_, String>(0)
                })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let seen_paths = seen_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<HashSet<_>>();

        let deleted =
            self.with_named_write_transaction("delete_fs_manifest_missing_under_root", |tx| {
                let mut deleted = 0;
                for path in existing_paths {
                    if !seen_paths.contains(&path) {
                        deleted += tx.execute(
                            "DELETE FROM fs_manifest
                     WHERE scope_key = ?1 AND source_kind = ?2 AND path = ?3 AND root = ?4",
                            params![DEFAULT_SCOPE_KEY, source_kind, path, root_key],
                        )?;
                    }
                }
                Ok(deleted)
            })?;
        Ok(deleted)
    }

    pub(in crate::storage) fn list_fs_manifest_for_domain(
        &self,
        domain: &str,
        scope_key: &ScopeKey,
    ) -> Result<Vec<FsManifestEntry>> {
        let kinds = manifest_source_kinds(domain)?;
        let placeholders = std::iter::repeat_n("?", kinds.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT source_kind, path, root, agent, scope, mtime_ns, size, inode, device,
                    sha256, parser_version, last_seen_at, parse_status, resource_path
             FROM fs_manifest
             WHERE scope_key = ?1
               AND source_kind IN ({placeholders})
             ORDER BY source_kind ASC, path ASC"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let mut values = vec![SqlValue::Text(scope_key.as_str().to_string())];
        values.extend(kinds.iter().map(|kind| SqlValue::Text((*kind).to_string())));
        let rows = statement.query_map(params_from_iter(values.iter()), fs_manifest_from_row)?;
        let entries = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    pub(in crate::storage) fn finalize_projection_domain_in_tx(
        &self,
        tx: &Transaction<'_>,
        domain: &str,
        scope_key: &ScopeKey,
        entries: &[FsManifestEntry],
        ready: bool,
        error: Option<String>,
    ) -> Result<()> {
        let kinds = manifest_source_kinds(domain)?;
        let delete_sql = format!(
            "DELETE FROM fs_manifest
             WHERE scope_key = ?1 AND source_kind IN ({})",
            std::iter::repeat_n("?", kinds.len())
                .enumerate()
                .map(|(index, _)| format!("?{}", index + 2))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut delete_params = vec![SqlValue::Text(scope_key.as_str().to_string())];
        delete_params.extend(kinds.iter().map(|kind| SqlValue::Text((*kind).to_string())));
        tx.execute(&delete_sql, params_from_iter(delete_params.iter()))?;
        for entry in entries {
            tx.execute(
                "INSERT INTO fs_manifest (
                    scope_key, source_kind, path, root, agent, scope, mtime_ns, size, inode, device,
                    sha256, parser_version, last_seen_at, parse_status, resource_path
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                 ON CONFLICT(scope_key, source_kind, path) DO UPDATE SET
                    root = excluded.root,
                    agent = excluded.agent,
                    scope = excluded.scope,
                    mtime_ns = excluded.mtime_ns,
                    size = excluded.size,
                    inode = excluded.inode,
                    device = excluded.device,
                    sha256 = excluded.sha256,
                    parser_version = excluded.parser_version,
                    last_seen_at = excluded.last_seen_at,
                    parse_status = excluded.parse_status,
                    resource_path = excluded.resource_path",
                params![
                    scope_key.as_str(),
                    entry.source_kind,
                    entry.path.display().to_string(),
                    entry.root.display().to_string(),
                    entry.agent,
                    entry.scope,
                    entry.mtime_ns,
                    entry.size,
                    entry.inode,
                    entry.device,
                    entry.sha256,
                    entry.parser_version,
                    entry.last_seen_at,
                    entry.parse_status,
                    entry
                        .resource_path
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned()),
                ],
            )?;
        }
        self.set_projection_context_in_tx(
            tx,
            &scope_key,
            domain,
            if ready { "ready" } else { "failed" },
            error,
        )?;
        let source_version = SourceVersion::new(PROJECTION_PARSER_VERSION)
            .map_err(|error| anyhow::anyhow!(error))?;
        advance_projection_head_in_tx(
            tx,
            &scope_key,
            domain,
            Some(&source_version),
            if ready { "ready" } else { "failed" },
        )?;
        self.sync_normalized_snapshot_revision_in_tx(tx, &scope_key, domain)?;
        Ok(())
    }

    pub(in crate::storage) fn set_projection_context(
        &self,
        domain: &str,
        workspace_root: &Path,
        state: &str,
        error: Option<String>,
    ) -> Result<()> {
        ensure_projection_domain(domain)?;
        let workspace_root = canonical_workspace_root(workspace_root);
        let scope_key = ScopeKey::new(format!("workspace:{}", workspace_root.display()))
            .map_err(|error| anyhow::anyhow!(error))?;
        self.with_named_write_transaction("set_projection_context", |tx| {
            self.set_projection_context_in_tx(&tx, &scope_key, domain, state, error)?;
            Ok(())
        })?;
        Ok(())
    }

    /// Persist a complete scan and bind all filesystem-backed projections to
    /// one canonical workspace context.
    #[cfg(test)]
    pub fn save_scan_for_workspace(
        &self,
        workspace_root: &Path,
        report: &ScanReport,
    ) -> Result<()> {
        self.save_scan_for_workspace_with_revisions(workspace_root, report, None)
            .map(|_| ())
    }

    pub fn save_scan_for_workspace_if_revisions(
        &self,
        workspace_root: &Path,
        report: &ScanReport,
        expected: &BTreeMap<String, Revision>,
    ) -> Result<bool> {
        self.save_scan_for_workspace_with_revisions(workspace_root, report, Some(expected))
    }

    fn save_scan_for_workspace_with_revisions(
        &self,
        workspace_root: &Path,
        report: &ScanReport,
        expected: Option<&BTreeMap<String, Revision>>,
    ) -> Result<bool> {
        const DOMAINS: [&str; 6] = ["agents", "skills", "rules", "hooks", "mcp", "sessions"];
        if let Some(expected) = expected {
            anyhow::ensure!(
                DOMAINS.iter().all(|domain| expected.contains_key(*domain)),
                "aggregate projection publication requires all six captured revisions"
            );
        }
        let workspace_root = canonical_workspace_root(workspace_root);
        let scope_key = workspace_scope_key(&workspace_root)?;
        let skill_source_records = skill_source_records_from_scan(&report.skills);
        let sources = self.prepare_session_sources(&scope_key, &report.sessions.sessions)?;
        let snapshots = [
            (
                "agents",
                &report.agents.warnings,
                serde_json::to_string(&report.agents)?,
            ),
            (
                "skills",
                &report.skills.warnings,
                serde_json::to_string(&report.skills)?,
            ),
            (
                "rules",
                &report.rules.warnings,
                serde_json::to_string(&report.rules)?,
            ),
            (
                "hooks",
                &report.hooks.warnings,
                serde_json::to_string(&report.hooks)?,
            ),
            (
                "mcp",
                &report.mcp.warnings,
                serde_json::to_string(&report.mcp)?,
            ),
        ];
        let prepared_projections = [
            (
                "agents",
                report.agents.warnings.is_empty(),
                report.agents.warnings.join("; "),
            ),
            (
                "skills",
                report.skills.warnings.is_empty(),
                report.skills.warnings.join("; "),
            ),
            (
                "rules",
                report.rules.warnings.is_empty(),
                report.rules.warnings.join("; "),
            ),
            (
                "hooks",
                report.hooks.warnings.is_empty(),
                report.hooks.warnings.join("; "),
            ),
            (
                "mcp",
                report.mcp.warnings.is_empty(),
                report.mcp.warnings.join("; "),
            ),
        ]
        .into_iter()
        .map(|(domain, ready, error)| {
            let entries = match domain {
                "agents" => manifest_entries_for_agents(&report.agents, &workspace_root),
                "skills" => manifest_entries_for_skills(&report.skills, &workspace_root),
                "rules" => manifest_entries_for_rules(&report.rules, &workspace_root),
                "hooks" => manifest_entries_for_hooks(&report.hooks, &workspace_root),
                "mcp" => manifest_entries_for_mcp(&report.mcp, &workspace_root),
                _ => unreachable!("projection domain is validated by the match above"),
            };
            (domain, ready, error, entries)
        })
        .collect::<Vec<_>>();
        self.with_named_write_transaction("save_scan_for_workspace", |tx| {
            if let Some(expected) = expected {
                for domain in DOMAINS {
                    let current = tx.query_row("SELECT revision FROM projection_heads WHERE scope_key = ?1 AND domain = ?2", params![scope_key.as_str(), domain], |row| row.get::<_, u64>(0)).optional()?.unwrap_or(0);
                    if current != expected[domain].value() { return Ok(false); }
                }
            }
            if report.skills.warnings.is_empty() {
                self.replace_skill_source_projection_in_tx(&tx, &scope_key, &skill_source_records)?;
            }
            for (domain, warnings, json) in &snapshots {
                if warnings.is_empty() {
                    self.write_normalized_snapshot_json_in_tx(tx, &scope_key, domain, json)?;
                }
            }
            if report.sessions.warnings.is_empty() {
                self.save_sessions_at_with_scope_in_tx(
                    &tx,
                    &report.sessions,
                    unix_now(),
                    &scope_key,
                    &sources,
                )?;
            }
            for (domain, ready, error, entries) in prepared_projections {
                if ready {
                    if let Some(expected) = expected {
                        tx.execute("UPDATE projection_dirty_resources SET generation = 0 WHERE scope_key = ?1 AND domain = ?2 AND generation <= ?3", params![scope_key.as_str(), domain, expected[domain].value()])?;
                        tx.execute("DELETE FROM projection_dirty_resources WHERE scope_key = ?1 AND domain = ?2 AND generation = 0 AND reconcile_generation = 0", params![scope_key.as_str(), domain])?;
                    }
                }
                self.finalize_projection_domain_in_tx(
                    &tx,
                    domain,
                    &scope_key,
                    &entries,
                    ready,
                    (!ready).then_some(error),
                )?;
            }
            Ok(true)
        })
    }

    pub fn upsert_fs_manifest(&self, entry: &FsManifestEntry) -> Result<()> {
        self.upsert_fs_manifest_entries(std::slice::from_ref(entry))?;
        Ok(())
    }

    pub fn upsert_fs_manifest_entries(&self, entries: &[FsManifestEntry]) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }

        self.with_named_write_transaction("upsert_fs_manifest_entries", |tx| {
            for entry in entries {
                tx.execute(
                    "INSERT INTO fs_manifest (
                    scope_key, source_kind, path, root, agent, scope, mtime_ns, size, inode, device,
                    sha256, parser_version, last_seen_at, parse_status, resource_path
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                 ON CONFLICT(scope_key, source_kind, path) DO UPDATE SET
                    root = excluded.root,
                    agent = excluded.agent,
                    scope = excluded.scope,
                    mtime_ns = excluded.mtime_ns,
                    size = excluded.size,
                    inode = excluded.inode,
                    device = excluded.device,
                    sha256 = excluded.sha256,
                    parser_version = excluded.parser_version,
                    last_seen_at = excluded.last_seen_at,
                    parse_status = excluded.parse_status,
                    resource_path = excluded.resource_path",
                    params![
                        DEFAULT_SCOPE_KEY,
                        entry.source_kind,
                        entry.path.display().to_string(),
                        entry.root.display().to_string(),
                        entry.agent,
                        entry.scope,
                        entry.mtime_ns,
                        entry.size,
                        entry.inode,
                        entry.device,
                        entry.sha256,
                        entry.parser_version,
                        entry.last_seen_at,
                        entry.parse_status,
                        entry
                            .resource_path
                            .as_ref()
                            .map(|path| path.to_string_lossy().into_owned()),
                    ],
                )?;
            }
            Ok(())
        })?;
        Ok(entries.len())
    }

    pub fn fs_manifest_entry(
        &self,
        source_kind: &str,
        path: &Path,
    ) -> Result<Option<FsManifestEntry>> {
        self.conn
            .query_row(
                "SELECT source_kind, path, root, agent, scope, mtime_ns, size, inode, device,
                        sha256, parser_version, last_seen_at, parse_status, resource_path
                 FROM fs_manifest
                 WHERE scope_key = ?1 AND source_kind = ?2 AND path = ?3",
                params![DEFAULT_SCOPE_KEY, source_kind, path.display().to_string()],
                fs_manifest_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_fs_manifest_for_root(&self, root: &Path) -> Result<Vec<FsManifestEntry>> {
        let scope_key = workspace_scope_key(&workspace_root_for_manifest_root(root))?;
        let mut statement = self.conn.prepare(
            "SELECT source_kind, path, root, agent, scope, mtime_ns, size, inode, device,
                    sha256, parser_version, last_seen_at, parse_status, resource_path
             FROM fs_manifest
             WHERE scope_key = ?1 AND root = ?2
             ORDER BY source_kind ASC, path ASC",
        )?;
        let rows = statement.query_map(
            params![scope_key.as_str(), root.display().to_string()],
            fs_manifest_from_row,
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}
