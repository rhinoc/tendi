    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn current_index_version_skips_repeated_migration() {
        let base = std::env::temp_dir().join(format!(
            "tendi-session-skill-index-version-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = Store::open(base.join("tendi.sqlite3")).unwrap();
        let scope_key = ScopeKey::new("workspace:/workspace").unwrap();

        assert!(ensure_current_version(&store, &scope_key).unwrap());
        assert!(!ensure_current_version(&store, &scope_key).unwrap());

        let rows = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM meta WHERE key = ?1 AND value = ?2",
                params![
                    format!("{MIGRATION_KEY_PREFIX}{}", scope_key.as_str()),
                    INDEX_VERSION
                ],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(rows, 1);
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }
