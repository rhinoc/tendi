import { Tooltip } from "./shared/Tooltip.tsx";
import { Info } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";

import { createTokenizerWorker, type TokenizerWorkerClient } from "../lib/tokenizer-client.ts";
import { formatTokenCount } from "../lib/token-format.ts";
import {
  TOKENIZER_PACKAGE,
  TOKENIZER_URL,
  type TokenBreakdownDetail,
  type TokenBreakdownSegment,
} from "../lib/tokenizer-types.ts";
import { tokenToneClass, type TokenTone } from "../lib/token-style.ts";
import { TauriCommand, safeInvoke } from "../lib/tauri";
import "./TokenStatusBar.css";

type TokenStatusBarProps = {
  activePath?: string;
  content?: string;
  selectionText?: string;
  segments?: TokenSegmentProps[];
  metrics?: TokenMetricProps[];
  usageSource?: "estimated" | "reported";
  leadingSlot?: ReactNode;
};

export type TokenMetricProps = {
  label: string;
  value: string;
  title?: string;
  tone?: TokenTone;
};

export type TokenSegmentProps = {
  label: string;
  value: number;
  details?: TokenBreakdownDetail[];
  notes?: string[];
};

function tokenValue(value: number, usageSource: "estimated" | "reported") {
  return `${usageSource === "estimated" ? "~" : ""}${formatTokenCount(value)}`;
}

export function TokenSegment({ label, value, usageSource = "estimated" }: TokenSegmentProps & { usageSource?: "estimated" | "reported" }) {
  return (
    <span className="tokenSegment">
      <span className="tokenSegmentLabel">{label}</span>{" "}
      <span className={`tokenSegmentValue ${tokenToneClass(value)}`}>{tokenValue(value, usageSource)}</span>
    </span>
  );
}

function tokenBreakdownRows(segments: TokenSegmentProps[]) {
  return segments.flatMap((segment) => {
    if (segment.label === "Total") return [];
    const details = segment.details && segment.details.length > 0
      ? segment.details
      : [{ label: segment.label, value: segment.value }];
    return details.map((detail) => ({
      bucket: segment.label,
      label: detail.label,
      value: detail.value,
    }));
  });
}

function tokenBreakdownNotes(segments: TokenSegmentProps[]) {
  return [...new Set(segments.flatMap((segment) => segment.notes ?? []))];
}

function openTokenizerUrl(event: React.MouseEvent<HTMLAnchorElement>) {
  event.preventDefault();
  event.stopPropagation();
  void safeInvoke(TauriCommand.OpenUrl, { url: TOKENIZER_URL });
}

export function TokenBreakdownPanel({ segments, usageSource = "estimated" }: { segments: TokenSegmentProps[]; usageSource?: "estimated" | "reported" }) {
  const rows = tokenBreakdownRows(segments);
  const notes = tokenBreakdownNotes(segments);
  const showTable = rows.length > 1;
  return (
    <div className="tokenBreakdownPanel" role="tooltip" data-selectable-text onMouseDown={(event) => event.stopPropagation()}>
      <div className="tokenBreakdownHeader">
        {usageSource === "reported" ? (
          <>
            <span>Actual token usage</span>
            <span>Read from usage records reported in the session JSONL. No tokenizer estimate.</span>
          </>
        ) : (
          <>
            <span>Estimated token usage</span>
            <span>
              ~ values use OpenAI o200k_base via{" "}
              <a href={TOKENIZER_URL} onClick={openTokenizerUrl}>{TOKENIZER_PACKAGE}</a>
              . They are not billing usage.
            </span>
          </>
        )}
      </div>
      {showTable ? (
        <div className="tokenBreakdownTableWrap">
          <table className="tokenBreakdownTable">
            <thead>
              <tr>
                <th>Bucket</th>
                <th>Category</th>
                <th>Tokens</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={`${row.bucket}-${row.label}`}>
                  <td><span className={`tokenBucket tokenBucket-${row.bucket.toLowerCase()}`}>{row.bucket}</span></td>
                  <Tooltip content={row.label} onlyWhenTruncated><td>{row.label}</td></Tooltip>
                  <td className={tokenToneClass(row.value)}>{tokenValue(row.value, usageSource)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="tokenBreakdownSingle">
          {rows.length === 1 ? (
            <>
              <span className={`tokenBucket tokenBucket-${rows[0].bucket.toLowerCase()}`}>{rows[0].bucket}</span>
              <Tooltip content={rows[0].label} onlyWhenTruncated><span>{rows[0].label}</span></Tooltip>
              <strong className={tokenToneClass(rows[0].value)}>{tokenValue(rows[0].value, usageSource)}</strong>
            </>
          ) : (
            <span>No token details</span>
          )}
        </div>
      )}
      {notes.length > 0 ? (
        <div className="tokenBreakdownNotes">
          {notes.map((note) => <span key={note}>{note}</span>)}
        </div>
      ) : null}
    </div>
  );
}

export function TokenEstimateInfo() {
  return (
    <span className="tokenEstimateInfo">
      <Tooltip content="Token breakdown"><button type="button" className="tokenEstimateInfoButton" aria-label="Show token breakdown">
        <Info size={12} />
      </button></Tooltip>
    </span>
  );
}

export function TokenSegments({ segments, usageSource = "estimated" }: { segments: TokenSegmentProps[]; usageSource?: "estimated" | "reported" }) {
  return (
    <span className="tokenSegments">
      {segments.map((segment, index) => (
        <span className="tokenSegmentWrap" key={segment.label}>
          {index > 0 && <span className="tokenSegmentDivider">|</span>}
          <TokenSegment {...segment} usageSource={usageSource} />
        </span>
      ))}
    </span>
  );
}

export function TokenMetrics({ metrics }: { metrics: TokenMetricProps[] }) {
  return metrics.map((metric) => (
    <span className="tokenMetricWrap" key={metric.label}>
      <span className="tokenSegmentDivider">|</span>
      <span className="tokenMetric">
        <span className="tokenSegmentLabel">{metric.label}</span>{" "}
        <span className={`tokenMetricValue ${metric.tone ? `tokenTone-${metric.tone}` : ""}`}>{metric.value}</span>
      </span>
    </span>
  ));
}

export function TokenStatusBar({ activePath = "", content = "", selectionText = "", segments: providedSegments, metrics = [], usageSource = "estimated", leadingSlot }: TokenStatusBarProps) {
  const workerRef = useRef<TokenizerWorkerClient | null>(null);
  const latestRequestRef = useRef(0);
  const [estimatedSegments, setEstimatedSegments] = useState<TokenBreakdownSegment[] | null>(null);

  useEffect(() => {
    if (providedSegments) return;
    const worker = createTokenizerWorker(
      (response) => {
        if (response.id !== latestRequestRef.current) return;
        if (response.type === "result") setEstimatedSegments(response.segments);
        else setEstimatedSegments([]);
      },
      () => {
        if (latestRequestRef.current > 0) setEstimatedSegments([]);
      },
    );
    workerRef.current = worker;
    return () => {
      workerRef.current = null;
      worker.dispose();
    };
  }, [providedSegments]);

  useEffect(() => {
    if (providedSegments || !workerRef.current) return;
    setEstimatedSegments(null);
    latestRequestRef.current = workerRef.current.request({
      kind: "markdown",
      activePath,
      content,
      selectionText,
    });
  }, [activePath, content, providedSegments, selectionText]);

  const segments = providedSegments ?? estimatedSegments ?? [];

  return (
    <div className={`tokenStatusCluster ${leadingSlot ? "hasLeadingSlot" : ""}`}>
      {leadingSlot ? <div className="tokenStatusSlot">{leadingSlot}</div> : null}
      <div className="tokenStatusBar" aria-label="Token statistics" tabIndex={0}>
        <TokenSegments segments={segments} usageSource={usageSource} />
        <TokenMetrics metrics={metrics} />
        <TokenEstimateInfo />
        <TokenBreakdownPanel segments={segments} usageSource={usageSource} />
      </div>
    </div>
  );
}
