import { createElement, type ReactNode } from "react";
import { Share2 } from "lucide-react";
import antigravityIcon from "@lobehub/icons-static-svg/icons/antigravity-color.svg";
import claudeIcon from "@lobehub/icons-static-svg/icons/claude-color.svg";
import clineIcon from "@lobehub/icons-static-svg/icons/cline.svg";
import codebuddyIcon from "@lobehub/icons-static-svg/icons/codebuddy-color.svg";
import codexIcon from "@lobehub/icons-static-svg/icons/codex-color.svg";
import copilotIcon from "@lobehub/icons-static-svg/icons/githubcopilot.svg";
import cursorIcon from "@lobehub/icons-static-svg/icons/cursor.svg";
import geminiCliIcon from "@lobehub/icons-static-svg/icons/geminicli-color.svg";
import hermesIcon from "@lobehub/icons-static-svg/icons/hermesagent.svg";
import kimiIcon from "@lobehub/icons-static-svg/icons/kimi-color.svg";
import opencodeIcon from "@lobehub/icons-static-svg/icons/opencode.svg";
import qoderIcon from "@lobehub/icons-static-svg/icons/qoder-color.svg";
import qwenIcon from "@lobehub/icons-static-svg/icons/qwen-color.svg";
import stepfunIcon from "@lobehub/icons-static-svg/icons/stepfun-color.svg";
import traeIcon from "@lobehub/icons-static-svg/icons/trae-color.svg";

import { titleValue } from "./strings.ts";

export const agentIcons: Record<string, string> = {
  antigravity: antigravityIcon,
  claude: claudeIcon,
  claudecode: claudeIcon,
  cline: clineIcon,
  codebuddy: codebuddyIcon,
  codex: codexIcon,
  copilot: copilotIcon,
  cursor: cursorIcon,
  gemini: geminiCliIcon,
  geminicli: geminiCliIcon,
  hermes: hermesIcon,
  hermesagent: hermesIcon,
  kimi: kimiIcon,
  opencode: opencodeIcon,
  qoder: qoderIcon,
  qwen: qwenIcon,
  stepfun: stepfunIcon,
  trae: traeIcon,
};

export function normalizedAgentKey(agent: unknown): string {
  return `${agent ?? ""}`.toLowerCase().replace(/[^a-z0-9]+/g, "");
}

export function agentClassName(agent: unknown): string {
  const key = normalizedAgentKey(agent);
  return key === "claudecode" ? "claude" : key;
}

export function agentIcon(agent: unknown): ReactNode {
  const key = normalizedAgentKey(agent);
  if (key === "shared") {
    return createElement(Share2, { className: "agentIconSvg", "aria-hidden": "true" });
  }
  const icon = agentIcons[key];
  if (icon) {
    return createElement("img", { className: "agentIconImage", src: icon, alt: "", draggable: false });
  }
  return createElement("span", { className: "agentIconFallback" }, `${agent ?? "Ag"}`.slice(0, 2));
}

export function friendlyAgent(agent: unknown): string {
  const key = normalizedAgentKey(agent);
  if (key === "claude" || key === "claudecode") return "Claude";
  if (key === "codex") return "Codex";
  if (key === "cursor") return "Cursor";
  if (key === "shared") return "Codex";
  return titleValue(agent);
}

export function GitLabSourceIcon(): ReactNode {
  return createElement(
    "svg",
    { className: "skillInfoSourceSvg", "aria-hidden": "true", viewBox: "0 0 24 24" },
    createElement("path", {
      d: "M12 21.2 4.7 15.9 1.8 7.1l3.5.1L6.8 2.9l3.1 9.5h4.2l3.1-9.5 1.5 4.3 3.5-.1-2.9 8.8L12 21.2Z",
    }),
    createElement("path", { d: "M12 21.2 9.9 12.4h4.2L12 21.2Z" }),
  );
}

export function sameAgent(value: unknown, expected: unknown): boolean {
  return friendlyAgent(value).toLowerCase() === friendlyAgent(expected).toLowerCase();
}
