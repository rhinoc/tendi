import type { BadgeTone } from "../../components/shared/Badge.tsx";

export const SKILL_BADGE_TONES = {
  update: "warning",
  wrapper: "neutral",
} as const satisfies Record<"update" | "wrapper", BadgeTone>;
