#[path = "cli_install.rs"]
mod cli_registration;

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::{ActivationPolicy, Emitter, Manager, RunEvent};
#[cfg(target_os = "macos")]
use tauri::{LogicalPosition, TitleBarStyle};
use tauri_plugin_updater::UpdaterExt;
use tendi_core::AgentKind;

static SESSION_SKILL_INDEX_RUNNING: AtomicBool = AtomicBool::new(false);
static SKILL_UPDATE_CHECK_RUNNING: AtomicBool = AtomicBool::new(false);
static SKILL_UPDATE_CACHE: LazyLock<Mutex<BTreeMap<PathBuf, SkillUpdateCache>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
static SESSION_ANALYTICS_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static SKILL_DELETE_PREVIEW_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static SKILL_DELETE_PREVIEW: LazyLock<Mutex<Option<SkillDeletePreview>>> =
    LazyLock::new(|| Mutex::new(None));
static SKILL_UPDATE_PREVIEW_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static SKILL_UPDATE_PREVIEW: LazyLock<Mutex<Option<SkillUpdatePreview>>> =
    LazyLock::new(|| Mutex::new(None));
static SKILL_ADD_PREVIEW_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static SKILL_ADD_PREVIEW: LazyLock<Mutex<Option<SkillAddPreview>>> =
    LazyLock::new(|| Mutex::new(None));
static SKILL_DIR_CACHE: LazyLock<Mutex<BTreeMap<(PathBuf, String), PathBuf>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
static SKILL_SCAN_CACHE: LazyLock<Mutex<BTreeMap<PathBuf, tendi_core::skills::SkillScan>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
static SKILL_AUTHORITY_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static SKILL_AUTHORITY_REVISION: AtomicU64 = AtomicU64::new(0);
static HOOK_MUTATION_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static RULE_PATH_CACHE: LazyLock<Mutex<BTreeMap<PathBuf, BTreeSet<PathBuf>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
static HOOK_PATH_CACHE: LazyLock<Mutex<BTreeMap<PathBuf, BTreeSet<PathBuf>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

const SESSION_SCAN_EVENT: &str = "sessions://scan";
const ANALYTICS_PROGRESS_EVENT: &str = "analytics://progress";
const ANALYTICS_REVISION_EVENT: &str = "analytics://revision";
const SKILL_UPDATE_EVENT: &str = "skills://updates";
const SKILL_UPDATE_CACHE_TTL: Duration = Duration::from_secs(60);
const SESSION_SCAN_BATCH_SIZE: usize = 32;
const SESSION_WATCH_DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCheckResult {
    status: &'static str,
    version: Option<String>,
}

#[tauri::command]
fn bundled_skill_status() -> Result<tendi_core::bundled_skill::BundledSkillStatus, String> {
    tendi_core::bundled_skill::status(AgentKind::Shared).map_err(|error| error.to_string())
}

#[tauri::command]
fn bundled_skill_install() -> Result<tendi_core::bundled_skill::BundledSkillInstallReport, String> {
    tendi_core::bundled_skill::install(AgentKind::Shared, false, false)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn bundled_skill_prompt_dismiss() -> Result<(), String> {
    tendi_core::bundled_skill::dismiss_prompt().map_err(|error| error.to_string())
}

#[tauri::command]
async fn cli_status() -> Result<cli_registration::CliInstallStatus, String> {
    tauri::async_runtime::spawn_blocking(cli_registration::status)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn cli_install() -> Result<cli_registration::CliInstallStatus, String> {
    tauri::async_runtime::spawn_blocking(cli_registration::install)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn cli_remove() -> Result<cli_registration::CliInstallStatus, String> {
    tauri::async_runtime::spawn_blocking(cli_registration::remove)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn check_for_updates(app: tauri::AppHandle) -> Result<UpdateCheckResult, String> {
    let updater = app.updater().map_err(|error| error.to_string())?;
    let Some(update) = updater.check().await.map_err(|error| error.to_string())? else {
        return Ok(UpdateCheckResult {
            status: "up-to-date",
            version: None,
        });
    };

    let version = update.version.clone();
    let mut downloaded = 0_u64;
    update
        .download_and_install(
            |chunk_length, content_length| {
                downloaded += chunk_length as u64;
                eprintln!(
                    "tendi update download: {downloaded} bytes of {:?}",
                    content_length
                );
            },
            || eprintln!("tendi update download finished"),
        )
        .await
        .map_err(|error| error.to_string())?;

    eprintln!("tendi update installed: {version}");
    app.restart();
}

struct SessionScanRuntime {
    generation: Arc<AtomicU64>,
    scan_running: Arc<AtomicBool>,
    watcher: Arc<Mutex<SessionWatcherState>>,
    watch_tx: Sender<notify::Result<Event>>,
    analytics_tx: Sender<AnalyticsRefreshJob>,
}

#[derive(Debug, Clone)]
struct AnalyticsRefreshJob {
    phase: &'static str,
    sessions: Vec<tendi_core::SessionRecord>,
}

#[derive(Default)]
struct SessionWatcherState {
    watcher: Option<RecommendedWatcher>,
    watched_paths: BTreeSet<PathBuf>,
    tutti_run_roots: Vec<PathBuf>,
}

impl SessionScanRuntime {
    fn new(app: tauri::AppHandle) -> Self {
        let (watch_tx, watch_rx) = mpsc::channel();
        let (analytics_tx, analytics_rx) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let scan_running = Arc::new(AtomicBool::new(false));
        let watcher = Arc::new(Mutex::new(SessionWatcherState::default()));
        let worker_generation = Arc::clone(&generation);
        let worker_scan_running = Arc::clone(&scan_running);
        let worker_watcher = Arc::clone(&watcher);
        let watch_analytics_tx = analytics_tx.clone();
        let watch_app = app.clone();
        std::thread::spawn(move || {
            session_watch_loop(
                watch_app,
                watch_rx,
                worker_generation,
                worker_scan_running,
                worker_watcher,
                watch_analytics_tx,
            )
        });
        std::thread::spawn(move || session_analytics_loop(app, analytics_rx));
        Self {
            generation,
            scan_running,
            watcher,
            watch_tx,
            analytics_tx,
        }
    }

    fn configure_watcher(
        &self,
        plan: &tendi_core::sessions::SessionWatchPlan,
    ) -> Result<(), String> {
        let tx = self.watch_tx.clone();
        let mut watcher = notify::recommended_watcher(tx).map_err(|err| err.to_string())?;
        let mut watched_paths = BTreeSet::new();
        for target in plan.targets.iter().filter(|target| target.path.exists()) {
            let mode = if target.recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            watcher
                .watch(&target.path, mode)
                .map_err(|err| format!("failed to watch {}: {err}", target.path.display()))?;
            watched_paths.insert(target.path.clone());
        }
        *self
            .watcher
            .lock()
            .map_err(|_| "session watcher is unavailable".to_string())? = SessionWatcherState {
            watcher: Some(watcher),
            watched_paths,
            tutti_run_roots: plan.tutti_run_roots.clone(),
        };
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionScanEvent {
    generation: u64,
    phase: &'static str,
    upserts: Vec<tendi_core::SessionRecord>,
    deleted: Vec<tendi_core::sessions::SessionIdentity>,
    scanned: usize,
    complete: bool,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalyticsProgressEvent {
    phase: &'static str,
    #[serde(flatten)]
    progress: tendi_core::analytics::AnalyticsRefreshProgress,
    running: bool,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalyticsRevisionEvent {
    revision: u64,
}

fn emit_analytics_revision(app: &tauri::AppHandle, revision: u64) {
    let _ = app.emit(
        ANALYTICS_REVISION_EVENT,
        AnalyticsRevisionEvent { revision },
    );
}

fn emit_analytics_progress(
    app: &tauri::AppHandle,
    phase: &'static str,
    progress: tendi_core::analytics::AnalyticsRefreshProgress,
    error: Option<String>,
) {
    let _ = app.emit(
        ANALYTICS_PROGRESS_EVENT,
        AnalyticsProgressEvent {
            phase,
            running: error.is_none() && progress.completed < progress.total,
            progress,
            error,
        },
    );
}

fn refresh_session_analytics_serialized(
    app: &tauri::AppHandle,
    phase: &'static str,
    store: &tendi_core::storage::Store,
    sessions: &[tendi_core::SessionRecord],
) -> Result<tendi_core::analytics::AnalyticsRefreshReport, String> {
    let initial = tendi_core::analytics::AnalyticsRefreshProgress {
        total: sessions.len(),
        ..Default::default()
    };
    let _guard = match SESSION_ANALYTICS_LOCK.lock() {
        Ok(guard) => guard,
        Err(_) => {
            let message = "session analytics worker is unavailable".to_string();
            emit_analytics_progress(app, phase, initial, Some(message.clone()));
            return Err(message);
        }
    };
    emit_analytics_progress(app, phase, initial, None);
    let mut last_progress = initial;
    match store.refresh_session_analytics_with_progress(sessions, |progress| {
        last_progress = progress;
        emit_analytics_progress(app, phase, progress, None);
    }) {
        Ok(report) => Ok(report),
        Err(error) => {
            let message = format!("{error:#}");
            emit_analytics_progress(app, phase, last_progress, Some(message.clone()));
            Err(message)
        }
    }
}

fn session_analytics_loop(app: tauri::AppHandle, receiver: Receiver<AnalyticsRefreshJob>) {
    let store = match tendi_core::storage::Store::open_default() {
        Ok(store) => store,
        Err(error) => {
            emit_analytics_progress(
                &app,
                "backfill",
                tendi_core::analytics::AnalyticsRefreshProgress::default(),
                Some(format!("{error:#}")),
            );
            return;
        }
    };
    let mut legacy_backfill_complete = false;
    let mut last_backfill_revision_emit = Instant::now();
    loop {
        let received = if legacy_backfill_complete {
            match receiver.recv() {
                Ok(job) => Some(job),
                Err(_) => break,
            }
        } else {
            match receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        };
        let mut phase = "backfill";
        let mut refresh_requested = false;
        let mut pending = BTreeMap::<String, tendi_core::SessionRecord>::new();
        for job in received.into_iter().chain(receiver.try_iter()) {
            phase = job.phase;
            refresh_requested = true;
            for session in job.sessions {
                let key = format!(
                    "{:?}\0{}\0{}",
                    session.agent,
                    session.id,
                    session.path.display()
                );
                pending.insert(key, session);
            }
        }

        if refresh_requested {
            let sessions = pending.into_values().collect::<Vec<_>>();
            let _ = refresh_session_analytics_serialized(&app, phase, &store, &sessions);
            if let Ok(revision) = store.analytics_revision() {
                emit_analytics_revision(&app, revision);
            }
        }
        if !legacy_backfill_complete {
            match store.backfill_session_analytics_overview_index_batch(32) {
                Ok(report) => {
                    legacy_backfill_complete = report.remaining == 0;
                    if report.processed > 0
                        && (legacy_backfill_complete
                            || last_backfill_revision_emit.elapsed() >= Duration::from_millis(500))
                    {
                        emit_analytics_revision(&app, report.revision);
                        last_backfill_revision_emit = Instant::now();
                    }
                }
                Err(error) => {
                    emit_analytics_progress(
                        &app,
                        "backfill",
                        tendi_core::analytics::AnalyticsRefreshProgress::default(),
                        Some(format!("{error:#}")),
                    );
                }
            }
        }
    }
}

struct SkillDeletePreview {
    id: String,
    names: BTreeSet<String>,
    plan: tendi_core::skills::SkillDeletePlan,
}

struct SkillUpdatePreview {
    id: String,
    names: BTreeSet<String>,
    plan: tendi_core::skills::SkillUpdatePlan,
}

struct SkillAddPreview {
    id: String,
    source: String,
    target: tendi_core::SkillTarget,
    scope: tendi_core::SkillInstallScope,
    copy: bool,
    visibility: tendi_core::SkillVisibility,
    fingerprint: String,
    plan: tendi_core::skills::SkillAddPlan,
}

fn store_skill_add_preview(
    options: &tendi_core::skills::SkillAddOptions,
    plan: tendi_core::skills::SkillAddPlan,
) -> Result<String, String> {
    let fingerprint = tendi_core::skills::skill_add_catalog_fingerprint(&plan)
        .map_err(|err| format!("{err:#}"))?;
    let id = format!(
        "add-{}",
        SKILL_ADD_PREVIEW_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let mut preview = SKILL_ADD_PREVIEW
        .lock()
        .map_err(|_| "skill add preview store is unavailable".to_string())?;
    *preview = Some(SkillAddPreview {
        id: id.clone(),
        source: options.source.clone(),
        target: options.target.clone(),
        scope: options.scope,
        copy: options.copy,
        visibility: options.visibility,
        fingerprint,
        plan,
    });
    Ok(id)
}

fn take_skill_add_preview(
    id: &str,
    options: &tendi_core::skills::SkillAddOptions,
) -> Result<tendi_core::skills::SkillAddPlan, String> {
    let mut stored = SKILL_ADD_PREVIEW
        .lock()
        .map_err(|_| "skill add preview store is unavailable".to_string())?;
    let preview = stored
        .as_ref()
        .ok_or_else(|| "skill add preview expired; preview the installation again".to_string())?;
    if preview.id != id
        || preview.source != options.source
        || preview.target != options.target
        || preview.scope != options.scope
        || preview.copy != options.copy
        || preview.visibility != options.visibility
    {
        return Err("skill add options changed; preview the installation again".to_string());
    }
    let fingerprint = tendi_core::skills::skill_add_catalog_fingerprint(&preview.plan)
        .map_err(|_| "skill source changed; preview the installation again".to_string())?;
    if fingerprint != preview.fingerprint {
        return Err("skill source changed; preview the installation again".to_string());
    }
    Ok(stored.take().expect("checked skill add preview").plan)
}

fn store_skill_delete_preview(
    names: &[String],
    plan: tendi_core::skills::SkillDeletePlan,
) -> Result<String, String> {
    let id = format!(
        "delete-{}",
        SKILL_DELETE_PREVIEW_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let mut preview = SKILL_DELETE_PREVIEW
        .lock()
        .map_err(|_| "delete preview store is unavailable".to_string())?;
    *preview = Some(SkillDeletePreview {
        id: id.clone(),
        names: names.iter().cloned().collect(),
        plan,
    });
    Ok(id)
}

fn take_skill_delete_preview(
    id: &str,
    names: &[String],
) -> Result<tendi_core::skills::SkillDeletePlan, String> {
    let mut stored = SKILL_DELETE_PREVIEW
        .lock()
        .map_err(|_| "delete preview store is unavailable".to_string())?;
    let preview = stored
        .as_ref()
        .ok_or_else(|| "delete preview expired; preview the deletion again".to_string())?;
    if preview.id != id || preview.names != names.iter().cloned().collect() {
        return Err("delete selection changed; preview the deletion again".to_string());
    }
    Ok(stored.take().expect("checked delete preview").plan)
}

fn store_skill_update_preview(
    names: &[String],
    plan: tendi_core::skills::SkillUpdatePlan,
) -> Result<String, String> {
    let id = format!(
        "update-{}",
        SKILL_UPDATE_PREVIEW_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let mut preview = SKILL_UPDATE_PREVIEW
        .lock()
        .map_err(|_| "update preview store is unavailable".to_string())?;
    *preview = Some(SkillUpdatePreview {
        id: id.clone(),
        names: names.iter().cloned().collect(),
        plan,
    });
    Ok(id)
}

fn take_skill_update_preview(
    id: &str,
    names: &[String],
) -> Result<tendi_core::skills::SkillUpdatePlan, String> {
    let mut stored = SKILL_UPDATE_PREVIEW
        .lock()
        .map_err(|_| "update preview store is unavailable".to_string())?;
    let preview = stored
        .as_ref()
        .ok_or_else(|| "update preview expired; preview the update again".to_string())?;
    if preview.id != id || preview.names != names.iter().cloned().collect() {
        return Err("update selection changed; preview the update again".to_string());
    }
    Ok(stored.take().expect("checked update preview").plan)
}

fn replace_skill_dir_cache(cwd: &Path, skills: &[tendi_core::skills::SkillRecord]) {
    let Ok(mut cache) = SKILL_DIR_CACHE.lock() else {
        return;
    };
    cache.retain(|(cached_cwd, _), _| cached_cwd != cwd);
    for skill in skills {
        if let Some(path) = skill.paths.first().map(|entry| entry.path.clone()) {
            cache.insert((cwd.to_path_buf(), skill.name.clone()), path);
        }
    }
}

fn cache_skill_scan(cwd: &Path, scan: &tendi_core::skills::SkillScan) {
    replace_skill_dir_cache(cwd, &scan.skills);
    if let Ok(mut cache) = SKILL_SCAN_CACHE.lock() {
        cache.insert(cwd.to_path_buf(), scan.clone());
        SKILL_AUTHORITY_REVISION.fetch_add(1, Ordering::AcqRel);
    }
}

fn lock_skill_authority() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    SKILL_AUTHORITY_LOCK
        .lock()
        .map_err(|_| "skill authority is unavailable".to_string())
}

fn skill_scan_snapshot(cwd: &Path) -> Result<tendi_core::skills::SkillScan, String> {
    cached_or_load_skill_scan(cwd, || {
        let scan = tendi_core::skills::scan_skills_synced(cwd).map_err(|err| format!("{err:#}"))?;
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        store.save_skills(&scan).map_err(|err| format!("{err:#}"))?;
        Ok(scan)
    })
}

fn cached_or_load_skill_scan(
    cwd: &Path,
    load: impl FnOnce() -> Result<tendi_core::skills::SkillScan, String>,
) -> Result<tendi_core::skills::SkillScan, String> {
    if let Some(scan) = SKILL_SCAN_CACHE
        .lock()
        .map_err(|_| "skill scan cache is unavailable".to_string())?
        .get(cwd)
        .cloned()
    {
        return Ok(scan);
    }
    let scan = load()?;
    cache_skill_scan(cwd, &scan);
    Ok(scan)
}

fn commit_skill_refresh(
    cwd: &Path,
    before: &tendi_core::skills::SkillScan,
    names: &[String],
    extra_skill_dirs: &[PathBuf],
) -> Result<tendi_core::skills::SkillScan, String> {
    let refreshed = tendi_core::skills::refresh_skill_scan(cwd, before, names, extra_skill_dirs)
        .map_err(|err| format!("{err:#}"))?;
    let before_json = before
        .skills
        .iter()
        .map(|skill| {
            serde_json::to_string(skill)
                .map(|json| (skill.name.as_str(), json))
                .map_err(|err| err.to_string())
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let upserts = refreshed
        .skills
        .iter()
        .map(|skill| {
            let json = serde_json::to_string(skill).map_err(|err| err.to_string())?;
            Ok((before_json.get(skill.name.as_str()) != Some(&json)).then(|| skill.clone()))
        })
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let refreshed_names = refreshed
        .skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect::<BTreeSet<_>>();
    let removed = before
        .skills
        .iter()
        .map(|skill| &skill.name)
        .filter(|name| !refreshed_names.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
    store
        .save_skill_delta(&upserts, &removed)
        .map_err(|err| format!("{err:#}"))?;
    cache_skill_scan(cwd, &refreshed);
    Ok(refreshed)
}

fn cached_skill_dir(cwd: &Path, name: &str) -> Option<PathBuf> {
    let key = (cwd.to_path_buf(), name.to_string());
    let mut cache = SKILL_DIR_CACHE.lock().ok()?;
    let path = cache.get(&key)?.clone();
    if tendi_core::skills::skill_dir_matches_name(&path, name) {
        Some(path)
    } else {
        cache.remove(&key);
        None
    }
}

fn start_skill_update_check(
    app: &tauri::AppHandle,
    cwd: PathBuf,
    scan: Option<(tendi_core::skills::SkillScan, u64)>,
    use_cache: bool,
) -> &'static str {
    if SKILL_UPDATE_CHECK_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return "already-running";
    }

    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let scan = match scan {
            Some(scan) => Ok(scan),
            None => lock_skill_authority().and_then(|_authority| {
                let scan =
                    tendi_core::skills::scan_skills(&cwd).map_err(|error| format!("{error:#}"))?;
                cache_skill_scan(&cwd, &scan);
                Ok((scan, SKILL_AUTHORITY_REVISION.load(Ordering::Acquire)))
            }),
        };
        let event = match scan {
            Ok((scan, base_revision)) => {
                let fingerprint = skill_scan_fingerprint(&scan);
                let cached = use_cache
                    .then(|| SKILL_UPDATE_CACHE.lock().ok())
                    .flatten()
                    .and_then(|cache| cache.get(&cwd).cloned())
                    .filter(|cache| {
                        cache.fingerprint == fingerprint
                            && cache.checked_at.elapsed() < SKILL_UPDATE_CACHE_TTL
                    });
                let updates = cached.map(|cache| cache.updates).unwrap_or_else(|| {
                    let updates = tendi_core::skills::check_skill_updates_for_scan(&scan);
                    if use_cache {
                        if let Ok(mut cache) = SKILL_UPDATE_CACHE.lock() {
                            cache.insert(
                                cwd.clone(),
                                SkillUpdateCache {
                                    checked_at: Instant::now(),
                                    fingerprint,
                                    updates: updates.clone(),
                                },
                            );
                        }
                    }
                    updates
                });
                skill_update_event_for_revision(
                    scan.skills,
                    updates,
                    base_revision,
                    SKILL_AUTHORITY_REVISION.load(Ordering::Acquire),
                )
            }
            Err(error) => SkillUpdateCheckEvent {
                status: "failed",
                skills: None,
                updates: Vec::new(),
                error: Some(error),
            },
        };
        let _ = app.emit(SKILL_UPDATE_EVENT, event);
        SKILL_UPDATE_CHECK_RUNNING.store(false, Ordering::Release);
    });

    "started"
}

fn skill_update_event_for_revision(
    skills: Vec<tendi_core::skills::SkillRecord>,
    updates: Vec<tendi_core::skills::SkillUpdateReport>,
    base_revision: u64,
    current_revision: u64,
) -> SkillUpdateCheckEvent {
    let current = base_revision == current_revision;
    SkillUpdateCheckEvent {
        status: "completed",
        skills: current.then_some(skills),
        updates: if current { updates } else { Vec::new() },
        error: None,
    }
}

fn skill_scan_fingerprint(scan: &tendi_core::skills::SkillScan) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for skill in &scan.skills {
        skill.name.hash(&mut hasher);
        for path in &skill.paths {
            path.path.hash(&mut hasher);
            path.source_kind.hash(&mut hasher);
            path.source.hash(&mut hasher);
            path.source_version.hash(&mut hasher);
            path.source_relative_path.hash(&mut hasher);
            path.sha256.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn replace_rule_path_cache(cwd: &Path, rules: &[tendi_core::rules::RuleRecord]) {
    let Ok(mut cache) = RULE_PATH_CACHE.lock() else {
        return;
    };
    cache.insert(
        cwd.to_path_buf(),
        rules.iter().map(|rule| rule.path.clone()).collect(),
    );
}

fn cached_rule_path(cwd: &Path, path: &Path) -> bool {
    RULE_PATH_CACHE
        .lock()
        .ok()
        .map(|cache| cache.get(cwd).is_some_and(|paths| paths.contains(path)))
        .unwrap_or(false)
}

fn replace_hook_path_cache(cwd: &Path, hooks: &[tendi_core::hooks::HookRecord]) {
    let Ok(mut cache) = HOOK_PATH_CACHE.lock() else {
        return;
    };
    cache.insert(
        cwd.to_path_buf(),
        hooks.iter().map(|hook| hook.path.clone()).collect(),
    );
}

fn cached_hook_path(cwd: &Path, path: &Path) -> bool {
    HOOK_PATH_CACHE
        .lock()
        .ok()
        .map(|cache| cache.get(cwd).is_some_and(|paths| paths.contains(path)))
        .unwrap_or(false)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionResumeRequest {
    id: String,
    agent: String,
    title: Option<String>,
    project: Option<String>,
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HookDeleteInput {
    path: String,
    expected_trust_hash: String,
    event: String,
    matcher: Option<String>,
    hook_type: Option<String>,
    command: Option<String>,
    url: Option<String>,
    prompt: Option<String>,
    filter: Option<String>,
    status_message: Option<String>,
}

impl From<HookDeleteInput> for tendi_core::hooks::HookDeleteRequest {
    fn from(input: HookDeleteInput) -> Self {
        Self {
            path: PathBuf::from(input.path),
            expected_trust_hash: input.expected_trust_hash,
            event: input.event,
            matcher: input.matcher,
            hook_type: input.hook_type,
            command: input.command,
            url: input.url,
            prompt: input.prompt,
            filter: input.filter,
            status_message: input.status_message,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalApp {
    id: &'static str,
    label: &'static str,
    available: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionResumeLaunch {
    agent: AgentKind,
    terminal: String,
    command_line: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillUpdateCheckEvent {
    status: &'static str,
    skills: Option<Vec<tendi_core::skills::SkillRecord>>,
    updates: Vec<tendi_core::skills::SkillUpdateReport>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct SkillUpdateCache {
    checked_at: Instant,
    fingerprint: u64,
    updates: Vec<tendi_core::skills::SkillUpdateReport>,
}

async fn blocking_json<T, F>(job: F) -> Result<serde_json::Value, String>
where
    T: Serialize + Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let result = tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|err| format!("background task failed: {err}"))?;
    serde_json::to_value(result?).map_err(|err| err.to_string())
}

async fn blocking_unit<F>(job: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|err| format!("background task failed: {err}"))?
}

#[tauri::command(rename_all = "camelCase")]
async fn scan() -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || tendi_core::scan_and_persist(cwd).map_err(|err| format!("{err:#}"))).await
}

#[tauri::command(rename_all = "camelCase")]
async fn agents_list() -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let report = tendi_core::agents::scan_agents(&cwd).map_err(|err| format!("{err:#}"))?;
        Ok(report.agents)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn agent_configs_list() -> Result<serde_json::Value, String> {
    blocking_json(move || {
        tendi_core::config::list_agent_configs().map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn agent_config_read(path: String) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        tendi_core::config::read_agent_config(std::path::Path::new(&path))
            .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn agent_config_save(
    path: String,
    expected_sha256: String,
    content: String,
) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        tendi_core::config::save_agent_config(
            std::path::Path::new(&path),
            &expected_sha256,
            &content,
        )
        .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn config_profile_create(
    agent: String,
    name: String,
    content: String,
) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let agent = parse_agent_result(&agent)?;
        tendi_core::config::create_config_profile(agent, &name, &content)
            .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn config_profile_set(
    agent: String,
    profile: Option<String>,
) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let agent = parse_agent_result(&agent)?;
        let agent_key = config_profile_key(agent)
            .ok_or_else(|| "config profiles are not supported for this agent".to_string())?;
        let profile = profile
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if let Some(name) = profile.as_deref() {
            tendi_core::config::validate_profile_name(name).map_err(|err| format!("{err:#}"))?;
            if !tendi_core::config::config_profile_exists(agent, name)
                .map_err(|err| format!("{err:#}"))?
            {
                return Err(format!("config profile not found: {name}"));
            }
        }
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        let mut settings = store.app_settings().map_err(|err| format!("{err:#}"))?;
        if let Some(profile) = profile {
            settings
                .config_profiles
                .insert(agent_key.to_string(), profile);
        } else {
            settings.config_profiles.remove(agent_key);
        }
        store
            .save_app_settings(settings)
            .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skills_list() -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let _authority = lock_skill_authority()?;
        let report =
            tendi_core::skills::scan_skills_synced(&cwd).map_err(|err| format!("{err:#}"))?;
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        store
            .save_skills(&report)
            .map_err(|err| format!("{err:#}"))?;
        cache_skill_scan(&cwd, &report);
        Ok(report.skills)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skills_refresh(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let _authority = lock_skill_authority()?;
        let report =
            tendi_core::skills::scan_skills_synced(&cwd).map_err(|err| format!("{err:#}"))?;
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        store
            .save_skills(&report)
            .map_err(|err| format!("{err:#}"))?;
        cache_skill_scan(&cwd, &report);
        let report_revision = SKILL_AUTHORITY_REVISION.load(Ordering::Acquire);
        let update_check =
            start_skill_update_check(&app, cwd, Some((report.clone(), report_revision)), true);
        Ok(serde_json::json!({
            "skills": report.skills,
            "updateCheck": update_check,
        }))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn sessions_list() -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        list_resolved_sessions(&store)
    })
    .await
}

fn list_resolved_sessions(
    store: &tendi_core::storage::Store,
) -> Result<Vec<tendi_core::SessionRecord>, String> {
    let mut report = store.list_sessions().map_err(|err| format!("{err:#}"))?;
    store
        .resolve_session_projects(&mut report.sessions)
        .map_err(|err| format!("{err:#}"))?;
    Ok(report.sessions)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionProjectIdentityRequest {
    id: String,
    agent: String,
    path: String,
}

#[tauri::command(rename_all = "camelCase")]
async fn sessions_project_merge(
    target_project_id: String,
    source_project_ids: Vec<String>,
) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        let project_name = store
            .merge_session_projects(&target_project_id, &source_project_ids)
            .map_err(|err| format!("{err:#}"))?;
        let merged_project_ids = std::iter::once(target_project_id.clone())
            .chain(source_project_ids)
            .collect::<BTreeSet<_>>();
        Ok(serde_json::json!({
            "kind": "merge",
            "projectId": target_project_id,
            "projectName": project_name,
            "mergedProjectIds": merged_project_ids,
        }))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn sessions_project_split(
    name: String,
    sessions: Vec<SessionProjectIdentityRequest>,
) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        let identities = sessions
            .into_iter()
            .map(|session| {
                Ok(tendi_core::sessions::SessionIdentity {
                    id: session.id,
                    agent: parse_agent_result(&session.agent)?,
                    path: PathBuf::from(session.path),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let project_id = store
            .split_sessions_into_project(&name, &identities)
            .map_err(|err| format!("{err:#}"))?;
        Ok(serde_json::json!({
            "kind": "split",
            "projectId": project_id,
            "projectName": name.trim(),
            "sessions": identities,
        }))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn analytics_overview(
    app: tauri::AppHandle,
    agent: Option<String>,
    days: Option<u32>,
    rank_days: Option<u32>,
    refresh_transcripts: Option<bool>,
) -> Result<serde_json::Value, String> {
    let agent = agent
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(parse_agent_result)
        .transpose()?;
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        let refresh_report = if refresh_transcripts.unwrap_or(false) {
            let sessions = store
                .list_sessions()
                .map_err(|err| format!("{err:#}"))?
                .sessions
                .into_iter()
                .filter(|session| agent.is_none_or(|agent| session.agent == agent))
                .collect::<Vec<_>>();
            Some(refresh_session_analytics_serialized(
                &app, "overview", &store, &sessions,
            )?)
        } else {
            None
        };
        let mut overview = store
            .overview_analytics(agent, days.unwrap_or(182), rank_days.unwrap_or(30))
            .map_err(|err| format!("{err:#}"))?;
        let (total_sessions, providers) = store
            .session_overview_metadata(agent)
            .map_err(|err| format!("{err:#}"))?;
        overview.coverage.total_sessions = total_sessions;
        for provider in providers {
            if overview
                .capabilities
                .iter()
                .all(|entry| entry.agent != provider)
            {
                overview
                    .capabilities
                    .push(tendi_core::analytics::AnalyticsProviderCapability {
                        agent: provider,
                        capabilities: tendi_core::analytics::AnalyticsCapabilities::for_agent(
                            provider,
                        ),
                    });
            }
        }
        overview.capabilities.sort_by_key(|entry| entry.agent);
        if let Some(report) = refresh_report {
            overview.warnings.extend(report.warnings);
        }
        if refresh_transcripts.unwrap_or(false) {
            emit_analytics_revision(&app, overview.revision);
        }
        Ok(overview)
    })
    .await
}

#[tauri::command]
async fn analytics_revision() -> Result<serde_json::Value, String> {
    blocking_json(move || {
        tendi_core::storage::Store::open_default()
            .map_err(|err| format!("{err:#}"))?
            .analytics_revision()
            .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn session_analytics(
    app: tauri::AppHandle,
    session_id: String,
    agent: String,
    path: String,
) -> Result<serde_json::Value, String> {
    let agent = parse_agent_result(&agent)?;
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        let session = store
            .list_sessions()
            .map_err(|err| format!("{err:#}"))?
            .sessions
            .into_iter()
            .find(|session| {
                session.id == session_id
                    && session.agent == agent
                    && session.path == PathBuf::from(&path)
            })
            .ok_or_else(|| format!("session analytics source not found: {path}"))?;
        refresh_session_analytics_serialized(
            &app,
            "session",
            &store,
            std::slice::from_ref(&session),
        )?;
        let detail = store
            .cached_session_analytics_detail(&session)
            .map_err(|err| format!("{err:#}"))?;
        emit_analytics_revision(
            &app,
            store
                .analytics_revision()
                .map_err(|err| format!("{err:#}"))?,
        );
        Ok(detail)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn sessions_scan_start(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, SessionScanRuntime>,
) -> Result<u64, String> {
    if runtime.scan_running.load(Ordering::SeqCst) {
        return Ok(runtime.generation.load(Ordering::SeqCst));
    }
    let cwd = active_cwd()?;
    let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
    let additional_session_roots = store
        .app_settings()
        .map_err(|err| format!("{err:#}"))?
        .additional_session_roots
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let watch_plan = tendi_core::sessions::session_watch_plan(&cwd, &additional_session_roots);
    runtime.configure_watcher(&watch_plan)?;
    if runtime
        .scan_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(runtime.generation.load(Ordering::SeqCst));
    }
    let generation = runtime.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let scan_running = Arc::clone(&runtime.scan_running);
    let analytics_tx = runtime.analytics_tx.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = run_session_scan(
            &app,
            &cwd,
            &additional_session_roots,
            generation,
            &analytics_tx,
        ) {
            emit_session_scan_event(
                &app,
                SessionScanEvent {
                    generation,
                    phase: "error",
                    upserts: Vec::new(),
                    deleted: Vec::new(),
                    scanned: 0,
                    complete: true,
                    error: Some(error),
                },
            );
        }
        scan_running.store(false, Ordering::SeqCst);
    });
    Ok(generation)
}

fn run_session_scan(
    app: &tauri::AppHandle,
    cwd: &Path,
    additional_session_roots: &[PathBuf],
    generation: u64,
    analytics_tx: &Sender<AnalyticsRefreshJob>,
) -> Result<(), String> {
    let scan_started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
    let last_scan_at = store
        .sessions_last_scan_at()
        .map_err(|err| format!("{err:#}"))?;
    let cache = store
        .session_scan_cache()
        .map_err(|err| format!("{err:#}"))?;
    let mut scanned = 0;
    let mut roots = tendi_core::sessions::session_watch_roots(cwd, additional_session_roots);
    roots.sort_by_key(|root| session_root_priority(root));
    for root in roots {
        let recent_paths = tendi_core::sessions::recent_session_paths_in_root(&root, last_scan_at);
        for paths in recent_paths.chunks(SESSION_SCAN_BATCH_SIZE) {
            let report = tendi_core::sessions::scan_session_paths(paths, &cache);
            let analytics_sessions = report.sessions.clone();
            let changed = cache.changed_sessions(&report.sessions);
            let mut upserts = store
                .apply_session_delta(&changed)
                .map_err(|err| format!("{err:#}"))?;
            store
                .resolve_session_projects(&mut upserts)
                .map_err(|err| format!("{err:#}"))?;
            scanned += paths.len();
            emit_session_scan_event(
                app,
                SessionScanEvent {
                    generation,
                    phase: "recent",
                    upserts,
                    deleted: Vec::new(),
                    scanned,
                    complete: false,
                    error: None,
                },
            );
            let _ = analytics_tx.send(AnalyticsRefreshJob {
                phase: "recent",
                sessions: analytics_sessions,
            });
        }
    }
    emit_session_scan_event(
        app,
        SessionScanEvent {
            generation,
            phase: "recent",
            upserts: Vec::new(),
            deleted: Vec::new(),
            scanned,
            complete: true,
            error: None,
        },
    );

    let cache = store
        .session_scan_cache()
        .map_err(|err| format!("{err:#}"))?;
    let report = tendi_core::sessions::scan_sessions_with_additional_roots_cached(
        cwd,
        additional_session_roots,
        &cache,
    )
    .map_err(|err| format!("{err:#}"))?;
    store
        .save_sessions_at(&report, scan_started_at)
        .map_err(|err| format!("{err:#}"))?;
    let analytics_sessions = report.sessions.clone();
    emit_session_scan_event(
        app,
        SessionScanEvent {
            generation,
            phase: "backfill",
            upserts: Vec::new(),
            deleted: Vec::new(),
            scanned: report.sessions.len(),
            complete: true,
            error: None,
        },
    );
    let _ = analytics_tx.send(AnalyticsRefreshJob {
        phase: "backfill",
        sessions: analytics_sessions,
    });
    Ok(())
}

fn session_root_priority(root: &Path) -> u8 {
    let path = root.to_string_lossy();
    if path.contains("/.codex/sessions") {
        0
    } else if path.contains("/.cursor/") {
        1
    } else if path.contains("/.claude/") {
        2
    } else if path.contains("/.codex/archived_sessions")
        || path.ends_with("/.codex/session_index.jsonl")
    {
        4
    } else {
        3
    }
}

fn session_watch_loop(
    app: tauri::AppHandle,
    receiver: Receiver<notify::Result<Event>>,
    generation: Arc<AtomicU64>,
    scan_running: Arc<AtomicBool>,
    watcher: Arc<Mutex<SessionWatcherState>>,
    analytics_tx: Sender<AnalyticsRefreshJob>,
) {
    let mut pending = BTreeSet::new();
    let mut pending_since = None;
    loop {
        match receiver.recv_timeout(SESSION_WATCH_DEBOUNCE) {
            Ok(Ok(event)) => {
                for path in event.paths {
                    if let Some(session_root) = advance_tutti_session_watcher(&watcher, &path) {
                        pending.extend(tendi_core::sessions::recent_session_paths_in_root(
                            &session_root,
                            None,
                        ));
                    }
                    if tendi_core::sessions::is_session_candidate_path(&path) || !path.exists() {
                        pending.insert(path);
                    }
                }
                if !pending.is_empty() && pending_since.is_none() {
                    pending_since = Some(Instant::now());
                }
            }
            Ok(Err(error)) => emit_session_scan_event(
                &app,
                SessionScanEvent {
                    generation: generation.load(Ordering::SeqCst),
                    phase: "error",
                    upserts: Vec::new(),
                    deleted: Vec::new(),
                    scanned: 0,
                    complete: false,
                    error: Some(error.to_string()),
                },
            ),
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let debounce_elapsed =
            pending_since.is_some_and(|started| started.elapsed() >= SESSION_WATCH_DEBOUNCE);
        if pending.is_empty() || !debounce_elapsed || scan_running.load(Ordering::SeqCst) {
            continue;
        }
        let paths = std::mem::take(&mut pending).into_iter().collect::<Vec<_>>();
        pending_since = None;
        process_session_watch_paths(
            &app,
            generation.load(Ordering::SeqCst),
            &paths,
            &analytics_tx,
        );
    }
}

fn advance_tutti_session_watcher(
    state: &Arc<Mutex<SessionWatcherState>>,
    event_path: &Path,
) -> Option<PathBuf> {
    let mut state = state.lock().ok()?;
    let run_root = state
        .tutti_run_roots
        .iter()
        .find(|root| event_path.starts_with(root))?
        .clone();
    let run_name = event_path
        .strip_prefix(&run_root)
        .ok()?
        .components()
        .next()?;
    let run_dir = run_root.join(run_name);
    if !run_dir.is_dir() {
        return None;
    }
    let codex_home = run_dir.join("codex-home");
    let session_root = codex_home.join("sessions");
    if session_root.is_dir() {
        let newly_watched = watch_session_path(&mut state, &session_root, true);
        unwatch_session_path(&mut state, &run_dir);
        unwatch_session_path(&mut state, &codex_home);
        return newly_watched.then_some(session_root);
    }
    if codex_home.is_dir() {
        watch_session_path(&mut state, &codex_home, false);
        unwatch_session_path(&mut state, &run_dir);
    } else {
        watch_session_path(&mut state, &run_dir, false);
    }
    None
}

fn watch_session_path(state: &mut SessionWatcherState, path: &Path, recursive: bool) -> bool {
    if state.watched_paths.contains(path) {
        return false;
    }
    let mode = if recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    let Some(watcher) = state.watcher.as_mut() else {
        return false;
    };
    if watcher.watch(path, mode).is_err() {
        return false;
    }
    state.watched_paths.insert(path.to_path_buf());
    true
}

fn unwatch_session_path(state: &mut SessionWatcherState, path: &Path) {
    if !state.watched_paths.remove(path) {
        return;
    }
    if let Some(watcher) = state.watcher.as_mut() {
        let _ = watcher.unwatch(path);
    }
}

fn process_session_watch_paths(
    app: &tauri::AppHandle,
    generation: u64,
    paths: &[PathBuf],
    analytics_tx: &Sender<AnalyticsRefreshJob>,
) {
    let result = (|| -> Result<(
        Vec<tendi_core::SessionRecord>,
        Vec<tendi_core::sessions::SessionIdentity>,
        Vec<tendi_core::SessionRecord>,
    ), String> {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        let cache = store.session_scan_cache().map_err(|err| format!("{err:#}"))?;
        let existing_paths = paths
            .iter()
            .filter(|path| path.is_file())
            .cloned()
            .collect::<Vec<_>>();
        let deleted_paths = paths
            .iter()
            .filter(|path| !path.exists())
            .cloned()
            .collect::<Vec<_>>();
        let report = tendi_core::sessions::scan_session_paths(&existing_paths, &cache);
        let analytics_sessions = report.sessions.clone();
        let empty_paths = existing_paths
            .iter()
            .filter(|path| path.extension().is_some_and(|extension| extension == "jsonl"))
            .filter(|path| !report.sessions.iter().any(|session| session.path == **path))
            .cloned()
            .collect::<Vec<_>>();
        let changed = cache.changed_sessions(&report.sessions);
        let mut upserts = store
            .apply_session_delta(&changed)
            .map_err(|err| format!("{err:#}"))?;
        store
            .resolve_session_projects(&mut upserts)
            .map_err(|err| format!("{err:#}"))?;
        let mut deleted = store
            .remove_sessions_for_paths(&deleted_paths)
            .map_err(|err| format!("{err:#}"))?;
        deleted.extend(
            store
                .remove_sessions_for_paths(&empty_paths)
                .map_err(|err| format!("{err:#}"))?,
        );
        Ok((upserts, deleted, analytics_sessions))
    })();
    match result {
        Ok((upserts, deleted, analytics_sessions))
            if !upserts.is_empty() || !deleted.is_empty() || !analytics_sessions.is_empty() =>
        {
            let index_sessions = upserts.clone();
            emit_session_scan_event(
                app,
                SessionScanEvent {
                    generation,
                    phase: "watch",
                    scanned: paths.len(),
                    upserts,
                    deleted,
                    complete: true,
                    error: None,
                },
            );
            if !index_sessions.is_empty()
                && let Ok(store) = tendi_core::storage::Store::open_default()
            {
                let _ = store.index_session_delta(&index_sessions);
            }
            let _ = analytics_tx.send(AnalyticsRefreshJob {
                phase: "watch",
                sessions: analytics_sessions,
            });
        }
        Ok(_) => {}
        Err(error) => emit_session_scan_event(
            app,
            SessionScanEvent {
                generation,
                phase: "error",
                upserts: Vec::new(),
                deleted: Vec::new(),
                scanned: paths.len(),
                complete: false,
                error: Some(error),
            },
        ),
    }
}

fn emit_session_scan_event(app: &tauri::AppHandle, event: SessionScanEvent) {
    let _ = app.emit(SESSION_SCAN_EVENT, event);
}

#[tauri::command(rename_all = "camelCase")]
async fn sessions_search(query: String) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        let mut report = store
            .search_sessions(&query)
            .map_err(|err| format!("{err:#}"))?;
        let mut sessions = report
            .iter()
            .map(|hit| hit.session.clone())
            .collect::<Vec<_>>();
        store
            .resolve_session_projects(&mut sessions)
            .map_err(|err| format!("{err:#}"))?;
        for (hit, session) in report.iter_mut().zip(sessions) {
            hit.session = session;
        }
        Ok(report)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn session_skill_index_status() -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        store
            .session_skill_index_status(SESSION_SKILL_INDEX_RUNNING.load(Ordering::SeqCst))
            .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn session_skill_index_run(force: bool) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    let started = SESSION_SKILL_INDEX_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    if started {
        tauri::async_runtime::spawn_blocking(move || {
            let result = tendi_core::session_skills::run_index(&cwd, force);
            if let Err(err) = result {
                eprintln!("session skill index failed: {err:#}");
            }
            SESSION_SKILL_INDEX_RUNNING.store(false, Ordering::SeqCst);
        });
    }
    session_skill_index_status().await
}

#[tauri::command(rename_all = "camelCase")]
async fn session_skill_links(
    session_id: String,
    agent: String,
) -> Result<serde_json::Value, String> {
    let agent = parse_agent(&agent);
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        store
            .session_skill_links(&session_id, agent)
            .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skill_session_links(skill_name: String) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        store
            .skill_session_links(&skill_name)
            .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn settings_get() -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        store.app_settings().map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn settings_save(
    appearance: String,
    light_theme: String,
    dark_theme: String,
    terminal: String,
    editor: String,
    additional_session_roots: Vec<String>,
    config_profiles: BTreeMap<String, String>,
) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        store
            .save_app_settings(tendi_core::storage::AppSettings {
                appearance,
                light_theme,
                dark_theme,
                terminal,
                editor,
                additional_session_roots,
                config_profiles,
            })
            .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn terminal_apps_list() -> Result<serde_json::Value, String> {
    blocking_json(move || Ok(terminal_apps())).await
}

#[tauri::command(rename_all = "camelCase")]
async fn terminal_app_test(terminal: String) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let terminal = resolve_terminal(&terminal);
        let app_name = terminal_application_name(&terminal);
        open_terminal_application(&app_name)?;
        Ok(app_name)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn editor_app_test(editor: String) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        test_editor_command(&editor)?;
        Ok(true)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn session_resume_in_terminal(
    session: SessionResumeRequest,
) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let agent = parse_agent_result(&session.agent)?;
        let path = PathBuf::from(session.path);
        let project = absolute_project_path(session.project)?
            .or_else(|| tendi_core::sessions::infer_session_project(&path, agent));
        let record = tendi_core::SessionRecord {
            id: session.id,
            agent,
            title: session.title,
            project,
            repository: None,
            repository_url: None,
            logical_project_id: None,
            logical_project_name: None,
            path,
            started_at: None,
            updated_at: None,
            message_count: None,
            first_user_message: None,
            last_user_message: None,
            last_assistant_message: None,
            turn_count: None,
            model: None,
            mode: None,
            approval_mode: None,
            is_run_everything: None,
            parent_session_id: None,
            token_usage: None,
        };
        let mut plan =
            tendi_core::plan_session_resume(&record).map_err(|err| format!("{err:#}"))?;
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        let settings = store.app_settings().map_err(|err| format!("{err:#}"))?;
        if let Some(profile) = config_profile_key(plan.agent)
            .and_then(|agent| settings.config_profiles.get(agent))
            .map(String::as_str)
        {
            match plan.agent {
                AgentKind::Codex => {
                    if !tendi_core::config::config_profile_exists(AgentKind::Codex, profile)
                        .map_err(|err| format!("{err:#}"))?
                    {
                        return Err(format!("Codex profile not found: {profile}"));
                    }
                    plan.command
                        .args
                        .splice(0..0, ["--profile".to_string(), profile.to_string()]);
                }
                AgentKind::Claude => {
                    let profile_path =
                        tendi_core::config::config_profile_path(AgentKind::Claude, profile)
                            .map_err(|err| format!("{err:#}"))?;
                    if !profile_path.is_file() {
                        return Err(format!("Claude Code profile not found: {profile}"));
                    }
                    plan.command.args.splice(
                        0..0,
                        ["--settings".to_string(), profile_path.display().to_string()],
                    );
                }
                AgentKind::Cursor => {
                    let profile_path =
                        tendi_core::config::config_profile_path(AgentKind::Cursor, profile)
                            .map_err(|err| format!("{err:#}"))?;
                    if !profile_path.is_file() {
                        return Err(format!("Cursor profile not found: {profile}"));
                    }
                    let config_dir = profile_path
                        .parent()
                        .ok_or_else(|| "Cursor profile directory is unavailable".to_string())?;
                    plan.command.env.push((
                        "CURSOR_CONFIG_DIR".to_string(),
                        config_dir.display().to_string(),
                    ));
                }
                _ => {}
            }
        }
        let terminal = resolve_terminal(&settings.terminal);
        let command_line = launch_command_in_terminal(&plan.command, &terminal)?;
        Ok(SessionResumeLaunch {
            agent: plan.agent,
            terminal,
            command_line,
        })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn rules_list() -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let report = tendi_core::rules::scan_rules(&cwd).map_err(|err| format!("{err:#}"))?;
        replace_rule_path_cache(&cwd, &report.rules);
        Ok(report.rules)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn rule_file_read(path: String) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let path = std::path::Path::new(&path);
        if cached_rule_path(&cwd, path) {
            tendi_core::rules::read_rule_file_at_path(path)
        } else {
            tendi_core::rules::read_rule_file(&cwd, path)
        }
        .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn rule_file_save(
    path: String,
    expected_sha256: String,
    content: String,
) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let path = std::path::Path::new(&path);
        let result = if cached_rule_path(&cwd, path) {
            tendi_core::rules::save_rule_file_at_path(path, &expected_sha256, &content)
        } else {
            tendi_core::rules::save_rule_file(&cwd, path, &expected_sha256, &content)
        };
        result.map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn hooks_list() -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let report = tendi_core::hooks::scan_hooks(&cwd).map_err(|err| format!("{err:#}"))?;
        replace_hook_path_cache(&cwd, &report.hooks);
        Ok(report.hooks)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn hook_delete(
    path: String,
    expected_trust_hash: String,
    event: String,
    matcher: Option<String>,
    hook_type: Option<String>,
    command: Option<String>,
    url: Option<String>,
    prompt: Option<String>,
    filter: Option<String>,
    status_message: Option<String>,
) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let _mutation = HOOK_MUTATION_LOCK
            .lock()
            .map_err(|_| "hook mutation authority is unavailable".to_string())?;
        tendi_core::hooks::delete_hook(tendi_core::hooks::HookDeleteRequest {
            path: PathBuf::from(path),
            expected_trust_hash,
            event,
            matcher,
            hook_type,
            command,
            url,
            prompt,
            filter,
            status_message,
        })
        .map_err(|err| format!("{err:#}"))?;
        let report = tendi_core::hooks::scan_hooks(&cwd).map_err(|err| format!("{err:#}"))?;
        replace_hook_path_cache(&cwd, &report.hooks);
        Ok(report.hooks)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn hook_delete_many(requests: Vec<HookDeleteInput>) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let _mutation = HOOK_MUTATION_LOCK
            .lock()
            .map_err(|_| "hook mutation authority is unavailable".to_string())?;
        tendi_core::hooks::delete_hooks(requests.into_iter().map(Into::into).collect())
            .map_err(|err| format!("{err:#}"))?;
        let report = tendi_core::hooks::scan_hooks(&cwd).map_err(|err| format!("{err:#}"))?;
        replace_hook_path_cache(&cwd, &report.hooks);
        Ok(report.hooks)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn hook_set_enabled(
    path: String,
    expected_trust_hash: String,
    event: String,
    matcher: Option<String>,
    hook_type: Option<String>,
    command: Option<String>,
    url: Option<String>,
    prompt: Option<String>,
    filter: Option<String>,
    status_message: Option<String>,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let _mutation = HOOK_MUTATION_LOCK
            .lock()
            .map_err(|_| "hook mutation authority is unavailable".to_string())?;
        tendi_core::hooks::set_hook_enabled(tendi_core::hooks::HookSetEnabledRequest {
            path: PathBuf::from(path),
            expected_trust_hash,
            event,
            matcher,
            hook_type,
            command,
            url,
            prompt,
            filter,
            status_message,
            enabled,
        })
        .map_err(|err| format!("{err:#}"))?;
        let report = tendi_core::hooks::scan_hooks(&cwd).map_err(|err| format!("{err:#}"))?;
        replace_hook_path_cache(&cwd, &report.hooks);
        Ok(report.hooks)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn hook_review(
    path: String,
    expected_trust_hash: String,
    event: String,
    matcher: Option<String>,
    hook_type: Option<String>,
    command: Option<String>,
    url: Option<String>,
    prompt: Option<String>,
    filter: Option<String>,
    status_message: Option<String>,
) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let _mutation = HOOK_MUTATION_LOCK
            .lock()
            .map_err(|_| "hook mutation authority is unavailable".to_string())?;
        let report = tendi_core::hooks::review_hook_and_scan(
            &cwd,
            tendi_core::hooks::HookReviewRequest {
                path: PathBuf::from(path),
                expected_trust_hash,
                event,
                matcher,
                hook_type,
                command,
                url,
                prompt,
                filter,
                status_message,
            },
        )
        .map_err(|err| format!("{err:#}"))?;
        replace_hook_path_cache(&cwd, &report.hooks);
        Ok(report.hooks)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn hook_source_read(
    path: String,
    expected_trust_hash: Option<String>,
    event: Option<String>,
    matcher: Option<String>,
    hook_type: Option<String>,
    command: Option<String>,
    url: Option<String>,
    prompt: Option<String>,
    filter: Option<String>,
    status_message: Option<String>,
    enabled: Option<bool>,
) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let hook_match = event.map(|event| tendi_core::hooks::HookSourceMatch {
            event,
            matcher,
            hook_type,
            command,
            url,
            prompt,
            filter,
            status_message,
            enabled,
        });
        let path = std::path::Path::new(&path);
        let result = if cached_hook_path(&cwd, path) {
            tendi_core::hooks::read_hook_source_at_path(
                path,
                expected_trust_hash.as_deref(),
                hook_match.as_ref(),
            )
        } else {
            tendi_core::hooks::read_hook_source(
                &cwd,
                path,
                expected_trust_hash.as_deref(),
                hook_match.as_ref(),
            )
        };
        result.map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn mcp_list() -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let report = tendi_core::mcp::scan_mcp(&cwd).map_err(|err| format!("{err:#}"))?;
        Ok(report.servers)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn prompts_list() -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        store.list_prompts().map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn prompt_save(
    id: Option<String>,
    title: String,
    tags: Vec<String>,
    body: String,
) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        store
            .save_prompt(tendi_core::storage::PromptWrite {
                id,
                title,
                tags,
                body,
            })
            .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn prompts_delete_many(ids: Vec<String>) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        let deleted = store
            .delete_prompts(&ids)
            .map_err(|err| format!("{err:#}"))?;
        Ok(serde_json::json!({ "deleted": deleted }))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn session_transcript(
    path: String,
    agent: String,
    cursor: Option<String>,
    limit: Option<usize>,
    known_source_version: Option<String>,
) -> Result<serde_json::Value, String> {
    let agent = parse_agent(&agent);
    blocking_json(move || {
        if cursor.is_none() {
            let source_version =
                tendi_core::transcript::transcript_source_version(std::path::Path::new(&path))
                    .map_err(|err| format!("{err:#}"))?;
            if known_source_version.as_deref() == Some(&source_version) {
                return Ok(tendi_core::transcript::TranscriptPage {
                    items: Vec::new(),
                    warnings: Vec::new(),
                    next_cursor: None,
                    done: true,
                    source_version,
                    restart_required: false,
                    unchanged: true,
                });
            }
        }
        tendi_core::transcript::parse_transcript_page(
            std::path::Path::new(&path),
            agent,
            cursor.as_deref(),
            limit,
        )
        .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skills_set(
    names: Vec<String>,
    visibility: String,
    dry_run: bool,
) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    let visibility = parse_visibility(&visibility)?;
    blocking_json(move || {
        let _authority = (!dry_run).then(lock_skill_authority).transpose()?;
        let before = (!dry_run).then(|| skill_scan_snapshot(&cwd)).transpose()?;
        let changeset = if let Some(before) = before.as_ref() {
            tendi_core::skills::plan_visibility_many_for_scan(before, &names, visibility)
        } else {
            tendi_core::skills::plan_visibility_many(&cwd, &names, visibility)
        }
        .map_err(|err| format!("{err:#}"))?;
        let summary = tendi_core::skills::format_changeset(&changeset);
        if !dry_run {
            tendi_core::skills::apply_changes(&changeset).map_err(|err| format!("{err:#}"))?;
        }
        let skills = before
            .as_ref()
            .map(|before| commit_skill_refresh(&cwd, before, &names, &[]))
            .transpose()?
            .map(|scan| scan.skills);
        Ok(serde_json::json!({
            "summary": summary,
            "applied": !dry_run,
            "skills": skills,
        }))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skills_wrap(
    name: String,
    names: Vec<String>,
    description: Option<String>,
    manual_children: bool,
    refresh: bool,
    dry_run: bool,
) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let _authority = (!dry_run).then(lock_skill_authority).transpose()?;
        let before = (!dry_run).then(|| skill_scan_snapshot(&cwd)).transpose()?;
        let changeset = if refresh {
            if let Some(before) = before.as_ref() {
                tendi_core::skills::refresh_wrapper_from_names_for_scan(
                    before,
                    &name,
                    &names,
                    manual_children,
                )
            } else {
                tendi_core::skills::refresh_wrapper_from_names(&cwd, &name, &names, manual_children)
            }
        } else if let Some(before) = before.as_ref() {
            tendi_core::skills::plan_wrapper_from_names_for_scan(
                before,
                &name,
                &names,
                description.as_deref(),
                manual_children,
            )
        } else {
            tendi_core::skills::plan_wrapper_from_names(
                &cwd,
                &name,
                &names,
                description.as_deref(),
                manual_children,
            )
        }
        .map_err(|err| format!("{err:#}"))?;
        let summary = tendi_core::skills::format_changeset(&changeset);
        if !dry_run {
            tendi_core::skills::apply_changes(&changeset).map_err(|err| format!("{err:#}"))?;
        }
        let skills = if let Some(before) = before.as_ref() {
            let skill_dirs = changeset
                .changes
                .iter()
                .filter(|change| {
                    change
                        .path
                        .file_name()
                        .is_some_and(|name| name == "SKILL.md")
                })
                .filter_map(|change| change.path.parent().map(Path::to_path_buf))
                .collect::<Vec<_>>();
            Some(
                commit_skill_refresh(&cwd, before, std::slice::from_ref(&name), &skill_dirs)?
                    .skills,
            )
        } else {
            None
        };
        Ok(serde_json::json!({
            "summary": summary,
            "applied": !dry_run,
            "skills": skills,
        }))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skills_updates(app: tauri::AppHandle, check: bool) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        if check {
            let update_check = start_skill_update_check(&app, cwd, None, false);
            Ok(serde_json::json!({ "updateCheck": update_check }))
        } else {
            let _authority = lock_skill_authority()?;
            let report =
                tendi_core::skills::scan_skills_synced(&cwd).map_err(|err| format!("{err:#}"))?;
            let store =
                tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
            store
                .save_skills(&report)
                .map_err(|err| format!("{err:#}"))?;
            cache_skill_scan(&cwd, &report);
            serde_json::to_value(report.skills).map_err(|err| err.to_string())
        }
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skills_update(pattern: String, dry_run: bool) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let _authority = (!dry_run).then(lock_skill_authority).transpose()?;
        let before = (!dry_run).then(|| skill_scan_snapshot(&cwd)).transpose()?;
        let plan = tendi_core::skills::plan_skill_updates(&cwd, &pattern)
            .map_err(|err| format!("{err:#}"))?;
        let summary = tendi_core::skills::format_update_plan(&plan);
        if !dry_run {
            tendi_core::skills::apply_skill_update_plan(&plan).map_err(|err| format!("{err:#}"))?;
        }
        let mut changed_names = plan
            .git_updates
            .iter()
            .map(|update| update.name.clone())
            .collect::<Vec<_>>();
        if let Some(before) = before.as_ref() {
            changed_names.extend(before.skills.iter().filter_map(|skill| {
                let changed = skill.paths.iter().any(|path| {
                    plan.file_changes
                        .changes
                        .iter()
                        .any(|change| change.path.starts_with(&path.path))
                        || {
                            plan.git_updates
                                .iter()
                                .any(|update| path.path.starts_with(&update.repo))
                        }
                });
                changed.then(|| skill.name.clone())
            }));
            changed_names.sort();
            changed_names.dedup();
        }
        let skills = before
            .as_ref()
            .map(|before| commit_skill_refresh(&cwd, before, &changed_names, &[]))
            .transpose()?
            .map(|scan| scan.skills);
        Ok(serde_json::json!({
            "summary": summary,
            "applied": !dry_run,
            "plan": plan,
            "skills": skills,
        }))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skills_update_many(
    names: Vec<String>,
    preview_id: Option<String>,
    dry_run: bool,
) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let _authority = (!dry_run).then(lock_skill_authority).transpose()?;
        let before = (!dry_run).then(|| skill_scan_snapshot(&cwd)).transpose()?;
        let plan = if dry_run {
            tendi_core::skills::plan_skill_updates_many(&cwd, &names)
                .map_err(|err| format!("{err:#}"))?
        } else if let Some(preview_id) = preview_id.as_deref() {
            take_skill_update_preview(preview_id, &names)?
        } else {
            tendi_core::skills::plan_skill_updates_many(&cwd, &names)
                .map_err(|err| format!("{err:#}"))?
        };
        let summary = tendi_core::skills::format_update_plan(&plan);
        let stored_preview_id = if dry_run {
            Some(store_skill_update_preview(&names, plan.clone())?)
        } else {
            None
        };
        if !dry_run {
            tendi_core::skills::apply_skill_update_plan(&plan).map_err(|err| format!("{err:#}"))?;
        }
        let mut changed_names = names.clone();
        if let Some(before) = before.as_ref() {
            changed_names.extend(before.skills.iter().filter_map(|skill| {
                skill
                    .paths
                    .iter()
                    .any(|path| {
                        plan.git_updates
                            .iter()
                            .any(|update| path.path.starts_with(&update.repo))
                    })
                    .then(|| skill.name.clone())
            }));
            changed_names.sort();
            changed_names.dedup();
        }
        let skills = before
            .as_ref()
            .map(|before| commit_skill_refresh(&cwd, before, &changed_names, &[]))
            .transpose()?
            .map(|scan| scan.skills);
        Ok(serde_json::json!({
            "summary": summary,
            "applied": !dry_run,
            "plan": plan,
            "previewId": stored_preview_id,
            "skills": skills,
        }))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
fn skills_targets() -> serde_json::Value {
    serde_json::Value::Array(
        tendi_core::skill_targets::target_catalog()
            .iter()
            .map(|target| {
                serde_json::json!({
                    "id": target.id,
                    "displayName": target.display_name,
                    "projectSkillsDir": target.project_skills_dir,
                    "supportsGlobal": target.supports_global(),
                })
            })
            .collect(),
    )
}

#[tauri::command(rename_all = "camelCase")]
async fn skills_add(
    source: String,
    target: String,
    scope: String,
    skills: Vec<String>,
    copy: bool,
    overwrite: bool,
    visibility: String,
    preview_id: Option<String>,
    dry_run: bool,
) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    let target = target
        .parse::<tendi_core::SkillTarget>()
        .map_err(|err| format!("{err:#}"))?;
    let scope = scope
        .parse::<tendi_core::SkillInstallScope>()
        .map_err(|err| format!("{err:#}"))?;
    let visibility = parse_visibility(&visibility)?;
    blocking_json(move || {
        let options = tendi_core::skills::SkillAddOptions {
            source,
            target,
            scope,
            skills,
            copy,
            overwrite,
            visibility,
        };
        if dry_run {
            let plan = tendi_core::skills::plan_skill_add(&cwd, &options)
                .map_err(|err| format!("{err:#}"))?;
            let preview_id = store_skill_add_preview(&options, plan.clone())?;
            return Ok(serde_json::json!({
                "applied": false,
                "plan": plan,
                "previewId": preview_id,
            }));
        }
        let preview_id = preview_id.as_deref().ok_or_else(|| {
            "skill add preview expired; preview the installation again".to_string()
        })?;
        let preview = take_skill_add_preview(preview_id, &options)?;
        let _authority = lock_skill_authority()?;
        let report = tendi_core::skills::apply_skill_add_preview(&preview, &options)
            .map_err(|err| format!("{err:#}"))?;
        let source_records = tendi_core::skills::skill_source_records_for_add(&report);
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        store
            .upsert_skill_source_records(&source_records)
            .map_err(|err| format!("{err:#}"))?;
        // Re-scan through the source store so the first UI result already uses
        // the newly authoritative database row instead of transient local inference.
        let refreshed =
            tendi_core::skills::scan_skills_synced(&cwd).map_err(|err| format!("{err:#}"))?;
        store
            .save_skills(&refreshed)
            .map_err(|err| format!("{err:#}"))?;
        cache_skill_scan(&cwd, &refreshed);
        Ok(serde_json::json!({
            "applied": true,
            "report": report.clone(),
            "plan": report.plan,
            "results": report.results,
            "skills": refreshed.skills,
        }))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skills_delete_many(
    names: Vec<String>,
    preview_id: Option<String>,
    dry_run: bool,
) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let _authority = (!dry_run).then(lock_skill_authority).transpose()?;
        let before = (!dry_run).then(|| skill_scan_snapshot(&cwd)).transpose()?;
        let plan = if dry_run {
            tendi_core::skills::plan_skill_delete_many(&cwd, &names)
                .map_err(|err| format!("{err:#}"))?
        } else if let Some(preview_id) = preview_id.as_deref() {
            take_skill_delete_preview(preview_id, &names)?
        } else if let Some(before) = before.as_ref() {
            tendi_core::skills::plan_skill_delete_many_for_scan(before, &names)
                .map_err(|err| format!("{err:#}"))?
        } else {
            tendi_core::skills::plan_skill_delete_many(&cwd, &names)
                .map_err(|err| format!("{err:#}"))?
        };
        let refresh_required = false;
        let summary = tendi_core::skills::format_delete_plan(&plan);
        let stored_preview_id = if dry_run {
            Some(store_skill_delete_preview(&names, plan.clone())?)
        } else {
            None
        };
        if !dry_run {
            tendi_core::skills::apply_skill_delete_plan(&plan).map_err(|err| format!("{err:#}"))?;
            let source_paths = plan
                .targets
                .iter()
                .map(|target| target.path.clone())
                .collect::<Vec<_>>();
            let store =
                tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
            store
                .delete_skill_source_records(&source_paths)
                .map_err(|err| format!("{err:#}"))?;
        }
        let skills = before
            .as_ref()
            .map(|before| commit_skill_refresh(&cwd, before, &names, &[]))
            .transpose()?
            .map(|scan| scan.skills);
        Ok(serde_json::json!({
            "summary": summary,
            "applied": !dry_run,
            "plan": plan,
            "previewId": stored_preview_id,
            "refreshRequired": refresh_required,
            "skills": skills,
        }))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skill_files(name: String) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    let cached_skill_dir = cached_skill_dir(&cwd, &name);
    blocking_json(move || {
        tendi_core::files::list_skill_files(&cwd, &name, cached_skill_dir.as_deref())
            .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skill_file_read(name: String, relative_path: String) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    let cached_skill_dir = cached_skill_dir(&cwd, &name);
    blocking_json(move || {
        tendi_core::files::read_skill_file(&cwd, &name, &relative_path, cached_skill_dir.as_deref())
            .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skill_file_save(
    name: String,
    relative_path: String,
    expected_sha256: String,
    content: String,
) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    let cached_skill_dir = cached_skill_dir(&cwd, &name);
    blocking_json(move || {
        let _authority = (relative_path == "SKILL.md")
            .then(lock_skill_authority)
            .transpose()?;
        let before = (relative_path == "SKILL.md")
            .then(|| skill_scan_snapshot(&cwd))
            .transpose()?;
        let result = tendi_core::files::save_skill_file(
            &cwd,
            &name,
            &relative_path,
            &expected_sha256,
            &content,
            cached_skill_dir.as_deref(),
        )
        .map_err(|err| format!("{err:#}"))?;
        let extra_skill_dirs = cached_skill_dir.iter().cloned().collect::<Vec<_>>();
        let skills = before
            .as_ref()
            .map(|before| {
                commit_skill_refresh(&cwd, before, std::slice::from_ref(&name), &extra_skill_dirs)
            })
            .transpose()?
            .map(|scan| scan.skills);
        let mut value = serde_json::to_value(result).map_err(|err| err.to_string())?;
        if let (Some(skills), Some(object)) = (skills, value.as_object_mut()) {
            object.insert(
                "skills".to_string(),
                serde_json::to_value(skills).map_err(|err| err.to_string())?,
            );
        }
        Ok(value)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skill_file_create(
    name: String,
    relative_path: String,
) -> Result<serde_json::Value, String> {
    if relative_path == "SKILL.md" {
        return Err("the root SKILL.md must be edited in place".to_string());
    }
    let cwd = active_cwd()?;
    let cached_skill_dir = cached_skill_dir(&cwd, &name);
    blocking_json(move || {
        tendi_core::files::create_skill_file(
            &cwd,
            &name,
            &relative_path,
            cached_skill_dir.as_deref(),
        )
        .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skill_folder_create(name: String, relative_path: String) -> Result<(), String> {
    let cwd = active_cwd()?;
    let cached_skill_dir = cached_skill_dir(&cwd, &name);
    blocking_unit(move || {
        tendi_core::files::create_skill_folder(
            &cwd,
            &name,
            &relative_path,
            cached_skill_dir.as_deref(),
        )
        .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skill_path_rename(
    name: String,
    from_relative_path: String,
    to_relative_path: String,
) -> Result<(), String> {
    if from_relative_path == "SKILL.md" || to_relative_path == "SKILL.md" {
        return Err("the root SKILL.md cannot be renamed".to_string());
    }
    let cwd = active_cwd()?;
    let cached_skill_dir = cached_skill_dir(&cwd, &name);
    blocking_unit(move || {
        tendi_core::files::rename_skill_path(
            &cwd,
            &name,
            &from_relative_path,
            &to_relative_path,
            cached_skill_dir.as_deref(),
        )
        .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skill_path_delete(name: String, relative_path: String) -> Result<(), String> {
    if relative_path == "SKILL.md" {
        return Err("the root SKILL.md cannot be deleted".to_string());
    }
    let cwd = active_cwd()?;
    let cached_skill_dir = cached_skill_dir(&cwd, &name);
    blocking_unit(move || {
        tendi_core::files::delete_skill_path(
            &cwd,
            &name,
            &relative_path,
            cached_skill_dir.as_deref(),
        )
        .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
fn open_in_editor(path: String, line: Option<u32>) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.exists() {
        return Err(format!("path does not exist: {}", path.display()));
    }

    let editor = tendi_core::storage::Store::open_default()
        .and_then(|store| store.app_settings())
        .map(|settings| settings.editor)
        .map_err(|err| format!("failed to load editor setting: {err:#}"))?;
    open_editor_file(&editor, &path, line)
}

fn open_editor_file(editor: &str, path: &Path, line: Option<u32>) -> Result<(), String> {
    let editor = normalize_editor(editor);
    let args = editor_file_args(&editor, path, line);
    for program in editor_program_candidates(&editor) {
        match Command::new(&program).args(&args).status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(_) | Err(_) => continue,
        }
    }
    Err(format!("could not open path with editor: {editor}"))
}

fn test_editor_command(editor: &str) -> Result<(), String> {
    let editor = normalize_editor(editor);
    let version_arg = if editor == "coteditor" {
        "-v"
    } else {
        "--version"
    };
    for program in editor_program_candidates(&editor) {
        match Command::new(&program).arg(version_arg).status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(_) | Err(_) => continue,
        }
    }
    Err(format!("editor command is not available: {editor}"))
}

fn normalize_editor(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "vscode" | "vs code" | "visual studio code" | "code" => "vscode".to_string(),
        "zed" => "zed".to_string(),
        "cursor" => "cursor".to_string(),
        "coteditor" | "cot editor" | "cot" => "coteditor".to_string(),
        _ => value.trim().to_string(),
    }
}

fn editor_program_candidates(editor: &str) -> Vec<String> {
    match editor {
        "vscode" => vec![
            "code".to_string(),
            "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code".to_string(),
        ],
        "zed" => vec!["zed".to_string()],
        "cursor" => vec![
            "cursor".to_string(),
            "/Applications/Cursor.app/Contents/Resources/app/bin/cursor".to_string(),
        ],
        "coteditor" => vec![
            "cot".to_string(),
            "/Applications/CotEditor.app/Contents/SharedSupport/bin/cot".to_string(),
        ],
        custom => vec![custom.to_string()],
    }
}

fn editor_file_args(editor: &str, path: &Path, line: Option<u32>) -> Vec<String> {
    let path_text = path.to_string_lossy().to_string();
    let goto_target = match line.filter(|value| *value > 0) {
        Some(line) => format!("{path_text}:{line}"),
        None => path_text.clone(),
    };
    match editor {
        "vscode" => vec!["-g".to_string(), goto_target],
        "cursor" => vec!["--goto".to_string(), goto_target],
        "coteditor" => match line.filter(|value| *value > 0) {
            Some(line) => vec!["--line".to_string(), line.to_string(), path_text],
            None => vec![path_text],
        },
        _ => vec![goto_target],
    }
}

#[tauri::command(rename_all = "camelCase")]
fn reveal_in_finder(path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.exists() {
        return Err(format!("path does not exist: {}", path.display()));
    }

    #[cfg(target_os = "macos")]
    let status = Command::new("open")
        .arg("-R")
        .arg(&path)
        .status()
        .map_err(|err| format!("failed to reveal path: {err}"))?;

    #[cfg(target_os = "windows")]
    let status = Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .status()
        .map_err(|err| format!("failed to reveal path: {err}"))?;

    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open")
        .arg(if path.is_dir() {
            path.as_path()
        } else {
            path.parent().unwrap_or(path.as_path())
        })
        .status()
        .map_err(|err| format!("failed to reveal path: {err}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("reveal command exited with {status}"))
    }
}

#[tauri::command(rename_all = "camelCase")]
fn open_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(format!("unsupported url: {trimmed}"));
    }

    #[cfg(target_os = "macos")]
    let status = Command::new("open")
        .arg(trimmed)
        .status()
        .map_err(|err| format!("failed to open url: {err}"))?;

    #[cfg(target_os = "windows")]
    let status = Command::new("cmd")
        .args(["/C", "start", "", trimmed])
        .status()
        .map_err(|err| format!("failed to open url: {err}"))?;

    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open")
        .arg(trimmed)
        .status()
        .map_err(|err| format!("failed to open url: {err}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("open url command exited with {status}"))
    }
}

fn terminal_apps() -> Vec<TerminalApp> {
    vec![
        TerminalApp {
            id: "auto",
            label: "Auto",
            available: true,
        },
        TerminalApp {
            id: "terminal",
            label: "Terminal",
            available: app_available(
                "Terminal",
                &[
                    "/System/Applications/Utilities/Terminal.app",
                    "/Applications/Utilities/Terminal.app",
                ],
            ),
        },
        TerminalApp {
            id: "iterm",
            label: "iTerm",
            available: app_available(
                "iTerm",
                &["/Applications/iTerm.app", "/Applications/iTerm2.app"],
            ),
        },
        TerminalApp {
            id: "ghostty",
            label: "Ghostty",
            available: app_available("Ghostty", &["/Applications/Ghostty.app"]),
        },
        TerminalApp {
            id: "warp",
            label: "Warp",
            available: app_available("Warp", &["/Applications/Warp.app"]),
        },
    ]
}

fn app_available(app_name: &str, paths: &[&str]) -> bool {
    let _ = app_name;
    if paths.iter().any(|path| PathBuf::from(path).exists()) {
        return true;
    }
    false
}

fn resolve_terminal(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "terminal" => "terminal".to_string(),
        "iterm" | "iterm2" => "iterm".to_string(),
        "ghostty" => "ghostty".to_string(),
        "warp" => "warp".to_string(),
        "" | "auto" => {
            if terminal_apps()
                .iter()
                .any(|app| app.id == "terminal" && app.available)
            {
                "terminal".to_string()
            } else {
                "auto".to_string()
            }
        }
        _ => value.trim().to_string(),
    }
}

fn terminal_application_name(terminal: &str) -> String {
    match terminal {
        "auto" | "terminal" => "Terminal".to_string(),
        "iterm" => "iTerm".to_string(),
        "ghostty" => "Ghostty".to_string(),
        "warp" => "Warp".to_string(),
        app_name => app_name.to_string(),
    }
}

fn open_terminal_application(app_name: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("open")
            .args(["-a", app_name])
            .status()
            .map_err(|err| format!("failed to open {app_name}: {err}"))?;
        return command_status(status, &format!("open {app_name}"));
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = app_name;
        Err("testing terminal applications is only supported on macOS".to_string())
    }
}

fn launch_command_in_terminal(
    command: &tendi_core::SessionCommand,
    terminal: &str,
) -> Result<String, String> {
    let script = terminal_script(command);
    match terminal {
        "iterm" => run_iterm_script(&script)?,
        "terminal" | "auto" => run_terminal_script(&script)?,
        "ghostty" => open_command_file("Ghostty", &script)?,
        "warp" => open_command_file("Warp", &script)?,
        app_name => open_command_file(app_name, &script)?,
    }
    Ok(shell_command(command))
}

fn terminal_script(command: &tendi_core::SessionCommand) -> String {
    match &command.cwd {
        Some(cwd) => format!(
            "cd {} && exec {}",
            shell_quote(&cwd.display().to_string()),
            shell_command(command)
        ),
        None => format!("exec {}", shell_command(command)),
    }
}

fn shell_command(command: &tendi_core::SessionCommand) -> String {
    let command_line = std::iter::once(command.executable.as_str())
        .chain(command.args.iter().map(String::as_str))
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ");
    if command.env.is_empty() {
        command_line
    } else {
        let env = command
            .env
            .iter()
            .map(|(key, value)| format!("{}={}", shell_quote(key), shell_quote(value)))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{env} {command_line}")
    }
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn applescript_quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn run_terminal_script(script: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("osascript")
            .args([
                "-e",
                "tell application \"Terminal\"",
                "-e",
                "activate",
                "-e",
                &format!("do script \"{}\"", applescript_quote(script)),
                "-e",
                "end tell",
            ])
            .status()
            .map_err(|err| format!("failed to open Terminal: {err}"))?;
        return command_status(status, "open Terminal");
    }

    #[cfg(not(target_os = "macos"))]
    {
        open_command_file("Terminal", script)
    }
}

fn run_iterm_script(script: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("osascript")
            .args([
                "-e",
                "tell application \"iTerm\"",
                "-e",
                "activate",
                "-e",
                "set newWindow to (create window with default profile)",
                "-e",
                "tell current session of newWindow",
                "-e",
                &format!("write text \"{}\"", applescript_quote(script)),
                "-e",
                "end tell",
                "-e",
                "end tell",
            ])
            .status()
            .map_err(|err| format!("failed to open iTerm: {err}"))?;
        return command_status(status, "open iTerm");
    }

    #[cfg(not(target_os = "macos"))]
    {
        open_command_file("iTerm", script)
    }
}

fn open_command_file(app_name: &str, script: &str) -> Result<(), String> {
    let path = env::temp_dir().join(format!("tendi-session-{}.command", std::process::id()));
    fs::write(&path, format!("#!/bin/zsh\n{script}\n"))
        .map_err(|err| format!("failed to write command file: {err}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path)
            .map_err(|err| format!("failed to stat command file: {err}"))?
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions)
            .map_err(|err| format!("failed to make command file executable: {err}"))?;
    }

    #[cfg(target_os = "macos")]
    let status = Command::new("open")
        .args(["-a", app_name])
        .arg(&path)
        .status()
        .map_err(|err| format!("failed to open {app_name}: {err}"))?;

    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open")
        .arg(&path)
        .status()
        .map_err(|err| format!("failed to open command file: {err}"))?;

    #[cfg(windows)]
    let status = Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(&path)
        .status()
        .map_err(|err| format!("failed to open command file: {err}"))?;

    command_status(status, &format!("open {app_name}"))
}

fn command_status(status: std::process::ExitStatus, action: &str) -> Result<(), String> {
    if status.success() {
        Ok(())
    } else {
        Err(format!("{action} exited with {status}"))
    }
}

fn build_main_window(app: &tauri::AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    // Keep in sync with TRAFFIC_LIGHT_* in apps/desktop/src/lib/constants.ts.
    // Visual top/left inset are equal (aligns with expanded tab icons).
    // tao sets titlebar height to button_height + y and leaves button origin.y
    // at the native ~9pt, so y must be inset + 9 to get the desired top margin.
    #[cfg(target_os = "macos")]
    const TRAFFIC_LIGHT_INSET: f64 = 20.0;
    #[cfg(target_os = "macos")]
    const TRAFFIC_LIGHT_BUTTON_ORIGIN_Y: f64 = 9.0;

    let builder = tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::default())
        .title("tendi")
        .transparent(false)
        .decorations(true)
        .accept_first_mouse(true)
        .inner_size(1280.0, 820.0)
        .min_inner_size(920.0, 640.0)
        .visible(false);
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(LogicalPosition::new(
            TRAFFIC_LIGHT_INSET,
            TRAFFIC_LIGHT_INSET + TRAFFIC_LIGHT_BUTTON_ORIGIN_Y,
        ));
    let window = builder.build()?;
    init_main_window(&window);
    Ok(window)
}

#[cfg(target_os = "macos")]
fn init_main_window(_window: &tauri::WebviewWindow) {}

#[cfg(not(target_os = "macos"))]
fn init_main_window(_window: &tauri::WebviewWindow) {}

#[cfg(target_os = "macos")]
fn activate_native_window(window: &tauri::WebviewWindow) {
    use cocoa::{
        appkit::{NSApp, NSApplication, NSWindow},
        base::{YES, id, nil},
    };

    if let Ok(ns_window) = window.ns_window() {
        unsafe {
            let ns_window = ns_window as id;
            ns_window.makeKeyAndOrderFront_(nil);
            ns_window.orderFrontRegardless();
            NSApp().activateIgnoringOtherApps_(YES);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn activate_native_window(_window: &tauri::WebviewWindow) {}

pub fn run() {
    let mut app = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            app.set_activation_policy(ActivationPolicy::Regular);
            app.manage(SessionScanRuntime::new(app.handle().clone()));
            let window = build_main_window(app.handle())?;
            let _ = window.center();
            let _ = window.show();
            let _ = window.set_focus();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            scan,
            bundled_skill_status,
            bundled_skill_install,
            bundled_skill_prompt_dismiss,
            cli_status,
            cli_install,
            cli_remove,
            agents_list,
            agent_configs_list,
            agent_config_read,
            agent_config_save,
            config_profile_create,
            config_profile_set,
            skills_list,
            skills_refresh,
            sessions_list,
            sessions_project_merge,
            sessions_project_split,
            sessions_scan_start,
            analytics_overview,
            analytics_revision,
            session_analytics,
            sessions_search,
            session_skill_index_status,
            session_skill_index_run,
            session_skill_links,
            skill_session_links,
            settings_get,
            settings_save,
            terminal_apps_list,
            terminal_app_test,
            editor_app_test,
            session_resume_in_terminal,
            rules_list,
            rule_file_read,
            rule_file_save,
            hooks_list,
            hook_delete,
            hook_delete_many,
            hook_set_enabled,
            hook_review,
            hook_source_read,
            mcp_list,
            prompts_list,
            prompt_save,
            prompts_delete_many,
            session_transcript,
            skills_set,
            skills_wrap,
            skills_updates,
            skills_update,
            skills_update_many,
            skills_targets,
            skills_add,
            skills_delete_many,
            skill_files,
            skill_file_read,
            skill_file_save,
            skill_file_create,
            skill_folder_create,
            skill_path_rename,
            skill_path_delete,
            reveal_in_finder,
            open_in_editor,
            open_url,
            check_for_updates
        ])
        .build(tauri::generate_context!())
        .expect("error while building tendi desktop");
    app.set_activation_policy(ActivationPolicy::Regular);
    app.run(|app, event| {
        if let RunEvent::Ready = event {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                activate_native_window(&window);
                let _ = window.set_focus();
            }
        }
    });
}

fn config_profile_key(agent: AgentKind) -> Option<&'static str> {
    match agent {
        AgentKind::Codex => Some("codex"),
        AgentKind::Claude => Some("claude"),
        AgentKind::Cursor => Some("cursor"),
        AgentKind::Shared | AgentKind::Unknown => None,
    }
}

fn parse_agent(value: &str) -> AgentKind {
    let normalized = value
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>();
    match normalized.as_str() {
        "codex" => AgentKind::Codex,
        "cursor" => AgentKind::Cursor,
        "claude" | "claudecode" => AgentKind::Claude,
        "shared" => AgentKind::Shared,
        _ => AgentKind::Unknown,
    }
}

fn parse_agent_result(value: &str) -> Result<AgentKind, String> {
    match parse_agent(value) {
        AgentKind::Unknown => Err(format!("unknown agent target {value}")),
        agent => Ok(agent),
    }
}

fn absolute_project_path(value: Option<String>) -> Result<Option<PathBuf>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        Ok(Some(path))
    } else {
        Err(format!("workspace path is not absolute: {trimmed}"))
    }
}

fn active_cwd() -> Result<PathBuf, String> {
    match env::var_os("TENDI_CWD") {
        Some(value) if !value.is_empty() => Ok(PathBuf::from(value)),
        _ => env::current_dir().map_err(|err| err.to_string()),
    }
}

fn parse_visibility(value: &str) -> Result<tendi_core::SkillVisibility, String> {
    match value.to_ascii_lowercase().as_str() {
        "auto" => Ok(tendi_core::SkillVisibility::Auto),
        "manual" => Ok(tendi_core::SkillVisibility::Manual),
        "off" => Ok(tendi_core::SkillVisibility::Off),
        _ => Err(format!("unknown visibility {value}")),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use tendi_core::skills::{
        InstallableSkill, SkillAddOptions, SkillAddPlan, SkillDeletePlan, SkillScan,
        SkillUpdateReport,
    };

    use super::{
        cached_or_load_skill_scan, cached_skill_dir, editor_file_args, normalize_editor,
        resolve_terminal, session_root_priority, skill_update_event_for_revision,
        store_skill_add_preview, store_skill_delete_preview, take_skill_add_preview,
        take_skill_delete_preview, terminal_application_name,
    };

    #[test]
    fn skill_write_cache_miss_loads_authoritative_snapshot_once() {
        let cwd = PathBuf::from(format!(
            "/tmp/tendi-skill-cache-miss-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let loads = Cell::new(0);
        let load = || {
            loads.set(loads.get() + 1);
            Ok(SkillScan {
                roots: Vec::new(),
                skills: Vec::new(),
                warnings: Vec::new(),
            })
        };

        cached_or_load_skill_scan(&cwd, load).unwrap();
        cached_or_load_skill_scan(&cwd, load).unwrap();

        assert_eq!(loads.get(), 1);
    }

    #[test]
    fn skill_add_preview_is_reused_once_and_rejects_changed_source() {
        let root = std::env::temp_dir().join(format!(
            "tendi-add-preview-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let skill_dir = root.join("demo");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: demo\n---\n").unwrap();
        let options = SkillAddOptions {
            source: root.display().to_string(),
            target: tendi_core::AgentKind::Shared.into(),
            scope: tendi_core::SkillInstallScope::Global,
            skills: vec!["demo".to_string()],
            copy: true,
            overwrite: false,
            visibility: tendi_core::SkillVisibility::Auto,
        };
        let skill = InstallableSkill {
            name: "demo".to_string(),
            description: None,
            path: skill_dir,
            relative_path: "demo".to_string(),
            dependencies: Vec::new(),
        };
        let plan = SkillAddPlan {
            source: options.source.clone(),
            source_kind: "local".to_string(),
            source_root: root.clone(),
            source_ref: None,
            target: options.target.clone(),
            scope: options.scope,
            mode: "copy".to_string(),
            available: vec![skill.clone()],
            selected: vec![skill],
            operations: Vec::new(),
        };

        let id = store_skill_add_preview(&options, plan.clone()).unwrap();
        assert!(take_skill_add_preview(&id, &options).is_ok());
        assert!(take_skill_add_preview(&id, &options).is_err());

        let id = store_skill_add_preview(&options, plan).unwrap();
        fs::write(
            root.join("demo/SKILL.md"),
            "---\nname: demo\ndescription: changed\n---\n",
        )
        .unwrap();
        assert!(take_skill_add_preview(&id, &options).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_skill_update_check_does_not_publish_old_skill_rows() {
        let event = skill_update_event_for_revision(
            Vec::new(),
            vec![SkillUpdateReport {
                name: "deleted-during-check".to_string(),
                status: "up-to-date".to_string(),
                current_version: None,
                latest_version: None,
                source: None,
                source_kind: "local".to_string(),
            }],
            7,
            8,
        );

        assert!(event.skills.is_none());
        assert!(event.updates.is_empty());
    }

    #[test]
    fn stale_skill_dir_cache_is_rejected() {
        let root = std::env::temp_dir().join(format!(
            "tendi-stale-skill-dir-cache-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let skill_dir = root.join("cached");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: another-skill\n---\n",
        )
        .unwrap();
        super::SKILL_DIR_CACHE
            .lock()
            .unwrap()
            .insert((root.clone(), "expected-skill".to_string()), skill_dir);

        assert!(cached_skill_dir(&root, "expected-skill").is_none());
        assert!(
            !super::SKILL_DIR_CACHE
                .lock()
                .unwrap()
                .contains_key(&(root.clone(), "expected-skill".to_string()))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_scan_prioritizes_live_roots_before_archives() {
        assert_eq!(
            session_root_priority(Path::new("/Users/dev/.codex/sessions")),
            0
        );
        assert_eq!(
            session_root_priority(Path::new("/Users/dev/.cursor/projects")),
            1
        );
        assert_eq!(
            session_root_priority(Path::new("/Users/dev/.claude/projects")),
            2
        );
        assert_eq!(
            session_root_priority(Path::new("/Users/dev/.codex/archived_sessions")),
            4
        );
    }

    #[test]
    fn delete_preview_can_only_be_applied_once_to_the_same_selection() {
        let names = vec!["alpha".to_string()];
        let id = store_skill_delete_preview(
            &names,
            SkillDeletePlan {
                targets: Vec::new(),
                dependencies: Vec::new(),
                dependents: Vec::new(),
            },
        )
        .unwrap();

        assert!(take_skill_delete_preview(&id, &names).is_ok());
        assert!(take_skill_delete_preview(&id, &names).is_err());
    }

    #[test]
    fn terminal_resolution_preserves_custom_application_names() {
        assert_eq!(resolve_terminal(" CustomTerm "), "CustomTerm");
    }

    #[test]
    fn terminal_resolution_normalizes_known_application_names() {
        assert_eq!(resolve_terminal("iTerm2"), "iterm");
        assert_eq!(resolve_terminal("GHOSTTY"), "ghostty");
        assert_eq!(resolve_terminal("Warp"), "warp");
    }

    #[test]
    fn terminal_application_names_match_macos_apps() {
        assert_eq!(terminal_application_name("auto"), "Terminal");
        assert_eq!(terminal_application_name("iterm"), "iTerm");
        assert_eq!(terminal_application_name("CustomTerm"), "CustomTerm");
    }

    #[test]
    fn editor_resolution_normalizes_known_presets_and_preserves_custom_commands() {
        assert_eq!(normalize_editor("VS Code"), "vscode");
        assert_eq!(normalize_editor("Cot Editor"), "coteditor");
        assert_eq!(normalize_editor("my-editor"), "my-editor");
    }

    #[test]
    fn editor_file_args_preserve_line_navigation_for_each_preset() {
        let path = Path::new("/tmp/config.json");
        assert_eq!(
            editor_file_args("vscode", path, Some(7)),
            vec!["-g".to_string(), "/tmp/config.json:7".to_string()]
        );
        assert_eq!(
            editor_file_args("zed", path, Some(7)),
            vec!["/tmp/config.json:7".to_string()]
        );
        assert_eq!(
            editor_file_args("coteditor", path, Some(7)),
            vec![
                "--line".to_string(),
                "7".to_string(),
                "/tmp/config.json".to_string()
            ]
        );
        assert_eq!(
            editor_file_args("cursor", path, None),
            vec!["--goto".to_string(), "/tmp/config.json".to_string()]
        );
    }
}
