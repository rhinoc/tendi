import { Tooltip } from "../../components/shared/Tooltip.tsx";
import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, FileText, Folder } from "lucide-react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";
import { Dialog } from "radix-ui";

import { buildFileTreeRows, displayFileName, formatUserPath, isJsonPath, isYamlPath, skillChangeActionLabel, skillChangeBusyLabel, skillChangeCanConfirm, skillChangeDescription, skillChangeDisabledReason, skillChangeLoadingCopy, skillChangeTitle, SkillChangeCommand } from "../../lib/index.ts";
import { CodeMirrorFileEditor, CodeMirrorLanguage } from "../../components/shared/CodeMirrorFileEditor.tsx";
import { DialogActionButton } from "../../components/shared/DialogActionButton.tsx";
import { DialogShell } from "../../components/shared/DialogShell.tsx";
import { DialogStatefulButton } from "../../components/shared/DialogStatefulButton.tsx";
import { LoadingState } from "../../components/shared/LoadingState.tsx";
import { ResizeSeparator } from "../../components/shared/ResizeSeparator.tsx";
import { Toast } from "../../components/shared/Toast.tsx";
import { AsyncStatus } from "../../lib/async-status.ts";
import type { SkillChangeResponse } from "../../lib/runtime-gateway.ts";

const KEEP_LOCAL_RESOLUTION = "__tendi_keep_local__";
const USE_UPDATE_RESOLUTION = "__tendi_use_update__";
const CONFLICT_MARKER_PATTERN = /^(?:<{7} |={7}$|>{7} |\|{7} )/m;

export type ConfirmSkillChangesDialogProps = {
  open: boolean;
  command: SkillChangeCommand | null;
  names?: string[];
  preview?: SkillChangeResponse | null;
  previewError?: string;
  applyError?: string;
  busy?: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: (resolutions?: Record<string, string>) => void;
};

type UpdateFile = {
  path: string;
  resolutionKey: string;
  before: string;
  base: string;
  incoming: string;
  after: string;
  status: string;
};

type UpdateFileSource = {
  path: string;
  resolution_key?: string;
  before?: string | null;
  base?: string;
  incoming?: string;
  after: string;
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
              const updateFile = isFolder ? undefined : filesByPath.get(file.name);
              const isChanged = updateFile ? hasFileDiff(updateFile) : false;
              const needsResolution = updateFile ? isMergeResolutionStatus(updateFile.status) : false;
              return (
                <Tooltip key={file.name} content={formatUserPath(file.name)} onlyWhenTruncated><button
                  type="button"
                  aria-current={isActive ? "true" : undefined}
                  aria-expanded={isFolder ? !isCollapsed : undefined}
                  className={`skillUpdateTreeRow ${isFolder ? "folder" : ""} ${isChanged ? "changed" : ""} ${needsResolution ? "needsResolution" : ""} ${isActive ? "active" : ""}`}
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
                  {isChanged ? (
                    <span className={`skillUpdateFileStatus ${needsResolution ? "needsResolution" : "changed"}`}>
                      {needsResolution ? "!" : "M"}
                    </span>
                  ) : null}
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
  const unresolvedFiles = files.filter((file) => {
    if (!isMergeResolutionStatus(file.status)) return false;
    const content = resolvedUpdateContent(file, resolutions);
    return resolutions[file.resolutionKey] === undefined
      || (file.status !== "binary" && hasUnresolvedConflictMarkers(content));
  });
  const resolutionFiles = files.filter((file) => isMergeResolutionStatus(file.status));
  const resolvedFiles = resolutionFiles.length - unresolvedFiles.length;
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
        {previewLoading && <LoadingState className="skillUpdatePreviewLoading" label={skillChangeLoadingCopy.previewLabel} />}
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
        {resolutionFiles.length > 0 ? (
          <span className="skillUpdateResolutionProgress" role="status" aria-live="polite">
            {resolvedFiles}/{resolutionFiles.length} resolved
          </span>
        ) : null}
        <Tooltip content={canApply ? "" : applyDisabledReason}>
          <span
            className="skillUpdateApplyTooltipTarget"
            tabIndex={canApply ? undefined : 0}
          >
            <DialogStatefulButton
              state={busy ? AsyncStatus.Loading : AsyncStatus.Idle}
              loadingLabel={busyLabel}
              variant={command === SkillChangeCommand.DeleteMany ? "danger" : "primary"}
              aria-label={actionLabel}
              onClick={() => onConfirm(resolutions)}
              disabled={!canApply}
            >
              {actionLabel}
            </DialogStatefulButton>
          </span>
        </Tooltip>
      </div>
    </DialogShell>
  );
}
