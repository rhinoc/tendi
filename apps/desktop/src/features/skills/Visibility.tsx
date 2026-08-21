import { SkillVisibility, editableSkillVisibilities, isSkillVisibilityEditable } from "../../lib/index.ts";
import { SelectControl } from "../../components/shared/SelectControl.tsx";
import "./Visibility.css";

export type VisibilitySkill = {
  name: string;
  visibility?: SkillVisibility | string;
};

export type VisibilityProps = {
  value: SkillVisibility | string;
  skill: VisibilitySkill;
  onSetVisibility?: (names: string[], option: SkillVisibility) => void;
  readOnly?: boolean;
};

export function Visibility({ value, skill, onSetVisibility, readOnly = false }: VisibilityProps) {
  const disabled = readOnly || !isSkillVisibilityEditable(skill) || value === SkillVisibility.Mixed;
  const options = value === SkillVisibility.Mixed
    ? [{ value: SkillVisibility.Mixed, label: SkillVisibility.Mixed }]
    : editableSkillVisibilities.map((option) => ({ value: option, label: option }));

  return <SelectControl
    value={value}
    onValueChange={(nextValue) => {
      if (!disabled) onSetVisibility?.([skill.name], nextValue as SkillVisibility);
    }}
    label="Visibility"
    options={options}
    className="visibility"
    disabled={disabled}
  />;
}
