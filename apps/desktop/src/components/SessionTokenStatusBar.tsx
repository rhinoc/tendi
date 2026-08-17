import { useEffect, useRef, useState } from "react";

import { TokenStatusBar, type TokenMetricProps, type TokenSegmentProps } from "./TokenStatusBar.tsx";
import {
  compactTranscriptSkillLinks,
  compactTranscriptTokenItems,
  createTokenizerWorker,
  type TokenizerWorkerClient,
} from "../lib/tokenizer-client.ts";
import type { TranscriptSkillLink, TranscriptTokenItem } from "../lib/tokenizer-types.ts";

export type SessionTokenStatusBarProps = {
  items: TranscriptTokenItem[];
  skillLinks: TranscriptSkillLink[];
  reportedSegments: TokenSegmentProps[] | null;
  metrics: TokenMetricProps[];
  usageSource: "estimated" | "reported";
};

type EstimateState = {
  status: "loading" | "ready" | "error";
  segments: TokenSegmentProps[] | null;
};

const emptySegments: TokenSegmentProps[] = [];

export function SessionTokenStatusBar({ items, skillLinks, reportedSegments, metrics, usageSource }: SessionTokenStatusBarProps) {
  const [estimate, setEstimate] = useState<EstimateState>({ status: "loading", segments: null });
  const workerRef = useRef<TokenizerWorkerClient | null>(null);
  const latestRequestRef = useRef(0);

  useEffect(() => {
    if (reportedSegments) {
      workerRef.current?.dispose();
      workerRef.current = null;
      setEstimate({ status: "ready", segments: null });
      return;
    }

    setEstimate({ status: "loading", segments: null });
    const worker = createTokenizerWorker(
      (response) => {
        if (response.id !== latestRequestRef.current) return;
        if (response.type === "result") setEstimate({ status: "ready", segments: response.segments });
        else setEstimate({ status: "error", segments: null });
      },
      () => setEstimate({ status: "error", segments: null }),
    );
    workerRef.current = worker;

    return () => {
      if (workerRef.current === worker) workerRef.current = null;
      worker.dispose();
    };
  }, [reportedSegments]);

  useEffect(() => {
    if (reportedSegments || !workerRef.current) return;
    setEstimate({ status: "loading", segments: null });
    latestRequestRef.current = workerRef.current.request({
      kind: "transcript",
      items: compactTranscriptTokenItems(items),
      skillLinks: compactTranscriptSkillLinks(skillLinks),
    });
  }, [items, reportedSegments, skillLinks]);

  const segments = reportedSegments ?? estimate.segments ?? emptySegments;
  return <TokenStatusBar segments={segments} metrics={metrics} usageSource={usageSource} />;
}
