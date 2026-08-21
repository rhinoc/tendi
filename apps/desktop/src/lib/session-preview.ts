import { formatMarkdownLinkLabels, formatTranscriptText } from "./transcript-format.ts";
import type { SessionRecord } from "./sessions.ts";

export type RecentSessionPreview = {
  userLast: string;
  assistantLast: string;
};

function compactSessionText(value: string): string {
  return value
    .replace(/<\/?image\b[^>\n]*(?:>|$)/gi, " ")
    .replace(/!\[[^\]]*\]\([^)]*\)/gi, " ")
    .replace(/\s+/g, " ")
    .trim();
}

export function formatTranscriptPreview(value: string | null | undefined): string {
  const raw = `${value ?? ""}`;
  const compact = compactSessionText(raw);
  if (!compact) return /<\/?image/i.test(raw) || /!\[[^\]]*\]\([^)]*\)/i.test(raw) ? "Image" : "";
  return compact;
}

export function formatSessionTitle(value: string | null | undefined): string {
  return formatTranscriptText(formatMarkdownLinkLabels(value ?? ""));
}

export function summarizeSessionPreviewRecord(
  session: Pick<SessionRecord, "firstUserMessage" | "lastUserMessage" | "lastAssistantMessage">,
): RecentSessionPreview | null {
  const firstUser = session.firstUserMessage ?? "";
  const lastUser = session.lastUserMessage ?? "";
  const lastAssistant = session.lastAssistantMessage ?? "";
  if (!firstUser && !lastUser && !lastAssistant) return null;
  return {
    userLast: lastUser || "—",
    assistantLast: lastAssistant || "—",
  };
}
