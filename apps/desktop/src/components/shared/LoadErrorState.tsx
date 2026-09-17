import { Toast } from "./Toast.tsx";

export type LoadErrorStateProps = {
  message: string;
  onRetry?: () => void;
  retryLabel?: string;
};

export function LoadErrorState({ message, onRetry, retryLabel = "Retry" }: LoadErrorStateProps) {
  return (
    <Toast
      tone="error"
      message={message}
      action={onRetry ? { label: retryLabel, onClick: onRetry } : undefined}
    />
  );
}
