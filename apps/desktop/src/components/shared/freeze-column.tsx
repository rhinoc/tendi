import { Tooltip } from "./Tooltip.tsx";
import { useCallback, useRef, useState, type CSSProperties } from "react";

import { clamp } from "../../lib/index.ts";

export type FreezeColumnResizeOptions = {
  defaultWidth: number;
  min: number;
  max: number;
};

export type FreezeColumnResizeHandleProps = Record<string, unknown> & {
  "aria-valuemax": number;
  "aria-valuemin": number;
  "aria-valuenow": number;
  "data-resize-handle-active"?: string;
  onPointerDown: (event: React.PointerEvent<HTMLButtonElement>) => void;
  onPointerMove: (event: React.PointerEvent<HTMLButtonElement>) => void;
  onPointerUp: (event: React.PointerEvent<HTMLButtonElement>) => void;
  onPointerCancel: (event: React.PointerEvent<HTMLButtonElement>) => void;
  onKeyDown: (event: React.KeyboardEvent<HTMLButtonElement>) => void;
};

export type FreezeColumnResizeState = {
  width: number;
  style: CSSProperties & { "--data-freeze-column-width": string };
  handleProps: FreezeColumnResizeHandleProps;
};

export function useFreezeColumnResize({ defaultWidth, min, max }: FreezeColumnResizeOptions): FreezeColumnResizeState {
  const [width, setWidth] = useState(defaultWidth);
  const [active, setActive] = useState(false);
  const resizeRef = useRef<{ pointerId: number; startX: number; startWidth: number } | null>(null);

  const beginResize = useCallback((event: React.PointerEvent<HTMLButtonElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    event.currentTarget.setPointerCapture?.(event.pointerId);
    resizeRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startWidth: width,
    };
    setActive(true);
  }, [width]);

  const updateResize = useCallback((event: React.PointerEvent<HTMLButtonElement>) => {
    const resize = resizeRef.current;
    if (!resize || resize.pointerId !== event.pointerId) return;
    event.preventDefault();
    const nextWidth = Math.round(resize.startWidth + event.clientX - resize.startX);
    setWidth(clamp(nextWidth, min, max));
  }, [max, min]);

  const finishResize = useCallback((event: React.PointerEvent<HTMLButtonElement>) => {
    const resize = resizeRef.current;
    if (!resize || resize.pointerId !== event.pointerId) return;
    event.currentTarget.releasePointerCapture?.(event.pointerId);
    resizeRef.current = null;
    setActive(false);
  }, []);

  const resizeWithKeyboard = useCallback((event: React.KeyboardEvent<HTMLButtonElement>) => {
    const step = event.shiftKey ? 40 : 10;
    if (event.key === "ArrowLeft") {
      event.preventDefault();
      setWidth((current) => clamp(current - step, min, max));
    } else if (event.key === "ArrowRight") {
      event.preventDefault();
      setWidth((current) => clamp(current + step, min, max));
    } else if (event.key === "Home") {
      event.preventDefault();
      setWidth(min);
    } else if (event.key === "End") {
      event.preventDefault();
      setWidth(max);
    }
  }, [max, min]);

  return {
    width,
    style: { "--data-freeze-column-width": `${width}px` },
    handleProps: {
      "aria-valuemax": max,
      "aria-valuemin": min,
      "aria-valuenow": width,
      "data-resize-handle-active": active ? "" : undefined,
      onPointerDown: beginResize,
      onPointerMove: updateResize,
      onPointerUp: finishResize,
      onPointerCancel: finishResize,
      onKeyDown: resizeWithKeyboard,
    },
  };
}

export type FreezeColumnResizeHandleComponentProps = {
  label: string;
  resize: FreezeColumnResizeState;
};

export function FreezeColumnResizeHandle({ label, resize }: FreezeColumnResizeHandleComponentProps) {
  return (
    <Tooltip content={label}><button
      type="button"
      className="sessionFreezeResizeHandle"
      aria-label={label}
      role="separator"
      aria-orientation="vertical"
      data-no-drag
      {...resize.handleProps}
    /></Tooltip>
  );
}
