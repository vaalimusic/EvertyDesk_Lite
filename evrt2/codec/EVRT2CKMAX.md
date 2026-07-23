# EVRT2CKMAX — Codec Specification

**Author:** Arthur Valiev  
**Full name:** EvertyDesk Remote Transport 2 Codec Max  
**Version:** 2026-draft-4  
**Based on:** EVRT perceptual latency research (Habr, 2025)

**A note on the name:** "Codec" is kept for continuity with the
EVRT / EVRTCK / EVRT2CKMAX naming lineage — it is what the encoder
module inside this system is called. The system as a whole is not
an encode/decode compression pipeline; it is an observe → prioritize
→ allocate → transport → reconstruct pipeline, of which the codec is
one stage. Treat "EVRT2CKMAX" as the name of the overall standard,
and "codec" as the name of its innermost component.

---

## Document Map

| Section | Answers |
|---------|---------|
| [Foundational Statement](#foundational-statement) | What is EVRT2CKMAX, in one paragraph |
| [The Perceptual Age Field](#the-perceptual-age-field-paf) | The central object being optimized, and the formal objective function / EVRT Gain metric that make it measurable |
| [Valiev Law of Computational Opportunity](#valiev-law-of-computational-opportunity) | How silicon is discovered, shared, and never wasted — VUCP, Maximum Silicon Utilization, the marginal utility test, Overload Principle, Zero-Idle Doctrine |
| [Five Fundamental Objects](#five-fundamental-objects) | The five inputs every decision is a function of: Attention Map, Temporal Confidence, Reconstruction Budget, Transport Feedback, Execution Capability |
| [Attention Priority Field (APF)](#attention-priority-field-apf) | The wire format that carries attention data, including Temporal APF |
| [The Frame as Visual Debt](#the-frame-as-visual-debt) | The mental model for why regions update at different rates |
| [Warp and Prediction](#warp-and-prediction) | Client-side prediction and its hard boundary — the Causal Integrity Principle |
| [Valiev First Light Principle](#valiev-first-light-principle-принцип-первого-света) | Why the real target is arriving first, not compressing best — Perceived Acceleration, Codec Race, cross-codec splicing |
| [AR2R47 Modes](#ar2r47-modes--attention-budget-allocation) | How AR / 2R / 47 allocate the Attention Budget differently |
| [Comparison with Alternative Approaches](#comparison-with-alternative-approaches) | Codecs (H264/AV1/EVRTCK) as tools EVRT2CKMAX orchestrates, vs. orchestration layers (foveated rendering, DLSS) it's actually comparable to |
| [What this is not](#what-this-is-not) | Explicit non-claims — where the standard's promises end |

Related documents in this spec family: [`EVRT2_OVERVIEW.md`](../spec/EVRT2_OVERVIEW.md) (architecture),
[`EVRT2_PACKET.md`](../spec/EVRT2_PACKET.md) (wire format), [`SDUDP.md`](../transport/SDUDP.md) (transport),
[`AR2R47_MODES.md`](../modes/AR2R47_MODES.md) (mode detail).

---

## Foundational Statement

EVRT2CKMAX is an adaptive image representation system that allocates
computational, network, and encoding resources according to a dynamic
scene significance map for human perception.

This is not a compression algorithm.  
This is a **system of decisions about what information the viewer
must receive first** — and how much it costs to delay everything else.

The central hypothesis, from which the entire architecture follows:

> **Not every pixel in a frame is equally valuable to the viewer's
> perception at any given moment.**

From this follows a different optimization criterion.

A classical codec asks: *"How do we reduce the frame size?"*  
EVRT2CKMAX asks: *"What information must the viewer receive first?"*

This is a fundamentally different question. It changes the unit of
measurement from bits-per-frame to a field of per-region ages — the
**Perceptual Age Field (PAF)** — and the optimization target from
**bandwidth** to **Attention Budget**.

> **This is the central thesis of EVRT2CKMAX:**  
> **latency should be allocated according to human consequence,
> not pixel equality.**

Everything below — the Attention Map, the Reconstruction Budget,
the silicon scheduling rules — exists to serve this one idea.
The Perceptual Age Field is the object being optimized;
everything else is the mechanism that optimizes it.

---

## The Perceptual Age Field (PAF)

Every point on a displayed image carries an implicit **age** —
the time elapsed between when it was captured on the host and when
the viewer sees it.

In conventional streaming, age is treated as a constant: if the
frame arrived 50ms late, every pixel is 50ms late.

```
Classical model:
  Center:     50ms
  Edges:      50ms
  Crosshair:  50ms
  HUD:        50ms
  Background: 50ms
```

EVRT2CKMAX treats age as a **field** distributed across the image:

```
EVRT2CKMAX perceptual model:
  Local input (client-side):   0–5ms
  Focus zone (crosshair):     10–18ms
  Center of attention:        12–25ms
  Mid zone:                   25–40ms
  Periphery:                  50–90ms
  Static background:          80ms+   (sometimes reused from cache)
```

The question is no longer "how old is the screen?"  
The question is: **"In what order does the system return causally
important parts of the present to the viewer?"**

### Formal age definition

```
age(x, y, t) = t_display - t_capture(x, y, context)
```

Where `context` includes: content type, motion state, network
conditions, silicon availability, and the current Attention Map.

### Acceptable age function

For each scene object `i`, acceptable age is:

```
age_max(i) = a_min + (a_max - a_min) × (1 - P_i)^γ
```

Where:
- `a_min` = minimum latency (hardware floor, ~8ms on LAN)
- `a_max` = maximum acceptable age for background (~80ms)
- `P_i`   = attention priority of object i (0.0 to 1.0)
- `γ`     = nonlinearity factor (typically 0.5–2.0)

Example with `a_min=12ms`, `a_max=80ms`, `γ=0.7`:

| Object | Priority P_i | Acceptable age |
|--------|-------------|----------------|
| Active crosshair | 0.95 | ~12ms |
| Moving enemy | 0.85 | ~16ms |
| Player's hand | 0.75 | ~22ms |
| HUD (health) | 0.60 | ~30ms |
| Mid-ground action | 0.40 | ~44ms |
| Static background | 0.10 | ~70ms |
| Sky / fog | 0.02 | ~78ms |

### Formal objective function

Every scheduling, encoding, and transport decision in EVRT2CKMAX
reduces to one optimization problem:

```
Minimize:
    Σ_i  P_i × age_i

Subject to:
    bandwidth_used  ≤  B_available
    compute_used    ≤  C_available
    energy_used     ≤  E_budget
    age_i           ≤  age_max(i)      for every region i
```

This is the formal core the rest of this document implements.
The Attention Map supplies `P_i`. The Reconstruction Budget enforces
the constraints. The Execution Capability map determines what
`compute_used` can actually achieve. Every named principle in this
specification is a strategy for solving this one inequality under
real-world hardware and network conditions.

### Success metric: EVRT Gain

A concrete, measurable definition of the improvement EVRT2CKMAX
claims over frame-uniform delivery:

```
EVRT Gain = Uniform_Frame_Age − Attention_Weighted_Age

Attention_Weighted_Age = Σ_i (P_i × age_i) / Σ_i P_i
```

Example, 50ms uniform frame budget:

```
Uniform delivery:
    every region: 50ms
    Attention_Weighted_Age = 50ms
    EVRT Gain = 0

EVRT2CKMAX delivery (same total bandwidth):
    crosshair (P=0.95):     12ms
    enemy (P=0.85):         18ms
    HUD (P=0.60):           30ms
    background (P=0.10):    70ms

    Attention_Weighted_Age ≈ 19ms
    EVRT Gain ≈ 31ms
```

EVRT Gain is always measured **at fixed total bandwidth** — it is
not a claim of moving more bits, only of moving the *right* bits
sooner. This makes the metric falsifiable: it can be measured on
a real session by comparing achieved per-region age against a
uniform baseline at identical network conditions.

---

## Valiev Law of Computational Opportunity

> **Every available computational opportunity shall be exploited
> before additional latency is accepted.**

This is the architectural law from which silicon utilization follows.

EVRT2CKMAX does not target CPUs.  
EVRT2CKMAX does not target GPUs.  
EVRT2CKMAX targets **computational capability itself.**

The distinction matters because hardware generations end.
The law does not:

```
2024   CPU
2025   CPU  GPU
2026   CPU  GPU  NPU
2028   CPU  GPU  NPU  SmartNIC
2030   CPU  GPU  NPU  SmartNIC  Neural Upscaler
203x   Photonic Processor

The standard does not care.
```

The scheduler never asks "which RTX do you have?"  
It asks the platform: **"Who can do this fastest?"**

### Valiev Universal Compute Principle (VUCP)

Every compatible computational resource present on the platform
shall be considered a potential participant in the EVRT2CKMAX
execution pipeline.

If a computational resource is unavailable, the system shall
transparently redistribute the workload across the remaining
resources while preserving the lowest achievable perceptual latency.

### Valiev Maximum Silicon Utilization Principle

No available silicon shall remain idle if using it produces a net
improvement to the objective function above. Utilization is a means,
not an end — a resource is claimed because it helps, not because it
exists.

**Optimization hierarchy (in order):**

```
1. Perceptual latency     (PAF — Σ P_i × age_i, minimized)
2. Visual correctness     (no fabricated content — see Temporal Confidence)
3. Throughput              (sustained frame rate under load)
4. Energy efficiency       (cost per delivered frame)
5. Hardware utilization    (a consequence of 1–4, never a goal by itself)
```

Hardware utilization is deliberately last. A scheduler that maximizes
utilization at the expense of latency has inverted the standard's
purpose. **The goal was never "use everything." The goal was always
"minimize perceptual age." Using more silicon is only correct when
it serves that goal.**

### The marginal utility test

Parallelism has a cost: synchronization, cache invalidation, memory
traffic, PCIe transfer, and scheduling overhead are all real and can
exceed the benefit of adding another compute unit.

```
Example — ROI encode, 4ms budget:

  GPU alone:
      encode:              1.5ms
      total:               1.5ms   ✓ within budget

  GPU + CPU "helping":
      encode (GPU):        1.5ms
      copy to CPU:         0.8ms
      synchronization:     0.5ms
      total:               2.8ms   ✓ still within budget, but WORSE

  → CPU assistance here is a net loss. The Law of Computational
    Opportunity does not require it. The scheduler leaves CPU
    idle for this operation, and that is correct — not a violation.
```

The rule not: **"use everything."**  
The rule is:

> **Every resource whose marginal contribution improves the objective
> function shall be used. Idle silicon is not a violation.
> Wasteful silicon usage is.**

This is the condition that governs every principle below it. The
Overload Principle and Zero-Idle Doctrine describe *how* to spread
work across heterogeneous silicon when doing so helps — they do not
override this test.

### Valiev Overload Principle (Принцип Нагрузки Валиева)

The three principles above answer: **"who does this operation fastest?"**
— they select a single best provider per capability.

The Overload Principle answers a different question:
**"how is one operation split across every silicon unit present,
at the same instant, so that none of them sit idle while others work?"**

> **Every computational core available inside the silicon —
> old and new alike — shall carry a share of the load simultaneously.**

This is not sequential fallback (try NVENC, else CPU).  
This is **concurrent partition**: the same frame's workload is
sliced across whatever heterogeneous compute is physically present,
in parallel, in the same time window.

```
Single frame, mode 47, one instant:

  CPU core 0–1     →  entropy coding (AVX-512 / SVE2 SIMD path)
  CPU core 2       →  packet scheduling, FEC XOR
  GPU (RTX/iGPU)   →  ROI encode via silicon video engine
  NPU              →  motion prediction, attention map refinement
  Network card     →  DMA scatter/gather, checksum offload
  PCIe / NVLink    →  async copy, zero-copy buffer hand-off
  Old silicon      →  still assigned a share — legacy decode paths,
                       audio, or low-priority tile fallback
```

**Old silicon is not retired — it is demoted, not disconnected.**
A five-year-old integrated GPU that cannot keep up with mode 47's
ROI encoding is still assigned motion-vector precomputation or
FEC parity generation. It contributes something, always.

**Distributed instruction sets are first-class participants.**
AVX2, AVX-512, ARM SVE2, and any successor SIMD ISA are treated as
Execution Capabilities exactly like a discrete NPU — the scheduler
does not distinguish "an instruction set on the CPU" from
"a separate chip." Both are silicon that can carry load.

**Relationship to the Law of Computational Opportunity:**
the Law selects *the best single provider* when a capability has
one clear winner. The Overload Principle governs what happens to
*everything else* — it is never left idle, it is folded into the
same frame's workload as a lower-priority, lower-precision, or
auxiliary contributor.

```
future_hardware_rule:
    if new_compute_unit_detected(bus | core | accelerator):
        register_as_execution_capability_provider(unit)
        // no code change required — the standard already expects it
```

The standard does not enumerate today's silicon. It defines the
*obligation* to use whatever silicon is present, fully, at all times —
including hardware that will exist after this document is written.

### Valiev Zero-Idle Doctrine (Доктрина Нулевого Простоя)

The Overload Principle names cores and chips. The Zero-Idle Doctrine
goes one level deeper: **no silicon block is exempt from consideration
by virtue of being small** — every block is a candidate, subject to
the marginal utility test above.

> **If a transistor cluster inside the platform can perform work
> that passes the marginal utility test, it is not idle silicon.
> It is unclaimed capacity. EVRT2CKMAX claims it.**

This is a doctrine of *default inclusion*, not of *unconditional use*.
Every block below is evaluated by the same rule: does using it reduce
`Σ P_i × age_i` more than it costs to coordinate? If yes, claim it.
If no, it correctly sits idle — that is not a failure of the standard,
it is the standard working as designed.

This is deliberately exhaustive. The standard does not stop at
"CPU, GPU, NPU" — it descends to every functional block a modern
SoC or discrete card exposes, because every one of them is silicon
paid for and sitting unused the moment it is not driving the pipeline:

```
Compute blocks          →  CPU cores, GPU shader cores, NPU tensor cores
SIMD / vector units      →  AVX2, AVX-512, ARM NEON, SVE2, RVV
Dedicated media silicon  →  video encode/decode engines, ISP, JPEG blocks
DSP blocks               →  audio DSP, sensor-hub DSP, modem DSP
Crypto silicon           →  AES-NI, ARM CE, TPM crypto co-processor
Memory subsystem         →  memory controllers, cache prefetchers, TLB
                             walkers (as scheduling signal, not compute
                             — but their pressure gates every decision)
I/O and interconnect     →  PCIe lanes, NVLink, Thunderbolt, USB4 DMA
Network silicon          →  NIC offload engines, SmartNIC cores,
                             RDMA engines, checksum/segmentation offload
Display silicon          →  scaler/composer blocks, color pipeline,
                             display DSC encoders (reusable for tiling)
Storage silicon          →  NVMe controller queues, compression offload
                             (some SSD controllers run inline zstd)
Secure / auxiliary cores →  security enclave co-processors, always-on
                             low-power islands (when thermal budget allows)
```

**Nothing on this list is excluded by default because of size.**
A block is excluded when the marginal utility test fails for it —
coordination cost exceeds benefit — not because it seemed too small
to matter a priori.

**The granularity test:** before any block is marked "not applicable,"
the scheduler must ask — *can this block shift an operation off the
critical path, and does doing so cost less than it saves?* If both
are true, even a single-digit-percent contribution is claimed.
If either is false, the block is correctly left idle.

**This is the standard's identity, not an optimization detail.**
EVRT2CKMAX does not treat unexamined idle silicon as acceptable —
every block must be evaluated against the objective function at
least once per session. But a block that was evaluated and found
not to help is not a violation. **The violation is never
considering it. The violation is not using it when it would help.**
A conforming implementation evaluates every claimable block;
it does not necessarily use every one.

---

## Five Fundamental Objects

The EVRT2CKMAX system operates on five first-class objects.
Objects 1–4 describe what the system **knows** about the scene and
the network. Object 5 describes what the system **can do** about it —
the execution surface available to act on that knowledge.

Every scheduling, encoding, and transport decision is a function of
all five. None of them is optional; a system missing any one of them
is not implementing EVRT2CKMAX, only borrowing its name.

### 1. Attention Map

**What:** a 2D field of attention weights, same resolution as the frame
(downsampled to tile granularity for efficiency).

**How computed:**

```
P_i = normalize(
    w_A × A_i +   // attention probability (gaze model / history)
    w_M × M_i +   // motion intensity (optical flow magnitude)
    w_U × U_i +   // UI element importance (HUD, crosshair, minimap)
    w_T × T_i +   // target presence (enemy, objective, cursor)
    w_G × G_i +   // gaze transfer probability (where will player look next?)
    w_S × S_i +   // scene surprise (sudden appearance, flash, explosion)
    w_E × E_i     // engine semantic importance (game/app-reported)
)
```

**Why `E_i` is necessary, not optional:** motion and color-entropy
signals fail on content that is semantically important but visually
quiet — an enemy crouched behind smoke has near-zero motion and
near-zero entropy, yet the player is actively tracking it. `E_i` is
a hint channel the host application can populate directly: object
class, gameplay relevance, quest/objective flags, depth-occluded
target markers. When available, `E_i` is authoritative — it comes
from ground truth (the game engine), not inference from pixels.
When unavailable, `w_E = 0` and the formula degrades gracefully to
the vision-only signals above.

The weights `w_*` are mode-dependent:
- AR mode: w_U dominates (UI elements matter most in support sessions)
- 2R mode: w_M dominates (motion regions drive attention)
- 47 mode: w_T, w_S, and w_E dominate (targets, surprises, and engine
  hints in gaming — this is where semantic importance matters most)

**Output:** `attention_map[tile_x][tile_y] → f32 in [0.0, 1.0]`

The Attention Map is computed once per frame on the host,
transmitted as a compact priority field, and used by both
encoder (resource allocation) and decoder (reconstruction priority).

### 2. Temporal Confidence

**What:** per-region confidence that data from a previous frame
can safely represent current state.

```
C_i = f(motion_vector_coherence, depth_consistency,
         occlusion_probability, time_since_last_update)
```

**Key rule — direction of dependence:**

Low confidence does **not** give the system permission to delay a region further.  
Low confidence **removes** permission to show old data.

```
if C_i < C_threshold:
    force_refresh(region_i)  // must request fresh server data
else:
    age_budget(i) += delta_age × C_i  // can extend age proportionally
```

High confidence allows reuse. Low confidence forces correction.
This is the boundary between acceptable motion prediction
and fabricating content that never existed.

### 3. Reconstruction Budget

**What:** the total available compute and network capacity
for this frame, distributed across regions by attention priority.

```
total_budget = network_bandwidth × frame_interval - protocol_overhead

per_region_budget(i) = total_budget × (P_i / sum(P_j for all j))
```

This is the **Attention Budget** — the budget is not allocated
to the frame. It is allocated to human attention.

High-priority regions get more bits, more compute, fresher data.
Low-priority regions get fewer bits, reuse cache, update less often.

The encoder never asks "how much does this region cost?"  
It asks "how much attention budget does this region deserve?"

### 4. Transport Feedback

**What:** real-time network state from the client's ReceiverFeedback2.

```rust
struct ReceiverFeedback2 {
    pressure:        f32,   // decode queue pressure 0.0–1.0
    jitter_p95_us:   u32,   // measured network jitter (P95)
    rtt_us:          u32,   // round-trip time
    decoded_fps:     f32,   // actual client FPS
    silicon_ok:      bool,  // hardware decoder healthy
    dropped_frames:  u32,   // dropped since last feedback
    attention_hints: Vec<AttentionHint>,  // client-side gaze signals
}
```

Transport Feedback closes the loop: the encoder knows exactly
what the network allows, adjusts Reconstruction Budget accordingly,
and recalculates the Attention Map's achievable age targets.

### 5. Execution Capability

**What:** a capability is not hardware. It is a measurable ability
to perform a specific operation at a given cost.

The platform exposes a set of Execution Capabilities at session start.
The scheduler queries this set — not a hardware list.

```
Capability          Example providers
──────────────────────────────────────────────────────────────
ROI Encoding        NVENC, MediaCodec, VideoToolbox, AMF, QSV
Motion Estimation   GPU compute shader, NPU, CPU SIMD
Entropy Coding      CPU SIMD (x86 VPCLMULQDQ), dedicated engine
Warp / Homography   GPU vertex shader, NPU inference core
Prediction          ML model on NPU, CPU heuristic
DMA Transfer        iGPU shared memory, SmartNIC offload
Async Copy          CUDA copy engine, Metal blit encoder
Video Engine        dedicated video silicon
Crypto Engine       AES-NI, ARM crypto extension
Neural Upscaler     NPU, GPU compute (DLSS-style)
```

The scheduler never says "use NVENC."  
It says "I need ROI Encoding" and queries the platform.

```
capability_query(need: ROI_ENCODING, budget_ms: 4.0) →
    [
        (provider: NVENC,        cost_ms: 1.2, quality: 0.9),
        (provider: MediaCodec,   cost_ms: 1.8, quality: 0.85),
        (provider: CPU_x264_SW,  cost_ms: 12.0, quality: 0.7),
    ]

scheduler picks: NVENC  // lowest cost within budget
```

**Capability registration** happens in three phases:
1. **Probe at startup** — enumerate all present capabilities
2. **Cost calibration** — measure actual encode latency with a test frame
3. **Runtime tracking** — update cost estimates via Transport Feedback

Capabilities can appear and disappear mid-session
(GPU thermal throttle, NPU context stolen by OS).
The scheduler rebalances in the next frame boundary.

**Why this is the fifth object, not an implementation detail:**
the Reconstruction Budget (object 3) decides how much to spend.
The Execution Capability map decides where it can be spent.
Without both, the allocation is incomplete.

---

## Attention Priority Field (APF)

The APF is the wire representation of the Attention Map.

It is transmitted as part of each keyframe and optionally delta-updated
mid-session when content context changes (e.g., game scene switches).

### Wire format

```
APF Header (8 bytes):
  version    u8        = 1
  tile_size  u8        = 32 (pixels per tile edge)
  cols       u16       = ceil(frame_width / tile_size)
  rows       u16       = ceil(frame_height / tile_size)
  encoding   u8        = 0x01 (u4 packed) | 0x02 (f16) | 0x03 (delta) | 0x04 (temporal)
  reserved   u8        = 0

APF Payload:
  For encoding=u4:       4 bits per tile, 16 priority levels, rows×cols/2 bytes
  For encoding=f16:      2 bytes per tile (float16), rows×cols×2 bytes
  For encoding=delta:    run-length encoded delta from previous APF
  For encoding=temporal: 3 bytes per tile (priority + max_age + confidence),
                         see Temporal APF below
```

At 1080p with 32px tiles and u4 encoding:
- Grid: 60×34 = 2040 tiles
- APF size: 2040 / 2 = **1020 bytes** per full keyframe APF
- Delta APF for minor attention shifts: typically **50–200 bytes**

### Temporal APF (encoding = 0x04)

A priority value alone tells the decoder *how important* a tile is,
but not *how stale it is allowed to become* or *whether the last
known data can still be trusted*. Temporal APF packs all three:

```
Temporal APF tile (3 bytes, encoding=0x04):
  priority     u4    // 16 levels, same as base APF
  max_age      u12   // acceptable age in ms, 0–4095ms range (2ms steps: 0–8190ms)
  confidence   u8    // C_i × 255, packed reconstruction confidence

Example tile:
  priority:    0.92   (15/16)
  max_age:     15ms
  confidence:  0.85
  → "this region matters a lot, cannot be older than 15ms,
     and the decoder may currently trust ~85% of what it has"
```

This directly wires the `age_max(i)` formula and the Temporal
Confidence object (`C_i`) onto the transport layer, instead of
leaving the decoder to infer them from priority alone. A tile with
high priority but high confidence can still be left stale a little
longer than its `max_age` nominal value; a tile with low confidence
must be refreshed regardless of priority — the encoder no longer has
to choose between transmitting priority *or* staleness tolerance.

Temporal APF is optional — used in 2R and 47 modes where staleness
tolerance varies rapidly frame to frame. AR mode uses the simpler
u4/f16 encodings, since its lossless requirement makes `max_age` and
`confidence` largely moot (every dirty tile is retransmitted anyway).

### Semantic zones (examples)

The encoder classifies regions into semantic zones
to initialize weights before per-frame refinement:

```
Zone                  Default Priority    Refinement source
─────────────────────────────────────────────────────────
Active crosshair      0.95               Input state machine
Enemy / target        0.85               Game engine overlay / heuristic
Player hands / weapon 0.75               Motion + depth center
HUD (health/ammo)    0.65               UI region detector
Minimap              0.55               UI region detector
Center action area   0.50               Motion field
Mid-ground           0.30               Distance + motion
Periphery            0.15               Spatial falloff
Static background    0.08               Low motion, low priority
Sky / fog / smoke    0.03               Color statistics + low entropy
```

These defaults are overridden per frame by the actual Attention Map computation.

---

## The Frame as Visual Debt

A useful mental model: each screen region carries a **debt** to
the viewer — a promise of accurate, timely information.

The urgency of these debts differs:

| Region | Debt type | Urgency |
|--------|-----------|---------|
| Crosshair | Direct hand-eye link | Immediate |
| Enemy position | Consequence-critical | High |
| Explosion flash | Surprise event | High |
| HUD | Action confirmation | Medium |
| Mid-ground | Situational awareness | Medium |
| Background texture | Completeness | Low |
| Fog / sky | Atmosphere | Very low |

Classical streaming pays all debts simultaneously: it waits for a
complete new frame and replaces everything at once.

EVRT2CKMAX pays the most urgent debts first, and lets less urgent
debts settle over multiple frames. The **frame is not a photograph
of one moment** — it is a contract between:

- the last known truth (previous frame)
- fresh user input (local warp)
- local prediction (motion extrapolation)
- network delivery (actual server data)
- server correction (incoming delta)

---

## Warp and Prediction

When a fresh full frame is not yet available, EVRT2CKMAX applies
**Warp** — a local geometric transformation of the previous frame
using fresh client input, to reduce perceived latency before server
data arrives.

```
warped_frame(x, y, t) = last_frame(transform(x, y, Δinput))
```

### Two crosshairs principle

A direct consequence of the perceptual model.
Two representations of the player's aim exist simultaneously:

```
crosshair_local(t)  = cursor_position(t)              // 0–5ms age
crosshair_server(t) = server_acknowledged_position(t) // 12–25ms age
```

The distance between them is the **visual control debt**:

```
visual_debt = speed_px_per_ms × server_crosshair_age
// At 0.8px/ms speed, 15ms server age: 12px visual debt
```

`crosshair_local` confirms the input was received.  
`crosshair_server` confirms the game world acknowledged it.

### Valiev Causal Integrity Principle

`crosshair_local` must never claim a hit the server hasn't confirmed.
This is not a UI detail — it is a named, binding principle:

> **Prediction may reduce perceived latency but must never create
> unconfirmed reality.**

Every predictive mechanism in EVRT2CKMAX — Warp, motion extrapolation,
NPU-driven prediction (Execution Capability object 5) — is bound by
this line. The system is allowed to guess *where things are likely
to be*. It is never allowed to assert *that something happened* until
the server has confirmed it. Predicting a hand's trajectory is
prediction. Predicting a kill confirmation is fabrication. The
boundary between the two is the boundary of what this specification
permits.

### Reconstruction Confidence boundary

Warp is safe when C_i is high (static scene, predictable motion).  
Warp becomes dangerous when C_i drops below threshold:
newly occluded areas, fast-moving independent objects, explosions,
sudden new objects entering the frame.

```
if C_i(region) < 0.3:
    // Warp cannot invent this — request fresh server data
    emit IDR_REQUEST(region_mask=region_i)
```

---

## Valiev First Light Principle (Принцип Первого Света)

> **Something must reach the eye before nothing would have.**

This principle reframes what EVRT2CKMAX is actually optimizing for.
AV1 and H265 are not competitors here — they are Execution Capability
providers EVRT2CKMAX can call on (see Five Fundamental Objects,
object 5), and beating them at compression is not this project's
contest to fight; that ground is already won by decades of dedicated
codec engineering. EVRT2CKMAX's job is the orchestration decision
around them:

> **Be the first signal to arrive, not the most correct one.**

If the AV1 provider needs 40ms to produce its optimal encode and
EVRT2CKMAX's scheduler can put a plausible, honestly-labeled
approximation on screen in 12ms via a faster provider — followed by
a correction once the AV1 result exists — the viewer experienced
motion 28ms sooner. Whether that first frame came from "the best
possible encoding" is irrelevant to what the eye registered; it is
EVRT2CKMAX's scheduling decision, not any single codec's compression
ratio, that produced the gain.

### Perceived Acceleration

A frame delivered before the viewer's expectation baseline is not
experienced as "a smaller delay." It is experienced as
responsiveness — the system reads as *fast*, not *less slow*.

```
Perceived_Acceleration = expectation_baseline - age(first_visual_signal)

expectation_baseline = viewer's implicit latency expectation,
                        calibrated from local input round-trip
                        and prior session history

If age(first_visual_signal) < expectation_baseline:
    the delay is perceptually inverted — it reads as acceleration,
    not as waiting, even though physical transport time is > 0.
```

This is the same psychological mechanism behind the Two Crosshairs
principle (Warp and Prediction, above), generalized from cursor
position to arbitrary visual content: local approximation arrives
first, server truth corrects it silently and continuously, and the
viewer never perceives a gap because a plausible answer was always
on screen.

### Codec Race — running multiple providers on the same task at once

The Execution Capability object (object 5) currently describes
selecting **one best provider** per capability ahead of time. First
Light adds a second mode, used only where latency is the dominant
term of the objective function (the Visible Region, EVRT2CKMAX-TASK-01;
cold start; scene cuts):

```
race(capability: RoiEncoding, deadline_ms: 8.0) -> FirstResult:
    launch simultaneously, same region, same instant:
        - AV1 hardware block   (if present)
        - H265 hardware block  (if present)
        - EVRTCK tile delta    (always available, cheapest floor)
        - Warp extrapolation   (always available, near-zero cost)

    first candidate to produce a usable result wins and is sent.
    remaining candidates are cancelled — unless one of them finishes
    before the *next* frame's deadline, in which case its result
    silently replaces the raced frame as a correction, not a new frame.
```

**This is a third resource-use pattern, distinct from the other two
already in this specification:**

```
Law of Computational Opportunity  →  ONE task,  ONE chosen provider   (select ahead of time)
Overload Principle                →  MANY tasks, MANY providers       (spatial split, same instant)
First Light Codec Race            →  ONE task,  MANY providers        (temporal race, same instant)
```

Racing burns redundant compute — the cancelled candidates were not
free. This is a deliberate, bounded exception to the marginal
utility test, permitted only where the dominant term of
`Σ P_i × age_i` justifies the waste — the same exception carved out
for the Visible Region in EVRT2CKMAX-TASK-01. Racing the whole frame,
every frame, is not covered by this principle and remains subject to
the ordinary marginal utility test.

### Cross-codec region splicing

A direct consequence: nothing requires one frame to use one codec.
If AV1's hardware block finishes the crosshair region first while
H265 wins the mid-ground and EVRTCK tile-delta wins a static HUD
corner, all three ship in the same frame, tagged per-slice with the
provider that produced them. The decoder does not need to know or
care that three different codecs cooperated on one image — it
decodes each tagged slice with its matching decoder and composites
the result. Wire-level requirement: `VIDEO_FRAME` slices gain a
`provider_id` byte (packed alongside `PacketIndex`), so a single
`FrameId` can legitimately contain mixed-codec payloads.

### Boundary with Causal Integrity

First Light governs **visual appearance only**. It does not relax
the [Causal Integrity Principle](#valiev-causal-integrity-principle)
by one inch. An approximate frame may show a plausible silhouette
where an enemy probably is; it must never show a confirmed hit,
kill, or state change that the server has not sent. "Show something"
means *something visually reasonable*, never *something claimed as
fact*. The same line that governs Warp governs every raced candidate.

### Why untested claims are acceptable here, and where they stop being acceptable

This principle is, honestly, unproven. No implementation exists yet.
That is a correct state for a specification to be in — the claim is
falsifiable (EVRT Gain and Perceived Acceleration are both
measurable), and it stops being acceptable the moment someone reports
results without having run the test. Until EVRT2CKMAX-TASK-01 and
TASK-02 produce a working benchmark, "we will beat AV1 to the eye" is
a hypothesis this document states clearly as a hypothesis — not a
result.

---

## AR2R47 Modes — Attention Budget Allocation

The three modes differ in how the Attention Budget is allocated
and what encoding strategies are available.

### AR — Static / Support

```
Attention Budget allocation:
  UI elements:     40% of budget
  Cursor / focus:  25% of budget
  Text regions:    20% of budget
  Background:      15% of budget

Encoding:
  High-priority tiles:  XOR-delta, lossless (zstd-3)
  Low-priority tiles:   XOR-delta, lossless (ZRLE)
  Zero-change tiles:    1 bit in dirty-map only

APF update:   On cursor move, UI change, or >5% tile dirty
Warp:         Not used (latency not critical, accuracy matters more)
Max age:      12ms (UI elements) … 200ms (static background cache)
```

AR is the only mode where **lossless** is the absolute requirement.
Text and UI must be pixel-perfect. No exceptions.

### 2R — Dynamic / Video

```
Attention Budget allocation:
  Motion regions:  45% of budget
  Center area:     25% of budget
  UI overlays:     15% of budget
  Background:      15% of budget

Encoding:
  High-motion high-priority:  Silicon NAL slice (H264/H265)
  Low-motion any priority:    XOR-delta tile
  Static tiles:               Cache reuse (no transmission)

APF update:   Every 3–5 frames (optical flow delta)
Warp:         Light warp on camera moves (homography)
Max age:      15ms (motion center) … 80ms (static background)
```

### 47 — Gaming / No Compromises

```
Attention Budget allocation:
  Crosshair zone:  30% of budget (minimum age: 8ms)
  Enemy / target:  30% of budget (minimum age: 10ms)
  Action center:   25% of budget
  Periphery:       15% of budget

Encoding:
  All regions:     Silicon encoder (NVENC / MediaCodec / VT)
  XOR-delta:       DISABLED
  B-frames:        DISABLED
  Look-ahead:      DISABLED
  Low-latency:     HARDWARE FLAG ON

APF update:       Every frame (game state changes every frame)
Warp:             Aggressive (applied while waiting for next server frame)
Max age target:   8ms (focus zone) … 40ms (periphery)
Silicon:          REQUIRED
```

In mode 47, the Attention Map drives silicon encoder's **ROI
(Region of Interest)** hints — the hardware encoder itself allocates
more bits to high-priority tiles at the hardware level.
This is the intersection of perceptual model and silicon acceleration.

---

## Comparison with Alternative Approaches

This comparison only makes sense across two different categories, and
conflating them is a mistake worth naming explicitly: **codecs** answer
"how do I compress this region," while **EVRT2CKMAX is the layer that
decides which codec, if any, should handle that region, and when.**
H264/H265/AV1 are not peers of EVRT2CKMAX in this table — they are
tools EVRT2CKMAX can call on, exposed as Execution Capabilities (see
Five Fundamental Objects, object 5). Putting "EVRT2CKMAX vs AV1" side
by side as competitors was the wrong framing in earlier drafts of this
document; the corrected framing is below.

**Codecs — representation tools, called by EVRT2CKMAX per region:**

| Codec | Optimization Target | Attention-aware on its own? |
|-------|--------------------|--------------------|
| H264 / H265 | Bits per frame | No |
| AV1 | Bits per frame (better) | No |
| VP9 | Bits per frame | No |
| EVRTCK tile-delta | Bits per changed pixel, lossless | No (dirty-tile only, not priority-ordered pre-Task-01) |

None of these codecs know what matters to the viewer. Each compresses
whatever region it is handed with the same effort, regardless of
whether that region is the crosshair or the sky. That is not a flaw
in the codecs — it is outside their job description. It is EVRT2CKMAX's
job description.

**Orchestration/reconstruction layers — decide what gets attention:**

| Approach | Decides | Attention-aware? |
|----------|--------------------|--------------------|
| DLSS / FSR | Which pixels to reconstruct from fewer samples | Partially (sharpening heuristics, not scene semantics) |
| Foveated rendering | Where to render at full resolution | Yes, but only a single gaze point, requires an eye tracker |
| **EVRT2CKMAX** | **Which codec, which age budget, which region, right now** | **Yes — full field, no eye tracker required** |

EVRT2CKMAX's Attention Map is built from game state signals, motion
analysis, input device state, and historical gaze probability — not
from hardware gaze tracking. This is what makes it comparable to
foveated rendering in intent, but broader in mechanism: foveated
rendering answers "where is the eye," EVRT2CKMAX answers "where does
this scene, this input state, and this codec's cost model say
attention belongs" — and then hands the actual pixel work to
whichever codec (from the table above) fits that region's deadline.

---

## What this is not

This specification describes a **system of decisions**, not a
compression algorithm.

- It does not claim to invent pixels that don't exist.
- It does not claim to remove network latency.
- It does not claim to beat physics.
- It does not name any specific GPU, CPU, or silicon vendor.

It claims: **given the same network and hardware budget,
a viewer receives the causally important parts of reality faster
than any frame-uniform codec can provide.**

The gain is perceptual, not physical. Perceptual gains are real gains.

The hardware independence is intentional and permanent.
Any specification that names a specific accelerator
is a specification that ages.
EVRT2CKMAX names capabilities, not products.

---

*EVRT2CKMAX specification. Arthur Valiev, 2026.*  
*Based on "A frame doesn't have to exist in one time" (Habr, 2025).*
