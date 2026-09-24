import { useCallback, useEffect, useRef, useState } from "react";

import {
  getEditorDraft,
  updateEditorDraft,
} from "../../lib/editor-draft-state.ts";
import { hydrateEditorDraft, isEditorDraftDirty, type EditorDraft } from "../../lib/editor-draft-logic.ts";
import type { RuleRecord } from "../../lib/rules.ts";
import { readRule, saveRule } from "../../lib/runtime-gateway.ts";

type UseRuleEditorOptions = {
  rule: RuleRecord | null;
  resourceKey: string;
  draft: EditorDraft;
  onBeginMutation: () => () => void;
  onRuleSaved?: (path: string, sha256: string) => void;
};

export function useRuleEditor({ rule, resourceKey, draft, onBeginMutation, onRuleSaved }: UseRuleEditorOptions) {
  const [loading, setLoading] = useState(false);
  const [loadFailed, setLoadFailed] = useState(false);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const loadedRulePathRef = useRef("");
  const dirty = isEditorDraftDirty(draft);
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;
  const draftRef = useRef(draft);
  draftRef.current = draft;

  const hasLoadedRule = Boolean(
    rule
      && (loadedRulePathRef.current === rule.path
        || isEditorDraftDirty(draft) && Boolean(draft.sha256)),
  );
  const retryLoad = useCallback(() => setLoadAttempt((attempt) => attempt + 1), []);

  useEffect(() => {
    let cancelled = false;
    const rulePath = rule?.path ?? "";
    if (!rulePath) {
      loadedRulePathRef.current = "";
      setLoadFailed(false);
      setLoading(false);
      return () => { cancelled = true; };
    }

    if (dirtyRef.current && draftRef.current.sha256) {
      loadedRulePathRef.current = rulePath;
      setLoadFailed(false);
      setLoading(false);
      return () => { cancelled = true; };
    }

    const sameRule = loadedRulePathRef.current === rulePath;
    if (!sameRule) loadedRulePathRef.current = "";
    setLoading(true);
    setLoadFailed(false);

    async function loadRule() {
      try {
        const result = await readRule(rulePath);
        if (cancelled) return;
        loadedRulePathRef.current = rulePath;
        updateEditorDraft(
          resourceKey,
          hydrateEditorDraft(getEditorDraft(resourceKey), result.content, result.sha256),
        );
        setLoadFailed(false);
      } catch {
        if (cancelled) return;
        if (!sameRule) loadedRulePathRef.current = "";
        setLoadFailed(true);
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    void loadRule();
    return () => { cancelled = true; };
  }, [resourceKey, rule?.path, rule?.sha256, loadAttempt]);

  const save = useCallback(async () => {
    const rulePath = rule?.path;
    if (!dirty || !draft.sha256 || !rulePath) return;
    const releaseMutation = onBeginMutation();
    try {
      const result = await saveRule({
        path: rulePath,
        expectedSha256: draft.sha256,
        content: draft.content,
      });
      if (typeof result?.sha256 === "string") {
        const savedContent = typeof result.content === "string" ? result.content : draft.content;
        updateEditorDraft(resourceKey, { content: savedContent, originalContent: savedContent, sha256: result.sha256 });
        onRuleSaved?.(rulePath, result.sha256);
      }
    } finally {
      releaseMutation();
    }
  }, [draft.content, draft.sha256, dirty, onBeginMutation, onRuleSaved, resourceKey, rule?.path]);

  return { hasLoadedRule, loading, loadFailed, retryLoad, save };
}
