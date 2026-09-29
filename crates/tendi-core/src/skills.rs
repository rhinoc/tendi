use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    env, fs,
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, TimeZone, Utc};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use walkdir::WalkDir;

use crate::fsutil::{atomic_write, atomic_write_bytes, sha256_bytes, sha256_file, sha256_text};
use crate::git::{self, CommandFailure};
use crate::runtime_contract::InstallationId;
use crate::skill_targets::{SkillInstallScope, SkillTarget, skill_target_root};
use crate::time::compare_timestamps;

const WRAPPER_CATALOG_START: &str = "<catalog>";
const WRAPPER_CATALOG_END: &str = "</catalog>";
const MAX_CONCURRENT_GIT_FETCHES: usize = 8;
static GIT_UPDATE_CHECK_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static CANONICAL_MATERIALIZATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const KEEP_LOCAL_RESOLUTION: &str = "__tendi_keep_local__";
const USE_UPDATE_RESOLUTION: &str = "__tendi_use_update__";

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    Codex,
    Cursor,
    Claude,
    Shared,
    Unknown,
}

impl AgentKind {
    pub fn label(self) -> &'static str {
        crate::providers::agent_provider(self).storage_key()
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillVisibility {
    Auto,
    Manual,
    Off,
    Mixed,
}

impl SkillVisibility {
    pub(crate) fn label(self) -> &'static str {
        match self {
            SkillVisibility::Auto => "auto",
            SkillVisibility::Manual => "manual",
            SkillVisibility::Off => "off",
            SkillVisibility::Mixed => "mixed",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillRoot {
    pub path: PathBuf,
    pub scope: String,
    pub agent: AgentKind,
    pub plugin_id: Option<String>,
    pub plugin_enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillPath {
    pub path: PathBuf,
    pub root: PathBuf,
    pub scope: String,
    pub agent: AgentKind,
    pub install_target: String,
    pub source_kind: String,
    pub source: Option<String>,
    pub source_ref: Option<String>,
    pub source_version: Option<String>,
    pub source_relative_path: Option<String>,
    pub symlink_status: String,
    pub update_status: String,
    pub sha256: String,
    pub tags: Vec<String>,
    pub tendi_visibility: Option<SkillVisibility>,
    pub effective_visibility: SkillVisibility,
    pub provider_allow_implicit_invocation: Option<bool>,
    pub provider_skill_enabled: Option<bool>,
    pub provider_disable_model_invocation: Option<bool>,
    pub plugin_id: Option<String>,
    pub plugin_enabled: Option<bool>,
}

pub fn skill_backup_exclusion_reason(paths: &[SkillPath]) -> Option<&'static str> {
    paths.iter().find_map(|path| {
        crate::providers::agent_provider(path.agent).skill_backup_exclusion_reason(path)
    })
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillSourceRecord {
    pub skill_name: String,
    pub skill_path: PathBuf,
    pub source_kind: String,
    pub source: Option<String>,
    #[serde(default)]
    pub source_ref: Option<String>,
    pub source_version: Option<String>,
    pub source_relative_path: Option<String>,
    pub update_status: String,
    pub origin: String,
}

#[derive(Debug, Clone)]
pub struct SkillSnapshotFile {
    pub relative_path: String,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct SkillSnapshot {
    pub skill_path: PathBuf,
    pub source_version: String,
    pub files: Vec<SkillSnapshotFile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillRecord {
    /// Stable installation identity. It is never a display name.
    #[serde(default)]
    pub id: String,
    #[serde(rename = "installationId", default)]
    pub installation_id: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub dependencies: Vec<String>,
    pub dependents: Vec<String>,
    #[serde(rename = "dependencyIds", default)]
    pub dependency_ids: Vec<String>,
    #[serde(rename = "dependentIds", default)]
    pub dependent_ids: Vec<String>,
    #[serde(default)]
    pub is_wrapper: bool,
    pub visibility: SkillVisibility,
    pub agents: Vec<AgentKind>,
    pub paths: Vec<SkillPath>,
    pub source_summary: String,
    pub install_targets: Vec<String>,
    pub update_status: String,
    pub is_system: bool,
    pub ctime: Option<String>,
    pub mtime: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillScan {
    pub roots: Vec<SkillRoot>,
    pub skills: Vec<SkillRecord>,
    pub warnings: Vec<String>,
}

impl SkillScan {
    /// Finds the unique scanned skill location identified by `location_id`.
    ///
    /// A duplicate is treated as no match so callers cannot accidentally operate on an
    /// arbitrary location if a malformed or manually assembled scan violates the identity
    /// invariant.
    pub fn find_skill_location_by_id(
        &self,
        location_id: &str,
    ) -> Option<(&SkillRecord, &SkillPath)> {
        if location_id.trim().is_empty() {
            return None;
        }

        let mut matched = None;
        for skill in &self.skills {
            for path in &skill.paths {
                if skill_location_id(path) != location_id {
                    continue;
                }
                if matched.is_some() {
                    return None;
                }
                matched = Some((skill, path));
            }
        }
        matched
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangeSet {
    pub changes: Vec<FileChange>,
}

/// Owns any copy-on-write materializations performed before a skill mutation.
///
/// The transaction deliberately works on the observed skill path instead of
/// inspecting a particular source type. A writable skill is left untouched;
/// a readable skill whose actual directory rejects the write probe is copied
/// into the same active location before the caller applies its normal changes.
#[derive(Debug)]
pub struct SkillWriteTransaction {
    materializations: Vec<SkillMaterialization>,
    projection_relinks: Vec<SkillProjectionRelink>,
    resources: Option<crate::coordination::ResourceLease>,
}

#[derive(Debug)]
struct SkillMaterialization {
    source: PathBuf,
    target: PathBuf,
    backup: PathBuf,
}

#[derive(Debug)]
struct SkillProjectionRelink {
    path: PathBuf,
    original_target: PathBuf,
}

impl SkillWriteTransaction {
    /// Prepare all skill directories for mutation. The returned transaction
    /// retains the filesystem lease until commit or rollback.
    pub fn prepare(paths: &[PathBuf]) -> Result<Self> {
        let resources = crate::coordination::acquire_file_resources(paths)?;
        let mut transaction = Self {
            materializations: Vec::new(),
            projection_relinks: Vec::new(),
            resources: Some(resources),
        };
        let mut seen = BTreeSet::new();
        for path in paths {
            if !seen.insert(path.clone()) {
                continue;
            }
            if let Err(error) = transaction.prepare_path(path) {
                let rollback_error = transaction.rollback_materializations();
                transaction.resources.take();
                return match rollback_error {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(anyhow::anyhow!(
                        "{error:#}; failed to roll back skill materialization: {rollback_error:#}"
                    )),
                };
            }
        }
        Ok(transaction)
    }

    /// Keep the materialized directories and release the filesystem lease.
    pub fn commit(mut self) {
        for materialization in &self.materializations {
            if let Err(error) = remove_materialization_backup(&materialization.backup) {
                crate::logging::global().warn(
                    "skill materialization backup cleanup failed",
                    serde_json::json!({
                        "operation": "skill_write_commit",
                        "target": &materialization.target,
                        "backup": &materialization.backup,
                        "error": error.to_string(),
                    }),
                );
            }
        }
        self.resources.take();
    }

    /// Restore every observed skill path to its pre-mutation filesystem entry.
    pub fn rollback(mut self) -> Result<()> {
        let result = self.rollback_materializations();
        self.resources.take();
        result
    }

    fn prepare_path(&mut self, path: &Path) -> Result<()> {
        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("failed to inspect skill {}", path.display()))?;
        if !metadata.is_dir() && !metadata.file_type().is_symlink() {
            bail!("skill path is not a directory: {}", path.display());
        }
        if !path.is_dir() {
            bail!("skill path is not readable: {}", path.display());
        }
        let source = path
            .canonicalize()
            .with_context(|| format!("failed to resolve skill {}", path.display()))?;
        if !source.join("SKILL.md").is_file() {
            bail!("{} is not a skill directory", source.display());
        }

        if let Some(materialization) = self
            .materializations
            .iter()
            .find(|materialization| materialization.source == source)
        {
            if metadata.file_type().is_symlink() {
                let target = materialization.target.canonicalize().with_context(|| {
                    format!(
                        "failed to resolve materialized skill {}",
                        materialization.target.display()
                    )
                })?;
                self.relink_projection(path, &target)?;
                return Ok(());
            }
        }

        match probe_skill_directory_write(&source) {
            Ok(()) => return Ok(()),
            Err(error) if is_write_capability_error(&error) => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to verify write access to {}", source.display())
                });
            }
        }

        let parent = path
            .parent()
            .with_context(|| format!("skill path has no parent: {}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .context("skill path has no usable file name")?;
        let sequence = CANONICAL_MATERIALIZATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".{name}.tendi-materialize-{sequence}"));
        let backup = parent.join(format!(".{name}.tendi-original-{sequence}"));

        fs::create_dir(&temporary)
            .with_context(|| format!("failed to create {}", temporary.display()))?;
        if let Err(error) = copy_dir(&source, &temporary) {
            let _ = fs::remove_dir_all(&temporary);
            return Err(error).with_context(|| {
                format!(
                    "failed to materialize {} into {}",
                    source.display(),
                    temporary.display()
                )
            });
        }
        if !temporary.join("SKILL.md").is_file() {
            let _ = fs::remove_dir_all(&temporary);
            bail!(
                "materialized skill is missing SKILL.md: {}",
                temporary.display()
            );
        }

        if let Err(error) = rename_skill_directory(path, &backup) {
            let _ = fs::remove_dir_all(&temporary);
            return Err(error)
                .with_context(|| format!("failed to preserve original skill {}", path.display()));
        }
        if let Err(error) = rename_skill_directory(&temporary, path) {
            let _ = rename_skill_directory(&backup, path);
            let _ = fs::remove_dir_all(&temporary);
            return Err(error)
                .with_context(|| format!("failed to install writable skill {}", path.display()));
        }

        self.materializations.push(SkillMaterialization {
            source: source.clone(),
            target: path.to_path_buf(),
            backup,
        });
        crate::logging::global().debug(
            "skill materialized for write",
            serde_json::json!({
                "operation": "skill_write_materialize",
                "source": source,
                "target": path,
            }),
        );
        Ok(())
    }

    fn relink_projection(&mut self, path: &Path, target: &Path) -> Result<()> {
        let original_target = fs::read_link(path)
            .with_context(|| format!("failed to read skill projection {}", path.display()))?;
        fs::remove_file(path)
            .with_context(|| format!("failed to replace skill projection {}", path.display()))?;
        if let Err(error) = create_symlink(target, path) {
            let _ = create_symlink(&original_target, path);
            return Err(error)
                .with_context(|| format!("failed to relink skill projection {}", path.display()));
        }
        self.projection_relinks.push(SkillProjectionRelink {
            path: path.to_path_buf(),
            original_target,
        });
        Ok(())
    }

    fn rollback_materializations(&mut self) -> Result<()> {
        let mut failures = Vec::new();
        for relink in self.projection_relinks.iter().rev() {
            if let Err(error) = remove_filesystem_entry(&relink.path) {
                failures.push(format!(
                    "failed to remove relinked projection {}: {error:#}",
                    relink.path.display()
                ));
                continue;
            }
            if let Err(error) = create_symlink(&relink.original_target, &relink.path) {
                failures.push(format!(
                    "failed to restore skill projection {}: {error:#}",
                    relink.path.display()
                ));
            }
        }
        self.projection_relinks.clear();
        for materialization in self.materializations.iter().rev() {
            if let Err(error) = remove_filesystem_entry(&materialization.target) {
                failures.push(format!(
                    "failed to remove {}: {error:#}",
                    materialization.target.display()
                ));
                continue;
            }
            if let Err(error) =
                rename_skill_directory(&materialization.backup, &materialization.target)
            {
                failures.push(format!(
                    "failed to restore {}: {error:#}",
                    materialization.target.display()
                ));
            }
        }
        self.materializations.clear();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(failures.join("; ")))
        }
    }
}

fn probe_skill_directory_write(directory: &Path) -> std::io::Result<()> {
    let sequence = CANONICAL_MATERIALIZATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let probe = directory.join(format!(
        ".tendi-write-probe-{}-{sequence}",
        std::process::id()
    ));
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)?;
    file.sync_all()?;
    fs::remove_file(probe)
}

fn is_write_capability_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem
    )
}

fn remove_materialization_backup(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        for entry in WalkDir::new(path).follow_links(false) {
            let entry = entry?;
            if !entry.file_type().is_dir() {
                continue;
            }
            let mut permissions = fs::metadata(entry.path())?.permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                permissions.set_mode(permissions.mode() | 0o700);
            }
            fs::set_permissions(entry.path(), permissions)?;
        }
    }
    remove_filesystem_entry(path)
}

#[derive(Debug, Clone, Serialize)]
pub struct FileChange {
    pub path: PathBuf,
    pub before_sha256: Option<String>,
    pub before: Option<String>,
    pub after: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MaterializeResult {
    pub source: PathBuf,
    pub target: PathBuf,
    pub mode: String,
    pub health: String,
    pub applied: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillDistributionMode {
    Move,
    Symlink,
    Copy,
}

impl FromStr for SkillDistributionMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "move" => Ok(Self::Move),
            "symlink" => Ok(Self::Symlink),
            "copy" => Ok(Self::Copy),
            _ => bail!("unknown skill distribution mode: {value}"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillDistributionPlan {
    pub name: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub mode: SkillDistributionMode,
    pub source_symlink: bool,
    pub destination_exists: bool,
    pub source_sha256: String,
    pub status: String,
    pub message: Option<String>,
    #[serde(skip)]
    pub source_record: SkillSourceRecord,
    #[serde(skip)]
    pub projection_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillAddOptions {
    pub source: String,
    pub target: SkillTarget,
    pub scope: SkillInstallScope,
    pub skills: Vec<String>,
    pub copy: bool,
    pub overwrite: bool,
    pub visibility: SkillVisibility,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillAddPlan {
    pub source: String,
    pub source_kind: String,
    pub source_ref: Option<String>,
    pub source_root: PathBuf,
    pub target: SkillTarget,
    pub scope: SkillInstallScope,
    pub mode: String,
    pub available: Vec<InstallableSkill>,
    pub selected: Vec<InstallableSkill>,
    pub operations: Vec<SkillAddOperation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallableSkill {
    pub name: String,
    pub description: Option<String>,
    pub path: PathBuf,
    pub relative_path: String,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillAddOperation {
    pub name: String,
    pub source: PathBuf,
    pub target: PathBuf,
    pub mode: String,
    pub status: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillAddApplyReport {
    pub plan: SkillAddPlan,
    pub results: Vec<MaterializeResult>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillUpdateReport {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub status: String,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub source: Option<String>,
    pub source_kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillSourceUpdate {
    pub skill_path: PathBuf,
    pub source_version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillMergeIssue {
    pub name: String,
    pub path: PathBuf,
    pub resolution_key: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub before: String,
    pub base: String,
    pub incoming: String,
    pub after: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillUpdatePlan {
    pub file_changes: ChangeSet,
    pub git_updates: Vec<GitUpdateAction>,
    pub skipped: Vec<SkillUpdateReport>,
    pub source_updates: Vec<SkillSourceUpdate>,
    pub merge_issues: Vec<SkillMergeIssue>,
}

impl SkillUpdatePlan {
    pub fn can_apply(&self) -> bool {
        !self.file_changes.changes.is_empty()
            || self
                .git_updates
                .iter()
                .any(GitUpdateAction::has_effective_changes)
            || !self.merge_issues.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct SkillUpdatePersistence {
    pub source_records: Vec<SkillSourceRecord>,
    pub snapshots: Vec<SkillSnapshot>,
    pub expected_source_versions: Vec<(PathBuf, Option<String>)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillDeletePlan {
    pub targets: Vec<SkillDeleteTarget>,
    pub dependencies: Vec<SkillDeleteRelation>,
    pub dependents: Vec<SkillDeleteRelation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillDeleteTarget {
    pub name: String,
    pub path: PathBuf,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillDeleteRelation {
    pub name: String,
    pub related: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitUpdateAction {
    pub name: String,
    pub skill_names: Vec<String>,
    pub repo: PathBuf,
    pub source: String,
    pub source_ref: Option<String>,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub diff: String,
    pub files: Vec<GitUpdateFile>,
    pub tendi_settings: Vec<GitSkillVisibility>,
    pub materialized_targets: Vec<MaterializedGitTarget>,
}

impl GitUpdateAction {
    pub fn has_effective_changes(&self) -> bool {
        !self.files.is_empty()
            || self
                .materialized_targets
                .iter()
                .any(|target| !target.files.is_empty())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MaterializedGitTarget {
    pub name: String,
    pub target: PathBuf,
    pub agent: AgentKind,
    pub source_relative_path: Option<String>,
    pub visibility: SkillVisibility,
    pub uses_shared_layout: bool,
    pub files: Vec<GitUpdateFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitUpdateFile {
    pub path: String,
    pub resolution_key: String,
    pub before: String,
    pub base: String,
    pub incoming: String,
    pub after: String,
    #[serde(skip)]
    pub before_bytes: Option<Vec<u8>>,
    #[serde(skip)]
    pub incoming_bytes: Option<Vec<u8>>,
    #[serde(skip)]
    pub after_bytes: Option<Vec<u8>>,
    pub before_exists: bool,
    pub incoming_exists: bool,
    pub after_exists: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitSkillVisibility {
    pub skill_dir: PathBuf,
    pub agent: AgentKind,
    pub visibility: SkillVisibility,
}

#[derive(Debug, Clone)]
struct RawSkill {
    name: String,
    description: Option<String>,
    tags: Vec<String>,
    dependencies: Vec<String>,
    dependency_files: Vec<PathBuf>,
    is_wrapper: bool,
    is_system: bool,
    path: SkillPath,
}

#[derive(Debug, Clone)]
struct InstallableSkillCandidate {
    skill: InstallableSkill,
    dependency_files: Vec<PathBuf>,
}

pub fn scan_skills(cwd: &Path) -> Result<SkillScan> {
    let store = crate::storage::Store::open_default()?;
    scan_skills_with_source_store(cwd, &store)
}

pub fn scan_skills_for_project_roots(cwd: &Path, project_roots: &[PathBuf]) -> Result<SkillScan> {
    let store = crate::storage::Store::open_default()?;
    scan_skills_for_project_roots_with_store(cwd, &store, project_roots)
}

pub fn scan_skills_for_project_roots_with_store(
    cwd: &Path,
    store: &crate::storage::Store,
    project_roots: &[PathBuf],
) -> Result<SkillScan> {
    scan_skills_with_source_store_for_projects_for_projection(cwd, store, project_roots)
}

pub fn scan_skills_synced_for_project_roots(
    cwd: &Path,
    project_roots: &[PathBuf],
) -> Result<SkillScan> {
    let store = crate::storage::Store::open_default()?;
    scan_skills_synced_for_project_roots_with_store(cwd, &store, project_roots)
}

pub fn scan_skills_synced_for_project_roots_with_store(
    cwd: &Path,
    store: &crate::storage::Store,
    project_roots: &[PathBuf],
) -> Result<SkillScan> {
    scan_skills_synced_for_project_roots_with_store_for_projection(cwd, store, project_roots)
}

pub fn scan_skills_synced_for_project_roots_with_store_for_projection(
    cwd: &Path,
    store: &crate::storage::Store,
    project_roots: &[PathBuf],
) -> Result<SkillScan> {
    let scan =
        scan_skills_with_source_store_for_projects_for_projection(cwd, store, project_roots)?;
    if materialize_tendi_cache_links(&scan)? {
        return scan_skills_synced_for_project_roots_with_store_for_projection(
            cwd,
            store,
            project_roots,
        );
    }
    let scan = reconcile_skill_visibility_for_workspace(store, cwd, scan, project_roots)?;
    let changeset = plan_wrapper_sync(&scan)?;
    if changeset.changes.is_empty() {
        return Ok(scan);
    }
    apply_changes(&changeset)?;
    scan_skills_with_source_store_for_projects_for_projection(cwd, store, project_roots)
}

fn scan_skills_with_source_store(cwd: &Path, store: &crate::storage::Store) -> Result<SkillScan> {
    scan_skills_with_source_store_for_projects(cwd, store, &[])
}

pub(crate) fn scan_skills_for_workspace_initialization(
    cwd: &Path,
    store: &crate::storage::Store,
    project_roots: &[PathBuf],
) -> Result<SkillScan> {
    let source_records = store.skill_source_records_for_workspace(cwd)?;
    let mut provenance_resolver = ProvenanceResolver::managed(cwd, source_records, project_roots);
    scan_skills_with_resolver_for_projects(cwd, &mut provenance_resolver, project_roots)
}

fn scan_skills_with_source_store_for_projects(
    cwd: &Path,
    store: &crate::storage::Store,
    project_roots: &[PathBuf],
) -> Result<SkillScan> {
    scan_skills_with_source_store_for_projects_for_projection(cwd, store, project_roots)
}

fn scan_skills_with_source_store_for_projects_for_projection(
    cwd: &Path,
    store: &crate::storage::Store,
    project_roots: &[PathBuf],
) -> Result<SkillScan> {
    let source_records = store.skill_source_records_for_workspace(cwd)?;
    let mut provenance_resolver = ProvenanceResolver::managed(cwd, source_records, project_roots);
    let scan =
        scan_skills_with_resolver_for_projects(cwd, &mut provenance_resolver, project_roots)?;
    apply_persisted_skill_visibilities(store, cwd, scan)
}

fn apply_persisted_skill_visibilities(
    store: &crate::storage::Store,
    cwd: &Path,
    mut scan: SkillScan,
) -> Result<SkillScan> {
    let persisted = store.skill_visibilities_for_workspace(cwd)?;
    for skill in &mut scan.skills {
        for path in &mut skill.paths {
            let key = canonical_skill_dir(&path.path);
            // Retain the user's target for reconciliation; display stays provider-derived.
            path.tendi_visibility = persisted.get(&key).copied();
        }
        skill.visibility = skill_visibility_from_paths(&skill.paths);
    }
    Ok(scan)
}

fn preferred_skill_visibility(left: SkillVisibility, right: SkillVisibility) -> SkillVisibility {
    match (left, right) {
        (SkillVisibility::Off, _) | (_, SkillVisibility::Off) => SkillVisibility::Off,
        (SkillVisibility::Manual, _) | (_, SkillVisibility::Manual) => SkillVisibility::Manual,
        _ => SkillVisibility::Auto,
    }
}

fn skill_visibility_from_paths(paths: &[SkillPath]) -> SkillVisibility {
    paths.iter().fold(SkillVisibility::Auto, |current, path| {
        preferred_skill_visibility(current, path.effective_visibility)
    })
}

pub fn reconcile_skill_visibility_for_workspace(
    store: &crate::storage::Store,
    cwd: &Path,
    scan: SkillScan,
    project_roots: &[PathBuf],
) -> Result<SkillScan> {
    let (scan, visibility_changed) =
        reconcile_skill_visibility_for_workspace_with_report(store, cwd, scan)?;
    if visibility_changed {
        scan_skills_with_source_store_for_projects_for_projection(cwd, store, project_roots)
    } else {
        Ok(scan)
    }
}

fn reconcile_skill_visibility_for_workspace_with_report(
    store: &crate::storage::Store,
    cwd: &Path,
    scan: SkillScan,
) -> Result<(SkillScan, bool)> {
    let scan = apply_persisted_skill_visibilities(store, cwd, scan)?;
    let mut targets = BTreeMap::<PathBuf, (AgentKind, SkillVisibility, bool)>::new();
    for skill in &scan.skills {
        if skill.is_system {
            continue;
        }
        for path in &skill.paths {
            let canonical = canonical_skill_dir(&path.path);
            let direct = path.symlink_status == "direct";
            if !direct && !is_tendi_source_cache_path(&canonical) {
                continue;
            }
            match targets.get(&canonical) {
                Some((_, _, true)) if !direct => {}
                _ => {
                    targets.insert(canonical, (path.agent, path.effective_visibility, direct));
                }
            }
        }
    }
    let resources = targets
        .iter()
        .flat_map(|(path, (agent, _, _))| skill_visibility_resource_paths(path, *agent, true))
        .collect::<Vec<_>>();
    let _resources = crate::coordination::acquire_file_resources(&resources)?;
    // Explicit Tendi choices are authoritative, including commands that
    // changed no file bytes. Reload them after admission rather than applying
    // a stale visibility plan.
    let persisted = store.skill_visibilities_for_workspace(cwd)?;
    let mut scan = scan;
    for skill in &mut scan.skills {
        for path in &mut skill.paths {
            path.tendi_visibility = persisted.get(&canonical_skill_dir(&path.path)).copied();
        }
        skill.visibility = skill_visibility_from_paths(&skill.paths);
    }
    let mut changes = Vec::new();
    for (path, (agent, _, _)) in targets {
        let Some(visibility) = persisted.get(&path).copied() else {
            continue;
        };
        if !path.join("SKILL.md").is_file() {
            continue;
        }
        match plan_skill_visibility_at_path(&path, agent, visibility, true) {
            Ok(planned) => changes.extend(planned),
            Err(error) => {
                // A malformed provider-owned policy must not make every
                // installation in this scope retry forever. Preserve the
                // user's bytes, expose the exact resource in the scan, and
                // let the rest of the reconciliation commit and acknowledge
                // its durable receipt.
                let warning = format!(
                    "{}: provider visibility sync skipped: {error:#}",
                    path.display()
                );
                crate::logging::global().warn(
                    "skill provider visibility sync skipped",
                    serde_json::json!({
                        "path": &path,
                        "agent": agent.label(),
                        "error": error.to_string(),
                    }),
                );
                scan.warnings.push(warning);
            }
        }
    }
    let changeset = ChangeSet {
        changes: dedupe_changes(changes),
    };
    if changeset.changes.is_empty() {
        return Ok((scan, false));
    }
    apply_changes(&changeset)?;
    Ok((scan, true))
}

pub fn scan_skills_synced_for_projection(cwd: &Path) -> Result<SkillScan> {
    let store = crate::storage::Store::open_default()?;
    scan_skills_synced_for_project_roots_with_store_for_projection(cwd, &store, &[])
}

#[cfg(test)]
fn scan_skills_without_source_database(cwd: &Path) -> Result<SkillScan> {
    scan_skills_with_resolver_for_projects(cwd, &mut ProvenanceResolver::default(), &[])
}

fn scan_skills_with_resolver_for_projects(
    cwd: &Path,
    provenance_resolver: &mut ProvenanceResolver,
    project_roots: &[PathBuf],
) -> Result<SkillScan> {
    let mut warnings = Vec::new();
    let mut roots = discover_roots_for_projects(cwd, project_roots);
    let mut raw_skills = Vec::new();
    let mut scanned_files = BTreeSet::new();
    let mut referenced_files = VecDeque::new();

    for root in &roots {
        for skill_file in find_skill_files(&root.path) {
            match read_skill(root, &skill_file, provenance_resolver) {
                Ok(skill) => {
                    scanned_files.insert(skill_file_key(&skill_file));
                    referenced_files.extend(skill.dependency_files.iter().cloned());
                    raw_skills.push(skill);
                }
                Err(err) => warnings.push(format!("{}: {err:#}", skill_file.display())),
            }
        }
    }

    while let Some(skill_file) = referenced_files.pop_front() {
        if !skill_file.is_file() || !scanned_files.insert(skill_file_key(&skill_file)) {
            continue;
        }
        let Some(skill_dir) = skill_file.parent() else {
            continue;
        };
        let root = SkillRoot {
            path: skill_dir.to_path_buf(),
            scope: "referenced".to_string(),
            agent: AgentKind::Unknown,
            plugin_id: None,
            plugin_enabled: None,
        };
        match read_skill(&root, &skill_file, provenance_resolver) {
            Ok(skill) => {
                referenced_files.extend(skill.dependency_files.iter().cloned());
                raw_skills.push(skill);
                push_root(
                    &mut roots,
                    root.path,
                    root.scope,
                    root.agent,
                    root.plugin_id,
                    root.plugin_enabled,
                );
            }
            Err(err) => warnings.push(format!("{}: {err:#}", skill_file.display())),
        }
    }

    resolve_raw_skill_path_dependencies(&mut raw_skills);
    let mut skills = merge_raw_skills(raw_skills);
    resolve_scanned_skill_relations(&mut skills);
    Ok(SkillScan {
        roots,
        skills,
        warnings,
    })
}

pub fn skill_dir_matches_name(skill_dir: &Path, expected_name: &str) -> bool {
    let Ok(text) = fs::read_to_string(skill_dir.join("SKILL.md")) else {
        return false;
    };
    let frontmatter = parse_frontmatter(&text);
    let name = frontmatter
        .as_ref()
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .or_else(|| skill_dir.file_name().and_then(|name| name.to_str()));
    name.is_some_and(|name| {
        normalize_skill_match_name(name) == normalize_skill_match_name(expected_name)
    })
}

pub fn scan_skills_synced(cwd: &Path) -> Result<SkillScan> {
    let store = crate::storage::Store::open_default()?;
    let scan = scan_skills_with_source_store(cwd, &store)?;
    let changeset = plan_wrapper_sync(&scan)?;
    if changeset.changes.is_empty() {
        return Ok(scan);
    }
    apply_changes(&changeset)?;
    scan_skills(cwd)
}

pub fn refresh_skill_scan(
    cwd: &Path,
    scan: SkillScan,
    skill_ids: &[String],
    extra_skill_dirs: &[PathBuf],
) -> Result<SkillScan> {
    let provenance_resolver = ProvenanceResolver::from_skills(scan.skills.iter().filter(|skill| {
        skill_ids.iter().any(|id| skill_matches_id(skill, id))
            || skill
                .paths
                .iter()
                .any(|path| extra_skill_dirs.iter().any(|dir| dir == &path.path))
    }));
    refresh_skill_scan_with_resolver(cwd, scan, skill_ids, extra_skill_dirs, provenance_resolver)
}

pub fn refresh_skill_scan_for_workspace(
    cwd: &Path,
    store: &crate::storage::Store,
    scan: SkillScan,
    skill_ids: &[String],
    extra_skill_dirs: &[PathBuf],
) -> Result<SkillScan> {
    let source_records = store.skill_source_records_for_workspace(cwd)?;
    let refreshed = refresh_skill_scan_with_resolver(
        cwd,
        scan,
        skill_ids,
        extra_skill_dirs,
        ProvenanceResolver::managed(cwd, source_records, &[]),
    )?;
    apply_persisted_skill_visibilities(store, cwd, refreshed)
}

/// Rebuild derived rows without modifying wrappers or provider configuration.
/// File reconciliation remains an explicit operation via the existing synced APIs.
pub fn refresh_skill_scan_for_workspace_projection(
    cwd: &Path,
    store: &crate::storage::Store,
    scan: SkillScan,
    skill_ids: &[String],
    extra_skill_dirs: &[PathBuf],
) -> Result<SkillScan> {
    let source_records = store.skill_source_records_for_workspace(cwd)?;
    let refreshed = refresh_skill_scan_with_wrapper_sync(
        cwd,
        scan,
        skill_ids,
        extra_skill_dirs,
        ProvenanceResolver::managed(cwd, source_records, &[]),
        false,
    )?;
    apply_persisted_skill_visibilities(store, cwd, refreshed)
}

/// Resolve dirty source paths against the persisted installation graph. This
/// walks a directory only when that directory itself was reported changed.
fn dirty_skill_targets(
    scan: &SkillScan,
    resources: &[PathBuf],
) -> Result<(Vec<String>, Vec<PathBuf>)> {
    let root_paths = scan
        .roots
        .iter()
        .map(|root| (root.path.clone(), canonical_skill_dir(&root.path)))
        .collect::<BTreeMap<_, _>>();
    let insert_directory = |directory: &Path, directories: &mut BTreeSet<PathBuf>| {
        let mut matched = false;
        for (logical, physical) in &root_paths {
            if let Ok(relative) = directory.strip_prefix(physical) {
                directories.insert(logical.join(relative));
                matched = true;
            }
        }
        if !matched {
            directories.insert(directory.to_path_buf());
        }
    };
    let physical = |path: &SkillPath| {
        if path.symlink_status != "direct" {
            return canonical_skill_dir(&path.path);
        }
        root_paths
            .get(&path.root)
            .and_then(|root| {
                path.path
                    .strip_prefix(&path.root)
                    .ok()
                    .map(|relative| root.join(relative))
            })
            .unwrap_or_else(|| path.path.clone())
    };
    let mut selected = BTreeSet::new();
    let mut directories = BTreeSet::new();
    for skill in &scan.skills {
        if skill.paths.iter().any(|path| {
            let canonical = physical(path);
            resources.iter().any(|resource| {
                resource.starts_with(&path.path)
                    || path.path.starts_with(resource)
                    || resource.starts_with(&canonical)
                    || canonical.starts_with(resource)
            })
        }) {
            selected.insert(skill.id.clone());
        }
    }
    // Dependents contain actual derived references (including wrapper catalogs),
    // not every installation that happens to share a provider or workspace.
    loop {
        let previous = selected.len();
        for skill in &scan.skills {
            if selected.contains(&skill.id) {
                selected.extend(skill.dependent_ids.iter().cloned());
            }
        }
        if previous == selected.len() {
            break;
        }
    }
    for resource in resources {
        let inside_root = root_paths.iter().any(|(logical, canonical)| {
            resource.starts_with(logical) || resource.starts_with(canonical)
        });
        if !inside_root {
            continue;
        }
        let mut candidate = resource.as_path();
        while root_paths.iter().any(|(logical, canonical)| {
            candidate.starts_with(logical) || candidate.starts_with(canonical)
        }) {
            if candidate.join("SKILL.md").is_file() {
                insert_directory(candidate, &mut directories);
                break;
            }
            let Some(parent) = candidate.parent() else {
                break;
            };
            candidate = parent;
        }
        if resource.is_dir() && !resource.join("SKILL.md").is_file() {
            for entry in WalkDir::new(resource).follow_links(false) {
                let entry = entry.with_context(|| {
                    format!(
                        "failed to inspect dirty skill resource {}",
                        resource.display()
                    )
                })?;
                if entry.file_type().is_file() && entry.file_name() == "SKILL.md" {
                    if let Some(parent) = entry.path().parent() {
                        insert_directory(parent, &mut directories);
                    }
                }
            }
        }
    }
    Ok((
        selected.into_iter().collect(),
        directories.into_iter().collect(),
    ))
}

pub fn refresh_dirty_skill_projection(
    cwd: &Path,
    store: &crate::storage::Store,
    mut cached: SkillScan,
    resources: &[PathBuf],
    full: bool,
    project_roots: &[PathBuf],
) -> Result<SkillScan> {
    if full {
        return scan_skills_for_project_roots_with_store(cwd, store, project_roots);
    }
    // A newly created provider root was absent from the previous snapshot. Ask
    // the provider owner for roots only when the dirty path has no cached root;
    // never infer another workspace's ownership from its directory spelling.
    if resources.iter().any(|resource| {
        !cached.roots.iter().any(|root| {
            resource.starts_with(&root.path)
                || resource.starts_with(canonical_skill_dir(&root.path))
        })
    }) {
        for root in discover_roots_for_projects(cwd, project_roots) {
            if !cached.roots.iter().any(|current| current.path == root.path) {
                cached.roots.push(root);
            }
        }
    }
    let (ids, directories) = dirty_skill_targets(&cached, resources)?;
    if ids.is_empty() && directories.is_empty() {
        return Ok(cached);
    }
    refresh_skill_scan_for_workspace_projection(cwd, store, cached, &ids, &directories)
}

/// Repair only the installations named by the durable maintenance receipt.
/// The caller acknowledges that receipt after this succeeds.
pub fn skill_reconciliation_resource_paths(
    _cwd: &Path,
    _store: &crate::storage::Store,
    scan: &SkillScan,
    resources: &[PathBuf],
    full: bool,
) -> Result<Vec<PathBuf>> {
    let selected = if full {
        scan.skills
            .iter()
            .map(|skill| skill.id.clone())
            .collect::<BTreeSet<_>>()
    } else {
        dirty_skill_targets(scan, resources)?
            .0
            .into_iter()
            .collect()
    };
    Ok(scan
        .skills
        .iter()
        .filter(|skill| selected.contains(&skill.id))
        .flat_map(|skill| {
            skill
                .paths
                .iter()
                .flat_map(|path| skill_visibility_resource_paths(&path.path, path.agent, true))
        })
        .collect())
}

pub fn reconcile_dirty_skill_resources(
    cwd: &Path,
    store: &crate::storage::Store,
    mut scan: SkillScan,
    resources: &[PathBuf],
    full: bool,
) -> Result<()> {
    let selected = if full {
        scan.skills
            .iter()
            .map(|skill| skill.id.clone())
            .collect::<BTreeSet<_>>()
    } else {
        dirty_skill_targets(&scan, resources)?
            .0
            .into_iter()
            .collect()
    };
    if selected.is_empty() {
        return Ok(());
    }
    let subset = SkillScan {
        roots: scan.roots.clone(),
        warnings: Vec::new(),
        skills: scan
            .skills
            .iter()
            .filter(|skill| selected.contains(&skill.id))
            .cloned()
            .collect(),
    };
    let paths = subset
        .skills
        .iter()
        .flat_map(|skill| {
            skill
                .paths
                .iter()
                .flat_map(|path| skill_visibility_resource_paths(&path.path, path.agent, true))
        })
        .collect::<Vec<_>>();
    let _resources = crate::coordination::acquire_file_resources(&paths)?;
    let (reconciled, visibility_changed) =
        reconcile_skill_visibility_for_workspace_with_report(store, cwd, subset)?;
    for updated in reconciled.skills {
        if let Some(current) = scan.skills.iter_mut().find(|skill| skill.id == updated.id) {
            *current = updated;
        }
    }
    let changes = plan_wrapper_sync_for_ids(&scan, Some(&selected))?;
    let wrappers_changed = !changes.changes.is_empty();
    if wrappers_changed {
        apply_changes(&changes)?;
    }
    if !visibility_changed && !wrappers_changed {
        return Ok(());
    }
    let touched = scan
        .skills
        .iter()
        .filter(|skill| selected.contains(&skill.id))
        .flat_map(|skill| skill.paths.iter().map(|path| path.path.clone()))
        .collect::<Vec<_>>();
    store.invalidate_projection_resources("skills", cwd, &touched, false)
}

fn refresh_skill_scan_with_resolver(
    cwd: &Path,
    scan: SkillScan,
    skill_ids: &[String],
    extra_skill_dirs: &[PathBuf],
    provenance_resolver: ProvenanceResolver,
) -> Result<SkillScan> {
    refresh_skill_scan_with_wrapper_sync(
        cwd,
        scan,
        skill_ids,
        extra_skill_dirs,
        provenance_resolver,
        true,
    )
}

fn refresh_skill_scan_with_wrapper_sync(
    cwd: &Path,
    mut scan: SkillScan,
    skill_ids: &[String],
    extra_skill_dirs: &[PathBuf],
    mut provenance_resolver: ProvenanceResolver,
    sync_wrappers: bool,
) -> Result<SkillScan> {
    let skill_ids = skill_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut refresh_dirs = extra_skill_dirs.iter().cloned().collect::<BTreeSet<_>>();
    let mut roots_by_dir = BTreeMap::<PathBuf, SkillRoot>::new();
    let mut remaining = Vec::with_capacity(scan.skills.len());

    for skill in std::mem::take(&mut scan.skills) {
        let refresh = skill_ids.iter().any(|id| skill_matches_id(&skill, id))
            || skill
                .paths
                .iter()
                .any(|path| refresh_dirs.contains(&path.path));
        if refresh {
            for path in &skill.paths {
                refresh_dirs.insert(path.path.clone());
                roots_by_dir.insert(
                    path.path.clone(),
                    SkillRoot {
                        path: path.root.clone(),
                        scope: path.scope.clone(),
                        agent: path.agent,
                        plugin_id: path.plugin_id.clone(),
                        plugin_enabled: path.plugin_enabled,
                    },
                );
            }
        } else {
            remaining.push(skill);
        }
    }

    let mut refreshed_raw = Vec::new();
    let known_files = remaining
        .iter()
        .flat_map(|skill| {
            skill
                .paths
                .iter()
                .map(|path| skill_file_key(&path.path.join("SKILL.md")))
        })
        .collect::<BTreeSet<_>>();
    let mut refreshed_files = BTreeSet::new();
    let mut referenced_files = VecDeque::new();
    for skill_dir in refresh_dirs {
        let skill_file = skill_dir.join("SKILL.md");
        if !skill_file.is_file() {
            continue;
        }
        let root = roots_by_dir.get(&skill_dir).cloned().or_else(|| {
            scan.roots
                .iter()
                .filter(|root| skill_dir.starts_with(&root.path))
                .max_by_key(|root| root.path.components().count())
                .cloned()
        });
        let root = root
            .or_else(|| infer_skill_root_for_dir(&skill_dir))
            .unwrap_or_else(|| SkillRoot {
                path: skill_dir.clone(),
                scope: "referenced".to_string(),
                agent: AgentKind::Unknown,
                plugin_id: None,
                plugin_enabled: None,
            });
        let skill = read_skill(&root, &skill_file, &mut provenance_resolver)?;
        refreshed_files.insert(skill_file_key(&skill_file));
        referenced_files.extend(skill.dependency_files.iter().cloned());
        refreshed_raw.push(skill);
    }
    while let Some(skill_file) = referenced_files.pop_front() {
        let key = skill_file_key(&skill_file);
        if known_files.contains(&key) || !skill_file.is_file() || !refreshed_files.insert(key) {
            continue;
        }
        let Some(skill_dir) = skill_file.parent() else {
            continue;
        };
        let root = SkillRoot {
            path: skill_dir.to_path_buf(),
            scope: "referenced".to_string(),
            agent: AgentKind::Unknown,
            plugin_id: None,
            plugin_enabled: None,
        };
        let skill = read_skill(&root, &skill_file, &mut provenance_resolver)?;
        referenced_files.extend(skill.dependency_files.iter().cloned());
        refreshed_raw.push(skill);
    }

    let mut names_by_file = remaining
        .iter()
        .flat_map(|skill| {
            skill.paths.iter().map(|path| {
                (
                    skill_file_key(&path.path.join("SKILL.md")),
                    skill.name.clone(),
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    names_by_file.extend(refreshed_raw.iter().map(|skill| {
        (
            skill_file_key(&skill.path.path.join("SKILL.md")),
            skill.name.clone(),
        )
    }));
    for skill in &mut refreshed_raw {
        let mut dependencies = skill.dependencies.iter().cloned().collect::<BTreeSet<_>>();
        dependencies.extend(
            skill
                .dependency_files
                .iter()
                .filter_map(|path| names_by_file.get(&skill_file_key(path)).cloned()),
        );
        skill.dependencies = dependencies.into_iter().collect();
    }

    remaining.extend(merge_raw_skills(refreshed_raw));
    remaining.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });
    resolve_scanned_skill_relations(&mut remaining);
    loop {
        let before_len = remaining.len();
        remaining.retain(|skill| {
            !skill.paths.iter().all(|path| path.scope == "referenced")
                || !skill.dependents.is_empty()
        });
        if remaining.len() == before_len {
            break;
        }
        resolve_scanned_skill_relations(&mut remaining);
    }

    let refreshed = SkillScan {
        roots: scan.roots,
        skills: remaining,
        warnings: scan.warnings,
    };
    if !sync_wrappers {
        return Ok(refreshed);
    }
    let changeset = plan_wrapper_sync(&refreshed)?;
    if changeset.changes.is_empty() {
        return Ok(refreshed);
    }
    apply_changes(&changeset)?;

    let wrapper_dirs = changeset
        .changes
        .iter()
        .filter_map(|change| change.path.parent().map(Path::to_path_buf))
        .collect::<BTreeSet<_>>();
    let wrapper_ids = refreshed
        .skills
        .iter()
        .filter(|skill| {
            skill.paths.iter().any(|path| {
                wrapper_dirs.contains(&path.path) && path.path.join("SKILL.md").is_file()
            })
        })
        .map(|skill| skill.id.clone())
        .collect::<Vec<_>>();
    if wrapper_ids.is_empty() {
        return Ok(refreshed);
    }
    refresh_skill_scan_with_resolver(
        cwd,
        refreshed,
        &wrapper_ids,
        &wrapper_dirs.into_iter().collect::<Vec<_>>(),
        provenance_resolver,
    )
}

pub fn plan_visibility(
    cwd: &Path,
    pattern: &str,
    visibility: SkillVisibility,
) -> Result<ChangeSet> {
    let scan = scan_skills(cwd)?;
    let matches = matching_skills(&scan, pattern);
    if matches.is_empty() {
        bail!("no skills matched pattern {pattern:?}");
    }

    let mut changes = Vec::new();
    for skill in &matches {
        for path in &skill.paths {
            changes.extend(plan_skill_visibility_at_path(
                &path.path, path.agent, visibility, true,
            )?);
        }
    }

    Ok(ChangeSet {
        changes: dedupe_changes(changes),
    })
}

pub fn plan_visibility_many_for_scan(
    scan: &SkillScan,
    skill_ids: &[String],
    visibility: SkillVisibility,
) -> Result<ChangeSet> {
    let matches = scan
        .skills
        .iter()
        .filter(|skill| skill_ids.iter().any(|id| skill_matches_id(skill, id)))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        bail!("no skills matched selected skill ids");
    }

    let mut changes = Vec::new();
    for skill in &matches {
        for path in &skill.paths {
            changes.extend(plan_skill_visibility_at_path(
                &path.path, path.agent, visibility, true,
            )?);
        }
    }

    Ok(ChangeSet {
        changes: dedupe_changes(changes),
    })
}

pub fn plan_wrapper(
    cwd: &Path,
    name: &str,
    pattern: &str,
    manual_children: bool,
) -> Result<ChangeSet> {
    let scan = scan_skills(cwd)?;
    let matches = matching_skills(&scan, pattern);
    if matches.is_empty() {
        bail!("no skills matched pattern {pattern:?}");
    }

    let target_root = scan
        .roots
        .iter()
        .find(|root| root.agent == AgentKind::Shared && root.scope == "global")
        .map(|root| root.path.clone())
        .or_else(|| {
            dirs::home_dir().and_then(|home| {
                crate::providers::agent_provider(AgentKind::Shared).global_skill_root(&home)
            })
        })
        .context("could not resolve wrapper target root")?;

    let wrapper_dir = target_root.join(name);
    let wrapper_file = wrapper_dir.join("SKILL.md");
    let before = read_optional(&wrapper_file)?;
    let after = render_wrapper_after(name, &matches, before.as_deref());
    let before_sha256 = before.as_ref().map(|text| sha256_text(text));

    let mut changes = vec![FileChange {
        path: wrapper_file,
        before_sha256,
        before,
        after,
    }];

    if manual_children {
        for skill in matches {
            if skill.name == name {
                continue;
            }
            for path in &skill.paths {
                changes.extend(plan_skill_visibility_at_path(
                    &path.path,
                    path.agent,
                    SkillVisibility::Manual,
                    true,
                )?);
            }
        }
    }

    Ok(ChangeSet {
        changes: dedupe_changes(changes),
    })
}

pub fn refresh_wrapper(
    cwd: &Path,
    name: &str,
    pattern: &str,
    manual_children: bool,
) -> Result<ChangeSet> {
    let scan = scan_skills(cwd)?;
    let wrapper_exists = scan.skills.iter().any(|skill| skill.name == name);
    if !wrapper_exists {
        bail!("wrapper skill {name:?} does not exist");
    }
    let matches = matching_skills(&scan, pattern)
        .into_iter()
        .filter(|skill| skill.name != name)
        .collect::<Vec<_>>();
    plan_wrapper_for_matches(&scan, name, matches, None, manual_children)
}

pub fn plan_wrapper_for_ids(
    scan: &SkillScan,
    name: &str,
    skill_ids: &[String],
    description: Option<&str>,
    manual_children: bool,
) -> Result<ChangeSet> {
    let matches = scan
        .skills
        .iter()
        .filter(|skill| skill_ids.iter().any(|id| skill_matches_id(skill, id)))
        .collect::<Vec<_>>();
    plan_wrapper_for_matches(scan, name, matches, description, manual_children)
}
pub fn refresh_wrapper_for_ids(
    scan: &SkillScan,
    name: &str,
    skill_ids: &[String],
    manual_children: bool,
) -> Result<ChangeSet> {
    let wrapper_exists = scan.skills.iter().any(|skill| skill.name == name);
    if !wrapper_exists {
        bail!("wrapper skill {name:?} does not exist");
    }
    let matches = scan
        .skills
        .iter()
        .filter(|skill| {
            skill.name != name && skill_ids.iter().any(|id| skill_matches_id(skill, id))
        })
        .collect::<Vec<_>>();
    plan_wrapper_for_matches(&scan, name, matches, None, manual_children)
}

pub fn plan_skill_delete_many(cwd: &Path, skill_ids: &[String]) -> Result<SkillDeletePlan> {
    let scan = scan_skills(cwd)?;
    plan_skill_delete_many_for_scan(&scan, skill_ids)
}

pub fn plan_skill_delete_many_for_scan(
    scan: &SkillScan,
    skill_ids: &[String],
) -> Result<SkillDeletePlan> {
    let matches = scan
        .skills
        .iter()
        .filter(|skill| skill_ids.iter().any(|id| skill_matches_id(skill, id)))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        bail!("no skills matched selected skill ids");
    }

    let mut seen = BTreeSet::new();
    let mut targets = Vec::new();
    for skill in &matches {
        if skill.is_system {
            bail!("refusing to delete read-only system skill {}", skill.name);
        }
        for path in &skill.paths {
            let key = path
                .path
                .canonicalize()
                .unwrap_or_else(|_| path.path.clone());
            if !seen.insert(key) {
                continue;
            }
            let metadata = fs::symlink_metadata(&path.path)
                .with_context(|| format!("failed to inspect {}", path.path.display()))?;
            let kind = if metadata.file_type().is_symlink() {
                "symlink"
            } else if metadata.is_dir() {
                "directory"
            } else {
                "file"
            };
            targets.push(SkillDeleteTarget {
                name: skill.name.clone(),
                path: path.path.clone(),
                kind: kind.to_string(),
            });
        }
    }

    let selected_ids = matches
        .iter()
        .map(|skill| ensure_skill_record_id(skill))
        .collect::<BTreeSet<_>>();
    let dependencies = matches
        .iter()
        .filter_map(|skill| {
            let related = skill
                .dependencies
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    skill
                        .dependency_ids
                        .get(*index)
                        .map(|id| !selected_ids.contains(id))
                        .unwrap_or(false)
                })
                .map(|(_, name)| name)
                .cloned()
                .collect::<Vec<_>>();
            (!related.is_empty()).then(|| SkillDeleteRelation {
                name: skill.name.clone(),
                related,
            })
        })
        .collect();
    let dependents = matches
        .iter()
        .filter_map(|skill| {
            let related = skill
                .dependent_ids
                .iter()
                .filter(|id| !selected_ids.contains(*id))
                .filter_map(|id| {
                    scan.skills
                        .iter()
                        .find(|candidate| ensure_skill_record_id(candidate) == *id)
                        .map(|candidate| candidate.name.clone())
                })
                .collect::<Vec<_>>();
            (!related.is_empty()).then(|| SkillDeleteRelation {
                name: skill.name.clone(),
                related,
            })
        })
        .collect();

    Ok(SkillDeletePlan {
        targets,
        dependencies,
        dependents,
    })
}

pub fn format_delete_plan(plan: &SkillDeletePlan) -> String {
    if plan.targets.is_empty() {
        return "no skills to delete".to_string();
    }

    let mut sections = Vec::new();
    sections.push(
        plan.targets
            .iter()
            .map(|target| {
                format!(
                    "D {} ({})\n  {}",
                    target.name,
                    target.kind,
                    target.path.display()
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
    );
    if !plan.dependents.is_empty() {
        sections.push(format!(
            "Dependents:\n{}",
            plan.dependents
                .iter()
                .map(|relation| format!(
                    "  {} is used by {}",
                    relation.name,
                    relation.related.join(", ")
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if !plan.dependencies.is_empty() {
        sections.push(format!(
            "Dependencies:\n{}",
            plan.dependencies
                .iter()
                .map(|relation| format!(
                    "  {} depends on {}",
                    relation.name,
                    relation.related.join(", ")
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    sections.join("\n\n")
}

pub fn skill_delete_resource_paths(plan: &SkillDeletePlan) -> Vec<PathBuf> {
    plan.targets
        .iter()
        .map(|target| target.path.clone())
        .collect()
}

pub fn changeset_resource_paths(changeset: &ChangeSet) -> Vec<PathBuf> {
    changeset
        .changes
        .iter()
        .map(|change| change.path.clone())
        .collect()
}

pub fn apply_skill_delete_plan(plan: &SkillDeletePlan) -> Result<()> {
    let _resources =
        crate::coordination::acquire_file_resources(&skill_delete_resource_paths(plan))?;
    for target in &plan.targets {
        let metadata = fs::symlink_metadata(&target.path)
            .with_context(|| format!("failed to inspect {}", target.path.display()))?;
        let current_kind = if metadata.file_type().is_symlink() {
            "symlink"
        } else if metadata.is_dir() {
            "directory"
        } else {
            "file"
        };
        if current_kind != target.kind {
            bail!(
                "skill installation identity changed since preview: {}",
                target.path.display()
            );
        }
        if metadata.file_type().is_symlink() || metadata.is_file() {
            fs::remove_file(&target.path)
                .with_context(|| format!("failed to delete {}", target.path.display()))?;
        } else if metadata.is_dir() {
            fs::remove_dir_all(&target.path)
                .with_context(|| format!("failed to delete {}", target.path.display()))?;
        } else {
            bail!(
                "refusing to delete unsupported path {}",
                target.path.display()
            );
        }
    }
    Ok(())
}

pub fn apply_changes(changeset: &ChangeSet) -> Result<()> {
    let _resources =
        crate::coordination::acquire_file_resources(&changeset_resource_paths(changeset))?;
    let mut applied = Vec::<(PathBuf, Option<Vec<u8>>)>::new();
    for change in &changeset.changes {
        let current = read_optional(&change.path)?;
        match (&change.before_sha256, &current) {
            (Some(expected), Some(text)) if *expected == sha256_text(text) => {}
            (Some(_), Some(_)) => {
                let error = anyhow::anyhow!(
                    "refusing to overwrite changed file {}",
                    change.path.display()
                );
                rollback_applied_files(&applied);
                return Err(error);
            }
            (Some(_), None) => {
                let error = anyhow::anyhow!(
                    "refusing to overwrite missing file {}",
                    change.path.display()
                );
                rollback_applied_files(&applied);
                return Err(error);
            }
            (None, None) => {}
            (None, Some(_)) => {
                let error = anyhow::anyhow!(
                    "refusing to overwrite existing file {}",
                    change.path.display()
                );
                rollback_applied_files(&applied);
                return Err(error);
            }
        }
        if let Err(error) = atomic_write(&change.path, &change.after) {
            rollback_applied_files(&applied);
            return Err(error);
        }
        let logger = crate::logging::global();
        if logger.debug_enabled() {
            logger.debug(
                "skill file change applied",
                serde_json::json!({
                    "operation": "apply_changes",
                    "path": &change.path,
                    "beforeSha256": current.as_deref().map(sha256_text),
                    "expectedBeforeSha256": &change.before_sha256,
                    "afterSha256": sha256_text(&change.after),
                }),
            );
        }
        applied.push((change.path.clone(), current.map(|text| text.into_bytes())));
    }
    Ok(())
}

fn rollback_applied_files(applied: &[(PathBuf, Option<Vec<u8>>)]) {
    for (path, before) in applied.iter().rev() {
        match before {
            Some(bytes) => {
                let _ = atomic_write_bytes(path, bytes);
            }
            None => {
                if path.exists() {
                    let _ = fs::remove_file(path);
                }
            }
        }
    }
}

pub fn materialize_skill_dir(
    source: &Path,
    agent: AgentKind,
    name: Option<&str>,
    dry_run: bool,
) -> Result<MaterializeResult> {
    materialize_skill_dir_mode(source, agent, name, false, false, dry_run)
}

pub fn materialize_skill_dir_mode(
    source: &Path,
    agent: AgentKind,
    name: Option<&str>,
    copy: bool,
    overwrite: bool,
    dry_run: bool,
) -> Result<MaterializeResult> {
    materialize_skill_dir_for_target(
        source,
        &agent.into(),
        SkillInstallScope::Global,
        Path::new("."),
        name,
        copy,
        overwrite,
        dry_run,
    )
}

pub fn materialize_skill_dir_for_target(
    source: &Path,
    target: &SkillTarget,
    scope: SkillInstallScope,
    cwd: &Path,
    name: Option<&str>,
    copy: bool,
    overwrite: bool,
    dry_run: bool,
) -> Result<MaterializeResult> {
    let resolved = source
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", source.display()))?;
    // A cache projection is promoted in place before materialization. Preview
    // and apply must infer the same name and therefore reserve the same target.
    let naming_source = if !copy
        && is_tendi_source_cache_path(&resolved)
        && fs::symlink_metadata(source)?.file_type().is_symlink()
    {
        source
    } else {
        &resolved
    };
    let skill_name = name
        .map(str::to_string)
        .or_else(|| {
            naming_source
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::to_string)
        })
        .context("could not infer skill name")?;
    let target_root = skill_target_root(cwd, target, scope)?;
    let destination = target_root.join(sanitize_skill_dir_name(&skill_name)?);
    let _resources = if dry_run {
        None
    } else {
        Some(crate::coordination::acquire_file_resources(&[
            source.to_path_buf(),
            destination,
        ])?)
    };
    if !copy && !dry_run {
        promote_tendi_cache_symlink(source)?;
    }
    let source = source
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", source.display()))?;
    if !source.join("SKILL.md").is_file() {
        bail!("{} is not a skill directory", source.display());
    }

    materialize_skill_dir_to_root(&source, &target_root, &skill_name, copy, overwrite, dry_run)
}

pub fn plan_skill_distribution(
    cwd: &Path,
    source: &Path,
    target: &SkillTarget,
    scope: SkillInstallScope,
    mode: SkillDistributionMode,
) -> Result<SkillDistributionPlan> {
    let scan = scan_skills(cwd)?;
    plan_skill_distribution_for_scan(cwd, &scan, source, target, scope, mode)
}

pub fn plan_skill_distribution_for_scan(
    cwd: &Path,
    scan: &SkillScan,
    source: &Path,
    target: &SkillTarget,
    scope: SkillInstallScope,
    mode: SkillDistributionMode,
) -> Result<SkillDistributionPlan> {
    let (skill_name, skill_path) = scan
        .skills
        .iter()
        .find_map(|skill| {
            skill
                .paths
                .iter()
                .find(|path| path.path == source)
                .map(|path| (skill.name.clone(), path.clone()))
        })
        .with_context(|| format!("skill installation was not found: {}", source.display()))?;
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect {}", source.display()))?;
    if !source.join("SKILL.md").is_file() {
        bail!("{} is not a skill directory", source.display());
    }
    if skill_path.scope.eq_ignore_ascii_case("plugin")
        || source.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|value| value == ".system" || value == "cache")
        })
    {
        bail!("refusing to distribute read-only skill {}", skill_name);
    }
    let canonical_source = source
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", source.display()))?;
    let source_sha256 = sha256_file(&source.join("SKILL.md"))?;
    let target_root = skill_target_root(cwd, target, scope)?;
    let destination = target_root.join(sanitize_skill_dir_name(&skill_name)?);
    ensure_path_inside(&target_root, &destination)?;

    let destination_metadata = fs::symlink_metadata(&destination).ok();
    let destination_exists = destination_metadata.is_some();
    let already_linked = destination_exists
        && destination
            .canonicalize()
            .ok()
            .is_some_and(|path| path == canonical_source);
    let (status, message) = if source == destination {
        ("already-at-destination".to_string(), None)
    } else if already_linked {
        (
            "already-installed".to_string(),
            Some("destination already links to this source".to_string()),
        )
    } else if destination_exists {
        (
            "conflict".to_string(),
            Some(format!("target already exists: {}", destination.display())),
        )
    } else {
        ("ready".to_string(), None)
    };
    let canonical_source = canonical_skill_dir(source);
    let projection_paths = scan
        .skills
        .iter()
        .flat_map(|skill| skill.paths.iter())
        .filter(|candidate| {
            candidate.path != source
                && fs::symlink_metadata(&candidate.path)
                    .map(|metadata| metadata.file_type().is_symlink())
                    .unwrap_or(false)
                && canonical_skill_dir(&candidate.path) == canonical_source
        })
        .map(|candidate| candidate.path.clone())
        .collect();

    Ok(SkillDistributionPlan {
        name: skill_name.clone(),
        source: source.to_path_buf(),
        destination,
        mode,
        source_symlink: metadata.file_type().is_symlink(),
        destination_exists,
        source_sha256,
        status,
        message,
        source_record: skill_source_record_for_path(
            &skill_name,
            &skill_path,
            source.to_path_buf(),
            "tendi-distribution",
        ),
        projection_paths,
    })
}

pub fn skill_distribution_resource_paths(plan: &SkillDistributionPlan) -> Vec<PathBuf> {
    let mut paths = vec![plan.source.clone(), plan.destination.clone()];
    paths.extend(plan.projection_paths.iter().cloned());
    paths
}

pub fn apply_skill_distribution_plan(plan: &SkillDistributionPlan) -> Result<MaterializeResult> {
    let _resources =
        crate::coordination::acquire_file_resources(&skill_distribution_resource_paths(plan))?;
    let destination_points_to_tendi_cache = plan
        .destination
        .canonicalize()
        .ok()
        .is_some_and(|path| is_tendi_source_cache_path(&path));
    let cache_link_needs_replacement =
        plan.status == "already-installed" && destination_points_to_tendi_cache;
    if plan.status == "already-at-destination"
        || (plan.status == "already-installed"
            && plan.mode == SkillDistributionMode::Symlink
            && !destination_points_to_tendi_cache)
    {
        return Ok(MaterializeResult {
            source: plan.source.clone(),
            target: plan.destination.clone(),
            mode: match plan.mode {
                SkillDistributionMode::Move => "move",
                SkillDistributionMode::Symlink => "symlink",
                SkillDistributionMode::Copy => "copy",
            }
            .to_string(),
            health: if plan.status == "already-at-destination" {
                "already-at-destination"
            } else {
                "symlink-ok"
            }
            .to_string(),
            applied: false,
        });
    }
    if plan.status == "conflict" || (plan.destination_exists && !cache_link_needs_replacement) {
        bail!("target already exists: {}", plan.destination.display());
    }
    if fs::symlink_metadata(&plan.destination).is_ok() && !cache_link_needs_replacement {
        bail!(
            "target appeared since preview: {}",
            plan.destination.display()
        );
    }

    let current_sha256 = sha256_file(&plan.source.join("SKILL.md"))?;
    if current_sha256 != plan.source_sha256 {
        bail!("source changed since preview: {}", plan.source.display());
    }
    let source = plan
        .source
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", plan.source.display()))?;
    if !source.join("SKILL.md").is_file() {
        bail!("{} is not a skill directory", plan.source.display());
    }
    let source_is_tendi_cache = is_tendi_source_cache_path(&source);
    let promoted = source_is_tendi_cache
        && plan.source_symlink
        && matches!(
            plan.mode,
            SkillDistributionMode::Move | SkillDistributionMode::Symlink
        )
        && promote_tendi_cache_symlink(&plan.source)?;
    let source = plan
        .source
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", plan.source.display()))?;
    let target_root = plan
        .destination
        .parent()
        .context("skill distribution destination has no parent")?;
    fs::create_dir_all(target_root)?;
    if cache_link_needs_replacement {
        fs::remove_file(&plan.destination).with_context(|| {
            format!(
                "failed to remove cached skill projection {}",
                plan.destination.display()
            )
        })?;
    }

    match plan.mode {
        SkillDistributionMode::Move => {
            if plan.source_symlink && !promoted {
                if source_is_tendi_cache {
                    copy_dir(&source, &plan.destination)?;
                    return Ok(MaterializeResult {
                        source: plan.source.clone(),
                        target: plan.destination.clone(),
                        mode: "copy".to_string(),
                        health: "copy-ok".to_string(),
                        applied: true,
                    });
                }
                create_symlink(&source, &plan.destination).with_context(|| {
                    format!(
                        "failed to link {} to {}",
                        source.display(),
                        plan.destination.display()
                    )
                })?;
                if let Err(error) = fs::remove_file(&plan.source) {
                    let _ = fs::remove_file(&plan.destination);
                    return Err(error)
                        .with_context(|| format!("failed to remove {}", plan.source.display()));
                }
            } else {
                move_canonical_skill_and_relink_projections(
                    &plan.source,
                    &plan.destination,
                    &plan.projection_paths,
                )?;
            }
            Ok(MaterializeResult {
                source: plan.source.clone(),
                target: plan.destination.clone(),
                mode: "move".to_string(),
                health: "move-ok".to_string(),
                applied: true,
            })
        }
        SkillDistributionMode::Symlink => {
            if source_is_tendi_cache && !promoted {
                copy_dir(&source, &plan.destination)?;
                return Ok(MaterializeResult {
                    source: plan.source.clone(),
                    target: plan.destination.clone(),
                    mode: "copy".to_string(),
                    health: "copy-ok".to_string(),
                    applied: true,
                });
            }
            let link_source = if promoted { &plan.source } else { &source };
            create_symlink(link_source, &plan.destination).with_context(|| {
                format!(
                    "failed to link {} to {}",
                    link_source.display(),
                    plan.destination.display()
                )
            })?;
            Ok(MaterializeResult {
                source: plan.source.clone(),
                target: plan.destination.clone(),
                mode: "symlink".to_string(),
                health: "symlink-ok".to_string(),
                applied: true,
            })
        }
        SkillDistributionMode::Copy => {
            copy_dir(&source, &plan.destination)?;
            Ok(MaterializeResult {
                source: plan.source.clone(),
                target: plan.destination.clone(),
                mode: "copy".to_string(),
                health: "copy-ok".to_string(),
                applied: true,
            })
        }
    }
}

fn move_canonical_skill_and_relink_projections(
    source: &Path,
    destination: &Path,
    projections: &[PathBuf],
) -> Result<()> {
    move_canonical_skill_and_relink_projections_with_destination(
        source,
        destination,
        projections,
        false,
    )
}

/// Move a canonical skill into one of its existing projections while preserving
/// the other projections. This is used when a user removes the current
/// canonical installation but keeps another provider location enabled.
pub fn rehome_canonical_skill_and_relink_projections(
    source: &Path,
    destination: &Path,
    projections: &[PathBuf],
) -> Result<()> {
    let mut resources = vec![source.to_path_buf(), destination.to_path_buf()];
    resources.extend_from_slice(projections);
    let _resources = crate::coordination::acquire_file_resources(&resources)?;
    move_canonical_skill_and_relink_projections_with_destination(
        source,
        destination,
        projections,
        true,
    )
}

fn move_canonical_skill_and_relink_projections_with_destination(
    source: &Path,
    destination: &Path,
    projections: &[PathBuf],
    replace_destination_projection: bool,
) -> Result<()> {
    let destination_link = if replace_destination_projection {
        match fs::symlink_metadata(destination) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let resolved = destination.canonicalize().with_context(|| {
                    format!(
                        "failed to resolve destination projection {}",
                        destination.display()
                    )
                })?;
                let canonical_source = source
                    .canonicalize()
                    .with_context(|| format!("failed to resolve {}", source.display()))?;
                if resolved != canonical_source {
                    bail!(
                        "destination projection {} does not point to {}",
                        destination.display(),
                        source.display()
                    );
                }
                Some(fs::read_link(destination).with_context(|| {
                    format!("failed to read projection {}", destination.display())
                })?)
            }
            Ok(_) => bail!(
                "cannot rehome skill into existing non-symlink {}",
                destination.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect {}", destination.display()));
            }
        }
    } else {
        None
    };

    let previous_links = projections
        .iter()
        .filter(|path| *path != destination)
        .filter_map(|path| {
            fs::read_link(path)
                .ok()
                .map(|target| (path.clone(), target))
        })
        .collect::<Vec<_>>();

    if destination_link.is_some() {
        fs::remove_file(destination).with_context(|| {
            format!(
                "failed to remove destination projection {}",
                destination.display()
            )
        })?;
    }

    if let Err(error) = fs::rename(source, destination) {
        if let Some(target) = destination_link {
            let _ = create_symlink(&target, destination);
        }
        return Err(error).with_context(|| {
            format!(
                "failed to move {} to {}",
                source.display(),
                destination.display()
            )
        });
    }

    let mut relinked = Vec::new();
    for path in projections {
        if path == destination {
            continue;
        }
        let result = (|| -> Result<()> {
            fs::remove_file(path)
                .with_context(|| format!("failed to remove projection {}", path.display()))?;
            create_symlink(destination, path).with_context(|| {
                format!(
                    "failed to relink projection {} to {}",
                    path.display(),
                    destination.display()
                )
            })?;
            Ok(())
        })();
        if let Err(error) = result {
            for path in relinked.iter().rev() {
                let _ = fs::remove_file(path);
            }
            let _ = fs::rename(destination, source);
            for (path, target) in &previous_links {
                if fs::symlink_metadata(path).is_err() {
                    let _ = create_symlink(target, path);
                }
            }
            if let Some(target) = destination_link {
                let _ = create_symlink(&target, destination);
            }
            return Err(error);
        }
        relinked.push(path);
    }
    Ok(())
}

pub fn skill_source_record_for_path(
    skill_name: &str,
    path: &SkillPath,
    skill_path: PathBuf,
    origin: &str,
) -> SkillSourceRecord {
    SkillSourceRecord {
        skill_name: skill_name.to_string(),
        skill_path,
        source_kind: path.source_kind.clone(),
        source: path.source.clone(),
        source_ref: path.source_ref.clone(),
        source_version: path.source_version.clone(),
        source_relative_path: path.source_relative_path.clone(),
        update_status: path.update_status.clone(),
        origin: origin.to_string(),
    }
}

fn materialize_skill_dir_to_root(
    source: &Path,
    target_root: &Path,
    name: &str,
    copy: bool,
    overwrite: bool,
    dry_run: bool,
) -> Result<MaterializeResult> {
    let target = target_root.join(sanitize_skill_dir_name(name)?);
    let _resources = if dry_run {
        None
    } else {
        Some(crate::coordination::acquire_file_resources(&[
            source.to_path_buf(),
            target,
        ])?)
    };
    let source = source
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", source.display()))?;
    if !source.join("SKILL.md").is_file() {
        bail!("{} is not a skill directory", source.display());
    }
    let copy = copy || is_tendi_source_cache_path(&source);
    let skill_name = sanitize_skill_dir_name(name)?;
    let target = target_root.join(&skill_name);
    ensure_path_inside(target_root, &target)?;
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if target == source {
            return Ok(MaterializeResult {
                source,
                target,
                mode: if copy { "copy" } else { "symlink" }.to_string(),
                health: "already-at-destination".to_string(),
                applied: false,
            });
        }
        let target_points_to_source = target.canonicalize().ok().as_ref() == Some(&source);
        if !copy && target_points_to_source {
            return Ok(MaterializeResult {
                source,
                target,
                mode: "symlink".to_string(),
                health: "symlink-ok".to_string(),
                applied: false,
            });
        }
        if copy && target_points_to_source && !is_tendi_source_cache_path(&source) {
            bail!("target already exists: {}", target.display());
        }
        if !overwrite {
            if !(copy && target_points_to_source && is_tendi_source_cache_path(&source)) {
                bail!("target already exists: {}", target.display());
            }
        }
        if dry_run {
            return Ok(MaterializeResult {
                source,
                target,
                mode: if copy { "copy" } else { "symlink" }.to_string(),
                health: "replace-planned".to_string(),
                applied: false,
            });
        }
        if metadata.is_dir() {
            fs::remove_dir_all(&target)
                .with_context(|| format!("failed to replace {}", target.display()))?;
        } else {
            fs::remove_file(&target)
                .with_context(|| format!("failed to replace {}", target.display()))?;
        }
    }

    if dry_run {
        return Ok(MaterializeResult {
            source,
            target,
            mode: if copy { "copy" } else { "symlink" }.to_string(),
            health: "planned".to_string(),
            applied: false,
        });
    }

    fs::create_dir_all(&target_root)?;
    if copy {
        copy_dir(&source, &target)?;
        let result = MaterializeResult {
            source,
            target,
            mode: "copy".to_string(),
            health: "copy-ok".to_string(),
            applied: true,
        };
        log_skill_materialization(&result);
        return Ok(result);
    }

    match create_symlink(&source, &target) {
        Ok(()) if target.join("SKILL.md").is_file() => {
            let result = MaterializeResult {
                source,
                target,
                mode: "symlink".to_string(),
                health: "symlink-ok".to_string(),
                applied: true,
            };
            log_skill_materialization(&result);
            Ok(result)
        }
        _ => {
            let _ = fs::remove_file(&target);
            copy_dir(&source, &target)?;
            let result = MaterializeResult {
                source,
                target,
                mode: "copy".to_string(),
                health: "copy-fallback".to_string(),
                applied: true,
            };
            log_skill_materialization(&result);
            Ok(result)
        }
    }
}

fn log_skill_materialization(result: &MaterializeResult) {
    let logger = crate::logging::global();
    if logger.debug_enabled() {
        logger.debug(
            "skill directory materialized",
            serde_json::json!({
                "operation": "materialize_skill_dir_to_root",
                "source": &result.source,
                "target": &result.target,
                "mode": &result.mode,
                "health": &result.health,
                "sourceSkillSha256": sha256_file(&result.source.join("SKILL.md")).ok(),
                "targetSkillSha256": sha256_file(&result.target.join("SKILL.md")).ok(),
            }),
        );
    }
}

pub fn list_installable_skills(cwd: &Path, source: &str) -> Result<SkillAddPlan> {
    let resolved = resolve_add_source(cwd, source, false)?;
    let available = discover_installable_skills(&resolved.root)?;
    let plan = SkillAddPlan {
        source: resolved.display_source.clone(),
        source_kind: resolved.kind.clone(),
        source_ref: resolved.git_ref.clone(),
        source_root: resolved.root.clone(),
        target: AgentKind::Shared.into(),
        scope: SkillInstallScope::Global,
        mode: "list".to_string(),
        available: available.clone(),
        selected: available,
        operations: Vec::new(),
    };
    cleanup_resolved_source(&resolved);
    Ok(plan)
}

pub fn plan_skill_add(cwd: &Path, options: &SkillAddOptions) -> Result<SkillAddPlan> {
    let resolved = resolve_add_source(cwd, &options.source, true)?;
    let result = build_skill_add_plan(cwd, &resolved, options, true);
    cleanup_resolved_source(&resolved);
    result
}

pub fn skill_add_preparation_resource_paths(
    cwd: &Path,
    options: &SkillAddOptions,
) -> Result<Vec<PathBuf>> {
    let parsed = parse_add_source(cwd, &options.source)?;
    if parsed.kind == "local" {
        return Ok(Vec::new());
    }
    let key = match parsed.kind.as_str() {
        "github" | "git" | "gitlab" | "huggingface" => parsed
            .git_ref
            .map(|git_ref| format!("{}#{git_ref}", parsed.url))
            .unwrap_or(parsed.url),
        "well-known" | "clawhub" => parsed.url,
        _ => bail!("unsupported skill source {}", options.source),
    };
    let root = persistent_source_root(&key)?;
    let mut paths = vec![root.clone()];
    if matches!(
        parsed.kind.as_str(),
        "github" | "git" | "gitlab" | "huggingface"
    ) {
        paths.extend(git::mutation_resource_paths(&root)?);
    }
    Ok(paths)
}

pub fn skill_update_preparation_resource_paths(
    scan: &SkillScan,
    skill_ids: &[String],
) -> Result<Vec<PathBuf>> {
    let mut paths = BTreeSet::new();
    for skill in scan
        .skills
        .iter()
        .filter(|skill| skill_ids.iter().any(|id| skill_matches_id(skill, id)))
    {
        for path in &skill.paths {
            if !is_git_source_kind(&path.source_kind) {
                continue;
            }
            if let Some(repo) = git_repository_boundary(&path.path) {
                paths.extend(git::mutation_resource_paths(&repo)?);
                paths.insert(repo);
            } else if let Some(source) = &path.source {
                let key = path
                    .source_ref
                    .as_ref()
                    .map(|reference| format!("{source}#{reference}"))
                    .unwrap_or_else(|| source.clone());
                let repo = persistent_source_root(&key)?;
                paths.extend(git::mutation_resource_paths(&repo)?);
                paths.insert(repo);
            }
        }
    }
    Ok(paths.into_iter().collect())
}

pub fn apply_skill_add(cwd: &Path, options: &SkillAddOptions) -> Result<SkillAddApplyReport> {
    let target_root = skill_target_root(cwd, &options.target, options.scope)?;
    apply_skill_add_with_target_root(cwd, options, &target_root)
}

pub fn skill_add_catalog_fingerprint(plan: &SkillAddPlan) -> Result<String> {
    let mut catalog = BTreeSet::new();
    for skill in &plan.available {
        for entry in WalkDir::new(&skill.path).follow_links(true) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            catalog.insert(format!(
                "{}:{}",
                entry.path().display(),
                sha256_file(entry.path())?
            ));
        }
    }
    Ok(sha256_text(
        &catalog.into_iter().collect::<Vec<_>>().join("\n"),
    ))
}

pub fn apply_skill_add_preview(
    preview: &SkillAddPlan,
    options: &SkillAddOptions,
) -> Result<SkillAddApplyReport> {
    // `source` is normalized in the preview (for example GitHub shorthand becomes
    // a clone URL). The preview already owns the resolved catalog and source root,
    // so compare the options that can still change the resulting installation.
    if preview.target != options.target || preview.scope != options.scope || preview.mode != "copy"
    {
        bail!("skill add options changed; preview the installation again");
    }
    let target_root = preview
        .operations
        .first()
        .and_then(|operation| operation.target.parent())
        .map(Path::to_path_buf)
        .context("skill add preview contains no target operations")?;
    let resolved = ResolvedAddSource {
        root: preview.source_root.clone(),
        kind: preview.source_kind.clone(),
        display_source: preview.source.clone(),
        git_ref: preview.source_ref.clone(),
        temporary: false,
    };
    let plan = build_skill_add_plan_from_available(
        &resolved,
        options,
        false,
        &target_root,
        preview.available.clone(),
    )?;
    apply_built_skill_add_plan(plan, options, &target_root)
}

pub fn skill_source_records_for_add(report: &SkillAddApplyReport) -> Vec<SkillSourceRecord> {
    let git_source = matches!(
        report.plan.source_kind.as_str(),
        "github" | "git" | "gitlab" | "huggingface"
    );
    let source_version = git_source
        .then(|| git_output(&report.plan.source_root, &["rev-parse", "HEAD"]))
        .flatten();
    let source_repo = git_source
        .then(|| git_repository_boundary(&report.plan.source_root))
        .flatten();
    report
        .plan
        .selected
        .iter()
        .zip(&report.results)
        .map(|(skill, result)| SkillSourceRecord {
            skill_name: skill.name.clone(),
            skill_path: result.target.clone(),
            source_kind: report.plan.source_kind.clone(),
            source: Some(report.plan.source.clone()),
            source_ref: report.plan.source_ref.clone(),
            source_version: source_version.clone(),
            source_relative_path: source_repo
                .as_deref()
                .and_then(|repo| {
                    skill
                        .path
                        .canonicalize()
                        .ok()
                        .and_then(|path| path.strip_prefix(repo).ok().map(Path::to_path_buf))
                })
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .or_else(|| (!skill.relative_path.is_empty()).then(|| skill.relative_path.clone())),
            update_status: if matches!(
                report.plan.source_kind.as_str(),
                "github" | "git" | "gitlab" | "huggingface"
            ) {
                "tracked"
            } else {
                "local"
            }
            .to_string(),
            origin: "tendi-install".to_string(),
        })
        .collect()
}

pub fn capture_skill_snapshots(records: &[SkillSourceRecord]) -> Result<Vec<SkillSnapshot>> {
    let mut snapshots = Vec::new();
    for record in records {
        if is_git_source_kind(&record.source_kind) {
            continue;
        }
        let Some(source_version) = record.source_version.as_deref() else {
            continue;
        };
        if !record.skill_path.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        for entry in WalkDir::new(&record.skill_path)
            .follow_links(true)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let relative_path = entry
                .path()
                .strip_prefix(&record.skill_path)
                .with_context(|| {
                    format!("failed to resolve snapshot path {}", entry.path().display())
                })?
                .to_string_lossy()
                .replace('\\', "/");
            files.push(SkillSnapshotFile {
                relative_path,
                content: fs::read(entry.path())
                    .with_context(|| format!("failed to read {}", entry.path().display()))?,
            });
        }
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        if !files.is_empty() {
            snapshots.push(SkillSnapshot {
                skill_path: record.skill_path.clone(),
                source_version: source_version.to_string(),
                files,
            });
        }
    }
    Ok(snapshots)
}

fn apply_skill_add_with_target_root(
    cwd: &Path,
    options: &SkillAddOptions,
    target_root: &Path,
) -> Result<SkillAddApplyReport> {
    let resolved = resolve_add_source(cwd, &options.source, true)?;
    let plan = build_skill_add_plan_with_target_root(&resolved, options, false, target_root)?;
    let report = apply_built_skill_add_plan(plan, options, target_root);
    cleanup_resolved_source(&resolved);
    report
}

pub fn skill_add_resource_paths(plan: &SkillAddPlan) -> Result<Vec<PathBuf>> {
    let agent = plan.target.agent_kind()?;
    let mut paths = Vec::new();
    for operation in &plan.operations {
        paths.push(operation.source.clone());
        paths.extend(skill_visibility_resource_paths(
            &operation.target,
            agent,
            plan.target.uses_shared_layout(),
        ));
    }
    Ok(paths)
}

fn skill_visibility_resource_paths(
    skill_dir: &Path,
    agent: AgentKind,
    update_provider_config: bool,
) -> Vec<PathBuf> {
    let mut paths = crate::providers::agent_provider(agent)
        .skill_mutation_resource_paths(skill_dir, update_provider_config);
    if matches!(
        agent,
        AgentKind::Cursor | AgentKind::Claude | AgentKind::Shared
    ) {
        paths.extend(
            crate::providers::agent_provider(AgentKind::Codex)
                .skill_mutation_resource_paths(skill_dir, update_provider_config),
        );
    }
    paths
}

fn apply_built_skill_add_plan(
    plan: SkillAddPlan,
    options: &SkillAddOptions,
    target_root: &Path,
) -> Result<SkillAddApplyReport> {
    let _resources =
        crate::coordination::acquire_file_resources(&skill_add_resource_paths(&plan)?)?;
    let mut results = Vec::new();
    let mut visibility_changes = Vec::new();
    for skill in &plan.selected {
        let result = materialize_skill_dir_to_root(
            &skill.path,
            target_root,
            &skill.name,
            true,
            options.overwrite,
            false,
        )?;
        visibility_changes.extend(plan_skill_visibility_at_path(
            &result.target,
            options.target.agent_kind()?,
            options.visibility,
            options.target.uses_shared_layout(),
        )?);
        results.push(result);
    }
    apply_changes(&ChangeSet {
        changes: dedupe_changes(visibility_changes),
    })?;
    Ok(SkillAddApplyReport { plan, results })
}

#[derive(Debug, Clone)]
struct ResolvedAddSource {
    root: PathBuf,
    kind: String,
    display_source: String,
    git_ref: Option<String>,
    temporary: bool,
}

fn build_skill_add_plan(
    cwd: &Path,
    resolved: &ResolvedAddSource,
    options: &SkillAddOptions,
    dry_run: bool,
) -> Result<SkillAddPlan> {
    let target_root = skill_target_root(cwd, &options.target, options.scope)?;
    build_skill_add_plan_with_target_root(resolved, options, dry_run, &target_root)
}

fn build_skill_add_plan_with_target_root(
    resolved: &ResolvedAddSource,
    options: &SkillAddOptions,
    dry_run: bool,
    target_root: &Path,
) -> Result<SkillAddPlan> {
    let available = discover_installable_skills(&resolved.root)?;
    build_skill_add_plan_from_available(resolved, options, dry_run, target_root, available)
}

fn build_skill_add_plan_from_available(
    resolved: &ResolvedAddSource,
    options: &SkillAddOptions,
    dry_run: bool,
    target_root: &Path,
    available: Vec<InstallableSkill>,
) -> Result<SkillAddPlan> {
    if available.is_empty() {
        bail!("no skills found in {}", resolved.display_source);
    }

    let (available, selected) = select_installable_skills(available, &options.skills)?;
    let selected = expand_installable_dependencies(&available, selected);
    // An add always creates or updates the canonical materialized installation.
    // Provider projections are created through the distribution/link API.
    let mode = "copy".to_string();
    let mut operations = Vec::new();
    for skill in &selected {
        let source = skill
            .path
            .canonicalize()
            .with_context(|| format!("failed to canonicalize {}", skill.path.display()))?;
        let target = target_root.join(sanitize_skill_dir_name(&skill.name)?);
        ensure_path_inside(&target_root, &target)?;
        let mut status = if dry_run { "planned" } else { "ready" }.to_string();
        let mut message = None;
        if fs::symlink_metadata(&target).is_ok() {
            if !options.copy && target.canonicalize().ok().as_ref() == Some(&source) {
                status = "already-installed".to_string();
                message = Some("already points at this source".to_string());
            } else if options.overwrite {
                status = if dry_run { "replace" } else { "ready" }.to_string();
                message = Some(format!(
                    "will replace existing target: {}",
                    target.display()
                ));
            } else {
                status = "already-exists".to_string();
                message = Some(format!("target already exists: {}", target.display()));
            }
        }
        operations.push(SkillAddOperation {
            name: skill.name.clone(),
            source,
            target,
            mode: mode.clone(),
            status,
            message,
        });
    }

    Ok(SkillAddPlan {
        source: resolved.display_source.clone(),
        source_kind: resolved.kind.clone(),
        source_ref: resolved.git_ref.clone(),
        source_root: resolved.root.clone(),
        target: options.target.clone(),
        scope: options.scope,
        mode,
        available,
        selected,
        operations,
    })
}

fn select_installable_skills(
    available: Vec<InstallableSkill>,
    names: &[String],
) -> Result<(Vec<InstallableSkill>, Vec<InstallableSkill>)> {
    if names.is_empty() || names.iter().any(|name| name == "*") {
        let selected = available.clone();
        return Ok((available, selected));
    }

    let mut selected = Vec::new();
    let mut missing = Vec::new();
    for name in names {
        let Some(skill) = available.iter().find(|skill| {
            skill.name.eq_ignore_ascii_case(name)
                || normalize_skill_match_name(&skill.name) == normalize_skill_match_name(name)
        }) else {
            missing.push(name.clone());
            continue;
        };
        if !selected
            .iter()
            .any(|selected: &InstallableSkill| selected.name == skill.name)
        {
            selected.push(skill.clone());
        }
    }

    if !missing.is_empty() {
        let available_names = available
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "no matching skills found for {}; available skills: {}",
            missing.join(", "),
            available_names
        );
    }

    Ok((available, selected))
}

fn expand_installable_dependencies(
    available: &[InstallableSkill],
    selected: Vec<InstallableSkill>,
) -> Vec<InstallableSkill> {
    let by_name = available
        .iter()
        .map(|skill| (skill.name.as_str(), skill))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut expanded = Vec::with_capacity(selected.len());
    let mut stack = Vec::new();

    for skill in selected {
        if !seen.insert(skill.name.clone()) {
            continue;
        }
        for dependency in skill.dependencies.iter().rev() {
            if !seen.contains(dependency) {
                stack.push(dependency.clone());
            }
        }
        expanded.push(skill);
    }

    while let Some(name) = stack.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(skill) = by_name.get(name.as_str()) else {
            continue;
        };
        for dependency in skill.dependencies.iter().rev() {
            if !seen.contains(dependency) {
                stack.push(dependency.clone());
            }
        }
        expanded.push((*skill).clone());
    }

    expanded.sort_by(|left, right| left.name.cmp(&right.name));
    expanded
}

fn normalize_skill_match_name(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn resolve_add_source(
    cwd: &Path,
    source: &str,
    persistent_remote: bool,
) -> Result<ResolvedAddSource> {
    let parsed = parse_add_source(cwd, source)?;
    match parsed.kind.as_str() {
        "local" => Ok(ResolvedAddSource {
            root: parsed
                .root
                .ok_or_else(|| anyhow::anyhow!("local skill source has no local root"))?,
            kind: "local".to_string(),
            display_source: source.to_string(),
            git_ref: None,
            temporary: false,
        }),
        "github" | "git" | "gitlab" | "huggingface" => {
            let cache_key = match &parsed.git_ref {
                Some(git_ref) => format!("{}#{git_ref}", parsed.url),
                None => parsed.url.clone(),
            };
            let root = if persistent_remote {
                persistent_source_root(&cache_key)?
            } else {
                temporary_source_root(&cache_key)?
            };
            let _resources =
                crate::coordination::acquire_file_resources(std::slice::from_ref(&root))?;
            if !root.join(".git").is_dir() {
                if root.exists() {
                    fs::remove_dir_all(&root)
                        .with_context(|| format!("failed to reset {}", root.display()))?;
                }
                if let Some(parent) = root.parent() {
                    fs::create_dir_all(parent)?;
                }
                run_git_clone(
                    &parsed.url,
                    parsed.git_ref.as_deref(),
                    &root,
                    git::never_cancelled(),
                )?;
            }
            let discovery_root = match parsed.subpath {
                Some(subpath) => {
                    let candidate = root.join(subpath);
                    let canonical_root = root.canonicalize()?;
                    let canonical_candidate = candidate.canonicalize().with_context(|| {
                        format!(
                            "skill source subpath does not exist: {}",
                            candidate.display()
                        )
                    })?;
                    if !canonical_candidate.starts_with(&canonical_root) {
                        bail!("skill source subpath escapes the cloned repository");
                    }
                    canonical_candidate
                }
                None => root,
            };
            Ok(ResolvedAddSource {
                root: discovery_root,
                kind: parsed.kind,
                display_source: parsed.url,
                git_ref: parsed.git_ref,
                temporary: !persistent_remote,
            })
        }
        "well-known" | "clawhub" => {
            let root = if persistent_remote {
                persistent_source_root(&parsed.url)?
            } else {
                temporary_source_root(&parsed.url)?
            };
            let _resources =
                crate::coordination::acquire_file_resources(std::slice::from_ref(&root))?;
            if !root.exists() {
                if let Some(parent) = root.parent() {
                    fs::create_dir_all(parent)?;
                }
                if let Err(error) = crate::skill_source::materialize_well_known(&parsed.url, &root)
                {
                    let _ = fs::remove_dir_all(&root);
                    return Err(error);
                }
            }
            Ok(ResolvedAddSource {
                root,
                kind: parsed.kind,
                display_source: parsed.url,
                git_ref: None,
                temporary: !persistent_remote,
            })
        }
        _ => bail!("unsupported skill source {}", source),
    }
}

#[derive(Debug, Clone)]
struct ParsedAddSource {
    kind: String,
    root: Option<PathBuf>,
    url: String,
    git_ref: Option<String>,
    subpath: Option<PathBuf>,
}

fn parse_add_source(cwd: &Path, source: &str) -> Result<ParsedAddSource> {
    let parsed = crate::skill_source::parse(cwd, source)?;
    Ok(ParsedAddSource {
        kind: parsed.kind,
        root: parsed.local_root,
        url: parsed.url,
        git_ref: parsed.git_ref,
        subpath: parsed.subpath,
    })
}

fn persistent_source_root(source: &str) -> Result<PathBuf> {
    let hash = short_sha(source, 12);
    Ok(tendi_state_root()?
        .join("sources")
        .join(format!("{}-{hash}", sanitize_skill_dir_name(source)?)))
}

fn tendi_state_root() -> Result<PathBuf> {
    #[cfg(test)]
    crate::test_support::ensure_isolated_environment();

    dirs::home_dir()
        .map(|home| home.join(".tendi"))
        .context("could not resolve Tendi state directory")
}

fn is_tendi_source_cache_path(path: &Path) -> bool {
    let mut roots = Vec::new();
    if let Ok(root) = tendi_state_root() {
        roots.push(root.join("sources"));
    }
    if let Ok(db_path) = crate::storage::default_db_path()
        && let Some(parent) = db_path.parent()
    {
        roots.push(parent.join("sources"));
    }
    roots.into_iter().any(|root| path.starts_with(root))
}

fn promote_tendi_cache_symlink(path: &Path) -> Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };
    if !metadata.file_type().is_symlink() {
        return Ok(false);
    }
    let resolved = path
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))?;
    if !is_tendi_source_cache_path(&resolved) {
        return Ok(false);
    }
    if !resolved.join("SKILL.md").is_file() {
        bail!("{} is not a skill directory", resolved.display());
    }
    let parent = path
        .parent()
        .context("managed skill symlink has no parent directory")?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("skill");
    let sequence = CANONICAL_MATERIALIZATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{name}.tendi-canonical-{}-{sequence}",
        std::process::id()
    ));
    if temporary.exists() {
        fs::remove_dir_all(&temporary)
            .with_context(|| format!("failed to clear {}", temporary.display()))?;
    }
    copy_dir(&resolved, &temporary)?;
    if let Err(error) = fs::remove_file(path) {
        let _ = fs::remove_dir_all(&temporary);
        return Err(error).with_context(|| format!("failed to replace {}", path.display()));
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = create_symlink(&resolved, path);
        let _ = fs::remove_dir_all(&temporary);
        return Err(error).with_context(|| format!("failed to promote {}", path.display()));
    }
    let logger = crate::logging::global();
    if logger.debug_enabled() {
        let source_skill_sha256 = sha256_file(&resolved.join("SKILL.md")).ok();
        logger.debug(
            "Tendi cache symlink promoted to directory",
            serde_json::json!({
                "operation": "promote_tendi_cache_symlink",
                "source": resolved,
                "target": path,
                "sourceSkillSha256": source_skill_sha256,
                "targetSkillSha256": sha256_file(&path.join("SKILL.md")).ok(),
            }),
        );
    }
    Ok(true)
}

pub(crate) fn materialize_tendi_cache_links(scan: &SkillScan) -> Result<bool> {
    let mut groups = BTreeMap::<PathBuf, Vec<PathBuf>>::new();
    for skill in &scan.skills {
        for path in &skill.paths {
            let metadata = match fs::symlink_metadata(&path.path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if !metadata.file_type().is_symlink() {
                continue;
            }
            let Ok(resolved) = path.path.canonicalize() else {
                continue;
            };
            if is_tendi_source_cache_path(&resolved) {
                groups.entry(resolved).or_default().push(path.path.clone());
            }
        }
    }

    let mut changed = false;
    for paths in groups.values_mut() {
        let _resources = crate::coordination::acquire_file_resources(paths)?;
        paths.sort();
        let Some(canonical_path) = paths.first().cloned() else {
            continue;
        };
        changed |= promote_tendi_cache_symlink(&canonical_path)?;
        for projection in paths.iter().skip(1) {
            let metadata = fs::symlink_metadata(projection).with_context(|| {
                format!(
                    "failed to inspect cache projection {}",
                    projection.display()
                )
            })?;
            if !metadata.file_type().is_symlink() {
                continue;
            }
            fs::remove_file(projection).with_context(|| {
                format!("failed to remove cache projection {}", projection.display())
            })?;
            create_symlink(&canonical_path, projection).with_context(|| {
                format!(
                    "failed to relink cache projection {} to {}",
                    projection.display(),
                    canonical_path.display()
                )
            })?;
            changed = true;
        }
    }
    Ok(changed)
}

fn temporary_source_root(source: &str) -> Result<PathBuf> {
    let hash = short_sha(
        &format!(
            "{}-{}",
            source,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ),
        12,
    );
    Ok(std::env::temp_dir().join(format!("tendi-skill-add-{hash}")))
}

fn cleanup_resolved_source(source: &ResolvedAddSource) {
    if source.temporary {
        let cleanup_root = source
            .root
            .ancestors()
            .find(|path| path.join(".git").is_dir())
            .unwrap_or(&source.root);
        let _ = fs::remove_dir_all(cleanup_root);
    }
}

fn run_git_clone(
    source: &str,
    git_ref: Option<&str>,
    target: &Path,
    cancelled: &AtomicBool,
) -> Result<()> {
    let mut args = vec![
        "clone".to_string(),
        "--depth".to_string(),
        "1".to_string(),
        "--single-branch".to_string(),
    ];
    if let Some(git_ref) = git_ref {
        args.extend(["--branch".to_string(), git_ref.to_string()]);
    }
    args.extend([source.to_string(), target.display().to_string()]);
    let cwd = target.parent().unwrap_or_else(|| Path::new("."));
    let output = git::run_git(cwd, args, git::NETWORK_COMMAND_TIMEOUT, cancelled)
        .with_context(|| format!("failed to run git clone for {source}"))?;
    if !output.status.success() {
        bail!(
            "git clone failed for {source}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn discover_installable_skills(root: &Path) -> Result<Vec<InstallableSkill>> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", root.display()))?;
    let mut skills = Vec::new();
    let mut seen = BTreeSet::new();

    if root.join("SKILL.md").is_file() {
        if let Some(candidate) = read_installable_skill(&root, &root)? {
            return Ok(vec![candidate.skill]);
        }
    }

    let mut search_roots = vec![
        root.join("skills"),
        root.join("skills/.curated"),
        root.join("skills/.experimental"),
        root.join("skills/.system"),
    ];
    let provider_context = crate::providers::ProviderContext::new(&root);
    search_roots.extend(
        crate::providers::all_providers()
            .into_iter()
            .flat_map(|provider| provider.skill_roots(&provider_context))
            .filter(|skill_root| skill_root.scope == "project")
            .map(|skill_root| skill_root.path),
    );
    search_roots.retain(|path| path.is_dir());

    for search_root in search_roots {
        for entry in WalkDir::new(&search_root)
            .follow_links(false)
            .max_depth(3)
            .into_iter()
            .filter_entry(|entry| !is_skipped_skill_search_entry(entry.path()))
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file() && entry.file_name() == "SKILL.md")
        {
            let Some(skill_dir) = entry.path().parent() else {
                continue;
            };
            if let Some(candidate) = read_installable_skill(&root, skill_dir)? {
                if seen.insert(normalize_skill_match_name(&candidate.skill.name)) {
                    skills.push(candidate);
                }
            }
        }
    }

    if skills.is_empty() {
        let mut walker = WalkBuilder::new(&root);
        walker
            .hidden(false)
            .ignore(false)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(false)
            .follow_links(false)
            .max_depth(Some(5))
            .filter_entry(|entry| !is_skipped_skill_search_entry(entry.path()));
        for entry in walker
            .build()
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_file())
                    && entry.file_name() == "SKILL.md"
            })
        {
            let Some(skill_dir) = entry.path().parent() else {
                continue;
            };
            if let Some(candidate) = read_installable_skill(&root, skill_dir)? {
                if seen.insert(normalize_skill_match_name(&candidate.skill.name)) {
                    skills.push(candidate);
                }
            }
        }
    }

    resolve_installable_dependencies(&mut skills);
    skills.sort_by(|left, right| left.skill.name.cmp(&right.skill.name));
    Ok(skills
        .into_iter()
        .map(|candidate| candidate.skill)
        .collect())
}

fn is_skipped_skill_search_entry(path: &Path) -> bool {
    path.components().any(|part| {
        part.as_os_str().to_str().is_some_and(|value| {
            matches!(
                value,
                ".git" | "node_modules" | "dist" | "build" | "__pycache__"
            )
        })
    })
}

fn read_installable_skill(
    root: &Path,
    skill_dir: &Path,
) -> Result<Option<InstallableSkillCandidate>> {
    let skill_file = skill_dir.join("SKILL.md");
    let text = fs::read_to_string(&skill_file)
        .with_context(|| format!("failed to read {}", skill_file.display()))?;
    let frontmatter = parse_frontmatter(&text);
    let name = frontmatter
        .as_ref()
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            skill_dir
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        });
    let Some(name) = name else {
        return Ok(None);
    };
    let description = frontmatter
        .as_ref()
        .and_then(|value| value.get("description"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let relative_path = skill_dir
        .strip_prefix(root)
        .unwrap_or(skill_dir)
        .components()
        .filter_map(|part| part.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/");
    let dependencies = parse_declared_skill_dependencies(frontmatter.as_ref());
    let dependency_files = parse_skill_file_references(&text)
        .into_iter()
        .filter_map(|path| resolve_skill_file_reference(&skill_file, &path))
        .collect();
    Ok(Some(InstallableSkillCandidate {
        skill: InstallableSkill {
            name,
            description,
            path: skill_dir.to_path_buf(),
            relative_path,
            dependencies,
        },
        dependency_files,
    }))
}

fn resolve_installable_dependencies(skills: &mut [InstallableSkillCandidate]) {
    let by_normalized_name = skills
        .iter()
        .map(|candidate| {
            (
                normalize_skill_match_name(&candidate.skill.name),
                candidate.skill.name.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let by_file = skills
        .iter()
        .map(|candidate| {
            (
                skill_file_key(&candidate.skill.path.join("SKILL.md")),
                candidate.skill.name.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    for candidate in skills {
        let own_name = normalize_skill_match_name(&candidate.skill.name);
        let path_dependencies = candidate
            .dependency_files
            .iter()
            .filter_map(|path| by_file.get(&skill_file_key(path)).cloned())
            .collect::<Vec<_>>();
        candidate.skill.dependencies = candidate
            .skill
            .dependencies
            .iter()
            .cloned()
            .chain(path_dependencies)
            .filter_map(|dependency| {
                let dependency_name =
                    by_normalized_name.get(&normalize_skill_match_name(&dependency))?;
                (normalize_skill_match_name(dependency_name) != own_name)
                    .then(|| dependency_name.clone())
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
    }
}
pub fn check_skill_updates(cwd: &Path) -> Result<Vec<SkillUpdateReport>> {
    let scan = scan_skills(cwd)?;
    Ok(check_skill_updates_for_scan(&scan))
}

pub fn check_skill_updates_for_scan(scan: &SkillScan) -> Vec<SkillUpdateReport> {
    check_skill_updates_for_scan_with_cancel(scan, git::never_cancelled())
}

pub fn check_skill_updates_for_scan_with_cancel(
    scan: &SkillScan,
    cancelled: &AtomicBool,
) -> Vec<SkillUpdateReport> {
    let skills = scan.skills.iter().collect::<Vec<_>>();
    check_skill_updates_for_skills(&skills, cancelled)
}

pub fn plan_skill_updates(cwd: &Path, pattern: &str) -> Result<SkillUpdatePlan> {
    let scan = scan_skills(cwd)?;
    let store = crate::storage::Store::open_default()?;
    let matches = matching_skills(&scan, pattern);
    if matches.is_empty() {
        bail!("no skills matched pattern {pattern:?}");
    }

    let reports = check_skill_updates_for_skills(&matches, git::never_cancelled());
    plan_skill_updates_for_matches(&scan, matches, &store, Some(cwd), &reports)
}

pub fn plan_skill_updates_many_for_scan_in_workspace_with_store(
    scan: &SkillScan,
    skill_ids: &[String],
    workspace_root: &Path,
    store: &crate::storage::Store,
) -> Result<SkillUpdatePlan> {
    let matches = scan
        .skills
        .iter()
        .filter(|skill| skill_ids.iter().any(|id| skill_matches_id(skill, id)))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        bail!("no skills matched update selection");
    }

    let reports = check_skill_updates_for_skills(&matches, git::never_cancelled());
    plan_skill_updates_for_matches(scan, matches, store, Some(workspace_root), &reports)
}

pub fn plan_skill_updates_many_for_scan_in_workspace_with_store_and_reports(
    scan: &SkillScan,
    skill_ids: &[String],
    workspace_root: &Path,
    store: &crate::storage::Store,
    reports: &[SkillUpdateReport],
) -> Result<SkillUpdatePlan> {
    let matches = scan
        .skills
        .iter()
        .filter(|skill| skill_ids.iter().any(|id| skill_matches_id(skill, id)))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        bail!("no skills matched update selection");
    }

    plan_skill_updates_for_matches(scan, matches, store, Some(workspace_root), reports)
}

pub fn format_update_plan(plan: &SkillUpdatePlan) -> String {
    let mut lines = Vec::new();

    for change in &plan.file_changes.changes {
        lines.push(format!("Update file {}", change.path.display()));
    }
    for action in &plan.git_updates {
        lines.push(format!("Merge {} from {}", action.name, action.source));
    }
    for issue in &plan.merge_issues {
        lines.push(format!("Block {} ({})", issue.path.display(), issue.status));
    }
    for skipped in &plan.skipped {
        lines.push(format!("Skip {} ({})", skipped.name, skipped.status));
    }

    if lines.is_empty() {
        "No applicable updates.".to_string()
    } else {
        lines.join("\n")
    }
}

fn plan_skill_updates_for_matches(
    scan: &SkillScan,
    matches: Vec<&SkillRecord>,
    store: &crate::storage::Store,
    workspace_root: Option<&Path>,
    reports: &[SkillUpdateReport],
) -> Result<SkillUpdatePlan> {
    let mut file_changes = Vec::new();
    let mut git_updates = BTreeMap::new();
    let mut skipped = Vec::new();
    let mut source_updates = Vec::new();
    let mut merge_issues = Vec::new();
    let mut updates_by_id = reports
        .iter()
        .cloned()
        .into_iter()
        .map(|update| (update.id.clone(), update))
        .collect::<BTreeMap<_, _>>();

    for skill in matches {
        let update = updates_by_id
            .remove(&ensure_skill_record_id(skill))
            .context("skill update report missing for selected skill")?;
        if update.status != "update-available" {
            skipped.push(update);
            continue;
        }

        let Some(path) = select_update_path(skill) else {
            skipped.push(update);
            continue;
        };
        let Some(source_version) = update.latest_version.clone() else {
            skipped.push(update);
            continue;
        };

        match path.source_kind.as_str() {
            "git" | "github" | "gitlab" | "huggingface" => {
                if let Some(action) =
                    plan_git_update(scan, skill, path, &update, store, workspace_root)?
                {
                    if !action.has_effective_changes() {
                        skipped.push(SkillUpdateReport {
                            status: "up-to-date".to_string(),
                            ..update
                        });
                        continue;
                    }
                    source_updates.push(SkillSourceUpdate {
                        skill_path: path.path.clone(),
                        source_version: source_version.clone(),
                    });
                    git_updates
                        .entry(action.repo.clone())
                        .and_modify(|existing: &mut GitUpdateAction| {
                            if !action.diff.is_empty() {
                                existing.diff.push_str(&action.diff);
                            }
                            existing.files.extend(action.files.clone());
                            existing
                                .materialized_targets
                                .extend(action.materialized_targets.clone());
                            existing.skill_names.extend(action.skill_names.clone());
                        })
                        .or_insert(action);
                } else {
                    skipped.push(update);
                }
            }
            "registry" => {
                match plan_registry_update(skill, path, store, workspace_root, &source_version)? {
                    Some(RegistryUpdatePlan::Change(change, source_update)) => {
                        file_changes.push(change);
                        source_updates.push(source_update);
                    }
                    None => skipped.push(update),
                    Some(RegistryUpdatePlan::Issue(issue)) => {
                        source_updates.push(SkillSourceUpdate {
                            skill_path: path.path.clone(),
                            source_version,
                        });
                        merge_issues.push(issue);
                    }
                }
            }
            _ => skipped.push(update),
        }
    }

    Ok(SkillUpdatePlan {
        file_changes: ChangeSet {
            changes: dedupe_changes(file_changes),
        },
        git_updates: git_updates.into_values().collect(),
        skipped,
        source_updates,
        merge_issues,
    })
}

enum RegistryUpdatePlan {
    Change(FileChange, SkillSourceUpdate),
    Issue(SkillMergeIssue),
}

#[cfg(test)]
pub fn apply_skill_update_plan_with_store(
    plan: &SkillUpdatePlan,
    store: &crate::storage::Store,
) -> Result<()> {
    let plan = prepare_skill_update_plan_with_resolutions(plan, &BTreeMap::new())?;
    apply_prepared_skill_update_plan(store, None, &plan)
}

/// Filesystem serialization belongs to the affected skills, not the database.
/// Persistence performs its version check and commit in one short transaction.
pub fn apply_prepared_skill_update_plan_for_workspace(
    store: &crate::storage::Store,
    workspace_root: &Path,
    plan: &SkillUpdatePlan,
) -> Result<()> {
    apply_prepared_skill_update_plan(store, Some(workspace_root), plan)
}

pub fn skill_update_resource_paths(plan: &SkillUpdatePlan) -> Result<Vec<PathBuf>> {
    let mut paths = plan
        .file_changes
        .changes
        .iter()
        .map(|change| change.path.clone())
        .collect::<Vec<_>>();
    paths.extend(
        skill_source_updates_for_plan(plan)
            .into_iter()
            .map(|update| update.skill_path),
    );
    for action in &plan.git_updates {
        paths.push(action.repo.clone());
        paths.extend(git::mutation_resource_paths(&action.repo)?);
        paths.extend(
            action
                .materialized_targets
                .iter()
                .map(|target| target.target.clone()),
        );
        paths.extend(
            action
                .tendi_settings
                .iter()
                .map(|setting| setting.skill_dir.clone()),
        );
        for target in &action.materialized_targets {
            paths.extend(skill_visibility_resource_paths(
                &target.target,
                target.agent,
                target.uses_shared_layout,
            ));
        }
    }
    Ok(paths)
}

fn apply_prepared_skill_update_plan(
    store: &crate::storage::Store,
    workspace_root: Option<&Path>,
    plan: &SkillUpdatePlan,
) -> Result<()> {
    let _resources =
        crate::coordination::acquire_file_resources(&skill_update_resource_paths(plan)?)?;
    let prepare = || match workspace_root {
        Some(workspace) => prepare_skill_update_persistence_for_workspace(store, workspace, plan),
        None => prepare_skill_update_persistence(store, plan),
    };
    let expected_source_versions = prepare()?.expected_source_versions;
    match workspace_root {
        Some(workspace) => store
            .validate_skill_source_versions_for_workspace(workspace, &expected_source_versions)?,
        None => store.validate_skill_source_versions(&expected_source_versions)?,
    }
    let filesystem = apply_skill_update_plan_filesystem_transaction(plan)?;
    let result = (|| {
        let persistence = prepare()?;
        match workspace_root {
            Some(workspace) => store.persist_skill_update_persistence_for_workspace_checked(
                workspace,
                &expected_source_versions,
                &persistence.source_records,
                &persistence.snapshots,
            ),
            None => store.persist_skill_update_persistence_checked(
                &expected_source_versions,
                &persistence.source_records,
                &persistence.snapshots,
            ),
        }
    })();
    match result {
        Ok(()) => {
            filesystem.commit();
            Ok(())
        }
        Err(error) => Err(error.context(filesystem.rollback_context()?)),
    }
}

pub fn prepare_skill_update_plan_with_resolutions(
    plan: &SkillUpdatePlan,
    resolutions: &BTreeMap<String, String>,
) -> Result<SkillUpdatePlan> {
    let mut plan = plan.clone();
    apply_merge_resolutions(&mut plan, resolutions)?;
    if plan_has_merge_blockers(&plan) {
        bail!("skill update has unresolved merge conflicts");
    }
    Ok(plan)
}

pub fn apply_skill_update_plan_filesystem(plan: &SkillUpdatePlan) -> Result<()> {
    let filesystem = apply_skill_update_plan_filesystem_transaction(plan)?;
    filesystem.commit();
    Ok(())
}

pub fn apply_skill_update_plan_filesystem_transaction(
    plan: &SkillUpdatePlan,
) -> Result<SkillFilesystemTransaction> {
    let resources =
        crate::coordination::acquire_file_resources(&skill_update_resource_paths(plan)?)?;
    let mut filesystem = SkillFilesystemTransaction::capture(plan)?;
    filesystem.resources = Some(resources);
    if let Err(error) = apply_skill_update_plan_filesystem_unchecked(plan) {
        let _ = filesystem.rollback_context();
        return Err(error);
    }
    Ok(filesystem)
}

fn apply_skill_update_plan_filesystem_unchecked(plan: &SkillUpdatePlan) -> Result<()> {
    apply_changes(&plan.file_changes)?;
    for action in &plan.git_updates {
        apply_git_update_with_store(action, None)?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct SkillFilesystemTransaction {
    // Retained until explicit commit/rollback so another operation cannot observe
    // or overwrite a filesystem change whose database commit is still pending.
    resources: Option<crate::coordination::ResourceLease>,
    backups: Vec<(PathBuf, Option<FilesystemBackup>)>,
    git_heads: Vec<(PathBuf, String)>,
}

#[derive(Debug)]
enum FilesystemBackup {
    File(Vec<u8>),
    Directory(Vec<(PathBuf, FilesystemBackup)>),
    Symlink(PathBuf),
}

impl SkillFilesystemTransaction {
    fn capture(plan: &SkillUpdatePlan) -> Result<Self> {
        let mut paths = Vec::new();
        paths.extend(
            plan.file_changes
                .changes
                .iter()
                .map(|change| change.path.clone()),
        );
        let mut git_repos = BTreeSet::new();
        for action in &plan.git_updates {
            git_repos.insert(action.repo.clone());
            if action.materialized_targets.is_empty() {
                paths.extend(action.files.iter().map(|file| action.repo.join(&file.path)));
            } else {
                paths.extend(
                    action
                        .materialized_targets
                        .iter()
                        .map(|target| target.target.clone()),
                );
            }
            paths.extend(action.tendi_settings.iter().flat_map(|setting| {
                [
                    setting.skill_dir.join("SKILL.md"),
                    setting.skill_dir.join("agents/openai.yaml"),
                ]
            }));
            for target in &action.materialized_targets {
                paths.extend(skill_visibility_resource_paths(
                    &target.target,
                    target.agent,
                    target.uses_shared_layout,
                ));
            }
        }
        let paths = dedupe_backup_paths(paths);
        let backups = paths
            .into_iter()
            .map(|path| Ok((path.clone(), capture_filesystem_backup(&path)?)))
            .collect::<Result<Vec<_>>>()?;
        let git_heads = git_repos
            .into_iter()
            .filter_map(|repo| git_output(&repo, &["rev-parse", "HEAD"]).map(|head| (repo, head)))
            .collect();
        Ok(Self {
            resources: None,
            backups,
            git_heads,
        })
    }

    pub fn commit(self) {}

    pub fn rollback_context(self) -> Result<String> {
        let mut failures = Vec::new();
        for (repo, head) in self.git_heads.iter().rev() {
            if let Err(error) = run_git(repo, &["reset", "--hard", head]) {
                failures.push(format!("{}: {error:#}", repo.display()));
            }
        }
        for (path, backup) in self.backups.iter().rev() {
            if let Err(error) = restore_filesystem_backup(path, backup.as_ref()) {
                failures.push(format!("{}: {error:#}", path.display()));
            }
        }
        if failures.is_empty() {
            Ok("filesystem transaction rolled back".to_string())
        } else {
            Err(anyhow::anyhow!(
                "filesystem rollback failed: {}",
                failures.join("; ")
            ))
        }
    }
}

fn dedupe_backup_paths(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });
    paths.dedup();
    let mut retained = Vec::new();
    for path in paths {
        if retained
            .iter()
            .any(|parent: &PathBuf| path.starts_with(parent))
        {
            continue;
        }
        retained.push(path);
    }
    retained
}

fn capture_filesystem_backup(path: &Path) -> Result<Option<FilesystemBackup>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.is_file() {
        return Ok(Some(FilesystemBackup::File(fs::read(path)?)));
    }
    if metadata.is_dir() {
        let mut entries = fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort();
        let children = entries
            .into_iter()
            .map(|child| {
                let relative = child
                    .strip_prefix(path)
                    .map(Path::to_path_buf)
                    .expect("read directory child is inside parent");
                Ok((
                    relative,
                    capture_filesystem_backup(&child)?.expect("child exists"),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(Some(FilesystemBackup::Directory(children)));
    }
    if metadata.file_type().is_symlink() {
        return Ok(Some(FilesystemBackup::Symlink(fs::read_link(path)?)));
    }
    bail!("unsupported filesystem entry {}", path.display())
}

fn restore_filesystem_backup(path: &Path, backup: Option<&FilesystemBackup>) -> Result<()> {
    remove_filesystem_entry(path)?;
    let Some(backup) = backup else {
        return Ok(());
    };
    match backup {
        FilesystemBackup::File(bytes) => atomic_write_bytes(path, bytes)?,
        FilesystemBackup::Directory(children) => {
            fs::create_dir_all(path)?;
            for (relative, child) in children {
                restore_filesystem_backup(&path.join(relative), Some(child))?;
            }
        }
        FilesystemBackup::Symlink(target) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            create_symlink(target, path)?;
        }
    }
    Ok(())
}

fn remove_filesystem_entry(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn rename_skill_directory(source: &Path, target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if !metadata.is_dir() {
        fs::rename(source, target)?;
        return Ok(());
    }

    let original_permissions = metadata.permissions();
    let mut rename_permissions = original_permissions.clone();
    let permissions_changed = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let original_mode = rename_permissions.mode();
            rename_permissions.set_mode(rename_permissions.mode() | 0o700);
            original_mode != rename_permissions.mode()
        }
        #[cfg(not(unix))]
        {
            let changed = original_permissions.readonly();
            rename_permissions.set_readonly(false);
            changed
        }
    };
    if permissions_changed {
        fs::set_permissions(source, rename_permissions)?;
    }

    if let Err(error) = fs::rename(source, target) {
        let restore = if permissions_changed {
            fs::set_permissions(source, original_permissions)
        } else {
            Ok(())
        };
        return match restore {
            Ok(()) => Err(error.into()),
            Err(restore_error) => Err(anyhow::anyhow!(
                "{error}; failed to restore permissions on {}: {restore_error}",
                source.display()
            )),
        };
    }

    if permissions_changed {
        if let Err(error) = fs::set_permissions(target, original_permissions.clone()) {
            if let Err(rollback_error) = fs::rename(target, source) {
                return Err(anyhow::anyhow!(
                    "failed to restore directory permissions after rename: {error}; ".to_owned()
                        + &format!("failed to move {} back: {rollback_error}", source.display())
                ));
            }
            if let Err(restore_error) = fs::set_permissions(source, original_permissions) {
                return Err(anyhow::anyhow!(
                    "failed to restore directory permissions after rename: {error}; ".to_owned()
                        + &format!("failed to restore {}: {restore_error}", source.display())
                ));
            }
            return Err(anyhow::anyhow!(
                "failed to restore directory permissions after rename: {error}"
            ));
        }
    }
    Ok(())
}

pub fn persist_skill_update_plan(
    store: &crate::storage::Store,
    plan: &SkillUpdatePlan,
) -> Result<()> {
    let persistence = prepare_skill_update_persistence(store, plan)?;
    persist_skill_update_persistence(store, &persistence)
}

pub fn prepare_skill_update_persistence(
    store: &crate::storage::Store,
    plan: &SkillUpdatePlan,
) -> Result<SkillUpdatePersistence> {
    let source_updates = skill_source_updates_for_plan(plan);
    let records = store.skill_source_records()?;
    let expected_source_versions = expected_source_versions(&records, &source_updates);
    let source_records = updated_skill_source_records_for_records(&records, &source_updates)?;
    let snapshots = capture_skill_snapshots(&source_records)?;
    Ok(SkillUpdatePersistence {
        source_records,
        snapshots,
        expected_source_versions,
    })
}

pub fn prepare_skill_update_persistence_for_workspace(
    store: &crate::storage::Store,
    workspace_root: &Path,
    plan: &SkillUpdatePlan,
) -> Result<SkillUpdatePersistence> {
    let source_updates = skill_source_updates_for_plan(plan);
    let records = store.skill_source_records_for_workspace(workspace_root)?;
    let expected_source_versions = expected_source_versions(&records, &source_updates);
    let source_records = updated_skill_source_records_for_records(&records, &source_updates)?;
    let snapshots = capture_skill_snapshots(&source_records)?;
    Ok(SkillUpdatePersistence {
        source_records,
        snapshots,
        expected_source_versions,
    })
}

pub fn persist_skill_update_persistence(
    store: &crate::storage::Store,
    persistence: &SkillUpdatePersistence,
) -> Result<()> {
    store.persist_skill_update_persistence(&persistence.source_records, &persistence.snapshots)
}

fn skill_source_updates_for_plan(plan: &SkillUpdatePlan) -> Vec<SkillSourceUpdate> {
    let mut source_updates = plan.source_updates.clone();
    for action in &plan.git_updates {
        let Some(source_version) = action.latest_version.clone() else {
            continue;
        };
        source_updates.extend(
            action
                .materialized_targets
                .iter()
                .map(|target| SkillSourceUpdate {
                    skill_path: target.target.clone(),
                    source_version: source_version.clone(),
                }),
        );
    }
    source_updates
}

fn updated_skill_source_records_for_records(
    records: &[SkillSourceRecord],
    updates: &[SkillSourceUpdate],
) -> Result<Vec<SkillSourceRecord>> {
    if updates.is_empty() {
        return Ok(Vec::new());
    }
    let update_by_path = updates
        .iter()
        .filter(|update| !update.source_version.is_empty())
        .map(|update| (update.skill_path.clone(), update.source_version.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut records = records
        .into_iter()
        .cloned()
        .filter_map(|mut record| {
            let version = update_by_path.get(&record.skill_path)?.clone();
            record.source_version = Some(version);
            record.update_status = "tracked".to_string();
            Some(record)
        })
        .collect::<Vec<_>>();
    records.sort_by(|left, right| left.skill_path.cmp(&right.skill_path));
    Ok(records)
}

fn expected_source_versions(
    records: &[SkillSourceRecord],
    updates: &[SkillSourceUpdate],
) -> Vec<(PathBuf, Option<String>)> {
    updates
        .iter()
        .filter_map(|update| {
            records
                .iter()
                .find(|record| record.skill_path == update.skill_path)
                .map(|record| (record.skill_path.clone(), record.source_version.clone()))
        })
        .collect()
}

fn apply_merge_resolutions(
    plan: &mut SkillUpdatePlan,
    resolutions: &BTreeMap<String, String>,
) -> Result<()> {
    let mut resolved_issues = Vec::new();
    for issue in &plan.merge_issues {
        let Some(content) = resolutions.get(&issue.resolution_key) else {
            continue;
        };
        resolved_issues.push(FileChange {
            path: issue.path.clone(),
            before_sha256: Some(sha256_text(&issue.before)),
            before: Some(issue.before.clone()),
            after: match content.as_str() {
                KEEP_LOCAL_RESOLUTION => issue.before.clone(),
                USE_UPDATE_RESOLUTION => issue.incoming.clone(),
                _ => content.clone(),
            },
        });
    }
    plan.merge_issues
        .retain(|issue| !resolutions.contains_key(&issue.resolution_key));
    plan.file_changes.changes.extend(resolved_issues);

    for action in &mut plan.git_updates {
        for file in &mut action.files {
            resolve_update_file(file, resolutions);
        }
        for target in &mut action.materialized_targets {
            for file in &mut target.files {
                resolve_update_file(file, resolutions);
            }
        }
    }
    Ok(())
}

fn resolve_update_file(file: &mut GitUpdateFile, resolutions: &BTreeMap<String, String>) {
    let Some(content) = resolutions.get(&file.resolution_key) else {
        return;
    };
    match content.as_str() {
        KEEP_LOCAL_RESOLUTION => {
            file.after = String::new();
            file.after_bytes = file.before_bytes.clone();
            file.after_exists = file.before_exists;
            file.status = "resolved-local".to_string();
            return;
        }
        USE_UPDATE_RESOLUTION => {
            file.after = String::new();
            file.after_bytes = file.incoming_bytes.clone();
            file.after_exists = file.incoming_exists;
            file.status = "resolved-remote".to_string();
            return;
        }
        _ => {}
    }
    file.after = content.clone();
    file.after_bytes = None;
    file.after_exists = if content == &file.before {
        file.before_exists
    } else if content == &file.incoming {
        file.incoming_exists
    } else {
        true
    };
    file.status = "resolved".to_string();
}

fn plan_has_merge_blockers(plan: &SkillUpdatePlan) -> bool {
    if !plan.merge_issues.is_empty() {
        return true;
    }
    plan.git_updates.iter().any(|action| {
        action.files.iter().any(is_merge_blocker)
            || action
                .materialized_targets
                .iter()
                .any(|target| target.files.iter().any(is_merge_blocker))
    })
}

fn is_merge_blocker(file: &GitUpdateFile) -> bool {
    matches!(file.status.as_str(), "conflict" | "unavailable" | "binary")
}

fn plan_wrapper_for_matches(
    scan: &SkillScan,
    name: &str,
    matches: Vec<&SkillRecord>,
    description: Option<&str>,
    manual_children: bool,
) -> Result<ChangeSet> {
    if matches.is_empty() {
        bail!("no skills matched wrapper selection");
    }

    let target_root = scan
        .roots
        .iter()
        .find(|root| root.agent == AgentKind::Shared && root.scope == "global")
        .map(|root| root.path.clone())
        .or_else(|| {
            dirs::home_dir().and_then(|home| {
                crate::providers::agent_provider(AgentKind::Shared).global_skill_root(&home)
            })
        })
        .context("could not resolve wrapper target root")?;

    let wrapper_dir = target_root.join(name);
    let wrapper_file = wrapper_dir.join("SKILL.md");
    let before = read_optional(&wrapper_file)?;
    let after =
        render_wrapper_after_with_description(name, &matches, before.as_deref(), description);
    let before_sha256 = before.as_ref().map(|text| sha256_text(text));

    let mut changes = vec![FileChange {
        path: wrapper_file,
        before_sha256,
        before,
        after,
    }];

    if manual_children {
        for skill in matches {
            if skill.name == name {
                continue;
            }
            for path in &skill.paths {
                changes.extend(plan_skill_visibility_at_path(
                    &path.path,
                    path.agent,
                    SkillVisibility::Manual,
                    true,
                )?);
            }
        }
    }

    Ok(ChangeSet {
        changes: dedupe_changes(changes),
    })
}

fn plan_wrapper_sync(scan: &SkillScan) -> Result<ChangeSet> {
    plan_wrapper_sync_for_ids(scan, None)
}

fn plan_wrapper_sync_for_ids(
    scan: &SkillScan,
    selected: Option<&BTreeSet<String>>,
) -> Result<ChangeSet> {
    let mut changes = Vec::new();

    for wrapper in &scan.skills {
        if selected.is_some_and(|selected| !selected.contains(&wrapper.id)) {
            continue;
        }
        if !wrapper.is_wrapper {
            continue;
        }
        let wrapper_name = wrapper.name.clone();
        for path in &wrapper.paths {
            let wrapper_file = path.path.join("SKILL.md");
            let Some(before) = read_optional(&wrapper_file)? else {
                continue;
            };
            let routes = parse_wrapper_routes(&before);
            if routes.is_empty() {
                continue;
            }
            let children = routes
                .iter()
                .filter_map(|route| {
                    let exact_match = route.path.as_ref().and_then(|path| {
                        let path = if path.is_absolute() {
                            path.clone()
                        } else {
                            wrapper_file
                                .parent()
                                .unwrap_or_else(|| Path::new("."))
                                .join(path)
                        };
                        path.parent().and_then(|skill_dir| {
                            let canonical = canonical_skill_dir(skill_dir);
                            scan.skills.iter().find(|skill| {
                                skill.name == route.name
                                    && skill.paths.iter().any(|candidate| {
                                        canonical_skill_dir(&candidate.path) == canonical
                                    })
                            })
                        })
                    });
                    exact_match.or_else(|| unique_skill_named(&scan.skills, &route.name))
                })
                .filter(|skill| skill.name != wrapper_name)
                .collect::<Vec<_>>();
            let after = render_wrapper_after(&wrapper_name, &children, Some(&before));
            if after == before {
                continue;
            }
            changes.push(FileChange {
                path: wrapper_file,
                before_sha256: Some(sha256_text(&before)),
                before: Some(before),
                after,
            });
        }
    }

    Ok(ChangeSet {
        changes: dedupe_changes(changes),
    })
}

fn discover_roots_for_projects(cwd: &Path, project_roots: &[PathBuf]) -> Vec<SkillRoot> {
    #[cfg(test)]
    crate::test_support::ensure_isolated_environment();

    let mut roots = Vec::new();
    let ctx = crate::providers::ProviderContext::with_additional_project_dirs(cwd, project_roots);
    for provider in crate::providers::all_providers() {
        for root in provider.skill_roots(&ctx) {
            push_root(
                &mut roots,
                root.path,
                root.scope,
                root.agent,
                root.plugin_id,
                root.plugin_enabled,
            );
        }
    }

    roots
}

pub(crate) fn global_agent_skill_root(agent: AgentKind) -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not resolve home directory")?;
    crate::providers::agent_provider(agent)
        .global_skill_root(&home)
        .ok_or_else(|| anyhow::anyhow!("unknown agent target"))
}

#[cfg(unix)]
fn create_symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(not(unix))]
fn create_symlink(_source: &Path, _target: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

fn copy_dir(source: &Path, target: &Path) -> Result<()> {
    for entry in WalkDir::new(source).follow_links(false).into_iter() {
        let entry = entry?;
        let relative = entry.path().strip_prefix(source)?;
        let destination = target.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&destination)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &destination)?;
        }
    }
    Ok(())
}

fn sanitize_skill_dir_name(name: &str) -> Result<String> {
    let sanitized = name
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .collect::<Vec<_>>()
        .join("-");
    let sanitized = sanitized
        .trim_matches(['.', '-'])
        .chars()
        .take(255)
        .collect::<String>();
    if sanitized.is_empty() {
        bail!("skill name must contain at least one usable character");
    }
    Ok(sanitized)
}

fn ensure_path_inside(base: &Path, target: &Path) -> Result<()> {
    let base = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    let target = target
        .parent()
        .map(|parent| {
            parent
                .canonicalize()
                .unwrap_or_else(|_| parent.to_path_buf())
        })
        .unwrap_or_else(|| target.to_path_buf())
        .join(target.file_name().unwrap_or_default());
    if target == base || target.starts_with(&base) {
        Ok(())
    } else {
        bail!("refusing to write outside {}", base.display())
    }
}

fn short_sha(value: &str, len: usize) -> String {
    sha256_text(value).chars().take(len).collect()
}

fn push_root(
    roots: &mut Vec<SkillRoot>,
    path: PathBuf,
    scope: String,
    agent: AgentKind,
    plugin_id: Option<String>,
    plugin_enabled: Option<bool>,
) {
    if path.is_dir() && !roots.iter().any(|root| root.path == path) {
        roots.push(SkillRoot {
            path,
            scope,
            agent,
            plugin_id,
            plugin_enabled,
        });
    }
}

fn find_skill_files(root: &Path) -> Vec<PathBuf> {
    WalkDir::new(root)
        .follow_links(true)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "SKILL.md")
        .map(|entry| entry.into_path())
        .collect()
}

fn read_skill(
    root: &SkillRoot,
    skill_file: &Path,
    provenance_resolver: &mut ProvenanceResolver,
) -> Result<RawSkill> {
    let text = fs::read_to_string(skill_file)
        .with_context(|| format!("failed to read {}", skill_file.display()))?;
    let frontmatter = parse_frontmatter(&text);
    let skill_dir = skill_file
        .parent()
        .context("SKILL.md did not have a parent directory")?;

    let name = frontmatter
        .as_ref()
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            skill_dir
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .context("skill name was missing")?;

    let description = frontmatter
        .as_ref()
        .and_then(|value| value.get("description"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let tags = frontmatter
        .as_ref()
        .map(parse_frontmatter_tags)
        .unwrap_or_default();
    let dependencies = parse_declared_skill_dependencies(frontmatter.as_ref());
    let dependency_files = parse_skill_file_references(&text)
        .into_iter()
        .filter_map(|path| resolve_skill_file_reference(skill_file, &path))
        .collect();
    let is_wrapper = !parse_wrapper_routes(&text).is_empty();

    let tendi_visibility = None;
    let mut effective_visibility = SkillVisibility::Auto;
    let mut provider_allow_implicit_invocation = None;
    let mut provider_skill_enabled = None;
    let mut provider_disable_model_invocation = None;
    for provider in crate::providers::skill_visibility_providers(root.agent) {
        let metadata =
            provider.skill_visibility_metadata(skill_dir, skill_file, frontmatter.as_ref())?;
        let provider_visibility = provider.effective_skill_visibility(
            tendi_visibility,
            metadata.provider_visibility,
            root,
        );
        effective_visibility =
            preferred_skill_visibility(effective_visibility, provider_visibility);
        provider_allow_implicit_invocation =
            provider_allow_implicit_invocation.or(metadata.allow_implicit_invocation);
        provider_skill_enabled = match (provider_skill_enabled, metadata.enabled) {
            (_, Some(false)) => Some(false),
            (None, Some(true)) => Some(true),
            (current, None | Some(true)) => current,
        };
        provider_disable_model_invocation = match (
            provider_disable_model_invocation,
            metadata.disable_model_invocation,
        ) {
            (_, Some(true)) => Some(true),
            (None, Some(false)) => Some(false),
            (current, None) | (current, Some(false)) => current,
        };
    }
    let provenance_dir = skill_dir
        .canonicalize()
        .unwrap_or_else(|_| skill_dir.to_path_buf());
    let provenance = provenance_resolver.infer_installed(
        skill_dir,
        &provenance_dir,
        &root.path,
        &root.scope,
        &name,
        &frontmatter,
    );
    let symlink_status = symlink_status(&root.path, skill_dir);

    let sha256 = sha256_text(&text);

    Ok(RawSkill {
        name,
        description,
        tags: tags.clone(),
        dependencies,
        dependency_files,
        is_wrapper,
        is_system: skill_dir.components().any(|part| {
            part.as_os_str()
                .to_str()
                .is_some_and(|value| value == ".system" || value == "cache")
        }),
        path: SkillPath {
            path: skill_dir.to_path_buf(),
            root: root.path.clone(),
            scope: root.scope.clone(),
            agent: root.agent,
            install_target: install_target(root.agent, &root.path),
            source_kind: provenance.kind,
            source: provenance.source,
            source_ref: provenance.source_ref,
            source_version: provenance.version,
            source_relative_path: provenance.relative_path,
            symlink_status,
            update_status: provenance.update_status,
            sha256,
            tags,
            tendi_visibility,
            effective_visibility,
            provider_allow_implicit_invocation,
            provider_skill_enabled,
            provider_disable_model_invocation,
            plugin_id: root.plugin_id.clone(),
            plugin_enabled: root.plugin_enabled,
        },
    })
}

pub(crate) fn parse_frontmatter(text: &str) -> Option<Value> {
    let (yaml, _, _) = split_frontmatter_raw(text)?;
    serde_yaml::from_str(yaml)
        .ok()
        .or_else(|| parse_frontmatter_lenient(yaml))
}

fn parse_declared_skill_dependencies(frontmatter: Option<&Value>) -> Vec<String> {
    let mut dependencies = BTreeSet::new();
    if let Some(frontmatter) = frontmatter {
        for key in [
            "dependencies",
            "depends_on",
            "depends-on",
            "requires",
            "skill_dependencies",
            "skill-dependencies",
        ] {
            if let Some(value) = frontmatter.get(key) {
                collect_dependency_value(value, &mut dependencies);
            }
        }
        if let Some(siblings) = frontmatter
            .get("metadata")
            .and_then(|metadata| metadata.get("requires"))
            .and_then(|requires| requires.get("siblings"))
        {
            collect_dependency_value(siblings, &mut dependencies);
        }
    }

    dependencies.into_iter().collect()
}

fn parse_skill_file_references(text: &str) -> Vec<PathBuf> {
    let body = split_frontmatter_raw(text)
        .map(|(_, tail, newline)| tail.strip_prefix(newline).unwrap_or(tail))
        .unwrap_or(text);
    let mut refs = BTreeSet::new();

    collect_markdown_skill_file_references(body, &mut refs);
    for (index, code) in body.split('`').enumerate() {
        if index % 2 == 1 {
            collect_skill_file_reference(code, &mut refs);
        }
    }
    for token in body.split_whitespace() {
        collect_skill_file_reference(token, &mut refs);
    }

    refs.into_iter().collect()
}

fn collect_markdown_skill_file_references(text: &str, refs: &mut BTreeSet<PathBuf>) {
    let mut remaining = text;
    while let Some((_, after_open)) = remaining.split_once("](") {
        let (destination, after_close) = if let Some(after_angle) = after_open.strip_prefix('<') {
            let Some((destination, after_close)) = after_angle.split_once('>') else {
                break;
            };
            (destination, after_close)
        } else {
            let Some((destination, after_close)) = after_open.split_once(')') else {
                break;
            };
            (
                destination.split_whitespace().next().unwrap_or_default(),
                after_close,
            )
        };
        collect_skill_file_reference(destination, refs);
        remaining = after_close;
    }
}

fn collect_skill_file_reference(raw: &str, refs: &mut BTreeSet<PathBuf>) {
    let raw = raw.trim().trim_matches(|ch: char| {
        matches!(
            ch,
            '`' | '"'
                | '\''
                | '<'
                | '>'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | ','
                | ';'
                | ':'
                | '。'
                | '，'
                | '；'
                | '：'
        )
    });
    let raw = raw.rsplit("](").next().unwrap_or(raw);
    let raw = raw.split(['?', '#']).next().unwrap_or(raw);
    if raw.is_empty() || raw.contains("://") {
        return;
    }
    let normalized = raw.replace('\\', "/");
    let path = PathBuf::from(normalized);
    let is_skill_file = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("SKILL.md"));
    let has_skill_parent = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .is_some_and(|name| !name.is_empty() && name != "." && name != "..");
    if is_skill_file && has_skill_parent {
        refs.insert(path);
    }
}

fn resolve_skill_file_reference(skill_file: &Path, reference: &Path) -> Option<PathBuf> {
    let path = if reference.is_absolute() {
        reference.to_path_buf()
    } else {
        skill_file.parent()?.join(reference)
    };
    Some(path.canonicalize().unwrap_or(path))
}

fn skill_file_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn collect_dependency_value(value: &Value, dependencies: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => {
            for item in text.split([',', ' ']) {
                let item = item.trim().trim_matches(['`', '/', '$', '"', '\'']);
                if !item.is_empty() {
                    dependencies.insert(item.to_string());
                }
            }
        }
        Value::Sequence(items) => {
            for item in items {
                collect_dependency_value(item, dependencies);
            }
        }
        Value::Mapping(map) => {
            if let Some(name) = map.get("name") {
                collect_dependency_value(name, dependencies);
            } else {
                for key in map.keys() {
                    collect_dependency_value(key, dependencies);
                }
            }
        }
        _ => {}
    }
}

fn parse_frontmatter_lenient(yaml: &str) -> Option<Value> {
    let mut root = serde_yaml::Mapping::new();
    let lines = yaml.lines().collect::<Vec<_>>();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() || line.starts_with(char::is_whitespace) {
            index += 1;
            continue;
        }

        let Some((key, value)) = line.split_once(':') else {
            index += 1;
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "name" | "description" | "source" | "version" => {
                if !value.is_empty() {
                    root.insert(
                        Value::String(key.to_string()),
                        Value::String(unquote_frontmatter_scalar(value).to_string()),
                    );
                }
            }
            _ if let Ok(parsed) = value.parse::<bool>() => {
                root.insert(Value::String(key.to_string()), Value::Bool(parsed));
            }
            "tags" => {
                let mut tags = Vec::new();
                if !value.is_empty() {
                    tags.push(Value::String(unquote_frontmatter_scalar(value).to_string()));
                }
                let mut next = index + 1;
                while let Some(item) = lines.get(next).map(|line| line.trim_start()) {
                    let Some(tag) = item.strip_prefix("- ") else {
                        break;
                    };
                    let tag = tag.trim();
                    if !tag.is_empty() {
                        tags.push(Value::String(unquote_frontmatter_scalar(tag).to_string()));
                    }
                    next += 1;
                }
                if !tags.is_empty() {
                    root.insert(Value::String(key.to_string()), Value::Sequence(tags));
                }
                index = next.saturating_sub(1);
            }
            "tendi" => {
                let mut tendi = serde_yaml::Mapping::new();
                let mut next = index + 1;
                while let Some(line) = lines.get(next) {
                    if !line.starts_with(char::is_whitespace) {
                        break;
                    }
                    let trimmed = line.trim();
                    if let Some((child_key, child_value)) = trimmed.split_once(':') {
                        let child_key = child_key.trim();
                        let child_value = child_value.trim();
                        if child_key == "visibility" && !child_value.is_empty() {
                            tendi.insert(
                                Value::String(child_key.to_string()),
                                Value::String(unquote_frontmatter_scalar(child_value).to_string()),
                            );
                        }
                    }
                    next += 1;
                }
                if !tendi.is_empty() {
                    root.insert(Value::String(key.to_string()), Value::Mapping(tendi));
                }
                index = next.saturating_sub(1);
            }
            _ => {}
        }
        index += 1;
    }

    (!root.is_empty()).then_some(Value::Mapping(root))
}

fn unquote_frontmatter_scalar(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
}

fn parse_frontmatter_tags(frontmatter: &Value) -> Vec<String> {
    let Some(tags) = frontmatter.get("tags") else {
        return Vec::new();
    };
    let mut values = BTreeSet::new();
    match tags {
        Value::Sequence(items) => {
            for item in items {
                if let Some(tag) = item.as_str().map(str::trim).filter(|tag| !tag.is_empty()) {
                    values.insert(tag.to_string());
                }
            }
        }
        Value::String(tag) => {
            let tag = tag.trim();
            if !tag.is_empty() {
                values.insert(tag.to_string());
            }
        }
        _ => {}
    }
    values.into_iter().collect()
}

pub(crate) fn combine_skill_visibility(
    tendi_visibility: Option<SkillVisibility>,
    provider_visibility: SkillVisibility,
) -> SkillVisibility {
    if provider_visibility == SkillVisibility::Off || tendi_visibility == Some(SkillVisibility::Off)
    {
        return SkillVisibility::Off;
    }
    if tendi_visibility == Some(SkillVisibility::Manual)
        || provider_visibility == SkillVisibility::Manual
    {
        return SkillVisibility::Manual;
    }
    SkillVisibility::Auto
}

fn unique_skill_named<'a>(skills: &'a [SkillRecord], name: &str) -> Option<&'a SkillRecord> {
    let normalized = normalize_skill_match_name(name);
    let mut candidates = skills
        .iter()
        .filter(|skill| normalize_skill_match_name(&skill.name) == normalized)
        .collect::<Vec<_>>();
    (candidates.len() == 1).then(|| candidates.pop().unwrap())
}

pub fn skill_matches_id(skill: &SkillRecord, id: &str) -> bool {
    !id.trim().is_empty()
        && if skill.id.trim().is_empty() {
            ensure_skill_record_id(skill) == id
        } else {
            skill.id == id
        }
}

pub fn skill_ids_matching_pattern(scan: &SkillScan, pattern: &str) -> Vec<String> {
    scan.skills
        .iter()
        .filter(|skill| matches_pattern(&skill.name, pattern))
        .map(ensure_skill_record_id)
        .collect()
}

/// Returns the stable identity of one installed skill location.
///
/// The identity is based on location metadata only. It deliberately excludes content and
/// provenance values such as `sha256`, `source_version`, and `update_status`, which can change
/// without the installed location changing.
pub fn skill_location_id(path: &SkillPath) -> String {
    let canonical_path = canonical_skill_dir(&path.path)
        .to_string_lossy()
        .into_owned();
    let observed_path = path.path.to_string_lossy();
    format!(
        "skill-location@v1:agent={}:scope={}:canonical={}:path={}",
        encode_skill_location_component(path.agent.label()),
        encode_skill_location_component(&path.scope),
        encode_skill_location_component(&canonical_path),
        encode_skill_location_component(&observed_path),
    )
}

fn encode_skill_location_component(value: &str) -> String {
    format!("{}:{value}", value.len())
}

fn ensure_skill_record_id(skill: &SkillRecord) -> String {
    skill_record_id(&skill.name, &skill.paths)
}

fn merge_raw_skills(raws: Vec<RawSkill>) -> Vec<SkillRecord> {
    let mut by_group = BTreeMap::<PathBuf, Vec<RawSkill>>::new();
    for raw in raws {
        by_group
            .entry(skill_merge_group_key(&raw.path))
            .or_default()
            .push(raw);
    }
    by_group
        .into_iter()
        .map(|(_key, group)| {
            let name = group
                .first()
                .map(|raw| raw.name.clone())
                .unwrap_or_default();
            merge_skill(name, group)
        })
        .collect()
}

fn skill_merge_group_key(path: &SkillPath) -> PathBuf {
    canonical_skill_dir(&path.path)
}

fn skill_record_id(name: &str, paths: &[SkillPath]) -> String {
    let Some(path) = paths.first() else {
        return format!("skill:{name}");
    };
    format!(
        "skill@path:{}",
        canonical_skill_dir(&path.path).to_string_lossy()
    )
}

fn canonical_skill_dir(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn infer_skill_root_for_dir(skill_dir: &Path) -> Option<SkillRoot> {
    let skills_root = skill_dir.parent()?;
    if skills_root.file_name()?.to_str()? != "skills" {
        return None;
    }
    let agent_dir = skills_root.parent()?;
    let agent = match agent_dir.file_name()?.to_str()? {
        ".agents" => AgentKind::Shared,
        ".claude" => AgentKind::Claude,
        ".codex" => AgentKind::Codex,
        ".cursor" => AgentKind::Cursor,
        _ => return None,
    };
    let scope = match dirs::home_dir() {
        Some(home) if agent_dir.parent().is_some_and(|parent| parent == home) => "global",
        _ => "project",
    };
    Some(SkillRoot {
        path: skills_root.to_path_buf(),
        scope: scope.to_string(),
        agent,
        plugin_id: None,
        plugin_enabled: None,
    })
}

fn merge_skill(name: String, raws: Vec<RawSkill>) -> SkillRecord {
    let canonical_path = raws.first().map(|raw| canonical_skill_dir(&raw.path.path));
    assert!(
        raws.iter().all(|raw| {
            canonical_path
                .as_ref()
                .is_some_and(|canonical| canonical == &canonical_skill_dir(&raw.path.path))
        }),
        "skill record paths must resolve to one canonical directory"
    );
    let mut agents = BTreeSet::new();
    let mut tags = BTreeSet::new();
    let mut dependencies = BTreeSet::new();
    let mut paths = Vec::new();
    let mut source_summaries = BTreeSet::new();
    let mut install_targets = BTreeSet::new();
    let mut update_statuses = BTreeSet::new();
    let mut ctime = None;
    let mut mtime = None;
    let mut visibility = SkillVisibility::Auto;
    let mut is_system = true;
    let mut is_wrapper = false;
    let description = raws.iter().find_map(|raw| raw.description.clone());
    let effective_visibilities = raws
        .iter()
        .map(|raw| raw.path.effective_visibility)
        .collect::<BTreeSet<_>>();
    if effective_visibilities.len() == 1 {
        visibility = *effective_visibilities
            .iter()
            .next()
            .unwrap_or(&SkillVisibility::Auto);
    } else if effective_visibilities.len() > 1 {
        visibility = SkillVisibility::Mixed;
    }

    for raw in raws {
        let (raw_ctime, raw_mtime) = skill_times(&raw.path.path, &raw.path.path.join("SKILL.md"));
        update_latest_timestamp(&mut ctime, raw_ctime);
        update_latest_timestamp(&mut mtime, raw_mtime);
        is_system &= raw.is_system;
        is_wrapper |= raw.is_wrapper;
        tags.extend(raw.tags);
        dependencies.extend(raw.dependencies);
        agents.insert(raw.path.agent);
        source_summaries.insert(match &raw.path.source {
            Some(source) => format!("{}:{}", raw.path.source_kind, source),
            None => raw.path.source_kind.clone(),
        });
        install_targets.insert(raw.path.install_target.clone());
        update_statuses.insert(raw.path.update_status.clone());
        paths.push(raw.path);
    }

    let id = skill_record_id(&name, &paths);
    let installation_id = InstallationId::new(id.clone())
        .expect("skill record identity is constructed from a non-empty skill name");
    SkillRecord {
        id,
        installation_id: installation_id.to_string(),
        name,
        description,
        tags: tags.into_iter().collect(),
        dependencies: dependencies.into_iter().collect(),
        dependents: Vec::new(),
        dependency_ids: Vec::new(),
        dependent_ids: Vec::new(),
        is_wrapper,
        visibility,
        agents: agents.into_iter().collect(),
        paths,
        source_summary: source_summaries.into_iter().next().unwrap_or_default(),
        install_targets: install_targets.into_iter().collect(),
        update_status: summarize_update_status(update_statuses),
        is_system,
        ctime,
        mtime,
    }
}

fn skill_times(skill_dir: &Path, skill_file: &Path) -> (Option<String>, Option<String>) {
    let directory_created = fs::symlink_metadata(skill_dir)
        .ok()
        .and_then(|value| value.created().ok())
        .and_then(usable_skill_time);
    let file_modified = fs::metadata(skill_file)
        .ok()
        .and_then(|value| value.modified().ok())
        .and_then(usable_skill_time);
    (
        directory_created.and_then(system_time_to_iso),
        skill_modified_time(file_modified, directory_created),
    )
}

fn usable_skill_time(value: std::time::SystemTime) -> Option<std::time::SystemTime> {
    let elapsed = value.duration_since(std::time::UNIX_EPOCH).ok()?;
    // Packaged plugin files can carry Unix epoch + 1 second as an archive
    // placeholder instead of a real content modification time.
    (elapsed > std::time::Duration::from_secs(1)).then_some(value)
}

fn skill_modified_time(
    modified: Option<std::time::SystemTime>,
    created: Option<std::time::SystemTime>,
) -> Option<String> {
    modified
        .and_then(usable_skill_time)
        .or_else(|| created.and_then(usable_skill_time))
        .and_then(system_time_to_iso)
}

fn system_time_to_iso(value: std::time::SystemTime) -> Option<String> {
    let millis = i64::try_from(
        value
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis(),
    )
    .ok()?;
    Utc.timestamp_millis_opt(millis)
        .single()
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn update_latest_timestamp(current: &mut Option<String>, candidate: Option<String>) {
    let Some(candidate) = candidate else {
        return;
    };
    if current
        .as_deref()
        .is_none_or(|value| compare_timestamps(Some(candidate.as_str()), Some(value)).is_gt())
    {
        *current = Some(candidate);
    }
}

fn resolve_raw_skill_path_dependencies(skills: &mut [RawSkill]) {
    let by_file = skills
        .iter()
        .map(|skill| {
            (
                skill_file_key(&skill.path.path.join("SKILL.md")),
                skill.name.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    for skill in skills {
        let mut dependencies = skill.dependencies.iter().cloned().collect::<BTreeSet<_>>();
        dependencies.extend(
            skill
                .dependency_files
                .iter()
                .filter_map(|path| by_file.get(&skill_file_key(path)).cloned()),
        );
        skill.dependencies = dependencies.into_iter().collect();
    }
}

fn resolve_scanned_skill_relations(skills: &mut [SkillRecord]) {
    let known_names = skills
        .iter()
        .map(|skill| normalize_skill_match_name(&skill.name))
        .collect::<BTreeSet<_>>();
    let mut dependents_by_id: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut dependent_ids_by_id: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut resolved_dependencies = Vec::with_capacity(skills.len());
    let mut resolved_dependency_ids = Vec::with_capacity(skills.len());

    for skill in skills.iter() {
        let own_name = normalize_skill_match_name(&skill.name);
        let mut seen = BTreeSet::new();
        let mut dependencies = Vec::new();
        let mut dependency_ids = Vec::new();
        for dependency in &skill.dependencies {
            let normalized = normalize_skill_match_name(dependency);
            if normalized == own_name || !known_names.contains(&normalized) {
                continue;
            }
            if !seen.insert(normalized) {
                continue;
            }
            let Some(preferred) = unique_skill_named(skills, dependency) else {
                continue;
            };
            dependencies.push(preferred.name.clone());
            let preferred_id = ensure_skill_record_id(preferred);
            dependency_ids.push(preferred_id.clone());
            dependents_by_id
                .entry(preferred_id.clone())
                .or_default()
                .insert(skill.name.clone());
            dependent_ids_by_id
                .entry(preferred_id)
                .or_default()
                .insert(ensure_skill_record_id(skill));
        }
        resolved_dependencies.push(dependencies);
        resolved_dependency_ids.push(dependency_ids);
    }

    for ((skill, dependencies), dependency_ids) in skills
        .iter_mut()
        .zip(resolved_dependencies)
        .zip(resolved_dependency_ids)
    {
        skill.dependencies = dependencies;
        skill.dependency_ids = dependency_ids;
        skill.dependents = dependents_by_id
            .remove(&ensure_skill_record_id(skill))
            .map(|names| names.into_iter().collect())
            .unwrap_or_default();
        skill.dependent_ids = dependent_ids_by_id
            .remove(&ensure_skill_record_id(skill))
            .map(|ids| ids.into_iter().collect())
            .unwrap_or_default();
    }
}

pub fn format_skill_table(skills: &[SkillRecord]) -> String {
    let mut lines = vec![format!(
        "{:<28} {:<8} {:<14} {:<28} {:<18} {:<12} {}",
        "name", "mode", "tags", "agents", "source", "update", "target"
    )];

    for skill in skills {
        let agents = skill
            .agents
            .iter()
            .map(|agent| agent.label())
            .collect::<Vec<_>>()
            .join(",");
        let tags = skill.tags.join(",");
        let target = skill.install_targets.join(",");
        lines.push(format!(
            "{:<28} {:<8} {:<14} {:<28} {:<18} {:<12} {}",
            skill.name,
            skill.visibility.label(),
            compact(&tags, 14),
            agents,
            compact(&skill.source_summary, 18),
            compact(&skill.update_status, 12),
            target
        ));
    }

    lines.join("\n")
}

#[derive(Debug, Clone)]
struct Provenance {
    kind: String,
    source: Option<String>,
    source_ref: Option<String>,
    version: Option<String>,
    relative_path: Option<String>,
    update_status: String,
}

#[derive(Debug, Clone)]
struct GitRepositoryProvenance {
    root: PathBuf,
    remote: Option<String>,
    head: Option<String>,
}

#[derive(Debug, Default)]
struct ProvenanceResolver {
    repositories: BTreeMap<PathBuf, Option<GitRepositoryProvenance>>,
    project_repositories: BTreeSet<PathBuf>,
    source_records: BTreeMap<PathBuf, SkillSourceRecord>,
}

impl ProvenanceResolver {
    fn managed(
        cwd: &Path,
        records: Vec<SkillSourceRecord>,
        additional_project_dirs: &[PathBuf],
    ) -> Self {
        let project_repositories = crate::providers::ProviderContext::with_additional_project_dirs(
            cwd,
            additional_project_dirs,
        )
        .project_dirs()
        .iter()
        .filter_map(|directory| git_repository_boundary(directory))
        .collect();
        Self {
            project_repositories,
            source_records: records
                .into_iter()
                .map(|record| (record.skill_path.clone(), record))
                .collect(),
            ..Self::default()
        }
    }

    fn from_skills<'a>(skills: impl IntoIterator<Item = &'a SkillRecord>) -> Self {
        let source_records = skills
            .into_iter()
            .flat_map(|skill| {
                skill.paths.iter().map(|path| SkillSourceRecord {
                    skill_name: skill.name.clone(),
                    skill_path: path.path.clone(),
                    source_kind: path.source_kind.clone(),
                    source: path.source.clone(),
                    source_ref: path.source_ref.clone(),
                    source_version: path.source_version.clone(),
                    source_relative_path: path.source_relative_path.clone(),
                    update_status: path.update_status.clone(),
                    origin: "scan-cache".to_string(),
                })
            })
            .map(|record| (record.skill_path.clone(), record))
            .collect();
        Self {
            source_records,
            ..Self::default()
        }
    }

    fn infer_installed(
        &mut self,
        installed_dir: &Path,
        provenance_dir: &Path,
        install_root: &Path,
        scope: &str,
        name: &str,
        frontmatter: &Option<Value>,
    ) -> Provenance {
        if let Some(record) = self.source_records.get(installed_dir).filter(|record| {
            normalize_skill_match_name(&record.skill_name) == normalize_skill_match_name(name)
        }) {
            return record.provenance();
        }

        if let Some(provenance) = frontmatter_provenance(frontmatter) {
            return provenance;
        }

        let repository = self.repository_for(provenance_dir);
        let canonical_root = install_root
            .canonicalize()
            .unwrap_or_else(|_| install_root.to_path_buf());
        let is_materialized_inside_root = provenance_dir.starts_with(&canonical_root);

        if !is_materialized_inside_root {
            if let Some(repository) = repository.clone() {
                return repository_provenance(provenance_dir, repository);
            }
        }

        let is_project_repository = scope == "project"
            && repository
                .as_ref()
                .is_some_and(|repository| self.project_repositories.contains(&repository.root));
        // A Git remote on the project identifies the project, not an external skill source.
        if scope == "project" && (is_materialized_inside_root || is_project_repository) {
            return local_provenance(provenance_dir);
        }

        repository
            .map(|repository| repository_provenance(provenance_dir, repository))
            .unwrap_or_else(|| local_provenance(provenance_dir))
    }

    fn repository_for(&mut self, skill_dir: &Path) -> Option<GitRepositoryProvenance> {
        let candidate = git_repository_boundary(skill_dir)?;
        self.repositories
            .entry(candidate.clone())
            .or_insert_with(|| {
                Some(GitRepositoryProvenance {
                    remote: git_output(&candidate, &["config", "--get", "remote.origin.url"]),
                    head: git_output(&candidate, &["rev-parse", "HEAD"]),
                    root: candidate.clone(),
                })
            })
            .clone()
    }
}

impl SkillSourceRecord {
    fn provenance(&self) -> Provenance {
        Provenance {
            kind: self.source_kind.clone(),
            source: self.source.clone(),
            source_ref: self.source_ref.clone(),
            version: self.source_version.clone(),
            relative_path: self.source_relative_path.clone(),
            update_status: self.update_status.clone(),
        }
    }
}

fn frontmatter_provenance(frontmatter: &Option<Value>) -> Option<Provenance> {
    let source = frontmatter
        .as_ref()
        .and_then(|value| value.get("source").or_else(|| value.get("source_url")))
        .and_then(Value::as_str)?;
    Some(Provenance {
        kind: if source.contains("github.com") {
            "github".to_string()
        } else {
            "registry".to_string()
        },
        source: Some(source.to_string()),
        source_ref: None,
        version: frontmatter
            .as_ref()
            .and_then(|value| value.get("version"))
            .and_then(Value::as_str)
            .map(str::to_string),
        relative_path: None,
        update_status: "checkable".to_string(),
    })
}

fn repository_provenance(skill_dir: &Path, repository: GitRepositoryProvenance) -> Provenance {
    let relative_path = skill_dir
        .strip_prefix(&repository.root)
        .ok()
        .map(|path| path.display().to_string())
        .filter(|path| !path.is_empty());
    Provenance {
        kind: if repository
            .remote
            .as_deref()
            .is_some_and(|remote| remote.contains("github.com"))
        {
            "github".to_string()
        } else {
            "git".to_string()
        },
        source: repository
            .remote
            .or_else(|| Some(repository.root.display().to_string())),
        source_ref: None,
        version: repository.head,
        relative_path,
        update_status: "checkable".to_string(),
    }
}

fn local_provenance(skill_dir: &Path) -> Provenance {
    Provenance {
        kind: "local".to_string(),
        source: Some(skill_dir.display().to_string()),
        source_ref: None,
        version: None,
        relative_path: None,
        update_status: "local".to_string(),
    }
}

fn git_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = git::run_git(
        cwd,
        args,
        git::LOCAL_COMMAND_TIMEOUT,
        git::never_cancelled(),
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn git_repository_is_shallow(repo: &Path) -> bool {
    git_output(repo, &["rev-parse", "--is-shallow-repository"]).as_deref() == Some("true")
}

fn git_repository_boundary(path: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    canonical
        .ancestors()
        .find(|ancestor| fs::symlink_metadata(ancestor.join(".git")).is_ok())
        .map(Path::to_path_buf)
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<()> {
    let timeout = if is_network_git_command(args) {
        git::NETWORK_COMMAND_TIMEOUT
    } else {
        git::LOCAL_COMMAND_TIMEOUT
    };
    let output = git::run_git(cwd, args, timeout, git::never_cancelled())
        .with_context(|| format!("failed to run git in {}", cwd.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "git {:?} failed in {}: {}",
        args,
        cwd.display(),
        stderr.trim()
    )
}

fn is_network_git_command(args: &[&str]) -> bool {
    matches!(args.first().copied(), Some("clone" | "fetch" | "pull"))
}

fn symlink_status(root: &Path, skill_dir: &Path) -> String {
    let root_link = fs::symlink_metadata(root)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    let skill_link = fs::symlink_metadata(skill_dir)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);

    if root_link || skill_link {
        if root.canonicalize().is_ok() && skill_dir.canonicalize().is_ok() {
            "symlink-ok".to_string()
        } else {
            "symlink-broken".to_string()
        }
    } else {
        "direct".to_string()
    }
}

fn install_target(agent: AgentKind, root: &Path) -> String {
    format!("{}:{}", agent.label(), root.display())
}

fn is_update_check_path(path: &SkillPath) -> bool {
    matches!(path.update_status.as_str(), "checkable" | "tracked")
}

fn select_update_path(skill: &SkillRecord) -> Option<&SkillPath> {
    skill
        .paths
        .iter()
        .find(|path| is_update_check_path(path))
        .or_else(|| skill.paths.first())
}

fn summarize_update_status(statuses: BTreeSet<String>) -> String {
    if statuses.iter().any(|status| status == "checkable") {
        "checkable".to_string()
    } else if statuses.iter().any(|status| status == "local") {
        "local".to_string()
    } else {
        statuses.into_iter().next().unwrap_or_default()
    }
}

fn check_skill_updates_for_skills(
    skills: &[&SkillRecord],
    cancelled: &AtomicBool,
) -> Vec<SkillUpdateReport> {
    let started = Instant::now();
    crate::logging::global().info(
        "skill update check started",
        serde_json::json!({ "skillCount": skills.len() }),
    );
    let stage_started = Instant::now();
    let mut git_remote_heads = fetch_git_remote_heads(skills, cancelled);
    crate::logging::global().info(
        "skill update git remote heads completed",
        serde_json::json!({
            "skillCount": skills.len(),
            "repoCount": git_remote_heads.len(),
            "resolvedRepoCount": git_remote_heads.values().filter(|head| head.is_some()).count(),
            "durationMs": stage_started.elapsed().as_secs_f64() * 1000.0,
        }),
    );
    let stage_started = Instant::now();
    fetch_git_remote_commits(skills, &mut git_remote_heads, cancelled);
    crate::logging::global().info(
        "skill update git commits completed",
        serde_json::json!({
            "skillCount": skills.len(),
            "durationMs": stage_started.elapsed().as_secs_f64() * 1000.0,
        }),
    );
    let stage_started = Instant::now();
    let git_changed_paths = fetch_git_changed_paths(skills, &git_remote_heads, cancelled);
    crate::logging::global().info(
        "skill update git changed paths completed",
        serde_json::json!({
            "skillCount": skills.len(),
            "repoCount": git_changed_paths.len(),
            "durationMs": stage_started.elapsed().as_secs_f64() * 1000.0,
        }),
    );
    let mut reports = Vec::with_capacity(skills.len());
    for batch in skills.chunks(MAX_CONCURRENT_GIT_FETCHES) {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        let batch_reports = std::thread::scope(|scope| {
            let remote_heads = &git_remote_heads;
            let changed_paths = &git_changed_paths;
            batch
                .iter()
                .map(|skill| {
                    let inherited = skill_update_resource_reservation(skill, cancelled);
                    scope.spawn(move || {
                        let inherited = match inherited {
                            Ok(reservation) => reservation,
                            Err(error) => {
                                crate::logging::global().warn(
                                    "skill update resource delegation failed",
                                    serde_json::json!({
                                        "skill": skill.name,
                                        "error": error.to_string(),
                                    }),
                                );
                                None
                            }
                        };
                        let _inherited = inherited.map(|reservation| reservation.enter());
                        check_skill_update(skill, remote_heads, changed_paths, cancelled)
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .collect::<Vec<_>>()
        });
        reports.extend(batch_reports);
    }
    cleanup_git_remote_heads(git_remote_heads);
    crate::logging::global().info(
        "skill update check completed",
        serde_json::json!({
            "skillCount": skills.len(),
            "durationMs": started.elapsed().as_secs_f64() * 1000.0,
        }),
    );
    reports
}

fn skill_update_resource_reservation(
    skill: &SkillRecord,
    cancelled: &AtomicBool,
) -> Result<Option<crate::coordination::ResourceReservation>> {
    let Some(path) = select_update_path(skill) else {
        return Ok(None);
    };
    if !is_git_source_kind(&path.source_kind) || git_repository_boundary(&path.path).is_some() {
        return Ok(None);
    }
    let Some(repo) = git_checkout_for_skill_path(path, cancelled) else {
        return Ok(None);
    };
    let mut paths = git::mutation_resource_paths(&repo)?;
    paths.push(repo);
    crate::coordination::fork_current_file_resources(&paths)
}

#[derive(Clone)]
struct GitRemoteHead {
    oid: String,
    reference: String,
}

fn fetch_git_remote_heads(
    skills: &[&SkillRecord],
    cancelled: &AtomicBool,
) -> BTreeMap<PathBuf, Option<GitRemoteHead>> {
    let remotes = skills
        .iter()
        .filter_map(|skill| {
            let path = skill.paths.iter().find(|path| is_update_check_path(path))?;
            is_git_source_kind(&path.source_kind).then(|| {
                Some((
                    git_checkout_for_skill_path(path, cancelled)?,
                    (path.source.clone()?, path.source_ref.clone()),
                ))
            })?
        })
        .collect::<BTreeMap<_, _>>();
    let remotes = remotes.into_iter().collect::<Vec<_>>();
    let mut heads = BTreeMap::new();
    for batch in remotes.chunks(MAX_CONCURRENT_GIT_FETCHES) {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        let results = std::thread::scope(|scope| {
            batch
                .iter()
                .map(|(repo, (source, source_ref))| {
                    scope.spawn(move || {
                        (
                            repo.clone(),
                            resolve_git_remote_head(repo, source, source_ref.as_deref(), cancelled),
                        )
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .collect::<Vec<_>>()
        });
        heads.extend(results);
    }
    heads
}

fn fetch_git_remote_commits(
    skills: &[&SkillRecord],
    heads: &mut BTreeMap<PathBuf, Option<GitRemoteHead>>,
    cancelled: &AtomicBool,
) {
    let mut requests = BTreeMap::<PathBuf, String>::new();
    for skill in skills {
        let Some(path) = skill.paths.iter().find(|path| is_update_check_path(path)) else {
            continue;
        };
        let Some(repo) = git_checkout_for_skill_path(path, cancelled) else {
            continue;
        };
        let Some(Some(head)) = heads.get(&repo) else {
            continue;
        };
        let needs_commit = if git_repository_boundary(&path.path).is_none() {
            !path.source_version.as_deref().is_some_and(|current| {
                current.len() == 40 && current.eq_ignore_ascii_case(&head.oid)
            })
        } else {
            !source_revision_matches(path.source_version.as_deref(), &head.oid)
        };
        if needs_commit {
            if let Some(source) = path.source.clone() {
                requests.entry(repo).or_insert(source);
            }
        }
    }

    let requests = requests.into_iter().collect::<Vec<_>>();
    for batch in requests.chunks(MAX_CONCURRENT_GIT_FETCHES) {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        let results = std::thread::scope(|scope| {
            batch
                .iter()
                .map(|(repo, source)| {
                    let head = heads.get(repo).and_then(Option::as_ref);
                    let inherited = git::mutation_resource_paths(repo)
                        .and_then(|paths| crate::coordination::fork_current_file_resources(&paths));
                    scope.spawn(move || {
                        let inherited = match inherited {
                            Ok(reservation) => reservation,
                            Err(error) => {
                                crate::logging::global().warn(
                                    "git fetch resource delegation failed",
                                    serde_json::json!({ "repo": repo, "error": error.to_string() }),
                                );
                                return (repo.clone(), false);
                            }
                        };
                        let _inherited = inherited.map(|reservation| reservation.enter());
                        let fetched = head.is_some_and(|head| {
                            fetch_git_remote_commit(
                                repo,
                                source,
                                &head.oid,
                                &head.reference,
                                cancelled,
                            )
                        });
                        (repo.clone(), fetched)
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .collect::<Vec<_>>()
        });
        for (repo, fetched) in results {
            if !fetched {
                heads.insert(repo, None);
            }
        }
    }
}

fn fetch_git_changed_paths(
    skills: &[&SkillRecord],
    git_remote_heads: &BTreeMap<PathBuf, Option<GitRemoteHead>>,
    cancelled: &AtomicBool,
) -> BTreeMap<PathBuf, Option<BTreeSet<String>>> {
    let mut requests = BTreeMap::<PathBuf, (String, BTreeSet<String>)>::new();
    for skill in skills {
        let Some(path) = skill.paths.iter().find(|path| is_update_check_path(path)) else {
            continue;
        };
        if !is_git_source_kind(&path.source_kind) || git_repository_boundary(&path.path).is_none() {
            continue;
        }
        let Some(repo) = git_checkout_for_skill_path(path, cancelled) else {
            continue;
        };
        let Some(Some(head)) = git_remote_heads.get(&repo) else {
            continue;
        };
        if source_revision_matches(path.source_version.as_deref(), &head.oid) {
            continue;
        }
        requests
            .entry(repo)
            .and_modify(|(_, paths)| {
                paths.insert(
                    path.source_relative_path
                        .as_deref()
                        .unwrap_or(".")
                        .to_string(),
                );
            })
            .or_insert_with(|| {
                (
                    head.oid.clone(),
                    BTreeSet::from([path
                        .source_relative_path
                        .as_deref()
                        .unwrap_or(".")
                        .to_string()]),
                )
            });
    }

    let requests = requests.into_iter().collect::<Vec<_>>();
    let mut changed_paths = BTreeMap::new();
    for batch in requests.chunks(MAX_CONCURRENT_GIT_FETCHES) {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        let results = std::thread::scope(|scope| {
            batch
                .iter()
                .map(|(repo, (remote_head, paths))| {
                    scope.spawn(move || {
                        let result = git_diff_changed_paths(repo, remote_head, paths, cancelled);
                        (repo.clone(), result.ok())
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .collect::<Vec<_>>()
        });
        changed_paths.extend(results);
    }
    changed_paths
}

fn check_skill_update(
    skill: &SkillRecord,
    git_remote_heads: &BTreeMap<PathBuf, Option<GitRemoteHead>>,
    git_changed_paths: &BTreeMap<PathBuf, Option<BTreeSet<String>>>,
    cancelled: &AtomicBool,
) -> SkillUpdateReport {
    let Some(path) = select_update_path(skill) else {
        return SkillUpdateReport {
            id: ensure_skill_record_id(skill),
            name: skill.name.clone(),
            status: "unknown".to_string(),
            current_version: None,
            latest_version: None,
            source: None,
            source_kind: "unknown".to_string(),
        };
    };

    match path.source_kind.as_str() {
        "git" | "github" | "gitlab" | "huggingface" => {
            check_git_update(skill, path, git_remote_heads, git_changed_paths, cancelled)
        }
        "registry" => check_registry_update(skill, path),
        _ => SkillUpdateReport {
            id: ensure_skill_record_id(skill),
            name: skill.name.clone(),
            status: "local".to_string(),
            current_version: path.source_version.clone(),
            latest_version: None,
            source: path.source.clone(),
            source_kind: path.source_kind.clone(),
        },
    }
}

fn check_git_update(
    skill: &SkillRecord,
    path: &SkillPath,
    git_remote_heads: &BTreeMap<PathBuf, Option<GitRemoteHead>>,
    git_changed_paths: &BTreeMap<PathBuf, Option<BTreeSet<String>>>,
    cancelled: &AtomicBool,
) -> SkillUpdateReport {
    let Some(source) = path.source.clone() else {
        return SkillUpdateReport {
            id: ensure_skill_record_id(skill),
            name: skill.name.clone(),
            status: "missing-source".to_string(),
            current_version: path.source_version.clone(),
            latest_version: None,
            source: None,
            source_kind: path.source_kind.clone(),
        };
    };

    let materialized = git_repository_boundary(&path.path).is_none();
    let repo = git_checkout_for_skill_path(path, cancelled);
    let latest = repo
        .as_ref()
        .and_then(|repo| git_remote_heads.get(repo).cloned())
        .flatten();
    let latest_oid = latest.as_ref().map(|head| head.oid.as_str());
    let current_matches = repo.as_ref().zip(latest_oid).is_some_and(|(repo, latest)| {
        if materialized {
            materialized_remote_has_no_effective_changes(repo, path, latest)
                .is_some_and(|no_changes| no_changes)
                || git_worktree_matches_revision(repo, path, latest)
        } else {
            source_revision_matches(path.source_version.as_deref(), latest)
                || git_worktree_matches_revision(repo, path, latest)
        }
    });
    let status = match (&path.source_version, latest_oid) {
        (_, Some(_)) if current_matches => "up-to-date",
        (Some(_), Some(_)) if materialized => "update-available",
        (Some(_), Some(_)) => match repo
            .as_ref()
            .and_then(|repo| git_path_changed(git_changed_paths, repo, path))
        {
            Some(true) => "update-available",
            Some(false) => "up-to-date",
            None if cancelled.load(Ordering::Acquire) => "cancelled",
            None => "unreachable",
        },
        (None, Some(_)) => "unknown-current",
        (_, None) => "unreachable",
    };

    SkillUpdateReport {
        id: ensure_skill_record_id(skill),
        name: skill.name.clone(),
        status: status.to_string(),
        current_version: path.source_version.clone(),
        latest_version: latest.map(|head| head.oid),
        source: Some(source),
        source_kind: path.source_kind.clone(),
    }
}

struct MergeOutcome {
    status: String,
    content: Option<String>,
    reason: Option<String>,
}

fn merge_text(base: Option<&str>, local: Option<&str>, incoming: Option<&str>) -> MergeOutcome {
    if local == incoming {
        return MergeOutcome {
            status: "unchanged".to_string(),
            content: local.map(str::to_string),
            reason: None,
        };
    }
    if local == base {
        return MergeOutcome {
            status: "remote".to_string(),
            content: incoming.map(str::to_string),
            reason: None,
        };
    }
    if incoming == base {
        return MergeOutcome {
            status: "local".to_string(),
            content: local.map(str::to_string),
            reason: None,
        };
    }

    let Some(local) = local else {
        return MergeOutcome {
            status: "conflict".to_string(),
            content: Some(format!(
                "<<<<<<< local\n=======\n{}>>>>>>> remote\n",
                incoming.unwrap_or_default()
            )),
            reason: None,
        };
    };
    let Some(incoming) = incoming else {
        return MergeOutcome {
            status: "conflict".to_string(),
            content: Some(format!("<<<<<<< local\n{}=======\n>>>>>>> remote\n", local)),
            reason: None,
        };
    };
    let Some(base) = base else {
        return MergeOutcome {
            status: "conflict".to_string(),
            content: Some(format!(
                "<<<<<<< local\n{}=======\n{}>>>>>>> remote\n",
                local, incoming
            )),
            reason: None,
        };
    };
    match git_merge_file_text(base, local, incoming) {
        Ok((content, conflict)) => MergeOutcome {
            status: if conflict {
                "conflict".to_string()
            } else {
                "merged".to_string()
            },
            content: Some(content),
            reason: None,
        },
        Err(reason) => MergeOutcome {
            status: "unavailable".to_string(),
            content: None,
            reason: Some(reason),
        },
    }
}

fn merge_skill_manifest_text(
    base: Option<&str>,
    local: Option<&str>,
    incoming: Option<&str>,
) -> MergeOutcome {
    if local != incoming && skill_manifest_semantically_equal(local, incoming) {
        return MergeOutcome {
            status: "remote".to_string(),
            content: incoming.map(str::to_string),
            reason: None,
        };
    }
    merge_text(base, local, incoming)
}

fn skill_manifest_semantically_equal(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            let Ok(left) = MarkdownDoc::parse_lenient(left) else {
                return false;
            };
            let Ok(right) = MarkdownDoc::parse_lenient(right) else {
                return false;
            };
            left.meta == right.meta
                && normalize_line_endings(&left.body) == normalize_line_endings(&right.body)
        }
        _ => false,
    }
}

fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn git_merge_file_text(base: &str, local: &str, incoming: &str) -> Result<(String, bool), String> {
    let root = std::env::temp_dir().join(format!(
        "tendi-merge-{}-{}",
        std::process::id(),
        GIT_UPDATE_CHECK_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(error) = fs::create_dir_all(&root) {
        return Err(format!(
            "Could not prepare the automatic merge workspace: {error}"
        ));
    }
    let local_path = root.join("local");
    let base_path = root.join("base");
    let incoming_path = root.join("incoming");
    let result = (|| -> Result<(String, bool), String> {
        fs::write(&local_path, local)
            .map_err(|error| format!("Could not write the local merge input: {error}"))?;
        fs::write(&base_path, base)
            .map_err(|error| format!("Could not write the base merge input: {error}"))?;
        fs::write(&incoming_path, incoming)
            .map_err(|error| format!("Could not write the update merge input: {error}"))?;
        let output = git::run_git(
            &root,
            [
                "merge-file",
                "--stdout",
                "--diff3",
                "-L",
                "local",
                "-L",
                "base",
                "-L",
                "remote",
                "local",
                "base",
                "incoming",
            ],
            git::LOCAL_COMMAND_TIMEOUT,
            git::never_cancelled(),
        )
        .map_err(|error| format!("Automatic merge tool failed: {error}"))?;
        // `git merge-file` returns the number of conflicts (1..=127), not a
        // boolean exit code. A file with multiple conflict regions must still
        // expose its diff3 output as an ordinary conflict; only other exit
        // statuses indicate that the merge tool itself was unavailable.
        let conflict = output
            .status
            .code()
            .is_some_and(|code| (1..=127).contains(&code));
        if !output.status.success() && !conflict {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if detail.is_empty() {
                format!("Automatic merge tool exited with status {}", output.status)
            } else {
                format!("Automatic merge tool failed: {detail}")
            });
        }
        String::from_utf8(output.stdout)
            .map(|content| (content, conflict))
            .map_err(|_| "Automatic merge tool returned non-text output.".to_string())
    })();
    let _ = fs::remove_dir_all(&root);
    result
}

fn normalize_skill_manifest_for_visibility(
    text: &str,
    agent: AgentKind,
    visibility: SkillVisibility,
    provider_visibility: Option<(&str, bool)>,
) -> String {
    let rendered = if let Some((provider_key, provider_visibility)) = provider_visibility {
        render_provider_skill_frontmatter_with_provider_key_value(
            text,
            visibility,
            Some(provider_key),
            Some(provider_visibility),
        )
        .unwrap_or_else(|_| text.to_string())
    } else {
        let provider = crate::providers::agent_provider(agent);
        render_provider_skill_frontmatter_with_provider_key_value(
            text,
            visibility,
            provider.skill_frontmatter_visibility_key(),
            None,
        )
        .unwrap_or_else(|_| text.to_string())
    };

    let canonical_provider_visibility = provider_visibility.or_else(|| {
        crate::providers::agent_provider(agent)
            .skill_frontmatter_visibility_key()
            .map(|provider_key| (provider_key, !matches!(visibility, SkillVisibility::Auto)))
    });
    let rendered = if let Some((provider_key, provider_visibility)) = canonical_provider_visibility
    {
        canonicalize_provider_visibility_key(&rendered, provider_key, provider_visibility)
    } else {
        rendered
    };
    rendered
}

fn canonicalize_provider_visibility_key(
    text: &str,
    provider_key: &str,
    provider_visibility: bool,
) -> String {
    let Some((yaml, tail, newline)) = split_frontmatter_raw(text) else {
        return text.to_string();
    };
    let mut lines = yaml
        .lines()
        .filter(|line| !line.starts_with(&format!("{provider_key}:")))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if provider_visibility {
        lines.push(format!("{provider_key}: true"));
    }
    format!("---{newline}{}{newline}---{tail}", lines.join(newline))
}

fn provider_visibility_override(
    local: Option<&str>,
    base: Option<&str>,
    incoming: Option<&str>,
    agent: AgentKind,
) -> Option<(&'static str, bool)> {
    crate::providers::skill_frontmatter_visibility_keys(agent)
        .into_iter()
        .find_map(|provider_key| {
            let has_provider_visibility =
                [local, base, incoming].into_iter().flatten().any(|text| {
                    MarkdownDoc::parse_lenient(text)
                        .ok()
                        .and_then(|doc| doc.meta.get(provider_key).and_then(Value::as_bool))
                        .is_some()
                });
            has_provider_visibility.then(|| {
                (
                    provider_key,
                    local
                        .and_then(|text| {
                            MarkdownDoc::parse_lenient(text)
                                .ok()
                                .and_then(|doc| doc.meta.get(provider_key).and_then(Value::as_bool))
                        })
                        .unwrap_or(false),
                )
            })
        })
}

fn normalize_skill_manifests_for_merge(
    local: Option<&str>,
    base: Option<&str>,
    incoming: Option<&str>,
    agent: AgentKind,
    visibility: SkillVisibility,
) -> (Option<String>, Option<String>, Option<String>) {
    let provider_visibility = provider_visibility_override(local, base, incoming, agent);
    let normalize = |text: Option<&str>| {
        text.map(|text| {
            normalize_skill_manifest_for_visibility(text, agent, visibility, provider_visibility)
        })
    };
    (normalize(local), normalize(base), normalize(incoming))
}

fn normalize_provider_skill_file_for_merge(
    agent: AgentKind,
    path: &str,
    local: Option<&str>,
    base: Option<&str>,
    incoming: Option<&str>,
    visibility: SkillVisibility,
) -> (Option<String>, Option<String>, Option<String>) {
    crate::providers::normalize_skill_file_for_merge(agent, path, local, base, incoming, visibility)
        .unwrap_or_else(|| {
            (
                local.map(str::to_string),
                base.map(str::to_string),
                incoming.map(str::to_string),
            )
        })
}

fn git_files_at_revision(
    repo: &Path,
    revision: &str,
    relative: &str,
) -> Option<BTreeMap<String, Vec<u8>>> {
    let mut args = vec![
        "archive".to_string(),
        "--format=tar".to_string(),
        revision.to_string(),
    ];
    if !relative.is_empty() && relative != "." {
        args.extend(["--".to_string(), relative.to_string()]);
    }
    let output = git::run_git(
        repo,
        args,
        git::LOCAL_COMMAND_TIMEOUT,
        git::never_cancelled(),
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut files = BTreeMap::new();
    let mut archive = tar::Archive::new(std::io::Cursor::new(output.stdout));
    for entry in archive.entries().ok()? {
        let mut entry = entry.ok()?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            continue;
        }
        let name = entry.path().ok()?.to_string_lossy().replace('\\', "/");
        let mut bytes = Vec::new();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            bytes.extend(entry.link_name().ok()??.to_string_lossy().as_bytes());
        } else if entry_type.is_file() {
            entry.read_to_end(&mut bytes).ok()?;
        } else {
            continue;
        }
        files.insert(name, bytes);
    }
    Some(files)
}

fn git_files_at_tree(repo: &Path, tree: &str, relative: &str) -> Option<BTreeMap<String, Vec<u8>>> {
    let files = git_files_at_revision(repo, tree, ".")?;
    if relative.is_empty() || relative == "." {
        return Some(files);
    }
    Some(
        files
            .into_iter()
            .map(|(file, content)| {
                (
                    Path::new(relative)
                        .join(file)
                        .to_string_lossy()
                        .replace('\\', "/"),
                    content,
                )
            })
            .collect(),
    )
}

fn git_files_at_source_version(
    repo: &Path,
    revision: &str,
    relative: &str,
    materialized: bool,
) -> Option<BTreeMap<String, Vec<u8>>> {
    if !materialized {
        return git_files_at_revision(repo, revision, relative);
    }
    let commit_files = git_files_at_revision(repo, revision, relative);
    if commit_files.as_ref().is_some_and(|files| !files.is_empty()) {
        return commit_files;
    }
    git_files_at_tree(repo, revision, relative)
}

// A previous update can leave the checkout content current while the recorded
// source revision still points at the old commit. In that state the normal
// three-way merge has no effective file changes, so the worktree is already
// up-to-date even though the source revision differs.
fn git_worktree_matches_revision(repo: &Path, path: &SkillPath, revision: &str) -> bool {
    let relative = normalized_skill_repo_path(path);
    let Some(incoming) = git_files_at_revision(repo, revision, relative) else {
        return false;
    };
    if incoming.is_empty() {
        return false;
    }

    let local = local_skill_files(&path.path, repo, relative);
    merge_file_maps(
        Some(incoming.clone()),
        local,
        incoming,
        relative,
        "",
        path.effective_visibility,
        path.agent,
    )
    .is_empty()
}

fn materialized_remote_has_no_effective_changes(
    repo: &Path,
    path: &SkillPath,
    revision: &str,
) -> Option<bool> {
    // A commit is only a candidate signal for materialized skills. Compare the
    // recorded source tree with the incoming tree through the same normalized
    // merge path used by the preview so unrelated/provider-managed files do
    // not create a false update.
    let current = path.source_version.as_deref()?;
    let relative = normalized_skill_repo_path(path);
    let base = git_files_at_source_version(repo, current, relative, true)?;
    let incoming = git_files_at_revision(repo, revision, relative)?;
    let changes = merge_file_maps(
        Some(base.clone()),
        base,
        incoming,
        relative,
        "",
        path.effective_visibility,
        path.agent,
    );
    Some(changes.is_empty())
}

fn local_skill_files(
    skill_dir: &Path,
    repo: &Path,
    repo_relative: &str,
) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in WalkDir::new(skill_dir)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let Ok(relative) = entry.path().strip_prefix(skill_dir) else {
            continue;
        };
        let file = if repo_relative.is_empty() || repo_relative == "." {
            relative.to_path_buf()
        } else {
            Path::new(repo_relative).join(relative)
        };
        if let Ok(content) = fs::read(entry.path()) {
            files.insert(file.to_string_lossy().replace('\\', "/"), content);
        }
    }
    let _ = repo;
    files
}

fn snapshot_files(
    store: &crate::storage::Store,
    path: &SkillPath,
    repo_relative: &str,
    workspace_root: Option<&Path>,
) -> Option<BTreeMap<String, Vec<u8>>> {
    let snapshot = workspace_root
        .map_or_else(
            || store.skill_snapshot(&path.path),
            |workspace_root| store.skill_snapshot_for_workspace(workspace_root, &path.path),
        )
        .ok()
        .flatten()?;
    if path.source_version.as_deref() != Some(snapshot.source_version.as_str()) {
        return None;
    }
    let mut files = BTreeMap::new();
    for file in snapshot.files {
        let repo_file = if repo_relative.is_empty() || repo_relative == "." {
            PathBuf::from(&file.relative_path)
        } else {
            Path::new(repo_relative).join(&file.relative_path)
        };
        files.insert(repo_file.to_string_lossy().replace('\\', "/"), file.content);
    }
    Some(files)
}

fn git_update_base(
    repo: &Path,
    path: &SkillPath,
    relative: &str,
    store: &crate::storage::Store,
    workspace_root: Option<&Path>,
) -> Result<BTreeMap<String, Vec<u8>>> {
    let revision = path.source_version.as_deref().unwrap_or("<missing>");
    let materialized = git_repository_boundary(&path.path).is_none();
    if revision != "<missing>" {
        let base = git_files_at_source_version(repo, revision, relative, materialized);
        if let Some(base) = base {
            return Ok(base);
        }
    }
    if let Some(base) = snapshot_files(store, path, relative, workspace_root) {
        return Ok(base);
    }
    if let (Some(source), Some(revision)) = (path.source.as_deref(), path.source_version.as_deref())
        && ensure_git_object_available(repo, source, revision, git::never_cancelled())
    {
        let base = git_files_at_source_version(repo, revision, relative, materialized);
        if let Some(base) = base {
            return Ok(base);
        }
    }
    bail!(
        "cannot prepare skill update base for {} at {}: recorded revision is unavailable and no matching snapshot exists",
        path.path.display(),
        revision,
    )
}

fn merge_git_path_files(
    repo: &Path,
    path: &SkillPath,
    remote_revision: &str,
    _tendi_settings: &[GitSkillVisibility],
    store: &crate::storage::Store,
    workspace_root: Option<&Path>,
) -> Result<Vec<GitUpdateFile>> {
    let relative = normalized_skill_repo_path(path);
    let base = git_update_base(repo, path, relative, store, workspace_root)?;
    let local = local_skill_files(&path.path, repo, relative);
    let incoming = git_files_at_revision(repo, remote_revision, relative).unwrap_or_default();
    Ok(merge_file_maps(
        Some(base),
        local,
        incoming,
        relative,
        &path.path.display().to_string(),
        path.effective_visibility,
        path.agent,
    ))
}

fn merge_materialized_git_path_files(
    repo: &Path,
    path: &SkillPath,
    remote_revision: &str,
    visibility: SkillVisibility,
    store: &crate::storage::Store,
    workspace_root: Option<&Path>,
) -> Result<Vec<GitUpdateFile>> {
    let relative = normalized_skill_repo_path(path);
    let base = if let Some(revision) =
        materialized_source_revision_for_update(repo, path, remote_revision)
    {
        git_files_at_source_version(repo, &revision, relative, true)
            .context("matching materialized skill source revision has no files")?
    } else {
        git_update_base(repo, path, relative, store, workspace_root)?
    };
    let local = local_skill_files(&path.path, repo, relative);
    let incoming = git_files_at_revision(repo, remote_revision, relative).unwrap_or_default();
    Ok(merge_file_maps(
        Some(base),
        local,
        incoming,
        relative,
        &path.path.display().to_string(),
        visibility,
        path.agent,
    ))
}

/// A materialized skill may have been refreshed by an older installer without
/// advancing Tendi's recorded source revision. If its source-owned files still
/// match a commit between the recorded revision and the incoming revision, use
/// that commit as the merge base. Extra local files remain local and are still
/// preserved by the normal merge.
fn materialized_source_revision_for_update(
    repo: &Path,
    path: &SkillPath,
    remote_revision: &str,
) -> Option<String> {
    let current_revision = path.source_version.as_deref()?;
    if current_revision == remote_revision {
        return Some(current_revision.to_string());
    }
    let relative = normalized_skill_repo_path(path);
    let range = format!("{current_revision}..{remote_revision}");
    let revisions = git::run_git(
        repo,
        [
            "rev-list",
            "--ancestry-path",
            range.as_str(),
            "--",
            relative,
        ],
        git::LOCAL_COMMAND_TIMEOUT,
        git::never_cancelled(),
    )
    .ok()
    .filter(|output| output.status.success())
    .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())?;

    revisions.lines().find_map(|revision| {
        materialized_skill_matches_revision(repo, path, revision).then(|| revision.to_string())
    })
}

fn materialized_skill_matches_revision(repo: &Path, path: &SkillPath, revision: &str) -> bool {
    let relative = normalized_skill_repo_path(path);
    let Some(source_files) = git_files_at_source_version(repo, revision, relative, true) else {
        return false;
    };
    if source_files.is_empty() {
        return false;
    }
    let local_files = local_skill_files(&path.path, repo, relative);
    let source_changes = merge_file_maps(
        Some(source_files.clone()),
        local_files,
        source_files.clone(),
        relative,
        "",
        path.effective_visibility,
        path.agent,
    );
    source_changes
        .iter()
        .all(|file| !source_files.contains_key(&file.path) || file.status != "local")
}

fn merge_file_maps(
    base: Option<BTreeMap<String, Vec<u8>>>,
    local: BTreeMap<String, Vec<u8>>,
    incoming: BTreeMap<String, Vec<u8>>,
    relative: &str,
    key_prefix: &str,
    visibility: SkillVisibility,
    agent: AgentKind,
) -> Vec<GitUpdateFile> {
    let base_available = base.is_some();
    let base = base.unwrap_or_default();
    let mut paths = BTreeSet::new();
    paths.extend(base.keys().cloned());
    paths.extend(local.keys().cloned());
    paths.extend(incoming.keys().cloned());
    let prefix = relative.trim_matches('/');
    paths
        .into_iter()
        .filter_map(|path| {
            let path_in_skill = path
                .strip_prefix(prefix)
                .unwrap_or(&path)
                .trim_start_matches('/');
            if crate::providers::is_managed_skill_file(agent, path_in_skill) {
                return None;
            }
            let before_bytes = local.get(&path);
            let base_bytes = base.get(&path);
            let incoming_bytes = incoming.get(&path);
            let before = before_bytes.map(|bytes| String::from_utf8(bytes.clone()).ok());
            let base_text = base_bytes.map(|bytes| String::from_utf8(bytes.clone()).ok());
            let incoming_text = incoming_bytes.map(|bytes| String::from_utf8(bytes.clone()).ok());
            let resolution_key = format!("{key_prefix}:{path}");
            if !base_available {
                let before = before.flatten();
                let incoming = incoming_text.flatten();
                return Some(GitUpdateFile {
                    path,
                    resolution_key,
                    before: before.unwrap_or_default(),
                    base: String::new(),
                    incoming: incoming.unwrap_or_default(),
                    after: String::new(),
                    before_bytes: before_bytes.cloned(),
                    incoming_bytes: incoming_bytes.cloned(),
                    after_bytes: None,
                    before_exists: before_bytes.is_some(),
                    incoming_exists: incoming_bytes.is_some(),
                    after_exists: false,
                    status: "unavailable".to_string(),
                    reason: Some(
                        "The previous source version is unavailable, so Tendi cannot calculate a three-way merge."
                            .to_string(),
                    ),
                });
            }
            if before.as_ref().is_some_and(Option::is_none)
                || base_text.as_ref().is_some_and(Option::is_none)
                || incoming_text.as_ref().is_some_and(Option::is_none)
            {
                return Some(GitUpdateFile {
                    path,
                    resolution_key,
                    before: String::new(),
                    base: String::new(),
                    incoming: String::new(),
                    after: String::new(),
                    before_bytes: before_bytes.cloned(),
                    incoming_bytes: incoming_bytes.cloned(),
                    after_bytes: None,
                    before_exists: before_bytes.is_some(),
                    incoming_exists: incoming_bytes.is_some(),
                    after_exists: false,
                    status: "binary".to_string(),
                    reason: None,
                });
            }
            let before = before.flatten();
            let mut local = before.clone();
            let mut base = base_text.flatten();
            let mut incoming = incoming_text.flatten();
            if path_in_skill == "SKILL.md" {
                (local, base, incoming) = normalize_skill_manifests_for_merge(
                    local.as_deref(),
                    base.as_deref(),
                    incoming.as_deref(),
                    agent,
                    visibility,
                );
            } else {
                (local, base, incoming) = normalize_provider_skill_file_for_merge(
                    agent,
                    path_in_skill,
                    local.as_deref(),
                    base.as_deref(),
                    incoming.as_deref(),
                    visibility,
                );
            }
            let merged = if path_in_skill == "SKILL.md" {
                merge_skill_manifest_text(base.as_deref(), local.as_deref(), incoming.as_deref())
            } else {
                merge_text(base.as_deref(), local.as_deref(), incoming.as_deref())
            };
            if merged.status == "unchanged" {
                return None;
            }
            let merged_content = merged.content;
            let merged_status = merged.status;
            Some(GitUpdateFile {
                path,
                resolution_key,
                before: before.unwrap_or_default(),
                base: base.unwrap_or_default(),
                incoming: incoming.unwrap_or_default(),
                after: if merged_status == "unavailable" {
                    String::new()
                } else {
                    merged_content.clone().unwrap_or_default()
                },
                before_bytes: before_bytes.cloned(),
                incoming_bytes: incoming_bytes.cloned(),
                after_bytes: None,
                before_exists: before_bytes.is_some(),
                incoming_exists: incoming_bytes.is_some(),
                after_exists: !matches!(merged_status.as_str(), "conflict" | "unavailable" | "binary")
                    && merged_content.is_some(),
                status: merged_status,
                reason: merged.reason,
            })
        })
        .collect()
}

fn plan_git_update(
    scan: &SkillScan,
    skill: &SkillRecord,
    path: &SkillPath,
    update: &SkillUpdateReport,
    store: &crate::storage::Store,
    workspace_root: Option<&Path>,
) -> Result<Option<GitUpdateAction>> {
    let materialized = git_repository_boundary(&path.path).is_none();
    let Some(repo) = git_checkout_for_skill_path(path, git::never_cancelled()) else {
        return Ok(None);
    };
    let tendi_settings = if materialized {
        Vec::new()
    } else {
        git_tendi_settings(scan, &repo)
    };
    let Some(latest) = update.latest_version.as_deref() else {
        return Ok(None);
    };
    let Some(source) = path.source.clone() else {
        return Ok(None);
    };
    let files = if materialized {
        Vec::new()
    } else {
        merge_git_path_files(&repo, path, latest, &tendi_settings, store, workspace_root)?
    };
    let materialized_files = if materialized {
        merge_materialized_git_path_files(
            &repo,
            path,
            latest,
            path.effective_visibility,
            store,
            workspace_root,
        )?
    } else {
        Vec::new()
    };
    let diff = if materialized {
        String::new()
    } else {
        git_path_diff(&repo, path, latest)
    };
    Ok(Some(GitUpdateAction {
        name: skill.name.clone(),
        skill_names: vec![skill.name.clone()],
        repo,
        source,
        source_ref: path.source_ref.clone(),
        current_version: update.current_version.clone(),
        latest_version: update.latest_version.clone(),
        diff,
        files,
        tendi_settings,
        materialized_targets: materialized
            .then(|| MaterializedGitTarget {
                name: skill.name.clone(),
                target: path.path.clone(),
                agent: path.agent,
                source_relative_path: path.source_relative_path.clone(),
                visibility: path.effective_visibility,
                uses_shared_layout: crate::providers::agent_provider(path.agent)
                    .uses_shared_skill_layout(),
                files: materialized_files,
            })
            .into_iter()
            .collect(),
    }))
}

fn git_tendi_settings(scan: &SkillScan, repo: &Path) -> Vec<GitSkillVisibility> {
    scan.skills
        .iter()
        .flat_map(|skill| skill.paths.iter())
        .filter_map(|path| {
            let visibility = path.tendi_visibility?;
            let skill_dir = path.path.canonicalize().ok()?;
            let skill_repo = git_repository_boundary(&skill_dir)?;
            (skill_repo == repo).then_some((skill_dir, (path.agent, visibility)))
        })
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .map(|(skill_dir, (agent, visibility))| GitSkillVisibility {
            skill_dir,
            agent,
            visibility,
        })
        .collect()
}

fn plan_registry_update(
    skill: &SkillRecord,
    path: &SkillPath,
    store: &crate::storage::Store,
    workspace_root: Option<&Path>,
    source_version: &str,
) -> Result<Option<RegistryUpdatePlan>> {
    let Some(source) = &path.source else {
        return Ok(None);
    };
    let Some(incoming) = read_registry_source(source) else {
        return Ok(None);
    };
    let skill_file = path.path.join("SKILL.md");
    let before = read_optional(&skill_file)?.context("SKILL.md does not exist")?;
    let snapshot = workspace_root.map_or_else(
        || store.skill_snapshot(&path.path),
        |workspace_root| store.skill_snapshot_for_workspace(workspace_root, &path.path),
    )?;
    let base = snapshot
        .filter(|snapshot| path.source_version.as_deref() == Some(snapshot.source_version.as_str()))
        .and_then(|snapshot| {
            snapshot
                .files
                .into_iter()
                .find(|file| file.relative_path == "SKILL.md")
        })
        .map(|file| String::from_utf8_lossy(&file.content).into_owned());
    let resolution_key = format!("{}:{}", skill.name, skill_file.display());
    let Some(base) = base else {
        return Ok(Some(RegistryUpdatePlan::Issue(SkillMergeIssue {
            name: skill.name.clone(),
            path: skill_file,
            resolution_key,
            status: "unavailable".to_string(),
            reason: Some(
                "The previous snapshot for this skill is unavailable, so Tendi cannot calculate a three-way merge."
                    .to_string(),
            ),
            before: before.clone(),
            base: String::new(),
            incoming,
            after: String::new(),
        })));
    };
    let (Some(local), Some(base), Some(incoming)) = normalize_skill_manifests_for_merge(
        Some(&before),
        Some(&base),
        Some(&incoming),
        path.agent,
        path.effective_visibility,
    ) else {
        return Ok(None);
    };
    let merged = merge_skill_manifest_text(Some(&base), Some(&local), Some(&incoming));
    let merged_status = merged.status.clone();
    let merged_content = merged.content.unwrap_or_default();
    if merged_status == "conflict" {
        return Ok(Some(RegistryUpdatePlan::Issue(SkillMergeIssue {
            name: skill.name.clone(),
            path: skill_file,
            resolution_key,
            status: merged_status,
            reason: merged.reason,
            before,
            base,
            incoming,
            after: merged_content,
        })));
    }
    Ok(Some(RegistryUpdatePlan::Change(
        FileChange {
            path: skill_file,
            before_sha256: Some(sha256_text(&before)),
            before: Some(before),
            after: merged_content,
        },
        SkillSourceUpdate {
            skill_path: path.path.clone(),
            source_version: source_version.to_string(),
        },
    )))
}

#[cfg(test)]
fn apply_git_update(action: &GitUpdateAction) -> Result<()> {
    apply_git_update_with_store(action, None)
}

fn apply_git_update_with_store(
    action: &GitUpdateAction,
    store: Option<&crate::storage::Store>,
) -> Result<()> {
    if !action.materialized_targets.is_empty() {
        return apply_materialized_git_update(action, store);
    }
    // The preview already contains the fetched and merged bytes. Applying that
    // immutable plan must neither contact the remote nor advance its revision.
    apply_update_files(&action.repo, &action.files)
}

fn apply_update_files(root: &Path, files: &[GitUpdateFile]) -> Result<()> {
    let mut applied = Vec::<(PathBuf, Option<Vec<u8>>)>::new();
    for file in files {
        let path = root.join(&file.path);
        ensure_path_inside(root, &path)?;
        let before = fs::read(&path).ok();
        if let Err(error) = validate_update_file(&path, file) {
            rollback_applied_files(&applied);
            return Err(error);
        }
        let result = if file.after_exists {
            if let Some(bytes) = &file.after_bytes {
                atomic_write_bytes(&path, bytes)
            } else {
                atomic_write(&path, &file.after)
            }
        } else if path.exists() {
            fs::remove_file(&path).with_context(|| format!("failed to remove {}", path.display()))
        } else {
            Ok(())
        };
        if let Err(error) = result {
            rollback_applied_files(&applied);
            return Err(error);
        }
        applied.push((path, before));
    }
    Ok(())
}

fn validate_update_file(path: &Path, file: &GitUpdateFile) -> Result<()> {
    let current = fs::read(path).ok();
    let current_exists = current.is_some();
    if current_exists != file.before_exists
        || current.as_deref().map(sha256_bytes) != file.before_bytes.as_deref().map(sha256_bytes)
    {
        bail!("refusing to overwrite changed file {}", path.display());
    }
    Ok(())
}

fn materialized_target_file(
    target: &MaterializedGitTarget,
    file: &GitUpdateFile,
) -> Result<PathBuf> {
    let repo_relative = target
        .source_relative_path
        .as_deref()
        .unwrap_or(".")
        .trim_end_matches("/SKILL.md")
        .trim_end_matches("SKILL.md")
        .trim_end_matches('/');
    let local_relative = if repo_relative.is_empty() || repo_relative == "." {
        PathBuf::from(&file.path)
    } else {
        Path::new(&file.path)
            .strip_prefix(repo_relative)
            .with_context(|| {
                format!(
                    "merged file {} is outside skill {}",
                    file.path, repo_relative
                )
            })?
            .to_path_buf()
    };
    let target_file = target.target.join(local_relative);
    ensure_path_inside(&target.target, &target_file)?;
    Ok(target_file)
}

fn apply_materialized_git_update(
    action: &GitUpdateAction,
    store: Option<&crate::storage::Store>,
) -> Result<()> {
    let latest = action
        .latest_version
        .as_deref()
        .context("materialized skill update has no remote revision")?;
    let logger = crate::logging::global();
    if logger.debug_enabled() {
        logger.debug(
            "materialized skill update started",
            serde_json::json!({
                "operation": "apply_materialized_git_update",
                "repo": &action.repo,
                "source": &action.source,
                "currentVersion": &action.current_version,
                "latestVersion": latest,
                "targetCount": action.materialized_targets.len(),
            }),
        );
    }
    for target in &action.materialized_targets {
        for file in &target.files {
            validate_update_file(&materialized_target_file(target, file)?, file)?;
        }
    }
    run_git(&action.repo, &["reset", "--hard", latest])?;
    let mut changes = Vec::new();
    for target in &action.materialized_targets {
        let relative = target
            .source_relative_path
            .as_deref()
            .unwrap_or(".")
            .trim_end_matches("/SKILL.md")
            .trim_end_matches("SKILL.md")
            .trim_end_matches('/');
        let source_dir = if relative.is_empty() || relative == "." {
            action.repo.clone()
        } else {
            action.repo.join(relative)
        };
        let target_root = target
            .target
            .parent()
            .context("materialized skill target has no parent")?;
        let target_name = target
            .target
            .file_name()
            .and_then(|name| name.to_str())
            .context("materialized skill target has no valid name")?;
        materialize_skill_dir_to_root(&source_dir, target_root, target_name, true, true, false)?;
        if !target.files.is_empty() {
            for file in &target.files {
                let target_file = materialized_target_file(target, file)?;
                if file.after_exists {
                    if let Some(bytes) = &file.after_bytes {
                        atomic_write_bytes(&target_file, bytes)?;
                    } else {
                        atomic_write(&target_file, &file.after)?;
                    }
                } else if target_file.exists() {
                    fs::remove_file(&target_file)
                        .with_context(|| format!("failed to remove {}", target_file.display()))?;
                }
            }
        }
        changes.extend(plan_skill_visibility_at_path(
            &target.target,
            target.agent,
            target.visibility,
            target.uses_shared_layout,
        )?);
    }
    apply_changes(&ChangeSet {
        changes: dedupe_changes(changes),
    })?;
    let mut records = Vec::new();
    if let Some(store) = store {
        for target in &action.materialized_targets {
            if let Some(mut record) = store.skill_source_record(&target.target)? {
                record.source_version = Some(latest.to_string());
                record.update_status = "tracked".to_string();
                records.push(record);
            }
        }
        store.upsert_skill_source_records(&records)?;
    }
    if logger.debug_enabled() {
        logger.debug(
            "materialized skill update completed",
            serde_json::json!({
                "operation": "apply_materialized_git_update",
                "repo": &action.repo,
                "latestVersion": latest,
                "targets": action
                    .materialized_targets
                    .iter()
                    .map(|target| {
                        serde_json::json!({
                            "path": &target.target,
                            "skillSha256": sha256_file(&target.target.join("SKILL.md")).ok(),
                        })
                    })
                    .collect::<Vec<_>>(),
            }),
        );
    }
    Ok(())
}

#[cfg(test)]
fn clear_tendi_git_changes(action: &GitUpdateAction) -> Result<()> {
    let status = git_output(&action.repo, &["status", "--porcelain"]).unwrap_or_default();
    if status.trim().is_empty() {
        return Ok(());
    }

    let mut managed = BTreeMap::new();
    for setting in &action.tendi_settings {
        let skill_file = setting.skill_dir.join("SKILL.md");
        let skill_relative = skill_file.strip_prefix(&action.repo).with_context(|| {
            format!(
                "{} is outside {}",
                skill_file.display(),
                action.repo.display()
            )
        })?;
        let skill_baseline = git_show_file(&action.repo, skill_relative)?;
        let skill_expected = render_skill_frontmatter_for_visibility(
            &skill_baseline,
            setting.agent,
            setting.visibility,
        )?;
        let skill_current =
            read_optional(&skill_file)?.context("SKILL.md disappeared before update")?;
        if skill_current != skill_expected {
            bail!(
                "refusing to update dirty git skill repo {}; {} has changes outside Tendi visibility settings",
                action.repo.display(),
                skill_file.display()
            );
        }
        if skill_current != skill_baseline {
            managed.insert(skill_relative.to_path_buf(), true);
        }

        let policy_file = crate::providers::codex::skill_policy_path(&setting.skill_dir);
        let policy_relative = policy_file.strip_prefix(&action.repo).with_context(|| {
            format!(
                "{} is outside {}",
                policy_file.display(),
                action.repo.display()
            )
        })?;
        let policy_baseline = git_show_optional_file(&action.repo, policy_relative)?;
        let policy_current = read_optional(&policy_file)?;
        if policy_current != policy_baseline {
            let matches_tendi_change = crate::providers::codex::policy_matches_visibility_change(
                policy_baseline.as_deref(),
                policy_current.as_deref(),
                setting.visibility,
            )
            .with_context(|| format!("failed to parse {}", policy_file.display()))?;
            if !matches_tendi_change {
                bail!(
                    "refusing to update dirty git skill repo {}; {} has changes outside Tendi visibility settings",
                    action.repo.display(),
                    policy_file.display()
                );
            }
            managed.insert(policy_relative.to_path_buf(), policy_baseline.is_some());
        }
    }

    let changed = git_output(&action.repo, &["diff", "--name-only"]).unwrap_or_default();
    for path in changed.lines().filter(|path| !path.is_empty()) {
        if !managed.contains_key(Path::new(path)) {
            bail!(
                "refusing to update dirty git skill repo {}; {} is not a Tendi-managed setting",
                action.repo.display(),
                path
            );
        }
    }
    let untracked = git_output(
        &action.repo,
        &["ls-files", "--others", "--exclude-standard"],
    )
    .unwrap_or_default();
    for path in untracked.lines().filter(|path| !path.is_empty()) {
        if !managed.contains_key(Path::new(path)) {
            bail!(
                "refusing to update git skill repo {}; {} is an untracked file",
                action.repo.display(),
                path
            );
        }
    }

    for (path, tracked) in managed {
        if tracked {
            let path = path.to_string_lossy().to_string();
            run_git(&action.repo, &["checkout", "--", &path])?;
        } else {
            let path = action.repo.join(path);
            if path.is_file() {
                fs::remove_file(&path)
                    .with_context(|| format!("failed to clear {}", path.display()))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn git_show_file(repo: &Path, path: &Path) -> Result<String> {
    let path = path.to_string_lossy();
    let output = git::run_git(
        repo,
        ["show".to_string(), format!("HEAD:{path}")],
        git::LOCAL_COMMAND_TIMEOUT,
        git::never_cancelled(),
    )
    .with_context(|| format!("failed to read {} from {}", path, repo.display()))?;
    if !output.status.success() {
        bail!("git show failed for {} in {}", path, repo.display());
    }
    String::from_utf8(output.stdout).context("git show returned non-UTF-8 skill content")
}

#[cfg(test)]
fn git_show_optional_file(repo: &Path, path: &Path) -> Result<Option<String>> {
    let path_text = path.to_string_lossy().to_string();
    let output = git::run_git(
        repo,
        [
            "ls-tree".to_string(),
            "--name-only".to_string(),
            "HEAD".to_string(),
            "--".to_string(),
            path_text.clone(),
        ],
        git::LOCAL_COMMAND_TIMEOUT,
        git::never_cancelled(),
    )
    .with_context(|| format!("failed to inspect {} in {}", path.display(), repo.display()))?;
    if !output.status.success() {
        bail!(
            "git ls-tree failed for {} in {}",
            path.display(),
            repo.display()
        );
    }
    let tracked = String::from_utf8(output.stdout)
        .context("git ls-tree returned non-UTF-8 paths")?
        .lines()
        .any(|candidate| candidate == path_text);
    tracked.then(|| git_show_file(repo, path)).transpose()
}

fn check_registry_update(skill: &SkillRecord, path: &SkillPath) -> SkillUpdateReport {
    let Some(source) = path.source.clone() else {
        return SkillUpdateReport {
            id: ensure_skill_record_id(skill),
            name: skill.name.clone(),
            status: "missing-source".to_string(),
            current_version: path.source_version.clone(),
            latest_version: None,
            source: None,
            source_kind: path.source_kind.clone(),
        };
    };

    let current = path
        .source_version
        .clone()
        .unwrap_or_else(|| path.sha256.clone());
    let latest = registry_latest_fingerprint(&source);
    let status = match &latest {
        Some(latest) if latest == &current => "up-to-date",
        Some(_) => "update-available",
        None => "unreachable",
    };

    SkillUpdateReport {
        id: ensure_skill_record_id(skill),
        name: skill.name.clone(),
        status: status.to_string(),
        current_version: Some(current),
        latest_version: latest,
        source: Some(source),
        source_kind: path.source_kind.clone(),
    }
}

#[cfg(test)]
fn fetch_git_remote_head(
    repo: &Path,
    source: &str,
    source_ref: Option<&str>,
    cancelled: &AtomicBool,
) -> Option<GitRemoteHead> {
    let head = resolve_git_remote_head(repo, source, source_ref, cancelled)?;
    if fetch_git_remote_commit(repo, source, &head.oid, &head.reference, cancelled) {
        Some(head)
    } else {
        None
    }
}

fn resolve_git_remote_head(
    repo: &Path,
    source: &str,
    source_ref: Option<&str>,
    cancelled: &AtomicBool,
) -> Option<GitRemoteHead> {
    let reference = format!(
        "refs/tendi/update-check/{}-{}",
        std::process::id(),
        GIT_UPDATE_CHECK_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let requested = source_ref.unwrap_or("HEAD");
    let started = Instant::now();
    let output = git::run_git(
        repo,
        [
            "ls-remote".to_string(),
            source.to_string(),
            requested.to_string(),
        ],
        git::NETWORK_COMMAND_TIMEOUT,
        cancelled,
    );
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            crate::logging::global().warn(
                "skill update git remote head failed",
                serde_json::json!({
                    "repo": repo,
                    "requested": requested,
                    "durationMs": started.elapsed().as_secs_f64() * 1000.0,
                    "error": error.to_string(),
                }),
            );
            return None;
        }
    };
    if !output.status.success() {
        crate::logging::global().warn(
            "skill update git remote head failed",
            serde_json::json!({
                "repo": repo,
                "requested": requested,
                "durationMs": started.elapsed().as_secs_f64() * 1000.0,
                "exitCode": output.status.code(),
            }),
        );
        return None;
    }

    let oid = String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .find_map(|line| {
            line.split_whitespace()
                .next()
                .filter(|value| !value.is_empty())
        })?
        .to_string();
    crate::logging::global().info(
        "skill update git remote head completed",
        serde_json::json!({
            "repo": repo,
            "requested": requested,
            "durationMs": started.elapsed().as_secs_f64() * 1000.0,
        }),
    );
    Some(GitRemoteHead { oid, reference })
}

fn fetch_git_remote_commit(
    repo: &Path,
    source: &str,
    oid: &str,
    reference: &str,
    cancelled: &AtomicBool,
) -> bool {
    let started = Instant::now();
    let result = run_git_fetch(
        repo,
        [
            "--no-write-fetch-head".to_string(),
            "--no-tags".to_string(),
            source.to_string(),
            format!("+{oid}:{reference}"),
        ],
        cancelled,
    );
    let success = result.is_ok();
    if success {
        crate::logging::global().info(
            "skill update git remote commit completed",
            serde_json::json!({
                "repo": repo,
                "durationMs": started.elapsed().as_secs_f64() * 1000.0,
            }),
        );
    } else if let Err(error) = &result {
        crate::logging::global().warn(
            "skill update git remote commit failed",
            serde_json::json!({
                "repo": repo,
                "durationMs": started.elapsed().as_secs_f64() * 1000.0,
                "error": error.to_string(),
            }),
        );
    }
    if !success {
        delete_git_ref(repo, reference);
    }
    success
}

pub(crate) fn is_full_git_revision(value: &str) -> bool {
    value.len() == 40 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

pub(crate) fn is_abbreviated_git_revision(value: &str) -> bool {
    (7..40).contains(&value.len()) && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn resolve_git_object(repo: &Path, object: &str) -> Option<String> {
    let output = git::run_git(
        repo,
        [
            "rev-parse".to_string(),
            "--verify".to_string(),
            format!("{object}^{{object}}"),
        ],
        git::LOCAL_COMMAND_TIMEOUT,
        git::never_cancelled(),
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let resolved = String::from_utf8(output.stdout).ok()?.trim().to_string();
    is_full_git_revision(&resolved).then_some(resolved)
}

fn git_object_available(repo: &Path, object: &str) -> bool {
    git::run_git(
        repo,
        ["cat-file".to_string(), "-e".to_string(), object.to_string()],
        git::LOCAL_COMMAND_TIMEOUT,
        git::never_cancelled(),
    )
    .is_ok_and(|output| output.status.success())
}

fn ensure_git_object_available(
    repo: &Path,
    source: &str,
    object: &str,
    cancelled: &AtomicBool,
) -> bool {
    if git_object_available(repo, object) {
        return true;
    }
    if !is_full_git_revision(object) {
        return false;
    }
    let reference = format!("refs/tendi/base/{object}");
    fetch_git_remote_commit(repo, source, object, &reference, cancelled)
        && git_object_available(repo, object)
}

pub(crate) fn resolve_git_source_version(record: &SkillSourceRecord) -> Option<String> {
    let object = record.source_version.as_deref()?;
    if !is_git_source_kind(&record.source_kind) || !is_abbreviated_git_revision(object) {
        return None;
    }
    let repo = git_repository_boundary(&record.skill_path).or_else(|| {
        let source = record.source.as_deref()?;
        git_checkout_for_source(source, record.source_ref.as_deref(), git::never_cancelled())
    })?;
    let source = record.source.as_deref()?;
    if !ensure_git_object_available_for_source_ref(
        &repo,
        source,
        record.source_ref.as_deref(),
        object,
        git::never_cancelled(),
    ) {
        return None;
    }
    resolve_git_object(&repo, object)
}

fn ensure_git_object_available_for_source_ref(
    repo: &Path,
    source: &str,
    source_ref: Option<&str>,
    object: &str,
    cancelled: &AtomicBool,
) -> bool {
    if git_object_available(repo, object) {
        return true;
    }
    if !is_full_git_revision(object) && !is_abbreviated_git_revision(object) {
        return false;
    }

    if is_full_git_revision(object) {
        let reference = format!("refs/tendi/base/{object}");
        return fetch_git_remote_commit(repo, source, object, &reference, cancelled)
            && git_object_available(repo, object);
    }

    let Some(remote_head) = resolve_git_remote_head(repo, source, source_ref, cancelled) else {
        return false;
    };
    let probe_reference = format!(
        "refs/tendi/base-probe/{}-{}",
        std::process::id(),
        GIT_UPDATE_CHECK_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let mut depth = 1usize;
    loop {
        if cancelled.load(Ordering::Acquire)
            || !fetch_git_remote_commit_at_depth(
                repo,
                source,
                &remote_head.oid,
                &probe_reference,
                depth,
                cancelled,
            )
        {
            delete_git_ref(repo, &probe_reference);
            return false;
        }

        if let Some(resolved) = resolve_git_object(repo, object) {
            let reference = format!("refs/tendi/base/{resolved}");
            let retained = update_git_ref(repo, &reference, &resolved);
            delete_git_ref(repo, &probe_reference);
            return retained && git_object_available(repo, &resolved);
        }
        if !git_repository_is_shallow(repo) {
            delete_git_ref(repo, &probe_reference);
            return false;
        }
        let Some(next_depth) = depth.checked_mul(2) else {
            delete_git_ref(repo, &probe_reference);
            return false;
        };
        depth = next_depth;
    }
}

fn update_git_ref(repo: &Path, reference: &str, object: &str) -> bool {
    let Ok(_resources) = git::mutation_resource_paths(repo)
        .and_then(|paths| crate::coordination::acquire_file_resources(&paths))
    else {
        return false;
    };
    git::run_git(
        repo,
        [
            "update-ref".to_string(),
            reference.to_string(),
            object.to_string(),
        ],
        git::LOCAL_COMMAND_TIMEOUT,
        git::never_cancelled(),
    )
    .is_ok_and(|output| output.status.success())
}

fn fetch_git_remote_commit_at_depth(
    repo: &Path,
    source: &str,
    oid: &str,
    reference: &str,
    depth: usize,
    cancelled: &AtomicBool,
) -> bool {
    let depth = git_repository_is_shallow(repo).then_some(depth);
    run_git_fetch_at_depth(
        repo,
        depth,
        [
            "--no-write-fetch-head".to_string(),
            "--no-tags".to_string(),
            source.to_string(),
            format!("+{oid}:{reference}"),
        ],
        cancelled,
    )
    .is_ok()
}

fn run_git_fetch(
    repo: &Path,
    arguments: impl IntoIterator<Item = String>,
    cancelled: &AtomicBool,
) -> Result<()> {
    let depth = git_repository_is_shallow(repo).then_some(1);
    run_git_fetch_at_depth(repo, depth, arguments, cancelled)
}

fn run_git_fetch_at_depth(
    repo: &Path,
    depth: Option<usize>,
    arguments: impl IntoIterator<Item = String>,
    cancelled: &AtomicBool,
) -> Result<()> {
    let _resources =
        crate::coordination::acquire_file_resources(&git::mutation_resource_paths(repo)?)?;
    let mut args = vec!["fetch".to_string()];
    if let Some(depth) = depth {
        args.push(format!("--depth={depth}"));
    }
    args.extend(arguments);
    let output = git::run_git(repo, args, git::NETWORK_COMMAND_TIMEOUT, cancelled)
        .with_context(|| format!("failed to run git fetch in {}", repo.display()))?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "git fetch failed in {}: {}",
        repo.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

pub(crate) fn is_git_source_kind(kind: &str) -> bool {
    matches!(kind, "git" | "github" | "gitlab" | "huggingface")
}

fn git_checkout_for_source(
    source: &str,
    source_ref: Option<&str>,
    cancelled: &AtomicBool,
) -> Option<PathBuf> {
    let cache_key = source_ref
        .as_deref()
        .map(|source_ref| format!("{source}#{source_ref}"))
        .unwrap_or_else(|| source.to_string());
    let repo = persistent_source_root(&cache_key).ok()?;
    let mut resources = git::mutation_resource_paths(&repo).ok()?;
    resources.push(repo.clone());
    let _resources = crate::coordination::acquire_file_resources(&resources).ok()?;
    if !repo.join(".git").is_dir() {
        if repo.exists() {
            fs::remove_dir_all(&repo).ok()?;
        }
        fs::create_dir_all(repo.parent()?).ok()?;
        run_git_clone(source, source_ref, &repo, cancelled).ok()?;
    }
    Some(repo)
}

fn git_checkout_for_skill_path(path: &SkillPath, cancelled: &AtomicBool) -> Option<PathBuf> {
    if let Some(repo) = git_repository_boundary(&path.path) {
        return Some(repo);
    }
    let source = path.source.as_deref()?;
    git_checkout_for_source(source, path.source_ref.as_deref(), cancelled)
}

fn normalized_skill_repo_path(path: &SkillPath) -> &str {
    path.source_relative_path
        .as_deref()
        .unwrap_or(".")
        .trim_end_matches("/SKILL.md")
        .trim_end_matches("SKILL.md")
        .trim_end_matches('/')
}

fn source_revision_matches(current: Option<&str>, latest: &str) -> bool {
    let Some(current) = current else {
        return false;
    };
    (7..=40).contains(&current.len())
        && current.chars().all(|ch| ch.is_ascii_hexdigit())
        && latest.starts_with(current)
}

fn cleanup_git_remote_heads(heads: BTreeMap<PathBuf, Option<GitRemoteHead>>) {
    for (repo, head) in heads {
        if let Some(head) = head {
            delete_git_ref(&repo, &head.reference);
        }
    }
}

fn delete_git_ref(repo: &Path, reference: &str) {
    let Ok(_resources) = git::mutation_resource_paths(repo)
        .and_then(|paths| crate::coordination::acquire_file_resources(&paths))
    else {
        return;
    };
    let _ = git::run_git(
        repo,
        ["update-ref", "-d", reference],
        git::LOCAL_COMMAND_TIMEOUT,
        git::never_cancelled(),
    );
}

fn git_diff_changed_paths(
    repo: &Path,
    remote_head: &str,
    pathspecs: &BTreeSet<String>,
    cancelled: &AtomicBool,
) -> Result<BTreeSet<String>, CommandFailure> {
    let mut args = vec![
        "diff".to_string(),
        "--name-only".to_string(),
        "HEAD".to_string(),
        remote_head.to_string(),
        "--".to_string(),
    ];
    args.extend(pathspecs.iter().cloned());
    let output = git::run_git(repo, args, git::LOCAL_COMMAND_TIMEOUT, cancelled)
        .map_err(|error| error.kind)?;
    if !output.status.success() {
        return Err(CommandFailure::Wait);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}

fn git_path_changed(
    changed_paths: &BTreeMap<PathBuf, Option<BTreeSet<String>>>,
    repo: &Path,
    path: &SkillPath,
) -> Option<bool> {
    let changed_paths = changed_paths.get(repo)?.as_ref()?;
    let relative = path.source_relative_path.as_deref().unwrap_or(".");
    Some(
        relative == "."
            || changed_paths.iter().any(|changed| {
                changed == relative
                    || changed
                        .strip_prefix(relative)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            }),
    )
}

fn git_path_diff(repo: &Path, path: &SkillPath, remote_head: &str) -> String {
    let relative = path.source_relative_path.as_deref().unwrap_or(".");
    let output = git::run_git(
        repo,
        [
            "diff",
            "--no-ext-diff",
            "--unified=3",
            "HEAD",
            remote_head,
            "--",
            relative,
        ],
        git::LOCAL_COMMAND_TIMEOUT,
        git::never_cancelled(),
    );
    output
        .ok()
        .filter(|output| output.status.success() || output.status.code() == Some(1))
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .unwrap_or_default()
}

#[cfg(test)]
fn git_materialized_path_files(
    repo: &Path,
    installed_dir: &Path,
    repo_relative: &str,
    remote_head: &str,
) -> Vec<GitUpdateFile> {
    let mut remote_files = BTreeSet::new();
    let mut args = vec!["ls-tree", "-r", "--name-only", remote_head];
    if !repo_relative.is_empty() && repo_relative != "." {
        args.extend(["--", repo_relative]);
    }
    if let Some(listing) = git_output(repo, &args) {
        remote_files.extend(listing.lines().map(str::to_string));
    }

    let mut local_files = BTreeSet::new();
    for entry in WalkDir::new(installed_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(installed_dir) else {
            continue;
        };
        let repo_file = if repo_relative.is_empty() || repo_relative == "." {
            relative.to_path_buf()
        } else {
            Path::new(repo_relative).join(relative)
        };
        local_files.insert(repo_file.to_string_lossy().replace('\\', "/"));
    }

    remote_files
        .union(&local_files)
        .filter_map(|repo_file| {
            let local_relative = if repo_relative.is_empty() || repo_relative == "." {
                PathBuf::from(repo_file)
            } else {
                Path::new(repo_file)
                    .strip_prefix(repo_relative)
                    .ok()?
                    .to_path_buf()
            };
            let local_path = installed_dir.join(local_relative);
            let before_exists = local_files.contains(repo_file);
            let before = fs::read(&local_path)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            let after = git_show_revision_file(repo, remote_head, repo_file);
            let after_exists = remote_files.contains(repo_file);
            if before_exists == after_exists && before == after {
                return None;
            }
            Some(GitUpdateFile {
                path: repo_file.clone(),
                resolution_key: repo_file.clone(),
                before_bytes: Some(before.as_bytes().to_vec()),
                incoming_bytes: Some(after.as_bytes().to_vec()),
                after_bytes: None,
                before,
                base: String::new(),
                incoming: after.clone(),
                after,
                before_exists,
                incoming_exists: after_exists,
                after_exists,
                status: "remote".to_string(),
                reason: None,
            })
        })
        .collect()
}

#[cfg(test)]
fn git_show_revision_file(repo: &Path, revision: &str, path: &str) -> String {
    let output = git::run_git(
        repo,
        ["show".to_string(), format!("{revision}:{path}")],
        git::LOCAL_COMMAND_TIMEOUT,
        git::never_cancelled(),
    );
    output
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
        .unwrap_or_default()
}

fn registry_latest_fingerprint(source: &str) -> Option<String> {
    let text = read_registry_source(source)?;
    let version = parse_frontmatter(&text).and_then(|value| {
        value
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    Some(version.unwrap_or_else(|| sha256_text(&text)))
}

fn read_registry_source(source: &str) -> Option<String> {
    if let Some(path) = source.strip_prefix("file://") {
        return fs::read_to_string(path).ok();
    }
    if source.starts_with('/') {
        return fs::read_to_string(source).ok();
    }
    if source.starts_with("http://") || source.starts_with("https://") {
        return ureq::get(source).call().ok()?.into_string().ok();
    }
    None
}

fn compact(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    value
        .chars()
        .take(max.saturating_sub(3))
        .collect::<String>()
        + "..."
}

pub fn format_changeset(changeset: &ChangeSet) -> String {
    if changeset.changes.is_empty() {
        return "no changes".to_string();
    }

    changeset
        .changes
        .iter()
        .map(|change| {
            let op = if change.before.is_some() { "M" } else { "A" };
            let preview = file_change_preview(change, 10);
            if preview.is_empty() {
                format!("{op} {}", change.path.display())
            } else {
                format!("{op} {}\n{}", change.path.display(), preview)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn file_change_preview(change: &FileChange, max_lines: usize) -> String {
    match &change.before {
        Some(before) => modified_preview(before, &change.after, max_lines),
        None => added_preview(&change.after, max_lines),
    }
}

fn added_preview(after: &str, max_lines: usize) -> String {
    let mut lines = after
        .lines()
        .take(max_lines)
        .map(|line| format!("  + {line}"))
        .collect::<Vec<_>>();
    if after.lines().count() > max_lines {
        lines.push("  ...".to_string());
    }
    lines.join("\n")
}

fn modified_preview(before: &str, after: &str, max_lines: usize) -> String {
    if before == after {
        return String::new();
    }

    let before_lines = before.lines().collect::<Vec<_>>();
    let after_lines = after.lines().collect::<Vec<_>>();
    let mut prefix = 0;
    while prefix < before_lines.len()
        && prefix < after_lines.len()
        && before_lines[prefix] == after_lines[prefix]
    {
        prefix += 1;
    }

    let mut before_suffix = before_lines.len();
    let mut after_suffix = after_lines.len();
    while before_suffix > prefix
        && after_suffix > prefix
        && before_lines[before_suffix - 1] == after_lines[after_suffix - 1]
    {
        before_suffix -= 1;
        after_suffix -= 1;
    }

    let context_start = prefix.saturating_sub(2);
    let context_end_before = (before_suffix + 2).min(before_lines.len());
    let context_end_after = (after_suffix + 2).min(after_lines.len());
    let mut lines = Vec::new();

    if context_start > 0 {
        lines.push("  ...".to_string());
    }
    for line in &before_lines[context_start..prefix] {
        lines.push(format!("    {line}"));
    }
    for line in &before_lines[prefix..before_suffix] {
        lines.push(format!("  - {line}"));
    }
    for line in &after_lines[prefix..after_suffix] {
        lines.push(format!("  + {line}"));
    }
    for line in &after_lines[after_suffix..context_end_after] {
        lines.push(format!("    {line}"));
    }
    if context_end_before < before_lines.len() || context_end_after < after_lines.len() {
        lines.push("  ...".to_string());
    }

    if lines.len() > max_lines {
        lines.truncate(max_lines);
        lines.push("  ...".to_string());
    }
    lines.join("\n")
}

fn matching_skills<'a>(scan: &'a SkillScan, pattern: &str) -> Vec<&'a SkillRecord> {
    scan.skills
        .iter()
        .filter(|skill| matches_pattern(&skill.name, pattern))
        .collect()
}

fn matches_pattern(value: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return value == pattern || value.contains(pattern);
    }

    let mut rest = value;
    for part in pattern.split('*').filter(|part| !part.is_empty()) {
        if let Some(index) = rest.find(part) {
            rest = &rest[index + part.len()..];
        } else {
            return false;
        }
    }
    true
}

fn plan_skill_frontmatter_for_agents(
    path: PathBuf,
    agents: &[AgentKind],
    visibility: SkillVisibility,
) -> Result<FileChange> {
    if visibility == SkillVisibility::Mixed {
        bail!("mixed visibility is a scan summary and cannot be written to SKILL.md");
    }
    let before = read_optional(&path)?.context("SKILL.md does not exist")?;
    let after = render_skill_frontmatter_for_agents(&before, agents, visibility)?;
    Ok(FileChange {
        path,
        before_sha256: Some(sha256_text(&before)),
        before: Some(before),
        after,
    })
}

fn render_skill_frontmatter_for_agents(
    before: &str,
    agents: &[AgentKind],
    visibility: SkillVisibility,
) -> Result<String> {
    if visibility == SkillVisibility::Mixed {
        bail!("mixed visibility is a scan summary and cannot be written to SKILL.md");
    }
    let mut after = before.to_string();
    let mut applied_agents = BTreeSet::new();
    for agent in agents {
        if !applied_agents.insert(*agent) {
            continue;
        }
        let doc = MarkdownDoc::parse_lenient(&after)?;
        let provider = crate::providers::agent_provider(*agent);
        let provider_key = provider.skill_frontmatter_visibility_key();
        if !provider_skill_frontmatter_satisfies(&doc.meta, visibility, provider_key) {
            after = render_provider_skill_frontmatter_with_provider_key_value(
                &after,
                visibility,
                provider_key,
                None,
            )?;
        }
    }
    Ok(after)
}

fn skill_visibility_frontmatter_agents(agent: AgentKind) -> Vec<AgentKind> {
    crate::providers::skill_frontmatter_visibility_providers(agent)
        .into_iter()
        .map(|provider| provider.kind())
        .collect()
}

fn plan_skill_visibility_at_path(
    skill_dir: &Path,
    agent: AgentKind,
    visibility: SkillVisibility,
    update_provider_config: bool,
) -> Result<Vec<FileChange>> {
    let frontmatter_agents = skill_visibility_frontmatter_agents(agent);
    let mut changes = Vec::new();
    let frontmatter_change = plan_skill_frontmatter_for_agents(
        skill_dir.join("SKILL.md"),
        &frontmatter_agents,
        visibility,
    )?;
    if frontmatter_change.before.as_deref() != Some(frontmatter_change.after.as_str()) {
        changes.push(frontmatter_change);
    }
    changes.extend(
        crate::providers::agent_provider(agent).plan_skill_visibility(
            skill_dir,
            visibility,
            update_provider_config,
        )?,
    );
    if matches!(
        agent,
        AgentKind::Cursor | AgentKind::Claude | AgentKind::Shared
    ) {
        changes.extend(
            crate::providers::agent_provider(AgentKind::Codex).plan_skill_visibility(
                skill_dir,
                visibility,
                update_provider_config,
            )?,
        );
    }
    Ok(changes)
}

#[cfg(test)]
fn render_skill_frontmatter_for_visibility(
    before: &str,
    agent: AgentKind,
    visibility: SkillVisibility,
) -> Result<String> {
    render_skill_frontmatter_for_agents(
        before,
        &skill_visibility_frontmatter_agents(agent),
        visibility,
    )
}

fn render_provider_skill_frontmatter_with_provider_key_value(
    before: &str,
    visibility: SkillVisibility,
    provider_key: Option<&str>,
    provider_value: Option<bool>,
) -> Result<String> {
    let mut doc = MarkdownDoc::parse_lenient(before)?;
    if let Some((yaml, tail, newline)) = split_frontmatter_raw(before) {
        let mut lines = yaml.lines().map(str::to_string).collect::<Vec<_>>();
        if let Some(provider_key) = provider_key {
            set_top_level_bool(
                &mut lines,
                provider_key,
                provider_value.unwrap_or(!matches!(visibility, SkillVisibility::Auto)),
            );
        }
        return Ok(format!(
            "---{newline}{}{newline}---{tail}",
            lines.join(newline)
        ));
    }

    if let Some(provider_key) = provider_key {
        let provider_key = Value::String(provider_key.to_string());
        if !provider_value.unwrap_or(!matches!(visibility, SkillVisibility::Auto)) {
            doc.meta.remove(&provider_key);
        } else {
            doc.meta.insert(provider_key, Value::Bool(true));
        }
    }
    doc.render()
}

fn provider_skill_frontmatter_satisfies(
    meta: &serde_yaml::Mapping,
    visibility: SkillVisibility,
    provider_key: Option<&str>,
) -> bool {
    let Some(provider_key) = provider_key else {
        return true;
    };
    let provider_disabled = meta
        .get(provider_key)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match visibility {
        SkillVisibility::Auto => !provider_disabled,
        SkillVisibility::Manual | SkillVisibility::Off => provider_disabled,
        SkillVisibility::Mixed => false,
    }
}

pub(crate) fn split_frontmatter_raw(text: &str) -> Option<(&str, &str, &'static str)> {
    let (newline, rest) = if let Some(rest) = text.strip_prefix("---\r\n") {
        ("\r\n", rest)
    } else if let Some(rest) = text.strip_prefix("---\n") {
        ("\n", rest)
    } else {
        return None;
    };
    let closing = format!("{newline}---");
    let end = rest.find(&closing)?;
    Some((&rest[..end], &rest[end + closing.len()..], newline))
}

fn set_top_level_bool(lines: &mut Vec<String>, key: &str, value: bool) {
    if !value {
        let prefix = format!("{key}:");
        lines.retain(|line| !line.starts_with(&prefix));
        return;
    }

    let rendered = format!("{key}: {value}");
    if let Some(line) = lines
        .iter_mut()
        .find(|line| line.starts_with(&format!("{key}:")))
    {
        *line = rendered;
        return;
    }
    lines.push(rendered);
}

pub(crate) struct MarkdownDoc {
    pub(crate) meta: serde_yaml::Mapping,
    body: String,
}

impl MarkdownDoc {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        if let Some((yaml, body, newline)) = split_frontmatter_raw(text) {
            let meta = serde_yaml::from_str::<serde_yaml::Mapping>(yaml)
                .context("failed to parse SKILL.md frontmatter")?;
            let body = body.strip_prefix(newline).unwrap_or(body).to_string();
            return Ok(Self { meta, body });
        }

        Ok(Self {
            meta: Default::default(),
            body: text.to_string(),
        })
    }

    fn parse_lenient(text: &str) -> Result<Self> {
        match Self::parse(text) {
            Ok(doc) => Ok(doc),
            Err(_) if split_frontmatter_raw(text).is_some() => {
                let meta = parse_frontmatter(text)
                    .and_then(|frontmatter| frontmatter.as_mapping().cloned())
                    .unwrap_or_default();
                Ok(Self {
                    meta,
                    body: text.to_string(),
                })
            }
            Err(err) => Err(err),
        }
    }

    pub(crate) fn render(&self) -> Result<String> {
        let newline = if self.body.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let yaml = serde_yaml::to_string(&self.meta)?.replace('\n', newline);
        Ok(format!("---{newline}{yaml}---{newline}{}", self.body))
    }
}

fn render_wrapper_skill(name: &str, skills: &[&SkillRecord]) -> String {
    render_wrapper_skill_with_description(name, skills, None)
}

fn render_wrapper_skill_with_description(
    name: &str,
    skills: &[&SkillRecord],
    description: Option<&str>,
) -> String {
    let generated_description;
    let description = match description.map(str::trim).filter(|value| !value.is_empty()) {
        Some(description) => description,
        None => {
            generated_description = generated_wrapper_description(name, skills);
            &generated_description
        }
    };
    let mut lines = vec![
        render_wrapper_frontmatter(name, description),
        format!("# {name}"),
        String::new(),
        "Route requests to selected child skills.".to_string(),
        String::new(),
    ];

    lines.push(render_wrapper_sections(name, skills));
    lines.push("## Procedure".to_string());
    lines.push(String::new());
    lines.push("1. Pick the best route.".to_string());
    lines.push("2. Open the selected route's `SKILL.md` link.".to_string());
    lines.push("3. Follow that skill's instructions.".to_string());
    lines.push(String::new());
    lines.join("\n")
}

fn render_wrapper_frontmatter(name: &str, description: &str) -> String {
    #[derive(Serialize)]
    struct WrapperFrontmatter<'a> {
        name: &'a str,
        description: &'a str,
    }

    let frontmatter = WrapperFrontmatter { name, description };
    let yaml = serde_yaml::to_string(&frontmatter).unwrap_or_else(|_| {
        "name: wrapper\ndescription: Route to selected child skills.\n".to_string()
    });
    format!("---\n{}---\n", yaml)
}

fn render_wrapper_after(name: &str, skills: &[&SkillRecord], before: Option<&str>) -> String {
    render_wrapper_after_with_description(name, skills, before, None)
}

fn render_wrapper_after_with_description(
    name: &str,
    skills: &[&SkillRecord],
    before: Option<&str>,
    description: Option<&str>,
) -> String {
    let sections = render_wrapper_sections(name, skills);
    let output = match before {
        Some(text) => replace_wrapper_route_section(text, &sections)
            .unwrap_or_else(|| format!("{}\n\n{}", text.trim_end(), sections)),
        None if description
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none() =>
        {
            render_wrapper_skill(name, skills)
        }
        None => render_wrapper_skill_with_description(name, skills, description),
    };
    match description.map(str::trim).filter(|value| !value.is_empty()) {
        Some(description) if before.is_some() => replace_wrapper_description(&output, description),
        _ => output,
    }
}

fn generated_wrapper_description(name: &str, skills: &[&SkillRecord]) -> String {
    let domain = name.replace(['-', '_'], " ");
    let child_names = skills
        .iter()
        .filter(|skill| skill.name != name)
        .map(|skill| skill.name.as_str())
        .take(4)
        .collect::<Vec<_>>();
    let child_summary = if child_names.is_empty() {
        "the selected child skills".to_string()
    } else {
        let remaining = skills
            .iter()
            .filter(|skill| skill.name != name)
            .count()
            .saturating_sub(child_names.len());
        let suffix = if remaining > 0 {
            format!(", and {remaining} more")
        } else {
            String::new()
        };
        format!("{}{}", child_names.join(", "), suffix)
    };
    format!(
        "Use when the request is about {domain} and matches one of these child skills: {child_summary}."
    )
}

fn replace_wrapper_description(text: &str, description: &str) -> String {
    let Some((yaml, tail, newline)) = split_frontmatter_raw(text) else {
        return text.to_string();
    };
    let mut lines = yaml.lines().map(str::to_string).collect::<Vec<_>>();
    set_top_level_string(&mut lines, "description", description);
    format!("---{newline}{}{newline}---{tail}", lines.join(newline))
}

fn set_top_level_string(lines: &mut Vec<String>, key: &str, value: &str) {
    let rendered = format!("{key}: {}", yaml_string_scalar(value));
    if let Some(line) = lines
        .iter_mut()
        .find(|line| line.starts_with(&format!("{key}:")))
    {
        *line = rendered;
        return;
    }
    lines.push(rendered);
}

fn yaml_string_scalar(value: &str) -> String {
    serde_yaml::to_string(value)
        .map(|yaml| yaml.trim_end().to_string())
        .unwrap_or_else(|_| format!("'{value}'"))
}

fn render_wrapper_sections(name: &str, skills: &[&SkillRecord]) -> String {
    let mut lines = vec!["## Route".to_string(), String::new()];
    lines.push(render_wrapper_route_block(name, skills));
    lines.join("\n")
}

fn render_wrapper_route_block(name: &str, skills: &[&SkillRecord]) -> String {
    let mut lines = vec![WRAPPER_CATALOG_START.to_string()];

    for skill in skills {
        if skill.name == name {
            continue;
        }
        let description = skill.description.as_deref().unwrap_or("");
        let skill_file = skill.paths.first().map(|path| path.path.join("SKILL.md"));
        let route = match skill_file {
            Some(path) => format!("[`{}`]({})", skill.name, markdown_link_destination(&path)),
            None => format!("`{}`", skill.name),
        };
        lines.push(format!("- {route}: {description}"));
    }

    lines.push(WRAPPER_CATALOG_END.to_string());
    lines.push(String::new());
    lines.join("\n")
}

fn markdown_link_destination(path: &Path) -> String {
    format!("<{}>", path.display())
}

struct WrapperRoute {
    name: String,
    path: Option<PathBuf>,
}

fn parse_wrapper_routes(text: &str) -> Vec<WrapperRoute> {
    let section = wrapper_route_xml_section(text).or_else(|| wrapper_route_section(text));
    let Some(section) = section else {
        return Vec::new();
    };
    let mut routes = Vec::new();
    for line in section.lines() {
        let line = line.trim_start();
        let Some(route) = line.strip_prefix("- ") else {
            continue;
        };
        if let Some(name) = parse_route_name(route) {
            routes.push(WrapperRoute {
                name,
                path: parse_route_path(route),
            });
        }
    }
    routes
}

fn parse_route_path(route: &str) -> Option<PathBuf> {
    let (_, destination) = route.split_once("](")?;
    let destination = destination
        .strip_prefix('<')
        .and_then(|value| value.split_once('>').map(|(path, _)| path))
        .or_else(|| destination.split_once(')').map(|(path, _)| path))?;
    (!destination.trim().is_empty() && !destination.contains("://"))
        .then(|| PathBuf::from(destination.trim()))
}

fn wrapper_route_xml_section(text: &str) -> Option<&str> {
    let (start, end) = wrapper_route_xml_range(text)?;
    Some(&text[start..end])
}

fn wrapper_route_xml_range(text: &str) -> Option<(usize, usize)> {
    xml_tag_range(text, WRAPPER_CATALOG_START, WRAPPER_CATALOG_END)
}

fn xml_tag_range(text: &str, start_tag: &str, end_tag: &str) -> Option<(usize, usize)> {
    let start = text.find(start_tag)?;
    let end = text[start..].find(end_tag)? + start + end_tag.len();
    Some((start, end))
}

fn wrapper_route_section(text: &str) -> Option<&str> {
    let headings = markdown_level2_headings(text);
    let route_index = headings
        .iter()
        .position(|heading| is_route_heading(&heading.title))?;
    let start = headings[route_index].start;
    let end = headings
        .get(route_index + 1)
        .map(|heading| heading.start)
        .unwrap_or(text.len());
    Some(&text[start..end])
}

fn replace_wrapper_route_section(text: &str, sections: &str) -> Option<String> {
    if let Some((start, end)) = wrapper_route_xml_range(text) {
        let block = render_wrapper_route_block_from_section(sections);
        return Some(replace_range(text, start, end, &block));
    }

    let headings = markdown_level2_headings(text);
    let route_index = headings
        .iter()
        .position(|heading| is_route_heading(&heading.title))?;
    let start = headings[route_index].start;
    let end = headings
        .get(route_index + 1)
        .map(|heading| heading.start)
        .unwrap_or(text.len());
    Some(replace_range(text, start, end, sections))
}

fn render_wrapper_route_block_from_section(section: &str) -> String {
    let route_lines = section
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("- ")
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut lines = vec![WRAPPER_CATALOG_START.to_string()];
    lines.extend(route_lines);
    lines.push(WRAPPER_CATALOG_END.to_string());
    lines.join("\n")
}

fn parse_route_name(route: &str) -> Option<String> {
    let route = route.trim();
    if let Some(rest) = route.strip_prefix("[`") {
        return rest
            .split_once("`]")
            .map(|(name, _)| name.trim().to_string())
            .filter(|name| !name.is_empty());
    }
    if let Some(rest) = route.strip_prefix('`') {
        return rest
            .split_once('`')
            .map(|(name, _)| name.trim().to_string())
            .filter(|name| !name.is_empty());
    }
    if let Some(rest) = route.strip_prefix('[') {
        return rest
            .split_once(']')
            .map(|(name, _)| name.trim().trim_matches('`').to_string())
            .filter(|name| !name.is_empty());
    }
    route
        .split_once(':')
        .map(|(name, _)| name.trim().trim_matches('`').to_string())
        .filter(|name| !name.is_empty())
}

fn replace_range(text: &str, start: usize, end: usize, replacement: &str) -> String {
    let mut out = String::new();
    out.push_str(text[..start].trim_end());
    out.push_str("\n\n");
    out.push_str(replacement.trim_end());
    out.push_str("\n");
    out.push_str(text[end..].trim_start_matches('\n'));
    out
}

fn is_route_heading(title: &str) -> bool {
    title.eq_ignore_ascii_case("route") || title.eq_ignore_ascii_case("routes")
}

#[derive(Debug)]
struct MarkdownHeading {
    start: usize,
    title: String,
}

fn markdown_level2_headings(text: &str) -> Vec<MarkdownHeading> {
    let mut headings = Vec::new();
    let mut offset = 0;

    for line in text.split_inclusive('\n') {
        if let Some(title) = line.strip_prefix("## ") {
            headings.push(MarkdownHeading {
                start: offset,
                title: title.trim().trim_end_matches('#').trim().to_string(),
            });
        }
        offset += line.len();
    }

    headings
}

pub(crate) fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

pub(crate) fn dedupe_changes(changes: Vec<FileChange>) -> Vec<FileChange> {
    let mut by_path = BTreeMap::new();
    for change in changes {
        if change.before.as_deref() == Some(change.after.as_str()) {
            continue;
        }
        let key = canonical_change_path(&change.path);
        by_path.entry(key).or_insert(change);
    }
    by_path.into_values().collect()
}

fn canonical_change_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut existing_ancestor = absolute.as_path();
    let mut missing_suffix = Vec::new();

    loop {
        if let Ok(canonical) = existing_ancestor.canonicalize() {
            return missing_suffix
                .iter()
                .rev()
                .fold(canonical, |resolved, part| resolved.join(part));
        }
        let Some(name) = existing_ancestor.file_name() else {
            return absolute;
        };
        missing_suffix.push(name.to_os_string());
        let Some(parent) = existing_ancestor.parent() else {
            return absolute;
        };
        existing_ancestor = parent;
    }
}

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;
