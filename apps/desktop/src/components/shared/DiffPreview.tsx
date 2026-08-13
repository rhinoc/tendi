export type DiffLineSegment = {
  text: string;
  changed?: boolean;
};

export type DiffLine = {
  kind?: "added" | "removed" | "unchanged" | string;
  text?: string;
  segments?: DiffLineSegment[];
};

export type DiffPreviewProps = {
  lines: DiffLine[];
};

export function DiffPreview({ lines }: DiffPreviewProps) {
  return (
    <div className="diffPane tiptapDiffPreview">
      {lines.map((line, index) => (
        <div className={`diffLine ${line.kind || "unchanged"}`} key={`${line.kind}-${index}-${line.text}`}>
          <span className="diffMarker">{line.kind === "added" ? "+" : line.kind === "removed" ? "-" : " "}</span>
          <span>
            {line.segments
              ? line.segments.map((segment, segmentIndex) => (
                <span className={segment.changed ? "diffCharChanged" : ""} key={`${segmentIndex}-${segment.text}`}>
                  {segment.text}
                </span>
              ))
              : line.text || " "}
          </span>
        </div>
      ))}
    </div>
  );
}
