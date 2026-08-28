import type { ReactElement, ReactNode } from "react";
import { DropdownMenu } from "radix-ui";

import { useRowMenuOpenChange } from "./row-menu-context.tsx";

export type InfoDropdownMenuProps = {
  trigger: ReactElement;
  label: ReactNode;
  title: ReactNode;
  children: ReactNode;
  contentClassName?: string;
  sideOffset?: number;
};

export function InfoDropdownMenu({
  trigger,
  label,
  title,
  children,
  contentClassName,
  sideOffset = 8,
}: InfoDropdownMenuProps) {
  const notifyRowMenuOpenChange = useRowMenuOpenChange();
  const className = ["skillInfoContent", contentClassName].filter(Boolean).join(" ");
  return (
    <DropdownMenu.Root onOpenChange={(open) => notifyRowMenuOpenChange?.(open)}>
      <DropdownMenu.Trigger asChild>{trigger}</DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content className={className} align="end" sideOffset={sideOffset} data-no-drag onMouseDown={(event) => event.stopPropagation()}>
          <div className="skillInfoHeader">
            <span>{label}</span>
            <strong>{title}</strong>
          </div>
          <div className="skillInfoSections">{children}</div>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
