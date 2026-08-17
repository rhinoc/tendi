import type { ComponentPropsWithoutRef, ReactNode } from "react";
import { Check } from "lucide-react";
import { Tooltip } from "./Tooltip.tsx";
import { Select } from "radix-ui";

import { SelectTrigger } from "./SelectTrigger.tsx";

export type SelectOption = {
  value: string;
  label: string;
};

type SelectContentProps = ComponentPropsWithoutRef<typeof Select.Content>;

export type SelectControlProps = {
  value: string;
  onValueChange: (value: string) => void;
  label: string;
  options: SelectOption[];
  className?: string;
  contentClassName?: string;
  itemClassName?: string;
  renderOption?: (option: SelectOption) => ReactNode;
  renderValue?: (option: SelectOption | undefined) => ReactNode;
  side?: SelectContentProps["side"];
  align?: SelectContentProps["align"];
  showChevron?: boolean;
  triggerTooltipContent?: ReactNode;
};

export function SelectControl({
  value,
  onValueChange,
  label,
  options,
  className = "",
  contentClassName = "",
  itemClassName = "",
  renderOption,
  renderValue,
  side,
  align,
  showChevron = true,
  triggerTooltipContent,
}: SelectControlProps) {
  const selectedOption = options.find((option) => option.value === value);
  const selectedLabel = selectedOption?.label ?? value;
  return (
    <Select.Root value={value} onValueChange={onValueChange}>
      <Tooltip content={triggerTooltipContent}>
        <SelectTrigger className={className} label={label} showChevron={showChevron}>
          <Select.Value>
            {renderValue ? renderValue(selectedOption) : <span className="selectValueText">{selectedLabel}</span>}
          </Select.Value>
        </SelectTrigger>
      </Tooltip>
      <Select.Portal>
        <Select.Content
          className={`skillMenuContent ${contentClassName}`.trim()}
          position="popper"
          side={side}
          align={align}
          sideOffset={6}
        >
          <Select.Viewport className="selectViewport">
            {options.map((option) => (
              <Select.Item className={`skillMenuItem ${itemClassName}`.trim()} value={option.value} key={option.value}>
                <Tooltip content={option.label} onlyWhenTruncated>
                  <Select.ItemText asChild>
                    <span className="selectItemText">
                      {renderOption ? renderOption(option) : option.label}
                    </span>
                  </Select.ItemText>
                </Tooltip>
                <Select.ItemIndicator className="selectItemIndicator">
                  <Check size={14} aria-hidden="true" />
                </Select.ItemIndicator>
              </Select.Item>
            ))}
          </Select.Viewport>
        </Select.Content>
      </Select.Portal>
    </Select.Root>
  );
}
