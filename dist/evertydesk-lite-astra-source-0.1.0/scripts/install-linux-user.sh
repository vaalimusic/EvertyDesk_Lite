#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_SRC="$ROOT/target/release/evertydesk-lite"
SAFE_RUNNER_SRC="$ROOT/scripts/run-linux-safe.sh"
APP_ID="evertydesk-lite"
APP_NAME="EvertyDesk Lite"
BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"
BIN_DST="$BIN_DIR/$APP_ID"
BIN_REAL="$BIN_DIR/$APP_ID-bin"
SAFE_RUNNER="$BIN_DIR/$APP_ID-safe"
DESKTOP_FILE="$APP_DIR/$APP_ID.desktop"

if [[ ! -x "$BIN_SRC" ]]; then
  echo "Release binary not found: $BIN_SRC" >&2
  echo "Build it first:" >&2
  echo "  ./scripts/build-linux.sh" >&2
  exit 2
fi

mkdir -p "$BIN_DIR" "$APP_DIR"
install -m 0755 "$BIN_SRC" "$BIN_REAL"
install -m 0755 "$SAFE_RUNNER_SRC" "$SAFE_RUNNER"

cat > "$BIN_DST" <<EOF
#!/usr/bin/env bash
exec "$SAFE_RUNNER" "$BIN_REAL" "\$@"
EOF
chmod 0755 "$BIN_DST"

cat > "$DESKTOP_FILE" <<EOF
[Desktop Entry]
Type=Application
Name=$APP_NAME
Comment=Lightweight remote desktop client
Exec=$BIN_DST
Icon=computer
Terminal=false
Categories=Network;RemoteAccess;
StartupNotify=true
EOF

chmod 0644 "$DESKTOP_FILE"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
fi

echo "$APP_NAME installed for current user."
echo "Binary:  $BIN_REAL"
echo "Wrapper: $BIN_DST"
echo "Safe runner: $SAFE_RUNNER"
echo "Launcher: $DESKTOP_FILE"
echo
echo "Open it from the application menu, or run:"
echo "  $BIN_DST"
