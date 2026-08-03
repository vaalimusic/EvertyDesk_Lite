// =============================================================================
// EVRT2 — RTT measurement + path degradation detection
// Spec: evrt2/transport/SDUDP.md § 6 Path Probing
// Author of the standard: Arthur Valiev. Rust implementation below.
// =============================================================================
//
//! ROADMAP.md Phase 5.4. SDUDP.md § 6 says the primary path should be
//! abandoned for another candidate when it "degrades > 3× RTT increase" —
//! this module is the part that decides *when* that's true. It has no idea
//! what a path even is; it only turns a stream of RTT samples into an
//! edge-triggered "yes, switch now" signal. The actual RTT samples come
//! from `evrt2_session::{build_keepalive_ping, parse_keepalive_ping}` (a
//! KEEPALIVE round trip); the actual switching (re-race candidates, adopt
//! a new `Evrt2Session`) is `evrt2_experiment.rs`'s job.
//!
//! This replaces a gap that was flagged honestly since Phase 5.2/5.3:
//! `ReceiverFeedback2.rtt_us` was always sent as `0` because "no RTT probe
//! implemented for this experimental path yet". It's real now.

use std::time::Duration;

/// How many early samples establish the baseline RTT this path is judged
/// against. Small enough that baseline is ready within a few seconds of
/// connecting; large enough that one lucky/unlucky first ping doesn't
/// become the whole baseline.
const BASELINE_SAMPLES: usize = 5;

/// SDUDP.md § 6's own number: "> 3× RTT increase".
const DEGRADATION_FACTOR: f32 = 3.0;

/// Require the smoothed RTT to stay above the degradation threshold for
/// this many consecutive samples before triggering — a single spike
/// (a GC pause, a WiFi retransmit) is normal jitter, not a dead path, and
/// shouldn't tear down a working session over it.
const CONSECUTIVE_BREACHES_REQUIRED: u32 = 3;

/// EMA smoothing factor for the "current" RTT estimate — same shape as
/// `evrt2_jitter::JitterEstimator`'s own smoothing, kept separate because
/// RTT and inter-packet jitter are different signals measured differently
/// (ping/pong round trip vs. one-way packet arrival spacing).
const EMA_ALPHA: f32 = 0.3;

/// Tracks RTT samples for one path and decides when it has genuinely
/// degraded relative to how it started out — not relative to some fixed
/// global threshold, since "good" RTT is completely different for a LAN
/// candidate (sub-millisecond) than a relay-tunneled WAN candidate
/// (tens of milliseconds are normal there).
pub struct RttEstimator {
    baseline_us: Option<f32>,
    samples_for_baseline: Vec<u32>,
    ema_us: f32,
    consecutive_breaches: u32,
}

impl RttEstimator {
    pub fn new() -> Self {
        Self {
            baseline_us: None,
            samples_for_baseline: Vec::new(),
            ema_us: 0.0,
            consecutive_breaches: 0,
        }
    }

    /// Feed one fresh RTT sample (a completed ping/pong round trip).
    /// Returns `true` exactly once, on the sample that completes
    /// `CONSECUTIVE_BREACHES_REQUIRED` — edge-triggered so a caller acting
    /// on this (re-racing candidates) does it once per degradation event,
    /// not once per sample for as long as the path stays bad.
    pub fn on_sample(&mut self, rtt: Duration) -> bool {
        let us = rtt.as_micros() as f32;
        // EMA is purely for `current()` reporting — deliberately NOT used
        // for the breach check below. Feeding a smoothed value into a
        // "consecutive breaches" counter double-applies hysteresis: the EMA
        // itself lags behind a real recovery, so the counter would keep
        // seeing "still degraded" for several samples after the path
        // actually recovered. The breach check uses the raw sample so
        // "3 consecutive genuinely bad round trips" means exactly that.
        self.ema_us = if self.ema_us == 0.0 {
            us
        } else {
            EMA_ALPHA * us + (1.0 - EMA_ALPHA) * self.ema_us
        };

        let Some(baseline) = self.baseline_us else {
            self.samples_for_baseline.push(us as u32);
            if self.samples_for_baseline.len() >= BASELINE_SAMPLES {
                let sum: u32 = self.samples_for_baseline.iter().sum();
                self.baseline_us = Some(sum as f32 / self.samples_for_baseline.len() as f32);
            }
            return false;
        };

        if us > baseline * DEGRADATION_FACTOR {
            self.consecutive_breaches += 1;
            self.consecutive_breaches == CONSECUTIVE_BREACHES_REQUIRED
        } else {
            self.consecutive_breaches = 0;
            false
        }
    }

    pub fn baseline(&self) -> Option<Duration> {
        self.baseline_us.map(|us| Duration::from_micros(us as u64))
    }

    pub fn current(&self) -> Duration {
        Duration::from_micros(self.ema_us as u64)
    }

    /// Call once a path switch has actually happened — re-arms baseline
    /// collection so the estimator judges the NEW path against its own
    /// early samples, not forever against the old (degraded) path's
    /// baseline. Without this, a switch to a path with a genuinely higher
    /// but stable RTT (e.g. LAN → relay/WAN) would immediately look
    /// "degraded" again and the estimator would demand yet another switch.
    pub fn reset_baseline(&mut self) {
        self.baseline_us = None;
        self.samples_for_baseline.clear();
        self.consecutive_breaches = 0;
        self.ema_us = 0.0;
    }
}

impl Default for RttEstimator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_trigger_before_baseline_is_established() {
        let mut est = RttEstimator::new();
        for _ in 0..(BASELINE_SAMPLES - 1) {
            assert!(!est.on_sample(Duration::from_millis(10)));
        }
        assert!(est.baseline().is_none());
    }

    #[test]
    fn baseline_is_the_average_of_the_first_samples() {
        let mut est = RttEstimator::new();
        let samples_ms = [8, 10, 12, 10, 10]; // avg = 10ms
        for &ms in &samples_ms {
            est.on_sample(Duration::from_millis(ms));
        }
        let baseline = est.baseline().expect("baseline should be established");
        assert_eq!(baseline, Duration::from_micros(10_000));
    }

    #[test]
    fn a_single_spike_does_not_trigger_a_switch() {
        let mut est = RttEstimator::new();
        for _ in 0..BASELINE_SAMPLES {
            est.on_sample(Duration::from_millis(10)); // baseline = 10ms
        }
        // One wildly slow sample (well over 3x) — EMA moves toward it but
        // consecutive_breaches only reaches 1, below the required 3.
        let triggered = est.on_sample(Duration::from_millis(100));
        assert!(!triggered, "a single spike must not trigger a path switch");
    }

    #[test]
    fn sustained_degradation_triggers_exactly_once() {
        let mut est = RttEstimator::new();
        for _ in 0..BASELINE_SAMPLES {
            est.on_sample(Duration::from_millis(10)); // baseline = 10ms, threshold = 30ms
        }
        let mut trigger_count = 0;
        for _ in 0..6 {
            if est.on_sample(Duration::from_millis(200)) {
                trigger_count += 1;
            }
        }
        assert_eq!(
            trigger_count, 1,
            "must trigger exactly once, not once per sample while still degraded"
        );
    }

    #[test]
    fn recovery_before_the_threshold_resets_the_breach_counter() {
        let mut est = RttEstimator::new();
        for _ in 0..BASELINE_SAMPLES {
            est.on_sample(Duration::from_millis(10));
        }
        assert!(!est.on_sample(Duration::from_millis(100)));
        assert!(!est.on_sample(Duration::from_millis(100)));
        // Recovers before the 3rd consecutive breach.
        assert!(!est.on_sample(Duration::from_millis(10)));
        // Two more bad samples — still shouldn't trigger, counter was reset.
        assert!(!est.on_sample(Duration::from_millis(100)));
        assert!(!est.on_sample(Duration::from_millis(100)));
    }

    #[test]
    fn reset_baseline_re_arms_detection_against_the_new_path() {
        let mut est = RttEstimator::new();
        for _ in 0..BASELINE_SAMPLES {
            est.on_sample(Duration::from_millis(10));
        }
        for _ in 0..CONSECUTIVE_BREACHES_REQUIRED {
            est.on_sample(Duration::from_millis(100));
        }
        assert!(est.baseline().is_some());

        est.reset_baseline();
        assert!(est.baseline().is_none());
        // The "new path" happens to have a stable 90ms RTT — must NOT be
        // judged against the old 10ms baseline anymore.
        let mut triggered_again = false;
        for _ in 0..(BASELINE_SAMPLES + CONSECUTIVE_BREACHES_REQUIRED as usize) {
            if est.on_sample(Duration::from_millis(90)) {
                triggered_again = true;
            }
        }
        assert!(
            !triggered_again,
            "a stable new path must not immediately re-trigger against a stale baseline"
        );
    }
}
