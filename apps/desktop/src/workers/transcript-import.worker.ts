import { agentDefinitions } from "../lib/agent/index.ts";
import { parseJsonlTranscriptForProvider } from "../lib/agent/transcript.ts";

type TranscriptImportWorkerRequest = { file: File; providerId: string };
type TranscriptImportWorkerResponse =
  | { ok: true; result: ReturnType<typeof parseJsonlTranscriptForProvider> }
  | { ok: false; error: string };

const workerScope = self as unknown as {
  onmessage: ((event: MessageEvent<TranscriptImportWorkerRequest>) => void) | null;
  postMessage: (message: TranscriptImportWorkerResponse) => void;
};

workerScope.onmessage = async (event) => {
  try {
    const text = await event.data.file.text();
    const result = parseJsonlTranscriptForProvider(text, event.data.providerId, agentDefinitions);
    workerScope.postMessage({ ok: true, result });
  } catch (error) {
    workerScope.postMessage({
      ok: false,
      error: error instanceof Error ? error.message : `${error}`,
    });
  }
};
