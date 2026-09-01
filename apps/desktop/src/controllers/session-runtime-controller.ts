import { useCallback, useEffect, useRef } from "react";

import {
  invokeAnalyticsRevision,
  invokeSessionScanStart,
  invokeSessionSnapshot,
  SessionScanPhase,
  SkillUpdateEventStatus,
  subscribeRuntimeEvents,
} from "../lib/runtime-gateway.ts";
import type { RuntimeEvent } from "../lib/runtime-gateway.ts";
import { commitSessionEventBuffer } from "../lib/runtime-workflows.ts";
import { RuntimeEventName } from "../lib/generated/runtime-events.ts";
import { logger } from "../lib/logger.ts";
import { RuntimeDomainKey } from "../lib/domain.ts";
import { sessionIdentityRecordKey, sessionLogicalIdentityRecordKey } from "../lib/sessions.ts";
import type { SessionIdentityRecord } from "../lib/sessions.ts";
import type { RawDomainRow } from "./controller-types.ts";
import { desktopStore } from "../store/desktop-store.ts";

const SESSION_EVENT_FLUSH_MS = 200;
const SESSION_REFRESH_ERROR = "Could not refresh sessions. Try again.";

type SessionScanEvent = Extract<RuntimeEvent, { event: typeof RuntimeEventName.SessionsScan }>;

export type SessionRuntimeControllerOptions = {
  refreshSessionProjects: () => Promise<void>;
  runSkillIndex: () => Promise<unknown> | void;
  setSessionRefreshError: (message: string) => void;
  setAnalyticsRevision: (revision: number) => void;
  setAnalyticsRevisionReady: (ready: boolean) => void;
  setAnalyticsRevisionError: (message: string) => void;
  setSkillUpdateError: (message: string) => void;
  setCheckingSkillUpdates: (checking: boolean) => void;
};

export type SessionRuntimeController = {
  refreshSessionsFromScan: () => Promise<number | null>;
  resyncSessionSnapshot: (onComplete?: () => void) => Promise<void>;
  whenEventsReady: () => Promise<void>;
};

export function useSessionRuntimeController(
  options: SessionRuntimeControllerOptions,
): SessionRuntimeController {
  const {
    refreshSessionProjects,
    runSkillIndex,
    setSessionRefreshError,
    setAnalyticsRevision,
    setAnalyticsRevisionReady,
    setAnalyticsRevisionError,
    setSkillUpdateError,
    setCheckingSkillUpdates,
  } = options;
  const sessionsRefreshInFlight = useRef<Promise<number | null> | null>(null);
  const sessionEventReady = useRef<Promise<void>>(Promise.resolve());
  const sessionEventFlushTimer = useRef<number | undefined>(undefined);
  const sessionSnapshotResyncInFlight = useRef<Promise<void> | null>(null);
  const lastDaemonEventId = useRef<number | null>(null);
  const sessionScopeKey = useRef<string | null>(null);
  const sessionScanGeneration = useRef(0);
  const completedSessionScans = useRef(new Set<number>());
  const sessionScanWaiters = useRef(new Map<number, Array<() => void>>());
  const pendingRecentSessions = useRef(new Map<string, RawDomainRow>());
  const pendingWatchSessions = useRef(new Map<string, RawDomainRow>());
  const pendingDeletedSessions = useRef(new Map<string, SessionIdentityRecord>());
  const disposed = useRef(false);

  const setSessionLoadError = useCallback(() => {
    const hasRows = desktopStore.getSnapshot().catalogs.data.sessions.length > 0;
    desktopStore.actions.setDomainError(RuntimeDomainKey.Sessions, hasRows ? "" : "Could not load sessions. Try again.");
    setSessionRefreshError(SESSION_REFRESH_ERROR);
  }, [setSessionRefreshError]);

  const resyncSessionSnapshot = useCallback((onComplete?: () => void) => {
    const inFlight = sessionSnapshotResyncInFlight.current;
    if (inFlight) return inFlight.finally(() => onComplete?.());
    const request = invokeSessionSnapshot()
      .then((snapshot) => {
        if (sessionScopeKey.current && sessionScopeKey.current !== snapshot.scopeKey) {
          throw new Error("Sessions snapshot scope changed during the current app session");
        }
        sessionScopeKey.current = snapshot.scopeKey;
        const localRevision = desktopStore.getSnapshot().catalogs.revisions.sessions ?? 0;
        if (snapshot.revision < localRevision) return;
        desktopStore.actions.commitSessionSnapshot(snapshot.payload);
        desktopStore.actions.setDomainRevision(RuntimeDomainKey.Sessions, snapshot.revision);
      })
      .catch((error) => {
        logger.error("sessions snapshot resync failed", { error });
        setSessionLoadError();
      })
      .finally(() => {
        sessionSnapshotResyncInFlight.current = null;
        onComplete?.();
      });
    sessionSnapshotResyncInFlight.current = request;
    return request;
  }, [setSessionLoadError]);

  const flushBufferedSessionEvents = useCallback(() => {
    const recent = [...pendingRecentSessions.current.values()];
    const watch = [...pendingWatchSessions.current.values()];
    const deleted = [...pendingDeletedSessions.current.values()];
    pendingRecentSessions.current.clear();
    pendingWatchSessions.current.clear();
    pendingDeletedSessions.current.clear();
    sessionEventFlushTimer.current = undefined;
    if (recent.length === 0 && watch.length === 0 && deleted.length === 0) return;
    commitSessionEventBuffer(desktopStore, recent, watch, deleted);
  }, []);

  const scheduleSessionEventFlush = useCallback(() => {
    if (sessionEventFlushTimer.current !== undefined) return;
    sessionEventFlushTimer.current = window.setTimeout(flushBufferedSessionEvents, SESSION_EVENT_FLUSH_MS);
  }, [flushBufferedSessionEvents]);

  const finishSessionScanWaiters = useCallback((generation: number) => {
    completedSessionScans.current.add(generation);
    const waiters = sessionScanWaiters.current.get(generation) ?? [];
    sessionScanWaiters.current.delete(generation);
    waiters.forEach((resolve) => resolve());
  }, []);

  const handleSessionScanEvent = useCallback((daemonEvent: SessionScanEvent) => {
    const event = daemonEvent.payload;
    if (daemonEvent.scopeKey) {
      if (sessionScopeKey.current && sessionScopeKey.current !== daemonEvent.scopeKey) return;
      sessionScopeKey.current = daemonEvent.scopeKey;
    }
    if (daemonEvent.domain === RuntimeDomainKey.Sessions
      && Number.isSafeInteger(daemonEvent.baseRevision)
      && Number.isSafeInteger(daemonEvent.revision)) {
      const baseRevision = daemonEvent.baseRevision as number;
      const revision = daemonEvent.revision as number;
      const decision = desktopStore.actions.acceptDomainRevision(
        RuntimeDomainKey.Sessions,
        baseRevision,
        revision,
      );
      if (decision === "stale") return;
      if (decision === "resync") {
        void resyncSessionSnapshot(
          event.phase === SessionScanPhase.Backfill && event.complete
            ? () => finishSessionScanWaiters(event.generation)
            : undefined,
        );
        return;
      }
    }
    if (event.generation < sessionScanGeneration.current) return;
    sessionScanGeneration.current = event.generation;
    if (event.phase === SessionScanPhase.Recent) {
      for (const session of event.upserts) {
        const key = sessionLogicalIdentityRecordKey(session);
        if (key) pendingRecentSessions.current.set(key, session);
      }
    } else if (event.phase === SessionScanPhase.Watch) {
      for (const session of event.upserts) {
        const key = sessionLogicalIdentityRecordKey(session);
        if (key) pendingWatchSessions.current.set(key, session);
      }
    }
    for (const identity of event.deleted) {
      const key = sessionIdentityRecordKey(identity);
      if (!key) continue;
      pendingDeletedSessions.current.set(key, identity);
    }
    scheduleSessionEventFlush();
    if (event.phase === SessionScanPhase.Recent && event.complete) {
      setSessionRefreshError("");
      if (sessionEventFlushTimer.current !== undefined) window.clearTimeout(sessionEventFlushTimer.current);
      flushBufferedSessionEvents();
    }
    if (event.phase === SessionScanPhase.Backfill && event.complete) {
      void resyncSessionSnapshot(() => {
        setSessionRefreshError("");
        finishSessionScanWaiters(event.generation);
      });
    } else if (event.phase === SessionScanPhase.Error) {
      logger.error("sessions scan failed", {
        generation: event.generation,
        phase: event.phase,
        scanned: event.scanned,
        complete: event.complete,
        error: event.error ?? "unknown session scan error",
      });
      if (!event.complete) return;
      setSessionRefreshError(SESSION_REFRESH_ERROR);
      setSessionLoadError();
      finishSessionScanWaiters(event.generation);
    } else if (event.phase === SessionScanPhase.Watch && event.complete) {
      if (event.error) {
        setSessionRefreshError(SESSION_REFRESH_ERROR);
        setSessionLoadError();
      } else {
        setSessionRefreshError("");
      }
    }
  }, [finishSessionScanWaiters, flushBufferedSessionEvents, resyncSessionSnapshot, scheduleSessionEventFlush, setSessionLoadError, setSessionRefreshError]);

  useEffect(() => {
    disposed.current = false;
    let unsubscribe: (() => void) | null = null;
    const setup = subscribeRuntimeEvents((event) => {
      if (disposed.current) return;
      const previousEventId = lastDaemonEventId.current;
      lastDaemonEventId.current = event.id;
      if (event.event === RuntimeEventName.SessionsScan) {
        if (previousEventId !== null && event.id > previousEventId + 1) {
          const scanEvent = event.payload;
          void resyncSessionSnapshot(
            scanEvent.phase === SessionScanPhase.Backfill && scanEvent.complete
              ? () => finishSessionScanWaiters(scanEvent.generation)
              : undefined,
          );
          void invokeAnalyticsRevision()
            .then((revision) => {
              setAnalyticsRevision(revision);
              setAnalyticsRevisionReady(true);
              setAnalyticsRevisionError("");
            })
            .catch((error) => {
              logger.error("analytics revision resync failed", { error });
              setAnalyticsRevisionError(error instanceof Error ? error.message : `${error}`);
            });
          return;
        }
        handleSessionScanEvent(event);
      } else if (event.event === RuntimeEventName.AnalyticsRevision) {
        if (sessionScopeKey.current && event.payload.scopeKey !== sessionScopeKey.current) return;
        setAnalyticsRevision(event.payload.revision);
        setAnalyticsRevisionReady(true);
        setAnalyticsRevisionError("");
      } else if (event.event === RuntimeEventName.AnalyticsProgress) {
        desktopStore.actions.setAnalyticsProgress(event.payload);
      } else if (event.event === RuntimeEventName.SkillsUpdates) {
        const payload = event.payload;
        if (payload.status === SkillUpdateEventStatus.Completed) {
          if (Array.isArray(payload.skills)) {
            desktopStore.actions.replaceSkills(payload.skills);
            desktopStore.actions.markDomainLoaded(RuntimeDomainKey.Skills);
            desktopStore.actions.setDomainError(RuntimeDomainKey.Skills, "");
          }
          desktopStore.actions.setSkillUpdateReports(payload.updates);
          setSkillUpdateError("");
        } else {
          setSkillUpdateError(payload.error || "Update check failed");
        }
        setCheckingSkillUpdates(false);
      }
    });
    sessionEventReady.current = setup.then((cleanup) => {
      if (disposed.current) cleanup();
      else unsubscribe = cleanup;
    }).catch((error) => {
      if (!disposed.current) logger.warn("daemon event subscription failed", { error });
    });
    return () => {
      disposed.current = true;
      unsubscribe?.();
      if (sessionEventFlushTimer.current !== undefined) {
        window.clearTimeout(sessionEventFlushTimer.current);
        sessionEventFlushTimer.current = undefined;
      }
      for (const waiters of sessionScanWaiters.current.values()) waiters.forEach((resolve) => resolve());
      sessionScanWaiters.current.clear();
    };
  }, [finishSessionScanWaiters, handleSessionScanEvent, resyncSessionSnapshot, setAnalyticsRevision, setAnalyticsRevisionError, setAnalyticsRevisionReady, setCheckingSkillUpdates, setSkillUpdateError]);

  const refreshSessionsFromScan = useCallback(() => {
    setSessionRefreshError("");
    desktopStore.actions.setDomainLoading(RuntimeDomainKey.Sessions, true);
    if (!sessionsRefreshInFlight.current) {
      sessionsRefreshInFlight.current = sessionEventReady.current
        .then(() => invokeSessionScanStart())
        .then(async ({ generation, started }) => {
          sessionScanGeneration.current = Math.max(sessionScanGeneration.current, generation);
          if (!completedSessionScans.current.has(generation)) {
            await new Promise<void>((resolve) => {
              const waiters = sessionScanWaiters.current.get(generation) ?? [];
              waiters.push(resolve);
              sessionScanWaiters.current.set(generation, waiters);
            });
          }
          await refreshSessionProjects();
          if (started) void runSkillIndex();
          return generation;
        })
        .catch((error) => {
          logger.error("sessions scan start failed", { error });
          setSessionRefreshError(SESSION_REFRESH_ERROR);
          setSessionLoadError();
          return null;
        })
        .finally(() => {
          desktopStore.actions.setDomainLoading(RuntimeDomainKey.Sessions, false);
          sessionsRefreshInFlight.current = null;
        });
    }
    return sessionsRefreshInFlight.current;
  }, [refreshSessionProjects, runSkillIndex, setSessionLoadError, setSessionRefreshError]);

  return {
    refreshSessionsFromScan,
    resyncSessionSnapshot,
    whenEventsReady: useCallback(() => sessionEventReady.current, []),
  };
}
