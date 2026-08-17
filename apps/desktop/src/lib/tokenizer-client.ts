import type {
  TokenBreakdownSegment,
  TranscriptSkillLink,
  TranscriptTokenItem,
} from "./tokenizer-types.ts";

export type TokenizerWorkerRequest =
  | {
      kind: "markdown";
      activePath: string;
      content: string;
      selectionText: string;
    }
  | {
      kind: "transcript";
      items: TranscriptTokenItem[];
      skillLinks: TranscriptSkillLink[];
    };

export type TokenizerWorkerResponse =
  | {
      id: number;
      kind: TokenizerWorkerRequest["kind"];
      type: "result";
      segments: TokenBreakdownSegment[];
    }
  | {
      id: number;
      kind: TokenizerWorkerRequest["kind"];
      type: "error";
      message: string;
    };

export type TokenizerWorkerClient = {
  request: (request: TokenizerWorkerRequest) => number;
  dispose: () => void;
};

export function createTokenizerWorker(
  onResponse: (response: TokenizerWorkerResponse) => void,
  onError: (error: Error) => void,
): TokenizerWorkerClient {
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
  for (const key of ["type", "kind", "body", "tag", "command", "result"] as const) {
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
  return links.map((link) => ({
    skill_name: link.skill_name,
    skillName: link.skillName,
  }));
}
