import type { ColumnDef } from "../components/DataTable.types";

export type ProjectSummary = {
  id?: string;
  name?: string;
  rootPath?: string;
  remoteUrl?: string | null;
  status?: string;
  lastScannedAt?: string | null;
};

export type MissingSessionProjectPolicy = "show" | "hide" | "merge-by-name";

export type SessionProjectSummary = {
  id?: string;
  name?: string;
  missing?: boolean;
  paths?: string[];
};

export function normalizeMissingSessionProjectPolicy(value: unknown): MissingSessionProjectPolicy {
  if (value === "hide") return "hide";
  if (value === "merge-by-name") return "merge-by-name";
  return "show";
}

export function sessionProjectOptionForPaths(
  session: {
    key: string;
    label: string;
    title: string;
    logicalProjectId?: string;
    workspacePath?: string;
    repositoryPath?: string;
  },
  policy: MissingSessionProjectPolicy,
  sessionProjects: readonly SessionProjectSummary[],
  projects: readonly ProjectSummary[],
): { key: string; label: string; title: string } | null {
  const summary = sessionProjects.find((project) => (
    session.logicalProjectId && `${project.id ?? ""}`.trim() === session.logicalProjectId.trim()
  ) || (session.workspacePath && (project.paths ?? []).some((path) => normalizedPath(path) === normalizedPath(session.workspacePath ?? ""))));
  if (summary?.missing && policy === "hide") return null;
  if (policy !== "merge-by-name") return { key: session.key, label: session.label, title: session.title };

  const normalizedName = session.label.trim().toLocaleLowerCase();
  const sameNameProjects = projects.filter((project) => {
    const projectName = `${project.name ?? ""}`.trim().toLocaleLowerCase();
    const rootName = normalizedPath(`${project.rootPath ?? ""}`).split("/").filter(Boolean).pop()?.toLocaleLowerCase() ?? "";
    return normalizedName && (projectName === normalizedName || rootName === normalizedName);
  });
  const scannedProject = projectForPath(session.workspacePath, projects)
    ?? projectForPath(session.repositoryPath, projects)
    ?? (summary?.missing && sameNameProjects.length === 1 ? sameNameProjects[0] : null);
  if (!scannedProject) return { key: session.key, label: session.label, title: session.title };
  return {
    key: JSON.stringify(["scanned-project", scannedProject.id || scannedProject.rootPath || scannedProject.name || ""]),
    label: `${scannedProject.name ?? ""}`.trim() || session.label,
    title: `${scannedProject.remoteUrl ?? ""}`.trim() || `${scannedProject.rootPath ?? ""}`.trim() || session.title,
  };
}

function normalizedPath(value: string): string {
  const path = value.trim().replaceAll("\\", "/");
  if (path.length > 1) return path.replace(/\/+$/, "");
  return path;
}

export function projectForPath(path: string | null | undefined, projects: readonly ProjectSummary[]): ProjectSummary | null {
  const target = normalizedPath(`${path ?? ""}`);
  if (!target) return null;
  return projects
    .filter((project) => {
      const root = normalizedPath(`${project.rootPath ?? ""}`);
      return root && (target === root || target.startsWith(`${root}/`));
    })
    .sort((left, right) => normalizedPath(`${right.rootPath ?? ""}`).length - normalizedPath(`${left.rootPath ?? ""}`).length)[0] ?? null;
}

export function scopeNameForPath(path: string | null | undefined, projects: readonly ProjectSummary[]): string {
  return projectForPath(path, projects)?.name?.trim() || "Global";
}

export function scopeColumn<TRow>(projects: readonly ProjectSummary[], getPath: (row: TRow) => string | null | undefined): ColumnDef<TRow> {
  return {
    key: "scope",
    header: "Scope",
    label: "Scope",
    type: "enum",
    width: "150px",
    sortable: true,
    sortValue: (row) => scopeNameForPath(getPath(row), projects).toLowerCase(),
    groupBy: (row) => scopeNameForPath(getPath(row), projects),
    value: (row) => scopeNameForPath(getPath(row), projects),
  };
}
