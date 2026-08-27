export const OVERVIEW_COUNT_RETRY_ATTEMPTS = 3;

export type OverviewCountRetryOptions = {
  loadCount: () => Promise<boolean>;
  reloadDomain: () => Promise<void>;
  shouldContinue?: () => boolean;
};

export async function loadOverviewCountWithRetry({
  loadCount,
  reloadDomain,
  shouldContinue = () => true,
}: OverviewCountRetryOptions): Promise<boolean> {
  for (let attempt = 0; attempt < OVERVIEW_COUNT_RETRY_ATTEMPTS; attempt += 1) {
    if (!shouldContinue()) return false;
    if (attempt > 0) await reloadDomain();
    if (await loadCount()) return true;
  }
  return false;
}
