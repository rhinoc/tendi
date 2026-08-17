import { forwardRef } from "react";
import { MoreHorizontal } from "lucide-react";

import { IconButton, type IconButtonProps } from "./IconButton.tsx";

export type MoreActionsButtonProps = Omit<IconButtonProps, "children">;

export const MoreActionsButton = forwardRef<HTMLButtonElement, MoreActionsButtonProps>(function MoreActionsButton(
  { className = "", ...props },
  ref,
) {
  return (
    <IconButton ref={ref} {...props} className={className}>
      <MoreHorizontal size={16} />
    </IconButton>
  );
});
