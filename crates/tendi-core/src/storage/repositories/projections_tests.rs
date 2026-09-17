use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

fn with_database(test: impl FnOnce(&Path, Store)) {
    let root = std::env::temp_dir().join(format!(
        "tendi-projection-cas-{}-{}-{}",
        std::process::id(),
        NEXT_DATABASE.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let store = Store::open(root.join("test.sqlite3")).unwrap();
    test(&root, store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dirty_resources_survive_restart_and_reach_known_workspaces() {
    with_database(|workspace, store| {
        let other = workspace.join("other-workspace");
        fs::create_dir(&other).unwrap();
        let unrelated = workspace.join("unrelated-workspace");
        fs::create_dir(&unrelated).unwrap();
        let resource = workspace.join("shared-rule");
        fs::write(&resource, "shared").unwrap();
        let scan = RuleScan {
            rules: vec![RuleRecord {
                agents: vec![AgentKind::Shared],
                kind: "agents".into(),
                scope: "global".into(),
                path: resource.clone(),
                order: 0,
                sha256: String::new(),
            }],
            warnings: vec![],
        };
        store.save_rules_for_workspace(workspace, &scan).unwrap();
        let mut other_scan = scan.clone();
        #[cfg(unix)]
        {
            let alias = other.join("rule-alias");
            std::os::unix::fs::symlink(&resource, &alias).unwrap();
            other_scan.rules[0].path = alias;
        }
        store.save_rules_for_workspace(&other, &other_scan).unwrap();
        store
            .save_rules_for_workspace(
                &unrelated,
                &RuleScan {
                    rules: vec![],
                    warnings: vec![],
                },
            )
            .unwrap();
        store
            .invalidate_projection_resources("rules", workspace, &[resource.clone()], true)
            .unwrap();
        let path = store.path().to_path_buf();
        drop(store);
        let store = Store::open(path).unwrap();
        let state = store
            .read_projection_refresh_state::<RuleScan>("rules", &other)
            .unwrap();
        assert!(!state.full_refresh);
        assert_eq!(
            state.resources,
            vec![crate::coordination::canonical_resource_path(&resource).unwrap()]
        );
        assert_eq!(state.reconcile_resources, state.resources);
        assert_eq!(store.pending_projection_scopes("rules").unwrap().len(), 2);
        let untouched = store
            .read_projection_refresh_state::<RuleScan>("rules", &unrelated)
            .unwrap();
        assert!(untouched.resources.is_empty());
        assert!(!untouched.full_refresh);
    });
}

#[test]
fn full_invalidation_does_not_fan_out_to_other_scopes() {
    with_database(|workspace, store| {
        let other = workspace.join("other-workspace");
        fs::create_dir(&other).unwrap();
        let scan = RuleScan {
            rules: vec![],
            warnings: vec![],
        };
        store.save_rules_for_workspace(workspace, &scan).unwrap();
        store.save_rules_for_workspace(&other, &scan).unwrap();

        store
            .invalidate_projection_resources("rules", workspace, &[], true)
            .unwrap();

        let current = store
            .read_projection_refresh_state::<RuleScan>("rules", workspace)
            .unwrap();
        assert!(current.full_refresh);
        assert!(current.reconcile_full);

        let untouched = store
            .read_projection_refresh_state::<RuleScan>("rules", &other)
            .unwrap();
        assert!(!untouched.full_refresh);
        assert!(!untouched.reconcile_full);
        assert_eq!(
            store.pending_projection_scopes("rules").unwrap(),
            vec![workspace.canonicalize().unwrap()]
        );
    });
}

#[test]
fn pending_scopes_prune_dirty_rows_for_deleted_workspaces() {
    with_database(|workspace, store| {
        let deleted = workspace.join("deleted-workspace");
        fs::create_dir(&deleted).unwrap();
        let scan = RuleScan {
            rules: vec![],
            warnings: vec![],
        };
        store.save_rules_for_workspace(workspace, &scan).unwrap();
        store.save_rules_for_workspace(&deleted, &scan).unwrap();
        store
            .invalidate_projection_resources("rules", workspace, &[], true)
            .unwrap();
        store
            .invalidate_projection_resources("rules", &deleted, &[], true)
            .unwrap();
        fs::remove_dir_all(&deleted).unwrap();

        assert_eq!(
            store.pending_projection_scopes("rules").unwrap(),
            vec![workspace.canonicalize().unwrap()]
        );
        assert!(
            !store
                .read_projection_refresh_state::<RuleScan>("rules", &deleted)
                .unwrap()
                .reconcile_full
        );
    });
}

#[test]
fn dirty_acknowledgement_never_clears_a_newer_generation() {
    with_database(|workspace, store| {
        let scan = RuleScan {
            rules: vec![],
            warnings: vec![],
        };
        store.save_rules_for_workspace(workspace, &scan).unwrap();
        let resource = workspace.join("rule");
        store
            .invalidate_projection_resources("rules", workspace, &[resource.clone()], true)
            .unwrap();
        let first = store
            .read_projection_refresh_state::<RuleScan>("rules", workspace)
            .unwrap();
        store
            .invalidate_projection_resources("rules", workspace, &[resource], true)
            .unwrap();
        assert!(
            !store
                .save_rules_for_workspace_if_revision(workspace, &scan, first.revision)
                .unwrap()
        );
        store
            .acknowledge_projection_reconciliation("rules", workspace, first.revision)
            .unwrap();
        let current = store
            .read_projection_refresh_state::<RuleScan>("rules", workspace)
            .unwrap();
        assert_eq!(current.resources.len(), 1);
        assert_eq!(current.reconcile_resources.len(), 1);
        assert!(
            store
                .save_rules_for_workspace_if_revision(workspace, &scan, current.revision)
                .unwrap()
        );
        let published = store
            .read_projection_refresh_state::<RuleScan>("rules", workspace)
            .unwrap();
        assert!(published.resources.is_empty());
        assert_eq!(published.reconcile_resources.len(), 1);
        store
            .acknowledge_projection_reconciliation("rules", workspace, current.revision)
            .unwrap();
        assert!(store.pending_projection_scopes("rules").unwrap().is_empty());
    });
}

#[test]
fn dirty_resource_marking_rolls_back_with_canonical_mutation() {
    with_database(|workspace, store| {
        let scan = RuleScan {
            rules: vec![],
            warnings: vec![],
        };
        store.save_rules_for_workspace(workspace, &scan).unwrap();
        let before = store
            .read_projection_refresh_state::<RuleScan>("rules", workspace)
            .unwrap();
        let scope = workspace_scope_key(workspace).unwrap();
        let result: Result<()> = store.with_named_write_transaction("test.dirty_rollback", |tx| {
            store.mark_projection_resources_in_tx(
                tx,
                &scope,
                "rules",
                &[workspace.join("rule")],
                false,
                true,
            )?;
            anyhow::bail!("abort canonical mutation");
        });
        assert!(result.is_err());
        let after = store
            .read_projection_refresh_state::<RuleScan>("rules", workspace)
            .unwrap();
        assert_eq!(before.revision, after.revision);
        assert!(after.resources.is_empty());
        store.invalidate_projection("rules", workspace).unwrap();
        assert!(
            store
                .read_projection_refresh_state::<RuleScan>("rules", workspace)
                .unwrap()
                .full_refresh
        );
    });
}

#[test]
fn dirty_aggregate_publication_checks_every_domain_before_writing() {
    with_database(|workspace, store| {
        let report = ScanReport {
            agents: crate::agents::AgentScan {
                agents: vec![],
                warnings: vec![],
            },
            skills: SkillScan {
                roots: vec![],
                skills: vec![],
                warnings: vec![],
            },
            sessions: SessionScan {
                sessions: vec![],
                warnings: vec![],
            },
            rules: RuleScan {
                rules: vec![],
                warnings: vec![],
            },
            hooks: HookScan {
                hooks: vec![],
                warnings: vec![],
            },
            mcp: McpScan {
                servers: vec![],
                warnings: vec![],
            },
        };
        let scope = workspace_scope_key(workspace).unwrap();
        let capture = || {
            ["agents", "skills", "rules", "hooks", "mcp", "sessions"]
                .into_iter()
                .map(|domain| {
                    (
                        domain.to_owned(),
                        store
                            .projection_head(&scope, domain)
                            .unwrap()
                            .map(|head| head.revision)
                            .unwrap_or(Revision::ZERO),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        };
        assert!(
            store
                .save_scan_for_workspace_if_revisions(workspace, &report, &capture())
                .unwrap()
        );
        let old = capture();
        store
            .invalidate_projection_resources("rules", workspace, &[workspace.join("rule")], false)
            .unwrap();
        assert!(
            !store
                .save_scan_for_workspace_if_revisions(workspace, &report, &old)
                .unwrap()
        );
        assert_eq!(
            store
                .projection_head(&scope, "agents")
                .unwrap()
                .unwrap()
                .revision,
            old["agents"]
        );
        assert_eq!(
            store
                .read_projection_refresh_state::<RuleScan>("rules", workspace)
                .unwrap()
                .resources
                .len(),
            1
        );
        assert!(
            store
                .save_scan_for_workspace_if_revisions(workspace, &report, &capture())
                .unwrap()
        );
        assert!(store.pending_projection_scopes("rules").unwrap().is_empty());
    });
}

#[test]
fn stale_scan_cannot_overwrite_a_newer_publication() {
    with_database(|workspace, store| {
        let scan = RuleScan {
            rules: vec![],
            warnings: vec![],
        };
        let (initial, cached) = store
            .read_cached_projection_with_revision::<RuleScan>("rules", workspace)
            .unwrap();
        assert_eq!(initial, Revision::ZERO);
        assert!(cached.is_none());
        assert!(
            store
                .save_rules_for_workspace_if_revision(workspace, &scan, initial)
                .unwrap()
        );
        let (published, cached) = store
            .read_cached_projection_with_revision::<RuleScan>("rules", workspace)
            .unwrap();
        assert_eq!(published.value(), 1);
        assert!(cached.is_some());
        let stale = RuleScan {
            rules: vec![],
            warnings: vec!["old failure".to_owned()],
        };
        assert!(
            !store
                .save_rules_for_workspace_if_revision(workspace, &stale, initial)
                .unwrap()
        );
        let head = store
            .projection_head(&workspace_scope_key(workspace).unwrap(), "rules")
            .unwrap()
            .unwrap();
        assert_eq!(head.revision, published);
        assert_eq!(head.status, "ready");
    });
}

#[test]
fn invalidation_rejects_in_flight_scan_without_erasing_cached_rows() {
    with_database(|workspace, store| {
        let scan = RuleScan {
            rules: vec![],
            warnings: vec![],
        };
        store.save_rules_for_workspace(workspace, &scan).unwrap();
        let (before, _) = store
            .read_cached_projection_with_revision::<RuleScan>("rules", workspace)
            .unwrap();
        store.invalidate_projection("rules", workspace).unwrap();
        let (invalidated, cached) = store
            .read_cached_projection_with_revision::<RuleScan>("rules", workspace)
            .unwrap();
        assert!(invalidated > before);
        assert!(cached.is_some());
        assert!(
            !store
                .save_rules_for_workspace_if_revision(workspace, &scan, before)
                .unwrap()
        );
        assert!(
            store
                .save_rules_for_workspace_if_revision(workspace, &scan, invalidated)
                .unwrap()
        );
    });
}

#[test]
fn concurrent_prepared_scans_have_exactly_one_winner() {
    with_database(|workspace, first| {
        let second = Store::open(first.path()).unwrap();
        let workspace_owned = workspace.to_path_buf();
        let scan = RuleScan {
            rules: vec![],
            warnings: vec![],
        };
        let other_scan = scan.clone();
        let thread = std::thread::spawn(move || {
            first
                .save_rules_for_workspace_if_revision(&workspace_owned, &other_scan, Revision::ZERO)
                .unwrap()
        });
        let second_won = second
            .save_rules_for_workspace_if_revision(workspace, &scan, Revision::ZERO)
            .unwrap();
        assert_ne!(thread.join().unwrap(), second_won);
        let (revision, _) = second
            .read_cached_projection_with_revision::<RuleScan>("rules", workspace)
            .unwrap();
        assert_eq!(revision.value(), 1);
    });
}
