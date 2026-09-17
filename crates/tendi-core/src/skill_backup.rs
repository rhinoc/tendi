use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::{SkillInstallScope, SkillTarget, git, skills::SkillSourceRecord, storage::Store};

const BACKUP_MANIFEST_VERSION: u32 = 1;
const MAX_SKILL_BYTES: u64 = 100 * 1024 * 1024;
const MAX_SKILL_FILES: usize = 1_000;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupCategorySelection {
    pub enabled: bool,
    #[serde(default)]
    pub excluded: Vec<String>,
}

impl Default for BackupCategorySelection {
    fn default() -> Self {
        Self {
            enabled: true,
            excluded: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupContents {
    #[serde(default)]
    pub skills: BackupCategorySelection,
    #[serde(default)]
    pub mcp: BackupCategorySelection,
    #[serde(default)]
    pub rules: BackupCategorySelection,
    #[serde(default)]
    pub hooks: BackupCategorySelection,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupCatalogItem {
    pub id: String,
    pub label: String,
    pub detail: String,
    #[serde(skip)]
    pub(crate) source_path: Option<PathBuf>,
    #[serde(skip)]
    pub(crate) agent: Option<crate::skills::AgentKind>,
    #[serde(skip)]
    pub(crate) source_key: Option<String>,
    #[serde(skip)]
    pub(crate) entry_key: Option<String>,
    #[serde(skip)]
    pub(crate) entry_selector: Vec<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupCatalog {
    pub skills: Vec<BackupCatalogItem>,
    pub mcp: Vec<BackupCatalogItem>,
    pub rules: Vec<BackupCatalogItem>,
    pub hooks: Vec<BackupCatalogItem>,
}

#[derive(Debug, Clone, Default)]
pub struct BackupBuildOptions {
    pub device_label: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupConfig {
    pub remote_url: String,
    pub checkout_path: PathBuf,
    #[serde(default)]
    pub contents: BackupContents,
}

impl BackupConfig {
    pub fn new(remote_url: impl Into<String>, checkout_path: PathBuf) -> Self {
        Self {
            remote_url: normalize_remote_url(&remote_url.into()),
            checkout_path,
            contents: BackupContents::default(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if !self.remote_url.is_empty() && has_embedded_credentials(&self.remote_url) {
            bail!("sync remote URLs must not contain credentials");
        }
        if !self.checkout_path.is_absolute() {
            bail!("sync checkout path must be absolute");
        }
        Ok(())
    }
}

pub fn current_machine_name() -> Result<String> {
    #[cfg(target_os = "macos")]
    let output = Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .context("failed to read machine name")?;
    #[cfg(not(target_os = "macos"))]
    let output = Command::new("hostname")
        .output()
        .context("failed to read machine name")?;

    if !output.status.success() {
        bail!("machine name command exited with {}", output.status);
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        bail!("machine name is empty");
    }
    Ok(name)
}

pub fn validate_remote(remote_url: &str, working_directory: &Path) -> Result<()> {
    let remote_url = normalize_remote_url(remote_url);
    if remote_url.is_empty() {
        bail!("a sync remote URL is required");
    }
    if has_embedded_credentials(&remote_url) {
        bail!("sync remote URLs must not contain credentials");
    }
    let output = git::run_git(
        working_directory,
        ["ls-remote", &remote_url],
        git::NETWORK_COMMAND_TIMEOUT,
        git::never_cancelled(),
    )
    .context("failed to check the sync Git remote")?;
    if !output.status.success() {
        bail!("Git remote is not reachable or is not a Git repository");
    }
    Ok(())
}

pub fn is_remote_repository(value: &str) -> bool {
    let value = value.trim();
    value.contains("://")
        || value.starts_with("git@")
        || (value
            .find(':')
            .is_some_and(|index| value[..index].contains('@')))
        || value
            .strip_prefix("github.com/")
            .is_some_and(is_github_repository_path)
        || is_github_repository_path(value)
}

pub fn discover_git_repository_root(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let output = git_output(path, ["rev-parse", "--show-toplevel"], false)?;
    if !output.status.success() {
        return Ok(None);
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(root)))
}

/// Expand the GitHub shorthand accepted by the Backup UI into a Git remote URL.
/// Other Git remote forms keep their existing behavior.
fn normalize_remote_url(remote_url: &str) -> String {
    let remote_url = remote_url.trim();
    let github_path = remote_url
        .strip_prefix("github.com/")
        .or_else(|| is_github_repository_path(remote_url).then_some(remote_url));

    match github_path.filter(|path| is_github_repository_path(path)) {
        Some(path) => {
            let path = path.trim_end_matches('/');
            let path = path.strip_suffix(".git").unwrap_or(path);
            format!("https://github.com/{path}.git")
        }
        None => remote_url.to_string(),
    }
}

fn is_github_repository_path(value: &str) -> bool {
    let value = value.trim_end_matches('/');
    let mut segments = value.split('/');
    let Some(owner) = segments.next() else {
        return false;
    };
    let Some(repository) = segments.next() else {
        return false;
    };
    segments.next().is_none()
        && is_github_name(owner)
        && is_github_name(repository.trim_end_matches(".git"))
}

fn is_github_name(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub fn default_checkout_path() -> Result<PathBuf> {
    let db_path = crate::storage::default_db_path()?;
    let data_dir = db_path
        .parent()
        .context("backup database has no parent directory")?;
    Ok(data_dir.join("skill-backup"))
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSyncReport {
    pub manifest: BackupManifest,
    pub commit: Option<String>,
    pub pushed: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupVersion {
    pub id: String,
    pub created_at: i64,
    pub summary: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRestorePlan {
    pub revision: String,
    pub target_root: PathBuf,
    pub operations: Vec<BackupRestoreOperation>,
    #[serde(skip)]
    checkout: PathBuf,
    #[serde(skip)]
    skills: BTreeMap<String, BackupSkill>,
    #[serde(skip)]
    artifacts: BTreeMap<String, BackupArtifact>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRestoreOperation {
    pub id: String,
    pub name: String,
    pub category: String,
    pub target: PathBuf,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRestoreResolution {
    pub id: String,
    pub action: String,
}

#[derive(Debug)]
pub struct BackupRestoreApplyResult {
    pub operations: Vec<BackupRestoreOperation>,
    pub source_records: Vec<SkillSourceRecord>,
}

pub fn plan_backup_restore(
    store: &Store,
    cwd: &Path,
    revision: &str,
    skill_ids: &[String],
    target: &SkillTarget,
    scope: SkillInstallScope,
) -> Result<BackupRestorePlan> {
    if !is_commit_id(revision) {
        bail!("sync restore requires a Git commit id");
    }
    let config = store
        .skill_backup_config()?
        .context("skill backup is not configured")?;
    let manifest = manifest_at_revision(&config.checkout_path, revision)?;
    let target_root = crate::skill_targets::skill_target_root(cwd, target, scope)?;
    let requested = skill_ids.iter().cloned().collect::<BTreeSet<_>>();
    let selected_skills = manifest
        .skills
        .iter()
        .filter(|skill| requested.is_empty() || requested.contains(&skill.id))
        .cloned()
        .collect::<Vec<_>>();
    let selected_artifacts = manifest
        .artifacts
        .iter()
        .filter(|artifact| requested.is_empty() || requested.contains(&artifact.id))
        .cloned()
        .collect::<Vec<_>>();
    if !requested.is_empty() && selected_skills.len() + selected_artifacts.len() != requested.len()
    {
        bail!("one or more requested sync contents are not in sync version {revision}");
    }
    let mut reserved_targets = BTreeSet::new();
    let mut operations = Vec::new();
    let mut skills = BTreeMap::new();
    let mut artifacts = BTreeMap::new();
    for skill in selected_skills {
        let folder = restore_folder_name(&skill, &reserved_targets);
        reserved_targets.insert(folder.clone());
        let destination = target_root.join(&folder);
        let (status, message) = if destination.exists() {
            (
                "conflict".to_string(),
                Some(
                    "target already exists; choose a restore resolution before applying"
                        .to_string(),
                ),
            )
        } else {
            ("planned".to_string(), None)
        };
        operations.push(BackupRestoreOperation {
            id: skill.id.clone(),
            name: skill.name.clone(),
            category: "skills".to_string(),
            target: destination,
            status,
            message,
        });
        skills.insert(skill.id.clone(), skill);
    }
    for artifact in selected_artifacts {
        let destination = restore_artifact_target(&artifact)?;
        let (status, message) = if !artifact.entry_key.is_empty() {
            // Entry artifacts merge into an existing provider config; the file's
            // presence is not itself a restore conflict.
            ("planned".to_string(), None)
        } else if destination.exists() {
            (
                "conflict".to_string(),
                Some(
                    "target already exists; choose a restore resolution before applying"
                        .to_string(),
                ),
            )
        } else {
            ("planned".to_string(), None)
        };
        operations.push(BackupRestoreOperation {
            id: artifact.id.clone(),
            name: artifact.name.clone(),
            category: artifact.category.clone(),
            target: destination,
            status,
            message,
        });
        artifacts.insert(artifact.id.clone(), artifact);
    }
    Ok(BackupRestorePlan {
        revision: revision.to_string(),
        target_root,
        operations,
        checkout: config.checkout_path,
        skills,
        artifacts,
    })
}

fn restore_artifact_target(artifact: &BackupArtifact) -> Result<PathBuf> {
    let agent = crate::providers::parse_agent(&artifact.agent)
        .with_context(|| format!("backup artifact {} has an unknown provider", artifact.name))?;
    crate::providers::agent_provider(agent)
        .restore_global_source_path(&artifact.source_relative_path)
        .with_context(|| {
            format!(
                "backup artifact {} has no valid global target",
                artifact.name
            )
        })
}

/// Keep-both can allocate a sibling name; reserve those containing directories
/// together with the checkout before choosing names or reading merge targets.
pub fn backup_restore_resource_paths(plan: &BackupRestorePlan) -> Vec<PathBuf> {
    let mut paths = vec![plan.checkout.clone(), plan.target_root.clone()];
    paths.extend(plan.operations.iter().map(|operation| {
        operation
            .target
            .parent()
            .unwrap_or(&operation.target)
            .to_path_buf()
    }));
    paths
}

pub fn apply_backup_restore(
    plan: &BackupRestorePlan,
    store: &Store,
    workspace_root: &Path,
    resolutions: &[BackupRestoreResolution],
) -> Result<Vec<BackupRestoreOperation>> {
    let _resources =
        crate::coordination::acquire_file_resources(&backup_restore_resource_paths(plan))?;
    let result = apply_backup_restore_without_database(plan, resolutions)?;
    store.upsert_skill_source_records_for_workspace(workspace_root, &result.source_records)?;
    Ok(result.operations)
}

pub fn apply_backup_restore_without_database(
    plan: &BackupRestorePlan,
    resolutions: &[BackupRestoreResolution],
) -> Result<BackupRestoreApplyResult> {
    let _resources =
        crate::coordination::acquire_file_resources(&backup_restore_resource_paths(plan))?;
    let mut operations = plan.operations.clone();
    let mut source_records = Vec::new();
    let mut resolutions_by_id = BTreeMap::new();
    for resolution in resolutions {
        if resolutions_by_id
            .insert(resolution.id.as_str(), resolution.action.as_str())
            .is_some()
        {
            bail!("sync restore contains duplicate conflict resolutions");
        }
    }
    for id in resolutions_by_id.keys() {
        if !operations.iter().any(|operation| operation.id == *id) {
            bail!("sync restore resolution references unknown content {id}");
        }
    }
    let mut reserved_targets = operations
        .iter()
        .map(|operation| operation.target.clone())
        .collect::<BTreeSet<_>>();
    for operation in &mut operations {
        if operation.status != "conflict" {
            continue;
        }
        let Some(action) = resolutions_by_id.get(operation.id.as_str()).copied() else {
            continue;
        };
        match action {
            "skip" => {
                operation.status = "skipped".to_string();
                operation.message = Some("kept the existing skill".to_string());
            }
            "replace" => {
                operation.status = "replace".to_string();
                operation.message = None;
            }
            "keep-both" => {
                reserved_targets.remove(&operation.target);
                operation.target = if operation.category == "skills" {
                    unique_restore_target(&operation.target, &reserved_targets)?
                } else {
                    unique_restore_file_target(&operation.target, &reserved_targets)?
                };
                reserved_targets.insert(operation.target.clone());
                operation.status = "planned".to_string();
                operation.message = None;
            }
            _ => bail!("unknown sync restore resolution {action}"),
        }
    }
    for operation in &mut operations {
        if operation.status != "planned" && operation.status != "replace" {
            continue;
        }
        let replacing = operation.status == "replace";
        if replacing && operation.category == "skills" {
            remove_restore_target(&plan.target_root, &operation.target)?;
            operation.status = "planned".to_string();
        } else if replacing && operation.target.exists() {
            fs::remove_file(&operation.target)
                .with_context(|| format!("failed to replace {}", operation.target.display()))?;
            operation.status = "planned".to_string();
        }
        let is_entry_artifact = if operation.category == "skills" {
            false
        } else {
            plan.artifacts
                .get(&operation.id)
                .is_some_and(|artifact| !artifact.entry_key.is_empty())
        };
        if !is_entry_artifact && operation.target.exists() {
            bail!(
                "restore target {} changed after the preview; create a new restore plan",
                operation.target.display()
            );
        }
        if operation.category == "skills" {
            let skill = plan
                .skills
                .get(&operation.id)
                .context("backup restore plan lost its selected skill")?;
            fs::create_dir_all(&operation.target)
                .with_context(|| format!("failed to create {}", operation.target.display()))?;
            for file in &skill.files {
                let relative = safe_relative_path(&file.path)?;
                let output = run_git_success(
                    &plan.checkout,
                    [
                        "show",
                        &format!("{}:skills/{}/{}", plan.revision, skill.id, file.path),
                    ],
                    false,
                )?;
                let content = output.stdout;
                if sha256_hex(&content) != file.sha256 || content.len() as u64 != file.size {
                    bail!(
                        "backup version {} failed integrity verification for {}",
                        plan.revision,
                        file.path
                    );
                }
                let target = operation.target.join(relative);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&target, content)
                    .with_context(|| format!("failed to restore {}", target.display()))?;
            }
            let record = SkillSourceRecord {
                skill_name: skill.name.clone(),
                skill_path: operation.target.clone(),
                source_kind: "tendi-backup".to_string(),
                source: None,
                source_ref: Some(plan.revision.clone()),
                source_version: skill.source.source_version.clone(),
                source_relative_path: Some(format!("skills/{}", skill.id)),
                update_status: "local".to_string(),
                origin: "tendi-backup-restore".to_string(),
            };
            source_records.push(record);
        } else {
            let artifact = plan
                .artifacts
                .get(&operation.id)
                .context("backup restore plan lost its selected artifact")?;
            if artifact.files.len() != 1 {
                bail!(
                    "backup artifact {} has unsupported file layout",
                    artifact.name
                );
            }
            let file = &artifact.files[0];
            let output = run_git_success(
                &plan.checkout,
                [
                    "show",
                    &format!(
                        "{}:{}/{}/{}",
                        plan.revision, artifact.category, artifact.id, file.path
                    ),
                ],
                false,
            )?;
            let content = output.stdout;
            if sha256_hex(&content) != file.sha256 || content.len() as u64 != file.size {
                bail!(
                    "backup version {} failed integrity verification for {}",
                    plan.revision,
                    file.path
                );
            }
            if let Some(parent) = operation.target.parent() {
                fs::create_dir_all(parent)?;
            }
            if artifact.entry_key.is_empty() {
                fs::write(&operation.target, content)
                    .with_context(|| format!("failed to restore {}", operation.target.display()))?;
            } else {
                let entry = serde_json::from_slice::<serde_json::Value>(&content)
                    .with_context(|| format!("invalid sync entry {}", artifact.name))?;
                let agent = crate::providers::parse_agent(&artifact.agent)
                    .with_context(|| format!("unknown provider {}", artifact.agent))?;
                let provider = crate::providers::agent_provider(agent);
                let merged = match artifact.category.as_str() {
                    "mcp" => provider.restore_mcp_entry(
                        &operation.target,
                        &artifact.entry_selector,
                        &artifact.entry_key,
                        &entry,
                    )?,
                    "hooks" => {
                        let identity =
                            crate::hooks::hook_source_match_from_key(&artifact.entry_key)?;
                        provider.restore_hook_entry(&operation.target, &identity, &entry)?
                    }
                    _ => bail!("unsupported entry sync category {}", artifact.category),
                };
                crate::fsutil::atomic_write(&operation.target, &merged)
                    .with_context(|| format!("failed to restore {}", operation.target.display()))?;
            }
        }
        operation.status = "restored".to_string();
        operation.message = None;
    }
    Ok(BackupRestoreApplyResult {
        operations,
        source_records,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSkillStatus {
    pub skill_path: PathBuf,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub fn adopt_skill_for_backup(
    store: &Store,
    workspace_root: &Path,
    skill_path: &Path,
    name: impl Into<String>,
) -> Result<SkillSourceRecord> {
    let _resources = crate::coordination::acquire_file_resources(&[skill_path.to_path_buf()])?;
    let record = skill_backup_record_for_adoption(skill_path, name)?;
    store
        .upsert_skill_source_records_for_workspace(workspace_root, std::slice::from_ref(&record))?;
    Ok(record)
}

pub fn skill_backup_record_for_adoption(
    skill_path: &Path,
    name: impl Into<String>,
) -> Result<SkillSourceRecord> {
    if let Some(reason) = backup_exclusion_reason(skill_path)? {
        bail!("skill cannot be added to backup: {reason}");
    }
    let name = name.into().trim().to_string();
    if name.is_empty() {
        bail!("skill name is required to add it to backup");
    }
    let record = SkillSourceRecord {
        skill_name: name,
        skill_path: skill_path.to_path_buf(),
        source_kind: "local".to_string(),
        source: None,
        source_ref: None,
        source_version: None,
        source_relative_path: None,
        update_status: "local".to_string(),
        origin: "tendi-backup-adopt".to_string(),
    };
    Ok(record)
}

pub fn backup_statuses_for_paths(
    store: &Store,
    workspace_root: &Path,
    paths: &[PathBuf],
) -> Result<Vec<BackupSkillStatus>> {
    let config = store.skill_backup_config()?;
    if config.is_none() {
        return Ok(Vec::new());
    }
    let records = store.skill_source_records_for_workspace(workspace_root)?;
    let records_by_path = records
        .iter()
        .map(|record| (record.skill_path.as_path(), record))
        .collect::<BTreeMap<_, _>>();
    let persisted = config
        .as_ref()
        .and_then(|config| read_checkout_manifest(&config.checkout_path).ok().flatten());
    let checkout_has_conflicts = config
        .as_ref()
        .is_some_and(|config| checkout_has_conflicts(&config.checkout_path));
    let mut statuses = Vec::new();
    for path in paths {
        let Some(record) = records_by_path.get(path.as_path()) else {
            let exclusion = backup_exclusion_reason(path)?;
            statuses.push(BackupSkillStatus {
                skill_path: path.clone(),
                state: if exclusion.is_some() {
                    "excluded"
                } else {
                    "unmanaged"
                }
                .to_string(),
                reason: exclusion,
            });
            continue;
        };
        let candidate = build_manifest(
            std::slice::from_ref(*record),
            &BackupBuildOptions::default(),
        )?;
        if let Some(excluded) = candidate.excluded.first() {
            statuses.push(BackupSkillStatus {
                skill_path: path.clone(),
                state: "excluded".to_string(),
                reason: Some(excluded.reason.clone()),
            });
            continue;
        }
        let current = candidate
            .skills
            .first()
            .expect("included candidate has one skill");
        if checkout_has_conflicts {
            statuses.push(BackupSkillStatus {
                skill_path: path.clone(),
                state: "needs-attention".to_string(),
                reason: Some("remote-conflict".to_string()),
            });
            continue;
        }
        let state = persisted
            .as_ref()
            .and_then(|manifest| {
                manifest
                    .skills
                    .iter()
                    .find(|saved| saved.id == current.id && saved.files == current.files)
                    .or_else(|| {
                        manifest
                            .skills
                            .iter()
                            .find(|saved| saved.files == current.files)
                    })
            })
            .map(|_| "backed-up")
            .unwrap_or_else(|| {
                if persisted.is_some() {
                    "pending"
                } else {
                    "not-backed-up"
                }
            });
        statuses.push(BackupSkillStatus {
            skill_path: path.clone(),
            state: state.to_string(),
            reason: None,
        });
    }
    Ok(statuses)
}

pub fn backup_catalog(store: &Store, cwd: &Path) -> Result<BackupCatalog> {
    let mut catalog = BackupCatalog::default();
    if let Some(scan) = store.list_skills_cached_for_workspace(cwd)? {
        let mut skills = Vec::new();
        for skill in scan.skills {
            let Some(global_path) = skill
                .paths
                .iter()
                .filter(|path| path.scope == "global")
                .map(|path| path.path.clone())
                .next()
            else {
                continue;
            };
            if skill.is_system {
                continue;
            }
            let detail = match skill.description.unwrap_or_default().trim() {
                "" => global_path.display().to_string(),
                description => format!("{description} · {}", global_path.display()),
            };
            skills.push(BackupCatalogItem {
                id: format!("skill:{}", skill.id),
                label: skill.name,
                detail,
                source_path: Some(global_path),
                agent: None,
                source_key: None,
                entry_key: None,
                entry_selector: Vec::new(),
            });
        }
        skills.sort_by(|left, right| left.label.cmp(&right.label).then(left.id.cmp(&right.id)));
        catalog.skills = skills;
    }

    catalog.mcp = catalog_entry_items(
        store
            .list_mcp_for_workspace(cwd)?
            .map(|scan| scan.servers)
            .unwrap_or_default()
            .into_iter()
            .filter(|server| server.scope == "global" && server.read_only_reason.is_none())
            .map(|server| {
                let detail = String::new();
                let entry_key = server.name.clone();
                (
                    server.agent,
                    server.path,
                    server.name,
                    detail,
                    entry_key,
                    server.server_path,
                )
            })
            .collect(),
        "mcp",
    );
    catalog.rules = catalog_source_files(
        store
            .list_rules_for_workspace(cwd)?
            .map(|scan| scan.rules)
            .unwrap_or_default()
            .into_iter()
            .filter(|rule| rule.scope == "global")
            .filter_map(|rule| {
                let path = rule.path;
                let title = source_file_label(&path);
                let subtitle = path.display().to_string();
                rule.agents
                    .into_iter()
                    .next()
                    .map(|agent| (agent, path, title, subtitle))
            })
            .collect(),
        "rules",
    );
    catalog.hooks = catalog_entry_items(
        store
            .list_hooks_for_workspace(cwd)?
            .map(|scan| scan.hooks)
            .unwrap_or_default()
            .into_iter()
            .filter(|hook| {
                crate::providers::agent_provider(hook.agent).is_global_hook_path(&hook.path)
                    && hook.read_only_reason.is_none()
            })
            .map(|hook| {
                let entry_key = crate::hooks::hook_source_match_key(&hook);
                let detail = hook
                    .command
                    .clone()
                    .or_else(|| hook.url.clone())
                    .or_else(|| hook.prompt.clone())
                    .unwrap_or_default();
                let label = hook.event;
                (hook.agent, hook.path, label, detail, entry_key, Vec::new())
            })
            .collect::<Vec<_>>(),
        "hooks",
    );
    Ok(catalog)
}

fn catalog_entry_items(
    sources: Vec<(
        crate::skills::AgentKind,
        PathBuf,
        String,
        String,
        String,
        Vec<String>,
    )>,
    category: &str,
) -> Vec<BackupCatalogItem> {
    sources
        .into_iter()
        .map(|(agent, path, label, detail, entry_key, entry_selector)| {
            let provider = crate::providers::agent_provider(agent);
            BackupCatalogItem {
                id: format!(
                    "{category}:{}:{}:{}",
                    agent.label(),
                    path.display(),
                    entry_key
                ),
                label,
                detail,
                source_key: provider.backup_global_source_key(&path),
                source_path: Some(path),
                agent: Some(agent),
                entry_key: Some(entry_key),
                entry_selector,
            }
        })
        .collect()
}

fn catalog_source_files(
    sources: Vec<(crate::skills::AgentKind, PathBuf, String, String)>,
    category: &str,
) -> Vec<BackupCatalogItem> {
    let mut grouped =
        BTreeMap::<(crate::skills::AgentKind, PathBuf), BTreeMap<String, BTreeSet<String>>>::new();
    for (agent, path, title, subtitle) in sources {
        grouped
            .entry((agent, path))
            .or_default()
            .entry(title)
            .or_default()
            .insert(subtitle);
    }
    grouped
        .into_iter()
        .map(|((agent, path), title_details)| {
            let provider = crate::providers::agent_provider(agent);
            let path_label = source_file_label(&path);
            let titles = title_details
                .keys()
                .filter(|title| !title.trim().is_empty())
                .cloned()
                .collect::<Vec<_>>();
            let subtitles = title_details
                .values()
                .flat_map(|values| values.iter())
                .filter(|subtitle| !subtitle.trim().is_empty())
                .cloned()
                .collect::<BTreeSet<_>>();
            BackupCatalogItem {
                id: format!("{category}:{}:{}", agent.label(), path.display()),
                label: if titles.is_empty() {
                    path_label.clone()
                } else {
                    titles.join(", ")
                },
                detail: if subtitles.len() == 1 {
                    subtitles.into_iter().next().unwrap_or_default()
                } else {
                    String::new()
                },
                source_key: provider.backup_global_source_key(&path),
                source_path: Some(path),
                agent: Some(agent),
                entry_key: None,
                entry_selector: Vec::new(),
            }
        })
        .collect()
}

fn source_file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("configuration")
        .to_string()
}

/// A linked worktree stores its index and shared Git metadata outside the checkout.
/// Admission and the core mutation owner must reserve the same complete resource set.
pub fn checkout_mutation_resource_paths(checkout: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = vec![checkout.to_path_buf()];
    paths.extend(git::mutation_resource_paths(checkout)?);
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Materialize the current managed skills into the configured checkout and create one
/// atomic Git commit, pushing it when the checkout has an origin remote. Credential
/// resolution is intentionally delegated to the system Git client, so tokens never
/// enter Tendi's database or a remote URL in a manifest.
pub fn backup_now(store: &Store, cwd: &Path) -> Result<BackupSyncReport> {
    let config = store
        .skill_backup_config()?
        .context("skill backup is not configured")?;
    let machine_name = current_machine_name()?;
    let local_manifest = build_backup_manifest(store, cwd, &config, &machine_name)?;
    let _resources = crate::coordination::acquire_file_resources(
        &checkout_mutation_resource_paths(&config.checkout_path)?,
    )?;
    let has_remote = ensure_checkout(&config)?;
    let (existing_manifest, preserved_checkout) = if has_remote {
        synchronize_remote_checkout(&config)?
    } else {
        (read_checkout_manifest(&config.checkout_path)?, None)
    };
    let temporary_root = preserved_checkout.as_ref().map(|(_, root)| root.clone());
    let existing_manifest = match preserved_checkout {
        Some((preserved, _)) => Some(merge_manifests(existing_manifest, preserved)?),
        None => existing_manifest,
    };
    let result = if local_manifest.skills.is_empty()
        && local_manifest.artifacts.is_empty()
        && existing_manifest.is_some()
    {
        // A newly connected device has no managed skills until the user chooses
        // restore targets. Never turn that empty local state into a destructive
        // remote commit.
        Ok(BackupSyncReport {
            manifest: existing_manifest.expect("checked above"),
            commit: None,
            pushed: false,
        })
    } else {
        (|| {
            let manifest = merge_manifests(existing_manifest, local_manifest)?;
            write_snapshot(&manifest, &config.checkout_path)?;
            let changed = commit_checkout(&config, &manifest, &machine_name)?;
            let needs_push = has_remote && (changed || checkout_needs_push(&config.checkout_path)?);
            if needs_push {
                run_git_success(
                    &config.checkout_path,
                    ["push", "--set-upstream", "origin", "main"],
                    true,
                )?;
            }
            Ok(BackupSyncReport {
                manifest,
                commit: if changed || needs_push {
                    Some(current_commit(&config.checkout_path)?)
                } else {
                    None
                },
                pushed: needs_push,
            })
        })()
    };
    if let Some(root) = temporary_root {
        let _ = fs::remove_dir_all(root);
    }
    result
}

/// Prepare a configured checkout so a new device can inspect and restore remote
/// versions immediately after entering the repository.
pub fn sync_checkout_for_restore(config: &BackupConfig) -> Result<Option<BackupManifest>> {
    let _resources = crate::coordination::acquire_file_resources(
        &checkout_mutation_resource_paths(&config.checkout_path)?,
    )?;
    let has_remote = ensure_checkout(config)?;
    if !has_remote {
        return read_checkout_manifest(&config.checkout_path);
    }
    let (manifest, preserved_checkout) = synchronize_remote_checkout(config)?;
    if let Some((_, temporary_root)) = preserved_checkout {
        let _ = fs::remove_dir_all(temporary_root);
    }
    Ok(manifest)
}

pub fn backup_versions(store: &Store, limit: usize) -> Result<Vec<BackupVersion>> {
    let config = store
        .skill_backup_config()?
        .context("skill backup is not configured")?;
    if !config.checkout_path.exists()
        || discover_git_repository_root(&config.checkout_path)?.is_none()
    {
        return Ok(Vec::new());
    }
    // A newly initialized checkout can exist briefly while the first backup
    // commit is being created. `git log` treats that unborn branch as an
    // error, but an empty version list is the correct status during that
    // transition.
    let head = git_output(
        &config.checkout_path,
        ["rev-parse", "--verify", "HEAD"],
        false,
    )?;
    if !head.status.success() {
        return Ok(Vec::new());
    }
    let output = run_git_success(
        &config.checkout_path,
        [
            "log",
            "--format=%H%x09%ct%x09%s",
            "-n",
            &limit.max(1).min(200).to_string(),
        ],
        false,
    )?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            Some(BackupVersion {
                id: fields.next()?.to_string(),
                created_at: fields.next()?.parse().ok()?,
                summary: fields.next()?.to_string(),
            })
        })
        .collect())
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct BackupManifest {
    pub version: u32,
    pub device_label: String,
    pub skills: Vec<BackupSkill>,
    #[serde(default)]
    pub artifacts: Vec<BackupArtifact>,
    pub excluded: Vec<BackupExcludedSkill>,
    #[serde(skip)]
    #[serde(default)]
    source_paths: BTreeMap<String, PathBuf>,
    #[serde(skip)]
    #[serde(default)]
    artifact_source_paths: BTreeMap<String, PathBuf>,
    #[serde(skip)]
    #[serde(default)]
    artifact_contents: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct BackupSkill {
    pub id: String,
    pub name: String,
    pub source: BackupSkillSource,
    pub files: Vec<BackupFile>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct BackupExcludedSkill {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct BackupSkillSource {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_relative_path: Option<String>,
    pub origin: String,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct BackupFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupArtifact {
    pub id: String,
    pub category: String,
    pub name: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub source_relative_path: String,
    #[serde(default)]
    pub entry_key: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entry_selector: Vec<String>,
    pub files: Vec<BackupFile>,
}

pub fn build_manifest(
    records: &[SkillSourceRecord],
    options: &BackupBuildOptions,
) -> Result<BackupManifest> {
    let mut skills = Vec::new();
    let mut excluded = Vec::new();
    let mut source_paths = BTreeMap::new();
    let mut seen_installations = BTreeSet::new();

    for record in records {
        let name = record.skill_name.trim();
        if name.is_empty() {
            continue;
        }
        let reason = backup_exclusion_reason(&record.skill_path)?;
        if let Some(reason) = reason {
            excluded.push(BackupExcludedSkill {
                name: name.to_string(),
                reason,
            });
            continue;
        }

        let canonical_path = record
            .skill_path
            .canonicalize()
            .unwrap_or_else(|_| record.skill_path.clone());
        if !seen_installations.insert(canonical_path) {
            continue;
        }

        let files = match backup_files(&record.skill_path) {
            Ok(files) => files,
            Err(error) => {
                excluded.push(BackupExcludedSkill {
                    name: name.to_string(),
                    reason: exclusion_reason(&error),
                });
                continue;
            }
        };
        let base_id = backup_skill_id(record);
        let content_id = sha256_hex(
            serde_json::to_string(&files)
                .expect("backup file metadata serializes")
                .as_bytes(),
        );
        let mut id = base_id.clone();
        if skills.iter().any(|skill: &BackupSkill| skill.id == id) {
            id = format!("{base_id}-{}", &content_id[..8]);
        }
        let mut suffix = 2usize;
        while skills.iter().any(|skill: &BackupSkill| skill.id == id) {
            id = format!("{base_id}-{}-{suffix}", &content_id[..8]);
            suffix += 1;
        }
        source_paths.insert(id.clone(), record.skill_path.clone());
        skills.push(BackupSkill {
            id,
            name: name.to_string(),
            source: portable_source(record),
            files,
        });
    }
    skills.sort_by(|left, right| left.id.cmp(&right.id));
    excluded.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.reason.cmp(&right.reason))
    });

    let manifest = BackupManifest {
        version: BACKUP_MANIFEST_VERSION,
        device_label: options.device_label.trim().to_string(),
        skills,
        artifacts: Vec::new(),
        excluded,
        source_paths,
        artifact_source_paths: BTreeMap::new(),
        artifact_contents: BTreeMap::new(),
    };
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub fn build_backup_manifest(
    store: &Store,
    cwd: &Path,
    config: &BackupConfig,
    machine_name: &str,
) -> Result<BackupManifest> {
    let catalog = backup_catalog(store, cwd)?;
    let skill_ids = catalog
        .skills
        .iter()
        .filter(|item| category_item_selected(&config.contents.skills, &item.id))
        .map(|item| item.id.as_str())
        .collect::<BTreeSet<_>>();
    let selected_skill_paths = catalog
        .skills
        .iter()
        .filter(|item| skill_ids.contains(item.id.as_str()))
        .filter_map(|item| item.source_path.as_ref())
        .map(|path| (path.canonicalize().unwrap_or_else(|_| path.clone()), path))
        .collect::<BTreeMap<_, _>>();
    let records = store
        .skill_source_records_for_workspace(cwd)?
        .into_iter()
        .filter(|record| {
            if !config.contents.skills.enabled {
                return false;
            }
            if catalog.skills.is_empty() {
                return true;
            }
            let path = record
                .skill_path
                .canonicalize()
                .unwrap_or_else(|_| record.skill_path.clone());
            selected_skill_paths.contains_key(&path)
        })
        .collect::<Vec<_>>();
    let mut manifest = build_manifest(
        &records,
        &BackupBuildOptions {
            device_label: machine_name.to_string(),
        },
    )?;
    let mut artifact_source_paths = BTreeMap::new();
    let mut artifact_contents = BTreeMap::new();
    for (category, selection, items) in [
        ("mcp", &config.contents.mcp, &catalog.mcp),
        ("rules", &config.contents.rules, &catalog.rules),
        ("hooks", &config.contents.hooks, &catalog.hooks),
    ] {
        if !selection.enabled {
            continue;
        }
        for item in items {
            if !category_item_selected(selection, &item.id) {
                continue;
            }
            let Some(source_path) = item.source_path.as_ref() else {
                continue;
            };
            let Some(agent) = item.agent else {
                continue;
            };
            let Some(source_key) = item.source_key.as_ref() else {
                continue;
            };
            let (file_name, content) = if let Some(entry_key) = item.entry_key.as_deref() {
                let entry = match category {
                    "mcp" => crate::providers::agent_provider(agent).backup_mcp_entry(
                        source_path,
                        &item.entry_selector,
                        entry_key,
                    )?,
                    "hooks" => {
                        let identity = crate::hooks::hook_source_match_from_key(entry_key)?;
                        crate::providers::agent_provider(agent)
                            .backup_hook_entry(source_path, &identity)?
                    }
                    _ => bail!("unsupported entry sync category {category}"),
                };
                let mut content = serde_json::to_vec_pretty(&entry)?;
                content.push(b'\n');
                ("entry.json".to_string(), content)
            } else {
                let content = fs::read(source_path)
                    .with_context(|| format!("failed to read {}", source_path.display()))?;
                let file_name = source_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("source")
                    .to_string();
                (file_name, content)
            };
            let entry_key = item.entry_key.clone().unwrap_or_default();
            let selector =
                serde_json::to_string(&item.entry_selector).expect("MCP server path serializes");
            let identity = format!(
                "{category}:{}:{source_key}:{selector}:{entry_key}",
                agent.label()
            );
            let id = format!("{category}-{}", &sha256_hex(identity.as_bytes())[..16]);
            let artifact = BackupArtifact {
                id: id.clone(),
                category: category.to_string(),
                name: item.label.clone(),
                agent: agent.label().to_string(),
                source_relative_path: source_key.clone(),
                entry_key,
                entry_selector: item.entry_selector.clone(),
                files: vec![BackupFile {
                    path: file_name,
                    sha256: sha256_hex(&content),
                    size: content.len() as u64,
                }],
            };
            let key = artifact_key(category, &id);
            if item.entry_key.is_some() {
                artifact_contents.insert(key, content);
            } else {
                artifact_source_paths.insert(key, source_path.clone());
            }
            manifest.artifacts.push(artifact);
        }
    }
    manifest.artifact_source_paths = artifact_source_paths;
    manifest.artifact_contents = artifact_contents;
    manifest
        .artifacts
        .sort_by(|left, right| left.id.cmp(&right.id));
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn category_item_selected(selection: &BackupCategorySelection, id: &str) -> bool {
    selection.enabled && !selection.excluded.iter().any(|excluded| excluded == id)
}

fn artifact_key(category: &str, id: &str) -> String {
    format!("{category}:{id}")
}

pub fn write_snapshot(manifest: &BackupManifest, destination: &Path) -> Result<()> {
    validate_manifest(manifest)?;
    let _resources = crate::coordination::acquire_file_resources(&[destination.to_path_buf()])?;
    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create backup snapshot {}", destination.display()))?;
    let skills_root = destination.join("skills");
    fs::create_dir_all(&skills_root)?;
    for skill in &manifest.skills {
        let target = skills_root.join(&skill.id);
        let Some(source) = manifest.source_paths.get(&skill.id) else {
            verify_snapshot_skill(skill, &target)?;
            continue;
        };
        if target.exists() {
            fs::remove_dir_all(&target)
                .with_context(|| format!("failed to replace {}", target.display()))?;
        }
        copy_skill_files(skill, source, &target).with_context(|| {
            format!(
                "skill {} changed while preparing backup; retry the backup",
                skill.name
            )
        })?;
    }
    for artifact in &manifest.artifacts {
        let target = destination.join(&artifact.category).join(&artifact.id);
        if let Some(content) = manifest
            .artifact_contents
            .get(&artifact_key(&artifact.category, &artifact.id))
        {
            if target.exists() {
                fs::remove_dir_all(&target)
                    .with_context(|| format!("failed to replace {}", target.display()))?;
            }
            write_artifact_files(artifact, content, &target)?;
        } else if let Some(source) = manifest
            .artifact_source_paths
            .get(&artifact_key(&artifact.category, &artifact.id))
        {
            if target.exists() {
                fs::remove_dir_all(&target)
                    .with_context(|| format!("failed to replace {}", target.display()))?;
            }
            copy_artifact_files(artifact, source, &target)?;
        } else {
            verify_snapshot_artifact(artifact, &target)?;
        }
    }
    fs::write(
        destination.join("manifest.json"),
        serde_json::to_vec_pretty(manifest)?,
    )
    .with_context(|| {
        format!(
            "failed to write backup manifest in {}",
            destination.display()
        )
    })?;
    Ok(())
}

fn merge_manifests(
    existing: Option<BackupManifest>,
    mut local: BackupManifest,
) -> Result<BackupManifest> {
    let Some(mut merged) = existing else {
        return Ok(local);
    };
    merged.device_label = local.device_label.clone();
    for mut skill in local.skills.drain(..) {
        let original_id = skill.id.clone();
        if merged
            .skills
            .iter()
            .any(|existing| existing.files == skill.files)
        {
            continue;
        }
        let content_id = sha256_hex(
            serde_json::to_string(&skill.files)
                .expect("backup file metadata serializes")
                .as_bytes(),
        );
        if merged.skills.iter().any(|existing| existing.id == skill.id) {
            let base = skill.id.clone();
            skill.id = format!("{base}-{}", &content_id[..8]);
            let mut suffix = 2usize;
            while merged.skills.iter().any(|existing| existing.id == skill.id) {
                skill.id = format!("{base}-{}-{suffix}", &content_id[..8]);
                suffix += 1;
            }
        }
        if let Some(source_path) = local.source_paths.remove(&original_id) {
            merged.source_paths.insert(skill.id.clone(), source_path);
        }
        merged.skills.push(skill);
    }
    for mut artifact in local.artifacts.drain(..) {
        let original_id = artifact.id.clone();
        let original_key = artifact_key(&artifact.category, &original_id);
        if merged.artifacts.iter().any(|existing| {
            existing.category == artifact.category
                && existing.agent == artifact.agent
                && existing.source_relative_path == artifact.source_relative_path
                && existing.entry_key == artifact.entry_key
                && existing.entry_selector == artifact.entry_selector
                && existing.files == artifact.files
        }) {
            continue;
        }
        if merged
            .artifacts
            .iter()
            .any(|existing| existing.id == artifact.id)
        {
            let content_id = sha256_hex(
                serde_json::to_string(&artifact.files)
                    .expect("backup artifact metadata serializes")
                    .as_bytes(),
            );
            let base = artifact.id.clone();
            artifact.id = format!("{base}-{}", &content_id[..8]);
            let mut suffix = 2usize;
            while merged
                .artifacts
                .iter()
                .any(|existing| existing.id == artifact.id)
            {
                artifact.id = format!("{base}-{}-{suffix}", &content_id[..8]);
                suffix += 1;
            }
        }
        if let Some(source_path) = local.artifact_source_paths.remove(&original_key) {
            merged
                .artifact_source_paths
                .insert(artifact_key(&artifact.category, &artifact.id), source_path);
        }
        if let Some(content) = local.artifact_contents.remove(&original_key) {
            merged
                .artifact_contents
                .insert(artifact_key(&artifact.category, &artifact.id), content);
        }
        merged.artifacts.push(artifact);
    }
    let mut excluded = merged
        .excluded
        .into_iter()
        .map(|skill| (skill.name, skill.reason))
        .collect::<BTreeSet<_>>();
    excluded.extend(
        local
            .excluded
            .into_iter()
            .map(|skill| (skill.name, skill.reason)),
    );
    merged.excluded = excluded
        .into_iter()
        .map(|(name, reason)| BackupExcludedSkill { name, reason })
        .collect();
    merged.skills.sort_by(|left, right| left.id.cmp(&right.id));
    merged
        .artifacts
        .sort_by(|left, right| left.id.cmp(&right.id));
    validate_manifest(&merged)?;
    Ok(merged)
}

fn verify_snapshot_skill(skill: &BackupSkill, target: &Path) -> Result<()> {
    if !target.is_dir() {
        bail!("backup snapshot is missing files for {}", skill.name);
    }
    for file in &skill.files {
        let path = target.join(safe_relative_path(&file.path)?);
        let content = fs::read(&path)
            .with_context(|| format!("backup snapshot is missing {}", path.display()))?;
        if sha256_hex(&content) != file.sha256 || content.len() as u64 != file.size {
            bail!(
                "backup snapshot integrity check failed for {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn copy_skill_files(skill: &BackupSkill, source: &Path, target: &Path) -> Result<()> {
    for file in &skill.files {
        let relative = safe_relative_path(&file.path)?;
        let source_file = source.join(&relative);
        let target_file = target.join(&relative);
        let content = fs::read(&source_file)
            .with_context(|| format!("failed to read {}", source_file.display()))?;
        if sha256_hex(&content) != file.sha256 || content.len() as u64 != file.size {
            bail!("backup skill content did not match its manifest");
        }
        if let Some(parent) = target_file.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target_file, content)
            .with_context(|| format!("failed to write {}", target_file.display()))?;
    }
    Ok(())
}

fn verify_snapshot_artifact(artifact: &BackupArtifact, target: &Path) -> Result<()> {
    if !target.is_dir() {
        bail!("backup snapshot is missing files for {}", artifact.name);
    }
    for file in &artifact.files {
        let path = target.join(safe_relative_path(&file.path)?);
        let content = fs::read(&path)
            .with_context(|| format!("backup snapshot is missing {}", path.display()))?;
        if sha256_hex(&content) != file.sha256 || content.len() as u64 != file.size {
            bail!(
                "backup snapshot integrity check failed for {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn copy_artifact_files(artifact: &BackupArtifact, source: &Path, target: &Path) -> Result<()> {
    let content =
        fs::read(source).with_context(|| format!("failed to read {}", source.display()))?;
    write_artifact_files(artifact, &content, target)
}

fn write_artifact_files(artifact: &BackupArtifact, content: &[u8], target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
    for file in &artifact.files {
        if sha256_hex(content) != file.sha256 || content.len() as u64 != file.size {
            bail!("backup artifact content did not match its manifest");
        }
        let target_file = target.join(safe_relative_path(&file.path)?);
        if let Some(parent) = target_file.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target_file, content)
            .with_context(|| format!("failed to write {}", target_file.display()))?;
    }
    Ok(())
}

pub fn validate_manifest(manifest: &BackupManifest) -> Result<()> {
    if manifest.version != BACKUP_MANIFEST_VERSION {
        bail!("unsupported backup manifest version {}", manifest.version);
    }
    let mut ids = BTreeSet::new();
    for skill in &manifest.skills {
        if skill.id.trim().is_empty() || skill.id.contains('/') || skill.id.contains('\\') {
            bail!("backup manifest contains an invalid skill id");
        }
        if !ids.insert(&skill.id) {
            bail!("backup manifest contains duplicate skill id {}", skill.id);
        }
        if skill.files.is_empty() {
            bail!("backup manifest skill {} is missing files", skill.name);
        }
        let mut paths = BTreeSet::new();
        for file in &skill.files {
            safe_relative_path(&file.path)?;
            if !paths.insert(&file.path) {
                bail!(
                    "backup manifest skill {} contains duplicate file paths",
                    skill.name
                );
            }
            if file.sha256.len() != 64 || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                bail!(
                    "backup manifest skill {} contains an invalid file hash",
                    skill.name
                );
            }
        }
    }
    let mut artifact_ids = BTreeSet::new();
    for artifact in &manifest.artifacts {
        if artifact.id.trim().is_empty()
            || artifact.id.contains('/')
            || artifact.id.contains('\\')
            || artifact.category.trim().is_empty()
            || artifact.category.contains('/')
            || artifact.category.contains('\\')
        {
            bail!("backup manifest contains an invalid artifact id");
        }
        if !artifact_ids.insert(&artifact.id) {
            bail!(
                "backup manifest contains duplicate artifact id {}",
                artifact.id
            );
        }
        if artifact.files.is_empty() {
            bail!(
                "backup manifest artifact {} is missing files",
                artifact.name
            );
        }
        if !artifact.source_relative_path.is_empty() {
            safe_relative_path(&artifact.source_relative_path)?;
        }
        if !artifact.entry_key.is_empty()
            && artifact.category != "mcp"
            && artifact.category != "hooks"
        {
            bail!(
                "backup manifest artifact {} has an entry key for unsupported category {}",
                artifact.name,
                artifact.category
            );
        }
        if artifact
            .entry_selector
            .iter()
            .any(|component| component.is_empty())
        {
            bail!(
                "backup manifest artifact {} contains an invalid MCP server path",
                artifact.name
            );
        }
        let mut paths = BTreeSet::new();
        for file in &artifact.files {
            safe_relative_path(&file.path)?;
            if !paths.insert(&file.path) {
                bail!(
                    "backup manifest artifact {} contains duplicate file paths",
                    artifact.name
                );
            }
            if file.sha256.len() != 64 || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                bail!(
                    "backup manifest artifact {} contains an invalid file hash",
                    artifact.name
                );
            }
        }
    }
    Ok(())
}

fn backup_exclusion_reason(skill_path: &Path) -> Result<Option<String>> {
    if !skill_path.join("SKILL.md").is_file() {
        return Ok(Some("missing-skill-file".to_string()));
    }
    if is_inside_git_worktree(skill_path) {
        return Ok(Some("project-repository".to_string()));
    }
    Ok(None)
}

fn is_inside_git_worktree(path: &Path) -> bool {
    git::local_repository_snapshot(path, git::never_cancelled())
        .map(|snapshot| snapshot.repo_root.is_some())
        .unwrap_or(false)
}

fn backup_files(skill_path: &Path) -> Result<Vec<BackupFile>> {
    let mut files = Vec::new();
    let mut total_bytes = 0u64;
    let walker = WalkDir::new(skill_path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry
                .path()
                .strip_prefix(skill_path)
                .map(|relative| !should_skip(relative))
                .unwrap_or(false)
        });
    for entry in walker {
        let entry = entry.with_context(|| format!("failed to inspect {}", skill_path.display()))?;
        if entry.depth() == 0 {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(skill_path)
            .context("backup walk escaped skill root")?;
        if entry.file_type().is_symlink() {
            bail!("unsupported-symlink");
        }
        if entry.file_type().is_dir() {
            continue;
        }
        if is_sensitive_file(relative) {
            bail!("sensitive-content");
        }
        let bytes = fs::read(entry.path())
            .with_context(|| format!("failed to read {}", entry.path().display()))?;
        if contains_secret(&bytes) {
            bail!("sensitive-content");
        }
        total_bytes = total_bytes.saturating_add(bytes.len() as u64);
        if total_bytes > MAX_SKILL_BYTES {
            bail!("size-limit");
        }
        if files.len() >= MAX_SKILL_FILES {
            bail!("file-limit");
        }
        files.push(BackupFile {
            path: path_to_manifest(relative)?,
            sha256: sha256_hex(&bytes),
            size: bytes.len() as u64,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    if files.is_empty() {
        bail!("missing-files");
    }
    Ok(files)
}

fn should_skip(path: &Path) -> bool {
    path.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        matches!(
            name.to_str(),
            Some(".git" | "node_modules" | ".cache" | "__pycache__" | "tmp" | "temp")
        )
    })
}

fn is_sensitive_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    let lower = name.to_ascii_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || matches!(
            lower.as_str(),
            "credentials"
                | "credentials.json"
                | "secrets"
                | "secrets.json"
                | "id_rsa"
                | "id_ed25519"
        )
        || matches!(
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.to_ascii_lowercase())
                .as_deref(),
            Some("pem" | "key" | "p12" | "pfx" | "keystore")
        )
}

fn contains_secret(content: &[u8]) -> bool {
    contains_prefixed_token(content, b"ghp_", 20)
        || contains_prefixed_token(content, b"github_pat_", 20)
        || contains_prefixed_token(content, b"sk-", 20)
        || contains_prefixed_token(content, b"AKIA", 16)
        || content
            .windows(b"-----BEGIN PRIVATE KEY-----".len())
            .any(|window| window == b"-----BEGIN PRIVATE KEY-----")
        || content
            .windows(b"-----BEGIN RSA PRIVATE KEY-----".len())
            .any(|window| window == b"-----BEGIN RSA PRIVATE KEY-----")
}

fn contains_prefixed_token(content: &[u8], prefix: &[u8], minimum_suffix_len: usize) -> bool {
    content
        .windows(prefix.len())
        .enumerate()
        .any(|(index, window)| {
            if window != prefix || (index > 0 && content[index - 1].is_ascii_alphanumeric()) {
                return false;
            }
            content[index + prefix.len()..]
                .iter()
                .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                .count()
                >= minimum_suffix_len
        })
}

fn backup_skill_id(record: &SkillSourceRecord) -> String {
    if record.source_kind == "tendi-backup" {
        if let Some(id) = record
            .source_relative_path
            .as_deref()
            .and_then(|path| path.strip_prefix("skills/"))
        {
            if !id.is_empty() && !id.contains('/') && !id.contains('\\') {
                return id.to_string();
            }
        }
    }
    let slug = record
        .skill_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() {
        "skill".to_string()
    } else {
        slug
    }
}

fn portable_source(record: &SkillSourceRecord) -> BackupSkillSource {
    BackupSkillSource {
        kind: record.source_kind.clone(),
        source: record
            .source
            .as_deref()
            .filter(|value| !Path::new(value).is_absolute() && !has_embedded_credentials(value))
            .map(str::to_string),
        source_ref: record.source_ref.clone(),
        source_version: record.source_version.clone(),
        source_relative_path: record
            .source_relative_path
            .as_deref()
            .filter(|value| !Path::new(value).is_absolute())
            .map(str::to_string),
        origin: record.origin.clone(),
    }
}

fn has_embedded_credentials(value: &str) -> bool {
    let Some((scheme, remainder)) = value.split_once("://") else {
        return false;
    };
    let authority = remainder.split('/').next().unwrap_or_default();
    let Some((userinfo, _)) = authority.rsplit_once('@') else {
        return false;
    };
    matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") || userinfo.contains(':')
}

fn exclusion_reason(error: &anyhow::Error) -> String {
    let message = error.to_string();
    [
        "sensitive-content",
        "size-limit",
        "file-limit",
        "unsupported-symlink",
        "missing-files",
    ]
    .iter()
    .find(|reason| message.contains(**reason))
    .map(|reason| (*reason).to_string())
    .unwrap_or_else(|| "unreadable".to_string())
}

fn safe_relative_path(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("backup manifest file path must be relative and contained");
    }
    Ok(path.to_path_buf())
}

fn path_to_manifest(path: &Path) -> Result<String> {
    let path = safe_relative_path(path.to_str().context("backup paths must be valid UTF-8")?)?;
    Ok(path.to_string_lossy().replace('\\', "/"))
}

fn sha256_hex(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

fn ensure_checkout(config: &BackupConfig) -> Result<bool> {
    config.validate()?;
    let remote_url = normalize_remote_url(&config.remote_url);
    let is_git_checkout = config.checkout_path.exists()
        && discover_git_repository_root(&config.checkout_path)?.is_some();
    if !is_git_checkout {
        fs::create_dir_all(&config.checkout_path)
            .with_context(|| format!("failed to create {}", config.checkout_path.display()))?;
        run_git_success(
            &config.checkout_path,
            ["init", "--initial-branch=main"],
            false,
        )?;
        run_git_success(
            &config.checkout_path,
            ["config", "user.email", "backup@tendi.local"],
            false,
        )?;
        if !remote_url.is_empty() {
            run_git_success(
                &config.checkout_path,
                ["remote", "add", "origin", &remote_url],
                false,
            )?;
        }
        return checkout_has_remote(&config.checkout_path);
    }
    if !remote_url.is_empty() {
        let remote = git_output(
            &config.checkout_path,
            ["remote", "get-url", "origin"],
            false,
        )?;
        if remote.status.success() {
            let current = String::from_utf8_lossy(&remote.stdout).trim().to_string();
            if current != remote_url {
                run_git_success(
                    &config.checkout_path,
                    ["remote", "set-url", "origin", &remote_url],
                    false,
                )?;
            }
        } else {
            run_git_success(
                &config.checkout_path,
                ["remote", "add", "origin", &remote_url],
                false,
            )?;
        }
    }
    checkout_has_remote(&config.checkout_path)
}

fn checkout_has_remote(checkout: &Path) -> Result<bool> {
    let remote = git_output(checkout, ["remote", "get-url", "origin"], false)?;
    Ok(remote.status.success() && !String::from_utf8_lossy(&remote.stdout).trim().is_empty())
}

fn synchronize_remote_checkout(
    config: &BackupConfig,
) -> Result<(Option<BackupManifest>, Option<(BackupManifest, PathBuf)>)> {
    if checkout_has_conflicts(&config.checkout_path) {
        bail!("backup checkout has unresolved Git conflicts");
    }
    let current_manifest = read_checkout_manifest(&config.checkout_path)?;
    run_git_success(&config.checkout_path, ["fetch", "origin"], true)?;
    let remote_head = git_output(
        &config.checkout_path,
        ["rev-parse", "--verify", "refs/remotes/origin/main"],
        false,
    )?;
    if !remote_head.status.success() {
        return Ok((current_manifest, None));
    }
    let local_head = git_output(
        &config.checkout_path,
        ["rev-parse", "--verify", "HEAD"],
        false,
    )?;
    if !local_head.status.success() {
        run_git_success(
            &config.checkout_path,
            ["checkout", "-B", "main", "origin/main"],
            false,
        )?;
        return Ok((read_checkout_manifest(&config.checkout_path)?, None));
    }
    if git_is_ancestor(&config.checkout_path, "HEAD", "origin/main")? {
        run_git_success(
            &config.checkout_path,
            ["checkout", "-B", "main", "origin/main"],
            false,
        )?;
        return Ok((read_checkout_manifest(&config.checkout_path)?, None));
    }
    if git_is_ancestor(&config.checkout_path, "origin/main", "HEAD")? {
        return Ok((current_manifest, None));
    }

    let manifest = current_manifest.context("diverged backup checkout is missing manifest.json")?;
    let (preserved, temporary_root) = capture_checkout_manifest(&manifest, &config.checkout_path)?;
    if let Err(error) = run_git_success(
        &config.checkout_path,
        ["checkout", "-B", "main", "origin/main"],
        false,
    ) {
        let _ = fs::remove_dir_all(&temporary_root);
        return Err(error.context("failed to switch a diverged backup checkout to the remote head"));
    }
    Ok((
        read_checkout_manifest(&config.checkout_path)?,
        Some((preserved, temporary_root)),
    ))
}

fn git_is_ancestor(checkout: &Path, first: &str, second: &str) -> Result<bool> {
    let output = git_output(
        checkout,
        ["merge-base", "--is-ancestor", first, second],
        false,
    )?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("failed to compare backup Git history"),
    }
}

fn checkout_needs_push(checkout: &Path) -> Result<bool> {
    let local_head = git_output(checkout, ["rev-parse", "--verify", "HEAD"], false)?;
    if !local_head.status.success() {
        return Ok(false);
    }
    let remote_head = git_output(
        checkout,
        ["rev-parse", "--verify", "refs/remotes/origin/main"],
        false,
    )?;
    if !remote_head.status.success() {
        return Ok(true);
    }
    Ok(!git_is_ancestor(checkout, "HEAD", "origin/main")?)
}

fn capture_checkout_manifest(
    manifest: &BackupManifest,
    checkout: &Path,
) -> Result<(BackupManifest, PathBuf)> {
    let temporary_root = std::env::temp_dir().join(format!(
        "tendi-skill-backup-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    ));
    fs::create_dir_all(&temporary_root)
        .with_context(|| format!("failed to create {}", temporary_root.display()))?;
    let mut preserved = manifest.clone();
    for skill in &manifest.skills {
        let source = checkout.join("skills").join(&skill.id);
        let target = temporary_root.join(&skill.id);
        if let Err(error) = copy_skill_files(skill, &source, &target) {
            let _ = fs::remove_dir_all(&temporary_root);
            return Err(error);
        }
        preserved.source_paths.insert(skill.id.clone(), target);
    }
    for artifact in &manifest.artifacts {
        let Some(file) = artifact.files.first() else {
            continue;
        };
        let source = checkout
            .join(&artifact.category)
            .join(&artifact.id)
            .join(safe_relative_path(&file.path)?);
        let content =
            fs::read(&source).with_context(|| format!("failed to read {}", source.display()))?;
        if sha256_hex(&content) != file.sha256 || content.len() as u64 != file.size {
            let _ = fs::remove_dir_all(&temporary_root);
            bail!("backup artifact content did not match its manifest");
        }
        preserved
            .artifact_contents
            .insert(artifact_key(&artifact.category, &artifact.id), content);
    }
    Ok((preserved, temporary_root))
}

fn commit_checkout(
    config: &BackupConfig,
    manifest: &BackupManifest,
    machine_name: &str,
) -> Result<bool> {
    run_git_success(&config.checkout_path, ["add", "--all"], false)?;
    let diff = git_output(
        &config.checkout_path,
        ["diff", "--cached", "--quiet"],
        false,
    )?;
    if diff.status.success() {
        return Ok(false);
    }
    if diff.status.code() != Some(1) {
        bail!("failed to determine whether the backup checkout changed");
    }
    run_git_success(
        &config.checkout_path,
        [
            "config",
            "user.name",
            &format!("Tendi Backup ({machine_name})"),
        ],
        false,
    )?;
    let message = format!(
        "backup: {} skill{} and {} configuration source{} from {}",
        manifest.skills.len(),
        if manifest.skills.len() == 1 { "" } else { "s" },
        manifest.artifacts.len(),
        if manifest.artifacts.len() == 1 {
            ""
        } else {
            "s"
        },
        machine_name,
    );
    run_git_success(&config.checkout_path, ["commit", "-m", &message], false)?;
    Ok(true)
}

fn current_commit(checkout: &Path) -> Result<String> {
    let output = run_git_success(checkout, ["rev-parse", "HEAD"], false)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn checkout_has_conflicts(checkout: &Path) -> bool {
    if !checkout.join(".git").is_dir() {
        return false;
    }
    git_output(checkout, ["diff", "--name-only", "--diff-filter=U"], false)
        .map(|output| !output.stdout.is_empty())
        .unwrap_or(false)
}

fn read_checkout_manifest(checkout: &Path) -> Result<Option<BackupManifest>> {
    let path = checkout.join("manifest.json");
    if !path.is_file() {
        return Ok(None);
    }
    let manifest = serde_json::from_slice::<BackupManifest>(&fs::read(&path)?)
        .with_context(|| format!("invalid backup manifest {}", path.display()))?;
    validate_manifest(&manifest)?;
    Ok(Some(manifest))
}

fn manifest_at_revision(checkout: &Path, revision: &str) -> Result<BackupManifest> {
    let output = run_git_success(
        checkout,
        ["show", &format!("{revision}:manifest.json")],
        false,
    )?;
    let manifest = serde_json::from_slice::<BackupManifest>(&output.stdout)
        .context("backup version contains an invalid manifest")?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn is_commit_id(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn restore_folder_name(skill: &BackupSkill, reserved: &BTreeSet<String>) -> String {
    let base = skill
        .name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    let base = if base.is_empty() {
        "skill".to_string()
    } else {
        base
    };
    if !reserved.contains(&base) {
        return base;
    }
    format!("{base}-{}", &skill.id[skill.id.len().saturating_sub(8)..])
}

fn unique_restore_target(target: &Path, reserved: &BTreeSet<PathBuf>) -> Result<PathBuf> {
    let parent = target
        .parent()
        .context("restore target has no parent directory")?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .context("restore target name is not valid UTF-8")?;
    for suffix in std::iter::once(String::new()).chain((2..).map(|index| format!("-{index}"))) {
        let candidate = parent.join(format!("{name}-restored{suffix}"));
        if !candidate.exists() && !reserved.contains(&candidate) {
            return Ok(candidate);
        }
    }
    unreachable!("unbounded restore target suffix iterator always yields a candidate")
}

fn unique_restore_file_target(target: &Path, reserved: &BTreeSet<PathBuf>) -> Result<PathBuf> {
    let parent = target
        .parent()
        .context("restore target has no parent directory")?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .context("restore target name is not valid UTF-8")?;
    for suffix in std::iter::once(String::new()).chain((2..).map(|index| format!("-{index}"))) {
        let candidate = parent.join(format!("{name}.restored{suffix}"));
        if !candidate.exists() && !reserved.contains(&candidate) {
            return Ok(candidate);
        }
    }
    unreachable!("unbounded restore file suffix iterator always yields a candidate")
}

fn remove_restore_target(target_root: &Path, target: &Path) -> Result<()> {
    if target.parent() != Some(target_root) {
        bail!("backup restore refused to replace a target outside the selected skills directory");
    }
    let metadata = fs::symlink_metadata(target)
        .with_context(|| format!("failed to inspect restore target {}", target.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("backup restore only replaces an existing skill directory");
    }
    fs::remove_dir_all(target)
        .with_context(|| format!("failed to replace restore target {}", target.display()))
}

fn run_git_success<I, S>(checkout: &Path, args: I, network: bool) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = git_output(checkout, args, network)?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "Git backup operation failed{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
    }
    Ok(output)
}

fn git_output<I, S>(checkout: &Path, args: I, network: bool) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    git::run_git(
        checkout,
        args,
        if network {
            git::NETWORK_COMMAND_TIMEOUT
        } else {
            git::LOCAL_COMMAND_TIMEOUT
        },
        git::never_cancelled(),
    )
    .map_err(Into::into)
}

#[cfg(test)]
#[path = "skill_backup_tests.rs"]
mod tests;
