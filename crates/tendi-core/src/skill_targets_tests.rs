use super::*;

#[test]
fn legacy_names_keep_their_serialized_values() {
    for (kind, expected) in [
        (AgentKind::Shared, "shared"),
        (AgentKind::Codex, "codex"),
        (AgentKind::Cursor, "cursor"),
        (AgentKind::Claude, "claude"),
    ] {
        let target = SkillTarget::from(kind);
        assert_eq!(
            serde_json::to_string(&target).unwrap(),
            format!("\"{expected}\"")
        );
        assert!(target_config(&target).is_ok());
    }

    let claude_alias: SkillTarget = "claude".parse().unwrap();
    assert_eq!(claude_alias.id(), "claude");
    assert_eq!(target_config(&claude_alias).unwrap().id, "claude-code");
    assert_eq!(claude_alias.agent_kind().unwrap(), AgentKind::Claude);
    assert_eq!(
        "claude-code"
            .parse::<SkillTarget>()
            .unwrap()
            .agent_kind()
            .unwrap(),
        AgentKind::Claude
    );
}

#[test]
fn project_roots_are_expressed_from_the_cwd() {
    let cwd = Path::new("/tmp/project");
    assert_eq!(
        skill_target_root(cwd, &"eve".parse().unwrap(), SkillInstallScope::Project).unwrap(),
        cwd.join("agent/skills")
    );
    assert_eq!(
        skill_target_root(
            cwd,
            &"claude-code".parse().unwrap(),
            SkillInstallScope::Project
        )
        .unwrap(),
        cwd.join(".claude/skills")
    );
}

#[test]
fn unknown_skill_target_does_not_become_an_unknown_agent() {
    let target = SkillTarget("not-a-provider".to_string());
    assert!(target.agent_kind().is_err());
}
