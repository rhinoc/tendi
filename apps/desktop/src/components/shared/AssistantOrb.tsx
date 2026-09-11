import { ArrowUp, ArrowUpRight, Menu, MessageSquare, Pin, Square, SquarePen, X } from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { Fragment, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type FormEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";

import type { AssistantChatSession, AssistantContext, AssistantMessage, AssistantStreamEvent } from "../../lib/generated/runtime-types.ts";
import { assistantAskRequest, assistantPromptSuggestions, canMarkAssistantReplyRead, persistAssistantAgent, readAssistantAgent, type AssistantPromptSuggestion } from "../../lib/assistant.ts";
import { agentIdentityKey, isVisibleAgent } from "../../lib/agents.ts";
import { BLOUB_LOADING_SHAPE_PATHS, bloubLoadingShapeSequence, BloubFaceEngine, type BloubFaceFrame, type BloubFaceMood } from "../../lib/bloub-face.ts";
import { askAssistantStream, cancelAssistant, loadAssistantChatSessions } from "../../lib/runtime-gateway.ts";
import { logger } from "../../lib/logger.ts";
import { AgentOptionLabel } from "./AgentOptionLabel.tsx";
import { Button } from "./Button.tsx";
import { EmptyState } from "./EmptyState.tsx";
import { IconButton } from "./IconButton.tsx";
import { LoadingIcon } from "./LoadingIcon.tsx";
import { SelectControl, type SelectOption } from "./SelectControl.tsx";
import { SharedLayoutBg } from "./SharedLayoutBg.tsx";
import { TiptapMarkdownPreview } from "./TiptapMarkdownPreview.tsx";
import { ToolCall } from "./ToolCall.tsx";
import { Toast } from "./Toast.tsx";
import "./AssistantOrb.css";

type AssistantOrbProps = {
  agent: string;
  agentLabel: string;
  agentOptions: SelectOption[];
  workspace: string;
  getContext: () => AssistantContext;
};

type AssistantMessageFrom = "user" | "assistant";
type AssistantToolStatus = "running" | "complete" | "error" | "cancelled";
type AssistantToolActivity = {
  id: string;
  title: string;
  input: string;
  output: string;
  status: AssistantToolStatus;
};

const ASSISTANT_SESSION_TITLE_LIMIT = 52;
const ASSISTANT_PINNED_STORAGE_KEY = "tendi.assistant.pinned";

function readAssistantPinned(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(ASSISTANT_PINNED_STORAGE_KEY) === "true";
  } catch {
    return false;
  }
}

function persistAssistantPinned(pinned: boolean): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(ASSISTANT_PINNED_STORAGE_KEY, String(pinned));
  } catch {
    // The in-memory state remains the source of truth if browser storage is unavailable.
  }
}

function assistantSessionTitle(messages: readonly AssistantMessage[]): string {
  const firstUserMessage = messages.find((message) => message.role === "user")?.content.trim().replace(/\s+/g, " ") ?? "";
  if (!firstUserMessage) return "New conversation";
  if (firstUserMessage.length <= ASSISTANT_SESSION_TITLE_LIMIT) return firstUserMessage;
  return `${firstUserMessage.slice(0, ASSISTANT_SESSION_TITLE_LIMIT - 1)}…`;
}

function newAssistantChatSessionId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `assistant-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function mergeAssistantChatSessions(
  persisted: readonly AssistantChatSession[],
  local: readonly AssistantChatSession[],
): AssistantChatSession[] {
  const localById = new Map(local.map((session) => [session.id, session]));
  const merged = persisted.map((session) => {
    const localSession = localById.get(session.id);
    if (!localSession || localSession.messages.length < session.messages.length) return session;
    return localSession;
  });
  const persistedIds = new Set(persisted.map((session) => session.id));
  return [...local.filter((session) => !persistedIds.has(session.id)), ...merged];
}

function AssistantChatHistory({
  sessions,
  activeSessionId,
  onSelect,
}: {
  sessions: readonly AssistantChatSession[];
  activeSessionId: string | null;
  onSelect: (sessionId: string) => void;
}) {
  return (
    <section id="assistant-chat-history" className="assistantChatHistory" aria-label="Assistant conversation history">
      <h2 className="assistantChatHistoryHeading">History</h2>
      {sessions.length === 0 ? (
        <EmptyState
          className="assistantChatHistoryEmpty"
          compact
          icon={<MessageSquare size={22} strokeWidth={1.75} />}
          iconTone="muted"
          title="No conversations yet"
          description="Send a message to start a conversation."
        />
      ) : (
        <ul className="assistantChatHistoryList">
          {sessions.map((session) => {
            return (
              <li key={session.id}>
                <button
                  type="button"
                  className={`assistantChatHistoryItem${session.id === activeSessionId ? " isActive" : ""}`}
                  aria-current={session.id === activeSessionId ? "true" : undefined}
                  onClick={() => onSelect(session.id)}
                >
                  <span className="assistantChatHistoryItemTitle">{assistantSessionTitle(session.messages)}</span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

const MESSAGE_POP_UP = {
  type: "spring",
  stiffness: 480,
  damping: 32,
  mass: 0.62,
} as const;

const ORB_SVG_WIDTH = 64;
const ORB_CENTER_Y = 40;
const ORB_RADIUS = 18;
const ORB_REST_X = 64;
const ORB_OPEN_X = 38;
const ORB_BREAK_AT = 0.76;
const ORB_ANIMATION_DURATION = 640;
const PANEL_RADIUS = 24;
const PANEL_INITIAL_RADIUS = 32;

function nextBloubShapeIndex(currentShapeIndex: number): number {
  const shapeCount = BLOUB_LOADING_SHAPE_PATHS.length;
  return (
    currentShapeIndex
    + 1
    + Math.floor(Math.random() * (shapeCount - 1))
  ) % shapeCount;
}
const PANEL_MORPH_TRANSITION = {
  type: "spring",
  stiffness: 420,
  damping: 34,
  mass: 0.7,
} as const;
const PANEL_EXIT_TRANSITION = {
  duration: 0.18,
  ease: "easeOut",
} as const;
const clampUnit = (value: number) => Math.min(1, Math.max(0, value));
const mixNumber = (from: number, to: number, amount: number) => from + (to - from) * amount;
const easeLiquid = (value: number) => (
  value < 0.5
    ? 4 * value * value * value
    : 1 - ((-2 * value + 2) ** 3) / 2
);

function pointOnOrb(cx: number, angle: number) {
  const radians = (angle * Math.PI) / 180;
  return {
    x: cx + ORB_RADIUS * Math.cos(radians),
    y: ORB_CENTER_Y + ORB_RADIUS * Math.sin(radians),
  };
}

function orbArcTangent(angle: number) {
  const radians = (angle * Math.PI) / 180;
  return {
    x: Math.sin(radians),
    y: -Math.cos(radians),
  };
}

function attachedOrbPath(cx: number, progress: number) {
  const local = clampUnit(progress / ORB_BREAK_AT);
  const pinch = easeLiquid(local ** 2.8);
  const angle = mixNumber(130, 6, pinch);
  const edgeSpan = mixNumber(30, 2.4, pinch);
  const upper = pointOnOrb(cx, -angle);
  const lower = pointOnOrb(cx, angle);
  const upperTangent = orbArcTangent(-angle);
  const lowerTangent = orbArcTangent(angle);
  const edgeTop = ORB_CENTER_Y - edgeSpan;
  const edgeBottom = ORB_CENTER_Y + edgeSpan;
  const arcIsLarge = angle < 90 ? 1 : 0;
  const circleHandle = mixNumber(7, 1.4, pinch);
  const edgeHandle = mixNumber(11, 1.4, pinch);

  return [
    `M ${ORB_SVG_WIDTH} ${edgeTop}`,
    `C ${ORB_SVG_WIDTH} ${edgeTop + edgeHandle}`,
    `${upper.x - upperTangent.x * circleHandle} ${upper.y - upperTangent.y * circleHandle}`,
    `${upper.x} ${upper.y}`,
    `A ${ORB_RADIUS} ${ORB_RADIUS} 0 ${arcIsLarge} 0 ${lower.x} ${lower.y}`,
    `C ${lower.x + lowerTangent.x * circleHandle} ${lower.y + lowerTangent.y * circleHandle}`,
    `${ORB_SVG_WIDTH} ${edgeBottom - edgeHandle}`,
    `${ORB_SVG_WIDTH} ${edgeBottom}`,
    "Z",
  ].join(" ");
}

function AssistantOrbFace({ mood, lookInward = false }: { mood: BloubFaceMood; lookInward?: boolean }) {
  const reduceMotion = useReducedMotion() ?? false;
  const engineRef = useRef<BloubFaceEngine | null>(null);
  const clockRef = useRef(0);
  const eyeRefs = useRef<Array<SVGPathElement | null>>([]);
  const dotRefs = useRef<Array<SVGCircleElement | null>>([]);
  if (!engineRef.current) engineRef.current = new BloubFaceEngine(mood, lookInward);
  const initialFrameRef = useRef<BloubFaceFrame | null>(null);
  if (!initialFrameRef.current) initialFrameRef.current = engineRef.current.sample(0, reduceMotion);

  const applyFrame = useCallback((nextFrame: BloubFaceFrame) => {
    nextFrame.eyes.forEach((eyeFrame, index) => {
      const eye = eyeRefs.current[index];
      if (!eye) return;
      eye.setAttribute("d", eyeFrame.d);
      eye.setAttribute("transform", eyeFrame.matrix);
      eye.setAttribute("opacity", `${eyeFrame.opacity}`);
    });
    nextFrame.dots.forEach((dot, index) => {
      const element = dotRefs.current[index];
      if (!element) return;
      element.setAttribute("cx", `${dot.x}`);
      element.setAttribute("cy", `${dot.y}`);
      element.setAttribute("r", `${dot.r}`);
      element.setAttribute("opacity", `${dot.opacity}`);
    });
  }, []);

  useLayoutEffect(() => {
    const now = clockRef.current;
    engineRef.current?.setMood(mood, now, lookInward);
    applyFrame(engineRef.current!.sample(now, reduceMotion));
  }, [applyFrame, lookInward, mood, reduceMotion]);

  useEffect(() => {
    if (reduceMotion) return undefined;
    let animationFrame = 0;
    let lastFrameAt = 0;
    const sample = (timestamp: number) => {
      const elapsed = lastFrameAt ? Math.min(timestamp - lastFrameAt, 64) : 0;
      lastFrameAt = timestamp;
      clockRef.current += elapsed;
      applyFrame(engineRef.current!.sample(clockRef.current));
      animationFrame = window.requestAnimationFrame(sample);
    };
    animationFrame = window.requestAnimationFrame(sample);
    return () => window.cancelAnimationFrame(animationFrame);
  }, [applyFrame, reduceMotion]);

  return (
    <svg className="assistantOrbFace" viewBox="-10 -10 20 20" data-mood={mood} aria-hidden="true">
      {initialFrameRef.current.eyes.map((_, index) => (
        <path
          ref={(element) => { eyeRefs.current[index] = element; }}
          className="assistantOrbFaceEye"
          key={index}
        />
      ))}
      {initialFrameRef.current.dots.map((_, index) => (
        <circle
          ref={(element) => { dotRefs.current[index] = element; }}
          className="assistantOrbFaceDot"
          key={index}
        />
      ))}
    </svg>
  );
}

function AssistantOrbLoadingShape({ shapeIndex }: { shapeIndex: number }) {
  const reduceMotion = useReducedMotion() ?? false;
  const sequence = bloubLoadingShapeSequence(shapeIndex);
  const initialPath = sequence[0]!;

  return (
    <svg className="assistantComposerLoadingShape" viewBox="-12 -12 24 24" aria-hidden="true">
      <motion.path
        d={initialPath}
        transform="scale(1.2)"
        animate={reduceMotion ? { d: initialPath } : { d: sequence }}
        transition={reduceMotion
          ? { duration: 0 }
          : { duration: 3.2, ease: "easeInOut", repeat: Number.POSITIVE_INFINITY, repeatType: "loop" }}
      />
    </svg>
  );
}

function AssistantOrbPendantShape({ fromShapeIndex, shapeIndex }: { fromShapeIndex: number; shapeIndex: number }) {
  const reduceMotion = useReducedMotion() ?? false;
  const count = BLOUB_LOADING_SHAPE_PATHS.length;
  const from = ((fromShapeIndex % count) + count) % count;
  const to = ((shapeIndex % count) + count) % count;

  return (
    <svg className="assistantComposerShape" viewBox="-12 -12 24 24" aria-hidden="true">
      <motion.path
        d={BLOUB_LOADING_SHAPE_PATHS[from]!}
        animate={reduceMotion ? { d: BLOUB_LOADING_SHAPE_PATHS[to]! } : { d: BLOUB_LOADING_SHAPE_PATHS[to]! }}
        transform="scale(1.2)"
        transition={reduceMotion ? { duration: 0 } : { duration: 0.22, ease: [0.22, 1, 0.36, 1] }}
      />
    </svg>
  );
}

function AssistantOrbTrigger({
  onCaptureContext,
  onOpen,
  notification,
  shapeIndex,
}: {
  onCaptureContext: () => void;
  onOpen: () => void;
  notification: boolean;
  shapeIndex: number;
}) {
  const reduceMotion = useReducedMotion() ?? false;
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const [progress, setProgress] = useState(0);
  const progressRef = useRef(0);
  const expanded = hovered || focused;

  useEffect(() => {
    const target = expanded ? 1 : 0;
    const from = progressRef.current;
    if (from === target) return undefined;

    if (reduceMotion) {
      progressRef.current = target;
      setProgress(target);
      return undefined;
    }

    let frame = 0;
    let startedAt = 0;
    const duration = Math.max(ORB_ANIMATION_DURATION * Math.abs(target - from), 1);
    const animateOrb = (timestamp: number) => {
      if (!startedAt) startedAt = timestamp;
      const elapsed = clampUnit((timestamp - startedAt) / duration);
      const nextProgress = mixNumber(from, target, easeLiquid(elapsed));
      progressRef.current = nextProgress;
      setProgress(nextProgress);
      if (elapsed < 1) frame = window.requestAnimationFrame(animateOrb);
    };
    frame = window.requestAnimationFrame(animateOrb);
    return () => window.cancelAnimationFrame(frame);
  }, [expanded, reduceMotion]);

  const cx = mixNumber(ORB_REST_X, ORB_OPEN_X, easeLiquid(progress));
  const attachedPath = attachedOrbPath(cx, progress);
  const detachedPath = BLOUB_LOADING_SHAPE_PATHS[shapeIndex % BLOUB_LOADING_SHAPE_PATHS.length] ?? BLOUB_LOADING_SHAPE_PATHS[0];
  const detached = progress >= ORB_BREAK_AT;
  const faceDetached = progress >= 0.98;
  const mood: BloubFaceMood = notification ? "surpris" : faceDetached ? "attentif" : "neutre";
  const handlePointerEnter = () => {
    setHovered(true);
  };
  const handlePointerLeave = () => {
    setHovered(false);
  };

  return (
    <div
      className="assistantOrbTriggerArea"
      data-state={progress >= ORB_BREAK_AT ? "detached" : progress > 0 ? "stretching" : "attached"}
      onPointerEnter={handlePointerEnter}
      onPointerLeave={handlePointerLeave}
    >
      <svg className="assistantOrbLiquid" viewBox="0 0 64 80" aria-hidden="true">
        {detached ? (
          <motion.path
            d={BLOUB_LOADING_SHAPE_PATHS[0]!}
            animate={{ d: detachedPath }}
            transition={reduceMotion ? { duration: 0 } : { duration: 0.56, ease: [0.22, 1, 0.36, 1] }}
            transform={`translate(${cx} ${ORB_CENTER_Y}) scale(${ORB_RADIUS / 9})`}
          />
        ) : (
          <path d={attachedPath} />
        )}
        {notification ? (
          <circle
            className="assistantOrbNotificationDot"
            cx={Math.min(cx + ORB_RADIUS * 0.72, ORB_SVG_WIDTH - 4)}
            cy={ORB_CENTER_Y - ORB_RADIUS * 0.72}
            r="4"
          />
        ) : null}
      </svg>
      <button
        type="button"
        className="assistantOrbButton"
        style={{ left: cx - ORB_RADIUS }}
        aria-label="Open Tendi assistant"
        aria-expanded={false}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        onMouseDown={onCaptureContext}
        onClick={onOpen}
      >
        <span className="assistantOrbMark" aria-hidden="true">
          <AssistantOrbFace mood={mood} lookInward={!faceDetached} />
        </span>
      </button>
    </div>
  );
}

function AgentMessage({
  id,
  from,
  animateIn = false,
  children,
}: {
  id: string;
  from: AssistantMessageFrom;
  animateIn?: boolean;
  children: ReactNode;
}) {
  const reduce = useReducedMotion() ?? false;

  return (
    <motion.article
      id={id}
      data-slot="message"
      data-from={from}
      aria-label={`${from} message`}
      initial={
        animateIn && !reduce
          ? { opacity: 0, transform: "translateY(8px) scale(0.95)" }
          : false
      }
      animate={
        animateIn && !reduce
          ? { opacity: 1, transform: "translateY(0px) scale(1)" }
          : { opacity: 1 }
      }
      transition={reduce ? { duration: 0.12 } : MESSAGE_POP_UP}
      style={{ transformOrigin: from === "user" ? "100% 100%" : "0% 100%" }}
      className={`assistantMessageRow assistantMessageRow--${from}`}
    >
      {children}
    </motion.article>
  );
}

function AgentMessageContent({ children }: { children: ReactNode }) {
  return (
    <div data-slot="message-content" className="assistantMessageContent">
      {children}
    </div>
  );
}

function AgentMessageBubble({
  variant,
  children,
}: {
  variant: AssistantMessageFrom;
  children: ReactNode;
}) {
  return (
    <div
      data-slot="message-bubble"
      data-align={variant === "user" ? "end" : "start"}
      data-variant={variant === "user" ? "solid" : "soft"}
      className={`assistantMessageBubble assistantMessageBubble--${variant}`}
    >
      {children}
    </div>
  );
}

function AgentMessageBubbleContent({ children }: { children: ReactNode }) {
  return (
    <div data-slot="message-bubble-content" className="assistantMessageBubbleContent">
      <div className="assistantMessageBubbleText">{children}</div>
    </div>
  );
}

function AgentMessageMarkdown({ content }: { content: string }) {
  return <TiptapMarkdownPreview content={content} stripFrontmatter={false} immediatelyRender />;
}

function AgentStreamingResponse({
  status,
  children,
}: {
  status: "streaming" | "complete";
  children: ReactNode;
}) {
  return (
    <div data-state={status} aria-busy={status === "streaming"} className="assistantStreamingResponse">
      <div aria-live="off" className="assistantStreamingResponseText">
        {children}
      </div>
    </div>
  );
}

function AgentToolActivityList({ activities }: { activities: readonly AssistantToolActivity[] }) {
  return (
    <div className="assistantToolActivityList" aria-label="Tool activity">
      {activities.map((activity) => (
        <ToolCall
          key={activity.id}
          item={{ command: activity.input, result: activity.output }}
          itemKey={activity.id}
          summary={activity.title}
          status={activity.status}
        />
      ))}
    </div>
  );
}

function updateAssistantToolActivities(
  current: AssistantToolActivity[],
  event: AssistantStreamEvent,
  requestId: number,
): AssistantToolActivity[] {
  if (event.kind !== "tool-call" && event.kind !== "tool-result") return current;
  const title = event.detail?.trim() || "Tool call";
  const matchingRunningIndex = (() => {
    for (let index = current.length - 1; index >= 0; index -= 1) {
      if (current[index].status === "running" && (title === "Tool call" || current[index].title === title)) return index;
    }
    return -1;
  })();
  if (event.kind === "tool-call") {
    if (matchingRunningIndex >= 0) {
      return current.map((activity, index) => index === matchingRunningIndex
        ? { ...activity, input: event.text?.trim() || activity.input }
        : activity);
    }
    return [
      ...current,
      {
        id: `${requestId}-${current.length}`,
        title,
        input: event.text?.trim() || "",
        output: "",
        status: "running",
      },
    ];
  }
  if (matchingRunningIndex >= 0) {
    return current.map((activity, index) => index === matchingRunningIndex
      ? { ...activity, output: event.text?.trim() || activity.output, status: "complete" }
      : activity);
  }
  return [
    ...current,
    {
      id: `${requestId}-${current.length}`,
      title,
      input: "",
      output: event.text?.trim() || "",
      status: "complete",
    },
  ];
}

function AgentMessageTyping({ label = "Responding" }: { label?: string }) {
  const reduce = useReducedMotion() ?? false;

  return (
    <span data-slot="message-typing" className="assistantMessageTyping">
      <span className="assistantScreenReaderOnly">{label}</span>
      {[0, 1, 2].map((index) => (
        <motion.span
          key={index}
          aria-hidden="true"
          className="assistantTypingDot"
          animate={
            reduce
              ? { opacity: 0.45 }
              : { opacity: [0.28, 0.85, 0.28], y: [0, -2, 0] }
          }
          transition={{
            duration: 1.05,
            ease: "easeOut",
            repeat: Number.POSITIVE_INFINITY,
            delay: index * 0.14,
          }}
        />
      ))}
    </span>
  );
}

function displayAgent(agent: string, label: string): string {
  return label.trim() || agent.trim() || "No agent";
}

function requestAssistantCancellation(sessionId: string | null): void {
  if (!sessionId) return;
  void cancelAssistant(sessionId).catch((cancelError) => {
    logger.warn("assistant cancellation request failed", { error: cancelError });
  });
}

export function AssistantOrb({ agent, agentLabel, agentOptions, workspace, getContext }: AssistantOrbProps) {
  const reduceMotion = useReducedMotion() ?? false;
  const [open, setOpen] = useState(false);
  const [pinned, setPinned] = useState(readAssistantPinned);
  const [selectedAgent, setSelectedAgent] = useState(() => readAssistantAgent() || agent);
  const [draft, setDraft] = useState("");
  const [sessions, setSessions] = useState<AssistantChatSession[]>([]);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [streamingAnswer, setStreamingAnswer] = useState("");
  const [streamingProgress, setStreamingProgress] = useState("");
  const [toolActivities, setToolActivities] = useState<AssistantToolActivity[]>([]);
  const [error, setError] = useState("");
  const [failedMessage, setFailedMessage] = useState("");
  const [composerFocused, setComposerFocused] = useState(false);
  const [contextAtOpen, setContextAtOpen] = useState<AssistantContext | null>(null);
  const [bloubShapeIndex, setBloubShapeIndex] = useState(0);
  const [petFromShapeIndex, setPetFromShapeIndex] = useState(0);
  const [notification, setNotification] = useState(false);
  const contextCapturedRef = useRef(false);
  const openRequestedRef = useRef(false);
  const composingRef = useRef(false);
  const composerTextareaRef = useRef<HTMLTextAreaElement>(null);
  const composerMeasurementRef = useRef<HTMLDivElement>(null);
  const messageViewportRef = useRef<HTMLElement>(null);
  const messageContentRef = useRef<HTMLDivElement>(null);
  const assistantHostRef = useRef<HTMLDivElement>(null);
  const followMessagesRef = useRef(true);
  const programmaticScrollRef = useRef(false);
  const scrollTimerRef = useRef<number | undefined>(undefined);
  const requestIdRef = useRef(0);
  const pendingMessageRef = useRef("");

  const activeSession = activeSessionId
    ? sessions.find((session) => session.id === activeSessionId)
    : undefined;
  const messages = activeSession?.messages ?? [];
  const visibleAgentOptions = agentOptions.filter((option) => isVisibleAgent(option.value));

  useEffect(() => {
    if (visibleAgentOptions.length === 0) return;
    setSelectedAgent((current) => {
      if (visibleAgentOptions.some((option) => option.value === current)) return current;
      return visibleAgentOptions.some((option) => option.value === agent)
        ? agent
        : visibleAgentOptions[0]?.value ?? current;
    });
  }, [agent, visibleAgentOptions]);

  useEffect(() => {
    if (selectedAgent) persistAssistantAgent(selectedAgent);
  }, [selectedAgent]);

  useEffect(() => {
    persistAssistantPinned(pinned);
  }, [pinned]);

  useEffect(() => {
    let cancelled = false;
    void loadAssistantChatSessions()
      .then((persisted) => {
        if (cancelled) return;
        setSessions((current) => mergeAssistantChatSessions(persisted, current));
      })
      .catch((loadError) => {
        if (!cancelled) setError(`Could not load assistant history: ${loadError instanceof Error ? loadError.message : `${loadError}`}`);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const activeAgent = visibleAgentOptions.some((option) => option.value === selectedAgent)
    ? selectedAgent
    : isVisibleAgent(agent)
      ? agentIdentityKey(agent)
      : visibleAgentOptions[0]?.value ?? "";
  const activeAgentAvailable = visibleAgentOptions.some((option) => option.value === activeAgent);
  const activeAgentOption = visibleAgentOptions.find((option) => option.value === activeAgent);
  const activeAgentLabel = activeAgentOption?.label ?? displayAgent(activeAgent, activeAgent === agent ? agentLabel : "");
  const composerAgentOptions = activeAgent && !activeAgentAvailable
    ? [{ value: activeAgent, label: `${activeAgentLabel} (unavailable)` }, ...visibleAgentOptions]
    : visibleAgentOptions;
  const composerMood: BloubFaceMood = !activeAgentAvailable || error
    ? "confus"
    : busy
      ? "thinking"
      : draft.trim()
        ? "attentif"
        : composerFocused
          ? "curieux"
          : messages.at(-1)?.role === "assistant"
            ? "heureux"
            : "neutre";

  const openAssistant = () => {
    if (openRequestedRef.current) return;
    openRequestedRef.current = true;
    setNotification(false);
    if (!contextCapturedRef.current) setContextAtOpen(getContext());
    contextCapturedRef.current = false;
    setOpen(true);
  };

  const changeComposerBloubShape = () => {
    if (busy) return;
    const nextShapeIndex = nextBloubShapeIndex(bloubShapeIndex);
    setPetFromShapeIndex(bloubShapeIndex);
    setBloubShapeIndex(nextShapeIndex);
  };

  const captureContextBeforeFocus = () => {
    setContextAtOpen(getContext());
    contextCapturedRef.current = true;
  };

  const closeAssistant = useCallback(() => {
    openRequestedRef.current = false;
    setComposerFocused(false);
    setHistoryOpen(false);
    setOpen(false);
  }, []);

  useEffect(() => {
    if (!open) return undefined;
    const handleOutsidePointerDown = (event: globalThis.PointerEvent) => {
      if (pinned) return;
      const host = assistantHostRef.current;
      const target = event.target;
      if (!host || !(target instanceof Node) || host.contains(target)) return;
      closeAssistant();
    };
    document.addEventListener("pointerdown", handleOutsidePointerDown);
    return () => document.removeEventListener("pointerdown", handleOutsidePointerDown);
  }, [closeAssistant, open, pinned]);

  const updateSessionMessages = useCallback((sessionId: string, update: (current: AssistantMessage[]) => AssistantMessage[]) => {
    setSessions((current) => {
      const existing = current.find((session) => session.id === sessionId);
      const updated = existing
        ? { ...existing, messages: update(existing.messages) }
        : { id: sessionId, messages: update([]), linkedSession: null };
      return [updated, ...current.filter((session) => session.id !== sessionId)];
    });
  }, []);

  const selectAssistantSession = useCallback((sessionId: string) => {
    if (sessionId === activeSessionId) {
      setHistoryOpen(false);
      return;
    }
    if (busy) requestAssistantCancellation(activeSessionId);
    requestIdRef.current += 1;
    setActiveSessionId(sessionId);
    setHistoryOpen(false);
    setBusy(false);
    setStreamingAnswer("");
    setStreamingProgress("");
    setToolActivities([]);
    setDraft("");
    setError("");
    setFailedMessage("");
    pendingMessageRef.current = "";
    setComposerFocused(false);
    followMessagesRef.current = true;
    programmaticScrollRef.current = false;
    setContextAtOpen(getContext());
    contextCapturedRef.current = false;
  }, [activeSessionId, busy, getContext]);

  const currentContext = useCallback(() => {
    const current = getContext();
    const opened = contextAtOpen;
    if (!opened) return current;
    return {
      ...current,
      selection: current.selection || opened.selection,
      selectedContent: [...new Set([...current.selectedContent, ...opened.selectedContent])].slice(0, 24),
    };
  }, [contextAtOpen, getContext]);

  const assistantContext = useMemo(() => currentContext(), [currentContext]);
  const promptSuggestions = useMemo(
    () => messages.length === 0 ? assistantPromptSuggestions(assistantContext) : [],
    [assistantContext, messages.length],
  );

  useEffect(() => {
    if (!open) return undefined;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (historyOpen) setHistoryOpen(false);
      else closeAssistant();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [closeAssistant, historyOpen, open]);

  const scrollMessagesToEnd = useCallback((behavior: ScrollBehavior = "auto") => {
    const viewport = messageViewportRef.current;
    if (!viewport) return;
    programmaticScrollRef.current = true;
    if (typeof viewport.scrollTo === "function") {
      viewport.scrollTo({ top: viewport.scrollHeight, behavior });
    } else {
      viewport.scrollTop = viewport.scrollHeight;
    }
    if (scrollTimerRef.current) window.clearTimeout(scrollTimerRef.current);
    scrollTimerRef.current = window.setTimeout(() => {
      programmaticScrollRef.current = false;
    }, behavior === "smooth" ? 320 : 0);
  }, []);

  const clearNotificationIfReplyVisible = useCallback(() => {
    if (typeof document === "undefined") return;
    if (!canMarkAssistantReplyRead(
      open,
      followMessagesRef.current,
      document.visibilityState === "visible",
      document.hasFocus(),
    )) return;
    setNotification(false);
  }, [open]);

  const handleMessageScroll = () => {
    const viewport = messageViewportRef.current;
    if (!viewport || programmaticScrollRef.current) return;
    const distanceFromEnd = viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight;
    followMessagesRef.current = distanceFromEnd <= 56;
    if (followMessagesRef.current) clearNotificationIfReplyVisible();
  };

  const leaveLiveEdge = () => {
    programmaticScrollRef.current = false;
  };

  useLayoutEffect(() => {
    if (!open || !followMessagesRef.current) return undefined;
    const frame = window.requestAnimationFrame(() => {
      scrollMessagesToEnd(window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth");
    });
    return () => window.cancelAnimationFrame(frame);
  }, [activeSessionId, busy, messages.length, open, scrollMessagesToEnd, toolActivities.length]);

  useLayoutEffect(() => {
    if (messages.at(-1)?.role !== "assistant") return;
    clearNotificationIfReplyVisible();
  }, [activeSessionId, clearNotificationIfReplyVisible, messages.length]);

  useEffect(() => {
    const clearWhenWindowVisible = () => clearNotificationIfReplyVisible();
    window.addEventListener("focus", clearWhenWindowVisible);
    document.addEventListener("visibilitychange", clearWhenWindowVisible);
    return () => {
      window.removeEventListener("focus", clearWhenWindowVisible);
      document.removeEventListener("visibilitychange", clearWhenWindowVisible);
    };
  }, [clearNotificationIfReplyVisible]);

  useEffect(() => {
    if (!open || !messageContentRef.current || typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(() => {
      if (followMessagesRef.current) {
        scrollMessagesToEnd(window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth");
      }
    });
    observer.observe(messageContentRef.current);
    return () => observer.disconnect();
  }, [activeSessionId, messages.length, open, scrollMessagesToEnd]);

  useEffect(
    () => () => {
      if (scrollTimerRef.current) window.clearTimeout(scrollTimerRef.current);
    },
    [],
  );

  const sendMessage = useCallback(async (value: string, appendUser: boolean) => {
    const message = value.trim();
    if (!message || busy) return;
    if (!activeAgentAvailable) {
      setDraft(message);
      setFailedMessage(message);
      setError("No available assistant agent. Choose one in Settings.");
      return;
    }
    const requestId = requestIdRef.current + 1;
    requestIdRef.current = requestId;
    setBusy(true);
    setStreamingAnswer("");
    setStreamingProgress("Starting");
    setToolActivities([]);
    setError("");
    setFailedMessage("");
    setNotification(false);
    pendingMessageRef.current = message;
    const sessionId = activeSessionId ?? newAssistantChatSessionId();
    const context = currentContext();
    const history = appendUser ? messages : messages.filter((item) => item.content !== message || item.role !== "user");
    if (appendUser) {
      setActiveSessionId(sessionId);
      updateSessionMessages(sessionId, (current) => [...current, { role: "user", content: message }]);
    }
    setDraft("");
    try {
      const response = await askAssistantStream(
        assistantAskRequest(message, history, context, activeAgent, workspace, sessionId, appendUser),
        (event) => {
          if (requestId !== requestIdRef.current) return;
          if (event.kind === "delta" && event.text) {
            setStreamingAnswer((current) => current + event.text);
          } else if (event.kind === "replace") {
            setStreamingAnswer(event.text ?? "");
          } else if (event.kind === "progress") {
            setStreamingProgress(event.detail || "Working");
          } else if (event.kind === "tool-call" || event.kind === "tool-result") {
            setToolActivities((current) => updateAssistantToolActivities(current, event, requestId));
            setStreamingProgress(event.kind === "tool-call"
              ? `Using ${event.detail || "tool"}`
              : `Finished ${event.detail || "tool"}`);
          } else if (event.kind === "error") {
            setToolActivities((current) => current.map((activity) => (
              activity.status === "running" ? { ...activity, status: "error" } : activity
            )));
          }
        },
      );
      if (requestId !== requestIdRef.current) return;
      if (response.status === "completed" && response.answer.trim()) {
        updateSessionMessages(sessionId, (current) => [...current, { role: "assistant", content: response.answer.trim() }]);
        setNotification(true);
        setStreamingAnswer("");
        setStreamingProgress("");
        pendingMessageRef.current = "";
      } else {
        const detail = response.error?.trim() || "Assistant returned no answer.";
        setToolActivities((current) => current.map((activity) => (
          activity.status === "running" ? { ...activity, status: "error" } : activity
        )));
        setStreamingAnswer("");
        setStreamingProgress("");
        setDraft(message);
        setFailedMessage(message);
        setError(detail);
        pendingMessageRef.current = "";
      }
    } catch (requestError) {
      if (requestId !== requestIdRef.current) return;
      setToolActivities((current) => current.map((activity) => (
        activity.status === "running" ? { ...activity, status: "error" } : activity
      )));
      setStreamingAnswer("");
      setStreamingProgress("");
      setDraft(message);
      setFailedMessage(message);
      setError(requestError instanceof Error ? requestError.message : `${requestError}`);
      pendingMessageRef.current = "";
    } finally {
      if (requestId === requestIdRef.current) setBusy(false);
    }
  }, [activeAgent, activeAgentAvailable, activeSessionId, busy, currentContext, messages, updateSessionMessages, workspace]);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const message = draft.trim();
    void sendMessage(draft, failedMessage !== message);
  };

  const stopGeneration = () => {
    const sessionId = activeSessionId;
    const message = pendingMessageRef.current;
    requestIdRef.current += 1;
    setBusy(false);
    setStreamingAnswer("");
    setStreamingProgress("");
    setToolActivities((current) => current.map((activity) => (
      activity.status === "running" ? { ...activity, status: "cancelled" } : activity
    )));
    if (message) {
      setDraft(message);
      setFailedMessage(message);
    }
    setError("Generation stopped.");
    pendingMessageRef.current = "";
    requestAssistantCancellation(sessionId);
  };

  const startNewSession = useCallback(() => {
    if (busy) requestAssistantCancellation(activeSessionId);
    requestIdRef.current += 1;
    setBusy(false);
    setStreamingAnswer("");
    setStreamingProgress("");
    setToolActivities([]);
    setDraft("");
    setError("");
    setFailedMessage("");
    pendingMessageRef.current = "";
    setNotification(false);
    setComposerFocused(false);
    setHistoryOpen(false);
    followMessagesRef.current = true;
    programmaticScrollRef.current = false;
    setContextAtOpen(getContext());
    contextCapturedRef.current = false;
    setActiveSessionId(null);
    window.requestAnimationFrame(() => composerTextareaRef.current?.focus());
  }, [activeSessionId, busy, getContext]);

  const runSuggestedPrompt = (suggestion: AssistantPromptSuggestion) => {
    setError("");
    setFailedMessage("");
    setDraft(suggestion.message);
    window.requestAnimationFrame(() => composerTextareaRef.current?.focus());
  };

  const resizeComposer = useCallback(() => {
    const textarea = composerTextareaRef.current;
    const measurement = composerMeasurementRef.current;
    if (!textarea || !measurement || textarea.value !== draft) return;
    const nextHeight = Math.min(Math.max(measurement.scrollHeight, 48), 192);
    const height = `${nextHeight}px`;
    if (textarea.style.height !== height) textarea.style.height = height;
  }, [draft]);

  useLayoutEffect(() => {
    resizeComposer();
  }, [resizeComposer]);

  useEffect(() => {
    const textarea = composerTextareaRef.current;
    if (!textarea || typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(resizeComposer);
    observer.observe(textarea);
    return () => observer.disconnect();
  }, [resizeComposer]);

  return (
    <div
      ref={assistantHostRef}
      className={`assistantOrbHost${open ? " isOpen" : ""}`}
    >
      <AnimatePresence initial={false} mode="wait">
        {open ? (
          <motion.section
            key="assistant-panel"
            className="assistantChatPanel"
            aria-label="Tendi assistant chat"
            initial={reduceMotion ? { opacity: 0 } : { opacity: 0, scale: 0.96, borderRadius: PANEL_INITIAL_RADIUS }}
            animate={reduceMotion ? { opacity: 1 } : { opacity: 1, scale: 1, borderRadius: PANEL_RADIUS }}
            exit={reduceMotion
              ? { opacity: 0, transition: { duration: 0.12, ease: "easeOut" } }
              : { opacity: 0, scale: 0.96, borderRadius: PANEL_INITIAL_RADIUS, transition: PANEL_EXIT_TRANSITION }}
            transition={reduceMotion ? { duration: 0.12, ease: "easeOut" } : PANEL_MORPH_TRANSITION}
          >
          <header className="assistantChatHeader">
            <div className="assistantChatHeaderStart">
              <IconButton
                className="assistantChatHistoryToggle"
                aria-label="Show assistant conversation history"
                aria-controls="assistant-chat-history"
                aria-expanded={historyOpen}
                aria-pressed={historyOpen}
                onClick={() => setHistoryOpen((current) => !current)}
              >
                <Menu size={16} aria-hidden="true" />
              </IconButton>
              <IconButton
                className="assistantChatNewSession"
                aria-label="Start a new assistant session"
                onClick={startNewSession}
              >
                <SquarePen size={15} aria-hidden="true" />
              </IconButton>
            </div>
            <div className="assistantChatHeaderEnd">
              <IconButton
                className="assistantChatPin"
                aria-label={pinned ? "Unpin assistant" : "Pin assistant open"}
                aria-pressed={pinned}
                onClick={() => setPinned((current) => !current)}
              >
                <Pin size={15} fill={pinned ? "currentColor" : "none"} aria-hidden="true" />
              </IconButton>
              <button type="button" className="assistantChatClose" aria-label="Close assistant" onClick={closeAssistant}>
                <X size={15} aria-hidden="true" />
              </button>
            </div>
          </header>
          <section
            ref={messageViewportRef}
            className="assistantChatMessages"
            aria-label="Assistant conversation"
            onScroll={handleMessageScroll}
            onWheel={leaveLiveEdge}
            onTouchStart={leaveLiveEdge}
            onKeyDown={(event) => {
              if (["ArrowUp", "PageUp", "Home"].includes(event.key)) leaveLiveEdge();
            }}
          >
            <div
              ref={messageContentRef}
              role="log"
              aria-live="polite"
              aria-relevant="additions text"
              aria-busy={busy}
              data-selectable-text
              className="assistantMessageScrollerContent"
            >
              <div data-slot="message-group" className="assistantMessageGroup">
                {messages.length === 0 ? (
                  <div className="assistantChatEmpty">
                    <SharedLayoutBg
                      className="assistantPromptList"
                      pillClassName="assistantPromptPill"
                      inset={0}
                      aria-label="Suggested questions"
                    >
                      {promptSuggestions.map((suggestion) => (
                        <Button
                          key={suggestion.label}
                          className="assistantPromptButton"
                          variant="outline"
                          disabled={busy || !activeAgentAvailable}
                          aria-label={`Use suggestion: ${suggestion.message}`}
                          onClick={() => runSuggestedPrompt(suggestion)}
                        >
                          <span className="assistantPromptCopy">
                            <strong>{suggestion.label}</strong>
                            <span>{suggestion.detail}</span>
                          </span>
                          <ArrowUpRight size={15} aria-hidden="true" />
                        </Button>
                      ))}
                    </SharedLayoutBg>
                  </div>
                ) : null}
                {messages.map((message, index) => (
                  <Fragment key={`${activeSessionId}-${message.role}-${index}`}>
                    {!busy && toolActivities.length > 0 && message.role === "assistant" && index === messages.length - 1 ? (
                      <AgentMessage id="assistant-tool-activity" from="assistant">
                        <AgentMessageContent>
                          <AgentToolActivityList activities={toolActivities} />
                        </AgentMessageContent>
                      </AgentMessage>
                    ) : null}
                    <AgentMessage
                      id={`assistant-message-${activeSessionId}-${index}`}
                      from={message.role}
                      animateIn={message.role === "user"}
                    >
                      <AgentMessageContent>
                        <AgentMessageBubble variant={message.role}>
                          <AgentMessageBubbleContent>
                            {message.role === "assistant" ? (
                              <AgentStreamingResponse status="complete">
                                <AgentMessageMarkdown content={message.content} />
                              </AgentStreamingResponse>
                            ) : (
                              <AgentMessageMarkdown content={message.content} />
                            )}
                          </AgentMessageBubbleContent>
                        </AgentMessageBubble>
                      </AgentMessageContent>
                    </AgentMessage>
                  </Fragment>
                ))}
                {!busy && toolActivities.length > 0 && messages.at(-1)?.role !== "assistant" ? (
                  <AgentMessage id="assistant-tool-activity" from="assistant">
                    <AgentMessageContent>
                      <AgentToolActivityList activities={toolActivities} />
                    </AgentMessageContent>
                  </AgentMessage>
                ) : null}
                {busy ? (
                  <AgentMessage id="assistant-message-pending" from="assistant">
                    <AgentMessageContent>
                      {toolActivities.length > 0 ? <AgentToolActivityList activities={toolActivities} /> : null}
                      {streamingAnswer ? (
                        <AgentMessageBubble variant="assistant">
                          <AgentMessageBubbleContent>
                            <AgentStreamingResponse status="streaming">
                              <AgentMessageMarkdown content={streamingAnswer} />
                            </AgentStreamingResponse>
                          </AgentMessageBubbleContent>
                        </AgentMessageBubble>
                      ) : (
                        <AgentMessageTyping label={streamingProgress || "Responding"} />
                      )}
                    </AgentMessageContent>
                  </AgentMessage>
                ) : null}
              </div>
            </div>
          </section>
          <AnimatePresence initial={false}>
            {historyOpen ? (
              <>
                <motion.button
                  type="button"
                  className="assistantChatHistoryBackdrop"
                  aria-label="Close assistant conversation history"
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  exit={{ opacity: 0 }}
                  transition={reduceMotion ? { duration: 0.1 } : { duration: 0.18 }}
                  onClick={() => setHistoryOpen(false)}
                />
                <motion.aside
                  className="assistantChatHistoryDrawer"
                  aria-label="Assistant conversation history"
                  initial={reduceMotion ? { opacity: 0 } : { x: "-100%" }}
                  animate={reduceMotion ? { opacity: 1 } : { x: 0 }}
                  exit={reduceMotion ? { opacity: 0 } : { x: "-100%" }}
                  transition={reduceMotion ? { duration: 0.1 } : { type: "spring", stiffness: 420, damping: 38 }}
                >
                  <AssistantChatHistory
                    sessions={sessions}
                    activeSessionId={activeSessionId}
                    onSelect={selectAssistantSession}
                  />
                </motion.aside>
              </>
            ) : null}
          </AnimatePresence>
          <div className="assistantComposerDock">
            <form className="assistantChatForm" onSubmit={submit}>
              <button
                type="button"
                className="assistantComposerPendant"
                data-mood={composerMood}
                data-state={busy ? "loading" : notification ? "notification" : "idle"}
                aria-label="Change assistant shape"
                disabled={busy}
                onClick={changeComposerBloubShape}
              >
                {busy ? (
                  <>
                    <AssistantOrbLoadingShape shapeIndex={bloubShapeIndex} />
                    <span className="assistantComposerFace">
                      <AssistantOrbFace mood="attentif" />
                    </span>
                  </>
                ) : (
                  <>
                    <AssistantOrbPendantShape fromShapeIndex={petFromShapeIndex} shapeIndex={bloubShapeIndex} />
                    <span className="assistantComposerFace">
                      <AssistantOrbFace mood={notification ? "surpris" : composerMood} />
                    </span>
                  </>
                )}
                {notification ? <span className="assistantComposerNotificationDot" /> : null}
              </button>
              <div ref={composerMeasurementRef} aria-hidden="true" className="assistantComposerMeasurement">
                {`${draft}\u200b`}
              </div>
              <textarea
                ref={composerTextareaRef}
                aria-label="Message Tendi assistant"
                placeholder={activeAgentAvailable ? "Ask anything…" : "Choose an assistant agent below"}
                value={draft}
                disabled={busy || !activeAgentAvailable}
                rows={2}
                onFocus={() => setComposerFocused(true)}
                onBlur={() => setComposerFocused(false)}
                onChange={(event) => {
                  setDraft(event.target.value);
                  setFailedMessage("");
                }}
                onCompositionStart={() => {
                  composingRef.current = true;
                }}
                onCompositionEnd={() => {
                  composingRef.current = false;
                }}
                onKeyDown={(event) => {
                  if (event.key !== "Enter") return;
                  if (composingRef.current || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
                  if (event.shiftKey) return;
                  event.preventDefault();
                  event.currentTarget.form?.requestSubmit();
                }}
              />
              <div className="assistantComposerToolbar">
                {composerAgentOptions.length > 0 ? (
                  <SelectControl
                    className="assistantAgentSelect"
                    contentClassName="assistantAgentSelectContent"
                    label="Assistant agent"
                    value={activeAgent}
                    onValueChange={setSelectedAgent}
                    options={composerAgentOptions}
                    side="top"
                    align="start"
                    renderValue={(option) => (
                      <AgentOptionLabel
                        agent={option?.value ?? activeAgent}
                        label={option?.label ?? activeAgentLabel}
                        variant="filter"
                      />
                    )}
                    renderOption={(option) => <AgentOptionLabel agent={option.value} label={option.label} variant="filter" />}
                  />
                ) : (
                  <span className="assistantComposerNoAgent">No agent available</span>
                )}
                <div className="assistantComposerActions">
                  <button type={busy ? "button" : "submit"} className="assistantSendButton" aria-label={busy ? "Stop generating" : "Send message"} disabled={busy ? false : !draft.trim() || !activeAgentAvailable} aria-busy={busy} onClick={busy ? stopGeneration : undefined}>
                    {busy ? <Square size={12} fill="currentColor" aria-hidden="true" /> : <ArrowUp size={16} aria-hidden="true" />}
                  </button>
                </div>
              </div>
            </form>
          </div>
          </motion.section>
        ) : (
          <motion.div
            key="assistant-trigger"
            initial={reduceMotion ? { opacity: 0 } : { opacity: 0, scale: 0.92 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={reduceMotion ? { opacity: 0 } : { opacity: 0, scale: 0.92 }}
            transition={reduceMotion ? { duration: 0.12, ease: "easeOut" } : PANEL_EXIT_TRANSITION}
          >
            <AssistantOrbTrigger
              onCaptureContext={captureContextBeforeFocus}
              onOpen={openAssistant}
              notification={notification}
              shapeIndex={bloubShapeIndex}
            />
          </motion.div>
        )}
      </AnimatePresence>
      {error && typeof document !== "undefined" ? createPortal(<Toast tone="error" message={error} onDismiss={() => setError("")} />, document.body) : null}
    </div>
  );
}
