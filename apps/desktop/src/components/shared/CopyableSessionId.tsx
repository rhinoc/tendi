import { Tooltip } from "./Tooltip.tsx";
import { useEffect, useState } from "react";
import { Check, Copy } from "lucide-react";

import { copyText } from "../../lib/index.ts";

export type CopyableSessionIdProps = {
  sessionId: string;
  className?: string;
};

export function CopyableSessionId({ sessionId, className = "" }: CopyableSessionIdProps) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return undefined;
    const timer = window.setTimeout(() => setCopied(false), 1400);
    return () => window.clearTimeout(timer);
  }, [copied]);

  if (!sessionId) return <code>-</code>;

  const copySessionId = async (event: React.MouseEvent<HTMLButtonElement>) => {
    event.preventDefault();
    event.stopPropagation();
    await copyText(sessionId);
    setCopied(true);
  };

  return (
    <span className={`copyableSessionId ${copied ? "isCopied " : ""}${className}`}>
      <Tooltip content={sessionId} onlyWhenTruncated><code>{sessionId}</code></Tooltip>
      <button
        type="button"
        aria-label={copied ? "Session ID copied" : "Copy session ID"}
        className="copyableSessionIdButton"
        onClick={copySessionId}
      >
        <span className={`copyIconSwap${copied ? " isCopied" : ""}`} aria-hidden="true">
          <span className="copyIconSwapLayer copyIconSwapActive">
            <Check size={13} strokeWidth={2.6} />
          </span>
          <span className="copyIconSwapLayer copyIconSwapIdle">
            <Copy size={13} strokeWidth={2} />
          </span>
        </span>
      </button>
    </span>
  );
}
