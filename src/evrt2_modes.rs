// =============================================================================
// EVRT2 — AR2R47 Mode Profiles + Transition State Machine
// Spec: evrt2/modes/AR2R47_MODES.md
// Author of the standard: Arthur Valiev. Rust implementation below.
// =============================================================================
//
//! Per-mode technical profiles (bandwidth, FPS, resolution, latency,
//! silicon requirement — the "Summary Table" in AR2R47_MODES.md) plus the
//! mode-transition state machine from the "Mode Transition Rules" diagram:
//!
//! ```text
//!           ┌─────────────────────────────────────────────┐
//!     ┌─────▼──────┐   motion↑          ┌────────────────▼──────┐
//!     │     AR     │ ─────────────────► │          2R           │
//!     │  (static)  │ ◄───────────────── │       (dynamic)       │
//!     └────────────┘   motion↓ 5s idle  └──────────┬────────────┘
//!                                                   │        ▲
//!                                          game↑   │        │  game↓
//!                                       silicon✓   │        │  or minimize
//!                                                   ▼        │
//!                                          ┌────────────────────────┐
//!                                          │           47           │
//!                                          │        (gaming)        │
//!                                          └────────────────────────┘
//! ```
//!
//! "All transitions are signaled via MODE_SWITCH packet. Client decoder is
//! mode-agnostic — receives any mode transparently." The MODE_SWITCH packet
//! itself is `evrt2_packet::PacketType::ModeSwitch`; this module decides
//! *when* to emit one, not how it's carried on the wire.

use crate::evrt2_fec::FecConfig;
use crate::evrt2_packet::Mode;
use std::time::Duration;

/// Technical profile for one mode — every field taken directly from the
/// "Summary Table" / per-mode "Technical profile" blocks in
/// AR2R47_MODES.md. Kept as plain data (not behavior) so callers
/// (encoder, bitrate controller, resolution negotiator) read one source of
/// truth instead of each hardcoding the table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModeProfile {
    pub mode: Mode,
    /// Bytes/second (the spec gives these in KB/s and MB/s — byte units,
    /// not bit units — so despite the field name suffix matching common
    /// "bps" networking shorthand, these are BYTES per second throughout,
    /// matching AR2R47_MODES.md's table exactly).
    pub bandwidth_min_bps: u32,
    /// `None` = uncapped (47 mode).
    pub bandwidth_max_bps: Option<u32>,
    pub max_fps: u32,
    pub max_resolution: (u32, u32),
    pub latency_target: Duration,
    pub lossless: bool,
    pub silicon_required: bool,
    /// XOR-delta tile engine (EVRTCK) participates in this mode's encoding.
    pub tile_engine_enabled: bool,
    pub fec: FecConfig,
}

impl ModeProfile {
    pub const AR: Self = Self {
        mode: Mode::Ar,
        bandwidth_min_bps: 10_000,          // 10 KB/s
        bandwidth_max_bps: Some(5_000_000), // 5 MB/s
        max_fps: 60,
        max_resolution: (1920, 1080),
        latency_target: Duration::from_millis(30),
        lossless: true,
        silicon_required: false,
        tile_engine_enabled: true,
        fec: FecConfig::AR,
    };

    pub const R2: Self = Self {
        mode: Mode::R2,
        bandwidth_min_bps: 200_000,         // 200 KB/s
        bandwidth_max_bps: Some(8_000_000), // 8 MB/s
        max_fps: 60,
        max_resolution: (2560, 1440),
        latency_target: Duration::from_millis(20),
        lossless: false,
        silicon_required: false,
        tile_engine_enabled: true, // "Partial" per summary table — static/low-motion regions
        fec: FecConfig::R2,
    };

    pub const MODE47: Self = Self {
        mode: Mode::Mode47,
        bandwidth_min_bps: 500_000,
        bandwidth_max_bps: None, // uncapped
        max_fps: 120,
        max_resolution: (3840, 2160),
        latency_target: Duration::from_millis(8),
        lossless: false,
        silicon_required: true,
        tile_engine_enabled: false, // "DISABLED" — pure silicon path
        fec: FecConfig::MODE47,
    };

    pub fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::Ar => Self::AR,
            Mode::R2 => Self::R2,
            Mode::Mode47 => Self::MODE47,
        }
    }
}

/// Why a mode transition happened — useful for logging/telemetry and for
/// populating a `MODE_SWITCH` packet's reason field (not itself specified
/// as a wire field in EVRT2_PACKET.md's draft, but named in prose in
/// AR2R47_MODES.md's "NO_SILICON" example — kept here as an enum so the
/// eventual wire encoding has a fixed, tested source of truth to encode
/// from, rather than inventing reason strings ad hoc at each call site).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchReason {
    MotionIncreased,
    MotionIdle,
    GameDetected,
    GameExitedOrMinimized,
    /// AR2R47_MODES.md § 47 requirements: "If no silicon encoder is found
    /// at session start: ... Session continues in 2R mode with SW
    /// fallback."
    NoSilicon,
    /// AR2R47_MODES.md § AR: "Network bandwidth < 200KB/s (forced to AR as
    /// only viable mode)".
    BandwidthForcedAr,
    /// Explicit user action ("User explicitly selects 'Game mode' in
    /// client UI").
    UserRequested,
}

impl SwitchReason {
    /// ROADMAP.md Phase 2.2: stable wire encoding for the MODE_SWITCH
    /// packet's reason byte. Not itself a wire field EVRT2_PACKET.md
    /// specifies at the byte level (only named in AR2R47_MODES.md's prose
    /// "NO_SILICON" example) — this is the first concrete encoding of it.
    pub fn to_wire_code(self) -> u8 {
        match self {
            SwitchReason::MotionIncreased => 1,
            SwitchReason::MotionIdle => 2,
            SwitchReason::GameDetected => 3,
            SwitchReason::GameExitedOrMinimized => 4,
            SwitchReason::NoSilicon => 5,
            SwitchReason::BandwidthForcedAr => 6,
            SwitchReason::UserRequested => 7,
        }
    }

    pub fn from_wire_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => SwitchReason::MotionIncreased,
            2 => SwitchReason::MotionIdle,
            3 => SwitchReason::GameDetected,
            4 => SwitchReason::GameExitedOrMinimized,
            5 => SwitchReason::NoSilicon,
            6 => SwitchReason::BandwidthForcedAr,
            7 => SwitchReason::UserRequested,
            _ => return None,
        })
    }
}

/// Live signals the mode selector reacts to — sampled by the caller (host
/// capture/encode pipeline) at whatever cadence makes sense (the spec
/// doesn't fix one; motion/idle are naturally per-frame, bandwidth/game
/// detection are naturally coarser).
#[derive(Debug, Clone, Copy)]
pub struct ModeSignals {
    /// Fraction of pixels changed this frame, 0.0–1.0. Drives AR↔2R and the
    /// 2R→47 motion>70% threshold.
    pub motion_ratio: f32,
    /// How long motion has stayed below the AR threshold, continuously.
    pub idle_duration: Duration,
    /// A game process is in the foreground (platform-reported or
    /// user-toggled "Game mode").
    pub game_detected: bool,
    /// A hardware encoder is available (Execution Capability probe —
    /// `execution_capability::CapabilityRegistry::providers_for(RoiEncoding)`
    /// being non-empty is the natural source for this in this codebase).
    pub silicon_available: bool,
    /// Current measured/estimated available bandwidth.
    pub bandwidth_bps: u32,
    /// User explicitly requested game mode via the client UI, overriding
    /// motion-based detection (AR2R47_MODES.md: "User explicitly selects
    /// 'Game mode' in client UI").
    pub user_requested_game_mode: bool,
}

/// Motion ratio above which 2R is considered "dynamic content" (lower bound
/// of the "30-70%" range in AR2R47_MODES.md § 2R).
const MOTION_THRESHOLD_AR_TO_2R: f32 = 0.30;
/// Motion ratio above which 47 mode is warranted (AR2R47_MODES.md § 47:
/// "Motion level >70% of pixels per frame").
const MOTION_THRESHOLD_2R_TO_47: f32 = 0.70;
/// "motion↓ 5s idle" in the transition diagram: 2R→AR requires motion below
/// threshold continuously for this long, not just a single quiet frame.
const IDLE_DURATION_FOR_AR: Duration = Duration::from_secs(5);
/// AR2R47_MODES.md § AR: "Network bandwidth < 200KB/s (forced to AR as only
/// viable mode)". `pub` so a real `bandwidth_bps` estimator (ROADMAP.md
/// Phase 2.4′, `evrt2_experiment.rs`) can compare against the exact same
/// threshold this rule uses, instead of duplicating the magic number.
pub const BANDWIDTH_FORCES_AR_BPS: u32 = 200_000;

/// Drives the AR↔2R↔47 state machine from live signals. Holds only the
/// current mode — every transition decision is a pure function of
/// (current_mode, signals), matching the diagram exactly (no history needed
/// beyond `idle_duration`, which the caller already tracks and passes in).
pub struct ModeSelector {
    current: Mode,
}

impl ModeSelector {
    pub fn new(initial: Mode) -> Self {
        Self { current: initial }
    }

    pub fn current(&self) -> Mode {
        self.current
    }

    pub fn current_profile(&self) -> ModeProfile {
        ModeProfile::for_mode(self.current)
    }

    /// Evaluate signals against the current mode and return `Some(new_mode,
    /// reason)` if a transition should happen, or `None` to stay. Does NOT
    /// mutate `self` — call `apply` (or construct a new selector with the
    /// returned mode) once the caller has actually sent the MODE_SWITCH
    /// packet, so the state machine never claims a transition that wasn't
    /// actually communicated to the peer.
    pub fn evaluate(&self, signals: &ModeSignals) -> Option<(Mode, SwitchReason)> {
        // Bandwidth floor is an override that applies regardless of current
        // mode — "forced to AR as only viable mode".
        if signals.bandwidth_bps < BANDWIDTH_FORCES_AR_BPS && self.current != Mode::Ar {
            return Some((Mode::Ar, SwitchReason::BandwidthForcedAr));
        }

        match self.current {
            Mode::Ar => {
                // Live-found bug: without this guard, sustained low
                // bandwidth AND sustained high motion together caused an
                // unbounded MODE_SWITCH oscillation — the bandwidth-floor
                // check above forces AR every time `evaluate` runs while
                // constrained, but this arm then immediately escalated
                // back to R2 on the very next call because it only looked
                // at motion, not at whether the reason it's in AR is still
                // in effect. AR is the floor mode BECAUSE bandwidth is
                // constrained; motion alone can't out-vote that until
                // bandwidth actually recovers.
                if signals.motion_ratio >= MOTION_THRESHOLD_AR_TO_2R
                    && signals.bandwidth_bps >= BANDWIDTH_FORCES_AR_BPS
                {
                    return Some((Mode::R2, SwitchReason::MotionIncreased));
                }
                None
            }
            Mode::R2 => {
                let wants_game = signals.user_requested_game_mode
                    || (signals.game_detected && signals.motion_ratio > MOTION_THRESHOLD_2R_TO_47);
                if wants_game {
                    if signals.silicon_available {
                        let reason = if signals.user_requested_game_mode {
                            SwitchReason::UserRequested
                        } else {
                            SwitchReason::GameDetected
                        };
                        return Some((Mode::Mode47, reason));
                    }
                    // "session falls back to 2R if no silicon found" — we're
                    // already in 2R, so this is a no-op transition (stay),
                    // not a Some(...) — nothing to signal since the mode
                    // isn't changing. The NoSilicon reason is surfaced by
                    // `evaluate` only when 47 was ACTIVE and silicon drops
                    // out mid-session — see the Mode47 arm below.
                    return None;
                }
                if signals.motion_ratio < MOTION_THRESHOLD_AR_TO_2R
                    && signals.idle_duration >= IDLE_DURATION_FOR_AR
                {
                    return Some((Mode::Ar, SwitchReason::MotionIdle));
                }
                None
            }
            Mode::Mode47 => {
                if !signals.silicon_available {
                    // AR2R47_MODES.md § 47: "Silicon: REQUIRED — no SW
                    // fallback in 47 mode (session falls back to 2R if no
                    // silicon found)". This covers silicon disappearing
                    // mid-session (thermal throttle, driver reset) just as
                    // much as it not being there at session start.
                    return Some((Mode::R2, SwitchReason::NoSilicon));
                }
                if !signals.game_detected && !signals.user_requested_game_mode {
                    // "game↓ or minimize" in the diagram.
                    return Some((Mode::R2, SwitchReason::GameExitedOrMinimized));
                }
                None
            }
        }
    }

    /// Commit a transition already decided by `evaluate` (and already
    /// signaled to the peer via a MODE_SWITCH packet).
    pub fn apply(&mut self, new_mode: Mode) {
        self.current = new_mode;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_signals() -> ModeSignals {
        ModeSignals {
            motion_ratio: 0.0,
            idle_duration: Duration::ZERO,
            game_detected: false,
            silicon_available: true,
            bandwidth_bps: 10_000_000,
            user_requested_game_mode: false,
        }
    }

    #[test]
    fn profiles_match_summary_table() {
        assert_eq!(ModeProfile::AR.max_fps, 60);
        assert_eq!(ModeProfile::AR.max_resolution, (1920, 1080));
        assert_eq!(ModeProfile::AR.latency_target, Duration::from_millis(30));
        assert!(ModeProfile::AR.lossless);
        assert!(!ModeProfile::AR.silicon_required);
        assert_eq!(ModeProfile::AR.bandwidth_min_bps, 10_000);
        assert_eq!(ModeProfile::AR.bandwidth_max_bps, Some(5_000_000));

        assert_eq!(ModeProfile::R2.max_resolution, (2560, 1440));
        assert_eq!(ModeProfile::R2.latency_target, Duration::from_millis(20));
        assert!(!ModeProfile::R2.lossless);
        assert_eq!(ModeProfile::R2.bandwidth_min_bps, 200_000);
        assert_eq!(ModeProfile::R2.bandwidth_max_bps, Some(8_000_000));

        assert_eq!(ModeProfile::MODE47.max_fps, 120);
        assert_eq!(ModeProfile::MODE47.max_resolution, (3840, 2160));
        assert_eq!(ModeProfile::MODE47.latency_target, Duration::from_millis(8));
        assert!(ModeProfile::MODE47.silicon_required);
        assert!(!ModeProfile::MODE47.tile_engine_enabled);
        assert_eq!(ModeProfile::MODE47.bandwidth_max_bps, None);
    }

    #[test]
    fn ar_transitions_to_2r_on_motion_increase() {
        let sel = ModeSelector::new(Mode::Ar);
        let mut sig = base_signals();
        sig.motion_ratio = 0.35;
        assert_eq!(
            sel.evaluate(&sig),
            Some((Mode::R2, SwitchReason::MotionIncreased))
        );
    }

    #[test]
    fn ar_stays_ar_below_motion_threshold() {
        let sel = ModeSelector::new(Mode::Ar);
        let mut sig = base_signals();
        sig.motion_ratio = 0.10;
        assert_eq!(sel.evaluate(&sig), None);
    }

    #[test]
    fn ar_does_not_bounce_to_2r_while_bandwidth_still_forces_ar() {
        // Live bug found during Phase 6.4/1.3 network testing: sustained low
        // bandwidth (BandwidthForcedAr) and sustained high motion together
        // used to cause an unbounded AR<->R2 MODE_SWITCH flap — the
        // bandwidth-floor check forced AR every call, then this arm
        // immediately escalated back to R2 next call because it only
        // checked motion. AR must stay AR while bandwidth is still below
        // the floor, no matter how high motion is.
        let sel = ModeSelector::new(Mode::Ar);
        let mut sig = base_signals();
        sig.motion_ratio = 0.90; // well above MOTION_THRESHOLD_AR_TO_2R
        sig.bandwidth_bps = BANDWIDTH_FORCES_AR_BPS - 1; // still constrained
        assert_eq!(sel.evaluate(&sig), None);
    }

    #[test]
    fn ar_transitions_to_2r_once_bandwidth_recovers_even_with_same_high_motion() {
        // Companion to the bounce-guard above: once bandwidth genuinely
        // recovers, the same high motion that was blocked before must be
        // able to escalate normally — the guard isn't a permanent latch.
        let sel = ModeSelector::new(Mode::Ar);
        let mut sig = base_signals();
        sig.motion_ratio = 0.90;
        sig.bandwidth_bps = BANDWIDTH_FORCES_AR_BPS;
        assert_eq!(
            sel.evaluate(&sig),
            Some((Mode::R2, SwitchReason::MotionIncreased))
        );
    }

    #[test]
    fn r2_transitions_to_ar_only_after_5s_sustained_idle() {
        let sel = ModeSelector::new(Mode::R2);
        let mut sig = base_signals();
        sig.motion_ratio = 0.05;
        sig.idle_duration = Duration::from_secs(2);
        assert_eq!(
            sel.evaluate(&sig),
            None,
            "must not drop to AR before 5s idle"
        );

        sig.idle_duration = Duration::from_secs(5);
        assert_eq!(
            sel.evaluate(&sig),
            Some((Mode::Ar, SwitchReason::MotionIdle))
        );
    }

    #[test]
    fn r2_transitions_to_47_when_game_and_silicon_present() {
        let sel = ModeSelector::new(Mode::R2);
        let mut sig = base_signals();
        sig.game_detected = true;
        sig.motion_ratio = 0.80;
        sig.silicon_available = true;
        assert_eq!(
            sel.evaluate(&sig),
            Some((Mode::Mode47, SwitchReason::GameDetected))
        );
    }

    #[test]
    fn r2_stays_r2_when_game_detected_but_no_silicon() {
        // AR2R47_MODES.md: "If no silicon encoder is found... Session
        // continues in 2R mode with SW fallback" — must not transition.
        let sel = ModeSelector::new(Mode::R2);
        let mut sig = base_signals();
        sig.game_detected = true;
        sig.motion_ratio = 0.80;
        sig.silicon_available = false;
        assert_eq!(sel.evaluate(&sig), None);
    }

    #[test]
    fn user_requested_game_mode_overrides_motion_heuristic() {
        let sel = ModeSelector::new(Mode::R2);
        let mut sig = base_signals();
        sig.user_requested_game_mode = true;
        sig.motion_ratio = 0.05; // low motion, would never auto-trigger 47
        assert_eq!(
            sel.evaluate(&sig),
            Some((Mode::Mode47, SwitchReason::UserRequested))
        );
    }

    #[test]
    fn mode47_falls_back_to_2r_when_silicon_disappears_mid_session() {
        let sel = ModeSelector::new(Mode::Mode47);
        let mut sig = base_signals();
        sig.game_detected = true;
        sig.silicon_available = false;
        assert_eq!(
            sel.evaluate(&sig),
            Some((Mode::R2, SwitchReason::NoSilicon))
        );
    }

    #[test]
    fn mode47_falls_back_to_2r_on_game_exit() {
        let sel = ModeSelector::new(Mode::Mode47);
        let mut sig = base_signals();
        sig.game_detected = false;
        sig.user_requested_game_mode = false;
        assert_eq!(
            sel.evaluate(&sig),
            Some((Mode::R2, SwitchReason::GameExitedOrMinimized))
        );
    }

    #[test]
    fn mode47_stays_while_game_active_and_silicon_healthy() {
        let sel = ModeSelector::new(Mode::Mode47);
        let mut sig = base_signals();
        sig.game_detected = true;
        sig.silicon_available = true;
        assert_eq!(sel.evaluate(&sig), None);
    }

    #[test]
    fn low_bandwidth_forces_ar_from_any_mode() {
        for start in [Mode::R2, Mode::Mode47] {
            let sel = ModeSelector::new(start);
            let mut sig = base_signals();
            sig.bandwidth_bps = 150_000; // < 200KB/s floor
            sig.game_detected = true;
            sig.silicon_available = true;
            assert_eq!(
                sel.evaluate(&sig),
                Some((Mode::Ar, SwitchReason::BandwidthForcedAr)),
                "starting from {start:?}"
            );
        }
    }

    #[test]
    fn switch_reason_wire_codes_roundtrip() {
        for reason in [
            SwitchReason::MotionIncreased,
            SwitchReason::MotionIdle,
            SwitchReason::GameDetected,
            SwitchReason::GameExitedOrMinimized,
            SwitchReason::NoSilicon,
            SwitchReason::BandwidthForcedAr,
            SwitchReason::UserRequested,
        ] {
            let code = reason.to_wire_code();
            assert_eq!(SwitchReason::from_wire_code(code), Some(reason));
        }
        assert_eq!(SwitchReason::from_wire_code(0), None);
        assert_eq!(SwitchReason::from_wire_code(255), None);
    }

    #[test]
    fn apply_commits_the_transition() {
        let mut sel = ModeSelector::new(Mode::Ar);
        let sig = ModeSignals {
            motion_ratio: 0.5,
            ..base_signals()
        };
        let (next, _) = sel.evaluate(&sig).unwrap();
        sel.apply(next);
        assert_eq!(sel.current(), Mode::R2);
    }

    #[test]
    fn evaluate_does_not_mutate_state() {
        let sel = ModeSelector::new(Mode::Ar);
        let sig = ModeSignals {
            motion_ratio: 0.9,
            ..base_signals()
        };
        let _ = sel.evaluate(&sig);
        assert_eq!(sel.current(), Mode::Ar, "evaluate must be side-effect-free");
    }
}
