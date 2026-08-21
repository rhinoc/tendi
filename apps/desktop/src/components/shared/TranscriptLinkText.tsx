import { Link2 } from "lucide-react";
import type { MouseEvent as ReactMouseEvent, ReactNode } from "react";

import { safeInvoke, TauriCommand } from "../../lib/tauri.ts";
import { formatMarkdownLinkLabels, transcriptLinkLabel, transcriptLinkTokens } from "../../lib/transcript-format.ts";
import { findTextRanges } from "./text-ranges.ts";
import "./TranscriptLinkText.css";

export type TranscriptLinkTextProps = {
  interactive?: boolean;
  query?: string;
  value: string;
};

export type SessionTitleTextProps = Omit<TranscriptLinkTextProps, "value"> & {
  value: string | null | undefined;
};

function highlightText(value: string, query: string): ReactNode {
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

function openLink(event: ReactMouseEvent<HTMLAnchorElement>, url: string) {
  event.preventDefault();
  event.stopPropagation();
  void safeInvoke(TauriCommand.OpenUrl, { url });
}

export function TranscriptLinkText({ interactive = true, query = "", value }: TranscriptLinkTextProps) {
  const tokens = transcriptLinkTokens(value);
  if (tokens.length === 0) return highlightText(value, query);

  const nodes: ReactNode[] = [];
  let offset = 0;
  const needle = query.trim().toLowerCase();
  tokens.forEach((token, index) => {
    if (token.start > offset) nodes.push(highlightText(value.slice(offset, token.start), query));
    const label = transcriptLinkLabel(token.url, token.label);
    const isSearchMatch = Boolean(needle && `${token.label ?? ""} ${token.url}`.toLowerCase().includes(needle));
    const linkContent = (
      <span className="transcriptLinkContent">
        <Link2 aria-hidden="true" className="transcriptLinkIcon" size={12} strokeWidth={2} />
        <span className="transcriptLinkLabel">{highlightText(label, query)}</span>
      </span>
    );
    const link = interactive ? (
      <a
        aria-label={token.url}
        className={`transcriptLink${isSearchMatch ? " isSearchMatch" : ""}`}
        href={token.url}
        onClick={(event) => openLink(event, token.url)}
      >
        {linkContent}
      </a>
    ) : (
      <span aria-label={token.url} className={`transcriptLink${isSearchMatch ? " isSearchMatch" : ""}`}>
        {linkContent}
      </span>
    );
    nodes.push(<span className={interactive ? undefined : "transcriptStaticLink"} key={`transcript-link-${token.start}-${index}`}>{link}</span>);
    if (token.trailing) nodes.push(token.trailing);
    offset = token.end;
  });
  if (offset < value.length) nodes.push(highlightText(value.slice(offset), query));
  return nodes;
}

export function SessionTitleText({ value, ...props }: SessionTitleTextProps) {
  return <TranscriptLinkText {...props} value={formatMarkdownLinkLabels(value ?? "")} />;
}
