import { Separator as PanelResizeHandle } from "react-resizable-panels";

import "./ResizeSeparator.css";

export type ResizeSeparatorProps = {
  className?: string;
};

export function ResizeSeparator({ className = "" }: ResizeSeparatorProps) {
  return <PanelResizeHandle className={`panelResizeHandle ${className}`} />;
}
