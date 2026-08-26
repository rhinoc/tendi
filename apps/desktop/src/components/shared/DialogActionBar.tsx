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
      {leading ? <div className="dialogActionsLeading">{leading}</div> : null}
      {leading ? (
        <div className="dialogActionsMain">
          <DialogActionButton variant="secondary" disabled={cancelDisabled} onClick={onCancel}>Cancel</DialogActionButton>
          {children}
        </div>
      ) : (
        <>
          <DialogActionButton variant="secondary" disabled={cancelDisabled} onClick={onCancel}>Cancel</DialogActionButton>
          {children}
        </>
      )}
    </div>
  );
}
