use super::*;

struct Fixture {
    root: PathBuf,
    workspace: PathBuf,
    store: Store,
    config: BackupConfig,
}
impl Fixture {
    fn new(label: &str) -> Self {
        crate::test_support::ensure_isolated_environment();
        let root = std::env::temp_dir().join(format!(
            "tendi-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let store = Store::open(root.join("store.sqlite3")).unwrap();
        let config = BackupConfig::new("", root.join("checkout"));
        store.save_skill_backup_config(&config).unwrap();
        ensure_checkout(&config).unwrap();
        Self {
            workspace: root.join("receiver"),
            root,
            store,
            config,
        }
    }
    fn publish(&self, manifest: &BackupManifest) -> String {
        write_snapshot(manifest, &self.config.checkout_path).unwrap();
        commit_checkout(&self.config, manifest, "test").unwrap();
        current_commit(&self.config.checkout_path).unwrap()
    }
    fn plan(&self) -> BackupPullPlan {
        plan_backup_pull(
            &self.store,
            &self.workspace,
            &"shared".parse().unwrap(),
            crate::SkillInstallScope::Project,
            None,
        )
        .unwrap()
    }
    fn source(&self, name: &str, body: &str) -> SkillSourceRecord {
        let path = self.root.join("publisher").join(name);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {name}\n---\n{body}\n"),
        )
        .unwrap();
        SkillSourceRecord {
            skill_name: name.into(),
            skill_path: path,
            source_kind: "local".into(),
            source: None,
            source_ref: None,
            source_version: None,
            source_relative_path: None,
            update_status: "local".into(),
            origin: "tendi-install".into(),
        }
    }
    fn rule(&self, body: &str) -> (BackupManifest, PathBuf) {
        let relative = format!(
            "{}/AGENTS.md",
            self.root.file_name().unwrap().to_string_lossy()
        );
        let target = crate::providers::agent_provider(crate::AgentKind::Codex)
            .restore_global_source_path(&relative)
            .unwrap();
        let mut manifest = BackupManifest {
            version: BACKUP_MANIFEST_VERSION,
            ..Default::default()
        };
        manifest.artifacts.push(BackupArtifact {
            id: "rules-test".into(),
            category: "rules".into(),
            name: "AGENTS.md".into(),
            agent: "codex".into(),
            source_relative_path: relative,
            entry_key: String::new(),
            entry_selector: Vec::new(),
            files: vec![BackupFile {
                path: "AGENTS.md".into(),
                sha256: sha256_hex(body.as_bytes()),
                size: body.len() as u64,
            }],
        });
        manifest
            .artifact_contents
            .insert("rules:rules-test".into(), body.as_bytes().to_vec());
        (manifest, target)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn rules_keep_both_tracks_the_sibling_through_updates_and_removal() {
    let fixture = Fixture::new("pull-rule-sibling");
    let (manifest, target) = fixture.rule("# remote v1\n");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, "# local rules\n").unwrap();
    fixture.publish(&manifest);
    let plan = fixture.plan();
    assert_eq!(plan.operations[0].status, "conflict");
    let operations = apply_backup_pull(
        &plan,
        &fixture.store,
        &fixture.workspace,
        &[BackupRestoreResolution {
            id: "rules-test".into(),
            action: "keep-both".into(),
        }],
    )
    .unwrap();
    let sibling = operations[0].target.clone();
    assert_ne!(sibling, target);
    let (updated, _) = fixture.rule("# remote v2\n");
    fixture.publish(&updated);
    let plan = fixture.plan();
    assert_eq!(plan.operations[0].target, sibling);
    assert_eq!(plan.operations[0].status, "replace");
    apply_backup_pull(&plan, &fixture.store, &fixture.workspace, &[]).unwrap();
    assert_eq!(fs::read_to_string(&sibling).unwrap(), "# remote v2\n");
    fixture.publish(&BackupManifest {
        version: BACKUP_MANIFEST_VERSION,
        ..Default::default()
    });
    apply_backup_pull(&fixture.plan(), &fixture.store, &fixture.workspace, &[]).unwrap();
    assert!(!sibling.exists());
    assert_eq!(fs::read_to_string(&target).unwrap(), "# local rules\n");
    fs::remove_dir_all(target.parent().unwrap()).unwrap();
}

#[test]
fn keep_local_is_remembered_without_claiming_or_deleting_local_content() {
    let fixture = Fixture::new("pull-keep-local");
    let record = fixture.source("review", "remote v1");
    let manifest = build_manifest(&[record.clone()], &BackupBuildOptions::default()).unwrap();
    fixture.publish(&manifest);
    let local = fixture.workspace.join(".agents/skills/review");
    fs::create_dir_all(&local).unwrap();
    fs::write(local.join("SKILL.md"), "local skill\n").unwrap();
    let plan = fixture.plan();
    apply_backup_pull(
        &plan,
        &fixture.store,
        &fixture.workspace,
        &[BackupRestoreResolution {
            id: plan.operations[0].id.clone(),
            action: "skip".into(),
        }],
    )
    .unwrap();
    let next = fixture.plan();
    assert_eq!(next.operations[0].status, "skipped");
    apply_backup_pull(&next, &fixture.store, &fixture.workspace, &[]).unwrap();
    let updated = fixture.source("review", "remote v2");
    fixture.publish(&build_manifest(&[updated], &BackupBuildOptions::default()).unwrap());
    assert_eq!(fixture.plan().operations[0].status, "skipped");
    fixture.publish(&BackupManifest {
        version: BACKUP_MANIFEST_VERSION,
        ..Default::default()
    });
    apply_backup_pull(&fixture.plan(), &fixture.store, &fixture.workspace, &[]).unwrap();
    assert_eq!(
        fs::read_to_string(local.join("SKILL.md")).unwrap(),
        "local skill\n"
    );
    fixture.publish(&build_manifest(&[record], &BackupBuildOptions::default()).unwrap());
    assert_eq!(fixture.plan().operations[0].status, "conflict");
}

#[test]
fn first_pull_migrates_verified_legacy_restores_and_retires_removed_entries() {
    let fixture = Fixture::new("pull-legacy-restore");
    let alpha = fixture.source("alpha", "alpha v1");
    let retired = fixture.source("retired", "retired v1");
    let old = build_manifest(&[alpha.clone(), retired], &BackupBuildOptions::default()).unwrap();
    let revision = fixture.publish(&old);
    let target = "shared".parse().unwrap();
    let plan = plan_backup_restore(
        &fixture.store,
        &fixture.workspace,
        &revision,
        &[],
        &target,
        crate::SkillInstallScope::Project,
    )
    .unwrap();
    apply_backup_restore(&plan, &fixture.store, &fixture.workspace, &[]).unwrap();
    let local_only = fixture.workspace.join(".agents/skills/local-only");
    fs::create_dir_all(&local_only).unwrap();
    fs::write(local_only.join("SKILL.md"), "local only\n").unwrap();
    fixture.publish(&build_manifest(&[alpha], &BackupBuildOptions::default()).unwrap());
    let plan = fixture.plan();
    assert!(
        plan.operations
            .iter()
            .any(|operation| operation.name == "retired" && operation.status == "remove")
    );
    apply_backup_pull(&plan, &fixture.store, &fixture.workspace, &[]).unwrap();
    assert!(!fixture.workspace.join(".agents/skills/retired").exists());
    assert!(local_only.join("SKILL.md").exists());
}

#[test]
fn stale_current_targets_require_a_new_pull_plan() {
    let fixture = Fixture::new("pull-stale-target");
    let record = fixture.source("review", "remote");
    fixture.publish(&build_manifest(&[record], &BackupBuildOptions::default()).unwrap());
    apply_backup_pull(&fixture.plan(), &fixture.store, &fixture.workspace, &[]).unwrap();
    let plan = fixture.plan();
    assert_eq!(plan.operations[0].status, "current");
    let path = plan.operations[0].target.join("SKILL.md");
    fs::write(&path, "edited while waiting\n").unwrap();
    assert!(!backup_pull_plan_is_current(&plan, &fixture.store).unwrap());
    assert!(apply_backup_pull(&plan, &fixture.store, &fixture.workspace, &[]).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "edited while waiting\n");
}

#[test]
fn pull_resources_include_shared_git_metadata_for_linked_worktrees() {
    let fixture = Fixture::new("pull-worktree-resources");
    fixture.publish(&BackupManifest {
        version: BACKUP_MANIFEST_VERSION,
        ..Default::default()
    });
    let linked = fixture.root.join("linked");
    run_git_success(
        &fixture.config.checkout_path,
        [
            "worktree",
            "add",
            "-b",
            "receiver",
            linked.to_str().unwrap(),
        ],
        false,
    )
    .unwrap();
    fixture
        .store
        .save_skill_backup_config(&BackupConfig::new("", linked))
        .unwrap();
    let common =
        crate::coordination::canonical_resource_path(&fixture.config.checkout_path.join(".git"))
            .unwrap();
    assert!(
        pull_resource_paths(&fixture.plan())
            .iter()
            .any(|path| { crate::coordination::canonical_resource_path(path).unwrap() == common })
    );
}

#[cfg(unix)]
#[test]
fn dangling_rule_symlinks_are_rejected_before_any_external_file_is_created() {
    let fixture = Fixture::new("pull-dangling-rule");
    let (manifest, target) = fixture.rule("# remote rules\n");
    fixture.publish(&manifest);
    let outside = fixture.root.join("external-rule");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &target).unwrap();
    let plan = fixture.plan();
    assert!(apply_backup_pull(&plan, &fixture.store, &fixture.workspace, &[]).is_err());
    assert!(!outside.exists());
    assert!(
        fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    fs::remove_dir_all(target.parent().unwrap()).unwrap();
}
