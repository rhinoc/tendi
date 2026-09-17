use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

struct Database(PathBuf);

impl Database {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tendi-settings-patch-{}-{}-{}",
            std::process::id(),
            NEXT_DATABASE.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn open(&self) -> Store {
        Store::open(self.0.join("test.sqlite3")).unwrap()
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn patch(value: serde_json::Value) -> AppSettingsPatch {
    serde_json::from_value(value).unwrap()
}

#[test]
fn independent_field_patches_do_not_overwrite_each_other() {
    let db = Database::new();
    let first = db.open();
    let second = db.open();
    let barrier = Arc::new(Barrier::new(2));
    let other_barrier = barrier.clone();
    let writer = std::thread::spawn(move || {
        other_barrier.wait();
        for _ in 0..40 {
            first
                .patch_app_settings(patch(serde_json::json!({"appearance":"dark"})))
                .unwrap();
        }
    });
    barrier.wait();
    for _ in 0..40 {
        second
            .patch_app_settings(patch(serde_json::json!({"terminal":"custom-terminal"})))
            .unwrap();
    }
    writer.join().unwrap();
    let saved = second.app_settings().unwrap();
    assert_eq!(saved.appearance, "dark");
    assert_eq!(saved.terminal, "custom-terminal");
}

#[test]
fn settings_read_never_mixes_committed_field_versions() {
    let db = Database::new();
    let writer = db.open();
    let reader = db.open();
    writer
        .patch_app_settings(patch(
            serde_json::json!({"terminal":"false","developerMode":false}),
        ))
        .unwrap();
    let handle = std::thread::spawn(move || {
        for index in 0..100 {
            let enabled = index % 2 == 0;
            writer
                .patch_app_settings(patch(
                    serde_json::json!({"terminal":enabled.to_string(),"developerMode":enabled}),
                ))
                .unwrap();
        }
    });
    for _ in 0..100 {
        let settings = reader.app_settings().unwrap();
        assert_eq!(settings.terminal, settings.developer_mode.to_string());
    }
    handle.join().unwrap();
}

#[test]
fn invalid_patch_is_atomic_and_missing_fields_are_unchanged() {
    let db = Database::new();
    let store = db.open();
    store
        .patch_app_settings(patch(serde_json::json!({"terminal":"custom"})))
        .unwrap();
    assert!(
        store
            .patch_app_settings(patch(
                serde_json::json!({"terminal":"new","appearance":"invalid"})
            ))
            .is_err()
    );
    assert_eq!(store.app_settings().unwrap().terminal, "custom");
    let saved = store
        .patch_app_settings(patch(
            serde_json::json!({"developerMode":false,"additionalSessionRoots":[]}),
        ))
        .unwrap();
    assert_eq!(saved.terminal, "custom");
    assert!(!saved.developer_mode);
}

#[test]
fn profile_updates_merge_by_provider_and_conditional_delete_preserves_new_selection() {
    let db = Database::new();
    let store = db.open();
    store.set_config_profile("codex", Some("old")).unwrap();
    store.set_config_profile("claude", Some("other")).unwrap();
    store.set_config_profile("codex", Some("new")).unwrap();
    store
        .patch_app_settings(patch(serde_json::json!({"appearance":"dark"})))
        .unwrap();
    let settings = store
        .clear_config_profiles_if_matching(&[("codex".to_owned(), "old".to_owned())])
        .unwrap();
    assert_eq!(settings.config_profiles["codex"], "new");
    assert_eq!(settings.config_profiles["claude"], "other");
    store.set_config_profile("codex", Some("new")).unwrap();
    let settings = store
        .clear_config_profiles_if_matching(&[
            ("codex".to_owned(), "new".to_owned()),
            ("codex".to_owned(), "old".to_owned()),
        ])
        .unwrap();
    assert!(!settings.config_profiles.contains_key("codex"));
    assert_eq!(settings.config_profiles["claude"], "other");
    assert_eq!(settings.appearance, "dark");
    let settings = store.set_config_profile("codex", None).unwrap();
    assert!(!settings.config_profiles.contains_key("codex"));
    assert_eq!(settings.config_profiles["claude"], "other");
}
