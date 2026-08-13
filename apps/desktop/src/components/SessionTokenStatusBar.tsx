import { useMemo } from "react";

import { TokenStatusBar, type TokenMetricProps, type TokenSegmentProps } from "./TokenStatusBar.tsx";
import { transcriptTokenSegments, type TranscriptSkillLink, type TranscriptTokenItem } from "../lib/tokenizer.ts";

export type SessionTokenStatusBarProps = {
  items: TranscriptTokenItem[];
  skillLinks: TranscriptSkillLink[];
  reportedSegments: TokenSegmentProps[] | null;
  metrics: TokenMetricProps[];
  usageSource: "estimated" | "reported";
};

export function SessionTokenStatusBar({ items, skillLinks, reportedSegments, metrics, usageSource }: SessionTokenStatusBarProps) {
  const segments = useMemo(
    () => reportedSegments ?? transcriptTokenSegments(items, skillLinks),
    [items, reportedSegments, skillLinks],
  );
  return <TokenStatusBar segments={segments} metrics={metrics} usageSource={usageSource} />;
}
