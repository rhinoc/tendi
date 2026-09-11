import type { ReactNode } from "react";
import { PanelRightClose } from "lucide-react";
import { IconButton } from "./IconButton.tsx";

export type DetailPanelVariant = "ruleEditor";

export type DetailPanelProps = {
  className?: string;
  variant?: DetailPanelVariant;
  title: ReactNode;
  meta?: ReactNode;
  headerActions?: ReactNode;
  collapseLabel: string;
  onCollapse: () => void;
  children: ReactNode;
};

export function DetailPanel({
  className = "",
  variant,
  title,
  meta,
  headerActions,
  collapseLabel,
  onCollapse,
  children,
}: DetailPanelProps) {
  const variantClassName = variant ? `detailPanel--${variant}` : "";
  return (
    <aside className={`detailPanel ${variantClassName} ${className}`.trim()}>
      <header className="threadHeader">
        <div className="threadTitleLine">
          <h2>{title}</h2>
          <div className="threadHeaderActions">
            {headerActions}
            <IconButton
              className="threadPanelToggle"
              aria-label={collapseLabel}
              onClick={onCollapse}
            >
              <PanelRightClose size={16} />
            </IconButton>
          </div>
        </div>
        {meta}
      </header>
      {children}
    </aside>
  );
}
