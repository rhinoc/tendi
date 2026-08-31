import type { RuntimeData } from "../lib/data.ts";
import type { DomainKey, RuntimeDomainKey } from "../lib/domain.ts";
import type { RawDomainRow } from "../lib/raw-domain.ts";

export type { RawDomainRow } from "../lib/raw-domain.ts";

export type RawDomainRows = readonly RawDomainRow[];

export type DomainRows = {
  [K in RuntimeDomainKey]: RuntimeData[K];
};

export type CatalogSource = {
  label: string;
  count: number;
};

export type CatalogIndexes = {
  counts: Record<DomainKey, number>;
  sources: CatalogSource[];
  installedAgentKeys: string[];
  loadedDomains: ReadonlySet<DomainKey>;
  errorDomains: ReadonlySet<DomainKey>;
};
