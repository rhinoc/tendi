import { titleValue } from "./strings.ts";
import type { AgentKind } from "./generated/runtime-types.ts";

export type HookRecord = {
  id: string;
  agent: AgentKind;
  event: string;
  matcher?: string | null;
  enabled: boolean;
  needs_review: boolean;
  filter?: string | null;
  hook_type?: string | null;
  command?: string | null;
  url?: string | null;
  prompt?: string | null;
  path: string;
  trust_hash: string;
  read_only_reason?: string | null;
  status_message?: string | null;
};

export type HookItem = { key: string; hook: HookRecord };

export function hookDisplayName(hook: Pick<HookRecord, "event"> | null | undefined): string {
  return hook?.event || "Hook";
}

function requiredString(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const normalized = value.trim();
  return normalized ? normalized : undefined;
}

export function normalizeHook(hook: Record<string, unknown>): HookRecord | undefined {
  const id = requiredString(hook.id);
  const agent = requiredString(hook.agent) as AgentKind | undefined;
  const event = requiredString(hook.event);
  const path = requiredString(hook.path);
  const trustHash = requiredString(hook.trust_hash);
  if (!id || !agent || !event || !path || !trustHash) return undefined;
  if (typeof hook.enabled !== "boolean" || typeof hook.needs_review !== "boolean") return undefined;
  return {
    id,
    agent,
    event,
    matcher: typeof hook.matcher === "string" ? hook.matcher : undefined,
    hook_type: typeof hook.hook_type === "string" ? hook.hook_type : undefined,
    command: typeof hook.command === "string" ? hook.command : undefined,
    url: typeof hook.url === "string" ? hook.url : undefined,
    prompt: typeof hook.prompt === "string" ? hook.prompt : undefined,
    filter: typeof hook.filter === "string" ? hook.filter : undefined,
    status_message: typeof hook.status_message === "string" ? hook.status_message : undefined,
    enabled: hook.enabled,
    path,
    trust_hash: trustHash,
    needs_review: hook.needs_review,
    read_only_reason: typeof hook.read_only_reason === "string" ? hook.read_only_reason : undefined,
  };
}

export function hookDeleteIdentity(hook: HookRecord | null | undefined): string | undefined {
  return hook?.id || undefined;
}

export function hookEnableDisabledReason(hook: HookRecord | null | undefined): string {
  if (!hook) return "Missing hook source path";
  if (!hookSourcePath(hook)) return "Missing hook source path";
  return hook.read_only_reason ?? "";
}

export function hookReviewDisabledReason(hook: HookRecord | null | undefined): string {
  return hook?.needs_review ? "" : "Hook does not need review";
}

export function hookSelectionTargets(items: HookItem[]) {
  return {
    deletable: items.filter((item) => !hookDeleteDisabledReason(item.hook)),
    enable: items.filter((item) => !hookEnableDisabledReason(item.hook) && !item.hook.enabled),
    disable: items.filter((item) => !hookEnableDisabledReason(item.hook) && Boolean(item.hook.enabled)),
  };
}

export function hookKey(hook: HookRecord | null | undefined, duplicateIndex = 0): string | undefined {
  const base = hookDeleteIdentity(hook);
  if (!base) return undefined;
  return duplicateIndex === 0 ? base : `${base}#${duplicateIndex}`;
}

export function hookItemsFromRows(rows: HookRecord[]): HookItem[] {
  const counts = new Map<string, number>();
  return rows.flatMap((hook) => {
    const base = hookDeleteIdentity(hook);
    if (!base) return [];
    const duplicateIndex = counts.get(base) ?? 0;
    counts.set(base, duplicateIndex + 1);
    const key = hookKey(hook, duplicateIndex);
    return key ? [{ key, hook }] : [];
  });
}

export function hookHandlerText(hook: HookRecord | null | undefined): string {
  return hook?.command ?? hook?.url ?? hook?.prompt ?? "";
}

export function hookTypeLabel(hook: HookRecord | null | undefined): string {
  return hook?.hook_type ? titleValue(hook.hook_type) : "";
}

export function hookTrustHash(hook: HookRecord | null | undefined): string {
  return hook?.trust_hash ?? "";
}

export function hookSearchText(hook: HookRecord | null | undefined): string {
  return [
    hook?.agent,
    hook?.event,
    hook?.matcher,
    hook?.filter,
    hook?.status_message,
    hookTypeLabel(hook),
    hookHandlerText(hook),
    hook?.path,
    hookTrustHash(hook),
  ].map((value) => `${value ?? ""}`.toLowerCase()).join(" ");
}

export function filterHooks(items: HookItem[], query: string): HookItem[] {
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) return items;
  return items.filter((item) => hookSearchText(item.hook).includes(normalizedQuery));
}

export function hookSourcePath(hook: HookRecord | null | undefined): string {
  return hook?.path ?? "";
}

export function hookDeleteDisabledReason(hook: HookRecord | null | undefined): string {
  if (!hookSourcePath(hook)) return "Missing hook source path";
  const readOnlyReason = hook?.read_only_reason;
  if (readOnlyReason) return readOnlyReason;
  return "";
}

export type HookMutationDelta = {
  updated?: unknown[];
  deleted?: unknown[];
};

export function isHookMutationDelta(value: unknown): value is HookMutationDelta {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  return ("updated" in record || "deleted" in record) && !("error" in record);
}
