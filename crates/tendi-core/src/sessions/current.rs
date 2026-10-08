use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use serde::Serialize;
use walkdir::WalkDir;

use crate::{
    providers::{ProviderContext, agent_provider, agent_providers},
    skills::AgentKind,
};

#[derive(Debug, Clone, Serialize)]
pub struct CurrentSession {
    pub id: String,
    pub agent: AgentKind,
    pub path: Option<PathBuf>,
}

pub fn current_session(cwd: &Path, agent: Option<AgentKind>) -> Result<CurrentSession> {
    let env = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    resolve_current_session(&ProviderContext::new(cwd), agent, &env)
}

fn resolve_current_session(
    ctx: &ProviderContext,
    agent: Option<AgentKind>,
    env: &BTreeMap<String, String>,
) -> Result<CurrentSession> {
    if let Some(agent) = agent
        && agent_provider(agent).current_session_env_key().is_none()
    {
        bail!("current sessions are not supported for {}", agent.label());
    }
    let providers = agent
        .map(|agent| vec![agent_provider(agent)])
        .unwrap_or_else(agent_providers);
    let identities = providers
        .into_iter()
        .filter_map(|provider| {
            let id = env.get(provider.current_session_env_key()?)?.trim();
            (!id.is_empty()).then(|| (provider, id.to_string()))
        })
        .collect::<Vec<_>>();
    let (provider, id) = match identities.as_slice() {
        [] => bail!(
            "missing current session context; run this command from an agent shell{}",
            agent
                .map(|agent| format!(" for {}", agent.label()))
                .unwrap_or_default()
        ),
        [identity] => identity,
        _ => bail!(
            "multiple current session contexts ({}); specify --agent",
            identities
                .iter()
                .map(|(provider, _)| provider.storage_key())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    Ok(CurrentSession {
        id: id.clone(),
        agent: provider.kind(),
        path: provider.current_session_transcript(ctx, id, env)?,
    })
}

pub(crate) fn find_transcript(
    roots: &[PathBuf],
    depth: usize,
    matches: impl Fn(&Path) -> bool,
) -> Result<Option<PathBuf>> {
    let mut paths = std::collections::BTreeSet::new();
    for root in roots.iter().filter(|root| root.is_dir()) {
        for entry in WalkDir::new(root).max_depth(depth) {
            let entry = entry?;
            if entry.file_type().is_file()
                && entry.path().extension().is_some_and(|ext| ext == "jsonl")
                && matches(entry.path())
            {
                paths.insert(entry.into_path());
            }
        }
    }
    if paths.len() > 1 {
        bail!(
            "multiple transcripts match the current session: {}",
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(paths.into_iter().next())
}

#[cfg(test)]
#[path = "current_tests.rs"]
mod tests;
