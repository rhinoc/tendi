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
import { RuntimeDomainKey, type DomainKey } from "./domain.ts";

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
type DomainPage = Exclude<AppPage, AppPage.Overview | AppPage.Config | AppPage.Settings | AppPage.SkillDetail>;
export type DomainNavItem = { id: DomainPage; domain: DomainKey; label: string; icon: LucideIcon };

export const DOMAIN_NAV_ITEMS = [
  { id: AppPage.Skills, domain: RuntimeDomainKey.Skills, label: "Skills", icon: Sparkles },
  { id: AppPage.Sessions, domain: RuntimeDomainKey.Sessions, label: "Sessions", icon: MessagesSquare },
  { id: AppPage.Rules, domain: RuntimeDomainKey.Rules, label: "Rules", icon: ScrollText },
  { id: AppPage.Mcp, domain: RuntimeDomainKey.Mcp, label: "MCPs", icon: Server },
  { id: AppPage.Hooks, domain: RuntimeDomainKey.Hooks, label: "Hooks", icon: Webhook },
  { id: AppPage.Prompts, domain: RuntimeDomainKey.Prompts, label: "Prompts", icon: MessageSquareText },
] as const satisfies ReadonlyArray<DomainNavItem>;

export const navItems: NavItem[] = [
  { id: AppPage.Overview, label: "Overview", icon: LayoutDashboard },
  ...DOMAIN_NAV_ITEMS.map(({ id, label, icon }) => ({ id, label, icon })),
  { id: AppPage.Config, label: "Configs", icon: FileCode },
  { id: AppPage.Settings, label: "Settings", icon: Settings },
];

export const EMPTY_DISPLAY_VALUE = "—";

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

export const MCP_FREEZE_COLUMN: FreezeColumnConfig = { defaultWidth: 220, min: 160, max: 420 };


export const MARQUEE_DRAG_THRESHOLD = 4;

export const MARQUEE_AUTO_SCROLL_EDGE = 44;

export const MARQUEE_AUTO_SCROLL_MAX_SPEED = 18;
