import { Archive, CircleDot, GitFork, type LucideIcon } from "lucide-react";

import { LoadingIcon } from "../../components/shared/LoadingIcon.tsx";
import { Tooltip } from "../../components/shared/Tooltip.tsx";
import { sessionLogicalIdentityRecordKey, type SessionIdentityRecord } from "../../lib/sessions.ts";
import { useDesktopStore } from "../../store/desktop-store.ts";

function SessionStatusIcon({ label, tone, Icon }: {
  label: string;
  tone: "subagent" | "archived";
  Icon: LucideIcon;
}) {
  return (
    <Tooltip content={label}>
      <span className={`sessionStatusIcon sessionStatusIcon--${tone}`} role="img" aria-label={label} tabIndex={0}>
        <Icon size={14} aria-hidden="true" />
      </span>
    </Tooltip>
  );
}

export function SessionStatusIcons({ session, subagent, archived, releasing, onRelease }: {
  session: SessionIdentityRecord;
  subagent: boolean;
  archived?: boolean;
  releasing?: boolean;
  onRelease?: () => void;
}) {
  const key = sessionLogicalIdentityRecordKey(session);
  const held = useDesktopStore((state) => Boolean(key && state.sessions.heldSessionKeys.has(key)));
  if (!subagent && !archived && !held && !releasing) return null;

  return (
    <span className="sessionStatusIcons">
      {subagent ? <SessionStatusIcon label="Subagent" tone="subagent" Icon={GitFork} /> : null}
      {held || releasing ? (
        onRelease ? (
          <Tooltip content="Release thread">
            <button
              type="button"
              className="sessionStatusIcon sessionStatusIcon--inUse"
              aria-label="Release thread"
              aria-busy={releasing || undefined}
              disabled={releasing}
              onClick={(event) => {
                event.stopPropagation();
                onRelease();
              }}
            >
              {releasing ? <LoadingIcon size={14} /> : <CircleDot size={14} aria-hidden="true" />}
            </button>
          </Tooltip>
        ) : (
          <Tooltip content="In use">
            <span className="sessionStatusIcon sessionStatusIcon--inUse" role="img" aria-label="In use" tabIndex={0}>
              <CircleDot size={14} aria-hidden="true" />
            </span>
          </Tooltip>
        )
      ) : null}
      {archived ? <SessionStatusIcon label="Archived" tone="archived" Icon={Archive} /> : null}
    </span>
  );
}
