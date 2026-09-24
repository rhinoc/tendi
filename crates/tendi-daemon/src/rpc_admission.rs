//! Request preparation declares resources before execution is admitted.
use super::*;
use request_scheduler::{Step, Workload};
use std::time::Instant;
use tendi_core::coordination::ResourceRequest;

type RpcResult = Result<Value, DaemonError>;
pub(super) type PreparedExecute = Box<dyn FnOnce() -> RpcResult + Send>;
type Execute = Box<dyn FnOnce(Option<PreparedExecute>) -> RpcResult + Send>;

fn skills_projection_resource(store: &tendi_core::storage::Store) -> ResourceRequest {
    ResourceRequest::named(
        store.path(),
        tendi_core::coordination::shared_projection_key("skills"),
    )
}

fn skills_projection_scope_resource(
    store: &tendi_core::storage::Store,
    workspace: &Path,
) -> ResourceRequest {
    ResourceRequest::named(
        store.path(),
        tendi_core::coordination::projection_key("skills", workspace),
    )
}

fn skill_projection_writer(method: &str) -> bool {
    matches!(
        method,
        "scan"
            | "agent_config_save"
            | "agent_configs_delete_many"
            | "config_profile_create"
            | "config_profile_set"
            | "skill_file_save"
            | "skill_file_create"
            | "skill_folder_create"
            | "skill_path_rename"
            | "skill_path_delete"
            | "skills_refresh"
            | "skills_add"
            | "skills_update"
            | "skills_update_many"
            | "skills_delete_many"
            | "skills_remove_locations"
            | "skills_set"
            | "skills_wrap"
            | "skills_distribute"
            | "bundled_skill_install"
            | "bundled_skill_remove"
            | "skills_backup_restore"
            | "skills_backup_adopt"
            | "skills_backup_adopt_many"
    )
}

pub(super) struct Cleanup(pub Option<Box<dyn FnOnce() + Send>>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(cleanup) = self.0.take() {
            cleanup();
        }
    }
}

impl Daemon {
    pub(super) fn reconciliation_step_with_cleanup(
        &self,
        workspace: PathBuf,
        cleanup: Cleanup,
    ) -> Step<()> {
        keep_cleanup(self.reconciliation_step(workspace), cleanup)
    }
    pub(super) fn reconciliation_step(&self, workspace: PathBuf) -> Step<()> {
        let daemon = self.clone();
        Step::acquire(Workload::Compute, Vec::new(), move || {
            let started = Instant::now();
            let store = daemon.open_store()?;
            let receipt = store.read_projection_refresh_state::<tendi_core::skills::SkillScan>(
                "skills", &workspace,
            )?;
            tendi_core::logging::global().info(
                "skill reconciliation stage inspected",
                serde_json::json!({
                    "workspace": workspace,
                    "revision": receipt.revision,
                    "fullRefresh": receipt.full_refresh,
                    "resourceCount": receipt.resources.len(),
                    "reconcileFull": receipt.reconcile_full,
                    "reconcileResourceCount": receipt.reconcile_resources.len(),
                    "durationMs": started.elapsed().as_secs_f64() * 1000.0,
                }),
            );
            if receipt.full_refresh || !receipt.resources.is_empty() {
                let resources = vec![skills_projection_scope_resource(&store, &workspace)];
                return Ok(Step::acquire(Workload::Compute, resources, move || {
                    let started = Instant::now();
                    let store = daemon.open_store()?;
                    let state = store
                        .read_projection_refresh_state::<tendi_core::skills::SkillScan>(
                            "skills", &workspace,
                        )?;
                    let roots = Daemon::registered_project_roots(&store)?;
                    let scan = match state.snapshot {
                        Some(cached) => tendi_core::skills::refresh_dirty_skill_projection(
                            &workspace,
                            &store,
                            cached,
                            &state.resources,
                            state.full_refresh,
                            &roots,
                        )?,
                        None => tendi_core::skills::scan_skills_for_project_roots_with_store(
                            &workspace, &store, &roots,
                        )?,
                    };
                    store.save_skills_for_workspace_if_revision(
                        &workspace,
                        &scan,
                        state.revision,
                    )?;
                    tendi_core::logging::global().info(
                        "skill reconciliation projection refreshed",
                        serde_json::json!({
                            "workspace": workspace,
                            "revision": state.revision,
                            "durationMs": started.elapsed().as_secs_f64() * 1000.0,
                        }),
                    );
                    Ok(daemon.reconciliation_step(workspace))
                }));
            }
            if !receipt.reconcile_full && receipt.reconcile_resources.is_empty() {
                daemon.clear_skill_reconciliation_backoff(&workspace);
                return Ok(Step::Complete(()));
            }
            let scan = receipt
                .snapshot
                .ok_or_else(|| anyhow::anyhow!("reconciliation requires a prepared snapshot"))?;
            let paths = tendi_core::skills::skill_reconciliation_resource_paths(
                &workspace,
                &store,
                &scan,
                &receipt.reconcile_resources,
                receipt.reconcile_full,
            )?;
            let resources = vec![
                skills_projection_scope_resource(&store, &workspace),
                ResourceRequest::files(paths.clone())?,
            ];
            Ok(Step::acquire(Workload::Compute, resources, move || {
                let started = Instant::now();
                let store = daemon.open_store()?;
                let current = store
                    .read_projection_refresh_state::<tendi_core::skills::SkillScan>(
                        "skills", &workspace,
                    )?;
                if current.revision != receipt.revision {
                    return Ok(daemon.reconciliation_step(workspace));
                }
                tendi_core::skills::reconcile_dirty_skill_resources(
                    &workspace,
                    &store,
                    scan,
                    &receipt.reconcile_resources,
                    receipt.reconcile_full,
                )?;
                // The core reconciliation commits one projection invalidation
                // only when it materializes a file change. A no-op must not
                // advance the CAS revision before its maintenance receipt is
                // acknowledged.
                store.acknowledge_projection_reconciliation(
                    "skills",
                    &workspace,
                    receipt.revision,
                )?;
                daemon.clear_skill_reconciliation_backoff(&workspace);
                tendi_core::logging::global().info(
                    "skill reconciliation completed",
                    serde_json::json!({
                        "workspace": workspace,
                        "revision": receipt.revision,
                        "durationMs": started.elapsed().as_secs_f64() * 1000.0,
                    }),
                );
                Ok(daemon.reconciliation_step(workspace))
            }))
        })
    }
    pub(super) fn prepare_rpc_step(
        &self,
        method: String,
        params: Value,
        workload: Workload,
        execute: Execute,
    ) -> Step<RpcResult> {
        if matches!(
            method.as_str(),
            "session_transcript" | "session_transcript_locator" | "session_transcript_search"
        ) {
            return Step::acquire(workload, Vec::new(), move || {
                Ok(Step::Complete(execute(None)))
            });
        }
        if matches!(
            method.as_str(),
            "settings_save"
                | "prompt_save"
                | "prompts_delete_many"
                | "project_scan_scopes_save"
                | "skills_backup_disconnect"
                | "skills_backup_sync"
        ) {
            return Step::acquire(workload, Vec::new(), move || {
                Ok(Step::Complete(execute(None)))
            });
        }
        let daemon = self.clone();
        Step::acquire(Workload::Prepare, Vec::new(), move || {
            let store = daemon.open_store()?;
            for domain in projection_dependencies(&method) {
                if store.projection_status(domain, &daemon.state.cwd)?
                    != tendi_core::storage::ProjectionStatus::Fresh
                {
                    let resources = vec![if *domain == "skills" {
                        skills_projection_resource(&store)
                    } else {
                        ResourceRequest::named(
                            store.path(),
                            tendi_core::coordination::projection_key(domain, &daemon.state.cwd),
                        )
                    }];
                    return Ok(Step::acquire(
                        Workload::Compute,
                        resources,
                        move || match daemon.refresh_projection_domain(domain) {
                            Ok(_) => Ok(daemon.prepare_rpc_step(method, params, workload, execute)),
                            Err(error) => Ok(Step::Complete(Err(error))),
                        },
                    ));
                }
            }
            if method == "mcp_probe" {
                return Ok(daemon.mcp_probe_step(params, execute));
            }
            if method == "skills_update" {
                let request: runtime_schema::SkillsUpdateRequest =
                    serde_json::from_value(params)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                let scan = daemon
                    .cached_skills(&store)
                    .map_err(|error| anyhow::anyhow!(error.message))?;
                let ids = match &request.pattern {
                    Some(pattern) => tendi_core::skills::skill_ids_matching_pattern(&scan, pattern),
                    None => request.skill_ids.clone().unwrap_or_default(),
                };
                let paths =
                    tendi_core::skills::skill_update_preparation_resource_paths(&scan, &ids)?;
                let projection = skills_projection_resource(&store);
                return Ok(Step::acquire(
                    Workload::ExternalIo,
                    vec![projection.clone(), ResourceRequest::files(paths)?],
                    move || {
                        let store = daemon.open_store()?;
                        let plan_started = Instant::now();
                        tendi_core::logging::global().info(
                            "skill update plan started",
                            serde_json::json!({
                                "skillCount": ids.len(),
                                "dryRun": request.dry_run.unwrap_or(false),
                            }),
                        );
                        let plan = match tendi_core::skills::plan_skill_updates_many_for_scan_in_workspace_with_store(
                            &scan, &ids, &daemon.state.cwd, &store,
                        ) {
                            Ok(plan) => plan,
                            Err(error) => {
                                tendi_core::logging::global().error(
                                    "skill update plan failed",
                                    serde_json::json!({
                                        "skillCount": ids.len(),
                                        "durationMs": plan_started.elapsed().as_secs_f64() * 1000.0,
                                        "error": error.to_string(),
                                    }),
                                );
                                return Err(error);
                            }
                        };
                        tendi_core::logging::global().info(
                            "skill update plan completed",
                            serde_json::json!({
                                "skillCount": ids.len(),
                                "durationMs": plan_started.elapsed().as_secs_f64() * 1000.0,
                                "canApply": plan.can_apply(),
                                "fileChangeCount": plan.file_changes.changes.len(),
                                "gitUpdateCount": plan.git_updates.len(),
                                "mergeIssueCount": plan.merge_issues.len(),
                            }),
                        );
                        let resources = if request.dry_run.unwrap_or(false) {
                            vec![projection]
                        } else {
                            vec![
                                projection,
                                ResourceRequest::files(
                                    tendi_core::skills::skill_update_resource_paths(&plan)?,
                                )?,
                            ]
                        };
                        Ok(Step::acquire(Workload::Compute, resources, move || {
                            Ok(Step::Complete(execute(Some(Box::new(move || {
                                daemon.finish_skill_update(request, scan, plan).and_then(
                                    |response| {
                                        serde_json::to_value(response).map_err(internal_error)
                                    },
                                )
                            })))))
                        }))
                    },
                ));
            }
            let resources = match daemon.rpc_resources(&store, &method, &params) {
                Ok(resources) => resources,
                Err(error) => return Ok(Step::Complete(Err(error))),
            };
            let expected_resources = resources.clone();
            Ok(Step::acquire(workload, resources, move || {
                let store = daemon.open_store()?;
                // Waiting can change selected installations or provider settings.
                // Reprepare only the unconsumed command, never an executed mutation.
                let current = match daemon.rpc_resources(&store, &method, &params) {
                    Ok(resources) => resources,
                    Err(error) => {
                        return Ok(Step::Complete(execute(Some(Box::new(move || Err(error))))));
                    }
                };
                if current != expected_resources {
                    return Ok(daemon.prepare_rpc_step(method, params, workload, execute));
                }
                Ok(Step::Complete(execute(None)))
            }))
        })
    }

    fn mcp_probe_step(&self, params: Value, execute: Execute) -> Step<RpcResult> {
        let daemon = self.clone();
        Step::acquire(Workload::ExternalIo, Vec::new(), move || {
            let prepared = (|| -> Result<_, DaemonError> {
                let request: runtime_schema::McpProbeRequest = serde_json::from_value(params)
                    .map_err(|error| invalid_argument(error.to_string()))?;
                let store = daemon.open_store().map_err(core_error)?;
                let scan: tendi_core::mcp::McpScan = store
                    .read_cached_projection("mcp", &daemon.state.cwd)
                    .map_err(core_error)?
                    .ok_or_else(|| conflict_error("MCP projection unavailable"))?;
                let current = mcp_server_for_id(&scan.servers, &request.id)?.clone();
                let expected = (current.trust_hash.clone(), current.enabled);
                let updated =
                    tendi_core::mcp::probe_server(mcp_probe_request_for_record(&current), current)
                        .map_err(core_error)?;
                Ok((request, expected, updated))
            })();
            let (request, (hash, enabled), updated) = match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    return Ok(Step::Complete(execute(Some(Box::new(move || Err(error))))));
                }
            };
            let resources = vec![ResourceRequest::files(vec![updated.path.clone()])?];
            Ok(Step::acquire(Workload::Interactive, resources, move || {
                Ok(Step::Complete(execute(Some(Box::new(move || {
                    daemon
                        .publish_mcp_probe(request, hash, enabled, updated)
                        .and_then(|response| serde_json::to_value(response).map_err(internal_error))
                })))))
            }))
        })
    }

    fn rpc_resources(
        &self,
        store: &tendi_core::storage::Store,
        method: &str,
        params: &Value,
    ) -> Result<Vec<ResourceRequest>, DaemonError> {
        let mut paths = Vec::<PathBuf>::new();
        let text = |key: &str| -> Result<&str, DaemonError> {
            params
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| invalid_argument(format!("missing argument: {key}")))
        };
        let strings = |key: &str| -> Vec<String> {
            params
                .get(key)
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        let dry = params
            .get("dryRun")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        match method {
            "projects_scan" => {
                return Ok(vec![ResourceRequest::named(
                    store.path(),
                    "project-catalog",
                )]);
            }
            "agent_config_save" | "rule_file_save" => paths.push(PathBuf::from(text("path")?)),
            "agent_configs_delete_many" | "rule_file_delete_many" => {
                paths.extend(strings("paths").into_iter().map(PathBuf::from))
            }
            "config_profile_create" | "config_profile_set" => {
                let agent: runtime_schema::AgentKind =
                    serde_json::from_value(params.get("agent").cloned().unwrap_or(Value::Null))
                        .map_err(|error| invalid_argument(error.to_string()))?;
                let name = if method == "config_profile_create" {
                    Some(text("name")?)
                } else {
                    params.get("profile").and_then(Value::as_str)
                };
                if let Some(name) = name {
                    paths.push(
                        tendi_core::config::config_profile_path(
                            agent_kind_from_request(agent),
                            name,
                        )
                        .map_err(core_error)?,
                    );
                }
            }
            "hook_delete"
            | "hook_delete_many"
            | "hook_set_enabled"
            | "hook_set_enabled_many"
            | "hook_review" => {
                let scan: tendi_core::hooks::HookScan = store
                    .read_cached_projection("hooks", &self.state.cwd)
                    .map_err(core_error)?
                    .ok_or_else(|| conflict_error("hooks projection unavailable"))?;
                let ids = if method.ends_with("_many") {
                    if method == "hook_set_enabled_many" {
                        params
                            .get("requests")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|request| request.get("id").and_then(Value::as_str))
                            .map(str::to_owned)
                            .collect()
                    } else {
                        strings("ids")
                    }
                } else {
                    vec![text("id")?.to_string()]
                };
                for id in ids {
                    let record = hook_for_id(&scan.hooks, &id)?;
                    if method == "hook_review" {
                        paths.extend(
                            tendi_core::hooks::hook_review_resource_paths(
                                record.agent,
                                &record.path,
                            )
                            .map_err(core_error)?,
                        );
                    } else {
                        paths.push(record.path.clone());
                    }
                }
            }
            "mcp_set_enabled" | "mcp_set_enabled_many" => {
                let scan: tendi_core::mcp::McpScan = store
                    .read_cached_projection("mcp", &self.state.cwd)
                    .map_err(core_error)?
                    .ok_or_else(|| conflict_error("MCP projection unavailable"))?;
                let ids = if method.ends_with("_many") {
                    params
                        .get("requests")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|request| request.get("id").and_then(Value::as_str))
                        .map(str::to_owned)
                        .collect()
                } else {
                    vec![text("id")?.to_string()]
                };
                for id in ids {
                    paths.push(mcp_server_for_id(&scan.servers, &id)?.path.clone());
                }
            }
            "skill_file_save"
            | "skill_file_create"
            | "skill_folder_create"
            | "skill_path_rename"
            | "skill_path_delete" => {
                let scan = self.cached_skills(store)?;
                let (_, path) = self.skill_context_for_id(
                    params.get("locationId").and_then(Value::as_str),
                    text("skillId")?,
                    &scan,
                )?;
                paths.push(path);
            }
            "skills_add" if !dry => {
                let previews = self
                    .state
                    .add_preview
                    .lock()
                    .map_err(|_| internal_error("preview store unavailable"))?;
                let preview = previews
                    .get(text("previewId")?)
                    .ok_or_else(|| conflict_error("skill add preview expired"))?;
                paths.extend(
                    tendi_core::skills::skill_add_resource_paths(&preview.plan)
                        .map_err(core_error)?,
                );
                if params
                    .get("source")
                    .and_then(Value::as_str)
                    .is_some_and(|source| {
                        source.trim() == tendi_core::bundled_skill::INSTALL_SOURCE
                    })
                {
                    for operation in [
                        tendi_core::bundled_skill::BundledSkillOperation::PrepareSource,
                        tendi_core::bundled_skill::BundledSkillOperation::DismissPrompt,
                    ] {
                        paths.extend(
                            tendi_core::bundled_skill::operation_resource_paths(operation)
                                .map_err(core_error)?,
                        );
                    }
                }
            }
            "skills_add" => {
                let request: runtime_schema::SkillsAddRequest =
                    serde_json::from_value(params.clone())
                        .map_err(|error| invalid_argument(error.to_string()))?;
                let options = self.skill_add_options(&request)?;
                paths.extend(
                    tendi_core::skills::skill_add_preparation_resource_paths(
                        &self.state.cwd,
                        &options,
                    )
                    .map_err(core_error)?,
                );
                if request.source.trim() == tendi_core::bundled_skill::INSTALL_SOURCE {
                    paths.extend(
                        tendi_core::bundled_skill::operation_resource_paths(
                            tendi_core::bundled_skill::BundledSkillOperation::PrepareSource,
                        )
                        .map_err(core_error)?,
                    );
                }
            }
            "skills_update_many" if !dry => {
                if let Some(preview_id) = params
                    .get("previewId")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    let previews = self
                        .state
                        .update_preview
                        .lock()
                        .map_err(|_| internal_error("preview store unavailable"))?;
                    let preview = previews.get(preview_id).ok_or_else(|| {
                        conflict_error("skill update preview expired; preview the update again")
                    })?;
                    paths.extend(
                        tendi_core::skills::skill_update_resource_paths(&preview.plan)
                            .map_err(core_error)?,
                    );
                }
            }
            "skills_update_many" => {
                let scan = self.cached_skills(store)?;
                paths.extend(
                    tendi_core::skills::skill_update_preparation_resource_paths(
                        &scan,
                        &strings("skillIds"),
                    )
                    .map_err(core_error)?,
                );
            }
            "skills_delete_many" | "skills_remove_locations" => {
                let ids = strings("skillIds");
                let scan = self.skill_projection_for_ids(&ids)?;
                if method == "skills_delete_many" {
                    let plan = tendi_core::skills::plan_skill_delete_many_for_scan(&scan, &ids)
                        .map_err(core_error)?;
                    paths.extend(tendi_core::skills::skill_delete_resource_paths(&plan));
                } else {
                    paths.extend(
                        scan.skills
                            .iter()
                            .filter(|skill| {
                                ids.iter()
                                    .any(|id| tendi_core::skills::skill_matches_id(skill, id))
                            })
                            .flat_map(|skill| skill.paths.iter().map(|path| path.path.clone())),
                    );
                }
            }
            "skills_set" | "skills_wrap" if !dry => {
                let mut scan = self.cached_skills(store)?;
                let ids = if let Some(pattern) = params.get("pattern").and_then(Value::as_str) {
                    tendi_core::skills::skill_ids_matching_pattern(&scan, pattern)
                } else {
                    strings("skillIds")
                };
                paths.extend(
                    scan.skills
                        .iter()
                        .filter(|skill| {
                            ids.iter()
                                .any(|id| tendi_core::skills::skill_matches_id(skill, id))
                        })
                        .flat_map(|skill| skill.paths.iter().map(|path| path.path.clone())),
                );
                let changes = if method == "skills_set" {
                    let visibility: runtime_schema::SkillVisibility = serde_json::from_value(
                        params.get("visibility").cloned().unwrap_or(Value::Null),
                    )
                    .map_err(|error| invalid_argument(error.to_string()))?;
                    tendi_core::skills::plan_visibility_many_for_scan(
                        &scan,
                        &ids,
                        skill_visibility_from_request(visibility),
                    )
                } else {
                    let shared = tendi_core::skill_targets::skill_target_root(
                        &self.state.cwd,
                        &"shared".parse().map_err(core_error)?,
                        tendi_core::SkillInstallScope::Global,
                    )
                    .map_err(core_error)?;
                    if !scan.roots.iter().any(|root| root.path == shared) {
                        scan.roots.push(tendi_core::skills::SkillRoot {
                            path: shared,
                            scope: "global".into(),
                            agent: tendi_core::AgentKind::Shared,
                            plugin_id: None,
                            plugin_enabled: None,
                        });
                    }
                    let manual = params
                        .get("manualChildren")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if params
                        .get("refresh")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        tendi_core::skills::refresh_wrapper_for_ids(
                            &scan,
                            text("name")?,
                            &ids,
                            manual,
                        )
                    } else {
                        tendi_core::skills::plan_wrapper_for_ids(
                            &scan,
                            text("name")?,
                            &ids,
                            params.get("description").and_then(Value::as_str),
                            manual,
                        )
                    }
                }
                .map_err(core_error)?;
                paths.extend(tendi_core::skills::changeset_resource_paths(&changes));
            }
            "skills_distribute" if !dry => {
                if let Some(id) = params.get("previewId").and_then(Value::as_str) {
                    let previews = self
                        .state
                        .distribution_preview
                        .lock()
                        .map_err(|_| internal_error("preview store unavailable"))?;
                    let preview = previews
                        .get(id)
                        .ok_or_else(|| conflict_error("skill distribution preview expired"))?;
                    paths.extend(
                        preview
                            .plans
                            .iter()
                            .flat_map(tendi_core::skills::skill_distribution_resource_paths),
                    );
                } else {
                    let request: runtime_schema::SkillsDistributeRequest =
                        serde_json::from_value(params.clone())
                            .map_err(|error| invalid_argument(error.to_string()))?;
                    let scan = self.cached_skills(store)?;
                    let scope = request.scope.parse().map_err(core_error)?;
                    let mode = request.mode.parse().map_err(core_error)?;
                    for target in distribution_targets(&request)? {
                        for source in &request.source_paths {
                            let plan = tendi_core::skills::plan_skill_distribution_for_scan(
                                &self.state.cwd,
                                &scan,
                                Path::new(source),
                                &target,
                                scope,
                                mode,
                            )
                            .map_err(core_error)?;
                            paths.extend(tendi_core::skills::skill_distribution_resource_paths(
                                &plan,
                            ));
                        }
                    }
                }
            }
            "skills_backup_configure" => {
                let request: runtime_schema::SkillsBackupConfigureRequest =
                    serde_json::from_value(params.clone())
                        .map_err(|error| invalid_argument(error.to_string()))?;
                let repository = PathBuf::from(&request.repository);
                let checkout = if !repository.exists()
                    && tendi_core::skill_backup::is_remote_repository(&request.repository)
                {
                    request
                        .checkout_path
                        .filter(|path| !path.trim().is_empty())
                        .map(PathBuf::from)
                        .unwrap_or(
                            tendi_core::skill_backup::default_checkout_path()
                                .map_err(core_error)?,
                        )
                } else {
                    tendi_core::skill_backup::discover_git_repository_root(&repository)
                        .map_err(core_error)?
                        .unwrap_or(repository)
                };
                paths.extend(
                    tendi_core::skill_backup::checkout_mutation_resource_paths(&checkout)
                        .map_err(core_error)?,
                );
            }
            "skills_backup_restore" => {
                let request: runtime_schema::SkillsBackupRestoreRequest =
                    serde_json::from_value(params.clone())
                        .map_err(|error| invalid_argument(error.to_string()))?;
                let plan = tendi_core::skill_backup::plan_backup_restore(
                    store,
                    &self.state.cwd,
                    &request.revision,
                    &request.skill_ids.unwrap_or_default(),
                    &request.target.parse().map_err(core_error)?,
                    request.scope.parse().map_err(core_error)?,
                )
                .map_err(core_error)?;
                paths.extend(tendi_core::skill_backup::backup_restore_resource_paths(
                    &plan,
                ));
            }
            "skills_backup_now" | "skills_backup_versions" | "skills_backup_status" => {
                if let Some(config) = store.skill_backup_config().map_err(core_error)? {
                    paths.extend(
                        tendi_core::skill_backup::checkout_mutation_resource_paths(
                            &config.checkout_path,
                        )
                        .map_err(core_error)?,
                    );
                }
            }
            "skills_backup_adopt" => paths.push(PathBuf::from(text("skillPath")?)),
            "skills_backup_adopt_many" => paths.extend(
                params
                    .get("skills")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| entry.get("skillPath").and_then(Value::as_str))
                    .map(PathBuf::from),
            ),
            "bundled_skill_install" | "bundled_skill_remove" | "bundled_skill_prompt_dismiss" => {
                use tendi_core::bundled_skill::BundledSkillOperation;
                let operation = match method {
                    "bundled_skill_remove" => {
                        BundledSkillOperation::Remove(tendi_core::AgentKind::Shared)
                    }
                    "bundled_skill_prompt_dismiss" => BundledSkillOperation::DismissPrompt,
                    _ => {
                        let request: runtime_schema::BundledSkillInstallRequest =
                            serde_json::from_value(params.clone())
                                .map_err(|error| invalid_argument(error.to_string()))?;
                        BundledSkillOperation::Install(bundled_skill_agent(request.agent))
                    }
                };
                paths.extend(
                    tendi_core::bundled_skill::operation_resource_paths(operation)
                        .map_err(core_error)?,
                );
            }
            _ => {}
        }
        if matches!(
            method,
            "skill_file_save"
                | "skill_file_create"
                | "skill_folder_create"
                | "skill_path_rename"
                | "skill_path_delete"
                | "skills_add"
                | "skills_update"
                | "skills_update_many"
                | "skills_delete_many"
                | "skills_remove_locations"
                | "skills_set"
                | "skills_wrap"
                | "skills_distribute"
        ) {
            paths.extend(
                self.cached_skills(store)?
                    .skills
                    .into_iter()
                    .filter(|skill| skill.is_wrapper)
                    .flat_map(|skill| skill.paths.into_iter())
                    .map(|path| path.path.join("SKILL.md")),
            );
        }
        let mut resources = Vec::new();
        if skill_projection_writer(method) {
            resources.push(skills_projection_resource(store));
        }
        if !paths.is_empty() {
            resources.push(ResourceRequest::files(paths).map_err(core_error)?);
        }
        Ok(resources)
    }

    fn cached_skills(
        &self,
        store: &tendi_core::storage::Store,
    ) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        store
            .read_cached_projection("skills", &self.state.cwd)
            .map_err(core_error)?
            .ok_or_else(|| conflict_error("skills projection unavailable"))
    }
}

fn keep_cleanup<T: Send + 'static>(step: Step<T>, cleanup: Cleanup) -> Step<T> {
    match step {
        Step::Complete(value) => {
            drop(cleanup);
            Step::Complete(value)
        }
        Step::Acquire {
            workload,
            resources,
            run,
        } => Step::acquire(workload, resources, move || {
            run().map(|next| keep_cleanup(next, cleanup))
        }),
    }
}

fn projection_dependencies(method: &str) -> &'static [&'static str] {
    match method {
        "scan" => &["agents", "skills", "rules", "hooks", "mcp"],
        "skill_file_save"
        | "skill_file_create"
        | "skill_folder_create"
        | "skill_path_rename"
        | "skill_path_delete"
        | "skills_add"
        | "skills_update"
        | "skills_update_many"
        | "skills_delete_many"
        | "skills_remove_locations"
        | "skills_set"
        | "skills_wrap"
        | "skills_distribute"
        | "bundled_skill_install"
        | "bundled_skill_remove"
        | "skills_backup_restore"
        | "skills_backup_adopt"
        | "skills_backup_adopt_many" => &["skills"],
        "hook_delete"
        | "hook_delete_many"
        | "hook_set_enabled"
        | "hook_set_enabled_many"
        | "hook_review" => &["hooks"],
        "mcp_probe" | "mcp_set_enabled" | "mcp_set_enabled_many" => &["mcp"],
        "rule_file_save" | "rule_file_delete_many" => &["rules"],
        // These commands own their cache/refresh policy in the operation
        // itself. Requiring a fresh projection here duplicates the refresh
        // and makes a cached read wait behind unrelated reconciliation.
        "skills_refresh" | "skills_updates" | "skills_backup_now" | "skills_backup_status" => &[],
        _ => &[],
    }
}

#[cfg(test)]
#[path = "rpc_admission_tests.rs"]
mod admission_rpc_tests;
