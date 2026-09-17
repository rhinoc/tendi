import { createContext, useCallback, useContext, useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";

import "./Toast.css";

export type ToastTone = "error" | "success" | "info";

export type ToastProps = {
  message: string;
  tone?: ToastTone;
  action?: {
    label: string;
    onClick: () => void;
    disabled?: boolean;
  };
  onDismiss?: () => void;
};

type ToastEntry = ToastProps & { id: string };

type ToastContextValue = {
  register: (entry: ToastEntry) => void;
  update: (id: string, props: ToastProps) => void;
  unregister: (id: string) => void;
  show: (props: ToastProps) => void;
};

const ToastContext = createContext<ToastContextValue | null>(null);

const toneIcons = {
  error: AlertCircle,
  success: CheckCircle2,
  info: Info,
} as const;

function ToastCard({ message, tone = "info", action, onDismiss, standalone = false }: ToastProps & { standalone?: boolean }) {
  const Icon = toneIcons[tone];

  return (
    <div className={`appToast appToast--${tone}${standalone ? " appToast--standalone" : ""}`} role={tone === "error" ? "alert" : "status"} aria-live={tone === "error" ? "assertive" : "polite"}>
      <Icon className="appToastIcon" size={16} aria-hidden="true" />
      <span className="appToastMessage">{message}</span>
      {action ? (
        <button type="button" className="appToastAction" onClick={action.onClick} disabled={action.disabled}>
          {action.label}
        </button>
      ) : null}
      {onDismiss ? (
        <button type="button" className="appToastDismiss" aria-label="Dismiss notification" onClick={onDismiss}>
          <X size={14} aria-hidden="true" />
        </button>
      ) : null}
    </div>
  );
}

function toastIdentity(toast: ToastProps): string {
  return `${toast.tone ?? "info"}\u0000${toast.message}`;
}

function sameToastProps(left: ToastProps, right: ToastProps): boolean {
  return left.message === right.message
    && left.tone === right.tone
    && left.action?.label === right.action?.label
    && left.action?.disabled === right.action?.disabled
    && left.action?.onClick === right.action?.onClick
    && left.onDismiss === right.onDismiss;
}

function mergeDismissHandlers(...handlers: Array<(() => void) | undefined>): (() => void) | undefined {
  const uniqueHandlers = [...new Set(handlers.filter((handler): handler is () => void => Boolean(handler)))];
  if (uniqueHandlers.length === 0) return undefined;
  if (uniqueHandlers.length === 1) return uniqueHandlers[0];
  return () => {
    for (const handler of uniqueHandlers) handler();
  };
}

function uniqueToastEntries(entries: ToastEntry[]): Array<{ key: string; toast: ToastProps }> {
  const unique = new Map<string, ToastProps>();
  for (const entry of entries) {
    const key = toastIdentity(entry);
    const current = unique.get(key);
    if (!current) {
      unique.set(key, entry);
      continue;
    }
    unique.set(key, {
      ...current,
      action: current.action ?? entry.action,
      onDismiss: mergeDismissHandlers(current.onDismiss, entry.onDismiss),
    });
  }
  return [...unique.entries()].map(([key, toast]) => ({ key, toast }));
}

export function ToastProvider({ children }: { children: ReactNode }) {
  const [entries, setEntries] = useState<ToastEntry[]>([]);
  const toastSequence = useRef(0);
  const toastTimeouts = useRef(new Map<string, number>());
  const register = useCallback((entry: ToastEntry) => {
    setEntries((current) => [...current.filter((item) => item.id !== entry.id), entry]);
  }, []);
  const update = useCallback((id: string, props: ToastProps) => {
    setEntries((current) => {
      const entry = current.find((item) => item.id === id);
      if (!entry || sameToastProps(entry, props)) return current;
      return current.map((item) => item.id === id ? { ...item, ...props } : item);
    });
  }, []);
  const unregister = useCallback((id: string) => {
    const timeout = toastTimeouts.current.get(id);
    if (timeout !== undefined) {
      window.clearTimeout(timeout);
      toastTimeouts.current.delete(id);
    }
    setEntries((current) => current.filter((entry) => entry.id !== id));
  }, []);
  const show = useCallback((props: ToastProps) => {
    const id = `global-toast-${++toastSequence.current}`;
    const dismiss = () => {
      props.onDismiss?.();
      unregister(id);
    };
    setEntries((current) => [...current, { id, ...props, onDismiss: dismiss }]);
    const timeout = window.setTimeout(dismiss, 6000);
    toastTimeouts.current.set(id, timeout);
  }, [unregister]);
  useEffect(() => () => {
    for (const timeout of toastTimeouts.current.values()) window.clearTimeout(timeout);
    toastTimeouts.current.clear();
  }, []);
  const contextValue = useMemo(() => ({ register, update, unregister, show }), [register, show, unregister, update]);
  const visibleToasts = uniqueToastEntries(entries);
  const viewport = typeof document === "undefined" ? null : createPortal(
    <div className="appToastViewport">
      {visibleToasts.map(({ key, toast }) => <ToastCard key={key} {...toast} />)}
    </div>,
    document.body,
  );

  return (
    <ToastContext.Provider value={contextValue}>
      {children}
      {viewport}
    </ToastContext.Provider>
  );
}

export function Toast(props: ToastProps) {
  const context = useContext(ToastContext);
  const id = useId();
  const latestProps = useRef(props);
  latestProps.current = props;

  useEffect(() => {
    if (!context) return;
    context.register({ id, ...latestProps.current });
    return () => context.unregister(id);
  }, [context, id]);

  useEffect(() => {
    if (context) context.update(id, props);
  }, [context, id, props.action, props.message, props.onDismiss, props.tone]);

  return context ? null : <ToastCard {...props} standalone />;
}

export function useToast(): (props: ToastProps) => void {
  const context = useContext(ToastContext);
  if (!context) throw new Error("useToast must be used inside ToastProvider");
  return context.show;
}
