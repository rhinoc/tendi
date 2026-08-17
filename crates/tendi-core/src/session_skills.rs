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
const EXPLICIT_CONFIDENCE: &str = "explicit";
const SESSION_SKILL_INDEX_VERSION: &str = "2";

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
pub(crate) struct Evidence {
    pub(crate) kind: String,
    pub(crate) text: String,
    pub(crate) time: Option<String>,
}

pub fn run_index(cwd: &Path, force: bool) -> Result<SessionSkillIndexReport> {
    let skill_scan = crate::skills::scan_skills_synced(cwd)?;
    let store = Store::open_default()?;
    let session_scan = store.list_sessions()?;

    store.save_skills(&skill_scan)?;
    store.ensure_session_skill_index_version(SESSION_SKILL_INDEX_VERSION)?;
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
        if line.trim().is_empty()
            || (!line.contains("SKILL.md")
                && !line.contains("<skill>")
                && !line.contains('$')
                && !is_skill_tool_line(&line))
        {
            continue;
        }
        let value = serde_json::from_str::<Value>(&line)
            .with_context(|| format!("failed to parse {}", session.path.display()))?;

        for candidate in explicit_skill_candidates(&value, session.agent) {
            let skill = candidate
                .path
                .as_deref()
                .and_then(|path| lookup.match_candidate(path, session.project.as_deref()))
                .or_else(|| {
                    candidate
                        .name
                        .as_deref()
                        .and_then(|name| lookup.match_name(name, session.agent))
                });
            let Some(skill) = skill else {
                continue;
            };
            insert_skill_link(
                &mut links_by_key,
                session,
                skill,
                &candidate.evidence,
                EXPLICIT_CONFIDENCE,
            );
        }

        for (payload, evidence) in tool_payloads(session.agent, &value) {
            let mut strings = Vec::new();
            collect_strings(payload, &mut strings);
            for text in strings {
                for candidate in skill_file_candidates(text) {
                    let Some(skill) = lookup.match_candidate(candidate, session.project.as_deref())
                    else {
                        continue;
                    };
                    insert_skill_link(
                        &mut links_by_key,
                        session,
                        skill,
                        &evidence,
                        OBSERVED_CONFIDENCE,
                    );
                }
            }
        }
    }

    Ok(links_by_key.into_values().collect())
}

#[derive(Debug, Clone)]
struct ExplicitSkillCandidate {
    name: Option<String>,
    path: Option<String>,
    evidence: Evidence,
}

fn insert_skill_link(
    links_by_key: &mut BTreeMap<String, SessionSkillLink>,
    session: &SessionRecord,
    skill: &SkillPathRef,
    evidence: &Evidence,
    confidence: &str,
) {
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
        confidence: confidence.to_string(),
    });
}

fn explicit_skill_candidates(value: &Value, agent: AgentKind) -> Vec<ExplicitSkillCandidate> {
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut candidates = Vec::new();

    if is_user_message(value) {
        for text in user_message_texts(value) {
            if text.contains("<skill>") {
                let name = xml_tag_value(&text, "name");
                let path = xml_tag_value(&text, "path");
                if name.is_some() || path.is_some() {
                    candidates.push(ExplicitSkillCandidate {
                        name,
                        path,
                        evidence: Evidence {
                            kind: "explicit_skill".to_string(),
                            text: text.clone(),
                            time: timestamp.clone(),
                        },
                    });
                }
            }
            for raw_name in dollar_skill_names(&text) {
                if text.contains("<skill>") {
                    continue;
                }
                candidates.push(ExplicitSkillCandidate {
                    name: Some(raw_name.clone()),
                    path: None,
                    evidence: Evidence {
                        kind: "explicit_skill".to_string(),
                        text: format!("explicit skill reference: ${raw_name}"),
                        time: timestamp.clone(),
                    },
                });
            }
        }
    }

    for (payload, evidence) in tool_payloads(agent, value) {
        if !evidence.kind.eq_ignore_ascii_case("skill") {
            continue;
        }
        if let Some(name) = explicit_skill_tool_name(payload) {
            candidates.push(ExplicitSkillCandidate {
                name: Some(name),
                path: None,
                evidence: Evidence {
                    kind: "explicit_skill".to_string(),
                    text: evidence.text,
                    time: evidence.time,
                },
            });
        }
    }

    candidates
}

fn is_skill_tool_line(line: &str) -> bool {
    line.contains(r#""name":"skill""#)
        || line.contains(r#""name":"Skill""#)
        || line.contains(r#""name": "skill""#)
        || line.contains(r#""name": "Skill""#)
}

fn is_user_message(value: &Value) -> bool {
    if value.get("type").and_then(Value::as_str) == Some("response_item") {
        return value.pointer("/payload/type").and_then(Value::as_str) == Some("message")
            && value.pointer("/payload/role").and_then(Value::as_str) == Some("user");
    }
    value.get("type").and_then(Value::as_str) == Some("user")
        || value.get("role").and_then(Value::as_str) == Some("user")
}

fn user_message_texts(value: &Value) -> Vec<String> {
    let message = if value.get("type").and_then(Value::as_str) == Some("response_item") {
        value.get("payload").unwrap_or(&Value::Null)
    } else {
        value
    };
    let content = message
        .get("content")
        .or_else(|| message.pointer("/message/content"))
        .or_else(|| message.get("message"))
        .unwrap_or(&Value::Null);
    let mut strings = Vec::new();
    collect_strings(content, &mut strings);
    strings.into_iter().map(str::to_string).collect()
}

fn xml_tag_value(text: &str, tag: &str) -> Option<String> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = text.find(&start_tag)? + start_tag.len();
    let end = text[start..].find(&end_tag)? + start;
    let value = text[start..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn dollar_skill_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut search_from = 0;
    while let Some(offset) = text[search_from..].find('$') {
        let start = search_from + offset;
        let raw_start = start + 1;
        let raw_end = text[raw_start..]
            .char_indices()
            .find_map(|(offset, ch)| (!is_skill_name_char(ch)).then_some(raw_start + offset))
            .unwrap_or(text.len());
        if raw_end > raw_start && text[raw_end..].trim().is_empty() {
            let name = text[raw_start..raw_end].to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        search_from = raw_end.max(start + 1);
    }
    names
}

fn is_skill_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':')
}

fn explicit_skill_tool_name(payload: &Value) -> Option<String> {
    let direct = payload
        .pointer("/input/skill")
        .or_else(|| payload.get("skill"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let argument = payload
        .get("arguments")
        .and_then(Value::as_str)
        .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
        .and_then(|arguments| {
            arguments
                .get("skill")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    direct
        .or(argument)
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

fn tool_payloads<'a>(agent: AgentKind, value: &'a Value) -> Vec<(&'a Value, Evidence)> {
    crate::providers::agent_provider(agent).extract_skill_tool_payloads(value)
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

pub(crate) fn summarize_evidence(value: &Value, fallback: &str) -> String {
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

    fn match_name(&self, name: &str, agent: AgentKind) -> Option<&SkillPathRef> {
        let names = [name, name.rsplit_once(':').map_or(name, |(_, name)| name)];
        for name in names {
            let Some(items) = self.by_name.get(name) else {
                continue;
            };
            let agent_matches = items
                .iter()
                .filter(|item| item.skill_agent == agent)
                .collect::<Vec<_>>();
            if agent_matches.len() == 1 {
                return agent_matches.into_iter().next();
            }
            let shared_matches = items
                .iter()
                .filter(|item| item.skill_agent == AgentKind::Shared)
                .collect::<Vec<_>>();
            if shared_matches.len() == 1 {
                return shared_matches.into_iter().next();
            }
            if items.len() == 1 {
                return items.first();
            }
        }
        None
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
    fn extracts_explicit_skill_marker() {
        let root = temp_dir("explicit-marker");
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
        assert_eq!(links[0].confidence, "explicit");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_dollar_skill_reference() {
        let root = temp_dir("explicit-dollar");
        let skill_dir = root.join(".codex/skills/foo");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: foo\n---\n").unwrap();
        let transcript = root.join("session.jsonl");
        fs::write(
            &transcript,
            format!(
                "{}\n",
                json!({
                    "type": "user",
                    "timestamp": "2026-06-24T10:00:00Z",
                    "message": { "content": "Please use $foo" }
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
        assert!(dollar_skill_names("Please use $foo now").is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_explicit_skill_tool_argument() {
        let root = temp_dir("explicit-tool");
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
                        "name": "Skill",
                        "arguments": "{\"skill\":\"foo\"}"
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
}
