import { Tooltip } from "./Tooltip.tsx";
import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, FileText, Folder } from "lucide-react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";
import { Dialog } from "radix-ui";

import { buildFileTreeRows, displayFileName, isYamlPath, SkillChangeCommand } from "../../lib/index.ts";
import { CodeMirrorFileEditor } from "./CodeMirrorFileEditor.tsx";
import { DialogActionButton } from "./DialogActionButton.tsx";
import { LoadingInline } from "./LoadingInline.tsx";
import { ResizeSeparator } from "./ResizeSeparator.tsx";
import "./confirm-dialog.css";

export type ConfirmSkillChangesDialogProps = {
  open: boolean;
  command: SkillChangeCommand | null;
  preview?: Record<string, unknown> | null;
  previewError?: string;
  applyError?: string;
  busy?: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
  onConfirmRelated?: () => void;
};

function skillChangeDescription(command: SkillChangeCommand | null) {
  if (command === SkillChangeCommand.DeleteMany) return "Delete the selected skills from their installed locations.";
  if (command === SkillChangeCommand.UpdateMany) return "Apply available updates for the selected skills.";
  if (command === SkillChangeCommand.Set) return "Apply the selected visibility change.";
  return "Apply the selected skill change.";
}

function skillConfirmActionLabel(command: SkillChangeCommand | null) {
  if (command === SkillChangeCommand.DeleteMany) return "Delete skills";
  if (command === SkillChangeCommand.UpdateMany) return "Apply updates";
  if (command === SkillChangeCommand.Set) return "Apply visibility";
  if (command === SkillChangeCommand.Wrap) return "Create skill";
  return "Apply changes";
}

function skillConfirmBusyLabel(command: SkillChangeCommand | null) {
  if (command === SkillChangeCommand.DeleteMany) return "Deleting…";
  if (command === SkillChangeCommand.UpdateMany) return "Updating…";
  if (command === SkillChangeCommand.Wrap) return "Creating…";
  return "Applying…";
}

function deleteRelations(preview: Record<string, unknown> | null | undefined, key: "dependencies" | "dependents") {
  const plan = preview?.plan as Record<string, unknown> | undefined;
  const relations = plan?.[key];
  if (!Array.isArray(relations)) return [];
  return relations
    .map((relation) => relation as { name?: unknown; related?: unknown })
    .map((relation) => ({
      name: `${relation.name ?? ""}`,
      related: Array.isArray(relation.related) ? relation.related.map((name) => `${name}`) : [],
    }))
    .filter((relation) => relation.name && relation.related.length > 0);
}

type UpdateFile = { path: string; before: string; after: string };

function normalizeUpdateFile(value: unknown): UpdateFile | null {
  if (!value || typeof value !== "object") return null;
  const file = value as { path?: unknown; before?: unknown; after?: unknown };
  const path = `${file.path ?? ""}`;
  if (!path) return null;
  return {
    path,
    before: typeof file.before === "string" ? file.before : "",
    after: typeof file.after === "string" ? file.after : "",
  };
}

function updateFiles(preview: Record<string, unknown> | null | undefined): UpdateFile[] {
  const plan = preview?.plan as Record<string, unknown> | undefined;
  const updates = plan?.git_updates;
  const fileChanges = (plan?.file_changes as Record<string, unknown> | undefined)?.changes;
  const gitFiles = Array.isArray(updates)
    ? updates.flatMap((update) => {
      if (!update || typeof update !== "object") return [];
      const files = (update as { files?: unknown }).files;
      return Array.isArray(files) ? files : [];
    })
    : [];
  const files = [...gitFiles, ...(Array.isArray(fileChanges) ? fileChanges : [])]
    .map(normalizeUpdateFile)
    .filter((file): file is UpdateFile => file !== null);
  return [...new Map(files.map((file) => [file.path, file])).values()];
}

function updatePreviewSummary(preview: Record<string, unknown> | null | undefined): string {
  const summary = preview?.summary;
  return typeof summary === "string" && summary.trim() ? summary : "No applicable updates.";
}

function SkillUpdateDiffPreview({ files }: { files: UpdateFile[] }) {
  const [selectedPath, setSelectedPath] = useState("");
  const [collapsedFolders, setCollapsedFolders] = useState<Set<string>>(() => new Set());
  useEffect(() => {
    setSelectedPath((current) => files.some((file) => file.path === current) ? current : (files[0]?.path ?? ""));
  }, [files]);
  const selected = useMemo(() => files.find((file) => file.path === selectedPath) ?? files[0], [files, selectedPath]);
  const rows = useMemo(
    () => buildFileTreeRows(
      files.map((file) => ({ name: file.path, kind: "file" })),
      collapsedFolders,
    ),
    [collapsedFolders, files],
  );
  const toggleFolder = (path: string) => {
    setCollapsedFolders((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };
  if (!selected) return null;
  return (
    <div className="skillUpdateDiffFrame">
      <PanelGroup className="skillUpdateDiff" orientation="horizontal">
        <Panel
          className="skillUpdateDiffFilesPanel"
          defaultSize="260px"
          minSize="190px"
          maxSize="520px"
          style={{ overflowX: "hidden", overflowY: "auto" }}
        >
          <nav className="skillUpdateDiffFiles" aria-label="Changed files">
            {rows.map(({ file, depth, isFolder }) => {
              const isCollapsed = collapsedFolders.has(file.name);
              const isActive = !isFolder && file.name === selected.path;
              return (
                <Tooltip key={file.name} content={file.name} onlyWhenTruncated><button
                  type="button"
                  aria-current={isActive ? "true" : undefined}
                  aria-expanded={isFolder ? !isCollapsed : undefined}
                  className={`skillUpdateTreeRow ${isFolder ? "folder" : ""} ${isActive ? "active" : ""}`}
                  key={file.name}
                  onClick={() => {
                    if (isFolder) toggleFolder(file.name);
                    else setSelectedPath(file.name);
                  }}
                  style={{ paddingLeft: `${8 + depth * 16}px` }}
                >
                  <span className="skillUpdateTreeChevron">
                    {isFolder ? (isCollapsed ? <ChevronRight size={12} /> : <ChevronDown size={12} />) : null}
                  </span>
                  {isFolder ? <Folder size={14} /> : <FileText size={14} />}
                  <span className="skillUpdateTreeName">{displayFileName(file.name)}</span>
                </button></Tooltip>
              );
            })}
          </nav>
        </Panel>
        <ResizeSeparator className="skillUpdateDiffResizeHandle" />
        <Panel className="skillUpdateDiffEditor" minSize="45%">
          <CodeMirrorFileEditor
            content={selected.after}
            language={isYamlPath(selected.path) ? "yaml" : undefined}
            originalContent={selected.before}
            showDiff
            readOnly
          />
        </Panel>
      </PanelGroup>
    </div>
  );
}

export function ConfirmSkillChangesDialog({
  open,
  command,
  preview,
  previewError,
  applyError,
  busy,
  onOpenChange,
  onConfirm,
  onConfirmRelated,
}: ConfirmSkillChangesDialogProps) {
  const previewLoading = (command === SkillChangeCommand.UpdateMany || command === SkillChangeCommand.DeleteMany) && !preview && !previewError;
  const actionLabel = skillConfirmActionLabel(command);
  const busyLabel = skillConfirmBusyLabel(command);
  const dependencies = command === SkillChangeCommand.DeleteMany ? deleteRelations(preview, "dependencies") : [];
  const dependents = command === SkillChangeCommand.DeleteMany ? deleteRelations(preview, "dependents") : [];
  const files = useMemo(
    () => command === SkillChangeCommand.UpdateMany ? updateFiles(preview) : [],
    [command, preview],
  );
  const emptyPreview = command === SkillChangeCommand.UpdateMany && preview && !previewError && files.length === 0;
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialogOverlay" />
        <Dialog.Content className="confirmDialogPanel" aria-describedby="skill-changes-description" data-no-drag data-update-preview={command === SkillChangeCommand.UpdateMany} onMouseDown={(event) => event.stopPropagation()}>
          <Dialog.Title className="confirmDialogTitle">Confirm skill changes</Dialog.Title>
          <p id="skill-changes-description" className="confirmDialogDescription">
            {skillChangeDescription(command)}
          </p>
          {previewLoading && <div className="skillUpdatePreviewLoading"><LoadingInline label={command === SkillChangeCommand.DeleteMany ? "Preparing deletion preview" : "Preparing update preview"} /></div>}
          {previewError && <p className="skillUpdatePreviewError" role="alert">{previewError}</p>}
          {applyError && <p className="skillUpdatePreviewError" role="alert">{applyError}</p>}
          {files.length > 0 && <SkillUpdateDiffPreview files={files} />}
          {emptyPreview && <div className="skillUpdatePreviewEmpty" data-selectable-text>{updatePreviewSummary(preview)}</div>}
          {(dependencies.length > 0 || dependents.length > 0) && (
            <div className="confirmDialogImpact" data-selectable-text>
              {dependents.length > 0 && (
                <section>
                  <strong>Used by installed skills</strong>
                  {dependents.map((relation) => (
                    <span key={`dependent-${relation.name}`}>{relation.name}: {relation.related.join(", ")}</span>
                  ))}
                </section>
              )}
              {dependencies.length > 0 && (
                <section>
                  <strong>Depends on installed skills</strong>
                  {dependencies.map((relation) => (
                    <span key={`dependency-${relation.name}`}>{relation.name}: {relation.related.join(", ")}</span>
                  ))}
                </section>
              )}
            </div>
          )}
          <div className="confirmDialogActions">
            <DialogActionButton variant="secondary" disabled={busy} onClick={() => onOpenChange(false)}>Cancel</DialogActionButton>
            {command === SkillChangeCommand.DeleteMany && (dependencies.length > 0 || dependents.length > 0) && onConfirmRelated && (
              <DialogActionButton variant="danger-subtle" disabled={busy} onClick={onConfirmRelated}>
                {busy ? "Deleting…" : "Delete related too"}
              </DialogActionButton>
            )}
            <DialogActionButton variant={command === SkillChangeCommand.DeleteMany ? "danger" : "primary"} disabled={busy || previewLoading || Boolean(previewError)} onClick={onConfirm}>
              {busy ? busyLabel : actionLabel}
            </DialogActionButton>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
