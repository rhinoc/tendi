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
  return (
    <DialogActionButton
      variant="primary"
      className={`dialogAdvanceButton ${busy ? "isBusy" : ""}`}
      aria-label={ariaLabel ?? (busy ? busyLabel : label)}
      aria-busy={busy}
      onClick={onClick}
      disabled={disabled || busy}
    >
      {busy ? <LoadingIcon size={16} /> : <span>{label}</span>}
    </DialogActionButton>
  );
}
