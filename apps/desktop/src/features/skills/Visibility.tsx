import { SkillVisibility, editableSkillVisibilities, isSkillVisibilityEditable } from "../../lib/index.ts";
import {
  SegmentedControl,
  SegmentedControlItem,
} from "../../components/shared/SegmentedControl.tsx";
import "./Visibility.css";

export type VisibilitySkill = {
  id?: string;
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
  const selector = skill.id?.trim();

  return (
    <div
      className="visibility"
      data-no-row-click
      onClick={(event) => event.stopPropagation()}
    >
      <SegmentedControl
        value={value}
        onValueChange={(nextValue) => {
          if (!disabled && selector && nextValue) {
            onSetVisibility?.([selector], nextValue as SkillVisibility);
          }
        }}
        className="visibility"
        disabled={disabled}
        aria-label="Visibility"
      >
        {options.map((option) => (
          <SegmentedControlItem key={option.value} value={option.value}>
            {option.label}
          </SegmentedControlItem>
        ))}
      </SegmentedControl>
    </div>
  );
}
