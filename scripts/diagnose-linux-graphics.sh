#!/usr/bin/env bash
set -uo pipefail

echo "EvertyDesk Lite Linux graphics diagnostics"
echo

echo "Session:"
echo "  XDG_SESSION_TYPE=${XDG_SESSION_TYPE:-}"
echo "  DISPLAY=${DISPLAY:-}"
echo "  WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-}"
echo

echo "Tools:"
for tool in glxinfo vulkaninfo xdpyinfo; do
  if command -v "$tool" >/dev/null 2>&1; then
    echo "  $tool: $(command -v "$tool")"
  else
    echo "  $tool: missing"
  fi
done
echo

if command -v glxinfo >/dev/null 2>&1; then
  echo "OpenGL:"
  glxinfo -B 2>&1 | sed 's/^/  /'
  echo
else
  echo "OpenGL: glxinfo missing. Install mesa-utils."
  echo
fi

if command -v xdpyinfo >/dev/null 2>&1; then
  echo "X11 GLX extension:"
  xdpyinfo 2>/dev/null | grep -E 'GLX|MIT-SHM|DRI' | sed 's/^/  /' || echo "  no GLX/DRI lines found"
  echo
fi

if command -v vulkaninfo >/dev/null 2>&1; then
  echo "Vulkan summary:"
  vulkaninfo --summary 2>&1 | sed 's/^/  /' | head -120
  echo
else
  echo "Vulkan: vulkaninfo missing. Install vulkan-tools."
  echo
fi

echo "EvertyDesk renderer probes:"
if [[ -x ./target/release/evertydesk-lite ]]; then
  echo "  Binary exists: ./target/release/evertydesk-lite"
else
  echo "  Binary missing: ./target/release/evertydesk-lite"
fi
