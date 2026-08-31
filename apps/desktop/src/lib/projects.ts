import { ColumnDataType, type ColumnDef } from "../components/DataTable.types";
import { logger } from "./logger.ts";

export type ProjectSummary = {
  id: string;
  name?: string;
  rootPath?: string;
  remoteUrl?: string | null;
  status?: string;
  lastScannedAt?: string | null;
};

export enum MissingSessionProjectPolicy {
  Show = "show",
  Hide = "hide",
  MergeByName = "merge-by-name",
}

export type SessionProjectSummary = {
  id: string;
  name?: string;
  missing?: boolean;
  paths?: string[];
};

export function normalizeMissingSessionProjectPolicy(value: unknown): MissingSessionProjectPolicy {
  if (value === MissingSessionProjectPolicy.Hide) return MissingSessionProjectPolicy.Hide;
  if (value === MissingSessionProjectPolicy.MergeByName) return MissingSessionProjectPolicy.MergeByName;
  return MissingSessionProjectPolicy.Show;
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
    session.logicalProjectId && project.id.trim() === session.logicalProjectId.trim()
  ) || (session.workspacePath && (project.paths ?? []).some((path) => normalizedPath(path) === normalizedPath(session.workspacePath ?? ""))));
  if (summary?.missing && policy === MissingSessionProjectPolicy.Hide) return null;
  if (policy !== MissingSessionProjectPolicy.MergeByName) return { key: session.key, label: session.label, title: session.title };

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
    key: JSON.stringify(["scanned-project", scannedProject.id]),
    label: `${scannedProject.name ?? ""}`.trim(),
    title: `${scannedProject.remoteUrl ?? ""}`.trim() || `${scannedProject.rootPath ?? ""}`.trim(),
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
  const normalized = normalizedPath(`${path ?? ""}`);
  if (!normalized) {
    return scopeInvariantFailure("scope source path is missing", {
      path: path ?? null,
      projectCount: projects.length,
    });
  }
  const project = projectForPath(normalized, projects);
  if (!project) return "Global";
  const name = `${project.name ?? ""}`.trim();
  if (!name) {
    return scopeInvariantFailure("matched project is missing a name", {
      path: normalized,
      projectRoot: project.rootPath ?? null,
    });
  }
  return name;
}

function scopeInvariantFailure(reason: string, fields: Record<string, unknown>): never {
  const error = new Error(`Scope invariant violated: ${reason}`);
  logger.error("scope invariant violated", { reason, ...fields, error });
  throw error;
}

export function scopeColumn<TRow>(projects: readonly ProjectSummary[], getPath: (row: TRow) => string | null | undefined): ColumnDef<TRow> {
  return {
    key: "scope",
    header: "Scope",
    label: "Scope",
    type: ColumnDataType.Enum,
    width: "128px",
    sortable: true,
    sortValue: (row) => scopeNameForPath(getPath(row), projects).toLowerCase(),
    groupBy: (row) => scopeNameForPath(getPath(row), projects),
    value: (row) => scopeNameForPath(getPath(row), projects),
  };
}
