import { Link2 } from "lucide-react";
import type { MouseEvent as ReactMouseEvent, ReactNode } from "react";

import { safeInvoke, TauriCommand } from "../../lib/tauri.ts";
import "./TranscriptLinkText.css";

type TranscriptLinkToken = {
  end: number;
  label?: string;
  start: number;
  trailing: string;
  url: string;
};

export type TranscriptLinkTextProps = {
  interactive?: boolean;
  query?: string;
  value: string;
};

const TRANSCRIPT_LINK_PATTERN = /\[([^\]\n]+)\]\((https?:\/\/[^)\s]+)\)|https?:\/\/[^\s<>"'`]+/gi;

function highlightText(value: string, query: string): ReactNode {
  const needle = query.trim().toLowerCase();
  if (!needle) return value;

  const parts: ReactNode[] = [];
  const haystack = value.toLowerCase();
  let offset = 0;
  let matchIndex = haystack.indexOf(needle, offset);
  while (matchIndex >= 0) {
    if (matchIndex > offset) parts.push(value.slice(offset, matchIndex));
    parts.push(<mark className="transcriptSearchMark" key={`${matchIndex}-${parts.length}`}>{value.slice(matchIndex, matchIndex + needle.length)}</mark>);
    offset = matchIndex + needle.length;
    matchIndex = haystack.indexOf(needle, offset);
  }
  if (offset < value.length) parts.push(value.slice(offset));
  return parts;
}

function trimLinkEnd(value: string) {
  let end = value.length;
  while (end > 0 && /[.,!?;:，。！？；：、）》）】］}…"'`]/.test(value[end - 1])) end -= 1;
  return { value: value.slice(0, end), trailing: value.slice(end) };
}

function linkTokens(value: string): TranscriptLinkToken[] {
  const tokens: TranscriptLinkToken[] = [];
  for (const match of value.matchAll(TRANSCRIPT_LINK_PATTERN)) {
    const raw = match[0];
    const markdownUrl = match[2];
    const trimmed = trimLinkEnd(markdownUrl ?? raw);
    if (!trimmed.value) continue;
    tokens.push({
      end: match.index! + raw.length,
      label: markdownUrl ? match[1] : undefined,
      start: match.index!,
      trailing: trimmed.trailing,
      url: trimmed.value,
    });
  }
  return tokens;
}

function shortenLinkPart(value: string, limit = 24) {
  if (value.length <= limit) return value;
  return `${value.slice(0, Math.max(8, limit - 8))}…${value.slice(-6)}`;
}

function linkLabel(url: string, providedLabel?: string) {
  const label = providedLabel?.trim();
  if (label && label !== url) return label;

  try {
    const parsed = new URL(url);
    const hostParts = parsed.hostname.replace(/^www\./, "").split(".");
    const host = hostParts.slice(-2).join(".") || parsed.hostname;
    const segments = parsed.pathname.split("/").filter(Boolean);
    const path = shortenLinkPart(segments.slice(-2).join("/") || host);
    return path === host ? host : `${host} · ${path}`;
  } catch {
    return "链接";
  }
}

function openLink(event: ReactMouseEvent<HTMLAnchorElement>, url: string) {
  event.preventDefault();
  event.stopPropagation();
  void safeInvoke(TauriCommand.OpenUrl, { url });
}

export function TranscriptLinkText({ interactive = true, query = "", value }: TranscriptLinkTextProps) {
  const tokens = linkTokens(value);
  if (tokens.length === 0) return highlightText(value, query);

  const nodes: ReactNode[] = [];
  let offset = 0;
  const needle = query.trim().toLowerCase();
  tokens.forEach((token, index) => {
    if (token.start > offset) nodes.push(highlightText(value.slice(offset, token.start), query));
    const label = linkLabel(token.url, token.label);
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
