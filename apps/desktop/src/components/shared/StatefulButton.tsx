import { AlertCircle, Check } from "lucide-react";
import { forwardRef, type CSSProperties, type ReactNode } from "react";

import { Button, type ButtonProps } from "./Button.tsx";
import { LoadingIcon } from "./LoadingIcon.tsx";
import "./StatefulButton.css";

export type StatefulButtonState = "idle" | "loading" | "success" | "error";

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
  const stateLabel = state === "loading"
    ? loadingLabel ?? ariaLabel ?? "Working"
    : state === "success"
      ? successLabel ?? ariaLabel ?? "Completed"
      : state === "error"
        ? errorLabel ?? ariaLabel ?? "Failed"
        : ariaLabel;

  const content = state === "loading"
    ? loadingContent ?? <LoadingIcon size={16} />
    : state === "success"
      ? successContent ?? (successLabel ? <><Check size={16} aria-hidden="true" /><span>{successLabel}</span></> : <Check size={16} aria-hidden="true" />)
      : state === "error"
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
      disabled={disabled || state === "loading"}
      aria-label={stateLabel}
      aria-busy={state === "loading"}
      data-state={state}
    >
      {content}
    </Button>
  );
});
