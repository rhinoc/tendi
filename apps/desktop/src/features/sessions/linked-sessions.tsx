import { useMemo } from "react";
import { Waypoints, X } from "lucide-react";
import { Dialog } from "radix-ui";

import { LoadingState } from "../../components/shared/LoadingState.tsx";
import { LoadErrorState } from "../../components/shared/LoadErrorState.tsx";
import { IconButton } from "../../components/shared/IconButton.tsx";
import { formatSessionTitle, normalizeSession, sessionProject, sessionProjectGroupKey } from "../../lib/index.ts";
import { SkillSessionProjectChart, type SkillSessionProjectItem } from "./SkillSessionProjectChart.tsx";
import "../../components/shared/confirm-dialog.css";
import "./linked-sessions.css";

export type LinkedSessionLink = Record<string, unknown> & {
  session_path?: string;
  sessionPath?: string;
  session_project?: string;
  sessionProject?: string;
  session_id?: string | number;
  sessionId?: string | number;
  agent?: string;
  session_title?: string;
  sessionTitle?: string;
  session_started_at?: string;
  sessionStartedAt?: string;
  session_updated_at?: string;
  sessionUpdatedAt?: string;
  evidence_time?: string;
  evidenceTime?: string;
  session_message_count?: number;
  sessionMessageCount?: number;
  skill_name?: string;
  skillName?: string;
  skill_path?: string;
  skillPath?: string;
  evidence_text?: string;
  evidenceText?: string;
};

export type LinkedSessionRow = ReturnType<typeof linkedSessionToSession>;


export function linkedSessionToSession(link: LinkedSessionLink, index: number) {
  const sessionPath = link.session_path ?? link.sessionPath ?? "";
  const project = link.session_project ?? link.sessionProject ?? "";
  const normalized = normalizeSession({
    id: link.session_id ?? link.sessionId,
    agent: link.agent,
    title: link.session_title ?? link.sessionTitle,
    project,
    path: sessionPath,
    started_at: link.session_started_at ?? link.sessionStartedAt,
    updated_at: link.session_updated_at ?? link.sessionUpdatedAt ?? link.evidence_time ?? link.evidenceTime,
    message_count: link.session_message_count ?? link.sessionMessageCount,
  }, index);
  return {
    ...normalized,
    projectPath: project || normalized.projectPath,
    path: sessionPath || normalized.path,
    linkedSessionLink: link,
  };
}


export type LinkedSessionsIndexStatus = {
  indexed?: number;
  total?: number;
  failed?: number;
  running?: boolean;
  pending?: number;
};

export type LinkedSessionsSummaryProps = {
  links: LinkedSessionLink[];
  loading: boolean;
  status?: LinkedSessionsIndexStatus;
  onOpen: () => void;
};

export type LinkedSessionsDrawerProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  skillName: string;
  links: LinkedSessionLink[];
  loading: boolean;
  error?: string;
  onRetry?: () => void;
  onOpenSession?: (session: LinkedSessionLink | LinkedSessionRow) => void;
};

export function LinkedSessionsSummary({ links, loading, status, onOpen }: LinkedSessionsSummaryProps) {
  const indexed = Number(status?.indexed ?? 0);
  const total = Number(status?.total ?? 0);
  const failed = Number(status?.failed ?? 0);
  const indexing = Boolean(status?.running || (status?.pending ?? 0) > 0);
  return (
    <section className="linkedSessionsSummary">
      <div>
        <span>Recent sessions</span>
        <strong>{loading ? "..." : links.length}</strong>
      </div>
      <div className="linkedSessionsMeta">
        {total > 0 ? <span>{indexed}/{total} indexed</span> : <span>No indexed sessions</span>}
        {indexing ? <span>Indexing</span> : null}
        {failed > 0 ? <span>{failed} failed</span> : null}
      </div>
      <IconButton onClick={onOpen} aria-label="Open recent sessions chart">
        <Waypoints size={15} />
      </IconButton>
    </section>
  );
}

export function LinkedSessionsDrawer({ open, onOpenChange, skillName, links, loading, error = "", onRetry, onOpenSession }: LinkedSessionsDrawerProps) {
  const visibleRows = useMemo(
    () => [...links]
      .sort((left, right) => linkedSessionUpdatedAt(right) - linkedSessionUpdatedAt(left))
      .slice(0, 50)
      .map((link, index) => linkedSessionToSession(link, index)),
    [links],
  );
  const chartItems = useMemo<SkillSessionProjectItem[]>(
    () => visibleRows.map((session, index) => {
      const link = session.linkedSessionLink;
      const displayedTitle = formatSessionTitle(session.title) || session.id || "Untitled session";
      const project = sessionProject(session) || "Unknown";
      const projectKey = sessionProjectGroupKey(session) || project;
      return {
        key: `${session.agent}-${session.id}-${session.path}-${index}`,
        skillKey: link.skill_name ?? link.skillName ?? skillName,
        skillLabel: link.skill_name ?? link.skillName ?? skillName,
        sessionLabel: displayedTitle,
        sessionTitle: `${displayedTitle} · ${session.id}`,
        projectKey,
        projectLabel: project,
      };
    }),
    [skillName, visibleRows],
  );
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange} modal={false}>
      <Dialog.Portal>
        <Dialog.Content
          className="linkedSessionsDrawer"
          aria-describedby="linked-sessions-drawer-description"
          data-no-drag
          onEscapeKeyDown={() => onOpenChange(false)}
          onMouseDown={(event) => event.stopPropagation()}
        >
          <div className="linkedSessionsDrawerHeader">
            <Dialog.Title className="linkedSessionsDrawerTitle">Recent Sessions</Dialog.Title>
            <Dialog.Description id="linked-sessions-drawer-description" className="dialogVisuallyHidden">
              View recent sessions linked to this skill.
            </Dialog.Description>
            <div className="linkedSessionsDrawerActions">
              <Dialog.Close asChild>
                <IconButton aria-label="Close recent sessions"><X size={15} /></IconButton>
              </Dialog.Close>
            </div>
          </div>
          {loading ? (
            <LoadingState className="linkedSessionsDrawerEmpty" label="Loading recent sessions" />
          ) : error ? (
            <LoadErrorState message={error} onRetry={onRetry} />
          ) : chartItems.length > 0 ? (
            <div className="linkedSessionsDrawerChart">
              <SkillSessionProjectChart
                items={chartItems}
                ariaLabel="Latest skill to session to project links"
                onSessionClick={onOpenSession ? (item) => {
                  const index = chartItems.findIndex((candidate) => candidate.key === item.key);
                  const session = visibleRows[index];
                  if (session) onOpenSession(session.linkedSessionLink ?? session);
                } : undefined}
              />
            </div>
          ) : <div className="linkedSessionsDrawerEmpty">No recent sessions. Sessions appear when this skill is used.</div>}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function linkedSessionUpdatedAt(link: LinkedSessionLink): number {
  return Date.parse(
    `${link.session_updated_at
      ?? link.sessionUpdatedAt
      ?? link.evidence_time
      ?? link.evidenceTime
      ?? ""}`,
  ) || 0;
}
