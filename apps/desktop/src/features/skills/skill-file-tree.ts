import type { SkillFileEntry } from "../../lib/file-tree.ts";

export function skillFileTreeTone(entry: SkillFileEntry): string {
  if (entry.kind === "folder") return "fileTone-folder";
  const name = entry.name.toLowerCase();
  if (name.endsWith(".json") || name.endsWith(".yaml") || name.endsWith(".yml")) return "fileTone-data";
  if (name.endsWith(".css") || name.endsWith(".scss") || name.endsWith(".less")) return "fileTone-style";
  if (name.endsWith(".md") || name.endsWith(".mdc") || name.endsWith(".markdown")) return "fileTone-document";
  if (name.endsWith(".ts") || name.endsWith(".tsx") || name.endsWith(".js") || name.endsWith(".jsx")) return "fileTone-code";
  return "fileTone-file";
}
