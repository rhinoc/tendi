import { Tooltip } from "../components/shared/Tooltip.tsx";
import { lazy, Suspense, useCallback, useDeferredValue, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Dialog } from "radix-ui";
import { ArrowRightLeft, Check, Plus, RefreshCw, Save } from "lucide-react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";

import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef } from "../components/DataTable.types.ts";
import { AgentFilterOptionLabel } from "../components/shared/AgentFilterOptionLabel.tsx";
import { DialogActionBar } from "../components/shared/DialogActionBar.tsx";
import { DialogActionButton } from "../components/shared/DialogActionButton.tsx";
import { DiscardChangesDialog } from "../components/shared/DiscardChangesDialog.tsx";
import { DialogTextField } from "../components/shared/DialogTextField.tsx";
import { LoadingIcon } from "../components/shared/LoadingIcon.tsx";
import { LoadingInline } from "../components/shared/LoadingInline.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { ResizeSeparator } from "../components/shared/ResizeSeparator.tsx";
import { SelectControl } from "../components/shared/SelectControl.tsx";
import { TauriCommand, diffPreview, friendlyAgent } from "../lib/index.ts";
import "./ConfigView.css";

const BASE_PROFILE_VALUE = "__tendi_base_config__";
const MarkdownFilePane = lazy(() => import("../components/shared/MarkdownFilePane.tsx").then(({ MarkdownFilePane: component }) => ({ default: component })));

type AgentConfigFile = Record<string, unknown> & {
  agent: string;
  label: string;
  path: string;
  format: "json" | "toml";
  exists: boolean;
  profile?: string;
};

type AgentConfigContent = {
  path: string;
  content: string;
  sha256: string;
  exists: boolean;
};

type AppSettings = {
  configProfiles: Record<string, string>;
};

function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Config operation failed";
}

export function ConfigView() {
  const [configs, setConfigs] = useState<AgentConfigFile[]>([]);
  const [activePath, setActivePath] = useState("");
  const [content, setContent] = useState("");
  const [originalContent, setOriginalContent] = useState("");
  const [sha256, setSha256] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [pendingPath, setPendingPath] = useState("");
  const [pendingReload, setPendingReload] = useState(false);
  const [showDiscardDialog, setShowDiscardDialog] = useState(false);
  const [activeProfiles, setActiveProfiles] = useState<Record<string, string>>({});
  const [selectedProfileValue, setSelectedProfileValue] = useState(BASE_PROFILE_VALUE);
  const [profileDialogOpen, setProfileDialogOpen] = useState(false);
  const [profileName, setProfileName] = useState("");
  const [profileSaving, setProfileSaving] = useState(false);
  const [profileSwitching, setProfileSwitching] = useState(false);
  const [profileError, setProfileError] = useState("");
  const activeConfig = configs.find((config) => config.path === activePath) ?? null;
  const dirty = content !== originalContent;
  const profileAgent = activeConfig?.agent ?? null;
  const profileConfigs = useMemo(
    () => profileAgent ? configs.filter((config) => config.agent === profileAgent && config.profile) : [],
    [configs, profileAgent],
  );
  const activeProfile = profileAgent ? activeProfiles[profileAgent] ?? "" : "";
  const profileOptions = useMemo(() => [
    { value: BASE_PROFILE_VALUE, label: "Base config" },
    ...profileConfigs.map((config) => ({
      value: `profile:${config.profile}`,
      label: config.profile ?? "",
    })),
  ], [profileConfigs]);
  const activeProfileValue = activeProfile && profileConfigs.some((config) => config.profile === activeProfile)
    ? `profile:${activeProfile}`
    : BASE_PROFILE_VALUE;
  const profileSelectionChanged = selectedProfileValue !== activeProfileValue;
  const columns = useMemo<ColumnDef<AgentConfigFile>[]>(() => [
    {
      key: "agent",
      header: "Agent",
      width: "minmax(180px, 1fr)",
      render: (config) => (
        <div className="configAgentCell">
          <AgentFilterOptionLabel agent={config.agent} collapsed />
          <span className="configAgentText">
            <strong>{config.label}</strong>
            <Tooltip content={config.path} onlyWhenTruncated><span>{config.path}</span></Tooltip>
          </span>
        </div>
      ),
    },
  ], []);
  const deferredContent = useDeferredValue(content);
  const diffStats = useMemo(() => {
    const lines = diffPreview(originalContent, deferredContent);
    return {
      added: lines.filter((line) => line.kind === "added").length,
      removed: lines.filter((line) => line.kind === "removed").length,
    };
  }, [deferredContent, originalContent]);

  useEffect(() => {
    setSelectedProfileValue(activeProfileValue);
  }, [activeProfileValue, profileAgent]);

  const readConfig = useCallback(async (path: string) => {
    setLoading(true);
    try {
      const next = await invoke<AgentConfigContent>(TauriCommand.AgentConfigRead, { path });
      setActivePath(next.path);
      setContent(next.content);
      setOriginalContent(next.content);
      setSha256(next.sha256);
    } catch (error) {
      console.error("Failed to read config", errorMessage(error));
    } finally {
      setLoading(false);
    }
  }, []);

  const loadConfigs = useCallback(async () => {
    setLoading(true);
    try {
      const next = await invoke<AgentConfigFile[]>(TauriCommand.AgentConfigsList);
      setConfigs(next);
      const path = next.find((config) => config.path === activePath)?.path ?? next[0]?.path;
      if (path) await readConfig(path);
      else setLoading(false);
    } catch (error) {
      console.error("Failed to load configs", errorMessage(error));
      setLoading(false);
    }
  }, [activePath, readConfig]);

  const loadSettings = useCallback(async () => {
    try {
      const next = await invoke<AppSettings>(TauriCommand.SettingsGet);
      setActiveProfiles(next.configProfiles ?? {});
    } catch (error) {
      console.error("Failed to load config settings", errorMessage(error));
    }
  }, []);

  useEffect(() => {
    void loadConfigs();
    void loadSettings();
    // The initial catalog load must not repeat after activePath is populated.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const chooseConfig = (path: string) => {
    if (path === activePath) return;
    if (dirty) {
      setPendingPath(path);
      setPendingReload(false);
      setShowDiscardDialog(true);
      return;
    }
    void readConfig(path);
  };

  const reload = () => {
    if (!activePath) {
      void loadConfigs();
      return;
    }
    if (dirty) {
      setPendingPath("");
      setPendingReload(true);
      setShowDiscardDialog(true);
      return;
    }
    void readConfig(activePath);
  };

  const discardPendingChanges = () => {
    const nextPath = pendingPath;
    const shouldReload = pendingReload;
    setPendingPath("");
    setPendingReload(false);
    if (nextPath) void readConfig(nextPath);
    else if (shouldReload && activePath) void readConfig(activePath);
  };

  const activateProfile = useCallback(async (agent: string, value: string) => {
    if (profileSwitching) return;
    const profile = value === BASE_PROFILE_VALUE ? null : value.replace(/^profile:/, "");
    const target = configs.find((config) => (
      config.agent === agent && (profile ? config.profile === profile : !config.profile)
    ));
    if (!target) {
      console.error("Config profile not found; reload the config list");
      return;
    }
    if (dirty) {
      return;
    }
    setProfileSwitching(true);
    try {
      const next = await invoke<AppSettings>(TauriCommand.ConfigProfileSet, { agent, profile });
      setActiveProfiles(next.configProfiles ?? {});
      if (target.path !== activePath) await readConfig(target.path);
    } catch (error) {
      console.error("Failed to activate config profile", errorMessage(error));
    } finally {
      setProfileSwitching(false);
    }
  }, [activePath, configs, dirty, profileSwitching, readConfig]);

  const openProfileDialog = () => {
    if (!profileAgent) return;
    setProfileName("");
    setProfileError("");
    setProfileDialogOpen(true);
  };

  const createProfile = useCallback(async () => {
    if (!profileAgent) return;
    const name = profileName.trim();
    if (!name) {
      setProfileError("Enter a profile name");
      return;
    }
    setProfileSaving(true);
    setProfileError("");
    try {
      await invoke<AgentConfigContent>(TauriCommand.ConfigProfileCreate, {
        agent: profileAgent,
        name,
        content,
      });
      const next = await invoke<AgentConfigFile[]>(TauriCommand.AgentConfigsList);
      setConfigs(next);
      setSelectedProfileValue(`profile:${name}`);
      setProfileDialogOpen(false);
      setProfileName("");
    } catch (error) {
      setProfileError(errorMessage(error));
    } finally {
      setProfileSaving(false);
    }
  }, [content, profileAgent, profileName]);

  const save = useCallback(async () => {
    if (!activeConfig || !dirty || saving) return;
    setSaving(true);
    try {
      const saved = await invoke<AgentConfigContent>(TauriCommand.AgentConfigSave, {
        path: activeConfig.path,
        expectedSha256: sha256,
        content,
      });
      setContent(saved.content);
      setOriginalContent(saved.content);
      setSha256(saved.sha256);
      setConfigs((current) => current.map((config) => (
        config.path === saved.path ? { ...config, exists: true } : config
      )));
    } catch (error) {
      console.error("Failed to save config", errorMessage(error));
    } finally {
      setSaving(false);
    }
  }, [activeConfig, content, dirty, saving, sha256]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "s") return;
      event.preventDefault();
      void save();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [save]);

  return (
    <PanelGroup className="sessionsLayout configLayout" orientation="horizontal">
      <DiscardChangesDialog
        open={showDiscardDialog}
        onOpenChange={setShowDiscardDialog}
        onDiscard={discardPendingChanges}
      />
      <Dialog.Root
        open={profileDialogOpen}
        onOpenChange={(open) => {
          if (!profileSaving) setProfileDialogOpen(open);
        }}
      >
        <Dialog.Portal>
          <Dialog.Overlay className="dialogOverlay" />
          <Dialog.Content
            className="confirmDialogPanel configProfileDialog"
            aria-describedby="config-profile-description"
            data-no-drag
            onMouseDown={(event) => event.stopPropagation()}
          >
            <Dialog.Title className="confirmDialogTitle">Create config profile</Dialog.Title>
            <p id="config-profile-description" className="confirmDialogDescription">
              Save the current {profileAgent ? friendlyAgent(profileAgent) : "agent"} {activeConfig?.format.toUpperCase() ?? "config"} as a named profile.
            </p>
            <div className="configProfileDialogBody">
              <DialogTextField
                label="Profile name"
                value={profileName}
                onChange={setProfileName}
                placeholder="deep-review"
              />
              <p className="configProfileHint">Use letters, numbers, hyphens, or underscores.</p>
              {profileError ? <div className="addSkillError">{profileError}</div> : null}
            </div>
            <DialogActionBar cancelDisabled={profileSaving} onCancel={() => setProfileDialogOpen(false)}>
              <DialogActionButton
                variant="primary"
                className="dialogAdvanceButton"
                disabled={!profileName.trim() || profileSaving}
                onClick={() => { void createProfile(); }}
              >
                <span>{profileSaving ? "Creating" : "Create"}</span>
                {profileSaving ? <LoadingIcon size={16} /> : <Save size={16} />}
              </DialogActionButton>
            </DialogActionBar>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
      <Panel className="sessionListPanel configListPanel" defaultSize="36%" minSize="280px">
        <div className="sessionListPane configListPane">
          <PageHeader title="Config" compact>
            <button
              className="iconButton"
              aria-label="Reload config"
              disabled={loading}
              onClick={reload}
            >
              {loading ? <LoadingIcon size={14} /> : <RefreshCw size={14} />}
            </button>
          </PageHeader>
          <div className="sessionListBody">
            <DataTable
              rows={configs}
              columns={columns}
              getRowId={(config) => config.path}
              getRowLabel={(config) => config.label}
              onRowClick={(config) => chooseConfig(config.path)}
              rowProps={(config) => ({
                className: config.path === activePath ? "configRowActive" : "",
                "aria-current": config.path === activePath ? "true" : undefined,
              })}
              loading={loading && configs.length === 0}
              loadingLabel={<LoadingInline label="Loading configs" />}
              emptyState="No supported agent configs found. Open a supported agent, then refresh."
            />
          </div>
        </div>
      </Panel>
      <ResizeSeparator />
      <Panel className="configEditorPanelHost" defaultSize="64%" minSize="420px">
        <aside className="ruleEditorPanel configEditorPanel">
          <header className="threadHeader configEditorHeader">
            <div className="threadTitleLine">
              <div className="configTitle">
                <h2>{activeConfig?.label ?? "Config file"}</h2>
                {!loading && activeConfig && dirty ? <span className="configDirty">modified</span> : null}
              </div>
            </div>
            {profileAgent ? (
              <div className="configProfileFloating">
                <SelectControl
                  className="configProfileSelect"
                  contentClassName="configProfileSelectContent"
                  label={`Active ${friendlyAgent(profileAgent)} profile`}
                  value={selectedProfileValue}
                  onValueChange={setSelectedProfileValue}
                  options={profileOptions}
                />
                <Tooltip content={dirty ? "Save or discard current changes before activating a profile" : profileSelectionChanged ? "Activate selected config profile" : "Active config profile"}><button
                  className={`configProfileSwitchButton ${profileSelectionChanged ? "" : "isActive"}`}
                  aria-label={dirty ? "Save or discard changes before activating a config profile" : profileSelectionChanged ? "Activate selected config profile" : "Active config profile"}
                  disabled={!profileSelectionChanged || dirty || profileSwitching || profileSaving}
                  onClick={() => { void activateProfile(profileAgent, selectedProfileValue); }}
                >
                  {profileSwitching ? <LoadingIcon size={14} /> : profileSelectionChanged ? <ArrowRightLeft size={14} /> : <Check size={14} />}
                  <span>{profileSwitching ? "Switching…" : profileSelectionChanged ? "Activate" : "Active"}</span>
                </button></Tooltip>
                <button
                  className="iconButton"
                  aria-label="Create config profile"
                  disabled={profileSaving}
                  onClick={openProfileDialog}
                >
                  <Plus size={14} />
                </button>
              </div>
            ) : null}
          </header>
          {loading ? (
            <div className="configEditorMessage"><LoadingInline label="Loading config" /></div>
          ) : activeConfig ? (
            <Suspense fallback={<div className="configEditorMessage"><LoadingInline label="Loading editor" /></div>}>
              <MarkdownFilePane
                activePath={activeConfig.path}
                dirty={dirty}
                diffStats={diffStats}
                content={content}
                originalContent={originalContent}
                language={activeConfig.format}
                onChange={(value) => {
                  setContent(value);
                }}
                onSave={() => { void save(); }}
                copyablePath
                showDirtyIndicator={false}
                showTokenStatusBar={false}
              />
            </Suspense>
          ) : (
            <div className="configEditorMessage">Select a config file</div>
          )}
        </aside>
      </Panel>
    </PanelGroup>
  );
}
