use std::{fs, path::Path};

use anyhow::Result;
use anyhow::bail;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use walkdir::WalkDir;

use crate::{
    sessions::extract_cursor_blob_model,
    transcript::{TranscriptItem, collect_generic_item},
};

use super::*;

pub(super) struct CursorProvider;

pub(super) fn matches_name(normalized: &str) -> bool {
    normalized == "cursor"
}

pub(crate) fn scan_project_mcp(
    root: &Path,
    servers: &mut Vec<McpServerRecord>,
    warnings: &mut Vec<String>,
) {
    crate::mcp::scan_project_mcp(root, AgentKind::Cursor, servers, warnings);
    if !root.is_dir() {
        return;
    }
    for entry in WalkDir::new(root)
        .follow_links(true)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "SERVER_METADATA.json")
    {
        let path = entry.path();
        let scope = entry_scope(root, &path);
        scan_cursor_metadata_mcp(&path, &scope, servers, warnings);
    }
}

fn entry_scope(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .and_then(|path| path.components().next())
        .and_then(|component| component.as_os_str().to_str())
        .unwrap_or("project")
        .to_string()
}

fn scan_cursor_metadata_mcp(
    path: &Path,
    scope: &str,
    servers: &mut Vec<McpServerRecord>,
    warnings: &mut Vec<String>,
) {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return;
        }
    };
    let value = match serde_json::from_str::<Value>(&text) {
        Ok(value) => value,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return;
        }
    };
    let Some(identifier) = value.get("serverIdentifier").and_then(Value::as_str) else {
        return;
    };
    let name = value
        .get("serverName")
        .and_then(Value::as_str)
        .unwrap_or(identifier);
    let status_path = path.parent().map(|parent| parent.join("STATUS.md"));
    let status = status_path
        .as_ref()
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|text| {
            if text.to_ascii_lowercase().contains("needs authentication") {
                "needs-auth"
            } else {
                "configured"
            }
        })
        .unwrap_or("configured");
    servers.push(McpServerRecord {
        agent: AgentKind::Cursor,
        name: name.to_string(),
        scope: scope.to_string(),
        transport: "cursor-plugin".to_string(),
        status: status.to_string(),
        path: path.to_path_buf(),
    });
}

pub(super) fn apply_config_profile(command: &mut SessionCommand, profile: &str) -> Result<()> {
    let path = crate::config::config_profile_path(crate::skills::AgentKind::Cursor, profile)?;
    if !path.is_file() {
        bail!("Cursor profile not found: {profile}");
    }
    let config_dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cursor profile directory is unavailable"))?;
    command.env.push((
        "CURSOR_CONFIG_DIR".to_string(),
        config_dir.display().to_string(),
    ));
    Ok(())
}

pub(super) fn parse_transcript(value: &Value, items: &mut Vec<TranscriptItem>) {
    collect_generic_item(value, items);
}

pub(super) fn append_transcript_metadata(
    path: &Path,
    items: &mut Vec<TranscriptItem>,
) -> Result<()> {
    let Some(store_path) = find_cursor_store_db(path) else {
        return Ok(());
    };
    append_transcript_metadata_from_store(&store_path, items)?;
    Ok(())
}

pub(super) fn append_transcript_metadata_from_store(
    path: &Path,
    items: &mut Vec<TranscriptItem>,
) -> Result<()> {
    let Ok(connection) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Ok(());
    };
    let Ok(mut statement) = connection.prepare(
        "select data from blobs
         where instr(data, 'modelName') > 0
         order by rowid",
    ) else {
        return Ok(());
    };
    let Ok(rows) = statement.query_map([], |row| row.get::<_, Vec<u8>>(0)) else {
        return Ok(());
    };

    let mut models = Vec::new();
    for bytes in rows.filter_map(Result::ok) {
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let Some(model) = extract_cursor_blob_model(&value) else {
            continue;
        };
        models.push(model);
    }

    insert_cursor_model_configs(items, &models);
    Ok(())
}

pub(super) fn may_contain_search_message(line: &str) -> bool {
    let hint = crate::transcript::search_json_hint(line);
    matches!(
        crate::transcript::json_string_hint(hint, "\"role\"")
            .or_else(|| crate::transcript::json_string_hint(hint, "\"type\"")),
        Some("user" | "assistant")
    )
}

pub(super) fn find_cursor_store_db(path: &Path) -> Option<std::path::PathBuf> {
    let session_id = path.file_stem()?.to_str()?.trim();
    if session_id.is_empty() {
        return None;
    }
    let chats_root = dirs::home_dir()?.join(".cursor/chats");
    let entries = std::fs::read_dir(chats_root).ok()?;
    entries.filter_map(Result::ok).find_map(|entry| {
        let candidate = entry.path().join(session_id).join("store.db");
        candidate.is_file().then_some(candidate)
    })
}

fn insert_cursor_model_configs(items: &mut Vec<TranscriptItem>, models: &[String]) {
    if models.is_empty() {
        return;
    }
    let assistant_positions = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| (item.kind == "assistant").then_some(index))
        .collect::<Vec<_>>();
    let mut previous_model: Option<&str> = None;
    let model_count = models.len();
    let assistant_count = assistant_positions.len();
    let mut insertions = Vec::new();
    for (sample_index, model) in models.iter().enumerate() {
        if previous_model == Some(model.as_str()) {
            continue;
        }
        previous_model = Some(model);
        let assistant_index = sample_index
            .saturating_mul(assistant_count)
            .checked_div(model_count)
            .unwrap_or(0);
        let item_index = assistant_positions
            .get(assistant_index)
            .copied()
            .unwrap_or(items.len());
        insertions.push((item_index, model.clone()));
    }
    let mut offset = 0usize;
    for (item_index, model) in insertions {
        items.insert(
            item_index + offset,
            model_config_item(Some(model), None, None),
        );
        offset += 1;
    }
}

fn model_config_item(
    model: Option<String>,
    effort: Option<String>,
    time: Option<String>,
) -> TranscriptItem {
    let body = [
        model.as_ref().map(|value| format!("Model: {value}")),
        effort.as_ref().map(|value| format!("Effort: {value}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    TranscriptItem {
        kind: "model_config".to_string(),
        body,
        tag: None,
        time,
        command: None,
        result: None,
        duration_ms: None,
        linked_session_id: None,
        model,
        effort,
        call_id: None,
        started_at_ms: None,
    }
}
impl super::AgentProvider for CursorProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Cursor
    }

    fn storage_key(&self) -> &'static str {
        "cursor"
    }

    fn discoverable(&self) -> bool {
        true
    }

    fn global_skill_root(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".cursor/skills"))
    }

    fn config_profile_path(&self, home: &Path, _codex_home: &Path, name: &str) -> Option<PathBuf> {
        Some(
            home.join(".cursor/tendi-profiles")
                .join(name)
                .join("cli-config.json"),
        )
    }

    fn config_profile_format(&self) -> Option<&'static str> {
        Some("json")
    }

    fn config_files(&self, home: &Path, _codex_home: &Path) -> Vec<crate::config::AgentConfigFile> {
        let mut configs = vec![crate::config::AgentConfigFile {
            agent: self.kind(),
            label: "Cursor".to_string(),
            path: home.join(".cursor/cli-config.json"),
            format: "json".to_string(),
            exists: home.join(".cursor/cli-config.json").is_file(),
            profile: None,
        }];
        configs.extend(crate::config::cursor_profile_configs_for_root(
            &home.join(".cursor/tendi-profiles"),
        ));
        configs
    }

    fn config_order(&self) -> usize {
        2
    }

    fn matches_name(&self, normalized: &str) -> bool {
        matches_name(normalized)
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("Cursor")
    }

    fn executable_names(&self) -> &'static [&'static str] {
        &["cursor"]
    }

    fn config_dir(&self, ctx: &ProviderContext) -> Option<PathBuf> {
        ctx.home.as_ref().map(|home| home.join(".cursor"))
    }

    fn skill_roots(&self, ctx: &ProviderContext) -> Vec<SkillRoot> {
        let mut roots = Vec::new();
        if let Some(home) = &ctx.home {
            push_skill_root(
                &mut roots,
                home.join(".cursor/skills"),
                "global",
                self.kind(),
            );
        }
        for dir in ctx.project_dirs() {
            push_skill_root(
                &mut roots,
                dir.join(".cursor/skills"),
                "project",
                self.kind(),
            );
        }
        roots
    }

    fn scan_sessions(
        &self,
        ctx: &ProviderContext,
        sessions_out: &mut Vec<SessionRecord>,
        _warnings: &mut Vec<String>,
        cache: Option<&SessionScanCache>,
    ) -> Result<()> {
        if let Some(home) = &ctx.home {
            sessions::scan_cursor_meta(
                &home.join(".cursor/acp-sessions"),
                sessions_out,
                self.kind(),
                cache,
            );
            sessions::scan_cursor_meta(
                &home.join(".cursor/chats"),
                sessions_out,
                self.kind(),
                cache,
            );
            sessions::scan_cursor_agent_transcripts(
                &home.join(".cursor/projects"),
                sessions_out,
                cache,
            );
        }
        Ok(())
    }

    fn session_roots(&self, ctx: &ProviderContext) -> Vec<PathBuf> {
        let Some(home) = &ctx.home else {
            return Vec::new();
        };
        vec![
            home.join(".cursor/acp-sessions"),
            home.join(".cursor/chats"),
            home.join(".cursor/projects"),
        ]
    }

    fn scan_rules(
        &self,
        ctx: &ProviderContext,
        rules_out: &mut Vec<RuleRecord>,
        warnings: &mut Vec<String>,
        order: &mut usize,
    ) {
        for dir in ctx.project_dirs() {
            rules::add_rule_file(
                rules_out,
                warnings,
                order,
                self.kind(),
                "AGENTS.md",
                "project",
                dir.join("AGENTS.md"),
            );
            rules::add_rule_tree(
                rules_out,
                warnings,
                order,
                self.kind(),
                "cursor-rule",
                "project",
                &dir.join(".cursor/rules"),
                Some("mdc"),
                6,
            );
        }
    }

    fn resume_session_command(&self, session: &SessionRecord) -> Option<SessionCommand> {
        let project = absolute_project(session);
        let mut args = vec![
            "agent".to_string(),
            "--resume".to_string(),
            session.id.clone(),
        ];
        if let Some(project) = project.as_ref() {
            args.push("--workspace".to_string());
            args.push(project.display().to_string());
        }
        Some(SessionCommand {
            executable: "cursor".to_string(),
            args,
            cwd: project,
            env: Vec::new(),
        })
    }

    fn parse_transcript_value(&self, value: &Value, items: &mut Vec<TranscriptItem>) {
        parse_transcript(value, items);
    }

    fn config_profile_key(&self) -> Option<&'static str> {
        Some("cursor")
    }

    fn apply_config_profile(&self, command: &mut SessionCommand, profile: &str) -> Result<()> {
        apply_config_profile(command, profile)
    }

    fn append_transcript_metadata(
        &self,
        path: &Path,
        items: &mut Vec<TranscriptItem>,
    ) -> Result<()> {
        append_transcript_metadata(path, items)
    }

    fn append_transcript_metadata_from_store(
        &self,
        path: &Path,
        items: &mut Vec<TranscriptItem>,
    ) -> Result<()> {
        append_transcript_metadata_from_store(path, items)
    }

    fn transcript_metadata_store_path(&self, path: &Path) -> Option<PathBuf> {
        find_cursor_store_db(path)
    }

    fn transcript_search_hint(&self, line: &str) -> bool {
        may_contain_search_message(line)
    }

    fn transcript_cacheable(&self) -> bool {
        false
    }

    fn recognizes_transcript(&self, value: &Value) -> bool {
        value.get("role").and_then(Value::as_str).is_some() && value.get("message").is_some()
    }

    fn session_supports_append_cache(&self) -> bool {
        true
    }

    fn parse_analytics_line(&self, line: &str, record: &mut SessionAnalyticsRecord) {
        crate::analytics::parse_message_line(line, record);
    }

    fn extract_skill_tool_payloads<'a>(&self, value: &'a Value) -> Vec<(&'a Value, Evidence)> {
        crate::providers::shared::generic_tool_payloads(value)
    }

    fn infer_session_project(&self, path: &Path, project: Option<PathBuf>) -> Option<PathBuf> {
        project.or_else(|| sessions::cursor_project_from_transcript_path(path))
    }

    fn infer_meta_project(&self, value: &Value) -> Option<PathBuf> {
        sessions::cursor_project_from_meta(Some(value))
    }

    fn session_project_aliases(&self, path: &str) -> Vec<String> {
        crate::storage::cursor_codex_worktree_alias(path)
            .into_iter()
            .collect()
    }

    fn scan_explicit_session_path(
        &self,
        path: &Path,
        sessions: &mut Vec<SessionRecord>,
        cache: Option<&SessionScanCache>,
    ) -> bool {
        if sessions::is_cursor_meta_file(path) {
            sessions::scan_cursor_meta_file(path, sessions, self.kind(), cache);
            true
        } else if path.file_name().and_then(|name| name.to_str()) == Some("store.db") {
            sessions::scan_cursor_store_file(path, sessions, self.kind(), cache);
            true
        } else {
            false
        }
    }

    fn scan_mcp(
        &self,
        ctx: &ProviderContext,
        servers: &mut Vec<McpServerRecord>,
        warnings: &mut Vec<String>,
    ) -> Result<()> {
        if let Some(home) = &ctx.home {
            crate::mcp::scan_json_mcp(
                &home.join(".cursor/cli-config.json"),
                self.kind(),
                "global",
                servers,
                warnings,
            );
            scan_project_mcp(&home.join(".cursor/projects"), servers, warnings);
        }
        for ancestor in ctx.project_dirs() {
            let scope = ancestor.display().to_string();
            crate::mcp::scan_json_mcp(
                &ancestor.join(".cursor/mcp.json"),
                self.kind(),
                &scope,
                servers,
                warnings,
            );
        }
        Ok(())
    }

    fn managed_hook_path(&self, path: &Path) -> bool {
        let path = path.to_string_lossy();
        path.starts_with("/etc/cursor/") || path.starts_with("/Library/Application Support/Cursor/")
    }

    fn scan_hooks(
        &self,
        ctx: &ProviderContext,
        scanned_files: &mut HashSet<PathBuf>,
        hooks: &mut Vec<HookRecord>,
        warnings: &mut Vec<String>,
    ) {
        if let Some(home) = &ctx.home {
            crate::hooks::scan_hook_file_once(
                &home.join(".cursor/hooks.json"),
                self.kind(),
                scanned_files,
                hooks,
                warnings,
            );
        }
        for path in [
            PathBuf::from("/etc/cursor/hooks.json"),
            PathBuf::from("/Library/Application Support/Cursor/hooks.json"),
        ] {
            crate::hooks::scan_hook_file_once(&path, self.kind(), scanned_files, hooks, warnings);
        }
        for ancestor in ctx.project_dirs() {
            crate::hooks::scan_hook_file_once(
                &ancestor.join(".cursor/hooks.json"),
                self.kind(),
                scanned_files,
                hooks,
                warnings,
            );
        }
    }

    fn review_hook(&self, hook: &HookRecord) -> Result<()> {
        crate::hooks::review_hook_with_tendi_state(hook)
    }
}
