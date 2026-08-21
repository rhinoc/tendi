use anyhow::{Context, Result, bail};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CliInstallState {
    Installed,
    NotInstalled,
    Stale,
    Conflict,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliInstallStatus {
    pub state: CliInstallState,
    pub supported: bool,
    pub command_path: Option<String>,
    pub bundled_path: Option<String>,
    pub path_configured: bool,
    pub current_target: Option<String>,
    pub detail: String,
}

pub fn status() -> Result<CliInstallStatus> {
    platform::Installer::discover()?.status()
}

pub fn install() -> Result<CliInstallStatus> {
    platform::Installer::discover()?.install()
}

pub fn remove() -> Result<CliInstallStatus> {
    platform::Installer::discover()?.remove()
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    pub struct Installer;

    impl Installer {
        pub fn discover() -> Result<Self> {
            Ok(Self)
        }

        pub fn status(&self) -> Result<CliInstallStatus> {
            Ok(unsupported_status())
        }

        pub fn install(&self) -> Result<CliInstallStatus> {
            bail!("CLI registration is currently supported only on macOS")
        }

        pub fn remove(&self) -> Result<CliInstallStatus> {
            Ok(unsupported_status())
        }
    }

    fn unsupported_status() -> CliInstallStatus {
        CliInstallStatus {
            state: CliInstallState::Unsupported,
            supported: false,
            command_path: None,
            bundled_path: None,
            path_configured: false,
            current_target: None,
            detail: "CLI registration is currently supported only on macOS".to_string(),
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::{
        env, fs, io,
        io::Read,
        os::unix::{fs::PermissionsExt, fs::symlink},
        path::{Path, PathBuf},
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    use super::*;

    const COMMAND_NAME: &str = "tendi";
    const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(2);

    #[derive(Debug)]
    pub struct Installer {
        bundled_path: PathBuf,
        command_path: PathBuf,
        shell_path: String,
        allow_privileged_install: bool,
        development: bool,
    }

    impl Installer {
        pub fn discover() -> Result<Self> {
            let executable = env::current_exe().context("resolve the Tendi app executable")?;
            let executable_dir = executable
                .parent()
                .context("the Tendi app executable has no parent directory")?;
            let bundled_path = executable_dir.join(COMMAND_NAME);
            let development = is_dev_app(&executable);
            let home = dirs::home_dir().context("resolve the user home directory")?;
            let shell_path = login_shell_path().unwrap_or_else(process_path);
            let command_path = choose_command_path(&home, &shell_path);
            Ok(Self {
                bundled_path,
                command_path,
                shell_path,
                allow_privileged_install: true,
                development,
            })
        }

        #[cfg(test)]
        fn for_test(bundled_path: PathBuf, command_path: PathBuf, shell_path: String) -> Self {
            Self::for_test_with_development(bundled_path, command_path, shell_path, false)
        }

        #[cfg(test)]
        fn for_test_with_development(
            bundled_path: PathBuf,
            command_path: PathBuf,
            shell_path: String,
            development: bool,
        ) -> Self {
            Self {
                bundled_path,
                command_path,
                shell_path,
                allow_privileged_install: false,
                development,
            }
        }

        pub fn status(&self) -> Result<CliInstallStatus> {
            let unsupported_detail = if is_unstable_app_location(&self.bundled_path) {
                Some("Move Tendi to Applications before registering its CLI.".to_string())
            } else if (!is_managed_tendi_target(&self.bundled_path) && !self.development)
                || !is_executable_file(&self.bundled_path)
            {
                Some("This Tendi app build does not contain the bundled CLI.".to_string())
            } else {
                None
            };
            if let Some(detail) = unsupported_detail {
                return Ok(CliInstallStatus {
                    state: CliInstallState::Unsupported,
                    supported: false,
                    command_path: Some(display_path(&self.command_path)),
                    bundled_path: Some(display_path(&self.bundled_path)),
                    path_configured: path_contains_dir(
                        &self.shell_path,
                        self.command_path.parent().unwrap_or(Path::new("/")),
                    ),
                    current_target: None,
                    detail,
                });
            }

            let inspected = inspect_command(&self.command_path, &self.bundled_path)?;
            let path_configured = path_contains_dir(
                &self.shell_path,
                self.command_path.parent().unwrap_or(Path::new("/")),
            );
            let detail = status_detail(
                inspected.state,
                &self.command_path,
                path_configured,
                inspected.current_target.as_deref(),
            );
            Ok(CliInstallStatus {
                state: inspected.state,
                supported: true,
                command_path: Some(display_path(&self.command_path)),
                bundled_path: Some(display_path(&self.bundled_path)),
                path_configured,
                current_target: inspected.current_target.map(|path| display_path(&path)),
                detail,
            })
        }

        pub fn install(&self) -> Result<CliInstallStatus> {
            let before = self.status()?;
            match before.state {
                CliInstallState::Installed => return Ok(before),
                CliInstallState::Conflict => {
                    bail!(
                        "Refusing to replace a non-Tendi command at {}",
                        self.command_path.display()
                    )
                }
                CliInstallState::Unsupported => bail!("{}", before.detail),
                CliInstallState::NotInstalled | CliInstallState::Stale => {}
            }

            let expected_link = fs::read_link(&self.command_path).ok();
            if let Err(error) = self.install_direct() {
                if !self.allow_privileged_install || !is_permission_error(&error) {
                    return Err(error);
                }
                self.install_privileged(before.state, expected_link.as_deref())?;
            }

            let after = self.status()?;
            if after.state != CliInstallState::Installed {
                bail!("CLI registration did not produce a valid Tendi command")
            }
            Ok(after)
        }

        pub fn remove(&self) -> Result<CliInstallStatus> {
            let before = self.status()?;
            match before.state {
                CliInstallState::NotInstalled | CliInstallState::Unsupported => return Ok(before),
                CliInstallState::Conflict => {
                    bail!(
                        "Refusing to remove a non-Tendi command at {}",
                        self.command_path.display()
                    )
                }
                CliInstallState::Installed | CliInstallState::Stale => {}
            }

            let expected_link = fs::read_link(&self.command_path)
                .with_context(|| format!("read {}", self.command_path.display()))?;
            if let Err(error) =
                remove_managed_link(&self.command_path, &self.bundled_path, Some(&expected_link))
            {
                if !self.allow_privileged_install || !is_permission_error(&error) {
                    return Err(error);
                }
                self.remove_privileged(&expected_link)?;
            }
            self.status()
        }

        fn install_direct(&self) -> Result<()> {
            let inspected = inspect_command(&self.command_path, &self.bundled_path)?;
            match inspected.state {
                CliInstallState::Installed => return Ok(()),
                CliInstallState::Conflict => {
                    bail!(
                        "Refusing to replace a non-Tendi command at {}",
                        self.command_path.display()
                    )
                }
                CliInstallState::Stale => {
                    remove_managed_link(&self.command_path, &self.bundled_path, None)?;
                }
                CliInstallState::NotInstalled => {}
                CliInstallState::Unsupported => unreachable!(),
            }

            let parent = self
                .command_path
                .parent()
                .context("the CLI command path has no parent directory")?;
            fs::create_dir_all(parent)
                .with_context(|| format!("create CLI directory {}", parent.display()))?;
            symlink(&self.bundled_path, &self.command_path).with_context(|| {
                format!(
                    "link {} to {}",
                    self.command_path.display(),
                    self.bundled_path.display()
                )
            })?;
            Ok(())
        }

        fn install_privileged(
            &self,
            state: CliInstallState,
            expected_link: Option<&Path>,
        ) -> Result<()> {
            let parent = self
                .command_path
                .parent()
                .context("the CLI command path has no parent directory")?;
            let command = match (state, expected_link) {
                (CliInstallState::Stale, Some(expected)) => format!(
                    concat!(
                        "test -L {path} && test \"$(/usr/bin/readlink {path})\" = {expected} && ",
                        "/bin/rm -- {path} && /bin/mkdir -p -- {parent} && ",
                        "/bin/ln -s -- {target} {path}"
                    ),
                    path = quote_shell(&self.command_path),
                    expected = quote_shell(expected),
                    parent = quote_shell(parent),
                    target = quote_shell(&self.bundled_path),
                ),
                (CliInstallState::NotInstalled, _) => format!(
                    "test ! -e {path} && test ! -L {path} && /bin/mkdir -p -- {parent} && /bin/ln -s -- {target} {path}",
                    path = quote_shell(&self.command_path),
                    parent = quote_shell(parent),
                    target = quote_shell(&self.bundled_path),
                ),
                _ => bail!("CLI registration changed while requesting administrator access"),
            };
            run_privileged(&command)
        }

        fn remove_privileged(&self, expected_link: &Path) -> Result<()> {
            let command = format!(
                "test -L {path} && test \"$(/usr/bin/readlink {path})\" = {expected} && /bin/rm -- {path}",
                path = quote_shell(&self.command_path),
                expected = quote_shell(expected_link),
            );
            run_privileged(&command)
        }
    }

    #[derive(Debug)]
    struct InspectedCommand {
        state: CliInstallState,
        current_target: Option<PathBuf>,
    }

    fn inspect_command(command_path: &Path, bundled_path: &Path) -> Result<InspectedCommand> {
        let metadata = match fs::symlink_metadata(command_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(InspectedCommand {
                    state: CliInstallState::NotInstalled,
                    current_target: None,
                });
            }
            Err(error) => {
                return Err(error).with_context(|| format!("inspect {}", command_path.display()));
            }
        };
        if !metadata.file_type().is_symlink() {
            return Ok(InspectedCommand {
                state: CliInstallState::Conflict,
                current_target: None,
            });
        }

        let raw_target = fs::read_link(command_path)
            .with_context(|| format!("read link {}", command_path.display()))?;
        let resolved_target = if raw_target.is_absolute() {
            raw_target
        } else {
            command_path
                .parent()
                .unwrap_or(Path::new("/"))
                .join(raw_target)
        };
        let state = if resolved_target == bundled_path {
            CliInstallState::Installed
        } else if is_managed_tendi_target(&resolved_target) {
            CliInstallState::Stale
        } else {
            CliInstallState::Conflict
        };
        Ok(InspectedCommand {
            state,
            current_target: Some(resolved_target),
        })
    }

    fn remove_managed_link(
        command_path: &Path,
        bundled_path: &Path,
        expected_raw_target: Option<&Path>,
    ) -> Result<()> {
        let metadata = fs::symlink_metadata(command_path)
            .with_context(|| format!("inspect {}", command_path.display()))?;
        if !metadata.file_type().is_symlink() {
            bail!(
                "Refusing to remove a non-symlink at {}",
                command_path.display()
            )
        }
        let raw_target = fs::read_link(command_path)
            .with_context(|| format!("read link {}", command_path.display()))?;
        if expected_raw_target.is_some_and(|expected| raw_target != expected) {
            bail!("CLI registration changed before it could be removed")
        }
        let resolved_target = if raw_target.is_absolute() {
            raw_target
        } else {
            command_path
                .parent()
                .unwrap_or(Path::new("/"))
                .join(raw_target)
        };
        if resolved_target != bundled_path && !is_managed_tendi_target(&resolved_target) {
            bail!(
                "Refusing to remove a non-Tendi command at {}",
                command_path.display()
            )
        }
        fs::remove_file(command_path)
            .with_context(|| format!("remove CLI link {}", command_path.display()))?;
        Ok(())
    }

    fn choose_command_path(home: &Path, shell_path: &str) -> PathBuf {
        for directory in split_path(shell_path) {
            let candidate = directory.join(COMMAND_NAME);
            if fs::symlink_metadata(&candidate).is_ok() {
                return candidate;
            }
        }

        let user_local_bin = home.join(".local/bin");
        if path_contains_dir(shell_path, &user_local_bin) {
            return user_local_bin.join(COMMAND_NAME);
        }

        PathBuf::from("/usr/local/bin/tendi")
    }

    fn status_detail(
        state: CliInstallState,
        command_path: &Path,
        path_configured: bool,
        current_target: Option<&Path>,
    ) -> String {
        match state {
            CliInstallState::Installed if path_configured => {
                format!("Available at {}.", command_path.display())
            }
            CliInstallState::Installed => format!(
                "{} is registered, but its directory is not in your login shell PATH.",
                command_path.display()
            ),
            CliInstallState::NotInstalled => {
                format!(
                    "Register {} to use Tendi from the terminal.",
                    command_path.display()
                )
            }
            CliInstallState::Stale => format!(
                "{} points to an older Tendi app at {}.",
                command_path.display(),
                current_target
                    .map(display_path)
                    .unwrap_or_else(|| "an unknown path".to_string())
            ),
            CliInstallState::Conflict => format!(
                "{} already exists and is not managed by Tendi.",
                command_path.display()
            ),
            CliInstallState::Unsupported => {
                "This Tendi app build does not contain the bundled CLI.".to_string()
            }
        }
    }

    fn is_managed_tendi_target(path: &Path) -> bool {
        let Some(file_name) = path.file_name() else {
            return false;
        };
        if file_name != COMMAND_NAME {
            return false;
        }
        let Some(macos_dir) = path.parent() else {
            return false;
        };
        if macos_dir.file_name().is_none_or(|name| name != "MacOS") {
            return false;
        }
        let Some(contents_dir) = macos_dir.parent() else {
            return false;
        };
        if contents_dir
            .file_name()
            .is_none_or(|name| name != "Contents")
        {
            return false;
        }
        contents_dir.parent().is_some_and(|app_dir| {
            app_dir
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("tendi.app"))
        })
    }

    fn is_dev_app(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "tendi-desktop")
            && path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "debug")
    }

    fn is_executable_file(path: &Path) -> bool {
        fs::metadata(path)
            .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    fn is_unstable_app_location(path: &Path) -> bool {
        path.starts_with("/Volumes")
            || path
                .components()
                .any(|component| component.as_os_str() == "AppTranslocation")
    }

    fn process_path() -> String {
        env::var("PATH").unwrap_or_default()
    }

    fn login_shell_path() -> Option<String> {
        let shell = env::var_os("SHELL")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && path.is_file())
            .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
        let mut child = Command::new(shell)
            .args(["-l", "-c", "printf '\\n__TENDI_PATH__%s\\n' \"$PATH\""])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let (output_tx, output_rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let mut output = String::new();
            let _ = stdout.read_to_string(&mut output);
            let _ = output_tx.send(output);
        });
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() < LOGIN_SHELL_TIMEOUT => {
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                Err(_) => return None,
            }
        }
        let output = output_rx.recv_timeout(Duration::from_millis(200)).ok()?;
        output
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix("__TENDI_PATH__"))
            .map(str::to_string)
    }

    fn split_path(value: &str) -> impl Iterator<Item = PathBuf> + '_ {
        value
            .split(':')
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                if entry == "~" {
                    dirs::home_dir().unwrap_or_else(|| PathBuf::from(entry))
                } else if let Some(relative) = entry.strip_prefix("~/") {
                    dirs::home_dir()
                        .map(|home| home.join(relative))
                        .unwrap_or_else(|| PathBuf::from(entry))
                } else {
                    PathBuf::from(entry)
                }
            })
    }

    fn path_contains_dir(value: &str, expected: &Path) -> bool {
        split_path(value).any(|entry| entry == expected)
    }

    fn is_permission_error(error: &anyhow::Error) -> bool {
        error.chain().any(|cause| {
            cause
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::PermissionDenied)
        })
    }

    fn run_privileged(command: &str) -> Result<()> {
        let script = format!(
            "do shell script {} with administrator privileges",
            quote_applescript(command)
        );
        let output = Command::new("/usr/bin/osascript")
            .args(["-e", &script])
            .output()
            .context("request administrator access for CLI registration")?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "CLI registration was not completed{}",
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        )
    }

    fn quote_shell(path: &Path) -> String {
        let value = path.to_string_lossy();
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }

    fn quote_applescript(value: &str) -> String {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }

    fn display_path(path: &Path) -> String {
        path.display().to_string()
    }

    #[cfg(test)]
    mod tests {
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
    }
}
