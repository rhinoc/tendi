import type { ComponentPropsWithoutRef, ReactNode } from "react";
import { Check } from "lucide-react";
import { Tooltip } from "./Tooltip.tsx";
import { Select } from "radix-ui";

import { SelectTrigger } from "./SelectTrigger.tsx";
import { resolveSelectValue } from "../../lib/select-options.ts";

export type SelectOption = {
  value: string;
  label: string;
};

export type SelectMenuAction = {
  label: string;
  onSelect: () => void;
  icon?: ReactNode;
  disabled?: boolean;
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
  indicatorPosition?: "left" | "right";
  showChevron?: boolean;
  showOptionTooltip?: boolean;
  disabled?: boolean;
  triggerTooltipContent?: ReactNode;
  menuAction?: SelectMenuAction;
};

const SELECT_MENU_ACTION_VALUE = "__select_control_menu_action__";

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
  indicatorPosition = "right",
  showChevron = true,
  showOptionTooltip = true,
  disabled = false,
  triggerTooltipContent,
  menuAction,
}: SelectControlProps) {
  const resolvedValue = resolveSelectValue(value, options);
  const selectedOption = options.find((option) => option.value === resolvedValue);
  const selectedLabel = selectedOption?.label ?? value;
  return (
    <Select.Root
      value={resolvedValue}
      onValueChange={(nextValue) => {
        if (nextValue === SELECT_MENU_ACTION_VALUE) {
          menuAction?.onSelect();
          return;
        }
        onValueChange(nextValue);
      }}
    >
      <Tooltip content={triggerTooltipContent}>
        <SelectTrigger className={className} label={label} showChevron={showChevron} disabled={disabled}>
          <Select.Value>
            {renderValue
              ? renderValue(selectedOption)
              : renderOption && selectedOption
                ? renderOption(selectedOption)
                : <span className="selectValueText">{selectedLabel}</span>}
          </Select.Value>
        </SelectTrigger>
      </Tooltip>
      <Select.Portal>
        <Select.Content
          className={`skillMenuContent selectControlContent ${contentClassName}`.trim()}
          data-no-drag
          position="popper"
          side={side}
          align={align}
          sideOffset={6}
          data-no-drag
        >
          <Select.Viewport className="selectViewport">
            {options.map((option) => (
              <Select.Item
                className={`skillMenuItem ${itemClassName} ${indicatorPosition === "right" ? "selectItemIndicatorRight" : ""}`.trim()}
                value={option.value}
                key={option.value}
              >
                <span className="selectItemLeadingIcon" aria-hidden="true">
                  <Select.ItemIndicator className="selectItemIndicator">
                    <Check size={14} />
                  </Select.ItemIndicator>
                </span>
                {showOptionTooltip ? (
                  <Tooltip content={option.label} onlyWhenTruncated>
                    <Select.ItemText asChild>
                      <span className="selectItemText">
                        {renderOption ? renderOption(option) : option.label}
                      </span>
                    </Select.ItemText>
                  </Tooltip>
                ) : (
                  <Select.ItemText asChild>
                    <span className="selectItemText">
                      {renderOption ? renderOption(option) : option.label}
                    </span>
                  </Select.ItemText>
                )}
              </Select.Item>
            ))}
          </Select.Viewport>
          {menuAction ? (
            <>
              <Select.Separator className="selectMenuActionSeparator" />
              <Select.Item
                className="skillMenuItem selectMenuActionItem"
                value={SELECT_MENU_ACTION_VALUE}
                disabled={menuAction.disabled}
              >
                <span className="selectItemLeadingIcon selectMenuActionIcon" aria-hidden="true">
                  {menuAction.icon}
                </span>
                <Select.ItemText asChild>
                  <span className="selectItemText">{menuAction.label}</span>
                </Select.ItemText>
              </Select.Item>
            </>
          ) : null}
        </Select.Content>
      </Select.Portal>
    </Select.Root>
  );
}
