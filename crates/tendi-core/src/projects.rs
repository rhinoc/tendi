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
#[path = "projects_tests.rs"]
mod tests;
