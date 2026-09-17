use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;
use serde_json::json;

use super::*;

#[test]
fn extracts_skill_evidence_from_cursor_store() {
    let root = std::env::temp_dir().join(format!(
        "tendi-cursor-skill-evidence-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("create temp directory");
    let skill_path = root.join(".ctx/plans/e2e/SKILL.md");
    fs::create_dir_all(skill_path.parent().expect("skill parent")).expect("create skill");
    let store_path = root.join("store.db");
    let connection = Connection::open(&store_path).expect("open store");
    connection
        .execute("CREATE TABLE blobs (data BLOB)", [])
        .expect("create blobs table");
    for value in [
        json!({
            "role": "assistant",
            "content": [{
                "type": "tool-call",
                "toolCallId": "read-1",
                "toolName": "Read",
                "args": { "path": skill_path }
            }]
        }),
        json!({
            "role": "user",
            "content": [{
                "type": "text",
                "text": format!("<agent_skill fullPath=\"{}\">", skill_path.display())
            }]
        }),
    ] {
        let data = value.to_string();
        connection
            .execute("INSERT INTO blobs (data) VALUES (?1)", [data.as_bytes()])
            .expect("insert blob");
    }
    drop(connection);

    let evidence = cursor_store_skill_evidence_for_path(&store_path);
    assert!(evidence.iter().any(|candidate| {
        candidate.path.as_deref() == Some(skill_path.to_str().expect("skill path"))
            && candidate.evidence.kind == "Read"
            && candidate.confidence == "observed"
    }));
    assert!(evidence.iter().any(|candidate| {
        candidate.path.as_deref() == Some(skill_path.to_str().expect("skill path"))
            && candidate.evidence.kind == "agent_skill"
            && candidate.confidence == "explicit"
    }));

    fs::remove_dir_all(root).expect("remove temp directory");
}
