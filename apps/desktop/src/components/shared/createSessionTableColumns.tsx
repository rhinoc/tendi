import { Tooltip } from "./Tooltip.tsx";
import { AlertCircle, ArrowLeft, ArrowRight, Check } from "lucide-react";

import type { ColumnDef } from "../DataTable.types";
import { friendlyAgent, sessionCacheRate, sessionKind, sessionProject, sessionProjectGroupKey, sessionProjectGroupLabel, sortValue, summarizeSessionPreviewRecord, type SessionRecord } from "../../lib/index.ts";
import { cacheRateTone } from "../../lib/token-style.ts";
import { AgentBadge } from "./AgentBadge.tsx";
import { CopyableSessionId } from "./CopyableSessionId.tsx";
import { LoadingIcon } from "./LoadingIcon.tsx";
import { TranscriptLinkText } from "./TranscriptLinkText.tsx";

export type SessionTableRow = {
  id?: string;
  title?: string;
  agent?: string;
  project?: string;
  projectPath?: string;
  repository?: string;
  repositoryPath?: string;
  repositoryUrl?: string;
  logicalProjectId?: string;
  logicalProjectName?: string;
  startedLabel?: string;
  startedAt?: string;
  updatedLabel?: string;
  updatedAt?: string;
  time?: string;
  path?: string;
  messages?: number;
  turnCount?: number;
  parentSessionId?: string;
  searchSnippet?: string;
  tokenUsage?: SessionRecord["tokenUsage"];
  [key: string]: unknown;
};

export type CreateSessionTableColumnsOptions<T extends SessionTableRow = SessionTableRow> = {
  normalizedQuery?: string;
  resumeSession?: (session: T) => void | Promise<void>;
  resumeState?: (session: T) => "idle" | "loading" | "success" | "error";
  keys?: string[];
  widths?: Partial<Record<string, string>>;
};

export function createSessionTableColumns<T extends SessionTableRow = SessionTableRow>({
  normalizedQuery = "",
  resumeSession,
  resumeState,
  keys,
  widths = {},
}: CreateSessionTableColumnsOptions<T> = {}): ColumnDef<T>[] {
  const canResume = typeof resumeSession === "function";
  const columns: ColumnDef<T>[] = [
    {
      key: "title",
      header: "Session",
      type: "text",
      sticky: true,
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, "title"),
      width: widths.title ?? "var(--data-freeze-column-width, 360px)",
      render: (session) => {
        const preview = summarizeSessionPreviewRecord(session as SessionRecord);
        const displayTitle = preview?.title || `${session.title ?? ""}`;
        return (
          <>
            <span className="sessionTitleWithKind">
              <Tooltip content={displayTitle} onlyWhenTruncated>
                <span className="sessionTitleText"><TranscriptLinkText interactive={false} value={displayTitle} /></span>
              </Tooltip>
              {sessionKind(session as SessionRecord) === "child" ? <span className="sessionChildBadge">Child</span> : null}
            </span>
            {normalizedQuery && session.searchSnippet ? (
              <span className="dataCellSubLine">
                <Tooltip content={session.searchSnippet.replace(/[⟦⟧]/g, "")} onlyWhenTruncated><span className="dataCellSub sessionSearchSnippet">
                  <TranscriptLinkText interactive={false} query={normalizedQuery} value={session.searchSnippet.replace(/[⟦⟧]/g, "")} />
                </span></Tooltip>
              </span>
            ) : preview ? (
              <span className="dataCellSubLine sessionPreviewSubLine">
                <span className="sessionPreviewMessage">
                  <ArrowRight size={13} aria-hidden="true" />
                  <span className="dataCellSub sessionPreviewText"><TranscriptLinkText interactive={false} value={preview.userLast} /></span>
                </span>
                <span className="sessionPreviewMessage">
                  <ArrowLeft size={13} aria-hidden="true" />
                  <span className="dataCellSub sessionPreviewText"><TranscriptLinkText interactive={false} value={preview.assistantLast} /></span>
                </span>
              </span>
            ) : (
              <span className="dataCellSubLine">
                <CopyableSessionId sessionId={`${session.id ?? ""}`} className="dataCellSub inSessionTable" />
              </span>
            )}
          </>
        );
      },
    },
    {
      key: "agent",
      header: "Agent",
      type: "enum",
      groupBy: (session) => friendlyAgent(session.agent),
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, "agent"),
      width: widths.agent ?? "78px",
      render: (session) => {
        if (!canResume) {
          return (
            <span className="sessionAgentCell">
              <AgentBadge agent={friendlyAgent(session.agent ?? "")} />
            </span>
          );
        }
        const state = resumeState?.(session as T) ?? "idle";
        const stateClassName = state === "loading" ? "isLoading" : state === "success" ? "isSuccess" : state === "error" ? "isError" : "";
        const label = state === "loading"
          ? `Opening ${session.agent} session in terminal`
          : state === "success"
            ? "Session opened in terminal"
            : state === "error"
              ? "Could not open session in terminal"
              : `Resume ${session.agent} session in terminal`;
        return (
          <Tooltip content={label}><button
            type="button"
            className={`sessionAgentCell sessionAgentButton ${stateClassName}`}
            aria-label={label}
            aria-busy={state === "loading"}
            aria-disabled={state === "loading" || undefined}
            data-no-row-click
            onClick={(event) => {
              event.stopPropagation();
              if (state === "loading") return;
              resumeSession?.(session as T);
            }}
            onKeyDown={(event) => event.stopPropagation()}
          >
            {state === "loading"
              ? <LoadingIcon size={14} />
              : state === "success"
                ? <Check size={14} />
                : state === "error"
                  ? <AlertCircle size={14} />
                  : <AgentBadge agent={friendlyAgent(session.agent)} />}
          </button></Tooltip>
        );
      },
    },
    {
      key: "project",
      header: "Project",
      type: "enum",
      groupBy: (session) => sessionProjectGroupKey(session as SessionRecord),
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, "project"),
      width: widths.project ?? "202px",
      value: (session) => sessionProject(session as SessionRecord),
      groupLabel: sessionProjectGroupLabel,
      empty: "Unknown",
    },
    {
      key: "startedAt",
      header: "Started",
      type: "date",
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, "startedAt"),
      width: widths.startedAt ?? "104px",
      value: (session) => session.startedLabel,
      title: (session) => session.startedAt,
    },
    {
      key: "updatedAt",
      header: "Updated",
      type: "date",
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, "updatedAt"),
      width: widths.updatedAt ?? "104px",
      value: (session) => session.updatedLabel,
      title: (session) => session.updatedAt,
    },
    {
      key: "messages",
      header: "Msgs",
      type: "text",
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, "messages"),
      width: widths.messages ?? "70px",
      cell: "number",
    },
    {
      key: "turns",
      header: "Turns",
      type: "text",
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, "turns"),
      width: widths.turns ?? "70px",
      cell: "number",
      value: (session) => session.turnCount,
    },
    {
      key: "cacheRate",
      header: "Cache",
      type: "text",
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, "cacheRate"),
      width: widths.cacheRate ?? "70px",
      cell: "number",
      render: (session) => {
        const record = session as SessionRecord;
        const rate = sessionCacheRate(record);
        const usage = record.tokenUsage;
        if (rate === undefined || !usage) return <span className="dataCellNumber">-</span>;
        const value = `${rate.toFixed(1)}%`;
        return (
          <Tooltip content={`${usage.cachedInputTokens.toLocaleString()} cached of ${usage.inputTokens.toLocaleString()} input tokens`}><span
            className={`dataCellNumber tokenTone-${cacheRateTone(rate)}`}
          >
            {value}
          </span></Tooltip>
        );
      },
    },
  ];
  if (!keys?.length) return columns;
  const allowed = new Set(keys);
  return columns.filter((column) => allowed.has(column.key));
}
