import type { ReactNode } from "react";
import { Panel } from "react-resizable-panels";

import { DetailCollapsedRail } from "./DetailCollapsedRail.tsx";
import { ResizeSeparator } from "./ResizeSeparator.tsx";
import "./detail-panel.css";

const COLLAPSED_WIDTH = "48px";
const MIN_EXPANDED_WIDTH = "420px";

export type DetailPanelHostProps = {
  collapsed: boolean;
  onExpand: () => void;
  expandLabel: string;
  railLabel: string;
  hasSelection: boolean;
  emptyState: ReactNode;
  children: ReactNode;
  expandedDefaultSize?: string;
  hostClassName?: string;
  panelClassName?: string;
};

export function DetailPanelHost({
  collapsed,
  onExpand,
  expandLabel,
  railLabel,
  hasSelection,
  emptyState,
  children,
  expandedDefaultSize = "46%",
  hostClassName = "",
  panelClassName = "ruleEditorPanel",
}: DetailPanelHostProps) {
  return (
    <>
      {!collapsed && <ResizeSeparator />}
      <Panel
        data-detail-panel
        className={`transcriptPanelHost ${hostClassName} ${collapsed ? "collapsed" : ""}`.trim()}
        defaultSize={collapsed ? COLLAPSED_WIDTH : expandedDefaultSize}
        minSize={collapsed ? COLLAPSED_WIDTH : MIN_EXPANDED_WIDTH}
        maxSize={collapsed ? COLLAPSED_WIDTH : undefined}
      >
        <div className="detailPanelContent" hidden={collapsed} aria-hidden={collapsed}>
          {hasSelection ? children : <aside className={`${panelClassName} emptyTranscript`}>{emptyState}</aside>}
        </div>
        {collapsed ? <DetailCollapsedRail label={railLabel} expandLabel={expandLabel} onExpand={onExpand} /> : null}
      </Panel>
    </>
  );
}
