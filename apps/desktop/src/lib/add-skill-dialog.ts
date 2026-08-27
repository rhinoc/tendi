export function shouldShowSkillQuickSelect(availableCount: number, existingCount: number) {
  return availableCount > 0 && existingCount > 0;
}

export type SkillSourcePageSnapshot<T> = {
  kind: "recommended" | "matches";
  source: string;
  marketplaceResults: T[];
};

export function captureSkillSourcePage<T>(source: string, marketplaceResults: T[]): SkillSourcePageSnapshot<T> {
  return {
    kind: source.trim() ? "matches" : "recommended",
    source,
    marketplaceResults,
  };
}

export function restoreSkillSourcePage<T>(page: SkillSourcePageSnapshot<T>) {
  const source = page.kind === "recommended" ? "" : page.source;
  return {
    source,
    marketplaceQuery: source,
    marketplaceResults: page.marketplaceResults,
  };
}

export function skillSourceErrorMessage(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  if (/git clone failed/i.test(message) && /repository not found/i.test(message)) {
    return "Repository not found.";
  }
  return message.replace(/^DaemonCommandError:\s*/i, "");
}

export function isSkillSourceActionReady(source: string, isDirectSource: boolean, target: string) {
  return Boolean(source.trim()) && (!isDirectSource || Boolean(target.trim()));
}

export function resolveSkillInstallTarget<T extends { id: string }>(target: string, options: T[]) {
  return options.some((option) => option.id === target) ? target : options[0]?.id ?? "";
}
