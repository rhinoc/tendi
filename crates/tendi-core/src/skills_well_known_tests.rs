use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
};

const MANIFEST: &str = "---\nname: demo\ndescription: Demo\n---\n\n# Original\n";
static REMOTE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct RemoteSkill {
    root: PathBuf,
    address: String,
    stopped: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
}

impl RemoteSkill {
    fn new() -> Self {
        let sequence = REMOTE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = temp_dir(&format!("tendi-well-known-update-{sequence}"));
        let web = root.join("web");
        fs::create_dir_all(&web).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop_server = stopped.clone();
        let server = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                if stop_server.load(Ordering::Acquire) {
                    break;
                }
                let mut request = String::new();
                BufReader::new(&mut stream).read_line(&mut request).unwrap();
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .trim_start_matches('/');
                let (status, body) = match fs::read(web.join(path)) {
                    Ok(bytes) => ("200 OK", bytes),
                    Err(_) => ("404 Not Found", Vec::new()),
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let fixture = Self {
            root,
            address,
            stopped,
            server: Some(server),
        };
        fixture.write_remote("SKILL.md", MANIFEST.as_bytes());
        fixture.index(&["SKILL.md"]);
        fixture
    }

    fn url(&self) -> String {
        format!("http://{}/.well-known/skills/demo/SKILL.md", self.address)
    }

    fn write_remote(&self, path: &str, bytes: &[u8]) {
        let destination = self.root.join("web/.well-known/skills/demo").join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(destination, bytes).unwrap();
    }

    fn index(&self, files: &[&str]) {
        let index = serde_json::json!({ "skills": [
            { "name": "demo", "description": "Demo", "files": files },
            { "name": "other", "description": "Must not download", "files": ["SKILL.md"] }
        ] });
        fs::write(
            self.root.join("web/.well-known/skills/index.json"),
            index.to_string(),
        )
        .unwrap();
    }

    fn skill(&self, agent: AgentKind, tracked_version: bool) -> SkillRecord {
        let directory = self.root.join(format!("installed/{}/demo", agent.label()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("SKILL.md"), MANIFEST).unwrap();
        let mut skill = test_skill("demo", "Demo", &directory);
        let path = &mut skill.paths[0];
        path.agent = agent;
        path.source_kind = "well-known".to_string();
        path.source = Some(self.url());
        path.update_status = "tracked".to_string();
        path.source_version = if tracked_version {
            let target = match agent {
                AgentKind::Claude => "claude-code",
                AgentKind::Cursor => "cursor",
                AgentKind::Codex => "codex",
                _ => "shared",
            }
            .parse()
            .unwrap();
            Some(super::super::well_known::installed_source_version(&directory, &target).unwrap())
        } else {
            None
        };
        skill
    }

    fn store(&self, skill: &SkillRecord) -> crate::storage::Store {
        let store = crate::storage::Store::open(self.root.join("store.sqlite3")).unwrap();
        let records = skill
            .paths
            .iter()
            .map(|path| SkillSourceRecord {
                skill_name: skill.name.clone(),
                skill_path: path.path.clone(),
                source_kind: path.source_kind.clone(),
                source: path.source.clone(),
                source_ref: None,
                source_version: path.source_version.clone(),
                source_relative_path: None,
                update_status: "tracked".to_string(),
                origin: "test".to_string(),
            })
            .collect::<Vec<_>>();
        let snapshots = super::super::capture_skill_snapshots(&records).unwrap();
        store
            .persist_skill_update_persistence_for_workspace(&self.root, &records, &snapshots)
            .unwrap();
        store
    }

    fn plan(&self, skill: &SkillRecord, store: &crate::storage::Store) -> SkillUpdatePlan {
        let scan = SkillScan {
            roots: Vec::new(),
            skills: vec![skill.clone()],
            warnings: Vec::new(),
        };
        super::super::plan_skill_updates_many_for_scan_in_workspace_with_store(
            &scan,
            &[skill.id.clone()],
            &self.root,
            store,
        )
        .unwrap()
    }
}

impl Drop for RemoteSkill {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = TcpStream::connect(&self.address);
        self.server.take().unwrap().join().unwrap();
        let _ = fs::remove_dir_all(super::super::persistent_source_root(&self.url()).unwrap());
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn well_known_checks_preserve_source_cache_and_ignore_provider_permissions() {
    let remote = RemoteSkill::new();
    let cache = super::super::persistent_source_root(&remote.url()).unwrap();
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("sentinel"), "keep").unwrap();
    for agent in [
        AgentKind::Codex,
        AgentKind::Claude,
        AgentKind::Cursor,
        AgentKind::Shared,
    ] {
        let skill = remote.skill(agent, false);
        let path = &skill.paths[0].path;
        let local = MANIFEST.replace(
            "description: Demo",
            "description: Demo\ndisable-model-invocation: true",
        );
        // Codex does not use the frontmatter flag; its policy is in openai.yaml.
        fs::write(
            path.join("SKILL.md"),
            if agent == AgentKind::Codex {
                MANIFEST
            } else {
                &local
            },
        )
        .unwrap();
        fs::create_dir_all(path.join("agents")).unwrap();
        fs::write(
            path.join("agents/openai.yaml"),
            "policy:\n  allow_implicit_invocation: false\n",
        )
        .unwrap();
        let reports = super::super::check_skill_updates_for_skills(
            &[&skill],
            super::super::git::never_cancelled(),
        );
        assert_eq!(reports[0].status, "up-to-date", "{agent:?}");
        assert_eq!(fs::read_to_string(cache.join("sentinel")).unwrap(), "keep");
    }
    remote.write_remote(
        "SKILL.md",
        MANIFEST.replace("Original", "Updated").as_bytes(),
    );
    let skill = remote.skill(AgentKind::Shared, false);
    let reports = super::super::check_skill_updates_for_skills(
        &[&skill],
        super::super::git::never_cancelled(),
    );
    assert_eq!(reports[0].status, "update-available");
}

#[test]
fn well_known_updates_merge_files_persist_and_recheck_without_network_on_apply() {
    let remote = RemoteSkill::new();
    let mut skill = remote.skill(AgentKind::Claude, true);
    let store = remote.store(&skill);
    let path = skill.paths[0].path.clone();
    fs::write(
        path.join("SKILL.md"),
        MANIFEST.replace(
            "description: Demo",
            "description: Local\ndisable-model-invocation: true",
        ),
    )
    .unwrap();
    skill.paths[0].effective_visibility = SkillVisibility::Manual;
    remote.write_remote(
        "SKILL.md",
        MANIFEST.replace("Original", "Updated").as_bytes(),
    );
    remote.write_remote("references/new.md", b"New reference");
    remote.index(&["SKILL.md", "references/new.md"]);
    let plan = remote.plan(&skill, &store);
    assert!(plan.can_apply());
    assert_eq!(plan.directory_updates.len(), 1);
    let files = &plan.directory_updates[0].files;
    assert_eq!(files.len(), 2);
    assert!(
        files
            .iter()
            .all(|file| file.status != "unavailable" && file.status != "conflict")
    );
    // The application consumes the exact preview bytes even if the source disappears.
    fs::remove_dir_all(remote.root.join("web")).unwrap();
    let prepared =
        super::super::prepare_skill_update_plan_with_resolutions(&plan, &BTreeMap::new()).unwrap();
    super::super::apply_prepared_skill_update_plan_for_workspace(&store, &remote.root, &prepared)
        .unwrap();
    let updated = fs::read_to_string(path.join("SKILL.md")).unwrap();
    assert!(updated.contains("description: Local"));
    assert!(updated.contains("# Updated"));
    assert!(updated.contains("disable-model-invocation: true"));
    assert_eq!(
        fs::read(path.join("references/new.md")).unwrap(),
        b"New reference"
    );
    let records = store
        .skill_source_records_for_workspace(&remote.root)
        .unwrap();
    let version = records[0].source_version.clone();
    assert_ne!(version, skill.paths[0].source_version);
    assert_eq!(records[0].update_status, "tracked");
    let snapshot = store
        .skill_snapshot_for_workspace(&remote.root, &path)
        .unwrap()
        .unwrap();
    assert_eq!(Some(snapshot.source_version), version);
    skill.paths[0].source_version = version;
    remote.write_remote(
        "SKILL.md",
        MANIFEST.replace("Original", "Updated").as_bytes(),
    );
    remote.write_remote("references/new.md", b"New reference");
    remote.index(&["SKILL.md", "references/new.md"]);
    let reports = super::super::check_skill_updates_for_skills(
        &[&skill],
        super::super::git::never_cancelled(),
    );
    assert_eq!(reports[0].status, "up-to-date");
}

#[test]
fn well_known_missing_snapshot_requires_resolution_and_retains_visibility() {
    let remote = RemoteSkill::new();
    let mut skill = remote.skill(AgentKind::Shared, false);
    skill.paths[0].effective_visibility = SkillVisibility::Manual;
    let path = &skill.paths[0].path;
    fs::write(
        path.join("SKILL.md"),
        MANIFEST.replace(
            "description: Demo",
            "description: Demo\ndisable-model-invocation: true",
        ),
    )
    .unwrap();
    let store = remote.store(&skill);
    remote.write_remote(
        "SKILL.md",
        MANIFEST.replace("Original", "Updated").as_bytes(),
    );
    let plan = remote.plan(&skill, &store);
    let file = &plan.directory_updates[0].files[0];
    assert_eq!(file.status, "unavailable");
    assert!(
        super::super::prepare_skill_update_plan_with_resolutions(&plan, &BTreeMap::new()).is_err()
    );
    let resolutions = BTreeMap::from([(
        file.resolution_key.clone(),
        "__tendi_use_update__".to_string(),
    )]);
    let prepared =
        super::super::prepare_skill_update_plan_with_resolutions(&plan, &resolutions).unwrap();
    super::super::apply_prepared_skill_update_plan_for_workspace(&store, &remote.root, &prepared)
        .unwrap();
    let updated = fs::read_to_string(path.join("SKILL.md")).unwrap();
    assert!(updated.contains("# Updated"));
    assert!(updated.contains("disable-model-invocation: true"));
}

#[test]
fn well_known_binary_deletion_and_stale_preview_use_existing_transaction() {
    let remote = RemoteSkill::new();
    let skill = remote.skill(AgentKind::Shared, true);
    let path = &skill.paths[0].path;
    fs::write(path.join("old.md"), b"Old reference").unwrap();
    let store = remote.store(&skill);
    remote.write_remote("assets/icon.png", &[0x89, 0xff]);
    remote.index(&["SKILL.md", "assets/icon.png"]);
    let plan = remote.plan(&skill, &store);
    let binary = plan.directory_updates[0]
        .files
        .iter()
        .find(|file| file.path == "assets/icon.png")
        .unwrap();
    assert_eq!(binary.status, "binary");
    assert!(
        plan.directory_updates[0]
            .files
            .iter()
            .any(|file| file.path == "old.md" && !file.after_exists)
    );
    let resolutions = BTreeMap::from([(
        binary.resolution_key.clone(),
        "__tendi_use_update__".to_string(),
    )]);
    let prepared =
        super::super::prepare_skill_update_plan_with_resolutions(&plan, &resolutions).unwrap();
    fs::write(path.join("old.md"), b"Edited after preview").unwrap();
    assert!(
        super::super::apply_prepared_skill_update_plan_for_workspace(
            &store,
            &remote.root,
            &prepared
        )
        .is_err()
    );
    assert_eq!(
        fs::read(path.join("old.md")).unwrap(),
        b"Edited after preview"
    );
    assert!(!path.join("assets/icon.png").exists());
    fs::write(path.join("old.md"), b"Old reference").unwrap();
    super::super::apply_prepared_skill_update_plan_for_workspace(&store, &remote.root, &prepared)
        .unwrap();
    assert!(!path.join("old.md").exists());
    assert_eq!(
        fs::read(path.join("assets/icon.png")).unwrap(),
        [0x89, 0xff]
    );
}

#[test]
fn well_known_network_failures_are_reported_and_direct_urls_work() {
    let remote = RemoteSkill::new();
    let skill = remote.skill(AgentKind::Shared, false);
    fs::remove_file(remote.root.join("web/.well-known/skills/index.json")).unwrap();
    let reports = super::super::check_skill_updates_for_skills(
        &[&skill],
        super::super::git::never_cancelled(),
    );
    assert_eq!(reports[0].status, "up-to-date");
    fs::remove_file(remote.root.join("web/.well-known/skills/demo/SKILL.md")).unwrap();
    let reports = super::super::check_skill_updates_for_skills(
        &[&skill],
        super::super::git::never_cancelled(),
    );
    assert_eq!(reports[0].status, "unreachable");
}

#[test]
fn well_known_install_records_version_and_snapshot_for_next_update() {
    let remote = RemoteSkill::new();
    let other = remote.root.join("web/.well-known/skills/other");
    fs::create_dir_all(&other).unwrap();
    fs::write(
        other.join("SKILL.md"),
        MANIFEST.replace("name: demo", "name: other"),
    )
    .unwrap();
    let options = super::super::SkillAddOptions {
        source: remote.url(),
        target: "shared".parse().unwrap(),
        scope: SkillInstallScope::Project,
        skills: vec!["demo".to_string()],
        copy: true,
        overwrite: false,
        visibility: SkillVisibility::Manual,
    };
    let report =
        apply_skill_add_with_target_root(&remote.root, &options, &remote.root.join("new-install"))
            .unwrap();
    let records = super::super::skill_source_records_for_add(&report).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source_kind, "well-known");
    assert_eq!(records[0].update_status, "tracked");
    assert!(
        records[0]
            .source_version
            .as_deref()
            .unwrap()
            .starts_with("sha256:")
    );
    let snapshots = super::super::capture_skill_snapshots(&records).unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(
        Some(&snapshots[0].source_version),
        records[0].source_version.as_ref()
    );
}

#[test]
fn well_known_v2_archive_update_checks_digest_and_all_files() {
    let remote = RemoteSkill::new();
    let skill = remote.skill(AgentKind::Shared, false);
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    archive
        .start_file("SKILL.md", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(MANIFEST.as_bytes()).unwrap();
    archive
        .start_file(
            "references/guide.md",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    archive.write_all(b"New guide").unwrap();
    let bytes = archive.finish().unwrap().into_inner();
    fs::write(remote.root.join("web/demo.zip"), &bytes).unwrap();
    let index = serde_json::json!({
        "$schema": "https://schemas.agentskills.io/discovery/0.2.0/schema.json",
        "skills": [{ "name": "demo", "description": "Demo", "type": "archive",
            "url": "/demo.zip", "digest": format!("sha256:{}", super::super::sha256_bytes(&bytes)) }]
    });
    fs::write(
        remote.root.join("web/.well-known/skills/index.json"),
        index.to_string(),
    )
    .unwrap();
    let reports = super::super::check_skill_updates_for_skills(
        &[&skill],
        super::super::git::never_cancelled(),
    );
    assert_eq!(reports[0].status, "update-available");
    let store = remote.store(&skill);
    let plan = remote.plan(&skill, &store);
    assert_eq!(plan.directory_updates[0].files.len(), 1);
    assert_eq!(
        plan.directory_updates[0].files[0].path,
        "references/guide.md"
    );
    fs::write(remote.root.join("web/demo.zip"), b"corrupted").unwrap();
    let reports = super::super::check_skill_updates_for_skills(
        &[&skill],
        super::super::git::never_cancelled(),
    );
    assert_eq!(reports[0].status, "unreachable");
}

#[test]
fn well_known_description_limit_counts_unicode_characters() {
    let remote = RemoteSkill::new();
    let skill = remote.skill(AgentKind::Shared, false);
    let index_path = remote.root.join("web/.well-known/skills/index.json");
    let index = serde_json::json!({ "skills": [{
        "name": "demo", "description": "飞书".repeat(300), "files": ["SKILL.md"]
    }] });
    fs::write(&index_path, index.to_string()).unwrap();
    let reports = super::super::check_skill_updates_for_skills(
        &[&skill],
        super::super::git::never_cancelled(),
    );
    assert_eq!(reports[0].status, "up-to-date");
    let mut too_long = index;
    too_long["skills"][0]["description"] = serde_json::json!("飞".repeat(1025));
    fs::write(index_path, too_long.to_string()).unwrap();
    let reports = super::super::check_skill_updates_for_skills(
        &[&skill],
        super::super::git::never_cancelled(),
    );
    assert_eq!(reports[0].status, "unreachable");
}

#[test]
fn well_known_content_conflicts_and_changed_sources_require_new_decisions() {
    let remote = RemoteSkill::new();
    let skill = remote.skill(AgentKind::Shared, true);
    let store = remote.store(&skill);
    fs::write(
        skill.paths[0].path.join("SKILL.md"),
        MANIFEST.replace("Original", "Local"),
    )
    .unwrap();
    remote.write_remote(
        "SKILL.md",
        MANIFEST.replace("Original", "Remote").as_bytes(),
    );
    let plan = remote.plan(&skill, &store);
    let file = &plan.directory_updates[0].files[0];
    assert_eq!(file.status, "conflict");
    let resolutions = BTreeMap::from([(
        file.resolution_key.clone(),
        "__tendi_keep_local__".to_string(),
    )]);
    let prepared =
        super::super::prepare_skill_update_plan_with_resolutions(&plan, &resolutions).unwrap();
    super::super::apply_prepared_skill_update_plan_for_workspace(&store, &remote.root, &prepared)
        .unwrap();
    assert!(
        fs::read_to_string(skill.paths[0].path.join("SKILL.md"))
            .unwrap()
            .contains("# Local")
    );
    let reports = super::super::check_skill_updates_for_skills(
        &[&skill],
        super::super::git::never_cancelled(),
    );
    remote.write_remote(
        "SKILL.md",
        MANIFEST.replace("Original", "Another update").as_bytes(),
    );
    let scan = SkillScan {
        roots: Vec::new(),
        skills: vec![skill.clone()],
        warnings: Vec::new(),
    };
    let cached = plan_skill_updates_many_for_scan_in_workspace_with_store_and_reports(
        &scan,
        &[skill.id.clone()],
        &remote.root,
        &store,
        &reports,
    )
    .unwrap();
    let incoming = &cached.directory_updates[0].files[0].incoming;
    assert!(incoming.contains("# Remote"));
    assert!(!incoming.contains("Another update"));
    let mut uncached_reports = reports;
    for report in &mut uncached_reports {
        report.source_files = None;
    }
    let result = plan_skill_updates_many_for_scan_in_workspace_with_store_and_reports(
        &scan,
        &[skill.id.clone()],
        &remote.root,
        &store,
        &uncached_reports,
    );
    assert!(result.unwrap_err().to_string().contains("source changed"));
}

#[test]
fn well_known_shared_symlink_updates_both_source_records() {
    let remote = RemoteSkill::new();
    let mut skill = remote.skill(AgentKind::Shared, false);
    let alias = remote.root.join("claude/demo");
    fs::create_dir_all(alias.parent().unwrap()).unwrap();
    create_symlink(&skill.paths[0].path, &alias).unwrap();
    let mut alias_path = skill.paths[0].clone();
    alias_path.path = alias.clone();
    alias_path.agent = AgentKind::Claude;
    skill.paths.push(alias_path);
    let store = remote.store(&skill);
    remote.write_remote(
        "SKILL.md",
        MANIFEST.replace("Original", "Updated").as_bytes(),
    );
    let plan = remote.plan(&skill, &store);
    assert_eq!(plan.directory_updates.len(), 1);
    assert_eq!(plan.source_updates.len(), 2);
    let resolutions = plan.directory_updates[0]
        .files
        .iter()
        .map(|file| {
            (
                file.resolution_key.clone(),
                "__tendi_use_update__".to_string(),
            )
        })
        .collect();
    let prepared =
        super::super::prepare_skill_update_plan_with_resolutions(&plan, &resolutions).unwrap();
    super::super::apply_prepared_skill_update_plan_for_workspace(&store, &remote.root, &prepared)
        .unwrap();
    assert!(
        fs::read_to_string(alias.join("SKILL.md"))
            .unwrap()
            .contains("Updated")
    );
    assert!(
        fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let records = store
        .skill_source_records_for_workspace(&remote.root)
        .unwrap();
    assert_eq!(records.len(), 2);
    assert!(records[0].source_version.is_some());
    assert_eq!(records[0].source_version, records[1].source_version);
}

#[test]
fn well_known_verified_imported_digest_allows_automatic_merge() {
    let remote = RemoteSkill::new();
    let mut skill = remote.skill(AgentKind::Shared, false);
    let path = skill.paths[0].path.clone();
    let digest = crate::skill_source::skill_files_digest(
        &crate::skill_source::read_skill_files(&path).unwrap(),
    );
    fs::write(
        path.join("SKILL.md"),
        MANIFEST.replace(
            "description: Demo",
            "description: Demo\ndisable-model-invocation: true",
        ),
    )
    .unwrap();
    skill.paths[0].source_version = Some(digest.clone());
    skill.paths[0].effective_visibility = SkillVisibility::Manual;
    let snapshot = super::super::well_known::verified_source_snapshot(&skill.paths[0], &digest)
        .unwrap()
        .unwrap();
    let store = remote.store(&skill);
    store
        .replace_skill_snapshots_for_workspace(&remote.root, &[snapshot])
        .unwrap();
    remote.write_remote(
        "SKILL.md",
        MANIFEST.replace("Original", "Updated").as_bytes(),
    );
    let plan = remote.plan(&skill, &store);
    let file = &plan.directory_updates[0].files[0];
    assert_eq!(file.status, "remote");
    assert!(file.after.contains("# Updated"));
    assert!(file.after.contains("disable-model-invocation: true"));
    assert!(
        super::super::prepare_skill_update_plan_with_resolutions(&plan, &BTreeMap::new()).is_ok()
    );
}

#[test]
fn well_known_preview_reuses_checked_content_without_network() {
    let remote = RemoteSkill::new();
    let skill = remote.skill(AgentKind::Shared, true);
    let store = remote.store(&skill);
    remote.write_remote(
        "SKILL.md",
        MANIFEST.replace("Original", "Checked update").as_bytes(),
    );
    remote.write_remote("references/checked.md", b"Checked reference");
    remote.index(&["SKILL.md", "references/checked.md"]);
    let reports = super::super::check_skill_updates_for_skills(
        &[&skill],
        super::super::git::never_cancelled(),
    );
    assert_eq!(reports[0].status, "update-available");
    assert!(reports[0].source_files.is_some());
    let wire = serde_json::to_value(&reports[0]).unwrap();
    assert!(wire.get("source_files").is_none());
    assert!(!wire.to_string().contains("Checked reference"));
    fs::remove_dir_all(remote.root.join("web")).unwrap();
    let scan = SkillScan {
        roots: Vec::new(),
        skills: vec![skill.clone()],
        warnings: Vec::new(),
    };
    for _ in 0..2 {
        let plan = plan_skill_updates_many_for_scan_in_workspace_with_store_and_reports(
            &scan,
            &[skill.id.clone()],
            &remote.root,
            &store,
            &reports,
        )
        .unwrap();
        assert!(plan.can_apply());
        let files = &plan.directory_updates[0].files;
        assert!(
            files
                .iter()
                .any(|file| file.path == "SKILL.md" && file.after.contains("# Checked update"))
        );
        assert!(
            files
                .iter()
                .any(|file| file.path == "references/checked.md"
                    && file.after == "Checked reference")
        );
    }
}
