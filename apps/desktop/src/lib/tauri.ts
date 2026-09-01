import { invoke } from "@tauri-apps/api/core";

import { COMMAND_METADATA, TauriCommand as GeneratedTauriCommand, isDaemonCommand, isDesktopCommand, type CliInstallStatus as RuntimeCliInstallStatus, type CommandName, type JsonRpcRequest, type JsonRpcResponse, type RequestFor, type ResponseFor, type UpdateCheckResult as RuntimeUpdateCheckResult } from "./generated/runtime-types.ts";
import { RuntimeClient, RuntimeRemoteError } from "./generated/runtime-client.ts";
import { RuntimeContractError, validateEvent, validateRequest, validateResult } from "./generated/runtime-validators.ts";
import { RuntimeEventName } from "./generated/runtime-events.ts";
import { omitUndefinedProperties, type DaemonEvent as RuntimeDaemonEvent } from "./runtime-contract.ts";

import type { RawSkillRecord } from "./skills.ts";
import { logger } from "./logger.ts";

export enum CliInstallState {
  Installed = "installed",
  NotInstalled = "not-installed",
  Stale = "stale",
  Conflict = "conflict",
  Unsupported = "unsupported",
}

export type CliInstallStatus = RuntimeCliInstallStatus;

export type BundledSkillStatus = {
  name: string;
  target: string;
  installed: boolean;
  current: boolean;
  promptHandled: boolean;
  shouldPrompt: boolean;
};

export type BundledSkillInstallReport = {
  applied: boolean;
  status: BundledSkillStatus;
  updated?: RawSkillRecord[];
};

export type UpdateCheckResult = RuntimeUpdateCheckResult;

export enum UpdateCheckStatus {
  UpToDate = "up-to-date",
  Available = "available",
  Busy = "busy",
}

export type DesktopUpdateState = {
  status: DesktopUpdateStatus;
  version?: string;
  body?: string;
  error?: string;
};

export enum DesktopUpdateStatus {
  Idle = "idle",
  Checking = "checking",
  UpToDate = "up-to-date",
  Available = "available",
  Installing = "installing",
  Error = "error",
}

export const UPDATE_AVAILABLE_EVENT = RuntimeEventName.TendiUpdateAvailable;
export const TauriCommand = GeneratedTauriCommand;
export type TauriCommand = CommandName;

type TauriWindow = Window & {
  __TAURI_INTERNALS__?: {
    invoke?: unknown;
    transformCallback?: unknown;
  };
};

export function isTauriRuntime(): boolean {
  if (typeof window === "undefined") return false;
  const internals = (window as TauriWindow).__TAURI_INTERNALS__;
  return typeof internals?.invoke === "function" && typeof internals.transformCallback === "function";
}

export class DaemonCommandError extends Error {
  readonly code: string;
  readonly numericCode: number | undefined;
  readonly data: unknown;

  constructor(code: string, message: string, data?: unknown, numericCode?: number) {
    super(message);
    this.name = "DaemonCommandError";
    this.code = code;
    this.numericCode = numericCode;
    this.data = data;
  }
}

export type DaemonEvent<T = unknown> = RuntimeDaemonEvent<T>;

type DaemonEventHandler = (event: DaemonEvent) => void;

type RuntimeCommand = CommandName;

function commandName(command: RuntimeCommand): CommandName {
  if (!Object.hasOwn(COMMAND_METADATA, command)) {
    throw new RuntimeContractError(String(command), "$.method", "unknown command");
  }
  return command as CommandName;
}

async function requestDaemon(request: JsonRpcRequest): Promise<JsonRpcResponse> {
  if (isTauriRuntime()) {
    return invoke<JsonRpcResponse>("daemon_invoke", { request });
  }
  const response = await fetch("/__tendi/invoke", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(request),
  });
  const payload = await response.json() as JsonRpcResponse;
  if (!response.ok && !payload.error) throw new Error(`Web bridge request failed (${response.status})`);
  return payload;
}

const runtimeClient = new RuntimeClient({ request: requestDaemon });

export async function invokeCommand<C extends CommandName>(command: C, args?: RequestFor<C>): Promise<ResponseFor<C>> {
  try {
    const request = (args === undefined ? {} : omitUndefinedProperties(args)) as RequestFor<C>;
    if (isDaemonCommand(command)) {
      const method = commandName(command);
      const result = await runtimeClient.call(method, request);
      return result as ResponseFor<C>;
    }
    if (!isTauriRuntime()) {
      if (isDesktopCommand(command)) {
        throw new Error(`Command ${command} is only available in the Tauri desktop runtime`);
      }
      throw new Error(`Command ${command} is not exposed by the shared daemon API`);
    }
    const method = commandName(command);
    validateRequest(method, request as RequestFor<CommandName>);
    const result = await invoke<ResponseFor<C>>(command, args === undefined ? undefined : request);
    validateResult(method, result);
    return result;
  } catch (error) {
    logger.error("tendi command failed", { command, error });
    if (error instanceof RuntimeRemoteError) {
      throw new DaemonCommandError(error.kind ?? "DAEMON_ERROR", error.message, undefined, error.code);
    }
    throw error;
  }
}

export async function safeInvoke<C extends CommandName>(command: C, args?: RequestFor<C>): Promise<ResponseFor<C> | null> {
  try {
    return await invokeCommand(command, args);
  } catch {
    return null;
  }
}

export async function subscribeDaemonEvents(handler: DaemonEventHandler): Promise<() => void> {
  if (!isTauriRuntime()) {
    const controller = new AbortController();
    let disposed = false;
    let lastEventId: number | null = null;
    const consume = async () => {
      while (!disposed) {
        try {
          const headers: Record<string, string> = { accept: "text/event-stream" };
          if (lastEventId !== null) headers["last-event-id"] = `${lastEventId}`;
          const response = await fetch("/__tendi/events", {
            headers,
            signal: controller.signal,
          });
          if (!response.ok || !response.body) {
            throw new Error(`Daemon event stream failed (${response.status})`);
          }
          const reader = response.body.getReader();
          const decoder = new TextDecoder();
          let buffer = "";
          while (!disposed) {
            const { done, value } = await reader.read();
            if (done) break;
            buffer += decoder.decode(value, { stream: true });
            const blocks = buffer.split("\n\n");
            buffer = blocks.pop() || "";
            for (const block of blocks) {
              const event = parseSseEvent(block);
              if (!event) continue;
              lastEventId = event.id;
              handler(event);
            }
          }
        } catch (error) {
          if (!disposed) logger.warn("daemon event stream failed; retrying", { error, lastEventId });
        }
        if (!disposed) await new Promise<void>((resolve) => window.setTimeout(resolve, 250));
      }
    };
    void consume();
    return () => {
      disposed = true;
      controller.abort();
    };
  }

  const subscriptionId = await invoke<string>("daemon_subscribe_events");
  let disposed = false;
  const consume = async () => {
    try {
      while (!disposed) {
        const event = await invoke<DaemonEvent | null>("daemon_next_event", {
          subscriptionId,
          timeoutMs: 25_000,
        });
        if (event && !disposed) {
          validateEvent(event);
          handler(event);
        }
      }
    } catch (error) {
      if (!disposed) logger.warn("daemon event subscription failed", { error });
    }
  };
  void consume();
  return () => {
    disposed = true;
    void invoke("daemon_unsubscribe_events", { subscriptionId });
  };
}

function parseSseEvent(block: string): DaemonEvent | null {
  let id: number | undefined;
  let event = "message";
  const data: string[] = [];
  for (const line of block.split("\n")) {
    if (line.startsWith("id:")) {
      const parsedId = Number(line.slice(3).trim());
      if (!Number.isSafeInteger(parsedId) || parsedId < 0) return null;
      id = parsedId;
    }
    else if (line.startsWith("event:")) event = line.slice(6).trim();
    else if (line.startsWith("data:")) data.push(line.slice(5).trimStart());
  }
  if (id === undefined || data.length === 0) return null;
  try {
    const parsed = JSON.parse(data.join("\n")) as unknown;
    if (!parsed || typeof parsed !== "object") return null;
    const daemonEvent = parsed as Partial<DaemonEvent>;
    if (daemonEvent.id !== id || daemonEvent.event !== event || !("payload" in daemonEvent)) {
      logger.warn("invalid daemon SSE event envelope", { id, event });
      return null;
    }
    validateEvent(daemonEvent);
    return daemonEvent as DaemonEvent;
  } catch (error) {
    logger.warn("invalid daemon SSE event", { error });
    return null;
  }
}

export async function copyText(value: string | null | undefined): Promise<void> {
  if (!value) return;
  await navigator.clipboard?.writeText(value).catch((error) => {
    logger.warn("copy failed", { error });
  });
}
