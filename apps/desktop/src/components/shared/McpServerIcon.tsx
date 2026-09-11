import { useState } from "react";

import type { McpIcon } from "../../lib/mcp.ts";
import "./McpServerIcon.css";

type McpServerIconProps = {
  icons?: McpIcon[];
  size?: number;
};

export function McpServerIcon({ icons = [], size = 18 }: McpServerIconProps) {
  const [failed, setFailed] = useState(false);
  const src = icons[0]?.src;
  if (!src || failed) {
    return (
      <span className="mcpServerIcon mcpServerIconPlaceholder" style={{ width: size, height: size }} aria-hidden="true" />
    );
  }
  return (
    <span className="mcpServerIcon" style={{ width: size, height: size }} aria-hidden="true">
      <img src={src} alt="" onError={() => setFailed(true)} />
    </span>
  );
}
