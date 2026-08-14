import { Tooltip } from "./Tooltip.tsx";
import { CopyFeedbackIcon, useCopyFeedback } from "./useCopyFeedback.tsx";

import { copyText } from "../../lib/index.ts";

export type CopyableSessionIdProps = {
  sessionId: string;
  className?: string;
};

export function CopyableSessionId({ sessionId, className = "" }: CopyableSessionIdProps) {
  const { copied, markCopied } = useCopyFeedback();

  if (!sessionId) return <code>-</code>;

  const copySessionId = async (event: React.MouseEvent<HTMLButtonElement>) => {
    event.preventDefault();
    event.stopPropagation();
    await copyText(sessionId);
    markCopied();
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
        <CopyFeedbackIcon copied={copied} />
      </button>
    </span>
  );
}
