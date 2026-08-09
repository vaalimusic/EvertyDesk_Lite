#!/usr/bin/env bash
# Compatibility wrapper for the EvertyDesk Next macOS package.
# The maintained packaging script lives under desktop-next/scripts so release
# automation, local developer runs, and this legacy entrypoint all use the
# same bundle layout and asset pipeline.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec "$REPO_ROOT/desktop-next/scripts/package-macos-dmg.sh" "$@"
