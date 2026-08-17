import { useCallback, useDeferredValue, useEffect, useMemo, useState } from "react";
import { FolderOpen } from "lucide-react";

import { TauriCommand, diffPreview, ruleTitle, safeInvoke } from "../../lib/index.ts";
import { DiscardChangesDialog } from "../../components/shared/DiscardChangesDialog.tsx";
import { EditorStatePlaceholder } from "../../components/shared/EditorStatePlaceholder.tsx";
import { EditorHeader } from "../../components/shared/EditorHeader.tsx";
import { MarkdownFilePane, type DiffStats } from "../../components/shared/MarkdownFilePane.tsx";

export type RuleEditorRecord = {
  path?: string;
  sha256?: string;
  agent?: string;
  kind?: string;
  scope?: string;
};

export type RuleEditorViewProps = {
  rule?: RuleEditorRecord | null;
  back: () => void;
};

type RuleFileResult = {
  content?: string;
  sha256?: string;
};

export function RuleEditorView({ rule, back }: RuleEditorViewProps) {
  const currentRule = rule ?? {};
  const activePath = ruleTitle(currentRule);
  const [draft, setDraft] = useState({ content: "", originalContent: "", sha256: currentRule.sha256 ?? "" });
  const [showDiscardDialog, setShowDiscardDialog] = useState(false);
  const [loadingRule, setLoadingRule] = useState(Boolean(currentRule.path));
  const content = draft.content;
  const deferredContent = useDeferredValue(content);
  const contentReady = !loadingRule && (!currentRule.path || Boolean(draft.sha256));
  const dirty = content !== draft.originalContent;
  const diffLines = useMemo(
    () => diffPreview(draft.originalContent, deferredContent),
    [deferredContent, draft.originalContent],
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

  useEffect(() => {
    let cancelled = false;
    let frame = 0;
    let timer = 0;
    async function loadRule() {
      if (!currentRule.path) {
        setLoadingRule(false);
        return;
      }
      setLoadingRule(true);
      setDraft({ content: "", originalContent: "", sha256: currentRule.sha256 ?? "" });
      const result = await safeInvoke(TauriCommand.RuleFileRead, { path: currentRule.path }) as RuleFileResult | null;
      if (cancelled) return;
      if (typeof result?.content === "string") {
        setDraft({ content: result.content, originalContent: result.content, sha256: result.sha256 ?? "" });
      }
      setLoadingRule(false);
    }
    frame = window.requestAnimationFrame(() => {
      timer = window.setTimeout(loadRule, 0);
    });
    return () => {
      cancelled = true;
      window.cancelAnimationFrame(frame);
      window.clearTimeout(timer);
      setLoadingRule(false);
    };
  }, [currentRule.path, currentRule.sha256]);

  const save = useCallback(async () => {
    if (!dirty || !draft.sha256 || !currentRule.path) return;
    const result = await safeInvoke(TauriCommand.RuleFileSave, {
      path: currentRule.path,
      expectedSha256: draft.sha256,
      content,
    }) as RuleFileResult | null;
    if (result?.sha256) {
      setDraft({ content: result.content ?? "", originalContent: result.content ?? "", sha256: result.sha256 ?? "" });
    }
  }, [content, currentRule.path, dirty, draft.sha256]);

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

  const handleBack = () => {
    if (dirty) {
      setShowDiscardDialog(true);
      return;
    }
    back();
  };

  return (
    <section className="editorPage">
      <EditorHeader
        title={ruleTitle(currentRule)}
        backLabel="Back to rules"
        onBack={handleBack}
        actions={(
          <button
            className="headerGhostButton"
            aria-label="Reveal rule in Finder"
            onClick={() => currentRule.path && safeInvoke(TauriCommand.RevealInFinder, { path: currentRule.path })}
          >
            <FolderOpen size={15} />
          </button>
        )}
      />
      <DiscardChangesDialog open={showDiscardDialog} onOpenChange={setShowDiscardDialog} onDiscard={back} />
      <section className="editorShell singlePaneEditor">
        {contentReady ? (
          <MarkdownFilePane
            activePath={currentRule.path ?? activePath}
            dirty={dirty}
            diffStats={diffStats}
            content={content}
            originalContent={draft.originalContent}
            onSave={save}
            onChange={(nextContent: string) => setDraft((current) => ({ ...current, content: nextContent }))}
            onNormalize={(nextContent: string) => {
              setDraft((current) => {
                if (current.content !== current.originalContent || current.content === nextContent) return current;
                return { ...current, content: nextContent, originalContent: nextContent };
              });
            }}
          />
        ) : (
          <EditorStatePlaceholder label={loadingRule ? "Loading rule" : undefined}>
            {loadingRule ? null : "Select a rule"}
          </EditorStatePlaceholder>
        )}
      </section>
    </section>
  );
}
