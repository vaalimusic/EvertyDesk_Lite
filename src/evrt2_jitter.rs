// =============================================================================
// EVRT2 — Adaptive Jitter Buffer
// Spec: evrt2/transport/SDUDP.md § 2. Jitter Buffer (Adaptive)
// Spec: evrt2/tasks/01_ABSOLUTE_NO_DELAY_VISIBLE_REGION.md § Mechanism, 3. Wire
//       signal — "The client's jitter buffer treats VISIBLE_REGION packets
//       with buffer_depth = 0 — no buffering delay is applied."
// Author of the standard: Arthur Valiev. Rust implementation below.
// =============================================================================
//
//! Receiver-side jitter estimation and adaptive buffer depth, per SDUDP.md:
//!
//! ```text
//! jitter_p95 = EMA(abs(arrival_delta - expected_delta), alpha=0.1)
//! buffer_depth = max(4ms, min(50ms, jitter_p95 × 1.5))
//! In 47 mode: buffer_depth = max(2ms, jitter_p95 × 1.2) — more aggressive.
//! ```
//!
//! This module tracks packet arrival timing and produces a `buffer_depth`
//! recommendation the receive pipeline should honor before releasing a
//! frame to the decoder — except for `VISIBLE_REGION`-flagged packets
//! (EVRT2CKMAX-TASK-01), which bypass buffering entirely by design: M4 in
//! the task's implementation milestones.
//!
//! `EMA(x, alpha)` here is the textbook exponential moving average: an
//! approximation of the true P95 percentile, not the percentile itself —
//! the spec names it "jitter_p95" but defines it via a single EMA rather
//! than a real windowed percentile estimator. This module implements
//! exactly the formula as specified (EMA of absolute jitter), including
//! that naming; a true streaming P95 (e.g. via a t-digest) would be a
//! stricter but spec-diverging alternative and is deliberately not
//! substituted in here.

use std::time::Duration;

/// EMA smoothing factor from the spec: `alpha=0.1`.
const JITTER_EMA_ALPHA: f32 = 0.1;

/// How often the EMA-derived buffer_depth recommendation should be
/// recomputed/published, per spec: "updated every 500ms".
pub const UPDATE_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeProfile {
    /// AR/2R share the same buffer formula per SDUDP.md (only 47 differs).
    Standard,
    /// 47 — more aggressive: `max(2ms, jitter_p95 × 1.2)`.
    Mode47,
}

impl ModeProfile {
    pub fn from_evrt2_mode(mode: crate::evrt2_packet::Mode) -> Self {
        use crate::evrt2_packet::Mode;
        match mode {
            Mode::Ar | Mode::R2 => Self::Standard,
            Mode::Mode47 => Self::Mode47,
        }
    }
}

/// Tracks inter-arrival jitter and derives the buffer depth the receive
/// pipeline should apply to non-visible-region packets.
pub struct JitterEstimator {
    mode: ModeProfile,
    /// Expected inter-arrival delta, derived from the sender's declared
    /// frame cadence (set via `set_expected_interval`, e.g. 1000/fps ms).
    expected_delta: Duration,
    /// Running EMA of `|arrival_delta - expected_delta|`, in microseconds.
    jitter_p95_ema_us: f32,
    last_arrival: Option<std::time::Instant>,
    samples_seen: u64,
}

impl JitterEstimator {
    pub fn new(mode: ModeProfile, expected_interval: Duration) -> Self {
        Self {
            mode,
            expected_delta: expected_interval,
            jitter_p95_ema_us: 0.0,
            last_arrival: None,
            samples_seen: 0,
        }
    }

    pub fn set_expected_interval(&mut self, interval: Duration) {
        self.expected_delta = interval;
    }

    /// ROADMAP.md Phase 2.2: switch the buffer-depth formula (Standard vs
    /// Mode47) when a MODE_SWITCH changes which mode is active — the
    /// accumulated jitter EMA is kept (it's still a valid estimate of this
    /// network path's jitter regardless of which mode is streaming over
    /// it), only the depth *formula* applied to that estimate changes.
    pub fn set_mode(&mut self, mode: ModeProfile) {
        self.mode = mode;
    }

    /// Feed one packet arrival (wall-clock `now`, or a fixed clock in
    /// tests). Updates the EMA. First call only seeds `last_arrival`
    /// (there is no delta to measure yet).
    pub fn on_packet_arrival(&mut self, now: std::time::Instant) {
        if let Some(prev) = self.last_arrival {
            let arrival_delta = now.saturating_duration_since(prev);
            let diff_us =
                (arrival_delta.as_micros() as f32 - self.expected_delta.as_micros() as f32).abs();
            self.jitter_p95_ema_us += JITTER_EMA_ALPHA * (diff_us - self.jitter_p95_ema_us);
            self.samples_seen += 1;
        }
        self.last_arrival = Some(now);
    }

    /// Same as `on_packet_arrival` but takes the delta directly — convenient
    /// for tests and for callers that already compute inter-arrival time
    /// from packet-level timestamps rather than wall-clock `Instant`s.
    pub fn on_arrival_delta(&mut self, arrival_delta: Duration) {
        let diff_us =
            (arrival_delta.as_micros() as f32 - self.expected_delta.as_micros() as f32).abs();
        self.jitter_p95_ema_us += JITTER_EMA_ALPHA * (diff_us - self.jitter_p95_ema_us);
        self.samples_seen += 1;
    }

    pub fn jitter_p95(&self) -> Duration {
        Duration::from_micros(self.jitter_p95_ema_us.max(0.0) as u64)
    }

    /// The buffer_depth recommendation, per the mode-specific formula.
    /// This is what the receive pipeline should hold non-visible-region
    /// packets for before releasing them to the decoder.
    pub fn buffer_depth(&self) -> Duration {
        let jitter_ms = self.jitter_p95().as_secs_f32() * 1000.0;
        let ms = match self.mode {
            ModeProfile::Standard => (jitter_ms * 1.5).clamp(4.0, 50.0),
            ModeProfile::Mode47 => (jitter_ms * 1.2).max(2.0),
        };
        Duration::from_secs_f32(ms / 1000.0)
    }

    pub fn samples_seen(&self) -> u64 {
        self.samples_seen
    }
}

/// EVRT2CKMAX-TASK-01 § Wire signal: the buffer_depth to apply for a given
/// packet, taking the VISIBLE_REGION bypass into account. This is the
/// actual decision point the receive pipeline calls per-packet — never
/// call `JitterEstimator::buffer_depth()` directly for a packet without
/// first checking this.
///
/// > "The client's jitter buffer treats VISIBLE_REGION packets with
/// > buffer_depth = 0 — no buffering delay is applied, they are decoded
/// > and rendered as soon as they arrive, ahead of buffer-depth rules that
/// > apply to the rest of the frame."
pub fn buffer_depth_for_packet(
    estimator: &JitterEstimator,
    header: &crate::evrt2_packet::PacketHeader,
) -> Duration {
    if header.is_visible_region() {
        Duration::ZERO
    } else {
        estimator.buffer_depth()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evrt2_packet::{flags, Mode, PacketHeader, PacketType};

    fn header_with_flags(flags_value: u16) -> PacketHeader {
        PacketHeader {
            packet_type: PacketType::VideoFrame,
            mode: Mode::R2,
            flags: flags_value,
            frame_id: 1,
            packet_index: 0,
            packet_count: 1,
            presentation_time_us: 0,
            fec_group: 0,
            fec_idx: 0,
            fec_total: 0,
            auth_tag: 0,
        }
    }

    #[test]
    fn perfectly_regular_arrivals_converge_to_near_zero_jitter() {
        let interval = Duration::from_millis(16); // ~60fps
        let mut est = JitterEstimator::new(ModeProfile::Standard, interval);
        for _ in 0..200 {
            est.on_arrival_delta(interval);
        }
        assert!(
            est.jitter_p95() < Duration::from_micros(50),
            "got {:?}",
            est.jitter_p95()
        );
    }

    #[test]
    fn bursty_arrivals_increase_estimated_jitter() {
        let interval = Duration::from_millis(16);
        let mut est = JitterEstimator::new(ModeProfile::Standard, interval);
        // Alternate fast/slow arrivals — sustained jitter, not a one-off spike.
        for i in 0..200 {
            let delta = if i % 2 == 0 {
                Duration::from_millis(4)
            } else {
                Duration::from_millis(28)
            };
            est.on_arrival_delta(delta);
        }
        assert!(
            est.jitter_p95() > Duration::from_millis(5),
            "got {:?}",
            est.jitter_p95()
        );
    }

    #[test]
    fn buffer_depth_clamped_to_spec_range_standard_mode() {
        let interval = Duration::from_millis(16);
        let mut est = JitterEstimator::new(ModeProfile::Standard, interval);
        // Zero jitter → still clamped to the 4ms floor.
        for _ in 0..50 {
            est.on_arrival_delta(interval);
        }
        assert_eq!(est.buffer_depth(), Duration::from_millis(4));

        // Enormous jitter → clamped to the 50ms ceiling (float round-trip
        // through seconds can land a few nanoseconds off exact 50ms).
        let mut est2 = JitterEstimator::new(ModeProfile::Standard, interval);
        for _ in 0..50 {
            est2.on_arrival_delta(Duration::from_millis(500));
        }
        let depth = est2.buffer_depth();
        assert!(
            depth >= Duration::from_millis(50) && depth < Duration::from_millis(51),
            "got {depth:?}"
        );
    }

    #[test]
    fn mode_47_uses_more_aggressive_floor_and_multiplier() {
        let interval = Duration::from_millis(8); // 120fps
        let mut est = JitterEstimator::new(ModeProfile::Mode47, interval);
        for _ in 0..50 {
            est.on_arrival_delta(interval);
        }
        // Zero jitter → 2ms floor (vs 4ms for AR/2R).
        assert_eq!(est.buffer_depth(), Duration::from_millis(2));
    }

    #[test]
    fn visible_region_packets_bypass_buffering_entirely() {
        let interval = Duration::from_millis(16);
        let mut est = JitterEstimator::new(ModeProfile::Standard, interval);
        for _ in 0..50 {
            est.on_arrival_delta(Duration::from_millis(200)); // huge jitter
        }
        assert!(est.buffer_depth() > Duration::ZERO);

        let visible_header = header_with_flags(flags::VISIBLE_REGION);
        assert_eq!(
            buffer_depth_for_packet(&est, &visible_header),
            Duration::ZERO
        );

        let normal_header = header_with_flags(flags::IS_KEYFRAME);
        assert_eq!(
            buffer_depth_for_packet(&est, &normal_header),
            est.buffer_depth()
        );
    }

    #[test]
    fn mode_profile_maps_from_evrt2_mode_correctly() {
        assert_eq!(
            ModeProfile::from_evrt2_mode(Mode::Ar),
            ModeProfile::Standard
        );
        assert_eq!(
            ModeProfile::from_evrt2_mode(Mode::R2),
            ModeProfile::Standard
        );
        assert_eq!(
            ModeProfile::from_evrt2_mode(Mode::Mode47),
            ModeProfile::Mode47
        );
    }

    #[test]
    fn first_arrival_seeds_without_producing_a_spurious_sample() {
        let mut est = JitterEstimator::new(ModeProfile::Standard, Duration::from_millis(16));
        let t0 = std::time::Instant::now();
        est.on_packet_arrival(t0);
        assert_eq!(est.samples_seen(), 0);
        est.on_packet_arrival(t0 + Duration::from_millis(16));
        assert_eq!(est.samples_seen(), 1);
    }
}
