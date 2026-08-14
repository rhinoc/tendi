import { useEffect, useState } from "react";
import { Check, Copy } from "lucide-react";

export function useCopyFeedback(timeout = 1400) {
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) return undefined;
    const timer = window.setTimeout(() => setCopied(false), timeout);
    return () => window.clearTimeout(timer);
  }, [copied, timeout]);

  return { copied, markCopied: () => setCopied(true), resetCopied: () => setCopied(false) };
}

export type CopyFeedbackIconProps = {
  copied: boolean;
  size?: number;
  copiedStrokeWidth?: number;
  idleStrokeWidth?: number;
  swap?: boolean;
};

export function CopyFeedbackIcon({ copied, size = 13, copiedStrokeWidth = 2.6, idleStrokeWidth = 2, swap = true }: CopyFeedbackIconProps) {
  if (!swap) return copied ? <Check size={size} strokeWidth={copiedStrokeWidth} /> : <Copy size={size} strokeWidth={idleStrokeWidth} />;

  return (
    <span className={`copyIconSwap${copied ? " isCopied" : ""}`} aria-hidden="true">
      <span className="copyIconSwapLayer copyIconSwapActive">
        <Check size={size} strokeWidth={copiedStrokeWidth} />
      </span>
      <span className="copyIconSwapLayer copyIconSwapIdle">
        <Copy size={size} strokeWidth={idleStrokeWidth} />
      </span>
    </span>
  );
}
