import "./LoadingState.css";

import { LoadingDots } from "./LoadingDots.tsx";

export type LoadingStateProps = {
  label: string;
  className?: string;
  variant?: "dots" | "progress";
};

export function LoadingState({ label, className = "", variant = "dots" }: LoadingStateProps) {
  return (
    <div className={["loadingState", variant === "progress" ? "loadingStateProgress" : "", className].filter(Boolean).join(" ")} role="status" aria-label={label}>
      {variant === "progress" ? (
        <div className="loadingStateProgressContent">
          <span className="loadingStateProgressLabel">{label}</span>
          <div className="loadingStateProgressTrack" role="progressbar" aria-label={label} aria-valuetext="In progress">
            <span className="loadingStateProgressIndicator" />
          </div>
        </div>
      ) : <LoadingDots className="loadingStateDots" />}
    </div>
  );
}
