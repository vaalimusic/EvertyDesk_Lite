# Android outgoing client

EvertyDesk Android starts as an outgoing client: the phone connects to a
remote desktop host. It is not the same product shape as the existing
`EvertyGame-main/app` sender POC, which captures and streams the phone screen.

## Current module

- Path: `EvertyGame-main/android-client`.
- Package: `com.everty.evertydesk.client`.
- Permissions: `INTERNET` and `ACCESS_NETWORK_STATE` only.
- UI: compact Jetpack Compose shell with remote ID, password/approval, server,
  relay, public key, FPS, codec, quick actions, status, and logs.
- Controller: `AndroidConnectionController` is a scaffold. It validates input
  and records planned connection stages, but does not yet open the RustDesk
  relay stream.

## Next implementation stages

1. Move the RustDesk-compatible transport core to Android:
   framing, protobuf messages, public-key validation, login request, relay
   reservation, and secure stream handling.
2. Add live video receive:
   H.264 first through `MediaCodec`, with a latest-frame-only queue.
3. Add remote input:
   touch-to-mouse mapping, keyboard text, control keys, wheel/pinch handling,
   and stuck-input release.
4. Add RustDesk-style session controls:
   monitor switch, clipboard paste, fullscreen/orientation policy, refresh,
   disconnect, Ctrl+Alt+Del, and lock screen.
5. Add telemetry:
   incoming FPS, render FPS, codec, queue age, decode time, packet size,
   relay/direct status, and visible health text.
6. Decide the protocol-core ownership:
   Kotlin implementation for Android speed of iteration, Rust core via JNI for
   desktop/mobile sharing, or a hybrid where protobuf/framing is shared first.

## Guardrails

- Keep Android outgoing-only until the viewer is stable.
- Do not request screen-capture or Accessibility permissions for the outgoing
  client.
- Prefer H.264 first for compatibility. Add H.265/AV1 only behind capability
  checks and fallback.
- Keep the desktop Rust client as the protocol reference while Android catches
  up.
