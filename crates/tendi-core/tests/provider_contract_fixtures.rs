use std::{fs, path::PathBuf, time::{SystemTime, UNIX_EPOCH}};

use tendi_core::{AgentKind, TranscriptItem};
use tendi_core::transcript::parse_transcript;

fn fixture_path(name: &str, content: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "tendi-provider-fixture-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
    ));
    fs::create_dir_all(&root).unwrap();
    let path = root.join(name);
    fs::write(&path, content).unwrap();
    path
}

fn tool(items: &[TranscriptItem]) -> &TranscriptItem {
    items.iter().find(|item| item.kind == "tool").unwrap()
}

#[test]
fn provider_fixtures_keep_parser_ownership_and_tool_identity() {
    let cases = [
        ("codex.jsonl", AgentKind::Codex, include_str!("../testdata/transcripts/codex.jsonl")),
        ("claude.jsonl", AgentKind::Claude, include_str!("../testdata/transcripts/claude.jsonl")),
        ("cursor.jsonl", AgentKind::Cursor, include_str!("../testdata/transcripts/cursor.jsonl")),
    ];

    for (name, agent, content) in cases {
        let path = fixture_path(name, content);
        let scan = parse_transcript(&path, agent).unwrap();
        let second_scan = parse_transcript(&path, agent).unwrap();
        assert!(scan.warnings.is_empty(), "{agent:?}: {:?}", scan.warnings);
        assert_eq!(
            serde_json::to_value(&scan.items).unwrap(),
            serde_json::to_value(&second_scan.items).unwrap(),
            "{agent:?} normalization must be deterministic",
        );
        assert!(scan.items.iter().any(|item| item.kind == "user"));
        let item = tool(&scan.items);
        assert_eq!(item.command.as_deref(), Some("printf fixture"));
        if agent == AgentKind::Codex {
            assert_eq!(item.call_id.as_deref(), Some("codex-call-1"));
            assert_eq!(item.result.as_deref(), Some("fixture"));
        }
        if agent == AgentKind::Claude {
            assert_eq!(item.call_id.as_deref(), Some("claude-tool-1"));
            assert_eq!(item.result.as_deref(), Some("fixture"));
        }
        if agent == AgentKind::Cursor {
            assert_eq!(item.time.as_deref(), Some("18:11"));
        }
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
