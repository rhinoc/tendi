import { Tooltip } from "./Tooltip.tsx";
import { Check, ChevronDown, PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { Select } from "radix-ui";

import { navItems, startWindowDrag } from "../../lib/index.ts";
import { AgentFilterOptionLabel } from "./AgentFilterOptionLabel.tsx";

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
          <Select.Root value={agentFilter} onValueChange={setAgentFilter}>
            <Tooltip content={collapsed ? selectedAgentLabel : undefined}>
              <Select.Trigger
                className="agentSelectTrigger"
                aria-label="Agent filter"
              >
                <Select.Value>
                  <AgentFilterOptionLabel agent={agentFilter} label={selectedAgentLabel} collapsed={collapsed} />
                </Select.Value>
                {!collapsed && (
                  <Select.Icon asChild>
                    <ChevronDown size={14} />
                  </Select.Icon>
                )}
              </Select.Trigger>
            </Tooltip>
            <Select.Portal>
              <Select.Content
                className="skillMenuContent agentSelectContent"
                position="popper"
                side="top"
                align="start"
                sideOffset={6}
              >
                <Select.Viewport>
                  {agentOptions.map((option) => (
                    <Select.Item className="skillMenuItem" value={option.value} key={option.value}>
                      <Select.ItemText>
                        <AgentFilterOptionLabel agent={option.value} label={option.label} />
                      </Select.ItemText>
                      <Select.ItemIndicator className="selectItemIndicator">
                        <Check size={13} strokeWidth={2.6} />
                      </Select.ItemIndicator>
                    </Select.Item>
                  ))}
                </Select.Viewport>
              </Select.Content>
            </Select.Portal>
          </Select.Root>
          <button
            className="paneButton sidebarToggle"
            aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
            onClick={() => setCollapsed((value) => !value)}
          >
            {collapsed ? <PanelLeftOpen size={15} /> : <PanelLeftClose size={15} />}
          </button>
        </div>
      </div>
    </aside>
  );
}
