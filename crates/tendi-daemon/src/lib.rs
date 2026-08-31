use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use serde_json::{Value, json};

mod operation_coordinator;
use operation_coordinator::OperationCoordinator;
use tendi_core::generated::runtime_contract as runtime_schema;

include!("generated/runtime_dispatch.rs");

pub const SESSION_SCAN_EVENT: &str = runtime_schema::EventName::SessionsScan.as_str();
pub const ANALYTICS_PROGRESS_EVENT: &str = runtime_schema::EventName::AnalyticsProgress.as_str();
pub const ANALYTICS_REVISION_EVENT: &str = runtime_schema::EventName::AnalyticsRevision.as_str();
pub const SKILL_UPDATE_EVENT: &str = runtime_schema::EventName::SkillsUpdates.as_str();
pub const CONFIG_CHANGED_EVENT: &str = runtime_schema::EventName::ConfigChanged.as_str();
const SESSION_SCAN_BATCH_SIZE: usize = 32;
const SESSION_WATCH_DEBOUNCE: Duration = Duration::from_millis(500);
const CONFIG_WATCH_DEBOUNCE: Duration = Duration::from_millis(150);
const BACKUP_SYNC_INTERVAL: Duration = Duration::from_secs(10 * 60);
const DATABASE_WRITE_LOCK_ATTEMPTS: usize = 100;
const DATABASE_WRITE_LOCK_RETRY: Duration = Duration::from_millis(50);
const SESSION_WATCH_DATABASE_RETRY_ATTEMPTS: usize = 4;
const SESSION_WATCH_DATABASE_RETRY: Duration = Duration::from_millis(100);

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
            serde_json::from_value(payload).expect("analytics progress event matches generated schema"),
        ),
        ANALYTICS_REVISION_EVENT => runtime_schema::RuntimeEventPayload::AnalyticsRevision(
            serde_json::from_value(payload).expect("analytics revision event matches generated schema"),
        ),
        SKILL_UPDATE_EVENT => runtime_schema::RuntimeEventPayload::SkillsUpdates(
            serde_json::from_value(payload).expect("skills update event matches generated schema"),
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
            tendi_core::logging::global().error("runtime event contract failed", json!({ "error": error }));
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.history.push_back(event.clone());
        while state.history.len() > 256 {
            state.history.pop_front();
        }
        state.subscribers.retain(|subscriber| subscriber.send(event.clone()).is_ok());
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

#[derive(Debug)]
struct DaemonState {
    cwd: PathBuf,
    events: EventHub,
    operations: OperationCoordinator,
    session_runtime: Arc<SessionRuntime>,
    scoped_search_rebuilt: AtomicBool,
    config_runtime: Arc<ConfigRuntime>,
    skill_authority: Mutex<()>,
    control_authority: Mutex<()>,
    session_skill_index_running: AtomicBool,
    skill_update_running: AtomicBool,
    skill_update_cancelled: AtomicBool,
    backup_sync_dirty: AtomicBool,
    backup_sync_running: AtomicBool,
    add_preview: Mutex<Option<SkillAddPreview>>,
    update_preview: Mutex<Option<SkillUpdatePreview>>,
    distribution_preview: Mutex<Option<SkillDistributionPreview>>,
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
}

impl Clone for Daemon {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            lifecycle: Arc::clone(&self.lifecycle),
            owner: false,
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if self.owner {
            self.shutdown();
        }
    }
}

impl Daemon {
    pub fn new(cwd: PathBuf) -> Self {
        let (watch_tx, watch_rx) = mpsc::channel();
        let (config_watch_tx, config_watch_rx) = mpsc::channel();
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
            watch_tx,
            analytics_tx,
        });
        let config_runtime = Arc::new(ConfigRuntime {
            watcher: Mutex::new(ConfigWatcherState::default()),
            watch_tx: config_watch_tx,
        });
        let lifecycle = Arc::new(DaemonLifecycle::new());
        let daemon = Self {
            state: Arc::new(DaemonState {
                cwd,
                events,
                operations: OperationCoordinator::new(),
                session_runtime,
                scoped_search_rebuilt: AtomicBool::new(false),
                config_runtime,
                skill_authority: Mutex::new(()),
                control_authority: Mutex::new(()),
                session_skill_index_running: AtomicBool::new(false),
                skill_update_running: AtomicBool::new(false),
                skill_update_cancelled: AtomicBool::new(false),
                backup_sync_dirty: AtomicBool::new(true),
                backup_sync_running: AtomicBool::new(false),
                add_preview: Mutex::new(None),
                update_preview: Mutex::new(None),
                distribution_preview: Mutex::new(None),
                preview_sequence: Mutex::new(0),
            }),
            lifecycle: Arc::clone(&lifecycle),
            owner: true,
        };
        let recovery_id = tendi_core::OperationId::new("daemon-startup-recovery")
            .expect("startup recovery operation id is valid");
        if let Ok(Ok(recovered)) = daemon.state.operations.execute(recovery_id, || {
            let store = tendi_core::storage::Store::open_default()?;
            with_database_write_lock(&store, || store.recover_inflight_operations())
                .map_err(daemon_error_anyhow)
        }) {
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
        let analytics_worker = thread::spawn(move || session_analytics_loop(analytics_daemon, analytics_rx));
        let config_daemon = daemon.clone();
        let config_worker = thread::spawn(move || config_watch_loop(config_daemon, config_watch_rx));
        let backup_daemon = daemon.clone();
        let backup_worker = thread::spawn(move || backup_sync_loop(backup_daemon));
        lifecycle
            .workers
            .lock()
            .expect("daemon worker registry is healthy")
            .extend([watch_worker, analytics_worker, config_worker, backup_worker]);
        daemon.initialize_config_watcher();
        daemon
    }

    pub fn shutdown(&self) {
        self.lifecycle.shutdown_and_join();
        self.state.operations.shutdown();
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
            let domain = event_projection_domain(event);
            let revision = domain
                .as_deref()
                .and_then(|domain| {
                    let store = tendi_core::storage::Store::open_default().ok()?;
                    store
                        .projection_head(scope_key.as_ref()?, domain)
                        .ok()?
                        .map(|head| head.revision.value())
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
        if !command_requires_serialized_write(method, params) {
            return self.dispatch(method, params);
        }

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
        let journal_operation_id = operation_id.clone();
        let result = match self.state.operations.execute(operation_id, move || {
            let journal_scope = daemon_scope_key(&daemon).ok();
            let journal_store = journal_scope.as_ref().and_then(|scope| {
                let store = tendi_core::storage::Store::open_default().ok()?;
                let input_revision = store
                    .projection_head(scope, "sessions")
                    .ok()
                    .flatten()
                    .map(|head| head.revision)
                    .unwrap_or(tendi_core::Revision::ZERO);
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
                with_database_write_lock(&store, || store.record_operation(&record))
                    .ok()?;
                Some(store)
            });
            let result = daemon.dispatch(&method, &params);
            if let Some(store) = journal_store {
                let (status, error) = match &result {
                    Ok(_) => (tendi_core::OperationStatus::Committed, None),
                    Err(error) => (
                        tendi_core::OperationStatus::Failed,
                        Some(error.message.as_str()),
                    ),
                };
                let _ = with_database_write_lock(&store, || {
                    store.update_operation(&journal_operation_id, status, None, error)
                });
            }
            Ok(result)
        }) {
            Ok(result) => result,
            Err(error) => {
                return Err(internal_error(format!(
                    "serialized operation could not be queued: {error:?}"
                )));
            }
        };

        match result {
            Ok(result) => result,
            Err(error) => Err(internal_error(format!(
                "serialized operation failed: {error:#}"
            ))),
        }
    }

    /// JSON-RPC 2.0 is the only daemon wire envelope.
    pub fn handle_json_rpc(&self, value: Value) -> Value {
        let id = value
            .as_object()
            .and_then(|object| object.get("id"))
            .cloned()
            .unwrap_or(Value::Null);
        let Some(object) = value.as_object() else {
            return rpc_error_response(id, -32600, "INVALID_REQUEST", "request must be an object", None);
        };
        let allowed = ["jsonrpc", "id", "method", "params"];
        if object.keys().any(|key| !allowed.contains(&key.as_str())) {
            return rpc_error_response(id, -32600, "INVALID_REQUEST", "unknown JSON-RPC request field", None);
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
            return rpc_error_response(Value::Null, -32600, "INVALID_REQUEST", "id must be a string, integer, or null", None);
        }
        if !request.params.is_object() {
            return rpc_error_response(id, -32602, "INVALID_PARAMS", "params must be an object", None);
        }
        let Some(metadata) = runtime_schema::command_metadata(&request.method) else {
            return rpc_error_response(id, -32601, "METHOD_NOT_FOUND", &format!("unsupported daemon method: {}", request.method), None);
        };
        if metadata.owner != runtime_schema::Owner::Daemon {
            return rpc_error_response(id, -32004, "UNSUPPORTED_TRANSPORT", "method is not owned by the daemon", None);
        }
        if let Err(message) = runtime_schema::validate_request(&request.method, &request.params) {
            return rpc_error_response(id, -32602, "INVALID_PARAMS", &message, None);
        }

        match self.execute_method(&request.method, &request.params) {
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
        runtime_dispatch!(self, command, args)
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

    fn agents_list(&self) -> Result<runtime_schema::AgentRecordList, DaemonError> {
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
        serde_json::from_value(serde_json::to_value(report.agents).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn scan(&self) -> Result<runtime_schema::ScanResponse, DaemonError> {
        let _authority = self.control_authority()?;
        let report = tendi_core::scan(&self.state.cwd).map_err(core_error)?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        with_database_write_lock(&store, || store.save_scan_for_workspace(&self.state.cwd, &report))?;
        serde_json::from_value(serde_json::to_value(report).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn bundled_skill_status(&self) -> Result<runtime_schema::BundledSkillStatusResponse, DaemonError> {
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
        let overwrite = request.overwrite.unwrap_or(false);
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_mutation()?;
        let report =
            tendi_core::bundled_skill::install(tendi_core::AgentKind::Shared, overwrite, false)
                .map_err(core_error)?;
        let name = report.status.name.to_string();
        let skill_path = PathBuf::from(&report.status.target);
        let refresh_names = [name.clone()];
        let refreshed = self.refresh_skill_projection(
            before,
            &refresh_names,
            std::slice::from_ref(&skill_path),
        )?;
        let updated = skills_matching_names(&refreshed.skills, &refresh_names);
        let mut value = serde_json::to_value(report).map_err(internal_error)?;
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "updated".to_string(),
                serde_json::to_value(updated).map_err(internal_error)?,
            );
        }
        serde_json::from_value(value).map_err(internal_error)
    }

    fn bundled_skill_remove(&self) -> Result<runtime_schema::BundledSkillRemoveResponse, DaemonError> {
        serde_json::from_value(
            serde_json::to_value(
                tendi_core::bundled_skill::remove(tendi_core::AgentKind::Shared)
                    .map_err(core_error)?,
            )
            .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn bundled_skill_prompt_dismiss(&self) -> Result<runtime_schema::BundledSkillPromptDismissResponse, DaemonError> {
        tendi_core::bundled_skill::dismiss_prompt().map_err(core_error)?;
        Ok(None)
    }

    fn terminal_apps_list(&self) -> Result<runtime_schema::TerminalAppRecordList, DaemonError> {
        fn app_available(paths: &[&str]) -> bool {
            paths.iter().any(|path| Path::new(path).exists())
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
        ];
        Ok(apps)
    }

    fn sessions_snapshot(&self) -> Result<runtime_schema::SessionSnapshot, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let scope_key = daemon_scope_key(self)?;
        let revision = store
            .projection_head(&scope_key, "sessions")
            .map_err(core_error)?
            .map(|head| head.revision.value())
            .unwrap_or(0);
        let rows = store
            .list_sessions_for_scope(&scope_key)
            .map_err(core_error)?
            .sessions;
        let value = json!({
            "scopeKey": scope_key,
            "domain": "sessions",
            "revision": revision,
            "schemaVersion": 1,
            "snapshotId": format!("sessions:{}:{}", scope_key, revision),
            "payload": rows,
        });
        serde_json::from_value(value).map_err(|error| {
            DaemonError::new("CONTRACT_VIOLATION", format!("sessions snapshot encode failed: {error}"))
        })
    }

    fn sessions_scan_start(&self) -> Result<runtime_schema::SessionScanStartResponse, DaemonError> {
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
        let scope_key = daemon_scope_key(self)?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
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
        if let Err(error) = self.state.operations.submit(operation_id, move || {
            if let Ok(store) = tendi_core::storage::Store::open_default() {
                let _ = with_database_write_lock(&store, || store.record_operation(&tendi_core::OperationRecord {
                    operation_id: operation_id_for_job.clone(),
                    kind: tendi_core::OperationKind::Scan,
                    scope_key: operation_scope_for_job.clone(),
                    status: tendi_core::OperationStatus::Queued,
                    input_revision: tendi_core::Revision::new(input_revision),
                    source_version: None,
                    checkpoint_json: None,
                    error: None,
                }));
                let _ = with_database_write_lock(&store, || store.update_operation(
                    &operation_id_for_job,
                    tendi_core::OperationStatus::Running,
                    None,
                    None,
                ));
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
            if let Ok(store) = tendi_core::storage::Store::open_default() {
                let status = if result.is_ok() {
                    tendi_core::OperationStatus::Committed
                } else {
                    tendi_core::OperationStatus::Failed
                };
                let _ = with_database_write_lock(&store, || store.update_operation(
                    &operation_id_for_job,
                    status,
                    None,
                    operation_error.as_deref(),
                ));
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
                    runtime_event(SESSION_SCAN_EVENT, json!({
                        "generation": generation,
                        "phase": "error",
                        "upserts": [],
                        "deleted": [],
                        "scanned": 0,
                        "complete": true,
                        "error": error.message,
                    })),
                );
            }
            daemon
                .state
                .session_runtime
                .scan_running
                .store(false, Ordering::SeqCst);
        }) {
            runtime.scan_running.store(false, Ordering::SeqCst);
            return Err(internal_error(format!("failed to queue session scan: {error:?}")));
        }
        Ok(session_scan_start_response(generation, true))
    }

    fn sessions_search(
        &self,
        request: runtime_schema::SessionsSearchRequest,
    ) -> Result<runtime_schema::SessionSearchHitList, DaemonError> {
        let candidates = request
            .candidates
            .map(|values| {
                values
                    .into_iter()
                    .map(session_identity_from_request)
                    .collect::<Vec<_>>()
            });
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let scope_key = daemon_scope_key(self)?;
        let hits = store
            .search_sessions_for_scope(&scope_key, &request.query, candidates.as_deref())
            .map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(hits).map_err(internal_error)?)
            .map_err(|error| DaemonError::new("CONTRACT_VIOLATION", format!("session search encode failed: {error}")))
    }

    fn analytics_overview(
        &self,
        request: runtime_schema::AnalyticsOverviewRequest,
    ) -> Result<runtime_schema::AnalyticsOverview, DaemonError> {
        let agent = request
            .agent
            .as_deref()
            .map(parse_agent)
            .transpose()?;
        let days = request.days as u32;
        let rank_days = request.rank_days as u32;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
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
            .overview_analytics_for_scope(&daemon_scope_key(self)?, agent, days, rank_days)
            .map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(overview).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn analytics_revision(&self) -> Result<runtime_schema::Revision, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
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
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
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
        let result = tendi_core::session_skills::run_index_for_scope(
            &self.state.cwd,
            &scope_key,
            force,
        )
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
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let scope_key = daemon_scope_key(self)?;
        serde_json::from_value(serde_json::to_value(
            store
                .session_skill_links_for_scope(&scope_key, &session_id, agent)
                .map_err(core_error)?,
        ).map_err(internal_error)?)
        .map_err(internal_error)
    }

    fn skill_session_links(
        &self,
        request: runtime_schema::SkillSessionLinksRequest,
    ) -> Result<runtime_schema::SessionSkillLinkList, DaemonError> {
        let name = request.skill_name;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let scope_key = daemon_scope_key(self)?;
        serde_json::from_value(serde_json::to_value(
            store
                .skill_session_links_for_scope(&scope_key, &name)
                .map_err(core_error)?,
        ).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn settings_get(&self) -> Result<runtime_schema::AppSettings, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        serde_json::from_value(
            serde_json::to_value(store.app_settings().map_err(core_error)?)
                .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn settings_save(
        &self,
        request: runtime_schema::AppSettings,
    ) -> Result<runtime_schema::AppSettings, DaemonError> {
        let settings = tendi_core::storage::AppSettings {
            appearance: request.appearance,
            font_family: request.font_family,
            light_theme: request.light_theme,
            dark_theme: request.dark_theme,
            app_icon: request.app_icon,
            terminal: request.terminal,
            session_resume_target: request.session_resume_target,
            missing_session_project_policy: request.missing_session_project_policy,
            editor: request.editor,
            developer_mode: request.developer_mode,
            additional_session_roots: request.additional_session_roots,
            config_profiles: request.config_profiles,
        };
        let _authority = self.control_authority()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let saved = with_database_write_lock(&store, || store.save_app_settings(settings.clone()))?;
        serde_json::from_value(serde_json::to_value(saved).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn session_projects_list(
        &self,
    ) -> Result<runtime_schema::SessionProjectSummaryList, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let scope_key = daemon_scope_key(self)?;
        serde_json::from_value(serde_json::to_value(
            store
                .list_session_projects_for_scope(&scope_key)
                .map_err(core_error)?,
        )
        .map_err(internal_error)?)
        .map_err(internal_error)
    }

    fn project_scan_scopes_list(
        &self,
    ) -> Result<runtime_schema::ProjectScanScopeList, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(store.project_scan_scopes().map_err(core_error)?)
            .map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn project_scan_scopes_save(
        &self,
        request: runtime_schema::ProjectScanScopesSaveRequest,
    ) -> Result<runtime_schema::ProjectScanScopeList, DaemonError> {
        let paths = request.paths;
        let _authority = self.control_authority()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let saved =
            with_database_write_lock(&store, || store.save_project_scan_scopes(paths.clone()))?;
        serde_json::from_value(serde_json::to_value(saved).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn projects_list(&self) -> Result<runtime_schema::ProjectRecordList, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        serde_json::from_value(
            serde_json::to_value(store.list_projects().map_err(core_error)?)
                .map_err(internal_error)?,
        )
        .map_err(internal_error)
    }

    fn projects_scan(&self) -> Result<runtime_schema::ProjectsScanResponse, DaemonError> {
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
        let _authority = self.control_authority()?;
        match tendi_core::config::save_agent_config(
            Path::new(&request.path),
            &request.expected_sha256,
            &request.content,
        ) {
            Ok(saved) => serde_json::from_value(serde_json::to_value(saved).map_err(internal_error)?)
                .map_err(internal_error),
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
        let _authority = self.control_authority()?;
        let configs = tendi_core::config::list_agent_configs().map_err(core_error)?;
        tendi_core::config::delete_agent_configs(&paths).map_err(core_error)?;

        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let mut settings = store.app_settings().map_err(core_error)?;
        let mut settings_changed = false;
        for config in configs.iter().filter(|config| paths.contains(&config.path)) {
            let Some(profile) = config.profile.as_deref() else {
                continue;
            };
            let Some(key) = tendi_core::config_profile_key(config.agent) else {
                continue;
            };
            if settings.config_profiles.get(key).map(String::as_str) == Some(profile) {
                settings.config_profiles.remove(key);
                settings_changed = true;
            }
        }
        if settings_changed {
            with_database_write_lock(&store, || store.save_app_settings(settings.clone()))?;
        }
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
        let _authority = self.control_authority()?;
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
        serde_json::from_value(serde_json::to_value(saved).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn rules_list(&self) -> Result<runtime_schema::RuleRecordList, DaemonError> {
        let report = self.rules_projection()?;
        serde_json::from_value(serde_json::to_value(report.rules).map_err(internal_error)?)
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
        let _authority = self.control_authority()?;
        let before = self.rules_projection()?;
        for path in &paths {
            if !before.rules.iter().any(|rule| rule.path == *path) {
                return Err(core_error(format!(
                    "refusing to delete unknown rule {}",
                    path.display()
                )));
            }
        }
        tendi_core::rules::delete_rule_files(&paths).map_err(core_error)?;
        let mut after = before;
        after.rules.retain(|rule| !paths.contains(&rule.path));
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let cwd = self.state.cwd.clone();
        with_database_write_lock(&store, || store.save_rules_for_workspace(&cwd, &after))?;
        self.mark_skill_backup_dirty();
        serde_json::from_value(json!({ "deleted": paths })).map_err(internal_error)
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

    fn hooks_list(&self) -> Result<runtime_schema::HookRecordList, DaemonError> {
        let report = self.hooks_projection()?;
        serde_json::from_value(serde_json::to_value(report.hooks).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn hook_delete(
        &self,
        request: runtime_schema::HookDeleteRequest,
    ) -> Result<runtime_schema::HookDeleteResponse, DaemonError> {
        let request = hook_delete_request(request)?;
        let _authority = self.control_authority()?;
        let before = self.hooks_projection()?;
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
        self.save_hook_scan(&scan)?;
        serde_json::from_value(serde_json::to_value(hook_mutation_delta(
            &scan,
            std::slice::from_ref(&request.path),
            deleted,
        )).map_err(internal_error)?)
        .map_err(internal_error)
    }

    fn hook_delete_many(
        &self,
        request: runtime_schema::HookDeleteManyRequest,
    ) -> Result<runtime_schema::HookDeleteManyResponse, DaemonError> {
        let _authority = self.control_authority()?;
        let requests = request
            .requests
            .into_iter()
            .map(hook_delete_request)
            .collect::<Result<Vec<_>, _>>()?;
        let before = self.hooks_projection()?;
        let deleted = tendi_core::hooks::hooks_matching_delete_requests(&before.hooks, &requests)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        tendi_core::hooks::delete_hooks(requests.clone()).map_err(core_error)?;
        let scan =
            tendi_core::hooks::refresh_hook_scan_after_delete(&self.state.cwd, before, &requests)
                .map_err(core_error)?;
        self.mark_skill_backup_dirty();
        self.save_hook_scan(&scan)?;
        let paths = requests
            .iter()
            .map(|request| request.path.clone())
            .collect::<Vec<_>>();
        serde_json::from_value(serde_json::to_value(hook_mutation_delta(
            &scan, &paths, deleted,
        )).map_err(internal_error)?)
        .map_err(internal_error)
    }

    fn hook_set_enabled(
        &self,
        request: runtime_schema::HookSetEnabledRequest,
    ) -> Result<runtime_schema::HookSetEnabledResponse, DaemonError> {
        let request = hook_set_enabled_request(request)?;
        let _authority = self.control_authority()?;
        let before = self.hooks_projection()?;
        tendi_core::hooks::set_hooks_enabled(vec![request.clone()]).map_err(core_error)?;
        let scan = tendi_core::hooks::refresh_hook_scan_after_set_enabled(
            &self.state.cwd,
            before,
            &request,
        )
        .map_err(core_error)?;
        self.mark_skill_backup_dirty();
        self.save_hook_scan(&scan)?;
        serde_json::from_value(serde_json::to_value(hook_mutation_delta(
            &scan,
            std::slice::from_ref(&request.path),
            Vec::new(),
        )).map_err(internal_error)?)
        .map_err(internal_error)
    }

    fn hook_set_enabled_many(
        &self,
        request: runtime_schema::HookSetEnabledManyRequest,
    ) -> Result<runtime_schema::HookSetEnabledManyResponse, DaemonError> {
        let _authority = self.control_authority()?;
        let requests = request
            .requests
            .into_iter()
            .map(hook_set_enabled_request)
            .collect::<Result<Vec<_>, _>>()?;
        if requests.is_empty() {
            return serde_json::from_value(json!({ "updated": [], "deleted": [] }))
                .map_err(internal_error);
        }
        let before = self.hooks_projection()?;
        tendi_core::hooks::set_hooks_enabled(requests.clone()).map_err(core_error)?;
        let scan = tendi_core::hooks::refresh_hook_scan_after_set_enabled_many(
            &self.state.cwd,
            before,
            &requests,
        )
        .map_err(core_error)?;
        self.mark_skill_backup_dirty();
        self.save_hook_scan(&scan)?;
        let paths = requests
            .iter()
            .map(|request| request.path.clone())
            .collect::<Vec<_>>();
        serde_json::from_value(serde_json::to_value(hook_mutation_delta(
            &scan,
            &paths,
            Vec::new(),
        )).map_err(internal_error)?)
        .map_err(internal_error)
    }

    fn hook_review(
        &self,
        request: runtime_schema::HookReviewRequest,
    ) -> Result<runtime_schema::HookReviewResponse, DaemonError> {
        let request = hook_review_request(request)?;
        let path = request.path.clone();
        let _authority = self.control_authority()?;
        let before = self.hooks_projection()?;
        let scan = tendi_core::hooks::review_hook_from_scan(before, request).map_err(core_error)?;
        self.save_hook_scan(&scan)?;
        serde_json::from_value(serde_json::to_value(hook_mutation_delta(
            &scan,
            std::slice::from_ref(&path),
            Vec::new(),
        )).map_err(internal_error)?)
        .map_err(internal_error)
    }

    fn hook_source_read(
        &self,
        request: runtime_schema::HookSourceReadRequest,
    ) -> Result<runtime_schema::HookSourceReadResponse, DaemonError> {
        let hook_match = request
            .event
            .clone()
            .map(|event| tendi_core::hooks::HookSourceMatch {
                event,
                matcher: request.matcher.clone(),
                hook_type: request.hook_type.clone(),
                command: request.command.clone(),
                url: request.url.clone(),
                prompt: request.prompt.clone(),
                filter: request.filter.clone(),
                status_message: request.status_message.clone(),
                enabled: request.enabled,
            });
        let expected_hash = request
            .expected_trust_hash
            .as_deref()
            .filter(|hash| !hash.is_empty());
        let path = Path::new(&request.path);
        let result =
            tendi_core::hooks::read_hook_source_at_path(
                path,
                agent_kind_from_request(request.agent),
                expected_hash,
                hook_match.as_ref(),
            )
                .map_err(core_error)?;
        serde_json::from_value(serde_json::to_value(result).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn mcp_list(&self) -> Result<runtime_schema::McpServerRecordList, DaemonError> {
        let report = self.mcp_projection()?;
        serde_json::from_value(serde_json::to_value(report.servers).map_err(internal_error)?)
            .map_err(internal_error)
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

    fn mcp_set_enabled(
        &self,
        request: runtime_schema::McpSetEnabledRequest,
    ) -> Result<runtime_schema::McpSetEnabledResponse, DaemonError> {
        let request = mcp_set_enabled_request(request)?;
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
                && server.server_path == request.server_path
        }) {
            return Err(conflict_error(
                "MCP server is not present in the current projection; refresh MCP before changing it",
            ));
        }
        let trust_hash = tendi_core::mcp::set_server_enabled(request.clone()).map_err(core_error)?;
        self.mark_skill_backup_dirty();
        update_mcp_projection_for_toggle(&mut report, &request, trust_hash)?;
        with_database_write_lock(&store, || store.save_mcp_for_workspace(&cwd, &report))?;
        let updated = report
            .servers
            .iter()
            .find(|server| {
                server.agent == request.agent
                    && server.name == request.name
                    && server.path == request.path
                    && server.server_path == request.server_path
            })
            .cloned()
            .ok_or_else(|| {
                conflict_error(
                    "MCP server disappeared from the current projection while changing it",
                )
            })?;
        serde_json::from_value(json!({ "updated": [updated] })).map_err(internal_error)
    }

    fn mcp_set_enabled_many(
        &self,
        request: runtime_schema::McpSetEnabledManyRequest,
    ) -> Result<runtime_schema::McpSetEnabledManyResponse, DaemonError> {
        let mut requests = request
            .requests
            .into_iter()
            .map(mcp_set_enabled_request)
            .collect::<Result<Vec<_>, _>>()?;
        if requests.is_empty() {
            return serde_json::from_value(json!({ "updated": [] })).map_err(internal_error);
        }
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
        let mut updated_keys = Vec::with_capacity(requests.len());
        for request in &mut requests {
            let current = report
                .servers
                .iter()
                .find(|server| {
                    server.agent == request.agent
                        && server.name == request.name
                        && server.path == request.path
                        && server.server_path == request.server_path
                })
                .ok_or_else(|| {
                    conflict_error(
                        "MCP server is not present in the current projection; refresh MCP before changing it",
                    )
                })?;
            request.expected_trust_hash = current.trust_hash.clone();
            let trust_hash =
                tendi_core::mcp::set_server_enabled(request.clone()).map_err(core_error)?;
            update_mcp_projection_for_toggle(&mut report, request, trust_hash)?;
            updated_keys.push((
                request.agent,
                request.name.clone(),
                request.path.clone(),
                request.server_path.clone(),
            ));
        }
        self.mark_skill_backup_dirty();
        with_database_write_lock(&store, || store.save_mcp_for_workspace(&cwd, &report))?;
        let updated = report
            .servers
            .iter()
            .filter(|server| {
                updated_keys.iter().any(|(agent, name, path, server_path)| {
                    server.agent == *agent
                        && server.name == *name
                        && server.path == *path
                        && server.server_path == *server_path
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        serde_json::from_value(json!({ "updated": updated })).map_err(internal_error)
    }

    fn prompts_list(&self) -> Result<runtime_schema::PromptRecordList, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
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
        let _authority = self.control_authority()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let prompt = tendi_core::storage::PromptWrite {
            id,
            title,
            tags,
            body,
        };
        let saved = with_database_write_lock(&store, || store.save_prompt(prompt.clone()))?;
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
        let _authority = self.control_authority()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let deleted = with_database_write_lock(&store, || store.delete_prompts(&ids))?;
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
        let page = tendi_core::transcript::parse_transcript_locator_page(
            Path::new(&request.path),
            agent,
        )
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

    fn skills_list(&self) -> Result<runtime_schema::SkillRecordList, DaemonError> {
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
        serde_json::from_value(serde_json::to_value(scan.skills).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn skills_refresh(&self) -> Result<runtime_schema::SkillsRefreshResponse, DaemonError> {
        let scan = {
            let _authority = self.lock_authority()?;
            self.scan_and_persist()?
        };
        let update_check = self.start_skill_update_check(scan.clone());
        serde_json::from_value(json!({ "skills": scan.skills, "updateCheck": update_check }))
            .map_err(internal_error)
    }

    fn skills_targets(&self) -> Result<runtime_schema::SkillTargetRecordList, DaemonError> {
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

    fn skills_backup_status(&self) -> Result<runtime_schema::SkillsBackupStatusResponse, DaemonError> {
        self.refresh_backup_projections()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let config = store.skill_backup_config().map_err(core_error)?;
        let catalog = tendi_core::skill_backup::backup_catalog(&store, &self.state.cwd)
            .map_err(core_error)?;
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
        let mut statuses = tendi_core::skill_backup::backup_statuses_for_paths(
            &store,
            &self.state.cwd,
            &paths,
        )
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
            let checkout_path = request.checkout_path
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
        let _authority = self.lock_authority()?;
        let working_directory = config
            .checkout_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        if !config.remote_url.is_empty() {
            tendi_core::skill_backup::validate_remote(&config.remote_url, working_directory)
                .map_err(core_error)?;
        }
        tendi_core::skill_backup::sync_checkout_for_restore(&config).map_err(core_error)?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let config = with_database_write_lock(&store, || store.save_skill_backup_config(&config))?;
        self.mark_skill_backup_dirty();
        serde_json::from_value(serde_json::to_value(config).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn skills_backup_now(&self) -> Result<runtime_schema::SkillsBackupNowResponse, DaemonError> {
        let _authority = self.lock_authority()?;
        if self.state.backup_sync_running.swap(true, Ordering::AcqRel) {
            return Err(invalid_argument("a skill sync is already running"));
        }
        self.state.backup_sync_dirty.store(false, Ordering::Release);
        let report = (|| -> Result<_, DaemonError> {
            self.refresh_backup_projections()?;
            let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
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
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
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
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
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
        let target = request.target
            .parse::<tendi_core::SkillTarget>()
            .map_err(core_error)?;
        let scope = request.scope
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
            .map(|resolution| tendi_core::skill_backup::BackupRestoreResolution {
                id: resolution.id,
                action: resolution.action,
            })
            .collect::<Vec<_>>();
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_mutation()?;
        let applied = tendi_core::skill_backup::apply_backup_restore_without_database(
            &plan,
            &resolutions,
        )
        .map_err(core_error)?;
        let operations = applied.operations;
        with_database_write_lock(&store, || {
            store.upsert_skill_source_records_for_workspace(&self.state.cwd, &applied.source_records)
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
        let scan = self.refresh_skill_projection(before, &refresh_names, &extra_skill_dirs)?;
        let updated = skills_matching_names(&scan.skills, &refresh_names);
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
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_mutation()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let mut records = Vec::with_capacity(entries.len());
        for (name, skill_path) in &entries {
            records.push(
                tendi_core::skill_backup::skill_backup_record_for_adoption(skill_path, name.clone())
                    .map_err(core_error)?,
            );
        }
        with_database_write_lock(&store, || {
            store.upsert_skill_source_records_for_workspace(&self.state.cwd, &records)
        })?;
        let refresh_names = records
            .iter()
            .map(|record| record.skill_name.clone())
            .collect::<Vec<_>>();
        let refresh_dirs = records
            .iter()
            .map(|record| record.skill_path.clone())
            .collect::<Vec<_>>();
        let scan = self.refresh_skill_projection(before, &refresh_names, &refresh_dirs)?;
        let updated = scan
            .skills
            .into_iter()
            .filter(|skill| refresh_names.iter().any(|name| name == &skill.name))
            .collect::<Vec<_>>();
        serde_json::from_value(json!({
            "records": records,
            "updated": updated,
            "skills": updated,
        }))
        .map_err(internal_error)
    }

    fn skills_backup_disconnect(&self) -> Result<runtime_schema::SkillsBackupDisconnectResponse, DaemonError> {
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let disconnected = with_database_write_lock(&store, || store.clear_skill_backup_config())?;
        serde_json::from_value(json!({ "disconnected": disconnected })).map_err(internal_error)
    }

    fn skills_add(
        &self,
        request: runtime_schema::SkillsAddRequest,
    ) -> Result<runtime_schema::SkillsAddResponse, DaemonError> {
        let bundled_source = request.source.trim() == tendi_core::bundled_skill::INSTALL_SOURCE;
        let options = self.skill_add_options(&request)?;
        if request.dry_run {
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
            store.persist_skill_update_persistence_for_workspace(
                &self.state.cwd,
                &source_records,
                &snapshots,
            )
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
        let refreshed = self.refresh_skill_projection(before, &refresh_names, &extra_skill_dirs)?;
        let updated = skills_matching_names(&refreshed.skills, &refresh_names);
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
        serde_json::from_value(json!({
            "name": skill.name,
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
        let scope = request.scope
            .parse::<tendi_core::SkillInstallScope>()
            .map_err(core_error)?;
        let mode = request.mode
            .parse::<tendi_core::skills::SkillDistributionMode>()
            .map_err(core_error)?;

        if targets.len() > 1 {
            if dry_run || preview_id.is_some() {
                return Err(invalid_argument(
                    "multi-target skill distribution does not support dryRun or previewId",
                ));
            }
            return serde_json::from_value(self.skills_distribute_to_targets(
                &sources, &targets, scope, mode,
            )?)
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
            return serde_json::from_value(json!({
                "applied": false,
                "plans": plans,
                "previewId": id,
            }))
            .map_err(internal_error);
        }

        let mut before = None;
        let plans = if let Some(preview_id) = preview_id.as_deref() {
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
        self.apply_skill_distribution_plans(plans, mode, Some(scan))
    }

    fn apply_skill_distribution_plans(
        &self,
        mut plans: Vec<tendi_core::skills::SkillDistributionPlan>,
        mode: tendi_core::skills::SkillDistributionMode,
        before: Option<tendi_core::skills::SkillScan>,
    ) -> Result<Value, DaemonError> {
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
            store.persist_skill_update_persistence_for_workspace_with_deleted(
                &self.state.cwd,
                &moved_sources,
                &target_records,
                &snapshots,
            )
        })?;
        let refresh_names = plans
            .iter()
            .map(|plan| plan.source_record.skill_name.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let before = match before {
            Some(scan) => scan,
            // Distribution is selected by source path.  A display name is only
            // used to refresh the affected projection after the filesystem
            // operation, so it must not trigger name-based ambiguity checks.
            None => self.skill_projection_for_mutation()?,
        };
        let extra_skill_dirs = plans
            .iter()
            .map(|plan| plan.destination.clone())
            .collect::<Vec<_>>();
        let refreshed = self.refresh_skill_projection(before, &refresh_names, &extra_skill_dirs)?;
        let updated = skills_matching_names(&refreshed.skills, &refresh_names);
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

        let _authority = self.lock_authority()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let scan = self.skill_projection_for_ids(&ids)?;
        let mut seen = BTreeSet::new();
        let mut targets = Vec::new();
        for skill in scan
            .skills
            .iter()
            .filter(|skill| ids.iter().any(|id| tendi_core::skills::skill_matches_id(skill, id)))
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
        self.mark_skill_backup_dirty();
        let paths = plan
            .targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<Vec<_>>();
        let scan = tendi_core::skills::refresh_skill_scan(&self.state.cwd, scan, &ids, &[])
            .map_err(core_error)?;
        let cwd = self.state.cwd.clone();
        with_database_write_lock(&store, || {
            store.delete_skill_source_records_for_workspace(&cwd, &paths)?;
            store.save_skills_for_workspace(&cwd, &scan)
        })?;
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
        let _authority = (!dry_run).then(|| self.lock_authority()).transpose()?;
        let before = self.skill_projection_for_ids(&ids)?;
        let changeset =
            tendi_core::skills::plan_visibility_many_for_scan(&before, &ids, visibility)
                .map_err(core_error)?;
        let summary = tendi_core::skills::format_changeset(&changeset);
        if !dry_run {
            tendi_core::skills::apply_changes(&changeset).map_err(core_error)?;
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
        let _authority = (!dry_run).then(|| self.lock_authority()).transpose()?;
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
            tendi_core::skills::refresh_wrapper_from_names_for_scan(
                &before,
                &name,
                &ids,
                manual_children,
            )
        } else {
            tendi_core::skills::plan_wrapper_from_names_for_scan(
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
            tendi_core::skills::apply_changes(&changeset).map_err(core_error)?;
            let mut refresh_names = ids;
            refresh_names.push(name);
            let extra_skill_dirs = changeset
                .changes
                .iter()
                .filter_map(|change| change.path.parent().map(Path::to_path_buf))
                .collect::<Vec<_>>();
            let refreshed =
                self.refresh_skill_projection(before, &refresh_names, &extra_skill_dirs)?;
            let updated = skills_matching_names(&refreshed.skills, &refresh_names);
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
            let scan = self.skill_projection()?;
            return serde_json::from_value(json!({
                "updateCheck": self.start_skill_update_check(scan),
            }))
            .map_err(internal_error);
        }
        let _authority = self.lock_authority()?;
        let scan = self.scan_and_persist()?;
        serde_json::from_value(serde_json::to_value(scan.skills).map_err(internal_error)?)
            .map_err(internal_error)
    }

    fn skills_updates_cancel(&self) -> Result<runtime_schema::SkillsUpdatesCancelResponse, DaemonError> {
        let running = self.state.skill_update_running.load(Ordering::Acquire);
        if running {
            self.state
                .skill_update_cancelled
                .store(true, Ordering::Release);
        }
        serde_json::from_value(json!({
            "status": if running { "cancellation-requested" } else { "not-running" },
        }))
        .map_err(internal_error)
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
        let operation_id = match tendi_core::OperationId::new(format!(
            "skill-update-check-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )) {
            Ok(operation_id) => operation_id,
            Err(_) => {
                self.state
                    .skill_update_running
                    .store(false, Ordering::Release);
                return "unavailable";
            }
        };
        if self.state.operations.submit(operation_id, move || {
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
            daemon.emit_event(SKILL_UPDATE_EVENT, runtime_event(SKILL_UPDATE_EVENT, event));
            daemon
                .state
                .skill_update_running
                .store(false, Ordering::Release);
        }).is_err() {
            self.state
                .skill_update_running
                .store(false, Ordering::Release);
            return "unavailable";
        }
        "started"
    }

    fn skills_update(
        &self,
        request: runtime_schema::SkillsUpdateRequest,
    ) -> Result<runtime_schema::SkillsUpdateResponse, DaemonError> {
        let dry_run = request.dry_run.unwrap_or(false);
        let _authority = (!dry_run).then(|| self.lock_authority()).transpose()?;
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
        let plan = tendi_core::skills::plan_skill_updates_many_for_scan(&scan, &ids)
            .map_err(core_error)?;
        let summary = tendi_core::skills::format_update_plan(&plan);
        let can_apply = plan.can_apply();
        if !dry_run && !can_apply {
            return Err(conflict_error(
                "skill update has no applicable changes; preview the update again",
            ));
        }
        if !dry_run {
            let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
            let resolutions = request
                .resolutions
                .as_ref()
                .map(|resolutions| resolutions.extra.clone())
                .unwrap_or_default();
            let prepared = tendi_core::skills::prepare_skill_update_plan_with_resolutions(
                &plan,
                &resolutions,
            )
            .map_err(core_error)?;
            let expected_source_versions =
                tendi_core::skills::prepare_skill_update_persistence_for_workspace(
                    &store,
                    &self.state.cwd,
                    &prepared,
                )
                .map_err(core_error)?
                .expected_source_versions;
            with_database_write_lock(&store, || {
                store.validate_skill_source_versions_for_workspace(
                    &self.state.cwd,
                    &expected_source_versions,
                )?;
                let filesystem =
                    tendi_core::skills::apply_skill_update_plan_filesystem_transaction(&prepared)?;
                let result = (|| {
                    let persistence =
                        tendi_core::skills::prepare_skill_update_persistence_for_workspace(
                            &store,
                            &self.state.cwd,
                            &prepared,
                        )?;
                    store.persist_skill_update_persistence_for_workspace_checked(
                        &self.state.cwd,
                        &expected_source_versions,
                        &persistence.source_records,
                        &persistence.snapshots,
                    )
                })();
                match result {
                    Ok(()) => {
                        filesystem.commit();
                        Ok(())
                    }
                    Err(error) => Err(error.context(filesystem.rollback_context()?)),
                }
            })?;
            let refresh_names = skill_update_refresh_names(&scan, &plan);
            let extra_skill_dirs = skill_update_refresh_dirs(&plan);
            let scan = self.refresh_skill_projection(scan, &refresh_names, &extra_skill_dirs)?;
            let updated = skills_matching_names(&scan.skills, &refresh_names);
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
        let preview_id = if dry_run {
            None
        } else {
            Some(request.preview_id.ok_or_else(|| {
                conflict_error("skill update preview expired; preview the update again")
            })?)
        };
        let plan = if dry_run {
            let scan = self.skill_projection_for_ids(&ids)?;
            tendi_core::skills::plan_skill_updates_many_for_scan(&scan, &ids)
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
            if preview.id != preview_id || preview.names != ids {
                return Err(conflict_error(
                    "update selection changed; preview the update again",
                ));
            }
            preview.plan.clone()
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
                *self
                    .state
                    .update_preview
                    .lock()
                    .map_err(|_| internal_error("update preview store is unavailable"))? =
                    Some(SkillUpdatePreview {
                        id: id.clone(),
                        names: ids.clone(),
                        plan: plan.clone(),
                    });
                Value::String(id)
            } else {
                *self
                    .state
                    .update_preview
                    .lock()
                    .map_err(|_| internal_error("update preview store is unavailable"))? = None;
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
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_ids(&ids)?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let resolutions = request
            .resolutions
            .map(|resolutions| resolutions.extra)
            .unwrap_or_default();
        let prepared = tendi_core::skills::prepare_skill_update_plan_with_resolutions(
            &plan,
            &resolutions,
        )
        .map_err(core_error)?;
        let expected_source_versions =
            tendi_core::skills::prepare_skill_update_persistence_for_workspace(
                &store,
                &self.state.cwd,
                &prepared,
            )
            .map_err(core_error)?
            .expected_source_versions;
        with_database_write_lock(&store, || {
            store.validate_skill_source_versions_for_workspace(
                &self.state.cwd,
                &expected_source_versions,
            )?;
            let filesystem =
                tendi_core::skills::apply_skill_update_plan_filesystem_transaction(&prepared)?;
            let result = (|| {
                let persistence =
                    tendi_core::skills::prepare_skill_update_persistence_for_workspace(
                        &store,
                        &self.state.cwd,
                        &prepared,
                    )?;
                store.persist_skill_update_persistence_for_workspace_checked(
                    &self.state.cwd,
                    &expected_source_versions,
                    &persistence.source_records,
                    &persistence.snapshots,
                )
            })();
            match result {
                Ok(()) => {
                    filesystem.commit();
                    Ok(())
                }
                Err(error) => Err(error.context(filesystem.rollback_context()?)),
            }
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
        let refresh_names = ids.clone();
        let extra_skill_dirs = skill_update_refresh_dirs(&plan);
        let scan = self.refresh_skill_projection(before, &refresh_names, &extra_skill_dirs)?;
        let updated = skills_matching_names(&scan.skills, &refresh_names);
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
        let _authority = self.lock_authority()?;
        let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
        let scan = self.skill_projection_for_ids(&ids)?;
        let plan = tendi_core::skills::plan_skill_delete_many_for_scan(&scan, &ids)
            .map_err(core_error)?;
        let summary = tendi_core::skills::format_delete_plan(&plan);
        tendi_core::skills::apply_skill_delete_plan(&plan).map_err(core_error)?;
        self.mark_skill_backup_dirty();
        let paths = plan
            .targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<Vec<_>>();
        let scan = tendi_core::skills::refresh_skill_scan(&self.state.cwd, scan, &ids, &[])
            .map_err(core_error)?;
        let cwd = self.state.cwd.clone();
        with_database_write_lock(&store, || {
            store.delete_skill_source_records_for_workspace(&cwd, &paths)?;
            store.save_skills_for_workspace(&cwd, &scan)
        })?;
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
        let (name, cached) = self.skill_context_for_id(
            request.skill_path.as_deref(),
            &skill_id,
            &scan,
        )?;
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
        let (name, cached) = self.skill_context_for_id(
            request.skill_path.as_deref(),
            &skill_id,
            &scan,
        )?;
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
        skill_path: Option<&str>,
        skill_id: &str,
        scan: &tendi_core::skills::SkillScan,
    ) -> Result<(String, PathBuf), DaemonError> {
        let skill = scan
            .skills
            .iter()
            .find(|skill| tendi_core::skills::skill_matches_id(skill, skill_id))
            .ok_or_else(|| conflict_error(format!("unknown skill id: {skill_id}")))?;
        if let Some(path) = skill_path {
            let path = PathBuf::from(path);
            if skill.paths.iter().any(|entry| entry.path == path) && path.is_dir() {
                return Ok((skill.name.clone(), path));
            }
            return Err(conflict_error(format!(
                "skill path is not registered for skill {skill_id}"
            )));
        }
        skill
            .paths
            .first()
            .map(|path| (skill.name.clone(), path.path.clone()))
            .filter(|(_, path)| path.is_dir())
            .ok_or_else(|| core_error(format!("skill {skill_id} has no readable location")))
    }

    fn skill_file_save(
        &self,
        request: runtime_schema::SkillFileSaveRequest,
    ) -> Result<runtime_schema::SkillFileSaveResponse, DaemonError> {
        let skill_id = request.skill_id;
        let relative_path = request.relative_path;
        let expected_sha256 = request.expected_sha256;
        let content = request.content;
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_ids(std::slice::from_ref(&skill_id))?;
        let (name, cached) = self.skill_context_for_id(
            request.skill_path.as_deref(),
            &skill_id,
            &before,
        )?;
        let result = tendi_core::files::save_skill_file(
            &self.state.cwd,
            &name,
            &relative_path,
            &expected_sha256,
            &content,
            Some(cached.as_path()),
        )
        .map_err(core_error)?;
        let mut value = serde_json::to_value(result).map_err(internal_error)?;
        if tendi_core::files::skill_relative_path_affects_projection(&relative_path) {
            let scan = self.refresh_skill_projection(before, std::slice::from_ref(&skill_id), &[])?;
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
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_ids(std::slice::from_ref(&skill_id))?;
        let (name, cached) = self.skill_context_for_id(
            request.skill_path.as_deref(),
            &skill_id,
            &before,
        )?;
        let result = tendi_core::files::create_skill_file(
            &self.state.cwd,
            &name,
            &relative_path,
            Some(cached.as_path()),
        )
        .map_err(core_error)?;
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
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_ids(std::slice::from_ref(&skill_id))?;
        let (name, cached) = self.skill_context_for_id(
            request.skill_path.as_deref(),
            &skill_id,
            &before,
        )?;
        tendi_core::files::create_skill_folder(
            &self.state.cwd,
            &name,
            &relative_path,
            Some(cached.as_path()),
        )
        .map_err(core_error)?;
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
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_ids(std::slice::from_ref(&skill_id))?;
        let (name, cached) = self.skill_context_for_id(
            request.skill_path.as_deref(),
            &skill_id,
            &before,
        )?;
        tendi_core::files::rename_skill_path(
            &self.state.cwd,
            &name,
            &from,
            &to,
            Some(cached.as_path()),
        )
        .map_err(core_error)?;
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
        let _authority = self.lock_authority()?;
        let before = self.skill_projection_for_ids(std::slice::from_ref(&skill_id))?;
        let (name, cached) = self.skill_context_for_id(
            request.skill_path.as_deref(),
            &skill_id,
            &before,
        )?;
        tendi_core::files::delete_skill_path(
            &self.state.cwd,
            &name,
            &relative_path,
            Some(cached.as_path()),
        )
        .map_err(core_error)?;
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
        let files = tendi_core::files::list_skill_files(
            &self.state.cwd,
            name,
            Some(skill_dir),
        )
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

        let cached = tendi_core::skills::scan_skills_synced_for_projection(&cwd)
            .map_err(core_error)?;
        for source_path in source_paths {
            if cached
                .scan
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
        Ok(cached.scan)
    }

    fn skill_projection_for_ids(
        &self,
        ids: &[String],
    ) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        let scan = self.skill_projection_for_mutation()?;
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
        names: &[String],
        extra_skill_dirs: &[PathBuf],
    ) -> Result<tendi_core::skills::SkillScan, DaemonError> {
        self.mark_skill_backup_dirty();
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
        Ok(scan)
    }

    fn mark_skill_backup_dirty(&self) {
        self.state
            .backup_sync_dirty
            .store(true, Ordering::Release);
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
        if self.state.operations.submit(operation_id, move || {
            let result = (|| -> anyhow::Result<()> {
                let store = tendi_core::storage::Store::open_default()?;
                if store.skill_backup_config()?.is_none() {
                    return Ok(());
                }
                daemon
                    .refresh_backup_projections()
                    .map_err(|error| anyhow::anyhow!(error.message))?;
                tendi_core::skill_backup::backup_now(&store, &daemon.state.cwd)?;
                Ok(())
            })();
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
        }).is_err() {
            self.state
                .backup_sync_running
                .store(false, Ordering::Release);
            self.mark_skill_backup_dirty();
        }
    }

    fn refresh_backup_projections(&self) -> Result<(), DaemonError> {
        self.skill_projection()?;
        self.mcp_projection()?;
        self.rules_projection()?;
        self.hooks_projection()?;
        Ok(())
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
        request: &runtime_schema::SkillsAddRequest,
    ) -> Result<tendi_core::skills::SkillAddOptions, DaemonError> {
        required_request_text(&request.source, "source")?;
        required_request_text(&request.target, "target")?;
        required_request_text(&request.scope, "scope")?;
        request_text_items(&request.skills, "skills")?;
        let source = request.source.clone();
        let source = if source.trim() == tendi_core::bundled_skill::INSTALL_SOURCE {
            tendi_core::bundled_skill::install_source_path()
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
    let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
    let scope_key = daemon_scope_key(daemon)?;
    if daemon
        .state
        .scoped_search_rebuilt
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        if let Err(error) = store.rebuild_scoped_session_search_for_scope(&scope_key) {
            daemon
                .state
                .scoped_search_rebuilt
                .store(false, Ordering::Release);
            return Err(core_error(error));
        }
    }
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
            let changed = cache.changed_sessions(&report.sessions);
            let base_revision = store
                .projection_head(&scope_key, "sessions")
                .map_err(core_error)?
                .map(|head| head.revision)
                .unwrap_or(tendi_core::Revision::ZERO);
            let upserts = with_session_database_write_lock(&store, || {
                store.apply_session_delta_and_resolve_projects_for_scope(&scope_key, &changed)
            })?;
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
                runtime_event(SESSION_SCAN_EVENT, json!({
                    "generation": generation,
                    "phase": "recent",
                    "upserts": upserts,
                    "deleted": [],
                    "scanned": scanned,
                    "complete": false,
                    "error": Value::Null,
                })),
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
        runtime_event(SESSION_SCAN_EVENT, json!({
            "generation": generation,
            "phase": "recent",
            "upserts": [],
            "deleted": [],
            "scanned": scanned,
            "complete": true,
            "error": Value::Null,
        })),
    );

    let cache = store
        .session_scan_cache_for_scope(&scope_key)
        .map_err(core_error)?;
    let mut report = tendi_core::sessions::scan_sessions_with_additional_roots_cached(
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
    with_session_database_write_lock(&store, || {
        store.resolve_session_projects_for_scope(&scope_key, &mut report.sessions)?;
        store.save_sessions_at_for_scope(&scope_key, &report, scan_started_at)
    })?;
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
        runtime_event(SESSION_SCAN_EVENT, json!({
            "generation": generation,
            "phase": "backfill",
            "upserts": [],
            "deleted": [],
            "scanned": report.sessions.len(),
            "complete": true,
            "error": Value::Null,
        })),
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
    let root = tendi_core::storage::canonical_workspace_root(&daemon.state.cwd);
    tendi_core::ScopeKey::new(root.display().to_string())
        .map_err(|error| core_error(anyhow::anyhow!(error)))
}

fn event_projection_domain(event: &str) -> Option<String> {
    match event {
        SESSION_SCAN_EVENT => Some("sessions".to_string()),
        ANALYTICS_PROGRESS_EVENT | ANALYTICS_REVISION_EVENT => Some("analytics".to_string()),
        SKILL_UPDATE_EVENT => Some("skills".to_string()),
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
        let paths = std::mem::take(&mut pending);
        for path in paths {
            match tendi_core::config::read_agent_config(&path) {
                Ok(snapshot) => daemon.emit_event(
                    CONFIG_CHANGED_EVENT,
                    runtime_event(CONFIG_CHANGED_EVENT, serde_json::to_value(snapshot).expect("config event serializes")),
                ),
                Err(error) => tendi_core::logging::global().warn(
                    "config change snapshot failed",
                    json!({ "path": path, "error": error.to_string() }),
                ),
            }
        }
    }
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

fn session_watch_loop(daemon: Daemon, receiver: Receiver<notify::Result<Event>>) {
    let runtime = Arc::clone(&daemon.state.session_runtime);
    let mut pending = BTreeSet::new();
    let mut pending_since = None;
    loop {
        if daemon.is_shutting_down() {
            break;
        }
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
                    runtime_event(SESSION_SCAN_EVENT, json!({
                        "generation": generation,
                        "phase": "watch",
                        "upserts": [],
                        "deleted": [],
                        "scanned": 0,
                        "complete": false,
                        "error": message,
                    })),
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
        let operation_id = tendi_core::OperationId::new(format!(
            "session-watch-dispatch-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let daemon_for_job = daemon.clone();
        let paths_for_log = paths.clone();
        if let Ok(operation_id) = operation_id {
            if daemon
                .state
                .operations
                .submit(operation_id, move || process_session_watch_paths(&daemon_for_job, &paths))
                .is_err()
            {
                tendi_core::logging::global().warn(
                    "session watcher operation queue is full",
                    json!({ "paths": paths_for_log }),
                );
            }
        }
    }
}

fn process_session_watch_paths(daemon: &Daemon, paths: &[PathBuf]) {
    let scope_key = match daemon_scope_key(daemon) {
        Ok(scope_key) => scope_key,
        Err(error) => {
            tendi_core::logging::global().error(
                "session watcher scope resolution failed",
                json!({ "error": &error.message }),
            );
            return;
        }
    };
    let result = retry_session_watch_update(|| process_session_watch_paths_once(paths, &scope_key));

    match result {
        Ok((
            upserts,
            deleted,
            analytics_sessions,
            base_revision,
            revision,
            operation_id,
        ))
            if !upserts.is_empty() || !deleted.is_empty() || !analytics_sessions.is_empty() =>
        {
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
                daemon.emit_event(SESSION_SCAN_EVENT, runtime_event(SESSION_SCAN_EVENT, payload));
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

fn retry_session_watch_update<T, F>(mut update: F) -> Result<T, DaemonError>
where
    F: FnMut() -> Result<T, DaemonError>,
{
    for attempt in 0..SESSION_WATCH_DATABASE_RETRY_ATTEMPTS {
        match update() {
            Ok(value) => return Ok(value),
            Err(error)
                if is_database_lock_error(&error)
                    && attempt + 1 < SESSION_WATCH_DATABASE_RETRY_ATTEMPTS =>
            {
                tendi_core::logging::global().warn(
                    "session watcher update deferred",
                    json!({
                        "attempt": attempt + 1,
                        "retry_in_ms": SESSION_WATCH_DATABASE_RETRY.as_millis(),
                        "error": &error.message,
                    }),
                );
                thread::sleep(SESSION_WATCH_DATABASE_RETRY);
            }
            result => return result,
        }
    }
    unreachable!("session watcher retry loop must return from every attempt")
}

fn is_database_lock_error(error: &DaemonError) -> bool {
    let message = error.message.to_ascii_lowercase();
    message.contains("database is locked")
        || message.contains("database table is locked")
        || message.contains("database schema is locked")
        || message.contains("timed out waiting for the database write lock")
}

fn process_session_watch_paths_once(
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
    let store = tendi_core::storage::Store::open_default().map_err(core_error)?;
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
    let report = tendi_core::sessions::scan_session_paths(&existing_paths, &cache);
    let analytics_sessions = report.sessions.clone();
    let empty_paths = existing_paths
        .iter()
        .filter(|path| tendi_core::sessions::is_session_candidate_path(path))
        .filter(|path| !report.sessions.iter().any(|session| session.path == **path))
        .cloned()
        .collect::<Vec<_>>();
    let changed = cache.changed_sessions(&report.sessions);
    let mut removed_paths = deleted_paths;
    removed_paths.extend(empty_paths);
    let (upserts, deleted) = with_session_database_write_lock(&store, || {
        store.apply_session_changes_for_scope(scope_key, &changed, &removed_paths)
    })?;
    let revision = store
        .projection_head(scope_key, "sessions")
        .map_err(core_error)?
        .map(|head| head.revision)
        .unwrap_or(base_revision);
    Ok((
        upserts,
        deleted,
        analytics_sessions,
        base_revision,
        revision,
        operation_id,
    ))
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
        runtime_event(ANALYTICS_PROGRESS_EVENT, json!({
            "phase": phase,
            "completed": initial.completed,
            "total": initial.total,
            "running": true,
            "error": Value::Null,
        })),
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
    let result = match daemon.state.operations.execute(operation_id, move || {
        let store = tendi_core::storage::Store::open_default()?;
        with_database_write_lock(&store, || {
            store.refresh_session_analytics_for_scope_with_progress(&scope_key, &sessions, |progress| {
                if let Ok(mut current) = progress_state.lock() {
                    *current = progress;
                }
                daemon_for_job.emit_event(
                    ANALYTICS_PROGRESS_EVENT,
                    runtime_event(ANALYTICS_PROGRESS_EVENT, json!({
                        "phase": phase,
                        "completed": progress.completed,
                        "total": progress.total,
                        "running": progress.completed < progress.total,
                        "error": Value::Null,
                    })),
                );
            })
        })
        .map_err(daemon_error_anyhow)
    }) {
        Ok(result) => result.map_err(core_error),
        Err(error) => Err(internal_error(format!(
            "analytics operation could not be queued: {error:?}"
        ))),
    };
    match result {
        Ok(report) => Ok(report),
        Err(error) => {
            let message = error.message.clone();
            let last_progress = last_progress
                .lock()
                .map(|progress| *progress)
                .unwrap_or(initial);
            daemon.emit_event(
                ANALYTICS_PROGRESS_EVENT,
                runtime_event(ANALYTICS_PROGRESS_EVENT, json!({
                    "phase": phase,
                    "completed": last_progress.completed,
                    "total": last_progress.total,
                    "running": false,
                    "error": message,
                })),
            );
            Err(core_error(message))
        }
    }
}

fn execute_analytics_storage_operation<T, F>(
    daemon: &Daemon,
    operation_name: &'static str,
    operation: F,
) -> Result<T, DaemonError>
where
    T: Send + 'static,
    F: FnOnce(&tendi_core::storage::Store) -> anyhow::Result<T> + Send + 'static,
{
    let operation_id = tendi_core::OperationId::new(format!(
        "{operation_name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
    .map_err(|error| core_error(anyhow::anyhow!(error)))?;
    match daemon.state.operations.execute(operation_id, move || {
        let store = tendi_core::storage::Store::open_default()?;
        operation(&store)
    }) {
        Ok(result) => result.map_err(core_error),
        Err(error) => Err(internal_error(format!(
            "analytics operation could not be queued: {error:?}"
        ))),
    }
}

fn session_analytics_loop(daemon: Daemon, receiver: Receiver<AnalyticsRefreshJob>) {
    let store = match tendi_core::storage::Store::open_default() {
        Ok(store) => store,
        Err(error) => {
            daemon.emit_event(
                ANALYTICS_PROGRESS_EVENT,
                runtime_event(ANALYTICS_PROGRESS_EVENT, json!({
                    "phase": "backfill",
                    "completed": 0,
                    "total": 0,
                    "running": false,
                    "error": format!("{error:#}"),
                })),
            );
            return;
        }
    };
    let mut legacy_backfill_complete = false;
    let mut overview_backfill_complete = false;
    let mut last_backfill_revision_emit = Instant::now();
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
        let mut pending = BTreeMap::<
            String,
            (tendi_core::ScopeKey, tendi_core::SessionRecord),
        >::new();
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
            let mut by_scope = BTreeMap::<
                tendi_core::ScopeKey,
                Vec<tendi_core::SessionRecord>,
            >::new();
            for (_key, (scope_key, session)) in pending {
                by_scope.entry(scope_key).or_default().push(session);
            }
            for (scope_key, sessions) in by_scope {
                let _ = refresh_session_analytics_serialized(&daemon, phase, &scope_key, &sessions);
                if let Ok(revision) = store
                    .projection_head(&scope_key, "analytics")
                    .map(|head| head.map(|head| head.revision.value()).unwrap_or_default())
                {
                    daemon.emit_event(
                        ANALYTICS_REVISION_EVENT,
                        runtime_event(ANALYTICS_REVISION_EVENT, json!({ "scopeKey": scope_key, "revision": revision })),
                    );
                }
            }
        }
        if !legacy_backfill_complete {
            let backfill_result = execute_analytics_storage_operation(
                &daemon,
                "analytics-legacy-backfill",
                |store| {
                    with_database_write_lock(store, || {
                        store.backfill_session_analytics_overview_index_batch(32)
                    })
                    .map_err(daemon_error_anyhow)
                },
            );
            match backfill_result {
                Ok(report) => {
                    legacy_backfill_complete = report.remaining == 0;
                    if report.processed > 0
                        && (legacy_backfill_complete
                            || last_backfill_revision_emit.elapsed() >= Duration::from_millis(500))
                    {
                        last_backfill_revision_emit = Instant::now();
                    }
                }
                Err(error) => daemon.emit_event(
                    ANALYTICS_PROGRESS_EVENT,
                    runtime_event(ANALYTICS_PROGRESS_EVENT, json!({
                        "phase": "backfill",
                        "completed": 0,
                        "total": 0,
                        "running": false,
                        "error": error.message,
                    })),
                ),
            }
        }
        if !overview_backfill_complete {
            let overview_result = execute_analytics_storage_operation(
                &daemon,
                "analytics-overview-backfill",
                |store| {
                    with_database_write_lock(store, || {
                        store.backfill_session_analytics_overview_batch(32)
                    })
                    .map_err(daemon_error_anyhow)
                },
            );
            match overview_result {
                Ok(report) => {
                    overview_backfill_complete =
                        report.remaining == 0 || (report.processed == 0 && report.failed > 0);
                }
                Err(error) => daemon.emit_event(
                    ANALYTICS_PROGRESS_EVENT,
                    runtime_event(ANALYTICS_PROGRESS_EVENT, json!({
                        "phase": "backfill",
                        "completed": 0,
                        "total": 0,
                        "running": false,
                        "error": error.message,
                    })),
                ),
            }
        }
    }
}

fn skills_matching_names(
    skills: &[tendi_core::skills::SkillRecord],
    names: &[String],
) -> Vec<tendi_core::skills::SkillRecord> {
    skills
        .iter()
        .filter(|skill| {
            names
                .iter()
                .any(|name| tendi_core::skills::skill_matches_selector(skill, name))
        })
        .cloned()
        .collect()
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

fn command_requires_serialized_write(command: &str, args: &Value) -> bool {
    runtime_schema::command_requires_serialized_write(command, args)
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
    Ok(vec![target.parse::<tendi_core::SkillTarget>().map_err(core_error)?])
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

fn hook_mutation_delta(
    scan: &tendi_core::hooks::HookScan,
    paths: &[PathBuf],
    deleted: Vec<tendi_core::hooks::HookRecord>,
) -> tendi_core::hooks::HookMutationDelta {
    let path_set = paths.iter().collect::<std::collections::HashSet<_>>();
    tendi_core::hooks::HookMutationDelta {
        updated: scan
            .hooks
            .iter()
            .filter(|hook| path_set.contains(&hook.path))
            .cloned()
            .collect(),
        deleted,
    }
}

fn parse_agent(value: &str) -> Result<tendi_core::AgentKind, DaemonError> {
    tendi_core::parse_agent(value).map_err(core_error)
}

fn required_request_text(value: &str, name: &str) -> Result<(), DaemonError> {
    if value.trim().is_empty() {
        return Err(invalid_argument(format!("missing or empty argument: {name}")));
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

fn session_identity_from_request(
    value: runtime_schema::SessionIdentity,
) -> tendi_core::sessions::SessionIdentity {
    tendi_core::sessions::SessionIdentity {
        id: value.id,
        agent: agent_kind_from_request(value.agent),
        path: PathBuf::from(value.path),
    }
}

fn hook_delete_request(
    request: runtime_schema::HookDeleteRequest,
) -> Result<tendi_core::hooks::HookDeleteRequest, DaemonError> {
    let event = request
        .event
        .trim()
        .to_string();
    if event.is_empty() {
        return Err(invalid_argument("missing or empty argument: event"));
    }
    if request.expected_trust_hash.trim().is_empty() {
        return Err(invalid_argument(
            "missing or empty argument: expectedTrustHash",
        ));
    }
    Ok(tendi_core::hooks::HookDeleteRequest {
        agent: agent_kind_from_request(request.agent),
        path: PathBuf::from(request.path),
        expected_trust_hash: request.expected_trust_hash,
        event,
        matcher: request.matcher,
        hook_type: request.hook_type,
        command: request.command,
        url: request.url,
        prompt: request.prompt,
        filter: request.filter,
        status_message: request.status_message,
    })
}

fn hook_set_enabled_request(
    request: runtime_schema::HookSetEnabledRequest,
) -> Result<tendi_core::hooks::HookSetEnabledRequest, DaemonError> {
    let event = request
        .event
        .trim()
        .to_string();
    if event.is_empty() {
        return Err(invalid_argument("missing or empty argument: event"));
    }
    if request.expected_trust_hash.trim().is_empty() {
        return Err(invalid_argument(
            "missing or empty argument: expectedTrustHash",
        ));
    }
    Ok(tendi_core::hooks::HookSetEnabledRequest {
        agent: agent_kind_from_request(request.agent),
        path: PathBuf::from(request.path),
        expected_trust_hash: request.expected_trust_hash,
        event,
        matcher: request.matcher,
        hook_type: request.hook_type,
        command: request.command,
        url: request.url,
        prompt: request.prompt,
        filter: request.filter,
        status_message: request.status_message,
        enabled: request.enabled,
    })
}

fn hook_review_request(
    request: runtime_schema::HookReviewRequest,
) -> Result<tendi_core::hooks::HookReviewRequest, DaemonError> {
    let event = request
        .event
        .trim()
        .to_string();
    if event.is_empty() {
        return Err(invalid_argument("missing or empty argument: event"));
    }
    if request.expected_trust_hash.trim().is_empty() {
        return Err(invalid_argument(
            "missing or empty argument: expectedTrustHash",
        ));
    }
    Ok(tendi_core::hooks::HookReviewRequest {
        agent: agent_kind_from_request(request.agent),
        path: PathBuf::from(request.path),
        expected_trust_hash: request.expected_trust_hash,
        event,
        matcher: request.matcher,
        hook_type: request.hook_type,
        command: request.command,
        url: request.url,
        prompt: request.prompt,
        filter: request.filter,
        status_message: request.status_message,
    })
}

fn mcp_set_enabled_request(
    request: runtime_schema::McpSetEnabledRequest,
) -> Result<tendi_core::mcp::McpSetEnabledRequest, DaemonError> {
    Ok(tendi_core::mcp::McpSetEnabledRequest {
        agent: agent_kind_from_request(request.agent),
        path: PathBuf::from(request.path),
        expected_trust_hash: request.expected_trust_hash,
        name: request.name,
        enabled: request.enabled,
        server_path: request.server_path,
    })
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
        }
        if server.agent == request.agent
            && server.name == request.name
            && server.path == request.path
            && server.server_path == request.server_path
        {
            server.enabled = request.enabled;
            server.status =
                tendi_core::mcp::mcp_status_after_toggle(request.agent, request.enabled).to_string();
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
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream; charset=utf-8\r\ncache-control: no-cache\r\naccess-control-allow-origin: *\r\naccess-control-allow-headers: content-type, authorization\r\nconnection: keep-alive\r\n\r\n"
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

    fn listed_skill_id(response: &Value, name: &str) -> String {
        response
            .as_array()
            .and_then(|skills| skills.iter().find(|skill| skill["name"] == name))
            .and_then(|skill| skill["id"].as_str())
            .unwrap_or_else(|| panic!("skill {name} was not present in listing: {response}"))
            .to_string()
    }

    fn run_method(daemon: &Daemon, method: &str, params: Value) -> Result<Value, DaemonError> {
        daemon.execute_method(method, &params)
    }

    fn run_method_ok(daemon: &Daemon, method: &str, params: Value) -> Value {
        run_method(daemon, method, params)
            .unwrap_or_else(|error| panic!("test command failed: {error:?}"))
    }

    #[test]
    fn prompt_save_rejects_empty_title_as_invalid_argument() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = Daemon::new(root.clone());
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
        assert_eq!(
            error.message,
            "missing or empty argument: title"
        );
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

        assert_eq!(result.generation, 7);
        assert!(!result.started);
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
        let listed = run_method_ok(&daemon, "skills_list", json!({}));
        let demo_id = listed_skill_id(&listed, "demo");
        let skill_path = root.join(".agents/skills/demo");
        let _files = run_method_ok(
            &daemon,
            "skill_files",
            json!({ "skillId": demo_id.clone(), "skillPath": skill_path }),
        );
        let read = run_method_ok(
            &daemon,
            "skill_file_read",
            json!({ "skillId": demo_id.clone(), "relativePath": "SKILL.md", "skillPath": skill_path }),
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
    fn direct_reads_do_not_wait_for_database_write_lock() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let daemon = Daemon::new(root.clone());
        let listed = run_method_ok(&daemon, "skills_list", json!({}));
        let demo_id = listed_skill_id(&listed, "demo");
        let skill_path = root.join(".agents/skills/demo");
        let requests = [
            ("settings_get", json!({})),
            ("skill_session_links", json!({ "skillName": "demo" })),
            ("skills_targets", json!({})),
            (
                "skill_files",
                json!({ "skillId": demo_id.clone(), "skillPath": skill_path.clone() }),
            ),
            (
                "skill_file_read",
                json!({
                    "skillId": demo_id,
                    "relativePath": "SKILL.md",
                    "skillPath": skill_path,
                }),
            ),
        ];
        for (method, params) in &requests {
            let response = run_method(&daemon, method, params.clone());
            assert!(response.is_ok(), "warm-up response: {response:?}");
        }
        let (release_tx, holder) = hold_default_database_write_lock();

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
                panic!(
                    "request {method} waited for the database authority: {error}"
                )
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
        let daemon = Daemon::new(root.clone());
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

        daemon.shutdown();
        server.join().unwrap().unwrap();
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
        let listed = run_method_ok(&daemon, "skills_list", json!({}));
        let demo_id = listed_skill_id(&listed, "demo");

        let response = run_method_ok(&daemon, "skills_delete_many", json!({ "skillIds": [demo_id.clone()] }));
        assert!(!skill_dir.exists());
        assert_eq!(response["deleted"], json!([demo_id]));
        assert!(response["skills"].is_null());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rule_file_delete_many_refreshes_the_projection() {
        let _test_lock = TEST_LOCK.lock().unwrap();
        let root = temp_workspace();
        let rule_path = root.join("AGENTS.md");
        fs::write(&rule_path, "delete me").expect("write rule");
        let daemon = Daemon::new(root.clone());

        let _listed = run_method_ok(&daemon, "rules_list", json!({}));
        let response = run_method_ok(
            &daemon,
            "rule_file_delete_many",
            json!({ "paths": [rule_path] }),
        );

        assert!(!rule_path.exists());
        let rule_path_text = rule_path.to_string_lossy();
        assert_eq!(
            response["deleted"],
            json!([rule_path_text.as_ref()])
        );
        let listed_after = run_method_ok(&daemon, "rules_list", json!({}));
        assert!(!listed_after
            .as_array()
            .expect("rules response should be an array")
            .iter()
            .any(|rule| rule["path"].as_str() == Some(rule_path_text.as_ref())));
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
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));
        let preview = run_method_ok(&daemon, "skills_distribute", json!({
                "sourcePaths": [first_source, second_source],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "dryRun": true
        }));
        assert_eq!(preview["plans"].as_array().unwrap().len(), 2);
        let preview_id = preview["previewId"].as_str().unwrap();

        let applied = run_method_ok(&daemon, "skills_distribute", json!({
                "sourcePaths": [root.join(".agents/skills/demo"), second],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "previewId": preview_id,
                "dryRun": false
        }));
        assert_eq!(applied["results"].as_array().unwrap().len(), 2);
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
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));

        let applied = run_method_ok(&daemon, "skills_distribute", json!({
                "sourcePaths": [source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "dryRun": false
        }));

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
        let daemon = Daemon::new(root.clone());
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));

        let distributed = run_method_ok(&daemon, "skills_distribute", json!({
                "sourcePaths": [source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "copy",
                "dryRun": false
        }));
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
                        paths.iter().any(|path| path["path"].as_str() == Some(target_path.as_ref()))
                    })
                })
            })
            .and_then(|skill| skill["id"].as_str())
            .expect("distribution should expose the target installation id")
            .to_string();
        let removed = run_method_ok(&daemon, "skills_remove_locations", json!({
                "skillIds": [target_id],
                "targets": ["claude-code"],
                "scope": "project"
        }));
        assert!(source.exists());
        assert!(!target.exists(), "remove response: {removed}");
        assert_eq!(
            removed["plan"]["targets"]
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
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));

        let preview = run_method_ok(&daemon, "skills_distribute", json!({
                "sourcePaths": [source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "symlink",
                "dryRun": true
        }));
        let preview_id = preview["previewId"].as_str().unwrap();

        let applied = run_method_ok(&daemon, "skills_distribute", json!({
                "sourcePaths": [source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "previewId": preview_id,
                "dryRun": false
        }));
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
        let daemon = Daemon::new(root.clone());
        let _listed = run_method_ok(&daemon, "skills_list", json!({}));

        let preview = run_method_ok(&daemon, "skills_distribute", json!({
                "sourcePaths": [same_path.clone(), moved_source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "dryRun": true
        }));
        let plans = preview["plans"].as_array().unwrap();
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0]["status"], "already-at-destination");
        assert_eq!(plans[1]["status"], "ready");
        let preview_id = preview["previewId"].as_str().unwrap();

        let _applied = run_method_ok(&daemon, "skills_distribute", json!({
                "sourcePaths": [same_path.clone(), moved_source.clone()],
                "target": "claude-code",
                "scope": "project",
                "mode": "move",
                "previewId": preview_id,
                "dryRun": false
        }));
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
        let listed = run_method_ok(&daemon, "skills_list", json!({}));
        let demo_id = listed_skill_id(&listed, "demo");

        let _visibility = run_method_ok(
            &daemon,
            "skills_set",
            json!({ "skillIds": [demo_id], "visibility": "manual" }),
        );
        assert!(
            fs::read_to_string(skill_dir.join("SKILL.md"))
                .unwrap()
                .contains("visibility: manual")
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
        let _renamed = run_method_ok(&daemon, "skill_path_rename", json!({
                "skillId": demo_id.clone(),
                "fromRelativePath": "references/notes.md",
                "toRelativePath": "references/renamed.md"
        }));
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
                    server_path: Vec::new(),
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
                    server_path: Vec::new(),
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
        let daemon = Daemon::new(temp_workspace());
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
    fn session_watch_retries_database_lock_but_not_other_errors() {
        let mut attempts = 0;
        let result = retry_session_watch_update(|| {
            attempts += 1;
            if attempts < 3 {
                Err(DaemonError::new("CORE_ERROR", "database is locked"))
            } else {
                Ok(42)
            }
        })
        .unwrap();
        assert_eq!(result, 42);
        assert_eq!(attempts, 3);

        let mut non_lock_attempts = 0;
        let error = retry_session_watch_update(|| {
            non_lock_attempts += 1;
            Err::<(), _>(DaemonError::new("CORE_ERROR", "invalid session data"))
        })
        .expect_err("non-lock errors should fail immediately");
        assert_eq!(error.message, "invalid session data");
        assert_eq!(non_lock_attempts, 1);
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
        let daemon = Daemon::new(PathBuf::from("/tmp/tendi-runtime-contract-test"));

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
    }
}
