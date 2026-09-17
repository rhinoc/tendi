import { TokenizerKind, TokenizerWorkerResponseType } from "./tokenizer-types.ts";
import type {
  TokenBreakdownSegment,
  TranscriptSkillLink,
  TranscriptTokenItem,
} from "./tokenizer-types.ts";
import {
  markdownTokenParts,
  markdownTokenSegments,
  markdownTokenStatsFromCounts,
  transcriptTokenSegmentsFromCounts,
  transcriptTokenTexts,
} from "./tokenizer-shared.ts";
import { invokeCommand, isTauriRuntime, TauriCommand } from "./tauri.ts";

export type TokenizerWorkerRequest =
  | {
      kind: TokenizerKind.Markdown;
      activePath: string;
      content: string;
      selectionText: string;
    }
  | {
      kind: TokenizerKind.Transcript;
      items: TranscriptTokenItem[];
      skillLinks: TranscriptSkillLink[];
    };

export type TokenizerWorkerResponse =
  | {
      id: number;
      kind: TokenizerWorkerRequest["kind"];
      type: TokenizerWorkerResponseType.Result;
      segments: TokenBreakdownSegment[];
    }
  | {
      id: number;
      kind: TokenizerWorkerRequest["kind"];
      type: TokenizerWorkerResponseType.Error;
      message: string;
    };

export type TokenizerWorkerClient = {
  request: (request: TokenizerWorkerRequest) => number;
  dispose: () => void;
};

async function countNativeTokens(texts: string[]): Promise<number[]> {
  const result = await invokeCommand(TauriCommand.TokenizerCount, { texts });
  if (result.counts.length !== texts.length) {
    throw new Error(`Tokenizer returned ${result.counts.length} counts for ${texts.length} texts`);
  }
  return result.counts;
}

function createNativeTokenizerClient(
  onResponse: (response: TokenizerWorkerResponse) => void,
  onError: (error: Error) => void,
): TokenizerWorkerClient {
  let nextRequestId = 0;
  let disposed = false;
  const emitResponse = (response: TokenizerWorkerResponse) => {
    if (!disposed) onResponse(response);
  };

  const run = async (request: TokenizerWorkerRequest, id: number) => {
    if (request.kind === TokenizerKind.Markdown) {
      const parts = markdownTokenParts(request.activePath, request.content, request.selectionText);
      const counts = await countNativeTokens(parts.texts);
      emitResponse({
        id,
        kind: request.kind,
        type: TokenizerWorkerResponseType.Result,
        segments: markdownTokenSegments(markdownTokenStatsFromCounts(parts, counts)),
      });
      return;
    }

    const texts = transcriptTokenTexts(request.items);
    const counts = await countNativeTokens(texts);
    const countByText = new Map(texts.map((text, index) => [text, counts[index] ?? 0]));
    emitResponse({
      id,
      kind: request.kind,
      type: TokenizerWorkerResponseType.Result,
      segments: transcriptTokenSegmentsFromCounts(request.items, request.skillLinks, countByText),
    });
  };

  return {
    request(request) {
      const id = ++nextRequestId;
      void run(request, id).catch((error) => {
        if (disposed) return;
        onError(error instanceof Error ? error : new Error(String(error)));
      });
      return id;
    },
    dispose() {
      disposed = true;
    },
  };
}

export function createTokenizerWorker(
  onResponse: (response: TokenizerWorkerResponse) => void,
  onError: (error: Error) => void,
): TokenizerWorkerClient {
  if (isTauriRuntime()) return createNativeTokenizerClient(onResponse, onError);
  const worker = new Worker(new URL("./tokenizer.worker.ts", import.meta.url), { type: "module" });
  let nextRequestId = 0;
  let disposed = false;

  worker.onmessage = (event: MessageEvent<TokenizerWorkerResponse>) => {
    if (!disposed) onResponse(event.data);
  };
  worker.onerror = (event) => {
    if (!disposed) onError(new Error(event.message || "Tokenizer worker failed"));
  };

  return {
    request(request) {
      const id = ++nextRequestId;
      try {
        worker.postMessage({ ...request, id });
      } catch (error) {
        onError(error instanceof Error ? error : new Error(String(error)));
      }
      return id;
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      worker.onmessage = null;
      worker.onerror = null;
      worker.terminate();
    },
  };
}

function compactTranscriptTokenItem(item: TranscriptTokenItem): TranscriptTokenItem {
  const compact: TranscriptTokenItem = {};
  for (const key of ["type", "body", "tag", "command", "result"] as const) {
    const value = item[key];
    if (value !== undefined) compact[key] = value;
  }
  if (item.tools) compact.tools = item.tools.map(compactTranscriptTokenItem);
  return compact;
}

export function compactTranscriptTokenItems(items: TranscriptTokenItem[]): TranscriptTokenItem[] {
  return items.map(compactTranscriptTokenItem);
}

export function compactTranscriptSkillLinks(links: TranscriptSkillLink[]): TranscriptSkillLink[] {
  return links.map((link) => ({ skill_name: link.skill_name }));
}
