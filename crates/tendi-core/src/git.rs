use std::{
    collections::{HashMap, HashSet},
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

pub(crate) const LOCAL_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const NETWORK_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

static NEVER_CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum CommandFailure {
    Cancelled,
    TimedOut,
    Spawn,
    Wait,
}

#[derive(Debug)]
pub(crate) struct CommandError {
    pub(crate) kind: CommandFailure,
    program: OsString,
    args: Vec<OsString>,
    cwd: PathBuf,
    timeout: Duration,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let command = std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(
                self.args
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned()),
            )
            .collect::<Vec<_>>()
            .join(" ");
        let detail = match self.kind {
            CommandFailure::Cancelled => "cancelled".to_string(),
            CommandFailure::TimedOut => format!("timed out after {}ms", self.timeout.as_millis()),
            CommandFailure::Spawn => "failed to spawn".to_string(),
            CommandFailure::Wait => "failed while waiting".to_string(),
        };
        write!(formatter, "{command} in {} {detail}", self.cwd.display())
    }
}

impl std::error::Error for CommandError {}

pub(crate) fn never_cancelled() -> &'static AtomicBool {
    &NEVER_CANCELLED
}

pub(crate) fn run_git<I, S>(
    cwd: &Path,
    args: I,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect::<Vec<_>>();
    let mutating_command = args
        .first()
        .is_some_and(|command| is_local_repository_mutation(command));
    let output = run_program("git", cwd, &args, timeout, cancelled)?;
    if output.status.success() && mutating_command {
        let _ = invalidate_local_repository_snapshot(cwd, None);
    }
    Ok(output)
}

fn is_local_repository_mutation(command: &OsStr) -> bool {
    matches!(
        command.to_str(),
        Some(
            "fetch"
                | "pull"
                | "merge"
                | "reset"
                | "checkout"
                | "switch"
                | "commit"
                | "rebase"
                | "cherry-pick"
                | "clone"
        )
    )
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct GitRepositorySnapshot {
    pub(crate) workspace: PathBuf,
    pub(crate) repo_root: Option<PathBuf>,
    pub(crate) git_dir: Option<PathBuf>,
    pub(crate) common_dir: Option<PathBuf>,
    pub(crate) remote_url: Option<String>,
    pub(crate) head_oid: Option<String>,
    pub(crate) local_checked_at: SystemTime,
    pub(crate) error: Option<String>,
}

pub(crate) fn logical_repository_root(snapshot: &GitRepositorySnapshot) -> Option<PathBuf> {
    match (&snapshot.repo_root, &snapshot.git_dir, &snapshot.common_dir) {
        (Some(_), Some(git_dir), Some(common_dir))
            if git_dir != common_dir
                && common_dir.file_name().and_then(|name| name.to_str()) == Some(".git") =>
        {
            common_dir.parent().map(Path::to_path_buf)
        }
        (repo_root, _, _) => repo_root.clone(),
    }
}

#[derive(Debug)]
pub(crate) enum GitRepositorySnapshotError {
    Command(CommandError),
    QueryFailed {
        operation: &'static str,
        detail: String,
    },
    UnsupportedCommand {
        command: String,
    },
    CachePoisoned,
}

impl std::fmt::Display for GitRepositorySnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command(error) => error.fmt(formatter),
            Self::QueryFailed { operation, detail } => {
                write!(formatter, "git {operation} failed: {detail}")
            }
            Self::UnsupportedCommand { command } => {
                write!(
                    formatter,
                    "git command is not a local snapshot query: {command}"
                )
            }
            Self::CachePoisoned => formatter.write_str("git repository snapshot cache is poisoned"),
        }
    }
}

impl std::error::Error for GitRepositorySnapshotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Command(error) => Some(error),
            Self::QueryFailed { .. } | Self::UnsupportedCommand { .. } | Self::CachePoisoned => {
                None
            }
        }
    }
}

const DEFAULT_REPOSITORY_SNAPSHOT_TTL: Duration = Duration::from_secs(2);

static PROCESS_REPOSITORY_SNAPSHOT_CACHE: OnceLock<GitRepositorySnapshotCache> = OnceLock::new();

pub(crate) fn local_repository_snapshot(
    workspace: &Path,
    cancelled: &AtomicBool,
) -> Result<GitRepositorySnapshot, GitRepositorySnapshotError> {
    process_repository_snapshot_cache().metadata_snapshot(workspace, cancelled)
}

/// Git worktrees have distinct `.git` pointer files but share refs and objects.
/// Query ownership fresh: a cached repository snapshot is not a lock identity.
pub(crate) fn mutation_resource_paths(repository: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if !repository.join(".git").exists() && !repository.join("HEAD").is_file() {
        // A clone destination has no Git metadata yet. Its owner is the exact
        // destination directory, including the metadata that clone will create.
        return Ok(vec![repository.to_path_buf()]);
    }
    let mut resources = vec![repository.join(".git")];
    for (argument, operation) in [
        ("--git-dir", "git-dir"),
        ("--git-common-dir", "git-common-dir"),
    ] {
        let output = query_required(
            repository,
            &["rev-parse", argument],
            never_cancelled(),
            operation,
        )?;
        resources.push(resolve_git_path(repository, &output, operation)?);
    }
    resources.sort();
    resources.dedup();
    Ok(resources)
}

pub(crate) fn invalidate_local_repository_snapshot(
    workspace: &Path,
    repo_root: Option<&Path>,
) -> Result<(), GitRepositorySnapshotError> {
    process_repository_snapshot_cache().invalidate(workspace, repo_root)
}

fn process_repository_snapshot_cache() -> &'static GitRepositorySnapshotCache {
    PROCESS_REPOSITORY_SNAPSHOT_CACHE
        .get_or_init(|| GitRepositorySnapshotCache::new(DEFAULT_REPOSITORY_SNAPSHOT_TTL))
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
struct GitRepositoryCacheKey {
    workspace: PathBuf,
    repo_root: PathBuf,
}

#[derive(Debug, Clone)]
struct CachedGitRepositorySnapshot {
    snapshot: GitRepositorySnapshot,
    cached_at: Instant,
}

pub(crate) struct GitRepositorySnapshotCache {
    ttl: Duration,
    entries: Mutex<HashMap<GitRepositoryCacheKey, CachedGitRepositorySnapshot>>,
}

impl GitRepositorySnapshotCache {
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn metadata_snapshot(
        &self,
        workspace: &Path,
        cancelled: &AtomicBool,
    ) -> Result<GitRepositorySnapshot, GitRepositorySnapshotError> {
        let workspace = normalize_path(workspace);
        if cancelled.load(Ordering::Acquire) {
            return Err(GitRepositorySnapshotError::Command(CommandError {
                kind: CommandFailure::Cancelled,
                program: OsString::from("git"),
                args: Vec::new(),
                cwd: workspace,
                timeout: LOCAL_COMMAND_TIMEOUT,
            }));
        }

        if let Some(snapshot) = self.fresh_snapshot(&workspace)? {
            return Ok(snapshot);
        }

        let snapshot = collect_repository_snapshot(&workspace, cancelled)?;
        if snapshot.error.is_none() {
            self.store_snapshot(snapshot.clone())?;
        }
        Ok(snapshot)
    }

    pub(crate) fn invalidate(
        &self,
        workspace: &Path,
        repo_root: Option<&Path>,
    ) -> Result<(), GitRepositorySnapshotError> {
        let workspace = normalize_path(workspace);
        let repo_root = repo_root.map(normalize_path);
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| GitRepositorySnapshotError::CachePoisoned)?;
        entries.retain(|key, _| {
            if key.workspace != workspace {
                return true;
            }
            repo_root
                .as_ref()
                .is_some_and(|repo_root| key.repo_root != *repo_root)
        });
        Ok(())
    }

    fn fresh_snapshot(
        &self,
        workspace: &Path,
    ) -> Result<Option<GitRepositorySnapshot>, GitRepositorySnapshotError> {
        if self.ttl.is_zero() {
            return Ok(None);
        }
        let entries = self
            .entries
            .lock()
            .map_err(|_| GitRepositorySnapshotError::CachePoisoned)?;
        Ok(entries
            .iter()
            .find(|(key, entry)| key.workspace == workspace && entry.cached_at.elapsed() < self.ttl)
            .map(|(_, entry)| entry.snapshot.clone()))
    }

    fn store_snapshot(
        &self,
        snapshot: GitRepositorySnapshot,
    ) -> Result<(), GitRepositorySnapshotError> {
        let Some(repo_root) = snapshot.repo_root.as_ref() else {
            return Ok(());
        };
        let key = GitRepositoryCacheKey {
            workspace: snapshot.workspace.clone(),
            repo_root: repo_root.clone(),
        };
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| GitRepositorySnapshotError::CachePoisoned)?;
        entries.retain(|existing_key, _| existing_key.workspace != key.workspace);
        entries.insert(
            key,
            CachedGitRepositorySnapshot {
                snapshot,
                cached_at: Instant::now(),
            },
        );
        Ok(())
    }
}

fn collect_repository_snapshot(
    workspace: &Path,
    cancelled: &AtomicBool,
) -> Result<GitRepositorySnapshot, GitRepositorySnapshotError> {
    let root_output = run_local_git_query(workspace, &["rev-parse", "--show-toplevel"], cancelled)?;
    if !root_output.status.success() {
        return Ok(GitRepositorySnapshot {
            workspace: workspace.to_path_buf(),
            repo_root: None,
            git_dir: None,
            common_dir: None,
            remote_url: None,
            head_oid: None,
            local_checked_at: SystemTime::now(),
            error: Some(git_output_detail(&root_output)),
        });
    }

    let repo_root = resolve_git_path(workspace, &root_output, "show-toplevel")?;
    let git_dir = resolve_git_path(
        workspace,
        &query_required(workspace, &["rev-parse", "--git-dir"], cancelled, "git-dir")?,
        "git-dir",
    )?;
    let common_dir = resolve_git_path(
        workspace,
        &query_required(
            workspace,
            &["rev-parse", "--git-common-dir"],
            cancelled,
            "git-common-dir",
        )?,
        "git-common-dir",
    )?;

    let remote_url = resolve_remote_url(
        workspace,
        query_optional(
            workspace,
            &["config", "--get", "remote.origin.url"],
            cancelled,
        )?,
        cancelled,
    )?;
    let head_oid = query_optional(workspace, &["rev-parse", "HEAD"], cancelled)?;

    Ok(GitRepositorySnapshot {
        workspace: workspace.to_path_buf(),
        repo_root: Some(repo_root),
        git_dir: Some(git_dir),
        common_dir: Some(common_dir),
        remote_url,
        head_oid,
        local_checked_at: SystemTime::now(),
        error: None,
    })
}

fn query_required(
    cwd: &Path,
    args: &[&str],
    cancelled: &AtomicBool,
    operation: &'static str,
) -> Result<Output, GitRepositorySnapshotError> {
    let output = run_local_git_query(cwd, args, cancelled)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(GitRepositorySnapshotError::QueryFailed {
            operation,
            detail: git_output_detail(&output),
        })
    }
}

fn query_optional(
    cwd: &Path,
    args: &[&str],
    cancelled: &AtomicBool,
) -> Result<Option<String>, GitRepositorySnapshotError> {
    let output = run_local_git_query(cwd, args, cancelled)?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty()))
}

fn resolve_remote_url(
    workspace: &Path,
    remote_url: Option<String>,
    cancelled: &AtomicBool,
) -> Result<Option<String>, GitRepositorySnapshotError> {
    let Some(mut remote_url) = remote_url else {
        return Ok(None);
    };
    let mut repository = workspace.to_path_buf();
    let mut visited_paths = HashSet::new();

    loop {
        let Some(remote_path) = local_remote_path(&repository, &remote_url) else {
            return Ok(Some(remote_url));
        };
        let remote_path = normalize_path(&remote_path);
        if !remote_path.is_dir() || !visited_paths.insert(remote_path.clone()) {
            return Ok(None);
        }
        remote_url = match query_optional(
            &remote_path,
            &["config", "--get", "remote.origin.url"],
            cancelled,
        )? {
            Some(remote_url) => remote_url,
            None => return Ok(None),
        };
        repository = remote_path;
    }
}

fn local_remote_path(workspace: &Path, remote_url: &str) -> Option<PathBuf> {
    let path = Path::new(remote_url);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    if matches!(
        path.components().next(),
        Some(std::path::Component::CurDir | std::path::Component::ParentDir)
    ) {
        return Some(workspace.join(path));
    }
    None
}

fn resolve_git_path(
    workspace: &Path,
    output: &Output,
    operation: &'static str,
) -> Result<PathBuf, GitRepositorySnapshotError> {
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if value.is_empty() {
        return Err(GitRepositorySnapshotError::QueryFailed {
            operation,
            detail: "git returned an empty path".to_string(),
        });
    }
    let path = PathBuf::from(value);
    let path = if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    };
    Ok(normalize_path(&path))
}

fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(path) = fs::canonicalize(path) {
        return path;
    }
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current_dir| current_dir.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn run_local_git_query(
    cwd: &Path,
    args: &[&str],
    cancelled: &AtomicBool,
) -> Result<Output, GitRepositorySnapshotError> {
    #[cfg(test)]
    record_local_git_query(args);
    if !is_local_git_query(args) {
        return Err(GitRepositorySnapshotError::UnsupportedCommand {
            command: args.join(" "),
        });
    }
    run_git(cwd, args, LOCAL_COMMAND_TIMEOUT, cancelled)
        .map_err(GitRepositorySnapshotError::Command)
}

#[cfg(test)]
thread_local! {
    static LOCAL_GIT_QUERY_TRACE: std::cell::RefCell<Vec<Vec<String>>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

#[cfg(test)]
fn record_local_git_query(args: &[&str]) {
    LOCAL_GIT_QUERY_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .push(args.iter().map(|arg| (*arg).to_string()).collect());
    });
}

#[cfg(test)]
fn reset_local_git_query_trace() {
    LOCAL_GIT_QUERY_TRACE.with(|trace| trace.borrow_mut().clear());
}

#[cfg(test)]
fn local_git_query_trace() -> Vec<Vec<String>> {
    LOCAL_GIT_QUERY_TRACE.with(|trace| trace.borrow().clone())
}

fn is_local_git_query(args: &[&str]) -> bool {
    matches!(
        args,
        ["rev-parse", "--show-toplevel"]
            | ["rev-parse", "--git-dir"]
            | ["rev-parse", "--git-common-dir"]
            | ["rev-parse", "HEAD"]
            | ["config", "--get", "remote.origin.url"]
            | ["status", "--porcelain", "--untracked-files=normal"]
    )
}

fn git_output_detail(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !stdout.is_empty() {
        return stdout;
    }
    output
        .status
        .code()
        .map(|code| format!("git exited with status {code}"))
        .unwrap_or_else(|| "git exited without a status code".to_string())
}

pub(crate) fn run_program<I, S, P>(
    program: P,
    cwd: &Path,
    args: I,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
    P: AsRef<OsStr>,
{
    let program = program.as_ref().to_os_string();
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect::<Vec<_>>();
    if cancelled.load(Ordering::Acquire) {
        return Err(CommandError {
            kind: CommandFailure::Cancelled,
            program,
            args,
            cwd: cwd.to_path_buf(),
            timeout,
        });
    }

    let mut command = Command::new(&program);
    command
        .args(&args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|_| CommandError {
        kind: CommandFailure::Spawn,
        program: program.clone(),
        args: args.clone(),
        cwd: cwd.to_path_buf(),
        timeout,
    })?;
    let stdout = spawn_reader(child.stdout.take());
    let stderr = spawn_reader(child.stderr.take());
    let started = Instant::now();

    let status = loop {
        if cancelled.load(Ordering::Acquire) {
            terminate(&mut child);
            let _ = child.wait();
            return Err(CommandError {
                kind: CommandFailure::Cancelled,
                program,
                args,
                cwd: cwd.to_path_buf(),
                timeout,
            });
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                terminate(&mut child);
                let _ = child.wait();
                return Err(CommandError {
                    kind: CommandFailure::TimedOut,
                    program,
                    args,
                    cwd: cwd.to_path_buf(),
                    timeout,
                });
            }
            // Keep cancellation and timeout checks responsive without adding a
            // full 20ms of latency to every short-lived local git command.
            Ok(None) => thread::sleep(Duration::from_millis(2)),
            Err(_) => {
                terminate(&mut child);
                let _ = child.wait();
                return Err(CommandError {
                    kind: CommandFailure::Wait,
                    program,
                    args,
                    cwd: cwd.to_path_buf(),
                    timeout,
                });
            }
        }
    };

    Ok(Output {
        status,
        stdout: join_reader(stdout),
        stderr: join_reader(stderr),
    })
}

fn spawn_reader(reader: Option<impl Read + Send + 'static>) -> Option<thread::JoinHandle<Vec<u8>>> {
    reader.map(|mut reader| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = reader.read_to_end(&mut bytes);
            bytes
        })
    })
}

fn join_reader(reader: Option<thread::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    reader
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &format!("-{}", child.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
