import { Tooltip } from "../../components/shared/Tooltip.tsx";
import { Badge } from "../../components/shared/Badge.tsx";
import { lazy, Suspense, useCallback, useDeferredValue, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  FilePlus,
  FileText,
  Folder,
  FolderPlus,
  Waypoints,
} from "lucide-react";
import { Group as PanelGroup, Panel, usePanelRef } from "react-resizable-panels";
import { ContextMenu } from "radix-ui";
import {
  buildFileTreeRows,
  diffPreview,
  displayFileName,
  isReadOnlySkillSource,
  joinRelativePath,
  normalizeSessionSkillLink,
  parentPath,
  preferredSkillFileName,
  dialogCopy,
  safeInvoke,
  TauriCommand,
  uniqueChildPath,
  type SkillFileEntry,
  type NormalizedSkill,
  type RawSkillRecord,
  type SessionSkillLinkRecord,
} from "../../lib/index.ts";
import { DiffLineKind } from "../../lib/diff.ts";
import { SaveStatus } from "../../lib/save-status.ts";
import {
  createSkillFile,
  createSkillFolder,
  deleteSkillPath,
  invokeSkillSessionLinks,
  readSkillFile,
  readSkillFiles,
  renameSkillPath,
  saveSkillFile,
  type SkillFileMutationResponse,
  type SkillFileReadResponse,
} from "../../lib/runtime-gateway.ts";
import type { SkillIndexStatus } from "../../store/desktop-store.ts";
import { EditorHeader } from "../../components/shared/EditorHeader.tsx";
import { DialogLoadingFallback } from "../../components/shared/DialogLoadingFallback.tsx";
import { DialogActionButton } from "../../components/shared/DialogActionButton.tsx";
import { DeleteConfirmationDialog } from "../../components/shared/DeleteConfirmationDialog.tsx";
import { EditorStatePlaceholder } from "../../components/shared/EditorStatePlaceholder.tsx";
import { FileTreeContextMenuItems } from "./FileTreeContextMenuItems.tsx";
import { IconButton } from "../../components/shared/IconButton.tsx";
import { LoadingState } from "../../components/shared/LoadingState.tsx";
import { MarkdownFilePane, type DiffStats } from "../../components/shared/MarkdownFilePane.tsx";
import { ResizeSeparator } from "../../components/shared/ResizeSeparator.tsx";
import { Toast } from "../../components/shared/Toast.tsx";
import type { SkillDependencyRecord } from "./SkillDependencyGraph.tsx";
import { SkillInfoMenu } from "./SkillInfoMenu.tsx";
import { LinkedSessionsDrawerFallback } from "../sessions/LinkedSessionsDrawerFallback.tsx";

const DiscardChangesDialog = lazy(() => import("../../components/shared/DiscardChangesDialog.tsx").then(({ DiscardChangesDialog: component }) => ({ default: component })));
const LinkedSessionsDrawer = lazy(() => import("../sessions/linked-sessions.tsx").then(({ LinkedSessionsDrawer: component }) => ({ default: component })));

export type SkillEditorViewProps = {
  skill: NormalizedSkill;
  skills: SkillDependencyRecord[];
  back: () => void;
  onReadSkillIndexStatus?: () => Promise<SkillIndexStatus | null>;
  skillIndexStatus?: SkillIndexStatus | null;
  onOpenSession?: (link: Record<string, unknown>) => void;
  onOpenSkill?: (name: string) => void;
  onSaved?: (skills?: RawSkillRecord[]) => void;
};

type SkillDraft = {
  content: string;
  originalContent: string;
  sha256: string;
};

const MAX_CLEAN_DRAFT_CHARS = 32 * 1024 * 1024;

function trimCleanDrafts(drafts: Record<string, SkillDraft>, activePath: string) {
  let cleanChars = 0;
  const evictable: Array<[string, SkillDraft]> = [];
  for (const [path, draft] of Object.entries(drafts)) {
    if (path === activePath || draft.content !== draft.originalContent) continue;
    cleanChars += draft.content.length;
    evictable.push([path, draft]);
  }
  if (cleanChars <= MAX_CLEAN_DRAFT_CHARS) return drafts;

  const next = { ...drafts };
  for (const [path, draft] of evictable) {
    if (cleanChars <= MAX_CLEAN_DRAFT_CHARS) break;
    delete next[path];
    cleanChars -= draft.content.length;
  }
  return next;
}

export function SkillEditorView({ skill, skills, back, onReadSkillIndexStatus, skillIndexStatus, onOpenSession, onOpenSkill, onSaved }: SkillEditorViewProps) {
  const currentSkill = skill;
  const skillPath = currentSkill.paths.find((path) => path.path)?.path;
  const readOnly = isReadOnlySkillSource(currentSkill);
  const [files, setFiles] = useState<SkillFileEntry[]>([]);
  const [linkedSessions, setLinkedSessions] = useState<SessionSkillLinkRecord[]>([]);
  const [loadingLinkedSessions, setLoadingLinkedSessions] = useState(false);
  const [linkedSessionsError, setLinkedSessionsError] = useState("");
  const [showLinkedSessions, setShowLinkedSessions] = useState(false);
  const [activePath, setActivePath] = useState("");
  const [selectedPath, setSelectedPath] = useState("");
  const [drafts, setDrafts] = useState<Record<string, SkillDraft>>({});
  const [createdPaths, setCreatedPaths] = useState(() => new Set<string>());
  const [fileTreeCollapsed, setFileTreeCollapsed] = useState(false);
  const [collapsedFolders, setCollapsedFolders] = useState(() => new Set<string>());
  const [showDiscardDialog, setShowDiscardDialog] = useState(false);
  const [pendingDeleteEntry, setPendingDeleteEntry] = useState<SkillFileEntry | null>(null);
  const [deletingEntry, setDeletingEntry] = useState(false);
  const [renamingPath, setRenamingPath] = useState("");
  const [renameValue, setRenameValue] = useState("");
  const [loadingFiles, setLoadingFiles] = useState(true);
  const [fileError, setFileError] = useState("");
  const [loadingContent, setLoadingContent] = useState(false);
  const [contentError, setContentError] = useState("");
  const [saveState, setSaveState] = useState<SaveStatus>(SaveStatus.Idle);
  const [saveError, setSaveError] = useState("");
  const renameInputRef = useRef<HTMLInputElement>(null);
  const linkedSessionsRequestRef = useRef(0);
  const linkedSessionsSkillRef = useRef(currentSkill.name);
  const fileTreePanelRef = usePanelRef();
  const activeDraft = drafts[activePath] ?? { content: "", originalContent: "", sha256: "" };
  const content = activeDraft.content;
  const deferredContent = useDeferredValue(content);
  const contentReady = Boolean(drafts[activePath]);
  const original = { content: activeDraft.originalContent, sha256: activeDraft.sha256 };
  const dirty = !readOnly && content !== original.content;
  const hasUnsavedDrafts = useMemo(
    () => !readOnly && Object.values(drafts).some((draft) => draft.content !== draft.originalContent),
    [drafts, readOnly],
  );
  useEffect(() => {
    let cancelled = false;
    async function loadFiles() {
      setLoadingFiles(true);
      setFileError("");
      setFiles([]);
      setActivePath("");
      setSelectedPath("");
      setDrafts({});
      setCreatedPaths(new Set());
      setRenamingPath("");
      setCollapsedFolders(new Set());
      setShowDiscardDialog(false);
      setLoadingContent(true);
      const fileListRead = readSkillFiles({
        skillId: currentSkill.id,
        skillPath,
      });
      const initialContentRead = readSkillFile({
        skillId: currentSkill.id,
        relativePath: "SKILL.md",
        skillPath,
      });
      const [fileListResult, initialContentResult] = await Promise.allSettled([fileListRead, initialContentRead]);
      if (fileListResult.status === "rejected") {
        if (!cancelled) {
          const error = fileListResult.reason;
          setFileError(error instanceof Error ? error.message : `${error}`);
          setLoadingFiles(false);
          setLoadingContent(false);
        }
        return;
      }
      if (cancelled) return;
      const next = [...fileListResult.value];
      const firstFile = preferredSkillFileName(next);
      setFiles(next);
      setActivePath(firstFile ?? "");
      setSelectedPath(firstFile ?? "");
      if (firstFile &&
        initialContentResult.status === "fulfilled"
        && typeof initialContentResult.value.content === "string"
        && typeof initialContentResult.value.sha256 === "string"
      ) {
        const fileContent = initialContentResult.value.content;
        const fileHash = initialContentResult.value.sha256;
        setDrafts((current) => trimCleanDrafts({
          ...current,
          "SKILL.md": {
            content: fileContent,
            originalContent: fileContent,
            sha256: fileHash,
          },
        }, firstFile));
      }
      setLoadingFiles(false);
      setLoadingContent(false);
    }
    void loadFiles();
    return () => {
      cancelled = true;
    };
  }, [currentSkill.name, skillPath]);

  useEffect(() => {
    if (renamingPath) {
      requestAnimationFrame(() => {
        renameInputRef.current?.focus();
        renameInputRef.current?.select();
      });
    }
  }, [renamingPath]);

  useEffect(() => {
    setSaveState(SaveStatus.Idle);
    setSaveError("");
    setContentError("");
  }, [activePath]);

  useEffect(() => {
    if (loadingFiles || drafts[activePath]) return undefined;
    let cancelled = false;
    async function loadContent() {
      setLoadingContent(true);
      setContentError("");
      const fileRead = readSkillFile({
        skillId: currentSkill.id,
        relativePath: activePath,
        skillPath,
      });
      let result: SkillFileReadResponse;
      try {
        result = await fileRead;
      } catch (error) {
        if (!cancelled) {
          setContentError(error instanceof Error ? error.message : `${error}`);
          setLoadingContent(false);
        }
        return;
      }
      if (cancelled) return;
      if (typeof result?.content === "string" && typeof result.sha256 === "string") {
        const fileContent = result.content;
        const fileHash = result.sha256;
        setDrafts((current) => trimCleanDrafts({
          ...current,
          [activePath]: {
            content: fileContent,
            originalContent: fileContent,
            sha256: fileHash,
          },
        }, activePath));
      }
      setLoadingContent(false);
    }
    void loadContent();
    return () => {
      cancelled = true;
      setLoadingContent(false);
    };
  }, [activePath, currentSkill.name, drafts, loadingFiles, skillPath]);

  const save = useCallback(async () => {
    if (readOnly || !dirty || !original.sha256 || saveState === SaveStatus.Saving) return;
    setSaveState(SaveStatus.Saving);
    setSaveError("");
    try {
      const result = await saveSkillFile({
        skillId: currentSkill.id,
        relativePath: activePath,
        expectedSha256: original.sha256,
        content,
      });
      if (typeof result.sha256 !== "string") {
        throw new Error("Save returned incomplete file metadata");
      }
      const savedSha256 = result.sha256;
      setDrafts((current) => ({
        ...current,
        [activePath]: { content, originalContent: content, sha256: savedSha256 },
      }));
      setSaveState(SaveStatus.Saved);
      onSaved?.(result.skills);
    } catch (error) {
      const message = error instanceof Error ? error.message : `${error}`;
      setSaveError(message);
      setSaveState(SaveStatus.Error);
    }
  }, [activePath, content, currentSkill.name, dirty, onSaved, original.sha256, readOnly, saveState]);
  const rows = useMemo(() => buildFileTreeRows(files, collapsedFolders), [collapsedFolders, files]);
  const selectedEntry = useMemo(
    () => files.find((file) => file.name === selectedPath) ?? files.find((file) => file.name === activePath) ?? null,
    [activePath, files, selectedPath],
  );
  const diffLines = useMemo(
    () => !contentReady || !dirty ? [] : diffPreview(original.content, deferredContent),
    [contentReady, deferredContent, dirty, original.content],
  );
  const diffStats = useMemo(
    () => diffLines.reduce(
      (counts: DiffStats, line: { kind?: string }) => ({
        added: counts.added + (line.kind === DiffLineKind.Added ? 1 : 0),
        removed: counts.removed + (line.kind === DiffLineKind.Removed ? 1 : 0),
      }),
      { added: 0, removed: 0 },
    ),
    [diffLines],
  );
  const applyFilesList = (fileList: readonly SkillFileEntry[], preferredPath = selectedPath) => {
    const next = [...fileList];
    const firstFile = preferredSkillFileName(next);
    const preferred = next.find((file) => file.name === preferredPath && file.kind === "file");
    const nextSelected = preferred?.name ?? firstFile ?? "";
    setFiles(next);
    setSelectedPath(nextSelected);
    if (!next.some((file) => file.name === activePath && file.kind === "file")) {
      setActivePath(firstFile ?? "");
    }
    return next;
  };
  const reloadFiles = async (preferredPath = selectedPath) => {
    let result: SkillFileEntry[];
    try {
      result = await readSkillFiles({
        skillId: currentSkill.id,
        skillPath,
      });
      setFileError("");
    } catch (error) {
      setFileError(error instanceof Error ? error.message : `${error}`);
      return [];
    }
    return applyFilesList(result, preferredPath);
  };
  const applyMutationFiles = (result: SkillFileMutationResponse | null, preferredPath: string) => {
    if (result?.files) {
      setFileError("");
      return applyFilesList(result.files, preferredPath);
    }
    return null;
  };
  const toggleFolder = (name: string) => {
    setCollapsedFolders((current) => {
      const next = new Set(current);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  };
  const revealSelected = () => {
    if (selectedEntry?.path) safeInvoke(TauriCommand.RevealInFinder, { path: selectedEntry.path });
  };
  const beginRename = (entry: SkillFileEntry | null = selectedEntry) => {
    if (readOnly || !entry || entry.name === "SKILL.md") return;
    setSelectedPath(entry.name);
    setRenamingPath(entry.name);
    setRenameValue(displayFileName(entry.name));
  };
  const submitRename = async () => {
    if (readOnly) {
      setRenamingPath("");
      return;
    }
    const entry = files.find((file) => file.name === renamingPath);
    const nextName = renameValue.trim();
    if (!entry || !nextName) {
      setRenamingPath("");
      return;
    }
    const nextPath = joinRelativePath(parentPath(entry.name), nextName);
    setRenamingPath("");
    if (nextPath === entry.name) return;
    let result: SkillFileMutationResponse;
    try {
      result = await renameSkillPath({
        skillId: currentSkill.id,
        fromRelativePath: entry.name,
        toRelativePath: nextPath,
        skillPath,
      });
    } catch (error) {
      setFileError(error instanceof Error ? error.message : `${error}`);
      return;
    }
    setDrafts((current) => {
      const next: Record<string, SkillDraft> = {};
      for (const [path, draft] of Object.entries(current)) {
        if (path === entry.name || path.startsWith(`${entry.name}/`)) {
          next[`${nextPath}${path.slice(entry.name.length)}`] = draft;
        } else {
          next[path] = draft;
        }
      }
      return next;
    });
    setCreatedPaths((current) => {
      const next = new Set(current);
      for (const path of current) {
        if (path === entry.name || path.startsWith(`${entry.name}/`)) {
          next.delete(path);
          next.add(`${nextPath}${path.slice(entry.name.length)}`);
        }
      }
      return next;
    });
    if (activePath === entry.name || activePath.startsWith(`${entry.name}/`)) {
      setActivePath(`${nextPath}${activePath.slice(entry.name.length)}`);
    }
    if (!applyMutationFiles(result, nextPath)) await reloadFiles(nextPath);
    if (result.skills) onSaved?.(result.skills);
  };
  const createEntry = async (kind: "file" | "folder", baseEntry: SkillFileEntry | null = selectedEntry) => {
    if (readOnly) return;
    const parent = baseEntry?.kind === "folder" ? baseEntry.name : parentPath(baseEntry?.name ?? activePath);
    const relativePath = uniqueChildPath(files, parent, kind);
    let result: SkillFileMutationResponse | null = null;
    try {
      if (kind === "folder") {
        result = await createSkillFolder({ skillId: currentSkill.id, relativePath, skillPath });
      } else {
        result = await createSkillFile({ skillId: currentSkill.id, relativePath, skillPath });
      }
    } catch (error) {
      setFileError(error instanceof Error ? error.message : `${error}`);
      return;
    }
    if (kind === "folder") {
      setCollapsedFolders((current) => {
        const next = new Set(current);
        if (parent) next.delete(parent);
        return next;
      });
    } else {
      const createdSha256 = result?.sha256;
      if (typeof createdSha256 === "string") {
        setActivePath(relativePath);
        setDrafts((current) => ({
          ...current,
          [relativePath]: { content: "", originalContent: "", sha256: createdSha256 },
        }));
      }
    }
    setCreatedPaths((current) => new Set(current).add(relativePath));
    if (!applyMutationFiles(result, relativePath)) await reloadFiles(relativePath);
    if (result?.skills) onSaved?.(result.skills);
    beginRename({ name: relativePath, kind: kind as SkillFileEntry["kind"] });
  };
  const performDeleteEntry = async (entry: SkillFileEntry) => {
    if (readOnly || !entry || entry.name === "SKILL.md") return;
    let result: SkillFileMutationResponse;
    try {
      result = await deleteSkillPath({ skillId: currentSkill.id, relativePath: entry.name, skillPath });
    } catch (error) {
      setFileError(error instanceof Error ? error.message : `${error}`);
      return;
    }
    const next = applyMutationFiles(result, activePath === entry.name ? "" : activePath)
      ?? await reloadFiles(activePath === entry.name ? "" : activePath);
    if (activePath === entry.name || entry.kind === "folder" && activePath.startsWith(`${entry.name}/`)) {
      const nextFile = preferredSkillFileName(next);
      setActivePath(nextFile ?? "");
    }
    setDrafts((current) => Object.fromEntries(
      Object.entries(current).filter(([path]) => path !== entry.name && !path.startsWith(`${entry.name}/`)),
    ));
    setCreatedPaths((current) => {
      const nextCreated = new Set(current);
      for (const path of current) {
        if (path === entry.name || path.startsWith(`${entry.name}/`)) nextCreated.delete(path);
      }
      return nextCreated;
    });
    if (result.skills) onSaved?.(result.skills);
  };
  const requestDeleteEntry = (entry: SkillFileEntry | null = selectedEntry) => {
    if (readOnly || !entry || entry.name === "SKILL.md") return;
    setPendingDeleteEntry(entry);
  };
  const confirmDeleteEntry = async () => {
    const entry = pendingDeleteEntry;
    if (!entry || deletingEntry) return;
    setDeletingEntry(true);
    try {
      await performDeleteEntry(entry);
      setPendingDeleteEntry(null);
    } finally {
      setDeletingEntry(false);
    }
  };
  const handleFileTreeKeyDown = (event: React.KeyboardEvent<HTMLElement>) => {
    if (renamingPath) return;
    if ((event.target as HTMLElement | null)?.closest(".fileTreeActions, .fileTreeToggle")) return;
    if (event.key === "Enter") {
      if (readOnly) return;
      event.preventDefault();
      beginRename();
    } else if (event.key === "Delete" || event.key === "Backspace") {
      if (readOnly) return;
      event.preventDefault();
      requestDeleteEntry();
    } else if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "r") {
      event.preventDefault();
      revealSelected();
    }
  };
  const fileTreeStatus = (entry: SkillFileEntry) => {
    if (readOnly) return "";
    const paths = entry.kind === "folder"
      ? files.filter((file) => file.name.startsWith(`${entry.name}/`)).map((file) => file.name)
      : [entry.name];
    if (paths.some((path) => createdPaths.has(path))) return "U";
    if (paths.some((path) => {
      const draft = drafts[path];
      return draft && draft.content !== draft.originalContent;
    })) return "M";
    return "";
  };
  const handleBack = () => {
    if (hasUnsavedDrafts) {
      setShowDiscardDialog(true);
      return;
    }
    back();
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        save();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [save]);

  useLayoutEffect(() => {
    if (linkedSessionsSkillRef.current === currentSkill.name) return;
    linkedSessionsSkillRef.current = currentSkill.name;
    linkedSessionsRequestRef.current += 1;
    setLinkedSessions([]);
    setLinkedSessionsError("");
    setLoadingLinkedSessions(false);
    setShowLinkedSessions(false);
  }, [currentSkill.name]);

  const loadLinkedSessions = useCallback(async () => {
    const request = ++linkedSessionsRequestRef.current;
    setLoadingLinkedSessions(true);
    setLinkedSessionsError("");
    try {
      if (onReadSkillIndexStatus) await onReadSkillIndexStatus();
      const links = await invokeSkillSessionLinks(currentSkill.name);
      if (request === linkedSessionsRequestRef.current) {
        setLinkedSessions(links.flatMap((link) => {
          const normalized = normalizeSessionSkillLink(link);
          return normalized ? [normalized] : [];
        }));
      }
    } catch (error) {
      if (request === linkedSessionsRequestRef.current) setLinkedSessionsError(`${error}`);
    } finally {
      if (request === linkedSessionsRequestRef.current) setLoadingLinkedSessions(false);
    }
  }, [currentSkill.name, onReadSkillIndexStatus, skillIndexStatus?.last_indexed_at]);

  useEffect(() => {
    if (!showLinkedSessions) return;
    void loadLinkedSessions();
    return () => { linkedSessionsRequestRef.current += 1; };
  }, [currentSkill.name, loadLinkedSessions, showLinkedSessions]);

  /* The open handler primes the state so the drawer never renders a stale empty result. */
  const openLinkedSessions = () => {
    setLinkedSessionsError("");
    setLoadingLinkedSessions(true);
    setShowLinkedSessions(true);
  };

  const fileTreeSize = fileTreeCollapsed ? 44 : 240;
  useLayoutEffect(() => {
    fileTreePanelRef.current?.resize(fileTreeSize);
  }, [fileTreeSize]);

  return (
    <section className="editorPage">
      <EditorHeader
        title={currentSkill.name}
        backLabel="Back to skills"
        onBack={handleBack}
        actions={(
          <>
              <IconButton
              onClick={openLinkedSessions}
              aria-label="Open recent sessions"
            >
              <Waypoints size={15} />
            </IconButton>
            <SkillInfoMenu skill={currentSkill} skills={skills} onOpenSkill={onOpenSkill} />
          </>
        )}
      />
      {fileError ? <Toast tone="error" message={fileError} onDismiss={() => setFileError("")} /> : null}
      {contentError ? <Toast tone="error" message={contentError} onDismiss={() => setContentError("")} /> : null}
      {showDiscardDialog ? (
        <Suspense fallback={(
          <DialogLoadingFallback
            title={dialogCopy.discardChangesTitle}
            label="Loading discard dialog"
            descriptionId="discard-changes-loading-description"
            onOpenChange={setShowDiscardDialog}
            description="This file has edits that have not been saved."
            showLoading={false}
            actions={(
              <>
                <DialogActionButton variant="secondary" onClick={() => setShowDiscardDialog(false)}>Cancel</DialogActionButton>
                <DialogActionButton
                  variant="danger"
                  onClick={() => {
                    setShowDiscardDialog(false);
                    back();
                  }}
                >
                  Discard changes
                </DialogActionButton>
              </>
            )}
          />
        )}>
          <DiscardChangesDialog open onOpenChange={setShowDiscardDialog} onDiscard={back} />
        </Suspense>
      ) : null}
      <DeleteConfirmationDialog
        open={Boolean(pendingDeleteEntry)}
        items={pendingDeleteEntry ? [pendingDeleteEntry.name] : []}
        itemLabel={pendingDeleteEntry?.kind === "folder" ? "folder" : "file"}
        description="Delete this skill file? This action cannot be undone."
        busy={deletingEntry}
        onOpenChange={(open) => { if (!open) setPendingDeleteEntry(null); }}
        onConfirm={() => { void confirmDeleteEntry(); }}
      />
      {showLinkedSessions ? (
        <Suspense fallback={<LinkedSessionsDrawerFallback onClose={() => setShowLinkedSessions(false)} />}>
          <LinkedSessionsDrawer
            open
            onOpenChange={setShowLinkedSessions}
            links={linkedSessions}
            loading={loadingLinkedSessions}
            error={linkedSessionsError}
            onRetry={() => { void loadLinkedSessions(); }}
            onOpenSession={onOpenSession}
          />
        </Suspense>
      ) : null}
      <PanelGroup
        className="editorShell"
        orientation="horizontal"
      >
        <Panel
          className={`fileTreePanel ${fileTreeCollapsed ? "collapsed" : ""}`}
          defaultSize="240px"
          groupResizeBehavior="preserve-pixel-size"
          maxSize={fileTreeCollapsed ? "44px" : "500px"}
          minSize={fileTreeCollapsed ? "44px" : "190px"}
          panelRef={fileTreePanelRef}
        >
          <aside className="fileTree" onKeyDown={handleFileTreeKeyDown}>
            <ContextMenu.Root>
              <ContextMenu.Trigger asChild>
                <div className="fileTreeBody">
                  <div className="fileTreeHeader">
                    <button
                      className="fileTreeToggle"
                      aria-label={fileTreeCollapsed ? "Expand files" : "Collapse files"}
                      onClick={() => setFileTreeCollapsed((value) => !value)}
                    >
                      {fileTreeCollapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
                      {!fileTreeCollapsed && <span>Files</span>}
                    </button>
                    {!readOnly && !fileTreeCollapsed && (
                      <div className="fileTreeActions">
                        <IconButton aria-label="New file" onClick={() => createEntry("file")}><FilePlus size={13} /></IconButton>
                        <IconButton aria-label="New folder" onClick={() => createEntry("folder")}><FolderPlus size={13} /></IconButton>
                      </div>
                    )}
                  </div>
                  {!fileTreeCollapsed && loadingFiles && (
                    <LoadingState className="fileTreeLoading" label="Loading files" />
                  )}
                  {!fileTreeCollapsed && !loadingFiles && rows.map(({ file, depth, isFolder }) => {
                const isCollapsed = collapsedFolders.has(file.name);
                const isSelected = selectedPath === file.name;
                const isRenaming = renamingPath === file.name;
                const rowClassName = `fileItem ${isFolder ? "folderItem" : ""} ${file.name === activePath ? "active" : ""} ${isSelected ? "selected" : ""}`;
                const rowStyle = { paddingLeft: `${9 + depth * 16}px` };
                const rowIcon = isFolder ? <Folder size={14} /> : <FileText size={14} />;
                const rowChevron = <span className="treeChevron">{isFolder ? (isCollapsed ? <ChevronRight size={12} /> : <ChevronDown size={12} />) : null}</span>;
                const status = fileTreeStatus(file);
                if (!readOnly && isRenaming) {
                  return (
                    <div className={`${rowClassName} renaming`} key={file.name} style={rowStyle}>
                      {rowChevron}
                      {rowIcon}
                      <input
                        aria-label={`Rename ${displayFileName(file.name)}`}
                        ref={renameInputRef}
                        value={renameValue}
                        onBlur={submitRename}
                        onChange={(event) => setRenameValue(event.target.value)}
                        onKeyDown={(event) => {
                          if (event.key === "Enter") {
                            event.preventDefault();
                            submitRename();
                          } else if (event.key === "Escape") {
                            event.preventDefault();
                            setRenamingPath("");
                          }
                        }}
                      />
                    </div>
                  );
                }
                return (
                  <ContextMenu.Root key={file.name}>
                    <Tooltip content={file.name} onlyWhenTruncated>
                      <ContextMenu.Trigger asChild>
                        <button
                        aria-expanded={isFolder ? !isCollapsed : undefined}
                        className={rowClassName}
                        onClick={() => {
                          setSelectedPath(file.name);
                          if (isFolder) toggleFolder(file.name);
                          else setActivePath(file.name);
                        }}
                        onContextMenu={() => setSelectedPath(file.name)}
                        onDoubleClick={readOnly ? undefined : () => beginRename(file)}
                        style={rowStyle}
                      >
                        {rowChevron}
                        {rowIcon}
                        <span className="fileItemName">{displayFileName(file.name)}</span>
                        {status && <Badge tone={status === "U" ? "success" : "warning"}>{status}</Badge>}
                        </button>
                      </ContextMenu.Trigger>
                    </Tooltip>
                    <ContextMenu.Portal>
                      <ContextMenu.Content className="skillMenuContent" {...({ sideOffset: 6 } as Record<string, unknown>)}>
                        <FileTreeContextMenuItems
                          Menu={ContextMenu}
                          entry={file}
                          readOnly={readOnly || file.name === "SKILL.md"}
                          onNewFile={() => createEntry("file", file)}
                          onNewFolder={() => createEntry("folder", file)}
                          onReveal={() => {
                            if (file.path) safeInvoke(TauriCommand.RevealInFinder, { path: file.path });
                          }}
                          onRename={() => beginRename(file)}
                          onDelete={() => requestDeleteEntry(file)}
                        />
                      </ContextMenu.Content>
                    </ContextMenu.Portal>
                  </ContextMenu.Root>
                );
                  })}
                </div>
              </ContextMenu.Trigger>
              {!readOnly && <ContextMenu.Portal>
                <ContextMenu.Content className="skillMenuContent" {...({ sideOffset: 6 } as Record<string, unknown>)}>
                  <FileTreeContextMenuItems
                    Menu={ContextMenu}
                    entry={null}
                    readOnly={readOnly || selectedEntry?.name === "SKILL.md"}
                    onNewFile={() => createEntry("file", null)}
                    onNewFolder={() => createEntry("folder", null)}
                    onReveal={revealSelected}
                    onRename={() => beginRename()}
                    onDelete={() => requestDeleteEntry()}
                  />
                </ContextMenu.Content>
              </ContextMenu.Portal>}
            </ContextMenu.Root>
          </aside>
        </Panel>
        {!fileTreeCollapsed && <ResizeSeparator className="fileTreeResizeHandle" />}
        <Panel className="codePanePanel" minSize="360px">
          {contentReady ? (
            <MarkdownFilePane
              activePath={activePath}
              dirty={dirty}
              diffStats={diffStats}
              content={content}
              originalContent={original.content}
              readOnly={readOnly}
              saveState={saveState}
              saveError={saveError}
              onSave={save}
              onChange={(nextContent: string) => {
                if (readOnly) return;
                setSaveState(SaveStatus.Idle);
                setSaveError("");
                setDrafts((current) => {
                  const draft = current[activePath] ?? { content: "", originalContent: "", sha256: "" };
                  return { ...current, [activePath]: { ...draft, content: nextContent } };
                });
              }}
              onNormalize={(nextContent: string) => {
                if (readOnly) return;
                setDrafts((current) => {
                  const draft = current[activePath] ?? { content: "", originalContent: "", sha256: "" };
                  if (draft.content !== draft.originalContent || draft.content === nextContent) return current;
                  return {
                    ...current,
                    [activePath]: { ...draft, content: nextContent, originalContent: nextContent },
                  };
                });
              }}
            />
          ) : (
            <EditorStatePlaceholder
              label={!contentReady || loadingContent || loadingFiles ? "Loading file" : undefined}
            >
              {contentReady && !loadingContent && !loadingFiles ? "Select a file" : null}
            </EditorStatePlaceholder>
          )}
        </Panel>
      </PanelGroup>
    </section>
  );
}
