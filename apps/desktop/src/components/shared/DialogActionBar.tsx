import type { ReactNode } from "react";

import "./dialog-action-bar.css";

export type DialogActionBarProps = {
  onCancel: () => void;
  cancelDisabled?: boolean;
  children?: ReactNode;
};

export function DialogActionBar({ onCancel, cancelDisabled = false, children }: DialogActionBarProps) {
  return (
    <div className="dialogActions">
      <button className="secondary" disabled={cancelDisabled} onClick={onCancel}>Cancel</button>
      {children}
    </div>
  );
}
