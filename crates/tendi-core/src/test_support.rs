use std::{
    env, fs,
    path::PathBuf,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

/// Keep core tests away from the user's provider roots, lock files, config, and logs.
///
/// Tests run in one process and the platform home directory is process-global. Initialize the
/// isolated paths once, before any test-owned scan or migration can resolve a provider root.
pub(crate) fn ensure_isolated_environment() {
    static TEST_HOME: OnceLock<PathBuf> = OnceLock::new();

    TEST_HOME.get_or_init(|| {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos();
        let raw_home = env::temp_dir().join(format!(
            "tendi-core-test-home-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&raw_home).expect("create isolated test home");
        let home = fs::canonicalize(&raw_home).expect("canonicalize isolated test home");
        let state = home.join(".state");
        let config = home.join(".config");
        let cache = home.join(".cache");
        let log_dir = home.join("logs");

        for directory in [&state, &config, &cache, &log_dir] {
            fs::create_dir_all(directory).expect("create isolated test directory");
        }

        unsafe {
            env::set_var("HOME", &home);
            env::set_var("XDG_STATE_HOME", &state);
            env::set_var("XDG_CONFIG_HOME", &config);
            env::set_var("XDG_CACHE_HOME", &cache);
            env::remove_var("CODEX_HOME");
            env::remove_var("AUTOHAND_HOME");
            env::remove_var("GROK_HOME");
            env::remove_var("HERMES_HOME");
            env::remove_var("VIBE_HOME");
            env::set_var("TENDI_LOG_DIR", &log_dir);
            env::remove_var("TENDI_LOG_PATH");
        }

        home
    });
}
