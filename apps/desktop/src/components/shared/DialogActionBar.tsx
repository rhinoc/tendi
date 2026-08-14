import type { ReactNode } from "react";

import { DialogActionButton } from "./DialogActionButton.tsx";
import "./dialog-action-bar.css";

export type DialogActionBarProps = {
  onCancel: () => void;
  cancelDisabled?: boolean;
  children?: ReactNode;
};

export function DialogActionBar({ onCancel, cancelDisabled = false, children }: DialogActionBarProps) {
  return (
    <div className="dialogActions">
      <DialogActionButton variant="secondary" disabled={cancelDisabled} onClick={onCancel}>Cancel</DialogActionButton>
      {children}
    </div>
  );
}
