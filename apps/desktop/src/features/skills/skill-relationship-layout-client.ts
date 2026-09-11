import type { RelationshipGraph, RelationshipGraphEdge, RelationshipGraphNode } from "./skill-relationship-layout.ts";
import type { RelationshipLayoutWorkerRequest, RelationshipLayoutWorkerResponse } from "./skill-relationship-layout.worker.ts";

type PendingLayout = {
  resolve: (graph: RelationshipGraph) => void;
  reject: (error: Error) => void;
};

const cache = new Map<string, RelationshipGraph>();
const inFlight = new Map<string, Promise<RelationshipGraph>>();
const pending = new Map<number, PendingLayout>();
let worker: Worker | null = null;
let nextRequestId = 0;
const LAYOUT_CACHE_LIMIT = 12;

function cacheLayout(key: string, graph: RelationshipGraph) {
  cache.delete(key);
  cache.set(key, graph);
  if (cache.size > LAYOUT_CACHE_LIMIT) cache.delete(cache.keys().next().value!);
}

function layoutWorker() {
  if (worker) return worker;
  worker = new Worker(new URL("./skill-relationship-layout.worker.ts", import.meta.url), { type: "module" });
  worker.onmessage = ({ data }: MessageEvent<RelationshipLayoutWorkerResponse>) => {
    const request = pending.get(data.id);
    if (!request) return;
    pending.delete(data.id);
    if (data.ok) request.resolve(data.graph);
    else request.reject(new Error(data.error));
  };
  worker.onerror = (event) => {
    const error = new Error(event.message || "Relationship layout worker failed");
    for (const request of pending.values()) request.reject(error);
    pending.clear();
    worker?.terminate();
    worker = null;
  };
  return worker;
}

export function cachedRelationshipLayout(key: string) {
  return cache.get(key);
}

export function requestRelationshipLayout(input: {
  key: string;
  nodes: RelationshipGraphNode[];
  edges?: RelationshipGraphEdge[];
  focusName?: string;
  compact: boolean;
}): Promise<RelationshipGraph> {
  const cached = cache.get(input.key);
  if (cached) return Promise.resolve(cached);
  const activeRequest = inFlight.get(input.key);
  if (activeRequest) return activeRequest;
  const id = ++nextRequestId;
  const request: RelationshipLayoutWorkerRequest = { id, nodes: input.nodes, edges: input.edges, focusName: input.focusName, compact: input.compact };
  const promise = new Promise<RelationshipGraph>((resolve, reject) => {
    pending.set(id, {
      resolve: (graph) => {
        cacheLayout(input.key, graph);
        resolve(graph);
      },
      reject,
    });
    layoutWorker().postMessage(request);
  });
  inFlight.set(input.key, promise);
  void promise.then(
    () => inFlight.delete(input.key),
    () => inFlight.delete(input.key),
  );
  return promise;
}
