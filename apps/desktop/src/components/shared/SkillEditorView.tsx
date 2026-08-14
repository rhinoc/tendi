import { Tooltip } from "./Tooltip.tsx";
import { lazy, Suspense, useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  FilePlus,
  FileText,
  Folder,
  FolderPlus,
  Table as TableIcon,
} from "lucide-react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";
import { ContextMenu } from "radix-ui";
import { invoke } from "@tauri-apps/api/core";

import {
  buildFileTreeRows,
  diffPreview,
  displayFileName,
  fallbackSkillContent,
  fallbackSkills,
  isMarkdownPath,
  isReadOnlySkillSource,
  joinRelativePath,
  normalizeSkillFileEntries,
  parentPath,
  preferredSkillFileName,
  safeInvoke,
  SkillVisibility,
  TauriCommand,
  uniqueChildPath,
  type SkillFileEntry,
  type SkillLike,
} from "../../lib/index.ts";
import { EditorHeader } from "./EditorHeader.tsx";
import { FileTreeContextMenuItems } from "./FileTreeContextMenuItems.tsx";
import { LoadingInline } from "./LoadingInline.tsx";
import { MarkdownFilePane, type DiffStats } from "./MarkdownFilePane.tsx";
import { ResizeSeparator } from "./ResizeSeparator.tsx";
import type { SkillDependencyRecord } from "./SkillDependencyGraph.tsx";
import { SkillInfoMenu } from "./SkillInfoMenu.tsx";
import { loadCodeMirrorFileEditor } from "./code-mirror-loader.ts";

const DiscardChangesDialog = lazy(() => import("./DiscardChangesDialog.tsx").then(({ DiscardChangesDialog: component }) => ({ default: component })));
const LinkedSessionsDrawer = lazy(() => import("./linked-sessions.tsx").then(({ LinkedSessionsDrawer: component }) => ({ default: component })));

export type SkillEditorRecord = SkillLike & {
  name: string;
  agents?: string[];
  visibility?: string;
  description?: string;
  dependencies?: string[];
  dependents?: string[];
};

export type SkillIndexStatus = {
  indexed?: number;
  running?: boolean;
};

export type SkillEditorViewProps = {
  skill?: SkillEditorRecord | null;
  skills?: SkillDependencyRecord[];
  back: () => void;
  skillIndexStatus?: SkillIndexStatus | null;
  onOpenSession?: (link: Record<string, unknown>) => void;
  onOpenSkill?: (name: string) => void;
  onSaved?: (skills?: SkillDependencyRecord[]) => void;
};

type SkillDraft = {
  content: string;
  originalContent: string;
  sha256: string;
};

type SkillFileReadResult = {
  content?: string;
  sha256?: string;
};

type SkillFileWriteResult = {
  content?: string;
  sha256?: string;
  skills?: SkillDependencyRecord[];
};

export function SkillEditorView({ skill, skills = [], back, skillIndexStatus, onOpenSession, onOpenSkill, onSaved }: SkillEditorViewProps) {
  const currentSkill = skill ?? fallbackSkills[0];
  const readOnly = isReadOnlySkillSource(currentSkill);
  const [files, setFiles] = useState<SkillFileEntry[]>([]);
  const [linkedSessions, setLinkedSessions] = useState<Record<string, unknown>[]>([]);
  const [loadingLinkedSessions, setLoadingLinkedSessions] = useState(false);
  const [showLinkedSessions, setShowLinkedSessions] = useState(false);
  const [activePath, setActivePath] = useState("SKILL.md");
  const [selectedPath, setSelectedPath] = useState("SKILL.md");
  const [drafts, setDrafts] = useState<Record<string, SkillDraft>>({});
  const [createdPaths, setCreatedPaths] = useState(() => new Set<string>());
  const [fileTreeCollapsed, setFileTreeCollapsed] = useState(false);
  const [collapsedFolders, setCollapsedFolders] = useState(() => new Set<string>());
  const [showDiscardDialog, setShowDiscardDialog] = useState(false);
  const [renamingPath, setRenamingPath] = useState("");
  const [renameValue, setRenameValue] = useState("");
  const [loadingFiles, setLoadingFiles] = useState(true);
  const [loadingContent, setLoadingContent] = useState(false);
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const [saveError, setSaveError] = useState("");
  const renameInputRef = useRef<HTMLInputElement>(null);
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
      setFiles([]);
      setActivePath("SKILL.md");
      setSelectedPath("SKILL.md");
      setDrafts({});
      setCreatedPaths(new Set());
      setRenamingPath("");
      setCollapsedFolders(new Set());
      setShowDiscardDialog(false);
      const fileListRead = safeInvoke(TauriCommand.SkillFiles, { name: currentSkill.name });
      void loadCodeMirrorFileEditor();
      const result = await fileListRead;
      if (cancelled) return;
      const next = normalizeSkillFileEntries(result);
      const firstFile = preferredSkillFileName(next);
      setFiles(next);
      setActivePath(firstFile);
      setSelectedPath(firstFile);
      setLoadingFiles(false);
    }
    void loadFiles();
    return () => {
      cancelled = true;
    };
  }, [currentSkill.name]);

  useEffect(() => {
    if (renamingPath) {
      requestAnimationFrame(() => {
        renameInputRef.current?.focus();
        renameInputRef.current?.select();
      });
    }
  }, [renamingPath]);

  useEffect(() => {
    setSaveState("idle");
    setSaveError("");
  }, [activePath]);

  useEffect(() => {
    if (loadingFiles || drafts[activePath]) return undefined;
    let cancelled = false;
    async function loadContent() {
      setLoadingContent(true);
      const fileRead = safeInvoke(TauriCommand.SkillFileRead, { name: currentSkill.name, relativePath: activePath });
      if (isMarkdownPath(activePath)) void loadCodeMirrorFileEditor();
      const result = await fileRead as SkillFileReadResult | null;
      if (cancelled) return;
      if (typeof result?.content === "string") {
        const fileContent = result.content;
        setDrafts((current) => ({
          ...current,
          [activePath]: {
            content: fileContent,
            originalContent: fileContent,
            sha256: result.sha256 ?? "",
          },
        }));
      } else {
        const fallback = fallbackSkillContent({
          name: currentSkill.name,
          description: currentSkill.description ?? "",
          visibility: currentSkill.visibility ?? SkillVisibility.Auto,
        });
        setDrafts((current) => ({
          ...current,
          [activePath]: { content: fallback, originalContent: fallback, sha256: "" },
        }));
      }
      setLoadingContent(false);
    }
    void loadContent();
    return () => {
      cancelled = true;
      setLoadingContent(false);
    };
  }, [activePath, currentSkill, drafts, loadingFiles]);

  const save = useCallback(async () => {
    if (readOnly || !dirty || !original.sha256 || saveState === "saving") return;
    setSaveState("saving");
    setSaveError("");
    try {
      const result = await invoke<SkillFileWriteResult>(TauriCommand.SkillFileSave, {
        name: currentSkill.name,
        relativePath: activePath,
        expectedSha256: original.sha256,
        content,
      });
      const savedSha256 = result?.sha256;
      if (!savedSha256) throw new Error("Save returned no file hash");
      setDrafts((current) => ({
        ...current,
        [activePath]: { content: result.content ?? content, originalContent: result.content ?? content, sha256: savedSha256 },
      }));
      setSaveState("saved");
      onSaved?.(result.skills);
    } catch (error) {
      const message = error instanceof Error ? error.message : `${error}`;
      setSaveError(message);
      setSaveState("error");
    }
  }, [activePath, content, currentSkill.name, dirty, onSaved, original.sha256, readOnly, saveState]);
  const rows = useMemo(() => buildFileTreeRows(files, collapsedFolders), [collapsedFolders, files]);
  const selectedEntry = useMemo(
    () => files.find((file) => file.name === selectedPath) ?? files.find((file) => file.name === activePath) ?? null,
    [activePath, files, selectedPath],
  );
  const diffLines = useMemo(
    () => diffPreview(original.content, deferredContent),
    [deferredContent, original.content],
  );
  const diffStats = useMemo(
    () => diffLines.reduce(
      (counts: DiffStats, line: { kind?: string }) => ({
        added: counts.added + (line.kind === "added" ? 1 : 0),
        removed: counts.removed + (line.kind === "removed" ? 1 : 0),
      }),
      { added: 0, removed: 0 },
    ),
    [diffLines],
  );
  const reloadFiles = async (preferredPath = selectedPath) => {
    const result = await safeInvoke(TauriCommand.SkillFiles, { name: currentSkill.name });
    const next = normalizeSkillFileEntries(result);
    const firstFile = preferredSkillFileName(next);
    const preferred = next.find((file) => file.name === preferredPath);
    const nextSelected = preferred?.name ?? firstFile;
    setFiles(next);
    setSelectedPath(nextSelected);
    if (!next.some((file) => file.name === activePath && file.kind === "file")) {
      setActivePath(firstFile);
    }
    return next;
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
    await safeInvoke(TauriCommand.SkillPathRename, {
      name: currentSkill.name,
      fromRelativePath: entry.name,
      toRelativePath: nextPath,
    });
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
    await reloadFiles(nextPath);
  };
  const createEntry = async (kind: "file" | "folder", baseEntry: SkillFileEntry | null = selectedEntry) => {
    if (readOnly) return;
    const parent = baseEntry?.kind === "folder" ? baseEntry.name : parentPath(baseEntry?.name ?? activePath);
    const relativePath = uniqueChildPath(files, parent, kind);
    if (kind === "folder") {
      await safeInvoke(TauriCommand.SkillFolderCreate, { name: currentSkill.name, relativePath });
      setCollapsedFolders((current) => {
        const next = new Set(current);
        if (parent) next.delete(parent);
        return next;
      });
    } else {
      const result = await safeInvoke(TauriCommand.SkillFileCreate, { name: currentSkill.name, relativePath }) as SkillFileReadResult | null;
      if (result?.content !== undefined) {
        setActivePath(relativePath);
        setDrafts((current) => ({
          ...current,
          [relativePath]: { content: result.content ?? "", originalContent: result.content ?? "", sha256: result.sha256 ?? "" },
        }));
      }
    }
    setCreatedPaths((current) => new Set(current).add(relativePath));
    await reloadFiles(relativePath);
    beginRename({ name: relativePath, kind: kind as SkillFileEntry["kind"] });
  };
  const deleteEntry = async (entry: SkillFileEntry | null = selectedEntry) => {
    if (readOnly || !entry || entry.name === "SKILL.md") return;
    await safeInvoke(TauriCommand.SkillPathDelete, { name: currentSkill.name, relativePath: entry.name });
    const next = await reloadFiles(activePath === entry.name ? "" : activePath);
    if (activePath === entry.name || entry.kind === "folder" && activePath.startsWith(`${entry.name}/`)) {
      const nextFile = preferredSkillFileName(next);
      setActivePath(nextFile);
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
      deleteEntry();
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

  useEffect(() => {
    if (!showLinkedSessions) return;
    let cancelled = false;
    setLoadingLinkedSessions(true);
    safeInvoke(TauriCommand.SkillSessionLinks, { skillName: currentSkill.name }).then((links) => {
      if (!cancelled) {
        setLinkedSessions(Array.isArray(links) ? links : []);
        setLoadingLinkedSessions(false);
      }
    });
    return () => { cancelled = true; };
  }, [currentSkill.name, showLinkedSessions, skillIndexStatus?.indexed, skillIndexStatus?.running]);

  return (
    <section className="editorPage">
      <EditorHeader
        title={currentSkill.name}
        backLabel="Back to skills"
        onBack={handleBack}
        actions={(
          <>
            <Tooltip content="Linked sessions"><button
              className="headerGhostButton"
              onClick={() => setShowLinkedSessions(true)}
              aria-label="Open linked sessions"
            >
              <TableIcon size={15} />
            </button></Tooltip>
            <SkillInfoMenu skill={currentSkill} skills={skills} onOpenSkill={onOpenSkill} />
          </>
        )}
      />
      {showDiscardDialog ? (
        <Suspense fallback={null}>
          <DiscardChangesDialog open onOpenChange={setShowDiscardDialog} onDiscard={back} />
        </Suspense>
      ) : null}
      {showLinkedSessions ? (
        <Suspense fallback={null}>
          <LinkedSessionsDrawer
            open
            onOpenChange={setShowLinkedSessions}
            links={linkedSessions}
            loading={loadingLinkedSessions}
            onOpenSession={onOpenSession}
          />
        </Suspense>
      ) : null}
      <PanelGroup
        className="editorShell"
        key={fileTreeCollapsed ? "editor-files-collapsed" : "editor-files-expanded"}
        orientation="horizontal"
      >
        <Panel
          className={`fileTreePanel ${fileTreeCollapsed ? "collapsed" : ""}`}
          defaultSize={fileTreeCollapsed ? "44px" : "240px"}
          groupResizeBehavior="preserve-pixel-size"
          maxSize={fileTreeCollapsed ? "44px" : "500px"}
          minSize={fileTreeCollapsed ? "44px" : "190px"}
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
                        <button aria-label="New file" onClick={() => createEntry("file")}><FilePlus size={13} /></button>
                        <button aria-label="New folder" onClick={() => createEntry("folder")}><FolderPlus size={13} /></button>
                      </div>
                    )}
                  </div>
                  {!fileTreeCollapsed && loadingFiles && (
                    <div className="emptyState fileTreeEmpty"><LoadingInline label="Loading files" /></div>
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
                        {status && <span className={`fileTreeStatus ${status === "U" ? "untracked" : "modified"}`}>{status}</span>}
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
                          onDelete={() => deleteEntry(file)}
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
                    onDelete={() => deleteEntry()}
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
                setSaveState("idle");
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
            <div className="emptyState editorLoadingState">
              {!contentReady || loadingContent || loadingFiles ? <LoadingInline label="Loading file" /> : "Select a file"}
            </div>
          )}
        </Panel>
      </PanelGroup>
    </section>
  );
}
