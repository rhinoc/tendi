use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = env::temp_dir().join(format!(
        "tendi-cli-install-{name}-{}-{unique}",
        std::process::id()
    ));
    let bundled = root.join("tendi.app/Contents/MacOS/tendi");
    let command = root.join("bin/tendi");
    fs::create_dir_all(bundled.parent().unwrap()).unwrap();
    fs::write(&bundled, "binary").unwrap();
    let mut permissions = fs::metadata(&bundled).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&bundled, permissions).unwrap();
    (root, bundled, command)
}

#[test]
fn installs_and_removes_the_bundled_cli() {
    let (root, bundled, command) = fixture("install");
    let installer = Installer::for_test(
        bundled.clone(),
        command.clone(),
        command.parent().unwrap().display().to_string(),
    );

    assert_eq!(
        installer.status().unwrap().state,
        CliInstallState::NotInstalled
    );
    let installed = installer.install().unwrap();
    assert_eq!(installed.state, CliInstallState::Installed);
    assert!(installed.path_configured);
    assert_eq!(fs::read_link(&command).unwrap(), bundled);
    assert_eq!(
        installer.remove().unwrap().state,
        CliInstallState::NotInstalled
    );
    assert!(!command.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn installs_and_removes_the_dev_cli() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = env::temp_dir().join(format!(
        "tendi-cli-install-dev-{}-{unique}",
        std::process::id()
    ));
    let bundled = root.join("target/tauri-dev/debug/tendi");
    let command = root.join("bin/tendi");
    fs::create_dir_all(bundled.parent().unwrap()).unwrap();
    fs::write(&bundled, "dev binary").unwrap();
    let mut permissions = fs::metadata(&bundled).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&bundled, permissions).unwrap();
    let installer = Installer::for_test_with_development(
        bundled.clone(),
        command.clone(),
        command.parent().unwrap().display().to_string(),
        true,
    );

    let before = installer.status().unwrap();
    assert_eq!(before.state, CliInstallState::NotInstalled);
    assert!(before.supported);
    assert_eq!(
        installer.install().unwrap().state,
        CliInstallState::Installed
    );
    assert_eq!(fs::read_link(&command).unwrap(), bundled);
    assert_eq!(
        installer.remove().unwrap().state,
        CliInstallState::NotInstalled
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn refuses_to_replace_an_unmanaged_command() {
    let (root, bundled, command) = fixture("conflict");
    fs::create_dir_all(command.parent().unwrap()).unwrap();
    fs::write(&command, "user command").unwrap();
    let installer = Installer::for_test(
        bundled,
        command.clone(),
        command.parent().unwrap().display().to_string(),
    );

    assert_eq!(installer.status().unwrap().state, CliInstallState::Conflict);
    assert!(
        installer
            .install()
            .unwrap_err()
            .to_string()
            .contains("Refusing")
    );
    assert_eq!(fs::read_to_string(command).unwrap(), "user command");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn refuses_to_replace_an_unmanaged_symlink() {
    let (root, bundled, command) = fixture("symlink-conflict");
    let other = root.join("Other.app/Contents/MacOS/tendi");
    fs::create_dir_all(command.parent().unwrap()).unwrap();
    symlink(&other, &command).unwrap();
    let installer = Installer::for_test(
        bundled,
        command.clone(),
        command.parent().unwrap().display().to_string(),
    );

    assert_eq!(installer.status().unwrap().state, CliInstallState::Conflict);
    assert!(
        installer
            .install()
            .unwrap_err()
            .to_string()
            .contains("Refusing")
    );
    assert_eq!(fs::read_link(command).unwrap(), other);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repairs_a_managed_link_to_an_old_app() {
    let (root, bundled, command) = fixture("stale");
    let old = root.join("old/tendi.app/Contents/MacOS/tendi");
    fs::create_dir_all(command.parent().unwrap()).unwrap();
    symlink(&old, &command).unwrap();
    let installer = Installer::for_test(
        bundled.clone(),
        command.clone(),
        command.parent().unwrap().display().to_string(),
    );

    assert_eq!(installer.status().unwrap().state, CliInstallState::Stale);
    assert_eq!(
        installer.install().unwrap().state,
        CliInstallState::Installed
    );
    assert_eq!(fs::read_link(command).unwrap(), bundled);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prefers_user_local_bin_when_it_is_on_path() {
    let (root, _bundled, _) = fixture("user-local");
    let home = root.join("home");
    let user_bin = home.join(".local/bin");
    let command = choose_command_path(&home, &user_bin.display().to_string());
    assert_eq!(command, user_bin.join("tendi"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn shell_quoting_preserves_apostrophes() {
    assert_eq!(
        quote_shell(Path::new("/tmp/Tendi's App")),
        "'/tmp/Tendi'\"'\"'s App'"
    );
}
