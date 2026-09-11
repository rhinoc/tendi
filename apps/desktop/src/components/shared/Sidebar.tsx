import { Tooltip } from "./Tooltip.tsx";
import { Check, ChevronDown, ChevronRight, PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { DropdownMenu } from "radix-ui";

import { ALL_AGENT_FILTER, AppPage, navItems, ProjectScopeFilter, startWindowDrag } from "../../lib/index.ts";
import { AgentOptionLabel } from "./AgentOptionLabel.tsx";
import { Badge } from "./Badge.tsx";
import { Button } from "./Button.tsx";
import { MenuContent } from "./MenuContent.tsx";

export type SidebarSource = {
  label: string;
};

type SidebarFilterOption = {
  label: string;
  value: string;
};

const projectScopeOptions: Array<{ label: string; value: ProjectScopeFilter }> = [
  { label: "Global", value: ProjectScopeFilter.Global },
  { label: "Project", value: ProjectScopeFilter.Project },
  { label: "All", value: ProjectScopeFilter.All },
];

type SidebarFilterProps = {
  agentOptions: SidebarFilterOption[];
  agentFilter: string;
  projectScopeFilter: ProjectScopeFilter;
  collapsed: boolean;
  onAgentChange: (value: string) => void;
  onProjectScopeChange: (value: ProjectScopeFilter) => void;
};

function SidebarFilter({
  agentOptions,
  agentFilter,
  projectScopeFilter,
  collapsed,
  onAgentChange,
  onProjectScopeChange,
}: SidebarFilterProps) {
  const selectedAgentLabel = agentOptions.find((option) => option.value === agentFilter)?.label ?? agentFilter;
  const selectedScopeLabel = projectScopeOptions.find((option) => option.value === projectScopeFilter)?.label ?? "All";
  const selectedFilterLabel = selectedScopeLabel === "All"
    ? selectedAgentLabel
    : `${selectedAgentLabel} · ${selectedScopeLabel}`;

  return (
    <DropdownMenu.Root>
      <Tooltip content={collapsed ? selectedFilterLabel : undefined}>
        <DropdownMenu.Trigger asChild>
          <button type="button" className="selectControlTrigger" aria-label="Agent and scope filters">
            <AgentOptionLabel
              agent={agentFilter}
              label={selectedFilterLabel}
              variant="filter"
              collapsed={collapsed}
            />
            {!collapsed ? <ChevronDown size={14} /> : null}
          </button>
        </DropdownMenu.Trigger>
      </Tooltip>
      <DropdownMenu.Portal>
        <MenuContent
          className="selectControlContent agentSelectContent"
          side="top"
          align="start"
          sideOffset={8}
          style={{ width: "max-content", minWidth: "168px" }}
          data-no-drag
        >
          <DropdownMenu.Sub>
            <DropdownMenu.SubTrigger className="menuItem menuSubTrigger">
              <span className="selectItemText">Agent</span>
              <ChevronRight className="menuSubIcon" size={14} />
            </DropdownMenu.SubTrigger>
            <DropdownMenu.Portal>
              <DropdownMenu.SubContent
                className="menuContent selectControlContent agentSelectContent"
                sideOffset={8}
                alignOffset={-6}
                style={{ width: "max-content" }}
              >
                {agentOptions.map((agent) => {
                  const selected = agent.value === agentFilter;
                  return (
                    <DropdownMenu.Item
                      className="menuItem selectItemIndicatorRight"
                      key={agent.value}
                      onSelect={() => onAgentChange(agent.value)}
                    >
                      <span className="selectItemText">
                        <AgentOptionLabel agent={agent.value} label={agent.label} variant="filter" />
                      </span>
                      <span className="selectItemLeadingIcon" aria-hidden="true">
                        {selected ? <Check className="selectItemIndicator" size={14} /> : null}
                      </span>
                    </DropdownMenu.Item>
                  );
                })}
              </DropdownMenu.SubContent>
            </DropdownMenu.Portal>
          </DropdownMenu.Sub>
          <DropdownMenu.Sub>
            <DropdownMenu.SubTrigger className="menuItem menuSubTrigger">
              <span className="selectItemText">Scope</span>
              <ChevronRight className="menuSubIcon" size={14} />
            </DropdownMenu.SubTrigger>
            <DropdownMenu.Portal>
              <DropdownMenu.SubContent
                className="menuContent selectControlContent agentSelectContent"
                sideOffset={8}
                alignOffset={-6}
                style={{ width: "max-content" }}
              >
                {projectScopeOptions.map((scope) => {
                  const selected = scope.value === projectScopeFilter;
                  return (
                    <DropdownMenu.Item
                      className="menuItem selectItemIndicatorRight"
                      key={scope.value}
                      onSelect={() => onProjectScopeChange(scope.value)}
                    >
                      <span className="selectItemText">{scope.label}</span>
                      <span className="selectItemLeadingIcon" aria-hidden="true">
                        {selected ? <Check className="selectItemIndicator" size={14} /> : null}
                      </span>
                    </DropdownMenu.Item>
                  );
                })}
              </DropdownMenu.SubContent>
            </DropdownMenu.Portal>
          </DropdownMenu.Sub>
        </MenuContent>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

export type SidebarProps<TView extends string = string> = {
  view: TView;
  setView: (view: TView) => void;
  onPrefetchView?: (view: TView) => void;
  onCancelPrefetchView?: (view: TView) => void;
  sources: SidebarSource[];
  collapsed: boolean;
  setCollapsed: (value: boolean | ((current: boolean) => boolean)) => void;
  agentFilter: string;
  setAgentFilter: (value: string) => void;
  projectScopeFilter: ProjectScopeFilter;
  setProjectScopeFilter: (value: ProjectScopeFilter) => void;
  updateAvailable?: boolean;
};

export function Sidebar<TView extends string = string>({
  view,
  setView,
  onPrefetchView,
  onCancelPrefetchView,
  sources,
  collapsed,
  setCollapsed,
  agentFilter,
  setAgentFilter,
  projectScopeFilter,
  setProjectScopeFilter,
  updateAvailable = false,
}: SidebarProps<TView>) {
  const agentOptions = [{ label: ALL_AGENT_FILTER, value: ALL_AGENT_FILTER }, ...sources.map((source) => ({ label: source.label, value: source.label }))];

  return (
    <aside className={`sidebar ${collapsed ? "collapsed" : ""}`} data-state={collapsed ? "collapsed" : "expanded"}>
      <div className="sidebarTop dragRegion" data-window-drag onMouseDown={(event) => startWindowDrag(event.nativeEvent)}>
        <div className="titlebarSpacer" aria-hidden="true" />
      </div>
      <div className="sidebarBody">
        <ul className="nav">
          {navItems.map((item) => {
            const Icon = item.icon;
            return (
              <li
                key={item.id}
                className="navItemRow"
                onMouseEnter={() => onPrefetchView?.(item.id as TView)}
                onMouseLeave={() => onCancelPrefetchView?.(item.id as TView)}
              >
                <Tooltip content={collapsed ? (item.id === AppPage.Settings && updateAvailable ? "Settings — update available" : item.label) : undefined}>
                  <button
                    type="button"
                    className={`navItem ${view === item.id ? "active" : ""}`}
                    onClick={() => setView(item.id as TView)}
                    onFocus={() => onPrefetchView?.(item.id as TView)}
                    onBlur={() => onCancelPrefetchView?.(item.id as TView)}
                    aria-label={item.id === AppPage.Settings && updateAvailable ? "Settings, update available" : item.label}
                    aria-current={view === item.id ? "page" : undefined}
                  >
                    {view === item.id ? (
                      <span className="navSelectedPill" aria-hidden="true" />
                    ) : null}
                    <span className="navItemIcon" aria-hidden="true"><Icon size={16} /></span>
                    <span
                      className={`navItemLabel ${collapsed ? "isCollapsed" : ""}`}
                      aria-hidden={collapsed}
                    >
                      {item.label}
                    </span>
                    {item.id === AppPage.Settings ? (
                      <Badge tone="warning" className="navItemBadge" data-visible={updateAvailable} aria-hidden="true">New</Badge>
                    ) : null}
                  </button>
                </Tooltip>
              </li>
            );
          })}
        </ul>
        <div className="agentSelectGroup">
          <SidebarFilter
            agentOptions={agentOptions}
            agentFilter={agentFilter}
            projectScopeFilter={projectScopeFilter}
            collapsed={collapsed}
            onAgentChange={setAgentFilter}
            onProjectScopeChange={setProjectScopeFilter}
          />
          <Button
            variant="icon"
            className="sidebarToggle"
            aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
            onClick={() => setCollapsed((value) => !value)}
          >
            {collapsed ? <PanelLeftOpen size={15} /> : <PanelLeftClose size={15} />}
          </Button>
        </div>
      </div>
    </aside>
  );
}
