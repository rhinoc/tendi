import { useEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent, WheelEvent } from "react";

import { formatSessionTitle } from "../../lib/session-preview.ts";
import { useTrackpadZoom } from "../../lib/zoom-gesture.ts";
import { LoadErrorState } from "../../components/shared/LoadErrorState.tsx";
import { LoadingState } from "../../components/shared/LoadingState.tsx";
import { cachedRelationshipLayout, requestRelationshipLayout } from "./skill-relationship-layout-client.ts";
import {
  buildRelationshipGraph,
  RELATIONSHIP_VIEWBOX_HEIGHT as VIEWBOX_HEIGHT,
  RELATIONSHIP_VIEWBOX_WIDTH as VIEWBOX_WIDTH,
  relationshipHash as hashString,
  relationshipNodeKey as nodeKey,
  type LayoutNode,
  type RelationshipGraph,
  type RelationshipGraphEdge,
  type RelationshipGraphNode,
} from "./skill-relationship-layout.ts";

export type { RelationshipGraphEdge, RelationshipGraphNode } from "./skill-relationship-layout.ts";

const MAX_LABEL_LENGTH = 24;
const MIN_LABEL_OPACITY = 0.42;
const MAX_LABEL_OPACITY = 0.7;
const PAN_OVERSCROLL_RATIO = 0.5;
const GRAPH_ZOOM_MIN = 0.72;
const GRAPH_ZOOM_MAX = 2.5;
const GRAPH_LABEL_MAX_SCALE = 1.35;
const TOUCH_PINCH_ZOOM_SPEED = 1.5;
const EMPTY_GRAPH_NODES: RelationshipGraphNode[] = [];
const EMPTY_GRAPH: RelationshipGraph = { nodes: [], edges: [] };

export enum RelationshipGraphKind {
  Session = "session",
  SessionParent = "session-parent",
  SessionChild = "session-child",
  SkillUsed = "skill-used",
  Wrapper = "wrapper",
  Plugin = "plugin",
  Remote = "remote",
  System = "system",
  Local = "local",
}

const RELATIONSHIP_KIND_LABELS = [
  { kind: RelationshipGraphKind.SessionParent, label: "Parent session" },
  { kind: RelationshipGraphKind.SessionChild, label: "Child session" },
  { kind: RelationshipGraphKind.SkillUsed, label: "Skill used" },
  { kind: RelationshipGraphKind.Wrapper, label: "Wrapper" },
  { kind: RelationshipGraphKind.Plugin, label: "Plugin" },
  { kind: RelationshipGraphKind.Remote, label: "Remote" },
  { kind: RelationshipGraphKind.System, label: "System" },
  { kind: RelationshipGraphKind.Local, label: "Local" },
] as const;

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

function isSessionNodeKind(kind?: string) {
  return kind === RelationshipGraphKind.Session || kind?.startsWith("session-") === true;
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

function labelScaleForZoom(zoom: number) {
  return Math.min(1, GRAPH_LABEL_MAX_SCALE / zoom);
}

type RenderedRelationshipNode = LayoutNode & {
  label: string;
  labelOnRight: boolean;
};

function estimatedLabelWidth(label: string) {
  return [...label].reduce((width, character) => width + (character.codePointAt(0)! > 0xff ? 15 : 8.2), 0);
}

function labelBoundsForNode(node: RenderedRelationshipNode, labelScale: number) {
  const labelX = node.x + (node.labelOnRight ? node.radius + 7 : -node.radius - 7);
  const width = estimatedLabelWidth(node.label) * labelScale;
  const height = 20 * labelScale;
  return {
    left: node.labelOnRight ? labelX : labelX - width,
    right: node.labelOnRight ? labelX + width : labelX,
    top: node.y - height / 2,
    bottom: node.y + height / 2,
  };
}

function labelsOverlap(left: ReturnType<typeof labelBoundsForNode>, right: ReturnType<typeof labelBoundsForNode>) {
  return left.left < right.right && right.left < left.right && left.top < right.bottom && right.top < left.bottom;
}

function isFocusedNode(node: LayoutNode, focusName?: string) {
  return Boolean(focusName && (node.name === focusName || nodeKey(node) === focusName));
}

function visibleLabelKeys(
  nodes: RenderedRelationshipNode[],
  labelScale: number,
  hoveredName: string | null,
  focusName?: string,
  connectedNames?: Set<string> | null,
) {
  const ordered = [...nodes].sort((left, right) => {
    const leftPriority = (nodeKey(left) === hoveredName ? 2 : 0) + (isFocusedNode(left, focusName) ? 1 : 0);
    const rightPriority = (nodeKey(right) === hoveredName ? 2 : 0) + (isFocusedNode(right, focusName) ? 1 : 0);
    return rightPriority - leftPriority
      || right.degree - left.degree
      || right.radius - left.radius
      || left.label.localeCompare(right.label);
  });
  const visible = new Set<string>();
  const visibleBounds: ReturnType<typeof labelBoundsForNode>[] = [];
  for (const node of ordered) {
    const bounds = labelBoundsForNode(node, labelScale);
    const forced = connectedNames?.has(nodeKey(node)) === true
      || nodeKey(node) === hoveredName
      || isFocusedNode(node, focusName);
    if (!forced && visibleBounds.some((visibleBound) => labelsOverlap(bounds, visibleBound))) continue;
    visible.add(nodeKey(node));
    visibleBounds.push(bounds);
  }
  return visible;
}

function labelOnRightForNode(node: LayoutNode, label: string) {
  const outwardRight = node.x >= VIEWBOX_WIDTH / 2;
  const labelX = node.x + (outwardRight ? node.radius + 7 : -node.radius - 7);
  const width = estimatedLabelWidth(label);
  const left = outwardRight ? labelX : labelX - width;
  const right = outwardRight ? labelX + width : labelX;
  if (left >= 16 && right <= VIEWBOX_WIDTH - 16) return outwardRight;
  return !outwardRight;
}

export function buildRelationshipGraphForPerformance(
  skills: RelationshipGraphNode[],
  explicitEdges?: RelationshipGraphEdge[],
  focusName?: string,
  compact = false,
) {
  return buildRelationshipGraph(skills, explicitEdges, focusName, compact);
}

function edgeCurve(from: LayoutNode, to: LayoutNode, key: string) {
  const deltaX = to.x - from.x;
  const deltaY = to.y - from.y;
  const length = Math.max(1, Math.sqrt(deltaX * deltaX + deltaY * deltaY));
  const curveFactor = (hashString(key) % 2001) / 1000 - 1;
  const curve = curveFactor * Math.min(30, length * 0.1);
  return {
    controlX: (from.x + to.x) / 2 - (deltaY / length) * curve,
    controlY: (from.y + to.y) / 2 + (deltaX / length) * curve,
  };
}

function edgePoint(from: LayoutNode, to: LayoutNode, controlX: number, controlY: number, progress: number) {
  const inverseProgress = 1 - progress;
  return {
    x: inverseProgress * inverseProgress * from.x
      + 2 * inverseProgress * progress * controlX
      + progress * progress * to.x,
    y: inverseProgress * inverseProgress * from.y
      + 2 * inverseProgress * progress * controlY
      + progress * progress * to.y,
  };
}

function edgeCenterlinePath(from: LayoutNode, to: LayoutNode, key: string) {
  const { controlX, controlY } = edgeCurve(from, to, key);
  return `M ${from.x.toFixed(1)} ${from.y.toFixed(1)} Q ${controlX.toFixed(1)} ${controlY.toFixed(1)} ${to.x.toFixed(1)} ${to.y.toFixed(1)}`;
}

function edgeRibbonPath(from: LayoutNode, to: LayoutNode, key: string, startWidth: number, endWidth: number) {
  const { controlX, controlY } = edgeCurve(from, to, key);
  const left: string[] = [];
  const right: string[] = [];
  const sampleCount = 14;

  for (let index = 0; index <= sampleCount; index += 1) {
    const progress = index / sampleCount;
    const point = edgePoint(from, to, controlX, controlY, progress);
    const tangentX = 2 * ((1 - progress) * (controlX - from.x) + progress * (to.x - controlX));
    const tangentY = 2 * ((1 - progress) * (controlY - from.y) + progress * (to.y - controlY));
    const tangentLength = Math.max(1, Math.sqrt(tangentX * tangentX + tangentY * tangentY));
    const halfWidth = (startWidth + (endWidth - startWidth) * progress) / 2;
    const normalX = -tangentY / tangentLength;
    const normalY = tangentX / tangentLength;
    left.push(`${(point.x + normalX * halfWidth).toFixed(2)} ${(point.y + normalY * halfWidth).toFixed(2)}`);
    right.push(`${(point.x - normalX * halfWidth).toFixed(2)} ${(point.y - normalY * halfWidth).toFixed(2)}`);
  }

  return `M ${left.join(" L ")} L ${right.reverse().join(" L ")} Z`;
}

function edgeWidthForDegree(degree: number) {
  if (degree >= 12) return 0.58;
  if (degree >= 5) return 0.86;
  return 1.22;
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
  const viewportFrameRef = useRef(0);
  const pendingViewportRef = useRef<{ zoom?: number; pan: { x: number; y: number } } | null>(null);
  const graphNodes: RelationshipGraphNode[] = nodes ?? EMPTY_GRAPH_NODES;
  const graphInputKey = useMemo(() => JSON.stringify({
    compact,
    focusName: focusName ?? "",
    nodes: graphNodes.map((node) => [
      nodeKey(node),
      node.name,
      node.label ?? "",
      node.kind ?? "",
      node.dependencies,
      node.dependents,
      node.dependencyIds,
      node.dependentIds,
    ]),
    edges: edges?.map((edge) => [edge.from, edge.to, edge.key ?? ""]),
  }), [compact, edges, focusName, graphNodes]);
  const [layoutState, setLayoutState] = useState<{
    key: string;
    graph: RelationshipGraph;
    loading: boolean;
    error: string;
  }>({ key: "", graph: EMPTY_GRAPH, loading: false, error: "" });
  useEffect(() => {
    if (graphNodes.length === 0) {
      setLayoutState({ key: graphInputKey, graph: EMPTY_GRAPH, loading: false, error: "" });
      return;
    }
    const cached = cachedRelationshipLayout(graphInputKey);
    if (cached) {
      setLayoutState({ key: graphInputKey, graph: cached, loading: false, error: "" });
      return;
    }
    let cancelled = false;
    setLayoutState((current) => ({ ...current, key: graphInputKey, loading: true, error: "" }));
    void requestRelationshipLayout({ key: graphInputKey, nodes: graphNodes, edges, focusName, compact })
      .then((graph) => {
        if (!cancelled) setLayoutState({ key: graphInputKey, graph, loading: false, error: "" });
      })
      .catch((layoutError) => {
        if (!cancelled) setLayoutState((current) => ({
          ...current,
          key: graphInputKey,
          loading: false,
          error: layoutError instanceof Error ? layoutError.message : String(layoutError),
        }));
      });
    return () => {
      cancelled = true;
    };
  }, [graphInputKey]);
  useEffect(() => () => {
    if (viewportFrameRef.current !== 0) window.cancelAnimationFrame(viewportFrameRef.current);
  }, []);
  const graph = layoutState.graph;
  const layoutLoading = layoutState.loading || layoutState.key !== graphInputKey;
  const labelScale = labelScaleForZoom(zoom);
  const scheduleViewport = (next: { zoom?: number; pan: { x: number; y: number } }) => {
    pendingViewportRef.current = next;
    if (viewportFrameRef.current !== 0) return;
    viewportFrameRef.current = window.requestAnimationFrame(() => {
      viewportFrameRef.current = 0;
      const pendingViewport = pendingViewportRef.current;
      pendingViewportRef.current = null;
      if (!pendingViewport) return;
      if (pendingViewport.zoom !== undefined) setZoom(pendingViewport.zoom);
      setPan(pendingViewport.pan);
    });
  };
  const viewBox = useMemo(() => graphViewBox(graph.nodes, compact), [compact, graph.nodes]);
  const pannedViewBox = useMemo(() => zoomedViewBox(viewBox, zoom, pan), [pan, viewBox, zoom]);
  const renderedEdges = useMemo(() => {
    const nodesByKey = new Map(graph.nodes.map((node) => [nodeKey(node), node]));
    return graph.edges.flatMap((edge) => {
      const from = nodesByKey.get(edge.from);
      const to = nodesByKey.get(edge.to);
      const endpointDegree = from && to ? Math.max(from.degree, to.degree) : 0;
      const density = endpointDegree >= 12 ? "high" : endpointDegree >= 5 ? "medium" : "low";
      return from && to ? [{
        key: edge.key,
        d: edgeRibbonPath(from, to, edge.key, edgeWidthForDegree(from.degree), edgeWidthForDegree(to.degree)),
        centerline: edgeCenterlinePath(from, to, edge.key),
        from: edge.from,
        to: edge.to,
        congested: density === "high",
        density,
      }] : [];
    });
  }, [graph.edges, graph.nodes]);
  const renderedNodes = useMemo(() => graph.nodes.map((node) => ({
    ...node,
    label: shortName(relationshipNodeLabel(node)),
    labelOnRight: labelOnRightForNode(node, shortName(relationshipNodeLabel(node))),
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
  const visibleLabels = useMemo(
    () => visibleLabelKeys(renderedNodes, labelScale, hoveredName, focusName, connectedNames),
    [connectedNames, focusName, hoveredName, labelScale, renderedNodes],
  );
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
      scheduleViewport({ zoom: nextZoom, pan: nextPan });
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
    scheduleViewport({ pan: clampPan({
      x: interaction.startPanX + (deltaX / rect.width) * interaction.viewBoxWidth,
      y: interaction.startPanY + (deltaY / rect.height) * interaction.viewBoxHeight,
    }, bounds, { width: interaction.viewBoxWidth, height: interaction.viewBoxHeight }) });
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
  const hasGraphInput = graphNodes.length > 0;
  const hasGraphNodes = graph.nodes.length > 0;

  return (
    <section
      className={`skillRelationshipMap${compact ? " isCompact" : ""}`}
      data-hovered={Boolean(hoveredName)}
      aria-label="Skill relationships"
    >
      {(loading || layoutLoading) && !hasGraphNodes ? (
        <LoadingState label="Loading skill relationships" />
      ) : (error || layoutState.error) && !hasGraphNodes ? (
        <LoadErrorState message={error || layoutState.error} onRetry={onRetry} />
      ) : !hasGraphInput ? (
        <div className="skillRelationshipEmpty">No skill relationships found.</div>
      ) : (
        <div className="skillRelationshipContent">
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
            <defs>
              <marker
                id="skillRelationshipArrow"
                markerHeight="6"
                markerUnits="userSpaceOnUse"
                markerWidth="7"
                orient="auto"
                refX="6"
                refY="3"
                viewBox="0 0 7 6"
              >
                <path d="M 0 0 L 7 3 L 0 6 Z" fill="var(--line-strong)" />
              </marker>
            </defs>
            <g className="skillRelationshipEdges" aria-hidden="true">
              {renderedEdges.map((edge) => {
                const active = !hoveredName || edge.from === hoveredName || edge.to === hoveredName;
                return (
                  <g key={edge.key}>
                    <path
                      className="skillRelationshipEdge"
                      data-active={active}
                      data-congested={edge.congested}
                      data-density={edge.density}
                      d={edge.d}
                    />
                    <path
                      className="skillRelationshipEdgeDirection"
                      data-active={active}
                      d={edge.centerline}
                      markerEnd="url(#skillRelationshipArrow)"
                    />
                  </g>
                );
              })}
            </g>
            <g className="skillRelationshipNodes">
              {renderedNodes.map((node) => {
                const selfKey = nodeKey(node);
                const active = !connectedNames || connectedNames.has(selfKey);
                const labelOpacity = selfKey === hoveredName ? 1 : active ? labelOpacityForNode(node, compact) : 0.16;
                const labelX = node.x + (node.labelOnRight ? node.radius + 7 : -node.radius - 7);
                return (
                  <g
                    aria-label={`${relationshipKindLabel(node.kind) ? `${relationshipKindLabel(node.kind)}: ` : ""}${relationshipNodeLabel(node)}, ${node.degree} relationships`}
                    className="skillRelationshipNodeGroup"
                    data-active={active}
                    data-clickable={Boolean(onOpenSkill)}
                    data-kind={node.kind}
                    data-node-name={selfKey}
                    key={selfKey}
                    onBlur={() => setHoveredName(null)}
                    onFocus={() => setHoveredName(selfKey)}
                    onKeyDown={(event) => handleKeyDown(event, selfKey)}
                    onMouseEnter={() => setHoveredName(selfKey)}
                    onMouseLeave={() => setHoveredName(null)}
                    role={onOpenSkill ? "button" : undefined}
                    tabIndex={onOpenSkill ? 0 : undefined}
                  >
                    <circle className="skillRelationshipNode" cx={node.x} cy={node.y} r={node.radius} />
                    <text
                      className="skillRelationshipLabel"
                      data-active={active}
                      dominantBaseline="middle"
                      style={{ opacity: visibleLabels.has(selfKey) ? labelOpacity : 0 }}
                      textAnchor={node.labelOnRight ? "start" : "end"}
                      transform={`translate(${labelX} ${node.y}) scale(${labelScale}) translate(${-labelX} ${-node.y})`}
                      x={labelX}
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
          {loading || layoutLoading ? (
            <div className="skillRelationshipStatusOverlay" aria-live="polite">
              <LoadingState label="Refreshing skill relationships" />
            </div>
          ) : error || layoutState.error ? (
            <div className="skillRelationshipStatusOverlay">
              <LoadErrorState message={error || layoutState.error} onRetry={onRetry} />
            </div>
          ) : null}
        </div>
      )}
    </section>
  );
}
