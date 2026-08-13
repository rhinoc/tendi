use std::{fs, io::Write, path::Path};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

pub fn sha256_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(sha256_text(&text))
}

pub fn atomic_write(path: &Path, text: &str) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;

    let tmp_path = parent.join(format!(
        ".{}.tendi-tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("write"),
        std::process::id()
    ));

    {
        let mut file = fs::File::create(&tmp_path)
            .with_context(|| format!("failed to create {}", tmp_path.display()))?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }

    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to rename {} to {}",
            tmp_path.display(),
            path.display()
        )
    })?;
    fs::File::open(parent)
        .with_context(|| format!("failed to open {}", parent.display()))?
        .sync_all()
        .with_context(|| format!("failed to sync {}", parent.display()))?;
    Ok(())
}
