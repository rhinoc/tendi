use super::*;

#[test]
fn excerpt_pages_keep_message_time_context_and_resume_index() {
    let root = std::env::temp_dir().join(format!(
        "tendi-excerpt-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session.jsonl");
    let mut lines = String::new();
    for i in 0..430 {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        let body = if i == 400 || i == 420 {
            format!("引入叭哥说登录 {i}")
        } else {
            format!("context {i}")
        };
        lines.push_str(&(serde_json::json!({"type":"response_item","timestamp":"2026-06-23T15:48:04Z","payload":{"type":"message","role":role,"content":[{"type":"input_text","text":body}]}}).to_string()+"\n"));
    }
    fs::write(&path, lines).unwrap();
    let options = TranscriptExcerptOptions {
        query: Some("叭哥说 登录".into()),
        roles: vec!["user".into()],
        limit: 1,
        around: 2,
        ..Default::default()
    };
    let excerpt = read_transcript_excerpt(&path, AgentKind::Codex, &options).unwrap();
    assert_eq!(excerpt.items.len(), 5);
    let center = excerpt.items.iter().find(|item| item.matched).unwrap();
    assert!(center.item.body.ends_with("400"));
    assert_eq!(center.item.time.as_deref(), Some("2026-06-23T15:48:04Z"));
    let next = read_transcript_excerpt(
        &path,
        AgentKind::Codex,
        &TranscriptExcerptOptions {
            offset: excerpt.next_offset.unwrap(),
            ..options
        },
    )
    .unwrap();
    assert!(
        next.items
            .iter()
            .find(|item| item.matched)
            .unwrap()
            .item
            .body
            .ends_with("420")
    );
    assert!(next.items[0].index > center.index);
    let empty = read_transcript_excerpt(
        &path,
        AgentKind::Codex,
        &TranscriptExcerptOptions {
            query: Some("absent".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(empty.items.is_empty());
    assert!(empty.next_offset.is_none());
    fs::remove_dir_all(root).unwrap();
}
