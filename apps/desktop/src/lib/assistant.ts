import type {
  AssistantAskRequest,
  AssistantContext,
  AssistantMessage,
  JsonObject,
  JsonValue,
} from "./generated/runtime-types.ts";

export const ASSISTANT_CONTEXT_TEXT_LIMIT = 6_000;
export const ASSISTANT_CONTEXT_ITEM_LIMIT = 24;
export const ASSISTANT_PAGE_ROW_LIMIT = 24;
export const ASSISTANT_AGENT_STORAGE_KEY = "tendi.assistant.agent";

export function readAssistantAgent(): string {
  if (typeof window === "undefined") return "";
  try {
    return window.localStorage.getItem(ASSISTANT_AGENT_STORAGE_KEY)?.trim() ?? "";
  } catch {
    return "";
  }
}

export function persistAssistantAgent(agent: string): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(ASSISTANT_AGENT_STORAGE_KEY, agent.trim());
  } catch {
    // The in-memory selection remains the source of truth if browser storage is unavailable.
  }
}

type ContextRecord = Record<string, unknown>;

export type AssistantPromptSuggestion = {
  label: string;
  detail: string;
  message: string;
};

export type AssistantContextInput = {
  pageId: string;
  pageTitle: string;
  filters?: ContextRecord;
  selection?: string;
  selectedContent?: string[];
  pageData?: ContextRecord[];
  pageDataTotal?: number;
  skill?: ContextRecord | null;
  session?: ContextRecord | null;
  tendi?: ContextRecord;
};

export type AssistantRequest = Omit<AssistantAskRequest, "history"> & {
  history: AssistantMessage[];
};

function isJsonValue(value: unknown): value is JsonValue {
  if (value === null || typeof value === "string" || typeof value === "boolean") return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) return value.every(isJsonValue);
  return Boolean(value && typeof value === "object")
    && Object.values(value as Record<string, unknown>).every(isJsonValue);
}

function jsonObject(value: ContextRecord | undefined): JsonObject {
  if (!value) return {};
  return Object.fromEntries(
    Object.entries(value).filter(([, item]) => item !== undefined && isJsonValue(item)),
  ) as JsonObject;
}

function jsonRecord(value: JsonValue | undefined): JsonObject | null {
  return value && typeof value === "object" && !Array.isArray(value) ? value as JsonObject : null;
}

function stringValue(value: JsonValue | undefined): string {
  return typeof value === "string" ? value : "";
}

function pageRowCount(context: AssistantContext): number | null {
  const page = jsonRecord(context.tendi.currentPage);
  const count = page?.rowCount;
  return typeof count === "number" && Number.isSafeInteger(count) && count >= 0 ? count : null;
}

function pageScopeDetail(context: AssistantContext, detail: string, noun: string): string {
  const count = pageRowCount(context);
  return count === null || count === 0 ? detail : `${detail} · ${count} ${noun} in view`;
}

export function assistantPromptSuggestions(context: AssistantContext): AssistantPromptSuggestion[] {
  if (context.selectedContent.length > 0 || context.selection) {
    return [
      {
        label: "Review the selected items",
        detail: "Find conflicts, gaps, and the first action to take.",
        message: "Review the selected items from this page. Identify conflicts, missing fields, and the first concrete action I should take.",
      },
      {
        label: "Explain the selection",
        detail: "Turn the selected rows into a short summary.",
        message: "Explain the selected items in plain language. Group related items and call out anything unusual.",
      },
      {
        label: "Compare the selected items",
        detail: "Show the differences that matter.",
        message: "Compare the selected items. Focus on meaningful differences in status, scope, ownership, and configuration.",
      },
    ];
  }

  const sessionTitle = stringValue(context.session?.title) || "this session";
  const skillName = stringValue(context.skill?.name) || "this skill";

  switch (context.pageId) {
    case "overview":
      return [
        {
          label: "What needs attention?",
          detail: "Prioritize the counts and warnings on Overview.",
          message: "Review the Overview data and prioritize the three most actionable things I should check first. Cite the relevant counts.",
        },
        {
          label: "Compare the agents",
          detail: "Look for coverage gaps in the installed agents.",
          message: "Compare the installed agents in Tendi and point out coverage gaps, unusual differences, or an agent that needs attention.",
        },
        {
          label: "Give me a cleanup plan",
          detail: "Turn the current state into ordered next steps.",
          message: "Turn the current Tendi overview into a short cleanup plan. Order the steps by impact and include the exact area to open.",
        },
      ];
    case "skills":
      return [
        {
          label: "Find skills to update",
          detail: pageScopeDetail(context, "Surface stale skills in this catalog.", "skills"),
          message: "From the Skills rows in the current Tendi context, list skills with available updates and tell me which to update first.",
        },
        {
          label: "Spot skill conflicts",
          detail: "Check visibility, duplicates, and agent coverage.",
          message: "Review the Skills rows for duplicate skills, conflicting visibility, or missing agent coverage. Give me concrete names and fixes.",
        },
        {
          label: "Find unused complexity",
          detail: "Point out skills that can be simplified or removed.",
          message: "Review the Skills catalog and identify overlapping, redundant, or unnecessarily complex skills. Explain the evidence for each suggestion.",
        },
      ];
    case "skillDetail":
      return [
        {
          label: `Explain ${skillName}`,
          detail: "Summarize its purpose, scope, and agent coverage.",
          message: `Explain ${skillName}: what it does, when it is invoked, which agents it covers, and any configuration that affects it.`,
        },
        {
          label: "Check this skill for issues",
          detail: "Look for visibility, dependency, or path problems.",
          message: `Inspect ${skillName} for configuration issues, missing dependencies, visibility conflicts, or stale paths. List concrete fixes.`,
        },
        {
          label: "Show where it is used",
          detail: "Connect this skill to sessions and projects.",
          message: `Find where ${skillName} is used in the available Tendi data and explain what those uses have in common.`,
        },
      ];
    case "sessions":
      if (context.session) {
        return [
          {
            label: "What is the next step?",
            detail: `Continue ${sessionTitle} with a concrete action.`,
            message: `Based on ${sessionTitle}, identify the next concrete step and any unresolved work. Use the session transcript if needed.`,
          },
          {
            label: "Summarize the last turn",
            detail: "Keep the important details and open work.",
            message: `Summarize the last user and assistant turn of ${sessionTitle}, then list unresolved work and decisions still needed.`,
          },
          {
            label: "Check for a better approach",
            detail: "Find wasted effort or a clearer route forward.",
            message: `Review ${sessionTitle} and point out where the approach can be simplified, corrected, or made more reliable.`,
          },
        ];
      }
      return [
        {
          label: "Find sessions to revisit",
          detail: pageScopeDetail(context, "Prioritize recent or unresolved work.", "sessions"),
          message: "From the Sessions rows in the current Tendi context, find the sessions most worth revisiting and explain why.",
        },
        {
          label: "Spot expensive sessions",
          detail: "Use token, message, and turn counts.",
          message: "From the Sessions rows, identify the highest-token or longest sessions and explain what makes them stand out.",
        },
        {
          label: "Group the current work",
          detail: "Find related sessions by project or goal.",
          message: "Group the visible sessions by project or shared goal. Point out duplicates, follow-ups, and sessions that can be closed.",
        },
      ];
    case "rules":
      return [
        {
          label: "Find rules to review",
          detail: pageScopeDetail(context, "Check scope, order, and agent coverage.", "rules"),
          message: "Review the Rules rows in the current Tendi context and list rules with conflicting scope, suspicious order, or incomplete agent coverage.",
        },
        {
          label: "Explain the rule order",
          detail: "Show which rule wins and why.",
          message: "Explain how the visible rules are ordered and where their scopes overlap. Call out any order that could surprise me.",
        },
        {
          label: "Simplify these rules",
          detail: "Suggest safe consolidation opportunities.",
          message: "Find rules that overlap or repeat each other. Suggest a simpler arrangement and state what behavior it preserves.",
        },
      ];
    case "hooks":
      return [
        {
          label: "Find hooks to review",
          detail: pageScopeDetail(context, "Start with flagged or disabled hooks.", "hooks"),
          message: "Review the Hooks rows and list hooks that are flagged, disabled, or likely to fail. Give me the first fix for each.",
        },
        {
          label: "Check hook coverage",
          detail: "Find lifecycle events with no useful automation.",
          message: "Review the visible hooks by agent and event. Point out important lifecycle gaps, duplicate handlers, or uneven coverage.",
        },
        {
          label: "Explain the risky hooks",
          detail: "Translate commands and review state into impact.",
          message: "Explain which visible hooks deserve the most caution and why. Include the event, handler, and concrete mitigation.",
        },
      ];
    case "mcp":
      return [
        {
          label: "Find MCP servers to fix",
          detail: pageScopeDetail(context, "Start with disabled or unhealthy servers.", "servers"),
          message: "Review the MCP rows and list servers that are disabled, unhealthy, or misconfigured. Give me the first fix for each.",
        },
        {
          label: "Explain this MCP setup",
          detail: "Map servers to agents, scope, and transport.",
          message: "Explain the visible MCP setup by agent, scope, and transport. Point out anything inconsistent or unexpectedly unavailable.",
        },
        {
          label: "Reduce MCP clutter",
          detail: "Identify duplicate or unnecessary server entries.",
          message: "Review the MCP rows for duplicate, overlapping, or unnecessary servers. Suggest a concrete cleanup order.",
        },
      ];
    case "prompts":
      return [
        {
          label: "Find prompts to consolidate",
          detail: pageScopeDetail(context, "Spot overlap by title, tags, and content.", "prompts"),
          message: "Review the visible prompts and identify overlapping or redundant prompts. Suggest which ones to merge and why.",
        },
        {
          label: "Improve this prompt library",
          detail: "Find gaps, vague titles, and stale wording.",
          message: "Review the visible prompt library and list vague titles, missing tags, stale wording, or important gaps. Suggest concrete edits.",
        },
        {
          label: "Find a prompt for my task",
          detail: "Search the current library by intent.",
          message: "Based on the visible prompt library, recommend the closest prompt for the task I am likely doing here and explain how to adapt it.",
        },
      ];
    case "config":
      return [
        {
          label: "Explain the active config",
          detail: "Tie profiles to agents and file paths.",
          message: "Explain the active agent configuration in Tendi, including profiles, file paths, and which settings take effect.",
        },
        {
          label: "Find config gaps",
          detail: "Look for missing or inconsistent agent setup.",
          message: "Review the available agent configuration and identify missing, inconsistent, or unexpectedly inactive setup.",
        },
        {
          label: "What should I change first?",
          detail: "Prioritize configuration changes by impact.",
          message: "Based on the current agent configuration, tell me the first change worth making and the behavior it would improve.",
        },
      ];
    case "settings":
      return [
        {
          label: "Check my assistant setup",
          detail: "Verify the selected agent and workspace.",
          message: "Check the current Tendi assistant setup. Verify the selected agent, workspace, and any setting that could prevent useful answers.",
        },
        {
          label: "Explain these settings",
          detail: "Translate the current choices into behavior.",
          message: "Explain the current Tendi settings in terms of the behavior I will see, and call out any choice that has a surprising effect.",
        },
        {
          label: "Suggest a sensible default",
          detail: "Recommend a focused setup for daily work.",
          message: "Based on the current Tendi setup, recommend a sensible default configuration for daily agent work and explain each change.",
        },
      ];
    default:
      return [
        {
          label: "What needs attention?",
          detail: `Prioritize the data visible on ${context.pageTitle || "this page"}.`,
          message: "Review the current Tendi context and prioritize the most actionable things I should check first.",
        },
        {
          label: "Explain what I am seeing",
          detail: "Turn the current page into a clear summary.",
          message: "Explain the current Tendi page in plain language. Focus on the data visible here and call out anything unusual.",
        },
        {
          label: "Give me next steps",
          detail: "Turn the current state into an ordered plan.",
          message: "Based on the current Tendi context, give me a short ordered plan for what to do next.",
        },
      ];
  }
}

export function truncateAssistantText(value: string, limit = ASSISTANT_CONTEXT_TEXT_LIMIT): string {
  const normalized = value.trim();
  if (limit <= 0) return "";
  if (normalized.length <= limit) return normalized;
  const suffix = "\n… [truncated]";
  if (limit <= suffix.length) return suffix.slice(0, limit);
  return `${normalized.slice(0, Math.max(0, limit - suffix.length))}${suffix}`;
}

export function canMarkAssistantReplyRead(
  panelOpen: boolean,
  followingMessages: boolean,
  documentVisible: boolean,
  windowFocused: boolean,
): boolean {
  return panelOpen && followingMessages && documentVisible && windowFocused;
}

export function buildAssistantContext(input: AssistantContextInput): AssistantContext {
  const selectedContent = (input.selectedContent ?? [])
    .map((item) => truncateAssistantText(item))
    .filter(Boolean)
    .slice(0, ASSISTANT_CONTEXT_ITEM_LIMIT);
  const selection = truncateAssistantText(input.selection ?? "");
  if (selection && !selectedContent.includes(selection)) {
    selectedContent.unshift(selection);
    selectedContent.splice(ASSISTANT_CONTEXT_ITEM_LIMIT);
  }
  const pageRows = (input.pageData ?? [])
    .map(jsonObject)
    .filter((row) => Object.keys(row).length > 0)
    .slice(0, ASSISTANT_PAGE_ROW_LIMIT);
  const tendi = jsonObject(input.tendi);
  if (input.pageData) {
    const pageRowTotal = input.pageDataTotal ?? input.pageData.length;
    tendi.currentPage = {
      id: input.pageId,
      rowCount: pageRowTotal,
      shownRowCount: pageRows.length,
      truncated: pageRowTotal > pageRows.length,
      rows: pageRows,
    };
  }
  return {
    pageId: input.pageId,
    pageTitle: input.pageTitle,
    filters: jsonObject(input.filters),
    selection,
    selectedContent,
    skill: input.skill === null ? null : jsonObject(input.skill),
    session: input.session === null ? null : jsonObject(input.session),
    tendi,
  };
}

export function assistantAskRequest(
  message: string,
  history: readonly AssistantMessage[],
  context: AssistantContext,
  agent: string,
  workspace: string,
  conversationId: string,
  persistUserMessage: boolean,
): AssistantRequest {
  return {
    conversationId,
    requestId: newAssistantRequestId(),
    message: message.trim(),
    history: history.slice(-20),
    context,
    agent,
    workspace,
    persistUserMessage,
  };
}

function newAssistantRequestId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `assistant-request-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}
