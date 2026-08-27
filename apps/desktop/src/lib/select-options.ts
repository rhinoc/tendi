export type SelectOptionValue = {
  value: string;
};

export function resolveSelectValue<T extends SelectOptionValue>(value: string, options: T[]) {
  if (!value || options.some((option) => option.value === value)) return value;
  return options[0]?.value ?? value;
}
