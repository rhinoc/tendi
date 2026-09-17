use std::{
    fs::{self, OpenOptions},
    io::Write,
    time::{SystemTime, UNIX_EPOCH},
};

use super::*;
use chrono::{Offset, TimeZone};

fn temp_dir(name: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("{name}-{suffix}"))
}

fn session(path: &Path, agent: AgentKind) -> SessionRecord {
    SessionRecord {
        id: "session-1".to_string(),
        agent,
        title: None,
        project: None,
        repository: None,
        repository_url: None,
        logical_project_id: None,
        logical_project_name: None,
        path: path.to_path_buf(),
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
fn cumulative_usage_diff_ignores_duplicates_and_handles_reset() {
    let first = AnalyticsTokenUsage {
        input_tokens: 100,
        total_tokens: 120,
        ..AnalyticsTokenUsage::default()
    };
    assert_eq!(diff_usage(first, first).total_tokens, 0);
    let reset = AnalyticsTokenUsage {
        input_tokens: 10,
        total_tokens: 12,
        ..AnalyticsTokenUsage::default()
    };
    assert_eq!(diff_usage(first, reset), reset);
}

#[test]
fn codex_parser_uses_last_usage_for_non_monotonic_cumulative_totals() {
    let root = temp_dir("tendi-analytics-codex-last-usage");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-session-1.jsonl");
    let events = [
        ("2026-08-01T01:00:00Z", 1_000_u64, 100_u64),
        ("2026-08-01T01:00:01Z", 2_000, 120),
        ("2026-08-01T01:00:02Z", 2_000, 120),
        ("2026-08-01T01:00:03Z", 1_500, 130),
        ("2026-08-01T01:00:04Z", 2_500, 140),
    ];
    let content = events
        .iter()
        .map(|(timestamp, cumulative, last)| {
            serde_json::json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": { "total_tokens": cumulative },
                        "last_token_usage": { "total_tokens": last },
                    },
                },
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&path, format!("{content}\n")).unwrap();

    let parsed = analyze_session(&session(&path, AgentKind::Codex), None).unwrap();

    assert_eq!(
        parsed
            .analytics
            .responses
            .iter()
            .map(|response| response.usage.total_tokens)
            .collect::<Vec<_>>(),
        vec![100, 120, 130, 140]
    );
    assert_eq!(
        parsed
            .analytics
            .responses
            .last()
            .map(|response| response.cumulative.total_tokens),
        Some(490)
    );
    assert_eq!(parsed.state.parser_version, ANALYTICS_PARSER_VERSION);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn codex_parser_tracks_models_runs_health_and_rate_windows() {
    let root = temp_dir("tendi-analytics-codex");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-session-1.jsonl");
    fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-one\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":20,\"reasoning_output_tokens\":5,\"total_tokens\":120}},\"rate_limits\":{\"primary\":{\"window_minutes\":10080,\"used_percent\":42}}}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":20,\"reasoning_output_tokens\":5,\"total_tokens\":120}}}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:04Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-two\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:05Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":180,\"cached_input_tokens\":140,\"output_tokens\":40,\"reasoning_output_tokens\":9,\"total_tokens\":220}}}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:06Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"context_compacted\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:07Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n"
            ),
        )
        .unwrap();

    let parsed = analyze_session(&session(&path, AgentKind::Codex), None).unwrap();
    assert_eq!(parsed.analytics.responses.len(), 2);
    assert_eq!(parsed.analytics.responses[0].model, "gpt-one");
    assert_eq!(parsed.analytics.responses[1].model, "gpt-two");
    assert_eq!(parsed.analytics.responses[1].usage.total_tokens, 100);
    assert_eq!(parsed.analytics.runs.len(), 1);
    assert!(parsed.analytics.runs[0].completed);
    assert_eq!(parsed.analytics.runs[0].model, "gpt-one");
    let overview = aggregate_overview(std::slice::from_ref(&parsed), 365, 30, Vec::new());
    let day = overview
        .days
        .iter()
        .find(|day| day.date == "2026-08-01")
        .expect("fixture date is included in the overview");
    let model = day
        .models
        .iter()
        .find(|model| model.model == "gpt-one")
        .expect("run model is included in the overview");
    assert_eq!(model.total_ms, 6_000);
    assert_eq!(model.completed_runs, 1);
    assert_eq!(parsed.analytics.compactions.len(), 1);
    assert_eq!(parsed.analytics.limit_samples[0].window_minutes, 10080);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn codex_orchestration_parent_task_is_not_a_model_run() {
    let root = temp_dir("tendi-analytics-codex-orchestration-parent");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-session-1.jsonl");
    fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"source\":\"cli\",\"thread_source\":\"user\",\"model_provider\":\"openai\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"parent-turn\",\"model_context_window\":258400}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:10Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"parent-turn\",\"last_agent_message\":null,\"duration_ms\":9000}}\n"
            ),
        )
        .unwrap();

    let parsed = analyze_session(&session(&path, AgentKind::Codex), None).unwrap();

    assert!(parsed.analytics.runs.is_empty());
    assert!(parsed.state.open_run.is_none());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn codex_does_not_use_session_last_model_for_an_earlier_run() {
    let root = temp_dir("tendi-analytics-codex-model-order");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-session-1.jsonl");
    fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:01Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-first\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n"
            ),
        )
        .unwrap();
    let mut codex_session = session(&path, AgentKind::Codex);
    codex_session.model = Some("gpt-last".to_string());

    let parsed = analyze_session(&codex_session, None).unwrap();

    assert_eq!(parsed.analytics.runs.len(), 1);
    assert_eq!(parsed.analytics.runs[0].model, "gpt-first");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn codex_uses_session_provenance_model_when_turn_context_is_absent() {
    let root = temp_dir("tendi-analytics-codex-provenance-model");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-session-1.jsonl");
    fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"base_instructions\":{\"provenance\":{\"model\":\"gpt-provenance\"}}}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n"
            ),
        )
        .unwrap();
    let mut codex_session = session(&path, AgentKind::Codex);
    codex_session.model = Some("gpt-last".to_string());

    let parsed = analyze_session(&codex_session, None).unwrap();

    assert_eq!(parsed.analytics.runs.len(), 1);
    assert_eq!(parsed.analytics.runs[0].model, "gpt-provenance");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn fills_unlabeled_runs_when_the_session_has_one_observed_model() {
    let root = temp_dir("tendi-analytics-single-model");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-session-1.jsonl");
    fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1,\"total_tokens\":1}}}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:03Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-only\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:04Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:05Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":2,\"total_tokens\":2}}}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:06Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n"
            ),
        )
        .unwrap();

    let parsed = analyze_session(&session(&path, AgentKind::Codex), None).unwrap();

    assert_eq!(parsed.analytics.runs.len(), 2);
    assert_eq!(
        parsed
            .analytics
            .runs
            .iter()
            .map(|run| run.model.as_str())
            .collect::<Vec<_>>(),
        vec!["gpt-only", "gpt-only"]
    );
    assert_eq!(parsed.analytics.responses.len(), 2);
    assert!(
        parsed
            .analytics
            .responses
            .iter()
            .all(|response| response.model == "gpt-only")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn claude_usage_is_deduplicated_without_deduplicating_tools() {
    let root = temp_dir("tendi-analytics-claude");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session-1.jsonl");
    fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"user\",\"message\":{\"content\":\"do it\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:01Z\",\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"model\":\"claude-one\",\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":20,\"cache_creation_input_tokens\":5,\"output_tokens\":2},\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Read\",\"input\":{\"path\":\"/tmp/skills/foo-1.2.3/SKILL.md\"}}]}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:02Z\",\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"model\":\"claude-one\",\"stop_reason\":\"end_turn\",\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":20,\"cache_creation_input_tokens\":5,\"output_tokens\":2},\"content\":[{\"type\":\"tool_use\",\"id\":\"t2\",\"name\":\"Shell\",\"input\":{\"command\":\"true\"}}]}}\n",
                "{\"type\":\"queue-operation\",\"timestamp\":\"2026-08-18T14:17:04Z\"}\n",
                "{\"timestamp\":\"2026-08-18T14:17:05Z\",\"type\":\"user\",\"message\":{\"content\":\"another turn\"}}\n"
            ),
        )
        .unwrap();

    let parsed = analyze_session(&session(&path, AgentKind::Claude), None).unwrap();
    assert_eq!(parsed.analytics.responses.len(), 1);
    assert_eq!(parsed.analytics.responses[0].usage.input_tokens, 30);
    assert_eq!(parsed.analytics.responses[0].usage.total_tokens, 37);
    assert_eq!(parsed.analytics.tools.len(), 2);
    assert_eq!(parsed.analytics.skills[0].name, "foo");
    assert_eq!(parsed.analytics.runs.len(), 1);
    assert_eq!(parsed.analytics.runs[0].end, "2026-08-01T01:00:02Z");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn claude_synthetic_responses_are_not_model_runs() {
    let root = temp_dir("tendi-analytics-claude-synthetic");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session-1.jsonl");
    fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"user\",\"message\":{\"content\":\"invalid model\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"user\",\"isSidechain\":false,\"isMeta\":true,\"message\":{\"content\":\"metadata\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:01Z\",\"type\":\"assistant\",\"message\":{\"model\":\"<synthetic>\",\"stop_reason\":\"stop_sequence\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0},\"content\":[{\"type\":\"text\",\"text\":\"model unavailable\"}]}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:02Z\",\"type\":\"user\",\"message\":{\"content\":\"valid model\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:03Z\",\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-real\",\"stop_reason\":\"end_turn\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"content\":[]}}\n"
            ),
        )
        .unwrap();

    let parsed = analyze_session(&session(&path, AgentKind::Claude), None).unwrap();

    assert_eq!(parsed.analytics.runs.len(), 1);
    assert_eq!(parsed.analytics.runs[0].model, "claude-real");
    assert!(
        !serde_json::to_string(&parsed)
            .unwrap()
            .contains("<synthetic>")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cursor_role_records_keep_turn_and_tool_timestamps() {
    let root = temp_dir("tendi-analytics-cursor");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session-1.jsonl");
    fs::write(
            &path,
            concat!(
                "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"do it\"}]}}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{\"path\":\"/tmp/skills/foo/SKILL.md\"}}]}}\n"
            ),
        )
        .unwrap();
    let mut cursor_session = session(&path, AgentKind::Cursor);
    cursor_session.started_at = Some("2026-08-01T01:00:00Z".to_string());
    let parsed = analyze_session(&cursor_session, None).unwrap();
    assert_eq!(parsed.analytics.tools.len(), 1);
    assert_eq!(parsed.analytics.tools[0].timestamp, "2026-08-01T01:00:00Z");
    assert_eq!(parsed.analytics.skills[0].name, "foo");
    assert_eq!(parsed.analytics.snapshot_runs(&parsed.state).len(), 1);
    assert!(!parsed.analytics.snapshot_runs(&parsed.state)[0].completed);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cursor_analytics_uses_embedded_timestamps_for_each_turn() {
    let root = temp_dir("tendi-analytics-cursor-embedded-timestamp");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session-1.jsonl");
    fs::write(
            &path,
            concat!(
                "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<timestamp>Thursday, Aug 27, 2026, 11:59 PM (UTC+8)</timestamp>\\n<user_query>First</user_query>\"}]}}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{\"path\":\"/tmp/first\"}}]}}\n",
                "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<timestamp>Friday, Aug 28, 2026, 12:01 AM (UTC+8)</timestamp>\\n<user_query>Second</user_query>\"}]}}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{\"path\":\"/tmp/second\"}}]}}\n"
            ),
        )
        .unwrap();

    let parsed = analyze_session(&session(&path, AgentKind::Cursor), None).unwrap();

    assert_eq!(
        parsed
            .analytics
            .tools
            .iter()
            .map(|tool| tool.timestamp.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-27T23:59:00+08:00", "2026-08-28T00:01:00+08:00"]
    );
    let runs = parsed.analytics.snapshot_runs(&parsed.state);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].start, "2026-08-27T23:59:00+08:00");
    assert_eq!(runs[0].end, "2026-08-27T23:59:00+08:00");
    assert_eq!(runs[1].start, "2026-08-28T00:01:00+08:00");
    assert_eq!(runs[1].end, "2026-08-28T00:01:00+08:00");
    let overview = aggregate_overview(std::slice::from_ref(&parsed), 365, 1, Vec::new());
    let first_day = overview
        .days
        .iter()
        .find(|day| day.date == "2026-08-27")
        .expect("first Cursor timestamp is included");
    assert_eq!(first_day.runs.completed, 1);
    assert_eq!(first_day.runs.total_ms, 0);
    assert_eq!(first_day.runs.timed_completed, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn overview_splits_timed_run_across_local_calendar_days() {
    let local = Local::now().offset().fix();
    let start = local
        .from_local_datetime(
            &(Local::now().date_naive() - Duration::days(1))
                .and_hms_opt(23, 59, 0)
                .unwrap(),
        )
        .single()
        .unwrap();
    let end = local
        .from_local_datetime(&Local::now().date_naive().and_hms_opt(0, 1, 0).unwrap())
        .single()
        .unwrap();
    let record = SessionAnalyticsRecord {
        analytics: SessionAnalytics {
            session_id: "cross-day".to_string(),
            agent: AgentKind::Codex,
            session_path: PathBuf::from("/tmp/cross-day.jsonl"),
            runs: vec![AnalyticsRun {
                model: "gpt-cross-day".to_string(),
                start: start.to_rfc3339(),
                end: end.to_rfc3339(),
                completed: true,
            }],
            ..SessionAnalytics::default()
        },
        state: AnalyticsParserState::default(),
        file_mtime: 0,
        file_size: 0,
    };

    let overview = aggregate_overview(std::slice::from_ref(&record), 2, 1, Vec::new());
    let previous = overview
        .days
        .iter()
        .find(|day| day.date == (Local::now().date_naive() - Duration::days(1)).to_string())
        .unwrap();
    let current = overview
        .days
        .iter()
        .find(|day| day.date == Local::now().date_naive().to_string())
        .unwrap();
    assert_eq!(previous.runs.started, 1);
    assert_eq!(previous.runs.completed, 1);
    assert_eq!(previous.runs.total_ms, 60_000);
    assert_eq!(current.runs.started, 0);
    assert_eq!(current.runs.completed, 0);
    assert_eq!(current.runs.total_ms, 60_000);
    assert_eq!(overview.summary.runs.timed_completed, 1);
    assert_eq!(overview.summary.runs.total_ms, 120_000);

    let projected = aggregate_overview_records(&[overview_record(&record)], 2, 1, Vec::new());
    assert_eq!(
        serde_json::to_value(&overview.days).unwrap(),
        serde_json::to_value(&projected.days).unwrap()
    );
    assert_eq!(
        overview.summary.runs.total_ms,
        projected.summary.runs.total_ms
    );
}

#[test]
fn overview_excludes_unattributed_runs_from_timing_and_model_breakdown() {
    let start = Local::now();
    let record = SessionAnalyticsRecord {
        analytics: SessionAnalytics {
            session_id: "unattributed".to_string(),
            agent: AgentKind::Codex,
            session_path: PathBuf::from("/tmp/unattributed.jsonl"),
            runs: vec![AnalyticsRun {
                model: String::new(),
                start: start.to_rfc3339(),
                end: (start + Duration::seconds(9)).to_rfc3339(),
                completed: true,
            }],
            ..SessionAnalytics::default()
        },
        state: AnalyticsParserState::default(),
        file_mtime: 0,
        file_size: 0,
    };

    let overview = aggregate_overview(std::slice::from_ref(&record), 1, 1, Vec::new());
    let day = &overview.days[0];

    assert_eq!(day.runs.completed, 1);
    assert_eq!(day.runs.total_ms, 0);
    assert_eq!(day.runs.timed_completed, 0);
    assert!(day.models.is_empty());
    assert_eq!(overview.summary.runs.total_ms, 0);

    let projected = aggregate_overview_records(&[overview_record(&record)], 1, 1, Vec::new());
    assert!(projected.days[0].models.is_empty());
    assert_eq!(projected.days[0].runs.total_ms, 0);
}

#[test]
fn append_parse_matches_full_parse() {
    let root = temp_dir("tendi-analytics-append");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-session-1.jsonl");
    fs::write(
            &path,
            "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":10,\"total_tokens\":10}}}}\n",
        )
        .unwrap();
    let session = session(&path, AgentKind::Codex);
    let first = analyze_session(&session, None).unwrap();
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
            file,
            "{{\"timestamp\":\"2026-08-01T01:00:01Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":25,\"total_tokens\":25}}}}}}}}"
        )
        .unwrap();
    let appended = analyze_session(&session, Some(&first)).unwrap();
    let full = analyze_session(&session, None).unwrap();
    assert_eq!(appended.analytics.responses, full.analytics.responses);
    assert_eq!(appended.state.previous_usage, full.state.previous_usage);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn snapshot_parser_defers_partial_trailing_line_and_resumes_once() {
    let root = temp_dir("tendi-analytics-partial-append");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-session-1.jsonl");
    let first = "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":10,\"total_tokens\":10}}}}\n";
    let second = "{\"timestamp\":\"2026-08-01T01:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":25,\"total_tokens\":25}}}}\n";
    let split = second.len() / 2;
    fs::write(&path, format!("{first}{}", &second[..split])).unwrap();
    let session = session(&path, AgentKind::Codex);

    let partial = analyze_session(&session, None).unwrap();
    assert_eq!(partial.analytics.responses.len(), 1);
    assert_eq!(partial.file_size as usize, first.len());

    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    write!(file, "{}", &second[split..]).unwrap();
    let resumed = analyze_session(&session, Some(&partial)).unwrap();

    assert_eq!(resumed.analytics.responses.len(), 2);
    assert_eq!(resumed.analytics.responses[1].usage.total_tokens, 15);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn snapshot_parser_accepts_valid_final_json_without_newline() {
    let root = temp_dir("tendi-analytics-final-line");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-session-1.jsonl");
    fs::write(
            &path,
            "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":10,\"total_tokens\":10}}}}",
        )
        .unwrap();

    let parsed = analyze_session(&session(&path, AgentKind::Codex), None).unwrap();
    assert_eq!(parsed.analytics.responses.len(), 1);
    assert_eq!(parsed.file_size, fs::metadata(&path).unwrap().len() as i64);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rewritten_larger_source_does_not_append_to_old_analytics() {
    let root = temp_dir("tendi-analytics-rewrite");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-session-1.jsonl");
    fs::write(
            &path,
            "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":10,\"total_tokens\":10}}}}\n",
        )
        .unwrap();
    let session = session(&path, AgentKind::Codex);
    let first = analyze_session(&session, None).unwrap();
    fs::write(
            &path,
            "{\"timestamp\":\"2026-08-02T01:00:00Z\",\"type\":\"event_msg\",\"padding\":\"rewritten-file-is-longer-than-the-original\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":50,\"total_tokens\":50}}}}\n",
        )
        .unwrap();

    let rewritten = analyze_session(&session, Some(&first)).unwrap();
    assert_eq!(rewritten.analytics.responses.len(), 1);
    assert_eq!(rewritten.analytics.responses[0].usage.total_tokens, 50);
    assert_eq!(
        rewritten.analytics.responses[0].timestamp,
        "2026-08-02T01:00:00Z"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn overview_keeps_model_attribution_and_mcp_servers_separate() {
    let timestamp = Local::now().to_rfc3339();
    let usage = AnalyticsTokenUsage {
        input_tokens: 8,
        output_tokens: 2,
        total_tokens: 10,
        ..AnalyticsTokenUsage::default()
    };
    let record = SessionAnalyticsRecord {
        analytics: SessionAnalytics {
            session_id: "overview-1".to_string(),
            agent: AgentKind::Codex,
            session_path: PathBuf::from("/tmp/overview-1.jsonl"),
            responses: vec![AnalyticsResponseUsage {
                index: 1,
                timestamp: timestamp.clone(),
                model: "gpt-test".to_string(),
                usage,
                cumulative: usage,
            }],
            tools: vec![
                AnalyticsToolCall {
                    timestamp: timestamp.clone(),
                    name: "search".to_string(),
                    server: "alpha".to_string(),
                },
                AnalyticsToolCall {
                    timestamp: timestamp.clone(),
                    name: "search".to_string(),
                    server: "beta".to_string(),
                },
            ],
            skills: vec![AnalyticsSkillCall {
                timestamp,
                name: "debugging".to_string(),
            }],
            ..SessionAnalytics::default()
        },
        state: AnalyticsParserState::default(),
        file_mtime: 0,
        file_size: 0,
    };

    let projected_record = overview_record(&record);
    let projected = aggregate_overview_records(&[projected_record], 1, 1, Vec::new());
    let overview = aggregate_overview(std::slice::from_ref(&record), 1, 1, Vec::new());
    assert_eq!(
        serde_json::to_value(&overview.summary).unwrap(),
        serde_json::to_value(&projected.summary).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&overview.days).unwrap(),
        serde_json::to_value(&projected.days).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&overview.tools).unwrap(),
        serde_json::to_value(&projected.tools).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&overview.skills).unwrap(),
        serde_json::to_value(&projected.skills).unwrap()
    );
    assert_eq!(overview.summary.usage.total_tokens, 10);
    assert_eq!(overview.days[0].models[0].model, "gpt-test");
    assert_eq!(overview.days[0].tools.len(), 2);
    assert_eq!(
        overview.days[0]
            .tools
            .iter()
            .map(|tool| tool.calls)
            .sum::<u64>(),
        2
    );
    assert_eq!(overview.days[0].skills[0].name, "debugging");
    assert_eq!(overview.days[0].skills[0].calls, 1);
    assert_eq!(overview.tools.len(), 2);
    assert_eq!(overview.tools[0].name, "search");
    assert_ne!(overview.tools[0].server, overview.tools[1].server);
}

#[test]
fn overview_groups_cost_and_usage_by_project() {
    let timestamp = Local::now().to_rfc3339();
    let usage = AnalyticsTokenUsage {
        input_tokens: 10,
        cached_input_tokens: 2,
        output_tokens: 4,
        total_tokens: 14,
        ..AnalyticsTokenUsage::default()
    };
    let record = SessionAnalyticsRecord {
        analytics: SessionAnalytics {
            session_id: "project-session".to_string(),
            agent: AgentKind::Codex,
            session_path: PathBuf::from("/tmp/project-session.jsonl"),
            project: Some(AnalyticsProjectIdentity {
                id: "project-1".to_string(),
                name: "Tendi".to_string(),
            }),
            responses: vec![AnalyticsResponseUsage {
                index: 1,
                timestamp,
                model: "gpt-5".to_string(),
                usage,
                cumulative: usage,
            }],
            ..SessionAnalytics::default()
        },
        state: AnalyticsParserState::default(),
        file_mtime: 0,
        file_size: 0,
    };

    let overview = aggregate_overview(std::slice::from_ref(&record), 1, 1, Vec::new());
    let projected = aggregate_overview_records(&[overview_record(&record)], 1, 1, Vec::new());
    let project = &overview.days[0].projects[0];

    assert_eq!(project.id, "project-1");
    assert_eq!(project.name, "Tendi");
    assert_eq!(project.usage, usage);
    assert!(project.cost.total_usd > 0.0);
    assert_eq!(overview.days[0].cost, project.cost);
    assert_eq!(
        serde_json::to_value(&overview.days).unwrap(),
        serde_json::to_value(&projected.days).unwrap()
    );
    assert_eq!(overview.summary.cost, projected.summary.cost);
}

#[test]
fn overview_tracks_daily_sessions_by_agent() {
    let timestamp = Local::now().to_rfc3339();
    let record = |id: &str, agent: AgentKind| SessionAnalyticsRecord {
        analytics: SessionAnalytics {
            session_id: id.to_string(),
            agent,
            session_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            responses: vec![AnalyticsResponseUsage {
                index: 1,
                timestamp: timestamp.clone(),
                model: String::new(),
                usage: AnalyticsTokenUsage::default(),
                cumulative: AnalyticsTokenUsage::default(),
            }],
            ..SessionAnalytics::default()
        },
        state: AnalyticsParserState::default(),
        file_mtime: 0,
        file_size: 0,
    };

    let overview = aggregate_overview(
        &[
            record("codex-session", AgentKind::Codex),
            record("claude-session", AgentKind::Claude),
        ],
        1,
        1,
        Vec::new(),
    );

    assert_eq!(overview.days[0].sessions, 2);
    assert_eq!(
        overview.days[0].sessions_by_agent.get(&AgentKind::Codex),
        Some(&1)
    );
    assert_eq!(
        overview.days[0].sessions_by_agent.get(&AgentKind::Claude),
        Some(&1)
    );
    let serialized = serde_json::to_value(&overview.days[0]).unwrap();
    assert_eq!(serialized["sessionsByAgent"]["codex"], 1);
    assert_eq!(serialized["sessionsByAgent"]["claude"], 1);
}

#[test]
fn overview_index_uses_local_calendar_dates_across_midnight_offsets() {
    let local = Local::now().offset().fix();
    let first_local = local
        .from_local_datetime(
            &NaiveDate::from_ymd_opt(2026, 8, 12)
                .unwrap()
                .and_hms_opt(23, 59, 0)
                .unwrap(),
        )
        .single()
        .unwrap();
    let last_local = local
        .from_local_datetime(
            &NaiveDate::from_ymd_opt(2026, 8, 13)
                .unwrap()
                .and_hms_opt(0, 1, 0)
                .unwrap(),
        )
        .single()
        .unwrap();
    let record = SessionAnalytics {
        responses: vec![
            AnalyticsResponseUsage {
                index: 1,
                timestamp: first_local.to_rfc3339(),
                model: String::new(),
                usage: AnalyticsTokenUsage::default(),
                cumulative: AnalyticsTokenUsage::default(),
            },
            AnalyticsResponseUsage {
                index: 2,
                timestamp: last_local.to_rfc3339(),
                model: String::new(),
                usage: AnalyticsTokenUsage::default(),
                cumulative: AnalyticsTokenUsage::default(),
            },
        ],
        ..SessionAnalytics::default()
    };

    let index = record.overview_index(&AnalyticsParserState::default());

    assert_eq!(index.first.as_deref(), Some("2026-08-12"));
    assert_eq!(index.last.as_deref(), Some("2026-08-13"));
}

#[test]
fn overview_hard_caps_extreme_history_to_one_year() {
    let overview = aggregate_overview(&[], 36_500, 30, Vec::new());

    assert_eq!(overview.days_requested, 365);
    assert_eq!(overview.days.len(), 365);
}
