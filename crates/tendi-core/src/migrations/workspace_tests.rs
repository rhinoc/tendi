    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parser_context_migration_marks_empty_database_complete() {
        let base = std::env::temp_dir().join(format!(
            "tendi-parser-context-migration-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = Store::open(base.join("tendi.sqlite3")).unwrap();
        let migration_key =
            format!("projection_context_parser_migrated_v1:{PROJECTION_PARSER_VERSION}");

        invalidate_old_projection_contexts(&store).unwrap();
        assert!(migration_completed(&store, &migration_key).unwrap());
        invalidate_old_projection_contexts(&store).unwrap();

        drop(store);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn completed_workspace_migrations_reuse_a_fresh_projection() {
        let base = std::env::temp_dir().join(format!(
            "tendi-workspace-migrations-complete-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cwd = base.join("workspace");
        let store = Store::open(base.join("tendi.sqlite3")).unwrap();
        let workspace_root = crate::storage::canonical_workspace_root(&cwd);

        mark_migration_completed(&store, CODEX_GLOBAL_SKILL_CONFIG_MIGRATION_KEY).unwrap();
        mark_migration_completed(
            &store,
            &format!(
                "skill_visibility_database_migrated_v1:{}",
                workspace_root.display()
            ),
        )
        .unwrap();
        mark_migration_completed(
            &store,
            &skill_sources::migration_key_for_workspace(&cwd).unwrap(),
        )
        .unwrap();

        let empty_scan = SkillScan {
            roots: Vec::new(),
            skills: Vec::new(),
            warnings: Vec::new(),
        };
        store.save_skills_for_workspace(&cwd, &empty_scan).unwrap();
        assert_eq!(
            store.projection_status("skills", &cwd).unwrap(),
            crate::storage::ProjectionStatus::Fresh
        );
        assert!(skill_metadata::migration_completed_for_workspace(&store, &cwd).unwrap());
        assert!(skill_sources::migration_completed_for_workspace(&store, &cwd).unwrap());
        assert!(!run_workspace(&store, &cwd, &[]).unwrap());
        crate::initialize_workspace(&store, &cwd, &[]).unwrap();
        assert_eq!(
            store.projection_status("skills", &cwd).unwrap(),
            crate::storage::ProjectionStatus::Fresh
        );

        fs::remove_dir_all(base).unwrap();
    }
