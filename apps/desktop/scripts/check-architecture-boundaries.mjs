import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "../../..");

async function source(relativePath) {
  return readFile(resolve(root, relativePath), "utf8");
}

function requireText(text, needle, label) {
  if (!text.includes(needle)) throw new Error(`${label}: missing ${needle}`);
}

function rejectText(text, needle, label) {
  if (text.includes(needle)) throw new Error(`${label}: forbidden ${needle}`);
}

const storage = await source("crates/tendi-core/src/storage.rs");
const daemon = await source("crates/tendi-daemon/src/lib.rs");
const coordinator = await source("crates/tendi-daemon/src/operation_coordinator.rs");
const contracts = await source("crates/tendi-core/src/runtime_contract.rs");
const generatedContracts = await source("crates/tendi-core/src/generated/runtime_contract.rs");
const frontendContracts = await source("apps/desktop/src/lib/runtime-contract.ts");
const virtualization = await source("apps/desktop/src/lib/virtualization.ts");
const transcript = await source("apps/desktop/src/lib/agent/transcript.ts");
const transcriptWorker = await source("apps/desktop/src/workers/transcript-import.worker.ts");
const dataTable = await source("apps/desktop/src/components/DataTable.tsx");
const sessionsView = await source("apps/desktop/src/views/SessionsView.tsx");
const tauri = await source("apps/desktop/src/lib/tauri.ts");
const runtimeGateway = await source("apps/desktop/src/lib/runtime-gateway.ts");
const app = await source("apps/desktop/src/App.tsx");
const sessionRuntimeController = await source("apps/desktop/src/controllers/session-runtime-controller.ts");
const settingsView = await source("apps/desktop/src/features/settings/SettingsView.tsx");
const catalogController = await source("apps/desktop/src/controllers/catalog-controller.ts");
const hooks = await source("apps/desktop/src/lib/hooks.ts");
const mcp = await source("apps/desktop/src/lib/mcp.ts");
const mcpView = await source("apps/desktop/src/views/McpView.tsx");
const hooksView = await source("apps/desktop/src/views/HooksView.tsx");
const rulesView = await source("apps/desktop/src/views/RulesView.tsx");
const cli = await source("crates/tendi-cli/src/main.rs");
const sessionScanGateway = runtimeGateway.slice(
  runtimeGateway.indexOf("function parseSessionScanEvent"),
  runtimeGateway.indexOf("function parseSkillUpdateEvent"),
);
const sessionSnapshotGateway = runtimeGateway.slice(
  runtimeGateway.indexOf("export async function invokeSessionSnapshot"),
  runtimeGateway.indexOf("export async function invokeSessionScanStart"),
);

requireText(storage, "CREATE TABLE IF NOT EXISTS normalized_snapshots", "storage snapshots");
requireText(storage, "PRIMARY KEY (scope_key, domain)", "storage snapshot scope");
requireText(storage, "CREATE TABLE IF NOT EXISTS scoped_projection_contexts", "projection context scope");
requireText(storage, "pub fn list_sessions_for_scope", "session scope read");
requireText(storage, "pub fn skill_source_records_for_workspace", "skill source scope read");
requireText(storage, "CREATE TABLE IF NOT EXISTS scoped_skill_sources", "skill source physical scope");
requireText(storage, "CREATE TABLE IF NOT EXISTS scoped_skill_snapshots", "skill snapshot physical scope");
requireText(storage, "pub fn upsert_skill_source_records_for_workspace", "skill source scope write");
requireText(storage, "persist_skill_update_persistence_for_workspace_with_deleted", "atomic skill persistence");
requireText(storage, "save_sessions_at_with_scope_in_tx", "atomic session persistence");
requireText(storage, "finalize_projection_domain_in_tx", "atomic projection persistence");
requireText(storage, "pub fn overview_analytics_for_scope", "analytics scope read");
requireText(storage, "CREATE TABLE IF NOT EXISTS scoped_session_analytics", "analytics physical scope");
requireText(storage, "PRIMARY KEY (scope_key, session_id, agent, session_path)", "analytics composite identity");
requireText(storage, "pub fn session_scan_cache_for_scope", "session cache scope");
requireText(contracts, "pub struct SourceLocator", "source identity contract");
requireText(contracts, "pub struct SessionKey", "session identity contract");
requireText(coordinator, "struct OperationCoordinator", "operation writer owner");
requireText(daemon, "self.state.operations.shutdown()", "operation shutdown");
requireText(daemon, "last-event-id", "event replay input");
requireText(daemon, "serde_json::to_string(&event)", "event envelope transport");
requireText(frontendContracts, "function decideRevision", "frontend revision reducer contract");
requireText(virtualization, "export type VirtualizationContract", "virtualization contract");
requireText(virtualization, "start: Math.min(start, end)", "virtualization range invariant");
requireText(virtualization, "end: Math.max(start, end)", "virtualization range invariant");
requireText(dataTable, "virtualRangeFor({", "data table virtualization contract");
requireText(sessionsView, "measured,", "transcript measured virtualization contract");
requireText(transcript, "parseJsonlTranscriptForProvider", "provider-owned import parser");
requireText(transcriptWorker, "providerId: string", "explicit import provider boundary");
requireText(tauri, "headers[\"last-event-id\"]", "event stream replay reconnect");
requireText(runtimeGateway, "skillId: string", "skill file identity boundary");
requireText(daemon, "fn skill_context_for_id", "daemon skill file identity boundary");
requireText(daemon, "fn skill_projection_for_ids", "daemon skill mutation identity boundary");
requireText(app, "useSessionRuntimeController", "session runtime lifecycle owner");
requireText(sessionRuntimeController, "sessionScanWaiters", "session scan waiter owner");
requireText(sessionRuntimeController, "lastDaemonEventId", "session event gap owner");
requireText(sessionRuntimeController, "resyncSessionSnapshot", "session snapshot resync owner");
requireText(sessionScanGateway, "recordRows(value.upserts", "session scan raw row validation");
requireText(sessionSnapshotGateway, "recordRows(snapshot.payload", "session snapshot raw row validation");
rejectText(sessionScanGateway, "normalizeSession", "session scan must not normalize at gateway");
rejectText(sessionSnapshotGateway, "normalizeSession", "session snapshot must not normalize at gateway");
for (const needle of ["sessionScanWaiters", "lastDaemonEventId", "sessionSnapshotResyncInFlight", "pendingRecentSessions", "pendingWatchSessions", "pendingDeletedSessions"]) {
  rejectText(app, needle, "App must not own session runtime lifecycle");
}
rejectText(daemon, "fn skill_dir_from_scan(", "skill name path resolution");
requireText(cli, "runtime_client.skills_add", "CLI skill add generated client");
requireText(cli, "runtime_client.skills_set", "CLI skill set generated client");
requireText(cli, "runtime_client.skills_update", "CLI skill update generated client");
requireText(cli, "runtime_client.skills_distribute", "CLI skill distribution uses active distribution client");
rejectText(app, "normalizeSettings(", "App must not normalize the Settings response");
rejectText(app, "normalizeAppIcon(", "App must not normalize the Settings response");
rejectText(settingsView, "const savedSettings = normalizeSettings(value)", "SettingsView must not normalize the saved response");
requireText(catalogController, "const hook = normalizeHook(row)", "Hook controller normalizes updated rows");
requireText(catalogController, "const server = normalizeMcp(row)", "MCP controller normalizes updated rows");
rejectText(catalogController, "Array.isArray(result) ? applyDomainSnapshot(current, RuntimeDomainKey.Hooks", "Hook command must use the typed delta contract");
rejectText(catalogController, "Array.isArray(result) ? applyDomainSnapshot(current, RuntimeDomainKey.Mcp", "MCP command must use the typed delta contract");
rejectText(catalogController, "if (Array.isArray(result)) return applyDomainSnapshot(current, RuntimeDomainKey.Rules", "Rule command must use the typed delete contract");
rejectText(mcpView, "Array.isArray(result)", "MCP view must consume the object mutation contract");
rejectText(hooksView, "Array.isArray(result)", "Hooks view must consume the object mutation contract");
rejectText(rulesView, "Array.isArray(result)", "Rules view must consume the object mutation contract");
requireText(generatedContracts, '"agents_list" => Some(CommandMetadata {', "read refresh coordinator boundary");
requireText(generatedContracts, 'name: "agents_list"', "read refresh command metadata");
requireText(generatedContracts, "serialized_write: true", "read refresh serialized write metadata");

for (const needle of ["collect_generic_item", "generic_tool_payloads", "AgentTranscriptFormat = \"generic\""]) {
  rejectText(transcript, needle, "provider parser naming");
}
rejectText(daemon, "store.list_skills()", "daemon workspace reads");
const backupStatusMetadata = generatedContracts.slice(
  generatedContracts.indexOf('"skills_backup_status" => Some(CommandMetadata {'),
  generatedContracts.indexOf('"skills_backup_configure" => Some(CommandMetadata {'),
);
requireText(backupStatusMetadata, 'serialized_write: true', "serialized backup projection read");

console.log("architecture boundary checks passed");
