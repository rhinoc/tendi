use super::*;

#[test]
fn stale_project_scan_cannot_publish_after_configuration_changes() {
    let root = std::env::temp_dir().join(format!(
        "tendi-project-scan-cas-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("projects")).unwrap();
    let store = Store::open(root.join("test.sqlite3")).unwrap();
    store
        .save_project_scan_scopes(vec![root.join("projects").to_string_lossy().into_owned()])
        .unwrap();
    let scan = store.prepare_project_scan().unwrap();
    store.save_project_scan_scopes(vec![]).unwrap();
    let error = store.commit_project_scan(scan, None).unwrap_err();
    assert!(error.to_string().contains("configuration changed"));
    assert!(store.list_projects().unwrap().is_empty());
    assert!(
        store
            .project_scan_scopes()
            .unwrap()
            .iter()
            .all(|scope| scope.last_scanned_at.is_none())
    );
    // A fresh scan against the new configuration still succeeds.
    store.scan_projects().unwrap();
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
