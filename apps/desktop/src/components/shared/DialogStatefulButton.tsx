import { AlertCircle, Check } from "lucide-react";
import type { ReactNode } from "react";

import {
  DialogActionButton,
  type DialogActionButtonProps,
} from "./DialogActionButton.tsx";
import { LoadingIcon } from "./LoadingIcon.tsx";
import "./StatefulButton.css";

export type DialogStatefulButtonState = "idle" | "loading" | "success" | "error";

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
  const stateLabel = state === "loading"
    ? loadingLabel ?? ariaLabel ?? "Working"
    : state === "success"
      ? successLabel ?? ariaLabel ?? "Completed"
      : state === "error"
        ? errorLabel ?? ariaLabel ?? "Failed"
        : ariaLabel;

  const content = state === "loading"
    ? loadingContent ?? <LoadingIcon size={14} />
    : state === "success"
      ? successContent ?? (successLabel ? <><Check size={14} aria-hidden="true" /><span>{successLabel}</span></> : <Check size={14} aria-hidden="true" />)
      : state === "error"
        ? errorContent ?? (errorLabel ? <><AlertCircle size={14} aria-hidden="true" /><span>{errorLabel}</span></> : <AlertCircle size={14} aria-hidden="true" />)
        : children;

  return (
    <DialogActionButton
      {...buttonProps}
      className={["statefulButton", "dialogStatefulButton", state === "loading" ? "isBusy" : "", className].filter(Boolean).join(" ")}
      disabled={disabled || state === "loading"}
      aria-label={stateLabel}
      aria-busy={state === "loading"}
      data-state={state}
    >
      {content}
    </DialogActionButton>
  );
}
