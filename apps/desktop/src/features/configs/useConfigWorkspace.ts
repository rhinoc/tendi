import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";

import {
  clearEditorDraft,
  getEditorDraft,
  updateEditorDraft,
} from "../../lib/editor-draft-state.ts";
import { hydrateEditorDraft, isEditorDraftDirty, reconcileExternalEditorDraft } from "../../lib/editor-draft-logic.ts";
import { RuntimeEventName } from "../../lib/generated/runtime-events.ts";
import { DaemonCommandError, logger, normalizeConfigProfiles, subscribeDaemonEvents } from "../../lib/index.ts";
import type { DaemonEvent } from "../../lib/index.ts";
import {
  createConfigProfile,
  deleteAgentConfigs,
  readAgentConfig,
  readAgentConfigs,
  saveAgentConfig,
  setConfigProfile,
  watchAgentConfig,
} from "../../lib/runtime-gateway.ts";
import type {
  AgentConfigContentResponse,
  AgentConfigFileResponse,
} from "../../lib/runtime-gateway.ts";
import { errorMessage, isConfigSnapshot, isConflictMarkerContent } from "./config-logic.ts";

export type ConfigConflict = {
  base: string;
  local: string;
  disk: string;
  diskSha256: string;
  diskExists: boolean;
  diskUpdatedAt?: string;
  merged: boolean;
};

type UseConfigWorkspaceOptions = {
  activePath: string;
  setActivePath: Dispatch<SetStateAction<string>>;
  setSelectedPath: Dispatch<SetStateAction<string>>;
  onConfigSelected?: (config?: AgentConfigFileResponse) => void;
  onConfigRowsChange?: (rows: AgentConfigFileResponse[]) => void;
  activeProfiles: Record<string, string>;
  onActiveProfilesChange: (profiles: Record<string, string>) => void;
};

export function useConfigWorkspace({
  activePath,
  setActivePath,
  setSelectedPath,
  onConfigSelected,
  onConfigRowsChange,
  activeProfiles,
  onActiveProfilesChange,
}: UseConfigWorkspaceOptions) {
  const [configs, setConfigs] = useState<AgentConfigFileResponse[]>([]);
  const [loadingConfig, setLoadingConfig] = useState(true);
  const [loadingConfigs, setLoadingConfigs] = useState(true);
  const [loadError, setLoadError] = useState("");
  const [contentError, setContentError] = useState("");
  const [conflict, setConflict] = useState<ConfigConflict | null>(null);
  const [saveError, setSaveError] = useState("");
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState("");
  const [profileSaving, setProfileSaving] = useState(false);
  const [profileError, setProfileError] = useState("");
  const [profileSwitching, setProfileSwitching] = useState(false);
  const readRequestRef = useRef(0);
  const externalReadRequestRef = useRef(0);
  const configListRequestRef = useRef(0);
  const configListRevisionRef = useRef(0);
  const configListInFlightRef = useRef(false);
  const configReadInFlightRef = useRef(false);
  const configMutationDepthRef = useRef(0);
  const configRefreshQueuedRef = useRef(false);
  const loadConfigsRef = useRef<() => Promise<void>>(() => Promise.resolve());
  const activePathRef = useRef(activePath);
  activePathRef.current = activePath;
  const activeProfilesRef = useRef(activeProfiles);
  activeProfilesRef.current = activeProfiles;
  const profileRevisionRef = useRef<Record<string, number>>({});
  const filterDeletableConfigs = useCallback(<T extends Pick<AgentConfigFileResponse, "exists">>(targets: readonly T[]): T[] => (
    targets.filter((config) => config.exists)
  ), []);

  const applyExternalSnapshot = useCallback((next: AgentConfigContentResponse) => {
    setConfigs((current) => current.map((config) => (
      config.path === next.path ? { ...config, exists: next.exists, updatedAt: next.updatedAt } : config
    )));
    if (next.path !== activePathRef.current) return;
    const resourceKey = `configs:${next.path}`;
    const change = reconcileExternalEditorDraft(getEditorDraft(resourceKey), next);
    if (change.kind === "unchanged") return;
    if (change.kind === "updated") {
      updateEditorDraft(resourceKey, change.draft);
      setSaveError("");
    } else {
      setConflict({
        base: change.draft.originalContent,
        local: change.draft.content,
        disk: next.content,
        diskSha256: next.sha256,
        diskExists: next.exists,
        diskUpdatedAt: next.updatedAt,
        merged: false,
      });
      setSaveError("");
    }
  }, []);

  const readExternalSnapshot = useCallback(async (path: string) => {
    const isActivePath = path === activePathRef.current;
    const requestId = isActivePath ? ++readRequestRef.current : ++externalReadRequestRef.current;
    configReadInFlightRef.current = true;
    const isCurrent = () => isActivePath
      ? requestId === readRequestRef.current
      : requestId === externalReadRequestRef.current;
    try {
      const next = await readAgentConfig(path);
      if (!isCurrent()) return;
      if (isActivePath) {
        applyExternalSnapshot(next);
      } else {
        setConfigs((current) => current.map((config) => (
          config.path === next.path ? { ...config, exists: next.exists, updatedAt: next.updatedAt } : config
        )));
      }
    } catch (error) {
      if (isCurrent() && isActivePath) setSaveError(errorMessage(error));
    } finally {
      if (isCurrent()) configReadInFlightRef.current = false;
    }
  }, [applyExternalSnapshot]);

  const readConfig = useCallback(async (path: string, config?: AgentConfigFileResponse) => {
    const requestId = ++readRequestRef.current;
    configReadInFlightRef.current = true;
    setSelectedPath(path);
    setLoadingConfig(true);
    setContentError("");
    try {
      const next = await readAgentConfig(path);
      if (requestId !== readRequestRef.current) return;
      setSelectedPath(next.path);
      setActivePath(next.path);
      onConfigSelected?.(config);
      const resourceKey = `configs:${next.path}`;
      const change = reconcileExternalEditorDraft(getEditorDraft(resourceKey), next);
      updateEditorDraft(resourceKey, change.kind === "updated"
        ? change.draft
        : hydrateEditorDraft(change.draft, next.content, next.sha256));
      setConflict(change.kind === "conflict" ? {
        base: change.draft.originalContent,
        local: change.draft.content,
        disk: change.snapshot.content,
        diskSha256: change.snapshot.sha256,
        diskExists: next.exists,
        diskUpdatedAt: next.updatedAt,
        merged: false,
      } : null);
      setSaveError("");
      setConfigs((current) => current.map((item) => (
        item.path === next.path ? { ...item, exists: next.exists, updatedAt: next.updatedAt } : item
      )));
    } catch (error) {
      if (requestId !== readRequestRef.current) return;
      setContentError(errorMessage(error));
    } finally {
      if (requestId === readRequestRef.current) {
        configReadInFlightRef.current = false;
        setLoadingConfig(false);
      }
    }
  }, [onConfigSelected, setActivePath, setSelectedPath]);

  const loadConfigs = useCallback(async () => {
    if (configMutationDepthRef.current > 0) {
      configRefreshQueuedRef.current = true;
      return;
    }
    const requestId = ++configListRequestRef.current;
    const snapshotRevision = configListRevisionRef.current;
    configListInFlightRef.current = true;
    setLoadingConfigs(true);
    setLoadingConfig(true);
    setLoadError("");
    try {
      const next = await readAgentConfigs();
      if (requestId !== configListRequestRef.current || snapshotRevision !== configListRevisionRef.current || configMutationDepthRef.current > 0) return;
      setConfigs(next);
      onConfigRowsChange?.(next);
      const selected = next.find((config) => config.path === activePath) ?? next[0];
      if (selected) {
        onConfigSelected?.(selected);
        await readConfig(selected.path, selected);
      } else {
        readRequestRef.current += 1;
        if (activePathRef.current) clearEditorDraft(`configs:${activePathRef.current}`);
        setActivePath("");
        setSelectedPath("");
        setConflict(null);
        setSaveError("");
        setContentError("");
        setLoadingConfig(false);
      }
    } catch (error) {
      if (requestId === configListRequestRef.current) {
        setLoadError(errorMessage(error));
        setLoadingConfig(false);
      }
    } finally {
      if (requestId === configListRequestRef.current) {
        configListInFlightRef.current = false;
        setLoadingConfigs(false);
      }
    }
  }, [activePath, onConfigRowsChange, onConfigSelected, readConfig, setActivePath, setSelectedPath]);
  loadConfigsRef.current = loadConfigs;

  const beginConfigMutation = useCallback(() => {
    if (configMutationDepthRef.current === 0 && configListInFlightRef.current) {
      configRefreshQueuedRef.current = true;
    }
    configMutationDepthRef.current += 1;
    configListRevisionRef.current += 1;
    // Cancel both a stale config list and an in-flight editor read. Their
    // responses were started before this mutation and cannot be authoritative.
    configListRequestRef.current += 1;
    externalReadRequestRef.current += 1;
    if (configReadInFlightRef.current) {
      configReadInFlightRef.current = false;
      setLoadingConfig(false);
    }
    readRequestRef.current += 1;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      configMutationDepthRef.current -= 1;
      if (configMutationDepthRef.current === 0 && configRefreshQueuedRef.current) {
        configRefreshQueuedRef.current = false;
        void loadConfigsRef.current();
      }
    };
  }, []);

  const invalidatePendingRead = useCallback(() => {
    readRequestRef.current += 1;
  }, []);

  const beginProfileChange = useCallback((agents: string[]) => Object.fromEntries(agents.map((agent) => {
    const revision = (profileRevisionRef.current[agent] ?? 0) + 1;
    profileRevisionRef.current[agent] = revision;
    return [agent, revision];
  })), []);

  const updateActiveProfiles = useCallback((next: Record<string, string>, ticket: Record<string, number>) => {
    const merged = { ...activeProfilesRef.current };
    for (const [agent, revision] of Object.entries(ticket)) {
      if (profileRevisionRef.current[agent] !== revision) continue;
      if (next[agent] === undefined) delete merged[agent];
      else merged[agent] = next[agent];
    }
    activeProfilesRef.current = merged;
    onActiveProfilesChange(merged);
  }, [onActiveProfilesChange]);

  const saveContent = useCallback(async (path: string, content: string, expectedSha256: string) => {
    if (saving) return false;
    setSaving(true);
    setSaveError("");
    const releaseMutation = beginConfigMutation();
    try {
      const saved = await saveAgentConfig({ path, expectedSha256, content });
      const savedContent = typeof saved.content === "string" ? saved.content : content;
      updateEditorDraft(`configs:${saved.path}`, {
        content: savedContent,
        originalContent: savedContent,
        sha256: saved.sha256,
      });
      setConflict(null);
      setConfigs((current) => current.map((config) => (
        config.path === saved.path ? { ...config, exists: true, updatedAt: saved.updatedAt } : config
      )));
      return true;
    } catch (error) {
      logger.error("failed to save config", { error: errorMessage(error), path });
      if (error instanceof DaemonCommandError && error.code === "CONFLICT") {
        if (isConfigSnapshot(error.data)) applyExternalSnapshot(error.data);
        else await readExternalSnapshot(path);
        setSaveError("Source file changed on disk. Review the conflict before saving.");
      } else {
        setSaveError(errorMessage(error));
      }
      return false;
    } finally {
      releaseMutation();
      setSaving(false);
    }
  }, [applyExternalSnapshot, beginConfigMutation, readExternalSnapshot, saving]);

  const saveCurrent = useCallback(async () => {
    const activeConfig = configs.find((config) => config.path === activePath);
    const draft = getEditorDraft(`configs:${activePath}`);
    if (!activeConfig || !isEditorDraftDirty(draft) || saving || conflict || isConflictMarkerContent(draft.content)) return;
    await saveContent(activeConfig.path, draft.content, draft.sha256);
  }, [activePath, configs, conflict, saveContent, saving]);

  const activateProfile = useCallback(async (agent: string, profile: string | null) => {
    if (profileSwitching) return;
    const target = configs.find((config) => (
      config.agent === agent && (profile ? config.profile === profile : !config.profile)
    ));
    if (!target) {
      logger.error("config profile not found", { action: "activate_profile" });
      return;
    }
    if (isEditorDraftDirty(getEditorDraft(`configs:${activePath}`))) return;
    setProfileSwitching(true);
    const profileTicket = beginProfileChange([agent]);
    const releaseMutation = beginConfigMutation();
    try {
      const next = await setConfigProfile(agent, profile);
      updateActiveProfiles(next.configProfiles, profileTicket);
      if (target.path !== activePath) await readConfig(target.path, target);
    } catch (error) {
      logger.error("failed to activate config profile", { error: errorMessage(error), agent });
    } finally {
      releaseMutation();
      setProfileSwitching(false);
    }
  }, [activePath, beginConfigMutation, beginProfileChange, configs, profileSwitching, readConfig, updateActiveProfiles]);

  const createProfile = useCallback(async (name: string) => {
    const profileAgent = configs.find((config) => config.path === activePath)?.agent;
    if (!profileAgent) return undefined;
    const normalizedName = name.trim();
    if (!normalizedName) {
      setProfileError("Enter a profile name");
      return undefined;
    }
    setProfileSaving(true);
    setProfileError("");
    const releaseMutation = beginConfigMutation();
    try {
      const created = await createConfigProfile({
        agent: profileAgent,
        name: normalizedName,
        content: getEditorDraft(`configs:${activePath}`).content,
      });
      setConfigs((current) => {
        if (current.some((config) => config.path === created.path)) {
          return current.map((config) => (config.path === created.path ? { ...config, ...created } : config));
        }
        return [...current, created];
      });
      return created;
    } catch (error) {
      setProfileError(errorMessage(error));
      return undefined;
    } finally {
      releaseMutation();
      setProfileSaving(false);
    }
  }, [activePath, beginConfigMutation, configs]);

  const deleteConfigs = useCallback(async (
    targets: AgentConfigFileResponse[],
    onDeleteApplied?: () => void,
  ) => {
    const deletableTargets = filterDeletableConfigs(targets);
    if (deletableTargets.length === 0 || deleting) return;
    setDeleting(true);
    setDeleteError("");
    const profileTicket = beginProfileChange([...new Set(deletableTargets.filter((config) => config.profile).map((config) => config.agent))]);
    const releaseMutation = beginConfigMutation();
    try {
      const result = await deleteAgentConfigs(deletableTargets.map((config) => config.path));
      onDeleteApplied?.();
      updateActiveProfiles(normalizeConfigProfiles(result.configProfiles), profileTicket);
      if (Array.isArray(result.configs)) {
        setConfigs(result.configs);
        onConfigRowsChange?.(result.configs);
        const deletedActive = deletableTargets.some((config) => config.path === activePath);
        const selected = result.configs.find((config) => config.path === activePath)
          ?? result.configs[0];
        if (deletedActive || !selected) {
          if (selected) await readConfig(selected.path, selected);
          else {
            invalidatePendingRead();
            if (activePathRef.current) clearEditorDraft(`configs:${activePathRef.current}`);
            setActivePath("");
            setSelectedPath("");
            setConflict(null);
            setSaveError("");
            setContentError("");
            setLoadingConfig(false);
          }
        }
      } else {
        await loadConfigs();
      }
    } catch (error) {
      setDeleteError(errorMessage(error));
    } finally {
      releaseMutation();
      setDeleting(false);
    }
  }, [activePath, beginConfigMutation, beginProfileChange, deleting, filterDeletableConfigs, invalidatePendingRead, loadConfigs, onConfigRowsChange, readConfig, setActivePath, setSelectedPath, updateActiveProfiles]);

  useEffect(() => {
    void loadConfigs();
    // The initial catalog load must not repeat after activePath is populated.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => () => {
    readRequestRef.current += 1;
  }, []);

  useEffect(() => {
    if (!activePath) return undefined;
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    void watchAgentConfig(activePath).catch((error) => {
      if (!disposed) logger.warn("failed to watch config", { path: activePath, error: errorMessage(error) });
    });
    void subscribeDaemonEvents((event: DaemonEvent) => {
      if (disposed || event.event !== RuntimeEventName.ConfigChanged || !isConfigSnapshot(event.payload)) return;
      if (configMutationDepthRef.current > 0) {
        configRefreshQueuedRef.current = true;
        return;
      }
      void readExternalSnapshot(event.payload.path);
    }).then((dispose) => {
      if (disposed) dispose();
      else unsubscribe = dispose;
    }).catch((error) => {
      if (!disposed) logger.warn("failed to subscribe to config changes", { error: errorMessage(error) });
    });
    return () => {
      disposed = true;
      unsubscribe?.();
    };
  }, [activePath, readExternalSnapshot]);

  return {
    configs,
    loadingConfig,
    setLoadingConfig,
    loadingConfigs,
    loadError,
    contentError,
    setContentError,
    conflict,
    setConflict,
    saveError,
    setSaveError,
    saving,
    deleting,
    deleteError,
    setDeleteError,
    profileSaving,
    profileError,
    setProfileError,
    profileSwitching,
    invalidatePendingRead,
    saveContent,
    saveCurrent,
    activateProfile,
    createProfile,
    deleteConfigs,
    filterDeletableConfigs,
    loadConfigs,
    readConfig,
  };
}
