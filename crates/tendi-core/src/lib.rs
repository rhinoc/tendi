pub mod agents;
pub mod analytics;
pub mod bundled_skill;
pub mod config;
pub mod files;
mod fsutil;
mod git;
pub mod hooks;
pub mod logging;
pub mod mcp;
mod providers;
pub mod rules;
pub mod session_skills;
pub mod sessions;
pub mod skill_marketplace;
pub mod skill_restore;
mod skill_source;
pub mod skill_targets;
pub mod skills;
pub mod storage;
pub mod transcript;

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

pub use agents::{AgentRecord, AgentScan};
pub use hooks::{HookRecord, HookScan};
pub use mcp::{McpScan, McpServerRecord};
pub use providers::{
    SessionCommand, SessionResumePlan, apply_session_config_profile, config_profile_key,
    parse_agent, plan_session_resume,
};
pub use rules::{RuleRecord, RuleScan};
pub use sessions::{SessionRecord, SessionScan};
pub use skill_targets::{SkillInstallScope, SkillTarget};
pub use skills::{AgentKind, SkillRecord, SkillScan, SkillVisibility};
pub use storage::SessionSearchHit;
pub use transcript::{TranscriptItem, TranscriptScan};

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub agents: AgentScan,
    pub skills: SkillScan,
    pub sessions: SessionScan,
    pub rules: RuleScan,
    pub hooks: HookScan,
    pub mcp: McpScan,
}

pub fn scan(cwd: impl AsRef<Path>) -> Result<ScanReport> {
    let cwd = cwd.as_ref().to_path_buf();
    let store = storage::Store::open_default()?;
    let additional_session_roots = store
        .app_settings()?
        .additional_session_roots
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let session_scan_cache = store.session_scan_cache()?;
    std::thread::scope(|scope| {
        let agents = scope.spawn(|| agents::scan_agents(&cwd));
        let skills = scope.spawn(|| skills::scan_skills_synced(&cwd));
        let sessions = scope.spawn(|| {
            sessions::scan_sessions_with_additional_roots_cached(
                &cwd,
                &additional_session_roots,
                &session_scan_cache,
            )
        });
        let rules = scope.spawn(|| rules::scan_rules(&cwd));
        let hooks = scope.spawn(|| hooks::scan_hooks(&cwd));
        let mcp = scope.spawn(|| mcp::scan_mcp(&cwd));

        Ok(ScanReport {
            agents: agents.join().expect("agents scan thread panicked")?,
            skills: skills.join().expect("skills scan thread panicked")?,
            sessions: sessions.join().expect("sessions scan thread panicked")?,
            rules: rules.join().expect("rules scan thread panicked")?,
            hooks: hooks.join().expect("hooks scan thread panicked")?,
            mcp: mcp.join().expect("mcp scan thread panicked")?,
        })
    })
}

pub fn scan_and_persist(cwd: impl AsRef<Path>) -> Result<ScanReport> {
    let report = scan(cwd)?;
    let store = storage::Store::open_default()?;
    store.save_scan(&report)?;
    Ok(report)
}
