# Video codecs and hardware encoders

EvertyDesk Lite negotiates live-video codecs through RustDesk-compatible `SupportedDecoding`.

## Current runtime behavior

- `Auto` prefers modern codecs only when the local build has a real decoder backend.
- H264 is available through OpenH264 when the `live-h264` feature is enabled.
- VP9 is available through libvpx, system libvpx, or Windows Media Foundation depending on build features.
- H265 and AV1 are present in the protocol and settings, but are not advertised as decodable until a real decoder backend is wired in.
- If a server still sends unsupported H265/AV1/VP8/VP9, the client requests a supported fallback and keeps screenshot refresh alive.

## NVENC

The app detects NVENC on two levels:

- `NvEncodeAPI` runtime probe through a Rust FFI wrapper around `NvEncodeAPICreateInstance`.
- Packet encoder availability through `ffmpeg -encoders` (`h264_nvenc`, `hevc_nvenc`, `av1_nvenc`).

The result is shown in the video settings and status label. If the FFI probe works but the ffmpeg adapter is missing, the app reports that explicitly and keeps using software H264.

On macOS, NVENC is intentionally disabled and the app uses software H264/other compiled software paths. NVIDIA does not provide the NVENC runtime used here for current macOS builds, so Mac launch remains independent of the SDK folder.

The repository can also discover NVIDIA Video Codec SDK automatically. Put the SDK in the project root as `Video_Codec_SDK_*` or set one of:

- `EVERTYDESK_NV_CODEC_SDK`
- `NV_CODEC_SDK`
- `NVIDIA_VIDEO_CODEC_SDK`

The build script checks for `Interface/nvEncodeAPI.h` and exposes the detected SDK path/version to the Rust code. Feature `live-nvenc-sdk` enables the `NvEncodeAPI` FFI probe, but does not hard-link the final binary to NVIDIA SDK stubs. The driver library is loaded dynamically at runtime, so portable builds still start on machines without NVIDIA.

## Host encoder selection

Host streaming now has a backend selector:

- `Software` always uses OpenH264 when `live-h264` is enabled.
- `Auto` / `NVENC` try the ffmpeg-NVENC adapter first when the requested codec is supported by both the host runtime and the connected client.
- If NVENC startup, packet reading, or frame writing fails, the session falls back to OpenH264 without disconnecting.

The ffmpeg-NVENC adapter accepts BGRA capture frames and emits RustDesk-compatible packet payloads:

- H264 and H265 are emitted as Annex-B access units split by Access Unit Delimiters.
- AV1 is emitted through IVF output; the adapter strips IVF container headers and sends the raw AV1 frame payload.

Do not advertise H265 or AV1 support until both sides can decode the selected codec reliably.

## macOS VideoToolbox

Apple Silicon and modern Intel Macs expose hardware video encode through
VideoToolbox. EvertyDesk Lite treats it as a separate hardware backend from
NVENC:

- macOS `Auto` encoder selection tries VideoToolbox before software H264.
- The explicit hardware encoder option is labeled `VideoToolbox` on macOS.
- Runtime detection uses `ffmpeg -encoders` and looks for
  `h264_videotoolbox` / `hevc_videotoolbox`.
- If ffmpeg is not installed, VideoToolbox is reported as unavailable and the
  session falls back to OpenH264 without disconnecting.

The current VideoToolbox adapter accepts BGRA capture frames and emits
RustDesk-compatible H264/H265 Annex-B access units. H264 is the preferred Mac
hardware path until a native H265 decoder is wired into the client side.

Long term, a native `VTCompressionSession` backend can remove the ffmpeg
runtime dependency while still keeping the app launchable as one portable
binary on machines without optional hardware acceleration.
