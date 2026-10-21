use std::path::PathBuf;

#[derive(Debug)]
pub(crate) struct Process {
    pub pid: i32,
    pub executable: PathBuf,
}

#[derive(Default)]
pub(crate) struct Processes {
    entries: Option<Vec<Process>>,
}

impl Processes {
    pub fn entries(&mut self) -> &[Process] {
        self.entries.get_or_insert_with(list)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::ffi::{CStr, c_char, c_void};
    use std::os::unix::ffi::OsStrExt;

    unsafe extern "C" {
        fn tendi_process_start(pid: i32) -> u64;
        fn tendi_process_ids(ids: *mut i32, bytes: i32) -> i32;
        fn tendi_process_path(pid: i32, path: *mut c_char, bytes: i32) -> i32;
        fn tendi_process_writable_files(
            pid: i32,
            visit: unsafe extern "C" fn(*const c_char, *mut c_void),
            context: *mut c_void,
        );
        fn tendi_process_monitor_create() -> i32;
        fn tendi_process_monitor_watch(queue: i32, pid: i32, enabled: i32);
        fn tendi_process_monitor_changed(queue: i32) -> i32;
        fn tendi_process_monitor_close(queue: i32);
    }

    pub fn start(pid: i32) -> Option<u64> {
        let stamp = unsafe { tendi_process_start(pid) };
        (stamp > 0).then_some(stamp)
    }

    pub fn list() -> Vec<Process> {
        let count = unsafe { tendi_process_ids(std::ptr::null_mut(), 0) };
        if count <= 0 {
            return Vec::new();
        }
        let mut ids = vec![0; count as usize + 128];
        let count =
            unsafe { tendi_process_ids(ids.as_mut_ptr(), (ids.len() * size_of::<i32>()) as i32) };
        ids.truncate(count.max(0) as usize);
        ids.into_iter()
            .filter(|pid| *pid > 0 && *pid != std::process::id() as i32)
            .filter_map(|pid| {
                let mut path = [0 as c_char; 4096];
                let len = unsafe { tendi_process_path(pid, path.as_mut_ptr(), path.len() as i32) };
                if len <= 0 {
                    return None;
                }
                let bytes = unsafe { CStr::from_ptr(path.as_ptr()) }.to_bytes();
                Some(Process {
                    pid,
                    executable: std::ffi::OsStr::from_bytes(bytes).into(),
                })
            })
            .collect()
    }

    pub fn writable_files(pid: i32) -> Vec<PathBuf> {
        unsafe extern "C" fn visit(path: *const c_char, context: *mut c_void) {
            let paths = unsafe { &mut *(context as *mut Vec<PathBuf>) };
            let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
            paths.push(std::ffi::OsStr::from_bytes(bytes).into());
        }
        let mut paths = Vec::new();
        unsafe {
            tendi_process_writable_files(pid, visit, (&mut paths as *mut Vec<PathBuf>).cast())
        };
        paths
    }

    pub struct ProcessMonitor {
        queue: i32,
        pids: std::collections::BTreeSet<i32>,
    }

    impl Default for ProcessMonitor {
        fn default() -> Self {
            Self {
                queue: unsafe { tendi_process_monitor_create() },
                pids: Default::default(),
            }
        }
    }

    impl ProcessMonitor {
        pub fn update(&mut self, pids: std::collections::BTreeSet<i32>) {
            if self.queue < 0 {
                return;
            }
            for pid in self.pids.difference(&pids) {
                unsafe { tendi_process_monitor_watch(self.queue, *pid, 0) };
            }
            for pid in pids.difference(&self.pids) {
                unsafe { tendi_process_monitor_watch(self.queue, *pid, 1) };
            }
            self.pids = pids;
        }
        pub fn changed(&mut self) -> bool {
            self.queue >= 0 && unsafe { tendi_process_monitor_changed(self.queue) } > 0
        }
    }

    impl Drop for ProcessMonitor {
        fn drop(&mut self) {
            if self.queue >= 0 {
                unsafe { tendi_process_monitor_close(self.queue) };
            }
        }
    }

    #[cfg(test)]
    mod tests {
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
    }
}

#[cfg(target_os = "macos")]
pub use macos::ProcessMonitor;
#[cfg(target_os = "macos")]
pub(crate) use macos::{list, start, writable_files};

#[cfg(not(target_os = "macos"))]
#[derive(Default)]
pub struct ProcessMonitor;
#[cfg(not(target_os = "macos"))]
impl ProcessMonitor {
    pub fn update(&mut self, _pids: std::collections::BTreeSet<i32>) {}
    pub fn changed(&mut self) -> bool {
        false
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn list() -> Vec<Process> {
    Vec::new()
}
#[cfg(not(target_os = "macos"))]
pub(crate) fn start(_pid: i32) -> Option<u64> {
    None
}
#[cfg(not(target_os = "macos"))]
pub(crate) fn writable_files(_pid: i32) -> Vec<PathBuf> {
    Vec::new()
}
