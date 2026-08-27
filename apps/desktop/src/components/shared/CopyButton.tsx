import { useEffect, useState, type ButtonHTMLAttributes, type MouseEvent, type ReactNode } from "react";
import { AlertCircle } from "lucide-react";

import { actionLabels, copyText } from "../../lib/index.ts";
import { CopyFeedbackIcon, useCopyFeedback } from "./useCopyFeedback.tsx";

export type CopyButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "onClick" | "children" | "title"> & {
  value?: string | null;
  onCopy?: () => Promise<unknown> | unknown;
  copyLabel: string;
  copiedLabel?: string;
  iconSize?: number;
  stopPropagation?: boolean;
  children?: ReactNode;
  copiedChildren?: ReactNode;
};

export function CopyButton({
  value,
  onCopy,
  copyLabel,
  copiedLabel = actionLabels.copied,
  iconSize = 13,
  stopPropagation = false,
  className = "",
  disabled,
  children,
  copiedChildren,
  ...props
}: CopyButtonProps) {
  const { copied, markCopied, resetCopied } = useCopyFeedback();
  const [copyError, setCopyError] = useState(false);

  useEffect(() => {
    if (!copyError) return undefined;
    const timer = window.setTimeout(() => setCopyError(false), 1600);
    return () => window.clearTimeout(timer);
  }, [copyError]);

  const handleClick = async (event: MouseEvent<HTMLButtonElement>) => {
    if (stopPropagation) event.stopPropagation();
    if (disabled) return;
    setCopyError(false);
    try {
      if (onCopy) {
        const result = await onCopy();
        if (result === false) return;
      } else {
        await copyText(value);
      }
      markCopied();
    } catch {
      resetCopied();
      setCopyError(true);
    }
  };

  const label = copyError ? actionLabels.copyFailed : copied ? copiedLabel : copyLabel;
  const resolvedClassName = `${className}${copied ? " isCopied" : ""}${copyError ? " isCopyError" : ""}`.trim();
  const hasCustomChildren = children != null || copiedChildren != null;

  return (
    <button
      type="button"
      {...props}
      aria-label={label}
      className={resolvedClassName}
      disabled={disabled}
      onClick={handleClick}
    >
      {copyError ? (
        <>
          <AlertCircle size={iconSize} strokeWidth={2.2} />
          {hasCustomChildren ? children : null}
        </>
      ) : !hasCustomChildren ? (
        <CopyFeedbackIcon copied={copied} size={iconSize} />
      ) : copied ? (
        <>
          <CopyFeedbackIcon copied size={iconSize} swap={false} />
          {copiedChildren ?? children}
        </>
      ) : (
        <>
          <CopyFeedbackIcon copied={false} size={iconSize} swap={false} />
          {children}
        </>
      )}
    </button>
  );
}
