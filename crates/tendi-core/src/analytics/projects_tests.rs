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
