import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";

import "./Toast.css";

export type ToastTone = "error" | "success" | "info";

export type ToastProps = {
  message: string;
  tone?: ToastTone;
  action?: {
    label: string;
    onClick: () => void;
    disabled?: boolean;
  };
  onDismiss?: () => void;
};

const toneIcons = {
  error: AlertCircle,
  success: CheckCircle2,
  info: Info,
} as const;

export function Toast({ message, tone = "info", action, onDismiss }: ToastProps) {
  const Icon = toneIcons[tone];

  return (
    <div className={`appToast appToast--${tone}`} role={tone === "error" ? "alert" : "status"} aria-live={tone === "error" ? "assertive" : "polite"}>
      <Icon className="appToastIcon" size={16} aria-hidden="true" />
      <span className="appToastMessage">{message}</span>
      {action ? (
        <button type="button" className="appToastAction" onClick={action.onClick} disabled={action.disabled}>
          {action.label}
        </button>
      ) : null}
      {onDismiss ? (
        <button type="button" className="appToastDismiss" aria-label="Dismiss notification" onClick={onDismiss}>
          <X size={14} aria-hidden="true" />
        </button>
      ) : null}
    </div>
  );
}
