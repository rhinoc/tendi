import { useCallback, useRef, useSyncExternalStore, type Dispatch, type SetStateAction } from "react";

type TabStateEntry<T> = {
  key: string;
  value: T;
  listeners: Set<() => void>;
};

const tabStates = new Map<string, TabStateEntry<unknown>>();
const tabScrollPositions = new Map<string, { top: number; left: number }>();

export function getTabScrollPosition(key: string) {
  return tabScrollPositions.get(key);
}

export function setTabScrollPosition(key: string, position: { top: number; left: number }) {
  tabScrollPositions.set(key, position);
}

function resolveInitialValue<T>(initialValue: T | (() => T)): T {
  return typeof initialValue === "function" ? (initialValue as () => T)() : initialValue;
}

function getTabStateEntry<T>(key: string, initialValue: T | (() => T)): TabStateEntry<T> {
  const existing = tabStates.get(key);
  if (existing) return existing as TabStateEntry<T>;

  const entry: TabStateEntry<T> = {
    key,
    value: resolveInitialValue(initialValue),
    listeners: new Set(),
  };
  tabStates.set(key, entry);
  return entry;
}

export function useTabState<T>(key: string, initialValue: T | (() => T)): [T, Dispatch<SetStateAction<T>>] {
  const entryRef = useRef<TabStateEntry<T> | null>(null);
  const entry = entryRef.current?.key === key
    ? entryRef.current
    : getTabStateEntry(key, initialValue);
  entryRef.current = entry;

  const subscribe = useCallback((listener: () => void) => {
    entry.listeners.add(listener);
    return () => {
      entry.listeners.delete(listener);
    };
  }, [entry]);
  const getSnapshot = useCallback(() => entry.value, [entry]);
  const value = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  const setValue = useCallback<Dispatch<SetStateAction<T>>>((action) => {
    const nextValue = typeof action === "function"
      ? (action as (previousValue: T) => T)(entry.value)
      : action;
    if (Object.is(entry.value, nextValue)) return;

    entry.value = nextValue;
    for (const listener of entry.listeners) listener();
  }, [entry]);

  return [value, setValue];
}
