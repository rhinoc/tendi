import { useEffect, useRef, useState } from "react";

import { TokenStatusBar, type TokenMetricProps, type TokenSegmentProps } from "./TokenStatusBar.tsx";
import {
  compactTranscriptSkillLinks,
  compactTranscriptTokenItems,
  createTokenizerWorker,
  type TokenizerWorkerClient,
} from "../lib/tokenizer-client.ts";
import { TokenEstimateStatus, TokenUsageSource, TokenizerKind, TokenizerWorkerResponseType, type TranscriptSkillLink, type TranscriptTokenItem } from "../lib/tokenizer-types.ts";

export type SessionTokenStatusBarProps = {
  items: TranscriptTokenItem[];
  skillLinks: TranscriptSkillLink[];
  reportedSegments: TokenSegmentProps[] | null;
  metrics: TokenMetricProps[];
  usageSource: TokenUsageSource;
};

type EstimateState = {
  status: TokenEstimateStatus;
  segments: TokenSegmentProps[] | null;
};

const emptySegments: TokenSegmentProps[] = [];

export function SessionTokenStatusBar({ items, skillLinks, reportedSegments, metrics, usageSource }: SessionTokenStatusBarProps) {
  const [estimate, setEstimate] = useState<EstimateState>({ status: TokenEstimateStatus.Loading, segments: null });
  const workerRef = useRef<TokenizerWorkerClient | null>(null);
  const latestRequestRef = useRef(0);

  useEffect(() => {
    if (reportedSegments) {
      workerRef.current?.dispose();
      workerRef.current = null;
      setEstimate({ status: TokenEstimateStatus.Ready, segments: null });
      return;
    }

    setEstimate((current) => ({ ...current, status: TokenEstimateStatus.Loading }));
    const worker = createTokenizerWorker(
      (response) => {
        if (response.id !== latestRequestRef.current) return;
        if (response.type === TokenizerWorkerResponseType.Result) setEstimate({ status: TokenEstimateStatus.Ready, segments: response.segments });
        else setEstimate((current) => ({ ...current, status: TokenEstimateStatus.Error }));
      },
      () => setEstimate((current) => ({ ...current, status: TokenEstimateStatus.Error })),
    );
    workerRef.current = worker;

    return () => {
      if (workerRef.current === worker) workerRef.current = null;
      worker.dispose();
    };
  }, [reportedSegments]);

  useEffect(() => {
    if (reportedSegments || !workerRef.current) return;
    setEstimate((current) => ({ ...current, status: TokenEstimateStatus.Loading }));
    latestRequestRef.current = workerRef.current.request({
      kind: TokenizerKind.Transcript,
      items: compactTranscriptTokenItems(items),
      skillLinks: compactTranscriptSkillLinks(skillLinks),
    });
  }, [items, reportedSegments, skillLinks]);

  const segments = reportedSegments ?? estimate.segments ?? emptySegments;
  return <TokenStatusBar segments={segments} metrics={metrics} usageSource={usageSource} />;
}
