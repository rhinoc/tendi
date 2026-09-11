import { buildRelationshipGraph, type RelationshipGraphEdge, type RelationshipGraphNode } from "./skill-relationship-layout.ts";

export type RelationshipLayoutWorkerRequest = {
  id: number;
  nodes: RelationshipGraphNode[];
  edges?: RelationshipGraphEdge[];
  focusName?: string;
  compact: boolean;
};

export type RelationshipLayoutWorkerResponse =
  | { id: number; ok: true; graph: ReturnType<typeof buildRelationshipGraph> }
  | { id: number; ok: false; error: string };

const workerScope = self as unknown as {
  onmessage: ((event: MessageEvent<RelationshipLayoutWorkerRequest>) => void) | null;
  postMessage: (message: RelationshipLayoutWorkerResponse) => void;
};

workerScope.onmessage = ({ data }) => {
  try {
    workerScope.postMessage({
      id: data.id,
      ok: true,
      graph: buildRelationshipGraph(data.nodes, data.edges, data.focusName, data.compact),
    });
  } catch (error) {
    workerScope.postMessage({ id: data.id, ok: false, error: error instanceof Error ? error.message : String(error) });
  }
};
