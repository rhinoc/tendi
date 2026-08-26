import { MenuContent, type MenuContentProps } from "./MenuContent.tsx";

export type DialogMenuContentProps = MenuContentProps;

export function DialogMenuContent({ className = "", ...props }: DialogMenuContentProps) {
  return <MenuContent {...props} className={["dialogSelectContent", className].filter(Boolean).join(" ")} />;
}
