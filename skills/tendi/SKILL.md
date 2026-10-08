---
name: tendi
description: Use the local Tendi CLI to identify the current coding-agent session, read its transcript, search earlier sessions, or manage installed agent skills and configuration. Trigger when the user mentions Tendi, asks for the current session ID or transcript, wants to find local agent history, or wants to manage installed skills through Tendi.
---

# Tendi

This file is a discovery stub. The installed `tendi` binary owns the full command guide so
the instructions stay aligned with the version that will execute them.

## Resolve the CLI

If `TENDI_CLI_COMMAND` is set, use its value. Otherwise, use `tendi`.

Run the selected command with `--version`. If it cannot run, report the exact error and stop.
Do not guess commands or edit Tendi's SQLite database or agent directories directly.

## Load the current guide

Run:

```text
TENDI skills guide
```

`TENDI` is a placeholder for the executable resolved above. Substitute it before running
anything; do not create a shell variable or execute the placeholder literally.

Read the returned guide before using Tendi. Prefer `--json` for data that the agent will
interpret. Preview mutating operations and require the user's approval unless their request
already authorizes the exact change.

If the selected binary explicitly reports that `skills guide` is unknown, use only
`TENDI --help`, `TENDI sessions --help`, and `TENDI skills --help`. Do not invent flags.
