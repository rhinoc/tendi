import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import { useElementSize, type ElementRef, type ElementSize } from "./useElementSize.ts";

export type VirtualViewportAxis = "horizontal" | "vertical";

export type UseVirtualViewportOptions<T extends HTMLElement> = {
  ref: ElementRef<T>;
  axis?: VirtualViewportAxis;
  enabled?: boolean;
  refreshKey?: unknown;
  readSize?: (element: T) => ElementSize;
  isValidSize?: (size: ElementSize) => boolean;
  isEqual?: (current: ElementSize, next: ElementSize) => boolean;
};

function finiteOffset(value: number) {
  return Number.isFinite(value) && value > 0 ? value : 0;
}

export function useVirtualViewport<T extends HTMLElement>(
  initialSize: ElementSize,
  options: UseVirtualViewportOptions<T>,
) {
  const axis = options.axis ?? "vertical";
  const { ref, size } = useElementSize(initialSize, {
    ref: options.ref,
    enabled: options.enabled,
    refreshKey: options.refreshKey,
    readSize: options.readSize,
    isValidSize: options.isValidSize,
    isEqual: options.isEqual,
  });
  const [scrollOffset, setScrollOffset] = useState(0);
  const scrollOffsetRef = useRef(0);
  const frameRef = useRef<number | null>(null);

  const readScrollOffset = useCallback(() => {
    const element = ref.current;
    if (!element) return 0;
    return finiteOffset(axis === "horizontal" ? element.scrollLeft : element.scrollTop);
  }, [axis, ref]);

  const readViewportSize = useCallback(() => {
    const element = ref.current;
    const fallback = axis === "horizontal" ? size.width : size.height;
    if (!element) return fallback;
    const next = axis === "horizontal" ? element.clientWidth : element.clientHeight;
    return next > 0 ? next : fallback;
  }, [axis, ref, size.height, size.width]);

  const syncScrollPosition = useCallback(() => {
    const next = readScrollOffset();
    scrollOffsetRef.current = next;
    setScrollOffset((current) => current === next ? current : next);
    return next;
  }, [readScrollOffset]);

  const scheduleScrollSync = useCallback(() => {
    if (options.enabled === false || frameRef.current !== null) return;
    frameRef.current = window.requestAnimationFrame(() => {
      frameRef.current = null;
      syncScrollPosition();
    });
  }, [options.enabled, syncScrollPosition]);

  useLayoutEffect(() => {
    syncScrollPosition();
  }, [options.refreshKey, readViewportSize, syncScrollPosition]);

  useEffect(() => () => {
    if (frameRef.current !== null) window.cancelAnimationFrame(frameRef.current);
  }, []);

  return {
    ref,
    size,
    scrollOffset,
    scrollOffsetRef,
    readViewportSize,
    syncScrollPosition,
    scheduleScrollSync,
  };
}
