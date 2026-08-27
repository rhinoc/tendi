import { Tooltip } from "../../components/shared/Tooltip.tsx";
import { ExternalLink, FolderOpen, Info } from "lucide-react";

import {
  copiedPathLabel,
  copiedValueLabel,
  copyPathLabel,
  copyValueLabel,
  formatUserPath,
  openSource,
  safeInvoke,
  skillSourceDetails,
  skillTargets,
  isWebSource,
  sourceIconDetails,
  sourceOpenUrl,
  TauriCommand,
  type SkillLike,
} from "../../lib/index.ts";
import { AgentBadge } from "../../components/shared/AgentBadge.tsx";
import { AgentChips } from "../../components/shared/AgentChips.tsx";
import { CopyButton } from "../../components/shared/CopyButton.tsx";
import { InfoDropdownMenu } from "../../components/shared/InfoDropdownMenu.tsx";
import { InfoSection } from "../../components/shared/InfoSection.tsx";
import { IconButton } from "../../components/shared/IconButton.tsx";
import type { SkillDependencyRecord } from "./SkillDependencyGraph.tsx";
import { Visibility } from "./Visibility.tsx";

export type SkillInfoMenuSkill = SkillLike & {
  name: string;
  visibility: string;
  agents: string[];
  dependencies: string[];
  dependents: string[];
  description: string;
};

export type SkillInfoMenuProps = {
  skill: SkillInfoMenuSkill;
  skills: SkillDependencyRecord[];
  onOpenSkill?: (name: string) => void;
};

function relationList(names: string[], skills: SkillDependencyRecord[]) {
  const skillsByName = new Map(skills.map((item) => [item.name, item]));
  return names
    .flatMap((name) => {
      const skill = skillsByName.get(name);
      return skill ? [skill] : [];
    })
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
                  <Tooltip key={relatedSkill.name} content={relatedSkill.description}><button
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

export function SkillInfoMenu({ skill, skills, onOpenSkill }: SkillInfoMenuProps) {
  const sourceDetails = skillSourceDetails(skill);
  const sourceIcon = sourceIconDetails(sourceDetails);
  const sourceValue = sourceDetails.value;
  const displaySourceValue = isWebSource(sourceValue.trim()) ? sourceValue : formatUserPath(sourceValue);
  const sourceUrl = sourceOpenUrl(sourceValue, sourceDetails.kind, sourceDetails.relativePath);
  const installLocations = skillTargets(skill);
  const dependencies = relationList(skill.dependencies, skills);
  const dependents = relationList(skill.dependents, skills);
  const relationRows = [
    { label: "Depends on", skills: dependencies },
    { label: "Used by", skills: dependents },
  ].filter((row) => row.skills.length > 0);
  return (
    <InfoDropdownMenu
      trigger={(
        <IconButton aria-label="Show skill info">
          <Info size={15} />
        </IconButton>
      )}
      label="Skill info"
      title={skill.name}
    >
            <InfoSection label="Source" className="skillInfoSourceSection">
                {sourceValue ? (
                  <button
                    className="skillInfoSourceIcon"
                    aria-label={sourceUrl ? `Open ${sourceIcon.label} source` : `Reveal ${sourceIcon.label} source in Finder`}
                    onClick={() => openSource(sourceValue, sourceDetails.kind, sourceDetails.relativePath)}
                  >
                    {sourceIcon.icon}
                  </button>
                ) : (
                  <Tooltip content={sourceIcon.label}><span className="skillInfoSourceIcon" aria-label={`${sourceIcon.label} source`}>
                    {sourceIcon.icon}
                  </span></Tooltip>
                )}
                {sourceValue && (isWebSource(sourceValue.trim())
                  ? <code>{displaySourceValue}</code>
                  : <Tooltip content={displaySourceValue} onlyWhenTruncated><code>{displaySourceValue}</code></Tooltip>)}
                {sourceValue && (
                  <>
                    <button
                      aria-label={sourceUrl ? "Open source link" : "Reveal source in Finder"}
                      className="appButton appButton-icon"
                      onClick={() => openSource(sourceValue, sourceDetails.kind, sourceDetails.relativePath)}
                    >
                      {sourceUrl ? <ExternalLink size={13} /> : <FolderOpen size={13} />}
                    </button>
                    <CopyButton className="appButton appButton-icon" value={sourceValue} copyLabel={copyValueLabel("source")} copiedLabel={copiedValueLabel("source")} />
                  </>
                )}
            </InfoSection>
            <InfoSection label="Visibility" valueLine={false}>
              <Visibility value={skill.visibility} skill={skill} readOnly />
            </InfoSection>
            <InfoSection label="Agents" valueLine={false}>
              <div className="skillInfoAgents"><AgentChips agents={skill.agents} /></div>
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
                    <Tooltip content={formatUserPath(target.path)} onlyWhenTruncated><code>{formatUserPath(target.path)}</code></Tooltip>
                    <button
                      aria-label={`Reveal ${target.label} in Finder`}
                      className="appButton appButton-icon"
                      onClick={() => target.path && safeInvoke(TauriCommand.RevealInFinder, { path: target.path })}
                    >
                      <FolderOpen size={13} />
                    </button>
                    <CopyButton
                      className="appButton appButton-icon"
                      value={target.path}
                      copyLabel={copyPathLabel(target.label)}
                      copiedLabel={copiedPathLabel(target.label)}
                      disabled={!target.path}
                    />
                  </div>
                ))}
              </div>
            </InfoSection>
    </InfoDropdownMenu>
  );
}
