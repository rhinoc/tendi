use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
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
use tauri_plugin_updater::UpdaterExt;
#[cfg(target_os = "macos")]
use tauri::{LogicalPosition, TitleBarStyle};
use tendi_core::AgentKind;

static SESSION_SKILL_INDEX_RUNNING: AtomicBool = AtomicBool::new(false);
static SESSION_ANALYTICS_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static SKILL_DELETE_PREVIEW_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static SKILL_DELETE_PREVIEW: LazyLock<Mutex<Option<SkillDeletePreview>>> =
    LazyLock::new(|| Mutex::new(None));
static SKILL_DIR_CACHE: LazyLock<Mutex<BTreeMap<(PathBuf, String), PathBuf>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

const SESSION_SCAN_EVENT: &str = "sessions://scan";
const ANALYTICS_PROGRESS_EVENT: &str = "analytics://progress";
const SESSION_SCAN_BATCH_SIZE: usize = 32;
const SESSION_WATCH_DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCheckResult {
    status: &'static str,
    version: Option<String>,
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
        let generation = Arc::new(AtomicU64::new(0));
        let scan_running = Arc::new(AtomicBool::new(false));
        let watcher = Arc::new(Mutex::new(SessionWatcherState::default()));
        let worker_generation = Arc::clone(&generation);
        let worker_scan_running = Arc::clone(&scan_running);
        let worker_watcher = Arc::clone(&watcher);
        std::thread::spawn(move || {
            session_watch_loop(
                app,
                watch_rx,
                worker_generation,
                worker_scan_running,
                worker_watcher,
            )
        });
        Self {
            generation,
            scan_running,
            watcher,
            watch_tx,
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
    emit_analytics_progress(app, phase, initial, None);
    let _guard = match SESSION_ANALYTICS_LOCK.lock() {
        Ok(guard) => guard,
        Err(_) => {
            let message = "session analytics worker is unavailable".to_string();
            emit_analytics_progress(app, phase, initial, Some(message.clone()));
            return Err(message);
        }
    };
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

struct SkillDeletePreview {
    id: String,
    names: BTreeSet<String>,
    plan: tendi_core::skills::SkillDeletePlan,
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

fn cached_skill_dir(cwd: &Path, name: &str) -> Option<PathBuf> {
    SKILL_DIR_CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.get(&(cwd.to_path_buf(), name.to_string())).cloned())
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
        let report =
            tendi_core::skills::scan_skills_synced(&cwd).map_err(|err| format!("{err:#}"))?;
        replace_skill_dir_cache(&cwd, &report.skills);
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        store
            .save_skills(&report)
            .map_err(|err| format!("{err:#}"))?;
        Ok(report.skills)
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
        store
            .merge_session_projects(&target_project_id, &source_project_ids)
            .map_err(|err| format!("{err:#}"))?;
        list_resolved_sessions(&store)
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
        store
            .split_sessions_into_project(&name, &identities)
            .map_err(|err| format!("{err:#}"))?;
        list_resolved_sessions(&store)
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
        let sessions = store
            .list_sessions()
            .map_err(|err| format!("{err:#}"))?
            .sessions
            .into_iter()
            .filter(|session| agent.is_none_or(|agent| session.agent == agent))
            .collect::<Vec<_>>();
        let refresh_report = if refresh_transcripts.unwrap_or(false) {
            Some(refresh_session_analytics_serialized(
                &app, "overview", &store, &sessions,
            )?)
        } else {
            None
        };
        let mut overview = store
            .overview_analytics(agent, days.unwrap_or(182), rank_days.unwrap_or(30))
            .map_err(|err| format!("{err:#}"))?;
        overview.coverage.total_sessions = sessions.len();
        for provider in sessions
            .iter()
            .map(|session| session.agent)
            .collect::<BTreeSet<_>>()
        {
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
        Ok(overview)
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
        store
            .cached_session_analytics_detail(&session)
            .map_err(|err| format!("{err:#}"))
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
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = run_session_scan(&app, &cwd, &additional_session_roots, generation) {
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
        process_session_watch_paths(&app, generation.load(Ordering::SeqCst), &paths);
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

fn process_session_watch_paths(app: &tauri::AppHandle, generation: u64, paths: &[PathBuf]) {
    let result = (|| -> Result<(Vec<tendi_core::SessionRecord>, Vec<tendi_core::sessions::SessionIdentity>), String> {
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
        let changed = cache.changed_sessions(&report.sessions);
        let mut upserts = store
            .apply_session_delta(&changed)
            .map_err(|err| format!("{err:#}"))?;
        store
            .resolve_session_projects(&mut upserts)
            .map_err(|err| format!("{err:#}"))?;
        let deleted = store
            .remove_sessions_for_paths(&deleted_paths)
            .map_err(|err| format!("{err:#}"))?;
        Ok((upserts, deleted))
    })();
    match result {
        Ok((upserts, deleted)) if !upserts.is_empty() || !deleted.is_empty() => {
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
    terminal: String,
    additional_session_roots: Vec<String>,
    config_profiles: BTreeMap<String, String>,
) -> Result<serde_json::Value, String> {
    blocking_json(move || {
        let store = tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
        store
            .save_app_settings(tendi_core::storage::AppSettings {
                appearance,
                terminal,
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
        Ok(report.rules)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn rule_file_read(path: String) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        tendi_core::rules::read_rule_file(&cwd, std::path::Path::new(&path))
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
        tendi_core::rules::save_rule_file(
            &cwd,
            std::path::Path::new(&path),
            &expected_sha256,
            &content,
        )
        .map_err(|err| format!("{err:#}"))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn hooks_list() -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let report = tendi_core::hooks::scan_hooks(&cwd).map_err(|err| format!("{err:#}"))?;
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
        tendi_core::hooks::read_hook_source(
            &cwd,
            std::path::Path::new(&path),
            expected_trust_hash.as_deref(),
            hook_match.as_ref(),
        )
        .map_err(|err| format!("{err:#}"))
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
async fn session_transcript(path: String, agent: String) -> Result<serde_json::Value, String> {
    let agent = parse_agent(&agent);
    blocking_json(move || {
        let transcript =
            tendi_core::transcript::parse_transcript(std::path::Path::new(&path), agent)
                .map_err(|err| format!("{err:#}"))?;
        Ok(transcript.items)
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
        let changeset = tendi_core::skills::plan_visibility_many(&cwd, &names, visibility)
            .map_err(|err| format!("{err:#}"))?;
        let summary = tendi_core::skills::format_changeset(&changeset);
        if !dry_run {
            tendi_core::skills::apply_changes(&changeset).map_err(|err| format!("{err:#}"))?;
        }
        Ok(serde_json::json!({
            "summary": summary,
            "applied": !dry_run,
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
        let changeset = if refresh {
            tendi_core::skills::refresh_wrapper_from_names(&cwd, &name, &names, manual_children)
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
        Ok(serde_json::json!({
            "summary": summary,
            "applied": !dry_run,
        }))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skills_updates(check: bool) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        if check {
            let updates =
                tendi_core::skills::check_skill_updates(&cwd).map_err(|err| format!("{err:#}"))?;
            serde_json::to_value(updates).map_err(|err| err.to_string())
        } else {
            let report =
                tendi_core::skills::scan_skills_synced(&cwd).map_err(|err| format!("{err:#}"))?;
            let store =
                tendi_core::storage::Store::open_default().map_err(|err| format!("{err:#}"))?;
            store
                .save_skills(&report)
                .map_err(|err| format!("{err:#}"))?;
            serde_json::to_value(report.skills).map_err(|err| err.to_string())
        }
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skills_update(pattern: String, dry_run: bool) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let plan = tendi_core::skills::plan_skill_updates(&cwd, &pattern)
            .map_err(|err| format!("{err:#}"))?;
        let summary = tendi_core::skills::format_update_plan(&plan);
        if !dry_run {
            tendi_core::skills::apply_skill_update_plan(&plan).map_err(|err| format!("{err:#}"))?;
        }
        Ok(serde_json::json!({
            "summary": summary,
            "applied": !dry_run,
            "plan": plan,
        }))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skills_update_many(
    names: Vec<String>,
    dry_run: bool,
) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    blocking_json(move || {
        let plan = tendi_core::skills::plan_skill_updates_many(&cwd, &names)
            .map_err(|err| format!("{err:#}"))?;
        let summary = tendi_core::skills::format_update_plan(&plan);
        if !dry_run {
            tendi_core::skills::apply_skill_update_plan(&plan).map_err(|err| format!("{err:#}"))?;
        }
        Ok(serde_json::json!({
            "summary": summary,
            "applied": !dry_run,
            "plan": plan,
        }))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skills_add(
    source: String,
    target: String,
    skills: Vec<String>,
    copy: bool,
    overwrite: bool,
    dry_run: bool,
) -> Result<serde_json::Value, String> {
    let cwd = active_cwd()?;
    let target = parse_agent_result(&target)?;
    blocking_json(move || {
        let options = tendi_core::skills::SkillAddOptions {
            source,
            target,
            skills,
            copy,
            overwrite,
        };
        if dry_run {
            let plan = tendi_core::skills::plan_skill_add(&cwd, &options)
                .map_err(|err| format!("{err:#}"))?;
            return Ok(serde_json::json!({
                "applied": false,
                "plan": plan,
            }));
        }
        let report = tendi_core::skills::apply_skill_add(&cwd, &options)
            .map_err(|err| format!("{err:#}"))?;
        Ok(serde_json::json!({
            "applied": true,
            "report": report.clone(),
            "plan": report.plan,
            "results": report.results,
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
        let plan = if dry_run {
            tendi_core::skills::plan_skill_delete_many(&cwd, &names)
                .map_err(|err| format!("{err:#}"))?
        } else if let Some(preview_id) = preview_id.as_deref() {
            take_skill_delete_preview(preview_id, &names)?
        } else {
            tendi_core::skills::plan_skill_delete_many(&cwd, &names)
                .map_err(|err| format!("{err:#}"))?
        };
        let summary = tendi_core::skills::format_delete_plan(&plan);
        let stored_preview_id = if dry_run {
            Some(store_skill_delete_preview(&names, plan.clone())?)
        } else {
            None
        };
        if !dry_run {
            tendi_core::skills::apply_skill_delete_plan(&plan).map_err(|err| format!("{err:#}"))?;
        }
        Ok(serde_json::json!({
            "summary": summary,
            "applied": !dry_run,
            "plan": plan,
            "previewId": stored_preview_id,
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
        let result = tendi_core::files::save_skill_file(
            &cwd,
            &name,
            &relative_path,
            &expected_sha256,
            &content,
            cached_skill_dir.as_deref(),
        )
        .map_err(|err| format!("{err:#}"))?;
        Ok(result)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn skill_file_create(
    name: String,
    relative_path: String,
) -> Result<serde_json::Value, String> {
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

    let path_text = path.to_string_lossy();
    let goto_target = match line.filter(|value| *value > 0) {
        Some(line) => format!("{path_text}:{line}"),
        None => path_text.to_string(),
    };

    let editors = [
        ("cursor", vec!["--goto", goto_target.as_str()]),
        ("code", vec!["-g", goto_target.as_str()]),
    ];
    for (program, args) in editors {
        match Command::new(program).args(args).status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(_) => continue,
            Err(_) => continue,
        }
    }

    #[cfg(target_os = "macos")]
    let fallback = Command::new("open").arg(&path).status();

    #[cfg(target_os = "windows")]
    let fallback = Command::new("cmd")
        .args(["/C", "start", "", &path.to_string_lossy()])
        .status();

    #[cfg(all(unix, not(target_os = "macos")))]
    let fallback = Command::new("xdg-open").arg(&path).status();

    match fallback {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("open command exited with {status}")),
        Err(err) => Err(format!("failed to open path in editor: {err}")),
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
        base::{id, nil},
    };

    if let Ok(ns_window) = window.ns_window() {
        unsafe {
            let ns_window = ns_window as id;
            ns_window.makeKeyAndOrderFront_(nil);
            ns_window.orderFrontRegardless();
            NSApp().activateIgnoringOtherApps_(true);
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
            agents_list,
            agent_configs_list,
            agent_config_read,
            agent_config_save,
            config_profile_create,
            config_profile_set,
            skills_list,
            sessions_list,
            sessions_project_merge,
            sessions_project_split,
            sessions_scan_start,
            analytics_overview,
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
            session_resume_in_terminal,
            rules_list,
            rule_file_read,
            rule_file_save,
            hooks_list,
            hook_delete,
            hook_set_enabled,
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
    use std::path::Path;

    use tendi_core::skills::SkillDeletePlan;

    use super::{
        resolve_terminal, session_root_priority, store_skill_delete_preview,
        take_skill_delete_preview, terminal_application_name,
    };

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
}
