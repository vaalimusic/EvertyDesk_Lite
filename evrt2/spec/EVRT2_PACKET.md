# EVRT2 — Wire Packet Format

**Version:** 2026-draft-1

---

## Overview

EVRT2 uses a fixed 32-byte header (vs EVRT1's 24 bytes).
The extra 8 bytes add: mode field, FEC metadata, and an extended flags field.
Maximum UDP datagram: **1400 bytes** (MTU-safe with IPv6 + GRE headroom).
Maximum payload: **1400 − 32 = 1368 bytes**.

---

## Packet Header (32 bytes)

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                     Magic  0x45565232 ("EVR2")                | 4
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|  Ver=2  | Type  |    Mode   |          Flags (16-bit)         | 8
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                          FrameId (32-bit)                     | 12
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|        PacketIndex (16-bit)   |      PacketCount (16-bit)     | 16
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                  PresentationTimeUs (64-bit)                  | 20
|                                                               | 24
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|   FEC Group (8-bit) | FEC Idx | FEC Total  |   Reserved      | 28
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                     AuthTag (32-bit truncated)                | 32
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                          Payload …                            |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

### Field descriptions

| Field | Bits | Description |
|-------|------|-------------|
| Magic | 32 | `0x45565232` — "EVR2". Distinguishes from EVRT1 |
| Ver | 4 | Protocol version = 2 |
| Type | 4 | Packet type (see below) |
| Mode | 8 | Active codec mode: `0x01`=AR, `0x02`=2R, `0x47`=47 |
| Flags | 16 | Bitmask (see Flags table) |
| FrameId | 32 | Monotonic frame counter, wraps at 2³² |
| PacketIndex | 16 | This packet's index within the frame (0-based) |
| PacketCount | 16 | Total packets in this frame |
| PresentationTimeUs | 64 | Capture timestamp, microseconds since Unix epoch |
| FEC Group | 8 | FEC repair group identifier |
| FEC Idx | 8 | This packet's index within FEC group |
| FEC Total | 8 | Total packets in FEC group (data + repair) |
| Reserved | 8 | Must be 0 |
| AuthTag | 32 | Truncated HMAC-SHA256 of header+payload |

---

## Packet Types

| Value | Name | Direction | Description |
|-------|------|-----------|-------------|
| `0x01` | SESSION_HELLO | C→H, H→C | Handshake initiation |
| `0x02` | SESSION_ACK | H→C, C→H | Handshake acknowledgment |
| `0x03` | CODEC_CONFIG | H→C | Codec parameters + silicon info |
| `0x04` | MODE_SWITCH | H→C | Runtime mode change (AR↔2R↔47) |
| `0x05` | VIDEO_FRAME | H→C | Encoded video data |
| `0x06` | AUDIO_FRAME | H→C | Encoded audio data |
| `0x07` | FEEDBACK | C→H | ReceiverFeedback2 |
| `0x08` | KEEPALIVE | both | Heartbeat (empty payload) |
| `0x09` | FEC_REPAIR | H→C | FEC repair packet |
| `0x0A` | IDR_REQUEST | C→H | Client requests keyframe |
| `0x0B` | GOODBYE | both | Clean session termination |
| `0x0C` | RELAY_WRAP | both | EVRT2 packet tunneled over TCP relay |

---

## Flags (16-bit bitmask)

| Bit | Name | Description |
|-----|------|-------------|
| 0 | IS_KEYFRAME | This packet contains or starts a keyframe |
| 1 | IS_SILICON | Frame was encoded by hardware silicon |
| 2 | HAS_AUDIO | Audio data piggybacked after video payload |
| 3 | ENCRYPTED | Payload is encrypted (AES-GCM-256) |
| 4 | COMPRESSED | Payload has an additional zstd wrapper |
| 5 | ROI_HINT | Payload includes ROI bitmask before frame data |
| 6 | FEC_ENABLED | FEC repair packets follow for this frame |
| 7 | RELAY_MODE | Packet is traveling through TCP relay tunnel |
| 8-15 | Reserved | Must be 0 |

---

## Mode byte encoding

The Mode field encodes the active AR2R47 mode:

```
0x01  =  AR  (static desktop, support mode)
0x02  =  2R  (dynamic content, video/animation)
0x47  =  47  (gaming, no compromises)   ← 0x47 = 71 decimal
```

Note: `0x47` was chosen deliberately — it is the ASCII code of 'G' (for Gaming)
and forms part of the AR2R**47** tribute to Arthur Valiev.

---

## Maximum frame size

```
Max packets per frame:  65535  (PacketCount is 16-bit)
Max payload per packet: 1368 bytes
Max frame payload:      65535 × 1368 = ~87 MB

Practical limits per mode:
  AR mode:  typically 5–50 KB/frame  (sparse delta, lossless)
  2R mode:  typically 20–200 KB/frame (adaptive lossy)
  47 mode:  up to 500 KB/frame @ 4K120 (silicon, hardware bitrate)
```

---

## FEC (Forward Error Correction)

EVRT2 adds native FEC — missing in EVRT1. Each FEC group contains:
- N data packets (the actual frame slices)
- K repair packets (XOR combinations)

Any N packets from a group of (N+K) recovers the full frame.
Default: N=8, K=2 (20% redundancy). Configurable per session.

FEC is **disabled** in 47 mode by default (latency > recovery value at 120fps).
FEC is **enabled** in AR and 2R modes over WAN.
