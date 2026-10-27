use super::{RexBetaProvider, TerminalProvider, local_client, new_session_command};
use serde_json::json;

#[test]
fn creates_a_session_with_literal_arguments_environment_and_working_directory() {
    let session = tendi_core::SessionCommand {
        executable: "codex".into(),
        args: vec!["resume".into(), "session 'with' $characters".into()],
        cwd: Some("/tmp/project with spaces/中文".into()),
        env: vec![(
            "CODEX_HOME".into(),
            "/tmp/profile 'with' $characters".into(),
        )],
    };
    let command = new_session_command(&session).unwrap();
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_str().unwrap())
        .collect();
    assert_eq!(
        &args[2..],
        &[
            "new",
            "--json",
            "--cwd",
            "/tmp/project with spaces/中文",
            "--",
            "/usr/bin/env",
            "CODEX_HOME=/tmp/profile 'with' $characters",
            "codex",
            "resume",
            "session 'with' $characters",
        ]
    );
    assert_eq!(args[0], "--server");
    assert!(args[1].ends_with("/rex/server.sock"));
}

#[test]
fn launches_directly_when_no_environment_or_directory_is_supplied() {
    let session = tendi_core::SessionCommand {
        executable: "claude".into(),
        args: vec!["--resume".into(), "session-id".into()],
        cwd: None,
        env: vec![],
    };
    let command = new_session_command(&session).unwrap();
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_str().unwrap())
        .collect();
    assert_eq!(
        &args[2..],
        &["new", "--json", "--", "claude", "--resume", "session-id"]
    );
}

#[test]
fn selects_the_local_beta_app_among_other_clients() {
    let bytes = serde_json::to_vec(&json!({ "clients": [
        { "client_id": "server", "principal": { "transport": "unix" } },
        { "client_id": "cli", "info": { "kind": "cli", "name": "rex" }, "principal": { "transport": "unix" } },
        { "client_id": "remote", "info": { "kind": "app", "name": "Rex Beta" }, "principal": { "transport": "tcp" } },
        { "client_id": "reconnecting", "info": { "kind": "app", "name": "Rex Beta" }, "principal": { "transport": "unix" }, "connection_state": "lost" },
        { "client_id": "unknown", "info": {}, "principal": { "transport": "unix" } },
        { "client_id": "local", "info": { "kind": "app", "name": "Rex Beta" }, "principal": { "transport": "unix" } },
    ] })).unwrap();
    assert_eq!(local_client(&bytes).unwrap().as_deref(), Some("local"));
    assert_eq!(local_client(br#"{"clients":[]}"#).unwrap(), None);
}

#[test]
fn rejects_ambiguous_clients_and_invalid_responses() {
    let client = json!({ "client_id": "local", "info": { "kind": "app", "name": "Rex Beta" }, "principal": { "transport": "unix" } });
    let bytes = serde_json::to_vec(&json!({ "clients": [client, client] })).unwrap();
    assert!(
        local_client(&bytes)
            .unwrap_err()
            .detail
            .contains("multiple local")
    );
    assert!(local_client(b"{}").is_err());
}

#[test]
fn resolves_beta_terminal_identifier() {
    assert_eq!(super::super::resolve_terminal(" Rex-Beta "), "rex-beta");
    assert_eq!(RexBetaProvider.application_name(), "Rex Beta");
}
