import { invoke } from "@tauri-apps/api/core";

import type { SkillChangeCommand } from "./skills.ts";

export type CliInstallState = "installed" | "not-installed" | "stale" | "conflict" | "unsupported";

export type CliInstallStatus = {
  state: CliInstallState;
  supported: boolean;
  commandPath: string | null;
  bundledPath: string | null;
  pathConfigured: boolean;
  currentTarget: string | null;
  detail: string;
};

export type BundledSkillStatus = {
  name: string;
  target: string;
  installed: boolean;
  current: boolean;
  promptHandled: boolean;
  shouldPrompt: boolean;
};

export type BundledSkillInstallReport = {
  applied: boolean;
  status: BundledSkillStatus;
};

export enum TauriCommand {
  Scan = "scan",
  BundledSkillStatus = "bundled_skill_status",
  BundledSkillInstall = "bundled_skill_install",
  BundledSkillPromptDismiss = "bundled_skill_prompt_dismiss",
  CliStatus = "cli_status",
  CliInstall = "cli_install",
  CliRemove = "cli_remove",
  AgentsList = "agents_list",
  AgentConfigsList = "agent_configs_list",
  AgentConfigRead = "agent_config_read",
  AgentConfigSave = "agent_config_save",
  ConfigProfileCreate = "config_profile_create",
  ConfigProfileSet = "config_profile_set",
  SkillsList = "skills_list",
  SkillsRefresh = "skills_refresh",
  SessionsList = "sessions_list",
  SessionsProjectMerge = "sessions_project_merge",
  SessionsProjectSplit = "sessions_project_split",
  SessionsScanStart = "sessions_scan_start",
  AnalyticsOverview = "analytics_overview",
  AnalyticsRevision = "analytics_revision",
  SessionAnalytics = "session_analytics",
  SessionsSearch = "sessions_search",
  SessionSkillIndexStatus = "session_skill_index_status",
  SessionSkillIndexRun = "session_skill_index_run",
  SessionSkillLinks = "session_skill_links",
  SkillSessionLinks = "skill_session_links",
  SettingsGet = "settings_get",
  SettingsSave = "settings_save",
  TerminalAppsList = "terminal_apps_list",
  TerminalAppTest = "terminal_app_test",
  EditorAppTest = "editor_app_test",
  SessionResumeInTerminal = "session_resume_in_terminal",
  RulesList = "rules_list",
  RuleFileRead = "rule_file_read",
  RuleFileSave = "rule_file_save",
  HooksList = "hooks_list",
  HookDelete = "hook_delete",
  HookDeleteMany = "hook_delete_many",
  HookSetEnabled = "hook_set_enabled",
  HookReview = "hook_review",
  HookSourceRead = "hook_source_read",
  McpList = "mcp_list",
  PromptsList = "prompts_list",
  PromptSave = "prompt_save",
  PromptsDeleteMany = "prompts_delete_many",
  SessionTranscript = "session_transcript",
  SkillsUpdates = "skills_updates",
  SkillsUpdate = "skills_update",
  SkillsTargets = "skills_targets",
  SkillsAdd = "skills_add",
  SkillFiles = "skill_files",
  SkillFileRead = "skill_file_read",
  SkillFileSave = "skill_file_save",
  SkillFileCreate = "skill_file_create",
  SkillFolderCreate = "skill_folder_create",
  SkillPathRename = "skill_path_rename",
  SkillPathDelete = "skill_path_delete",
  OpenInEditor = "open_in_editor",
  RevealInFinder = "reveal_in_finder",
  OpenUrl = "open_url",
  CheckForUpdates = "check_for_updates",
}

export async function safeInvoke<T = unknown>(command: TauriCommand | SkillChangeCommand, args?: Record<string, unknown>): Promise<T | null> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    console.warn(`tendi command failed: ${command}`, error);
    return null;
  }
}

export async function copyText(value: string | null | undefined): Promise<void> {
  if (!value) return;
  await navigator.clipboard?.writeText(value).catch((error) => {
    console.warn("copy failed", error);
  });
}
