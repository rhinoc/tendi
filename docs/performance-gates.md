# Performance gates

Run the fast local gate:

```sh
node scripts/perf-check.mjs --fast
```

Run the full local gate:

```sh
node scripts/perf-check.mjs --full
```

The full gate uses local Session data when available. It also creates a deterministic 96 MiB
transcript under `target/perf-fixtures`. Generated results are written to
`target/perf/latest.json`.

To include the desktop idle CPU check, leave Tendi idle and pass its process ID:

```sh
node scripts/perf-check.mjs --full --app-pid "$(pgrep -n -f '/target/debug/tendi-desktop$')"
```

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

## Threshold overrides

Thresholds have conservative defaults and can be overridden for diagnostics:

```sh
TENDI_PERF_SKILLS_MS=2000 node scripts/perf-check.mjs --fast
TENDI_PERF_SESSIONS_MS=15000 node scripts/perf-check.mjs --full
```

Available variables are listed in `scripts/perf-check.mjs` under `thresholds`.
