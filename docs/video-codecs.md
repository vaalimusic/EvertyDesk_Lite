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
2. Platform-specific direct hardware backend placeholders.
3. OpenH264 software H.264.
4. Screenshot fallback when live video is unavailable.

H.265 is selected only when both sides support it. H.264 remains the safest
default path.

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

VideoToolbox is the planned native macOS hardware encoder backend.

Target behavior:

- use `VTCompressionSession`;
- support H.264 first;
- add H.265 when client decode is stable;
- keep OpenH264 fallback;
- keep application startup independent from optional hardware availability.

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
- Add per-session codec telemetry.
- Add D3D11/async MFT support for true hardware Media Foundation paths.
- Add direct NVENC encoder backend.
- Add captured-packet tests for H.265 and AV1 decoder stability.
