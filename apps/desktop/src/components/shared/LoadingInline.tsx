import { RefreshCw } from "lucide-react";

import "./LoadingInline.css";

export type LoadingInlineProps = {
  label: string;
};

export function LoadingInline({ label }: LoadingInlineProps) {
  return (
    <span className="loadingInline">
      <RefreshCw className="loadingSpinner" size={15} aria-hidden="true" />
      <span>{label}</span>
    </span>
  );
}
