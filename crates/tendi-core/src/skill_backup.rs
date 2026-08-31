use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::{git, skills::SkillSourceRecord, storage::Store, SkillInstallScope, SkillTarget};

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

pub fn apply_backup_restore(
    plan: &BackupRestorePlan,
    store: &Store,
    workspace_root: &Path,
    resolutions: &[BackupRestoreResolution],
) -> Result<Vec<BackupRestoreOperation>> {
    let result = apply_backup_restore_without_database(plan, resolutions)?;
    store.upsert_skill_source_records_for_workspace(workspace_root, &result.source_records)?;
    Ok(result.operations)
}

pub fn apply_backup_restore_without_database(
    plan: &BackupRestorePlan,
    resolutions: &[BackupRestoreResolution],
) -> Result<BackupRestoreApplyResult> {
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
                fs::write(&operation.target, content).with_context(|| {
                    format!("failed to restore {}", operation.target.display())
                })?;
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
                        let identity = crate::hooks::hook_source_match_from_key(&artifact.entry_key)?;
                        provider.restore_hook_entry(&operation.target, &identity, &entry)?
                    }
                    _ => bail!("unsupported entry sync category {}", artifact.category),
                };
                crate::fsutil::atomic_write(&operation.target, &merged).with_context(|| {
                    format!("failed to restore {}", operation.target.display())
                })?;
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
    let record = skill_backup_record_for_adoption(skill_path, name)?;
    store.upsert_skill_source_records_for_workspace(workspace_root, std::slice::from_ref(&record))?;
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
        let mut skills = BTreeMap::<String, (BTreeSet<String>, String)>::new();
        for skill in scan.skills {
            let global_paths = skill
                .paths
                .iter()
                .filter(|path| path.scope == "global")
                .map(|path| path.path.display().to_string())
                .collect::<BTreeSet<_>>();
            if !global_paths.is_empty() && !skill.is_system {
                skills.insert(
                    skill.name,
                    (global_paths, skill.description.unwrap_or_default()),
                );
            }
        }
        catalog.skills = skills
            .into_iter()
            .map(|(name, (_paths, detail))| BackupCatalogItem {
                id: format!("skill:{name}"),
                label: name,
                detail,
                source_path: None,
                agent: None,
                source_key: None,
                entry_key: None,
                entry_selector: Vec::new(),
            })
            .collect();
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
    let mut grouped = BTreeMap::<
        (crate::skills::AgentKind, PathBuf),
        BTreeMap<String, BTreeSet<String>>,
    >::new();
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
        if skills
            .iter()
            .any(|skill: &BackupSkill| skill.files == files)
        {
            // Exact copies share one canonical directory, even if different agents
            // discover them at different local paths.
            continue;
        }
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
    let records = store
        .skill_source_records_for_workspace(cwd)?
        .into_iter()
        .filter(|record| {
            config.contents.skills.enabled
                && (catalog.skills.is_empty()
                    || skill_ids.contains(format!("skill:{}", record.skill_name).as_str()))
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
            let selector = serde_json::to_string(&item.entry_selector)
                .expect("MCP server path serializes");
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
    let content = fs::read(source)
        .with_context(|| format!("failed to read {}", source.display()))?;
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
        if artifact.entry_selector.iter().any(|component| component.is_empty()) {
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
        let content = fs::read(&source)
            .with_context(|| format!("failed to read {}", source.display()))?;
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
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        adopt_skill_for_backup, backup_now, backup_statuses_for_paths, backup_versions,
        build_manifest, catalog_source_files, current_machine_name, discover_git_repository_root,
        ensure_checkout,
        is_remote_repository, normalize_remote_url, sync_checkout_for_restore,
        validate_manifest, write_snapshot,
        BackupBuildOptions, BackupConfig, BackupManifest,
    };
    use crate::{
        skills::{AgentKind, SkillSourceRecord},
        storage::Store,
        SkillInstallScope,
        SkillTarget,
    };

    #[test]
    fn backup_statuses_skip_all_skill_work_without_a_configured_repository() {
        let root = temp_dir("tendi-skill-backup-status-no-config");
        let skill = root.join("global/review");
        write_skill(&skill, "review");
        let store = Store::open(root.join("tendi.sqlite3")).unwrap();
        store
            .upsert_skill_source_records_for_workspace(&root, &[source("review", &skill)])
            .unwrap();

        let statuses =
            backup_statuses_for_paths(&store, &root, std::slice::from_ref(&skill)).unwrap();

        assert!(statuses.is_empty());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_source_items_use_file_titles_for_rules() {
        let items = catalog_source_files(
            vec![(
                AgentKind::Codex,
                PathBuf::from("/tmp/AGENTS.md"),
                "AGENTS.md".to_string(),
                String::new(),
            )],
            "rules",
        );

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "AGENTS.md");
        assert_eq!(items[0].detail, "");
    }

    #[test]
    fn manifest_contains_only_managed_non_project_skills_and_snapshot_is_self_contained() {
        let root = temp_dir("tendi-skill-backup-manifest");
        let global = root.join("global/review");
        write_skill(&global, "review");
        fs::write(global.join("helper.ts"), "export const review = true;\n").unwrap();
        fs::create_dir_all(global.join("node_modules/package")).unwrap();
        fs::write(global.join("node_modules/package/index.js"), "ignored").unwrap();

        let project = root.join("project/.agents/skills/project-only");
        write_skill(&project, "project-only");
        run_git(&root.join("project"), &["init"]);

        let options = BackupBuildOptions {
            device_label: "Test Mac".to_string(),
        };
        let manifest = build_manifest(
            &[source("review", &global), source("project-only", &project)],
            &options,
        )
        .unwrap();

        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.device_label, "Test Mac");
        assert_eq!(manifest.skills.len(), 1);
        assert_eq!(manifest.skills[0].name, "review");
        assert_eq!(manifest.excluded.len(), 1);
        assert_eq!(manifest.excluded[0].reason, "project-repository");

        let snapshot = root.join("snapshot");
        write_snapshot(&manifest, &snapshot).unwrap();
        assert!(snapshot.join("manifest.json").is_file());
        assert!(snapshot
            .join("skills")
            .join(&manifest.skills[0].id)
            .join("SKILL.md")
            .is_file());
        assert!(snapshot
            .join("skills")
            .join(&manifest.skills[0].id)
            .join("helper.ts")
            .is_file());
        assert!(!snapshot
            .join("skills")
            .join(&manifest.skills[0].id)
            .join("node_modules")
            .exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sensitive_content_excludes_the_whole_skill() {
        let root = temp_dir("tendi-skill-backup-sensitive");
        let skill = root.join("global/deploy");
        write_skill(&skill, "deploy");
        fs::write(skill.join(".env"), "API_TOKEN=super-secret\n").unwrap();

        let manifest =
            build_manifest(&[source("deploy", &skill)], &BackupBuildOptions::default()).unwrap();

        assert!(manifest.skills.is_empty());
        assert_eq!(manifest.excluded[0].reason, "sensitive-content");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn token_scanner_blocks_real_tokens_without_rejecting_ordinary_skill_text() {
        let root = temp_dir("tendi-skill-backup-token-scanner");
        let ordinary = root.join("global/task-review");
        let sensitive = root.join("global/api-client");
        write_skill(&ordinary, "task-review");
        fs::write(
            ordinary.join("SKILL.md"),
            "---\nname: task-review\n---\nUse task-based review steps.\n",
        )
        .unwrap();
        write_skill(&sensitive, "api-client");
        fs::write(
            sensitive.join("token.txt"),
            "sk-proj-123456789012345678901234567890\n",
        )
        .unwrap();

        let manifest = build_manifest(
            &[
                source("task-review", &ordinary),
                source("api-client", &sensitive),
            ],
            &BackupBuildOptions::default(),
        )
        .unwrap();

        assert_eq!(manifest.skills.len(), 1);
        assert_eq!(manifest.skills[0].name, "task-review");
        assert_eq!(manifest.excluded[0].reason, "sensitive-content");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn matching_skill_content_is_canonicalized_without_storing_local_paths() {
        let root = temp_dir("tendi-skill-backup-canonical");
        let first = root.join("one/review");
        let second = root.join("two/review");
        let distinct = root.join("three/review");
        write_skill(&first, "review");
        write_skill(&second, "review");
        write_skill(&distinct, "review");
        fs::write(
            distinct.join("SKILL.md"),
            "---\nname: review\n---\n# changed\n",
        )
        .unwrap();

        let manifest = build_manifest(
            &[
                source("review", &first),
                source("review", &second),
                source("review", &distinct),
            ],
            &BackupBuildOptions::default(),
        )
        .unwrap();

        assert_eq!(manifest.skills.len(), 2);
        assert!(manifest.skills.iter().any(|skill| skill.id == "review"));
        assert!(manifest
            .skills
            .iter()
            .any(|skill| skill.id.starts_with("review-")));
        let serialized = serde_json::to_string(&manifest).unwrap();
        assert!(!serialized.contains(&first.display().to_string()));
        assert!(!serialized.contains(&second.display().to_string()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backup_config_rejects_a_remote_with_embedded_credentials() {
        let error = BackupConfig::new(
            "https://token@example.com/tendi-skills.git",
            PathBuf::from("/tmp/tendi-skill-backup"),
        )
        .validate()
        .unwrap_err();

        assert!(error.to_string().contains("credentials"));
    }

    #[test]
    fn repository_input_distinguishes_remote_urls_from_local_paths() {
        assert!(is_remote_repository("git@github.com:rhinoc/skills.git"));
        assert!(is_remote_repository("rhinoc/skills"));
        assert!(is_remote_repository("github.com/rhinoc/skills"));
        assert!(is_remote_repository("https://github.com/rhinoc/skills.git"));
        assert!(!is_remote_repository("/tmp/tendi-skill-backup"));
        assert!(!is_remote_repository("./tendi-skill-backup"));
    }

    #[test]
    fn local_checkout_is_created_and_keeps_commits_local_without_an_origin() {
        let root = temp_dir("tendi-backup-local-checkout");
        let checkout = root.join("nested/checkout");
        let config = BackupConfig::new("", checkout.clone());

        assert_eq!(discover_git_repository_root(&checkout).unwrap(), None);
        assert!(!ensure_checkout(&config).unwrap());
        assert!(checkout.join(".git").exists());

        let remote = Command::new("git")
            .args(["-C", checkout.to_str().unwrap(), "remote"])
            .output()
            .unwrap();
        assert!(remote.status.success());
        assert!(String::from_utf8_lossy(&remote.stdout).trim().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_backup_creates_a_commit_without_pushing() {
        let root = temp_dir("tendi-backup-local-only");
        let skill = root.join("global/review");
        let checkout = root.join("backup");
        write_skill(&skill, "review");
        let store = Store::open(root.join("tendi.sqlite3")).unwrap();
        store
            .upsert_skill_source_records_for_workspace(&root, &[source("review", &skill)])
            .unwrap();
        store
            .save_skill_backup_config(&BackupConfig::new("", checkout.clone()))
            .unwrap();

        let report = backup_now(&store, &root).unwrap();

        assert!(report.commit.is_some());
        assert!(!report.pushed);
        let machine_name = current_machine_name().unwrap();
        let commit = Command::new("git")
            .args([
                "-C",
                checkout.to_str().unwrap(),
                "log",
                "-1",
                "--format=%an|%s",
            ])
            .output()
            .unwrap();
        let commit_text = String::from_utf8_lossy(&commit.stdout);
        assert!(commit_text.contains(&format!("Tendi Backup ({machine_name})")));
        assert!(commit_text.contains(&format!("from {machine_name}")));
        assert_eq!(backup_versions(&store, 1).unwrap().len(), 1);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backup_remote_validation_rejects_an_unreachable_repository() {
        let root = temp_dir("tendi-backup-remote-validation");
        fs::create_dir_all(&root).unwrap();

        let error = super::validate_remote(&root.join("missing.git").display().to_string(), &root)
            .unwrap_err();

        assert!(error.to_string().contains("not reachable"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backup_config_normalizes_github_remote_shorthands() {
        for remote in [
            "rhinoc/skills",
            "github.com/rhinoc/skills",
            "rhinoc/skills.git",
        ] {
            let config = BackupConfig::new(remote, PathBuf::from("/tmp/tendi-skill-backup"));

            assert_eq!(config.remote_url, "https://github.com/rhinoc/skills.git");
        }

        assert_eq!(
            normalize_remote_url("https://github.com/rhinoc/skills.git"),
            "https://github.com/rhinoc/skills.git"
        );
        assert_eq!(
            normalize_remote_url("git@github.com:rhinoc/skills.git"),
            "git@github.com:rhinoc/skills.git"
        );
        assert_eq!(normalize_remote_url("../skills"), "../skills");
    }

    #[test]
    fn existing_backup_checkout_updates_a_github_shorthand_origin() {
        let root = temp_dir("tendi-backup-normalized-origin");
        let checkout = root.join("checkout");
        let config = BackupConfig {
            remote_url: "rhinoc/skills".to_string(),
            checkout_path: checkout.clone(),
            contents: Default::default(),
        };

        ensure_checkout(&config).unwrap();

        let remote = Command::new("git")
            .args([
                "-C",
                checkout.to_str().unwrap(),
                "remote",
                "get-url",
                "origin",
            ])
            .output()
            .unwrap();
        assert!(remote.status.success());
        assert_eq!(
            String::from_utf8_lossy(&remote.stdout).trim(),
            "https://github.com/rhinoc/skills.git"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backup_config_includes_all_global_categories_by_default() {
        let config = BackupConfig::new(
            "git@github.com:you/tendi-backup.git",
            PathBuf::from("/tmp/tendi-backup"),
        );

        assert!(config.contents.skills.enabled);
        assert!(config.contents.mcp.enabled);
        assert!(config.contents.rules.enabled);
        assert!(config.contents.hooks.enabled);
        assert!(config.contents.skills.excluded.is_empty());
    }

    #[test]
    fn snapshot_writes_rules_files_under_category_roots() {
        let root = temp_dir("tendi-backup-category-roots");
        let source = root.join("AGENTS.md");
        let content = b"# Team rules\n";
        fs::create_dir_all(&root).unwrap();
        fs::write(&source, content).unwrap();
        let artifact_id = "rules-agents".to_string();
        let mut artifact_source_paths = std::collections::BTreeMap::new();
        artifact_source_paths.insert(format!("rules:{artifact_id}"), source);
        let manifest = BackupManifest {
            version: 1,
            device_label: "Test Mac".to_string(),
            skills: Vec::new(),
            artifacts: vec![super::BackupArtifact {
                id: artifact_id.clone(),
                category: "rules".to_string(),
                name: "AGENTS.md".to_string(),
                agent: "codex".to_string(),
                source_relative_path: "AGENTS.md".to_string(),
                entry_key: String::new(),
                entry_selector: Vec::new(),
                files: vec![super::BackupFile {
                    path: "AGENTS.md".to_string(),
                    sha256: super::sha256_hex(content),
                    size: content.len() as u64,
                }],
            }],
            excluded: Vec::new(),
            source_paths: Default::default(),
            artifact_source_paths,
            artifact_contents: Default::default(),
        };
        let snapshot = root.join("snapshot");

        write_snapshot(&manifest, &snapshot).unwrap();

        assert!(snapshot
            .join("rules")
            .join(&artifact_id)
            .join("AGENTS.md")
            .is_file());
        assert!(!snapshot.join("global").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_device_can_prepare_remote_checkout_for_restore() {
        let root = temp_dir("tendi-backup-new-device-restore");
        let skill = root.join("first/global/review");
        write_skill(&skill, "review");
        let remote = root.join("remote.git");
        fs::create_dir_all(&root).unwrap();
        run_git(&root, &["init", "--bare", "remote.git"]);

        let first_store = Store::open(root.join("first.sqlite3")).unwrap();
        first_store
            .upsert_skill_source_records_for_workspace(&root, &[source("review", &skill)])
            .unwrap();
        first_store
            .save_skill_backup_config(&BackupConfig::new(
                remote.display().to_string(),
                root.join("first-checkout"),
            ))
            .unwrap();
        backup_now(&first_store, &root).unwrap();

        let second_config = BackupConfig::new(
            remote.display().to_string(),
            root.join("second-checkout"),
        );
        let manifest = sync_checkout_for_restore(&second_config).unwrap();

        assert_eq!(manifest.unwrap().skills[0].name, "review");
        assert!(second_config.checkout_path.join("manifest.json").is_file());
        drop(first_store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_plan_applies_a_rules_file_artifact() {
        let root = temp_dir("tendi-backup-artifact-restore");
        fs::create_dir_all(&root).unwrap();
        let checkout = root.join("checkout");
        run_git(&root, &["init", "checkout"]);
        run_git(&checkout, &["config", "user.email", "test@example.com"]);
        run_git(&checkout, &["config", "user.name", "Tendi test"]);
        let source = root.join("AGENTS.md");
        let content = b"# Restored rules\n";
        fs::write(&source, content).unwrap();
        let artifact_id = "rules-agents".to_string();
        let mut artifact_source_paths = std::collections::BTreeMap::new();
        artifact_source_paths.insert(format!("rules:{artifact_id}"), source);
        let artifact = super::BackupArtifact {
            id: artifact_id.clone(),
            category: "rules".to_string(),
            name: "AGENTS.md".to_string(),
            agent: "codex".to_string(),
            source_relative_path: "AGENTS.md".to_string(),
            entry_key: String::new(),
            entry_selector: Vec::new(),
            files: vec![super::BackupFile {
                path: "AGENTS.md".to_string(),
                sha256: super::sha256_hex(content),
                size: content.len() as u64,
            }],
        };
        let manifest = BackupManifest {
            version: 1,
            device_label: "Test Mac".to_string(),
            skills: Vec::new(),
            artifacts: vec![artifact.clone()],
            excluded: Vec::new(),
            source_paths: Default::default(),
            artifact_source_paths,
            artifact_contents: Default::default(),
        };
        write_snapshot(&manifest, &checkout).unwrap();
        run_git(&checkout, &["add", "."]);
        run_git(&checkout, &["commit", "-m", "backup"]);
        let revision = super::current_commit(&checkout).unwrap();
        let target = root.join("restore/AGENTS.md");
        let plan = super::BackupRestorePlan {
            revision,
            target_root: root.join("restore"),
            operations: vec![super::BackupRestoreOperation {
                id: artifact_id.clone(),
                name: artifact.name.clone(),
                category: artifact.category.clone(),
                target: target.clone(),
                status: "planned".to_string(),
                message: None,
            }],
            checkout,
            skills: Default::default(),
            artifacts: std::collections::BTreeMap::from([(artifact_id, artifact)]),
        };

        let result = super::apply_backup_restore_without_database(&plan, &[]).unwrap();

        assert_eq!(result.operations[0].status, "restored");
        assert_eq!(fs::read(target).unwrap(), content);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_plan_merges_one_mcp_entry_into_an_existing_config() {
        let root = temp_dir("tendi-backup-mcp-entry-restore");
        fs::create_dir_all(&root).unwrap();
        let checkout = root.join("checkout");
        run_git(&root, &["init", "checkout"]);
        run_git(&checkout, &["config", "user.email", "test@example.com"]);
        run_git(&checkout, &["config", "user.name", "Tendi test"]);
        let entry = br#"{
  "command": "restored"
}
"#;
        let artifact_id = "mcp-entry-demo".to_string();
        let artifact = super::BackupArtifact {
            id: artifact_id.clone(),
            category: "mcp".to_string(),
            name: "demo".to_string(),
            agent: "codex".to_string(),
            source_relative_path: "config.toml".to_string(),
            entry_key: "demo".to_string(),
            entry_selector: vec!["mcp_servers".to_string()],
            files: vec![super::BackupFile {
                path: "entry.json".to_string(),
                sha256: super::sha256_hex(entry),
                size: entry.len() as u64,
            }],
        };
        let manifest = BackupManifest {
            version: 1,
            device_label: "Test Mac".to_string(),
            skills: Vec::new(),
            artifacts: vec![artifact.clone()],
            excluded: Vec::new(),
            source_paths: Default::default(),
            artifact_source_paths: Default::default(),
            artifact_contents: std::collections::BTreeMap::from([(
                format!("mcp:{artifact_id}"),
                entry.to_vec(),
            )]),
        };
        write_snapshot(&manifest, &checkout).unwrap();
        run_git(&checkout, &["add", "."]);
        run_git(&checkout, &["commit", "-m", "sync"]);
        let revision = super::current_commit(&checkout).unwrap();
        let target = root.join("restore/config.toml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(
            &target,
            "[mcp_servers.kept]\nurl = \"https://example.com/mcp\"\n",
        )
        .unwrap();
        let plan = super::BackupRestorePlan {
            revision,
            target_root: root.join("restore"),
            operations: vec![super::BackupRestoreOperation {
                id: artifact_id.clone(),
                name: artifact.name.clone(),
                category: artifact.category.clone(),
                target: target.clone(),
                status: "planned".to_string(),
                message: None,
            }],
            checkout,
            skills: Default::default(),
            artifacts: std::collections::BTreeMap::from([(artifact_id, artifact)]),
        };

        let result = super::apply_backup_restore_without_database(&plan, &[]).unwrap();
        let value = toml::from_str::<toml::Value>(&fs::read_to_string(&target).unwrap()).unwrap();

        assert_eq!(result.operations[0].status, "restored");
        assert_eq!(
            value["mcp_servers"]["demo"]["command"],
            toml::Value::String("restored".to_string())
        );
        assert_eq!(
            value["mcp_servers"]["kept"]["url"],
            toml::Value::String("https://example.com/mcp".to_string())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_validation_rejects_paths_that_escape_a_skill_directory() {
        let manifest = BackupManifest {
            version: 1,
            device_label: "Test Mac".to_string(),
            skills: vec![super::BackupSkill {
                id: "demo".to_string(),
                name: "demo".to_string(),
                source: super::BackupSkillSource::default(),
                files: vec![super::BackupFile {
                    path: "../outside".to_string(),
                    sha256: "0".repeat(64),
                    size: 0,
                }],
            }],
            artifacts: Vec::new(),
            excluded: Vec::new(),
            source_paths: Default::default(),
            artifact_source_paths: Default::default(),
            artifact_contents: Default::default(),
        };

        let error = validate_manifest(&manifest).unwrap_err();
        assert!(error.to_string().contains("file path"));
    }

    #[test]
    fn backup_now_commits_a_self_contained_snapshot_to_the_configured_remote() {
        let root = temp_dir("tendi-skill-backup-git");
        let skill = root.join("global/review");
        write_skill(&skill, "review");
        let remote = root.join("remote.git");
        fs::create_dir_all(&root).unwrap();
        run_git(&root, &["init", "--bare", "remote.git"]);
        let checkout = root.join("checkout");
        let store = Store::open(root.join("tendi.sqlite3")).unwrap();
        store
            .upsert_skill_source_records_for_workspace(&root, &[source("review", &skill)])
            .unwrap();
        store
            .save_skill_backup_config(&BackupConfig::new(
                remote.display().to_string(),
                checkout.clone(),
            ))
            .unwrap();

        let report = backup_now(&store, &root).unwrap();

        assert!(report.pushed);
        assert_eq!(report.manifest.skills.len(), 1);
        assert!(checkout.join("manifest.json").is_file());
        let remote_manifest = Command::new("git")
            .args([
                "--git-dir",
                remote.to_str().unwrap(),
                "show",
                "main:manifest.json",
            ])
            .output()
            .unwrap();
        assert!(remote_manifest.status.success());
        assert!(String::from_utf8_lossy(&remote_manifest.stdout).contains("review"));
        assert_eq!(backup_versions(&store, 10).unwrap().len(), 1);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backup_from_another_device_keeps_remote_skills_that_are_not_locally_installed() {
        let root = temp_dir("tendi-skill-backup-merge");
        let review = root.join("first/review");
        let draft = root.join("second/draft");
        write_skill(&review, "review");
        write_skill(&draft, "draft");
        let remote = root.join("remote.git");
        fs::create_dir_all(&root).unwrap();
        run_git(&root, &["init", "--bare", "remote.git"]);

        let first_store = Store::open(root.join("first.sqlite3")).unwrap();
        first_store
            .upsert_skill_source_records_for_workspace(&root, &[source("review", &review)])
            .unwrap();
        first_store
            .save_skill_backup_config(&BackupConfig::new(
                remote.display().to_string(),
                root.join("first-checkout"),
            ))
            .unwrap();
        backup_now(&first_store, &root).unwrap();

        let second_store = Store::open(root.join("second.sqlite3")).unwrap();
        second_store
            .upsert_skill_source_records_for_workspace(&root, &[source("draft", &draft)])
            .unwrap();
        second_store
            .save_skill_backup_config(&BackupConfig::new(
                remote.display().to_string(),
                root.join("second-checkout"),
            ))
            .unwrap();
        let report = backup_now(&second_store, &root).unwrap();

        assert_eq!(report.manifest.skills.len(), 2);
        assert!(report
            .manifest
            .skills
            .iter()
            .any(|skill| skill.name == "review"));
        assert!(report
            .manifest
            .skills
            .iter()
            .any(|skill| skill.name == "draft"));
        let remote_manifest = Command::new("git")
            .args([
                "--git-dir",
                remote.to_str().unwrap(),
                "show",
                "main:manifest.json",
            ])
            .output()
            .unwrap();
        assert!(remote_manifest.status.success());
        let remote_text = String::from_utf8_lossy(&remote_manifest.stdout);
        assert!(remote_text.contains("review"));
        assert!(remote_text.contains("draft"));

        drop(second_store);
        drop(first_store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diverged_devices_keep_each_skill_version_without_a_git_merge_conflict() {
        let root = temp_dir("tendi-skill-backup-diverged");
        let first_skill = root.join("first/review");
        let second_skill = root.join("second/review");
        write_skill(&first_skill, "review");
        write_skill(&second_skill, "review");
        fs::write(
            second_skill.join("SKILL.md"),
            "---\nname: review\n---\n# second device\n",
        )
        .unwrap();
        let remote = root.join("remote.git");
        fs::create_dir_all(&root).unwrap();
        run_git(&root, &["init", "--bare", "remote.git"]);

        let first_checkout = root.join("first-checkout");
        let first_config = BackupConfig::new(
            remote.display().to_string(),
            first_checkout.clone(),
        );
        let first_store = Store::open(root.join("first.sqlite3")).unwrap();
        first_store
            .upsert_skill_source_records_for_workspace(&root, &[source("review", &first_skill)])
            .unwrap();
        first_store.save_skill_backup_config(&first_config).unwrap();
        backup_now(&first_store, &root).unwrap();

        let second_store = Store::open(root.join("second.sqlite3")).unwrap();
        second_store
            .upsert_skill_source_records_for_workspace(&root, &[source("review", &second_skill)])
            .unwrap();
        second_store
            .save_skill_backup_config(&BackupConfig::new(
                remote.display().to_string(),
                root.join("second-checkout"),
            ))
            .unwrap();
        backup_now(&second_store, &root).unwrap();

        fs::write(
            first_skill.join("SKILL.md"),
            "---\nname: review\n---\n# first device\n",
        )
        .unwrap();
        let local_only = build_manifest(
            &first_store.skill_source_records_for_workspace(&root).unwrap(),
            &BackupBuildOptions {
                device_label: "First Mac".to_string(),
            },
        )
        .unwrap();
        write_snapshot(&local_only, &first_checkout).unwrap();
        let machine_name = current_machine_name().unwrap();
        assert!(super::commit_checkout(&first_config, &local_only, &machine_name).unwrap());

        let report = backup_now(&first_store, &root).unwrap();

        assert!(report.pushed);
        assert_eq!(report.manifest.skills.len(), 3);
        assert!(report
            .manifest
            .skills
            .iter()
            .all(|skill| skill.name == "review"));
        assert!(!super::checkout_has_conflicts(&first_checkout));
        drop(second_store);
        drop(first_store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn adopting_a_project_skill_for_backup_is_refused() {
        let root = temp_dir("tendi-skill-backup-adopt");
        let skill = root.join("project/.agents/skills/demo");
        write_skill(&skill, "demo");
        run_git(&root.join("project"), &["init"]);
        let store = Store::open(root.join("tendi.sqlite3")).unwrap();

        let error = adopt_skill_for_backup(&store, &root, &skill, "demo").unwrap_err();

        assert!(error.to_string().contains("project-repository"));
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_plan_materializes_a_selected_snapshot_into_the_user_selected_target() {
        let root = temp_dir("tendi-skill-backup-restore");
        let skill = root.join("global/review");
        write_skill(&skill, "review");
        let remote = root.join("remote.git");
        fs::create_dir_all(&root).unwrap();
        run_git(&root, &["init", "--bare", "remote.git"]);
        let store = Store::open(root.join("tendi.sqlite3")).unwrap();
        store
            .upsert_skill_source_records_for_workspace(&root, &[source("review", &skill)])
            .unwrap();
        store
            .save_skill_backup_config(&BackupConfig::new(
                remote.display().to_string(),
                root.join("checkout"),
            ))
            .unwrap();
        backup_now(&store, &root).unwrap();
        let mut versions = backup_versions(&store, 1).unwrap();
        let revision = versions.remove(0).id;

        let target: SkillTarget = "shared".parse().unwrap();
        let plan = super::plan_backup_restore(
            &store,
            &root.join("restore-workspace"),
            &revision,
            &[],
            &target,
            SkillInstallScope::Project,
        )
        .unwrap();
        assert_eq!(plan.operations[0].status, "planned");
        let operations = super::apply_backup_restore(&plan, &store, &root, &[]).unwrap();

        assert_eq!(operations[0].status, "restored");
        assert!(operations[0].target.join("SKILL.md").is_file());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_keep_both_preserves_an_existing_skill_directory() {
        let root = temp_dir("tendi-skill-backup-keep-both");
        let skill = root.join("global/review");
        write_skill(&skill, "review");
        let remote = root.join("remote.git");
        fs::create_dir_all(&root).unwrap();
        run_git(&root, &["init", "--bare", "remote.git"]);
        let store = Store::open(root.join("tendi.sqlite3")).unwrap();
        store
            .upsert_skill_source_records_for_workspace(&root, &[source("review", &skill)])
            .unwrap();
        store
            .save_skill_backup_config(&BackupConfig::new(
                remote.display().to_string(),
                root.join("checkout"),
            ))
            .unwrap();
        backup_now(&store, &root).unwrap();
        let revision = backup_versions(&store, 1).unwrap().remove(0).id;

        let target: SkillTarget = "shared".parse().unwrap();
        let workspace = root.join("restore-workspace");
        let existing = workspace.join(".agents/skills/review");
        write_skill(&existing, "local-review");
        let plan = super::plan_backup_restore(
            &store,
            &workspace,
            &revision,
            &[],
            &target,
            SkillInstallScope::Project,
        )
        .unwrap();
        assert_eq!(plan.operations[0].status, "conflict");

        let operations = super::apply_backup_restore(
            &plan,
            &store,
            &workspace,
            &[super::BackupRestoreResolution {
                id: plan.operations[0].id.clone(),
                action: "keep-both".to_string(),
            }],
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(existing.join("SKILL.md")).unwrap(),
            "---\nname: local-review\n---\n# local-review\n"
        );
        assert_eq!(operations[0].status, "restored");
        assert!(operations[0].target.ends_with("review-restored"));
        assert!(operations[0].target.join("SKILL.md").is_file());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    fn source(name: &str, path: &Path) -> SkillSourceRecord {
        SkillSourceRecord {
            skill_name: name.to_string(),
            skill_path: path.to_path_buf(),
            source_kind: "local".to_string(),
            source: None,
            source_ref: None,
            source_version: None,
            source_relative_path: None,
            update_status: "local".to_string(),
            origin: "tendi-install".to_string(),
        }
    }

    fn write_skill(path: &Path, name: &str) {
        fs::create_dir_all(path).unwrap();
        fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {name}\n---\n# {name}\n"),
        )
        .unwrap();
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
    }
}
