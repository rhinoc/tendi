import { Tooltip } from "../../components/shared/Tooltip.tsx";
import { AlertCircle, ArrowLeft, ArrowRight, Check } from "lucide-react";

import { ColumnCellVariant, ColumnDataType, type ColumnDef } from "../../components/DataTable.types";
import { AsyncStatus, friendlyAgent, sessionCacheRate, SessionKind, sessionKind, sessionProject, sessionProjectGroupKey, sessionProjectGroupLabel, sessionResumeLabel, sessionResumeTargetForAgent, sessionResumeTargetForMenu, SessionResumeTarget, SessionSortKey, sortValue, summarizeSessionPreviewRecord, type SessionRecord, type SessionResumeState } from "../../lib/index.ts";
import { cacheRateTone } from "../../lib/token-style.ts";
import { AgentBadge } from "../../components/shared/AgentBadge.tsx";
import { Badge } from "../../components/shared/Badge.tsx";
import { LoadingIcon } from "../../components/shared/LoadingIcon.tsx";
import { StatefulButton } from "../../components/shared/StatefulButton.tsx";
import { SessionTitleText, TranscriptLinkText } from "../../components/shared/TranscriptLinkText.tsx";
import { formatSessionTitle } from "../../lib/session-preview.ts";

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
  resumeState?: (session: T) => SessionResumeState;
  resumeTarget?: SessionResumeTarget;
  resumeTargetForSession?: (session: T) => Exclude<SessionResumeTarget, SessionResumeTarget.Auto> | undefined;
  keys?: string[];
  widths?: Partial<Record<string, string>>;
};

export function createSessionTableColumns<T extends SessionTableRow = SessionTableRow>({
  normalizedQuery = "",
  resumeSession,
  resumeState,
  resumeTarget = SessionResumeTarget.Auto,
  resumeTargetForSession,
  keys,
  widths = {},
}: CreateSessionTableColumnsOptions<T> = {}): ColumnDef<T>[] {
  const canResume = typeof resumeSession === "function";
  const columns: ColumnDef<T>[] = [
    {
      key: SessionSortKey.Title,
      header: "Session",
      type: ColumnDataType.Text,
      sticky: true,
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, SessionSortKey.Title),
      width: widths.title ?? "var(--data-freeze-column-width, 360px)",
      render: (session) => {
        const preview = summarizeSessionPreviewRecord(session as SessionRecord);
        const displayTitle = `${session.title ?? ""}`;
        const tooltipTitle = formatSessionTitle(displayTitle);
        return (
          <>
            <span className="sessionTitleWithKind">
              <Tooltip content={tooltipTitle} onlyWhenTruncated>
                <span className="sessionTitleText"><SessionTitleText interactive={false} value={displayTitle} /></span>
              </Tooltip>
              {sessionKind(session as SessionRecord) === SessionKind.Child ? <Badge tone="warning" uppercase>Child</Badge> : null}
            </span>
            {normalizedQuery && session.searchSnippet ? (
              <span className="dataCellSubLine">
                <Tooltip content={session.searchSnippet.replace(/[⟦⟧]/g, "")} onlyWhenTruncated><span className="dataCellSub sessionSearchSnippet">
                  <TranscriptLinkText interactive={false} query={normalizedQuery} value={session.searchSnippet.replace(/[⟦⟧]/g, "")} />
                </span></Tooltip>
              </span>
            ) : (
              <span className="dataCellSubLine sessionPreviewSubLine">
                <span className="sessionPreviewMessage">
                  <ArrowRight size={13} aria-hidden="true" />
                  <span className="dataCellSub sessionPreviewText"><TranscriptLinkText interactive={false} value={preview?.userLast ?? "—"} /></span>
                </span>
                <span className="sessionPreviewMessage">
                  <ArrowLeft size={13} aria-hidden="true" />
                  <span className="dataCellSub sessionPreviewText"><TranscriptLinkText interactive={false} value={preview?.assistantLast ?? "—"} /></span>
                </span>
              </span>
            )}
          </>
        );
      },
    },
    {
      key: SessionSortKey.Agent,
      header: "Agent",
      type: ColumnDataType.Enum,
      groupBy: (session) => friendlyAgent(session.agent),
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, SessionSortKey.Agent),
      width: widths.agent ?? "78px",
      render: (session) => {
        if (!canResume) {
          return (
            <span className="sessionAgentCell">
              <AgentBadge agent={friendlyAgent(session.agent)} />
            </span>
          );
        }
        const state = resumeState?.(session as T) ?? AsyncStatus.Idle;
        const configuredTarget = sessionResumeTargetForAgent(resumeTarget, session.agent);
        const resolvedTarget = sessionResumeTargetForMenu(configuredTarget, resumeTargetForSession?.(session as T));
        const label = sessionResumeLabel(state, resolvedTarget);
        return (
          <Tooltip content={label}><StatefulButton
            size="sm"
            width={16}
            minWidth={16}
            className="sessionAgentCell sessionAgentButton"
            state={state}
            aria-label={label}
            aria-disabled={state === AsyncStatus.Loading || undefined}
            data-no-row-click
            onClick={(event) => {
              event.stopPropagation();
              if (state === AsyncStatus.Loading) return;
              resumeSession?.(session as T);
            }}
            onKeyDown={(event) => event.stopPropagation()}
            loadingContent={<LoadingIcon size={14} />}
            successContent={<Check size={14} aria-hidden="true" />}
            errorContent={<AlertCircle size={14} aria-hidden="true" />}
          >
            <AgentBadge agent={friendlyAgent(session.agent)} />
          </StatefulButton></Tooltip>
        );
      },
    },
    {
      key: SessionSortKey.Project,
      header: "Project",
      type: ColumnDataType.Enum,
      groupBy: (session) => sessionProjectGroupKey(session as SessionRecord),
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, SessionSortKey.Project),
      width: widths.project ?? "202px",
      value: (session) => sessionProject(session as SessionRecord),
      groupLabel: sessionProjectGroupLabel,
    },
    {
      key: SessionSortKey.StartedAt,
      header: "Started",
      type: ColumnDataType.Date,
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, SessionSortKey.StartedAt),
      width: widths.startedAt ?? "104px",
      value: (session) => session.startedLabel,
      title: (session) => session.startedAt,
    },
    {
      key: SessionSortKey.UpdatedAt,
      header: "Updated",
      type: ColumnDataType.Date,
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, SessionSortKey.UpdatedAt),
      width: widths.updatedAt ?? "104px",
      value: (session) => session.updatedLabel,
      title: (session) => session.updatedAt,
    },
    {
      key: SessionSortKey.Messages,
      header: "Msgs",
      type: ColumnDataType.Text,
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, SessionSortKey.Messages),
      width: widths.messages ?? "70px",
      cell: ColumnCellVariant.Number,
    },
    {
      key: SessionSortKey.Turns,
      header: "Turns",
      type: ColumnDataType.Text,
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, SessionSortKey.Turns),
      width: widths.turns ?? "70px",
      cell: ColumnCellVariant.Number,
      value: (session) => session.turnCount,
    },
    {
      key: SessionSortKey.CacheRate,
      header: "Cache",
      type: ColumnDataType.Text,
      sortable: true,
      sortValue: (session) => sortValue(session as SessionRecord, SessionSortKey.CacheRate),
      width: widths.cacheRate ?? "70px",
      cell: ColumnCellVariant.Number,
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
