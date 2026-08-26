import { type ButtonHTMLAttributes, type HTMLAttributes, type ReactNode } from "react";

import "./Badge.css";

export type BadgeTone = "neutral" | "accent" | "info" | "success" | "warning" | "danger" | "meta";

export type BadgeProps = Omit<HTMLAttributes<HTMLElement>, "children" | "className"> & {
  as?: "span" | "button";
  children: ReactNode;
  className?: string;
  disabled?: boolean;
  tone?: BadgeTone;
  type?: ButtonHTMLAttributes<HTMLButtonElement>["type"];
  mono?: boolean;
  uppercase?: boolean;
};

export function Badge({
  as = "span",
  children,
  className = "",
  disabled = false,
  mono = false,
  tone = "neutral",
  type = "button",
  uppercase = false,
  ...props
}: BadgeProps) {
  const Component = as;
  const isTextOnly = typeof children === "string" || typeof children === "number";
  return (
    <Component
      {...props}
      {...(as === "button" ? { disabled, type } : {})}
      className={["badge", className].filter(Boolean).join(" ")}
      data-interactive={as === "button" ? "true" : undefined}
      data-mono={mono ? "true" : undefined}
      data-tone={tone}
      data-uppercase={uppercase ? "true" : undefined}
    >
      {isTextOnly ? <span className="badgeText">{children}</span> : children}
    </Component>
  );
}
