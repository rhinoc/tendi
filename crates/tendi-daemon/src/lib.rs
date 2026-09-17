use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use serde_json::{Value, json};

mod cancellable_job;
mod operation_coordinator;
mod preview_store;
mod request_scheduler;
mod rpc_admission;
use operation_coordinator::OperationCoordinator;
use preview_store::PreviewStore;
use tendi_core::generated::runtime_contract as runtime_schema;

include!("generated/runtime_dispatch.rs");

pub const SESSION_SCAN_EVENT: &str = runtime_schema::EventName::SessionsScan.as_str();
pub const ANALYTICS_PROGRESS_EVENT: &str = runtime_schema::EventName::AnalyticsProgress.as_str();
pub const ANALYTICS_REVISION_EVENT: &str = runtime_schema::EventName::AnalyticsRevision.as_str();
pub const SKILL_UPDATE_EVENT: &str = runtime_schema::EventName::SkillsUpdates.as_str();
pub const SKILL_CHANGED_EVENT: &str = runtime_schema::EventName::SkillsChanged.as_str();
pub const PROJECTION_CHANGED_EVENT: &str = runtime_schema::EventName::ProjectionChanged.as_str();
pub const CONFIG_CHANGED_EVENT: &str = runtime_schema::EventName::ConfigChanged.as_str();
const SESSION_SCAN_BATCH_SIZE: usize = 32;
const SESSION_SCAN_PERSIST_BATCH_SIZE: usize = 8;
const SESSION_WATCH_DEBOUNCE: Duration = Duration::from_millis(500);
const CONFIG_WATCH_DEBOUNCE: Duration = Duration::from_millis(150);
const BACKUP_SYNC_INTERVAL: Duration = Duration::from_secs(10 * 60);
const SESSION_WATCH_RETRY_INITIAL: Duration = Duration::from_millis(500);
const SESSION_WATCH_RETRY_MAX: Duration = Duration::from_secs(30);
const DATABASE_RECOVERY_RETRY_INITIAL: Duration = Duration::from_secs(1);
const DATABASE_RECOVERY_RETRY_MAX: Duration = Duration::from_secs(30);
const DATABASE_RECOVERY_WAIT: Duration = Duration::from_secs(2);
const DATABASE_RECOVERY_SUCCESS_COOLDOWN: Duration = Duration::from_secs(2);
const SKILL_RECONCILIATION_RETRY_INITIAL: Duration = Duration::from_secs(2);
const SKILL_RECONCILIATION_RETRY_MAX: Duration = Duration::from_secs(30);

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

pub type DaemonEvent = runtime_schema::RuntimeEventEnvelope;

#[derive(Debug, Clone)]
struct EventHub {
    next_id: Arc<AtomicU64>,
    state: Arc<Mutex<EventHubState>>,
}

#[derive(Debug, Default)]
struct EventHubState {
    history: VecDeque<DaemonEvent>,
    subscribers: Vec<Sender<DaemonEvent>>,
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

fn runtime_event(event: &str, payload: Value) -> runtime_schema::RuntimeEventPayload {
    match event {
        SESSION_SCAN_EVENT => runtime_schema::RuntimeEventPayload::SessionsScan(
            serde_json::from_value(payload).expect("session scan event matches generated schema"),
        ),
        ANALYTICS_PROGRESS_EVENT => runtime_schema::RuntimeEventPayload::AnalyticsProgress(
            serde_json::from_value(payload)
                .expect("analytics progress event matches generated schema"),
        ),
        ANALYTICS_REVISION_EVENT => runtime_schema::RuntimeEventPayload::AnalyticsRevision(
            serde_json::from_value(payload)
                .expect("analytics revision event matches generated schema"),
        ),
        SKILL_UPDATE_EVENT => runtime_schema::RuntimeEventPayload::SkillsUpdates(
            serde_json::from_value(payload).expect("skills update event matches generated schema"),
        ),
        SKILL_CHANGED_EVENT => runtime_schema::RuntimeEventPayload::SkillsChanged(
            serde_json::from_value(payload).expect("skills changed event matches generated schema"),
        ),
        PROJECTION_CHANGED_EVENT => runtime_schema::RuntimeEventPayload::ProjectionChanged(
            serde_json::from_value(payload)
                .expect("projection changed event matches generated schema"),
        ),
        CONFIG_CHANGED_EVENT => runtime_schema::RuntimeEventPayload::ConfigChanged(
            serde_json::from_value(payload).expect("config changed event matches generated schema"),
        ),
        _ => panic!("unsupported runtime event: {event}"),
    }
}

impl EventHub {
    fn subscribe(&self) -> DaemonEventSubscription {
        self.subscribe_from(None)
    }

    fn subscribe_from(&self, last_event_id: Option<u64>) -> DaemonEventSubscription {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut state) = self.state.lock() {
            if let Some(last_event_id) = last_event_id {
                for event in state
                    .history
                    .iter()
                    .filter(|event| event.id > last_event_id)
                {
                    let _ = sender.send(event.clone());
                }
            }
            state.subscribers.push(sender);
        }
        DaemonEventSubscription { receiver }
    }

    fn publish_revisioned(
        &self,
        event: &str,
        payload: Value,
        scope_key: &tendi_core::ScopeKey,
        domain: &str,
        operation_id: &tendi_core::OperationId,
        base_revision: tendi_core::Revision,
        revision: tendi_core::Revision,
        source_version: Option<&tendi_core::SourceVersion>,
    ) {
        self.publish_with_metadata(
            event,
            payload,
            Some(scope_key.as_str().to_string()),
            Some(domain.to_string()),
            Some(operation_id.as_str().to_string()),
            Some(base_revision.value()),
            Some(revision.value()),
            source_version.map(|value| value.as_str().to_string()),
        );
    }

    fn publish_with_metadata(
        &self,
        event: &str,
        payload: Value,
        scope_key: Option<String>,
        domain: Option<String>,
        operation_id: Option<String>,
        base_revision: Option<u64>,
        revision: Option<u64>,
        source_version: Option<String>,
    ) {
        let Some(payload) = payload.as_object().cloned() else {
            tendi_core::logging::global().error(
                "runtime event payload must be an object",
                json!({ "event": event }),
            );
            return;
        };
        let event = DaemonEvent {
            id: self.next_id.fetch_add(1, Ordering::Relaxed) + 1,
            event: event.to_string(),
            payload,
            scope_key,
            domain,
            operation_id,
            base_revision,
            revision,
            source_version,
        };
        let contract_event = runtime_schema::RuntimeEventEnvelope {
            id: event.id,
            event: event.event.clone(),
            payload: event.payload.clone(),
            scope_key: event.scope_key.clone(),
            domain: event.domain.clone(),
            operation_id: event.operation_id.clone(),
            base_revision: event.base_revision,
            revision: event.revision,
            source_version: event.source_version.clone(),
        };
        if let Err(error) = runtime_schema::validate_event(&contract_event) {
            tendi_core::logging::global()
                .error("runtime event contract failed", json!({ "error": error }));
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.history.push_back(event.clone());
        while state.history.len() > 256 {
            state.history.pop_front();
        }
        state
            .subscribers
            .retain(|subscriber| subscriber.send(event.clone()).is_ok());
    }
}

#[derive(Debug, Clone)]
struct AnalyticsRefreshJob {
    phase: &'static str,
    scope_key: tendi_core::ScopeKey,
    sessions: Vec<tendi_core::SessionRecord>,
}

#[derive(Default, Debug)]
struct SessionWatcherState {
    watcher: Option<RecommendedWatcher>,
    watched_paths: BTreeSet<PathBuf>,
    dynamic_roots: Vec<PathBuf>,
}

#[derive(Debug)]
struct SessionWatchRetryState {
    paths: BTreeSet<PathBuf>,
    retry_at: Option<Instant>,
    delay: Duration,
}

impl Default for SessionWatchRetryState {
    fn default() -> Self {
        Self {
            paths: BTreeSet::new(),
            retry_at: None,
            delay: SESSION_WATCH_RETRY_INITIAL,
        }
    }
}

#[derive(Debug)]
struct SessionRuntime {
    generation: AtomicU64,
    scan_running: AtomicBool,
    watch_revision: AtomicU64,
    completed_revision: AtomicU64,
    watcher: Mutex<SessionWatcherState>,
    retry: Mutex<SessionWatchRetryState>,
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

#[derive(Default, Debug)]
struct SkillWatcherState {
    watcher: Option<RecommendedWatcher>,
    watched_paths: BTreeSet<PathBuf>,
}

#[derive(Debug)]
struct SkillRuntime {
    watcher: Mutex<SkillWatcherState>,
    watch_tx: Sender<notify::Result<Event>>,
}

#[derive(Debug)]
struct SkillAddPreview {
    options: tendi_core::skills::SkillAddOptions,
    plan: tendi_core::skills::SkillAddPlan,
    source_fingerprint: String,
}

#[derive(Debug)]
struct SkillUpdatePreview {
    skill_ids: Vec<String>,
    plan: tendi_core::skills::SkillUpdatePlan,
}

#[derive(Debug, Clone)]
struct SkillUpdateCheckCache {
    projection_revision: tendi_core::Revision,
    reports: Vec<tendi_core::skills::SkillUpdateReport>,
    skill_fingerprints: BTreeMap<String, String>,
    checked_at: Instant,
}

const SKILL_UPDATE_REPORT_CACHE_TTL: Duration = Duration::from_secs(15);

#[derive(Debug)]
struct SkillDistributionPreview {
    sources: Vec<PathBuf>,
    target: tendi_core::SkillTarget,
    scope: tendi_core::SkillInstallScope,
    plans: Vec<tendi_core::skills::SkillDistributionPlan>,
}

#[derive(Debug)]
struct StorageRecoveryState {
    in_progress: bool,
    retry_at: Option<Instant>,
    retry_delay: Duration,
    last_completed_at: Option<Instant>,
    last_succeeded: bool,
}

impl Default for StorageRecoveryState {
    fn default() -> Self {
        Self {
            in_progress: false,
            retry_at: None,
            retry_delay: DATABASE_RECOVERY_RETRY_INITIAL,
            last_completed_at: None,
            last_succeeded: false,
        }
    }
}

#[derive(Debug, Default)]
struct StorageRecoveryRuntime {
    state: Mutex<StorageRecoveryState>,
    changed: Condvar,
}

#[derive(Debug)]
struct SkillReconciliationBackoff {
    retry_at: Instant,
    delay: Duration,
    failure_count: u32,
    blocked_until_event: bool,
}

#[derive(Debug)]
struct DaemonState {
    background_enabled: bool,
    cwd: PathBuf,
    database_path: PathBuf,
    storage_recovery: StorageRecoveryRuntime,
    events: EventHub,
    requests: request_scheduler::RequestScheduler,
    session_operations: OperationCoordinator,
    analytics_operations: OperationCoordinator,
    projection_refreshes: Mutex<BTreeSet<String>>,
    skill_reconciliation_backoff: Mutex<BTreeMap<PathBuf, SkillReconciliationBackoff>>,
    session_runtime: Arc<SessionRuntime>,
    config_runtime: Arc<ConfigRuntime>,
    skill_runtime: Arc<SkillRuntime>,
    session_skill_index_running: AtomicBool,
    skill_update: cancellable_job::CancellableJob,
    backup_sync_dirty: AtomicBool,
    backup_sync_running: AtomicBool,
    add_preview: Mutex<PreviewStore<SkillAddPreview>>,
    update_preview: Mutex<PreviewStore<SkillUpdatePreview>>,
    skill_update_check: Mutex<Option<SkillUpdateCheckCache>>,
    distribution_preview: Mutex<PreviewStore<SkillDistributionPreview>>,
    preview_sequence: Mutex<u64>,
}

#[derive(Debug)]
struct DaemonLifecycle {
    shutdown: AtomicBool,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl DaemonLifecycle {
    fn new() -> Self {
        Self {
            shutdown: AtomicBool::new(false),
            workers: Mutex::new(Vec::new()),
        }
    }

    fn is_shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    fn shutdown_and_join(&self) {
        if self.shutdown.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut workers) = self.workers.lock() {
            for worker in workers.drain(..) {
                let _ = worker.join();
            }
        }
    }
}

#[derive(Debug)]
pub struct Daemon {
    state: Arc<DaemonState>,
    lifecycle: Arc<DaemonLifecycle>,
    owner: bool,
    #[cfg(test)]
    test_database_path: Option<PathBuf>,
}

impl Clone for Daemon {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            lifecycle: Arc::clone(&self.lifecycle),
            owner: false,
            #[cfg(test)]
            test_database_path: None,
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if self.owner {
            self.shutdown();
            #[cfg(test)]
            if let Some(path) = self.test_database_path.take() {
                cleanup_test_database(&path);
            }
        }
    }
}

impl Daemon {
    pub fn new(cwd: PathBuf) -> Self {
        Self::with_database(
            cwd,
            tendi_core::storage::default_db_path().expect("default database path"),
            true,
        )
    }

    /// Embedders own whether watchers and recovery start; storage identity is
    /// injected independently and is used by every request and background job.
    pub fn with_database(cwd: PathBuf, database_path: PathBuf, start_background: bool) -> Self {
        let (watch_tx, watch_rx) = mpsc::channel();
        let (config_watch_tx, config_watch_rx) = mpsc::channel();
        let (skill_watch_tx, skill_watch_rx) = mpsc::channel();
        let (analytics_tx, analytics_rx) = mpsc::channel();
        let events = EventHub {
            next_id: Arc::new(AtomicU64::new(0)),
            state: Arc::new(Mutex::new(EventHubState::default())),
        };
        let session_runtime = Arc::new(SessionRuntime {
            generation: AtomicU64::new(0),
            scan_running: AtomicBool::new(false),
            watch_revision: AtomicU64::new(0),
            completed_revision: AtomicU64::new(0),
            watcher: Mutex::new(SessionWatcherState::default()),
            retry: Mutex::new(SessionWatchRetryState::default()),
            watch_tx,
            analytics_tx,
        });
        let config_runtime = Arc::new(ConfigRuntime {
            watcher: Mutex::new(ConfigWatcherState::default()),
            watch_tx: config_watch_tx,
        });
        let skill_runtime = Arc::new(SkillRuntime {
            watcher: Mutex::new(SkillWatcherState::default()),
            watch_tx: skill_watch_tx,
        });
        let lifecycle = Arc::new(DaemonLifecycle::new());
        let daemon = Self {
            state: Arc::new(DaemonState {
                background_enabled: start_background,
                cwd,
                database_path,
                storage_recovery: StorageRecoveryRuntime::default(),
                events,
                requests: request_scheduler::RequestScheduler::default(),
                session_operations: OperationCoordinator::named("session-metadata"),
                analytics_operations: OperationCoordinator::named("analytics"),
                projection_refreshes: Mutex::new(BTreeSet::new()),
                skill_reconciliation_backoff: Mutex::new(BTreeMap::new()),
                session_runtime,
                config_runtime,
                skill_runtime,
                session_skill_index_running: AtomicBool::new(false),
                skill_update: cancellable_job::CancellableJob::default(),
                backup_sync_dirty: AtomicBool::new(true),
                backup_sync_running: AtomicBool::new(false),
                add_preview: Mutex::new(PreviewStore::default()),
                update_preview: Mutex::new(PreviewStore::default()),
                skill_update_check: Mutex::new(None),
                distribution_preview: Mutex::new(PreviewStore::default()),
                preview_sequence: Mutex::new(0),
            }),
            lifecycle: Arc::clone(&lifecycle),
            owner: true,
            #[cfg(test)]
            test_database_path: None,
        };
        if !start_background {
            return daemon;
        }
        if let Ok(recovered) = daemon
            .open_store()
            .and_then(|store| store.recover_inflight_operations())
        {
            if recovered > 0 {
                tendi_core::logging::global().warn(
                    "recovered unfinished operations",
                    json!({ "count": recovered }),
                );
            }
        }
        let watch_daemon = daemon.clone();
        let watch_worker = thread::spawn(move || session_watch_loop(watch_daemon, watch_rx));
        let analytics_daemon = daemon.clone();
        let analytics_worker =
            thread::spawn(move || session_analytics_loop(analytics_daemon, analytics_rx));
        let search_daemon = daemon.clone();
        let search_worker = thread::Builder::new()
            .name("tendi-session-search".to_string())
            .spawn(move || session_search_loop(search_daemon))
            .expect("session search worker must start");
        let config_daemon = daemon.clone();
        let config_worker =
            thread::spawn(move || config_watch_loop(config_daemon, config_watch_rx));
        let skill_daemon = daemon.clone();
        let skill_worker = thread::spawn(move || skill_watch_loop(skill_daemon, skill_watch_rx));
        let backup_daemon = daemon.clone();
        let backup_worker = thread::spawn(move || backup_sync_loop(backup_daemon));
        let projection_daemon = daemon.clone();
        let projection_worker = thread::spawn(move || projection_recovery_loop(projection_daemon));
        lifecycle
            .workers
            .lock()
            .expect("daemon worker registry is healthy")
            .extend([
                watch_worker,
                analytics_worker,
                search_worker,
                config_worker,
                skill_worker,
                backup_worker,
                projection_worker,
            ]);
        daemon.initialize_config_watcher();
        daemon
    }

    pub fn shutdown(&self) {
        self.state.skill_update.cancel();
        self.lifecycle.shutdown_and_join();
        self.state.requests.shutdown();
        self.state.session_operations.shutdown();
        self.state.analytics_operations.shutdown();
    }

    fn open_store(&self) -> anyhow::Result<tendi_core::storage::Store> {
        let result = tendi_core::storage::Store::open(&self.state.database_path);
        match result {
            Ok(store) => Ok(store),
            Err(error) if tendi_core::storage::is_database_io_error(&error) => {
                if self.recover_storage(&error) {
                    tendi_core::storage::Store::open(&self.state.database_path)
                } else {
                    Err(error)
                }
            }
            Err(error) => Err(error),
        }
    }

    fn recover_storage(&self, reason: impl std::fmt::Display) -> bool {
        let reason = reason.to_string();
        let now = Instant::now();
        let recovery = &self.state.storage_recovery;
        let Ok(mut state) = recovery.state.lock() else {
            tendi_core::logging::global().error(
                "database recovery state is unavailable",
                json!({ "reason": reason }),
            );
            return false;
        };
        if state.in_progress {
            let deadline = now + DATABASE_RECOVERY_WAIT;
            while state.in_progress {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return false;
                }
                let Ok((next, _)) = recovery.changed.wait_timeout(state, remaining) else {
                    return false;
                };
                state = next;
            }
            return state.last_succeeded
                && state
                    .last_completed_at
                    .is_some_and(|completed_at| completed_at >= now);
        }
        if state.last_succeeded
            && state
                .last_completed_at
                .is_some_and(|completed_at| completed_at + DATABASE_RECOVERY_SUCCESS_COOLDOWN > now)
        {
            return true;
        }
        if state.retry_at.is_some_and(|retry_at| retry_at > now) {
            return false;
        }
        state.in_progress = true;
        drop(state);

        tendi_core::logging::global().warn(
            "database connection recovery started",
            json!({ "database": self.state.database_path, "reason": reason }),
        );
        let started = Instant::now();
        let result = tendi_core::storage::recover_database(&self.state.database_path);
        let Ok(mut state) = recovery.state.lock() else {
            return result.is_ok();
        };
        state.in_progress = false;
        state.last_completed_at = Some(Instant::now());
        state.last_succeeded = result.is_ok();
        recovery.changed.notify_all();
        match result {
            Ok(()) => {
                state.retry_at = None;
                state.retry_delay = DATABASE_RECOVERY_RETRY_INITIAL;
                tendi_core::logging::global().info(
                    "database connection recovery completed",
                    json!({
                        "database": self.state.database_path,
                        "durationMs": started.elapsed().as_secs_f64() * 1000.0,
                    }),
                );
                true
            }
            Err(error) => {
                let retry_delay = state.retry_delay;
                state.retry_at = Some(Instant::now() + retry_delay);
                state.retry_delay =
                    std::cmp::min(retry_delay.saturating_mul(2), DATABASE_RECOVERY_RETRY_MAX);
                tendi_core::logging::global().error(
                    "database connection recovery failed",
                    json!({
                        "database": self.state.database_path,
                        "durationMs": started.elapsed().as_secs_f64() * 1000.0,
                        "retryAfterMs": retry_delay.as_secs_f64() * 1000.0,
                        "error": error.to_string(),
                    }),
                );
                false
            }
        }
    }

    fn recover_storage_error(&self, error: &DaemonError) -> bool {
        let is_storage_error = error
            .data
            .as_ref()
            .and_then(|data| data.get("category"))
            .and_then(Value::as_str)
            == Some("storage");
        if is_storage_error {
            return self.recover_storage(&error.message);
        }
        false
    }

    fn is_shutting_down(&self) -> bool {
        self.lifecycle.is_shutting_down()
    }

    pub fn cwd(&self) -> &Path {
        &self.state.cwd
    }

    pub fn subscribe_events(&self) -> DaemonEventSubscription {
        self.state.events.subscribe()
    }

    fn emit_event(&self, event: &str, payload: runtime_schema::RuntimeEventPayload) {
        if event != payload.event_name() {
            tendi_core::logging::global().error(
                "runtime event name does not match payload",
                json!({ "event": event, "payloadEvent": payload.event_name() }),
            );
            return;
        }
        let payload = payload.into_json();
        {
            let scope_key = daemon_scope_key(self).ok();
            let domain = event_projection_domain(event, &payload);
            let revision = domain
                .as_deref()
                .and_then(|domain| {
                    let store = self.open_store().ok()?;
                    match store.projection_head(scope_key.as_ref()?, domain) {
                        Ok(head) => head.map(|head| head.revision.value()),
                        Err(error) => {
                            if tendi_core::storage::is_database_io_error(&error) {
                                self.recover_storage(&error);
                            }
                            None
                        }
                    }
                })
                .unwrap_or_default();
            self.state.events.publish_with_metadata(
                event,
                payload,
                scope_key.map(|scope| scope.as_str().to_string()),
                domain,
                None,
                None,
                Some(revision),
                None,
            );
        }
    }

    fn emit_revisioned_event(
        &self,
        event: &str,
        scope_key: &tendi_core::ScopeKey,
        domain: &str,
        operation_id: &tendi_core::OperationId,
        base_revision: tendi_core::Revision,
        revision: tendi_core::Revision,
        source_version: Option<&tendi_core::SourceVersion>,
        payload: runtime_schema::RuntimeEventPayload,
    ) {
        if event != payload.event_name() {
            tendi_core::logging::global().error(
                "runtime event name does not match payload",
                json!({ "event": event, "payloadEvent": payload.event_name() }),
            );
            return;
        }
        self.state.events.publish_revisioned(
            event,
            payload.into_json(),
            scope_key,
            domain,
            operation_id,
            base_revision,
            revision,
            source_version,
        );
    }

    fn execute_method(&self, method: &str, params: &Value) -> Result<Value, DaemonError> {
        let Some(workload) = request_scheduler::workload_for_request(method, params) else {
            return self.dispatch(method, params);
        };
        let trace_skill_change = matches!(
            method,
            "skills_update" | "skills_update_many" | "skills_refresh" | "skills_updates"
        );
        let started = Instant::now();

        let operation_id = tendi_core::OperationId::new(format!(
            "rpc-{}-{}",
            method,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
        .map_err(|error| internal_error(error.to_string()))?;
        let daemon = self.clone();
        let method = method.to_string();
        let params = params.clone();
        let journal_operation = should_record_runtime_operation(&method, &params);
        let journal_operation_id = operation_id.clone();
        if tendi_core::logging::global().debug_enabled() {
            tendi_core::logging::global().debug(
                "runtime operation request",
                json!({
                    "operationId": operation_id.as_str(),
                    "method": &method,
                    "journalOperation": journal_operation,
                    "dryRun": params.get("dryRun").and_then(Value::as_bool),
                    "skillCount": params.get("skillIds").and_then(Value::as_array).map(Vec::len),
                    "hasPreviewId": params.get("previewId").is_some(),
                }),
            );
        }
        if trace_skill_change {
            tendi_core::logging::global().info(
                "skill runtime operation started",
                json!({
                    "operationId": operation_id.as_str(),
                    "method": &method,
                    "workload": format!("{workload:?}"),
                    "dryRun": params.get("dryRun").and_then(Value::as_bool),
                    "skillCount": params.get("skillIds").and_then(Value::as_array).map(Vec::len),
                    "hasPreviewId": params.get("previewId").is_some(),
                }),
            );
        }
        let admission_daemon = self.clone();
        let admission_method = method.clone();
        let admission_params = params.clone();
        let execution_method = method.clone();
        let result = match self.state.requests.execute_class(
            workload,
            operation_id.clone(),
            admission_daemon.prepare_rpc_step(
                admission_method,
                admission_params,
                workload,
                Box::new(move |prepared: Option<rpc_admission::PreparedExecute>| {
                    let journal_store = if journal_operation {
                        let journal_scope = daemon_scope_key(&daemon).ok();
                        journal_scope.as_ref().and_then(|scope| {
                            let store = daemon.open_store().ok()?;
                            let input_revision =
                                runtime_operation_input_revision(&store, scope, &execution_method);
                            let record = tendi_core::OperationRecord {
                                operation_id: journal_operation_id.clone(),
                                kind: tendi_core::OperationKind::Projection,
                                scope_key: scope.clone(),
                                status: tendi_core::OperationStatus::Running,
                                input_revision,
                                source_version: None,
                                checkpoint_json: None,
                                error: None,
                            };
                            store.record_operation(&record).map_err(core_error).ok()?;
                            Some(store)
                        })
                    } else {
                        None
                    };
                    let result = match prepared {
                        Some(run) => run(),
                        None => daemon.dispatch(&execution_method, &params),
                    };
                    if let Some(store) = journal_store {
                        let (status, error) = match &result {
                            Ok(_) => (tendi_core::OperationStatus::Committed, None),
                            Err(error) => (
                                tendi_core::OperationStatus::Failed,
                                Some(error.message.as_str()),
                            ),
                        };
                        let _ = store
                            .update_operation(&journal_operation_id, status, None, error)
                            .map_err(core_error);
                    }
                    result
                }),
            ),
        ) {
            Ok(result) => result,
            Err(error) => {
                let error = internal_error(format!(
                    "request execution capacity is unavailable: {error:?}"
                ));
                if trace_skill_change {
                    tendi_core::logging::global().error(
                        "skill runtime operation admission failed",
                        json!({
                            "operationId": operation_id.as_str(),
                            "method": &method,
                            "durationMs": started.elapsed().as_secs_f64() * 1000.0,
                            "error": &error.message,
                        }),
                    );
                }
                return Err(error);
            }
        };

        if trace_skill_change {
            let level = if result.is_ok() { "info" } else { "error" };
            let fields = json!({
                "operationId": operation_id.as_str(),
                "method": &method,
                "durationMs": started.elapsed().as_secs_f64() * 1000.0,
                "succeeded": result.is_ok(),
            });
            if level == "info" {
                tendi_core::logging::global().info("skill runtime operation completed", fields);
            } else {
                tendi_core::logging::global().error("skill runtime operation failed", fields);
            }
        }

        result
    }

    /// JSON-RPC 2.0 is the only daemon wire envelope.
    pub fn handle_json_rpc(&self, value: Value) -> Value {
        let id = value
            .as_object()
            .and_then(|object| object.get("id"))
            .cloned()
            .unwrap_or(Value::Null);
        let Some(object) = value.as_object() else {
            return rpc_error_response(
                id,
                -32600,
                "INVALID_REQUEST",
                "request must be an object",
                None,
            );
        };
        let allowed = ["jsonrpc", "id", "method", "params"];
        if object.keys().any(|key| !allowed.contains(&key.as_str())) {
            return rpc_error_response(
                id,
                -32600,
                "INVALID_REQUEST",
                "unknown JSON-RPC request field",
                None,
            );
        }
        let request = match serde_json::from_value::<runtime_schema::JsonRpcRequest>(value) {
            Ok(request) => request,
            Err(error) => {
                return rpc_error_response(id, -32600, "INVALID_REQUEST", &error.to_string(), None);
            }
        };
        if request.jsonrpc != "2.0" {
            return rpc_error_response(id, -32600, "INVALID_REQUEST", "jsonrpc must be 2.0", None);
        }
        if !valid_json_rpc_id(&request.id) {
            return rpc_error_response(
                Value::Null,
                -32600,
                "INVALID_REQUEST",
                "id must be a string, integer, or null",
                None,
            );
        }
        if !request.params.is_object() {
            return rpc_error_response(
                id,
                -32602,
                "INVALID_PARAMS",
                "params must be an object",
                None,
            );
        }
        let Some(metadata) = runtime_schema::command_metadata(&request.method) else {
            return rpc_error_response(
                id,
                -32601,
                "METHOD_NOT_FOUND",
                &format!("unsupported daemon method: {}", request.method),
                None,
            );
        };
        if metadata.owner != runtime_schema::Owner::Daemon {
            return rpc_error_response(
                id,
                -32004,
                "UNSUPPORTED_TRANSPORT",
                "method is not owned by the daemon",
                None,
            );
        }
        if let Err(message) = runtime_schema::validate_request(&request.method, &request.params) {
            return rpc_error_response(id, -32602, "INVALID_PARAMS", &message, None);
        }

        let operation_started = Instant::now();
        let operation_result = self.execute_method(&request.method, &request.params);
        let operation_duration_ms = operation_started.elapsed().as_secs_f64() * 1000.0;
        if operation_duration_ms >= 100.0 || operation_result.is_err() {
            let fields = json!({
                "requestId": request.id.clone(),
                "method": &request.method,
                "durationMs": operation_duration_ms,
                "succeeded": operation_result.is_ok(),
                "scheduled": request_scheduler::workload_for_request(&request.method, &request.params).is_some(),
            });
            if operation_result.is_ok() {
                tendi_core::logging::global().info("runtime operation completed", fields);
            } else {
                tendi_core::logging::global().warn("runtime operation failed", fields);
            }
        }

        match operation_result {
            Ok(result) => {
                if let Err(message) = runtime_schema::validate_result(&request.method, &result) {
                    return rpc_error_response(
                        request.id.clone(),
                        -32005,
                        "CONTRACT_VIOLATION",
                        &message,
                        None,
                    );
                }
                serde_json::to_value(runtime_schema::JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    result: Some(result),
                    error: None,
                })
                .expect("JSON-RPC response serializes")
            }
            Err(error) => {
                let numeric_code = rpc_error_code(&error.code);
                let data = runtime_schema::JsonRpcErrorData {
                    kind: error.code,
                    details: error.data,
                };
                serde_json::to_value(runtime_schema::JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    result: None,
                    error: Some(runtime_schema::JsonRpcError {
                        code: numeric_code,
                        message: error.message,
                        data: Some(data),
                    }),
                })
                .expect("JSON-RPC error serializes")
            }
        }
    }

    fn dispatch(&self, command: &str, args: &Value) -> Result<Value, DaemonError> {
        let result = runtime_dispatch!(self, command, args);
        if let Err(error) = &result {
            let read_command = runtime_schema::command_metadata(command)
                .is_some_and(|metadata| metadata.execution == runtime_schema::Execution::Read);
            if read_command && self.recover_storage_error(error) {
                let retry = runtime_dispatch!(self, command, args);
                if let Err(error) = &retry {
                    self.recover_storage_error(error);
                }
                return retry;
            }
            self.recover_storage_error(error);
        }
        result
    }

    /// Retry only the pure projection merge. Filesystem changes and network
    /// probes have already completed and are never replayed by this loop.
    fn merge_projection<T, Merge, Save>(
        &self,
        domain: &str,
        mut merge: Merge,
        save: Save,
    ) -> Result<T, DaemonError>
    where
        T: serde::de::DeserializeOwned,
        Merge: FnMut(T) -> Result<T, DaemonError>,
        Save: Fn(
            &tendi_core::storage::Store,
            &Path,
            &T,
            tendi_core::Revision,
        ) -> anyhow::Result<bool>,
    {
        let store = self.open_store().map_err(core_error)?;
        for _ in 0..16 {
            let (revision, current) = store
                .read_cached_projection_with_revision::<T>(domain, &self.state.cwd)
                .map_err(core_error)?;
            let current = current.ok_or_else(|| {
                conflict_error(format!(
                    "{domain} projection is unavailable; refresh before applying changes"
                ))
            })?;
            // A stale head can point to a snapshot preceding some other file
            // change. Refresh that entire base before merging this operation;
            // otherwise a partial merge would incorrectly mark it all fresh.
            let current = if store
                .projection_status(domain, &self.state.cwd)
                .map_err(core_error)?
                != tendi_core::storage::ProjectionStatus::Fresh
            {
                self.prepare_projection_base::<T>(&store, domain)?
            } else {
                current
            };
            let updated = merge(current)?;
            if save(&store, &self.state.cwd, &updated, revision).map_err(core_error)? {
                return Ok(updated);
            }
        }
        Err(conflict_error(format!(
            "{domain} projection kept changing; refresh and retry"
        )))
    }

    fn prepare_projection_base<T: serde::de::DeserializeOwned>(
        &self,
        store: &tendi_core::storage::Store,
        domain: &str,
    ) -> Result<T, DaemonError> {
        let roots = Self::registered_project_roots(store).map_err(core_error)?;
        let value = match domain {
            "skills" => serde_json::to_value(
                tendi_core::skills::scan_skills_for_project_roots_with_store(
                    &self.state.cwd,
                    store,
                    &roots,
                )
                .map_err(core_error)?,
            ),
            "rules" => serde_json::to_value(
                tendi_core::rules::scan_rules_for_project_roots(&self.state.cwd, &roots)
                    .map_err(core_error)?,
            ),
            "hooks" => serde_json::to_value(
                tendi_core::hooks::scan_hooks(&self.state.cwd).map_err(core_error)?,
            ),
            "mcp" => serde_json::to_value(
                tendi_core::mcp::scan_mcp_for_project_roots(&self.state.cwd, &roots)
                    .map_err(core_error)?,
            ),
            _ => {
                return Err(internal_error(format!(
                    "unsupported projection merge: {domain}"
                )));
            }
        }
        .map_err(internal_error)?;
        serde_json::from_value(value).map_err(internal_error)
    }

    fn read_cached_projection<T>(&self, domain: &'static str) -> Result<Option<T>, DaemonError>
    where
        T: serde::de::DeserializeOwned,
    {
        let cwd = self.state.cwd.clone();
        let store = self.open_store().map_err(core_error)?;
        let cached = store
            .read_cached_projection(domain, &cwd)
            .map_err(core_error)?;
        if store.projection_status(domain, &cwd).map_err(core_error)?
            != tendi_core::storage::ProjectionStatus::Fresh
        {
            self.schedule_projection_refresh(domain);
        }
        Ok(cached)
    }

    fn schedule_projection_refresh(&self, domain: &'static str) {
        if !self.state.background_enabled || self.is_shutting_down() {
            return;
        }
        let should_schedule = self
            .state
            .projection_refreshes
            .lock()
            .map(|mut refreshes| refreshes.insert(domain.to_string()))
            .unwrap_or(false);
        if !should_schedule {
            return;
        }

        let operation_id = match tendi_core::OperationId::new(format!(
            "projection-refresh-{domain}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )) {
            Ok(operation_id) => operation_id,
            Err(error) => {
                self.clear_projection_refresh(domain);
                tendi_core::logging::global().error(
                    "projection refresh operation id is invalid",
                    json!({ "domain": domain, "error": error.to_string() }),
                );
                return;
            }
        };
        let daemon = self.clone();
        let cleanup_daemon = self.clone();
        let cleanup = rpc_admission::Cleanup(Some(Box::new(move || {
            cleanup_daemon.clear_projection_refresh(domain);
        })));
        let store = match self.open_store() {
            Ok(store) => store,
            Err(error) => {
                self.clear_projection_refresh(domain);
                tendi_core::logging::global().warn(
                    "projection refresh store unavailable",
                    json!({ "domain": domain, "error": error.to_string() }),
                );
                return;
            }
        };
        let projection_key = if domain == "skills" {
            tendi_core::coordination::shared_projection_key(domain)
        } else {
            tendi_core::coordination::projection_key(domain, &self.state.cwd)
        };
        let resources = vec![tendi_core::coordination::ResourceRequest::named(
            store.path(),
            projection_key,
        )];
        let workload = if domain == "mcp" {
            request_scheduler::Workload::ExternalIo
        } else {
            request_scheduler::Workload::Compute
        };
        let step = request_scheduler::Step::acquire(workload, resources, move || {
            let _cleanup = cleanup;
            let result = daemon.refresh_projection_domain(domain);
            match result {
                Ok(true) => daemon.emit_event(
                    PROJECTION_CHANGED_EVENT,
                    runtime_event(
                        PROJECTION_CHANGED_EVENT,
                        json!({ "domain": domain, "error": Value::Null }),
                    ),
                ),
                Ok(false) => {}
                Err(error) => daemon.emit_event(
                    PROJECTION_CHANGED_EVENT,
                    runtime_event(
                        PROJECTION_CHANGED_EVENT,
                        json!({ "domain": domain, "error": error.message }),
                    ),
                ),
            }
            Ok(request_scheduler::Step::Complete(()))
        });
        if self
            .state
            .requests
            .submit(operation_id, step, Arc::new(AtomicBool::new(false)))
            .is_err()
        {
            self.clear_projection_refresh(domain);
            tendi_core::logging::global().warn(
                "projection refresh queue is full",
                json!({ "domain": domain }),
            );
        }
    }

    fn clear_projection_refresh(&self, domain: &str) {
        if let Ok(mut refreshes) = self.state.projection_refreshes.lock() {
            refreshes.remove(domain);
        }
    }

    fn skill_reconciliation_retry_ready(&self, workspace: &Path) -> bool {
        let workspace = tendi_core::storage::canonical_workspace_root(workspace);
        self.state
            .skill_reconciliation_backoff
            .lock()
            .map(|backoff| {
                backoff
                    .get(&workspace)
                    .map(|state| !state.blocked_until_event && Instant::now() >= state.retry_at)
                    .unwrap_or(true)
            })
            .unwrap_or(false)
    }

    fn record_skill_reconciliation_failure(&self, workspace: &Path, error: &str) {
        let workspace = tendi_core::storage::canonical_workspace_root(workspace);
        let Ok(mut backoffs) = self.state.skill_reconciliation_backoff.lock() else {
            tendi_core::logging::global().error(
                "skill reconciliation failure backoff unavailable",
                json!({ "workspace": workspace, "error": error }),
            );
            return;
        };
        let state =
            backoffs
                .entry(workspace.clone())
                .or_insert_with(|| SkillReconciliationBackoff {
                    retry_at: Instant::now(),
                    delay: SKILL_RECONCILIATION_RETRY_INITIAL,
                    failure_count: 0,
                    blocked_until_event: false,
                });
        state.failure_count = state.failure_count.saturating_add(1);
        state.blocked_until_event = error.contains("failed to parse ");
        let delay = state.delay;
        state.retry_at = Instant::now() + delay;
        state.delay = std::cmp::min(delay.saturating_mul(2), SKILL_RECONCILIATION_RETRY_MAX);
        tendi_core::logging::global().error(
            "skill reconciliation failed; retry backed off",
            json!({
                "workspace": workspace,
                "error": error,
                "failureCount": state.failure_count,
                "retryInMs": delay.as_secs_f64() * 1000.0,
                "blockedUntilEvent": state.blocked_until_event,
            }),
        );
    }

    fn clear_skill_reconciliation_backoff(&self, workspace: &Path) {
        let workspace = tendi_core::storage::canonical_workspace_root(workspace);
        if let Ok(mut backoffs) = self.state.skill_reconciliation_backoff.lock() {
            backoffs.remove(&workspace);
        }
    }

    fn refresh_projection_domain(&self, domain: &str) -> Result<bool, DaemonError> {
        match domain {
            "agents" => {
                self.agents_projection()?;
            }
            "skills" => {
                self.skill_projection()?;
            }
            "rules" => {
                self.rules_projection()?;
            }
            "hooks" => {
                self.hooks_projection()?;
            }
            "mcp" => {
                self.refresh_mcp_projection()?;
            }
            _ => {
                return Err(invalid_argument(format!(
                    "unsupported projection domain: {domain}"
                )));
            }
        }
        Ok(true)
    }

    fn ensure_projection<T, Ready, Refresh>(
        &self,
        domain: &str,
        ready: Ready,
        mut refresh: Refresh,
    ) -> Result<T, DaemonError>
    where
        Ready: Fn(&tendi_core::storage::Store) -> anyhow::Result<Option<T>>,
        Refresh: FnMut(&tendi_core::storage::Store, tendi_core::Revision) -> anyhow::Result<T>,
    {
        for _ in 0..16 {
            let store = self.open_store().map_err(core_error)?;
            if let Some(value) = ready(&store).map_err(core_error)? {
                return Ok(value);
            }
            let scope = daemon_scope_key(self)?;
            let revision = store
                .projection_head(&scope, domain)
                .map_err(core_error)?
                .map(|head| head.revision)
                .unwrap_or(tendi_core::Revision::new(0));
            match refresh(&store, revision) {
                Err(error) if error.to_string() == "projection changed during preparation" => {
                    continue;
                }
                result => return result.map_err(core_error),
            }
        }
        Err(internal_error(format!(
            "{domain} projection changed repeatedly during preparation"
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

    fn agents_projection(&self) -> Result<tendi_core::agents::AgentScan, DaemonError> {
        let cwd = self.state.cwd.clone();
        self.ensure_projection(
            "agents",
            |store| store.list_agents_for_workspace(&cwd),
            |store, revision| {
                let report = tendi_core::agents::scan_agents(&cwd)?;
                anyhow::ensure!(
                    store.save_agents_for_workspace_if_revision(&cwd, &report, revision)?,
                    "projection changed during preparation"
                );
                Ok(report)
            },
        )
    }

    fn agents_list(&self) -> Result<runtime_schema::AgentRecordList, DaemonError> {
        let report = self.read_cached_projection::<tendi_core::agents::AgentScan>("agents")?;
        serde_json::from_value(
            serde_json::to_value(report.map(|report| report.agents).unwrap_or_default())
                .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn scan(&self) -> Result<runtime_schema::ScanResponse, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        for domain in ["agents", "rules", "hooks", "mcp"] {
            store
                .invalidate_projection(domain, &self.state.cwd)
                .map_err(core_error)?;
        }
        let daemon = self.clone();
        let sessions = self
            .state
            .session_operations
            .execute(
                tendi_core::OperationId::new("scan-session-metadata").expect("static operation id"),
                move || {
                    let store = daemon.open_store()?;
                    let roots = store
                        .app_settings()?
                        .additional_session_roots
                        .into_iter()
                        .map(PathBuf::from)
                        .collect::<Vec<_>>();
                    let report = tendi_core::sessions::scan_sessions_with_additional_roots(
                        &daemon.state.cwd,
                        &roots,
                    )?;
                    let scope = daemon_scope_key(&daemon)
                        .map_err(|error| anyhow::anyhow!(error.message))?;
                    store.apply_session_delta_and_resolve_projects_for_scope(
                        &scope,
                        &report.sessions,
                    )?;
                    Ok(report)
                },
            )
            .map_err(|error| internal_error(format!("session scan admission failed: {error:?}")))?
            .map_err(core_error)?;
        let report = tendi_core::ScanReport {
            agents: self.agents_projection()?,
            skills: self.scan_and_persist()?,
            sessions,
            rules: self.rules_projection()?,
            hooks: self.hooks_projection()?,
            mcp: self.mcp_projection()?,
        };
        serde_json::from_value(serde_json::to_value(report).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn bundled_skill_status(
        &self,
    ) -> Result<runtime_schema::BundledSkillStatusResponse, DaemonError> {
        serde_json::from_value(
            serde_json::to_value(
                tendi_core::bundled_skill::status(tendi_core::AgentKind::Shared)
                    .map_err(core_error)?,
            )
            .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn bundled_skill_install(
        &self,
        request: runtime_schema::BundledSkillInstallRequest,
    ) -> Result<runtime_schema::BundledSkillInstallResponse, DaemonError> {
        let agent = bundled_skill_agent(request.agent);
        let overwrite = request.overwrite.unwrap_or(false);
        let before = self.skill_projection_for_mutation()?;
        let report =
            tendi_core::bundled_skill::install(agent, overwrite, false).map_err(core_error)?;
        let skill_path = PathBuf::from(&report.status.target);
        let refresh_ids = Vec::new();
        let refreshed =
            self.refresh_skill_projection(before, &refresh_ids, std::slice::from_ref(&skill_path))?;
        let updated = skills_matching_paths(&refreshed.skills, std::slice::from_ref(&skill_path));
        let mut value = serde_json::to_value(report).map_err(internal_error)?;
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "updated".to_string(),
                serde_json::to_value(updated).map_err(internal_error)?,
            );
        }
        serde_json::from_value(value).map_err(internal_error)
    }

    fn bundled_skill_remove(
        &self,
    ) -> Result<runtime_schema::BundledSkillRemoveResponse, DaemonError> {
        serde_json::from_value(
            serde_json::to_value(
                tendi_core::bundled_skill::remove(tendi_core::AgentKind::Shared)
                    .map_err(core_error)?,
            )
            .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn bundled_skill_prompt_dismiss(
        &self,
    ) -> Result<runtime_schema::BundledSkillPromptDismissResponse, DaemonError> {
        tendi_core::bundled_skill::dismiss_prompt().map_err(core_error)?;
        Ok(None)
    }

    fn terminal_apps_list(&self) -> Result<runtime_schema::TerminalAppRecordList, DaemonError> {
        fn app_available(paths: &[&str]) -> bool {
            paths.iter().any(|path| Path::new(path).exists())
        }
        fn command_available(name: &str) -> bool {
            let in_path = std::env::var_os("PATH").is_some_and(|path| {
                std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
            });
            in_path
                || std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .is_some_and(|home| home.join(".superset/bin").join(name).is_file())
        }

        let apps = vec![
            runtime_schema::TerminalAppRecord {
                id: "auto".to_string(),
                label: "Auto".to_string(),
                available: true,
            },
            runtime_schema::TerminalAppRecord {
                id: "terminal".to_string(),
                label: "Terminal".to_string(),
                available: app_available(&[
                    "/System/Applications/Utilities/Terminal.app",
                    "/Applications/Utilities/Terminal.app",
                ]),
            },
            runtime_schema::TerminalAppRecord {
                id: "iterm".to_string(),
                label: "iTerm".to_string(),
                available: app_available(&["/Applications/iTerm.app", "/Applications/iTerm2.app"]),
            },
            runtime_schema::TerminalAppRecord {
                id: "ghostty".to_string(),
                label: "Ghostty".to_string(),
                available: app_available(&["/Applications/Ghostty.app"]),
            },
            runtime_schema::TerminalAppRecord {
                id: "warp".to_string(),
                label: "Warp".to_string(),
                available: app_available(&["/Applications/Warp.app"]),
            },
            runtime_schema::TerminalAppRecord {
                id: "orca".to_string(),
                label: "Orca".to_string(),
                available: app_available(&["/Applications/Orca.app"]),
            },
            runtime_schema::TerminalAppRecord {
                id: "superset".to_string(),
                label: "Superset".to_string(),
                available: app_available(&["/Applications/Superset.app"])
                    && command_available("superset"),
            },
        ];
        Ok(apps)
    }

    fn sessions_snapshot(&self) -> Result<runtime_schema::SessionSnapshot, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let scope_key = daemon_scope_key(self)?;
        let (revision, scan) = store
            .session_snapshot_for_scope(&scope_key)
            .map_err(core_error)?;
        let revision = revision.value();
        let rows = scan.sessions;
        let value = json!({
            "scopeKey": scope_key,
            "domain": "sessions",
            "revision": revision,
            "schemaVersion": 1,
            "snapshotId": format!("sessions:{}:{}", scope_key, revision),
            "payload": rows,
        });
        serde_json::from_value(value).map_err(|error| {
            DaemonError::new(
                "CONTRACT_VIOLATION",
                format!("sessions snapshot encode failed: {error}"),
            )
        })
    }

    fn sessions_list(
        &self,
        request: runtime_schema::SessionsListRequest,
    ) -> Result<runtime_schema::SessionsListResponse, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let scope_key = daemon_scope_key(self)?;
        let (revision, page) = store
            .session_page_with_revision_for_scope(
                &scope_key,
                tendi_core::storage::SessionListQuery {
                    query: request.query,
                    agent: request.agent.map(agent_kind_from_request),
                    sort_key: session_list_sort_key(request.sort_key),
                    sort_direction: session_list_sort_direction(request.sort_direction),
                    group_by: request.group_by.map(session_list_sort_key),
                    page: usize::try_from(request.page)
                        .map_err(|_| invalid_argument("session list page is too large"))?,
                    page_size: usize::try_from(request.page_size)
                        .map_err(|_| invalid_argument("session list page size is too large"))?,
                    show_child_sessions: request.show_child_sessions,
                    selected_project_keys: request.selected_project_keys,
                    locate: request.locate.map(session_identity_from_request),
                },
            )
            .map_err(core_error)?;
        let mut value = serde_json::to_value(page).map_err(internal_error)?;
        value["revision"] = json!(revision.value());
        serde_json::from_value(value).map_err(|error| {
            DaemonError::new(
                "CONTRACT_VIOLATION",
                format!("session list encode failed: {error}"),
            )
        })
    }

    fn sessions_scan_start(&self) -> Result<runtime_schema::SessionScanStartResponse, DaemonError> {
        let additional_session_roots = {
            let store = self.open_store().map_err(core_error)?;
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
        let scope_key = daemon_scope_key(self)?;
        let store = self.open_store().map_err(core_error)?;
        let input_revision = store
            .projection_head(&scope_key, "sessions")
            .map_err(core_error)?
            .map(|head| head.revision.value())
            .unwrap_or(0);
        let operation_id = tendi_core::OperationId::new(format!("session-scan-{generation}"))
            .map_err(|error| core_error(anyhow::anyhow!(error)))?;
        let operation_scope = scope_key;
        let operation_id_for_job = operation_id.clone();
        let operation_scope_for_job = operation_scope.clone();
        let daemon = self.clone();
        if let Err(error) = self.state.session_operations.submit(operation_id, move || {
            if let Ok(store) = daemon.open_store() {
                let _ = store
                    .record_operation(&tendi_core::OperationRecord {
                        operation_id: operation_id_for_job.clone(),
                        kind: tendi_core::OperationKind::Scan,
                        scope_key: operation_scope_for_job.clone(),
                        status: tendi_core::OperationStatus::Queued,
                        input_revision: tendi_core::Revision::new(input_revision),
                        source_version: None,
                        checkpoint_json: None,
                        error: None,
                    })
                    .map_err(core_error);
                let _ = store
                    .update_operation(
                        &operation_id_for_job,
                        tendi_core::OperationStatus::Running,
                        None,
                        None,
                    )
                    .map_err(core_error);
            }
            let result = run_session_scan(
                &daemon,
                generation,
                &additional_session_roots,
                &operation_id_for_job,
            );
            let operation_error = result.as_ref().err().map(|error| error.message.clone());
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
            if let Ok(store) = daemon.open_store() {
                let status = if result.is_ok() {
                    tendi_core::OperationStatus::Committed
                } else {
                    tendi_core::OperationStatus::Failed
                };
                let _ = store
                    .update_operation(
                        &operation_id_for_job,
                        status,
                        None,
                        operation_error.as_deref(),
                    )
                    .map_err(core_error);
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
                    runtime_event(
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
                    ),
                );
            }
            daemon
                .state
                .session_runtime
                .scan_running
                .store(false, Ordering::SeqCst);
        }) {
            runtime.scan_running.store(false, Ordering::SeqCst);
            return Err(internal_error(format!(
                "failed to queue session scan: {error:?}"
            )));
        }
        Ok(session_scan_start_response(generation, true))
    }

    fn sessions_search(
        &self,
        request: runtime_schema::SessionsSearchRequest,
    ) -> Result<runtime_schema::SessionSearchHitList, DaemonError> {
        let candidates = request.candidates.map(|values| {
            values
                .into_iter()
                .map(session_identity_from_request)
                .collect::<Vec<_>>()
        });
        let store = self.open_store().map_err(core_error)?;
        let scope_key = daemon_scope_key(self)?;
        let hits = store
            .search_sessions_for_scope(&scope_key, &request.query, candidates.as_deref())
            .map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(hits).map_err(internal_error)?).map_err(
            |error| {
                DaemonError::new(
                    "CONTRACT_VIOLATION",
                    format!("session search encode failed: {error}"),
                )
            },
        )
    }

    fn analytics_overview(
        &self,
        request: runtime_schema::AnalyticsOverviewRequest,
    ) -> Result<runtime_schema::AnalyticsOverview, DaemonError> {
        let agent = request.agent.as_deref().map(parse_agent).transpose()?;
        let days = request.days as u32;
        let rank_days = request.rank_days as u32;
        let end_date = request.end_date.as_deref();
        let store = self.open_store().map_err(core_error)?;
        let refresh_transcripts = request.refresh_transcripts;
        if refresh_transcripts {
            let sessions = store
                .list_sessions_for_scope(&daemon_scope_key(self)?)
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
                    scope_key: daemon_scope_key(self)?,
                    sessions,
                });
        }
        let overview = store
            .overview_analytics_for_scope_until(
                &daemon_scope_key(self)?,
                agent,
                days,
                rank_days,
                end_date,
            )
            .map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(overview).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn analytics_revision(&self) -> Result<runtime_schema::Revision, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let scope_key = daemon_scope_key(self)?;
        let revision = store
            .projection_head(&scope_key, "analytics")
            .map_err(core_error)?
            .map(|head| head.revision.value())
            .unwrap_or_default();
        Ok(revision)
    }

    fn session_skill_index_status(
        &self,
    ) -> Result<runtime_schema::SessionSkillIndexStatus, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let scope_key = daemon_scope_key(self)?;
        let status = store
            .session_skill_index_status_for_scope(
                &scope_key,
                self.state
                    .session_skill_index_running
                    .load(Ordering::Acquire),
            )
            .map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(status).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn session_skill_index_run(
        &self,
        request: runtime_schema::SessionSkillIndexRunRequest,
    ) -> Result<runtime_schema::SessionSkillIndexStatus, DaemonError> {
        if self
            .state
            .session_skill_index_running
            .swap(true, Ordering::AcqRel)
        {
            return self.session_skill_index_status();
        }
        let force = request.force;
        let scope_key = daemon_scope_key(self)?;
        let result =
            tendi_core::session_skills::run_index_for_scope(&self.state.cwd, &scope_key, force)
                .map_err(core_error);
        self.state
            .session_skill_index_running
            .store(false, Ordering::Release);
        result?;
        self.session_skill_index_status()
    }

    fn session_skill_links(
        &self,
        request: runtime_schema::SessionSkillLinksRequest,
    ) -> Result<runtime_schema::SessionSkillLinkList, DaemonError> {
        let session_id = request.session_id;
        let agent = agent_kind_from_request(request.agent);
        let store = self.open_store().map_err(core_error)?;
        let scope_key = daemon_scope_key(self)?;
        serde_json::from_value(
            serde_json::to_value(
                store
                    .session_skill_links_for_scope(&scope_key, &session_id, agent)
                    .map_err(core_error)?,
            )
            .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn skill_session_links(
        &self,
        request: runtime_schema::SkillSessionLinksRequest,
    ) -> Result<runtime_schema::SessionSkillLinkList, DaemonError> {
        let skill_id = request.skill_id;
        let scan = self.skill_projection_for_ids(std::slice::from_ref(&skill_id))?;
        let skill_paths = scan
            .skills
            .iter()
            .find(|skill| tendi_core::skills::skill_matches_id(skill, &skill_id))
            .map(|skill| {
                skill
                    .paths
                    .iter()
                    .map(|path| path.path.clone())
                    .collect::<Vec<_>>()
            })
            .ok_or_else(|| conflict_error(format!("unknown skill id: {skill_id}")))?;
        let store = self.open_store().map_err(core_error)?;
        let scope_key = daemon_scope_key(self)?;
        serde_json::from_value(
            serde_json::to_value(
                store
                    .skill_session_links_for_scope(&scope_key, &skill_paths)
                    .map_err(core_error)?,
            )
            .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn settings_get(&self) -> Result<runtime_schema::AppSettings, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        serde_json::from_value(
            serde_json::to_value(store.app_settings().map_err(core_error)?)
                .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn settings_save(
        &self,
        request: runtime_schema::AppSettingsPatch,
    ) -> Result<runtime_schema::AppSettings, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let saved = store.patch_app_settings(request).map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(saved).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn session_projects_list(
        &self,
    ) -> Result<runtime_schema::SessionProjectSummaryList, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let scope_key = daemon_scope_key(self)?;
        serde_json::from_value(
            serde_json::to_value(
                store
                    .list_session_projects_for_scope(&scope_key)
                    .map_err(core_error)?,
            )
            .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn project_scan_scopes_list(
        &self,
    ) -> Result<runtime_schema::ProjectScanScopeList, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        serde_json::from_value(
            serde_json::to_value(store.project_scan_scopes().map_err(core_error)?)
                .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn project_scan_scopes_save(
        &self,
        request: runtime_schema::ProjectScanScopesSaveRequest,
    ) -> Result<runtime_schema::ProjectScanScopeList, DaemonError> {
        let paths = request.paths;
        let store = self.open_store().map_err(core_error)?;
        let saved = store
            .save_project_scan_scopes(paths.clone())
            .map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(saved).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn projects_list(&self) -> Result<runtime_schema::ProjectRecordList, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        serde_json::from_value(
            serde_json::to_value(store.list_projects().map_err(core_error)?)
                .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn projects_scan(&self) -> Result<runtime_schema::ProjectsScanResponse, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let cwd = self.state.cwd.clone();
        let result = store
            .scan_projects_for_workspace(&cwd)
            .map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(result).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn agent_configs_list(&self) -> Result<runtime_schema::AgentConfigFileList, DaemonError> {
        serde_json::from_value(
            serde_json::to_value(tendi_core::config::list_agent_configs().map_err(core_error)?)
                .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn agent_config_watch(
        &self,
        request: runtime_schema::AgentConfigWatchRequest,
    ) -> Result<runtime_schema::AgentConfigWatchResponse, DaemonError> {
        let path = PathBuf::from(request.path);
        tendi_core::config::read_agent_config(&path).map_err(core_error)?;
        self.register_config_watch_path(&path)?;
        Ok(runtime_schema::AgentConfigWatchResponse {
            path: path.to_string_lossy().into_owned(),
        })
    }

    fn agent_config_read(
        &self,
        request: runtime_schema::AgentConfigPathRequest,
    ) -> Result<runtime_schema::AgentConfigContent, DaemonError> {
        serde_json::from_value(
            serde_json::to_value(
                tendi_core::config::read_agent_config(Path::new(&request.path))
                    .map_err(core_error)?,
            )
            .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn agent_config_save(
        &self,
        request: runtime_schema::AgentConfigSaveRequest,
    ) -> Result<runtime_schema::AgentConfigWriteResult, DaemonError> {
        let _resources =
            tendi_core::coordination::acquire_file_resources(&[PathBuf::from(&request.path)])
                .map_err(core_error)?;
        match tendi_core::config::save_agent_config(
            Path::new(&request.path),
            &request.expected_sha256,
            &request.content,
        ) {
            Ok(saved) => {
                self.invalidate_config_projections()?;
                serde_json::from_value(serde_json::to_value(saved).map_err(internal_error)?)
                    .map_err(internal_error)
            }
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

    fn agent_configs_delete_many(
        &self,
        request: runtime_schema::AgentConfigsDeleteRequest,
    ) -> Result<runtime_schema::AgentConfigDeleteResult, DaemonError> {
        let paths = request
            .paths
            .into_iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let configs = tendi_core::config::list_agent_configs().map_err(core_error)?;
        let _resources =
            tendi_core::coordination::acquire_file_resources(&paths).map_err(core_error)?;
        tendi_core::config::delete_agent_configs(&paths).map_err(core_error)?;

        let store = self.open_store().map_err(core_error)?;
        let mut removed_profiles = Vec::new();
        for config in configs.iter().filter(|config| paths.contains(&config.path)) {
            let Some(profile) = config.profile.as_deref() else {
                continue;
            };
            let Some(key) = tendi_core::config_profile_key(config.agent) else {
                continue;
            };
            removed_profiles.push((key.to_string(), profile.to_string()));
        }
        let settings = store
            .clear_config_profiles_if_matching(&removed_profiles)
            .map_err(core_error)?;
        self.invalidate_config_projections()?;
        let remaining = configs
            .into_iter()
            .filter_map(|mut config| {
                if !paths.contains(&config.path) {
                    return Some(config);
                }
                if config.profile.is_some() {
                    return None;
                }
                config.exists = false;
                config.updated_at = None;
                Some(config)
            })
            .collect::<Vec<_>>();
        serde_json::from_value(json!({
            "configs": remaining,
            "deleted": paths,
            "configProfiles": settings.config_profiles,
        }))
        .map_err(internal_error)
    }

    fn config_profile_create(
        &self,
        request: runtime_schema::ConfigProfileCreateRequest,
    ) -> Result<runtime_schema::AgentConfigFile, DaemonError> {
        let agent = agent_kind_from_request(request.agent);
        serde_json::from_value(
            serde_json::to_value(
                tendi_core::config::create_config_profile(agent, &request.name, &request.content)
                    .map_err(core_error)?,
            )
            .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn config_profile_set(
        &self,
        request: runtime_schema::ConfigProfileSetRequest,
    ) -> Result<runtime_schema::AppSettings, DaemonError> {
        let agent = agent_kind_from_request(request.agent);
        let profile = request.profile;
        let key = tendi_core::config_profile_key(agent)
            .ok_or_else(|| invalid_argument("config profiles are not supported for this agent"))?;
        let _resources = profile
            .as_deref()
            .map(|name| {
                let path = tendi_core::config::config_profile_path(agent, name)?;
                tendi_core::coordination::acquire_file_resources(&[path])
            })
            .transpose()
            .map_err(core_error)?;
        if let Some(name) = profile.as_deref() {
            tendi_core::config::validate_profile_name(name).map_err(core_error)?;
            if !tendi_core::config::config_profile_exists(agent, name).map_err(core_error)? {
                return Err(DaemonError::new(
                    "NOT_FOUND",
                    format!("config profile not found: {name}"),
                ));
            }
        }
        let store = self.open_store().map_err(core_error)?;
        let saved = store
            .set_config_profile(key, profile.as_deref())
            .map_err(core_error)?;
        self.invalidate_config_projections()?;
        serde_json::from_value(serde_json::to_value(saved).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn rules_list(&self) -> Result<runtime_schema::RuleRecordList, DaemonError> {
        let report = self.read_cached_projection::<tendi_core::rules::RuleScan>("rules")?;
        serde_json::from_value(
            serde_json::to_value(report.map(|report| report.rules).unwrap_or_default())
                .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn rule_file_read(
        &self,
        request: runtime_schema::RuleFileReadRequest,
    ) -> Result<runtime_schema::RuleFileReadResponse, DaemonError> {
        required_request_text(&request.path, "path")?;
        let path = Path::new(&request.path);
        serde_json::from_value(
            serde_json::to_value(
                tendi_core::rules::read_rule_file_at_path(path).map_err(core_error)?,
            )
            .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn rule_file_save(
        &self,
        request: runtime_schema::RuleFileSaveRequest,
    ) -> Result<runtime_schema::RuleFileSaveResponse, DaemonError> {
        required_request_text(&request.path, "path")?;
        required_request_text(&request.expected_sha256, "expectedSha256")?;
        let path = request.path;
        let expected = request.expected_sha256;
        let content = request.content;
        let path = Path::new(&path);
        let before = self.rules_projection()?;
        let _resources = tendi_core::coordination::acquire_file_resources(&[path.to_path_buf()])
            .map_err(core_error)?;
        if !before.rules.iter().any(|rule| rule.path == path) {
            return Err(core_error(format!(
                "refusing to edit unknown rule {}",
                path.display()
            )));
        }
        let result = tendi_core::rules::save_rule_file_at_path(path, &expected, &content)
            .map_err(core_error)?;
        self.merge_projection(
            "rules",
            |mut current: tendi_core::rules::RuleScan| {
                if let Some(rule) = current.rules.iter_mut().find(|rule| rule.path == path) {
                    rule.sha256 = result.sha256.clone();
                }
                Ok(current)
            },
            tendi_core::storage::Store::save_rules_for_workspace_if_revision,
        )?;
        self.mark_skill_backup_dirty();
        serde_json::from_value(serde_json::to_value(result).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn rule_file_delete_many(
        &self,
        request: runtime_schema::RuleFileDeleteManyRequest,
    ) -> Result<runtime_schema::RuleFileDeleteManyResponse, DaemonError> {
        required_request_texts(&request.paths, "paths")?;
        let paths = request
            .paths
            .into_iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let before = self.rules_projection()?;
        let _resources =
            tendi_core::coordination::acquire_file_resources(&paths).map_err(core_error)?;
        for path in &paths {
            if !before.rules.iter().any(|rule| rule.path == *path) {
                return Err(core_error(format!(
                    "refusing to delete unknown rule {}",
                    path.display()
                )));
            }
        }
        tendi_core::rules::delete_rule_files(&paths).map_err(core_error)?;
        self.merge_projection(
            "rules",
            |mut current: tendi_core::rules::RuleScan| {
                current.rules.retain(|rule| !paths.contains(&rule.path));
                Ok(current)
            },
            tendi_core::storage::Store::save_rules_for_workspace_if_revision,
        )?;
        self.mark_skill_backup_dirty();
        serde_json::from_value(json!({ "deleted": paths })).map_err(internal_error)
    }

    fn rules_projection(&self) -> Result<tendi_core::rules::RuleScan, DaemonError> {
        let cwd = self.state.cwd.clone();
        self.ensure_projection(
            "rules",
            |store| store.list_rules_for_workspace(&cwd),
            |store, revision| {
                let project_roots = Self::registered_project_roots(store)?;
                let report = tendi_core::rules::scan_rules_for_project_roots(&cwd, &project_roots)?;
                anyhow::ensure!(
                    store.save_rules_for_workspace_if_revision(&cwd, &report, revision)?,
                    "projection changed during preparation"
                );
                Ok(report)
            },
        )
    }

    fn hooks_list(&self) -> Result<runtime_schema::HookRecordList, DaemonError> {
        let report = self.read_cached_projection::<tendi_core::hooks::HookScan>("hooks")?;
        let hooks = report.map(|report| report.hooks).unwrap_or_default();
        serde_json::from_value(hook_records_runtime_value(&hooks)?).map_err(internal_error)
    }

    fn hook_delete(
        &self,
        request: runtime_schema::HookDeleteRequest,
    ) -> Result<runtime_schema::HookDeleteResponse, DaemonError> {
        required_request_text(&request.id, "id")?;
        let before = self.hooks_projection()?;
        let request = hook_delete_request_for_record(hook_for_id(&before.hooks, &request.id)?);
        let _resources = tendi_core::coordination::acquire_file_resources(&[request.path.clone()])
            .map_err(core_error)?;
        let deleted = tendi_core::hooks::hooks_matching_delete_requests(
            &before.hooks,
            std::slice::from_ref(&request),
        )
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
        tendi_core::hooks::delete_hook(request.clone()).map_err(core_error)?;
        let scan = tendi_core::hooks::refresh_hook_scan_after_delete(
            &self.state.cwd,
            before,
            std::slice::from_ref(&request),
        )
        .map_err(core_error)?;
        self.mark_skill_backup_dirty();
        self.save_hook_scan(&scan, std::slice::from_ref(&request.path))?;
        serde_json::from_value(hook_mutation_delta_value(
            &scan,
            std::slice::from_ref(&request.path),
            deleted,
        )?)
        .map_err(internal_error)
    }

    fn hook_delete_many(
        &self,
        request: runtime_schema::HookDeleteManyRequest,
    ) -> Result<runtime_schema::HookDeleteManyResponse, DaemonError> {
        required_request_texts(&request.ids, "ids")?;
        let before = self.hooks_projection()?;
        let requests = request
            .ids
            .iter()
            .map(|id| {
                Ok(hook_delete_request_for_record(hook_for_id(
                    &before.hooks,
                    id,
                )?))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let deleted = tendi_core::hooks::hooks_matching_delete_requests(&before.hooks, &requests)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let resource_paths = requests
            .iter()
            .map(|request| request.path.clone())
            .collect::<Vec<_>>();
        let _resources = tendi_core::coordination::acquire_file_resources(&resource_paths)
            .map_err(core_error)?;
        tendi_core::hooks::delete_hooks(requests.clone()).map_err(core_error)?;
        let scan =
            tendi_core::hooks::refresh_hook_scan_after_delete(&self.state.cwd, before, &requests)
                .map_err(core_error)?;
        self.mark_skill_backup_dirty();
        self.save_hook_scan(&scan, &resource_paths)?;
        let paths = requests
            .iter()
            .map(|request| request.path.clone())
            .collect::<Vec<_>>();
        serde_json::from_value(hook_mutation_delta_value(&scan, &paths, deleted)?)
            .map_err(internal_error)
    }

    fn hook_set_enabled(
        &self,
        request: runtime_schema::HookSetEnabledRequest,
    ) -> Result<runtime_schema::HookSetEnabledResponse, DaemonError> {
        required_request_text(&request.id, "id")?;
        let before = self.hooks_projection()?;
        let request = hook_set_enabled_request_for_record(
            hook_for_id(&before.hooks, &request.id)?,
            request.enabled,
        );
        let _resources = tendi_core::coordination::acquire_file_resources(&[request.path.clone()])
            .map_err(core_error)?;
        tendi_core::hooks::set_hooks_enabled(vec![request.clone()]).map_err(core_error)?;
        let scan = tendi_core::hooks::refresh_hook_scan_after_set_enabled(
            &self.state.cwd,
            before,
            &request,
        )
        .map_err(core_error)?;
        self.mark_skill_backup_dirty();
        self.save_hook_scan(&scan, std::slice::from_ref(&request.path))?;
        serde_json::from_value(hook_mutation_delta_value(
            &scan,
            std::slice::from_ref(&request.path),
            Vec::new(),
        )?)
        .map_err(internal_error)
    }

    fn hook_set_enabled_many(
        &self,
        request: runtime_schema::HookSetEnabledManyRequest,
    ) -> Result<runtime_schema::HookSetEnabledManyResponse, DaemonError> {
        let before = self.hooks_projection()?;
        let requests = request
            .requests
            .into_iter()
            .map(|request| {
                required_request_text(&request.id, "id")?;
                Ok(hook_set_enabled_request_for_record(
                    hook_for_id(&before.hooks, &request.id)?,
                    request.enabled,
                ))
            })
            .collect::<Result<Vec<_>, DaemonError>>()?;
        if requests.is_empty() {
            return serde_json::from_value(json!({ "updated": [], "deleted": [] }))
                .map_err(internal_error);
        }
        let resource_paths = requests
            .iter()
            .map(|request| request.path.clone())
            .collect::<Vec<_>>();
        let _resources = tendi_core::coordination::acquire_file_resources(&resource_paths)
            .map_err(core_error)?;
        tendi_core::hooks::set_hooks_enabled(requests.clone()).map_err(core_error)?;
        let scan = tendi_core::hooks::refresh_hook_scan_after_set_enabled_many(
            &self.state.cwd,
            before,
            &requests,
        )
        .map_err(core_error)?;
        self.mark_skill_backup_dirty();
        self.save_hook_scan(&scan, &resource_paths)?;
        let paths = requests
            .iter()
            .map(|request| request.path.clone())
            .collect::<Vec<_>>();
        serde_json::from_value(hook_mutation_delta_value(&scan, &paths, Vec::new())?)
            .map_err(internal_error)
    }

    fn hook_review(
        &self,
        request: runtime_schema::HookReviewRequest,
    ) -> Result<runtime_schema::HookReviewResponse, DaemonError> {
        required_request_text(&request.id, "id")?;
        let before = self.hooks_projection()?;
        let request = hook_review_request_for_record(hook_for_id(&before.hooks, &request.id)?);
        let path = request.path.clone();
        let _resources = tendi_core::coordination::acquire_file_resources(
            &tendi_core::hooks::hook_review_resource_paths(request.agent, &path)
                .map_err(core_error)?,
        )
        .map_err(core_error)?;
        let scan = tendi_core::hooks::review_hook_from_scan(before, request).map_err(core_error)?;
        self.save_hook_scan(&scan, std::slice::from_ref(&path))?;
        serde_json::from_value(hook_mutation_delta_value(
            &scan,
            std::slice::from_ref(&path),
            Vec::new(),
        )?)
        .map_err(internal_error)
    }

    fn hook_source_read(
        &self,
        request: runtime_schema::HookSourceReadRequest,
    ) -> Result<runtime_schema::HookSourceReadResponse, DaemonError> {
        required_request_text(&request.id, "id")?;
        let projection = self.hooks_projection()?;
        let current = hook_for_id(&projection.hooks, &request.id)?;
        let hook_match = tendi_core::hooks::HookSourceMatch {
            event: current.event.clone(),
            matcher: current.matcher.clone(),
            hook_type: current.hook_type.clone(),
            command: current.command.clone(),
            url: current.url.clone(),
            prompt: current.prompt.clone(),
            filter: current.filter.clone(),
            status_message: current.status_message.clone(),
            enabled: None,
        };
        let path = Path::new(&current.path);
        let result = tendi_core::hooks::read_hook_source_at_path(
            path,
            current.agent,
            Some(&current.trust_hash),
            Some(&hook_match),
        )
        .map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(result).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn mcp_list(&self) -> Result<runtime_schema::McpServerRecordList, DaemonError> {
        let report = self.read_cached_projection::<tendi_core::mcp::McpScan>("mcp")?;
        let servers = report.map(|report| report.servers).unwrap_or_default();
        serde_json::from_value(mcp_records_runtime_value(&servers)?).map_err(internal_error)
    }

    fn mcp_probe(
        &self,
        request: runtime_schema::McpProbeRequest,
    ) -> Result<runtime_schema::McpProbeResponse, DaemonError> {
        required_request_text(&request.id, "id")?;
        let report = self.mcp_projection()?;
        let current = mcp_server_for_id(&report.servers, &request.id)?.clone();
        let expected_trust_hash = current.trust_hash.clone();
        let expected_enabled = current.enabled;
        let internal_request = mcp_probe_request_for_record(&current);
        let updated =
            tendi_core::mcp::probe_server(internal_request, current).map_err(core_error)?;
        self.publish_mcp_probe(request, expected_trust_hash, expected_enabled, updated)
    }

    fn publish_mcp_probe(
        &self,
        request: runtime_schema::McpProbeRequest,
        expected_trust_hash: String,
        expected_enabled: bool,
        updated: tendi_core::mcp::McpServerRecord,
    ) -> Result<runtime_schema::McpProbeResponse, DaemonError> {
        let _resources = tendi_core::coordination::acquire_file_resources(&[updated.path.clone()])
            .map_err(core_error)?;
        tendi_core::mcp::verify_probe_source(&updated.path, &expected_trust_hash)
            .map_err(core_error)?;
        self.merge_projection("mcp", |mut report: tendi_core::mcp::McpScan| {
            let latest = mcp_server_for_id(&report.servers, &request.id)?;
            if latest.trust_hash != expected_trust_hash || latest.enabled != expected_enabled {
                return Err(conflict_error("MCP configuration changed while probing; probe the current configuration again"));
            }
            update_mcp_projection_for_probe(&mut report, &updated)?;
            Ok(report)
        }, tendi_core::storage::Store::save_mcp_for_workspace_if_revision)?;
        serde_json::from_value(json!({
            "updated": mcp_records_runtime_value(std::slice::from_ref(&updated))?
        }))
        .map_err(internal_error)
    }

    fn mcp_projection(&self) -> Result<tendi_core::mcp::McpScan, DaemonError> {
        let cwd = self.state.cwd.clone();
        self.ensure_projection(
            "mcp",
            |store| store.list_mcp_for_workspace(&cwd),
            |store, revision| Self::scan_mcp_metadata_projection(store, &cwd, revision),
        )
    }

    fn refresh_mcp_projection(&self) -> Result<(), DaemonError> {
        let cwd = self.state.cwd.clone();
        self.ensure_projection(
            "mcp",
            |store| Self::ready_mcp_projection(store, &cwd),
            |store, revision| Self::scan_mcp_projection(store, &cwd, revision),
        )
        .map(|_| ())
    }

    fn scan_mcp_projection(
        store: &tendi_core::storage::Store,
        cwd: &Path,
        revision: tendi_core::Revision,
    ) -> anyhow::Result<tendi_core::mcp::McpScan> {
        let project_roots = Self::registered_project_roots(store)?;
        let cached = store.read_cached_projection::<tendi_core::mcp::McpScan>("mcp", cwd)?;
        let report = tendi_core::mcp::scan_mcp_for_project_roots_with_cached(
            cwd,
            &project_roots,
            cached.as_ref(),
        )?;
        // Network probes are not repeated on a concurrent publication.
        if !store.save_mcp_for_workspace_if_revision(cwd, &report, revision)? {
            return store
                .read_cached_projection("mcp", cwd)?
                .ok_or_else(|| anyhow::anyhow!("MCP projection changed while probing"));
        }
        Ok(report)
    }

    fn scan_mcp_metadata_projection(
        store: &tendi_core::storage::Store,
        cwd: &Path,
        revision: tendi_core::Revision,
    ) -> anyhow::Result<tendi_core::mcp::McpScan> {
        let project_roots = Self::registered_project_roots(store)?;
        let report = tendi_core::mcp::scan_mcp_for_project_roots(cwd, &project_roots)?;
        anyhow::ensure!(
            store.save_mcp_for_workspace_if_revision(cwd, &report, revision)?,
            "projection changed during preparation"
        );
        Ok(report)
    }

    fn ready_mcp_projection(
        store: &tendi_core::storage::Store,
        cwd: &Path,
    ) -> anyhow::Result<Option<tendi_core::mcp::McpScan>> {
        if store.projection_status("mcp", cwd)? != tendi_core::storage::ProjectionStatus::Fresh {
            return Ok(None);
        }
        Ok(store.list_mcp_for_workspace(cwd)?)
    }

    fn mcp_set_enabled(
        &self,
        request: runtime_schema::McpSetEnabledRequest,
    ) -> Result<runtime_schema::McpSetEnabledResponse, DaemonError> {
        required_request_text(&request.id, "id")?;
        let mut report = self.mcp_projection()?;
        let current = mcp_server_for_id(&report.servers, &request.id)?.clone();
        let _resources = tendi_core::coordination::acquire_file_resources(&[current.path.clone()])
            .map_err(core_error)?;
        let internal_request = mcp_set_enabled_request_for_record(&current, request.enabled);
        let trust_hash =
            tendi_core::mcp::set_server_enabled(internal_request.clone()).map_err(core_error)?;
        self.mark_skill_backup_dirty();
        report = self.merge_projection(
            "mcp",
            |mut current: tendi_core::mcp::McpScan| {
                update_mcp_projection_for_toggle(
                    &mut current,
                    &internal_request,
                    trust_hash.clone(),
                )?;
                Ok(current)
            },
            tendi_core::storage::Store::save_mcp_for_workspace_if_revision,
        )?;
        let updated = report
            .servers
            .iter()
            .find(|server| tendi_core::mcp::mcp_server_matches_id(server, &request.id))
            .cloned()
            .ok_or_else(|| {
                conflict_error(
                    "MCP server disappeared from the current projection while changing it",
                )
            })?;
        serde_json::from_value(json!({
            "updated": mcp_records_runtime_value(std::slice::from_ref(&updated))?
        }))
        .map_err(internal_error)
    }

    fn mcp_set_enabled_many(
        &self,
        request: runtime_schema::McpSetEnabledManyRequest,
    ) -> Result<runtime_schema::McpSetEnabledManyResponse, DaemonError> {
        if request.requests.is_empty() {
            return serde_json::from_value(json!({ "updated": [] })).map_err(internal_error);
        }
        let mut report = self.mcp_projection()?;
        let paths = request
            .requests
            .iter()
            .map(|request| {
                mcp_server_for_id(&report.servers, &request.id).map(|record| record.path.clone())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let _resources =
            tendi_core::coordination::acquire_file_resources(&paths).map_err(core_error)?;
        let mut updated_ids = Vec::with_capacity(request.requests.len());
        let mut toggles = Vec::new();
        for request in request.requests {
            required_request_text(&request.id, "id")?;
            let current = mcp_server_for_id(&report.servers, &request.id)?.clone();
            let internal_request = mcp_set_enabled_request_for_record(&current, request.enabled);
            let trust_hash = tendi_core::mcp::set_server_enabled(internal_request.clone())
                .map_err(core_error)?;
            update_mcp_projection_for_toggle(&mut report, &internal_request, trust_hash.clone())?;
            toggles.push((internal_request, trust_hash));
            updated_ids.push(request.id);
        }
        self.mark_skill_backup_dirty();
        report = self.merge_projection(
            "mcp",
            |mut current: tendi_core::mcp::McpScan| {
                for (request, trust_hash) in &toggles {
                    update_mcp_projection_for_toggle(&mut current, request, trust_hash.clone())?;
                }
                Ok(current)
            },
            tendi_core::storage::Store::save_mcp_for_workspace_if_revision,
        )?;
        let updated = report
            .servers
            .iter()
            .filter(|server| {
                updated_ids
                    .iter()
                    .any(|id| tendi_core::mcp::mcp_server_matches_id(server, id))
            })
            .cloned()
            .collect::<Vec<_>>();
        serde_json::from_value(json!({
            "updated": mcp_records_runtime_value(&updated)?
        }))
        .map_err(internal_error)
    }

    fn prompts_list(&self) -> Result<runtime_schema::PromptRecordList, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        serde_json::from_value(
            serde_json::to_value(store.list_prompts().map_err(core_error)?)
                .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn prompt_save(
        &self,
        request: runtime_schema::PromptSaveRequest,
    ) -> Result<runtime_schema::PromptSaveResponse, DaemonError> {
        required_request_text(&request.title, "title")?;
        request_text_items(&request.tags, "tags")?;
        let id = optional_request_text(request.id);
        let title = request.title;
        let tags = request.tags;
        let body = request.body;
        let store = self.open_store().map_err(core_error)?;
        let prompt = tendi_core::storage::PromptWrite {
            id,
            title,
            tags,
            body,
        };
        let saved = store.save_prompt(prompt.clone()).map_err(core_error)?;
        let mut value = serde_json::to_value(saved).map_err(internal_error)?;
        if let Some(object) = value.as_object_mut() {
            object.remove("body");
        }
        serde_json::from_value(value).map_err(internal_error)
    }

    fn prompts_delete_many(
        &self,
        request: runtime_schema::PromptsDeleteManyRequest,
    ) -> Result<runtime_schema::PromptsDeleteManyResponse, DaemonError> {
        required_request_texts(&request.ids, "ids")?;
        let ids = request.ids;
        let store = self.open_store().map_err(core_error)?;
        let deleted = store.delete_prompts(&ids).map_err(core_error)?;
        serde_json::from_value(json!({ "deleted": deleted })).map_err(internal_error)
    }

    fn session_transcript(
        &self,
        request: runtime_schema::SessionTranscriptRequest,
    ) -> Result<runtime_schema::TranscriptPage, DaemonError> {
        let agent = agent_kind_from_request(request.agent);
        let page = tendi_core::transcript::parse_transcript_page_if_changed(
            Path::new(&request.path),
            agent,
            request.cursor.as_deref(),
            request.limit.map(|value| value as usize),
            request.known_source_version.as_deref(),
        )
        .map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(page).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn session_transcript_locator(
        &self,
        request: runtime_schema::SessionTranscriptLocatorRequest,
    ) -> Result<runtime_schema::TranscriptLocatorPage, DaemonError> {
        let agent = agent_kind_from_request(request.agent);
        let page =
            tendi_core::transcript::parse_transcript_locator_page(Path::new(&request.path), agent)
                .map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(page).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn session_transcript_search(
        &self,
        request: runtime_schema::SessionTranscriptSearchRequest,
    ) -> Result<runtime_schema::TranscriptSearchResult, DaemonError> {
        let agent = agent_kind_from_request(request.agent);
        let scopes = tendi_core::transcript::TranscriptSearchScopes {
            user: request.scopes.user,
            assistant: request.scopes.assistant,
            system: request.scopes.system,
            tool: request.scopes.tool,
        };
        let result = tendi_core::transcript::search_transcript(
            Path::new(&request.path),
            agent,
            &request.query,
            &scopes,
        )
        .map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(result).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn hooks_projection(&self) -> Result<tendi_core::hooks::HookScan, DaemonError> {
        let cwd = self.state.cwd.clone();
        self.ensure_projection(
            "hooks",
            |store| store.list_hooks_for_workspace(&cwd),
            |store, revision| {
                let report = tendi_core::hooks::scan_hooks(&cwd)?;
                anyhow::ensure!(
                    store.save_hooks_for_workspace_if_revision(&cwd, &report, revision)?,
                    "projection changed during preparation"
                );
                Ok(report)
            },
        )
    }

    fn save_hook_scan(
        &self,
        scan: &tendi_core::HookScan,
        paths: &[PathBuf],
    ) -> Result<(), DaemonError> {
        self.merge_projection(
            "hooks",
            |mut current: tendi_core::HookScan| {
                current.hooks.retain(|hook| !paths.contains(&hook.path));
                current.hooks.extend(
                    scan.hooks
                        .iter()
                        .filter(|hook| paths.contains(&hook.path))
                        .cloned(),
                );
                Ok(current)
            },
            tendi_core::storage::Store::save_hooks_for_workspace_if_revision,
        )
        .map(|_| ())
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

    fn configure_skill_watcher(
        &self,
        scan: &tendi_core::skills::SkillScan,
    ) -> Result<(), DaemonError> {
        let mut paths = BTreeSet::new();
        paths.extend(
            scan.roots
                .iter()
                .filter(|root| root.path.is_dir())
                .map(|root| root.path.clone()),
        );
        paths.extend(scan.skills.iter().flat_map(|skill| {
            skill.paths.iter().filter_map(|path| {
                let directory = path
                    .path
                    .canonicalize()
                    .unwrap_or_else(|_| path.path.clone());
                directory.is_dir().then_some(directory)
            })
        }));
        if paths.is_empty() {
            return Ok(());
        }

        let mut state = self
            .state
            .skill_runtime
            .watcher
            .lock()
            .map_err(|_| internal_error("skill watcher is unavailable"))?;
        if state.watcher.is_none() {
            let tx = self.state.skill_runtime.watch_tx.clone();
            state.watcher = Some(
                notify::recommended_watcher(move |result: notify::Result<Event>| {
                    let _ = tx.send(result);
                })
                .map_err(internal_error)?,
            );
        }
        for path in paths {
            if state.watched_paths.contains(&path) {
                continue;
            }
            state
                .watcher
                .as_mut()
                .expect("skill watcher was initialized")
                .watch(&path, RecursiveMode::Recursive)
                .map_err(|error| {
                    internal_error(format!(
                        "failed to watch skill directory {}: {error}",
                        path.display()
                    ))
                })?;
            state.watched_paths.insert(path);
        }
        Ok(())
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

    fn skills_list(&self) -> Result<runtime_schema::SkillRecordList, DaemonError> {
        let scan = self.read_cached_projection::<tendi_core::skills::SkillScan>("skills")?;
        if let Some(scan) = &scan {
            if let Err(error) = self.configure_skill_watcher(scan) {
                tendi_core::logging::global().warn(
                    "skill watcher registration failed",
                    json!({ "error": error.message }),
                );
            }
        }
        let skills = scan.map(|scan| scan.skills).unwrap_or_default();
        serde_json::from_value(skills_runtime_value(&skills)?).map_err(internal_error)
    }

    fn skills_refresh(&self) -> Result<runtime_schema::SkillsRefreshResponse, DaemonError> {
        let scan = { self.scan_and_persist()? };
        let check_revision = self.skill_projection_revision()?;
        let update_check = self.start_skill_update_check(scan.clone(), check_revision);
        serde_json::from_value(json!({ "skills": scan.skills, "updateCheck": update_check }))
            .map_err(internal_error)
    }

    fn reconcile_skill_visibility_after_external_change(
        &self,
        paths: &[PathBuf],
    ) -> Result<(), DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let _projection = tendi_core::coordination::ResourceLease::acquire(
            store.path(),
            &tendi_core::coordination::shared_projection_key("skills"),
        )
        .map_err(core_error)?;
        store
            .invalidate_projection_resources("skills", &self.state.cwd, paths, true)
            .map_err(core_error)?;
        self.clear_skill_reconciliation_backoff(&self.state.cwd);
        self.schedule_skill_reconciliation();
        Ok(())
    }

    fn skills_targets(&self) -> Result<runtime_schema::SkillTargetRecordList, DaemonError> {
        let targets = std::iter::once(("shared", "Shared", true))
            .chain(
                tendi_core::skill_targets::target_catalog()
                    .iter()
                    .map(|target| (target.id, target.display_name, target.supports_global())),
            )
            .map(|(id, display_name, supports_global)| {
                let target = id.parse::<tendi_core::SkillTarget>().map_err(core_error)?;
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
                Ok(runtime_schema::SkillTargetRecord {
                    id: id.to_string(),
                    display_name: display_name.to_string(),
                    supports_global,
                    global_path,
                })
            })
            .collect::<Result<Vec<_>, DaemonError>>()?;
        Ok(targets)
    }

    fn skills_backup_status(
        &self,
    ) -> Result<runtime_schema::SkillsBackupStatusResponse, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let config = store.skill_backup_config().map_err(core_error)?;
        let catalog = tendi_core::skill_backup::backup_catalog(&store, &self.state.cwd)
            .map_err(core_error)?;
        if store
            .projection_status("skills", &self.state.cwd)
            .map_err(core_error)?
            != tendi_core::storage::ProjectionStatus::Fresh
        {
            self.schedule_projection_refresh("skills");
        }
        if config.is_none() {
            return serde_json::from_value(json!({
                "config": config,
                "statuses": [],
                "versions": [],
                "catalog": catalog,
            }))
            .map_err(internal_error);
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
        let mut statuses =
            tendi_core::skill_backup::backup_statuses_for_paths(&store, &self.state.cwd, &paths)
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
        serde_json::from_value(json!({
            "config": config,
            "statuses": statuses,
            "versions": versions,
            "catalog": catalog,
        }))
        .map_err(internal_error)
    }

    fn skills_backup_configure(
        &self,
        request: runtime_schema::SkillsBackupConfigureRequest,
    ) -> Result<runtime_schema::SkillsBackupConfigureResponse, DaemonError> {
        required_request_text(&request.repository, "repository")?;
        let repository = request.repository;
        let repository_path = Path::new(&repository);
        let is_remote = !repository_path.exists()
            && tendi_core::skill_backup::is_remote_repository(&repository);
        let (remote_url, checkout_path) = if is_remote {
            let checkout_path = request
                .checkout_path
                .filter(|path| !path.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or(tendi_core::skill_backup::default_checkout_path().map_err(core_error)?);
            (repository, checkout_path)
        } else {
            let requested_path = PathBuf::from(&repository);
            let checkout_path =
                tendi_core::skill_backup::discover_git_repository_root(&requested_path)
                    .map_err(core_error)?
                    .unwrap_or(requested_path);
            (String::new(), checkout_path)
        };
        let contents = request
            .contents
            .map(backup_contents_from_request)
            .unwrap_or_default();
        let mut config = tendi_core::skill_backup::BackupConfig::new(remote_url, checkout_path);
        config.contents = contents;
        config.validate().map_err(core_error)?;
        let working_directory = config
            .checkout_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        if !config.remote_url.is_empty() {
            tendi_core::skill_backup::validate_remote(&config.remote_url, working_directory)
                .map_err(core_error)?;
        }
        tendi_core::skill_backup::sync_checkout_for_restore(&config).map_err(core_error)?;
        let store = self.open_store().map_err(core_error)?;
        let config = store
            .save_skill_backup_config(&config)
            .map_err(core_error)?;
        self.mark_skill_backup_dirty();
        serde_json::from_value(serde_json::to_value(config).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn skills_backup_now(&self) -> Result<runtime_schema::SkillsBackupNowResponse, DaemonError> {
        if self.state.backup_sync_running.swap(true, Ordering::AcqRel) {
            return Err(invalid_argument("a skill sync is already running"));
        }
        self.state.backup_sync_dirty.store(false, Ordering::Release);
        let report = (|| -> Result<_, DaemonError> {
            self.refresh_backup_projections()?;
            let store = self.open_store().map_err(core_error)?;
            tendi_core::skill_backup::backup_now(&store, &self.state.cwd).map_err(core_error)
        })();
        if report.is_err() {
            self.mark_skill_backup_dirty();
        }
        self.state
            .backup_sync_running
            .store(false, Ordering::Release);
        let report = report?;
        serde_json::from_value(serde_json::to_value(report).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn skills_backup_sync(&self) -> Result<runtime_schema::SkillsBackupSyncResponse, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let configured = store.skill_backup_config().map_err(core_error)?.is_some();
        if configured {
            self.mark_skill_backup_dirty();
        }
        serde_json::from_value(json!({ "scheduled": configured })).map_err(internal_error)
    }

    fn skills_backup_versions(
        &self,
        request: runtime_schema::SkillsBackupVersionsRequest,
    ) -> Result<runtime_schema::SkillsBackupVersionsResponse, DaemonError> {
        let limit = request.limit.unwrap_or(50) as usize;
        let store = self.open_store().map_err(core_error)?;
        let versions =
            tendi_core::skill_backup::backup_versions(&store, limit).map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(versions).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn skills_backup_restore(
        &self,
        request: runtime_schema::SkillsBackupRestoreRequest,
    ) -> Result<runtime_schema::SkillsBackupRestoreResponse, DaemonError> {
        required_request_text(&request.revision, "revision")?;
        required_request_text(&request.target, "target")?;
        required_request_text(&request.scope, "scope")?;
        if let Some(skill_ids) = request.skill_ids.as_ref() {
            required_request_texts(skill_ids, "skillIds")?;
        }
        let revision = request.revision;
        let skill_ids = request.skill_ids.unwrap_or_default();
        let target = request
            .target
            .parse::<tendi_core::SkillTarget>()
            .map_err(core_error)?;
        let scope = request
            .scope
            .parse::<tendi_core::SkillInstallScope>()
            .map_err(core_error)?;
        let store = self.open_store().map_err(core_error)?;
        let plan = tendi_core::skill_backup::plan_backup_restore(
            &store,
            &self.state.cwd,
            &revision,
            &skill_ids,
            &target,
            scope,
        )
        .map_err(core_error)?;
        if request.dry_run.unwrap_or(false) {
            return serde_json::from_value(serde_json::to_value(plan).map_err(internal_error)?)
                .map_err(internal_error);
        }
        if !request.confirmed.unwrap_or(false) {
            return Err(invalid_argument(
                "sync restore requires confirmed: true after preview",
            ));
        }
        let resolutions = request
            .resolutions
            .unwrap_or_default()
            .into_iter()
            .map(
                |resolution| tendi_core::skill_backup::BackupRestoreResolution {
                    id: resolution.id,
                    action: resolution.action,
                },
            )
            .collect::<Vec<_>>();
        let before = self.skill_projection_for_mutation()?;
        let resources = tendi_core::coordination::acquire_file_resources(
            &tendi_core::skill_backup::backup_restore_resource_paths(&plan),
        )
        .map_err(core_error)?;
        let applied =
            tendi_core::skill_backup::apply_backup_restore_without_database(&plan, &resolutions)
                .map_err(core_error)?;
        let operations = applied.operations;
        store
            .upsert_skill_source_records_for_workspace(&self.state.cwd, &applied.source_records)
            .map_err(core_error)?;
        self.invalidate_skill_projection(
            &applied
                .source_records
                .iter()
                .map(|record| record.skill_path.clone())
                .collect::<Vec<_>>(),
        )?;
        drop(resources);
        let refresh_ids = Vec::new();
        let extra_skill_dirs = operations
            .iter()
            .filter(|operation| operation.status == "restored")
            .map(|operation| operation.target.clone())
            .collect::<Vec<_>>();
        let scan = self.refresh_skill_projection(before, &refresh_ids, &extra_skill_dirs)?;
        let updated = skills_matching_paths(&scan.skills, &extra_skill_dirs);
        serde_json::from_value(json!({ "operations": operations, "updated": updated }))
            .map_err(internal_error)
    }

    fn skills_backup_adopt(
        &self,
        request: runtime_schema::SkillsBackupAdoptRequest,
    ) -> Result<runtime_schema::SkillsBackupAdoptResponse, DaemonError> {
        required_request_text(&request.name, "name")?;
        required_request_text(&request.skill_path, "skillPath")?;
        let name = request.name;
        let skill_path = PathBuf::from(request.skill_path);
        self.skills_backup_adopt_records(vec![(name, skill_path)])
    }

    fn skills_backup_adopt_many(
        &self,
        request: runtime_schema::SkillsBackupAdoptManyRequest,
    ) -> Result<runtime_schema::SkillsBackupAdoptManyResponse, DaemonError> {
        if request.skills.is_empty() {
            return Err(invalid_argument("skills must not be empty"));
        }
        for entry in &request.skills {
            required_request_text(&entry.name, "skills[].name")?;
            required_request_text(&entry.skill_path, "skills[].skillPath")?;
        }
        let records = request
            .skills
            .into_iter()
            .map(|entry| (entry.name, PathBuf::from(entry.skill_path)))
            .collect::<Vec<_>>();
        self.skills_backup_adopt_records(records)
    }

    fn skills_backup_adopt_records(
        &self,
        entries: Vec<(String, PathBuf)>,
    ) -> Result<runtime_schema::BackupAdoptResponse, DaemonError> {
        let before = self.skill_projection_for_mutation()?;
        let paths = entries
            .iter()
            .map(|(_, path)| path.clone())
            .collect::<Vec<_>>();
        let resources =
            tendi_core::coordination::acquire_file_resources(&paths).map_err(core_error)?;
        let store = self.open_store().map_err(core_error)?;
        let mut records = Vec::with_capacity(entries.len());
        for (name, skill_path) in &entries {
            records.push(
                tendi_core::skill_backup::skill_backup_record_for_adoption(
                    skill_path,
                    name.clone(),
                )
                .map_err(core_error)?,
            );
        }
        store
            .upsert_skill_source_records_for_workspace(&self.state.cwd, &records)
            .map_err(core_error)?;
        self.invalidate_skill_projection(&paths)?;
        drop(resources);
        let refresh_ids = Vec::new();
        let refresh_dirs = records
            .iter()
            .map(|record| record.skill_path.clone())
            .collect::<Vec<_>>();
        let scan = self.refresh_skill_projection(before, &refresh_ids, &refresh_dirs)?;
        let updated = skills_matching_paths(&scan.skills, &refresh_dirs);
        serde_json::from_value(json!({
            "records": records,
            "updated": updated,
            "skills": updated,
        }))
        .map_err(internal_error)
    }

    fn skills_backup_disconnect(
        &self,
    ) -> Result<runtime_schema::SkillsBackupDisconnectResponse, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let disconnected = store.clear_skill_backup_config().map_err(core_error)?;
        serde_json::from_value(json!({ "disconnected": disconnected })).map_err(internal_error)
    }

    fn skills_add(
        &self,
        request: runtime_schema::SkillsAddRequest,
    ) -> Result<runtime_schema::SkillsAddResponse, DaemonError> {
        let bundled_source = request.source.trim() == tendi_core::bundled_skill::INSTALL_SOURCE;
        let options = self.skill_add_options(&request)?;
        if bundled_source {
            tendi_core::bundled_skill::install_source_path().map_err(core_error)?;
        }
        if request.dry_run {
            let plan = tendi_core::skills::plan_skill_add(&self.state.cwd, &options)
                .map_err(core_error)?;
            let source_fingerprint =
                tendi_core::skills::skill_add_catalog_fingerprint(&plan).map_err(core_error)?;
            let id = self.next_preview_id("add")?;
            self.state
                .add_preview
                .lock()
                .map_err(|_| internal_error("skill add preview store is unavailable"))?
                .insert(
                    id.clone(),
                    SkillAddPreview {
                        options,
                        plan: plan.clone(),
                        source_fingerprint,
                    },
                )
                .map_err(conflict_error)?;
            return serde_json::from_value(json!({
                "applied": false,
                "plan": plan,
                "previewId": id,
            }))
            .map_err(internal_error);
        }

        let preview_id = request
            .preview_id
            .ok_or_else(|| invalid_argument("missing or empty argument: previewId"))?;
        let preview = {
            let mut stored = self
                .state
                .add_preview
                .lock()
                .map_err(|_| internal_error("skill add preview store is unavailable"))?;
            let preview = stored.get(&preview_id).ok_or_else(|| {
                conflict_error("skill add preview expired; preview the installation again")
            })?;
            if preview.options.source != options.source
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
            stored
                .remove(&preview_id)
                .expect("checked skill add preview")
        };
        let before = self.skill_projection_for_mutation()?;
        let resources = tendi_core::coordination::acquire_file_resources(
            &tendi_core::skills::skill_add_resource_paths(&preview.plan).map_err(core_error)?,
        )
        .map_err(core_error)?;
        if tendi_core::skills::skill_add_catalog_fingerprint(&preview.plan).map_err(core_error)?
            != preview.source_fingerprint
        {
            return Err(conflict_error(
                "skill add source changed; preview the installation again",
            ));
        }
        let report = tendi_core::skills::apply_skill_add_preview(&preview.plan, &options)
            .map_err(core_error)?;
        let store = self.open_store().map_err(core_error)?;
        let visibility_values = report
            .results
            .iter()
            .map(|result| {
                (
                    result
                        .target
                        .canonicalize()
                        .unwrap_or_else(|_| result.target.clone()),
                    options.visibility,
                )
            })
            .collect::<Vec<_>>();
        store
            .upsert_skill_visibilities_for_workspace(&self.state.cwd, &visibility_values)
            .map_err(core_error)?;
        let source_records = tendi_core::skills::skill_source_records_for_add(&report);
        let snapshots =
            tendi_core::skills::capture_skill_snapshots(&source_records).map_err(core_error)?;
        store
            .persist_skill_update_persistence_for_workspace(
                &self.state.cwd,
                &source_records,
                &snapshots,
            )
            .map_err(core_error)?;
        let refresh_ids = Vec::new();
        let extra_skill_dirs = report
            .results
            .iter()
            .map(|result| result.target.clone())
            .collect::<Vec<_>>();
        self.invalidate_skill_projection(
            &report
                .results
                .iter()
                .map(|result| result.target.clone())
                .collect::<Vec<_>>(),
        )?;
        drop(resources);
        let refreshed = self.refresh_skill_projection(before, &refresh_ids, &extra_skill_dirs)?;
        let updated = skills_matching_paths(&refreshed.skills, &extra_skill_dirs);
        if bundled_source {
            tendi_core::bundled_skill::dismiss_prompt().map_err(core_error)?;
        }
        serde_json::from_value(json!({
            "applied": true,
            "report": report,
            "plan": report.plan,
            "results": report.results,
            "updated": updated,
        }))
        .map_err(internal_error)
    }

    fn skills_add_preview_read(
        &self,
        request: runtime_schema::SkillsAddPreviewReadRequest,
    ) -> Result<runtime_schema::SkillsAddPreviewReadResponse, DaemonError> {
        required_request_text(&request.preview_id, "previewId")?;
        required_request_text(&request.skill_name, "skillName")?;
        let preview_id = request.preview_id;
        let skill_name = request.skill_name;
        let preview = self
            .state
            .add_preview
            .lock()
            .map_err(|_| internal_error("skill add preview store is unavailable"))?;
        let skill = preview
            .get(&preview_id)
            .and_then(|preview| {
                preview
                    .plan
                    .available
                    .iter()
                    .find(|skill| skill.name == skill_name)
            })
            .ok_or_else(|| conflict_error("skill is not in the current preview"))?;
        let path = skill.path.join("SKILL.md");
        let name = skill.name.clone();
        drop(preview);
        let content =
            fs::read_to_string(&path).map_err(|error| core_error(anyhow::Error::new(error)))?;
        serde_json::from_value(json!({
            "name": name,
            "relativePath": "SKILL.md",
            "content": content,
        }))
        .map_err(internal_error)
    }

    fn skills_distribute(
        &self,
        request: runtime_schema::SkillsDistributeRequest,
    ) -> Result<runtime_schema::SkillsDistributeResponse, DaemonError> {
        required_request_texts(&request.source_paths, "sourcePaths")?;
        let targets = distribution_targets(&request)?;
        let dry_run = request.dry_run.unwrap_or(false);
        let preview_id = request.preview_id;
        let sources = request
            .source_paths
            .into_iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        if sources.is_empty() {
            return Err(invalid_argument("sourcePaths must not be empty"));
        }
        required_request_text(&request.scope, "scope")?;
        required_request_text(&request.mode, "mode")?;
        let scope = request
            .scope
            .parse::<tendi_core::SkillInstallScope>()
            .map_err(core_error)?;
        let mode = request
            .mode
            .parse::<tendi_core::skills::SkillDistributionMode>()
            .map_err(core_error)?;

        if targets.len() > 1 {
            if dry_run || preview_id.is_some() {
                return Err(invalid_argument(
                    "multi-target skill distribution does not support dryRun or previewId",
                ));
            }
            return serde_json::from_value(
                self.skills_distribute_to_targets(&sources, &targets, scope, mode)?,
            )
            .map_err(internal_error);
        }

        let target = targets
            .into_iter()
            .next()
            .ok_or_else(|| invalid_argument("target or targets must not be empty"))?;

        if dry_run {
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
            self.state
                .distribution_preview
                .lock()
                .map_err(|_| internal_error("skill distribution preview store is unavailable"))?
                .insert(
                    id.clone(),
                    SkillDistributionPreview {
                        sources,
                        target,
                        scope,
                        plans: plans.clone(),
                    },
                )
                .map_err(conflict_error)?;
            return serde_json::from_value(json!({
                "applied": false,
                "plans": plans,
                "previewId": id,
            }))
            .map_err(internal_error);
        }

        let mut before = None;
        let plans = if let Some(preview_id) = preview_id.as_deref() {
            let mut stored =
                self.state.distribution_preview.lock().map_err(|_| {
                    internal_error("skill distribution preview store is unavailable")
                })?;
            let preview = stored.get(preview_id).ok_or_else(|| {
                conflict_error("skill distribution preview expired; preview the change again")
            })?;
            if preview.sources != sources || preview.target != target || preview.scope != scope {
                return Err(conflict_error(
                    "skill distribution options changed; preview the change again",
                ));
            }
            stored
                .remove(preview_id)
                .expect("checked skill distribution preview")
                .plans
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

        serde_json::from_value(self.apply_skill_distribution_plans(plans, mode, before)?)
            .map_err(internal_error)
    }

    fn skills_distribute_to_targets(
        &self,
        sources: &[PathBuf],
        targets: &[tendi_core::SkillTarget],
        scope: tendi_core::SkillInstallScope,
        mode: tendi_core::skills::SkillDistributionMode,
    ) -> Result<Value, DaemonError> {
        let scan = self.skill_projection_for_preview(sources)?;
        let mut plans = Vec::new();
        for target in targets {
            for source in sources {
                plans.push(
                    tendi_core::skills::plan_skill_distribution_for_scan(
                        &self.state.cwd,
                        &scan,
                        source,
                        target,
                        scope,
                        mode,
                    )
                    .map_err(core_error)?,
                );
            }
        }
        if mode == tendi_core::skills::SkillDistributionMode::Move {
            let mut canonical_by_source = BTreeMap::<PathBuf, PathBuf>::new();
            for plan in &mut plans {
                let original_source = plan.source.clone();
                if let Some(canonical_source) = canonical_by_source.get(&original_source) {
                    plan.source = canonical_source.clone();
                    plan.source_symlink = false;
                    plan.mode = tendi_core::skills::SkillDistributionMode::Symlink;
                } else {
                    let canonical_source = if plan.status == "ready" {
                        plan.destination.clone()
                    } else {
                        plan.source.clone()
                    };
                    canonical_by_source.insert(original_source, canonical_source);
                }
            }
        }
        self.apply_skill_distribution_plans_without_mode_override(plans, Some(scan))
    }

    fn apply_skill_distribution_plans(
        &self,
        mut plans: Vec<tendi_core::skills::SkillDistributionPlan>,
        mode: tendi_core::skills::SkillDistributionMode,
        before: Option<tendi_core::skills::SkillScan>,
    ) -> Result<Value, DaemonError> {
        for plan in &mut plans {
            plan.mode = mode;
        }
        self.apply_skill_distribution_plans_locked(plans, before)
    }

    fn apply_skill_distribution_plans_without_mode_override(
        &self,
        plans: Vec<tendi_core::skills::SkillDistributionPlan>,
        before: Option<tendi_core::skills::SkillScan>,
    ) -> Result<Value, DaemonError> {
        self.apply_skill_distribution_plans_locked(plans, before)
    }

    fn apply_skill_distribution_plans_locked(
        &self,
        plans: Vec<tendi_core::skills::SkillDistributionPlan>,
        before: Option<tendi_core::skills::SkillScan>,
    ) -> Result<Value, DaemonError> {
        let paths = plans
            .iter()
            .flat_map(tendi_core::skills::skill_distribution_resource_paths)
            .collect::<Vec<_>>();
        let resources =
            tendi_core::coordination::acquire_file_resources(&paths).map_err(core_error)?;
        let results = plans
            .iter()
            .map(tendi_core::skills::apply_skill_distribution_plan)
            .collect::<Result<Vec<_>, _>>()
            .map_err(core_error)?;
        let store = self.open_store().map_err(core_error)?;
        for plan in &plans {
            store
                .copy_skill_visibility_for_workspace(
                    &self.state.cwd,
                    &plan.source,
                    &plan.destination,
                    plan.mode == tendi_core::skills::SkillDistributionMode::Move,
                )
                .map_err(core_error)?;
        }
        let mut target_records = Vec::with_capacity(plans.len());
        let mut moved_sources = Vec::new();
        for plan in &plans {
            let mut target_record = plan.source_record.clone();
            target_record.skill_path = plan.destination.clone();
            target_record.origin = "tendi-distribution".to_string();
            if plan.mode == tendi_core::skills::SkillDistributionMode::Move {
                moved_sources.push(plan.source_record.skill_path.clone());
            }
            target_records.push(target_record);
        }
        moved_sources.sort();
        moved_sources.dedup();
        let snapshots =
            tendi_core::skills::capture_skill_snapshots(&target_records).map_err(core_error)?;
        store
            .persist_skill_update_persistence_for_workspace_with_deleted(
                &self.state.cwd,
                &moved_sources,
                &target_records,
                &snapshots,
            )
            .map_err(core_error)?;
        self.invalidate_skill_projection(&paths)?;
        drop(resources);
        let before = match before {
            Some(scan) => scan,
            None => self.skill_projection_for_mutation()?,
        };
        let refresh_ids = plans
            .iter()
            .filter_map(|plan| {
                before
                    .skills
                    .iter()
                    .find(|skill| {
                        skill
                            .paths
                            .iter()
                            .any(|path| path.path == plan.source_record.skill_path)
                    })
                    .map(|skill| skill.id.clone())
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let extra_skill_dirs = plans
            .iter()
            .map(|plan| plan.destination.clone())
            .collect::<Vec<_>>();
        let refreshed = self.refresh_skill_projection(before, &refresh_ids, &extra_skill_dirs)?;
        let updated =
            skills_matching_ids_or_paths(&refreshed.skills, &refresh_ids, &extra_skill_dirs);
        Ok(json!({
            "applied": true,
            "plans": plans,
            "results": results,
            "updated": updated,
        }))
    }

    fn skills_remove_locations(
        &self,
        request: runtime_schema::SkillsRemoveLocationsRequest,
    ) -> Result<runtime_schema::SkillsRemoveLocationsResponse, DaemonError> {
        let ids = request.skill_ids;
        let target_ids = request.targets;
        request_text_items(&ids, "skillIds")?;
        request_text_items(&target_ids, "targets")?;
        if ids.is_empty() {
            return Err(invalid_argument("skillIds must not be empty"));
        }
        if target_ids.is_empty() {
            return Err(invalid_argument("targets must not be empty"));
        }
        let scope = request
            .scope
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

        let store = self.open_store().map_err(core_error)?;
        let scan = self.skill_projection_for_ids(&ids)?;
        let mut seen = BTreeSet::new();
        let mut targets = Vec::new();
        for skill in scan.skills.iter().filter(|skill| {
            ids.iter()
                .any(|id| tendi_core::skills::skill_matches_id(skill, id))
        }) {
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
        let requested_paths = plan
            .targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<Vec<_>>();
        // Rehoming changes both canonical installations and their projections.
        let mut resources_paths = tendi_core::skills::skill_delete_resource_paths(&plan);
        resources_paths.extend(
            scan.skills
                .iter()
                .filter(|skill| {
                    ids.iter()
                        .any(|id| tendi_core::skills::skill_matches_id(skill, id))
                })
                .flat_map(|skill| skill.paths.iter().map(|path| path.path.clone())),
        );
        let resources = tendi_core::coordination::acquire_file_resources(&resources_paths)
            .map_err(core_error)?;
        let mut dirty_paths = resources_paths.clone();
        dirty_paths.extend(
            resources_paths
                .iter()
                .filter_map(|path| path.canonicalize().ok()),
        );
        let requested_path_set = requested_paths.iter().cloned().collect::<BTreeSet<_>>();
        let mut rehomed_paths = BTreeSet::new();
        for skill in scan.skills.iter().filter(|skill| {
            ids.iter()
                .any(|id| tendi_core::skills::skill_matches_id(skill, id))
        }) {
            let canonical_targets = plan
                .targets
                .iter()
                .filter(|target| target.kind == "directory")
                .filter(|target| skill.paths.iter().any(|path| path.path == target.path))
                .filter_map(|target| {
                    let canonical = target.path.canonicalize().ok()?;
                    let projections = skill
                        .paths
                        .iter()
                        .filter(|candidate| {
                            candidate.path != target.path
                                && !requested_path_set.contains(&candidate.path)
                                && fs::symlink_metadata(&candidate.path)
                                    .map(|metadata| metadata.file_type().is_symlink())
                                    .unwrap_or(false)
                                && candidate.path.canonicalize().ok().as_ref() == Some(&canonical)
                        })
                        .map(|candidate| candidate.path.clone())
                        .collect::<Vec<_>>();
                    projections
                        .first()
                        .cloned()
                        .map(|destination| (target.path.clone(), destination, projections))
                })
                .collect::<Vec<_>>();
            for (source, destination, projections) in canonical_targets {
                tendi_core::skills::rehome_canonical_skill_and_relink_projections(
                    &source,
                    &destination,
                    &projections,
                )
                .map_err(core_error)?;
                store
                    .copy_skill_visibility_for_workspace(
                        &self.state.cwd,
                        &source,
                        &destination,
                        true,
                    )
                    .map_err(core_error)?;
                rehomed_paths.insert(source);
            }
        }
        let plan = tendi_core::skills::SkillDeletePlan {
            targets: plan
                .targets
                .into_iter()
                .filter(|target| !rehomed_paths.contains(&target.path))
                .collect(),
            dependencies: plan.dependencies,
            dependents: plan.dependents,
        };
        let summary = tendi_core::skills::format_delete_plan(&plan);
        let deleted_visibility_paths = plan
            .targets
            .iter()
            .filter(|target| target.kind == "directory")
            .map(|target| {
                target
                    .path
                    .canonicalize()
                    .unwrap_or_else(|_| target.path.clone())
            })
            .collect::<Vec<_>>();
        tendi_core::skills::apply_skill_delete_plan(&plan).map_err(core_error)?;
        self.mark_skill_backup_dirty();
        let paths = requested_paths;
        let cwd = self.state.cwd.clone();
        store
            .delete_skill_sources_for_workspace(&cwd, &paths, &deleted_visibility_paths)
            .map_err(core_error)?;
        self.invalidate_skill_projection(&dirty_paths)?;
        drop(resources);
        let scan = self.refresh_skill_projection(scan, &ids, &[])?;
        let updated = skills_matching_ids(&scan.skills, &ids);
        let deleted = ids
            .iter()
            .filter(|id| {
                !scan
                    .skills
                    .iter()
                    .any(|skill| tendi_core::skills::skill_matches_id(skill, id))
            })
            .cloned()
            .collect::<Vec<_>>();
        serde_json::from_value(json!({
            "summary": summary,
            "applied": true,
            "plan": plan,
            "updated": updated,
            "deleted": deleted,
        }))
        .map_err(internal_error)
    }

    fn skills_set(
        &self,
        request: runtime_schema::SkillsSetRequest,
    ) -> Result<runtime_schema::SkillsSetResponse, DaemonError> {
        let visibility = skill_visibility_from_request(request.visibility);
        let dry_run = request.dry_run.unwrap_or(false);
        let scan = self.skill_projection()?;
        let ids = if let Some(pattern) = request.pattern.as_deref() {
            tendi_core::skills::skill_ids_matching_pattern(&scan, pattern)
        } else {
            request.skill_ids.unwrap_or_default()
        };
        request_text_items(&ids, "skillIds")?;
        if ids.is_empty() {
            return Err(invalid_argument("no skills matched the selection"));
        }
        let before = self.skill_projection_for_ids(&ids)?;
        let changeset =
            tendi_core::skills::plan_visibility_many_for_scan(&before, &ids, visibility)
                .map_err(core_error)?;
        let summary = tendi_core::skills::format_changeset(&changeset);
        if !dry_run {
            let store = self.open_store().map_err(core_error)?;
            let mut paths = tendi_core::skills::changeset_resource_paths(&changeset);
            paths.extend(
                before
                    .skills
                    .iter()
                    .filter(|skill| {
                        ids.iter()
                            .any(|id| tendi_core::skills::skill_matches_id(skill, id))
                    })
                    .flat_map(|skill| skill.paths.iter().map(|path| path.path.clone())),
            );
            let resources =
                tendi_core::coordination::acquire_file_resources(&paths).map_err(core_error)?;
            let previous = store
                .skill_visibilities_for_workspace(&self.state.cwd)
                .map_err(core_error)?;
            let selected_paths = before
                .skills
                .iter()
                .filter(|skill| {
                    ids.iter()
                        .any(|id| tendi_core::skills::skill_matches_id(skill, id))
                })
                .flat_map(|skill| skill.paths.iter().map(|path| path.path.clone()))
                .collect::<Vec<_>>();
            let materialization =
                match tendi_core::skills::SkillWriteTransaction::prepare(&selected_paths) {
                    Ok(materialization) => materialization,
                    Err(error) => return Err(core_error(error)),
                };
            let visibility_values = before
                .skills
                .iter()
                .filter(|skill| {
                    ids.iter()
                        .any(|id| tendi_core::skills::skill_matches_id(skill, id))
                })
                .flat_map(|skill| skill.paths.iter())
                .map(|path| {
                    (
                        path.path
                            .canonicalize()
                            .unwrap_or_else(|_| path.path.clone()),
                        visibility,
                    )
                })
                .collect::<Vec<_>>();
            let result = (|| -> anyhow::Result<_> {
                store
                    .upsert_skill_visibilities_for_workspace(&self.state.cwd, &visibility_values)?;
                if let Err(error) = tendi_core::skills::apply_changes(&changeset) {
                    let inserted = visibility_values
                        .iter()
                        .map(|(path, _)| path.clone())
                        .collect::<Vec<_>>();
                    store.delete_skill_visibilities_for_workspace(&self.state.cwd, &inserted)?;
                    let previous_values = previous
                        .iter()
                        .filter(|(path, _)| inserted.contains(path))
                        .map(|(path, value)| (path.clone(), *value))
                        .collect::<Vec<_>>();
                    store.upsert_skill_visibilities_for_workspace(
                        &self.state.cwd,
                        &previous_values,
                    )?;
                    return Err(error);
                }
                Ok(())
            })();
            if let Err(error) = result {
                let rollback = materialization.rollback();
                return match rollback {
                    Ok(()) => Err(core_error(error)),
                    Err(rollback_error) => Err(core_error(format!(
                        "{error:#}; skill materialization rollback failed: {rollback_error:#}"
                    ))),
                };
            }
            materialization.commit();
            self.invalidate_skill_projection(&paths)?;
            drop(resources);
            let refreshed = self.refresh_skill_projection(before, &ids, &[])?;
            return serde_json::from_value(json!({
                "summary": summary,
                "applied": true,
                "updated": refreshed
                    .skills
                    .into_iter()
                    .filter(|skill| ids.iter().any(|id| tendi_core::skills::skill_matches_id(skill, id)))
                    .collect::<Vec<_>>(),
            }))
            .map_err(internal_error);
        }
        serde_json::from_value(json!({
            "summary": summary,
            "applied": false,
            "updated": Value::Null,
        }))
        .map_err(internal_error)
    }

    fn skills_wrap(
        &self,
        request: runtime_schema::SkillsWrapRequest,
    ) -> Result<runtime_schema::SkillsWrapResponse, DaemonError> {
        required_request_text(&request.name, "name")?;
        let name = request.name;
        let description = request.description;
        let manual_children = request.manual_children.unwrap_or(false);
        let refresh = request.refresh.unwrap_or(false);
        let dry_run = request.dry_run.unwrap_or(false);
        let scan = self.skill_projection()?;
        let ids = if let Some(pattern) = request.pattern.as_deref() {
            tendi_core::skills::skill_ids_matching_pattern(&scan, pattern)
        } else {
            request.skill_ids.unwrap_or_default()
        };
        request_text_items(&ids, "skillIds")?;
        if ids.is_empty() {
            return Err(invalid_argument("no skills matched the selection"));
        }
        let mut before = self.skill_projection_for_ids(&ids)?;
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
            tendi_core::skills::refresh_wrapper_for_ids(&before, &name, &ids, manual_children)
        } else {
            tendi_core::skills::plan_wrapper_for_ids(
                &before,
                &name,
                &ids,
                description.as_deref(),
                manual_children,
            )
        }
        .map_err(core_error)?;
        let summary = tendi_core::skills::format_changeset(&changeset);
        if !dry_run {
            let mut paths = tendi_core::skills::changeset_resource_paths(&changeset);
            paths.extend(
                before
                    .skills
                    .iter()
                    .filter(|skill| {
                        ids.iter()
                            .any(|id| tendi_core::skills::skill_matches_id(skill, id))
                    })
                    .flat_map(|skill| skill.paths.iter().map(|path| path.path.clone())),
            );
            let resources =
                tendi_core::coordination::acquire_file_resources(&paths).map_err(core_error)?;
            let selected_paths = if manual_children {
                before
                    .skills
                    .iter()
                    .filter(|skill| {
                        skill.name != name
                            && ids
                                .iter()
                                .any(|id| tendi_core::skills::skill_matches_id(skill, id))
                    })
                    .flat_map(|skill| skill.paths.iter().map(|path| path.path.clone()))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let materialization =
                tendi_core::skills::SkillWriteTransaction::prepare(&selected_paths)
                    .map_err(core_error)?;
            let result = (|| -> anyhow::Result<_> {
                tendi_core::skills::apply_changes(&changeset)?;
                if manual_children {
                    let store = self.open_store()?;
                    let values = before
                        .skills
                        .iter()
                        .filter(|skill| {
                            skill.name != name
                                && ids
                                    .iter()
                                    .any(|id| tendi_core::skills::skill_matches_id(skill, id))
                        })
                        .flat_map(|skill| skill.paths.iter())
                        .map(|path| {
                            (
                                path.path
                                    .canonicalize()
                                    .unwrap_or_else(|_| path.path.clone()),
                                tendi_core::SkillVisibility::Manual,
                            )
                        })
                        .collect::<Vec<_>>();
                    store.upsert_skill_visibilities_for_workspace(&self.state.cwd, &values)?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                let rollback = materialization.rollback();
                return match rollback {
                    Ok(()) => Err(core_error(error)),
                    Err(rollback_error) => Err(core_error(format!(
                        "{error:#}; skill materialization rollback failed: {rollback_error:#}"
                    ))),
                };
            }
            materialization.commit();
            self.invalidate_skill_projection(&paths)?;
            drop(resources);
            let refresh_ids = ids;
            let extra_skill_dirs = changeset
                .changes
                .iter()
                .filter_map(|change| change.path.parent().map(Path::to_path_buf))
                .collect::<Vec<_>>();
            let refreshed =
                self.refresh_skill_projection(before, &refresh_ids, &extra_skill_dirs)?;
            let updated =
                skills_matching_ids_or_paths(&refreshed.skills, &refresh_ids, &extra_skill_dirs);
            return serde_json::from_value(json!({
                "summary": summary,
                "applied": true,
                "updated": updated,
            }))
            .map_err(internal_error);
        }
        serde_json::from_value(json!({
            "summary": summary,
            "applied": false,
            "updated": Value::Null,
        }))
        .map_err(internal_error)
    }

    fn skills_updates(
        &self,
        request: runtime_schema::SkillsUpdatesRequest,
    ) -> Result<runtime_schema::SkillsUpdatesResponse, DaemonError> {
        if request.check.unwrap_or(false) {
            let (scan, check_revision) = self.cached_skill_projection_with_revision()?;
            return serde_json::from_value(json!({
                "updateCheck": self.start_skill_update_check(scan, check_revision),
            }))
            .map_err(internal_error);
        }
        let scan = self.scan_and_persist()?;
        serde_json::from_value(serde_json::to_value(scan.skills).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn skills_updates_cancel(
        &self,
    ) -> Result<runtime_schema::SkillsUpdatesCancelResponse, DaemonError> {
        let running = self.state.skill_update.cancel();
        serde_json::from_value(json!({
            "status": if running { "cancellation-requested" } else { "not-running" },
        }))
        .map_err(internal_error)
    }

    fn start_skill_update_check(
        &self,
        scan: tendi_core::skills::SkillScan,
        check_revision: tendi_core::Revision,
    ) -> &'static str {
        let skill_ids = scan
            .skills
            .iter()
            .map(|skill| skill.id.clone())
            .collect::<Vec<_>>();
        if let Ok(Some(updates)) =
            self.cached_skill_update_reports_at_revision(&scan, &skill_ids, check_revision)
        {
            tendi_core::logging::global().info(
                "skill update check completed from cache",
                json!({
                    "skillCount": scan.skills.len(),
                    "cacheTtlMs": SKILL_UPDATE_REPORT_CACHE_TTL.as_secs_f64() * 1000.0,
                }),
            );
            self.emit_event(
                SKILL_UPDATE_EVENT,
                runtime_event(
                    SKILL_UPDATE_EVENT,
                    json!({
                        "status": "completed",
                        "skills": scan.skills,
                        "updates": updates,
                        "error": Value::Null,
                    }),
                ),
            );
            return "started";
        }
        let Some(job) = self.state.skill_update.start() else {
            return "already-running";
        };
        if self
            .state
            .skill_update_check
            .lock()
            .map(|mut cache| *cache = None)
            .is_err()
        {
            return "unavailable";
        }
        let daemon = self.clone();
        let operation_id = match tendi_core::OperationId::new(format!(
            "skill-update-check-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )) {
            Ok(operation_id) => operation_id,
            Err(_) => {
                return "unavailable";
            }
        };
        let token = job.token();
        let step = request_scheduler::Step::acquire(
            request_scheduler::Workload::Prepare,
            Vec::new(),
            move || {
                let ids = scan
                    .skills
                    .iter()
                    .map(|skill| skill.id.clone())
                    .collect::<Vec<_>>();
                let paths =
                    tendi_core::skills::skill_update_preparation_resource_paths(&scan, &ids)?;
                let resources = vec![tendi_core::coordination::ResourceRequest::files(paths)?];
                Ok(request_scheduler::Step::acquire(
                    request_scheduler::Workload::ExternalIo,
                    resources,
                    move || {
                        let result = (|| -> Result<Value, DaemonError> {
                            let updates =
                                tendi_core::skills::check_skill_updates_for_scan_with_cancel(
                                    &scan,
                                    job.cancelled(),
                                );
                            if job.cancelled().load(Ordering::Acquire) {
                                return Err(DaemonError::new(
                                    "CANCELLED",
                                    "skill update check cancelled",
                                ));
                            }
                            daemon.cache_skill_update_check(check_revision, &scan, &updates)?;
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
                        drop(job);
                        daemon.emit_event(
                            SKILL_UPDATE_EVENT,
                            runtime_event(SKILL_UPDATE_EVENT, event),
                        );
                        Ok(request_scheduler::Step::Complete(()))
                    },
                ))
            },
        );
        let rejected_daemon = self.clone();
        if self
            .state
            .requests
            .submit_with_rejection(operation_id, step, token, move |error| {
                rejected_daemon.emit_event(
                    SKILL_UPDATE_EVENT,
                    runtime_event(
                        SKILL_UPDATE_EVENT,
                        json!({
                            "status": "failed",
                            "skills": Value::Null,
                            "updates": [],
                            "error": error.to_string(),
                        }),
                    ),
                );
            })
            .is_err()
        {
            return "unavailable";
        }
        "started"
    }

    fn skills_update(
        &self,
        request: runtime_schema::SkillsUpdateRequest,
    ) -> Result<runtime_schema::SkillsUpdateResponse, DaemonError> {
        let (scan, projection_revision) = self.skill_projection_with_revision()?;
        let ids = if let Some(pattern) = request.pattern.as_deref() {
            tendi_core::skills::skill_ids_matching_pattern(&scan, pattern)
        } else {
            request.skill_ids.clone().unwrap_or_default()
        };
        request_text_items(&ids, "skillIds")?;
        if ids.is_empty() {
            return Err(invalid_argument("no skills matched the selection"));
        }
        let plan_started = Instant::now();
        let store = self.open_store().map_err(core_error)?;
        let cached_reports =
            self.cached_skill_update_reports_at_revision(&scan, &ids, projection_revision)?;
        let used_cached_reports = cached_reports.is_some();
        let plan = match cached_reports {
            Some(reports) => tendi_core::skills::plan_skill_updates_many_for_scan_in_workspace_with_store_and_reports(
                &scan,
                &ids,
                &self.state.cwd,
                &store,
                &reports,
            ),
            None => tendi_core::skills::plan_skill_updates_many_for_scan_in_workspace_with_store(
                &scan,
                &ids,
                &self.state.cwd,
                &store,
            ),
        }
        .map_err(core_error)?;
        tendi_core::logging::global().info(
            "skill update planning completed",
            json!({
                "skillCount": ids.len(),
                "usedCachedReports": used_cached_reports,
                "dryRun": request.dry_run.unwrap_or(false),
                "fileChangeCount": plan.file_changes.changes.len(),
                "durationMs": plan_started.elapsed().as_secs_f64() * 1000.0,
            }),
        );
        self.finish_skill_update(request, scan, plan)
    }

    fn finish_skill_update(
        &self,
        request: runtime_schema::SkillsUpdateRequest,
        scan: tendi_core::skills::SkillScan,
        plan: tendi_core::skills::SkillUpdatePlan,
    ) -> Result<runtime_schema::SkillsUpdateResponse, DaemonError> {
        let dry_run = request.dry_run.unwrap_or(false);
        let summary = tendi_core::skills::format_update_plan(&plan);
        let can_apply = plan.can_apply();
        if !dry_run && !can_apply {
            return Err(conflict_error(
                "skill update has no applicable changes; preview the update again",
            ));
        }
        if !dry_run {
            let store = self.open_store().map_err(core_error)?;
            let resolutions = request
                .resolutions
                .as_ref()
                .map(|resolutions| resolutions.extra.clone())
                .unwrap_or_default();
            let prepared =
                tendi_core::skills::prepare_skill_update_plan_with_resolutions(&plan, &resolutions)
                    .map_err(core_error)?;
            tendi_core::skills::apply_prepared_skill_update_plan_for_workspace(
                &store,
                &self.state.cwd,
                &prepared,
            )
            .map_err(core_error)?;
            let refresh_ids = skill_update_refresh_ids(&scan, &plan);
            let extra_skill_dirs = skill_update_refresh_dirs(&plan);
            let scan = self.refresh_skill_projection(scan, &refresh_ids, &extra_skill_dirs)?;
            let updated =
                skills_matching_ids_or_paths(&scan.skills, &refresh_ids, &extra_skill_dirs);
            return serde_json::from_value(json!({
                "summary": summary,
                "applied": true,
                "canApply": can_apply,
                "plan": plan,
                "updated": updated
            }))
            .map_err(internal_error);
        }
        serde_json::from_value(json!({
            "summary": summary,
            "applied": false,
            "canApply": can_apply,
            "plan": plan,
            "updated": Value::Null
        }))
        .map_err(internal_error)
    }

    fn skills_update_many(
        &self,
        request: runtime_schema::SkillsUpdateManyRequest,
    ) -> Result<runtime_schema::SkillsUpdateManyResponse, DaemonError> {
        let ids = request.skill_ids;
        required_request_texts(&ids, "skillIds")?;
        let dry_run = request.dry_run.unwrap_or(false);
        let plan_started = Instant::now();
        tendi_core::logging::global().info(
            "skill update many planning started",
            json!({ "skillCount": ids.len(), "dryRun": dry_run }),
        );
        let preview_id = if dry_run {
            None
        } else {
            Some(request.preview_id.ok_or_else(|| {
                conflict_error("skill update preview expired; preview the update again")
            })?)
        };
        let plan = if dry_run {
            let scan = self.skill_projection_for_ids(&ids)?;
            let store = self.open_store().map_err(core_error)?;
            let cached_reports = self.cached_skill_update_reports(&scan, &ids)?;
            let used_cached_reports = cached_reports.is_some();
            let plan = match cached_reports {
                Some(reports) => {
                    tendi_core::skills::plan_skill_updates_many_for_scan_in_workspace_with_store_and_reports(
                        &scan,
                        &ids,
                        &self.state.cwd,
                        &store,
                        &reports,
                    )
                    .map_err(core_error)?
                }
                None => tendi_core::skills::plan_skill_updates_many_for_scan_in_workspace_with_store(
                    &scan,
                    &ids,
                    &self.state.cwd,
                    &store,
                )
                .map_err(core_error)?,
            };
            tendi_core::logging::global().info(
                "skill update many planning completed",
                json!({
                    "skillCount": ids.len(),
                    "dryRun": dry_run,
                    "usedCachedReports": used_cached_reports,
                    "durationMs": plan_started.elapsed().as_secs_f64() * 1000.0,
                    "canApply": plan.can_apply(),
                    "fileChangeCount": plan.file_changes.changes.len(),
                    "gitUpdateCount": plan.git_updates.len(),
                    "mergeIssueCount": plan.merge_issues.len(),
                }),
            );
            plan
        } else {
            let preview_id = preview_id.as_deref().expect("non-dry-run has preview id");
            let mut stored = self
                .state
                .update_preview
                .lock()
                .map_err(|_| internal_error("update preview store is unavailable"))?;
            let preview = stored.get(preview_id).ok_or_else(|| {
                conflict_error("skill update preview expired; preview the update again")
            })?;
            if preview.skill_ids != ids {
                return Err(conflict_error(
                    "update selection changed; preview the update again",
                ));
            }
            stored
                .remove(preview_id)
                .expect("checked skill update preview")
                .plan
        };
        let summary = tendi_core::skills::format_update_plan(&plan);
        let can_apply = plan.can_apply();
        if !dry_run && !can_apply {
            return Err(conflict_error(
                "skill update has no applicable changes; preview the update again",
            ));
        }
        if dry_run {
            let preview_id = if can_apply {
                let id = self.next_preview_id("update")?;
                self.state
                    .update_preview
                    .lock()
                    .map_err(|_| internal_error("update preview store is unavailable"))?
                    .insert(
                        id.clone(),
                        SkillUpdatePreview {
                            skill_ids: ids.clone(),
                            plan: plan.clone(),
                        },
                    )
                    .map_err(conflict_error)?;
                Value::String(id)
            } else {
                Value::Null
            };
            return serde_json::from_value(json!({
                "summary": summary,
                "applied": false,
                "canApply": can_apply,
                "plan": plan,
                "previewId": preview_id,
                "skills": Value::Null
            }))
            .map_err(internal_error);
        }
        tendi_core::logging::global().info(
            "skill update many apply started",
            json!({
                "skillCount": ids.len(),
                "fileChangeCount": plan.file_changes.changes.len(),
                "gitUpdateCount": plan.git_updates.len(),
                "mergeIssueCount": plan.merge_issues.len(),
            }),
        );
        let before = self.skill_projection_for_ids(&ids)?;
        let refresh_ids = ids.clone();
        let extra_skill_dirs = skill_update_refresh_dirs(&plan);
        let store = self.open_store().map_err(core_error)?;
        let resolutions = request
            .resolutions
            .map(|resolutions| resolutions.extra)
            .unwrap_or_default();
        let prepared =
            tendi_core::skills::prepare_skill_update_plan_with_resolutions(&plan, &resolutions)
                .map_err(core_error)?;
        tendi_core::skills::apply_prepared_skill_update_plan_for_workspace(
            &store,
            &self.state.cwd,
            &prepared,
        )
        .map_err(core_error)?;
        let scan = self.refresh_skill_projection(before, &refresh_ids, &extra_skill_dirs)?;
        let updated = skills_matching_ids_or_paths(&scan.skills, &refresh_ids, &extra_skill_dirs);
        tendi_core::logging::global().info(
            "skill update many apply completed",
            json!({ "skillCount": ids.len(), "updatedCount": updated.len() }),
        );
        serde_json::from_value(json!({
                "summary": summary,
                "applied": true,
                "canApply": can_apply,
                "plan": plan,
                "previewId": Value::Null,
                "updated": updated
        }))
        .map_err(internal_error)
    }

    fn skills_delete_many(
        &self,
        request: runtime_schema::SkillsDeleteManyRequest,
    ) -> Result<runtime_schema::SkillsDeleteManyResponse, DaemonError> {
        let ids = request.skill_ids;
        required_request_texts(&ids, "skillIds")?;
        let store = self.open_store().map_err(core_error)?;
        let scan = self.skill_projection_for_ids(&ids)?;
        let plan =
            tendi_core::skills::plan_skill_delete_many_for_scan(&scan, &ids).map_err(core_error)?;
        let resources = tendi_core::coordination::acquire_file_resources(
            &tendi_core::skills::skill_delete_resource_paths(&plan),
        )
        .map_err(core_error)?;
        let summary = tendi_core::skills::format_delete_plan(&plan);
        let deleted_visibility_paths = plan
            .targets
            .iter()
            .filter(|target| target.kind == "directory")
            .map(|target| {
                target
                    .path
                    .canonicalize()
                    .unwrap_or_else(|_| target.path.clone())
            })
            .collect::<Vec<_>>();
        tendi_core::skills::apply_skill_delete_plan(&plan).map_err(core_error)?;
        self.mark_skill_backup_dirty();
        let paths = plan
            .targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<Vec<_>>();
        let cwd = self.state.cwd.clone();
        store
            .delete_skill_sources_for_workspace(&cwd, &paths, &deleted_visibility_paths)
            .map_err(core_error)?;
        self.invalidate_skill_projection(&paths)?;
        drop(resources);
        self.refresh_skill_projection(scan, &ids, &[])?;
        serde_json::from_value(json!({
            "summary": summary,
            "applied": true,
            "plan": plan,
            "previewId": Value::Null,
            "refreshRequired": false,
            "deleted": ids,
        }))
        .map_err(internal_error)
    }

    fn skills_marketplace_search(
        &self,
        request: runtime_schema::SkillsMarketplaceSearchRequest,
    ) -> Result<runtime_schema::SkillsMarketplaceSearchResponse, DaemonError> {
        required_request_text(&request.query, "query")?;
        let result = tendi_core::skill_marketplace::search(&request.query).map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(result).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn skill_files(
        &self,
        request: runtime_schema::SkillFilesRequest,
    ) -> Result<runtime_schema::SkillFilesResponse, DaemonError> {
        let skill_id = request.skill_id;
        let scan = self.skill_projection_for_ids(std::slice::from_ref(&skill_id))?;
        let (name, cached) =
            self.skill_context_for_id(request.location_id.as_deref(), &skill_id, &scan)?;
        let result =
            tendi_core::files::list_skill_files(&self.state.cwd, &name, Some(cached.as_path()))
                .map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(result).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn skill_file_read(
        &self,
        request: runtime_schema::SkillFileReadRequest,
    ) -> Result<runtime_schema::SkillFileReadResponse, DaemonError> {
        let skill_id = request.skill_id;
        let relative_path = request.relative_path;
        let scan = self.skill_projection_for_ids(std::slice::from_ref(&skill_id))?;
        let (name, cached) =
            self.skill_context_for_id(request.location_id.as_deref(), &skill_id, &scan)?;
        let result = tendi_core::files::read_skill_file(
            &self.state.cwd,
            &name,
            &relative_path,
            Some(cached.as_path()),
        )
        .map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(result).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn skill_context_for_id(
        &self,
        location_id: Option<&str>,
        skill_id: &str,
        scan: &tendi_core::skills::SkillScan,
    ) -> Result<(String, PathBuf), DaemonError> {
        let skill = scan
            .skills
            .iter()
            .find(|skill| tendi_core::skills::skill_matches_id(skill, skill_id))
            .ok_or_else(|| conflict_error(format!("unknown skill id: {skill_id}")))?;
        if let Some(location_id) = location_id {
            if let Some((located_skill, path)) = scan.find_skill_location_by_id(location_id) {
                if tendi_core::skills::skill_matches_id(located_skill, skill_id)
                    && path.path.is_dir()
                {
                    return Ok((skill.name.clone(), path.path.clone()));
                }
            }
            return Err(conflict_error(format!(
                "skill location is not registered for skill {skill_id}"
            )));
        }
        skill
            .paths
            .first()
            .map(|path| (skill.name.clone(), path.path.clone()))
            .filter(|(_, path)| path.is_dir())
            .ok_or_else(|| core_error(format!("skill {skill_id} has no readable location")))
    }

    fn finish_skill_write<T>(
        transaction: tendi_core::skills::SkillWriteTransaction,
        result: anyhow::Result<T>,
    ) -> Result<T, DaemonError> {
        match result {
            Ok(value) => {
                transaction.commit();
                Ok(value)
            }
            Err(error) => match transaction.rollback() {
                Ok(()) => Err(core_error(error)),
                Err(rollback_error) => Err(core_error(format!(
                    "{error:#}; skill materialization rollback failed: {rollback_error:#}"
                ))),
            },
        }
    }

    fn skill_file_save(
        &self,
        request: runtime_schema::SkillFileSaveRequest,
    ) -> Result<runtime_schema::SkillFileSaveResponse, DaemonError> {
        let skill_id = request.skill_id;
        let relative_path = request.relative_path;
        let expected_sha256 = request.expected_sha256;
        let content = request.content;
        let before = self.skill_projection_for_ids(std::slice::from_ref(&skill_id))?;
        let (name, cached) =
            self.skill_context_for_id(request.location_id.as_deref(), &skill_id, &before)?;
        let resources = tendi_core::coordination::acquire_file_resources(&[cached.clone()])
            .map_err(core_error)?;
        let materialization =
            tendi_core::skills::SkillWriteTransaction::prepare(std::slice::from_ref(&cached))
                .map_err(core_error)?;
        let result = Self::finish_skill_write(
            materialization,
            tendi_core::files::save_skill_file(
                &self.state.cwd,
                &name,
                &relative_path,
                &expected_sha256,
                &content,
                Some(cached.as_path()),
            ),
        )?;
        self.invalidate_skill_projection(std::slice::from_ref(&cached))?;
        drop(resources);
        let mut value = serde_json::to_value(result).map_err(internal_error)?;
        if tendi_core::files::skill_relative_path_affects_projection(&relative_path) {
            let scan =
                self.refresh_skill_projection(before, std::slice::from_ref(&skill_id), &[])?;
            if let Some(object) = value.as_object_mut() {
                let updated = scan
                    .skills
                    .into_iter()
                    .filter(|skill| tendi_core::skills::skill_matches_id(skill, &skill_id))
                    .collect::<Vec<_>>();
                object.insert(
                    "skills".to_string(),
                    serde_json::to_value(updated).map_err(internal_error)?,
                );
            }
        }
        serde_json::from_value(value).map_err(internal_error)
    }

    fn skill_file_create(
        &self,
        request: runtime_schema::SkillFileCreateRequest,
    ) -> Result<runtime_schema::SkillFileCreateResponse, DaemonError> {
        let skill_id = request.skill_id;
        let relative_path = request.relative_path;
        let before = self.skill_projection_for_ids(std::slice::from_ref(&skill_id))?;
        let (name, cached) =
            self.skill_context_for_id(request.location_id.as_deref(), &skill_id, &before)?;
        let resources = tendi_core::coordination::acquire_file_resources(&[cached.clone()])
            .map_err(core_error)?;
        let materialization =
            tendi_core::skills::SkillWriteTransaction::prepare(std::slice::from_ref(&cached))
                .map_err(core_error)?;
        let result = Self::finish_skill_write(
            materialization,
            tendi_core::files::create_skill_file(
                &self.state.cwd,
                &name,
                &relative_path,
                Some(cached.as_path()),
            ),
        )?;
        self.invalidate_skill_projection(std::slice::from_ref(&cached))?;
        drop(resources);
        self.skill_tree_mutation_response(
            &skill_id,
            &name,
            cached.as_path(),
            before,
            &[relative_path.as_str()],
            Some(result),
        )
    }

    fn skill_folder_create(
        &self,
        request: runtime_schema::SkillFolderCreateRequest,
    ) -> Result<runtime_schema::SkillFolderCreateResponse, DaemonError> {
        let skill_id = request.skill_id;
        let relative_path = request.relative_path;
        let before = self.skill_projection_for_ids(std::slice::from_ref(&skill_id))?;
        let (name, cached) =
            self.skill_context_for_id(request.location_id.as_deref(), &skill_id, &before)?;
        let resources = tendi_core::coordination::acquire_file_resources(&[cached.clone()])
            .map_err(core_error)?;
        let materialization =
            tendi_core::skills::SkillWriteTransaction::prepare(std::slice::from_ref(&cached))
                .map_err(core_error)?;
        Self::finish_skill_write(
            materialization,
            tendi_core::files::create_skill_folder(
                &self.state.cwd,
                &name,
                &relative_path,
                Some(cached.as_path()),
            ),
        )?;
        self.invalidate_skill_projection(std::slice::from_ref(&cached))?;
        drop(resources);
        self.skill_tree_mutation_response(
            &skill_id,
            &name,
            cached.as_path(),
            before,
            &[relative_path.as_str()],
            None,
        )
    }

    fn skill_path_rename(
        &self,
        request: runtime_schema::SkillPathRenameRequest,
    ) -> Result<runtime_schema::SkillPathRenameResponse, DaemonError> {
        let skill_id = request.skill_id;
        let from = request.from_relative_path;
        let to = request.to_relative_path;
        let before = self.skill_projection_for_ids(std::slice::from_ref(&skill_id))?;
        let (name, cached) =
            self.skill_context_for_id(request.location_id.as_deref(), &skill_id, &before)?;
        let resources = tendi_core::coordination::acquire_file_resources(&[cached.clone()])
            .map_err(core_error)?;
        let materialization =
            tendi_core::skills::SkillWriteTransaction::prepare(std::slice::from_ref(&cached))
                .map_err(core_error)?;
        Self::finish_skill_write(
            materialization,
            tendi_core::files::rename_skill_path(
                &self.state.cwd,
                &name,
                &from,
                &to,
                Some(cached.as_path()),
            ),
        )?;
        self.invalidate_skill_projection(std::slice::from_ref(&cached))?;
        drop(resources);
        self.skill_tree_mutation_response(
            &skill_id,
            &name,
            cached.as_path(),
            before,
            &[from.as_str(), to.as_str()],
            None,
        )
    }

    fn skill_path_delete(
        &self,
        request: runtime_schema::SkillPathDeleteRequest,
    ) -> Result<runtime_schema::SkillPathDeleteResponse, DaemonError> {
        let skill_id = request.skill_id;
        let relative_path = request.relative_path;
        let before = self.skill_projection_for_ids(std::slice::from_ref(&skill_id))?;
        let (name, cached) =
            self.skill_context_for_id(request.location_id.as_deref(), &skill_id, &before)?;
        let resources = tendi_core::coordination::acquire_file_resources(&[cached.clone()])
            .map_err(core_error)?;
        let materialization =
            tendi_core::skills::SkillWriteTransaction::prepare(std::slice::from_ref(&cached))
                .map_err(core_error)?;
        Self::finish_skill_write(
            materialization,
            tendi_core::files::delete_skill_path(
                &self.state.cwd,
                &name,
                &relative_path,
                Some(cached.as_path()),
            ),
        )?;
        self.invalidate_skill_projection(std::slice::from_ref(&cached))?;
        drop(resources);
        self.skill_tree_mutation_response(
            &skill_id,
            &name,
            cached.as_path(),
            before,
            &[relative_path.as_str()],
            None,
        )
    }

    fn skill_tree_mutation_response(
        &self,
        skill_id: &str,
        name: &str,
        skill_dir: &Path,
        before: tendi_core::skills::SkillScan,
        relative_paths: &[&str],
        write: Option<tendi_core::files::SkillFileWriteResult>,
    ) -> Result<runtime_schema::SkillFileMutationResponse, DaemonError> {
        let files = tendi_core::files::list_skill_files(&self.state.cwd, name, Some(skill_dir))
            .map_err(core_error)?;
        let mut value = if let Some(write) = write {
            serde_json::to_value(write).map_err(internal_error)?
        } else {
            Value::Object(serde_json::Map::new())
        };
        let object = value
            .as_object_mut()
            .ok_or_else(|| internal_error("skill mutation response must be an object"))?;
        object.insert(
            "files".to_string(),
            serde_json::to_value(files).map_err(internal_error)?,
        );
        if relative_paths
            .iter()
            .any(|path| tendi_core::files::skill_relative_path_affects_projection(path))
        {
            let ids = [skill_id.to_string()];
            let scan = self.refresh_skill_projection(before, &ids, &[])?;
            let updated = scan
                .skills
                .into_iter()
                .filter(|skill| tendi_core::skills::skill_matches_id(skill, skill_id))
                .collect::<Vec<_>>();
            object.insert(
                "skills".to_string(),
                serde_json::to_value(updated).map_err(internal_error)?,
            );
        }
        serde_json::from_value(value).map_err(internal_error)
    }

    fn skill_projection_revision(&self) -> Result<tendi_core::Revision, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let scope = daemon_scope_key(self)?;
        Ok(store
            .projection_head(&scope, "skills")
            .map_err(core_error)?
            .map(|head| head.revision)
            .unwrap_or(tendi_core::Revision::ZERO))
    }

    fn cache_skill_update_check(
        &self,
        projection_revision: tendi_core::Revision,
        scan: &tendi_core::skills::SkillScan,
        reports: &[tendi_core::skills::SkillUpdateReport],
    ) -> Result<(), DaemonError> {
        let skill_fingerprints = scan
            .skills
            .iter()
            .map(|skill| (skill.id.clone(), skill_update_fingerprint(skill)))
            .collect();
        self.state
            .skill_update_check
            .lock()
            .map_err(|_| internal_error("skill update check cache is unavailable"))?
            .replace(SkillUpdateCheckCache {
                projection_revision,
                reports: reports.to_vec(),
                skill_fingerprints,
                checked_at: Instant::now(),
            });
        Ok(())
    }

    fn cached_skill_update_reports(
        &self,
        scan: &tendi_core::skills::SkillScan,
        skill_ids: &[String],
    ) -> Result<Option<Vec<tendi_core::skills::SkillUpdateReport>>, DaemonError> {
        let revision = self.skill_projection_revision()?;
        self.cached_skill_update_reports_at_revision(scan, skill_ids, revision)
    }

    fn cached_skill_update_reports_at_revision(
        &self,
        scan: &tendi_core::skills::SkillScan,
        skill_ids: &[String],
        revision: tendi_core::Revision,
    ) -> Result<Option<Vec<tendi_core::skills::SkillUpdateReport>>, DaemonError> {
        let selected_ids = scan
            .skills
            .iter()
            .filter(|skill| {
                skill_ids
                    .iter()
                    .any(|id| tendi_core::skills::skill_matches_id(skill, id))
            })
            .map(|skill| skill.id.as_str())
            .collect::<BTreeSet<_>>();
        let cache = self
            .state
            .skill_update_check
            .lock()
            .map_err(|_| internal_error("skill update check cache is unavailable"))?;
        let Some(cache) = cache.as_ref() else {
            tendi_core::logging::global().warn(
                "skill update report cache miss",
                json!({
                    "reason": "empty",
                    "requestedSkillCount": skill_ids.len(),
                    "selectedSkillCount": selected_ids.len(),
                    "projectionRevision": revision,
                }),
            );
            return Ok(None);
        };
        if cache.checked_at.elapsed() > SKILL_UPDATE_REPORT_CACHE_TTL {
            tendi_core::logging::global().info(
                "skill update report cache miss",
                json!({
                    "reason": "expired",
                    "ageMs": cache.checked_at.elapsed().as_secs_f64() * 1000.0,
                    "cacheTtlMs": SKILL_UPDATE_REPORT_CACHE_TTL.as_secs_f64() * 1000.0,
                }),
            );
            return Ok(None);
        }
        let revision_matches = cache.projection_revision == revision;
        let selected_ids_present = selected_ids
            .iter()
            .all(|id| cache.reports.iter().any(|report| report.id == *id));
        let selected_skills_unchanged = scan
            .skills
            .iter()
            .filter(|skill| selected_ids.contains(skill.id.as_str()))
            .all(|skill| {
                cache
                    .skill_fingerprints
                    .get(&skill.id)
                    .is_some_and(|fingerprint| fingerprint == &skill_update_fingerprint(skill))
            });
        if selected_ids.is_empty() || !selected_ids_present || !selected_skills_unchanged {
            let reason = if !selected_skills_unchanged {
                "selected-skill-changed"
            } else if selected_ids.is_empty() {
                "selected-skills-not-found"
            } else {
                "reports-incomplete"
            };
            tendi_core::logging::global().warn(
                "skill update report cache miss",
                json!({
                    "reason": reason,
                    "requestedSkillCount": skill_ids.len(),
                    "selectedSkillCount": selected_ids.len(),
                    "cachedReportCount": cache.reports.len(),
                    "cachedProjectionRevision": cache.projection_revision,
                    "projectionRevision": revision,
                }),
            );
            return Ok(None);
        }
        if !revision_matches {
            tendi_core::logging::global().info(
                "skill update report cache reused after unrelated projection change",
                json!({
                    "requestedSkillCount": skill_ids.len(),
                    "selectedSkillCount": selected_ids.len(),
                    "cachedProjectionRevision": cache.projection_revision,
                    "projectionRevision": revision,
                }),
            );
        }
        tendi_core::logging::global().info(
            "skill update report cache hit",
            json!({
                "requestedSkillCount": skill_ids.len(),
                "selectedSkillCount": selected_ids.len(),
                "cachedReportCount": cache.reports.len(),
                "projectionRevision": revision,
            }),
        );
        Ok(Some(cache.reports.clone()))
    }

    fn skill_projection_with_revision(
        &self,
    ) -> Result<(tendi_core::skills::SkillScan, tendi_core::Revision), DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        if let Some(scan) = store
            .list_skills_for_workspace(&self.state.cwd)
            .map_err(core_error)?
        {
            let scope = daemon_scope_key(self)?;
            let revision = store
                .projection_head(&scope, "skills")
                .map_err(core_error)?
                .map(|head| head.revision)
                .unwrap_or(tendi_core::Revision::ZERO);
            return Ok((scan, revision));
        }
        drop(store);
        let scan = self.refresh_pending_skills()?;
        let revision = self.skill_projection_revision()?;
        Ok((scan, revision))
    }

    fn cached_skill_projection_with_revision(
        &self,
    ) -> Result<(tendi_core::skills::SkillScan, tendi_core::Revision), DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let (revision, scan) = store
            .read_cached_projection_with_revision::<tendi_core::skills::SkillScan>(
                "skills",
                &self.state.cwd,
            )
            .map_err(core_error)?;
        if let Some(scan) = scan {
            if store
                .projection_status("skills", &self.state.cwd)
                .map_err(core_error)?
                != tendi_core::storage::ProjectionStatus::Fresh
            {
                self.schedule_projection_refresh("skills");
            }
            return Ok((scan, revision));
        }
        drop(store);
        let scan = self.refresh_pending_skills()?;
        let revision = self.skill_projection_revision()?;
        Ok((scan, revision))
    }

    fn skill_projection(&self) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        if let Some(scan) = store
            .list_skills_for_workspace(&self.state.cwd)
            .map_err(core_error)?
        {
            return Ok(scan);
        }
        self.refresh_pending_skills()
    }

    fn refresh_pending_skills(&self) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let roots = Self::registered_project_roots(&store).map_err(core_error)?;
        for _ in 0..16 {
            let receipt = store
                .read_projection_refresh_state::<tendi_core::skills::SkillScan>(
                    "skills",
                    &self.state.cwd,
                )
                .map_err(core_error)?;
            let full = receipt.full_refresh
                || (receipt.resources.is_empty()
                    && store
                        .projection_status("skills", &self.state.cwd)
                        .map_err(core_error)?
                        != tendi_core::storage::ProjectionStatus::Fresh);
            let scan = match receipt.snapshot {
                Some(cached) => tendi_core::skills::refresh_dirty_skill_projection(
                    &self.state.cwd,
                    &store,
                    cached,
                    &receipt.resources,
                    full,
                    &roots,
                ),
                None => tendi_core::skills::scan_skills_for_project_roots_with_store(
                    &self.state.cwd,
                    &store,
                    &roots,
                ),
            }
            .map_err(core_error)?;
            if store
                .save_skills_for_workspace_if_revision(&self.state.cwd, &scan, receipt.revision)
                .map_err(core_error)?
            {
                if let Err(error) = self.configure_skill_watcher(&scan) {
                    tendi_core::logging::global().warn(
                        "skill watcher registration failed",
                        json!({"error":error.message}),
                    );
                }
                return Ok(scan);
            }
        }
        Err(conflict_error(
            "skill resources changed repeatedly during refresh",
        ))
    }

    fn skill_projection_for_mutation(&self) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        self.skill_projection()
    }

    fn skill_projection_for_preview(
        &self,
        source_paths: &[PathBuf],
    ) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        let cwd = self.state.cwd.clone();
        let store = self.open_store().map_err(core_error)?;
        if let Some(scan) = store.list_skills_for_workspace(&cwd).map_err(core_error)? {
            return Ok(scan);
        }

        let cached = self.skill_projection()?;
        for source_path in source_paths {
            if cached
                .skills
                .iter()
                .find(|skill| skill.paths.iter().any(|path| path.path == *source_path))
                .is_none()
            {
                return Err(conflict_error(
                    "skills list is stale; refresh skills before previewing this location change",
                ));
            }
        }
        Ok(cached)
    }

    fn skill_projection_for_ids(
        &self,
        ids: &[String],
    ) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        let cached = self.skill_projection_for_mutation()?;
        let scan = if ids.iter().all(|id| {
            cached
                .skills
                .iter()
                .any(|skill| tendi_core::skills::skill_matches_id(skill, id))
        }) {
            cached
        } else {
            self.skill_projection()?
        };
        let missing = ids
            .iter()
            .filter(|id| {
                !scan
                    .skills
                    .iter()
                    .any(|skill| tendi_core::skills::skill_matches_id(skill, id))
            })
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(conflict_error(format!(
                "unknown skill id(s): {}",
                missing.join(", ")
            )));
        }
        Ok(scan)
    }

    fn refresh_skill_projection(
        &self,
        before: tendi_core::skills::SkillScan,
        skill_ids: &[String],
        extra_skill_dirs: &[PathBuf],
    ) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        self.mark_skill_backup_dirty();
        let mut changed = extra_skill_dirs.to_vec();
        changed.extend(
            before
                .skills
                .iter()
                .filter(|skill| {
                    skill_ids
                        .iter()
                        .any(|id| tendi_core::skills::skill_matches_id(skill, id))
                })
                .flat_map(|skill| skill.paths.iter().map(|path| path.path.clone())),
        );
        if !changed.is_empty() {
            self.invalidate_skill_projection(&changed)?;
        }
        let scan = self.refresh_pending_skills()?;
        let store = self.open_store().map_err(core_error)?;
        let scan = tendi_core::skills::refresh_skill_scan_for_workspace(
            &self.state.cwd,
            &store,
            scan,
            skill_ids,
            extra_skill_dirs,
        )
        .map_err(core_error)?;
        let scope = daemon_scope_key(self)?;
        let revision = store
            .projection_head(&scope, "skills")
            .map_err(core_error)?
            .map(|head| head.revision)
            .unwrap_or(tendi_core::Revision::ZERO);
        if !store
            .save_skills_for_workspace_if_revision(&self.state.cwd, &scan, revision)
            .map_err(core_error)?
        {
            return Err(conflict_error(
                "skill projection changed during refresh; refresh remains pending",
            ));
        }
        self.schedule_skill_reconciliation();
        Ok(scan)
    }

    fn scan_and_persist(&self) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        self.invalidate_skill_projection(&[])?;
        let scan = self.refresh_pending_skills()?;
        self.schedule_skill_reconciliation();
        Ok(scan)
    }

    fn mark_skill_backup_dirty(&self) {
        self.state.backup_sync_dirty.store(true, Ordering::Release);
    }

    fn invalidate_skill_projection(&self, paths: &[PathBuf]) -> Result<(), DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let _projection = tendi_core::coordination::ResourceLease::acquire(
            store.path(),
            &tendi_core::coordination::shared_projection_key("skills"),
        )
        .map_err(core_error)?;
        store
            .invalidate_projection_resources("skills", &self.state.cwd, paths, true)
            .map_err(core_error)?;
        self.clear_skill_reconciliation_backoff(&self.state.cwd);
        Ok(())
    }

    fn invalidate_config_projections(&self) -> Result<(), DaemonError> {
        let store = self.open_store().map_err(core_error)?;
        let _skills_projection = tendi_core::coordination::ResourceLease::acquire(
            store.path(),
            &tendi_core::coordination::shared_projection_key("skills"),
        )
        .map_err(core_error)?;
        for domain in ["mcp", "hooks", "skills"] {
            store
                .invalidate_projection_resources(domain, &self.state.cwd, &[], domain == "skills")
                .map_err(core_error)?;
            self.schedule_projection_refresh(domain);
        }
        self.schedule_skill_reconciliation();
        Ok(())
    }

    fn schedule_skill_reconciliation(&self) {
        self.schedule_skill_reconciliation_for_scope(self.state.cwd.clone());
    }

    fn schedule_skill_reconciliation_for_scope(&self, workspace: PathBuf) {
        if !self.state.background_enabled || self.is_shutting_down() {
            return;
        }
        let workspace = tendi_core::storage::canonical_workspace_root(&workspace);
        if !self.skill_reconciliation_retry_ready(&workspace) {
            return;
        }
        let key = format!("skills-reconcile:{}", workspace.display());
        if !self
            .state
            .projection_refreshes
            .lock()
            .map(|mut pending| pending.insert(key.clone()))
            .unwrap_or(false)
        {
            return;
        }
        let cleanup_daemon = self.clone();
        let cleanup_key = key.clone();
        let cleanup = rpc_admission::Cleanup(Some(Box::new(move || {
            cleanup_daemon.clear_projection_refresh(&cleanup_key)
        })));
        let daemon = self.clone();
        let step_workspace = workspace.clone();
        let step = request_scheduler::Step::acquire(
            request_scheduler::Workload::Compute,
            Vec::new(),
            move || {
                // Keep cleanup owned by the last continuation, including rejection.
                Ok(daemon.reconciliation_step_with_cleanup(step_workspace, cleanup))
            },
        );
        let operation = tendi_core::OperationId::new(format!(
            "skills-reconcile-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
        .expect("reconciliation operation id is valid");
        tendi_core::logging::global().info(
            "skill reconciliation scheduled",
            json!({
                "operationId": operation.as_str(),
                "workspace": workspace,
                "resourceMode": "scope",
            }),
        );
        let rejected_daemon = self.clone();
        let rejected_key = key.clone();
        let rejected_workspace = workspace.clone();
        if let Err(error) = self.state.requests.submit_with_rejection(
            operation,
            step,
            Arc::new(AtomicBool::new(false)),
            move |error| {
                // The durable dirty receipt remains authoritative. Drop only
                // the in-memory claim so recovery can enqueue this scope again
                // after the backoff window.
                rejected_daemon
                    .record_skill_reconciliation_failure(&rejected_workspace, &error.to_string());
                rejected_daemon.clear_projection_refresh(&rejected_key);
                tendi_core::logging::global().warn(
                    "skill reconciliation admission rejected",
                    json!({ "workspace": rejected_workspace, "error": error.to_string() }),
                );
            },
        ) {
            tendi_core::logging::global().warn(
                "skill reconciliation scheduling failed",
                json!({ "workspace": workspace, "error": error.to_string() }),
            );
        }
    }

    fn run_scheduled_skill_backup(&self) {
        if !claim_scheduled_skill_backup(
            &self.state.backup_sync_dirty,
            &self.state.backup_sync_running,
        ) {
            return;
        }
        let daemon = self.clone();
        let operation_id = match tendi_core::OperationId::new(format!(
            "skill-backup-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )) {
            Ok(operation_id) => operation_id,
            Err(_) => {
                self.state
                    .backup_sync_running
                    .store(false, Ordering::Release);
                self.mark_skill_backup_dirty();
                return;
            }
        };
        let completed = Arc::new(AtomicBool::new(false));
        let completion = Arc::clone(&completed);
        let cleanup_daemon = self.clone();
        let cleanup = rpc_admission::Cleanup(Some(Box::new(move || {
            cleanup_daemon
                .state
                .backup_sync_running
                .store(false, Ordering::Release);
            if !completion.load(Ordering::Acquire) {
                cleanup_daemon.mark_skill_backup_dirty();
            }
        })));
        let step = self.prepare_rpc_step(
            "skills_backup_now".into(),
            json!({}),
            request_scheduler::Workload::ExternalIo,
            Box::new(move |_| {
                let _cleanup = cleanup;
                let result = (|| -> anyhow::Result<()> {
                    let store = daemon.open_store()?;
                    if store.skill_backup_config()?.is_none() {
                        return Ok(());
                    }
                    daemon
                        .refresh_backup_projections()
                        .map_err(|error| anyhow::anyhow!(error.message))?;
                    tendi_core::skill_backup::backup_now(&store, &daemon.state.cwd)?;
                    Ok(())
                })();
                completed.store(result.is_ok(), Ordering::Release);
                if let Err(error) = result {
                    daemon.mark_skill_backup_dirty();
                    tendi_core::logging::global().warn(
                        "skill sync failed",
                        json!({ "error": format!("{error:#}") }),
                    );
                }
                daemon
                    .state
                    .backup_sync_running
                    .store(false, Ordering::Release);
                Ok(Value::Null)
            }),
        );
        if self
            .state
            .requests
            .submit(operation_id, step, Arc::new(AtomicBool::new(false)))
            .is_err()
        {
            self.state
                .backup_sync_running
                .store(false, Ordering::Release);
            self.mark_skill_backup_dirty();
        }
    }

    fn refresh_backup_projections(&self) -> Result<(), DaemonError> {
        self.skill_projection()?;
        Ok(())
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
        request: &runtime_schema::SkillsAddRequest,
    ) -> Result<tendi_core::skills::SkillAddOptions, DaemonError> {
        required_request_text(&request.source, "source")?;
        required_request_text(&request.target, "target")?;
        required_request_text(&request.scope, "scope")?;
        request_text_items(&request.skills, "skills")?;
        let source = request.source.clone();
        let source = if source.trim() == tendi_core::bundled_skill::INSTALL_SOURCE {
            tendi_core::bundled_skill::source_path()
                .map_err(core_error)?
                .to_string_lossy()
                .into_owned()
        } else {
            source
        };
        Ok(tendi_core::skills::SkillAddOptions {
            source,
            target: request
                .target
                .parse()
                .map_err(|error| invalid_argument(format!("invalid skill target: {error}")))?,
            scope: request
                .scope
                .parse()
                .map_err(|error| invalid_argument(format!("invalid skill scope: {error}")))?,
            skills: request.skills.clone(),
            copy: request.copy,
            overwrite: request.overwrite,
            visibility: skill_visibility_from_request(request.visibility),
        })
    }
}

fn skill_update_fingerprint(skill: &tendi_core::skills::SkillRecord) -> String {
    format!("{}:{:?}", skill.id, skill.paths)
}

fn should_record_runtime_operation(method: &str, params: &Value) -> bool {
    !matches!(
        method,
        "session_transcript" | "session_transcript_locator" | "session_transcript_search"
    ) && !(method == "skills_update_many"
        && params
            .get("dryRun")
            .and_then(Value::as_bool)
            .unwrap_or(false))
}

fn runtime_operation_input_revision(
    store: &tendi_core::storage::Store,
    scope: &tendi_core::ScopeKey,
    method: &str,
) -> tendi_core::Revision {
    let domain = if method.starts_with("skills_") {
        "skills"
    } else if method.starts_with("sessions_") || method == "session_transcript" {
        "sessions"
    } else if method.starts_with("rules_") || method.starts_with("rule_") {
        "rules"
    } else if method.starts_with("hooks_") || method.starts_with("hook_") {
        "hooks"
    } else if method.starts_with("mcp_") {
        "mcp"
    } else {
        "sessions"
    };
    store
        .projection_head(scope, domain)
        .ok()
        .flatten()
        .map(|head| head.revision)
        .unwrap_or(tendi_core::Revision::ZERO)
}

fn skill_update_refresh_ids(
    scan: &tendi_core::skills::SkillScan,
    plan: &tendi_core::skills::SkillUpdatePlan,
) -> Vec<String> {
    let mut ids = BTreeSet::new();
    for update in &plan.source_updates {
        if let Some(skill) = scan.skills.iter().find(|skill| {
            skill
                .paths
                .iter()
                .any(|path| path.path == update.skill_path)
        }) {
            ids.insert(skill.id.clone());
        }
    }
    for change in &plan.file_changes.changes {
        for skill in &scan.skills {
            if skill
                .paths
                .iter()
                .any(|path| change.path.starts_with(&path.path))
            {
                ids.insert(skill.id.clone());
            }
        }
    }
    for action in &plan.git_updates {
        for target in &action.materialized_targets {
            if let Some(skill) = scan
                .skills
                .iter()
                .find(|skill| skill.paths.iter().any(|path| path.path == target.target))
            {
                ids.insert(skill.id.clone());
            }
        }
    }
    ids.into_iter().collect()
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

fn session_scan_is_current(
    generation: u64,
    observed_revision: u64,
    completed_revision: u64,
) -> bool {
    generation != 0 && observed_revision == completed_revision
}

fn session_scan_start_response(
    generation: u64,
    started: bool,
) -> runtime_schema::SessionScanStartResponse {
    runtime_schema::SessionScanStartResponse {
        generation,
        started,
    }
}

fn session_root_priority(root: &Path) -> u8 {
    tendi_core::session_root_priority(root)
}

fn run_session_scan(
    daemon: &Daemon,
    generation: u64,
    additional_session_roots: &[PathBuf],
    operation_id: &tendi_core::OperationId,
) -> Result<(), DaemonError> {
    let scan_started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let store = daemon.open_store().map_err(core_error)?;
    let scope_key = daemon_scope_key(daemon)?;
    let last_scan_at = store
        .sessions_last_scan_at_for_scope(&scope_key)
        .map_err(core_error)?;
    let cache = store
        .session_scan_cache_for_scope(&scope_key)
        .map_err(core_error)?;
    let mut scanned = 0;
    let mut roots =
        tendi_core::sessions::session_watch_roots(&daemon.state.cwd, additional_session_roots);
    roots.sort_by_key(|root| session_root_priority(root));
    for root in roots {
        let recent_paths = tendi_core::sessions::recent_session_paths_in_root(&root, last_scan_at);
        for paths in recent_paths.chunks(SESSION_SCAN_BATCH_SIZE) {
            let report = tendi_core::sessions::scan_session_paths(paths, &cache);
            let analytics_sessions = report.sessions.clone();
            let base_revision = store
                .projection_head(&scope_key, "sessions")
                .map_err(core_error)?
                .map(|head| head.revision)
                .unwrap_or(tendi_core::Revision::ZERO);
            let upserts = store
                .apply_session_delta_and_resolve_projects_for_scope(&scope_key, &report.sessions)
                .map_err(core_error)?;
            let revision = store
                .projection_head(&scope_key, "sessions")
                .map_err(core_error)?
                .map(|head| head.revision)
                .unwrap_or(base_revision);
            scanned += paths.len();
            daemon.emit_revisioned_event(
                SESSION_SCAN_EVENT,
                &scope_key,
                "sessions",
                operation_id,
                base_revision,
                revision,
                None,
                runtime_event(
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
                ),
            );
            let _ = daemon
                .state
                .session_runtime
                .analytics_tx
                .send(AnalyticsRefreshJob {
                    phase: "recent",
                    scope_key: scope_key.clone(),
                    sessions: analytics_sessions,
                });
        }
    }
    daemon.emit_event(
        SESSION_SCAN_EVENT,
        runtime_event(
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
        ),
    );

    let cache = store
        .session_scan_cache_for_scope(&scope_key)
        .map_err(core_error)?;
    let report = tendi_core::sessions::scan_sessions_with_additional_roots_cached(
        &daemon.state.cwd,
        additional_session_roots,
        &cache,
    )
    .map_err(core_error)?;
    let base_revision = store
        .projection_head(&scope_key, "sessions")
        .map_err(core_error)?
        .map(|head| head.revision)
        .unwrap_or(tendi_core::Revision::ZERO);
    for sessions in report.sessions.chunks(SESSION_SCAN_PERSIST_BATCH_SIZE) {
        store
            .apply_session_delta_and_resolve_projects_for_scope(&scope_key, sessions)
            .map_err(core_error)?;
    }
    store
        .finalize_session_scan_for_scope(&scope_key, &report, scan_started_at)
        .map_err(core_error)?;
    let revision = store
        .projection_head(&scope_key, "sessions")
        .map_err(core_error)?
        .map(|head| head.revision)
        .unwrap_or(base_revision);
    let analytics_sessions = report.sessions.clone();
    daemon.emit_revisioned_event(
        SESSION_SCAN_EVENT,
        &scope_key,
        "sessions",
        operation_id,
        base_revision,
        revision,
        None,
        runtime_event(
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
        ),
    );
    let _ = daemon
        .state
        .session_runtime
        .analytics_tx
        .send(AnalyticsRefreshJob {
            phase: "backfill",
            scope_key: scope_key.clone(),
            sessions: analytics_sessions,
        });
    Ok(())
}

fn daemon_scope_key(daemon: &Daemon) -> Result<tendi_core::ScopeKey, DaemonError> {
    tendi_core::storage::workspace_scope_key(&daemon.state.cwd)
        .map_err(|error| core_error(anyhow::anyhow!(error)))
}

fn event_projection_domain(event: &str, payload: &Value) -> Option<String> {
    match event {
        SESSION_SCAN_EVENT => Some("sessions".to_string()),
        ANALYTICS_PROGRESS_EVENT | ANALYTICS_REVISION_EVENT => Some("analytics".to_string()),
        SKILL_UPDATE_EVENT | SKILL_CHANGED_EVENT => Some("skills".to_string()),
        PROJECTION_CHANGED_EVENT => payload
            .get("domain")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
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
        if daemon.is_shutting_down() {
            break;
        }
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
        let paths = std::mem::take(&mut pending).into_iter().collect::<Vec<_>>();
        if let Err(error) = daemon.invalidate_config_projections() {
            tendi_core::logging::global().warn(
                "config projection invalidation failed",
                json!({"error": error.message}),
            );
        }
        for path in paths {
            match tendi_core::config::read_agent_config(&path) {
                Ok(snapshot) => daemon.emit_event(
                    CONFIG_CHANGED_EVENT,
                    runtime_event(
                        CONFIG_CHANGED_EVENT,
                        serde_json::to_value(snapshot).expect("config event serializes"),
                    ),
                ),
                Err(error) => tendi_core::logging::global().warn(
                    "config change snapshot failed",
                    json!({ "path": path, "error": error.to_string() }),
                ),
            }
        }
    }
}

fn skill_watch_loop(daemon: Daemon, receiver: Receiver<notify::Result<Event>>) {
    let mut pending = BTreeSet::new();
    loop {
        if daemon.is_shutting_down() {
            break;
        }
        match receiver.recv_timeout(CONFIG_WATCH_DEBOUNCE) {
            Ok(Ok(event)) => pending.extend(
                event
                    .paths
                    .into_iter()
                    .filter(|path| !is_skill_watcher_transient_path(path)),
            ),
            Ok(Err(error)) => {
                tendi_core::logging::global().error(
                    "skill watcher failed",
                    json!({ "error": error.to_string() }),
                );
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if pending.is_empty() {
            continue;
        }
        let paths = std::mem::take(&mut pending).into_iter().collect::<Vec<_>>();
        if let Err(error) = daemon.reconcile_skill_visibility_after_external_change(&paths) {
            tendi_core::logging::global().warn(
                "skill visibility reconciliation failed",
                json!({ "error": error.message }),
            );
        }
        let paths = paths
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        daemon.emit_event(
            SKILL_CHANGED_EVENT,
            runtime_event(SKILL_CHANGED_EVENT, json!({ "paths": paths })),
        );
    }
}

fn is_skill_watcher_transient_path(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|name| name.contains(".tendi-tmp-"))
    })
}

fn claim_scheduled_skill_backup(dirty: &AtomicBool, running: &AtomicBool) -> bool {
    if running.swap(true, Ordering::AcqRel) {
        return false;
    }
    if !dirty.swap(false, Ordering::AcqRel) {
        running.store(false, Ordering::Release);
        return false;
    }
    true
}

fn backup_sync_loop(daemon: Daemon) {
    while !daemon.is_shutting_down() {
        let started = Instant::now();
        while started.elapsed() < BACKUP_SYNC_INTERVAL && !daemon.is_shutting_down() {
            thread::sleep(Duration::from_millis(100));
        }
        if daemon.is_shutting_down() {
            break;
        }
        daemon.run_scheduled_skill_backup();
    }
}

fn projection_recovery_loop(daemon: Daemon) {
    let mut retry_delay = DATABASE_RECOVERY_RETRY_INITIAL;
    while !daemon.is_shutting_down() {
        match daemon
            .open_store()
            .and_then(|store| store.pending_projection_scopes("skills"))
        {
            Ok(scopes) => {
                retry_delay = DATABASE_RECOVERY_RETRY_INITIAL;
                for scope in scopes {
                    if daemon.skill_reconciliation_retry_ready(&scope) {
                        daemon.schedule_skill_reconciliation_for_scope(scope);
                    }
                }
            }
            Err(error) => {
                if tendi_core::storage::is_database_io_error(&error) {
                    daemon.recover_storage(&error);
                } else {
                    tendi_core::logging::global().warn(
                        "projection recovery enumeration failed",
                        json!({"error":error.to_string()}),
                    );
                }
                retry_delay =
                    std::cmp::min(retry_delay.saturating_mul(2), DATABASE_RECOVERY_RETRY_MAX);
            }
        }
        sleep_worker_retry(&daemon, retry_delay);
    }
}

fn session_watch_loop(daemon: Daemon, receiver: Receiver<notify::Result<Event>>) {
    let runtime = Arc::clone(&daemon.state.session_runtime);
    let mut pending = BTreeSet::new();
    let mut pending_since = None;
    let mut pending_live_previews = BTreeSet::new();
    let mut live_preview_since = None;
    loop {
        if daemon.is_shutting_down() {
            break;
        }
        if let Some(paths) = take_due_session_watch_retries(&runtime) {
            pending.extend(paths);
            pending_since.get_or_insert_with(Instant::now);
        }
        match receiver.recv_timeout(SESSION_WATCH_DEBOUNCE) {
            Ok(Ok(event)) => {
                let mut relevant = false;
                let mut relevant_paths = Vec::new();
                let event_paths = event.paths;
                for path in event_paths.iter().cloned() {
                    if let Some(session_root) = advance_session_watcher(&runtime, &path) {
                        relevant = true;
                        relevant_paths.push(path.clone());
                        pending.extend(tendi_core::sessions::recent_session_paths_in_root(
                            &session_root,
                            None,
                        ));
                    }
                    if tendi_core::sessions::is_session_candidate_path(&path) || !path.exists() {
                        relevant = true;
                        relevant_paths.push(path.clone());
                        pending.insert(path.clone());
                    }
                    if tendi_core::sessions::is_session_candidate_path(&path) {
                        if path.is_file() {
                            pending_live_previews.insert(path);
                            if live_preview_since.is_none() {
                                live_preview_since = Some(Instant::now());
                            }
                        } else {
                            pending_live_previews.remove(&path);
                        }
                    }
                }
                if relevant {
                    runtime.watch_revision.fetch_add(1, Ordering::AcqRel);
                    tendi_core::logging::global().info(
                        "session watcher event queued",
                        json!({
                            "eventPathCount": event_paths.len(),
                            "relevantPathCount": relevant_paths.len(),
                            "pendingPathCount": pending.len(),
                            "pendingLivePreviewCount": pending_live_previews.len(),
                            "paths": relevant_paths,
                        }),
                    );
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
                    runtime_event(
                        SESSION_SCAN_EVENT,
                        json!({
                            "generation": generation,
                            "phase": "watch",
                            "upserts": [],
                            "deleted": [],
                            "scanned": 0,
                            "complete": false,
                            "error": message,
                        }),
                    ),
                );
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if runtime.scan_running.load(Ordering::Acquire) {
            if !pending_live_previews.is_empty()
                && live_preview_since
                    .is_some_and(|started| started.elapsed() >= SESSION_WATCH_DEBOUNCE)
            {
                let paths = std::mem::take(&mut pending_live_previews)
                    .into_iter()
                    .collect::<Vec<_>>();
                live_preview_since = None;
                emit_live_session_watch_previews(&daemon, &paths);
            }
            continue;
        }
        if pending.is_empty()
            || !pending_since.is_some_and(|started| started.elapsed() >= SESSION_WATCH_DEBOUNCE)
        {
            continue;
        }
        let paths = std::mem::take(&mut pending).into_iter().collect::<Vec<_>>();
        pending_live_previews.clear();
        live_preview_since = None;
        pending_since = None;
        tendi_core::logging::global().info(
            "session watcher dispatching batch",
            json!({
                "pathCount": paths.len(),
                "generation": runtime.generation.load(Ordering::SeqCst),
                "paths": &paths,
            }),
        );
        let operation_id = tendi_core::OperationId::new(format!(
            "session-watch-dispatch-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let daemon_for_job = daemon.clone();
        let paths_for_job = paths.clone();
        if let Ok(operation_id) = operation_id {
            if daemon
                .state
                .session_operations
                .submit(operation_id, move || {
                    process_session_watch_paths(&daemon_for_job, &paths_for_job)
                })
                .is_err()
            {
                pending.extend(paths.iter().cloned());
                pending_since.get_or_insert_with(Instant::now);
                tendi_core::logging::global().warn(
                    "session watcher operation queue is full",
                    json!({ "paths": paths }),
                );
            }
        } else {
            pending.extend(paths.iter().cloned());
            pending_since.get_or_insert_with(Instant::now);
        }
    }
}

fn take_due_session_watch_retries(runtime: &SessionRuntime) -> Option<Vec<PathBuf>> {
    let mut retry = runtime.retry.lock().ok()?;
    let Some(retry_at) = retry.retry_at else {
        return None;
    };
    if retry_at > Instant::now() {
        return None;
    }
    retry.retry_at = None;
    Some(std::mem::take(&mut retry.paths).into_iter().collect())
}

fn schedule_session_watch_retry(runtime: &SessionRuntime, paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    let Ok(mut retry) = runtime.retry.lock() else {
        tendi_core::logging::global().error(
            "session watcher retry state is unavailable",
            json!({ "paths": paths }),
        );
        return;
    };
    retry.paths.extend(paths.iter().cloned());
    let retry_at = Instant::now() + retry.delay;
    retry.retry_at = Some(
        retry
            .retry_at
            .map_or(retry_at, |current| current.min(retry_at)),
    );
    retry.delay = std::cmp::min(
        retry
            .delay
            .checked_mul(2)
            .unwrap_or(SESSION_WATCH_RETRY_MAX),
        SESSION_WATCH_RETRY_MAX,
    );
}

fn complete_session_watch_paths(runtime: &SessionRuntime, paths: &[PathBuf]) {
    let Ok(mut retry) = runtime.retry.lock() else {
        return;
    };
    for path in paths {
        retry.paths.remove(path);
    }
    if retry.paths.is_empty() {
        retry.retry_at = None;
        retry.delay = SESSION_WATCH_RETRY_INITIAL;
    }
}

fn emit_live_session_watch_previews(daemon: &Daemon, paths: &[PathBuf]) {
    let scope_key = match daemon_scope_key(daemon) {
        Ok(scope_key) => scope_key,
        Err(error) => {
            tendi_core::logging::global().debug(
                "live session preview skipped",
                json!({ "phase": "watch", "reason": "scope", "error": error.message }),
            );
            return;
        }
    };
    let store = match daemon.open_store() {
        Ok(store) => store,
        Err(error) => {
            tendi_core::logging::global().debug(
                "live session preview skipped",
                json!({ "phase": "watch", "reason": "store", "error": error.to_string() }),
            );
            return;
        }
    };
    let cache = match store.session_scan_cache_for_scope(&scope_key) {
        Ok(cache) => cache,
        Err(error) => {
            tendi_core::logging::global().debug(
                "live session preview skipped",
                json!({ "phase": "watch", "reason": "cache", "error": error.to_string() }),
            );
            return;
        }
    };
    let sessions = live_session_watch_previews(paths, &cache);
    if sessions.is_empty() {
        return;
    }
    daemon.emit_event(
        SESSION_SCAN_EVENT,
        runtime_event(
            SESSION_SCAN_EVENT,
            json!({
                "generation": daemon.state.session_runtime.generation.load(Ordering::SeqCst),
                "phase": "watch",
                "upserts": sessions,
                "deleted": [],
                "scanned": paths.len(),
                "complete": true,
                "error": Value::Null,
            }),
        ),
    );
}

fn live_session_watch_previews(
    paths: &[PathBuf],
    cache: &tendi_core::sessions::SessionScanCache,
) -> Vec<tendi_core::SessionRecord> {
    let paths = paths
        .iter()
        .filter(|path| path.is_file() && tendi_core::sessions::is_session_candidate_path(path))
        .cloned()
        .collect::<Vec<_>>();
    tendi_core::sessions::scan_session_paths(&paths, cache).sessions
}

fn process_session_watch_paths(daemon: &Daemon, paths: &[PathBuf]) {
    let runtime = &daemon.state.session_runtime;
    tendi_core::logging::global().info(
        "session watcher update started",
        json!({
            "pathCount": paths.len(),
            "generation": runtime.generation.load(Ordering::SeqCst),
            "paths": paths,
        }),
    );
    let scope_key = match daemon_scope_key(daemon) {
        Ok(scope_key) => scope_key,
        Err(error) => {
            tendi_core::logging::global().error(
                "session watcher scope resolution failed",
                json!({ "error": &error.message }),
            );
            schedule_session_watch_retry(runtime, paths);
            return;
        }
    };
    let result = process_session_watch_paths_once(daemon, paths, &scope_key);

    match result {
        Ok((upserts, deleted, analytics_sessions, base_revision, revision, operation_id))
            if !upserts.is_empty() || !deleted.is_empty() || !analytics_sessions.is_empty() =>
        {
            complete_session_watch_paths(runtime, paths);
            let has_session_delta = !upserts.is_empty() || !deleted.is_empty();
            let payload = json!({
                "generation": daemon.state.session_runtime.generation.load(Ordering::SeqCst),
                "phase": "watch",
                "upserts": upserts,
                "deleted": deleted,
                "scanned": paths.len(),
                "complete": true,
                "error": Value::Null,
            });
            if has_session_delta && base_revision != revision {
                daemon.emit_revisioned_event(
                    SESSION_SCAN_EVENT,
                    &scope_key,
                    "sessions",
                    &operation_id,
                    base_revision,
                    revision,
                    None,
                    runtime_event(SESSION_SCAN_EVENT, payload),
                );
            } else {
                daemon.emit_event(
                    SESSION_SCAN_EVENT,
                    runtime_event(SESSION_SCAN_EVENT, payload),
                );
            }
            let _ = daemon
                .state
                .session_runtime
                .analytics_tx
                .send(AnalyticsRefreshJob {
                    phase: "watch",
                    scope_key: scope_key.clone(),
                    sessions: analytics_sessions,
                });
        }
        Ok((upserts, deleted, _analytics_sessions, base_revision, revision, operation_id)) => {
            complete_session_watch_paths(runtime, paths);
            tendi_core::logging::global().info(
                "session watcher update completed without session delta",
                json!({
                    "pathCount": paths.len(),
                    "upsertCount": upserts.len(),
                    "deletedCount": deleted.len(),
                    "baseRevision": base_revision,
                    "revision": revision,
                    "operationId": &operation_id,
                }),
            );
        }
        Err(error) => {
            daemon.recover_storage_error(&error);
            schedule_session_watch_retry(runtime, paths);
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
                runtime_event(SESSION_SCAN_EVENT, json!({
                    "generation": daemon.state.session_runtime.generation.load(Ordering::SeqCst),
                    "phase": "watch",
                    "upserts": [],
                    "deleted": [],
                    "scanned": paths.len(),
                    "complete": true,
                    "error": error.message,
                })),
            );
        }
    }
}

fn process_session_watch_paths_once(
    daemon: &Daemon,
    paths: &[PathBuf],
    scope_key: &tendi_core::ScopeKey,
) -> Result<
    (
        Vec<tendi_core::SessionRecord>,
        Vec<tendi_core::sessions::SessionIdentity>,
        Vec<tendi_core::SessionRecord>,
        tendi_core::Revision,
        tendi_core::Revision,
        tendi_core::OperationId,
    ),
    DaemonError,
> {
    let store = daemon.open_store().map_err(core_error)?;
    let operation_id = tendi_core::OperationId::new(format!(
        "session-watch-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        paths.len()
    ))
    .map_err(|error| core_error(anyhow::anyhow!(error)))?;
    let base_revision = store
        .projection_head(scope_key, "sessions")
        .map_err(core_error)?
        .map(|head| head.revision)
        .unwrap_or(tendi_core::Revision::ZERO);
    let cache = store
        .session_scan_cache_for_scope(scope_key)
        .map_err(core_error)?;
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
    tendi_core::logging::global().info(
        "session watcher scan inputs resolved",
        json!({
            "pathCount": paths.len(),
            "existingPathCount": existing_paths.len(),
            "deletedPathCount": deleted_paths.len(),
            "existingPaths": &existing_paths,
            "deletedPaths": &deleted_paths,
        }),
    );
    let report = tendi_core::sessions::scan_session_paths(&existing_paths, &cache);
    tendi_core::logging::global().info(
        "session watcher scan completed",
        json!({
            "pathCount": paths.len(),
            "sessionCount": report.sessions.len(),
            "warningCount": report.warnings.len(),
            "warnings": &report.warnings,
            "sessions": report
                .sessions
                .iter()
                .map(session_watch_log_summary)
                .collect::<Vec<_>>(),
        }),
    );
    let analytics_sessions = report.sessions.clone();
    let empty_paths = existing_paths
        .iter()
        .filter(|path| tendi_core::sessions::is_session_candidate_path(path))
        .filter(|path| !report.sessions.iter().any(|session| session.path == **path))
        .cloned()
        .collect::<Vec<_>>();
    let mut removed_paths = deleted_paths;
    removed_paths.extend(empty_paths);
    let (upserts, deleted) = store
        .apply_session_changes_for_scope(scope_key, &report.sessions, &removed_paths)
        .map_err(core_error)?;
    let revision = store
        .projection_head(scope_key, "sessions")
        .map_err(core_error)?
        .map(|head| head.revision)
        .unwrap_or(base_revision);
    tendi_core::logging::global().info(
        "session watcher projection applied",
        json!({
            "inputSessionCount": report.sessions.len(),
            "upsertCount": upserts.len(),
            "deletedCount": deleted.len(),
            "removedPathCount": removed_paths.len(),
            "baseRevision": base_revision,
            "revision": revision,
            "operationId": &operation_id,
            "upserts": upserts
                .iter()
                .map(session_watch_log_summary)
                .collect::<Vec<_>>(),
        }),
    );
    Ok((
        upserts,
        deleted,
        analytics_sessions,
        base_revision,
        revision,
        operation_id,
    ))
}

fn session_watch_log_summary(session: &tendi_core::SessionRecord) -> Value {
    json!({
        "id": session.id,
        "agent": session.agent.label(),
        "path": session.path,
        "messageCount": session.message_count,
        "userLastPresent": session.last_user_message.as_ref().is_some_and(|message| !message.is_empty()),
        "assistantLastPresent": session.last_assistant_message.as_ref().is_some_and(|message| !message.is_empty()),
        "updatedAt": session.updated_at,
    })
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
    scope_key: &tendi_core::ScopeKey,
    sessions: &[tendi_core::SessionRecord],
) -> Result<tendi_core::analytics::AnalyticsRefreshReport, DaemonError> {
    let initial = tendi_core::analytics::AnalyticsRefreshProgress {
        total: sessions.len(),
        ..Default::default()
    };
    daemon.emit_event(
        ANALYTICS_PROGRESS_EVENT,
        runtime_event(
            ANALYTICS_PROGRESS_EVENT,
            json!({
                "phase": phase,
                "completed": initial.completed,
                "total": initial.total,
                "running": true,
                "error": Value::Null,
            }),
        ),
    );
    let last_progress = Arc::new(Mutex::new(initial));
    let progress_state = Arc::clone(&last_progress);
    let daemon_for_job = daemon.clone();
    let scope_key = scope_key.clone();
    let sessions = sessions.to_vec();
    let operation_id = tendi_core::OperationId::new(format!(
        "analytics-{}-{}",
        phase,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
    .map_err(|error| core_error(anyhow::anyhow!(error)))?;
    let result = match daemon
        .state
        .analytics_operations
        .execute(operation_id, move || {
            let store = daemon_for_job.open_store()?;
            store.refresh_session_analytics_for_scope_with_progress(
                &scope_key,
                &sessions,
                |progress| {
                    if let Ok(mut current) = progress_state.lock() {
                        *current = progress;
                    }
                    daemon_for_job.emit_event(
                        ANALYTICS_PROGRESS_EVENT,
                        runtime_event(
                            ANALYTICS_PROGRESS_EVENT,
                            json!({
                                "phase": phase,
                                "completed": progress.completed,
                                "total": progress.total,
                                "running": progress.completed < progress.total,
                                "error": Value::Null,
                            }),
                        ),
                    );
                },
            )
        }) {
        Ok(result) => result.map_err(core_error),
        Err(error) => Err(internal_error(format!(
            "analytics operation could not be queued: {error:?}"
        ))),
    };
    match result {
        Ok(report) => {
            let final_progress = last_progress
                .lock()
                .map(|progress| *progress)
                .unwrap_or(initial);
            daemon.emit_event(
                ANALYTICS_PROGRESS_EVENT,
                runtime_event(
                    ANALYTICS_PROGRESS_EVENT,
                    json!({
                        "phase": phase,
                        "completed": final_progress.total,
                        "total": final_progress.total,
                        "running": false,
                        "error": Value::Null,
                    }),
                ),
            );
            Ok(report)
        }
        Err(error) => {
            daemon.recover_storage_error(&error);
            let message = error.message.clone();
            let last_progress = last_progress
                .lock()
                .map(|progress| *progress)
                .unwrap_or(initial);
            daemon.emit_event(
                ANALYTICS_PROGRESS_EVENT,
                runtime_event(
                    ANALYTICS_PROGRESS_EVENT,
                    json!({
                        "phase": phase,
                        "completed": last_progress.completed,
                        "total": last_progress.total,
                        "running": false,
                        "error": message,
                    }),
                ),
            );
            Err(core_error(message))
        }
    }
}

/// Derived search work has its own lifecycle: a metadata commit never waits
/// for transcript parsing. Persisted dirty scopes survive queue saturation,
/// process restarts and failures; this worker also repairs them on startup.
fn session_search_loop(daemon: Daemon) {
    let mut retry_delay = DATABASE_RECOVERY_RETRY_INITIAL;
    while !daemon.is_shutting_down() {
        let result = (|| -> anyhow::Result<()> {
            let store = daemon.open_store()?;
            let active_scope =
                daemon_scope_key(&daemon).map_err(|error| anyhow::anyhow!(error.message))?;
            // Other workspaces belong to their own daemon/CLI lifecycle. An
            // unsolicited event must not establish another workspace's UI scope.
            for scope_key in store
                .pending_session_search_scopes()?
                .into_iter()
                .filter(|scope| scope == &active_scope)
            {
                if daemon.is_shutting_down() {
                    break;
                }
                let (publications, warnings, _pending) = match store
                    .refresh_pending_session_search_for_scope_until(&scope_key, || {
                        daemon.is_shutting_down()
                    }) {
                    Ok(report) => report,
                    Err(error) => {
                        if tendi_core::storage::is_database_io_error(&error) {
                            daemon.recover_storage(&error);
                            return Err(error);
                        }
                        tendi_core::logging::global().warn(
                            "session search scope deferred",
                            json!({"scopeKey": scope_key, "error": format!("{error:#}")}),
                        );
                        continue;
                    }
                };
                for warning in warnings {
                    tendi_core::logging::global().warn(
                        "session search refresh deferred",
                        json!({"scopeKey": scope_key, "error": warning}),
                    );
                }
                for publication in publications {
                    let operation_id = tendi_core::OperationId::new(format!(
                        "session-search-{}",
                        publication.revision.value()
                    ))
                    .map_err(|error| anyhow::anyhow!(error))?;
                    daemon.emit_revisioned_event(
                        SESSION_SCAN_EVENT, &scope_key, "sessions", &operation_id,
                        publication.base_revision, publication.revision, None,
                        runtime_event(SESSION_SCAN_EVENT, json!({
                            "generation": daemon.state.session_runtime.generation.load(Ordering::Acquire),
                            "phase": "watch", "scanned": 1, "upserts": [publication.session],
                            "deleted": [], "complete": true, "error": Value::Null,
                        })),
                    );
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => retry_delay = DATABASE_RECOVERY_RETRY_INITIAL,
            Err(error) => {
                if tendi_core::storage::is_database_io_error(&error) {
                    daemon.recover_storage(&error);
                } else {
                    tendi_core::logging::global().warn(
                        "session search worker deferred",
                        json!({"error": format!("{error:#}")}),
                    );
                }
                retry_delay =
                    std::cmp::min(retry_delay.saturating_mul(2), DATABASE_RECOVERY_RETRY_MAX);
            }
        }
        // A failed transcript is retried later, never in a tight writer loop.
        sleep_worker_retry(&daemon, retry_delay);
    }
}

fn sleep_worker_retry(daemon: &Daemon, delay: Duration) {
    let deadline = Instant::now() + delay;
    while !daemon.is_shutting_down() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        thread::sleep(remaining.min(Duration::from_millis(100)));
    }
}

fn session_analytics_loop(daemon: Daemon, receiver: Receiver<AnalyticsRefreshJob>) {
    loop {
        if daemon.is_shutting_down() {
            break;
        }
        let received = match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(job) => Some(job),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let mut phase = "backfill";
        let mut refresh_requested = false;
        let mut pending =
            BTreeMap::<String, (tendi_core::ScopeKey, tendi_core::SessionRecord)>::new();
        for job in received.into_iter().chain(receiver.try_iter()) {
            phase = job.phase;
            refresh_requested = true;
            for session in job.sessions {
                let key = format!(
                    "{}\0{:?}\0{}\0{}",
                    job.scope_key,
                    session.agent,
                    session.id,
                    session.path.display()
                );
                pending.insert(key, (job.scope_key.clone(), session));
            }
        }
        if refresh_requested {
            let mut by_scope =
                BTreeMap::<tendi_core::ScopeKey, Vec<tendi_core::SessionRecord>>::new();
            for (_key, (scope_key, session)) in pending {
                by_scope.entry(scope_key).or_default().push(session);
            }
            for (scope_key, sessions) in by_scope {
                if refresh_session_analytics_serialized(&daemon, phase, &scope_key, &sessions)
                    .is_ok()
                {
                    // Readers are deliberately short-lived. A Store kept by
                    // this worker can retain a WAL file descriptor that was
                    // replaced by another process while the worker was idle.
                    if let Ok(store) = daemon.open_store()
                        && let Ok(Some(head)) = store.projection_head(&scope_key, "analytics")
                    {
                        daemon.emit_event(
                            ANALYTICS_REVISION_EVENT,
                            runtime_event(
                                ANALYTICS_REVISION_EVENT,
                                json!({
                                    "scopeKey": scope_key,
                                    "revision": head.revision.value()
                                }),
                            ),
                        );
                    }
                }
            }
        }
    }
}

fn skills_matching_ids(
    skills: &[tendi_core::skills::SkillRecord],
    ids: &[String],
) -> Vec<tendi_core::skills::SkillRecord> {
    skills
        .iter()
        .filter(|skill| {
            ids.iter()
                .any(|id| tendi_core::skills::skill_matches_id(skill, id))
        })
        .cloned()
        .collect()
}

fn skills_matching_paths(
    skills: &[tendi_core::skills::SkillRecord],
    paths: &[PathBuf],
) -> Vec<tendi_core::skills::SkillRecord> {
    skills
        .iter()
        .filter(|skill| {
            skill.paths.iter().any(|skill_path| {
                paths.iter().any(|path| {
                    skill_path.path == *path
                        || skill_path.path.canonicalize().ok().as_ref()
                            == path.canonicalize().ok().as_ref()
                })
            })
        })
        .cloned()
        .collect()
}

fn skills_matching_ids_or_paths(
    skills: &[tendi_core::skills::SkillRecord],
    ids: &[String],
    paths: &[PathBuf],
) -> Vec<tendi_core::skills::SkillRecord> {
    let mut matched = skills_matching_ids(skills, ids);
    let existing = matched
        .iter()
        .map(|skill| skill.id.clone())
        .collect::<BTreeSet<_>>();
    matched.extend(
        skills_matching_paths(skills, paths)
            .into_iter()
            .filter(|skill| !existing.contains(&skill.id)),
    );
    matched
}

fn valid_json_rpc_id(value: &Value) -> bool {
    value.is_null() || value.is_string() || value.as_i64().is_some() || value.as_u64().is_some()
}

fn rpc_error_code(kind: &str) -> i32 {
    runtime_schema::error_code(kind)
}

fn rpc_error_response(
    id: Value,
    code: i32,
    kind: &str,
    message: &str,
    details: Option<Value>,
) -> Value {
    serde_json::to_value(runtime_schema::JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: None,
        error: Some(runtime_schema::JsonRpcError {
            code,
            message: message.to_string(),
            data: Some(runtime_schema::JsonRpcErrorData {
                kind: kind.to_string(),
                details,
            }),
        }),
    })
    .expect("JSON-RPC error serializes")
}

fn distribution_targets(
    request: &runtime_schema::SkillsDistributeRequest,
) -> Result<Vec<tendi_core::SkillTarget>, DaemonError> {
    if let Some(values) = request.targets.as_ref() {
        if values.is_empty() {
            return Err(invalid_argument("targets must not be empty"));
        }
        request_text_items(values, "targets")?;
        return values
            .iter()
            .map(|value| value.parse::<tendi_core::SkillTarget>().map_err(core_error))
            .collect();
    }
    let target = request
        .target
        .as_deref()
        .ok_or_else(|| invalid_argument("missing argument: target or targets"))?;
    required_request_text(target, "target")?;
    Ok(vec![
        target
            .parse::<tendi_core::SkillTarget>()
            .map_err(core_error)?,
    ])
}

fn backup_contents_from_request(
    contents: runtime_schema::BackupContents,
) -> tendi_core::skill_backup::BackupContents {
    fn selection(
        value: runtime_schema::BackupCategorySelection,
    ) -> tendi_core::skill_backup::BackupCategorySelection {
        tendi_core::skill_backup::BackupCategorySelection {
            enabled: value.enabled,
            excluded: value.excluded,
        }
    }

    tendi_core::skill_backup::BackupContents {
        skills: selection(contents.skills),
        mcp: selection(contents.mcp),
        rules: selection(contents.rules),
        hooks: selection(contents.hooks),
    }
}

fn skills_runtime_value(skills: &[tendi_core::skills::SkillRecord]) -> Result<Value, DaemonError> {
    let mut value = serde_json::to_value(skills).map_err(internal_error)?;
    let records = value
        .as_array_mut()
        .ok_or_else(|| internal_error("skill records must serialize as an array"))?;
    for (record, skill) in records.iter_mut().zip(skills) {
        let Some(paths) = record.get_mut("paths").and_then(Value::as_array_mut) else {
            continue;
        };
        for (path_value, path) in paths.iter_mut().zip(&skill.paths) {
            let object = path_value
                .as_object_mut()
                .ok_or_else(|| internal_error("skill location must serialize as an object"))?;
            object.insert(
                "locationId".to_string(),
                json!(tendi_core::skills::skill_location_id(path)),
            );
        }
    }
    Ok(value)
}

fn hook_record_runtime_value(hook: &tendi_core::hooks::HookRecord) -> Result<Value, DaemonError> {
    let mut value = serde_json::to_value(hook).map_err(internal_error)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| internal_error("hook record must serialize as an object"))?;
    object.insert(
        "id".to_string(),
        json!(tendi_core::hooks::hook_record_id(hook)),
    );
    Ok(value)
}

fn hook_records_runtime_value(
    hooks: &[tendi_core::hooks::HookRecord],
) -> Result<Value, DaemonError> {
    hooks
        .iter()
        .map(hook_record_runtime_value)
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn hook_mutation_delta_value(
    scan: &tendi_core::hooks::HookScan,
    paths: &[PathBuf],
    deleted: Vec<tendi_core::hooks::HookRecord>,
) -> Result<Value, DaemonError> {
    let path_set = paths.iter().collect::<std::collections::HashSet<_>>();
    let updated = scan
        .hooks
        .iter()
        .filter(|hook| path_set.contains(&hook.path))
        .map(hook_record_runtime_value)
        .collect::<Result<Vec<_>, _>>()?;
    let deleted = deleted
        .iter()
        .map(hook_record_runtime_value)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({ "updated": updated, "deleted": deleted }))
}

fn parse_agent(value: &str) -> Result<tendi_core::AgentKind, DaemonError> {
    tendi_core::parse_agent(value).map_err(core_error)
}

fn required_request_text(value: &str, name: &str) -> Result<(), DaemonError> {
    if value.trim().is_empty() {
        return Err(invalid_argument(format!(
            "missing or empty argument: {name}"
        )));
    }
    Ok(())
}

fn required_request_texts(values: &[String], name: &str) -> Result<(), DaemonError> {
    if values.is_empty() {
        return Err(invalid_argument(format!("{name} must not be empty")));
    }
    if values.iter().any(|value| value.trim().is_empty()) {
        return Err(invalid_argument(format!(
            "argument values must be non-empty strings: {name}"
        )));
    }
    Ok(())
}

fn request_text_items(values: &[String], name: &str) -> Result<(), DaemonError> {
    if values.iter().any(|value| value.trim().is_empty()) {
        return Err(invalid_argument(format!(
            "argument values must be non-empty strings: {name}"
        )));
    }
    Ok(())
}

fn optional_request_text(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn agent_kind_from_request(value: runtime_schema::AgentKind) -> tendi_core::AgentKind {
    match value {
        runtime_schema::AgentKind::Codex => tendi_core::AgentKind::Codex,
        runtime_schema::AgentKind::Cursor => tendi_core::AgentKind::Cursor,
        runtime_schema::AgentKind::Claude => tendi_core::AgentKind::Claude,
        runtime_schema::AgentKind::Shared => tendi_core::AgentKind::Shared,
        runtime_schema::AgentKind::Unknown => tendi_core::AgentKind::Unknown,
    }
}

fn session_list_sort_key(value: runtime_schema::SessionListSortKey) -> String {
    match value {
        runtime_schema::SessionListSortKey::Title => "title",
        runtime_schema::SessionListSortKey::Agent => "agent",
        runtime_schema::SessionListSortKey::Project => "project",
        runtime_schema::SessionListSortKey::StartedAt => "startedAt",
        runtime_schema::SessionListSortKey::UpdatedAt => "updatedAt",
        runtime_schema::SessionListSortKey::Messages => "messages",
        runtime_schema::SessionListSortKey::Turns => "turns",
        runtime_schema::SessionListSortKey::CacheRate => "cacheRate",
        runtime_schema::SessionListSortKey::SearchScore => "searchScore",
    }
    .to_string()
}

fn session_list_sort_direction(value: runtime_schema::SessionListSortDirection) -> String {
    match value {
        runtime_schema::SessionListSortDirection::Asc => "asc",
        runtime_schema::SessionListSortDirection::Desc => "desc",
    }
    .to_string()
}

fn bundled_skill_agent(value: Option<runtime_schema::AgentKind>) -> tendi_core::AgentKind {
    value
        .map(agent_kind_from_request)
        .unwrap_or(tendi_core::AgentKind::Shared)
}

fn session_identity_from_request(
    value: runtime_schema::SessionIdentity,
) -> tendi_core::sessions::SessionIdentity {
    tendi_core::sessions::SessionIdentity {
        id: value.id,
        agent: agent_kind_from_request(value.agent),
        path: PathBuf::from(value.path),
    }
}

fn hook_for_id<'a>(
    hooks: &'a [tendi_core::hooks::HookRecord],
    id: &str,
) -> Result<&'a tendi_core::hooks::HookRecord, DaemonError> {
    hooks
        .iter()
        .find(|hook| tendi_core::hooks::hook_matches_id(hook, id))
        .ok_or_else(|| {
            conflict_error(
                "hook is not present in the current projection; refresh hooks before changing it",
            )
        })
}

fn hook_delete_request_for_record(
    hook: &tendi_core::hooks::HookRecord,
) -> tendi_core::hooks::HookDeleteRequest {
    tendi_core::hooks::HookDeleteRequest {
        agent: hook.agent,
        path: hook.path.clone(),
        expected_trust_hash: hook.trust_hash.clone(),
        event: hook.event.clone(),
        matcher: hook.matcher.clone(),
        hook_type: hook.hook_type.clone(),
        command: hook.command.clone(),
        url: hook.url.clone(),
        prompt: hook.prompt.clone(),
        filter: hook.filter.clone(),
        status_message: hook.status_message.clone(),
    }
}

fn hook_set_enabled_request_for_record(
    hook: &tendi_core::hooks::HookRecord,
    enabled: bool,
) -> tendi_core::hooks::HookSetEnabledRequest {
    tendi_core::hooks::HookSetEnabledRequest {
        agent: hook.agent,
        path: hook.path.clone(),
        expected_trust_hash: hook.trust_hash.clone(),
        event: hook.event.clone(),
        matcher: hook.matcher.clone(),
        hook_type: hook.hook_type.clone(),
        command: hook.command.clone(),
        url: hook.url.clone(),
        prompt: hook.prompt.clone(),
        filter: hook.filter.clone(),
        status_message: hook.status_message.clone(),
        enabled,
    }
}

fn hook_review_request_for_record(
    hook: &tendi_core::hooks::HookRecord,
) -> tendi_core::hooks::HookReviewRequest {
    tendi_core::hooks::HookReviewRequest {
        agent: hook.agent,
        path: hook.path.clone(),
        expected_trust_hash: hook.trust_hash.clone(),
        event: hook.event.clone(),
        matcher: hook.matcher.clone(),
        hook_type: hook.hook_type.clone(),
        command: hook.command.clone(),
        url: hook.url.clone(),
        prompt: hook.prompt.clone(),
        filter: hook.filter.clone(),
        status_message: hook.status_message.clone(),
    }
}

fn mcp_server_for_id<'a>(
    servers: &'a [tendi_core::mcp::McpServerRecord],
    id: &str,
) -> Result<&'a tendi_core::mcp::McpServerRecord, DaemonError> {
    servers
        .iter()
        .find(|server| tendi_core::mcp::mcp_server_matches_id(server, id))
        .ok_or_else(|| conflict_error("MCP server is not present in the current projection; refresh MCP before changing it"))
}

fn mcp_records_runtime_value(
    servers: &[tendi_core::mcp::McpServerRecord],
) -> Result<Value, DaemonError> {
    servers
        .iter()
        .map(|server| {
            let mut value = serde_json::to_value(server).map_err(internal_error)?;
            let object = value
                .as_object_mut()
                .ok_or_else(|| internal_error("MCP server record must serialize as an object"))?;
            object.insert(
                "id".to_string(),
                json!(tendi_core::mcp::mcp_server_id(server)),
            );
            Ok(value)
        })
        .collect::<Result<Vec<_>, DaemonError>>()
        .map(Value::Array)
}

fn mcp_set_enabled_request_for_record(
    server: &tendi_core::mcp::McpServerRecord,
    enabled: bool,
) -> tendi_core::mcp::McpSetEnabledRequest {
    tendi_core::mcp::McpSetEnabledRequest {
        agent: server.agent,
        path: server.path.clone(),
        expected_trust_hash: server.trust_hash.clone(),
        name: server.name.clone(),
        enabled,
        server_path: server.server_path.clone(),
    }
}

fn mcp_probe_request_for_record(
    server: &tendi_core::mcp::McpServerRecord,
) -> tendi_core::mcp::McpProbeRequest {
    tendi_core::mcp::McpProbeRequest {
        agent: server.agent,
        path: server.path.clone(),
        expected_trust_hash: server.trust_hash.clone(),
        name: server.name.clone(),
        server_path: server.server_path.clone(),
    }
}

fn update_mcp_projection_for_probe(
    scan: &mut tendi_core::mcp::McpScan,
    updated: &tendi_core::mcp::McpServerRecord,
) -> Result<(), DaemonError> {
    let Some(server) = scan.servers.iter_mut().find(|server| {
        server.agent == updated.agent
            && server.name == updated.name
            && server.path == updated.path
            && server.server_path == updated.server_path
    }) else {
        return Err(conflict_error(
            "MCP server disappeared from the current projection while checking its connection",
        ));
    };
    *server = updated.clone();
    Ok(())
}

fn update_mcp_projection_for_toggle(
    scan: &mut tendi_core::mcp::McpScan,
    request: &tendi_core::mcp::McpSetEnabledRequest,
    trust_hash: String,
) -> Result<(), DaemonError> {
    let mut matched = false;
    for server in &mut scan.servers {
        if server.path == request.path {
            server.trust_hash = trust_hash.clone();
            server.probe_state = tendi_core::mcp::McpProbeState::Unknown;
            server.server_name = None;
            server.server_title = None;
            server.server_version = None;
            server.server_description = None;
            server.server_website_url = None;
            server.icons.clear();
            server.tools.clear();
        }
        if server.agent == request.agent
            && server.name == request.name
            && server.path == request.path
            && server.server_path == request.server_path
        {
            server.enabled = request.enabled;
            server.status =
                tendi_core::mcp::mcp_status_after_toggle(request.agent, request.enabled)
                    .to_string();
            matched = true;
        }
    }
    if !matched {
        return Err(conflict_error(
            "MCP server disappeared from the current projection while changing it",
        ));
    }
    Ok(())
}

fn skill_visibility_from_request(
    value: runtime_schema::SkillVisibility,
) -> tendi_core::SkillVisibility {
    match value {
        runtime_schema::SkillVisibility::Auto => tendi_core::SkillVisibility::Auto,
        runtime_schema::SkillVisibility::Manual => tendi_core::SkillVisibility::Manual,
        runtime_schema::SkillVisibility::Off => tendi_core::SkillVisibility::Off,
        runtime_schema::SkillVisibility::Mixed => tendi_core::SkillVisibility::Mixed,
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
    let is_storage_error = tendi_core::storage::is_database_io_error_message(&message);
    let code = if message.contains("refusing to overwrite changed")
        || message.contains("preview expired")
        || message.contains("selection changed")
        || message.contains("resource acquisition cannot expand")
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
    if is_storage_error {
        DaemonError::with_data(code, message, json!({ "category": "storage" }))
    } else {
        DaemonError::new(code, message)
    }
}

pub fn run_http(
    daemon: Daemon,
    listener: TcpListener,
    token: Option<String>,
) -> std::io::Result<()> {
    listener.set_nonblocking(true)?;
    let mut connections = Vec::new();
    let result = loop {
        if daemon.is_shutting_down() {
            break Ok(());
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let daemon = daemon.clone();
                let token = token.clone();
                connections.push(thread::spawn(move || {
                    let _ = handle_connection(stream, &daemon, token.as_deref());
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => break Err(error),
        }
    };
    for connection in connections {
        let _ = connection.join();
    }
    result
}

fn handle_connection(
    mut stream: TcpStream,
    daemon: &Daemon,
    token: Option<&str>,
) -> std::io::Result<()> {
    // The listener is nonblocking so accept can poll for shutdown. Accepted
    // sockets must block while write_all drains large RPC responses.
    stream.set_nonblocking(false)?;
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
            &serde_json::to_string(&json!({
                "ok": true,
                "cwd": daemon.cwd(),
                "protocolVersion": runtime_schema::PROTOCOL_VERSION,
                "schemaVersion": runtime_schema::SCHEMA_VERSION,
                "contractFingerprint": runtime_schema::RUNTIME_CONTRACT_FINGERPRINT,
            }))
            .expect("health serializes"),
        );
    }
    let is_events = method == "GET" && path == "/v1/events";
    let is_log = method == "POST" && path == "/v1/log";
    if !is_events && !is_log && (method != "POST" || path != "/v1/rpc") {
        return write_http(
            &mut stream,
            404,
            &serde_json::to_string(&rpc_error_response(
                Value::Null,
                -32601,
                "METHOD_NOT_FOUND",
                "not found",
                None,
            ))
            .expect("response serializes"),
        );
    }
    if token
        .is_some_and(|expected| headers.get("authorization") != Some(&format!("Bearer {expected}")))
    {
        return write_http(
            &mut stream,
            401,
            &serde_json::to_string(&rpc_error_response(
                Value::Null,
                -32003,
                "UNAUTHORIZED",
                "invalid daemon token",
                None,
            ))
            .expect("response serializes"),
        );
    }
    if is_events {
        let last_event_id = headers
            .get("last-event-id")
            .and_then(|value| value.parse::<u64>().ok());
        return handle_event_stream(&mut stream, daemon, last_event_id);
    }
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if length > 64 * 1024 * 1024 {
        return write_http(
            &mut stream,
            413,
            &serde_json::to_string(&rpc_error_response(
                Value::Null,
                -32600,
                "REQUEST_TOO_LARGE",
                "request body is too large",
                None,
            ))
            .expect("response serializes"),
        );
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body)?;
    let request = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    if is_log {
        let level = request.get("level").and_then(Value::as_str).unwrap_or("");
        let message = request.get("message").and_then(Value::as_str).unwrap_or("");
        let fields = request.get("fields").cloned().unwrap_or_else(|| json!({}));
        let response = match tendi_core::logging::log_event(level, message, fields) {
            Ok(()) => json!({}),
            Err(error) => rpc_error_response(
                Value::Null,
                -32000,
                "LOG_WRITE_FAILED",
                &error.to_string(),
                None,
            ),
        };
        return write_http(
            &mut stream,
            200,
            &serde_json::to_string(&response).expect("log response serializes"),
        );
    }
    let response = daemon.handle_json_rpc(request);
    write_http(
        &mut stream,
        200,
        &serde_json::to_string(&response).expect("response serializes"),
    )
}

fn handle_event_stream(
    stream: &mut TcpStream,
    daemon: &Daemon,
    last_event_id: Option<u64>,
) -> std::io::Result<()> {
    stream.write_all(
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream; charset=utf-8\r\ncache-control: no-cache\r\naccess-control-allow-origin: *\r\naccess-control-allow-headers: content-type, authorization\r\nconnection: keep-alive\r\n\r\n",
    )?;
    stream.flush()?;
    let subscription = daemon.state.events.subscribe_from(last_event_id);
    loop {
        if daemon.is_shutting_down() {
            return Ok(());
        }
        match subscription.recv_timeout(Duration::from_secs(15)) {
            Ok(event) => {
                let payload = serde_json::to_string(&event).expect("event serializes");
                let event = format!(
                    "id: {}\nevent: {}\ndata: {}\n\n",
                    event.id, event.event, payload
                );
                stream.write_all(event.as_bytes())?;
                stream.flush()?;
            }
            Err(RecvTimeoutError::Timeout) => {
                stream.write_all(b": keep-alive\n\n")?;
                stream.flush()?;
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn write_http<W: Write>(stream: &mut W, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let headers = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json; charset=utf-8\r\ncontent-length: {}\r\ncache-control: no-store\r\naccess-control-allow-origin: *\r\naccess-control-allow-headers: content-type, authorization\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
fn cleanup_test_database(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut target = path.as_os_str().to_os_string();
        target.push(suffix);
        let _ = fs::remove_file(PathBuf::from(target));
    }
    let Some(parent) = path.parent() else {
        return;
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let lock_prefix = format!("{name}.resource-");
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            if file_name
                .to_str()
                .is_some_and(|value| value.starts_with(&lock_prefix))
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    struct ShortWriter {
        bytes: Vec<u8>,
        max_write: usize,
    }

    impl Write for ShortWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let written = bytes.len().min(self.max_write);
            self.bytes.extend_from_slice(&bytes[..written]);
            Ok(written)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

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

    fn test_daemon(cwd: PathBuf) -> Daemon {
        fs::create_dir_all(&cwd).unwrap();
        let database_path = cwd.with_extension("sqlite3");
        let mut daemon = Daemon::with_database(cwd, database_path.clone(), true);
        daemon.test_database_path = Some(database_path);
        daemon
    }

    fn test_daemon_without_background(cwd: PathBuf) -> Daemon {
        fs::create_dir_all(&cwd).unwrap();
        let database_path = cwd.with_extension("sqlite3");
        let mut daemon = Daemon::with_database(cwd, database_path.clone(), false);
        daemon.test_database_path = Some(database_path);
        daemon
    }

    fn test_store(daemon: &Daemon) -> tendi_core::storage::Store {
        tendi_core::storage::Store::open(&daemon.state.database_path).unwrap()
    }

    fn listed_skill_id(response: &Value, name: &str) -> String {
        response
            .as_array()
            .and_then(|skills| skills.iter().find(|skill| skill["name"] == name))
            .and_then(|skill| skill["id"].as_str())
            .unwrap_or_else(|| panic!("skill {name} was not present in listing: {response}"))
            .to_string()
    }

    #[test]
    fn skill_update_preview_is_not_recorded_as_a_runtime_operation() {
        assert!(!should_record_runtime_operation(
            "skills_update_many",
            &json!({ "dryRun": true })
        ));
        assert!(should_record_runtime_operation(
            "skills_update_many",
            &json!({ "dryRun": false })
        ));
        assert!(should_record_runtime_operation(
            "skills_delete_many",
            &json!({})
        ));
    }

    #[test]
    fn skill_watcher_ignores_tendi_atomic_write_temporary_paths() {
        assert!(is_skill_watcher_transient_path(Path::new(
            "/tmp/demo/.SKILL.md.tendi-tmp-123-1"
        )));
        assert!(is_skill_watcher_transient_path(Path::new(
            "/tmp/demo/agents/.openai.yaml.tendi-tmp-123-2"
        )));
        assert!(!is_skill_watcher_transient_path(Path::new(
            "/tmp/demo/SKILL.md"
        )));
    }

    #[test]
    fn skill_reconciliation_failure_backoff_is_scoped_and_event_resettable() {
        let root = temp_workspace();
        let other = root.join("other-workspace");
        fs::create_dir_all(&other).unwrap();
        let daemon = test_daemon_without_background(root.clone());
        let workspace = tendi_core::storage::canonical_workspace_root(&root);
        let other_workspace = tendi_core::storage::canonical_workspace_root(&other);

        daemon.record_skill_reconciliation_failure(&workspace, "invalid skill metadata");
        assert!(!daemon.skill_reconciliation_retry_ready(&workspace));
        assert!(daemon.skill_reconciliation_retry_ready(&other_workspace));
        let first_delay = daemon
            .state
            .skill_reconciliation_backoff
            .lock()
            .unwrap()
            .get(&workspace)
            .unwrap()
            .delay;

        daemon.record_skill_reconciliation_failure(&workspace, "invalid skill metadata");
        let second_delay = daemon
            .state
            .skill_reconciliation_backoff
            .lock()
            .unwrap()
            .get(&workspace)
            .unwrap()
            .delay;
        assert!(second_delay > first_delay);

        daemon.invalidate_skill_projection(&[]).unwrap();
        assert!(daemon.skill_reconciliation_retry_ready(&workspace));
        daemon.shutdown();
        drop(daemon);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn permanent_skill_parse_failure_waits_for_a_filesystem_event() {
        let root = temp_workspace();
        let daemon = test_daemon_without_background(root.clone());
        let workspace = tendi_core::storage::canonical_workspace_root(&root);

        daemon.record_skill_reconciliation_failure(
            &workspace,
            "failed to parse /tmp/SKILL.md: invalid frontmatter",
        );
        assert!(!daemon.skill_reconciliation_retry_ready(&workspace));

        daemon.invalidate_skill_projection(&[]).unwrap();
        assert!(daemon.skill_reconciliation_retry_ready(&workspace));
        daemon.shutdown();
        drop(daemon);
        fs::remove_dir_all(root).unwrap();
    }

    fn run_method(daemon: &Daemon, method: &str, params: Value) -> Result<Value, DaemonError> {
        daemon.execute_method(method, &params)
    }

    fn projection_domain_for_method(method: &str) -> Option<&'static str> {
        match method {
            "agents_list" => Some("agents"),
            "skills_list" => Some("skills"),
            "rules_list" => Some("rules"),
            "hooks_list" => Some("hooks"),
            "mcp_list" => Some("mcp"),
            _ => None,
        }
    }

    fn run_method_ok(daemon: &Daemon, method: &str, params: Value) -> Value {
        let subscription = projection_domain_for_method(method).map(|_| daemon.subscribe_events());
        let result = run_method(daemon, method, params.clone())
            .unwrap_or_else(|error| panic!("test command failed: {error:?}"));
        let Some(domain) = projection_domain_for_method(method) else {
            return result;
        };
        let store = test_store(&daemon);
        let status = store.projection_status(domain, daemon.cwd()).unwrap();
        if status == tendi_core::storage::ProjectionStatus::Fresh {
            return result;
        }
        let Some(subscription) = subscription else {
            return result;
        };
        for _ in 0..300 {
            let _ = subscription.recv_timeout(Duration::from_millis(100));
            if store.projection_status(domain, daemon.cwd()).unwrap()
                == tendi_core::storage::ProjectionStatus::Fresh
            {
                return run_method(daemon, method, params)
                    .unwrap_or_else(|error| panic!("test command failed: {error:?}"));
            }
        }
        result
    }

    #[test]
    fn prompt_save_rejects_empty_title_as_invalid_argument() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());
        let error = run_method(
            &daemon,
            "prompt_save",
            json!({
                "title": "  \n\t",
                "tags": [],
                "body": "Body"
            }),
        )
        .expect_err("empty title should fail");
        assert_eq!(error.code, "INVALID_ARGUMENT");
        assert_eq!(error.message, "missing or empty argument: title");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bundled_skill_install_uses_requested_agent_and_defaults_to_shared() {
        assert_eq!(
            bundled_skill_agent(Some(runtime_schema::AgentKind::Claude)),
            tendi_core::AgentKind::Claude
        );
        assert_eq!(
            bundled_skill_agent(Some(runtime_schema::AgentKind::Codex)),
            tendi_core::AgentKind::Codex
        );
        assert_eq!(bundled_skill_agent(None), tendi_core::AgentKind::Shared);
    }

    #[cfg(unix)]
    #[test]
    fn skills_set_materializes_a_read_only_skill_before_provider_writes() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let source = root.join("vendor/example");
        let target = root.join(".agents/skills/example");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: example\ndescription: Example\n---\n\n# Example\n",
        )
        .unwrap();
        symlink(&source, &target).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();
        let source_before = fs::read_to_string(source.join("SKILL.md")).unwrap();

        let daemon = test_daemon(root.clone());
        let listed = run_method_ok(&daemon, "skills_list", json!({}));
        let skill_id = listed_skill_id(&listed, "example");
        run_method(
            &daemon,
            "skills_set",
            json!({
                "skillIds": [skill_id],
                "visibility": "manual",
                "dryRun": false
            }),
        )
        .unwrap();

        assert!(fs::symlink_metadata(&target).unwrap().is_dir());
        assert_eq!(
            fs::read_to_string(source.join("SKILL.md")).unwrap(),
            source_before
        );
        assert_ne!(
            fs::read_to_string(target.join("SKILL.md")).unwrap(),
            source_before
        );
        assert!(target.join("agents/openai.yaml").is_file());

        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scheduled_backup_claim_consumes_dirty_and_releases_when_clean() {
        let dirty = AtomicBool::new(true);
        let running = AtomicBool::new(false);

        assert!(claim_scheduled_skill_backup(&dirty, &running));
        assert!(!dirty.load(Ordering::Acquire));
        assert!(running.load(Ordering::Acquire));

        running.store(false, Ordering::Release);
        assert!(!claim_scheduled_skill_backup(&dirty, &running));
        assert!(!running.load(Ordering::Acquire));
    }

    #[test]
    fn scheduled_backup_claim_keeps_dirty_when_another_backup_is_running() {
        let dirty = AtomicBool::new(true);
        let running = AtomicBool::new(true);

        assert!(!claim_scheduled_skill_backup(&dirty, &running));
        assert!(dirty.load(Ordering::Acquire));
        assert!(running.load(Ordering::Acquire));
    }

    #[test]
    fn sessions_scan_start_marks_current_scan_as_not_started() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());
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

        assert_eq!(result.generation, 7);
        assert!(!result.started);
        let _ = fs::remove_dir_all(root);
    }

    fn hold_database_write_lock(daemon: &Daemon) -> (mpsc::Sender<()>, thread::JoinHandle<()>) {
        let store = test_store(&daemon);
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = thread::spawn(move || {
            let conn = rusqlite::Connection::open(store.path()).unwrap();
            conn.busy_timeout(Duration::from_secs(5)).unwrap();
            conn.execute_batch("BEGIN IMMEDIATE").unwrap();
            acquired_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            conn.execute_batch("COMMIT").unwrap();
        });
        acquired_rx.recv().unwrap();
        (release_tx, holder)
    }

    #[test]
    fn skill_file_round_trip_and_conflict_are_protocol_errors() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());
        let listed = run_method_ok(&daemon, "skills_list", json!({}));
        let demo_id = listed_skill_id(&listed, "demo");
        assert!(listed[0]["paths"][0]["locationId"].as_str().is_some());
        let _files = run_method_ok(
            &daemon,
            "skill_files",
            json!({ "skillId": demo_id.clone() }),
        );
        let read = run_method_ok(
            &daemon,
            "skill_file_read",
            json!({ "skillId": demo_id.clone(), "relativePath": "SKILL.md" }),
        );
        let sha = read["sha256"].as_str().unwrap().to_string();
        let saved = run_method_ok(
            &daemon,
            "skill_file_save",
            json!({ "skillId": demo_id.clone(), "relativePath": "SKILL.md", "expectedSha256": sha, "content": "updated" }),
        );
        assert!(saved["content"].is_null());
        assert!(saved["skills"].is_array());
        assert_eq!(saved["sha256"].as_str().unwrap().len(), 64);
        let notes = run_method_ok(
            &daemon,
            "skill_file_create",
            json!({ "skillId": demo_id.clone(), "relativePath": "notes.md" }),
        );
        assert!(notes["files"].is_array());
        assert!(notes["content"].is_null());
        assert!(notes["skills"].is_null());
        let conflict = run_method(
            &daemon,
            "skill_file_save",
            json!({ "skillId": demo_id, "relativePath": "SKILL.md", "expectedSha256": "stale", "content": "bad" }),
        )
            .expect_err("stale skill file save should conflict");
        assert_eq!(conflict.code, "CONFLICT");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_file_reads_refresh_when_selected_skill_is_added_after_listing() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));

        let added_skill = root.join(".agents/skills/added");
        fs::create_dir_all(&added_skill).unwrap();
        fs::write(
            added_skill.join("SKILL.md"),
            "---\nname: added\ndescription: Added\n---\n\n# Added\n",
        )
        .unwrap();
        let added_id = format!(
            "skill@path:{}",
            added_skill.canonicalize().unwrap().display()
        );

        let files = run_method_ok(&daemon, "skill_files", json!({ "skillId": added_id }));
        assert!(
            files.as_array().is_some_and(|files| {
                files.iter().any(|file| file["relative_path"] == "SKILL.md")
            }),
            "unexpected files response: {files}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_file_changes_emit_a_runtime_event() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));
        let subscription = daemon.subscribe_events();
        let skill_file = root.join(".agents/skills/demo/SKILL.md");
        fs::write(
            &skill_file,
            "---\nname: demo\ndescription: Changed\n---\n\n# Changed\n",
        )
        .unwrap();

        let event = subscription
            .recv_timeout(Duration::from_secs(3))
            .expect("skill file change should emit a runtime event");
        assert_eq!(event.event, SKILL_CHANGED_EVENT);
        let skill_dir = skill_file.parent().unwrap();
        let canonical_skill_dir = skill_dir.canonicalize().unwrap();
        assert!(
            event.payload["paths"].as_array().is_some_and(|paths| {
                paths.iter().filter_map(|path| path.as_str()).any(|path| {
                    Path::new(path)
                        .canonicalize()
                        .is_ok_and(|path| path.starts_with(&canonical_skill_dir))
                })
            }),
            "unexpected skill change paths: {}",
            event.payload["paths"]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn direct_reads_do_not_wait_for_database_write_lock() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());
        let listed = run_method_ok(&daemon, "skills_list", json!({}));
        let demo_id = listed_skill_id(&listed, "demo");
        let requests = [
            ("settings_get", json!({})),
            ("skill_session_links", json!({ "skillId": demo_id.clone() })),
            ("skills_targets", json!({})),
            ("skill_files", json!({ "skillId": demo_id.clone() })),
            (
                "skill_file_read",
                json!({
                    "skillId": demo_id,
                    "relativePath": "SKILL.md",
                }),
            ),
        ];
        for (method, params) in &requests {
            let response = run_method(&daemon, method, params.clone());
            assert!(response.is_ok(), "warm-up response: {response:?}");
        }
        let (release_tx, holder) = hold_database_write_lock(&daemon);

        for (method, params) in requests {
            let request_daemon = daemon.clone();
            let (response_tx, response_rx) = mpsc::channel();
            let request_thread = thread::spawn(move || {
                response_tx
                    .send(run_method(&request_daemon, method, params))
                    .unwrap();
            });
            let response = response_rx.recv_timeout(Duration::from_secs(5));
            request_thread.join().unwrap();
            let response = response.unwrap_or_else(|error| {
                panic!("request {method} waited for the database authority: {error}")
            });
            assert!(response.is_ok(), "response: {response:?}");
        }

        release_tx.send(()).unwrap();
        holder.join().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn http_connections_join_when_daemon_shuts_down() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server_daemon = daemon.clone();
        let server = thread::spawn(move || run_http(server_daemon, listener, None));

        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.contains("\"ok\":true"));
        assert!(response.contains(&format!(
            "\"contractFingerprint\":\"{}\"",
            runtime_schema::RUNTIME_CONTRACT_FINGERPRINT
        )));

        daemon.shutdown();
        server.join().unwrap().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn accepted_http_connections_switch_back_to_blocking_mode() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server_daemon = daemon.clone();
        let server = thread::spawn(move || run_http(server_daemon, listener, None));

        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(b"GET /health HTTP/1.1\r\n").unwrap();
        thread::sleep(Duration::from_millis(50));
        stream
            .write_all(b"Host: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();

        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.contains("\"ok\":true"));

        daemon.shutdown();
        server.join().unwrap().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn http_response_writes_all_bytes_after_short_writes() {
        let body = "x".repeat(1024 * 1024);
        let mut writer = ShortWriter {
            bytes: Vec::new(),
            max_write: 3,
        };

        write_http(&mut writer, 200, &body).unwrap();

        let response = String::from_utf8(writer.bytes).unwrap();
        assert!(response.ends_with(&body));
        assert!(response.contains(&format!("content-length: {}", body.len())));
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
        let daemon = test_daemon(root.clone());
        let listed = run_method_ok(&daemon, "skills_list", json!({}));
        let demo_id = listed_skill_id(&listed, "demo");

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
        let preview = run_method_ok(
            &daemon,
            "skills_update_many",
            json!({ "skillIds": [demo_id.clone()], "dryRun": true }),
        );

        assert_eq!(preview["canApply"], false);
        assert!(preview["previewId"].is_null());
        let apply = run_method(
            &daemon,
            "skills_update_many",
            json!({ "skillIds": [demo_id], "dryRun": false }),
        )
        .expect_err("unrelated skill changes should conflict");
        assert_eq!(apply.code, "CONFLICT");
        let store = test_store(&daemon);
        for _ in 0..100 {
            if store.projection_status("skills", &root).unwrap()
                == tendi_core::storage::ProjectionStatus::Stale
            {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
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
        let daemon = test_daemon(root.clone());
        let listed = run_method_ok(&daemon, "skills_list", json!({}));
        let demo_id = listed_skill_id(&listed, "demo");

        fs::write(
            &selected_skill_file,
            "---\nname: demo\ndescription: Selected skill changed\n---\n\n# Demo\n",
        )
        .unwrap();
        let _preview = run_method_ok(
            &daemon,
            "skills_update_many",
            json!({ "skillIds": [demo_id], "dryRun": true }),
        );

        let store = test_store(&daemon);
        for _ in 0..100 {
            if store.projection_status("skills", &root).unwrap()
                == tendi_core::storage::ProjectionStatus::Stale
            {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
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
        let daemon = test_daemon(root.clone());
        let listed = run_method_ok(&daemon, "skills_list", json!({}));
        let demo_id = listed_skill_id(&listed, "demo");

        let response = run_method_ok(
            &daemon,
            "skills_delete_many",
            json!({ "skillIds": [demo_id.clone()] }),
        );
        assert!(!skill_dir.exists());
        assert_eq!(response["deleted"], json!([demo_id]));
        assert!(response["skills"].is_null());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_delete_many_refreshes_stale_projection_before_mutation() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));

        let added_skill = root.join(".agents/skills/added");
        fs::create_dir_all(&added_skill).unwrap();
        fs::write(
            added_skill.join("SKILL.md"),
            "---\nname: added\ndescription: Added\n---\n\n# Added\n",
        )
        .unwrap();
        let added_id = format!(
            "skill@path:{}",
            added_skill.canonicalize().unwrap().display()
        );

        let response = run_method_ok(
            &daemon,
            "skills_delete_many",
            json!({ "skillIds": [added_id.clone()] }),
        );

        assert!(!added_skill.exists());
        assert_eq!(response["deleted"], json!([added_id]));
        daemon.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rule_file_delete_many_refreshes_the_projection() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let rule_path = root.join("AGENTS.md");
        fs::write(&rule_path, "delete me").expect("write rule");
        let daemon = test_daemon(root.clone());

        let _listed = run_method_ok(&daemon, "rules_list", json!({}));
        let response = run_method_ok(
            &daemon,
            "rule_file_delete_many",
            json!({ "paths": [rule_path] }),
        );

        assert!(!rule_path.exists());
        let rule_path_text = rule_path.to_string_lossy();
        assert_eq!(response["deleted"], json!([rule_path_text.as_ref()]));
        let listed_after = run_method_ok(&daemon, "rules_list", json!({}));
        assert!(
            !listed_after
                .as_array()
                .expect("rules response should be an array")
                .iter()
                .any(|rule| rule["path"].as_str() == Some(rule_path_text.as_ref()))
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
        let daemon = test_daemon(root.clone());
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));
        let preview = run_method_ok(
            &daemon,
            "skills_distribute",
            json!({
                    "sourcePaths": [first_source, second_source],
                    "target": "claude-code",
                    "scope": "project",
                    "mode": "move",
                    "dryRun": true
            }),
        );
        assert_eq!(preview["plans"].as_array().unwrap().len(), 2);
        let preview_id = preview["previewId"].as_str().unwrap();

        let applied = run_method_ok(
            &daemon,
            "skills_distribute",
            json!({
                    "sourcePaths": [root.join(".agents/skills/demo"), second],
                    "target": "claude-code",
                    "scope": "project",
                    "mode": "move",
                    "previewId": preview_id,
                    "dryRun": false
            }),
        );
        assert_eq!(applied["results"].as_array().unwrap().len(), 2);
        assert!(!root.join(".agents/skills/demo").exists());
        assert!(!root.join(".agents/skills/second").exists());
        assert!(root.join(".claude/skills/demo/SKILL.md").is_file());
        assert!(root.join(".claude/skills/second/SKILL.md").is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_distribution_moves_once_and_links_additional_targets() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let source = root.join(".agents/skills/demo");
        let codex = root.join(".codex/skills/demo");
        let cursor = root.join(".cursor/skills/demo");
        let daemon = test_daemon(root.clone());
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));

        let applied = run_method_ok(
            &daemon,
            "skills_distribute",
            json!({
                "sourcePaths": [source.clone()],
                "targets": ["codex", "cursor"],
                "scope": "project",
                "mode": "move",
                "dryRun": false
            }),
        );

        assert_eq!(applied["results"].as_array().unwrap().len(), 2);
        assert!(!source.exists());
        assert!(codex.join("SKILL.md").is_file());
        assert!(
            !fs::symlink_metadata(&codex)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(cursor.join("SKILL.md").is_file());
        assert!(
            fs::symlink_metadata(&cursor)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            cursor.canonicalize().unwrap(),
            codex.canonicalize().unwrap()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_distribution_applies_without_preview() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let source = root.join(".agents/skills/demo");
        let daemon = test_daemon(root.clone());
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));

        let applied = run_method_ok(
            &daemon,
            "skills_distribute",
            json!({
                    "sourcePaths": [source.clone()],
                    "target": "claude-code",
                    "scope": "project",
                    "mode": "move",
                    "dryRun": false
            }),
        );

        assert_eq!(applied["plans"][0]["mode"], "move");
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
        let daemon = test_daemon(root.clone());
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));

        let distributed = run_method_ok(
            &daemon,
            "skills_distribute",
            json!({
                    "sourcePaths": [source.clone()],
                    "target": "claude-code",
                    "scope": "project",
                    "mode": "copy",
                    "dryRun": false
            }),
        );
        assert!(source.exists());
        assert!(target.exists());
        assert!(
            distributed["updated"]
                .as_array()
                .unwrap()
                .iter()
                .any(|skill| skill["name"] == "demo"),
            "distribution response did not include demo: {distributed}"
        );
        let target_path = target.to_string_lossy();
        let target_id = distributed["updated"]
            .as_array()
            .and_then(|skills| {
                skills.iter().find(|skill| {
                    skill["paths"].as_array().is_some_and(|paths| {
                        paths
                            .iter()
                            .any(|path| path["path"].as_str() == Some(target_path.as_ref()))
                    })
                })
            })
            .and_then(|skill| skill["id"].as_str())
            .expect("distribution should expose the target installation id")
            .to_string();
        let removed = run_method_ok(
            &daemon,
            "skills_remove_locations",
            json!({
                    "skillIds": [target_id],
                    "targets": ["claude-code"],
                    "scope": "project"
            }),
        );
        assert!(source.exists());
        assert!(!target.exists(), "remove response: {removed}");
        assert_eq!(removed["plan"]["targets"].as_array().unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_remove_locations_rehomes_canonical_installation() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let source = root.join(".agents/skills/demo");
        let codex = root.join(".codex/skills/demo");
        let cursor = root.join(".cursor/skills/demo");
        let daemon = test_daemon(root.clone());
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));

        run_method_ok(
            &daemon,
            "skills_distribute",
            json!({
                "sourcePaths": [source.clone()],
                "targets": ["codex", "cursor"],
                "scope": "project",
                "mode": "symlink",
                "dryRun": false
            }),
        );
        assert!(source.join("SKILL.md").is_file());
        assert!(
            fs::symlink_metadata(&codex)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::symlink_metadata(&cursor)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        let listed = run_method_ok(&daemon, "skills_list", json!({}));
        let skill_id = listed_skill_id(&listed, "demo");
        let removed = run_method_ok(
            &daemon,
            "skills_remove_locations",
            json!({
                "skillIds": [skill_id],
                "targets": ["shared"],
                "scope": "project"
            }),
        );

        assert!(!source.exists(), "remove response: {removed}");
        assert!(codex.join("SKILL.md").is_file());
        assert!(
            !fs::symlink_metadata(&codex)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(cursor.join("SKILL.md").is_file());
        assert!(
            fs::symlink_metadata(&cursor)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            cursor.canonicalize().unwrap(),
            codex.canonicalize().unwrap()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn wrapper_syncs_child_content_location_and_deletion() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let child_dir = root.join(".agents/skills/child");
        let wrapper_dir = root.join(".agents/skills/wrapper");
        fs::create_dir_all(&child_dir).unwrap();
        fs::create_dir_all(&wrapper_dir).unwrap();
        fs::write(
            child_dir.join("SKILL.md"),
            "---\nname: child\ndescription: Original child\n---\n\n# Child\n",
        )
        .unwrap();
        fs::write(
            wrapper_dir.join("SKILL.md"),
            format!(
                "---\nname: wrapper\ndescription: Wrapper\n---\n\n# Wrapper\n\n## Route\n\n- [`child`](<{}>): Original child\n",
                child_dir.join("SKILL.md").display()
            ),
        )
        .unwrap();

        let daemon = test_daemon(root.clone());
        let listed = run_method_ok(&daemon, "skills_list", json!({}));
        let wrapper = listed
            .as_array()
            .and_then(|skills| skills.iter().find(|skill| skill["name"] == "wrapper"))
            .expect("wrapper should be listed");
        assert_eq!(wrapper["is_wrapper"], json!(true));
        let child_id = listed_skill_id(&listed, "child");
        let read = run_method_ok(
            &daemon,
            "skill_file_read",
            json!({ "skillId": child_id.clone(), "relativePath": "SKILL.md" }),
        );
        run_method_ok(
            &daemon,
            "skill_file_save",
            json!({
                "skillId": child_id,
                "relativePath": "SKILL.md",
                "expectedSha256": read["sha256"],
                "content": "---\nname: child\ndescription: Updated child\n---\n\n# Child\n"
            }),
        );
        let wrapper_file = wrapper_dir.join("SKILL.md");
        let wrapper_after_save = fs::read_to_string(&wrapper_file).unwrap();
        assert!(wrapper_after_save.contains("Updated child"));

        let distributed = run_method_ok(
            &daemon,
            "skills_distribute",
            json!({
                "sourcePaths": [child_dir.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "dryRun": false
            }),
        );
        let target_dir = root.join(".claude/skills/child");
        assert!(!child_dir.exists());
        assert!(target_dir.join("SKILL.md").is_file());
        let target_path = target_dir.to_string_lossy();
        let target_id = distributed["updated"]
            .as_array()
            .and_then(|skills| {
                skills.iter().find(|skill| {
                    skill["paths"].as_array().is_some_and(|paths| {
                        paths
                            .iter()
                            .any(|path| path["path"].as_str() == Some(target_path.as_ref()))
                    })
                })
            })
            .and_then(|skill| skill["id"].as_str())
            .expect("distribution should expose the moved child installation id")
            .to_string();
        let wrapper_after_move = fs::read_to_string(&wrapper_file).unwrap();
        assert!(
            wrapper_after_move.contains(&target_dir.join("SKILL.md").display().to_string()),
            "wrapper after move: {wrapper_after_move}"
        );
        assert!(!wrapper_after_move.contains(&child_dir.join("SKILL.md").display().to_string()));
        assert!(wrapper_after_move.contains("Updated child"));

        run_method_ok(
            &daemon,
            "skills_remove_locations",
            json!({
                "skillIds": [target_id],
                "targets": ["claude-code"],
                "scope": "project"
            }),
        );
        assert!(!target_dir.exists());
        let wrapper_after_delete = fs::read_to_string(wrapper_file).unwrap();
        assert!(!wrapper_after_delete.contains("[`child`]"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_distribution_allows_mode_change_after_preview() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let source = root.join(".agents/skills/demo");
        let daemon = test_daemon(root.clone());
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));

        let preview = run_method_ok(
            &daemon,
            "skills_distribute",
            json!({
                    "sourcePaths": [source.clone()],
                    "target": "claude-code",
                    "scope": "project",
                    "mode": "symlink",
                    "dryRun": true
            }),
        );
        let preview_id = preview["previewId"].as_str().unwrap();

        let applied = run_method_ok(
            &daemon,
            "skills_distribute",
            json!({
                    "sourcePaths": [source.clone()],
                    "target": "claude-code",
                    "scope": "project",
                    "mode": "move",
                    "previewId": preview_id,
                    "dryRun": false
            }),
        );
        assert_eq!(applied["plans"][0]["mode"], "move");
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
        let daemon = test_daemon(root.clone());
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));

        let preview = run_method_ok(
            &daemon,
            "skills_distribute",
            json!({
                    "sourcePaths": [same_path.clone(), moved_source.clone()],
                    "target": "claude-code",
                    "scope": "project",
                    "mode": "move",
                    "dryRun": true
            }),
        );
        let plans = preview["plans"].as_array().unwrap();
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0]["status"], "already-at-destination");
        assert_eq!(plans[1]["status"], "ready");
        let preview_id = preview["previewId"].as_str().unwrap();

        let _applied = run_method_ok(
            &daemon,
            "skills_distribute",
            json!({
                    "sourcePaths": [same_path.clone(), moved_source.clone()],
                    "target": "claude-code",
                    "scope": "project",
                    "mode": "move",
                    "previewId": preview_id,
                    "dryRun": false
            }),
        );
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
        let daemon = test_daemon(root.clone());
        let listed = run_method_ok(&daemon, "skills_list", json!({}));
        let demo_id = listed_skill_id(&listed, "demo");

        let _visibility = run_method_ok(
            &daemon,
            "skills_set",
            json!({ "skillIds": [demo_id], "visibility": "manual" }),
        );
        let store = test_store(&daemon);
        let visibility = store
            .skill_visibilities_for_workspace(&root)
            .unwrap()
            .get(&skill_dir.canonicalize().unwrap())
            .copied();
        assert_eq!(visibility, Some(tendi_core::SkillVisibility::Manual));
        assert!(
            fs::read_to_string(skill_dir.join("SKILL.md"))
                .unwrap()
                .contains("disable-model-invocation: true")
        );
        assert!(
            fs::read_to_string(skill_dir.join("agents/openai.yaml"))
                .unwrap()
                .contains("allow_implicit_invocation: false")
        );

        let _folder = run_method_ok(
            &daemon,
            "skill_folder_create",
            json!({ "skillId": demo_id.clone(), "relativePath": "references" }),
        );
        let _file = run_method_ok(
            &daemon,
            "skill_file_create",
            json!({ "skillId": demo_id.clone(), "relativePath": "references/notes.md" }),
        );
        let _renamed = run_method_ok(
            &daemon,
            "skill_path_rename",
            json!({
                    "skillId": demo_id.clone(),
                    "fromRelativePath": "references/notes.md",
                    "toRelativePath": "references/renamed.md"
            }),
        );
        let _deleted = run_method_ok(
            &daemon,
            "skill_path_delete",
            json!({ "skillId": demo_id, "relativePath": "references/renamed.md" }),
        );
        assert!(!skill_dir.join("references/renamed.md").exists());

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
                    probe_cache_version: tendi_core::mcp::MCP_PROBE_CACHE_VERSION,
                    probe_state: tendi_core::mcp::McpProbeState::Unknown,
                    server_path: Vec::new(),
                    read_only_reason: None,
                    server_name: None,
                    server_title: None,
                    server_version: None,
                    server_description: None,
                    server_website_url: None,
                    probe_error: None,
                    icons: Vec::new(),
                    tools: Vec::new(),
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
                    probe_cache_version: tendi_core::mcp::MCP_PROBE_CACHE_VERSION,
                    probe_state: tendi_core::mcp::McpProbeState::Unknown,
                    server_path: Vec::new(),
                    read_only_reason: None,
                    server_name: None,
                    server_title: None,
                    server_version: None,
                    server_description: None,
                    server_website_url: None,
                    probe_error: None,
                    icons: Vec::new(),
                    tools: Vec::new(),
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
            server_path: Vec::new(),
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
        let daemon = test_daemon(temp_workspace());
        let error = run_method(&daemon, "not_implemented", json!({}))
            .expect_err("unknown command should fail");
        assert_eq!(error.code, "METHOD_NOT_FOUND");
    }

    #[test]
    fn event_subscription_preserves_shared_envelope() {
        let hub = EventHub {
            next_id: Arc::new(AtomicU64::new(0)),
            state: Arc::new(Mutex::new(EventHubState::default())),
        };
        let subscription = hub.subscribe();
        hub.publish_with_metadata(
            "analytics://revision",
            json!({ "scopeKey": "test", "revision": 42 }),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let event = subscription
            .recv_timeout(Duration::from_secs(1))
            .expect("event should be delivered");
        assert_eq!(event.id, 1);
        assert_eq!(event.event, "analytics://revision");
        assert_eq!(event.payload["revision"], 42);
    }

    #[test]
    fn event_subscription_replays_events_after_last_event_id() {
        let hub = EventHub {
            next_id: Arc::new(AtomicU64::new(0)),
            state: Arc::new(Mutex::new(EventHubState::default())),
        };
        hub.publish_with_metadata(
            "analytics://revision",
            json!({ "scopeKey": "test", "revision": 1 }),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        hub.publish_with_metadata(
            "analytics://revision",
            json!({ "scopeKey": "test", "revision": 2 }),
            None,
            None,
            None,
            None,
            None,
            None,
        );

        let subscription = hub.subscribe_from(Some(1));
        let event = subscription
            .recv_timeout(Duration::from_secs(1))
            .expect("the missed event should be replayed");
        assert_eq!(event.id, 2);
        assert_eq!(event.payload["revision"], 2);
        assert!(matches!(
            subscription.recv_timeout(Duration::from_millis(20)),
            Err(RecvTimeoutError::Timeout)
        ));
    }

    #[test]
    fn projection_refresh_event_carries_domain_metadata() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());
        let subscription = daemon.subscribe_events();
        daemon.emit_event(
            PROJECTION_CHANGED_EVENT,
            runtime_event(
                PROJECTION_CHANGED_EVENT,
                json!({ "domain": "rules", "error": Value::Null }),
            ),
        );

        let event = subscription
            .recv_timeout(Duration::from_secs(1))
            .expect("projection refresh event should be delivered");
        assert_eq!(event.event, PROJECTION_CHANGED_EVENT);
        assert_eq!(event.domain.as_deref(), Some("rules"));
        assert_eq!(event.payload["domain"], "rules");
        daemon.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn session_watch_retry_state_keeps_dirty_paths_until_success() {
        let (watch_tx, _watch_rx) = mpsc::channel();
        let (analytics_tx, _analytics_rx) = mpsc::channel();
        let runtime = SessionRuntime {
            generation: AtomicU64::new(0),
            scan_running: AtomicBool::new(false),
            watch_revision: AtomicU64::new(0),
            completed_revision: AtomicU64::new(0),
            watcher: Mutex::new(SessionWatcherState::default()),
            retry: Mutex::new(SessionWatchRetryState::default()),
            watch_tx,
            analytics_tx,
        };
        let path = PathBuf::from("/tmp/tendi-session-watch-retry.jsonl");

        schedule_session_watch_retry(&runtime, std::slice::from_ref(&path));
        {
            let retry = runtime.retry.lock().unwrap();
            assert!(retry.paths.contains(&path));
            assert_eq!(retry.delay, SESSION_WATCH_RETRY_INITIAL * 2);
        }

        runtime.retry.lock().unwrap().retry_at = Some(Instant::now());
        assert_eq!(
            take_due_session_watch_retries(&runtime),
            Some(vec![path.clone()])
        );
        complete_session_watch_paths(&runtime, std::slice::from_ref(&path));

        let retry = runtime.retry.lock().unwrap();
        assert!(retry.paths.is_empty());
        assert!(retry.retry_at.is_none());
        assert_eq!(retry.delay, SESSION_WATCH_RETRY_INITIAL);
    }

    #[test]
    fn live_session_watch_preview_includes_assistant_reply() {
        let root = temp_workspace();
        let path = root.join("rollout-12345678-1234-1234-1234-123456789012.jsonl");
        fs::write(
            &path,
            [
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Question"}]}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Answer"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let sessions = live_session_watch_previews(
            std::slice::from_ref(&path),
            &tendi_core::sessions::SessionScanCache::default(),
        );

        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].last_assistant_message.as_deref(),
            Some("Answer")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn analytics_refresh_completes_while_session_lane_is_blocked() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());
        let scope_key = daemon_scope_key(&daemon).unwrap();
        let warmed = run_method_ok(&daemon, "skills_backup_status", json!({}));
        assert!(warmed.is_object());
        let subscription = daemon.subscribe_events();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        daemon
            .state
            .session_operations
            .submit(
                tendi_core::OperationId::new("test-session-blocker")
                    .expect("test operation id is valid"),
                move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                },
            )
            .unwrap();
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("session lane blocker should start");

        let analytics_daemon = daemon.clone();
        let analytics_scope = scope_key.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let analytics = thread::spawn(move || {
            let result = refresh_session_analytics_serialized(
                &analytics_daemon,
                "test",
                &analytics_scope,
                &[],
            );
            done_tx.send(result).unwrap();
        });
        let progress = subscription
            .recv_timeout(Duration::from_secs(1))
            .expect("analytics refresh should start before waiting on its lane");
        assert_eq!(progress.event, ANALYTICS_PROGRESS_EVENT);

        let completed_before_session_release = done_rx.recv_timeout(Duration::from_secs(10));
        release_tx.send(()).unwrap();
        analytics.join().unwrap();
        let report = completed_before_session_release
            .expect("analytics should complete while session lane is blocked")
            .expect("analytics refresh should succeed");
        assert_eq!(report.total, 0);
        daemon.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn foreground_rpc_runs_while_analytics_lane_is_blocked() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());
        let scope_key = daemon_scope_key(&daemon).unwrap();
        let subscription = daemon.subscribe_events();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        daemon
            .state
            .analytics_operations
            .submit(
                tendi_core::OperationId::new("test-analytics-blocker")
                    .expect("test operation id is valid"),
                move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                },
            )
            .unwrap();
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("analytics lane blocker should start");

        let analytics_daemon = daemon.clone();
        let analytics_scope = scope_key.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let analytics = thread::spawn(move || {
            let result = refresh_session_analytics_serialized(
                &analytics_daemon,
                "test",
                &analytics_scope,
                &[],
            );
            done_tx.send(result).unwrap();
        });
        let progress = subscription
            .recv_timeout(Duration::from_secs(1))
            .expect("analytics refresh should start before waiting on its lane");
        assert_eq!(progress.event, ANALYTICS_PROGRESS_EVENT);

        let foreground = run_method_ok(&daemon, "skills_backup_status", json!({}));
        assert!(foreground.is_object());

        release_tx.send(()).unwrap();
        let report = done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("analytics refresh should finish after release")
            .expect("analytics refresh should complete");
        analytics.join().unwrap();
        assert_eq!(report.total, 0);

        daemon.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn foreground_rpc_runs_while_maintenance_lane_is_blocked() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());
        let warmed = run_method_ok(&daemon, "skills_backup_status", json!({}));
        assert!(warmed.is_object());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        daemon
            .state
            .requests
            .submit(
                tendi_core::OperationId::new("test-maintenance-blocker")
                    .expect("test operation id is valid"),
                request_scheduler::Step::acquire(
                    request_scheduler::Workload::ExternalIo,
                    Vec::new(),
                    move || {
                        started_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        Ok(request_scheduler::Step::Complete(()))
                    },
                ),
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("maintenance lane blocker should start");

        let foreground = run_method_ok(&daemon, "skills_backup_status", json!({}));
        assert!(foreground.is_object());

        release_tx.send(()).unwrap();
        daemon.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn projection_read_does_not_block_foreground_rpc() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());
        let warmed = run_method_ok(&daemon, "skills_backup_status", json!({}));
        assert!(warmed.is_object());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        daemon
            .state
            .requests
            .submit(
                tendi_core::OperationId::new("test-projection-blocker")
                    .expect("test operation id is valid"),
                request_scheduler::Step::acquire(
                    request_scheduler::Workload::Compute,
                    Vec::new(),
                    move || {
                        started_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        Ok(request_scheduler::Step::Complete(()))
                    },
                ),
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("projection lane blocker should start");

        let read_daemon = daemon.clone();
        let (read_tx, read_rx) = mpsc::channel();
        let projection_read = thread::spawn(move || {
            read_tx
                .send(
                    run_method(&read_daemon, "agents_list", json!({}))
                        .unwrap_or_else(|error| panic!("test command failed: {error:?}")),
                )
                .unwrap();
        });

        let projection_result = read_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("cached projection read should not wait for its refresh lane");
        assert!(projection_result.is_array());

        let foreground = run_method_ok(&daemon, "skills_backup_status", json!({}));
        assert!(foreground.is_object());

        release_tx.send(()).unwrap();
        projection_read.join().unwrap();
        daemon.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn store_write_waits_for_another_sqlite_transaction() {
        let root = temp_workspace();
        let db = root.join("tendi.sqlite3");
        let store_a = tendi_core::storage::Store::open(&db).unwrap();
        let store_b = tendi_core::storage::Store::open(&db).unwrap();
        let settings = store_b.app_settings().unwrap();
        let expected_appearance = settings.appearance.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = thread::spawn(move || {
            let conn = rusqlite::Connection::open(store_a.path()).unwrap();
            conn.execute_batch("BEGIN IMMEDIATE").unwrap();
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            conn.execute_batch("COMMIT").unwrap();
        });
        started_rx.recv().unwrap();

        let contender = thread::spawn(move || store_b.save_app_settings(settings).unwrap());
        thread::sleep(Duration::from_millis(100));
        assert!(
            !contender.is_finished(),
            "write must wait for the SQLite owner"
        );
        release_tx.send(()).unwrap();

        assert_eq!(contender.join().unwrap().appearance, expected_appearance);
        holder.join().unwrap();
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
        let daemon = test_daemon_without_background(root.clone());
        let store = test_store(&daemon);
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = thread::spawn(move || {
            let conn = rusqlite::Connection::open(store.path()).unwrap();
            conn.busy_timeout(Duration::from_secs(5)).unwrap();
            conn.execute_batch("BEGIN IMMEDIATE").unwrap();
            acquired_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            conn.execute_batch("COMMIT").unwrap();
        });
        acquired_rx.recv().unwrap();

        let request_daemon = daemon.clone();
        let path = transcript.display().to_string();
        let (response_tx, response_rx) = mpsc::channel();
        let request = thread::spawn(move || {
            let response = run_method(
                &request_daemon,
                "session_transcript",
                json!({ "path": path, "agent": "codex", "limit": 1 }),
            );
            response_tx.send(response).unwrap();
        });
        let response = response_rx.recv_timeout(Duration::from_millis(300));
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        request.join().unwrap();

        let response = response
            .expect("transcript read should not wait for database write lock")
            .expect("transcript read should succeed");
        assert_eq!(response["items"].as_array().unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn json_rpc_boundary_uses_generated_envelope_and_numeric_errors() {
        let root = temp_workspace();
        let daemon = test_daemon(root.clone());

        let response = daemon.handle_json_rpc(json!({
            "jsonrpc": "2.0",
            "id": "test-1",
            "method": "unknown_method",
            "params": {}
        }));
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], "test-1");
        assert_eq!(response["error"]["code"], -32601);
        assert_eq!(response["error"]["data"]["kind"], "METHOD_NOT_FOUND");
        assert!(response.get("ok").is_none());

        let response = daemon.handle_json_rpc(json!({
            "jsonrpc": "2.0",
            "id": "test-2",
            "method": "sessions_snapshot",
            "params": { "unexpected": true }
        }));
        assert_eq!(response["error"]["code"], -32602);
        assert_eq!(response["error"]["data"]["kind"], "INVALID_PARAMS");

        let response = daemon.handle_json_rpc(json!({
            "jsonrpc": "2.0",
            "id": "test-3",
            "method": "sessions_snapshot"
        }));
        assert_eq!(response["error"]["code"], -32600);
        assert_eq!(response["error"]["data"]["kind"], "INVALID_REQUEST");

        let response = daemon.handle_json_rpc(json!({
            "jsonrpc": "2.0",
            "id": ["invalid"],
            "method": "sessions_snapshot",
            "params": {}
        }));
        assert!(response["id"].is_null());
        assert_eq!(response["error"]["code"], -32600);
        daemon.shutdown();
        let _ = fs::remove_dir_all(root);
    }
}
