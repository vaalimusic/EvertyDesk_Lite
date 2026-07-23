# Receiver POC

The desktop receiver lives in the Gradle module `:receiver`.

## What it does

- listens for UDP datagrams on a chosen port
- parses the Everty transport header
- reassembles fragmented H.264 access units by `frameId`
- prepends the latest codec config before keyframes
- feeds an Annex-B H.264 byte stream into FFmpeg through JavaCV
- renders the latest decoded frame in a Swing window

## Run from Gradle

```powershell
./gradlew.bat :receiver:run
```

Default UDP port in the UI is `5001`, which matches the Android sender default.

## Current limitations

- no keyframe request path yet
- no reconnect orchestration besides restarting the listener
- no hardware decode selection UI or fallback reporting
- no persistence for port or session settings
- FFmpeg is used through `javacv-platform`, so the first dependency resolution is heavy

## Expected flow

1. Start `:receiver:run`
2. Keep UDP port `5001`
3. Start the Android sender
4. Enter the receiver machine IP on Android
5. Approve screen capture
6. Receiver should switch from `Listening` to active decode once the first keyframe arrives
