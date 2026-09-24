import {
  mergeTranscriptItems,
  transcriptEvidenceSearchText,
  transcriptItemType,
  transcriptItemsSize,
  TranscriptGroupType,
  type TranscriptLocatorItem,
  type TranscriptPage,
  type TranscriptSearchHit,
  type TranscriptSearchScopes,
} from "../../lib/transcript.ts";
import type { SessionSkillLinkRecord } from "../../lib/sessions.ts";

export type TranscriptItemRecord = {
  type: string;
  body: string;
  tag?: string;
  time?: string;
  command?: string;
  result?: string;
  durationMs?: string | number;
  linkedSessionId?: string;
  model?: string;
  effort?: string;
  mode?: string;
  tools?: TranscriptItemRecord[];
  callId?: string;
  startedAtMs?: number;
};

export type TranscriptLoadMoreResult = {
  items: TranscriptItemRecord[];
  status: "loaded" | "exhausted" | "failed" | "cancelled";
};

export type TranscriptTarget = {
  key: string;
  groupKey?: string;
  index: number;
};

export type TranscriptSearchTarget = TranscriptTarget;
export type TranscriptSearchScopeState = TranscriptSearchScopes;

export type SessionLocatorItem = {
  key: string;
  index: number;
  label: string;
  response: string;
};

export const TRANSCRIPT_SEARCH_SCOPES: Array<{
  id: keyof TranscriptSearchScopeState;
  label: string;
  ariaLabel: string;
}> = [
  { id: "user", label: "User", ariaLabel: "Search user messages" },
  { id: "assistant", label: "Assistant", ariaLabel: "Search assistant messages" },
  { id: "system", label: "System", ariaLabel: "Search system messages" },
  { id: "tool", label: "Tools", ariaLabel: "Search tool calls" },
];

export const DEFAULT_TRANSCRIPT_SEARCH_SCOPES: TranscriptSearchScopeState = {
  user: true,
  assistant: true,
  system: false,
  tool: false,
};

const TRANSCRIPT_CACHE_ITEM_LIMIT = 1_200;
const TRANSCRIPT_CACHE_CHARACTER_LIMIT = 8 * 1024 * 1024;

export function transcriptSearchScope(type: string | undefined): keyof TranscriptSearchScopeState {
  switch (type) {
    case "user":
    case "notification":
      return "user";
    case "context":
    case "compaction":
    case "model_config":
      return "system";
    case "tool":
    case TranscriptGroupType.ToolGroup:
      return "tool";
    case "assistant":
    case "thinking":
    case "reasoning":
    default:
      return "assistant";
  }
}

export function transcriptItemKey(prefix: string | undefined, index: string | number) {
  return prefix ? `${prefix}-${index}` : `${index}`;
}

export function transcriptGroupChildKey(groupIndex: string | number, childIndex: number, item: TranscriptItemRecord) {
  return transcriptItemKey(transcriptItemType(item), `${groupIndex}-${childIndex}`);
}

function transcriptItemText(item: TranscriptItemRecord) {
  return [item.body, item.command, item.result, item.tag, item.time]
    .map((value) => `${value ?? ""}`)
    .join("\n")
    .toLowerCase();
}

function evidenceMatchesItem(item: TranscriptItemRecord, evidenceText: string, evidenceTime: string) {
  const itemTime = `${item.time ?? ""}`.trim();
  if (evidenceTime && itemTime === evidenceTime) return true;
  const needle = evidenceText.toLowerCase();
  const command = `${item.command ?? ""}`.trim().toLowerCase();
  if (!needle) return false;
  return transcriptItemText(item).includes(needle) || Boolean(command && needle.includes(command));
}

export function findSkillEvidenceTarget(
  transcriptItems: TranscriptItemRecord[],
  link: SessionSkillLinkRecord,
): TranscriptTarget | null {
  const evidenceText = transcriptEvidenceSearchText(link.evidence_text);
  const evidenceTime = `${link.evidence_time ?? ""}`.trim();
  for (let index = 0; index < transcriptItems.length; index += 1) {
    const item = transcriptItems[index];
    if (transcriptItemType(item) === TranscriptGroupType.ToolGroup) {
      const tools = item.tools ?? [];
      for (let toolIndex = 0; toolIndex < tools.length; toolIndex += 1) {
        if (evidenceMatchesItem(tools[toolIndex], evidenceText, evidenceTime)) {
          return {
            key: transcriptGroupChildKey(index, toolIndex, tools[toolIndex]),
            groupKey: transcriptItemKey("tool-group", index),
            index,
          };
        }
      }
      continue;
    }
    if (evidenceMatchesItem(item, evidenceText, evidenceTime)) {
      return { key: transcriptItemKey(transcriptItemType(item), index), index };
    }
  }
  return null;
}

export function findTranscriptSearchTargets(
  transcriptItems: TranscriptItemRecord[],
  query: string,
  scopes: TranscriptSearchScopeState,
): TranscriptSearchTarget[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];

  const matches: TranscriptSearchTarget[] = [];
  transcriptItems.forEach((item, index) => {
    const type = transcriptItemType(item);
    if (type === TranscriptGroupType.ToolGroup) {
      (item.tools ?? []).forEach((tool, toolIndex) => {
        if (scopes[transcriptSearchScope(transcriptItemType(tool))] && transcriptItemText(tool).includes(needle)) {
          matches.push({
            key: transcriptGroupChildKey(index, toolIndex, tool),
            groupKey: transcriptItemKey("tool-group", index),
            index,
          });
        }
      });
      return;
    }
    if (!scopes[transcriptSearchScope(type)]) return;
    if (transcriptItemText(item).includes(needle)) {
      matches.push({ key: transcriptItemKey(type, index), index });
    }
  });
  return matches;
}

type TranscriptSearchIndexEntry = {
  groupIndex: number;
  itemIndex: number;
  childIndex?: number;
  toolIndex?: number;
};

export function transcriptSearchIndexForBackendGroups(transcriptItems: TranscriptItemRecord[]) {
  const entries: TranscriptSearchIndexEntry[] = [];
  let groupIndex = 0;
  for (let itemIndex = 0; itemIndex < transcriptItems.length; itemIndex += 1) {
    const item = transcriptItems[itemIndex];
    if (transcriptItemType(item) !== TranscriptGroupType.ToolGroup) {
      entries.push({
        groupIndex,
        itemIndex,
        ...(transcriptItemType(item) === "tool" ? { toolIndex: 0 } : {}),
      });
      groupIndex += 1;
      continue;
    }

    let toolGroupIndex: number | undefined;
    let toolIndex = 0;
    for (let childIndex = 0; childIndex < (item.tools ?? []).length; childIndex += 1) {
      const child = item.tools![childIndex];
      if (transcriptItemType(child) === "tool") {
        if (toolGroupIndex === undefined) {
          toolGroupIndex = groupIndex;
          groupIndex += 1;
          toolIndex = 0;
        } else {
          toolIndex += 1;
        }
        entries.push({ groupIndex: toolGroupIndex, itemIndex, childIndex, toolIndex });
      } else {
        toolGroupIndex = undefined;
        entries.push({ groupIndex, itemIndex, childIndex });
        groupIndex += 1;
      }
    }
  }
  return { entries, groupCount: groupIndex };
}

export function transcriptSearchTargetForHit(
  transcriptItems: TranscriptItemRecord[],
  hit: TranscriptSearchHit,
): TranscriptSearchTarget | null {
  const indexEntry = transcriptSearchIndexForBackendGroups(transcriptItems).entries.find((entry) => (
    entry.groupIndex === hit.groupIndex && entry.toolIndex === hit.toolIndex
  ));
  if (!indexEntry) return null;
  const item = transcriptItems[indexEntry.itemIndex];
  if (!item) return null;
  if (indexEntry.childIndex !== undefined && item.tools?.[indexEntry.childIndex]) {
    const child = item.tools[indexEntry.childIndex];
    return {
      key: transcriptGroupChildKey(indexEntry.itemIndex, indexEntry.childIndex, child),
      groupKey: transcriptItemKey("tool-group", indexEntry.itemIndex),
      index: indexEntry.itemIndex,
    };
  }
  const type = transcriptItemType(item);
  return {
    key: transcriptItemKey(type, indexEntry.itemIndex),
    index: indexEntry.itemIndex,
  };
}

export function transcriptItemSearchQuery(
  item: TranscriptItemRecord,
  query: string,
  scopes: TranscriptSearchScopeState,
): string {
  if (!query) return "";
  if (transcriptItemType(item) === TranscriptGroupType.ToolGroup) {
    return (item.tools ?? []).some((child) => scopes[transcriptSearchScope(transcriptItemType(child))]) ? query : "";
  }
  if (!scopes[transcriptSearchScope(transcriptItemType(item))]) return "";
  return query;
}

export function buildSessionLocatorItems(transcriptItems: TranscriptItemRecord[]): SessionLocatorItem[] {
  const locatorItems: SessionLocatorItem[] = [];
  let pendingResponse: SessionLocatorItem | null = null;
  for (let index = 0; index < transcriptItems.length; index += 1) {
    const item = transcriptItems[index];
    const type = transcriptItemType(item);
    if (type === "user") {
      pendingResponse = {
        key: transcriptItemKey("user", index),
        index,
        label: item.body.trim(),
        response: "",
      };
      locatorItems.push(pendingResponse);
    } else if (type === "assistant" && pendingResponse) {
      pendingResponse.response = item.body.trim();
      pendingResponse = null;
    }
  }
  return locatorItems;
}

export function buildSessionLocatorItemsFromMetadata(
  locatorMetadata: TranscriptLocatorItem[],
  indexOffset: number,
): SessionLocatorItem[] {
  return locatorMetadata.map((item) => {
    const index = item.index + indexOffset;
    return {
      key: transcriptItemKey("user", index),
      index,
      label: item.label,
      response: item.response,
    };
  });
}

function transcriptRefreshIdentity(item: TranscriptItemRecord) {
  return JSON.stringify([
    transcriptItemType(item),
    item.body,
    item.tag,
    item.time,
    item.command,
    item.linkedSessionId,
    item.model,
    item.effort,
    item.callId,
    item.tools?.map((tool) => [
      transcriptItemType(tool),
      tool.body,
      tool.tag,
      tool.time,
      tool.command,
      tool.linkedSessionId,
      tool.model,
      tool.effort,
      tool.callId,
    ]),
  ]);
}

export function transcriptItemsSharePrefix(currentItems: TranscriptItemRecord[], refreshedItems: TranscriptItemRecord[]) {
  const prefixLength = Math.min(currentItems.length, refreshedItems.length);
  for (let index = 0; index < prefixLength; index += 1) {
    if (transcriptRefreshIdentity(currentItems[index]) !== transcriptRefreshIdentity(refreshedItems[index])) return false;
  }
  return true;
}

export function preserveTranscriptTail(currentItems: TranscriptItemRecord[], refreshedItems: TranscriptItemRecord[]) {
  if (currentItems.length <= refreshedItems.length) return refreshedItems;
  return transcriptItemsSharePrefix(currentItems, refreshedItems)
    ? [...refreshedItems, ...currentItems.slice(refreshedItems.length)]
    : refreshedItems;
}

export function trimTranscriptCache(cache: Map<string, TranscriptPage>) {
  let itemCount = 0;
  let characterCount = 0;
  for (const page of cache.values()) {
    itemCount += page.items.length;
    characterCount += transcriptItemsSize(page.items);
  }
  while (
    cache.size > 1
    && (itemCount > TRANSCRIPT_CACHE_ITEM_LIMIT || characterCount > TRANSCRIPT_CACHE_CHARACTER_LIMIT)
  ) {
    const oldestKey = cache.keys().next().value;
    if (oldestKey === undefined) break;
    const oldest = cache.get(oldestKey);
    cache.delete(oldestKey);
    if (oldest) {
      itemCount -= oldest.items.length;
      characterCount -= transcriptItemsSize(oldest.items);
    }
  }
  if (cache.size === 1 && (itemCount > TRANSCRIPT_CACHE_ITEM_LIMIT || characterCount > TRANSCRIPT_CACHE_CHARACTER_LIMIT)) {
    const key = cache.keys().next().value;
    if (key !== undefined) cache.delete(key);
  }
}

export function mergeTranscriptPage(cached: TranscriptPage | undefined, currentItems: TranscriptItemRecord[], page: TranscriptPage): TranscriptPage {
  return {
    items: mergeTranscriptItems(cached?.items ?? currentItems, page.items),
    locatorItems: cached?.locatorItems ?? page.locatorItems,
    warnings: [...(cached?.warnings ?? []), ...page.warnings],
    nextCursor: page.nextCursor,
    done: page.done,
    sourceVersion: page.sourceVersion,
    restartRequired: false,
    unchanged: false,
  };
}

export function asTranscriptPage(
  page: TranscriptPage,
  items: TranscriptItemRecord[],
  sourceVersion: string,
  warnings: string[],
  nextCursor?: string,
): TranscriptPage {
  return {
    ...page,
    items,
    warnings,
    sourceVersion,
    nextCursor,
    done: !nextCursor,
    restartRequired: false,
    unchanged: false,
  };
}
