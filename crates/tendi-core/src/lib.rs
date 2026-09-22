pub mod agents;
pub mod analytics;
pub mod assistant;
pub mod bundled_skill;
pub mod config;
pub mod files;
mod fsutil;
pub mod generated;
mod git;
pub mod hooks;
mod json_edit;
pub mod logging;
pub mod mcp;
pub(crate) use storage::migrations;
pub mod coordination;
pub mod projects;
mod providers;
pub mod rules;
pub mod runtime_contract;
pub mod session_skills;
pub mod sessions;
pub mod skill_backup;
pub mod skill_marketplace;
pub mod skill_restore;
mod skill_source;
pub mod skill_targets;
pub mod skills;
pub mod storage;
mod time;
pub mod transcript;

#[cfg(test)]
pub(crate) mod test_support;

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub use agents::{AgentRecord, AgentScan};
pub use hooks::{HookRecord, HookScan};
pub use mcp::{McpScan, McpServerRecord};
pub use providers::{
    SessionCommand, SessionResumePlan, SessionWriter, accepts_session_app_url,
    active_session_writer, apply_session_config_profile, config_profile_key, parse_agent,
    plan_assistant_ask, plan_session_resume, session_resume_repair_commands, session_root_priority,
};
pub use rules::{RuleRecord, RuleScan};
pub use runtime_contract::{
    DomainSnapshot, InstallationId, OperationId, OperationKind, OperationRecord, OperationStatus,
    ProjectionHead, Revision, RevisionDecision, RevisionedEvent, ScopeKey, SessionKey,
    SourceLocator, SourceRef, SourceVersion, decide_revision,
};
pub use sessions::{SessionRecord, SessionScan};
pub use skill_targets::{SkillInstallScope, SkillTarget};
pub use skills::{AgentKind, SkillRecord, SkillScan, SkillVisibility};
pub use storage::SessionSearchHit;
pub use transcript::{TranscriptItem, TranscriptScan};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScanReport {
    pub agents: AgentScan,
    pub skills: SkillScan,
    pub sessions: SessionScan,
    pub rules: RuleScan,
    pub hooks: HookScan,
    pub mcp: McpScan,
}

/// Prepare a workspace before runtime projections are scanned.
///
/// The startup transition runner owns both database and workspace-scoped transitions. Runtime
/// scanners only read the resulting current-format records.
pub fn initialize_workspace(
    store: &storage::Store,
    cwd: impl AsRef<Path>,
    project_roots: &[PathBuf],
) -> Result<()> {
    #[cfg(test)]
    test_support::ensure_isolated_environment();

    let cwd = cwd.as_ref();
    migrations::run_workspace(store, cwd, project_roots)?;
    store.invalidate_projection("skills", cwd)?;
    Ok(())
}

pub fn scan(cwd: impl AsRef<Path>) -> Result<ScanReport> {
    let cwd = cwd.as_ref();
    let store = storage::Store::open_default()?;
    initialize_workspace(&store, cwd, &[])?;
    scan_with_store(cwd, &store)
}

/// Prepare a report without changing installations. Startup transitions are a
/// separate boundary and must not be replayed when a projection CAS loses.
pub fn scan_with_store(cwd: &Path, store: &storage::Store) -> Result<ScanReport> {
    let database_path = store.path().to_path_buf();
    let additional_session_roots = store
        .app_settings()?
        .additional_session_roots
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    std::thread::scope(|scope| {
        let agents = scope.spawn(|| agents::scan_agents(&cwd));
        let skills = scope.spawn(|| {
            let reader = storage::Store::open(&database_path)?;
            skills::scan_skills_for_project_roots_with_store(cwd, &reader, &[])
        });
        let sessions = scope.spawn(|| {
            sessions::scan_sessions_with_additional_roots(&cwd, &additional_session_roots)
        });
        let rules = scope.spawn(|| rules::scan_rules(&cwd));
        let hooks = scope.spawn(|| hooks::scan_hooks(&cwd));
        let mcp = scope.spawn(|| mcp::scan_mcp(&cwd));
        let skills = skills.join().expect("skills scan thread panicked")?;

        Ok(ScanReport {
            agents: agents.join().expect("agents scan thread panicked")?,
            skills,
            sessions: sessions.join().expect("sessions scan thread panicked")?,
            rules: rules.join().expect("rules scan thread panicked")?,
            hooks: hooks.join().expect("hooks scan thread panicked")?,
            mcp: mcp.join().expect("mcp scan thread panicked")?,
        })
    })
}

pub fn scan_and_persist(cwd: impl AsRef<Path>) -> Result<ScanReport> {
    let store = storage::Store::open_default()?;
    scan_and_persist_with_store(cwd.as_ref(), &store)
}

pub fn scan_and_persist_with_store(cwd: &Path, store: &storage::Store) -> Result<ScanReport> {
    initialize_workspace(store, cwd, &[])?;
    let workspace = storage::canonical_workspace_root(cwd);
    let scope = ScopeKey::new(format!("workspace:{}", workspace.display()))?;
    let started = std::time::Instant::now();
    loop {
        let revisions = ["agents", "skills", "rules", "hooks", "mcp", "sessions"]
            .into_iter()
            .map(|domain| {
                Ok((
                    domain.to_owned(),
                    store
                        .projection_head(&scope, domain)?
                        .map_or(Revision::ZERO, |head| head.revision),
                ))
            })
            .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
        let report = scan_with_store(&workspace, store)?;
        if store.save_scan_for_workspace_if_revisions(&workspace, &report, &revisions)? {
            return Ok(report);
        }
        if started.elapsed() >= std::time::Duration::from_secs(30) {
            anyhow::bail!(
                "workspace sources kept changing during projection preparation; retry the scan"
            );
        }
    }
}
