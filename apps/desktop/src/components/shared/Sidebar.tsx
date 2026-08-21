import { Tooltip } from "./Tooltip.tsx";
import { PanelLeftClose, PanelLeftOpen } from "lucide-react";

import { navItems, startWindowDrag } from "../../lib/index.ts";
import { AgentOptionLabel } from "./AgentOptionLabel.tsx";
import { Button } from "./Button.tsx";
import { SelectControl } from "./SelectControl.tsx";

export type SidebarSource = {
  label: string;
};

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
}: SidebarProps<TView>) {
  const agentOptions = [{ label: "All agents", value: "All" }, ...sources.map((source) => ({ label: source.label, value: source.label }))];
  const selectedAgentLabel = agentOptions.find((option) => option.value === agentFilter)?.label ?? agentFilter;

  return (
    <aside className={`sidebar ${collapsed ? "collapsed" : ""}`}>
      <div className="sidebarTop dragRegion" data-window-drag onMouseDown={(event) => startWindowDrag(event.nativeEvent)}>
        <div className="titlebarSpacer" aria-hidden="true" />
      </div>
      <div className="sidebarBody">
        <nav className="nav">
          {navItems.map((item) => {
            const Icon = item.icon;
            return (
              <Tooltip key={item.id} content={collapsed ? item.label : undefined}><button
                key={item.id}
                className={`navItem ${view === item.id ? "active" : ""}`}
                onClick={() => setView(item.id as TView)}
                onMouseEnter={() => onPrefetchView?.(item.id as TView)}
                onMouseLeave={() => onCancelPrefetchView?.(item.id as TView)}
                onFocus={() => onPrefetchView?.(item.id as TView)}
                onBlur={() => onCancelPrefetchView?.(item.id as TView)}
                aria-label={item.label}
                aria-current={view === item.id ? "page" : undefined}
              >
                <Icon size={16} />
                <span>{item.label}</span>
              </button></Tooltip>
            );
          })}
        </nav>
        <div className="agentSelectGroup">
          <SelectControl
            contentClassName="agentSelectContent"
            label="Agent filter"
            value={agentFilter}
            onValueChange={setAgentFilter}
            options={agentOptions}
            side="top"
            align="start"
            indicatorPosition="right"
            showChevron={!collapsed}
            triggerTooltipContent={collapsed ? selectedAgentLabel : undefined}
            renderValue={(option) => (
              <AgentOptionLabel
                agent={option?.value ?? agentFilter}
                label={option?.label ?? selectedAgentLabel}
                variant="filter"
                collapsed={collapsed}
              />
            )}
            renderOption={(option) => <AgentOptionLabel agent={option.value} label={option.label} variant="filter" />}
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
