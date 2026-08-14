import type { ButtonHTMLAttributes } from "react";
import { MoreHorizontal } from "lucide-react";

export type MoreActionsButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children">;

export function MoreActionsButton({ className = "", ...props }: MoreActionsButtonProps) {
  return (
    <button type="button" {...props} className={`iconButton${className ? ` ${className}` : ""}`}>
      <MoreHorizontal size={16} />
    </button>
  );
}
