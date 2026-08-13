import { PanelRightOpen } from "lucide-react";

export type DetailCollapsedRailProps = {
  label: string;
  expandLabel: string;
  onExpand: () => void;
};

export function DetailCollapsedRail({ label, expandLabel, onExpand }: DetailCollapsedRailProps) {
  return (
    <aside className="transcriptRail">
      <button
        className="threadPanelToggle railToggle"
        aria-label={expandLabel}
        onClick={onExpand}
      >
        <PanelRightOpen size={16} />
      </button>
      <div className="railLabel">{label}</div>
    </aside>
  );
}
