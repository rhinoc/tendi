import {
  FileCode,
  LayoutDashboard,
  MessageSquareText,
  MessagesSquare,
  ScrollText,
  Server,
  Settings,
  Sparkles,
  Webhook,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";

import { SkillVisibility } from "./skills.ts";

export type NavItem = { id: string; label: string; icon: LucideIcon };

export const navItems: NavItem[] = [
  { id: "overview", label: "Overview", icon: LayoutDashboard },
  { id: "skills", label: "Skills", icon: Sparkles },
  { id: "prompts", label: "Prompts", icon: MessageSquareText },
  { id: "sessions", label: "Sessions", icon: MessagesSquare },
  { id: "rules", label: "Rules", icon: ScrollText },
  { id: "hooks", label: "Hooks", icon: Webhook },
  { id: "mcp", label: "MCP", icon: Server },
  { id: "config", label: "Config", icon: FileCode },
  { id: "settings", label: "Settings", icon: Settings },
];

export const fallbackAgents = [
  { label: "Codex", count: 1 },
  { label: "Cursor", count: 1 },
  { label: "Claude", count: 1 },
];

export const fallbackSkills = [
  {
    id: "lark-im",
    section: "Local",
    name: "lark-im",
    description: "Send messages, search chats, manage groups, files, reactions, and feed pins.",
    agents: ["Codex", "Cursor"],
    visibility: SkillVisibility.Manual,
    statusTone: "warn",
    source: "local",
    installTargets: ["shared"],
    updateStatus: "local",
  },
  {
    id: "lark-doc",
    section: "Local",
    name: "lark-doc",
    description: "Read, create, and edit Lark cloud documents from URLs or tokens.",
    agents: ["Codex", "Cursor"],
    visibility: SkillVisibility.Manual,
    statusTone: "warn",
    source: "local",
    installTargets: ["shared"],
    updateStatus: "local",
  },
  {
    id: "pr",
    section: "Local",
    name: "pr",
    description: "Focused GitHub pull request workflow for creating, updating, and validating PRs.",
    agents: ["Codex", "Cursor", "Claude"],
    visibility: SkillVisibility.Auto,
    statusTone: "ok",
    source: "local",
    installTargets: ["shared"],
    updateStatus: "local",
  },
  {
    id: "openai-docs",
    section: "System",
    name: "openai-docs",
    description: "Official OpenAI and Codex product documentation lookup.",
    agents: ["Codex"],
    visibility: SkillVisibility.Auto,
    statusTone: "muted",
    source: "system",
    installTargets: ["codex"],
    updateStatus: "local",
  },
];

export const fallbackSkillFiles = [
  { name: "SKILL.md", kind: "file" },
  { name: "references", kind: "folder" },
  { name: "references/example.md", kind: "file" },
];

export const fallbackSessions = [
  {
    id: "s1",
    title: "Review skill routing",
    project: "tendi",
    agent: "Codex",
    startedAt: "09:58",
    updatedAt: "10:56",
    time: "10:56",
    messages: 3,
  },
  {
    id: "s2",
    title: "Update desktop UI",
    project: "tendi",
    agent: "Claude",
    startedAt: "09:12",
    updatedAt: "09:40",
    time: "09:40",
    messages: 5,
  },
];

export const fallbackTranscript = [
  { type: "user", body: "Refactor the auth middleware.", time: "09:11" },
  { type: "assistant", body: "I'll read the current middleware first.", time: "09:11" },
  {
    type: "tool",
    body: "cat src/middleware/auth.rs",
    command: "cat src/middleware/auth.rs",
    result: "pub fn auth_middleware() {\n    // ...\n}",
    durationMs: 42,
    tag: "read_file",
  },
];

export const SIDEBAR_SIZE = "200px";
/** Aligns traffic-light top/left inset with expanded sidebar tab icons (`.nav` 10px + `.navItem` 10px). */
export const TRAFFIC_LIGHT_INSET = 20;
/** Close→zoom span: 2×23pt origin spacing + 14pt button width on current macOS. */
export const TRAFFIC_LIGHT_CLUSTER_WIDTH = 60;
/** Equal left/right inset around traffic lights when sidebar is collapsed. */
export const COLLAPSED_SIDEBAR_SIZE = `${TRAFFIC_LIGHT_INSET * 2 + TRAFFIC_LIGHT_CLUSTER_WIDTH}px`;

export type FreezeColumnConfig = { defaultWidth: number; min: number; max: number };

export const SESSION_FREEZE_COLUMN: FreezeColumnConfig = { defaultWidth: 360, min: 220, max: 560 };

export const RULE_FREEZE_COLUMN: FreezeColumnConfig = { defaultWidth: 292, min: 220, max: 520 };

export const HOOK_FREEZE_COLUMN: FreezeColumnConfig = { defaultWidth: 330, min: 220, max: 560 };

export const MCP_FREEZE_COLUMN: FreezeColumnConfig = { defaultWidth: 96, min: 78, max: 160 };


export const MARQUEE_DRAG_THRESHOLD = 4;

export const MARQUEE_AUTO_SCROLL_EDGE = 44;

export const MARQUEE_AUTO_SCROLL_MAX_SPEED = 18;
