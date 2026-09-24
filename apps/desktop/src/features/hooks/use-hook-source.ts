import { useCallback, useEffect, useState } from "react";

import type { HookRecord } from "../../lib/hooks.ts";
import { readHookSource } from "../../lib/runtime-gateway.ts";

type HookSourceData = {
  content?: string;
  source_line?: number | null;
  path?: string;
} | null;

type HookSourceState = {
  key: string;
  loading: boolean;
  data: HookSourceData;
  error: string;
};

function hookSourceIdentity(hook: HookRecord | null): string {
  if (!hook) return "";
  return [
    hook.agent,
    hook.path,
    hook.event,
    hook.matcher,
    hook.hook_type,
    hook.command,
    hook.url,
    hook.prompt,
    hook.filter,
  ].map((value) => `${value ?? ""}`).join("|");
}

export function useHookSource(hook: HookRecord | null) {
  const identity = hookSourceIdentity(hook);
  const agent = hook?.agent;
  const path = hook?.path;
  const expectedTrustHash = hook?.trust_hash;
  const event = hook?.event;
  const matcher = hook?.matcher;
  const hookType = hook?.hook_type;
  const command = hook?.command;
  const url = hook?.url;
  const prompt = hook?.prompt;
  const filter = hook?.filter;
  const statusMessage = hook?.status_message;
  const enabled = hook?.enabled;
  const requestIdentity = JSON.stringify([
    agent,
    command,
    enabled,
    event,
    filter,
    hookType,
    matcher,
    path,
    prompt,
    statusMessage,
    expectedTrustHash,
    url,
  ]);
  const [state, setState] = useState<HookSourceState>({ key: "", loading: false, data: null, error: "" });

  useEffect(() => {
    if (!agent) {
      setState({ key: "", loading: false, data: null, error: "" });
      return undefined;
    }
    if (!path) {
      setState({ key: identity, loading: false, data: null, error: "Missing hook source path" });
      return undefined;
    }
    if (expectedTrustHash === undefined || event === undefined) {
      setState({ key: identity, loading: false, data: null, error: "Missing hook source identity" });
      return undefined;
    }

    let cancelled = false;
    setState((current) => ({
      key: identity,
      loading: true,
      data: current.key === identity ? current.data : null,
      error: "",
    }));
    readHookSource({
      agent,
      path,
      expectedTrustHash,
      event,
      matcher,
      hookType,
      command,
      url,
      prompt,
      filter,
      statusMessage,
    })
      .then((data) => {
        if (!cancelled) setState({ key: identity, loading: false, data, error: "" });
      })
      .catch((error) => {
        if (!cancelled) setState((current) => ({
          key: identity,
          loading: false,
          data: current.key === identity ? current.data : null,
          error: `${error}`,
        }));
      });
    return () => {
      cancelled = true;
    };
  }, [
    agent,
    command,
    enabled,
    event,
    filter,
    hookType,
    matcher,
    path,
    prompt,
    statusMessage,
    expectedTrustHash,
    url,
    identity,
  ]);

  const clearError = useCallback(() => setState((current) => ({ ...current, error: "" })), []);

  return {
    state,
    data: state.key === identity ? state.data : null,
    requestIdentity,
    clearError,
  };
}
