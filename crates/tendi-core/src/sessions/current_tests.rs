use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use super::*;

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    ctx: ProviderContext,
    env: BTreeMap<String, String>,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tendi-current-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).unwrap();
        let mut ctx = ProviderContext::new(&root);
        ctx.home = Some(root);
        Self {
            ctx,
            env: BTreeMap::new(),
        }
    }

    fn write(&self, relative: &str, body: &str) -> PathBuf {
        let path = self.ctx.home.as_ref().unwrap().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        path
    }

    fn set(&mut self, key: &str, value: &str) {
        self.env.insert(key.to_string(), value.to_string());
    }

    fn current(&self, agent: Option<AgentKind>) -> Result<CurrentSession> {
        resolve_current_session(&self.ctx, agent, &self.env)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(self.ctx.home.as_ref().unwrap()).unwrap();
    }
}

#[test]
fn codex_current_uses_thread_identity_instead_of_root_session_or_latest_file() {
    let mut fixture = Fixture::new();
    let id = "01a0f0e1-131a-73a3-b443-d1f3745d53e0";
    fixture.set("CODEX_THREAD_ID", id);
    fixture.set("CODEX_SESSION_ID", "parent-session");
    let path = fixture.write(
        &format!(".codex/sessions/2026/09/30/rollout-2026-09-30T13-54-37-{id}.jsonl"),
        "",
    );
    fixture.write(".codex/sessions/newer-unrelated.jsonl", "");
    let current = fixture.current(None).unwrap();
    assert_eq!(current.id, id);
    assert_eq!(current.agent, AgentKind::Codex);
    assert_eq!(current.path, Some(path));
}

#[test]
fn codex_current_respects_config_home_and_archived_sessions() {
    let mut fixture = Fixture::new();
    let home = fixture.ctx.home.as_ref().unwrap().join("codex-profile");
    fixture.set("CODEX_HOME", home.to_str().unwrap());
    fixture.set("CODEX_THREAD_ID", "archived-id");
    let path = fixture.write("codex-profile/archived_sessions/archived-id.jsonl", "");
    fixture.write(".codex/sessions/archived-id.jsonl", "");
    assert_eq!(fixture.current(None).unwrap().path, Some(path));
}

#[test]
fn claude_current_reads_native_identity_and_provider_transcript() {
    let mut fixture = Fixture::new();
    fixture.set("CLAUDE_CODE_SESSION_ID", "claude-id");
    fixture.set(
        "CLAUDE_SESSION_ID",
        "skill-placeholder-is-not-runtime-identity",
    );
    let path = fixture.write(
        ".claude/projects/project/claude-id.jsonl",
        "{\"type\":\"user\",\"message\":{\"content\":\"current Claude message\"}}\n",
    );
    let current = fixture.current(None).unwrap();
    assert_eq!(current.id, "claude-id");
    assert_eq!(current.agent, AgentKind::Claude);
    assert_eq!(current.path, Some(path.clone()));
    let transcript = crate::transcript::parse_transcript(&path, current.agent).unwrap();
    assert!(
        transcript
            .items
            .iter()
            .any(|item| item.body == "current Claude message")
    );
}

#[test]
fn claude_current_respects_config_home_and_nested_agent_transcripts() {
    let mut fixture = Fixture::new();
    let home = fixture.ctx.home.as_ref().unwrap().join("claude-profile");
    fixture.set("CLAUDE_CONFIG_DIR", home.to_str().unwrap());
    fixture.set("CLAUDE_CODE_SESSION_ID", "agent-child");
    let path = fixture.write(
        "claude-profile/projects/custom-project/parent/subagents/agent-child.jsonl",
        "",
    );
    fixture.write(".claude/projects/project/agent-child.jsonl", "");
    assert_eq!(fixture.current(None).unwrap().path, Some(path));
}

#[test]
fn cursor_current_uses_conversation_id_and_nested_jsonl_transcript() {
    let mut fixture = Fixture::new();
    fixture.set("CURSOR_CONVERSATION_ID", "cursor-id");
    fixture.set("CURSOR_REQUEST_ID", "generation-id");
    fixture.set("CURSOR_TRACE_ID", "trace-id");
    let path = fixture.write(
        ".cursor/projects/project/agent-transcripts/cursor-id/cursor-id.jsonl",
        "{\"role\":\"user\",\"message\":{\"content\":\"current Cursor message\"}}\n",
    );
    let current = fixture.current(None).unwrap();
    assert_eq!(current.id, "cursor-id");
    assert_eq!(current.agent, AgentKind::Cursor);
    assert_eq!(current.path, Some(path.clone()));
    let transcript = crate::transcript::parse_transcript(&path, current.agent).unwrap();
    assert!(
        transcript
            .items
            .iter()
            .any(|item| item.body == "current Cursor message")
    );
}

#[test]
fn cursor_current_respects_runtime_transcript_directory() {
    let mut fixture = Fixture::new();
    fixture.set("CURSOR_CONVERSATION_ID", "cursor-id");
    let root = fixture
        .ctx
        .home
        .as_ref()
        .unwrap()
        .join("remote-transcripts");
    fixture.set("AGENT_TRANSCRIPTS", root.to_str().unwrap());
    let path = fixture.write("remote-transcripts/cursor-id.jsonl", "");
    fixture.write(
        ".cursor/projects/project/agent-transcripts/cursor-id.jsonl",
        "",
    );
    assert_eq!(fixture.current(None).unwrap().path, Some(path));
}

#[test]
fn cursor_current_respects_explicit_runtime_transcript_path() {
    let mut fixture = Fixture::new();
    fixture.set("CURSOR_CONVERSATION_ID", "cursor-id");
    let path = fixture.write("runtime-transcript.jsonl", "");
    fixture.set("CURSOR_TRANSCRIPT_PATH", path.to_str().unwrap());
    fixture.write(
        ".cursor/projects/project/agent-transcripts/cursor-id.jsonl",
        "",
    );
    assert_eq!(fixture.current(None).unwrap().path, Some(path.clone()));
    fs::remove_file(path).unwrap();
    assert!(fixture.current(None).unwrap().path.is_none());
}

#[test]
fn cursor_current_reports_unsupported_transcript_format() {
    let mut fixture = Fixture::new();
    fixture.set("CURSOR_CONVERSATION_ID", "cursor-id");
    fixture.set("CURSOR_TRANSCRIPT_PATH", "/transcripts/cursor-id.txt");
    assert!(
        fixture
            .current(None)
            .unwrap_err()
            .to_string()
            .contains("must be JSONL")
    );
}

#[test]
fn current_identity_remains_available_before_transcript_is_written() {
    let mut fixture = Fixture::new();
    fixture.set("CLAUDE_CODE_SESSION_ID", "not-written-yet");
    let current = fixture.current(None).unwrap();
    assert_eq!(current.id, "not-written-yet");
    assert!(current.path.is_none());
    assert_eq!(
        serde_json::to_value(current).unwrap()["path"],
        serde_json::Value::Null
    );
}

#[test]
fn current_rejects_missing_context_even_when_recent_transcripts_exist() {
    let mut fixture = Fixture::new();
    fixture.write(".claude/projects/project/latest.jsonl", "");
    fixture.set("CODEX_SESSION_ID", "root-only");
    fixture.set("CLAUDE_SESSION_ID", "placeholder-only");
    fixture.set("CURSOR_TRACE_ID", "trace-only");
    fixture.set("CODEX_THREAD_ID", " ");
    assert!(
        fixture
            .current(None)
            .unwrap_err()
            .to_string()
            .contains("missing current session context")
    );
}

#[test]
fn current_requires_provider_selection_when_identities_are_inherited() {
    let mut fixture = Fixture::new();
    fixture.set("CODEX_THREAD_ID", "parent-codex");
    fixture.set("CLAUDE_CODE_SESSION_ID", "child-claude");
    fixture.set("CURSOR_CONVERSATION_ID", "child-cursor");
    assert!(
        fixture
            .current(None)
            .unwrap_err()
            .to_string()
            .contains("specify --agent")
    );
    for (agent, id) in [
        (AgentKind::Codex, "parent-codex"),
        (AgentKind::Claude, "child-claude"),
        (AgentKind::Cursor, "child-cursor"),
    ] {
        let current = fixture.current(Some(agent)).unwrap();
        assert_eq!(current.agent, agent);
        assert_eq!(current.id, id);
    }
}

#[test]
fn selected_provider_cannot_use_another_providers_identity() {
    let mut fixture = Fixture::new();
    fixture.set("CODEX_THREAD_ID", "codex-id");
    assert!(
        fixture
            .current(Some(AgentKind::Claude))
            .unwrap_err()
            .to_string()
            .contains("missing current session context")
    );
    assert!(
        fixture
            .current(Some(AgentKind::Shared))
            .unwrap_err()
            .to_string()
            .contains("not supported")
    );
}

#[test]
fn current_rejects_multiple_matching_transcripts() {
    let mut fixture = Fixture::new();
    fixture.set("CODEX_THREAD_ID", "same-id");
    fixture.write(".codex/sessions/same-id.jsonl", "");
    fixture.write(".codex/archived_sessions/same-id.jsonl", "");
    assert!(
        fixture
            .current(None)
            .unwrap_err()
            .to_string()
            .contains("multiple transcripts match")
    );
}

#[test]
fn current_does_not_consider_metadata_or_another_providers_transcript() {
    let mut fixture = Fixture::new();
    fixture.set("CURSOR_CONVERSATION_ID", "cursor-id");
    fixture.write(".cursor/chats/project/cursor-id/meta.json", "{}");
    fixture.write(".cursor/chats/project/cursor-id/store.db", "");
    fixture.write(".claude/projects/project/cursor-id.jsonl", "");
    fixture.write(
        ".cursor/projects/project/agent-transcripts/not-cursor-id.jsonl",
        "",
    );
    assert!(fixture.current(None).unwrap().path.is_none());
}
