import type { ReactNode } from "react";

import "./editor-state-placeholder.css";

import { LoadingState } from "./LoadingState.tsx";

export type EditorStatePlaceholderProps = {
  className?: string;
  label?: string;
  children?: ReactNode;
};

export function EditorStatePlaceholder({ className, label, children }: EditorStatePlaceholderProps) {
  return (
    <div className={`editorStatePlaceholder${className ? ` ${className}` : ""}`}>
      {label ? <LoadingState label={label} /> : children}
    </div>
  );
}
