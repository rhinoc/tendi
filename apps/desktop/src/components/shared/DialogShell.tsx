import type { ComponentPropsWithoutRef, ReactNode } from "react";
import { X } from "lucide-react";
import { Dialog } from "radix-ui";

import { IconButton } from "./IconButton.tsx";
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
  dismissOnOutsideClick?: boolean;
  showCloseButton?: boolean;
  closeButtonLabel?: string;
  contentProps?: DialogShellContentProps;
};

export function DialogShell({
  open,
  onOpenChange,
  children,
  trigger,
  className = "confirmDialogPanel",
  descriptionId,
  dismissOnOutsideClick = false,
  showCloseButton = false,
  closeButtonLabel = "Close dialog",
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
          onPointerDownOutside={(event) => {
            contentProps?.onPointerDownOutside?.(event);
            if (!dismissOnOutsideClick) event.preventDefault();
          }}
          onInteractOutside={(event) => {
            contentProps?.onInteractOutside?.(event);
            if (!dismissOnOutsideClick) event.preventDefault();
          }}
          onMouseDown={(event) => event.stopPropagation()}
        >
          {children}
          {showCloseButton ? (
            <Dialog.Close asChild>
              <IconButton className="dialogCloseButton" aria-label={closeButtonLabel}>
                <X size={15} aria-hidden="true" />
              </IconButton>
            </Dialog.Close>
          ) : null}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
