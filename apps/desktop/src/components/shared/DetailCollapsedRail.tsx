import { PanelRightOpen } from "lucide-react";
import { IconButton } from "./IconButton.tsx";

export type DetailCollapsedRailProps = {
  label: string;
  expandLabel: string;
  onExpand: () => void;
};

export function DetailCollapsedRail({ label, expandLabel, onExpand }: DetailCollapsedRailProps) {
  return (
    <aside className="transcriptRail">
      <IconButton
        className="threadPanelToggle railToggle"
        aria-label={expandLabel}
        onClick={onExpand}
      >
        <PanelRightOpen size={16} />
      </IconButton>
      <div className="railLabel">{label}</div>
    </aside>
  );
}
