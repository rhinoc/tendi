import type { ReactNode } from "react";
import { DropdownMenu } from "radix-ui";

import { MenuContent } from "./MenuContent.tsx";
import { MoreActionsButton } from "./MoreActionsButton.tsx";
import { useRowMenuOpenChange } from "./row-menu-context.tsx";

export type RowActionsMenuProps = {
  ariaLabel: string;
  children: ReactNode;
  onOpenChange?: (open: boolean) => void;
};

export function RowActionsMenu({ ariaLabel, children, onOpenChange }: RowActionsMenuProps) {
  const notifyRowMenuOpenChange = useRowMenuOpenChange();
  return (
    <DropdownMenu.Root onOpenChange={(open) => {
      onOpenChange?.(open);
      notifyRowMenuOpenChange?.(open);
    }}>
      <DropdownMenu.Trigger asChild>
        <MoreActionsButton aria-label={ariaLabel} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <MenuContent align="end" sideOffset={6} data-no-drag>
          {children}
        </MenuContent>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
