# Tauri Updater

Tendi uses the Tauri updater plugin, following the updater workflow used by `stiki`. It does not
embed Sparkle or commit a framework binary into the repository.

## Local key

Generate a project-specific signing key from `apps/desktop`:

```bash
npx tauri signer generate --write-keys ~/.tauri/tendi.key
```

The command writes the private key and public key separately. The public key belongs in
`apps/desktop/src-tauri/tauri.conf.json`; the private key must stay outside the repository.

For a local signed build:

```bash
TENDI_INCLUDE_UPDATER_ARTIFACTS=1 \
TAURI_SIGNING_PRIVATE_KEY_PATH=~/.tauri/tendi.key \
npm run build:dmg --prefix apps/desktop
```

This produces the DMG, `tendi-<version>-<arch>.app.tar.gz`, and its `.sig` file under `dist/`.

## Release artifacts

`.github/workflows/release.yml` builds the Apple Silicon artifact, publishes the DMG to a GitHub
Release, and uploads the updater archive, signature, and `latest.json` to the same release. The
app checks:

```text
https://github.com/rhinoc/tendi/releases/latest/download/latest.json
```

Configure these GitHub Actions secrets before running a release:

- `TAURI_SIGNING_PRIVATE_KEY`: full contents of the private key file.
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: optional password, if the key is protected.

The updater manifest includes Apple Silicon entries (`darwin-aarch64` and
`darwin-aarch64-app`) only. Releases support Apple Silicon Macs.

## Application behavior

After startup, Tendi silently checks for updates at most once per day. If a signed update is found,
it shows a non-blocking notification with the available version; it does not download or install
the update automatically. Choose **Install** in the notification, or open **Settings → Check for
Updates** and install it from there. Tendi downloads the signed package, installs it, and restarts
into the new version only after that action.

Manual checks bypass the daily cooldown. Automatic check failures stay silent; manual failures are
shown in the settings action.

Local DMGs and unsigned builds remain subject to Gatekeeper rules. See the installation notes in
the [README](../README.md#first-launch-and-gatekeeper).
