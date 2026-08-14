import { Tooltip } from "./Tooltip.tsx";
import { ExternalLink, FolderOpen, Info } from "lucide-react";
import { DropdownMenu } from "radix-ui";

import {
  openSource,
  safeInvoke,
  SkillVisibility,
  skillSourceDetails,
  skillTargets,
  isWebSource,
  sourceIconDetails,
  sourceOpenUrl,
  TauriCommand,
  type SkillLike,
} from "../../lib/index.ts";
import { AgentBadge } from "./AgentBadge.tsx";
import { AgentChips } from "./AgentChips.tsx";
import { CopyButton } from "./CopyButton.tsx";
import { InfoSection } from "./InfoSection.tsx";
import type { SkillDependencyRecord } from "./SkillDependencyGraph.tsx";
import { Visibility } from "./Visibility.tsx";

export type SkillInfoMenuSkill = SkillLike & {
  name: string;
  visibility?: string;
  agents?: string[];
  dependencies?: string[];
  dependents?: string[];
};

export type SkillInfoMenuProps = {
  skill: SkillInfoMenuSkill;
  skills?: SkillDependencyRecord[];
  onOpenSkill?: (name: string) => void;
};

function relationList(names: string[], skills: SkillDependencyRecord[]) {
  const skillsByName = new Map(skills.map((item) => [item.name, item]));
  return names
    .map((name) => skillsByName.get(name) ?? { name })
    .sort((left, right) => left.name.localeCompare(right.name));
}

function SkillInfoRelations({
  rows,
  onOpenSkill,
}: {
  rows: { label: string; skills: SkillDependencyRecord[] }[];
  onOpenSkill?: (name: string) => void;
}) {
  return (
    <table className="skillInfoRelationsTable">
      <tbody>
        {rows.map((row) => (
          <tr key={row.label}>
            <th scope="row">{row.label}</th>
            <td>
              <div className="skillInfoRelationList">
                {row.skills.map((relatedSkill) => (
                  <Tooltip key={relatedSkill.name} content={relatedSkill.description || relatedSkill.name}><button
                    className="skillInfoRelationChip"
                    key={relatedSkill.name}
                    disabled={!onOpenSkill}
                    onClick={() => onOpenSkill?.(relatedSkill.name)}
                  >
                    {relatedSkill.name}
                  </button></Tooltip>
                ))}
              </div>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function SkillInfoMenu({ skill, skills = [], onOpenSkill }: SkillInfoMenuProps) {
  const sourceDetails = skillSourceDetails(skill);
  const sourceIcon = sourceIconDetails(sourceDetails);
  const sourceValue = sourceDetails.value;
  const sourceUrl = sourceOpenUrl(sourceValue, sourceDetails.kind, sourceDetails.relativePath);
  const installLocations = skillTargets(skill);
  const dependencies = relationList(skill.dependencies ?? [], skills);
  const dependents = relationList(skill.dependents ?? [], skills);
  const relationRows = [
    { label: "Depends on", skills: dependencies },
    { label: "Used by", skills: dependents },
  ].filter((row) => row.skills.length > 0);
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button className="headerGhostButton" aria-label="Show skill info">
          <Info size={15} />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content className="skillInfoContent" align="end" sideOffset={8} data-no-drag onMouseDown={(event) => event.stopPropagation()}>
          <div className="skillInfoHeader">
            <span>Skill info</span>
            <strong>{skill.name}</strong>
          </div>
          <div className="skillInfoSections">
            <InfoSection label="Source">
                {sourceValue ? (
                  <Tooltip content={sourceUrl ? `Open ${sourceIcon.label} source` : `Reveal ${sourceIcon.label} source in Finder`}><button
                    className="skillInfoSourceIcon"
                    aria-label={sourceUrl ? `Open ${sourceIcon.label} source` : `Reveal ${sourceIcon.label} source in Finder`}
                    onClick={() => openSource(sourceValue, sourceDetails.kind, sourceDetails.relativePath)}
                  >
                    {sourceIcon.icon}
                  </button></Tooltip>
                ) : (
                  <Tooltip content={sourceIcon.label}><span className="skillInfoSourceIcon" aria-label={`${sourceIcon.label} source`}>
                    {sourceIcon.icon}
                  </span></Tooltip>
                )}
                {sourceValue && (isWebSource(sourceValue.trim())
                  ? <code>{sourceValue}</code>
                  : <Tooltip content={sourceValue} onlyWhenTruncated><code>{sourceValue}</code></Tooltip>)}
                {sourceValue && (
                  <>
                    <button
                      aria-label={sourceUrl ? "Open source link" : "Reveal source in Finder"}
                      className="skillInfoIconButton"
                      onClick={() => openSource(sourceValue, sourceDetails.kind, sourceDetails.relativePath)}
                    >
                      {sourceUrl ? <ExternalLink size={13} /> : <FolderOpen size={13} />}
                    </button>
                    <CopyButton className="skillInfoIconButton" value={sourceValue} copyLabel="Copy source" copiedLabel="Source copied" />
                  </>
                )}
            </InfoSection>
            <InfoSection label="Visibility" valueLine={false}>
              <Visibility value={skill.visibility ?? SkillVisibility.Auto} skill={skill} readOnly />
            </InfoSection>
            <InfoSection label="Agents" valueLine={false}>
              <div className="skillInfoAgents"><AgentChips agents={skill.agents ?? []} /></div>
            </InfoSection>
            {relationRows.length > 0 && (
              <InfoSection label="Relationships" valueLine={false}>
                <SkillInfoRelations rows={relationRows} onOpenSkill={onOpenSkill} />
              </InfoSection>
            )}
            <InfoSection label="Install location" valueLine={false}>
              <div className="skillInfoPathList">
                {installLocations.map((target) => (
                  <div className="skillInfoPathRow" key={target.id}>
                    <span className="skillInfoInstallAgent">
                      <AgentBadge agent={target.agent} small />
                    </span>
                    <Tooltip content={target.path ?? ""} onlyWhenTruncated><code>{target.path ?? ""}</code></Tooltip>
                    <button
                      aria-label={`Reveal ${target.label} in Finder`}
                      className="skillInfoIconButton"
                      onClick={() => target.path && safeInvoke(TauriCommand.RevealInFinder, { path: target.path })}
                    >
                      <FolderOpen size={13} />
                    </button>
                    <CopyButton
                      className="skillInfoIconButton"
                      value={target.path}
                      copyLabel={`Copy ${target.label} path`}
                      copiedLabel={`${target.label} path copied`}
                      disabled={!target.path}
                    />
                  </div>
                ))}
              </div>
            </InfoSection>
          </div>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
