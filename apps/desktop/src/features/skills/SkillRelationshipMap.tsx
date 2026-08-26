import { useMemo, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent, WheelEvent } from "react";

import { formatSessionTitle } from "../../lib/session-preview.ts";
import { useTrackpadZoom } from "../../lib/zoom-gesture.ts";
import { LoadErrorState } from "../../components/shared/LoadErrorState.tsx";
import { LoadingState } from "../../components/shared/LoadingState.tsx";

const VIEWBOX_WIDTH = 1200;
const VIEWBOX_HEIGHT = 660;
const MAX_LABEL_LENGTH = 24;
const MIN_LABEL_OPACITY = 0.42;
const MAX_LABEL_OPACITY = 0.7;
const PAN_OVERSCROLL_RATIO = 0.5;
const GRAPH_ZOOM_MIN = 0.72;
const GRAPH_ZOOM_MAX = 2.5;
const TOUCH_PINCH_ZOOM_SPEED = 1.5;
const EMPTY_GRAPH_NODES: RelationshipGraphNode[] = [];
const GRAPH_LAYOUT = {
  centerForce: 0.001,
  degreeCenterForce: 0.0035,
  repelForce: 1400,
  collisionForce: 4.2,
  linkForce: 0.015,
  linkDistance: 150,
  iterations: 220,
  damping: 0.8,
} as const;

const RELATIONSHIP_KIND_LABELS = [
  { kind: "session-parent", label: "Parent session" },
  { kind: "session-child", label: "Child session" },
  { kind: "skill-used", label: "Skill used" },
  { kind: "wrapper", label: "Wrapper" },
  { kind: "plugin", label: "Plugin" },
  { kind: "remote", label: "Remote" },
  { kind: "system", label: "System" },
  { kind: "local", label: "Local" },
] as const;

export type RelationshipGraphNode = {
  name: string;
  label?: string;
  description?: string;
  kind?: string;
  dependencies?: string[];
  dependents?: string[];
};

export type RelationshipGraphEdge = {
  from: string;
  to: string;
  key?: string;
};

type RelationshipEdge = {
  from: string;
  to: string;
  key: string;
};

type LayoutNode = RelationshipGraphNode & {
  degree: number;
  radius: number;
  x: number;
  y: number;
};

type RelationshipGraph = {
  nodes: LayoutNode[];
  edges: RelationshipEdge[];
};

type PanInteraction = {
  pointerId: number;
  startClientX: number;
  startClientY: number;
  startPanX: number;
  startPanY: number;
  viewBoxWidth: number;
  viewBoxHeight: number;
  startedNodeName: string | null;
  moved: boolean;
};

type PointerPosition = {
  x: number;
  y: number;
};

type PinchInteraction = {
  pointerIds: [number, number];
  startDistance: number;
  startZoom: number;
  focalWorldX: number;
  focalWorldY: number;
};

function hashString(value: string) {
  let hash = 2166136261;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

function relationEdges(skills: RelationshipGraphNode[], explicitEdges?: RelationshipGraphEdge[]) {
  const names = new Set(skills.map((skill) => skill.name));
  const edges = new Map<string, RelationshipEdge>();
  const addEdge = (from: string, to: string, suppliedKey?: string) => {
    if (!names.has(from) || !names.has(to) || from === to) return;
    const key = `${from}\u0000${to}`;
    if (!edges.has(key)) edges.set(key, { from, to, key: suppliedKey ?? key });
  };

  if (explicitEdges) {
    for (const edge of explicitEdges) addEdge(edge.from, edge.to, edge.key);
    return [...edges.values()];
  }

  for (const skill of skills) {
    if (skill.dependencies) {
      for (const dependency of skill.dependencies) addEdge(dependency, skill.name);
    }
    if (skill.dependents) {
      for (const dependent of skill.dependents) addEdge(skill.name, dependent);
    }
  }

  return [...edges.values()];
}

function connectedComponents(skills: RelationshipGraphNode[], edges: RelationshipEdge[]) {
  const neighbors = new Map(skills.map((skill) => [skill.name, new Set<string>()]));
  for (const edge of edges) {
    neighbors.get(edge.from)?.add(edge.to);
    neighbors.get(edge.to)?.add(edge.from);
  }

  const byName = new Map(skills.map((skill) => [skill.name, skill]));
  const visited = new Set<string>();
  const components: RelationshipGraphNode[][] = [];
  for (const skill of skills) {
    if (visited.has(skill.name)) continue;
    const component: RelationshipGraphNode[] = [];
    const stack = [skill.name];
    while (stack.length > 0) {
      const name = stack.pop();
      if (!name || visited.has(name)) continue;
      visited.add(name);
      const item = byName.get(name);
      if (item) component.push(item);
      for (const neighbor of neighbors.get(name) ?? []) {
        if (!visited.has(neighbor)) stack.push(neighbor);
      }
    }
    components.push(component);
  }

  return components;
}

function clusterComponents(skills: RelationshipGraphNode[], edges: RelationshipEdge[]) {
  const components = connectedComponents(skills, edges).sort((left, right) => right.length - left.length);
  const connected = components.filter((component) => component.length > 1);
  const isolated = components.filter((component) => component.length === 1).flat();
  for (let index = 0; index < isolated.length; index += 12) connected.push(isolated.slice(index, index + 12));
  return connected.length > 0 ? connected : components;
}

function isSessionNodeKind(kind?: string) {
  return kind === "session" || kind?.startsWith("session-") === true;
}

function relationshipKindLabel(kind?: string) {
  return RELATIONSHIP_KIND_LABELS.find((item) => item.kind === kind)?.label;
}

export function relationshipNodeLabel(node: RelationshipGraphNode) {
  if (isSessionNodeKind(node.kind)) return formatSessionTitle(node.label ?? "");
  return node.label ?? "";
}

function shortName(name: string) {
  return name.length > MAX_LABEL_LENGTH ? `${name.slice(0, MAX_LABEL_LENGTH - 1)}…` : name;
}

function labelOpacityForNode(node: LayoutNode, compact: boolean) {
  const minRadius = compact ? 6 : 3.6;
  const maxRadius = compact ? 14 : 9;
  const sizeRatio = Math.min(1, Math.max(0, (node.radius - minRadius) / (maxRadius - minRadius)));
  return MIN_LABEL_OPACITY + (MAX_LABEL_OPACITY - MIN_LABEL_OPACITY) * sizeRatio;
}

function buildGraph(
  skills: RelationshipGraphNode[],
  explicitEdges?: RelationshipGraphEdge[],
  focusName?: string,
  compact = false,
): RelationshipGraph {
  const edges = relationEdges(skills, explicitEdges);
  const degree = new Map(skills.map((skill) => [skill.name, 0]));
  for (const edge of edges) {
    degree.set(edge.from, (degree.get(edge.from) ?? 0) + 1);
    degree.set(edge.to, (degree.get(edge.to) ?? 0) + 1);
  }

  const clusters = clusterComponents(skills, edges);
  const positions = new Map<string, { x: number; y: number }>();
  const centerX = VIEWBOX_WIDTH / 2;
  const centerY = VIEWBOX_HEIGHT / 2;
  const satelliteRadius = Math.min(390, 288 + clusters.length * 12);

  clusters.forEach((cluster, clusterIndex) => {
    const angle = -Math.PI / 2 + ((clusterIndex - 1) / Math.max(1, clusters.length - 1)) * Math.PI * 2;
    const clusterCenter = clusterIndex === 0 || clusters.length === 1
      ? { x: centerX, y: centerY }
      : {
        x: centerX + Math.cos(angle) * satelliteRadius,
        y: centerY + Math.sin(angle) * Math.min(218, satelliteRadius * 0.54),
      };
    const ordered = [...cluster].sort((left, right) => {
      if (left.name === focusName) return -1;
      if (right.name === focusName) return 1;
      const degreeDelta = (degree.get(right.name) ?? 0) - (degree.get(left.name) ?? 0);
      return degreeDelta || left.name.localeCompare(right.name);
    });
    const hub = ordered[0];
    if (hub) positions.set(hub.name, clusterCenter);
    if (ordered.length === 1) return;

    const ringRadius = clusterIndex === 0 || clusters.length === 1
      ? Math.min(compact ? 180 : 260, (compact ? 40 : 68) + Math.sqrt(ordered.length) * 34)
      : Math.min(88, 22 + Math.sqrt(ordered.length) * 14);
    const horizontalSpread = compact ? 1.45 : 1.2;
    ordered.slice(1).forEach((skill, index) => {
      const nodeAngle = -Math.PI / 2 + (index / (ordered.length - 1)) * Math.PI * 2;
      const jitter = 0.9 + (hashString(skill.name) % 15) / 100;
      positions.set(skill.name, {
        x: clusterCenter.x + Math.cos(nodeAngle) * ringRadius * horizontalSpread * jitter,
        y: clusterCenter.y + Math.sin(nodeAngle) * ringRadius * jitter,
      });
    });
  });

  const nodes = skills.map((skill) => {
    const nodeDegree = degree.get(skill.name) ?? 0;
    const position = positions.get(skill.name) ?? { x: centerX, y: centerY };
    return {
      ...skill,
      degree: nodeDegree,
      radius: skill.name === focusName
        ? compact
          ? Math.min(24, 13 + Math.sqrt(nodeDegree) * 2.2)
          : Math.min(16, 8 + Math.sqrt(nodeDegree) * 1.6)
        : compact
          ? Math.min(14, 6 + Math.sqrt(nodeDegree) * 1.8)
          : Math.min(9, 3.6 + Math.sqrt(nodeDegree) * 1.25),
      ...position,
    };
  });

  return { nodes: relaxedNodes(nodes, edges), edges };
}

export function buildRelationshipGraphForPerformance(
  skills: RelationshipGraphNode[],
  explicitEdges?: RelationshipGraphEdge[],
  focusName?: string,
  compact = false,
) {
  return buildGraph(skills, explicitEdges, focusName, compact);
}

function edgePath(from: LayoutNode, to: LayoutNode, key: string) {
  const deltaX = to.x - from.x;
  const deltaY = to.y - from.y;
  const length = Math.max(1, Math.sqrt(deltaX * deltaX + deltaY * deltaY));
  const curve = ((hashString(key) % 3) - 1) * Math.min(24, length * 0.08);
  const controlX = (from.x + to.x) / 2 - (deltaY / length) * curve;
  const controlY = (from.y + to.y) / 2 + (deltaX / length) * curve;
  return `M ${from.x.toFixed(1)} ${from.y.toFixed(1)} Q ${controlX.toFixed(1)} ${controlY.toFixed(1)} ${to.x.toFixed(1)} ${to.y.toFixed(1)}`;
}

function relaxedNodes(nodes: LayoutNode[], edges: RelationshipEdge[]) {
  const positioned = nodes.map((node) => ({ ...node, velocityX: 0, velocityY: 0 }));
  const indexByName = new Map(positioned.map((node, index) => [node.name, index]));
  const maxDegree = Math.max(1, ...positioned.map((node) => node.degree));
  const minX = 34;
  const maxX = VIEWBOX_WIDTH - 34;
  const minY = 28;
  const maxY = VIEWBOX_HEIGHT - 28;

  for (let iteration = 0; iteration < GRAPH_LAYOUT.iterations; iteration += 1) {
    const forces = positioned.map(() => ({ x: 0, y: 0 }));
    for (let leftIndex = 0; leftIndex < positioned.length; leftIndex += 1) {
      const left = positioned[leftIndex];
      for (let rightIndex = leftIndex + 1; rightIndex < positioned.length; rightIndex += 1) {
        const right = positioned[rightIndex];
        const deltaX = right.x - left.x;
        const deltaY = right.y - left.y;
        const distance = Math.max(1, Math.sqrt(deltaX * deltaX + deltaY * deltaY));
        const minimumDistance = 26 + left.radius + right.radius;
        const strength = distance < minimumDistance
          ? GRAPH_LAYOUT.collisionForce + (minimumDistance - distance) * 0.18
          : GRAPH_LAYOUT.repelForce / (distance * distance);
        const forceX = (deltaX / distance) * strength;
        const forceY = (deltaY / distance) * strength;
        forces[leftIndex].x -= forceX;
        forces[leftIndex].y -= forceY;
        forces[rightIndex].x += forceX;
        forces[rightIndex].y += forceY;
      }
    }

    for (const edge of edges) {
      const fromIndex = indexByName.get(edge.from);
      const toIndex = indexByName.get(edge.to);
      if (fromIndex === undefined || toIndex === undefined) continue;
      const from = positioned[fromIndex];
      const to = positioned[toIndex];
      const deltaX = to.x - from.x;
      const deltaY = to.y - from.y;
      const distance = Math.max(1, Math.sqrt(deltaX * deltaX + deltaY * deltaY));
      const strength = (distance - GRAPH_LAYOUT.linkDistance) * GRAPH_LAYOUT.linkForce;
      const forceX = (deltaX / distance) * strength;
      const forceY = (deltaY / distance) * strength;
      forces[fromIndex].x += forceX;
      forces[fromIndex].y += forceY;
      forces[toIndex].x -= forceX;
      forces[toIndex].y -= forceY;
    }

    positioned.forEach((node, index) => {
      const centralPull = GRAPH_LAYOUT.centerForce + (node.degree / maxDegree) * GRAPH_LAYOUT.degreeCenterForce;
      forces[index].x += (VIEWBOX_WIDTH / 2 - node.x) * centralPull;
      forces[index].y += (VIEWBOX_HEIGHT / 2 - node.y) * centralPull;
      node.velocityX = (node.velocityX + forces[index].x) * GRAPH_LAYOUT.damping;
      node.velocityY = (node.velocityY + forces[index].y) * GRAPH_LAYOUT.damping;
      node.x = Math.max(minX, Math.min(maxX, node.x + node.velocityX));
      node.y = Math.max(minY, Math.min(maxY, node.y + node.velocityY));
    });
  }

  return positioned.map(({ velocityX: _velocityX, velocityY: _velocityY, ...node }) => node);
}

function graphViewBox(nodes: LayoutNode[], compact: boolean) {
  if (!compact || nodes.length === 0) return `0 0 ${VIEWBOX_WIDTH} ${VIEWBOX_HEIGHT}`;

  const minX = Math.min(...nodes.map((node) => node.x - node.radius));
  const maxX = Math.max(...nodes.map((node) => node.x + node.radius));
  const minY = Math.min(...nodes.map((node) => node.y - node.radius));
  const maxY = Math.max(...nodes.map((node) => node.y + node.radius));
  const horizontalPadding = 42;
  const labelAllowance = 70;
  const verticalPadding = 42;
  const aspectRatio = VIEWBOX_WIDTH / VIEWBOX_HEIGHT;
  let width = Math.max(360, maxX - minX + horizontalPadding * 2 + labelAllowance * 2);
  let height = Math.max(180, maxY - minY + verticalPadding * 2);

  if (width / height < aspectRatio) width = height * aspectRatio;
  else height = width / aspectRatio;

  const centerX = (minX + maxX) / 2;
  const centerY = (minY + maxY) / 2;
  return [centerX - width / 2, centerY - height / 2, width, height]
    .map((value) => value.toFixed(1))
    .join(" ");
}

function viewBoxBounds(viewBox: string) {
  const values = viewBox.trim().split(/\s+/).map(Number);
  if (values.length !== 4 || values.some((value) => !Number.isFinite(value))) return null;
  return { x: values[0], y: values[1], width: values[2], height: values[3] };
}

function viewBoxSize(viewBox: string) {
  const bounds = viewBoxBounds(viewBox);
  return bounds ? { width: bounds.width, height: bounds.height } : null;
}

function clampPan(
  pan: { x: number; y: number },
  bounds: ReturnType<typeof viewBoxBounds>,
  viewport: { width: number; height: number } | null,
) {
  if (!bounds || !viewport) return pan;
  const maxPanX = Math.max(0, bounds.width - viewport.width) + viewport.width * PAN_OVERSCROLL_RATIO;
  const maxPanY = Math.max(0, bounds.height - viewport.height) + viewport.height * PAN_OVERSCROLL_RATIO;
  return {
    x: Math.max(-maxPanX, Math.min(maxPanX, pan.x)),
    y: Math.max(-maxPanY, Math.min(maxPanY, pan.y)),
  };
}

function zoomedViewBox(viewBox: string, zoom: number, pan: { x: number; y: number }) {
  const bounds = viewBoxBounds(viewBox);
  if (!bounds) return viewBox;
  const width = bounds.width / zoom;
  const height = bounds.height / zoom;
  const centeredX = bounds.x + (bounds.width - width) / 2;
  const centeredY = bounds.y + (bounds.height - height) / 2;
  const boundedPan = clampPan(pan, bounds, { width, height });
  return [centeredX - boundedPan.x, centeredY - boundedPan.y, width, height]
    .map((value) => value.toFixed(1))
    .join(" ");
}

export type SkillRelationshipMapProps = {
  nodes?: RelationshipGraphNode[];
  edges?: RelationshipGraphEdge[];
  focusName?: string;
  compact?: boolean;
  loading?: boolean;
  error?: string;
  onRetry?: () => void;
  onOpenSkill?: (name: string) => void;
};

export function SkillRelationshipMap({
  nodes,
  edges,
  focusName,
  compact = false,
  loading = false,
  error = "",
  onRetry,
  onOpenSkill,
}: SkillRelationshipMapProps) {
  const [hoveredName, setHoveredName] = useState<string | null>(null);
  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [isPanning, setIsPanning] = useState(false);
  const panInteractionRef = useRef<PanInteraction | null>(null);
  const pointerPositionsRef = useRef(new Map<number, PointerPosition>());
  const pinchInteractionRef = useRef<PinchInteraction | null>(null);
  const graphNodes: RelationshipGraphNode[] = nodes ?? EMPTY_GRAPH_NODES;
  const graphInputKey = useMemo(() => JSON.stringify({
    compact,
    focusName: focusName ?? "",
    nodes: graphNodes.map((node) => [
      node.name,
      node.label ?? "",
      node.kind ?? "",
      node.dependencies,
      node.dependents,
    ]),
    edges: edges?.map((edge) => [edge.from, edge.to, edge.key ?? ""]),
  }), [compact, edges, focusName, graphNodes]);
  const graph = useMemo(() => buildGraph(graphNodes, edges, focusName, compact), [graphInputKey]);
  const viewBox = useMemo(() => graphViewBox(graph.nodes, compact), [compact, graph.nodes]);
  const pannedViewBox = useMemo(() => zoomedViewBox(viewBox, zoom, pan), [pan, viewBox, zoom]);
  const renderedEdges = useMemo(() => {
    const nodesByName = new Map(graph.nodes.map((node) => [node.name, node]));
    return graph.edges.flatMap((edge) => {
      const from = nodesByName.get(edge.from);
      const to = nodesByName.get(edge.to);
      return from && to ? [{ key: edge.key, d: edgePath(from, to, edge.key), from: edge.from, to: edge.to }] : [];
    });
  }, [graph.edges, graph.nodes]);
  const renderedNodes = useMemo(() => graph.nodes.map((node) => ({
    ...node,
    label: shortName(relationshipNodeLabel(node)),
    labelOnRight: node.x < VIEWBOX_WIDTH / 2,
  })), [graph.nodes]);
  const connectedNames = useMemo(() => {
    if (!hoveredName) return null;
    const names = new Set([hoveredName]);
    for (const edge of graph.edges) {
      if (edge.from === hoveredName) names.add(edge.to);
      if (edge.to === hoveredName) names.add(edge.from);
    }
    return names;
  }, [graph.edges, hoveredName]);
  const handleTrackpadZoom = useTrackpadZoom(({ factor, clientX, clientY, rect }) => {
    const bounds = viewBoxBounds(viewBox);
    const size = viewBoxSize(pannedViewBox);
    if (!bounds || !size || rect.width <= 0 || rect.height <= 0) return;
    const currentBounds = viewBoxBounds(pannedViewBox);
    if (!currentBounds) return;
    const pointerRatioX = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
    const pointerRatioY = Math.max(0, Math.min(1, (clientY - rect.top) / rect.height));
    const focalWorldX = currentBounds.x + pointerRatioX * currentBounds.width;
    const focalWorldY = currentBounds.y + pointerRatioY * currentBounds.height;
    const nextZoom = Math.max(
      GRAPH_ZOOM_MIN,
      Math.min(GRAPH_ZOOM_MAX, zoom * factor),
    );
    if (nextZoom === zoom) return;
    const nextWidth = bounds.width / nextZoom;
    const nextHeight = bounds.height / nextZoom;
    const centeredX = bounds.x + (bounds.width - nextWidth) / 2;
    const centeredY = bounds.y + (bounds.height - nextHeight) / 2;
    setZoom(nextZoom);
    setPan(clampPan({
      x: centeredX - (focalWorldX - pointerRatioX * nextWidth),
      y: centeredY - (focalWorldY - pointerRatioY * nextHeight),
    }, bounds, { width: nextWidth, height: nextHeight }));
  });
  const handleWheel = (event: WheelEvent<HTMLDivElement>) => {
    if (handleTrackpadZoom(event)) return;
    event.preventDefault();
    event.stopPropagation();
    const rect = event.currentTarget.getBoundingClientRect();
    const bounds = viewBoxBounds(viewBox);
    const size = viewBoxSize(pannedViewBox);
    if (!bounds || !size || rect.width <= 0 || rect.height <= 0) return;

    const horizontalDelta = event.shiftKey ? event.deltaY : event.deltaX;
    const verticalDelta = event.shiftKey ? 0 : event.deltaY;
    setPan((current) => clampPan({
      x: current.x - (horizontalDelta / rect.width) * size.width,
      y: current.y - (verticalDelta / rect.height) * size.height,
    }, bounds, size));
  };
  const handlePointerDown = (event: PointerEvent<SVGSVGElement>) => {
    if (event.pointerType === "mouse" && event.button !== 0) return;
    const size = viewBoxSize(pannedViewBox);
    const rect = event.currentTarget.getBoundingClientRect();
    if (!size || rect.width <= 0 || rect.height <= 0) return;
    pointerPositionsRef.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
    event.currentTarget.setPointerCapture(event.pointerId);

    const activePointers = [...pointerPositionsRef.current.entries()].slice(0, 2);
    if (activePointers.length === 2) {
      const [first, second] = activePointers;
      const distance = Math.hypot(second[1].x - first[1].x, second[1].y - first[1].y);
      const currentBounds = viewBoxBounds(pannedViewBox);
      if (distance > 0 && currentBounds) {
        const midpoint = {
          x: (first[1].x + second[1].x) / 2,
          y: (first[1].y + second[1].y) / 2,
        };
        const pointerRatioX = Math.max(0, Math.min(1, (midpoint.x - rect.left) / rect.width));
        const pointerRatioY = Math.max(0, Math.min(1, (midpoint.y - rect.top) / rect.height));
        pinchInteractionRef.current = {
          pointerIds: [first[0], second[0]],
          startDistance: distance,
          startZoom: zoom,
          focalWorldX: currentBounds.x + pointerRatioX * currentBounds.width,
          focalWorldY: currentBounds.y + pointerRatioY * currentBounds.height,
        };
        panInteractionRef.current = null;
        setIsPanning(false);
        event.preventDefault();
        return;
      }
    }

    const target = event.target instanceof Element ? event.target : null;
    const nodeTarget = target?.closest<SVGGElement>(".skillRelationshipNodeGroup");
    const bounds = viewBoxBounds(viewBox);
    const boundedPan = clampPan(pan, bounds, size);
    panInteractionRef.current = {
      pointerId: event.pointerId,
      startClientX: event.clientX,
      startClientY: event.clientY,
      startPanX: boundedPan.x,
      startPanY: boundedPan.y,
      viewBoxWidth: size.width,
      viewBoxHeight: size.height,
      startedNodeName: nodeTarget?.dataset.nodeName ?? null,
      moved: false,
    };
    event.preventDefault();
  };
  const handlePointerMove = (event: PointerEvent<SVGSVGElement>) => {
    const activePointer = pointerPositionsRef.current.get(event.pointerId);
    if (!activePointer) return;
    activePointer.x = event.clientX;
    activePointer.y = event.clientY;

    const pinch = pinchInteractionRef.current;
    if (pinch && pinch.pointerIds.includes(event.pointerId)) {
      const first = pointerPositionsRef.current.get(pinch.pointerIds[0]);
      const second = pointerPositionsRef.current.get(pinch.pointerIds[1]);
      const rect = event.currentTarget.getBoundingClientRect();
      const bounds = viewBoxBounds(viewBox);
      if (!first || !second || !bounds || rect.width <= 0 || rect.height <= 0) return;
      const distance = Math.hypot(second.x - first.x, second.y - first.y);
      if (distance <= 0) return;

      const midpoint = {
        x: (first.x + second.x) / 2,
        y: (first.y + second.y) / 2,
      };
      const pointerRatioX = Math.max(0, Math.min(1, (midpoint.x - rect.left) / rect.width));
      const pointerRatioY = Math.max(0, Math.min(1, (midpoint.y - rect.top) / rect.height));
      const nextZoom = Math.max(
        GRAPH_ZOOM_MIN,
        Math.min(GRAPH_ZOOM_MAX, pinch.startZoom * Math.pow(distance / pinch.startDistance, TOUCH_PINCH_ZOOM_SPEED)),
      );
      const nextWidth = bounds.width / nextZoom;
      const nextHeight = bounds.height / nextZoom;
      const centeredX = bounds.x + (bounds.width - nextWidth) / 2;
      const centeredY = bounds.y + (bounds.height - nextHeight) / 2;
      const nextPan = clampPan({
        x: centeredX - (pinch.focalWorldX - pointerRatioX * nextWidth),
        y: centeredY - (pinch.focalWorldY - pointerRatioY * nextHeight),
      }, bounds, { width: nextWidth, height: nextHeight });
      setZoom(nextZoom);
      setPan(nextPan);
      setIsPanning(false);
      return;
    }

    const interaction = panInteractionRef.current;
    if (!interaction || interaction.pointerId !== event.pointerId) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const deltaX = event.clientX - interaction.startClientX;
    const deltaY = event.clientY - interaction.startClientY;
    if (!interaction.moved && Math.hypot(deltaX, deltaY) < 4) return;
    interaction.moved = true;
    setIsPanning(true);
    const bounds = viewBoxBounds(viewBox);
    setPan(clampPan({
      x: interaction.startPanX + (deltaX / rect.width) * interaction.viewBoxWidth,
      y: interaction.startPanY + (deltaY / rect.height) * interaction.viewBoxHeight,
    }, bounds, { width: interaction.viewBoxWidth, height: interaction.viewBoxHeight }));
  };
  const handlePointerUp = (event: PointerEvent<SVGSVGElement>, activateNode = true) => {
    const wasPinching = pinchInteractionRef.current?.pointerIds.includes(event.pointerId) ?? false;
    pointerPositionsRef.current.delete(event.pointerId);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (wasPinching) {
      pinchInteractionRef.current = null;
      panInteractionRef.current = null;
      setIsPanning(false);
      return;
    }

    const interaction = panInteractionRef.current;
    if (!interaction || interaction.pointerId !== event.pointerId) return;
    panInteractionRef.current = null;
    setIsPanning(false);
    if (activateNode && !interaction.moved && interaction.startedNodeName) {
      onOpenSkill?.(interaction.startedNodeName);
    }
  };
  const handleKeyDown = (event: KeyboardEvent<SVGGElement>, name: string) => {
    if ((event.key === "Enter" || event.key === " ") && onOpenSkill) {
      event.preventDefault();
      onOpenSkill(name);
    }
  };

  return (
    <section className={`skillRelationshipMap${compact ? " isCompact" : ""}`} aria-label="Skill relationships">
      {loading && graphNodes.length === 0 ? (
        <LoadingState label="Loading skill relationships" />
      ) : error ? (
        <LoadErrorState message={error} onRetry={onRetry} />
      ) : graphNodes.length === 0 ? (
        <div className="skillRelationshipEmpty">No skill relationships found.</div>
      ) : (
        <div className="skillRelationshipCanvas" onWheel={handleWheel}>
          <svg
            className={`skillRelationshipSvg${isPanning ? " isPanning" : ""}`}
            viewBox={pannedViewBox}
            role="img"
            aria-label={`${graphNodes.length} nodes and ${graph.edges.length} relationships`}
            onPointerCancel={(event) => handlePointerUp(event, false)}
            onPointerDown={handlePointerDown}
            onPointerMove={handlePointerMove}
            onPointerUp={handlePointerUp}
          >
            <g className="skillRelationshipEdges" aria-hidden="true">
              {renderedEdges.map((edge) => {
                const active = !hoveredName || edge.from === hoveredName || edge.to === hoveredName;
                return (
                  <path
                    className="skillRelationshipEdge"
                    data-active={active}
                    d={edge.d}
                    key={edge.key}
                  />
                );
              })}
            </g>
            <g className="skillRelationshipNodes">
              {renderedNodes.map((node) => {
                const active = !connectedNames || connectedNames.has(node.name);
                const labelOpacity = node.name === hoveredName ? 1 : active ? labelOpacityForNode(node, compact) : 0.16;
                return (
                  <g
                    aria-label={`${relationshipKindLabel(node.kind) ? `${relationshipKindLabel(node.kind)}: ` : ""}${relationshipNodeLabel(node)}, ${node.degree} relationships`}
                    className="skillRelationshipNodeGroup"
                    data-active={active}
                    data-clickable={Boolean(onOpenSkill)}
                    data-kind={node.kind}
                    data-node-name={node.name}
                    key={node.name}
                    onBlur={() => setHoveredName(null)}
                    onFocus={() => setHoveredName(node.name)}
                    onKeyDown={(event) => handleKeyDown(event, node.name)}
                    onMouseEnter={() => setHoveredName(node.name)}
                    onMouseLeave={() => setHoveredName(null)}
                    role={onOpenSkill ? "button" : undefined}
                    tabIndex={onOpenSkill ? 0 : undefined}
                  >
                    <circle className="skillRelationshipNode" cx={node.x} cy={node.y} r={node.radius} />
                    <text
                      className="skillRelationshipLabel"
                      data-active={active}
                      dominantBaseline="middle"
                      style={{ opacity: labelOpacity }}
                      textAnchor={node.labelOnRight ? "start" : "end"}
                      x={node.x + (node.labelOnRight ? node.radius + 7 : -node.radius - 7)}
                      y={node.y}
                    >
                      {node.label}
                    </text>
                  </g>
                );
              })}
            </g>
          </svg>
        </div>
      )}
    </section>
  );
}
