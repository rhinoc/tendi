import type { ReactNode } from "react";

import { DialogActionButton } from "./DialogActionButton.tsx";
import "./dialog-action-bar.css";

export type DialogActionBarProps = {
  onCancel: () => void;
  cancelDisabled?: boolean;
  leading?: ReactNode;
  children?: ReactNode;
};

export function DialogActionBar({ onCancel, cancelDisabled = false, leading, children }: DialogActionBarProps) {
  return (
    <div className="dialogActions">
      <div className="dialogActionsLeading">
        <DialogActionButton variant="secondary" disabled={cancelDisabled} onClick={onCancel}>Cancel</DialogActionButton>
        {leading}
      </div>
      {children ? <div className="dialogActionsMain">{children}</div> : null}
    </div>
  );
}
