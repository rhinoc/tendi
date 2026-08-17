import { useMemo, useState, type KeyboardEvent } from "react";

import { useResponsiveChartSize } from "../../components/shared/chart/useResponsiveChartSize.ts";
import { formatSessionTitle } from "../../lib/session-preview.ts";

// Lieflat L12 · Type Colonnade · templates/lupi-gallery.html · "Forty-four repos, ten owners"
// Adapted to the real three-hop relation: skill → session → project.
export type SkillSessionProjectItem = {
  key: string;
  skillKey: string;
  skillLabel: string;
  sessionLabel: string;
  sessionTitle?: string;
  projectKey: string;
  projectLabel: string;
};

type ChartGroup = {
  key: string;
  label: string;
  count: number;
};

type ActiveNode =
  | { type: "skill"; key: string }
  | { type: "session"; key: string }
  | { type: "project"; key: string }
  | null;

function compactLabel(value: string, maxLength: number) {
  return value.length > maxLength ? `${value.slice(0, maxLength - 1)}…` : value;
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function labelOpacityForFontSize(fontSize: number, minFontSize: number, maxFontSize: number) {
  const sizeRatio = clamp((fontSize - minFontSize) / Math.max(1, maxFontSize - minFontSize), 0, 1);
  return 0.42 + sizeRatio * 0.58;
}

function activateOnKeyboard(event: KeyboardEvent<SVGGElement>, activate: () => void) {
  if (event.key !== "Enter" && event.key !== " ") return;
  event.preventDefault();
  activate();
}

export function SkillSessionProjectChart({
  items,
  ariaLabel,
  onSessionClick,
}: {
  items: SkillSessionProjectItem[];
  ariaLabel: string;
  onSessionClick?: (item: SkillSessionProjectItem) => void;
}) {
  const [activeNode, setActiveNode] = useState<ActiveNode>(null);
  const { containerRef: chartRef, size: viewport } = useResponsiveChartSize({ width: 620, height: 320 });
  const skills = useMemo(() => groupSkillSessionProjectItems(items, (item) => [item.skillKey, item.skillLabel]), [items]);
  const projects = useMemo(() => groupSkillSessionProjectItems(items, (item) => [item.projectKey, item.projectLabel]), [items]);
  if (items.length === 0) return null;

  const width = 620;
  const height = Math.max(320, Math.round(width * viewport.height / Math.max(1, viewport.width)));
  const laneTop = 26;
  const laneBottom = height - 46;
  const skillX = 118;
  const sessionX = 252;
  const projectX = 480;
  const svgScale = Math.max(0.01, Math.min(viewport.width / width, viewport.height / height));
  const laneHeight = laneBottom - laneTop;
  const fontSizeForRows = (rowCount: number, minPixels: number, maxPixels: number) => {
    const rowGapPixels = laneHeight / Math.max(1, rowCount) * svgScale;
    return clamp(rowGapPixels * 0.68, minPixels, maxPixels) / svgScale;
  };
  const sessionFontSize = fontSizeForRows(items.length, 5.5, 10);
  const skillFontSize = fontSizeForRows(skills.length, 7, 13);
  const projectFontSize = fontSizeForRows(projects.length, 7, 13);
  const sessionLabelOpacity = labelOpacityForFontSize(sessionFontSize, 5.5, 10);
  const skillLabelOpacity = labelOpacityForFontSize(skillFontSize, 7, 13);
  const projectLabelOpacity = labelOpacityForFontSize(projectFontSize, 7, 13);
  const distributeY = (index: number, count: number) => count <= 1
    ? (laneTop + laneBottom) / 2
    : laneTop + index * (laneBottom - laneTop) / (count - 1);
  const sessionY = (index: number) => distributeY(index, items.length);
  const skillY = (index: number) => distributeY(index, skills.length);
  const projectY = (index: number) => distributeY(index, projects.length);
  const skillIndex = new Map(skills.map((group, index) => [group.key, index]));
  const projectIndex = new Map(projects.map((group, index) => [group.key, index]));

  return (
    <div ref={chartRef} className="skillSessionProjectChart" role="group" aria-label={ariaLabel}>
      <svg viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="xMidYMid meet" aria-label={ariaLabel}>
        {items.map((item, index) => {
          const y = sessionY(index);
          const sourceY = skillY(skillIndex.get(item.skillKey) ?? 0);
          const targetY = projectY(projectIndex.get(item.projectKey) ?? 0);
          const isDirectlyConnected = activeNode === null
            || (activeNode.type === "skill" && activeNode.key === item.skillKey)
            || (activeNode.type === "session" && activeNode.key === item.key)
            || (activeNode.type === "project" && activeNode.key === item.projectKey);
          const isActive = activeNode?.type === "session" && activeNode.key === item.key;
          const connectionClass = activeNode === null ? "" : isDirectlyConnected ? " isActive" : " isDimmed";
          const activate = () => onSessionClick?.(item);
          const sessionLabel = formatSessionTitle(item.sessionLabel) || "Untitled session";
          const sessionTitle = formatSessionTitle(item.sessionTitle ?? item.sessionLabel) || sessionLabel;
          return (
            <g
              key={item.key}
              className={`skillSessionProjectItem${connectionClass}`}
              role={onSessionClick ? "button" : undefined}
              tabIndex={onSessionClick ? 0 : undefined}
              aria-label={onSessionClick ? `Open ${sessionTitle}` : undefined}
              onClick={onSessionClick ? activate : undefined}
              onKeyDown={onSessionClick ? (event) => activateOnKeyboard(event, activate) : undefined}
              onMouseEnter={() => setActiveNode({ type: "session", key: item.key })}
              onMouseLeave={() => setActiveNode(null)}
              onFocus={() => setActiveNode({ type: "session", key: item.key })}
              onBlur={() => setActiveNode(null)}
            >
              <path className="skillSessionProjectPath" d={`M ${skillX + 8} ${sourceY} C 145 ${sourceY} 205 ${y} ${sessionX - 8} ${y}`} />
              <path className="skillSessionProjectPath" d={`M ${sessionX + 8} ${y} C 360 ${y} 430 ${targetY} ${projectX - 8} ${targetY}`} />
              <circle className="skillSessionProjectSessionNode" cx={sessionX} cy={y} r="2.1" />
              <text
                className="skillSessionProjectSessionLabel"
                fontSize={sessionFontSize}
                style={{ opacity: isActive ? 1 : sessionLabelOpacity }}
                x={sessionX + 12}
                y={y + 2}
              >
                {compactLabel(sessionLabel, 30)}
              </text>
            </g>
          );
        })}
        {skills.map((skill, index) => {
          const y = skillY(index);
          const radius = Math.min(14, 3 + skill.count * 0.45);
          const isActive = activeNode?.type === "skill" && activeNode.key === skill.key;
          return (
            <g
              key={skill.key}
              className={`skillSessionProjectSkill${isActive ? " isActive" : ""}`}
              onMouseEnter={() => setActiveNode({ type: "skill", key: skill.key })}
              onMouseLeave={() => setActiveNode(null)}
              onFocus={() => setActiveNode({ type: "skill", key: skill.key })}
              onBlur={() => setActiveNode(null)}
            >
              <circle className="skillSessionProjectSkillNode" cx={skillX} cy={y} r={radius} />
              <text
                className="skillSessionProjectSkillLabel"
                fontSize={skillFontSize}
                style={{ opacity: isActive ? 1 : skillLabelOpacity }}
                x={skillX - radius - 7}
                y={y + 2.5}
                textAnchor="end"
              >
                {compactLabel(skill.label, 14)}
              </text>
            </g>
          );
        })}
        {projects.map((project, index) => {
          const y = projectY(index);
          const radius = Math.min(16, 3 + project.count * 0.55);
          const isActive = activeNode?.type === "project" && activeNode.key === project.key;
          const activate = () => setActiveNode((current) => current?.type === "project" && current.key === project.key ? null : { type: "project", key: project.key });
          return (
            <g
              key={project.key}
              className={`skillSessionProjectProject${isActive ? " isActive" : ""}`}
              role="button"
              tabIndex={0}
              aria-label={`${project.label}, ${project.count} linked sessions`}
              onKeyDown={(event) => activateOnKeyboard(event, activate)}
              onMouseEnter={() => setActiveNode({ type: "project", key: project.key })}
              onMouseLeave={() => setActiveNode(null)}
              onFocus={() => setActiveNode({ type: "project", key: project.key })}
              onBlur={() => setActiveNode(null)}
            >
              <circle className="skillSessionProjectProjectNode" cx={projectX} cy={y} r={radius} />
              <text
                className="skillSessionProjectProjectLabel"
                fontSize={projectFontSize}
                style={{ opacity: isActive ? 1 : projectLabelOpacity }}
                x={projectX + radius + 8}
                y={y + 2.5}
              >
                {`${compactLabel(project.label, 20)} · ${project.count}`}
              </text>
            </g>
          );
        })}
      </svg>
    </div>
  );
}

export function groupSkillSessionProjectItems(items: SkillSessionProjectItem[], getGroup: (item: SkillSessionProjectItem) => [string, string]): ChartGroup[] {
  const groups = new Map<string, ChartGroup>();
  for (const item of items) {
    const [key, label] = getGroup(item);
    const existing = groups.get(key);
    if (existing) existing.count += 1;
    else groups.set(key, { key, label, count: 1 });
  }
  return [...groups.values()].sort((left, right) => right.count - left.count || left.label.localeCompare(right.label));
}
