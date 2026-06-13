# EvertyDesk Lite: technical plan

Status date: 2026-06-06.

This document tracks what is already implemented and what remains to turn
EvertyDesk Lite from a compact RustDesk-compatible client into a fast,
portable support workstation.

> **Связанные документы:**
> - [`EVRT_ROADMAP.md`](EVRT_ROADMAP.md) — игровой стриминговый стек EVRT
>   (прямой UDP, низкая задержка, адаптация). Разработка Артура Валиева.
> - [`WINDOWS_DEV_GUIDE.md`](WINDOWS_DEV_GUIDE.md) — сборка и тестирование EVRT
>   под Windows, чек-листы валидации на железе.

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
- A separate "interactive/game-grade" path for high-motion sessions: 60 FPS
  target, low input latency, hardware capture/encode/decode where possible,
  and graceful fallback when a machine cannot sustain it.

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
- Shared compact remote-session toolbar for inline and detached viewers:
  monitor selection, clipboard paste, fullscreen, fit-to-window, refresh,
  PNG save, video profile switching, coordinate mode, Ctrl+Alt+Del, lock,
  logs, and support report.
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
- Direct macOS VideoToolbox H.264 encoder backend added.
- macOS VideoToolbox encode path emits Annex-B H.264 packets and keeps
  OpenH264 fallback when startup/output fails.
- Direct macOS VideoToolbox H.264 client decoder backend added.
- macOS H.264 decode now tries VideoToolbox first and falls back to OpenH264
  when available.
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
- First Windows Desktop Duplication capture prototype with GDI fallback.
- First macOS CoreGraphics main-display capture path.
- Screenshot fallback remains alive when live video is unsupported.
- Build variants for Windows and Linux codec availability.
- `cargo check`, `cargo test`, and desktop Linux checks with explicit `desktop-gui` pass.

### Android Outgoing Client

- First Android module added at `EvertyGame-main/android-client`.
- The module is intentionally separate from the existing phone screen sender
  POC and starts as an outgoing remote-control client.
- Current Android scope: compact Compose shell, remote ID/password form,
  EvertyDesk server settings, FPS/codec preference fields, quick-action layout,
  and connection-controller scaffold.
- The Android app currently does not perform the RustDesk handshake yet; the
  next step is moving the shared framing/protobuf/crypto transport into a
  reusable mobile-friendly layer.

## Recently Changed

- Reworked the desktop remote-session toolbar so inline and detached viewers
  share the same monitor, clipboard, fullscreen, refresh, quality, coordinate,
  system-command, log, and report controls.
- Added active-session video profile switching from the remote toolbar through
  `SessionCommand::SetVideoProfile`, with persisted FPS/codec preferences.
- Added the first Android outgoing-client module and documentation.
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
- Added host video stage timing telemetry for capture, change detection,
  encode, and send phases.
- Reduced frame-change detector memory copying by storing a sampled frame
  signature instead of cloning the whole previous BGRA frame.
- Separated client-side incoming live-video packet rate from actually rendered
  FPS, and stopped treating decoder buffering as a fatal decode failure.
- Added a reusable host capture buffer and cached Windows GDI capture objects
  to reduce per-frame allocation and DC/bitmap churn on the current Windows
  host path.
- Added a Windows DXGI/D3D11 Desktop Duplication capture path before GDI
  fallback. It currently copies the duplicated desktop texture into the
  existing BGRA buffer path so the old encoder/fallback behavior stays intact.
- Added a macOS CoreGraphics capture path for the main display.
- Added a macOS VideoToolbox H.264 host encoder backend with SPS/PPS keyframe
  packaging and OpenH264 fallback.
- Added a macOS VideoToolbox H.264 client decoder backend for RustDesk-style
  Annex-B packets.
- Added RustDesk-compatible video QoS negotiation: client login/sync now sends
  `ImageQuality::Best` with target FPS, and EvertyDesk hosts use received
  quality options to raise H.264 bitrate headroom for high-motion sessions.
- Tuned client adaptive streaming so low-latency frame drops do not immediately
  collapse an interactive 60 FPS target below 30 FPS.
- Changed Auto codec negotiation to prefer H.264 for interactive stability,
  while keeping H.265/AV1 available when explicitly requested or when H.264 is
  unavailable.
- Added best-effort Media Foundation CodecAPI tuning for low-latency mode,
  low-delay VBR, bitrate, GOP size, quality/speed, and force-keyframe.
- Added codec preference tests for conservative H.264/H.265/AV1/VP9 fallback
  ordering.
- Added an EVRT audio jitter buffer on the client playback path:
  40 ms prebuffer, bounded 180 ms queue, partial WASAPI writes preserved
  instead of dropping the unwritten tail.
- Added tile-based ROI dirty-region detection without keeping a previous full
  frame copy; EVRT now sends dirty ROI metadata before video frames.
- Added ROI-driven bitrate adaptation in the unified encoder pipeline:
  small dirty regions lower the target bitrate, while fullscreen and IDR frames
  stay at full quality.
- Connected EVRT `ReceiverFeedback` / `AdaptiveRelief` to the live encoder
  bitrate path through a shared integer scale, so client pressure now lowers
  target bitrate instead of only producing logs.
- Added host-stream recovery pressure rules: TCP keyframes get a short delivery
  grace period, dropped video frames request the next keyframe, and congested
  queues reduce bitrate pressure instead of silently corrupting inter-frame
  prediction.
- Tuned Linux/software H.264 quality policy for desktop text: 1080p captures
  stay native, larger captures scale only to 1080p-class frames, and software
  streams get a bitrate floor for readable fonts.
- Completed the text clipboard path for desktop sessions: local paste sends a
  RustDesk-compatible `Clipboard` message, incoming text clipboard writes into
  the local system clipboard, and both client and host honor
  `security.allow_clipboard`.
- Cleaned the main desktop connect card: removed the separate remote-ID check
  action, removed the extra hero logo plate, tightened spacing, and added
  project UI/stream stability rules under `docs/`.
- Added Linux X11/RandR per-monitor capture: `display_infos()` now reports
  active monitor rectangles and `capture_display_into(display)` captures the
  selected monitor instead of the whole root desktop.

## Remaining Work

### 1. Stabilize Windows Media Foundation Video

Priority: high.

- Add runtime Media Foundation CodecAPI bitrate update without recreating the
  encoder.
- Stop marking keyframes optimistically; rely on real IDR/IRAP detection or
  CodecAPI feedback.
- Improve SPS/PPS/VPS handling for H.264/H.265 streams.
- Add robust recovery when MFT returns stream change, bad output type, or empty
  packets.
- Add richer UI history/graphs for runtime telemetry: selected encoder, packet
  size, FPS, bitrate, dropped frames, and fallback reason.
- Keep AV1 disabled by default until the MF AV1 decoder survives long sessions.
- Build an isolated AV1/H.265 decoder test harness with captured packets.
- Add decoder-side "waiting for keyframe" UI state and request-IDR feedback
  when the client detects a broken or missing reference chain.

### 2. True Hardware Paths

Priority: high.

- Implement async/D3D11 Media Foundation MFT support.
- Move from CPU BGRA -> NV12 conversion toward GPU/D3D11 texture paths.
- Validate the Desktop Duplication prototype on a Windows/MSVC host and keep
  GDI as fallback for unsupported desktops, secure screens, and API resets.
- Move Desktop Duplication from CPU readback toward zero-copy or low-copy
  D3D11 texture handoff into Media Foundation/NVENC.
- Direct Windows NVENC backend through `NvEncodeAPI` and a Windows-only SDK
  shim is added; keep improving it without adding a hard runtime dependency.
- NVENC H.264/H.265 are wired first; AV1 remains disabled until client decode
  is stable.
- Add capability reporting that distinguishes:
  - software encode;
  - system MFT encode;
  - hardware MFT encode;
  - NVENC encode.

### 3. Cross-Platform Hardware Encode

Priority: medium.

- Extend the native macOS VideoToolbox path:
  - add H.265 only after decoder stability is proven;
  - replace CoreGraphics capture with ScreenCaptureKit for high-motion work.
- Linux VA-API path for Intel/AMD where available.
- Linux NVENC path through direct NVENC API.
- Keep all platform hardware backends optional.
- Never make application startup depend on optional codec runtime availability.

### 4. Codec Negotiation and Fallback

Priority: high.

- Make codec preference negotiation more conservative by default.
- Add explicit "codec failed, downgrade" session event.
- Add explicit "stream corrupt, request keyframe" and "queue pressure" session
  events.
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

- Treat 2-3 FPS during window dragging as a pipeline bug, not an acceptable
  fallback. Use telemetry first to identify whether the current bottleneck is
  capture, BGRA/NV12 conversion, encode, relay send, client decode, or render.
- Diagnose low FPS by comparing client `fps` and `in`: low `in` means
  host/capture/encode/relay bottleneck; high `in` with low `fps` means
  client decode/render bottleneck.
- Make BGRA -> NV12 conversion faster with row batching and optional SIMD.
- Reuse capture buffers and platform capture objects; avoid per-frame
  allocation of device contexts, bitmaps, and full-frame scratch buffers.
- Promote the Windows Desktop Duplication path from prototype to production and
  use Windows Graphics Capture where Desktop Duplication is not enough.
- Add dirty-rectangle capture/encode where the host backend supports it.
- Extend adaptive bitrate with measured relay throughput; frame-change density
  and EVRT receiver pressure already drive target bitrate.
- Add adaptive FPS floor/ceiling for static vs active screens.
- Avoid copying decoded RGBA more than necessary before egui texture upload.
- Measure CPU cost separately for capture, conversion, encode, network, decode,
  and render.

### 6. Interactive/Game-Grade Streaming

Priority: high.

Target profile:

- 1080p60 for ordinary desktop motion on modern machines.
- 1080p60 game/video motion when hardware capture and hardware encode are
  available.
- End-to-end input-to-photon latency target below 80 ms on a local network.
- No UI stutter when moving windows, scrolling, or playing video.

Required architecture:

- Windows: Desktop Duplication API or Windows Graphics Capture for low-copy
  capture; D3D11 texture path into async Media Foundation/NVENC.
- macOS: ScreenCaptureKit capture and `VTCompressionSession` encode.
- Linux X11: XDamage/region tracking instead of full-screen polling.
- Linux Wayland: PipeWire portal capture with damage/region metadata where
  available.
- Encode only changed regions when the codec/backend can accept it; otherwise
  use dirty-region-driven bitrate/FPS decisions.
- Move toward GPU texture upload/decode paths on the client, not repeated
  full RGBA copies.
- Add transport mode for high-motion sessions: lower buffering, explicit frame
  dropping, frame pacing, and latency telemetry. Relay TCP remains the safe
  compatibility path; low-latency UDP/QUIC/WebRTC-like transport is a separate
  optional path.
- Separate control/input priority from video traffic so mouse/keyboard remain
  responsive during large frames.
- Add a "Performance mode" profile in settings:
  - Support mode: conservative, stable, lower CPU.
  - Interactive mode: higher FPS, lower buffering.
  - Game mode: lowest latency, hardware-only preference, aggressive frame drop.

### 7. Android Outgoing Client

Priority: high.

- Extract or mirror the RustDesk-compatible message framing, protobuf models,
  key exchange, login, and relay stream flow for Android.
- Keep the first Android release outgoing-only: connect to desktop hosts,
  render video, send input, switch displays, paste clipboard, and disconnect.
- Add MediaCodec H.264 decode first, then H.265/AV1 only after desktop decoder
  stability is proven.
- Add a real viewer surface with low-latency frame replacement, no growing
  queue, and touch/mouse/keyboard mapping.
- Add monitor switching, clipboard send, fullscreen/orientation handling, and
  session telemetry to match the desktop toolbar.
- Decide whether the shared protocol core becomes Kotlin code, a Rust library
  via JNI, or a small mixed layer.

### 8. Host/Service Mode

Priority: medium.

- Windows service installer/uninstaller.
- Linux systemd unit generation and installer.
- Clear distinction between GUI client mode and host service mode.
- Windows session switching and secure desktop limitations.
- Linux X11/Wayland capture/input permissions.
- Host logs with session IDs and approval decisions.

### 9. Terminal and Automation

Priority: medium.

- Harden shell session isolation.
- Add explicit permission model for terminal access.
- Add transcript export.
- Add operator command snippets.
- Add safe script templates.
- Add file transfer with audit trail.
- Add automation run history.
- Keep AI suggestions human-approved.

### 9. API and Address Book

Priority: medium.

- Token refresh and expiry handling.
- Better distinction between auth failure, API shape mismatch, and server error.
- Contact create/update/delete parity with RustDesk-compatible API.
- Online status where the server exposes it.
- Import/export contacts.

### 10. Packaging and Diagnostics

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

- Desktop Duplication API capture prototype added.
- Windows host build/runtime validation for the DXGI capture path.
- Faster BGRA/NV12 conversion.
- Encoder latency and packet-size metrics.
- Adaptive FPS and bitrate tuning.

### Milestone C: macOS Native Video Path

- CoreGraphics capture works as the compatibility baseline.
- VideoToolbox H.264 encode survives long host sessions.
- macOS client uses VideoToolbox H.264 decode before OpenH264 fallback.
- ScreenCaptureKit replaces the high-motion capture path.
- Telemetry shows capture, encode, decode, render, and packet timing.

### Milestone D: Direct NVENC

- Windows shim loads `nvEncodeAPI64.dll`/`nvEncodeAPI.dll` dynamically.
- Windows shim creates a D3D11-backed NVENC session.
- Encode H.264/H.265 from host BGRA frames through registered D3D11 textures.
- Force IDR/SPS/PPS on keyframe requests.
- Add direct Desktop Duplication/Windows Graphics Capture texture handoff.
- Add Linux NVENC session/input-resource backend.
- Keep fallback behavior identical to MF/OpenH264.

### Milestone E: Production Host Mode

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
