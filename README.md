# EvertyDesk Lite

EvertyDesk Lite is a minimal Rust + egui remote desktop client for the EvertyDesk infrastructure.

The goal is simple: a small, fast, native remote access tool that starts quickly, has no heavy UI stack, and can be built for Windows and Linux, including conservative enterprise Linux environments such as Astra Linux.

This repository contains only the EvertyDesk Lite client. The `rustdesk-master/` directory is used locally as protocol documentation only and is intentionally ignored by Git.

## Why EvertyDesk Lite

Remote desktop tools became too heavy for many everyday support tasks. EvertyDesk Lite takes the opposite path:

- minimal native UI built with Rust + egui;
- direct EvertyDesk server configuration;
- RustDesk-compatible rendezvous/relay protocol work;
- separate remote desktop viewer window;
- screenshot fallback plus live-video codec negotiation;
- mouse, keyboard, text input, display switching, and connection diagnostics;
- simple codebase that can grow into a controlled support platform.

This is the first public step. Give it a little time, and EvertyDesk will become more than a remote desktop client: it will become a compact automation workstation for support teams, administrators, integrators, and field engineers.

## Current Features

- Native Rust + egui desktop app.
- EvertyDesk server settings built in.
- Connection progress with clear diagnostics.
- Remote desktop viewer in a separate window.
- Working remote image display through an optimized screenshot fallback.
- Live H264 support in the default build.
- Optional VP8/VP9 support for RustDesk-compatible live video.
- Mouse control with coordinate modes for different monitor layouts.
- Keyboard input, text send, Enter, Ctrl+Alt+Del, Lock.
- Multi-display selection.
- Auto refresh with configurable interval.
- Recent remote IDs.
- Clean disconnect and window close handling.
- Background PNG decode and texture reuse for smoother UI.
- Codec sync through RustDesk-compatible `SupportedDecoding` updates.

## Current Status

EvertyDesk Lite is now a working minimal client. It connects through the EvertyDesk/RustDesk-compatible server path, opens a remote screen window, displays the desktop, and sends mouse/keyboard input.

The default Windows/MSVC build supports H264 and keeps a screenshot fallback for compatibility. VP8/VP9 support is available through the `live-vpx` feature and is the preferred path when the remote RustDesk host chooses VP9.

## Server Defaults

```text
API URL:     https://desk.everty.ru
ID server:   edesk.server1.everty.ru
Relay server: edesk.server1.everty.ru
Public key:  MrGdbay3g8Qr84YYnxr4qLjw5zLWM1oAOdfehbBnlRs=
```

## Run

```powershell
cargo run
```

## Build

Debug build:

```powershell
cargo build
```

Release build:

```powershell
cargo build --release
```

Release binary:

```text
target\release\evertydesk-lite.exe
```

This build is the normal Windows/MSVC release with H264 support.

Default H264 build:

```powershell
cargo build --features live-h264
```

Windows VP8/VP9 build:

```powershell
.\scripts\build-windows-vpx.ps1
```

The VPX script downloads a portable `w64devkit` toolchain into `engine/` and builds:

```text
target\x86_64-pc-windows-gnu\release\evertydesk-lite.exe
```

Use this VPX build when connecting to a regular RustDesk host or when the viewer
stays in PNG fallback mode. It advertises VP8/VP9 plus H264 and is the preferred
build for low-latency live video testing.

`engine/` is ignored by Git.

## Linux Notes

The client is designed to stay lightweight and portable. For Linux builds, install the usual Rust desktop dependencies for `eframe`/`egui` and your distribution graphics stack.

On Astra Linux / Debian-like systems:

```bash
sudo apt update
sudo apt install -y \
  build-essential pkg-config curl ca-certificates git cmake clang nasm \
  libx11-dev libxi-dev libxcursor-dev libxrandr-dev libxinerama-dev \
  libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev \
  libwayland-dev libasound2-dev libudev-dev libxtst-dev \
  libgl1-mesa-dev libegl1-mesa-dev libvulkan1 mesa-vulkan-drivers \
  xdotool
```

For Wayland host sessions, install the optional helpers available in your
distribution: `ydotool` + running `ydotoold` for input and `grim` for capture.
X11/Xorg remains the fastest and most complete Linux host backend.

Install Rust if it is not installed:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

Build release. By default the script builds the full codec client:
H264 + VP8/VP9 live video + PNG fallback. It does not silently fall back to
H264-only, because that hides VP9 decoder problems and causes slow PNG mode.

```bash
./scripts/build-linux.sh
```

Explicit codec builds:

```bash
./scripts/build-linux.sh h264
./scripts/build-linux.sh vpx
```

Install for the current Linux user so it opens from the application menu:

```bash
chmod +x scripts/build-linux.sh scripts/install-linux-user.sh
./scripts/build-linux.sh
./scripts/install-linux-user.sh
```

After that, open **EvertyDesk Lite** from the Astra/Linux application menu.
The installer copies the binary to `~/.local/bin/evertydesk-lite` and creates:

```text
~/.local/share/applications/evertydesk-lite.desktop
```

On Linux the binary starts in automatic renderer mode. It tries the safe GUI
paths in separate child processes so a broken GLX/WGPU backend cannot crash the
main launcher:

1. Wayland OpenGL software, when Wayland is available.
2. X11 OpenGL software with conservative Astra/Mesa variables.
3. WGPU hardware/software backend.
4. CPU software UI backend if OpenGL/Vulkan are both rejected.
5. Headless host mode only when no desktop session exists.

To force hardware rendering manually:

```bash
EVERTYDESK_RENDERER=wgpu ~/.local/bin/evertydesk-lite
```

Run without installing:

```bash
chmod +x scripts/run-linux-safe.sh
./scripts/run-linux-safe.sh
```

Check the Linux desktop/graphics environment with the same startup file:

```bash
./scripts/run-linux-safe.sh --check
```

If no renderer works, install runtime graphics packages and collect diagnostics:

```bash
chmod +x scripts/install-linux-runtime-deps.sh scripts/diagnose-linux-graphics.sh
./scripts/install-linux-runtime-deps.sh
./scripts/diagnose-linux-graphics.sh
```

Uninstall:

```bash
chmod +x scripts/uninstall-linux-user.sh
./scripts/uninstall-linux-user.sh
```

Build VP8/VP9 release:

```bash
./scripts/build-linux.sh vpx
```

Linux builds intentionally use `--no-default-features` because the default
Windows build enables Windows Media Foundation VP9. The Linux scripts enable
only portable codec features.

The long-term target is a simple package flow for:

- Windows;
- common Linux distributions;
- Astra Linux;
- portable admin builds.

## Documentation

- [AI terminal setup](docs/terminal-llm.md)

## Roadmap

Next planned direction:

- Improve VP8/VP9 live video performance and packaging.
- Add codec/fps diagnostics in the UI.
- Better keyboard mapping for mixed Windows/Linux sessions.
- Terminal inside EvertyDesk Lite.
- Script runner for support and administration tasks.
- Automation templates for common service operations.
- Automation Store: a curated marketplace of safe scripts, checks, repairs, deployment steps, and customer-specific workflows.
- Role-based automation access.
- Audit log for remote actions.
- One-click diagnostics bundle.
- File transfer.
- Session recording and support reports.

## Vision

EvertyDesk should not be just another remote desktop window.

The larger idea is a remote workbench:

- connect to a machine;
- see the desktop;
- control it;
- open terminal;
- run approved scripts;
- install or repair software;
- collect diagnostics;
- share automation packages through a store;
- keep everything small, auditable, and understandable.

EvertyDesk Lite is the minimal client that starts this path.

## Repository Policy

- Do not commit `rustdesk-master/`.
- Do not commit `target/`.
- Do not commit local passwords, config files, logs, or `.env` files.
- Keep the client small and practical.
