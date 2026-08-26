import { Dialog } from "radix-ui";

import { DialogActionButton } from "../../components/shared/DialogActionButton.tsx";
import { DialogShell } from "../../components/shared/DialogShell.tsx";
import { DialogStatefulButton } from "../../components/shared/DialogStatefulButton.tsx";
import { Toast } from "../../components/shared/Toast.tsx";

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
    <DialogShell
      open={open}
      onOpenChange={(nextOpen) => !nextOpen && !busy && onDismiss()}
      descriptionId="bundled-skill-description"
    >
          <Dialog.Title className="confirmDialogTitle">Set up Tendi for coding agents?</Dialog.Title>
          <p id="bundled-skill-description" className="confirmDialogDescription">
            This registers the <code>tendi</code> command on your shell PATH, then installs the
            Tendi skill into <span>{target}</span>. Coding agents can search local sessions and
            manage skills; no session data is uploaded.
          </p>
          {error ? <Toast tone="error" message={error} /> : null}
          <div className="confirmDialogActions">
            <DialogActionButton variant="secondary" disabled={busy} onClick={onDismiss}>Skip</DialogActionButton>
            <DialogStatefulButton
              className="dialogStatefulButtonWide"
              state={busy ? "loading" : "idle"}
              loadingLabel="Setting up"
              variant="primary"
              aria-label="Set up"
              onClick={onInstall}
            >
              Set up
            </DialogStatefulButton>
          </div>
    </DialogShell>
  );
}
