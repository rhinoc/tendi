export type PromptRecord = {
  id: string;
  title: string;
  tags: string[];
  body: string;
  createdAt: string;
  updatedAt: string;
};

export function normalizePromptTags(value: unknown): string[] {
  const rawTags = Array.isArray(value) ? value : `${value ?? ""}`.split(",");
  const tags: string[] = [];
  for (const tag of rawTags.flatMap((item) => `${item ?? ""}`.split(","))) {
    const trimmed = tag.trim();
    if (!trimmed || tags.includes(trimmed)) continue;
    tags.push(trimmed);
  }
  return tags;
}

export function normalizePrompt(prompt: Record<string, unknown>, index: number): PromptRecord {
  const tags = normalizePromptTags(prompt.tags ?? prompt.tag ?? prompt.category ?? []);
  return {
    id: `${prompt.id ?? `prompt-${index}`}`,
    title: `${prompt.title || "Untitled prompt"}`,
    tags,
    body: `${prompt.body ?? ""}`,
    createdAt: `${prompt.created_at ?? prompt.createdAt ?? ""}`,
    updatedAt: `${prompt.updated_at ?? prompt.updatedAt ?? ""}`,
  };
}

export function promptTagsLabel(prompt: Pick<PromptRecord, "tags">): string {
  return prompt.tags?.length ? prompt.tags.join(", ") : "Untagged";
}

export function promptPreview(prompt: Pick<PromptRecord, "body">): string {
  const body = `${prompt.body ?? ""}`.replace(/\s+/g, " ").trim();
  return body || "Empty prompt";
}

export function promptTitleFromBody(body: string): string {
  const firstLine = body.split(/\r?\n/).find((line) => line.trim())?.trim().replace(/\s+/g, " ") ?? "";
  if (!firstLine) return "Saved prompt";
  return firstLine.length > 80 ? `${firstLine.slice(0, 79).trimEnd()}…` : firstLine;
}
