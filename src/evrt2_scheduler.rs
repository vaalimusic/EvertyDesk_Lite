// =============================================================================
// EVRT2 — Packet Scheduler + EVRT2CKMAX-TASK-01 (Visible Region guarantee)
// Spec: evrt2/tasks/01_ABSOLUTE_NO_DELAY_VISIBLE_REGION.md § Mechanism,
//       Breach Handling, Definition: Visible Region
// Spec: evrt2/transport/SDUDP.md § 1. Packet Scheduler
// Author of the standard: Arthur Valiev. Rust implementation below.
// =============================================================================
//
//! Two things live here, both from Task 01's implementation milestones:
//!
//! - **M3** — send order: visible-region slices first, then IDR, then the
//!   rest in priority order, then FEC repair; and the "drop, don't delay"
//!   congestion rule.
//! - **M5** — `DEGRADE_SIGNAL` emission when the age ceiling is breached
//!   anyway (Breach Handling).
//!
//! **Honest scope note on "Visible Region" computation:** the task doc
//! defines two sources — "explicit client focus (cursor bounding box,
//! active input target)" or, failing that, "top-percentile tiles from the
//! Attention Map." This codebase has no Attention Map (no gaze model, no
//! game-engine semantic hints — see the scoping discussion for Task 01/02
//! in this session: EVRT2CKMAX.md's `P_i` formula needs signal sources a
//! general remote-desktop product doesn't have). Only the **explicit
//! focus** path is implemented here — the same cursor position already fed
//! into EVRTCK v2's tile-priority ordering (`EvrtckEncoder::set_focus_pixel`).
//! The Attention Map fallback is a real gap, not silently faked: see
//! `visible_region_from_attention_map`'s doc comment.

use crate::evrt2_packet::Mode;
use std::time::Duration;

// ── Definition: Visible Region (Task 01 § Definition) ──────────────────────────

/// Default `P_visible_threshold` by mode — only meaningful once an
/// Attention Map exists to threshold against (see module doc: not
/// implemented here). Kept as documented spec data so the eventual
/// Attention Map integration has the right constants ready.
pub fn visible_threshold(mode: Mode) -> f32 {
    match mode {
        Mode::Ar => 0.85,
        Mode::R2 => 0.80,
        Mode::Mode47 => 0.75,
    }
}

/// Task 01 § Age Ceiling — hard per-mode constant, NOT derived from
/// `age_max(i)`, independent of network conditions.
pub fn age_ceiling(mode: Mode) -> Duration {
    match mode {
        Mode::Ar => Duration::from_millis(12),
        Mode::R2 => Duration::from_millis(15),
        Mode::Mode47 => Duration::from_millis(8),
    }
}

/// Visible Region as a set of tile indices (matching EVRTCK v2's
/// self-describing `tile_idx` wire field — see evrtck.rs module docs — so
/// this can be handed straight to `order_by_focus`-style tile reordering
/// without a translation layer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleRegion {
    pub tiles: Vec<u16>,
}

impl VisibleRegion {
    pub fn contains_tile(&self, tile_idx: u16) -> bool {
        self.tiles.contains(&tile_idx)
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

/// Explicit-focus source: a small bounding box of tiles around the client's
/// cursor/aim position — "typically covers a small, bounded area — a few
/// tiles around the cursor or crosshair, not the whole screen. This is what
/// makes an absolute guarantee affordable: the floor is cheap because the
/// region is small."
///
/// `focus_tile`: (tx, ty) tile coordinates, e.g. from
/// `cursor_pixel / TILE_SIZE` (same conversion `EvrtckEncoder::set_focus_pixel`
/// already does). `radius`: tiles outward from focus in each direction — a
/// radius of 1 gives a 3×3 box (9 tiles), matching "a few tiles."
pub fn visible_region_from_focus(
    focus_tile: (usize, usize),
    tiles_x: usize,
    tiles_y: usize,
    radius: usize,
) -> VisibleRegion {
    if tiles_x == 0 || tiles_y == 0 {
        return VisibleRegion { tiles: Vec::new() };
    }
    let (fx, fy) = focus_tile;
    let x0 = fx.saturating_sub(radius);
    let x1 = (fx + radius).min(tiles_x - 1);
    let y0 = fy.saturating_sub(radius);
    let y1 = (fy + radius).min(tiles_y - 1);

    let mut tiles = Vec::with_capacity((x1 - x0 + 1) * (y1 - y0 + 1));
    for ty in y0..=y1 {
        for tx in x0..=x1 {
            tiles.push((tx + ty * tiles_x) as u16);
        }
    }
    VisibleRegion { tiles }
}

/// Attention-Map fallback source — now real: see
/// `evrt2_attention::visible_region_from_map`, which builds `P_i` from
/// actually-measured motion + focus + surprise signals (not fabricated —
/// see that module's doc comment for exactly which of the spec's seven
/// signal channels are available in this product and which are honestly
/// zero-weighted). Kept as a separate module (not re-exported here) so
/// `evrt2_scheduler` doesn't depend back on `evrt2_attention`, which itself
/// depends on this module for `VisibleRegion` — avoids a cycle.

// ── M3: Packet scheduler send order ────────────────────────────────────────────

/// One outgoing slice, tagged with enough to sort it into the Task 01
/// send order. `payload` is opaque here (the scheduler orders, it doesn't
/// know or care about codec bytes) — callers attach whatever wire packet
/// they've already built (see `evrt2_packet::PacketHeader` + payload).
#[derive(Debug, Clone)]
pub struct Slice<T> {
    pub kind: SliceKind,
    pub payload: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SliceKind {
    /// Rank 0 — highest priority, always first. EVRT2CKMAX-TASK-01's one
    /// deliberate exception to ordinary priority-proportional scheduling.
    VisibleRegion,
    /// Rank 1 — existing IDR-first ordering.
    Idr,
    /// Rank 2 — remaining slices in priority order. Carries an explicit
    /// priority so slices within this rank sort by it (higher priority
    /// first); `Reverse`-free by storing priority as "distance", smaller
    /// sorts first — see `Ord` note on the field below.
    Normal(NormalPriority),
    /// Rank 3 — FEC repair, always last.
    FecRepair,
}

/// Wraps a priority such that a *smaller* value sorts *first* when
/// `SliceKind` is compared via its derived `Ord` — i.e. this stores
/// something like "distance from focus" or "1.0 - P_i", not raw priority,
/// so ordinary tuple/derive ordering ("smaller sorts first") does the right
/// thing without a custom comparator at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NormalPriority(pub u32);

/// Task 01 § Mechanism, 2. Packet scheduler preemption:
/// "Send order (revised): 1. Visible-region slices ... 2. IDR slices ...
/// 3. Remaining slices, priority order ... 4. FEC repair ..."
///
/// Stable sort: slices that compare equal (e.g. two `Normal` entries with
/// the same priority) keep their input relative order, which matters for
/// tile_idx-ordered EVRTCK payloads where insertion order already encodes
/// something meaningful (nearest-to-focus-first from `order_by_focus`).
pub fn schedule_send_order<T>(mut slices: Vec<Slice<T>>) -> Vec<Slice<T>> {
    slices.sort_by(|a, b| a.kind.cmp(&b.kind));
    slices
}

/// Task 01 § Mechanism, 2.: "If mid-frame congestion is detected... before
/// the visible region has been fully sent, all remaining non-visible-region
/// packets for this frame are dropped, not delayed."
///
/// `visible_region_fully_sent`: caller's own bookkeeping (e.g. "have all
/// `SliceKind::VisibleRegion` entries for this FrameId been handed to the
/// socket yet"). This function only decides WHAT to drop once that's true
/// — it does not itself track send progress, since that requires knowing
/// about actual wire transmission, out of scope for a pure scheduling
/// decision.
pub fn apply_congestion_drop<T>(
    scheduled: Vec<Slice<T>>,
    congestion_detected: bool,
    visible_region_fully_sent: bool,
) -> Vec<Slice<T>> {
    if congestion_detected && !visible_region_fully_sent {
        scheduled
            .into_iter()
            .filter(|s| s.kind == SliceKind::VisibleRegion)
            .collect()
    } else {
        scheduled
    }
}

// ── M5: DEGRADE_SIGNAL (Breach Handling) ───────────────────────────────────────

/// Task 01 § Breach Handling:
/// "if actual_age(visible_region) > age_ceiling: emit DEGRADE_SIGNAL
/// { region: visible_region, measured_age } ..."
#[derive(Debug, Clone, PartialEq)]
pub struct DegradeSignal {
    pub region: VisibleRegion,
    pub measured_age: Duration,
    pub ceiling: Duration,
}

/// Checks a measured Visible Region age against its mode's ceiling and
/// produces a `DegradeSignal` if breached. Pure function — emitting it onto
/// the wire (as `PacketType::DegradeSignal`) and driving the client's
/// degradation indicator / more-aggressive Warp fallback are the caller's
/// job; this only decides *whether* the guarantee was broken this frame.
///
/// "This must not become an excuse to fabricate" — this function does not
/// invent a region or an age; both are the caller's actual measurements,
/// passed in.
pub fn check_breach(
    mode: Mode,
    region: VisibleRegion,
    measured_age: Duration,
) -> Option<DegradeSignal> {
    let ceiling = age_ceiling(mode);
    if measured_age > ceiling {
        Some(DegradeSignal {
            region,
            measured_age,
            ceiling,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_and_ceilings_match_spec_table() {
        assert_eq!(visible_threshold(Mode::Ar), 0.85);
        assert_eq!(visible_threshold(Mode::R2), 0.80);
        assert_eq!(visible_threshold(Mode::Mode47), 0.75);

        assert_eq!(age_ceiling(Mode::Ar), Duration::from_millis(12));
        assert_eq!(age_ceiling(Mode::R2), Duration::from_millis(15));
        assert_eq!(age_ceiling(Mode::Mode47), Duration::from_millis(8));
    }

    #[test]
    fn visible_region_from_focus_is_small_and_bounded() {
        // 64×64 tile grid (2048×2048px @ 32px tiles) — radius=1 must stay a
        // 3×3 box regardless of grid size, matching "typically covers a
        // small, bounded area... not the whole screen."
        let region = visible_region_from_focus((30, 30), 64, 64, 1);
        assert_eq!(region.tiles.len(), 9);
    }

    #[test]
    fn visible_region_clamps_at_grid_edges() {
        // Focus in the top-left corner: radius=2 box must clip, not wrap
        // or panic, and must not include negative/out-of-range tiles.
        let region = visible_region_from_focus((0, 0), 10, 10, 2);
        // Clipped box is 3×3 (x:0..=2, y:0..=2), not 5×5.
        assert_eq!(region.tiles.len(), 9);
        assert!(region.tiles.iter().all(|&t| (t as usize) < 100));
    }

    #[test]
    fn visible_region_focus_tile_is_always_included() {
        let region = visible_region_from_focus((5, 5), 20, 20, 1);
        let focus_idx = (5 + 5 * 20) as u16;
        assert!(region.contains_tile(focus_idx));
    }

    #[test]
    fn send_order_matches_task01_mechanism_exactly() {
        let slices = vec![
            Slice {
                kind: SliceKind::FecRepair,
                payload: "fec",
            },
            Slice {
                kind: SliceKind::Normal(NormalPriority(5)),
                payload: "normal-low-pri",
            },
            Slice {
                kind: SliceKind::Idr,
                payload: "idr",
            },
            Slice {
                kind: SliceKind::Normal(NormalPriority(1)),
                payload: "normal-high-pri",
            },
            Slice {
                kind: SliceKind::VisibleRegion,
                payload: "visible",
            },
        ];
        let ordered = schedule_send_order(slices);
        let order: Vec<&str> = ordered.iter().map(|s| s.payload).collect();
        assert_eq!(
            order,
            vec!["visible", "idr", "normal-high-pri", "normal-low-pri", "fec"]
        );
    }

    #[test]
    fn send_order_is_stable_within_same_rank() {
        // Two VisibleRegion slices: input order (e.g. from EVRTCK's
        // nearest-to-focus-first tile ordering) must be preserved, not
        // reshuffled by the sort.
        let slices = vec![
            Slice {
                kind: SliceKind::VisibleRegion,
                payload: "first",
            },
            Slice {
                kind: SliceKind::VisibleRegion,
                payload: "second",
            },
            Slice {
                kind: SliceKind::VisibleRegion,
                payload: "third",
            },
        ];
        let ordered = schedule_send_order(slices);
        let order: Vec<&str> = ordered.iter().map(|s| s.payload).collect();
        assert_eq!(order, vec!["first", "second", "third"]);
    }

    #[test]
    fn congestion_drops_only_non_visible_region_before_visible_region_sent() {
        let slices = vec![
            Slice {
                kind: SliceKind::VisibleRegion,
                payload: 1,
            },
            Slice {
                kind: SliceKind::Idr,
                payload: 2,
            },
            Slice {
                kind: SliceKind::Normal(NormalPriority(0)),
                payload: 3,
            },
            Slice {
                kind: SliceKind::FecRepair,
                payload: 4,
            },
        ];
        let remaining = apply_congestion_drop(slices, true, false);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].kind, SliceKind::VisibleRegion);
    }

    #[test]
    fn no_drop_when_visible_region_already_fully_sent() {
        let slices = vec![
            Slice {
                kind: SliceKind::Idr,
                payload: 1,
            },
            Slice {
                kind: SliceKind::FecRepair,
                payload: 2,
            },
        ];
        let remaining = apply_congestion_drop(slices, true, true);
        assert_eq!(
            remaining.len(),
            2,
            "visible region already sent — congestion this late shouldn't wipe the rest"
        );
    }

    #[test]
    fn no_drop_without_congestion() {
        let slices = vec![
            Slice {
                kind: SliceKind::Idr,
                payload: 1,
            },
            Slice {
                kind: SliceKind::Normal(NormalPriority(0)),
                payload: 2,
            },
        ];
        let remaining = apply_congestion_drop(slices, false, false);
        assert_eq!(remaining.len(), 2);
    }

    #[test]
    fn breach_detected_when_age_exceeds_ceiling() {
        let region = VisibleRegion {
            tiles: vec![10, 11, 20],
        };
        let signal = check_breach(Mode::Mode47, region.clone(), Duration::from_millis(12));
        let signal = signal.expect("47 ceiling is 8ms, 12ms must breach");
        assert_eq!(signal.region, region);
        assert_eq!(signal.measured_age, Duration::from_millis(12));
        assert_eq!(signal.ceiling, Duration::from_millis(8));
    }

    #[test]
    fn no_breach_when_within_ceiling() {
        let region = VisibleRegion { tiles: vec![1] };
        assert_eq!(
            check_breach(Mode::Ar, region, Duration::from_millis(10)),
            None
        );
    }

    #[test]
    fn breach_is_exclusive_at_exactly_the_ceiling() {
        // "> age_ceiling" — exactly AT the ceiling is not a breach.
        let region = VisibleRegion { tiles: vec![1] };
        assert_eq!(
            check_breach(Mode::R2, region, Duration::from_millis(15)),
            None
        );
    }
}
