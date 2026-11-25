use super::*;
use crate::providers::{AgentProvider, ProviderContext, antigravity::AntigravityProvider};

#[test]
fn discovers_app_cli_and_t3_acp_session_sources() {
    let home = std::env::temp_dir().join(format!(
        "tendi-agy-discovery-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let [cli, app] = data_roots(&home).collect::<Vec<_>>().try_into().unwrap();
    let t3 = home
        .join(T3_PROFILES)
        .join("test-profile")
        .join(T3_APP_DATA);
    const T3_ID: &str = "11111111-2222-4333-8444-555555555555";
    let t3_path = transcript_path(&t3, T3_ID);
    let cli_path = transcript_path(&cli, "ec33ebf9-0cba-4100-8142-c61503f6c587");
    let app_path = transcript_path(&app, "055a398f-db14-4c5f-abbb-1bf03f8120a7");
    for path in [&app_path, &cli_path, &t3_path] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            include_str!("../../testdata/transcripts/antigravity.jsonl"),
        )
        .unwrap();
        fs::copy(path, path.with_file_name("transcript_full.jsonl")).unwrap();
    }
    let summary = Connection::open(summary_path(&t3)).unwrap();
    summary
        .execute_batch(
            "CREATE TABLE conversation_summaries (
            conversation_id TEXT, title TEXT, workspace_uris TEXT, parent_conversation_id TEXT
        )",
        )
        .unwrap();
    summary
        .execute(
            "INSERT INTO conversation_summaries VALUES (?1, 'Native title', NULL, NULL)",
            [T3_ID],
        )
        .unwrap();
    let t3_store = Connection::open(home.join(".t3/userdata/statev2.sqlite")).unwrap();
    t3_store.execute_batch(
        "CREATE TABLE orchestration_v2_projection_threads (
            thread_id TEXT, title TEXT, active_provider_thread_id TEXT, updated_at TEXT, deleted_at TEXT
         );
         CREATE TABLE orchestration_v2_projection_provider_threads (
            provider_thread_id TEXT, thread_id TEXT, driver TEXT, payload_json TEXT
         );
         INSERT INTO orchestration_v2_projection_threads VALUES ('thread', 'Title shown in T3', 'active', '2026-11-25', NULL);",
    ).unwrap();
    let payload = serde_json::json!({"nativeThreadRef": {"nativeId": T3_ID}}).to_string();
    t3_store.execute(
        "INSERT INTO orchestration_v2_projection_provider_threads VALUES ('active', 'thread', 'antigravity', ?1)",
        [payload],
    ).unwrap();
    let ctx = ProviderContext {
        home: Some(home.clone()),
        project_dirs: vec![home.clone()],
    };
    let mut records = Vec::new();
    let mut warnings = Vec::new();
    AntigravityProvider
        .scan_sessions(&ctx, &mut records, &mut warnings, None)
        .unwrap();
    assert!(warnings.is_empty());
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].path, cli_path);
    assert_eq!(records[1].path, app_path);
    assert_eq!(records[2].path, t3_path);
    assert_eq!(records[2].title.as_deref(), Some("Title shown in T3"));
    assert!(!records[2].resumable);
    let roots = AntigravityProvider.session_roots(&ctx);
    assert!(roots.contains(&t3.join("brain")));
    assert!(roots.contains(&summary_path(&t3)));
    assert_eq!(
        summary_watch_source(&t3.join("conversation_summaries.db-wal")),
        Some(summary_path(&t3)),
    );
    let current = AntigravityProvider
        .current_session_transcript(&ctx, T3_ID, &BTreeMap::new())
        .unwrap();
    assert_eq!(current.as_deref(), Some(t3_path.as_path()));
    assert!(Source::from_path(&cli_path).unwrap().is_native_cli(&home));
    assert!(!Source::from_path(&app_path).unwrap().is_native_cli(&home));
    assert!(
        AntigravityProvider
            .session_roots(&ctx)
            .contains(&cli.join("brain"))
    );
    assert!(
        AntigravityProvider
            .session_roots(&ctx)
            .contains(&app.join("brain"))
    );
    assert_eq!(
        summary_watch_source(&app.join("conversation_summaries.db-wal")),
        Some(summary_path(&app))
    );

    let imported = sessions::scan_sessions_with_additional_roots(&home, &[home.clone()]).unwrap();
    assert!(
        imported
            .sessions
            .iter()
            .any(|session| session.path == cli_path)
    );
    assert!(
        imported
            .sessions
            .iter()
            .any(|session| session.path == app_path)
    );
    fs::remove_dir_all(home).unwrap();
}
