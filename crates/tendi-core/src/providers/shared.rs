use serde_json::Value;

use super::*;
use crate::transcript::TranscriptItem;

pub(super) struct SharedProvider;

pub(super) fn parse_transcript(value: &Value, items: &mut Vec<TranscriptItem>) {
    crate::transcript::collect_shared_item(value, items);
}

pub(crate) fn shared_tool_payloads(value: &Value) -> Vec<(&Value, Evidence)> {
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
        .filter_map(|item| {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())?;
            Some((
                item,
                Evidence {
                    kind: name.to_string(),
                    text: crate::session_skills::summarize_evidence(item),
                    time: value
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
            ))
        })
        .collect()
}
impl super::AgentProvider for SharedProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Shared
    }

    fn mcp_status_after_toggle(&self, enabled: bool) -> &'static str {
        if enabled { "configured" } else { "disabled" }
    }

    fn storage_key(&self) -> &'static str {
        "shared"
    }

    fn global_skill_root(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".agents/skills"))
    }

    fn projection_directories(&self) -> &'static [&'static str] {
        &[".agents"]
    }

    fn projection_candidate_files(&self, domain: &str, ancestor: &Path) -> Vec<PathBuf> {
        match domain {
            "rules" => vec![
                ancestor.join("AGENTS.md"),
                ancestor.join(".github/copilot-instructions.md"),
            ],
            "skills" => vec![ancestor.join(".agents/skills")],
            _ => Vec::new(),
        }
    }

    fn uses_shared_skill_layout(&self) -> bool {
        true
    }

    fn matches_name(&self, normalized: &str) -> bool {
        normalized == "shared"
    }

    fn skill_roots(&self, ctx: &ProviderContext) -> Vec<SkillRoot> {
        let mut roots = Vec::new();
        if let Some(home) = &ctx.home {
            push_skill_root(
                &mut roots,
                home.join(".agents/skills"),
                "global",
                self.kind(),
            );
        }
        for dir in ctx.project_dirs() {
            push_skill_root(
                &mut roots,
                dir.join(".agents/skills"),
                "project",
                self.kind(),
            );
        }
        roots
    }

    fn parse_transcript_value(&self, value: &Value, items: &mut Vec<TranscriptItem>) {
        parse_transcript(value, items);
    }
}
