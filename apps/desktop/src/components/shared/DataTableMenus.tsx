import type { ComponentType, ReactNode } from "react";
import { Copy, Trash2 } from "lucide-react";

import { TauriCommand, copyText, safeInvoke } from "../../lib/index.ts";

export type DataTableMenuComponents = {
  Item: ComponentType<{
    className?: string;
    disabled?: boolean;
    key?: string;
    onSelect?: () => void;
    children?: ReactNode;
  }>;
  Separator: ComponentType<{ className?: string }>;
};

export function RevealInFinderMenuItem({
  Menu,
  path,
  label = "Reveal in Finder",
}: {
  Menu: DataTableMenuComponents;
  path?: string | null;
  label?: string;
}) {
  const resolved = `${path ?? ""}`.trim();
  return (
    <Menu.Item
      className="skillMenuItem"
      disabled={!resolved}
      onSelect={() => resolved && safeInvoke(TauriCommand.RevealInFinder, { path: resolved })}
    >
      {label}
    </Menu.Item>
  );
}

export function CopyPathMenuItem({
  Menu,
  path,
  label = "Copy path",
}: {
  Menu: DataTableMenuComponents;
  path?: string | null;
  label?: string;
}) {
  const resolved = `${path ?? ""}`.trim();
  return (
    <Menu.Item className="skillMenuItem" disabled={!resolved} onSelect={() => resolved && copyText(resolved)}>
      {label}
    </Menu.Item>
  );
}

export function CopyTextMenuItem({
  Menu,
  text,
  label,
  disabled = false,
}: {
  Menu: DataTableMenuComponents;
  text: string;
  label: string;
  disabled?: boolean;
}) {
  return (
    <Menu.Item className="skillMenuItem" disabled={disabled || !text} onSelect={() => copyText(text)}>
      {label}
    </Menu.Item>
  );
}

export function DeleteMenuItem({
  Menu,
  label,
  onSelect,
  disabled = false,
}: {
  Menu: DataTableMenuComponents;
  label: string;
  onSelect: () => void;
  disabled?: boolean;
}) {
  return (
    <Menu.Item className="skillMenuItem danger" disabled={disabled} onSelect={onSelect}>
      <Trash2 size={14} />
      {label}
    </Menu.Item>
  );
}

export function BulkDeleteMenuItem({
  Menu,
  label = "Delete selected",
  onSelect,
  disabled = false,
}: {
  Menu: DataTableMenuComponents;
  label?: string;
  onSelect: () => void;
  disabled?: boolean;
}) {
  return (
    <Menu.Item className="skillMenuItem danger" disabled={disabled} onSelect={onSelect}>
      <Trash2 size={14} />
      {label}
    </Menu.Item>
  );
}
