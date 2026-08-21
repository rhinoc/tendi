import type { ButtonHTMLAttributes } from "react";

import { Button, type ButtonVariant } from "./Button.tsx";
import "./dialog-action-button.css";

export type DialogActionButtonVariant = "primary" | "secondary" | "danger";

export type DialogActionButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "className"> & {
  variant: DialogActionButtonVariant;
  className?: string;
};

export function DialogActionButton({ variant, className, ...buttonProps }: DialogActionButtonProps) {
  const buttonVariant: ButtonVariant = variant === "secondary" ? "ghost" : variant;
  return (
    <Button
      {...buttonProps}
      variant={buttonVariant}
      size="sm"
      className={["dialogActionButton", className].filter(Boolean).join(" ")}
    />
  );
}
