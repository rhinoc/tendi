import { Sparkles } from "lucide-react";

import { ContentTopDragStrip } from "./ContentTopDragStrip.tsx";
import "./PlaceholderView.css";

export type PlaceholderViewProps = {
  title: string;
};

export function PlaceholderView({ title }: PlaceholderViewProps) {
  return (
    <section className="content placeholder">
      <ContentTopDragStrip />
      <Sparkles size={22} />
      <h1>{title}</h1>
    </section>
  );
}
