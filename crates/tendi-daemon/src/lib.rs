use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const SESSION_SCAN_EVENT: &str = "sessions://scan";
pub const ANALYTICS_PROGRESS_EVENT: &str = "analytics://progress";
pub const ANALYTICS_REVISION_EVENT: &str = "analytics://revision";
pub const SKILL_UPDATE_EVENT: &str = "skills://updates";
pub const CONFIG_CHANGED_EVENT: &str = "config://changed";
const SESSION_SCAN_BATCH_SIZE: usize = 32;
const SESSION_WATCH_DEBOUNCE: Duration = Duration::from_millis(500);
const CONFIG_WATCH_DEBOUNCE: Duration = Duration::from_millis(150);
const BACKUP_SYNC_INTERVAL: Duration = Duration::from_secs(10 * 60);
const DATABASE_WRITE_LOCK_ATTEMPTS: usize = 100;
const DATABASE_WRITE_LOCK_RETRY: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Deserialize)]
pub struct DaemonRequest {
    pub command: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct DaemonError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl DaemonError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            data: None,
        }
    }

    fn with_data(code: impl Into<String>, message: impl Into<String>, data: Value) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            data: Some(data),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DaemonResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DaemonError>,
}

impl DaemonResponse {
    fn success(result: Value) -> Self {
        Self {
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    fn failure(error: DaemonError) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonEvent {
    pub id: u64,
    pub event: String,
    pub payload: Value,
}

#[derive(Debug, Clone)]
struct EventHub {
    next_id: Arc<AtomicU64>,
    subscribers: Arc<Mutex<Vec<Sender<DaemonEvent>>>>,
}

#[derive(Debug)]
pub struct DaemonEventSubscription {
    receiver: Receiver<DaemonEvent>,
}

impl DaemonEventSubscription {
    pub fn recv_timeout(&self, timeout: Duration) -> Result<DaemonEvent, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }
}

impl EventHub {
    fn subscribe(&self) -> DaemonEventSubscription {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.push(sender);
        }
        DaemonEventSubscription { receiver }
    }

    fn publish(&self, event: &str, payload: Value) {
        let event = DaemonEvent {
            id: self.next_id.fetch_add(1, Ordering::Relaxed) + 1,
            event: event.to_string(),
            payload,
        };
        let Ok(mut subscribers) = self.subscribers.lock() else {
            return;
        };
        subscribers.retain(|subscriber| subscriber.send(event.clone()).is_ok());
    }
}

#[derive(Debug, Clone)]
struct AnalyticsRefreshJob {
    phase: &'static str,
    sessions: Vec<tendi_core::SessionRecord>,
}

#[derive(Default, Debug)]
struct SessionWatcherState {
    watcher: Option<RecommendedWatcher>,
    watched_paths: BTreeSet<PathBuf>,
    dynamic_roots: Vec<PathBuf>,
}

#[derive(Debug)]
struct SessionRuntime {
    generation: AtomicU64,
    scan_running: AtomicBool,
    watch_revision: AtomicU64,
    completed_revision: AtomicU64,
    watcher: Mutex<SessionWatcherState>,
    watch_tx: Sender<notify::Result<Event>>,
    analytics_tx: Sender<AnalyticsRefreshJob>,
}

#[derive(Default, Debug)]
struct ConfigWatcherState {
    watcher: Option<RecommendedWatcher>,
    watched_paths: BTreeSet<PathBuf>,
    watched_dirs: BTreeSet<PathBuf>,
}

#[derive(Debug)]
struct ConfigRuntime {
    watcher: Mutex<ConfigWatcherState>,
    watch_tx: Sender<notify::Result<Event>>,
}

#[derive(Debug)]
struct SkillAddPreview {
    id: String,
    options: tendi_core::skills::SkillAddOptions,
    plan: tendi_core::skills::SkillAddPlan,
}

#[derive(Debug)]
struct SkillUpdatePreview {
    id: String,
    names: Vec<String>,
    plan: tendi_core::skills::SkillUpdatePlan,
}

#[derive(Debug)]
struct SkillDistributionPreview {
    id: String,
    sources: Vec<PathBuf>,
    target: tendi_core::SkillTarget,
    scope: tendi_core::SkillInstallScope,
    plans: Vec<tendi_core::skills::SkillDistributionPlan>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HookInput {
    agent: tendi_core::AgentKind,
    path: String,
    #[serde(default)]
    expected_trust_hash: String,
    #[serde(default)]
    event: Option<String>,
    matcher: Option<String>,
    hook_type: Option<String>,
    command: Option<String>,
    url: Option<String>,
    prompt: Option<String>,
    filter: Option<String>,
    status_message: Option<String>,
    enabled: Option<bool>,
}

impl HookInput {
    fn required_event(&self) -> Result<&str, DaemonError> {
        self.event
            .as_deref()
            .filter(|event| !event.trim().is_empty())
            .ok_or_else(|| invalid_argument("missing or empty argument: event"))
    }

    fn required_trust_hash(&self) -> Result<&str, DaemonError> {
        if self.expected_trust_hash.trim().is_empty() {
            return Err(invalid_argument(
                "missing or empty argument: expectedTrustHash",
            ));
        }
        Ok(&self.expected_trust_hash)
    }

    fn delete_request(self) -> Result<tendi_core::hooks::HookDeleteRequest, DaemonError> {
        let event = self.required_event()?.to_string();
        let expected_trust_hash = self.required_trust_hash()?.to_string();
        Ok(tendi_core::hooks::HookDeleteRequest {
            agent: self.agent,
            path: PathBuf::from(self.path),
            expected_trust_hash,
            event,
            matcher: self.matcher,
            hook_type: self.hook_type,
            command: self.command,
            url: self.url,
            prompt: self.prompt,
            filter: self.filter,
            status_message: self.status_message,
        })
    }

    fn set_enabled_request(self) -> Result<tendi_core::hooks::HookSetEnabledRequest, DaemonError> {
        let event = self.required_event()?.to_string();
        let expected_trust_hash = self.required_trust_hash()?.to_string();
        let enabled = self
            .enabled
            .ok_or_else(|| invalid_argument("missing argument: enabled"))?;
        Ok(tendi_core::hooks::HookSetEnabledRequest {
            agent: self.agent,
            path: PathBuf::from(self.path),
            expected_trust_hash,
            event,
            matcher: self.matcher,
            hook_type: self.hook_type,
            command: self.command,
            url: self.url,
            prompt: self.prompt,
            filter: self.filter,
            status_message: self.status_message,
            enabled,
        })
    }

    fn review_request(self) -> Result<tendi_core::hooks::HookReviewRequest, DaemonError> {
        let event = self.required_event()?.to_string();
        let expected_trust_hash = self.required_trust_hash()?.to_string();
        Ok(tendi_core::hooks::HookReviewRequest {
            agent: self.agent,
            path: PathBuf::from(self.path),
            expected_trust_hash,
            event,
            matcher: self.matcher,
            hook_type: self.hook_type,
            command: self.command,
            url: self.url,
            prompt: self.prompt,
            filter: self.filter,
            status_message: self.status_message,
        })
    }
}

#[derive(Debug)]
struct DaemonState {
    cwd: PathBuf,
    events: EventHub,
    session_runtime: Arc<SessionRuntime>,
    config_runtime: Arc<ConfigRuntime>,
    skill_authority: Mutex<()>,
    control_authority: Mutex<()>,
    session_skill_index_running: AtomicBool,
    skill_update_running: AtomicBool,
    skill_update_cancelled: AtomicBool,
    backup_sync_running: AtomicBool,
    add_preview: Mutex<Option<SkillAddPreview>>,
    update_preview: Mutex<Option<SkillUpdatePreview>>,
    distribution_preview: Mutex<Option<SkillDistributionPreview>>,
    preview_sequence: Mutex<u64>,
}

#[derive(Clone, Debug)]
pub struct Daemon {
    state: Arc<DaemonState>,
}

impl Daemon {
    pub fn new(cwd: PathBuf) -> Self {
        let (watch_tx, watch_rx) = mpsc::channel();
        let (config_watch_tx, config_watch_rx) = mpsc::channel();
        let (analytics_tx, analytics_rx) = mpsc::channel();
        let events = EventHub {
            next_id: Arc::new(AtomicU64::new(0)),
            subscribers: Arc::new(Mutex::new(Vec::new())),
        };
        let session_runtime = Arc::new(SessionRuntime {
            generation: AtomicU64::new(0),
            scan_running: AtomicBool::new(false),
            watch_revision: AtomicU64::new(0),
            completed_revision: AtomicU64::new(0),
            watcher: Mutex::new(SessionWatcherState::default()),
            watch_tx,
            analytics_tx,
        });
        let config_runtime = Arc::new(ConfigRuntime {
            watcher: Mutex::new(ConfigWatcherState::default()),
            watch_tx: config_watch_tx,
        });
        let daemon = Self {
            state: Arc::new(DaemonState {
                cwd,
                events,
                session_runtime,
                config_runtime,
                skill_authority: Mutex::new(()),
                control_authority: Mutex::new(()),
                session_skill_index_running: AtomicBool::new(false),
                skill_update_running: AtomicBool::new(false),
                skill_update_cancelled: AtomicBool::new(false),
                backup_sync_running: AtomicBool::new(false),
                add_preview: Mutex::new(None),
                update_preview: Mutex::new(None),
                distribution_preview: Mutex::new(None),
                preview_sequence: Mutex::new(0),
            }),
        };
        let watch_daemon = daemon.clone();
        thread::spawn(move || session_watch_loop(watch_daemon, watch_rx));
        let analytics_daemon = daemon.clone();
        thread::spawn(move || session_analytics_loop(analytics_daemon, analytics_rx));
        let config_daemon = daemon.clone();
        thread::spawn(move || config_watch_loop(config_daemon, config_watch_rx));
        let backup_daemon = daemon.clone();
        thread::spawn(move || backup_sync_loop(backup_daemon));
        daemon.initialize_config_watcher();
        daemon.schedule_skill_backup();
        daemon
    }

    pub fn cwd(&self) -> &Path {
        &self.state.cwd
    }

    pub fn subscribe_events(&self) -> DaemonEventSubscription {
        self.state.events.subscribe()
    }

    fn emit_event<T: Serialize>(&self, event: &str, payload: T) {
        if let Ok(payload) = serde_json::to_value(payload) {
            self.state.events.publish(event, payload);
        }
    }

    pub fn handle_json(&self, value: Value) -> Value {
        let request = match serde_json::from_value::<DaemonRequest>(value) {
            Ok(request) if !request.command.trim().is_empty() => request,
            Ok(_) => {
                return serde_json::to_value(DaemonResponse::failure(DaemonError::new(
                    "INVALID_REQUEST",
                    "command must not be empty",
                )))
                .expect("daemon response serializes");
            }
            Err(error) => {
                return serde_json::to_value(DaemonResponse::failure(DaemonError::new(
                    "INVALID_REQUEST",
                    error.to_string(),
                )))
                .expect("daemon response serializes");
            }
        };

        let dispatch = || match self.dispatch(&request.command, &request.args) {
            Ok(result) => DaemonResponse::success(result),
            Err(error) => DaemonResponse::failure(error),
        };
        let response = dispatch();
        if !response.ok {
            tendi_core::logging::global().warn(
                "daemon request failed",
                json!({ "command": &request.command, "error": response.error.as_ref() }),
            );
        }
        let response = serde_json::to_value(response).expect("daemon response serializes");
        tendi_core::logging::global().debug(
            "daemon request handled",
            json!({
                "command": &request.command,
                "ok": response.get("ok").and_then(Value::as_bool).unwrap_or(false),
            }),
        );
        response
    }

    fn dispatch(&self, command: &str, args: &Value) -> Result<Value, DaemonError> {
        match command {
            "agents_list" => self.agents_list(),
            "overview_count" => self.overview_count(args),
            "bundled_skill_status" => self.bundled_skill_status(),
            "bundled_skill_install" => self.bundled_skill_install(args),
            "bundled_skill_remove" => self.bundled_skill_remove(),
            "bundled_skill_prompt_dismiss" => self.bundled_skill_prompt_dismiss(),
            "skills_list" => self.skills_list(),
            "skills_refresh" => self.skills_refresh(),
            "sessions_list" => self.sessions_list(),
            "sessions_scan_start" => self.sessions_scan_start(),
            "sessions_search" => self.sessions_search(args),
            "analytics_overview" => self.analytics_overview(args),
            "analytics_revision" => self.analytics_revision(),
            "session_skill_index_status" => self.session_skill_index_status(),
            "session_skill_index_run" => self.session_skill_index_run(args),
            "session_skill_links" => self.session_skill_links(args),
            "skill_session_links" => self.skill_session_links(args),
            "settings_get" => self.settings_get(),
            "settings_save" => self.settings_save(args),
            "session_projects_list" => self.session_projects_list(),
            "project_scan_scopes_list" => self.project_scan_scopes_list(),
            "project_scan_scopes_save" => self.project_scan_scopes_save(args),
            "projects_list" => self.projects_list(),
            "projects_scan" => self.projects_scan(),
            "terminal_apps_list" => self.terminal_apps_list(),
            "agent_configs_list" => self.agent_configs_list(),
            "agent_config_watch" => self.agent_config_watch(args),
            "agent_config_read" => self.agent_config_read(args),
            "agent_config_save" => self.agent_config_save(args),
            "config_profile_create" => self.config_profile_create(args),
            "config_profile_set" => self.config_profile_set(args),
            "rules_list" => self.rules_list(),
            "rule_file_read" => self.rule_file_read(args),
            "rule_file_save" => self.rule_file_save(args),
            "hooks_list" => self.hooks_list(),
            "hook_delete" => self.hook_delete(args),
            "hook_delete_many" => self.hook_delete_many(args),
            "hook_set_enabled" => self.hook_set_enabled(args),
            "hook_review" => self.hook_review(args),
            "hook_source_read" => self.hook_source_read(args),
            "mcp_list" => self.mcp_list(),
            "mcp_set_enabled" => self.mcp_set_enabled(args),
            "prompts_list" => self.prompts_list(),
            "prompt_save" => self.prompt_save(args),
            "prompts_delete_many" => self.prompts_delete_many(args),
            "session_transcript" => self.session_transcript(args),
            "session_transcript_locator" => self.session_transcript_locator(args),
            "session_transcript_search" => self.session_transcript_search(args),
            "skills_targets" => self.skills_targets(),
            "skills_backup_status" => self.skills_backup_status(),
            "skills_backup_configure" => self.skills_backup_configure(args),
            "skills_backup_sync" => self.skills_backup_sync(),
            "skills_backup_now" => self.skills_backup_now(),
            "skills_backup_versions" => self.skills_backup_versions(args),
            "skills_backup_restore" => self.skills_backup_restore(args),
            "skills_backup_adopt" => self.skills_backup_adopt(args),
            "skills_backup_disconnect" => self.skills_backup_disconnect(),
            "skills_add" => self.skills_add(args),
            "skills_add_preview_read" => self.skills_add_preview_read(args),
            "skills_distribute" => self.skills_distribute(args),
            "skills_remove_locations" => self.skills_remove_locations(args),
            "skills_set" => self.skills_set(args),
            "skills_wrap" => self.skills_wrap(args),
            "skills_updates" => self.skills_updates(args),
            "skills_updates_cancel" => self.skills_updates_cancel(),
            "skills_update" => self.skills_update(args),
            "skills_update_many" => self.skills_update_many(args),
            "skills_delete_many" => self.skills_delete_many(args),
            "skills_marketplace_search" => self.skills_marketplace_search(args),
            "skill_files" => self.skill_files(args),
            "skill_file_read" => self.skill_file_read(args),
            "skill_file_save" => self.skill_file_save(args),
            "skill_file_create" => self.skill_file_create(args),
            "skill_folder_create" => self.skill_folder_create(args),
            "skill_path_rename" => self.skill_path_rename(args),
            "skill_path_delete" => self.skill_path_delete(args),
            _ => Err(DaemonError::new(
                "METHOD_NOT_FOUND",
                format!("unsupported daemon method: {command}"),
            )),
        }
    }

    fn ensure_projection<T, Ready, Refresh>(
        &self,
        domain: &str,
        ready: Ready,
        mut refresh: Refresh,
    ) -> Result<T, DaemonError>
    where
        Ready: Fn(&tendi_core::storage::Store) -> anyhow::Result<Option<T>>,
        Refresh: FnMut(&tendi_core::storage::Store) -> anyhow::Result<T>,
    {
        for _ in 0..100 {
            let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
            if let Some(value) = ready(&store).map_err(core_error)? {
                return Ok(value);
            }
            if let Some(value) = store
                .with_projection_refresh_lock(domain, || refresh(&store))
                .map_err(core_error)?
            {
                return Ok(value);
            }
            thread::sleep(Duration::from_millis(50));
        }
        Err(internal_error(format!(
            "timed out waiting for {domain} projection refresh"
        )))
    }

    fn registered_project_roots(
        store: &tendi_core::storage::Store,
    ) -> anyhow::Result<Vec<PathBuf>> {
        Ok(store
            .list_projects()?
            .into_iter()
            .map(|project| project.root_path)
            .collect())
    }

    fn agents_list(&self) -> Result<Value, DaemonError> {
        let cwd = self.state.cwd.clone();
        let report = self.ensure_projection(
            "agents",
            |store| store.list_agents_for_workspace(&cwd),
            |store| {
                let report = tendi_core::agents::scan_agents(&cwd)?;
                with_database_write_lock(store, || store.save_agents_for_workspace(&cwd, &report))
                    .map_err(daemon_error_anyhow)?;
                Ok(report)
            },
        )?;
        serde_json::to_value(report.agents).map_err(internal_error)
    }

    fn overview_count(&self, args: &Value) -> Result<Value, DaemonError> {
        let domain = string_arg(args, "domain")?;
        let agent = optional_string_arg(args, "agent")
            .map(|value| parse_agent(&value))
            .transpose()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let count = match domain.as_str() {
            "sessions" => {
                tendi_core::storage::ProjectionCount {
                    rows: store.count_sessions(agent).map_err(core_error)?,
                    secondary: 0,
                }
            }
            "prompts" => {
                tendi_core::storage::ProjectionCount {
                    rows: store.count_prompts().map_err(core_error)?,
                    secondary: 0,
                }
            }
            "agents" | "skills" | "rules" | "hooks" | "mcp" => {
                let cwd = self.state.cwd.clone();
                match store
                    .count_projection_for_workspace(&domain, &cwd, agent)
                    .map_err(core_error)?
                {
                    Some(count) => count,
                    None => {
                        return Ok(json!({
                            "count": 0,
                            "secondaryCount": 0,
                            "ready": false,
                        }));
                    }
                }
            }
            _ => {
                return Err(invalid_argument(format!(
                    "unsupported overview count domain: {domain}"
                )));
            }
        };
        Ok(json!({
            "count": count.rows,
            "secondaryCount": count.secondary,
            "ready": true,
        }))
    }

    fn bundled_skill_status(&self) -> Result<Value, DaemonError> {
        serde_json::to_value(
            tendi_core::bundled_skill::status(tendi_core::AgentKind::Shared).map_err(core_error)?,
        )
        .map_err(internal_error)
    }

    fn bundled_skill_install(&self, args: &Value) -> Result<Value, DaemonError> {
        let overwrite = args
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        serde_json::to_value(
            tendi_core::bundled_skill::install(tendi_core::AgentKind::Shared, overwrite, false)
                .map_err(core_error)?,
        )
        .map_err(internal_error)
    }

    fn bundled_skill_remove(&self) -> Result<Value, DaemonError> {
        serde_json::to_value(
            tendi_core::bundled_skill::remove(tendi_core::AgentKind::Shared).map_err(core_error)?,
        )
        .map_err(internal_error)
    }

    fn bundled_skill_prompt_dismiss(&self) -> Result<Value, DaemonError> {
        tendi_core::bundled_skill::dismiss_prompt().map_err(core_error)?;
        Ok(Value::Null)
    }

    fn terminal_apps_list(&self) -> Result<Value, DaemonError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct TerminalApp {
            id: &'static str,
            label: &'static str,
            available: bool,
        }

        fn app_available(paths: &[&str]) -> bool {
            paths.iter().any(|path| Path::new(path).exists())
        }

        let apps = vec![
            TerminalApp {
                id: "auto",
                label: "Auto",
                available: true,
            },
            TerminalApp {
                id: "terminal",
                label: "Terminal",
                available: app_available(&[
                    "/System/Applications/Utilities/Terminal.app",
                    "/Applications/Utilities/Terminal.app",
                ]),
            },
            TerminalApp {
                id: "iterm",
                label: "iTerm",
                available: app_available(&["/Applications/iTerm.app", "/Applications/iTerm2.app"]),
            },
            TerminalApp {
                id: "ghostty",
                label: "Ghostty",
                available: app_available(&["/Applications/Ghostty.app"]),
            },
            TerminalApp {
                id: "warp",
                label: "Warp",
                available: app_available(&["/Applications/Warp.app"]),
            },
            TerminalApp {
                id: "orca",
                label: "Orca",
                available: app_available(&["/Applications/Orca.app"]),
            },
        ];
        serde_json::to_value(apps).map_err(internal_error)
    }

    fn sessions_list(&self) -> Result<Value, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        match store.list_sessions() {
            Ok(report) => serde_json::to_value(report.sessions).map_err(internal_error),
            Err(error) => Err(core_error(error)),
        }
    }

    fn sessions_scan_start(&self) -> Result<Value, DaemonError> {
        let additional_session_roots = {
            let _authority = self.control_authority()?;
            let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
            store
                .app_settings()
                .map_err(core_error)?
                .additional_session_roots
                .into_iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        };
        let watch_plan =
            tendi_core::sessions::session_watch_plan(&self.state.cwd, &additional_session_roots);
        self.configure_session_watcher(&watch_plan)?;
        let runtime = &self.state.session_runtime;
        if runtime.scan_running.load(Ordering::SeqCst) {
            return Ok(session_scan_start_response(
                runtime.generation.load(Ordering::SeqCst),
                false,
            ));
        }
        let observed_revision = runtime.watch_revision.load(Ordering::Acquire);
        let completed_revision = runtime.completed_revision.load(Ordering::Acquire);
        let current_generation = runtime.generation.load(Ordering::SeqCst);
        if session_scan_is_current(current_generation, observed_revision, completed_revision) {
            return Ok(session_scan_start_response(current_generation, false));
        }
        if runtime
            .scan_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(session_scan_start_response(
                runtime.generation.load(Ordering::SeqCst),
                false,
            ));
        }
        let generation = runtime.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let scan_revision = observed_revision;
        let daemon = self.clone();
        thread::spawn(move || {
            let result = run_session_scan(&daemon, generation, &additional_session_roots);
            if let Err(error) = &result {
                tendi_core::logging::global().error(
                    "session scan failed",
                    json!({
                        "generation": generation,
                        "code": &error.code,
                        "error": &error.message,
                    }),
                );
            }
            if result.is_ok() {
                daemon
                    .state
                    .session_runtime
                    .completed_revision
                    .store(scan_revision, Ordering::Release);
            } else if let Err(error) = result {
                daemon.emit_event(
                    SESSION_SCAN_EVENT,
                    json!({
                        "generation": generation,
                        "phase": "error",
                        "upserts": [],
                        "deleted": [],
                        "scanned": 0,
                        "complete": true,
                        "error": error.message,
                    }),
                );
            }
            daemon
                .state
                .session_runtime
                .scan_running
                .store(false, Ordering::SeqCst);
        });
        Ok(session_scan_start_response(generation, true))
    }

    fn sessions_search(&self, args: &Value) -> Result<Value, DaemonError> {
        let query = string_arg(args, "query")?;
        let candidates = args
            .get("candidates")
            .map(|value| {
                serde_json::from_value::<Vec<tendi_core::sessions::SessionIdentity>>(value.clone())
                    .map_err(|error| invalid_argument(format!("invalid session search candidates: {error}")))
            })
            .transpose()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let mut hits = match candidates.as_deref() {
            Some(candidates) => store
                .search_sessions_batch(&query, candidates)
                .map_err(core_error)?,
            None => store.search_sessions(&query).map_err(core_error)?,
        };
        let mut sessions = hits
            .iter()
            .map(|hit| hit.session.clone())
            .collect::<Vec<_>>();
        store
            .resolve_session_projects(&mut sessions)
            .map_err(core_error)?;
        for (hit, session) in hits.iter_mut().zip(sessions) {
            hit.session = session;
        }
        serde_json::to_value(hits).map_err(internal_error)
    }

    fn analytics_overview(&self, args: &Value) -> Result<Value, DaemonError> {
        let agent = optional_string_arg(args, "agent")
            .map(|value| parse_agent(&value))
            .transpose()?;
        let days = args.get("days").and_then(Value::as_u64).unwrap_or(182) as u32;
        let rank_days = args.get("rankDays").and_then(Value::as_u64).unwrap_or(30) as u32;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let refresh_transcripts = bool_arg(args, "refreshTranscripts");
        if refresh_transcripts {
            let sessions = store
                .list_sessions()
                .map_err(core_error)?
                .sessions
                .into_iter()
                .filter(|session| agent.is_none_or(|expected| session.agent == expected))
                .collect::<Vec<_>>();
            let _ = self
                .state
                .session_runtime
                .analytics_tx
                .send(AnalyticsRefreshJob {
                    phase: "manual",
                    sessions,
                });
        }
        let overview = store
            .overview_analytics(agent, days, rank_days)
            .map_err(core_error)?;
        serde_json::to_value(overview).map_err(internal_error)
    }

    fn analytics_revision(&self) -> Result<Value, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        Ok(json!(store.analytics_revision().map_err(core_error)?))
    }

    fn session_skill_index_status(&self) -> Result<Value, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let status = store
            .session_skill_index_status(
                self.state
                    .session_skill_index_running
                    .load(Ordering::Acquire),
            )
            .map_err(core_error)?;
        serde_json::to_value(status).map_err(internal_error)
    }

    fn session_skill_index_run(&self, args: &Value) -> Result<Value, DaemonError> {
        if self
            .state
            .session_skill_index_running
            .swap(true, Ordering::AcqRel)
        {
            return self.session_skill_index_status();
        }
        let force = bool_arg(args, "force");
        let result = tendi_core::session_skills::run_index(&self.state.cwd, force)
            .map_err(core_error);
        self.state
            .session_skill_index_running
            .store(false, Ordering::Release);
        result?;
        self.session_skill_index_status()
    }

    fn session_skill_links(&self, args: &Value) -> Result<Value, DaemonError> {
        let session_id = string_arg(args, "sessionId")?;
        let agent = parse_agent(&string_arg(args, "agent")?)?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        serde_json::to_value(
            store
                .session_skill_links(&session_id, agent)
                .map_err(core_error)?,
        )
        .map_err(internal_error)
    }

    fn skill_session_links(&self, args: &Value) -> Result<Value, DaemonError> {
        let name = string_arg(args, "skillName")?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        serde_json::to_value(store.skill_session_links(&name).map_err(core_error)?)
            .map_err(internal_error)
    }

    fn settings_get(&self) -> Result<Value, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        serde_json::to_value(store.app_settings().map_err(core_error)?).map_err(internal_error)
    }

    fn settings_save(&self, args: &Value) -> Result<Value, DaemonError> {
        let settings = serde_json::from_value::<tendi_core::storage::AppSettings>(args.clone())
            .map_err(internal_error)?;
        let _authority = self.control_authority()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let saved = with_database_write_lock(&store, || store.save_app_settings(settings.clone()))?;
        serde_json::to_value(saved).map_err(internal_error)
    }

    fn session_projects_list(&self) -> Result<Value, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        serde_json::to_value(store.list_session_projects().map_err(core_error)?)
            .map_err(internal_error)
    }

    fn project_scan_scopes_list(&self) -> Result<Value, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        serde_json::to_value(store.project_scan_scopes().map_err(core_error)?)
            .map_err(internal_error)
    }

    fn project_scan_scopes_save(&self, args: &Value) -> Result<Value, DaemonError> {
        let paths = string_vec_arg(args, "paths")?;
        let _authority = self.control_authority()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let saved =
            with_database_write_lock(&store, || store.save_project_scan_scopes(paths.clone()))?;
        serde_json::to_value(saved).map_err(internal_error)
    }

    fn projects_list(&self) -> Result<Value, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        serde_json::to_value(store.list_projects().map_err(core_error)?).map_err(internal_error)
    }

    fn projects_scan(&self) -> Result<Value, DaemonError> {
        let _authority = self.control_authority()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let cwd = self.state.cwd.clone();
        let result = with_database_write_lock(&store, || {
            let result = store.scan_projects()?;
            store.invalidate_projection("skills", &cwd)?;
            store.invalidate_projection("rules", &cwd)?;
            store.invalidate_projection("mcp", &cwd)?;
            Ok(result)
        })?;
        serde_json::to_value(result).map_err(internal_error)
    }

    fn agent_configs_list(&self) -> Result<Value, DaemonError> {
        serde_json::to_value(tendi_core::config::list_agent_configs().map_err(core_error)?)
            .map_err(internal_error)
    }

    fn agent_config_watch(&self, args: &Value) -> Result<Value, DaemonError> {
        let path = PathBuf::from(string_arg(args, "path")?);
        tendi_core::config::read_agent_config(&path).map_err(core_error)?;
        self.register_config_watch_path(&path)?;
        Ok(json!({ "path": path }))
    }

    fn agent_config_read(&self, args: &Value) -> Result<Value, DaemonError> {
        let path = string_arg(args, "path")?;
        serde_json::to_value(
            tendi_core::config::read_agent_config(Path::new(&path)).map_err(core_error)?,
        )
        .map_err(internal_error)
    }

    fn agent_config_save(&self, args: &Value) -> Result<Value, DaemonError> {
        let path = string_arg(args, "path")?;
        let expected = string_arg(args, "expectedSha256")?;
        let content = string_arg_allow_empty(args, "content")?;
        let _authority = self.control_authority()?;
        match tendi_core::config::save_agent_config(Path::new(&path), &expected, &content) {
            Ok(saved) => serde_json::to_value(saved).map_err(internal_error),
            Err(error) => {
                if let Some(changed) =
                    error.downcast_ref::<tendi_core::config::ConfigChangedError>()
                {
                    return Err(DaemonError::with_data(
                        "CONFLICT",
                        changed.to_string(),
                        serde_json::to_value(&changed.current).map_err(internal_error)?,
                    ));
                }
                Err(core_error(error))
            }
        }
    }

    fn config_profile_create(&self, args: &Value) -> Result<Value, DaemonError> {
        let agent = parse_agent(&string_arg(args, "agent")?)?;
        let name = string_arg(args, "name")?;
        let content = string_arg_allow_empty(args, "content")?;
        let _authority = self.control_authority()?;
        serde_json::to_value(
            tendi_core::config::create_config_profile(agent, &name, &content)
                .map_err(core_error)?,
        )
        .map_err(internal_error)
    }

    fn config_profile_set(&self, args: &Value) -> Result<Value, DaemonError> {
        let agent = parse_agent(&string_arg(args, "agent")?)?;
        let profile = optional_string_arg(args, "profile");
        let _authority = self.control_authority()?;
        let key = tendi_core::config_profile_key(agent)
            .ok_or_else(|| invalid_argument("config profiles are not supported for this agent"))?;
        if let Some(name) = profile.as_deref() {
            tendi_core::config::validate_profile_name(name).map_err(core_error)?;
            if !tendi_core::config::config_profile_exists(agent, name).map_err(core_error)? {
                return Err(DaemonError::new(
                    "NOT_FOUND",
                    format!("config profile not found: {name}"),
                ));
            }
        }
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let mut settings = store.app_settings().map_err(core_error)?;
        if let Some(profile) = profile {
            settings.config_profiles.insert(key.to_string(), profile);
        } else {
            settings.config_profiles.remove(key);
        }
        let saved = with_database_write_lock(&store, || store.save_app_settings(settings.clone()))?;
        serde_json::to_value(saved).map_err(internal_error)
    }

    fn rules_list(&self) -> Result<Value, DaemonError> {
        let report = self.rules_projection()?;
        serde_json::to_value(report.rules).map_err(internal_error)
    }

    fn rule_file_read(&self, args: &Value) -> Result<Value, DaemonError> {
        let path = string_arg(args, "path")?;
        let path = Path::new(&path);
        serde_json::to_value(tendi_core::rules::read_rule_file_at_path(path).map_err(core_error)?)
            .map_err(internal_error)
    }

    fn rule_file_save(&self, args: &Value) -> Result<Value, DaemonError> {
        let path = string_arg(args, "path")?;
        let expected = string_arg(args, "expectedSha256")?;
        let content = string_arg_allow_empty(args, "content")?;
        let _authority = self.control_authority()?;
        let path = Path::new(&path);
        let before = self.rules_projection()?;
        if !before.rules.iter().any(|rule| rule.path == path) {
            return Err(core_error(format!(
                "refusing to edit unknown rule {}",
                path.display()
            )));
        }
        let result = tendi_core::rules::save_rule_file_at_path(path, &expected, &content)
            .map_err(core_error)?;
        let mut after = before;
        if let Some(rule) = after.rules.iter_mut().find(|rule| rule.path == path) {
            rule.sha256 = result.sha256.clone();
        }
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let cwd = self.state.cwd.clone();
        with_database_write_lock(&store, || store.save_rules_for_workspace(&cwd, &after))?;
        serde_json::to_value(result).map_err(internal_error)
    }

    fn rules_projection(&self) -> Result<tendi_core::rules::RuleScan, DaemonError> {
        let cwd = self.state.cwd.clone();
        self.ensure_projection(
            "rules",
            |store| store.list_rules_for_workspace(&cwd),
            |store| {
                let project_roots = Self::registered_project_roots(store)?;
                let report = tendi_core::rules::scan_rules_for_project_roots(&cwd, &project_roots)?;
                with_database_write_lock(store, || store.save_rules_for_workspace(&cwd, &report))
                    .map_err(daemon_error_anyhow)?;
                Ok(report)
            },
        )
    }

    fn hooks_list(&self) -> Result<Value, DaemonError> {
        let report = self.hooks_projection()?;
        serde_json::to_value(report.hooks).map_err(internal_error)
    }

    fn hook_delete(&self, args: &Value) -> Result<Value, DaemonError> {
        let request = hook_delete_request(args)?;
        let _authority = self.control_authority()?;
        let before = self.hooks_projection()?;
        tendi_core::hooks::delete_hook(request.clone()).map_err(core_error)?;
        let scan = tendi_core::hooks::refresh_hook_scan_after_delete(
            &self.state.cwd,
            before,
            std::slice::from_ref(&request),
        )
        .map_err(core_error)?;
        self.save_hook_scan(&scan)?;
        serde_json::to_value(scan.hooks).map_err(internal_error)
    }

    fn hook_delete_many(&self, args: &Value) -> Result<Value, DaemonError> {
        let values = args
            .get("requests")
            .cloned()
            .ok_or_else(|| invalid_argument("missing argument: requests"))?;
        let requests = serde_json::from_value::<Vec<HookInput>>(values).map_err(internal_error)?;
        let _authority = self.control_authority()?;
        let requests = requests
            .into_iter()
            .map(HookInput::delete_request)
            .collect::<Result<Vec<_>, _>>()?;
        let before = self.hooks_projection()?;
        tendi_core::hooks::delete_hooks(requests.clone()).map_err(core_error)?;
        let scan =
            tendi_core::hooks::refresh_hook_scan_after_delete(&self.state.cwd, before, &requests)
                .map_err(core_error)?;
        self.save_hook_scan(&scan)?;
        serde_json::to_value(scan.hooks).map_err(internal_error)
    }

    fn hook_set_enabled(&self, args: &Value) -> Result<Value, DaemonError> {
        let request = hook_set_enabled_request(args)?;
        let _authority = self.control_authority()?;
        let before = self.hooks_projection()?;
        tendi_core::hooks::set_hook_enabled(request.clone()).map_err(core_error)?;
        let scan = tendi_core::hooks::refresh_hook_scan_after_set_enabled(
            &self.state.cwd,
            before,
            &request,
        )
        .map_err(core_error)?;
        self.save_hook_scan(&scan)?;
        serde_json::to_value(scan.hooks).map_err(internal_error)
    }

    fn hook_review(&self, args: &Value) -> Result<Value, DaemonError> {
        let request = hook_review_request(args)?;
        let _authority = self.control_authority()?;
        let before = self.hooks_projection()?;
        let scan = tendi_core::hooks::review_hook_from_scan(before, request).map_err(core_error)?;
        self.save_hook_scan(&scan)?;
        serde_json::to_value(scan.hooks).map_err(internal_error)
    }

    fn hook_source_read(&self, args: &Value) -> Result<Value, DaemonError> {
        let input = serde_json::from_value::<HookInput>(args.clone()).map_err(internal_error)?;
        let hook_match = input
            .event
            .clone()
            .map(|event| tendi_core::hooks::HookSourceMatch {
                event,
                matcher: input.matcher.clone(),
                hook_type: input.hook_type.clone(),
                command: input.command.clone(),
                url: input.url.clone(),
                prompt: input.prompt.clone(),
                filter: input.filter.clone(),
                status_message: input.status_message.clone(),
                enabled: input.enabled,
            });
        let expected_hash =
            (!input.expected_trust_hash.is_empty()).then_some(input.expected_trust_hash.as_str());
        let path = Path::new(&input.path);
        let result =
            tendi_core::hooks::read_hook_source_at_path(
                path,
                input.agent,
                expected_hash,
                hook_match.as_ref(),
            )
                .map_err(core_error)?;
        serde_json::to_value(result).map_err(internal_error)
    }

    fn mcp_list(&self) -> Result<Value, DaemonError> {
        let report = self.mcp_projection()?;
        serde_json::to_value(report.servers).map_err(internal_error)
    }

    fn mcp_projection(&self) -> Result<tendi_core::mcp::McpScan, DaemonError> {
        let cwd = self.state.cwd.clone();
        self.ensure_projection(
            "mcp",
            |store| store.list_mcp_for_workspace(&cwd),
            |store| {
                let project_roots = Self::registered_project_roots(store)?;
                let report = tendi_core::mcp::scan_mcp_for_project_roots(&cwd, &project_roots)?;
                with_database_write_lock(store, || store.save_mcp_for_workspace(&cwd, &report))
                    .map_err(daemon_error_anyhow)?;
                Ok(report)
            },
        )
    }

    fn mcp_set_enabled(&self, args: &Value) -> Result<Value, DaemonError> {
        let request = mcp_set_enabled_request(args)?;
        let _authority = self.control_authority()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let cwd = self.state.cwd.clone();
        let mut report = match store
            .list_mcp_for_workspace(&cwd)
            .map_err(core_error)?
        {
            Some(report) => report,
            None => self.mcp_projection()?,
        };
        if !report.servers.iter().any(|server| {
            server.agent == request.agent
                && server.name == request.name
                && server.path == request.path
        }) {
            return Err(conflict_error(
                "MCP server is not present in the current projection; refresh MCP before changing it",
            ));
        }
        let trust_hash = tendi_core::mcp::set_server_enabled(request.clone()).map_err(core_error)?;
        update_mcp_projection_for_toggle(&mut report, &request, trust_hash)?;
        with_database_write_lock(&store, || store.save_mcp_for_workspace(&cwd, &report))?;
        serde_json::to_value(report.servers).map_err(internal_error)
    }

    fn prompts_list(&self) -> Result<Value, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        serde_json::to_value(store.list_prompts().map_err(core_error)?).map_err(internal_error)
    }

    fn prompt_save(&self, args: &Value) -> Result<Value, DaemonError> {
        let id = optional_string_arg(args, "id");
        let title = string_arg(args, "title")?;
        let tags = string_vec_arg(args, "tags")?;
        let body = string_arg_allow_empty(args, "body")?;
        let _authority = self.control_authority()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let prompt = tendi_core::storage::PromptWrite {
            id,
            title,
            tags,
            body,
        };
        let saved = with_database_write_lock(&store, || store.save_prompt(prompt.clone()))?;
        serde_json::to_value(saved).map_err(internal_error)
    }

    fn prompts_delete_many(&self, args: &Value) -> Result<Value, DaemonError> {
        let ids = string_vec_arg(args, "ids")?;
        let _authority = self.control_authority()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let deleted = with_database_write_lock(&store, || store.delete_prompts(&ids))?;
        Ok(json!({ "deleted": deleted }))
    }

    fn session_transcript(&self, args: &Value) -> Result<Value, DaemonError> {
        let path = string_arg(args, "path")?;
        let agent = parse_agent(&string_arg(args, "agent")?)?;
        let cursor = optional_string_arg(args, "cursor");
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|value| value as usize);
        let known = optional_string_arg(args, "knownSourceVersion");
        serde_json::to_value(
            tendi_core::transcript::parse_transcript_page_if_changed(
                Path::new(&path),
                agent,
                cursor.as_deref(),
                limit,
                known.as_deref(),
            )
            .map_err(core_error)?,
        )
        .map_err(internal_error)
    }

    fn session_transcript_locator(&self, args: &Value) -> Result<Value, DaemonError> {
        let path = string_arg(args, "path")?;
        let agent = parse_agent(&string_arg(args, "agent")?)?;
        serde_json::to_value(
            tendi_core::transcript::parse_transcript_locator_page(Path::new(&path), agent)
                .map_err(core_error)?,
        )
        .map_err(internal_error)
    }

    fn session_transcript_search(&self, args: &Value) -> Result<Value, DaemonError> {
        let path = string_arg(args, "path")?;
        let agent = parse_agent(&string_arg(args, "agent")?)?;
        let query = string_arg_allow_empty(args, "query")?;
        let scopes = serde_json::from_value(args.get("scopes").cloned().unwrap_or_else(
            || json!({ "user": true, "assistant": true, "system": false, "tool": false }),
        ))
        .map_err(internal_error)?;
        serde_json::to_value(
            tendi_core::transcript::search_transcript(Path::new(&path), agent, &query, &scopes)
                .map_err(core_error)?,
        )
        .map_err(internal_error)
    }

    fn hooks_projection(&self) -> Result<tendi_core::hooks::HookScan, DaemonError> {
        let cwd = self.state.cwd.clone();
        self.ensure_projection(
            "hooks",
            |store| store.list_hooks_for_workspace(&cwd),
            |store| {
                let report = tendi_core::hooks::scan_hooks(&cwd)?;
                with_database_write_lock(store, || store.save_hooks_for_workspace(&cwd, &report))
                    .map_err(daemon_error_anyhow)?;
                Ok(report)
            },
        )
    }

    fn save_hook_scan(&self, scan: &tendi_core::HookScan) -> Result<(), DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let cwd = self.state.cwd.clone();
        with_database_write_lock(&store, || store.save_hooks_for_workspace(&cwd, scan)).map(|_| ())
    }

    fn configure_session_watcher(
        &self,
        plan: &tendi_core::sessions::SessionWatchPlan,
    ) -> Result<(), DaemonError> {
        let tx = self.state.session_runtime.watch_tx.clone();
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let _ = tx.send(result);
        })
        .map_err(internal_error)?;
        let mut watched_paths = BTreeSet::new();
        for target in plan.targets.iter().filter(|target| target.path.exists()) {
            let mode = if target.recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            watcher.watch(&target.path, mode).map_err(|error| {
                internal_error(format!(
                    "failed to watch {}: {error}",
                    target.path.display()
                ))
            })?;
            watched_paths.insert(target.path.clone());
        }
        *self
            .state
            .session_runtime
            .watcher
            .lock()
            .map_err(|_| internal_error("session watcher is unavailable"))? = SessionWatcherState {
            watcher: Some(watcher),
            watched_paths,
            dynamic_roots: plan.dynamic_roots.clone(),
        };
        Ok(())
    }

    fn initialize_config_watcher(&self) {
        let Ok(configs) = tendi_core::config::list_agent_configs() else {
            return;
        };
        for config in configs {
            if let Err(error) = self.register_config_watch_path(&config.path) {
                tendi_core::logging::global().warn(
                    "config watcher registration failed",
                    json!({ "path": config.path, "error": error.message }),
                );
            }
        }
    }

    fn register_config_watch_path(&self, path: &Path) -> Result<(), DaemonError> {
        let Some(directory) = existing_watch_directory(path) else {
            return Err(internal_error(format!(
                "config parent directory is unavailable: {}",
                path.display()
            )));
        };
        let mut state = self
            .state
            .config_runtime
            .watcher
            .lock()
            .map_err(|_| internal_error("config watcher is unavailable"))?;
        if state.watcher.is_none() {
            let tx = self.state.config_runtime.watch_tx.clone();
            state.watcher = Some(
                notify::recommended_watcher(move |result: notify::Result<Event>| {
                    let _ = tx.send(result);
                })
                .map_err(internal_error)?,
            );
        }
        if !state.watched_dirs.contains(&directory) {
            state
                .watcher
                .as_mut()
                .expect("config watcher was initialized")
                .watch(&directory, RecursiveMode::NonRecursive)
                .map_err(|error| {
                    internal_error(format!(
                        "failed to watch config directory {}: {error}",
                        directory.display()
                    ))
                })?;
            state.watched_dirs.insert(directory.clone());
        }
        state.watched_paths.insert(path.to_path_buf());
        Ok(())
    }

    fn config_watch_paths(&self) -> Vec<PathBuf> {
        self.state
            .config_runtime
            .watcher
            .lock()
            .map(|state| state.watched_paths.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn skills_list(&self) -> Result<Value, DaemonError> {
        let cwd = self.state.cwd.clone();
        let scan = self.ensure_projection(
            "skills",
            |store| store.list_skills_for_workspace(&cwd),
            |store| {
                let _authority = self.lock_authority().map_err(daemon_error_anyhow)?;
                let project_roots = Self::registered_project_roots(store)?;
                let scanned = tendi_core::skills::scan_skills_synced_for_project_roots_with_store_for_projection(
                    &cwd,
                    store,
                    &project_roots,
                )?;
                with_database_write_lock(store, || {
                    store.save_skills_for_workspace_with_source_migrations(
                        &cwd,
                        &scanned.scan,
                        &scanned.source_migrations,
                    )
                })
                .map_err(daemon_error_anyhow)?;
                Ok(scanned.scan)
            },
        )?;
        serde_json::to_value(scan.skills).map_err(internal_error)
    }

    fn skills_refresh(&self) -> Result<Value, DaemonError> {
        let scan = {
            let _authority = self.lock_authority()?;
            self.scan_and_persist()?
        };
        let update_check = self.start_skill_update_check(scan.clone());
        Ok(json!({ "skills": scan.skills, "updateCheck": update_check }))
    }

    fn skills_targets(&self) -> Result<Value, DaemonError> {
        let targets = std::iter::once(("shared", "Shared", true))
            .chain(tendi_core::skill_targets::target_catalog().iter().map(|target| {
                (target.id, target.display_name, target.supports_global())
            }))
            .map(|(id, display_name, supports_global)| {
                let target = id
                    .parse::<tendi_core::SkillTarget>()
                    .map_err(core_error)?;
                let global_path = if supports_global {
                    Some(
                        tendi_core::skill_targets::skill_target_root(
                            &self.state.cwd,
                            &target,
                            tendi_core::SkillInstallScope::Global,
                        )
                        .map_err(core_error)?
                        .to_string_lossy()
                        .into_owned(),
                    )
                } else {
                    None
                };
                Ok(json!({
                    "id": id,
                    "displayName": display_name,
                    "supportsGlobal": supports_global,
                    "globalPath": global_path,
                }))
            })
            .collect::<Result<Vec<_>, DaemonError>>()?;
        Ok(Value::Array(targets))
    }

    fn skills_backup_status(&self) -> Result<Value, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let config = store.skill_backup_config().map_err(core_error)?;
        if config.is_none() {
            return Ok(json!({ "config": config, "statuses": [], "versions": [] }));
        }
        let cached_scan = store
            .list_skills_cached_for_workspace(&self.state.cwd)
            .map_err(core_error)?;
        let paths = cached_scan
            .as_ref()
            .map(|scan| {
                scan.skills
                    .iter()
                    .flat_map(|skill| skill.paths.iter().map(|path| path.path.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut statuses = tendi_core::skill_backup::backup_statuses_for_paths(&store, &paths)
            .map_err(core_error)?;
        let excluded_paths = cached_scan
            .as_ref()
            .map(|scan| {
                scan.skills
                    .iter()
                    .flat_map(|skill| {
                        let reason = if skill.is_system {
                            Some("system-skill")
                        } else {
                            tendi_core::skills::skill_backup_exclusion_reason(&skill.paths)
                        };
                        skill.paths.iter().filter_map(move |path| {
                            reason.map(|reason| (path.path.clone(), reason))
                        })
                    })
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        for status in &mut statuses {
            if let Some(reason) = excluded_paths.get(&status.skill_path) {
                status.state = "excluded".to_string();
                status.reason = Some((*reason).to_string());
            }
        }
        let versions = if config.is_some() {
            tendi_core::skill_backup::backup_versions(&store, 50).map_err(core_error)?
        } else {
            Vec::new()
        };
        Ok(json!({ "config": config, "statuses": statuses, "versions": versions }))
    }

    fn skills_backup_configure(&self, args: &Value) -> Result<Value, DaemonError> {
        let remote_url = string_arg(args, "remoteUrl")?;
        let checkout_path = optional_string_arg(args, "checkoutPath")
            .filter(|path| !path.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or(tendi_core::skill_backup::default_checkout_path().map_err(core_error)?);
        let device_label = optional_string_arg(args, "deviceLabel")
            .filter(|label| !label.trim().is_empty())
            .unwrap_or_else(|| "My device".to_string());
        let config =
            tendi_core::skill_backup::BackupConfig::new(remote_url, checkout_path, device_label);
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let config = with_database_write_lock(&store, || store.save_skill_backup_config(&config))?;
        self.schedule_skill_backup();
        serde_json::to_value(config).map_err(internal_error)
    }

    fn skills_backup_now(&self) -> Result<Value, DaemonError> {
        let _authority = self.lock_authority()?;
        if self.state.backup_sync_running.swap(true, Ordering::AcqRel) {
            return Err(invalid_argument("a skill backup sync is already running"));
        }
        let report = (|| -> Result<_, DaemonError> {
            let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
            tendi_core::skill_backup::backup_now(&store).map_err(core_error)
        })();
        self.state
            .backup_sync_running
            .store(false, Ordering::Release);
        let report = report?;
        serde_json::to_value(report).map_err(internal_error)
    }

    fn skills_backup_sync(&self) -> Result<Value, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let configured = store.skill_backup_config().map_err(core_error)?.is_some();
        if configured {
            self.schedule_skill_backup();
        }
        Ok(json!({ "scheduled": configured }))
    }

    fn skills_backup_versions(&self, args: &Value) -> Result<Value, DaemonError> {
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let versions =
            tendi_core::skill_backup::backup_versions(&store, limit).map_err(core_error)?;
        serde_json::to_value(versions).map_err(internal_error)
    }

    fn skills_backup_restore(&self, args: &Value) -> Result<Value, DaemonError> {
        let revision = string_arg(args, "revision")?;
        let skill_ids = string_vec_arg(args, "skillIds")?;
        let target = string_arg(args, "target")?
            .parse::<tendi_core::SkillTarget>()
            .map_err(core_error)?;
        let scope = string_arg(args, "scope")?
            .parse::<tendi_core::SkillInstallScope>()
            .map_err(core_error)?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let plan = tendi_core::skill_backup::plan_backup_restore(
            &store,
            &self.state.cwd,
            &revision,
            &skill_ids,
            &target,
            scope,
        )
        .map_err(core_error)?;
        if bool_arg(args, "dryRun") {
            return serde_json::to_value(plan).map_err(internal_error);
        }
        if !bool_arg(args, "confirmed") {
            return Err(invalid_argument(
                "backup restore requires confirmed: true after preview",
            ));
        }
        let resolutions = args
            .get("resolutions")
            .cloned()
            .map(serde_json::from_value::<Vec<tendi_core::skill_backup::BackupRestoreResolution>>)
            .transpose()
            .map_err(|error| {
                invalid_argument(format!("invalid backup restore resolutions: {error}"))
            })?
            .unwrap_or_default();
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_mutation()?;
        let applied = tendi_core::skill_backup::apply_backup_restore_without_database(
            &plan,
            &resolutions,
        )
        .map_err(core_error)?;
        let operations = applied.operations;
        with_database_write_lock(&store, || {
            store.upsert_skill_source_records(&applied.source_records)
        })?;
        let refresh_names = operations
            .iter()
            .filter(|operation| operation.status == "restored")
            .map(|operation| operation.name.clone())
            .collect::<Vec<_>>();
        let extra_skill_dirs = operations
            .iter()
            .filter(|operation| operation.status == "restored")
            .map(|operation| operation.target.clone())
            .collect::<Vec<_>>();
        let scan = self.refresh_skill_projection(&before, &refresh_names, &extra_skill_dirs)?;
        Ok(json!({ "operations": operations, "skills": scan.skills }))
    }

    fn skills_backup_adopt(&self, args: &Value) -> Result<Value, DaemonError> {
        let name = string_arg(args, "name")?;
        let skill_path = PathBuf::from(string_arg(args, "skillPath")?);
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_mutation()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let record = tendi_core::skill_backup::skill_backup_record_for_adoption(
            &skill_path,
            name.clone(),
        )
        .map_err(core_error)?;
        let record = with_database_write_lock(&store, || {
            store
                .upsert_skill_source_records(std::slice::from_ref(&record))
                .map(|_| record.clone())
        })?;
        let scan = self.refresh_skill_projection(
            &before,
            std::slice::from_ref(&record.skill_name),
            std::slice::from_ref(&record.skill_path),
        )?;
        Ok(json!({ "record": record, "skills": scan.skills }))
    }

    fn skills_backup_disconnect(&self) -> Result<Value, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let disconnected = with_database_write_lock(&store, || store.clear_skill_backup_config())?;
        Ok(json!({ "disconnected": disconnected }))
    }

    fn skills_add(&self, args: &Value) -> Result<Value, DaemonError> {
        let options = self.skill_add_options(args)?;
        if bool_arg(args, "dryRun") {
            let plan = tendi_core::skills::plan_skill_add(&self.state.cwd, &options)
                .map_err(core_error)?;
            let id = self.next_preview_id("add")?;
            *self
                .state
                .add_preview
                .lock()
                .map_err(|_| internal_error("skill add preview store is unavailable"))? =
                Some(SkillAddPreview {
                    id: id.clone(),
                    options,
                    plan: plan.clone(),
                });
            return Ok(json!({ "applied": false, "plan": plan, "previewId": id }));
        }

        let preview_id = string_arg(args, "previewId")?;
        let preview = {
            let mut stored = self
                .state
                .add_preview
                .lock()
                .map_err(|_| internal_error("skill add preview store is unavailable"))?;
            let preview = stored.as_ref().ok_or_else(|| {
                conflict_error("skill add preview expired; preview the installation again")
            })?;
            if preview.id != preview_id
                || preview.options.source != options.source
                || preview.options.target != options.target
                || preview.options.scope != options.scope
                || preview.options.skills != options.skills
                || preview.options.copy != options.copy
                || preview.options.overwrite != options.overwrite
                || preview.options.visibility != options.visibility
            {
                return Err(conflict_error(
                    "skill add options changed; preview the installation again",
                ));
            }
            stored.take().expect("checked skill add preview")
        };
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_mutation()?;
        let report = tendi_core::skills::apply_skill_add_preview(&preview.plan, &options)
            .map_err(core_error)?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let source_records = tendi_core::skills::skill_source_records_for_add(&report);
        let snapshots =
            tendi_core::skills::capture_skill_snapshots(&source_records).map_err(core_error)?;
        with_database_write_lock(&store, || {
            store.upsert_skill_source_records(&source_records)?;
            store.replace_skill_snapshots(&snapshots)
        })?;
        let refresh_names = source_records
            .iter()
            .map(|record| record.skill_name.clone())
            .collect::<Vec<_>>();
        let extra_skill_dirs = report
            .results
            .iter()
            .map(|result| result.target.clone())
            .collect::<Vec<_>>();
        let refreshed = self.refresh_skill_projection(&before, &refresh_names, &extra_skill_dirs)?;
        Ok(json!({
            "applied": true,
            "report": report,
            "plan": report.plan,
            "results": report.results,
            "skills": refreshed.skills,
        }))
    }

    fn skills_add_preview_read(&self, args: &Value) -> Result<Value, DaemonError> {
        let preview_id = string_arg(args, "previewId")?;
        let skill_name = string_arg(args, "skillName")?;
        let preview = self
            .state
            .add_preview
            .lock()
            .map_err(|_| internal_error("skill add preview store is unavailable"))?;
        let skill = preview
            .as_ref()
            .filter(|preview| preview.id == preview_id)
            .and_then(|preview| {
                preview
                    .plan
                    .available
                    .iter()
                    .find(|skill| skill.name == skill_name)
            })
            .ok_or_else(|| conflict_error("skill is not in the current preview"))?;
        let path = skill.path.join("SKILL.md");
        let content =
            fs::read_to_string(&path).map_err(|error| core_error(anyhow::Error::new(error)))?;
        Ok(json!({ "name": skill.name, "relativePath": "SKILL.md", "content": content }))
    }

    fn skills_distribute(&self, args: &Value) -> Result<Value, DaemonError> {
        let sources = string_vec_arg(args, "sourcePaths")?
            .into_iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        if sources.is_empty() {
            return Err(invalid_argument("sourcePaths must not be empty"));
        }
        let target = string_arg(args, "target")?
            .parse::<tendi_core::SkillTarget>()
            .map_err(core_error)?;
        let scope = string_arg(args, "scope")?
            .parse::<tendi_core::SkillInstallScope>()
            .map_err(core_error)?;
        let mode = string_arg(args, "mode")?
            .parse::<tendi_core::skills::SkillDistributionMode>()
            .map_err(core_error)?;

        if bool_arg(args, "dryRun") {
            let scan = self.skill_projection_for_preview(&sources)?;
            let plans = sources
                .iter()
                .map(|source| {
                    tendi_core::skills::plan_skill_distribution_for_scan(
                        &self.state.cwd,
                        &scan,
                        source,
                        &target,
                        scope,
                        mode,
                    )
                    .map_err(core_error)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let id = self.next_preview_id("distribution")?;
            *self
                .state
                .distribution_preview
                .lock()
                .map_err(|_| internal_error("skill distribution preview store is unavailable"))? =
                Some(SkillDistributionPreview {
                    id: id.clone(),
                    sources,
                    target,
                    scope,
                    plans: plans.clone(),
                });
            return Ok(json!({
                "applied": false,
                "plans": plans,
                "previewId": id,
            }));
        }

        let mut before = None;
        let mut plans = if let Some(preview_id) = args.get("previewId").and_then(Value::as_str) {
            let mut stored = self.state.distribution_preview.lock().map_err(|_| {
                internal_error("skill distribution preview store is unavailable")
            })?;
            let preview = stored.as_ref().ok_or_else(|| {
                conflict_error("skill distribution preview expired; preview the change again")
            })?;
            if preview.id != preview_id
                || preview.sources != sources
                || preview.target != target
                || preview.scope != scope
            {
                return Err(conflict_error(
                    "skill distribution options changed; preview the change again",
                ));
            }
            stored.take().expect("checked skill distribution preview").plans
        } else {
            let scan = self.skill_projection_for_preview(&sources)?;
            before = Some(scan.clone());
            let plans = sources
                .iter()
                .map(|source| {
                    tendi_core::skills::plan_skill_distribution_for_scan(
                        &self.state.cwd,
                        &scan,
                        source,
                        &target,
                        scope,
                        mode,
                    )
                    .map_err(core_error)
                })
                .collect::<Result<Vec<_>, _>>()?;
            plans
        };

        let _authority = self.lock_authority()?;
        for plan in &mut plans {
            plan.mode = mode;
        }
        let results = plans
            .iter()
            .map(tendi_core::skills::apply_skill_distribution_plan)
            .collect::<Result<Vec<_>, _>>()
            .map_err(core_error)?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let mut target_records = Vec::with_capacity(plans.len());
        let mut moved_sources = Vec::new();
        for plan in &plans {
            let mut target_record = plan.source_record.clone();
            target_record.skill_path = plan.destination.clone();
            target_record.origin = "tendi-distribution".to_string();
            if mode == tendi_core::skills::SkillDistributionMode::Move {
                moved_sources.push(plan.source.clone());
            }
            target_records.push(target_record);
        }
        let snapshots =
            tendi_core::skills::capture_skill_snapshots(&target_records).map_err(core_error)?;
        with_database_write_lock(&store, || {
            if !moved_sources.is_empty() {
                store.delete_skill_source_records(&moved_sources)?;
            }
            store.upsert_skill_source_records(&target_records)?;
            store.replace_skill_snapshots(&snapshots)
        })?;
        let refresh_names = plans
            .iter()
            .map(|plan| plan.source_record.skill_name.clone())
            .collect::<Vec<_>>();
        let before = match before {
            Some(scan) => scan,
            None => self.skill_projection_for_names(&refresh_names)?,
        };
        let extra_skill_dirs = plans
            .iter()
            .map(|plan| plan.destination.clone())
            .collect::<Vec<_>>();
        let refreshed = self.refresh_skill_projection(&before, &refresh_names, &extra_skill_dirs)?;
        Ok(json!({
            "applied": true,
            "plans": plans,
            "results": results,
            "skills": refreshed.skills,
        }))
    }

    fn skills_remove_locations(&self, args: &Value) -> Result<Value, DaemonError> {
        let names = string_vec_arg(args, "names")?;
        let target_ids = string_vec_arg(args, "targets")?;
        if names.is_empty() {
            return Err(invalid_argument("names must not be empty"));
        }
        if target_ids.is_empty() {
            return Err(invalid_argument("targets must not be empty"));
        }
        let scope = string_arg(args, "scope")?
            .parse::<tendi_core::SkillInstallScope>()
            .map_err(core_error)?;
        let target_roots = target_ids
            .iter()
            .map(|target| {
                let parsed = target
                    .parse::<tendi_core::SkillTarget>()
                    .map_err(core_error)?;
                let root =
                    tendi_core::skill_targets::skill_target_root(&self.state.cwd, &parsed, scope)
                        .map_err(core_error)?;
                Ok::<_, DaemonError>(root)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let _authority = self.lock_authority()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let scan = self.skill_projection_for_names(&names)?;
        let mut seen = BTreeSet::new();
        let mut targets = Vec::new();
        for skill in scan
            .skills
            .iter()
            .filter(|skill| names.iter().any(|name| name == &skill.name))
        {
            if skill.is_system {
                return Err(conflict_error(format!(
                    "refusing to remove a location from read-only system skill {}",
                    skill.name
                )));
            }
            for path in &skill.paths {
                let is_target_path = path
                    .path
                    .parent()
                    .is_some_and(|parent| target_roots.iter().any(|root| root == parent));
                if !is_target_path || !seen.insert(path.path.clone()) {
                    continue;
                }
                let metadata = fs::symlink_metadata(&path.path)
                    .map_err(|error| core_error(anyhow::Error::new(error)))?;
                let kind = if metadata.file_type().is_symlink() {
                    "symlink"
                } else if metadata.is_dir() {
                    "directory"
                } else if metadata.is_file() {
                    "file"
                } else {
                    return Err(conflict_error(format!(
                        "refusing to remove unsupported path {}",
                        path.path.display()
                    )));
                };
                targets.push(tendi_core::skills::SkillDeleteTarget {
                    name: skill.name.clone(),
                    path: path.path.clone(),
                    kind: kind.to_string(),
                });
            }
        }

        let plan = tendi_core::skills::SkillDeletePlan {
            targets,
            dependencies: Vec::new(),
            dependents: Vec::new(),
        };
        let summary = tendi_core::skills::format_delete_plan(&plan);
        tendi_core::skills::apply_skill_delete_plan(&plan).map_err(core_error)?;
        let paths = plan
            .targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<Vec<_>>();
        let scan = tendi_core::skills::refresh_skill_scan(&self.state.cwd, &scan, &names, &[])
            .map_err(core_error)?;
        let cwd = self.state.cwd.clone();
        with_database_write_lock(&store, || {
            store.delete_skill_source_records(&paths)?;
            store.save_skills_for_workspace(&cwd, &scan)
        })?;
        Ok(json!({
            "summary": summary,
            "applied": true,
            "plan": plan,
            "skills": scan.skills,
        }))
    }

    fn skills_set(&self, args: &Value) -> Result<Value, DaemonError> {
        let names = string_vec_arg(args, "names")?;
        let visibility = parse_visibility(&string_arg(args, "visibility")?)?;
        let dry_run = bool_arg(args, "dryRun");
        let _authority = (!dry_run).then(|| self.lock_authority()).transpose()?;
        let before = self.skill_projection_for_names(&names)?;
        let changeset =
            tendi_core::skills::plan_visibility_many_for_scan(&before, &names, visibility)
                .map_err(core_error)?;
        let summary = tendi_core::skills::format_changeset(&changeset);
        if !dry_run {
            tendi_core::skills::apply_changes(&changeset).map_err(core_error)?;
            let refreshed = self.refresh_skill_projection(&before, &names, &[])?;
            return Ok(json!({ "summary": summary, "applied": true, "skills": refreshed.skills }));
        }
        Ok(json!({ "summary": summary, "applied": false, "skills": Value::Null }))
    }

    fn skills_wrap(&self, args: &Value) -> Result<Value, DaemonError> {
        let name = string_arg(args, "name")?;
        let names = string_vec_arg(args, "names")?;
        let description = optional_string_arg(args, "description");
        let manual_children = bool_arg(args, "manualChildren");
        let refresh = bool_arg(args, "refresh");
        let dry_run = bool_arg(args, "dryRun");
        let _authority = (!dry_run).then(|| self.lock_authority()).transpose()?;
        let mut projection_names = names.clone();
        if refresh && !projection_names.iter().any(|selected| selected == &name) {
            projection_names.push(name.clone());
        }
        let mut before = self.skill_projection_for_names(&projection_names)?;
        let shared_target = "shared"
            .parse::<tendi_core::SkillTarget>()
            .map_err(core_error)?;
        let shared_root = tendi_core::skill_targets::skill_target_root(
            &self.state.cwd,
            &shared_target,
            tendi_core::SkillInstallScope::Global,
        )
        .map_err(core_error)?;
        if !before.roots.iter().any(|root| root.path == shared_root) {
            before.roots.push(tendi_core::skills::SkillRoot {
                path: shared_root,
                scope: "global".to_string(),
                agent: tendi_core::AgentKind::Shared,
                plugin_id: None,
                plugin_enabled: None,
            });
        }
        let changeset = if refresh {
            tendi_core::skills::refresh_wrapper_from_names_for_scan(
                &before,
                &name,
                &names,
                manual_children,
            )
        } else {
            tendi_core::skills::plan_wrapper_from_names_for_scan(
                &before,
                &name,
                &names,
                description.as_deref(),
                manual_children,
            )
        }
        .map_err(core_error)?;
        let summary = tendi_core::skills::format_changeset(&changeset);
        if !dry_run {
            tendi_core::skills::apply_changes(&changeset).map_err(core_error)?;
            let mut refresh_names = names;
            refresh_names.push(name);
            let extra_skill_dirs = changeset
                .changes
                .iter()
                .filter_map(|change| change.path.parent().map(Path::to_path_buf))
                .collect::<Vec<_>>();
            let refreshed =
                self.refresh_skill_projection(&before, &refresh_names, &extra_skill_dirs)?;
            return Ok(json!({ "summary": summary, "applied": true, "skills": refreshed.skills }));
        }
        Ok(json!({ "summary": summary, "applied": false, "skills": Value::Null }))
    }

    fn skills_updates(&self, args: &Value) -> Result<Value, DaemonError> {
        if bool_arg(args, "check") {
            let scan = self.skill_projection()?;
            return Ok(json!({
                "updateCheck": self.start_skill_update_check(scan),
            }));
        }
        let _authority = self.lock_authority()?;
        let scan = self.scan_and_persist()?;
        serde_json::to_value(scan.skills).map_err(internal_error)
    }

    fn skills_updates_cancel(&self) -> Result<Value, DaemonError> {
        let running = self.state.skill_update_running.load(Ordering::Acquire);
        if running {
            self.state
                .skill_update_cancelled
                .store(true, Ordering::Release);
        }
        Ok(json!({
            "status": if running { "cancellation-requested" } else { "not-running" },
        }))
    }

    fn start_skill_update_check(&self, scan: tendi_core::skills::SkillScan) -> &'static str {
        if self
            .state
            .skill_update_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return "already-running";
        }
        self.state
            .skill_update_cancelled
            .store(false, Ordering::Release);
        let daemon = self.clone();
        thread::spawn(move || {
            let result = (|| -> Result<Value, DaemonError> {
                let updates = tendi_core::skills::check_skill_updates_for_scan_with_cancel(
                    &scan,
                    &daemon.state.skill_update_cancelled,
                );
                if daemon.state.skill_update_cancelled.load(Ordering::Acquire) {
                    return Err(DaemonError::new(
                        "CANCELLED",
                        "skill update check cancelled",
                    ));
                }
                Ok(json!({
                    "status": "completed",
                    "skills": scan.skills,
                    "updates": updates,
                    "error": Value::Null,
                }))
            })();
            let event = match result {
                Ok(value) => value,
                Err(error) => json!({
                    "status": "failed",
                    "skills": Value::Null,
                    "updates": [],
                    "error": error.message,
                }),
            };
            daemon.emit_event(SKILL_UPDATE_EVENT, event);
            daemon
                .state
                .skill_update_running
                .store(false, Ordering::Release);
        });
        "started"
    }

    fn skills_update(&self, args: &Value) -> Result<Value, DaemonError> {
        let pattern = string_arg(args, "pattern")?;
        let dry_run = bool_arg(args, "dryRun");
        let _authority = (!dry_run).then(|| self.lock_authority()).transpose()?;
        let scan = self.skill_projection()?;
        let plan =
            tendi_core::skills::plan_skill_updates_for_scan(&scan, &pattern).map_err(core_error)?;
        let summary = tendi_core::skills::format_update_plan(&plan);
        if !dry_run {
            let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
            let resolutions = string_map_arg(args, "resolutions")?;
            let prepared = tendi_core::skills::prepare_skill_update_plan_with_resolutions(
                &plan,
                &resolutions,
            )
            .map_err(core_error)?;
            tendi_core::skills::apply_skill_update_plan_filesystem(&prepared)
                .map_err(core_error)?;
            let persistence = tendi_core::skills::prepare_skill_update_persistence(
                &store,
                &prepared,
            )
            .map_err(core_error)?;
            with_database_write_lock(&store, || {
                tendi_core::skills::persist_skill_update_persistence(&store, &persistence)
            })?;
            let refresh_names = skill_update_refresh_names(&scan, &plan);
            let extra_skill_dirs = skill_update_refresh_dirs(&plan);
            let scan = self.refresh_skill_projection(&scan, &refresh_names, &extra_skill_dirs)?;
            return Ok(
                json!({ "summary": summary, "applied": true, "plan": plan, "skills": scan.skills }),
            );
        }
        Ok(json!({ "summary": summary, "applied": false, "plan": plan, "skills": Value::Null }))
    }

    fn skills_update_many(&self, args: &Value) -> Result<Value, DaemonError> {
        let names = string_vec_arg(args, "names")?;
        let dry_run = bool_arg(args, "dryRun");
        let preview_id = if dry_run {
            None
        } else {
            Some(optional_string_arg(args, "previewId").ok_or_else(|| {
                conflict_error("skill update preview expired; preview the update again")
            })?)
        };
        let plan = if dry_run {
            let scan = self.skill_projection_for_names(&names)?;
            tendi_core::skills::plan_skill_updates_many_for_scan(&scan, &names)
                .map_err(core_error)?
        } else {
            let preview_id = preview_id.as_deref().expect("non-dry-run has preview id");
            let stored = self
                .state
                .update_preview
                .lock()
                .map_err(|_| internal_error("update preview store is unavailable"))?;
            let preview = stored.as_ref().ok_or_else(|| {
                conflict_error("skill update preview expired; preview the update again")
            })?;
            if preview.id != preview_id || preview.names != names {
                return Err(conflict_error(
                    "update selection changed; preview the update again",
                ));
            }
            preview.plan.clone()
        };
        let summary = tendi_core::skills::format_update_plan(&plan);
        if dry_run {
            let id = self.next_preview_id("update")?;
            *self
                .state
                .update_preview
                .lock()
                .map_err(|_| internal_error("update preview store is unavailable"))? =
                Some(SkillUpdatePreview {
                    id: id.clone(),
                    names,
                    plan: plan.clone(),
                });
            return Ok(
                json!({ "summary": summary, "applied": false, "plan": plan, "previewId": id, "skills": Value::Null }),
            );
        }
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_names(&names)?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let resolutions = string_map_arg(args, "resolutions")?;
        let prepared = tendi_core::skills::prepare_skill_update_plan_with_resolutions(
            &plan,
            &resolutions,
        )
        .map_err(core_error)?;
        tendi_core::skills::apply_skill_update_plan_filesystem(&prepared)
            .map_err(core_error)?;
        let persistence = tendi_core::skills::prepare_skill_update_persistence(
            &store,
            &prepared,
        )
        .map_err(core_error)?;
        with_database_write_lock(&store, || {
            tendi_core::skills::persist_skill_update_persistence(&store, &persistence)
        })?;
        if let Some(preview_id) = preview_id {
            if let Ok(mut stored) = self.state.update_preview.lock() {
                if stored
                    .as_ref()
                    .is_some_and(|preview| preview.id == preview_id)
                {
                    stored.take();
                }
            }
        }
        let refresh_names = names.clone();
        let extra_skill_dirs = skill_update_refresh_dirs(&plan);
        let scan = self.refresh_skill_projection(&before, &refresh_names, &extra_skill_dirs)?;
        Ok(
            json!({ "summary": summary, "applied": true, "plan": plan, "previewId": Value::Null, "skills": scan.skills }),
        )
    }

    fn skills_delete_many(&self, args: &Value) -> Result<Value, DaemonError> {
        let names = string_vec_arg(args, "names")?;
        let _authority = self.lock_authority()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let scan = self.skill_projection()?;
        let plan = tendi_core::skills::plan_skill_delete_many_for_scan(&scan, &names)
            .map_err(core_error)?;
        let summary = tendi_core::skills::format_delete_plan(&plan);
        tendi_core::skills::apply_skill_delete_plan(&plan).map_err(core_error)?;
        let paths = plan
            .targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<Vec<_>>();
        let scan = tendi_core::skills::refresh_skill_scan(&self.state.cwd, &scan, &names, &[])
            .map_err(core_error)?;
        let cwd = self.state.cwd.clone();
        with_database_write_lock(&store, || {
            store.delete_skill_source_records(&paths)?;
            store.save_skills_for_workspace(&cwd, &scan)
        })?;
        Ok(
            json!({ "summary": summary, "applied": true, "plan": plan, "previewId": Value::Null, "refreshRequired": false, "skills": scan.skills }),
        )
    }

    fn skills_marketplace_search(&self, args: &Value) -> Result<Value, DaemonError> {
        let query = string_arg(args, "query")?;
        let result = tendi_core::skill_marketplace::search(&query).map_err(core_error)?;
        serde_json::to_value(result).map_err(internal_error)
    }

    fn skill_files(&self, args: &Value) -> Result<Value, DaemonError> {
        let name = string_arg(args, "name")?;
        let cached = self.skill_dir_for_read(args, &name)?;
        let result =
            tendi_core::files::list_skill_files(&self.state.cwd, &name, Some(cached.as_path()))
                .map_err(core_error)?;
        serde_json::to_value(result).map_err(internal_error)
    }

    fn skill_file_read(&self, args: &Value) -> Result<Value, DaemonError> {
        let name = string_arg(args, "name")?;
        let relative_path = string_arg(args, "relativePath")?;
        let cached = self.skill_dir_for_read(args, &name)?;
        let result = tendi_core::files::read_skill_file(
            &self.state.cwd,
            &name,
            &relative_path,
            Some(cached.as_path()),
        )
        .map_err(core_error)?;
        serde_json::to_value(result).map_err(internal_error)
    }

    fn skill_dir_for_read(&self, args: &Value, name: &str) -> Result<PathBuf, DaemonError> {
        if let Some(path) = args.get("skillPath").and_then(Value::as_str) {
            let path = PathBuf::from(path);
            if path.is_dir() {
                return Ok(path);
            }
        }
        let scan = self.skill_projection_for_names(&[name.to_string()])?;
        self.skill_dir_from_scan(&scan, name)
    }

    fn skill_file_save(&self, args: &Value) -> Result<Value, DaemonError> {
        let name = string_arg(args, "name")?;
        let relative_path = string_arg(args, "relativePath")?;
        let expected_sha256 = string_arg(args, "expectedSha256")?;
        let content = string_arg_allow_empty(args, "content")?;
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_names(std::slice::from_ref(&name))?;
        let cached = self.skill_dir_from_scan(&before, &name)?;
        let result = tendi_core::files::save_skill_file(
            &self.state.cwd,
            &name,
            &relative_path,
            &expected_sha256,
            &content,
            Some(cached.as_path()),
        )
        .map_err(core_error)?;
        let scan = self.refresh_skill_projection(&before, std::slice::from_ref(&name), &[])?;
        let mut value = serde_json::to_value(result).map_err(internal_error)?;
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "skills".to_string(),
                serde_json::to_value(scan.skills).map_err(internal_error)?,
            );
        }
        Ok(value)
    }

    fn skill_file_create(&self, args: &Value) -> Result<Value, DaemonError> {
        let name = string_arg(args, "name")?;
        let relative_path = string_arg(args, "relativePath")?;
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_names(std::slice::from_ref(&name))?;
        let cached = self.skill_dir_from_scan(&before, &name)?;
        let result = tendi_core::files::create_skill_file(
            &self.state.cwd,
            &name,
            &relative_path,
            Some(cached.as_path()),
        )
        .map_err(core_error)?;
        self.refresh_skill_projection(&before, std::slice::from_ref(&name), &[])?;
        serde_json::to_value(result).map_err(internal_error)
    }

    fn skill_folder_create(&self, args: &Value) -> Result<Value, DaemonError> {
        let name = string_arg(args, "name")?;
        let relative_path = string_arg(args, "relativePath")?;
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_names(std::slice::from_ref(&name))?;
        let cached = self.skill_dir_from_scan(&before, &name)?;
        tendi_core::files::create_skill_folder(
            &self.state.cwd,
            &name,
            &relative_path,
            Some(cached.as_path()),
        )
        .map_err(core_error)?;
        self.refresh_skill_projection(&before, std::slice::from_ref(&name), &[])?;
        Ok(Value::Null)
    }

    fn skill_path_rename(&self, args: &Value) -> Result<Value, DaemonError> {
        let name = string_arg(args, "name")?;
        let from = string_arg(args, "fromRelativePath")?;
        let to = string_arg(args, "toRelativePath")?;
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_names(std::slice::from_ref(&name))?;
        let cached = self.skill_dir_from_scan(&before, &name)?;
        tendi_core::files::rename_skill_path(
            &self.state.cwd,
            &name,
            &from,
            &to,
            Some(cached.as_path()),
        )
        .map_err(core_error)?;
        self.refresh_skill_projection(&before, std::slice::from_ref(&name), &[])?;
        Ok(Value::Null)
    }

    fn skill_path_delete(&self, args: &Value) -> Result<Value, DaemonError> {
        let name = string_arg(args, "name")?;
        let relative_path = string_arg(args, "relativePath")?;
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_names(std::slice::from_ref(&name))?;
        let cached = self.skill_dir_from_scan(&before, &name)?;
        tendi_core::files::delete_skill_path(
            &self.state.cwd,
            &name,
            &relative_path,
            Some(cached.as_path()),
        )
        .map_err(core_error)?;
        self.refresh_skill_projection(&before, std::slice::from_ref(&name), &[])?;
        Ok(Value::Null)
    }

    fn skill_projection(&self) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        let cwd = self.state.cwd.clone();
        self.ensure_projection(
            "skills",
            |store| store.list_skills_for_workspace(&cwd),
            |store| {
                let project_roots = Self::registered_project_roots(store)?;
                let scanned = tendi_core::skills::scan_skills_synced_for_project_roots_with_store_for_projection(
                    &cwd,
                    store,
                    &project_roots,
                )?;
                with_database_write_lock(store, || {
                    store.save_skills_for_workspace_with_source_migrations(
                        &cwd,
                        &scanned.scan,
                        &scanned.source_migrations,
                    )
                })
                .map_err(daemon_error_anyhow)?;
                Ok(scanned.scan)
            },
        )
    }

    fn skill_projection_for_mutation(&self) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        if let Some(scan) = store
            .list_skills_cached_for_workspace(&self.state.cwd)
            .map_err(core_error)?
        {
            return Ok(scan);
        }
        self.skill_projection()
    }

    fn skill_projection_for_preview(
        &self,
        source_paths: &[PathBuf],
    ) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        let cwd = self.state.cwd.clone();
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        if let Some(scan) = store.list_skills_for_workspace(&cwd).map_err(core_error)? {
            return Ok(scan);
        }

        let cached = store.list_skills().map_err(core_error)?;
        let mut names = BTreeSet::new();
        for source_path in source_paths {
            let Some(skill) = cached
                .skills
                .iter()
                .find(|skill| skill.paths.iter().any(|path| path.path == *source_path))
            else {
                return Err(conflict_error(
                    "skills list is stale; refresh skills before previewing this location change",
                ));
            };
            names.insert(skill.name.clone());
        }
        tendi_core::skills::refresh_skill_scan(
            &cwd,
            &cached,
            &names.into_iter().collect::<Vec<_>>(),
            &[],
        )
        .map_err(core_error)
    }

    fn skill_projection_for_names(
        &self,
        names: &[String],
    ) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        let cwd = self.state.cwd.clone();
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        if let Some(scan) = store
            .list_skills_for_names_if_current(&cwd, names)
            .map_err(core_error)?
        {
            return Ok(scan);
        }

        let cached = store.list_skills().map_err(core_error)?;
        if names
            .iter()
            .any(|name| !cached.skills.iter().any(|skill| skill.name == *name))
        {
            return Err(conflict_error(
                "skills list is stale; refresh skills before continuing this selection",
            ));
        }
        tendi_core::skills::refresh_skill_scan(&cwd, &cached, names, &[]).map_err(core_error)
    }

    fn skill_dir_from_scan(
        &self,
        scan: &tendi_core::skills::SkillScan,
        name: &str,
    ) -> Result<PathBuf, DaemonError> {
        scan.skills
            .iter()
            .find(|skill| skill.name == name)
            .and_then(|skill| skill.paths.first())
            .map(|path| path.path.clone())
            .filter(|path| path.is_dir())
            .ok_or_else(|| core_error(format!("skill {name} was not found")))
    }

    fn refresh_skill_projection(
        &self,
        before: &tendi_core::skills::SkillScan,
        names: &[String],
        extra_skill_dirs: &[PathBuf],
    ) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        let scan = tendi_core::skills::refresh_skill_scan(
            &self.state.cwd,
            before,
            names,
            extra_skill_dirs,
        )
        .map_err(core_error)?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let cwd = self.state.cwd.clone();
        with_database_write_lock(&store, || store.save_skills_for_workspace(&cwd, &scan))?;
        self.schedule_skill_backup();
        Ok(scan)
    }

    fn scan_and_persist(&self) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        let cwd = self.state.cwd.clone();
        let scan = self.ensure_projection(
            "skills",
            |_| Ok(None),
            |store| {
                let project_roots = Self::registered_project_roots(store)?;
                let scanned = tendi_core::skills::scan_skills_synced_for_project_roots_with_store_for_projection(
                    &cwd,
                    store,
                    &project_roots,
                )?;
                with_database_write_lock(store, || {
                    store.save_skills_for_workspace_with_source_migrations(
                        &cwd,
                        &scanned.scan,
                        &scanned.source_migrations,
                    )
                })
                .map_err(daemon_error_anyhow)?;
                Ok(scanned.scan)
            },
        )?;
        self.schedule_skill_backup();
        Ok(scan)
    }

    fn schedule_skill_backup(&self) {
        if self.state.backup_sync_running.swap(true, Ordering::AcqRel) {
            return;
        }
        let daemon = self.clone();
        thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                let store = tendi_core::storage::Store::open_default()?;
                if store.skill_backup_config()?.is_none() {
                    return Ok(());
                }
                tendi_core::skill_backup::backup_now(&store)?;
                Ok(())
            })();
            if let Err(error) = result {
                tendi_core::logging::global().warn(
                    "skill backup sync failed",
                    json!({ "error": format!("{error:#}") }),
                );
            }
            daemon
                .state
                .backup_sync_running
                .store(false, Ordering::Release);
        });
    }

    fn lock_authority(&self) -> Result<std::sync::MutexGuard<'_, ()>, DaemonError> {
        self.state
            .skill_authority
            .lock()
            .map_err(|_| internal_error("skill authority is unavailable"))
    }

    fn control_authority(&self) -> Result<std::sync::MutexGuard<'_, ()>, DaemonError> {
        self.state
            .control_authority
            .lock()
            .map_err(|_| internal_error("daemon control authority is unavailable"))
    }

    fn next_preview_id(&self, kind: &str) -> Result<String, DaemonError> {
        let mut sequence = self
            .state
            .preview_sequence
            .lock()
            .map_err(|_| internal_error("preview sequence is unavailable"))?;
        let id = format!("daemon-{kind}-{}", *sequence);
        *sequence += 1;
        Ok(id)
    }

    fn skill_add_options(
        &self,
        args: &Value,
    ) -> Result<tendi_core::skills::SkillAddOptions, DaemonError> {
        Ok(tendi_core::skills::SkillAddOptions {
            source: string_arg(args, "source")?,
            target: string_arg(args, "target")?
                .parse()
                .map_err(|error| invalid_argument(format!("invalid skill target: {error}")))?,
            scope: string_arg(args, "scope")?
                .parse()
                .map_err(|error| invalid_argument(format!("invalid skill scope: {error}")))?,
            skills: string_vec_arg(args, "skills")?,
            copy: bool_arg(args, "copy"),
            overwrite: bool_arg(args, "overwrite"),
            visibility: parse_visibility(&string_arg(args, "visibility")?)?,
        })
    }
}

fn skill_update_refresh_names(
    scan: &tendi_core::skills::SkillScan,
    plan: &tendi_core::skills::SkillUpdatePlan,
) -> Vec<String> {
    let mut names = BTreeSet::new();
    for action in &plan.git_updates {
        names.extend(action.skill_names.iter().cloned());
        names.extend(
            action
                .materialized_targets
                .iter()
                .map(|target| target.name.clone()),
        );
    }
    for update in &plan.source_updates {
        if let Some(skill) = scan.skills.iter().find(|skill| {
            skill
                .paths
                .iter()
                .any(|path| path.path == update.skill_path)
        }) {
            names.insert(skill.name.clone());
        }
    }
    for change in &plan.file_changes.changes {
        for skill in &scan.skills {
            if skill
                .paths
                .iter()
                .any(|path| change.path.starts_with(&path.path))
            {
                names.insert(skill.name.clone());
            }
        }
    }
    names.into_iter().collect()
}

fn skill_update_refresh_dirs(plan: &tendi_core::skills::SkillUpdatePlan) -> Vec<PathBuf> {
    plan.git_updates
        .iter()
        .flat_map(|action| {
            action
                .materialized_targets
                .iter()
                .map(|target| target.target.clone())
        })
        .collect()
}

fn with_session_database_write_lock<T, F>(
    store: &tendi_core::storage::Store,
    write: F,
) -> Result<T, DaemonError>
where
    F: FnMut() -> anyhow::Result<T>,
{
    with_database_write_lock(store, write)
}

fn daemon_error_anyhow(error: DaemonError) -> anyhow::Error {
    anyhow::anyhow!("{}: {}", error.code, error.message)
}

fn with_database_write_lock<T, F>(
    store: &tendi_core::storage::Store,
    mut write: F,
) -> Result<T, DaemonError>
where
    F: FnMut() -> anyhow::Result<T>,
{
    for attempt in 0..DATABASE_WRITE_LOCK_ATTEMPTS {
        if let Some(value) = store
            .with_database_write_lock(&mut write)
            .map_err(core_error)?
        {
            return Ok(value);
        }
        if attempt + 1 < DATABASE_WRITE_LOCK_ATTEMPTS {
            thread::sleep(DATABASE_WRITE_LOCK_RETRY);
        }
    }
    Err(internal_error(
        "timed out waiting for the database write lock",
    ))
}

fn session_scan_is_current(
    generation: u64,
    observed_revision: u64,
    completed_revision: u64,
) -> bool {
    generation != 0 && observed_revision == completed_revision
}

fn session_scan_start_response(generation: u64, started: bool) -> Value {
    json!({
        "generation": generation,
        "started": started,
    })
}

fn session_root_priority(root: &Path) -> u8 {
    tendi_core::session_root_priority(root)
}

fn run_session_scan(
    daemon: &Daemon,
    generation: u64,
    additional_session_roots: &[PathBuf],
) -> Result<(), DaemonError> {
    let scan_started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
    let last_scan_at = store.sessions_last_scan_at().map_err(core_error)?;
    let cache = store.session_scan_cache().map_err(core_error)?;
    let mut scanned = 0;
    let mut roots =
        tendi_core::sessions::session_watch_roots(&daemon.state.cwd, additional_session_roots);
    roots.sort_by_key(|root| session_root_priority(root));
    for root in roots {
        let recent_paths = tendi_core::sessions::recent_session_paths_in_root(&root, last_scan_at);
        for paths in recent_paths.chunks(SESSION_SCAN_BATCH_SIZE) {
            let report = tendi_core::sessions::scan_session_paths(paths, &cache);
            let analytics_sessions = report.sessions.clone();
            let changed = cache.changed_sessions(&report.sessions);
            let upserts = with_session_database_write_lock(&store, || {
                let mut upserts = store.apply_session_delta(&changed)?;
                store.resolve_session_projects(&mut upserts)?;
                Ok(upserts)
            })?;
            scanned += paths.len();
            daemon.emit_event(
                SESSION_SCAN_EVENT,
                json!({
                    "generation": generation,
                    "phase": "recent",
                    "upserts": upserts,
                    "deleted": [],
                    "scanned": scanned,
                    "complete": false,
                    "error": Value::Null,
                }),
            );
            let _ = daemon
                .state
                .session_runtime
                .analytics_tx
                .send(AnalyticsRefreshJob {
                    phase: "recent",
                    sessions: analytics_sessions,
                });
        }
    }
    daemon.emit_event(
        SESSION_SCAN_EVENT,
        json!({
            "generation": generation,
            "phase": "recent",
            "upserts": [],
            "deleted": [],
            "scanned": scanned,
            "complete": true,
            "error": Value::Null,
        }),
    );

    let cache = store.session_scan_cache().map_err(core_error)?;
    let report = tendi_core::sessions::scan_sessions_with_additional_roots_cached(
        &daemon.state.cwd,
        additional_session_roots,
        &cache,
    )
    .map_err(core_error)?;
    with_session_database_write_lock(&store, || store.save_sessions_at(&report, scan_started_at))?;
    let analytics_sessions = report.sessions.clone();
    daemon.emit_event(
        SESSION_SCAN_EVENT,
        json!({
            "generation": generation,
            "phase": "backfill",
            "upserts": [],
            "deleted": [],
            "scanned": report.sessions.len(),
            "complete": true,
            "error": Value::Null,
        }),
    );
    let _ = daemon
        .state
        .session_runtime
        .analytics_tx
        .send(AnalyticsRefreshJob {
            phase: "backfill",
            sessions: analytics_sessions,
        });
    Ok(())
}

fn existing_watch_directory(path: &Path) -> Option<PathBuf> {
    let mut directory = path.parent()?.to_path_buf();
    while !directory.is_dir() {
        directory = directory.parent()?.to_path_buf();
    }
    Some(directory)
}

fn config_watch_loop(daemon: Daemon, receiver: Receiver<notify::Result<Event>>) {
    let mut pending = BTreeSet::new();
    loop {
        match receiver.recv_timeout(CONFIG_WATCH_DEBOUNCE) {
            Ok(Ok(event)) => {
                let watched_paths = daemon.config_watch_paths();
                for path in &watched_paths {
                    let parent = path.parent();
                    if event.paths.iter().any(|changed| {
                        changed == path || parent.is_some_and(|parent| changed == parent)
                    }) {
                        pending.insert(path.clone());
                    }
                    if parent.is_some_and(Path::is_dir) {
                        let _ = daemon.register_config_watch_path(path);
                    }
                }
            }
            Ok(Err(error)) => {
                tendi_core::logging::global().error(
                    "config watcher failed",
                    json!({ "error": error.to_string() }),
                );
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if pending.is_empty() {
            continue;
        }
        let paths = std::mem::take(&mut pending);
        for path in paths {
            match tendi_core::config::read_agent_config(&path) {
                Ok(snapshot) => daemon.emit_event(CONFIG_CHANGED_EVENT, snapshot),
                Err(error) => tendi_core::logging::global().warn(
                    "config change snapshot failed",
                    json!({ "path": path, "error": error.to_string() }),
                ),
            }
        }
    }
}

fn backup_sync_loop(daemon: Daemon) {
    loop {
        thread::sleep(BACKUP_SYNC_INTERVAL);
        daemon.schedule_skill_backup();
    }
}

fn session_watch_loop(daemon: Daemon, receiver: Receiver<notify::Result<Event>>) {
    let runtime = Arc::clone(&daemon.state.session_runtime);
    let mut pending = BTreeSet::new();
    let mut pending_since = None;
    loop {
        match receiver.recv_timeout(SESSION_WATCH_DEBOUNCE) {
            Ok(Ok(event)) => {
                let mut relevant = false;
                for path in event.paths {
                    if let Some(session_root) = advance_session_watcher(&runtime, &path) {
                        relevant = true;
                        pending.extend(tendi_core::sessions::recent_session_paths_in_root(
                            &session_root,
                            None,
                        ));
                    }
                    if tendi_core::sessions::is_session_candidate_path(&path) || !path.exists() {
                        relevant = true;
                        pending.insert(path);
                    }
                }
                if relevant {
                    runtime.watch_revision.fetch_add(1, Ordering::AcqRel);
                }
                if !pending.is_empty() && pending_since.is_none() {
                    pending_since = Some(Instant::now());
                }
            }
            Ok(Err(error)) => {
                let generation = runtime.generation.load(Ordering::SeqCst);
                let message = error.to_string();
                tendi_core::logging::global().error(
                    "session watcher failed",
                    json!({
                        "generation": generation,
                        "phase": "watch",
                        "error": &message,
                    }),
                );
                daemon.emit_event(
                    SESSION_SCAN_EVENT,
                    json!({
                        "generation": generation,
                        "phase": "error",
                        "upserts": [],
                        "deleted": [],
                        "scanned": 0,
                        "complete": false,
                        "error": message,
                    }),
                );
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if pending.is_empty()
            || !pending_since.is_some_and(|started| started.elapsed() >= SESSION_WATCH_DEBOUNCE)
            || runtime.scan_running.load(Ordering::Acquire)
        {
            continue;
        }
        let paths = std::mem::take(&mut pending).into_iter().collect::<Vec<_>>();
        pending_since = None;
        process_session_watch_paths(&daemon, &paths);
    }
}

fn process_session_watch_paths(daemon: &Daemon, paths: &[PathBuf]) {
    let result = (|| -> Result<
        (
            Vec<tendi_core::SessionRecord>,
            Vec<tendi_core::sessions::SessionIdentity>,
            Vec<tendi_core::SessionRecord>,
        ),
        DaemonError,
    > {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let cache = store.session_scan_cache().map_err(core_error)?;
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
            .filter(|path| tendi_core::sessions::is_session_candidate_path(path))
            .filter(|path| !report.sessions.iter().any(|session| session.path == **path))
            .cloned()
            .collect::<Vec<_>>();
        let changed = cache.changed_sessions(&report.sessions);
        let (upserts, deleted) = with_session_database_write_lock(&store, || {
            let mut upserts = store.apply_session_delta(&changed)?;
            store.resolve_session_projects(&mut upserts)?;
            let mut deleted = store.remove_sessions_for_paths(&deleted_paths)?;
            deleted.extend(
                store.remove_sessions_for_paths(&empty_paths)?,
            );
            Ok((upserts, deleted))
        })?;
        Ok((upserts, deleted, analytics_sessions))
    })();

    match result {
        Ok((upserts, deleted, analytics_sessions))
            if !upserts.is_empty() || !deleted.is_empty() || !analytics_sessions.is_empty() =>
        {
            daemon.emit_event(
                SESSION_SCAN_EVENT,
                json!({
                    "generation": daemon.state.session_runtime.generation.load(Ordering::SeqCst),
                    "phase": "watch",
                    "upserts": upserts,
                    "deleted": deleted,
                    "scanned": paths.len(),
                    "complete": true,
                    "error": Value::Null,
                }),
            );
            let _ = daemon
                .state
                .session_runtime
                .analytics_tx
                .send(AnalyticsRefreshJob {
                    phase: "watch",
                    sessions: analytics_sessions,
                });
        }
        Ok(_) => {}
        Err(error) => {
            tendi_core::logging::global().error(
                "session watcher update failed",
                json!({
                    "generation": daemon.state.session_runtime.generation.load(Ordering::SeqCst),
                    "phase": "watch",
                    "paths": paths,
                    "code": &error.code,
                    "error": &error.message,
                }),
            );
            daemon.emit_event(
                SESSION_SCAN_EVENT,
                json!({
                    "generation": daemon.state.session_runtime.generation.load(Ordering::SeqCst),
                    "phase": "error",
                    "upserts": [],
                    "deleted": [],
                    "scanned": paths.len(),
                    "complete": true,
                    "error": error.message,
                }),
            );
        }
    }
}

fn advance_session_watcher(runtime: &Arc<SessionRuntime>, event_path: &Path) -> Option<PathBuf> {
    let mut state = runtime.watcher.lock().ok()?;
    let expansion =
        tendi_core::sessions::session_watch_expansion(&state.dynamic_roots, event_path)?;
    let run_dir = expansion.run_dir;
    if !run_dir.is_dir() {
        return None;
    }
    let agent_home = expansion.agent_home;
    let session_root = expansion.session_root;
    if session_root.is_dir() {
        let newly_watched = watch_session_path(&mut state, &session_root, true);
        unwatch_session_path(&mut state, &run_dir);
        unwatch_session_path(&mut state, &agent_home);
        return newly_watched.then_some(session_root);
    }
    if agent_home.is_dir() {
        watch_session_path(&mut state, &agent_home, false);
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
    let Some(watcher) = state.watcher.as_mut() else {
        return false;
    };
    let mode = if recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
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

fn refresh_session_analytics_serialized(
    daemon: &Daemon,
    phase: &'static str,
    store: &tendi_core::storage::Store,
    sessions: &[tendi_core::SessionRecord],
) -> Result<tendi_core::analytics::AnalyticsRefreshReport, DaemonError> {
    let initial = tendi_core::analytics::AnalyticsRefreshProgress {
        total: sessions.len(),
        ..Default::default()
    };
    daemon.emit_event(
        ANALYTICS_PROGRESS_EVENT,
        json!({
            "phase": phase,
            "completed": initial.completed,
            "total": initial.total,
            "running": true,
            "error": Value::Null,
        }),
    );
    let mut last_progress = initial;
    let result = with_database_write_lock(store, || {
        store.refresh_session_analytics_with_progress(sessions, |progress| {
            last_progress = progress;
            daemon.emit_event(
                ANALYTICS_PROGRESS_EVENT,
                json!({
                    "phase": phase,
                    "completed": progress.completed,
                    "total": progress.total,
                    "running": progress.completed < progress.total,
                    "error": Value::Null,
                }),
            );
        })
    });
    match result {
        Ok(report) => Ok(report),
        Err(error) => {
            let message = error.message.clone();
            daemon.emit_event(
                ANALYTICS_PROGRESS_EVENT,
                json!({
                    "phase": phase,
                    "completed": last_progress.completed,
                    "total": last_progress.total,
                    "running": false,
                    "error": message,
                }),
            );
            Err(core_error(message))
        }
    }
}

fn session_analytics_loop(daemon: Daemon, receiver: Receiver<AnalyticsRefreshJob>) {
    let store = match tendi_core::storage::Store::open_default() {
        Ok(store) => store,
        Err(error) => {
            daemon.emit_event(
                ANALYTICS_PROGRESS_EVENT,
                json!({
                    "phase": "backfill",
                    "completed": 0,
                    "total": 0,
                    "running": false,
                    "error": format!("{error:#}"),
                }),
            );
            return;
        }
    };
    let mut legacy_backfill_complete = false;
    let mut overview_backfill_complete = false;
    let mut last_backfill_revision_emit = Instant::now();
    loop {
        let received = if legacy_backfill_complete && overview_backfill_complete {
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
            let _ = refresh_session_analytics_serialized(&daemon, phase, &store, &sessions);
            if let Ok(revision) = store.analytics_revision() {
                daemon.emit_event(ANALYTICS_REVISION_EVENT, json!({ "revision": revision }));
            }
        }
        if !legacy_backfill_complete {
            let backfill_result = with_database_write_lock(&store, || {
                store.backfill_session_analytics_overview_index_batch(32)
            });
            match backfill_result {
                Ok(report) => {
                    legacy_backfill_complete = report.remaining == 0;
                    if report.processed > 0
                        && (legacy_backfill_complete
                            || last_backfill_revision_emit.elapsed() >= Duration::from_millis(500))
                    {
                        daemon.emit_event(
                            ANALYTICS_REVISION_EVENT,
                            json!({ "revision": report.revision }),
                        );
                        last_backfill_revision_emit = Instant::now();
                    }
                }
                Err(error) => daemon.emit_event(
                    ANALYTICS_PROGRESS_EVENT,
                    json!({
                        "phase": "backfill",
                        "completed": 0,
                        "total": 0,
                        "running": false,
                        "error": error.message,
                    }),
                ),
            }
        }
        if !overview_backfill_complete {
            let overview_result = with_database_write_lock(&store, || {
                store.backfill_session_analytics_overview_batch(32)
            });
            match overview_result {
                Ok(report) => {
                    overview_backfill_complete =
                        report.remaining == 0 || (report.processed == 0 && report.failed > 0);
                }
                Err(error) => daemon.emit_event(
                    ANALYTICS_PROGRESS_EVENT,
                    json!({
                        "phase": "backfill",
                        "completed": 0,
                        "total": 0,
                        "running": false,
                        "error": error.message,
                    }),
                ),
            }
        }
    }
}

fn string_arg(args: &Value, name: &str) -> Result<String, DaemonError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid_argument(format!("missing or empty argument: {name}")))
}

fn string_arg_allow_empty(args: &Value, name: &str) -> Result<String, DaemonError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| invalid_argument(format!("missing argument: {name}")))
}

fn optional_string_arg(args: &Value, name: &str) -> Option<String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn string_vec_arg(args: &Value, name: &str) -> Result<Vec<String>, DaemonError> {
    let values = args
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_argument(format!("missing argument: {name}")))?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .ok_or_else(|| invalid_argument(format!("argument values must be non-empty strings: {name}")))
        })
        .collect()
}

fn string_map_arg(args: &Value, name: &str) -> Result<BTreeMap<String, String>, DaemonError> {
    let Some(value) = args.get(name) else {
        return Ok(BTreeMap::new());
    };
    let object = value
        .as_object()
        .ok_or_else(|| invalid_argument(format!("argument must be an object: {name}")))?;
    object
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key.clone(), value.to_string()))
                .ok_or_else(|| invalid_argument(format!("argument values must be strings: {name}")))
        })
        .collect()
}

fn bool_arg(args: &Value, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
}

fn parse_agent(value: &str) -> Result<tendi_core::AgentKind, DaemonError> {
    tendi_core::parse_agent(value).map_err(core_error)
}

fn hook_delete_request(args: &Value) -> Result<tendi_core::hooks::HookDeleteRequest, DaemonError> {
    serde_json::from_value::<HookInput>(args.clone())
        .map_err(internal_error)?
        .delete_request()
}

fn hook_set_enabled_request(
    args: &Value,
) -> Result<tendi_core::hooks::HookSetEnabledRequest, DaemonError> {
    serde_json::from_value::<HookInput>(args.clone())
        .map_err(internal_error)?
        .set_enabled_request()
}

fn hook_review_request(args: &Value) -> Result<tendi_core::hooks::HookReviewRequest, DaemonError> {
    serde_json::from_value::<HookInput>(args.clone())
        .map_err(internal_error)?
        .review_request()
}

fn mcp_set_enabled_request(
    args: &Value,
) -> Result<tendi_core::mcp::McpSetEnabledRequest, DaemonError> {
    serde_json::from_value(args.clone()).map_err(internal_error)
}

fn update_mcp_projection_for_toggle(
    scan: &mut tendi_core::mcp::McpScan,
    request: &tendi_core::mcp::McpSetEnabledRequest,
    trust_hash: String,
) -> Result<(), DaemonError> {
    let server = scan
        .servers
        .iter_mut()
        .find(|server| {
            server.agent == request.agent
                && server.name == request.name
                && server.path == request.path
        })
        .ok_or_else(|| {
            conflict_error(
                "MCP server disappeared from the current projection while changing it",
            )
        })?;
    server.enabled = request.enabled;
    server.status =
        tendi_core::mcp::mcp_status_after_toggle(request.agent, request.enabled).to_string();
    server.trust_hash = trust_hash;
    Ok(())
}

fn parse_visibility(value: &str) -> Result<tendi_core::SkillVisibility, DaemonError> {
    match value.to_ascii_lowercase().as_str() {
        "auto" => Ok(tendi_core::SkillVisibility::Auto),
        "manual" => Ok(tendi_core::SkillVisibility::Manual),
        "off" => Ok(tendi_core::SkillVisibility::Off),
        _ => Err(invalid_argument(format!("unknown visibility {value}"))),
    }
}

fn invalid_argument(message: impl Into<String>) -> DaemonError {
    DaemonError::new("INVALID_ARGUMENT", message)
}

fn conflict_error(message: impl Into<String>) -> DaemonError {
    DaemonError::new("CONFLICT", message)
}

fn internal_error(message: impl std::fmt::Display) -> DaemonError {
    DaemonError::new("INTERNAL", message.to_string())
}

fn core_error(error: impl std::fmt::Display) -> DaemonError {
    let message = error.to_string();
    let code = if message.contains("refusing to overwrite changed")
        || message.contains("preview expired")
        || message.contains("selection changed")
    {
        "CONFLICT"
    } else if message.contains("path escapes")
        || message.contains("cannot be renamed")
        || message.contains("cannot be deleted")
    {
        "INVALID_PATH"
    } else if message.contains("not found") || message.contains("no skills matched") {
        "NOT_FOUND"
    } else {
        "CORE_ERROR"
    };
    DaemonError::new(code, message)
}

pub fn run_http(
    daemon: Daemon,
    listener: TcpListener,
    token: Option<String>,
) -> std::io::Result<()> {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let daemon = daemon.clone();
        let token = token.clone();
        thread::spawn(move || {
            let _ = handle_connection(stream, &daemon, token.as_deref());
        });
    }
    Ok(())
}

fn handle_connection(
    mut stream: TcpStream,
    daemon: &Daemon,
    token: Option<&str>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut headers = std::collections::BTreeMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    if method == "OPTIONS" {
        return write_http(&mut stream, 204, "{}");
    }
    if method == "GET" && path == "/health" {
        return write_http(
            &mut stream,
            200,
            &serde_json::to_string(&json!({ "ok": true, "cwd": daemon.cwd() }))
                .expect("health serializes"),
        );
    }
    let is_events = method == "GET" && path == "/v1/events";
    let is_log = method == "POST" && path == "/v1/log";
    if !is_events && !is_log && (method != "POST" || path != "/v1/rpc") {
        return write_http(
            &mut stream,
            404,
            &serde_json::to_string(&DaemonResponse::failure(DaemonError::new(
                "NOT_FOUND",
                "not found",
            )))
            .expect("response serializes"),
        );
    }
    if token
        .is_some_and(|expected| headers.get("authorization") != Some(&format!("Bearer {expected}")))
    {
        return write_http(
            &mut stream,
            401,
            &serde_json::to_string(&DaemonResponse::failure(DaemonError::new(
                "UNAUTHORIZED",
                "invalid daemon token",
            )))
            .expect("response serializes"),
        );
    }
    if is_events {
        return handle_event_stream(&mut stream, daemon);
    }
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if length > 64 * 1024 * 1024 {
        return write_http(
            &mut stream,
            413,
            &serde_json::to_string(&DaemonResponse::failure(DaemonError::new(
                "REQUEST_TOO_LARGE",
                "request body is too large",
            )))
            .expect("response serializes"),
        );
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body)?;
    let request = serde_json::from_slice::<Value>(&body).unwrap_or_else(
        |error| json!({ "command": "", "args": { "parseError": error.to_string() } }),
    );
    if is_log {
        let level = request.get("level").and_then(Value::as_str).unwrap_or("");
        let message = request.get("message").and_then(Value::as_str).unwrap_or("");
        let fields = request.get("fields").cloned().unwrap_or_else(|| json!({}));
        let response = match tendi_core::logging::log_event(level, message, fields) {
            Ok(()) => DaemonResponse::success(json!({})),
            Err(error) => {
                DaemonResponse::failure(DaemonError::new("LOG_WRITE_FAILED", error.to_string()))
            }
        };
        return write_http(
            &mut stream,
            200,
            &serde_json::to_string(&response).expect("log response serializes"),
        );
    }
    let response = daemon.handle_json(request);
    write_http(
        &mut stream,
        200,
        &serde_json::to_string(&response).expect("response serializes"),
    )
}

fn handle_event_stream(stream: &mut TcpStream, daemon: &Daemon) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream; charset=utf-8\r\ncache-control: no-cache\r\naccess-control-allow-origin: *\r\naccess-control-allow-headers: content-type, authorization\r\nconnection: keep-alive\r\n\r\n"
    )?;
    stream.flush()?;
    let subscription = daemon.subscribe_events();
    loop {
        match subscription.recv_timeout(Duration::from_secs(15)) {
            Ok(event) => {
                let payload = serde_json::to_string(&event.payload).expect("event serializes");
                write!(
                    stream,
                    "id: {}\nevent: {}\ndata: {}\n\n",
                    event.id, event.event, payload
                )?;
                stream.flush()?;
            }
            Err(RecvTimeoutError::Timeout) => {
                write!(stream, ": keep-alive\n\n")?;
                stream.flush()?;
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn write_http(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json; charset=utf-8\r\ncontent-length: {}\r\ncache-control: no-store\r\naccess-control-allow-origin: *\r\naccess-control-allow-headers: content-type, authorization\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn temp_workspace() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "tendi-daemon-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join(".agents/skills/demo")).unwrap();
        fs::write(
            root.join(".agents/skills/demo/SKILL.md"),
            "---\nname: demo\ndescription: Demo\n---\n\n# Demo\n",
        )
        .unwrap();
        root
    }

    #[test]
    fn string_vec_arg_rejects_invalid_items_instead_of_dropping_them() {
        let args = json!({ "names": ["first", 42, "third"] });
        let error = string_vec_arg(&args, "names").expect_err("invalid vector item must fail");
        assert_eq!(error.code, "INVALID_ARGUMENT");

        let args = json!({ "names": ["first", "", "third"] });
        assert!(string_vec_arg(&args, "names").is_err());
    }


    #[test]
    fn sessions_scan_start_marks_current_scan_as_not_started() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = Daemon::new(root.clone());
        daemon
            .state
            .session_runtime
            .generation
            .store(7, Ordering::SeqCst);
        daemon
            .state
            .session_runtime
            .watch_revision
            .store(3, Ordering::Release);
        daemon
            .state
            .session_runtime
            .completed_revision
            .store(3, Ordering::Release);

        let result = daemon.sessions_scan_start().unwrap();

        assert_eq!(
            result,
            json!({ "generation": 7, "started": false })
        );
        let _ = fs::remove_dir_all(root);
    }

    fn hold_default_database_write_lock() -> (mpsc::Sender<()>, thread::JoinHandle<()>) {
        let store = tendi_core::storage::Store::open_default().unwrap();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = thread::spawn(move || {
            for _ in 0..100 {
                match store.with_database_write_lock(|| {
                    acquired_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok::<_, anyhow::Error>(())
                }) {
                    Ok(Some(())) => return,
                    Ok(None) => thread::sleep(Duration::from_millis(10)),
                    Err(error) => panic!("failed to hold database write lock: {error:#}"),
                }
            }
            panic!("timed out acquiring database write lock for test");
        });
        acquired_rx.recv().unwrap();
        (release_tx, holder)
    }

    #[test]
    fn skill_file_round_trip_and_conflict_are_protocol_errors() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = Daemon::new(root.clone());
        let listed = daemon.handle_json(json!({ "command": "skills_list", "args": {} }));
        assert_eq!(listed["ok"], true);
        let skill_path = root.join(".agents/skills/demo");
        let files = daemon.handle_json(json!({ "command": "skill_files", "args": { "name": "demo", "skillPath": skill_path } }));
        assert_eq!(files["ok"], true);
        let read = daemon.handle_json(json!({ "command": "skill_file_read", "args": { "name": "demo", "relativePath": "SKILL.md", "skillPath": skill_path } }));
        assert_eq!(read["ok"], true);
        let sha = read["result"]["sha256"].as_str().unwrap().to_string();
        let saved = daemon.handle_json(json!({ "command": "skill_file_save", "args": { "name": "demo", "relativePath": "SKILL.md", "expectedSha256": sha, "content": "updated" } }));
        assert_eq!(saved["ok"], true);
        let conflict = daemon.handle_json(json!({ "command": "skill_file_save", "args": { "name": "demo", "relativePath": "SKILL.md", "expectedSha256": "stale", "content": "bad" } }));
        assert_eq!(conflict["ok"], false);
        assert_eq!(conflict["error"]["code"], "CONFLICT");
        let _ = fs::remove_dir_all(root);
    }




    #[test]
    fn direct_reads_do_not_wait_for_database_write_lock() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = Daemon::new(root.clone());
        let skill_path = root.join(".agents/skills/demo");
        let requests = [
            json!({ "command": "settings_get", "args": {} }),
            json!({ "command": "skill_session_links", "args": { "skillName": "demo" } }),
            json!({ "command": "overview_count", "args": { "domain": "sessions" } }),
            json!({ "command": "overview_count", "args": { "domain": "prompts" } }),
            json!({ "command": "skills_targets", "args": {} }),
            json!({
                "command": "skill_files",
                "args": { "name": "demo", "skillPath": skill_path.clone() },
            }),
            json!({
                "command": "skill_file_read",
                "args": {
                    "name": "demo",
                    "relativePath": "SKILL.md",
                    "skillPath": skill_path,
                },
            }),
        ];
        for request in &requests {
            let response = daemon.handle_json(request.clone());
            assert_eq!(response["ok"], true, "warm-up response: {response}");
        }
        let (release_tx, holder) = hold_default_database_write_lock();

        for request in requests {
            let command = request["command"].clone();
            let request_daemon = daemon.clone();
            let (response_tx, response_rx) = mpsc::channel();
            let request_thread = thread::spawn(move || {
                response_tx
                    .send(request_daemon.handle_json(request))
                    .unwrap();
            });
            let response = response_rx.recv_timeout(Duration::from_secs(5));
            request_thread.join().unwrap();
            let response = response.unwrap_or_else(|error| {
                panic!(
                    "request {} waited for the old database authority: {error}",
                    command
                )
            });
            assert_eq!(response["ok"], true, "response: {response}");
        }

        release_tx.send(()).unwrap();
        holder.join().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_update_preview_ignores_unrelated_skill_changes() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let unrelated_skill_file = root.join(".agents/skills/.system/unrelated/SKILL.md");
        let unrelated_user_skill_file = root.join(".agents/skills/unrelated-user/SKILL.md");
        fs::create_dir_all(unrelated_skill_file.parent().unwrap()).unwrap();
        fs::create_dir_all(unrelated_user_skill_file.parent().unwrap()).unwrap();
        fs::write(
            &unrelated_skill_file,
            "---\nname: unrelated\ndescription: Unrelated system skill\n---\n\n# Unrelated\n",
        )
        .unwrap();
        fs::write(
            &unrelated_user_skill_file,
            "---\nname: unrelated-user\ndescription: Unrelated user skill\n---\n\n# Unrelated user\n",
        )
        .unwrap();
        let daemon = Daemon::new(root.clone());
        let listed = daemon.handle_json(json!({ "command": "skills_list", "args": {} }));
        assert_eq!(listed["ok"], true);

        fs::write(
            &unrelated_skill_file,
            "---\nname: unrelated\ndescription: Externally updated system skill\n---\n\n# Unrelated\n",
        )
        .unwrap();
        fs::write(
            &unrelated_user_skill_file,
            "---\nname: unrelated-user\ndescription: Externally updated user skill\n---\n\n# Unrelated user\n",
        )
        .unwrap();
        let preview = daemon.handle_json(json!({
            "command": "skills_update_many",
            "args": { "names": ["demo"], "dryRun": true }
        }));

        assert_eq!(preview["ok"], true, "preview response: {preview}");
        assert!(preview["result"]["previewId"].is_string());
        let store = tendi_core::storage::Store::open_default().unwrap();
        assert_eq!(
            store.projection_status("skills", &root).unwrap(),
            tendi_core::storage::ProjectionStatus::Stale
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_update_preview_refreshes_when_selected_skill_changes() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let selected_skill_file = root.join(".agents/skills/demo/SKILL.md");
        let daemon = Daemon::new(root.clone());
        let listed = daemon.handle_json(json!({ "command": "skills_list", "args": {} }));
        assert_eq!(listed["ok"], true);

        fs::write(
            &selected_skill_file,
            "---\nname: demo\ndescription: Selected skill changed\n---\n\n# Demo\n",
        )
        .unwrap();
        let preview = daemon.handle_json(json!({
            "command": "skills_update_many",
            "args": { "names": ["demo"], "dryRun": true }
        }));

        assert_eq!(preview["ok"], true, "preview response: {preview}");
        let store = tendi_core::storage::Store::open_default().unwrap();
        assert_eq!(
            store.projection_status("skills", &root).unwrap(),
            tendi_core::storage::ProjectionStatus::Stale
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_delete_many_applies_without_a_preview() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let skill_dir = root.join(".agents/skills/demo");
        let daemon = Daemon::new(root.clone());
        let listed = daemon.handle_json(json!({ "command": "skills_list", "args": {} }));
        assert_eq!(listed["ok"], true);

        let response = daemon
            .handle_json(json!({ "command": "skills_delete_many", "args": { "names": ["demo"] } }));

        assert_eq!(response["ok"], true);
        assert!(!skill_dir.exists());
        assert!(
            response["result"]["skills"]
                .as_array()
                .unwrap()
                .iter()
                .all(|skill| skill["name"] != "demo")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_distribution_moves_multiple_skills_in_one_preview() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let second = root.join(".agents/skills/second");
        fs::create_dir_all(&second).unwrap();
        fs::write(
            second.join("SKILL.md"),
            "---\nname: second\ndescription: Second\n---\n\n# Second\n",
        )
        .unwrap();
        let first_source = root.join(".agents/skills/demo");
        let second_source = second.clone();
        let daemon = Daemon::new(root.clone());
        let listed = daemon.handle_json(json!({ "command": "skills_list", "args": {} }));
        assert_eq!(listed["ok"], true);
        let preview = daemon.handle_json(json!({
            "command": "skills_distribute",
            "args": {
                "sourcePaths": [first_source, second_source],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "dryRun": true
            }
        }));
        assert_eq!(preview["ok"], true);
        assert_eq!(preview["result"]["plans"].as_array().unwrap().len(), 2);
        let preview_id = preview["result"]["previewId"].as_str().unwrap();

        let applied = daemon.handle_json(json!({
            "command": "skills_distribute",
            "args": {
                "sourcePaths": [root.join(".agents/skills/demo"), second],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "previewId": preview_id,
                "dryRun": false
            }
        }));
        assert_eq!(applied["ok"], true);
        assert_eq!(applied["result"]["results"].as_array().unwrap().len(), 2);
        assert!(!root.join(".agents/skills/demo").exists());
        assert!(!root.join(".agents/skills/second").exists());
        assert!(root.join(".claude/skills/demo/SKILL.md").is_file());
        assert!(root.join(".claude/skills/second/SKILL.md").is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_distribution_applies_without_preview() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let source = root.join(".agents/skills/demo");
        let daemon = Daemon::new(root.clone());
        let listed = daemon.handle_json(json!({ "command": "skills_list", "args": {} }));
        assert_eq!(listed["ok"], true, "list response: {listed}");

        let applied = daemon.handle_json(json!({
            "command": "skills_distribute",
            "args": {
                "sourcePaths": [source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "dryRun": false
            }
        }));

        assert_eq!(applied["ok"], true, "apply response: {applied}");
        assert_eq!(applied["result"]["plans"][0]["mode"], "move");
        assert!(!source.exists());
        assert!(root.join(".claude/skills/demo/SKILL.md").is_file());
        let _ = fs::remove_dir_all(root);
    }


    #[test]
    fn skill_remove_locations_deletes_only_selected_target() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let source = root.join(".agents/skills/demo");
        let target = root.join(".claude/skills/demo");
        let daemon = Daemon::new(root.clone());
        let listed = daemon.handle_json(json!({ "command": "skills_list", "args": {} }));
        assert_eq!(listed["ok"], true, "list response: {listed}");

        let distributed = daemon.handle_json(json!({
            "command": "skills_distribute",
            "args": {
                "sourcePaths": [source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "copy",
                "dryRun": false
            }
        }));
        assert_eq!(
            distributed["ok"], true,
            "distribution response: {distributed}"
        );
        assert!(source.exists());
        assert!(target.exists());
        assert!(
            distributed["result"]["skills"]
                .as_array()
                .unwrap()
                .iter()
                .any(|skill| skill["name"] == "demo"),
            "distribution response did not include demo: {distributed}"
        );
        let removed = daemon.handle_json(json!({
            "command": "skills_remove_locations",
            "args": {
                "names": ["demo"],
                "targets": ["claude-code"],
                "scope": "project"
            }
        }));
        assert_eq!(removed["ok"], true, "remove response: {removed}");
        assert!(source.exists());
        assert!(!target.exists(), "remove response: {removed}");
        assert_eq!(
            removed["result"]["plan"]["targets"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_distribution_allows_mode_change_after_preview() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let source = root.join(".agents/skills/demo");
        let daemon = Daemon::new(root.clone());

        let preview = daemon.handle_json(json!({
            "command": "skills_distribute",
            "args": {
                "sourcePaths": [source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "symlink",
                "dryRun": true
            }
        }));
        assert_eq!(preview["ok"], true, "preview response: {preview}");
        let preview_id = preview["result"]["previewId"].as_str().unwrap();

        let applied = daemon.handle_json(json!({
            "command": "skills_distribute",
            "args": {
                "sourcePaths": [source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "previewId": preview_id,
                "dryRun": false
            }
        }));
        assert_eq!(applied["ok"], true, "apply response: {applied}");
        assert_eq!(applied["result"]["plans"][0]["mode"], "move");
        assert!(!source.exists());
        assert!(root.join(".claude/skills/demo/SKILL.md").is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_distribution_allows_same_path_alongside_moved_skill() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let same_path = root.join(".claude/skills/demo");
        fs::create_dir_all(&same_path).unwrap();
        fs::write(
            same_path.join("SKILL.md"),
            "---\nname: demo\ndescription: Demo\n---\n\n# Demo\n",
        )
        .unwrap();
        let moved_source = root.join(".agents/skills/second");
        fs::create_dir_all(&moved_source).unwrap();
        fs::write(
            moved_source.join("SKILL.md"),
            "---\nname: second\ndescription: Second\n---\n\n# Second\n",
        )
        .unwrap();
        let daemon = Daemon::new(root.clone());
        let listed = daemon.handle_json(json!({ "command": "skills_list", "args": {} }));
        assert_eq!(listed["ok"], true, "list response: {listed}");

        let preview = daemon.handle_json(json!({
            "command": "skills_distribute",
            "args": {
                "sourcePaths": [same_path.clone(), moved_source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "dryRun": true
            }
        }));
        assert_eq!(preview["ok"], true, "preview response: {preview}");
        let plans = preview["result"]["plans"].as_array().unwrap();
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0]["status"], "already-at-destination");
        assert_eq!(plans[1]["status"], "ready");
        let preview_id = preview["result"]["previewId"].as_str().unwrap();

        let applied = daemon.handle_json(json!({
            "command": "skills_distribute",
            "args": {
                "sourcePaths": [same_path.clone(), moved_source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "previewId": preview_id,
                "dryRun": false
            }
        }));
        assert_eq!(applied["ok"], true, "apply response: {applied}");
        assert!(!moved_source.exists());
        assert!(same_path.join("SKILL.md").is_file());
        assert!(root.join(".claude/skills/second/SKILL.md").is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_mutations_refresh_the_projection_without_a_full_rescan() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let skill_dir = root.join(".agents/skills/demo");
        let daemon = Daemon::new(root.clone());
        let listed = daemon.handle_json(json!({ "command": "skills_list", "args": {} }));
        assert_eq!(listed["ok"], true);

        let visibility = daemon.handle_json(json!({
            "command": "skills_set",
            "args": { "names": ["demo"], "visibility": "manual" }
        }));
        assert_eq!(visibility["ok"], true);
        assert!(
            fs::read_to_string(skill_dir.join("SKILL.md"))
                .unwrap()
                .contains("visibility: manual")
        );

        let folder = daemon.handle_json(json!({
            "command": "skill_folder_create",
            "args": { "name": "demo", "relativePath": "references" }
        }));
        assert_eq!(folder["ok"], true);
        let file = daemon.handle_json(json!({
            "command": "skill_file_create",
            "args": { "name": "demo", "relativePath": "references/notes.md" }
        }));
        assert_eq!(file["ok"], true);
        let renamed = daemon.handle_json(json!({
            "command": "skill_path_rename",
            "args": {
                "name": "demo",
                "fromRelativePath": "references/notes.md",
                "toRelativePath": "references/renamed.md"
            }
        }));
        assert_eq!(renamed["ok"], true);
        let deleted = daemon.handle_json(json!({
            "command": "skill_path_delete",
            "args": { "name": "demo", "relativePath": "references/renamed.md" }
        }));
        assert_eq!(deleted["ok"], true);
        assert!(!skill_dir.join("references/renamed.md").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn overview_counts_do_not_start_cold_projection_scan() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = Daemon::new(root.clone());

        let response = daemon.handle_json(json!({
            "command": "overview_count",
            "args": { "domain": "mcp" },
        }));

        assert_eq!(response["ok"], true);
        assert_eq!(response["result"]["ready"], false);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn mcp_toggle_updates_selected_cached_row_without_full_rescan() {
        let path = PathBuf::from("/tmp/tendi-mcp-config.json");
        let other_path = PathBuf::from("/tmp/tendi-other-mcp-config.json");
        let mut scan = tendi_core::mcp::McpScan {
            servers: vec![
                tendi_core::mcp::McpServerRecord {
                    agent: tendi_core::AgentKind::Claude,
                    name: "demo".to_string(),
                    scope: "global".to_string(),
                    transport: "stdio".to_string(),
                    enabled: true,
                    status: "configured".to_string(),
                    path: path.clone(),
                    trust_hash: "old-demo".to_string(),
                    read_only_reason: None,
                },
                tendi_core::mcp::McpServerRecord {
                    agent: tendi_core::AgentKind::Claude,
                    name: "other".to_string(),
                    scope: "global".to_string(),
                    transport: "stdio".to_string(),
                    enabled: true,
                    status: "configured".to_string(),
                    path: other_path,
                    trust_hash: "old-other".to_string(),
                    read_only_reason: None,
                },
            ],
            warnings: Vec::new(),
        };
        let request = tendi_core::mcp::McpSetEnabledRequest {
            agent: tendi_core::AgentKind::Claude,
            path,
            expected_trust_hash: "old-demo".to_string(),
            name: "demo".to_string(),
            enabled: false,
        };

        update_mcp_projection_for_toggle(&mut scan, &request, "new-demo".to_string()).unwrap();

        assert_eq!(scan.servers[0].status, "disabled");
        assert!(!scan.servers[0].enabled);
        assert_eq!(scan.servers[0].trust_hash, "new-demo");
        assert_eq!(scan.servers[1].status, "configured");
        assert!(scan.servers[1].enabled);
        assert_eq!(scan.servers[1].trust_hash, "old-other");
    }

    #[test]
    fn unknown_method_is_explicit() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let daemon = Daemon::new(temp_workspace());
        let response = daemon.handle_json(json!({ "command": "not_implemented", "args": {} }));
        assert_eq!(response["error"]["code"], "METHOD_NOT_FOUND");
    }

    #[test]
    fn event_subscription_preserves_shared_envelope() {
        let hub = EventHub {
            next_id: Arc::new(AtomicU64::new(0)),
            subscribers: Arc::new(Mutex::new(Vec::new())),
        };
        let subscription = hub.subscribe();
        hub.publish("analytics://revision", json!({ "revision": 42 }));
        let event = subscription
            .recv_timeout(Duration::from_secs(1))
            .expect("event should be delivered");
        assert_eq!(event.id, 1);
        assert_eq!(event.event, "analytics://revision");
        assert_eq!(event.payload["revision"], 42);
    }

    #[test]
    fn session_database_write_lock_waits_for_another_owner() {
        let root = temp_workspace();
        let db = root.join("tendi.sqlite3");
        let store_a = tendi_core::storage::Store::open(&db).unwrap();
        let store_b = tendi_core::storage::Store::open(&db).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = thread::spawn(move || {
            store_a
                .with_database_write_lock(|| {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok::<_, anyhow::Error>(())
                })
                .unwrap()
        });
        started_rx.recv().unwrap();

        let contender = thread::spawn(move || {
            with_session_database_write_lock(&store_b, || Ok::<_, anyhow::Error>(42)).unwrap()
        });
        thread::sleep(Duration::from_millis(100));
        release_tx.send(()).unwrap();

        assert_eq!(contender.join().unwrap(), 42);
        assert_eq!(holder.join().unwrap(), Some(()));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn transcript_read_does_not_wait_for_database_write_lock() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let transcript = root.join("session.jsonl");
        fs::write(
            &transcript,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}"#,
        )
        .unwrap();
        let daemon = Daemon::new(root.clone());
        let store = tendi_core::storage::Store::open_default().unwrap();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = thread::spawn(move || {
            for _ in 0..100 {
                match store.with_database_write_lock(|| {
                    acquired_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok::<_, anyhow::Error>(())
                }) {
                    Ok(Some(())) => return,
                    Ok(None) => thread::sleep(Duration::from_millis(10)),
                    Err(error) => panic!("failed to hold database write lock: {error:#}"),
                }
            }
            panic!("timed out acquiring database write lock for test");
        });
        acquired_rx.recv().unwrap();

        let request_daemon = daemon.clone();
        let path = transcript.display().to_string();
        let (response_tx, response_rx) = mpsc::channel();
        let request = thread::spawn(move || {
            let response = request_daemon.handle_json(json!({
                "command": "session_transcript",
                "args": { "path": path, "agent": "codex", "limit": 1 }
            }));
            response_tx.send(response).unwrap();
        });
        let response = response_rx.recv_timeout(Duration::from_millis(300));
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        request.join().unwrap();

        let response = response.expect("transcript read should not wait for database write lock");
        assert_eq!(response["ok"], true);
        assert_eq!(response["result"]["items"].as_array().unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }
}
