// =============================================================================
// EVRT2 — Attention Map (Five Fundamental Objects, object 1)
// Spec: evrt2/codec/EVRT2CKMAX.md § Five Fundamental Objects, 1. Attention Map
// Author of the standard: Arthur Valiev. Rust implementation below.
// =============================================================================
//
//! Computes `P_i` — the per-tile attention priority — from signals this
//! product can *actually* measure. The spec's full formula is:
//!
//! ```text
//! P_i = normalize(
//!     w_A × A_i +   // attention probability (gaze model / history)
//!     w_M × M_i +   // motion intensity (optical flow magnitude)
//!     w_U × U_i +   // UI element importance (HUD, crosshair, minimap)
//!     w_T × T_i +   // target presence (enemy, objective, cursor)
//!     w_G × G_i +   // gaze transfer probability
//!     w_S × S_i +   // scene surprise
//!     w_E × E_i     // engine semantic importance (game/app-reported)
//! )
//! ```
//!
//! **Honest scope:** `A_i` (eye-tracker gaze), `G_i` (gaze-transfer
//! prediction), and `E_i` (game-engine semantic hints) have no signal
//! source in a general-purpose remote-desktop product — there is no eye
//! tracker, no game engine reporting object classes. Per the spec's own
//! design, this is exactly the documented degradation path: "`E_i` is a
//! hint channel the host application can populate directly... When
//! unavailable, `w_E = 0` and the formula degrades gracefully to the
//! vision-only signals above." This module sets `w_A = w_G = w_E = 0`
//! honestly and computes the rest from real, measured signals:
//!
//! - **`M_i` (motion)** — per-tile dirty-byte ratio between this frame and
//!   the previous one, at the exact tile grid EVRTCK already uses
//!   (`evrtck::tile_is_dirty`/`TILE_SIZE`) — so a `VisibleRegion`'s tile
//!   indices are directly interchangeable with EVRTCK's `tile_idx` wire
//!   field, no translation layer.
//! - **`T_i` (target presence, cursor)** — the spec's own worked example
//!   lists "cursor" under `T_i`, not a separate signal: inverse-distance
//!   falloff from the client's cursor/focus tile, same shape as
//!   `EvrtckEncoder::set_focus_pixel`'s existing priority anchor.
//! - **`S_i` (scene surprise)** — a tile that was static for several
//!   consecutive frames and suddenly becomes dirty (a dialog appearing, a
//!   notification, a flash) — tracked via a per-tile static-streak
//!   counter, genuinely computed, not inferred from nothing.
//! - **`U_i` (UI element importance)** — no semantic UI detector exists in
//!   this codebase either; `w_U = 0`, same honesty as `A/G/E`.
//!
//! Mode-dependent weights (EVRT2CKMAX.md: "AR mode: w_U dominates... 2R
//! mode: w_M dominates... 47 mode: w_T, w_S, w_E dominate") are
//! re-derived below using only the signals actually available — see
//! `AttentionWeights::for_mode`'s doc comment for exactly how each
//! mode's *intent* is preserved despite the missing channels.

use crate::evrt2_packet::Mode;

/// Per-mode signal weights, using only the signals this module can
/// actually compute (`w_a = w_u = w_g = w_e = 0` always — see module doc).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttentionWeights {
    pub w_m: f32, // motion
    pub w_t: f32, // target/cursor presence
    pub w_s: f32, // scene surprise
}

impl AttentionWeights {
    /// AR: spec says `w_U` (UI importance) dominates — unavailable here.
    /// The closest available proxy for "what matters in a static desktop
    /// session" is the user's own cursor/focus (`T_i`), since UI
    /// importance in a support session is, in practice, wherever the
    /// operator is pointing. Motion gets a smaller share (AR content is
    /// mostly static by definition — a dirty tile there is meaningful but
    /// secondary to focus).
    pub const AR: Self = Self {
        w_m: 0.30,
        w_t: 0.70,
        w_s: 0.0,
    };

    /// 2R: spec says `w_M` dominates — this one signal IS available and
    /// used at full weight the spec intends. Small surprise term for scene
    /// cuts; small focus term since cursor still matters during playback
    /// scrubbing/UI interaction.
    pub const R2: Self = Self {
        w_m: 0.65,
        w_t: 0.20,
        w_s: 0.15,
    };

    /// 47: spec says `w_T, w_S, w_E` dominate. `w_E` (engine hints)
    /// unavailable → its weight mass is folded into `w_T` (cursor/aim is
    /// the strongest available proxy for "where the player is looking" in
    /// a gaming context without engine cooperation) and `w_S` (sudden
    /// events — explosions, flashes — are still genuinely detectable via
    /// the static-streak surprise signal even without semantic
    /// understanding of *what* appeared).
    pub const MODE47: Self = Self {
        w_m: 0.15,
        w_t: 0.55,
        w_s: 0.30,
    };

    pub fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::Ar => Self::AR,
            Mode::R2 => Self::R2,
            Mode::Mode47 => Self::MODE47,
        }
    }
}

/// Tracks per-tile state across frames — the static-streak counter needed
/// for `S_i` requires memory; motion (`M_i`) only needs the previous
/// frame's bytes, already required for tile-dirty comparison anyway.
pub struct AttentionMapBuilder {
    width: usize,
    height: usize,
    tiles_x: usize,
    tiles_y: usize,
    /// Consecutive frames each tile has been observed static (not dirty).
    /// Reset to 0 the instant a tile goes dirty.
    static_streak: Vec<u32>,
    prev_frame: Vec<u8>,
    have_prev: bool,
}

/// A tile counts as "was static long enough to be surprising if it changes
/// now" after this many consecutive unchanged frames — short enough to
/// react within a fraction of a second at normal frame rates, long enough
/// that ordinary per-frame motion noise doesn't trigger false surprises.
const SURPRISE_STATIC_STREAK_FRAMES: u32 = 8;

impl AttentionMapBuilder {
    pub fn new(width: usize, height: usize) -> Self {
        let tiles_x = crate::evrtck::tiles_in_dim(width);
        let tiles_y = crate::evrtck::tiles_in_dim(height);
        Self {
            width,
            height,
            tiles_x,
            tiles_y,
            static_streak: vec![0; tiles_x * tiles_y],
            prev_frame: vec![0u8; width * height * 4],
            have_prev: false,
        }
    }

    pub fn tiles_x(&self) -> usize {
        self.tiles_x
    }
    pub fn tiles_y(&self) -> usize {
        self.tiles_y
    }
    pub fn tile_count(&self) -> usize {
        self.tiles_x * self.tiles_y
    }

    /// Compute the Attention Map for `bgra` (current frame), given an
    /// optional focus tile (client cursor position, tile coordinates —
    /// same conversion as `EvrtckEncoder::set_focus_pixel`) and the active
    /// mode (selects `AttentionWeights`).
    ///
    /// Returns `attention_map[tile_idx] -> P_i in [0.0, 1.0]`, raster-order
    /// indexed (`tx + ty * tiles_x`) — the same indexing EVRTCK's wire
    /// `tile_idx` field uses, so results plug directly into
    /// `VisibleRegion { tiles: Vec<u16> }`.
    ///
    /// On the very first call (no previous frame yet), motion and surprise
    /// are both zero everywhere (nothing to compare against) — only focus
    /// contributes, which is correct: a freshly connected session has no
    /// temporal history to judge motion from.
    pub fn compute(
        &mut self,
        bgra: &[u8],
        focus_tile: Option<(usize, usize)>,
        mode: Mode,
    ) -> Vec<f32> {
        assert_eq!(
            bgra.len(),
            self.width * self.height * 4,
            "frame buffer size mismatch"
        );
        let weights = AttentionWeights::for_mode(mode);
        let n = self.tile_count();
        let mut map = vec![0.0f32; n];

        for ty in 0..self.tiles_y {
            for tx in 0..self.tiles_x {
                let idx = tx + ty * self.tiles_x;

                let m_i = if self.have_prev {
                    self.tile_motion_ratio_and_update(bgra, tx, ty)
                } else {
                    0.0
                };

                let is_dirty = m_i > 0.0;
                let s_i = if self.have_prev
                    && is_dirty
                    && self.static_streak[idx] >= SURPRISE_STATIC_STREAK_FRAMES
                {
                    1.0
                } else {
                    0.0
                };
                if is_dirty {
                    self.static_streak[idx] = 0;
                } else {
                    self.static_streak[idx] = self.static_streak[idx].saturating_add(1);
                }

                let t_i = focus_tile
                    .map(|f| target_proximity(tx, ty, f))
                    .unwrap_or(0.0);

                map[idx] =
                    (weights.w_m * m_i + weights.w_t * t_i + weights.w_s * s_i).clamp(0.0, 1.0);
            }
        }

        if !self.have_prev {
            // First frame ever: no prior tile-diff pass touched
            // `prev_frame` yet (the `have_prev` branch above never ran),
            // so this is the only place that needs to seed it — a one-time
            // cost, not paid again.
            self.prev_frame.copy_from_slice(bgra);
            self.have_prev = true;
        }
        // No whole-frame `copy_from_slice` here on the steady-state path
        // any more — see `tile_motion_ratio_and_update`'s doc comment for
        // why it's now redundant.

        map
    }

    /// Fraction of bytes that differ between this tile in `bgra` and the
    /// stored previous frame — a continuous motion magnitude (not just
    /// dirty/not-dirty), closer to the spec's "optical flow magnitude"
    /// intent than a binary flag, while still being cheap (no actual flow
    /// estimation, just a byte-diff ratio).
    ///
    /// Live-found (chasing `attn_cost_ms`'s remaining ~20ms after task
    /// #33's word-at-a-time diff): `compute()` used to follow this same
    /// per-tile diff pass with a SEPARATE, unconditional whole-frame
    /// `self.prev_frame.copy_from_slice(bgra)` — a second full pass over
    /// every byte this loop had just finished reading, purely to catch up
    /// `prev_frame` to the new frame. Since every tile in the grid gets
    /// visited exactly once per `compute()` call (the `ty`/`tx` loop in
    /// `compute` covers the whole frame, edge tiles included via the same
    /// `x1`/`y1` clamping used here), this method now folds that update
    /// into the SAME per-8-byte-word pass `diff_count_fast` already does —
    /// `prev`'s word only gets written when it actually changed (an
    /// unchanged word was skipped for counting AND now for writing too),
    /// which is strictly less memory traffic than the old two-pass
    /// approach, not just the same work reordered.
    fn tile_motion_ratio_and_update(&mut self, bgra: &[u8], tx: usize, ty: usize) -> f32 {
        let x0 = tx * crate::evrtck::TILE_SIZE;
        let y0 = ty * crate::evrtck::TILE_SIZE;
        let x1 = (x0 + crate::evrtck::TILE_SIZE).min(self.width);
        let y1 = (y0 + crate::evrtck::TILE_SIZE).min(self.height);

        let mut diff = 0usize;
        let mut total = 0usize;
        for y in y0..y1 {
            let row_start = (y * self.width + x0) * 4;
            let row_end = (y * self.width + x1) * 4;
            let cur_row = &bgra[row_start..row_end];
            let prev_row = &mut self.prev_frame[row_start..row_end];
            diff += diff_count_fast_and_update(cur_row, prev_row);
            total += cur_row.len();
        }
        if total == 0 {
            0.0
        } else {
            diff as f32 / total as f32
        }
    }
}

/// ROADMAP.md task #33 — `attn_cost_ms` (this whole module's `compute`) was
/// found live to be a stable ~24-27ms every single frame, second only to
/// capture cost in the per-frame breakdown. The naive byte-by-byte loop this
/// replaces spent that time re-checking bytes one at a time even across
/// wide, genuinely-static regions of a mostly-idle desktop — the overwhelmingly
/// common case for real usage. This computes the EXACT SAME diff count (not
/// an approximation — `tile_motion_ratio_and_update`'s own doc promises a
/// continuous, meaningful magnitude, not a sampled estimate, so changing
/// what's measured here would be a real behavior change, not just a
/// speedup) by comparing 8 bytes at a time via a single `u64` XOR: an
/// unchanged 8-byte word skips straight past with one comparison instead of
/// eight, and only a word that actually differs pays the original per-byte
/// cost to count exactly which bytes changed. Falls back to the plain
/// per-byte loop for the ≤7-byte tail (tile edge rows aren't always a
/// multiple of 8 bytes wide when the display resolution isn't a multiple of
/// `TILE_SIZE`).
///
/// Follow-up: also writes `cur`'s bytes back into `prev` in the same pass
/// (an unchanged word is skipped for the write too, not just the count) —
/// see `tile_motion_ratio_and_update`'s doc comment for why folding the
/// `prev_frame` update into this same scan, instead of a separate
/// whole-frame `copy_from_slice` afterward, removes a fully redundant
/// second pass over every byte.
///
/// Second follow-up: dispatches to a 32-bytes-at-a-time AVX2 version when
/// the running CPU actually has it (checked once per call via
/// `is_x86_feature_detected!`, which the standard library caches after the
/// first check — this is not a per-frame-meaningful cost), falling back to
/// the 8-bytes-at-a-time scalar version otherwise. Live-measured
/// `attn_cost_ms` under real full-screen motion stayed dominated by this
/// scan even after removing the redundant `copy_from_slice` (task #33's
/// follow-up above) — the scan itself, touching ~30MB (`cur` + `prev`) at
/// 2560×1440, was always the real floor, not extra work layered on top of
/// it.
#[inline]
fn diff_count_fast_and_update(cur: &[u8], prev: &mut [u8]) -> usize {
    debug_assert_eq!(cur.len(), prev.len());
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // Safety: guarded by the runtime feature check directly above —
            // `diff_count_avx2_and_update` is only ever reached on a CPU
            // that actually implements AVX2. The intrinsics used inside are
            // all unaligned load/store (`_mm256_loadu_si256`/`_storeu_...`),
            // so `cur`/`prev`'s slice alignment (or lack of it) is never a
            // safety concern.
            return unsafe { diff_count_avx2_and_update(cur, prev) };
        }
    }
    diff_count_scalar_and_update(cur, prev)
}

/// The original 8-bytes-at-a-time scalar version — kept as both the
/// non-x86_64/non-AVX2 fallback and the tail handler for the AVX2 path
/// below (whose 32-byte main loop needs a shorter-granularity fallback for
/// the ≤31-byte remainder, exactly the same reason the original version
/// already had its own ≤7-byte per-byte tail).
#[inline]
fn diff_count_scalar_and_update(cur: &[u8], prev: &mut [u8]) -> usize {
    debug_assert_eq!(cur.len(), prev.len());
    let mut count = 0usize;
    let mut i = 0usize;
    let len = cur.len();
    while i + 8 <= len {
        let wa = u64::from_ne_bytes(cur[i..i + 8].try_into().unwrap());
        let wb = u64::from_ne_bytes(prev[i..i + 8].try_into().unwrap());
        if wa != wb {
            for k in 0..8 {
                if cur[i + k] != prev[i + k] {
                    count += 1;
                }
            }
            prev[i..i + 8].copy_from_slice(&cur[i..i + 8]);
        }
        i += 8;
    }
    while i < len {
        if cur[i] != prev[i] {
            count += 1;
            prev[i] = cur[i];
        }
        i += 1;
    }
    count
}

/// AVX2 version of the same fused diff-count-and-update: 32 bytes/iteration
/// instead of 8 (`diff_count_scalar_and_update`'s word size), a 4x wider
/// compare per instruction. `_mm256_cmpeq_epi8` + `_mm256_movemask_epi8`
/// turns the 32-byte compare into a single 32-bit mask (one bit per byte,
/// set where the bytes were EQUAL); an all-ones mask means the whole lane
/// was unchanged and is skipped entirely (no count, no write) exactly like
/// the scalar version's all-equal-word fast path. Otherwise the differing
/// byte count is `(!mask).count_ones()` (population count of the inverted
/// mask — one bit per byte that DIFFERED) and the whole 32-byte lane gets
/// written back to `prev` in one unaligned store, same "write the whole
/// changed chunk, not just the differing bytes" granularity the scalar
/// version uses for its 8-byte words.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn diff_count_avx2_and_update(cur: &[u8], prev: &mut [u8]) -> usize {
    use std::arch::x86_64::{
        __m256i, _mm256_cmpeq_epi8, _mm256_loadu_si256, _mm256_movemask_epi8, _mm256_storeu_si256,
    };
    let len = cur.len();
    let mut i = 0usize;
    let mut count = 0usize;
    while i + 32 <= len {
        let va = _mm256_loadu_si256(cur.as_ptr().add(i) as *const __m256i);
        let vb = _mm256_loadu_si256(prev.as_ptr().add(i) as *const __m256i);
        let eq_mask = _mm256_movemask_epi8(_mm256_cmpeq_epi8(va, vb)) as u32;
        if eq_mask != u32::MAX {
            count += (!eq_mask).count_ones() as usize;
            _mm256_storeu_si256(prev.as_mut_ptr().add(i) as *mut __m256i, va);
        }
        i += 32;
    }
    count + diff_count_scalar_and_update(&cur[i..], &mut prev[i..])
}

/// Inverse-distance falloff from a focus tile, in tile-grid Chebyshev
/// distance (matches `evrt2_scheduler::visible_region_from_focus`'s
/// box-shaped notion of "nearby" rather than Euclidean, so a tile at the
/// edge of the focus box doesn't score much lower than the center — the
/// point is "in the focus neighborhood" not "exactly at the cursor").
fn target_proximity(tx: usize, ty: usize, focus: (usize, usize)) -> f32 {
    let dx = (tx as isize - focus.0 as isize).unsigned_abs();
    let dy = (ty as isize - focus.1 as isize).unsigned_abs();
    let dist = dx.max(dy) as f32;
    // Falls to ~0 by 6 tiles out (192px at 32px tiles) — a generous focus
    // neighborhood without lighting up the whole screen.
    (1.0 - dist / 6.0).max(0.0)
}

/// Top-percentile tile selection — the Attention Map fallback source for
/// `evrt2_scheduler::VisibleRegion` when no explicit client focus is
/// available (Task 01 § Definition: "else the top-percentile tiles from
/// the Attention Map itself"). Selects every tile at or above
/// `p_visible_threshold`.
pub fn visible_region_from_map(
    attention_map: &[f32],
    tiles_x: usize,
    p_visible_threshold: f32,
) -> crate::evrt2_scheduler::VisibleRegion {
    let _ = tiles_x; // kept in the signature for callers that need grid shape context
    let tiles: Vec<u16> = attention_map
        .iter()
        .enumerate()
        .filter(|&(_, &p)| p >= p_visible_threshold)
        .map(|(idx, _)| idx as u16)
        .collect();
    crate::evrt2_scheduler::VisibleRegion { tiles }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_frame(width: usize, height: usize, value: u8) -> Vec<u8> {
        vec![value; width * height * 4]
    }

    /// ROADMAP.md task #33: every diff-and-update variant (dispatcher,
    /// scalar fallback, and — when this test's own CPU actually has it —
    /// the AVX2 fast path) must produce the EXACT same count as the naive
    /// per-byte loop they all replace, checked over lengths that
    /// deliberately straddle BOTH the 8-byte scalar word boundary and the
    /// 32-byte AVX2 lane boundary (tile edge rows aren't always a multiple
    /// of either when the display resolution isn't a multiple of
    /// `TILE_SIZE`), not just round numbers. Also checks the follow-up
    /// behavior shared by all variants (folding the `prev_frame` write into
    /// the same pass): `prev` must end up bytewise identical to `cur`
    /// afterward, exactly as the original separate `copy_from_slice` would
    /// have left it.
    #[test]
    fn diff_count_fast_and_update_matches_naive_byte_comparison() {
        fn naive_diff(a: &[u8], b: &[u8]) -> usize {
            a.iter().zip(b.iter()).filter(|(x, y)| x != y).count()
        }

        let mut seed: u64 = 0x243F6A8885A308D3; // arbitrary fixed seed — deterministic test
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        let lengths = [
            0, 1, 3, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 4001,
        ];
        for len in lengths {
            let a: Vec<u8> = (0..len).map(|_| (next() % 256) as u8).collect();
            let mut b = a.clone();
            // Flip roughly a third of the bytes so both "identical
            // word/lane, skip fast" and "word/lane differs, count exactly"
            // paths are actually exercised, not just an all-same or
            // all-different degenerate case.
            for byte in b.iter_mut() {
                if next() % 3 == 0 {
                    *byte = byte.wrapping_add(1);
                }
            }
            let expected = naive_diff(&a, &b);

            let mut via_dispatcher = b.clone();
            assert_eq!(
                diff_count_fast_and_update(&a, &mut via_dispatcher),
                expected,
                "len={len} dispatcher mismatch"
            );
            assert_eq!(
                via_dispatcher, a,
                "len={len}: dispatcher must leave prev matching cur"
            );

            let mut via_scalar = b.clone();
            assert_eq!(
                diff_count_scalar_and_update(&a, &mut via_scalar),
                expected,
                "len={len} scalar mismatch"
            );
            assert_eq!(
                via_scalar, a,
                "len={len}: scalar must leave prev matching cur"
            );

            #[cfg(target_arch = "x86_64")]
            if is_x86_feature_detected!("avx2") {
                let mut via_avx2 = b.clone();
                let actual = unsafe { diff_count_avx2_and_update(&a, &mut via_avx2) };
                assert_eq!(actual, expected, "len={len} avx2 mismatch");
                assert_eq!(via_avx2, a, "len={len}: avx2 must leave prev matching cur");
            }
        }
    }

    #[test]
    fn first_frame_has_zero_attention_everywhere_without_focus() {
        let mut builder = AttentionMapBuilder::new(128, 128); // 4×4 tiles
        let map = builder.compute(&solid_frame(128, 128, 10), None, Mode::R2);
        assert!(map.iter().all(|&p| p == 0.0));
    }

    #[test]
    fn static_content_produces_zero_motion_after_first_frame() {
        let mut builder = AttentionMapBuilder::new(128, 128);
        let frame = solid_frame(128, 128, 10);
        builder.compute(&frame, None, Mode::R2); // seed prev_frame
        let map = builder.compute(&frame, None, Mode::R2); // identical frame
        assert!(
            map.iter().all(|&p| p == 0.0),
            "identical frames must show zero motion"
        );
    }

    #[test]
    fn changed_tile_gets_nonzero_motion_weight() {
        let mut builder = AttentionMapBuilder::new(64, 64); // 2×2 tiles
        let frame1 = solid_frame(64, 64, 10);
        builder.compute(&frame1, None, Mode::R2);

        let mut frame2 = frame1.clone();
        // Dirty the top-left tile (tx=0, ty=0) only.
        for y in 0..32 {
            for x in 0..32 {
                let i = (y * 64 + x) * 4;
                frame2[i] = 255;
            }
        }
        let map = builder.compute(&frame2, None, Mode::R2);
        assert!(map[0] > 0.0, "changed tile 0 must have nonzero P_i");
        assert_eq!(map[1], 0.0, "unchanged tile 1 must stay zero");
    }

    #[test]
    fn focus_tile_dominates_in_ar_mode_even_without_motion() {
        // 20×20 tile grid (640×640px) so a corner is genuinely outside the
        // 6-tile falloff radius from a near-corner focus point.
        let mut builder = AttentionMapBuilder::new(640, 640);
        let frame = solid_frame(640, 640, 5);
        builder.compute(&frame, Some((1, 1)), Mode::Ar);
        let map = builder.compute(&frame, Some((1, 1)), Mode::Ar);
        let focus_idx = 1 + 1 * 20;
        assert!(
            map[focus_idx] > 0.5,
            "focus tile should score highly in AR mode: got {}",
            map[focus_idx]
        );
        // The opposite corner (Chebyshev distance 18) is well outside the
        // falloff radius and must score zero.
        let far_idx = 19 + 19 * 20;
        assert_eq!(map[far_idx], 0.0);
    }

    #[test]
    fn surprise_fires_only_after_sustained_static_streak() {
        let mut builder = AttentionMapBuilder::new(64, 64);
        let mut frame = solid_frame(64, 64, 1);
        builder.compute(&frame, None, Mode::Mode47); // frame 0: seeds prev

        // Keep tile 0 static for fewer frames than the surprise threshold,
        // then dirty it — should NOT count as a surprise yet.
        for _ in 0..(SURPRISE_STATIC_STREAK_FRAMES - 2) {
            builder.compute(&frame, None, Mode::Mode47);
        }
        for y in 0..32 {
            for x in 0..32 {
                let i = (y * 64 + x) * 4;
                frame[i] = 200;
            }
        }
        let map_early = builder.compute(&frame, None, Mode::Mode47);
        // Motion alone still contributes, but let's confirm surprise's
        // marginal effect by comparing against a case with a full streak.
        let early_score = map_early[0];

        // Reset and repeat with a FULL static streak before the change.
        let mut builder2 = AttentionMapBuilder::new(64, 64);
        let mut frame2 = solid_frame(64, 64, 1);
        builder2.compute(&frame2, None, Mode::Mode47);
        for _ in 0..(SURPRISE_STATIC_STREAK_FRAMES + 2) {
            builder2.compute(&frame2, None, Mode::Mode47);
        }
        for y in 0..32 {
            for x in 0..32 {
                let i = (y * 64 + x) * 4;
                frame2[i] = 200;
            }
        }
        let map_late = builder2.compute(&frame2, None, Mode::Mode47);
        let late_score = map_late[0];

        assert!(late_score > early_score, "sustained-static tile going dirty must score higher (surprise) than one that was recently already dirty: early={early_score} late={late_score}");
    }

    #[test]
    fn weights_reflect_spec_mode_priorities() {
        // AR: w_U (unavailable) dominates in spec → here, w_t (its proxy) dominates.
        assert!(AttentionWeights::AR.w_t > AttentionWeights::AR.w_m);
        // 2R: w_M dominates.
        assert!(AttentionWeights::R2.w_m > AttentionWeights::R2.w_t);
        assert!(AttentionWeights::R2.w_m > AttentionWeights::R2.w_s);
        // 47: w_t + w_s (proxies for w_T/w_S/w_E) dominate over motion alone.
        assert!(
            AttentionWeights::MODE47.w_t + AttentionWeights::MODE47.w_s
                > AttentionWeights::MODE47.w_m
        );
    }

    #[test]
    fn visible_region_from_map_selects_only_tiles_above_threshold() {
        let map = vec![0.9, 0.5, 0.86, 0.2, 0.85];
        let region = visible_region_from_map(&map, 5, 0.85);
        assert_eq!(region.tiles, vec![0, 2, 4]);
    }

    #[test]
    fn target_proximity_peaks_at_focus_and_falls_off() {
        assert_eq!(target_proximity(5, 5, (5, 5)), 1.0);
        assert!(target_proximity(6, 5, (5, 5)) < 1.0);
        assert_eq!(target_proximity(20, 20, (5, 5)), 0.0);
    }
}
