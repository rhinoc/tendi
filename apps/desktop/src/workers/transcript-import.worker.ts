import { parseJsonlTranscript } from "../lib/transcript.ts";

type TranscriptImportWorkerResponse =
  | { ok: true; result: ReturnType<typeof parseJsonlTranscript> }
  | { ok: false; error: string };

const workerScope = self as unknown as {
  onmessage: ((event: MessageEvent<File>) => void) | null;
  postMessage: (message: TranscriptImportWorkerResponse) => void;
};

workerScope.onmessage = async (event) => {
  try {
    const text = await event.data.text();
    workerScope.postMessage({ ok: true, result: parseJsonlTranscript(text) });
  } catch (error) {
    workerScope.postMessage({
      ok: false,
      error: error instanceof Error ? error.message : `${error}`,
    });
  }
};
