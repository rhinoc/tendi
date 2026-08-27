export const actionLabels = {
  openInEditor: "Open in editor",
  revealInFinder: "Reveal in Finder",
  copyPath: "Copy path",
  pathCopied: "Path copied",
  deleteSelected: "Delete selected",
  copy: "Copy",
  copied: "Copied",
  copyFailed: "Copy failed",
  saveFailed: "Save failed",
  enable: "Enable",
  disable: "Disable",
} as const;

export function selectionCopyLabel(noun: string, count: number): string {
  const normalizedNoun = noun.trim();
  if (count === 1) return copyValueLabel(normalizedNoun);
  return `Copy selected ${normalizedNoun.endsWith("s") ? normalizedNoun : `${normalizedNoun}s`}`;
}

export function copyValueLabel(subject: string): string {
  const normalizedSubject = subject.trim();
  return normalizedSubject ? `Copy ${normalizedSubject}` : actionLabels.copy;
}

export function copiedValueLabel(subject: string): string {
  const normalizedSubject = subject.trim();
  if (!normalizedSubject) return actionLabels.copied;
  return `${normalizedSubject[0].toUpperCase()}${normalizedSubject.slice(1)} copied`;
}

export function copyPathLabel(subject?: string): string {
  const normalizedSubject = subject?.trim();
  return normalizedSubject ? copyValueLabel(`${normalizedSubject} path`) : actionLabels.copyPath;
}

export function copiedPathLabel(subject?: string): string {
  const normalizedSubject = subject?.trim();
  return normalizedSubject ? copiedValueLabel(`${normalizedSubject} path`) : actionLabels.pathCopied;
}

export function selectionCopiedLabel(noun: string, count: number): string {
  const normalizedNoun = noun.trim();
  if (count === 1) return copiedValueLabel(normalizedNoun);
  const pluralNoun = normalizedNoun.endsWith("s") ? normalizedNoun : `${normalizedNoun}s`;
  return `Selected ${pluralNoun} copied`;
}

export function selectionDeleteLabel(noun: string, count: number): string {
  const normalizedNoun = noun.trim();
  if (count === 1) return `Delete ${normalizedNoun}`;
  return `Delete selected ${normalizedNoun.endsWith("s") ? normalizedNoun : `${normalizedNoun}s`}`;
}
