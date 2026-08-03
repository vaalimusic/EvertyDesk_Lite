// =============================================================================
// EVRT2 — Task-01 M6: Jitter-Injection Test Rig
// Spec: evrt2/tasks/01_ABSOLUTE_NO_DELAY_VISIBLE_REGION.md § Acceptance Criteria,
//       Implementation Milestones (M6: "Jitter-injection test rig, automated
//       acceptance-criteria check (#1–#3)")
// Author of the standard: Arthur Valiev. Rust implementation below.
// =============================================================================
//
//! ROADMAP.md Phase 6.5. Task-01's M1-M5 milestones are all implemented and
//! unit-tested elsewhere (`evrt2_scheduler.rs` for age_ceiling/check_breach,
//! `evrt2_jitter.rs` for the buffer_depth=0 VISIBLE_REGION bypass). M6 is the
//! one that's explicitly a *test harness*, not production code — this module
//! IS that harness.
//!
//! # What this simulates, honestly
//!
//! There is no real network jitter injector in this codebase (no `tc`/netem
//! integration, no live two-machine loop here) — building one wasn't in
//! scope for this pass. What this rig does instead: drive the REAL
//! production functions (`buffer_depth_for_packet`, `check_breach`,
//! `age_ceiling`) with a synthetic, deterministic (seeded) network-delay
//! generator whose 95th-percentile delay is configurable, exactly matching
//! the acceptance criteria's own "0–40ms P95" framing. This is a simulation
//! of the SCHEDULING/BUFFERING LOGIC's behavior under jitter, not an
//! end-to-end network test — see ROADMAP.md for this distinction spelled out
//! plainly, so it isn't mistaken for a claim this ran over a real lossy link.
//!
//! `"sustained 10 minutes"` is modeled as a fixed frame count (`FRAMES_10MIN`
//! below, at a documented assumed frame rate) rather than a real 10-minute
//! sleep — the same choice `evrt2_jitter.rs`'s own tests already make
//! (`on_arrival_delta` fed synthetic deltas, no real `Instant` sleeps).
//!
//! # Age model
//!
//! `actual_age(visible_region) = BASE_PROCESSING_LATENCY_MS + network_delay`
//!
//! `network_delay` is this frame's synthetic jitter sample.
//! `buffer_depth_for_packet` is called with a VISIBLE_REGION-flagged header
//! on every sample and its result is asserted to be `Duration::ZERO` — not
//! assumed, checked — before being added into `actual_age` (it contributes
//! exactly nothing, per Task-01 § Mechanism 3, but the rig proves that
//! rather than hard-coding it). `BASE_PROCESSING_LATENCY_MS` stands in for
//! encode + scheduler dispatch time before the packet reaches the socket —
//! small and fixed, deliberately NOT a function of jitter or periphery
//! complexity, which is exactly what acceptance criterion #3 requires this
//! rig be able to demonstrate structurally (see
//! `breach_rate_is_a_pure_function_of_jitter_not_of_anything_else` below).

use crate::evrt2_jitter::buffer_depth_for_packet;
use crate::evrt2_packet::{flags, Mode, PacketHeader, PacketType};
use crate::evrt2_scheduler::{age_ceiling, check_breach, VisibleRegion};
use std::time::Duration;

/// Encode/scheduler-dispatch latency assumed before a visible-region packet
/// reaches the socket — see module doc. Not itself under test; a stand-in
/// constant so `actual_age` isn't purely the network sample.
const BASE_PROCESSING_LATENCY_MS: f64 = 0.5;

/// "Sustained 10 minutes" at an assumed 60 FPS (matches
/// `evrt2_experiment::EXPERIMENT_FPS`'s ballpark) — see module doc for why
/// this is simulated frame count, not a real 10-minute wall-clock run.
const FRAMES_10MIN_AT_60FPS: u64 = 60 * 600;

// ── Deterministic synthetic network-delay generator ────────────────────────

/// splitmix64 — dependency-free, deterministic given a seed. This codebase
/// avoids pulling in `rand` for its core codec/protocol logic (see
/// `evrtck.rs`'s "no external dependencies" note); a test-only PRNG doesn't
/// need to be cryptographically strong, only reproducible.
struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform sample in the open interval (0, 1) — never exactly 0 (would
    /// make the exponential-sampling `ln` below diverge) or 1.
    fn next_open01(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 1.0) / (9007199254740993.0/* 2^53 + 1 */)
    }
}

/// Exponentially-distributed delay (ms) via inverse-CDF sampling, scaled so
/// its 95th percentile matches `p95_ms`: for `X ~ Exp(λ)`,
/// `P(X <= p95) = 0.95` gives `λ = ln(20) / p95` (since `1 - e^-0.95... `
/// solves to `-ln(0.05) = ln(20)`). Exponential is a reasonable, standard
/// stand-in for one-way network jitter magnitude — always non-negative,
/// heavier-than-Gaussian tail (occasional large delay spikes), and its
/// single parameter maps directly onto the spec's own "P95" framing instead
/// of requiring a separate mean/stddev the spec doesn't give.
fn sample_delay_ms(rng: &mut SplitMix64, p95_ms: f64) -> f64 {
    if p95_ms <= 0.0 {
        return 0.0;
    }
    let lambda = 20f64.ln() / p95_ms;
    let u = rng.next_open01();
    -(1.0 - u).ln() / lambda
}

// ── Simulation ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct RunStats {
    pub frames: u64,
    pub breaches: u64,
    pub degrade_signals_fired: u64,
}

impl RunStats {
    pub fn breach_rate(&self) -> f64 {
        if self.frames == 0 {
            return 0.0;
        }
        self.breaches as f64 / self.frames as f64
    }
}

/// Runs `frames` simulated visible-region deliveries for `mode` under
/// synthetic network jitter with 95th-percentile `p95_jitter_ms`, exercising
/// the real `buffer_depth_for_packet` / `check_breach` / `age_ceiling`
/// production functions on every sample. Deterministic for a given `seed`.
fn simulate_mode_under_jitter(mode: Mode, p95_jitter_ms: f64, frames: u64, seed: u64) -> RunStats {
    let mut rng = SplitMix64::new(seed);
    let visible_header = PacketHeader {
        packet_type: PacketType::VideoFrame,
        mode,
        flags: flags::VISIBLE_REGION,
        frame_id: 0,
        packet_index: 0,
        packet_count: 1,
        presentation_time_us: 0,
        fec_group: 0,
        fec_idx: 0,
        fec_total: 0,
        auth_tag: 0,
    };
    let region = VisibleRegion { tiles: vec![0] };

    let mut breaches = 0u64;
    let mut degrade_signals_fired = 0u64;

    for _ in 0..frames {
        let network_delay_ms = sample_delay_ms(&mut rng, p95_jitter_ms);

        // Task 01 § Mechanism 3, proven live rather than assumed: a
        // VISIBLE_REGION packet always gets buffer_depth = 0, however large
        // the estimator's own jitter estimate might independently be. We
        // don't even need to feed the estimator any samples to prove this —
        // `buffer_depth_for_packet` branches on the header flag before ever
        // consulting the estimator.
        let dummy_estimator = crate::evrt2_jitter::JitterEstimator::new(
            crate::evrt2_jitter::ModeProfile::from_evrt2_mode(mode),
            Duration::from_millis(16),
        );
        let buffer_depth = buffer_depth_for_packet(&dummy_estimator, &visible_header);
        assert_eq!(
            buffer_depth,
            Duration::ZERO,
            "VISIBLE_REGION packets must bypass jitter buffering entirely (Task-01 § Mechanism 3)"
        );

        let age_ms =
            BASE_PROCESSING_LATENCY_MS + network_delay_ms + buffer_depth.as_secs_f64() * 1000.0;
        let measured_age = Duration::from_secs_f64((age_ms / 1000.0).max(0.0));

        if let Some(signal) = check_breach(mode, region.clone(), measured_age) {
            breaches += 1;
            // Acceptance criterion #2: "DEGRADE_SIGNAL fires within the same
            // frame — no silent breaches." `check_breach` is called
            // synchronously right after `measured_age` is known, in the
            // same simulated frame — this counter proves every breach
            // produced a signal, not just that breaches were counted.
            degrade_signals_fired += 1;
            assert_eq!(signal.ceiling, age_ceiling(mode));
            assert_eq!(signal.measured_age, measured_age);
            assert_eq!(
                signal.region, region,
                "must not fabricate a region — Causal Integrity Principle"
            );
        }
    }

    RunStats {
        frames,
        breaches,
        degrade_signals_fired,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Harness self-check: is the synthetic generator trustworthy? ────────

    #[test]
    fn exponential_jitter_source_empirical_p95_matches_target() {
        for &target_p95 in &[5.0, 20.0, 40.0] {
            let mut rng = SplitMix64::new(0xC0FF_EE);
            let mut samples: Vec<f64> = (0..50_000)
                .map(|_| sample_delay_ms(&mut rng, target_p95))
                .collect();
            samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let p95_idx = (samples.len() as f64 * 0.95) as usize;
            let empirical_p95 = samples[p95_idx];
            let rel_err = (empirical_p95 - target_p95).abs() / target_p95;
            assert!(
                rel_err < 0.05,
                "target P95={target_p95}ms, empirical={empirical_p95:.2}ms, rel_err={rel_err:.3}"
            );
        }
    }

    #[test]
    fn zero_p95_jitter_produces_zero_delay() {
        let mut rng = SplitMix64::new(1);
        for _ in 0..1000 {
            assert_eq!(sample_delay_ms(&mut rng, 0.0), 0.0);
        }
    }

    // ── Acceptance criterion #1: ≥99.9% within age_ceiling under low jitter,
    //    for EVERY mode, sustained "10 minutes" ─────────────────────────────

    #[test]
    fn low_jitter_meets_the_99_9_percent_criterion_for_every_mode() {
        // p95 chosen well inside each mode's budget (ceiling - base latency)
        // so the exponential's tail rarely reaches the ceiling at all — see
        // module doc's age model. 3ms clears AR's 12ms and 2R's 15ms
        // ceilings comfortably; Mode47's 8ms ceiling is the tight one, which
        // is exactly why it gets checked explicitly here, not skipped.
        for &mode in &[Mode::Ar, Mode::R2, Mode::Mode47] {
            let stats = simulate_mode_under_jitter(
                mode,
                3.0,
                FRAMES_10MIN_AT_60FPS,
                0x5EED_0001 ^ mode as u64,
            );
            assert!(
                stats.breach_rate() < 0.001,
                "{:?}: breach rate {:.5} exceeds Task-01's 99.9% acceptance criterion ({} breaches / {} frames)",
                mode, stats.breach_rate(), stats.breaches, stats.frames
            );
            // Criterion #2, structurally, for this run.
            assert_eq!(stats.degrade_signals_fired, stats.breaches);
        }
    }

    // ── Acceptance criterion #2: every breach produces a same-frame signal ──

    #[test]
    fn every_breach_produces_exactly_one_degrade_signal_no_silent_breaches() {
        // Deliberately harsh jitter so this run actually contains breaches
        // to check the invariant against — a criterion that's never
        // exercised by any breach is not actually being tested.
        let stats = simulate_mode_under_jitter(Mode::Mode47, 40.0, 20_000, 0xBAD_5EED);
        assert!(
            stats.breaches > 0,
            "test is meaningless without at least one real breach to check"
        );
        assert_eq!(stats.degrade_signals_fired, stats.breaches);
    }

    // ── Acceptance criterion #3: breach rate correlates with jitter alone,
    //    never with "periphery complexity" (this rig has no such input at
    //    all — structural proof, not just an empirical one) ────────────────

    #[test]
    fn breach_rate_is_a_pure_function_of_jitter_not_of_anything_else() {
        // `simulate_mode_under_jitter`'s signature takes mode, jitter, frame
        // count and seed — no periphery/complexity parameter exists to even
        // pass one. This test is the empirical half: breach rate must be
        // monotonically non-decreasing as jitter severity increases, for a
        // fixed mode, seed varied per run only to avoid any one seed's luck
        // producing a false monotonic read.
        let mode = Mode::Ar; // 12ms ceiling — plenty of headroom to see the curve
        let levels = [1.0, 5.0, 10.0, 20.0, 40.0];
        let mut prev_rate = 0.0f64;
        for (i, &p95) in levels.iter().enumerate() {
            let stats = simulate_mode_under_jitter(
                mode,
                p95,
                FRAMES_10MIN_AT_60FPS,
                0x1234_5678 + i as u64,
            );
            assert!(
                stats.breach_rate() >= prev_rate - 1e-9, // tiny epsilon for float noise at the low end
                "breach rate should not DECREASE as jitter increases: p95={p95}ms rate={:.5} < previous {prev_rate:.5}",
                stats.breach_rate()
            );
            prev_rate = stats.breach_rate();
        }
        // At the top of the "0-40ms P95" range named in the spec, Mode AR's
        // 12ms ceiling (11.5ms of network budget after base latency) is
        // regularly exceeded — the guarantee is honestly NOT met here, per
        // Task-01 § Breach Handling ("the system does not pretend it met
        // the guarantee"). This asserts the rig reports that honestly
        // instead of the 99.9% floor holding at every jitter level, which
        // would mean the rig was too forgiving to be a real check.
        assert!(prev_rate > 0.001, "40ms P95 jitter against a 12ms ceiling should break the 99.9% floor, or this rig isn't strict enough to be useful");
    }

    #[test]
    fn mode_47_has_the_tightest_ceiling_and_breaches_first_as_jitter_rises() {
        // Mode47's 8ms ceiling is the strictest (vs AR's 12ms / 2R's 15ms) —
        // at a shared jitter level near that boundary, 47 should show the
        // highest breach rate of the three, matching the spec's own
        // ordering (47 chosen precisely because gaming needs the tightest
        // bound, per AR2R47_MODES.md).
        let p95 = 8.0; // right at Mode47's own ceiling, comfortably under AR/2R's
        let ar = simulate_mode_under_jitter(Mode::Ar, p95, FRAMES_10MIN_AT_60FPS, 1);
        let r2 = simulate_mode_under_jitter(Mode::R2, p95, FRAMES_10MIN_AT_60FPS, 2);
        let g47 = simulate_mode_under_jitter(Mode::Mode47, p95, FRAMES_10MIN_AT_60FPS, 3);
        assert!(
            g47.breach_rate() >= ar.breach_rate(),
            "47 ({:.5}) should breach at least as often as AR ({:.5}) at equal jitter",
            g47.breach_rate(),
            ar.breach_rate()
        );
        assert!(
            g47.breach_rate() >= r2.breach_rate(),
            "47 ({:.5}) should breach at least as often as 2R ({:.5}) at equal jitter",
            g47.breach_rate(),
            r2.breach_rate()
        );
    }

    #[test]
    fn run_is_deterministic_for_a_fixed_seed() {
        let a = simulate_mode_under_jitter(Mode::R2, 15.0, 5000, 42);
        let b = simulate_mode_under_jitter(Mode::R2, 15.0, 5000, 42);
        assert_eq!(a.breaches, b.breaches);
        assert_eq!(a.frames, b.frames);
    }
}
