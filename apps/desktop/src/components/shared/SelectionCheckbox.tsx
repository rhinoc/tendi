import { Check, Minus } from "lucide-react";
import { Checkbox } from "radix-ui";

import "./SelectionCheckbox.css";

export type SelectionCheckboxProps = {
  checked: boolean;
  mixed?: boolean;
  label: string;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
  className?: string;
};

export function SelectionCheckbox({
  checked,
  mixed = false,
  label,
  disabled = false,
  onChange,
  className = "",
}: SelectionCheckboxProps) {
  return (
    <Checkbox.Root
      className={`selectionCheckbox ${checked ? "isChecked" : ""} ${mixed ? "isMixed" : ""} ${className}`}
      checked={mixed ? "indeterminate" : checked}
      aria-label={label}
      disabled={disabled}
      onCheckedChange={(next) => onChange(next === true)}
    >
      <Checkbox.Indicator className="selectionCheckboxIndicator">
        {mixed ? <Minus size={12} strokeWidth={2.7} /> : <Check size={12} strokeWidth={2.7} />}
      </Checkbox.Indicator>
    </Checkbox.Root>
  );
}
