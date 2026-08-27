import { FilePlus, FileText, FolderOpen, FolderPlus, Pencil, Trash2 } from "lucide-react";
import type { ComponentType, ReactNode } from "react";

import { actionLabels } from "../../lib/action-labels.ts";

export type FileTreeEntry = {
  name: string;
  kind: "file" | "folder" | string;
  path?: string;
};

export type FileTreeMenuComponents = {
  Item: ComponentType<{ className?: string; disabled?: boolean; onSelect?: () => void; children?: ReactNode }>;
  Separator: ComponentType<{ className?: string }>;
};

export type FileTreeContextMenuItemsProps = {
  Menu: FileTreeMenuComponents;
  entry: FileTreeEntry | null;
  readOnly?: boolean;
  onNewFile: () => void;
  onNewFolder: () => void;
  onReveal: () => void;
  onRename: () => void;
  onDelete: () => void;
};


export function FileTreeContextMenuItems({
  Menu,
  entry,
  readOnly = false,
  onNewFile,
  onNewFolder,
  onReveal,
  onRename,
  onDelete,
}: FileTreeContextMenuItemsProps) {
  const canCreateChildren = !entry || entry.kind === "folder";
  const showEntryActions = Boolean(entry);
  const showWriteActions = !readOnly;
  return (
    <>
      {showWriteActions && canCreateChildren && (
        <>
          <Menu.Item className="skillMenuItem" onSelect={onNewFile}>
            <FilePlus size={14} />
            New file
          </Menu.Item>
          <Menu.Item className="skillMenuItem" onSelect={onNewFolder}>
            <FolderPlus size={14} />
            New folder
          </Menu.Item>
          {showEntryActions && <Menu.Separator className="skillMenuSeparator" />}
        </>
      )}
      {showEntryActions && (
        <>
          <Menu.Item className="skillMenuItem" disabled={!entry?.path} onSelect={onReveal}>
            <FolderOpen size={14} />
            {actionLabels.revealInFinder}
          </Menu.Item>
          {showWriteActions && (
            <>
              <Menu.Item className="skillMenuItem" onSelect={onRename}>
                <Pencil size={14} />
                Rename
              </Menu.Item>
              <Menu.Item className="skillMenuItem danger" onSelect={onDelete}>
                <Trash2 size={14} />
                Delete
              </Menu.Item>
            </>
          )}
        </>
      )}
    </>
  );
}
