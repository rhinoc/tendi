use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::{FileChange, SkillVisibility};

pub(crate) const RELATIVE_PATH: &str = "agents/tendi.yaml";
const CURRENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TendiSkillMetadataFile {
    #[serde(default = "current_schema_version")]
    schema_version: u32,
    visibility: SkillVisibility,
}

fn current_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

pub(crate) fn path(skill_dir: &Path) -> PathBuf {
    skill_dir.join(RELATIVE_PATH)
}

pub(crate) fn read_visibility(skill_dir: &Path) -> Result<Option<SkillVisibility>> {
    let metadata_path = path(skill_dir);
    let Some(text) = fs::read_to_string(&metadata_path).ok() else {
        return Ok(None);
    };
    let metadata = serde_yaml::from_str::<TendiSkillMetadataFile>(&text)
        .with_context(|| format!("failed to parse {}", metadata_path.display()))?;
    if metadata.schema_version != CURRENT_SCHEMA_VERSION {
        bail!(
            "unsupported Tendi skill metadata schema version {} in {}",
            metadata.schema_version,
            metadata_path.display()
        );
    }
    if metadata.visibility == SkillVisibility::Mixed {
        bail!(
            "mixed visibility cannot be persisted in {}",
            metadata_path.display()
        );
    }
    Ok(Some(metadata.visibility))
}

pub(crate) fn render_visibility(visibility: SkillVisibility) -> Result<String> {
    if visibility == SkillVisibility::Mixed {
        bail!("mixed visibility cannot be persisted in Tendi skill metadata");
    }
    Ok(serde_yaml::to_string(&TendiSkillMetadataFile {
        schema_version: CURRENT_SCHEMA_VERSION,
        visibility,
    })?)
}

pub(crate) fn plan_visibility(skill_dir: &Path, visibility: SkillVisibility) -> Result<FileChange> {
    let metadata_path = path(skill_dir);
    let before = fs::read_to_string(&metadata_path).ok();
    let after = render_visibility(visibility)?;
    Ok(FileChange {
        path: metadata_path,
        before_sha256: before.as_deref().map(crate::fsutil::sha256_text),
        before,
        after,
    })
}
