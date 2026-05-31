#!/usr/bin/env bash
set -euo pipefail

if ! command -v apt >/dev/null 2>&1; then
  echo "This helper expects an Astra/Debian-like system with apt." >&2
  exit 2
fi

sudo apt update
sudo apt install -y \
  libgl1-mesa-dri \
  libgl1-mesa-glx \
  libegl1-mesa \
  libgles2-mesa \
  mesa-utils \
  libvulkan1 \
  mesa-vulkan-drivers \
  vulkan-tools \
  libx11-6 \
  libx11-xcb1 \
  libxcb1 \
  libxcb-glx0 \
  libxcb-dri2-0 \
  libxcb-dri3-0 \
  libxcb-present0 \
  libxcb-sync1 \
  libxcb-xfixes0 \
  libxcb-shape0 \
  libxdamage1 \
  libxfixes3 \
  libxrandr2 \
  libxrender1 \
  libxi6 \
  libxcursor1 \
  libxinerama1 \
  libxkbcommon0 \
  libwayland-client0 \
  libwayland-cursor0 \
  libwayland-egl1

echo
echo "Runtime graphics packages installed."
echo "Log out/in if the desktop session keeps the old GL state."
