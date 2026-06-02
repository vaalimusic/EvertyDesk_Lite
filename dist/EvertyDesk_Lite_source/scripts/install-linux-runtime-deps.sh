#!/usr/bin/env bash
set -euo pipefail

install_optional() {
  local manager="$1"
  shift
  for pkg in "$@"; do
    if ! sudo "$manager" install -y "$pkg"; then
      echo "Optional package not available: $pkg"
    fi
  done
}

if command -v apt >/dev/null 2>&1; then
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
    libxtst6 \
    libxcursor1 \
    libxinerama1 \
    libxkbcommon0 \
    libwayland-client0 \
    libwayland-cursor0 \
    libwayland-egl1
  install_optional apt xdotool ydotool wtype grim
elif command -v dnf >/dev/null 2>&1; then
  sudo dnf install -y \
    mesa-dri-drivers \
    mesa-libGL \
    mesa-libEGL \
    mesa-libGLES \
    mesa-vulkan-drivers \
    vulkan-loader \
    vulkan-tools \
    libX11 \
    libX11-xcb \
    libxcb \
    libXdamage \
    libXfixes \
    libXrandr \
    libXrender \
    libXi \
    libXtst \
    libXcursor \
    libXinerama \
    libxkbcommon \
    wayland
  install_optional dnf xdotool ydotool wtype grim
elif command -v yum >/dev/null 2>&1; then
  sudo yum install -y \
    mesa-dri-drivers \
    mesa-libGL \
    mesa-libEGL \
    mesa-libGLES \
    mesa-vulkan-drivers \
    vulkan-loader \
    libX11 \
    libX11-xcb \
    libxcb \
    libXdamage \
    libXfixes \
    libXrandr \
    libXrender \
    libXi \
    libXtst \
    libXcursor \
    libXinerama \
    libxkbcommon \
    wayland
  install_optional yum xdotool ydotool wtype grim
else
  echo "Supported package managers: apt, dnf, yum." >&2
  exit 2
fi

echo
echo "Runtime graphics/input packages installed."
echo "For Wayland input via ydotool, make sure ydotoold is running and allowed to use /dev/uinput."
echo "Log out/in if the desktop session keeps the old GL/Input state."
