export type DialogTextFieldProps = {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
};

export function DialogTextField({ label, value, onChange, placeholder }: DialogTextFieldProps) {
  return (
    <label className="dialogField">
      <span>{label}</span>
      <input
        className="dialogTextInput"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        placeholder={placeholder}
      />
    </label>
  );
}
