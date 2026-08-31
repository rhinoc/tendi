#[path = "cli_install.rs"]
mod cli_registration;
mod terminals;

use std::{
    collections::HashMap,
    env,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use tauri::{
    ActivationPolicy, Emitter, Manager, RunEvent,
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder},
};
#[cfg(target_os = "macos")]
use tauri::{LogicalPosition, TitleBarStyle};
use tauri_plugin_updater::UpdaterExt;
use tendi_core::AgentKind;
use tendi_core::generated::runtime_contract as runtime_schema;

struct DaemonState {
    daemon: Arc<tendi_daemon::Daemon>,
    subscriptions: Mutex<HashMap<String, Arc<Mutex<tendi_daemon::DaemonEventSubscription>>>>,
    next_subscription_id: AtomicU64,
}

const UPDATE_AVAILABLE_EVENT: &str = "tendi://update-available";

struct UpdateState {
    operation_in_flight: Arc<AtomicBool>,
}

fn runtime_cli_status(
    status: cli_registration::CliInstallStatus,
) -> Result<runtime_schema::CliInstallStatus, String> {
    serde_json::from_value(serde_json::to_value(status).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            operation_in_flight: Arc::new(AtomicBool::new(false)),
        }
    }
}

struct UpdateOperationGuard {
    operation_in_flight: Arc<AtomicBool>,
}

impl Drop for UpdateOperationGuard {
    fn drop(&mut self) {
        self.operation_in_flight.store(false, Ordering::Release);
    }
}

fn begin_update_operation(operation_in_flight: Arc<AtomicBool>) -> Option<UpdateOperationGuard> {
    operation_in_flight
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .ok()
        .map(|_| UpdateOperationGuard {
            operation_in_flight,
        })
}

#[tauri::command]
async fn cli_status() -> Result<runtime_schema::CliInstallStatus, String> {
    let status = tauri::async_runtime::spawn_blocking(cli_registration::status)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    runtime_cli_status(status)
}

#[tauri::command(rename_all = "camelCase")]
fn log_event(level: String, message: String, fields: Option<Value>) -> Result<(), String> {
    tendi_core::logging::log_event(
        &level,
        &message,
        fields.unwrap_or_else(|| Value::Object(Default::default())),
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn app_icon_set(app: tauri::AppHandle, icon: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        if icon.is_empty() {
            return Err("app icon data is empty".to_string());
        }
        app.run_on_main_thread(move || {
            use cocoa::{
                appkit::{NSApp, NSApplication, NSImage},
                base::nil,
                foundation::NSData,
            };

            unsafe {
                let data =
                    NSData::dataWithBytes_length_(nil, icon.as_ptr() as *const _, icon.len() as _);
                let image = NSImage::initWithData_(NSImage::alloc(nil), data);
                if image != nil {
                    NSApp().setApplicationIconImage_(image);
                }
            }
        })
        .map_err(|error| error.to_string())?;
    }

    #[cfg(not(target_os = "macos"))]
    let _ = (app, icon);

    Ok(())
}

#[tauri::command]
async fn cli_install() -> Result<runtime_schema::CliInstallStatus, String> {
    let status = tauri::async_runtime::spawn_blocking(cli_registration::install)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    runtime_cli_status(status)
}

#[tauri::command]
async fn cli_remove() -> Result<runtime_schema::CliInstallStatus, String> {
    let status = tauri::async_runtime::spawn_blocking(cli_registration::remove)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    runtime_cli_status(status)
}

async fn check_for_updates_inner(
    app: tauri::AppHandle,
    operation_in_flight: Arc<AtomicBool>,
) -> Result<runtime_schema::UpdateCheckResult, String> {
    let Some(_operation) = begin_update_operation(operation_in_flight) else {
        return Ok(runtime_schema::UpdateCheckResult {
            status: "busy".to_string(),
            version: None,
            body: None,
        });
    };
    let updater = app.updater().map_err(|error| error.to_string())?;
    let Some(update) = updater.check().await.map_err(|error| error.to_string())? else {
        return Ok(runtime_schema::UpdateCheckResult {
            status: "up-to-date".to_string(),
            version: None,
            body: None,
        });
    };

    Ok(runtime_schema::UpdateCheckResult {
        status: "available".to_string(),
        version: Some(update.version),
        body: update.body,
    })
}

#[tauri::command]
async fn check_for_updates(
    app: tauri::AppHandle,
    state: tauri::State<'_, UpdateState>,
) -> Result<runtime_schema::UpdateCheckResult, String> {
    check_for_updates_inner(app, state.operation_in_flight.clone()).await
}

#[tauri::command]
async fn install_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, UpdateState>,
) -> Result<runtime_schema::UpdateCheckResult, String> {
    let Some(_operation) = begin_update_operation(state.operation_in_flight.clone()) else {
        return Ok(runtime_schema::UpdateCheckResult {
            status: "busy".to_string(),
            version: None,
            body: None,
        });
    };
    let updater = app.updater().map_err(|error| error.to_string())?;
    let Some(update) = updater.check().await.map_err(|error| error.to_string())? else {
        return Ok(runtime_schema::UpdateCheckResult {
            status: "up-to-date".to_string(),
            version: None,
            body: None,
        });
    };

    let version = update.version.clone();
    let mut downloaded = 0_u64;
    update
        .download_and_install(
            |chunk_length, content_length| {
                downloaded += chunk_length as u64;
                tendi_core::logging::global().info(
                    "desktop update download",
                    serde_json::json!({
                        "downloaded_bytes": downloaded,
                        "content_length": content_length,
                    }),
                );
            },
            || tendi_core::logging::global().info("desktop update download finished", Value::Null),
        )
        .await
        .map_err(|error| error.to_string())?;

    tendi_core::logging::global().info(
        "desktop update installed",
        serde_json::json!({ "version": version }),
    );
    app.restart();
}

fn build_app_menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let check_for_updates =
        MenuItemBuilder::with_id("check_for_updates_menu", "Check for Updates...").build(app)?;
    let app_menu = SubmenuBuilder::new(app, "tendi")
        .item(&PredefinedMenuItem::about(app, None, None)?)
        .separator()
        .item(&check_for_updates)
        .separator()
        .item(&PredefinedMenuItem::quit(app, None)?)
        .build()?;
    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .item(&PredefinedMenuItem::undo(app, None)?)
        .item(&PredefinedMenuItem::redo(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::cut(app, None)?)
        .item(&PredefinedMenuItem::copy(app, None)?)
        .item(&PredefinedMenuItem::paste(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::select_all(app, None)?)
        .build()?;
    let window_menu = SubmenuBuilder::new(app, "Window")
        .item(&PredefinedMenuItem::minimize(app, None)?)
        .item(&PredefinedMenuItem::close_window(app, None)?)
        .build()?;

    MenuBuilder::new(app)
        .item(&app_menu)
        .item(&edit_menu)
        .item(&window_menu)
        .build()
}

#[tauri::command]
async fn daemon_invoke(
    request: serde_json::Value,
    state: tauri::State<'_, DaemonState>,
) -> Result<serde_json::Value, String> {
    let daemon = Arc::clone(&state.daemon);
    tauri::async_runtime::spawn_blocking(move || Ok(daemon.handle_json_rpc(request)))
        .await
        .map_err(|error| format!("daemon request failed: {error}"))?
}

#[tauri::command]
fn daemon_subscribe_events(state: tauri::State<'_, DaemonState>) -> Result<String, String> {
    let id = format!(
        "tauri-{}",
        state.next_subscription_id.fetch_add(1, Ordering::Relaxed) + 1
    );
    let subscription = Arc::new(Mutex::new(state.daemon.subscribe_events()));
    state
        .subscriptions
        .lock()
        .map_err(|_| "daemon event subscriptions are unavailable".to_string())?
        .insert(id.clone(), subscription);
    Ok(id)
}

#[tauri::command(rename_all = "camelCase")]
async fn daemon_next_event(
    subscription_id: String,
    timeout_ms: Option<u64>,
    state: tauri::State<'_, DaemonState>,
) -> Result<Option<tendi_daemon::DaemonEvent>, String> {
    let subscription = state
        .subscriptions
        .lock()
        .map_err(|_| "daemon event subscriptions are unavailable".to_string())?
        .get(&subscription_id)
        .cloned()
        .ok_or_else(|| "daemon event subscription not found".to_string())?;
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(25_000).min(30_000));
    tauri::async_runtime::spawn_blocking(move || {
        let subscription = subscription
            .lock()
            .map_err(|_| "daemon event subscription is unavailable".to_string())?;
        match subscription.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Ok(None),
        }
    })
    .await
    .map_err(|error| format!("daemon event wait failed: {error}"))?
}

#[tauri::command(rename_all = "camelCase")]
fn daemon_unsubscribe_events(
    subscription_id: String,
    state: tauri::State<'_, DaemonState>,
) -> Result<(), String> {
    state
        .subscriptions
        .lock()
        .map_err(|_| "daemon event subscriptions are unavailable".to_string())?
        .remove(&subscription_id);
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
async fn session_resume_target(
    session: runtime_schema::SessionResumeRequest,
) -> Result<runtime_schema::SessionResumeTargetResponse, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let agent = parse_agent_result(&session.agent)?;
        let target =
            tendi_core::sessions::infer_session_resume_target(&PathBuf::from(session.path), agent)
                .unwrap_or("terminal");
        Ok(match target {
            "app" => runtime_schema::SessionResumeTargetResponse::App,
            _ => runtime_schema::SessionResumeTargetResponse::Terminal,
        })
    })
    .await
    .map_err(|error| format!("background task failed: {error}"))?
}

#[tauri::command(rename_all = "camelCase")]
async fn terminal_app_test(
    terminal: String,
) -> Result<runtime_schema::TerminalAppTestResponse, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let terminal = terminals::resolve_terminal(&terminal);
        let app_name = terminals::terminal_application_name(&terminal);
        terminals::open_terminal_application(&app_name)?;
        Ok(app_name)
    })
    .await
    .map_err(|error| format!("background task failed: {error}"))?
}

#[tauri::command(rename_all = "camelCase")]
async fn editor_app_test(editor: String) -> Result<runtime_schema::EditorAppTestResponse, String> {
    tauri::async_runtime::spawn_blocking(move || {
        test_editor_command(&editor)?;
        Ok(true)
    })
    .await
    .map_err(|error| format!("background task failed: {error}"))?
}

#[tauri::command(rename_all = "camelCase")]
async fn session_resume_in_terminal(
    session: runtime_schema::SessionResumeRequest,
) -> Result<runtime_schema::SessionResumeResponse, String> {
    tauri::async_runtime::spawn_blocking(move || {
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
        if let Some(writer) =
            tendi_core::active_session_writer(&record).map_err(|error| format!("{error:#}"))?
        {
            return Ok(runtime_schema::SessionResumeResponse {
                status: "activeWriter".to_string(),
                lock_path: Some(writer.lock_path.display().to_string()),
                agent: None,
                terminal: None,
                command_line: None,
            });
        }
        let mut plan =
            tendi_core::plan_session_resume(&record).map_err(|error| format!("{error:#}"))?;
        let store =
            tendi_core::storage::Store::open_default().map_err(|error| format!("{error:#}"))?;
        let settings = store.app_settings().map_err(|error| format!("{error:#}"))?;
        if let Some(profile) = tendi_core::config_profile_key(plan.agent)
            .and_then(|agent| settings.config_profiles.get(agent))
            .map(String::as_str)
        {
            tendi_core::apply_session_config_profile(plan.agent, &mut plan.command, profile)
                .map_err(|error| format!("{error:#}"))?;
        }
        let terminal = terminals::resolve_terminal(&settings.terminal);
        let command_line = terminals::launch_command_in_terminal(&plan.command, &terminal)?;
        Ok(runtime_schema::SessionResumeResponse {
            status: "launched".to_string(),
            lock_path: None,
            agent: Some(plan.agent.label().to_string()),
            terminal: Some(terminal),
            command_line: Some(command_line),
        })
    })
    .await
    .map_err(|error| format!("background task failed: {error}"))?
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
        .map_err(|error| format!("failed to load editor setting: {error:#}"))?;
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
        .map_err(|error| format!("failed to reveal path: {error}"))?;

    #[cfg(target_os = "windows")]
    let status = Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .status()
        .map_err(|error| format!("failed to reveal path: {error}"))?;

    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open")
        .arg(if path.is_dir() {
            path.as_path()
        } else {
            path.parent().unwrap_or(path.as_path())
        })
        .status()
        .map_err(|error| format!("failed to reveal path: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("reveal command exited with {status}"))
    }
}

#[tauri::command]
fn logs_export() -> Result<runtime_schema::LogsExportResponse, String> {
    match export_logs_archive() {
        Ok((path, file_count)) => {
            tendi_core::logging::global().info(
                "logs exported",
                serde_json::json!({
                    "path": path,
                    "file_count": file_count,
                }),
            );
            Ok(path.to_string_lossy().into_owned())
        }
        Err(error) => {
            tendi_core::logging::global()
                .error("logs export failed", serde_json::json!({"error": error}));
            Err(error)
        }
    }
}

fn export_logs_archive() -> Result<(PathBuf, usize), String> {
    let active_path = tendi_core::logging::Logger::path()
        .map_err(|error| format!("could not resolve log path: {error:#}"))?;
    let log_files = log_export_files(&active_path)?;
    if log_files.is_empty() {
        return Err(format!("no log files found in {}", active_path.display()));
    }

    let download_dir =
        dirs::download_dir().ok_or_else(|| "could not resolve Downloads directory".to_string())?;
    fs::create_dir_all(&download_dir)
        .map_err(|error| format!("could not create Downloads directory: {error}"))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("could not create export timestamp: {error}"))?
        .as_secs();
    let archive_path = download_dir.join(format!("tendi-logs-{timestamp}.zip"));
    let archive_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&archive_path)
        .map_err(|error| format!("could not create {}: {error}", archive_path.display()))?;
    let mut archive = zip::ZipWriter::new(archive_file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let file_count = log_files.len();

    for path in log_files {
        let name = path
            .file_name()
            .ok_or_else(|| format!("log path has no file name: {}", path.display()))?
            .to_string_lossy()
            .into_owned();
        archive
            .start_file(name, options)
            .map_err(|error| format!("could not add log file to archive: {error}"))?;
        let mut input = File::open(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        io::copy(&mut input, &mut archive)
            .map_err(|error| format!("could not copy {} to archive: {error}", path.display()))?;
    }

    archive
        .finish()
        .map_err(|error| format!("could not finish log archive: {error}"))?;
    Ok((archive_path, file_count))
}

fn log_export_files(active_path: &Path) -> Result<Vec<PathBuf>, String> {
    let directory = active_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let active_name = active_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("log path has no valid file name: {}", active_path.display()))?;
    let rotation = active_name
        .rsplit_once('.')
        .map(|(prefix, extension)| (format!("{prefix}."), format!(".{extension}")));
    let entries = fs::read_dir(directory).map_err(|error| {
        format!(
            "could not read log directory {}: {error}",
            directory.display()
        )
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "could not read log directory entry in {}: {error}",
                directory.display()
            )
        })?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_active = name == active_name;
        let is_rotated = rotation.as_ref().map_or(false, |(rotated_prefix, suffix)| {
            name.starts_with(rotated_prefix)
                && name.ends_with(suffix)
                && is_rotated_log_name(&name, rotated_prefix, suffix)
        });
        if !(is_active || is_rotated) {
            continue;
        }
        if !entry
            .metadata()
            .map_err(|error| format!("could not inspect {}: {error}", path.display()))?
            .is_file()
        {
            continue;
        }
        paths.push(path);
    }
    paths.sort();
    Ok(paths)
}

fn is_rotated_log_name(name: &str, rotated_prefix: &str, suffix: &str) -> bool {
    let Some(value) = name
        .strip_prefix(rotated_prefix)
        .and_then(|value| value.strip_suffix(suffix))
    else {
        return false;
    };
    let mut parts = value.split('.');
    let Some(day) = parts.next() else {
        return false;
    };
    if day.len() != 10
        || !day.chars().enumerate().all(|(index, character)| {
            character.is_ascii_digit() || (index == 4 || index == 7) && character == '-'
        })
    {
        return false;
    }
    match parts.next() {
        None => true,
        Some(index) => index.parse::<usize>().is_ok() && parts.next().is_none(),
    }
}

#[tauri::command(rename_all = "camelCase")]
fn open_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    let is_web_url = trimmed.starts_with("http://") || trimmed.starts_with("https://");
    let is_agent_session_url = tendi_core::accepts_session_app_url(trimmed);
    if !is_web_url && !is_agent_session_url {
        return Err(format!("unsupported url: {trimmed}"));
    }

    open_external_url(trimmed)
}

fn open_external_url(trimmed: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = Command::new("open")
        .arg(trimmed)
        .status()
        .map_err(|error| format!("failed to open url: {error}"))?;

    #[cfg(target_os = "windows")]
    let status = Command::new("cmd")
        .args(["/C", "start", "", trimmed])
        .status()
        .map_err(|error| format!("failed to open url: {error}"))?;

    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open")
        .arg(trimmed)
        .status()
        .map_err(|error| format!("failed to open url: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("open url command exited with {status}"))
    }
}

fn build_main_window(app: &tauri::AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    // Keep in sync with TRAFFIC_LIGHT_* in apps/desktop/src/lib/constants.ts.
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
    if let Err(error) = tendi_core::logging::init("tendi-desktop") {
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "configure tendi logger: {error:#}");
    }
    tendi_core::logging::global().info("desktop process starting", Value::Null);
    let mut app = tauri::Builder::default()
        .manage(UpdateState::default())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            app.set_activation_policy(ActivationPolicy::Regular);
            let cwd = active_cwd().map_err(std::io::Error::other)?;
            app.manage(DaemonState {
                daemon: Arc::new(tendi_daemon::Daemon::new(cwd)),
                subscriptions: Mutex::new(HashMap::new()),
                next_subscription_id: AtomicU64::new(0),
            });
            app.set_menu(build_app_menu(app.handle())?)?;
            let window = build_main_window(app.handle())?;
            let _ = window.center();
            let _ = window.show();
            let _ = window.set_focus();
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id() == "check_for_updates_menu" {
                let app = app.clone();
                let operation_in_flight = app.state::<UpdateState>().operation_in_flight.clone();
                tauri::async_runtime::spawn(async move {
                    match check_for_updates_inner(app.clone(), operation_in_flight).await {
                        Ok(result) if result.status == "available" => {
                            let _ = app.emit(UPDATE_AVAILABLE_EVENT, result);
                        }
                        Ok(_) => {}
                        Err(error) => {
                            tendi_core::logging::global().error(
                                "desktop update check failed",
                                serde_json::json!({ "error": error }),
                            );
                        }
                    }
                });
            }
        })
        .invoke_handler(include!("generated/tauri_handler.rs"))
        .build(tauri::generate_context!())
        .expect("error while building tendi desktop");
    app.set_activation_policy(ActivationPolicy::Regular);
    app.run(|app, event| match event {
        RunEvent::Ready => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                activate_native_window(&window);
                let _ = window.set_focus();
            }
        }
        RunEvent::Exit => {
            if let Some(state) = app.try_state::<DaemonState>() {
                state.daemon.shutdown();
            }
        }
        _ => {}
    });
}

fn parse_agent_result(value: &str) -> Result<AgentKind, String> {
    tendi_core::parse_agent(value).map_err(|error| format!("{error:#}"))
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
        _ => env::current_dir().map_err(|error| error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::log_export_files;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn log_export_files_includes_active_and_valid_rotated_logs() {
        let directory = temp_directory();
        for name in [
            "tendi.log",
            "tendi.2026-08-17.log",
            "tendi.2026-08-18.1.log",
            "tendi.invalid.log",
            "other.log",
        ] {
            fs::write(directory.join(name), name).unwrap();
        }

        let files = log_export_files(&directory.join("tendi.log")).unwrap();
        let names = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "tendi.2026-08-17.log",
                "tendi.2026-08-18.1.log",
                "tendi.log",
            ]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    fn temp_directory() -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("tendi-log-export-test-{timestamp}"));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
