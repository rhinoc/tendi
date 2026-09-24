import { useCallback, useEffect, useRef, useState } from "react";

import { createImportedSessionRecord } from "../../controllers/session-controller.ts";
import { logger } from "../../lib/logger.ts";
import type { SessionRecord } from "../../lib/sessions.ts";
import type { JsonlTranscriptParseResult } from "../../lib/transcript.ts";
import type { TranscriptItemRecord } from "./session-transcript-logic.ts";

export enum ImportFeedbackState {
  Idle = "idle",
  Loading = "loading",
  Success = "success",
  Warning = "warning",
  Error = "error",
}

type TranscriptImportWorkerResponse =
  | { ok: true; result: JsonlTranscriptParseResult }
  | { ok: false; error: string };

type TranscriptImportWorkerHandle = {
  worker: Worker;
  cancel: () => void;
};

type UseSessionImportOptions = {
  providerId: string;
  showSessionError: (message: string) => void;
  onImported: (session: SessionRecord) => void;
};

function parseImportedTranscript(
  file: File,
  providerId: string,
  workerRef: { current: TranscriptImportWorkerHandle | null },
): Promise<JsonlTranscriptParseResult> {
  const worker = new Worker(
    new URL("../../workers/transcript-import.worker.ts", import.meta.url),
    { type: "module" },
  );
  return new Promise((resolve, reject) => {
    let settled = false;
    const cleanup = () => {
      if (workerRef.current?.worker === worker) workerRef.current = null;
      worker.terminate();
    };
    const cancel = () => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(new Error("Transcript import cancelled"));
    };
    worker.onmessage = (event: MessageEvent<TranscriptImportWorkerResponse>) => {
      if (settled) return;
      settled = true;
      cleanup();
      if (event.data.ok) resolve(event.data.result);
      else reject(new Error(event.data.error));
    };
    worker.onerror = (event) => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(new Error(event.message || "Transcript import worker failed"));
    };
    workerRef.current = { worker, cancel };
    try {
      worker.postMessage({ file, providerId });
    } catch (error) {
      logger.error("sessions transcript import worker failed", { error });
      settled = true;
      cleanup();
      reject(error);
    }
  });
}

export function useSessionImport({ providerId, showSessionError, onImported }: UseSessionImportOptions) {
  const [importedSessions, setImportedSessions] = useState<SessionRecord[]>([]);
  const [importedTranscripts, setImportedTranscripts] = useState<Record<string, TranscriptItemRecord[]>>({});
  const [importFeedback, setImportFeedback] = useState<ImportFeedbackState>(ImportFeedbackState.Idle);
  const [importError, setImportError] = useState("");
  const importWorkerRef = useRef<TranscriptImportWorkerHandle | null>(null);
  const importFeedbackTimerRef = useRef<number | undefined>(undefined);
  const mountedRef = useRef(true);

  const clearImportFeedbackTimer = useCallback(() => {
    if (importFeedbackTimerRef.current !== undefined) {
      window.clearTimeout(importFeedbackTimerRef.current);
      importFeedbackTimerRef.current = undefined;
    }
  }, []);

  const finishImportFeedback = useCallback((state: Exclude<ImportFeedbackState, ImportFeedbackState.Idle | ImportFeedbackState.Loading>) => {
    clearImportFeedbackTimer();
    setImportFeedback(state);
    if (state === ImportFeedbackState.Success) {
      importFeedbackTimerRef.current = window.setTimeout(() => {
        setImportFeedback(ImportFeedbackState.Idle);
        importFeedbackTimerRef.current = undefined;
      }, 1600);
    }
  }, [clearImportFeedbackTimer]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      clearImportFeedbackTimer();
      importWorkerRef.current?.cancel();
      importWorkerRef.current = null;
    };
  }, [clearImportFeedbackTimer]);

  const importJsonl = useCallback(async (file: File) => {
    clearImportFeedbackTimer();
    setImportFeedback(ImportFeedbackState.Loading);
    setImportError("");
    try {
      if (!providerId) {
        setImportError("Choose a transcript provider before importing");
        showSessionError("Choose a transcript provider before importing.");
        finishImportFeedback(ImportFeedbackState.Error);
        return;
      }
      const parsed = await parseImportedTranscript(file, providerId, importWorkerRef);
      if (!mountedRef.current) return;
      if (parsed.parsedCount === 0 || parsed.items.length === 0) {
        setImportError("Could not import JSONL transcript");
        showSessionError("Could not import JSONL transcript. Check the file and try again.");
        finishImportFeedback(ImportFeedbackState.Error);
        return;
      }
      const id = `import:${file.name}:${file.lastModified}:${Date.now()}`;
      const session = createImportedSessionRecord({ id, fileName: file.name, lastModified: file.lastModified, parsed });
      setImportedTranscripts((current) => ({ ...current, [id]: parsed.items as TranscriptItemRecord[] }));
      setImportedSessions((current) => [session, ...current]);
      onImported(session);
      setImportError(parsed.warnings.length ? `${parsed.warnings.length} invalid lines skipped` : "");
      finishImportFeedback(parsed.warnings.length ? ImportFeedbackState.Warning : ImportFeedbackState.Success);
    } catch (error) {
      if (!mountedRef.current) return;
      logger.warn("sessions transcript import failed", { error });
      setImportError("Could not import JSONL transcript");
      showSessionError("Could not import JSONL transcript. Check the file and try again.");
      finishImportFeedback(ImportFeedbackState.Error);
    }
  }, [clearImportFeedbackTimer, finishImportFeedback, onImported, providerId, showSessionError]);

  return {
    importedSessions,
    importedTranscripts,
    importFeedback,
    importError,
    importJsonl,
  };
}
