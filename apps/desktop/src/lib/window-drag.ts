import { getCurrentWindow } from "@tauri-apps/api/window";
import { isTauriRuntime } from "./tauri.ts";

export const WINDOW_DRAG_REGION_SELECTOR = "[data-window-drag], [data-tauri-drag-region]";

export const WINDOW_DRAG_BLOCK_SELECTOR = [
  "button",
  "input",
  "textarea",
  "select",
  "a",
  "summary",
  "[contenteditable]",
  "[data-no-drag]",
  "[data-no-window-drag]",
  "[data-selectable-text]",
  "[role='button']",
  "[role='tooltip']",
  "[role='checkbox']",
  "[role='combobox']",
  "[role='link']",
  "[role='menuitem']",
  "[role='menuitemcheckbox']",
  "[role='menuitemradio']",
  "[role='option']",
  "[role='radio']",
  "[role='searchbox']",
  "[role='slider']",
  "[role='switch']",
  "[role='tab']",
  "[role='textbox']",
  ".editorActions",
  ".headerTools",
].join(", ");

type DragEventLike = {
  button: number;
  defaultPrevented: boolean;
  target: EventTarget | null;
};

export function shouldStartWindowDrag(event: DragEventLike): boolean {
  if (event.button !== 0 || event.defaultPrevented) return false;
  const target = event.target;
  if (!(target instanceof Element) || !target.closest) return false;
  if (!target.closest(WINDOW_DRAG_REGION_SELECTOR)) return false;
  if (target.closest(WINDOW_DRAG_BLOCK_SELECTOR)) return false;
  return true;
}

export function startWindowDrag(event: DragEventLike): void {
  if (!shouldStartWindowDrag(event)) return;
  if (!isTauriRuntime()) return;
  getCurrentWindow().startDragging().catch((error) => {
    console.warn("window drag failed", error);
  });
}
