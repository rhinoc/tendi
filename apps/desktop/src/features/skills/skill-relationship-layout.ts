export const RELATIONSHIP_VIEWBOX_WIDTH = 1200;
export const RELATIONSHIP_VIEWBOX_HEIGHT = 660;

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

export type RelationshipGraphNode = {
  id?: string;
  name: string;
  label?: string;
  description?: string;
  kind?: string;
  dependencies?: string[];
  dependents?: string[];
  dependencyIds?: string[];
  dependentIds?: string[];
};

export function resolveRelationshipNode<T extends { id?: string; name: string }>(
  nodes: readonly T[],
  reference: string,
): T | undefined {
  const trimmed = reference.trim();
  if (!trimmed) return undefined;
  return nodes.find((node) => node.id?.trim() === trimmed);
}

export type RelationshipGraphEdge = {
  from: string;
  to: string;
  key?: string;
};

export type RelationshipEdge = {
  from: string;
  to: string;
  key: string;
};

export type LayoutNode = RelationshipGraphNode & {
  degree: number;
  radius: number;
  x: number;
  y: number;
};

export type RelationshipGraph = {
  nodes: LayoutNode[];
  edges: RelationshipEdge[];
};

export function relationshipNodeKey(node: { id?: string; name: string }) {
  return node.id?.trim() || node.name;
}

export function relationshipHash(value: string) {
  let hash = 2166136261;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

function resolveNodeKey(nodes: RelationshipGraphNode[], reference: string): string | undefined {
  const node = resolveRelationshipNode(nodes, reference);
  return node ? relationshipNodeKey(node) : undefined;
}

function matchesFocus(node: RelationshipGraphNode, focus?: string) {
  return Boolean(focus && relationshipNodeKey(node) === focus);
}

function relationEdges(nodes: RelationshipGraphNode[], explicitEdges?: RelationshipGraphEdge[]) {
  const keys = new Set(nodes.map(relationshipNodeKey));
  const edges = new Map<string, RelationshipEdge>();
  const addEdge = (from: string, to: string, suppliedKey?: string) => {
    if (!keys.has(from) || !keys.has(to) || from === to) return;
    const key = `${from}\u0000${to}`;
    if (!edges.has(key)) edges.set(key, { from, to, key: suppliedKey ?? key });
  };

  if (explicitEdges) {
    for (const edge of explicitEdges) addEdge(edge.from, edge.to, edge.key);
    return [...edges.values()];
  }

  for (const node of nodes) {
    const selfKey = relationshipNodeKey(node);
    for (const dependency of node.dependencyIds ?? []) {
      const dependencyKey = resolveNodeKey(nodes, dependency);
      if (dependencyKey) addEdge(dependencyKey, selfKey);
    }
    for (const dependent of node.dependentIds ?? []) {
      const dependentKey = resolveNodeKey(nodes, dependent);
      if (dependentKey) addEdge(selfKey, dependentKey);
    }
  }
  return [...edges.values()];
}

function connectedComponents(nodes: RelationshipGraphNode[], edges: RelationshipEdge[]) {
  const neighbors = new Map(nodes.map((node) => [relationshipNodeKey(node), new Set<string>()]));
  for (const edge of edges) {
    neighbors.get(edge.from)?.add(edge.to);
    neighbors.get(edge.to)?.add(edge.from);
  }
  const byKey = new Map(nodes.map((node) => [relationshipNodeKey(node), node]));
  const visited = new Set<string>();
  const components: RelationshipGraphNode[][] = [];
  for (const node of nodes) {
    const selfKey = relationshipNodeKey(node);
    if (visited.has(selfKey)) continue;
    const component: RelationshipGraphNode[] = [];
    const stack = [selfKey];
    while (stack.length > 0) {
      const key = stack.pop();
      if (!key || visited.has(key)) continue;
      visited.add(key);
      const item = byKey.get(key);
      if (item) component.push(item);
      for (const neighbor of neighbors.get(key) ?? []) {
        if (!visited.has(neighbor)) stack.push(neighbor);
      }
    }
    components.push(component);
  }
  return components;
}

function clusterComponents(nodes: RelationshipGraphNode[], edges: RelationshipEdge[]) {
  const components = connectedComponents(nodes, edges).sort((left, right) => right.length - left.length);
  const connected = components.filter((component) => component.length > 1);
  const isolated = components.filter((component) => component.length === 1).flat();
  for (let index = 0; index < isolated.length; index += 12) connected.push(isolated.slice(index, index + 12));
  return connected.length > 0 ? connected : components;
}

function relaxNodes(nodes: LayoutNode[], edges: RelationshipEdge[]) {
  const positioned = nodes.map((node) => ({ ...node, velocityX: 0, velocityY: 0 }));
  const indexByKey = new Map(positioned.map((node, index) => [relationshipNodeKey(node), index]));
  const maxDegree = Math.max(1, ...positioned.map((node) => node.degree));
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
      const fromIndex = indexByKey.get(edge.from);
      const toIndex = indexByKey.get(edge.to);
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
      forces[index].x += (RELATIONSHIP_VIEWBOX_WIDTH / 2 - node.x) * centralPull;
      forces[index].y += (RELATIONSHIP_VIEWBOX_HEIGHT / 2 - node.y) * centralPull;
      node.velocityX = (node.velocityX + forces[index].x) * GRAPH_LAYOUT.damping;
      node.velocityY = (node.velocityY + forces[index].y) * GRAPH_LAYOUT.damping;
      node.x += node.velocityX;
      node.y += node.velocityY;
    });
  }

  return positioned.map(({ velocityX: _velocityX, velocityY: _velocityY, ...node }) => node);
}

function normalizeNodes(nodes: LayoutNode[]) {
  if (nodes.length < 2) return nodes;
  const minX = Math.min(...nodes.map((node) => node.x));
  const maxX = Math.max(...nodes.map((node) => node.x));
  const minY = Math.min(...nodes.map((node) => node.y));
  const maxY = Math.max(...nodes.map((node) => node.y));
  const sourceWidth = Math.max(1, maxX - minX);
  const sourceHeight = Math.max(1, maxY - minY);
  const scaleX = Math.min(1, (RELATIONSHIP_VIEWBOX_WIDTH - 160) / sourceWidth);
  const scaleY = Math.min(1, (RELATIONSHIP_VIEWBOX_HEIGHT - 128) / sourceHeight);
  const sourceCenterX = (minX + maxX) / 2;
  const sourceCenterY = (minY + maxY) / 2;
  const targetCenterX = RELATIONSHIP_VIEWBOX_WIDTH / 2;
  const targetCenterY = RELATIONSHIP_VIEWBOX_HEIGHT / 2;
  return nodes.map((node) => ({
    ...node,
    x: targetCenterX + (node.x - sourceCenterX) * scaleX,
    y: targetCenterY + (node.y - sourceCenterY) * scaleY,
  }));
}

function resolveNodeCollisions(nodes: LayoutNode[]) {
  const positioned = nodes.map((node) => ({ ...node }));
  for (let iteration = 0; iteration < 120; iteration += 1) {
    let overlaps = 0;
    for (let leftIndex = 0; leftIndex < positioned.length; leftIndex += 1) {
      const left = positioned[leftIndex];
      for (let rightIndex = leftIndex + 1; rightIndex < positioned.length; rightIndex += 1) {
        const right = positioned[rightIndex];
        let deltaX = right.x - left.x;
        let deltaY = right.y - left.y;
        let distance = Math.sqrt(deltaX * deltaX + deltaY * deltaY);
        const minimumDistance = 26 + left.radius + right.radius;
        if (distance >= minimumDistance) continue;
        overlaps += 1;
        if (distance < 0.001) {
          const angle = (relationshipHash(`${relationshipNodeKey(left)}\u0000${relationshipNodeKey(right)}`) % 628) / 100;
          deltaX = Math.cos(angle);
          deltaY = Math.sin(angle);
          distance = 1;
        }
        const correction = (minimumDistance - distance) / distance * 0.5;
        left.x -= deltaX * correction;
        left.y -= deltaY * correction;
        right.x += deltaX * correction;
        right.y += deltaY * correction;
      }
    }
    if (overlaps === 0) break;
  }
  return positioned;
}

export function buildRelationshipGraph(
  nodes: RelationshipGraphNode[],
  explicitEdges?: RelationshipGraphEdge[],
  focusName?: string,
  compact = false,
): RelationshipGraph {
  const edges = relationEdges(nodes, explicitEdges);
  const degree = new Map(nodes.map((node) => [relationshipNodeKey(node), 0]));
  for (const edge of edges) {
    degree.set(edge.from, (degree.get(edge.from) ?? 0) + 1);
    degree.set(edge.to, (degree.get(edge.to) ?? 0) + 1);
  }

  const clusters = clusterComponents(nodes, edges);
  const positions = new Map<string, { x: number; y: number }>();
  const centerX = RELATIONSHIP_VIEWBOX_WIDTH / 2;
  const centerY = RELATIONSHIP_VIEWBOX_HEIGHT / 2;
  const satelliteRadius = Math.min(390, 288 + clusters.length * 12);
  clusters.forEach((cluster, clusterIndex) => {
    const clusterSeed = relationshipHash(cluster.map(relationshipNodeKey).join("\u0000"));
    const angleJitter = ((clusterSeed % 101) / 100 - 0.5) * 0.45;
    const angle = -Math.PI / 2 + ((clusterIndex - 1) / Math.max(1, clusters.length - 1)) * Math.PI * 2 + angleJitter;
    const clusterRatio = Math.sqrt(Math.max(0, (clusterIndex - 0.5) / Math.max(1, clusters.length - 1)));
    const clusterDistance = satelliteRadius * (0.55 + clusterRatio * 0.45) * (0.86 + (clusterSeed % 29) / 100);
    const clusterCenter = clusterIndex === 0 || clusters.length === 1
      ? { x: centerX, y: centerY }
      : {
          x: centerX + Math.cos(angle) * clusterDistance,
          y: centerY + Math.sin(angle) * Math.min(218, satelliteRadius * 0.54) * (0.86 + (clusterSeed % 23) / 100),
        };
    const ordered = [...cluster].sort((left, right) => {
      if (matchesFocus(left, focusName)) return -1;
      if (matchesFocus(right, focusName)) return 1;
      return (degree.get(relationshipNodeKey(right)) ?? 0) - (degree.get(relationshipNodeKey(left)) ?? 0)
        || left.name.localeCompare(right.name);
    });
    const hub = ordered[0];
    if (hub) positions.set(relationshipNodeKey(hub), clusterCenter);
    if (ordered.length === 1) return;
    const clusterRadius = clusterIndex === 0 || clusters.length === 1
      ? Math.min(compact ? 180 : 260, (compact ? 40 : 68) + Math.sqrt(ordered.length) * 34)
      : Math.min(88, 22 + Math.sqrt(ordered.length) * 14);
    const horizontalSpread = compact ? 1.45 : 1.2;
    const satelliteCount = ordered.length - 1;
    ordered.slice(1).forEach((node, index) => {
      const selfKey = relationshipNodeKey(node);
      const nodeSeed = relationshipHash(selfKey);
      const nodeAngle = index * 2.399963 + ((nodeSeed % 101) / 100 - 0.5) * 0.32;
      const radialRatio = Math.sqrt((index + 0.75) / (satelliteCount + 0.5));
      const jitter = 0.86 + (nodeSeed % 29) / 100;
      const nodeDistance = clusterRadius * (0.38 + radialRatio * 0.62) * jitter;
      positions.set(selfKey, {
        x: clusterCenter.x + Math.cos(nodeAngle) * nodeDistance * horizontalSpread,
        y: clusterCenter.y + Math.sin(nodeAngle) * nodeDistance,
      });
    });
  });

  const layoutNodes = nodes.map((node) => {
    const selfKey = relationshipNodeKey(node);
    const nodeDegree = degree.get(selfKey) ?? 0;
    const position = positions.get(selfKey) ?? { x: centerX, y: centerY };
    return {
      ...node,
      degree: nodeDegree,
      radius: matchesFocus(node, focusName)
        ? compact ? Math.min(24, 13 + Math.sqrt(nodeDegree) * 2.2) : Math.min(16, 8 + Math.sqrt(nodeDegree) * 1.6)
        : compact ? Math.min(14, 6 + Math.sqrt(nodeDegree) * 1.8) : Math.min(9, 3.6 + Math.sqrt(nodeDegree) * 1.25),
      ...position,
    };
  });
  return { nodes: resolveNodeCollisions(normalizeNodes(relaxNodes(layoutNodes, edges))), edges };
}
