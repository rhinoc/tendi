import "./LoadingInline.css";

import { LoadingIcon } from "./LoadingIcon.tsx";

export type LoadingInlineProps = {
  label: string;
};

export function LoadingInline({ label }: LoadingInlineProps) {
  return (
    <span className="loadingInline">
      <LoadingIcon size={15} />
      <span>{label}</span>
    </span>
  );
}
