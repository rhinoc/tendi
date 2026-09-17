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
