# Current session context

Investigated on 2026-09-30. `tendi sessions current --json` identifies the calling
session from its shell environment. `tendi sessions transcript --current --json`
reads its local JSONL transcript through the owning provider's existing parser.
Neither command requires the daemon or a session-index refresh.

| Provider | Identity | Local transcript resolution |
| --- | --- | --- |
| Codex | `CODEX_THREAD_ID` | Matching rollout under `CODEX_HOME/sessions` or `archived_sessions`; default `~/.codex` |
| Claude Code | `CLAUDE_CODE_SESSION_ID` | Matching JSONL under `CLAUDE_CONFIG_DIR/projects`; default `~/.claude` |
| Cursor | `CURSOR_CONVERSATION_ID` | `CURSOR_TRANSCRIPT_PATH` when supplied, otherwise matching JSONL under `AGENT_TRANSCRIPTS` or `~/.cursor/projects` |

The JSON response contains `id`, `agent`, and `path`. `path` is null if no local
transcript is available. Reading that transcript reports an error. The source
can lag the conversation because only messages written to disk are available.
Multiple provider identities require `--agent`; duplicate transcript matches
report ambiguity. Missing identity never selects the newest session or guesses
from the working directory. Provider selection cannot use another provider's ID.

## Codex

Confirmed in the local Codex source: `codex-rs/protocol/src/shell_environment.rs`
injects `CODEX_THREAD_ID` into tool subprocesses. `core/src/exec_env.rs` identifies
`CODEX_SESSION_ID` as the shared root-session identity. Tendi uses the current
thread ID and does not substitute the root ID. The current Codex session and its
transcript were also read successfully with the local Tendi CLI.

## Claude Code

The [official environment-variable reference](https://code.claude.com/docs/en/env-vars)
documents `CLAUDE_CODE_SESSION_ID` in Bash, PowerShell, hook-command, and stdio MCP
subprocesses. Bash and hook identities update on `/clear`; MCP subprocesses keep
the identity they were spawned with. Invoke Tendi through the shell tool when
identifying the current session. The same reference defines `CLAUDE_CONFIG_DIR`
as the root for configuration and session history.

The [skills reference](https://code.claude.com/docs/en/skills#available-string-substitutions)
also supports `${CLAUDE_SESSION_ID}` text substitution in skill instructions.
That placeholder is distinct from the native subprocess environment variable.
No additional hook installation or placeholder substitution is needed by Tendi.

The [hooks reference](https://code.claude.com/docs/en/hooks#common-input-fields)
provides `session_id` and `transcript_path`, and describes asynchronous transcript
writes. `SessionStart` can persist custom environment variables through
`CLAUDE_ENV_FILE`, but this implementation uses the native ID instead.

## Cursor

The installed Cursor Agent CLI build `2026.09.28-64d2043`, in `index.js`, explicitly
injects `CURSOR_CONVERSATION_ID` from the shell request's conversation identity.
It also sets `AGENT_TRANSCRIPTS` to the project's transcript directory. The
installed desktop extension-host code excludes conversation IDs from reusable
shell snapshots, consistent with keeping them scoped to each shell invocation.
This is evidence from the shipped implementation, not an official compatibility
guarantee for every Cursor version or surface.

The [official hooks reference](https://cursor.com/docs/hooks) documents a stable
`conversation_id`, `session_id`, and an optional `transcript_path`. Hooks can also
receive `CURSOR_TRANSCRIPT_PATH`. A `sessionStart` response can return session
environment variables and additional context, but the documented environment
guarantee covers subsequent hooks; it does not establish propagation into agent
shell tools. Tendi does not install a hook or claim such propagation.

Cursor current-session reads require the runtime's native conversation ID and
a local JSONL transcript. Metadata JSON, SQLite stores, cloud-only history, and
plain-text transcripts are not treated as JSONL transcripts. When the runtime
provides an explicit transcript path, its absence is reported instead of selecting
another file.

## Verification

`sessions::current::tests` covers provider identities, custom roots, nested JSONL
files, archived Codex sessions, missing transcripts, ambiguity, and negative
identity cases. CLI tests cover argument selection and the existing explicit-path
syntax. Claude Code and Cursor context handling is verified with local fixtures;
live sessions in those products are not launched by these checks.
