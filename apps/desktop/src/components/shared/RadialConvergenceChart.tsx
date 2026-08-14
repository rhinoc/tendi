// Lieflat L5 · Radial Convergence · templates/lupi-gallery.html · "48 requests pull toward five themes"
// Reused for session relationships: each item converges on the current session set.
export type RadialConvergenceNode = {
  key: string;
  label: string;
};

export function RadialConvergenceChart({
  nodes,
  centerLabel,
  ariaLabel,
}: {
  nodes: RadialConvergenceNode[];
  centerLabel: string;
  ariaLabel: string;
}) {
  if (nodes.length === 0) return null;

  const width = 360;
  const height = 196;
  const centerX = width / 2;
  const centerY = height / 2;
  const radius = 74;
  const showLabels = nodes.length <= 12;
  const positionedNodes = nodes.map((node, index) => {
    const angle = -Math.PI / 2 + index / nodes.length * Math.PI * 2;
    const x = centerX + Math.cos(angle) * radius;
    const y = centerY + Math.sin(angle) * radius;
    return { node, x, y, angle };
  });

  return (
    <div
      className="sessionRelationConvergence"
      role="img"
      aria-label={ariaLabel}
    >
      <svg viewBox={`0 0 ${width} ${height}`} aria-hidden="true">
        {positionedNodes.map(({ node, x, y }) => {
          const controlOneX = centerX + (x - centerX) * 0.62;
          const controlOneY = centerY + (y - centerY) * 0.62;
          const controlTwoX = centerX + (x - centerX) * 0.24;
          const controlTwoY = centerY + (y - centerY) * 0.24;
          return (
            <path
              key={`${node.key}-path`}
              className="sessionRelationConvergencePath"
              d={`M ${x} ${y} C ${controlOneX} ${controlOneY}, ${controlTwoX} ${controlTwoY}, ${centerX} ${centerY}`}
            />
          );
        })}
        {positionedNodes.map(({ node, x, y, angle }) => {
          const name = node.label || "Unnamed item";
          const labelX = centerX + Math.cos(angle) * (radius + 13);
          const labelY = centerY + Math.sin(angle) * (radius + 13);
          const textAnchor = Math.cos(angle) > 0.35 ? "start" : Math.cos(angle) < -0.35 ? "end" : "middle";
          return (
            <g key={`${node.key}-node`}>
              <circle className="sessionRelationConvergenceNode" cx={x} cy={y} r="3" />
              {showLabels ? (
                <text
                  className="sessionRelationConvergenceLabel"
                  x={labelX}
                  y={labelY}
                  textAnchor={textAnchor}
                  dominantBaseline="middle"
                >
                  {name.length > 18 ? `${name.slice(0, 17)}…` : name}
                </text>
              ) : null}
            </g>
          );
        })}
        <circle className="sessionRelationConvergenceCore" cx={centerX} cy={centerY} r="23" />
        <text className="sessionRelationConvergenceCoreLabel" x={centerX} y={centerY - 3} textAnchor="middle">{centerLabel}</text>
        <text className="sessionRelationConvergenceCoreCount" x={centerX} y={centerY + 12} textAnchor="middle">{nodes.length}</text>
      </svg>
    </div>
  );
}
