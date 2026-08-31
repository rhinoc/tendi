export type PromptRecord = {
  id: string;
  title: string;
  tags: string[];
  body: string;
  createdAt: string;
  updatedAt: string;
};

export function normalizePromptTags(value: string[]): string[] {
  const tags: string[] = [];
  for (const tag of value.flatMap((item) => item.split(","))) {
    const trimmed = tag.trim();
    if (!trimmed || tags.includes(trimmed)) continue;
    tags.push(trimmed);
  }
  return tags;
}

export function normalizePrompt(prompt: Record<string, unknown>): PromptRecord | undefined {
  const id = prompt.id;
  if (
    typeof id !== "string"
    || !id.trim()
    || typeof prompt.title !== "string"
    || !Array.isArray(prompt.tags)
    || !prompt.tags.every((tag): tag is string => typeof tag === "string")
    || typeof prompt.created_at !== "string"
    || typeof prompt.updated_at !== "string"
  ) return undefined;
  const body = typeof prompt.body === "string" ? prompt.body : "";
  const tags = normalizePromptTags(prompt.tags);
  return {
    id,
    title: prompt.title,
    tags,
    body,
    createdAt: prompt.created_at,
    updatedAt: prompt.updated_at,
  };
}

export function promptTagsLabel(prompt: Pick<PromptRecord, "tags">): string {
  return prompt.tags.join(", ");
}

export function promptPreview(prompt: Pick<PromptRecord, "body">): string {
  const body = prompt.body.replace(/\s+/g, " ").trim();
  return body;
}

export function promptTitleFromBody(body: string): string {
  const firstLine = body.split(/\r?\n/).find((line) => line.trim())?.trim().replace(/\s+/g, " ") ?? "";
  if (!firstLine) return "";
  return firstLine.length > 80 ? `${firstLine.slice(0, 79).trimEnd()}…` : firstLine;
}
