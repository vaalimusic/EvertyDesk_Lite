# EVRT2 — 2026 Specification

**EVRT2** is the next generation of the EvertyDesk Remote Transport protocol,
succeeding EVRT (2025). Designed and authored by **Arthur Valiev**.

---

## What changed from EVRT → EVRT2

| | EVRT (2025) | EVRT2 (2026) |
|-|-------------|--------------|
| Transport | UDP | Super Dynamic UDP (SD-UDP) |
| Visual intelligence | Fixed tile codec selection (EVRTCK, or hand off whole-frame to H264/H265/AV1) | EVRT2CKMAX — attention-driven representation orchestration |
| Modes | One mode | AR · 2R · 47 (AR2R47) |
| Latency target | <20ms LAN | <8ms LAN, <30ms WAN |
| Max FPS | 60 | 120 |
| Max resolution | 1080p native, 4K downscaled | 4K native |
| Min bandwidth | ~500KB/s | 10KB/s (AR mode) |
| Silicon awareness | Optional (EVRTCK-Silicon) | Native, mandatory probe at start |

---

## Directory structure

```
evrt2/
  README.md               ← this file
  spec/
    EVRT2_OVERVIEW.md     ← high-level architecture
    EVRT2_PACKET.md       ← binary packet format (wire spec)
    EVRT2_SDUDP.md        ← Super Dynamic UDP transport layer
    EVRT2_SECURITY.md     ← auth, encryption, session tokens
  codec/
    EVRT2CKMAX.md         ← codec overview and design goals
    SILICON_PROBE.md      ← hardware detection and delegation
    AR2R47_MODES.md       ← three codec operating modes
  transport/
    SDUDP.md              ← SD-UDP packet scheduling, jitter, FEC
    FEEDBACK.md           ← receiver feedback loop v2
    RELAY_TUNNEL.md       ← EVRT2 over TCP relay (fallback path)
  modes/
    AR_STATIC.md          ← AR mode: desktop/support, lossless
    2R_DYNAMIC.md         ← 2R mode: video/animation
    47_GAMING.md          ← 47 mode: gaming, no compromises
```

---

## Relationship with existing codecs

**EVRT2CKMAX is codec-agnostic.** H264, H265, AV1, and any future codec
are not competitors to be beaten — they are **Execution Capabilities**
(see [`EVRT2CKMAX.md`](codec/EVRT2CKMAX.md) § Five Fundamental Objects,
object 5) that EVRT2 can call on for a given region, at a given moment,
when they are the fastest tool for that specific job.

```
EVRT2CKMAX
    │
    ├── AV1 capability            (RoiEncoding provider)
    ├── H265 capability           (RoiEncoding provider)
    ├── custom tile-delta capability   (EVRTCK, lossless, zero silicon)
    ├── lossless block-transfer capability
    ├── prediction capability     (Warp, motion extrapolation)
    └── hardware encoder capability    (whatever silicon is present)
```

The optimization target is **not compression efficiency**. It is
**minimum perceptual age under constrained resources** (the Perceptual
Age Field — see EVRT2CKMAX.md). A better codec improves one possible
execution path. EVRT2 improves the decision of when, where, and how
that path is used — and that decision can vary **per region, per frame**:

```
One EVRT2 frame, one instant:

  crosshair:    local prediction + tiny delta   (near-zero age)
  enemy:        low-latency hardware encode     (H264/H265 slice)
  background:   AV1, cached, relaxed deadline    (best compression, no rush)
  static UI:    lossless tile delta              (EVRTCK, pixel-perfect)
  fog:          reuse previous reconstruction    (zero transmission)
```

All five regions ship in the same frame, each through whichever
capability is the right tool for that region's priority and deadline.

**The question this project is actually asking is not** "can EVRT2CKMAX
beat AV1 at compression?" — that contest belongs to codec engineering,
not to this project. **The question is:**

> Can EVRT2 deliver the perceptually significant part of a changing
> scene to a human being faster than any frame-uniform pipeline can,
> under the same resource constraints?

That is a transport-and-orchestration question, not a compression
question, and it is the one this specification is built to answer.

---

## The name: AR2R47 = ARTUR 47

The three codec modes spell **AR · 2R · 47** — a tribute to **Arthur Valiev**,
creator of EVRT, EVRTCK, and the EvertyDesk platform.

> *"The codec carries the name of its author, encoded in its operating modes."*

---

## Current status

Specification phase. No production code yet.
Existing EVRT + EVRTCK clients continue to operate unchanged.
