#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="$(node -e 'const fs=require("node:fs"); console.log(JSON.parse(fs.readFileSync("apps/desktop/src-tauri/tauri.conf.json", "utf8")).version)')"
RELEASE_ARCHES="${TENDI_RELEASE_ARCHS:-}"
if [[ -z "$RELEASE_ARCHES" ]]; then
  RELEASE_ARCHES="$(uname -m)"
fi

UPDATER_ENABLED="${TENDI_INCLUDE_UPDATER_ARTIFACTS:-0}"
if [[ "$UPDATER_ENABLED" == "1" && -z "${TAURI_SIGNING_PRIVATE_KEY:-}" && -n "${TAURI_SIGNING_PRIVATE_KEY_PATH:-}" ]]; then
  if [[ ! -f "$TAURI_SIGNING_PRIVATE_KEY_PATH" ]]; then
    echo "error: updater private key not found at $TAURI_SIGNING_PRIVATE_KEY_PATH" >&2
    exit 1
  fi
  export TAURI_SIGNING_PRIVATE_KEY="$(<"$TAURI_SIGNING_PRIVATE_KEY_PATH")"
fi
if [[ "$UPDATER_ENABLED" == "1" && -z "${TAURI_SIGNING_PRIVATE_KEY:-}" && -z "${TAURI_SIGNING_PRIVATE_KEY_PATH:-}" ]]; then
  echo "error: updater release requires TAURI_SIGNING_PRIVATE_KEY or TAURI_SIGNING_PRIVATE_KEY_PATH" >&2
  exit 1
fi

mkdir -p "$ROOT/dist"
rm -f "$ROOT"/dist/tendi-${VERSION}-*.dmg \
  "$ROOT"/dist/tendi-${VERSION}-*.app.tar.gz \
  "$ROOT"/dist/tendi-${VERSION}-*.app.tar.gz.sig \
  "$ROOT/dist/latest.json"

build_arch() {
  local target="$1"
  local arch="$2"
  local target_triple="$target"
  local host_triple
  host_triple="$(rustc -vV | awk '/^host:/ { print $2 }')"
  local build_args=(--bundles app --ci --config src-tauri/tauri.release.conf.json)
  local target_args=()
  local cargo_target_args=()
  local cli_output_dir="$ROOT/target/$target/release"
  if [[ "$target" != "host" ]]; then
    target_args=(--target "$target")
    cargo_target_args=(--target "$target")
  else
    target_triple="$host_triple"
    cli_output_dir="$ROOT/target/release"
  fi
  if [[ "$UPDATER_ENABLED" == "1" ]]; then
    build_args+=(--config '{"bundle":{"createUpdaterArtifacts":true}}')
  fi

  cargo build --release -p tendi-cli "${cargo_target_args[@]}"
  local sidecar_dir="$ROOT/apps/desktop/src-tauri/binaries"
  local sidecar="$sidecar_dir/tendi-$target_triple"
  mkdir -p "$sidecar_dir"
  cp "$cli_output_dir/tendi" "$sidecar"
  chmod 755 "$sidecar"

  npm --prefix "$ROOT/apps/desktop" run build:tauri -- "${target_args[@]}" "${build_args[@]}"

  local bundle_root="$ROOT/target/$target/release/bundle"
  if [[ "$target" == "host" ]]; then
    bundle_root="$ROOT/target/release/bundle"
  fi
  local app="$bundle_root/macos/tendi.app"
  local updater_src="$bundle_root/macos/tendi.app.tar.gz"
  local updater_sig_src="$updater_src.sig"
  local dmg="$ROOT/dist/tendi-${VERSION}-${arch}.dmg"
  local bundled_cli="$app/Contents/MacOS/tendi"

  [[ -d "$app" ]] || { echo "error: missing app bundle at $app" >&2; exit 1; }
  [[ -x "$bundled_cli" ]] || { echo "error: missing bundled CLI at $bundled_cli" >&2; exit 1; }
  if [[ "$target_triple" == "$host_triple" ]]; then
    "$bundled_cli" --version | grep -q "^tendi $VERSION$" || {
      echo "error: bundled CLI version does not match app version $VERSION" >&2
      exit 1
    }
  else
    local expected_arch
    case "$target_triple" in
      aarch64-apple-darwin) expected_arch="arm64" ;;
      x86_64-apple-darwin) expected_arch="x86_64" ;;
      *) echo "error: unsupported target triple: $target_triple" >&2; exit 1 ;;
    esac
    file "$bundled_cli" | grep -q "Mach-O 64-bit executable $expected_arch" || {
      echo "error: bundled CLI does not match target architecture $expected_arch" >&2
      exit 1
    }
  fi
  "$ROOT/scripts/build-dmg.sh" "$app" "$dmg" tendi

  if [[ "$UPDATER_ENABLED" == "1" ]]; then
    [[ -f "$updater_src" ]] || { echo "error: missing updater archive at $updater_src" >&2; exit 1; }
    [[ -f "$updater_sig_src" ]] || { echo "error: missing updater signature at $updater_sig_src" >&2; exit 1; }
    cp "$updater_src" "$ROOT/dist/tendi-${VERSION}-${arch}.app.tar.gz"
    cp "$updater_sig_src" "$ROOT/dist/tendi-${VERSION}-${arch}.app.tar.gz.sig"
  fi
}

for requested_arch in $RELEASE_ARCHES; do
  case "$requested_arch" in
    arm64|aarch64)
      build_arch aarch64-apple-darwin aarch64
      ;;
    x86_64|x64)
      build_arch x86_64-apple-darwin x64
      ;;
    host)
      case "$(uname -m)" in
        arm64) build_arch host aarch64 ;;
        x86_64) build_arch host x64 ;;
        *) echo "error: unsupported host architecture: $(uname -m)" >&2; exit 1 ;;
      esac
      ;;
    *)
      echo "error: unsupported release arch: $requested_arch" >&2
      exit 1
      ;;
  esac
done

if [[ "$UPDATER_ENABLED" == "1" && -f "$ROOT/dist/tendi-${VERSION}-aarch64.app.tar.gz.sig" && -f "$ROOT/dist/tendi-${VERSION}-x64.app.tar.gz.sig" ]]; then
  "$ROOT/scripts/write-latest-json.sh"
fi

for requested_arch in $RELEASE_ARCHES; do
  echo "Built tendi ${VERSION} for ${requested_arch}"
done
