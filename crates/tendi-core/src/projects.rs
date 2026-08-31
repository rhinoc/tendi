use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::Result;
use chrono::Local;
use ignore::{
    DirEntry, WalkBuilder,
    gitignore::{Gitignore, GitignoreBuilder},
};
use serde::{Deserialize, Serialize};

use crate::{fsutil::sha256_text, git};

const SKIPPED_DIRECTORIES: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".turbo",
    "__pycache__",
];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectScanScope {
    pub id: String,
    pub path: PathBuf,
    pub excluded: bool,
    pub enabled: bool,
    pub last_scanned_at: Option<String>,
    pub project_count: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub root_path: PathBuf,
    pub remote_url: Option<String>,
    pub scope_id: String,
    pub status: String,
    pub last_scanned_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectScanResult {
    pub projects: Vec<ProjectRecord>,
    pub scopes: Vec<ProjectScanScope>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedScopePath {
    pub path: PathBuf,
    pub excluded: bool,
}

pub fn scope_id(path: &Path) -> String {
    format!("scope-{}", &sha256_text(&path.to_string_lossy())[..24])
}

pub fn scope_id_for_path(path: &Path, excluded: bool) -> String {
    if excluded {
        scope_id(Path::new(&format!("!{}", path.display())))
    } else {
        scope_id(path)
    }
}

pub fn project_id(path: &Path) -> String {
    format!("project-{}", &sha256_text(&path.to_string_lossy())[..24])
}

pub fn normalize_scope_paths(values: Vec<String>) -> Result<Vec<NormalizedScopePath>> {
    let mut paths = BTreeSet::new();
    let mut matcher = GitignoreBuilder::new(Path::new("/"));
    for value in values {
        for line in value.lines() {
            let value = line.trim();
            if value.is_empty() {
                continue;
            }
            let (excluded, value) = match value.strip_prefix('!') {
                Some(value) => (true, value.trim()),
                None => (false, value),
            };
            let path = if value == "~" {
                dirs::home_dir()
                    .ok_or_else(|| anyhow::anyhow!("could not resolve home directory"))?
            } else if let Some(relative) = value.strip_prefix("~/") {
                dirs::home_dir()
                    .ok_or_else(|| anyhow::anyhow!("could not resolve home directory"))?
                    .join(relative)
            } else {
                PathBuf::from(value)
            };
            if !path.is_absolute() {
                anyhow::bail!("Project scan scope must be an absolute path: {value}");
            }
            if excluded {
                matcher.add_line(None, &path.to_string_lossy())?;
            }
            let encoded = format!("{}{}", if excluded { "!" } else { "" }, path.display());
            paths.insert(encoded);
        }
    }
    matcher.build()?;
    Ok(paths
        .into_iter()
        .map(|value| {
            let (excluded, path) = match value.strip_prefix('!') {
                Some(path) => (true, path),
                None => (false, value.as_str()),
            };
            NormalizedScopePath {
                path: PathBuf::from(path),
                excluded,
            }
        })
        .collect())
}

pub fn build_exclusion_matcher(scopes: &[ProjectScanScope]) -> Result<Gitignore> {
    let mut builder = GitignoreBuilder::new(Path::new("/"));
    for scope in scopes
        .iter()
        .filter(|scope| scope.enabled && scope.excluded)
    {
        builder.add_line(None, &scope.path.to_string_lossy())?;
    }
    Ok(builder.build()?)
}

pub fn path_is_excluded(matcher: &Gitignore, path: &Path, is_dir: bool) -> bool {
    matcher
        .matched_path_or_any_parents(path, is_dir)
        .is_ignore()
}

pub fn scan_scope(
    scope: &Path,
    scope_id: &str,
    exclusion_matcher: &Gitignore,
) -> (Vec<ProjectRecord>, Vec<String>) {
    let mut roots = BTreeSet::new();
    let mut warnings = Vec::new();
    if !scope.is_dir() {
        warnings.push(format!(
            "Project scan scope is not a directory: {}",
            scope.display()
        ));
        return (Vec::new(), warnings);
    }

    let exclusion_matcher = exclusion_matcher.clone();
    let mut walker = WalkBuilder::new(scope);
    walker
        .hidden(false)
        .ignore(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .follow_links(false)
        .filter_entry(move |entry| {
            let is_dir = entry
                .file_type()
                .is_some_and(|file_type| file_type.is_dir());
            !should_skip_directory(entry)
                && !path_is_excluded(&exclusion_matcher, entry.path(), is_dir)
        });
    let entries = walker.build();
    for entry in entries.filter_map(Result::ok) {
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_dir())
        {
            continue;
        }
        let path = entry.path();
        if path.join(".git").is_dir() || path.join(".git").is_file() {
            match path.canonicalize() {
                Ok(root) => {
                    roots.insert(root);
                }
                Err(error) => warnings.push(format!("{}: {error}", path.display())),
            }
        }
    }

    let roots = roots
        .into_iter()
        .map(
            |root| match git::local_repository_snapshot(&root, git::never_cancelled()) {
                Ok(snapshot) => git::logical_repository_root(&snapshot).unwrap_or(root),
                Err(error) => {
                    warnings.push(format!("{}: {error}", root.display()));
                    root
                }
            },
        )
        .collect::<BTreeSet<_>>();
    let scanned_at = Local::now().to_rfc3339();
    let projects = roots
        .into_iter()
        .filter_map(|root| match scan_project(&root, scope_id, &scanned_at) {
            Some(project) => Some(project),
            None => {
                warnings.push(format!(
                    "Project repository has no usable directory name: {}",
                    root.display()
                ));
                None
            }
        })
        .collect();
    (projects, warnings)
}

fn scan_project(root: &Path, scope_id: &str, scanned_at: &str) -> Option<ProjectRecord> {
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())?
        .to_string();
    let remote_url = git::local_repository_snapshot(root, git::never_cancelled())
        .ok()
        .and_then(|snapshot| snapshot.remote_url);

    Some(ProjectRecord {
        id: project_id(root),
        name,
        root_path: root.to_path_buf(),
        remote_url,
        scope_id: scope_id.to_string(),
        status: "ready".to_string(),
        last_scanned_at: scanned_at.to_string(),
    })
}

fn should_skip_directory(entry: &DirEntry) -> bool {
    entry
        .file_type()
        .is_some_and(|file_type| file_type.is_dir())
        && entry.depth() > 0
        && entry
            .file_name()
            .to_str()
            .is_some_and(|name| SKIPPED_DIRECTORIES.contains(&name))
}

#[cfg(test)]
mod tests {
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
        assert!(
            project_roots.contains(&visible_repo.canonicalize().expect("canonical visible repo"))
        );
        assert!(
            !project_roots.contains(&ignored_repo.canonicalize().expect("canonical ignored repo"))
        );

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
}
