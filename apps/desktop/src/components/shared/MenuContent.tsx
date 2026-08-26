import type { ComponentPropsWithoutRef } from "react";
import { DropdownMenu } from "radix-ui";

export type MenuContentProps = ComponentPropsWithoutRef<typeof DropdownMenu.Content>;

export function MenuContent({ className = "", ...props }: MenuContentProps) {
  return (
    <DropdownMenu.Content
      {...props}
      className={["skillMenuContent", className].filter(Boolean).join(" ")}
    />
  );
}
