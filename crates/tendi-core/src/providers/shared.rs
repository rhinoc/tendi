use serde_json::Value;

use super::*;
use crate::transcript::TranscriptItem;

pub(super) struct SharedProvider;

pub(super) fn parse_transcript(value: &Value, items: &mut Vec<TranscriptItem>) {
    crate::transcript::collect_generic_item(value, items);
}

pub(crate) fn generic_tool_payloads(value: &Value) -> Vec<(&Value, Evidence)> {
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
                    text: crate::session_skills::summarize_evidence(item, name),
                    time: value
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
            )
        })
        .collect()
}
impl super::AgentProvider for SharedProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Shared
    }

    fn storage_key(&self) -> &'static str {
        "shared"
    }

    fn global_skill_root(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".agents/skills"))
    }

    fn materialized_skill_is_shared_or_codex(&self) -> bool {
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
