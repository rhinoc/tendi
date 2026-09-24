import { useCallback, useEffect, useRef, useState } from "react";

import { IMPORTED_SESSION_AGENT } from "../../controllers/session-controller.ts";
import { AsyncStatus } from "../../lib/async-status.ts";
import { logger } from "../../lib/logger.ts";
import {
  sessionResumeError,
  sessionResumeErrorMessage,
  type SessionResumeState,
} from "../../lib/session-resume.ts";
import {
  SessionResumeErrorAction,
  SessionResumeErrorCode,
  SessionResumeOutcomeStatus,
  SessionResumeTarget,
  type SessionRecord,
  type SessionResumeOutcome,
} from "../../lib/sessions.ts";

type ResumeTarget = Exclude<SessionResumeTarget, SessionResumeTarget.Auto>;
type ResumeFeedbackState = AsyncStatus.Loading | AsyncStatus.Success | AsyncStatus.Error;

export type PendingSessionResumeConflict = {
  session: SessionRecord;
  target?: ResumeTarget;
};

export type SessionResumeOperation = (
  session: SessionRecord,
  target?: ResumeTarget,
  options?: { reconcile?: boolean },
) => Promise<SessionResumeOutcome | null | undefined>;

export type SessionResumeOperationsOptions = {
  onResumeSession?: SessionResumeOperation;
  showSessionError: (message: string) => void;
  dismissSessionError: () => void;
};

export type SessionResumeOperationsResult = {
  pendingResumeConflict: PendingSessionResumeConflict | null;
  repairingResume: boolean;
  resumeSession: SessionResumeOperationHandler;
  repairAndResume: () => Promise<void>;
  cancelResumeConflict: () => void;
  getResumeState: (session: SessionRecord) => SessionResumeState;
};

type SessionResumeOperationHandler = (
  session: SessionRecord,
  target?: ResumeTarget,
  options?: { reconcile?: boolean },
) => Promise<void>;

export function useSessionResumeOperations({
  onResumeSession,
  showSessionError,
  dismissSessionError,
}: SessionResumeOperationsOptions): SessionResumeOperationsResult {
  const [resumeFeedback, setResumeFeedback] = useState<Record<string, ResumeFeedbackState>>({});
  const [pendingResumeConflict, setPendingResumeConflict] = useState<PendingSessionResumeConflict | null>(null);
  const [repairingResume, setRepairingResume] = useState(false);
  const resumeFeedbackTimerRef = useRef<Record<string, number>>({});

  const finishResumeFeedback = useCallback((sessionId: string, state: AsyncStatus.Success | AsyncStatus.Error) => {
    const existingTimer = resumeFeedbackTimerRef.current[sessionId];
    if (existingTimer !== undefined) {
      window.clearTimeout(existingTimer);
    }
    setResumeFeedback((current) => ({ ...current, [sessionId]: state }));
    resumeFeedbackTimerRef.current[sessionId] = window.setTimeout(() => {
      setResumeFeedback((current) => {
        const next = { ...current };
        delete next[sessionId];
        return next;
      });
      delete resumeFeedbackTimerRef.current[sessionId];
    }, 1800);
  }, []);

  const clearResumeFeedback = useCallback((sessionId: string) => {
    const existingTimer = resumeFeedbackTimerRef.current[sessionId];
    if (existingTimer !== undefined) {
      window.clearTimeout(existingTimer);
      delete resumeFeedbackTimerRef.current[sessionId];
    }
    setResumeFeedback((current) => {
      const next = { ...current };
      delete next[sessionId];
      return next;
    });
  }, []);

  useEffect(() => () => {
    Object.values(resumeFeedbackTimerRef.current).forEach((timer) => window.clearTimeout(timer));
  }, []);

  const resumeSession = useCallback(async (
    session: SessionRecord,
    target?: ResumeTarget,
    options?: { reconcile?: boolean },
  ) => {
    if (session.agent === IMPORTED_SESSION_AGENT) {
      finishResumeFeedback(session.id, AsyncStatus.Error);
      showSessionError("Imported sessions cannot be opened.");
      return;
    }
    if (!onResumeSession) {
      finishResumeFeedback(session.id, AsyncStatus.Error);
      showSessionError("Resume is unavailable.");
      return;
    }
    dismissSessionError();
    setResumeFeedback((current) => ({ ...current, [session.id]: AsyncStatus.Loading }));
    try {
      const result = await onResumeSession(session, target, options);
      if (result?.status === SessionResumeOutcomeStatus.ActiveWriter) {
        clearResumeFeedback(session.id);
        setPendingResumeConflict({ session, target });
      } else if (result?.status === SessionResumeOutcomeStatus.Launched) {
        finishResumeFeedback(session.id, AsyncStatus.Success);
        setPendingResumeConflict(null);
      } else if (result?.status === SessionResumeOutcomeStatus.Failed) {
        finishResumeFeedback(session.id, AsyncStatus.Error);
        showSessionError(sessionResumeErrorMessage(result.error));
      } else {
        finishResumeFeedback(session.id, AsyncStatus.Error);
        showSessionError(sessionResumeErrorMessage(
          sessionResumeError(
            SessionResumeErrorCode.Internal,
            null,
            true,
            SessionResumeErrorAction.Retry,
          ),
        ));
      }
    } catch (error) {
      finishResumeFeedback(session.id, AsyncStatus.Error);
      logger.warn("sessions resume failed", { error });
      showSessionError(sessionResumeErrorMessage(
        sessionResumeError(
          SessionResumeErrorCode.DesktopCommandFailed,
          null,
          true,
          SessionResumeErrorAction.Retry,
        ),
      ));
    }
  }, [clearResumeFeedback, dismissSessionError, finishResumeFeedback, onResumeSession, showSessionError]);

  const repairAndResume = useCallback(async () => {
    if (!pendingResumeConflict || repairingResume) return;
    setRepairingResume(true);
    try {
      await resumeSession(
        pendingResumeConflict.session,
        pendingResumeConflict.target,
        { reconcile: true },
      );
    } finally {
      setRepairingResume(false);
    }
  }, [pendingResumeConflict, repairingResume, resumeSession]);

  const cancelResumeConflict = useCallback(() => {
    if (repairingResume) return;
    if (pendingResumeConflict) clearResumeFeedback(pendingResumeConflict.session.id);
    setPendingResumeConflict(null);
  }, [clearResumeFeedback, pendingResumeConflict, repairingResume]);

  const getResumeState = useCallback((session: SessionRecord): SessionResumeState => (
    resumeFeedback[session.id] ?? AsyncStatus.Idle
  ), [resumeFeedback]);

  return {
    pendingResumeConflict,
    repairingResume,
    resumeSession,
    repairAndResume,
    cancelResumeConflict,
    getResumeState,
  };
}
