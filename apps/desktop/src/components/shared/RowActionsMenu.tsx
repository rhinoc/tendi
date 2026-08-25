import type { ReactNode } from "react";
import { DropdownMenu } from "radix-ui";

import { MoreActionsButton } from "./MoreActionsButton.tsx";

export type RowActionsMenuProps = {
  ariaLabel: string;
  children: ReactNode;
  onOpenChange?: (open: boolean) => void;
};

export function RowActionsMenu({ ariaLabel, children, onOpenChange }: RowActionsMenuProps) {
  return (
    <DropdownMenu.Root onOpenChange={onOpenChange}>
      <DropdownMenu.Trigger asChild>
        <MoreActionsButton aria-label={ariaLabel} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content className="skillMenuContent" align="end" sideOffset={6} data-no-drag>
          {children}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
