import { forwardRef, type ButtonHTMLAttributes } from "react";

import "./Switch.css";

export type SwitchProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children" | "disabled"> & {
  checked: boolean;
  label: string;
  disabled?: boolean;
  onCheckedChange: (checked: boolean) => void;
};

export const Switch = forwardRef<HTMLButtonElement, SwitchProps>(function Switch({
  checked,
  label,
  disabled = false,
  onCheckedChange,
  className = "",
  onClick,
  onKeyDown,
  "aria-label": ariaLabel,
  ...buttonProps
}, ref) {
  return (
    <button
      {...buttonProps}
      ref={ref}
      type="button"
      className={["appSwitch", checked ? "isOn" : "isOff", className].filter(Boolean).join(" ")}
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel ?? label}
      aria-disabled={disabled || undefined}
      onClick={(event) => {
        onClick?.(event);
        if (!disabled && !event.defaultPrevented) onCheckedChange(!checked);
      }}
      onKeyDown={(event) => {
        onKeyDown?.(event);
        if (disabled && (event.key === "Enter" || event.key === " ")) {
          event.preventDefault();
          event.stopPropagation();
        }
      }}
    >
      <span className="appSwitchThumb" />
    </button>
  );
});
