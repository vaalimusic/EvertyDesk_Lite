# Everty EVRT Transport Protocol

The Android sender streams low-latency video with a fixed EVRT binary header. The same packet payload is currently carried over:

- UDP datagrams for LAN mode
- length-prefixed TCP for ADB tunnel mode

## Packet Layout

- `magic` `uint32`, big-endian: `0x45565254` (`EVRT`)
- `version` `uint8`: `2`
- `type` `uint8`
- `flags` `uint16`
- `frameId` `uint32`
- `packetIndex` `uint16`
- `packetCount` `uint16`
- `presentationTimeUs` `uint64`
- `payload` `bytes`

Header size: `24` bytes.
Maximum UDP datagram size: `1200` bytes.
Maximum UDP payload size: `1176` bytes.

## Transport Framing

### UDP

- one EVRT packet maps to one UDP datagram

### ADB Tunnel / TCP

- one EVRT packet is prefixed by a `uint32` big-endian packet length
- receiver and sender exchange control packets over the same TCP connection
- intended for `adb reverse tcp:<port> tcp:<port>` over USB

## Packet Types

- `1` `TYPE_SESSION_CONFIG`
  Payload: UTF-8 JSON with session parameters.
  Example:
  `{"codec":"video/avc","preset":"INSTANT_PLAY","width":640,"height":360,"fps":60,"bitrate":2200000}`

- `2` `TYPE_CODEC_CONFIG`
  Payload: codec configuration block.
  For AVC this is Annex-B SPS/PPS.
  For HEVC this is the VPS/SPS/PPS block emitted by Android `MediaCodec`.

- `3` `TYPE_VIDEO_FRAME`
  Payload: a fragment of one encoded access unit.

- `4` `TYPE_CONTROL`
  Payload: UTF-8 JSON control message sent from receiver back to sender.
  Examples:
  `{"kind":"request_keyframe"}`
  `{"kind":"receiver_feedback","pressure":"high","backlogFrames":2,"queueDrops":7,"decodeFps":31}`

- `5` `TYPE_AUDIO_CONFIG`
  Payload: UTF-8 JSON with PCM audio stream parameters.
  Example:
  `{"codec":"pcm_s16le","sampleRate":48000,"channels":2,"bytesPerSample":2}`

- `6` `TYPE_AUDIO_FRAME`
  Payload: one fragment of a PCM audio chunk.

## Flags

- `0x0001` `FLAG_KEYFRAME`
  Set on every packet that belongs to a keyframe access unit.

## Reassembly Rules

- Packets for the same encoded frame are grouped by `frameId`.
- A frame is ready only after all `packetCount` fragments arrive.
- Protocol `v2` is latest-frame oriented:
  - if packets for a newer frame arrive before an older incomplete frame is finished, the older incomplete frame is abandoned immediately
  - late packets for already completed or superseded frames are discarded
  - after an incomplete inter-frame loss, receiver may ignore following inter-frames until the next keyframe to recover quickly
- When `TYPE_CODEC_CONFIG` arrives, the receiver must replace the active codec config.
- The sender repeats codec config before keyframes so the receiver can resync without restarting the whole session.
- Audio frames are also fragmented by `frameId` and must be reassembled before playback.
- For low latency, receiver audio playback may drop old queued chunks instead of letting latency accumulate.

## Control Channel

- In UDP mode, receiver sends control packets back to the sender using the source address and UDP port of the incoming stream.
- In ADB tunnel mode, receiver sends control packets back over the same TCP connection.
- `request_keyframe` asks Android `MediaCodec` to emit a fresh sync frame as soon as possible.
- `receiver_feedback` reports decode pressure to the sender.
- Sender may react by lowering bitrate, requesting a fresh keyframe, gradually recovering bitrate after the receiver becomes stable again, and dropping oversized non-key inter-frames when they threaten realtime latency.

## Codec Scope

- Current sender codecs:
  - `video/avc` (`H.264 / AVC`) as the default low-latency path
  - `video/hevc` (`H.265 / HEVC`) as an experimental higher-efficiency path
- Current audio transport:
  - `pcm_s16le` captured from Android playback on Android 10+ and streamed directly over LAN for low decode latency
- Receiver decode path is codec-aware and may try hardware acceleration before software fallback.
- Receiver keeps a feedback loop so decode backlog can influence sender behavior in real time.
- Session config may include:
  - `transport:"EVRT_REALTIME_V2_UDP"`
  - `transport:"EVRT_REALTIME_V2_TCP_ADB"`
  to signal the active carrier to the receiver.

## Current Scope

- Designed for local-network realtime preview and play testing.
- No guaranteed delivery.
- No ACK/NACK, receiver-to-sender keyframe request, or congestion control yet.
