import { X } from "lucide-react";

import { LoadingState } from "../../components/shared/LoadingState.tsx";
import { IconButton } from "../../components/shared/IconButton.tsx";
import { dialogCopy } from "../../lib/index.ts";
import "./linked-sessions.css";

export type LinkedSessionsDrawerFallbackProps = {
  onClose: () => void;
};

export function LinkedSessionsDrawerFallback({ onClose }: LinkedSessionsDrawerFallbackProps) {
  return (
    <div className="linkedSessionsDrawer" aria-busy="true" data-no-drag>
      <div className="linkedSessionsDrawerHeader">
        <strong className="linkedSessionsDrawerTitle">{dialogCopy.recentSessionsLabel}</strong>
        <div className="linkedSessionsDrawerActions">
          <IconButton aria-label={dialogCopy.linkedSessionsCloseLabel} onClick={onClose}>
            <X size={15} />
          </IconButton>
        </div>
      </div>
      <LoadingState className="linkedSessionsDrawerEmpty" label={dialogCopy.linkedSessionsLoadingLabel} />
    </div>
  );
}
