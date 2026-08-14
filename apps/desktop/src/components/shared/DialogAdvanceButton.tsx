import { ChevronRight } from "lucide-react";

import { DialogActionButton } from "./DialogActionButton.tsx";
import { LoadingIcon } from "./LoadingIcon.tsx";

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
    <DialogActionButton
      variant="primary"
      className={`dialogAdvanceButton ${busy ? "isBusy" : ""}`}
      aria-label={ariaLabel ?? currentLabel}
      aria-busy={busy}
      onClick={onClick}
      disabled={disabled || busy}
    >
      <span>{currentLabel}</span>
      {busy ? <LoadingIcon size={16} /> : <ChevronRight size={16} />}
    </DialogActionButton>
  );
}
