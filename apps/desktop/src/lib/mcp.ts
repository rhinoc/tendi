export type McpRecord = {
  agent: string;
  name: string;
  scope: string;
  transport: string;
  enabled: boolean;
  status: string;
  path: string;
  trust_hash: string;
  server_path?: string[];
  read_only_reason?: string | null;
};

function requiredString(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const normalized = value.trim();
  return normalized ? normalized : undefined;
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
  return {
    agent,
    name,
    scope,
    transport,
    enabled: row.enabled,
    status,
    path,
    trust_hash: trustHash,
    server_path: Array.isArray(row.server_path)
      ? row.server_path.filter((value): value is string => typeof value === "string")
      : [],
    read_only_reason: typeof row.read_only_reason === "string" ? row.read_only_reason : undefined,
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
