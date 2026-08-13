use std::{env, io::Write};

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "tendi", version, about = "Local agent control plane")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Scan {
        #[arg(long)]
        json: bool,
    },
    Agents {
        #[command(subcommand)]
        command: ListCommand,
    },
    Skills {
        #[command(subcommand)]
        command: SkillCommand,
    },
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Rules {
        #[command(subcommand)]
        command: ListCommand,
    },
    Hooks {
        #[command(subcommand)]
        command: ListCommand,
    },
    Mcp {
        #[command(subcommand)]
        command: ListCommand,
    },
}

#[derive(Debug, Subcommand)]
enum SkillCommand {
    List {
        #[arg(long)]
        json: bool,
    },
    Add {
        source: String,
        #[arg(long = "to", default_value = "shared")]
        to: AgentArg,
        #[arg(long = "skill")]
        skills: Vec<String>,
        #[arg(long)]
        list: bool,
        #[arg(long)]
        copy: bool,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
    Set {
        pattern: String,
        #[arg(long)]
        visibility: VisibilityArg,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
    Wrap {
        name: String,
        #[arg(long = "from")]
        pattern: String,
        #[arg(long, default_value_t = true, action = ArgAction::Set)]
        manual_children: bool,
        #[arg(long)]
        refresh: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
    Updates {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        check: bool,
    },
    Update {
        pattern: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
    Link {
        source: String,
        #[arg(long)]
        to: AgentArg,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    List {
        #[arg(long)]
        json: bool,
    },
    Search {
        query: String,
        #[arg(long)]
        json: bool,
    },
    Transcript {
        path: String,
        #[arg(long)]
        agent: AgentArg,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ListCommand {
    List {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum VisibilityArg {
    Auto,
    Manual,
    Off,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AgentArg {
    Codex,
    Cursor,
    Claude,
    Shared,
}

impl From<AgentArg> for tendi_core::AgentKind {
    fn from(value: AgentArg) -> Self {
        match value {
            AgentArg::Codex => tendi_core::AgentKind::Codex,
            AgentArg::Cursor => tendi_core::AgentKind::Cursor,
            AgentArg::Claude => tendi_core::AgentKind::Claude,
            AgentArg::Shared => tendi_core::AgentKind::Shared,
        }
    }
}

impl From<VisibilityArg> for tendi_core::SkillVisibility {
    fn from(value: VisibilityArg) -> Self {
        match value {
            VisibilityArg::Auto => tendi_core::SkillVisibility::Auto,
            VisibilityArg::Manual => tendi_core::SkillVisibility::Manual,
            VisibilityArg::Off => tendi_core::SkillVisibility::Off,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = env::current_dir()?;

    match cli.command {
        Command::Scan { json } => {
            let report = tendi_core::scan_and_persist(cwd)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_summary(&report)?;
            }
        }
        Command::Agents { command } => match command {
            ListCommand::List { json } => {
                let report = tendi_core::agents::scan_agents(&cwd)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report.agents)?);
                } else {
                    print_agents(&report.agents)?;
                }
            }
        },
        Command::Skills { command } => match command {
            SkillCommand::List { json } => {
                let report = tendi_core::skills::scan_skills_synced(&cwd)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report.skills)?);
                } else {
                    println!("{}", tendi_core::skills::format_skill_table(&report.skills));
                }
            }
            SkillCommand::Add {
                source,
                to,
                skills,
                list,
                copy,
                overwrite,
                dry_run,
                yes,
            } => {
                if list {
                    let plan = tendi_core::skills::list_installable_skills(&cwd, &source)?;
                    print_installable_skills(&plan)?;
                    return Ok(());
                }

                let options = tendi_core::skills::SkillAddOptions {
                    source,
                    target: to.into(),
                    skills,
                    copy,
                    overwrite,
                };
                let plan = tendi_core::skills::plan_skill_add(&cwd, &options)?;
                print_skill_add_plan(&plan)?;
                if dry_run {
                    return Ok(());
                }
                if !yes && !confirm("Install these skills? [y/N] ")? {
                    println!("aborted");
                    return Ok(());
                }
                let report = tendi_core::skills::apply_skill_add(&cwd, &options)?;
                print_skill_add_results(&report.results)?;
            }
            SkillCommand::Set {
                pattern,
                visibility,
                dry_run,
                yes,
            } => {
                let changeset =
                    tendi_core::skills::plan_visibility(&cwd, &pattern, visibility.into())?;
                run_changeset(changeset, dry_run, yes)?;
            }
            SkillCommand::Wrap {
                name,
                pattern,
                manual_children,
                refresh,
                dry_run,
                yes,
            } => {
                let changeset = if refresh {
                    tendi_core::skills::refresh_wrapper(&cwd, &name, &pattern, manual_children)?
                } else {
                    tendi_core::skills::plan_wrapper(&cwd, &name, &pattern, manual_children)?
                };
                run_changeset(changeset, dry_run, yes)?;
            }
            SkillCommand::Updates { json, check } => {
                if check {
                    let updates = tendi_core::skills::check_skill_updates(&cwd)?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&updates)?);
                    } else {
                        print_checked_updates(&updates)?;
                    }
                    return Ok(());
                }

                let report = tendi_core::skills::scan_skills_synced(&cwd)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report.skills)?);
                } else {
                    print_skill_updates(&report.skills)?;
                }
            }
            SkillCommand::Update {
                pattern,
                dry_run,
                yes,
            } => {
                let plan = tendi_core::skills::plan_skill_updates(&cwd, &pattern)?;
                print_update_plan(&plan)?;
                if dry_run {
                    return Ok(());
                }
                if !yes && !confirm("Apply these updates? [y/N] ")? {
                    println!("aborted");
                    return Ok(());
                }
                tendi_core::skills::apply_skill_update_plan(&plan)?;
                println!("applied");
            }
            SkillCommand::Link {
                source,
                to,
                name,
                dry_run,
                yes,
            } => {
                let source = std::path::PathBuf::from(source);
                let preview = tendi_core::skills::materialize_skill_dir(
                    &source,
                    to.into(),
                    name.as_deref(),
                    true,
                )?;
                println!(
                    "{} {} -> {}",
                    preview.mode,
                    preview.source.display(),
                    preview.target.display()
                );
                if dry_run {
                    return Ok(());
                }
                if !yes && !confirm("Apply this link? [y/N] ")? {
                    println!("aborted");
                    return Ok(());
                }
                let result = tendi_core::skills::materialize_skill_dir(
                    &source,
                    to.into(),
                    name.as_deref(),
                    false,
                )?;
                println!("{}: {}", result.mode, result.health);
            }
        },
        Command::Sessions { command } => match command {
            SessionCommand::List { json } => {
                let store = tendi_core::storage::Store::open_default()?;
                let cache = store.session_scan_cache()?;
                let report = tendi_core::sessions::scan_sessions_with_additional_roots_cached(
                    &cwd,
                    &[],
                    &cache,
                )?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report.sessions)?);
                } else {
                    print_sessions(&report.sessions)?;
                }
            }
            SessionCommand::Search { query, json } => {
                let store = tendi_core::storage::Store::open_default()?;
                let cache = store.session_scan_cache()?;
                let report = tendi_core::sessions::scan_sessions_with_additional_roots_cached(
                    &cwd,
                    &[],
                    &cache,
                )?;
                store.save_sessions(&report)?;
                let hits = store.search_sessions(&query)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&hits)?);
                } else {
                    print_session_search_hits(&hits)?;
                }
            }
            SessionCommand::Transcript { path, agent, json } => {
                let transcript = tendi_core::transcript::parse_transcript(
                    std::path::Path::new(&path),
                    agent.into(),
                )?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&transcript.items)?);
                } else {
                    print_transcript(&transcript.items)?;
                }
            }
        },
        Command::Rules { command } => match command {
            ListCommand::List { json } => {
                let report = tendi_core::rules::scan_rules(&cwd)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report.rules)?);
                } else {
                    print_rules(&report.rules)?;
                }
            }
        },
        Command::Hooks { command } => match command {
            ListCommand::List { json } => {
                let report = tendi_core::hooks::scan_hooks(&cwd)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report.hooks)?);
                } else {
                    print_hooks(&report.hooks)?;
                }
            }
        },
        Command::Mcp { command } => match command {
            ListCommand::List { json } => {
                let report = tendi_core::mcp::scan_mcp(&cwd)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report.servers)?);
                } else {
                    print_mcp(&report.servers)?;
                }
            }
        },
    }

    Ok(())
}

fn print_agents(agents: &[tendi_core::AgentRecord]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{:<10} {:<10} {:<24} {}",
        "agent", "installed", "version", "config"
    )?;
    for agent in agents {
        writeln!(
            stdout,
            "{:<10} {:<10} {:<24} {}",
            agent_label(agent.kind),
            agent.installed,
            agent.version.as_deref().unwrap_or("-"),
            agent
                .config_dir
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string())
        )?;
    }
    Ok(())
}

fn print_update_plan(plan: &tendi_core::skills::SkillUpdatePlan) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    for change in &plan.file_changes.changes {
        let op = if change.before.is_some() { "M" } else { "A" };
        writeln!(stdout, "{op} {}", change.path.display())?;
    }
    for action in &plan.git_updates {
        writeln!(
            stdout,
            "G {} {} -> {}",
            action.name,
            action.current_version.as_deref().unwrap_or("-"),
            action
                .latest_version
                .as_deref()
                .map(short_hash)
                .unwrap_or_else(|| "-".to_string())
        )?;
    }
    for skipped in &plan.skipped {
        writeln!(stdout, "S {} {}", skipped.name, skipped.status)?;
    }
    Ok(())
}

fn print_installable_skills(plan: &tendi_core::skills::SkillAddPlan) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "source: {} ({})", plan.source, plan.source_kind)?;
    writeln!(stdout, "{:<28} {:<34} description", "name", "path")?;
    for skill in &plan.available {
        writeln!(
            stdout,
            "{:<28} {:<34} {}",
            skill.name,
            compact_cli(&skill.relative_path, 34),
            skill.description.as_deref().unwrap_or("-")
        )?;
    }
    Ok(())
}

fn print_skill_add_plan(plan: &tendi_core::skills::SkillAddPlan) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "source: {} ({})", plan.source, plan.source_kind)?;
    for operation in &plan.operations {
        writeln!(
            stdout,
            "A {} {} -> {} ({})",
            operation.name,
            operation.source.display(),
            operation.target.display(),
            operation.mode
        )?;
    }
    Ok(())
}

fn print_skill_add_results(results: &[tendi_core::skills::MaterializeResult]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    for result in results {
        writeln!(
            stdout,
            "{}: {} -> {} ({})",
            result.mode,
            result.source.display(),
            result.target.display(),
            result.health
        )?;
    }
    Ok(())
}

fn print_checked_updates(updates: &[tendi_core::skills::SkillUpdateReport]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{:<28} {:<18} {:<12} {:<12} {}",
        "name", "status", "current", "latest", "source"
    )?;
    for update in updates {
        writeln!(
            stdout,
            "{:<28} {:<18} {:<12} {:<12} {}",
            update.name,
            update.status,
            update.current_version.as_deref().unwrap_or("-"),
            update
                .latest_version
                .as_deref()
                .map(short_hash)
                .unwrap_or_else(|| "-".to_string()),
            update.source.as_deref().unwrap_or("-")
        )?;
    }
    Ok(())
}

fn print_skill_updates(skills: &[tendi_core::SkillRecord]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{:<28} {:<12} {:<18} {}",
        "name", "status", "version", "source"
    )?;
    for skill in skills {
        let version = skill
            .paths
            .iter()
            .find_map(|path| path.source_version.as_deref())
            .unwrap_or("-");
        writeln!(
            stdout,
            "{:<28} {:<12} {:<18} {}",
            skill.name, skill.update_status, version, skill.source_summary
        )?;
    }
    Ok(())
}

fn short_hash(value: &str) -> String {
    value.chars().take(12).collect()
}

fn compact_cli(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let keep = max.saturating_sub(3);
    format!("{}...", value.chars().take(keep).collect::<String>())
}

fn print_sessions(sessions: &[tendi_core::SessionRecord]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{:<8} {:<36} {:<6} {:<28} {:<28} {}",
        "agent", "id", "msgs", "started", "updated", "title"
    )?;
    for session in sessions.iter().take(80) {
        writeln!(
            stdout,
            "{:<8} {:<36} {:<6} {:<28} {:<28} {}",
            agent_label(session.agent),
            session.id,
            session
                .message_count
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            session.started_at.as_deref().unwrap_or("-"),
            session.updated_at.as_deref().unwrap_or("-"),
            session.title.as_deref().unwrap_or("-")
        )?;
    }
    Ok(())
}

fn print_session_search_hits(hits: &[tendi_core::SessionSearchHit]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    for hit in hits {
        writeln!(
            stdout,
            "[{}] {}  {}",
            agent_label(hit.session.agent),
            hit.session.id,
            hit.session.title.as_deref().unwrap_or("-"),
        )?;
        writeln!(stdout, "  {}", hit.search_snippet)?;
    }
    Ok(())
}

fn print_transcript(items: &[tendi_core::TranscriptItem]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{:<9} {:<8} {:<16} body", "kind", "time", "tag")?;
    for item in items.iter().take(120) {
        writeln!(
            stdout,
            "{:<9} {:<8} {:<16} {}",
            item.kind,
            item.time.as_deref().unwrap_or("-"),
            item.tag.as_deref().unwrap_or("-"),
            item.body.replace('\n', " ")
        )?;
    }
    Ok(())
}

fn print_rules(rules: &[tendi_core::RuleRecord]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{:<8} {:<12} {:<8} {:<5} {}",
        "agent", "kind", "scope", "order", "path"
    )?;
    for rule in rules {
        writeln!(
            stdout,
            "{:<8} {:<12} {:<8} {:<5} {}",
            agent_label(rule.agent),
            rule.kind,
            rule.scope,
            rule.order,
            rule.path.display()
        )?;
    }
    Ok(())
}

fn print_hooks(hooks: &[tendi_core::HookRecord]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{:<8} {:<22} {:<7} {:<12} {}",
        "agent", "event", "enabled", "hash", "command"
    )?;
    for hook in hooks.iter().take(120) {
        writeln!(
            stdout,
            "{:<8} {:<22} {:<7} {:<12} {}",
            agent_label(hook.agent),
            hook.event,
            hook.enabled,
            &hook.trust_hash[..12.min(hook.trust_hash.len())],
            hook.command.as_deref().unwrap_or("-")
        )?;
    }
    Ok(())
}

fn print_mcp(servers: &[tendi_core::McpServerRecord]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{:<8} {:<24} {:<12} {:<12} {}",
        "agent", "name", "transport", "status", "path"
    )?;
    for server in servers {
        writeln!(
            stdout,
            "{:<8} {:<24} {:<12} {:<12} {}",
            agent_label(server.agent),
            server.name,
            server.transport,
            server.status,
            server.path.display()
        )?;
    }
    Ok(())
}

fn agent_label(agent: tendi_core::AgentKind) -> &'static str {
    match agent {
        tendi_core::AgentKind::Codex => "codex",
        tendi_core::AgentKind::Cursor => "cursor",
        tendi_core::AgentKind::Claude => "claude",
        tendi_core::AgentKind::Shared => "shared",
        tendi_core::AgentKind::Unknown => "unknown",
    }
}

fn print_summary(report: &tendi_core::ScanReport) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "agents: {}", report.agents.agents.len())?;
    writeln!(stdout, "skills: {}", report.skills.skills.len())?;
    writeln!(stdout, "sessions: {}", report.sessions.sessions.len())?;
    writeln!(stdout, "rules: {}", report.rules.rules.len())?;
    writeln!(stdout, "hooks: {}", report.hooks.hooks.len())?;
    writeln!(stdout, "mcp servers: {}", report.mcp.servers.len())?;
    writeln!(stdout, "roots: {}", report.skills.roots.len())?;
    writeln!(
        stdout,
        "db: {}",
        tendi_core::storage::default_db_path()?.display()
    )?;

    let warning_count = report.skills.warnings.len()
        + report.sessions.warnings.len()
        + report.rules.warnings.len()
        + report.hooks.warnings.len()
        + report.mcp.warnings.len();
    if warning_count > 0 {
        writeln!(stdout, "warnings: {warning_count}")?;
        for warning in report
            .skills
            .warnings
            .iter()
            .chain(report.sessions.warnings.iter())
            .chain(report.rules.warnings.iter())
            .chain(report.hooks.warnings.iter())
            .chain(report.mcp.warnings.iter())
        {
            writeln!(stdout, "- {warning}")?;
        }
    }
    Ok(())
}

fn run_changeset(changeset: tendi_core::skills::ChangeSet, dry_run: bool, yes: bool) -> Result<()> {
    println!("{}", tendi_core::skills::format_changeset(&changeset));
    if dry_run {
        return Ok(());
    }

    if !yes && !confirm("Apply these changes? [y/N] ")? {
        println!("aborted");
        return Ok(());
    }

    tendi_core::skills::apply_changes(&changeset)?;
    println!("applied");
    Ok(())
}

fn confirm(prompt: &str) -> Result<bool> {
    let mut stdout = std::io::stdout().lock();
    write!(stdout, "{prompt}")?;
    stdout.flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim(), "y" | "Y" | "yes" | "YES"))
}
