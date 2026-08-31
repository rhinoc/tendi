import type { ReactNode } from "react";
import { Dialog } from "radix-ui";

import { LoadingState } from "./LoadingState.tsx";
import { DialogShell } from "./DialogShell.tsx";

export type DialogLoadingFallbackProps = {
  title: string;
  label: string;
  descriptionId: string;
  onOpenChange: (open: boolean) => void;
  className?: string;
  description?: ReactNode;
  actions?: ReactNode;
  showLoading?: boolean;
};

export function DialogLoadingFallback({
  title,
  label,
  descriptionId,
  onOpenChange,
  className,
  description,
  actions,
  showLoading = true,
}: DialogLoadingFallbackProps) {
  return (
    <DialogShell
      open
      onOpenChange={onOpenChange}
      className={className}
      descriptionId={descriptionId}
    >
      <Dialog.Title className="confirmDialogTitle">{title}</Dialog.Title>
      <Dialog.Description id={descriptionId} className={description !== undefined ? "confirmDialogDescription" : "dialogVisuallyHidden"}>
        {description ?? label}
      </Dialog.Description>
      {showLoading ? <LoadingState className="loadingStateCompact" label={label} /> : null}
      {actions ? <div className="confirmDialogActions">{actions}</div> : null}
    </DialogShell>
  );
}
