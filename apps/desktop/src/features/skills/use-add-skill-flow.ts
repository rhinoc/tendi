import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { isExistingSkillOperationStatus } from "../../controllers/skill-controller.ts";
import {
  captureSkillSourcePage,
  isDirectSkillSource,
  isSkillSourceActionReady,
  normalizeSkillAddPlan,
  restoreSkillSourcePage,
  skillSourceErrorMessage,
  type SkillSourcePageSnapshot,
} from "../../lib/add-skill-dialog.ts";
import type { AvailableSkill, SkillAddPlan, SkillInstallResult } from "../../lib/skills.ts";
import { SkillVisibility } from "../../lib/skills.ts";
import {
  installSkillAdd,
  previewSkillAdd,
  readSkillPreview,
  searchSkillMarketplace,
  SkillScope,
  type MarketplaceSource,
  type SkillPreviewReadResponse,
} from "../../lib/runtime-gateway.ts";

export enum SkillAddBusyAction {
  Idle = "",
  Preview = "preview",
  Install = "install",
}

export const recommendedSkillSources: MarketplaceSource[] = [
  {
    id: "mattpocock-skills",
    name: "Matt Pocock Skills",
    source: "mattpocock/skills",
    url: "https://github.com/mattpocock/skills",
    trustLabel: "Community",
    kind: "Collection",
  },
  {
    id: "claude-code-plugin-dev",
    name: "Claude Code Plugin Dev",
    source: "anthropics/claude-code/plugins/plugin-dev",
    url: "https://github.com/anthropics/claude-code/tree/main/plugins/plugin-dev",
    trustLabel: "Anthropic official",
    kind: "Collection",
  },
  {
    id: "emil-skills",
    name: "Emil's Design Skills",
    source: "emilkowalski/skills",
    url: "https://github.com/emilkowalski/skills",
    trustLabel: "Community",
    kind: "Collection",
  },
  {
    id: "vercel-agent-skills",
    name: "Vercel Agent Skills",
    source: "vercel-labs/agent-skills",
    url: "https://github.com/vercel-labs/agent-skills",
    trustLabel: "Vercel official",
    kind: "Collection",
  },
];

type UseAddSkillFlowOptions = {
  open: boolean;
  initialSource: string;
  lockedSource: boolean;
  source: string;
  onSourceChange: (source: string) => void;
  target: string;
  copy: boolean;
  visibility: SkillVisibility;
  replaceExisting: boolean;
  onPlanReady: (plan: SkillAddPlan) => void;
  onPageRestored: () => void;
  onSourceChanged: () => void;
  onPreviewError: (message: string) => void;
  onInstalled: (result: SkillInstallResult) => void;
  onBeginMutation: () => () => void;
  onRequestWrapper: (skills: Array<{ id: string; name: string; description?: string }>) => void;
  onClose: () => void;
};

export function useAddSkillFlow({
  open,
  initialSource,
  lockedSource,
  source,
  onSourceChange,
  target,
  copy,
  visibility,
  replaceExisting,
  onPlanReady,
  onPageRestored,
  onSourceChanged,
  onPreviewError,
  onInstalled,
  onBeginMutation,
  onRequestWrapper,
  onClose,
}: UseAddSkillFlowOptions) {
  const [sourcePageBeforePreview, setSourcePageBeforePreview] = useState<SkillSourcePageSnapshot<MarketplaceSource> | null>(null);
  const [marketplaceQuery, setMarketplaceQuery] = useState("");
  const [marketplaceResults, setMarketplaceResults] = useState<MarketplaceSource[]>([]);
  const [marketplaceBusy, setMarketplaceBusy] = useState(false);
  const [marketplaceError, setMarketplaceError] = useState("");
  const [marketplaceNotice, setMarketplaceNotice] = useState("");
  const [skillPreview, setSkillPreview] = useState<SkillPreviewReadResponse | null>(null);
  const [skillPreviewBusy, setSkillPreviewBusy] = useState("");
  const [skillPreviewError, setSkillPreviewError] = useState("");
  const [plan, setPlan] = useState<SkillAddPlan | null>(null);
  const [busyAction, setBusyAction] = useState<SkillAddBusyAction>(SkillAddBusyAction.Idle);
  const [error, setError] = useState("");
  const operationRevisionRef = useRef(0);
  const previewReadRevisionRef = useRef(0);
  const previewIdRef = useRef("");
  const initialPreviewRequestRef = useRef("");

  const busy = busyAction !== SkillAddBusyAction.Idle;
  const installing = busyAction === SkillAddBusyAction.Install;
  const dialogBusy = busy || marketplaceBusy || Boolean(skillPreviewBusy);
  const directSource = isDirectSkillSource(source);
  const sourceActionReady = isSkillSourceActionReady(source, directSource, target);
  const sourceCandidates = source.trim() ? marketplaceResults : recommendedSkillSources;
  const sourceCandidatesLabel = source.trim() ? "Matches" : "Recommended";
  const sourceActionText = directSource ? "Scan" : "Search";
  const sourceActionLabel = directSource ? "Scan repository" : "Search marketplaces";
  const sourceEmptyTitle = directSource ? "Scan a repository" : "Search for a skill";
  const sourceEmptyDescription = directSource
    ? "Click Scan to inspect available skills."
    : "Click Search to find matching skills.";

  const nextOperationRevision = useCallback(() => {
    operationRevisionRef.current += 1;
    return operationRevisionRef.current;
  }, []);
  const isCurrentOperation = useCallback((revision: number) => operationRevisionRef.current === revision, []);
  const invalidatePendingRequests = useCallback(() => {
    operationRevisionRef.current += 1;
    previewReadRevisionRef.current += 1;
    setMarketplaceBusy(false);
    setSkillPreviewBusy("");
  }, []);

  const setActivePreviewId = useCallback((value: string) => {
    previewIdRef.current = value;
    previewReadRevisionRef.current += 1;
    setSkillPreviewBusy("");
    setSkillPreview(null);
    setSkillPreviewError("");
  }, []);

  const clearTransientErrors = useCallback(() => {
    setError("");
    setMarketplaceError("");
    setMarketplaceNotice("");
    setSkillPreviewError("");
  }, []);

  const clearPlan = useCallback(() => {
    setPlan(null);
    setActivePreviewId("");
  }, [setActivePreviewId]);

  const handleSourceChange = useCallback((value: string) => {
    if (marketplaceBusy || lockedSource) return;
    invalidatePendingRequests();
    onSourceChange(value);
    setSourcePageBeforePreview(null);
    setMarketplaceQuery(value);
    setMarketplaceResults([]);
    setMarketplaceError("");
    setMarketplaceNotice("");
    clearPlan();
    setSkillPreview(null);
    setSkillPreviewError("");
    setBusyAction(SkillAddBusyAction.Idle);
    setError("");
    onSourceChanged();
  }, [clearPlan, invalidatePendingRequests, lockedSource, marketplaceBusy, onSourceChange, onSourceChanged]);

  const searchMarketplace = useCallback(async (rawQuery = source) => {
    const query = rawQuery.trim();
    if (query.length < 2 || marketplaceBusy || busy) return;
    const revision = nextOperationRevision();
    setMarketplaceQuery(query);
    setMarketplaceBusy(true);
    setMarketplaceError("");
    setMarketplaceNotice("");
    clearPlan();
    setError("");
    try {
      const response = await searchSkillMarketplace(query);
      if (!isCurrentOperation(revision)) return;
      setMarketplaceResults(response.items);
      setMarketplaceNotice(response.warnings.length
        ? "Some marketplaces are unavailable."
        : response.items.length ? "" : "No matching skills.");
    } catch (searchError) {
      if (!isCurrentOperation(revision)) return;
      setMarketplaceResults([]);
      setMarketplaceError(String(searchError));
    } finally {
      if (isCurrentOperation(revision)) setMarketplaceBusy(false);
    }
  }, [busy, clearPlan, isCurrentOperation, marketplaceBusy, nextOperationRevision, source]);

  const restorePage = useCallback((page: SkillSourcePageSnapshot<MarketplaceSource>) => {
    invalidatePendingRequests();
    const restoredPage = restoreSkillSourcePage(page);
    clearPlan();
    setSkillPreview(null);
    setSkillPreviewError("");
    onSourceChange(restoredPage.source);
    setMarketplaceQuery(restoredPage.marketplaceQuery);
    setMarketplaceResults(restoredPage.marketplaceResults);
    setSourcePageBeforePreview(null);
    clearTransientErrors();
    setBusyAction(SkillAddBusyAction.Idle);
    onPageRestored();
  }, [clearPlan, clearTransientErrors, invalidatePendingRequests, onPageRestored, onSourceChange]);

  const previewSource = useCallback(async (nextSource = source) => {
    const normalizedSource = nextSource.trim();
    if (!normalizedSource || !target || busy || marketplaceBusy) return;
    const previousPage = captureSkillSourcePage(source, marketplaceResults);
    const revision = nextOperationRevision();
    setSourcePageBeforePreview(previousPage);
    onSourceChange(normalizedSource);
    setMarketplaceError("");
    setMarketplaceNotice("");
    setSkillPreview(null);
    setSkillPreviewError("");
    setBusyAction(SkillAddBusyAction.Preview);
    setError("");
    try {
      const response = await previewSkillAdd({
        source: normalizedSource,
        target,
        scope: SkillScope.Global,
        skills: [],
        copy,
        overwrite: false,
        visibility,
        dryRun: true,
      });
      if (!isCurrentOperation(revision)) return;
      if (!response?.plan || !response.previewId) {
        throw new Error("Skill preview returned no data. Restart the development service and try again.");
      }
      const nextPlan = normalizeSkillAddPlan(response.plan);
      if (!nextPlan) throw new Error("Skill preview returned an invalid plan. Restart the development service and try again.");
      setPlan(nextPlan);
      setActivePreviewId(response.previewId);
      setBusyAction(SkillAddBusyAction.Idle);
      onPlanReady(nextPlan);
    } catch (previewError) {
      if (!isCurrentOperation(revision)) return;
      const message = skillSourceErrorMessage(previewError);
      if (lockedSource) {
        onSourceChange(initialSource);
        setError(message);
      } else {
        restorePage(previousPage);
      }
      onPreviewError(message);
    } finally {
      if (isCurrentOperation(revision)) setBusyAction(SkillAddBusyAction.Idle);
    }
  }, [busy, copy, initialSource, isCurrentOperation, lockedSource, marketplaceBusy, marketplaceResults, nextOperationRevision, onPlanReady, onPreviewError, onSourceChange, restorePage, source, target, visibility, setActivePreviewId]);

  const resolveSourceInput = useCallback(() => {
    const value = source.trim();
    if (!isSkillSourceActionReady(value, isDirectSkillSource(value), target) || busy || marketplaceBusy) return;
    if (isDirectSkillSource(value)) void previewSource(value);
    else void searchMarketplace(value);
  }, [busy, marketplaceBusy, previewSource, searchMarketplace, source, target]);

  const selectMarketplaceSource = useCallback((marketplaceSource: MarketplaceSource) => {
    void previewSource(marketplaceSource.source);
  }, [previewSource]);

  const previewSkill = useCallback(async (name: string) => {
    const activePreviewId = previewIdRef.current;
    if (!activePreviewId || skillPreviewBusy || busy) return;
    const revision = ++previewReadRevisionRef.current;
    setSkillPreviewBusy(name);
    setSkillPreviewError("");
    try {
      const preview = await readSkillPreview(activePreviewId, name);
      if (revision !== previewReadRevisionRef.current || activePreviewId !== previewIdRef.current) return;
      setSkillPreview(preview);
    } catch (previewError) {
      if (revision !== previewReadRevisionRef.current || activePreviewId !== previewIdRef.current) return;
      setSkillPreview(null);
      setSkillPreviewError(`${previewError}`);
    } finally {
      if (revision === previewReadRevisionRef.current && activePreviewId === previewIdRef.current) setSkillPreviewBusy("");
    }
  }, [busy, skillPreviewBusy]);

  const clearSkillPreview = useCallback(() => {
    previewReadRevisionRef.current += 1;
    setSkillPreview(null);
    setSkillPreviewBusy("");
    setSkillPreviewError("");
  }, []);

  const restorePreviousPage = useCallback(() => {
    if (busy || marketplaceBusy || skillPreviewBusy) return;
    restorePage(sourcePageBeforePreview ?? captureSkillSourcePage(marketplaceQuery || source, marketplaceResults));
  }, [busy, marketplaceBusy, marketplaceQuery, marketplaceResults, restorePage, skillPreviewBusy, source, sourcePageBeforePreview]);

  const reset = useCallback(() => {
    invalidatePendingRequests();
    onSourceChange(initialSource);
    setSourcePageBeforePreview(null);
    setMarketplaceQuery("");
    setMarketplaceResults([]);
    setMarketplaceError("");
    setMarketplaceNotice("");
    setSkillPreview(null);
    setSkillPreviewError("");
    clearPlan();
    setBusyAction(SkillAddBusyAction.Idle);
    clearTransientErrors();
    initialPreviewRequestRef.current = "";
  }, [clearPlan, clearTransientErrors, initialSource, invalidatePendingRequests, onSourceChange]);

  const install = useCallback(async ({
    selected,
    selectedRoots,
    available,
    createWrapper,
  }: {
    selected: string[];
    selectedRoots: string[];
    available: AvailableSkill[];
    createWrapper: boolean;
  }) => {
    if (!plan || !target || selected.length === 0 || busy || marketplaceBusy || skillPreviewBusy) return;
    const revision = nextOperationRevision();
    const normalizedSource = source.trim();
    setError("");
    setBusyAction(SkillAddBusyAction.Preview);
    try {
      const previewResponse = await previewSkillAdd({
        source: normalizedSource,
        target,
        scope: SkillScope.Global,
        skills: selected,
        copy,
        overwrite: replaceExisting,
        visibility,
        dryRun: true,
      });
      if (!isCurrentOperation(revision)) return;
      if (!previewResponse?.plan || !previewResponse.previewId) {
        throw new Error("Skill preview returned no data. Restart the development service and try again.");
      }
      const finalPlan = normalizeSkillAddPlan(previewResponse.plan);
      if (!finalPlan) {
        throw new Error("Skill preview returned an invalid plan. Restart the development service and try again.");
      }
      setPlan(finalPlan);
      setActivePreviewId(previewResponse.previewId);
      const finalOperationByName = new Map(finalPlan.operations.map((operation) => [operation.name, operation]));
      const finalSelectedHasExisting = selected.some((name) => isExistingSkillOperationStatus(finalOperationByName.get(name)?.status));
      if (finalSelectedHasExisting && !replaceExisting) {
        throw new Error("Some selected skills already exist at this destination. Choose Replace existing skills and try again.");
      }
      setBusyAction(SkillAddBusyAction.Install);
      const releaseMutation = onBeginMutation();
      try {
        const result = await installSkillAdd({
          source: normalizedSource,
          target,
          scope: SkillScope.Global,
          skills: selected,
          copy,
          overwrite: replaceExisting,
          visibility,
          previewId: previewResponse.previewId,
          dryRun: false,
        });
        onInstalled(result);
        if (createWrapper && selectedRoots.length > 1) {
          const selectedRootSet = new Set(selectedRoots);
          onRequestWrapper(available
            .filter((skill) => selectedRootSet.has(skill.name))
            .map((skill) => ({
              id: skill.name,
              name: skill.name,
              description: skill.description,
            })));
        }
        onClose();
      } finally {
        releaseMutation();
      }
    } catch (installError) {
      if (isCurrentOperation(revision)) setError(`${installError}`);
    } finally {
      if (isCurrentOperation(revision)) setBusyAction(SkillAddBusyAction.Idle);
    }
  }, [busy, copy, isCurrentOperation, marketplaceBusy, nextOperationRevision, onBeginMutation, onClose, onInstalled, onRequestWrapper, plan, replaceExisting, skillPreviewBusy, source, target, visibility, setActivePreviewId]);

  useEffect(() => {
    if (!open || !lockedSource || !initialSource.trim() || plan || busyAction || marketplaceBusy || !target) return;
    if (initialPreviewRequestRef.current === initialSource) return;
    initialPreviewRequestRef.current = initialSource;
    void previewSource(initialSource);
  }, [busyAction, initialSource, lockedSource, marketplaceBusy, open, plan, previewSource, target]);

  useEffect(() => {
    if (open || busyAction === SkillAddBusyAction.Install) return;
    if (busyAction === SkillAddBusyAction.Idle && !marketplaceBusy && !skillPreviewBusy) return;
    invalidatePendingRequests();
    setBusyAction(SkillAddBusyAction.Idle);
  }, [busyAction, invalidatePendingRequests, marketplaceBusy, open, skillPreviewBusy]);

  useEffect(() => () => {
    operationRevisionRef.current += 1;
    previewReadRevisionRef.current += 1;
  }, []);

  return useMemo(() => ({
    marketplaceBusy,
    marketplaceError,
    marketplaceNotice,
    skillPreview,
    skillPreviewBusy,
    skillPreviewError,
    plan,
    busyAction,
    error,
    busy,
    installing,
    dialogBusy,
    sourceActionReady,
    sourceActionText,
    sourceActionLabel,
    sourceEmptyTitle,
    sourceEmptyDescription,
    sourceCandidates,
    sourceCandidatesLabel,
    canGoBack: Boolean(plan) || sourcePageBeforePreview !== null,
    handleSourceChange,
    previewSource,
    resolveSourceInput,
    selectMarketplaceSource,
    previewSkill,
    clearSkillPreview,
    restorePreviousPage,
    reset,
    install,
  }), [
    busy,
    busyAction,
    clearSkillPreview,
    dialogBusy,
    error,
    handleSourceChange,
    install,
    installing,
    marketplaceBusy,
    marketplaceError,
    marketplaceNotice,
    plan,
    previewSkill,
    previewSource,
    reset,
    resolveSourceInput,
    restorePreviousPage,
    selectMarketplaceSource,
    skillPreview,
    skillPreviewBusy,
    skillPreviewError,
    sourceActionLabel,
    sourceActionReady,
    sourceActionText,
    sourceCandidates,
    sourceCandidatesLabel,
    sourceEmptyDescription,
    sourceEmptyTitle,
    sourcePageBeforePreview,
  ]);
}
