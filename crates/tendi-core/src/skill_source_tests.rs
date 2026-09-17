use super::*;
use std::io::Write;

#[test]
fn parses_github_tree_ref_subpath_and_fragment_ref() {
    let cwd = Path::new("/tmp/missing-tendi-source-parser");
    let tree = parse(cwd, "https://github.com/acme/skills/tree/main/skills/demo").unwrap();
    assert_eq!(tree.kind, "github");
    assert_eq!(tree.url, "https://github.com/acme/skills.git");
    assert_eq!(tree.git_ref.as_deref(), Some("main"));
    assert_eq!(tree.subpath.as_deref(), Some(Path::new("skills/demo")));

    let fragment = parse(cwd, "acme/skills/skills/demo#feature%2Finstall").unwrap();
    assert_eq!(fragment.git_ref.as_deref(), Some("feature/install"));
    assert_eq!(fragment.subpath.as_deref(), Some(Path::new("skills/demo")));
}

#[test]
fn parses_clawhub_download_source() {
    let source = parse(
        Path::new("/tmp/missing-tendi-source-parser"),
        "https://clawhub.ai/api/v1/download?slug=demo&version=1.0.0",
    )
    .unwrap();
    assert_eq!(source.kind, "clawhub");
    assert_eq!(
        source.url,
        "https://clawhub.ai/api/v1/download?slug=demo&version=1.0.0"
    );
}

#[test]
fn parses_gitlab_https_ssh_and_hugging_face() {
    let cwd = Path::new("/tmp/missing-tendi-source-parser");
    let gitlab = parse(
        cwd,
        "https://gitlab.com/group/sub/repo/-/tree/release/skills/demo",
    )
    .unwrap();
    assert_eq!(gitlab.kind, "gitlab");
    assert_eq!(gitlab.url, "https://gitlab.com/group/sub/repo.git");
    assert_eq!(gitlab.git_ref.as_deref(), Some("release"));
    assert_eq!(gitlab.subpath.as_deref(), Some(Path::new("skills/demo")));

    let ssh = parse(cwd, "git@gitlab.com:group/repo.git#v1.2.0").unwrap();
    assert_eq!(ssh.kind, "gitlab");
    assert_eq!(ssh.git_ref.as_deref(), Some("v1.2.0"));

    let hf = parse(
        cwd,
        "https://huggingface.co/datasets/acme/skills/tree/main/skills/demo",
    )
    .unwrap();
    assert_eq!(hf.kind, "huggingface");
    assert_eq!(hf.url, "https://huggingface.co/datasets/acme/skills.git");
    assert_eq!(hf.git_ref.as_deref(), Some("main"));
    assert_eq!(hf.subpath.as_deref(), Some(Path::new("skills/demo")));
}

#[test]
fn rejects_path_traversal_and_unsafe_refs() {
    let cwd = Path::new("/tmp/missing-tendi-source-parser");
    for source in [
        "acme/skills/../secret",
        "https://github.com/acme/skills/tree/main/skills/../secret",
        "https://gitlab.com/acme/skills/-/tree/main/../../secret",
    ] {
        assert!(parse(cwd, source).is_err(), "accepted {source}");
    }
    for source in [
        "acme/skills#--upload-pack=evil",
        "acme/skills#refs/heads/main.lock",
        "acme/skills#main%20evil",
        "acme/skills#main..evil",
        "acme/skills#main%5Eevil",
    ] {
        assert!(parse(cwd, source).is_err(), "accepted {source}");
    }
}

#[test]
fn recognizes_well_known_and_builds_safe_candidates() {
    let cwd = Path::new("/tmp/missing-tendi-source-parser");
    let source = parse(cwd, "https://docs.example.com/product").unwrap();
    assert_eq!(source.kind, "well-known");
    assert_eq!(
        discovery_index_candidates(&source.url).unwrap(),
        vec![
            "https://docs.example.com/product/.well-known/agent-skills/index.json",
            "https://docs.example.com/product/.well-known/skills/index.json",
            "https://docs.example.com/.well-known/agent-skills/index.json",
            "https://docs.example.com/.well-known/skills/index.json",
        ]
    );
    assert_eq!(
        discovery_index_candidates("https://docs.example.com/.well-known/agent-skills/demo")
            .unwrap(),
        vec!["https://docs.example.com/.well-known/agent-skills/index.json"]
    );
    assert!(discovery_index_candidates("https://user@example.com").is_err());
}

#[test]
fn validates_v2_digest_and_legacy_paths() {
    let bytes = b"---\nname: demo\ndescription: Demo\n---\n";
    let digest = format!("sha256:{:x}", Sha256::digest(bytes));
    verify_digest(bytes, Some(&digest)).unwrap();
    assert!(verify_digest(b"changed", Some(&digest)).is_err());
    assert!(validate_relative_path("references/guide.md").is_ok());
    assert!(validate_relative_path("../secret").is_err());
    assert!(validate_relative_path("references\\secret").is_err());
    assert!(validate_skill_name("demo-skill").is_ok());
    assert!(validate_skill_name("../demo").is_err());
}

#[test]
fn rejects_unsafe_legacy_index_before_downloading_files() {
    let index: DiscoveryIndex = serde_json::from_value(serde_json::json!({
        "skills": [{
            "name": "demo",
            "description": "Demo",
            "files": ["SKILL.md", "../secret"]
        }]
    }))
    .unwrap();
    let target =
        std::env::temp_dir().join(format!("tendi-well-known-security-{}", std::process::id()));
    let error = materialize_index(
        "https://invalid.example/.well-known/agent-skills/index.json",
        &index,
        &target,
    )
    .unwrap_err();
    assert!(error.to_string().contains("unsafe source subpath"));
    assert!(!target.exists());
}

#[test]
fn extracts_safe_zip_and_rejects_traversal() {
    let skill = b"---\nname: demo\ndescription: Demo\n---\n";
    let mut safe = zip::ZipWriter::new(Cursor::new(Vec::new()));
    safe.start_file("SKILL.md", zip::write::SimpleFileOptions::default())
        .unwrap();
    safe.write_all(skill).unwrap();
    safe.start_file(
        "references/guide.md",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    safe.write_all(b"Guide").unwrap();
    let safe = safe.finish().unwrap().into_inner();
    let target = std::env::temp_dir().join(format!(
        "tendi-well-known-zip-{}-{}",
        std::process::id(),
        safe.len()
    ));
    extract_skill_archive(&safe, "https://example.com/demo.zip", &target).unwrap();
    assert_eq!(fs::read(target.join("SKILL.md")).unwrap(), skill);
    assert_eq!(
        fs::read_to_string(target.join("references/guide.md")).unwrap(),
        "Guide"
    );
    let _ = fs::remove_dir_all(&target);

    let mut unsafe_zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    unsafe_zip
        .start_file("../secret", zip::write::SimpleFileOptions::default())
        .unwrap();
    unsafe_zip.write_all(b"secret").unwrap();
    let unsafe_zip = unsafe_zip.finish().unwrap().into_inner();
    assert!(extract_zip(&unsafe_zip, &target).is_err());
    assert!(!target.join("secret").exists());
    let _ = fs::remove_dir_all(target);
}
