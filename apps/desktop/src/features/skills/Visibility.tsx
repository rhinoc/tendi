import { SkillVisibility, editableSkillVisibilities, isSkillVisibilityEditable } from "../../lib/index.ts";
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
  // Use aria-disabled (not native disabled) so clicks still hit these controls and do not fall through to row onClick.
  const disabled = readOnly || !isSkillVisibilityEditable(skill);
  const options = value === SkillVisibility.Mixed ? [SkillVisibility.Mixed] : editableSkillVisibilities;
  return (
    <div className="visibility" aria-label="Visibility" data-no-row-click>
      {options.map((option) => (
        <button
          className={value === option ? "selected" : ""}
          aria-disabled={disabled || undefined}
          key={option}
          onClick={(event) => {
            event.stopPropagation();
            if (disabled) return;
            onSetVisibility?.([skill.name], option);
          }}
          onKeyDown={(event) => {
            if (!disabled) return;
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              event.stopPropagation();
            }
          }}
        >
          {option}
        </button>
      ))}
    </div>
  );
}
