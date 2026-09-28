use super::*;

#[test]
fn completed_search_publication_does_not_emit_sessions_scan() {
    let root = std::env::temp_dir().join(format!(
        "tendi-search-publication-event-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("create search publication test workspace");
    let daemon = Daemon::with_database(root.clone(), root.join("state.sqlite3"), false);
    let subscription = daemon.subscribe_events();
    let scope =
        tendi_core::ScopeKey::new("workspace:search-publication-test").expect("valid test scope");

    record_session_search_publication(
        &scope,
        "session-1",
        tendi_core::Revision::new(4),
        tendi_core::Revision::new(5),
    );

    assert!(matches!(
        subscription.recv_timeout(Duration::from_millis(20)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    daemon.shutdown();
    fs::remove_dir_all(root).expect("remove search publication test workspace");
}
