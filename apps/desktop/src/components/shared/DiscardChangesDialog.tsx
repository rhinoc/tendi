import { Dialog } from "radix-ui";

import { DialogActionButton } from "./DialogActionButton.tsx";
import { DialogShell } from "./DialogShell.tsx";
import { dialogCopy } from "../../lib/index.ts";

export type DiscardChangesDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDiscard: () => void;
};

export function DiscardChangesDialog({ open, onOpenChange, onDiscard }: DiscardChangesDialogProps) {
  return (
    <DialogShell open={open} onOpenChange={onOpenChange} descriptionId="discard-changes-description">
          <Dialog.Title className="confirmDialogTitle">{dialogCopy.discardChangesTitle}</Dialog.Title>
          <p id="discard-changes-description" className="confirmDialogDescription">
            This file has edits that have not been saved.
          </p>
          <div className="confirmDialogActions">
            <DialogActionButton variant="secondary" onClick={() => onOpenChange(false)}>Cancel</DialogActionButton>
            <DialogActionButton
              variant="danger"
              onClick={() => {
                onOpenChange(false);
                onDiscard();
              }}
            >
              Discard changes
            </DialogActionButton>
          </div>
    </DialogShell>
  );
}
