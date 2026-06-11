#!/usr/bin/env bash
set -euo pipefail

APP_ID="evertydesk-lite"
BIN_DST="$HOME/.local/bin/$APP_ID"
BIN_REAL="$HOME/.local/bin/$APP_ID-bin"
SAFE_RUNNER="$HOME/.local/bin/$APP_ID-safe"
DESKTOP_FILE="$HOME/.local/share/applications/$APP_ID.desktop"

rm -f "$BIN_DST" "$BIN_REAL" "$SAFE_RUNNER" "$DESKTOP_FILE"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$HOME/.local/share/applications" >/dev/null 2>&1 || true
fi

echo "EvertyDesk Lite removed from current user."
