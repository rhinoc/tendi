use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_home(name: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should follow Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "tendi-config-{name}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn accepts_a_custom_codex_home() {
    let home = temp_home("custom-codex");
    let codex_home = home.join("custom-codex");
    let configs = configs_for_roots_with_codex_home(&home, &codex_home);
    assert_eq!(configs[0].path, codex_home.join("config.toml"));
}

#[test]
fn file_resources_serialize_config_save_with_hook_review_and_recheck_hash() {
    let home = temp_home("cross-domain-cas");
    fs::create_dir_all(home.join(".codex")).unwrap();
    let path = home.join(".codex/config.toml");
    let before = "model = \"original\"\n";
    fs::write(&path, before).unwrap();
    let lease = crate::coordination::acquire_file_resources(std::slice::from_ref(&path)).unwrap();
    let config = resolve_config_for_roots(&home, &home.join(".codex"), &path).unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        done_tx
            .send(save_config(
                &config,
                &sha256_text(before),
                "model = \"edited\"\n",
            ))
            .unwrap();
    });
    started_rx.recv().unwrap();
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err()
    );
    crate::providers::codex::write_trusted_hash(&path, "source:hook", "sha256:trusted").unwrap();
    drop(lease);
    let result = done_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap();
    assert!(
        result
            .unwrap_err()
            .downcast_ref::<ConfigChangedError>()
            .is_some()
    );
    worker.join().unwrap();
    let current = fs::read_to_string(&path).unwrap();
    assert!(current.contains("model = \"original\""));
    assert!(current.contains("sha256:trusted"));
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn file_resources_allow_unrelated_provider_config_save() {
    let home = temp_home("independent-config");
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(home.join(".claude")).unwrap();
    let busy = home.join(".codex/config.toml");
    fs::write(&busy, "model = \"busy\"\n").unwrap();
    let independent = home.join(".claude/settings.json");
    fs::write(&independent, "{}\n").unwrap();
    let lease = crate::coordination::acquire_file_resources(&[busy]).unwrap();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let config = resolve_config_for_path(&home, &independent).unwrap();
    let worker = std::thread::spawn(move || {
        done_tx
            .send(save_config(
                &config,
                &sha256_text("{}\n"),
                "{\"permissions\":{}}\n",
            ))
            .unwrap();
    });
    done_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
        .unwrap();
    worker.join().unwrap();
    drop(lease);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn resolves_supported_profile_paths_without_listing_catalog() {
    let home = temp_home("direct-resolve");
    let codex_home = home.join(".codex");
    let cases = vec![
        (
            codex_home.join("deep-review.config.toml"),
            AgentKind::Codex,
            "Codex / deep-review",
            "toml",
        ),
        (
            home.join(".claude/tendi-profiles/safe-mode.settings.json"),
            AgentKind::Claude,
            "Claude Code / safe-mode",
            "json",
        ),
        (
            home.join(".cursor/tendi-profiles/safe-mode/cli-config.json"),
            AgentKind::Cursor,
            "Cursor / safe-mode",
            "json",
        ),
    ];

    for (path, agent, label, format) in cases {
        let config = resolve_config_for_roots(&home, &codex_home, &path)
            .expect("supported profile path should resolve directly");
        assert_eq!(config.agent, agent);
        assert_eq!(config.label, label);
        assert_eq!(config.format, format);
        assert_eq!(config.path, path);
        assert!(!config.exists);
    }

    let _ = fs::remove_dir_all(home);
}

#[test]
fn lists_codex_profile_files() {
    let home = temp_home("profiles");
    let codex_home = home.join(".codex");
    fs::create_dir_all(&codex_home).expect("create codex home");
    fs::write(
        codex_home.join("deep-review.config.toml"),
        "model = \"one\"\n",
    )
    .expect("write profile");
    fs::write(codex_home.join("not-a-profile.toml"), "model = \"two\"\n")
        .expect("write unrelated config");

    let configs = configs_for_roots_with_codex_home(&home, &codex_home);
    let profile = configs
        .iter()
        .find(|config| config.profile.as_deref() == Some("deep-review"))
        .expect("profile should be listed");
    assert_eq!(profile.path, codex_home.join("deep-review.config.toml"));
    assert_eq!(profile.format, "toml");
    assert_eq!(
        configs
            .iter()
            .filter(|config| config.profile.is_some())
            .count(),
        1
    );
    fs::remove_dir_all(home).expect("temporary home should be removable");
}

#[test]
fn lists_claude_profile_files() {
    let home = temp_home("claude-profiles");
    let profile_dir = home.join(".claude/tendi-profiles");
    fs::create_dir_all(&profile_dir).expect("create Claude profile directory");
    fs::write(
        profile_dir.join("safe-mode.settings.json"),
        "{\"permissions\":{\"defaultMode\":\"plan\"}}\n",
    )
    .expect("write profile");

    let configs = configs_for_roots(&home);
    let profile = configs
        .iter()
        .find(|config| config.profile.as_deref() == Some("safe-mode"))
        .expect("Claude profile should be listed");
    assert_eq!(profile.agent, AgentKind::Claude);
    assert_eq!(
        profile.path,
        home.join(".claude/tendi-profiles/safe-mode.settings.json")
    );
    assert_eq!(profile.format, "json");
    fs::remove_dir_all(home).expect("temporary home should be removable");
}

#[test]
fn lists_cursor_profile_files() {
    let home = temp_home("cursor-profiles");
    let profile_dir = home.join(".cursor/tendi-profiles");
    let profile_path = profile_dir.join("safe-mode/cli-config.json");
    fs::create_dir_all(profile_path.parent().expect("profile parent"))
        .expect("create Cursor profile directory");
    fs::write(&profile_path, "{\"permissions\":{\"allow\":[]}}\n").expect("write profile");

    let configs = configs_for_roots(&home);
    let profile = configs
        .iter()
        .find(|config| config.profile.as_deref() == Some("safe-mode"))
        .expect("Cursor profile should be listed");
    assert_eq!(profile.agent, AgentKind::Cursor);
    assert_eq!(
        profile.path,
        home.join(".cursor/tendi-profiles/safe-mode/cli-config.json")
    );
    assert_eq!(profile.format, "json");
    fs::remove_dir_all(home).expect("temporary home should be removable");
}

#[test]
fn creates_profile_with_comments() {
    let home = temp_home("create-profile");
    let codex_home = home.join(".codex");
    let content = "# profile comment\nmodel = \"one\"\n";
    let created = create_profile_for_roots(AgentKind::Codex, &home, "deep-review", content)
        .expect("valid profile should be created");
    assert_eq!(created.path, codex_home.join("deep-review.config.toml"));
    assert!(created.exists);
    assert_eq!(
        fs::read_to_string(codex_home.join("deep-review.config.toml"))
            .expect("profile should be readable"),
        content
    );
    assert!(
        create_profile_for_roots(AgentKind::Codex, &home, "deep-review", content,)
            .expect_err("duplicate profile should fail")
            .to_string()
            .contains("already exists")
    );
    fs::remove_dir_all(home).expect("temporary home should be removable");
}

#[test]
fn creates_claude_profile_as_json() {
    let home = temp_home("create-claude-profile");
    let content = "{\"permissions\":{\"defaultMode\":\"plan\"}}\n";
    let created = create_profile_for_roots(AgentKind::Claude, &home, "safe-mode", content)
        .expect("valid Claude profile should be created");
    assert_eq!(
        created.path,
        home.join(".claude/tendi-profiles/safe-mode.settings.json")
    );
    assert_eq!(
        fs::read_to_string(&created.path).expect("profile should be readable"),
        content
    );
    fs::remove_dir_all(home).expect("temporary home should be removable");
}

#[test]
fn creates_cursor_profile_as_json() {
    let home = temp_home("create-cursor-profile");
    let content = "{\"permissions\":{\"allow\":[]}}\n";
    let created = create_profile_for_roots(AgentKind::Cursor, &home, "safe-mode", content)
        .expect("valid Cursor profile should be created");
    assert_eq!(
        created.path,
        home.join(".cursor/tendi-profiles/safe-mode/cli-config.json")
    );
    assert_eq!(
        fs::read_to_string(&created.path).expect("profile should be readable"),
        content
    );
    fs::remove_dir_all(home).expect("temporary home should be removable");
}

#[test]
fn deletes_only_resolved_config_files() {
    let home = temp_home("delete-config");
    let created = create_profile_for_roots(AgentKind::Claude, &home, "safe-mode", "{}\n")
        .expect("profile should be created");
    delete_configs_for_home(&home, std::slice::from_ref(&created.path))
        .expect("profile should be deleted");
    assert!(!created.path.exists());
    assert!(delete_configs_for_home(&home, &[home.join(".claude/other.json")]).is_err());
    fs::remove_dir_all(home).expect("temporary home should be removable");
}

#[test]
fn rejects_unsafe_config_profile_names() {
    assert!(validate_profile_name("../escape").is_err());
    assert!(validate_profile_name("deep review").is_err());
    assert!(validate_profile_name("deep-review_2").is_ok());
}

#[test]
fn creates_and_reads_a_valid_config() {
    let home = temp_home("save");
    let path = home.join(".claude/settings.json");
    let initial = read_config_from_home(&home, &path).expect("missing config should be editable");
    let saved = save_config_from_home(&home, &path, &initial.sha256, "{\"theme\":\"dark\"}\n")
        .expect("valid config should save");
    assert!(saved.exists);
    assert!(saved.updated_at.is_some());
    assert_eq!(
        read_config_from_home(&home, &path)
            .expect("saved config should read")
            .content,
        "{\"theme\":\"dark\"}\n"
    );
    fs::remove_dir_all(home).expect("temporary home should be removable");
}

#[test]
fn rejects_invalid_or_stale_writes() {
    let home = temp_home("reject");
    let path = home.join(".codex/config.toml");
    let initial = read_config_from_home(&home, &path).expect("missing config should be editable");
    assert!(
        save_config_from_home(&home, &path, &initial.sha256, "[broken")
            .expect_err("invalid TOML should fail")
            .to_string()
            .contains("invalid TOML")
    );
    save_config_from_home(&home, &path, &initial.sha256, "model = \"one\"\n")
        .expect("valid TOML should save");
    let stale_error = save_config_from_home(&home, &path, &initial.sha256, "model = \"two\"\n")
        .expect_err("stale write should fail");
    assert!(stale_error.to_string().contains("changed on disk"));
    let conflict = stale_error
        .downcast_ref::<ConfigChangedError>()
        .expect("stale write should carry the current snapshot");
    assert_eq!(conflict.current.content, "model = \"one\"\n");
    assert!(conflict.current.exists);
    fs::remove_dir_all(home).expect("temporary home should be removable");
}

#[test]
fn rejects_paths_outside_the_catalog() {
    let home = temp_home("path");
    let error = read_config_from_home(&home, &home.join(".ssh/config"))
        .expect_err("arbitrary files must not be readable");
    assert!(error.to_string().contains("unsupported agent config path"));
}
