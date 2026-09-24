import type { JsonValue } from "./generated/runtime-types.ts";

export type McpProbeState = "unknown" | "ready" | "ready-empty" | "needs-auth" | "failed";

export type McpRecord = {
  id: string;
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

export type McpToolParameter = {
  name: string;
  type: string;
  required: boolean;
  description?: string;
  defaultValue?: JsonValue;
  enumValues?: JsonValue[];
};

function jsonObject(value: JsonValue | undefined): Record<string, JsonValue> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, JsonValue>
    : undefined;
}

function mcpSchemaType(value: JsonValue): string {
  const schema = jsonObject(value);
  const type = schema?.type;
  if (typeof type === "string") return type;
  if (Array.isArray(type)) {
    const types = type.filter((item): item is string => typeof item === "string");
    if (types.length > 0) return types.join(" | ");
  }
  if (typeof schema?.$ref === "string") return schema.$ref.split("/").pop() || schema.$ref;
  if (Array.isArray(schema?.enum)) return "enum";
  if (jsonObject(schema?.properties)) return "object";
  return "any";
}

export function mcpToolParameters(tool: McpTool): McpToolParameter[] {
  const schema = jsonObject(tool.input_schema);
  const properties = jsonObject(schema?.properties);
  if (!properties) return [];
  const required = new Set(
    Array.isArray(schema?.required)
      ? schema.required.filter((item): item is string => typeof item === "string")
      : [],
  );
  return Object.entries(properties).map(([name, value]) => {
    const property = jsonObject(value);
    return {
      name,
      type: mcpSchemaType(value),
      required: required.has(name),
      description: typeof property?.description === "string" ? property.description : undefined,
      defaultValue: property && "default" in property ? property.default : undefined,
      enumValues: property && Array.isArray(property.enum) ? property.enum : undefined,
    };
  });
}

export function mcpToolCount(row: Pick<McpRecord, "probe_state" | "tools">): number | undefined {
  if (row.probe_state !== "ready" && row.probe_state !== "ready-empty") return undefined;
  return row.tools.length;
}

export function supportsMcpProbe(transport: string): boolean {
  return transport === "stdio" || transport === "http" || transport === "sse" || transport === "cursor-plugin";
}

export function mcpEnableBlockReason(row: McpRecord | null | undefined): string {
  if (!row) return "Missing MCP server";
  if (row.read_only_reason) return row.read_only_reason;
  if (!mcpSourcePath(row)) return "Missing MCP source path";
  if (!row.trust_hash) return "MCP source hash is unavailable; reload the list";
  return "";
}

export function mcpToggleTargets(rows: readonly McpRecord[], enabled: boolean): McpRecord[] {
  return rows.filter((row) => !mcpEnableBlockReason(row) && row.enabled !== enabled);
}

export function mcpOperationError(result: unknown): string | undefined {
  if (result && typeof result === "object" && "error" in result) {
    const error = (result as { error?: unknown }).error;
    return typeof error === "string" ? error : undefined;
  }
  return undefined;
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
  const id = requiredString(row.id);
  const agent = requiredString(row.agent);
  const name = requiredString(row.name);
  const scope = requiredString(row.scope);
  const transport = requiredString(row.transport);
  const status = requiredString(row.status);
  const path = requiredString(row.path);
  const trustHash = requiredString(row.trust_hash);
  if (!id || !agent || !name || !scope || !transport || !status || !path || !trustHash) return undefined;
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
    id,
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
  return row.id;
}
