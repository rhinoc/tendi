import { AlertCircle, Check } from "lucide-react";
import { forwardRef, type CSSProperties, type ReactNode } from "react";

import { Button, type ButtonProps } from "./Button.tsx";
import { LoadingIcon } from "./LoadingIcon.tsx";
import { AsyncStatus } from "../../lib/async-status.ts";
import "./StatefulButton.css";

export type StatefulButtonState = AsyncStatus;

export type StatefulButtonProps = Omit<ButtonProps, "children" | "disabled" | "aria-busy"> & {
  state: StatefulButtonState;
  children: ReactNode;
  loadingLabel?: string;
  successLabel?: string;
  errorLabel?: string;
  loadingContent?: ReactNode;
  successContent?: ReactNode;
  errorContent?: ReactNode;
  disabled?: boolean;
  width?: CSSProperties["width"];
  minWidth?: CSSProperties["minWidth"];
};

export const StatefulButton = forwardRef<HTMLButtonElement, StatefulButtonProps>(function StatefulButton({
  state,
  children,
  loadingLabel,
  successLabel,
  errorLabel,
  loadingContent,
  successContent,
  errorContent,
  disabled = false,
  width,
  minWidth,
  style,
  className = "",
  "aria-label": ariaLabel,
  ...buttonProps
}, ref) {
  const stateLabel = state === AsyncStatus.Loading
    ? loadingLabel ?? ariaLabel ?? "Working"
    : state === AsyncStatus.Success
      ? successLabel ?? ariaLabel ?? "Completed"
      : state === AsyncStatus.Error
        ? errorLabel ?? ariaLabel ?? "Failed"
        : ariaLabel;

  const content = state === AsyncStatus.Loading
    ? loadingContent ?? <LoadingIcon size={16} />
    : state === AsyncStatus.Success
      ? successContent ?? (successLabel ? <><Check size={16} aria-hidden="true" /><span>{successLabel}</span></> : <Check size={16} aria-hidden="true" />)
      : state === AsyncStatus.Error
        ? errorContent ?? (errorLabel ? <><AlertCircle size={16} aria-hidden="true" /><span>{errorLabel}</span></> : <AlertCircle size={16} aria-hidden="true" />)
        : children;

  const resolvedStyle: CSSProperties = {
    ...style,
    ...(width === undefined ? {} : { width }),
    ...(minWidth === undefined ? {} : { minWidth }),
  };

  return (
    <Button
      ref={ref}
      {...buttonProps}
      className={["statefulButton", className].filter(Boolean).join(" ")}
      style={resolvedStyle}
      disabled={disabled || state === AsyncStatus.Loading}
      aria-label={stateLabel}
      aria-busy={state === AsyncStatus.Loading}
      data-state={state}
    >
      {content}
    </Button>
  );
});
