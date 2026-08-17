import type { ComponentPropsWithoutRef, ReactNode } from "react";
import { Dialog } from "radix-ui";

import "./confirm-dialog.css";

type DialogShellContentProps = Omit<ComponentPropsWithoutRef<typeof Dialog.Content>, "children" | "className" | "aria-describedby" | "data-no-drag" | "onMouseDown">
  & Record<`data-${string}`, string | number | boolean | undefined>;

export type DialogShellProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  children: ReactNode;
  trigger?: ReactNode;
  className?: string;
  descriptionId?: string;
  contentProps?: DialogShellContentProps;
};

export function DialogShell({
  open,
  onOpenChange,
  children,
  trigger,
  className = "confirmDialogPanel",
  descriptionId,
  contentProps,
}: DialogShellProps) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      {trigger}
      <Dialog.Portal>
        <Dialog.Overlay className="dialogOverlay" />
        <Dialog.Content
          {...contentProps}
          className={["dialogShell", className].filter(Boolean).join(" ")}
          aria-describedby={descriptionId}
          data-no-drag
          onMouseDown={(event) => event.stopPropagation()}
        >
          {children}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
