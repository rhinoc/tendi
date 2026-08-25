use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::fsutil::{atomic_write, sha256_text};

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

pub fn list_skill_files(
    cwd: &Path,
    skill_name: &str,
    cached_skill_dir: Option<&Path>,
) -> Result<Vec<SkillFileEntry>> {
    let skill_dir = resolve_skill_dir(cwd, skill_name, cached_skill_dir)?;
    let mut entries = Vec::new();

    for entry in WalkDir::new(&skill_dir)
        .follow_links(true)
        .max_depth(4)
        .into_iter()
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
) -> Result<SkillFileContent> {
    let skill_dir = resolve_skill_dir(cwd, skill_name, cached_skill_dir)?;
    let path = safe_join(&skill_dir, relative_path)?;
    let current_sha = sha256_file_streaming(&path)?;
    if current_sha != expected_sha256 {
        bail!("refusing to overwrite changed file {}", path.display());
    }

    atomic_write(&path, content)?;
    Ok(SkillFileContent {
        path,
        relative_path: relative_path.to_string(),
        content: content.to_string(),
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
) -> Result<SkillFileContent> {
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
    Ok(SkillFileContent {
        path,
        relative_path: relative_path.to_string(),
        content: String::new(),
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
        create_skill_file, create_skill_folder, delete_skill_path, read_skill_file,
        rename_skill_path, save_skill_file, sha256_file_streaming, sha256_text,
    };

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
            updated.content
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

    #[test]
    fn large_skill_file_hash_matches_text_hash() {
        let root = std::env::temp_dir().join(format!(
            "tendi-large-skill-hash-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp dir");
        let content = "large skill content\n".repeat(512 * 1024);
        let path = root.join("large.md");
        fs::write(&path, &content).expect("write large skill");

        assert_eq!(
            sha256_file_streaming(&path).expect("hash large skill"),
            sha256_text(&content),
        );

        let _ = fs::remove_dir_all(root);
    }
}
