use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::fsutil::{atomic_write, atomic_write_bytes, sha256_bytes, sha256_file, sha256_text};
use crate::git::{self, CommandFailure};
use crate::skill_targets::{SkillInstallScope, SkillTarget, skill_target_root};

const WRAPPER_CATALOG_START: &str = "<catalog>";
const WRAPPER_CATALOG_END: &str = "</catalog>";
const MAX_CONCURRENT_GIT_FETCHES: usize = 8;
static GIT_UPDATE_CHECK_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const KEEP_LOCAL_RESOLUTION: &str = "__tendi_keep_local__";
const USE_UPDATE_RESOLUTION: &str = "__tendi_use_update__";

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize)]
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
    fn label(self) -> &'static str {
        match self {
            SkillVisibility::Auto => "auto",
            SkillVisibility::Manual => "manual",
            SkillVisibility::Off => "off",
            SkillVisibility::Mixed => "mixed",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
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
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub dependencies: Vec<String>,
    pub dependents: Vec<String>,
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

#[derive(Debug, Clone, Serialize)]
pub struct SkillScan {
    pub roots: Vec<SkillRoot>,
    pub skills: Vec<SkillRecord>,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub struct SkillScanWithSourceMigrations {
    pub scan: SkillScan,
    pub source_migrations: Vec<SkillSourceRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangeSet {
    pub changes: Vec<FileChange>,
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

#[derive(Debug, Clone)]
pub struct SkillUpdatePersistence {
    pub source_records: Vec<SkillSourceRecord>,
    pub snapshots: Vec<SkillSnapshot>,
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
    Ok(scan_skills_with_source_store_for_projects_for_projection(cwd, store, project_roots)?.scan)
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
    Ok(
        scan_skills_synced_for_project_roots_with_store_for_projection(
            cwd,
            store,
            project_roots,
        )?
        .scan,
    )
}

pub fn scan_skills_synced_for_project_roots_with_store_for_projection(
    cwd: &Path,
    store: &crate::storage::Store,
    project_roots: &[PathBuf],
) -> Result<SkillScanWithSourceMigrations> {
    let result = scan_skills_with_source_store_for_projects_for_projection(
        cwd,
        store,
        project_roots,
    )?;
    let scan = result.scan;
    let source_migrations = result.source_migrations;
    let changeset = plan_wrapper_sync(&scan)?;
    if changeset.changes.is_empty() {
        return Ok(SkillScanWithSourceMigrations {
            scan,
            source_migrations,
        });
    }
    apply_changes(&changeset)?;
    scan_skills_with_source_store_for_projects_for_projection(cwd, store, project_roots)
}

fn scan_skills_with_source_store(cwd: &Path, store: &crate::storage::Store) -> Result<SkillScan> {
    scan_skills_with_source_store_for_projects(cwd, store, &[])
}

fn scan_skills_with_source_store_for_projects(
    cwd: &Path,
    store: &crate::storage::Store,
    project_roots: &[PathBuf],
) -> Result<SkillScan> {
    Ok(scan_skills_with_source_store_for_projects_for_projection(cwd, store, project_roots)?.scan)
}

fn scan_skills_with_source_store_for_projects_for_projection(
    cwd: &Path,
    store: &crate::storage::Store,
    project_roots: &[PathBuf],
) -> Result<SkillScanWithSourceMigrations> {
    let source_records = store.skill_source_records()?;
    let mut provenance_resolver = ProvenanceResolver::managed(cwd, source_records);
    let scan =
        scan_skills_with_resolver_for_projects(cwd, &mut provenance_resolver, project_roots)?;
    Ok(SkillScanWithSourceMigrations {
        scan,
        source_migrations: provenance_resolver.migrations,
    })
}

pub fn scan_skills_synced_for_projection(
    cwd: &Path,
) -> Result<SkillScanWithSourceMigrations> {
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
    let mut by_name: BTreeMap<String, Vec<RawSkill>> = BTreeMap::new();
    for skill in raw_skills {
        by_name.entry(skill.name.clone()).or_default().push(skill);
    }

    let mut skills = by_name
        .into_iter()
        .map(|(name, raws)| merge_skill(name, raws))
        .collect::<Vec<_>>();
    resolve_scanned_skill_relations(&mut skills);
    warnings.append(&mut provenance_resolver.lock_warnings);

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
    let scan = scan_skills(cwd)?;
    let changeset = plan_wrapper_sync(&scan)?;
    if changeset.changes.is_empty() {
        return Ok(scan);
    }
    apply_changes(&changeset)?;
    scan_skills(cwd)
}

pub fn refresh_skill_scan(
    _cwd: &Path,
    scan: &SkillScan,
    names: &[String],
    extra_skill_dirs: &[PathBuf],
) -> Result<SkillScan> {
    let names = names.iter().cloned().collect::<BTreeSet<_>>();
    let mut refresh_dirs = extra_skill_dirs.iter().cloned().collect::<BTreeSet<_>>();
    let mut roots_by_dir = BTreeMap::<PathBuf, SkillRoot>::new();
    let mut remaining = Vec::new();

    for skill in &scan.skills {
        if names.contains(&skill.name)
            || skill
                .paths
                .iter()
                .any(|path| refresh_dirs.contains(&path.path))
        {
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
            remaining.push(skill.clone());
        }
    }

    let mut provenance_resolver = ProvenanceResolver::from_scan(scan);
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
        let root = root.unwrap_or_else(|| SkillRoot {
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

    let mut refreshed_by_name = BTreeMap::<String, Vec<RawSkill>>::new();
    for skill in refreshed_raw {
        refreshed_by_name
            .entry(skill.name.clone())
            .or_default()
            .push(skill);
    }
    remaining.extend(
        refreshed_by_name
            .into_iter()
            .map(|(name, raws)| merge_skill(name, raws)),
    );
    remaining.sort_by(|left, right| left.name.cmp(&right.name));
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

    Ok(SkillScan {
        roots: scan.roots.clone(),
        skills: remaining,
        warnings: scan.warnings.clone(),
    })
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

pub fn plan_visibility_many(
    cwd: &Path,
    names: &[String],
    visibility: SkillVisibility,
) -> Result<ChangeSet> {
    let scan = scan_skills(cwd)?;
    plan_visibility_many_for_scan(&scan, names, visibility)
}

pub fn plan_visibility_many_for_scan(
    scan: &SkillScan,
    names: &[String],
    visibility: SkillVisibility,
) -> Result<ChangeSet> {
    let matches = scan
        .skills
        .iter()
        .filter(|skill| names.iter().any(|name| name == &skill.name))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        bail!("no skills matched selected names");
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

pub fn plan_wrapper_from_names(
    cwd: &Path,
    name: &str,
    names: &[String],
    description: Option<&str>,
    manual_children: bool,
) -> Result<ChangeSet> {
    let scan = scan_skills(cwd)?;
    plan_wrapper_from_names_for_scan(&scan, name, names, description, manual_children)
}

pub fn plan_wrapper_from_names_for_scan(
    scan: &SkillScan,
    name: &str,
    names: &[String],
    description: Option<&str>,
    manual_children: bool,
) -> Result<ChangeSet> {
    let matches = scan
        .skills
        .iter()
        .filter(|skill| names.iter().any(|selected| selected == &skill.name))
        .collect::<Vec<_>>();
    plan_wrapper_for_matches(scan, name, matches, description, manual_children)
}
pub fn refresh_wrapper_from_names(
    cwd: &Path,
    name: &str,
    names: &[String],
    manual_children: bool,
) -> Result<ChangeSet> {
    let scan = scan_skills(cwd)?;
    refresh_wrapper_from_names_for_scan(&scan, name, names, manual_children)
}

pub fn refresh_wrapper_from_names_for_scan(
    scan: &SkillScan,
    name: &str,
    names: &[String],
    manual_children: bool,
) -> Result<ChangeSet> {
    let wrapper_exists = scan.skills.iter().any(|skill| skill.name == name);
    if !wrapper_exists {
        bail!("wrapper skill {name:?} does not exist");
    }
    let matches = scan
        .skills
        .iter()
        .filter(|skill| skill.name != name && names.iter().any(|selected| selected == &skill.name))
        .collect::<Vec<_>>();
    plan_wrapper_for_matches(&scan, name, matches, None, manual_children)
}

pub fn plan_skill_delete_many(cwd: &Path, names: &[String]) -> Result<SkillDeletePlan> {
    let scan = scan_skills(cwd)?;
    plan_skill_delete_many_for_scan(&scan, names)
}

pub fn plan_skill_delete_many_for_scan(
    scan: &SkillScan,
    names: &[String],
) -> Result<SkillDeletePlan> {
    let matches = scan
        .skills
        .iter()
        .filter(|skill| names.iter().any(|selected| selected == &skill.name))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        bail!("no skills matched selected names");
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

    let selected_names = matches
        .iter()
        .map(|skill| skill.name.clone())
        .collect::<BTreeSet<_>>();
    let dependencies = matches
        .iter()
        .filter_map(|skill| {
            let related = skill
                .dependencies
                .iter()
                .filter(|name| !selected_names.contains(*name))
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
                .dependents
                .iter()
                .filter(|name| !selected_names.contains(*name))
                .cloned()
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

pub fn apply_skill_delete_plan(plan: &SkillDeletePlan) -> Result<()> {
    for target in &plan.targets {
        let metadata = fs::symlink_metadata(&target.path)
            .with_context(|| format!("failed to inspect {}", target.path.display()))?;
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
    for change in &changeset.changes {
        let current = read_optional(&change.path)?;
        match (&change.before_sha256, &current) {
            (Some(expected), Some(text)) if *expected == sha256_text(text) => {}
            (Some(_), Some(_)) => bail!(
                "refusing to overwrite changed file {}",
                change.path.display()
            ),
            (Some(_), None) => bail!(
                "refusing to overwrite missing file {}",
                change.path.display()
            ),
            (None, None) => {}
            (None, Some(_)) => bail!(
                "refusing to overwrite existing file {}",
                change.path.display()
            ),
        }
        atomic_write(&change.path, &change.after)?;
    }
    Ok(())
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
    let source = source
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", source.display()))?;
    if !source.join("SKILL.md").is_file() {
        bail!("{} is not a skill directory", source.display());
    }

    let skill_name = name
        .map(str::to_string)
        .or_else(|| {
            source
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::to_string)
        })
        .context("could not infer skill name")?;
    let target_root = skill_target_root(cwd, target, scope)?;
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
    })
}

pub fn apply_skill_distribution_plan(plan: &SkillDistributionPlan) -> Result<MaterializeResult> {
    if plan.status == "already-at-destination"
        || (plan.status == "already-installed" && plan.mode == SkillDistributionMode::Symlink)
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
    if plan.status == "conflict" || plan.destination_exists {
        bail!("target already exists: {}", plan.destination.display());
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
    let target_root = plan
        .destination
        .parent()
        .context("skill distribution destination has no parent")?;
    fs::create_dir_all(target_root)?;

    match plan.mode {
        SkillDistributionMode::Move => {
            if plan.source_symlink {
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
                fs::rename(&plan.source, &plan.destination).with_context(|| {
                    format!(
                        "failed to move {} to {}",
                        plan.source.display(),
                        plan.destination.display()
                    )
                })?;
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
            create_symlink(&source, &plan.destination).with_context(|| {
                format!(
                    "failed to link {} to {}",
                    source.display(),
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
    let source = source
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", source.display()))?;
    if !source.join("SKILL.md").is_file() {
        bail!("{} is not a skill directory", source.display());
    }
    let skill_name = sanitize_skill_dir_name(name)?;
    let target = target_root.join(&skill_name);
    ensure_path_inside(target_root, &target)?;
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if !copy && target.canonicalize().ok().as_ref() == Some(&source) {
            return Ok(MaterializeResult {
                source,
                target,
                mode: "symlink".to_string(),
                health: "symlink-ok".to_string(),
                applied: false,
            });
        }
        if !overwrite {
            bail!("target already exists: {}", target.display());
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
        return Ok(MaterializeResult {
            source,
            target,
            mode: "copy".to_string(),
            health: "copy-ok".to_string(),
            applied: true,
        });
    }

    match create_symlink(&source, &target) {
        Ok(()) if target.join("SKILL.md").is_file() => Ok(MaterializeResult {
            source,
            target,
            mode: "symlink".to_string(),
            health: "symlink-ok".to_string(),
            applied: true,
        }),
        _ => {
            let _ = fs::remove_file(&target);
            copy_dir(&source, &target)?;
            Ok(MaterializeResult {
                source,
                target,
                mode: "copy".to_string(),
                health: "copy-fallback".to_string(),
                applied: true,
            })
        }
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
    if preview.target != options.target
        || preview.scope != options.scope
        || preview.mode != if options.copy { "copy" } else { "symlink" }
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

fn apply_built_skill_add_plan(
    plan: SkillAddPlan,
    options: &SkillAddOptions,
    target_root: &Path,
) -> Result<SkillAddApplyReport> {
    let mut results = Vec::new();
    let mut visibility_changes = Vec::new();
    for skill in &plan.selected {
        let result = materialize_skill_dir_to_root(
            &skill.path,
            target_root,
            &skill.name,
            options.copy,
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

    let selected = expand_installable_dependencies(
        &available,
        select_installable_skills(&available, &options.skills)?,
    );
    let mode = if options.copy { "copy" } else { "symlink" }.to_string();
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
    available: &[InstallableSkill],
    names: &[String],
) -> Result<Vec<InstallableSkill>> {
    if names.is_empty() || names.iter().any(|name| name == "*") {
        return Ok(available.to_vec());
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

    Ok(selected)
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
    let mut expanded = Vec::new();
    let mut stack = selected
        .into_iter()
        .map(|skill| skill.name)
        .collect::<Vec<_>>();

    while let Some(name) = stack.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(skill) = by_name.get(name.as_str()) else {
            continue;
        };
        expanded.push((*skill).clone());
        for dependency in skill.dependencies.iter().rev() {
            if !seen.contains(dependency) {
                stack.push(dependency.clone());
            }
        }
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

fn parse_github_shorthand(source: &str) -> Option<(String, String)> {
    if source.contains("://") || source.contains('@') {
        return None;
    }
    let mut parts = source.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next().is_some() || owner.is_empty() || repo.is_empty() {
        return None;
    }
    if !owner
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
        || !repo
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return None;
    }
    Some((owner.to_string(), repo.trim_end_matches(".git").to_string()))
}

fn persistent_source_root(source: &str) -> Result<PathBuf> {
    let db_path = crate::storage::default_db_path()?;
    let data_dir = db_path
        .parent()
        .context("tendi database path did not have a parent directory")?;
    let hash = short_sha(source, 12);
    Ok(data_dir
        .join("sources")
        .join(format!("{}-{hash}", sanitize_skill_dir_name(source)?)))
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
    let mut args = vec!["clone".to_string(), "--depth".to_string(), "1".to_string()];
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
        for entry in WalkDir::new(&root)
            .follow_links(false)
            .max_depth(5)
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
    plan_skill_updates_for_scan(&scan, pattern)
}

pub fn plan_skill_updates_for_scan(scan: &SkillScan, pattern: &str) -> Result<SkillUpdatePlan> {
    let store = crate::storage::Store::open_default()?;
    let matches = matching_skills(&scan, pattern);
    if matches.is_empty() {
        bail!("no skills matched pattern {pattern:?}");
    }

    plan_skill_updates_for_matches(&scan, matches, &store)
}

pub fn plan_skill_updates_many(cwd: &Path, names: &[String]) -> Result<SkillUpdatePlan> {
    let scan = scan_skills(cwd)?;
    plan_skill_updates_many_for_scan(&scan, names)
}

pub fn plan_skill_updates_many_for_scan(
    scan: &SkillScan,
    names: &[String],
) -> Result<SkillUpdatePlan> {
    let store = crate::storage::Store::open_default()?;
    let matches = scan
        .skills
        .iter()
        .filter(|skill| names.iter().any(|selected| selected == &skill.name))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        bail!("no skills matched update selection");
    }

    plan_skill_updates_for_matches(&scan, matches, &store)
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
) -> Result<SkillUpdatePlan> {
    let mut file_changes = Vec::new();
    let mut git_updates = BTreeMap::new();
    let mut skipped = Vec::new();
    let mut source_updates = Vec::new();
    let mut merge_issues = Vec::new();
    let updates_by_name = check_skill_updates_for_skills(&matches, git::never_cancelled())
        .into_iter()
        .map(|update| (update.name.clone(), update))
        .collect::<BTreeMap<_, _>>();

    for skill in matches {
        let update = updates_by_name
            .get(&skill.name)
            .cloned()
            .context("skill update report missing for selected skill")?;
        if update.status != "update-available" {
            skipped.push(update);
            continue;
        }

        let Some(path) = skill
            .paths
            .iter()
            .find(|path| path.update_status == "checkable")
            .or_else(|| skill.paths.first())
        else {
            skipped.push(update);
            continue;
        };
        let Some(source_version) = update.latest_version.clone() else {
            skipped.push(update);
            continue;
        };

        match path.source_kind.as_str() {
            "git" | "github" | "gitlab" | "huggingface" => {
                if let Some(action) = plan_git_update(scan, skill, path, &update, store) {
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
            "registry" => match plan_registry_update(skill, path, store, &source_version)? {
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
            },
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

pub fn apply_skill_update_plan(plan: &SkillUpdatePlan) -> Result<()> {
    let store = crate::storage::Store::open_default()?;
    apply_skill_update_plan_with_store(plan, &store)
}

pub fn apply_skill_update_plan_with_store(
    plan: &SkillUpdatePlan,
    store: &crate::storage::Store,
) -> Result<()> {
    let plan = prepare_skill_update_plan_with_resolutions(plan, &BTreeMap::new())?;
    apply_skill_update_plan_filesystem(&plan)?;
    persist_skill_update_plan(store, &plan)
}

pub fn apply_skill_update_plan_with_resolutions(
    plan: &SkillUpdatePlan,
    store: &crate::storage::Store,
    resolutions: &BTreeMap<String, String>,
) -> Result<()> {
    let plan = prepare_skill_update_plan_with_resolutions(plan, resolutions)?;
    apply_skill_update_plan_filesystem(&plan)?;
    persist_skill_update_plan(store, &plan)
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
    apply_changes(&plan.file_changes)?;
    for action in &plan.git_updates {
        apply_git_update_with_store(action, None)?;
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
    let source_records = updated_skill_source_records(store, &source_updates)?;
    let snapshots = capture_skill_snapshots(&source_records)?;
    Ok(SkillUpdatePersistence {
        source_records,
        snapshots,
    })
}

pub fn persist_skill_update_persistence(
    store: &crate::storage::Store,
    persistence: &SkillUpdatePersistence,
) -> Result<()> {
    store.upsert_skill_source_records(&persistence.source_records)?;
    store.replace_skill_snapshots(&persistence.snapshots)?;
    Ok(())
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

fn updated_skill_source_records(
    store: &crate::storage::Store,
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
    let mut records = store
        .skill_source_records()?
        .into_iter()
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
            after: content.clone(),
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
    if file.status == "binary" {
        match content.as_str() {
            KEEP_LOCAL_RESOLUTION => {
                file.after = String::new();
                file.after_bytes = file.before_bytes.clone();
                file.after_exists = file.before_exists;
                file.status = "resolved-local".to_string();
            }
            USE_UPDATE_RESOLUTION => {
                file.after = String::new();
                file.after_bytes = file.incoming_bytes.clone();
                file.after_exists = file.incoming_exists;
                file.status = "resolved-remote".to_string();
            }
            _ => {}
        }
        return;
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
    let skills_by_name = scan
        .skills
        .iter()
        .map(|skill| (skill.name.as_str(), skill))
        .collect::<BTreeMap<_, _>>();
    let mut changes = Vec::new();

    for wrapper in scan
        .skills
        .iter()
        .filter(|skill| skill.tags.iter().any(|tag| tag == "wrapper"))
    {
        for path in &wrapper.paths {
            if !path.tags.iter().any(|tag| tag == "wrapper") {
                continue;
            }
            let wrapper_file = path.path.join("SKILL.md");
            let Some(before) = read_optional(&wrapper_file)? else {
                continue;
            };
            let child_names = parse_wrapper_route_names(&before);
            if child_names.is_empty() {
                continue;
            }
            let children = child_names
                .iter()
                .filter_map(|name| skills_by_name.get(name.as_str()).copied())
                .filter(|skill| skill.name != wrapper.name)
                .collect::<Vec<_>>();
            if children.is_empty() {
                continue;
            }
            let after = render_wrapper_after(&wrapper.name, &children, Some(&before));
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

    let tendi_visibility = frontmatter.as_ref().and_then(parse_tendi_visibility);
    let provider = crate::providers::agent_provider(root.agent);
    let metadata =
        provider.skill_visibility_metadata(skill_dir, skill_file, frontmatter.as_ref())?;
    let effective_visibility = provider.effective_skill_visibility(
        tendi_visibility,
        metadata.provider_visibility,
        root,
    );
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
            provider_allow_implicit_invocation: metadata.allow_implicit_invocation,
            provider_skill_enabled: metadata.enabled,
            provider_disable_model_invocation: metadata.disable_model_invocation,
            plugin_id: root.plugin_id.clone(),
            plugin_enabled: root.plugin_enabled,
        },
    })
}

fn parse_frontmatter(text: &str) -> Option<Value> {
    let rest = text.strip_prefix("---\n")?;
    let (yaml, _) = rest.split_once("\n---")?;
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
        .map(|(_, tail)| tail.strip_prefix('\n').unwrap_or(tail))
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

fn parse_tendi_visibility(frontmatter: &Value) -> Option<SkillVisibility> {
    let value = frontmatter
        .get("tendi")
        .and_then(|tendi| tendi.get("visibility"))
        .or_else(|| frontmatter.get("tendi.visibility"))
        .and_then(Value::as_str)?;

    match value.to_ascii_lowercase().as_str() {
        "auto" => Some(SkillVisibility::Auto),
        "manual" => Some(SkillVisibility::Manual),
        "off" => Some(SkillVisibility::Off),
        _ => None,
    }
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
    if provider_visibility == SkillVisibility::Off
        || tendi_visibility == Some(SkillVisibility::Off)
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

fn merge_skill(name: String, raws: Vec<RawSkill>) -> SkillRecord {
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

    SkillRecord {
        name,
        description,
        tags: tags.into_iter().collect(),
        dependencies: dependencies.into_iter().collect(),
        dependents: Vec::new(),
        visibility,
        agents: agents.into_iter().collect(),
        paths,
        source_summary: source_summaries
            .into_iter()
            .next()
            .unwrap_or_default(),
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
        .is_none_or(|value| candidate.as_str() > value)
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
    let by_normalized_name = skills
        .iter()
        .map(|skill| (normalize_skill_match_name(&skill.name), skill.name.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut dependents_by_name: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for skill in skills.iter_mut() {
        let own_name = normalize_skill_match_name(&skill.name);
        let mut seen = BTreeSet::new();
        skill.dependencies = skill
            .dependencies
            .iter()
            .filter_map(|dependency| {
                let dependency_name =
                    by_normalized_name.get(&normalize_skill_match_name(dependency))?;
                (normalize_skill_match_name(dependency_name) != own_name
                    && seen.insert(dependency_name.clone()))
                .then(|| dependency_name.clone())
            })
            .collect();
        for dependency in &skill.dependencies {
            dependents_by_name
                .entry(dependency.clone())
                .or_default()
                .insert(skill.name.clone());
        }
    }

    for skill in skills {
        skill.dependents = dependents_by_name
            .remove(&skill.name)
            .map(|names| names.into_iter().collect())
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

#[derive(Debug, Default, Deserialize)]
struct SkillsCliLockFile {
    version: u64,
    #[serde(default)]
    skills: BTreeMap<String, SkillsCliLockEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillsCliLockEntry {
    source: String,
    source_type: String,
    source_url: Option<String>,
    r#ref: Option<String>,
    skill_path: Option<String>,
    skill_folder_hash: Option<String>,
    computed_hash: Option<String>,
}

#[derive(Debug, Default)]
struct SkillsCliLockDatabase {
    global: BTreeMap<String, SkillsCliLockEntry>,
    projects: Vec<(PathBuf, BTreeMap<String, SkillsCliLockEntry>)>,
}

#[derive(Debug, Default)]
struct ProvenanceResolver {
    repositories: BTreeMap<PathBuf, Option<GitRepositoryProvenance>>,
    source_records: BTreeMap<PathBuf, SkillSourceRecord>,
    lock_cwd: Option<PathBuf>,
    skills_cli_locks: Option<SkillsCliLockDatabase>,
    lock_warnings: Vec<String>,
    migrations: Vec<SkillSourceRecord>,
}

impl ProvenanceResolver {
    fn managed(cwd: &Path, records: Vec<SkillSourceRecord>) -> Self {
        Self {
            source_records: records
                .into_iter()
                .map(|record| (record.skill_path.clone(), record))
                .collect(),
            lock_cwd: Some(cwd.to_path_buf()),
            ..Self::default()
        }
    }

    fn from_scan(scan: &SkillScan) -> Self {
        let source_records = scan
            .skills
            .iter()
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

    #[cfg(test)]
    fn infer(&mut self, skill_dir: &Path, frontmatter: &Option<Value>) -> Provenance {
        if let Some(provenance) = frontmatter_provenance(frontmatter) {
            return provenance;
        }

        if let Some(repository) = self.repository_for(skill_dir) {
            return repository_provenance(skill_dir, repository);
        }

        local_provenance(skill_dir)
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

        if let Some(provenance) = self.lock_provenance(name, scope, installed_dir) {
            self.migrations.push(SkillSourceRecord::from_provenance(
                name,
                installed_dir,
                &provenance,
                "skills-cli-lock",
            ));
            return provenance;
        }

        repository
            .map(|repository| repository_provenance(provenance_dir, repository))
            .unwrap_or_else(|| local_provenance(provenance_dir))
    }

    fn lock_provenance(
        &mut self,
        name: &str,
        scope: &str,
        installed_dir: &Path,
    ) -> Option<Provenance> {
        if self.skills_cli_locks.is_none() {
            let cwd = self.lock_cwd.as_deref()?;
            let (locks, mut warnings) = SkillsCliLockDatabase::load(cwd);
            self.skills_cli_locks = Some(locks);
            self.lock_warnings.append(&mut warnings);
        }
        self.skills_cli_locks
            .as_ref()?
            .provenance(name, scope, installed_dir)
    }

    fn repository_for(&mut self, skill_dir: &Path) -> Option<GitRepositoryProvenance> {
        let candidate = git_repository_boundary(skill_dir)?;
        self.repositories
            .entry(candidate.clone())
            .or_insert_with(|| {
                Some(GitRepositoryProvenance {
                    remote: git_output(&candidate, &["config", "--get", "remote.origin.url"]),
                    head: git_output(&candidate, &["rev-parse", "--short", "HEAD"]),
                    root: candidate.clone(),
                })
            })
            .clone()
    }
}

impl SkillSourceRecord {
    fn from_provenance(name: &str, path: &Path, provenance: &Provenance, origin: &str) -> Self {
        Self {
            skill_name: name.to_string(),
            skill_path: path.to_path_buf(),
            source_kind: provenance.kind.clone(),
            source: provenance.source.clone(),
            source_ref: provenance.source_ref.clone(),
            source_version: provenance.version.clone(),
            source_relative_path: provenance.relative_path.clone(),
            update_status: provenance.update_status.clone(),
            origin: origin.to_string(),
        }
    }

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

impl SkillsCliLockDatabase {
    fn load(cwd: &Path) -> (Self, Vec<String>) {
        let mut database = Self::default();
        let mut warnings = Vec::new();

        if let Some(path) = global_skills_cli_lock_path() {
            if let Some(lock) = read_skills_cli_lock(&path, 3, &mut warnings) {
                database.global = lock.skills;
            }
        }

        for project_dir in skill_project_dirs(cwd) {
            let path = project_dir.join("skills-lock.json");
            if let Some(lock) = read_skills_cli_lock(&path, 1, &mut warnings) {
                database.projects.push((project_dir, lock.skills));
            }
        }

        (database, warnings)
    }

    fn provenance(&self, name: &str, scope: &str, skill_dir: &Path) -> Option<Provenance> {
        let entry = match scope {
            "global" => matching_skills_cli_lock_entry(&self.global, name),
            "project" => self
                .projects
                .iter()
                .filter(|(root, _)| skill_dir.starts_with(root))
                .max_by_key(|(root, _)| root.components().count())
                .and_then(|(_, skills)| matching_skills_cli_lock_entry(skills, name)),
            _ => None,
        }?;

        Some(entry.provenance())
    }
}

impl SkillsCliLockEntry {
    fn provenance(&self) -> Provenance {
        let kind = self.source_type.trim().to_ascii_lowercase();
        let source = self
            .source_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(self.source.as_str());
        let source = normalize_locked_source(source, &kind);
        let version = self
            .skill_folder_hash
            .as_deref()
            .and_then(|value| non_empty(Some(value)))
            .or_else(|| non_empty(self.computed_hash.as_deref()));

        Provenance {
            kind,
            source: non_empty(Some(&source)),
            source_ref: non_empty(self.r#ref.as_deref()),
            version,
            relative_path: non_empty(self.skill_path.as_deref()),
            update_status: "tracked".to_string(),
        }
    }
}

fn read_skills_cli_lock(
    path: &Path,
    expected_version: u64,
    warnings: &mut Vec<String>,
) -> Option<SkillsCliLockFile> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            warnings.push(format!(
                "{}: failed to read skills CLI lock: {err}",
                path.display()
            ));
            return None;
        }
    };
    let lock = match serde_json::from_str::<SkillsCliLockFile>(&text) {
        Ok(lock) => lock,
        Err(err) => {
            warnings.push(format!(
                "{}: invalid skills CLI lock: {err}",
                path.display()
            ));
            return None;
        }
    };
    if lock.version != expected_version {
        warnings.push(format!(
            "{}: unsupported skills CLI lock version {}; expected {}",
            path.display(),
            lock.version,
            expected_version
        ));
        return None;
    }
    Some(lock)
}

fn global_skills_cli_lock_path() -> Option<PathBuf> {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .map(|root| root.join("skills/.skill-lock.json"))
        .or_else(|| dirs::home_dir().map(|home| home.join(".agents/.skill-lock.json")))
}

fn skill_project_dirs(cwd: &Path) -> Vec<PathBuf> {
    let root = cwd
        .ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(cwd);
    let mut dirs = cwd
        .ancestors()
        .take_while(|path| *path != root)
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    dirs.push(root.to_path_buf());
    dirs
}

fn matching_skills_cli_lock_entry<'a>(
    skills: &'a BTreeMap<String, SkillsCliLockEntry>,
    name: &str,
) -> Option<&'a SkillsCliLockEntry> {
    skills.get(name).or_else(|| {
        let normalized_name = normalize_skill_match_name(name);
        skills.iter().find_map(|(candidate, entry)| {
            (normalize_skill_match_name(candidate) == normalized_name).then_some(entry)
        })
    })
}

fn normalize_locked_source(source: &str, kind: &str) -> String {
    let source = source.trim();
    if kind == "github" && parse_github_shorthand(source).is_some() {
        return format!("https://github.com/{source}.git");
    }
    source.to_string()
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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

fn summarize_update_status(statuses: BTreeSet<String>) -> String {
    if statuses.iter().any(|status| status == "checkable") {
        "checkable".to_string()
    } else if statuses.iter().any(|status| status == "local") {
        "local".to_string()
    } else {
        statuses
            .into_iter()
            .next()
            .unwrap_or_default()
    }
}

fn check_skill_updates_for_skills(
    skills: &[&SkillRecord],
    cancelled: &AtomicBool,
) -> Vec<SkillUpdateReport> {
    let mut git_remote_heads = fetch_git_remote_heads(skills, cancelled);
    fetch_git_remote_commits(skills, &mut git_remote_heads, cancelled);
    let git_changed_paths = fetch_git_changed_paths(skills, &git_remote_heads, cancelled);
    let reports = skills
        .iter()
        .map(|skill| check_skill_update(skill, &git_remote_heads, &git_changed_paths, cancelled))
        .collect();
    cleanup_git_remote_heads(git_remote_heads);
    reports
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
            let path = skill
                .paths
                .iter()
                .find(|path| matches!(path.update_status.as_str(), "checkable" | "tracked"))?;
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
        let Some(path) = skill
            .paths
            .iter()
            .find(|path| matches!(path.update_status.as_str(), "checkable" | "tracked"))
        else {
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
                    scope.spawn(move || {
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
        let Some(path) = skill
            .paths
            .iter()
            .find(|path| matches!(path.update_status.as_str(), "checkable" | "tracked"))
        else {
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
    let Some(path) = skill
        .paths
        .iter()
        .find(|path| path.update_status == "checkable")
        .or_else(|| skill.paths.first())
    else {
        return SkillUpdateReport {
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
            materialized_source_matches(repo, path, latest)
        } else {
            source_revision_matches(path.source_version.as_deref(), latest)
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
}

fn merge_text(base: Option<&str>, local: Option<&str>, incoming: Option<&str>) -> MergeOutcome {
    if local == incoming {
        return MergeOutcome {
            status: "unchanged".to_string(),
            content: local.map(str::to_string),
        };
    }
    if local == base {
        return MergeOutcome {
            status: "remote".to_string(),
            content: incoming.map(str::to_string),
        };
    }
    if incoming == base {
        return MergeOutcome {
            status: "local".to_string(),
            content: local.map(str::to_string),
        };
    }

    let Some(local) = local else {
        return MergeOutcome {
            status: "conflict".to_string(),
            content: Some(format!(
                "<<<<<<< local\n=======\n{}>>>>>>> remote\n",
                incoming.unwrap_or_default()
            )),
        };
    };
    let Some(incoming) = incoming else {
        return MergeOutcome {
            status: "conflict".to_string(),
            content: Some(format!("<<<<<<< local\n{}=======\n>>>>>>> remote\n", local)),
        };
    };
    let Some(base) = base else {
        return MergeOutcome {
            status: "conflict".to_string(),
            content: Some(format!(
                "<<<<<<< local\n{}=======\n{}>>>>>>> remote\n",
                local, incoming
            )),
        };
    };
    match git_merge_file_text(base, local, incoming) {
        Some((content, conflict)) => MergeOutcome {
            status: if conflict {
                "conflict".to_string()
            } else {
                "merged".to_string()
            },
            content: Some(content),
        },
        None => MergeOutcome {
            status: "unavailable".to_string(),
            content: None,
        },
    }
}

fn git_merge_file_text(base: &str, local: &str, incoming: &str) -> Option<(String, bool)> {
    let root = std::env::temp_dir().join(format!(
        "tendi-merge-{}-{}",
        std::process::id(),
        GIT_UPDATE_CHECK_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).ok()?;
    let local_path = root.join("local");
    let base_path = root.join("base");
    let incoming_path = root.join("incoming");
    let result = (|| {
        fs::write(&local_path, local).ok()?;
        fs::write(&base_path, base).ok()?;
        fs::write(&incoming_path, incoming).ok()?;
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
        .ok()?;
        let conflict = output.status.code() == Some(1);
        if !output.status.success() && !conflict {
            return None;
        }
        Some((String::from_utf8(output.stdout).ok()?, conflict))
    })();
    let _ = fs::remove_dir_all(&root);
    result
}

fn normalize_skill_manifest_for_visibility(
    text: &str,
    agent: AgentKind,
    visibility: SkillVisibility,
) -> String {
    crate::providers::agent_provider(agent)
        .render_skill_frontmatter(text, visibility)
        .unwrap_or_else(|_| text.to_string())
}

fn git_files_at_revision(
    repo: &Path,
    revision: &str,
    relative: &str,
) -> Option<BTreeMap<String, Vec<u8>>> {
    let mut args = vec![
        "ls-tree".to_string(),
        "-r".to_string(),
        "--name-only".to_string(),
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
    for file in String::from_utf8(output.stdout).ok()?.lines() {
        let bytes = git::run_git(
            repo,
            ["show".to_string(), format!("{revision}:{file}")],
            git::LOCAL_COMMAND_TIMEOUT,
            git::never_cancelled(),
        )
        .ok()?;
        if !bytes.status.success() {
            return None;
        }
        files.insert(file.to_string(), bytes.stdout);
    }
    Some(files)
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
) -> Option<BTreeMap<String, Vec<u8>>> {
    let snapshot = store.skill_snapshot(&path.path).ok().flatten()?;
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

fn merge_git_path_files(
    repo: &Path,
    path: &SkillPath,
    remote_revision: &str,
    _tendi_settings: &[GitSkillVisibility],
    store: &crate::storage::Store,
) -> Vec<GitUpdateFile> {
    let relative = normalized_skill_repo_path(path);
    let base = path
        .source_version
        .as_deref()
        .and_then(|revision| git_files_at_revision(repo, revision, relative))
        .or_else(|| snapshot_files(store, path, relative));
    let local = local_skill_files(&path.path, repo, relative);
    let incoming = git_files_at_revision(repo, remote_revision, relative).unwrap_or_default();
    merge_file_maps(
        base,
        local,
        incoming,
        relative,
        &path.path.display().to_string(),
        path.effective_visibility,
        path.agent,
    )
}

fn merge_materialized_git_path_files(
    repo: &Path,
    path: &SkillPath,
    remote_revision: &str,
    visibility: SkillVisibility,
    store: &crate::storage::Store,
) -> Vec<GitUpdateFile> {
    let relative = normalized_skill_repo_path(path);
    let base = path
        .source_version
        .as_deref()
        .and_then(|revision| git_files_at_revision(repo, revision, relative))
        .or_else(|| snapshot_files(store, path, relative));
    let local = local_skill_files(&path.path, repo, relative);
    let incoming = git_files_at_revision(repo, remote_revision, relative).unwrap_or_default();
    merge_file_maps(
        base,
        local,
        incoming,
        relative,
        &path.path.display().to_string(),
        visibility,
        path.agent,
    )
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
            if crate::providers::agent_provider(agent).is_managed_skill_file(path_in_skill) {
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
                return Some(GitUpdateFile {
                    path,
                    resolution_key,
                    before: before.flatten().unwrap_or_default(),
                    base: String::new(),
                    incoming: incoming_text.flatten().unwrap_or_default(),
                    after: String::new(),
                    before_bytes: before_bytes.cloned(),
                    incoming_bytes: incoming_bytes.cloned(),
                    after_bytes: None,
                    before_exists: before_bytes.is_some(),
                    incoming_exists: incoming_bytes.is_some(),
                    after_exists: false,
                    status: "unavailable".to_string(),
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
                });
            }
            let before = before.flatten();
            let mut base = base_text.flatten();
            let mut incoming = incoming_text.flatten();
            if path_in_skill == "SKILL.md" {
                base = base
                    .map(|text| normalize_skill_manifest_for_visibility(&text, agent, visibility));
                incoming = incoming
                    .map(|text| normalize_skill_manifest_for_visibility(&text, agent, visibility));
            }
            let merged = merge_text(base.as_deref(), before.as_deref(), incoming.as_deref());
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
                after: merged_content.clone().unwrap_or_default(),
                before_bytes: before_bytes.cloned(),
                incoming_bytes: incoming_bytes.cloned(),
                after_bytes: None,
                before_exists: before_bytes.is_some(),
                incoming_exists: incoming_bytes.is_some(),
                after_exists: !matches!(
                    merged_status.as_str(),
                    "conflict" | "unavailable" | "binary"
                ) && merged_content.is_some(),
                status: merged_status,
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
) -> Option<GitUpdateAction> {
    let materialized = git_repository_boundary(&path.path).is_none();
    let repo = git_checkout_for_skill_path(path, git::never_cancelled())?;
    let tendi_settings = if materialized {
        Vec::new()
    } else {
        git_tendi_settings(scan, &repo)
    };
    let latest = update.latest_version.as_deref()?;
    let files = if materialized {
        Vec::new()
    } else {
        merge_git_path_files(&repo, path, latest, &tendi_settings, store)
    };
    let materialized_files = if materialized {
        merge_materialized_git_path_files(&repo, path, latest, path.effective_visibility, store)
    } else {
        Vec::new()
    };
    let diff = if materialized {
        String::new()
    } else {
        git_path_diff(&repo, path, latest)
    };
    Some(GitUpdateAction {
        name: skill.name.clone(),
        skill_names: vec![skill.name.clone()],
        repo,
        source: path.source.clone()?,
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
    })
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
    let incoming =
        normalize_skill_manifest_for_visibility(&incoming, path.agent, path.effective_visibility);
    let snapshot = store.skill_snapshot(&path.path)?;
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
            before,
            base: String::new(),
            incoming,
            after: String::new(),
        })));
    };
    let merged = merge_text(Some(&base), Some(&before), Some(&incoming));
    let merged_status = merged.status.clone();
    let merged_content = merged.content.unwrap_or_default();
    if merged_status == "conflict" {
        return Ok(Some(RegistryUpdatePlan::Issue(SkillMergeIssue {
            name: skill.name.clone(),
            path: skill_file,
            resolution_key,
            status: merged_status,
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
    let update_result = action
        .source_ref
        .as_deref()
        .map(|source_ref| run_git(&action.repo, &["fetch", "origin", source_ref]))
        .unwrap_or_else(|| run_git(&action.repo, &["fetch", "origin"]));
    if let Err(update_error) = update_result {
        let restore_result = restore_tendi_git_settings(action);
        return match restore_result {
            Err(restore_error) => Err(update_error.context(format!(
                "also failed to restore Tendi visibility settings: {restore_error:#}"
            ))),
            Ok(()) => Err(update_error),
        };
    }
    apply_update_files(&action.repo, &action.files)?;
    let restore_result = restore_tendi_git_settings(action);
    restore_result
}

fn apply_update_files(root: &Path, files: &[GitUpdateFile]) -> Result<()> {
    for file in files {
        let path = root.join(&file.path);
        ensure_path_inside(root, &path)?;
        validate_update_file(&path, file)?;
        if file.after_exists {
            if let Some(bytes) = &file.after_bytes {
                atomic_write_bytes(&path, bytes)?;
            } else {
                atomic_write(&path, &file.after)?;
            }
        } else if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
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

fn restore_tendi_git_settings(action: &GitUpdateAction) -> Result<()> {
    let mut changes = Vec::new();
    for setting in &action.tendi_settings {
        if !setting.skill_dir.join("SKILL.md").is_file() {
            continue;
        }
        changes.extend(plan_skill_visibility_at_path(
            &setting.skill_dir,
            setting.agent,
            setting.visibility,
            true,
        )?);
    }
    apply_changes(&ChangeSet {
        changes: dedupe_changes(changes),
    })
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
    let output = git::run_git(
        repo,
        [
            "ls-remote".to_string(),
            source.to_string(),
            requested.to_string(),
        ],
        git::NETWORK_COMMAND_TIMEOUT,
        cancelled,
    )
    .ok()?;
    if !output.status.success() {
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
    Some(GitRemoteHead { oid, reference })
}

fn fetch_git_remote_commit(
    repo: &Path,
    source: &str,
    oid: &str,
    reference: &str,
    cancelled: &AtomicBool,
) -> bool {
    let output = git::run_git(
        repo,
        [
            "fetch".to_string(),
            "--no-write-fetch-head".to_string(),
            "--no-tags".to_string(),
            source.to_string(),
            format!("+{oid}:{reference}"),
        ],
        git::NETWORK_COMMAND_TIMEOUT,
        cancelled,
    );
    let success = output.is_ok_and(|output| output.status.success());
    if !success {
        delete_git_ref(repo, reference);
    }
    success
}

fn is_git_source_kind(kind: &str) -> bool {
    matches!(kind, "git" | "github" | "gitlab" | "huggingface")
}

fn git_checkout_for_skill_path(path: &SkillPath, cancelled: &AtomicBool) -> Option<PathBuf> {
    if let Some(repo) = git_repository_boundary(&path.path) {
        return Some(repo);
    }
    let source = path.source.as_deref()?;
    let cache_key = path
        .source_ref
        .as_deref()
        .map(|source_ref| format!("{source}#{source_ref}"))
        .unwrap_or_else(|| source.to_string());
    let repo = persistent_source_root(&cache_key).ok()?;
    if !repo.join(".git").is_dir() {
        if repo.exists() {
            fs::remove_dir_all(&repo).ok()?;
        }
        fs::create_dir_all(repo.parent()?).ok()?;
        run_git_clone(source, path.source_ref.as_deref(), &repo, cancelled).ok()?;
    }
    Some(repo)
}

fn normalized_skill_repo_path(path: &SkillPath) -> &str {
    path.source_relative_path
        .as_deref()
        .unwrap_or(".")
        .trim_end_matches("/SKILL.md")
        .trim_end_matches("SKILL.md")
        .trim_end_matches('/')
}

fn materialized_source_matches(repo: &Path, path: &SkillPath, latest: &str) -> bool {
    let Some(current) = path.source_version.as_deref() else {
        return false;
    };
    if current.len() == 64 && current.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return git_folder_sha256(repo, latest, normalized_skill_repo_path(path))
            .is_some_and(|fingerprint| fingerprint.eq_ignore_ascii_case(current));
    }
    if current.len() == 40 && current.chars().all(|ch| ch.is_ascii_hexdigit()) {
        if latest.eq_ignore_ascii_case(current) {
            return true;
        }
        return git_tree_oid(repo, latest, normalized_skill_repo_path(path))
            .is_some_and(|oid| oid.eq_ignore_ascii_case(current));
    }
    false
}

fn source_revision_matches(current: Option<&str>, latest: &str) -> bool {
    let Some(current) = current else {
        return false;
    };
    (7..=40).contains(&current.len())
        && current.chars().all(|ch| ch.is_ascii_hexdigit())
        && latest.starts_with(current)
}

fn git_tree_oid(repo: &Path, revision: &str, relative: &str) -> Option<String> {
    let spec = if relative.is_empty() || relative == "." {
        format!("{revision}^{{tree}}")
    } else {
        format!("{revision}:{relative}")
    };
    git_output(repo, &["rev-parse", &spec])
}

fn git_folder_sha256(repo: &Path, revision: &str, relative: &str) -> Option<String> {
    let mut args = vec!["ls-tree", "-r", "--name-only", revision];
    if !relative.is_empty() && relative != "." {
        args.extend(["--", relative]);
    }
    let listing = git_output(repo, &args)?;
    let prefix = relative.trim_matches('/');
    let mut files = listing.lines().map(str::to_string).collect::<Vec<_>>();
    files.sort();
    let mut hasher = Sha256::new();
    for file in files {
        let relative_file = file
            .strip_prefix(prefix)
            .unwrap_or(&file)
            .trim_start_matches('/');
        hasher.update(relative_file.as_bytes());
        let output = git::run_git(
            repo,
            ["show".to_string(), format!("{revision}:{file}")],
            git::LOCAL_COMMAND_TIMEOUT,
            git::never_cancelled(),
        )
        .ok()?;
        if !output.status.success() {
            return None;
        }
        hasher.update(&output.stdout);
    }
    Some(format!("{:x}", hasher.finalize()))
}

fn cleanup_git_remote_heads(heads: BTreeMap<PathBuf, Option<GitRemoteHead>>) {
    for (repo, head) in heads {
        if let Some(head) = head {
            delete_git_ref(&repo, &head.reference);
        }
    }
}

fn delete_git_ref(repo: &Path, reference: &str) {
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

pub(crate) fn plan_skill_frontmatter_for_agent(
    path: PathBuf,
    agent: AgentKind,
    visibility: SkillVisibility,
) -> Result<FileChange> {
    if visibility == SkillVisibility::Mixed {
        bail!("mixed visibility is a scan summary and cannot be written to SKILL.md");
    }
    let before = read_optional(&path)?.context("SKILL.md does not exist")?;
    let doc = MarkdownDoc::parse_lenient(&before)?;
    let provider = crate::providers::agent_provider(agent);
    if provider.skill_frontmatter_satisfies(&doc.meta, visibility) {
        return Ok(FileChange {
            path,
            before_sha256: Some(sha256_text(&before)),
            before: Some(before.clone()),
            after: before,
        });
    }
    let after = provider.render_skill_frontmatter(&before, visibility)?;
    Ok(FileChange {
        path,
        before_sha256: Some(sha256_text(&before)),
        before: Some(before),
        after,
    })
}

fn plan_skill_visibility_at_path(
    skill_dir: &Path,
    agent: AgentKind,
    visibility: SkillVisibility,
    update_provider_config: bool,
) -> Result<Vec<FileChange>> {
    let mut changes = vec![plan_skill_frontmatter_for_agent(
        skill_dir.join("SKILL.md"),
        agent,
        visibility,
    )?];
    changes.extend(
        crate::providers::agent_provider(agent).plan_skill_visibility(
            skill_dir,
            visibility,
            update_provider_config,
        )?,
    );
    Ok(changes)
}

#[cfg(test)]
fn render_skill_frontmatter_for_visibility(
    before: &str,
    agent: AgentKind,
    visibility: SkillVisibility,
) -> Result<String> {
    let doc = MarkdownDoc::parse_lenient(before)?;
    let provider = crate::providers::agent_provider(agent);
    if provider.skill_frontmatter_satisfies(&doc.meta, visibility) {
        return Ok(before.to_string());
    }
    provider.render_skill_frontmatter(before, visibility)
}

pub(crate) fn render_skill_frontmatter_with_provider_key(
    before: &str,
    visibility: SkillVisibility,
    provider_key: Option<&str>,
) -> Result<String> {
    let mut doc = MarkdownDoc::parse_lenient(before)?;
    if let Some((yaml, tail)) = split_frontmatter_raw(before) {
        let mut lines = yaml.lines().map(str::to_string).collect::<Vec<_>>();
        if let Some(provider_key) = provider_key {
            set_top_level_bool(
                &mut lines,
                provider_key,
                !matches!(visibility, SkillVisibility::Auto),
            );
        }
        set_tendi_visibility_lines(&mut lines, visibility);
        return Ok(format!("---\n{}\n---{}", lines.join("\n"), tail));
    }

    set_tendi_visibility(&mut doc.meta, visibility);
    if let Some(provider_key) = provider_key {
        let provider_key = Value::String(provider_key.to_string());
        if matches!(visibility, SkillVisibility::Auto) {
            doc.meta.remove(&provider_key);
        } else {
            doc.meta.insert(provider_key, Value::Bool(true));
        }
    }
    doc.render()
}

fn split_frontmatter_raw(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---\n")?;
    rest.split_once("\n---")
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

fn set_tendi_visibility_lines(lines: &mut Vec<String>, visibility: SkillVisibility) {
    let rendered = format!("visibility: {}", visibility.label());
    if let Some(line) = lines
        .iter_mut()
        .find(|line| line.starts_with("tendi.visibility:"))
    {
        *line = format!("tendi.{rendered}");
        return;
    }

    if let Some(tendi_index) = lines.iter().position(|line| line.trim_end() == "tendi:") {
        let next_top_level = lines
            .iter()
            .enumerate()
            .skip(tendi_index + 1)
            .find_map(|(index, line)| {
                (!line.is_empty() && !line.starts_with(' ') && !line.starts_with('\t'))
                    .then_some(index)
            })
            .unwrap_or(lines.len());
        if let Some(line) = lines[tendi_index + 1..next_top_level]
            .iter_mut()
            .find(|line| line.trim_start().starts_with("visibility:"))
        {
            let indent = line.len() - line.trim_start().len();
            *line = format!("{}{}", " ".repeat(indent), rendered);
        } else {
            lines.insert(tendi_index + 1, format!("  {rendered}"));
        }
        return;
    }

    lines.push("tendi:".to_string());
    lines.push(format!("  {rendered}"));
}

pub(crate) fn skill_frontmatter_satisfies_with_provider_key(
    meta: &serde_yaml::Mapping,
    visibility: SkillVisibility,
    provider_key: Option<&str>,
) -> bool {
    let meta = Value::Mapping(meta.clone());
    let tendi_visibility = parse_tendi_visibility(&meta);
    let provider_disabled = provider_key.is_some_and(|provider_key| {
        meta.get(provider_key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    });

    match visibility {
        SkillVisibility::Auto => {
            !provider_disabled && tendi_visibility == Some(SkillVisibility::Auto)
        }
        SkillVisibility::Manual => {
            if provider_key.is_some() {
                provider_disabled
                    && matches!(tendi_visibility, None | Some(SkillVisibility::Manual))
            } else {
                tendi_visibility == Some(SkillVisibility::Manual)
            }
        }
        SkillVisibility::Off => {
            (provider_key.is_none() || provider_disabled)
                && tendi_visibility == Some(SkillVisibility::Off)
        }
        SkillVisibility::Mixed => false,
    }
}

fn set_tendi_visibility(meta: &mut serde_yaml::Mapping, visibility: SkillVisibility) {
    let tendi_key = Value::String("tendi".to_string());
    if !matches!(meta.get(&tendi_key), Some(Value::Mapping(_))) {
        meta.insert(tendi_key.clone(), Value::Mapping(Default::default()));
    }
    if let Some(tendi) = meta.get_mut(&tendi_key).and_then(Value::as_mapping_mut) {
        tendi.insert(
            Value::String("visibility".to_string()),
            Value::String(visibility.label().to_string()),
        );
    }
}

struct MarkdownDoc {
    meta: serde_yaml::Mapping,
    body: String,
}

impl MarkdownDoc {
    fn parse(text: &str) -> Result<Self> {
        if let Some(rest) = text.strip_prefix("---\n") {
            if let Some((yaml, body)) = rest.split_once("\n---") {
                let meta = serde_yaml::from_str::<serde_yaml::Mapping>(yaml)
                    .context("failed to parse SKILL.md frontmatter")?;
                let body = body.strip_prefix('\n').unwrap_or(body).to_string();
                return Ok(Self { meta, body });
            }
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

    fn render(&self) -> Result<String> {
        let yaml = serde_yaml::to_string(&self.meta)?;
        Ok(format!("---\n{}---\n{}", yaml, self.body))
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
        tags: [&'a str; 1],
    }

    let frontmatter = WrapperFrontmatter {
        name,
        description,
        tags: ["wrapper"],
    };
    let yaml = serde_yaml::to_string(&frontmatter).unwrap_or_else(|_| {
        "name: wrapper\ndescription: Route to selected child skills.\ntags:\n- wrapper\n"
            .to_string()
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
    let Some((yaml, tail)) = split_frontmatter_raw(text) else {
        return text.to_string();
    };
    let mut lines = yaml.lines().map(str::to_string).collect::<Vec<_>>();
    set_top_level_string(&mut lines, "description", description);
    format!("---\n{}\n---{}", lines.join("\n"), tail)
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

fn parse_wrapper_route_names(text: &str) -> Vec<String> {
    let section = wrapper_route_xml_section(text).or_else(|| wrapper_route_section(text));
    let Some(section) = section else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut names = Vec::new();
    for line in section.lines() {
        let line = line.trim_start();
        let Some(route) = line.strip_prefix("- ") else {
            continue;
        };
        if let Some(name) = parse_route_name(route) {
            if seen.insert(name.clone()) {
                names.push(name);
            }
        }
    }
    names
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

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn dedupe_changes(changes: Vec<FileChange>) -> Vec<FileChange> {
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
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::skill_targets::SkillInstallScope;
    use serde_yaml::Value;

    use super::{
        AgentKind, ChangeSet, GitSkillVisibility, GitUpdateAction, GitUpdateFile,
        MarkdownDoc, MaterializedGitTarget, ResolvedAddSource, SkillDistributionMode,
        SkillDistributionPlan, SkillPath, SkillRecord, SkillSourceRecord,
        SkillUpdatePlan, SkillVisibility, WRAPPER_CATALOG_END, WRAPPER_CATALOG_START,
        apply_git_update, apply_skill_add_with_target_root, apply_skill_delete_plan,
        apply_skill_distribution_plan, apply_skill_update_plan_with_store, apply_update_files,
        build_skill_add_plan_with_target_root, check_skill_updates, clear_tendi_git_changes,
        copy_dir, discover_installable_skills, format_delete_plan,
        git_materialized_path_files, materialize_skill_dir_to_root, parse_add_source,
        parse_skill_file_references,
        parse_tendi_visibility, plan_skill_add, plan_skill_delete_many,
        render_wrapper_after,
        sanitize_skill_dir_name, scan_skills_without_source_database as scan_skills,
        sha256_file, skill_backup_exclusion_reason,
    };

    fn temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn run_test_git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn three_way_merge_keeps_independent_local_and_remote_edits() {
        let merged = super::merge_text(
            Some("title: old\nbody: old\nfooter: old\n"),
            Some("title: local\nbody: old\nfooter: old\n"),
            Some("title: old\nbody: old\nfooter: remote\n"),
        );

        assert_eq!(merged.status, "merged");
        let content = merged.content.unwrap();
        assert!(content.contains("title: local"));
        assert!(content.contains("footer: remote"));
    }

    #[test]
    fn three_way_merge_reports_same_region_conflicts() {
        let merged = super::merge_text(
            Some("title: old\n"),
            Some("title: local\n"),
            Some("title: remote\n"),
        );

        assert_eq!(merged.status, "conflict");
        let content = merged.content.unwrap();
        assert!(content.contains("<<<<<<< local"));
        assert!(content.contains(">>>>>>> remote"));
    }


    #[test]
    fn update_application_refuses_a_stale_local_file() {
        let root = temp_dir("tendi-update-stale-file-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("SKILL.md");
        fs::write(&path, "edited-after-preview\n").unwrap();
        let file = GitUpdateFile {
            path: "SKILL.md".to_string(),
            resolution_key: "demo:SKILL.md".to_string(),
            before: "before\n".to_string(),
            base: "before\n".to_string(),
            incoming: "incoming\n".to_string(),
            after: "incoming\n".to_string(),
            before_bytes: Some(b"before\n".to_vec()),
            incoming_bytes: Some(b"incoming\n".to_vec()),
            after_bytes: None,
            before_exists: true,
            incoming_exists: true,
            after_exists: true,
            status: "remote".to_string(),
        };

        let error = apply_update_files(&root, &[file]).unwrap_err();
        assert!(format!("{error:#}").contains("refusing to overwrite changed file"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "edited-after-preview\n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn binary_resolution_writes_selected_bytes() {
        let root = temp_dir("tendi-binary-resolution-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("asset.bin");
        let local = vec![0_u8, 255, 1];
        let incoming = vec![2_u8, 254, 3];
        fs::write(&path, &local).unwrap();
        let mut file = GitUpdateFile {
            path: "asset.bin".to_string(),
            resolution_key: "demo:asset.bin".to_string(),
            before: String::new(),
            base: String::new(),
            incoming: String::new(),
            after: String::new(),
            before_bytes: Some(local.clone()),
            incoming_bytes: Some(incoming.clone()),
            after_bytes: None,
            before_exists: true,
            incoming_exists: true,
            after_exists: false,
            status: "binary".to_string(),
        };
        super::resolve_update_file(
            &mut file,
            &BTreeMap::from([(
                "demo:asset.bin".to_string(),
                super::USE_UPDATE_RESOLUTION.to_string(),
            )]),
        );
        apply_update_files(&root, &[file]).unwrap();
        assert_eq!(fs::read(&path).unwrap(), incoming);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parse_add_source_accepts_local_github_and_git_sources() {
        let root = temp_dir("tendi-parse-add-source-test");
        fs::create_dir_all(&root).unwrap();
        let local = root.join("skills");
        fs::create_dir_all(&local).unwrap();

        let parsed_local = parse_add_source(&root, "skills").unwrap();
        assert_eq!(parsed_local.kind, "local");
        assert_eq!(parsed_local.root, Some(local.canonicalize().unwrap()));

        let parsed_shorthand = parse_add_source(&root, "vercel-labs/agent-skills").unwrap();
        assert_eq!(parsed_shorthand.kind, "github");
        assert_eq!(
            parsed_shorthand.url,
            "https://github.com/vercel-labs/agent-skills.git"
        );

        let parsed_git =
            parse_add_source(&root, "git@github.com:vercel-labs/agent-skills.git").unwrap();
        assert_eq!(parsed_git.kind, "github");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn git_clone_checks_out_requested_ref() {
        let root = temp_dir("tendi-clone-ref-test");
        let repository = root.join("repository");
        let checkout = root.join("checkout");
        fs::create_dir_all(&repository).unwrap();
        run_test_git(&repository, &["init", "-b", "main"]);
        run_test_git(&repository, &["config", "user.email", "test@example.com"]);
        run_test_git(&repository, &["config", "user.name", "Test"]);
        fs::write(repository.join("branch.txt"), "main").unwrap();
        fs::create_dir_all(repository.join("skills/demo")).unwrap();
        fs::write(
            repository.join("skills/demo/SKILL.md"),
            "---\nname: demo\ndescription: Demo\n---\n",
        )
        .unwrap();
        run_test_git(&repository, &["add", "."]);
        run_test_git(&repository, &["commit", "-m", "main"]);
        run_test_git(&repository, &["checkout", "-b", "release"]);
        fs::write(repository.join("branch.txt"), "release").unwrap();
        run_test_git(&repository, &["commit", "-am", "release"]);

        super::run_git_clone(
            repository.to_str().unwrap(),
            Some("release"),
            &checkout,
            super::git::never_cancelled(),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(checkout.join("branch.txt")).unwrap(),
            "release"
        );
        assert_eq!(
            run_test_git(&checkout, &["branch", "--show-current"]),
            "release"
        );
        let source_root = checkout.join("skills/demo");
        let installed = root.join("installed/demo");
        let report = super::SkillAddApplyReport {
            plan: super::SkillAddPlan {
                source: "https://github.com/example/repo.git".to_string(),
                source_kind: "github".to_string(),
                source_ref: Some("release".to_string()),
                source_root: source_root.clone(),
                target: AgentKind::Shared.into(),
                scope: SkillInstallScope::Project,
                mode: "copy".to_string(),
                available: Vec::new(),
                selected: vec![super::InstallableSkill {
                    name: "demo".to_string(),
                    description: None,
                    path: source_root,
                    relative_path: String::new(),
                    dependencies: Vec::new(),
                }],
                operations: Vec::new(),
            },
            results: vec![super::MaterializeResult {
                source: checkout.join("skills/demo"),
                target: installed,
                mode: "copy".to_string(),
                health: "copy-ok".to_string(),
                applied: true,
            }],
        };
        let records = super::skill_source_records_for_add(&report);
        assert_eq!(
            records[0].source_relative_path.as_deref(),
            Some("skills/demo")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn materialized_git_preview_compares_installed_files_with_remote_tree() {
        let root = temp_dir("tendi-materialized-git-preview-test");
        let repository = root.join("repository");
        let installed = root.join("installed/demo");
        fs::create_dir_all(repository.join("skills/demo/agents")).unwrap();
        fs::create_dir_all(installed.join("agents")).unwrap();
        run_test_git(&repository, &["init", "-b", "main"]);
        run_test_git(&repository, &["config", "user.email", "test@example.com"]);
        run_test_git(&repository, &["config", "user.name", "Test"]);
        fs::write(
            repository.join("skills/demo/SKILL.md"),
            "---\nname: demo\n---\n\nnew\n",
        )
        .unwrap();
        fs::write(
            repository.join("skills/demo/agents/openai.yaml"),
            "new-policy\n",
        )
        .unwrap();
        run_test_git(&repository, &["add", "."]);
        run_test_git(&repository, &["commit", "-m", "new"]);

        fs::write(installed.join("SKILL.md"), "---\nname: demo\n---\n\nold\n").unwrap();
        fs::write(installed.join("local.txt"), "removed\n").unwrap();

        let files = git_materialized_path_files(&repository, &installed, "skills/demo", "HEAD");
        let by_path = files
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            by_path["skills/demo/SKILL.md"].before,
            "---\nname: demo\n---\n\nold\n"
        );
        assert_eq!(
            by_path["skills/demo/SKILL.md"].after,
            "---\nname: demo\n---\n\nnew\n"
        );
        assert_eq!(by_path["skills/demo/agents/openai.yaml"].before, "");
        assert_eq!(
            by_path["skills/demo/agents/openai.yaml"].after,
            "new-policy\n"
        );
        assert_eq!(by_path["skills/demo/local.txt"].before, "removed\n");
        assert_eq!(by_path["skills/demo/local.txt"].after, "");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discover_installable_skills_finds_catalog_and_agent_dirs() {
        let root = temp_dir("tendi-discover-add-skills-test");
        let alpha = root.join("skills/frontend/alpha");
        let beta = root.join(".codex/skills/beta");
        fs::create_dir_all(&alpha).unwrap();
        fs::create_dir_all(&beta).unwrap();
        fs::write(
            alpha.join("SKILL.md"),
            "---\nname: alpha\ndescription: Alpha skill\n---\n\n# alpha\n",
        )
        .unwrap();
        fs::write(
            beta.join("SKILL.md"),
            "---\nname: beta\ndescription: Beta skill\n---\n\n# beta\n",
        )
        .unwrap();

        let skills = discover_installable_skills(&root).unwrap();
        let names = skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["alpha", "beta"]);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn add_plan_selects_named_local_skills() {
        let root = temp_dir("tendi-plan-add-skills-test");
        let repo = root.join("repo");
        let skill_dir = repo.join("skills/demo");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: Demo skill\n---\n\n# demo\n",
        )
        .unwrap();
        fs::write(skill_dir.join("Archive.zip"), [0_u8, 0xff, 0x00]).unwrap();

        let plan = plan_skill_add(
            &root,
            &super::SkillAddOptions {
                source: "repo".to_string(),
                target: AgentKind::Shared.into(),
                scope: SkillInstallScope::Global,
                skills: vec!["demo".to_string()],
                copy: true,
                overwrite: false,
                visibility: SkillVisibility::Auto,
            },
        )
        .unwrap();
        assert_eq!(plan.selected.len(), 1);
        assert_eq!(plan.operations[0].name, "demo");
        assert_eq!(plan.operations[0].mode, "copy");
        assert_eq!(plan.operations[0].status, "planned");
        assert!(super::skill_add_catalog_fingerprint(&plan).is_ok());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn apply_add_sets_selected_visibility_on_installed_skill() {
        let root = temp_dir("tendi-apply-add-visibility-test");
        let repo = root.join("repo");
        let target_root = root.join("installed");
        let skill_dir = repo.join("skills/demo");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: Demo skill\n---\n\n# demo\n",
        )
        .unwrap();

        let report = apply_skill_add_with_target_root(
            &root,
            &super::SkillAddOptions {
                source: "repo".to_string(),
                target: AgentKind::Cursor.into(),
                scope: SkillInstallScope::Global,
                skills: vec!["demo".to_string()],
                copy: true,
                overwrite: false,
                visibility: SkillVisibility::Manual,
            },
            &target_root,
        )
        .unwrap();

        let installed_skill = target_root.join("demo/SKILL.md");
        let frontmatter = fs::read_to_string(installed_skill).unwrap();
        assert!(frontmatter.contains("disable-model-invocation: true"));
        assert!(frontmatter.contains("visibility: manual"));
        assert!(!target_root.join("demo/agents/openai.yaml").exists());
        assert_eq!(report.results.len(), 1);
        let source_records = super::skill_source_records_for_add(&report);
        assert_eq!(source_records.len(), 1);
        assert_eq!(source_records[0].skill_name, "demo");
        assert_eq!(source_records[0].skill_path, target_root.join("demo"));
        assert_eq!(source_records[0].source_kind, "local");
        assert_eq!(
            source_records[0].source_relative_path.as_deref(),
            Some("skills/demo")
        );
        assert_eq!(source_records[0].origin, "tendi-install");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn add_plan_expands_skill_dependencies() {
        let root = temp_dir("tendi-plan-add-skill-dependencies-test");
        let repo = root.join("repo");
        let grill_with_docs_dir = repo.join("skills/engineering/grill-with-docs");
        let grilling_dir = repo.join("skills/productivity/grilling");
        let domain_modeling_dir = repo.join("skills/engineering/domain-modeling");
        fs::create_dir_all(&grill_with_docs_dir).unwrap();
        fs::create_dir_all(&grilling_dir).unwrap();
        fs::create_dir_all(&domain_modeling_dir).unwrap();
        fs::write(
            grill_with_docs_dir.join("SKILL.md"),
            "---\nname: grill-with-docs\ndescription: Grill with docs\n---\n\nRun [`grilling`](../../productivity/grilling/SKILL.md), then read [`domain-modeling`](../domain-modeling/SKILL.md).\n",
        )
        .unwrap();
        fs::write(
            grilling_dir.join("SKILL.md"),
            "---\nname: grilling\ndescription: Grilling\n---\n\nAsk questions.\n",
        )
        .unwrap();
        fs::write(
            domain_modeling_dir.join("SKILL.md"),
            "---\nname: domain-modeling\ndescription: Domain modeling\n---\n\nModel terms.\n",
        )
        .unwrap();

        let plan = plan_skill_add(
            &root,
            &super::SkillAddOptions {
                source: "repo".to_string(),
                target: AgentKind::Shared.into(),
                scope: SkillInstallScope::Global,
                skills: vec!["grill-with-docs".to_string()],
                copy: true,
                overwrite: false,
                visibility: SkillVisibility::Auto,
            },
        )
        .unwrap();
        let selected = plan
            .selected
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            selected,
            vec!["domain-modeling", "grill-with-docs", "grilling"]
        );
        let grill_with_docs = plan
            .available
            .iter()
            .find(|skill| skill.name == "grill-with-docs")
            .unwrap();
        assert_eq!(
            grill_with_docs.dependencies,
            vec!["domain-modeling".to_string(), "grilling".to_string()]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn add_plan_expands_transitive_frontmatter_dependencies() {
        let root = temp_dir("tendi-plan-add-transitive-skill-dependencies-test");
        let repo = root.join("repo");
        for name in ["parent", "child", "base"] {
            fs::create_dir_all(repo.join(format!("skills/{name}"))).unwrap();
        }
        fs::write(
            repo.join("skills/parent/SKILL.md"),
            "---\nname: parent\ndependencies:\n  - child\n---\n",
        )
        .unwrap();
        fs::write(
            repo.join("skills/child/SKILL.md"),
            "---\nname: child\nrequires: base\n---\n",
        )
        .unwrap();
        fs::write(repo.join("skills/base/SKILL.md"), "---\nname: base\n---\n").unwrap();

        let plan = plan_skill_add(
            &root,
            &super::SkillAddOptions {
                source: "repo".to_string(),
                target: AgentKind::Shared.into(),
                scope: SkillInstallScope::Global,
                skills: vec!["parent".to_string()],
                copy: true,
                overwrite: false,
                visibility: SkillVisibility::Auto,
            },
        )
        .unwrap();
        let selected = plan
            .selected
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(selected, vec!["base", "child", "parent"]);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn add_plan_expands_lark_style_dependencies() {
        let root = temp_dir("tendi-plan-add-lark-style-dependencies-test");
        let repo = root.join("repo");
        for name in ["lark-im", "lark-shared", "lark-sheets"] {
            fs::create_dir_all(repo.join(format!("skills/{name}"))).unwrap();
        }
        fs::write(
            repo.join("skills/lark-im/SKILL.md"),
            "---\nname: lark-im\n---\n\n开始前先读取 [`../lark-shared/SKILL.md`](../lark-shared/SKILL.md)。\n",
        )
        .unwrap();
        fs::write(
            repo.join("skills/lark-sheets/SKILL.md"),
            "---\nname: lark-sheets\nmetadata:\n  requires:\n    bins: [lark-cli]\n    siblings: [lark-shared]\n---\n",
        )
        .unwrap();
        fs::write(
            repo.join("skills/lark-shared/SKILL.md"),
            "---\nname: lark-shared\n---\n",
        )
        .unwrap();

        let plan = plan_skill_add(
            &root,
            &super::SkillAddOptions {
                source: "repo".to_string(),
                target: AgentKind::Shared.into(),
                scope: SkillInstallScope::Global,
                skills: vec!["lark-im".to_string(), "lark-sheets".to_string()],
                copy: true,
                overwrite: false,
                visibility: SkillVisibility::Auto,
            },
        )
        .unwrap();
        let selected = plan
            .selected
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(selected, vec!["lark-im", "lark-shared", "lark-sheets"]);
        for name in ["lark-im", "lark-sheets"] {
            let skill = plan
                .available
                .iter()
                .find(|skill| skill.name == name)
                .unwrap();
            assert_eq!(skill.dependencies, vec!["lark-shared"]);
        }

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parses_skill_file_paths_without_prose_heuristics() {
        let cases = [
            (
                "认证等通用处理只读 [`../lark-shared/SKILL.md`](../lark-shared/SKILL.md)。",
                vec![PathBuf::from("../lark-shared/SKILL.md")],
            ),
            (
                "No dependency keywords: `/Users/example/pr/SKILL.md`.",
                vec![PathBuf::from("/Users/example/pr/SKILL.md")],
            ),
            (
                "按需改用 [`../lark-drive/SKILL.md`](../lark-drive/SKILL.md)。",
                vec![PathBuf::from("../lark-drive/SKILL.md")],
            ),
            (
                "Use `$child`; describe `SKILL.md`; ignore `demo/SKILL.md.bak`.",
                vec![],
            ),
            (
                "Windows path: `C:\\skills\\child\\SKILL.md`.",
                vec![PathBuf::from("C:/skills/child/SKILL.md")],
            ),
            (
                "Remote docs are not local dependencies: [demo](https://example.com/demo/SKILL.md).",
                vec![],
            ),
        ];

        for (body, expected) in cases {
            let text = format!("---\nname: test\n---\n\n{body}\n");
            assert_eq!(parse_skill_file_references(&text), expected);
        }
    }

    #[test]
    fn scan_skills_follows_path_dependencies_outside_roots() {
        let root = temp_dir("tendi-scan-skill-relations-test");
        let parent_dir = root.join(".agents/skills/parent");
        let ignored_dir = root.join(".agents/skills/ignored");
        let child_dir = root.join("external/child-directory");
        let grandchild_dir = root.join("external/grandchild");
        fs::create_dir_all(&parent_dir).unwrap();
        fs::create_dir_all(&ignored_dir).unwrap();
        fs::create_dir_all(&child_dir).unwrap();
        fs::create_dir_all(&grandchild_dir).unwrap();
        fs::write(
            parent_dir.join("SKILL.md"),
            format!(
                "---\nname: parent\n---\n\nUse `$ignored`; load `{}`.\n",
                child_dir.join("SKILL.md").display()
            ),
        )
        .unwrap();
        fs::write(ignored_dir.join("SKILL.md"), "---\nname: ignored\n---\n").unwrap();
        fs::write(
            child_dir.join("SKILL.md"),
            "---\nname: renamed-child\n---\n\nRead `../grandchild/SKILL.md`.\n",
        )
        .unwrap();
        fs::write(
            grandchild_dir.join("SKILL.md"),
            "---\nname: grandchild\n---\n",
        )
        .unwrap();

        let scan = scan_skills(&root).unwrap();
        let parent = scan
            .skills
            .iter()
            .find(|skill| skill.name == "parent")
            .unwrap();
        let child = scan
            .skills
            .iter()
            .find(|skill| skill.name == "renamed-child")
            .unwrap();
        let ignored = scan
            .skills
            .iter()
            .find(|skill| skill.name == "ignored")
            .unwrap();
        let grandchild = scan
            .skills
            .iter()
            .find(|skill| skill.name == "grandchild")
            .unwrap();

        assert_eq!(parent.dependencies, vec!["renamed-child"]);
        assert_eq!(child.dependents, vec!["parent"]);
        assert_eq!(child.dependencies, vec!["grandchild"]);
        assert_eq!(grandchild.dependents, vec!["renamed-child"]);
        assert!(ignored.dependents.is_empty());
        assert!(
            child
                .paths
                .iter()
                .any(|path| { path.agent == AgentKind::Unknown && path.scope == "referenced" })
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_skills_reports_wrapper_catalog_dependencies() {
        let root = temp_dir("tendi-scan-wrapper-relations-test");
        let wrapper_dir = root.join(".agents/skills/wrapper");
        let child_dir = root.join(".agents/skills/child");
        let other_child_dir = root.join(".agents/skills/other-child");
        fs::create_dir_all(&wrapper_dir).unwrap();
        fs::create_dir_all(&child_dir).unwrap();
        fs::create_dir_all(&other_child_dir).unwrap();
        fs::write(
            wrapper_dir.join("SKILL.md"),
            format!(
                "---\nname: wrapper\ntags:\n  - wrapper\n---\n\n# wrapper\n\n## Route\n\n{WRAPPER_CATALOG_START}\n- [`child`](<{}/SKILL.md>): 中文描述，没有英文触发词。\n- [`other-child`](<{}/SKILL.md>): 更多中文说明。\n{WRAPPER_CATALOG_END}\n",
                child_dir.display(),
                other_child_dir.display()
            ),
        )
        .unwrap();
        fs::write(child_dir.join("SKILL.md"), "---\nname: child\n---\n").unwrap();
        fs::write(
            other_child_dir.join("SKILL.md"),
            "---\nname: other-child\n---\n",
        )
        .unwrap();

        let scan = scan_skills(&root).unwrap();
        let wrapper = scan
            .skills
            .iter()
            .find(|skill| skill.name == "wrapper")
            .unwrap();
        let child = scan
            .skills
            .iter()
            .find(|skill| skill.name == "child")
            .unwrap();
        let other_child = scan
            .skills
            .iter()
            .find(|skill| skill.name == "other-child")
            .unwrap();

        assert_eq!(wrapper.dependencies, vec!["child", "other-child"]);
        assert_eq!(child.dependents, vec!["wrapper"]);
        assert_eq!(other_child.dependents, vec!["wrapper"]);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_skills_reports_markdown_route_dependencies() {
        let root = temp_dir("tendi-scan-markdown-route-relations-test");
        let wrapper_dir = root.join(".agents/skills/wrapper");
        let child_dir = root.join(".agents/skills/child");
        fs::create_dir_all(&wrapper_dir).unwrap();
        fs::create_dir_all(&child_dir).unwrap();
        fs::write(
            wrapper_dir.join("SKILL.md"),
            "---\nname: wrapper\ntags:\n  - wrapper\n---\n\n# wrapper\n\n## Routes\n\n- [`child`](../child/SKILL.md): Route child requests.\n",
        )
        .unwrap();
        fs::write(child_dir.join("SKILL.md"), "---\nname: child\n---\n").unwrap();

        let scan = scan_skills(&root).unwrap();
        let wrapper = scan
            .skills
            .iter()
            .find(|skill| skill.name == "wrapper")
            .unwrap();
        let child = scan
            .skills
            .iter()
            .find(|skill| skill.name == "child")
            .unwrap();

        assert_eq!(wrapper.dependencies, vec!["child"]);
        assert_eq!(child.dependents, vec!["wrapper"]);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn delete_plan_reports_dependency_impact() {
        let root = temp_dir("tendi-delete-skill-relations-test");
        let parent_dir = root.join(".agents/skills/parent");
        let child_dir = root.join(".agents/skills/child");
        fs::create_dir_all(&parent_dir).unwrap();
        fs::create_dir_all(&child_dir).unwrap();
        fs::write(
            parent_dir.join("SKILL.md"),
            "---\nname: parent\n---\n\nUse [`child`](../child/SKILL.md).\n",
        )
        .unwrap();
        fs::write(child_dir.join("SKILL.md"), "---\nname: child\n---\n").unwrap();

        let plan = plan_skill_delete_many(&root, &["child".to_string()]).unwrap();

        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.dependents[0].name, "child");
        assert_eq!(plan.dependents[0].related, vec!["parent"]);
        assert!(format_delete_plan(&plan).contains("child is used by parent"));

        let _ = fs::remove_dir_all(root);
    }


    #[test]
    fn add_plan_marks_existing_targets_as_replace_when_overwrite_is_enabled() {
        let root = temp_dir("tendi-plan-overwrite-add-skills-test");
        let repo = root.join("repo");
        let skill_dir = repo.join("skills/demo");
        let target_root = root.join("target");
        let target_dir = target_root.join("demo");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: Demo skill\n---\n\n# demo\n",
        )
        .unwrap();
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(
            target_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: Existing demo\n---\n\n# demo\n",
        )
        .unwrap();

        let resolved = ResolvedAddSource {
            root: repo.canonicalize().unwrap(),
            kind: "local".to_string(),
            display_source: "repo".to_string(),
            git_ref: None,
            temporary: false,
        };
        let plan = build_skill_add_plan_with_target_root(
            &resolved,
            &super::SkillAddOptions {
                source: "repo".to_string(),
                target: AgentKind::Shared.into(),
                scope: SkillInstallScope::Global,
                skills: vec!["demo".to_string()],
                copy: false,
                overwrite: true,
                visibility: SkillVisibility::Auto,
            },
            true,
            &target_root,
        )
        .unwrap();
        assert_eq!(plan.operations[0].status, "replace");
        assert!(
            plan.operations[0]
                .message
                .as_deref()
                .unwrap_or("")
                .contains("will replace existing target")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn materialize_skill_dir_sanitizes_target_name_and_copies() {
        let root = temp_dir("tendi-materialize-add-test");
        let source = root.join("source");
        let target_root = root.join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target_root).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: demo\ndescription: Demo\n---\n\n# demo\n",
        )
        .unwrap();

        let result = materialize_skill_dir_to_root(
            &source,
            &target_root,
            "../Demo Skill",
            true,
            false,
            false,
        )
        .unwrap();
        assert_eq!(result.mode, "copy");
        assert!(target_root.join("demo-skill/SKILL.md").is_file());
        assert!(!target_root.join("../Demo Skill").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn materialize_skill_dir_overwrites_existing_target_when_requested() {
        let root = temp_dir("tendi-materialize-overwrite-add-test");
        let source = root.join("source");
        let target_root = root.join("target");
        let target = target_root.join("demo");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: demo\ndescription: New\n---\n\n# new\n",
        )
        .unwrap();
        fs::write(
            target.join("SKILL.md"),
            "---\nname: demo\ndescription: Old\n---\n\n# old\n",
        )
        .unwrap();

        let result =
            materialize_skill_dir_to_root(&source, &target_root, "demo", true, true, false)
                .unwrap();
        assert_eq!(result.mode, "copy");
        assert!(result.applied);
        assert!(
            fs::read_to_string(target.join("SKILL.md"))
                .unwrap()
                .contains("# new")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_distribution_moves_and_links_existing_installations() {
        let root = temp_dir("tendi-skill-distribution-test");
        let source = root.join("codex/demo");
        let moved = root.join("shared/demo");
        let linked = root.join("cursor/demo");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("SKILL.md"), "---\nname: demo\n---\n").unwrap();

        let source_record = || SkillSourceRecord {
            skill_name: "demo".to_string(),
            skill_path: source.clone(),
            source_kind: "local".to_string(),
            source: None,
            source_ref: None,
            source_version: None,
            source_relative_path: None,
            update_status: "local".to_string(),
            origin: "test".to_string(),
        };
        let move_plan = SkillDistributionPlan {
            name: "demo".to_string(),
            source: source.clone(),
            destination: moved.clone(),
            mode: SkillDistributionMode::Move,
            source_symlink: false,
            destination_exists: false,
            source_sha256: sha256_file(&source.join("SKILL.md")).unwrap(),
            status: "ready".to_string(),
            message: None,
            source_record: source_record(),
        };
        let moved_result = apply_skill_distribution_plan(&move_plan).unwrap();
        assert_eq!(moved_result.mode, "move");
        assert!(!source.exists());
        assert!(moved.join("SKILL.md").is_file());

        let link_source = root.join("link-source");
        fs::create_dir_all(&link_source).unwrap();
        fs::write(link_source.join("SKILL.md"), "---\nname: link\n---\n").unwrap();
        let link_plan = SkillDistributionPlan {
            name: "link".to_string(),
            source: link_source.clone(),
            destination: linked.clone(),
            mode: SkillDistributionMode::Symlink,
            source_symlink: false,
            destination_exists: false,
            source_sha256: sha256_file(&link_source.join("SKILL.md")).unwrap(),
            status: "ready".to_string(),
            message: None,
            source_record: source_record(),
        };
        let linked_result = apply_skill_distribution_plan(&link_plan).unwrap();
        assert_eq!(linked_result.mode, "symlink");
        assert!(link_source.join("SKILL.md").is_file());
        assert!(linked.join("SKILL.md").is_file());
        assert!(
            fs::symlink_metadata(&linked)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        let _ = fs::remove_dir_all(root);
    }


    #[test]
    fn sanitize_skill_dir_name_blocks_empty_and_traversal_names() {
        assert_eq!(sanitize_skill_dir_name("../Demo Skill").unwrap(), "demo-skill");
        assert!(sanitize_skill_dir_name("////").is_err());
    }


    #[test]
    fn registry_source_scan_and_update_check_are_recorded() {
        let root = temp_dir("tendi-registry-source-test");
        let skill_dir = root.join(".agents/skills/demo");
        fs::create_dir_all(&skill_dir).unwrap();
        let registry_file = root.join("registry-demo.md");
        fs::write(
            &registry_file,
            "---\nname: demo\nversion: 2.0.0\n---\n\n# demo\n",
        )
        .unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: demo\ndescription: Demo\nversion: 1.0.0\nsource: file://{}\n---\n\n# demo\n",
                registry_file.display()
            ),
        )
        .unwrap();

        let scan = scan_skills(&root).unwrap();
        let skill = scan
            .skills
            .iter()
            .find(|skill| skill.name == "demo")
            .unwrap();
        assert_eq!(skill.update_status, "checkable");
        assert_eq!(
            skill.source_summary,
            format!("registry:file://{}", registry_file.display())
        );
        assert_eq!(skill.paths[0].source_kind, "registry");
        assert_eq!(
            skill.paths[0].source.as_deref(),
            Some(format!("file://{}", registry_file.display()).as_str())
        );
        assert_eq!(skill.paths[0].source_version.as_deref(), Some("1.0.0"));

        let updates = check_skill_updates(&root).unwrap();
        let update = updates.iter().find(|update| update.name == "demo").unwrap();
        assert_eq!(update.status, "update-available");
        assert_eq!(update.current_version.as_deref(), Some("1.0.0"));
        assert_eq!(update.latest_version.as_deref(), Some("2.0.0"));
        assert_eq!(update.source_kind, "registry");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_update_check_does_not_touch_fetch_head() {
        let root = temp_dir("tendi-update-check-fetch-head");
        let remote = root.join("remote");
        let local = root.join("local");
        fs::create_dir_all(remote.join("skills/demo")).unwrap();
        fs::write(
            remote.join("skills/demo/SKILL.md"),
            "---\nname: demo\ndescription: one\n---\n",
        )
        .unwrap();
        run_test_git(&remote, &["init", "--quiet"]);
        run_test_git(&remote, &["config", "user.email", "tendi@example.test"]);
        run_test_git(&remote, &["config", "user.name", "Tendi Test"]);
        run_test_git(&remote, &["add", "."]);
        run_test_git(&remote, &["commit", "--quiet", "-m", "one"]);
        let output = Command::new("git")
            .args(["clone", "--quiet"])
            .arg(&remote)
            .arg(&local)
            .output()
            .unwrap();
        assert!(output.status.success());
        fs::write(
            remote.join("skills/demo/SKILL.md"),
            "---\nname: demo\ndescription: two\n---\n",
        )
        .unwrap();
        run_test_git(&remote, &["add", "."]);
        run_test_git(&remote, &["commit", "--quiet", "-m", "two"]);
        let fetch_head = local.join(".git/FETCH_HEAD");
        fs::write(&fetch_head, "sentinel\n").unwrap();

        let remote_head = super::fetch_git_remote_head(
            &local,
            &remote.display().to_string(),
            None,
            super::git::never_cancelled(),
        )
        .expect("fetch remote head");

        assert_eq!(fs::read_to_string(&fetch_head).unwrap(), "sentinel\n");
        assert_ne!(
            run_test_git(&local, &["rev-parse", "HEAD"]),
            remote_head.oid
        );
        assert!(
            !run_test_git(&local, &["diff", "--name-only", "HEAD", &remote_head.oid])
                .trim()
                .is_empty()
        );
        let reference = remote_head.reference.clone();
        super::cleanup_git_remote_heads(BTreeMap::from([(local.clone(), Some(remote_head))]));
        let status = Command::new("git")
            .args(["show-ref", "--verify", "--quiet", &reference])
            .current_dir(&local)
            .status()
            .unwrap();
        assert!(!status.success());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn up_to_date_remote_check_does_not_fetch_remote_objects() {
        let root = temp_dir("tendi-update-check-ls-remote");
        let remote = root.join("remote");
        let local = root.join("local");
        fs::create_dir_all(remote.join("skills/demo")).unwrap();
        fs::write(
            remote.join("skills/demo/SKILL.md"),
            "---\nname: demo\ndescription: one\n---\n",
        )
        .unwrap();
        run_test_git(&remote, &["init", "--quiet"]);
        run_test_git(&remote, &["config", "user.email", "tendi@example.test"]);
        run_test_git(&remote, &["config", "user.name", "Tendi Test"]);
        run_test_git(&remote, &["add", "."]);
        run_test_git(&remote, &["commit", "--quiet", "-m", "one"]);
        let output = Command::new("git")
            .args(["clone", "--quiet"])
            .arg(&remote)
            .arg(&local)
            .output()
            .unwrap();
        assert!(output.status.success());

        let current_version = run_test_git(&local, &["rev-parse", "HEAD"]);
        let skill_dir = local.join("skills/demo");
        let mut skill = test_skill("demo", "Demo", &skill_dir);
        let path = &mut skill.paths[0];
        path.source_kind = "git".to_string();
        path.source = Some(remote.display().to_string());
        path.source_version = Some(current_version);
        path.source_relative_path = Some("skills/demo".to_string());
        path.update_status = "checkable".to_string();
        let fetch_head = local.join(".git/FETCH_HEAD");
        fs::write(&fetch_head, "sentinel\n").unwrap();

        let updates =
            super::check_skill_updates_for_skills(&[&skill], super::git::never_cancelled());

        assert_eq!(updates[0].status, "up-to-date");
        assert_eq!(fs::read_to_string(fetch_head).unwrap(), "sentinel\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn git_update_clears_tracked_and_untracked_tendi_policy_changes() {
        let root = temp_dir("tendi-git-update-settings-test");
        let tracked_skill = root.join("skills/tracked-policy");
        let untracked_skill = root.join("skills/untracked-policy");
        fs::create_dir_all(tracked_skill.join("agents")).unwrap();
        fs::create_dir_all(&untracked_skill).unwrap();
        let baseline_skill = "---\nname: demo\ndescription: Demo\n---\n\n# Demo\n";
        fs::write(tracked_skill.join("SKILL.md"), baseline_skill).unwrap();
        fs::write(untracked_skill.join("SKILL.md"), baseline_skill).unwrap();
        let baseline_policy = concat!(
            "interface:\n",
            "  display_name: \"Demo\"\n",
            "policy:\n",
            "  allow_implicit_invocation: false\n",
        );
        fs::write(tracked_skill.join("agents/openai.yaml"), baseline_policy).unwrap();

        run_test_git(&root, &["init", "--quiet"]);
        run_test_git(&root, &["config", "user.email", "tendi@example.test"]);
        run_test_git(&root, &["config", "user.name", "Tendi Test"]);
        run_test_git(&root, &["add", "."]);
        run_test_git(&root, &["commit", "--quiet", "-m", "baseline"]);

        let initial_changes = vec![
            super::plan_skill_frontmatter_for_agent(
                tracked_skill.join("SKILL.md"),
                AgentKind::Codex,
                SkillVisibility::Manual,
            )
            .unwrap(),
            crate::providers::codex::plan_skill_policy_file(
                tracked_skill.join("agents/openai.yaml"),
                SkillVisibility::Auto,
            )
            .unwrap(),
            super::plan_skill_frontmatter_for_agent(
                untracked_skill.join("SKILL.md"),
                AgentKind::Codex,
                SkillVisibility::Manual,
            )
            .unwrap(),
            crate::providers::codex::plan_skill_policy_file(
                untracked_skill.join("agents/openai.yaml"),
                SkillVisibility::Manual,
            )
            .unwrap(),
        ];
        super::apply_changes(&ChangeSet {
            changes: initial_changes,
        })
        .unwrap();
        let manual_policy = crate::providers::codex::plan_skill_policy_file(
            tracked_skill.join("agents/openai.yaml"),
            SkillVisibility::Manual,
        )
        .unwrap();
        super::apply_changes(&ChangeSet {
            changes: vec![manual_policy],
        })
        .unwrap();
        let dirty = run_test_git(&root, &["status", "--short"]);
        assert!(dirty.contains("skills/tracked-policy/agents/openai.yaml"));
        assert!(untracked_skill.join("agents/openai.yaml").is_file());

        clear_tendi_git_changes(&GitUpdateAction {
            name: "demo".to_string(),
            skill_names: vec!["demo".to_string()],
            repo: root.clone(),
            source: root.display().to_string(),
            source_ref: None,
            current_version: None,
            latest_version: None,
            diff: String::new(),
            files: Vec::new(),
            tendi_settings: vec![
                GitSkillVisibility {
                    skill_dir: tracked_skill.clone(),
                    agent: AgentKind::Codex,
                    visibility: SkillVisibility::Manual,
                },
                GitSkillVisibility {
                    skill_dir: untracked_skill.clone(),
                    agent: AgentKind::Codex,
                    visibility: SkillVisibility::Manual,
                },
            ],
            materialized_targets: Vec::new(),
        })
        .unwrap();

        assert_eq!(run_test_git(&root, &["status", "--short"]), "");
        assert_eq!(
            fs::read_to_string(tracked_skill.join("agents/openai.yaml")).unwrap(),
            baseline_policy
        );
        assert!(!untracked_skill.join("agents/openai.yaml").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn git_update_restores_tendi_settings_when_fetch_fails() {
        let root = temp_dir("tendi-git-update-restore-test");
        let skill_dir = root.join("skills/demo");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: Demo\n---\n\n# Demo\n",
        )
        .unwrap();
        run_test_git(&root, &["init", "--quiet"]);
        run_test_git(&root, &["config", "user.email", "tendi@example.test"]);
        run_test_git(&root, &["config", "user.name", "Tendi Test"]);
        run_test_git(&root, &["add", "."]);
        run_test_git(&root, &["commit", "--quiet", "-m", "baseline"]);

        let changes = vec![
            crate::providers::cursor::plan_skill_frontmatter(
                skill_dir.join("SKILL.md"),
                SkillVisibility::Manual,
            )
            .unwrap(),
            crate::providers::codex::plan_skill_policy_file(
                skill_dir.join("agents/openai.yaml"),
                SkillVisibility::Manual,
            )
            .unwrap(),
        ];
        super::apply_changes(&ChangeSet { changes }).unwrap();
        let action = GitUpdateAction {
            name: "demo".to_string(),
            skill_names: vec!["demo".to_string()],
            repo: root.clone(),
            source: root.display().to_string(),
            source_ref: None,
            current_version: None,
            latest_version: None,
            diff: String::new(),
            files: Vec::new(),
            tendi_settings: vec![GitSkillVisibility {
                skill_dir: skill_dir.clone(),
                agent: AgentKind::Codex,
                visibility: SkillVisibility::Manual,
            }],
            materialized_targets: Vec::new(),
        };

        let error = apply_git_update(&action).unwrap_err();
        assert!(format!("{error:#}").contains("fetch"));
        let restored_skill = fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
        assert!(restored_skill.contains("visibility: manual"));
        let restored_policy = fs::read_to_string(skill_dir.join("agents/openai.yaml")).unwrap();
        assert!(restored_policy.contains("allow_implicit_invocation: false"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn materialized_git_update_replaces_copy_and_advances_database_source() {
        let root = temp_dir("tendi-materialized-git-update-test");
        let repo = root.join("repo");
        let source_skill = repo.join("skills/demo");
        let target = root.join("project/.agents/skills/demo");
        fs::create_dir_all(&source_skill).unwrap();
        fs::write(
            source_skill.join("SKILL.md"),
            "---\nname: demo\ndescription: old\n---\n\n# old\n",
        )
        .unwrap();
        run_test_git(&repo, &["init", "--quiet"]);
        run_test_git(&repo, &["config", "user.email", "tendi@example.test"]);
        run_test_git(&repo, &["config", "user.name", "Tendi Test"]);
        run_test_git(&repo, &["add", "."]);
        run_test_git(&repo, &["commit", "--quiet", "-m", "old"]);
        copy_dir(&source_skill, &target).unwrap();
        let old = run_test_git(&repo, &["rev-parse", "HEAD"]);

        fs::write(
            source_skill.join("SKILL.md"),
            "---\nname: demo\ndescription: new\n---\n\n# new\n",
        )
        .unwrap();
        run_test_git(&repo, &["add", "."]);
        run_test_git(&repo, &["commit", "--quiet", "-m", "new"]);
        let latest = run_test_git(&repo, &["rev-parse", "HEAD"]);

        let store = crate::storage::Store::open(root.join("tendi.sqlite3")).unwrap();
        store
            .upsert_skill_source_records(&[SkillSourceRecord {
                skill_name: "demo".to_string(),
                skill_path: target.clone(),
                source_kind: "github".to_string(),
                source: Some(repo.display().to_string()),
                source_ref: None,
                source_version: Some(old),
                source_relative_path: Some("skills/demo/SKILL.md".to_string()),
                update_status: "tracked".to_string(),
                origin: "skills-cli-lock".to_string(),
            }])
            .unwrap();
        let action = GitUpdateAction {
            name: "demo".to_string(),
            skill_names: vec!["demo".to_string()],
            repo: repo.clone(),
            source: repo.display().to_string(),
            source_ref: None,
            current_version: None,
            latest_version: Some(latest.clone()),
            diff: String::new(),
            files: Vec::new(),
            tendi_settings: Vec::new(),
            materialized_targets: vec![MaterializedGitTarget {
                name: "demo".to_string(),
                target: target.clone(),
                agent: AgentKind::Shared,
                source_relative_path: Some("skills/demo/SKILL.md".to_string()),
                visibility: SkillVisibility::Auto,
                uses_shared_layout: true,
                files: Vec::new(),
            }],
        };

        apply_skill_update_plan_with_store(
            &SkillUpdatePlan {
                file_changes: ChangeSet {
                    changes: Vec::new(),
                },
                git_updates: vec![action],
                skipped: Vec::new(),
                source_updates: Vec::new(),
                merge_issues: Vec::new(),
            },
            &store,
        )
        .unwrap();

        assert!(
            fs::read_to_string(target.join("SKILL.md"))
                .unwrap()
                .contains("# new")
        );
        assert_eq!(
            store
                .skill_source_record(&target)
                .unwrap()
                .unwrap()
                .source_version
                .as_deref(),
            Some(latest.as_str())
        );
        fs::remove_dir_all(root).unwrap();
    }













    #[test]
    fn wrapper_refresh_preserves_manual_content() {
        let root = Path::new("/tmp/tendi-skills");
        let lark_im = test_skill("lark-im", "Send Lark messages", &root.join("lark-im"));
        let lark_doc = test_skill("lark-doc", "Read Lark docs", &root.join("lark-doc"));
        let before = r#"---
name: lark
version: 1.0.0
description: Custom Lark router.
tendi:
  wrapper_description: Use when the user needs custom Lark routing across chat and docs.
---

# Lark Router

Keep this handmade intro.

## Root

```text
/old/root
```

## Route

- `lark-old`: Old route.

## Procedure

1. Preserve this exact workflow.
"#;
        let output = render_wrapper_after("lark", &[&lark_im, &lark_doc], Some(before));

        assert!(output.contains("version: 1.0.0"));
        assert!(output.contains("description: Custom Lark router."));
        assert!(output.contains(
            "wrapper_description: Use when the user needs custom Lark routing across chat and docs."
        ));
        assert!(output.contains("Keep this handmade intro."));
        assert!(output.contains("## Procedure"));
        assert!(output.contains("1. Preserve this exact workflow."));
        assert!(output.contains("## Root"));
        assert!(output.contains("/old/root"));
        assert!(output.contains(WRAPPER_CATALOG_START));
        assert!(output.contains(WRAPPER_CATALOG_END));
        assert!(
            output.contains(
                "- [`lark-im`](</tmp/tendi-skills/lark-im/SKILL.md>): Send Lark messages"
            )
        );
        assert!(
            output
                .contains("- [`lark-doc`](</tmp/tendi-skills/lark-doc/SKILL.md>): Read Lark docs")
        );
        assert!(!output.contains("lark-old"));
    }

    #[test]
    fn scan_skills_synced_refreshes_wrapper_from_child_descriptions() {
        let root = temp_dir("tendi-wrapper-sync-test");
        let skills_root = root.join(".agents/skills");
        let child_dir = skills_root.join("tendi-sync-child");
        let wrapper_dir = skills_root.join("tendi-sync-wrapper");
        fs::create_dir_all(&child_dir).unwrap();
        fs::create_dir_all(&wrapper_dir).unwrap();
        fs::write(
            child_dir.join("SKILL.md"),
            "---\nname: tendi-sync-child\ndescription: Send updated sync messages.\n---\n\n# tendi-sync-child\n",
        )
        .unwrap();
        fs::write(
            wrapper_dir.join("SKILL.md"),
            format!(
                "---\nname: tendi-sync-wrapper\ndescription: Old wrapper.\ntags:\n  - wrapper\n---\n\n# tendi-sync-wrapper\n\nKeep this intro.\n\n## Route\n\n- [`tendi-sync-child`](<{}>): Send stale sync messages.\n\n## Procedure\n\n1. Keep this workflow.\n",
                child_dir.join("SKILL.md").display()
            ),
        )
        .unwrap();

        let scan = crate::skills::scan_skills_synced(&root).unwrap();
        let wrapper_text = fs::read_to_string(wrapper_dir.join("SKILL.md")).unwrap();
        let wrapper = scan
            .skills
            .iter()
            .find(|skill| skill.name == "tendi-sync-wrapper")
            .unwrap();

        assert_eq!(wrapper.description.as_deref(), Some("Old wrapper."));
        assert!(wrapper_text.contains("description: Old wrapper."));
        assert!(wrapper_text.contains("Keep this intro."));
        assert!(wrapper_text.contains(WRAPPER_CATALOG_START));
        assert!(wrapper_text.contains(WRAPPER_CATALOG_END));
        assert!(wrapper_text.contains("- [`tendi-sync-child`]"));
        assert!(wrapper_text.contains("Send updated sync messages."));
        assert!(!wrapper_text.contains("Send stale sync messages."));
        assert!(wrapper_text.contains("1. Keep this workflow."));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn visibility_off_is_persisted_in_tendi_frontmatter() {
        let path = std::env::temp_dir().join(format!(
            "tendi-visibility-test-{}.md",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, "---\nname: demo\n---\n\n# demo\n").unwrap();

        let change = crate::providers::cursor::plan_skill_frontmatter(
            path.clone(),
            SkillVisibility::Off,
        )
        .unwrap();
        let doc = MarkdownDoc::parse(&change.after).unwrap();
        let meta = Value::Mapping(doc.meta);

        assert_eq!(parse_tendi_visibility(&meta), Some(SkillVisibility::Off));
        assert_eq!(
            meta.get("disable-model-invocation")
                .and_then(Value::as_bool),
            Some(true)
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn non_codex_skill_ignores_codex_skill_config() {
        let root = temp_dir("tendi-codex-skill-config-disabled-test");
        let skills_root = root.join(".agents/skills");
        let skill_dir = skills_root.join("pr");
        let skill_file = skill_dir.join("SKILL.md");
        let config_path = root.join(".codex/config.toml");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &skill_file,
            "---\nname: pr\ndescription: PR workflow\n---\n\n# PR\n",
        )
        .unwrap();
        fs::write(
            &config_path,
            format!(
                "[[skills.config]]\npath = \"{}\"\nenabled = false\n",
                skill_file.display()
            ),
        )
        .unwrap();

        let root_record = super::SkillRoot {
            path: skills_root,
            scope: "global".to_string(),
            agent: AgentKind::Shared,
            plugin_id: None,
            plugin_enabled: None,
        };
        let skill = super::read_skill(
            &root_record,
            &skill_file,
            &mut super::ProvenanceResolver::default(),
        )
        .unwrap();

        assert_eq!(skill.path.provider_skill_enabled, None);
        assert_eq!(skill.path.effective_visibility, SkillVisibility::Auto);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manual_visibility_reenables_disabled_codex_skill_config() {
        let root = temp_dir("tendi-codex-skill-config-reenable-test");
        let config_path = root.join(".codex/config.toml");
        let skill_file = root.join(".agents/skills/pr/SKILL.md");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            format!(
                "[[skills.config]]\npath = \"{}\"\nenabled = false\n",
                skill_file.display()
            ),
        )
        .unwrap();

        let change = crate::providers::codex::plan_skill_config_at(
            config_path.clone(),
            skill_file,
            SkillVisibility::Manual,
        )
        .unwrap()
        .unwrap();
        let parsed = toml::from_str::<toml::Value>(&change.after).unwrap();
        let enabled = parsed
            .get("skills")
            .and_then(|skills| skills.get("config"))
            .and_then(toml::Value::as_array)
            .and_then(|configs| configs.first())
            .and_then(|config| config.get("enabled"))
            .and_then(toml::Value::as_bool);

        assert_eq!(enabled, Some(true));

        let _ = fs::remove_dir_all(root);
    }


    #[test]
    fn off_visibility_preserves_unrelated_frontmatter_formatting() {
        let path = std::env::temp_dir().join(format!(
            "tendi-off-minimal-test-{}.md",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let before = "---\nname: demo\ndescription: \"Demo\"\ndisable-model-invocation: true\nmetadata:\n  bins: [\"demo\"]\n---\n\n# demo\n";
        fs::write(&path, before).unwrap();

        let change = crate::providers::cursor::plan_skill_frontmatter(
            path.clone(),
            SkillVisibility::Off,
        )
        .unwrap();

        assert!(change.after.contains("description: \"Demo\""));
        assert!(change.after.contains("  bins: [\"demo\"]"));
        assert!(change.after.contains("tendi:\n  visibility: off"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn scan_reads_tendi_visibility_off() {
        let root = std::env::temp_dir().join(format!(
            "tendi-scan-visibility-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let skill_dir = root.join(".agents/skills/tendi-off-demo");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: tendi-off-demo\ntendi:\n  visibility: off\ndisable-model-invocation: true\n---\n\n# demo\n",
        )
        .unwrap();

        let scan = scan_skills(&root).unwrap();
        let skill = scan
            .skills
            .iter()
            .find(|skill| skill.name == "tendi-off-demo")
            .unwrap();

        assert_eq!(skill.visibility, SkillVisibility::Off);

        let _ = fs::remove_dir_all(root);
    }



    #[test]
    fn merge_skill_reports_mixed_visibility_for_hybrid_sources() {
        let shared_path = test_skill_path(
            "/tmp/tendi-skills/hybrid",
            AgentKind::Shared,
            SkillVisibility::Auto,
            None,
        );
        let plugin_path = test_skill_path(
            "/tmp/codex/plugins/cache/openai-bundled/browser/1.0.0/skills/hybrid",
            AgentKind::Codex,
            SkillVisibility::Off,
            Some(false),
        );

        let skill = super::merge_skill(
            "hybrid".to_string(),
            vec![
                super::RawSkill {
                    name: "hybrid".to_string(),
                    description: Some("Hybrid".to_string()),
                    tags: Vec::new(),
                    dependencies: Vec::new(),
                    dependency_files: Vec::new(),
                    is_system: false,
                    path: shared_path,
                },
                super::RawSkill {
                    name: "hybrid".to_string(),
                    description: Some("Hybrid".to_string()),
                    tags: Vec::new(),
                    dependencies: Vec::new(),
                    dependency_files: Vec::new(),
                    is_system: true,
                    path: plugin_path,
                },
            ],
        );

        assert_eq!(skill.visibility, SkillVisibility::Mixed);
        assert!(skill.paths.iter().any(|path| {
            path.agent == AgentKind::Shared && path.effective_visibility == SkillVisibility::Auto
        }));
        assert!(skill.paths.iter().any(|path| {
            path.agent == AgentKind::Codex && path.effective_visibility == SkillVisibility::Off
        }));
    }

    #[test]
    fn skill_delete_plan_removes_selected_skill_directory() {
        let root = temp_dir("tendi-delete-skill-test");
        let skill_dir = root.join(".agents/skills/delete-demo");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: delete-demo\ndescription: Demo\n---\n\n# delete-demo\n",
        )
        .unwrap();

        let plan = plan_skill_delete_many(&root, &["delete-demo".to_string()]).unwrap();
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].name, "delete-demo");
        assert_eq!(plan.targets[0].kind, "directory");
        assert!(format_delete_plan(&plan).contains("D delete-demo"));

        apply_skill_delete_plan(&plan).unwrap();
        assert!(!skill_dir.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_local_skill_reports_canonical_source() {
        let root = std::env::temp_dir().join(format!(
            "tendi-scan-symlink-source-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source_dir = root.join(".agents/skills/demo");
        let target_root = root.join(".claude/skills");
        let target_dir = target_root.join("demo");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&target_root).unwrap();
        fs::write(
            source_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: Demo\n---\n\n# demo\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(&source_dir, &target_dir).unwrap();

        let scan = scan_skills(&root).unwrap();
        let skill = scan
            .skills
            .iter()
            .find(|skill| skill.name == "demo")
            .unwrap();
        let claude_path = skill
            .paths
            .iter()
            .find(|path| path.agent == AgentKind::Claude)
            .unwrap();

        assert_eq!(claude_path.symlink_status, "symlink-ok");
        assert_eq!(
            claude_path.source.as_deref(),
            Some(source_dir.canonicalize().unwrap().to_str().unwrap())
        );

        let _ = fs::remove_dir_all(root);
    }


    #[test]
    fn project_skills_cli_lock_migrates_once_into_source_database() {
        let root = temp_dir("tendi-project-skills-cli-lock-test");
        let skill_dir = root.join(".agents/skills/demo");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: Demo\n---\n\n# demo\n",
        )
        .unwrap();
        fs::write(
            root.join("skills-lock.json"),
            r#"{
  "version": 1,
  "skills": {
    "demo": {
      "source": "example/agent-skills",
      "sourceType": "github",
      "ref": "release",
      "skillPath": "skills/demo/SKILL.md",
      "computedHash": "content-hash"
    }
  }
}
"#,
        )
        .unwrap();
        run_test_git(&root, &["init"]);
        run_test_git(
            &root,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/example/host-project.git",
            ],
        );

        let store = crate::storage::Store::open(root.join("tendi.sqlite3")).unwrap();
        let scanned = super::scan_skills_synced_for_project_roots_with_store_for_projection(
            &root,
            &store,
            &[],
        )
        .unwrap();
        let path = &scanned
            .scan
            .skills
            .iter()
            .find(|skill| skill.name == "demo")
            .unwrap()
            .paths[0];
        assert_eq!(path.source_kind, "github");
        assert_eq!(
            path.source.as_deref(),
            Some("https://github.com/example/agent-skills.git")
        );
        assert_eq!(
            path.source_relative_path.as_deref(),
            Some("skills/demo/SKILL.md")
        );
        assert_eq!(path.source_version.as_deref(), Some("content-hash"));
        assert_eq!(path.update_status, "tracked");

        assert!(store.skill_source_records().unwrap().is_empty());
        store
            .save_skills_for_workspace_with_source_migrations(
                &root,
                &scanned.scan,
                &scanned.source_migrations,
            )
            .unwrap();

        let records = store.skill_source_records().unwrap();
        let migrated = records
            .iter()
            .filter(|record| record.skill_name == "demo")
            .collect::<Vec<_>>();
        assert_eq!(migrated.len(), 1, "{records:#?}");
        assert_eq!(migrated[0].origin, "skills-cli-lock");
        assert_eq!(migrated[0].source_ref.as_deref(), Some("release"));

        fs::write(
            root.join("skills-lock.json"),
            r#"{
  "version": 1,
  "skills": {
    "demo": {
      "source": "example/changed-after-migration",
      "sourceType": "github",
      "skillPath": "skills/changed/SKILL.md",
      "computedHash": "changed-hash"
    }
  }
}
"#,
        )
        .unwrap();
        let rescanned = super::scan_skills_with_source_store(&root, &store).unwrap();
        let persisted_path = &rescanned
            .skills
            .iter()
            .find(|skill| skill.name == "demo")
            .unwrap()
            .paths[0];
        assert_eq!(
            persisted_path.source.as_deref(),
            Some("https://github.com/example/agent-skills.git")
        );
        assert_eq!(
            persisted_path.source_relative_path.as_deref(),
            Some("skills/demo/SKILL.md")
        );
        assert_eq!(
            persisted_path.source_version.as_deref(),
            Some("content-hash")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scan_skills_includes_additional_project_roots() {
        let cwd = temp_dir("tendi-skills-scan-cwd");
        let project = temp_dir("tendi-skills-scan-project");
        let skill_dir = project.join(".agents/skills/project-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: project-skill\ndescription: Project skill\n---\n",
        )
        .unwrap();

        let store = crate::storage::Store::open(cwd.join("tendi.sqlite3")).unwrap();
        let scan = super::scan_skills_with_source_store_for_projects(
            &cwd,
            &store,
            std::slice::from_ref(&project),
        )
        .unwrap();

        let skill = scan
            .skills
            .iter()
            .find(|skill| skill.name == "project-skill")
            .unwrap();
        let skill_dir = skill_dir.canonicalize().unwrap();
        assert!(
            skill.paths.iter().any(|path| path.path == skill_dir),
            "skill paths: {:#?}; expected: {}",
            skill.paths,
            skill_dir.display()
        );
        store.save_skills_for_workspace(&cwd, &scan).unwrap();
        let persisted = store.list_skills_for_workspace(&cwd).unwrap().unwrap();
        assert!(
            persisted
                .skills
                .iter()
                .any(|skill| skill.name == "project-skill")
        );

        let _ = fs::remove_dir_all(cwd);
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn global_skills_cli_v3_lock_uses_source_url_and_tree_hash() {
        let root = temp_dir("tendi-global-skills-cli-lock-test");
        let lock_path = root.join(".skill-lock.json");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &lock_path,
            r#"{
  "version": 3,
  "skills": {
    "demo": {
      "source": "example/agent-skills",
      "sourceType": "github",
      "sourceUrl": "https://github.com/example/agent-skills.git",
      "ref": "main",
      "skillPath": "skills/demo/SKILL.md",
      "skillFolderHash": "tree-hash",
      "installedAt": "2026-08-01T00:00:00Z",
      "updatedAt": "2026-08-01T00:00:00Z"
    }
  }
}
"#,
        )
        .unwrap();
        let mut warnings = Vec::new();
        let lock = super::read_skills_cli_lock(&lock_path, 3, &mut warnings).unwrap();
        let provenance = lock.skills["demo"].provenance();

        assert!(warnings.is_empty());
        assert_eq!(provenance.kind, "github");
        assert_eq!(
            provenance.source.as_deref(),
            Some("https://github.com/example/agent-skills.git")
        );
        assert_eq!(provenance.version.as_deref(), Some("tree-hash"));
        assert_eq!(provenance.source_ref.as_deref(), Some("main"));
        assert_eq!(
            provenance.relative_path.as_deref(),
            Some("skills/demo/SKILL.md")
        );

        fs::remove_dir_all(root).unwrap();
    }


    #[test]
    fn existing_source_database_record_does_not_read_lock_file() {
        let root = temp_dir("tendi-source-database-authority-test");
        let skill_dir = root.join(".agents/skills/demo");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(root.join("skills-lock.json"), "not valid json").unwrap();
        let record = super::SkillSourceRecord {
            skill_name: "demo".to_string(),
            skill_path: skill_dir.clone(),
            source_kind: "github".to_string(),
            source: Some("https://github.com/example/database-source.git".to_string()),
            source_ref: Some("main".to_string()),
            source_version: Some("database-hash".to_string()),
            source_relative_path: Some("skills/demo/SKILL.md".to_string()),
            update_status: "tracked".to_string(),
            origin: "skills-cli-lock".to_string(),
        };
        let mut resolver = super::ProvenanceResolver::managed(&root, vec![record]);

        let provenance = resolver.infer_installed(
            &skill_dir,
            &skill_dir,
            &root.join(".agents/skills"),
            "project",
            "demo",
            &None,
        );

        assert_eq!(
            provenance.source.as_deref(),
            Some("https://github.com/example/database-source.git")
        );
        assert!(resolver.skills_cli_locks.is_none());
        assert!(resolver.lock_warnings.is_empty());
        assert!(resolver.migrations.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    fn test_skill(name: &str, description: &str, path: &Path) -> SkillRecord {
        SkillRecord {
            name: name.to_string(),
            description: Some(description.to_string()),
            tags: Vec::new(),
            dependencies: Vec::new(),
            dependents: Vec::new(),
            visibility: SkillVisibility::Auto,
            agents: vec![AgentKind::Shared],
            paths: vec![SkillPath {
                path: PathBuf::from(path),
                root: path.parent().unwrap_or(path).to_path_buf(),
                scope: "global".to_string(),
                agent: AgentKind::Shared,
                install_target: "shared:/tmp/tendi-skills".to_string(),
                source_kind: "local".to_string(),
                source: None,
                source_ref: None,
                source_version: None,
                source_relative_path: None,
                symlink_status: "direct".to_string(),
                update_status: "local".to_string(),
                sha256: "sha".to_string(),
                tags: Vec::new(),
                tendi_visibility: None,
                effective_visibility: SkillVisibility::Auto,
                provider_allow_implicit_invocation: None,
                provider_skill_enabled: None,
                provider_disable_model_invocation: None,
                plugin_id: None,
                plugin_enabled: None,
            }],
            source_summary: "local".to_string(),
            install_targets: vec!["shared:/tmp/tendi-skills".to_string()],
            update_status: "local".to_string(),
            is_system: false,
            ctime: None,
            mtime: None,
        }
    }


    #[test]
    fn skill_backup_exclusion_reason_stays_provider_owned() {
        let codex_plugin = test_skill_path(
            "/tmp/.codex/plugins/browser/skills/demo",
            AgentKind::Codex,
            SkillVisibility::Off,
            Some(false),
        );
        let cursor_plugin = test_skill_path(
            "/tmp/.cursor/plugins/browser/skills/demo",
            AgentKind::Cursor,
            SkillVisibility::Off,
            Some(false),
        );

        assert_eq!(
            skill_backup_exclusion_reason(std::slice::from_ref(&codex_plugin)),
            Some("plugin-skill")
        );
        assert_eq!(
            skill_backup_exclusion_reason(std::slice::from_ref(&cursor_plugin)),
            None
        );
    }

    fn test_skill_path(
        path: &str,
        agent: AgentKind,
        effective_visibility: SkillVisibility,
        plugin_enabled: Option<bool>,
    ) -> SkillPath {
        let path = PathBuf::from(path);
        let root = path.parent().unwrap_or(&path).to_path_buf();
        SkillPath {
            path,
            root: root.clone(),
            scope: if plugin_enabled.is_some() {
                "plugin".to_string()
            } else {
                "global".to_string()
            },
            agent,
            install_target: format!("{}:{}", agent.label(), root.display()),
            source_kind: "local".to_string(),
            source: None,
            source_ref: None,
            source_version: None,
            source_relative_path: None,
            symlink_status: "direct".to_string(),
            update_status: "local".to_string(),
            sha256: "sha".to_string(),
            tags: Vec::new(),
            tendi_visibility: None,
            effective_visibility,
            provider_allow_implicit_invocation: None,
            provider_skill_enabled: None,
            provider_disable_model_invocation: None,
            plugin_id: plugin_enabled.map(|_| "browser@openai-bundled".to_string()),
            plugin_enabled,
        }
    }
}
