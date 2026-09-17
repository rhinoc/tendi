import type { BadgeTone } from "../../components/shared/Badge.tsx";

export const SKILL_BADGE_TONES = {
  update: "warning",
} as const satisfies Record<"update", BadgeTone>;
