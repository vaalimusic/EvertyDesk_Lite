# SILICON_PROBE — how EVRT2 actually finds out what hardware it can use

**Author:** Arthur Valiev
**Status:** ROADMAP.md Phase 6.6 — written by fact of implementation, July 2026
**Spec:** [`02_SILICON_MARGINAL_UTILITY_SCHEDULER.md`](../tasks/02_SILICON_MARGINAL_UTILITY_SCHEDULER.md), [`EVRT2CKMAX.md`](EVRT2CKMAX.md) § Execution Capability

---

## Why this document exists

The task doc describes `CapabilityRegistry::probe()` as a single Phase-1
step: "enumerate: rayon thread count, AVX2/AVX-512 …, wgpu adapter info,
MediaCodec/NVENC/VideoToolbox presence." In the actual implementation
there are **two different probing strategies** living side by side —
static/cheap probing for capabilities that are safe to enumerate up front,
and try-it-and-see probing for the one capability (NVENC) where enumeration
alone can't answer the question that matters. This document records which
is which, and why, since the task doc's pseudocode doesn't distinguish
them.

## Strategy 1 — static probe at startup (`CapabilityRegistry::probe`, `src/execution_capability.rs`)

Called once per process (`evrtck.rs`'s `capability_registry()`, an
`OnceLock`). Registers, synchronously, with no hardware I/O beyond what's
already cheap:

- **EntropyCoding**: `PROVIDER_CPU_SEQUENTIAL` and `PROVIDER_CPU_RAYON` —
  always both present (rayon's global pool exists whenever the process
  does). `cost_ms` starts at `0.0` — real numbers come from a SEPARATE
  calibration step (`calibrate_entropy_coding`, Phase 2), not from `probe`
  itself. This is the direct replacement for the old `RAYON_THRESHOLD`
  constant in `evrtck.rs`.
- **RoiEncoding**: `PROVIDER_CPU_EVRTCK` always (EVRTCK's own CPU tile
  codec works everywhere, no probe needed), plus `PROVIDER_GPU_WGPU` only
  when the caller already knows a `wgpu` adapter was found (`gpu_available:
  bool` parameter — `probe()` doesn't do its own adapter enumeration; the
  caller, `evrtck_wgpu.rs`'s own `WgpuEvrtckEncoder::try_new`, already did
  that work and just reports the yes/no here rather than duplicating the
  GPU init).

This strategy works because both questions it answers — "does a CPU exist"
and "did the wgpu adapter probe already succeed" — are either always-true
or already-known by the time `probe()` runs. Neither requires opening a
session or reserving a hardware resource just to ask.

## Strategy 2 — try-it-and-see at runtime (`NvencWorker::spawn`, `src/evrt2_experiment.rs`)

NVENC (`PROVIDER_NVENC_H264`) is **not** registered by `probe()` at all —
see `PROVIDER_NVENC_H264`'s own doc comment in `execution_capability.rs`:
"NVENC session availability can only be known by actually trying to open
one at runtime." This is a real, verified fact about the platform, not a
convenience shortcut:

- `nvenc.rs` exposes a lighter enumeration function,
  `nvenc_encoder_codecs()`, that queries the driver for which CODECS an
  NVENC-capable GPU supports (H264/H265/AV1) WITHOUT opening an encode
  session. **The live EVRT1 pipeline uses exactly this** (`host.rs`, for
  its own client-build capability negotiation). **The EVRT2 experimental
  path does not use it at all.**
- Instead, `run_experiment_encode_loop` calls `NvencWorker::spawn(...)`
  directly — which, underneath, calls `NvencEncoder::new(...)`, which
  calls the real `nvEncOpenEncodeSessionEx` FFI entry point
  (`nvenc_shim.cpp` via `src/nvenc.rs`). This is a REAL session open
  attempt, not a query.

**Why the heavier probe, when a cheaper one exists:** codec-support
enumeration answers "can this GPU theoretically encode H264" — it says
nothing about whether a session SLOT is actually free right now. NVENC
enforces a small, driver-level cap on concurrent encode sessions per
process/GPU (consumer drivers historically limited this to a handful).
Since the live EVRT1 pipeline might already be holding a session on the
same GPU (a real, deliberately-tested scenario — see
`two_concurrent_nvenc_sessions_do_not_interfere` below), the only way to
answer "is a session actually available to ME, right now" is to attempt to
open one and see whether it succeeds. Querying codec support first would
not have caught that failure mode at all.

### Throttling: session-OPEN attempts are rate-limited; ENCODE calls are not

```rust
// evrt2_experiment.rs, run_experiment_encode_loop
if nvenc.is_none() && last_nvenc_calibration.elapsed() >= NVENC_CALIBRATION_INTERVAL {
    // ... NvencWorker::spawn(...) ...
}
```

`NVENC_CALIBRATION_INTERVAL` = 1 second. This governs ONLY how often a
failed-or-not-yet-open session is retried — opening (or failing to open) a
session is expensive (driver/GPU round trip), so retrying every frame at
60fps would waste real work on a GPU that's already declared it has no
free slot. Once a session IS open (`nvenc: Some(worker)`), every single
captured frame is encoded through it, every frame, no further throttling —
throttling the ENCODE call too would leave nothing for the Codec Race
(ROADMAP Phase 6.3) to race against.

### Failure is expected, not exceptional

```rust
Err(e) => {
    if !nvenc_probe_failed_once {
        nvenc_probe_failed_once = true;
        log(&events, format!("EVRT2 (experimental): NVENC unavailable ({e}) — staying on CPU_EVRTCK"));
    }
}
```

A failed open (no NVIDIA GPU, driver not present, session limit already
used by the live EVRT1 pipeline, non-Windows/non-`live-nvenc-sdk` build)
is logged exactly ONCE per session, not once per retry — at 1 retry/second
this would otherwise flood the log for the entire session lifetime on any
machine without NVENC. `silicon_available` (fed into `ModeSelector`, see
`evrt2_modes.rs`) simply stays honestly `false`; nothing pretends a session
exists. Retries keep happening silently in the background in case
conditions change (e.g. the live EVRT1 pipeline's own session closes,
freeing a slot) — there's no permanent give-up state.

### A real hardware constraint this probing strategy surfaced

Documented in ROADMAP.md Phase 6.3 and reproduced here for the same
"probing" topic: an early version of the live NVENC test used a 64×64
synthetic resolution and got `nvEncInitializeEncoder failed with NVENC
status 8 (NV_ENC_ERR_INVALID_PARAM)` — not a bug in this code, but NVENC's
own minimum supported resolution being higher than 64×64. Fixed by testing
at 640×480. This is exactly the kind of failure mode try-it-and-see
probing catches and static codec-enumeration would not (`nvenc_encoder_codecs()`
would have reported H264 as supported regardless of the resolution that
would later be requested).

### Registering the result: `register_provider`, not `probe`

Once a session opens successfully, its real measured per-frame cost is fed
in via `CapabilityRegistry::register_provider()` — a general insert-or-
update-by-`(capability, id)` method, distinct from `probe()`'s
startup-only, static-only registration:

```rust
registry.register_provider(Provider {
    id: PROVIDER_NVENC_H264.to_owned(),
    capability: Capability::RoiEncoding,
    cost_ms: nvenc_cost_ms,   // real, this-frame measured encode time
    quality: 0.85,            // lossy, unlike EVRTCK's 1.0 — an honest
                               // approximation, not a measured perceptual score
});
```

Called every frame once a session is open (not throttled — see above),
because NVENC's real per-frame cost is exactly what `schedule()`'s
marginal-utility test (`CapabilityRegistry::schedule`, Phase 3) needs to
compare against EVRTCK's own just-measured cost. `quality: 0.85` is
explicitly flagged in the surrounding code comment as an honest
approximation (lossy vs. EVRTCK's lossless `1.0`), not a real measured
perceptual quality score — no perceptual-quality measurement exists in
this codebase.

## Demotion is also runtime-only, never static

`rebalance()`/`demote_provider()` (ROADMAP Phase 6.2, ` execution_capability.rs`)
extend this same "some facts are only knowable at runtime" theme one step
further: NVENC's *decode-side* health (`ReceiverFeedback2.silicon_ok`) is
also unknowable at probe time — a session can open successfully on the
host and still produce a bitstream the CLIENT's decoder can't decode
cleanly for reasons that only show up once real frames actually flow. See
[`FEEDBACK.md`](../transport/FEEDBACK.md) for the exact demotion mechanism
and cooldown.

**Live-found-and-fixed bug (2026-07-27):** the original wiring called
`rebalance()` on every FEEDBACK packet where `silicon_ok == false`, with
no regard for whether that was a fresh failure or a repeat of the SAME
stale status. Since demotion stops further NVENC frames from being sent
(`use_nvenc` goes false), the client's decode health can never be
re-verified once demoted — it just keeps reporting the same cached
`false` forever, and each repeat re-armed the 30s `DEMOTION_COOLDOWN`
clock, turning a documented "timeout, not a life sentence" into a
permanent lockout after any single transient decode hiccup. Confirmed
live: `nvenc_demoted` flipped `true` early in a session and stayed `true`
for 80+ seconds straight, with NVENC measured consistently cheaper than
EVRTCK the whole time. Fixed by edge-triggering the `rebalance()` call in
`evrt2_experiment.rs` (only a fresh healthy→unhealthy transition
demotes) — live-reconfirmed: the same scenario now recovers to
`nvenc_demoted=false` after ~30s, as originally intended.

## Summary table

| Question | Answered by | When | Cost if wrong |
|---|---|---|---|
| "Does a CPU exist for EntropyCoding?" | `probe()` (always true) | Startup | N/A — never wrong |
| "Was a wgpu adapter already found?" | `probe(gpu_available)` param | Startup | Caller's own `try_new` probe already paid this cost |
| "Can this GPU theoretically encode H264?" | `nvenc_encoder_codecs()` | On demand (EVRT1 pipeline only — EVRT2 experimental path doesn't call this) | Cheap query, no session held |
| "Is an NVENC session actually available to me right now?" | `NvencWorker::spawn` → real `nvEncOpenEncodeSessionEx` | Retried every `NVENC_CALIBRATION_INTERVAL` until it succeeds | Real driver/GPU round trip per attempt — throttled specifically because of this |
| "Is NVENC's cost actually competitive with EVRTCK this frame?" | `register_provider` + `schedule()` | Every frame, once a session is open | None — this is the cheap part, pure arithmetic over already-measured numbers |
| "Is the client's decode of NVENC's output actually healthy?" | `ReceiverFeedback2.silicon_ok` → `rebalance()` | Every FEEDBACK packet (~3s) | Bounded by `DEMOTION_COOLDOWN` (30s) — see FEEDBACK.md |

---

*EVRT2 SILICON_PROBE. Arthur Valiev, 2026.*
