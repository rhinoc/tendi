import { canonicalSessionAgent } from "./session-selection.ts";
import type { SessionIdentityRecord, SessionRecord } from "./sessions.ts";

export type SessionProjectDelta =
  | {
    kind: "merge";
    projectId: string;
    projectName: string;
    mergedProjectIds: string[];
  }
  | {
    kind: "split";
    projectId: string;
    projectName: string;
    sessions: SessionIdentityRecord[];
  };

function projectSessionIdentity(session: SessionIdentityRecord): string {
  return `${canonicalSessionAgent(session.agent)}\u0000${session.id ?? ""}\u0000${session.path ?? ""}`;
}

export function applySessionProjectDelta(
  currentRows: SessionRecord[],
  delta: SessionProjectDelta,
): SessionRecord[] {
  const affectedProjectIds = delta.kind === "merge"
    ? new Set(delta.mergedProjectIds)
    : null;
  const affectedSessions = delta.kind === "split"
    ? new Set(delta.sessions.map(projectSessionIdentity))
    : null;
  let changed = false;
  const nextRows = currentRows.map((row) => {
    const affected = affectedProjectIds
      ? affectedProjectIds.has(`${row.logicalProjectId ?? ""}`)
      : affectedSessions?.has(projectSessionIdentity(row)) === true;
    if (!affected) return row;
    if (row.logicalProjectId === delta.projectId && row.logicalProjectName === delta.projectName) {
      return row;
    }
    changed = true;
    return {
      ...row,
      logicalProjectId: delta.projectId,
      logicalProjectName: delta.projectName,
    };
  });
  return changed ? nextRows : currentRows;
}
