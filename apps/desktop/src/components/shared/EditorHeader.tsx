import { ArrowLeft } from "lucide-react";
import type { ReactNode } from "react";

import { startWindowDrag } from "../../lib/index.ts";
import "./EditorHeader.css";

export type EditorHeaderProps = {
  title: string;
  backLabel: string;
  onBack: () => void;
  actions?: ReactNode;
};

export function EditorHeader({ title, backLabel, onBack, actions = null }: EditorHeaderProps) {
  return (
    <header className="editorHeader dragRegion" data-window-drag onMouseDown={(event) => startWindowDrag(event.nativeEvent)}>
      <button className="backButton" aria-label={backLabel} onClick={onBack}><ArrowLeft size={17} /></button>
      <div className="editorTitle">
        <h1>{title}</h1>
      </div>
      {actions && <div className="editorActions">{actions}</div>}
    </header>
  );
}
