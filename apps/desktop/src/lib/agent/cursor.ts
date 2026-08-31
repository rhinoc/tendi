import cursorIcon from "@lobehub/icons-static-svg/icons/cursor.svg";

import { collectCursorItemWithTimestamp, extractTitle, isJsonObject, stringAt } from "../transcript.ts";
import { compareTimestamps } from "../time.ts";
import { AgentTranscriptFormat, type AgentDefinition, type TranscriptParseContext } from "./types.ts";

function cursorEventTimestamp(value: Record<string, unknown>): string | undefined {
  const message = isJsonObject(value.message) ? value.message.content : value.message;
  const content = typeof message === "string"
    ? message
    : isJsonObject(message)
      ? stringAt(message, ["text"]) || stringAt(message, ["content"])
      : Array.isArray(message)
        ? message.map((item) => isJsonObject(item) ? stringAt(item, ["text"]) || stringAt(item, ["content"]) : "").join("\n")
        : "";
  const match = content.match(/<timestamp>\s*([^<]+?)\s*<\/timestamp>/i);
  if (!match) return undefined;
  const timestamp = Date.parse(match[1]);
  return Number.isNaN(timestamp) ? undefined : new Date(timestamp).toISOString();
}

function updateTimeBounds(value: string, context: TranscriptParseContext) {
  if (!context.meta.startedAt || compareTimestamps(value, context.meta.startedAt) < 0) context.meta.startedAt = value;
  if (!context.meta.updatedAt || compareTimestamps(value, context.meta.updatedAt) > 0) context.meta.updatedAt = value;
}

function cursorTranscriptParser(value: Record<string, unknown>, context: TranscriptParseContext) {
  if (value.message === undefined || typeof value.role !== "string") return false;
  const timestamp = cursorEventTimestamp(value);
  if (timestamp) updateTimeBounds(timestamp, context);
  context.meta.project ||= stringAt(value, ["cwd"]);
  context.meta.title ??= extractTitle(value);
  collectCursorItemWithTimestamp(value, context.items, timestamp || stringAt(value, ["timestamp"]));
  return true;
}

export const cursorAgent: AgentDefinition = {
  id: "cursor",
  aliases: ["cursor"],
  displayName: "Cursor",
  trendClass: "agentCursor",
  icon: cursorIcon,
  transcriptFormat: AgentTranscriptFormat.Cursor,
  transcriptParser: cursorTranscriptParser,
};
