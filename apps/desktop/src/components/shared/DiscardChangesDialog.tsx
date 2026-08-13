import { Dialog } from "radix-ui";

import "./confirm-dialog.css";

export type DiscardChangesDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDiscard: () => void;
};

export function DiscardChangesDialog({ open, onOpenChange, onDiscard }: DiscardChangesDialogProps) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialogOverlay" />
        <Dialog.Content className="confirmDialogPanel" aria-describedby="discard-changes-description" data-no-drag onMouseDown={(event) => event.stopPropagation()}>
          <Dialog.Title className="confirmDialogTitle">Discard unsaved changes?</Dialog.Title>
          <p id="discard-changes-description" className="confirmDialogDescription">
            This file has edits that have not been saved.
          </p>
          <div className="confirmDialogActions">
            <button className="secondary" onClick={() => onOpenChange(false)}>Cancel</button>
            <button
              className="danger"
              onClick={() => {
                onOpenChange(false);
                onDiscard();
              }}
            >
              Discard changes
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
