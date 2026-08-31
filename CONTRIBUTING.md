# Contributing to tendi

Thanks for taking the time to improve `tendi`. This project is a local-first macOS desktop app
and CLI for discovering and managing coding-agent state, so contributions should keep both user
data safety and public-repository hygiene in mind.

## Development Setup

Requirements:

- macOS for the current Tauri desktop workflow
- Rust stable toolchain with Cargo
- Node.js and npm
- Git
- `sqlite3` on `PATH` for the complete acceptance gate

The repo is a Cargo workspace with a React/Vite frontend and a Tauri shell. The CLI and core
crates are built with Cargo; the desktop frontend uses npm; the packaged app and local DMG are
built through the scripts in `scripts/`.

Clone and verify the project:

```bash
git clone https://github.com/rhinoc/tendi.git
cd tendi
cargo test -p tendi-core
cargo check
cd apps/desktop
npm ci
npm run typecheck
npm run build
```

Run the app with a project context when project-level agent files should be included:

```bash
TENDI_CWD=/path/to/project npm run dev:tauri
```

Create a local release DMG:

```bash
npm run build:dmg
```

The DMG builder uses `sips` and downloads `appdmg` through `npx`; it requires macOS and network
access for the first `appdmg` invocation. It runs `xattr -cr` on the temporary app copy to strip
extended attributes before packaging. This is DMG hygiene, not code signing or notarization.

Local DMGs are not signed or notarized. Browser downloads may add Gatekeeper **quarantine**
(`com.apple.quarantine`). After verifying the source and copying the app to Applications, remove
that metadata only for the app you intend to run:

```bash
xattr -dr com.apple.quarantine /Applications/tendi.app
```

## Pull Requests

- Keep pull requests focused on one behavior or one small set of related files.
- Include tests when changing parsing, scanning, storage, file writes, CLI behavior, UI behavior, or release scripts.
- Run the relevant focused checks and the complete acceptance gate when practical.
- Update `README.md`, `docs/`, or a focused document when behavior, dependencies, assets, release artifacts, or user-facing setup changes.
- Call out macOS-specific behavior, migration needs, known limitations, and any environment-dependent validation.

## Code Style

- Follow the existing Rust, React, TypeScript, and Tauri patterns.
- Fix the underlying cause and keep changes scoped to the affected workflow.
- Prefer hash checks, atomic writes, and explicit previews for file-changing operations.
- Do not expose environment values, tokens, hook output, full local paths containing usernames, or private transcripts in UI data, logs, tests, or documentation.
- Add or update tests when changing parsing, scanning, storage, file writes, or CLI behavior.

## Assets and Third-Party Content

Do not add new bundled artwork, fonts, models, media, SDK files, or binary blobs unless the
change records:

- Original source URL or creator.
- License or usage terms.
- Redistribution permission for inclusion in this repository and packaged app.
- Attribution text when required.
- File size and runtime impact.

For uncertain assets, prefer a downloader or user-supplied import path over bundling the file.
Do not commit generated data, screenshots, build output, credentials, API keys, certificates,
private keys, signing exports, passwords, or release artifacts.

## Release and Signing

Release and packaging automation lives in:

- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- `.changeset/`
- `scripts/build-release.sh`
- `scripts/build-dmg.sh`
- `scripts/render_changesets.sh`
- `scripts/consume_changesets.sh`
- `scripts/write-latest-json.sh`
- `apps/desktop/src-tauri/tauri.conf.json`
- `apps/desktop/src-tauri/res/dmg-background.png`

Changes to these files should explain how local builds, CI artifacts, DMG layout, background
assets, updater artifacts, signing, and notarization are affected. The release workflow builds
Tauri updater archives and DMGs for Apple Silicon, then publishes them with
`latest.json` to GitHub Releases. Add one English `.changeset/*.md` file for every user-visible
release change. The release workflow renders those entries into the updater notes and GitHub
Release body, then removes the consumed entries in its metadata commit.

Tauri updater signing uses a project-specific key. Keep the private key outside the repository.
Configure these GitHub Actions secrets before publishing:

| Secret | Description |
|--------|-------------|
| `TAURI_SIGNING_PRIVATE_KEY` | Contents of the Tauri updater private key file |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Optional password for that key |

For local key generation, use `npx tauri signer generate --write-keys ~/.tauri/tendi.key` from
`apps/desktop`. Never commit the private key or generated release artifacts.

## Security Reports

Do not open a public issue for a vulnerability, exposed credential, signing key, or private local
data. Follow **[SECURITY.md](./SECURITY.md)** instead.
