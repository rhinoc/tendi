use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::skill_targets::SkillInstallScope;
use serde_yaml::Value;

use super::{
    AgentKind, ChangeSet, FileChange, GitSkillVisibility, GitUpdateAction, GitUpdateFile,
    MarkdownDoc, MaterializedGitTarget, RegistryUpdatePlan, ResolvedAddSource,
    SkillDistributionMode, SkillDistributionPlan, SkillMergeIssue, SkillPath, SkillRecord,
    SkillScan, SkillSnapshot, SkillSnapshotFile, SkillSourceRecord, SkillSourceUpdate,
    SkillUpdatePlan, SkillUpdateReport, SkillVisibility, SkillWriteTransaction,
    WRAPPER_CATALOG_END, WRAPPER_CATALOG_START, apply_changes, apply_git_update,
    apply_skill_add_with_target_root, apply_skill_delete_plan, apply_skill_distribution_plan,
    apply_skill_update_plan_filesystem_transaction, apply_skill_update_plan_with_store,
    apply_update_files, build_skill_add_plan_with_target_root, check_skill_updates, copy_dir,
    create_symlink, discover_installable_skills, format_delete_plan, git_materialized_path_files,
    git_worktree_matches_revision, materialize_skill_dir_to_root, materialize_tendi_cache_links,
    materialized_remote_has_no_effective_changes, merge_file_maps, parse_add_source,
    parse_skill_file_references, plan_registry_update, plan_skill_add, plan_skill_delete_many,
    plan_skill_updates_many_for_scan_in_workspace_with_store_and_reports,
    plan_skill_visibility_at_path, prepare_skill_update_persistence,
    rehome_canonical_skill_and_relink_projections, render_wrapper_after, sanitize_skill_dir_name,
    scan_skills_without_source_database as scan_skills, select_update_path, sha256_file,
    sha256_text, skill_backup_exclusion_reason, tendi_state_root,
};

fn temp_dir(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[cfg(unix)]
#[test]
fn skill_write_materializes_read_only_source_at_observed_path() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = temp_dir("tendi-skill-write-materialize");
    let source = root.join("source/example");
    let target = root.join(".agents/skills/example");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: example\n---\n\nsource\n",
    )
    .unwrap();
    fs::write(source.join("references.md"), "reference\n").unwrap();
    symlink(&source, &target).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();

    let transaction = SkillWriteTransaction::prepare(std::slice::from_ref(&target)).unwrap();
    assert!(fs::symlink_metadata(&target).unwrap().is_dir());
    assert_eq!(
        fs::read_to_string(target.join("SKILL.md")).unwrap(),
        "---\nname: example\n---\n\nsource\n"
    );
    fs::write(target.join("SKILL.md"), "local\n").unwrap();
    transaction.commit();

    assert_eq!(
        fs::read_to_string(source.join("SKILL.md")).unwrap(),
        "---\nname: example\n---\n\nsource\n"
    );
    assert_eq!(
        fs::read_to_string(target.join("SKILL.md")).unwrap(),
        "local\n"
    );
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn skill_write_keeps_same_source_projections_shared() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = temp_dir("tendi-skill-write-shared-source");
    let source = root.join("source/example");
    let first = root.join(".agents/skills/example");
    let second = root.join(".claude/skills/example");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(first.parent().unwrap()).unwrap();
    fs::create_dir_all(second.parent().unwrap()).unwrap();
    fs::write(source.join("SKILL.md"), "source\n").unwrap();
    symlink(&source, &first).unwrap();
    symlink(&source, &second).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();

    let transaction = SkillWriteTransaction::prepare(&[first.clone(), second.clone()]).unwrap();
    assert!(fs::symlink_metadata(&first).unwrap().is_dir());
    assert!(
        fs::symlink_metadata(&second)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        second.canonicalize().unwrap(),
        first.canonicalize().unwrap()
    );
    fs::write(first.join("SKILL.md"), "local\n").unwrap();
    transaction.commit();

    assert_eq!(
        fs::read_to_string(second.join("SKILL.md")).unwrap(),
        "local\n"
    );
    assert_eq!(
        fs::read_to_string(source.join("SKILL.md")).unwrap(),
        "source\n"
    );
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn skill_created_time_survives_new_projections_and_directory_replacement() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = temp_dir("tendi-skill-created-time");
    let source = root.join("source/demo");
    let first = root.join(".agents/skills/demo");
    let second = root.join(".claude/skills/demo");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(first.parent().unwrap()).unwrap();
    fs::create_dir_all(second.parent().unwrap()).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: demo\ndescription: Original\n---\n\nOriginal\n",
    )
    .unwrap();
    symlink(&source, &first).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();

    let store = crate::storage::Store::open(root.join("tendi.sqlite3")).unwrap();
    let scan = super::scan_skills_with_source_store_for_projects_for_projection(&root, &store, &[])
        .unwrap();
    let created_at = scan
        .skills
        .iter()
        .find(|skill| skill.name == "demo")
        .unwrap()
        .ctime
        .clone()
        .expect("skill directory creation time is available");

    std::thread::sleep(std::time::Duration::from_millis(20));
    symlink(&source, &second).unwrap();
    let later_created_at = fs::symlink_metadata(&second)
        .unwrap()
        .created()
        .ok()
        .and_then(super::system_time_to_iso)
        .expect("provider projection creation time is available");
    assert!(created_at < later_created_at);
    let scan = super::scan_skills_with_source_store_for_projects_for_projection(&root, &store, &[])
        .unwrap();
    let skill = scan
        .skills
        .iter()
        .find(|skill| skill.name == "demo")
        .unwrap();
    assert_eq!(skill.ctime.as_deref(), Some(created_at.as_str()));
    assert_eq!(skill.paths.len(), 2);
    let cached_scan = scan.clone();

    drop(store);
    {
        let conn = rusqlite::Connection::open(root.join("tendi.sqlite3")).unwrap();
        conn.execute("DROP TABLE skill_installation_times", [])
            .unwrap();
        conn.pragma_update(None, "user_version", crate::storage::STORAGE_SCHEMA_VERSION)
            .unwrap();
    }
    let store = crate::storage::Store::open(root.join("tendi.sqlite3")).unwrap();

    let write = SkillWriteTransaction::prepare(&[first.clone(), second.clone()]).unwrap();
    write.commit();
    assert!(fs::symlink_metadata(&first).unwrap().is_dir());
    assert!(
        fs::symlink_metadata(&second)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let replacement = first.with_file_name("demo-update");
    fs::create_dir(&replacement).unwrap();
    fs::write(
        replacement.join("SKILL.md"),
        "---\nname: demo\ndescription: Updated\n---\n\nUpdated\n",
    )
    .unwrap();
    fs::remove_dir_all(&first).unwrap();
    fs::rename(&replacement, &first).unwrap();

    let scan = super::refresh_dirty_skill_projection(
        &root,
        &store,
        cached_scan,
        std::slice::from_ref(&first),
        true,
        &[],
    )
    .unwrap();
    let skill = scan
        .skills
        .iter()
        .find(|skill| skill.name == "demo")
        .unwrap();
    assert_eq!(skill.description.as_deref(), Some("Updated"));
    assert_eq!(skill.ctime.as_deref(), Some(created_at.as_str()));

    fs::remove_file(&second).unwrap();
    fs::remove_dir_all(&first).unwrap();
    store
        .delete_skill_sources_for_workspace(&root, &[first.clone(), second.clone()], &[])
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    symlink(&source, &first).unwrap();
    let scan = super::scan_skills_with_source_store_for_projects_for_projection(&root, &store, &[])
        .unwrap();
    let reinstalled = scan
        .skills
        .iter()
        .find(|skill| skill.name == "demo")
        .unwrap();
    assert_ne!(reinstalled.ctime.as_deref(), Some(created_at.as_str()));

    drop(store);
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn skill_write_rolls_back_materialized_source() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = temp_dir("tendi-skill-write-rollback");
    let source = root.join("source/example");
    let target = root.join(".agents/skills/example");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(source.join("SKILL.md"), "source\n").unwrap();
    symlink(&source, &target).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();

    let transaction = SkillWriteTransaction::prepare(std::slice::from_ref(&target)).unwrap();
    transaction.rollback().unwrap();

    let metadata = fs::symlink_metadata(&target).unwrap();
    assert!(metadata.file_type().is_symlink());
    assert_eq!(fs::read_link(&target).unwrap(), source);
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn skill_write_materializes_a_read_only_directory_without_a_symlink() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_dir("tendi-skill-write-directory");
    let target = root.join(".agents/skills/example");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("SKILL.md"), "source\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o555)).unwrap();

    let transaction = SkillWriteTransaction::prepare(std::slice::from_ref(&target)).unwrap();
    assert!(fs::symlink_metadata(&target).unwrap().is_dir());
    fs::write(target.join("SKILL.md"), "local\n").unwrap();
    transaction.commit();

    assert_eq!(
        fs::read_to_string(target.join("SKILL.md")).unwrap(),
        "local\n"
    );
    assert!(
        !fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .contains("tendi-original"))
    );
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn skill_update_plan_only_applies_when_it_has_actionable_changes() {
    let empty = SkillUpdatePlan {
        file_changes: ChangeSet {
            changes: Vec::new(),
        },
        git_updates: Vec::new(),
        skipped: Vec::new(),
        source_updates: Vec::new(),
        merge_issues: Vec::new(),
    };
    assert!(!empty.can_apply());

    let empty_git_action = GitUpdateAction {
        name: "demo".to_string(),
        skill_names: vec!["demo".to_string()],
        repo: PathBuf::from("repo"),
        source: "source".to_string(),
        source_ref: None,
        current_version: Some("old".to_string()),
        latest_version: Some("new".to_string()),
        diff: String::new(),
        files: Vec::new(),
        tendi_settings: Vec::new(),
        materialized_targets: vec![MaterializedGitTarget {
            name: "demo".to_string(),
            target: PathBuf::from("target"),
            agent: AgentKind::Shared,
            source_relative_path: Some("skills/demo/SKILL.md".to_string()),
            visibility: SkillVisibility::Auto,
            uses_shared_layout: false,
            files: Vec::new(),
        }],
    };
    let empty_git_plan = SkillUpdatePlan {
        file_changes: ChangeSet {
            changes: Vec::new(),
        },
        git_updates: vec![empty_git_action],
        skipped: Vec::new(),
        source_updates: Vec::new(),
        merge_issues: Vec::new(),
    };
    assert!(!empty_git_plan.can_apply());

    let actionable = SkillUpdatePlan {
        file_changes: ChangeSet {
            changes: vec![super::FileChange {
                path: PathBuf::from("SKILL.md"),
                before_sha256: None,
                before: None,
                after: "updated".to_string(),
            }],
        },
        git_updates: Vec::new(),
        skipped: Vec::new(),
        source_updates: Vec::new(),
        merge_issues: Vec::new(),
    };
    assert!(actionable.can_apply());
}

fn run_test_git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn git_worktree_matching_remote_revision_is_not_an_update() {
    let root = temp_dir("tendi-git-worktree-current-test");
    let repo = root.join("repo");
    let skill_dir = repo.join("skills/demo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: demo\ndescription: old\n---\nold\n",
    )
    .unwrap();
    run_test_git(&repo, &["init", "--quiet"]);
    run_test_git(&repo, &["config", "user.email", "tendi@example.test"]);
    run_test_git(&repo, &["config", "user.name", "Tendi Test"]);
    run_test_git(&repo, &["add", "."]);
    run_test_git(&repo, &["commit", "--quiet", "-m", "old"]);
    let old_revision = run_test_git(&repo, &["rev-parse", "HEAD"]);

    let incoming = "---\nname: demo\ndescription: new\n---\nnew\n";
    fs::write(skill_dir.join("SKILL.md"), incoming).unwrap();
    run_test_git(&repo, &["add", "."]);
    run_test_git(&repo, &["commit", "--quiet", "-m", "new"]);
    let remote_revision = run_test_git(&repo, &["rev-parse", "HEAD"]);

    run_test_git(&repo, &["reset", "--hard", old_revision.as_str()]);
    fs::write(skill_dir.join("SKILL.md"), incoming).unwrap();

    let mut path = test_skill_path(
        skill_dir.to_str().unwrap(),
        AgentKind::Shared,
        SkillVisibility::Auto,
        None,
    );
    path.source_relative_path = Some("skills/demo/SKILL.md".to_string());

    assert!(git_worktree_matches_revision(
        &repo,
        &path,
        &remote_revision
    ));

    fs::write(skill_dir.join("SKILL.md"), "local edit\n").unwrap();
    assert!(!git_worktree_matches_revision(
        &repo,
        &path,
        &remote_revision
    ));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn materialized_update_uses_matching_intermediate_source_revision() {
    let root = temp_dir("tendi-materialized-intermediate-base-test");
    let repo = root.join("repo");
    let source_skill = repo.join("skills/demo");
    let installed_skill = root.join("installed/demo");
    fs::create_dir_all(&source_skill).unwrap();
    fs::create_dir_all(source_skill.join("agents")).unwrap();
    fs::write(source_skill.join("SKILL.md"), "---\nname: demo\n---\nold\n").unwrap();
    fs::write(source_skill.join("guide.md"), "old guide\n").unwrap();
    fs::write(
        source_skill.join("agents/openai.yaml"),
        "policy:\n  allow_implicit_invocation: true\n",
    )
    .unwrap();
    run_test_git(&repo, &["init", "--quiet"]);
    run_test_git(&repo, &["config", "user.email", "tendi@example.test"]);
    run_test_git(&repo, &["config", "user.name", "Tendi Test"]);
    run_test_git(&repo, &["add", "."]);
    run_test_git(&repo, &["commit", "--quiet", "-m", "old"]);
    let old_revision = run_test_git(&repo, &["rev-parse", "HEAD"]);

    fs::write(
        source_skill.join("SKILL.md"),
        "---\nname: demo\n---\nintermediate\n",
    )
    .unwrap();
    fs::write(source_skill.join("guide.md"), "intermediate guide\n").unwrap();
    run_test_git(&repo, &["commit", "--quiet", "-am", "intermediate"]);
    let intermediate_revision = run_test_git(&repo, &["rev-parse", "HEAD"]);

    fs::write(
        source_skill.join("SKILL.md"),
        "---\nname: demo\n---\nlatest\n",
    )
    .unwrap();
    fs::write(source_skill.join("guide.md"), "latest guide\n").unwrap();
    run_test_git(&repo, &["commit", "--quiet", "-am", "latest"]);
    let latest_revision = run_test_git(&repo, &["rev-parse", "HEAD"]);

    fs::create_dir_all(&installed_skill).unwrap();
    fs::write(
        installed_skill.join("SKILL.md"),
        "---\nname: demo\ndisable-model-invocation: true\n---\nintermediate\n",
    )
    .unwrap();
    fs::write(installed_skill.join("guide.md"), "intermediate guide\n").unwrap();
    fs::create_dir_all(installed_skill.join("agents")).unwrap();
    fs::write(
        installed_skill.join("agents/openai.yaml"),
        "policy:\n  allow_implicit_invocation: false\n",
    )
    .unwrap();
    fs::write(installed_skill.join("local.md"), "local extra\n").unwrap();

    let mut path = test_skill_path(
        installed_skill.to_str().unwrap(),
        AgentKind::Shared,
        SkillVisibility::Auto,
        None,
    );
    path.source_version = Some(old_revision);
    path.source_relative_path = Some("skills/demo/SKILL.md".to_string());

    assert_eq!(
        super::materialized_source_revision_for_update(&repo, &path, &latest_revision),
        Some(intermediate_revision)
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn materialized_remote_change_check_ignores_unrelated_repository_commits() {
    let root = temp_dir("tendi-materialized-remote-change-test");
    let repo = root.join("repo");
    let skill_dir = repo.join("skills/demo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "---\nname: demo\n---\nold\n").unwrap();
    fs::write(repo.join("README.md"), "old\n").unwrap();
    run_test_git(&repo, &["init", "--quiet"]);
    run_test_git(&repo, &["config", "user.email", "tendi@example.test"]);
    run_test_git(&repo, &["config", "user.name", "Tendi Test"]);
    run_test_git(&repo, &["add", "."]);
    run_test_git(&repo, &["commit", "--quiet", "-m", "old"]);
    let old_revision = run_test_git(&repo, &["rev-parse", "HEAD"]);

    fs::write(repo.join("README.md"), "new\n").unwrap();
    run_test_git(&repo, &["commit", "--quiet", "-am", "unrelated"]);
    let latest_revision = run_test_git(&repo, &["rev-parse", "HEAD"]);

    let mut path = test_skill_path(
        skill_dir.to_str().unwrap(),
        AgentKind::Shared,
        SkillVisibility::Auto,
        None,
    );
    path.source_version = Some(old_revision);
    path.source_relative_path = Some("skills/demo/SKILL.md".to_string());

    assert_eq!(
        materialized_remote_has_no_effective_changes(&repo, &path, &latest_revision),
        Some(true)
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn three_way_merge_keeps_independent_local_and_remote_edits() {
    let merged = super::merge_text(
        Some("title: old\nbody: old\nfooter: old\n"),
        Some("title: local\nbody: old\nfooter: old\n"),
        Some("title: old\nbody: old\nfooter: remote\n"),
    );

    assert_eq!(merged.status, "merged");
    let content = merged.content.unwrap();
    assert!(content.contains("title: local"));
    assert!(content.contains("footer: remote"));
}

#[test]
fn three_way_merge_reports_same_region_conflicts() {
    let merged = super::merge_text(
        Some("title: old\n"),
        Some("title: local\n"),
        Some("title: remote\n"),
    );

    assert_eq!(merged.status, "conflict");
    let content = merged.content.unwrap();
    assert!(content.contains("<<<<<<< local"));
    assert!(content.contains(">>>>>>> remote"));
}

#[test]
fn three_way_merge_reports_multiple_conflict_regions_as_conflict() {
    let merged = super::merge_text(
        Some("a: old\nb: old\nc: old\nd: old\n"),
        Some("a: local\nb: old\nc: local\nd: old\n"),
        Some("a: remote\nb: old\nc: remote\nd: old\n"),
    );

    assert_eq!(merged.status, "conflict");
    assert!(merged.reason.is_none());
    let content = merged.content.unwrap();
    assert_eq!(content.matches("<<<<<<< local").count(), 2);
    assert_eq!(content.matches(">>>>>>> remote").count(), 2);
}

#[test]
fn update_application_refuses_a_stale_local_file() {
    let root = temp_dir("tendi-update-stale-file-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("SKILL.md");
    fs::write(&path, "edited-after-preview\n").unwrap();
    let file = GitUpdateFile {
        path: "SKILL.md".to_string(),
        resolution_key: "demo:SKILL.md".to_string(),
        before: "before\n".to_string(),
        base: "before\n".to_string(),
        incoming: "incoming\n".to_string(),
        after: "incoming\n".to_string(),
        before_bytes: Some(b"before\n".to_vec()),
        incoming_bytes: Some(b"incoming\n".to_vec()),
        after_bytes: None,
        before_exists: true,
        incoming_exists: true,
        after_exists: true,
        status: "remote".to_string(),
        reason: None,
    };

    let error = apply_update_files(&root, &[file]).unwrap_err();
    assert!(format!("{error:#}").contains("refusing to overwrite changed file"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "edited-after-preview\n");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn apply_changes_rolls_back_previous_files_when_a_later_change_is_stale() {
    let root = temp_dir("tendi-file-transaction-test");
    fs::create_dir_all(&root).unwrap();
    let first = root.join("first.md");
    let second = root.join("second.md");
    fs::write(&second, "changed-after-preview\n").unwrap();
    let changes = ChangeSet {
        changes: vec![
            super::FileChange {
                path: first.clone(),
                before_sha256: None,
                before: None,
                after: "new\n".to_string(),
            },
            super::FileChange {
                path: second.clone(),
                before_sha256: Some(sha256_text("before\n")),
                before: Some("before\n".to_string()),
                after: "after\n".to_string(),
            },
        ],
    };

    assert!(super::apply_changes(&changes).is_err());
    assert!(!first.exists());
    assert_eq!(
        fs::read_to_string(&second).unwrap(),
        "changed-after-preview\n"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn filesystem_transaction_rolls_back_when_persistence_becomes_stale() {
    let root = temp_dir("tendi-filesystem-transaction-persistence-test");
    let skill_dir = root.join("skills/demo");
    let skill_file = skill_dir.join("SKILL.md");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(&skill_file, "before\n").unwrap();
    let store = crate::storage::Store::open(root.join("tendi.sqlite3")).unwrap();
    let source = SkillSourceRecord {
        skill_name: "demo".to_string(),
        skill_path: skill_dir.clone(),
        source_kind: "github".to_string(),
        source: Some("https://github.com/example/demo.git".to_string()),
        source_ref: Some("main".to_string()),
        source_version: Some("version-a".to_string()),
        source_relative_path: Some("skills/demo".to_string()),
        update_status: "tracked".to_string(),
        origin: "test".to_string(),
    };
    store
        .upsert_skill_source_records(std::slice::from_ref(&source))
        .unwrap();
    let plan = SkillUpdatePlan {
        file_changes: ChangeSet {
            changes: vec![FileChange {
                path: skill_file.clone(),
                before_sha256: Some(sha256_text("before\n")),
                before: Some("before\n".to_string()),
                after: "after\n".to_string(),
            }],
        },
        git_updates: Vec::new(),
        skipped: Vec::new(),
        source_updates: vec![SkillSourceUpdate {
            skill_path: skill_dir.clone(),
            source_version: "version-b".to_string(),
        }],
        merge_issues: Vec::new(),
    };
    let persistence = prepare_skill_update_persistence(&store, &plan).unwrap();
    let filesystem = apply_skill_update_plan_filesystem_transaction(&plan).unwrap();
    let mut externally_updated = source;
    externally_updated.source_version = Some("version-c".to_string());
    store
        .upsert_skill_source_records(std::slice::from_ref(&externally_updated))
        .unwrap();
    let after_filesystem = prepare_skill_update_persistence(&store, &plan).unwrap();
    let error = store
        .persist_skill_update_persistence_checked(
            &persistence.expected_source_versions,
            &after_filesystem.source_records,
            &after_filesystem.snapshots,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("changed after the update preview")
    );
    filesystem.rollback_context().unwrap();
    assert_eq!(fs::read_to_string(skill_file).unwrap(), "before\n");
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_resources_remain_owned_until_filesystem_rollback_finishes() {
    let root = temp_dir("tendi-filesystem-resource-lifetime");
    let skill_dir = root.join("demo");
    let file = skill_dir.join("SKILL.md");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(&file, "before\n").unwrap();
    let plan = SkillUpdatePlan {
        file_changes: ChangeSet {
            changes: vec![super::FileChange {
                path: file.clone(),
                before: Some("before\n".into()),
                before_sha256: Some(sha256_text("before\n")),
                after: "applied\n".into(),
            }],
        },
        git_updates: Vec::new(),
        skipped: Vec::new(),
        source_updates: Vec::new(),
        merge_issues: Vec::new(),
    };
    let transaction = super::apply_skill_update_plan_filesystem_transaction(&plan).unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker_root = root.clone();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        done_tx
            .send(crate::files::save_skill_file(
                &worker_root,
                "demo",
                "SKILL.md",
                &sha256_text("before\n"),
                "edited\n",
                Some(&skill_dir),
            ))
            .unwrap();
    });
    started_rx.recv().unwrap();
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err()
    );
    transaction.rollback_context().unwrap();
    done_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
        .unwrap();
    worker.join().unwrap();
    assert_eq!(fs::read_to_string(file).unwrap(), "edited\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn projection_visibility_initialization_preserves_command_winner() {
    let root = temp_dir("tendi-visibility-initialization-race");
    let skill_dir = root.join("demo");
    fs::create_dir_all(&skill_dir).unwrap();
    let store = crate::storage::Store::open(root.join("test.sqlite3")).unwrap();
    // A scanner observed no row, then an explicit command published Off.
    let observed = vec![(skill_dir.clone(), SkillVisibility::Auto)];
    store
        .upsert_skill_visibilities_for_workspace(
            &root,
            &[(skill_dir.clone(), SkillVisibility::Off)],
        )
        .unwrap();
    store
        .initialize_skill_visibilities_for_workspace(&root, &observed)
        .unwrap();
    let scan = SkillScan {
        roots: Vec::new(),
        warnings: Vec::new(),
        skills: vec![test_skill("demo", "Demo", &skill_dir)],
    };
    let projected = super::apply_persisted_skill_visibilities(&store, &root, scan).unwrap();
    assert_eq!(projected.skills[0].visibility, SkillVisibility::Auto);
    assert_eq!(
        projected.skills[0].paths[0].effective_visibility,
        SkillVisibility::Auto
    );
    assert_eq!(
        projected.skills[0].paths[0].tendi_visibility,
        Some(SkillVisibility::Off)
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn global_skill_visibility_is_shared_across_workspace_scopes() {
    let root = temp_dir("tendi-global-visibility-scope");
    let child = root.join("apps/desktop");
    let global_skill = root.join("../global-skills/demo");
    fs::create_dir_all(&child).unwrap();
    let store = crate::storage::Store::open(root.join("test.sqlite3")).unwrap();

    store
        .upsert_skill_visibilities_for_workspace(
            &root,
            &[(global_skill.clone(), SkillVisibility::Manual)],
        )
        .unwrap();
    store
        .upsert_skill_visibilities_for_workspace(
            &child,
            &[(global_skill.clone(), SkillVisibility::Auto)],
        )
        .unwrap();

    assert_eq!(
        store
            .skill_visibilities_for_workspace(&root)
            .unwrap()
            .get(&global_skill.canonicalize().unwrap_or(global_skill.clone()))
            .copied(),
        Some(SkillVisibility::Auto)
    );
    assert_eq!(
        store
            .skill_visibilities_for_workspace(&child)
            .unwrap()
            .get(&global_skill.canonicalize().unwrap_or(global_skill.clone()))
            .copied(),
        Some(SkillVisibility::Auto)
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reconciliation_reloads_visibility_after_resource_admission() {
    let root = temp_dir("tendi-reconcile-resource-race");
    let skill_dir = root.join(".agents/skills/demo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "---\nname: demo\n---\n").unwrap();
    let database = root.join("test.sqlite3");
    let store = crate::storage::Store::open(&database).unwrap();
    store
        .upsert_skill_visibilities_for_workspace(
            &root,
            &[(skill_dir.clone(), SkillVisibility::Manual)],
        )
        .unwrap();
    let resources =
        crate::coordination::acquire_file_resources(std::slice::from_ref(&skill_dir)).unwrap();
    let skill = test_skill("demo", "Demo", &skill_dir);
    let scan = SkillScan {
        roots: Vec::new(),
        skills: vec![skill],
        warnings: Vec::new(),
    };
    let worker_root = root.clone();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let store = crate::storage::Store::open(&database).unwrap();
        started_tx.send(()).unwrap();
        done_tx
            .send(super::reconcile_skill_visibility_for_workspace(
                &store,
                &worker_root,
                scan,
                &[],
            ))
            .unwrap();
    });
    started_rx.recv().unwrap();
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err()
    );
    store
        .upsert_skill_visibilities_for_workspace(&root, &[(skill_dir, SkillVisibility::Off)])
        .unwrap();
    drop(resources);
    let scan = done_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
        .unwrap();
    assert_eq!(scan.skills[0].visibility, SkillVisibility::Off);
    worker.join().unwrap();
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_provider_policy_does_not_block_reconciliation() {
    let root = temp_dir("tendi-reconcile-malformed-provider-policy");
    let skill_dir = root.join("demo");
    fs::create_dir_all(skill_dir.join("agents")).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "---\nname: demo\n---\n").unwrap();
    let policy = skill_dir.join("agents/openai.yaml");
    fs::write(&policy, "- provider-format-from-future\n").unwrap();
    let store = crate::storage::Store::open(root.join("test.sqlite3")).unwrap();
    store
        .upsert_skill_visibilities_for_workspace(
            &root,
            &[(skill_dir.clone(), SkillVisibility::Manual)],
        )
        .unwrap();
    let mut skill = test_skill("demo", "Demo", &skill_dir);
    skill.agents = vec![AgentKind::Codex];
    skill.paths[0].agent = AgentKind::Codex;
    skill.paths[0].effective_visibility = SkillVisibility::Manual;
    let scan = SkillScan {
        roots: Vec::new(),
        skills: vec![skill],
        warnings: Vec::new(),
    };

    let reconciled =
        super::reconcile_skill_visibility_for_workspace(&store, &root, scan, &[]).unwrap();

    assert_eq!(
        fs::read_to_string(policy).unwrap(),
        "- provider-format-from-future\n"
    );
    assert_eq!(reconciled.skills.len(), 1);
    assert!(
        reconciled
            .warnings
            .iter()
            .any(|warning| warning.contains("provider visibility sync skipped"))
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn no_op_reconciliation_does_not_advance_projection_revision() {
    let root = temp_dir("tendi-reconcile-no-op");
    let skill_dir = root.join("demo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "---\nname: demo\n---\n").unwrap();
    let store = crate::storage::Store::open(root.join("test.sqlite3")).unwrap();
    let mut skill = test_skill("demo", "Demo", &skill_dir);
    skill.paths[0].agent = AgentKind::Unknown;
    let scan = SkillScan {
        roots: Vec::new(),
        skills: vec![skill],
        warnings: Vec::new(),
    };
    assert!(
        store
            .save_skills_for_workspace_if_revision(&root, &scan, crate::Revision::ZERO)
            .unwrap()
    );
    store
        .invalidate_projection_resources("skills", &root, std::slice::from_ref(&skill_dir), true)
        .unwrap();
    let receipt = store
        .read_projection_refresh_state::<SkillScan>("skills", &root)
        .unwrap();
    assert!(!receipt.reconcile_full);
    assert!(!receipt.reconcile_resources.is_empty());
    let captured = receipt.revision;
    super::reconcile_dirty_skill_resources(
        &root,
        &store,
        receipt.snapshot.unwrap(),
        &receipt.reconcile_resources,
        receipt.reconcile_full,
    )
    .unwrap();
    let current = store
        .projection_head(
            &crate::storage::workspace_scope_key(&root).unwrap(),
            "skills",
        )
        .unwrap()
        .unwrap();
    assert_eq!(current.revision, captured);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn git_update_files_roll_back_previous_files_when_a_later_change_is_stale() {
    let root = temp_dir("tendi-git-update-transaction-test");
    fs::create_dir_all(&root).unwrap();
    let first = root.join("first.md");
    let second = root.join("second.md");
    fs::write(&second, "changed-after-preview\n").unwrap();
    let files = vec![
        GitUpdateFile {
            path: "first.md".to_string(),
            resolution_key: "first".to_string(),
            before: String::new(),
            base: String::new(),
            incoming: "new\n".to_string(),
            after: "new\n".to_string(),
            before_bytes: None,
            incoming_bytes: Some(b"new\n".to_vec()),
            after_bytes: None,
            before_exists: false,
            incoming_exists: true,
            after_exists: true,
            status: "remote".to_string(),
            reason: None,
        },
        GitUpdateFile {
            path: "second.md".to_string(),
            resolution_key: "second".to_string(),
            before: "before\n".to_string(),
            base: "before\n".to_string(),
            incoming: "after\n".to_string(),
            after: "after\n".to_string(),
            before_bytes: Some(b"before\n".to_vec()),
            incoming_bytes: Some(b"after\n".to_vec()),
            after_bytes: None,
            before_exists: true,
            incoming_exists: true,
            after_exists: true,
            status: "remote".to_string(),
            reason: None,
        },
    ];

    assert!(apply_update_files(&root, &files).is_err());
    assert!(!first.exists());
    assert_eq!(
        fs::read_to_string(&second).unwrap(),
        "changed-after-preview\n"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn binary_resolution_writes_selected_bytes() {
    let root = temp_dir("tendi-binary-resolution-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("asset.bin");
    let local = vec![0_u8, 255, 1];
    let incoming = vec![2_u8, 254, 3];
    fs::write(&path, &local).unwrap();
    let mut file = GitUpdateFile {
        path: "asset.bin".to_string(),
        resolution_key: "demo:asset.bin".to_string(),
        before: String::new(),
        base: String::new(),
        incoming: String::new(),
        after: String::new(),
        before_bytes: Some(local.clone()),
        incoming_bytes: Some(incoming.clone()),
        after_bytes: None,
        before_exists: true,
        incoming_exists: true,
        after_exists: false,
        status: "binary".to_string(),
        reason: None,
    };
    super::resolve_update_file(
        &mut file,
        &BTreeMap::from([(
            "demo:asset.bin".to_string(),
            super::USE_UPDATE_RESOLUTION.to_string(),
        )]),
    );
    apply_update_files(&root, &[file]).unwrap();
    assert_eq!(fs::read(&path).unwrap(), incoming);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn file_resolution_can_accept_update_deletion() {
    let root = temp_dir("tendi-file-deletion-resolution-test");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("deleted.md");
    let local = b"local\n".to_vec();
    fs::write(&path, &local).unwrap();
    let mut file = GitUpdateFile {
        path: "deleted.md".to_string(),
        resolution_key: "demo:deleted.md".to_string(),
        before: "local\n".to_string(),
        base: String::new(),
        incoming: String::new(),
        after: String::new(),
        before_bytes: Some(local),
        incoming_bytes: None,
        after_bytes: None,
        before_exists: true,
        incoming_exists: false,
        after_exists: false,
        status: "unavailable".to_string(),
        reason: Some("reason".to_string()),
    };
    super::resolve_update_file(
        &mut file,
        &BTreeMap::from([(
            "demo:deleted.md".to_string(),
            super::USE_UPDATE_RESOLUTION.to_string(),
        )]),
    );

    assert_eq!(file.status, "resolved-remote");
    assert!(!file.after_exists);
    apply_update_files(&root, &[file]).unwrap();
    assert!(!path.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn merge_issue_resolution_uses_selected_side_content() {
    let incoming = "incoming\n".to_string();
    let plan = SkillUpdatePlan {
        file_changes: ChangeSet {
            changes: Vec::new(),
        },
        git_updates: Vec::new(),
        skipped: Vec::new(),
        source_updates: Vec::new(),
        merge_issues: vec![SkillMergeIssue {
            name: "demo".to_string(),
            path: PathBuf::from("demo/SKILL.md"),
            resolution_key: "demo:SKILL.md".to_string(),
            status: "unavailable".to_string(),
            reason: Some("reason".to_string()),
            before: "local\n".to_string(),
            base: String::new(),
            incoming: incoming.clone(),
            after: String::new(),
        }],
    };

    let resolved = super::prepare_skill_update_plan_with_resolutions(
        &plan,
        &BTreeMap::from([(
            "demo:SKILL.md".to_string(),
            super::USE_UPDATE_RESOLUTION.to_string(),
        )]),
    )
    .unwrap();

    assert!(resolved.merge_issues.is_empty());
    assert_eq!(resolved.file_changes.changes[0].after, incoming);
}

#[test]
fn parse_add_source_accepts_local_github_and_git_sources() {
    let root = temp_dir("tendi-parse-add-source-test");
    fs::create_dir_all(&root).unwrap();
    let local = root.join("skills");
    fs::create_dir_all(&local).unwrap();

    let parsed_local = parse_add_source(&root, "skills").unwrap();
    assert_eq!(parsed_local.kind, "local");
    assert_eq!(parsed_local.root, Some(local.canonicalize().unwrap()));

    let parsed_shorthand = parse_add_source(&root, "vercel-labs/agent-skills").unwrap();
    assert_eq!(parsed_shorthand.kind, "github");
    assert_eq!(
        parsed_shorthand.url,
        "https://github.com/vercel-labs/agent-skills.git"
    );

    let parsed_git =
        parse_add_source(&root, "git@github.com:vercel-labs/agent-skills.git").unwrap();
    assert_eq!(parsed_git.kind, "github");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn git_clone_checks_out_requested_ref() {
    let root = temp_dir("tendi-clone-ref-test");
    let repository = root.join("repository");
    let checkout = root.join("checkout");
    fs::create_dir_all(&repository).unwrap();
    run_test_git(&repository, &["init", "-b", "main"]);
    run_test_git(&repository, &["config", "user.email", "test@example.com"]);
    run_test_git(&repository, &["config", "user.name", "Test"]);
    fs::write(repository.join("branch.txt"), "main").unwrap();
    fs::create_dir_all(repository.join("skills/demo")).unwrap();
    fs::write(
        repository.join("skills/demo/SKILL.md"),
        "---\nname: demo\ndescription: Demo\n---\n",
    )
    .unwrap();
    run_test_git(&repository, &["add", "."]);
    run_test_git(&repository, &["commit", "-m", "main"]);
    run_test_git(&repository, &["checkout", "-b", "release"]);
    fs::write(repository.join("branch.txt"), "release").unwrap();
    run_test_git(&repository, &["commit", "-am", "release"]);

    super::run_git_clone(
        repository.to_str().unwrap(),
        Some("release"),
        &checkout,
        super::git::never_cancelled(),
    )
    .unwrap();

    assert_eq!(
        fs::read_to_string(checkout.join("branch.txt")).unwrap(),
        "release"
    );
    assert_eq!(
        run_test_git(&checkout, &["branch", "--show-current"]),
        "release"
    );
    let source_root = checkout.join("skills/demo");
    let installed = root.join("installed/demo");
    let report = super::SkillAddApplyReport {
        plan: super::SkillAddPlan {
            source: "https://github.com/example/repo.git".to_string(),
            source_kind: "github".to_string(),
            source_ref: Some("release".to_string()),
            source_root: source_root.clone(),
            target: AgentKind::Shared.into(),
            scope: SkillInstallScope::Project,
            mode: "copy".to_string(),
            available: Vec::new(),
            selected: vec![super::InstallableSkill {
                name: "demo".to_string(),
                description: None,
                path: source_root,
                relative_path: String::new(),
                dependencies: Vec::new(),
            }],
            operations: Vec::new(),
        },
        results: vec![super::MaterializeResult {
            source: checkout.join("skills/demo"),
            target: installed,
            mode: "copy".to_string(),
            health: "copy-ok".to_string(),
            applied: true,
        }],
    };
    let records = super::skill_source_records_for_add(&report);
    assert_eq!(
        records[0].source_relative_path.as_deref(),
        Some("skills/demo")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn resolve_add_source_refreshes_existing_persistent_git_checkout() {
    let root = temp_dir("tendi-refresh-persistent-source-test");
    let repository = root.join("repository");
    let remote = root.join("remote.git");
    fs::create_dir_all(&repository).unwrap();
    fs::create_dir_all(&remote).unwrap();
    run_test_git(&repository, &["init", "-b", "main"]);
    run_test_git(&repository, &["config", "user.email", "test@example.com"]);
    run_test_git(&repository, &["config", "user.name", "Test"]);
    run_test_git(&remote, &["init", "--bare", "-b", "main"]);
    run_test_git(
        &repository,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    fs::create_dir_all(repository.join("skills/demo")).unwrap();
    fs::write(repository.join("skills/demo/SKILL.md"), "version one\n").unwrap();
    run_test_git(&repository, &["add", "."]);
    run_test_git(&repository, &["commit", "-m", "version one"]);
    run_test_git(&repository, &["push", "--quiet", "-u", "origin", "main"]);

    let source = format!("file://{}", remote.display());
    let first = super::resolve_add_source(&root, &source, true).unwrap();
    let cached_source = super::persistent_source_root(&source).unwrap();
    assert_eq!(
        fs::read_to_string(first.root.join("skills/demo/SKILL.md")).unwrap(),
        "version one\n"
    );

    fs::write(repository.join("skills/demo/SKILL.md"), "version two\n").unwrap();
    run_test_git(&repository, &["add", "."]);
    run_test_git(&repository, &["commit", "-m", "version two"]);
    run_test_git(&repository, &["push", "--quiet", "origin", "main"]);
    let latest = run_test_git(&repository, &["rev-parse", "HEAD"]);

    let second = super::resolve_add_source(&root, &source, true).unwrap();
    assert_eq!(second.root, first.root);
    assert_eq!(
        fs::read_to_string(second.root.join("skills/demo/SKILL.md")).unwrap(),
        "version two\n"
    );
    assert_eq!(run_test_git(&cached_source, &["rev-parse", "HEAD"]), latest);

    let _ = fs::remove_dir_all(cached_source);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn git_remote_commit_fetch_keeps_shallow_repository_shallow() {
    let root = temp_dir("tendi-shallow-fetch-test");
    let repository = root.join("repository");
    let checkout = root.join("checkout");
    fs::create_dir_all(&repository).unwrap();
    run_test_git(&repository, &["init", "-b", "main"]);
    run_test_git(&repository, &["config", "user.email", "test@example.com"]);
    run_test_git(&repository, &["config", "user.name", "Test"]);
    fs::write(repository.join("skill.md"), "before\n").unwrap();
    run_test_git(&repository, &["add", "."]);
    run_test_git(&repository, &["commit", "-m", "before"]);

    let source = format!("file://{}", repository.display());
    super::run_git_clone(
        &source,
        Some("main"),
        &checkout,
        super::git::never_cancelled(),
    )
    .unwrap();
    fs::write(repository.join("skill.md"), "after\n").unwrap();
    run_test_git(&repository, &["commit", "-am", "after"]);
    let latest = run_test_git(&repository, &["rev-parse", "HEAD"]);
    let reference = "refs/tendi/test-shallow-fetch";

    assert!(super::fetch_git_remote_commit(
        &checkout,
        &source,
        &latest,
        reference,
        super::git::never_cancelled(),
    ));
    assert_eq!(run_test_git(&checkout, &["rev-parse", reference]), latest);
    assert_eq!(
        run_test_git(&checkout, &["rev-parse", "--is-shallow-repository"]),
        "true"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn shallow_repository_fetches_missing_recorded_revision() {
    let root = temp_dir("tendi-shallow-base-fetch-test");
    let repository = root.join("repository");
    let checkout = root.join("checkout");
    fs::create_dir_all(repository.join("skills/demo")).unwrap();
    run_test_git(&repository, &["init", "-b", "main"]);
    run_test_git(&repository, &["config", "user.email", "test@example.com"]);
    run_test_git(&repository, &["config", "user.name", "Test"]);
    fs::write(repository.join("skills/demo/SKILL.md"), "before\n").unwrap();
    run_test_git(&repository, &["add", "."]);
    run_test_git(&repository, &["commit", "-m", "before"]);
    let recorded_revision = run_test_git(&repository, &["rev-parse", "HEAD"]);
    fs::write(repository.join("skills/demo/SKILL.md"), "after\n").unwrap();
    run_test_git(&repository, &["commit", "-am", "after"]);

    let source = format!("file://{}", repository.display());
    super::run_git_clone(
        &source,
        Some("main"),
        &checkout,
        super::git::never_cancelled(),
    )
    .unwrap();
    assert_eq!(
        run_test_git(&checkout, &["rev-parse", "--is-shallow-repository"]),
        "true"
    );
    assert!(!super::git_object_available(&checkout, &recorded_revision));

    assert!(super::ensure_git_object_available(
        &checkout,
        &source,
        &recorded_revision,
        super::git::never_cancelled(),
    ));
    let files =
        super::git_files_at_source_version(&checkout, &recorded_revision, "skills/demo", false)
            .unwrap();
    assert_eq!(
        files.get("skills/demo/SKILL.md"),
        Some(&b"before\n".to_vec())
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shallow_repository_fetches_missing_abbreviated_recorded_revision() {
    let root = temp_dir("tendi-shallow-abbreviated-base-fetch-test");
    let repository = root.join("repository");
    let checkout = root.join("checkout");
    fs::create_dir_all(repository.join("skills/demo")).unwrap();
    run_test_git(&repository, &["init", "-b", "main"]);
    run_test_git(&repository, &["config", "user.email", "test@example.com"]);
    run_test_git(&repository, &["config", "user.name", "Test"]);
    fs::write(repository.join("skills/demo/SKILL.md"), "older\n").unwrap();
    run_test_git(&repository, &["add", "."]);
    run_test_git(&repository, &["commit", "-m", "older"]);
    fs::write(repository.join("skills/demo/SKILL.md"), "before\n").unwrap();
    run_test_git(&repository, &["commit", "-am", "before"]);
    let recorded_revision = run_test_git(&repository, &["rev-parse", "HEAD"]);
    for (message, content) in [
        ("middle-one", "middle-one\n"),
        ("middle-two", "middle-two\n"),
        ("after", "after\n"),
    ] {
        fs::write(repository.join("skills/demo/SKILL.md"), content).unwrap();
        run_test_git(&repository, &["commit", "-am", message]);
    }

    let source = format!("file://{}", repository.display());
    super::run_git_clone(
        &source,
        Some("main"),
        &checkout,
        super::git::never_cancelled(),
    )
    .unwrap();
    let abbreviated_revision = &recorded_revision[..7];
    assert!(!super::git_object_available(
        &checkout,
        abbreviated_revision
    ));

    assert!(super::ensure_git_object_available_for_source_ref(
        &checkout,
        &source,
        Some("main"),
        abbreviated_revision,
        super::git::never_cancelled(),
    ));
    assert!(super::git_object_available(&checkout, &recorded_revision));
    let files =
        super::git_files_at_source_version(&checkout, abbreviated_revision, "skills/demo", false)
            .unwrap();
    assert_eq!(
        files.get("skills/demo/SKILL.md"),
        Some(&b"before\n".to_vec())
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn shallow_repository_fetches_missing_materialized_tree() {
    let root = temp_dir("tendi-shallow-tree-fetch-test");
    let repository = root.join("repository");
    let checkout = root.join("checkout");
    fs::create_dir_all(repository.join("skills/demo")).unwrap();
    run_test_git(&repository, &["init", "-b", "main"]);
    run_test_git(&repository, &["config", "user.email", "test@example.com"]);
    run_test_git(&repository, &["config", "user.name", "Test"]);
    fs::write(repository.join("skills/demo/SKILL.md"), "before\n").unwrap();
    run_test_git(&repository, &["add", "."]);
    run_test_git(&repository, &["commit", "-m", "before"]);
    let recorded_tree = run_test_git(&repository, &["rev-parse", "HEAD:skills/demo"]);
    fs::write(repository.join("skills/demo/SKILL.md"), "after\n").unwrap();
    run_test_git(&repository, &["commit", "-am", "after"]);

    let source = format!("file://{}", repository.display());
    super::run_git_clone(
        &source,
        Some("main"),
        &checkout,
        super::git::never_cancelled(),
    )
    .unwrap();
    assert!(!super::git_object_available(&checkout, &recorded_tree));
    assert!(super::ensure_git_object_available(
        &checkout,
        &source,
        &recorded_tree,
        super::git::never_cancelled(),
    ));
    let files =
        super::git_files_at_source_version(&checkout, &recorded_tree, "skills/demo", true).unwrap();
    assert_eq!(
        files.get("skills/demo/SKILL.md"),
        Some(&b"before\n".to_vec())
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn materialized_git_preview_compares_installed_files_with_remote_tree() {
    let root = temp_dir("tendi-materialized-git-preview-test");
    let repository = root.join("repository");
    let installed = root.join("installed/demo");
    fs::create_dir_all(repository.join("skills/demo/agents")).unwrap();
    fs::create_dir_all(installed.join("agents")).unwrap();
    run_test_git(&repository, &["init", "-b", "main"]);
    run_test_git(&repository, &["config", "user.email", "test@example.com"]);
    run_test_git(&repository, &["config", "user.name", "Test"]);
    fs::write(
        repository.join("skills/demo/SKILL.md"),
        "---\nname: demo\n---\n\nnew\n",
    )
    .unwrap();
    fs::write(
        repository.join("skills/demo/agents/openai.yaml"),
        "new-policy\n",
    )
    .unwrap();
    run_test_git(&repository, &["add", "."]);
    run_test_git(&repository, &["commit", "-m", "new"]);

    fs::write(installed.join("SKILL.md"), "---\nname: demo\n---\n\nold\n").unwrap();
    fs::write(installed.join("local.txt"), "removed\n").unwrap();

    let files = git_materialized_path_files(&repository, &installed, "skills/demo", "HEAD");
    let by_path = files
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        by_path["skills/demo/SKILL.md"].before,
        "---\nname: demo\n---\n\nold\n"
    );
    assert_eq!(
        by_path["skills/demo/SKILL.md"].after,
        "---\nname: demo\n---\n\nnew\n"
    );
    assert_eq!(by_path["skills/demo/agents/openai.yaml"].before, "");
    assert_eq!(
        by_path["skills/demo/agents/openai.yaml"].after,
        "new-policy\n"
    );
    assert_eq!(by_path["skills/demo/local.txt"].before, "removed\n");
    assert_eq!(by_path["skills/demo/local.txt"].after, "");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn discover_installable_skills_finds_catalog_and_agent_dirs() {
    let root = temp_dir("tendi-discover-add-skills-test");
    let alpha = root.join("skills/frontend/alpha");
    let beta = root.join(".codex/skills/beta");
    fs::create_dir_all(&alpha).unwrap();
    fs::create_dir_all(&beta).unwrap();
    fs::write(
        alpha.join("SKILL.md"),
        "---\nname: alpha\ndescription: Alpha skill\n---\n\n# alpha\n",
    )
    .unwrap();
    fs::write(
        beta.join("SKILL.md"),
        "---\nname: beta\ndescription: Beta skill\n---\n\n# beta\n",
    )
    .unwrap();

    let skills = discover_installable_skills(&root).unwrap();
    let names = skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["alpha", "beta"]);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn discover_installable_skills_respects_repository_gitignore() {
    let root = temp_dir("tendi-discover-add-skills-gitignore-test");
    let ignored = root.join(".build/checkouts/ignored");
    let visible = root.join("other/visible");
    fs::create_dir_all(&ignored).unwrap();
    fs::create_dir_all(&visible).unwrap();
    run_test_git(&root, &["init", "--quiet"]);
    fs::write(root.join(".gitignore"), ".build/\n").unwrap();
    fs::write(
        ignored.join("SKILL.md"),
        "---\nname: ignored\ndescription: Ignored skill\n---\n\n# ignored\n",
    )
    .unwrap();
    fs::write(
        visible.join("SKILL.md"),
        "---\nname: visible\ndescription: Visible skill\n---\n\n# visible\n",
    )
    .unwrap();

    let skills = discover_installable_skills(&root).unwrap();
    let names = skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["visible"]);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn add_plan_selects_named_local_skills() {
    let root = temp_dir("tendi-plan-add-skills-test");
    let repo = root.join("repo");
    let skill_dir = repo.join("skills/demo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: demo\ndescription: Demo skill\n---\n\n# demo\n",
    )
    .unwrap();
    fs::write(skill_dir.join("Archive.zip"), [0_u8, 0xff, 0x00]).unwrap();

    let plan = plan_skill_add(
        &root,
        &super::SkillAddOptions {
            source: "repo".to_string(),
            target: AgentKind::Shared.into(),
            scope: SkillInstallScope::Global,
            skills: vec!["demo".to_string()],
            copy: true,
            overwrite: false,
            visibility: SkillVisibility::Auto,
        },
    )
    .unwrap();
    assert_eq!(plan.selected.len(), 1);
    assert_eq!(plan.operations[0].name, "demo");
    assert_eq!(plan.operations[0].mode, "copy");
    assert_eq!(plan.operations[0].status, "planned");
    assert!(super::skill_add_catalog_fingerprint(&plan).is_ok());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn apply_add_sets_selected_visibility_on_installed_skill() {
    let root = temp_dir("tendi-apply-add-visibility-test");
    let repo = root.join("repo");
    let target_root = root.join("installed");
    let skill_dir = repo.join("skills/demo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: demo\ndescription: Demo skill\n---\n\n# demo\n",
    )
    .unwrap();

    let report = apply_skill_add_with_target_root(
        &root,
        &super::SkillAddOptions {
            source: "repo".to_string(),
            target: AgentKind::Cursor.into(),
            scope: SkillInstallScope::Global,
            skills: vec!["demo".to_string()],
            copy: true,
            overwrite: false,
            visibility: SkillVisibility::Manual,
        },
        &target_root,
    )
    .unwrap();

    let installed_skill = target_root.join("demo/SKILL.md");
    let frontmatter = fs::read_to_string(installed_skill).unwrap();
    assert!(frontmatter.contains("disable-model-invocation: true"));
    assert!(!frontmatter.contains("tendi"));
    assert!(
        fs::read_to_string(target_root.join("demo/agents/openai.yaml"))
            .unwrap()
            .contains("allow_implicit_invocation: false")
    );
    assert_eq!(report.results.len(), 1);
    let source_records = super::skill_source_records_for_add(&report);
    assert_eq!(source_records.len(), 1);
    assert_eq!(source_records[0].skill_name, "demo");
    assert_eq!(source_records[0].skill_path, target_root.join("demo"));
    assert_eq!(source_records[0].source_kind, "local");
    assert_eq!(
        source_records[0].source_relative_path.as_deref(),
        Some("skills/demo")
    );
    assert_eq!(source_records[0].origin, "tendi-install");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn manual_visibility_survives_move_between_provider_locations() {
    let root = temp_dir("tendi-portable-visibility-move-test");
    let source = root.join(".codex/skills/demo");
    let destination = root.join(".claude/skills/demo");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: demo\ndescription: Demo skill\n---\n\n# demo\n",
    )
    .unwrap();

    let changes =
        plan_skill_visibility_at_path(&source, AgentKind::Codex, SkillVisibility::Manual, false)
            .unwrap();
    apply_changes(&ChangeSet { changes }).unwrap();

    let source_frontmatter = fs::read_to_string(source.join("SKILL.md")).unwrap();
    assert!(source_frontmatter.contains("disable-model-invocation: true"));
    assert!(!source_frontmatter.contains("tendi"));
    assert!(
        fs::read_to_string(source.join("agents/openai.yaml"))
            .unwrap()
            .contains("allow_implicit_invocation: false")
    );

    let source_record = SkillSourceRecord {
        skill_name: "demo".to_string(),
        skill_path: source.clone(),
        source_kind: "local".to_string(),
        source: None,
        source_ref: None,
        source_relative_path: None,
        source_version: None,
        update_status: "local".to_string(),
        origin: "test".to_string(),
    };
    let move_plan = SkillDistributionPlan {
        name: "demo".to_string(),
        source: source.clone(),
        destination: destination.clone(),
        mode: SkillDistributionMode::Move,
        source_symlink: false,
        destination_exists: false,
        source_sha256: sha256_file(&source.join("SKILL.md")).unwrap(),
        status: "ready".to_string(),
        message: None,
        source_record,
        projection_paths: Vec::new(),
    };
    apply_skill_distribution_plan(&move_plan).unwrap();

    assert!(!source.exists());
    let destination_frontmatter = fs::read_to_string(destination.join("SKILL.md")).unwrap();
    assert!(destination_frontmatter.contains("disable-model-invocation: true"));
    assert!(!destination_frontmatter.contains("tendi"));
    assert!(
        fs::read_to_string(destination.join("agents/openai.yaml"))
            .unwrap()
            .contains("allow_implicit_invocation: false")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn add_plan_expands_skill_dependencies() {
    let root = temp_dir("tendi-plan-add-skill-dependencies-test");
    let repo = root.join("repo");
    let grill_with_docs_dir = repo.join("skills/engineering/grill-with-docs");
    let grilling_dir = repo.join("skills/productivity/grilling");
    let domain_modeling_dir = repo.join("skills/engineering/domain-modeling");
    fs::create_dir_all(&grill_with_docs_dir).unwrap();
    fs::create_dir_all(&grilling_dir).unwrap();
    fs::create_dir_all(&domain_modeling_dir).unwrap();
    fs::write(
            grill_with_docs_dir.join("SKILL.md"),
            "---\nname: grill-with-docs\ndescription: Grill with docs\n---\n\nRun [`grilling`](../../productivity/grilling/SKILL.md), then read [`domain-modeling`](../domain-modeling/SKILL.md).\n",
        )
        .unwrap();
    fs::write(
        grilling_dir.join("SKILL.md"),
        "---\nname: grilling\ndescription: Grilling\n---\n\nAsk questions.\n",
    )
    .unwrap();
    fs::write(
        domain_modeling_dir.join("SKILL.md"),
        "---\nname: domain-modeling\ndescription: Domain modeling\n---\n\nModel terms.\n",
    )
    .unwrap();

    let plan = plan_skill_add(
        &root,
        &super::SkillAddOptions {
            source: "repo".to_string(),
            target: AgentKind::Shared.into(),
            scope: SkillInstallScope::Global,
            skills: vec!["grill-with-docs".to_string()],
            copy: true,
            overwrite: false,
            visibility: SkillVisibility::Auto,
        },
    )
    .unwrap();
    let selected = plan
        .selected
        .iter()
        .map(|skill| skill.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        selected,
        vec!["domain-modeling", "grill-with-docs", "grilling"]
    );
    let grill_with_docs = plan
        .available
        .iter()
        .find(|skill| skill.name == "grill-with-docs")
        .unwrap();
    assert_eq!(
        grill_with_docs.dependencies,
        vec!["domain-modeling".to_string(), "grilling".to_string()]
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn add_plan_expands_transitive_frontmatter_dependencies() {
    let root = temp_dir("tendi-plan-add-transitive-skill-dependencies-test");
    let repo = root.join("repo");
    for name in ["parent", "child", "base"] {
        fs::create_dir_all(repo.join(format!("skills/{name}"))).unwrap();
    }
    fs::write(
        repo.join("skills/parent/SKILL.md"),
        "---\nname: parent\ndependencies:\n  - child\n---\n",
    )
    .unwrap();
    fs::write(
        repo.join("skills/child/SKILL.md"),
        "---\nname: child\nrequires: base\n---\n",
    )
    .unwrap();
    fs::write(repo.join("skills/base/SKILL.md"), "---\nname: base\n---\n").unwrap();

    let plan = plan_skill_add(
        &root,
        &super::SkillAddOptions {
            source: "repo".to_string(),
            target: AgentKind::Shared.into(),
            scope: SkillInstallScope::Global,
            skills: vec!["parent".to_string()],
            copy: true,
            overwrite: false,
            visibility: SkillVisibility::Auto,
        },
    )
    .unwrap();
    let selected = plan
        .selected
        .iter()
        .map(|skill| skill.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(selected, vec!["base", "child", "parent"]);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn add_plan_expands_lark_style_dependencies() {
    let root = temp_dir("tendi-plan-add-lark-style-dependencies-test");
    let repo = root.join("repo");
    for name in ["lark-im", "lark-shared", "lark-sheets"] {
        fs::create_dir_all(repo.join(format!("skills/{name}"))).unwrap();
    }
    fs::write(
            repo.join("skills/lark-im/SKILL.md"),
            "---\nname: lark-im\n---\n\n开始前先读取 [`../lark-shared/SKILL.md`](../lark-shared/SKILL.md)。\n",
        )
        .unwrap();
    fs::write(
            repo.join("skills/lark-sheets/SKILL.md"),
            "---\nname: lark-sheets\nmetadata:\n  requires:\n    bins: [lark-cli]\n    siblings: [lark-shared]\n---\n",
        )
        .unwrap();
    fs::write(
        repo.join("skills/lark-shared/SKILL.md"),
        "---\nname: lark-shared\n---\n",
    )
    .unwrap();

    let plan = plan_skill_add(
        &root,
        &super::SkillAddOptions {
            source: "repo".to_string(),
            target: AgentKind::Shared.into(),
            scope: SkillInstallScope::Global,
            skills: vec!["lark-im".to_string(), "lark-sheets".to_string()],
            copy: true,
            overwrite: false,
            visibility: SkillVisibility::Auto,
        },
    )
    .unwrap();
    let selected = plan
        .selected
        .iter()
        .map(|skill| skill.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(selected, vec!["lark-im", "lark-shared", "lark-sheets"]);
    for name in ["lark-im", "lark-sheets"] {
        let skill = plan
            .available
            .iter()
            .find(|skill| skill.name == name)
            .unwrap();
        assert_eq!(skill.dependencies, vec!["lark-shared"]);
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn parses_skill_file_paths_without_prose_heuristics() {
    let cases = [
        (
            "认证等通用处理只读 [`../lark-shared/SKILL.md`](../lark-shared/SKILL.md)。",
            vec![PathBuf::from("../lark-shared/SKILL.md")],
        ),
        (
            "No dependency keywords: `/Users/example/pr/SKILL.md`.",
            vec![PathBuf::from("/Users/example/pr/SKILL.md")],
        ),
        (
            "按需改用 [`../lark-drive/SKILL.md`](../lark-drive/SKILL.md)。",
            vec![PathBuf::from("../lark-drive/SKILL.md")],
        ),
        (
            "Use `$child`; describe `SKILL.md`; ignore `demo/SKILL.md.bak`.",
            vec![],
        ),
        (
            "Windows path: `C:\\skills\\child\\SKILL.md`.",
            vec![PathBuf::from("C:/skills/child/SKILL.md")],
        ),
        (
            "Remote docs are not local dependencies: [demo](https://example.com/demo/SKILL.md).",
            vec![],
        ),
    ];

    for (body, expected) in cases {
        let text = format!("---\nname: test\n---\n\n{body}\n");
        assert_eq!(parse_skill_file_references(&text), expected);
    }
}

#[test]
fn scan_skills_follows_path_dependencies_outside_roots() {
    let root = temp_dir("tendi-scan-skill-relations-test");
    let parent_dir = root.join(".agents/skills/parent");
    let ignored_dir = root.join(".agents/skills/ignored");
    let child_dir = root.join("external/child-directory");
    let grandchild_dir = root.join("external/grandchild");
    fs::create_dir_all(&parent_dir).unwrap();
    fs::create_dir_all(&ignored_dir).unwrap();
    fs::create_dir_all(&child_dir).unwrap();
    fs::create_dir_all(&grandchild_dir).unwrap();
    fs::write(
        parent_dir.join("SKILL.md"),
        format!(
            "---\nname: parent\n---\n\nUse `$ignored`; load `{}`.\n",
            child_dir.join("SKILL.md").display()
        ),
    )
    .unwrap();
    fs::write(ignored_dir.join("SKILL.md"), "---\nname: ignored\n---\n").unwrap();
    fs::write(
        child_dir.join("SKILL.md"),
        "---\nname: renamed-child\n---\n\nRead `../grandchild/SKILL.md`.\n",
    )
    .unwrap();
    fs::write(
        grandchild_dir.join("SKILL.md"),
        "---\nname: grandchild\n---\n",
    )
    .unwrap();

    let scan = scan_skills(&root).unwrap();
    let parent = scan
        .skills
        .iter()
        .find(|skill| skill.name == "parent")
        .unwrap();
    let child = scan
        .skills
        .iter()
        .find(|skill| skill.name == "renamed-child")
        .unwrap();
    let ignored = scan
        .skills
        .iter()
        .find(|skill| skill.name == "ignored")
        .unwrap();
    let grandchild = scan
        .skills
        .iter()
        .find(|skill| skill.name == "grandchild")
        .unwrap();

    assert_eq!(parent.dependencies, vec!["renamed-child"]);
    assert_eq!(child.dependents, vec!["parent"]);
    assert_eq!(child.dependencies, vec!["grandchild"]);
    assert_eq!(grandchild.dependents, vec!["renamed-child"]);
    assert!(ignored.dependents.is_empty());
    assert!(
        child
            .paths
            .iter()
            .any(|path| { path.agent == AgentKind::Unknown && path.scope == "referenced" })
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scan_skills_reports_wrapper_catalog_dependencies() {
    let root = temp_dir("tendi-scan-wrapper-relations-test");
    let wrapper_dir = root.join(".agents/skills/wrapper");
    let child_dir = root.join(".agents/skills/child");
    let other_child_dir = root.join(".agents/skills/other-child");
    fs::create_dir_all(&wrapper_dir).unwrap();
    fs::create_dir_all(&child_dir).unwrap();
    fs::create_dir_all(&other_child_dir).unwrap();
    fs::write(
            wrapper_dir.join("SKILL.md"),
            format!(
            "---\nname: wrapper\n---\n\n# wrapper\n\n## Route\n\n{WRAPPER_CATALOG_START}\n- [`child`](<{}/SKILL.md>): 中文描述，没有英文触发词。\n- [`other-child`](<{}/SKILL.md>): 更多中文说明。\n{WRAPPER_CATALOG_END}\n",
                child_dir.display(),
                other_child_dir.display()
            ),
        )
        .unwrap();
    fs::write(child_dir.join("SKILL.md"), "---\nname: child\n---\n").unwrap();
    fs::write(
        other_child_dir.join("SKILL.md"),
        "---\nname: other-child\n---\n",
    )
    .unwrap();

    let scan = scan_skills(&root).unwrap();
    let wrapper = scan
        .skills
        .iter()
        .find(|skill| skill.name == "wrapper")
        .unwrap();
    let child = scan
        .skills
        .iter()
        .find(|skill| skill.name == "child")
        .unwrap();
    let other_child = scan
        .skills
        .iter()
        .find(|skill| skill.name == "other-child")
        .unwrap();

    assert!(wrapper.is_wrapper);
    assert_eq!(wrapper.dependencies, vec!["child", "other-child"]);
    assert_eq!(child.dependents, vec!["wrapper"]);
    assert_eq!(other_child.dependents, vec!["wrapper"]);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scan_skills_reports_markdown_route_dependencies() {
    let root = temp_dir("tendi-scan-markdown-route-relations-test");
    let wrapper_dir = root.join(".agents/skills/wrapper");
    let child_dir = root.join(".agents/skills/child");
    fs::create_dir_all(&wrapper_dir).unwrap();
    fs::create_dir_all(&child_dir).unwrap();
    fs::write(
            wrapper_dir.join("SKILL.md"),
            "---\nname: wrapper\n---\n\n# wrapper\n\n## Routes\n\n- [`child`](../child/SKILL.md): Route child requests.\n",
        )
        .unwrap();
    fs::write(child_dir.join("SKILL.md"), "---\nname: child\n---\n").unwrap();

    let scan = scan_skills(&root).unwrap();
    let wrapper = scan
        .skills
        .iter()
        .find(|skill| skill.name == "wrapper")
        .unwrap();
    let child = scan
        .skills
        .iter()
        .find(|skill| skill.name == "child")
        .unwrap();

    assert!(wrapper.is_wrapper);
    assert_eq!(wrapper.dependencies, vec!["child"]);
    assert_eq!(child.dependents, vec!["wrapper"]);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn delete_plan_reports_dependency_impact() {
    let root = temp_dir("tendi-delete-skill-relations-test");
    let parent_dir = root.join(".agents/skills/parent");
    let child_dir = root.join(".agents/skills/child");
    fs::create_dir_all(&parent_dir).unwrap();
    fs::create_dir_all(&child_dir).unwrap();
    fs::write(
        parent_dir.join("SKILL.md"),
        "---\nname: parent\n---\n\nUse [`child`](../child/SKILL.md).\n",
    )
    .unwrap();
    fs::write(child_dir.join("SKILL.md"), "---\nname: child\n---\n").unwrap();

    let scan = scan_skills(&root).unwrap();
    let child_id = scan
        .skills
        .iter()
        .find(|skill| skill.name == "child")
        .map(|skill| skill.id.clone())
        .unwrap();
    let plan = plan_skill_delete_many(&root, &[child_id]).unwrap();

    assert_eq!(plan.targets.len(), 1);
    assert_eq!(plan.dependents[0].name, "child");
    assert_eq!(plan.dependents[0].related, vec!["parent"]);
    assert!(format_delete_plan(&plan).contains("child is used by parent"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn add_plan_marks_existing_targets_as_replace_when_overwrite_is_enabled() {
    let root = temp_dir("tendi-plan-overwrite-add-skills-test");
    let repo = root.join("repo");
    let skill_dir = repo.join("skills/demo");
    let target_root = root.join("target");
    let target_dir = target_root.join("demo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: demo\ndescription: Demo skill\n---\n\n# demo\n",
    )
    .unwrap();
    fs::create_dir_all(&target_dir).unwrap();
    fs::write(
        target_dir.join("SKILL.md"),
        "---\nname: demo\ndescription: Existing demo\n---\n\n# demo\n",
    )
    .unwrap();

    let resolved = ResolvedAddSource {
        root: repo.canonicalize().unwrap(),
        kind: "local".to_string(),
        display_source: "repo".to_string(),
        git_ref: None,
        temporary: false,
    };
    let plan = build_skill_add_plan_with_target_root(
        &resolved,
        &super::SkillAddOptions {
            source: "repo".to_string(),
            target: AgentKind::Shared.into(),
            scope: SkillInstallScope::Global,
            skills: vec!["demo".to_string()],
            copy: false,
            overwrite: true,
            visibility: SkillVisibility::Auto,
        },
        true,
        &target_root,
    )
    .unwrap();
    assert_eq!(plan.operations[0].status, "replace");
    assert!(
        plan.operations[0]
            .message
            .as_deref()
            .unwrap_or("")
            .contains("will replace existing target")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn materialize_skill_dir_sanitizes_target_name_and_copies() {
    let root = temp_dir("tendi-materialize-add-test");
    let source = root.join("source");
    let target_root = root.join("target");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&target_root).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: demo\ndescription: Demo\n---\n\n# demo\n",
    )
    .unwrap();

    let result =
        materialize_skill_dir_to_root(&source, &target_root, "../Demo Skill", true, false, false)
            .unwrap();
    assert_eq!(result.mode, "copy");
    assert!(target_root.join("demo-skill/SKILL.md").is_file());
    assert!(!target_root.join("../Demo Skill").exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn materialize_skill_dir_overwrites_existing_target_when_requested() {
    let root = temp_dir("tendi-materialize-overwrite-add-test");
    let source = root.join("source");
    let target_root = root.join("target");
    let target = target_root.join("demo");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: demo\ndescription: New\n---\n\n# new\n",
    )
    .unwrap();
    fs::write(
        target.join("SKILL.md"),
        "---\nname: demo\ndescription: Old\n---\n\n# old\n",
    )
    .unwrap();

    let result =
        materialize_skill_dir_to_root(&source, &target_root, "demo", true, true, false).unwrap();
    assert_eq!(result.mode, "copy");
    assert!(result.applied);
    assert!(
        fs::read_to_string(target.join("SKILL.md"))
            .unwrap()
            .contains("# new")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_distribution_moves_and_links_existing_installations() {
    let root = temp_dir("tendi-skill-distribution-test");
    let source = root.join("codex/demo");
    let moved = root.join("shared/demo");
    let linked = root.join("cursor/demo");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("SKILL.md"), "---\nname: demo\n---\n").unwrap();

    let source_record = || SkillSourceRecord {
        skill_name: "demo".to_string(),
        skill_path: source.clone(),
        source_kind: "local".to_string(),
        source: None,
        source_ref: None,
        source_version: None,
        source_relative_path: None,
        update_status: "local".to_string(),
        origin: "test".to_string(),
    };
    let move_plan = SkillDistributionPlan {
        name: "demo".to_string(),
        source: source.clone(),
        destination: moved.clone(),
        mode: SkillDistributionMode::Move,
        source_symlink: false,
        destination_exists: false,
        source_sha256: sha256_file(&source.join("SKILL.md")).unwrap(),
        status: "ready".to_string(),
        message: None,
        source_record: source_record(),
        projection_paths: Vec::new(),
    };
    let moved_result = apply_skill_distribution_plan(&move_plan).unwrap();
    assert_eq!(moved_result.mode, "move");
    assert!(!source.exists());
    assert!(moved.join("SKILL.md").is_file());

    let link_source = root.join("link-source");
    fs::create_dir_all(&link_source).unwrap();
    fs::write(link_source.join("SKILL.md"), "---\nname: link\n---\n").unwrap();
    let link_plan = SkillDistributionPlan {
        name: "link".to_string(),
        source: link_source.clone(),
        destination: linked.clone(),
        mode: SkillDistributionMode::Symlink,
        source_symlink: false,
        destination_exists: false,
        source_sha256: sha256_file(&link_source.join("SKILL.md")).unwrap(),
        status: "ready".to_string(),
        message: None,
        source_record: source_record(),
        projection_paths: Vec::new(),
    };
    let linked_result = apply_skill_distribution_plan(&link_plan).unwrap();
    assert_eq!(linked_result.mode, "symlink");
    assert!(link_source.join("SKILL.md").is_file());
    assert!(linked.join("SKILL.md").is_file());
    assert!(
        fs::symlink_metadata(&linked)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn rehome_canonical_skill_keeps_remaining_projections_linked() {
    let root = temp_dir("tendi-skill-rehome-test");
    let source = root.join(".agents/skills/demo");
    let destination = root.join(".codex/skills/demo");
    let remaining = root.join(".cursor/skills/demo");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::create_dir_all(remaining.parent().unwrap()).unwrap();
    fs::write(source.join("SKILL.md"), "---\nname: demo\n---\n").unwrap();
    create_symlink(&source, &destination).unwrap();
    create_symlink(&source, &remaining).unwrap();

    rehome_canonical_skill_and_relink_projections(
        &source,
        &destination,
        &[destination.clone(), remaining.clone()],
    )
    .unwrap();

    assert!(!source.exists());
    assert!(destination.join("SKILL.md").is_file());
    assert!(
        !fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(remaining.join("SKILL.md").is_file());
    assert!(
        fs::symlink_metadata(&remaining)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        remaining.canonicalize().unwrap(),
        destination.canonicalize().unwrap()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn materialize_tendi_cache_links_prefers_one_physical_canonical() {
    let root = temp_dir("tendi-cache-materialization-test");
    let cache = tendi_state_root()
        .unwrap()
        .join("sources")
        .join(format!("test-cache-{}", std::process::id()));
    let shared = root.join(".agents/skills/demo");
    let codex = root.join(".codex/skills/demo");
    fs::create_dir_all(&cache).unwrap();
    fs::create_dir_all(shared.parent().unwrap()).unwrap();
    fs::create_dir_all(codex.parent().unwrap()).unwrap();
    fs::write(cache.join("SKILL.md"), "---\nname: demo\n---\n").unwrap();
    create_symlink(&cache, &shared).unwrap();
    create_symlink(&cache, &codex).unwrap();

    let mut skill = test_skill("demo", "Demo", &shared);
    let mut codex_path = skill.paths[0].clone();
    codex_path.path = codex.clone();
    codex_path.root = codex.parent().unwrap().to_path_buf();
    codex_path.agent = AgentKind::Codex;
    skill.paths.push(codex_path);
    skill.agents.push(AgentKind::Codex);
    let scan = SkillScan {
        roots: Vec::new(),
        skills: vec![skill],
        warnings: Vec::new(),
    };

    assert!(materialize_tendi_cache_links(&scan).unwrap());
    assert!(shared.join("SKILL.md").is_file());
    assert!(
        !fs::symlink_metadata(&shared)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(codex.join("SKILL.md").is_file());
    assert!(
        fs::symlink_metadata(&codex)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        codex.canonicalize().unwrap(),
        shared.canonicalize().unwrap()
    );

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(cache);
}

#[test]
fn sanitize_skill_dir_name_blocks_empty_and_traversal_names() {
    assert_eq!(
        sanitize_skill_dir_name("../Demo Skill").unwrap(),
        "demo-skill"
    );
    assert!(sanitize_skill_dir_name("////").is_err());
}

#[test]
fn registry_source_scan_and_update_check_are_recorded() {
    let root = temp_dir("tendi-registry-source-test");
    let skill_dir = root.join(".agents/skills/demo");
    fs::create_dir_all(&skill_dir).unwrap();
    let registry_file = root.join("registry-demo.md");
    fs::write(
        &registry_file,
        "---\nname: demo\nversion: 2.0.0\n---\n\n# demo\n",
    )
    .unwrap();
    fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: demo\ndescription: Demo\nversion: 1.0.0\nsource: file://{}\n---\n\n# demo\n",
                registry_file.display()
            ),
        )
        .unwrap();

    let scan = scan_skills(&root).unwrap();
    let skill = scan
        .skills
        .iter()
        .find(|skill| skill.name == "demo")
        .unwrap();
    assert_eq!(skill.update_status, "checkable");
    assert_eq!(
        skill.source_summary,
        format!("registry:file://{}", registry_file.display())
    );
    assert_eq!(skill.paths[0].source_kind, "registry");
    assert_eq!(
        skill.paths[0].source.as_deref(),
        Some(format!("file://{}", registry_file.display()).as_str())
    );
    assert_eq!(skill.paths[0].source_version.as_deref(), Some("1.0.0"));

    let updates = check_skill_updates(&root).unwrap();
    let update = updates.iter().find(|update| update.name == "demo").unwrap();
    assert_eq!(update.status, "update-available");
    assert_eq!(update.current_version.as_deref(), Some("1.0.0"));
    assert_eq!(update.latest_version.as_deref(), Some("2.0.0"));
    assert_eq!(update.source_kind, "registry");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn update_plan_can_use_cached_reports_without_rechecking_sources() {
    let root = temp_dir("tendi-cached-update-report-plan-test");
    let skill_dir = root.join(".agents/skills/demo");
    fs::create_dir_all(&skill_dir).unwrap();
    let registry_file = root.join("registry-demo.md");
    fs::write(
        &registry_file,
        "---\nname: demo\nversion: 2.0.0\n---\n\n# demo\n",
    )
    .unwrap();
    fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: demo\ndescription: Demo\nversion: 1.0.0\nsource: file://{}\n---\n\n# demo\n",
                registry_file.display()
            ),
        )
        .unwrap();

    let scan = scan_skills(&root).unwrap();
    let skill_id = scan
        .skills
        .iter()
        .find(|skill| skill.name == "demo")
        .map(|skill| skill.id.clone())
        .unwrap();
    let store = crate::storage::Store::open(root.join("tendi.sqlite3")).unwrap();
    let plan = plan_skill_updates_many_for_scan_in_workspace_with_store_and_reports(
        &scan,
        std::slice::from_ref(&skill_id),
        &root,
        &store,
        &[SkillUpdateReport {
            id: skill_id.clone(),
            name: "demo".to_string(),
            status: "update-available".to_string(),
            current_version: Some("1.0.0".to_string()),
            latest_version: Some("3.0.0".to_string()),
            source: Some(format!("file://{}", registry_file.display())),
            source_kind: "registry".to_string(),
        }],
    )
    .unwrap();

    assert_eq!(plan.source_updates[0].source_version, "3.0.0");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_update_check_does_not_touch_fetch_head() {
    let root = temp_dir("tendi-update-check-fetch-head");
    let remote = root.join("remote");
    let local = root.join("local");
    fs::create_dir_all(remote.join("skills/demo")).unwrap();
    fs::write(
        remote.join("skills/demo/SKILL.md"),
        "---\nname: demo\ndescription: one\n---\n",
    )
    .unwrap();
    run_test_git(&remote, &["init", "--quiet"]);
    run_test_git(&remote, &["config", "user.email", "tendi@example.test"]);
    run_test_git(&remote, &["config", "user.name", "Tendi Test"]);
    run_test_git(&remote, &["add", "."]);
    run_test_git(&remote, &["commit", "--quiet", "-m", "one"]);
    let output = Command::new("git")
        .args(["clone", "--quiet"])
        .arg(&remote)
        .arg(&local)
        .output()
        .unwrap();
    assert!(output.status.success());
    fs::write(
        remote.join("skills/demo/SKILL.md"),
        "---\nname: demo\ndescription: two\n---\n",
    )
    .unwrap();
    run_test_git(&remote, &["add", "."]);
    run_test_git(&remote, &["commit", "--quiet", "-m", "two"]);
    let fetch_head = local.join(".git/FETCH_HEAD");
    fs::write(&fetch_head, "sentinel\n").unwrap();

    let remote_head = super::fetch_git_remote_head(
        &local,
        &remote.display().to_string(),
        None,
        super::git::never_cancelled(),
    )
    .expect("fetch remote head");

    assert_eq!(fs::read_to_string(&fetch_head).unwrap(), "sentinel\n");
    assert_ne!(
        run_test_git(&local, &["rev-parse", "HEAD"]),
        remote_head.oid
    );
    assert!(
        !run_test_git(&local, &["diff", "--name-only", "HEAD", &remote_head.oid])
            .trim()
            .is_empty()
    );
    let reference = remote_head.reference.clone();
    super::cleanup_git_remote_heads(BTreeMap::from([(local.clone(), Some(remote_head))]));
    let status = Command::new("git")
        .args(["show-ref", "--verify", "--quiet", &reference])
        .current_dir(&local)
        .status()
        .unwrap();
    assert!(!status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn remote_update_check_delegates_repository_resources_to_report_workers() {
    let root = temp_dir("tendi-update-check-resource-delegation");
    let remote = root.join("remote");
    let skill_dir = root.join("installed/skills/demo");
    fs::create_dir_all(remote.join("skills/demo")).unwrap();
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        remote.join("skills/demo/SKILL.md"),
        "---\nname: demo\ndescription: one\n---\n\none\n",
    )
    .unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: demo\ndescription: one\n---\n\none\n",
    )
    .unwrap();
    run_test_git(&remote, &["init", "--quiet"]);
    run_test_git(&remote, &["config", "user.email", "tendi@example.test"]);
    run_test_git(&remote, &["config", "user.name", "Tendi Test"]);
    run_test_git(&remote, &["add", "."]);
    run_test_git(&remote, &["commit", "--quiet", "-m", "one"]);
    let current_version = run_test_git(&remote, &["rev-parse", "HEAD"]);

    let source = remote.display().to_string();
    let checkout = super::git_checkout_for_source(&source, None, super::git::never_cancelled())
        .expect("create persistent source checkout");

    fs::write(
        remote.join("skills/demo/SKILL.md"),
        "---\nname: demo\ndescription: two\n---\n\ntwo\n",
    )
    .unwrap();
    run_test_git(&remote, &["add", "."]);
    run_test_git(&remote, &["commit", "--quiet", "-m", "two"]);

    let mut resources = super::git::mutation_resource_paths(&checkout).unwrap();
    resources.push(checkout.clone());
    let _outer_resources = crate::coordination::acquire_file_resources(&resources).unwrap();

    let mut skill = test_skill("demo", "Demo", &skill_dir);
    let path = &mut skill.paths[0];
    path.source_kind = "git".to_string();
    path.source = Some(source);
    path.source_version = Some(current_version);
    path.source_relative_path = Some("skills/demo".to_string());
    path.update_status = "checkable".to_string();

    let updates = super::check_skill_updates_for_skills(&[&skill], super::git::never_cancelled());

    assert_eq!(updates[0].status, "update-available");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn up_to_date_remote_check_does_not_fetch_remote_objects() {
    let root = temp_dir("tendi-update-check-ls-remote");
    let remote = root.join("remote");
    let local = root.join("local");
    fs::create_dir_all(remote.join("skills/demo")).unwrap();
    fs::write(
        remote.join("skills/demo/SKILL.md"),
        "---\nname: demo\ndescription: one\n---\n",
    )
    .unwrap();
    run_test_git(&remote, &["init", "--quiet"]);
    run_test_git(&remote, &["config", "user.email", "tendi@example.test"]);
    run_test_git(&remote, &["config", "user.name", "Tendi Test"]);
    run_test_git(&remote, &["add", "."]);
    run_test_git(&remote, &["commit", "--quiet", "-m", "one"]);
    let output = Command::new("git")
        .args(["clone", "--quiet"])
        .arg(&remote)
        .arg(&local)
        .output()
        .unwrap();
    assert!(output.status.success());

    let current_version = run_test_git(&local, &["rev-parse", "HEAD"]);
    let skill_dir = local.join("skills/demo");
    let mut skill = test_skill("demo", "Demo", &skill_dir);
    let path = &mut skill.paths[0];
    path.source_kind = "git".to_string();
    path.source = Some(remote.display().to_string());
    path.source_version = Some(current_version);
    path.source_relative_path = Some("skills/demo".to_string());
    path.update_status = "checkable".to_string();
    let fetch_head = local.join(".git/FETCH_HEAD");
    fs::write(&fetch_head, "sentinel\n").unwrap();

    let updates = super::check_skill_updates_for_skills(&[&skill], super::git::never_cancelled());

    assert_eq!(updates[0].status, "up-to-date");
    assert_eq!(fs::read_to_string(fetch_head).unwrap(), "sentinel\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn git_update_clears_tracked_and_untracked_tendi_policy_changes() {
    let root = temp_dir("tendi-git-update-settings-test");
    let tracked_skill = root.join("skills/tracked-policy");
    let untracked_skill = root.join("skills/untracked-policy");
    fs::create_dir_all(tracked_skill.join("agents")).unwrap();
    fs::create_dir_all(&untracked_skill).unwrap();
    let baseline_skill = "---\nname: demo\ndescription: Demo\n---\n\n# Demo\n";
    fs::write(tracked_skill.join("SKILL.md"), baseline_skill).unwrap();
    fs::write(untracked_skill.join("SKILL.md"), baseline_skill).unwrap();
    let baseline_policy = concat!(
        "interface:\n",
        "  display_name: \"Demo\"\n",
        "policy:\n",
        "  allow_implicit_invocation: true\n",
    );
    fs::write(tracked_skill.join("agents/openai.yaml"), baseline_policy).unwrap();

    run_test_git(&root, &["init", "--quiet"]);
    run_test_git(&root, &["config", "user.email", "tendi@example.test"]);
    run_test_git(&root, &["config", "user.name", "Tendi Test"]);
    run_test_git(&root, &["add", "."]);
    run_test_git(&root, &["commit", "--quiet", "-m", "baseline"]);

    let mut initial_changes = Vec::new();
    initial_changes.push(
        super::plan_skill_frontmatter_for_agents(
            tracked_skill.join("SKILL.md"),
            &super::skill_visibility_frontmatter_agents(AgentKind::Codex),
            SkillVisibility::Manual,
        )
        .unwrap(),
    );
    initial_changes.push(
        crate::providers::codex::plan_skill_policy_file(
            tracked_skill.join("agents/openai.yaml"),
            SkillVisibility::Auto,
        )
        .unwrap(),
    );
    initial_changes.push(
        super::plan_skill_frontmatter_for_agents(
            untracked_skill.join("SKILL.md"),
            &super::skill_visibility_frontmatter_agents(AgentKind::Codex),
            SkillVisibility::Manual,
        )
        .unwrap(),
    );
    initial_changes.push(
        crate::providers::codex::plan_skill_policy_file(
            untracked_skill.join("agents/openai.yaml"),
            SkillVisibility::Manual,
        )
        .unwrap(),
    );
    super::apply_changes(&ChangeSet {
        changes: initial_changes,
    })
    .unwrap();
    let manual_policy = crate::providers::codex::plan_skill_policy_file(
        tracked_skill.join("agents/openai.yaml"),
        SkillVisibility::Manual,
    )
    .unwrap();
    super::apply_changes(&ChangeSet {
        changes: vec![manual_policy],
    })
    .unwrap();
    let dirty = run_test_git(&root, &["status", "--short"]);
    assert!(dirty.contains("skills/tracked-policy/agents/openai.yaml"));
    assert!(untracked_skill.join("agents/openai.yaml").is_file());

    super::clear_tendi_git_changes(&GitUpdateAction {
        name: "demo".to_string(),
        skill_names: vec!["demo".to_string()],
        repo: root.clone(),
        source: root.display().to_string(),
        source_ref: None,
        current_version: None,
        latest_version: None,
        diff: String::new(),
        files: Vec::new(),
        tendi_settings: vec![
            GitSkillVisibility {
                skill_dir: tracked_skill.clone(),
                agent: AgentKind::Codex,
                visibility: SkillVisibility::Manual,
            },
            GitSkillVisibility {
                skill_dir: untracked_skill.clone(),
                agent: AgentKind::Codex,
                visibility: SkillVisibility::Manual,
            },
        ],
        materialized_targets: Vec::new(),
    })
    .unwrap();

    assert_eq!(run_test_git(&root, &["status", "--short"]), "");
    assert_eq!(
        fs::read_to_string(tracked_skill.join("agents/openai.yaml")).unwrap(),
        baseline_policy
    );
    assert!(!untracked_skill.join("agents/openai.yaml").exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn git_update_applies_prepared_plan_without_contacting_remote() {
    let root = temp_dir("tendi-git-update-restore-test");
    let skill_dir = root.join("skills/demo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: demo\ndescription: Demo\n---\n\n# Demo\n",
    )
    .unwrap();
    run_test_git(&root, &["init", "--quiet"]);
    run_test_git(&root, &["config", "user.email", "tendi@example.test"]);
    run_test_git(&root, &["config", "user.name", "Tendi Test"]);
    run_test_git(&root, &["add", "."]);
    run_test_git(&root, &["commit", "--quiet", "-m", "baseline"]);

    let action = GitUpdateAction {
        name: "demo".to_string(),
        skill_names: vec!["demo".to_string()],
        repo: root.clone(),
        source: root.display().to_string(),
        source_ref: None,
        current_version: None,
        latest_version: None,
        diff: String::new(),
        files: Vec::new(),
        tendi_settings: Vec::new(),
        materialized_targets: Vec::new(),
    };

    // No origin is configured. All remote work belongs to preparation;
    // applying this immutable (empty) plan must still succeed offline.
    apply_git_update(&action).unwrap();
    assert_eq!(
        fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
        "---\nname: demo\ndescription: Demo\n---\n\n# Demo\n"
    );
    assert!(!skill_dir.join("agents/openai.yaml").exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn materialized_git_update_replaces_copy_and_advances_database_source() {
    let root = temp_dir("tendi-materialized-git-update-test");
    let repo = root.join("repo");
    let source_skill = repo.join("skills/demo");
    let target = root.join("project/.agents/skills/demo");
    fs::create_dir_all(&source_skill).unwrap();
    fs::write(
        source_skill.join("SKILL.md"),
        "---\nname: demo\ndescription: old\n---\n\n# old\n",
    )
    .unwrap();
    run_test_git(&repo, &["init", "--quiet"]);
    run_test_git(&repo, &["config", "user.email", "tendi@example.test"]);
    run_test_git(&repo, &["config", "user.name", "Tendi Test"]);
    run_test_git(&repo, &["add", "."]);
    run_test_git(&repo, &["commit", "--quiet", "-m", "old"]);
    copy_dir(&source_skill, &target).unwrap();
    let old = run_test_git(&repo, &["rev-parse", "HEAD"]);

    fs::write(
        source_skill.join("SKILL.md"),
        "---\nname: demo\ndescription: new\n---\n\n# new\n",
    )
    .unwrap();
    run_test_git(&repo, &["add", "."]);
    run_test_git(&repo, &["commit", "--quiet", "-m", "new"]);
    let latest = run_test_git(&repo, &["rev-parse", "HEAD"]);

    let store = crate::storage::Store::open(root.join("tendi.sqlite3")).unwrap();
    store
        .upsert_skill_source_records(&[SkillSourceRecord {
            skill_name: "demo".to_string(),
            skill_path: target.clone(),
            source_kind: "github".to_string(),
            source: Some(repo.display().to_string()),
            source_ref: None,
            source_version: Some(old),
            source_relative_path: Some("skills/demo/SKILL.md".to_string()),
            update_status: "tracked".to_string(),
            origin: "skills-cli-lock".to_string(),
        }])
        .unwrap();
    let action = GitUpdateAction {
        name: "demo".to_string(),
        skill_names: vec!["demo".to_string()],
        repo: repo.clone(),
        source: repo.display().to_string(),
        source_ref: None,
        current_version: None,
        latest_version: Some(latest.clone()),
        diff: String::new(),
        files: Vec::new(),
        tendi_settings: Vec::new(),
        materialized_targets: vec![MaterializedGitTarget {
            name: "demo".to_string(),
            target: target.clone(),
            agent: AgentKind::Shared,
            source_relative_path: Some("skills/demo/SKILL.md".to_string()),
            visibility: SkillVisibility::Auto,
            // This fixture verifies materialization and persistence, not
            // global provider configuration (covered by temp config tests).
            uses_shared_layout: false,
            files: Vec::new(),
        }],
    };

    apply_skill_update_plan_with_store(
        &SkillUpdatePlan {
            file_changes: ChangeSet {
                changes: Vec::new(),
            },
            git_updates: vec![action],
            skipped: Vec::new(),
            source_updates: Vec::new(),
            merge_issues: Vec::new(),
        },
        &store,
    )
    .unwrap();

    assert!(
        fs::read_to_string(target.join("SKILL.md"))
            .unwrap()
            .contains("# new")
    );
    assert_eq!(
        store
            .skill_source_record(&target)
            .unwrap()
            .unwrap()
            .source_version
            .as_deref(),
        Some(latest.as_str())
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wrapper_refresh_preserves_manual_content() {
    let root = Path::new("/tmp/tendi-skills");
    let lark_im = test_skill("lark-im", "Send Lark messages", &root.join("lark-im"));
    let lark_doc = test_skill("lark-doc", "Read Lark docs", &root.join("lark-doc"));
    let before = r#"---
name: lark
version: 1.0.0
description: Custom Lark router.
tendi:
  wrapper_description: Use when the user needs custom Lark routing across chat and docs.
---

# Lark Router

Keep this handmade intro.

## Root

```text
/old/root
```

## Route

- `lark-old`: Old route.

## Procedure

1. Preserve this exact workflow.
"#;
    let output = render_wrapper_after("lark", &[&lark_im, &lark_doc], Some(before));

    assert!(output.contains("version: 1.0.0"));
    assert!(output.contains("description: Custom Lark router."));
    assert!(output.contains(
        "wrapper_description: Use when the user needs custom Lark routing across chat and docs."
    ));
    assert!(output.contains("Keep this handmade intro."));
    assert!(output.contains("## Procedure"));
    assert!(output.contains("1. Preserve this exact workflow."));
    assert!(output.contains("## Root"));
    assert!(output.contains("/old/root"));
    assert!(output.contains(WRAPPER_CATALOG_START));
    assert!(output.contains(WRAPPER_CATALOG_END));
    assert!(
        output.contains("- [`lark-im`](</tmp/tendi-skills/lark-im/SKILL.md>): Send Lark messages")
    );
    assert!(
        output.contains("- [`lark-doc`](</tmp/tendi-skills/lark-doc/SKILL.md>): Read Lark docs")
    );
    assert!(!output.contains("lark-old"));
}

#[test]
fn scan_skills_synced_refreshes_wrapper_from_child_descriptions() {
    let root = temp_dir("tendi-wrapper-sync-test");
    let skills_root = root.join(".agents/skills");
    let child_dir = skills_root.join("tendi-sync-child");
    let wrapper_dir = skills_root.join("tendi-sync-wrapper");
    fs::create_dir_all(&child_dir).unwrap();
    fs::create_dir_all(&wrapper_dir).unwrap();
    fs::write(
            child_dir.join("SKILL.md"),
            "---\nname: tendi-sync-child\ndescription: Send updated sync messages.\n---\n\n# tendi-sync-child\n",
        )
        .unwrap();
    fs::write(
            wrapper_dir.join("SKILL.md"),
            format!(
                "---\nname: tendi-sync-wrapper\ndescription: Old wrapper.\n---\n\n# tendi-sync-wrapper\n\nKeep this intro.\n\n## Route\n\n- [`tendi-sync-child`](<{}>): Send stale sync messages.\n\n## Procedure\n\n1. Keep this workflow.\n",
                child_dir.join("SKILL.md").display()
            ),
        )
        .unwrap();

    let scan = crate::skills::scan_skills_synced(&root).unwrap();
    let wrapper_text = fs::read_to_string(wrapper_dir.join("SKILL.md")).unwrap();
    let wrapper = scan
        .skills
        .iter()
        .find(|skill| skill.name == "tendi-sync-wrapper")
        .unwrap();

    assert!(wrapper.is_wrapper);
    assert_eq!(wrapper.description.as_deref(), Some("Old wrapper."));
    assert!(wrapper_text.contains("description: Old wrapper."));
    assert!(wrapper_text.contains("Keep this intro."));
    assert!(wrapper_text.contains(WRAPPER_CATALOG_START));
    assert!(wrapper_text.contains(WRAPPER_CATALOG_END));
    assert!(wrapper_text.contains("- [`tendi-sync-child`]"));
    assert!(wrapper_text.contains("Send updated sync messages."));
    assert!(!wrapper_text.contains("Send stale sync messages."));
    assert!(!wrapper_text.contains("tags:"));
    assert!(wrapper_text.contains("1. Keep this workflow."));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn provider_frontmatter_does_not_persist_tendi_visibility() {
    let path = std::env::temp_dir().join(format!(
        "tendi-visibility-test-{}.md",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&path, "---\nname: demo\n---\n\n# demo\n").unwrap();

    let change = super::plan_skill_frontmatter_for_agents(
        path.clone(),
        &[AgentKind::Cursor],
        SkillVisibility::Off,
    )
    .unwrap();
    let doc = MarkdownDoc::parse(&change.after).unwrap();
    let meta = Value::Mapping(doc.meta);

    assert!(meta.get("tendi").is_none());
    assert_eq!(
        meta.get("disable-model-invocation")
            .and_then(Value::as_bool),
        Some(true)
    );

    let _ = fs::remove_file(path);
}

#[test]
fn database_visibility_survives_external_provider_rewrite() {
    let root = temp_dir("tendi-database-visibility-lock-test");
    let skill_dir = root.join(".agents/skills/demo");
    let skill_file = skill_dir.join("SKILL.md");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        &skill_file,
        "---\nname: demo\ndescription: Demo\n---\n\n# Demo\n",
    )
    .unwrap();

    let store = crate::storage::Store::open(root.join("tendi.sqlite3")).unwrap();
    let skill_path = skill_dir.canonicalize().unwrap();
    store
        .upsert_skill_visibilities_for_workspace(
            &root,
            &[(skill_path.clone(), SkillVisibility::Manual)],
        )
        .unwrap();

    fs::write(
        &skill_file,
        "---\nname: demo\ndescription: Demo\ndisable-model-invocation: false\n---\n\n# Demo\n",
    )
    .unwrap();
    let scan = SkillScan {
        roots: Vec::new(),
        skills: vec![test_skill("demo", "Demo", &skill_dir)],
        warnings: Vec::new(),
    };
    let projected =
        super::reconcile_skill_visibility_for_workspace(&store, &root, scan, &[]).unwrap();
    let skill = projected.skills.first().unwrap();

    assert_eq!(skill.visibility, SkillVisibility::Manual);
    assert!(
        fs::read_to_string(&skill_file)
            .unwrap()
            .contains("disable-model-invocation: true")
    );
    assert_eq!(
        store
            .skill_visibilities_for_workspace(&root)
            .unwrap()
            .get(&skill_path)
            .copied(),
        Some(SkillVisibility::Manual)
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn non_codex_skill_ignores_codex_skill_config() {
    let root = temp_dir("tendi-codex-skill-config-disabled-test");
    let skills_root = root.join(".agents/skills");
    let skill_dir = skills_root.join("pr");
    let skill_file = skill_dir.join("SKILL.md");
    let config_path = root.join(".codex/config.toml");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(
        &skill_file,
        "---\nname: pr\ndescription: PR workflow\n---\n\n# PR\n",
    )
    .unwrap();
    fs::write(
        &config_path,
        format!(
            "[[skills.config]]\npath = \"{}\"\nenabled = false\n",
            skill_file.display()
        ),
    )
    .unwrap();

    let root_record = super::SkillRoot {
        path: skills_root,
        scope: "global".to_string(),
        agent: AgentKind::Shared,
        plugin_id: None,
        plugin_enabled: None,
    };
    let skill = super::read_skill(
        &root_record,
        &skill_file,
        &mut super::ProvenanceResolver::default(),
    )
    .unwrap();

    assert_eq!(skill.path.provider_skill_enabled, None);
    assert_eq!(skill.path.effective_visibility, SkillVisibility::Auto);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn manual_visibility_reenables_disabled_codex_skill_config() {
    let root = temp_dir("tendi-codex-skill-config-reenable-test");
    let config_path = root.join(".codex/config.toml");
    let skill_file = root.join(".agents/skills/pr/SKILL.md");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(
        &config_path,
        format!(
            "[[skills.config]]\npath = \"{}\"\nenabled = false\n",
            skill_file.display()
        ),
    )
    .unwrap();

    let change = crate::providers::codex::plan_skill_config_at(
        config_path.clone(),
        skill_file,
        SkillVisibility::Manual,
    )
    .unwrap()
    .unwrap();
    let parsed = toml::from_str::<toml::Value>(&change.after).unwrap();
    let enabled = parsed
        .get("skills")
        .and_then(|skills| skills.get("config"))
        .and_then(toml::Value::as_array)
        .and_then(|configs| configs.first())
        .and_then(|config| config.get("enabled"))
        .and_then(toml::Value::as_bool);

    assert_eq!(enabled, Some(true));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn off_visibility_preserves_unrelated_frontmatter_formatting() {
    let path = std::env::temp_dir().join(format!(
        "tendi-off-minimal-test-{}.md",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let before = "---\nname: demo\ndescription: \"Demo\"\ndisable-model-invocation: true\nmetadata:\n  bins: [\"demo\"]\n---\n\n# demo\n";
    fs::write(&path, before).unwrap();

    let change = super::plan_skill_frontmatter_for_agents(
        path.clone(),
        &[AgentKind::Cursor],
        SkillVisibility::Off,
    )
    .unwrap();

    assert!(change.after.contains("description: \"Demo\""));
    assert!(change.after.contains("  bins: [\"demo\"]"));
    assert!(!change.after.contains("tendi:"));
    let _ = fs::remove_file(path);
}

#[test]
fn visibility_update_preserves_crlf_frontmatter_formatting() {
    let path = std::env::temp_dir().join(format!(
        "tendi-crlf-frontmatter-test-{}.md",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let before = "---\r\nname: demo\r\ndescription: \"Demo\"\r\n---\r\n\r\n# demo\r\n";
    fs::write(&path, before).unwrap();

    let change = super::plan_skill_frontmatter_for_agents(
        path.clone(),
        &[AgentKind::Cursor],
        SkillVisibility::Off,
    )
    .unwrap();

    assert!(change.after.contains("description: \"Demo\"\r\n"));
    assert!(change.after.contains("disable-model-invocation: true\r\n"));
    assert!(!change.after.contains("tendi:"));
    assert!(!change.after.replace("\r\n", "").contains('\n'));
    let _ = fs::remove_file(path);
}

#[test]
fn merge_skill_manifest_prefers_local_visibility_for_remote_visibility_change() {
    let base = "---\nname: demo\ndescription: Old\n---\n\n# old\n";
    let local = "---\nname: demo\ndescription: Old\n---\n\n# old\n";
    let incoming = "---\nname: demo\ndescription: New\n---\n\n# new\n";
    let expected = "---\nname: demo\ndescription: New\n---\n\n# new\n";

    let files = merge_file_maps(
        Some(BTreeMap::from([(
            "SKILL.md".to_string(),
            base.as_bytes().to_vec(),
        )])),
        BTreeMap::from([("SKILL.md".to_string(), local.as_bytes().to_vec())]),
        BTreeMap::from([("SKILL.md".to_string(), incoming.as_bytes().to_vec())]),
        "",
        "/tmp/demo",
        SkillVisibility::Manual,
        AgentKind::Shared,
    );

    let file = files.first().expect("merged skill file");
    assert_eq!(file.status, "remote");
    assert_eq!(file.after, expected);
}

#[test]
fn semantically_equal_skill_manifest_formatting_is_not_a_conflict() {
    let base = "---\nname: demo\ndescription: Old\n---\n\n# same\n";
    let local = "---\nname: demo\ndescription: New\nmetadata:\n  bins:\n  - demo\ndisable-model-invocation: true\n---\n\n# same\n";
    let incoming =
        "---\nname: demo\ndescription: \"New\"\nmetadata:\n  bins: [demo]\n---\n\n# same\n";

    let files = merge_file_maps(
        Some(BTreeMap::from([(
            "SKILL.md".to_string(),
            base.as_bytes().to_vec(),
        )])),
        BTreeMap::from([("SKILL.md".to_string(), local.as_bytes().to_vec())]),
        BTreeMap::from([("SKILL.md".to_string(), incoming.as_bytes().to_vec())]),
        "",
        "/tmp/demo",
        SkillVisibility::Manual,
        AgentKind::Shared,
    );

    let file = files.first().expect("remote skill manifest update");
    assert_eq!(file.status, "remote");
    assert!(!file.after.contains("<<<<<<< local"));
    assert!(file.after.contains("disable-model-invocation: true"));
}

#[test]
fn unavailable_skill_file_does_not_claim_a_merged_content() {
    let files = merge_file_maps(
        None,
        BTreeMap::from([("SKILL.md".to_string(), b"local\n".to_vec())]),
        BTreeMap::from([("SKILL.md".to_string(), b"incoming\n".to_vec())]),
        "",
        "/tmp/demo",
        SkillVisibility::Auto,
        AgentKind::Shared,
    );

    let file = files.first().expect("unavailable skill file");
    assert_eq!(file.status, "unavailable");
    assert!(file.after.is_empty());
    assert!(file.after_bytes.is_none());
    assert!(!file.after_exists);
    assert!(
        file.reason
            .as_deref()
            .unwrap()
            .contains("previous source version")
    );
}

#[test]
fn provider_visibility_is_not_a_skill_content_conflict() {
    let base = "---\nname: demo\ndescription: Old\n---\n\n# same\n";
    let local =
        "---\nname: demo\ndescription: Local\ndisable-model-invocation: true\n---\n\n# same\n";
    let incoming = "---\nname: demo\ndescription: Remote\n---\n\n# same\n";

    let files = merge_file_maps(
        Some(BTreeMap::from([(
            "SKILL.md".to_string(),
            base.as_bytes().to_vec(),
        )])),
        BTreeMap::from([("SKILL.md".to_string(), local.as_bytes().to_vec())]),
        BTreeMap::from([("SKILL.md".to_string(), incoming.as_bytes().to_vec())]),
        "",
        "/tmp/demo",
        SkillVisibility::Manual,
        AgentKind::Claude,
    );

    let file = files.first().expect("conflicting skill file");
    assert_eq!(file.status, "conflict");
    assert_eq!(
        file.after.matches("disable-model-invocation: true").count(),
        1
    );
    assert_eq!(file.after.matches("visibility: manual").count(), 0);
    let conflict_start = file.after.find("<<<<<<< local").expect("conflict start");
    let conflict_end = file.after.find(">>>>>>> remote").expect("conflict end");
    let conflict = &file.after[conflict_start..conflict_end];
    assert!(!conflict.contains("visibility:"));
    assert!(!conflict.contains("disable-model-invocation:"));
}

#[test]
fn shared_provider_visibility_is_not_a_skill_content_conflict() {
    let base = "---\nname: demo\ndescription: Old\n---\n\n# same\n";
    let local =
        "---\nname: demo\ndescription: Local\ndisable-model-invocation: true\n---\n\n# same\n";
    let incoming = "---\nname: demo\ndescription: Remote\n---\n\n# same\n";
    for agent in [AgentKind::Shared, AgentKind::Unknown] {
        let files = merge_file_maps(
            Some(BTreeMap::from([(
                "SKILL.md".to_string(),
                base.as_bytes().to_vec(),
            )])),
            BTreeMap::from([("SKILL.md".to_string(), local.as_bytes().to_vec())]),
            BTreeMap::from([("SKILL.md".to_string(), incoming.as_bytes().to_vec())]),
            "",
            "/tmp/demo",
            SkillVisibility::Auto,
            agent,
        );

        let file = files.first().expect("conflicting skill file");
        assert_eq!(file.status, "conflict");
        assert_eq!(
            file.after.matches("disable-model-invocation: true").count(),
            1
        );
        let conflict_start = file.after.find("<<<<<<< local").expect("conflict start");
        let conflict_end = file.after.find(">>>>>>> remote").expect("conflict end");
        let conflict = &file.after[conflict_start..conflict_end];
        assert!(!conflict.contains("disable-model-invocation:"));
    }
}

#[test]
fn shared_skill_merge_skips_managed_provider_files() {
    let base = "interface:\n  display_name: \"Better Typography\"\n  short_description: \"Web typography from fonts to spacing and wrapping\"\npolicy:\n  allow_implicit_invocation: true\n";
    let local = "interface:\n  display_name: \"Better Typography\"\n  short_description: \"Web typography from fonts to spacing and wrapping\"\npolicy:\n  allow_implicit_invocation: false\n";
    let incoming = "interface:\n  display_name: \"Better Typography\"\n  short_description: \"Fonts, type scales, spacing and wrapping\"\npolicy:\n  allow_implicit_invocation: true\n";

    let files = merge_file_maps(
        Some(BTreeMap::from([(
            "agents/openai.yaml".to_string(),
            base.as_bytes().to_vec(),
        )])),
        BTreeMap::from([("agents/openai.yaml".to_string(), local.as_bytes().to_vec())]),
        BTreeMap::from([(
            "agents/openai.yaml".to_string(),
            incoming.as_bytes().to_vec(),
        )]),
        "",
        "/tmp/demo",
        SkillVisibility::Manual,
        AgentKind::Shared,
    );

    assert!(files.is_empty());
}

#[test]
fn identical_binary_skill_files_do_not_report_an_update() {
    let base = vec![0x89, b'P', b'N', b'G', 0, 0xff];
    let changed = vec![0x89, b'P', b'N', b'G', 0, 0xfe];
    let files = merge_file_maps(
        Some(BTreeMap::from([(
            "assets/icon.png".to_string(),
            base.clone(),
        )])),
        BTreeMap::from([("assets/icon.png".to_string(), base.clone())]),
        BTreeMap::from([("assets/icon.png".to_string(), base.clone())]),
        "",
        "/tmp/demo",
        SkillVisibility::Auto,
        AgentKind::Shared,
    );

    assert!(files.is_empty());

    let files = merge_file_maps(
        Some(BTreeMap::from([(
            "assets/icon.png".to_string(),
            base.clone(),
        )])),
        BTreeMap::from([("assets/icon.png".to_string(), base)]),
        BTreeMap::from([("assets/icon.png".to_string(), changed)]),
        "",
        "/tmp/demo",
        SkillVisibility::Auto,
        AgentKind::Shared,
    );

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].status, "binary");
}

#[test]
fn registry_update_prefers_local_visibility_for_remote_visibility_change() {
    let root = temp_dir("tendi-registry-visibility-merge-test");
    let skill_dir = root.join("skills/demo");
    let registry_file = root.join("registry-demo.md");
    fs::create_dir_all(&skill_dir).unwrap();
    let base = "---\nname: demo\ndescription: Old\n---\n\n# old\n";
    let local = "---\nname: demo\ndescription: Old\n---\n\n# old\n";
    let incoming = "---\nname: demo\ndescription: New\n---\n\n# new\n";
    let expected = "---\nname: demo\ndescription: New\n---\n\n# new\n";
    fs::write(skill_dir.join("SKILL.md"), local).unwrap();
    fs::write(&registry_file, incoming).unwrap();

    let store = crate::storage::Store::open(root.join("tendi.sqlite3")).unwrap();
    store
        .replace_skill_snapshots_for_workspace(
            &root,
            &[SkillSnapshot {
                skill_path: skill_dir.clone(),
                source_version: "1".to_string(),
                files: vec![SkillSnapshotFile {
                    relative_path: "SKILL.md".to_string(),
                    content: base.as_bytes().to_vec(),
                }],
            }],
        )
        .unwrap();

    let mut skill = test_skill("demo", "Demo", &skill_dir);
    let path = &mut skill.paths[0];
    path.source_kind = "registry".to_string();
    path.source = Some(format!("file://{}", registry_file.display()));
    path.source_version = Some("1".to_string());
    path.tendi_visibility = Some(SkillVisibility::Manual);
    path.effective_visibility = SkillVisibility::Manual;

    let path = path.clone();
    let result = plan_registry_update(&skill, &path, &store, Some(&root), "2").unwrap();
    match result {
        Some(RegistryUpdatePlan::Change(change, _)) => {
            assert_eq!(change.after, expected);
        }
        Some(RegistryUpdatePlan::Issue(issue)) => {
            panic!("unexpected merge issue: {}", issue.path.display())
        }
        None => panic!("expected registry update"),
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn registry_provider_visibility_is_not_a_skill_content_conflict() {
    let root = temp_dir("tendi-registry-provider-visibility-merge-test");
    let skill_dir = root.join("skills/demo");
    let registry_file = root.join("registry-demo.md");
    fs::create_dir_all(&skill_dir).unwrap();
    let base = "---\nname: demo\ndescription: Old\n---\n\n# same\n";
    let local = "---\nname: demo\ndescription: Local\n---\n\n# same\n";
    let incoming = "---\nname: demo\ndescription: Remote\n---\n\n# same\n";
    fs::write(skill_dir.join("SKILL.md"), local).unwrap();
    fs::write(&registry_file, incoming).unwrap();

    let store = crate::storage::Store::open(root.join("tendi.sqlite3")).unwrap();
    store
        .replace_skill_snapshots(&[SkillSnapshot {
            skill_path: skill_dir.clone(),
            source_version: "1".to_string(),
            files: vec![SkillSnapshotFile {
                relative_path: "SKILL.md".to_string(),
                content: base.as_bytes().to_vec(),
            }],
        }])
        .unwrap();

    let mut skill = test_skill("demo", "Demo", &skill_dir);
    let path = &mut skill.paths[0];
    path.agent = AgentKind::Claude;
    path.source_kind = "registry".to_string();
    path.source = Some(format!("file://{}", registry_file.display()));
    path.source_version = Some("1".to_string());
    path.tendi_visibility = Some(SkillVisibility::Manual);
    path.effective_visibility = SkillVisibility::Manual;

    let path = path.clone();
    let result = plan_registry_update(&skill, &path, &store, None, "2").unwrap();
    match result {
        Some(RegistryUpdatePlan::Issue(issue)) => {
            assert_eq!(issue.status, "conflict");
            assert_eq!(
                issue
                    .after
                    .matches("disable-model-invocation: true")
                    .count(),
                1
            );
            let conflict_start = issue.after.find("<<<<<<< local").expect("conflict start");
            let conflict_end = issue.after.find(">>>>>>> remote").expect("conflict end");
            let conflict = &issue.after[conflict_start..conflict_end];
            assert!(!conflict.contains("visibility:"));
            assert!(!conflict.contains("disable-model-invocation:"));
        }
        Some(RegistryUpdatePlan::Change(_, _)) => {
            panic!("expected content conflict")
        }
        None => panic!("expected registry update"),
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn merge_skill_reports_mixed_visibility_for_hybrid_sources() {
    let shared_path = test_skill_path(
        "/tmp/tendi-skills/hybrid",
        AgentKind::Shared,
        SkillVisibility::Auto,
        None,
    );
    let mut plugin_path = test_skill_path(
        "/tmp/tendi-skills/hybrid",
        AgentKind::Codex,
        SkillVisibility::Off,
        Some(false),
    );
    plugin_path.root =
        PathBuf::from("/tmp/codex/plugins/cache/openai-bundled/browser/1.0.0/skills");

    let skill = super::merge_skill(
        "hybrid".to_string(),
        vec![
            super::RawSkill {
                name: "hybrid".to_string(),
                description: Some("Hybrid".to_string()),
                tags: Vec::new(),
                dependencies: Vec::new(),
                dependency_files: Vec::new(),
                is_wrapper: false,
                is_system: false,
                path: shared_path,
            },
            super::RawSkill {
                name: "hybrid".to_string(),
                description: Some("Hybrid".to_string()),
                tags: Vec::new(),
                dependencies: Vec::new(),
                dependency_files: Vec::new(),
                is_wrapper: false,
                is_system: true,
                path: plugin_path,
            },
        ],
    );

    assert_eq!(skill.visibility, SkillVisibility::Mixed);
    assert!(skill.paths.iter().any(|path| {
        path.agent == AgentKind::Shared && path.effective_visibility == SkillVisibility::Auto
    }));
    assert!(skill.paths.iter().any(|path| {
        path.agent == AgentKind::Codex && path.effective_visibility == SkillVisibility::Off
    }));
}

#[test]
fn skill_location_id_is_stable_without_content_metadata() {
    let root = temp_dir("tendi-skill-location-id-stability");
    let skill_dir = root.join("skills/demo");
    fs::create_dir_all(&skill_dir).unwrap();
    let mut path = test_skill_path(
        skill_dir.to_str().unwrap(),
        AgentKind::Shared,
        SkillVisibility::Auto,
        None,
    );

    let id = super::skill_location_id(&path);
    let canonical_path = skill_dir
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(id.contains(&canonical_path));

    path.sha256 = "changed-content".to_string();
    path.source_version = Some("changed-source-version".to_string());
    path.update_status = "update-available".to_string();
    assert_eq!(id, super::skill_location_id(&path));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_location_id_distinguishes_agent_and_scope() {
    let path = "/tmp/tendi-skill-location-id-distinction/demo";
    let shared_global = test_skill_path(path, AgentKind::Shared, SkillVisibility::Auto, None);
    let mut codex_global = shared_global.clone();
    codex_global.agent = AgentKind::Codex;
    let mut shared_project = shared_global.clone();
    shared_project.scope = "project".to_string();

    assert_ne!(
        super::skill_location_id(&shared_global),
        super::skill_location_id(&codex_global)
    );
    assert_ne!(
        super::skill_location_id(&shared_global),
        super::skill_location_id(&shared_project)
    );
}

#[test]
fn skill_scan_finds_unique_location_by_id() {
    let path = test_skill_path(
        "/tmp/tendi-skill-location-lookup/demo",
        AgentKind::Shared,
        SkillVisibility::Auto,
        None,
    );
    let location_id = super::skill_location_id(&path);
    let mut skill = test_skill("demo", "Demo", &path.path);
    skill.paths = vec![path.clone()];
    let scan = SkillScan {
        roots: Vec::new(),
        skills: vec![skill],
        warnings: Vec::new(),
    };

    let (found_skill, found_path) = scan
        .find_skill_location_by_id(&location_id)
        .expect("location should be found");
    assert_eq!(found_skill.name, "demo");
    assert_eq!(found_path.path, path.path);
    assert!(scan.find_skill_location_by_id("missing-location").is_none());

    let mut duplicate_skill = test_skill("demo", "Demo", &path.path);
    duplicate_skill.paths = vec![path.clone(), path];
    let duplicate_scan = SkillScan {
        roots: Vec::new(),
        skills: vec![duplicate_skill],
        warnings: Vec::new(),
    };
    assert!(
        duplicate_scan
            .find_skill_location_by_id(&location_id)
            .is_none()
    );
}

#[test]
fn merge_raw_skills_keeps_project_and_global_same_name_separate() {
    let mut global = test_skill_path(
        "/tmp/home/.agents/skills/domain-modeling",
        AgentKind::Shared,
        SkillVisibility::Auto,
        None,
    );
    global.root = PathBuf::from("/tmp/home/.agents/skills");
    global.scope = "global".to_string();

    let mut project = test_skill_path(
        "/tmp/repos/tutti/.codex/skills/domain-modeling",
        AgentKind::Codex,
        SkillVisibility::Manual,
        None,
    );
    project.root = PathBuf::from("/tmp/repos/tutti/.codex/skills");
    project.scope = "project".to_string();

    let skills = super::merge_raw_skills(vec![
        super::RawSkill {
            name: "domain-modeling".to_string(),
            description: Some("Global".to_string()),
            tags: Vec::new(),
            dependencies: Vec::new(),
            dependency_files: Vec::new(),
            is_wrapper: false,
            is_system: false,
            path: global,
        },
        super::RawSkill {
            name: "domain-modeling".to_string(),
            description: Some("Project".to_string()),
            tags: Vec::new(),
            dependencies: Vec::new(),
            dependency_files: Vec::new(),
            is_wrapper: false,
            is_system: false,
            path: project,
        },
    ]);

    assert_eq!(skills.len(), 2);
    let global_skill = skills
        .iter()
        .find(|skill| skill.paths.iter().all(|path| path.scope == "global"))
        .unwrap();
    let project_skill = skills
        .iter()
        .find(|skill| skill.paths.iter().all(|path| path.scope == "project"))
        .unwrap();
    assert_eq!(global_skill.name, "domain-modeling");
    assert_eq!(
        global_skill.id,
        "skill@path:/tmp/home/.agents/skills/domain-modeling"
    );
    assert_eq!(global_skill.visibility, SkillVisibility::Auto);
    assert_eq!(project_skill.name, "domain-modeling");
    assert_eq!(
        project_skill.id,
        "skill@path:/tmp/repos/tutti/.codex/skills/domain-modeling"
    );
    assert_eq!(project_skill.visibility, SkillVisibility::Manual);
    assert!(
        skills
            .iter()
            .all(|skill| skill.visibility != SkillVisibility::Mixed)
    );
}

#[test]
fn merge_raw_skills_keeps_same_name_direct_installations_separate() {
    let root = temp_dir("tendi-direct-skill-installations");
    let first = root.join("first/pr");
    let second = root.join("second/pr");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    fs::write(first.join("SKILL.md"), "---\nname: pr\n---\n\n# first\n").unwrap();
    fs::write(second.join("SKILL.md"), "---\nname: pr\n---\n\n# second\n").unwrap();

    let skills = super::merge_raw_skills(vec![
        raw_skill_for_path(
            "pr",
            test_skill_path(
                first.to_str().unwrap(),
                AgentKind::Shared,
                SkillVisibility::Auto,
                None,
            ),
        ),
        raw_skill_for_path(
            "pr",
            test_skill_path(
                second.to_str().unwrap(),
                AgentKind::Claude,
                SkillVisibility::Auto,
                None,
            ),
        ),
    ]);

    assert_eq!(skills.len(), 2);
    assert!(skills.iter().all(|skill| skill.paths.len() == 1));
    assert_ne!(skills[0].id, skills[1].id);
    assert_ne!(skills[0].installation_id, skills[1].installation_id);
    assert!(
        skills
            .iter()
            .all(|skill| skill.id.starts_with("skill@path:"))
    );
    assert!(skills.iter().all(|skill| skill.id == skill.installation_id));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn merge_raw_skills_keeps_same_content_copies_separate() {
    let root = temp_dir("tendi-copy-skill-installations");
    let first = root.join("first/pr");
    let second = root.join("second/pr");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    let content = "---\nname: pr\ndescription: same\n---\n\n# same\n";
    fs::write(first.join("SKILL.md"), content).unwrap();
    fs::write(second.join("SKILL.md"), content).unwrap();

    let mut first_path = test_skill_path(
        first.to_str().unwrap(),
        AgentKind::Shared,
        SkillVisibility::Auto,
        None,
    );
    let mut second_path = test_skill_path(
        second.to_str().unwrap(),
        AgentKind::Shared,
        SkillVisibility::Auto,
        None,
    );
    first_path.source_kind = "github".to_string();
    first_path.source = Some("https://example.com/skills.git".to_string());
    second_path.source_kind = first_path.source_kind.clone();
    second_path.source = first_path.source.clone();

    let skills = super::merge_raw_skills(vec![
        raw_skill_for_path("pr", first_path),
        raw_skill_for_path("pr", second_path),
    ]);

    assert_eq!(skills.len(), 2);
    assert_ne!(skills[0].id, skills[1].id);
    assert_ne!(skills[0].installation_id, skills[1].installation_id);
    assert_eq!(skills[0].paths[0].sha256, skills[1].paths[0].sha256);

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn merge_raw_skills_combines_symlinks_to_same_target_and_keeps_id_stable() {
    let root = temp_dir("tendi-linked-skill-installations");
    let source = root.join("source/pr");
    let first = root.join(".agents/skills/pr");
    let second = root.join(".claude/skills/pr");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(first.parent().unwrap()).unwrap();
    fs::create_dir_all(second.parent().unwrap()).unwrap();
    fs::write(source.join("SKILL.md"), "---\nname: pr\n---\n\n# shared\n").unwrap();
    std::os::unix::fs::symlink(&source, &first).unwrap();
    std::os::unix::fs::symlink(&source, &second).unwrap();

    let first_path = test_skill_path(
        first.to_str().unwrap(),
        AgentKind::Shared,
        SkillVisibility::Auto,
        None,
    );
    let second_path = test_skill_path(
        second.to_str().unwrap(),
        AgentKind::Claude,
        SkillVisibility::Manual,
        None,
    );
    let single_first_id =
        super::merge_raw_skills(vec![raw_skill_for_path("pr", first_path.clone())])[0]
            .id
            .clone();
    let single_second_id =
        super::merge_raw_skills(vec![raw_skill_for_path("pr", second_path.clone())])[0]
            .id
            .clone();
    let combined = super::merge_raw_skills(vec![
        raw_skill_for_path("pr", second_path),
        raw_skill_for_path("pr", first_path),
    ]);

    assert_eq!(combined.len(), 1);
    assert_eq!(combined[0].paths.len(), 2);
    assert_eq!(single_first_id, single_second_id);
    assert_eq!(combined[0].id, single_first_id);
    assert_eq!(combined[0].installation_id, combined[0].id);
    assert_eq!(
        super::canonical_skill_dir(&combined[0].paths[0].path),
        super::canonical_skill_dir(&combined[0].paths[1].path)
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_delete_plan_removes_selected_skill_directory() {
    let root = temp_dir("tendi-delete-skill-test");
    let skill_dir = root.join(".agents/skills/delete-demo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: delete-demo\ndescription: Demo\n---\n\n# delete-demo\n",
    )
    .unwrap();

    let scan = super::scan_skills_without_source_database(&root).unwrap();
    let skill_id = scan
        .skills
        .iter()
        .find(|skill| skill.name == "delete-demo")
        .map(|skill| skill.id.clone())
        .unwrap();
    let plan = super::plan_skill_delete_many_for_scan(&scan, &[skill_id]).unwrap();
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(plan.targets[0].name, "delete-demo");
    assert_eq!(plan.targets[0].kind, "directory");
    assert!(format_delete_plan(&plan).contains("D delete-demo"));

    apply_skill_delete_plan(&plan).unwrap();
    assert!(!skill_dir.exists());
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn symlinked_local_skill_reports_canonical_source() {
    let root = std::env::temp_dir().join(format!(
        "tendi-scan-symlink-source-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let source_dir = root.join(".agents/skills/demo");
    let target_root = root.join(".claude/skills");
    let target_dir = target_root.join("demo");
    fs::create_dir_all(&source_dir).unwrap();
    fs::create_dir_all(&target_root).unwrap();
    fs::write(
        source_dir.join("SKILL.md"),
        "---\nname: demo\ndescription: Demo\n---\n\n# demo\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&source_dir, &target_dir).unwrap();

    let scan = scan_skills(&root).unwrap();
    let skill = scan
        .skills
        .iter()
        .find(|skill| skill.name == "demo")
        .unwrap();
    let claude_path = skill
        .paths
        .iter()
        .find(|path| path.agent == AgentKind::Claude)
        .unwrap();

    assert_eq!(claude_path.symlink_status, "symlink-ok");
    assert_eq!(
        claude_path.source.as_deref(),
        Some(source_dir.canonicalize().unwrap().to_str().unwrap())
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn project_skills_cli_lock_imports_once_into_source_database() {
    let root = temp_dir("tendi-project-skills-cli-lock-test");
    let skill_dir = root.join(".agents/skills/demo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: demo\ndescription: Demo\n---\n\n# demo\n",
    )
    .unwrap();
    fs::write(
        root.join("skills-lock.json"),
        r#"{
  "version": 1,
  "skills": {
    "demo": {
      "source": "example/agent-skills",
      "sourceType": "github",
      "ref": "release",
      "skillPath": "skills/demo/SKILL.md",
      "computedHash": "content-hash"
    }
  }
}
"#,
    )
    .unwrap();
    run_test_git(&root, &["init"]);
    run_test_git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/host-project.git",
        ],
    );

    let store = crate::storage::Store::open(root.join("tendi.sqlite3")).unwrap();
    crate::initialize_workspace(&store, &root, &[]).unwrap();
    let scanned =
        super::scan_skills_synced_for_project_roots_with_store_for_projection(&root, &store, &[])
            .unwrap();
    let path = &scanned
        .skills
        .iter()
        .find(|skill| skill.name == "demo")
        .unwrap()
        .paths[0];
    assert_eq!(path.source_kind, "github");
    assert_eq!(
        path.source.as_deref(),
        Some("https://github.com/example/agent-skills.git")
    );
    assert_eq!(
        path.source_relative_path.as_deref(),
        Some("skills/demo/SKILL.md")
    );
    assert_eq!(path.source_version.as_deref(), Some("content-hash"));
    assert_eq!(path.update_status, "tracked");

    let records = store.skill_source_records_for_workspace(&root).unwrap();
    let imported = records
        .iter()
        .filter(|record| record.skill_name == "demo")
        .collect::<Vec<_>>();
    assert_eq!(imported.len(), 1, "{records:#?}");
    assert_eq!(imported[0].origin, "skills-cli-lock");
    assert_eq!(imported[0].source_ref.as_deref(), Some("release"));

    fs::write(
        root.join("skills-lock.json"),
        r#"{
  "version": 1,
  "skills": {
    "demo": {
      "source": "example/changed-after-import",
      "sourceType": "github",
      "skillPath": "skills/changed/SKILL.md",
      "computedHash": "changed-hash"
    }
  }
}
"#,
    )
    .unwrap();
    let rescanned = super::scan_skills_with_source_store(&root, &store).unwrap();
    let persisted_path = &rescanned
        .skills
        .iter()
        .find(|skill| skill.name == "demo")
        .unwrap()
        .paths[0];
    assert_eq!(
        persisted_path.source.as_deref(),
        Some("https://github.com/example/agent-skills.git")
    );
    assert_eq!(
        persisted_path.source_relative_path.as_deref(),
        Some("skills/demo/SKILL.md")
    );
    assert_eq!(
        persisted_path.source_version.as_deref(),
        Some("content-hash")
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn scan_skills_includes_additional_project_roots() {
    let cwd = temp_dir("tendi-skills-scan-cwd");
    let project = temp_dir("tendi-skills-scan-project");
    let skill_dir = project.join(".agents/skills/project-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: project-skill\ndescription: Project skill\n---\n",
    )
    .unwrap();

    let store = crate::storage::Store::open(cwd.join("tendi.sqlite3")).unwrap();
    let scan = super::scan_skills_with_source_store_for_projects(
        &cwd,
        &store,
        std::slice::from_ref(&project),
    )
    .unwrap();

    let skill = scan
        .skills
        .iter()
        .find(|skill| skill.name == "project-skill")
        .unwrap();
    let skill_dir = skill_dir.canonicalize().unwrap();
    assert!(
        skill.paths.iter().any(|path| path.path == skill_dir),
        "skill paths: {:#?}; expected: {}",
        skill.paths,
        skill_dir.display()
    );
    store.save_skills_for_workspace(&cwd, &scan).unwrap();
    let persisted = store.list_skills_for_workspace(&cwd).unwrap().unwrap();
    assert!(
        persisted
            .skills
            .iter()
            .any(|skill| skill.name == "project-skill")
    );

    let _ = fs::remove_dir_all(cwd);
    let _ = fs::remove_dir_all(project);
}

#[test]
fn project_repository_skill_is_local_without_external_provenance() {
    let cwd = temp_dir("tendi-skills-scan-repository-cwd");
    let project = temp_dir("tendi-skills-scan-repository-project");
    let skill_dir = project.join(".agents/skills/project-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: project-skill\ndescription: Project skill\n---\n",
    )
    .unwrap();
    run_test_git(&project, &["init", "--quiet"]);
    run_test_git(
        &project,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/project-skills.git",
        ],
    );
    let canonical_skill_dir = skill_dir.canonicalize().unwrap();

    let store = crate::storage::Store::open(cwd.join("tendi.sqlite3")).unwrap();
    let scan = super::scan_skills_with_source_store_for_projects(
        &cwd,
        &store,
        std::slice::from_ref(&project),
    )
    .unwrap();
    let skill = scan
        .skills
        .iter()
        .find(|skill| skill.name == "project-skill")
        .unwrap();
    let path = skill
        .paths
        .iter()
        .find(|path| path.path == canonical_skill_dir)
        .unwrap();

    assert_eq!(path.source_kind, "local");
    assert_eq!(path.update_status, "local");
    assert_eq!(
        path.source.as_deref(),
        Some(canonical_skill_dir.to_str().unwrap())
    );
    assert_eq!(skill.update_status, "local");

    let update = super::check_skill_update(
        skill,
        &BTreeMap::new(),
        &BTreeMap::new(),
        super::git::never_cancelled(),
    );
    assert_eq!(update.status, "local");

    let _ = fs::remove_dir_all(cwd);
    let _ = fs::remove_dir_all(project);
}

#[test]
fn existing_source_database_record_does_not_read_lock_file() {
    let root = temp_dir("tendi-source-database-authority-test");
    let skill_dir = root.join(".agents/skills/demo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(root.join("skills-lock.json"), "not valid json").unwrap();
    let record = super::SkillSourceRecord {
        skill_name: "demo".to_string(),
        skill_path: skill_dir.clone(),
        source_kind: "github".to_string(),
        source: Some("https://github.com/example/database-source.git".to_string()),
        source_ref: Some("main".to_string()),
        source_version: Some("database-hash".to_string()),
        source_relative_path: Some("skills/demo/SKILL.md".to_string()),
        update_status: "tracked".to_string(),
        origin: "skills-cli-lock".to_string(),
    };
    let mut resolver = super::ProvenanceResolver::managed(&root, vec![record], &[]);

    let provenance = resolver.infer_installed(
        &skill_dir,
        &skill_dir,
        &root.join(".agents/skills"),
        "project",
        "demo",
        &None,
    );

    assert_eq!(
        provenance.source.as_deref(),
        Some("https://github.com/example/database-source.git")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_path_selection_treats_tracked_paths_as_checkable() {
    let mut skill = test_skill("demo", "Demo", Path::new("/tmp/demo"));
    skill.paths[0].update_status = "tracked".to_string();

    assert_eq!(select_update_path(&skill).unwrap().update_status, "tracked");
}

#[test]
fn update_path_selection_prefers_checkable_or_tracked_over_local() {
    let mut skill = test_skill("demo", "Demo", Path::new("/tmp/demo"));
    let mut tracked = skill.paths[0].clone();
    tracked.path = PathBuf::from("/tmp/demo-tracked");
    tracked.update_status = "tracked".to_string();
    skill.paths[0].update_status = "local".to_string();
    skill.paths.push(tracked);

    assert_eq!(select_update_path(&skill).unwrap().update_status, "tracked");
}

#[test]
fn dirty_refresh_parses_only_changed_installation_and_handles_delete_and_add() {
    let temp = temp_dir("tendi-dirty-targeted-refresh");
    fs::create_dir(&temp).unwrap();
    let root = temp.canonicalize().unwrap();
    let install_root = root.join("skills");
    let first = install_root.join("first");
    let second = install_root.join("second");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(second.join("SKILL.md")).unwrap();
    fs::write(
        first.join("SKILL.md"),
        "---\nname: first\ndescription: updated\n---\nBody\n",
    )
    .unwrap();
    let store = crate::storage::Store::open(root.join("test.sqlite3")).unwrap();
    let mut skills = vec![
        test_skill("first", "old", &first),
        test_skill("second", "untouched", &second),
    ];
    for skill in &mut skills {
        skill.paths[0].agent = AgentKind::Unknown;
        skill.paths[0].scope = "project".into();
        skill.agents = vec![AgentKind::Unknown];
    }
    let scan = SkillScan {
        roots: vec![super::SkillRoot {
            path: install_root.clone(),
            scope: "project".into(),
            agent: AgentKind::Unknown,
            plugin_id: None,
            plugin_enabled: None,
        }],
        skills,
        warnings: vec![],
    };
    let scan = super::refresh_dirty_skill_projection(
        &root,
        &store,
        scan,
        &[first.join("SKILL.md")],
        false,
        &[],
    )
    .unwrap();
    assert_eq!(
        scan.skills
            .iter()
            .find(|skill| skill.name == "first")
            .unwrap()
            .description
            .as_deref(),
        Some("updated")
    );
    assert_eq!(
        scan.skills
            .iter()
            .find(|skill| skill.name == "second")
            .unwrap()
            .description
            .as_deref(),
        Some("untouched")
    );
    fs::remove_dir_all(&first).unwrap();
    let scan =
        super::refresh_dirty_skill_projection(&root, &store, scan, &[first], false, &[]).unwrap();
    assert_eq!(scan.skills.len(), 1);
    let third = install_root.join("third");
    fs::create_dir(&third).unwrap();
    fs::write(
        third.join("SKILL.md"),
        "---\nname: third\ndescription: new\n---\nBody\n",
    )
    .unwrap();
    let scan = super::refresh_dirty_skill_projection(
        &root,
        &store,
        scan,
        &[third.join("SKILL.md")],
        false,
        &[],
    )
    .unwrap();
    assert_eq!(scan.skills.len(), 2);
    assert!(scan.skills.iter().any(|skill| skill.name == "third"));
    drop(store);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn dirty_targets_include_real_dependents_but_not_unrelated_installations() {
    let temp = temp_dir("tendi-dirty-dependents");
    fs::create_dir(&temp).unwrap();
    let mut first = test_skill("first", "first", &temp.join("first"));
    first.dependent_ids.push("wrapper".into());
    let scan = SkillScan {
        roots: vec![],
        warnings: vec![],
        skills: vec![
            first,
            test_skill("wrapper", "wrapper", &temp.join("wrapper")),
            test_skill("other", "other", &temp.join("other")),
        ],
    };
    let (ids, _) = super::dirty_skill_targets(&scan, &[temp.join("first/SKILL.md")]).unwrap();
    assert_eq!(ids, vec!["first", "wrapper"]);
    fs::remove_dir_all(temp).unwrap();
}

#[cfg(unix)]
#[test]
fn dirty_physical_paths_keep_the_provider_owned_logical_installation() {
    let temp = temp_dir("tendi-dirty-provider-alias");
    fs::create_dir(&temp).unwrap();
    let root = temp.canonicalize().unwrap();
    let physical = root.join("physical");
    fs::create_dir_all(physical.join("demo")).unwrap();
    fs::write(physical.join("demo/SKILL.md"), "demo").unwrap();
    let logical = root.join("provider-root");
    create_symlink(&physical, &logical).unwrap();
    let scan = SkillScan {
        roots: vec![super::SkillRoot {
            path: logical.clone(),
            scope: "project".into(),
            agent: AgentKind::Codex,
            plugin_id: None,
            plugin_enabled: None,
        }],
        skills: vec![],
        warnings: vec![],
    };
    let (_, directories) =
        super::dirty_skill_targets(&scan, &[physical.join("demo/SKILL.md")]).unwrap();
    assert_eq!(directories, vec![logical.join("demo")]);
    fs::remove_dir_all(temp).unwrap();
}

fn test_skill(name: &str, description: &str, path: &Path) -> SkillRecord {
    SkillRecord {
        id: name.to_string(),
        installation_id: name.to_string(),
        name: name.to_string(),
        description: Some(description.to_string()),
        tags: Vec::new(),
        dependencies: Vec::new(),
        dependents: Vec::new(),
        dependency_ids: Vec::new(),
        dependent_ids: Vec::new(),
        is_wrapper: false,
        visibility: SkillVisibility::Auto,
        agents: vec![AgentKind::Shared],
        paths: vec![SkillPath {
            path: PathBuf::from(path),
            root: path.parent().unwrap_or(path).to_path_buf(),
            scope: "global".to_string(),
            agent: AgentKind::Shared,
            install_target: "shared:/tmp/tendi-skills".to_string(),
            source_kind: "local".to_string(),
            source: None,
            source_ref: None,
            source_version: None,
            source_relative_path: None,
            symlink_status: "direct".to_string(),
            update_status: "local".to_string(),
            sha256: "sha".to_string(),
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
        install_targets: vec!["shared:/tmp/tendi-skills".to_string()],
        update_status: "local".to_string(),
        is_system: false,
        ctime: None,
        mtime: None,
    }
}

fn raw_skill_for_path(name: &str, path: SkillPath) -> super::RawSkill {
    super::RawSkill {
        name: name.to_string(),
        description: Some(name.to_string()),
        tags: Vec::new(),
        dependencies: Vec::new(),
        dependency_files: Vec::new(),
        is_wrapper: false,
        is_system: false,
        path,
    }
}

#[test]
fn skill_backup_exclusion_reason_stays_provider_owned() {
    let codex_plugin = test_skill_path(
        "/tmp/.codex/plugins/browser/skills/demo",
        AgentKind::Codex,
        SkillVisibility::Off,
        Some(false),
    );
    let cursor_plugin = test_skill_path(
        "/tmp/.cursor/plugins/browser/skills/demo",
        AgentKind::Cursor,
        SkillVisibility::Off,
        Some(false),
    );

    assert_eq!(
        skill_backup_exclusion_reason(std::slice::from_ref(&codex_plugin)),
        Some("plugin-skill")
    );
    assert_eq!(
        skill_backup_exclusion_reason(std::slice::from_ref(&cursor_plugin)),
        None
    );
}

fn test_skill_path(
    path: &str,
    agent: AgentKind,
    effective_visibility: SkillVisibility,
    plugin_enabled: Option<bool>,
) -> SkillPath {
    let path = PathBuf::from(path);
    let root = path.parent().unwrap_or(&path).to_path_buf();
    SkillPath {
        path,
        root: root.clone(),
        scope: if plugin_enabled.is_some() {
            "plugin".to_string()
        } else {
            "global".to_string()
        },
        agent,
        install_target: format!("{}:{}", agent.label(), root.display()),
        source_kind: "local".to_string(),
        source: None,
        source_ref: None,
        source_version: None,
        source_relative_path: None,
        symlink_status: "direct".to_string(),
        update_status: "local".to_string(),
        sha256: "sha".to_string(),
        tags: Vec::new(),
        tendi_visibility: None,
        effective_visibility,
        provider_allow_implicit_invocation: None,
        provider_skill_enabled: None,
        provider_disable_model_invocation: None,
        plugin_id: plugin_enabled.map(|_| "browser@openai-bundled".to_string()),
        plugin_enabled,
    }
}
