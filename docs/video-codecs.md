# Video codecs and native encoder backends

EvertyDesk Lite negotiates live-video codecs through RustDesk-compatible
`SupportedDecoding`. The rule is conservative: advertise a codec only when the
local build has a decoder path that is expected to survive real sessions.

## Current Runtime Behavior

- H.264 is available through OpenH264 when `live-h264` is enabled.
- VP9 is available through libvpx, system libvpx, or Windows Media Foundation,
  depending on build features.
- H.265 decode is available through Windows Media Foundation on Windows builds
  with `live-vp9-mf`.
- macOS hosts have a native H.264 encode path through VideoToolbox and a
  CoreGraphics main-display capture path. macOS clients still decode H.264
  through the existing software decoder path for now.
- AV1 decode probing exists, but AV1 is not advertised by default because the
  current Windows Media Foundation AV1 path can crash native MFTs on some
  streams.
- If the server sends an unsupported codec anyway, the client treats it as
  skipped video, keeps screenshot refresh alive, and asks for fallback.

Experimental AV1 decode can be enabled manually:

```powershell
$env:EVERTYDESK_ENABLE_AV1_MF="1"
cargo run
```

## Windows Media Foundation Encode

The first direct native encoder backend is `src/mf_encode.rs`.

It does not start an external encoder process. It:

- receives BGRA frames from screen capture;
- converts BGRA to NV12;
- configures a Media Foundation encoder MFT;
- emits H.264 or H.265 packets;
- normalizes length-prefixed H.264/H.265 packets to Annex-B when needed;
- falls back to OpenH264 when startup/output fails.

The host selection order is:

1. Media Foundation H.264/H.265 when available and allowed by client capability.
2. VideoToolbox H.264 on macOS hosts when available.
3. Direct NVENC when that backend is implemented and available.
4. OpenH264 software H.264.
5. Screenshot fallback when live video is unavailable.

H.265 is selected only when both sides support it. H.264 remains the safest
default path.

Host sessions now emit periodic encode telemetry to logs:

- planned and active encoder backend;
- active codec;
- frame size, target FPS, and target bitrate;
- sent packets, bytes, average packet size, and keyframes;
- capture/change-detection/encode/send timings;
- empty encoder outputs;
- hardware/native fallback reason.

The latest host video telemetry is also surfaced in the Host UI and in
headless host output. Full per-interval details remain copyable from the host
diagnostics log.

## Windows Capture

The Windows host path now tries Desktop Duplication first and falls back to the
cached GDI capture path if DXGI/D3D11 is unavailable, reset, or incompatible
with the current desktop.

Current Desktop Duplication behavior:

- creates a D3D11 device and `IDXGIOutputDuplication`;
- acquires the primary output frame;
- copies it through a staging texture into the existing BGRA frame buffer;
- keeps existing encoder fallback behavior unchanged.

This is a low-copy capture prototype, not the final GPU pipeline. The next
Windows performance step is to validate it on a real Windows/MSVC host, then
pass D3D11 textures into async Media Foundation or NVENC without full CPU
readback.

## macOS Capture

The macOS host path now has a first native capture backend:

- captures the main display through CoreGraphics;
- writes BGRA directly into the existing reusable host frame buffer;
- feeds the same encoder selection/fallback pipeline as other platforms.

This is a compatibility capture path. The next performance step for macOS is
ScreenCaptureKit so high-motion sessions can use lower-copy capture and better
frame pacing.

## NVENC

The app currently detects NVIDIA support in two layers:

- `NvEncodeAPI` runtime probe through a Rust FFI wrapper.
- NVIDIA GPU presence through runtime diagnostics.

The direct NVENC encoder backend is still planned. The intended path is:

- load `nvEncodeAPI64.dll` dynamically on Windows;
- avoid hard-linking portable builds to the SDK;
- start with CPU/NV12 H.264;
- add H.265;
- add AV1 only after AV1 decode is stable;
- later add D3D11 interop for lower-copy capture/encode.

The NVIDIA Video Codec SDK can be discovered by the build script for headers
and version diagnostics. Put it in the project root as `Video_Codec_SDK_*` or
set one of:

- `EVERTYDESK_NV_CODEC_SDK`
- `NV_CODEC_SDK`
- `NVIDIA_VIDEO_CODEC_SDK`

The current accelerated Windows path is Media Foundation, not direct NVENC.

## macOS VideoToolbox

VideoToolbox is the first native macOS hardware encoder backend.

Current behavior:

- uses `VTCompressionSession`;
- supports H.264 first;
- requests hardware acceleration and real-time encode mode;
- emits Annex-B H.264 packets with SPS/PPS on keyframes;
- keeps OpenH264 fallback;
- keeps application startup independent from optional hardware availability.

Remaining work:

- add VideoToolbox H.264 decode on macOS clients;
- add H.265 only after cross-platform client decode is stable;
- move capture from CoreGraphics toward ScreenCaptureKit;
- validate long sessions and failure fallback on Intel and Apple Silicon Macs.

## Linux Hardware Encode

Linux hardware encode is planned but not first priority.

Likely paths:

- VA-API for Intel/AMD;
- direct NVENC for NVIDIA;
- software H.264 fallback;
- VP9 through libvpx/system libvpx where available.

Enterprise Linux builds must continue to work without optional hardware
runtime libraries.

## Compatibility Rules

- Do not advertise AV1 by default until the decoder is stable.
- Do not select H.265 unless both host and client support it.
- Do not make hardware acceleration mandatory.
- Do not remove screenshot fallback.
- Any backend failure must downgrade without disconnecting the session.

## Next Codec Tasks

- Add Media Foundation CodecAPI controls for bitrate, GOP, low latency, and
  force-keyframe.
- Add better SPS/PPS/VPS handling for H.264/H.265.
- Add richer per-session codec telemetry history/graphs in the UI.
- Add VideoToolbox H.264 decode on macOS clients.
- Validate Windows Desktop Duplication capture on a Windows host.
- Add D3D11/async MFT support for true hardware Media Foundation paths.
- Move macOS capture toward ScreenCaptureKit.
- Add direct NVENC encoder backend.
- Add captured-packet tests for H.265 and AV1 decoder stability.
- Add platform capture paths required for interactive/game-grade streaming:
  Desktop Duplication/Windows Graphics Capture, macOS ScreenCaptureKit, Linux
  XDamage/PipeWire.
