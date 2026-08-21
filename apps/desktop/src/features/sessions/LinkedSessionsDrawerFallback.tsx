import { X } from "lucide-react";

import { LoadingState } from "../../components/shared/LoadingState.tsx";
import { IconButton } from "../../components/shared/IconButton.tsx";
import "./linked-sessions.css";

export type LinkedSessionsDrawerFallbackProps = {
  onClose: () => void;
};

export function LinkedSessionsDrawerFallback({ onClose }: LinkedSessionsDrawerFallbackProps) {
  return (
    <div className="linkedSessionsDrawer" aria-busy="true" data-no-drag>
      <div className="linkedSessionsDrawerHeader">
        <strong className="linkedSessionsDrawerTitle">Recent Sessions</strong>
        <div className="linkedSessionsDrawerActions">
          <IconButton aria-label="Close recent sessions" onClick={onClose}>
            <X size={15} />
          </IconButton>
        </div>
      </div>
      <LoadingState className="linkedSessionsDrawerEmpty" label="Loading recent sessions" />
    </div>
  );
}
