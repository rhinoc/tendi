import type { ReactNode } from "react";

import "./PageHeader.css";

import { startWindowDrag } from "../../lib/index.ts";

export type PageHeaderProps = {
  title: string;
  compact?: boolean;
  children?: ReactNode;
};

export function PageHeader({ title, compact = false, children }: PageHeaderProps) {
  return (
    <header
      className={`pageHeader ${compact ? "compact " : ""}dragRegion`}
      data-window-drag
      onMouseDown={(event) => startWindowDrag(event.nativeEvent)}
    >
      <div><h1>{title}</h1></div>
      {children ? <div className="headerTools">{children}</div> : null}
    </header>
  );
}
