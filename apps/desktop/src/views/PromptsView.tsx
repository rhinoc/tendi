import { lazy, Suspense, useCallback, useEffect, useMemo, useState, type KeyboardEvent } from "react";
import { ContextMenu, Dialog, DropdownMenu } from "radix-ui";
import {
  Copy,
  MessageSquareText,
  Pencil,
  Plus,
  RefreshCw,
  Save,
  SearchX,
  Trash2,
  X,
} from "lucide-react";

import { Button } from "../components/shared/Button.tsx";
import { Badge } from "../components/shared/Badge.tsx";
import { ContentTopDragStrip } from "../components/shared/ContentTopDragStrip.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
import { CopyTextMenuItem, DeleteMenuItem } from "../components/shared/DataTableMenus.tsx";
import { DialogActionBar } from "../components/shared/DialogActionBar.tsx";
import { DialogStatefulButton } from "../components/shared/DialogStatefulButton.tsx";
import { DialogShell } from "../components/shared/DialogShell.tsx";
import { DialogTextField } from "../components/shared/DialogTextField.tsx";
import { EmptyState } from "../components/shared/EmptyState.tsx";
import { IconButton } from "../components/shared/IconButton.tsx";
import { LoadingState } from "../components/shared/LoadingState.tsx";
import { LoadErrorState } from "../components/shared/LoadErrorState.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { RowActionsMenu } from "../components/shared/RowActionsMenu.tsx";
import { SearchField } from "../components/shared/SearchField.tsx";
import { Tooltip } from "../components/shared/Tooltip.tsx";
import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef } from "../components/DataTable.types";
import type { DataTableMenuComponents } from "../components/shared/DataTableMenus.tsx";
import { TauriCommand, compactDateTime, normalizePromptTags, promptPreview, promptTagsLabel, safeInvoke, suppressNextClick } from "../lib/index.ts";

const PromptBodyEditor = lazy(() => import("../features/prompts/PromptBodyEditor.tsx").then(({ PromptBodyEditor: component }) => ({ default: component })));

type PromptRecord = {
  id: string;
  title: string;
  tags: string[];
  body: string;
  createdAt?: string;
  updatedAt?: string;
};

function PromptActionsMenuItems({
  Menu,
  prompt,
  isDeleting,
  onEdit,
  onDelete,
}: {
  Menu: DataTableMenuComponents;
  prompt: PromptRecord;
  isDeleting: boolean;
  onEdit: (prompt: PromptRecord) => void;
  onDelete: (id: string) => void;
}) {
  return (
    <>
      <Menu.Item className="skillMenuItem" onSelect={() => onEdit(prompt)}>
        <Pencil size={14} />
        Edit prompt
      </Menu.Item>
      <Menu.Separator className="skillMenuSeparator" />
      <DeleteMenuItem
        Menu={Menu}
        label="Delete prompt"
        disabled={isDeleting}
        onSelect={() => onDelete(prompt.id)}
      />
    </>
  );
}

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
          <Badge tone="accent" key={`${tag}-${index}`}>
            <span>{tag}</span>
            <button
              type="button"
              className="badgeRemove"
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
          </Badge>
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
    <DialogShell
      open={open}
      onOpenChange={onOpenChange}
      className="promptDialogPanel"
      descriptionId="prompt-dialog-description"
    >
      <Dialog.Title className="confirmDialogTitle">{editing ? "Edit prompt" : "New prompt"}</Dialog.Title>
      <p id="prompt-dialog-description" className="confirmDialogDescription">
        Save a reusable prompt for quick copying later.
      </p>
      <div className="promptDialogBody">
        <DialogTextField label="Title" value={title} onChange={setTitle} placeholder="Code review checklist" />
        <TagInput label="Tags" value={tags} onChange={setTags} placeholder="review, planning" />
        <div className="dialogField promptBodyField">
          <span>Prompt</span>
          <Suspense fallback={<div className="promptCodeMirrorEditor"><LoadingState label="Loading editor" /></div>}>
            <PromptBodyEditor value={body} onChange={setBody} />
          </Suspense>
        </div>
        {error ? <div className="dialogError">{error}</div> : null}
      </div>
      <DialogActionBar cancelDisabled={busy} onCancel={() => onOpenChange(false)}>
        <DialogStatefulButton
          state={busy ? "loading" : "idle"}
          loadingLabel="Saving prompt"
          variant="primary"
          className="dialogAdvanceButton"
          aria-label="Save prompt"
          disabled={!canSave}
          onClick={() => onSave({ id: prompt?.id, title, tags, body })}
        >
          <><span>Save</span><Save size={16} /></>
        </DialogStatefulButton>
      </DialogActionBar>
    </DialogShell>
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
  loadError?: string;
  hasRows?: boolean;
  onRefreshPrompts: () => void | Promise<void>;
  onPromptSaved?: (prompt: unknown) => void;
  onPromptsDeleted?: (ids: string[]) => void;
};

export function PromptsView({ prompts, loadingPrompts = false, loadError = "", hasRows = false, onRefreshPrompts, onPromptSaved, onPromptsDeleted }: PromptsViewProps) {
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
    onPromptSaved?.(result);
  };
  const copyPrompts = useCallback(async (items: PromptRecord[]) => {
    const text = items.map((prompt) => prompt.body).filter(Boolean).join("\n\n");
    if (!text) return false;
    await copyTextToClipboard(text);
    return true;
  }, []);
  const deleteSelected = useCallback(async (ids: string[]) => {
    if (ids.length === 0) return;
    const pendingIds = ids.filter((id) => !deletingPromptIds.includes(id));
    if (pendingIds.length === 0) return;
    setDeletingPromptIds((current) => Array.from(new Set([...current, ...pendingIds])));
    try {
      const result = await safeInvoke(TauriCommand.PromptsDeleteMany, { ids: pendingIds });
      if (!result) return;
      setSelected((current) => current.filter((id) => !pendingIds.includes(id)));
      onPromptsDeleted?.(pendingIds);
    } finally {
      setDeletingPromptIds((current) => current.filter((id) => !pendingIds.includes(id)));
    }
  }, [deletingPromptIds, onPromptsDeleted]);
  const columns = useMemo((): ColumnDef<PromptRecord>[] => [
    {
      key: "title",
      header: "Prompt",
      type: "text",
      sortValue: (prompt) => prompt.title?.toLowerCase() ?? "",
      width: "minmax(300px, 1fr)",
      render: (prompt) => (
        <>
          <Tooltip content={prompt.title} onlyWhenTruncated><span className="dataCellTitle">{prompt.title}</span></Tooltip>
          <span className="dataCellSubLine">
            <Tooltip content={prompt.body} onlyWhenTruncated><span className="dataCellSub">{promptPreview(prompt)}</span></Tooltip>
          </span>
        </>
      ),
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
          {prompt.tags.length
            ? prompt.tags.map((tag) => <Badge tone="accent" key={tag}>{tag}</Badge>)
            : <Badge tone="neutral">Untagged</Badge>}
        </span>
      ),
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
      width: "72px",
      render: (prompt) => {
        const isDeleting = deletingPromptIds.includes(prompt.id);
        return (
          <div className="rowActions">
            <CopyButton
              className="appButton appButton-icon"
              copyLabel={`Copy ${prompt.title}`}
              copiedLabel="Prompt copied"
              iconSize={15}
              stopPropagation
              onCopy={() => copyPrompts([prompt])}
            />
            <RowActionsMenu
              ariaLabel={`Prompt actions for ${prompt.title}`}
              onOpenChange={(open) => { if (!open) suppressNextClick(); }}
            >
              <PromptActionsMenuItems
                Menu={DropdownMenu}
                prompt={prompt}
                isDeleting={isDeleting}
                onEdit={openEditPrompt}
                onDelete={(id) => { void deleteSelected([id]); }}
              />
            </RowActionsMenu>
          </div>
        );
      },
    },
  ], [copyPrompts, deleteSelected, deletingPromptIds, openEditPrompt]);

  const bottomBar = useCallback((selectedRows: PromptRecord[]) => {
    const isDeleting = selectedRows.some((prompt) => deletingPromptIds.includes(prompt.id));
    return (
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
          className="danger promptsDeleteSelectedButton"
          aria-label="Delete selected prompts"
          disabled={isDeleting}
          onClick={() => deleteSelected(selectedRows.map((prompt) => prompt.id))}
        >
          <Trash2 size={15} />
          <span>Delete</span>
        </button>
      </>
    );
  }, [copyPrompts, deleteSelected, deletingPromptIds]);
  const rowContextMenu = useCallback((prompt: PromptRecord, { selectedRows, selected: isSelected }: { selectedRows: PromptRecord[]; selected: boolean }) => {
    const showBulk = isSelected && selectedRows.length > 1;
    return showBulk ? (
      <>
        <ContextMenu.Item className="skillMenuItem" onSelect={() => copyPrompts(selectedRows)}>
          <Copy size={14} />
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
        <PromptActionsMenuItems
          Menu={ContextMenu}
          prompt={prompt}
          isDeleting={deletingPromptIds.includes(prompt.id)}
          onEdit={openEditPrompt}
          onDelete={(id) => { void deleteSelected([id]); }}
        />
      </>
    );
  }, [copyPrompts, deleteSelected, deletingPromptIds, openEditPrompt]);

  return (
    <section className="content dataPage promptsPage">
      <ContentTopDragStrip />
      <PageHeader title="Prompts">
        <SearchField pageSearch placeholder="Search prompts" value={query} onChange={(event) => setQuery(event.target.value)} onClear={() => setQuery("")} />
        <IconButton aria-label="Refresh prompts" onClick={onRefreshPrompts}><RefreshCw size={16} /></IconButton>
        <IconButton className="filled" aria-label="Add prompt" onClick={openNewPrompt}><Plus size={16} /></IconButton>
      </PageHeader>
      {loadError && hasRows ? <LoadErrorState message={loadError} onRetry={() => { void onRefreshPrompts(); }} /> : null}
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
        loading={loadingPrompts && !hasRows}
        loadingLabel="Loading prompts"
        emptyState={loadError && !hasRows ? <LoadErrorState message={loadError} onRetry={() => { void onRefreshPrompts(); }} /> : (
          <EmptyState
            icon={normalizedQuery ? <SearchX size={21} strokeWidth={1.8} /> : <MessageSquareText size={29} strokeWidth={1.55} />}
            iconTone={normalizedQuery ? "muted" : "accent"}
            title={normalizedQuery ? "No prompts match this search" : "No prompts yet"}
            description={normalizedQuery ? "Try another search or clear the filter." : "Save reusable instructions for quick access."}
            action={(
              <Button
                size="sm"
                variant="ghost"
                onClick={normalizedQuery ? () => setQuery("") : openNewPrompt}
              >
                {normalizedQuery ? "Clear search" : <><Plus size={15} />Create prompt</>}
              </Button>
            )}
          />
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
