import type { ComponentType, ReactNode } from "react";
import { Code2, Copy, FolderOpen, Trash2 } from "lucide-react";

import { actionLabels, TauriCommand, copyText, safeInvoke } from "../../lib/index.ts";

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

export function OpenInEditorMenuItem({
  Menu,
  path,
  line,
  label = actionLabels.openInEditor,
}: {
  Menu: DataTableMenuComponents;
  path?: string | null;
  line?: number | null;
  label?: string;
}) {
  const resolved = `${path ?? ""}`.trim();
  return (
    <Menu.Item
      className="skillMenuItem"
      disabled={!resolved}
      onSelect={() => resolved && safeInvoke(TauriCommand.OpenInEditor, { path: resolved, line: line ?? undefined })}
    >
      <Code2 size={14} />
      {label}
    </Menu.Item>
  );
}

export function RevealInFinderMenuItem({
  Menu,
  path,
  label = actionLabels.revealInFinder,
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
      <FolderOpen size={14} />
      {label}
    </Menu.Item>
  );
}

export function CopyPathMenuItem({
  Menu,
  path,
  label = actionLabels.copyPath,
}: {
  Menu: DataTableMenuComponents;
  path?: string | null;
  label?: string;
}) {
  const resolved = `${path ?? ""}`.trim();
  return (
    <Menu.Item className="skillMenuItem" disabled={!resolved} onSelect={() => resolved && copyText(resolved)}>
      <Copy size={14} />
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
      <Copy size={14} />
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
  label = actionLabels.deleteSelected,
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
