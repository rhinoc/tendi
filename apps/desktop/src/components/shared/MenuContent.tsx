import { useRef, type ComponentPropsWithoutRef } from "react";
import { DropdownMenu } from "radix-ui";

export type MenuContentProps = ComponentPropsWithoutRef<typeof DropdownMenu.Content>;

export function MenuContent({ className = "", onCloseAutoFocus, onKeyDownCapture, onPointerDownCapture, ...props }: MenuContentProps) {
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
      className={["skillMenuContent", className].filter(Boolean).join(" ")}
    />
  );
}
