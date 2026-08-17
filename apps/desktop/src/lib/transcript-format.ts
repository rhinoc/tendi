export type TranscriptLinkToken = {
  end: number;
  label?: string;
  start: number;
  trailing: string;
  url: string;
};

const TRANSCRIPT_LINK_PATTERN = /\[([^\]\n]+)\]\((https?:\/\/[^)\s]+)\)|https?:\/\/[^\s<>"'`]+/gi;
const NON_HTTP_MARKDOWN_LINK_PATTERN = /(^|[^!])\[([^\]\n]+)\]\((?!(?:https?:)?\/\/)[^)\n]+\)/gi;

export function formatMarkdownLinkLabels(value: string) {
  return value.replace(
    NON_HTTP_MARKDOWN_LINK_PATTERN,
    (_match: string, prefix: string, label: string) => `${prefix}${label}`,
  );
}

export function trimLinkEnd(value: string) {
  let end = value.length;
  while (end > 0 && /[.,!?;:，。！？；：、）》）】］}…"'`]/.test(value[end - 1])) end -= 1;
  return { value: value.slice(0, end), trailing: value.slice(end) };
}

export function transcriptLinkTokens(value: string): TranscriptLinkToken[] {
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

export function transcriptLinkLabel(url: string, providedLabel?: string) {
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

export function formatTranscriptText(value: string) {
  const tokens = transcriptLinkTokens(value);
  if (tokens.length === 0) return value;

  let formatted = "";
  let offset = 0;
  for (const token of tokens) {
    formatted += value.slice(offset, token.start);
    formatted += transcriptLinkLabel(token.url, token.label);
    formatted += token.trailing;
    offset = token.end;
  }
  return formatted + value.slice(offset);
}
