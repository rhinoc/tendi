use super::*;

#[test]
fn targeted_events_probe_only_the_owner_without_enumerating_processes() {
    let selected = BTreeSet::from([AgentKind::Claude]);
    let probe = probe_selected(&std::env::temp_dir(), &[], Some(&selected));
    assert_eq!(probe.providers.len(), 1);
    assert_eq!(probe.providers[0].0, AgentKind::Claude);
    assert!(
        probe
            .watch_paths
            .iter()
            .all(|(agent, _)| *agent == AgentKind::Claude)
    );
    assert!(probe.process_ids.is_none());
}
