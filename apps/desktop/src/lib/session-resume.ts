import { AsyncStatus } from "./async-status.ts";
import { SessionResumeTarget } from "./sessions.ts";

export type SessionResumeState = AsyncStatus;

export function sessionResumeTargetForMenu(
  target: SessionResumeTarget,
  inferredTarget?: Exclude<SessionResumeTarget, SessionResumeTarget.Auto>,
): SessionResumeTarget {
  return target === SessionResumeTarget.Auto ? inferredTarget ?? SessionResumeTarget.Auto : target;
}

export function sessionResumeTargetsForMenu(capabilities: {
  terminal: boolean;
  app: boolean;
}): Array<Exclude<SessionResumeTarget, SessionResumeTarget.Auto>> {
  const targets: Array<Exclude<SessionResumeTarget, SessionResumeTarget.Auto>> = [];
  if (capabilities.terminal) targets.push(SessionResumeTarget.Terminal);
  if (capabilities.app) targets.push(SessionResumeTarget.App);
  return targets;
}

export function sessionResumeLabel(state: SessionResumeState, target: SessionResumeTarget): string {
  const targetSuffix = target === SessionResumeTarget.App || target === SessionResumeTarget.Terminal ? ` in ${target}` : "";
  if (state === AsyncStatus.Loading) return `Opening session${targetSuffix}`;
  if (state === AsyncStatus.Success) return `Session opened${targetSuffix}`;
  if (state === AsyncStatus.Error) return `Could not open session${targetSuffix}`;
  return targetSuffix ? `Resume${targetSuffix}` : "Resume";
}

export function sessionResumeErrorMessage(): string {
  return `${sessionResumeLabel(AsyncStatus.Error, SessionResumeTarget.Auto)}. Try again.`;
}
