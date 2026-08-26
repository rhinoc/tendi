use serde_json::Value;

use crate::transcript::TranscriptItem;

use super::*;

pub(super) struct UnknownProvider;

impl super::AgentProvider for UnknownProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Unknown
    }

    fn storage_key(&self) -> &'static str {
        "unknown"
    }

    fn mcp_status_after_toggle(&self, enabled: bool) -> &'static str {
        if enabled { "configured" } else { "disabled" }
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("Unknown")
    }

    fn parse_transcript_value(&self, _value: &Value, _items: &mut Vec<TranscriptItem>) {}
}
