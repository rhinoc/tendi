import { startWindowDrag } from "../../lib/index.ts";
import "./ContentTopDragStrip.css";

export function ContentTopDragStrip() {
  return (
    <div
      className="contentTopDragStrip"
      data-window-drag
      onMouseDown={(event) => startWindowDrag(event.nativeEvent)}
      aria-hidden="true"
    />
  );
}
