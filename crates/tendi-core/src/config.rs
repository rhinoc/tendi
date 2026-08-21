use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

use crate::{
    fsutil::{atomic_write, sha256_text},
    skills::AgentKind,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigFile {
    pub agent: AgentKind,
    pub label: String,
    pub path: PathBuf,
    pub format: String,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigContent {
    pub path: PathBuf,
    pub content: String,
    pub sha256: String,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug)]
pub struct ConfigChangedError {
    pub current: AgentConfigContent,
}

impl std::fmt::Display for ConfigChangedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "config changed on disk; review it before saving")
    }
}

impl std::error::Error for ConfigChangedError {}

pub fn list_agent_configs() -> Result<Vec<AgentConfigFile>> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    Ok(configs_for_environment(&home))
}

pub fn read_agent_config(path: &Path) -> Result<AgentConfigContent> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    let codex_home = codex_home_for_environment(&home);
    let config = resolve_config_for_path(&home, &codex_home, path)?;
    read_config(&config)
}

pub fn save_agent_config(
    path: &Path,
    expected_sha256: &str,
    content: &str,
) -> Result<AgentConfigContent> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    let codex_home = codex_home_for_environment(&home);
    let config = resolve_config_for_path(&home, &codex_home, path)?;
    save_config(&config, expected_sha256, content)
}

pub fn create_config_profile(
    agent: AgentKind,
    name: &str,
    content: &str,
) -> Result<AgentConfigContent> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    let codex_home = codex_home_for_environment(&home);
    create_profile_for_roots(agent, &home, &codex_home, name, content)
}

pub fn config_profile_exists(agent: AgentKind, name: &str) -> Result<bool> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    let codex_home = codex_home_for_environment(&home);
    Ok(config_profile_path_for_roots(agent, &home, &codex_home, name)?.is_file())
}

pub fn config_profile_path(agent: AgentKind, name: &str) -> Result<PathBuf> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    let codex_home = codex_home_for_environment(&home);
    config_profile_path_for_roots(agent, &home, &codex_home, name)
}

pub fn validate_profile_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("invalid config profile name; use letters, numbers, hyphens, or underscores");
    }
    Ok(())
}

#[cfg(test)]
fn configs_for_home(home: &Path) -> Vec<AgentConfigFile> {
    configs_for_roots(home, &home.join(".codex"))
}

fn configs_for_environment(home: &Path) -> Vec<AgentConfigFile> {
    let codex_home = codex_home_for_environment(home);
    configs_for_roots(home, &codex_home)
}

fn codex_home_for_environment(home: &Path) -> PathBuf {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
}

fn configs_for_roots(home: &Path, codex_home: &Path) -> Vec<AgentConfigFile> {
    let mut configs = crate::providers::agent_providers()
        .into_iter()
        .flat_map(|provider| provider.config_files(home, codex_home))
        .collect::<Vec<_>>();
    for config in &mut configs {
        config.updated_at = file_updated_at(&config.path);
    }
    configs.sort_by_key(|config| crate::providers::agent_provider(config.agent).config_order());
    configs
}

pub(crate) fn profile_paths_for_root(root: &Path, suffix: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let name = path.file_name()?.to_str()?.strip_suffix(suffix)?;
            validate_profile_name(name).ok().map(|_| name.to_string())
        })
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|profile| root.join(format!("{profile}{suffix}")))
        .collect()
}

pub(crate) fn cursor_profile_paths_for_root(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() || !path.join("cli-config.json").is_file() {
                return None;
            }
            let name = entry.file_name().to_str()?.to_string();
            validate_profile_name(&name).ok().map(|_| name)
        })
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|profile| root.join(&profile).join("cli-config.json"))
        .collect()
}

fn config_profile_path_for_roots(
    agent: AgentKind,
    home: &Path,
    codex_home: &Path,
    name: &str,
) -> Result<PathBuf> {
    validate_profile_name(name)?;
    crate::providers::agent_provider(agent)
        .config_profile_path(home, codex_home, name)
        .ok_or_else(|| anyhow::anyhow!("config profiles are not supported for this agent"))
}

fn resolve_config_for_path(home: &Path, codex_home: &Path, path: &Path) -> Result<AgentConfigFile> {
    crate::providers::agent_providers()
        .into_iter()
        .find_map(|provider| provider.config_file_for_path(home, codex_home, path))
        .ok_or_else(|| anyhow::anyhow!("unsupported agent config path: {}", path.display()))
}

#[cfg(test)]
fn read_config_from_home(home: &Path, path: &Path) -> Result<AgentConfigContent> {
    let config = resolve_config_for_path(home, &home.join(".codex"), path)?;
    read_config(&config)
}

fn read_config(config: &AgentConfigFile) -> Result<AgentConfigContent> {
    let exists = config.path.is_file();
    let content = if exists {
        fs::read_to_string(&config.path)
            .with_context(|| format!("failed to read {}", config.path.display()))?
    } else if config.format == "json" {
        "{}\n".to_string()
    } else {
        String::new()
    };
    let updated_at = file_updated_at(&config.path);
    Ok(AgentConfigContent {
        path: config.path.clone(),
        sha256: sha256_text(if exists { &content } else { "" }),
        content,
        exists,
        updated_at,
    })
}

#[cfg(test)]
fn save_config_from_home(
    home: &Path,
    path: &Path,
    expected_sha256: &str,
    content: &str,
) -> Result<AgentConfigContent> {
    let config = resolve_config_for_path(home, &home.join(".codex"), path)?;
    save_config(&config, expected_sha256, content)
}

fn save_config(
    config: &AgentConfigFile,
    expected_sha256: &str,
    content: &str,
) -> Result<AgentConfigContent> {
    let current = if config.path.is_file() {
        fs::read_to_string(&config.path)
            .with_context(|| format!("failed to read {}", config.path.display()))?
    } else {
        String::new()
    };
    if sha256_text(&current) != expected_sha256 {
        return Err(ConfigChangedError {
            current: read_config(config)?,
        }
        .into());
    }
    validate_config(&config.format, content)?;
    atomic_write(&config.path, content)?;
    let updated_at = file_updated_at(&config.path);
    Ok(AgentConfigContent {
        path: config.path.clone(),
        content: content.to_string(),
        sha256: sha256_text(content),
        exists: true,
        updated_at,
    })
}

fn create_profile_for_roots(
    agent: AgentKind,
    home: &Path,
    codex_home: &Path,
    name: &str,
    content: &str,
) -> Result<AgentConfigContent> {
    let format = crate::providers::agent_provider(agent)
        .config_profile_format()
        .ok_or_else(|| anyhow::anyhow!("config profiles are not supported for this agent"))?;
    let path = config_profile_path_for_roots(agent, home, codex_home, name)?;
    validate_config(format, content)?;
    if fs::symlink_metadata(&path).is_ok() {
        bail!("config profile already exists: {name}");
    }
    atomic_write(&path, content)?;
    let updated_at = file_updated_at(&path);
    Ok(AgentConfigContent {
        path,
        content: content.to_string(),
        sha256: sha256_text(content),
        exists: true,
        updated_at,
    })
}

fn file_updated_at(path: &Path) -> Option<String> {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .map(|modified| {
            DateTime::<Utc>::from(modified).to_rfc3339_opts(SecondsFormat::Millis, true)
        })
}

fn validate_config(format: &str, content: &str) -> Result<()> {
    match format {
        "json" => {
            serde_json::from_str::<serde_json::Value>(content).context("invalid JSON config")?;
        }
        "toml" => {
            toml::from_str::<toml::Value>(content).context("invalid TOML config")?;
        }
        _ => bail!("unsupported config format: {format}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
    fn lists_supported_agent_configs() {
        let home = temp_home("list");
        let configs = configs_for_home(&home);
        assert_eq!(configs.len(), 3);
        assert_eq!(configs[0].path, home.join(".codex/config.toml"));
        assert_eq!(configs[1].path, home.join(".claude/settings.json"));
        assert_eq!(configs[2].path, home.join(".cursor/cli-config.json"));
    }

    #[test]
    fn lists_config_updated_at_for_existing_files() {
        let home = temp_home("updated-at");
        let path = home.join(".codex/config.toml");
        fs::create_dir_all(path.parent().expect("config parent")).expect("create config home");
        fs::write(&path, "model = \"one\"\n").expect("write config");

        let configs = configs_for_home(&home);
        let config = configs
            .iter()
            .find(|config| config.path == path)
            .expect("config should be listed");
        assert!(config.updated_at.is_some());

        fs::remove_dir_all(home).expect("temporary home should be removable");
    }

    #[test]
    fn accepts_a_custom_codex_home() {
        let home = temp_home("custom-codex");
        let codex_home = home.join("custom-codex");
        let configs = configs_for_roots(&home, &codex_home);
        assert_eq!(configs[0].path, codex_home.join("config.toml"));
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
            let config = resolve_config_for_path(&home, &codex_home, &path)
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

        let configs = configs_for_roots(&home, &codex_home);
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

        let configs = configs_for_roots(&home, &home.join(".codex"));
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

        let configs = configs_for_roots(&home, &home.join(".codex"));
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
        let created =
            create_profile_for_roots(AgentKind::Codex, &home, &codex_home, "deep-review", content)
                .expect("valid profile should be created");
        assert_eq!(created.content, content);
        assert_eq!(
            fs::read_to_string(codex_home.join("deep-review.config.toml"))
                .expect("profile should be readable"),
            content
        );
        assert!(
            create_profile_for_roots(AgentKind::Codex, &home, &codex_home, "deep-review", content,)
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
        let created = create_profile_for_roots(
            AgentKind::Claude,
            &home,
            &home.join(".codex"),
            "safe-mode",
            content,
        )
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
        let created = create_profile_for_roots(
            AgentKind::Cursor,
            &home,
            &home.join(".codex"),
            "safe-mode",
            content,
        )
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
    fn rejects_unsafe_config_profile_names() {
        assert!(validate_profile_name("../escape").is_err());
        assert!(validate_profile_name("deep review").is_err());
        assert!(validate_profile_name("deep-review_2").is_ok());
    }

    #[test]
    fn creates_and_reads_a_valid_config() {
        let home = temp_home("save");
        let path = home.join(".claude/settings.json");
        let initial =
            read_config_from_home(&home, &path).expect("missing config should be editable");
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
        let initial =
            read_config_from_home(&home, &path).expect("missing config should be editable");
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
}
