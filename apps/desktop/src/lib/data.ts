import type { PromptRecord } from "./prompt-model.ts";
import type { HookRecord } from "./hooks.ts";
import type { McpRecord } from "./mcp.ts";
import type { RuleRecord } from "./rules.ts";
import type { SessionRecord } from "./sessions.ts";
import type { NormalizedSkill } from "./skills.ts";

/** Canonical desktop catalog. Derived indexes live in DesktopStore. */
export type RuntimeData = {
  agents: Record<string, unknown>[];
  skills: NormalizedSkill[];
  prompts: PromptRecord[];
  sessions: SessionRecord[];
  rules: RuleRecord[];
  hooks: HookRecord[];
  mcp: McpRecord[];
};

export function emptyRuntimeData(): RuntimeData {
  return { agents: [], skills: [], prompts: [], sessions: [], rules: [], hooks: [], mcp: [] };
}
