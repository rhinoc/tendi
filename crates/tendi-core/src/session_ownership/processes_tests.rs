use super::*;

#[test]
fn reports_process_start_and_writable_handles() {
    let pid = std::process::id() as i32;
    assert!(start(pid).is_some());
    assert!(start(i32::MAX).is_none());
    let path = std::env::temp_dir().join(format!("tendi-owned-fd-{}", std::process::id()));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .read(true)
        .open(&path)
        .unwrap();
    let path = path.canonicalize().unwrap();
    assert!(writable_files(pid).contains(&path));
    drop(file);
    assert!(!writable_files(pid).contains(&path));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn receives_exit_for_an_unrelated_child_process() {
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let mut monitor = ProcessMonitor::default();
    monitor.update(std::collections::BTreeSet::from([child.id() as i32]));
    assert!(!monitor.changed());
    child.kill().unwrap();
    child.wait().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !monitor.changed() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
