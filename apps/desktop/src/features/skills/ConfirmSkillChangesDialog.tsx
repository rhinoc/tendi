import { Tooltip } from "../../components/shared/Tooltip.tsx";
import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, FileText, Folder } from "lucide-react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";
import { Dialog } from "radix-ui";

import { buildFileTreeRows, displayFileName, formatUserPath, isJsonPath, isYamlPath, SkillChangeCommand } from "../../lib/index.ts";
import { CodeMirrorFileEditor } from "../../components/shared/CodeMirrorFileEditor.tsx";
import { DialogActionButton } from "../../components/shared/DialogActionButton.tsx";
import { DialogShell } from "../../components/shared/DialogShell.tsx";
import { DialogStatefulButton } from "../../components/shared/DialogStatefulButton.tsx";
import { LoadingState } from "../../components/shared/LoadingState.tsx";
import { ResizeSeparator } from "../../components/shared/ResizeSeparator.tsx";
import { Toast } from "../../components/shared/Toast.tsx";

const KEEP_LOCAL_RESOLUTION = "__tendi_keep_local__";
const USE_UPDATE_RESOLUTION = "__tendi_use_update__";
const CONFLICT_MARKER_PATTERN = /^(?:<{7} |={7}$|>{7} |\|{7} )/m;

export type ConfirmSkillChangesDialogProps = {
  open: boolean;
  command: SkillChangeCommand | null;
  names?: string[];
  preview?: Record<string, unknown> | null;
  previewError?: string;
  applyError?: string;
  busy?: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: (resolutions?: Record<string, string>) => void;
};

function skillChangeDescription(command: SkillChangeCommand | null) {
  if (command === SkillChangeCommand.DeleteMany) return "Delete the selected skills from their installed locations.";
  if (command === SkillChangeCommand.UpdateMany) return "Apply available updates for the selected skills.";
  if (command === SkillChangeCommand.Set) return "Apply the selected visibility change.";
  return "Apply the selected skill change.";
}

function skillChangeTitle(command: SkillChangeCommand | null) {
  return command === SkillChangeCommand.DeleteMany ? "Delete selected skills?" : "Confirm skill changes";
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

type UpdateFile = {
  path: string;
  resolutionKey: string;
  before: string;
  base: string;
  incoming: string;
  after: string;
  status: string;
};

function normalizeUpdateFile(value: unknown): UpdateFile | null {
  if (!value || typeof value !== "object") return null;
  const file = value as {
    path?: unknown;
    resolution_key?: unknown;
    resolutionKey?: unknown;
    before?: unknown;
    base?: unknown;
    incoming?: unknown;
    after?: unknown;
    status?: unknown;
  };
  const path = `${file.path ?? ""}`;
  if (!path) return null;
  return {
    path,
    resolutionKey: `${file.resolution_key ?? file.resolutionKey ?? path}`,
    before: typeof file.before === "string" ? file.before : "",
    base: typeof file.base === "string" ? file.base : "",
    incoming: typeof file.incoming === "string" ? file.incoming : "",
    after: typeof file.after === "string" ? file.after : "",
    status: `${file.status ?? "remote"}`,
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
      const targets = (update as { materialized_targets?: unknown }).materialized_targets;
      const targetFiles = Array.isArray(targets)
        ? targets.flatMap((target) => {
          if (!target || typeof target !== "object") return [];
          const files = (target as { files?: unknown }).files;
          return Array.isArray(files) ? files : [];
        })
        : [];
      return [...(Array.isArray(files) ? files : []), ...targetFiles];
    })
    : [];
  const mergeIssues = (plan?.merge_issues as unknown);
  const files = [...gitFiles, ...(Array.isArray(fileChanges) ? fileChanges : [])]
    .map(normalizeUpdateFile)
    .filter((file): file is UpdateFile => file !== null);
  const issues = (Array.isArray(mergeIssues) ? mergeIssues : [])
    .map(normalizeUpdateFile)
    .filter((file): file is UpdateFile => file !== null);
  return [...new Map([...files, ...issues].map((file) => [file.resolutionKey, file])).values()];
}

function updatePreviewSummary(preview: Record<string, unknown> | null | undefined): string {
  const summary = preview?.summary;
  return typeof summary === "string" && summary.trim() ? summary : "No applicable updates.";
}

function isMergeResolutionStatus(status: string) {
  return status === "conflict" || status === "unavailable" || status === "binary";
}

function hasUnresolvedConflictMarkers(content: string) {
  return CONFLICT_MARKER_PATTERN.test(content);
}

function resolvedUpdateContent(file: UpdateFile, resolutions: Record<string, string>) {
  const resolution = resolutions[file.resolutionKey];
  if (resolution === KEEP_LOCAL_RESOLUTION) return file.before;
  if (resolution === USE_UPDATE_RESOLUTION) return file.incoming;
  return resolution ?? file.after;
}

function SkillUpdateDiffPreview({
  files,
  resolutions,
  onResolve,
}: {
  files: UpdateFile[];
  resolutions: Record<string, string>;
  onResolve: (file: UpdateFile, content: string) => void;
}) {
  const [selectedPath, setSelectedPath] = useState("");
  const [collapsedFolders, setCollapsedFolders] = useState<Set<string>>(() => new Set());
  useEffect(() => {
    setSelectedPath((current) => files.some((file) => file.path === current) ? current : (files[0]?.path ?? ""));
  }, [files]);
  const selected = useMemo(() => files.find((file) => file.path === selectedPath) ?? files[0], [files, selectedPath]);
  const selectedContent = selected && selected.status !== "binary"
    ? resolvedUpdateContent(selected, resolutions)
    : selected && resolutions[selected.resolutionKey] === KEEP_LOCAL_RESOLUTION
      ? selected.before
      : selected && resolutions[selected.resolutionKey] === USE_UPDATE_RESOLUTION
        ? selected.incoming
        : "";
  const selectedIsMergeStatus = selected ? isMergeResolutionStatus(selected.status) : false;
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
                <Tooltip key={file.name} content={formatUserPath(file.name)} onlyWhenTruncated><button
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
                  <span className="skillUpdateTreeName">{formatUserPath(displayFileName(file.name))}</span>
                </button></Tooltip>
              );
            })}
          </nav>
        </Panel>
        <ResizeSeparator className="skillUpdateDiffResizeHandle" />
        <Panel className="skillUpdateDiffEditor" minSize="45%">
          <div className="skillUpdateDiffEditorBody">
            <CodeMirrorFileEditor
              content={selectedContent}
              language={isYamlPath(selected.path) ? "yaml" : isJsonPath(selected.path) ? "json" : undefined}
              originalContent={selected.base}
              showDiff={!selectedIsMergeStatus}
              showConflictMarkers={selectedIsMergeStatus && selected.status !== "binary"}
              readOnly={!selectedIsMergeStatus || selected.status === "binary"}
              onChange={selectedIsMergeStatus && selected.status !== "binary"
                ? (content) => onResolve(selected, content)
                : undefined}
              onConflictResolve={selectedIsMergeStatus && selected.status !== "binary"
                ? (content) => onResolve(selected, content)
                : undefined}
            />
            {selected.status === "binary" ? (
              <div className="skillMergeResolutionBar" role="group" aria-label="Resolve binary skill merge">
                <span>
                  Binary file cannot be merged
                </span>
                <button
                  type="button"
                  onClick={() => onResolve(selected, selected.status === "binary" ? KEEP_LOCAL_RESOLUTION : selected.before)}
                >
                  Keep local
                </button>
                <button
                  type="button"
                  onClick={() => onResolve(selected, selected.status === "binary" ? USE_UPDATE_RESOLUTION : selected.incoming)}
                >
                  Use update
                </button>
              </div>
            ) : null}
          </div>
        </Panel>
      </PanelGroup>
    </div>
  );
}

export function ConfirmSkillChangesDialog({
  open,
  command,
  names = [],
  preview,
  previewError,
  applyError,
  busy,
  onOpenChange,
  onConfirm,
}: ConfirmSkillChangesDialogProps) {
  const previewLoading = command === SkillChangeCommand.UpdateMany && !preview && !previewError;
  const actionLabel = skillConfirmActionLabel(command);
  const busyLabel = skillConfirmBusyLabel(command);
  const dialogError = applyError ?? previewError;
  const files = useMemo(
    () => command === SkillChangeCommand.UpdateMany ? updateFiles(preview) : [],
    [command, preview],
  );
  const [resolutions, setResolutions] = useState<Record<string, string>>({});
  useEffect(() => setResolutions({}), [preview]);
  const unresolvedFiles = files.filter((file) => {
    if (!isMergeResolutionStatus(file.status)) return false;
    const content = resolvedUpdateContent(file, resolutions);
    return resolutions[file.resolutionKey] === undefined
      || (file.status !== "binary" && hasUnresolvedConflictMarkers(content));
  });
  const emptyPreview = command === SkillChangeCommand.UpdateMany && preview && !previewError && files.length === 0;
  return (
    <DialogShell
      open={open}
      onOpenChange={onOpenChange}
      descriptionId="skill-changes-description"
      contentProps={{ "data-update-preview": command === SkillChangeCommand.UpdateMany }}
    >
      <div className="skillChangeDialogBody">
        <Dialog.Title className="confirmDialogTitle">{skillChangeTitle(command)}</Dialog.Title>
        <p id="skill-changes-description" className="confirmDialogDescription">
          {skillChangeDescription(command)}
        </p>
        {previewLoading && <LoadingState className="skillUpdatePreviewLoading" label="Preparing update preview" />}
        {command === SkillChangeCommand.DeleteMany && names.length > 0 && (
          <div className="skillDeleteNames" data-selectable-text>
            {names.map((name) => <span key={name}>{name}</span>)}
          </div>
        )}
        {files.length > 0 && (
          <SkillUpdateDiffPreview
            files={files}
            resolutions={resolutions}
            onResolve={(file, content) => setResolutions((current) => ({ ...current, [file.resolutionKey]: content }))}
          />
        )}
        {emptyPreview && <div className="skillUpdatePreviewEmpty" data-selectable-text>{updatePreviewSummary(preview)}</div>}
      </div>
      {dialogError ? <Toast tone="error" message={dialogError} /> : null}
      <div className="confirmDialogActions">
        <DialogActionButton variant="secondary" disabled={busy} onClick={() => onOpenChange(false)}>Cancel</DialogActionButton>
        <DialogStatefulButton
          state={busy ? "loading" : "idle"}
          loadingLabel={busyLabel}
          variant={command === SkillChangeCommand.DeleteMany ? "danger" : "primary"}
          aria-label={actionLabel}
          onClick={() => onConfirm(resolutions)}
          disabled={previewLoading || Boolean(previewError) || unresolvedFiles.length > 0}
        >
          {actionLabel}
        </DialogStatefulButton>
      </div>
    </DialogShell>
  );
}
