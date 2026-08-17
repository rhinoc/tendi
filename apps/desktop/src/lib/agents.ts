import { createElement, type ReactNode } from "react";
import { Share2 } from "lucide-react";
import { titleValue } from "./strings.ts";
import { agentDefinition } from "./agent/index.ts";
import { agentIcons } from "./agent/catalog.ts";

export { agentIcons } from "./agent/catalog.ts";

export function normalizedAgentKey(agent: unknown): string {
  return `${agent ?? ""}`.toLowerCase().replace(/[^a-z0-9]+/g, "");
}

export function agentIdentityKey(agent: unknown): string {
  const key = normalizedAgentKey(agent);
  const definition = agentDefinition(key);
  if (definition) return definition.id;
  return key;
}

export function isConcreteAgent(agent: unknown): boolean {
  const key = normalizedAgentKey(agent);
  return key !== "" && key !== "shared" && key !== "universal";
}

export function agentClassName(agent: unknown): string {
  const key = normalizedAgentKey(agent);
  const definition = agentDefinition(key);
  if (definition?.id !== "shared") return definition?.id ?? (key === "kimicodecli" ? "kimi" : key);
  if (key === "kimicodecli") return "kimi";
  return key;
}

export function agentIcon(agent: unknown): ReactNode {
  const key = normalizedAgentKey(agent);
  if (key === "shared") {
    return createElement(Share2, { className: "agentIconSvg", "aria-hidden": "true" });
  }
  const icon = agentDefinition(key)?.icon ?? agentIcons[key];
  if (icon) {
    return createElement("img", { className: "agentIconImage", src: icon, alt: "", draggable: false });
  }
  return null;
}

export function friendlyAgent(agent: unknown): string {
  const key = normalizedAgentKey(agent);
  const definition = agentDefinition(key);
  if (definition) return definition.displayName;
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
