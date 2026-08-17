import { forwardRef, type ButtonHTMLAttributes, type HTMLAttributes, type Ref } from "react";
import { Check, Minus } from "lucide-react";

import "./SelectionCheckbox.css";

type CheckboxIndicatorStateProps = {
  checked?: boolean;
  mixed?: boolean;
  disabled?: boolean;
  className?: string;
};

export type CheckboxIndicatorProps =
  | (CheckboxIndicatorStateProps & {
      interactive: true;
    } & Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children" | "className" | "disabled">)
  | (CheckboxIndicatorStateProps & {
      interactive?: false;
    } & Omit<HTMLAttributes<HTMLSpanElement>, "children" | "className">);

export const CheckboxIndicator = forwardRef<HTMLElement, CheckboxIndicatorProps>(function CheckboxIndicator({
  checked = false,
  mixed = false,
  interactive = false,
  disabled = false,
  className = "",
  ...props
}, ref) {
  const stateClassName = checked ? "isChecked" : mixed ? "isMixed" : "";
  const indicator = checked || mixed
    ? (
      <span className="selectionCheckboxIndicator">
        {mixed ? <Minus size={12} strokeWidth={2.7} /> : <Check size={12} strokeWidth={2.7} />}
      </span>
    )
    : null;
  const checkboxClassName = `selectionCheckbox ${stateClassName}${className ? ` ${className}` : ""}`;

  if (interactive) {
    return (
      <button
        {...props}
        ref={ref as Ref<HTMLButtonElement>}
        type="button"
        className={checkboxClassName}
        disabled={disabled}
      >
        {indicator}
      </button>
    );
  }

  return (
    <span
      {...props}
      ref={ref as Ref<HTMLSpanElement>}
      className={checkboxClassName}
      aria-hidden="true"
    >
      {indicator}
    </span>
  );
});
