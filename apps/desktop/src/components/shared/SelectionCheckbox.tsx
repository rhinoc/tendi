import { Checkbox } from "radix-ui";

import { CheckboxIndicator } from "./CheckboxIndicator.tsx";

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
      asChild
      checked={mixed ? "indeterminate" : checked}
      aria-label={label}
      disabled={disabled}
      onCheckedChange={(next) => onChange(next === true)}
    >
      <CheckboxIndicator
        checked={checked}
        mixed={mixed}
        className={className}
        interactive
        disabled={disabled}
        aria-label={label}
      />
    </Checkbox.Root>
  );
}
