//! Shared identity, snapshot, and operation contracts.
//!
//! These types deliberately carry no provider-specific parsing rules. They
//! make the boundaries between source data, projections, and asynchronous
//! operations explicit so callers cannot silently replace an identity with a
//! display name or a file path.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::skills::AgentKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError {
    field: &'static str,
}

impl ContractError {
    fn empty(field: &'static str) -> Self {
        Self { field }
    }
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} must not be empty", self.field)
    }
}

impl std::error::Error for ContractError {}

fn required(value: impl Into<String>, field: &'static str) -> Result<String, ContractError> {
    let value = value.into();
    (!value.trim().is_empty())
        .then_some(value)
        .ok_or_else(|| ContractError::empty(field))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScopeKey(String);

impl ScopeKey {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
        Ok(Self(required(value, "scope_key")?))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ScopeKey {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ScopeKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceVersion(String);

impl SourceVersion {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
        Ok(Self(required(value, "source_version")?))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for SourceVersion {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for SourceVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Revision(u64);

impl Revision {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn is_newer_than(self, other: Self) -> bool {
        self.0 > other.0
    }
}

impl From<u64> for Revision {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

impl From<Revision> for u64 {
    fn from(value: Revision) -> Self {
        value.value()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OperationId(String);

impl OperationId {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
        Ok(Self(required(value, "operation_id")?))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for OperationId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InstallationId(String);

impl InstallationId {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
        Ok(Self(required(value, "installation_id")?))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for InstallationId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for InstallationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    pub locator: String,
    pub source_kind: String,
    pub version: SourceVersion,
    pub parser_version: String,
}

impl SourceRef {
    pub fn new(
        locator: impl Into<String>,
        source_kind: impl Into<String>,
        version: SourceVersion,
        parser_version: impl Into<String>,
    ) -> Result<Self, ContractError> {
        Ok(Self {
            locator: required(locator, "source_ref.locator")?,
            source_kind: required(source_kind, "source_ref.source_kind")?,
            version,
            parser_version: required(parser_version, "source_ref.parser_version")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceLocator {
    pub provider: AgentKind,
    pub path: String,
    pub native_id: Option<String>,
}

impl SourceLocator {
    pub fn new(
        provider: AgentKind,
        path: impl Into<String>,
        native_id: Option<String>,
    ) -> Result<Self, ContractError> {
        Ok(Self {
            provider,
            path: required(path, "source_locator.path")?,
            native_id: native_id
                .map(|value| required(value, "source_locator.native_id"))
                .transpose()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionKey {
    pub provider: AgentKind,
    pub namespace: String,
    pub native_id: String,
}

impl SessionKey {
    pub fn new(
        provider: AgentKind,
        namespace: impl Into<String>,
        native_id: impl Into<String>,
    ) -> Result<Self, ContractError> {
        Ok(Self {
            provider,
            namespace: required(namespace, "session_key.namespace")?,
            native_id: required(native_id, "session_key.native_id")?,
        })
    }

    pub fn stable_string(&self) -> String {
        format!("{:?}\0{}\0{}", self.provider, self.namespace, self.native_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainSnapshot<T> {
    pub scope_key: ScopeKey,
    pub domain: String,
    pub revision: Revision,
    pub source_version: Option<SourceVersion>,
    pub schema_version: u32,
    pub snapshot_id: String,
    pub payload: T,
}

impl<T> DomainSnapshot<T> {
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> DomainSnapshot<U> {
        DomainSnapshot {
            scope_key: self.scope_key,
            domain: self.domain,
            revision: self.revision,
            source_version: self.source_version,
            schema_version: self.schema_version,
            snapshot_id: self.snapshot_id,
            payload: map(self.payload),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Scan,
    Watch,
    Backfill,
    Analytics,
    SkillUpdate,
    Projection,
}

impl OperationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Watch => "watch",
            Self::Backfill => "backfill",
            Self::Analytics => "analytics",
            Self::SkillUpdate => "skill_update",
            Self::Projection => "projection",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Queued,
    Running,
    Committing,
    Committed,
    Published,
    Failed,
    Cancelled,
    TimedOut,
    Stale,
}

impl OperationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Committing => "committing",
            Self::Committed => "committed",
            Self::Published => "published",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Stale => "stale",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRecord {
    pub operation_id: OperationId,
    pub kind: OperationKind,
    pub scope_key: ScopeKey,
    pub status: OperationStatus,
    pub input_revision: Revision,
    pub source_version: Option<SourceVersion>,
    pub checkpoint_json: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionHead {
    pub scope_key: ScopeKey,
    pub domain: String,
    pub revision: Revision,
    pub source_version: Option<SourceVersion>,
    pub schema_version: u32,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionedEvent<T> {
    pub scope_key: ScopeKey,
    pub domain: String,
    pub operation_id: OperationId,
    pub base_revision: Revision,
    pub revision: Revision,
    pub source_version: Option<SourceVersion>,
    pub payload: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionDecision<T> {
    pub accepted: bool,
    pub needs_resync: bool,
    pub payload: Option<T>,
}

pub fn decide_revision<T>(
    local_revision: Revision,
    event: RevisionedEvent<T>,
) -> RevisionDecision<T> {
    if event.revision <= local_revision {
        return RevisionDecision {
            accepted: false,
            needs_resync: false,
            payload: None,
        };
    }
    if event.base_revision != local_revision {
        return RevisionDecision {
            accepted: false,
            needs_resync: true,
            payload: None,
        };
    }
    RevisionDecision {
        accepted: true,
        needs_resync: false,
        payload: Some(event.payload),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_reject_empty_values() {
        assert!(ScopeKey::new(" ").is_err());
        assert!(OperationId::new("").is_err());
        assert!(InstallationId::new("\n").is_err());
        assert!(SessionKey::new(AgentKind::Codex, "codex", "").is_err());
    }

    #[test]
    fn revision_decision_drops_old_events_and_resyncs_gaps() {
        let scope_key = ScopeKey::new("workspace:/repo").unwrap();
        let operation_id = OperationId::new("op-1").unwrap();
        let old = RevisionedEvent {
            scope_key: scope_key.clone(),
            domain: "sessions".to_string(),
            operation_id: operation_id.clone(),
            base_revision: Revision::new(3),
            revision: Revision::new(3),
            source_version: None,
            payload: "old",
        };
        assert!(!decide_revision(Revision::new(3), old).accepted);

        let gap = RevisionedEvent {
            scope_key: scope_key.clone(),
            domain: "sessions".to_string(),
            operation_id: operation_id.clone(),
            base_revision: Revision::new(2),
            revision: Revision::new(4),
            source_version: None,
            payload: "gap",
        };
        let decision = decide_revision(Revision::new(3), gap);
        assert!(!decision.accepted);
        assert!(decision.needs_resync);

        let current = RevisionedEvent {
            scope_key,
            domain: "sessions".to_string(),
            operation_id,
            base_revision: Revision::new(3),
            revision: Revision::new(4),
            source_version: None,
            payload: "current",
        };
        let decision = decide_revision(Revision::new(3), current);
        assert!(decision.accepted);
        assert_eq!(decision.payload, Some("current"));
    }

    #[test]
    fn snapshots_can_change_payload_without_changing_identity_metadata() {
        let snapshot = DomainSnapshot {
            scope_key: ScopeKey::new("workspace:/repo").unwrap(),
            domain: "sessions".to_string(),
            revision: Revision::new(7),
            source_version: Some(SourceVersion::new("hash").unwrap()),
            schema_version: 1,
            snapshot_id: "snapshot-7".to_string(),
            payload: vec![1, 2],
        };
        let mapped = snapshot.map(|payload| payload.len());
        assert_eq!(mapped.revision, Revision::new(7));
        assert_eq!(mapped.payload, 2);
    }

    #[test]
    fn session_keys_keep_provider_identity_separate() {
        let codex = SessionKey::new(AgentKind::Codex, "codex", "same-native-id").unwrap();
        let cursor = SessionKey::new(AgentKind::Cursor, "cursor", "same-native-id").unwrap();
        assert_ne!(codex, cursor);
        assert_ne!(codex.stable_string(), cursor.stable_string());
    }

    #[test]
    fn source_locator_requires_a_path_and_preserves_optional_native_id() {
        assert!(SourceLocator::new(AgentKind::Claude, "", None).is_err());
        let locator = SourceLocator::new(
            AgentKind::Claude,
            "/tmp/session.jsonl",
            Some("native-1".to_string()),
        )
        .unwrap();
        assert_eq!(locator.native_id.as_deref(), Some("native-1"));
    }
}
