import type { CSSProperties } from "react";

import "./LoadingDots.css";

export type LoadingDotsProps = {
  size?: number;
  className?: string;
};

export function LoadingDots({ size = 15, className = "" }: LoadingDotsProps) {
  return (
    <span
      className={["loadingDots", className].filter(Boolean).join(" ")}
      style={{ "--loading-dots-size": `${size}px` } as CSSProperties}
      aria-hidden="true"
    />
  );
}
