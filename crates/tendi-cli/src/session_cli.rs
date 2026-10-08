use super::*;
use clap::Args;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use tendi_core::storage::{SessionRecallOptions, SessionRecallRole, SessionRecallSort};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum RoleArg {
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum SortArg {
    #[default]
    Relevance,
    TimeAsc,
    TimeDesc,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Match the recorded cwd or its descendants; repeat for historical paths.
    #[arg(long)]
    cwd: Vec<PathBuf>,
    #[arg(long, requires = "cwd")]
    cwd_exact: bool,
    /// Inclusive session start: YYYY-MM-DD (UTC) or RFC3339.
    #[arg(long)]
    since: Option<String>,
    /// Exclusive session start: YYYY-MM-DD (UTC) or RFC3339.
    #[arg(long)]
    until: Option<String>,
    #[arg(long)]
    agent: Option<AgentArg>,
    #[arg(long, value_enum)]
    role: Option<RoleArg>,
    /// Match the whole query as one literal phrase.
    #[arg(long)]
    phrase: bool,
    #[arg(long, value_enum, default_value = "relevance")]
    sort: SortArg,
    #[arg(long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 0)]
    offset: usize,
    /// Include full session metadata in JSON.
    #[arg(long, requires = "json")]
    full: bool,
    #[arg(long, value_delimiter = ',', requires = "json", conflicts_with = "full",
        value_parser = ["id", "agent", "title", "project", "path", "started_at", "score", "snippet", "role", "record_order"])]
    fields: Vec<String>,
    /// Explicitly scan and index before searching.
    #[arg(long)]
    refresh: bool,
}

#[derive(Debug, Args)]
pub struct TranscriptArgs {
    /// Resolve a previously indexed session; provider is inferred.
    #[arg(long)]
    session: Option<String>,
    /// Return messages containing all query words.
    #[arg(long = "match")]
    query: Option<String>,
    /// Restrict selected messages; nearby context may have other roles.
    #[arg(long = "role", value_delimiter = ',', default_value = "user,assistant", value_parser = ["user", "assistant", "tool", "context", "model_config"])]
    roles: Vec<String>,
    /// Number of matching messages (or page items with --cursor).
    #[arg(long, default_value_t = 20)]
    limit: usize,
    /// Resume at this normalized transcript item index.
    #[arg(long, default_value_t = 0)]
    offset: usize,
    /// Include this many neighboring items on either side of each match.
    #[arg(long, default_value_t = 0, requires = "query")]
    around: usize,
    /// Read a provider page, starting at this opaque cursor.
    #[arg(long, conflicts_with_all = ["query", "roles", "offset", "around", "all"])]
    cursor: Option<String>,
    /// Read the initial provider page and return its continuation cursor.
    #[arg(long, conflicts_with_all = ["query", "roles", "offset", "around", "all"])]
    page: bool,
    /// Read the complete transcript instead of a bounded excerpt.
    #[arg(long, conflicts_with_all = ["query", "roles", "offset", "around", "cursor"])]
    all: bool,
}

struct Progress {
    stopped: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Progress {
    fn start(stage: &'static str) -> Self {
        eprintln!("tendi: {stage}");
        Self::delayed(stage)
    }

    fn delayed(stage: &'static str) -> Self {
        let stopped = Arc::new(AtomicBool::new(false));
        let flag = stopped.clone();
        let thread = thread::spawn(move || {
            let start = Instant::now();
            while !flag.load(Ordering::Relaxed) {
                thread::park_timeout(Duration::from_secs(2));
                if !flag.load(Ordering::Relaxed) {
                    eprintln!("tendi: {stage} ({}s)", start.elapsed().as_secs());
                }
            }
        });
        Self {
            stopped,
            thread: Some(thread),
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

pub fn refresh(cwd: &Path) -> Result<serde_json::Value> {
    let started = Instant::now();
    let store = {
        let _progress = Progress::start("opening session store");
        tendi_core::storage::Store::open_default()?
    };
    let open_ms = started.elapsed().as_millis();
    let scope = workspace_scope_key(cwd)?;
    let phase = Instant::now();
    let cache = store.shared_session_scan_cache()?;
    let cache_ms = phase.elapsed().as_millis();
    let phase = Instant::now();
    let report = {
        let _progress = Progress::start("scanning provider sources with shared file cache");
        tendi_core::sessions::scan_sessions_with_additional_roots_cached(cwd, &[], &cache)?
    };
    let scan_ms = phase.elapsed().as_millis();
    let count = report.sessions.len();
    let phase = Instant::now();
    let changed = store.persist_session_scan_for_scope(&scope, &report, &cache, unix_now())?;
    let persist_ms = phase.elapsed().as_millis();
    let phase = Instant::now();
    {
        let _progress = Progress::start("publishing search index");
        refresh_session_search(&store, &scope)?;
    }
    let publish_ms = phase.elapsed().as_millis();
    let phase = Instant::now();
    let status = store.session_recall_status()?;
    let status_ms = phase.elapsed().as_millis();
    eprintln!(
        "tendi: indexed {count} sessions in {:.2}s",
        started.elapsed().as_secs_f64()
    );
    Ok(
        serde_json::json!({ "scope": scope.as_str(), "sessions": count, "changed_sessions": changed.len(),
        "elapsed_ms": started.elapsed().as_millis(), "warnings": report.warnings, "status": status,
        "timings_ms": {"open":open_ms,"cache":cache_ms,"scan":scan_ms,
            "persist":persist_ms,"publish":publish_ms,"status":status_ms} }),
    )
}

pub fn search(cwd: &Path, query: String, args: SearchArgs, json: bool) -> Result<()> {
    let options = SessionRecallOptions {
        query,
        cwd: args.cwd,
        exact_cwd: args.cwd_exact,
        since: args.since,
        until: args.until,
        agent: args.agent.map(Into::into),
        role: args.role.map(|role| match role {
            RoleArg::User => SessionRecallRole::User,
            RoleArg::Assistant => SessionRecallRole::Assistant,
        }),
        phrase: args.phrase,
        sort: match args.sort {
            SortArg::Relevance => SessionRecallSort::Relevance,
            SortArg::TimeAsc => SessionRecallSort::TimeAsc,
            SortArg::TimeDesc => SessionRecallSort::TimeDesc,
        },
        limit: args.limit,
        offset: args.offset,
    };
    if args.refresh {
        refresh(cwd)?;
    }
    let store = {
        let _progress = Progress::delayed("opening persistent session index");
        tendi_core::storage::Store::open_default()?
    };
    anyhow::ensure!(
        store.session_recall_status()?.indexed_sessions > 0,
        "no published search index; run tendi sessions refresh (search does not scan automatically)"
    );
    let page = {
        let _progress = Progress::delayed("querying persistent session index");
        store.recall_sessions(&options)?
    };
    if json {
        let mut value = serde_json::to_value(&page)?;
        if args.full {
            value["hits"] = serde_json::Value::Array(
                page.hits
                    .iter()
                    .map(|hit| {
                        let mut value = serde_json::to_value(&hit.session)?;
                        value["score"] = hit.score.into();
                        value["snippet"] = hit.snippet.clone().into();
                        value["role"] = hit.role.clone().into();
                        value["record_order"] = hit.record_order.into();
                        Ok(value)
                    })
                    .collect::<Result<Vec<_>>>()?,
            );
        } else if !args.fields.is_empty() {
            for hit in value["hits"].as_array_mut().unwrap() {
                hit.as_object_mut()
                    .unwrap()
                    .retain(|key, _| args.fields.contains(key));
            }
        }
        println!("{}", serde_json::to_string(&value)?);
    } else {
        for hit in &page.hits {
            println!(
                "{} · {} · {} · {}\n  {}\n  {}",
                hit.started_at.as_deref().unwrap_or("unknown time"),
                hit.agent.label(),
                hit.id,
                hit.role,
                hit.project
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                hit.snippet
            );
        }
        eprintln!(
            "tendi: {} of {} hits; persistent index, {} pending sessions",
            page.hits.len(),
            page.total,
            page.status.pending_sessions
        );
    }
    Ok(())
}

pub fn transcript(
    cwd: &Path,
    path: Option<String>,
    current: bool,
    agent: Option<AgentArg>,
    args: TranscriptArgs,
    json: bool,
) -> Result<()> {
    let (path, agent) = if current {
        let session = tendi_core::sessions::current_session(cwd, agent.map(Into::into))?;
        (
            session.path.with_context(|| {
                format!(
                    "no local transcript is available for {} session {}",
                    session.agent.label(),
                    session.id
                )
            })?,
            session.agent,
        )
    } else if let Some(id) = args.session.as_deref() {
        let session =
            tendi_core::storage::Store::open_default()?.session_by_id(id, agent.map(Into::into))?;
        (session.path, session.agent)
    } else {
        (
            PathBuf::from(path.context("transcript path is required")?),
            agent
                .context("--agent is required for an explicit path")?
                .into(),
        )
    };
    if args.all {
        if json {
            let stdout = std::io::stdout();
            let mut output = stdout.lock();
            tendi_core::transcript::write_transcript_json(&path, agent, &mut output)?;
            writeln!(output)?;
        } else {
            print_transcript(&tendi_core::transcript::parse_transcript(&path, agent)?.items)?;
        }
    } else if args.page || args.cursor.is_some() {
        let page = tendi_core::transcript::parse_transcript_page(
            &path,
            agent,
            args.cursor.as_deref(),
            Some(args.limit),
        )?;
        if json {
            println!("{}", serde_json::to_string(&page)?);
        } else {
            print_transcript(&page.items)?;
        }
    } else {
        let excerpt = tendi_core::transcript::read_transcript_excerpt(
            &path,
            agent,
            &tendi_core::transcript::TranscriptExcerptOptions {
                query: args.query,
                roles: args.roles,
                offset: args.offset,
                limit: args.limit,
                around: args.around,
            },
        )?;
        if json {
            println!("{}", serde_json::to_string(&excerpt)?);
        } else {
            for item in &excerpt.items {
                println!(
                    "[{}] {} · {}{}\n{}\n",
                    item.index,
                    item.item.time.as_deref().unwrap_or("unknown time"),
                    item.item.kind,
                    if item.matched { " · match" } else { "" },
                    item.item.body
                );
            }
            if let Some(offset) = excerpt.next_offset {
                eprintln!("tendi: continue with --offset {offset}");
            }
        }
    }
    Ok(())
}
