import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";

import "./Button.css";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "outline" | "danger" | "icon";
export type ButtonSize = "sm" | "compact";

export type ButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "className"> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
  iconOnly?: boolean;
  className?: string;
  children?: ReactNode;
};

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button({
  variant = "secondary",
  size = "sm",
  iconOnly = false,
  className = "",
  type = "button",
  ...buttonProps
}, ref) {
  return (
    <button
      ref={ref}
      {...buttonProps}
      type={type}
      className={["appButton", `appButton-${variant}`, `appButton-${size}`, iconOnly ? "appButton-iconOnly" : "", className].filter(Boolean).join(" ")}
    />
  );
});
