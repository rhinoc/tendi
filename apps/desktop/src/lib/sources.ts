import { createElement, type ReactNode } from "react";
import { Folder, GitBranch, Globe } from "lucide-react";
import githubIcon from "@lobehub/icons-static-svg/icons/github.svg";

import { GitLabSourceIcon } from "./agents.ts";
import { titleValue } from "./strings.ts";
import { TauriCommand, safeInvoke } from "./tauri.ts";

export type SkillLike = Record<string, unknown> & {
  paths?: Array<{
    source?: string | null;
    source_kind?: string | null;
    source_relative_path?: string | null;
    path?: string | null;
    root?: string | null;
    install_target?: string | null;
    scope?: string | null;
    agent?: string | null;
  }>;
  source_summary?: string | null;
  source?: string | null;
};

export type SourceDetails = {
  kind: string;
  label: string;
  relativePath?: string | null;
  value: string;
};

export type RemoteSource = {
  host: string;
  path: string;
  protocol: string;
};

export function friendlySource(source: unknown): string {
  const text = `${source ?? ""}`.trim();
  if (!text) return "";
  if (text.toLowerCase() === "unknown") return "";
  if (text.toLowerCase().includes("system")) return "System";
  return titleValue(text.split(":")[0]);
}

export function skillSourceDetails(skill: SkillLike): SourceDetails {
  const sourcePath = skill.paths?.find((path) => path.source) ?? skill.paths?.find((path) => path.source_kind);
  const summary = `${skill.source ?? ""}`;
  const [summaryKind, ...summaryRest] = summary.split(":");
  const summaryValue = summaryRest.length ? summaryRest.join(":") : "";
  const kind = sourcePath?.source_kind ?? summaryKind ?? "";
  return {
    kind,
    label: friendlySource(kind),
    relativePath: sourcePath?.source_relative_path,
    value: sourcePath?.source ?? summaryValue ?? summary,
  };
}

export function isWebSource(value: unknown): boolean {
  return /^https?:\/\//i.test(`${value ?? ""}`);
}

export function normalizeGitPath(path: unknown): string {
  return `${path ?? ""}`.replace(/^\/+/, "").replace(/\.git$/i, "");
}

export function parseRemoteSource(value: unknown): RemoteSource | null {
  const text = `${value ?? ""}`.trim();
  if (!text) return null;
  try {
    const parsed = new URL(text);
    if (parsed.hostname) {
      return {
        host: parsed.hostname.replace(/^www\./i, "").toLowerCase(),
        path: normalizeGitPath(parsed.pathname),
        protocol: parsed.protocol.replace(/:$/, "").toLowerCase(),
      };
    }
  } catch {
    // Fall through to scp-like git remotes.
  }
  const scpLike = text.match(/^[^@\s]+@([^:/\s]+)[:/]([^#?\s]+)$/);
  if (scpLike) {
    return {
      host: scpLike[1].replace(/^www\./i, "").toLowerCase(),
      path: normalizeGitPath(scpLike[2]),
      protocol: "ssh",
    };
  }
  return null;
}

export function remoteRepositoryLabel(value: unknown): string {
  const remote = parseRemoteSource(value);
  return remote?.path || `${value ?? ""}`.trim();
}

export function isGitSource(value: unknown, kind?: unknown): boolean {
  const text = `${value ?? ""}`.trim();
  const sourceKind = `${kind ?? ""}`.toLowerCase();
  const remote = parseRemoteSource(text);
  return (
    sourceKind === "git" ||
    sourceKind === "github" ||
    /^git(\+|:|@)/i.test(text) ||
    /^ssh:\/\//i.test(text) ||
    Boolean(remote?.protocol?.startsWith("git")) ||
    Boolean(remote?.host?.includes("github.")) ||
    Boolean(remote?.host?.includes("gitlab.")) ||
    /\.git(?:[#?].*)?$/i.test(text)
  );
}

export function normalizeSourceFilePath(relativePath: unknown): string {
  const path = `${relativePath ?? ""}`.trim().replace(/^\/+|\/+$/g, "");
  if (!path) return "";
  return path.endsWith("/SKILL.md") || path === "SKILL.md" ? path : `${path}/SKILL.md`;
}

export function encodeRemotePath(path: string): string {
  return path.split("/").map((part) => encodeURIComponent(part)).join("/");
}

export function sourceRemoteDetails(value: unknown, kind?: unknown): RemoteSource | null {
  const text = `${value ?? ""}`.trim();
  const sourceKind = `${kind ?? ""}`.toLowerCase();
  if (sourceKind === "github" && /^[^/\s]+\/[^/\s]+(?:\.git)?$/i.test(text)) {
    return { host: "github.com", path: normalizeGitPath(text), protocol: "https" };
  }
  return parseRemoteSource(text);
}

export function remoteSkillFileUrl(
  remote: RemoteSource | null | undefined,
  relativePath: unknown,
  kind?: unknown,
): string | null {
  const sourceFilePath = normalizeSourceFilePath(relativePath);
  if (!sourceFilePath || !remote?.host || !remote.path) return null;
  const repoPath = normalizeGitPath(remote.path);
  const filePath = encodeRemotePath(sourceFilePath);
  if (`${kind ?? ""}`.toLowerCase() === "github" || remote.host.includes("github.")) {
    return `https://${remote.host}/${repoPath}/blob/HEAD/${filePath}`;
  }
  if (`${kind ?? ""}`.toLowerCase() === "gitlab" || remote.host.includes("gitlab.")) {
    return `https://${remote.host}/${repoPath}/-/blob/HEAD/${filePath}`;
  }
  return null;
}

export function sourceOpenUrl(value: unknown, kind?: unknown, relativePath?: unknown): string | null {
  const text = `${value ?? ""}`.trim();
  if (!text) return null;
  const remote = sourceRemoteDetails(text, kind);
  if (!isGitSource(text, kind)) return isWebSource(text) ? text : null;
  const fileUrl = remoteSkillFileUrl(remote, relativePath, kind);
  if (fileUrl) return fileUrl;
  return null;
}

export function sourceLocalPath(value: unknown): string {
  const text = `${value ?? ""}`.trim();
  try {
    const parsed = new URL(text);
    if (parsed.protocol === "file:") return decodeURIComponent(parsed.pathname);
  } catch {
    // Plain local paths are already in the right shape.
  }
  return text;
}

export function sourceIconDetails(source: SourceDetails | null | undefined): { label: string; icon: ReactNode } {
  const value = source?.value ?? "";
  const kind = `${source?.kind ?? ""}`.toLowerCase();
  const remote = parseRemoteSource(value);
  const host = remote?.host ?? "";
  if (kind === "github" || host.includes("github.")) {
    return {
      label: "GitHub",
      icon: createElement("img", { className: "skillInfoSourceImage", src: githubIcon, alt: "", draggable: false }),
    };
  }
  if (kind === "gitlab" || host.includes("gitlab.")) {
    return { label: "GitLab", icon: createElement(GitLabSourceIcon) };
  }
  if (isGitSource(value, kind)) return { label: "Git", icon: createElement(GitBranch, { size: 13 }) };
  if (isWebSource(value)) return { label: "Web", icon: createElement(Globe, { size: 13 }) };
  return { label: source?.label ?? "", icon: createElement(Folder, { size: 13 }) };
}

export function pathLooksLikePluginCache(path: unknown): boolean {
  const value = `${path ?? ""}`.toLowerCase().replaceAll("\\", "/");
  return value.includes("/plugins/cache/");
}

export function openSource(value: unknown, kind?: unknown, relativePath?: unknown): void {
  if (!value) return;
  const url = sourceOpenUrl(value, kind, relativePath);
  if (url) {
    safeInvoke(TauriCommand.OpenUrl, { url });
    return;
  }
  safeInvoke(TauriCommand.RevealInFinder, { path: sourceLocalPath(value) });
}
