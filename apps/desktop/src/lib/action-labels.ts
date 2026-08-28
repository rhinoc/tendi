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
  checkForUpdates: "Check for updates",
  enable: "Enable",
  disable: "Disable",
} as const;

export const promptActionLabels = {
  save: "Save prompt",
  saving: "Saving prompt",
  saved: "Prompt saved",
  saveFailed: "Could not save prompt.",
} as const;

export const logExportLabels = {
  idle: "Export logs",
  loading: "Exporting logs",
  success: "Logs exported",
  error: "Export failed",
  retry: "Export logs again",
} as const;

function selectionDisplayNoun(noun: string, count: number): string {
  const normalizedNoun = noun.trim();
  return count === 1
    ? normalizedNoun
    : normalizedNoun.endsWith("s") ? normalizedNoun : `${normalizedNoun}s`;
}

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

export function revealPathLabel(subject?: string): string {
  const normalizedSubject = subject?.trim();
  return normalizedSubject ? `Reveal ${normalizedSubject} in Finder` : actionLabels.revealInFinder;
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
  const displayNoun = selectionDisplayNoun(noun, count);
  if (count === 1) return `Delete ${displayNoun}`;
  return `Delete selected ${displayNoun}`;
}

export function selectionDeleteLoadingLabel(noun: string, count: number): string {
  return `Deleting ${selectionDisplayNoun(noun, count)}`;
}

export function selectionDeleteErrorLabel(noun: string, count: number): string {
  return `Could not delete ${selectionDisplayNoun(noun, count)}.`;
}

export function deleteConfirmationDescription(noun: string, count: number): string {
  const displayNoun = selectionDisplayNoun(noun, count);
  return `Delete the selected ${displayNoun}? This action cannot be undone.`;
}
