import "./LoadingState.css";

import { LoadingDots } from "./LoadingDots.tsx";

export type LoadingStateProps = {
  label: string;
  className?: string;
};

export function LoadingState({ label, className = "" }: LoadingStateProps) {
  return (
    <div className={["loadingState", className].filter(Boolean).join(" ")} role="status" aria-label={label}>
      <LoadingDots variant="surface" className="loadingStateDots" />
    </div>
  );
}
