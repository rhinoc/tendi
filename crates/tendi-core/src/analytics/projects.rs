use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};

use crate::sessions::SessionRecord;

use super::{AnalyticsCost, AnalyticsTokenUsage};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsProjectIdentity {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsProjectUsage {
    pub id: String,
    pub name: String,
    pub usage: AnalyticsTokenUsage,
    pub responses: u64,
    pub cost: AnalyticsCost,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectUsageAccumulator {
    pub name: String,
    pub usage: AnalyticsTokenUsage,
    pub responses: u64,
    pub cost: AnalyticsCost,
}

impl ProjectUsageAccumulator {
    pub(crate) fn add(
        &mut self,
        name: &str,
        usage: AnalyticsTokenUsage,
        responses: u64,
        cost: AnalyticsCost,
    ) {
        if self.name.is_empty() {
            self.name = name.to_string();
        }
        self.usage.add_assign(usage);
        self.responses += responses;
        self.cost.add_assign(cost);
    }

    pub(crate) fn finish((id, usage): (String, Self)) -> AnalyticsProjectUsage {
        AnalyticsProjectUsage {
            id,
            name: usage.name,
            usage: usage.usage,
            responses: usage.responses,
            cost: usage.cost,
        }
    }
}

pub(crate) fn identity_for_session(session: &SessionRecord) -> Option<AnalyticsProjectIdentity> {
    if let Some(id) = session
        .logical_project_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        let name = session
            .logical_project_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| session_project_name(session))
            .unwrap_or_else(|| id.to_string());
        return Some(AnalyticsProjectIdentity {
            id: id.to_string(),
            name,
        });
    }

    let path = session.repository.as_ref().or(session.project.as_ref())?;
    let id = path.to_string_lossy().trim().to_string();
    (!id.is_empty()).then(|| AnalyticsProjectIdentity {
        name: project_name(path),
        id,
    })
}

fn session_project_name(session: &SessionRecord) -> Option<String> {
    session
        .repository
        .as_ref()
        .or(session.project.as_ref())
        .map(|path| project_name(path))
}

fn project_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

pub(crate) fn finish_projects(
    projects: BTreeMap<String, ProjectUsageAccumulator>,
) -> Vec<AnalyticsProjectUsage> {
    projects
        .into_iter()
        .map(ProjectUsageAccumulator::finish)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{sessions::SessionRecord, skills::AgentKind};

    use super::identity_for_session;

    fn session() -> SessionRecord {
        SessionRecord {
            id: "session".to_string(),
            agent: AgentKind::Codex,
            title: None,
            project: Some(PathBuf::from("/worktrees/tendi")),
            repository: Some(PathBuf::from("/repos/tendi")),
            repository_url: None,
            logical_project_id: Some("project-1".to_string()),
            logical_project_name: Some("Tendi".to_string()),
            path: PathBuf::from("/tmp/session.jsonl"),
            started_at: None,
            updated_at: None,
            message_count: None,
            first_user_message: None,
            last_user_message: None,
            last_assistant_message: None,
            turn_count: None,
            model: None,
            mode: None,
            approval_mode: None,
            is_run_everything: None,
            parent_session_id: None,
            token_usage: None,
        }
    }

    #[test]
    fn prefers_resolved_logical_project() {
        let identity = identity_for_session(&session()).unwrap();
        assert_eq!(identity.id, "project-1");
        assert_eq!(identity.name, "Tendi");
    }

    #[test]
    fn falls_back_to_repository_path() {
        let mut session = session();
        session.logical_project_id = None;
        session.logical_project_name = None;
        let identity = identity_for_session(&session).unwrap();
        assert_eq!(identity.id, "/repos/tendi");
        assert_eq!(identity.name, "tendi");
    }
}
