import { lazy, Suspense } from "react";
import { Dialog } from "radix-ui";

import { DialogActionButton } from "../../components/shared/DialogActionButton.tsx";
import { DialogShell } from "../../components/shared/DialogShell.tsx";

import "./UpdateNotesDialog.css";

const TiptapMarkdownPreview = lazy(() => import("../../components/shared/TiptapMarkdownPreview.tsx").then(({ TiptapMarkdownPreview: component }) => ({ default: component })));

export type UpdateNotesDialogProps = {
  open: boolean;
  version: string;
  body?: string;
  onOpenChange: (open: boolean) => void;
  onInstall: () => void;
};

export function UpdateNotesDialog({ open, version, body, onOpenChange, onInstall }: UpdateNotesDialogProps) {
  const notes = body?.trim() ?? "";

  return (
    <DialogShell
      open={open}
      onOpenChange={onOpenChange}
      className="updateNotesDialogPanel"
      descriptionId="update-notes-description"
    >
      <Dialog.Title className="confirmDialogTitle">Tendi {version} is available</Dialog.Title>
      <Dialog.Description id="update-notes-description" className="confirmDialogDescription">
        Review what changed before installing the update.
      </Dialog.Description>
      <div className="updateNotesDialogBody">
        {notes ? (
          <Suspense fallback={<pre className="updateNotesDialogPlaintext">{notes}</pre>}>
            <TiptapMarkdownPreview content={notes} />
          </Suspense>
        ) : (
          <p className="updateNotesDialogEmpty">Release notes are not available for this update.</p>
        )}
      </div>
      <div className="confirmDialogActions">
        <DialogActionButton variant="secondary" onClick={() => onOpenChange(false)}>Later</DialogActionButton>
        <DialogActionButton
          variant="primary"
          aria-label={`Install Tendi ${version}`}
          onClick={() => {
            onOpenChange(false);
            onInstall();
          }}
        >
          Install update
        </DialogActionButton>
      </div>
    </DialogShell>
  );
}
