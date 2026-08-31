import { AgentTranscriptFormat, type AgentDefinition } from "./types.ts";

export const sharedAgent: AgentDefinition = {
  id: "shared",
  aliases: ["shared"],
  displayName: "Shared",
  transcriptFormat: AgentTranscriptFormat.Shared,
};
