# EVRT2 — Architecture Overview

**Author:** Arthur Valiev  
**Version:** 2026-draft-1  
**Status:** Specification

---

## Design Goals

1. **Universally adaptive** — single protocol handles 10 KB/s throttled mobile
   connections and 10 Gbit/s LAN with 4K@120fps, without configuration.

2. **Silicon-first** — on every modern device there is hardware that can encode
   or decode video faster and better than any software codec. EVRT2 probes for
   it at session start and delegates unconditionally.

3. **Minimal latency as a hard constraint** — latency is not a quality trade-off
   knob. It is a hard constraint the codec must satisfy first, then maximize quality
   within that constraint. Target: ≤8ms end-to-end on LAN.

4. **Three operating modes, zero configuration** — the receiver profile
   (AR / 2R / 47) is negotiated automatically from session context.
   No user-visible codec settings needed.

5. **Backward relay compatibility** — EVRT2 sessions fall back to TCP relay
   transparently. The same EVRT2CKMAX codec runs over the relay tunnel.

---

## High-Level Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                         HOST SIDE                                │
│                                                                  │
│   Screen / Game / VM                                             │
│         │                                                        │
│         ▼                                                        │
│   ┌─────────────────┐     Silicon probe result                  │
│   │  EVRT2CKMAX     │◄────────────────────────                  │
│   │  Encoder        │                                           │
│   │  ┌──────────┐   │   Mode: AR / 2R / 47 (AR2R47)            │
│   │  │ AR mode  │   │   Backend: Silicon | SW fallback          │
│   │  │ 2R mode  │   │                                           │
│   │  │ 47 mode  │   │                                           │
│   │  └──────────┘   │                                           │
│   └────────┬────────┘                                           │
│            │ EncodedFrame + metadata                            │
│            ▼                                                    │
│   ┌─────────────────┐                                           │
│   │  SD-UDP Layer   │  Super Dynamic UDP                        │
│   │  (EVRT2 wire)   │  Packet scheduler + FEC + feedback        │
│   └────────┬────────┘                                           │
│            │                                                    │
└────────────┼────────────────────────────────────────────────────┘
             │ UDP (primary) / TCP relay tunnel (fallback)
             │
┌────────────┼────────────────────────────────────────────────────┐
│            ▼                         CLIENT SIDE                │
│   ┌─────────────────┐                                           │
│   │  SD-UDP Layer   │  Jitter buffer, reorder, FEC recovery     │
│   └────────┬────────┘                                           │
│            │                                                    │
│            ▼                                                    │
│   ┌─────────────────┐                                           │
│   │  EVRT2CKMAX     │  Silicon decode (MediaCodec / NVDEC / VT) │
│   │  Decoder        │  or SW fallback                           │
│   └────────┬────────┘                                           │
│            │ RGBA / Surface (zero-copy to GPU)                  │
│            ▼                                                    │
│        Display                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Session Lifecycle

```
1. HANDSHAKE
   Client → Host: ClientHello { evrt2_version, silicon_caps, max_res, max_fps }
   Host → Client: ServerHello { evrt2_version, mode, silicon_caps, session_token }

   Wire encoding (implemented): SESSION_HELLO payload =
     max_fps u32 BE | max_res_w u32 BE | max_res_h u32 BE | extra_caps bytes…
   (`silicon_caps` travels inside `extra_caps` as a free-form blob until
   the capability registry's binary encoding is specified.)
   SESSION_ACK currently carries an empty payload — the full ServerHello
   fields (mode, session_token) are a specified-but-unimplemented gap.

2. MODE NEGOTIATION
   Host reads screen context → selects AR / 2R / 47
   Mode can change mid-session (e.g. game launches → 47)

3. SILICON PROBE (parallel to handshake, result ready before first frame)
   Host probes: NVENC → MF → Silicon SW chain
   Client probes: NVDEC → MediaCodec → VideoToolbox → SW

4. STREAMING
   Host encodes → SD-UDP packetizes → Client reassembles → decodes → displays

5. FEEDBACK LOOP (every 50–100ms)
   Client → Host: ReceiverFeedback2 { pressure, jitter, decoded_fps, silicon_ok }
   Host adjusts: bitrate, fps, mode hint

6. TEARDOWN
   Either side sends Goodbye → flush → close
```

---

## Compatibility with EVRT (2025)

EVRT2 and EVRT1 are **separate protocols** with different magic bytes.
An EVRT2 host can detect an EVRT1 client in handshake and fall back
to EVRT1 session automatically. Existing clients are never broken.

```
EVRT1 magic:  0x45565254  ("EVRT")
EVRT2 magic:  0x45565232  ("EVR2")
```
