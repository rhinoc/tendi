import { Tooltip } from "../../components/shared/Tooltip.tsx";
import { useEffect, useMemo, useState } from "react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";
import { Dialog } from "radix-ui";

import { buildFileTreeRows, displayFileName, formatUserPath, isJsonPath, isYamlPath, parentPath, skillChangeActionLabel, skillChangeBusyLabel, skillChangeCanConfirm, skillChangeDescription, skillChangeDisabledReason, skillChangeLoadingCopy, skillChangeTitle, SkillChangeCommand } from "../../lib/index.ts";
import { CodeMirrorFileEditor, CodeMirrorLanguage } from "../../components/shared/CodeMirrorFileEditor.tsx";
import { DialogActionButton } from "../../components/shared/DialogActionButton.tsx";
import { DialogApplyButton } from "../../components/shared/DialogApplyButton.tsx";
import { DialogShell } from "../../components/shared/DialogShell.tsx";
import { FileTree } from "../../components/shared/FileTree.tsx";
import { LoadingState } from "../../components/shared/LoadingState.tsx";
import { ResizeSeparator } from "../../components/shared/ResizeSeparator.tsx";
import { Toast } from "../../components/shared/Toast.tsx";
import type { SkillChangeResponse } from "../../lib/runtime-gateway.ts";

import "./ConfirmSkillChangesDialog.css";

const KEEP_LOCAL_RESOLUTION = "__tendi_keep_local__";
const USE_UPDATE_RESOLUTION = "__tendi_use_update__";
const CONFLICT_MARKER_PATTERN = /^(?:<{7} |={7}$|>{7} |\|{7} )/m;

export type ConfirmSkillChangesDialogProps = {
  open: boolean;
  command: SkillChangeCommand | null;
  names?: string[];
  displayNames?: string[];
  preview?: SkillChangeResponse | null;
  previewError?: string;
  applyError?: string;
  busy?: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: (resolutions?: Record<string, string>) => void;
};

export type ConfirmSkillChangesDialogContentProps = Omit<ConfirmSkillChangesDialogProps, "open">;

type UpdateFile = {
  path: string;
  resolutionKey: string;
  before: string;
  base: string;
  incoming: string;
  after: string;
  beforeExists: boolean;
  incomingExists: boolean;
  status: string;
};

type UpdateFileSource = {
  path: string;
  resolution_key?: string;
  before?: string | null;
  base?: string;
  incoming?: string;
  after: string;
  before_exists?: boolean;
  incoming_exists?: boolean;
  status?: string;
};

function normalizeUpdateFile(file: UpdateFileSource, allowPathResolutionKey: boolean): UpdateFile | null {
  const path = file.path;
  if (!path) return null;
  const resolutionKey = file.resolution_key
    ? file.resolution_key
    : allowPathResolutionKey ? path : "";
  if (!resolutionKey) return null;
  return {
    path,
    resolutionKey,
    before: file.before ?? "",
    base: file.base ?? "",
    incoming: file.incoming ?? "",
    after: file.after,
    beforeExists: file.before_exists ?? file.before != null,
    incomingExists: file.incoming_exists ?? file.incoming != null,
    status: file.status ?? "",
  };
}

function updateFiles(preview: SkillChangeResponse | null | undefined): UpdateFile[] {
  const plan = preview?.plan;
  if (!plan) return [];
  const gitFiles = plan.git_updates.flatMap((update) => [
    ...update.files,
    ...update.materialized_targets.flatMap((target) => target.files),
  ]);
  const files = [
    ...gitFiles.map((file) => normalizeUpdateFile(file, false)),
    ...plan.file_changes.changes.map((file) => normalizeUpdateFile(file, true)),
  ]
    .filter((file): file is UpdateFile => file !== null);
  const issues = plan.merge_issues
    .map((file) => normalizeUpdateFile(file, false))
    .filter((file): file is UpdateFile => file !== null);
  return [...new Map([...files, ...issues].map((file) => [file.resolutionKey, file])).values()];
}

function updatePreviewSummary(preview: SkillChangeResponse | null | undefined): string {
  const summary = preview?.summary;
  return typeof summary === "string" && summary.trim() ? summary : "No applicable updates.";
}

function isMergeResolutionStatus(status: string) {
  return status === "conflict" || status === "unavailable" || status === "binary";
}

function isFileLevelResolution(file: UpdateFile) {
  return file.status === "binary"
    || file.status === "unavailable"
    || (file.status === "conflict" && file.beforeExists !== file.incomingExists);
}

function fileResolutionDescription(file: UpdateFile) {
  if (file.beforeExists && !file.incomingExists) {
    return "Modified locally, deleted remotely";
  }
  if (!file.beforeExists && file.incomingExists) {
    return "Deleted locally, modified remotely";
  }
  return null;
}

function hasUnresolvedConflictMarkers(content: string) {
  return CONFLICT_MARKER_PATTERN.test(content);
}

function hasFileDiff(file: UpdateFile) {
  if (isMergeResolutionStatus(file.status)) return true;
  if (file.status && file.status !== "unchanged") return true;
  return file.before !== file.after;
}

function resolvedUpdateContent(file: UpdateFile, resolutions: Record<string, string>) {
  const resolution = resolutions[file.resolutionKey];
  if (resolution === KEEP_LOCAL_RESOLUTION) return file.before;
  if (resolution === USE_UPDATE_RESOLUTION) return file.incoming;
  return resolution ?? file.after;
}

function isUnresolvedFile(file: UpdateFile, resolutions: Record<string, string>) {
  if (!isMergeResolutionStatus(file.status)) return false;
  const content = resolvedUpdateContent(file, resolutions);
  return resolutions[file.resolutionKey] === undefined
    || (file.status !== "binary" && hasUnresolvedConflictMarkers(content));
}

function SkillUpdateDiffPreview({
  files,
  resolutions,
  unresolvedFiles,
  resolutionCount,
  onResolve,
  onResolveAll,
}: {
  files: UpdateFile[];
  resolutions: Record<string, string>;
  unresolvedFiles: UpdateFile[];
  resolutionCount: number;
  onResolve: (file: UpdateFile, content: string) => void;
  onResolveAll: (content: string) => void;
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
  const selectedIsFileLevelResolution = selected ? isFileLevelResolution(selected) : false;
  const selectedFileResolutionDescription = selected ? fileResolutionDescription(selected) : null;
  const rows = useMemo(
    () => buildFileTreeRows(
      files.map((file) => ({ name: file.path, kind: "file" })),
      collapsedFolders,
    ),
    [collapsedFolders, files],
  );
  const filesByPath = useMemo(
    () => new Map(files.map((file) => [file.path, file])),
    [files],
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
  const items = rows.map(({ file, depth, isFolder }, index) => {
    return {
      id: file.name,
      label: formatUserPath(displayFileName(file.name)),
      kind: isFolder ? "folder" as const : "file" as const,
      depth,
      parentId: depth > 0 ? parentPath(file.name) : null,
      expanded: isFolder && !collapsedFolders.has(file.name),
      position: index + 1,
    };
  });
  return (
    <div className="skillUpdateDiffFrame">
      {unresolvedFiles.length > 0 ? (
        <div className="skillUpdateDiffToolbar" role="toolbar" aria-label="Bulk conflict resolution">
          <span className="skillUpdateDiffToolbarLabel">
            {unresolvedFiles.length}/{resolutionCount} conflicts to resolve
          </span>
          <div className="skillUpdateDiffToolbarActions">
            <DialogActionButton variant="secondary" onClick={() => onResolveAll(KEEP_LOCAL_RESOLUTION)}>
              Keep all local
            </DialogActionButton>
            <DialogActionButton variant="secondary" onClick={() => onResolveAll(USE_UPDATE_RESOLUTION)}>
              Use all updates
            </DialogActionButton>
          </div>
        </div>
      ) : null}
      <PanelGroup className="skillUpdateDiff" orientation="horizontal">
        <Panel
          className="skillUpdateDiffFilesPanel"
          defaultSize="260px"
          minSize="190px"
          maxSize="520px"
          style={{ overflow: "hidden" }}
        >
          <FileTree
            items={items}
            selectedId={selected.path}
            onItemActivate={(item) => {
              if (item.kind === "folder") toggleFolder(item.id);
              else setSelectedPath(item.id);
            }}
            onItemToggle={(item) => {
              if (item.kind === "folder") toggleFolder(item.id);
            }}
            renderItem={(item, row) => (
              <Tooltip content={formatUserPath(item.id)} onlyWhenTruncated>{row}</Tooltip>
            )}
            renderTrailing={(item) => {
              const updateFile = filesByPath.get(item.id);
              if (!updateFile || !hasFileDiff(updateFile)) return null;
              const needsResolution = isMergeResolutionStatus(updateFile.status);
              return (
                <span className={`skillUpdateFileStatus ${needsResolution ? "needsResolution" : "changed"}`}>
                  {needsResolution ? "!" : "M"}
                </span>
              );
            }}
            showHeader={false}
            className="skillUpdateFileTree"
            ariaLabel="Changed files"
          />
        </Panel>
        <ResizeSeparator className="skillUpdateDiffResizeHandle" />
        <Panel className="skillUpdateDiffEditor" minSize="45%">
          <div className="skillUpdateDiffEditorBody">
            <CodeMirrorFileEditor
              content={selectedContent}
              language={isYamlPath(selected.path) ? CodeMirrorLanguage.Yaml : isJsonPath(selected.path) ? CodeMirrorLanguage.Json : undefined}
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
            {selectedIsFileLevelResolution ? (
              <div className="skillMergeResolutionBar" role="group" aria-label="Resolve file conflict">
                {selectedFileResolutionDescription ? <span>{selectedFileResolutionDescription}</span> : null}
                <button
                  type="button"
                  onClick={() => onResolve(selected, KEEP_LOCAL_RESOLUTION)}
                >
                  Keep local
                </button>
                <button
                  type="button"
                  onClick={() => onResolve(selected, USE_UPDATE_RESOLUTION)}
                >
                  {selected.incomingExists ? "Use update" : "Delete file"}
                </button>
              </div>
            ) : null}
          </div>
        </Panel>
      </PanelGroup>
    </div>
  );
}

export function ConfirmSkillChangesDialogContent({
  command,
  names = [],
  displayNames = names,
  preview,
  previewError,
  applyError,
  busy,
  onOpenChange,
  onConfirm,
}: ConfirmSkillChangesDialogContentProps) {
  const previewLoading = command === SkillChangeCommand.UpdateMany && !preview && !previewError;
  const actionLabel = skillChangeActionLabel(command);
  const busyLabel = skillChangeBusyLabel(command);
  const dialogError = applyError ?? previewError;
  const files = useMemo(
    () => command === SkillChangeCommand.UpdateMany ? updateFiles(preview) : [],
    [command, preview],
  );
  const [resolutions, setResolutions] = useState<Record<string, string>>({});
  const previewIdentity = typeof preview?.previewId === "string" ? preview.previewId : "";
  useEffect(() => setResolutions({}), [command, previewIdentity]);
  const unresolvedFiles = files.filter((file) => isUnresolvedFile(file, resolutions));
  const resolutionCount = files.filter((file) => isMergeResolutionStatus(file.status)).length;
  const canApply = skillChangeCanConfirm(command, {
    previewLoading,
    previewError,
    canApply: preview?.canApply as boolean | undefined,
    unresolvedFiles: unresolvedFiles.length,
  });
  const applyDisabledReason = skillChangeDisabledReason(command, {
    previewLoading,
    previewError,
    canApply: preview?.canApply as boolean | undefined,
    unresolvedFiles: unresolvedFiles.length,
  });
  const emptyPreview = command === SkillChangeCommand.UpdateMany && preview && !previewError && files.length === 0;
  return (
    <>
      <div className="skillChangeDialogBody">
        <Dialog.Title className="confirmDialogTitle">{skillChangeTitle(command)}</Dialog.Title>
        <p id="skill-changes-description" className="confirmDialogDescription">
          {skillChangeDescription(command)}
        </p>
        {previewLoading && <LoadingState className="skillUpdatePreviewLoading" label={skillChangeLoadingCopy.previewLabel} />}
        {command === SkillChangeCommand.DeleteMany && names.length > 0 && (
          <div className="skillDeleteNames" data-selectable-text>
            {displayNames.map((name) => <span key={name}>{name}</span>)}
          </div>
        )}
        {files.length > 0 && (
          <SkillUpdateDiffPreview
            files={files}
            resolutions={resolutions}
            unresolvedFiles={unresolvedFiles}
            resolutionCount={resolutionCount}
            onResolve={(file, content) => setResolutions((current) => ({ ...current, [file.resolutionKey]: content }))}
            onResolveAll={(content) => setResolutions((current) => {
              const next = { ...current };
              for (const file of unresolvedFiles) next[file.resolutionKey] = content;
              return next;
            })}
          />
        )}
        {emptyPreview && <div className="skillUpdatePreviewEmpty" data-selectable-text>{updatePreviewSummary(preview)}</div>}
      </div>
      {dialogError ? <Toast tone="error" message={dialogError} /> : null}
      <div className="confirmDialogActions">
        <DialogActionButton variant="secondary" disabled={busy} onClick={() => onOpenChange(false)}>Cancel</DialogActionButton>
        <Tooltip content={canApply ? "" : applyDisabledReason}>
          <span
            className="skillUpdateApplyTooltipTarget"
            tabIndex={canApply ? undefined : 0}
          >
            <DialogApplyButton
              label={actionLabel}
              busy={busy}
              busyLabel={busyLabel}
              ariaLabel={actionLabel}
              autoFocus={command === SkillChangeCommand.DeleteMany && !busy && canApply}
              expandOnFocus={command !== SkillChangeCommand.DeleteMany}
              onClick={() => onConfirm(resolutions)}
              disabled={!canApply}
            />
          </span>
        </Tooltip>
      </div>
    </>
  );
}

export function ConfirmSkillChangesDialog({
  open,
  onOpenChange,
  ...contentProps
}: ConfirmSkillChangesDialogProps) {
  return (
    <DialogShell
      open={open}
      onOpenChange={onOpenChange}
      descriptionId="skill-changes-description"
      contentProps={{ "data-update-preview": contentProps.command === SkillChangeCommand.UpdateMany }}
    >
      <ConfirmSkillChangesDialogContent onOpenChange={onOpenChange} {...contentProps} />
    </DialogShell>
  );
}
