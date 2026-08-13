import { ChevronRight, RefreshCw } from "lucide-react";

export type DialogAdvanceButtonProps = {
  label: string;
  busyLabel?: string;
  busy?: boolean;
  disabled?: boolean;
  onClick: () => void;
  ariaLabel?: string;
};

export function DialogAdvanceButton({
  label,
  busyLabel = "Working",
  busy,
  disabled,
  onClick,
  ariaLabel,
}: DialogAdvanceButtonProps) {
  const currentLabel = busy ? busyLabel : label;
  return (
    <button
      className={`primary dialogAdvanceButton ${busy ? "isBusy" : ""}`}
      aria-label={ariaLabel ?? currentLabel}
      aria-busy={busy}
      onClick={onClick}
      disabled={disabled || busy}
    >
      <span>{currentLabel}</span>
      {busy ? <RefreshCw className="dialogLoadingIcon" size={16} /> : <ChevronRight size={16} />}
    </button>
  );
}
