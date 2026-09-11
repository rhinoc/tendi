import { AsyncStatus } from "./async-status.ts";
import { SessionResumeErrorAction, SessionResumeErrorCode, SessionResumeTarget, type SessionResumeError } from "./sessions.ts";

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

export function sessionResumeErrorMessage(error?: SessionResumeError): string {
  if (!error) return `${sessionResumeLabel(AsyncStatus.Error, SessionResumeTarget.Auto)}. Try again.`;
  switch (error.code) {
    case SessionResumeErrorCode.DesktopRuntimeRequired:
      return "Session resume is only available in the Tendi desktop app.";
    case SessionResumeErrorCode.DesktopCommandFailed:
      return "Tendi could not reach the desktop runtime. Restart Tendi and try again.";
    case SessionResumeErrorCode.WorktreeNotFound: {
      const provider = resumeProviderLabel(error.provider);
      return `Could not open this session in ${provider}. Open this project in ${provider} first, or choose another terminal in Settings.`;
    }
    case SessionResumeErrorCode.TerminalUnavailable:
      return "The selected terminal is unavailable. Choose another terminal in Settings.";
    case SessionResumeErrorCode.TerminalLaunchFailed:
      return `Could not open session${resumeProviderSuffix(error.provider)}. Try again.`;
    case SessionResumeErrorCode.SessionNotResumable:
      return "This session cannot be resumed in a terminal.";
    case SessionResumeErrorCode.Internal:
      return error.retryable
        ? "Could not prepare the session. Try again."
        : "Could not prepare the session.";
  }
}

export function sessionResumeError(
  code: SessionResumeErrorCode,
  provider: string | null = null,
  retryable = false,
  action: SessionResumeErrorAction = SessionResumeErrorAction.None,
): SessionResumeError {
  return { code, provider, retryable, action };
}

function resumeProviderSuffix(provider: string | null): string {
  if (!provider) return "";
  return ` in ${resumeProviderLabel(provider)}`;
}

function resumeProviderLabel(provider: string | null): string {
  if (!provider) return "the selected terminal";
  const labels: Record<string, string> = {
    terminal: "Terminal",
    iterm: "iTerm",
    ghostty: "Ghostty",
    warp: "Warp",
    orca: "Orca",
  };
  return labels[provider] ?? provider;
}
