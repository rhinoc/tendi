import { Dialog } from "radix-ui";

import { DialogActionButton } from "./DialogActionButton.tsx";
import "./confirm-dialog.css";

type BundledSkillInstallDialogProps = {
  open: boolean;
  target: string;
  busy: boolean;
  error?: string;
  onInstall: () => void;
  onDismiss: () => void;
};

export function BundledSkillInstallDialog({
  open,
  target,
  busy,
  error,
  onInstall,
  onDismiss,
}: BundledSkillInstallDialogProps) {
  return (
    <Dialog.Root open={open} onOpenChange={(nextOpen) => !nextOpen && !busy && onDismiss()}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialogOverlay" />
        <Dialog.Content
          className="confirmDialogPanel"
          aria-describedby="bundled-skill-description"
          data-no-drag
          onMouseDown={(event) => event.stopPropagation()}
        >
          <Dialog.Title className="confirmDialogTitle">Set up Tendi for coding agents?</Dialog.Title>
          <p id="bundled-skill-description" className="confirmDialogDescription">
            This registers the <code>tendi</code> command on your shell PATH, then installs the
            Tendi skill into <code>{target}</code>. Coding agents can search local sessions and
            manage skills; no session data is uploaded.
          </p>
          {error ? <p className="skillUpdatePreviewError" role="alert">{error}</p> : null}
          <div className="confirmDialogActions">
            <DialogActionButton variant="secondary" disabled={busy} onClick={onDismiss}>Skip</DialogActionButton>
            <DialogActionButton variant="primary" disabled={busy} onClick={onInstall}>
              {busy ? "Setting up…" : "Set up CLI & skill"}
            </DialogActionButton>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
