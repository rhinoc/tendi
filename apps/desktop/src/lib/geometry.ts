import {
  MARQUEE_AUTO_SCROLL_EDGE,
  MARQUEE_AUTO_SCROLL_MAX_SPEED,
} from "./constants.ts";

export type Point = { x: number; y: number };

export type Rect = {
  left: number;
  top: number;
  width: number;
  height: number;
  right?: number;
  bottom?: number;
};

export function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

export function edgeAutoScrollDelta(position: number, min: number, max: number): number {
  if (position < min + MARQUEE_AUTO_SCROLL_EDGE) {
    const distance = clamp(min + MARQUEE_AUTO_SCROLL_EDGE - position, 0, MARQUEE_AUTO_SCROLL_EDGE);
    return -Math.ceil((distance / MARQUEE_AUTO_SCROLL_EDGE) * MARQUEE_AUTO_SCROLL_MAX_SPEED);
  }
  if (position > max - MARQUEE_AUTO_SCROLL_EDGE) {
    const distance = clamp(position - (max - MARQUEE_AUTO_SCROLL_EDGE), 0, MARQUEE_AUTO_SCROLL_EDGE);
    return Math.ceil((distance / MARQUEE_AUTO_SCROLL_EDGE) * MARQUEE_AUTO_SCROLL_MAX_SPEED);
  }
  return 0;
}

export function marqueeAutoScrollDelta(point: Point, bounds: Rect): Point {
  const right = bounds.right ?? bounds.left + bounds.width;
  const bottom = bounds.bottom ?? bounds.top + bounds.height;
  return {
    x: edgeAutoScrollDelta(point.x, bounds.left, right),
    y: edgeAutoScrollDelta(point.y, bounds.top, bottom),
  };
}

export function clientRectFromPoints(start: Point, end: Point, bounds: Rect): Rect {
  const right = bounds.right ?? bounds.left + bounds.width;
  const bottom = bounds.bottom ?? bounds.top + bounds.height;
  const startX = clamp(start.x, bounds.left, right);
  const startY = clamp(start.y, bounds.top, bottom);
  const endX = clamp(end.x, bounds.left, right);
  const endY = clamp(end.y, bounds.top, bottom);
  const left = Math.min(startX, endX);
  const top = Math.min(startY, endY);
  const width = Math.abs(endX - startX);
  const height = Math.abs(endY - startY);
  return {
    left,
    top,
    right: left + width,
    bottom: top + height,
    width,
    height,
  };
}

export function rectFromPoints(start: Point, end: Point): Rect {
  const left = Math.min(start.x, end.x);
  const top = Math.min(start.y, end.y);
  const width = Math.abs(end.x - start.x);
  const height = Math.abs(end.y - start.y);
  return {
    left,
    top,
    right: left + width,
    bottom: top + height,
    width,
    height,
  };
}

export function contentPointFromClient(
  point: Point,
  scroller: HTMLElement,
  bounds: DOMRect = scroller.getBoundingClientRect(),
): Point {
  return {
    x: point.x - bounds.left + scroller.scrollLeft,
    y: point.y - bounds.top + scroller.scrollTop,
  };
}

export function clientPointFromContent(
  point: Point,
  scroller: HTMLElement,
  bounds: DOMRect = scroller.getBoundingClientRect(),
): Point {
  return {
    x: bounds.left + point.x - scroller.scrollLeft,
    y: bounds.top + point.y - scroller.scrollTop,
  };
}

export function elementContentRect(
  element: Element,
  scroller: HTMLElement,
  bounds: DOMRect = scroller.getBoundingClientRect(),
): Rect {
  const rect = element.getBoundingClientRect();
  const left = rect.left - bounds.left + scroller.scrollLeft;
  const top = rect.top - bounds.top + scroller.scrollTop;
  return {
    left,
    top,
    right: left + rect.width,
    bottom: top + rect.height,
    width: rect.width,
    height: rect.height,
  };
}

export function rectsIntersect(a: Rect, b: Rect): boolean {
  const aRight = a.right ?? a.left + a.width;
  const aBottom = a.bottom ?? a.top + a.height;
  const bRight = b.right ?? b.left + b.width;
  const bBottom = b.bottom ?? b.top + b.height;
  return a.left < bRight && aRight > b.left && a.top < bBottom && aBottom > b.top;
}
