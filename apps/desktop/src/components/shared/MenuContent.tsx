import { useRef, type ComponentPropsWithoutRef } from "react";
import { DropdownMenu } from "radix-ui";

export type MenuContentProps = ComponentPropsWithoutRef<typeof DropdownMenu.Content> & {
  variant?: "menu" | "select";
};

export function MenuContent({ variant = "menu", className = "", onCloseAutoFocus, onKeyDownCapture, onPointerDownCapture, ...props }: MenuContentProps) {
  const pointerInteractionRef = useRef(false);
  return (
    <DropdownMenu.Content
      {...props}
      onPointerDownCapture={(event) => {
        pointerInteractionRef.current = true;
        onPointerDownCapture?.(event);
      }}
      onKeyDownCapture={(event) => {
        pointerInteractionRef.current = false;
        onKeyDownCapture?.(event);
      }}
      onCloseAutoFocus={(event) => {
        onCloseAutoFocus?.(event);
        if (pointerInteractionRef.current) event.preventDefault();
        pointerInteractionRef.current = false;
      }}
      className={[variant === "select" ? "selectControlContent" : "menuContent", className].filter(Boolean).join(" ")}
    />
  );
}
