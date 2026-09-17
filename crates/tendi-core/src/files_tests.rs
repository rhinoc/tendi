use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    create_skill_file, create_skill_folder, delete_skill_path, list_skill_files, read_skill_file,
    rename_skill_path, save_skill_file,
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
    fs::write(root.join(".gitignore"), "ancestor-ignored.md\n").expect("write ancestor gitignore");
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
    fs::write(root.join("ancestor-ignored.md"), "ignored\n").expect("write ancestor ignored file");
    fs::write(skill_dir.join("local-ignored.md"), "ignored\n").expect("write local ignored file");
    fs::write(skill_dir.join("module.pyc"), "ignored\n").expect("write Python artifact");
    fs::write(skill_dir.join("Example.class"), "ignored\n").expect("write Java artifact");
    fs::create_dir_all(skill_dir.join("package.egg-info")).expect("create Python package artifact");

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

    let created = create_skill_file(&root, "demo", "references/notes.md", Some(cached_skill_dir))
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

    delete_skill_path(&root, "demo", "references", Some(cached_skill_dir)).expect("delete folder");
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
