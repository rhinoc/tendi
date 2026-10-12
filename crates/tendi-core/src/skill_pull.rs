use super::*;

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct PullReceipt {
    pub entries: Vec<PullEntry>,
    #[serde(default)]
    ignored: Vec<PullIgnoredEntry>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct PullIgnoredEntry {
    id: String,
    category: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct PullEntry {
    id: String,
    category: String,
    name: String,
    target: PathBuf,
    #[serde(default)]
    artifact: Option<BackupArtifact>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPullPlan {
    pub revision: String,
    pub target_root: PathBuf,
    pub operations: Vec<BackupRestoreOperation>,
    #[serde(skip)]
    restore: BackupRestorePlan,
    #[serde(skip)]
    previous: PullReceipt,
    #[serde(skip)]
    receipt_key: String,
    #[serde(skip)]
    config: BackupConfig,
    #[serde(skip)]
    checkout_head: String,
    #[serde(skip)]
    fingerprints: BTreeMap<PathBuf, String>,
    #[serde(skip)]
    workspace_root: PathBuf,
    #[serde(skip)]
    checkout_resources: Vec<PathBuf>,
}

fn effective_receipt(
    store: &Store,
    cwd: &Path,
    config: &BackupConfig,
    key: &str,
    target_root: &Path,
    revision: &str,
) -> Result<PullReceipt> {
    if let Some(receipt) = store.skill_pull_receipt(key)? {
        return Ok(receipt);
    }
    let mut receipt = PullReceipt::default();
    // Older restores recorded their exact repository revision and entry ID.
    // Only import ownership proven by an ancestor manifest in this repository.
    for record in store.skill_source_records_for_workspace(cwd)? {
        if record.source_kind != "tendi-backup"
            || record.origin != "tendi-backup-restore"
            || record.skill_path.parent() != Some(target_root)
        {
            continue;
        }
        let Some(old_revision) = record
            .source_ref
            .as_deref()
            .filter(|value| is_commit_id(value))
        else {
            continue;
        };
        let Some(id) = record
            .source_relative_path
            .as_deref()
            .and_then(|value| value.strip_prefix("skills/"))
        else {
            continue;
        };
        let Ok(manifest) = manifest_at_revision(&config.checkout_path, old_revision) else {
            continue;
        };
        if !git_is_ancestor(&config.checkout_path, old_revision, revision)? {
            continue;
        }
        let Some(skill) = manifest.skills.iter().find(|skill| skill.id == id) else {
            continue;
        };
        receipt.entries.push(PullEntry {
            id: id.to_string(),
            category: "skills".into(),
            name: skill.name.clone(),
            target: record.skill_path,
            artifact: None,
        });
    }
    receipt
        .entries
        .sort_by(|left, right| left.target.cmp(&right.target));
    Ok(receipt)
}

// Read local content without following symlinks. A final admitted operation
// must retry preparation if any path changed while it waited for its locks.
fn target_fingerprint(path: &Path) -> Result<String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok("missing".into()),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let mut hash = Sha256::new();
    for entry in WalkDir::new(path)
        .follow_links(false)
        .follow_root_links(false)
        .sort_by_file_name()
    {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        hash.update(
            entry
                .path()
                .strip_prefix(path)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .as_bytes(),
        );
        hash.update([0, u8::from(metadata.permissions().readonly())]);
        if metadata.file_type().is_symlink() {
            hash.update(b"link");
            hash.update(fs::read_link(entry.path())?.to_string_lossy().as_bytes());
        } else if metadata.is_dir() {
            hash.update(b"dir");
        } else {
            hash.update(b"file");
            hash.update(fs::read(entry.path())?);
        }
        hash.update([0]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn backup_pull_plan_is_current(plan: &BackupPullPlan, store: &Store) -> Result<bool> {
    let Some(config) = store.skill_backup_config()? else {
        return Ok(false);
    };
    if serde_json::to_string(&config)? != serde_json::to_string(&plan.config)?
        || current_commit(&config.checkout_path)? != plan.checkout_head
    {
        return Ok(false);
    }
    let receipt = effective_receipt(
        store,
        &plan.workspace_root,
        &config,
        &plan.receipt_key,
        &plan.target_root,
        &plan.revision,
    )?;
    if serde_json::to_string(&receipt)? != serde_json::to_string(&plan.previous)? {
        return Ok(false);
    }
    for (path, expected) in &plan.fingerprints {
        if target_fingerprint(path)? != *expected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn receipt_key(config: &BackupConfig, target_root: &Path) -> String {
    let repository = if config.remote_url.is_empty() {
        config.checkout_path.to_string_lossy().to_string()
    } else {
        normalize_remote_url(&config.remote_url)
    };
    format!(
        "skill_sync_pull:{}",
        sha256_hex(format!("{repository}\n{}", target_root.display()).as_bytes())
    )
}

pub fn pull_resource_paths(plan: &BackupPullPlan) -> Vec<PathBuf> {
    let mut paths = backup_restore_resource_paths(&plan.restore);
    paths.extend(plan.checkout_resources.iter().cloned());
    paths.extend(
        plan.previous
            .entries
            .iter()
            .map(|entry| entry.target.clone()),
    );
    paths
}

/// Fetch the published set. Local inclusion choices are publication settings,
/// so they do not filter what this receiving device pulls.
pub fn plan_backup_pull(
    store: &Store,
    cwd: &Path,
    target: &SkillTarget,
    scope: SkillInstallScope,
    expected_revision: Option<&str>,
) -> Result<BackupPullPlan> {
    let config = store
        .skill_backup_config()?
        .context("sync is not configured")?;
    let checkout_resources = checkout_mutation_resource_paths(&config.checkout_path)?;
    let _resources = crate::coordination::acquire_file_resources(&checkout_resources)?;
    let revision = if ensure_checkout(&config)? {
        refresh_remote_checkout(&config)?.context("the sync repository has no published set")?
    } else {
        current_commit(&config.checkout_path)?
    };
    if expected_revision.is_some_and(|expected| expected != revision) {
        bail!("the published set changed; pull again to review the latest version");
    }
    let mut restore = plan_backup_restore(store, cwd, &revision, &[], target, scope)?;
    let receipt_key = receipt_key(&config, &restore.target_root);
    let previous = effective_receipt(
        store,
        cwd,
        &config,
        &receipt_key,
        &restore.target_root,
        &revision,
    )?;
    for entry in &previous.entries {
        validate_owned_target(entry, &restore.target_root)?;
    }
    let mut fingerprints = BTreeMap::new();
    for path in previous.entries.iter().map(|entry| &entry.target).chain(
        restore
            .operations
            .iter()
            .filter(|operation| {
                !previous.ignored.iter().any(|ignored| {
                    ignored.id == operation.id && ignored.category == operation.category
                })
            })
            .map(|operation| &operation.target),
    ) {
        fingerprints.insert(path.clone(), target_fingerprint(path)?);
    }
    let mut reserved_targets = previous
        .entries
        .iter()
        .map(|entry| entry.target.clone())
        .chain(
            restore
                .operations
                .iter()
                .map(|operation| operation.target.clone()),
        )
        .collect::<BTreeSet<_>>();
    for operation in &mut restore.operations {
        if previous
            .ignored
            .iter()
            .any(|ignored| ignored.id == operation.id && ignored.category == operation.category)
        {
            operation.status = "skipped".into();
            operation.message = Some("kept local by your earlier choice".into());
            continue;
        }
        let mut owned = previous
            .entries
            .iter()
            .find(|entry| entry.id == operation.id && entry.category == operation.category);
        if let Some(owned) = owned {
            operation.target = owned.target.clone();
        } else if let Some(at_target) = previous.entries.iter().find(|entry| {
            entry.category == operation.category
                && entry.target == operation.target
                && (operation.category == "skills"
                    || entry.artifact.as_ref().is_some_and(|artifact| {
                        same_artifact_identity(artifact, &restore.artifacts[&operation.id])
                    }))
        }) {
            if operation.category == "skills" {
                if restore.skills.contains_key(&at_target.id) {
                    // Keep an existing managed destination reserved, including
                    // one allocated by an earlier keep-both resolution.
                    operation.target = unique_restore_target(&operation.target, &reserved_targets)?;
                    operation.status = "planned".into();
                    operation.message = None;
                } else {
                    // A new manifest identity can reuse a retired destination.
                    owned = Some(at_target);
                }
            } else if at_target.artifact.as_ref().is_some_and(|artifact| {
                same_artifact_identity(artifact, &restore.artifacts[&operation.id])
            }) {
                owned = Some(at_target);
            }
        }
        reserved_targets.insert(operation.target.clone());
        if !fingerprints.contains_key(&operation.target) {
            fingerprints.insert(
                operation.target.clone(),
                target_fingerprint(&operation.target)?,
            );
        }
        let current = if operation.category == "skills" {
            restore.skills.get(&operation.id).is_some_and(|skill| {
                backup_files(&operation.target).is_ok_and(|files| files == skill.files)
            })
        } else {
            let artifact = &restore.artifacts[&operation.id];
            artifact_is_current(artifact, &operation.target)?
        };
        if current {
            operation.status = "current".into();
            operation.message = None;
        } else if owned.is_some() {
            operation.status = if operation.target.exists()
                && (operation.category == "skills"
                    || restore.artifacts[&operation.id].entry_key.is_empty())
            {
                "replace".into()
            } else {
                "planned".into()
            };
            operation.message = None;
        } else if operation.category != "skills" {
            let artifact = &restore.artifacts[&operation.id];
            if artifact_entry_exists(artifact, &operation.target)? {
                operation.status = "conflict".into();
                operation.message = Some("local content exists; choose how to pull it".into());
            }
        }
    }
    let mut operations = restore.operations.clone();
    for entry in &previous.entries {
        if !operations.iter().any(|operation| {
            operation.id == entry.id
                && operation.category == entry.category
                && operation.target == entry.target
        }) {
            let id = if operations.iter().any(|operation| operation.id == entry.id) {
                format!(
                    "remove:{}:{}",
                    entry.id,
                    &sha256_hex(entry.target.to_string_lossy().as_bytes())[..16]
                )
            } else {
                entry.id.clone()
            };
            operations.push(BackupRestoreOperation {
                id,
                name: entry.name.clone(),
                category: entry.category.clone(),
                target: entry.target.clone(),
                status: "remove".into(),
                message: Some("removed from the published set".into()),
            });
        }
    }
    for (path, expected) in &fingerprints {
        if target_fingerprint(path)? != *expected {
            bail!("local pull targets changed during preparation; pull again");
        }
    }
    let checkout_head = current_commit(&config.checkout_path)?;
    Ok(BackupPullPlan {
        revision,
        target_root: restore.target_root.clone(),
        operations,
        restore,
        previous,
        receipt_key,
        config,
        checkout_head,
        fingerprints,
        workspace_root: cwd.to_path_buf(),
        checkout_resources,
    })
}

fn validate_owned_target(entry: &PullEntry, target_root: &Path) -> Result<()> {
    if entry.category == "skills" {
        if entry.target.parent() != Some(target_root) {
            bail!("pull receipt has a skill outside the selected target");
        }
    } else {
        let artifact = entry
            .artifact
            .as_ref()
            .context("pull receipt is missing artifact ownership")?;
        let expected = restore_artifact_target(artifact)?;
        let sibling = artifact.entry_key.is_empty() && is_restore_sibling(&expected, &entry.target);
        if expected != entry.target && !sibling {
            bail!("pull receipt has an invalid provider target");
        }
    }
    Ok(())
}

fn is_restore_sibling(expected: &Path, target: &Path) -> bool {
    if expected.parent() != target.parent() {
        return false;
    }
    let Some(name) = expected.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(target_name) = target.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(suffix) = target_name.strip_prefix(&format!("{name}.restored")) else {
        return false;
    };
    suffix.is_empty()
        || suffix
            .strip_prefix('-')
            .is_some_and(|value| value.parse::<usize>().is_ok_and(|index| index >= 2))
}

fn same_artifact_identity(left: &BackupArtifact, right: &BackupArtifact) -> bool {
    left.category == right.category
        && left.agent == right.agent
        && left.source_relative_path == right.source_relative_path
        && left.entry_key == right.entry_key
        && left.entry_selector == right.entry_selector
}

fn artifact_entry(artifact: &BackupArtifact, target: &Path) -> Result<Option<serde_json::Value>> {
    if !target.is_file() || artifact.entry_key.is_empty() {
        return Ok(None);
    }
    let agent = crate::providers::parse_agent(&artifact.agent).context("unknown sync provider")?;
    let provider = crate::providers::agent_provider(agent);
    let value = match artifact.category.as_str() {
        "mcp" => provider.backup_mcp_entry(target, &artifact.entry_selector, &artifact.entry_key),
        "hooks" => provider.backup_hook_entry(
            target,
            &crate::hooks::hook_source_match_from_key(&artifact.entry_key)?,
        ),
        _ => bail!("unsupported sync entry category {}", artifact.category),
    };
    match value {
        Ok(value) => Ok(Some(value)),
        Err(error)
            if error
                .downcast_ref::<crate::providers::MissingSyncEntry>()
                .is_some() =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn artifact_entry_exists(artifact: &BackupArtifact, target: &Path) -> Result<bool> {
    if artifact.entry_key.is_empty() {
        return Ok(target.exists());
    }
    Ok(artifact_entry(artifact, target)?.is_some())
}

fn artifact_is_current(artifact: &BackupArtifact, target: &Path) -> Result<bool> {
    let content = if artifact.entry_key.is_empty() {
        if !target.is_file() {
            return Ok(false);
        }
        fs::read(target)?
    } else {
        let Some(value) = artifact_entry(artifact, target)? else {
            return Ok(false);
        };
        let mut content = serde_json::to_vec_pretty(&value)?;
        content.push(b'\n');
        content
    };
    Ok(artifact.files.len() == 1
        && artifact.files[0].sha256 == sha256_hex(&content)
        && artifact.files[0].size == content.len() as u64)
}

pub fn apply_backup_pull(
    plan: &BackupPullPlan,
    store: &Store,
    cwd: &Path,
    resolutions: &[BackupRestoreResolution],
) -> Result<Vec<BackupRestoreOperation>> {
    let _resources = crate::coordination::acquire_file_resources(&pull_resource_paths(plan))?;
    if !backup_pull_plan_is_current(plan, store)? {
        bail!("pull inputs changed after the preview; pull again");
    }
    for operation in &plan.restore.operations {
        if operation.status == "conflict"
            && !resolutions
                .iter()
                .any(|resolution| resolution.id == operation.id)
        {
            bail!(
                "pull requires a resolution for local content {}",
                operation.name
            );
        }
    }
    // Verify every incoming byte before replacing any local directory or file.
    for skill in plan.restore.skills.values() {
        for file in &skill.files {
            revision_content(&plan.restore, "skills", &skill.id, file)?;
        }
    }
    for artifact in plan.restore.artifacts.values() {
        for file in &artifact.files {
            revision_content(&plan.restore, &artifact.category, &artifact.id, file)?;
        }
    }
    let mut resolved = resolve_restore_operations(&plan.restore, resolutions)?;
    resolved.extend(
        plan.operations
            .iter()
            .filter(|operation| operation.status == "remove")
            .cloned(),
    );
    let mut undo = PullUndo::capture(&resolved)?;
    let applied = (|| {
        let applied = apply_backup_restore_without_database(&plan.restore, resolutions)?;
        let mut operations = applied.operations;
        let mut deleted_skills = Vec::new();
        for operation in plan
            .operations
            .iter()
            .filter(|operation| operation.status == "remove")
        {
            let entry = plan
                .previous
                .entries
                .iter()
                .find(|entry| {
                    entry.target == operation.target && entry.category == operation.category
                })
                .context("pull lost removal ownership")?;
            if entry.category == "skills" {
                let reused = operations.iter().any(|current| {
                    current.category == "skills"
                        && current.target == entry.target
                        && matches!(current.status.as_str(), "restored" | "current")
                });
                if !reused && entry.target.exists() {
                    remove_restore_target(&plan.target_root, &entry.target)?;
                }
                deleted_skills.push(entry.target.clone());
            } else {
                let artifact = entry
                    .artifact
                    .as_ref()
                    .context("pull lost artifact ownership")?;
                let reused = operations.iter().any(|current| {
                    current.target == entry.target
                        && matches!(current.status.as_str(), "restored" | "current")
                        && plan
                            .restore
                            .artifacts
                            .get(&current.id)
                            .is_some_and(|incoming| same_artifact_identity(artifact, incoming))
                });
                if !reused {
                    remove_artifact(artifact, &entry.target)?;
                }
            }
            let mut removed = operation.clone();
            removed.status = "removed".into();
            operations.push(removed);
        }
        let mut receipt = PullReceipt::default();
        receipt.ignored = operations
            .iter()
            .filter(|operation| operation.status == "skipped")
            .map(|operation| PullIgnoredEntry {
                id: operation.id.clone(),
                category: operation.category.clone(),
            })
            .collect();
        for operation in operations
            .iter()
            .filter(|operation| operation.status == "restored" || operation.status == "current")
        {
            receipt.entries.push(PullEntry {
                id: operation.id.clone(),
                category: operation.category.clone(),
                name: operation.name.clone(),
                target: operation.target.clone(),
                artifact: plan.restore.artifacts.get(&operation.id).cloned(),
            });
        }
        // A skipped local conflict stays local. Only successful pulls grant
        // ownership for future replacement and removal.
        let mut source_records = applied.source_records;
        for operation in operations
            .iter()
            .filter(|operation| operation.category == "skills" && operation.status == "current")
        {
            source_records.push(restored_skill_source_record(
                &plan.restore.skills[&operation.id],
                &operation.target,
                &plan.revision,
            ));
        }
        store.complete_skill_pull(
            cwd,
            &plan.receipt_key,
            &receipt,
            &source_records,
            &deleted_skills,
        )?;
        Ok(operations)
    })();
    if let Err(error) = &applied {
        if let Err(rollback) = undo.restore() {
            undo.preserve = true;
            bail!(
                "pull failed ({error:#}); rollback failed ({rollback:#}); original files are saved in {}",
                undo.root.display()
            );
        }
    }
    applied
}

fn revision_content(
    plan: &BackupRestorePlan,
    category: &str,
    id: &str,
    file: &BackupFile,
) -> Result<Vec<u8>> {
    safe_relative_path(&file.path)?;
    let output = run_git_success(
        &plan.checkout,
        [
            "show",
            &format!("{}:{category}/{id}/{}", plan.revision, file.path),
        ],
        false,
    )?;
    if sha256_hex(&output.stdout) != file.sha256 || output.stdout.len() as u64 != file.size {
        bail!(
            "sync content failed integrity verification for {}",
            file.path
        );
    }
    Ok(output.stdout)
}

fn remove_artifact(artifact: &BackupArtifact, target: &Path) -> Result<()> {
    if !target.exists() {
        return Ok(());
    }
    if artifact.entry_key.is_empty() {
        fs::remove_file(target)?;
        return Ok(());
    }
    if !artifact_entry_exists(artifact, target)? {
        return Ok(());
    }
    let agent = crate::providers::parse_agent(&artifact.agent).context("unknown sync provider")?;
    let provider = crate::providers::agent_provider(agent);
    let content = match artifact.category.as_str() {
        "mcp" => {
            provider.remove_mcp_entry(target, &artifact.entry_selector, &artifact.entry_key)?
        }
        "hooks" => {
            let identity = crate::hooks::hook_source_match_from_key(&artifact.entry_key)?;
            provider.delete_hooks(
                &[crate::hooks::HookDeleteRequest {
                    agent,
                    path: target.to_path_buf(),
                    expected_trust_hash: String::new(),
                    event: identity.event,
                    matcher: identity.matcher,
                    hook_type: identity.hook_type,
                    command: identity.command,
                    url: identity.url,
                    prompt: identity.prompt,
                    filter: identity.filter,
                    status_message: identity.status_message,
                }],
                &fs::read_to_string(target)?,
            )?
        }
        _ => bail!("unsupported sync entry category {}", artifact.category),
    };
    crate::fsutil::atomic_write(target, &content)
}

// Keep original local bytes until the complete pull and receipt have succeeded.
// A failure must not leave half of a synchronized skill set on the device.
struct PullUndo {
    root: PathBuf,
    paths: Vec<(PathBuf, Option<PathBuf>)>,
    preserve: bool,
}
impl PullUndo {
    fn capture(operations: &[BackupRestoreOperation]) -> Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "tendi-pull-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fs::create_dir(&root)?;
        let mut undo = Self {
            root,
            paths: Vec::new(),
            preserve: false,
        };
        let paths = operations
            .iter()
            .filter(|operation| operation.status != "current" && operation.status != "skipped")
            .map(|operation| operation.target.clone())
            .collect::<BTreeSet<_>>();
        for (index, path) in paths.into_iter().enumerate() {
            let backup = match fs::symlink_metadata(&path) {
                Ok(_) => {
                    let backup = undo.root.join(index.to_string());
                    copy_local_path(&path, &backup)?;
                    Some(backup)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            undo.paths.push((path, backup));
        }
        Ok(undo)
    }
    fn restore(&self) -> Result<()> {
        for (path, backup) in self.paths.iter().rev() {
            if path.is_dir() {
                fs::remove_dir_all(path)?;
            } else if path.exists() {
                fs::remove_file(path)?;
            }
            if let Some(backup) = backup {
                copy_local_path(backup, path)?;
            }
        }
        Ok(())
    }
}
impl Drop for PullUndo {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn copy_local_path(source: &Path, target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        bail!(
            "pull target {} contains a symlink; keep it local or select another target",
            source.display()
        );
    }
    if metadata.is_dir() {
        fs::create_dir_all(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_local_path(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, target)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "skill_pull_tests.rs"]
mod tests;
