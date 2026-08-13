import { basename } from "./strings.ts";
import { fallbackSkillFiles } from "./constants.ts";
import type { NormalizedSkill } from "./skills.ts";

export type SkillFileEntry = {
  name: string;
  kind: string;
  path?: string;
};

export type FileTreeRow = {
  file: SkillFileEntry;
  depth: number;
  isFolder: boolean;
};

export function parentPath(path: unknown): string {
  const parts = `${path ?? ""}`.split("/").filter(Boolean);
  return parts.slice(0, -1).join("/");
}

export function joinRelativePath(parent: string, name: string): string {
  return parent ? `${parent}/${name}` : name;
}

export function displayFileName(name: string): string {
  return basename(name);
}

export function buildFileTreeRows(files: SkillFileEntry[], collapsedFolders: Set<string>): FileTreeRow[] {
  const rows: FileTreeRow[] = [];
  const entries = new Map<string, SkillFileEntry>();
  for (const file of files) {
    entries.set(file.name, file);
    const parts = file.name.split("/");
    for (let index = 1; index < parts.length; index += 1) {
      const folder = parts.slice(0, index).join("/");
      if (!entries.has(folder)) entries.set(folder, { name: folder, kind: "folder" });
    }
  }

  const children = new Map<string, SkillFileEntry[]>();
  for (const file of entries.values()) {
    const parent = parentPath(file.name);
    if (!children.has(parent)) children.set(parent, []);
    children.get(parent)!.push(file);
  }

  for (const childList of children.values()) {
    childList.sort((a, b) => {
      const aFolder = a.kind === "folder";
      const bFolder = b.kind === "folder";
      return Number(bFolder) - Number(aFolder) || displayFileName(a.name).localeCompare(displayFileName(b.name));
    });
  }

  function visit(parent: string, depth: number) {
    for (const file of children.get(parent) ?? []) {
      const isFolder = file.kind === "folder";
      rows.push({ file, depth, isFolder });
      if (isFolder && !collapsedFolders.has(file.name)) visit(file.name, depth + 1);
    }
  }

  visit("", 0);
  return rows;
}

export function normalizeSkillFileEntries(result: unknown): SkillFileEntry[] {
  if (!Array.isArray(result) || result.length === 0) return fallbackSkillFiles;
  return result.map((file: Record<string, unknown>) => ({
    name: `${file.relative_path ?? file.name}`,
    kind: `${file.kind ?? "file"}`,
    path: file.path as string | undefined,
  }));
}

export function preferredSkillFileName(files: SkillFileEntry[]): string {
  return files.find((file) => file.kind === "file" && file.name === "SKILL.md")?.name
    ?? files.find((file) => file.kind === "file")?.name
    ?? "SKILL.md";
}

export function uniqueChildPath(files: SkillFileEntry[], parent: string, kind: string): string {
  const existing = new Set(files.map((file) => file.name));
  const stem = kind === "folder" ? "new-folder" : "new-file";
  const extension = kind === "folder" ? "" : ".md";
  for (let index = 1; index < 1000; index += 1) {
    const name = index === 1 ? `${stem}${extension}` : `${stem}-${index}${extension}`;
    const candidate = joinRelativePath(parent, name);
    if (!existing.has(candidate)) return candidate;
  }
  return joinRelativePath(parent, `${stem}-${Date.now()}${extension}`);
}

export function fallbackSkillContent(skill: { name: string; description?: string; visibility: string }): string {
  return `---\nname: ${skill.name}\ndescription: ${skill.description ?? ""}\ntendi:\n  visibility: ${skill.visibility.toLowerCase()}\n---\n\n# ${skill.name}\n\nUse this skill when the user explicitly chooses it.\n`;
}

export function splitMarkdownFrontmatter(content: string): { frontmatter: string; body: string } {
  if (!content.startsWith("---\n")) return { frontmatter: "", body: content };
  const endIndex = content.indexOf("\n---", 4);
  if (endIndex < 0) return { frontmatter: "", body: content };
  const afterFence = endIndex + 4;
  let nextOffset = content[afterFence] === "\n" ? afterFence + 1 : afterFence;
  while (content[nextOffset] === "\n") nextOffset += 1;
  return {
    frontmatter: content.slice(0, nextOffset),
    body: content.slice(nextOffset),
  };
}

export function normalizeLinkHref(value: string): string {
  const href = value.trim();
  if (!href) return "";
  if (/^(https?:|mailto:|tel:|#|\/)/i.test(href)) return href;
  return `https://${href}`;
}

export function isMarkdownPath(value: unknown): boolean {
  const path = `${value ?? ""}`.toLowerCase();
  return path.endsWith(".md") || path.endsWith(".mdc") || path.endsWith(".markdown");
}

export function isYamlPath(value: unknown): boolean {
  const path = `${value ?? ""}`.toLowerCase();
  return path.endsWith(".yaml") || path.endsWith(".yml");
}
