use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    BackupBuildOptions, BackupConfig, BackupManifest, adopt_skill_for_backup, backup_catalog,
    backup_now, backup_statuses_for_paths, backup_versions, build_manifest, catalog_source_files,
    current_machine_name, discover_git_repository_root, ensure_checkout, is_remote_repository,
    normalize_remote_url, sync_checkout_for_restore, validate_manifest, write_snapshot,
};
use crate::{
    SkillInstallScope, SkillTarget,
    skills::{
        AgentKind, SkillPath, SkillRecord, SkillRoot, SkillScan, SkillSourceRecord, SkillVisibility,
    },
    storage::Store,
};

#[test]
fn checkout_mutation_resources_cover_an_uninitialized_checkout() {
    let root = temp_dir("tendi-backup-resources-new");
    let checkout = root.join("checkout");
    assert_eq!(
        super::checkout_mutation_resource_paths(&checkout).unwrap(),
        vec![checkout]
    );
    assert!(!root.exists());
}

#[test]
fn checkout_mutation_resources_conflict_across_linked_worktrees() {
    use crate::coordination::{ResourceLease, canonical_resource_path};

    let root = temp_dir("tendi-backup-resources-worktree");
    let repository = root.join("repository");
    let first = root.join("first");
    let second = root.join("second");
    fs::create_dir_all(&repository).unwrap();
    run_git(&repository, &["init", "--initial-branch=main"]);
    run_git(
        &repository,
        &[
            "-c",
            "user.name=Tendi test",
            "-c",
            "user.email=test@tendi.local",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    );
    run_git(
        &repository,
        &["worktree", "add", "-b", "first", first.to_str().unwrap()],
    );
    run_git(
        &repository,
        &["worktree", "add", "-b", "second", second.to_str().unwrap()],
    );
    let first_paths = super::checkout_mutation_resource_paths(&first).unwrap();
    let second_paths = super::checkout_mutation_resource_paths(&second).unwrap();
    let common = canonical_resource_path(&repository.join(".git")).unwrap();
    for paths in [&first_paths, &second_paths] {
        assert!(
            paths
                .iter()
                .any(|path| canonical_resource_path(path).unwrap() == common)
        );
    }
    let namespace = root.join("locks");
    let held = ResourceLease::acquire_paths(&namespace, &first_paths).unwrap();
    let other_namespace = namespace.clone();
    let blocked = std::thread::spawn(move || {
        ResourceLease::try_acquire_paths(&other_namespace, &second_paths)
            .unwrap()
            .is_none()
    })
    .join()
    .unwrap();
    assert!(
        blocked,
        "separate worktrees must serialize mutations to common Git metadata"
    );
    drop(held);
    assert!(
        ResourceLease::try_acquire_paths(
            &namespace,
            &super::checkout_mutation_resource_paths(&second).unwrap()
        )
        .unwrap()
        .is_some()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backup_statuses_skip_all_skill_work_without_a_configured_repository() {
    let root = temp_dir("tendi-skill-backup-status-no-config");
    let skill = root.join("global/review");
    write_skill(&skill, "review");
    let store = Store::open(root.join("tendi.sqlite3")).unwrap();
    store
        .upsert_skill_source_records_for_workspace(&root, &[source("review", &skill)])
        .unwrap();

    let statuses = backup_statuses_for_paths(&store, &root, std::slice::from_ref(&skill)).unwrap();

    assert!(statuses.is_empty());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn catalog_source_items_use_file_titles_for_rules() {
    let items = catalog_source_files(
        vec![(
            AgentKind::Codex,
            PathBuf::from("/tmp/AGENTS.md"),
            "AGENTS.md".to_string(),
            String::new(),
        )],
        "rules",
    );

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "AGENTS.md");
    assert_eq!(items[0].detail, "");
}

#[test]
fn backup_catalog_keeps_same_name_installations_separate() {
    let root = temp_dir("tendi-skill-backup-catalog");
    let first = root.join("global/first-review");
    let second = root.join("global/second-review");
    write_skill(&first, "review");
    write_skill(&second, "review");
    let store = Store::open(root.join("tendi.sqlite3")).unwrap();
    let scan = SkillScan {
        roots: vec![SkillRoot {
            path: root.join("global"),
            scope: "global".to_string(),
            agent: AgentKind::Shared,
            plugin_id: None,
            plugin_enabled: None,
        }],
        skills: vec![
            skill_record("review", &first),
            skill_record("review", &second),
        ],
        warnings: Vec::new(),
    };
    store.save_skills_for_workspace(&root, &scan).unwrap();

    let catalog = backup_catalog(&store, &root).unwrap();
    assert_eq!(catalog.skills.len(), 2);
    assert_ne!(catalog.skills[0].id, catalog.skills[1].id);
    assert!(catalog.skills.iter().all(|item| item.label == "review"));
    assert!(
        catalog
            .skills
            .iter()
            .all(|item| item.detail.contains("global/"))
    );

    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn manifest_contains_only_managed_non_project_skills_and_snapshot_is_self_contained() {
    let root = temp_dir("tendi-skill-backup-manifest");
    let global = root.join("global/review");
    write_skill(&global, "review");
    fs::write(global.join("helper.ts"), "export const review = true;\n").unwrap();
    fs::create_dir_all(global.join("node_modules/package")).unwrap();
    fs::write(global.join("node_modules/package/index.js"), "ignored").unwrap();

    let project = root.join("project/.agents/skills/project-only");
    write_skill(&project, "project-only");
    run_git(&root.join("project"), &["init"]);

    let options = BackupBuildOptions {
        device_label: "Test Mac".to_string(),
    };
    let manifest = build_manifest(
        &[source("review", &global), source("project-only", &project)],
        &options,
    )
    .unwrap();

    assert_eq!(manifest.version, 1);
    assert_eq!(manifest.device_label, "Test Mac");
    assert_eq!(manifest.skills.len(), 1);
    assert_eq!(manifest.skills[0].name, "review");
    assert_eq!(manifest.excluded.len(), 1);
    assert_eq!(manifest.excluded[0].reason, "project-repository");

    let snapshot = root.join("snapshot");
    write_snapshot(&manifest, &snapshot).unwrap();
    assert!(snapshot.join("manifest.json").is_file());
    assert!(
        snapshot
            .join("skills")
            .join(&manifest.skills[0].id)
            .join("SKILL.md")
            .is_file()
    );
    assert!(
        snapshot
            .join("skills")
            .join(&manifest.skills[0].id)
            .join("helper.ts")
            .is_file()
    );
    assert!(
        !snapshot
            .join("skills")
            .join(&manifest.skills[0].id)
            .join("node_modules")
            .exists()
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sensitive_content_excludes_the_whole_skill() {
    let root = temp_dir("tendi-skill-backup-sensitive");
    let skill = root.join("global/deploy");
    write_skill(&skill, "deploy");
    fs::write(skill.join(".env"), "API_TOKEN=super-secret\n").unwrap();

    let manifest =
        build_manifest(&[source("deploy", &skill)], &BackupBuildOptions::default()).unwrap();

    assert!(manifest.skills.is_empty());
    assert_eq!(manifest.excluded[0].reason, "sensitive-content");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn token_scanner_blocks_real_tokens_without_rejecting_ordinary_skill_text() {
    let root = temp_dir("tendi-skill-backup-token-scanner");
    let ordinary = root.join("global/task-review");
    let sensitive = root.join("global/api-client");
    write_skill(&ordinary, "task-review");
    fs::write(
        ordinary.join("SKILL.md"),
        "---\nname: task-review\n---\nUse task-based review steps.\n",
    )
    .unwrap();
    write_skill(&sensitive, "api-client");
    fs::write(
        sensitive.join("token.txt"),
        "sk-proj-123456789012345678901234567890\n",
    )
    .unwrap();

    let manifest = build_manifest(
        &[
            source("task-review", &ordinary),
            source("api-client", &sensitive),
        ],
        &BackupBuildOptions::default(),
    )
    .unwrap();

    assert_eq!(manifest.skills.len(), 1);
    assert_eq!(manifest.skills[0].name, "task-review");
    assert_eq!(manifest.excluded[0].reason, "sensitive-content");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn matching_skill_content_keeps_independent_installations() {
    let root = temp_dir("tendi-skill-backup-canonical");
    let first = root.join("one/review");
    let second = root.join("two/review");
    let distinct = root.join("three/review");
    write_skill(&first, "review");
    write_skill(&second, "review");
    write_skill(&distinct, "review");
    fs::write(
        distinct.join("SKILL.md"),
        "---\nname: review\n---\n# changed\n",
    )
    .unwrap();

    let manifest = build_manifest(
        &[
            source("review", &first),
            source("review", &second),
            source("review", &distinct),
        ],
        &BackupBuildOptions::default(),
    )
    .unwrap();

    assert_eq!(manifest.skills.len(), 3);
    assert!(manifest.skills.iter().any(|skill| skill.id == "review"));
    assert!(
        manifest
            .skills
            .iter()
            .any(|skill| skill.id.starts_with("review-"))
    );
    let serialized = serde_json::to_string(&manifest).unwrap();
    assert!(!serialized.contains(&first.display().to_string()));
    assert!(!serialized.contains(&second.display().to_string()));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backup_config_rejects_a_remote_with_embedded_credentials() {
    let error = BackupConfig::new(
        "https://token@example.com/tendi-skills.git",
        PathBuf::from("/tmp/tendi-skill-backup"),
    )
    .validate()
    .unwrap_err();

    assert!(error.to_string().contains("credentials"));
}

#[test]
fn repository_input_distinguishes_remote_urls_from_local_paths() {
    assert!(is_remote_repository("git@github.com:rhinoc/skills.git"));
    assert!(is_remote_repository("rhinoc/skills"));
    assert!(is_remote_repository("github.com/rhinoc/skills"));
    assert!(is_remote_repository("https://github.com/rhinoc/skills.git"));
    assert!(!is_remote_repository("/tmp/tendi-skill-backup"));
    assert!(!is_remote_repository("./tendi-skill-backup"));
}

#[test]
fn local_checkout_is_created_and_keeps_commits_local_without_an_origin() {
    let root = temp_dir("tendi-backup-local-checkout");
    let checkout = root.join("nested/checkout");
    let config = BackupConfig::new("", checkout.clone());

    assert_eq!(discover_git_repository_root(&checkout).unwrap(), None);
    assert!(!ensure_checkout(&config).unwrap());
    assert!(checkout.join(".git").exists());

    let remote = Command::new("git")
        .args(["-C", checkout.to_str().unwrap(), "remote"])
        .output()
        .unwrap();
    assert!(remote.status.success());
    assert!(String::from_utf8_lossy(&remote.stdout).trim().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn local_backup_creates_a_commit_without_pushing() {
    let root = temp_dir("tendi-backup-local-only");
    let skill = root.join("global/review");
    let checkout = root.join("backup");
    write_skill(&skill, "review");
    let store = Store::open(root.join("tendi.sqlite3")).unwrap();
    store
        .upsert_skill_source_records_for_workspace(&root, &[source("review", &skill)])
        .unwrap();
    store
        .save_skill_backup_config(&BackupConfig::new("", checkout.clone()))
        .unwrap();

    let report = backup_now(&store, &root).unwrap();

    assert!(report.commit.is_some());
    assert!(!report.pushed);
    let machine_name = current_machine_name().unwrap();
    let commit = Command::new("git")
        .args([
            "-C",
            checkout.to_str().unwrap(),
            "log",
            "-1",
            "--format=%an|%s",
        ])
        .output()
        .unwrap();
    let commit_text = String::from_utf8_lossy(&commit.stdout);
    assert!(commit_text.contains(&format!("Tendi Backup ({machine_name})")));
    assert!(commit_text.contains(&format!("from {machine_name}")));
    assert_eq!(backup_versions(&store, 1).unwrap().len(), 1);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backup_remote_validation_rejects_an_unreachable_repository() {
    let root = temp_dir("tendi-backup-remote-validation");
    fs::create_dir_all(&root).unwrap();

    let error =
        super::validate_remote(&root.join("missing.git").display().to_string(), &root).unwrap_err();

    assert!(error.to_string().contains("not reachable"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backup_config_normalizes_github_remote_shorthands() {
    for remote in [
        "rhinoc/skills",
        "github.com/rhinoc/skills",
        "rhinoc/skills.git",
    ] {
        let config = BackupConfig::new(remote, PathBuf::from("/tmp/tendi-skill-backup"));

        assert_eq!(config.remote_url, "https://github.com/rhinoc/skills.git");
    }

    assert_eq!(
        normalize_remote_url("https://github.com/rhinoc/skills.git"),
        "https://github.com/rhinoc/skills.git"
    );
    assert_eq!(
        normalize_remote_url("git@github.com:rhinoc/skills.git"),
        "git@github.com:rhinoc/skills.git"
    );
    assert_eq!(normalize_remote_url("../skills"), "../skills");
}

#[test]
fn existing_backup_checkout_updates_a_github_shorthand_origin() {
    let root = temp_dir("tendi-backup-normalized-origin");
    let checkout = root.join("checkout");
    let config = BackupConfig {
        remote_url: "rhinoc/skills".to_string(),
        checkout_path: checkout.clone(),
        contents: Default::default(),
    };

    ensure_checkout(&config).unwrap();

    let remote = Command::new("git")
        .args([
            "-C",
            checkout.to_str().unwrap(),
            "remote",
            "get-url",
            "origin",
        ])
        .output()
        .unwrap();
    assert!(remote.status.success());
    assert_eq!(
        String::from_utf8_lossy(&remote.stdout).trim(),
        "https://github.com/rhinoc/skills.git"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backup_config_includes_all_global_categories_by_default() {
    let config = BackupConfig::new(
        "git@github.com:you/tendi-backup.git",
        PathBuf::from("/tmp/tendi-backup"),
    );

    assert!(config.contents.skills.enabled);
    assert!(config.contents.mcp.enabled);
    assert!(config.contents.rules.enabled);
    assert!(config.contents.hooks.enabled);
    assert!(config.contents.skills.excluded.is_empty());
}

#[test]
fn snapshot_writes_rules_files_under_category_roots() {
    let root = temp_dir("tendi-backup-category-roots");
    let source = root.join("AGENTS.md");
    let content = b"# Team rules\n";
    fs::create_dir_all(&root).unwrap();
    fs::write(&source, content).unwrap();
    let artifact_id = "rules-agents".to_string();
    let mut artifact_source_paths = std::collections::BTreeMap::new();
    artifact_source_paths.insert(format!("rules:{artifact_id}"), source);
    let manifest = BackupManifest {
        version: 1,
        device_label: "Test Mac".to_string(),
        skills: Vec::new(),
        artifacts: vec![super::BackupArtifact {
            id: artifact_id.clone(),
            category: "rules".to_string(),
            name: "AGENTS.md".to_string(),
            agent: "codex".to_string(),
            source_relative_path: "AGENTS.md".to_string(),
            entry_key: String::new(),
            entry_selector: Vec::new(),
            files: vec![super::BackupFile {
                path: "AGENTS.md".to_string(),
                sha256: super::sha256_hex(content),
                size: content.len() as u64,
            }],
        }],
        excluded: Vec::new(),
        source_paths: Default::default(),
        artifact_source_paths,
        artifact_contents: Default::default(),
    };
    let snapshot = root.join("snapshot");

    write_snapshot(&manifest, &snapshot).unwrap();

    assert!(
        snapshot
            .join("rules")
            .join(&artifact_id)
            .join("AGENTS.md")
            .is_file()
    );
    assert!(!snapshot.join("global").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn new_device_can_prepare_remote_checkout_for_restore() {
    let root = temp_dir("tendi-backup-new-device-restore");
    let skill = root.join("first/global/review");
    write_skill(&skill, "review");
    let remote = root.join("remote.git");
    fs::create_dir_all(&root).unwrap();
    run_git(&root, &["init", "--bare", "remote.git"]);

    let first_store = Store::open(root.join("first.sqlite3")).unwrap();
    first_store
        .upsert_skill_source_records_for_workspace(&root, &[source("review", &skill)])
        .unwrap();
    first_store
        .save_skill_backup_config(&BackupConfig::new(
            remote.display().to_string(),
            root.join("first-checkout"),
        ))
        .unwrap();
    backup_now(&first_store, &root).unwrap();

    let second_config =
        BackupConfig::new(remote.display().to_string(), root.join("second-checkout"));
    let manifest = sync_checkout_for_restore(&second_config).unwrap();

    assert_eq!(manifest.unwrap().skills[0].name, "review");
    assert!(second_config.checkout_path.join("manifest.json").is_file());
    drop(first_store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn restore_plan_applies_a_rules_file_artifact() {
    let root = temp_dir("tendi-backup-artifact-restore");
    fs::create_dir_all(&root).unwrap();
    let checkout = root.join("checkout");
    run_git(&root, &["init", "checkout"]);
    run_git(&checkout, &["config", "user.email", "test@example.com"]);
    run_git(&checkout, &["config", "user.name", "Tendi test"]);
    let source = root.join("AGENTS.md");
    let content = b"# Restored rules\n";
    fs::write(&source, content).unwrap();
    let artifact_id = "rules-agents".to_string();
    let mut artifact_source_paths = std::collections::BTreeMap::new();
    artifact_source_paths.insert(format!("rules:{artifact_id}"), source);
    let artifact = super::BackupArtifact {
        id: artifact_id.clone(),
        category: "rules".to_string(),
        name: "AGENTS.md".to_string(),
        agent: "codex".to_string(),
        source_relative_path: "AGENTS.md".to_string(),
        entry_key: String::new(),
        entry_selector: Vec::new(),
        files: vec![super::BackupFile {
            path: "AGENTS.md".to_string(),
            sha256: super::sha256_hex(content),
            size: content.len() as u64,
        }],
    };
    let manifest = BackupManifest {
        version: 1,
        device_label: "Test Mac".to_string(),
        skills: Vec::new(),
        artifacts: vec![artifact.clone()],
        excluded: Vec::new(),
        source_paths: Default::default(),
        artifact_source_paths,
        artifact_contents: Default::default(),
    };
    write_snapshot(&manifest, &checkout).unwrap();
    run_git(&checkout, &["add", "."]);
    run_git(&checkout, &["commit", "-m", "backup"]);
    let revision = super::current_commit(&checkout).unwrap();
    let target = root.join("restore/AGENTS.md");
    let plan = super::BackupRestorePlan {
        revision,
        target_root: root.join("restore"),
        operations: vec![super::BackupRestoreOperation {
            id: artifact_id.clone(),
            name: artifact.name.clone(),
            category: artifact.category.clone(),
            target: target.clone(),
            status: "planned".to_string(),
            message: None,
        }],
        checkout,
        skills: Default::default(),
        artifacts: std::collections::BTreeMap::from([(artifact_id, artifact)]),
    };

    let result = super::apply_backup_restore_without_database(&plan, &[]).unwrap();

    assert_eq!(result.operations[0].status, "restored");
    assert_eq!(fs::read(target).unwrap(), content);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn restore_plan_merges_one_mcp_entry_into_an_existing_config() {
    let root = temp_dir("tendi-backup-mcp-entry-restore");
    fs::create_dir_all(&root).unwrap();
    let checkout = root.join("checkout");
    run_git(&root, &["init", "checkout"]);
    run_git(&checkout, &["config", "user.email", "test@example.com"]);
    run_git(&checkout, &["config", "user.name", "Tendi test"]);
    let entry = br#"{
  "command": "restored"
}
"#;
    let artifact_id = "mcp-entry-demo".to_string();
    let artifact = super::BackupArtifact {
        id: artifact_id.clone(),
        category: "mcp".to_string(),
        name: "demo".to_string(),
        agent: "codex".to_string(),
        source_relative_path: "config.toml".to_string(),
        entry_key: "demo".to_string(),
        entry_selector: vec!["mcp_servers".to_string()],
        files: vec![super::BackupFile {
            path: "entry.json".to_string(),
            sha256: super::sha256_hex(entry),
            size: entry.len() as u64,
        }],
    };
    let manifest = BackupManifest {
        version: 1,
        device_label: "Test Mac".to_string(),
        skills: Vec::new(),
        artifacts: vec![artifact.clone()],
        excluded: Vec::new(),
        source_paths: Default::default(),
        artifact_source_paths: Default::default(),
        artifact_contents: std::collections::BTreeMap::from([(
            format!("mcp:{artifact_id}"),
            entry.to_vec(),
        )]),
    };
    write_snapshot(&manifest, &checkout).unwrap();
    run_git(&checkout, &["add", "."]);
    run_git(&checkout, &["commit", "-m", "sync"]);
    let revision = super::current_commit(&checkout).unwrap();
    let target = root.join("restore/config.toml");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(
        &target,
        "[mcp_servers.kept]\nurl = \"https://example.com/mcp\"\n",
    )
    .unwrap();
    let plan = super::BackupRestorePlan {
        revision,
        target_root: root.join("restore"),
        operations: vec![super::BackupRestoreOperation {
            id: artifact_id.clone(),
            name: artifact.name.clone(),
            category: artifact.category.clone(),
            target: target.clone(),
            status: "planned".to_string(),
            message: None,
        }],
        checkout,
        skills: Default::default(),
        artifacts: std::collections::BTreeMap::from([(artifact_id, artifact)]),
    };

    let result = super::apply_backup_restore_without_database(&plan, &[]).unwrap();
    let value = toml::from_str::<toml::Value>(&fs::read_to_string(&target).unwrap()).unwrap();

    assert_eq!(result.operations[0].status, "restored");
    assert_eq!(
        value["mcp_servers"]["demo"]["command"],
        toml::Value::String("restored".to_string())
    );
    assert_eq!(
        value["mcp_servers"]["kept"]["url"],
        toml::Value::String("https://example.com/mcp".to_string())
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn manifest_validation_rejects_paths_that_escape_a_skill_directory() {
    let manifest = BackupManifest {
        version: 1,
        device_label: "Test Mac".to_string(),
        skills: vec![super::BackupSkill {
            id: "demo".to_string(),
            name: "demo".to_string(),
            source: super::BackupSkillSource::default(),
            files: vec![super::BackupFile {
                path: "../outside".to_string(),
                sha256: "0".repeat(64),
                size: 0,
            }],
        }],
        artifacts: Vec::new(),
        excluded: Vec::new(),
        source_paths: Default::default(),
        artifact_source_paths: Default::default(),
        artifact_contents: Default::default(),
    };

    let error = validate_manifest(&manifest).unwrap_err();
    assert!(error.to_string().contains("file path"));
}

#[test]
fn backup_now_commits_a_self_contained_snapshot_to_the_configured_remote() {
    let root = temp_dir("tendi-skill-backup-git");
    let skill = root.join("global/review");
    write_skill(&skill, "review");
    let remote = root.join("remote.git");
    fs::create_dir_all(&root).unwrap();
    run_git(&root, &["init", "--bare", "remote.git"]);
    let checkout = root.join("checkout");
    let store = Store::open(root.join("tendi.sqlite3")).unwrap();
    store
        .upsert_skill_source_records_for_workspace(&root, &[source("review", &skill)])
        .unwrap();
    store
        .save_skill_backup_config(&BackupConfig::new(
            remote.display().to_string(),
            checkout.clone(),
        ))
        .unwrap();

    let report = backup_now(&store, &root).unwrap();

    assert!(report.pushed);
    assert_eq!(report.manifest.skills.len(), 1);
    assert!(checkout.join("manifest.json").is_file());
    let remote_manifest = Command::new("git")
        .args([
            "--git-dir",
            remote.to_str().unwrap(),
            "show",
            "main:manifest.json",
        ])
        .output()
        .unwrap();
    assert!(remote_manifest.status.success());
    assert!(String::from_utf8_lossy(&remote_manifest.stdout).contains("review"));
    assert_eq!(backup_versions(&store, 10).unwrap().len(), 1);

    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backup_from_another_device_keeps_remote_skills_that_are_not_locally_installed() {
    let root = temp_dir("tendi-skill-backup-merge");
    let review = root.join("first/review");
    let draft = root.join("second/draft");
    write_skill(&review, "review");
    write_skill(&draft, "draft");
    let remote = root.join("remote.git");
    fs::create_dir_all(&root).unwrap();
    run_git(&root, &["init", "--bare", "remote.git"]);

    let first_store = Store::open(root.join("first.sqlite3")).unwrap();
    first_store
        .upsert_skill_source_records_for_workspace(&root, &[source("review", &review)])
        .unwrap();
    first_store
        .save_skill_backup_config(&BackupConfig::new(
            remote.display().to_string(),
            root.join("first-checkout"),
        ))
        .unwrap();
    backup_now(&first_store, &root).unwrap();

    let second_store = Store::open(root.join("second.sqlite3")).unwrap();
    second_store
        .upsert_skill_source_records_for_workspace(&root, &[source("draft", &draft)])
        .unwrap();
    second_store
        .save_skill_backup_config(&BackupConfig::new(
            remote.display().to_string(),
            root.join("second-checkout"),
        ))
        .unwrap();
    let report = backup_now(&second_store, &root).unwrap();

    assert_eq!(report.manifest.skills.len(), 2);
    assert!(
        report
            .manifest
            .skills
            .iter()
            .any(|skill| skill.name == "review")
    );
    assert!(
        report
            .manifest
            .skills
            .iter()
            .any(|skill| skill.name == "draft")
    );
    let remote_manifest = Command::new("git")
        .args([
            "--git-dir",
            remote.to_str().unwrap(),
            "show",
            "main:manifest.json",
        ])
        .output()
        .unwrap();
    assert!(remote_manifest.status.success());
    let remote_text = String::from_utf8_lossy(&remote_manifest.stdout);
    assert!(remote_text.contains("review"));
    assert!(remote_text.contains("draft"));

    drop(second_store);
    drop(first_store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn diverged_devices_keep_each_skill_version_without_a_git_merge_conflict() {
    let root = temp_dir("tendi-skill-backup-diverged");
    let first_skill = root.join("first/review");
    let second_skill = root.join("second/review");
    write_skill(&first_skill, "review");
    write_skill(&second_skill, "review");
    fs::write(
        second_skill.join("SKILL.md"),
        "---\nname: review\n---\n# second device\n",
    )
    .unwrap();
    let remote = root.join("remote.git");
    fs::create_dir_all(&root).unwrap();
    run_git(&root, &["init", "--bare", "remote.git"]);

    let first_checkout = root.join("first-checkout");
    let first_config = BackupConfig::new(remote.display().to_string(), first_checkout.clone());
    let first_store = Store::open(root.join("first.sqlite3")).unwrap();
    first_store
        .upsert_skill_source_records_for_workspace(&root, &[source("review", &first_skill)])
        .unwrap();
    first_store.save_skill_backup_config(&first_config).unwrap();
    backup_now(&first_store, &root).unwrap();

    let second_store = Store::open(root.join("second.sqlite3")).unwrap();
    second_store
        .upsert_skill_source_records_for_workspace(&root, &[source("review", &second_skill)])
        .unwrap();
    second_store
        .save_skill_backup_config(&BackupConfig::new(
            remote.display().to_string(),
            root.join("second-checkout"),
        ))
        .unwrap();
    backup_now(&second_store, &root).unwrap();

    fs::write(
        first_skill.join("SKILL.md"),
        "---\nname: review\n---\n# first device\n",
    )
    .unwrap();
    let local_only = build_manifest(
        &first_store
            .skill_source_records_for_workspace(&root)
            .unwrap(),
        &BackupBuildOptions {
            device_label: "First Mac".to_string(),
        },
    )
    .unwrap();
    write_snapshot(&local_only, &first_checkout).unwrap();
    let machine_name = current_machine_name().unwrap();
    assert!(super::commit_checkout(&first_config, &local_only, &machine_name).unwrap());

    let report = backup_now(&first_store, &root).unwrap();

    assert!(report.pushed);
    assert_eq!(report.manifest.skills.len(), 3);
    assert!(
        report
            .manifest
            .skills
            .iter()
            .all(|skill| skill.name == "review")
    );
    assert!(!super::checkout_has_conflicts(&first_checkout));
    drop(second_store);
    drop(first_store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn adopting_a_project_skill_for_backup_is_refused() {
    let root = temp_dir("tendi-skill-backup-adopt");
    let skill = root.join("project/.agents/skills/demo");
    write_skill(&skill, "demo");
    run_git(&root.join("project"), &["init"]);
    let store = Store::open(root.join("tendi.sqlite3")).unwrap();

    let error = adopt_skill_for_backup(&store, &root, &skill, "demo").unwrap_err();

    assert!(error.to_string().contains("project-repository"));
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn restore_plan_materializes_a_selected_snapshot_into_the_user_selected_target() {
    let root = temp_dir("tendi-skill-backup-restore");
    let skill = root.join("global/review");
    write_skill(&skill, "review");
    let remote = root.join("remote.git");
    fs::create_dir_all(&root).unwrap();
    run_git(&root, &["init", "--bare", "remote.git"]);
    let store = Store::open(root.join("tendi.sqlite3")).unwrap();
    store
        .upsert_skill_source_records_for_workspace(&root, &[source("review", &skill)])
        .unwrap();
    store
        .save_skill_backup_config(&BackupConfig::new(
            remote.display().to_string(),
            root.join("checkout"),
        ))
        .unwrap();
    backup_now(&store, &root).unwrap();
    let mut versions = backup_versions(&store, 1).unwrap();
    let revision = versions.remove(0).id;

    let target: SkillTarget = "shared".parse().unwrap();
    let plan = super::plan_backup_restore(
        &store,
        &root.join("restore-workspace"),
        &revision,
        &[],
        &target,
        SkillInstallScope::Project,
    )
    .unwrap();
    assert_eq!(plan.operations[0].status, "planned");
    let operations = super::apply_backup_restore(&plan, &store, &root, &[]).unwrap();

    assert_eq!(operations[0].status, "restored");
    assert!(operations[0].target.join("SKILL.md").is_file());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn restore_keep_both_preserves_an_existing_skill_directory() {
    let root = temp_dir("tendi-skill-backup-keep-both");
    let skill = root.join("global/review");
    write_skill(&skill, "review");
    let remote = root.join("remote.git");
    fs::create_dir_all(&root).unwrap();
    run_git(&root, &["init", "--bare", "remote.git"]);
    let store = Store::open(root.join("tendi.sqlite3")).unwrap();
    store
        .upsert_skill_source_records_for_workspace(&root, &[source("review", &skill)])
        .unwrap();
    store
        .save_skill_backup_config(&BackupConfig::new(
            remote.display().to_string(),
            root.join("checkout"),
        ))
        .unwrap();
    backup_now(&store, &root).unwrap();
    let revision = backup_versions(&store, 1).unwrap().remove(0).id;

    let target: SkillTarget = "shared".parse().unwrap();
    let workspace = root.join("restore-workspace");
    let existing = workspace.join(".agents/skills/review");
    write_skill(&existing, "local-review");
    let plan = super::plan_backup_restore(
        &store,
        &workspace,
        &revision,
        &[],
        &target,
        SkillInstallScope::Project,
    )
    .unwrap();
    assert_eq!(plan.operations[0].status, "conflict");

    let operations = super::apply_backup_restore(
        &plan,
        &store,
        &workspace,
        &[super::BackupRestoreResolution {
            id: plan.operations[0].id.clone(),
            action: "keep-both".to_string(),
        }],
    )
    .unwrap();

    assert_eq!(
        fs::read_to_string(existing.join("SKILL.md")).unwrap(),
        "---\nname: local-review\n---\n# local-review\n"
    );
    assert_eq!(operations[0].status, "restored");
    assert!(operations[0].target.ends_with("review-restored"));
    assert!(operations[0].target.join("SKILL.md").is_file());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

fn source(name: &str, path: &Path) -> SkillSourceRecord {
    SkillSourceRecord {
        skill_name: name.to_string(),
        skill_path: path.to_path_buf(),
        source_kind: "local".to_string(),
        source: None,
        source_ref: None,
        source_version: None,
        source_relative_path: None,
        update_status: "local".to_string(),
        origin: "tendi-install".to_string(),
    }
}

fn skill_record(name: &str, path: &Path) -> SkillRecord {
    let id = format!("skill@path:{}", path.canonicalize().unwrap().display());
    SkillRecord {
        id: id.clone(),
        installation_id: id,
        name: name.to_string(),
        description: Some(format!("{name} description")),
        tags: Vec::new(),
        dependencies: Vec::new(),
        dependents: Vec::new(),
        dependency_ids: Vec::new(),
        dependent_ids: Vec::new(),
        is_wrapper: false,
        visibility: SkillVisibility::Auto,
        agents: vec![AgentKind::Shared],
        paths: vec![SkillPath {
            path: path.to_path_buf(),
            root: path.parent().unwrap().to_path_buf(),
            scope: "global".to_string(),
            agent: AgentKind::Shared,
            install_target: "shared".to_string(),
            source_kind: "local".to_string(),
            source: None,
            source_ref: None,
            source_version: None,
            source_relative_path: None,
            symlink_status: "direct".to_string(),
            update_status: "local".to_string(),
            sha256: String::new(),
            tags: Vec::new(),
            tendi_visibility: None,
            effective_visibility: SkillVisibility::Auto,
            provider_allow_implicit_invocation: None,
            provider_skill_enabled: None,
            provider_disable_model_invocation: None,
            plugin_id: None,
            plugin_enabled: None,
        }],
        source_summary: "local".to_string(),
        install_targets: vec!["shared".to_string()],
        update_status: "local".to_string(),
        is_system: false,
        ctime: None,
        mtime: None,
    }
}

fn write_skill(path: &Path, name: &str) {
    fs::create_dir_all(path).unwrap();
    fs::write(
        path.join("SKILL.md"),
        format!("---\nname: {name}\n---\n# {name}\n"),
    )
    .unwrap();
}

fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
}
