use super::*;
use std::io::Write;

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "tendi-search-cursor-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

fn message(agent: AgentKind, text: &str) -> String {
    let value = match agent {
        AgentKind::Codex => {
            serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}})
        }
        AgentKind::Claude => {
            serde_json::json!({"type":"user","message":{"role":"user","content":text}})
        }
        AgentKind::Cursor => {
            serde_json::json!({"role":"user","message":{"content":[{"type":"text","text":text}]}})
        }
        _ => unreachable!(),
    };
    value.to_string() + "\n"
}

#[test]
fn every_provider_appends_only_new_bytes_after_serialized_checkpoint() {
    let root = fixture("provider-append");
    for (index, agent) in [AgentKind::Codex, AgentKind::Claude, AgentKind::Cursor]
        .into_iter()
        .enumerate()
    {
        let path = root.join(format!("{index}.jsonl"));
        let initial = message(agent, "historical needle").repeat(10_000);
        fs::write(&path, &initial).unwrap();
        let first = read_search_delta(&path, agent, None, &|| false)
            .unwrap()
            .unwrap();
        assert_eq!(first.items.len(), 10_000);
        let checkpoint: SearchCheckpoint =
            serde_json::from_str(&serde_json::to_string(&first.checkpoint).unwrap()).unwrap();
        let appended = message(agent, "new needle");
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(appended.as_bytes())
            .unwrap();
        let next = read_search_delta(&path, agent, Some(&checkpoint), &|| false)
            .unwrap()
            .unwrap();
        assert_eq!(next.start_record_order, 10_001);
        assert_eq!(next.items.len(), 1);
        assert_eq!(next.items[0].body, "new needle");
        assert_eq!(next.bytes_read, appended.len() as u64);
        assert!(next.validation_bytes_read <= 4 * ANCHOR_BYTES);
        eprintln!(
            "provider={agent:?} initial_bytes={} append_read={} validation_read={}",
            initial.len(),
            next.bytes_read,
            next.validation_bytes_read
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn provisional_eof_and_incomplete_tail_replay_without_duplicate_orders() {
    let root = fixture("tail");
    let path = root.join("session.jsonl");
    let line = message(AgentKind::Codex, "tail needle");
    fs::write(&path, line.trim_end()).unwrap();
    let first = read_search_delta(&path, AgentKind::Codex, None, &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.checkpoint.committed_offset, 0);
    assert_eq!(first.checkpoint.next_record_order, 1);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"\n{\"type\":")
        .unwrap();
    let next = read_search_delta(&path, AgentKind::Codex, Some(&first.checkpoint), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(next.start_record_order, 1);
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.checkpoint.committed_offset, line.len() as u64);
    assert_eq!(next.checkpoint.next_record_order, 2);
    assert!(next.warnings.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn split_utf8_tail_waits_for_the_rest_of_the_record() {
    let root = fixture("utf8-tail");
    let path = root.join("session.jsonl");
    let line = message(AgentKind::Codex, "中文 needle");
    let split = line.find('中').unwrap() + 1;
    fs::write(&path, &line.as_bytes()[..split]).unwrap();
    let first = read_search_delta(&path, AgentKind::Codex, None, &|| false)
        .unwrap()
        .unwrap();
    assert!(first.items.is_empty());
    assert_eq!(first.checkpoint.committed_offset, 0);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&line.as_bytes()[split..])
        .unwrap();
    let next = read_search_delta(&path, AgentKind::Codex, Some(&first.checkpoint), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.items[0].body, "中文 needle");
    assert_eq!(next.checkpoint.committed_offset, line.len() as u64);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn append_during_read_stops_at_the_captured_boundary_and_resumes() {
    let root = fixture("live-append");
    let path = root.join("session.jsonl");
    fs::write(&path, message(AgentKind::Codex, "initial")).unwrap();
    let appended = std::cell::Cell::new(false);
    let first = read_search_delta(&path, AgentKind::Codex, None, &|| {
        if !appended.replace(true) {
            fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(message(AgentKind::Codex, "later").as_bytes())
                .unwrap();
        }
        false
    })
    .unwrap()
    .unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].body, "initial");
    let second = read_search_delta(&path, AgentKind::Codex, Some(&first.checkpoint), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(second.start_record_order, 2);
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].body, "later");
    assert!(
        read_search_delta(&path, AgentKind::Codex, Some(&second.checkpoint), &|| true)
            .unwrap()
            .is_none()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn codex_inherited_history_filter_survives_checkpoint_restart() {
    let root = fixture("inherited");
    let path = root.join("session.jsonl");
    let ordinal_message = |ordinal: u64, text: &str| {
        let mut value: Value = serde_json::from_str(&message(AgentKind::Codex, text)).unwrap();
        value["ordinal"] = ordinal.into();
        value.to_string() + "\n"
    };
    let header = "{\"ordinal\":0,\"type\":\"session_meta\",\"payload\":{\"thread_source\":\"subagent\",\"subagent_history_start_ordinal\":3}}\n";
    // Resolve headers before messages even if a historical record precedes it.
    fs::write(
        &path,
        ordinal_message(1, "parent history") + header + &ordinal_message(3, "child start"),
    )
    .unwrap();
    let first = read_search_delta(&path, AgentKind::Codex, None, &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].body, "child start");
    let checkpoint: SearchCheckpoint =
        serde_json::from_str(&serde_json::to_string(&first.checkpoint).unwrap()).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(
            (ordinal_message(2, "inherited late") + &ordinal_message(4, "child later")).as_bytes(),
        )
        .unwrap();
    let next = read_search_delta(&path, AgentKind::Codex, Some(&checkpoint), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.items[0].body, "child later");
    assert_eq!(next.start_record_order, 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parser_change_truncate_replace_and_observed_rewrite_reset() {
    let root = fixture("reset");
    let path = root.join("session.jsonl");
    let initial = message(AgentKind::Codex, "original").repeat(100);
    fs::write(&path, &initial).unwrap();
    let first = read_search_delta(&path, AgentKind::Codex, None, &|| false)
        .unwrap()
        .unwrap();
    let mut old_parser = first.checkpoint.clone();
    old_parser.parser_version = "old-parser".into();
    let reset = read_search_delta(&path, AgentKind::Codex, Some(&old_parser), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(reset.start_record_order, 1);
    assert_eq!(reset.bytes_read, initial.len() as u64);
    // Same inode, growth and a changed historical prefix must reset.
    fs::write(&path, message(AgentKind::Codex, "rewritten").repeat(101)).unwrap();
    let rewrite = read_search_delta(&path, AgentKind::Codex, Some(&first.checkpoint), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(rewrite.start_record_order, 1);
    assert_eq!(rewrite.items[0].body, "rewritten");
    fs::write(&path, message(AgentKind::Codex, "truncated")).unwrap();
    let truncated = read_search_delta(&path, AgentKind::Codex, Some(&rewrite.checkpoint), &|| {
        false
    })
    .unwrap()
    .unwrap();
    assert_eq!(truncated.start_record_order, 1);
    assert_eq!(truncated.items.len(), 1);
    let replacement = root.join("replacement.jsonl");
    fs::write(
        &replacement,
        message(AgentKind::Codex, "replaced").repeat(101),
    )
    .unwrap();
    fs::rename(&replacement, &path).unwrap();
    let replaced = read_search_delta(
        &path,
        AgentKind::Codex,
        Some(&truncated.checkpoint),
        &|| false,
    )
    .unwrap()
    .unwrap();
    assert_eq!(replaced.start_record_order, 1);
    assert_ne!(replaced.checkpoint.identity, truncated.checkpoint.identity);
    assert_eq!(replaced.items[0].body, "replaced");
    fs::remove_dir_all(root).unwrap();
}
