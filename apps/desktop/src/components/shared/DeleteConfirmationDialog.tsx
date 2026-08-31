import { Dialog } from "radix-ui";

import { DialogActionButton } from "./DialogActionButton.tsx";
import { DialogShell } from "./DialogShell.tsx";
import { DialogStatefulButton } from "./DialogStatefulButton.tsx";
import { deleteConfirmationDescription, selectionDeleteLoadingLabel } from "../../lib/action-labels.ts";
import { AsyncStatus } from "../../lib/async-status.ts";

export function DeleteConfirmationDialog({
  open,
  items,
  itemLabel,
  title,
  description,
  confirmLabel,
  loadingLabel,
  busy = false,
  onOpenChange,
  onConfirm,
}: {
  open: boolean;
  items: readonly string[];
  itemLabel: string;
  title?: string;
  description?: string;
  confirmLabel?: string;
  loadingLabel?: string;
  busy?: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  const plural = items.length === 1 ? "" : "s";
  const actionLabel = confirmLabel ?? `Delete ${itemLabel}${plural}`;
  const dialogTitle = title ?? `Delete ${itemLabel}${plural}?`;
  return (
    <DialogShell
      open={open}
      onOpenChange={(nextOpen) => {
        if (!busy) onOpenChange(nextOpen);
      }}
      descriptionId="delete-confirmation-description"
    >
      <Dialog.Title className="confirmDialogTitle">{dialogTitle}</Dialog.Title>
      <p id="delete-confirmation-description" className="confirmDialogDescription">
        {description ?? deleteConfirmationDescription(itemLabel, items.length)}
      </p>
      <div className="deleteConfirmationNames" data-selectable-text>
        {items.map((item, index) => <span key={`${item}-${index}`}>{item}</span>)}
      </div>
      <div className="confirmDialogActions">
        <DialogActionButton variant="secondary" disabled={busy} onClick={() => onOpenChange(false)}>Cancel</DialogActionButton>
        <DialogStatefulButton
          state={busy ? AsyncStatus.Loading : AsyncStatus.Idle}
          loadingLabel={loadingLabel ?? selectionDeleteLoadingLabel(itemLabel, items.length)}
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
