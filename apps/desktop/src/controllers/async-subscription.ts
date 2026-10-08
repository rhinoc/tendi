export function startAsyncSubscription<T>(
  subscribe: (handler: (event: T) => void) => Promise<() => void>,
  onEvent: (event: T) => void,
  onError: (error: unknown) => void,
): { ready: Promise<void>; dispose: () => void } {
  let disposed = false;
  let unsubscribe: (() => void) | null = null;
  const ready = subscribe((event) => {
    if (!disposed) onEvent(event);
  }).then((cleanup) => {
    if (disposed) cleanup();
    else unsubscribe = cleanup;
  }).catch((error) => {
    if (!disposed) onError(error);
  });
  return {
    ready,
    dispose: () => {
      disposed = true;
      unsubscribe?.();
      unsubscribe = null;
    },
  };
}
