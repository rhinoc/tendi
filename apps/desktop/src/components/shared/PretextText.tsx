import { createElement, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type MouseEvent, type ReactNode } from "react";

import { safeInvoke, TauriCommand } from "../../lib/tauri.ts";
import { markdownToHtml } from "../../lib/tiptap.ts";
import { layoutTranscriptText, type TranscriptTextLayout } from "../../lib/pretext-layout.ts";
import { formatTranscriptText, transcriptLinkLabel, transcriptLinkTokens } from "../../lib/transcript-format.ts";
import { highlightTranscriptText } from "./TranscriptLinkText.tsx";
import "./PretextText.css";

const ALLOWED_TAGS = new Set([
  "a",
  "b",
  "blockquote",
  "br",
  "code",
  "del",
  "em",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "i",
  "li",
  "ol",
  "p",
  "pre",
  "s",
  "strong",
  "ul",
]);

export type PretextTextProps = {
  className?: string;
  content: string;
  interactiveLinks?: boolean;
  markdown?: boolean;
  query?: string;
  queryTerms?: readonly string[];
};

function handleLinkClick(event: MouseEvent<HTMLAnchorElement>, href: string) {
  event.preventDefault();
  event.stopPropagation();
  void safeInvoke(TauriCommand.OpenUrl, { url: href });
}

function escapeMarkdownLabel(value: string) {
  return value.replace(/\\/g, "\\\\").replace(/\[/g, "\\[").replace(/\]/g, "\\]");
}

function prepareContent(content: string) {
  const tokens = transcriptLinkTokens(content);
  if (tokens.length === 0) return content;

  let prepared = "";
  let offset = 0;
  for (const token of tokens) {
    prepared += content.slice(offset, token.start);
    prepared += `[${escapeMarkdownLabel(transcriptLinkLabel(token.url, token.label))}](${token.url})`;
    prepared += token.trailing;
    offset = token.end;
  }
  return prepared + content.slice(offset);
}

function parseCssPixels(value: string, fallback: number) {
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function readMetrics(element: HTMLDivElement): TranscriptTextLayout | null {
  const contentWidth = element.clientWidth;
  if (contentWidth <= 0) return null;

  const styles = getComputedStyle(element);
  const fontSize = parseCssPixels(styles.fontSize, 14);
  const font = `${styles.fontWeight} ${fontSize}px ${styles.fontFamily}`;
  const letterSpacing = styles.letterSpacing === "normal" ? 0 : parseCssPixels(styles.letterSpacing, 0);
  return {
    contentWidth,
    font,
    linkFont: `600 ${fontSize}px ${styles.fontFamily}`,
    lineHeight: parseCssPixels(styles.lineHeight, 20),
    letterSpacing,
  };
}

function renderNode(
  node: ChildNode,
  key: string,
  query: string,
  queryTerms: readonly string[] | undefined,
  interactiveLinks: boolean,
): ReactNode {
  if (node.nodeType === 3) return highlightTranscriptText(node.textContent ?? "", query, queryTerms);
  if (node.nodeType !== 1) return null;

  const element = node as Element;
  const tagName = element.tagName.toLowerCase();
  const children = Array.from(element.childNodes).map((child, index) => (
    renderNode(child, `${key}-${index}`, query, queryTerms, interactiveLinks)
  ));
  if (!ALLOWED_TAGS.has(tagName)) return children;

  if (tagName === "a") {
    const href = element.getAttribute("href") ?? "";
    if (!/^https?:\/\//i.test(href)) return children;
    return (
      <a
        key={key}
        href={href}
        onClick={interactiveLinks ? (event) => handleLinkClick(event, href) : undefined}
      >
        {children}
      </a>
    );
  }

  return tagName === "br"
    ? createElement(tagName, { key })
    : createElement(tagName, { key }, children);
}

export function PretextText({
  className,
  content,
  interactiveLinks = true,
  markdown = true,
  query = "",
  queryTerms,
}: PretextTextProps) {
  const elementRef = useRef<HTMLDivElement>(null);
  const [metrics, setMetrics] = useState<TranscriptTextLayout | null>(null);

  useLayoutEffect(() => {
    if (markdown) return;
    const element = elementRef.current;
    if (!element) return;

    const update = () => {
      const next = readMetrics(element);
      setMetrics((current) => (
        current?.contentWidth === next?.contentWidth
          && current?.font === next?.font
          && current?.linkFont === next?.linkFont
          && current?.lineHeight === next?.lineHeight
          && current?.letterSpacing === next?.letterSpacing
          ? current
          : next
      ));
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, [markdown]);

  const renderedContent = useMemo(() => {
    if (!markdown) return null;
    if (typeof DOMParser === "undefined") return null;
    const parsedDocument = new DOMParser().parseFromString(markdownToHtml(prepareContent(content)), "text/html");
    return Array.from(parsedDocument.body.childNodes).map((node, index) => (
      renderNode(node, `pretext-${index}`, query, queryTerms, interactiveLinks)
    ));
  }, [content, interactiveLinks, markdown, query, queryTerms]);

  const plainLines = useMemo(
    () => !markdown && metrics ? layoutTranscriptText(content, metrics) : null,
    [content, markdown, metrics],
  );

  const renderedPlainContent = plainLines?.map((line, lineIndex) => (
    <div className="pretextTextLine" key={`line-${lineIndex}`}>
      {line.fragments.map((fragment, fragmentIndex) => {
        const style: CSSProperties | undefined = fragment.leadingGap > 0
          ? { marginLeft: `${fragment.leadingGap}px` }
          : undefined;
        const children = highlightTranscriptText(fragment.text, query, queryTerms);
        if (!fragment.href) {
          return <span key={`fragment-${fragmentIndex}`} style={style}>{children}</span>;
        }
        return (
          <a
            key={`fragment-${fragmentIndex}`}
            href={fragment.href}
            style={style}
            onClick={interactiveLinks ? (event) => handleLinkClick(event, fragment.href!) : undefined}
          >
            {children}
          </a>
        );
      })}
    </div>
  ));

  // Keep manual line breaks at the measured width. Letting a fit-content
  // parent remeasure the new longest line creates a feedback loop.
  const manualLayoutStyle = !markdown && metrics
    ? { width: `${metrics.contentWidth}px` }
    : undefined;

  return (
    <div
      ref={elementRef}
      style={manualLayoutStyle}
      className={`pretextText${markdown ? "" : " pretextTextManual"}${className ? ` ${className}` : ""}`}
    >
      {markdown ? renderedContent : renderedPlainContent ?? <span>{formatTranscriptText(content)}</span>}
    </div>
  );
}
