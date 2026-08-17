use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
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
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigContent {
    pub path: PathBuf,
    pub content: String,
    pub sha256: String,
    pub exists: bool,
}

pub fn list_agent_configs() -> Result<Vec<AgentConfigFile>> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    Ok(configs_for_environment(&home))
}

pub fn read_agent_config(path: &Path) -> Result<AgentConfigContent> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    let configs = configs_for_environment(&home);
    read_config(&configs, path)
}

pub fn save_agent_config(
    path: &Path,
    expected_sha256: &str,
    content: &str,
) -> Result<AgentConfigContent> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    let configs = configs_for_environment(&home);
    save_config(&configs, path, expected_sha256, content)
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
    configs.sort_by_key(|config| crate::providers::agent_provider(config.agent).config_order());
    configs
}

pub(crate) fn profile_configs_for_root(
    agent: AgentKind,
    root: &Path,
    suffix: &str,
    format: &str,
    label: &str,
) -> Vec<AgentConfigFile> {
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
        .map(|profile| AgentConfigFile {
            agent,
            label: format!("{label} / {profile}"),
            path: root.join(format!("{profile}{suffix}")),
            format: format.to_string(),
            exists: true,
            profile: Some(profile),
        })
        .collect()
}

pub(crate) fn cursor_profile_configs_for_root(root: &Path) -> Vec<AgentConfigFile> {
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
        .map(|profile| AgentConfigFile {
            agent: AgentKind::Cursor,
            label: format!("Cursor / {profile}"),
            path: root.join(&profile).join("cli-config.json"),
            format: "json".to_string(),
            exists: true,
            profile: Some(profile),
        })
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

fn resolve_config(configs: &[AgentConfigFile], path: &Path) -> Result<AgentConfigFile> {
    configs
        .iter()
        .cloned()
        .into_iter()
        .find(|config| config.path == path)
        .ok_or_else(|| anyhow::anyhow!("unsupported agent config path: {}", path.display()))
}

#[cfg(test)]
fn read_config_from_home(home: &Path, path: &Path) -> Result<AgentConfigContent> {
    read_config(&configs_for_home(home), path)
}

fn read_config(configs: &[AgentConfigFile], path: &Path) -> Result<AgentConfigContent> {
    let config = resolve_config(configs, path)?;
    let exists = config.path.is_file();
    let content = if exists {
        fs::read_to_string(&config.path)
            .with_context(|| format!("failed to read {}", config.path.display()))?
    } else if config.format == "json" {
        "{}\n".to_string()
    } else {
        String::new()
    };
    Ok(AgentConfigContent {
        path: config.path,
        sha256: sha256_text(if exists { &content } else { "" }),
        content,
        exists,
    })
}

#[cfg(test)]
fn save_config_from_home(
    home: &Path,
    path: &Path,
    expected_sha256: &str,
    content: &str,
) -> Result<AgentConfigContent> {
    save_config(&configs_for_home(home), path, expected_sha256, content)
}

fn save_config(
    configs: &[AgentConfigFile],
    path: &Path,
    expected_sha256: &str,
    content: &str,
) -> Result<AgentConfigContent> {
    let config = resolve_config(configs, path)?;
    let current = if config.path.is_file() {
        fs::read_to_string(&config.path)
            .with_context(|| format!("failed to read {}", config.path.display()))?
    } else {
        String::new()
    };
    if sha256_text(&current) != expected_sha256 {
        bail!("config changed on disk; reload it before saving");
    }
    validate_config(&config.format, content)?;
    atomic_write(&config.path, content)?;
    Ok(AgentConfigContent {
        path: config.path,
        content: content.to_string(),
        sha256: sha256_text(content),
        exists: true,
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
    Ok(AgentConfigContent {
        path,
        content: content.to_string(),
        sha256: sha256_text(content),
        exists: true,
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
    fn accepts_a_custom_codex_home() {
        let home = temp_home("custom-codex");
        let codex_home = home.join("custom-codex");
        let configs = configs_for_roots(&home, &codex_home);
        assert_eq!(configs[0].path, codex_home.join("config.toml"));
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
        assert!(
            save_config_from_home(&home, &path, &initial.sha256, "model = \"two\"\n")
                .expect_err("stale write should fail")
                .to_string()
                .contains("changed on disk")
        );
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
