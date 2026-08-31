use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use chrono::{Duration, Local};
use serde_json::{Value, json};
use tendi_core::{
    AgentKind, ScopeKey, SessionRecord, SessionScan,
    hooks::{HookDeleteRequest, delete_hooks, read_hook_source_at_path, scan_hooks},
    rules::read_rule_file_at_path,
    session_skills::{SessionFileState, SessionSkillLink},
    sessions::SessionIdentity,
    skills::{SkillPath, SkillRecord, SkillRoot, SkillScan, SkillVisibility, refresh_skill_scan},
    storage::{PromptWrite, Store},
    transcript::parse_transcript_page,
};

const OVERVIEW_SESSIONS: usize = 512;
const LINKED_SESSIONS: usize = 600;
const SKILL_TREE_FILES: usize = 300;
const SKILL_REFRESH_COUNT: usize = 240;
const HOOK_COUNT: usize = 500;
const HOOK_DELETE_COUNT: usize = 100;
const PROMPT_COUNT: usize = 500;
const SESSION_SEARCH_SESSIONS: usize = 512;
const SESSION_SEARCH_CANDIDATES: usize = 100;
const SESSION_SOAK_COUNT: usize = 10_000;

fn main() -> Result<()> {
    let scenario = env::args().nth(1).context("usage: tendi-perf <scenario>")?;
    let result = match scenario.as_str() {
        "primary-overview" => primary_overview(),
        "primary-prompts" => primary_prompts(),
        "primary-config" => primary_config(),
        "primary-settings" => primary_settings(),
        "secondary-session-page" => secondary_session_page(),
        "secondary-session-search" => secondary_session_search(),
        "secondary-session-10k-soak" => secondary_session_10k_soak(),
        "secondary-linked-sessions" => secondary_linked_sessions(),
        "secondary-skill-files" => secondary_skill_files(),
        "secondary-rule-detail" => secondary_rule_detail(),
        "secondary-hook-detail" => secondary_hook_detail(),
        "secondary-config-read" => secondary_config_read(),
        "tertiary-skill-save" => tertiary_skill_save(),
        "tertiary-hook-delete" => tertiary_hook_delete(),
        "tertiary-prompt-crud" => tertiary_prompt_crud(),
        "tertiary-rule-save" => tertiary_rule_save(),
        "tertiary-settings-save" => tertiary_settings_save(),
        _ => bail!("unknown performance scenario: {scenario}"),
    }?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

fn primary_overview() -> Result<Value> {
    let scratch = Scratch::new("overview")?;
    let transcript_dir = scratch.path().join("transcripts");
    fs::create_dir_all(&transcript_dir)?;
    let mut sessions = Vec::with_capacity(OVERVIEW_SESSIONS);
    let today = Local::now().date_naive();
    for index in 0..OVERVIEW_SESSIONS {
        let date = today - Duration::days((index % 365) as i64);
        let timestamp = format!("{date}T01:00:00Z");
        let path = transcript_dir.join(format!("session-{index:04}.jsonl"));
        let transcript = format!(
            "{{\"timestamp\":\"{timestamp}\",\"type\":\"turn_context\",\"payload\":{{\"model\":\"gpt-perf\"}}}}\n\
             {{\"timestamp\":\"{timestamp}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\"}}}}\n\
             {{\"timestamp\":\"{timestamp}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":100,\"cached_input_tokens\":40,\"output_tokens\":25,\"reasoning_output_tokens\":5,\"total_tokens\":125}}}}}}}}\n\
             {{\"timestamp\":\"{timestamp}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_complete\"}}}}\n"
        );
        fs::write(&path, transcript)?;
        sessions.push(session(index, path, index % 32));
    }
    let store = Store::open(scratch.path().join("overview.sqlite3"))?;
    let scope_key = ScopeKey::new(scratch.path().display().to_string())
        .map_err(|error| anyhow::anyhow!(error))?;
    store.save_sessions_at_for_scope(
        &scope_key,
        &SessionScan {
            sessions: sessions.clone(),
            warnings: Vec::new(),
        },
        1,
    )?;
    let refreshed = store
        .refresh_session_analytics_for_scope_with_progress(&scope_key, &sessions, |_| {})?;
    if refreshed.failed != 0 || refreshed.parsed != OVERVIEW_SESSIONS {
        bail!(
            "overview fixture analytics incomplete: parsed {}, failed {}",
            refreshed.parsed,
            refreshed.failed
        );
    }

    measured(|| {
        let overview = store.overview_analytics_for_scope(&scope_key, None, 365, 365)?;
        let count = overview.days.len();
        Ok((overview, count))
    })
}

fn primary_prompts() -> Result<Value> {
    let scratch = Scratch::new("primary-prompts")?;
    let store = Store::open(scratch.path().join("prompts.sqlite3"))?;
    seed_prompts(&store)?;
    measured(|| {
        let prompts = store.list_prompts()?;
        let count = prompts.len();
        Ok((prompts, count))
    })
}

fn primary_config() -> Result<Value> {
    measured(|| {
        let configs = tendi_core::config::list_agent_configs()?;
        let count = configs.len();
        Ok((configs, count))
    })
}

fn primary_settings() -> Result<Value> {
    let scratch = Scratch::new("primary-settings")?;
    let store = Store::open(scratch.path().join("settings.sqlite3"))?;
    measured(|| Ok((store.app_settings()?, 1)))
}

fn secondary_session_page() -> Result<Value> {
    let scratch = Scratch::new("session-page")?;
    let path = scratch.path().join("transcript.jsonl");
    let mut lines = Vec::with_capacity(400);
    for index in 0..400 {
        lines.push(
            json!({
                "timestamp": "2026-08-14T01:00:00Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": if index % 2 == 0 { "user" } else { "assistant" },
                    "content": [{ "type": "input_text", "text": format!("message-{index:03}-{}", "x".repeat(4096)) }]
                }
            })
            .to_string(),
        );
    }
    fs::write(&path, lines.join("\n"))?;

    measured(|| {
        let page = parse_transcript_page(&path, AgentKind::Codex, None, Some(160))?;
        let count = page.items.len();
        if count != 160 || page.done {
            bail!(
                "unexpected transcript page: {count} items, done={}",
                page.done
            );
        }
        Ok((page, count))
    })
}

fn secondary_session_search() -> Result<Value> {
    let scratch = Scratch::new("session-search")?;
    let transcript_dir = scratch.path().join("transcripts");
    fs::create_dir_all(&transcript_dir)?;
    let mut sessions = Vec::with_capacity(SESSION_SEARCH_SESSIONS);
    for index in 0..SESSION_SEARCH_SESSIONS {
        let path = transcript_dir.join(format!("session-{index:04}.jsonl"));
        let body = format!(
            "session-search-needle fixture-{index:04} {}",
            "x".repeat(4096)
        );
        let line = format!(
            r#"{{"timestamp":"2026-08-14T01:00:00Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":{}}}]}}}}"#,
            serde_json::to_string(&body)?
        );
        fs::write(&path, format!("{line}\n"))?;
        sessions.push(session(index, path, index % 32));
    }

    let store = Store::open(scratch.path().join("session-search.sqlite3"))?;
    let scope_key = ScopeKey::new(scratch.path().display().to_string())
        .map_err(|error| anyhow::anyhow!(error))?;
    store.save_sessions_at_for_scope(
        &scope_key,
        &SessionScan {
            sessions: sessions.clone(),
            warnings: Vec::new(),
        },
        1,
    )?;
    let candidates = sessions
        .iter()
        .take(SESSION_SEARCH_CANDIDATES)
        .map(SessionIdentity::from)
        .collect::<Vec<_>>();

    measured(|| {
        let hits = store.search_sessions_for_scope(&scope_key, "needle", Some(&candidates))?;
        if hits.len() != SESSION_SEARCH_CANDIDATES {
            bail!("unexpected session search hit count: {}", hits.len());
        }
        if hits
            .iter()
            .enumerate()
            .any(|(index, hit)| hit.session.id != format!("perf-session-{index:04}"))
        {
            bail!("session search results changed candidate order");
        }
        let count = hits.len();
        Ok((hits, count))
    })
}

fn secondary_session_10k_soak() -> Result<Value> {
    let scratch = Scratch::new("session-10k-soak")?;
    let store = Store::open(scratch.path().join("session-10k.sqlite3"))?;
    let scope_key = ScopeKey::new(scratch.path().display().to_string())
        .map_err(|error| anyhow::anyhow!(error))?;
    let sessions = (0..SESSION_SOAK_COUNT)
        .map(|index| {
            session(
                index,
                scratch.path().join(format!("transcripts/session-{index:05}.jsonl")),
                index % 128,
            )
        })
        .collect::<Vec<_>>();
    store.save_sessions_at_for_scope(
        &scope_key,
        &SessionScan {
            sessions: sessions.clone(),
            warnings: Vec::new(),
        },
        1,
    )?;

    let candidates = sessions
        .iter()
        .step_by(100)
        .map(SessionIdentity::from)
        .collect::<Vec<_>>();
    measured(|| {
        let listed = store.list_sessions_for_scope(&scope_key)?.sessions;
        if listed.len() != SESSION_SOAK_COUNT {
            bail!("unexpected 10k session count: {}", listed.len());
        }
        let hits = store.search_sessions_for_scope(
            &scope_key,
            "performance session",
            Some(&candidates),
        )?;
        if hits.len() != candidates.len() {
            bail!("unexpected 10k session search hit count: {}", hits.len());
        }
        Ok((
            json!({
                "listed": listed.len(),
                "searched": hits.len(),
                "scopeKey": scope_key,
            }),
            listed.len(),
        ))
    })
}

fn secondary_linked_sessions() -> Result<Value> {
    let scratch = Scratch::new("linked-sessions")?;
    let store = Store::open(scratch.path().join("links.sqlite3"))?;
    let scope_key = ScopeKey::new(scratch.path().display().to_string())
        .map_err(|error| anyhow::anyhow!(error))?;
    let skill_path = scratch.path().join("perf-linked-skill");
    fs::create_dir_all(&skill_path)?;
    let sessions = (0..LINKED_SESSIONS)
        .map(|index| {
            session(
                index,
                scratch.path().join(format!("session-{index}.jsonl")),
                0,
            )
        })
        .collect::<Vec<_>>();
    store.save_sessions_at_for_scope(
        &scope_key,
        &SessionScan {
            sessions: sessions.clone(),
            warnings: Vec::new(),
        },
        1,
    )?;
    for session in &sessions {
        let link = SessionSkillLink {
            session_id: session.id.clone(),
            agent: session.agent,
            session_path: session.path.clone(),
            session_title: session.title.clone(),
            session_project: session.project.clone(),
            session_started_at: session.started_at.clone(),
            session_updated_at: session.updated_at.clone(),
            session_message_count: session.message_count,
            skill_name: "perf-linked-skill".to_string(),
            skill_path: skill_path.clone(),
            skill_agent: Some(AgentKind::Shared),
            skill_scope: Some("project".to_string()),
            evidence_kind: "observed".to_string(),
            evidence_text: "Used perf-linked-skill in this session".to_string(),
            evidence_time: session.updated_at.clone(),
            confidence: "observed".to_string(),
        };
        store.replace_session_skill_links_for_scope(
            &scope_key,
            session,
            &SessionFileState {
                file_mtime: 1,
                file_size: 128,
            },
            &[link],
        )?;
    }

    measured(|| {
        let links = store.skill_session_links_for_scope(&scope_key, "perf-linked-skill")?;
        let count = links.len();
        if count != LINKED_SESSIONS {
            bail!("unexpected linked session count: {count}");
        }
        Ok((links, count))
    })
}

fn secondary_skill_files() -> Result<Value> {
    let scratch = Scratch::new("skill-files")?;
    let skill_dir = scratch.path().join(".agents/skills/perf-secondary-skill");
    let references = skill_dir.join("references");
    fs::create_dir_all(&references)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: perf-secondary-skill\ndescription: performance fixture\n---\n",
    )?;
    for index in 0..SKILL_TREE_FILES {
        fs::write(
            references.join(format!("reference-{index:03}.md")),
            format!("#{index}\n{}", "x".repeat(4096)),
        )?;
    }

    measured(|| {
        let entries = tendi_core::files::list_skill_files(
            scratch.path(),
            "perf-secondary-skill",
            Some(&skill_dir),
        )?;
        let content = tendi_core::files::read_skill_file(
            scratch.path(),
            "perf-secondary-skill",
            "references/reference-299.md",
            Some(&skill_dir),
        )?;
        let count = entries.len();
        Ok((json!({ "entries": entries, "content": content }), count))
    })
}

fn secondary_rule_detail() -> Result<Value> {
    let scratch = Scratch::new("rule-detail")?;
    let path = scratch.path().join("AGENTS.md");
    fs::write(&path, format!("# Rule fixture\n{}", "r".repeat(128 * 1024)))?;
    measured(|| {
        let content = read_rule_file_at_path(&path)?;
        Ok((content, 1))
    })
}

fn secondary_hook_detail() -> Result<Value> {
    let scratch = Scratch::new("hook-detail")?;
    let path = scratch.path().join("hooks.json");
    fs::write(
        &path,
        format!("{{\"fixture\":\"{}\"}}", "h".repeat(192 * 1024)),
    )?;
    measured(|| {
        let content = read_hook_source_at_path(&path, AgentKind::Claude, None, None)?;
        if !content.truncated {
            bail!("hook detail fixture should exercise truncation");
        }
        Ok((content, 1))
    })
}

fn secondary_config_read() -> Result<Value> {
    let configs = tendi_core::config::list_agent_configs()?;
    measured(|| {
        if let Some(config) = configs.iter().find(|config| config.exists) {
            Ok((
                json!({ "config": config, "content": tendi_core::config::read_agent_config(&config.path)? }),
                1,
            ))
        } else {
            let count = configs.len();
            Ok((json!({ "configs": configs, "content": null }), count))
        }
    })
}

fn tertiary_skill_save() -> Result<Value> {
    let scratch = Scratch::new("skill-save")?;
    let skills_root = scratch.path().join(".agents/skills");
    fs::create_dir_all(&skills_root)?;
    for index in 0..SKILL_REFRESH_COUNT {
        let name = format!("perf-gate-skill-{index:03}");
        let skill_dir = skills_root.join(&name);
        fs::create_dir_all(&skill_dir)?;
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: original {index}\n---\n"),
        )?;
    }
    let scan = SkillScan {
        roots: vec![SkillRoot {
            path: skills_root.clone(),
            scope: "project".to_string(),
            agent: AgentKind::Shared,
            plugin_id: None,
            plugin_enabled: None,
        }],
        skills: (0..SKILL_REFRESH_COUNT)
            .map(|index| fixture_skill(&skills_root, index))
            .collect(),
        warnings: Vec::new(),
    };
    let target_name = "perf-gate-skill-120";
    let target = scan
        .skills
        .iter()
        .find(|skill| skill.name == target_name)
        .and_then(|skill| skill.paths.first())
        .context("target skill missing from fixture scan")?;
    let skill_dir = target.path.clone();
    let before = tendi_core::files::read_skill_file(
        scratch.path(),
        target_name,
        "SKILL.md",
        Some(&skill_dir),
    )?;
    let updated = "---\nname: perf-gate-skill-120\ndescription: updated performance fixture\n---\n";

    measured(|| {
        tendi_core::files::save_skill_file(
            scratch.path(),
            target_name,
            "SKILL.md",
            &before.sha256,
            updated,
            Some(&skill_dir),
        )?;
        tendi_core::files::create_skill_file(
            scratch.path(),
            target_name,
            "references/perf-created.md",
            Some(&skill_dir),
        )?;
        tendi_core::files::rename_skill_path(
            scratch.path(),
            target_name,
            "references/perf-created.md",
            "references/perf-renamed.md",
            Some(&skill_dir),
        )?;
        tendi_core::files::delete_skill_path(
            scratch.path(),
            target_name,
            "references/perf-renamed.md",
            Some(&skill_dir),
        )?;
        let refreshed = refresh_skill_scan(scratch.path(), scan, &[target_name.to_string()], &[])?;
        let target = refreshed
            .skills
            .iter()
            .find(|skill| skill.name == target_name)
            .cloned()
            .context("target skill missing after refresh")?;
        Ok((target, 5))
    })
}

fn tertiary_hook_delete() -> Result<Value> {
    let scratch = Scratch::new("hook-delete")?;
    let settings_dir = scratch.path().join(".claude");
    let path = settings_dir.join("settings.json");
    fs::create_dir_all(&settings_dir)?;
    let handlers = (0..HOOK_COUNT)
        .map(|index| json!({ "type": "command", "command": format!("echo perf-{index:03}") }))
        .collect::<Vec<_>>();
    fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "hooks": {
                "PreToolUse": [{ "matcher": "*", "hooks": handlers }]
            }
        }))?,
    )?;
    let scan = scan_hooks(scratch.path())?;
    let source_hooks = scan
        .hooks
        .iter()
        .filter(|hook| hook.path == path)
        .take(HOOK_DELETE_COUNT)
        .collect::<Vec<_>>();
    if source_hooks.len() != HOOK_DELETE_COUNT {
        bail!("hook fixture scan found only {} hooks", source_hooks.len());
    }
    let requests = source_hooks
        .into_iter()
        .map(|hook| HookDeleteRequest {
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
        })
        .collect::<Vec<_>>();

    measured(|| {
        delete_hooks(requests)?;
        let remaining = scan_hooks(scratch.path())?
            .hooks
            .into_iter()
            .filter(|hook| hook.path == path)
            .collect::<Vec<_>>();
        if remaining.len() != HOOK_COUNT - HOOK_DELETE_COUNT {
            bail!("unexpected remaining hook count: {}", remaining.len());
        }
        let count = remaining.len();
        Ok((remaining, count))
    })
}

fn tertiary_prompt_crud() -> Result<Value> {
    let scratch = Scratch::new("prompt-crud")?;
    let store = Store::open(scratch.path().join("prompts.sqlite3"))?;
    let ids = seed_prompts(&store)?;

    measured(|| {
        store.save_prompt(PromptWrite {
            id: Some("perf-prompt-0250".to_string()),
            title: "Updated performance prompt".to_string(),
            tags: vec!["perf".to_string(), "updated".to_string()],
            body: "updated body".to_string(),
        })?;
        let deleted = store.delete_prompts(&ids[..100])?;
        if deleted != 100 {
            bail!("unexpected deleted prompt count: {deleted}");
        }
        let prompts = store.list_prompts()?;
        let count = prompts.len();
        Ok((prompts, count))
    })
}

fn tertiary_rule_save() -> Result<Value> {
    let scratch = Scratch::new("rule-save")?;
    let path = scratch.path().join("AGENTS.md");
    fs::write(&path, format!("# Rule before\n{}", "r".repeat(128 * 1024)))?;
    let before = read_rule_file_at_path(&path)?;
    let after = format!("# Rule after\n{}", "s".repeat(128 * 1024));
    measured(|| {
        let saved = tendi_core::rules::save_rule_file_at_path(&path, &before.sha256, &after)?;
        Ok((saved, 1))
    })
}

fn tertiary_settings_save() -> Result<Value> {
    let scratch = Scratch::new("settings-save")?;
    let store = Store::open(scratch.path().join("settings.sqlite3"))?;
    let mut settings = store.app_settings()?;
    settings.appearance = "dark".to_string();
    settings.terminal = "auto".to_string();
    settings.additional_session_roots = (0..32)
        .map(|index| format!("/tmp/tendi-perf-session-root-{index:02}"))
        .collect();
    measured(|| Ok((store.save_app_settings(settings)?, 1)))
}

fn measured<T, F>(operation: F) -> Result<Value>
where
    T: serde::Serialize,
    F: FnOnce() -> Result<(T, usize)>,
{
    let started = Instant::now();
    let (payload, count) = operation()?;
    let payload = serde_json::to_vec(&payload)?;
    let duration_ms = started.elapsed().as_secs_f64() * 1_000.0;
    Ok(json!({
        "operationMs": duration_ms,
        "payloadBytes": payload.len(),
        "count": count,
    }))
}

fn session(index: usize, path: PathBuf, project_index: usize) -> SessionRecord {
    SessionRecord {
        id: format!("perf-session-{index:04}"),
        agent: AgentKind::Codex,
        title: Some(format!("Performance session {index:04}")),
        project: Some(PathBuf::from(format!(
            "/tmp/tendi-perf-project-{project_index:02}"
        ))),
        repository: None,
        repository_url: None,
        logical_project_id: None,
        logical_project_name: None,
        path,
        started_at: Some("2026-08-14T01:00:00Z".to_string()),
        updated_at: Some("2026-08-14T01:01:00Z".to_string()),
        message_count: Some(40),
        first_user_message: Some("First performance fixture message".to_string()),
        last_user_message: Some("Last performance fixture message".to_string()),
        last_assistant_message: Some("Last performance fixture response".to_string()),
        turn_count: Some(20),
        model: Some("gpt-perf".to_string()),
        mode: Some("default".to_string()),
        approval_mode: Some("never".to_string()),
        is_run_everything: Some(false),
        parent_session_id: None,
        token_usage: None,
    }
}

fn seed_prompts(store: &Store) -> Result<Vec<String>> {
    let mut ids = Vec::with_capacity(PROMPT_COUNT);
    for index in 0..PROMPT_COUNT {
        let prompt = store.save_prompt(PromptWrite {
            id: Some(format!("perf-prompt-{index:04}")),
            title: format!("Performance prompt {index:04}"),
            tags: vec!["perf".to_string(), format!("group-{}", index % 10)],
            body: format!("Prompt body {index}\n{}", "p".repeat(2048)),
        })?;
        ids.push(prompt.id);
    }
    Ok(ids)
}

fn fixture_skill(root: &Path, index: usize) -> SkillRecord {
    let name = format!("perf-gate-skill-{index:03}");
    SkillRecord {
        id: name.clone(),
        installation_id: name.clone(),
        name: name.clone(),
        description: Some(format!("original {index}")),
        tags: Vec::new(),
        dependencies: Vec::new(),
        dependents: Vec::new(),
        dependency_ids: Vec::new(),
        dependent_ids: Vec::new(),
        visibility: SkillVisibility::Auto,
        agents: vec![AgentKind::Shared],
        paths: vec![SkillPath {
            path: root.join(&name),
            root: root.to_path_buf(),
            scope: "project".to_string(),
            agent: AgentKind::Shared,
            install_target: "shared".to_string(),
            source_kind: "local".to_string(),
            source: None,
            source_ref: None,
            source_version: None,
            source_relative_path: None,
            symlink_status: "direct".to_string(),
            update_status: "not-tracked".to_string(),
            sha256: format!("fixture-{index:03}"),
            tags: Vec::new(),
            tendi_visibility: None,
            effective_visibility: SkillVisibility::Auto,
            provider_allow_implicit_invocation: None,
            provider_skill_enabled: None,
            provider_disable_model_invocation: None,
            plugin_id: None,
            plugin_enabled: None,
        }],
        source_summary: "local".to_string(),
        install_targets: vec!["shared".to_string()],
        update_status: "not-tracked".to_string(),
        is_system: false,
        ctime: None,
        mtime: None,
    }
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Result<Self> {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "tendi-perf-{label}-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
