#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="${1:-debug}"
TARGET_DIR="$ROOT_DIR/target/$PROFILE"
APP_DIR="$TARGET_DIR/EvertyDesk Next.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"
ICONSET_DIR="$TARGET_DIR/EvertyDeskNext.iconset"

LAUNCHER="$TARGET_DIR/evertydesk-launcher"
VIEWER="$TARGET_DIR/evertydesk-viewer"

if [[ ! -x "$LAUNCHER" || ! -x "$VIEWER" ]]; then
  echo "Build launcher and viewer before bundling:" >&2
  echo "  cargo build --manifest-path desktop-next/Cargo.toml --bin evertydesk-launcher --features viewer-core" >&2
  echo "  cargo build --manifest-path desktop-next/Cargo.toml --bin evertydesk-viewer --features viewer-core" >&2
  exit 1
fi

mkdir -p "$MACOS_DIR" "$RESOURCES_DIR" "$ICONSET_DIR"

cp "$ROOT_DIR/macos/Info.plist" "$CONTENTS_DIR/Info.plist"
cp "$LAUNCHER" "$MACOS_DIR/evertydesk-launcher"
cp "$VIEWER" "$MACOS_DIR/evertydesk-viewer"
chmod 755 "$MACOS_DIR/evertydesk-launcher" "$MACOS_DIR/evertydesk-viewer"

sips -z 16 16 "$ROOT_DIR/desktop-next-logo.png" --out "$ICONSET_DIR/icon_16x16.png" >/dev/null
sips -z 32 32 "$ROOT_DIR/desktop-next-logo.png" --out "$ICONSET_DIR/icon_16x16@2x.png" >/dev/null
sips -z 32 32 "$ROOT_DIR/desktop-next-logo.png" --out "$ICONSET_DIR/icon_32x32.png" >/dev/null
sips -z 64 64 "$ROOT_DIR/desktop-next-logo.png" --out "$ICONSET_DIR/icon_32x32@2x.png" >/dev/null
sips -z 128 128 "$ROOT_DIR/desktop-next-logo.png" --out "$ICONSET_DIR/icon_128x128.png" >/dev/null
sips -z 256 256 "$ROOT_DIR/desktop-next-logo.png" --out "$ICONSET_DIR/icon_128x128@2x.png" >/dev/null
sips -z 256 256 "$ROOT_DIR/desktop-next-logo.png" --out "$ICONSET_DIR/icon_256x256.png" >/dev/null
sips -z 512 512 "$ROOT_DIR/desktop-next-logo.png" --out "$ICONSET_DIR/icon_256x256@2x.png" >/dev/null
sips -z 512 512 "$ROOT_DIR/desktop-next-logo.png" --out "$ICONSET_DIR/icon_512x512.png" >/dev/null
sips -z 1024 1024 "$ROOT_DIR/desktop-next-logo.png" --out "$ICONSET_DIR/icon_512x512@2x.png" >/dev/null
iconutil -c icns "$ICONSET_DIR" -o "$RESOURCES_DIR/EvertyDeskNext.icns"

echo "$APP_DIR"
