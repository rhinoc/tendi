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

export enum AppPage {
  Overview = "overview",
  Skills = "skills",
  Prompts = "prompts",
  Sessions = "sessions",
  Rules = "rules",
  Hooks = "hooks",
  Mcp = "mcp",
  Config = "config",
  Settings = "settings",
  SkillDetail = "skillDetail",
}

export type NavItem = { id: Exclude<AppPage, AppPage.SkillDetail>; label: string; icon: LucideIcon };

export const navItems: NavItem[] = [
  { id: AppPage.Overview, label: "Overview", icon: LayoutDashboard },
  { id: AppPage.Skills, label: "Skills", icon: Sparkles },
  { id: AppPage.Sessions, label: "Sessions", icon: MessagesSquare },
  { id: AppPage.Rules, label: "Rules", icon: ScrollText },
  { id: AppPage.Mcp, label: "MCP", icon: Server },
  { id: AppPage.Hooks, label: "Hooks", icon: Webhook },
  { id: AppPage.Prompts, label: "Prompts", icon: MessageSquareText },
  { id: AppPage.Config, label: "Config", icon: FileCode },
  { id: AppPage.Settings, label: "Settings", icon: Settings },
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

export const SKILL_FREEZE_COLUMN: FreezeColumnConfig = { defaultWidth: 320, min: 250, max: 520 };

export const RULE_FREEZE_COLUMN: FreezeColumnConfig = { defaultWidth: 292, min: 220, max: 520 };

export const HOOK_FREEZE_COLUMN: FreezeColumnConfig = { defaultWidth: 330, min: 220, max: 560 };

export const MCP_FREEZE_COLUMN: FreezeColumnConfig = { defaultWidth: 96, min: 78, max: 160 };


export const MARQUEE_DRAG_THRESHOLD = 4;

export const MARQUEE_AUTO_SCROLL_EDGE = 44;

export const MARQUEE_AUTO_SCROLL_MAX_SPEED = 18;
