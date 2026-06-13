# EvertyDesk Lite Stream Stability Rules

These rules are for host video paths: TCP relay, EVRT UDP, software encode, and
hardware encode. The goal is predictable recovery, not just higher FPS.

## Why Video Corrupts

Inter-frame codecs send keyframes and predicted frames. If a predicted frame or
its reference is dropped, the decoder can keep applying broken prediction until
the next clean keyframe arrives. For a remote desktop this looks like blocks,
smearing, wrong colors, or a "falling apart" image.

## Recovery Rules

- A dropped encoded frame marks the stream as needing a recovery keyframe.
- A blocked video queue must lower bitrate pressure instead of only dropping
  more frames.
- IDR/keyframes get a short delivery grace period on TCP, because losing them
  is worse than losing one predicted frame.
- The host should send periodic IDR frames even without explicit client
  feedback.
- After reconnect, display change, codec switch, or queue overflow, the next
  encoded frame should be a keyframe.

## Latency Rules

- Control, mouse, keyboard, shell, and shutdown messages must not wait behind a
  long video backlog.
- Non-key video frames are droppable when the sender is congested.
- Queues should prefer the newest complete access unit.
- If pressure remains high, reduce bitrate first, then FPS, then resolution.

## Desktop Text Quality Rules

- For support sessions, readable text has priority over smooth motion.
- Software H.264 must keep 1920x1080 and smaller captures at native resolution.
- If software encode is too expensive, lower FPS and bitrate pressure before
  downscaling the frame.
- Captures larger than 1080p may downscale to a 1080p-class frame, but should
  not fall directly to 720p unless a dedicated low-bandwidth mode is selected.
- Full-screen/keyframe and meaningful dirty-region frames need a bitrate floor
  so terminal text, browser UI, and desktop fonts remain legible.

## Hardware Path Rules

- Use direct OS/GPU APIs before external tools.
- Windows: Media Foundation or NVENC, then OpenH264 fallback.
- macOS: VideoToolbox, then OpenH264 fallback.
- Linux: keep system-codec builds optional and keep screenshot/software
  fallback alive.
- Zero-copy paths must have explicit synchronization: shared handle ownership,
  keyed mutex or equivalent, and fallback to BGRA copy if sync fails.

## UI Rules For Stream State

- Show active codec/backend and fallback reason in diagnostics.
- Do not hide a broken decoder behind "connected".
- If the stream waits for keyframe, show recovery status instead of letting the
  operator guess.
- Experimental paths must be opt-in until tested on real hardware.
