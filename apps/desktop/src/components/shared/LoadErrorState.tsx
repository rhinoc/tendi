import { Button } from "./Button.tsx";
import { EmptyState } from "./EmptyState.tsx";

export type LoadErrorStateProps = {
  message: string;
  onRetry?: () => void;
  retryLabel?: string;
};

export function LoadErrorState({ message, onRetry, retryLabel = "Retry" }: LoadErrorStateProps) {
  return (
    <EmptyState
      title={message}
      role="alert"
      action={onRetry ? <Button size="sm" variant="ghost" onClick={onRetry}>{retryLabel}</Button> : undefined}
    />
  );
}
