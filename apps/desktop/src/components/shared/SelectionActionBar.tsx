import type { ReactNode } from "react";
import { X } from "lucide-react";

import { SelectionCheckbox } from "./SelectionCheckbox.tsx";
import "./SelectionActionBar.css";

export type SelectionActionBarProps = {
  selectedCount: number;
  allSelected: boolean;
  mixed: boolean;
  onToggleAll: (checked: boolean) => void;
  onClear?: () => void;
  children?: ReactNode;
  label?: string;
  checkboxLabel?: string;
  clearLabel?: string;
  actionsClassName?: string;
  className?: string;
};

export function SelectionActionBar({
  selectedCount,
  allSelected,
  mixed,
  onToggleAll,
  onClear,
  children,
  label = "selected",
  checkboxLabel = "Toggle visible selection",
  clearLabel = "Clear selection",
  actionsClassName = "",
  className = "",
}: SelectionActionBarProps) {
  if (selectedCount === 0) return null;
  const countLabel = selectedCount === 1 && label.endsWith("s") ? label.slice(0, -1) : label;
  return (
    <div className={`actionBar bottomBar ${className}`}>
      <div className="actionBarSurface">
        <div className="actionBarSelectionSummary">
          <SelectionCheckbox
            checked={allSelected}
            mixed={mixed}
            label={checkboxLabel}
            onChange={onToggleAll}
          />
          <span>{selectedCount} {countLabel}</span>
        </div>
        {children || onClear ? (
          <div className={`actionBarActions ${actionsClassName}`.trim()}>
            {children}
            {onClear ? (
              <button className="actionBarClearButton" type="button" aria-label={clearLabel} onClick={onClear}>
                <X size={14} strokeWidth={2.4} />
              </button>
            ) : null}
          </div>
        ) : null}
      </div>
    </div>
  );
}
