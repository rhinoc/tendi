import { basename, titleValue } from "./strings.ts";

export type HookRecord = {
  agent?: string | null;
  event?: string | null;
  matcher?: string | null;
  enabled?: boolean | null;
  needs_review?: boolean | null;
  filter?: string | null;
  hook_type?: string | null;
  hookType?: string | null;
  command?: string | null;
  url?: string | null;
  prompt?: string | null;
  script?: string | null;
  path?: string | null;
  source?: string | null;
  trust_hash?: string | null;
  trustHash?: string | null;
  hash?: string | null;
  read_only_reason?: string | null;
  readOnlyReason?: string | null;
  status_message?: string | null;
  statusMessage?: string | null;
};

export function hookDeleteIdentity(hook: HookRecord | null | undefined): string {
  return [
    hookSourcePath(hook),
    hook?.agent,
    hook?.event,
    hook?.matcher,
    hook?.hook_type ?? hook?.hookType,
    hookHandlerText(hook),
    hook?.filter,
    hook?.status_message ?? hook?.statusMessage,
  ].map((value) => `${value ?? ""}`).join("|");
}

export function hookKey(hook: HookRecord | null | undefined, duplicateIndex = 0): string {
  const base = hookDeleteIdentity(hook);
  return duplicateIndex === 0 ? base : `${base}#${duplicateIndex}`;
}

export function hookItemsFromRows(rows: HookRecord[]): Array<{ key: string; hook: HookRecord }> {
  const counts = new Map<string, number>();
  return rows.map((hook) => {
    const base = hookDeleteIdentity(hook);
    const duplicateIndex = counts.get(base) ?? 0;
    counts.set(base, duplicateIndex + 1);
    return { key: hookKey(hook, duplicateIndex), hook };
  });
}

export function hookHandlerText(hook: HookRecord | null | undefined): string {
  return hook?.command ?? hook?.url ?? hook?.prompt ?? hook?.script ?? "";
}

export function hookTypeLabel(hook: HookRecord | null | undefined): string {
  const value = hook?.hook_type ?? hook?.hookType;
  if (value) return titleValue(value);
  if (hook?.url) return "HTTP";
  if (hook?.prompt) return "Prompt";
  if (hook?.command ?? hook?.script) return "Command";
  return "Unknown";
}

export function hookSourceTitle(hook: HookRecord | null | undefined): string {
  return basename(hook?.path ?? hook?.source ?? "Hook");
}

export function hookTrustHash(hook: HookRecord | null | undefined): string {
  return `${hook?.trust_hash ?? hook?.trustHash ?? hook?.hash ?? ""}`;
}

export function hookSearchText(hook: HookRecord | null | undefined): string {
  return [
    hook?.agent,
    hook?.event,
    hook?.matcher,
    hook?.filter,
    hook?.status_message ?? hook?.statusMessage,
    hookTypeLabel(hook),
    hookHandlerText(hook),
    hook?.path,
    hook?.source,
    hookTrustHash(hook),
  ].map((value) => `${value ?? ""}`.toLowerCase()).join(" ");
}

export function hookSourcePath(hook: HookRecord | null | undefined): string {
  return `${hook?.path ?? hook?.source ?? ""}`;
}

export function hookDeleteDisabledReason(hook: HookRecord | null | undefined): string {
  const path = hookSourcePath(hook);
  if (!path) return "Missing hook source path";
  const readOnlyReason = hook?.read_only_reason ?? hook?.readOnlyReason;
  if (readOnlyReason) return readOnlyReason;
  if (!path.endsWith(".json") && !path.endsWith(".toml")) {
    return "Deleting hooks from this source type is not supported";
  }
  return "";
}
