use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;
use serde_json::json;

use crate::providers::codex::scan_jsonl_sessions_for_test as scan_codex_jsonl;
use crate::{
    git,
    providers::{codex::scan_session_index as scan_codex_index, cursor_sessions},
    skills::AgentKind,
};

use super::{
    SESSION_PREVIEW_MAX_CHARS, SessionRecord, SessionRepositoryResolver, SessionScanCache,
    SessionScanCacheEntry, SessionScanSourceState, clean_preview_text, clean_title,
    compare_timestamps, extract_session_message, extract_session_title_for_agent, file_state,
    infer_session_project, infer_session_resume_target, is_session_candidate_path, merge_sessions,
    normalize_session_projects, repository_from_git_snapshot, scan_additional_session_roots,
    scan_jsonl_meta_for_agent, scan_jsonl_sessions, session_requires_rescan, session_watch_plan,
    should_replace_session_path,
};

use cursor_sessions::{decode_cursor_project_dir, scan_cursor_meta};

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
fn compares_session_timestamps_by_instant_across_offsets() {
    assert_eq!(
        compare_timestamps(
            Some("2026-08-28T11:49:00+08:00"),
            Some("2026-08-28T03:57:03.099Z"),
        ),
        std::cmp::Ordering::Less,
    );
    assert_eq!(
        compare_timestamps(
            Some("2026-08-28T03:57:03.099Z"),
            Some("2026-08-28T11:49:00+08:00"),
        ),
        std::cmp::Ordering::Greater,
    );
}

#[test]
fn infers_session_resume_target_from_codex_session_meta() {
    let root = temp_dir("tendi-session-resume-target");
    fs::create_dir_all(&root).unwrap();
    let terminal = root.join("terminal.jsonl");
    fs::write(
        &terminal,
        r#"{"type":"session_meta","source":"cli","originator":"codex-tui"}"#,
    )
    .unwrap();
    assert_eq!(
        infer_session_resume_target(&terminal, AgentKind::Codex),
        Some("terminal")
    );

    let app = root.join("app.jsonl");
    fs::write(
        &app,
        r#"{"type":"session_meta","source":"vscode","originator":"Codex Desktop"}"#,
    )
    .unwrap();
    assert_eq!(
        infer_session_resume_target(&app, AgentKind::Codex),
        Some("app")
    );

    let unknown = root.join("unknown.jsonl");
    fs::write(&unknown, r#"{"type":"session_meta","source":"other"}"#).unwrap();
    assert_eq!(
        infer_session_resume_target(&unknown, AgentKind::Codex),
        None
    );
    fs::remove_dir_all(root).unwrap();
}

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn git_repository_groups_linked_worktree_under_main_checkout() {
    let root = temp_dir("tendi-session-repository-test");
    let repo = root.join("repo");
    let linked = root.join("linked");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["config", "user.email", "test@tendi.invalid"]);
    git(&repo, &["config", "user.name", "Tendi Test"]);
    fs::write(repo.join("README.md"), "seed\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "--quiet", "-m", "seed"]);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ],
    );

    let expected = fs::canonicalize(&repo).unwrap();
    assert_eq!(
        repository_from_git_snapshot(
            git::local_repository_snapshot(&repo, git::never_cancelled()).unwrap(),
        )
        .0
        .as_deref(),
        Some(expected.as_path())
    );
    assert_eq!(
        repository_from_git_snapshot(
            git::local_repository_snapshot(&linked, git::never_cancelled()).unwrap(),
        )
        .0
        .as_deref(),
        Some(expected.as_path())
    );
    let mut resolver = SessionRepositoryResolver::default();
    assert_eq!(
        resolver.resolve(&repo).0.as_deref(),
        Some(expected.as_path())
    );
    assert_eq!(
        resolver.resolve(&linked).0.as_deref(),
        Some(expected.as_path())
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_repository_resolver_reuses_local_snapshot_cache_across_scans() {
    let root = temp_dir("tendi-session-repository-snapshot-cache-test");
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "--quiet"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/tendi-before.git",
        ],
    );

    let mut first_resolver = SessionRepositoryResolver::default();
    let first = first_resolver.resolve(&root);
    assert_eq!(
        first.1.as_deref(),
        Some("https://github.com/example/tendi-before.git")
    );

    git(
        &root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/example/tendi-after.git",
        ],
    );

    let mut second_resolver = SessionRepositoryResolver::default();
    assert_eq!(second_resolver.resolve(&root), first);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_repository_resolver_skips_non_git_and_caches_repository() {
    let root = temp_dir("tendi-session-repository-cache-test");
    let plain = root.join("plain/project");
    let plain_sibling = root.join("plain/sibling");
    let repo = root.join("repo");
    let alpha = repo.join("packages/alpha");
    let beta = repo.join("packages/beta");
    fs::create_dir_all(&plain).unwrap();
    fs::create_dir_all(&plain_sibling).unwrap();
    fs::create_dir_all(&alpha).unwrap();
    fs::create_dir_all(&beta).unwrap();
    git(&repo, &["init", "--quiet"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/tendi-test.git",
        ],
    );

    let mut resolver = SessionRepositoryResolver::default();
    assert_eq!(resolver.resolve(&plain), (None, None));
    assert!(resolver.repositories.is_empty());
    let probes_after_plain = resolver.metadata_probes;
    assert_eq!(resolver.resolve(&plain_sibling), (None, None));
    assert_eq!(resolver.metadata_probes, probes_after_plain + 1);

    let alpha_result = resolver.resolve(&alpha);
    assert_eq!(resolver.resolve(&alpha), alpha_result);
    let probes_after_alpha = resolver.metadata_probes;
    let beta_result = resolver.resolve(&beta);
    assert_eq!(resolver.metadata_probes, probes_after_alpha + 1);
    assert_eq!(resolver.workspaces.len(), 4);
    assert_eq!(resolver.repositories.len(), 1);
    assert_eq!(alpha_result, beta_result);
    assert_eq!(alpha_result.0, Some(fs::canonicalize(&repo).unwrap()));
    assert_eq!(
        alpha_result.1.as_deref(),
        Some("https://github.com/example/tendi-test.git")
    );

    let nested_repo = repo.join("vendor/nested");
    let nested_workspace = nested_repo.join("src");
    fs::create_dir_all(&nested_workspace).unwrap();
    git(&nested_repo, &["init", "--quiet"]);
    let nested_result = resolver.resolve(&nested_workspace);
    assert_eq!(
        nested_result.0,
        Some(fs::canonicalize(&nested_repo).unwrap())
    );
    assert_ne!(nested_result, alpha_result);

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let alpha_link = root.join("alpha-link");
        symlink(&alpha, &alpha_link).unwrap();
        let probes_before_link = resolver.metadata_probes;
        assert_eq!(resolver.resolve(&alpha_link), alpha_result);
        assert_eq!(resolver.metadata_probes, probes_before_link);
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cursor_project_folder_decodes_to_workspace_path() {
    assert_eq!(
        decode_cursor_project_dir("Users-test-dev-example-nextop").as_deref(),
        Some(Path::new("/Users/test/dev/example/nextop"))
    );
}

#[test]
fn scans_additional_roots_by_transcript_format() {
    let root = temp_dir("tendi-additional-session-scan-test");
    let id = "5dcc0f66-1234-4f39-8e9d-123456789abc";
    let codex_session = root
        .join("private-run/state")
        .join(format!("rollout-{id}.jsonl"));
    let claude_session = root.join("private-run/claude-session.jsonl");
    let cursor_session = root.join("private-run/cursor-fixture.jsonl");
    fs::create_dir_all(codex_session.parent().unwrap()).unwrap();
    fs::write(
            &codex_session,
            concat!(
                r#"{"timestamp":"2026-07-30T10:00:00Z","type":"session_meta","payload":{"cwd":"/Users/test/dev/example-workspace","git":{"repository_url":"https://github.com/tutti-os/tutti.git"}}}"#,
                "\n",
                r#"{"timestamp":"2026-07-30T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Hello"}]}}"#
            ),
        )
        .unwrap();
    fs::write(
            &claude_session,
            r#"{"timestamp":"2026-07-30T10:00:00Z","type":"user","sessionId":"claude-session","message":{"role":"user","content":"Hello"}}"#,
        )
        .unwrap();
    fs::write(
        &cursor_session,
        r#"{"role":"user","message":{"content":"Hello"}}"#,
    )
    .unwrap();

    let mut sessions = Vec::new();
    scan_additional_session_roots(&[root.clone()], &mut sessions, None);

    assert_eq!(sessions.len(), 3);
    let codex = sessions
        .iter()
        .find(|session| session.agent == AgentKind::Codex)
        .unwrap();
    assert_eq!(codex.id, id);
    assert_eq!(
        codex.project.as_deref(),
        Some(Path::new("/Users/test/dev/example-workspace"))
    );
    assert_eq!(
        codex.repository_url.as_deref(),
        Some("https://github.com/tutti-os/tutti.git")
    );
    assert!(
        sessions
            .iter()
            .any(|session| session.agent == AgentKind::Claude)
    );
    assert!(
        sessions
            .iter()
            .any(|session| session.agent == AgentKind::Cursor)
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn skips_codex_session_with_only_session_metadata() {
    let root = temp_dir("tendi-empty-codex-session-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-empty-session-id.jsonl");
    fs::write(
            &path,
            r#"{"timestamp":"2026-08-13T11:58:34.979Z","type":"session_meta","payload":{"cwd":"/Users/test/dev/tendi"}}"#,
        )
        .unwrap();

    let mut sessions = Vec::new();
    scan_codex_jsonl(&root, &mut sessions, None);

    assert!(sessions.is_empty());
    assert!(!scan_jsonl_meta_for_agent(&path, None).has_content);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn skips_codex_session_with_only_injected_context_until_real_message_arrives() {
    let root = temp_dir("tendi-context-only-codex-session-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-context-only-session-id.jsonl");
    let context = r##"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions\n<INSTRUCTIONS>hidden</INSTRUCTIONS>"}]}}"##;
    fs::write(&path, context).unwrap();

    let mut sessions = Vec::new();
    scan_codex_jsonl(&root, &mut sessions, None);
    assert!(sessions.is_empty());
    assert!(!scan_jsonl_meta_for_agent(&path, Some(AgentKind::Codex)).has_content);

    let user_message = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Fix the session title"}]}}"#;
    fs::write(&path, format!("{context}\n{user_message}\n")).unwrap();

    scan_codex_jsonl(&root, &mut sessions, None);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title.as_deref(), Some("Fix the session title"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cached_empty_codex_session_is_rechecked_instead_of_reused() {
    let root = temp_dir("tendi-cached-empty-codex-session-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-empty-session-id.jsonl");
    fs::write(
            &path,
            r#"{"timestamp":"2026-08-13T11:58:34.979Z","type":"session_meta","payload":{"cwd":"/Users/test/dev/tendi"}}"#,
        )
        .unwrap();
    let (file_mtime, file_size) = file_state(&path).unwrap();
    let cache = SessionScanCache::from_entries([SessionScanCacheEntry {
        session: SessionRecord {
            id: "empty-session-id".to_string(),
            agent: AgentKind::Codex,
            title: None,
            project: None,
            repository: None,
            repository_url: None,
            logical_project_id: None,
            logical_project_name: None,
            path: path.clone(),
            started_at: None,
            updated_at: None,
            message_count: Some(1),
            first_user_message: None,
            last_user_message: None,
            last_assistant_message: None,
            turn_count: Some(0),
            model: None,
            mode: None,
            approval_mode: None,
            is_run_everything: None,
            parent_session_id: None,
            token_usage: None,
        },
        file_mtime,
        file_size,
        additional_file_states: Vec::new(),
    }]);

    let mut sessions = Vec::new();
    scan_codex_jsonl(&root, &mut sessions, Some(&cache));

    assert!(sessions.is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tutti_run_discovery_only_reads_the_run_session_directory() {
    let root = temp_dir("tendi-tutti-run-discovery-test").join("agent/runs");
    let expected = root.join(
        "run-one/codex-home/sessions/2026/08/12/rollout-12345678-1234-1234-1234-123456789012.jsonl",
    );
    let fixture = root.join(
            "run-one/codex-home/.tmp/plugins/fixtures/rollout-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.jsonl",
        );
    fs::create_dir_all(expected.parent().unwrap()).unwrap();
    fs::create_dir_all(fixture.parent().unwrap()).unwrap();
    fs::write(&expected, "{}\n").unwrap();
    fs::write(&fixture, "{}\n").unwrap();

    let mut paths = std::collections::BTreeSet::new();
    crate::providers::codex::collect_tutti_run_session_paths(&root, &mut paths);

    assert_eq!(paths.into_iter().collect::<Vec<_>>(), vec![expected]);
    fs::remove_dir_all(root.parent().unwrap().parent().unwrap()).unwrap();
}

#[test]
fn tutti_watch_plan_only_recurses_into_session_directories() {
    let base = temp_dir("tendi-tutti-watch-plan-test");
    let root = base.join("agent/runs");
    let sessions = root.join("run-one/codex-home/sessions");
    let unrelated = root.join("run-one/workspace/node_modules");
    fs::create_dir_all(&sessions).unwrap();
    fs::create_dir_all(&unrelated).unwrap();

    let plan = session_watch_plan(&base, std::slice::from_ref(&root));

    assert!(plan.dynamic_roots.contains(&root));
    assert!(
        plan.targets
            .iter()
            .any(|target| { target.path == root && !target.recursive })
    );
    assert!(
        plan.targets
            .iter()
            .any(|target| { target.path == sessions && target.recursive })
    );
    assert!(
        !plan
            .targets
            .iter()
            .any(|target| { target.path == unrelated })
    );
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn cached_codex_transcript_reuses_unchanged_metadata_and_invalidates_on_append() {
    let root = temp_dir("tendi-session-scan-cache-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-12345678-1234-1234-1234-123456789012.jsonl");
    let initial = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"First question"}]}}"#;
    fs::write(&path, initial).unwrap();

    let mut first_scan = Vec::new();
    scan_codex_jsonl(&root, &mut first_scan, None);
    assert_eq!(first_scan.len(), 1);
    assert_eq!(first_scan[0].message_count, Some(1));

    let (file_mtime, file_size) = file_state(&path).unwrap();
    let cache = SessionScanCache::from_entries([SessionScanCacheEntry {
        session: first_scan[0].clone(),
        file_mtime,
        file_size,
        additional_file_states: Vec::new(),
    }]);

    let mut reused_scan = Vec::new();
    scan_codex_jsonl(&root, &mut reused_scan, Some(&cache));
    assert_eq!(reused_scan[0].message_count, Some(1));
    assert_eq!(reused_scan[0].title, first_scan[0].title);

    fs::write(
            &path,
            format!(
                "{initial}\n{}",
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Answer"}]}}"#
            ),
        )
        .unwrap();

    let mut invalidated_scan = Vec::new();
    scan_codex_jsonl(&root, &mut invalidated_scan, Some(&cache));
    assert_eq!(invalidated_scan[0].message_count, Some(2));
    assert_eq!(
        invalidated_scan[0].last_assistant_message.as_deref(),
        Some("Answer")
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_scan_cache_invalidates_when_an_additional_source_changes() {
    let root = temp_dir("tendi-session-scan-additional-source-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-12345678-1234-1234-1234-123456789012.jsonl");
    let additional = root.join("metadata.json");
    fs::write(
            &path,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Question"}]}}"#,
        )
        .unwrap();
    fs::write(&additional, "initial metadata").unwrap();

    let mut sessions = Vec::new();
    scan_codex_jsonl(&root, &mut sessions, None);
    let session = sessions.into_iter().next().unwrap();
    let (file_mtime, file_size) = file_state(&path).unwrap();
    let (additional_mtime, additional_size) = file_state(&additional).unwrap();
    let cache = SessionScanCache::from_entries([SessionScanCacheEntry {
        session: session.clone(),
        file_mtime,
        file_size,
        additional_file_states: vec![SessionScanSourceState {
            path: additional.clone(),
            file_mtime: additional_mtime,
            file_size: additional_size,
        }],
    }]);

    assert!(cache.session_if_current(AgentKind::Codex, &path).is_some());
    fs::write(&additional, "updated metadata with a changed size").unwrap();
    assert!(cache.session_if_current(AgentKind::Codex, &path).is_none());

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn session_scan_cache_reuses_hard_linked_transcript_across_roots() {
    let root = temp_dir("tendi-session-hard-link-cache-test");
    let primary = root.join("primary/session.jsonl");
    let alias = root.join("alias/session.jsonl");
    fs::create_dir_all(primary.parent().unwrap()).unwrap();
    fs::create_dir_all(alias.parent().unwrap()).unwrap();
    fs::write(&primary, "{}\n").unwrap();
    fs::hard_link(&primary, &alias).unwrap();
    let (file_mtime, file_size) = file_state(&primary).unwrap();
    let cached_session = SessionRecord {
        id: "session-id".to_string(),
        agent: AgentKind::Codex,
        title: Some("Cached title".to_string()),
        project: None,
        repository: None,
        repository_url: None,
        logical_project_id: None,
        logical_project_name: None,
        path: primary.clone(),
        started_at: None,
        updated_at: None,
        message_count: Some(1),
        first_user_message: None,
        last_user_message: None,
        last_assistant_message: None,
        turn_count: Some(1),
        model: None,
        mode: None,
        approval_mode: None,
        is_run_everything: None,
        parent_session_id: None,
        token_usage: None,
    };
    let cache = SessionScanCache::from_entries([SessionScanCacheEntry {
        session: cached_session.clone(),
        file_mtime,
        file_size,
        additional_file_states: Vec::new(),
    }]);

    assert_eq!(
        cache.session_if_current(AgentKind::Codex, &alias),
        Some(cached_session)
    );
    assert_eq!(cache.agent_for_path(&alias), Some(AgentKind::Codex));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_candidate_discovery_uses_provider_owned_files() {
    assert!(is_session_candidate_path(Path::new("/tmp/session.jsonl")));
    assert!(is_session_candidate_path(Path::new(
        "/tmp/cursor/meta.json"
    )));
    assert!(is_session_candidate_path(Path::new("/tmp/cursor/store.db")));
    assert!(!is_session_candidate_path(Path::new(
        "/tmp/cursor/state.json"
    )));
}

#[test]
fn extracts_titles_from_user_messages() {
    let cursor = json!({
        "role": "user",
        "message": {
            "content": [{ "type": "text", "text": "<user_query>\nFix the install progress UI\n</user_query>" }]
        }
    });
    let codex = json!({
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "sessions 页面修一下标题" }]
        }
    });

    assert_eq!(
        extract_session_title_for_agent(AgentKind::Cursor, &cursor),
        Some("Fix the install progress UI".to_string())
    );
    assert_eq!(
        extract_session_title_for_agent(AgentKind::Codex, &codex),
        Some("sessions 页面修一下标题".to_string())
    );
    let codex_with_injected_context = json!({
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "user",
            "content": [
                { "type": "input_text", "text": "<recommended_plugins>\nhidden\n</recommended_plugins>" },
                { "type": "input_text", "text": "<environment_context>hidden</environment_context>" },
                { "type": "input_text", "text": "The real user request" }
            ]
        }
    });
    assert_eq!(
        extract_session_title_for_agent(AgentKind::Codex, &codex_with_injected_context),
        Some("The real user request".to_string())
    );
    let codex_with_embedded_context = json!({
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "user",
            "content": "Real request\n<turn_aborted>\nThe previous turn was interrupted.\n</turn_aborted>\nNext request"
        }
    });
    assert_eq!(
        extract_session_title_for_agent(AgentKind::Codex, &codex_with_embedded_context),
        Some("Real request".to_string())
    );

    let codex_selected_skill = json!({
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": "<skill><name>foo</name><path>/tmp/foo/SKILL.md</path></skill>"
            }],
            "internal_chat_message_metadata_passthrough": {
                "content_item_kinds": ["skills.selected_skill_instructions"]
            }
        }
    });
    assert_eq!(
        extract_session_title_for_agent(AgentKind::Codex, &codex_selected_skill),
        None
    );
    assert_eq!(extract_session_message(&codex_selected_skill), None);
    assert_eq!(
        clean_title(
            "<user_info>Ryan</user_info>\n<timestamp>today</timestamp>\n<user_query>\nUse the Cursor transcript title\n</user_query>"
        ),
        Some("Use the Cursor transcript title".to_string())
    );
    assert_eq!(
        clean_preview_text(
            "<environment_context>hidden</environment_context>\n<user_query>\nShow the session preview\n</user_query>"
        ),
        Some("Show the session preview".to_string())
    );
    assert_eq!(
        clean_preview_text(
            "<image name=[Image #1] path=\"/tmp/image.png\">\nShow the session preview"
        ),
        Some("Show the session preview".to_string())
    );
    assert_eq!(
        clean_preview_text("<image name=[Image #1] path=\"/tmp/image.png\">"),
        None
    );
    assert_eq!(clean_preview_text("<recommended_plugins>hidden"), None);
    assert_eq!(
        clean_title("<image name=[Image #1] path=\"/tmp/image.png\">"),
        None
    );
    assert_eq!(
        clean_title("<image name=[Image #1] path=\"/tmp/image.png\""),
        None
    );
    assert_eq!(
        clean_title("<image name=[Image #1] path=\"/tmp/image.png\">\nFollow-up request"),
        Some("Follow-up request".to_string())
    );
    assert_eq!(
        clean_title("<image name=[Image #1] path=\"/tmp/image.png\"> Follow-up request"),
        Some("Follow-up request".to_string())
    );
    assert_eq!(
        clean_title("![Image #1](/tmp/image.png) Follow-up request"),
        Some("Follow-up request".to_string())
    );
    let long_title = format!(
        "<image name=[Image #1] path=\"/tmp/image.png\">\n{}",
        "x".repeat(120)
    );
    let parsed_long_title = clean_title(&long_title).unwrap();
    assert_eq!(parsed_long_title.chars().count(), 96);
    assert!(parsed_long_title.chars().all(|character| character == 'x'));
    let oversized_preview = "界".repeat(SESSION_PREVIEW_MAX_CHARS + 1);
    let bounded_preview = clean_preview_text(&oversized_preview).unwrap();
    assert_eq!(
        bounded_preview.chars().count(),
        SESSION_PREVIEW_MAX_CHARS + 1
    );
    assert!(bounded_preview.ends_with('…'));
    assert_eq!(clean_title("# AGENTS.md instructions\nhidden"), None);
}

#[test]
fn cursor_embedded_timestamps_define_session_bounds() {
    let root = temp_dir("tendi-cursor-embedded-timestamp-session");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session.jsonl");
    let first = json!({
        "role": "user",
        "message": {
            "content": [{
                "type": "text",
                "text": "<timestamp>Thursday, Aug 27, 2026, 11:01 PM (UTC+8)</timestamp>\n<user_query>First</user_query>"
            }]
        }
    });
    let second = json!({
        "role": "user",
        "message": {
            "content": [{
                "type": "text",
                "text": "<timestamp>Friday, Aug 28, 2026, 12:05 AM (UTC+8)</timestamp>\n<user_query>Second</user_query>"
            }]
        }
    });
    fs::write(&path, format!("{first}\n{second}\n")).unwrap();

    let meta = scan_jsonl_meta_for_agent(&path, Some(AgentKind::Cursor));

    assert_eq!(
        meta.started_at.as_deref(),
        Some("2026-08-27T23:01:00+08:00")
    );
    assert_eq!(
        meta.updated_at.as_deref(),
        Some("2026-08-28T00:05:00+08:00")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn child_jsonl_title_skips_inherited_history_without_spawn_task_name() {
    let root = temp_dir("tendi-child-title-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-12345678-1234-1234-1234-123456789012.jsonl");
    fs::write(
            &path,
            [
                r#"{"ordinal":0,"type":"session_meta","payload":{"parent_thread_id":"parent-id","thread_source":"subagent","subagent_history_start_ordinal":3}}"#,
                r#"{"ordinal":1,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Parent orchestration"}]}}"#,
                r#"{"ordinal":2,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Inherited label"}]}}"#,
                r#"{"ordinal":3,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Child-specific task"}]}}"#,
                r#"{"ordinal":4,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Follow-up"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

    assert_eq!(
        scan_jsonl_meta_for_agent(&path, Some(AgentKind::Codex))
            .title
            .as_deref(),
        Some("Child-specific task")
    );

    let mut first_scan = Vec::new();
    scan_codex_jsonl(&root, &mut first_scan, None);
    assert_eq!(
        crate::providers::codex::session_title(&path).as_deref(),
        Some("Child-specific task")
    );
    let (file_mtime, file_size) = file_state(&path).unwrap();
    let mut cached_session = first_scan[0].clone();
    cached_session.title = Some("Follow-up".to_string());
    cached_session.first_user_message = Some("Inherited label".to_string());
    assert!(session_requires_rescan(&cached_session));
    let cache = SessionScanCache::from_entries([SessionScanCacheEntry {
        session: cached_session,
        file_mtime,
        file_size,
        additional_file_states: Vec::new(),
    }]);

    let mut rescanned = Vec::new();
    scan_codex_jsonl(&root, &mut rescanned, Some(&cache));

    assert_eq!(rescanned[0].title.as_deref(), Some("Child-specific task"));
    assert_eq!(
        rescanned[0].first_user_message.as_deref(),
        Some("Child-specific task")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn child_jsonl_title_uses_spawn_task_name_and_refreshes_cached_parent_title() {
    let root = temp_dir("tendi-child-spawn-task-title-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-12345678-1234-1234-1234-123456789012.jsonl");
    fs::write(
            &path,
            [
                r#"{"type":"session_meta","payload":{"parent_thread_id":"parent-id","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-id","task_name":"composer_connection_reuse","agent_path":"/root/legacy_name","agent_nickname":"Schrodinger"}}},"agent_path":"/root/legacy_name"}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Parent orchestration"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

    assert_eq!(
        scan_jsonl_meta_for_agent(&path, None).title.as_deref(),
        Some("composer_connection_reuse")
    );

    let mut first_scan = Vec::new();
    scan_codex_jsonl(&root, &mut first_scan, None);
    let (file_mtime, file_size) = file_state(&path).unwrap();
    let mut cached_session = first_scan[0].clone();
    cached_session.title = Some("Parent orchestration".to_string());
    let cache = SessionScanCache::from_entries([SessionScanCacheEntry {
        session: cached_session,
        file_mtime,
        file_size,
        additional_file_states: Vec::new(),
    }]);

    let mut rescanned = Vec::new();
    scan_codex_jsonl(&root, &mut rescanned, Some(&cache));

    assert_eq!(
        rescanned[0].title.as_deref(),
        Some("composer_connection_reuse")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn child_jsonl_cache_clears_inherited_preview_without_child_user_message() {
    let root = temp_dir("tendi-child-preview-boundary-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-12345678-1234-1234-1234-123456789012.jsonl");
    fs::write(
            &path,
            [
                r#"{"ordinal":0,"type":"session_meta","payload":{"parent_thread_id":"parent-id","thread_source":"subagent","subagent_history_start_ordinal":2,"source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-id","task_name":"child_without_user"}}}}}"#,
                r#"{"ordinal":1,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Parent user message"}]}}"#,
                r#"{"ordinal":2,"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Child response"}]}}"#,
                r#"{"ordinal":3,"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Child follow-up"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

    let mut first_scan = Vec::new();
    scan_codex_jsonl(&root, &mut first_scan, None);
    assert_eq!(first_scan[0].title.as_deref(), Some("child_without_user"));
    assert_eq!(first_scan[0].first_user_message, None);
    assert_eq!(first_scan[0].last_user_message, None);
    assert_eq!(
        first_scan[0].last_assistant_message.as_deref(),
        Some("Child follow-up")
    );

    let (file_mtime, file_size) = file_state(&path).unwrap();
    let mut cached_session = first_scan[0].clone();
    cached_session.first_user_message = Some("Parent user message".to_string());
    cached_session.last_user_message = Some("Parent user message".to_string());
    assert!(session_requires_rescan(&cached_session));
    let cache = SessionScanCache::from_entries([SessionScanCacheEntry {
        session: cached_session,
        file_mtime,
        file_size,
        additional_file_states: Vec::new(),
    }]);

    let mut rescanned = Vec::new();
    scan_codex_jsonl(&root, &mut rescanned, Some(&cache));

    assert_eq!(rescanned[0].first_user_message, None);
    assert_eq!(rescanned[0].last_user_message, None);
    assert_eq!(
        rescanned[0].last_assistant_message.as_deref(),
        Some("Child follow-up")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn codex_index_ignores_injected_thread_names() {
    let root = temp_dir("tendi-codex-index-injected-title-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session_index.jsonl");
    fs::write(
        &path,
        r#"{"id":"session-id","thread_name":"<recommended_plugins>"}"#,
    )
    .unwrap();

    let mut sessions = Vec::new();
    let mut warnings = Vec::new();
    scan_codex_index(&path, &mut sessions, &mut warnings).unwrap();

    assert_eq!(warnings, Vec::<String>::new());
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, None);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn counts_only_real_user_turns() {
    let root = temp_dir("tendi-session-turn-count-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session.jsonl");
    fs::write(
            &path,
            [
                r##"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<recommended_plugins>\nhidden\n</recommended_plugins>"},{"type":"input_text","text":"# AGENTS.md instructions\nhidden"},{"type":"input_text","text":"<environment_context>hidden</environment_context>"}]}}"##,
                r##"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions\nhidden"}]}}"##,
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"First question"}]}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"First answer"}]}}"#,
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"tool output"}]}}"#,
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"<user_query>\nSecond question\n</user_query>"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

    let meta = scan_jsonl_meta_for_agent(&path, None);

    assert_eq!(meta.message_count, Some(6));
    assert_eq!(meta.turn_count, Some(2));
    assert_eq!(meta.first_user_message.as_deref(), Some("First question"));
    assert_eq!(meta.last_user_message.as_deref(), Some("Second question"));
    assert_eq!(meta.last_assistant_message.as_deref(), Some("First answer"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn codex_goal_message_provides_session_title_and_user_preview() {
    let root = temp_dir("tendi-codex-goal-title-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-12345678-1234-1234-1234-123456789012.jsonl");
    fs::write(
            &path,
            [
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<codex_internal_context source=\"goal\">\n<objective>\nShip the release:\nFollow the checklist\n</objective>\n</codex_internal_context>"}]}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"I will start."}]}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Check the final result"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

    let meta = scan_jsonl_meta_for_agent(&path, Some(AgentKind::Codex));

    assert_eq!(meta.title.as_deref(), Some("Ship the release:"));
    assert_eq!(
        meta.first_user_message.as_deref(),
        Some("Ship the release: Follow the checklist")
    );
    assert_eq!(
        meta.last_user_message.as_deref(),
        Some("Check the final result")
    );
    assert_eq!(meta.turn_count, Some(2));
    assert_eq!(
        crate::providers::codex::session_title(&path).as_deref(),
        Some("Ship the release:")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn codex_goal_title_overrides_index_thread_name() {
    let root = temp_dir("tendi-codex-goal-index-title-test");
    fs::create_dir_all(&root).unwrap();
    fs::write(
            root.join("rollout-12345678-1234-1234-1234-123456789012.jsonl"),
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<codex_internal_context source=\"goal\"><objective>Goal title</objective></codex_internal_context>"}]}}"#,
        )
        .unwrap();

    let mut transcript_sessions = Vec::new();
    scan_codex_jsonl(&root, &mut transcript_sessions, None);
    assert_eq!(transcript_sessions.len(), 1);
    let mut index_session = transcript_sessions[0].clone();
    index_session.path = root.join("session_index.jsonl");
    index_session.title = Some("Index thread name".to_string());
    index_session.first_user_message = None;

    let sessions = merge_sessions(vec![index_session, transcript_sessions.remove(0)]);

    assert_eq!(sessions[0].title.as_deref(), Some("Goal title"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_fast_path_keeps_time_bounds_and_user_turns() {
    let root = temp_dir("tendi-session-metadata-fast-path-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session.jsonl");
    let tool_output = "x".repeat(super::METADATA_HINT_PREFIX_BYTES * 2);
    fs::write(
            &path,
            [
                format!(
                    r#"{{"timestamp":"2026-08-12T10:00:00Z","type":"response_item","payload":{{"type":"custom_tool_call_output","output":"{tool_output}"}}}}"#
                ),
                r#"{"timestamp":"2026-08-12T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Check CPU usage"}]}}"#.to_string(),
            ]
            .join("\n"),
        )
        .unwrap();

    let meta = scan_jsonl_meta_for_agent(&path, None);

    assert_eq!(meta.started_at.as_deref(), Some("2026-08-12T10:00:00Z"));
    assert_eq!(meta.updated_at.as_deref(), Some("2026-08-12T10:00:01Z"));
    assert_eq!(meta.message_count, Some(2));
    assert_eq!(meta.turn_count, Some(1));
    assert_eq!(meta.title.as_deref(), Some("Check CPU usage"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cursor_meta_skips_project_only_empty_sessions() {
    let root = temp_dir("tendi-cursor-empty-meta-test");
    let session_dir = root.join("session-id");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
            session_dir.join("meta.json"),
            r#"{"schemaVersion":1,"cwd":"/Users/test/.tutti-dev/apps/installations/ai-slide/f18a5113a098dcd2/runtime"}"#,
        )
        .unwrap();

    let mut sessions = Vec::new();
    scan_cursor_meta(&root, &mut sessions, AgentKind::Cursor, None);

    assert!(sessions.is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn cursor_meta_keeps_explicit_title_without_messages() {
    let root = temp_dir("tendi-cursor-titled-meta-test");
    let session_dir = root.join("session-id");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
            session_dir.join("meta.json"),
            r#"{"schemaVersion":1,"name":"<image name=[Image #1] path=\"/tmp/image.png\"> Pinned Cursor session","cwd":"/Users/test/dev/tendi"}"#,
        )
        .unwrap();

    let mut sessions = Vec::new();
    scan_cursor_meta(&root, &mut sessions, AgentKind::Cursor, None);

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title.as_deref(), Some("Pinned Cursor session"));
    assert_eq!(sessions[0].message_count, None);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn cursor_store_metadata_links_subagent_transcript_to_parent() {
    let root = temp_dir("tendi-cursor-subagent-test");
    let chats_root = root.join("chats");
    let transcripts_root = root.join("projects");
    let child_id = "3207a1c6-bfb3-41e6-bf81-cb557da65f35";
    let parent_id = "f524c2b4-7841-43f7-b177-ccd35f440bc1";
    let child_dir = chats_root.join("workspace").join(child_id);
    let transcript_dir = transcripts_root
        .join("workspace/agent-transcripts")
        .join(child_id);
    fs::create_dir_all(&child_dir).unwrap();
    fs::create_dir_all(&transcript_dir).unwrap();

    let connection = Connection::open(child_dir.join("store.db")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
    let metadata = json!({
        "agentId": child_id,
        "name": "New Agent",
        "createdAt": 1785420350441_i64,
        "mode": "default",
        "approvalMode": "unrestricted",
        "isRunEverything": false,
        "subagentInfo": {
            "parentAgentId": parent_id,
            "rootParentAgentId": parent_id,
            "toolCallId": "toolu_test"
        }
    })
    .to_string();
    let encoded = metadata
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    connection
        .execute("INSERT INTO meta (key, value) VALUES ('0', ?1)", [&encoded])
        .unwrap();
    let message_blob = json!({
        "role": "assistant",
        "content": [{
            "type": "reasoning",
            "providerOptions": {
                "cursor": { "modelName": "claude-fable-5-thinking-high" }
            }
        }]
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO blobs (id, data) VALUES ('message', ?1)",
            [message_blob.as_bytes()],
        )
        .unwrap();
    drop(connection);
    fs::write(
            transcript_dir.join(format!("{child_id}.jsonl")),
            r#"{"role":"user","message":{"content":[{"type":"text","text":"Investigate queue replay"}]}}"#,
        )
        .unwrap();

    let mut sessions = Vec::new();
    scan_cursor_meta(&chats_root, &mut sessions, AgentKind::Cursor, None);
    scan_jsonl_sessions(&transcripts_root, AgentKind::Cursor, 4, &mut sessions, None);
    let sessions = merge_sessions(sessions);

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, child_id);
    assert_eq!(
        sessions[0].title.as_deref(),
        Some("Investigate queue replay")
    );
    assert_eq!(sessions[0].parent_session_id.as_deref(), Some(parent_id));
    assert_eq!(
        sessions[0].model.as_deref(),
        Some("claude-fable-5-thinking-high")
    );
    assert_eq!(sessions[0].mode.as_deref(), Some("default"));
    assert_eq!(sessions[0].approval_mode.as_deref(), Some("unrestricted"));
    assert_eq!(sessions[0].is_run_everything, Some(false));
    assert_eq!(
        sessions[0].path,
        transcript_dir.join(format!("{child_id}.jsonl"))
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn cursor_store_changes_invalidate_cached_subagent_metadata() {
    let root = temp_dir("tendi-cursor-subagent-cache-test");
    let session_dir = root.join("session-id");
    fs::create_dir_all(&session_dir).unwrap();
    let meta_path = session_dir.join("meta.json");
    let store_path = session_dir.join("store.db");
    fs::write(&meta_path, r#"{"schemaVersion":1,"name":"New Agent"}"#).unwrap();

    let connection = Connection::open(&store_path).unwrap();
    connection
        .execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);")
        .unwrap();
    let initial_metadata = json!({
        "agentId": "session-id",
        "name": "New Agent",
        "createdAt": 1785420350441_i64
    })
    .to_string();
    let encoded = initial_metadata
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    connection
        .execute("INSERT INTO meta (key, value) VALUES ('0', ?1)", [&encoded])
        .unwrap();
    drop(connection);

    let mut sessions = Vec::new();
    cursor_sessions::scan_cursor_meta_file(&meta_path, &mut sessions, AgentKind::Cursor, None);
    let session = sessions.pop().unwrap();
    assert_eq!(session.parent_session_id, None);
    let (file_mtime, file_size) = file_state(&meta_path).unwrap();
    let (store_mtime, store_size) = file_state(&store_path).unwrap();
    let cache = SessionScanCache::from_entries([SessionScanCacheEntry {
        session,
        file_mtime,
        file_size,
        additional_file_states: vec![SessionScanSourceState {
            path: store_path.clone(),
            file_mtime: store_mtime,
            file_size: store_size,
        }],
    }]);

    std::thread::sleep(Duration::from_millis(2));
    let connection = Connection::open(&store_path).unwrap();
    let updated_metadata = json!({
        "agentId": "session-id",
        "name": "New Agent",
        "createdAt": 1785420350441_i64,
        "subagentInfo": { "parentAgentId": "parent-session" }
    })
    .to_string();
    let encoded = updated_metadata
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    connection
        .execute("UPDATE meta SET value = ?1 WHERE key = '0'", [&encoded])
        .unwrap();
    drop(connection);

    let mut rescanned = Vec::new();
    cursor_sessions::scan_cursor_meta_file(
        &meta_path,
        &mut rescanned,
        AgentKind::Cursor,
        Some(&cache),
    );
    assert_eq!(rescanned.len(), 1);
    assert_eq!(
        rescanned[0].parent_session_id.as_deref(),
        Some("parent-session")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn cursor_transcript_path_replaces_metadata_path() {
    assert!(should_replace_session_path(
        AgentKind::Cursor,
        Path::new("/Users/test/.cursor/chats/session-id/meta.json"),
        Path::new(
            "/Users/test/.cursor/projects/project/agent-transcripts/session-id/session-id.jsonl"
        )
    ));
    assert!(!should_replace_session_path(
        AgentKind::Cursor,
        Path::new(
            "/Users/test/.cursor/projects/project/agent-transcripts/session-id/session-id.jsonl"
        ),
        Path::new("/Users/test/.cursor/chats/session-id/meta.json")
    ));
}

#[test]
fn codex_transcript_project_fills_index_session() {
    let sessions = merge_sessions(vec![
        SessionRecord {
            id: "session-id".to_string(),
            agent: AgentKind::Codex,
            title: Some("Index title".to_string()),
            project: None,
            repository: None,
            repository_url: None,
            logical_project_id: None,
            logical_project_name: None,
            path: PathBuf::from("/Users/test/.codex/session_index.jsonl"),
            started_at: None,
            updated_at: Some("2026-06-23T14:54:04.092592Z".to_string()),
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
        },
        SessionRecord {
            id: "session-id".to_string(),
            agent: AgentKind::Codex,
            title: None,
            project: Some(PathBuf::from("/Users/test/dev/tendi")),
            repository: None,
            repository_url: None,
            logical_project_id: None,
            logical_project_name: None,
            path: PathBuf::from("/Users/test/.codex/archived_sessions/rollout-session-id.jsonl"),
            started_at: Some("2026-06-23T14:53:56.489Z".to_string()),
            updated_at: Some("2026-06-23T14:53:56.489Z".to_string()),
            message_count: Some(8),
            first_user_message: None,
            last_user_message: None,
            last_assistant_message: None,
            turn_count: Some(2),
            model: Some("gpt-5.6-sol".to_string()),
            mode: None,
            approval_mode: None,
            is_run_everything: None,
            parent_session_id: Some("parent-session".to_string()),
            token_usage: None,
        },
    ]);

    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].project.as_deref(),
        Some(Path::new("/Users/test/dev/tendi"))
    );
    assert_eq!(sessions[0].title.as_deref(), Some("Index title"));
    assert_eq!(sessions[0].message_count, Some(8));
    assert_eq!(sessions[0].turn_count, Some(2));
    assert_eq!(sessions[0].model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(
        sessions[0].parent_session_id.as_deref(),
        Some("parent-session")
    );
    assert_eq!(
        sessions[0].path,
        PathBuf::from("/Users/test/.codex/archived_sessions/rollout-session-id.jsonl")
    );
}

#[test]
fn infers_session_project_from_codex_jsonl_cwd() {
    let root = temp_dir("tendi-session-project-infer-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-session-id.jsonl");
    fs::write(
            &path,
            r#"{"timestamp":"2026-06-22T11:21:33.250Z","type":"session_meta","payload":{"id":"session-id","cwd":"/Users/test/dev/_scripts"}}"#,
        )
        .unwrap();

    assert_eq!(
        infer_session_project(&path, AgentKind::Codex).as_deref(),
        Some(Path::new("/Users/test/dev/_scripts"))
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn normalizes_ephemeral_chat_projects_without_touching_real_workspaces() {
    let mut sessions = vec![
        SessionRecord {
            id: "agent-chat".to_string(),
            agent: AgentKind::Claude,
            title: None,
            project: Some(PathBuf::from(
                "/Users/test/Documents/tutti/session-c98d1ced-5371-43cc-8173-9416c349a776",
            )),
            repository: None,
            repository_url: None,
            logical_project_id: None,
            logical_project_name: None,
            path: PathBuf::from("/tmp/codex-chat.jsonl"),
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
        },
        SessionRecord {
            id: "real-workspace".to_string(),
            agent: AgentKind::Codex,
            title: None,
            project: Some(PathBuf::from("/Users/test/dev/tendi")),
            repository: None,
            repository_url: None,
            logical_project_id: None,
            logical_project_name: None,
            path: PathBuf::from("/tmp/real-workspace.jsonl"),
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
        },
    ];

    let mut archive = sessions[0].clone();
    archive.id = "codex-archive".to_string();
    archive.agent = AgentKind::Codex;
    archive.project = Some(PathBuf::from("/Users/test/Documents/Codex/2026-08-09/c"));
    archive.path = PathBuf::from("/tmp/codex-archive.jsonl");
    sessions.push(archive);
    let mut tutti = sessions[0].clone();
    tutti.id = "codex-tutti".to_string();
    tutti.agent = AgentKind::Codex;
    tutti.project = Some(PathBuf::from(
        "/Users/test/Documents/tutti/session-c98d1ced-5371-43cc-8173-9416c349a776",
    ));
    tutti.path = PathBuf::from("/tmp/codex-tutti.jsonl");
    sessions.push(tutti);
    normalize_session_projects(&mut sessions);

    assert_eq!(
        sessions[0].project.as_deref(),
        Some(Path::new(
            "/Users/test/Documents/tutti/session-c98d1ced-5371-43cc-8173-9416c349a776",
        ))
    );
    assert_eq!(
        sessions[1].project.as_deref(),
        Some(Path::new("/Users/test/dev/tendi"))
    );
    assert_eq!(
        sessions[2].project.as_deref(),
        Some(Path::new("/Users/test/Documents/Codex"))
    );
    assert_eq!(
        sessions[3].project.as_deref(),
        Some(Path::new("/Users/test/Documents/tutti"))
    );
}

#[test]
fn extracts_latest_codex_session_cache_usage() {
    let root = temp_dir("tendi-session-token-usage-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-session-id.jsonl");
    fs::write(
            &path,
            concat!(
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":40,\"output_tokens\":20,\"reasoning_output_tokens\":5,\"total_tokens\":120}}}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":250,\"cached_input_tokens\":175,\"output_tokens\":50,\"reasoning_output_tokens\":10,\"total_tokens\":300}}}}\n"
            ),
        )
        .unwrap();

    let usage = scan_jsonl_meta_for_agent(&path, None).token_usage.unwrap();

    assert_eq!(usage.input_tokens, 250);
    assert_eq!(usage.cached_input_tokens, 175);
    assert_eq!(usage.output_tokens, 50);
    assert_eq!(usage.reasoning_output_tokens, 10);
    assert_eq!(usage.total_tokens, 300);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn extracts_deduplicated_claude_session_usage() {
    let root = temp_dir("tendi-claude-token-usage-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session-id.jsonl");
    fs::write(
            &path,
            concat!(
                "{\"type\":\"assistant\",\"message\":{\"id\":\"msg-1\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"msg-1\",\"usage\":{\"input_tokens\":10,\"cache_creation_input_tokens\":20,\"cache_read_input_tokens\":30,\"output_tokens\":5}}}\n",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"msg-2\",\"usage\":{\"input_tokens\":5,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":95,\"output_tokens\":10}}}\n",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"msg-2\",\"usage\":{\"input_tokens\":5,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":95,\"output_tokens\":10}}}\n",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"msg-empty\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n"
            ),
        )
        .unwrap();

    let usage = scan_jsonl_meta_for_agent(&path, None).token_usage.unwrap();

    assert_eq!(usage.input_tokens, 160);
    assert_eq!(usage.cached_input_tokens, 125);
    assert_eq!(usage.output_tokens, 15);
    assert_eq!(usage.reasoning_output_tokens, 0);
    assert_eq!(usage.total_tokens, 175);
    let _ = fs::remove_dir_all(root);
}
