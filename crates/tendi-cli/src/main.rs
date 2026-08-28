use std::{
    env,
    io::{IsTerminal, Write},
    thread,
    time::Duration,
};

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
    Setup {
        #[command(subcommand)]
        command: SetupCommand,
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
    Guide {
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Targets {
        #[arg(long)]
        json: bool,
    },
    Sync {
        #[command(subcommand)]
        command: BackupCommand,
    },
    Add {
        source: String,
        #[arg(long = "to", default_value = "shared")]
        to: tendi_core::SkillTarget,
        #[arg(long, default_value = "global")]
        scope: tendi_core::SkillInstallScope,
        #[arg(long = "skill")]
        skills: Vec<String>,
        #[arg(long)]
        list: bool,
        #[arg(long)]
        copy: bool,
        #[arg(long)]
        overwrite: bool,
        #[arg(long, value_enum, default_value = "auto")]
        visibility: VisibilityArg,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    Restore {
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
        to: tendi_core::SkillTarget,
        #[arg(long, default_value = "global")]
        scope: tendi_core::SkillInstallScope,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
enum BackupCommand {
    Configure {
        remote_url: String,
        #[arg(long)]
        checkout: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Status {
        #[arg(long)]
        json: bool,
    },
    Run {
        #[arg(long)]
        json: bool,
    },
    Versions {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    Restore {
        revision: String,
        #[arg(long = "skill")]
        skills: Vec<String>,
        #[arg(long = "to", default_value = "shared")]
        to: tendi_core::SkillTarget,
        #[arg(long, default_value = "global")]
        scope: tendi_core::SkillInstallScope,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
        #[arg(long, value_enum)]
            conflict: Option<BackupRestoreConflictArg>,
        #[arg(long)]
        json: bool,
    },
    Add {
        path: std::path::PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Disconnect {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BackupRestoreConflictArg {
    Skip,
    Replace,
    KeepBoth,
}

impl BackupRestoreConflictArg {
    fn action(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::Replace => "replace",
            Self::KeepBoth => "keep-both",
        }
    }
}

#[derive(Debug, Subcommand)]
enum SetupCommand {
    Skills {
        #[arg(long = "to", default_value = "shared")]
        to: AgentArg,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
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

fn ensure_projection<T, Ready, Refresh>(
    store: &tendi_core::storage::Store,
    domain: &str,
    ready: Ready,
    mut refresh: Refresh,
) -> Result<T>
where
    Ready: Fn() -> Result<Option<T>>,
    Refresh: FnMut() -> Result<T>,
{
    for _ in 0..100 {
        if let Some(value) = ready()? {
            return Ok(value);
        }
        if let Some(result) = store.with_projection_refresh_lock(domain, || refresh())? {
            return Ok(result);
        }
        thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("timed out waiting for another projection refresh")
}

fn with_database_write_lock<T, F>(store: &tendi_core::storage::Store, mut write: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    for _ in 0..100 {
        if let Some(value) = store.with_database_write_lock(&mut write)? {
            return Ok(value);
        }
        thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("timed out waiting for the database write lock")
}

fn invalidate_skills_projection(
    store: &tendi_core::storage::Store,
    cwd: &std::path::Path,
) -> Result<()> {
    with_database_write_lock(store, || store.invalidate_projection("skills", cwd))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = env::current_dir()?;
    maybe_offer_bundled_skill(&cli.command)?;

    match cli.command {
        Command::Scan { json } => {
            let store = tendi_core::storage::Store::open_default()?;
            let report = ensure_projection(
                &store,
                "agents",
                || Ok(None),
                || {
                    let report = tendi_core::scan(&cwd)?;
                    with_database_write_lock(&store, || {
                        store.save_scan_for_workspace(&cwd, &report)
                    })?;
                    Ok(report)
                },
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_summary(&report)?;
            }
        }
        Command::Agents { command } => match command {
            ListCommand::List { json } => {
                let store = tendi_core::storage::Store::open_default()?;
                let report = ensure_projection(
                    &store,
                    "agents",
                    || store.list_agents_for_workspace(&cwd),
                    || {
                        let report = tendi_core::agents::scan_agents(&cwd)?;
                        with_database_write_lock(&store, || {
                            store.save_agents_for_workspace(&cwd, &report)
                        })?;
                        Ok(report)
                    },
                )?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report.agents)?);
                } else {
                    print_agents(&report.agents)?;
                }
            }
        },
        Command::Skills { command } => match command {
            SkillCommand::Guide { json } => {
                let markdown = tendi_core::bundled_skill::guide_markdown();
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "name": "tendi",
                            "version": env!("CARGO_PKG_VERSION"),
                            "markdown": markdown,
                        }))?
                    );
                } else {
                    print!("{markdown}");
                }
            }
            SkillCommand::List { json } => {
                let store = tendi_core::storage::Store::open_default()?;
                let report = ensure_projection(
                    &store,
                    "skills",
                    || store.list_skills_for_workspace(&cwd),
                    || {
                        let scanned = tendi_core::skills::scan_skills_synced_for_project_roots_with_store_for_projection(
                            &cwd,
                            &store,
                            &[],
                        )?;
                        with_database_write_lock(&store, || {
                            store.save_skills_for_workspace_with_source_migrations(
                                &cwd,
                                &scanned.scan,
                                &scanned.source_migrations,
                            )
                        })?;
                        Ok(scanned.scan)
                    },
                )?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report.skills)?);
                } else {
                    println!("{}", tendi_core::skills::format_skill_table(&report.skills));
                }
            }
            SkillCommand::Targets { json } => {
                let targets = tendi_core::skill_targets::target_catalog();
                if json {
                    println!("{}", serde_json::to_string_pretty(targets)?);
                } else {
                    for target in targets {
                        let project_root = target.project_skills_root(&cwd)?;
                        let project_label = project_root
                            .strip_prefix(&cwd)
                            .unwrap_or(project_root.as_path())
                            .display()
                            .to_string();
                        println!(
                            "{:<20} {:<24} project={} global={}",
                            target.id,
                            target.display_name,
                            project_label,
                            if target.supports_global() {
                                "yes"
                            } else {
                                "no"
                            },
                        );
                    }
                }
            }
            SkillCommand::Sync { command } => match command {
                BackupCommand::Configure { remote_url, checkout, json } => {
                    let checkout_path = checkout
                        .unwrap_or(tendi_core::skill_backup::default_checkout_path()?);
                    let config =
                        tendi_core::skill_backup::BackupConfig::new(remote_url, checkout_path);
                    config.validate()?;
                    let working_directory = config
                        .checkout_path
                        .parent()
                        .unwrap_or_else(|| cwd.as_path());
                    tendi_core::skill_backup::validate_remote(
                        &config.remote_url,
                        working_directory,
                    )?;
                    let store = tendi_core::storage::Store::open_default()?;
                    let config = with_database_write_lock(&store, || {
                        store.save_skill_backup_config(&config)
                    })?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&config)?);
                    } else {
                        println!("Sync configured for {}", config.remote_url);
                    }
                }
                BackupCommand::Status { json } => {
                    let store = tendi_core::storage::Store::open_default()?;
                    let config = store.skill_backup_config()?;
                    let versions = if config.is_some() {
                        tendi_core::skill_backup::backup_versions(&store, 1)?
                    } else {
                        Vec::new()
                    };
                    if json {
                        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "config": config, "versions": versions }))?);
                    } else if let Some(config) = config {
                        println!("remote: {}", config.remote_url);
                        println!("checkout: {}", config.checkout_path.display());
                        if let Some(version) = versions.first() {
                            println!("latest: {} {}", version.id, version.summary);
                        } else {
                            println!("latest: none");
                        }
                    } else {
                        println!("Sync is not configured");
                    }
                }
                BackupCommand::Run { json } => {
                    let store = tendi_core::storage::Store::open_default()?;
                    let report = tendi_core::skill_backup::backup_now(&store, &cwd)?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&report)?);
                    } else if let Some(commit) = report.commit {
                        println!(
                            "Synced {} skills and {} configuration sources ({commit})",
                            report.manifest.skills.len(),
                            report.manifest.artifacts.len()
                        );
                    } else {
                        println!("Sync is already current");
                    }
                }
                BackupCommand::Versions { limit, json } => {
                    let store = tendi_core::storage::Store::open_default()?;
                    let versions = tendi_core::skill_backup::backup_versions(&store, limit)?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&versions)?);
                    } else {
                        for version in versions {
                            println!("{} {} {}", version.id, version.created_at, version.summary);
                        }
                    }
                }
                BackupCommand::Restore { revision, skills, to, scope, dry_run, yes, conflict, json } => {
                    let store = tendi_core::storage::Store::open_default()?;
                    let plan = tendi_core::skill_backup::plan_backup_restore(
                        &store,
                        &cwd,
                        &revision,
                        &skills,
                        &to,
                        scope,
                    )?;
                    if dry_run {
                        if json {
                            println!("{}", serde_json::to_string_pretty(&plan)?);
                        } else {
                            print_backup_restore_operations(&plan)?;
                        }
                        return Ok(());
                    }
                    if json && !yes {
                        anyhow::bail!("--json requires --yes for a real restore");
                    }
                    if !json {
                        print_backup_restore_operations(&plan)?;
                    }
                    let has_conflicts = plan.operations.iter().any(|operation| operation.status == "conflict");
                    let has_planned = plan.operations.iter().any(|operation| operation.status == "planned");
                    if has_conflicts && conflict.is_none() {
                        anyhow::bail!("restore has conflicts; pass --conflict skip, --conflict replace, or --conflict keep-both");
                    }
                    if !has_planned && !has_conflicts {
                        return Ok(());
                    }
                    if !yes && !confirm("Restore these sync skills? [y/N] ")? {
                        println!("aborted");
                        return Ok(());
                    }
                    let resolutions = conflict
                        .map(|conflict| {
                            plan.operations
                                .iter()
                                .filter(|operation| operation.status == "conflict")
                                .map(|operation| tendi_core::skill_backup::BackupRestoreResolution {
                                    id: operation.id.clone(),
                                    action: conflict.action().to_string(),
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let applied = tendi_core::skill_backup::apply_backup_restore_without_database(
                        &plan,
                        &resolutions,
                    )?;
                    let operations = applied.operations;
                    with_database_write_lock(&store, || {
                        store.upsert_skill_source_records(&applied.source_records)?;
                        store.invalidate_projection("skills", &cwd)
                    })?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&operations)?);
                    } else {
                        for operation in operations {
                            println!("{} {} -> {}", operation.status, operation.name, operation.target.display());
                        }
                    }
                }
                BackupCommand::Add { path, name, json } => {
                    let name = name.or_else(|| path.file_name().and_then(|name| name.to_str()).map(str::to_string))
                        .ok_or_else(|| anyhow::anyhow!("--name is required when the path has no file name"))?;
                    let store = tendi_core::storage::Store::open_default()?;
                    let record = tendi_core::skill_backup::skill_backup_record_for_adoption(
                        &path,
                        name,
                    )?;
                    let record = with_database_write_lock(&store, || {
                        store.upsert_skill_source_records(std::slice::from_ref(&record))?;
                        store.invalidate_projection("skills", &cwd)?;
                        Ok(record.clone())
                    })?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&record)?);
                    } else {
                        println!("Added {} to sync", record.skill_name);
                    }
                }
                BackupCommand::Disconnect { yes } => {
                    if !yes && !confirm("Disconnect this machine from skill sync? [y/N] ")? {
                        println!("aborted");
                        return Ok(());
                    }
                    let store = tendi_core::storage::Store::open_default()?;
                    if with_database_write_lock(&store, || store.clear_skill_backup_config())? {
                        println!("Disconnected this machine from skill sync");
                    } else {
                        println!("Sync was not configured");
                    }
                }
            },
            SkillCommand::Add {
                source,
                to,
                scope,
                skills,
                list,
                copy,
                overwrite,
                visibility,
                dry_run,
                yes,
                json,
            } => {
                if list {
                    let plan = tendi_core::skills::list_installable_skills(&cwd, &source)?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&plan)?);
                    } else {
                        print_installable_skills(&plan)?;
                    }
                    return Ok(());
                }

                let options = tendi_core::skills::SkillAddOptions {
                    source,
                    target: to,
                    scope,
                    skills,
                    copy,
                    overwrite,
                    visibility: visibility.into(),
                };
                let plan = tendi_core::skills::plan_skill_add(&cwd, &options)?;
                if dry_run {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&plan)?);
                    } else {
                        print_skill_add_plan(&plan)?;
                    }
                    return Ok(());
                }
                if json && !yes {
                    anyhow::bail!("--json requires --yes for a real installation");
                }
                if !yes && !confirm("Install these skills? [y/N] ")? {
                    println!("aborted");
                    return Ok(());
                }
                let report = tendi_core::skills::apply_skill_add(&cwd, &options)?;
                let source_records = tendi_core::skills::skill_source_records_for_add(&report);
                let snapshots = tendi_core::skills::capture_skill_snapshots(&source_records)?;
                let store = tendi_core::storage::Store::open_default()?;
                with_database_write_lock(&store, || {
                    store.upsert_skill_source_records(&source_records)?;
                    store.replace_skill_snapshots(&snapshots)?;
                    if json {
                        Ok(())
                    } else {
                        store.invalidate_projection("skills", &cwd)
                    }
                })?;
                if json {
                    let scanned = tendi_core::skills::scan_skills_synced_for_projection(&cwd)?;
                    let scan = scanned.scan.clone();
                    with_database_write_lock(&store, || {
                        store.save_skills_for_workspace_with_source_migrations(
                            &cwd,
                            &scanned.scan,
                            &scanned.source_migrations,
                        )
                    })?;
                    println!(
                        "{}",
                        serde_json::json!({
                            "applied": true,
                            "report": report,
                            "skills": scan.skills,
                        })
                    );
                } else {
                    print_skill_add_results(&report.results)?;
                }
            }
            SkillCommand::Restore { dry_run, yes } => {
                let store = tendi_core::storage::Store::open_default()?;
                let plan = tendi_core::skill_restore::plan_project_skill_restore(&cwd, &store)?;
                print_skill_restore_operations(
                    &plan.lock_path,
                    &plan.target_root,
                    &plan.operations,
                )?;
                if dry_run || !plan.operations.iter().any(|item| item.status == "planned") {
                    return Ok(());
                }
                if !yes && !confirm("Restore these project skills? [y/N] ")? {
                    println!("aborted");
                    return Ok(());
                }
                let applied = tendi_core::skill_restore::apply_project_skill_restore_without_database(&plan)?;
                let report = applied.report;
                with_database_write_lock(&store, || {
                    store.upsert_skill_source_records(&applied.source_records)?;
                    store.invalidate_projection("skills", &cwd)
                })?;
                print_skill_restore_operations(
                    &report.lock_path,
                    &report.target_root,
                    &report.operations,
                )?;
            }
            SkillCommand::Set {
                pattern,
                visibility,
                dry_run,
                yes,
            } => {
                let changeset =
                    tendi_core::skills::plan_visibility(&cwd, &pattern, visibility.into())?;
                run_changeset(changeset, dry_run, yes, &cwd)?;
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
                run_changeset(changeset, dry_run, yes, &cwd)?;
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

                let store = tendi_core::storage::Store::open_default()?;
                let report = ensure_projection(
                    &store,
                    "skills",
                    || store.list_skills_for_workspace(&cwd),
                    || {
                        let scanned = tendi_core::skills::scan_skills_synced_for_projection(&cwd)?;
                        with_database_write_lock(&store, || {
                            store.save_skills_for_workspace_with_source_migrations(
                                &cwd,
                                &scanned.scan,
                                &scanned.source_migrations,
                            )
                        })?;
                        Ok(scanned.scan)
                    },
                )?;
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
                let store = tendi_core::storage::Store::open_default()?;
                let prepared = tendi_core::skills::prepare_skill_update_plan_with_resolutions(
                    &plan,
                    &std::collections::BTreeMap::new(),
                )?;
                tendi_core::skills::apply_skill_update_plan_filesystem(&prepared)?;
                let persistence =
                    tendi_core::skills::prepare_skill_update_persistence(&store, &prepared)?;
                with_database_write_lock(&store, || {
                    tendi_core::skills::persist_skill_update_persistence(&store, &persistence)?;
                    store.invalidate_projection("skills", &cwd)
                })?;
                println!("applied");
            }
            SkillCommand::Link {
                source,
                to,
                scope,
                name,
                dry_run,
                yes,
            } => {
                let source = std::path::PathBuf::from(source);
                let preview = tendi_core::skills::materialize_skill_dir_for_target(
                    &source,
                    &to,
                    scope,
                    &cwd,
                    name.as_deref(),
                    false,
                    false,
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
                let result = tendi_core::skills::materialize_skill_dir_for_target(
                    &source,
                    &to,
                    scope,
                    &cwd,
                    name.as_deref(),
                    false,
                    false,
                    false,
                )?;
                let store = tendi_core::storage::Store::open_default()?;
                invalidate_skills_projection(&store, &cwd)?;
                println!("{}: {}", result.mode, result.health);
            }
        },
        Command::Setup { command } => match command {
            SetupCommand::Skills {
                to,
                overwrite,
                dry_run,
                yes,
                json,
            } => {
                let agent = to.into();
                let plan = tendi_core::bundled_skill::plan_install(agent)?;
                if dry_run {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&plan)?);
                    } else {
                        print_bundled_skill_install_plan(&plan)?;
                    }
                    return Ok(());
                }
                if json && !yes {
                    anyhow::bail!("--json requires --yes for a real installation");
                }
                if plan.requires_overwrite && !overwrite {
                    if !json {
                        print_bundled_skill_install_plan(&plan)?;
                    }
                    anyhow::bail!(
                        "the target contains different content; inspect it and rerun with --overwrite"
                    );
                }
                if !yes && !plan.changes.changes.is_empty() {
                    print_bundled_skill_install_plan(&plan)?;
                    let question = if plan.requires_overwrite {
                        "Replace the existing bundled skill files? [y/N] "
                    } else {
                        "Install the Tendi skill? [Y/n] "
                    };
                    let confirmed = if plan.requires_overwrite {
                        confirm(question)?
                    } else {
                        confirm_default_yes(question)?
                    };
                    if !confirmed {
                        tendi_core::bundled_skill::dismiss_prompt()?;
                        println!("aborted");
                        return Ok(());
                    }
                }
                let report = tendi_core::bundled_skill::install(agent, overwrite, false)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else if report.applied {
                    println!("installed: {}", report.status.target.display());
                } else {
                    println!("up to date: {}", report.status.target.display());
                }
            }
        },
        Command::Sessions { command } => match command {
            SessionCommand::List { json } => {
                let store = tendi_core::storage::Store::open_default()?;
                let mut report = store.list_sessions()?;
                if report.sessions.is_empty() && store.last_scan_at()?.is_none() {
                    let scan = tendi_core::scan(&cwd)?;
                    with_database_write_lock(&store, || store.save_scan_for_workspace(&cwd, &scan))?;
                    report = scan.sessions;
                }
                store.resolve_session_projects(&mut report.sessions)?;
                if json {
                    let stdout = std::io::stdout();
                    let mut output = stdout.lock();
                    serde_json::to_writer_pretty(&mut output, &report.sessions)?;
                    writeln!(output)?;
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
                let store = tendi_core::storage::Store::open_default()?;
                let report = ensure_projection(
                    &store,
                    "rules",
                    || store.list_rules_for_workspace(&cwd),
                    || {
                        let report = tendi_core::rules::scan_rules(&cwd)?;
                        with_database_write_lock(&store, || {
                            store.save_rules_for_workspace(&cwd, &report)
                        })?;
                        Ok(report)
                    },
                )?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report.rules)?);
                } else {
                    print_rules(&report.rules)?;
                }
            }
        },
        Command::Hooks { command } => match command {
            ListCommand::List { json } => {
                let store = tendi_core::storage::Store::open_default()?;
                let report = ensure_projection(
                    &store,
                    "hooks",
                    || store.list_hooks_for_workspace(&cwd),
                    || {
                        let report = tendi_core::hooks::scan_hooks(&cwd)?;
                        with_database_write_lock(&store, || {
                            store.save_hooks_for_workspace(&cwd, &report)
                        })?;
                        Ok(report)
                    },
                )?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report.hooks)?);
                } else {
                    print_hooks(&report.hooks)?;
                }
            }
        },
        Command::Mcp { command } => match command {
            ListCommand::List { json } => {
                let store = tendi_core::storage::Store::open_default()?;
                let report = ensure_projection(
                    &store,
                    "mcp",
                    || store.list_mcp_for_workspace(&cwd),
                    || {
                        let report = tendi_core::mcp::scan_mcp(&cwd)?;
                        with_database_write_lock(&store, || {
                            store.save_mcp_for_workspace(&cwd, &report)
                        })?;
                        Ok(report)
                    },
                )?;
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

fn print_skill_restore_operations(
    lock_path: &std::path::Path,
    target_root: &std::path::Path,
    operations: &[tendi_core::skill_restore::SkillRestoreOperation],
) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "lock: {}", lock_path.display())?;
    writeln!(stdout, "target: {}", target_root.display())?;
    for operation in operations {
        writeln!(
            stdout,
            "{} {} -> {}{}",
            operation.status,
            operation.name,
            operation.target.display(),
            operation
                .message
                .as_deref()
                .map(|message| format!(" ({message})"))
                .unwrap_or_default()
        )?;
    }
    Ok(())
}

fn print_backup_restore_operations(plan: &tendi_core::skill_backup::BackupRestorePlan) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "sync version: {}", plan.revision)?;
    writeln!(stdout, "target: {}", plan.target_root.display())?;
    for operation in &plan.operations {
        writeln!(
            stdout,
            "{} {} -> {}{}",
            operation.status,
            operation.name,
            operation.target.display(),
            operation.message.as_deref().map(|message| format!(" ({message})")).unwrap_or_default(),
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
        "{:<16} {:<12} {:<8} {:<5} {}",
        "agents", "kind", "scope", "order", "path"
    )?;
    for rule in rules {
        let agents = rule
            .agents
            .iter()
            .copied()
            .map(agent_label)
            .collect::<Vec<_>>()
            .join(",");
        writeln!(
            stdout,
            "{:<16} {:<12} {:<8} {:<5} {}",
            agents,
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
    agent.label()
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

fn run_changeset(
    changeset: tendi_core::skills::ChangeSet,
    dry_run: bool,
    yes: bool,
    cwd: &std::path::Path,
) -> Result<()> {
    println!("{}", tendi_core::skills::format_changeset(&changeset));
    if dry_run {
        return Ok(());
    }

    if !yes && !confirm("Apply these changes? [y/N] ")? {
        println!("aborted");
        return Ok(());
    }

    tendi_core::skills::apply_changes(&changeset)?;
    if !changeset.changes.is_empty() {
        let store = tendi_core::storage::Store::open_default()?;
        invalidate_skills_projection(&store, cwd)?;
    }
    println!("applied");
    Ok(())
}

fn maybe_offer_bundled_skill(command: &Command) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Ok(());
    }
    if matches!(
        command,
        Command::Skills {
            command: SkillCommand::Guide { .. }
        } | Command::Setup {
            command: SetupCommand::Skills { .. }
        }
    ) {
        return Ok(());
    }
    let status = tendi_core::bundled_skill::status(tendi_core::AgentKind::Shared)?;
    if !status.should_prompt {
        return Ok(());
    }

    println!("Tendi can install a small agent skill for searching sessions and managing skills.");
    if confirm_default_yes("Install it for your coding agents? [Y/n] ")? {
        let report =
            tendi_core::bundled_skill::install(tendi_core::AgentKind::Shared, false, false)?;
        println!("installed: {}", report.status.target.display());
    } else {
        tendi_core::bundled_skill::dismiss_prompt()?;
    }
    Ok(())
}

fn print_bundled_skill_install_plan(
    plan: &tendi_core::bundled_skill::BundledSkillInstallPlan,
) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "action: {}", plan.action)?;
    writeln!(stdout, "target: {}", plan.target.display())?;
    for change in &plan.changes.changes {
        let operation = if change.before.is_some() { "M" } else { "A" };
        writeln!(stdout, "{operation} {}", change.path.display())?;
    }
    if plan.requires_overwrite {
        writeln!(stdout, "requires: --overwrite")?;
    }
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

fn confirm_default_yes(prompt: &str) -> Result<bool> {
    let mut stdout = std::io::stdout().lock();
    write!(stdout, "{prompt}")?;
    stdout.flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(input.trim().is_empty() || matches!(input.trim(), "y" | "Y" | "yes" | "YES"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_add_accepts_extended_target_and_project_scope() {
        let cli = Cli::try_parse_from([
            "tendi", "skills", "add", "./repo", "--to", "opencode", "--scope", "project",
        ])
        .unwrap();
        let Command::Skills {
            command: SkillCommand::Add { to, scope, .. },
        } = cli.command
        else {
            panic!("unexpected command");
        };
        assert_eq!(to.id(), "opencode");
        assert_eq!(scope, tendi_core::SkillInstallScope::Project);
    }

    #[test]
    fn skill_link_keeps_legacy_shared_target() {
        let cli =
            Cli::try_parse_from(["tendi", "skills", "link", "./skill", "--to", "shared"]).unwrap();
        let Command::Skills {
            command: SkillCommand::Link { to, scope, .. },
        } = cli.command
        else {
            panic!("unexpected command");
        };
        assert_eq!(to.id(), "shared");
        assert_eq!(scope, tendi_core::SkillInstallScope::Global);
    }

    #[test]
    fn skill_restore_accepts_dry_run_and_yes() {
        let cli =
            Cli::try_parse_from(["tendi", "skills", "restore", "--dry-run", "--yes"]).unwrap();
        let Command::Skills {
            command: SkillCommand::Restore { dry_run, yes },
        } = cli.command
        else {
            panic!("unexpected command");
        };
        assert!(dry_run);
        assert!(yes);
    }

    #[test]
    fn skill_backup_configure_accepts_a_remote_without_a_device_label() {
        let cli = Cli::try_parse_from([
            "tendi",
            "skills",
            "sync",
            "configure",
            "git@github.com:example/skills.git",
        ])
        .unwrap();
        let Command::Skills {
            command: SkillCommand::Sync {
                command: BackupCommand::Configure { remote_url, .. },
            },
        } = cli.command
        else {
            panic!("unexpected command");
        };
        assert_eq!(remote_url, "git@github.com:example/skills.git");
    }
}
