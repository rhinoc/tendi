import type { ButtonHTMLAttributes } from "react";

import "./dialog-action-button.css";

export type DialogActionButtonVariant = "primary" | "secondary" | "danger" | "danger-subtle";

export type DialogActionButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "className"> & {
  variant: DialogActionButtonVariant;
  className?: string;
};

export function DialogActionButton({ variant, className, ...buttonProps }: DialogActionButtonProps) {
  const variantClass = variant === "danger-subtle" ? "danger subtle" : variant;
  return (
    <button
      {...buttonProps}
      className={["dialogActionButton", variantClass, className].filter(Boolean).join(" ")}
    />
  );
}
