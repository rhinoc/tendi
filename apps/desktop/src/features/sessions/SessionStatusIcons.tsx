import { Archive, CircleDot, GitFork, type LucideIcon } from "lucide-react";

import { Tooltip } from "../../components/shared/Tooltip.tsx";
import { sessionLogicalIdentityRecordKey, type SessionIdentityRecord } from "../../lib/sessions.ts";
import { useDesktopStore } from "../../store/desktop-store.ts";

function SessionStatusIcon({ label, tone, Icon }: {
  label: string;
  tone: "subagent" | "inUse" | "archived";
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

export function SessionStatusIcons({ session, subagent, archived }: {
  session: SessionIdentityRecord;
  subagent: boolean;
  archived?: boolean;
}) {
  const key = sessionLogicalIdentityRecordKey(session);
  const held = useDesktopStore((state) => Boolean(key && state.sessions.heldSessionKeys.has(key)));
  if (!subagent && !archived && !held) return null;

  return (
    <span className="sessionStatusIcons">
      {subagent ? <SessionStatusIcon label="Subagent" tone="subagent" Icon={GitFork} /> : null}
      {held ? <SessionStatusIcon label="In use" tone="inUse" Icon={CircleDot} /> : null}
      {archived ? <SessionStatusIcon label="Archived" tone="archived" Icon={Archive} /> : null}
    </span>
  );
}
