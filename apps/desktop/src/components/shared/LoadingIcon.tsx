import { RefreshCw } from "lucide-react";

import "./LoadingIcon.css";

export type LoadingIconProps = {
  size?: number;
  className?: string;
};

export function LoadingIcon({ size = 15, className = "" }: LoadingIconProps) {
  return (
    <RefreshCw
      className={`loadingIcon ${className}`.trim()}
      size={size}
      aria-hidden="true"
    />
  );
}
