import type { ReactNode } from "react";

import "./EmptyState.css";

export type EmptyStateProps = {
  icon?: ReactNode;
  iconTone?: "accent" | "muted";
  title: string;
  description?: string;
  action?: ReactNode;
  compact?: boolean;
  className?: string;
  role?: "alert" | "status";
};

export function EmptyState({
  icon,
  iconTone = "accent",
  title,
  description,
  action,
  compact = false,
  className = "",
  role,
}: EmptyStateProps) {
  return (
    <div
      className={["emptyState", compact ? "emptyState--compact" : "", className].filter(Boolean).join(" ")}
      role={role}
    >
      {icon ? <div className={`emptyStateIcon emptyStateIcon--${iconTone}`} aria-hidden="true">{icon}</div> : null}
      <div className="emptyStateCopy">
        <strong>{title}</strong>
        {description ? <span>{description}</span> : null}
      </div>
      {action ? <div className="emptyStateAction">{action}</div> : null}
    </div>
  );
}
