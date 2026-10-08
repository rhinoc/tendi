---
name: tendi
description: Use the local Tendi CLI to identify the current coding-agent session, read its transcript, search earlier sessions, or manage installed agent skills and configuration. Trigger when the user mentions Tendi, asks for the current session ID or transcript, wants to find local agent history, or wants to manage installed skills through Tendi.
---

# Tendi CLI

Use Tendi as the source of truth for local coding-agent sessions and installed agent assets.
Prefer JSON output for agent-driven reads. Never edit Tendi's SQLite database directly.

## Route the request

- Identify this session or read its conversation so far: follow **Current session**.
- Find earlier work or answer a question about past work: follow **Recall a session**.
- Inspect installed skills: follow **Inspect skills**.
- Install a third-party skill: follow **Install skills**.
- Change visibility, wrap, update, or link skills: follow **Change skills**.
- Inspect agents, rules, hooks, MCP servers, or the whole local inventory: follow
  **Other inventory**.

## Current session

Run these commands from the agent's shell tool:

```text
tendi sessions current --json
tendi sessions transcript --current --json
```

`current` returns `id`, `agent`, and `path`. It reads `CODEX_THREAD_ID` for Codex,
`CLAUDE_CODE_SESSION_ID` for Claude Code, or `CURSOR_CONVERSATION_ID` for Cursor.
The provider locates the matching transcript directly, without waiting for Tendi's index.
`path` is null when no local transcript has been written; reading it then reports an error.
Only conversation content already written to disk is available.

If more than one provider identity is present, specify the known calling provider with
`--agent codex`, `--agent claude`, or `--agent cursor`. If the runtime provides no identity,
report the error; do not choose a recent session or infer identity from the working directory.
Cursor support requires its runtime to export `CURSOR_CONVERSATION_ID` and provide a local
JSONL transcript. Hook-only session variables are not guaranteed to reach the shell tool.

## Recall a session

1. Search the persistent index with project and role filters:

   ```text
   tendi sessions search "<query>" --cwd <project-path> --role user --json
   ```

   Search does not scan sources automatically. With no published index, run
   `tendi sessions refresh`; use the same command when current coverage is needed.
   `--refresh` explicitly refreshes before a search. Refresh reports stages and elapsed
   time on stderr, keeping JSON stdout clean. Existing indexed sources are shared across
   workspace scopes; changing the shell cwd does not change the search corpus.

2. The JSON envelope contains `hits`, `total`, `limit`, `offset`, and `status`.
   Each compact hit contains `id`, `agent`, `path`, title, recorded project cwd,
   `started_at`, `score`, `snippet`, `role`, and `record_order`. Status reports
   published/pending session counts, scopes, and the last scan as Unix seconds.
   A pending count indicates the index has outstanding refresh work; a zero pending
   count does not establish that unscanned provider files are current.

   Available filters and controls:
   - `--cwd <path>` includes that recorded cwd and descendants. Repeat it for historical
     paths; `--cwd-exact` selects only exact paths. A renamed project is not silently
     inferred from today's cwd.
   - `--since <date>` is inclusive and `--until <date>` exclusive, using session start
     time. Dates are `YYYY-MM-DD` in UTC or RFC3339 with an explicit offset.
   - `--agent codex|claude|cursor` and `--role user|assistant` restrict evidence.
   - `--sort relevance|time-asc|time-desc`, `--limit <1..1000>` (default 20), and
     `--offset <n>` control ordering and pagination.
   - `--full --json` includes full session metadata. `--fields id,path,snippet --json`
     selects compact hit fields while retaining envelope status and totals.

   Queries match literal substrings, ignoring case for ordinary ASCII words. Whitespace
   separates AND terms within one indexed message or metadata record. `--phrase` matches
   the entire query as one literal phrase. This is keyword search, not semantic recall:
   alternate wording requires another query. Repeated occurrences do not inflate score.

3. Read only matching messages and context from selected sessions:

   ```text
   tendi sessions transcript --session <id> --match "<query>" --role user --around 2 --json
   ```

   Provider is inferred from the indexed ID. If identities are ambiguous, specify
   `--agent` or use an explicit path:

   ```text
   tendi sessions transcript <path> --agent <codex|cursor|claude> --match "<query>" --json
   ```

   The default is a bounded excerpt of 20 user/assistant messages. `--match` uses AND substrings;
   repeated `--role` or comma-separated roles restrict selected messages. `--around <0..20>`
   adds neighboring items of any role around matches. Items carry original normalized time,
   kind, body, and `index`; `matched` distinguishes selected messages from context.
   Continue using `--offset <next_offset>` when returned. Offset/index refer to normalized
   transcript items, not raw JSONL lines or the search index's `record_order`.
   Keep `source_version` when citing/rechecking a result; restart if the source changes.
   `--page` returns the first provider page and its `nextCursor`; continue with
   `--cursor <opaque-cursor>` separately from excerpt filters.
   Use `--all` only when the complete transcript is needed.

4. Answer from the retrieved evidence. Cite session ID, message time, and local source.
   Distinguish transcript facts from inference. An earliest indexed hit establishes the
   earliest retrieved discussion, not a deployment date; inspect its context before
   describing when an implementation was introduced.

Use `tendi sessions list --json` for the existing inventory listing. Use the session's
native resume command only when the user asks to continue it.

## Inspect skills

Run `tendi skills list --json`. Use the returned name, description, visibility, agent targets,
paths, provenance, update status, dependencies, and dependents. If the user asks to inspect the
actual instructions, read `SKILL.md` from the selected returned path. Do not assume duplicate
installations have identical content.

## Install skills

Tendi accepts a local directory, Git URL, GitHub shorthand, or supported registry source.
List the source before choosing when its contents are not already known:

```text
tendi skills add <source> --list
```

Preview the exact installation:

```text
tendi skills add <source> --skill <name> --to <shared|codex|cursor|claude> --dry-run
```

Use `shared` by default so compatible agents can discover one copy. Use an agent-specific target
only when the user requests it or the skill is agent-specific. After approval, repeat without
`--dry-run`; use `--yes` only when that approval is already explicit. Do not add `--overwrite`
unless the user approved replacing the reported target. Use `--copy` only when the installed
skill must not track a local source via symlink.

## Change skills

All write commands support a preview. Inspect it before applying:

```text
tendi skills set <pattern> --visibility <auto|manual|off> --dry-run
tendi skills wrap <name> --from <pattern> --dry-run
tendi skills updates --check --json
tendi skills update <pattern> --dry-run
tendi skills link <source> --to <shared|codex|cursor|claude> --dry-run
```

Repeat the approved command without `--dry-run`. Use `--yes` only after the exact operation is
authorized. For unfamiliar flags or newer commands, use `tendi skills --help`; do not guess.

## Other inventory

Use these read-only commands:

```text
tendi scan --json
tendi agents list --json
tendi rules list --json
tendi hooks list --json
tendi mcp list --json
```

`scan` returns the combined inventory and persists the latest snapshot. Prefer the narrower
command when only one domain is needed.

## Maintain the Tendi skill

Check the bundled skill status with:

```text
tendi setup skills --dry-run --json
```

Install it globally for compatible agents with `tendi setup skills --yes`. Use `--to` for an
agent-specific location. If the target contains different content, inspect it first; only then
use `--overwrite` with explicit approval.
