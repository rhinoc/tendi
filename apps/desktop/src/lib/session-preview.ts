import { transcriptItemType, type TranscriptItem } from "./transcript.ts";
import type { SessionRecord } from "./sessions.ts";

export type RecentSessionPreview = {
  title: string;
  userLast: string;
  assistantLast: string;
};

type PreviewSession = Pick<SessionRecord, "title">;
type PreviewItem = Pick<TranscriptItem, "type" | "kind" | "body">;

function compactSessionText(value: string): string {
  return value
    .replace(/<\/?image[^>\n]*>/gi, " ")
    .replace(/!\[[^\]]*\]\([^)]*\)/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function compactPreviewText(value: string): string {
  const body = value.trim();
  return compactSessionText(body) || (/<\/?image/i.test(body) ? "Image" : "");
}

function previewMessageText(item: PreviewItem): string {
  return compactPreviewText(`${item.body ?? ""}`);
}

export function summarizeSessionPreview(session: PreviewSession, items: PreviewItem[]): RecentSessionPreview {
  const userMessages = items
    .filter((item) => transcriptItemType(item) === "user")
    .map(previewMessageText)
    .filter(Boolean);
  const assistantMessages = items
    .filter((item) => transcriptItemType(item) === "assistant")
    .map(previewMessageText)
    .filter(Boolean);

  return {
    title: userMessages[0] || compactSessionText(session.title),
    userLast: userMessages.at(-1) || "—",
    assistantLast: assistantMessages.at(-1) || "—",
  };
}

export function summarizeSessionPreviewRecord(
  session: Pick<SessionRecord, "title" | "firstUserMessage" | "lastUserMessage" | "lastAssistantMessage">,
): RecentSessionPreview | null {
  const firstUser = compactPreviewText(`${session.firstUserMessage ?? ""}`);
  const lastUser = compactPreviewText(`${session.lastUserMessage ?? ""}`);
  const lastAssistant = compactPreviewText(`${session.lastAssistantMessage ?? ""}`);
  if (!firstUser && !lastUser && !lastAssistant) return null;
  return {
    title: firstUser || compactSessionText(session.title),
    userLast: lastUser || "—",
    assistantLast: lastAssistant || "—",
  };
}
