import { ChevronDown } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { LoadingIcon } from "../../components/shared/LoadingIcon.tsx";
import { Tooltip } from "../../components/shared/Tooltip.tsx";
import { logger } from "../../lib/logger.ts";
import { formatTokenCount } from "../../lib/token-format.ts";
import { formatSessionTitle } from "../../lib/session-preview.ts";
import { sessionCacheRate, sessionLogicalIdentityRecordKey, type SessionRecord } from "../../lib/sessions.ts";
import { useDesktopStore } from "../../store/desktop-store.ts";
import { createTokenizerWorker } from "../../lib/tokenizer-client.ts";
import { TokenizerKind, TokenizerWorkerResponseType } from "../../lib/tokenizer-types.ts";
import {
  buildSessionContext,
  CONTEXT_BAND_LABEL,
  contextShareLabel,
  contextTokenTexts,
  orderSessionFamily,
  sessionUsageParts,
  type ContextBand,
  type ContextFileRow,
  type ContextSourceItem,
  type ContextStep,
  type SessionContextStory,
} from "../../lib/session-context.ts";
import "./SessionContextBar.css";

const BAND_ORDER: ContextBand[] = ["context", "user", "assistant", "reasoning", "tool", "skill"];
const ROW_LIMIT = 6;
const TREND_HEIGHT = 112;
const EMPTY_STORY: SessionContextStory = { bands: [], steps: [], files: [] };

export function SessionContextBar({
  session,
  items,
  loading = false,
  onJump,
}: {
  session: SessionRecord;
  items: ContextSourceItem[];
  loading?: boolean;
  onJump: (key: string) => void;
}) {
  const [counts, setCounts] = useState<Map<string, number> | null>(null);
  const story = useMemo(
    () => counts ? buildSessionContext(items, counts) : EMPTY_STORY,
    [counts, items],
  );
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<number | null>(null);
  const texts = useMemo(() => contextTokenTexts(items), [items]);
  const waitingForTokens = texts.length > 0 && counts === null;
  const usage = session.tokenUsage;
  const visibleTokens = story.bands.reduce((total, band) => total + band.tokens, 0);

  useEffect(() => {
    if (texts.length === 0) {
      setCounts(new Map());
      return;
    }
    setCounts(null);
    let cancelled = false;
    let worker: ReturnType<typeof createTokenizerWorker>;
    try {
      worker = createTokenizerWorker(
        (response) => {
          if (cancelled) return;
          if (response.type === TokenizerWorkerResponseType.Result && response.kind === TokenizerKind.Texts) {
            setCounts(new Map(texts.map((text, index) => [text, response.counts[index] ?? 0])));
            return;
          }
          if (response.type === TokenizerWorkerResponseType.Error) {
            logger.warn("context tokenizer failed", { error: response.message });
          }
        },
        (error) => {
          if (cancelled) return;
          logger.warn("context tokenizer failed", { error });
        },
      );
    } catch (error) {
      logger.warn("context tokenizer failed", { error });
      return;
    }
    worker.request({ kind: TokenizerKind.Texts, texts });
    return () => {
      cancelled = true;
      worker.dispose();
    };
  }, [items, texts]);

  if (!usage && !loading && !waitingForTokens && story.bands.length === 0 && story.steps.length === 0) return null;

  const points = story.steps;
  const requestedIndex = selected ?? points.length - 1;
  const activeIndex = points.length === 0 ? -1 : Math.min(Math.max(requestedIndex, 0), points.length - 1);
  const active = activeIndex >= 0 ? points[activeIndex] : undefined;
  const cacheRate = sessionCacheRate(session);
  const headline = usage
    ? [
        `${formatTokenCount(usage.inputTokens)} in`,
        `${formatTokenCount(usage.outputTokens)} out`,
        cacheRate === undefined ? "" : `Cache ${cacheRate.toFixed(1)}%`,
      ].filter(Boolean)
    : visibleTokens > 0
      ? [`~${formatTokenCount(visibleTokens)} visible`]
      : [];
  const busy = loading || waitingForTokens;

  return (
    <section
      className={`sessionContext${open ? " isOpen" : ""}`}
      aria-label="Context"
      aria-busy={busy || undefined}
      onClick={open ? undefined : () => setOpen(true)}
    >
      <div className="sessionContextSummary">
        <span className="sessionContextHeadline">
          {headline.map((part) => <span key={part}>{part}</span>)}
          {busy ? <LoadingIcon size={12} /> : null}
        </span>
        {story.bands.length > 0 ? (
          <span className="sessionContextStrip" aria-hidden="true">
            {story.bands.map((band) => (
              <span key={band.band} data-band={band.band} style={{ flexGrow: Math.max(band.share, 1) }} />
            ))}
          </span>
        ) : null}
        <button
          type="button"
          className="sessionContextToggle"
          aria-expanded={open}
          aria-label={open ? "Hide context details" : "Show context details"}
          onClick={() => setOpen((current) => !current)}
        >
          <ChevronDown size={16} aria-hidden="true" />
        </button>
      </div>
      {open && story.bands.length > 0 ? (
        <div className="sessionContextLegend">
          {story.bands.map((band) => (
            <span key={band.band}>
              <span className="sessionContextSwatch" data-band={band.band} aria-hidden="true" />
              {CONTEXT_BAND_LABEL[band.band]} {contextShareLabel(band.share, band.tokens)}
            </span>
          ))}
        </div>
      ) : null}
      {open ? (
        <div className="sessionContextBody">
          {points.length > 0 ? (
            <>
              <ContextTrend
                points={points}
                activeIndex={activeIndex}
                onSelect={setSelected}
              />
              {active ? <ContextBrief point={active} onJump={onJump} /> : null}
            </>
          ) : null}
          {story.files.length > 0 ? (
            <ContextFiles files={story.files.slice(0, ROW_LIMIT)} onJump={onJump} />
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

function ContextTrend({
  points,
  activeIndex,
  onSelect,
}: {
  points: ContextStep[];
  activeIndex: number;
  onSelect: (index: number) => void;
}) {
  const peak = Math.max(1, ...points.map((point) => point.total));
  return (
    <div className="sessionContextTrend" role="group" aria-label="Context trend">
      {points.map((point, index) => {
        const height = point.total > 0
          ? Math.max(10, Math.round((point.total / peak) * TREND_HEIGHT))
          : 3;
        const label = `${index + 1}, ${point.compacted ? "compacted" : formatTokenCount(point.total)}`;
        return (
          <button
            key={`${point.key}-${index}`}
            type="button"
            className={index === activeIndex ? "isActive" : ""}
            data-compacted={point.compacted ? "true" : undefined}
            aria-label={label}
            aria-pressed={index === activeIndex}
            onMouseEnter={() => onSelect(index)}
            onFocus={() => onSelect(index)}
            onClick={() => onSelect(index)}
          >
            <span className="sessionContextTrendBar" style={{ height: `${height}px` }}>
              {point.total > 0 ? stackedBands(point) : <span data-band="tool" />}
            </span>
          </button>
        );
      })}
    </div>
  );
}

function stackedBands(point: ContextStep) {
  const total = BAND_ORDER.reduce((sum, band) => sum + point.bands[band], 0);
  if (total <= 0) return <span data-band="tool" />;
  return BAND_ORDER.filter((band) => point.bands[band] > 0).map((band) => (
    <span key={band} data-band={band} style={{ flexGrow: point.bands[band] }} />
  ));
}

function ContextBrief({
  point,
  onJump,
}: {
  point: ContextStep;
  onJump: (key: string) => void;
}) {
  return (
    <div className="sessionContextBrief">
      <BriefRow label="you" text={point.you} jumpKey={point.youKey} onJump={onJump} />
      <BriefRow label="in" text={point.incoming} jumpKey={point.incomingKey} onJump={onJump} />
      <BriefRow label="reply" text={point.reply} jumpKey={point.key} onJump={onJump} />
    </div>
  );
}

function BriefRow({
  label,
  text,
  jumpKey,
  onJump,
}: {
  label: string;
  text: string;
  jumpKey: string;
  onJump: (key: string) => void;
}) {
  const content = (
    <>
      <span>{label}</span>
      <span data-empty={text ? undefined : "true"}>{text || "—"}</span>
    </>
  );
  if (!text || !jumpKey) return <div className="sessionContextBriefRow">{content}</div>;
  return (
    <button type="button" className="sessionContextBriefRow" onClick={() => onJump(jumpKey)}>
      {content}
    </button>
  );
}

function ContextFiles({ files, onJump }: { files: ContextFileRow[]; onJump: (key: string) => void }) {
  return (
    <div className="sessionContextList">
      <div className="sessionContextListTitle">Files</div>
      {files.map((file) => (
        <button key={file.path} type="button" onClick={() => onJump(file.key)}>
          <span>{file.path}</span>
          <span>{fileCounts(file)}</span>
        </button>
      ))}
    </div>
  );
}

function fileCounts(file: ContextFileRow) {
  return [
    file.read > 0 ? `R${file.read}` : "",
    file.write > 0 ? `W${file.write}` : "",
    file.search > 0 ? `S${file.search}` : "",
  ].filter(Boolean).join(" ");
}

export function SessionFamilyList({
  session,
  sessionTree,
  onOpenSession,
  releasingSession,
  onReleaseHeld,
}: {
  session: SessionRecord;
  sessionTree: SessionRecord[];
  onOpenSession: (session: SessionRecord) => void;
  releasingSession?: (session: SessionRecord) => boolean;
  onReleaseHeld?: (session: SessionRecord) => void;
}) {
  const rows = useMemo(() => orderSessionFamily(sessionTree), [sessionTree]);
  const heldKeys = useDesktopStore((state) => state.sessions.heldSessionKeys);
  const together = sessionTree.reduce((total, member) => total + (member.tokenUsage?.totalTokens ?? 0), 0);
  return (
    <div className="sessionFamilyList">
      {rows.map(({ session: member, depth }) => {
        const current = member.id === session.id && member.agent === session.agent;
        const usage = member.tokenUsage;
        const cacheRate = sessionCacheRate(member);
        const heldKey = sessionLogicalIdentityRecordKey(member);
        const inUse = Boolean(heldKey && heldKeys.has(heldKey));
        const title = formatSessionTitle(member.title) || "Untitled";
        const releasing = releasingSession?.(member) === true;
        const meta = (
          <span className="sessionFamilyMeta">
            {usage ? <span>{formatTokenCount(usage.totalTokens)}</span> : null}
            {cacheRate === undefined ? null : <span>cache {cacheRate.toFixed(0)}%</span>}
            {inUse || releasing ? (
              <Tooltip content="Release thread">
                <button
                  type="button"
                  data-status="in-use"
                  aria-label="Release thread"
                  aria-busy={releasing || undefined}
                  disabled={!onReleaseHeld || releasing}
                  onClick={() => onReleaseHeld?.(member)}
                >
                  {releasing ? <LoadingIcon size={12} /> : "in use"}
                </button>
              </Tooltip>
            ) : null}
          </span>
        );
        return (
          <div key={`${member.agent}:${member.id}`} className="sessionFamilyRow" data-current={current ? "true" : undefined} style={{ paddingLeft: 4 + depth * 12 }}>
            {current ? <span className="sessionFamilyTitle">{title}</span> : (
              <button type="button" className="sessionFamilyTitle" onClick={() => onOpenSession(member)}>
                {title}
              </button>
            )}
            {meta}
          </div>
        );
      })}
      {together > 0 ? <div className="sessionFamilyTotal">{formatTokenCount(together)} together</div> : null}
    </div>
  );
}

export function SessionUsageMix({
  usage,
}: {
  usage: NonNullable<SessionRecord["tokenUsage"]>;
}) {
  const parts = sessionUsageParts(usage);
  const total = parts.reduce((sum, part) => sum + part.tokens, 0);
  if (total <= 0) return null;
  const label = parts.map((part) => `${formatTokenCount(part.tokens)} ${part.kind}`).join(" · ");
  return (
    <Tooltip content={label}>
      <span className="sessionUsageMix" aria-hidden="true">
        {parts.map((part) => (
          <span key={part.kind} data-kind={part.kind} style={{ width: `${(part.tokens / total) * 100}%` }} />
        ))}
      </span>
    </Tooltip>
  );
}
