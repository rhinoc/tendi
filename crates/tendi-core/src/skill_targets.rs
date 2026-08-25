use std::{
    env, fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize};

use crate::AgentKind;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillInstallScope {
    Global,
    Project,
}

impl FromStr for SkillInstallScope {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "global" => Ok(Self::Global),
            "project" => Ok(Self::Project),
            _ => bail!("unknown skill installation scope: {value}"),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SkillTarget(String);

impl SkillTarget {
    pub fn id(&self) -> &str {
        &self.0
    }

    pub fn uses_shared_layout(&self) -> bool {
        crate::providers::skill_target_uses_shared_layout(&self.0)
    }

    pub fn agent_kind(&self) -> AgentKind {
        if matches!(self.0.as_str(), "shared" | "universal") {
            return AgentKind::Shared;
        }
        crate::providers::all_providers()
            .into_iter()
            .find(|provider| provider.storage_key() == self.0)
            .map(|provider| provider.kind())
            .unwrap_or(AgentKind::Unknown)
    }
}

impl fmt::Display for SkillTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for SkillTarget {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let value = value.trim().to_ascii_lowercase();
        if value == "shared"
            || value == "claude"
            || target_catalog().iter().any(|item| item.id == value)
        {
            Ok(Self(value))
        } else {
            bail!(
                "unknown skill target `{value}`; run `tendi skills targets` to list supported targets"
            )
        }
    }
}

impl<'de> Deserialize<'de> for SkillTarget {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl From<AgentKind> for SkillTarget {
    fn from(value: AgentKind) -> Self {
        Self(
            crate::providers::agent_provider(value)
                .storage_key()
                .to_string(),
        )
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
pub struct SkillTargetConfig {
    pub id: &'static str,
    pub display_name: &'static str,
    pub project_skills_dir: &'static str,
    #[serde(skip)]
    global: GlobalRoot,
}

impl SkillTargetConfig {
    pub fn supports_global(self) -> bool {
        !matches!(self.global, GlobalRoot::Unsupported)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum GlobalRoot {
    Home(&'static str),
    Config(&'static str),
    Env {
        name: &'static str,
        fallback: &'static str,
    },
    OpenClaw,
    Unsupported,
}

macro_rules! target {
    ($id:literal, $display:literal, $project:literal, $global:expr) => {
        SkillTargetConfig {
            id: $id,
            display_name: $display,
            project_skills_dir: $project,
            global: $global,
        }
    };
}

static TARGETS: &[SkillTargetConfig] = &[
    target!(
        "aider-desk",
        "AiderDesk",
        ".aider-desk/skills",
        GlobalRoot::Home(".aider-desk/skills")
    ),
    target!(
        "amp",
        "Amp",
        ".agents/skills",
        GlobalRoot::Config("agents/skills")
    ),
    target!(
        "antigravity",
        "Antigravity",
        ".agents/skills",
        GlobalRoot::Home(".gemini/antigravity/skills")
    ),
    target!(
        "antigravity-cli",
        "Antigravity CLI",
        ".agents/skills",
        GlobalRoot::Home(".gemini/antigravity-cli/skills")
    ),
    target!(
        "astrbot",
        "AstrBot",
        "data/skills",
        GlobalRoot::Home(".astrbot/data/skills")
    ),
    target!(
        "autohand-code",
        "Autohand Code CLI",
        ".autohand/skills",
        GlobalRoot::Env {
            name: "AUTOHAND_HOME",
            fallback: ".autohand"
        }
    ),
    target!(
        "augment",
        "Augment",
        ".augment/skills",
        GlobalRoot::Home(".augment/skills")
    ),
    target!(
        "bob",
        "IBM Bob",
        ".bob/skills",
        GlobalRoot::Home(".bob/skills")
    ),
    target!(
        "claude-code",
        "Claude Code",
        ".claude/skills",
        GlobalRoot::Env {
            name: "CLAUDE_CONFIG_DIR",
            fallback: ".claude"
        }
    ),
    target!("openclaw", "OpenClaw", "skills", GlobalRoot::OpenClaw),
    target!(
        "cline",
        "Cline",
        ".agents/skills",
        GlobalRoot::Home(".agents/skills")
    ),
    target!(
        "codearts-agent",
        "CodeArts Agent",
        ".codeartsdoer/skills",
        GlobalRoot::Home(".codeartsdoer/skills")
    ),
    target!(
        "codebuddy",
        "CodeBuddy",
        ".codebuddy/skills",
        GlobalRoot::Home(".codebuddy/skills")
    ),
    target!(
        "codemaker",
        "Codemaker",
        ".codemaker/skills",
        GlobalRoot::Home(".codemaker/skills")
    ),
    target!(
        "codestudio",
        "Code Studio",
        ".codestudio/skills",
        GlobalRoot::Home(".codestudio/skills")
    ),
    target!(
        "codex",
        "Codex",
        ".agents/skills",
        GlobalRoot::Env {
            name: "CODEX_HOME",
            fallback: ".codex"
        }
    ),
    target!(
        "command-code",
        "Command Code",
        ".commandcode/skills",
        GlobalRoot::Home(".commandcode/skills")
    ),
    target!(
        "continue",
        "Continue",
        ".continue/skills",
        GlobalRoot::Home(".continue/skills")
    ),
    target!(
        "cortex",
        "Cortex Code",
        ".cortex/skills",
        GlobalRoot::Home(".snowflake/cortex/skills")
    ),
    target!(
        "crush",
        "Crush",
        ".crush/skills",
        GlobalRoot::Home(".config/crush/skills")
    ),
    target!(
        "cursor",
        "Cursor",
        ".agents/skills",
        GlobalRoot::Home(".cursor/skills")
    ),
    target!(
        "deepagents",
        "Deep Agents",
        ".agents/skills",
        GlobalRoot::Home(".deepagents/agent/skills")
    ),
    target!(
        "devin",
        "Devin for Terminal",
        ".devin/skills",
        GlobalRoot::Config("devin/skills")
    ),
    target!(
        "dexto",
        "Dexto",
        ".agents/skills",
        GlobalRoot::Home(".agents/skills")
    ),
    target!(
        "droid",
        "Droid",
        ".factory/skills",
        GlobalRoot::Home(".factory/skills")
    ),
    target!("eve", "Eve", "agent/skills", GlobalRoot::Unsupported),
    target!(
        "firebender",
        "Firebender",
        ".agents/skills",
        GlobalRoot::Home(".firebender/skills")
    ),
    target!(
        "forgecode",
        "ForgeCode",
        ".forge/skills",
        GlobalRoot::Home(".forge/skills")
    ),
    target!(
        "gemini-cli",
        "Gemini CLI",
        ".agents/skills",
        GlobalRoot::Home(".gemini/skills")
    ),
    target!(
        "github-copilot",
        "GitHub Copilot",
        ".agents/skills",
        GlobalRoot::Home(".copilot/skills")
    ),
    target!(
        "goose",
        "Goose",
        ".goose/skills",
        GlobalRoot::Config("goose/skills")
    ),
    target!(
        "grok",
        "Grok Build",
        ".grok/skills",
        GlobalRoot::Env {
            name: "GROK_HOME",
            fallback: ".grok"
        }
    ),
    target!(
        "hermes-agent",
        "Hermes Agent",
        ".hermes/skills",
        GlobalRoot::Env {
            name: "HERMES_HOME",
            fallback: ".hermes"
        }
    ),
    target!(
        "inference-sh",
        "inference.sh",
        ".inferencesh/skills",
        GlobalRoot::Home(".inferencesh/skills")
    ),
    target!(
        "jazz",
        "Jazz",
        ".jazz/skills",
        GlobalRoot::Home(".jazz/skills")
    ),
    target!(
        "junie",
        "Junie",
        ".junie/skills",
        GlobalRoot::Home(".junie/skills")
    ),
    target!(
        "iflow-cli",
        "iFlow CLI",
        ".iflow/skills",
        GlobalRoot::Home(".iflow/skills")
    ),
    target!(
        "kilo",
        "Kilo Code",
        ".kilocode/skills",
        GlobalRoot::Home(".kilocode/skills")
    ),
    target!(
        "kimchi",
        "Kimchi",
        ".kimchi/skills",
        GlobalRoot::Home(".config/kimchi/harness/skills")
    ),
    target!(
        "kimi-code-cli",
        "Kimi Code CLI",
        ".agents/skills",
        GlobalRoot::Home(".agents/skills")
    ),
    target!(
        "kiro-cli",
        "Kiro CLI",
        ".kiro/skills",
        GlobalRoot::Home(".kiro/skills")
    ),
    target!(
        "kode",
        "Kode",
        ".kode/skills",
        GlobalRoot::Home(".kode/skills")
    ),
    target!(
        "lingma",
        "Lingma",
        ".lingma/skills",
        GlobalRoot::Home(".lingma/skills")
    ),
    target!(
        "loaf",
        "Loaf",
        ".agents/skills",
        GlobalRoot::Home(".agents/skills")
    ),
    target!(
        "mcpjam",
        "MCPJam",
        ".mcpjam/skills",
        GlobalRoot::Home(".mcpjam/skills")
    ),
    target!(
        "minimax-code",
        "MiniMax Code",
        ".minimax/skills",
        GlobalRoot::Home(".minimax/skills")
    ),
    target!(
        "mistral-vibe",
        "Mistral Vibe",
        ".vibe/skills",
        GlobalRoot::Env {
            name: "VIBE_HOME",
            fallback: ".vibe"
        }
    ),
    target!(
        "moxby",
        "Moxby",
        ".moxby/skills",
        GlobalRoot::Home(".moxby/skills")
    ),
    target!("mux", "Mux", ".mux/skills", GlobalRoot::Home(".mux/skills")),
    target!(
        "opencode",
        "OpenCode",
        ".agents/skills",
        GlobalRoot::Config("opencode/skills")
    ),
    target!(
        "openhands",
        "OpenHands",
        ".openhands/skills",
        GlobalRoot::Home(".openhands/skills")
    ),
    target!("ona", "Ona", ".ona/skills", GlobalRoot::Home(".ona/skills")),
    target!(
        "pi",
        "Pi",
        ".pi/skills",
        GlobalRoot::Home(".pi/agent/skills")
    ),
    target!(
        "qoder",
        "Qoder",
        ".qoder/skills",
        GlobalRoot::Home(".qoder/skills")
    ),
    target!(
        "qoder-cn",
        "Qoder CN",
        ".qoder/skills",
        GlobalRoot::Home(".qoder-cn/skills")
    ),
    target!(
        "qwen-code",
        "Qwen Code",
        ".qwen/skills",
        GlobalRoot::Home(".qwen/skills")
    ),
    target!(
        "replit",
        "Replit",
        ".agents/skills",
        GlobalRoot::Config("agents/skills")
    ),
    target!(
        "reasonix",
        "Reasonix",
        ".reasonix/skills",
        GlobalRoot::Home(".reasonix/skills")
    ),
    target!(
        "rovodev",
        "Rovo Dev",
        ".rovodev/skills",
        GlobalRoot::Home(".rovodev/skills")
    ),
    target!(
        "roo",
        "Roo Code",
        ".roo/skills",
        GlobalRoot::Home(".roo/skills")
    ),
    target!(
        "tabnine-cli",
        "Tabnine CLI",
        ".tabnine/agent/skills",
        GlobalRoot::Home(".tabnine/agent/skills")
    ),
    target!(
        "terramind",
        "Terramind",
        ".terramind/skills",
        GlobalRoot::Home(".terramind/skills")
    ),
    target!(
        "tinycloud",
        "Tinycloud",
        ".tinycloud/skills",
        GlobalRoot::Home(".tinycloud/skills")
    ),
    target!(
        "trae",
        "Trae",
        ".trae/skills",
        GlobalRoot::Home(".trae/skills")
    ),
    target!(
        "trae-cn",
        "Trae CN",
        ".trae/skills",
        GlobalRoot::Home(".trae-cn/skills")
    ),
    target!(
        "warp",
        "Warp",
        ".agents/skills",
        GlobalRoot::Home(".agents/skills")
    ),
    target!(
        "windsurf",
        "Windsurf",
        ".windsurf/skills",
        GlobalRoot::Home(".codeium/windsurf/skills")
    ),
    target!(
        "zed",
        "Zed",
        ".agents/skills",
        GlobalRoot::Home(".agents/skills")
    ),
    target!(
        "zcode",
        "ZCode",
        ".zcode/skills",
        GlobalRoot::Home(".zcode/skills")
    ),
    target!(
        "zencoder",
        "Zencoder",
        ".zencoder/skills",
        GlobalRoot::Home(".zencoder/skills")
    ),
    target!(
        "zenflow",
        "Zenflow",
        ".zencoder/skills",
        GlobalRoot::Home(".zencoder/skills")
    ),
    target!(
        "neovate",
        "Neovate",
        ".neovate/skills",
        GlobalRoot::Home(".neovate/skills")
    ),
    target!(
        "pochi",
        "Pochi",
        ".pochi/skills",
        GlobalRoot::Home(".pochi/skills")
    ),
    target!(
        "promptscript",
        "PromptScript",
        ".agents/skills",
        GlobalRoot::Unsupported
    ),
    target!(
        "adal",
        "AdaL",
        ".adal/skills",
        GlobalRoot::Home(".adal/skills")
    ),
    target!(
        "universal",
        "Universal",
        ".agents/skills",
        GlobalRoot::Config("agents/skills")
    ),
];

static LEGACY_SHARED: SkillTargetConfig = target!(
    "shared",
    "Shared",
    ".agents/skills",
    GlobalRoot::Home(".agents/skills")
);

pub fn target_catalog() -> &'static [SkillTargetConfig] {
    TARGETS
}

pub fn target_config(target: &SkillTarget) -> Result<&'static SkillTargetConfig> {
    let canonical = match target.id() {
        "shared" => return Ok(&LEGACY_SHARED),
        "claude" => "claude-code",
        value => value,
    };
    TARGETS
        .iter()
        .find(|item| item.id == canonical)
        .with_context(|| format!("unknown skill target `{}`", target.id()))
}

pub fn skill_target_root(
    cwd: &Path,
    target: &SkillTarget,
    scope: SkillInstallScope,
) -> Result<PathBuf> {
    let config = target_config(target)?;
    if scope == SkillInstallScope::Project {
        return Ok(cwd.join(config.project_skills_dir));
    }

    let home = dirs::home_dir().context("could not resolve home directory")?;
    match config.global {
        GlobalRoot::Home(path) => Ok(home.join(path)),
        GlobalRoot::Config(path) => Ok(xdg_config_home(&home).join(path)),
        GlobalRoot::Env { name, fallback } => {
            let base = env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(fallback));
            Ok(base.join("skills"))
        }
        GlobalRoot::OpenClaw => {
            for directory in [".openclaw", ".clawdbot", ".moltbot"] {
                let root = home.join(directory);
                if root.exists() {
                    return Ok(root.join("skills"));
                }
            }
            Ok(home.join(".openclaw/skills"))
        }
        GlobalRoot::Unsupported => bail!(
            "skill target `{}` does not support global installation; use --scope project",
            target.id()
        ),
    }
}

fn xdg_config_home(home: &Path) -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_skills_cli_1_5_22_catalog_size_and_key_paths() {
        assert_eq!(target_catalog().len(), 76);
        assert_eq!(
            target_config(&"opencode".parse().unwrap())
                .unwrap()
                .project_skills_dir,
            ".agents/skills"
        );
        assert_eq!(
            target_config(&"windsurf".parse().unwrap())
                .unwrap()
                .project_skills_dir,
            ".windsurf/skills"
        );
        assert!(
            !target_config(&"eve".parse().unwrap())
                .unwrap()
                .supports_global()
        );
        assert!(
            !target_config(&"promptscript".parse().unwrap())
                .unwrap()
                .supports_global()
        );
    }

    #[test]
    fn legacy_names_keep_their_serialized_values() {
        for (kind, expected) in [
            (AgentKind::Shared, "shared"),
            (AgentKind::Codex, "codex"),
            (AgentKind::Cursor, "cursor"),
            (AgentKind::Claude, "claude"),
        ] {
            let target = SkillTarget::from(kind);
            assert_eq!(
                serde_json::to_string(&target).unwrap(),
                format!("\"{expected}\"")
            );
            assert!(target_config(&target).is_ok());
        }
    }

    #[test]
    fn project_roots_are_expressed_from_the_cwd() {
        let cwd = Path::new("/tmp/project");
        assert_eq!(
            skill_target_root(cwd, &"eve".parse().unwrap(), SkillInstallScope::Project).unwrap(),
            cwd.join("agent/skills")
        );
        assert_eq!(
            skill_target_root(
                cwd,
                &"claude-code".parse().unwrap(),
                SkillInstallScope::Project
            )
            .unwrap(),
            cwd.join(".claude/skills")
        );
    }
}
