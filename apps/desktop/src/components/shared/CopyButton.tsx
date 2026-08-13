import { useEffect, useState, type ButtonHTMLAttributes, type MouseEvent, type ReactNode } from "react";
import { AlertCircle, Check, Copy } from "lucide-react";

import { copyText } from "../../lib/index.ts";

export type CopyButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "onClick" | "children"> & {
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
  copiedLabel = "Copied",
  iconSize = 13,
  stopPropagation = false,
  className = "",
  title,
  disabled,
  children,
  copiedChildren,
  ...props
}: CopyButtonProps) {
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState(false);

  useEffect(() => {
    if (!copied) return undefined;
    const timer = window.setTimeout(() => setCopied(false), 1400);
    return () => window.clearTimeout(timer);
  }, [copied]);

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
      setCopied(true);
    } catch {
      setCopied(false);
      setCopyError(true);
    }
  };

  const label = copyError ? "Copy failed" : copied ? copiedLabel : copyLabel;
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
        <span className={`copyIconSwap${copied ? " isCopied" : ""}`} aria-hidden="true">
          <span className="copyIconSwapLayer copyIconSwapActive">
            <Check size={iconSize} strokeWidth={2.6} />
          </span>
          <span className="copyIconSwapLayer copyIconSwapIdle">
            <Copy size={iconSize} strokeWidth={2} />
          </span>
        </span>
      ) : copied ? (
        <>
          <Check size={iconSize} strokeWidth={2.6} />
          {copiedChildren ?? children}
        </>
      ) : (
        <>
          <Copy size={iconSize} strokeWidth={2} />
          {children}
        </>
      )}
    </button>
  );
}
