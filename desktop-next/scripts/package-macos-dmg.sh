#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${EVERTYDESK_RELEASE_VERSION:-}"

if [[ -z "$VERSION" ]]; then
  VERSION="$(awk -F'"' '/^version[[:space:]]*=/ { print $2; exit }' "$ROOT_DIR/Cargo.toml")"
fi

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "DMG version must be numeric X.Y.Z, got '$VERSION'" >&2
  exit 1
fi

cd "$ROOT_DIR"
cargo build --release --features viewer-core --bins

DIST_DIR="$ROOT_DIR/dist"
TARGET_DIR="$ROOT_DIR/target/release"
APP_DIR="$TARGET_DIR/EvertyDesk Next.app"
DMG_PATH="$DIST_DIR/EvertyDeskNext-$VERSION.dmg"

mkdir -p "$DIST_DIR"
"$ROOT_DIR/tools/bundle-macos.sh" release >/dev/null

python3 - "$APP_DIR/Contents/Info.plist" "$VERSION" <<'PY'
import plistlib
import sys

path, version = sys.argv[1], sys.argv[2]
with open(path, "rb") as f:
    plist = plistlib.load(f)
plist["CFBundleShortVersionString"] = version
plist["CFBundleVersion"] = version
with open(path, "wb") as f:
    plistlib.dump(plist, f)
PY

rm -f "$DMG_PATH"
hdiutil create \
  -volname "EvertyDesk Next" \
  -srcfolder "$APP_DIR" \
  -ov \
  -format UDZO \
  "$DMG_PATH"

shasum -a 256 "$DMG_PATH" | awk -v name="$(basename "$DMG_PATH")" '{print $1 "  " name}' > "$DMG_PATH.sha256"

echo "$DMG_PATH"
