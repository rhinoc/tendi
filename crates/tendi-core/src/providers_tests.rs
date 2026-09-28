use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    AgentKind, SessionRecord, codex, plan_assistant_ask, plan_session_resume, project_dirs,
    session_resume_repair_commands,
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

#[test]
fn codex_provider_includes_plugin_skill_roots() {
    let root = temp_dir("tendi-codex-plugin-skills-test");
    let codex_home = root.join(".codex");
    let global_skills = codex_home.join("skills");
    let plugin_skills =
        codex_home.join("plugins/cache/openai-primary-runtime/documents/26.622.11653/skills");
    fs::create_dir_all(&global_skills).unwrap();
    fs::create_dir_all(plugin_skills.join("documents")).unwrap();
    fs::write(
        codex_home.join("config.toml"),
        "[plugins.\"documents@openai-primary-runtime\"]\nenabled = false\n",
    )
    .unwrap();
    fs::write(
        plugin_skills.join("documents/SKILL.md"),
        "---\nname: documents\ndescription: Documents\n---\n",
    )
    .unwrap();

    let roots = codex::codex_skill_roots(&codex_home, &project_dirs(&root), AgentKind::Codex);

    assert!(roots.iter().any(|root| {
        root.path == global_skills && root.scope == "global" && root.agent == AgentKind::Codex
    }));
    assert!(roots.iter().any(|root| {
        root.path == plugin_skills
            && root.scope == "plugin"
            && root.agent == AgentKind::Codex
            && root.plugin_id.as_deref() == Some("documents@openai-primary-runtime")
            && root.plugin_enabled == Some(false)
    }));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn codex_plugin_config_maps_enabled_state_by_plugin_id() {
    let root = temp_dir("tendi-codex-plugin-config-test");
    let codex_home = root.join(".codex");
    let plugin_skills = codex_home.join("plugins/cache/openai-bundled/browser/1.0.0/skills");
    fs::create_dir_all(&plugin_skills).unwrap();
    fs::write(
        codex_home.join("config.toml"),
        "[plugins.\"browser@openai-bundled\"]\nenabled = false\n",
    )
    .unwrap();

    assert_eq!(
        codex::codex_plugin_id_for_skill_root(&codex_home, &plugin_skills).as_deref(),
        Some("browser@openai-bundled")
    );
    assert_eq!(
        codex::codex_plugin_enabled_by_id(&codex_home)
            .get("browser@openai-bundled")
            .copied(),
        Some(false)
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn codex_plugin_skill_roots_ignores_dependency_dirs() {
    let root = temp_dir("tendi-codex-plugin-skip-test");
    let codex_home = root.join(".codex");
    let plugin_skills = codex_home.join("plugins/cache/openai-bundled/browser/1.0.0/skills");
    let dependency_skills = codex_home
        .join("plugins/cache/openai-bundled/browser/1.0.0/scripts/node_modules/pkg/skills");
    fs::create_dir_all(&plugin_skills).unwrap();
    fs::create_dir_all(&dependency_skills).unwrap();

    let roots = codex::codex_plugin_skill_roots(&codex_home);

    assert_eq!(roots, vec![plugin_skills]);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn resume_command_ignores_relative_project_paths() {
    let session = SessionRecord {
        id: "019eef10-7054-7a63-b9c8-fd16cc70cd53".to_string(),
        agent: AgentKind::Codex,
        title: None,
        project: Some(PathBuf::from("tendi")),
        repository: None,
        repository_url: None,
        logical_project_id: None,
        logical_project_name: None,
        path: PathBuf::from("/Users/test/.codex/sessions/session.jsonl"),
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
    };

    let plan = plan_session_resume(&session).unwrap();

    assert_eq!(plan.command.cwd, None);
    assert_eq!(
        plan.command.args,
        vec!["resume", "019eef10-7054-7a63-b9c8-fd16cc70cd53"]
    );
}

#[test]
fn assistant_commands_keep_provider_specific_cli_contracts() {
    let workspace = PathBuf::from("/tmp/tendi-assistant");
    let prompt = "Answer this";
    let cases = [
        (
            AgentKind::Codex,
            "codex",
            vec!["-C", "/tmp/tendi-assistant", "exec"],
        ),
        (
            AgentKind::Claude,
            "claude",
            vec!["--print", "--verbose", "--output-format", "stream-json"],
        ),
        (
            AgentKind::Cursor,
            "cursor",
            vec!["agent", "--print", "--output-format", "stream-json"],
        ),
    ];

    for (agent, executable, prefix) in cases {
        let command = plan_assistant_ask(agent, &workspace, prompt).unwrap();
        assert_eq!(command.executable, executable);
        assert_eq!(command.cwd, Some(workspace.clone()));
        assert_eq!(command.args.last().map(String::as_str), Some(prompt));
        assert_eq!(command.args[..prefix.len()], prefix);

        let expected_mode = match agent {
            AgentKind::Codex => "--dangerously-bypass-approvals-and-sandbox",
            AgentKind::Claude => "--dangerously-skip-permissions",
            AgentKind::Cursor => "--yolo",
            AgentKind::Shared => unreachable!("shared agents are not in assistant cases"),
            AgentKind::Unknown => unreachable!("unknown agents are not in assistant cases"),
        };
        assert!(command.args.iter().any(|arg| arg == expected_mode));

        if agent == AgentKind::Claude {
            let prompt_index = command.args.len() - 1;
            assert_eq!(command.args[prompt_index - 1], "--");
            assert!(
                command
                    .args
                    .iter()
                    .any(|arg| arg == "--include-partial-messages")
            );
        }
        if agent == AgentKind::Cursor {
            assert!(
                command
                    .args
                    .iter()
                    .any(|arg| arg == "--stream-partial-output")
            );
        }
    }
    assert!(plan_assistant_ask(AgentKind::Unknown, &workspace, prompt).is_err());
}

#[test]
fn codex_writer_lock_path_supports_live_and_archived_sessions() {
    assert_eq!(
        codex::codex_thread_writer_lock_path(
            PathBuf::from("/Users/test/.codex/sessions/2026/08/24/session.jsonl").as_path(),
            "session-id"
        ),
        Some(PathBuf::from(
            "/Users/test/.codex/thread-writer-locks/session-id.lock"
        ))
    );
    assert_eq!(
        codex::codex_thread_writer_lock_path(
            PathBuf::from("/Users/test/.codex/archived_sessions/session.jsonl").as_path(),
            "session-id"
        ),
        Some(PathBuf::from(
            "/Users/test/.codex/thread-writer-locks/session-id.lock"
        ))
    );
    assert_eq!(
        codex::codex_thread_writer_lock_path(
            PathBuf::from("/Users/test/session.jsonl").as_path(),
            "session-id"
        ),
        None
    );
}

#[test]
fn plan_session_resume_rejects_an_active_codex_writer() {
    let root = temp_dir("tendi-codex-resume-lock-test");
    let session_path = root.join(".codex/sessions/2026/08/24/session-id.jsonl");
    fs::create_dir_all(session_path.parent().unwrap()).unwrap();
    fs::write(&session_path, "").unwrap();
    let session = SessionRecord {
        id: "session-id".to_string(),
        agent: AgentKind::Codex,
        title: None,
        project: None,
        repository: None,
        repository_url: None,
        logical_project_id: None,
        logical_project_name: None,
        path: session_path,
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
    };
    let lock_path = codex::codex_thread_writer_lock_path(&session.path, &session.id).unwrap();
    fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    lock_file.try_lock().unwrap();

    let writer = codex::active_session_writer(&session)
        .unwrap()
        .expect("the held lock should be reported as an active writer");
    assert_eq!(writer.lock_path, lock_path);

    let error = plan_session_resume(&session).unwrap_err();

    assert!(error.to_string().contains("already has an active writer"));
    lock_file.unlock().unwrap();
    drop(lock_file);
    assert!(plan_session_resume(&session).is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn codex_resume_repair_archives_then_unarchives_the_session() {
    let session = SessionRecord {
        id: "session-id".to_string(),
        agent: AgentKind::Codex,
        title: None,
        project: None,
        repository: None,
        repository_url: None,
        logical_project_id: None,
        logical_project_name: None,
        path: PathBuf::from("/Users/test/.codex/sessions/session-id.jsonl"),
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
    };

    let commands = session_resume_repair_commands(&session);

    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].executable, "codex");
    assert_eq!(commands[0].args, ["archive", "session-id"]);
    assert_eq!(commands[1].executable, "codex");
    assert_eq!(commands[1].args, ["unarchive", "session-id"]);
}
