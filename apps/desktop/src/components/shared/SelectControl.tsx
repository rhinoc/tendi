import { useState, type ComponentPropsWithoutRef, type FocusEventHandler, type KeyboardEventHandler, type ReactNode } from "react";
import { Check, ChevronDown, ChevronRight } from "lucide-react";
import { DropdownMenu, Select } from "radix-ui";

import { MenuContent } from "./MenuContent.tsx";
import { SelectTrigger } from "./SelectTrigger.tsx";
import { useElementSize } from "./useElementSize.ts";
import { useRowMenuOpenChange } from "./row-menu-context.tsx";
import { resolveSelectValue } from "../../lib/select-options.ts";

export type SelectOption = {
  value: string;
  label: string;
  available?: boolean;
};

export type SelectOptionGroup = {
  value: string;
  label: string;
  options: SelectOption[];
};

export type SelectMenuAction = {
  label: string;
  onSelect: () => void;
  icon?: ReactNode;
  disabled?: boolean;
};

type SelectContentProps = ComponentPropsWithoutRef<typeof Select.Content>;

export type SelectControlProps = {
  variant?: "default" | "editable";
  value: string;
  onValueChange: (value: string) => void;
  label: string;
  options?: SelectOption[];
  groups?: SelectOptionGroup[];
  className?: string;
  contentClassName?: string;
  itemClassName?: string;
  renderOption?: (option: SelectOption) => ReactNode;
  renderValue?: (option: SelectOption | undefined) => ReactNode;
  side?: SelectContentProps["side"];
  align?: SelectContentProps["align"];
  indicatorPosition?: "left" | "right";
  showChevron?: boolean;
  disabled?: boolean;
  menuAction?: SelectMenuAction;
  inputId?: string;
  inputAriaLabel?: string;
  inputPlaceholder?: string;
  inputValue?: string;
  onInputChange?: (value: string) => void;
  onInputBlur?: FocusEventHandler<HTMLInputElement>;
  onInputKeyDown?: KeyboardEventHandler<HTMLInputElement>;
  menuAriaLabel?: string;
};

const SELECT_MENU_ACTION_VALUE = "__select_control_menu_action__";
export function SelectControl({
  variant = "default",
  groups,
  ...props
}: SelectControlProps) {
  if (variant === "editable") return <EditableSelectControl {...props} variant={variant} />;
  if (groups) return <GroupedSelectControl {...props} groups={groups} variant={variant} />;
  return <StandardSelectControl {...props} variant={variant} />;
}

function StandardSelectControl({
  value,
  onValueChange,
  label,
  options = [],
  className = "",
  contentClassName = "",
  itemClassName = "",
  renderOption,
  renderValue,
  side,
  align,
  indicatorPosition = "right",
  showChevron = true,
  disabled = false,
  menuAction,
  variant: _variant,
  inputId: _inputId,
  inputAriaLabel: _inputAriaLabel,
  inputPlaceholder: _inputPlaceholder,
  inputValue: _inputValue,
  onInputChange: _onInputChange,
  onInputBlur: _onInputBlur,
  onInputKeyDown: _onInputKeyDown,
  menuAriaLabel: _menuAriaLabel,
}: SelectControlProps) {
  const notifyRowMenuOpenChange = useRowMenuOpenChange();
  const resolvedValue = resolveSelectValue(value, options);
  const selectedOption = options.find((option) => option.value === resolvedValue);
  const selectedLabel = selectedOption?.label ?? value;
  return (
    <Select.Root
      value={resolvedValue}
      onOpenChange={(open) => notifyRowMenuOpenChange?.(open)}
      onValueChange={(nextValue) => {
        if (nextValue === SELECT_MENU_ACTION_VALUE) {
          menuAction?.onSelect();
          return;
        }
        onValueChange(nextValue);
      }}
    >
      <SelectTrigger className={className} label={label} showChevron={showChevron} disabled={disabled}>
        <Select.Value>
          {renderValue
            ? renderValue(selectedOption)
            : renderOption && selectedOption
              ? renderOption(selectedOption)
              : <span className="selectValueText">{selectedLabel}</span>}
        </Select.Value>
      </SelectTrigger>
      <Select.Portal>
        <Select.Content
          className={["selectControlContent", contentClassName].filter(Boolean).join(" ")}
          position="popper"
          side={side}
          align={align}
          sideOffset={8}
          style={{
            width: "max-content",
            minWidth: "var(--radix-select-trigger-width)",
          }}
          data-no-drag
        >
          <Select.Viewport className="selectControlViewport selectViewport">
            {options.map((option) => (
              <Select.Item
                className={["selectControlItem", itemClassName, indicatorPosition === "right" ? "selectControlItemIndicatorRight selectItemIndicatorRight" : ""].filter(Boolean).join(" ")}
                value={option.value}
                key={option.value}
              >
                <span className="selectControlItemLeadingIcon selectItemLeadingIcon" aria-hidden="true">
                  <Select.ItemIndicator className="selectControlItemIndicator selectItemIndicator">
                    <Check size={14} />
                  </Select.ItemIndicator>
                </span>
                <Select.ItemText asChild>
                  <span className="selectControlItemText selectItemText">
                    {renderOption ? renderOption(option) : option.label}
                  </span>
                </Select.ItemText>
              </Select.Item>
            ))}
          </Select.Viewport>
          {menuAction ? (
            <>
              <Select.Separator className="selectControlMenuActionSeparator selectMenuActionSeparator" />
              <Select.Item
                className="selectControlItem selectMenuActionItem"
                value={SELECT_MENU_ACTION_VALUE}
                disabled={menuAction.disabled}
              >
                <span className="selectControlItemLeadingIcon selectItemLeadingIcon selectControlMenuActionIcon selectMenuActionIcon" aria-hidden="true">
                  {menuAction.icon}
                </span>
                <Select.ItemText asChild>
                  <span className="selectControlItemText selectItemText">{menuAction.label}</span>
                </Select.ItemText>
              </Select.Item>
            </>
          ) : null}
        </Select.Content>
      </Select.Portal>
    </Select.Root>
  );
}

function GroupedSelectControl({
  value,
  onValueChange,
  label,
  groups,
  className = "",
  contentClassName = "",
  itemClassName = "",
  renderOption,
  renderValue,
  side,
  align,
  indicatorPosition = "right",
  showChevron = true,
  disabled = false,
  variant: _variant,
  options: _options,
  menuAction: _menuAction,
  inputId: _inputId,
  inputAriaLabel: _inputAriaLabel,
  inputPlaceholder: _inputPlaceholder,
  inputValue: _inputValue,
  onInputChange: _onInputChange,
  onInputBlur: _onInputBlur,
  onInputKeyDown: _onInputKeyDown,
  menuAriaLabel: _menuAriaLabel,
}: SelectControlProps & { groups: SelectOptionGroup[] }) {
  const notifyRowMenuOpenChange = useRowMenuOpenChange();
  const { ref: triggerRef, size: triggerSize } = useElementSize<HTMLButtonElement>({ width: 0, height: 0 });
  const options = groups.flatMap((group) => group.options);
  const resolvedValue = resolveSelectValue(value, options);
  const selectedOption = options.find((option) => option.value === resolvedValue);
  const indicatorClass = indicatorPosition === "right" ? "selectItemIndicatorRight" : "";
  const renderMenuOption = (
    option: SelectOption,
    selected: boolean,
    accessibleLabel = option.label,
    visibleLabel?: ReactNode,
  ) => {
    const indicator = (
      <span className="selectItemLeadingIcon" aria-hidden="true">
        {selected ? <Check className="selectItemIndicator" size={14} /> : null}
      </span>
    );
    return (
      <DropdownMenu.Item
        className={["menuItem", indicatorClass, itemClassName].filter(Boolean).join(" ")}
        data-state={selected ? "checked" : undefined}
        aria-label={`${accessibleLabel}${selected ? ", selected" : ""}`}
        key={option.value}
        onSelect={() => onValueChange(option.value)}
      >
        {indicatorPosition === "left" ? indicator : null}
        <span className="selectItemText">
          {visibleLabel ?? (renderOption ? renderOption(option) : option.label)}
        </span>
        {indicatorPosition === "right" ? indicator : null}
      </DropdownMenu.Item>
    );
  };

  return (
    <DropdownMenu.Root onOpenChange={(open) => notifyRowMenuOpenChange?.(open)}>
      <DropdownMenu.Trigger asChild>
        <button
          ref={triggerRef}
          type="button"
          className={["selectControlTrigger", className].filter(Boolean).join(" ")}
          aria-label={label}
          disabled={disabled}
        >
          <span className="selectValueText">
            {renderValue
              ? renderValue(selectedOption)
              : renderOption && selectedOption
                ? renderOption(selectedOption)
                : selectedOption?.label ?? value}
          </span>
          {showChevron ? <ChevronDown size={14} aria-hidden="true" /> : null}
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <MenuContent
          variant="select"
          className={contentClassName}
          style={{ width: "max-content", minWidth: triggerSize.width || undefined }}
          side={side}
          align={align ?? "end"}
          sideOffset={8}
          data-no-drag
        >
          {groups.map((group) => {
            if (group.options.length === 1) {
              const option = group.options[0];
              return renderMenuOption(
                option,
                option.value === resolvedValue,
                `${group.label}, ${option.label}`,
                group.label,
              );
            }
            return (
              <DropdownMenu.Sub key={group.value}>
                <DropdownMenu.SubTrigger className={["menuItem", "menuSubTrigger", itemClassName].filter(Boolean).join(" ")}>
                  <span className="selectItemText">{group.label}</span>
                  <ChevronRight className="menuSubIcon" size={14} aria-hidden="true" />
                </DropdownMenu.SubTrigger>
                <DropdownMenu.Portal>
                  <DropdownMenu.SubContent
                    className={["menuContent", "selectControlContent", contentClassName].filter(Boolean).join(" ")}
                    sideOffset={8}
                    alignOffset={-6}
                    style={{ width: "max-content" }}
                    data-no-drag
                  >
                    {group.options.map((option) => renderMenuOption(
                      option,
                      option.value === resolvedValue,
                    ))}
                  </DropdownMenu.SubContent>
                </DropdownMenu.Portal>
              </DropdownMenu.Sub>
            );
          })}
        </MenuContent>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function EditableSelectControl({
  value,
  onValueChange,
  label,
  options = [],
  className = "",
  contentClassName = "",
  itemClassName = "",
  renderOption,
  side,
  align,
  indicatorPosition = "right",
  showChevron = true,
  disabled = false,
  menuAction,
  inputId,
  inputAriaLabel,
  inputPlaceholder,
  inputValue,
  onInputChange,
  onInputBlur,
  onInputKeyDown,
  menuAriaLabel,
}: SelectControlProps) {
  const notifyRowMenuOpenChange = useRowMenuOpenChange();
  const [menuOpen, setMenuOpen] = useState(false);
  const { ref: triggerRef, size: triggerSize } = useElementSize<HTMLDivElement>({ width: 0, height: 0 });
  const selectedOption = options.find((option) => option.value === value);
  const selectedLabel = selectedOption
    ? `${selectedOption.label}${selectedOption.available === false ? " (not found)" : ""}`
    : inputValue ?? value;
  const displayedValue = inputValue ?? selectedLabel;

  const chooseOption = (nextValue: string) => {
    onValueChange(nextValue);
    setMenuOpen(false);
  };

  return (
    <div
      ref={triggerRef}
      className={["selectControlEditable", className].filter(Boolean).join(" ")}
      data-state={menuOpen ? "open" : "closed"}
    >
        <input
          id={inputId}
          className="selectControlEditableInput"
          aria-label={inputAriaLabel ?? label}
          placeholder={inputPlaceholder}
          value={displayedValue}
          disabled={disabled}
          onChange={(event) => onInputChange?.(event.currentTarget.value)}
          onBlur={onInputBlur}
          onKeyDown={onInputKeyDown}
        />
        <DropdownMenu.Root
          open={menuOpen}
          onOpenChange={(open) => {
            setMenuOpen(open);
            notifyRowMenuOpenChange?.(open);
          }}
        >
          <DropdownMenu.Trigger asChild>
            <button
              type="button"
              className="selectControlEditableChevron"
              aria-label={menuAriaLabel ?? `Choose ${label.toLowerCase()}`}
              disabled={disabled}
            >
              {showChevron ? <ChevronDown size={14} aria-hidden="true" /> : null}
            </button>
          </DropdownMenu.Trigger>
          <DropdownMenu.Portal>
            <MenuContent
              variant="select"
              className={contentClassName}
              style={{
                width: "max-content",
                minWidth: triggerSize.width || undefined,
              }}
              side={side}
              align={align ?? "end"}
              alignOffset={-5}
              sideOffset={8}
              data-no-drag
            >
              <div className="selectControlViewport selectViewport selectEditableViewport">
                {options.map((option) => {
                  const optionLabel = `${option.label}${option.available === false ? " (not found)" : ""}`;
                  const optionText = (
                    <span className="selectControlItemText selectItemText">
                      {renderOption ? renderOption(option) : optionLabel}
                    </span>
                  );
                  return (
                    <DropdownMenu.Item
                      className={["selectControlItem", itemClassName, indicatorPosition === "right" ? "selectControlItemIndicatorRight selectItemIndicatorRight" : ""].filter(Boolean).join(" ")}
                      data-state={option.value === value ? "checked" : undefined}
                      key={option.value}
                      onSelect={() => chooseOption(option.value)}
                    >
                      <span className="selectControlItemLeadingIcon selectItemLeadingIcon" aria-hidden="true">
                        {option.value === value ? <Check className="selectControlItemIndicator selectItemIndicator" size={14} /> : null}
                      </span>
                      {optionText}
                    </DropdownMenu.Item>
                  );
                })}
              </div>
              {menuAction ? (
                <>
                  <DropdownMenu.Separator className="selectControlMenuActionSeparator selectMenuActionSeparator" />
                  <DropdownMenu.Item
                    className="selectControlItem selectMenuActionItem"
                    disabled={menuAction.disabled}
                    onSelect={menuAction.onSelect}
                  >
                    <span className="selectControlItemLeadingIcon selectItemLeadingIcon selectControlMenuActionIcon selectMenuActionIcon" aria-hidden="true">
                      {menuAction.icon}
                    </span>
                    <span className="selectControlItemText selectItemText">{menuAction.label}</span>
                  </DropdownMenu.Item>
                </>
              ) : null}
            </MenuContent>
          </DropdownMenu.Portal>
        </DropdownMenu.Root>
    </div>
  );
}
