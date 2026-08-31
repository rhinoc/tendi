#!/usr/bin/env node

import { readFile, mkdir, readdir, writeFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const requireFromDesktop = createRequire(resolve(root, "apps/desktop/package.json"));
const Ajv2020 = requireFromDesktop("ajv/dist/2020");
const schemaPath = resolve(root, "runtime-schema/runtime.openrpc.json");
const metaSchemaPath = resolve(root, "runtime-schema/meta-schema.json");
const checkOnly = process.argv.includes("--check");
const validateOnly = process.argv.includes("--validate-only");

const schema = JSON.parse(await readFile(schemaPath, "utf8"));
const metaSchema = JSON.parse(await readFile(metaSchemaPath, "utf8"));
const methods = schema.methods;
const components = schema.components?.schemas ?? {};
const events = schema["x-tendi"]?.events ?? [];
const errors = schema["x-tendi"]?.errors ?? [];

function fail(message) {
  throw new Error(`runtime schema: ${message}`);
}

function assert(condition, message) {
  if (!condition) fail(message);
}

function refName(value) {
  const ref = value?.$ref;
  return typeof ref === "string" && ref.startsWith("#/components/schemas/")
    ? ref.slice("#/components/schemas/".length)
    : undefined;
}

function resolveSchema(value, seen = new Set()) {
  const name = refName(value);
  if (!name) return value;
  assert(Object.hasOwn(components, name), `unresolved schema reference ${value.$ref}`);
  assert(!seen.has(name), `cyclic schema reference ${name}`);
  return resolveSchema(components[name], new Set([...seen, name]));
}

function validateDocument() {
  assert(metaSchema.$schema === "https://json-schema.org/draft/2020-12/schema", "meta-schema must use JSON Schema 2020-12");
  const metaValidator = new Ajv2020({ allErrors: true, strict: false }).compile(metaSchema);
  assert(metaValidator(schema), `meta-schema validation failed: ${metaValidator.errors?.[0]?.message ?? "invalid document"}`);
  for (const field of metaSchema.required ?? []) assert(Object.hasOwn(schema, field), `meta-schema required field ${field} is missing`);
  assert(schema.openrpc === "1.3.2", `unsupported OpenRPC version ${schema.openrpc}`);
  assert(schema.info?.title && schema.info?.version, "info.title and info.version are required");
  assert(Array.isArray(methods) && methods.length > 0, "methods must be a non-empty array");
  assert(Array.isArray(events), "x-tendi.events must be an array");
  assert(Array.isArray(errors) && errors.length > 0, "x-tendi.errors must be a non-empty array");
  const errorKinds = new Set();
  for (const error of errors) {
    assert(typeof error.kind === "string" && Number.isInteger(error.code), "every error needs a symbolic kind and numeric code");
    assert(!errorKinds.has(error.kind), `duplicate error kind ${error.kind}`);
    errorKinds.add(error.kind);
  }

  const methodNames = new Set();
  for (const method of methods) {
    assert(method && typeof method.name === "string" && /^[a-z][a-z0-9_]*$/.test(method.name), "every method needs a valid name");
    assert(!methodNames.has(method.name), `duplicate method ${method.name}`);
    methodNames.add(method.name);
    assert(Array.isArray(method.params), `${method.name}: params must be an array`);
    assert(method.params.length <= 1, `${method.name}: only one named request object is supported`);
    if (method.params.length === 1) {
      assert(method.params[0].name === "request" && method.params[0].required === true, `${method.name}: request must be a required named parameter`);
      resolveSchema(method.params[0].schema);
    }
    assert(method.result?.schema, `${method.name}: result schema is required`);
    resolveSchema(method.result.schema);
    const metadata = method["x-tendi"];
    assert(metadata?.owner && metadata?.wire && Array.isArray(metadata.clients) && metadata.execution, `${method.name}: incomplete x-tendi metadata`);
    assert(["daemon", "desktop"].includes(metadata.owner), `${method.name}: invalid owner`);
    assert(["jsonrpc", "tauri"].includes(metadata.wire), `${method.name}: invalid wire`);
    assert(["read", "write"].includes(metadata.execution), `${method.name}: invalid execution`);
  }

  const eventNames = new Set();
  for (const event of events) {
    assert(event && typeof event.name === "string" && event.name.length > 0, "every event needs a name");
    assert(!eventNames.has(event.name), `duplicate event ${event.name}`);
    eventNames.add(event.name);
    assert(event.payload, `${event.name}: payload schema is required`);
    resolveSchema(event.payload);
    assert(["sse", "tauri"].includes(event.wire), `${event.name}: invalid event wire`);
    assert(typeof event.revisioned === "boolean" && typeof event.replay === "boolean", `${event.name}: revision/replay metadata is required`);
  }

  for (const [name, component] of Object.entries(components)) {
    assert(component && typeof component === "object", `component ${name} must be an object`);
    if (component.type === "integer" && component.maximum === undefined) {
      fail(`${name}: integer wire values need an explicit maximum`);
    }
  }
}

async function readSourceTree(directory) {
  const contents = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) contents.push(await readSourceTree(path));
    else if (/\.(?:ts|tsx|mjs)$/.test(entry.name)) contents.push(await readFile(path, "utf8"));
  }
  return contents.join("\n");
}

async function sourceInventory() {
  const frontendSource = await readSourceTree(resolve(root, "apps/desktop/src"));
  const [daemon, daemonDispatch, tauri, cli] = await Promise.all([
    readFile(resolve(root, "crates/tendi-daemon/src/lib.rs"), "utf8"),
    readFile(resolve(root, "crates/tendi-daemon/src/generated/runtime_dispatch.rs"), "utf8").catch(() => ""),
    readFile(resolve(root, "apps/desktop/src-tauri/src/lib.rs"), "utf8"),
    readFile(resolve(root, "crates/tendi-cli/src/main.rs"), "utf8"),
  ]);
  {
    const daemonMethods = new Set([
      ...[...daemon.matchAll(/"([a-z][a-z0-9_]*)"\s*=>\s*self\./g)].map((match) => match[1]),
      ...[...daemonDispatch.matchAll(/"([a-z][a-z0-9_]*)"\s*=>\s*\{(?=[\s\S]{0,2500}?\$daemon\b)/g)].map((match) => match[1]),
    ]);
    const tauriCommands = new Set([...tauri.matchAll(/#\[tauri::command[^\]]*\]\s*(?:pub\s+)?(?:async\s+)?fn\s+([a-z][a-z0-9_]*)/g)].map((match) => match[1]));
    const transportNames = new Set(schema["x-tendi"].transports.map((transport) => transport.name));
    const cliMethods = new Set([
      ...[...cli.matchAll(/try_daemon_command\(\s*&cwd,\s*"([a-z][a-z0-9_]*)"/g)].map((match) => match[1]),
      ...[...cli.matchAll(/try_daemon_command\(\s*&cwd,\s*CommandName::([A-Za-z0-9_]+)/g)]
        .map((match) => methods.find((method) => pascalCase(method.name) === match[1])?.name)
        .filter(Boolean),
      ...[...cli.matchAll(/runtime_client\.([a-z][a-z0-9_]*)\s*\(/g)].map((match) => match[1]),
      ...[...cli.matchAll(/try_daemon_json!\(\s*&cwd\s*,\s*([a-z][a-z0-9_]*)\s*,/gs)].map((match) => match[1]),
    ]);
    const frontendMethods = new Set([
      ...[...frontendSource.matchAll(/TauriCommand\.([A-Z][A-Za-z0-9_]*)/g)]
        .map((match) => methods.find((method) => pascalCase(method.name) === match[1])?.name)
        .filter(Boolean),
    ]);
    const daemonEvents = new Set([
      ...[...daemon.matchAll(/pub const [A-Z0-9_]+EVENT:\s*&str\s*=\s*"([^"]+)"/g)].map((match) => match[1]),
      ...[...daemon.matchAll(/pub const [A-Z0-9_]+EVENT:\s*&str\s*=\s*runtime_schema::EventName::([A-Za-z0-9_]+)\.as_str\(\)/g)]
        .map((match) => events.find((event) => pascalCase(event.name) === match[1])?.name)
        .filter(Boolean),
    ]);
    const schemaMethods = new Set(methods.map((method) => method.name));
    const schemaEvents = new Set(events.map((event) => event.name));

    for (const name of daemonMethods) assert(schemaMethods.has(name), `daemon dispatch method ${name} is missing from schema`);
    for (const name of tauriCommands) assert(schemaMethods.has(name) || transportNames.has(name), `Tauri command ${name} is missing from schema or transport metadata`);
    for (const name of cliMethods) assert(schemaMethods.has(name), `CLI daemon method ${name} is missing from schema`);
    for (const name of frontendMethods) assert(schemaMethods.has(name), `frontend command ${name} is missing from schema`);
    for (const name of daemonEvents) assert(schemaEvents.has(name), `daemon event ${name} is missing from schema`);

    const daemonSchemaMethods = new Set(methods.filter((method) => method["x-tendi"].owner === "daemon").map((method) => method.name));
    const desktopSchemaMethods = new Set(methods.filter((method) => method["x-tendi"].owner === "desktop").map((method) => method.name));
    if (daemonMethods.size > 0) assert(setEqual(daemonMethods, daemonSchemaMethods), `daemon/schema mismatch: ${difference(daemonMethods, daemonSchemaMethods).join(", ")} ${difference(daemonSchemaMethods, daemonMethods).join(", ")}`);
    assert(setEqual(tauriCommands, new Set([...desktopSchemaMethods, ...transportNames])), `Tauri/schema mismatch: ${difference(tauriCommands, new Set([...desktopSchemaMethods, ...transportNames])).join(", ")} ${difference(new Set([...desktopSchemaMethods, ...transportNames]), tauriCommands).join(", ")}`);
    return { daemonMethods, tauriCommands, cliMethods, frontendMethods, daemonEvents };
  }
}

function setEqual(left, right) {
  return left.size === right.size && [...left].every((item) => right.has(item));
}

function difference(left, right) {
  return [...left].filter((item) => !right.has(item));
}

function pascalCase(name) {
  return name.split(/[^a-zA-Z0-9]+/).filter(Boolean).map((part) => part.slice(0, 1).toUpperCase() + part.slice(1)).join("");
}

function tsType(value) {
  const name = refName(value);
  if (name) return name;
  const resolved = resolveSchema(value);
  if (resolved.const !== undefined) return JSON.stringify(resolved.const);
  if (Array.isArray(resolved.enum)) return resolved.enum.map((item) => JSON.stringify(item)).join(" | ");
  if (Array.isArray(resolved.oneOf) || Array.isArray(resolved.anyOf)) {
    return (resolved.oneOf ?? resolved.anyOf).map((part) => tsType(part)).join(" | ");
  }
  if (Array.isArray(resolved.type)) {
    return resolved.type.map((type) => type === "null"
      ? "null"
      : type === "array"
        ? `${tsType(resolved.items)}[]`
        : primitiveTsType(type)).join(" | ");
  }
  if (resolved.type === "array") return `${tsType(resolved.items)}[]`;
  if (resolved.type === "object") {
    if (resolved.additionalProperties && typeof resolved.additionalProperties === "object") {
      return `Record<string, ${tsType(resolved.additionalProperties)}>`;
    }
    return "JsonObject";
  }
  return primitiveTsType(resolved.type);
}

function primitiveTsType(type) {
  return ({ string: "string", integer: "number", number: "number", boolean: "boolean", null: "null", object: "JsonObject", array: "JsonValue[]" })[type] ?? "JsonValue";
}

function rustType(value) {
  const name = refName(value);
  if (name === "JsonValue" || name === "JsonRpcId") return "JsonValue";
  if (name === "JsonObject") return "JsonObject";
  if (name) return name;
  const resolved = resolveSchema(value);
  if (resolved.const !== undefined) {
    if (typeof resolved.const === "string") return "String";
    if (typeof resolved.const === "boolean") return "bool";
    if (Number.isInteger(resolved.const)) return "u64";
    if (typeof resolved.const === "number") return "f64";
  }
  if (Array.isArray(resolved.oneOf) || Array.isArray(resolved.anyOf)) return "JsonValue";
  if (Array.isArray(resolved.type)) {
    if (resolved.type.includes("null") && resolved.type.length === 2) {
      const nonNull = resolved.type.find((type) => type !== "null");
      return `Option<${rustType({ ...resolved, type: nonNull })}>`;
    }
    return "JsonValue";
  }
  if (resolved.type === "array") return `Vec<${rustType(resolved.items)}>`;
  if (resolved.type === "object") {
    if (resolved.additionalProperties && typeof resolved.additionalProperties === "object") {
      return `std::collections::BTreeMap<String, ${rustType(resolved.additionalProperties)}>`;
    }
    return "JsonObject";
  }
  return rustPrimitiveType(resolved.type);
}

function rustPrimitiveType(type) {
  return ({ string: "String", integer: "u64", number: "f64", boolean: "bool", null: "()" })[type] ?? "JsonValue";
}

const builtinComponentNames = new Set([
  "JsonValue",
  "JsonObject",
  "EmptyRequest",
  "Unit",
  "Revision",
  "RuntimeEventEnvelope",
  "JsonRpcId",
  "JsonRpcErrorData",
  "JsonRpcError",
  "JsonRpcRequest",
  "JsonRpcResponse",
]);

function tsPropertyName(name) {
  return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name) ? name : JSON.stringify(name);
}

function tsComponent(name, rawSchema) {
  const schema = resolveSchema(rawSchema);
  if (Array.isArray(schema.enum)) return "export type " + name + " = " + schema.enum.map((value) => JSON.stringify(value)).join(" | ") + ";";
  if (Array.isArray(schema.oneOf) || Array.isArray(schema.anyOf)) {
    return "export type " + name + " = " + (schema.oneOf ?? schema.anyOf).map((part) => tsType(part)).join(" | ") + ";";
  }
  if (schema.type !== "object") return "export type " + name + " = " + tsType(rawSchema) + ";";
  const properties = Object.entries(schema.properties ?? {}).map(([property, value]) => {
    const required = (schema.required ?? []).includes(property);
    return "  " + tsPropertyName(property) + (required ? "" : "?") + ": " + tsType(value) + ";";
  });
  if (schema.additionalProperties === true) properties.push("  [key: string]: JsonValue | undefined;");
  else if (schema.additionalProperties && typeof schema.additionalProperties === "object") properties.push("  [key: string]: " + tsType(schema.additionalProperties) + ";");
  return "export type " + name + " = {\n" + properties.join("\n") + "\n};";
}

function rustFieldName(name) {
  return name.replace(/([a-z0-9])([A-Z])/g, "$1_$2").replace(/[^A-Za-z0-9_]/g, "_").toLowerCase();
}

function rustComponent(name, rawSchema) {
  const schema = resolveSchema(rawSchema);
  if (Array.isArray(schema.enum)) {
    const variants = schema.enum.map((value) => {
      const variant = pascalCase(String(value)) || "Unknown";
      return "    #[serde(rename = " + JSON.stringify(String(value)) + ")]\n    " + variant + ",";
    }).join("\n");
    return "#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]\npub enum " + name + " {\n" + variants + "\n}";
  }
  if (Array.isArray(schema.oneOf) || Array.isArray(schema.anyOf)) {
    const variants = (schema.oneOf ?? schema.anyOf).map((part, index) =>
      "    Variant" + index + "(" + rustType(part) + "),").join("\n");
    return "#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\n#[serde(untagged)]\npub enum " + name + " {\n" + variants + "\n}";
  }
  if (schema.type !== "object") return "pub type " + name + " = " + rustType(rawSchema) + ";";
  const required = new Set(schema.required ?? []);
  const fields = Object.entries(schema.properties ?? {}).map(([property, value]) => {
    const field = rustFieldName(property);
    const type = rustType(value);
    const rename = field === property ? "" : "    #[serde(rename = " + JSON.stringify(property) + ")]\n";
    if (required.has(property)) return rename + "    pub " + field + ": " + type + ",";
    if (type.startsWith("Option<")) return rename + "    #[serde(skip_serializing_if = \"Option::is_none\")]\n    pub " + field + ": " + type + ",";
    return rename + "    #[serde(skip_serializing_if = \"Option::is_none\")]\n    pub " + field + ": Option<" + type + ">,";
  });
  if (schema.additionalProperties === true) fields.push("    #[serde(flatten)]\n    pub extra: JsonObject,");
  else if (schema.additionalProperties && typeof schema.additionalProperties === "object") fields.push("    #[serde(flatten)]\n    pub extra: std::collections::BTreeMap<String, " + rustType(schema.additionalProperties) + ">,");
  return "#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\npub struct " + name + " {\n" + fields.join("\n") + "\n}";
}

function generatedComponentDefinitions(language) {
  return Object.entries(components)
    .filter(([name]) => !builtinComponentNames.has(name))
    .map(([name, schema]) => language === "ts" ? tsComponent(name, schema) : rustComponent(name, schema))
    .join("\n");
}

function generateTypes() {
  const commandNames = methods.map((method) => method.name);
  const commandObject = methods.map((method) => `  ${pascalCase(method.name)}: ${JSON.stringify(method.name)},`).join("\n");
  const requestMap = methods.map((method) => `  ${JSON.stringify(method.name)}: ${pascalCase(method.name)}Request;`).join("\n");
  const responseMap = methods.map((method) => `  ${JSON.stringify(method.name)}: ${pascalCase(method.name)}Response;`).join("\n");
  const dtoAliases = methods.map((method) => {
    const variant = pascalCase(method.name);
    const requestType = method.params.length === 0 ? "EmptyRequest" : tsType(method.params[0].schema);
    const responseType = tsType(method.result.schema);
    return [
      requestType !== `${variant}Request` ? `export type ${variant}Request = ${requestType};` : "",
      responseType !== `${variant}Response` ? `export type ${variant}Response = ${responseType};` : "",
    ].filter(Boolean).join("\n");
  }).join("\n");
  const metadata = methods.map((method) => {
    const value = method["x-tendi"];
    const serializedWrite = value.execution === "write" || Boolean(value.serializedWrite);
    return `  ${JSON.stringify(method.name)}: { owner: ${JSON.stringify(value.owner)}, wire: ${JSON.stringify(value.wire)}, clients: ${JSON.stringify(value.clients)}, execution: ${JSON.stringify(value.execution)}, serializedWrite: ${serializedWrite}, internal: ${Boolean(value.internal)}, deprecated: ${Boolean(value.deprecated)} },`;
  }).join("\n");
  return `// DO NOT EDIT. Generated by scripts/runtime-codegen.mjs from runtime-schema/runtime.openrpc.json.
export type JsonPrimitive = string | number | boolean | null;
export type JsonValue = JsonPrimitive | JsonObject | JsonValue[];
export type JsonObject = { [key: string]: JsonValue };
export type EmptyRequest = Record<string, never>;
export type Unit = Record<string, never> | null;
export type ScopeKey = string;
export type Revision = number;

${generatedComponentDefinitions("ts")}

export type JsonRpcId = string | number | null;
export type JsonRpcRequest = { jsonrpc: "2.0"; id: JsonRpcId; method: CommandName; params: JsonObject };
export type JsonRpcErrorData = { kind: string; details?: JsonValue };
export type JsonRpcError = { code: number; message: string; data?: JsonRpcErrorData };
export type JsonRpcResponse = { jsonrpc: "2.0"; id: JsonRpcId; result?: JsonValue; error?: JsonRpcError };
export type RuntimeEventEnvelope = { id: number; event: string; payload: JsonValue; scopeKey?: string; domain?: string; operationId?: string; baseRevision?: number; revision?: number; sourceVersion?: string | null };

export type CommandName = ${commandNames.map(JSON.stringify).join(" | ")};
export const TauriCommand = {
${commandObject}
} as const satisfies Record<string, CommandName>;

export type RequestFor<C extends CommandName> = RuntimeRequests[C];
export type ResponseFor<C extends CommandName> = RuntimeResponses[C];
${dtoAliases}
export type RuntimeRequests = {
${requestMap}
};
export type RuntimeResponses = {
${responseMap}
};

export type CommandMetadata = { owner: "daemon" | "desktop"; wire: "jsonrpc" | "tauri"; clients: readonly string[]; execution: "read" | "write"; serializedWrite: boolean; internal: boolean; deprecated: boolean };
export const COMMAND_METADATA = {
${metadata}
} as const satisfies Record<CommandName, CommandMetadata>;

export function isDaemonCommand(command: string): command is CommandName {
  return typeof command === "string" && Object.hasOwn(COMMAND_METADATA, command) && COMMAND_METADATA[command as CommandName].owner === "daemon";
}
export function isDesktopCommand(command: string): command is CommandName {
  return typeof command === "string" && Object.hasOwn(COMMAND_METADATA, command) && COMMAND_METADATA[command as CommandName].owner === "desktop";
}
export const PROTOCOL_VERSION = 2;
export const SCHEMA_VERSION = 1;
export const RUNTIME_ERROR_CODES = ${JSON.stringify(Object.fromEntries(errors.map((error) => [error.kind, error.code])))} as const;
`;
}

function generateValidators() {
  const generatedSchemas = JSON.stringify(components, null, 2);
  const requestSchemas = JSON.stringify(Object.fromEntries(methods.map((method) => [method.name, method.params.length === 0 ? "EmptyRequest" : refName(method.params[0].schema) ?? "JsonObject"])), null, 2);
  const resultSchemas = JSON.stringify(Object.fromEntries(methods.map((method) => [method.name, refName(method.result.schema) ?? "JsonValue"])), null, 2);
  const eventSchemas = JSON.stringify(Object.fromEntries(events.map((event) => [event.name, refName(event.payload) ?? "JsonValue"])), null, 2);
  return `// DO NOT EDIT. Generated by scripts/runtime-codegen.mjs.
import Ajv2020 from "ajv/dist/2020.js";
import type { CommandName, JsonObject, JsonRpcResponse, JsonValue, RuntimeEventEnvelope } from "./runtime-types.ts";

const SCHEMAS: Record<string, Record<string, unknown>> = ${generatedSchemas};
const REQUEST_SCHEMAS: Record<CommandName, string> = ${requestSchemas};
const RESULT_SCHEMAS: Record<CommandName, string> = ${resultSchemas};
const EVENT_SCHEMAS: Record<string, string> = ${eventSchemas};
const ajv = new Ajv2020({ allErrors: true, strict: false });
ajv.addSchema({ $id: "tendi-runtime-contract", components: { schemas: SCHEMAS } });

export class RuntimeContractError extends Error {
  readonly operation: string;
  readonly path: string;
  readonly schemaVersion = 1;
  constructor(operation: string, path: string, message: string) {
    super(operation + " at " + path + ": " + message + " (schema v1)");
    this.name = "RuntimeContractError";
    this.operation = operation;
    this.path = path;
  }
}

function isJsonValue(value: unknown): boolean {
  if (value === null || typeof value === "string" || typeof value === "boolean") return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) return value.every(isJsonValue);
  return Boolean(value && typeof value === "object" && !Array.isArray(value)) && Object.values(value as Record<string, unknown>).every(isJsonValue);
}
function assertJson(value: unknown, operation: string): asserts value is JsonValue {
  if (!isJsonValue(value)) throw new RuntimeContractError(operation, "$", "value is not valid JSON");
}
function assertJsonObject(value: unknown, operation: string): asserts value is JsonObject {
  assertJson(value, operation);
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new RuntimeContractError(operation, "$", "value must be an object");
  }
}
function validateSchema(operation: string, schemaName: string, value: unknown): void {
  const validator = ajv.getSchema("tendi-runtime-contract#/components/schemas/" + schemaName);
  if (!validator) throw new RuntimeContractError(operation, "$", "unknown schema " + schemaName);
  if (validator(value)) return;
  const error = validator.errors?.[0];
  const path = error?.instancePath || "$";
  const message = error?.message || "schema validation failed";
  throw new RuntimeContractError(operation, path, message);
}
export function validateRequest<C extends CommandName>(command: C, value: unknown): asserts value is JsonObject {
  assertJsonObject(value, command);
  validateSchema(command, REQUEST_SCHEMAS[command], value);
}
export function validateResult<C extends CommandName>(command: C, value: unknown): asserts value is JsonRpcResponse["result"] {
  assertJson(value, command);
  validateSchema(command, RESULT_SCHEMAS[command], value);
}
export function validateResponse(value: unknown): asserts value is JsonRpcResponse {
  assertJsonObject(value, "response");
  validateSchema("response", "JsonRpcResponse", value);
}
export function validateEvent(value: unknown): asserts value is RuntimeEventEnvelope {
  assertJsonObject(value, "event");
  validateSchema("event", "RuntimeEventEnvelope", value);
  const eventName = (value as RuntimeEventEnvelope).event;
  const eventSchema = EVENT_SCHEMAS[eventName];
  if (!eventSchema) throw new RuntimeContractError("event", "$.event", "unknown event");
  validateSchema("event " + eventName, eventSchema, (value as RuntimeEventEnvelope).payload);
}
export function validateEventPayload(event: string, value: unknown): void {
  assertJson(value, "event " + event);
  const eventSchema = EVENT_SCHEMAS[event];
  if (!eventSchema) throw new RuntimeContractError("event " + event, "$", "unknown event");
  validateSchema("event " + event, eventSchema, value);
}
`;
}

function generateEvents() {
  const eventNames = events.map((event) => event.name);
  const payloadTypes = [...new Set(events.map((event) => tsType(event.payload)))]
    .filter((type) => /^[A-Z][A-Za-z0-9_]*$/.test(type));
  const payloadImport = payloadTypes.length > 0
    ? `import type { ${payloadTypes.join(", ")} } from "./runtime-types.ts";\n`
    : "";
  const eventObject = events.map((event) => `  ${pascalCase(event.name)}: ${JSON.stringify(event.name)},`).join("\n");
  const union = events.map((event) => `  | { event: ${JSON.stringify(event.name)}; payload: ${tsType(event.payload)}; id: number; scopeKey?: string; domain?: string; operationId?: string; baseRevision?: number; revision?: number; sourceVersion?: string | null }`).join("\n");
  const metadata = events.map((event) => `  ${JSON.stringify(event.name)}: { wire: ${JSON.stringify(event.wire)}, revisioned: ${event.revisioned}, replay: ${event.replay}, clients: ${JSON.stringify(event.clients)} },`).join("\n");
  return `// DO NOT EDIT. Generated by scripts/runtime-codegen.mjs.
import { validateEvent, validateEventPayload } from "./runtime-validators.ts";
${payloadImport}
export type EventName = ${eventNames.map(JSON.stringify).join(" | ")};
export const RuntimeEventName = {
${eventObject}
} as const satisfies Record<string, EventName>;
export type RuntimeEvent =
${union};
export const EVENT_METADATA = {
${metadata}
} as const satisfies Record<EventName, { wire: "sse" | "tauri"; revisioned: boolean; replay: boolean; clients: readonly string[] }>;
export function assertRuntimeEvent(value: unknown): asserts value is RuntimeEvent {
  validateEvent(value);
}
export function assertRuntimeEventPayload<E extends EventName>(event: E, value: unknown): asserts value is Extract<RuntimeEvent, { event: E }>["payload"] {
  validateEventPayload(event, value);
}
`;
}

function generateClient() {
  const clientMethods = methods.map((method) => {
    const name = method.name.replace(/_([a-z])/g, (_, letter) => letter.toUpperCase());
    return `  async ${name}(request: RequestFor<${JSON.stringify(method.name)}>): Promise<ResponseFor<${JSON.stringify(method.name)}>> { return this.call(${JSON.stringify(method.name)}, request); }`;
  }).join("\n");
  return `// DO NOT EDIT. Generated by scripts/runtime-codegen.mjs.
import { COMMAND_METADATA, type CommandName, type JsonRpcId, type JsonRpcRequest, type JsonRpcResponse, type RequestFor, type ResponseFor, type RuntimeRequests } from "./runtime-types.ts";
import { RuntimeContractError, validateRequest, validateResponse, validateResult } from "./runtime-validators.ts";

export interface RuntimeTransport {
  request(value: JsonRpcRequest): Promise<JsonRpcResponse>;
}
export class RuntimeRemoteError extends Error {
  readonly code: number;
  readonly kind: string | undefined;
  constructor(code: number, message: string, kind?: string) { super(message); this.name = "RuntimeRemoteError"; this.code = code; this.kind = kind; }
}
export class RuntimeClient {
  private nextId = 0;
  private readonly transport: RuntimeTransport;
  constructor(transport: RuntimeTransport) { this.transport = transport; }
  async call<C extends CommandName>(command: C, request: RequestFor<C>): Promise<ResponseFor<C>> {
    validateRequest(command, request);
    const envelope: JsonRpcRequest = { jsonrpc: "2.0", id: "desktop-" + ++this.nextId as JsonRpcId, method: command, params: request };
    const response = await this.transport.request(envelope);
    validateResponse(response);
    if (response.jsonrpc !== "2.0" || response.id !== envelope.id) throw new RuntimeContractError(command, "$", "invalid JSON-RPC response envelope");
    if (response.error) throw new RuntimeRemoteError(response.error.code, response.error.message, response.error.data?.kind);
    if (!("result" in response)) throw new RuntimeContractError(command, "$.result", "result is missing");
    validateResult(command, response.result);
    return response.result as ResponseFor<C>;
  }
${clientMethods}
  capabilities(): readonly CommandName[] { return Object.keys(COMMAND_METADATA) as CommandName[]; }
}
export type RuntimeRequestMap = RuntimeRequests;
`;
}

function generateRust() {
  const commandVariants = methods.map((method) => `    ${pascalCase(method.name)},`).join("\n");
  const requestTypes = methods.map((method) => `${pascalCase(method.name)}Request`);
  const responseTypes = methods.map((method) => `${pascalCase(method.name)}Response`);
  const dtoAliases = methods.map((method) => {
    const variant = pascalCase(method.name);
    const requestType = method.params.length === 0 ? "EmptyRequest" : rustType(method.params[0].schema);
    const responseType = rustType(method.result.schema);
    const requestDefinition = requestType === `${variant}Request` ? "" : requestType === "JsonObject"
      ? `pub type ${variant}Request = JsonObject;`
      : `pub type ${variant}Request = ${requestType};`;
    const responseDefinition = responseType === `${variant}Response` ? "" : responseType === "JsonValue"
      ? `pub type ${variant}Response = JsonValue;`
      : `pub type ${variant}Response = ${responseType};`;
    return [requestDefinition, responseDefinition].filter(Boolean).join("\n");
  }).join("\n");
  const requestVariants = methods.map((method, index) => `    #[serde(rename = "${method.name}")]\n    ${pascalCase(method.name)}(${requestTypes[index]}),`).join("\n");
  const resultVariants = methods.map((method, index) => `    #[serde(rename = "${method.name}")]\n    ${pascalCase(method.name)}(${responseTypes[index]}),`).join("\n");
  const commandMatches = methods.map((method) => `            "${method.name}" => Some(Self::${pascalCase(method.name)}),`).join("\n");
  const metadata = methods.map((method) => {
    const value = method["x-tendi"];
    const serialized = value.execution === "write" || Boolean(value.serializedWrite);
    return `        "${method.name}" => Some(CommandMetadata { name: "${method.name}", owner: Owner::${pascalCase(value.owner)}, execution: Execution::${pascalCase(value.execution)}, serialized_write: ${serialized}, requires_params: ${method.params.length > 0}, internal: ${Boolean(value.internal)}, deprecated: ${Boolean(value.deprecated)} }),`;
  }).join("\n");
  const requestValidation = methods
    .filter((method) => method.params.length > 0 && !["JsonObject", "JsonValue"].includes(rustType(method.params[0].schema)))
    .map((method) => `        "${method.name}" => {
            serde_json::from_value::<${rustType(method.params[0].schema)}>(args.clone())
                .map_err(|error| format!("request is invalid: {error}"))?;
        },`)
    .join("\n");
  const resultValidation = methods
    .filter((method) => method.name !== "sessions_snapshot" && !["JsonObject", "JsonValue", "Unit"].includes(rustType(method.result.schema)))
    .map((method) => `        "${method.name}" => {
            serde_json::from_value::<${rustType(method.result.schema)}>(value.clone())
                .map_err(|error| format!("result is invalid: {error}"))?;
            Ok(())
        },`)
    .join("\n");
  const eventVariants = events.map((event) => `    ${pascalCase(event.name.replaceAll("/", " ").replaceAll(":", " ").replaceAll("-", " "))},`).join("\n");
  const eventPayloadVariants = events.map((event) => {
    const variant = pascalCase(event.name.replaceAll("/", " ").replaceAll(":", " ").replaceAll("-", " "));
    return `    ${variant}(${rustType(event.payload)}),`;
  }).join("\n");
  const eventPayloadNames = events.map((event) => {
    const variant = pascalCase(event.name.replaceAll("/", " ").replaceAll(":", " ").replaceAll("-", " "));
    return `            Self::${variant}(_) => "${event.name}",`;
  }).join("\n");
  const eventPayloadValues = events.map((event) => {
    const variant = pascalCase(event.name.replaceAll("/", " ").replaceAll(":", " ").replaceAll("-", " "));
    return `            Self::${variant}(payload) => serde_json::to_value(payload).expect("generated event payload serializes"),`;
  }).join("\n");
  const eventValidation = events.map((event) => {
    const eventType = rustType(event.payload);
    return `        "%%EVENT_NAME%%" => {
            serde_json::from_value::<%%EVENT_TYPE%%>(Value::Object(event.payload.clone()))
                .map_err(|error| format!("event payload is invalid: {error}"))?;
        },`
      .replace("%%EVENT_NAME%%", event.name)
      .replace("%%EVENT_TYPE%%", eventType);
  }).join("\n");
  const rustSchemaValidator = `const SCHEMA_DOCUMENT: &str = include_str!("../../../../runtime-schema/runtime.openrpc.json");

fn schema_document() -> &'static JsonValue {
    use std::sync::OnceLock;
    static DOCUMENT: OnceLock<JsonValue> = OnceLock::new();
    DOCUMENT.get_or_init(|| serde_json::from_str(SCHEMA_DOCUMENT).expect("runtime schema is valid JSON"))
}

fn component_schema(name: &str) -> Option<&'static JsonValue> {
    schema_document().get("components")?.get("schemas")?.get(name)
}

fn method_schema(name: &str, result: bool) -> Option<&'static JsonValue> {
    let method = schema_document().get("methods")?.as_array()?.iter().find(|method| {
        method.get("name").and_then(JsonValue::as_str) == Some(name)
    })?;
    if result {
        method.get("result")?.get("schema")
    } else {
        method.get("params")?.as_array()?.first()
            .and_then(|param| param.get("schema"))
            .or_else(|| component_schema("EmptyRequest"))
    }
}

fn event_schema(name: &str) -> Option<&'static JsonValue> {
    schema_document().get("x-tendi")?.get("events")?.as_array()?.iter().find(|event| {
        event.get("name").and_then(JsonValue::as_str) == Some(name)
    }).and_then(|event| event.get("payload"))
}

fn resolve_schema<'a>(schema: &'a JsonValue, root: &'a JsonValue) -> Result<&'a JsonValue, String> {
    let Some(reference) = schema.get("$ref").and_then(JsonValue::as_str) else { return Ok(schema); };
    let mut current = root;
    for part in reference.strip_prefix("#/").ok_or_else(|| format!("unsupported schema reference {reference}"))?.split('/') {
        current = current.get(part).ok_or_else(|| format!("schema reference not found: {reference}"))?;
    }
    Ok(current)
}

fn value_type(value: &JsonValue) -> &'static str {
    if value.is_null() { "null" }
    else if value.is_boolean() { "boolean" }
    else if value.is_string() { "string" }
    else if value.is_array() { "array" }
    else if value.is_object() { "object" }
    else if value.as_i64().is_some() || value.as_u64().is_some() { "integer" }
    else { "number" }
}

fn type_matches(value: &JsonValue, type_name: &str) -> bool {
    match type_name {
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        other => value_type(value) == other,
    }
}

fn validate_json_schema(value: &JsonValue, raw_schema: &JsonValue, root: &JsonValue, path: &str) -> Result<(), String> {
    let schema = resolve_schema(raw_schema, root)?;
    if let Some(any_of) = schema.get("anyOf").or_else(|| schema.get("oneOf")).and_then(JsonValue::as_array) {
        let matches = any_of.iter().filter(|candidate| validate_json_schema(value, candidate, root, path).is_ok()).count();
        let required_matches = schema.get("oneOf").is_some();
        if (required_matches && matches != 1) || (!required_matches && matches == 0) {
            return Err(format!("{path} does not match the declared schema"));
        }
        return Ok(());
    }
    if let Some(expected) = schema.get("const") {
        if value != expected { return Err(format!("{path} must equal {expected}")); }
    }
    if let Some(values) = schema.get("enum").and_then(JsonValue::as_array) {
        if !values.iter().any(|expected| expected == value) {
            return Err(format!("{path} has an invalid enum value"));
        }
    }
    if let Some(types) = schema.get("type") {
        let matches = types.as_array()
            .map(|values| values.iter().filter_map(JsonValue::as_str).any(|expected| type_matches(value, expected)))
            .or_else(|| types.as_str().map(|expected| type_matches(value, expected)))
            .unwrap_or(true);
        if !matches { return Err(format!("{path} must be {}, got {}", types, value_type(value))); }
    }
    if let Some(minimum) = schema.get("minimum").and_then(JsonValue::as_f64) {
        if value.as_f64().is_none_or(|actual| actual < minimum) { return Err(format!("{path} is below minimum {minimum}")); }
    }
    if let Some(maximum) = schema.get("maximum").and_then(JsonValue::as_f64) {
        if value.as_f64().is_none_or(|actual| actual > maximum) { return Err(format!("{path} exceeds maximum {maximum}")); }
    }
    if let Some(min_length) = schema.get("minLength").and_then(JsonValue::as_u64) {
        if value.as_str().is_none_or(|actual| actual.chars().count() < min_length as usize) { return Err(format!("{path} is shorter than {min_length}")); }
    }
    if let Some(max_length) = schema.get("maxLength").and_then(JsonValue::as_u64) {
        if value.as_str().is_none_or(|actual| actual.chars().count() > max_length as usize) { return Err(format!("{path} exceeds length {max_length}")); }
    }
    if let Some(items) = schema.get("items") {
        if let Some(values) = value.as_array() {
            for (index, item) in values.iter().enumerate() {
                validate_json_schema(item, items, root, &format!("{path}[{index}]"))?;
            }
        }
    }
    if let Some(object) = value.as_object() {
        let properties = schema.get("properties").and_then(JsonValue::as_object);
        if let Some(required) = schema.get("required").and_then(JsonValue::as_array) {
            for field in required.iter().filter_map(JsonValue::as_str) {
                if !object.contains_key(field) { return Err(format!("{path}.{field} is required")); }
            }
        }
        for (field, field_schema) in properties.into_iter().flat_map(|properties| properties.iter()) {
            if let Some(field_value) = object.get(field) {
                validate_json_schema(field_value, field_schema, root, &format!("{path}.{field}"))?;
            }
        }
        if schema.get("additionalProperties") == Some(&JsonValue::Bool(false)) {
            for field in object.keys() {
                if !properties.is_some_and(|properties| properties.contains_key(field)) {
                    return Err(format!("{path}.{field} is not allowed"));
                }
            }
        } else if let Some(additional) = schema.get("additionalProperties").filter(|value| value.is_object()) {
            for (field, field_value) in object {
                if !properties.is_some_and(|properties| properties.contains_key(field)) {
                    validate_json_schema(field_value, additional, root, &format!("{path}.{field}"))?;
                }
            }
        }
    }
    Ok(())
}

fn validate_method_schema(name: &str, result: bool, value: &JsonValue) -> Result<(), String> {
    let schema = method_schema(name, result).ok_or_else(|| format!("schema is missing for {name}"))?;
    validate_json_schema(value, schema, schema_document(), if result { "$.result" } else { "$.params" })
}`;
  return `// DO NOT EDIT. Generated by scripts/runtime-codegen.mjs.
use serde::{Deserialize, Serialize};
pub use serde_json::Value;

pub const PROTOCOL_VERSION: u64 = 2;
pub const SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Owner { Daemon, Desktop }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Execution { Read, Write }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandMetadata { pub name: &'static str, pub owner: Owner, pub execution: Execution, pub serialized_write: bool, pub requires_params: bool, pub internal: bool, pub deprecated: bool }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest { pub jsonrpc: String, pub id: Value, pub method: String, pub params: Value }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcErrorData { pub kind: String, #[serde(skip_serializing_if = "Option::is_none")] pub details: Option<Value> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError { pub code: i32, pub message: String, #[serde(skip_serializing_if = "Option::is_none")] pub data: Option<JsonRpcErrorData> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse { pub jsonrpc: String, pub id: Value, #[serde(skip_serializing_if = "Option::is_none")] pub result: Option<Value>, #[serde(skip_serializing_if = "Option::is_none")] pub error: Option<JsonRpcError> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEventEnvelope { pub id: u64, pub event: String, pub payload: JsonObject, #[serde(skip_serializing_if = "Option::is_none")] pub scope_key: Option<String>, #[serde(skip_serializing_if = "Option::is_none")] pub domain: Option<String>, #[serde(skip_serializing_if = "Option::is_none")] pub operation_id: Option<String>, #[serde(skip_serializing_if = "Option::is_none")] pub base_revision: Option<u64>, #[serde(skip_serializing_if = "Option::is_none")] pub revision: Option<u64>, #[serde(skip_serializing_if = "Option::is_none")] pub source_version: Option<String> }
pub type JsonObject = serde_json::Map<String, serde_json::Value>;
pub type JsonValue = serde_json::Value;
pub type Unit = Option<()>;
pub type Revision = u64;
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EmptyRequest {}
${generatedComponentDefinitions("rust")}
${rustSchemaValidator}
${dtoAliases}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum CommandRequest {
${requestVariants}
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "result")]
pub enum CommandResult {
${resultVariants}
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandName {
${commandVariants}
}
impl CommandName {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
${commandMatches}
            _ => None,
        }
    }
    pub const fn as_str(self) -> &'static str {
        match self { ${methods.map((method) => `Self::${pascalCase(method.name)} => "${method.name}"`).join(", ")} }
    }
}
pub fn command_metadata(name: &str) -> Option<CommandMetadata> {
    match name {
${metadata}
        _ => None,
    }
}
pub fn command_requires_serialized_write(name: &str, args: &Value) -> bool {
    if name == "analytics_overview" { return args.get("refreshTranscripts").and_then(Value::as_bool).unwrap_or(false); }
    command_metadata(name).is_some_and(|metadata| metadata.serialized_write)
}
pub fn error_code(kind: &str) -> i32 {
    match kind {
${errors.map((error) => `        "${error.kind}" => ${error.code},`).join("\n")}
        _ => -32001,
    }
}
pub fn validate_request(name: &str, args: &Value) -> Result<(), String> {
    let Some(metadata) = command_metadata(name) else { return Err("unknown method".to_string()); };
    let Some(object) = args.as_object() else { return Err("params must be an object".to_string()); };
    if !metadata.requires_params && !object.is_empty() { return Err("method does not accept params".to_string()); }
    validate_method_schema(name, false, args)?;
    match name {
${requestValidation}
        _ => {}
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventName {
${eventVariants}
}
pub enum RuntimeEventPayload {
${eventPayloadVariants}
}
impl RuntimeEventPayload {
    pub const fn event_name(&self) -> &'static str {
        match self {
${eventPayloadNames}
        }
    }
    pub fn into_json(self) -> JsonValue {
        match self {
${eventPayloadValues}
        }
    }
}
impl EventName {
    pub fn parse(value: &str) -> Option<Self> {
        match value { ${events.map((event) => `"${event.name}" => Some(Self::${pascalCase(event.name)}),`).join(" ")} _ => None }
    }
    pub const fn as_str(self) -> &'static str {
        match self { ${events.map((event) => `Self::${pascalCase(event.name)} => "${event.name}"`).join(", ")} }
    }
}
pub fn validate_result(name: &str, value: &Value) -> Result<(), String> {
    validate_method_schema(name, true, value)?;
    match name {
${resultValidation}
        "sessions_snapshot" => {
            let object = value.as_object().ok_or_else(|| "result must be an object".to_string())?;
            for field in ["scopeKey", "domain", "revision", "schemaVersion", "snapshotId", "payload"] {
                if !object.contains_key(field) { return Err(format!("result is missing {field}")); }
            }
            if object.get("domain").and_then(Value::as_str) != Some("sessions") { return Err("result.domain must be sessions".to_string()); }
            let revision = object.get("revision").and_then(Value::as_u64).ok_or_else(|| "result.revision must be an integer".to_string())?;
            if revision > 9007199254740991 { return Err("result.revision exceeds JavaScript safe integer range".to_string()); }
            if object.get("schemaVersion").and_then(Value::as_u64) != Some(1) { return Err("result.schemaVersion must be 1".to_string()); }
            if !object.get("payload").is_some_and(Value::is_array) { return Err("result.payload must be an array".to_string()); }
            Ok(())
        }
        _ => Ok(()),
    }
}
pub fn validate_response(value: &Value) -> Result<(), String> {
    let schema = component_schema("JsonRpcResponse")
        .ok_or_else(|| "schema is missing for JsonRpcResponse".to_string())?;
    validate_json_schema(value, schema, schema_document(), "$")
}
pub fn validate_event(event: &RuntimeEventEnvelope) -> Result<(), String> {
    EventName::parse(&event.event).ok_or_else(|| format!("unknown event {}", event.event))?;
    if event.id > 9007199254740991 { return Err("event.id exceeds JavaScript safe integer range".to_string()); }
    let payload = JsonValue::Object(event.payload.clone());
    let schema = event_schema(&event.event).ok_or_else(|| format!("schema is missing for event {}", event.event))?;
    validate_json_schema(&payload, schema, schema_document(), "$.payload")?;
    match event.event.as_str() {
${eventValidation}
        _ => {}
    }
    Ok(())
}
`;
}

function generateRustMod() {
  return "// DO NOT EDIT. Generated by scripts/runtime-codegen.mjs.\npub mod runtime_contract;\n";
}

function generateDaemonDispatch() {
  const daemonMethods = methods.filter((method) => method["x-tendi"].owner === "daemon");
  const traitMethods = daemonMethods.map((method) => {
    const variant = pascalCase(method.name);
    return `    fn ${method.name}(&self, request: runtime_schema::${variant}Request) -> Result<runtime_schema::${variant}Response, DaemonError>;`;
  }).join("\n");
  const implMethods = daemonMethods.map((method) => {
    const variant = pascalCase(method.name);
    const typedHandler = method["x-tendi"].owner === "daemon" || Boolean(method["x-tendi"].typedHandler);
    const call = typedHandler
      ? method.params.length === 0
        ? `{ let _ = request; Daemon::${method.name}(self) }`
        : `{ Daemon::${method.name}(self, request) }`
      : method.params.length === 0
        ? `{ let _ = request; Daemon::${method.name}(self) }`
        : `{ let args = serde_json::to_value(request).map_err(|error| DaemonError::new("INTERNAL", format!("generated request encode failed: {error}")))?; Daemon::${method.name}(self, &args) }`;
    return `    fn ${method.name}(&self, request: runtime_schema::${variant}Request) -> Result<runtime_schema::${variant}Response, DaemonError> {
        let result = ${call}?;
        let result = serde_json::to_value(result).map_err(|error| DaemonError::new("CONTRACT_VIOLATION", format!("generated response encode failed: {error}")))?;
        serde_json::from_value(result).map_err(|error| DaemonError::new("CONTRACT_VIOLATION", format!("generated response decode failed: {error}")))
    }`;
  }).join("\n");
  const arms = daemonMethods.map((method) => {
    const variant = pascalCase(method.name);
    return `            "${method.name}" => {
                let request: runtime_schema::${variant}Request = serde_json::from_value($args.clone()).map_err(|error| DaemonError::new("INVALID_PARAMS", format!("generated request decode failed: {error}")))?;
                let result = RuntimeHandler::${method.name}($daemon, request)?;
                serde_json::to_value(result).map_err(|error| DaemonError::new("CONTRACT_VIOLATION", format!("generated response encode failed: {error}")))
            },`;
  }).join("\n");
  return `// DO NOT EDIT. Generated by scripts/runtime-codegen.mjs.
pub trait RuntimeHandler {
${traitMethods}
}

impl RuntimeHandler for Daemon {
${implMethods}
}

macro_rules! runtime_dispatch {
    ($daemon:expr, $command:expr, $args:expr) => {
        match $command {
${arms}
            command => Err(DaemonError::new(
                "METHOD_NOT_FOUND",
                format!("unsupported daemon method: {command}"),
            )),
        }
    };
}
`;
}

function generateRustClient() {
  const clientMethods = methods.map((method) => {
    const variant = pascalCase(method.name);
    return `    pub fn ${method.name}(&mut self, params: tendi_core::generated::runtime_contract::${variant}Request) -> JsonRpcRequest { self.request(CommandName::${variant}, params) }
    pub fn decode_${method.name}_response(request_id: &Value, value: Value) -> anyhow::Result<tendi_core::generated::runtime_contract::${variant}Response> {
        let result = Self::decode_response(CommandName::${variant}, request_id, value)?;
        Ok(serde_json::from_value(result)?)
    }`;
  }).join("\n");
  return `// DO NOT EDIT. Generated by scripts/runtime-codegen.mjs.
use serde::Serialize;
use serde_json::Value;
use tendi_core::generated::runtime_contract::{validate_response, validate_result, CommandName, JsonRpcRequest, JsonRpcResponse};

pub struct RuntimeClient { next_id: u64 }
#[allow(dead_code)]
impl RuntimeClient {
    pub fn new() -> Self { Self { next_id: 0 } }
    pub fn request(&mut self, method: CommandName, params: impl Serialize) -> JsonRpcRequest { self.next_id += 1; JsonRpcRequest { jsonrpc: "2.0".to_string(), id: Value::String(format!("cli-{}", self.next_id)), method: method.as_str().to_string(), params: serde_json::to_value(params).expect("generated request serializes") } }
    pub fn decode_response(command: CommandName, request_id: &Value, value: Value) -> anyhow::Result<Value> {
        validate_response(&value).map_err(|error| anyhow::anyhow!("contract violation: {error}"))?;
        let response: JsonRpcResponse = serde_json::from_value(value)?;
        if response.jsonrpc != "2.0" { anyhow::bail!("daemon returned a non-JSON-RPC response"); }
        if response.id != *request_id { anyhow::bail!("daemon response id does not match request"); }
        if let Some(error) = response.error {
            let kind = error.data.map(|data| data.kind).unwrap_or_else(|| "DAEMON_ERROR".to_string());
            anyhow::bail!("{kind}: {}", error.message);
        }
        let result = response.result.ok_or_else(|| anyhow::anyhow!("daemon response is missing result"))?;
        validate_result(command.as_str(), &result).map_err(|error| anyhow::anyhow!("contract violation: {error}"))?;
        Ok(result)
    }
${clientMethods}
}
`;
}

function generateTauriHandler() {
  const functions = [
    ...schema["x-tendi"].transports.map((transport) => transport.name),
    ...methods.filter((method) => method["x-tendi"].owner === "desktop").map((method) => method.name),
  ];
  return `// DO NOT EDIT. Generated by scripts/runtime-codegen.mjs.
tauri::generate_handler![
${functions.map((name) => `    ${name},`).join("\n")}
]
`;
}

function generateInventory(inventory) {
  return JSON.stringify({
    generatedFrom: "runtime-schema/runtime.openrpc.json",
    openrpc: schema.openrpc,
    protocolVersion: schema["x-tendi"].protocolVersion,
    schemaVersion: schema["x-tendi"].schemaVersion,
    methods: methods.map((method) => ({
      name: method.name,
      owner: method["x-tendi"].owner,
      wire: method["x-tendi"].wire,
      clients: method["x-tendi"].clients,
      execution: method["x-tendi"].execution,
    })),
    events: events.map((event) => ({
      name: event.name,
      wire: event.wire,
      clients: event.clients,
      revisioned: event.revisioned,
      replay: event.replay,
    })),
    sourceInventory: {
      daemonMethods: [...inventory.daemonMethods].sort(),
      tauriCommands: [...inventory.tauriCommands].sort(),
      cliMethods: [...inventory.cliMethods].sort(),
      frontendMethods: [...inventory.frontendMethods].sort(),
      daemonEvents: [...inventory.daemonEvents].sort(),
    },
  }, null, 2) + "\n";
}

function exampleValue(rawSchema, seen = new Set()) {
  const name = refName(rawSchema);
  if (name) {
    if (seen.has(name)) return {};
    return exampleValue(components[name], new Set([...seen, name]));
  }
  const schema = rawSchema ?? {};
  if (schema.const !== undefined) return schema.const;
  if (Array.isArray(schema.enum) && schema.enum.length > 0) return schema.enum[0];
  if (Array.isArray(schema.oneOf) || Array.isArray(schema.anyOf)) {
    const choices = schema.oneOf ?? schema.anyOf;
    const example = choices.find((part) => part?.type !== "null") ?? choices[0];
    return exampleValue(example, seen);
  }
  if (Array.isArray(schema.allOf)) {
    return schema.allOf.reduce((value, part) => {
      const example = exampleValue(part, seen);
      return value && typeof value === "object" && !Array.isArray(value)
        && example && typeof example === "object" && !Array.isArray(example)
        ? { ...value, ...example }
        : example;
    }, {});
  }
  if (Array.isArray(schema.type)) {
    const nonNull = schema.type.find((type) => type !== "null");
    return nonNull ? exampleValue({ ...schema, type: nonNull }, seen) : null;
  }
  if (schema.type === "object" || schema.properties) {
    return Object.fromEntries((schema.required ?? []).map((property) => [
      property,
      exampleValue(schema.properties?.[property], seen),
    ]));
  }
  if (schema.type === "array") return [exampleValue(schema.items, seen)];
  if (schema.type === "string") return "fixture";
  if (schema.type === "integer" || schema.type === "number") return schema.minimum ?? 0;
  if (schema.type === "boolean") return false;
  if (schema.type === "null") return null;
  return {};
}

function generateFixtures() {
  const validRequests = Object.fromEntries(methods.map((method) => [
    method.name,
    method.params.length === 0 ? {} : exampleValue(method.params[0].schema),
  ]));
  const invalidRequests = Object.fromEntries(methods.map((method) => [method.name, []]));
  const validResults = Object.fromEntries(methods.map((method) => [method.name, exampleValue(method.result.schema)]));
  const validEvents = Object.fromEntries(events.map((event) => [event.name, {
    id: 1,
    event: event.name,
    payload: exampleValue(event.payload),
    ...(event.revisioned ? { baseRevision: 0, revision: 1 } : {}),
  }]));
  return JSON.stringify({
    generatedFrom: "runtime-schema/runtime.openrpc.json",
    validRequests,
    invalidRequests,
    validResults,
    invalidResponses: Object.fromEntries(methods.map((method) => [method.name, {
      jsonrpc: "1.0",
      id: "invalid",
      result: null,
    }])),
    validEvents,
    invalidEvents: Object.fromEntries(events.map((event) => [event.name, {
      id: "invalid",
      event: event.name,
      payload: {},
    }])),
  }, null, 2) + "\n";
}

function generateFiles(inventory) {
  return [
    ["runtime-schema/inventory.json", generateInventory(inventory)],
    ["runtime-schema/examples/contract-fixtures.json", generateFixtures()],
    ["apps/desktop/src/lib/generated/runtime-types.ts", generateTypes()],
    ["apps/desktop/src/lib/generated/runtime-validators.ts", generateValidators()],
    ["apps/desktop/src/lib/generated/runtime-events.ts", generateEvents()],
    ["apps/desktop/src/lib/generated/runtime-client.ts", generateClient()],
    ["crates/tendi-core/src/generated/runtime_contract.rs", generateRust()],
    ["crates/tendi-core/src/generated/mod.rs", generateRustMod()],
    ["crates/tendi-daemon/src/generated/runtime_dispatch.rs", generateDaemonDispatch()],
    ["crates/tendi-cli/src/generated/runtime_client.rs", generateRustClient()],
    ["apps/desktop/src-tauri/src/generated/tauri_handler.rs", generateTauriHandler()],
  ];
}

function sameText(left, right) {
  return left.replaceAll("\r\n", "\n") === right.replaceAll("\r\n", "\n");
}

function formatGenerated(relativePath, content) {
  if (!relativePath.endsWith(".rs") || relativePath.endsWith("tauri_handler.rs")) return content;
  return execFileSync("rustfmt", ["--edition", "2024", "--emit", "stdout"], {
    cwd: root,
    input: content,
    encoding: "utf8",
  });
}

validateDocument();
const inventory = await sourceInventory();
if (validateOnly) {
  console.log(`runtime schema valid (${methods.length} methods, ${events.length} events)`);
  process.exit(0);
}

for (const [relativePath, content] of generateFiles(inventory)) {
  const formattedContent = formatGenerated(relativePath, content);
  const outputPath = resolve(root, relativePath);
  if (checkOnly) {
    let current;
    try { current = await readFile(outputPath, "utf8"); } catch { fail(`missing generated file ${relativePath}`); }
    if (!sameText(current, formattedContent)) fail(`stale generated file ${relativePath}; run runtime:codegen`);
  } else {
    await mkdir(resolve(outputPath, ".."), { recursive: true });
    await writeFile(outputPath, formattedContent);
  }
}
console.log(`runtime codegen ${checkOnly ? "check passed" : "completed"} (${methods.length} methods, ${events.length} events)`);
