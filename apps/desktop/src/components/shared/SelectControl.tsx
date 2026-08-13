import { Tooltip } from "./Tooltip.tsx";
import { Check, ChevronDown } from "lucide-react";
import { Select } from "radix-ui";

export type SelectOption = {
  value: string;
  label: string;
};

export type SelectControlProps = {
  value: string;
  onValueChange: (value: string) => void;
  label: string;
  options: SelectOption[];
  className?: string;
  contentClassName?: string;
};

export function SelectControl({
  value,
  onValueChange,
  label,
  options,
  className = "",
  contentClassName = "",
}: SelectControlProps) {
  const selectedLabel = options.find((option) => option.value === value)?.label ?? value;
  return (
    <Select.Root value={value} onValueChange={onValueChange}>
      <Select.Trigger className={className} aria-label={label}>
        <Select.Value>
          <span className="selectValueText">{selectedLabel}</span>
        </Select.Value>
        <Select.Icon asChild>
          <ChevronDown size={14} />
        </Select.Icon>
      </Select.Trigger>
      <Select.Portal>
        <Select.Content className={`skillMenuContent ${contentClassName}`} position="popper" sideOffset={6}>
          <Select.Viewport className="selectViewport">
            {options.map((option) => (
              <Select.Item className="skillMenuItem" value={option.value} key={option.value}>
                <Tooltip content={option.label} onlyWhenTruncated>
                  <Select.ItemText asChild>
                    <span className="selectItemText">{option.label}</span>
                  </Select.ItemText>
                </Tooltip>
                <Select.ItemIndicator className="selectItemIndicator">
                  <Check size={13} strokeWidth={2.6} />
                </Select.ItemIndicator>
              </Select.Item>
            ))}
          </Select.Viewport>
        </Select.Content>
      </Select.Portal>
    </Select.Root>
  );
}
