use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    sessions::SessionRecord,
    skills::{AgentKind, SkillScan},
    storage::Store,
};

const OBSERVED_CONFIDENCE: &str = "observed";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSkillLink {
    pub session_id: String,
    pub agent: AgentKind,
    pub session_path: PathBuf,
    pub session_title: Option<String>,
    pub session_project: Option<PathBuf>,
    pub session_started_at: Option<String>,
    pub session_updated_at: Option<String>,
    pub session_message_count: Option<usize>,
    pub skill_name: String,
    pub skill_path: PathBuf,
    pub skill_agent: Option<AgentKind>,
    pub skill_scope: Option<String>,
    pub evidence_kind: String,
    pub evidence_text: String,
    pub evidence_time: Option<String>,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSkillIndexStatus {
    pub total: usize,
    pub indexed: usize,
    pub pending: usize,
    pub failed: usize,
    pub running: bool,
    pub last_indexed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSkillIndexReport {
    pub status: SessionSkillIndexStatus,
    pub parsed: usize,
    pub skipped: usize,
    pub failed: usize,
}

#[derive(Debug, Clone)]
pub struct SessionFileState {
    pub file_mtime: i64,
    pub file_size: i64,
}

#[derive(Debug, Clone)]
pub struct SkillPathRef {
    pub skill_name: String,
    pub skill_path: PathBuf,
    pub skill_agent: AgentKind,
    pub skill_scope: String,
}

#[derive(Debug, Clone)]
struct SkillLookup {
    by_file: BTreeMap<String, SkillPathRef>,
    by_name: BTreeMap<String, Vec<SkillPathRef>>,
}

#[derive(Debug, Clone)]
struct Evidence {
    kind: String,
    text: String,
    time: Option<String>,
}

pub fn run_index(cwd: &Path, force: bool) -> Result<SessionSkillIndexReport> {
    let skill_scan = crate::skills::scan_skills_synced(cwd)?;
    let store = Store::open_default()?;
    let session_scan = store.list_sessions()?;

    store.save_skills(&skill_scan)?;
    if force {
        store.clear_session_skill_index()?;
    }

    let lookup = SkillLookup::new(&skill_scan);
    let mut parsed = 0;
    let mut skipped = 0;
    let mut failed = 0;

    for session in &session_scan.sessions {
        let state = match session_file_state(&session.path) {
            Ok(state) => state,
            Err(err) => {
                failed += 1;
                store.mark_session_skill_index_failed(
                    session,
                    0,
                    0,
                    &format!("failed to inspect transcript: {err:#}"),
                )?;
                continue;
            }
        };

        if !force
            && store.session_skill_index_is_current(session, state.file_mtime, state.file_size)?
        {
            skipped += 1;
            continue;
        }

        match extract_session_skill_links(session, &lookup) {
            Ok(links) => {
                store.replace_session_skill_links(session, &state, &links)?;
                parsed += 1;
            }
            Err(err) => {
                failed += 1;
                store.mark_session_skill_index_failed(
                    session,
                    state.file_mtime,
                    state.file_size,
                    &format!("{err:#}"),
                )?;
            }
        }
    }

    let status = store.session_skill_index_status(false)?;
    Ok(SessionSkillIndexReport {
        status,
        parsed,
        skipped,
        failed,
    })
}

pub fn skill_path_refs(scan: &SkillScan) -> Vec<SkillPathRef> {
    scan.skills
        .iter()
        .flat_map(|skill| {
            skill.paths.iter().map(|path| SkillPathRef {
                skill_name: skill.name.clone(),
                skill_path: path.path.clone(),
                skill_agent: path.agent,
                skill_scope: path.scope.clone(),
            })
        })
        .collect()
}

pub fn session_file_state(path: &Path) -> Result<SessionFileState> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    Ok(SessionFileState {
        file_mtime: metadata
            .modified()
            .ok()
            .and_then(system_time_millis)
            .unwrap_or(0),
        file_size: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
    })
}

fn extract_session_skill_links(
    session: &SessionRecord,
    lookup: &SkillLookup,
) -> Result<Vec<SessionSkillLink>> {
    let file = fs::File::open(&session.path)
        .with_context(|| format!("failed to read {}", session.path.display()))?;
    let mut links_by_key: BTreeMap<String, SessionSkillLink> = BTreeMap::new();

    for line in BufReader::new(file).lines() {
        let line = line.with_context(|| format!("failed to read {}", session.path.display()))?;
        if line.trim().is_empty() || !line.contains("SKILL.md") {
            continue;
        }
        let value = serde_json::from_str::<Value>(&line)
            .with_context(|| format!("failed to parse {}", session.path.display()))?;
        for (payload, evidence) in tool_payloads(session.agent, &value) {
            let mut strings = Vec::new();
            collect_strings(payload, &mut strings);
            for text in strings {
                for candidate in skill_file_candidates(text) {
                    let Some(skill) = lookup.match_candidate(candidate, session.project.as_deref())
                    else {
                        continue;
                    };
                    let key = normalize_path_key(&skill.skill_path.join("SKILL.md"));
                    links_by_key.entry(key).or_insert_with(|| SessionSkillLink {
                        session_id: session.id.clone(),
                        agent: session.agent,
                        session_path: session.path.clone(),
                        session_title: session.title.clone(),
                        session_project: session.project.clone(),
                        session_started_at: session.started_at.clone(),
                        session_updated_at: session.updated_at.clone(),
                        session_message_count: session.message_count,
                        skill_name: skill.skill_name.clone(),
                        skill_path: skill.skill_path.clone(),
                        skill_agent: Some(skill.skill_agent),
                        skill_scope: Some(skill.skill_scope.clone()),
                        evidence_kind: evidence.kind.clone(),
                        evidence_text: truncate_evidence(&evidence.text),
                        evidence_time: evidence.time.clone(),
                        confidence: OBSERVED_CONFIDENCE.to_string(),
                    });
                }
            }
        }
    }

    Ok(links_by_key.into_values().collect())
}

fn tool_payloads<'a>(agent: AgentKind, value: &'a Value) -> Vec<(&'a Value, Evidence)> {
    match agent {
        AgentKind::Codex => codex_tool_payloads(value),
        AgentKind::Claude => claude_tool_payloads(value),
        AgentKind::Cursor | AgentKind::Shared | AgentKind::Unknown => generic_tool_payloads(value),
    }
}

fn codex_tool_payloads(value: &Value) -> Vec<(&Value, Evidence)> {
    if value.get("type").and_then(Value::as_str) != Some("response_item") {
        return Vec::new();
    }
    let Some(payload) = value.get("payload") else {
        return Vec::new();
    };
    if !matches!(
        payload.get("type").and_then(Value::as_str),
        Some("function_call" | "custom_tool_call" | "local_shell_call")
    ) {
        return Vec::new();
    }
    let name = payload
        .get("name")
        .or_else(|| payload.pointer("/action/type"))
        .and_then(Value::as_str)
        .unwrap_or("tool_call");
    vec![(
        payload,
        Evidence {
            kind: name.to_string(),
            text: summarize_evidence(payload, name),
            time: value
                .get("timestamp")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
    )]
}

fn claude_tool_payloads(value: &Value) -> Vec<(&Value, Evidence)> {
    let Some(content) = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    content
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_use"))
        .map(|item| {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool_use");
            (
                item,
                Evidence {
                    kind: name.to_string(),
                    text: summarize_evidence(item, name),
                    time: value
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
            )
        })
        .collect()
}

fn generic_tool_payloads(value: &Value) -> Vec<(&Value, Evidence)> {
    let content = value
        .pointer("/message/content")
        .or_else(|| value.get("content"))
        .or_else(|| value.get("message"));
    let Some(content) = content.and_then(Value::as_array) else {
        return Vec::new();
    };
    content
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_use"))
        .map(|item| {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool_use");
            (
                item,
                Evidence {
                    kind: name.to_string(),
                    text: summarize_evidence(item, name),
                    time: value
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
            )
        })
        .collect()
}

fn collect_strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => out.push(text),
        Value::Array(items) => {
            for item in items {
                collect_strings(item, out);
            }
        }
        Value::Object(object) => {
            for value in object.values() {
                collect_strings(value, out);
            }
        }
        _ => {}
    }
}

fn skill_file_candidates(text: &str) -> Vec<&str> {
    let mut candidates = Vec::new();
    let mut search_from = 0;
    while let Some(offset) = text[search_from..].find("SKILL.md") {
        let end = search_from + offset + "SKILL.md".len();
        let mut start = search_from + offset;
        while start > 0 {
            let previous = text[..start].chars().next_back().unwrap_or_default();
            if is_path_boundary(previous) {
                break;
            }
            start -= previous.len_utf8();
        }
        let candidate = text[start..end].trim_matches(|ch| {
            matches!(
                ch,
                '"' | '\'' | '`' | '<' | '>' | '[' | ']' | '(' | ')' | '{' | '}'
            )
        });
        if candidate.contains("/skills/") || candidate.contains("\\skills\\") {
            candidates.push(candidate);
        }
        search_from = end;
    }
    candidates
}

fn is_path_boundary(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '"' | '\'' | '`' | '<' | '>' | '|' | ';' | '&' | '(' | ')' | '[' | ']' | '{' | '}'
        )
}

fn summarize_evidence(value: &Value, fallback: &str) -> String {
    for pointer in [
        "/input/command",
        "/input/file_path",
        "/arguments",
        "/action/command",
        "/command",
        "/file_path",
    ] {
        if let Some(value) = value.pointer(pointer) {
            if let Some(text) = value.as_str() {
                return text.to_string();
            }
            if let Ok(text) = serde_json::to_string(value) {
                return text;
            }
        }
    }
    serde_json::to_string(value).unwrap_or_else(|_| fallback.to_string())
}

fn truncate_evidence(text: &str) -> String {
    let mut value = text.trim().chars().take(1200).collect::<String>();
    if text.trim().chars().count() > 1200 {
        value.push_str("\n... truncated");
    }
    value
}

impl SkillLookup {
    fn new(scan: &SkillScan) -> Self {
        let mut by_file = BTreeMap::new();
        let mut by_name: BTreeMap<String, Vec<SkillPathRef>> = BTreeMap::new();
        for item in skill_path_refs(scan) {
            by_file.insert(
                normalize_path_key(&item.skill_path.join("SKILL.md")),
                item.clone(),
            );
            by_name
                .entry(item.skill_name.clone())
                .or_default()
                .push(item);
        }
        Self { by_file, by_name }
    }

    fn match_candidate(&self, candidate: &str, project: Option<&Path>) -> Option<&SkillPathRef> {
        for key in candidate_keys(candidate, project) {
            if let Some(skill) = self.by_file.get(&key) {
                return Some(skill);
            }
        }
        let name = Path::new(candidate)
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())?;
        self.by_name
            .get(name)
            .and_then(|items| (items.len() == 1).then_some(&items[0]))
    }
}

fn candidate_keys(candidate: &str, project: Option<&Path>) -> Vec<String> {
    let path = expand_home(candidate);
    let raw = PathBuf::from(&path);
    let mut values = vec![normalize_path_key(&raw)];
    if !raw.is_absolute() {
        if let Some(project) = project {
            values.push(normalize_path_key(&project.join(&raw)));
        }
    }
    values
}

fn expand_home(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).display().to_string();
        }
    }
    value.to_string()
}

fn normalize_path_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
}

fn system_time_millis(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

pub fn unix_now_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde_json::json;

    use crate::skills::{SkillPath, SkillRecord, SkillVisibility};

    use super::*;

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
    fn ignores_available_skill_instructions_without_tool_read() {
        let root = temp_dir("instructions-only");
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
                        "type": "message",
                        "role": "developer",
                        "content": [{
                            "type": "input_text",
                            "text": format!("<skills_instructions>{}</skills_instructions>", skill_dir.join("SKILL.md").display())
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

        assert!(links.is_empty());
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
                name: name.to_string(),
                description: None,
                tags: Vec::new(),
                dependencies: Vec::new(),
                dependents: Vec::new(),
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
                    codex_allow_implicit_invocation: None,
                    codex_skill_enabled: None,
                    cursor_disable_model_invocation: None,
                    plugin_id: None,
                    plugin_enabled: None,
                }],
                source_summary: "local".to_string(),
                install_targets: vec!["global".to_string()],
                update_status: "local".to_string(),
                is_system: false,
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
}
