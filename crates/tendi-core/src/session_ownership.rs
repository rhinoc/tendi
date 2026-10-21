use crate::{
    providers::{ProviderContext, agent_providers},
    skills::AgentKind,
};
use anyhow::Result;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub(crate) mod processes;
pub use processes::ProcessMonitor;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct HeldSession {
    pub agent: AgentKind,
    pub id: String,
}

pub struct OwnershipProbe {
    pub watch_paths: Vec<(AgentKind, PathBuf)>,
    pub providers: Vec<(AgentKind, Result<BTreeSet<HeldSession>>)>,
    pub process_ids: Option<BTreeSet<i32>>,
}

pub fn probe(cwd: &Path, additional_roots: &[PathBuf]) -> OwnershipProbe {
    probe_selected(cwd, additional_roots, None)
}

pub fn probe_selected(
    cwd: &Path,
    additional_roots: &[PathBuf],
    selected: Option<&BTreeSet<AgentKind>>,
) -> OwnershipProbe {
    let context = ProviderContext::new(cwd);
    let mut processes = processes::Processes::default();
    let mut watch_paths = BTreeSet::new();
    let owners = agent_providers();
    let providers = owners
        .iter()
        .filter(|provider| selected.is_none_or(|selected| selected.contains(&provider.kind())))
        .map(|provider| {
            watch_paths.extend(
                provider
                    .session_ownership_paths(&context, additional_roots)
                    .into_iter()
                    .map(|path| (provider.kind(), path)),
            );
            (
                provider.kind(),
                provider.held_sessions(&context, additional_roots, &mut processes),
            )
        })
        .collect();
    let process_ids = selected.is_none().then(|| {
        processes
            .entries()
            .iter()
            .filter(|process| {
                owners
                    .iter()
                    .any(|provider| provider.owns_session_process(&process.executable))
            })
            .map(|process| process.pid)
            .collect()
    });
    OwnershipProbe {
        watch_paths: watch_paths.into_iter().collect(),
        providers,
        process_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targeted_events_probe_only_the_owner_without_enumerating_processes() {
        let selected = BTreeSet::from([AgentKind::Claude]);
        let probe = probe_selected(&std::env::temp_dir(), &[], Some(&selected));
        assert_eq!(probe.providers.len(), 1);
        assert_eq!(probe.providers[0].0, AgentKind::Claude);
        assert!(
            probe
                .watch_paths
                .iter()
                .all(|(agent, _)| *agent == AgentKind::Claude)
        );
        assert!(probe.process_ids.is_none());
    }
}
