import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";

import "./Button.css";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "icon";
export type ButtonSize = "sm";

export type ButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "className"> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
  className?: string;
  children?: ReactNode;
};

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button({
  variant = "secondary",
  size = "sm",
  className = "",
  type = "button",
  ...buttonProps
}, ref) {
  return (
    <button
      ref={ref}
      {...buttonProps}
      type={type}
      className={["appButton", `appButton-${variant}`, `appButton-${size}`, className].filter(Boolean).join(" ")}
    />
  );
});
