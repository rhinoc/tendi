import type { ReactNode } from "react";

import { ExpandingArrowButton, type ExpandingArrowButtonProps } from "./ExpandingArrowButton.tsx";

export type DialogApplyButtonProps = Omit<
  ExpandingArrowButtonProps,
  "children" | "disabled" | "expanded" | "aria-busy" | "aria-label"
> & {
  label: ReactNode;
  busy?: boolean;
  busyLabel?: string;
  expandOnFocus?: boolean;
  disabled?: boolean;
  ariaLabel?: string;
};

export function DialogApplyButton({
  label,
  busy = false,
  busyLabel,
  expandOnFocus,
  disabled = false,
  ariaLabel,
  className = "",
  ...buttonProps
}: DialogApplyButtonProps) {
  const labelText = typeof label === "string" ? label : "Apply";
  const accessibleLabel = busy
    ? busyLabel ?? ariaLabel ?? labelText
    : ariaLabel ?? labelText;

  return (
    <ExpandingArrowButton
      {...buttonProps}
      compact
      expandOnFocus={expandOnFocus}
      className={["dialogApplyButton", busy ? "isBusy" : "", className].filter(Boolean).join(" ")}
      aria-label={accessibleLabel}
      aria-busy={busy}
      data-state={busy ? "loading" : "idle"}
      expanded={busy}
      disabled={disabled || busy}
    >
      {label}
    </ExpandingArrowButton>
  );
}
