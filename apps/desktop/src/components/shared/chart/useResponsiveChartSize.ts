import { useElementSize, type ElementSize } from "../useElementSize.ts";

export type ResponsiveChartSize = ElementSize;

export function useResponsiveChartSize(initialSize: ResponsiveChartSize) {
  const { ref, size } = useElementSize<HTMLDivElement>(initialSize, {
    isValidSize: ({ width, height }) => width > 0 && height > 0,
  });
  return { containerRef: ref, size };
}
