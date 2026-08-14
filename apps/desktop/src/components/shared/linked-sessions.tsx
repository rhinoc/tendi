import { useMemo } from "react";
import { Table as TableIcon, X } from "lucide-react";
import { Dialog } from "radix-ui";

import { normalizeSession } from "../../lib/index.ts";
import { DataTable } from "../DataTable.tsx";
import { createSessionTableColumns } from "./createSessionTableColumns.tsx";
import { LoadingInline } from "./LoadingInline.tsx";
import { RadialConvergenceChart } from "./RadialConvergenceChart.tsx";
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
  links: LinkedSessionLink[];
  loading: boolean;
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
        <span>Linked sessions</span>
        <strong>{loading ? "..." : links.length}</strong>
      </div>
      <div className="linkedSessionsMeta">
        {total > 0 ? <span>{indexed}/{total} indexed</span> : <span>No indexed sessions</span>}
        {indexing ? <span>Indexing</span> : null}
        {failed > 0 ? <span>{failed} failed</span> : null}
      </div>
      <button className="headerGhostButton" onClick={onOpen} aria-label="Open linked sessions">
        <TableIcon size={15} />
      </button>
    </section>
  );
}

export function LinkedSessionsDrawer({ open, onOpenChange, links, loading, onOpenSession }: LinkedSessionsDrawerProps) {
  const sessionRows = useMemo(
    () => links.map((link, index) => linkedSessionToSession(link, index)),
    [links],
  );
  const columns = useMemo(() => createSessionTableColumns({
    keys: ["title", "agent", "project", "updatedAt"],
    widths: {
      title: "minmax(280px, 1fr)",
      agent: "56px",
      project: "minmax(116px, 0.58fr)",
      updatedAt: "104px",
    },
  }), []);
  const convergenceNodes = useMemo(
    () => sessionRows.map((session, index) => ({
      key: `${session.agent}-${session.id}-${session.path}-${index}`,
      label: session.title || session.id || "Untitled session",
    })),
    [sessionRows],
  );
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange} modal={false}>
      <Dialog.Portal>
        <Dialog.Content
          className="linkedSessionsDrawer"
          data-no-drag
          onEscapeKeyDown={() => onOpenChange(false)}
          onMouseDown={(event) => event.stopPropagation()}
        >
          <div className="linkedSessionsDrawerHeader">
            <Dialog.Title className="linkedSessionsDrawerTitle">Linked Sessions</Dialog.Title>
            <div className="linkedSessionsDrawerActions">
              <Dialog.Close asChild>
                <button className="headerGhostButton" aria-label="Close linked sessions"><X size={15} /></button>
              </Dialog.Close>
            </div>
          </div>
          {convergenceNodes.length > 0 ? (
            <div className="linkedSessionsDrawerChart">
              <RadialConvergenceChart
                nodes={convergenceNodes}
                centerLabel="LINKED"
                ariaLabel={`${convergenceNodes.length} linked sessions`}
              />
            </div>
          ) : null}
          <div className="linkedSessionsDrawerTable">
            <DataTable
              rows={sessionRows}
              columns={columns}
              getRowId={(session) => `${session.agent}-${session.id}-${session.path}`}
              getRowLabel={(session) => session.title}
              defaultSort={{ key: "updatedAt", direction: "desc" }}
              onRowClick={(session) => onOpenSession?.(session.linkedSessionLink ?? session)}
              loading={loading}
              loadingLabel={<LoadingInline label="Loading linked sessions" />}
              emptyState="No linked sessions. Links appear when this session references others."
            />
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
