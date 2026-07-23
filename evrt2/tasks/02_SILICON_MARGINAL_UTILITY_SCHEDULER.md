# EVRT2CKMAX — SILICON: THE MARGINAL UTILITY EXECUTION SCHEDULER

**Task ID:** EVRT2CKMAX-TASK-02  
**Author:** Arthur Valiev  
**Status:** Specification → pending implementation  
**Depends on:** [`EVRT2CKMAX.md`](../codec/EVRT2CKMAX.md) — Valiev Law of
Computational Opportunity, Execution Capability (object 5), the
marginal utility test

> Proposed pairing with Task 01 — Task 01 guarantees *when* the
> visible region must not be late; this task builds *what* decides
> which silicon does the work to make that guarantee affordable.
> Rename or rescope freely.

---

## Problem Statement

The specification currently *describes* five capability categories,
a `capability_query()` pseudocode shape, and a marginal utility test
with one worked example (GPU-alone vs GPU+CPU). None of it runs.
Today's actual code (`evrtck.rs`, `evrtck_wgpu.rs`) makes exactly one
binary choice — CPU tile path vs. GPU tile path via
`RAYON_THRESHOLD` — and nothing else in the pipeline queries
hardware capability at all.

This task turns Execution Capability from a documented object into
a running component: a registry that knows what the platform can do,
a cost model that knows what each option actually costs *on this
machine, right now*, and a scheduler that applies the marginal
utility test instead of a fixed threshold constant.

## Scope

Build the three phases already named in the spec
(`EVRT2CKMAX.md` § Execution Capability):

### Phase 1 — Capability Registry (probe at startup)

```rust
// src/execution_capability.rs (new)

pub enum Capability {
    RoiEncoding,
    MotionEstimation,
    EntropyCoding,
    WarpHomography,
    Prediction,
    DmaTransfer,
    AsyncCopy,
    VideoEngine,
    CryptoEngine,
    NeuralUpscale,
}

pub struct Provider {
    pub id: ProviderId,          // e.g. "NVENC", "CPU_AVX512", "MediaCodec"
    pub capability: Capability,
    pub cost_ms: f32,            // calibrated, not theoretical
    pub quality: f32,            // 0.0–1.0, relative to best known provider
}

pub struct CapabilityRegistry {
    providers: HashMap<Capability, Vec<Provider>>,
}

impl CapabilityRegistry {
    pub fn probe() -> Self { /* enumerate: rayon thread count, AVX2/AVX-512
                                 via std::is_x86_feature_detected!, wgpu
                                 adapter info, MediaCodec/NVENC/VideoToolbox
                                 presence via existing evrtck_wgpu.rs probe */ }
}
```

Reuses detection that partially exists already:
- `evrtck_wgpu.rs` already probes for a usable GPU adapter — extend
  it to register as a `RoiEncoding` / `MotionEstimation` provider
  instead of being a silent yes/no switch.
- Android `isMimeSupported()` (`VideoDecoder.kt`) is the same idea
  already implemented client-side for decode; this task is the
  host-side, encode-side, general-capability equivalent.

### Phase 2 — Cost Calibration

```rust
impl CapabilityRegistry {
    pub fn calibrate(&mut self, test_frame: &Frame) {
        // Run each registered provider once against a real frame,
        // measure actual cost_ms, not a spec-sheet number.
        // Store as EMA, updated again every N frames at runtime
        // (Phase 3) so thermal throttling changes the number too.
    }
}
```

Calibration must use a representative frame from the actual session
(current resolution, current tile density), not a synthetic
benchmark — a GPU's ROI-encode cost at 1080p30 and 4K120 are not the
same number, and the registry must not lie about that.

### Phase 3 — Marginal Utility Scheduler (runtime)

```rust
pub fn schedule(
    registry: &CapabilityRegistry,
    need: Capability,
    budget_ms: f32,
    baseline_cost_ms: f32,   // cost of doing nothing extra / single-provider cost
) -> Option<ProviderId> {
    let candidates = registry.providers_for(need);
    candidates.iter()
        .filter(|p| p.cost_ms <= budget_ms)
        .filter(|p| p.cost_ms < baseline_cost_ms) // marginal utility test:
                                                    // must be a net improvement,
                                                    // not just "available"
        .min_by(|a, b| a.cost_ms.partial_cmp(&b.cost_ms).unwrap())
        .map(|p| p.id.clone())
}
```

This function is the literal implementation of the marginal utility
test from `EVRT2CKMAX.md`: a provider is only selected if using it
is cheaper than the baseline, replacing today's fixed
`RAYON_THRESHOLD = 64` constant with a measured, per-session decision.

The `RAYON_THRESHOLD` constant in `evrtck.rs` becomes the **first
real caller** of this scheduler — replace the hardcoded tile-count
cutoff with `schedule(Capability::EntropyCoding, ...)`, calibrated
against the actual CPU core count and cache behavior of the host
machine instead of a number picked once and left in source.

### Phase 4 — Runtime Rebalancing

```rust
// Called every N frames from the Transport Feedback loop (evrt_session.rs)
pub fn rebalance(&mut self, feedback: &ReceiverFeedback2) {
    if !feedback.silicon_ok {
        self.demote_provider(current_video_provider);
    }
    // Re-run Phase 2 calibration periodically — catches thermal
    // throttle, OS reclaiming an NPU context, a GPU driver reset.
}
```

## Non-Goals (explicitly out of scope for this task)

- Full Execution Capability coverage of every block listed in the
  Zero-Idle Doctrine (crypto engines, storage compression offload,
  etc.) — start with the capabilities that already have partial
  probing code (`RoiEncoding` via `evrtck_wgpu.rs`, `EntropyCoding`
  via rayon/AVX) and expand later.
- SmartNIC / DMA offload — no target hardware for this in the current
  client base; defer until a concrete platform needs it.
- Cross-session learning (caching calibration results between runs) —
  calibrate fresh each session first; persistence is a later
  optimization, not required for the scheduler to be correct.

## Acceptance Criteria

1. `CapabilityRegistry::probe()` correctly enumerates available
   providers on: Windows+NVIDIA, Windows+integrated GPU only,
   Android (MediaCodec present/absent), macOS+VideoToolbox — verified
   against `benches/evrtck_bench.rs` on at least two of these
   platforms.
2. Replacing `RAYON_THRESHOLD` with `schedule()` produces equal or
   better encode latency on the existing `evrtck_bench.rs` benchmark
   suite — the new scheduler must not regress the one decision the
   codec already makes correctly.
3. The GPU-alone-vs-GPU+CPU marginal utility example from
   `EVRT2CKMAX.md` is reproducible as an automated test: construct a
   scenario where CPU assistance costs more than it saves, assert
   `schedule()` returns the GPU-only provider.
4. `rebalance()` demonstrably reacts to a simulated `silicon_ok: false`
   feedback signal within one calibration cycle.

## Implementation Milestones (Rust)

| Milestone | File | Description |
|-----------|------|-------------|
| M1 | `src/execution_capability.rs` (new) | `Capability`, `Provider`, `CapabilityRegistry` types + `probe()` |
| M2 | `src/execution_capability.rs` | `calibrate()` against a real frame |
| M3 | `src/execution_capability.rs` | `schedule()` — marginal utility test |
| M4 | `src/evrtck.rs` | Replace `RAYON_THRESHOLD` constant usage with `schedule()` call |
| M5 | `src/evrtck_wgpu.rs` | Register GPU adapter as `RoiEncoding`/`MotionEstimation` provider instead of binary probe |
| M6 | `src/evrt_session.rs` | Wire `rebalance()` into the existing Transport Feedback loop |
| M7 | `benches/evrtck_bench.rs` | Extend benchmark to cover Acceptance Criteria #2–#3 |

M1–M4 are buildable against the current EVRTCK/EVRT1 codebase today —
none of this requires the EVRT2 wire format. This is the same
observation as Task 01: the scheduling and capability logic is
transport-independent and can ship before the rest of EVRT2 does.

---

*EVRT2CKMAX-TASK-02. Arthur Valiev, 2026.*
