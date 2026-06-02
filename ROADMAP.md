# EvertyDesk Lite: technical plan

Status date: 2026-06-02.

This document tracks what is already implemented and what remains to turn
EvertyDesk Lite from a compact RustDesk-compatible client into a fast,
portable support workstation.

## Current Goal

EvertyDesk Lite should stay small, native, and usable on imperfect machines:
old corporate Linux desktops, Windows hosts without extra packages,
conservative enterprise distributions, virtual machines, and closed
environments where "install a driver/tool/runtime" is not a good answer.

The product direction is:

- Rust + egui native UI instead of Electron/WebView.
- RustDesk-compatible connection path: ID server, relay, numeric ID, password
  or remote approval.
- Live video when codecs are available.
- Safe fallback to screenshots when live video is not available.
- Terminal and operator automation as first-class support tools.
- Direct native codec backends instead of external encoder processes.

## Implemented

### Core Client

- Native Rust desktop application with egui/eframe.
- Built-in EvertyDesk server settings.
- Connection by numeric remote ID.
- RustDesk-compatible rendezvous/relay message flow.
- Login flow with password and remote approval support.
- Clear connection progress and failure diagnostics.
- Remote screen view in a separate window and inline mode.
- Multi-display handling and display switching.
- Mouse and keyboard input.
- Coordinate modes for different monitor layouts.
- Cursor data handling.
- Clean disconnect and stuck-input release.
- TCP framing with `TCP_NODELAY` for lower-latency control traffic.

### Renderer Resilience

- Normal eframe rendering through `wgpu`/`glow`.
- Linux safe launcher flow that tries multiple render paths.
- CPU software UI backend through `minifb`.
- Software egui painting path for machines where OpenGL/Vulkan are broken.
- UTF-8 locale setup for text input.
- Explicit Cyrillic-capable font discovery for fallback UI.

### Address Book and API

- RustDesk-compatible API login path.
- Personal address book loading.
- Contact pagination.
- Contact normalization.
- Better API diagnostics, including raw HTTP status/body for cases like `401`.

### Host and Approval Flow

- Host-side event model for incoming sessions.
- Remote approval request flow.
- UI modal for allow/reject.
- Separate password flow and approval flow.
- Early host/service mode direction for Windows services and Linux systemd.

### Terminal and Operator Tools

- Dedicated terminal message model.
- Terminal session open/input/output/close flow.
- Terminal UI buffer/history direction.
- LLM assistant support for terminal output analysis.
- Ollama, OpenAI-compatible endpoint, and YandexGPT provider paths.
- AI does not auto-execute commands; it suggests commands for the operator.

### Live Video and Codec Work

- OpenH264 software H.264 path.
- VP9 decode through Windows Media Foundation.
- VP8/VP9 through `libvpx` or system `libvpx` when built with those features.
- H.265 decode through Windows Media Foundation.
- AV1 decode probe exists, but AV1 is disabled by default because the current
  MF AV1 path can crash native MFTs on some streams. It is gated behind:

```powershell
$env:EVERTYDESK_ENABLE_AV1_MF="1"
```

- Direct Windows Media Foundation encoder backend added.
- H.264/H.265 Media Foundation encode detection.
- H.264/H.265 direct encode path uses BGRA -> NV12 -> MFT encoder.
- No external encoder executable is required for the new Windows MF encode path.
- Codec negotiation only selects H.265 when both sides report support.
- Fallback to OpenH264 if a hardware/native encoder fails to start or produces
  no packets.
- Packet normalization from length-prefixed H.264/H.265 to Annex-B where needed.

### Speed and Stability

- Background frame decode path.
- Texture reuse instead of recreating textures every frame.
- Frame change detector to skip static frames.
- Static-frame backoff to reduce CPU/network use.
- Target FPS negotiation.
- Screenshot fallback remains alive when live video is unsupported.
- Build variants for Windows and Linux codec availability.
- `cargo check`, `cargo test`, and `cargo check --no-default-features` pass.

## Recently Changed

- Removed the external encoder-process direction from the Windows acceleration
  path.
- Added `src/mf_encode.rs` as the first direct native encoder backend.
- Added Media Foundation H.264/H.265 encode selection before software fallback.
- Added Media Foundation encode status into codec diagnostics.
- Disabled default AV1 advertisement after a real runtime crash on AV1 decode.
- Added host-side encode telemetry logs for active backend, codec, bitrate,
  packet bytes, keyframes, empty outputs, and fallback reason.
- Surfaced the latest host video telemetry in the Host UI and headless host
  output.
- Added codec preference tests for conservative H.264/H.265/AV1/VP9 fallback
  ordering.

## Remaining Work

### 1. Stabilize Windows Media Foundation Video

Priority: high.

- Add CodecAPI controls for:
  - bitrate update;
  - GOP/keyframe interval;
  - explicit force-keyframe;
  - quality/speed mode;
  - low-latency mode where supported.
- Stop marking keyframes optimistically; rely on real IDR/IRAP detection or
  CodecAPI feedback.
- Improve SPS/PPS/VPS handling for H.264/H.265 streams.
- Add robust recovery when MFT returns stream change, bad output type, or empty
  packets.
- Add richer UI history/graphs for runtime telemetry: selected encoder, packet
  size, FPS, bitrate, dropped frames, and fallback reason.
- Keep AV1 disabled by default until the MF AV1 decoder survives long sessions.
- Build an isolated AV1/H.265 decoder test harness with captured packets.

### 2. True Hardware Paths

Priority: high.

- Implement async/D3D11 Media Foundation MFT support.
- Move from CPU BGRA -> NV12 conversion toward GPU/D3D11 texture paths.
- Investigate Desktop Duplication API capture on Windows for zero-copy or
  low-copy capture.
- Add direct NVENC backend through `NvEncodeAPI`, not through a helper process.
- Use NVIDIA Video Codec SDK only as headers/FFI source, with runtime driver
  loading.
- Add NVENC H.264/H.265 first, AV1 only when client decode is stable.
- Add capability reporting that distinguishes:
  - software encode;
  - system MFT encode;
  - hardware MFT encode;
  - NVENC encode.

### 3. Cross-Platform Hardware Encode

Priority: medium.

- Native VideoToolbox backend for macOS through `VTCompressionSession`.
- Linux VA-API path for Intel/AMD where available.
- Linux NVENC path through direct NVENC API.
- Keep all platform hardware backends optional.
- Never make application startup depend on optional codec runtime availability.

### 4. Codec Negotiation and Fallback

Priority: high.

- Make codec preference negotiation more conservative by default.
- Add explicit "codec failed, downgrade" session event.
- Remember failed codec/backend per session.
- Avoid advertising codecs that are only probed but not soak-tested.
- Add UI-visible codec state:
  - requested codec;
  - negotiated codec;
  - active decoder;
  - active encoder;
  - fallback reason.
- Extend tests for H.264/H.265/AV1/VP9 preference ordering across host encoder
  capability combinations.

### 5. Performance

Priority: high.

- Make BGRA -> NV12 conversion faster with row batching and optional SIMD.
- Add dirty-rectangle capture/encode where the host backend supports it.
- Add adaptive bitrate from frame change density and measured relay throughput.
- Add adaptive FPS floor/ceiling for static vs active screens.
- Avoid copying decoded RGBA more than necessary before egui texture upload.
- Measure CPU cost separately for capture, conversion, encode, network, decode,
  and render.

### 6. Host/Service Mode

Priority: medium.

- Windows service installer/uninstaller.
- Linux systemd unit generation and installer.
- Clear distinction between GUI client mode and host service mode.
- Windows session switching and secure desktop limitations.
- Linux X11/Wayland capture/input permissions.
- Host logs with session IDs and approval decisions.

### 7. Terminal and Automation

Priority: medium.

- Harden shell session isolation.
- Add explicit permission model for terminal access.
- Add transcript export.
- Add operator command snippets.
- Add safe script templates.
- Add file transfer with audit trail.
- Add automation run history.
- Keep AI suggestions human-approved.

### 8. API and Address Book

Priority: medium.

- Token refresh and expiry handling.
- Better distinction between auth failure, API shape mismatch, and server error.
- Contact create/update/delete parity with RustDesk-compatible API.
- Online status where the server exposes it.
- Import/export contacts.

### 9. Packaging and Diagnostics

Priority: medium.

- Windows release packaging.
- Linux AppImage/deb/rpm direction.
- Startup diagnostics command.
- Codec diagnostics command.
- Graphics diagnostics command.
- Machine-readable support bundle for bug reports.
- Document exact feature/build matrix.

## Near-Term Milestones

### Milestone A: Stable Windows H.264/H.265 Direct Encode

- MF H.264 encode works for long sessions.
- MF H.265 encode works only when both sides support H.265.
- Fallback to OpenH264 is automatic and visible.
- No native crash from advertised decoder capabilities.
- Codec status is visible in UI and logs.

### Milestone B: Low-Latency Windows Capture Path

- Desktop Duplication API capture prototype.
- Faster BGRA/NV12 conversion.
- Encoder latency and packet-size metrics.
- Adaptive FPS and bitrate tuning.

### Milestone C: Direct NVENC

- Load `nvEncodeAPI64.dll` dynamically.
- Create encoder session.
- Encode H.264 from CPU/NV12 first.
- Add D3D11 interop later.
- Keep fallback behavior identical to MF/OpenH264.

### Milestone D: Production Host Mode

- Windows service mode.
- Linux systemd mode.
- Approval and password policies.
- Logs and audit events.
- Installation docs.

## Non-Goals For Now

- Writing a full H.264/H.265/AV1 codec implementation from scratch.
- Requiring a large external media executable for the default accelerated path.
- Making AV1 default before it survives real remote desktop streams.
- Removing screenshot fallback.
- Making hardware acceleration mandatory.

## Current Risk Register

- Native Media Foundation decoders can crash the whole process if we advertise
  a codec before the backend is stable.
- Hardware encoder availability is not enough; the connected client must also
  decode the selected codec.
- H.265/AV1 packet format details can break compatibility if VPS/SPS/PPS or
  length-prefix/Annex-B handling is wrong.
- Linux enterprise environments may have old GL/Mesa/libvpx versions.
- Service mode is a separate product surface, not just a background flag.

## Engineering Rule

When in doubt, prefer a working fallback over an optimistic fast path.

The client should degrade from hardware encode to software H.264 to screenshots
without disconnecting the operator.
