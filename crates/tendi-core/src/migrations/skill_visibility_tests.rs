    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn visibility_data_cleanup_runs_once() {
        let base = std::env::temp_dir().join(format!(
            "tendi-skill-visibility-migration-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = Store::open(base.join("tendi.sqlite3")).unwrap();
        migrate_legacy_data(&store).unwrap();
        store
            .with_named_write_transaction("test.insert_unlocked_skill_visibility", |tx| {
                tx.execute(
                    "INSERT INTO scoped_skill_visibility
                        (scope_key, skill_path, visibility, locked)
                     VALUES ('workspace:/workspace', '/workspace/skill', 'auto', 0)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();

        migrate_legacy_data(&store).unwrap();

        let remaining = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM scoped_skill_visibility WHERE locked = 0",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(remaining, 1);
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }
