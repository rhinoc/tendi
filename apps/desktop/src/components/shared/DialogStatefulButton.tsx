import { AlertCircle, Check } from "lucide-react";
import type { ReactNode } from "react";

import {
  DialogActionButton,
  type DialogActionButtonProps,
} from "./DialogActionButton.tsx";
import { LoadingIcon } from "./LoadingIcon.tsx";
import { AsyncStatus } from "../../lib/async-status.ts";
import "./StatefulButton.css";

export type DialogStatefulButtonState = AsyncStatus;

export type DialogStatefulButtonProps = Omit<
  DialogActionButtonProps,
  "aria-busy" | "aria-label" | "children" | "disabled"
> & {
  state: DialogStatefulButtonState;
  children: ReactNode;
  loadingLabel?: string;
  successLabel?: string;
  errorLabel?: string;
  loadingContent?: ReactNode;
  successContent?: ReactNode;
  errorContent?: ReactNode;
  disabled?: boolean;
  "aria-label"?: string;
};

export function DialogStatefulButton({
  state,
  children,
  loadingLabel,
  successLabel,
  errorLabel,
  loadingContent,
  successContent,
  errorContent,
  disabled = false,
  className = "",
  "aria-label": ariaLabel,
  ...buttonProps
}: DialogStatefulButtonProps) {
  const stateLabel = state === AsyncStatus.Loading
    ? loadingLabel ?? ariaLabel ?? "Working"
    : state === AsyncStatus.Success
      ? successLabel ?? ariaLabel ?? "Completed"
      : state === AsyncStatus.Error
        ? errorLabel ?? ariaLabel ?? "Failed"
        : ariaLabel;

  const content = state === AsyncStatus.Loading
    ? loadingContent ?? <LoadingIcon size={14} />
    : state === AsyncStatus.Success
      ? successContent ?? (successLabel ? <><Check size={14} aria-hidden="true" /><span>{successLabel}</span></> : <Check size={14} aria-hidden="true" />)
      : state === AsyncStatus.Error
        ? errorContent ?? (errorLabel ? <><AlertCircle size={14} aria-hidden="true" /><span>{errorLabel}</span></> : <AlertCircle size={14} aria-hidden="true" />)
        : children;

  return (
    <DialogActionButton
      {...buttonProps}
      className={["statefulButton", "dialogStatefulButton", state === AsyncStatus.Loading ? "isBusy" : "", className].filter(Boolean).join(" ")}
      disabled={disabled || state === AsyncStatus.Loading}
      aria-label={stateLabel}
      aria-busy={state === AsyncStatus.Loading}
      data-state={state}
    >
      {content}
    </DialogActionButton>
  );
}
