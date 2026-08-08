#!/usr/bin/env bash
# Builds evertydesk-launcher / evertydesk-viewer in release mode, wraps the
# launcher in a minimal .app bundle, and packages it as an unsigned DMG.
#
# Unsigned + not notarized: Gatekeeper will block first launch until this is
# signed with a real Apple Developer ID and notarized. See
# desktop-next/RELEASE.md for that TODO. Until then, users need to right-click
# > Open the first time (or `xattr -d com.apple.quarantine`).
#
# Usage: scripts/package-macos-dmg.sh
#
# Set EVERTYDESK_RELEASE_VERSION to package under a specific version (e.g.
# for the monthly scheduled release) instead of the one committed in
# desktop-next/Cargo.toml.
#
# Output: dist/EvertyDeskLite-<version>.dmg, plus a .sha256 next to it.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DESKTOP_NEXT="$REPO_ROOT/desktop-next"
DIST_DIR="$REPO_ROOT/dist"

echo "==> Building release binaries"
(cd "$DESKTOP_NEXT" && cargo build --release --bin evertydesk-launcher --bin evertydesk-viewer --features viewer-core)

if [ -n "${EVERTYDESK_RELEASE_VERSION:-}" ]; then
    VERSION="$EVERTYDESK_RELEASE_VERSION"
    echo "==> Packaging version $VERSION (override via EVERTYDESK_RELEASE_VERSION)"
else
    VERSION=$(grep -m1 '^version' "$DESKTOP_NEXT/Cargo.toml" | sed -E 's/version *= *"([^"]+)"/\1/')
    echo "==> Packaging version $VERSION"
fi

APP_NAME="EvertyDesk Lite.app"
STAGE_DIR=$(mktemp -d)
APP_DIR="$STAGE_DIR/$APP_NAME"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

BIN_DIR="$DESKTOP_NEXT/target/release"
cp "$BIN_DIR/evertydesk-launcher" "$MACOS_DIR/evertydesk-launcher"
cp "$BIN_DIR/evertydesk-viewer" "$MACOS_DIR/evertydesk-viewer"
chmod +x "$MACOS_DIR/evertydesk-launcher" "$MACOS_DIR/evertydesk-viewer"

cat > "$CONTENTS_DIR/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>EvertyDesk Lite</string>
    <key>CFBundleDisplayName</key>
    <string>EvertyDesk Lite</string>
    <key>CFBundleIdentifier</key>
    <string>ru.everty.evertydesk-lite</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleExecutable</key>
    <string>evertydesk-launcher</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>EvertyDesk Lite streams system audio during a remote session.</string>
</dict>
</plist>
PLIST

# TODO: no .icns yet (only desktop-next/assets/logo.ico exists). Generate one
# with `sips`/`iconutil` from source artwork and drop it in Resources/ as
# AppIcon.icns, then add CFBundleIconFile=AppIcon to Info.plist above.

mkdir -p "$DIST_DIR"
OUTPUT_DMG="$DIST_DIR/EvertyDeskLite-$VERSION.dmg"
rm -f "$OUTPUT_DMG"

hdiutil create -volname "EvertyDesk Lite" -srcfolder "$STAGE_DIR" -ov -format UDZO "$OUTPUT_DMG"
rm -rf "$STAGE_DIR"

shasum -a 256 "$OUTPUT_DMG" | awk '{print $1}' > "$OUTPUT_DMG.sha256"

echo "==> Built $OUTPUT_DMG"
echo "==> SHA256: $(cat "$OUTPUT_DMG.sha256")"
