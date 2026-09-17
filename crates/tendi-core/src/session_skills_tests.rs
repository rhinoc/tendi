use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::json;

use crate::skills::{SkillPath, SkillRecord, SkillVisibility};

use super::*;

#[test]
fn skill_index_reads_a_published_projection_from_its_explicit_store() {
    let root = temp_dir("published-projection");
    let store = Store::open(root.join("test.sqlite3")).unwrap();
    let scan = crate::SkillScan {
        roots: vec![],
        skills: vec![],
        warnings: vec![],
    };
    assert!(
        store
            .save_skills_for_workspace_if_revision(&root, &scan, crate::Revision::ZERO)
            .unwrap()
    );
    let captured = store
        .read_projection_refresh_state::<crate::SkillScan>("skills", &root)
        .unwrap()
        .revision;
    let loaded = load_skills_for_index(&store, &root).unwrap();
    assert!(loaded.skills.is_empty());
    assert_eq!(
        store
            .read_projection_refresh_state::<crate::SkillScan>("skills", &root)
            .unwrap()
            .revision,
        captured
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extracts_codex_shell_skill_read() {
    let root = temp_dir("codex-shell");
    let skill_dir = root.join(".codex/skills/foo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "---\nname: foo\n---\n").unwrap();
    let transcript = root.join("session.jsonl");
    fs::write(
            &transcript,
            format!(
                "{}\n",
                json!({
                    "type": "response_item",
                    "timestamp": "2026-06-24T10:00:00Z",
                    "payload": {
                        "type": "function_call",
                        "name": "exec_command",
                        "arguments": format!("{{\"cmd\":\"cat {}\"}}", skill_dir.join("SKILL.md").display())
                    }
                })
            ),
        )
        .unwrap();

    let links = extract_session_skill_links(
        &session(&transcript, AgentKind::Codex),
        &SkillLookup::new(&skill_scan("foo", &skill_dir, AgentKind::Codex)),
    )
    .unwrap();

    assert_eq!(links.len(), 1);
    assert_eq!(links[0].skill_name, "foo");
    assert_eq!(links[0].evidence_kind, "exec_command");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn extracts_codex_custom_tool_skill_read_with_transcript_visible_evidence() {
    let root = temp_dir("codex-custom-tool-read");
    let skill_dir = root.join(".codex/skills/foo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "---\nname: foo\n---\n").unwrap();
    let transcript = root.join("session.jsonl");
    let input = format!(
        "const r = await tools.exec_command({{cmd: \"sed -n '1,300p' {}/SKILL.md\"}});",
        skill_dir.display()
    );
    fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({
                "type": "response_item",
                "timestamp": "2026-06-24T10:00:00Z",
                "payload": {
                    "type": "custom_tool_call",
                    "name": "exec",
                    "input": input,
                }
            })
        ),
    )
    .unwrap();

    let links = extract_session_skill_links(
        &session(&transcript, AgentKind::Codex),
        &SkillLookup::new(&skill_scan("foo", &skill_dir, AgentKind::Codex)),
    )
    .unwrap();

    assert_eq!(links.len(), 1);
    assert_eq!(links[0].evidence_kind, "exec");
    assert_eq!(links[0].evidence_text, input);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn extracts_claude_read_tool_skill_read() {
    let root = temp_dir("claude-read");
    let skill_dir = root.join(".claude/skills/foo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "---\nname: foo\n---\n").unwrap();
    let transcript = root.join("session.jsonl");
    fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({
                "type": "assistant",
                "timestamp": "2026-06-24T10:00:00Z",
                "message": {
                    "content": [{
                        "type": "tool_use",
                        "name": "Read",
                        "input": { "file_path": skill_dir.join("SKILL.md").display().to_string() }
                    }]
                }
            })
        ),
    )
    .unwrap();

    let links = extract_session_skill_links(
        &session(&transcript, AgentKind::Claude),
        &SkillLookup::new(&skill_scan("foo", &skill_dir, AgentKind::Claude)),
    )
    .unwrap();

    assert_eq!(links.len(), 1);
    assert_eq!(links[0].skill_name, "foo");
    assert_eq!(links[0].evidence_kind, "Read");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn extracts_plugin_cache_skill_read() {
    let root = temp_dir("plugin-cache");
    let skill_dir = root.join(".codex/plugins/cache/openai-bundled/browser/1.0.0/skills/bar");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "---\nname: bar\n---\n").unwrap();
    let transcript = root.join("session.jsonl");
    fs::write(
            &transcript,
            format!(
                "{}\n",
                json!({
                    "type": "response_item",
                    "timestamp": "2026-06-24T10:00:00Z",
                    "payload": {
                        "type": "function_call",
                        "name": "exec_command",
                        "arguments": format!("{{\"cmd\":\"cat {}\"}}", skill_dir.join("SKILL.md").display())
                    }
                })
            ),
        )
        .unwrap();

    let links = extract_session_skill_links(
        &session(&transcript, AgentKind::Codex),
        &SkillLookup::new(&skill_scan("bar", &skill_dir, AgentKind::Codex)),
    )
    .unwrap();

    assert_eq!(links.len(), 1);
    assert_eq!(links[0].skill_name, "bar");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn extracts_codex_selected_skill_without_treating_it_as_user_input() {
    let root = temp_dir("codex-selected-skill");
    let skill_dir = root.join(".agents/skills/foo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "---\nname: foo\n---\n").unwrap();
    let transcript = root.join("session.jsonl");
    fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({
                "type": "response_item",
                "timestamp": "2026-06-24T10:00:00Z",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{
                        "type": "input_text",
                        "text": format!(
                            "<skill>\n<name>foo</name>\n<path>{}</path>\n</skill>",
                            skill_dir.join("SKILL.md").display()
                        )
                    }]
                }
            })
        ),
    )
    .unwrap();

    let links = extract_session_skill_links(
        &session(&transcript, AgentKind::Codex),
        &SkillLookup::new(&skill_scan("foo", &skill_dir, AgentKind::Codex)),
    )
    .unwrap();

    assert_eq!(links.len(), 1);
    assert_eq!(links[0].skill_name, "foo");
    assert_eq!(links[0].evidence_kind, "explicit_skill");
    assert_eq!(links[0].confidence, EXPLICIT_CONFIDENCE);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_index_state_tracks_cursor_store_changes() {
    let root = temp_dir("cursor-freshness");
    fs::create_dir_all(&root).unwrap();
    let meta_path = root.join("meta.json");
    let store_path = root.join("store.db");
    fs::write(&meta_path, "{}\n").unwrap();
    fs::write(&store_path, "initial\n").unwrap();

    let session = session(&meta_path, AgentKind::Cursor);
    let first = session_skill_index_state(&session).unwrap();
    fs::write(&store_path, "updated store evidence\n").unwrap();
    let second = session_skill_index_state(&session).unwrap();

    assert_ne!(
        (first.file_mtime, first.file_size),
        (second.file_mtime, second.file_size)
    );
    let _ = fs::remove_dir_all(root);
}

fn session(path: &Path, agent: AgentKind) -> SessionRecord {
    SessionRecord {
        id: "session-1".to_string(),
        agent,
        title: Some("Test session".to_string()),
        project: path.parent().map(Path::to_path_buf),
        repository: None,
        repository_url: None,
        logical_project_id: None,
        logical_project_name: None,
        path: path.to_path_buf(),
        started_at: None,
        updated_at: Some("2026-06-24T10:00:00Z".to_string()),
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

fn skill_scan(name: &str, path: &Path, agent: AgentKind) -> SkillScan {
    SkillScan {
        roots: Vec::new(),
        warnings: Vec::new(),
        skills: vec![SkillRecord {
            id: name.to_string(),
            installation_id: name.to_string(),
            name: name.to_string(),
            description: None,
            tags: Vec::new(),
            dependencies: Vec::new(),
            dependents: Vec::new(),
            dependency_ids: Vec::new(),
            dependent_ids: Vec::new(),
            is_wrapper: false,
            visibility: SkillVisibility::Auto,
            agents: vec![agent],
            paths: vec![SkillPath {
                path: path.to_path_buf(),
                root: path.parent().unwrap_or(path).to_path_buf(),
                scope: "global".to_string(),
                agent,
                install_target: "global".to_string(),
                source_kind: "local".to_string(),
                source: None,
                source_ref: None,
                source_version: None,
                source_relative_path: None,
                symlink_status: "direct".to_string(),
                update_status: "local".to_string(),
                sha256: "hash".to_string(),
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
            install_targets: vec!["global".to_string()],
            update_status: "local".to_string(),
            is_system: false,
            ctime: None,
            mtime: None,
        }],
    }
}

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "tendi-session-skills-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
