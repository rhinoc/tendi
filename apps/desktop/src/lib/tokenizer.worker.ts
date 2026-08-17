import {
  markdownTokenStats,
  transcriptTokenSegments,
} from "./tokenizer.ts";
import type {
  TokenBreakdownSegment,
  TranscriptSkillLink,
  TranscriptTokenItem,
} from "./tokenizer-types.ts";

type TokenizerWorkerRequest =
  | {
      id: number;
      kind: "markdown";
      activePath: string;
      content: string;
      selectionText: string;
    }
  | {
      id: number;
      kind: "transcript";
      items: TranscriptTokenItem[];
      skillLinks: TranscriptSkillLink[];
    };

type TokenizerWorkerResponse =
  | { id: number; kind: TokenizerWorkerRequest["kind"]; type: "result"; segments: TokenBreakdownSegment[] }
  | { id: number; kind: TokenizerWorkerRequest["kind"]; type: "error"; message: string };

type TokenizerWorkerScope = {
  onmessage: ((event: MessageEvent<TokenizerWorkerRequest>) => void) | null;
  postMessage: (message: TokenizerWorkerResponse) => void;
};

const workerScope = self as unknown as TokenizerWorkerScope;

workerScope.onmessage = (event) => {
  const request = event.data;
  try {
    const segments = request.kind === "markdown"
      ? markdownTokenSegments(request.activePath, request.content, request.selectionText)
      : transcriptTokenSegments(request.items, request.skillLinks);
    workerScope.postMessage({
      id: request.id,
      kind: request.kind,
      type: "result",
      segments,
    });
  } catch (error) {
    workerScope.postMessage({
      id: request.id,
      kind: request.kind,
      type: "error",
      message: error instanceof Error ? error.message : String(error),
    });
  }
};

function markdownTokenSegments(activePath: string, content: string, selectionText: string): TokenBreakdownSegment[] {
  const stats = markdownTokenStats(activePath, content, selectionText);
  const segments: TokenBreakdownSegment[] = [];
  if (stats.selection > 0) segments.push({ label: "Selection", value: stats.selection });
  if (stats.isSkillMarkdown) {
    segments.push({ label: "Desc", value: stats.description ?? 0 });
    segments.push({ label: "Content", value: stats.content ?? 0 });
  } else {
    segments.push({ label: "File", value: stats.file });
  }
  return segments;
}
