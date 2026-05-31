#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

MODE="${1:-auto}"

echo "EvertyDesk Lite Linux build"
echo "mode: $MODE"
echo

case "$MODE" in
  auto)
    echo "codecs: H264 + VP8/VP9 live video + PNG fallback"
    echo "policy: full codec build; no silent H264-only fallback"
    cargo build --release --no-default-features --features live-h264,live-vpx
    ;;
  h264)
    echo "codecs: H264 live video + PNG fallback"
    echo "policy: explicit fallback build; VP8/VP9 disabled"
    cargo build --release --no-default-features --features live-h264
    ;;
  vpx|vp9|vp8)
    echo "codecs: H264 + VP8/VP9 live video + PNG fallback"
    cargo build --release --no-default-features --features live-h264,live-vpx
    ;;
  *)
    echo "Usage: ./scripts/build-linux.sh [auto|h264|vpx]" >&2
    echo "  auto  default: H264 + VP8/VP9 live video" >&2
    echo "  h264  H264 live video + PNG fallback" >&2
    echo "  vpx   H264 + VP8/VP9 live video + PNG fallback" >&2
    exit 2
    ;;
esac

echo
echo "Built:"
echo "$ROOT/target/release/evertydesk-lite"
