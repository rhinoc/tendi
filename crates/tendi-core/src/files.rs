use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::fsutil::{atomic_write, sha256_text};

const SKIPPED_SKILL_PATH_COMPONENTS: &[&str] = &[
    ".git",
    ".gitignore",
    ".gitattributes",
    ".gitmodules",
    ".DS_Store",
    "Thumbs.db",
    "desktop.ini",
    "node_modules",
    ".pnpm-store",
    ".yarn",
    ".npm",
    "out",
    ".build",
    "_build",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".cache",
    ".parcel-cache",
    ".vite",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".tox",
    ".nox",
    ".hypothesis",
    ".pytype",
    ".venv",
    "venv",
    "env",
    ".eggs",
    ".gradle",
    ".bundle",
    ".idea",
    ".vscode",
    "tmp",
    "temp",
];
const SKIPPED_SKILL_PATH_SUFFIXES: &[&str] = &[".pyc", ".pyo", ".class", ".egg-info"];

#[derive(Debug, Clone, Serialize)]
pub struct SkillFileEntry {
    pub path: PathBuf,
    pub relative_path: String,
    pub kind: String,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillFileContent {
    pub path: PathBuf,
    pub relative_path: String,
    pub content: String,
    pub sha256: String,
}

/// Write/create responses omit echoed file bodies; callers already have the bytes.
#[derive(Debug, Clone, Serialize)]
pub struct SkillFileWriteResult {
    pub path: PathBuf,
    pub relative_path: String,
    pub sha256: String,
}

pub fn skill_relative_path_affects_projection(relative_path: &str) -> bool {
    let path = relative_path.replace('\\', "/");
    path == "SKILL.md" || path.starts_with("agents/")
}

pub fn list_skill_files(
    cwd: &Path,
    skill_name: &str,
    cached_skill_dir: Option<&Path>,
) -> Result<Vec<SkillFileEntry>> {
    let skill_dir = resolve_skill_dir(cwd, skill_name, cached_skill_dir)?;
    let mut entries = Vec::new();
    let skill_dir_for_filter = skill_dir.clone();
    let mut walker = WalkBuilder::new(&skill_dir);
    walker
        .hidden(false)
        .parents(true)
        .ignore(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .follow_links(true)
        .max_depth(Some(4))
        .filter_entry(move |entry| !should_skip_skill_path(&skill_dir_for_filter, entry.path()));

    for entry in walker
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.path() != skill_dir)
    {
        let path = entry.into_path();
        let relative_path = path
            .strip_prefix(&skill_dir)
            .unwrap_or(&path)
            .display()
            .to_string();
        if path.is_dir() {
            entries.push(SkillFileEntry {
                path,
                relative_path,
                kind: "folder".to_string(),
                sha256: None,
            });
        } else if path.is_file() {
            entries.push(SkillFileEntry {
                path,
                relative_path,
                kind: "file".to_string(),
                sha256: None,
            });
        }
    }

    entries.sort_by(|a, b| {
        (a.kind.as_str() != "folder")
            .cmp(&(b.kind.as_str() != "folder"))
            .then_with(|| a.relative_path.cmp(&b.relative_path))
    });
    Ok(entries)
}

fn should_skip_skill_path(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).ok().is_some_and(|relative| {
        relative.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(should_skip_skill_path_component)
        })
    })
}

fn should_skip_skill_path_component(name: &str) -> bool {
    SKIPPED_SKILL_PATH_COMPONENTS.contains(&name)
        || SKIPPED_SKILL_PATH_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

pub fn read_skill_file(
    cwd: &Path,
    skill_name: &str,
    relative_path: &str,
    cached_skill_dir: Option<&Path>,
) -> Result<SkillFileContent> {
    let skill_dir = resolve_skill_dir(cwd, skill_name, cached_skill_dir)?;
    let path = safe_join(&skill_dir, relative_path)?;
    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let sha256 = sha256_text(&content);
    Ok(SkillFileContent {
        path,
        relative_path: relative_path.to_string(),
        content,
        sha256,
    })
}

pub fn save_skill_file(
    cwd: &Path,
    skill_name: &str,
    relative_path: &str,
    expected_sha256: &str,
    content: &str,
    cached_skill_dir: Option<&Path>,
) -> Result<SkillFileWriteResult> {
    let skill_dir = resolve_skill_dir(cwd, skill_name, cached_skill_dir)?;
    let path = safe_join(&skill_dir, relative_path)?;
    let current_sha = sha256_file_streaming(&path)?;
    if current_sha != expected_sha256 {
        bail!("refusing to overwrite changed file {}", path.display());
    }

    atomic_write(&path, content)?;
    Ok(SkillFileWriteResult {
        path,
        relative_path: relative_path.to_string(),
        sha256: sha256_text(content),
    })
}

fn sha256_file_streaming(path: &Path) -> Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn create_skill_file(
    cwd: &Path,
    skill_name: &str,
    relative_path: &str,
    cached_skill_dir: Option<&Path>,
) -> Result<SkillFileWriteResult> {
    ensure_not_root_skill_manifest(relative_path)?;
    let skill_dir = resolve_skill_dir(cwd, skill_name, cached_skill_dir)?;
    let path = safe_child_path(&skill_dir, relative_path)?;
    if path.exists() {
        bail!("path already exists: {}", path.display());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    atomic_write(&path, "")?;
    Ok(SkillFileWriteResult {
        path,
        relative_path: relative_path.to_string(),
        sha256: sha256_text(""),
    })
}

pub fn create_skill_folder(
    cwd: &Path,
    skill_name: &str,
    relative_path: &str,
    cached_skill_dir: Option<&Path>,
) -> Result<()> {
    let skill_dir = resolve_skill_dir(cwd, skill_name, cached_skill_dir)?;
    let path = safe_child_path(&skill_dir, relative_path)?;
    if path.exists() {
        bail!("path already exists: {}", path.display());
    }
    fs::create_dir_all(&path).with_context(|| format!("failed to create {}", path.display()))?;
    Ok(())
}

pub fn rename_skill_path(
    cwd: &Path,
    skill_name: &str,
    from_relative_path: &str,
    to_relative_path: &str,
    cached_skill_dir: Option<&Path>,
) -> Result<()> {
    let skill_dir = resolve_skill_dir(cwd, skill_name, cached_skill_dir)?;
    let from = safe_join(&skill_dir, from_relative_path)?;
    ensure_strict_skill_child(&skill_dir, &from)?;
    ensure_not_root_skill_manifest(from_relative_path)?;
    ensure_not_root_skill_manifest(to_relative_path)?;
    let to = safe_child_path(&skill_dir, to_relative_path)?;
    if to.exists() {
        bail!("path already exists: {}", to.display());
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::rename(&from, &to)
        .with_context(|| format!("failed to rename {} to {}", from.display(), to.display()))?;
    Ok(())
}

pub fn delete_skill_path(
    cwd: &Path,
    skill_name: &str,
    relative_path: &str,
    cached_skill_dir: Option<&Path>,
) -> Result<()> {
    let skill_dir = resolve_skill_dir(cwd, skill_name, cached_skill_dir)?;
    let path = safe_join(&skill_dir, relative_path)?;
    ensure_strict_skill_child(&skill_dir, &path)?;
    ensure_not_root_skill_manifest(relative_path)?;
    if path.is_dir() {
        fs::remove_dir_all(&path)
            .with_context(|| format!("failed to delete {}", path.display()))?;
    } else {
        fs::remove_file(&path).with_context(|| format!("failed to delete {}", path.display()))?;
    }
    Ok(())
}

fn resolve_skill_dir(
    _cwd: &Path,
    skill_name: &str,
    cached_skill_dir: Option<&Path>,
) -> Result<PathBuf> {
    let path = cached_skill_dir
        .filter(|path| path.is_dir())
        .with_context(|| {
            format!("skill {skill_name} is not available in the current projection")
        })?;
    path.canonicalize()
        .with_context(|| format!("failed to canonicalize {}", path.display()))
}

fn safe_join(root: &Path, relative_path: &str) -> Result<PathBuf> {
    let path = root.join(relative_path);
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", root.display()))?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", path.display()))?;
    if !canonical.starts_with(&root) {
        bail!("path escapes skill directory");
    }
    Ok(canonical)
}

fn ensure_strict_skill_child(root: &Path, path: &Path) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", root.display()))?;
    if path == root {
        bail!("the skill root cannot be mutated through the file tree");
    }
    Ok(())
}

fn ensure_not_root_skill_manifest(relative_path: &str) -> Result<()> {
    let mut clean = PathBuf::new();
    for component in Path::new(relative_path).components() {
        match component {
            Component::Normal(value) => clean.push(value),
            Component::CurDir => {}
            Component::ParentDir => {
                if !clean.pop() {
                    bail!("path escapes skill directory");
                }
            }
            Component::RootDir | Component::Prefix(_) => bail!("path escapes skill directory"),
        }
    }
    if clean == Path::new("SKILL.md") {
        bail!("the root SKILL.md must be edited in place");
    }
    Ok(())
}

fn safe_child_path(root: &Path, relative_path: &str) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", root.display()))?;
    let mut clean = PathBuf::new();
    for component in Path::new(relative_path).components() {
        match component {
            Component::Normal(value) => clean.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("path escapes skill directory");
            }
        }
    }
    if clean.as_os_str().is_empty() {
        bail!("path is empty");
    }
    Ok(root.join(clean))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        create_skill_file, create_skill_folder, delete_skill_path, list_skill_files,
        read_skill_file, rename_skill_path, save_skill_file,
    };

    #[test]
    fn list_skill_files_skips_metadata_and_generated_entries() {
        let root = std::env::temp_dir().join(format!(
            "tendi-list-skill-files-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        let skill_dir = root.join(".agents/skills/demo");
        fs::create_dir_all(skill_dir.join("references")).expect("create skill files");
        fs::write(skill_dir.join("SKILL.md"), "# Demo\n").expect("write skill");
        fs::write(skill_dir.join("references/guide.md"), "guide\n").expect("write guide");
        fs::write(root.join(".gitignore"), "ancestor-ignored.md\n")
            .expect("write ancestor gitignore");
        fs::write(
            skill_dir.join(".gitignore"),
            "local-generated/\nlocal-ignored.md\n",
        )
        .expect("write skill gitignore");
        fs::create_dir_all(skill_dir.join("local-generated/nested"))
            .expect("create locally generated dir");
        fs::write(
            skill_dir.join("local-generated/nested/ignored.txt"),
            "ignored\n",
        )
        .expect("write locally generated file");
        fs::write(root.join("ancestor-ignored.md"), "ignored\n")
            .expect("write ancestor ignored file");
        fs::write(skill_dir.join("local-ignored.md"), "ignored\n")
            .expect("write local ignored file");
        fs::write(skill_dir.join("module.pyc"), "ignored\n").expect("write Python artifact");
        fs::write(skill_dir.join("Example.class"), "ignored\n").expect("write Java artifact");
        fs::create_dir_all(skill_dir.join("package.egg-info"))
            .expect("create Python package artifact");

        for name in [
            ".DS_Store",
            ".gitattributes",
            ".gitmodules",
            "Thumbs.db",
            "desktop.ini",
        ] {
            fs::write(skill_dir.join(name), "ignored\n").expect("write metadata");
        }
        for name in [
            ".git",
            "node_modules",
            "__pycache__",
            ".pytest_cache",
            ".venv",
            ".gradle",
            ".next",
            ".turbo",
            ".cache",
            "tmp",
            "temp",
        ] {
            fs::create_dir_all(skill_dir.join(name).join("nested")).expect("create generated dir");
            fs::write(skill_dir.join(name).join("nested/ignored.txt"), "ignored\n")
                .expect("write generated file");
        }

        let entries = list_skill_files(&root, "demo", Some(&skill_dir)).expect("list skill files");
        let paths = entries
            .iter()
            .map(|entry| entry.relative_path.as_str())
            .collect::<std::collections::BTreeSet<_>>();

        assert!(paths.contains("SKILL.md"));
        assert!(paths.contains("references"));
        assert!(paths.contains("references/guide.md"));
        for name in [
            ".DS_Store",
            ".gitignore",
            ".gitattributes",
            ".gitmodules",
            "Thumbs.db",
            "desktop.ini",
            ".git",
            "node_modules",
            "ancestor-ignored.md",
            "local-generated",
            "local-generated/nested/ignored.txt",
            "local-ignored.md",
            "module.pyc",
            "Example.class",
            "package.egg-info",
            "__pycache__",
            ".pytest_cache",
            ".venv",
            ".gradle",
            ".next",
            ".turbo",
            ".cache",
            "tmp",
            "temp",
        ] {
            assert!(
                !paths
                    .iter()
                    .any(|path| *path == name || path.starts_with(&format!("{name}/")))
            );
        }

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn save_skill_file_checks_hash_before_atomic_write() {
        let root = std::env::temp_dir().join(format!(
            "tendi-save-skill-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        let skill_dir = root.join(".agents/skills/demo");
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: Demo\n---\n\nold\n",
        )
        .expect("write skill");
        let cached_skill_dir = skill_dir.as_path();

        let original =
            read_skill_file(&root, "demo", "SKILL.md", Some(cached_skill_dir)).expect("read skill");
        let updated = save_skill_file(
            &root,
            "demo",
            "SKILL.md",
            &original.sha256,
            "---\nname: demo\ndescription: Demo\n---\n\nnew\n",
            Some(cached_skill_dir),
        )
        .expect("save skill");
        assert_ne!(original.sha256, updated.sha256);

        let stale = save_skill_file(
            &root,
            "demo",
            "SKILL.md",
            &original.sha256,
            "---\nname: demo\ndescription: Demo\n---\n\nstale\n",
            Some(cached_skill_dir),
        );
        assert!(stale.is_err());
        assert_eq!(
            fs::read_to_string(skill_dir.join("SKILL.md")).expect("read saved skill"),
            "---\nname: demo\ndescription: Demo\n---\n\nnew\n",
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_file_tools_require_a_resolved_skill_directory() {
        let root = std::env::temp_dir().join(format!(
            "tendi-file-projection-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        let skill_dir = root.join(".agents/skills/demo");
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(skill_dir.join("SKILL.md"), "---\nname: demo\n---\n").expect("write skill");

        assert!(read_skill_file(&root, "demo", "SKILL.md", None).is_err());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_file_tree_mutations_stay_inside_skill_dir() {
        let root = std::env::temp_dir().join(format!(
            "tendi-skill-tree-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        let skill_dir = root.join(".agents/skills/demo");
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: Demo\n---\n\n# Demo\n",
        )
        .expect("write skill");
        let cached_skill_dir = skill_dir.as_path();

        create_skill_folder(&root, "demo", "references", Some(cached_skill_dir))
            .expect("create folder");
        assert!(skill_dir.join("references").is_dir());

        let created =
            create_skill_file(&root, "demo", "references/notes.md", Some(cached_skill_dir))
                .expect("create file");
        assert_eq!(created.relative_path, "references/notes.md");
        assert!(skill_dir.join("references/notes.md").is_file());

        rename_skill_path(
            &root,
            "demo",
            "references/notes.md",
            "references/renamed.md",
            Some(cached_skill_dir),
        )
        .expect("rename file");
        assert!(!skill_dir.join("references/notes.md").exists());
        assert!(skill_dir.join("references/renamed.md").is_file());

        delete_skill_path(&root, "demo", "references", Some(cached_skill_dir))
            .expect("delete folder");
        assert!(!skill_dir.join("references").exists());

        assert!(create_skill_file(&root, "demo", "../escape.md", Some(cached_skill_dir)).is_err());
        assert!(create_skill_folder(&root, "demo", "/tmp/escape", Some(cached_skill_dir)).is_err());
        assert!(create_skill_file(&root, "demo", "./SKILL.md", Some(cached_skill_dir)).is_err());
        assert!(
            rename_skill_path(
                &root,
                "demo",
                "SKILL.md",
                "renamed.md",
                Some(cached_skill_dir),
            )
            .is_err()
        );
        assert!(delete_skill_path(&root, "demo", "./SKILL.md", Some(cached_skill_dir)).is_err());
        assert!(delete_skill_path(&root, "demo", ".", Some(cached_skill_dir)).is_err());
        assert!(delete_skill_path(&root, "demo", "", Some(cached_skill_dir)).is_err());
        assert!(skill_dir.join("SKILL.md").is_file());

        let _ = fs::remove_dir_all(root);
    }
}
