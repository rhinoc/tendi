import { useId } from "react";

export type DialogTextFieldProps = {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  autoFocus?: boolean;
  required?: boolean;
  pattern?: string;
  maxLength?: number;
  error?: string;
  "aria-describedby"?: string;
};

export function DialogTextField({ label, value, onChange, placeholder, error, "aria-describedby": describedBy, ...inputProps }: DialogTextFieldProps) {
  const errorId = useId();
  return (
    <label className="dialogField">
      <span className="dialogTextFieldHeader">
        <span>{label}</span>
        <span id={errorId} className="dialogTextFieldError" aria-live="polite">{error}</span>
      </span>
      <input
        className="dialogTextInput"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        placeholder={placeholder}
        {...inputProps}
        aria-label={label}
        aria-invalid={error ? true : undefined}
        aria-describedby={[describedBy, error ? errorId : undefined].filter(Boolean).join(" ") || undefined}
      />
    </label>
  );
}
