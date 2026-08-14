---
name: tendi
description: Use the local Tendi CLI to search, inspect, and resume coding-agent sessions; list, inspect, install, update, and configure agent skills; or query agent, rule, hook, MCP, and configuration inventory. Trigger when the user mentions Tendi, asks what an earlier Codex, Claude Code, or Cursor session did, wants to find local agent history, or wants to manage installed skills through Tendi.
---

# Tendi CLI

Use Tendi as the source of truth for local coding-agent sessions and installed agent assets.
Prefer JSON output for agent-driven reads. Never edit Tendi's SQLite database directly.

## Route the request

- Find earlier work or answer a question about past work: follow **Recall a session**.
- Inspect installed skills: follow **Inspect skills**.
- Install a third-party skill: follow **Install skills**.
- Change visibility, wrap, update, or link skills: follow **Change skills**.
- Inspect agents, rules, hooks, MCP servers, or the whole local inventory: follow
  **Other inventory**.

## Recall a session

1. Search narrowly first:

   ```text
   tendi sessions search "<query>" --json
   ```

2. Use each hit's `agent`, `path`, title, timestamps, and `search_snippet` to select the
   smallest relevant set. Do not open every transcript.

3. Read the selected transcript only when the search result is insufficient:

   ```text
   tendi sessions transcript <path> --agent <codex|cursor|claude> --json
   ```

4. Answer from the retrieved evidence. Cite each material claim with a compact local source,
   such as `[codex · <session-id>]`. Distinguish transcript facts from your inference.

Use `tendi sessions list --json` when the user wants recent sessions rather than full-text
search. Use the session's native resume command only when the user asks to continue it; confirm
the exact command with `tendi sessions --help` if needed.

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
