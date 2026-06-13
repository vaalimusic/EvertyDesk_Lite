# EvertyDesk Lite Astra source package

This archive contains source code only. It intentionally does not include:

- Windows/Android build artifacts
- target/
- local keystores or secrets
- NVIDIA Video Codec SDK
- local toolchains from engine/
- external RustDesk/EvertyGame working copies

## Build on Astra Linux

Install Rust if it is not installed:

`ash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "\C:\Users\VAALI/.cargo/env"
`

Install build dependencies:

`ash
sudo apt update
sudo apt install -y build-essential pkg-config cmake nasm \
    libx11-dev libxcb1-dev libxkbcommon-dev \
    libgl1-mesa-dev libegl1-mesa-dev \
    libasound2-dev libxtst-dev xdotool libvpx-dev
`

Build the Astra-friendly profile:

`ash
chmod +x scripts/*.sh
./scripts/build-linux.sh astra
`

Run:

`ash
./target/release/evertydesk-lite
`

If GLX/OpenGL is unstable in the VM, use the CPU UI path:

`ash
EVERTYDESK_RENDERER=software ./target/release/evertydesk-lite
`

On Astra, auto startup prefers the CPU framebuffer UI first. Override only for
diagnostics:

`ash
# Force normal GL auto attempts before CPU fallback.
EVERTYDESK_LINUX_GL_AUTO=1 ./target/release/evertydesk-lite

# Allow WGPU as an extra auto candidate.
EVERTYDESK_LINUX_AUTO_WGPU=1 ./target/release/evertydesk-lite
`

Install for the current user:

`ash
./scripts/install-linux-user.sh
`

Runtime diagnostics:

`ash
./scripts/run-linux-safe.sh --check
`

## Codec profile

For Astra use ./scripts/build-linux.sh astra, which builds with:

`ash
cargo build --release --no-default-features --features desktop-gui,live-h264,live-vpx-system
`

That uses eframe/egui for the desktop UI, OpenH264, and the system libvpx-dev package instead of trying to build libvpx from source on the Astra toolchain.
