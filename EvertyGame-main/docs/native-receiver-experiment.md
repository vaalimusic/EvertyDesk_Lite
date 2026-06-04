# Everty Native Receiver Experiment

This is a new Windows-native receiver experiment based on `.NET WinForms + LibVLC`.

## Why it exists

The existing `:receiver` module uses a JVM decode/render path (`JavaCV + Swing`), which is convenient for protocol work but becomes the latency bottleneck for game-like use.

`receiver-native` keeps the same EVRT wire protocol, frame reassembly, and receiver feedback model, but replaces the hot decode/render path with LibVLC's native playback stack.

## Current scope

- UDP listener for the existing `EVRT` transport
- TCP listener for `ADB tunnel` mode (`adb reverse`)
- session config parsing
- codec config tracking
- latest-frame-oriented reassembly
- aggressive tail-drop input queue
- keyframe request / receiver feedback back to the Android sender
- native video playback through LibVLC
- hardware decode preference:
  - `Auto`
  - `D3D11VA`
  - `DXVA2`
  - `Disabled`

## Run

From the repo root:

```bat
run-native-receiver.cmd
```

Or directly:

```bat
dotnet run --project receiver-native/ReceiverNative.csproj
```

## ADB Tunnel

1. Start `receiver-native`.
2. Change `Transport` to `ADB tunnel / TCP`.
3. Click `Start`.
4. On the PC run:

```bat
adb reverse tcp:5001 tcp:5001
```

5. On the phone choose `ADB tunnel / TCP`.
6. Use host `127.0.0.1` and port `5001`.

The stream then uses the same EVRT packets over a length-prefixed TCP tunnel instead of UDP.

## Current limitations

- This is an experiment, not the final renderer.
- Audio is intentionally ignored for now.
- Telemetry is receiver-side and uses an input-FPS proxy, not a true rendered-FPS counter from the native video stack.
- Decoder selection is a requested LibVLC hardware mode, not a guaranteed final active path report from VLC internals.

## Next likely steps

1. Compare `receiver-native` against the old JVM `:receiver` on the same sender preset.
2. If latency is materially better, keep pushing this branch:
   - fullscreen-focused UI
   - stronger present-latest-only policy
   - optional HEVC tuning
   - measured end-to-end delay reporting
3. If LibVLC still is not enough, move to a lower-level Windows path (`Media Foundation / D3D11`).
