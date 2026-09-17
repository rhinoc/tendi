use std::{
    collections::BTreeSet,
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    ProjectScanScope, build_exclusion_matcher, normalize_scope_paths, scan_project, scan_scope,
};

fn temp_root(name: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("tendi-projects-{name}-{suffix}"));
    fs::create_dir_all(&root).expect("create temp root");
    root
}

fn git(root: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .expect("run git");
    assert!(status.success(), "git command failed: {args:?}");
}

#[test]
fn scan_scope_discovers_repositories() {
    let root = temp_root("scan");
    let repo = root.join("demo");
    fs::create_dir_all(repo.join("node_modules/vendor")).expect("create ignored directory");
    git(&repo, &["init", "--quiet"]);

    let exclusions = build_exclusion_matcher(&[]).expect("build exclusions");
    let (projects, warnings) = scan_scope(&root, "scope-test", &exclusions);
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "demo");
    assert_eq!(projects[0].scope_id, "scope-test");

    fs::remove_dir_all(root).expect("remove temp root");
}

#[test]
fn scan_scope_respects_repository_gitignore() {
    let root = temp_root("gitignore");
    let repo = root.join("mailia");
    let ignored_repo = repo.join(".build/checkouts/SwiftSoup");
    let visible_repo = repo.join("packages/Visible");
    fs::create_dir_all(&ignored_repo).expect("create ignored checkout");
    fs::create_dir_all(&visible_repo).expect("create visible checkout");
    git(&repo, &["init", "--quiet"]);
    git(&ignored_repo, &["init", "--quiet"]);
    git(&visible_repo, &["init", "--quiet"]);
    fs::write(repo.join(".gitignore"), ".build/\n").expect("write gitignore");

    let exclusions = build_exclusion_matcher(&[]).expect("build exclusions");
    let (projects, warnings) = scan_scope(&root, "scope-test", &exclusions);
    let project_roots = projects
        .iter()
        .map(|project| project.root_path.clone())
        .collect::<BTreeSet<_>>();

    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    assert!(project_roots.contains(&repo.canonicalize().expect("canonical repo")));
    assert!(project_roots.contains(&visible_repo.canonicalize().expect("canonical visible repo")));
    assert!(!project_roots.contains(&ignored_repo.canonicalize().expect("canonical ignored repo")));

    fs::remove_dir_all(root).expect("remove temp root");
}

#[test]
fn project_scan_drops_roots_without_a_directory_name() {
    assert!(scan_project(std::path::Path::new("/"), "scope-test", "now").is_none());
}

#[test]
fn normalize_scope_paths_supports_bang_prefixed_exclusions() {
    let root = temp_root("normalize");
    let excluded = root.join("archive");

    let scopes =
        normalize_scope_paths(vec![format!("{}\n!{}", root.display(), excluded.display())])
            .expect("normalize scan scopes");

    assert_eq!(scopes.len(), 2);
    assert!(
        scopes
            .iter()
            .any(|scope| { scope.path == root && !scope.excluded })
    );
    assert!(
        scopes
            .iter()
            .any(|scope| { scope.path == excluded && scope.excluded })
    );

    fs::remove_dir_all(root).expect("remove temp root");
}

#[test]
fn exclusion_matcher_supports_gitignore_globs() {
    let root = temp_root("matcher");
    let scopes = normalize_scope_paths(vec![format!("!{}/**/archive", root.display())])
        .expect("normalize scan scopes");
    let project_scope = ProjectScanScope {
        id: "scope-excluded".to_string(),
        path: scopes[0].path.clone(),
        excluded: true,
        enabled: true,
        last_scanned_at: None,
        project_count: 0,
    };
    let matcher = build_exclusion_matcher(&[project_scope]).expect("build exclusions");

    assert!(super::path_is_excluded(
        &matcher,
        &root.join("nested/archive"),
        true
    ));
    assert!(super::path_is_excluded(
        &matcher,
        &root.join("nested/archive/project"),
        true
    ));
    assert!(!super::path_is_excluded(
        &matcher,
        &root.join("nested/keep"),
        true
    ));

    fs::remove_dir_all(root).expect("remove temp root");
}

#[test]
fn scan_scope_deduplicates_linked_worktrees_using_logical_repository_root() {
    let root = temp_root("worktree");
    let repo = root.join("repo");
    let linked = root.join("linked");
    fs::create_dir_all(&repo).expect("create repository");
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["config", "user.email", "test@tendi.invalid"]);
    git(&repo, &["config", "user.name", "Tendi Test"]);
    fs::write(repo.join("README.md"), "seed\n").expect("write seed");
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "--quiet", "-m", "seed"]);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ],
    );

    let exclusions = build_exclusion_matcher(&[]).expect("build exclusions");
    let (projects, warnings) = scan_scope(&root, "scope-test", &exclusions);
    let expected = fs::canonicalize(&repo).expect("canonical repository");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].root_path, expected);
    assert_eq!(projects[0].id, super::project_id(&projects[0].root_path));

    fs::remove_dir_all(root).expect("remove temp root");
}
