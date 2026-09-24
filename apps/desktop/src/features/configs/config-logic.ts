import type { AgentConfigContentResponse } from "../../lib/runtime-gateway.ts";

export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Config operation failed";
}

export function isConfigSnapshot(value: unknown): value is AgentConfigContentResponse {
  if (!value || typeof value !== "object") return false;
  const snapshot = value as Partial<AgentConfigContentResponse>;
  return typeof snapshot.path === "string"
    && typeof snapshot.content === "string"
    && typeof snapshot.sha256 === "string"
    && typeof snapshot.exists === "boolean";
}

export function isConflictMarkerContent(value: string): boolean {
  return /^(?:<{7} |\|{7} |={7}$|>{7} )/m.test(value);
}
