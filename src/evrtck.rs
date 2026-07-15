//! EVRTCK — EvertyDesk Remote Transport Codec
//!
//! Tile-based lossless delta codec built for desktop/UI screen content.
//! No external dependencies; pure Rust.
//!
//! # Why not H.264 / H.265?
//!
//! Video codecs are designed for natural footage where every macroblock changes
//! every frame. Desktop content is the opposite: 80-90 % of tiles are pixel-
//! identical between frames. H.264 still processes them all.  EVRTCK costs
//! exactly 1 bit per static tile. Changed tiles go through:
//!
//!   1. Solid-color fast path (5 bytes per tile).
//!   2. XOR delta against the previous frame compressed with the better of ZRLE
//!      or zstd level-1. ZRLE wins on sparse deltas (mostly-zero XOR patterns
//!      typical for P-frames); zstd wins on keyframe tiles (raw pixel data).
//!
//! # Wire format
//!
//! ```text
//! Frame header (20 bytes)
//!   magic      [u8; 4]  = b"EVCK"
//!   version    u8       = 1
//!   flags      u8       (reserved, must be 0)
//!   frame_id   u32 LE
//!   width      u32 LE
//!   height     u32 LE
//!   map_bytes  u16 LE   — byte length of the tile dirty-map that follows
//!
//! Tile dirty-map
//!   One bit per tile (LSB first), 1 = tile changed.
//!   Padded to the next byte boundary.
//!
//! Tile data  (for each dirty tile, in raster order)
//!   mode  u8
//!     MODE_SOLID  = 1 → color [u8; 4] (RGBA)
//!     MODE_DELTA  = 2 → len u32 LE, then ZRLE-encoded XOR delta
//!     MODE_ZSTD   = 3 → len u32 LE, then zstd-compressed XOR delta
//! ```

use std::fmt;
use rayon::prelude::*;

// ── Constants ────────────────────────────────────────────────────────────────

pub const MAGIC: &[u8; 4] = b"EVCK";
pub const VERSION: u8 = 1;

/// Pixels per tile edge. 32×32 = 1024 px, maps well onto L1 cache lines.
pub const TILE_SIZE: usize = 32;

/// Below this tile count, sequential encode beats rayon (spawn overhead ~0.3 ms).
/// Roughly equivalent to a ~256×256 source frame (64 tiles of 32×32).
const RAYON_THRESHOLD: usize = 64;

/// Minimum zero-run length to justify a ZRLE ZeroRun token (3-byte overhead).
/// Runs of ≥4 zeros save ≥1 byte vs including them as literals.
const ZRLE_MIN_RUN: usize = 4;

const MODE_SOLID: u8 = 1;
const MODE_DELTA: u8 = 2;
/// XOR delta compressed with zstd level-1. Better than ZRLE on non-sparse data (keyframes).
const MODE_ZSTD: u8 = 3;

// ── Error ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum EvrtckError {
    InvalidMagic,
    UnsupportedVersion(u8),
    TruncatedData,
    DimensionMismatch { expected: (u32, u32), got: (u32, u32) },
    InvalidTileMode(u8),
    InvalidDelta,
}

impl fmt::Display for EvrtckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "invalid EVCK magic bytes"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported EVRTCK version {v}"),
            Self::TruncatedData => write!(f, "truncated EVRTCK frame"),
            Self::DimensionMismatch { expected, got } => {
                write!(f, "dimension mismatch: expected {expected:?}, got {got:?}")
            }
            Self::InvalidTileMode(m) => write!(f, "unknown tile mode 0x{m:02x}"),
            Self::InvalidDelta => write!(f, "malformed delta stream (ZRLE or zstd)"),
        }
    }
}

impl std::error::Error for EvrtckError {}

// ── Encoded packet ───────────────────────────────────────────────────────────

/// An encoded frame ready to be wrapped in an EVRT `TYPE_VIDEO_FRAME` packet.
#[derive(Debug, Clone)]
pub struct EvrtckPacket {
    pub frame_id: u32,
    pub width: u32,
    pub height: u32,
    /// Encoded bytes. Zero-copy: hand directly to the EVRT sender.
    pub data: Vec<u8>,
}

impl EvrtckPacket {
    /// Ratio of encoded size to raw RGBA size. Values near 0.0 mean mostly static frame.
    pub fn compression_ratio(&self) -> f32 {
        let raw = self.width as usize * self.height as usize * 4;
        if raw == 0 { return 1.0; }
        self.data.len() as f32 / raw as f32
    }
}

// ── Frame stats ──────────────────────────────────────────────────────────────

/// Per-frame encoding telemetry. Useful for adaptive bitrate / debug.
#[derive(Debug, Clone, Default)]
pub struct FrameStats {
    pub total_tiles: u32,
    pub dirty_tiles: u32,
    pub solid_tiles: u32,
    pub delta_tiles: u32,
    pub encoded_bytes: u32,
}

impl FrameStats {
    pub fn dirty_ratio(&self) -> f32 {
        if self.total_tiles == 0 { return 0.0; }
        self.dirty_tiles as f32 / self.total_tiles as f32
    }
}

// Wire flags byte (offset 5 in header).
pub(crate) const FLAG_KEYFRAME: u8 = 0x01;
// NOP frame: cur == prev, frame buffer unchanged. No tile map or payload.
pub(crate) const FLAG_NOP: u8 = 0x02;

// ── Backend trait ─────────────────────────────────────────────────────────────

/// Pluggable encoder backend.
///
/// CPU is always available and is the universal fallback. GPU backends
/// (WGPU compute shaders, platform-specific APIs) activate at runtime when
/// supported hardware is detected — zero-cost when absent.
///
/// # Adding a new backend
///
/// 1. Implement this trait for your backend struct.
/// 2. Guard it with `#[cfg(feature = "gpu-accel")]` or a platform `#[cfg(target_os)]`.
/// 3. Add a try-new probe in `new_backend()` before the CPU fallback.
///
/// The decoder side remains CPU-only — decoding is already O(dirty_tiles)
/// and typically runs on a remote machine with unknown hardware.
pub trait EvrtckEncoderBackend: Send {
    /// Encode one BGRA frame. Returns an encoded packet and per-frame stats.
    fn encode_inner(&mut self, bgra: &[u8], frame_id: u32) -> (EvrtckPacket, FrameStats);
    /// Signal that the next frame must be a full keyframe (resets prev to black).
    fn request_keyframe(&mut self);
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    /// Fast dirty-tile pre-scan (no compression). GPU backends may override with
    /// a compute shader. Default: O(W×H) CPU compare.
    fn dirty_ratio(&self, bgra: &[u8]) -> f32;
}

// ── CPU backend — always available, no GPU required ───────────────────────────

struct CpuEvrtckEncoder {
    prev: Vec<u8>,
    width: usize,
    height: usize,
    pending_keyframe: bool,
}

impl CpuEvrtckEncoder {
    fn new(width: usize, height: usize) -> Self {
        Self {
            prev: vec![0u8; width * height * 4],
            width,
            height,
            pending_keyframe: true, // first frame is always a keyframe
        }
    }
}

impl EvrtckEncoderBackend for CpuEvrtckEncoder {
    fn encode_inner(&mut self, bgra: &[u8], frame_id: u32) -> (EvrtckPacket, FrameStats) {
        debug_assert_eq!(bgra.len(), self.width * self.height * 4);
        let is_kf = self.pending_keyframe;
        self.pending_keyframe = false;
        let (data, stats) = encode_frame(bgra, &self.prev, self.width, self.height, frame_id, is_kf);
        // Skip memcpy on NOP frames (bgra == prev, copy would do nothing). Always
        // copy on keyframes so the next P-frame diffs against the correct baseline.
        if stats.dirty_tiles > 0 || is_kf {
            self.prev.copy_from_slice(bgra);
        }
        // Auto-escalate: if this P-frame was overwhelmingly dirty, force a keyframe
        // next turn to reset the XOR baseline and improve future delta compression.
        if !is_kf && stats.total_tiles > 0 && stats.dirty_tiles * 10 >= stats.total_tiles * 9 {
            self.pending_keyframe = true;
        }
        let pkt = EvrtckPacket {
            frame_id,
            width: self.width as u32,
            height: self.height as u32,
            data,
        };
        (pkt, stats)
    }

    fn request_keyframe(&mut self) {
        self.prev.fill(0);
        self.pending_keyframe = true;
    }

    fn width(&self) -> usize { self.width }
    fn height(&self) -> usize { self.height }

    fn dirty_ratio(&self, bgra: &[u8]) -> f32 {
        if bgra == self.prev { return 0.0; }
        let tiles_x = tiles_in_dim(self.width);
        let tiles_y = tiles_in_dim(self.height);
        let total = tiles_x * tiles_y;
        if total == 0 { return 0.0; }
        let mut dirty = 0u32;
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                if tile_is_dirty(bgra, &self.prev, self.width, self.height, tx, ty) {
                    dirty += 1;
                }
            }
        }
        dirty as f32 / total as f32
    }
}

// ── Backend factory — runtime hardware detection ───────────────────────────────

/// Returns the best available encoder backend for this machine.
///
/// Probe order (first success wins):
///   1. WGPU compute  (`gpu-accel` feature, cross-platform Vulkan/Metal/DX12)
///   2. Platform-native (future: DXGI-zero-copy on Windows, IOSurface on macOS)
///   3. CPU rayon     (always available — universal fallback)
fn new_backend(width: usize, height: usize) -> Box<dyn EvrtckEncoderBackend> {
    // ── GPU backends ─────────────────────────────────────────────────────────
    // Probe order: WGPU (cross-platform) → platform-native (future) → CPU.
    #[cfg(feature = "gpu-accel")]
    if let Some(gpu) = crate::evrtck_wgpu::WgpuEvrtckEncoder::try_new(width, height) {
        return Box::new(gpu);
    }
    // Future: DXGI zero-copy (Windows) — no PCIe roundtrip for captured frames.
    // #[cfg(all(target_os = "windows", feature = "dxgi-zero-copy"))]
    // if let Some(d) = crate::evrtck_dxgi::DxgiEvrtckEncoder::try_new(width, height) {
    //     return Box::new(d);
    // }

    Box::new(CpuEvrtckEncoder::new(width, height))
}

// ── Stateful encoder (public facade) ──────────────────────────────────────────

/// EVRTCK encoder. Wraps the best available backend transparently.
///
/// On machines with a supported GPU and `gpu-accel` feature, the inner backend
/// uses compute shaders for XOR-diff computation; on everything else it falls
/// back to the rayon CPU path. The public API is identical in both cases.
pub struct EvrtckEncoder {
    inner: Box<dyn EvrtckEncoderBackend>,
}

impl EvrtckEncoder {
    /// Create encoder, probing for the best available backend automatically.
    pub fn new(width: usize, height: usize) -> Self {
        Self { inner: new_backend(width, height) }
    }

    pub fn encode_with_stats(&mut self, rgba: &[u8], frame_id: u32) -> (EvrtckPacket, FrameStats) {
        self.inner.encode_inner(rgba, frame_id)
    }

    pub fn encode(&mut self, rgba: &[u8], frame_id: u32) -> EvrtckPacket {
        self.inner.encode_inner(rgba, frame_id).0
    }

    /// Force-key: decoder will reset its frame buffer before applying this frame.
    pub fn request_keyframe(&mut self) {
        self.inner.request_keyframe();
    }

    /// Fast dirty-tile scan with no compression. Call BEFORE encode() to
    /// decide whether EVRTCK is worthwhile. Cost: O(W×H) pixel compares only
    /// on CPU backend; compute shader on GPU backend.
    pub fn dirty_ratio(&self, rgba: &[u8]) -> f32 {
        self.inner.dirty_ratio(rgba)
    }

    pub fn width(&self) -> usize { self.inner.width() }
    pub fn height(&self) -> usize { self.inner.height() }
}

// ── Stateful decoder ─────────────────────────────────────────────────────────

#[derive(Default)]
pub struct EvrtckDecoder {
    frame: Vec<u8>,
    width: usize,
    height: usize,
}

impl EvrtckDecoder {
    pub fn new() -> Self { Self::default() }

    /// Decode a packet into the internal frame buffer. Returns a slice of the
    /// reconstructed RGBA frame. The slice is valid until the next `decode` call.
    pub fn decode(&mut self, pkt: &EvrtckPacket) -> Result<&[u8], EvrtckError> {
        self.decode_wire(&pkt.data)
    }

    /// Decode raw wire bytes — self-describing, reads dimensions from the header.
    /// Use this when the caller doesn't know the exact encoded dimensions.
    pub fn decode_wire(&mut self, data: &[u8]) -> Result<&[u8], EvrtckError> {
        // Wire header: magic(4) + ver(1) + flags(1) + frame_id(4) + w(4) + h(4) = 18 bytes minimum
        if data.len() < 18 { return Err(EvrtckError::TruncatedData); }
        if &data[0..4] != MAGIC { return Err(EvrtckError::InvalidMagic); }
        let w = u32::from_le_bytes(data[10..14].try_into().unwrap()) as usize;
        let h = u32::from_le_bytes(data[14..18].try_into().unwrap()) as usize;
        if self.width != w || self.height != h || self.frame.len() != w * h * 4 {
            self.frame = vec![0u8; w * h * 4];
            self.width = w;
            self.height = h;
        }
        decode_frame(data, &mut self.frame, w, h)?;
        Ok(&self.frame)
    }

    /// Reset decoder state (e.g. after requesting a keyframe).
    pub fn reset(&mut self) {
        self.frame.fill(0);
    }

    pub fn current_frame(&self) -> &[u8] { &self.frame }
    pub fn width(&self) -> usize { self.width }
    pub fn height(&self) -> usize { self.height }
}

// ── Core: encode ─────────────────────────────────────────────────────────────

pub(crate) fn encode_frame(
    rgba: &[u8],
    prev: &[u8],
    width: usize,
    height: usize,
    frame_id: u32,
    is_keyframe: bool,
) -> (Vec<u8>, FrameStats) {
    let tiles_x = tiles_in_dim(width);
    let tiles_y = tiles_in_dim(height);
    let tile_count = tiles_x * tiles_y;

    // Fast identical-frame check before the expensive rayon scan.
    // One memcmp of the whole buffer (~0.15 ms at 1080p) vs tile scan (~3.2 ms).
    // Fires whenever the screen is static — very common in typical desktop use.
    if !is_keyframe && rgba == prev {
        let mut out = Vec::with_capacity(20);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.push(FLAG_NOP);
        out.extend_from_slice(&frame_id.to_le_bytes());
        out.extend_from_slice(&(width as u32).to_le_bytes());
        out.extend_from_slice(&(height as u32).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        return (out, FrameStats {
            total_tiles: tile_count as u32,
            dirty_tiles: 0,
            solid_tiles: 0,
            delta_tiles: 0,
            encoded_bytes: 20,
        });
    }

    // Encode strategy:
    //
    // Keyframe: every tile is encoded — one rayon pass, no dirty check needed.
    //
    // P-frame: one rayon pass that does dirty check + encode together. This is
    // faster than a separate sequential dirty scan because:
    //   1. No extra full-frame scan pass (~2.5 ms at 1080p).
    //   2. Rayon spreads dirty-check work across cores in the same sweep.
    //
    // Sequential fallback only when tile_count is tiny (< RAYON_THRESHOLD), where
    // rayon spawn overhead (~0.3 ms) would dwarf the encode work itself.
    let tile_results: Vec<Option<(Vec<u8>, u8)>> = if is_keyframe {
        (0..tile_count)
            .into_par_iter()
            .map(|idx| Some(encode_tile_buf(rgba, prev, width, height, idx % tiles_x, idx / tiles_x, true)))
            .collect()
    } else if tile_count < RAYON_THRESHOLD {
        // Very small resolution (< ~256×256) — sequential encode, rayon overhead not worth it.
        (0..tile_count)
            .map(|idx| {
                if tile_is_dirty(rgba, prev, width, height, idx % tiles_x, idx / tiles_x) {
                    Some(encode_tile_buf(rgba, prev, width, height, idx % tiles_x, idx / tiles_x, false))
                } else {
                    None
                }
            })
            .collect()
    } else {
        // Single rayon pass: dirty check + encode in parallel, no pre-scan overhead.
        (0..tile_count)
            .into_par_iter()
            .map(|idx| {
                if tile_is_dirty(rgba, prev, width, height, idx % tiles_x, idx / tiles_x) {
                    Some(encode_tile_buf(rgba, prev, width, height, idx % tiles_x, idx / tiles_x, false))
                } else {
                    None
                }
            })
            .collect()
    };

    // Build dirty-map and collect stats.
    let map_bytes = (tile_count + 7) / 8;
    let mut tile_map = vec![0u8; map_bytes];
    let mut dirty_count = 0u32;
    let mut solid_count = 0u32;
    let mut delta_count = 0u32;
    for (i, result) in tile_results.iter().enumerate() {
        if let Some((_, mode)) = result {
            tile_map[i / 8] |= 1 << (i % 8);
            dirty_count += 1;
            match *mode {
                MODE_SOLID => solid_count += 1,
                MODE_DELTA | MODE_ZSTD => delta_count += 1,
                _ => {}
            }
        }
    }

    // Assemble final packet.
    // Capacity estimate: keyframe tiles compress to ~200 bytes each (zstd on raw RGBA);
    // P-frame dirty tiles average ~30 bytes each (ZRLE on sparse XOR delta).
    let bytes_per_tile = if is_keyframe { 200 } else { 30 };
    let mut out = Vec::with_capacity(20 + map_bytes + dirty_count as usize * bytes_per_tile);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(if is_keyframe { FLAG_KEYFRAME } else { 0 });
    out.extend_from_slice(&frame_id.to_le_bytes());
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    out.extend_from_slice(&(map_bytes as u16).to_le_bytes());
    out.extend_from_slice(&tile_map);
    for result in &tile_results {
        if let Some((encoded, _)) = result {
            out.extend_from_slice(encoded);
        }
    }

    let stats = FrameStats {
        total_tiles: tile_count as u32,
        dirty_tiles: dirty_count,
        solid_tiles: solid_count,
        delta_tiles: delta_count,
        encoded_bytes: out.len() as u32,
    };
    (out, stats)
}

#[cfg_attr(not(feature = "gpu-accel"), allow(dead_code))]
pub(crate) fn nop_packet_data(frame_id: u32, width: usize, height: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(20);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(FLAG_NOP);
    out.extend_from_slice(&frame_id.to_le_bytes());
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// Encode a P-frame given a pre-computed dirty tile index list.
///
/// Called by the GPU backend (Phase 2) which detects dirty tiles on the GPU
/// and hands off only the dirty indices for CPU compression. This avoids
#[cfg_attr(not(feature = "gpu-accel"), allow(dead_code))]
pub(crate) fn encode_pframe_from_dirty_indices(
    rgba: &[u8],
    prev: &[u8],
    width: usize,
    height: usize,
    frame_id: u32,
    dirty_indices: Vec<usize>,
) -> (Vec<u8>, FrameStats) {
    let tiles_x = tiles_in_dim(width);
    let tile_count = tiles_in_dim(width) * tiles_in_dim(height);

    let mut tile_results = vec![None::<(Vec<u8>, u8)>; tile_count];

    if dirty_indices.len() < RAYON_THRESHOLD {
        for &idx in &dirty_indices {
            tile_results[idx] = Some(encode_tile_buf(rgba, prev, width, height, idx % tiles_x, idx / tiles_x, false));
        }
    } else {
        let encoded: Vec<(usize, Vec<u8>, u8)> = dirty_indices
            .into_par_iter()
            .map(|idx| {
                let (data, mode) = encode_tile_buf(rgba, prev, width, height, idx % tiles_x, idx / tiles_x, false);
                (idx, data, mode)
            })
            .collect();
        for (idx, data, mode) in encoded {
            tile_results[idx] = Some((data, mode));
        }
    }

    let map_bytes = (tile_count + 7) / 8;
    let mut tile_map = vec![0u8; map_bytes];
    let mut dirty_count = 0u32;
    let mut solid_count = 0u32;
    let mut delta_count = 0u32;
    for (i, result) in tile_results.iter().enumerate() {
        if let Some((_, mode)) = result {
            tile_map[i / 8] |= 1 << (i % 8);
            dirty_count += 1;
            match *mode {
                MODE_SOLID => solid_count += 1,
                MODE_DELTA | MODE_ZSTD => delta_count += 1,
                _ => {}
            }
        }
    }

    let mut out = Vec::with_capacity(20 + map_bytes + dirty_count as usize * 72);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(0); // P-frame flag
    out.extend_from_slice(&frame_id.to_le_bytes());
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    out.extend_from_slice(&(map_bytes as u16).to_le_bytes());
    out.extend_from_slice(&tile_map);
    for result in &tile_results {
        if let Some((encoded, _)) = result {
            out.extend_from_slice(encoded);
        }
    }

    let stats = FrameStats {
        total_tiles: tile_count as u32,
        dirty_tiles: dirty_count,
        solid_tiles: solid_count,
        delta_tiles: delta_count,
        encoded_bytes: out.len() as u32,
    };
    (out, stats)
}

#[inline]
pub(crate) fn tiles_in_dim(px: usize) -> usize {
    (px + TILE_SIZE - 1) / TILE_SIZE
}

pub(crate) fn tile_is_dirty(
    rgba: &[u8],
    prev: &[u8],
    width: usize,
    height: usize,
    tx: usize,
    ty: usize,
) -> bool {
    let x0 = tx * TILE_SIZE;
    let y0 = ty * TILE_SIZE;
    let x1 = (x0 + TILE_SIZE).min(width);
    let y1 = (y0 + TILE_SIZE).min(height);
    // Non-edge tiles (most common): row width is a compile-time constant (128 bytes),
    // allowing LLVM to emit a fixed-size vectorised comparison without dynamic length.
    if x1 - x0 == TILE_SIZE {
        for y in y0..y1 {
            let base = (y * width + x0) * 4;
            if rgba[base..base + TILE_SIZE * 4] != prev[base..base + TILE_SIZE * 4] {
                return true;
            }
        }
    } else {
        for y in y0..y1 {
            let base = (y * width + x0) * 4;
            let end = base + (x1 - x0) * 4;
            if rgba[base..end] != prev[base..end] {
                return true;
            }
        }
    }
    false
}

/// Encode one dirty tile, returning (encoded_bytes, mode).
/// Called in parallel — no shared mutable state.
///
/// Codec selection:
///   Keyframe tiles → zstd level-1 on raw RGBA (better on dense data).
///   P-frame tiles  → ZRLE on XOR delta (optimised for mostly-zero buffers).
///                    Falls back to zstd when ZRLE ratio exceeds 75 % of raw.
fn encode_tile_buf(
    rgba: &[u8],
    prev: &[u8],
    width: usize,
    height: usize,
    tx: usize,
    ty: usize,
    is_keyframe: bool,
) -> (Vec<u8>, u8) {
    let x0 = tx * TILE_SIZE;
    let y0 = ty * TILE_SIZE;
    let x1 = (x0 + TILE_SIZE).min(width);
    let y1 = (y0 + TILE_SIZE).min(height);
    let tw = x1 - x0;
    let th = y1 - y0;
    let pixel_bytes = tw * th * 4;

    // Fast solid check on raw BGRA — no gather buffer needed.
    // Compares pixels as u32 words; short-circuits at the first differing pixel.
    let base0 = (y0 * width + x0) * 4;
    let first = u32::from_ne_bytes(rgba[base0..base0 + 4].try_into().unwrap());
    let is_solid = (y0..y1).all(|y| {
        let row = (y * width + x0) * 4;
        rgba[row..row + tw * 4]
            .chunks_exact(4)
            .all(|p| u32::from_ne_bytes(p.try_into().unwrap()) == first)
    });
    if is_solid {
        let p = &rgba[base0..base0 + 4]; // BGRA source
        let mut out = Vec::with_capacity(5);
        out.push(MODE_SOLID);
        out.push(p[2]); // R ← BGRA.B-position = R channel
        out.push(p[1]); // G
        out.push(p[0]); // B ← BGRA.R-position = B channel
        out.push(p[3]); // A
        return (out, MODE_SOLID);
    }

    if is_keyframe {
        // Gather RGBA (BGRA→RGBA swap). prev is all-zero after request_keyframe(),
        // so delta == tile — we compress tile directly.
        let mut tile = Vec::with_capacity(pixel_bytes);
        for y in y0..y1 {
            let row = (y * width + x0) * 4;
            for x in 0..tw {
                let o = row + x * 4;
                tile.push(rgba[o + 2]); // R
                tile.push(rgba[o + 1]); // G
                tile.push(rgba[o]);     // B
                tile.push(rgba[o + 3]); // A
            }
        }
        let compressed = zstd::encode_all(tile.as_slice(), 1).unwrap_or_default();
        let mut out = Vec::with_capacity(5 + compressed.len());
        out.push(MODE_ZSTD);
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&compressed);
        (out, MODE_ZSTD)
    } else {
        // Compute XOR delta inline with BGRA→RGBA swap — eliminates tile_prev buffer
        // and the separate XOR pass. One gather+XOR loop instead of three.
        let mut delta = Vec::with_capacity(pixel_bytes);
        for y in y0..y1 {
            let row = (y * width + x0) * 4;
            for x in 0..tw {
                let o = row + x * 4;
                delta.push(rgba[o + 2] ^ prev[o + 2]); // R
                delta.push(rgba[o + 1] ^ prev[o + 1]); // G
                delta.push(rgba[o]     ^ prev[o]);      // B
                delta.push(rgba[o + 3] ^ prev[o + 3]); // A
            }
        }
        let zrle = zrle_encode(&delta);
        // Fall back to zstd when ZRLE ratio is poor (non-sparse XOR — dense P-frame).
        if zrle.len() * 4 > pixel_bytes * 3 {
            let z = zstd::encode_all(delta.as_slice(), 1).unwrap_or_default();
            if !z.is_empty() && z.len() < zrle.len() {
                let mut out = Vec::with_capacity(5 + z.len());
                out.push(MODE_ZSTD);
                out.extend_from_slice(&(z.len() as u32).to_le_bytes());
                out.extend_from_slice(&z);
                return (out, MODE_ZSTD);
            }
        }
        let mut out = Vec::with_capacity(5 + zrle.len());
        out.push(MODE_DELTA);
        out.extend_from_slice(&(zrle.len() as u32).to_le_bytes());
        out.extend_from_slice(&zrle);
        (out, MODE_DELTA)
    }
}

// ── Core: decode ─────────────────────────────────────────────────────────────

/// Decoded pixel data for one dirty tile.
enum TilePixels {
    Solid([u8; 4]),
    Delta(Vec<u8>),
}

fn decode_frame(data: &[u8], frame: &mut Vec<u8>, width: usize, height: usize) -> Result<(), EvrtckError> {
    let mut pos = 0usize;

    macro_rules! need {
        ($n:expr) => {
            if pos + $n > data.len() {
                return Err(EvrtckError::TruncatedData);
            }
        };
    }
    macro_rules! read_bytes {
        ($n:expr) => {{
            need!($n);
            let s = &data[pos..pos + $n];
            pos += $n;
            s
        }};
    }
    macro_rules! read_u16 {
        () => { u16::from_le_bytes(read_bytes!(2).try_into().unwrap()) };
    }
    macro_rules! read_u32 {
        () => { u32::from_le_bytes(read_bytes!(4).try_into().unwrap()) };
    }

    if read_bytes!(4) != MAGIC { return Err(EvrtckError::InvalidMagic); }
    let ver = read_bytes!(1)[0];
    if ver != VERSION { return Err(EvrtckError::UnsupportedVersion(ver)); }
    let flags = read_bytes!(1)[0];
    let _frame_id = read_u32!();
    // Keyframe: encoder reset its prev to black → decoder must also reset.
    if flags & FLAG_KEYFRAME != 0 {
        frame.fill(0);
    }
    let w = read_u32!() as usize;
    let h = read_u32!() as usize;
    if w != width || h != height {
        return Err(EvrtckError::DimensionMismatch {
            expected: (width as u32, height as u32),
            got: (w as u32, h as u32),
        });
    }
    // NOP frame: screen unchanged, prev frame buffer is still correct.
    if flags & FLAG_NOP != 0 {
        return Ok(());
    }

    let map_bytes = read_u16!() as usize;
    // Borrow tile_map directly from data — no allocation needed.
    let tile_map = read_bytes!(map_bytes);

    let tiles_x = tiles_in_dim(width);
    let tiles_y = tiles_in_dim(height);
    let tile_count = tiles_x * tiles_y;

    // Phase 1: scan the byte stream, recording the position and metadata of each
    // dirty tile without decompressing. O(stream) sequential, no allocation per tile.
    // Fields: (x0, y0, x1, y1, mode, enc_start, enc_end)
    let mut dirty: Vec<(usize, usize, usize, usize, u8, usize, usize)> = Vec::new();

    for idx in 0..tile_count {
        let tx = idx % tiles_x;
        let ty = idx / tiles_x;
        let dirty_bit = (tile_map.get(idx / 8).copied().unwrap_or(0) >> (idx % 8)) & 1 == 1;
        if !dirty_bit { continue; }

        need!(1);
        let mode = data[pos]; pos += 1;

        let x0 = tx * TILE_SIZE;
        let y0 = ty * TILE_SIZE;
        let x1 = (x0 + TILE_SIZE).min(width);
        let y1 = (y0 + TILE_SIZE).min(height);

        match mode {
            MODE_SOLID => {
                need!(4);
                dirty.push((x0, y0, x1, y1, mode, pos, pos + 4));
                pos += 4;
            }
            MODE_DELTA | MODE_ZSTD => {
                let enc_len = read_u32!() as usize;
                need!(enc_len);
                dirty.push((x0, y0, x1, y1, mode, pos, pos + enc_len));
                pos += enc_len;
            }
            m => return Err(EvrtckError::InvalidTileMode(m)),
        }
    }

    // Phase 2: decompress dirty tiles — in parallel when there are enough to amortise
    // rayon spawn cost. Each tile is independent; order is preserved by collect().
    let decoded: Vec<Result<TilePixels, EvrtckError>> = if dirty.len() < RAYON_THRESHOLD {
        dirty.iter().map(|&(_, _, _, _, mode, enc_start, enc_end)| {
            decompress_tile(data, enc_start, enc_end, mode)
        }).collect()
    } else {
        dirty.par_iter().map(|&(_, _, _, _, mode, enc_start, enc_end)| {
            decompress_tile(data, enc_start, enc_end, mode)
        }).collect()
    };

    // Phase 3: apply decoded pixels to the frame buffer sequentially (tiles are
    // non-overlapping, but safe Rust can't express disjoint mutable borrows here).
    for (&(x0, y0, x1, y1, _, _, _), pixels_result) in dirty.iter().zip(decoded.into_iter()) {
        match pixels_result? {
            TilePixels::Solid(color) => {
                for y in y0..y1 {
                    for x in x0..x1 {
                        let off = (y * width + x) * 4;
                        frame[off..off + 4].copy_from_slice(&color);
                    }
                }
            }
            TilePixels::Delta(delta) => {
                let expected = (x1 - x0) * (y1 - y0) * 4;
                if delta.len() != expected {
                    return Err(EvrtckError::InvalidDelta);
                }
                let mut di = 0;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let off = (y * width + x) * 4;
                        frame[off]     ^= delta[di];
                        frame[off + 1] ^= delta[di + 1];
                        frame[off + 2] ^= delta[di + 2];
                        frame[off + 3] ^= delta[di + 3];
                        di += 4;
                    }
                }
            }
        }
    }

    Ok(())
}

fn decompress_tile(data: &[u8], enc_start: usize, enc_end: usize, mode: u8)
    -> Result<TilePixels, EvrtckError>
{
    match mode {
        MODE_SOLID => {
            let d = &data[enc_start..enc_end];
            Ok(TilePixels::Solid([d[0], d[1], d[2], d[3]]))
        }
        MODE_DELTA => {
            zrle_decode(&data[enc_start..enc_end])
                .ok_or(EvrtckError::InvalidDelta)
                .map(TilePixels::Delta)
        }
        MODE_ZSTD => {
            zstd::decode_all(&data[enc_start..enc_end])
                .map_err(|_| EvrtckError::InvalidDelta)
                .map(TilePixels::Delta)
        }
        _ => unreachable!(), // validated in phase 1
    }
}

// ── ZRLE — Zero-Run Length Encoding ──────────────────────────────────────────
//
// Optimised for XOR delta buffers where most bytes are 0x00.
//
// Token format (greedy, left-to-right):
//   0x00  count: u16 LE  — emit `count` zero bytes   (min run to justify: 4 bytes)
//   0x01  len:   u16 LE  — emit `len` literal bytes that follow
//
// A run of 65535 zeros encodes to 3 bytes. The average desktop XOR delta for
// a 32×32 tile with one changed pixel is ≈ 4092 zeros + 4 non-zeros → ~16 bytes
// instead of 4096 raw bytes (~256× for that tile).

fn zrle_encode(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() / 8 + 16);
    let mut i = 0;
    while i < src.len() {
        let z_start = i;
        // Fast zero scan: consume u64 words while all 8 bytes are zero.
        while i + 8 <= src.len() {
            if u64::from_ne_bytes(src[i..i + 8].try_into().unwrap()) != 0 { break; }
            i += 8;
        }
        // Mop up remaining zero bytes.
        while i < src.len() && src[i] == 0 { i += 1; }
        let zeros = i - z_start;

        if zeros >= ZRLE_MIN_RUN || (zeros > 0 && i == src.len()) {
            let mut rem = zeros;
            while rem > 0 {
                let n = rem.min(65535) as u16;
                out.push(0x00);
                out.extend_from_slice(&n.to_le_bytes());
                rem -= n as usize;
            }
            continue;
        }

        i = z_start;
        let lit_start = i;
        loop {
            if i >= src.len() { break; }
            if src[i] == 0 {
                // Fast check: is this zero run long enough to break?
                let mut z = 0usize;
                while i + z + 8 <= src.len()
                    && u64::from_ne_bytes(src[i + z..i + z + 8].try_into().unwrap()) == 0
                {
                    z += 8;
                }
                while i + z < src.len() && src[i + z] == 0 { z += 1; }
                if z >= ZRLE_MIN_RUN { break; }
            }
            i += 1;
        }
        let lit_len = i - lit_start;
        if lit_len > 0 {
            let mut j = 0;
            while j < lit_len {
                let n = (lit_len - j).min(65535) as u16;
                out.push(0x01);
                out.extend_from_slice(&n.to_le_bytes());
                out.extend_from_slice(&src[lit_start + j..lit_start + j + n as usize]);
                j += n as usize;
            }
        }
    }
    out
}

fn zrle_decode(src: &[u8]) -> Option<Vec<u8>> {
    // Pre-allocate for the common case of a full TILE_SIZE×TILE_SIZE tile.
    let mut out = Vec::with_capacity(TILE_SIZE * TILE_SIZE * 4);
    let mut i = 0;
    while i < src.len() {
        let tag = *src.get(i)?;
        i += 1;
        match tag {
            0x00 => {
                if i + 2 > src.len() { return None; }
                let count = u16::from_le_bytes([src[i], src[i + 1]]) as usize;
                i += 2;
                out.resize(out.len() + count, 0);
            }
            0x01 => {
                if i + 2 > src.len() { return None; }
                let len = u16::from_le_bytes([src[i], src[i + 1]]) as usize;
                i += 2;
                if i + len > src.len() { return None; }
                out.extend_from_slice(&src[i..i + len]);
                i += len;
            }
            _ => return None,
        }
    }
    Some(out)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a BGRA frame (raw capture format) filled with one color.
    fn solid_frame(w: usize, h: usize, color: [u8; 4]) -> Vec<u8> {
        color.iter().cycle().take(w * h * 4).copied().collect()
    }

    fn black(w: usize, h: usize) -> Vec<u8> {
        vec![0u8; w * h * 4]
    }

    /// Convert BGRA capture bytes to RGBA (same transform the encoder applies).
    /// Use this to build the expected decoded output from a BGRA input frame.
    fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
        bgra.chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect()
    }

    fn checkerboard(w: usize, h: usize) -> Vec<u8> {
        let mut f = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 4;
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                f[off] = v; f[off+1] = v; f[off+2] = v; f[off+3] = 255;
            }
        }
        f
    }

    // ── zrle ─────────────────────────────────────────────────────────────────

    #[test]
    fn zrle_roundtrip_sparse() {
        let mut data = vec![0u8; 4096];
        data[100] = 42;
        data[101] = 17;
        data[500] = 255;
        let enc = zrle_encode(&data);
        let dec = zrle_decode(&enc).unwrap();
        assert_eq!(dec, data);
        assert!(enc.len() < 40, "sparse delta should compress well: {} bytes", enc.len());
    }

    #[test]
    fn zrle_roundtrip_random_like() {
        let data: Vec<u8> = (0u16..1000).map(|i| (i.wrapping_mul(6271) & 0xFF) as u8).collect();
        let enc = zrle_encode(&data);
        let dec = zrle_decode(&enc).unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn zrle_all_zeros_is_tiny() {
        let data = vec![0u8; 65535];
        let enc = zrle_encode(&data);
        assert_eq!(enc.len(), 3); // one ZeroRun token
    }

    // ── encoder / decoder ────────────────────────────────────────────────────

    #[test]
    fn roundtrip_black_frame() {
        let (w, h) = (64, 64);
        let frame = black(w, h);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();
        let pkt = enc.encode(&frame, 1);
        assert_eq!(dec.decode(&pkt).unwrap(), frame.as_slice());
    }

    #[test]
    fn roundtrip_solid_color() {
        // Encoder takes BGRA, decoder outputs RGBA — expected must be RGBA.
        let (w, h) = (32, 32);
        let frame = solid_frame(w, h, [200, 100, 50, 255]);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();
        let pkt = enc.encode(&frame, 1);
        assert_eq!(dec.decode(&pkt).unwrap(), bgra_to_rgba(&frame).as_slice());
    }

    #[test]
    fn roundtrip_checkerboard() {
        let (w, h) = (128, 128);
        let frame = checkerboard(w, h);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();
        let pkt = enc.encode(&frame, 1);
        assert_eq!(dec.decode(&pkt).unwrap(), frame.as_slice());
    }

    #[test]
    fn roundtrip_non_tile_aligned() {
        let (w, h) = (100, 70); // not multiples of TILE_SIZE
        let frame = checkerboard(w, h);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();
        let pkt = enc.encode(&frame, 1);
        assert_eq!(dec.decode(&pkt).unwrap(), frame.as_slice());
    }

    #[test]
    fn sequential_frames_reconstruct_correctly() {
        let (w, h) = (64, 64);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();

        let f1 = solid_frame(w, h, [10, 20, 30, 255]);
        let f2 = checkerboard(w, h);      // grayscale — BGRA==RGBA for this one
        let f3 = solid_frame(w, h, [0, 0, 0, 0]);

        for (i, frame) in [&f1, &f2, &f3].iter().enumerate() {
            let pkt = enc.encode(frame, i as u32 + 1);
            let got = dec.decode(&pkt).unwrap();
            assert_eq!(got, bgra_to_rgba(frame).as_slice(), "frame {i} mismatch");
        }
    }

    #[test]
    fn static_frame_is_near_zero_bytes() {
        let (w, h) = (1920, 1080);
        let frame = solid_frame(w, h, [30, 30, 30, 255]);
        let mut enc = EvrtckEncoder::new(w, h);
        enc.encode(&frame, 1); // first frame — full encode
        let pkt2 = enc.encode(&frame, 2); // identical — should be tiny
        // header(20) + map_bytes_field(2) + tile_map(255) = 277 bytes maximum
        assert!(
            pkt2.data.len() <= 280,
            "static 1080p frame should be ≤280 bytes, got {}",
            pkt2.data.len()
        );
    }

    #[test]
    fn single_pixel_change_isolates_to_one_tile() {
        let (w, h) = (64, 64);
        let mut frame = black(w, h);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();

        dec.decode(&enc.encode(&frame, 1)).unwrap();
        frame[0] = 255; // change BGRA byte 0 = B channel of pixel (0,0)
        let pkt = enc.encode(&frame, 2);
        let got = dec.decode(&pkt).unwrap();
        // Encoder swaps BGRA→RGBA: B channel (index 0) becomes RGBA index 2.
        assert_eq!(got, bgra_to_rgba(&frame).as_slice());

        let (_, stats) = EvrtckEncoder::new(w, h).encode_with_stats(&frame, 1);
        // 64×64 = 4 tiles of 32×32; all 4 are dirty on the first encode
        assert_eq!(stats.total_tiles, 4);
    }

    #[test]
    fn keyframe_after_reset() {
        let (w, h) = (32, 32);
        let frame = solid_frame(w, h, [1, 2, 3, 4]);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();

        // Normal encode/decode
        dec.decode(&enc.encode(&frame, 1)).unwrap();

        // Reset both sides — next decode must still produce the correct frame
        enc.request_keyframe();
        dec.reset();
        let pkt = enc.encode(&frame, 2);
        let got = dec.decode(&pkt).unwrap();
        assert_eq!(got, bgra_to_rgba(&frame).as_slice());
    }

    #[test]
    fn compression_ratio_good_for_mostly_static_content() {
        // Second frame with only 1% of pixels changed — typical desktop delta.
        let (w, h) = (256, 256);
        let mut frame = solid_frame(w, h, [240, 240, 240, 255]);
        let mut enc = EvrtckEncoder::new(w, h);
        enc.encode(&frame, 1); // establish prev
        // Change ~1% of pixels in one region
        for i in 0..100 { frame[i * 4] = 0; }
        let pkt = enc.encode(&frame, 2);
        assert!(
            pkt.compression_ratio() < 0.1,
            "sparse delta should compress >10×, ratio = {}",
            pkt.compression_ratio()
        );
    }

    #[test]
    fn keyframe_1080p_compresses_better_than_raw() {
        // Keyframe with colored content where R ≠ B (exercises BGRA→RGBA swap).
        // Alternating [200,100,50,255] / [30,180,240,255] — different in all channels.
        // With pure ZRLE this would be ~6.7 MB; zstd at level-1 should do ≥2:1.
        let (w, h) = (1920, 1080);
        let frame: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                if i % 2 == 0 { [200u8, 100, 50, 255] } else { [30u8, 180, 240, 255] }
            })
            .collect();
        let mut enc = EvrtckEncoder::new(w, h);
        let pkt = enc.encode(&frame, 1);
        let raw_bytes = w * h * 4;
        assert!(
            pkt.data.len() < raw_bytes / 2,
            "1080p keyframe should compress at least 2× (raw={raw_bytes}, encoded={})",
            pkt.data.len()
        );
        // Decode must be lossless — and verify BGRA swap is correct.
        let mut dec = EvrtckDecoder::new();
        assert_eq!(dec.decode(&pkt).unwrap(), bgra_to_rgba(&frame).as_slice());
    }

    #[test]
    fn error_on_bad_magic() {
        let pkt = EvrtckPacket { frame_id: 0, width: 32, height: 32, data: b"BAAD\x01\x00\x00\x00\x00\x00\x20\x00\x00\x00\x20\x00\x00\x00\x02\x00".to_vec() };
        let mut dec = EvrtckDecoder::new();
        assert_eq!(dec.decode(&pkt), Err(EvrtckError::InvalidMagic));
    }

    #[test]
    fn error_on_truncated_data() {
        let (w, h) = (32, 32);
        let frame = solid_frame(w, h, [5, 5, 5, 255]);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut pkt = enc.encode(&frame, 1);
        pkt.data.truncate(10); // truncate mid-header
        let mut dec = EvrtckDecoder::new();
        assert_eq!(dec.decode(&pkt), Err(EvrtckError::TruncatedData));
    }

    #[test]
    fn nop_frame_for_identical_pframe() {
        let (w, h) = (64, 64);
        let frame = solid_frame(w, h, [42, 100, 200, 255]);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();

        // Keyframe — establishes prev on both sides.
        let kf = enc.encode(&frame, 1);
        assert!(kf.data[5] & FLAG_KEYFRAME != 0, "first frame must be keyframe");
        let pixels_after_kf = dec.decode(&kf).unwrap().to_vec();

        // Second encode with the SAME frame → must produce a NOP packet.
        let nop = enc.encode(&frame, 2);
        assert!(nop.data[5] & FLAG_NOP != 0, "identical P-frame must set FLAG_NOP");
        assert!(nop.data.len() <= 20, "NOP packet must be tiny (got {} bytes)", nop.data.len());

        // Decoder must return the same pixels as after the keyframe.
        let pixels_after_nop = dec.decode(&nop).unwrap();
        assert_eq!(pixels_after_nop, pixels_after_kf.as_slice(),
            "NOP decode must preserve frame buffer");

        // After a change the NOP must NOT fire.
        let frame2 = solid_frame(w, h, [1, 2, 3, 255]);
        let pkt = enc.encode(&frame2, 3);
        assert!(pkt.data[5] & FLAG_NOP == 0, "changed frame must not be NOP");
    }
}
