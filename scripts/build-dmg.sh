#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_PATH="${1:?usage: scripts/build-dmg.sh <app-path> <output-dmg> [volume-name]}"
OUTPUT_DMG="${2:?usage: scripts/build-dmg.sh <app-path> <output-dmg> [volume-name]}"
VOLNAME="${3:-tendi}"
DMG_BACKGROUND="$ROOT/apps/desktop/src-tauri/res/dmg-background.png"

for path in "$APP_PATH" "$DMG_BACKGROUND"; do
  if [[ ! -e "$path" ]]; then
    echo "error: missing DMG input: $path" >&2
    exit 1
  fi
done

if ! command -v sips >/dev/null 2>&1; then
  echo "error: sips is required to build a DMG on macOS" >&2
  exit 1
fi

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

mkdir -p "$(dirname "$OUTPUT_DMG")"

APP_COPY="$STAGE/$(basename "$APP_PATH")"
cp -R "$APP_PATH" "$APP_COPY"
find "$APP_COPY" -name '._*' -delete
xattr -cr "$APP_COPY" || true

DMG_BACKGROUND_COPY="$STAGE/dmg-background.png"
DMG_BACKGROUND_RETINA_COPY="$STAGE/dmg-background@2x.png"
sips -z 373 661 "$DMG_BACKGROUND" --out "$DMG_BACKGROUND_COPY" >/dev/null
sips -z 746 1322 "$DMG_BACKGROUND" --out "$DMG_BACKGROUND_RETINA_COPY" >/dev/null
sips -s dpiWidth 72 -s dpiHeight 72 "$DMG_BACKGROUND_COPY" >/dev/null
sips -s dpiWidth 144 -s dpiHeight 144 "$DMG_BACKGROUND_RETINA_COPY" >/dev/null

APPDMG_JSON="$STAGE/appdmg.json"
cat >"$APPDMG_JSON" <<JSON
{
  "title": "$VOLNAME",
  "background": "$DMG_BACKGROUND_COPY",
  "icon-size": 80,
  "window": {
    "position": { "x": 120, "y": 559 },
    "size": { "width": 661, "height": 379 }
  },
  "format": "UDZO",
  "filesystem": "HFS+",
  "contents": [
    { "x": 180, "y": 197, "type": "file", "path": "$APP_COPY" },
    { "x": 480, "y": 197, "type": "link", "path": "/Applications" }
  ]
}
JSON

rm -f "$OUTPUT_DMG"
npx --yes "appdmg@${APPDMG_VERSION:-0.6.6}" "$APPDMG_JSON" "$OUTPUT_DMG"

echo "Built $OUTPUT_DMG"
