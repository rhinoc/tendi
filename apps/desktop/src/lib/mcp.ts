import type { JsonValue } from "./generated/runtime-types.ts";
import { titleValue } from "./strings.ts";

export type McpProbeState = "unknown" | "ready" | "ready-empty" | "needs-auth" | "failed";

export type McpRecord = {
  agent: string;
  name: string;
  scope: string;
  transport: string;
  enabled: boolean;
  status: string;
  path: string;
  trust_hash: string;
  probe_state: McpProbeState;
  server_path?: string[];
  read_only_reason?: string | null;
  server_name?: string;
  server_title?: string;
  server_version?: string;
  server_description?: string;
  server_website_url?: string;
  probe_error?: string;
  icons: McpIcon[];
  tools: McpTool[];
};

export type McpIcon = {
  src: string;
  mime_type?: string;
  sizes?: string[];
  theme?: string;
};

export type McpTool = {
  name: string;
  title?: string;
  description?: string;
  input_schema?: JsonValue;
  icons: McpIcon[];
};

export function mcpNeedsLogin(status: unknown): boolean {
  return status === "need-login" || status === "needs-auth";
}

export function mcpStatusLabel(status: unknown): string {
  return mcpNeedsLogin(status) ? "Need login" : titleValue(status);
}

export function mcpDisplayName(
  row: Pick<McpRecord, "name"> | null | undefined,
): string {
  return row?.name || "MCP server";
}

function requiredString(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const normalized = value.trim();
  return normalized ? normalized : undefined;
}

function probeState(value: unknown): McpProbeState {
  return value === "ready" || value === "ready-empty" || value === "needs-auth" || value === "failed" ? value : "unknown";
}

function validIconSource(value: unknown): value is string {
  if (typeof value !== "string") return false;
  if (value.startsWith("http://") || value.startsWith("https://")) return true;
  return /^data:image\/(png|jpeg|jpg|svg\+xml|webp)(?:;[^,]*)?,/i.test(value);
}

function isJsonValue(value: unknown): value is JsonValue {
  if (value === null || typeof value === "string" || typeof value === "boolean") return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) return value.every(isJsonValue);
  return Boolean(value && typeof value === "object")
    && Object.values(value as Record<string, unknown>).every(isJsonValue);
}

export function normalizeMcp(row: Record<string, unknown>): McpRecord | undefined {
  const agent = requiredString(row.agent);
  const name = requiredString(row.name);
  const scope = requiredString(row.scope);
  const transport = requiredString(row.transport);
  const status = requiredString(row.status);
  const path = requiredString(row.path);
  const trustHash = requiredString(row.trust_hash);
  if (!agent || !name || !scope || !transport || !status || !path || !trustHash) return undefined;
  if (typeof row.enabled !== "boolean") return undefined;
  const icons = Array.isArray(row.icons)
    ? row.icons.flatMap((value): McpIcon[] => {
      if (!value || typeof value !== "object" || Array.isArray(value)) return [];
      const icon = value as Record<string, unknown>;
      if (!validIconSource(icon.src)) return [];
      return [{
        src: icon.src,
        mime_type: typeof icon.mime_type === "string" ? icon.mime_type : undefined,
        sizes: Array.isArray(icon.sizes) ? icon.sizes.filter((item): item is string => typeof item === "string") : [],
        theme: typeof icon.theme === "string" ? icon.theme : undefined,
      }];
    })
    : [];
  const tools = Array.isArray(row.tools)
    ? row.tools.flatMap((value): McpTool[] => {
      if (!value || typeof value !== "object" || Array.isArray(value)) return [];
      const tool = value as Record<string, unknown>;
      if (typeof tool.name !== "string" || !tool.name.trim()) return [];
      const toolIcons = Array.isArray(tool.icons)
        ? tool.icons.flatMap((iconValue): McpIcon[] => {
          if (!iconValue || typeof iconValue !== "object" || Array.isArray(iconValue)) return [];
          const icon = iconValue as Record<string, unknown>;
          if (!validIconSource(icon.src)) return [];
          return [{ src: icon.src, mime_type: typeof icon.mime_type === "string" ? icon.mime_type : undefined }];
        })
        : [];
      return [{
        name: tool.name,
        title: typeof tool.title === "string" ? tool.title : undefined,
        description: typeof tool.description === "string" ? tool.description : undefined,
        input_schema: isJsonValue(tool.input_schema) ? tool.input_schema : undefined,
        icons: toolIcons,
      }];
    })
    : [];
  return {
    agent,
    name,
    scope,
    transport,
    enabled: row.enabled,
    status,
    path,
    trust_hash: trustHash,
    probe_state: probeState(row.probe_state),
    server_path: Array.isArray(row.server_path)
      ? row.server_path.filter((value): value is string => typeof value === "string")
      : [],
    read_only_reason: typeof row.read_only_reason === "string" ? row.read_only_reason : undefined,
    server_name: typeof row.server_name === "string" ? row.server_name : undefined,
    server_title: typeof row.server_title === "string" ? row.server_title : undefined,
    server_version: typeof row.server_version === "string" ? row.server_version : undefined,
    server_description: typeof row.server_description === "string" ? row.server_description : undefined,
    server_website_url: typeof row.server_website_url === "string" ? row.server_website_url : undefined,
    probe_error: typeof row.probe_error === "string" ? row.probe_error : undefined,
    icons,
    tools,
  };
}

export function isMcpMutationDelta(value: unknown): value is McpMutationDelta {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  return "updated" in record && !("error" in record);
}

export type McpMutationDelta = {
  updated?: unknown[];
};

export function mcpSourcePath(row: McpRecord | null | undefined): string {
  return row?.path ?? "";
}

export function mcpRowKey(row: McpRecord): string {
  return JSON.stringify([row.agent, row.name, row.path, row.server_path ?? []]);
}
