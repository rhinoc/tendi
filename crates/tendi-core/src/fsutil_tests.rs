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
