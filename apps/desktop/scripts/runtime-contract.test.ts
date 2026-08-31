import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { decideRevision } from "../src/lib/runtime-contract.ts";
import { RuntimeContractError, validateEvent, validateRequest, validateResponse, validateResult } from "../src/lib/generated/runtime-validators.ts";
import { RuntimeClient } from "../src/lib/generated/runtime-client.ts";
import type { CommandName } from "../src/lib/generated/runtime-types.ts";

test("drops duplicate and stale revisioned events", () => {
  const decision = decideRevision(3, {
    scopeKey: "workspace:/repo",
    domain: "sessions",
    operationId: "op-1",
    baseRevision: 3,
    revision: 3,
    payload: "stale",
  });

  assert.deepEqual(decision, { accepted: false, needsResync: false });
});

test("requires resync when an event revision has a gap", () => {
  const decision = decideRevision(3, {
    scopeKey: "workspace:/repo",
    domain: "sessions",
    operationId: "op-1",
    baseRevision: 2,
    revision: 4,
    payload: "gap",
  });

  assert.deepEqual(decision, { accepted: false, needsResync: true });
});

test("accepts the next event only from the current revision", () => {
  const decision = decideRevision(3, {
    scopeKey: "workspace:/repo",
    domain: "sessions",
    operationId: "op-1",
    baseRevision: 3,
    revision: 4,
    payload: "next",
  });

  assert.deepEqual(decision, { accepted: true, needsResync: false, payload: "next" });
});

test("generated validators enforce empty request and safe revision fields", () => {
  validateRequest("sessions_snapshot", {});
  validateResult("open_url", null);
  assert.throws(() => validateRequest("open_url", null), RuntimeContractError);
  assert.throws(() => validateRequest("sessions_snapshot", { unexpected: true }), RuntimeContractError);
  assert.throws(() => validateResult("sessions_snapshot", {
    scopeKey: "workspace:/repo",
    domain: "sessions",
    revision: Number.MAX_SAFE_INTEGER + 1,
    schemaVersion: 1,
    snapshotId: "snapshot-1",
    payload: [],
  }), RuntimeContractError);
});

test("generated client sends one JSON-RPC envelope and validates its result", async () => {
  const requests: unknown[] = [];
  const client = new RuntimeClient({
    async request(request) {
      requests.push(request);
      return {
        jsonrpc: "2.0",
        id: request.id,
        result: {
          scopeKey: "workspace:/repo",
          domain: "sessions",
          revision: 0,
          schemaVersion: 1,
          snapshotId: "snapshot-1",
          payload: [],
        },
      };
    },
  });
  const snapshot = await client.sessionsSnapshot({});
  assert.equal(snapshot.domain, "sessions");
  assert.equal(requests.length, 1);
  const request = requests[0] as { jsonrpc: string; id: string; method: string; params: unknown };
  assert.deepEqual({ jsonrpc: request.jsonrpc, method: request.method, params: request.params }, {
    jsonrpc: "2.0",
    method: "sessions_snapshot",
    params: {},
  });
  assert.match(request.id, /^desktop-\d+$/);
});

test("generated event validator rejects an invalid event envelope", () => {
  assert.throws(() => validateEvent({ id: 1, event: "sessions://scan", payload: [], extra: true }), RuntimeContractError);
});

test("generated response validator enforces the JSON-RPC result/error union", () => {
  validateResponse({ jsonrpc: "2.0", id: "request-1", result: {} });
  validateResponse({ jsonrpc: "2.0", id: "request-2", error: { code: -32600, message: "invalid" } });
  assert.throws(() => validateResponse({ jsonrpc: "2.0", id: "request-3" }), RuntimeContractError);
  assert.throws(() => validateResponse({ jsonrpc: "2.0", id: "request-4", result: {}, error: { code: -32600, message: "invalid" } }), RuntimeContractError);
});

test("generated valid and invalid fixtures cover every command and event", () => {
  const fixtures = JSON.parse(readFileSync(new URL("../../../runtime-schema/examples/contract-fixtures.json", import.meta.url), "utf8")) as {
    validRequests: Record<string, unknown>;
    invalidRequests: Record<string, unknown>;
    validResults: Record<string, unknown>;
    invalidResponses: Record<string, unknown>;
    validEvents: Record<string, unknown>;
    invalidEvents: Record<string, unknown>;
  };
  for (const [command, request] of Object.entries(fixtures.validRequests)) {
    assert.doesNotThrow(() => validateRequest(command as CommandName, request));
  }
  for (const [command, request] of Object.entries(fixtures.invalidRequests)) {
    assert.throws(() => validateRequest(command as CommandName, request), RuntimeContractError);
  }
  for (const [command, result] of Object.entries(fixtures.validResults)) {
    assert.doesNotThrow(() => validateResult(command as CommandName, result));
  }
  for (const response of Object.values(fixtures.invalidResponses)) {
    assert.throws(() => validateResponse(response), RuntimeContractError);
  }
  for (const event of Object.values(fixtures.validEvents)) assert.doesNotThrow(() => validateEvent(event));
  for (const event of Object.values(fixtures.invalidEvents)) assert.throws(() => validateEvent(event), RuntimeContractError);
});
