export type SkillDependencyRecord = {
  id?: string;
  name: string;
  description: string;
  dependencies: string[];
  dependents: string[];
  dependencyIds?: string[];
  dependentIds?: string[];
  paths?: Array<{ scope?: string | null }>;
};

enum SkillGraphEmptySide {
  Both = "both",
  Dependencies = "dependencies",
  Dependents = "dependents",
  None = "none",
}

export type SkillDependencyGraphProps = {
  skill?: SkillDependencyRecord | null;
  skills: SkillDependencyRecord[];
  title?: string;
  emptyLabel?: string;
  onOpenSkill?: (name: string) => void;
};

function relationList(
  ids: string[],
  skills: SkillDependencyRecord[],
) {
  return ids
    .flatMap((id) => skills.filter((item) => item.id === id))
    .sort((left, right) => left.name.localeCompare(right.name));
}

function SkillRelationSection({
  title,
  skills,
  emptyLabel,
  onOpenSkill,
}: {
  title: string;
  skills: SkillDependencyRecord[];
  emptyLabel: string;
  onOpenSkill?: (name: string) => void;
}) {
  return (
    <div className="skillGraphSection">
      <div className="skillGraphSectionTitle">{title}</div>
      {skills.length > 0 ? (
        <div className="skillGraphList">
          {skills.map((item) => (
            <button
              className="skillGraphNode"
              key={item.id ?? item.name}
              disabled={!onOpenSkill}
              onClick={() => onOpenSkill?.(item.id ?? item.name)}
            >
              <strong>{item.name}</strong>
              {item.description && <span>{item.description}</span>}
            </button>
          ))}
        </div>
      ) : (
        <div className="skillGraphEmpty">{emptyLabel}</div>
      )}
    </div>
  );
}

export function SkillDependencyGraph({
  skill,
  skills,
  title = "Skill dependencies",
  emptyLabel = "No selected skill",
  onOpenSkill,
}: SkillDependencyGraphProps) {
  if (!skill) {
    return (
      <section className="skillGraph">
        <div className="skillGraphTitle">{title}</div>
        <div className="skillGraphEmpty">{emptyLabel}</div>
      </section>
    );
  }

  const dependencies = relationList(skill.dependencyIds ?? [], skills);
  const dependents = relationList(skill.dependentIds ?? [], skills);
  const emptySide = dependencies.length === 0
    ? dependents.length === 0 ? SkillGraphEmptySide.Both : SkillGraphEmptySide.Dependencies
    : dependents.length === 0 ? SkillGraphEmptySide.Dependents : SkillGraphEmptySide.None;

  return (
    <section className="skillGraph">
      <div className="skillGraphTitle">{title}</div>
      <div className="skillGraphRoot">
        <strong>{skill.name}</strong>
        {skill.description && <span>{skill.description}</span>}
      </div>
      <div className="skillGraphColumns" data-empty-side={emptySide}>
        <SkillRelationSection
          title="Depends on"
          skills={dependencies}
          emptyLabel="No dependencies"
          onOpenSkill={onOpenSkill}
        />
        <SkillRelationSection
          title="Used by"
          skills={dependents}
          emptyLabel="No dependents"
          onOpenSkill={onOpenSkill}
        />
      </div>
    </section>
  );
}
