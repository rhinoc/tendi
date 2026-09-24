export type TranscriptNavigationIntent = "idle" | "search" | "locator" | "skill" | "top" | "bottom";

export function createTranscriptNavigationAuthority() {
  let revision = 0;
  let intent: TranscriptNavigationIntent = "idle";

  return {
    begin(nextIntent: TranscriptNavigationIntent) {
      revision += 1;
      intent = nextIntent;
      return revision;
    },
    isCurrent(candidate: number) {
      return candidate === revision;
    },
    currentIntent() {
      return intent;
    },
  };
}

export type TranscriptTargetPage<Item, Status extends string> = {
  items: Item[];
  status: Status;
};

export type LoadTranscriptTargetResult<Item, Status extends string> = {
  items: Item[];
  status?: Status;
  loaded: boolean;
  cancelled: boolean;
};

export async function loadTranscriptTarget<Item, Status extends string>({
  getItems,
  loadMore,
  isTargetLoaded,
  isCurrent,
  onItemsLoaded,
  loadedStatus,
}: {
  getItems: () => Item[];
  loadMore: () => Promise<TranscriptTargetPage<Item, Status>>;
  isTargetLoaded: (items: Item[]) => boolean;
  isCurrent?: () => boolean;
  onItemsLoaded?: (items: Item[]) => void;
  loadedStatus: Status;
}): Promise<LoadTranscriptTargetResult<Item, Status>> {
  const isNavigationCurrent = isCurrent ?? (() => true);
  let items = getItems();
  let status: Status | undefined;

  while (!isTargetLoaded(items)) {
    if (!isNavigationCurrent()) {
      return { items, status, loaded: false, cancelled: true };
    }

    const page = await loadMore();
    items = page.items;
    status = page.status;
    onItemsLoaded?.(items);

    if (!isNavigationCurrent()) {
      return { items, status, loaded: false, cancelled: true };
    }
    if (status !== loadedStatus) break;
  }

  return {
    items,
    status,
    loaded: isTargetLoaded(items),
    cancelled: false,
  };
}
