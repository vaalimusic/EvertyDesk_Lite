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
//! # Wire format (v2)
//!
//! ```text
//! Frame header (20 bytes)
//!   magic      [u8; 4]  = b"EVCK"
//!   version    u8       = 2
//!   flags      u8       (reserved, must be 0)
//!   frame_id   u32 LE
//!   width      u32 LE
//!   height     u32 LE
//!   map_bytes  u16 LE   — byte length of the tile dirty-map that follows
//!
//! Tile dirty-map
//!   One bit per tile (LSB first), 1 = tile changed. Used only to derive the
//!   dirty tile COUNT via popcount — NOT to derive tile identity or stream
//!   position. Padded to the next byte boundary.
//!
//! Tile data  (repeated `popcount(dirty-map)` times, in WIRE order —
//!             which may differ from raster order; see "Priority ordering" below)
//!   tile_idx  u16 LE   — raster tile index (tx + ty * tiles_x); self-describing,
//!                        so entries can appear in any order the encoder chooses
//!   mode      u8
//!     MODE_SOLID  = 1 → color [u8; 4] (RGBA)
//!     MODE_DELTA  = 2 → len u32 LE, then ZRLE-encoded XOR delta
//!     MODE_ZSTD   = 3 → len u32 LE, then zstd-compressed XOR delta
//! ```
//!
//! # Priority ordering (EVRT2CKMAX-TASK-01)
//!
//! v1 required tile data to appear in ascending raster order (the decoder
//! inferred position from stream order + the dirty-map bit position). v2
//! makes each tile entry self-describing (explicit `tile_idx`), which lets
//! the encoder emit dirty tiles in ANY order — in particular, nearest-to-focus
//! first, so the Visible Region (see EVRT2CKMAX.md's Attention Map / Task 01)
//! reaches the decoder before less important regions, without changing what
//! "decoded correctly" means. See `EvrtckEncoder::set_focus_pixel`.

use rayon::prelude::*;
use std::fmt;
use std::sync::OnceLock;

// ── Constants ────────────────────────────────────────────────────────────────

pub const MAGIC: &[u8; 4] = b"EVCK";
pub const VERSION: u8 = 2;
pub const VERSION_COPY_RECTS: u8 = 3;

/// Pixels per tile edge. 32×32 = 1024 px, maps well onto L1 cache lines.
pub const TILE_SIZE: usize = 32;

// ── EVRT2CKMAX-TASK-02: sequential-vs-rayon is now a scheduled decision ───────
//
// This constant used to be the whole decision: "below N tiles, sequential
// beats rayon (spawn overhead ~0.3ms)" — a number picked once and left in
// source, exactly the thing evrt2/tasks/02_SILICON_MARGINAL_UTILITY_SCHEDULER.md
// names as the problem. It's replaced by `use_rayon()` below, which asks a
// `CapabilityRegistry` calibrated against *this machine's* actual measured
// cost of both paths, run once per process on first use.
static CAPABILITY_REGISTRY: OnceLock<crate::execution_capability::CapabilityRegistry> =
    OnceLock::new();

/// Calibration canvas: 32×16 = 512 tiles (1024×512px), enough range to
/// bracket real dirty-tile counts (a typical 1080p frame at 15% dirty is
/// ~500 tiles) without the calibration measurement itself taking long.
const CALIBRATION_TILES_SMALL: usize = 8;
const CALIBRATION_TILES_LARGE: usize = 512;
const CALIBRATION_CANVAS_W: usize = 32 * TILE_SIZE; // 1024
const CALIBRATION_CANVAS_H: usize = 16 * TILE_SIZE; // 512

/// Phase 1+2 — get (calibrating on first call) the process-wide capability
/// registry. `gpu_available` only takes effect on the very first call
/// (`OnceLock` semantics); subsequent callers pass whatever they know and it
/// is ignored once initialized. This mirrors the spec's "probe once per
/// session" model — mid-session GPU appearance/loss is Phase 4 (`rebalance`,
/// EVRT2CKMAX-TASK-02 Non-Goals), not in scope for this replacement.
fn capability_registry(
    gpu_available: bool,
) -> &'static crate::execution_capability::CapabilityRegistry {
    CAPABILITY_REGISTRY.get_or_init(|| {
        let mut reg = crate::execution_capability::CapabilityRegistry::probe(gpu_available);

        // Real, timed workload — not a synthetic microbenchmark: this calls
        // the exact same encode_tile_buf() the live encode path uses, on a
        // representative canvas, so the fitted cost model reflects this
        // machine's real per-tile compression cost (zstd/ZRLE included) and
        // real rayon dispatch overhead, not a guess.
        let rgba = vec![0u8; CALIBRATION_CANVAS_W * CALIBRATION_CANVAS_H * 4];
        let prev = vec![0xFFu8; CALIBRATION_CANVAS_W * CALIBRATION_CANVAS_H * 4]; // all-dirty
        let tiles_x = tiles_in_dim(CALIBRATION_CANVAS_W);

        let run_sequential = |n: usize| {
            let started = std::time::Instant::now();
            for idx in 0..n {
                std::hint::black_box(encode_tile_buf(
                    &rgba,
                    &prev,
                    CALIBRATION_CANVAS_W,
                    CALIBRATION_CANVAS_H,
                    idx % tiles_x,
                    idx / tiles_x,
                    false,
                ));
            }
            started.elapsed()
        };
        let run_rayon = |n: usize| {
            let started = std::time::Instant::now();
            (0..n).into_par_iter().for_each(|idx| {
                std::hint::black_box(encode_tile_buf(
                    &rgba,
                    &prev,
                    CALIBRATION_CANVAS_W,
                    CALIBRATION_CANVAS_H,
                    idx % tiles_x,
                    idx / tiles_x,
                    false,
                ));
            });
            started.elapsed()
        };

        reg.calibrate_entropy_coding(
            CALIBRATION_TILES_SMALL,
            CALIBRATION_TILES_LARGE,
            run_sequential,
            run_rayon,
        );
        reg
    })
}

/// The actual replacement for `tile_count < RAYON_THRESHOLD`: true if the
/// calibrated cost model says rayon is faster than sequential *for this many
/// tiles, on this machine, right now*. Falls back to a conservative
/// sequential choice (never wrong, just possibly slower) if called somehow
/// before the registry could calibrate — see
/// `CapabilityRegistry::entropy_coding_provider_for`.
fn use_rayon(tile_count: usize) -> bool {
    capability_registry(false).entropy_coding_provider_for(tile_count)
        == crate::execution_capability::PROVIDER_CPU_RAYON
}

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
    DimensionMismatch {
        expected: (u32, u32),
        got: (u32, u32),
    },
    InvalidTileMode(u8),
    InvalidDelta,
    /// v2: explicit `tile_idx` in the wire stream is >= tile_count for this
    /// frame's dimensions — corrupt data or a version/dimension mismatch.
    InvalidTileIndex(u16),
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
            Self::InvalidTileIndex(i) => {
                write!(f, "tile index {i} out of range for frame dimensions")
            }
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
        if raw == 0 {
            return 1.0;
        }
        self.data.len() as f32 / raw as f32
    }
}

/// EVRT2CKMAX-TASK-01 (ROADMAP.md Phase 1.2) — the exact byte range a given
/// tile occupies inside `EvrtckPacket.data` (the `[tile_idx u16][data...]`
/// entry, tile_idx prefix included so the whole entry is covered). Lets a
/// caller build the Visible Region's `visible_region_byte_ranges` from the
/// EXACT tiles the Attention Map selected, instead of approximating a byte
/// prefix from an average tile cost (the tile that's actually top-priority
/// isn't always the one nearest the encoder's own focus anchor, so a prefix
/// guess and the true selection can diverge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileOffset {
    pub tile_idx: u16,
    pub byte_start: usize,
    pub byte_len: usize,
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
        if self.total_tiles == 0 {
            return 0.0;
        }
        self.dirty_tiles as f32 / self.total_tiles as f32
    }
}

/// EVRTCK pre-encode frame analysis.
///
/// This is intentionally cheaper than a full encode: it scans dirty tiles and
/// samples XOR bytes from changed tiles to estimate whether the frame is
/// desktop/UI-like (EVRTCK sweet spot) or video/noise-like (hardware codec
/// should take over after a controlled renegotiation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameAnalysis {
    pub total_tiles: u32,
    pub dirty_tiles: u32,
    /// `dirty_tiles / total_tiles`.
    pub dirty_ratio: f32,
    /// Approximate diversity of sampled XOR bytes in dirty tiles. Low values
    /// mean repeated UI deltas; high values mean high-entropy/video-like data.
    pub entropy_score: f32,
    /// Rough wire-size estimate if this frame is encoded as EVRTCK P-frame.
    pub estimated_payload_bytes: u32,
    /// True when this frame is probably better handled by a hardware video
    /// codec. The live pipeline must still switch only through negotiated codec
    /// change; this flag is the policy signal, not a transport packet type.
    pub prefer_silicon: bool,
}

// Wire flags byte (offset 5 in header).
pub(crate) const FLAG_KEYFRAME: u8 = 0x01;
// NOP frame: cur == prev, frame buffer unchanged. No tile map or payload.
pub(crate) const FLAG_NOP: u8 = 0x02;
// v3 frame contains copy/move rectangles before the dirty tile map.
pub(crate) const FLAG_COPY_RECTS: u8 = 0x04;
const FRAME_HEADER_LEN: usize = 20;

/// A lossless framebuffer copy operation applied before tile deltas.
///
/// This is the missing primitive for scroll/drag/window-move scenes: instead
/// of marking every shifted tile dirty, the stream can say "copy this
/// rectangle from the previous framebuffer, then patch the newly exposed
/// strip with normal EVRTCK tiles".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyRect {
    pub src_x: u32,
    pub src_y: u32,
    pub dst_x: u32,
    pub dst_y: u32,
    pub width: u32,
    pub height: u32,
}

/// Pixel-space dirty rectangle supplied by a capture backend.
///
/// Coordinates are half-open: `[left, right) x [top, bottom)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyRect {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl DirtyRect {
    fn is_empty(self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }
}

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
    /// Same as `encode_inner`, plus the exact byte range of every dirty tile
    /// in the output (EVRT2CKMAX-TASK-01, ROADMAP.md Phase 1.2) — lets a
    /// caller build exact `visible_region_byte_ranges` instead of estimating
    /// a byte prefix. Default: delegates to `encode_inner` and returns no
    /// offsets, so backends that don't (yet) implement this still compile
    /// and behave exactly as before — only `CpuEvrtckEncoder` overrides it.
    fn encode_inner_with_offsets(
        &mut self,
        bgra: &[u8],
        frame_id: u32,
    ) -> (EvrtckPacket, FrameStats, Vec<TileOffset>) {
        let (packet, stats) = self.encode_inner(bgra, frame_id);
        (packet, stats, Vec::new())
    }
    fn encode_inner_with_copy_rects(
        &mut self,
        bgra: &[u8],
        frame_id: u32,
        copy_rects: &[CopyRect],
    ) -> (EvrtckPacket, FrameStats) {
        let _ = copy_rects;
        self.encode_inner(bgra, frame_id)
    }
    fn encode_inner_with_capture_hints(
        &mut self,
        bgra: &[u8],
        frame_id: u32,
        copy_rects: &[CopyRect],
        dirty_rects: &[DirtyRect],
    ) -> (EvrtckPacket, FrameStats) {
        let _ = copy_rects;
        let _ = dirty_rects;
        self.encode_inner(bgra, frame_id)
    }
    fn encode_inner_with_scroll_detection(
        &mut self,
        bgra: &[u8],
        frame_id: u32,
    ) -> (EvrtckPacket, FrameStats) {
        self.encode_inner(bgra, frame_id)
    }
    /// Signal that the next frame must be a full keyframe (resets prev to black).
    fn request_keyframe(&mut self);
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    /// Fast dirty-tile pre-scan (no compression). GPU backends may override with
    /// a compute shader. Default: O(W×H) CPU compare.
    fn dirty_ratio(&self, bgra: &[u8]) -> f32;
    /// Cheap pre-encode analysis used by the heterogeneous EVRTCK scheduler.
    fn analyze_next_frame(&self, bgra: &[u8]) -> FrameAnalysis {
        let dirty_ratio = self.dirty_ratio(bgra);
        let total_tiles = tiles_in_dim(self.width()) * tiles_in_dim(self.height());
        FrameAnalysis {
            total_tiles: total_tiles as u32,
            dirty_tiles: (dirty_ratio * total_tiles as f32).round() as u32,
            dirty_ratio,
            entropy_score: 0.0,
            estimated_payload_bytes: FRAME_HEADER_LEN as u32,
            prefer_silicon: false,
        }
    }
    /// Same scheduler pre-analysis, but constrained to capture-backend dirty
    /// rectangles. This prevents the heterogeneous scheduler from making a
    /// silicon-switch decision from a conservative full-frame scan when the
    /// encoder itself will use exact DXGI/DamageRect hints.
    fn analyze_next_frame_with_dirty_rects(
        &self,
        bgra: &[u8],
        dirty_rects: &[DirtyRect],
    ) -> FrameAnalysis {
        let _ = dirty_rects;
        self.analyze_next_frame(bgra)
    }
    /// Set (or clear, with `None`) the Visible Region anchor in TILE coordinates
    /// (not pixels). When set, dirty tiles are emitted nearest-to-focus first
    /// instead of raster order — see EVRT2CKMAX-TASK-01. Default: no-op, so
    /// backends that don't implement priority ordering still compile and behave
    /// exactly as before (raster order, since sort is a stable no-op when
    /// `focus` is `None`).
    fn set_focus(&mut self, _focus_tile: Option<(usize, usize)>) {}
    /// EVRT2CKMAX-TASK-02: does this backend run on a GPU adapter? Used to
    /// register a real, successfully-probed `RoiEncoding` provider in the
    /// `execution_capability` registry — reusing the same probe that already
    /// decides CPU-vs-GPU here (`new_backend`), instead of a second,
    /// duplicate GPU init just for capability registration. Default: false
    /// (every non-GPU backend, i.e. `CpuEvrtckEncoder`).
    fn is_gpu(&self) -> bool {
        false
    }
}

// ── CPU backend — always available, no GPU required ───────────────────────────

struct CpuEvrtckEncoder {
    prev: Vec<u8>,
    width: usize,
    height: usize,
    pending_keyframe: bool,
    focus: Option<(usize, usize)>,
}

impl CpuEvrtckEncoder {
    fn new(width: usize, height: usize) -> Self {
        Self {
            prev: vec![0u8; width * height * 4],
            width,
            height,
            pending_keyframe: true, // first frame is always a keyframe
            focus: None,
        }
    }

    fn analyze_next_frame_impl(&self, bgra: &[u8]) -> FrameAnalysis {
        let tiles_x = tiles_in_dim(self.width);
        let tiles_y = tiles_in_dim(self.height);
        let total_tiles = tiles_x * tiles_y;
        if total_tiles == 0 || bgra.len() != self.prev.len() || bgra == self.prev {
            return FrameAnalysis {
                total_tiles: total_tiles as u32,
                dirty_tiles: 0,
                dirty_ratio: 0.0,
                entropy_score: 0.0,
                estimated_payload_bytes: FRAME_HEADER_LEN as u32,
                prefer_silicon: false,
            };
        }

        const MAX_SAMPLED_DIRTY_TILES: usize = 96;
        const SAMPLE_STRIDE_BYTES: usize = 16;
        let map_bytes = total_tiles.div_ceil(8);
        let mut dirty_tiles = 0usize;
        let mut sampled_dirty_tiles = 0usize;
        let mut sampled_bytes = 0usize;
        let mut changed_sampled_bytes = 0usize;
        let mut histogram = [0u16; 256];
        let mut unique = 0usize;

        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                if !tile_is_dirty(bgra, &self.prev, self.width, self.height, tx, ty) {
                    continue;
                }
                dirty_tiles += 1;

                if sampled_dirty_tiles >= MAX_SAMPLED_DIRTY_TILES {
                    continue;
                }
                sampled_dirty_tiles += 1;
                let x0 = tx * TILE_SIZE;
                let y0 = ty * TILE_SIZE;
                let x1 = (x0 + TILE_SIZE).min(self.width);
                let y1 = (y0 + TILE_SIZE).min(self.height);

                for y in y0..y1 {
                    let row_start = (y * self.width + x0) * 4;
                    let row_end = (y * self.width + x1) * 4;
                    let mut off = row_start;
                    while off < row_end {
                        // Sample B/G/R only. Alpha is normally constant and
                        // would hide entropy in noisy/video content.
                        for channel in 0..3 {
                            let xor = bgra[off + channel] ^ self.prev[off + channel];
                            sampled_bytes += 1;
                            if xor != 0 {
                                changed_sampled_bytes += 1;
                            }
                            let bin = &mut histogram[xor as usize];
                            if *bin == 0 {
                                unique += 1;
                            }
                            *bin = bin.saturating_add(1);
                        }
                        off += SAMPLE_STRIDE_BYTES;
                    }
                }
            }
        }

        let dirty_ratio = dirty_tiles as f32 / total_tiles as f32;
        let changed_density = if sampled_bytes == 0 {
            0.0
        } else {
            changed_sampled_bytes as f32 / sampled_bytes as f32
        };
        let entropy_score = if sampled_bytes == 0 {
            0.0
        } else {
            (unique as f32 / 192.0).clamp(0.0, 1.0)
        };
        let dirty_tile_pixels = (self.width * self.height) as f32 / total_tiles as f32;
        let bytes_per_dirty_tile = if entropy_score >= 0.65 {
            9.0 + dirty_tile_pixels * 3.0 * changed_density * 0.92
        } else if entropy_score >= 0.25 {
            18.0 + dirty_tile_pixels * 0.55
        } else {
            8.0 + dirty_tile_pixels * 0.015
        };
        let estimated_payload_bytes =
            FRAME_HEADER_LEN + map_bytes + (dirty_tiles as f32 * bytes_per_dirty_tile) as usize;
        let raw_bytes = self.width.saturating_mul(self.height).saturating_mul(4);
        let prefer_silicon = dirty_tiles > 0
            && entropy_score >= 0.62
            && (dirty_ratio >= 0.12 || estimated_payload_bytes >= raw_bytes / 10);

        FrameAnalysis {
            total_tiles: total_tiles as u32,
            dirty_tiles: dirty_tiles as u32,
            dirty_ratio,
            entropy_score,
            estimated_payload_bytes: estimated_payload_bytes.min(u32::MAX as usize) as u32,
            prefer_silicon,
        }
    }

    fn analyze_next_frame_dirty_indices_impl(
        &self,
        bgra: &[u8],
        dirty_indices: &[usize],
    ) -> FrameAnalysis {
        let tiles_x = tiles_in_dim(self.width);
        let tiles_y = tiles_in_dim(self.height);
        let total_tiles = tiles_x * tiles_y;
        if total_tiles == 0 || bgra.len() != self.prev.len() || bgra == self.prev {
            return FrameAnalysis {
                total_tiles: total_tiles as u32,
                dirty_tiles: 0,
                dirty_ratio: 0.0,
                entropy_score: 0.0,
                estimated_payload_bytes: FRAME_HEADER_LEN as u32,
                prefer_silicon: false,
            };
        }

        const MAX_SAMPLED_DIRTY_TILES: usize = 96;
        const SAMPLE_STRIDE_BYTES: usize = 16;
        let map_bytes = total_tiles.div_ceil(8);
        let mut dirty_tiles = 0usize;
        let mut sampled_dirty_tiles = 0usize;
        let mut sampled_bytes = 0usize;
        let mut changed_sampled_bytes = 0usize;
        let mut histogram = [0u16; 256];
        let mut unique = 0usize;

        for tile_idx in dirty_indices.iter().copied() {
            if tile_idx >= total_tiles {
                continue;
            }
            let tx = tile_idx % tiles_x;
            let ty = tile_idx / tiles_x;
            if !tile_is_dirty(bgra, &self.prev, self.width, self.height, tx, ty) {
                continue;
            }
            dirty_tiles += 1;

            if sampled_dirty_tiles >= MAX_SAMPLED_DIRTY_TILES {
                continue;
            }
            sampled_dirty_tiles += 1;
            let x0 = tx * TILE_SIZE;
            let y0 = ty * TILE_SIZE;
            let x1 = (x0 + TILE_SIZE).min(self.width);
            let y1 = (y0 + TILE_SIZE).min(self.height);

            for y in y0..y1 {
                let row_start = (y * self.width + x0) * 4;
                let row_end = (y * self.width + x1) * 4;
                let mut off = row_start;
                while off < row_end {
                    for channel in 0..3 {
                        let xor = bgra[off + channel] ^ self.prev[off + channel];
                        sampled_bytes += 1;
                        if xor != 0 {
                            changed_sampled_bytes += 1;
                        }
                        let bin = &mut histogram[xor as usize];
                        if *bin == 0 {
                            unique += 1;
                        }
                        *bin = bin.saturating_add(1);
                    }
                    off += SAMPLE_STRIDE_BYTES;
                }
            }
        }

        let dirty_ratio = dirty_tiles as f32 / total_tiles as f32;
        let changed_density = if sampled_bytes == 0 {
            0.0
        } else {
            changed_sampled_bytes as f32 / sampled_bytes as f32
        };
        let entropy_score = if sampled_bytes == 0 {
            0.0
        } else {
            (unique as f32 / 192.0).clamp(0.0, 1.0)
        };
        let dirty_tile_pixels = (self.width * self.height) as f32 / total_tiles as f32;
        let bytes_per_dirty_tile = if entropy_score >= 0.65 {
            9.0 + dirty_tile_pixels * 3.0 * changed_density * 0.92
        } else if entropy_score >= 0.25 {
            18.0 + dirty_tile_pixels * 0.55
        } else {
            8.0 + dirty_tile_pixels * 0.015
        };
        let estimated_payload_bytes =
            FRAME_HEADER_LEN + map_bytes + (dirty_tiles as f32 * bytes_per_dirty_tile) as usize;
        let raw_bytes = self.width.saturating_mul(self.height).saturating_mul(4);
        let prefer_silicon = dirty_tiles > 0
            && entropy_score >= 0.62
            && (dirty_ratio >= 0.12 || estimated_payload_bytes >= raw_bytes / 10);

        FrameAnalysis {
            total_tiles: total_tiles as u32,
            dirty_tiles: dirty_tiles as u32,
            dirty_ratio,
            entropy_score,
            estimated_payload_bytes: estimated_payload_bytes.min(u32::MAX as usize) as u32,
            prefer_silicon,
        }
    }
}

impl CpuEvrtckEncoder {
    /// Shared body for `encode_inner`/`encode_inner_with_offsets` — the only
    /// difference between the two is whether `encode_frame_with_offsets` is
    /// asked to compute the (otherwise-free-to-skip) `Vec<TileOffset>`.
    fn encode_inner_impl(
        &mut self,
        bgra: &[u8],
        frame_id: u32,
        want_offsets: bool,
    ) -> (EvrtckPacket, FrameStats, Vec<TileOffset>) {
        debug_assert_eq!(bgra.len(), self.width * self.height * 4);
        let is_kf = self.pending_keyframe;
        self.pending_keyframe = false;
        let (data, stats, offsets) = encode_frame_with_offsets(
            bgra,
            &self.prev,
            self.width,
            self.height,
            frame_id,
            is_kf,
            self.focus,
            want_offsets,
        );
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
        (pkt, stats, offsets)
    }
}

impl EvrtckEncoderBackend for CpuEvrtckEncoder {
    fn encode_inner(&mut self, bgra: &[u8], frame_id: u32) -> (EvrtckPacket, FrameStats) {
        let (pkt, stats, _offsets) = self.encode_inner_impl(bgra, frame_id, false);
        (pkt, stats)
    }

    fn encode_inner_with_offsets(
        &mut self,
        bgra: &[u8],
        frame_id: u32,
    ) -> (EvrtckPacket, FrameStats, Vec<TileOffset>) {
        self.encode_inner_impl(bgra, frame_id, true)
    }

    fn encode_inner_with_copy_rects(
        &mut self,
        bgra: &[u8],
        frame_id: u32,
        copy_rects: &[CopyRect],
    ) -> (EvrtckPacket, FrameStats) {
        if self.pending_keyframe || copy_rects.is_empty() {
            return self.encode_inner(bgra, frame_id);
        }
        debug_assert_eq!(bgra.len(), self.width * self.height * 4);

        let valid_copy_rects: Vec<CopyRect> = copy_rects
            .iter()
            .copied()
            .filter(|rect| copy_rect_is_valid(*rect, self.width, self.height))
            .collect();
        if valid_copy_rects.is_empty() {
            return self.encode_inner(bgra, frame_id);
        }

        let mut predicted = self.prev.clone();
        apply_copy_rects(&mut predicted, self.width, self.height, &valid_copy_rects);
        let (data, stats, _offsets) = encode_frame_with_offsets_and_copy_rects(
            bgra,
            &predicted,
            self.width,
            self.height,
            frame_id,
            &valid_copy_rects,
            self.focus,
            false,
        );
        self.pending_keyframe = false;
        self.prev.copy_from_slice(bgra);
        let pkt = EvrtckPacket {
            frame_id,
            width: self.width as u32,
            height: self.height as u32,
            data,
        };
        (pkt, stats)
    }

    fn encode_inner_with_capture_hints(
        &mut self,
        bgra: &[u8],
        frame_id: u32,
        copy_rects: &[CopyRect],
        dirty_rects: &[DirtyRect],
    ) -> (EvrtckPacket, FrameStats) {
        if self.pending_keyframe {
            return self.encode_inner(bgra, frame_id);
        }
        debug_assert_eq!(bgra.len(), self.width * self.height * 4);

        let valid_copy_rects: Vec<CopyRect> = copy_rects
            .iter()
            .copied()
            .filter(|rect| copy_rect_is_valid(*rect, self.width, self.height))
            .collect();
        let dirty_indices = dirty_tile_indices_from_rects(dirty_rects, self.width, self.height);
        if dirty_indices.is_none() && valid_copy_rects.is_empty() {
            return self.encode_inner_with_scroll_detection(bgra, frame_id);
        }

        let mut predicted = self.prev.clone();
        if !valid_copy_rects.is_empty() {
            apply_copy_rects(&mut predicted, self.width, self.height, &valid_copy_rects);
        }

        let (data, stats) = if let Some(indices) = dirty_indices {
            if bgra == predicted.as_slice() {
                (
                    if valid_copy_rects.is_empty() {
                        nop_packet_data(frame_id, self.width, self.height)
                    } else {
                        copy_rect_only_packet_data(
                            frame_id,
                            self.width,
                            self.height,
                            &valid_copy_rects,
                        )
                    },
                    FrameStats {
                        total_tiles: (tiles_in_dim(self.width) * tiles_in_dim(self.height)) as u32,
                        encoded_bytes: if valid_copy_rects.is_empty() {
                            FRAME_HEADER_LEN as u32
                        } else {
                            (FRAME_HEADER_LEN + 2 + valid_copy_rects.len() * 24) as u32
                        },
                        ..Default::default()
                    },
                )
            } else if valid_copy_rects.is_empty() {
                encode_pframe_from_dirty_indices(
                    bgra,
                    &predicted,
                    self.width,
                    self.height,
                    frame_id,
                    indices,
                    self.focus,
                )
            } else {
                encode_copy_rect_frame_from_dirty_indices(
                    bgra,
                    &predicted,
                    self.width,
                    self.height,
                    frame_id,
                    &valid_copy_rects,
                    indices,
                    self.focus,
                )
            }
        } else {
            let (data, stats, _offsets) = encode_frame_with_offsets_and_copy_rects(
                bgra,
                &predicted,
                self.width,
                self.height,
                frame_id,
                &valid_copy_rects,
                self.focus,
                false,
            );
            (data, stats)
        };

        self.pending_keyframe = false;
        self.prev.copy_from_slice(bgra);
        let pkt = EvrtckPacket {
            frame_id,
            width: self.width as u32,
            height: self.height as u32,
            data,
        };
        (pkt, stats)
    }

    fn encode_inner_with_scroll_detection(
        &mut self,
        bgra: &[u8],
        frame_id: u32,
    ) -> (EvrtckPacket, FrameStats) {
        if self.pending_keyframe {
            return self.encode_inner(bgra, frame_id);
        }
        if bgra == self.prev.as_slice() {
            return self.encode_inner(bgra, frame_id);
        }
        if let Some(copy_rect) =
            detect_full_width_vertical_scroll(&self.prev, bgra, self.width, self.height)
        {
            return self.encode_inner_with_copy_rects(bgra, frame_id, &[copy_rect]);
        }
        self.encode_inner(bgra, frame_id)
    }

    fn request_keyframe(&mut self) {
        self.prev.fill(0);
        self.pending_keyframe = true;
    }

    fn width(&self) -> usize {
        self.width
    }
    fn height(&self) -> usize {
        self.height
    }

    fn dirty_ratio(&self, bgra: &[u8]) -> f32 {
        if bgra == self.prev {
            return 0.0;
        }
        let tiles_x = tiles_in_dim(self.width);
        let tiles_y = tiles_in_dim(self.height);
        let total = tiles_x * tiles_y;
        if total == 0 {
            return 0.0;
        }
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

    fn analyze_next_frame(&self, bgra: &[u8]) -> FrameAnalysis {
        self.analyze_next_frame_impl(bgra)
    }

    fn analyze_next_frame_with_dirty_rects(
        &self,
        bgra: &[u8],
        dirty_rects: &[DirtyRect],
    ) -> FrameAnalysis {
        match dirty_tile_indices_from_rects(dirty_rects, self.width, self.height) {
            Some(indices) => self.analyze_next_frame_dirty_indices_impl(bgra, &indices),
            None => self.analyze_next_frame_impl(bgra),
        }
    }

    fn set_focus(&mut self, focus_tile: Option<(usize, usize)>) {
        self.focus = focus_tile;
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
        let inner = new_backend(width, height);
        // EVRT2CKMAX-TASK-02 M5: seed the capability registry's RoiEncoding
        // provider from the SAME probe that just picked this backend — a
        // real, successfully-initialized GPU backend, not a second guess.
        // No-op on the 2nd+ encoder instance (OnceLock already initialized).
        let _ = capability_registry(inner.is_gpu());
        Self { inner }
    }

    pub fn encode_with_stats(&mut self, rgba: &[u8], frame_id: u32) -> (EvrtckPacket, FrameStats) {
        self.inner.encode_inner(rgba, frame_id)
    }

    pub fn encode(&mut self, rgba: &[u8], frame_id: u32) -> EvrtckPacket {
        self.inner.encode_inner(rgba, frame_id).0
    }

    /// EVRT2CKMAX-TASK-01 (ROADMAP.md Phase 1.2): same as `encode`, plus the
    /// exact byte range of every dirty tile — lets a caller build exact
    /// `visible_region_byte_ranges` from the Attention Map's actual tile
    /// selection instead of estimating a byte prefix. Empty offsets on a
    /// backend that hasn't implemented this yet (see trait default).
    pub fn encode_with_offsets(
        &mut self,
        rgba: &[u8],
        frame_id: u32,
    ) -> (EvrtckPacket, FrameStats, Vec<TileOffset>) {
        self.inner.encode_inner_with_offsets(rgba, frame_id)
    }

    /// Encode a P-frame using precomputed copy/move rectangles.
    ///
    /// Feed this from DXGI move rects, platform capture metadata, or the
    /// optional scroll detector. The decoder applies copy rects first, then
    /// normal EVRTCK tile deltas for the residual/newly exposed pixels.
    pub fn encode_with_copy_rects(
        &mut self,
        rgba: &[u8],
        frame_id: u32,
        copy_rects: &[CopyRect],
    ) -> (EvrtckPacket, FrameStats) {
        self.inner
            .encode_inner_with_copy_rects(rgba, frame_id, copy_rects)
    }

    /// Encode using capture-backend hints.
    ///
    /// `copy_rects` are applied first, then only tiles intersecting
    /// `dirty_rects` are scanned/encoded. Empty hints fall back to the normal
    /// safe encoder path.
    pub fn encode_with_capture_hints(
        &mut self,
        rgba: &[u8],
        frame_id: u32,
        copy_rects: &[CopyRect],
        dirty_rects: &[DirtyRect],
    ) -> (EvrtckPacket, FrameStats) {
        self.inner
            .encode_inner_with_capture_hints(rgba, frame_id, copy_rects, dirty_rects)
    }

    /// Encode with a conservative built-in scroll detector.
    ///
    /// This is a bridge until platform capture backends feed real move rects
    /// (DXGI move rects on Windows, equivalent APIs elsewhere). It only
    /// recognizes exact full-width vertical copies and falls back to ordinary
    /// EVRTCK if no safe copy is found.
    pub fn encode_with_scroll_detection(
        &mut self,
        rgba: &[u8],
        frame_id: u32,
    ) -> (EvrtckPacket, FrameStats) {
        self.inner
            .encode_inner_with_scroll_detection(rgba, frame_id)
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

    pub fn analyze_next_frame(&self, rgba: &[u8]) -> FrameAnalysis {
        self.inner.analyze_next_frame(rgba)
    }

    pub fn analyze_next_frame_with_dirty_rects(
        &self,
        rgba: &[u8],
        dirty_rects: &[DirtyRect],
    ) -> FrameAnalysis {
        self.inner
            .analyze_next_frame_with_dirty_rects(rgba, dirty_rects)
    }

    pub fn width(&self) -> usize {
        self.inner.width()
    }
    pub fn height(&self) -> usize {
        self.inner.height()
    }

    /// Set the Visible Region anchor in PIXEL coordinates (e.g. the last known
    /// cursor/aim position). Dirty tiles nearest this point are emitted first
    /// in the wire stream instead of raster order — see EVRT2CKMAX-TASK-01
    /// and the module-level "Priority ordering" doc comment.
    ///
    /// Cheap to call every frame; it only sets a field, no recompute happens
    /// until the next `encode()`/`encode_with_stats()` call.
    pub fn set_focus_pixel(&mut self, x: u32, y: u32) {
        let tx = (x as usize / TILE_SIZE).min(tiles_in_dim(self.width()).saturating_sub(1));
        let ty = (y as usize / TILE_SIZE).min(tiles_in_dim(self.height()).saturating_sub(1));
        self.inner.set_focus(Some((tx, ty)));
    }

    /// Clear the Visible Region anchor — dirty tiles go back to raster order.
    pub fn clear_focus(&mut self) {
        self.inner.set_focus(None);
    }
}

// ── Stateful decoder ─────────────────────────────────────────────────────────

#[derive(Default)]
pub struct EvrtckDecoder {
    frame: Vec<u8>,
    width: usize,
    height: usize,
}

impl EvrtckDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode a packet into the internal frame buffer. Returns a slice of the
    /// reconstructed RGBA frame. The slice is valid until the next `decode` call.
    pub fn decode(&mut self, pkt: &EvrtckPacket) -> Result<&[u8], EvrtckError> {
        self.decode_wire(&pkt.data)
    }

    /// Decode raw wire bytes — self-describing, reads dimensions from the header.
    /// Use this when the caller doesn't know the exact encoded dimensions.
    pub fn decode_wire(&mut self, data: &[u8]) -> Result<&[u8], EvrtckError> {
        // Wire header: magic(4) + ver(1) + flags(1) + frame_id(4) + w(4) + h(4) = 18 bytes minimum
        if data.len() < 18 {
            return Err(EvrtckError::TruncatedData);
        }
        if &data[0..4] != MAGIC {
            return Err(EvrtckError::InvalidMagic);
        }
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

    /// ROADMAP.md Phase 3.3 — same as `decode_wire`, but also returns the
    /// actual tile apply order, and lets the caller supply a priority
    /// function (e.g. from a decoded APF map) to steer that order. See
    /// `decode_frame_prioritized`'s doc for what "order" does and doesn't
    /// change — the reconstructed frame is identical either way.
    pub fn decode_wire_prioritized(
        &mut self,
        data: &[u8],
        tile_priority: Option<&dyn Fn(usize) -> f32>,
    ) -> Result<(&[u8], Vec<usize>), EvrtckError> {
        if data.len() < 18 {
            return Err(EvrtckError::TruncatedData);
        }
        if &data[0..4] != MAGIC {
            return Err(EvrtckError::InvalidMagic);
        }
        let w = u32::from_le_bytes(data[10..14].try_into().unwrap()) as usize;
        let h = u32::from_le_bytes(data[14..18].try_into().unwrap()) as usize;
        if self.width != w || self.height != h || self.frame.len() != w * h * 4 {
            self.frame = vec![0u8; w * h * 4];
            self.width = w;
            self.height = h;
        }
        let apply_order = decode_frame_prioritized(data, &mut self.frame, w, h, tile_priority)?;
        Ok((&self.frame, apply_order))
    }

    /// ROADMAP.md Phase 6.1/6.3 bug found during the Phase 6.4 investigation:
    /// when an IS_SILICON (NVENC/H264) frame is decoded and shown to the
    /// user, this decoder's OWN tracked framebuffer was never told about
    /// it — the next EVRTCK P-frame's MODE_DELTA tiles are an XOR against
    /// `self.frame`'s CURRENT bytes (see `decode_frame_prioritized`'s
    /// `TilePixels::Delta` handling), and without this call `self.frame`
    /// would still hold whatever the last EVRTCK frame painted, one or more
    /// frames stale — XORing a delta computed on the host against the TRUE
    /// previous frame, onto a client buffer that's actually further behind,
    /// reconstructs garbage pixels for exactly the tiles that changed during
    /// the silicon frame(s) in between. Confirmed against the same
    /// wire-format fact `keyframe_1080p_compresses_better_than_raw` already
    /// relies on: EVRTCK's own tile encode does the BGRA→RGBA channel swap
    /// internally, so its decoded `frame` buffer and openh264's
    /// `write_rgba8` output share the same RGBA byte layout — this is a
    /// straight buffer copy, no channel conversion needed. `rgba.len()` must
    /// equal `width * height * 4`; a mismatch means the caller decoded a
    /// silicon frame at a different resolution than this decoder is tracking
    /// (a genuine caller bug) and is treated as a no-op rather than a panic,
    /// since a resolution change is about to force a keyframe anyway.
    pub fn sync_from_rgba(&mut self, rgba: &[u8], width: usize, height: usize) {
        if rgba.len() != width * height * 4 {
            return;
        }
        if self.width != width || self.height != height || self.frame.len() != rgba.len() {
            self.width = width;
            self.height = height;
            self.frame = rgba.to_vec();
        } else {
            self.frame.copy_from_slice(rgba);
        }
    }

    /// ROADMAP.md Phase 6.4: applies a stream built by
    /// `encode_tile_subset_absolute` on top of whatever is currently in
    /// `self.frame` — e.g. right after `sync_from_rgba` synced in a
    /// different codec's (NVENC's) lossy background for this same frame.
    /// Every tile in `data` is ABSOLUTE (never MODE_DELTA — see that
    /// function's doc comment for why), so each tile's rect is zeroed right
    /// before it's applied: `TilePixels::Delta`'s apply step is an XOR, and
    /// `0 XOR absolute_bytes == absolute_bytes` regardless of what was
    /// there — this is what makes the overlay correct independent of the
    /// background codec's content. `TilePixels::Solid` doesn't need the
    /// zero (it's a direct overwrite either way) but gets it too, for one
    /// uniform code path rather than a mode-conditional one.
    ///
    /// Requires `self`'s current dimensions to already match `width`/
    /// `height` inside `data` (call `sync_from_rgba` or `decode_wire` first
    /// to establish them) — this method never resizes `self.frame`, since
    /// doing so would silently discard the background layer it's meant to
    /// sit on top of.
    ///
    /// Returns the tile indices actually applied, in wire order — useful
    /// for tests and telemetry, same shape as `decode_wire_prioritized`'s
    /// `apply_order`.
    pub fn apply_absolute_overlay(&mut self, data: &[u8]) -> Result<Vec<usize>, EvrtckError> {
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
            () => {
                u16::from_le_bytes(read_bytes!(2).try_into().unwrap())
            };
        }
        macro_rules! read_u32 {
            () => {
                u32::from_le_bytes(read_bytes!(4).try_into().unwrap())
            };
        }

        if read_bytes!(4) != MAGIC {
            return Err(EvrtckError::InvalidMagic);
        }
        let ver = read_bytes!(1)[0];
        if ver != VERSION {
            return Err(EvrtckError::UnsupportedVersion(ver));
        }
        let _flags = read_bytes!(1)[0]; // deliberately ignored — see encode_tile_subset_absolute's doc
        let _frame_id = read_u32!();
        let w = read_u32!() as usize;
        let h = read_u32!() as usize;
        if w != self.width || h != self.height {
            return Err(EvrtckError::DimensionMismatch {
                expected: (self.width as u32, self.height as u32),
                got: (w as u32, h as u32),
            });
        }

        let map_bytes = read_u16!() as usize;
        let tile_map = read_bytes!(map_bytes);
        let dirty_count: usize = tile_map.iter().map(|b| b.count_ones() as usize).sum();

        let tiles_x = tiles_in_dim(w);
        let tiles_y = tiles_in_dim(h);
        let tile_count = tiles_x * tiles_y;

        let mut applied = Vec::with_capacity(dirty_count);
        for _ in 0..dirty_count {
            let tile_idx = read_u16!();
            let idx = tile_idx as usize;
            if idx >= tile_count {
                return Err(EvrtckError::InvalidTileIndex(tile_idx));
            }
            let tx = idx % tiles_x;
            let ty = idx / tiles_x;
            let x0 = tx * TILE_SIZE;
            let y0 = ty * TILE_SIZE;
            let x1 = (x0 + TILE_SIZE).min(w);
            let y1 = (y0 + TILE_SIZE).min(h);

            need!(1);
            let mode = data[pos];
            pos += 1;
            let (enc_start, enc_end) = match mode {
                MODE_SOLID => {
                    need!(4);
                    let range = (pos, pos + 4);
                    pos += 4;
                    range
                }
                MODE_DELTA | MODE_ZSTD => {
                    let enc_len = read_u32!() as usize;
                    need!(enc_len);
                    let range = (pos, pos + enc_len);
                    pos += enc_len;
                    range
                }
                m => return Err(EvrtckError::InvalidTileMode(m)),
            };
            let pixels = decompress_tile(data, enc_start, enc_end, mode)?;

            // Zero this tile's rect first — see doc comment for why. Then
            // apply exactly like decode_frame_prioritized's Phase 3 does.
            let tw4 = (x1 - x0) * 4;
            for y in y0..y1 {
                let rs = (y * w + x0) * 4;
                self.frame[rs..rs + tw4].fill(0);
            }
            match pixels {
                TilePixels::Solid(color) => {
                    let mut row_buf = [0u8; TILE_SIZE * 4];
                    for chunk in row_buf[..tw4].chunks_exact_mut(4) {
                        chunk.copy_from_slice(&color);
                    }
                    let row_pat = &row_buf[..tw4];
                    for y in y0..y1 {
                        let rs = (y * w + x0) * 4;
                        self.frame[rs..rs + tw4].copy_from_slice(row_pat);
                    }
                }
                TilePixels::Delta(delta) => {
                    let expected = tw4 * (y1 - y0);
                    if delta.len() != expected {
                        return Err(EvrtckError::InvalidDelta);
                    }
                    let mut di = 0;
                    for y in y0..y1 {
                        let rs = (y * w + x0) * 4;
                        // frame was just zeroed above, so this XOR is a
                        // plain copy — kept as XOR for one uniform code
                        // path with the zero-fill step, not because the
                        // buffer might be non-zero here.
                        let frame_row = &mut self.frame[rs..rs + tw4];
                        let delta_row = &delta[di..di + tw4];
                        for (f, d) in frame_row.iter_mut().zip(delta_row) {
                            *f ^= d;
                        }
                        di += tw4;
                    }
                }
            }
            applied.push(idx);
        }

        Ok(applied)
    }

    /// Reset decoder state (e.g. after requesting a keyframe).
    pub fn reset(&mut self) {
        self.frame.fill(0);
    }

    pub fn current_frame(&self) -> &[u8] {
        &self.frame
    }
    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }
}

// ── Core: encode ─────────────────────────────────────────────────────────────

/// Squared Chebyshev-ish distance in tile-grid units — cheap (no sqrt), and
/// monotonic with actual distance, which is all a sort key needs.
#[inline]
fn tile_distance_key(idx: usize, tiles_x: usize, focus: (usize, usize)) -> u32 {
    let tx = (idx % tiles_x) as i64;
    let ty = (idx / tiles_x) as i64;
    let dx = tx - focus.0 as i64;
    let dy = ty - focus.1 as i64;
    (dx * dx + dy * dy) as u32
}

/// Reorder dirty tiles nearest-to-focus first (EVRT2CKMAX-TASK-01 Visible
/// Region). Stable sort: tiles at equal distance keep their original
/// (raster) relative order, so behavior is deterministic and — with
/// `focus = None` — this function is skipped entirely (raster order, same
/// as v1 always produced).
fn order_by_focus(
    dirty_tiles: &mut [(usize, Vec<u8>, u8)],
    tiles_x: usize,
    focus: Option<(usize, usize)>,
) {
    if let Some(focus) = focus {
        dirty_tiles.sort_by_key(|&(idx, _, _)| tile_distance_key(idx, tiles_x, focus));
    }
}

pub(crate) fn encode_frame(
    rgba: &[u8],
    prev: &[u8],
    width: usize,
    height: usize,
    frame_id: u32,
    is_keyframe: bool,
    focus: Option<(usize, usize)>,
) -> (Vec<u8>, FrameStats) {
    let (data, stats, _offsets) = encode_frame_with_offsets(
        rgba,
        prev,
        width,
        height,
        frame_id,
        is_keyframe,
        focus,
        false,
    );
    (data, stats)
}

/// Same encode path as `encode_frame`, optionally also returning the exact
/// byte range of every dirty tile in the output (`want_offsets` — skipped
/// when false, since computing it is free during assembly but the `Vec`
/// allocation itself isn't, and every caller that doesn't need Task-01
/// exact ranges shouldn't pay for it).
pub(crate) fn encode_frame_with_offsets(
    rgba: &[u8],
    prev: &[u8],
    width: usize,
    height: usize,
    frame_id: u32,
    is_keyframe: bool,
    focus: Option<(usize, usize)>,
    want_offsets: bool,
) -> (Vec<u8>, FrameStats, Vec<TileOffset>) {
    let tiles_x = tiles_in_dim(width);
    let tiles_y = tiles_in_dim(height);
    let tile_count = tiles_x * tiles_y;

    // Fast identical-frame check before the expensive rayon scan.
    // One memcmp of the whole buffer (~0.15 ms at 1080p) vs tile scan (~3.2 ms).
    // Fires whenever the screen is static — very common in typical desktop use.
    if !is_keyframe && rgba == prev {
        return (
            nop_packet_data(frame_id, width, height),
            FrameStats {
                total_tiles: tile_count as u32,
                encoded_bytes: 20,
                ..Default::default()
            },
            Vec::new(),
        );
    }

    // Encode strategy — result is a SPARSE list of dirty tiles only.
    //
    // Each entry: (tile_idx, encoded_bytes, mode). Using filter_map keeps the
    // result O(dirty) not O(total): on a 15%-dirty 1080p frame we get ~506
    // entries instead of 3375, saving allocations and the two post-encode scans.
    //
    // Keyframe: all tiles encoded in one rayon pass.
    // P-frame rayon: combined dirty-check + encode in a single filter_map pass.
    // P-frame sequential: same but without rayon — decided by the calibrated
    // EVRT2CKMAX-TASK-02 marginal-utility scheduler (use_rayon), not a fixed
    // constant. tile_count is the right input here (not dirty count): this
    // branch doesn't know the dirty count ahead of time, it checks-and-
    // encodes in one pass, same as before this change.
    let mut dirty_tiles: Vec<(usize, Vec<u8>, u8)> = if is_keyframe {
        (0..tile_count)
            .into_par_iter()
            .map(|idx| {
                let (data, mode) = encode_tile_buf(
                    rgba,
                    prev,
                    width,
                    height,
                    idx % tiles_x,
                    idx / tiles_x,
                    true,
                );
                (idx, data, mode)
            })
            .collect()
    } else if !use_rayon(tile_count) {
        (0..tile_count)
            .filter_map(|idx| {
                if tile_is_dirty(rgba, prev, width, height, idx % tiles_x, idx / tiles_x) {
                    let (data, mode) = encode_tile_buf(
                        rgba,
                        prev,
                        width,
                        height,
                        idx % tiles_x,
                        idx / tiles_x,
                        false,
                    );
                    Some((idx, data, mode))
                } else {
                    None
                }
            })
            .collect()
    } else {
        (0..tile_count)
            .into_par_iter()
            .filter_map(|idx| {
                if tile_is_dirty(rgba, prev, width, height, idx % tiles_x, idx / tiles_x) {
                    let (data, mode) = encode_tile_buf(
                        rgba,
                        prev,
                        width,
                        height,
                        idx % tiles_x,
                        idx / tiles_x,
                        false,
                    );
                    Some((idx, data, mode))
                } else {
                    None
                }
            })
            .collect()
    };

    // Build dirty-map and stats from compact dirty list — O(dirty) not O(total).
    // Map bit ORDER doesn't matter here (it's just a set of "which idx is dirty",
    // used by the decoder only for a popcount), so this loop runs before the
    // priority reorder below without any correctness dependency on it.
    let map_bytes = (tile_count + 7) / 8;
    let mut tile_map = vec![0u8; map_bytes];
    let mut solid_count = 0u32;
    let mut delta_count = 0u32;
    for &(idx, _, mode) in &dirty_tiles {
        tile_map[idx / 8] |= 1 << (idx % 8);
        match mode {
            MODE_SOLID => solid_count += 1,
            _ => delta_count += 1,
        }
    }
    let dirty_count = dirty_tiles.len() as u32;

    // EVRT2CKMAX-TASK-01: nearest-to-focus first. No-op (raster order
    // preserved) when focus is None.
    order_by_focus(&mut dirty_tiles, tiles_x, focus);

    // Assemble final packet. Capacity estimate: keyframe ~200 B/tile (zstd raw RGBA),
    // P-frame ~30 B/tile (ZRLE sparse XOR delta). +2 bytes/tile for explicit tile_idx (v2).
    let bytes_per_tile = if is_keyframe { 202 } else { 32 };
    let mut out = Vec::with_capacity(20 + map_bytes + dirty_tiles.len() * bytes_per_tile);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(if is_keyframe { FLAG_KEYFRAME } else { 0 });
    out.extend_from_slice(&frame_id.to_le_bytes());
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    out.extend_from_slice(&(map_bytes as u16).to_le_bytes());
    out.extend_from_slice(&tile_map);
    let mut offsets = if want_offsets {
        Vec::with_capacity(dirty_tiles.len())
    } else {
        Vec::new()
    };
    for (idx, data, _) in &dirty_tiles {
        let entry_start = out.len();
        out.extend_from_slice(&(*idx as u16).to_le_bytes());
        out.extend_from_slice(data);
        if want_offsets {
            offsets.push(TileOffset {
                tile_idx: *idx as u16,
                byte_start: entry_start,
                byte_len: out.len() - entry_start,
            });
        }
    }

    let stats = FrameStats {
        total_tiles: tile_count as u32,
        dirty_tiles: dirty_count,
        solid_tiles: solid_count,
        delta_tiles: delta_count,
        encoded_bytes: out.len() as u32,
    };
    (out, stats, offsets)
}

pub(crate) fn encode_frame_with_offsets_and_copy_rects(
    rgba: &[u8],
    predicted_prev: &[u8],
    width: usize,
    height: usize,
    frame_id: u32,
    copy_rects: &[CopyRect],
    focus: Option<(usize, usize)>,
    want_offsets: bool,
) -> (Vec<u8>, FrameStats, Vec<TileOffset>) {
    let tiles_x = tiles_in_dim(width);
    let tiles_y = tiles_in_dim(height);
    let tile_count = tiles_x * tiles_y;

    if rgba == predicted_prev {
        let mut out = Vec::with_capacity(20 + 2 + copy_rects.len() * 24);
        out.extend_from_slice(MAGIC);
        out.push(VERSION_COPY_RECTS);
        out.push(FLAG_COPY_RECTS);
        out.extend_from_slice(&frame_id.to_le_bytes());
        out.extend_from_slice(&(width as u32).to_le_bytes());
        out.extend_from_slice(&(height as u32).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        write_copy_rects(&mut out, copy_rects);
        let encoded_bytes = out.len() as u32;
        return (
            out,
            FrameStats {
                total_tiles: tile_count as u32,
                encoded_bytes,
                ..Default::default()
            },
            Vec::new(),
        );
    }

    let mut dirty_tiles: Vec<(usize, Vec<u8>, u8)> = if !use_rayon(tile_count) {
        (0..tile_count)
            .filter_map(|idx| {
                if tile_is_dirty(
                    rgba,
                    predicted_prev,
                    width,
                    height,
                    idx % tiles_x,
                    idx / tiles_x,
                ) {
                    let (data, mode) = encode_tile_buf(
                        rgba,
                        predicted_prev,
                        width,
                        height,
                        idx % tiles_x,
                        idx / tiles_x,
                        false,
                    );
                    Some((idx, data, mode))
                } else {
                    None
                }
            })
            .collect()
    } else {
        (0..tile_count)
            .into_par_iter()
            .filter_map(|idx| {
                if tile_is_dirty(
                    rgba,
                    predicted_prev,
                    width,
                    height,
                    idx % tiles_x,
                    idx / tiles_x,
                ) {
                    let (data, mode) = encode_tile_buf(
                        rgba,
                        predicted_prev,
                        width,
                        height,
                        idx % tiles_x,
                        idx / tiles_x,
                        false,
                    );
                    Some((idx, data, mode))
                } else {
                    None
                }
            })
            .collect()
    };

    let map_bytes = tile_count.div_ceil(8);
    let mut tile_map = vec![0u8; map_bytes];
    let mut solid_count = 0u32;
    let mut delta_count = 0u32;
    for &(idx, _, mode) in &dirty_tiles {
        tile_map[idx / 8] |= 1 << (idx % 8);
        match mode {
            MODE_SOLID => solid_count += 1,
            _ => delta_count += 1,
        }
    }
    let dirty_count = dirty_tiles.len() as u32;
    order_by_focus(&mut dirty_tiles, tiles_x, focus);

    let mut out =
        Vec::with_capacity(20 + 2 + copy_rects.len() * 24 + map_bytes + dirty_tiles.len() * 32);
    out.extend_from_slice(MAGIC);
    out.push(VERSION_COPY_RECTS);
    out.push(FLAG_COPY_RECTS);
    out.extend_from_slice(&frame_id.to_le_bytes());
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    out.extend_from_slice(&(map_bytes as u16).to_le_bytes());
    write_copy_rects(&mut out, copy_rects);
    out.extend_from_slice(&tile_map);

    let mut offsets = if want_offsets {
        Vec::with_capacity(dirty_tiles.len())
    } else {
        Vec::new()
    };
    for (idx, data, _) in &dirty_tiles {
        let entry_start = out.len();
        out.extend_from_slice(&(*idx as u16).to_le_bytes());
        out.extend_from_slice(data);
        if want_offsets {
            offsets.push(TileOffset {
                tile_idx: *idx as u16,
                byte_start: entry_start,
                byte_len: out.len() - entry_start,
            });
        }
    }

    let stats = FrameStats {
        total_tiles: tile_count as u32,
        dirty_tiles: dirty_count,
        solid_tiles: solid_count,
        delta_tiles: delta_count,
        encoded_bytes: out.len() as u32,
    };
    (out, stats, offsets)
}

fn write_copy_rects(out: &mut Vec<u8>, copy_rects: &[CopyRect]) {
    out.extend_from_slice(&(copy_rects.len().min(u16::MAX as usize) as u16).to_le_bytes());
    for rect in copy_rects.iter().take(u16::MAX as usize) {
        out.extend_from_slice(&rect.src_x.to_le_bytes());
        out.extend_from_slice(&rect.src_y.to_le_bytes());
        out.extend_from_slice(&rect.dst_x.to_le_bytes());
        out.extend_from_slice(&rect.dst_y.to_le_bytes());
        out.extend_from_slice(&rect.width.to_le_bytes());
        out.extend_from_slice(&rect.height.to_le_bytes());
    }
}

fn copy_rect_only_packet_data(
    frame_id: u32,
    width: usize,
    height: usize,
    copy_rects: &[CopyRect],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_HEADER_LEN + 2 + copy_rects.len() * 24);
    out.extend_from_slice(MAGIC);
    out.push(VERSION_COPY_RECTS);
    out.push(FLAG_COPY_RECTS);
    out.extend_from_slice(&frame_id.to_le_bytes());
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    write_copy_rects(&mut out, copy_rects);
    out
}

fn dirty_tile_indices_from_rects(
    dirty_rects: &[DirtyRect],
    width: usize,
    height: usize,
) -> Option<Vec<usize>> {
    if dirty_rects.is_empty() || width == 0 || height == 0 {
        return None;
    }

    let tiles_x = tiles_in_dim(width);
    let tiles_y = tiles_in_dim(height);
    let tile_count = tiles_x * tiles_y;
    let mut tile_map = vec![false; tile_count];
    let mut any_valid = false;

    for rect in dirty_rects.iter().copied() {
        if rect.is_empty() {
            continue;
        }
        let left = (rect.left as usize).min(width);
        let top = (rect.top as usize).min(height);
        let right = (rect.right as usize).min(width);
        let bottom = (rect.bottom as usize).min(height);
        if right <= left || bottom <= top {
            continue;
        }
        any_valid = true;
        let tx0 = left / TILE_SIZE;
        let ty0 = top / TILE_SIZE;
        let tx1 = (right - 1) / TILE_SIZE;
        let ty1 = (bottom - 1) / TILE_SIZE;
        for ty in ty0..=ty1.min(tiles_y.saturating_sub(1)) {
            for tx in tx0..=tx1.min(tiles_x.saturating_sub(1)) {
                tile_map[ty * tiles_x + tx] = true;
            }
        }
    }

    if !any_valid {
        return None;
    }

    Some(
        tile_map
            .into_iter()
            .enumerate()
            .filter_map(|(idx, dirty)| dirty.then_some(idx))
            .collect(),
    )
}

fn encode_copy_rect_frame_from_dirty_indices(
    rgba: &[u8],
    predicted_prev: &[u8],
    width: usize,
    height: usize,
    frame_id: u32,
    copy_rects: &[CopyRect],
    dirty_indices: Vec<usize>,
    focus: Option<(usize, usize)>,
) -> (Vec<u8>, FrameStats) {
    let tiles_x = tiles_in_dim(width);
    let tile_count = tiles_x * tiles_in_dim(height);
    let dirty_tiles: Vec<(usize, Vec<u8>, u8)> = if !use_rayon(dirty_indices.len()) {
        dirty_indices
            .iter()
            .filter_map(|&idx| {
                if tile_is_dirty(
                    rgba,
                    predicted_prev,
                    width,
                    height,
                    idx % tiles_x,
                    idx / tiles_x,
                ) {
                    let (data, mode) = encode_tile_buf(
                        rgba,
                        predicted_prev,
                        width,
                        height,
                        idx % tiles_x,
                        idx / tiles_x,
                        false,
                    );
                    Some((idx, data, mode))
                } else {
                    None
                }
            })
            .collect()
    } else {
        dirty_indices
            .into_par_iter()
            .filter_map(|idx| {
                if tile_is_dirty(
                    rgba,
                    predicted_prev,
                    width,
                    height,
                    idx % tiles_x,
                    idx / tiles_x,
                ) {
                    let (data, mode) = encode_tile_buf(
                        rgba,
                        predicted_prev,
                        width,
                        height,
                        idx % tiles_x,
                        idx / tiles_x,
                        false,
                    );
                    Some((idx, data, mode))
                } else {
                    None
                }
            })
            .collect()
    };

    if dirty_tiles.is_empty() {
        let out = copy_rect_only_packet_data(frame_id, width, height, copy_rects);
        let encoded_bytes = out.len() as u32;
        return (
            out,
            FrameStats {
                total_tiles: tile_count as u32,
                encoded_bytes,
                ..Default::default()
            },
        );
    }

    let map_bytes = tile_count.div_ceil(8);
    let mut tile_map = vec![0u8; map_bytes];
    let mut solid_count = 0u32;
    let mut delta_count = 0u32;
    for &(idx, _, mode) in &dirty_tiles {
        tile_map[idx / 8] |= 1 << (idx % 8);
        match mode {
            MODE_SOLID => solid_count += 1,
            _ => delta_count += 1,
        }
    }
    let dirty_count = dirty_tiles.len() as u32;
    let mut dirty_tiles = dirty_tiles;
    order_by_focus(&mut dirty_tiles, tiles_x, focus);

    let mut out = Vec::with_capacity(
        FRAME_HEADER_LEN + 2 + copy_rects.len() * 24 + map_bytes + dirty_tiles.len() * 32,
    );
    out.extend_from_slice(MAGIC);
    out.push(VERSION_COPY_RECTS);
    out.push(FLAG_COPY_RECTS);
    out.extend_from_slice(&frame_id.to_le_bytes());
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    out.extend_from_slice(&(map_bytes as u16).to_le_bytes());
    write_copy_rects(&mut out, copy_rects);
    out.extend_from_slice(&tile_map);
    for (idx, data, _) in &dirty_tiles {
        out.extend_from_slice(&(*idx as u16).to_le_bytes());
        out.extend_from_slice(data);
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

/// ROADMAP.md Phase 6.4 (cross-codec splicing): encodes ONLY the given
/// tiles, each forced into ABSOLUTE (self-contained, prev-independent)
/// encoding via `encode_tile_buf(..., is_keyframe=true)` — never MODE_DELTA.
/// This is the piece that makes splicing a silicon (NVENC) background with
/// an EVRTCK overlay for the same frame actually decode correctly: the
/// overlay's job is to sit on top of whatever a DIFFERENT codec's lossy
/// reconstruction just put in the client's buffer for those exact tiles,
/// and a MODE_DELTA tile (XOR against `frame`'s CURRENT bytes) would XOR
/// against that unrelated lossy content instead of the true previous frame
/// it was actually computed against — corrupting exactly the pixels this
/// overlay exists to make precise. Forcing absolute encoding (and, on
/// decode, zeroing each tile's rect before applying — see
/// `EvrtckDecoder::apply_absolute_overlay`) sidesteps the problem entirely:
/// `0 XOR absolute_data == absolute_data` regardless of what was there
/// before.
///
/// Wire format: identical 20-byte header + dirty-map + tile-record shape as
/// `encode_frame_with_offsets` (so `apply_absolute_overlay` can reuse the
/// same low-level tile scan/decompress code), except `flags` is always 0
/// (NOT `FLAG_KEYFRAME` — this stream is never meant to be decoded via the
/// ordinary `decode_frame`/`decode_wire` path, which would zero the WHOLE
/// framebuffer on that flag; only `apply_absolute_overlay` should ever
/// consume this stream's bytes).
///
/// `tile_indices` may contain duplicates or be unordered — deduped and
/// sorted ascending before encoding, so the wire stream (and its tile_map)
/// is deterministic regardless of the caller's own ordering (e.g. an
/// Attention Map's tile selection, which isn't itself sorted).
pub fn encode_tile_subset_absolute(
    rgba: &[u8],
    width: usize,
    height: usize,
    frame_id: u32,
    tile_indices: &[u16],
) -> Vec<u8> {
    let tiles_x = tiles_in_dim(width);
    let tiles_y = tiles_in_dim(height);
    let tile_count = tiles_x * tiles_y;

    let mut indices: Vec<u16> = tile_indices
        .iter()
        .copied()
        .filter(|&i| (i as usize) < tile_count)
        .collect();
    indices.sort_unstable();
    indices.dedup();

    let map_bytes = (tile_count + 7) / 8;
    let mut tile_map = vec![0u8; map_bytes];
    for &idx in &indices {
        tile_map[idx as usize / 8] |= 1 << (idx as usize % 8);
    }

    let mut out = Vec::with_capacity(20 + map_bytes + indices.len() * 40);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(0); // flags: deliberately NOT FLAG_KEYFRAME — see doc comment
    out.extend_from_slice(&frame_id.to_le_bytes());
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    out.extend_from_slice(&(map_bytes as u16).to_le_bytes());
    out.extend_from_slice(&tile_map);

    for &idx in &indices {
        let tx = idx as usize % tiles_x;
        let ty = idx as usize / tiles_x;
        // `prev` is never read when `is_keyframe=true` (see encode_tile_buf:
        // the solid-color fast path only reads `rgba`, and the keyframe
        // branch gathers straight from `rgba` too) — an empty slice is safe.
        let (data, _mode) = encode_tile_buf(rgba, &[], width, height, tx, ty, true);
        out.extend_from_slice(&idx.to_le_bytes());
        out.extend_from_slice(&data);
    }

    out
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
    focus: Option<(usize, usize)>,
) -> (Vec<u8>, FrameStats) {
    let tiles_x = tiles_in_dim(width);
    let tile_count = tiles_in_dim(width) * tiles_in_dim(height);

    // dirty_indices order doesn't need to be raster anymore (v2 tiles are
    // self-describing via explicit tile_idx) — this comment kept accurate:
    // input order here is whatever the GPU dirty-detect pass produced.
    // EVRT2CKMAX-TASK-02: calibrated scheduler decision, exact dirty count.
    let dirty_tiles: Vec<(usize, Vec<u8>, u8)> = if !use_rayon(dirty_indices.len()) {
        dirty_indices
            .iter()
            .map(|&idx| {
                let (data, mode) = encode_tile_buf(
                    rgba,
                    prev,
                    width,
                    height,
                    idx % tiles_x,
                    idx / tiles_x,
                    false,
                );
                (idx, data, mode)
            })
            .collect()
    } else {
        dirty_indices
            .into_par_iter()
            .map(|idx| {
                let (data, mode) = encode_tile_buf(
                    rgba,
                    prev,
                    width,
                    height,
                    idx % tiles_x,
                    idx / tiles_x,
                    false,
                );
                (idx, data, mode)
            })
            .collect()
    };

    let map_bytes = (tile_count + 7) / 8;
    let mut tile_map = vec![0u8; map_bytes];
    let mut solid_count = 0u32;
    let mut delta_count = 0u32;
    for &(idx, _, mode) in &dirty_tiles {
        tile_map[idx / 8] |= 1 << (idx % 8);
        match mode {
            MODE_SOLID => solid_count += 1,
            _ => delta_count += 1,
        }
    }
    let dirty_count = dirty_tiles.len() as u32;

    // EVRT2CKMAX-TASK-01: nearest-to-focus first.
    let mut dirty_tiles = dirty_tiles;
    order_by_focus(&mut dirty_tiles, tiles_x, focus);

    let mut out = Vec::with_capacity(20 + map_bytes + dirty_tiles.len() * 32);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(0); // P-frame
    out.extend_from_slice(&frame_id.to_le_bytes());
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    out.extend_from_slice(&(map_bytes as u16).to_le_bytes());
    out.extend_from_slice(&tile_map);
    for (idx, data, _) in &dirty_tiles {
        out.extend_from_slice(&(*idx as u16).to_le_bytes());
        out.extend_from_slice(data);
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

fn copy_rect_is_valid(rect: CopyRect, width: usize, height: usize) -> bool {
    if rect.width == 0 || rect.height == 0 {
        return false;
    }
    let width = width as u64;
    let height = height as u64;
    let src_x = rect.src_x as u64;
    let src_y = rect.src_y as u64;
    let dst_x = rect.dst_x as u64;
    let dst_y = rect.dst_y as u64;
    let rect_w = rect.width as u64;
    let rect_h = rect.height as u64;
    src_x + rect_w <= width
        && dst_x + rect_w <= width
        && src_y + rect_h <= height
        && dst_y + rect_h <= height
}

fn apply_copy_rects(frame: &mut [u8], width: usize, height: usize, copy_rects: &[CopyRect]) {
    for &rect in copy_rects {
        if !copy_rect_is_valid(rect, width, height) {
            continue;
        }
        let rect_w = rect.width as usize;
        let rect_h = rect.height as usize;
        let src_x = rect.src_x as usize;
        let src_y = rect.src_y as usize;
        let dst_x = rect.dst_x as usize;
        let dst_y = rect.dst_y as usize;
        let row_bytes = rect_w * 4;
        let mut temp = Vec::with_capacity(row_bytes * rect_h);
        for row in 0..rect_h {
            let start = ((src_y + row) * width + src_x) * 4;
            temp.extend_from_slice(&frame[start..start + row_bytes]);
        }
        for row in 0..rect_h {
            let dst = ((dst_y + row) * width + dst_x) * 4;
            let src = row * row_bytes;
            frame[dst..dst + row_bytes].copy_from_slice(&temp[src..src + row_bytes]);
        }
    }
}

fn detect_full_width_vertical_scroll(
    prev: &[u8],
    bgra: &[u8],
    width: usize,
    height: usize,
) -> Option<CopyRect> {
    if prev.len() != bgra.len() || width == 0 || height == 0 {
        return None;
    }
    let row_bytes = width.checked_mul(4)?;
    let candidate_offsets = [
        8usize,
        16,
        TILE_SIZE,
        TILE_SIZE * 2,
        TILE_SIZE * 3,
        TILE_SIZE * 4,
        TILE_SIZE * 5,
        TILE_SIZE * 6,
        TILE_SIZE * 7,
        TILE_SIZE * 8,
    ];
    let mut best: Option<(usize, bool)> = None;
    for dy in candidate_offsets {
        if dy >= height {
            continue;
        }
        let moved_rows = height - dy;
        let moved_bytes = moved_rows * row_bytes;
        // Scroll up: current top equals previous area below it.
        let prev_src = dy * row_bytes;
        if prev[prev_src..prev_src + moved_bytes] == bgra[..moved_bytes] {
            best = choose_larger_scroll(best, dy, true);
        }
        // Scroll down: current lower area equals previous top area.
        let cur_dst = dy * row_bytes;
        if prev[..moved_bytes] == bgra[cur_dst..cur_dst + moved_bytes] {
            best = choose_larger_scroll(best, dy, false);
        }
    }

    best.map(|(dy, up)| {
        if up {
            CopyRect {
                src_x: 0,
                src_y: dy as u32,
                dst_x: 0,
                dst_y: 0,
                width: width as u32,
                height: (height - dy) as u32,
            }
        } else {
            CopyRect {
                src_x: 0,
                src_y: 0,
                dst_x: 0,
                dst_y: dy as u32,
                width: width as u32,
                height: (height - dy) as u32,
            }
        }
    })
}

fn choose_larger_scroll(
    current: Option<(usize, bool)>,
    dy: usize,
    up: bool,
) -> Option<(usize, bool)> {
    match current {
        Some((best_dy, _)) if best_dy >= dy => current,
        _ => Some((dy, up)),
    }
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
                tile.push(rgba[o]); // B
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
                delta.push(rgba[o] ^ prev[o]); // B
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

fn decode_frame(
    data: &[u8],
    frame: &mut Vec<u8>,
    width: usize,
    height: usize,
) -> Result<(), EvrtckError> {
    decode_frame_prioritized(data, frame, width, height, None).map(|_apply_order| ())
}

/// ROADMAP.md Phase 3.3: same decode as `decode_frame`, except Phase 3
/// (apply decoded tiles to the framebuffer) processes tiles in DESCENDING
/// `tile_priority` order instead of raw byte-stream order, when a priority
/// function is supplied. `tile_priority(tile_idx)` should return higher
/// values for tiles that matter more (e.g. from a client's decoded APF
/// map); a tile with no entry in the caller's priority source should map
/// to a sensible default (0.0) rather than the function being partial.
///
/// The final framebuffer content is identical either way — tiles are
/// non-overlapping, so paint order never changes the end result — the only
/// thing this changes is which tiles are correct EARLIEST, observable via
/// the returned apply order (a caller/test can check that high-`P_i` tile
/// indices appear before low-`P_i` ones). `tile_priority: None` — which is
/// what `decode_frame` above always passes — makes this function behave
/// byte-for-byte like the original: `dirty` is never re-sorted, so
/// EVRT1's live production decode path (via `decode_frame`/`EvrtckDecoder`)
/// is completely unaffected by this function existing.
fn decode_frame_prioritized(
    data: &[u8],
    frame: &mut Vec<u8>,
    width: usize,
    height: usize,
    tile_priority: Option<&dyn Fn(usize) -> f32>,
) -> Result<Vec<usize>, EvrtckError> {
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
        () => {
            u16::from_le_bytes(read_bytes!(2).try_into().unwrap())
        };
    }
    macro_rules! read_u32 {
        () => {
            u32::from_le_bytes(read_bytes!(4).try_into().unwrap())
        };
    }

    if read_bytes!(4) != MAGIC {
        return Err(EvrtckError::InvalidMagic);
    }
    let ver = read_bytes!(1)[0];
    if ver != VERSION && ver != VERSION_COPY_RECTS {
        return Err(EvrtckError::UnsupportedVersion(ver));
    }
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
        return Ok(Vec::new());
    }

    let map_bytes = read_u16!() as usize;
    if ver == VERSION_COPY_RECTS || flags & FLAG_COPY_RECTS != 0 {
        let copy_rect_count = read_u16!() as usize;
        let mut copy_rects = Vec::with_capacity(copy_rect_count);
        for _ in 0..copy_rect_count {
            let rect = CopyRect {
                src_x: read_u32!(),
                src_y: read_u32!(),
                dst_x: read_u32!(),
                dst_y: read_u32!(),
                width: read_u32!(),
                height: read_u32!(),
            };
            if copy_rect_is_valid(rect, width, height) {
                copy_rects.push(rect);
            }
        }
        apply_copy_rects(frame, width, height, &copy_rects);
    }
    // Borrow tile_map directly from data — no allocation needed. v2 uses this
    // ONLY for its popcount (how many tile entries follow) — tile IDENTITY and
    // stream POSITION come from each entry's explicit tile_idx below, not from
    // bit position. This is what makes priority-ordered (non-raster) tile
    // streams decodable — see EVRT2CKMAX-TASK-01 and the module doc comment.
    let tile_map = read_bytes!(map_bytes);
    let dirty_count: usize = tile_map.iter().map(|b| b.count_ones() as usize).sum();

    let tiles_x = tiles_in_dim(width);
    let tiles_y = tiles_in_dim(height);
    let tile_count = tiles_x * tiles_y;

    // Phase 1: scan the byte stream, recording the position and metadata of each
    // dirty tile without decompressing. O(dirty_count) sequential, no per-tile
    // allocation. Entries may arrive in ANY order (priority order, not raster) —
    // each is self-describing via its explicit tile_idx.
    // Fields: (tile_idx, x0, y0, x1, y1, mode, enc_start, enc_end). `tile_idx`
    // is only needed for the priority sort below (ROADMAP.md Phase 3.3) —
    // Phase 1/2 never used it beyond deriving x0/y0/x1/y1, unchanged from
    // before this field was added.
    let mut dirty: Vec<(usize, usize, usize, usize, usize, u8, usize, usize)> =
        Vec::with_capacity(dirty_count);

    for _ in 0..dirty_count {
        let tile_idx = read_u16!();
        let idx = tile_idx as usize;
        if idx >= tile_count {
            return Err(EvrtckError::InvalidTileIndex(tile_idx));
        }
        let tx = idx % tiles_x;
        let ty = idx / tiles_x;

        need!(1);
        let mode = data[pos];
        pos += 1;

        let x0 = tx * TILE_SIZE;
        let y0 = ty * TILE_SIZE;
        let x1 = (x0 + TILE_SIZE).min(width);
        let y1 = (y0 + TILE_SIZE).min(height);

        match mode {
            MODE_SOLID => {
                need!(4);
                dirty.push((idx, x0, y0, x1, y1, mode, pos, pos + 4));
                pos += 4;
            }
            MODE_DELTA | MODE_ZSTD => {
                let enc_len = read_u32!() as usize;
                need!(enc_len);
                dirty.push((idx, x0, y0, x1, y1, mode, pos, pos + enc_len));
                pos += enc_len;
            }
            m => return Err(EvrtckError::InvalidTileMode(m)),
        }
    }

    // Phase 2: decompress dirty tiles — in parallel when there are enough to amortise
    // rayon spawn cost. Each tile is independent; order is preserved by collect().
    //
    // NOTE: decode (decompress_tile) is a different workload than encode
    // (encode_tile_buf) — cheaper per-tile, no compression search — so the
    // EntropyCoding registry calibrated for encode above does not apply
    // here without its own calibration pass. Out of scope for
    // EVRT2CKMAX-TASK-02's first pass (see task doc Non-Goals: "start with
    // the capabilities that already have partial probing code... expand
    // later"); kept as the same fixed heuristic it always was.
    const DECODE_RAYON_THRESHOLD: usize = 64;
    let decoded: Vec<Result<TilePixels, EvrtckError>> = if dirty.len() < DECODE_RAYON_THRESHOLD {
        dirty
            .iter()
            .map(|&(_, _, _, _, _, mode, enc_start, enc_end)| {
                decompress_tile(data, enc_start, enc_end, mode)
            })
            .collect()
    } else {
        dirty
            .par_iter()
            .map(|&(_, _, _, _, _, mode, enc_start, enc_end)| {
                decompress_tile(data, enc_start, enc_end, mode)
            })
            .collect()
    };

    // ROADMAP.md Phase 3.3: pair each tile with its decoded pixels, then —
    // only if the caller supplied a priority function — stable-sort by
    // DESCENDING priority before Phase 3 paints them. Stable sort keeps the
    // original (byte-stream) relative order for equal-priority tiles, so
    // this is deterministic. With `tile_priority: None` this whole block is
    // skipped and `paint_order` stays exactly the byte-stream order —
    // `decode_frame`'s production callers see zero behavior change.
    let mut paint_order: Vec<(
        usize,
        usize,
        usize,
        usize,
        usize,
        Result<TilePixels, EvrtckError>,
    )> = dirty
        .into_iter()
        .zip(decoded)
        .map(|((idx, x0, y0, x1, y1, _, _, _), pixels)| (idx, x0, y0, x1, y1, pixels))
        .collect();
    if let Some(priority) = tile_priority {
        paint_order.sort_by(|a, b| {
            priority(b.0)
                .partial_cmp(&priority(a.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    let apply_order: Vec<usize> = paint_order.iter().map(|entry| entry.0).collect();

    // Phase 3: apply decoded pixels to the frame buffer sequentially (tiles are
    // non-overlapping, but safe Rust can't express disjoint mutable borrows here).
    for (_, x0, y0, x1, y1, pixels_result) in paint_order {
        let tw4 = (x1 - x0) * 4;
        match pixels_result? {
            TilePixels::Solid(color) => {
                // Pre-fill one row on the stack, then copy_from_slice per frame row.
                // Replaces O(w×h) per-pixel stores with O(h) vectorised row copies.
                let mut row_buf = [0u8; TILE_SIZE * 4];
                for chunk in row_buf[..tw4].chunks_exact_mut(4) {
                    chunk.copy_from_slice(&color);
                }
                let row_pat = &row_buf[..tw4];
                for y in y0..y1 {
                    let rs = (y * width + x0) * 4;
                    frame[rs..rs + tw4].copy_from_slice(row_pat);
                }
            }
            TilePixels::Delta(delta) => {
                let expected = tw4 * (y1 - y0);
                if delta.len() != expected {
                    return Err(EvrtckError::InvalidDelta);
                }
                // Row-level zip gives LLVM a clear SIMD opportunity (AVX2: 32 B/cycle XOR).
                let mut di = 0;
                for y in y0..y1 {
                    let rs = (y * width + x0) * 4;
                    let frame_row = &mut frame[rs..rs + tw4];
                    let delta_row = &delta[di..di + tw4];
                    for (f, d) in frame_row.iter_mut().zip(delta_row) {
                        *f ^= d;
                    }
                    di += tw4;
                }
            }
        }
    }

    Ok(apply_order)
}

fn decompress_tile(
    data: &[u8],
    enc_start: usize,
    enc_end: usize,
    mode: u8,
) -> Result<TilePixels, EvrtckError> {
    match mode {
        MODE_SOLID => {
            let d = &data[enc_start..enc_end];
            Ok(TilePixels::Solid([d[0], d[1], d[2], d[3]]))
        }
        MODE_DELTA => zrle_decode(&data[enc_start..enc_end]).map(TilePixels::Delta),
        MODE_ZSTD => zstd::decode_all(&data[enc_start..enc_end])
            .map_err(|_| EvrtckError::InvalidDelta)
            .map(TilePixels::Delta),
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
            if u64::from_ne_bytes(src[i..i + 8].try_into().unwrap()) != 0 {
                break;
            }
            i += 8;
        }
        // Mop up remaining zero bytes.
        while i < src.len() && src[i] == 0 {
            i += 1;
        }
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
            if i >= src.len() {
                break;
            }
            if src[i] == 0 {
                // Fast check: is this zero run long enough to break?
                let mut z = 0usize;
                while i + z + 8 <= src.len()
                    && u64::from_ne_bytes(src[i + z..i + z + 8].try_into().unwrap()) == 0
                {
                    z += 8;
                }
                while i + z < src.len() && src[i + z] == 0 {
                    z += 1;
                }
                if z >= ZRLE_MIN_RUN {
                    break;
                }
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

fn zrle_decode(src: &[u8]) -> Result<Vec<u8>, EvrtckError> {
    // Pre-allocate for the common case of a full TILE_SIZE×TILE_SIZE tile.
    let mut out = Vec::with_capacity(TILE_SIZE * TILE_SIZE * 4);
    let mut i = 0;
    while i < src.len() {
        let tag = *src.get(i).ok_or(EvrtckError::InvalidDelta)?;
        i += 1;
        match tag {
            0x00 => {
                if i + 2 > src.len() {
                    return Err(EvrtckError::InvalidDelta);
                }
                let count = u16::from_le_bytes([src[i], src[i + 1]]) as usize;
                i += 2;
                out.resize(out.len() + count, 0);
            }
            0x01 => {
                if i + 2 > src.len() {
                    return Err(EvrtckError::InvalidDelta);
                }
                let len = u16::from_le_bytes([src[i], src[i + 1]]) as usize;
                i += 2;
                if i + len > src.len() {
                    return Err(EvrtckError::InvalidDelta);
                }
                out.extend_from_slice(&src[i..i + len]);
                i += len;
            }
            _ => return Err(EvrtckError::InvalidDelta),
        }
    }
    Ok(out)
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
                f[off] = v;
                f[off + 1] = v;
                f[off + 2] = v;
                f[off + 3] = 255;
            }
        }
        f
    }

    fn scrolling_text_like_frame(w: usize, h: usize) -> Vec<u8> {
        let mut f = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 4;
                let line = y / 12;
                let glyph = (x / 7 + line * 13) % 11 == 0;
                let shade = if glyph {
                    230
                } else {
                    24 + ((line * 9) % 28) as u8
                };
                f[off] = shade / 2;
                f[off + 1] = shade;
                f[off + 2] = shade;
                f[off + 3] = 255;
            }
        }
        f
    }

    fn scroll_up_with_new_bottom(base: &[u8], w: usize, h: usize, dy: usize) -> Vec<u8> {
        let mut out = vec![0u8; base.len()];
        let row_bytes = w * 4;
        for y in 0..h - dy {
            let dst = y * row_bytes;
            let src = (y + dy) * row_bytes;
            out[dst..dst + row_bytes].copy_from_slice(&base[src..src + row_bytes]);
        }
        for y in h - dy..h {
            for x in 0..w {
                let off = (y * w + x) * 4;
                let v = 80u8.wrapping_add(((x * 17 + y * 31) & 0xff) as u8);
                out[off] = v;
                out[off + 1] = v.wrapping_add(40);
                out[off + 2] = v.wrapping_add(90);
                out[off + 3] = 255;
            }
        }
        out
    }

    #[test]
    fn copy_rect_frame_decodes_scroll_without_reencoding_moved_area() {
        let (w, h) = (256usize, 256usize);
        let dy = 32usize;
        let base = scrolling_text_like_frame(w, h);
        let scrolled = scroll_up_with_new_bottom(&base, w, h, dy);
        let copy_rect = CopyRect {
            src_x: 0,
            src_y: dy as u32,
            dst_x: 0,
            dst_y: 0,
            width: w as u32,
            height: (h - dy) as u32,
        };

        let mut plain = EvrtckEncoder::new(w, h);
        plain.encode(&base, 1);
        let plain_scroll = plain.encode(&scrolled, 2);

        let mut moved = EvrtckEncoder::new(w, h);
        moved.encode(&base, 1);
        let (copy_scroll, stats) = moved.encode_with_copy_rects(&scrolled, 2, &[copy_rect]);

        let mut dec = EvrtckDecoder::new();
        dec.decode(&EvrtckPacket {
            frame_id: 1,
            width: w as u32,
            height: h as u32,
            data: EvrtckEncoder::new(w, h).encode(&base, 1).data,
        })
        .unwrap();
        let decoded = dec.decode(&copy_scroll).unwrap().to_vec();
        assert_eq!(decoded, bgra_to_rgba(&scrolled));
        assert_eq!(copy_scroll.data[4], VERSION_COPY_RECTS);
        assert!(copy_scroll.data[5] & FLAG_COPY_RECTS != 0);
        assert!(
            copy_scroll.data.len() < plain_scroll.data.len(),
            "copy-rect payload={} must beat plain tile-delta payload={}",
            copy_scroll.data.len(),
            plain_scroll.data.len()
        );
        let exposed_tile_budget = (tiles_in_dim(w) * tiles_in_dim(dy)) as u32;
        assert!(
            stats.dirty_tiles <= exposed_tile_budget,
            "copy rect should leave only exposed strip dirty; got {} tiles, budget {}",
            stats.dirty_tiles,
            exposed_tile_budget
        );
    }

    #[test]
    fn scroll_detection_emits_copy_rect_frame_for_exact_vertical_scroll() {
        let (w, h) = (256usize, 256usize);
        let dy = TILE_SIZE;
        let base = scrolling_text_like_frame(w, h);
        let scrolled = scroll_up_with_new_bottom(&base, w, h, dy);

        let mut enc = EvrtckEncoder::new(w, h);
        let keyframe = enc.encode(&base, 1);
        let (pkt, stats) = enc.encode_with_scroll_detection(&scrolled, 2);

        let mut dec = EvrtckDecoder::new();
        dec.decode(&keyframe).unwrap();
        assert_eq!(dec.decode(&pkt).unwrap(), bgra_to_rgba(&scrolled));
        assert_eq!(pkt.data[4], VERSION_COPY_RECTS);
        assert!(pkt.data[5] & FLAG_COPY_RECTS != 0);
        assert_eq!(stats.dirty_tiles, tiles_in_dim(w) as u32);
    }

    #[test]
    fn scroll_detection_keeps_static_frame_as_nop() {
        let (w, h) = (256usize, 256usize);
        let base = scrolling_text_like_frame(w, h);
        let mut enc = EvrtckEncoder::new(w, h);
        enc.encode(&base, 1);

        let (pkt, stats) = enc.encode_with_scroll_detection(&base, 2);

        assert_eq!(pkt.data.len(), FRAME_HEADER_LEN);
        assert_eq!(pkt.data[4], VERSION);
        assert!(pkt.data[5] & FLAG_NOP != 0);
        assert_eq!(stats.dirty_tiles, 0);
        assert_eq!(stats.encoded_bytes, FRAME_HEADER_LEN as u32);
    }

    #[test]
    fn capture_dirty_rect_limits_pframe_tile_scan() {
        let (w, h) = (128usize, 128usize);
        let base = checkerboard(w, h);
        let mut frame = base.clone();
        dirty_one_pixel_in_tile(&mut frame, w, 15, 4, 201);

        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();
        let key = enc.encode(&base, 1);
        dec.decode(&key).unwrap();

        let dirty_rect = DirtyRect {
            left: 96,
            top: 96,
            right: 97,
            bottom: 97,
        };
        let (pkt, stats) = enc.encode_with_capture_hints(&frame, 2, &[], &[dirty_rect]);
        let decoded = dec.decode(&pkt).unwrap();

        assert_eq!(decoded, bgra_to_rgba(&frame));
        assert_eq!(stats.dirty_tiles, 1);
    }

    #[test]
    fn capture_hints_combine_copy_rects_with_dirty_strip() {
        let (w, h) = (256usize, 256usize);
        let dy = TILE_SIZE;
        let base = scrolling_text_like_frame(w, h);
        let scrolled = scroll_up_with_new_bottom(&base, w, h, dy);

        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();
        let key = enc.encode(&base, 1);
        dec.decode(&key).unwrap();

        let copy_rect = CopyRect {
            src_x: 0,
            src_y: dy as u32,
            dst_x: 0,
            dst_y: 0,
            width: w as u32,
            height: (h - dy) as u32,
        };
        let dirty_rect = DirtyRect {
            left: 0,
            top: (h - dy) as u32,
            right: w as u32,
            bottom: h as u32,
        };

        let (pkt, stats) = enc.encode_with_capture_hints(&scrolled, 2, &[copy_rect], &[dirty_rect]);
        let decoded = dec.decode(&pkt).unwrap();

        assert_eq!(decoded, bgra_to_rgba(&scrolled));
        assert_eq!(stats.dirty_tiles, tiles_in_dim(w) as u32);
        assert_eq!(pkt.data[4], VERSION_COPY_RECTS);
        assert!(pkt.data[5] & FLAG_COPY_RECTS != 0);
    }

    fn dirty_tiles_frame(
        base: &[u8],
        w: usize,
        h: usize,
        dirty_fraction: f32,
        noisy: bool,
    ) -> Vec<u8> {
        let mut frame = base.to_vec();
        let tiles_x = tiles_in_dim(w);
        let tiles_y = tiles_in_dim(h);
        let total_tiles = tiles_x * tiles_y;
        let dirty_tiles = ((total_tiles as f32) * dirty_fraction).round() as usize;
        let mut seed = 0x4556_5254_434b_u64;
        for tile_idx in 0..dirty_tiles.min(total_tiles) {
            let tx = tile_idx % tiles_x;
            let ty = tile_idx / tiles_x;
            let x0 = tx * TILE_SIZE;
            let y0 = ty * TILE_SIZE;
            let x1 = (x0 + TILE_SIZE).min(w);
            let y1 = (y0 + TILE_SIZE).min(h);
            for y in y0..y1 {
                for x in x0..x1 {
                    let off = (y * w + x) * 4;
                    if noisy {
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                        frame[off] = seed as u8;
                        frame[off + 1] = (seed >> 8) as u8;
                        frame[off + 2] = (seed >> 16) as u8;
                    } else {
                        frame[off] = 255 - base[off];
                        frame[off + 1] = 255 - base[off + 1];
                        frame[off + 2] = 255 - base[off + 2];
                    }
                }
            }
        }
        frame
    }

    #[test]
    fn frame_analysis_keeps_low_entropy_desktop_delta_on_evrtck() {
        let (w, h) = (320, 192);
        let base = solid_frame(w, h, [30, 30, 30, 255]);
        let frame = dirty_tiles_frame(&base, w, h, 0.50, false);
        let mut enc = EvrtckEncoder::new(w, h);
        enc.encode(&base, 1);

        let analysis = enc.analyze_next_frame(&frame);

        assert!(analysis.dirty_ratio > 0.45);
        assert!(
            analysis.entropy_score < 0.25,
            "low-entropy UI delta must not look like video/noise: {analysis:?}"
        );
        assert!(
            !analysis.prefer_silicon,
            "EVRTCK must remain preferred for low-entropy desktop deltas: {analysis:?}"
        );
    }

    #[test]
    fn frame_analysis_marks_high_entropy_dirty_delta_for_silicon() {
        let (w, h) = (320, 192);
        let base = solid_frame(w, h, [30, 30, 30, 255]);
        let frame = dirty_tiles_frame(&base, w, h, 0.50, true);
        let mut enc = EvrtckEncoder::new(w, h);
        enc.encode(&base, 1);

        let analysis = enc.analyze_next_frame(&frame);

        assert!(analysis.dirty_ratio > 0.45);
        assert!(
            analysis.entropy_score > 0.60,
            "noisy delta should be classified as high entropy: {analysis:?}"
        );
        assert!(
            analysis.prefer_silicon,
            "high-entropy video-like delta should trigger silicon recommendation: {analysis:?}"
        );
    }

    // ── TASK-01 exact tile offsets (ROADMAP.md Phase 1.2) ──────────────────────

    #[test]
    fn hinted_frame_analysis_uses_dirty_rects_for_scheduler_decision() {
        let (w, h) = (256, 256);
        let base = solid_frame(w, h, [30, 30, 30, 255]);
        let frame = dirty_tiles_frame(&base, w, h, 1.0, true);
        let mut enc = EvrtckEncoder::new(w, h);
        enc.encode(&base, 1);

        let full = enc.analyze_next_frame(&frame);
        let hinted = enc.analyze_next_frame_with_dirty_rects(
            &frame,
            &[DirtyRect {
                left: 0,
                top: 0,
                right: TILE_SIZE as u32,
                bottom: TILE_SIZE as u32,
            }],
        );

        assert!(full.prefer_silicon);
        assert_eq!(hinted.dirty_tiles, 1);
        assert!(
            hinted.dirty_ratio < 0.02,
            "one hinted tile should stay a tiny dirty ratio: {hinted:?}"
        );
        assert!(
            !hinted.prefer_silicon,
            "small dirty capture region should stay on EVRTCK even if a full-frame scan would prefer silicon: {hinted:?}"
        );
    }

    #[test]
    fn tile_offsets_point_at_the_correct_tile_idx_prefix() {
        // 64×64 = 2×2 tiles at TILE_SIZE=32. Keyframe: every tile is dirty.
        let mut enc = EvrtckEncoder::new(64, 64);
        let (packet, stats, offsets) = enc.encode_with_offsets(&checkerboard(64, 64), 0);
        assert_eq!(
            offsets.len(),
            stats.dirty_tiles as usize,
            "one offset per dirty tile"
        );
        assert_eq!(offsets.len(), 4, "2x2 tile grid, keyframe → all 4 dirty");

        for off in &offsets {
            // Every tile entry is `[tile_idx u16 LE][data...]` — the offset's
            // byte_start must land exactly on that prefix, not mid-tile.
            let idx_bytes = &packet.data[off.byte_start..off.byte_start + 2];
            let idx = u16::from_le_bytes([idx_bytes[0], idx_bytes[1]]);
            assert_eq!(
                idx, off.tile_idx,
                "byte_start must point at this tile's own [tile_idx] prefix"
            );
            assert!(
                off.byte_start + off.byte_len <= packet.data.len(),
                "byte range must stay inside the packet"
            );
        }
    }

    #[test]
    fn tile_offsets_do_not_overlap_and_cover_every_dirty_tile_once() {
        // 128×128 = 4×4 tiles — dirty only the top-left quadrant (4 tiles),
        // so this also exercises the P-frame sparse path, not just keyframe.
        let mut enc = EvrtckEncoder::new(128, 128);
        let _ = enc.encode(&black(128, 128), 0); // seed keyframe baseline
        let mut frame = black(128, 128);
        for y in 0..64 {
            for x in 0..64 {
                let i = (y * 128 + x) * 4;
                frame[i] = 255;
            }
        }
        let (packet, stats, offsets) = enc.encode_with_offsets(&frame, 1);
        assert_eq!(offsets.len(), stats.dirty_tiles as usize);

        let mut sorted = offsets.clone();
        sorted.sort_by_key(|o| o.byte_start);
        for pair in sorted.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert!(
                a.byte_start + a.byte_len <= b.byte_start,
                "tile byte ranges must not overlap: {a:?} vs {b:?}"
            );
        }
        // Every tile_idx returned must be unique — no tile double-counted.
        let mut idxs: Vec<u16> = offsets.iter().map(|o| o.tile_idx).collect();
        idxs.sort_unstable();
        idxs.dedup();
        assert_eq!(
            idxs.len(),
            offsets.len(),
            "no duplicate tile_idx across offsets"
        );
        let _ = packet; // packet content already exercised above
    }

    #[test]
    fn encode_with_offsets_matches_encode_bit_for_bit() {
        // The offsets facade must not change what actually gets sent —
        // only add metadata alongside the identical wire bytes.
        let frame = checkerboard(64, 64);
        let mut enc_a = EvrtckEncoder::new(64, 64);
        let mut enc_b = EvrtckEncoder::new(64, 64);
        let plain = enc_a.encode(&frame, 0);
        let (with_offsets, _stats, _offsets) = enc_b.encode_with_offsets(&frame, 0);
        assert_eq!(
            plain.data, with_offsets.data,
            "encode() and encode_with_offsets() must produce identical wire bytes"
        );
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
        assert!(
            enc.len() < 40,
            "sparse delta should compress well: {} bytes",
            enc.len()
        );
    }

    #[test]
    fn zrle_roundtrip_random_like() {
        let data: Vec<u8> = (0u16..1000)
            .map(|i| (i.wrapping_mul(6271) & 0xFF) as u8)
            .collect();
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
    fn pframe_noise_roundtrip_is_strict_rgba() {
        let (w, h) = (96usize, 64usize);
        let base = solid_frame(w, h, [30, 40, 90, 255]);
        let mut frame = base.clone();
        let mut rng = 0x4556_5254_434b_u64;
        for y in 16..48 {
            for x in 24..72 {
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                let off = (y * w + x) * 4;
                frame[off] = rng as u8;
                frame[off + 1] = (rng >> 8) as u8;
                frame[off + 2] = (rng >> 16) as u8;
                frame[off + 3] = 255;
            }
        }

        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();
        dec.decode(&enc.encode(&base, 1)).unwrap();
        let pkt = enc.encode(&frame, 2);

        assert_eq!(dec.decode(&pkt).unwrap(), bgra_to_rgba(&frame).as_slice());
    }

    #[test]
    fn sequential_frames_reconstruct_correctly() {
        let (w, h) = (64, 64);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();

        let f1 = solid_frame(w, h, [10, 20, 30, 255]);
        let f2 = checkerboard(w, h); // grayscale — BGRA==RGBA for this one
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
        for i in 0..100 {
            frame[i * 4] = 0;
        }
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
                if i % 2 == 0 {
                    [200u8, 100, 50, 255]
                } else {
                    [30u8, 180, 240, 255]
                }
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
        let pkt = EvrtckPacket {
            frame_id: 0,
            width: 32,
            height: 32,
            data: b"BAAD\x01\x00\x00\x00\x00\x00\x20\x00\x00\x00\x20\x00\x00\x00\x02\x00".to_vec(),
        };
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

    // ── ROADMAP.md Phase 6.4 investigation: silicon-frame/EVRTCK framebuffer
    // desync bug found while designing cross-codec splicing ────────────────

    /// Without `sync_from_rgba`, a client that displayed one frame via a
    /// DIFFERENT codec (e.g. IS_SILICON/NVENC — simulated here by simply not
    /// feeding that frame to `dec` at all, exactly like the pre-fix client
    /// loop) ends up applying the next EVRTCK MODE_DELTA P-frame against a
    /// stale framebuffer, producing corrupted pixels — this is the bug this
    /// segment's fix addresses, reproduced here as a negative control so a
    /// future regression can't silently reintroduce it.
    #[test]
    fn decoding_a_delta_frame_against_a_desynced_buffer_corrupts_pixels() {
        let (w, h) = (64, 64);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();

        let frame0 = checkerboard(w, h);
        let pkt0 = enc.encode(&frame0, 0);
        dec.decode(&pkt0).unwrap();
        assert_eq!(dec.current_frame(), bgra_to_rgba(&frame0).as_slice());

        // Host encodes frame1 too (both providers always run), but this
        // frame is never sent to the client on the wire — NVENC "won" the
        // race this frame in the real system. The encoder's OWN internal
        // prev still advances to frame1's true content.
        let mut frame1 = frame0.clone();
        for i in 0..64 {
            frame1[i * 4] = 200;
        } // change one tile's blue channel (BGRA byte 0)
        let _pkt1_never_sent = enc.encode(&frame1, 1);
        // dec is NOT told about frame1 at all — the pre-fix behavior.

        let mut frame2 = frame1.clone();
        for i in 0..64 {
            frame2[i * 4 + 1] = 77;
        } // change the SAME tile further
        let pkt2 = enc.encode(&frame2, 2); // real MODE_DELTA against frame1, sent for real
        dec.decode(&pkt2).unwrap();

        // dec.frame is still frame0-shaped for that tile; XORing frame2's
        // delta-from-frame1 onto frame0 does NOT reconstruct frame2.
        assert_ne!(
            dec.current_frame(),
            bgra_to_rgba(&frame2).as_slice(),
            "expected corruption from the desynced buffer — if this now passes, the bug is gone even without sync_from_rgba"
        );
    }

    /// The fix: calling `sync_from_rgba` with the real pixels shown by the
    /// other codec keeps the decoder's tracked buffer truthful, so the next
    /// real MODE_DELTA P-frame decodes correctly — no corruption, no dropped
    /// frame, no keyframe request needed.
    #[test]
    fn sync_from_rgba_keeps_the_next_delta_frame_correct() {
        let (w, h) = (64, 64);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();

        let frame0 = checkerboard(w, h);
        let pkt0 = enc.encode(&frame0, 0);
        dec.decode(&pkt0).unwrap();

        let mut frame1 = frame0.clone();
        for i in 0..64 {
            frame1[i * 4] = 200;
        }
        let _pkt1_never_sent = enc.encode(&frame1, 1);
        // The client DID decode frame1 via the other codec (NVENC/H264) and
        // displayed it — tell this decoder about the real pixels shown.
        dec.sync_from_rgba(&bgra_to_rgba(&frame1), w, h);
        assert_eq!(dec.current_frame(), bgra_to_rgba(&frame1).as_slice());

        let mut frame2 = frame1.clone();
        for i in 0..64 {
            frame2[i * 4 + 1] = 77;
        }
        let pkt2 = enc.encode(&frame2, 2);
        dec.decode(&pkt2).unwrap();

        assert_eq!(
            dec.current_frame(),
            bgra_to_rgba(&frame2).as_slice(),
            "sync_from_rgba should have kept the decoder's buffer truthful enough for the next delta to reconstruct frame2 exactly"
        );
    }

    /// `sync_from_rgba` is a no-op (doesn't panic, doesn't corrupt existing
    /// state) when handed a buffer of the wrong length for the given
    /// dimensions — a genuine caller bug, not something to silently truncate
    /// or pad around.
    #[test]
    fn sync_from_rgba_ignores_a_mismatched_buffer_length() {
        let (w, h) = (16, 16);
        let mut dec = EvrtckDecoder::new();
        let mut enc = EvrtckEncoder::new(w, h);
        let frame0 = solid_frame(w, h, [10, 20, 30, 255]);
        dec.decode(&enc.encode(&frame0, 0)).unwrap();
        let before = dec.current_frame().to_vec();

        dec.sync_from_rgba(&[0u8; 4], w, h); // way too short for 16x16
        assert_eq!(dec.current_frame(), before.as_slice());
    }

    // ── ROADMAP.md Phase 6.4: cross-codec splicing overlay ──────────────────

    #[test]
    fn absolute_overlay_roundtrips_the_selected_tiles_exactly() {
        let (w, h) = (128, 96);
        let frame = checkerboard(w, h);
        let mut dec = EvrtckDecoder::new();
        // Establish dimensions with a black base (stands in for a
        // background layer from a different codec).
        dec.sync_from_rgba(&black(w, h), w, h);

        let tiles_x = w.div_ceil(TILE_SIZE);
        let selected: Vec<u16> = vec![0, 3, tiles_x as u16 + 1]; // scattered tiles
        let overlay = encode_tile_subset_absolute(&frame, w, h, 1, &selected);
        let applied = dec.apply_absolute_overlay(&overlay).unwrap();

        let mut expected_sorted = selected.clone();
        expected_sorted.sort_unstable();
        let mut applied_sorted: Vec<u16> = applied.iter().map(|&i| i as u16).collect();
        applied_sorted.sort_unstable();
        assert_eq!(applied_sorted, expected_sorted);

        // Every selected tile's pixels must exactly match the true frame
        // (checkerboard, converted to the decoder's RGBA layout); untouched
        // tiles must remain black (the background layer, unmodified).
        let expected_rgba = bgra_to_rgba(&frame);
        for &idx in &selected {
            let tx = idx as usize % tiles_x;
            let ty = idx as usize / tiles_x;
            let x0 = tx * TILE_SIZE;
            let y0 = ty * TILE_SIZE;
            let x1 = (x0 + TILE_SIZE).min(w);
            let y1 = (y0 + TILE_SIZE).min(h);
            for y in y0..y1 {
                let rs = (y * w + x0) * 4;
                let tw4 = (x1 - x0) * 4;
                assert_eq!(
                    dec.current_frame()[rs..rs + tw4],
                    expected_rgba[rs..rs + tw4],
                    "tile {idx} row {y} should exactly match the true frame"
                );
            }
        }
        // A tile NOT in the selection (e.g. the last one) should still be
        // black — the overlay must not have touched it.
        let untouched_idx = (tiles_x * h.div_ceil(TILE_SIZE)) - 1;
        if !selected.contains(&(untouched_idx as u16)) {
            let tx = untouched_idx % tiles_x;
            let ty = untouched_idx / tiles_x;
            let x0 = tx * TILE_SIZE;
            let y0 = (ty * TILE_SIZE).min(h - 1);
            let rs = (y0 * w + x0) * 4;
            assert_eq!(dec.current_frame()[rs..rs + 4], [0, 0, 0, 0]);
        }
    }

    /// The whole point of this overlay design: applying it on top of a
    /// background that is COMPLETELY DIFFERENT from what a normal
    /// MODE_DELTA P-frame would have assumed as `prev` must still
    /// reconstruct the true tile content exactly — proving the "zero the
    /// rect, then apply" trick actually neutralizes the desync problem
    /// `sync_from_rgba` was built to guard against, for the specific tiles
    /// the overlay covers.
    #[test]
    fn absolute_overlay_is_correct_even_over_a_wildly_different_background() {
        let (w, h) = (64, 64);
        let true_frame = checkerboard(w, h);
        let mut dec = EvrtckDecoder::new();

        // A background that shares NOTHING with true_frame — e.g. a
        // completely different lossy NVENC reconstruction.
        let unrelated_background = solid_frame(w, h, [200, 5, 90, 255]);
        dec.sync_from_rgba(&bgra_to_rgba(&unrelated_background), w, h);

        let overlay = encode_tile_subset_absolute(&true_frame, w, h, 1, &[0]);
        dec.apply_absolute_overlay(&overlay).unwrap();

        let expected_rgba = bgra_to_rgba(&true_frame);
        let tw4 = TILE_SIZE * 4;
        for y in 0..TILE_SIZE {
            let rs = y * w * 4;
            assert_eq!(
                dec.current_frame()[rs..rs + tw4],
                expected_rgba[rs..rs + tw4]
            );
        }
    }

    #[test]
    fn absolute_overlay_rejects_dimension_mismatch() {
        let mut dec = EvrtckDecoder::new();
        dec.sync_from_rgba(&black(32, 32), 32, 32);
        let overlay = encode_tile_subset_absolute(&checkerboard(64, 64), 64, 64, 1, &[0]);
        assert_eq!(
            dec.apply_absolute_overlay(&overlay),
            Err(EvrtckError::DimensionMismatch {
                expected: (32, 32),
                got: (64, 64)
            })
        );
    }

    #[test]
    fn encode_tile_subset_absolute_dedupes_and_ignores_out_of_range_indices() {
        let (w, h) = (64, 64); // 2x2 tiles
        let frame = checkerboard(w, h);
        let mut dec = EvrtckDecoder::new();
        dec.sync_from_rgba(&black(w, h), w, h);

        // Duplicate 0 three times, include an out-of-range index (99).
        let overlay = encode_tile_subset_absolute(&frame, w, h, 1, &[0, 0, 0, 99]);
        let applied = dec.apply_absolute_overlay(&overlay).unwrap();
        assert_eq!(
            applied,
            vec![0],
            "duplicates collapse to one entry, out-of-range indices are dropped"
        );
    }

    #[test]
    fn empty_tile_selection_produces_a_no_op_overlay() {
        let (w, h) = (32, 32);
        let mut dec = EvrtckDecoder::new();
        dec.sync_from_rgba(&solid_frame(w, h, [1, 2, 3, 255]), w, h);
        let before = dec.current_frame().to_vec();

        let overlay = encode_tile_subset_absolute(&checkerboard(w, h), w, h, 1, &[]);
        let applied = dec.apply_absolute_overlay(&overlay).unwrap();
        assert!(applied.is_empty());
        assert_eq!(dec.current_frame(), before.as_slice());
    }

    #[test]
    fn nop_frame_for_identical_pframe() {
        let (w, h) = (64, 64);
        let frame = solid_frame(w, h, [42, 100, 200, 255]);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();

        // Keyframe — establishes prev on both sides.
        let kf = enc.encode(&frame, 1);
        assert!(
            kf.data[5] & FLAG_KEYFRAME != 0,
            "first frame must be keyframe"
        );
        let pixels_after_kf = dec.decode(&kf).unwrap().to_vec();

        // Second encode with the SAME frame → must produce a NOP packet.
        let nop = enc.encode(&frame, 2);
        assert!(
            nop.data[5] & FLAG_NOP != 0,
            "identical P-frame must set FLAG_NOP"
        );
        assert!(
            nop.data.len() <= 20,
            "NOP packet must be tiny (got {} bytes)",
            nop.data.len()
        );

        // Decoder must return the same pixels as after the keyframe.
        let pixels_after_nop = dec.decode(&nop).unwrap();
        assert_eq!(
            pixels_after_nop,
            pixels_after_kf.as_slice(),
            "NOP decode must preserve frame buffer"
        );

        // After a change the NOP must NOT fire.
        let frame2 = solid_frame(w, h, [1, 2, 3, 255]);
        let pkt = enc.encode(&frame2, 3);
        assert!(pkt.data[5] & FLAG_NOP == 0, "changed frame must not be NOP");
    }

    // ── EVRT2CKMAX-TASK-01: focus-priority tile ordering (v2 wire format) ──────

    /// Extract the tile_idx of the FIRST tile entry in the wire stream.
    /// Layout: header(20) + tile_map(map_bytes) + [tile_idx(2) + mode(1) + ...]...
    fn first_wire_tile_idx(data: &[u8]) -> u16 {
        let map_bytes = u16::from_le_bytes([data[18], data[19]]) as usize;
        let start = 20 + map_bytes;
        u16::from_le_bytes(data[start..start + 2].try_into().unwrap())
    }

    /// Mark the tile at raster index `idx` (in a `tiles_x`-wide grid) dirty by
    /// changing one pixel strictly inside it.
    fn dirty_one_pixel_in_tile(frame: &mut [u8], w: usize, idx: usize, tiles_x: usize, value: u8) {
        let tx = idx % tiles_x;
        let ty = idx / tiles_x;
        let px = tx * TILE_SIZE + 3;
        let py = ty * TILE_SIZE + 3;
        let off = (py * w + px) * 4;
        frame[off] = value;
    }

    #[test]
    fn focus_priority_orders_nearest_tile_first_and_decodes_correctly() {
        let (w, h) = (128, 128); // 4×4 = 16 tiles of 32×32, tiles_x = 4
        let mut frame = black(w, h);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();

        dec.decode(&enc.encode(&frame, 1)).unwrap(); // keyframe baseline both sides

        // Dirty diagonal tiles 0, 5, 10, 15 — raster order would emit 0 first.
        for &idx in &[0usize, 5, 10, 15] {
            dirty_one_pixel_in_tile(&mut frame, w, idx, 4, 200);
        }

        // Focus on tile 15's pixel area — nearest dirty tile to the focus is 15 itself.
        enc.set_focus_pixel((3 * TILE_SIZE + 1) as u32, (3 * TILE_SIZE + 1) as u32);
        let pkt = enc.encode(&frame, 2);

        assert_eq!(
            first_wire_tile_idx(&pkt.data),
            15,
            "nearest-to-focus dirty tile must be emitted first, not raster-first"
        );

        // Correctness: priority-ordered stream must still decode byte-identical.
        let got = dec.decode(&pkt).unwrap();
        assert_eq!(got, bgra_to_rgba(&frame).as_slice());
    }

    #[test]
    fn clear_focus_reverts_to_raster_order() {
        let (w, h) = (128, 128);
        let mut frame = black(w, h);
        let mut enc = EvrtckEncoder::new(w, h);
        enc.encode(&frame, 1);

        for &idx in &[15usize, 10, 5, 0] {
            dirty_one_pixel_in_tile(&mut frame, w, idx, 4, 200);
        }

        enc.set_focus_pixel((3 * TILE_SIZE + 1) as u32, (3 * TILE_SIZE + 1) as u32);
        enc.clear_focus();
        let pkt = enc.encode(&frame, 2);

        assert_eq!(
            first_wire_tile_idx(&pkt.data),
            0,
            "without a focus point, tiles must stay in ascending raster order"
        );
    }

    #[test]
    fn focus_ordering_survives_equal_distance_ties_deterministically() {
        // Two tiles equidistant from focus — tie must break by original
        // (raster) order, not be arbitrary, so encode output is reproducible.
        let (w, h) = (128, 128);
        let mut frame = black(w, h);
        let mut enc = EvrtckEncoder::new(w, h);
        enc.encode(&frame, 1);

        // Tiles 1 and 4 are both distance-1 (Chebyshev) from tile 0 in a 4-wide grid:
        // idx 1 = (1,0), idx 4 = (0,1). Focus at tile 0 → both equidistant.
        dirty_one_pixel_in_tile(&mut frame, w, 4, 4, 111);
        dirty_one_pixel_in_tile(&mut frame, w, 1, 4, 111);
        enc.set_focus_pixel(1, 1); // tile (0,0)
        let pkt1 = enc.encode(&frame, 2);

        // Re-run from scratch — must produce the identical first tile every time.
        let mut frame_b = black(w, h);
        let mut enc_b = EvrtckEncoder::new(w, h);
        enc_b.encode(&frame_b, 1);
        dirty_one_pixel_in_tile(&mut frame_b, w, 4, 4, 111);
        dirty_one_pixel_in_tile(&mut frame_b, w, 1, 4, 111);
        enc_b.set_focus_pixel(1, 1);
        let pkt2 = enc_b.encode(&frame_b, 2);

        assert_eq!(
            first_wire_tile_idx(&pkt1.data),
            first_wire_tile_idx(&pkt2.data)
        );
        // And it must be the raster-earlier of the tied pair (tile 1, not tile 4).
        assert_eq!(first_wire_tile_idx(&pkt1.data), 1);
    }

    #[test]
    fn gpu_path_pframe_focus_ordering_and_roundtrip() {
        // Exercises encode_pframe_from_dirty_indices directly — the function
        // the WGPU backend calls after GPU dirty-tile detection.
        let (w, h) = (128, 128);
        let prev = black(w, h);
        let mut frame = black(w, h);
        let dirty_idxs = vec![0usize, 5, 10, 15];
        for &idx in &dirty_idxs {
            dirty_one_pixel_in_tile(&mut frame, w, idx, 4, 77);
        }

        // Focus tile (2,2) = raster idx 10 — nearest among the dirty set is itself.
        let (data, stats) =
            encode_pframe_from_dirty_indices(&frame, &prev, w, h, 1, dirty_idxs, Some((2, 2)));
        assert_eq!(stats.dirty_tiles, 4);
        assert_eq!(first_wire_tile_idx(&data), 10);

        // Round-trip via the low-level decode_frame (prev/frame both black, so
        // BGRA==RGBA at the byte level and no conversion is needed for the base).
        let mut out_frame = vec![0u8; w * h * 4];
        decode_frame(&data, &mut out_frame, w, h).unwrap();
        assert_eq!(out_frame, bgra_to_rgba(&frame));
    }

    #[test]
    fn corrupt_tile_index_is_rejected_not_undefined_behavior() {
        let (w, h) = (64, 64); // 2×2 = 4 tiles
        let frame = solid_frame(w, h, [9, 9, 9, 255]);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut pkt = enc.encode(&frame, 1);

        // Corrupt the first tile_idx (right after header + tile_map) to an
        // out-of-range value (tile_count for 2×2 is 4, so 999 is invalid).
        let map_bytes = u16::from_le_bytes([pkt.data[18], pkt.data[19]]) as usize;
        let idx_pos = 20 + map_bytes;
        pkt.data[idx_pos..idx_pos + 2].copy_from_slice(&999u16.to_le_bytes());

        let mut dec = EvrtckDecoder::new();
        assert_eq!(dec.decode(&pkt), Err(EvrtckError::InvalidTileIndex(999)));
    }

    // ── ROADMAP.md Phase 3.3: priority-driven apply order ──────────────────

    #[test]
    fn decode_frame_with_no_priority_applies_in_byte_stream_order() {
        // Regression guard for `decode_frame`'s production callers
        // (EVRT1's live decode path via `EvrtckDecoder`): the apply order
        // with `tile_priority: None` must be exactly the order tiles
        // appear in the wire stream — unchanged from before this function
        // existed. 4x4 grid = 4 tiles at TILE_SIZE=32, all dirty on a
        // keyframe, so the wire order is whatever the encoder emitted.
        let (w, h) = (64, 64);
        let mut enc = EvrtckEncoder::new(w, h);
        let pkt = enc.encode(&checkerboard(w, h), 0);
        let mut frame = black(w, h);
        let apply_order = decode_frame_prioritized(&pkt.data, &mut frame, w, h, None).unwrap();
        assert_eq!(apply_order.len(), 4, "all 4 tiles dirty on a keyframe");
        // Byte-stream order for a keyframe with no focus set is raster order.
        assert_eq!(apply_order, vec![0, 1, 2, 3]);
    }

    #[test]
    fn decode_frame_prioritized_paints_the_highest_priority_tile_first() {
        // 4x4 native tiles (2x2 grid, TILE_SIZE=32 → 64x64px), all dirty.
        // Byte-stream (raster) order is [0,1,2,3] — priority says tile 3
        // (bottom-right) matters most, so it must be FIRST in apply order,
        // not last, once a priority function is supplied.
        let (w, h) = (64, 64);
        let mut enc = EvrtckEncoder::new(w, h);
        let pkt = enc.encode(&checkerboard(w, h), 0);
        let mut frame = black(w, h);
        let priority = |idx: usize| if idx == 3 { 1.0 } else { 0.0 };
        let apply_order =
            decode_frame_prioritized(&pkt.data, &mut frame, w, h, Some(&priority)).unwrap();
        assert_eq!(
            apply_order.first(),
            Some(&3),
            "the one high-priority tile must be painted first"
        );
        // The final framebuffer must be identical regardless of paint order
        // (tiles are non-overlapping) — reordering must never change the
        // actual picture, only when each part of it became correct.
        let mut frame_unordered = black(w, h);
        decode_frame_prioritized(&pkt.data, &mut frame_unordered, w, h, None).unwrap();
        assert_eq!(
            frame, frame_unordered,
            "paint order must not change the final reconstructed frame"
        );
    }

    #[test]
    fn decode_frame_prioritized_is_a_stable_sort_for_equal_priorities() {
        // All tiles equal priority → must fall back to byte-stream order,
        // exactly like `tile_priority: None` — a real APF grid coarser
        // than the tile grid can legitimately assign the same priority to
        // several tiles, and that must not scramble their relative order.
        let (w, h) = (64, 64);
        let mut enc = EvrtckEncoder::new(w, h);
        let pkt = enc.encode(&checkerboard(w, h), 0);
        let mut frame = black(w, h);
        let flat_priority = |_idx: usize| 0.5f32;
        let apply_order =
            decode_frame_prioritized(&pkt.data, &mut frame, w, h, Some(&flat_priority)).unwrap();
        assert_eq!(apply_order, vec![0, 1, 2, 3]);
    }
}
