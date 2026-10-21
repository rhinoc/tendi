import { Archive, ArchiveRestore } from "lucide-react";

import { IconButton } from "../../components/shared/IconButton.tsx";
import { LoadingIcon } from "../../components/shared/LoadingIcon.tsx";
import { Tooltip } from "../../components/shared/Tooltip.tsx";
import type { SessionRecord } from "../../lib/sessions.ts";

export function SessionArchiveButton({ session, loading, onToggle }: {
  session: SessionRecord;
  loading: boolean;
  onToggle: (session: SessionRecord) => void | Promise<void>;
}) {
  if (!session.archiveSupported || typeof session.archived !== "boolean") return null;
  const label = session.archived ? "Unarchive" : "Archive";
  return (
    <Tooltip content={label}>
      <IconButton disabled={loading} aria-label={label} aria-busy={loading} onClick={() => { void onToggle(session); }}>
        {loading ? <LoadingIcon size={16} /> : session.archived ? <ArchiveRestore size={16} /> : <Archive size={16} />}
      </IconButton>
    </Tooltip>
  );
}
