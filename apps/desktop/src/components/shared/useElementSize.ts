import { useEffect, useRef, useState } from "react";

export type ElementSize = {
  width: number;
  height: number;
};

export type ElementRef<T extends Element> = {
  current: T | null;
};

export type UseElementSizeOptions<T extends Element> = {
  ref?: ElementRef<T>;
  enabled?: boolean;
  refreshKey?: unknown;
  readSize?: (element: T) => ElementSize;
  isValidSize?: (size: ElementSize) => boolean;
  isEqual?: (current: ElementSize, next: ElementSize) => boolean;
};

function readBoundingClientRect(element: Element): ElementSize {
  const { width, height } = element.getBoundingClientRect();
  return { width: Math.round(width), height: Math.round(height) };
}

export function useElementSize<T extends Element = HTMLElement>(
  initialSize: ElementSize,
  options: UseElementSizeOptions<T> = {},
) {
  const internalRef = useRef<T | null>(null);
  const elementRef = options.ref ?? internalRef;
  const readSizeRef = useRef(options.readSize ?? readBoundingClientRect);
  const isValidSizeRef = useRef(options.isValidSize ?? (() => true));
  const isEqualRef = useRef(options.isEqual ?? ((current, next) => current.width === next.width && current.height === next.height));
  const [size, setSize] = useState(initialSize);

  readSizeRef.current = options.readSize ?? readBoundingClientRect;
  isValidSizeRef.current = options.isValidSize ?? (() => true);
  isEqualRef.current = options.isEqual ?? ((current, next) => current.width === next.width && current.height === next.height);

  useEffect(() => {
    if (options.enabled === false) return undefined;
    const element = elementRef.current;
    if (!element) return undefined;

    const updateSize = () => {
      const next = readSizeRef.current(element);
      if (!isValidSizeRef.current(next)) return;
      setSize((current) => isEqualRef.current(current, next) ? current : next);
    };

    updateSize();
    if (typeof ResizeObserver !== "function") return undefined;
    const observer = new ResizeObserver(updateSize);
    observer.observe(element);
    return () => observer.disconnect();
  }, [elementRef, options.enabled, options.refreshKey]);

  return { ref: elementRef, size };
}
