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
  ROADMAP.md              ← implementation status + phased plan
  spec/
    EVRT2_OVERVIEW.md     ← high-level architecture
    EVRT2_PACKET.md       ← binary packet format (wire spec)
    EVRT2_SECURITY.md     ← auth, encryption, session tokens (both implemented)
  codec/
    EVRT2CKMAX.md         ← codec overview and design goals
    SILICON_PROBE.md      ← hardware detection and delegation, by fact (Phase 6.6)
  transport/
    SDUDP.md              ← SD-UDP scheduling, jitter, FEC, liveness
    FEEDBACK.md           ← receiver feedback loop v2, by fact (Phase 6.6)
    RELAY_TUNNEL.md       ← EVRT2 over TCP relay (fallback path, Phase 5.3)
  modes/
    AR2R47_MODES.md       ← the three operating modes (AR / 2R / 47)
  tasks/
    01_ABSOLUTE_NO_DELAY_VISIBLE_REGION.md    ← Task-01 (implemented)
    02_SILICON_MARGINAL_UTILITY_SCHEDULER.md  ← Task-02 (implemented)
```

As of ROADMAP.md Phase 6.6 (July 2026), every file this spec family
references has been written — `SILICON_PROBE.md` and `FEEDBACK.md` were
the last two, written by fact of implementation rather than up front, so
they document what actually ships (including the gaps between the design
in `SDUDP.md`/`02_SILICON_MARGINAL_UTILITY_SCHEDULER.md` and the real
code) instead of a plan.

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

Reference implementation in progress (EvertyDesk Lite, Rust) — the
protocol core is implemented, unit-tested, and **live-verified**
end-to-end (PC host ↔ Android phone over real WiFi, July 2026):

| Layer | Module | Status |
|-------|--------|--------|
| Wire format (32-byte header, magic `EVR2`) | `src/evrt2_packet.rs` | ✅ tested |
| XOR FEC + length-prefix recovery | `src/evrt2_fec.rs` | ✅ tested |
| Adaptive jitter buffer | `src/evrt2_jitter.rs` | ✅ live |
| AR2R47 mode state machine | `src/evrt2_modes.rs` | ✅ tested, not yet driving a live session |
| Task-01 scheduler (visible region, send order, breach) | `src/evrt2_scheduler.rs` | ✅ live |
| Attention Map (motion + focus + surprise) | `src/evrt2_attention.rs` | ✅ live |
| SD-UDP session engine (handshake, reassembly, FEC, transport-agnostic UDP/relay) | `src/evrt2_session.rs` | ✅ live |
| RELAY_WRAP (EVRT2 over TCP relay, symmetric-NAT fallback) | `src/evrt2_session.rs` + `evrt2_experiment.rs` | ⚠️ tested, partially live (see ROADMAP.md 5.3) |
| Execution Capability registry (Task-02) | `src/execution_capability.rs` | ✅ in production EVRTCK path |
| EVRT2-only live session mode | `src/evrt2_experiment.rs` | ✅ live |
| Attention Priority Field (u4 wire encoding) | `src/evrt2_apf.rs` | ✅ live |
| AuthTag (HMAC-SHA256) + ENCRYPTED (NaCl secretbox) | `src/evrt2_crypto.rs` | ✅ live |

See [`ROADMAP.md`](ROADMAP.md) for what is implemented versus still
specification-only, and the phased plan forward.
Existing EVRT + EVRTCK clients continue to operate unchanged.
