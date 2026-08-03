// =============================================================================
// EVRT2 — Attention Priority Field (APF) wire encoding
// Spec: evrt2/codec/EVRT2CKMAX.md § Attention Priority Field
// Author of the standard: Arthur Valiev. Rust implementation below.
// =============================================================================
//
//! Wire representation of the Attention Map (`evrt2_attention::AttentionMapBuilder`'s
//! `P_i` output). ROADMAP.md Phase 3.1: only the `encoding=0x01` (u4-packed)
//! variant is implemented here — `f16` (0x02), `delta` (0x03), and
//! `temporal` (0x04) are specified in EVRT2CKMAX.md but not built yet
//! (Phase 3.2/3.4 in ROADMAP.md). Sending the real thing for one encoding,
//! tested and live-verified, is worth more than four half-built ones.
//!
//! ```text
//! APF Header (8 bytes):
//!   version    u8   = 1
//!   tile_size  u8   = 32 (pixels per tile edge)
//!   cols       u16  BE
//!   rows       u16  BE
//!   encoding   u8   = 0x01 (u4 packed) — the only variant implemented
//!   reserved   u8   = 0
//! APF Payload (encoding=u4): 4 bits per tile, 16 priority levels,
//!   ceil(cols*rows/2) bytes — two tiles packed per byte, high nibble first
//!   (raster tile order, matching every other tile_idx-ordered field in
//!   this codebase).
//! ```

pub const APF_VERSION: u8 = 1;
pub const APF_HEADER_LEN: usize = 8;
pub const APF_ENCODING_U4: u8 = 0x01;
/// ROADMAP.md Phase 3.2. EVRT2CKMAX.md § APF: "encoding=delta: run-length
/// encoded delta from previous APF" — the spec names the encoding but
/// doesn't define its byte layout, so the concrete RLE scheme below (one
/// byte per record: a SKIP run of unchanged tiles, or a single CHANGE to a
/// new u4 level) is this implementation's own design, sized against the
/// spec's own worked numbers ("Delta APF for minor attention shifts:
/// typically 50-200 bytes") — see `matches_spec_typical_delta_size_range`.
pub const APF_ENCODING_DELTA: u8 = 0x03;
/// ROADMAP.md Phase 3.4. EVRT2CKMAX.md § Temporal APF: 3 bytes/tile —
/// priority(u4) + max_age(u12, 2ms steps → 0-8190ms) + confidence(u8,
/// C_i × 255) — "only 2R/47" per the same section (AR's own per-mode
/// profile has no use for staleness tolerance, see AR2R47_MODES.md).
pub const APF_ENCODING_TEMPORAL: u8 = 0x04;

/// Quantizes a `P_i` value (already clamped to [0.0, 1.0] by
/// `AttentionMapBuilder::compute`) to one of 16 priority levels. Rounds to
/// the nearest level rather than truncating, so a P_i of e.g. 0.94 lands on
/// level 15 (P_i=1.0) rather than being pulled down to level 14.
fn quantize_u4(p: f32) -> u8 {
    (p.clamp(0.0, 1.0) * 15.0).round() as u8
}

fn dequantize_u4(level: u8) -> f32 {
    (level & 0x0F) as f32 / 15.0
}

/// Encode an Attention Map (raster-order `P_i` values, one per tile) as a
/// full APF packet payload (header + u4-packed body). `attention_map.len()`
/// must equal `cols as usize * rows as usize` — this is the same indexing
/// `AttentionMapBuilder::compute` already produces, no translation needed.
pub fn encode_u4(attention_map: &[f32], cols: u16, rows: u16, tile_size: u8) -> Vec<u8> {
    let tile_count = cols as usize * rows as usize;
    debug_assert_eq!(
        attention_map.len(),
        tile_count,
        "attention_map must have exactly cols*rows entries"
    );
    let packed_len = tile_count.div_ceil(2);
    let mut out = Vec::with_capacity(APF_HEADER_LEN + packed_len);
    out.push(APF_VERSION);
    out.push(tile_size);
    out.extend_from_slice(&cols.to_be_bytes());
    out.extend_from_slice(&rows.to_be_bytes());
    out.push(APF_ENCODING_U4);
    out.push(0); // reserved

    for pair in attention_map.chunks(2) {
        let level_hi = quantize_u4(pair[0]);
        let level_lo = pair.get(1).map(|&p| quantize_u4(p)).unwrap_or(0);
        out.push((level_hi << 4) | level_lo);
    }
    out
}

/// Downsamples a raster-order Attention Map from the encoder's native tile
/// grid to a coarser grid, `scale`× larger cells in each dimension —
/// grouping `scale*scale` native tiles into one APF cell. This is what
/// actually closes the ROADMAP.md Phase 3.1 "doesn't fit one UDP datagram"
/// gap: the spec's own APF header carries its OWN `tile_size` field
/// (independent of EVRTCK's fixed 32px tile), precisely so the priority
/// summary can be coarser than the pixel-delta grid it summarizes — "same
/// resolution as the frame (downsampled to tile granularity for
/// efficiency)" per EVRT2CKMAX.md's own Attention Map definition.
/// Fragmentation would have solved the same problem at the cost of a whole
/// new reassembly path for what is, by design, meant to be a compact
/// priority summary — not the wire-critical video payload.
///
/// Uses MAX (not average) over each cell: the Attention Map's purpose is
/// flagging what matters, and a single hot native tile inside an otherwise
/// calm coarse cell should not get averaged away into invisibility.
pub fn downsample_max(
    attention_map: &[f32],
    tiles_x: usize,
    tiles_y: usize,
    scale: usize,
) -> (Vec<f32>, usize, usize) {
    assert!(scale >= 1, "scale must be at least 1 (1 = no downsampling)");
    let coarse_cols = tiles_x.div_ceil(scale);
    let coarse_rows = tiles_y.div_ceil(scale);
    let mut coarse = vec![0.0f32; coarse_cols * coarse_rows];
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let p = attention_map[tx + ty * tiles_x];
            let cx = tx / scale;
            let cy = ty / scale;
            let idx = cx + cy * coarse_cols;
            if p > coarse[idx] {
                coarse[idx] = p;
            }
        }
    }
    (coarse, coarse_cols, coarse_rows)
}

/// Smallest `scale` (native tiles per APF cell edge, ≥1) such that the
/// resulting u4-packed APF wire packet fits within `max_payload` bytes —
/// the largest UDP datagram this session's transport will carry (see
/// `evrt2_packet::MAX_PAYLOAD`). Returns `None` only if even the coarsest
/// representable cell size (`u8::MAX` tiles/edge — astronomically coarse)
/// still wouldn't fit, which does not happen for any realistic resolution.
pub fn fit_scale_for_budget(tiles_x: usize, tiles_y: usize, max_payload: usize) -> Option<usize> {
    for scale in 1..=255usize {
        let cols = tiles_x.div_ceil(scale);
        let rows = tiles_y.div_ceil(scale);
        if APF_HEADER_LEN + (cols * rows).div_ceil(2) <= max_payload {
            return Some(scale);
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApfHeader {
    pub version: u8,
    pub tile_size: u8,
    pub cols: u16,
    pub rows: u16,
    pub encoding: u8,
}

/// Decode an APF packet payload back into `(header, attention_map)`.
/// Returns `None` on truncated input or an encoding this decoder doesn't
/// support (only `APF_ENCODING_U4` — see module doc). The returned map has
/// exactly `header.cols * header.rows` entries, raster-ordered, matching
/// `encode_u4`'s input shape.
pub fn decode_u4(payload: &[u8]) -> Option<(ApfHeader, Vec<f32>)> {
    if payload.len() < APF_HEADER_LEN {
        return None;
    }
    let header = ApfHeader {
        version: payload[0],
        tile_size: payload[1],
        cols: u16::from_be_bytes([payload[2], payload[3]]),
        rows: u16::from_be_bytes([payload[4], payload[5]]),
        encoding: payload[6],
        // payload[7] reserved, ignored on read (must be written as 0)
    };
    if header.encoding != APF_ENCODING_U4 {
        return None;
    }
    let tile_count = header.cols as usize * header.rows as usize;
    let packed_len = tile_count.div_ceil(2);
    let body = payload.get(APF_HEADER_LEN..APF_HEADER_LEN + packed_len)?;

    let mut map = Vec::with_capacity(tile_count);
    for &byte in body {
        if map.len() < tile_count {
            map.push(dequantize_u4(byte >> 4));
        }
        if map.len() < tile_count {
            map.push(dequantize_u4(byte & 0x0F));
        }
    }
    Some((header, map))
}

/// Largest run of unchanged tiles a single SKIP record can carry (7 bits —
/// the 8th bit distinguishes SKIP from CHANGE, see the record format doc
/// on `encode_delta`).
const DELTA_MAX_SKIP_RUN: usize = 0x7F;

/// ROADMAP.md Phase 3.2 — RLE delta from a previous APF snapshot at the
/// SAME grid (`previous.len()` must equal `current.len()` — a resolution
/// or scale change must send a full `encode_u4` instead, this function
/// doesn't handle a dimension change). One byte per record, in tile raster
/// order:
///   bit7=0: SKIP  — bits[6:0] = count of unchanged tiles to skip (1-127)
///   bit7=1: CHANGE — bits[3:0] = new u4 priority level for the next tile
///                     (bits[6:4] reserved, written 0)
/// "Unchanged" compares QUANTIZED levels (not raw floats) — two P_i values
/// that map to the same u4 level are indistinguishable on the wire anyway,
/// so treating them as unchanged is not lossy relative to what `encode_u4`
/// itself already discards, and keeps typical deltas small.
pub fn encode_delta(
    previous: &[f32],
    current: &[f32],
    cols: u16,
    rows: u16,
    tile_size: u8,
) -> Vec<u8> {
    let tile_count = cols as usize * rows as usize;
    debug_assert_eq!(
        previous.len(),
        tile_count,
        "previous must match the declared grid"
    );
    debug_assert_eq!(
        current.len(),
        tile_count,
        "current must match the declared grid"
    );
    let mut out = Vec::with_capacity(APF_HEADER_LEN + 64);
    out.push(APF_VERSION);
    out.push(tile_size);
    out.extend_from_slice(&cols.to_be_bytes());
    out.extend_from_slice(&rows.to_be_bytes());
    out.push(APF_ENCODING_DELTA);
    out.push(0); // reserved

    let mut unchanged_run = 0usize;
    for i in 0..tile_count {
        if quantize_u4(previous[i]) == quantize_u4(current[i]) {
            unchanged_run += 1;
        } else {
            push_skip_records(&mut out, unchanged_run);
            unchanged_run = 0;
            out.push(0x80 | quantize_u4(current[i]));
        }
    }
    push_skip_records(&mut out, unchanged_run);
    out
}

fn push_skip_records(out: &mut Vec<u8>, mut run: usize) {
    while run > 0 {
        let n = run.min(DELTA_MAX_SKIP_RUN);
        out.push(n as u8); // bit7=0 is automatic since n <= 0x7F
        run -= n;
    }
}

/// Decode a delta APF payload against `previous` (the caller's own
/// last-known full map at the same grid — the receiver-side mirror of what
/// `encode_delta` was built from). Returns `None` on truncated/malformed
/// input, an encoding this isn't ready for, or a dimension mismatch against
/// `previous` (the caller must have dropped its baseline on a real
/// resolution change and asked the sender to resync with a full APF
/// instead — this function has no way to recover from a wrong baseline).
pub fn decode_delta(payload: &[u8], previous: &[f32]) -> Option<(ApfHeader, Vec<f32>)> {
    if payload.len() < APF_HEADER_LEN {
        return None;
    }
    let header = ApfHeader {
        version: payload[0],
        tile_size: payload[1],
        cols: u16::from_be_bytes([payload[2], payload[3]]),
        rows: u16::from_be_bytes([payload[4], payload[5]]),
        encoding: payload[6],
    };
    if header.encoding != APF_ENCODING_DELTA {
        return None;
    }
    let tile_count = header.cols as usize * header.rows as usize;
    if previous.len() != tile_count {
        return None;
    }
    let mut map = previous.to_vec();
    let mut idx = 0usize;
    let mut pos = APF_HEADER_LEN;
    while idx < tile_count {
        let byte = *payload.get(pos)?;
        pos += 1;
        if byte & 0x80 == 0 {
            let skip = (byte & 0x7F) as usize;
            if idx + skip > tile_count {
                return None; // malformed: skip run overruns the declared grid
            }
            idx += skip;
        } else {
            map[idx] = dequantize_u4(byte & 0x0F);
            idx += 1;
        }
    }
    Some((header, map))
}

/// One tile's worth of Temporal APF data (ROADMAP.md Phase 3.4).
/// `priority` and `confidence` are both already-normalized [0.0, 1.0] —
/// same convention as the base APF's `P_i` and `Five Fundamental Objects`'
/// `C_i` (Temporal Confidence) elsewhere in this codebase.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemporalApfTile {
    pub priority: f32,
    /// Acceptable staleness before this tile must be force-refreshed.
    /// Values above 8190ms are clamped — see `encode_temporal`.
    pub max_age_ms: u16,
    pub confidence: f32,
}

/// Largest `max_age_ms` representable in the 12-bit, 2ms-step wire field
/// (EVRT2CKMAX.md: "0-4095ms range (2ms steps: 0-8190ms)" — 4095 steps ×
/// 2ms = 8190ms is the actual ceiling the field can carry).
pub const TEMPORAL_APF_MAX_AGE_MS: u16 = 8190;

/// ROADMAP.md Phase 3.4 — EVRT2CKMAX.md § Temporal APF wire format:
/// ```text
/// byte0: priority(u4, high nibble) | max_age_high(u4, low nibble)
/// byte1: max_age_low(u8)               — together a 12-bit max_age in 2ms steps
/// byte2: confidence(u8)                 — C_i × 255
/// ```
/// 3 bytes/tile, raster order, no RLE/delta (temporal state changes too
/// unpredictably per-tile for a shared previous-frame baseline to pay off
/// the way it does for priority alone in `encode_delta`).
pub fn encode_temporal(tiles: &[TemporalApfTile], cols: u16, rows: u16, tile_size: u8) -> Vec<u8> {
    let tile_count = cols as usize * rows as usize;
    debug_assert_eq!(
        tiles.len(),
        tile_count,
        "tiles must have exactly cols*rows entries"
    );
    let mut out = Vec::with_capacity(APF_HEADER_LEN + tile_count * 3);
    out.push(APF_VERSION);
    out.push(tile_size);
    out.extend_from_slice(&cols.to_be_bytes());
    out.extend_from_slice(&rows.to_be_bytes());
    out.push(APF_ENCODING_TEMPORAL);
    out.push(0); // reserved

    for t in tiles {
        let priority_level = quantize_u4(t.priority);
        let age_steps = (t.max_age_ms.min(TEMPORAL_APF_MAX_AGE_MS) as u16 / 2).min(0x0FFF);
        let confidence_byte = (t.confidence.clamp(0.0, 1.0) * 255.0).round() as u8;
        out.push((priority_level << 4) | ((age_steps >> 8) as u8 & 0x0F));
        out.push((age_steps & 0xFF) as u8);
        out.push(confidence_byte);
    }
    out
}

/// Decode a Temporal APF payload. `None` on truncated input or an
/// unexpected encoding byte — same failure contract as `decode_u4`/
/// `decode_delta`.
pub fn decode_temporal(payload: &[u8]) -> Option<(ApfHeader, Vec<TemporalApfTile>)> {
    if payload.len() < APF_HEADER_LEN {
        return None;
    }
    let header = ApfHeader {
        version: payload[0],
        tile_size: payload[1],
        cols: u16::from_be_bytes([payload[2], payload[3]]),
        rows: u16::from_be_bytes([payload[4], payload[5]]),
        encoding: payload[6],
    };
    if header.encoding != APF_ENCODING_TEMPORAL {
        return None;
    }
    let tile_count = header.cols as usize * header.rows as usize;
    let body_len = tile_count * 3;
    let body = payload.get(APF_HEADER_LEN..APF_HEADER_LEN + body_len)?;

    let mut tiles = Vec::with_capacity(tile_count);
    for chunk in body.chunks_exact(3) {
        let (byte0, byte1, confidence_byte) = (chunk[0], chunk[1], chunk[2]);
        let priority_level = byte0 >> 4;
        let age_high = (byte0 & 0x0F) as u16;
        let age_steps = (age_high << 8) | byte1 as u16;
        tiles.push(TemporalApfTile {
            priority: dequantize_u4(priority_level),
            max_age_ms: age_steps * 2,
            confidence: confidence_byte as f32 / 255.0,
        });
    }
    Some((header, tiles))
}

/// Expected packet size for a given tile grid — matches EVRT2CKMAX.md's own
/// worked example (1080p/32px tiles/u4 → 1020 bytes payload, 1028 with the
/// 8-byte header).
pub fn expected_wire_len(cols: u16, rows: u16) -> usize {
    APF_HEADER_LEN + (cols as usize * rows as usize).div_ceil(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── downsampling (fits-in-one-datagram fix) ─────────────────────────────

    #[test]
    fn downsample_max_picks_the_hottest_tile_in_each_cell() {
        // 4x4 native grid, scale=2 → 2x2 coarse grid, each cell = 2x2 native.
        // Put a single hot tile at (3,3) — bottom-right corner of the whole
        // grid, inside the bottom-right coarse cell.
        let mut map = vec![0.1f32; 16];
        map[3 + 3 * 4] = 0.9; // (tx=3, ty=3)
        let (coarse, cols, rows) = downsample_max(&map, 4, 4, 2);
        assert_eq!((cols, rows), (2, 2));
        assert_eq!(coarse.len(), 4);
        assert_eq!(
            coarse[1 + 1 * 2],
            0.9,
            "hot tile must survive into its coarse cell, not get averaged away"
        );
        assert_eq!(
            coarse[0], 0.1,
            "untouched cell keeps the flat baseline value"
        );
    }

    #[test]
    fn downsample_max_handles_non_multiple_grid_sizes() {
        // 5x3 grid, scale=2 → ceil(5/2)=3 cols, ceil(3/2)=2 rows — the
        // right/bottom edge cells are partially filled by fewer native
        // tiles, must not panic or drop data.
        let map = vec![0.5f32; 5 * 3];
        let (coarse, cols, rows) = downsample_max(&map, 5, 3, 2);
        assert_eq!((cols, rows), (3, 2));
        assert_eq!(coarse.len(), 6);
        assert!(coarse.iter().all(|&p| p == 0.5));
    }

    #[test]
    fn downsample_scale_one_is_identity() {
        let map = vec![0.1, 0.9, 0.3, 0.7];
        let (coarse, cols, rows) = downsample_max(&map, 2, 2, 1);
        assert_eq!((cols, rows), (2, 2));
        assert_eq!(coarse, map);
    }

    #[test]
    fn fit_scale_for_budget_finds_the_smallest_fitting_scale() {
        // 3600 native tiles (60x60), same order of magnitude as the real
        // 2560x1440@32px case (80x45=3600) that triggered this fix.
        let scale = fit_scale_for_budget(60, 60, crate::evrt2_packet::MAX_PAYLOAD)
            .expect("must find a fitting scale");
        let cols = 60usize.div_ceil(scale);
        let rows = 60usize.div_ceil(scale);
        assert!(APF_HEADER_LEN + (cols * rows).div_ceil(2) <= crate::evrt2_packet::MAX_PAYLOAD);
        // Confirm it's actually the SMALLEST fitting scale, not just A fitting one.
        if scale > 1 {
            let smaller = scale - 1;
            let c2 = 60usize.div_ceil(smaller);
            let r2 = 60usize.div_ceil(smaller);
            assert!(
                APF_HEADER_LEN + (c2 * r2).div_ceil(2) > crate::evrt2_packet::MAX_PAYLOAD,
                "scale-1={smaller} should NOT fit, otherwise {scale} isn't the smallest"
            );
        }
    }

    #[test]
    fn fit_scale_for_budget_returns_one_for_small_grids() {
        // A grid that already fits at native resolution shouldn't be
        // coarsened at all.
        assert_eq!(
            fit_scale_for_budget(10, 10, crate::evrt2_packet::MAX_PAYLOAD),
            Some(1)
        );
    }

    #[test]
    fn downsampled_grid_actually_fits_the_wire() {
        // End-to-end: the real 2560x1440@32px case (80x45=3600 tiles, which
        // is exactly what triggered the "APF skipped" log live) must now
        // produce a packet within MAX_PAYLOAD once scaled.
        let tiles_x = 80;
        let tiles_y = 45;
        let map = vec![0.5f32; tiles_x * tiles_y];
        let scale =
            fit_scale_for_budget(tiles_x, tiles_y, crate::evrt2_packet::MAX_PAYLOAD).unwrap();
        let (coarse, cols, rows) = downsample_max(&map, tiles_x, tiles_y, scale);
        assert_eq!(coarse.len(), cols * rows);
        let wire = encode_u4(
            &coarse,
            cols as u16,
            rows as u16,
            (32usize * scale).min(255) as u8,
        );
        assert!(
            wire.len() <= crate::evrt2_packet::MAX_PAYLOAD,
            "wire={} budget={}",
            wire.len(),
            crate::evrt2_packet::MAX_PAYLOAD
        );
    }

    #[test]
    fn roundtrip_preserves_header_fields() {
        let map = vec![0.0f32; 6 * 4];
        let wire = encode_u4(&map, 6, 4, 32);
        let (header, _) = decode_u4(&wire).expect("must decode");
        assert_eq!(header.version, APF_VERSION);
        assert_eq!(header.tile_size, 32);
        assert_eq!(header.cols, 6);
        assert_eq!(header.rows, 4);
        assert_eq!(header.encoding, APF_ENCODING_U4);
    }

    #[test]
    fn roundtrip_preserves_priority_within_quantization_step() {
        // 16 levels over [0,1] → step size 1/15 ≈ 0.0667. Any P_i must
        // decode back within half a step of the original value.
        let map = vec![0.0, 0.1, 0.25, 0.5, 0.73, 0.9, 1.0];
        let cols = map.len() as u16;
        let wire = encode_u4(&map, cols, 1, 32);
        let (_, decoded) = decode_u4(&wire).expect("must decode");
        assert_eq!(decoded.len(), map.len());
        let half_step = 1.0 / 15.0 / 2.0 + 1e-6;
        for (original, got) in map.iter().zip(decoded.iter()) {
            assert!(
                (original - got).abs() <= half_step,
                "original={original} got={got} exceeds half-step {half_step}"
            );
        }
    }

    #[test]
    fn zero_and_one_are_exact() {
        // Boundary values must round-trip exactly — no quantization drift
        // at the extremes (level 0 and level 15).
        let map = vec![0.0, 1.0];
        let wire = encode_u4(&map, 2, 1, 32);
        let (_, decoded) = decode_u4(&wire).unwrap();
        assert_eq!(decoded, vec![0.0, 1.0]);
    }

    #[test]
    fn odd_tile_count_does_not_leak_a_padding_tile() {
        // 5 tiles → packed into 3 bytes (last byte's low nibble is padding
        // 0, not tile data) — decode must still produce exactly 5 entries,
        // not 6.
        let map = vec![0.2, 0.4, 0.6, 0.8, 1.0];
        let wire = encode_u4(&map, 5, 1, 32);
        assert_eq!(wire.len(), expected_wire_len(5, 1));
        let (header, decoded) = decode_u4(&wire).unwrap();
        assert_eq!(header.cols as usize * header.rows as usize, 5);
        assert_eq!(decoded.len(), 5);
    }

    #[test]
    fn matches_spec_worked_example_size_at_1080p() {
        // EVRT2CKMAX.md § APF: "1080p with 32px tiles and u4 encoding:
        // Grid: 60×34 = 2040 tiles, APF size: 2040/2 = 1020 bytes" (payload
        // only — the doc's number excludes the 8-byte header).
        let cols = 60u16;
        let rows = 34u16;
        assert_eq!(cols as usize * rows as usize, 2040);
        assert_eq!(expected_wire_len(cols, rows) - APF_HEADER_LEN, 1020);
    }

    #[test]
    fn decode_rejects_truncated_payload() {
        assert!(
            decode_u4(&[1, 32, 0, 6, 0, 4, APF_ENCODING_U4]).is_none(),
            "7 bytes, header alone needs 8"
        );
        let map = vec![0.5f32; 6 * 4];
        let wire = encode_u4(&map, 6, 4, 32);
        assert!(
            decode_u4(&wire[..wire.len() - 1]).is_none(),
            "body one byte short of declared grid"
        );
    }

    #[test]
    fn decode_rejects_unsupported_encoding() {
        let mut wire = encode_u4(&[0.5; 4], 2, 2, 32);
        wire[6] = 0x02; // claim f16 encoding, which this decoder doesn't implement
        assert!(decode_u4(&wire).is_none());
    }

    #[test]
    fn quantize_dequantize_are_monotonic() {
        // Higher input priority must never decode to a lower output
        // priority — a jittery/non-monotonic mapping would corrupt
        // priority ordering on the wire.
        let mut prev = -1.0f32;
        for i in 0..=100 {
            let p = i as f32 / 100.0;
            let decoded = dequantize_u4(quantize_u4(p));
            assert!(
                decoded >= prev - 1e-6,
                "non-monotonic at p={p}: prev={prev} got={decoded}"
            );
            prev = decoded;
        }
    }

    // ── Delta-APF (ROADMAP.md Phase 3.2) ────────────────────────────────

    #[test]
    fn delta_roundtrips_a_small_localized_change() {
        let cols = 60u16;
        let rows = 34u16;
        let previous = vec![0.1f32; cols as usize * rows as usize];
        let mut current = previous.clone();
        // A handful of tiles shift attention — a "minor attention shift"
        // per EVRT2CKMAX.md's own delta example.
        for i in [100usize, 101, 102, 500, 1500] {
            current[i] = 0.9;
        }
        let wire = encode_delta(&previous, &current, cols, rows, 32);
        let (header, decoded) = decode_delta(&wire, &previous).expect("must decode");
        assert_eq!(header.encoding, APF_ENCODING_DELTA);
        assert_eq!(decoded.len(), current.len());
        for i in 0..current.len() {
            assert_eq!(
                quantize_u4(decoded[i]),
                quantize_u4(current[i]),
                "tile {i} mismatch after delta round trip"
            );
        }
    }

    #[test]
    fn no_change_produces_a_minimal_all_skip_payload() {
        let cols = 60u16;
        let rows = 34u16;
        let map = vec![0.3f32; cols as usize * rows as usize];
        let wire = encode_delta(&map, &map, cols, rows, 32);
        let (_, decoded) = decode_delta(&wire, &map).unwrap();
        assert_eq!(decoded, map);
        // ceil(2040 / 127) = 17 skip records, well under a full 1020-byte
        // u4 snapshot — the whole point of the delta encoding existing.
        assert!(
            wire.len() < 32,
            "an all-unchanged delta should be tiny, got {} bytes",
            wire.len()
        );
    }

    #[test]
    fn matches_spec_typical_delta_size_range() {
        // EVRT2CKMAX.md § APF: "Delta APF for minor attention shifts:
        // typically 50-200 bytes" — simulate a plausible "minor shift"
        // (a compact cluster of ~40 tiles changing, e.g. a moving crosshair
        // or a small UI element) at the spec's own 1080p/32px grid.
        let cols = 60u16;
        let rows = 34u16;
        let tile_count = cols as usize * rows as usize;
        let previous = vec![0.1f32; tile_count];
        let mut current = previous.clone();
        for i in 400..440 {
            current[i] = 0.8;
        }
        let wire = encode_delta(&previous, &current, cols, rows, 32);
        let payload_len = wire.len() - APF_HEADER_LEN;
        assert!(
            (50..=200).contains(&payload_len),
            "expected a payload in the spec's stated 50-200 byte range for a minor shift, got {payload_len}"
        );
    }

    #[test]
    fn decode_delta_rejects_a_dimension_mismatch_against_the_baseline() {
        let cols = 10u16;
        let rows = 10u16;
        let map = vec![0.5f32; cols as usize * rows as usize];
        let wire = encode_delta(&map, &map, cols, rows, 32);
        let wrong_sized_previous = vec![0.5f32; 50]; // doesn't match 10x10=100
        assert!(decode_delta(&wire, &wrong_sized_previous).is_none());
    }

    #[test]
    fn decode_delta_rejects_a_malformed_skip_run_overrunning_the_grid() {
        let cols = 4u16;
        let rows = 1u16;
        let previous = vec![0.5f32; 4];
        let mut wire = encode_delta(&previous, &previous, cols, rows, 32);
        // Corrupt the single skip record to claim more tiles than the grid has.
        let body_start = APF_HEADER_LEN;
        wire[body_start] = 0x7F; // claims 127 unchanged tiles in a 4-tile grid
        assert!(decode_delta(&wire, &previous).is_none());
    }

    #[test]
    fn decode_delta_rejects_unsupported_encoding() {
        let map = vec![0.5f32; 4];
        let mut wire = encode_u4(&map, 2, 2, 32); // a real u4 packet, not delta
        wire[6] = APF_ENCODING_U4;
        assert!(decode_delta(&wire, &map).is_none());
    }

    #[test]
    fn delta_treats_same_quantized_level_as_unchanged_even_with_float_noise() {
        // Two floats close enough to quantize to the same u4 level must be
        // encoded as SKIP, not a spurious CHANGE — matches what `encode_u4`
        // itself would already discard, so this isn't extra lossiness.
        let cols = 2u16;
        let rows = 1u16;
        let previous = vec![0.50f32, 0.50f32];
        let current = vec![0.501f32, 0.50f32]; // same u4 level (8/15 ≈ 0.533 rounds both to level 8)
        let wire = encode_delta(&previous, &current, cols, rows, 32);
        assert_eq!(
            wire.len(),
            APF_HEADER_LEN + 1,
            "both tiles unchanged at u4 resolution → one skip record"
        );
    }

    // ── Temporal APF (ROADMAP.md Phase 3.4) ─────────────────────────────

    #[test]
    fn temporal_matches_spec_worked_example() {
        // EVRT2CKMAX.md § Temporal APF worked example: priority=0.92
        // (15/16), max_age=15ms, confidence=0.85.
        let tile = TemporalApfTile {
            priority: 0.92,
            max_age_ms: 15,
            confidence: 0.85,
        };
        let wire = encode_temporal(&[tile], 1, 1, 32);
        assert_eq!(wire.len() - APF_HEADER_LEN, 3, "spec: 3 bytes per tile");
        let (header, decoded) = decode_temporal(&wire).expect("must decode");
        assert_eq!(header.encoding, APF_ENCODING_TEMPORAL);
        assert_eq!(decoded.len(), 1);
        // priority 0.92 quantizes to level 14 (15/16=0.9375 is level 15;
        // 0.92*15=13.8 rounds to 14) — same u4 quantization as base APF.
        assert_eq!(decoded[0].priority, dequantize_u4(quantize_u4(0.92)));
        assert_eq!(
            decoded[0].max_age_ms, 14,
            "15ms rounds down to the nearest 2ms step"
        );
        let confidence_step = 1.0 / 255.0;
        assert!(
            (decoded[0].confidence - 0.85).abs() <= confidence_step,
            "confidence off by more than one u8 step"
        );
    }

    #[test]
    fn temporal_roundtrips_a_grid_of_varied_tiles() {
        let cols = 6u16;
        let rows = 4u16;
        let tile_count = cols as usize * rows as usize;
        let tiles: Vec<TemporalApfTile> = (0..tile_count)
            .map(|i| TemporalApfTile {
                priority: (i % 16) as f32 / 15.0,
                max_age_ms: ((i * 137) % 8191) as u16,
                confidence: (i % 256) as f32 / 255.0,
            })
            .collect();
        let wire = encode_temporal(&tiles, cols, rows, 32);
        let (header, decoded) = decode_temporal(&wire).expect("must decode");
        assert_eq!(header.cols, cols);
        assert_eq!(header.rows, rows);
        assert_eq!(decoded.len(), tile_count);
        for (original, got) in tiles.iter().zip(decoded.iter()) {
            assert_eq!(got.priority, dequantize_u4(quantize_u4(original.priority)));
            // max_age_ms rounds down to the nearest 2ms step.
            assert_eq!(got.max_age_ms, (original.max_age_ms / 2) * 2);
            let confidence_step = 1.0 / 255.0;
            assert!((got.confidence - original.confidence).abs() <= confidence_step + 1e-6);
        }
    }

    #[test]
    fn temporal_max_age_clamps_at_the_wire_ceiling() {
        // 12-bit field, 2ms steps → 8190ms is the largest representable
        // value (EVRT2CKMAX.md's own stated ceiling). A caller passing
        // something larger (a bug, or a deliberately "never expires" sentinel)
        // must not wrap around or panic — it clamps.
        let tile = TemporalApfTile {
            priority: 0.5,
            max_age_ms: u16::MAX,
            confidence: 0.5,
        };
        let wire = encode_temporal(&[tile], 1, 1, 32);
        let (_, decoded) = decode_temporal(&wire).unwrap();
        assert_eq!(decoded[0].max_age_ms, TEMPORAL_APF_MAX_AGE_MS);
    }

    #[test]
    fn temporal_confidence_zero_and_one_are_exact() {
        let tiles = [
            TemporalApfTile {
                priority: 0.5,
                max_age_ms: 100,
                confidence: 0.0,
            },
            TemporalApfTile {
                priority: 0.5,
                max_age_ms: 100,
                confidence: 1.0,
            },
        ];
        let wire = encode_temporal(&tiles, 2, 1, 32);
        let (_, decoded) = decode_temporal(&wire).unwrap();
        assert_eq!(decoded[0].confidence, 0.0);
        assert_eq!(decoded[1].confidence, 1.0);
    }

    #[test]
    fn decode_temporal_rejects_truncated_payload() {
        let tiles = vec![
            TemporalApfTile {
                priority: 0.5,
                max_age_ms: 100,
                confidence: 0.5
            };
            4
        ];
        let wire = encode_temporal(&tiles, 2, 2, 32);
        assert!(
            decode_temporal(&wire[..wire.len() - 1]).is_none(),
            "body one byte short of declared grid"
        );
        assert!(
            decode_temporal(&[1, 32, 0, 2, 0, 2, APF_ENCODING_TEMPORAL]).is_none(),
            "7 bytes, header alone needs 8"
        );
    }

    #[test]
    fn decode_temporal_rejects_unsupported_encoding() {
        let map = vec![0.5f32; 4];
        let wire = encode_u4(&map, 2, 2, 32); // a real u4 packet, not temporal
        assert!(decode_temporal(&wire).is_none());
    }
}
