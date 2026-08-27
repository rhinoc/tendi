import { invoke } from "@tauri-apps/api/core";

import type { SkillChangeCommand } from "./skills.ts";
import { logger } from "./logger.ts";

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

export type UpdateCheckResult = {
  status: "up-to-date" | "available" | "busy";
  version?: string | null;
  body?: string | null;
};

export type DesktopUpdateState = {
  status: "idle" | "checking" | "up-to-date" | "available" | "installing" | "error";
  version?: string;
  body?: string;
  error?: string;
};

export const UPDATE_AVAILABLE_EVENT = "tendi://update-available";

export enum TauriCommand {
  BundledSkillStatus = "bundled_skill_status",
  BundledSkillInstall = "bundled_skill_install",
  BundledSkillRemove = "bundled_skill_remove",
  BundledSkillPromptDismiss = "bundled_skill_prompt_dismiss",
  CliStatus = "cli_status",
  CliInstall = "cli_install",
  CliRemove = "cli_remove",
  AgentsList = "agents_list",
  OverviewCount = "overview_count",
  AgentConfigsList = "agent_configs_list",
  AgentConfigWatch = "agent_config_watch",
  AgentConfigRead = "agent_config_read",
  AgentConfigSave = "agent_config_save",
  AgentConfigsDeleteMany = "agent_configs_delete_many",
  ConfigProfileCreate = "config_profile_create",
  ConfigProfileSet = "config_profile_set",
  SkillsList = "skills_list",
  SkillsRefresh = "skills_refresh",
  SessionsList = "sessions_list",
  SessionsScanStart = "sessions_scan_start",
  AnalyticsOverview = "analytics_overview",
  AnalyticsRevision = "analytics_revision",
  SessionsSearch = "sessions_search",
  SessionSkillIndexStatus = "session_skill_index_status",
  SessionSkillIndexRun = "session_skill_index_run",
  SessionSkillLinks = "session_skill_links",
  SkillSessionLinks = "skill_session_links",
  SettingsGet = "settings_get",
  SettingsSave = "settings_save",
  SessionProjectsList = "session_projects_list",
  ProjectScanScopesList = "project_scan_scopes_list",
  ProjectScanScopesSave = "project_scan_scopes_save",
  ProjectsList = "projects_list",
  ProjectsScan = "projects_scan",
  AppIconSet = "app_icon_set",
  TerminalAppsList = "terminal_apps_list",
  TerminalAppTest = "terminal_app_test",
  EditorAppTest = "editor_app_test",
  SessionResumeTarget = "session_resume_target",
  SessionResumeInTerminal = "session_resume_in_terminal",
  RulesList = "rules_list",
  RuleFileRead = "rule_file_read",
  RuleFileSave = "rule_file_save",
  RuleFileDeleteMany = "rule_file_delete_many",
  HooksList = "hooks_list",
  HookDelete = "hook_delete",
  HookDeleteMany = "hook_delete_many",
  HookSetEnabled = "hook_set_enabled",
  HookReview = "hook_review",
  HookSourceRead = "hook_source_read",
  McpList = "mcp_list",
  McpSetEnabled = "mcp_set_enabled",
  PromptsList = "prompts_list",
  PromptSave = "prompt_save",
  PromptsDeleteMany = "prompts_delete_many",
  SessionTranscript = "session_transcript",
  SessionTranscriptLocator = "session_transcript_locator",
  SessionTranscriptSearch = "session_transcript_search",
  SkillsUpdates = "skills_updates",
  SkillsTargets = "skills_targets",
  SkillsBackupStatus = "skills_backup_status",
  SkillsBackupConfigure = "skills_backup_configure",
  SkillsBackupSync = "skills_backup_sync",
  SkillsBackupNow = "skills_backup_now",
  SkillsBackupVersions = "skills_backup_versions",
  SkillsBackupRestore = "skills_backup_restore",
  SkillsBackupAdopt = "skills_backup_adopt",
  SkillsBackupDisconnect = "skills_backup_disconnect",
  SkillsMarketplaceSearch = "skills_marketplace_search",
  SkillsAdd = "skills_add",
  SkillsAddPreviewRead = "skills_add_preview_read",
  SkillsDistribute = "skills_distribute",
  SkillsRemoveLocations = "skills_remove_locations",
  SkillFiles = "skill_files",
  SkillFileRead = "skill_file_read",
  SkillFileSave = "skill_file_save",
  SkillFileCreate = "skill_file_create",
  SkillFolderCreate = "skill_folder_create",
  SkillPathRename = "skill_path_rename",
  SkillPathDelete = "skill_path_delete",
  OpenInEditor = "open_in_editor",
  RevealInFinder = "reveal_in_finder",
  LogsExport = "logs_export",
  OpenUrl = "open_url",
  CheckForUpdates = "check_for_updates",
  InstallUpdate = "install_update",
}

type TauriWindow = Window & {
  __TAURI_INTERNALS__?: {
    invoke?: unknown;
    transformCallback?: unknown;
  };
};

export function isTauriRuntime(): boolean {
  if (typeof window === "undefined") return false;
  const internals = (window as TauriWindow).__TAURI_INTERNALS__;
  return typeof internals?.invoke === "function" && typeof internals.transformCallback === "function";
}

type WebInvokeResponse<T> = {
  ok: boolean;
  result?: T;
  error?: string | { code?: string; message?: string; data?: unknown };
};

export class DaemonCommandError extends Error {
  readonly code: string;
  readonly data: unknown;

  constructor(code: string, message: string, data?: unknown) {
    super(message);
    this.name = "DaemonCommandError";
    this.code = code;
    this.data = data;
  }
}

export type DaemonEvent = {
  id: number;
  event: string;
  payload: unknown;
};

type DaemonEventHandler = (event: DaemonEvent) => void;

const DAEMON_COMMANDS = new Set<string>([
  "agents_list",
  "overview_count",
  "bundled_skill_status",
  "bundled_skill_install",
  "bundled_skill_remove",
  "bundled_skill_prompt_dismiss",
  "agent_configs_list",
  "agent_config_watch",
  "agent_config_read",
  "agent_config_save",
  "agent_configs_delete_many",
  "config_profile_create",
  "config_profile_set",
  "skills_list",
  "skills_refresh",
  "skills_targets",
  "skills_backup_status",
  "skills_backup_configure",
  "skills_backup_sync",
  "skills_backup_now",
  "skills_backup_versions",
  "skills_backup_restore",
  "skills_backup_adopt",
  "skills_backup_disconnect",
  "skills_add",
  "skills_add_preview_read",
  "skills_distribute",
  "skills_remove_locations",
  "skills_set",
  "skills_wrap",
  "skills_updates",
  "skills_updates_cancel",
  "skills_update",
  "skills_update_many",
  "skills_delete_many",
  "skills_marketplace_search",
  "skill_files",
  "skill_file_read",
  "skill_file_save",
  "skill_file_create",
  "skill_folder_create",
  "skill_path_rename",
  "skill_path_delete",
  "sessions_list",
  "sessions_scan_start",
  "sessions_search",
  "analytics_overview",
  "analytics_revision",
  "session_skill_index_status",
  "session_skill_index_run",
  "session_skill_links",
  "skill_session_links",
  "settings_get",
  "settings_save",
  "session_projects_list",
  "project_scan_scopes_list",
  "project_scan_scopes_save",
  "projects_list",
  "projects_scan",
  "terminal_apps_list",
  "rules_list",
  "rule_file_read",
  "rule_file_save",
  "rule_file_delete_many",
  "hooks_list",
  "hook_delete",
  "hook_delete_many",
  "hook_set_enabled",
  "hook_review",
  "hook_source_read",
  "mcp_list",
  "mcp_set_enabled",
  "prompts_list",
  "prompt_save",
  "prompts_delete_many",
  "session_transcript",
  "session_transcript_locator",
  "session_transcript_search",
]);

// These commands are native desktop lifecycle/OS integration, not business
// operations. Web must fail explicitly instead of sending an unsupported RPC
// to the daemon and turning METHOD_NOT_FOUND into a silent null fallback.
const DESKTOP_ONLY_COMMANDS = new Set<string>([
  "app_icon_set",
  "cli_status",
  "cli_install",
  "cli_remove",
  "terminal_app_test",
  "editor_app_test",
  "session_resume_target",
  "session_resume_in_terminal",
  "open_in_editor",
  "reveal_in_finder",
  "logs_export",
  "open_url",
  "check_for_updates",
  "install_update",
]);

function daemonError(
  error: WebInvokeResponse<unknown>["error"],
  fallbackMessage: string,
): DaemonCommandError {
  if (typeof error === "string") {
    return new DaemonCommandError("DAEMON_ERROR", error || fallbackMessage);
  }
  return new DaemonCommandError(
    error?.code || "DAEMON_ERROR",
    error?.message || error?.code || fallbackMessage,
    error?.data,
  );
}

async function invokeWeb<T>(command: TauriCommand | SkillChangeCommand, args?: Record<string, unknown>): Promise<T> {
  const response = await fetch("/__tendi/invoke", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ command, args }),
  });
  const payload = await response.json() as WebInvokeResponse<T>;
  if (!response.ok || !payload.ok) {
    throw daemonError(payload.error, `Web bridge request failed (${response.status})`);
  }
  return payload.result as T;
}

export async function invokeCommand<T = unknown>(command: TauriCommand | SkillChangeCommand, args?: Record<string, unknown>): Promise<T> {
  try {
    if (DAEMON_COMMANDS.has(command)) {
      if (!isTauriRuntime()) return invokeWeb<T>(command, args);
      const response = await invoke<WebInvokeResponse<T>>("daemon_invoke", {
        request: { command, args: args ?? {} },
      });
      if (!response.ok) throw daemonError(response.error, "Daemon request failed");
      return response.result as T;
    }
    if (!isTauriRuntime()) {
      if (DESKTOP_ONLY_COMMANDS.has(command)) {
        throw new Error(`Command ${command} is only available in the Tauri desktop runtime`);
      }
      throw new Error(`Command ${command} is not exposed by the shared daemon API`);
    }
    return await invoke<T>(command, args);
  } catch (error) {
    logger.error("tendi command failed", { command, error });
    throw error;
  }
}

export async function safeInvoke<T = unknown>(command: TauriCommand | SkillChangeCommand, args?: Record<string, unknown>): Promise<T | null> {
  try {
    return await invokeCommand<T>(command, args);
  } catch {
    return null;
  }
}

export async function subscribeDaemonEvents(handler: DaemonEventHandler): Promise<() => void> {
  if (!isTauriRuntime()) {
    const controller = new AbortController();
    const response = await fetch("/__tendi/events", {
      headers: { accept: "text/event-stream" },
      signal: controller.signal,
    });
    if (!response.ok || !response.body) {
      throw new Error(`Daemon event stream failed (${response.status})`);
    }
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    let disposed = false;
    const consume = async () => {
      try {
        while (!disposed) {
          const { done, value } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });
          const blocks = buffer.split("\n\n");
          buffer = blocks.pop() || "";
          for (const block of blocks) {
            const event = parseSseEvent(block);
            if (event) handler(event);
          }
        }
      } catch (error) {
        if (!disposed) logger.warn("daemon event stream failed", { error });
      }
    };
    void consume();
    return () => {
      disposed = true;
      controller.abort();
      void reader.cancel();
    };
  }

  const subscriptionId = await invoke<string>("daemon_subscribe_events");
  let disposed = false;
  const consume = async () => {
    try {
      while (!disposed) {
        const event = await invoke<DaemonEvent | null>("daemon_next_event", {
          subscriptionId,
          timeoutMs: 25_000,
        });
        if (event && !disposed) handler(event);
      }
    } catch (error) {
      if (!disposed) logger.warn("daemon event subscription failed", { error });
    }
  };
  void consume();
  return () => {
    disposed = true;
    void invoke("daemon_unsubscribe_events", { subscriptionId });
  };
}

function parseSseEvent(block: string): DaemonEvent | null {
  let id: number | undefined;
  let event = "message";
  const data: string[] = [];
  for (const line of block.split("\n")) {
    if (line.startsWith("id:")) {
      const parsedId = Number(line.slice(3).trim());
      if (!Number.isSafeInteger(parsedId) || parsedId < 0) return null;
      id = parsedId;
    }
    else if (line.startsWith("event:")) event = line.slice(6).trim();
    else if (line.startsWith("data:")) data.push(line.slice(5).trimStart());
  }
  if (id === undefined || data.length === 0) return null;
  try {
    return { id, event, payload: JSON.parse(data.join("\n")) as unknown };
  } catch (error) {
    logger.warn("invalid daemon SSE event", { error });
    return null;
  }
}

export async function copyText(value: string | null | undefined): Promise<void> {
  if (!value) return;
  await navigator.clipboard?.writeText(value).catch((error) => {
    logger.warn("copy failed", { error });
  });
}
