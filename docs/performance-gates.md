# Performance gates

Run the fast local gate:

```sh
node scripts/perf-check.mjs --fast
```

Run only isolated CRUD fixtures while a desktop instance is using the local database:

```sh
node scripts/perf-check.mjs --fast --no-build --only tertiary-hook-toggle,tertiary-mcp-toggle,tertiary-rule-delete,tertiary-prompt-crud
```

Run the full local gate:

```sh
node scripts/perf-check.mjs --full
```

Both profiles run deterministic first-, second-, and third-level chain gates through production
core APIs. The runner creates isolated temporary files and SQLite databases, then deletes them.
The only user-data secondary check is Config read: it reads and serializes one existing config but
does not print or modify its content.

The Git pre-push hook runs the deterministic `--fast` profile, including the indexed Session
batch-search gate. GitHub Actions does not repeat this gate. The full profile, local-data checks,
and real-data WebView scenario remain local because they depend on the developer's Session
database, browser, or running desktop app.

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

## Git pre-commit gate

The repository-managed `.githooks/pre-commit` runs the layer-boundary check and the unused-code
check. The latter treats Rust `dead_code` warnings and unused desktop TypeScript locals or
parameters as errors. Run the unused-code check directly when iterating:

```sh
node scripts/check-unused-code.mjs
```

The check covers the whole workspace, including test targets, so it catches dead code that is only
referenced from tests.

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
| MCP list | Local data | median <= 45 ms |
| Overview | 512 analyzed Sessions, 365 days | operation <= 35 ms, RSS <= 28 MiB, payload <= 0.625 MiB |
| Prompts | 500 Prompts | operation <= 50 ms, RSS <= 24 MiB, payload <= 1.5 MiB |
| Config | Existing config catalog | operation <= 20 ms, RSS <= 16 MiB, payload <= 0.0625 MiB |
| Settings | Isolated Store | operation <= 2 ms, RSS <= 16 MiB, payload <= 0.015625 MiB |
| Sessions list, full profile | Local data | 3-run median <= 3 s, max <= 8 s, RSS <= 56 MiB, output <= 8 MiB |

### Second-level chains

| Check | Fixture | Default gate |
| --- | --- | --- |
| Session first transcript page | 400 x 4 KiB messages; returns 160 | operation <= 35 ms, RSS <= 24 MiB, payload <= 1 MiB |
| Session batch search | 512 indexed Sessions; searches 100 candidates | operation <= 100 ms, RSS <= 24 MiB, payload <= 0.25 MiB |
| Skill Linked Sessions | 600 links | operation <= 25 ms, RSS <= 24 MiB, payload <= 0.75 MiB |
| Skill file tree + file read | 300 x 4 KiB files | operation <= 10 ms, RSS <= 16 MiB, payload <= 0.125 MiB |
| Rule detail | 128 KiB file | operation <= 15 ms, RSS <= 16 MiB, payload <= 0.25 MiB |
| Hook detail | 192 KiB source, exercises 128 KiB truncation | operation <= 20 ms, RSS <= 16 MiB, payload <= 0.25 MiB |
| Config read | One existing config when available | operation <= 20 ms, RSS <= 16 MiB, payload <= 0.25 MiB |

### Third-level chains

| Check | Fixture | Default gate |
| --- | --- | --- |
| Skill save + create + rename + delete + targeted refresh | 240-Skill authority snapshot | operation <= 45 ms, RSS <= 24 MiB, payload <= 0.0625 MiB |
| Hook batch delete + rescan | Delete 100 of 500 Hooks | operation <= 55 ms, RSS <= 24 MiB, payload <= 0.25 MiB |
| Hook batch toggle + rescan | Disable 100 of 500 Hooks | operation <= 55 ms, RSS <= 24 MiB, payload <= 0.25 MiB |
| MCP toggle | Disable one Cursor MCP server | operation <= 40 ms, RSS <= 16 MiB, payload <= 0.015625 MiB |
| Prompt create + update + 100-row delete + list | 500 Prompts | operation <= 40 ms, RSS <= 24 MiB, payload <= 1 MiB |
| Session project merge + split | Merge 10 projects; split 100 of 500 Sessions | operation <= 10 ms, RSS <= 24 MiB, payload <= 0.0625 MiB |
| Rule save | 128 KiB file | operation <= 40 ms, RSS <= 16 MiB, payload <= 0.25 MiB |
| Rule delete | 128 KiB file | operation <= 40 ms, RSS <= 16 MiB, payload <= 0.015625 MiB |
| Settings save | 32 additional Session roots | operation <= 3 ms, RSS <= 16 MiB, payload <= 0.015625 MiB |

### Large-input and runtime gates

| Check | Default gate |
| --- | --- |
| Synthetic 96 MiB transcript | <= 600 ms, RSS <= 16 MiB |
| Largest indexed transcript | <= 4.5 s, RSS <= max(80 MiB, input x 0.30) |
| Desktop idle CPU | average <= 1%, max <= 5% |

The repeated Sessions check keeps the gate strict without making one filesystem scheduling spike
the only result. The maximum limit still fails a single long stall. CI uses macOS-runner-specific
headroom for operation timing in the session-page, linked-session, rule-detail, prompt-CRUD, and
chart gates. RSS, payload, and local default limits remain unchanged.

## Startup and Usage checkpoints

The Overview and chart gates above time an isolated core query and chart computation. They do not
measure how long the packaged app takes to display Usage, or how soon Usage updates during a fresh
analytics backfill. Keep these local checkpoints separate from the deterministic gates:

| Checkpoint | 2026-09-29 local observation | Remaining work |
| --- | --- | --- |
| Packaged app launch to first Usage query completion, with an indexed database | 4.41 s by log timestamps: 3.20 s to frontend start, 0.34 s to the revision response, then 0.87 s to query completion | Compare the new first data frame marker on repeated packaged launches and reduce the startup and query delay; the batch notification change does not shorten this path. |
| Fresh analytics backfill to a usable Usage chart | The initial backfill took about 4 minutes before its final revision. The daemon now publishes the first committed batch and further revisions at most every 5 seconds. | Measure time to `first usage data frame completed` and total backfill time on an isolated, repeatable Session fixture. Do not use final revision time as a proxy for first visible data. |
| Skills list at startup | 33.2 s before watcher batching; 2.92 s after batching on the local packaged app | Add an app startup gate if this latency needs an enforced limit; the 300 ms Skills CLI gate excludes watcher registration. |
| Warm startup Session scan and Skills reference index | Before the cache-path fix, a local workspace with 6,293 sessions and 33.86 GiB of transcripts spent about 88 s in Session scan and 58 s in Skills reference indexing. Two warm launches after the fixes, with 6,299 sessions, took 5.36–5.49 s for the full scan; Cursor took 0.301–0.331 s. The Skills reference index parsed 5–6 changed sessions in 0.128–0.188 s (0.35–2.30 s including loading). | Add a repeatable app startup gate with a fixed Session fixture; require unchanged transcripts and Cursor stores to remain on the cache path and growing Codex transcripts to use append indexing. |
| Live dev app startup, process 40148 | Desktop setup took 223 ms. First Prompts, Agents, Hooks, Rules, MCP, and Skills loads completed 389–429 ms after frontend start. Session watcher registration took 23 ms; its worker scanned 6,305 sessions, including 17 changed sessions, in 7.14 s. First Usage query completed 0.83 s after frontend start. | Repeat in a packaged foreground launch with a fixed fixture; the dev app and hidden WebView are not comparable to a packaged first-paint gate. |

In the isolated 500-Hook fixture, disabling 100 Hooks in one source took 117 ms before provider
batch mutation. Parsing and patching the source once per batch reduced four local runs to
41.5–49.2 ms. The gate above enforces 55 ms for this fixture.

For a warm-start comparison, restart the packaged app and correlate `desktop process starting`,
`desktop setup completed`, `frontend started`, `catalog domain load completed`,
`projection refresh completed`, `session scan completed`, `analytics_revision`,
`overview analytics query completed`, and `first usage data frame completed` in
`~/Library/Application Support/tendi/logs/tendi.log` by process ID. The frame marker is emitted
after two animation frames following a chart commit; it is a display milestone, not a pixel-level
paint measurement. `catalog domain load completed` includes subscription readiness and the frontend
request and records window visibility at start and end. During one hidden dev-window run, the
Sessions backend scan finished in 7.14 s but the frontend completion marker arrived 187.8 s
after request start, when the window became visible. Do not attribute hidden-WebView delay to disk
scanning. `projection refresh completed` separates scheduler queue time from scanning and saving.
Slow catalog reads also emit `cached projection read completed` with database open, cached read,
status, and refresh scheduling times. Subtract these from the corresponding JSON-RPC method time
to locate work such as watcher registration or response encoding. `session watcher configured`
splits the synchronous `sessions_scan_start` setup into settings, watch-plan, and watcher-registration
times; the scan itself completes later on a worker. Compare these times before attributing fan
activity to a provider scan.

The real-data WebView scenario supplies analytics synthesized from
Session metadata and cannot measure the daemon's indexing or Overview query latency. A fresh-index
gate needs an isolated database and a stable transcript fixture; it must not reset the user's data.

`operation` measures only the production API call and response serialization. Fixture setup is
outside that timer. `process` is still recorded for diagnosis, and RSS covers the whole process,
including fixture setup.

## Coverage boundaries

| Tab | Second level | Third level |
| --- | --- | --- |
| Overview | N/A: chart changes the same aggregate query | N/A |
| Skills | File tree/read; Linked Sessions | File CRUD and targeted refresh |
| Prompts | N/A: row body is already in the list payload | Create, update, and batch delete |
| Sessions | Transcript page and indexed batch search | Project merge and split |
| Rules | Rule read | Rule save and delete |
| Hooks | Source preview | Batch delete and toggle, each with rescan |
| MCP | N/A: no row detail | Cursor toggle; no probe gate because it depends on external servers |
| Config | Config read | Not automated: the public API only permits real user config paths |
| Settings | N/A: no row detail | Settings save |

The core gates do not measure WebView DOM/layout/paint.
All daemon JSON-RPC writes log `runtime operation completed` with their method and duration, and
frontend writes log `tendi command completed` with full request latency. Reads log when they take
at least 100 ms or fail. The CRUD logs cover operations without an isolated gate, while the fixture
gates above enforce the repeatable local paths. Repeated identical reads share an in-flight request;
writes execute individually.

Session pagination bounds each backend page, but repeatedly loading pages can still grow the
transcript DOM. The desktop Git update checker reuses a successful result for 60 seconds and
invalidates it when the skill scan changes. Git commands are bounded by local and network timeouts
and can be cancelled. Browser automation is not used by this gate.

## Threshold overrides

Thresholds have conservative defaults and can be overridden for diagnostics:

```sh
TENDI_PERF_SKILLS_MS=500 node scripts/perf-check.mjs --fast
TENDI_PERF_SESSIONS_MS=4000 TENDI_PERF_SESSIONS_MAX_MS=10000 node scripts/perf-check.mjs --full
```

Available variables are listed in `scripts/perf-check.mjs` under `thresholds`.
