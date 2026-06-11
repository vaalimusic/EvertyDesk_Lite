#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/release/evertydesk-lite"

if [[ $# -gt 0 && "${1:-}" != --* ]]; then
  BIN="$1"
  shift
fi

check_runtime() {
  echo "EvertyDesk Lite Linux check"
  echo "binary: $BIN"
  if [[ -x "$BIN" ]]; then
    echo "binary: OK"
  else
    echo "binary: missing or not executable"
  fi
  echo "session: ${XDG_SESSION_TYPE:-unknown}"
  echo "DISPLAY: ${DISPLAY:-not set}"
  echo "WAYLAND_DISPLAY: ${WAYLAND_DISPLAY:-not set}"
  if command -v glxinfo >/dev/null 2>&1; then
    glxinfo -B 2>/dev/null | sed -n '1,12p' || true
  else
    echo "glxinfo: not installed"
  fi
}

if [[ "${1:-}" == "--check" ]]; then
  check_runtime
  exit 0
fi

if [[ ! -x "$BIN" ]]; then
  echo "Binary not found or not executable: $BIN" >&2
  echo "Build first: ./scripts/build-linux.sh" >&2
  exit 2
fi

if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
  echo "EvertyDesk Lite: no desktop session detected, starting headless host."
  exec env EVERTYDESK_RENDERER=headless "$BIN" "$@"
fi

exec "$BIN" "$@"
