declare const rawDomainRowBrand: unique symbol;

/** Rows that have crossed the runtime boundary but have not entered a domain projection. */
export type RawDomainRow = Record<string, unknown> & {
  readonly [rawDomainRowBrand]: true;
};

export type RawDomainRows = readonly RawDomainRow[];

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === "object" && !Array.isArray(value));
}

/** Validate and brand rows at the runtime gateway boundary. */
export function toRawDomainRows(value: unknown, label: string): RawDomainRow[] {
  if (!Array.isArray(value) || value.some((row) => !isRecord(row))) {
    throw new Error(`Invalid ${label} response`);
  }
  return value as RawDomainRow[];
}

export function toRawDomainRow(value: unknown, label: string): RawDomainRow {
  if (!isRecord(value)) throw new Error(`Invalid ${label} response`);
  return value as RawDomainRow;
}
