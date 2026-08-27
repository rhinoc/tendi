import { Dialog } from "radix-ui";

import { DialogActionButton } from "./DialogActionButton.tsx";
import { DialogShell } from "./DialogShell.tsx";
import { DialogStatefulButton } from "./DialogStatefulButton.tsx";

export function DeleteConfirmationDialog({
  open,
  items,
  itemLabel,
  description,
  confirmLabel,
  busy = false,
  onOpenChange,
  onConfirm,
}: {
  open: boolean;
  items: readonly string[];
  itemLabel: string;
  description?: string;
  confirmLabel?: string;
  busy?: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  const plural = items.length === 1 ? "" : "s";
  const actionLabel = confirmLabel ?? `Delete ${itemLabel}${plural}`;
  return (
    <DialogShell
      open={open}
      onOpenChange={(nextOpen) => {
        if (!busy) onOpenChange(nextOpen);
      }}
      descriptionId="delete-confirmation-description"
    >
      <Dialog.Title className="confirmDialogTitle">Delete {itemLabel}{plural}?</Dialog.Title>
      <p id="delete-confirmation-description" className="confirmDialogDescription">
        {description ?? "This action cannot be undone."}
      </p>
      <div className="deleteConfirmationNames" data-selectable-text>
        {items.map((item, index) => <span key={`${item}-${index}`}>{item}</span>)}
      </div>
      <div className="confirmDialogActions">
        <DialogActionButton variant="secondary" disabled={busy} onClick={() => onOpenChange(false)}>Cancel</DialogActionButton>
        <DialogStatefulButton
          state={busy ? "loading" : "idle"}
          loadingLabel={actionLabel}
          variant="danger"
          aria-label={actionLabel}
          onClick={onConfirm}
        >
          {actionLabel}
        </DialogStatefulButton>
      </div>
    </DialogShell>
  );
}
