import "./LoadingInline.css";

import type { CSSProperties } from "react";

import { LoadingIcon } from "./LoadingIcon.tsx";

export type LoadingInlineProps = {
  label: string;
  size?: number;
  gap?: CSSProperties["gap"];
};

export function LoadingInline({ label, size = 15, gap }: LoadingInlineProps) {
  return (
    <span className="loadingInline" style={gap === undefined ? undefined : { gap }}>
      <LoadingIcon size={size} />
      <span>{label}</span>
    </span>
  );
}
