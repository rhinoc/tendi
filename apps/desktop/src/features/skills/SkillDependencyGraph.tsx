export type SkillDependencyRecord = {
  name: string;
  description: string;
  dependencies: string[];
  dependents: string[];
};

export type SkillDependencyGraphProps = {
  skill?: SkillDependencyRecord | null;
  skills: SkillDependencyRecord[];
  title?: string;
  emptyLabel?: string;
  onOpenSkill?: (name: string) => void;
};

function relationList(names: string[], skillsByName: Map<string, SkillDependencyRecord>) {
  return names
    .flatMap((name) => {
      const skill = skillsByName.get(name);
      return skill ? [skill] : [];
    })
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
          {skills.map((skill) => (
            <button
              className="skillGraphNode"
              key={skill.name}
              disabled={!onOpenSkill}
              onClick={() => onOpenSkill?.(skill.name)}
            >
              <strong>{skill.name}</strong>
              {skill.description && <span>{skill.description}</span>}
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
  const skillsByName = new Map(skills.map((item) => [item.name, item]));
  if (!skill) {
    return (
      <section className="skillGraph">
        <div className="skillGraphTitle">{title}</div>
        <div className="skillGraphEmpty">{emptyLabel}</div>
      </section>
    );
  }

  const dependencies = relationList(skill.dependencies, skillsByName);
  const dependents = relationList(skill.dependents, skillsByName);
  const emptySide = dependencies.length === 0
    ? dependents.length === 0 ? "both" : "dependencies"
    : dependents.length === 0 ? "dependents" : "none";

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
