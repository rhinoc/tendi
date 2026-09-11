import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { Check, ChevronsUpDown, Search, X } from "lucide-react";
import {
  createContext,
  type ChangeEventHandler,
  type CSSProperties,
  type InputHTMLAttributes,
  type KeyboardEventHandler,
  type ReactNode,
  type RefObject,
  useCallback,
  useContext,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";

import { EASE_OUT, SPRING_PANEL } from "../../lib/ease.ts";
import "./MultiSelect.css";

type RegisteredMultiSelectItem = {
  value: string;
  label: string;
  keywords: string[];
  disabled: boolean;
  id: string;
  ref: RefObject<HTMLButtonElement | null>;
};

type MultiSelectContextValue = {
  open: boolean;
  setOpen: (open: boolean, restoreFocus?: boolean) => void;
  values: string[];
  toggle: (value: string) => void;
  remove: (value: string) => void;
  query: string;
  setQuery: (query: string) => void;
  activeValue: string | null;
  setActiveValue: (value: string | null) => void;
  moveActive: (direction: 1 | -1 | "first" | "last") => void;
  toggleActive: () => void;
  registerItem: (item: RegisteredMultiSelectItem) => void;
  unregisterItem: (value: string) => void;
  labelFor: (value: string) => string;
  isVisible: (value: string) => boolean;
  visibleCount: number;
  activeItemId?: string;
  triggerId: string;
  listId: string;
  inputId: string;
  disabled: boolean;
  reduce: boolean;
  triggerRef: RefObject<HTMLDivElement | null>;
  contentRef: RefObject<HTMLDivElement | null>;
  inputRef: RefObject<HTMLInputElement | null>;
};

const MultiSelectContext = createContext<MultiSelectContextValue | null>(null);

function useMultiSelectContext(component: string) {
  const context = useContext(MultiSelectContext);
  if (!context) throw new Error(`${component} must be used within <MultiSelect>`);
  return context;
}

function mergeStringValues(values: string[]) {
  return [...new Set(values)];
}

export type MultiSelectProps = {
  children: ReactNode;
  value?: string[];
  defaultValue?: string[];
  onValueChange?: (value: string[]) => void;
  disabled?: boolean;
  className?: string;
};

export function MultiSelect({
  children,
  value: controlledValue,
  defaultValue = [],
  onValueChange,
  disabled = false,
  className = "",
}: MultiSelectProps) {
  const reduce = useReducedMotion() ?? false;
  const baseId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [internalValue, setInternalValue] = useState(() => mergeStringValues(defaultValue));
  const [open, setOpenState] = useState(false);
  const [query, setQueryState] = useState("");
  const [activeValue, setActiveValue] = useState<string | null>(null);
  const [items, setItems] = useState<Map<string, RegisteredMultiSelectItem>>(() => new Map());
  const values = controlledValue ?? internalValue;

  const updateQuery = useCallback((next: string) => setQueryState(next), []);
  const setOpen = useCallback((next: boolean, restoreFocus = false) => {
    if (disabled && next) return;
    setOpenState(next);
    if (!next) updateQuery("");
    if (restoreFocus) {
      requestAnimationFrame(() => inputRef.current?.focus({ preventScroll: true }));
    }
  }, [disabled, updateQuery]);
  const commitValue = useCallback((next: string[]) => {
    const normalized = mergeStringValues(next);
    if (controlledValue === undefined) setInternalValue(normalized);
    onValueChange?.(normalized);
  }, [controlledValue, onValueChange]);
  const toggle = useCallback((value: string) => {
    if (items.get(value)?.disabled) return;
    commitValue(values.includes(value) ? values.filter((item) => item !== value) : [...values, value]);
    updateQuery("");
    requestAnimationFrame(() => inputRef.current?.focus({ preventScroll: true }));
  }, [commitValue, items, updateQuery, values]);
  const remove = useCallback((value: string) => {
    if (values.includes(value)) commitValue(values.filter((item) => item !== value));
  }, [commitValue, values]);
  const registerItem = useCallback((item: RegisteredMultiSelectItem) => {
    setItems((current) => {
      const existing = current.get(item.value);
      if (
        existing?.label === item.label
        && existing.disabled === item.disabled
        && existing.id === item.id
        && existing.ref === item.ref
        && existing.keywords.join("\u0000") === item.keywords.join("\u0000")
      ) return current;
      const next = new Map(current);
      next.set(item.value, item);
      return next;
    });
  }, []);
  const unregisterItem = useCallback((value: string) => {
    setItems((current) => {
      if (!current.has(value)) return current;
      const next = new Map(current);
      next.delete(value);
      return next;
    });
  }, []);
  const filterValue = query.trim().toLocaleLowerCase();
  const visibleItems = useMemo(() => Array.from(items.values()).filter((item) => {
    if (!filterValue) return true;
    return [item.value, item.label, ...item.keywords].join(" ").toLocaleLowerCase().includes(filterValue);
  }), [filterValue, items]);
  const enabledVisibleItems = useMemo(() => visibleItems.filter((item) => !item.disabled), [visibleItems]);
  const visibleValues = useMemo(() => new Set(visibleItems.map((item) => item.value)), [visibleItems]);
  const resolvedActiveValue = activeValue && enabledVisibleItems.some((item) => item.value === activeValue)
    ? activeValue
    : values.find((value) => enabledVisibleItems.some((item) => item.value === value)) ?? enabledVisibleItems[0]?.value ?? null;
  const moveActive = useCallback((direction: 1 | -1 | "first" | "last") => {
    if (!enabledVisibleItems.length) {
      setActiveValue(null);
      return;
    }
    setActiveValue((current) => {
      const currentIndex = enabledVisibleItems.findIndex((item) => item.value === (current ?? resolvedActiveValue));
      const last = enabledVisibleItems.length - 1;
      const nextIndex = direction === "first"
        ? 0
        : direction === "last"
          ? last
          : (Math.max(currentIndex, 0) + direction + enabledVisibleItems.length) % enabledVisibleItems.length;
      return enabledVisibleItems[nextIndex]?.value ?? null;
    });
  }, [enabledVisibleItems, resolvedActiveValue]);
  const toggleActive = useCallback(() => {
    if (resolvedActiveValue) toggle(resolvedActiveValue);
  }, [resolvedActiveValue, toggle]);

  useEffect(() => {
    if (!open) return;
    const frame = requestAnimationFrame(() => inputRef.current?.focus({ preventScroll: true }));
    return () => cancelAnimationFrame(frame);
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const isInside = (target: Node) => Boolean(rootRef.current?.contains(target) || contentRef.current?.contains(target));
    const onPointerDown = (event: PointerEvent) => {
      if (!isInside(event.target as Node)) setOpen(false);
    };
    const onFocusIn = (event: FocusEvent) => {
      if (!isInside(event.target as Node)) setOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("focusin", onFocusIn);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("focusin", onFocusIn);
    };
  }, [open, setOpen]);

  const context = useMemo<MultiSelectContextValue>(() => ({
    open,
    setOpen,
    values,
    toggle,
    remove,
    query,
    setQuery: updateQuery,
    activeValue: resolvedActiveValue,
    setActiveValue,
    moveActive,
    toggleActive,
    registerItem,
    unregisterItem,
    labelFor: (value) => items.get(value)?.label ?? value,
    isVisible: (value) => !filterValue || visibleValues.has(value),
    visibleCount: visibleItems.length,
    activeItemId: resolvedActiveValue ? items.get(resolvedActiveValue)?.id : undefined,
    triggerId: `${baseId}-trigger`,
    listId: `${baseId}-list`,
    inputId: `${baseId}-input`,
    disabled,
    reduce,
    triggerRef,
    contentRef,
    inputRef,
  }), [
    baseId,
    disabled,
    filterValue,
    items,
    moveActive,
    open,
    query,
    reduce,
    registerItem,
    remove,
    resolvedActiveValue,
    setOpen,
    toggle,
    toggleActive,
    unregisterItem,
    updateQuery,
    values,
    visibleItems.length,
    visibleValues,
  ]);

  return (
    <MultiSelectContext.Provider value={context}>
      <div ref={rootRef} className={`multiSelectRoot ${className}`.trim()}>{children}</div>
    </MultiSelectContext.Provider>
  );
}

export type MultiSelectTriggerProps = { children: ReactNode; className?: string };

export function MultiSelectTrigger({ children, className = "" }: MultiSelectTriggerProps) {
  const context = useMultiSelectContext("MultiSelectTrigger");
  return (
    <div
      ref={context.triggerRef}
      id={context.triggerId}
      data-state={context.open ? "open" : "closed"}
      className={`multiSelectTrigger ${className}`.trim()}
      role="button"
      tabIndex={context.disabled ? -1 : 0}
      aria-haspopup="listbox"
      aria-expanded={context.open}
      aria-controls={context.listId}
      onPointerDown={(event) => {
        const target = event.target as HTMLElement;
        if (context.disabled || target === context.inputRef.current || target.closest("[data-multi-select-remove]")) return;
        event.preventDefault();
        context.inputRef.current?.focus({ preventScroll: true });
        context.setOpen(true);
      }}
      onKeyDown={(event) => {
        const target = event.target as HTMLElement;
        if (context.disabled || target === context.inputRef.current || target.closest("[data-multi-select-remove]")) return;
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          if (!context.open) context.setOpen(true);
          else context.moveActive(event.key === "ArrowDown" ? 1 : -1);
        } else if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          if (context.open) context.toggleActive();
          else context.setOpen(true);
        } else if (event.key === "Escape" && context.open) {
          event.preventDefault();
          context.setOpen(false);
        }
      }}
    >
      <div className="multiSelectTriggerValues">{children}</div>
      <ChevronsUpDown className="multiSelectChevron" size={15} aria-hidden="true" />
    </div>
  );
}

export type MultiSelectValueProps = {
  placeholder?: ReactNode;
  children?: (value: string, label: string) => ReactNode;
  className?: string;
  hidePlaceholderWhenOpen?: boolean;
};

export function MultiSelectValue({
  placeholder = "Select options",
  children,
  className = "",
  hidePlaceholderWhenOpen = true,
}: MultiSelectValueProps) {
  const context = useMultiSelectContext("MultiSelectValue");
  const showPlaceholder = context.values.length === 0 && (!hidePlaceholderWhenOpen || !context.open);
  return (
    <div className={`multiSelectValues ${className}`.trim()}>
      <AnimatePresence initial={false} mode="popLayout">
        {showPlaceholder ? <span key="multi-select-placeholder" className="multiSelectPlaceholder">{placeholder}</span> : null}
        {context.values.map((value) => {
          const label = context.labelFor(value);
          return (
            <motion.span
              layout={context.reduce ? false : "position"}
              key={`multi-select-value-${value}`}
              initial={{ opacity: 0, transform: context.reduce ? "translateY(0)" : "translateY(2px)" }}
              animate={{ opacity: 1, transform: "translateY(0)" }}
              exit={context.reduce ? { opacity: 0 } : { opacity: 0, clipPath: "inset(0 0 0 100% round 0.5rem)" }}
              transition={context.reduce ? { duration: 0 } : { layout: SPRING_PANEL, opacity: { duration: 0.12, ease: EASE_OUT }, transform: { duration: 0.12, ease: EASE_OUT } }}
              className="multiSelectValue"
            >
              <span className="multiSelectValueLabel">{children ? children(value, label) : label}</span>
              <button
                type="button"
                data-multi-select-remove=""
                aria-label={`Remove ${label}`}
                disabled={context.disabled}
                onPointerDown={(event) => event.stopPropagation()}
                onClick={(event) => {
                  event.stopPropagation();
                  context.remove(value);
                  context.inputRef.current?.focus({ preventScroll: true });
                }}
                className="multiSelectRemove"
              >
                <X size={12} aria-hidden="true" />
              </button>
            </motion.span>
          );
        })}
      </AnimatePresence>
    </div>
  );
}

export type MultiSelectInputProps = Omit<InputHTMLAttributes<HTMLInputElement>, "defaultValue" | "value" | "onChange" | "onKeyDown"> & {
  showIcon?: boolean;
  onChange?: ChangeEventHandler<HTMLInputElement>;
  onKeyDown?: KeyboardEventHandler<HTMLInputElement>;
};

export function MultiSelectInput({
  className = "",
  "aria-label": ariaLabel = "Search options",
  onChange,
  onClick,
  onFocus,
  onKeyDown,
  onPointerDown,
  placeholder = "Search…",
  showIcon = false,
  ...props
}: MultiSelectInputProps) {
  const context = useMultiSelectContext("MultiSelectInput");
  const handleKeyDown: KeyboardEventHandler<HTMLInputElement> = (event) => {
    onKeyDown?.(event);
    if (event.defaultPrevented) return;
    if (event.key === "Backspace" && !context.query && context.values.length) {
      event.preventDefault();
      context.remove(context.values.at(-1) ?? "");
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!context.open) context.setOpen(true);
      else context.moveActive(event.key === "ArrowDown" ? 1 : -1);
    } else if (event.key === "Home" && context.open) {
      event.preventDefault();
      context.moveActive("first");
    } else if (event.key === "End" && context.open) {
      event.preventDefault();
      context.moveActive("last");
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (context.open) context.toggleActive();
      else context.setOpen(true);
    } else if (event.key === "Escape" && context.open) {
      event.preventDefault();
      context.setOpen(false, true);
    }
  };
  return (
    <div className="multiSelectInputWrap">
      {showIcon ? <Search size={14} aria-hidden="true" className="multiSelectSearchIcon" /> : null}
      <input
        {...props}
        id={context.inputId}
        ref={context.inputRef}
        role="combobox"
        aria-label={ariaLabel}
        aria-autocomplete="list"
        aria-expanded={context.open}
        aria-controls={context.listId}
        aria-activedescendant={context.open ? context.activeItemId : undefined}
        autoComplete="off"
        disabled={context.disabled}
        value={context.query}
        placeholder={context.values.length ? "" : placeholder}
        onPointerDown={(event) => {
          onPointerDown?.(event);
          if (!event.defaultPrevented) context.setOpen(true);
        }}
        onFocus={(event) => {
          context.setOpen(true);
          onFocus?.(event);
        }}
        onClick={(event) => {
          context.setOpen(true);
          onClick?.(event);
        }}
        onChange={(event) => {
          context.setOpen(true);
          context.setQuery(event.target.value);
          onChange?.(event);
        }}
        onKeyDown={handleKeyDown}
        className={`multiSelectInput ${className}`.trim()}
      />
    </div>
  );
}

type MultiSelectPosition = { left: number; top: number; width: number; side: "top" | "bottom" };

function useMultiSelectPosition(
  triggerRef: RefObject<HTMLDivElement | null>,
  contentRef: RefObject<HTMLDivElement | null>,
  open: boolean,
) {
  const [position, setPosition] = useState<MultiSelectPosition | null>(null);
  const update = useCallback(() => {
    const trigger = triggerRef.current;
    const content = contentRef.current;
    if (!trigger || !content) return;
    const rect = trigger.getBoundingClientRect();
    const width = rect.width;
    const height = Math.min(content.scrollHeight, window.innerHeight - 24);
    const below = window.innerHeight - (rect.top + rect.height);
    const above = rect.top;
    const side: MultiSelectPosition["side"] = below < height + 6 && above > below ? "top" : "bottom";
    const desiredLeft = rect.left;
    const left = Math.min(Math.max(desiredLeft, 8), Math.max(8, window.innerWidth - width - 8));
    const top = side === "bottom" ? rect.top + rect.height + 6 : rect.top - height - 6;
    const next = { left, top, width, side };
    setPosition((current) => current && current.left === left && current.top === top && current.width === width && current.side === side ? current : next);
  }, [contentRef, triggerRef]);
  useLayoutEffect(() => {
    update();
    if (!open) return;
    const trigger = triggerRef.current;
    const content = contentRef.current;
    const observer = typeof ResizeObserver === "function" ? new ResizeObserver(update) : null;
    if (trigger) observer?.observe(trigger);
    if (content) observer?.observe(content);
    window.addEventListener("scroll", update, true);
    window.addEventListener("resize", update);
    return () => {
      observer?.disconnect();
      window.removeEventListener("scroll", update, true);
      window.removeEventListener("resize", update);
    };
  }, [contentRef, open, triggerRef, update]);
  return position;
}

export type MultiSelectContentProps = { children: ReactNode; className?: string };

export function MultiSelectContent({ children, className = "" }: MultiSelectContentProps) {
  const context = useMultiSelectContext("MultiSelectContent");
  const position = useMultiSelectPosition(context.triggerRef, context.contentRef, context.open);
  if (typeof document === "undefined") return null;
  return createPortal(
    <motion.div
      ref={context.contentRef}
      data-state={context.open ? "open" : "closed"}
      data-multi-select-content=""
      data-no-drag
      aria-hidden={!context.open}
      inert={!context.open}
      className={`multiSelectContent ${className}`.trim()}
      style={{
        left: position?.left ?? 0,
        top: position?.top ?? 0,
        width: position?.width ?? 0,
        visibility: position ? "visible" : "hidden",
        pointerEvents: context.open && position ? "auto" : "none",
        transformOrigin: position?.side === "top" ? "bottom center" : "top center",
      } as CSSProperties}
      initial={false}
      animate={{ opacity: context.open ? 1 : 0, y: context.open ? 0 : -4, scale: context.open ? 1 : 0.985 }}
      transition={context.reduce ? { duration: 0 } : { duration: 0.18, ease: EASE_OUT }}
    >
      {children}
    </motion.div>,
    document.body,
  );
}

export type MultiSelectListProps = { children: ReactNode; ariaLabel?: string; className?: string };

export function MultiSelectList({ children, ariaLabel = "Options", className = "" }: MultiSelectListProps) {
  const context = useMultiSelectContext("MultiSelectList");
  return <div id={context.listId} role="listbox" aria-label={ariaLabel} aria-multiselectable="true" className={`multiSelectList ${className}`.trim()}>{children}</div>;
}

export type MultiSelectItemProps = {
  value: string;
  children: ReactNode;
  textValue?: string;
  keywords?: string[];
  disabled?: boolean;
  onSelect?: (value: string) => void;
  className?: string;
};

export function MultiSelectItem({
  value,
  children,
  textValue,
  keywords = [],
  disabled = false,
  onSelect,
  className = "",
}: MultiSelectItemProps) {
  const context = useMultiSelectContext("MultiSelectItem");
  const id = useId();
  const itemRef = useRef<HTMLButtonElement>(null);
  const label = textValue ?? (typeof children === "string" ? children : value);
  const keywordKey = keywords.join("\u0000");
  const normalizedKeywords = useMemo(() => keywordKey ? keywordKey.split("\u0000") : [], [keywordKey]);
  const visible = context.isVisible(value);
  const selected = context.values.includes(value);
  const active = context.activeValue === value;

  useLayoutEffect(() => {
    context.registerItem({ value, label, keywords: normalizedKeywords, disabled, id, ref: itemRef });
    return () => context.unregisterItem(value);
  }, [context.registerItem, context.unregisterItem, disabled, id, label, normalizedKeywords, value]);

  if (!visible) return null;
  return (
    <button
      ref={itemRef}
      id={id}
      type="button"
      role="option"
      aria-selected={selected}
      disabled={disabled}
      tabIndex={-1}
      data-multi-select-item=""
      data-active={active || undefined}
      data-highlighted={active || undefined}
      data-selected={selected || undefined}
      data-state={selected ? "checked" : "unchecked"}
      data-disabled={disabled || undefined}
      onPointerMove={() => !disabled && context.setActiveValue(value)}
      onPointerDown={(event) => event.preventDefault()}
      onClick={() => {
        if (disabled) return;
        onSelect?.(value);
        context.toggle(value);
      }}
      className={`multiSelectItem ${className}`.trim()}
    >
      <span className="multiSelectItemContent">{children}</span>
      <motion.span
        aria-hidden="true"
        initial={false}
        animate={{ opacity: selected ? 1 : 0, transform: selected ? "scale(1)" : "scale(.82)" }}
        transition={context.reduce ? { duration: 0 } : { duration: 0.14, ease: EASE_OUT }}
        className="multiSelectItemCheck"
      >
        <Check size={14} />
      </motion.span>
    </button>
  );
}

export type MultiSelectEmptyProps = { children?: ReactNode; className?: string };

export function MultiSelectEmpty({ children = "No options found.", className = "" }: MultiSelectEmptyProps) {
  const context = useMultiSelectContext("MultiSelectEmpty");
  if (context.visibleCount > 0) return null;
  return <div role="status" className={`multiSelectEmpty ${className}`.trim()}>{children}</div>;
}
