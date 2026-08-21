export type OrderedSidebarSource = {
  label: string;
  count: number;
  installed: boolean;
  order: number;
};

export type SidebarSource = Pick<OrderedSidebarSource, "label" | "count">;

export function sortSidebarSources(sources: readonly OrderedSidebarSource[]): SidebarSource[] {
  return [...sources]
    .sort((left, right) => {
      if (left.installed !== right.installed) return left.installed ? -1 : 1;
      return left.order - right.order || left.label.localeCompare(right.label);
    })
    .map(({ label, count }) => ({ label, count }));
}
