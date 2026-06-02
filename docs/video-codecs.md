# Video codecs and NVENC

EvertyDesk Lite negotiates live-video codecs through RustDesk-compatible `SupportedDecoding`.

## Current runtime behavior

- `Auto` prefers modern codecs only when the local build has a real decoder backend.
- H264 is available through OpenH264 when the `live-h264` feature is enabled.
- VP9 is available through libvpx, system libvpx, or Windows Media Foundation depending on build features.
- H265 and AV1 are present in the protocol and settings, but are not advertised as decodable until a real decoder backend is wired in.
- If a server still sends unsupported H265/AV1/VP8/VP9, the client requests a supported fallback and keeps screenshot refresh alive.

## NVENC

The app now detects NVENC availability through `ffmpeg -encoders` and falls back to `nvidia-smi` for GPU presence. The result is shown in the video settings and status label.

Current host streaming still uses the OpenH264 software encoder. Real NVENC streaming needs a dedicated encoder backend that can produce RustDesk-compatible frame packets:

- `h264_nvenc` can replace OpenH264 for H264.
- `hevc_nvenc` can enable H265 host streaming after H265 decoding is implemented.
- `av1_nvenc` can enable AV1 host streaming after AV1 decoding is implemented.

Do not advertise H265 or AV1 support until both sides can decode the selected codec reliably.
