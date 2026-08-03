// =============================================================================
// EVRT2CKMAX-TASK-02 — Execution Capability: the Marginal Utility Scheduler
// Spec: evrt2/tasks/02_SILICON_MARGINAL_UTILITY_SCHEDULER.md
// Spec: evrt2/codec/EVRT2CKMAX.md — Valiev Law of Computational Opportunity,
//       Five Fundamental Objects § 5 (Execution Capability)
// Author of the standard: Arthur Valiev. Rust implementation below.
// =============================================================================
//
//! Turns "Execution Capability" from a documented object into a running
//! component: a registry that knows what the platform can do, a cost model
//! calibrated against real, measured work (not a spec-sheet number), and a
//! scheduler that applies the marginal utility test from the standard instead
//! of a fixed constant.
//!
//! Phase 1 (`probe`) enumerates providers. Phase 2 (`calibrate_linear`) times
//! real work at two workload sizes and fits a linear cost model — this is
//! what makes `RAYON_THRESHOLD` (a number picked once and left in source)
//! replaceable by a measured, per-session decision. Phase 3 (`schedule`) is
//! the literal marginal-utility test from EVRT2CKMAX.md: a provider is
//! selected only if it is *cheaper than the baseline*, not merely "available
//! and within budget" — see the GPU-alone-vs-GPU+CPU example in the spec.
//!
//! Explicitly out of scope for this pass (see task doc "Non-Goals"):
//! full Zero-Idle Doctrine coverage of every silicon block, SmartNIC/DMA
//! offload, cross-session calibration persistence. This module starts with
//! the one capability that already has a real fixed-vs-measured decision in
//! the codebase (`EntropyCoding`, replacing `RAYON_THRESHOLD` in
//! `evrtck.rs`) and is built so more providers/capabilities register into
//! the same registry without changing its shape.

use std::collections::HashMap;
use std::time::Duration;

/// A capability the pipeline might need — see EVRT2CKMAX.md § Five
/// Fundamental Objects, object 5. Not every variant has a registered
/// provider yet; `providers_for` simply returns an empty slice for those.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// One measured execution path for a capability.
///
/// `cost_ms` is a scalar snapshot used by [`CapabilityRegistry::schedule`],
/// matching the spec's `capability_query()` pseudocode exactly (needed so the
/// GPU-alone-vs-GPU+CPU marginal utility example is reproducible as a direct
/// test — see acceptance criterion #3 in the task doc). For capabilities
/// whose real cost depends on workload size (EntropyCoding: cost scales with
/// tile count), the registry additionally keeps a [`LinearCost`] model
/// alongside the scalar snapshot — see `entropy_coding_provider_for`.
#[derive(Debug, Clone)]
pub struct Provider {
    pub id: String,
    pub capability: Capability,
    pub cost_ms: f32,
    pub quality: f32,
}

/// Calibrated cost model: `cost(n) = fixed_ms + per_item_ms * n`.
///
/// Fit from two real, timed measurements — never a theoretical estimate.
/// `fixed_ms` captures dispatch/synchronization overhead (thread-pool wakeup,
/// GPU command submission); `per_item_ms` captures marginal per-unit cost.
/// This is precisely the shape needed to answer "is parallelism worth it
/// *for this many tiles, on this machine, right now*" — the question
/// `RAYON_THRESHOLD = 64` answered with a constant picked once.
#[derive(Debug, Clone, Copy, Default)]
pub struct LinearCost {
    pub fixed_ms: f32,
    pub per_item_ms: f32,
}

impl LinearCost {
    /// Fit from two (n, elapsed) samples. Samples must have different `n`;
    /// if they don't (degenerate calibration input), the model falls back to
    /// treating the single sample as pure fixed overhead — a safe, if
    /// uninformative, default rather than dividing by zero.
    fn fit(n_a: usize, dur_a: Duration, n_b: usize, dur_b: Duration) -> Self {
        let (n_lo, t_lo, n_hi, t_hi) = if n_a <= n_b {
            (
                n_a as f32,
                dur_a.as_secs_f32() * 1000.0,
                n_b as f32,
                dur_b.as_secs_f32() * 1000.0,
            )
        } else {
            (
                n_b as f32,
                dur_b.as_secs_f32() * 1000.0,
                n_a as f32,
                dur_a.as_secs_f32() * 1000.0,
            )
        };
        let dn = n_hi - n_lo;
        if dn <= 0.0 {
            return LinearCost {
                fixed_ms: t_lo.max(0.0),
                per_item_ms: 0.0,
            };
        }
        let per_item_ms = ((t_hi - t_lo) / dn).max(0.0);
        let fixed_ms = (t_lo - per_item_ms * n_lo).max(0.0);
        LinearCost {
            fixed_ms,
            per_item_ms,
        }
    }

    pub fn estimate_ms(&self, n: usize) -> f32 {
        self.fixed_ms + self.per_item_ms * n as f32
    }
}

/// Phase 1+2+3: registry of what the platform can do, what it costs, and the
/// scheduler that picks a provider by the marginal utility test.
pub struct CapabilityRegistry {
    providers: HashMap<Capability, Vec<Provider>>,
    /// EntropyCoding-specific calibrated models, keyed by provider id
    /// ("CPU_Sequential", "CPU_Rayon"). Populated by `calibrate_entropy_coding`.
    entropy_models: HashMap<String, LinearCost>,
    /// ROADMAP.md Phase 6.2 (task doc Phase 4 — Runtime Rebalancing):
    /// provider id → when it was last demoted. A demoted provider is
    /// excluded from `schedule()` until `DEMOTION_COOLDOWN` elapses — see
    /// `rebalance`.
    demoted: HashMap<String, std::time::Instant>,
}

/// How long a demoted provider stays excluded from `schedule()` before it's
/// eligible again. Matches the task doc's own rationale for periodic
/// re-calibration ("catches thermal throttle, OS reclaiming an NPU
/// context, a GPU driver reset") — none of those are necessarily
/// permanent, so a demotion is a timeout, not a life sentence.
const DEMOTION_COOLDOWN: Duration = Duration::from_secs(30);

/// Provider ids used by this module's built-in providers. Kept as constants
/// so `evrtck.rs` and tests don't hand-type strings that can drift out of
/// sync with `probe()`.
pub const PROVIDER_CPU_SEQUENTIAL: &str = "CPU_Sequential";
pub const PROVIDER_CPU_RAYON: &str = "CPU_Rayon";
pub const PROVIDER_GPU_WGPU: &str = "GPU_WGPU";
/// ROADMAP.md Phase 6.1. The EVRT2 experimental path's own CPU tile
/// encoder, registered as a `RoiEncoding` baseline so `schedule()` has
/// something to compare a real silicon provider (`PROVIDER_NVENC_H264`)
/// against — distinct from `PROVIDER_GPU_WGPU` above, which is
/// `evrtck_wgpu.rs`'s GPU-accelerated tile-diff backend (still EVRTCK's own
/// lossless format), not a hardware video encoder.
pub const PROVIDER_CPU_EVRTCK: &str = "CPU_EVRTCK";
/// ROADMAP.md Phase 6.1. Not registered by `probe()` (unlike the constants
/// above) — NVENC session availability can only be known by actually trying
/// to open one at runtime (see `nvenc.rs`), so this provider is added later
/// via `register_provider` once that succeeds, not at platform-probe time.
pub const PROVIDER_NVENC_H264: &str = "NVENC_H264";

impl CapabilityRegistry {
    /// Phase 1 — enumerate providers present on this platform.
    ///
    /// Registers what is cheap and safe to detect synchronously at startup:
    /// - `EntropyCoding`: sequential CPU path and rayon-parallel CPU path are
    ///   always present (rayon's global pool exists whenever the crate does).
    ///   Cost is a placeholder until `calibrate_entropy_coding` runs — real
    ///   numbers require actually timing work, not guessing at probe time.
    /// - `RoiEncoding`: registers a GPU provider only if `gpu_available` is
    ///   true, mirroring the existing binary probe in `evrtck_wgpu.rs`
    ///   (`WgpuEvrtckEncoder::try_new`) — this is the "extend it to register
    ///   as a provider instead of being a silent yes/no switch" step named
    ///   in the task doc's Phase 1 (M5 wiring lives in evrtck.rs, since that
    ///   is where the actual `try_new` probe call already happens).
    pub fn probe(gpu_available: bool) -> Self {
        let mut providers: HashMap<Capability, Vec<Provider>> = HashMap::new();

        providers.insert(
            Capability::EntropyCoding,
            vec![
                Provider {
                    id: PROVIDER_CPU_SEQUENTIAL.to_owned(),
                    capability: Capability::EntropyCoding,
                    cost_ms: 0.0, // filled in by calibrate_entropy_coding
                    quality: 1.0, // lossless either way — quality is identical, only speed differs
                },
                Provider {
                    id: PROVIDER_CPU_RAYON.to_owned(),
                    capability: Capability::EntropyCoding,
                    cost_ms: 0.0,
                    quality: 1.0,
                },
            ],
        );

        // ROADMAP.md Phase 6.1: CPU_EVRTCK is always the RoiEncoding
        // baseline — unlike GPU_WGPU below, it needs no hardware probe (the
        // whole point of EVRTCK's CPU path is that it works everywhere).
        // `schedule()` needs a real competing baseline to test a silicon
        // provider's cost against; before this, RoiEncoding had no entry at
        // all unless a GPU happened to be present.
        let mut roi_providers = vec![Provider {
            id: PROVIDER_CPU_EVRTCK.to_owned(),
            capability: Capability::RoiEncoding,
            cost_ms: 0.0, // filled in by real per-frame timing, see evrt2_experiment.rs
            quality: 1.0, // lossless
        }];
        if gpu_available {
            roi_providers.push(Provider {
                id: PROVIDER_GPU_WGPU.to_owned(),
                capability: Capability::RoiEncoding,
                cost_ms: 0.0,
                quality: 1.0,
            });
        }
        providers.insert(Capability::RoiEncoding, roi_providers);

        Self {
            providers,
            entropy_models: HashMap::new(),
            demoted: HashMap::new(),
        }
    }

    /// ROADMAP.md Phase 6.1: insert or update a provider by `(capability,
    /// id)` — a repeat call (e.g. periodic re-calibration) overwrites the
    /// existing entry in place instead of accumulating duplicates. General
    /// enough for any capability; this is how a provider only knowable at
    /// runtime (NVENC session availability can't be determined until one is
    /// actually opened) gets into the registry, unlike the platform-level
    /// checks `probe()` does synchronously at startup.
    pub fn register_provider(&mut self, provider: Provider) {
        let list = self.providers.entry(provider.capability).or_default();
        if let Some(existing) = list.iter_mut().find(|p| p.id == provider.id) {
            *existing = provider;
        } else {
            list.push(provider);
        }
    }

    /// Phase 2 — calibrate the EntropyCoding cost model against real work.
    ///
    /// `run_sequential`/`run_rayon` execute the actual tile-encode workload
    /// (caller-supplied, so this module has zero coupling to `evrtck.rs`'s
    /// private tile functions) at `n_small` and `n_large` items and return
    /// the elapsed wall-clock time. Two points are the minimum needed to fit
    /// a line; callers should pick `n_small`/`n_large` that bracket the
    /// tile-count range a real frame actually produces (e.g. 8 and 512) so
    /// the fitted model is not extrapolated far outside its calibration
    /// range for typical frames.
    pub fn calibrate_entropy_coding(
        &mut self,
        n_small: usize,
        n_large: usize,
        mut run_sequential: impl FnMut(usize) -> Duration,
        mut run_rayon: impl FnMut(usize) -> Duration,
    ) {
        let seq_small = run_sequential(n_small);
        let seq_large = run_sequential(n_large);
        let seq_model = LinearCost::fit(n_small, seq_small, n_large, seq_large);

        let rayon_small = run_rayon(n_small);
        let rayon_large = run_rayon(n_large);
        let rayon_model = LinearCost::fit(n_small, rayon_small, n_large, rayon_large);

        self.entropy_models
            .insert(PROVIDER_CPU_SEQUENTIAL.to_owned(), seq_model);
        self.entropy_models
            .insert(PROVIDER_CPU_RAYON.to_owned(), rayon_model);

        // Keep the scalar Provider.cost_ms in sync too (evaluated at n_large,
        // a representative "busy frame" workload) so schedule() — which only
        // sees the scalar — reflects the same calibration.
        if let Some(list) = self.providers.get_mut(&Capability::EntropyCoding) {
            for p in list.iter_mut() {
                if p.id == PROVIDER_CPU_SEQUENTIAL {
                    p.cost_ms = seq_model.estimate_ms(n_large);
                } else if p.id == PROVIDER_CPU_RAYON {
                    p.cost_ms = rayon_model.estimate_ms(n_large);
                }
            }
        }
    }

    pub fn providers_for(&self, need: Capability) -> &[Provider] {
        self.providers.get(&need).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Phase 3 — the marginal utility test, verbatim from EVRT2CKMAX.md
    /// (`capability_query` → `scheduler picks: lowest cost within budget`)
    /// and from the task doc's `schedule()` pseudocode: a provider is
    /// selected only if it is *cheaper than the baseline*, not merely
    /// available and within budget. This is what distinguishes "use the
    /// fastest available thing" from "use it only if it's a net improvement" —
    /// the GPU-alone-vs-GPU+CPU example in EVRT2CKMAX.md is exactly a case
    /// where a candidate is within budget (2.8ms ≤ 4ms) but still correctly
    /// rejected because it is worse than the baseline (1.5ms GPU-alone).
    pub fn schedule(
        &self,
        need: Capability,
        budget_ms: f32,
        baseline_cost_ms: f32,
    ) -> Option<String> {
        self.providers_for(need)
            .iter()
            .filter(|p| !self.is_demoted(&p.id))
            .filter(|p| p.cost_ms <= budget_ms)
            .filter(|p| p.cost_ms < baseline_cost_ms)
            .min_by(|a, b| a.cost_ms.partial_cmp(&b.cost_ms).unwrap())
            .map(|p| p.id.clone())
    }

    /// ROADMAP.md Phase 6.2 (task doc Phase 4 — Runtime Rebalancing),
    /// verbatim from the task doc's own pseudocode:
    /// ```text
    /// pub fn rebalance(&mut self, feedback: &ReceiverFeedback2) {
    ///     if !feedback.silicon_ok {
    ///         self.demote_provider(current_video_provider);
    ///     }
    /// }
    /// ```
    /// The pseudocode's `current_video_provider` isn't a field of
    /// `ReceiverFeedback2` (the client reports its OWN decode-side silicon
    /// health, not which encode-side provider produced what it's
    /// decoding) — the caller (whoever is actually running the encode
    /// loop and knows which provider it picked this session) supplies it.
    /// Called every N frames from the Transport Feedback loop, per the
    /// task doc's own comment on the pseudocode — currently wired into
    /// the EVRT2 experimental host loop (`evrt2_experiment.rs`), which is
    /// where `ReceiverFeedback2`/`silicon_ok` actually flow today. EVRT1's
    /// OWN production feedback loop (`evrt_session.rs`) uses its own,
    /// older `ReceiverFeedback` type that has no `silicon_ok` field at
    /// all — wiring this into that path too would mean extending a
    /// production wire message, a separate and riskier change not made
    /// here.
    pub fn rebalance(
        &mut self,
        feedback: &crate::evrt2_session::ReceiverFeedback2,
        current_video_provider: &str,
    ) {
        if !feedback.silicon_ok {
            self.demote_provider(current_video_provider);
        }
    }

    /// Exclude `provider_id` from `schedule()` for `DEMOTION_COOLDOWN`.
    /// Idempotent-ish: calling it again while already demoted just resets
    /// the cooldown clock, which is the correct behavior for "still
    /// unhealthy" feedback arriving repeatedly rather than starting a
    /// fresh countdown from a stale timestamp.
    pub fn demote_provider(&mut self, provider_id: &str) {
        self.demoted
            .insert(provider_id.to_owned(), std::time::Instant::now());
    }

    /// Whether `provider_id` is currently excluded from `schedule()`. A
    /// demotion older than `DEMOTION_COOLDOWN` is treated as expired —
    /// this method itself doesn't clean up the map entry (harmless to
    /// leave a handful of stale entries around; `probe()`/`schedule()`
    /// only ever look at a bounded set of real provider ids).
    pub fn is_demoted(&self, provider_id: &str) -> bool {
        self.demoted
            .get(provider_id)
            .is_some_and(|since| since.elapsed() < DEMOTION_COOLDOWN)
    }

    /// The actual per-frame decision `evrtck.rs` needs: given `n` tiles to
    /// process right now, which EntropyCoding provider is faster? This is
    /// the direct replacement for `tile_count < RAYON_THRESHOLD`.
    ///
    /// Falls back to sequential (the always-correct, always-available
    /// choice) if calibration hasn't run yet — never panics, never picks an
    /// unmeasured provider blind.
    pub fn entropy_coding_provider_for(&self, n: usize) -> &'static str {
        let (Some(seq), Some(rayon)) = (
            self.entropy_models.get(PROVIDER_CPU_SEQUENTIAL),
            self.entropy_models.get(PROVIDER_CPU_RAYON),
        ) else {
            return PROVIDER_CPU_SEQUENTIAL;
        };
        if rayon.estimate_ms(n) < seq.estimate_ms(n) {
            PROVIDER_CPU_RAYON
        } else {
            PROVIDER_CPU_SEQUENTIAL
        }
    }

    /// True once `calibrate_entropy_coding` has run. Lets callers decide
    /// whether to trust `entropy_coding_provider_for` or fall back to a
    /// fixed threshold for the first few frames of a session.
    pub fn is_entropy_calibrated(&self) -> bool {
        self.entropy_models.contains_key(PROVIDER_CPU_SEQUENTIAL)
            && self.entropy_models.contains_key(PROVIDER_CPU_RAYON)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_registers_entropy_coding_providers_always() {
        let reg = CapabilityRegistry::probe(false);
        let providers = reg.providers_for(Capability::EntropyCoding);
        assert_eq!(providers.len(), 2);
        assert!(providers.iter().any(|p| p.id == PROVIDER_CPU_SEQUENTIAL));
        assert!(providers.iter().any(|p| p.id == PROVIDER_CPU_RAYON));
    }

    #[test]
    fn probe_registers_gpu_only_when_available() {
        // ROADMAP.md Phase 6.1: CPU_EVRTCK is always present now (the
        // RoiEncoding baseline schedule() needs to compare a silicon
        // provider against) — GPU_WGPU is the one that's conditional.
        let with_gpu = CapabilityRegistry::probe(true);
        let with_gpu_ids: Vec<&str> = with_gpu
            .providers_for(Capability::RoiEncoding)
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(with_gpu_ids.len(), 2);
        assert!(with_gpu_ids.contains(&PROVIDER_CPU_EVRTCK));
        assert!(with_gpu_ids.contains(&PROVIDER_GPU_WGPU));

        let without_gpu = CapabilityRegistry::probe(false);
        let without_gpu_ids: Vec<&str> = without_gpu
            .providers_for(Capability::RoiEncoding)
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(without_gpu_ids, vec![PROVIDER_CPU_EVRTCK]);
    }

    #[test]
    fn linear_cost_fit_recovers_known_line() {
        // cost(n) = 0.5ms fixed + 0.1ms/item — verify the fit recovers it
        // from two samples exactly (no noise in this synthetic case).
        let model = LinearCost::fit(
            10,
            Duration::from_secs_f32(0.0005 + 0.0001 * 10.0),
            100,
            Duration::from_secs_f32(0.0005 + 0.0001 * 100.0),
        );
        assert!((model.fixed_ms - 0.5).abs() < 0.01);
        assert!((model.per_item_ms - 0.1).abs() < 0.01);
    }

    #[test]
    fn linear_cost_fit_never_produces_negative_model() {
        // Degenerate/noisy input (cost went DOWN as n went up — measurement
        // noise) must not produce a negative per_item_ms that would make
        // estimate_ms() decrease with workload size, which is physically
        // nonsensical and would corrupt every downstream decision.
        let model = LinearCost::fit(10, Duration::from_millis(5), 100, Duration::from_millis(3));
        assert!(model.per_item_ms >= 0.0);
        assert!(model.fixed_ms >= 0.0);
    }

    #[test]
    fn calibrate_entropy_coding_picks_rayon_for_large_workload_only() {
        // Simulate realistic shapes: sequential scales linearly with n;
        // rayon has higher fixed overhead (thread-pool dispatch) but a
        // shallower per-item slope (parallel work). Below the crossover,
        // sequential must win; above it, rayon must win — this is the exact
        // behavior RAYON_THRESHOLD approximated with a single constant.
        let mut reg = CapabilityRegistry::probe(false);
        reg.calibrate_entropy_coding(
            8,
            512,
            |n| Duration::from_secs_f32(0.01 * n as f32 / 1000.0), // 0.01ms/tile, ~0 fixed
            |n| Duration::from_secs_f32(0.5 / 1000.0 + 0.002 * n as f32 / 1000.0), // 0.5ms fixed + 0.002ms/tile
        );
        assert!(reg.is_entropy_calibrated());

        // Crossover: 0.01*n = 0.5 + 0.002*n → 0.008*n = 0.5 → n = 62.5
        assert_eq!(reg.entropy_coding_provider_for(4), PROVIDER_CPU_SEQUENTIAL);
        assert_eq!(reg.entropy_coding_provider_for(500), PROVIDER_CPU_RAYON);
    }

    #[test]
    fn entropy_coding_provider_for_falls_back_before_calibration() {
        let reg = CapabilityRegistry::probe(true);
        assert!(!reg.is_entropy_calibrated());
        // Must not panic and must not pick an unmeasured provider blind.
        assert_eq!(
            reg.entropy_coding_provider_for(10_000),
            PROVIDER_CPU_SEQUENTIAL
        );
    }

    #[test]
    fn schedule_reproduces_gpu_alone_vs_gpu_plus_cpu_example() {
        // Direct reproduction of the worked example in EVRT2CKMAX.md
        // (Valiev Law of Computational Opportunity § the marginal utility
        // test): GPU alone costs 1.5ms; "GPU + CPU helping" costs 2.8ms
        // (encode 1.5ms + copy 0.8ms + sync 0.5ms) — still within a 4ms
        // budget, but worse than the GPU-alone baseline, so it must be
        // rejected. schedule() must return the GPU-only provider, proving
        // "within budget" is not sufficient on its own — this is acceptance
        // criterion #3 in the task doc.
        let mut providers = HashMap::new();
        providers.insert(
            Capability::RoiEncoding,
            vec![
                Provider {
                    id: "GPU_alone".to_owned(),
                    capability: Capability::RoiEncoding,
                    cost_ms: 1.5,
                    quality: 0.9,
                },
                Provider {
                    id: "GPU_plus_CPU".to_owned(),
                    capability: Capability::RoiEncoding,
                    cost_ms: 2.8,
                    quality: 0.9,
                },
            ],
        );
        let reg = CapabilityRegistry {
            providers,
            entropy_models: HashMap::new(),
            demoted: HashMap::new(),
        };

        let chosen = reg.schedule(Capability::RoiEncoding, 4.0, 1.5);
        // GPU_plus_CPU is within the 4ms budget but NOT cheaper than the
        // 1.5ms baseline (equal, not strictly less) — correctly excluded.
        // GPU_alone is also not strictly cheaper than itself as baseline,
        // so with baseline_cost_ms == the best provider's own cost, nothing
        // passes the strict-improvement test. This documents the edge case:
        // the baseline in a real call site is the cost of NOT adding this
        // provider (e.g. CPU-only, or "no extra help"), not the winner's own
        // cost — see the next assertion for the realistic framing.
        assert_eq!(chosen, None);

        // Realistic framing: baseline is "CPU alone, no GPU" at, say, 6ms.
        // Both GPU options beat that baseline; the scheduler must pick the
        // cheaper one (GPU_alone), never GPU_plus_CPU.
        let chosen = reg.schedule(Capability::RoiEncoding, 4.0, 6.0);
        assert_eq!(chosen.as_deref(), Some("GPU_alone"));
    }

    // ── ROADMAP.md Phase 6.2: rebalance() / demote_provider() ──────────

    fn feedback_with_silicon_ok(silicon_ok: bool) -> crate::evrt2_session::ReceiverFeedback2 {
        crate::evrt2_session::ReceiverFeedback2 {
            frame_id: 0,
            pressure: 0.0,
            jitter_p95_us: 0,
            decoded_fps: 60.0,
            silicon_ok,
            dropped_frames: 0,
            rtt_us: 0,
        }
    }

    #[test]
    fn rebalance_demotes_the_current_provider_on_silicon_ok_false() {
        let mut providers = HashMap::new();
        providers.insert(
            Capability::RoiEncoding,
            vec![
                Provider {
                    id: "Silicon".to_owned(),
                    capability: Capability::RoiEncoding,
                    cost_ms: 1.0,
                    quality: 0.9,
                },
                Provider {
                    id: "CPU_fallback".to_owned(),
                    capability: Capability::RoiEncoding,
                    cost_ms: 5.0,
                    quality: 0.7,
                },
            ],
        );
        let mut reg = CapabilityRegistry {
            providers,
            entropy_models: HashMap::new(),
            demoted: HashMap::new(),
        };

        // Before rebalance: Silicon (cheaper) wins, as expected.
        assert_eq!(
            reg.schedule(Capability::RoiEncoding, 10.0, 100.0)
                .as_deref(),
            Some("Silicon")
        );

        // Task doc acceptance criterion #4: rebalance() reacts to a
        // simulated silicon_ok:false within one call — not eventually,
        // not after N samples, immediately.
        reg.rebalance(&feedback_with_silicon_ok(false), "Silicon");
        assert!(reg.is_demoted("Silicon"));

        // schedule() must now skip the demoted provider even though it's
        // still objectively the cheapest one on paper.
        assert_eq!(
            reg.schedule(Capability::RoiEncoding, 10.0, 100.0)
                .as_deref(),
            Some("CPU_fallback")
        );
    }

    #[test]
    fn rebalance_does_not_demote_on_silicon_ok_true() {
        let mut reg = CapabilityRegistry {
            providers: HashMap::new(),
            entropy_models: HashMap::new(),
            demoted: HashMap::new(),
        };
        reg.rebalance(&feedback_with_silicon_ok(true), "Silicon");
        assert!(!reg.is_demoted("Silicon"));
    }

    #[test]
    fn demote_provider_expires_after_the_cooldown() {
        let mut reg = CapabilityRegistry {
            providers: HashMap::new(),
            entropy_models: HashMap::new(),
            demoted: HashMap::new(),
        };
        reg.demote_provider("Silicon");
        assert!(
            reg.is_demoted("Silicon"),
            "must be demoted immediately after demote_provider"
        );
        // Backdate the demotion past the cooldown window directly (same
        // module, private field access) instead of a real sleep — proves
        // the SAME expiry logic `is_demoted` uses on a live timestamp,
        // without making the test take 30 real seconds.
        reg.demoted.insert(
            "Silicon".to_owned(),
            std::time::Instant::now() - DEMOTION_COOLDOWN - Duration::from_secs(1),
        );
        assert!(
            !reg.is_demoted("Silicon"),
            "must expire once DEMOTION_COOLDOWN has elapsed"
        );
    }

    #[test]
    fn repeated_demotion_resets_the_cooldown_clock() {
        let mut reg = CapabilityRegistry {
            providers: HashMap::new(),
            entropy_models: HashMap::new(),
            demoted: HashMap::new(),
        };
        // Simulate a demotion that's ALMOST expired...
        reg.demoted.insert(
            "Silicon".to_owned(),
            std::time::Instant::now() - DEMOTION_COOLDOWN + Duration::from_millis(100),
        );
        assert!(reg.is_demoted("Silicon"));
        // ...then fresh bad feedback arrives again — must restart the
        // cooldown from now, not leave the almost-expired timestamp.
        reg.demote_provider("Silicon");
        assert!(reg.is_demoted("Silicon"));
    }

    #[test]
    fn register_provider_appends_a_new_one_and_overwrites_a_repeat() {
        let mut reg = CapabilityRegistry::probe(false);
        assert_eq!(reg.providers_for(Capability::RoiEncoding).len(), 1); // CPU_EVRTCK baseline only

        reg.register_provider(Provider {
            id: PROVIDER_NVENC_H264.to_owned(),
            capability: Capability::RoiEncoding,
            cost_ms: 3.0,
            quality: 0.85,
        });
        assert_eq!(reg.providers_for(Capability::RoiEncoding).len(), 2);
        let nvenc = reg
            .providers_for(Capability::RoiEncoding)
            .iter()
            .find(|p| p.id == PROVIDER_NVENC_H264)
            .unwrap();
        assert_eq!(nvenc.cost_ms, 3.0);

        // Re-registering the SAME id (a real re-calibration tick) must
        // update in place, not create a second NVENC_H264 entry.
        reg.register_provider(Provider {
            id: PROVIDER_NVENC_H264.to_owned(),
            capability: Capability::RoiEncoding,
            cost_ms: 2.5,
            quality: 0.85,
        });
        assert_eq!(
            reg.providers_for(Capability::RoiEncoding).len(),
            2,
            "must overwrite, not duplicate"
        );
        let nvenc = reg
            .providers_for(Capability::RoiEncoding)
            .iter()
            .find(|p| p.id == PROVIDER_NVENC_H264)
            .unwrap();
        assert_eq!(nvenc.cost_ms, 2.5, "must reflect the latest calibration");
    }

    #[test]
    fn schedule_picks_nvenc_over_evrtck_once_registered_cheaper() {
        // ROADMAP.md Phase 6.1 end-to-end shape: CPU_EVRTCK is the
        // baseline, NVENC gets registered once a real session opens and is
        // calibrated cheaper — schedule() must then prefer it, same
        // marginal-utility test as any other capability.
        let mut reg = CapabilityRegistry::probe(false);
        reg.register_provider(Provider {
            id: PROVIDER_CPU_EVRTCK.to_owned(),
            capability: Capability::RoiEncoding,
            cost_ms: 8.0,
            quality: 1.0,
        });
        reg.register_provider(Provider {
            id: PROVIDER_NVENC_H264.to_owned(),
            capability: Capability::RoiEncoding,
            cost_ms: 3.0,
            quality: 0.85,
        });
        let chosen = reg.schedule(Capability::RoiEncoding, 16.0, 8.0);
        assert_eq!(chosen.as_deref(), Some(PROVIDER_NVENC_H264));
    }

    #[test]
    fn schedule_excludes_providers_over_budget() {
        let mut providers = HashMap::new();
        providers.insert(
            Capability::EntropyCoding,
            vec![Provider {
                id: "TooSlow".to_owned(),
                capability: Capability::EntropyCoding,
                cost_ms: 10.0,
                quality: 1.0,
            }],
        );
        let reg = CapabilityRegistry {
            providers,
            entropy_models: HashMap::new(),
            demoted: HashMap::new(),
        };
        assert_eq!(reg.schedule(Capability::EntropyCoding, 4.0, 20.0), None);
    }
}
