import { claudeAgent } from "./claude.ts";
import { codexAgent } from "./codex.ts";
import { cursorAgent } from "./cursor.ts";
import { sharedAgent } from "./shared.ts";
import type { AgentDefinition } from "./types.ts";

export type { AgentDefinition, AgentTranscriptFormat } from "./types.ts";

export const agentDefinitions: readonly AgentDefinition[] = [
  codexAgent,
  claudeAgent,
  cursorAgent,
  sharedAgent,
];

export const fallbackAgents = agentDefinitions
  .filter((definition) => definition.id !== "shared")
  .map((definition) => ({ label: definition.displayName, count: 1 }));

const definitionsByAlias = new Map(
  agentDefinitions.flatMap((definition) => definition.aliases.map((alias) => [alias, definition] as const)),
);

export function agentDefinition(key: string): AgentDefinition | undefined {
  return definitionsByAlias.get(key);
}
