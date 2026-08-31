import { useRef, type ReactNode } from "react";
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
  const openedByPointerRef = useRef(false);
  const notifyRowMenuOpenChange = useRowMenuOpenChange();
  return (
    <DropdownMenu.Root onOpenChange={(open) => {
      onOpenChange?.(open);
      notifyRowMenuOpenChange?.(open);
    }}>
      <DropdownMenu.Trigger asChild>
        <MoreActionsButton
          aria-label={ariaLabel}
          onPointerDownCapture={() => { openedByPointerRef.current = true; }}
          onKeyDownCapture={() => { openedByPointerRef.current = false; }}
        />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <MenuContent
          align="end"
          sideOffset={6}
          data-no-drag
          onKeyDownCapture={() => { openedByPointerRef.current = false; }}
          onCloseAutoFocus={(event) => {
            if (openedByPointerRef.current) event.preventDefault();
            openedByPointerRef.current = false;
          }}
        >
          {children}
        </MenuContent>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
