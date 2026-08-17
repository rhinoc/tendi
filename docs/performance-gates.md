# Performance gates

Run the fast local gate:

```sh
node scripts/perf-check.mjs --fast
```

Run the full local gate:

```sh
node scripts/perf-check.mjs --full
```

Both profiles run deterministic first-, second-, and third-level chain gates through production
core APIs. The runner creates isolated temporary files and SQLite databases, then deletes them.
The only user-data secondary check is Config read: it reads and serializes one existing config but
does not print or modify its content.

The full gate additionally uses local Session data when available. It also creates a deterministic
96 MiB transcript under `target/perf-fixtures`. Generated results are written to
`target/perf/latest.json`.

To include the desktop idle CPU check, leave Tendi idle and pass its process ID:

```sh
node scripts/perf-check.mjs --full --app-pid "$(pgrep -n -f '/target/debug/tendi-desktop$')"
```

Run the real-data WebView scenario from the desktop package:

```sh
pnpm --dir apps/desktop run e2e:real-data
```

This command uses the current `tendi` binary to load the local Skills, Sessions, Rules, Hooks,
and MCP snapshot, selects the largest readable local transcript, and opens the actual Vite page in
Playwright. It measures first view, chart scrolling, Sessions scrolling, transcript search, long
tasks, and long animation frames. It reports counts and byte sizes, not transcript contents. Set
`TENDI_BIN` when the binary is outside `target/debug/tendi`.

Save a local comparison baseline:

```sh
node scripts/perf-check.mjs --full --save-baseline
node scripts/perf-check.mjs --full
```

The second run reports percentage changes against `target/perf/baseline.json`. Static limits are
still authoritative, so deleting `target` does not disable the gate.

## Git pre-push gate

This repository includes `.githooks/pre-push`. Enable repository-managed hooks once per clone:

```sh
git config core.hooksPath .githooks
```

The hook runs only the fast profile. Network checks, full Session scans, and idle CPU checks are
excluded because they are too environment-dependent for every push.

## Default thresholds

### First-level chains

| Check | Fixture | Default gate |
| --- | --- | --- |
| Skills list | Local data | median <= 300 ms |
| Hooks list | Local data | median <= 40 ms |
| Rules list | Local data | median <= 40 ms |
| MCP list | Local data | median <= 40 ms |
| Overview | 512 analyzed Sessions, 365 days | operation <= 25 ms, RSS <= 24 MiB, payload <= 0.25 MiB |
| Prompts | 500 Prompts | operation <= 50 ms, RSS <= 24 MiB, payload <= 1.5 MiB |
| Config | Existing config catalog | operation <= 12 ms, RSS <= 16 MiB, payload <= 0.0625 MiB |
| Settings | Isolated Store | operation <= 2 ms, RSS <= 16 MiB, payload <= 0.015625 MiB |
| Sessions list, full profile | Local data | 3-run median <= 3 s, max <= 8 s, RSS <= 56 MiB, output <= 8 MiB |

### Second-level chains

| Check | Fixture | Default gate |
| --- | --- | --- |
| Session first transcript page | 400 x 4 KiB messages; returns 160 | operation <= 35 ms, RSS <= 24 MiB, payload <= 1 MiB |
| Skill Linked Sessions | 600 links | operation <= 25 ms, RSS <= 24 MiB, payload <= 0.75 MiB |
| Skill file tree + file read | 300 x 4 KiB files | operation <= 10 ms, RSS <= 16 MiB, payload <= 0.125 MiB |
| Rule detail | 128 KiB file | operation <= 15 ms, RSS <= 16 MiB, payload <= 0.25 MiB |
| Hook detail | 192 KiB source, exercises 128 KiB truncation | operation <= 20 ms, RSS <= 16 MiB, payload <= 0.25 MiB |
| Config read | One existing config when available | operation <= 10 ms, RSS <= 16 MiB, payload <= 0.25 MiB |

### Third-level chains

| Check | Fixture | Default gate |
| --- | --- | --- |
| Skill save + create + rename + delete + targeted refresh | 240-Skill authority snapshot | operation <= 45 ms, RSS <= 24 MiB, payload <= 0.0625 MiB |
| Hook batch delete + rescan | Delete 100 of 500 Hooks | operation <= 55 ms, RSS <= 24 MiB, payload <= 0.25 MiB |
| Prompt update + 100-row delete + list | 500 Prompts | operation <= 40 ms, RSS <= 24 MiB, payload <= 1 MiB |
| Session project merge + split | Merge 10 projects; split 100 of 500 Sessions | operation <= 10 ms, RSS <= 24 MiB, payload <= 0.0625 MiB |
| Rule save | 128 KiB file | operation <= 40 ms, RSS <= 16 MiB, payload <= 0.25 MiB |
| Settings save | 32 additional Session roots | operation <= 3 ms, RSS <= 16 MiB, payload <= 0.015625 MiB |

### Large-input and runtime gates

| Check | Default gate |
| --- | --- |
| Synthetic 96 MiB transcript | <= 600 ms, RSS <= 16 MiB |
| Largest indexed transcript | <= 4.5 s, RSS <= max(80 MiB, input x 0.30) |
| Desktop idle CPU | average <= 1%, max <= 5% |

The repeated Sessions check keeps the gate strict without making one filesystem scheduling spike
the only result. The maximum limit still fails a single long stall.

`operation` measures only the production API call and response serialization. Fixture setup is
outside that timer. `process` is still recorded for diagnosis, and RSS covers the whole process,
including fixture setup.

## Coverage boundaries

| Tab | Second level | Third level |
| --- | --- | --- |
| Overview | N/A: chart changes the same aggregate query | N/A |
| Skills | File tree/read; Linked Sessions | File CRUD and targeted refresh |
| Prompts | N/A: row body is already in the list payload | Save and batch delete |
| Sessions | Transcript page | Project merge and split |
| Rules | Rule read | Rule save |
| Hooks | Source preview | Batch delete and rescan |
| MCP | N/A: no row detail | N/A: no mutation UI |
| Config | Config read | Not automated: the public API only permits real user config paths |
| Settings | N/A: no row detail | Settings save |

The core gates do not measure WebView DOM/layout/paint. Session pagination bounds each backend page,
while the desktop Git update checker reuses a successful result for 60 seconds and invalidates it when
the skill scan changes. Git commands are still bounded by local/network timeouts and can be cancelled.
but repeatedly loading pages can still grow the transcript DOM. Browser automation is intentionally
not used by this gate.

## Threshold overrides

Thresholds have conservative defaults and can be overridden for diagnostics:

```sh
TENDI_PERF_SKILLS_MS=500 node scripts/perf-check.mjs --fast
TENDI_PERF_SESSIONS_MS=4000 TENDI_PERF_SESSIONS_MAX_MS=10000 node scripts/perf-check.mjs --full
```

Available variables are listed in `scripts/perf-check.mjs` under `thresholds`.
