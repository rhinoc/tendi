use std::{
    fs,
    io::Write,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

static ATOMIC_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn sha256_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn atomic_write(path: &Path, text: &str) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;

    let sequence = ATOMIC_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let tmp_path = parent.join(format!(
        ".{}.tendi-tmp-{}-{sequence}",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_file_accepts_binary_content() {
        let path = std::env::temp_dir().join(format!(
            "tendi-fsutil-binary-hash-{}.bin",
            std::process::id()
        ));
        let bytes = [0_u8, 0xff, 0x00, 0x80, 0x7f];
        fs::write(&path, bytes).unwrap();

        let actual = sha256_file(&path).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        assert_eq!(actual, format!("{:x}", hasher.finalize()));

        let _ = fs::remove_file(path);
    }
}
