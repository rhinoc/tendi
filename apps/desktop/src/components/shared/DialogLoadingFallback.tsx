import { Dialog } from "radix-ui";

import { LoadingState } from "./LoadingState.tsx";
import { DialogShell } from "./DialogShell.tsx";

export type DialogLoadingFallbackProps = {
  title: string;
  label: string;
  descriptionId: string;
  onOpenChange: (open: boolean) => void;
  className?: string;
};

export function DialogLoadingFallback({
  title,
  label,
  descriptionId,
  onOpenChange,
  className,
}: DialogLoadingFallbackProps) {
  return (
    <DialogShell
      open
      onOpenChange={onOpenChange}
      className={className}
      descriptionId={descriptionId}
    >
      <Dialog.Title className="confirmDialogTitle">{title}</Dialog.Title>
      <Dialog.Description id={descriptionId} className="dialogVisuallyHidden">
        {label}
      </Dialog.Description>
      <LoadingState className="loadingStateCompact" label={label} />
    </DialogShell>
  );
}
