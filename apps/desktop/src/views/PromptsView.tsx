import { lazy, Suspense, useCallback, useEffect, useMemo, useState, type KeyboardEvent } from "react";
import { ContextMenu, Dialog } from "radix-ui";
import {
  FileText,
  Pencil,
  Plus,
  RefreshCw,
  Save,
  Search,
  Trash2,
  X,
} from "lucide-react";

import { ContentTopDragStrip } from "../components/shared/ContentTopDragStrip.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
import { CopyTextMenuItem, DeleteMenuItem } from "../components/shared/DataTableMenus.tsx";
import { DialogActionBar } from "../components/shared/DialogActionBar.tsx";
import { DialogTextField } from "../components/shared/DialogTextField.tsx";
import { LoadingInline } from "../components/shared/LoadingInline.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef } from "../components/DataTable.types";
import { TauriCommand, compactDateTime, normalizePromptTags, promptPreview, promptTagsLabel, safeInvoke } from "../lib/index.ts";

const PromptBodyEditor = lazy(() => import("../components/shared/PromptBodyEditor.tsx").then(({ PromptBodyEditor: component }) => ({ default: component })));

type PromptRecord = {
  id: string;
  title: string;
  tags: string[];
  body: string;
  createdAt?: string;
  updatedAt?: string;
};

type PromptDraft = {
  id?: string;
  title: string;
  tags: string[];
  body: string;
};

type TagInputProps = {
  label: string;
  value: string[];
  onChange: (tags: string[]) => void;
  placeholder?: string;
};

export function TagInput({ label, value, onChange, placeholder }: TagInputProps) {
  const [draft, setDraft] = useState("");
  const addDraftTags = useCallback((text: string) => {
    const nextTags = normalizePromptTags([...value, text]);
    onChange(nextTags);
    setDraft("");
  }, [onChange, value]);
  const removeTag = useCallback((indexToRemove: number) => {
    onChange(value.filter((_, index) => index !== indexToRemove));
  }, [onChange, value]);
  const handleKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter" || event.key === "," || event.key === "Tab") {
      if (!draft.trim()) return;
      event.preventDefault();
      addDraftTags(draft);
    } else if (event.key === "Backspace" && !draft && value.length > 0) {
      onChange(value.slice(0, -1));
    }
  };
  return (
    <label className="dialogField">
      <span>{label}</span>
      <div className="tagInput">
        {value.map((tag, index) => (
          <span className="promptTag" key={`${tag}-${index}`}>
            <span>{tag}</span>
            <button
              type="button"
              aria-label={`Remove ${tag}`}
              onMouseDown={(event) => event.preventDefault()}
              onClick={(event) => {
                event.preventDefault();
                event.stopPropagation();
                removeTag(index);
              }}
            >
              <X size={12} />
            </button>
          </span>
        ))}
        <input
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={handleKeyDown}
          onBlur={() => {
            if (draft.trim()) addDraftTags(draft);
          }}
          onPaste={(event) => {
            const text = event.clipboardData.getData("text");
            if (!text.includes(",")) return;
            event.preventDefault();
            addDraftTags(text);
          }}
          placeholder={value.length ? "" : placeholder}
        />
      </div>
    </label>
  );
}

type PromptDialogProps = {
  open: boolean;
  prompt: PromptRecord | null;
  busy: boolean;
  error: string;
  onOpenChange: (open: boolean) => void;
  onSave: (draft: PromptDraft) => void;
};

export function PromptDialog({ open, prompt, busy, error, onOpenChange, onSave }: PromptDialogProps) {
  const [title, setTitle] = useState("");
  const [tags, setTags] = useState<string[]>([]);
  const [body, setBody] = useState("");
  const editing = Boolean(prompt?.id);

  useEffect(() => {
    if (!open) return;
    setTitle(prompt?.title ?? "");
    setTags(prompt?.tags ?? []);
    setBody(prompt?.body ?? "");
  }, [open, prompt]);

  const canSave = title.trim() && body.trim();
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialogOverlay" />
        <Dialog.Content className="promptDialogPanel" aria-describedby="prompt-dialog-description" data-no-drag onMouseDown={(event) => event.stopPropagation()}>
          <Dialog.Title className="confirmDialogTitle">{editing ? "Edit prompt" : "New prompt"}</Dialog.Title>
          <p id="prompt-dialog-description" className="confirmDialogDescription">
            Save a reusable prompt for quick copying later.
          </p>
          <div className="promptDialogBody">
            <DialogTextField label="Title" value={title} onChange={setTitle} placeholder="Code review checklist" />
            <TagInput label="Tags" value={tags} onChange={setTags} placeholder="review, planning" />
            <div className="dialogField promptBodyField">
              <span>Prompt</span>
              <Suspense fallback={<div className="promptCodeMirrorEditor"><LoadingInline label="Loading editor" /></div>}>
                <PromptBodyEditor value={body} onChange={setBody} />
              </Suspense>
            </div>
            {error ? <div className="addSkillError promptDialogError">{error}</div> : null}
          </div>
          <DialogActionBar cancelDisabled={busy} onCancel={() => onOpenChange(false)}>
            <button
              className="primary dialogAdvanceButton"
              disabled={!canSave || busy}
              onClick={() => onSave({ id: prompt?.id, title, tags, body })}
            >
              <span>{busy ? "Saving" : "Save"}</span>
              {busy ? <RefreshCw className="dialogLoadingIcon" size={16} /> : <Save size={16} />}
            </button>
          </DialogActionBar>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

export async function copyTextToClipboard(text: string) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  const copied = document.execCommand("copy");
  document.body.removeChild(textarea);
  if (!copied) throw new Error("clipboard write failed");
}

type PromptsViewProps = {
  prompts: PromptRecord[];
  loadingPrompts?: boolean;
  onRefreshPrompts: () => void | Promise<void>;
};

export function PromptsView({ prompts, loadingPrompts = false, onRefreshPrompts }: PromptsViewProps) {
  const [selected, setSelected] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [editingPrompt, setEditingPrompt] = useState<PromptRecord | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const [dialogError, setDialogError] = useState("");
  const [deletingPromptIds, setDeletingPromptIds] = useState<string[]>([]);
  const normalizedQuery = query.trim().toLowerCase();
  const visiblePrompts = useMemo(() => {
    if (!normalizedQuery) return prompts;
    return prompts.filter((prompt) => [prompt.title, promptTagsLabel(prompt), prompt.body]
      .some((value) => `${value ?? ""}`.toLowerCase().includes(normalizedQuery)));
  }, [normalizedQuery, prompts]);
  useEffect(() => {
    setSelected((current) => current.filter((id) => prompts.some((prompt) => prompt.id === id)));
  }, [prompts]);

  const openNewPrompt = () => {
    setDialogError("");
    setEditingPrompt(null);
    setDialogOpen(true);
  };
  const openEditPrompt = useCallback((prompt: PromptRecord) => {
    setDialogError("");
    setEditingPrompt(prompt);
    setDialogOpen(true);
  }, []);
  const savePrompt = async (draft: PromptDraft) => {
    if (saving) return;
    setSaving(true);
    setDialogError("");
    const result = await safeInvoke(TauriCommand.PromptSave, {
      id: draft.id ?? null,
      title: draft.title,
      tags: normalizePromptTags(draft.tags),
      body: draft.body,
    });
    setSaving(false);
    if (!result) {
      setDialogError("Could not save prompt.");
      return;
    }
    setDialogOpen(false);
    await onRefreshPrompts();
  };
  const copyPrompts = async (items: PromptRecord[]) => {
    const text = items.map((prompt) => prompt.body).filter(Boolean).join("\n\n");
    if (!text) return false;
    await copyTextToClipboard(text);
    return true;
  };
  const deleteSelected = async (ids: string[]) => {
    if (ids.length === 0) return;
    const pendingIds = ids.filter((id) => !deletingPromptIds.includes(id));
    if (pendingIds.length === 0) return;
    setDeletingPromptIds((current) => Array.from(new Set([...current, ...pendingIds])));
    try {
      const result = await safeInvoke(TauriCommand.PromptsDeleteMany, { ids: pendingIds });
      if (!result) return;
      setSelected((current) => current.filter((id) => !pendingIds.includes(id)));
      await onRefreshPrompts();
    } finally {
      setDeletingPromptIds((current) => current.filter((id) => !pendingIds.includes(id)));
    }
  };
  const columns = useMemo((): ColumnDef<PromptRecord>[] => [
    {
      key: "title",
      header: "Title",
      type: "text",
      sortValue: (prompt) => prompt.title?.toLowerCase() ?? "",
      width: "minmax(220px, 1fr)",
      cell: "title",
    },
    {
      key: "tags",
      header: "Tags",
      type: "enum",
      groupBy: (prompt) => promptTagsLabel(prompt),
      sortValue: (prompt) => promptTagsLabel(prompt).toLowerCase(),
      width: "160px",
      render: (prompt) => (
        <span className="promptTags">
          {prompt.tags.length ? prompt.tags.map((tag) => <span className="promptTag" key={tag}>{tag}</span>) : <span className="promptTag muted">Untagged</span>}
        </span>
      ),
    },
    {
      key: "preview",
      header: "Prompt",
      type: "text",
      sortValue: (prompt) => promptPreview(prompt).toLowerCase(),
      width: "260px",
      value: (prompt) => promptPreview(prompt),
      title: (prompt) => prompt.body,
    },
    {
      key: "updatedAt",
      header: "Updated",
      type: "date",
      sortValue: (prompt) => prompt.updatedAt ?? "",
      width: "116px",
      value: (prompt) => compactDateTime(prompt.updatedAt),
      empty: "",
    },
    {
      key: "actions",
      header: "",
      width: "108px",
      render: (prompt) => (
        <div className="rowActions">
          <CopyButton
            className="iconButton"
            copyLabel={`Copy ${prompt.title}`}
            copiedLabel="Prompt copied"
            title="Copy prompt"
            iconSize={15}
            stopPropagation
            onCopy={() => copyPrompts([prompt])}
          />
          <button
            className="iconButton"
            aria-label={`Edit ${prompt.title}`}
            onClick={(event) => {
              event.stopPropagation();
              openEditPrompt(prompt);
            }}
          >
            <Pencil size={15} />
          </button>
          <button
            className={`iconButton dangerIcon${deletingPromptIds.includes(prompt.id) ? " isBusy" : ""}`}
            aria-label={`Delete ${prompt.title}`}
            aria-busy={deletingPromptIds.includes(prompt.id)}
            aria-disabled={deletingPromptIds.includes(prompt.id) || undefined}
            data-no-row-click
            onClick={(event) => {
              event.stopPropagation();
              if (deletingPromptIds.includes(prompt.id)) return;
              deleteSelected([prompt.id]);
            }}
          >
            {deletingPromptIds.includes(prompt.id)
              ? <RefreshCw className="loadingSpinner" size={15} />
              : <Trash2 size={15} />}
          </button>
        </div>
      ),
    },
  ], [copyPrompts, deleteSelected, deletingPromptIds, openEditPrompt]);

  const bottomBar = useCallback((selectedRows: PromptRecord[]) => (
    <>
      <CopyButton
        copyLabel="Copy selected prompts"
        copiedLabel="Selected prompts copied"
        iconSize={15}
        onCopy={() => copyPrompts(selectedRows)}
      >
        Copy
      </CopyButton>
      <button
        className="danger"
        aria-label="Delete selected prompts"
        aria-busy={selectedRows.some((prompt) => deletingPromptIds.includes(prompt.id))}
        disabled={selectedRows.some((prompt) => deletingPromptIds.includes(prompt.id))}
        onClick={() => deleteSelected(selectedRows.map((prompt) => prompt.id))}
      >
        {selectedRows.some((prompt) => deletingPromptIds.includes(prompt.id))
          ? <RefreshCw className="loadingSpinner" size={15} />
          : <Trash2 size={15} />}
        {selectedRows.some((prompt) => deletingPromptIds.includes(prompt.id)) ? "Deleting" : "Delete"}
      </button>
    </>
  ), [copyPrompts, deleteSelected, deletingPromptIds]);
  const rowContextMenu = useCallback((prompt: PromptRecord, { selectedRows, selected: isSelected }: { selectedRows: PromptRecord[]; selected: boolean }) => {
    const showBulk = isSelected && selectedRows.length > 1;
    return showBulk ? (
      <>
        <ContextMenu.Item className="skillMenuItem" onSelect={() => copyPrompts(selectedRows)}>
          Copy selected
        </ContextMenu.Item>
        <ContextMenu.Separator className="skillMenuSeparator" />
        <DeleteMenuItem
          Menu={ContextMenu}
          label="Delete selected"
          disabled={selectedRows.some((item) => deletingPromptIds.includes(item.id))}
          onSelect={() => deleteSelected(selectedRows.map((item) => item.id))}
        />
      </>
    ) : (
      <>
        <CopyTextMenuItem Menu={ContextMenu} text={prompt.body} label="Copy prompt" />
        <ContextMenu.Item className="skillMenuItem" onSelect={() => openEditPrompt(prompt)}>
          Edit prompt
        </ContextMenu.Item>
        <ContextMenu.Separator className="skillMenuSeparator" />
        <DeleteMenuItem
          Menu={ContextMenu}
          label="Delete prompt"
          disabled={deletingPromptIds.includes(prompt.id)}
          onSelect={() => deleteSelected([prompt.id])}
        />
      </>
    );
  }, [copyPrompts, deleteSelected, deletingPromptIds, openEditPrompt]);

  return (
    <section className="content dataPage promptsPage">
      <ContentTopDragStrip />
      <PageHeader title="Prompts">
        <div className="searchBox">
          <Search size={15} />
          <input placeholder="Search prompts" value={query} onChange={(event) => setQuery(event.target.value)} />
        </div>
        <button className="iconButton" aria-label="Refresh prompts" onClick={onRefreshPrompts}><RefreshCw size={16} /></button>
        <button className="iconButton filled" aria-label="Add prompt" onClick={openNewPrompt}><Plus size={16} /></button>
      </PageHeader>
      <DataTable
        rows={visiblePrompts}
        columns={columns}
        getRowId={(prompt) => prompt.id}
        getRowLabel={(prompt) => prompt.title}
        selectable
        selectedIds={selected}
        onSelectionChange={setSelected}
        enableMarquee
        onRowClick={openEditPrompt}
        rowContextMenu={rowContextMenu}
        bottomBar={bottomBar}
        bottomBarCheckboxLabel="Select visible prompts from toolbar"
        selectionLabel="prompts"
        loading={loadingPrompts}
        loadingLabel={<LoadingInline label="Loading prompts" />}
        emptyState={normalizedQuery ? (
          <><FileText size={20} /><span>No prompts match this search</span><span>Try another search.</span></>
        ) : (
          <><FileText size={20} /><span>No prompts yet</span><span>Create a prompt to reuse instructions.</span></>
        )}
      />
      <PromptDialog
        open={dialogOpen}
        prompt={editingPrompt}
        busy={saving}
        error={dialogError}
        onOpenChange={setDialogOpen}
        onSave={savePrompt}
      />
    </section>
  );
}
