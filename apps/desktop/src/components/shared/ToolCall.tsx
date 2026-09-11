import { ArrowUpRight, Ban, Check, CircleX } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { formatDuration } from "../../lib/strings.ts";
import { EMPTY_DISPLAY_VALUE } from "../../lib/constants.ts";
import { Tooltip as AppTooltip } from "./Tooltip.tsx";
import { Badge } from "./Badge.tsx";
import { Disclosure } from "./Disclosure.tsx";
import { LoadingIcon } from "./LoadingIcon.tsx";
import { findTextRanges } from "./text-ranges.ts";
import "./ToolCall.css";

export type ToolCallItem = {
  tag?: string;
  command?: string;
  result?: string;
  durationMs?: string | number;
  linkedSessionId?: string;
};

export type ToolCallStatus = "running" | "complete" | "error" | "cancelled";

type ToolCallProps = {
  item: ToolCallItem;
  itemKey: string;
  summary?: string;
  status?: ToolCallStatus;
  nested?: boolean;
  highlighted?: boolean;
  searchQuery?: string;
  defaultOpen?: boolean;
  onOpenLinkedSession?: (sessionId: string) => void;
};

function highlightToolCallText(value: string, query: string): ReactNode {
  const ranges = findTextRanges(value, query);
  if (ranges.length === 0) return value;

  const parts: ReactNode[] = [];
  let offset = 0;
  for (const range of ranges) {
    if (range.from > offset) parts.push(value.slice(offset, range.from));
    parts.push(<mark className="transcriptSearchMark" key={`${range.from}-${parts.length}`}>{value.slice(range.from, range.to)}</mark>);
    offset = range.to;
  }
  if (offset < value.length) parts.push(value.slice(offset));
  return parts;
}

function statusLabel(status: ToolCallStatus): string {
  if (status === "running") return "Working";
  if (status === "complete") return "Completed";
  if (status === "error") return "Failed";
  return "Cancelled";
}

function statusIcon(status: ToolCallStatus): ReactNode {
  if (status === "running") return <LoadingIcon size={13} />;
  if (status === "complete") return <Check size={13} />;
  if (status === "error") return <CircleX size={13} />;
  return <Ban size={13} />;
}

export function ToolCall({
  item,
  itemKey,
  summary,
  status,
  nested = false,
  highlighted = false,
  searchQuery = "",
  defaultOpen = status === "running",
  onOpenLinkedSession,
}: ToolCallProps) {
  const [open, setOpen] = useState(defaultOpen);
  const command = item.command ?? "";
  const result = item.result ?? "";
  const summaryText = summary?.trim() || command || EMPTY_DISPLAY_VALUE;
  const duration = formatDuration(item.durationMs);
  const detailsId = `tool-call-details-${itemKey.replace(/[^a-zA-Z0-9_-]/g, "-")}`;

  useEffect(() => {
    if (status) setOpen(status === "running");
  }, [status]);

  return (
    <Disclosure
      className={`toolCall ${nested ? "nested" : ""} ${highlighted ? "transcriptTarget" : ""}`}
      data-transcript-key={itemKey}
      data-status={status}
      open={open}
      onOpenChange={setOpen}
      summaryRowClassName="toolCallSummary"
      summaryClassName="toolCallDisclosure"
      detailsClassName="toolCallDetails"
      detailsId={detailsId}
      contentPadding="compact"
      summaryActions={item.linkedSessionId && onOpenLinkedSession ? (
        <button
          type="button"
          className="toolCallSessionLink"
          aria-label="Open child session"
          onClick={() => onOpenLinkedSession(item.linkedSessionId!)}
        >
          <ArrowUpRight size={13} aria-hidden="true" />
        </button>
      ) : null}
      summary={(
        <>
          {status ? (
            <span className="toolCallStatusIcon" aria-hidden="true">{statusIcon(status)}</span>
          ) : item.tag ? (
            <Badge tone="neutral" mono>{item.tag}</Badge>
          ) : null}
          <AppTooltip content={summaryText} onlyWhenTruncated>
            <code>{highlightToolCallText(summaryText, searchQuery)}</code>
          </AppTooltip>
          {status ? <span className="toolCallStatus">{statusLabel(status)}</span> : duration ? <Badge tone="neutral" className="toolCallDuration">{duration}</Badge> : null}
        </>
      )}
    >
      {open ? (
        <>
          <div className="toolCallBlock">
            <div className="toolCallLabel">Command</div>
            <pre>{highlightToolCallText(command || EMPTY_DISPLAY_VALUE, searchQuery)}</pre>
          </div>
          <div className="toolCallBlock">
            <div className="toolCallLabel">Output</div>
            {status === "running" && !result ? (
              <span className="toolCallWaiting">Waiting for output…</span>
            ) : (
              <pre>{highlightToolCallText(result || EMPTY_DISPLAY_VALUE, searchQuery)}</pre>
            )}
          </div>
        </>
      ) : null}
    </Disclosure>
  );
}
