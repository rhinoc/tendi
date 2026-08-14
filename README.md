<div align="center">
  <br />
  <img src="./apps/desktop/src-tauri/icons/tendi-icon.svg" alt="Tendi app icon" width="112" height="112" />
  <h1>tendi</h1>
  <p>A local-first control plane for coding-agent sessions, skills, rules, hooks, MCP servers, and configuration.</p>
  <p>
    <a href="https://github.com/rhinoc/tendi/releases">Releases</a>
    &nbsp;·&nbsp;
    <a href="./LICENSE">License</a>
    &nbsp;·&nbsp;
    <a href="./CONTRIBUTING.md">Contributing</a>
  </p>
  <br />
</div>

> Status: early development. The current desktop workflow targets macOS and is not yet a
> signed or notarized public release.

## Features

- 🔎 **One searchable surface** — Scan local agent configuration and activity into one snapshot-backed CLI and desktop app.
- 🧩 **Skill management** — Add, link, copy, wrap, configure visibility, and check updates for skills from local paths, Git, registries, and HTTP sources.
- 🧰 **Agent configuration discovery** — Inspect Codex, Cursor, and Claude Code executables, versions, configuration roots, rules, hooks, and MCP servers.
- 🗂️ **Session search** — List sessions, search transcripts, and inspect token, model, cache, duration, and completion analytics.
- 🖥️ **Native desktop workflow** — Use React and Tauri views for overview, skills, prompts, sessions, rules, hooks, MCP, and agent profiles.
- 🛡️ **Safe file changes** — Preview plans, protect writes with hashes, and use atomic file replacement for managed changes.

## Requirements

- Rust stable toolchain with Cargo
- Node.js and npm
- Git, for Git-backed skill sources and update checks
- `sqlite3` on `PATH` when running the full acceptance script
- Apple Silicon macOS for the current desktop development workflow

## Install

Tendi does not have a signed public installer yet. To run the CLI from a checkout:

```bash
git clone https://github.com/rhinoc/tendi.git
cd tendi
cargo run -p tendi-cli -- scan
```

To install desktop dependencies and start the Tauri app:

```bash
cd apps/desktop
npm ci
TENDI_CWD=/path/to/project npm run dev:tauri
```

`TENDI_CWD` is optional. Without it, tendi uses the desktop process working directory when
scanning project-level rules, hooks, MCP configuration, and skills.

### Build a local macOS DMG

Build the release `.app` and a local disk image from the repository root:

```bash
cd apps/desktop
npm ci
npm run build:dmg
```

The artifact is written to `dist/tendi-<version>-<arch>.dmg`. The DMG contains `tendi.app`, an
**Applications** shortcut, and the checked-in background at
`apps/desktop/src-tauri/res/dmg-background.png`.

Release builds bundle the matching `tendi` CLI inside the app. The first-run setup or the
**Command line** section in Settings can register it on the user's shell `PATH`; the DMG does not
silently modify shell configuration.

Published releases also include Tauri updater artifacts and `latest.json`. Installed apps can
check for updates from **Settings → Updates**. Release signing requires a repository secret named
`TAURI_SIGNING_PRIVATE_KEY`; an optional
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` is supported when the key is password-protected.

### First launch and Gatekeeper

Browser downloads are tagged with Gatekeeper **quarantine** (`com.apple.quarantine`). The local
DMG build is not signed or notarized. If macOS warns that the app cannot be opened or is from an
unidentified developer, first verify that the DMG came from a trusted source, copy `tendi.app` to
**Applications**, then remove quarantine:

```bash
xattr -dr com.apple.quarantine /Applications/tendi.app
```

The DMG builder runs `xattr -cr` on its temporary app copy to strip extended attributes and avoid
AppleDouble sidecar files in the image. This packaging cleanup does not sign the app or replace
Developer ID signing and Apple notarization.

## Usage

### CLI

Run `cargo run -p tendi-cli -- --help` for the complete generated help. The main command groups
are:

```text
tendi scan [--json]
tendi agents list [--json]
tendi skills guide [--json]
tendi setup skills [--to <shared|codex|cursor|claude>] [options]
tendi skills list [--json]
tendi skills add <source> [--visibility <auto|manual|off>] [options]
tendi skills set <pattern> --visibility <auto|manual|off> [options]
tendi skills wrap <name> --from <pattern> [options]
tendi skills updates [--check] [--json]
tendi skills update <pattern> [options]
tendi skills link <source> --to <codex|cursor|claude|shared> [options]
tendi sessions list [--json]
tendi sessions search <query> [--json]
tendi sessions transcript <path> --agent <codex|cursor|claude|shared> [--json]
tendi rules list [--json]
tendi hooks list [--json]
tendi mcp list [--json]
```

Commands that change files show a plan first. Use `--dry-run` to inspect a change without
applying it and `--yes` to confirm non-interactively.

On the first interactive CLI run, Tendi offers to install its bundled agent skill into
`~/.agents/skills/tendi`. The desktop app shows the same one-time prompt. The installed skill is
a small discovery stub: it asks the current `tendi` binary for `tendi skills guide`, then uses
the version-matched guide to search sessions, inspect agent inventory, and preview or apply skill
changes. Existing files with different content require an explicit `--overwrite`.

### Desktop app

The Tauri app currently includes views for:

- Overview and analytics
- Skills and skill file editing
- Prompts
- Sessions, search, transcript details, and live scan updates
- Rules, hooks, and MCP
- Agent configuration profiles

The React UI uses real local data through Tauri. When opened in a browser without Tauri, it can
use prototype data for UI development.

### Local data and permissions

The core scanner and snapshot database run locally. On macOS, the default database is:

```text
~/Library/Application Support/tendi/tendi.sqlite3
```

The scanner reads agent configuration, skills, rules, hooks, MCP configuration, and local
session files from configured project and global roots. Remote access only occurs when a
requested skill source or update check uses GitHub, Git, a registry, or HTTP.

Global skill roots currently include:

```text
~/.agents/skills
~/.codex/skills
~/.cursor/skills
~/.claude/skills
```

Matching project-level `.agents`, `.codex`, `.cursor`, and `.claude` skill roots are included
when a project context is provided.

## Development

### Repository layout

```text
crates/tendi-core/       scanning, storage, planning, parsing, and domain logic
crates/tendi-cli/        `tendi` command-line interface
apps/desktop/            React/Vite frontend
apps/desktop/src-tauri/  Tauri shell and native commands
scripts/                 acceptance, performance, and smoke checks
docs/                    design and audit notes
```

### Validation

Run focused checks from the repository root:

```bash
cargo test -p tendi-core
cargo check
cd apps/desktop && npm run typecheck && npm run build
```

Run the complete local acceptance gate:

```bash
node scripts/acceptance.mjs
```

The acceptance gate covers core tests, Rust checks, a performance fast pass, real CLI scans,
SQLite count checks, CLI write confirmation, desktop alignment e2e, the frontend build, and the
Tauri bundle build.

Performance checks are also available independently:

```bash
node scripts/perf-check.mjs --fast
node scripts/perf-check.mjs --full
```

To enable the repository-managed pre-push check once per clone:

```bash
git config core.hooksPath .githooks
```

## Contributing

For development setup, coding conventions, tests, asset rules, release boundaries, and security
reports, read **[CONTRIBUTING.md](./CONTRIBUTING.md)**.

## Third-Party Assets and Licenses

The Rust workspace and source repository use the **MIT License**. See **[LICENSE](./LICENSE)**.

The Tendi app icon and DMG background are project assets. Any future bundled fonts, models,
media, SDK components, or other binary assets must include their source, license, attribution,
and redistribution terms before being added to the public repository.

## Current limitations

- Cursor transcript parsing does not yet cover every event shape.
- Rules, hooks, and MCP currently have scan/list workflows but not complete editor flows.
- There are no signed, notarized, or published release binaries yet.
- The updater is wired for signed Tauri artifacts, but no release has been published yet.

## License

The Rust workspace and source repository use the [MIT License](LICENSE). Third-party dependencies
and future bundled assets retain their own licenses and attribution requirements.
