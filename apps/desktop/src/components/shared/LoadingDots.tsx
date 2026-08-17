import type { CSSProperties } from "react";

import "./LoadingDots.css";

export type LoadingDotsProps = {
  size?: number;
  variant?: "icon" | "surface";
  className?: string;
};

export function LoadingDots({ size = 15, variant = "icon", className = "" }: LoadingDotsProps) {
  const classes = ["loadingDots", variant === "surface" ? "loadingDotsSurface" : "", className]
    .filter(Boolean)
    .join(" ");
  return (
    <span
      className={classes}
      style={{ "--loading-dots-size": `${size}px` } as CSSProperties}
      aria-hidden="true"
    />
  );
}
