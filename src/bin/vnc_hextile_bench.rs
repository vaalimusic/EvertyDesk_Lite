//! Honest, local, no-infrastructure size comparison: EVRTCK (real production
//! encoder) vs. a spec-compliant VNC Hextile encoder (RFC 6143 §7.7.4), on
//! the *same* synthetic frames used by `benches/evrtck_bench.rs`'s
//! `bench_payload_size_report` scenarios.
//!
//! Why Hextile and not Tight/ZRLE: Hextile's wire format is fully specified
//! by the RFC with no server-specific JPEG-quality or zlib-level knobs, so a
//! correct implementation genuinely represents what any RFB-compliant
//! encoder would put on the wire -- there's no "our implementation happened
//! to be worse" ambiguity. It's also still what RealVNC/TigerVNC/UltraVNC
//! fall back to over plain TCP when the client doesn't negotiate Tight/ZRLE,
//! so it's a legitimate "classic VNC" baseline, not a strawman.
//!
//! Scope/honesty notes (read before citing these numbers):
//!   - Two dirty-tile fill modes are used here: `Invert` (every pixel in a
//!     dirty 32x32 EVRTCK tile is uniformly flipped -- so each 16x16 hextile
//!     sub-tile inside it is exactly one solid color) and `Noise` (every
//!     pixel is independent PRNG output -- so each 16x16 sub-tile has close
//!     to 256 distinct colors). Both are binary cases (exactly 1 color, or
//!     "too many to bother enumerating") for which Hextile's
//!     background-only vs. Raw fallback IS the complete, optimal encoding --
//!     not an approximation. A tile with e.g. 2-6 distinct colors would use
//!     Hextile's subrect encoding, which is NOT implemented here, so this
//!     tool would mis-represent genuinely mixed-color content. It does not
//!     mis-represent the two scenarios actually tested.
//!   - Clustered dirty regions are sent as ONE bounding Hextile rectangle
//!     (realistic: real servers coalesce contiguous damage into one
//!     rectangle). Scattered dirty regions are sent as one rectangle PER
//!     dirty 32x32 tile (realistic: they're not adjacent, so there's nothing
//!     to coalesce). Hextile's background/foreground "carry forward" state
//!     resets at each rectangle boundary per RFC 6143 -- modeled correctly
//!     here, which matters a lot for the clustered/solid case.
//!   - This measures payload bytes only, not RFB handshake overhead, TLS,
//!     or real network framing -- same scope as EVRTCK's own
//!     `bench_payload_size_report`, so the comparison is apples to apples.
//!
//! Run: cargo run --release --bin vnc_hextile_bench

use evertydesk_core::evrtck::{EvrtckEncoder, TILE_SIZE};

const HEXTILE_SIZE: usize = 16;
const BPP: usize = 4;

// ── Frame generators (mirrors benches/evrtck_bench.rs exactly, kept
//    self-contained here since bench binaries in this repo don't share a
//    frame-gen module and this needs to match bit-for-bit) ─────────────────

fn solid_frame(w: usize, h: usize, color: [u8; 4]) -> Vec<u8> {
    color.iter().cycle().take(w * h * 4).copied().collect()
}

fn gradient_frame(w: usize, h: usize) -> Vec<u8> {
    (0..h)
        .flat_map(|y| {
            (0..w).flat_map(move |x| {
                let r = ((x * 255) / w) as u8;
                let g = ((y * 255) / h) as u8;
                let b = (((x + y) * 255) / (w + h)) as u8;
                [b, g, r, 255u8]
            })
        })
        .collect()
}

#[derive(Clone, Copy)]
enum DirtyDistribution {
    Clustered,
    Scattered,
}

#[derive(Clone, Copy)]
enum DirtyEntropy {
    Invert,
    Noise,
}

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

fn dirty_tile_indices(
    w: usize,
    h: usize,
    dirty_fraction: f32,
    distribution: DirtyDistribution,
    seed: u64,
) -> Vec<usize> {
    let tiles_x = (w + TILE_SIZE - 1) / TILE_SIZE;
    let tiles_y = (h + TILE_SIZE - 1) / TILE_SIZE;
    let total_tiles = tiles_x * tiles_y;
    let dirty_count = ((total_tiles as f32) * dirty_fraction).round() as usize;
    let dirty_count = dirty_count.min(total_tiles);

    match distribution {
        DirtyDistribution::Clustered => (0..dirty_count).collect(),
        DirtyDistribution::Scattered => {
            let mut rng = SplitMix64::new(seed);
            let mut tiles: Vec<usize> = (0..total_tiles).collect();
            for i in (1..tiles.len()).rev() {
                let j = (rng.next_u64() as usize) % (i + 1);
                tiles.swap(i, j);
            }
            tiles.truncate(dirty_count);
            tiles.sort_unstable();
            tiles
        }
    }
}

fn dirty_frame(
    base: &[u8],
    w: usize,
    h: usize,
    dirty_fraction: f32,
    distribution: DirtyDistribution,
    entropy: DirtyEntropy,
) -> (Vec<u8>, Vec<usize>) {
    let mut frame = base.to_vec();
    let tiles_x = (w + TILE_SIZE - 1) / TILE_SIZE;
    let dirty_tiles = dirty_tile_indices(w, h, dirty_fraction, distribution, 0x4556_5254_434b);
    let mut rng = SplitMix64::new(0x4556_5254_434b_0001);

    for &tile_idx in &dirty_tiles {
        let tx = tile_idx % tiles_x;
        let ty = tile_idx / tiles_x;
        let x0 = tx * TILE_SIZE;
        let y0 = ty * TILE_SIZE;
        let x1 = (x0 + TILE_SIZE).min(w);
        let y1 = (y0 + TILE_SIZE).min(h);
        for y in y0..y1 {
            for x in x0..x1 {
                let off = (y * w + x) * 4;
                match entropy {
                    DirtyEntropy::Invert => {
                        frame[off] = 255 - base[off];
                        frame[off + 1] = 255 - base[off + 1];
                        frame[off + 2] = 255 - base[off + 2];
                    }
                    DirtyEntropy::Noise => {
                        let noise = rng.next_u64();
                        frame[off] = noise as u8;
                        frame[off + 1] = (noise >> 8) as u8;
                        frame[off + 2] = (noise >> 16) as u8;
                    }
                }
            }
        }
    }
    (frame, dirty_tiles)
}

// ── Hextile encoder (RFC 6143 §7.7.4) ───────────────────────────────────────
// Computes the byte SIZE the real wire encoding would take, tile by tile,
// tracking last-background/last-foreground carry-forward state exactly as
// the spec defines (state resets at the start of each rectangle).

// Only tracks last-background: this tool's tiles are always exactly 1 color
// (BackgroundSpecified path) or too many to enumerate (Raw path) -- see the
// module doc comment. Raw never needs foreground/subrects, and the uniform
// case never needs them either, so last-foreground carry-forward (which RFC
// 6143 uses for the AnySubrects+non-SubrectsColoured case) has nothing to
// track here.
struct HextileState {
    last_bg: Option<[u8; 4]>,
}

impl HextileState {
    fn new() -> Self {
        Self { last_bg: None }
    }
}

/// Encodes one 16x16 (or smaller, at frame edges) sub-tile and returns its
/// byte cost, updating carry-forward state.
fn hextile_subtile_cost(
    frame: &[u8],
    w: usize,
    x0: usize,
    y0: usize,
    tw: usize,
    th: usize,
    state: &mut HextileState,
) -> usize {
    // Collect distinct colors in this sub-tile (cap the set small since we
    // only need to distinguish "1 color" from "many").
    let mut first: Option<[u8; 4]> = None;
    let mut uniform = true;
    for y in y0..y0 + th {
        for x in x0..x0 + tw {
            let off = (y * w + x) * BPP;
            let px = [frame[off], frame[off + 1], frame[off + 2], frame[off + 3]];
            match first {
                None => first = Some(px),
                Some(f) if f == px => {}
                Some(_) => {
                    uniform = false;
                    break;
                }
            }
        }
        if !uniform {
            break;
        }
    }

    if uniform {
        let color = first.unwrap_or([0, 0, 0, 0]);
        let mut cost = 1; // subencoding byte (BackgroundSpecified bit, or 0 if bg unchanged)
        if state.last_bg != Some(color) {
            cost += BPP; // background pixel value follows
            state.last_bg = Some(color);
        }
        cost
    } else {
        // Raw fallback: subencoding byte + full sub-tile pixel data. RFC
        // 6143 §7.7.4: "the background pixel value may not be carried over
        // if the previous tile was raw" -- so a Raw tile invalidates
        // carry-forward for whatever tile comes after it, forcing the next
        // uniform tile to re-specify its background explicitly even if the
        // color matches what was last established before the Raw tile.
        state.last_bg = None;
        1 + tw * th * BPP
    }
}

/// Encodes one Hextile rectangle (RFC: 12-byte header + tiles, state resets
/// at rectangle start) and returns its total byte cost including the header.
fn hextile_rect_cost(frame: &[u8], w: usize, rx0: usize, ry0: usize, rw: usize, rh: usize) -> usize {
    let mut state = HextileState::new();
    let mut cost = 12usize; // x, y, w, h (u16 x4) + encoding-type (i32)
    let mut y = ry0;
    while y < ry0 + rh {
        let th = HEXTILE_SIZE.min(ry0 + rh - y);
        let mut x = rx0;
        while x < rx0 + rw {
            let tw = HEXTILE_SIZE.min(rx0 + rw - x);
            cost += hextile_subtile_cost(frame, w, x, y, tw, th, &mut state);
            x += HEXTILE_SIZE;
        }
        y += HEXTILE_SIZE;
    }
    cost
}

const FRAMEBUFFER_UPDATE_HEADER: usize = 4; // message-type(1) + padding(1) + num-rects(2)

/// Encodes a full FramebufferUpdate for a set of dirty EVRTCK tiles (32x32
/// each), choosing the coalescing strategy documented at the top of this
/// file: one bounding rect for Clustered, one rect per tile for Scattered.
fn hextile_update_cost(
    frame: &[u8],
    w: usize,
    h: usize,
    dirty_tiles: &[usize],
    tiles_x: usize,
    distribution: DirtyDistribution,
) -> usize {
    if dirty_tiles.is_empty() {
        return FRAMEBUFFER_UPDATE_HEADER; // zero rectangles, header only
    }
    match distribution {
        DirtyDistribution::Clustered => {
            let (mut min_tx, mut min_ty, mut max_tx, mut max_ty) =
                (usize::MAX, usize::MAX, 0usize, 0usize);
            for &idx in dirty_tiles {
                let tx = idx % tiles_x;
                let ty = idx / tiles_x;
                min_tx = min_tx.min(tx);
                min_ty = min_ty.min(ty);
                max_tx = max_tx.max(tx);
                max_ty = max_ty.max(ty);
            }
            let rx0 = min_tx * TILE_SIZE;
            let ry0 = min_ty * TILE_SIZE;
            // Clamp to the actual frame: the last row/column of EVRTCK tiles
            // can overhang the real w/h when they aren't multiples of
            // TILE_SIZE (e.g. 1080 / 32 = 33.75), same as dirty_frame's own
            // fill loop already clamps per-tile x1/y1.
            let rw = ((max_tx - min_tx + 1) * TILE_SIZE).min(w - rx0);
            let rh = ((max_ty - min_ty + 1) * TILE_SIZE).min(h - ry0);
            FRAMEBUFFER_UPDATE_HEADER + hextile_rect_cost(frame, w, rx0, ry0, rw, rh)
        }
        DirtyDistribution::Scattered => {
            let mut total = FRAMEBUFFER_UPDATE_HEADER;
            for &idx in dirty_tiles {
                let tx = idx % tiles_x;
                let ty = idx / tiles_x;
                let x0 = tx * TILE_SIZE;
                let y0 = ty * TILE_SIZE;
                let tw = TILE_SIZE.min(w - x0);
                let th = TILE_SIZE.min(h - y0);
                total += hextile_rect_cost(frame, w, x0, y0, tw, th);
            }
            total
        }
    }
}

// ── Real byte-writing Hextile encoder (for timing, not just size) ──────────
// Mirrors the *_cost functions above exactly, field for field, but actually
// writes the wire bytes instead of just counting them -- so timing this
// against EvrtckEncoder::encode is a fair "real work done" comparison, not
// "arithmetic vs real compression". RFB integers are big-endian ("network
// byte order", RFC 6143 §7) -- to_be_bytes() throughout, not to_ne_bytes().

fn hextile_subtile_encode(
    frame: &[u8],
    w: usize,
    x0: usize,
    y0: usize,
    tw: usize,
    th: usize,
    state: &mut HextileState,
    out: &mut Vec<u8>,
) {
    let mut first: Option<[u8; 4]> = None;
    let mut uniform = true;
    for y in y0..y0 + th {
        for x in x0..x0 + tw {
            let off = (y * w + x) * BPP;
            let px = [frame[off], frame[off + 1], frame[off + 2], frame[off + 3]];
            match first {
                None => first = Some(px),
                Some(f) if f == px => {}
                Some(_) => {
                    uniform = false;
                    break;
                }
            }
        }
        if !uniform {
            break;
        }
    }

    if uniform {
        let color = first.unwrap_or([0, 0, 0, 0]);
        if state.last_bg != Some(color) {
            out.push(0x02); // BackgroundSpecified
            out.extend_from_slice(&color);
            state.last_bg = Some(color);
        } else {
            out.push(0x00); // nothing changed, carry forward
        }
    } else {
        out.push(0x01); // Raw
        for y in y0..y0 + th {
            let row_start = (y * w + x0) * BPP;
            out.extend_from_slice(&frame[row_start..row_start + tw * BPP]);
        }
        state.last_bg = None;
    }
}

fn hextile_rect_encode(
    frame: &[u8],
    w: usize,
    rx0: usize,
    ry0: usize,
    rw: usize,
    rh: usize,
    out: &mut Vec<u8>,
) {
    let mut state = HextileState::new();
    out.extend_from_slice(&(rx0 as u16).to_be_bytes());
    out.extend_from_slice(&(ry0 as u16).to_be_bytes());
    out.extend_from_slice(&(rw as u16).to_be_bytes());
    out.extend_from_slice(&(rh as u16).to_be_bytes());
    out.extend_from_slice(&5i32.to_be_bytes()); // encoding-type 5 = Hextile
    let mut y = ry0;
    while y < ry0 + rh {
        let th = HEXTILE_SIZE.min(ry0 + rh - y);
        let mut x = rx0;
        while x < rx0 + rw {
            let tw = HEXTILE_SIZE.min(rx0 + rw - x);
            hextile_subtile_encode(frame, w, x, y, tw, th, &mut state, out);
            x += HEXTILE_SIZE;
        }
        y += HEXTILE_SIZE;
    }
}

/// Scans `frame` against `prev` tile by tile (EVRTCK's own TILE_SIZE grid,
/// so both codecs are charged for detecting changes at the same
/// granularity) and returns the indices of tiles that differ. This is real
/// work a VNC server also has to do -- it doesn't get handed a list of what
/// changed for free any more than EVRTCK's encoder does. Only used on the
/// timed path; the size-comparison passes above reuse the dirty-tile list
/// the frame generator already computed, since that part isn't about timing.
fn detect_dirty_tiles(prev: &[u8], frame: &[u8], w: usize, h: usize) -> Vec<usize> {
    let tiles_x = (w + TILE_SIZE - 1) / TILE_SIZE;
    let tiles_y = (h + TILE_SIZE - 1) / TILE_SIZE;
    let mut dirty = Vec::new();
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let x0 = tx * TILE_SIZE;
            let y0 = ty * TILE_SIZE;
            let x1 = (x0 + TILE_SIZE).min(w);
            let y1 = (y0 + TILE_SIZE).min(h);
            let mut changed = false;
            'rows: for y in y0..y1 {
                let row_start = (y * w + x0) * BPP;
                let row_end = (y * w + x1) * BPP;
                if prev[row_start..row_end] != frame[row_start..row_end] {
                    changed = true;
                    break 'rows;
                }
            }
            if changed {
                dirty.push(ty * tiles_x + tx);
            }
        }
    }
    dirty
}

fn hextile_update_encode(
    frame: &[u8],
    w: usize,
    h: usize,
    dirty_tiles: &[usize],
    tiles_x: usize,
    distribution: DirtyDistribution,
    out: &mut Vec<u8>,
) {
    out.push(0); // message-type FramebufferUpdate
    out.push(0); // padding
    if dirty_tiles.is_empty() {
        out.extend_from_slice(&0u16.to_be_bytes());
        return;
    }
    match distribution {
        DirtyDistribution::Clustered => {
            out.extend_from_slice(&1u16.to_be_bytes());
            let (mut min_tx, mut min_ty, mut max_tx, mut max_ty) =
                (usize::MAX, usize::MAX, 0usize, 0usize);
            for &idx in dirty_tiles {
                let tx = idx % tiles_x;
                let ty = idx / tiles_x;
                min_tx = min_tx.min(tx);
                min_ty = min_ty.min(ty);
                max_tx = max_tx.max(tx);
                max_ty = max_ty.max(ty);
            }
            let rx0 = min_tx * TILE_SIZE;
            let ry0 = min_ty * TILE_SIZE;
            let rw = ((max_tx - min_tx + 1) * TILE_SIZE).min(w - rx0);
            let rh = ((max_ty - min_ty + 1) * TILE_SIZE).min(h - ry0);
            hextile_rect_encode(frame, w, rx0, ry0, rw, rh, out);
        }
        DirtyDistribution::Scattered => {
            out.extend_from_slice(&(dirty_tiles.len() as u16).to_be_bytes());
            for &idx in dirty_tiles {
                let tx = idx % tiles_x;
                let ty = idx / tiles_x;
                let x0 = tx * TILE_SIZE;
                let y0 = ty * TILE_SIZE;
                let tw = TILE_SIZE.min(w - x0);
                let th = TILE_SIZE.min(h - y0);
                hextile_rect_encode(frame, w, x0, y0, tw, th, out);
            }
        }
    }
}

fn main() {
    // ── Keyframes across resolutions (matches bench_keyframes in
    //    benches/evrtck_bench.rs: 720p/1080p/4k) ─────────────────────────
    println!("-- keyframes (first frame, nothing to diff against) --");
    println!("scenario,evrtck_bytes,hextile_bytes,evrtck_vs_hextile_ratio");
    for (res_name, rw, rh) in [("720p", 1280usize, 720usize), ("1080p", 1920, 1080), ("4k", 3840, 2160)] {
        let res_tiles_x = (rw + TILE_SIZE - 1) / TILE_SIZE;
        let res_base = solid_frame(rw, rh, [30, 30, 30, 255]);
        for (kind, frame) in [
            ("solid", res_base.clone()),
            ("gradient", gradient_frame(rw, rh)),
        ] {
            let mut enc = EvrtckEncoder::new(rw, rh);
            let pkt = enc.encode(&frame, 1);
            let evrtck_bytes = pkt.data.len();
            let all_tiles: Vec<usize> =
                (0..(res_tiles_x * ((rh + TILE_SIZE - 1) / TILE_SIZE))).collect();
            let hextile_bytes = hextile_update_cost(
                &frame,
                rw,
                rh,
                &all_tiles,
                res_tiles_x,
                DirtyDistribution::Clustered,
            );
            let ratio = hextile_bytes as f64 / evrtck_bytes.max(1) as f64;
            println!("keyframe_{kind}_{res_name},{evrtck_bytes},{hextile_bytes},{ratio:.2}");
        }
    }

    // ── P-frames, same scenario matrix as bench_payload_size_report, plus
    //    clustered_noise (a solid noisy/video region -- e.g. a video
    //    playing in a fixed window -- which wasn't covered before: the
    //    original matrix only paired Clustered with Invert and Scattered
    //    with both Invert and Noise) ───────────────────────────────────────
    let (w, h) = (1920usize, 1080usize);
    let raw_bytes = w * h * 4;
    let tiles_x = (w + TILE_SIZE - 1) / TILE_SIZE;
    let base = solid_frame(w, h, [30, 30, 30, 255]);

    println!("\n-- P-frames (delta against a solid {w}x{h} base frame, raw={raw_bytes} bytes) --");
    println!("scenario,evrtck_bytes,hextile_bytes,evrtck_vs_hextile_ratio");
    let scenarios: &[(&str, f32, DirtyDistribution, DirtyEntropy)] = &[
        ("static_0pct", 0.00, DirtyDistribution::Clustered, DirtyEntropy::Invert),
        ("clustered_invert_5pct", 0.05, DirtyDistribution::Clustered, DirtyEntropy::Invert),
        ("clustered_invert_15pct", 0.15, DirtyDistribution::Clustered, DirtyEntropy::Invert),
        ("clustered_invert_50pct", 0.50, DirtyDistribution::Clustered, DirtyEntropy::Invert),
        ("clustered_invert_90pct", 0.90, DirtyDistribution::Clustered, DirtyEntropy::Invert),
        ("scattered_invert_5pct", 0.05, DirtyDistribution::Scattered, DirtyEntropy::Invert),
        ("scattered_invert_15pct", 0.15, DirtyDistribution::Scattered, DirtyEntropy::Invert),
        ("scattered_invert_50pct", 0.50, DirtyDistribution::Scattered, DirtyEntropy::Invert),
        ("scattered_noise_5pct", 0.05, DirtyDistribution::Scattered, DirtyEntropy::Noise),
        ("scattered_noise_15pct", 0.15, DirtyDistribution::Scattered, DirtyEntropy::Noise),
        ("scattered_noise_50pct", 0.50, DirtyDistribution::Scattered, DirtyEntropy::Noise),
        ("scattered_noise_90pct", 0.90, DirtyDistribution::Scattered, DirtyEntropy::Noise),
        ("clustered_noise_5pct", 0.05, DirtyDistribution::Clustered, DirtyEntropy::Noise),
        ("clustered_noise_15pct", 0.15, DirtyDistribution::Clustered, DirtyEntropy::Noise),
        ("clustered_noise_50pct", 0.50, DirtyDistribution::Clustered, DirtyEntropy::Noise),
        ("clustered_noise_90pct", 0.90, DirtyDistribution::Clustered, DirtyEntropy::Noise),
    ];

    for (name, dirty_frac, distribution, entropy) in scenarios {
        let (frame, dirty_tiles) = dirty_frame(&base, w, h, *dirty_frac, *distribution, *entropy);

        let mut enc = EvrtckEncoder::new(w, h);
        enc.encode(&base, 1);
        let pkt = enc.encode(&frame, 2);
        let evrtck_bytes = pkt.data.len();

        let hextile_bytes = hextile_update_cost(&frame, w, h, &dirty_tiles, tiles_x, *distribution);
        let ratio = hextile_bytes as f64 / evrtck_bytes.max(1) as f64;
        println!("{name},{evrtck_bytes},{hextile_bytes},{ratio:.2}");
    }

    // ── Encode TIME, not just size ──────────────────────────────────────
    // Everything above compared payload bytes only. The original dispute
    // was about speed, so this times real work end to end: change
    // detection + encode for BOTH sides. EvrtckEncoder::encode() does its
    // own dirty-tile scan internally; hextile_update_encode() was being fed
    // a pre-computed dirty-tile list from the frame generator, which is
    // NOT real work a VNC server would get for free -- it has to scan for
    // changes itself too. Timed Hextile runs now call detect_dirty_tiles()
    // first, same as a real server pixel-diffing its framebuffer, so both
    // sides pay for detection at the same tile granularity. 200 iterations
    // per scenario, warmed up first so the timed runs don't include
    // one-time allocator/cache warmup; median reported since it's robust to
    // the occasional OS scheduler hiccup a mean isn't.
    const ITERATIONS: usize = 200;
    const WARMUP: usize = 20;

    fn median_ns(mut samples: Vec<u64>) -> u64 {
        samples.sort_unstable();
        samples[samples.len() / 2]
    }

    println!("\n-- encode time, median of {ITERATIONS} runs ({WARMUP} warmup, not counted) --");
    println!("-- both sides include change detection: EVRTCK does it inside encode(), Hextile via detect_dirty_tiles() --");
    println!("scenario,evrtck_us,hextile_us,evrtck_vs_hextile_time_ratio");
    for (name, dirty_frac, distribution, entropy) in scenarios {
        let (frame, _) = dirty_frame(&base, w, h, *dirty_frac, *distribution, *entropy);

        for _ in 0..WARMUP {
            let mut enc = EvrtckEncoder::new(w, h);
            enc.encode(&base, 1);
            std::hint::black_box(enc.encode(&frame, 2));
            let detected = detect_dirty_tiles(&base, &frame, w, h);
            let mut out = Vec::new();
            hextile_update_encode(&frame, w, h, &detected, tiles_x, *distribution, &mut out);
            std::hint::black_box(&out);
        }

        let mut evrtck_samples = Vec::with_capacity(ITERATIONS);
        for _ in 0..ITERATIONS {
            let mut enc = EvrtckEncoder::new(w, h);
            enc.encode(&base, 1); // keyframe not timed -- only the P-frame delta is
            let start = std::time::Instant::now();
            let pkt = enc.encode(&frame, 2);
            evrtck_samples.push(start.elapsed().as_nanos() as u64);
            std::hint::black_box(pkt);
        }

        let mut hextile_samples = Vec::with_capacity(ITERATIONS);
        for _ in 0..ITERATIONS {
            let start = std::time::Instant::now();
            let detected = detect_dirty_tiles(&base, &frame, w, h);
            let mut out = Vec::new();
            hextile_update_encode(&frame, w, h, &detected, tiles_x, *distribution, &mut out);
            hextile_samples.push(start.elapsed().as_nanos() as u64);
            std::hint::black_box(&out);
        }

        let evrtck_us = median_ns(evrtck_samples) as f64 / 1000.0;
        let hextile_us = median_ns(hextile_samples) as f64 / 1000.0;
        let ratio = evrtck_us / hextile_us.max(0.001);
        println!("{name},{evrtck_us:.2},{hextile_us:.2},{ratio:.2}");
    }
}
