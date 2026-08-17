import { useCallback, useRef, type WheelEvent } from "react";

export const TRACKPAD_ZOOM_SENSITIVITY = 0.006;
export const TRACKPAD_ZOOM_STEP = 1.25;

export type TrackpadZoomDetails = {
  deltaY: number;
  factor: number;
  clientX: number;
  clientY: number;
  rect: DOMRect;
};

export function trackpadZoomFactor(deltaY: number): number {
  return Math.exp(-deltaY * TRACKPAD_ZOOM_SENSITIVITY);
}

/** Returns -1 for zoom in, 1 for zoom out, and 0 while the gesture is below one step. */
export function trackpadZoomDirection(accumulatedFactor: number): -1 | 0 | 1 {
  if (accumulatedFactor >= TRACKPAD_ZOOM_STEP) return -1;
  if (accumulatedFactor <= 1 / TRACKPAD_ZOOM_STEP) return 1;
  return 0;
}

export function useTrackpadZoom<T extends Element>(
  onZoom: (details: TrackpadZoomDetails) => void,
): (event: WheelEvent<T>) => boolean {
  const onZoomRef = useRef(onZoom);
  onZoomRef.current = onZoom;

  return useCallback((event: WheelEvent<T>) => {
    if (!event.ctrlKey) return false;
    event.preventDefault();
    event.stopPropagation();
    onZoomRef.current({
      deltaY: event.deltaY,
      factor: trackpadZoomFactor(event.deltaY),
      clientX: event.clientX,
      clientY: event.clientY,
      rect: event.currentTarget.getBoundingClientRect(),
    });
    return true;
  }, []);
}
