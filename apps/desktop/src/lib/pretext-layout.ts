import {
  materializeRichInlineLineRange,
  prepareRichInline,
  walkRichInlineLineRanges,
} from "@chenglou/pretext/rich-inline";

import { transcriptLinkLabel, transcriptLinkTokens } from "./transcript-format.ts";

export type TranscriptTextLayout = {
  contentWidth: number;
  font: string;
  linkFont: string;
  lineHeight: number;
  letterSpacing: number;
};

export type TranscriptTextFragment = {
  href: string | null;
  leadingGap: number;
  text: string;
};

export type TranscriptTextLine = {
  fragments: TranscriptTextFragment[];
  width: number;
};

type TranscriptInlineItem = {
  font: string;
  href: string | null;
  letterSpacing: number;
  text: string;
};

function inlineItemsFor(value: string, metrics: TranscriptTextLayout): TranscriptInlineItem[] {
  const tokens = transcriptLinkTokens(value);
  if (tokens.length === 0) {
    return [{ text: value, font: metrics.font, letterSpacing: metrics.letterSpacing, href: null }];
  }

  const items: TranscriptInlineItem[] = [];
  let offset = 0;
  for (const token of tokens) {
    const before = value.slice(offset, token.start);
    if (before) items.push({ text: before, font: metrics.font, letterSpacing: metrics.letterSpacing, href: null });
    items.push({
      text: transcriptLinkLabel(token.url, token.label),
      font: metrics.linkFont,
      letterSpacing: metrics.letterSpacing,
      href: token.url,
    });
    if (token.trailing) items.push({ text: token.trailing, font: metrics.font, letterSpacing: metrics.letterSpacing, href: null });
    offset = token.end;
  }
  const after = value.slice(offset);
  if (after) items.push({ text: after, font: metrics.font, letterSpacing: metrics.letterSpacing, href: null });
  return items;
}

export function layoutTranscriptText(value: string, metrics: TranscriptTextLayout): TranscriptTextLine[] {
  const lines: TranscriptTextLine[] = [];
  for (const hardLine of value.split("\n")) {
    const items = inlineItemsFor(hardLine, metrics);
    const flow = prepareRichInline(items);
    let lineCount = 0;
    walkRichInlineLineRanges(flow, metrics.contentWidth, (range) => {
      const line = materializeRichInlineLineRange(flow, range);
      lines.push({
        width: line.width,
        fragments: line.fragments.map((fragment) => ({
          href: items[fragment.itemIndex]?.href ?? null,
          leadingGap: fragment.gapBefore,
          text: fragment.text,
        })),
      });
      lineCount += 1;
    });
    if (lineCount === 0) lines.push({ fragments: [], width: 0 });
  }
  return lines;
}

export function measureTranscriptTextHeight(
  value: string,
  metrics: TranscriptTextLayout,
) {
  if (typeof document === "undefined" || metrics.contentWidth <= 0 || metrics.lineHeight <= 0) return null;
  return Math.max(metrics.lineHeight, layoutTranscriptText(value, metrics).length * metrics.lineHeight);
}
