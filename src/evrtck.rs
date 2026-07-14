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
//!   2. XOR delta against the previous frame compressed with ZRLE (zero-run
//!      length encoding) — near-optimal for the sparse non-zero patterns that
//!      arise in UI deltas.
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
//! ```

use std::fmt;

// ── Constants ────────────────────────────────────────────────────────────────

pub const MAGIC: &[u8; 4] = b"EVCK";
pub const VERSION: u8 = 1;

/// Pixels per tile edge. 32×32 = 1024 px, maps well onto L1 cache lines.
pub const TILE_SIZE: usize = 32;

const MODE_SOLID: u8 = 1;
const MODE_DELTA: u8 = 2;

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
            Self::InvalidDelta => write!(f, "malformed ZRLE delta stream"),
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

// ── Stateful encoder ─────────────────────────────────────────────────────────

pub struct EvrtckEncoder {
    prev: Vec<u8>,
    width: usize,
    height: usize,
}

impl EvrtckEncoder {
    /// Create an encoder for frames of the given dimensions (RGBA, 4 bytes/pixel).
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            prev: vec![0u8; width * height * 4],
            width,
            height,
        }
    }

    /// Encode one RGBA frame. Returns both the packet and per-frame stats.
    ///
    /// `rgba` must be row-major, `width * height * 4` bytes.
    pub fn encode_with_stats(&mut self, rgba: &[u8], frame_id: u32) -> (EvrtckPacket, FrameStats) {
        debug_assert_eq!(rgba.len(), self.width * self.height * 4);
        let (data, stats) = encode_frame(rgba, &self.prev, self.width, self.height, frame_id);
        self.prev.copy_from_slice(rgba);
        let pkt = EvrtckPacket {
            frame_id,
            width: self.width as u32,
            height: self.height as u32,
            data,
        };
        (pkt, stats)
    }

    pub fn encode(&mut self, rgba: &[u8], frame_id: u32) -> EvrtckPacket {
        self.encode_with_stats(rgba, frame_id).0
    }

    /// Force-key: treat the next frame as if the previous was all black.
    /// Use after a seek or connection reset.
    pub fn request_keyframe(&mut self) {
        self.prev.fill(0);
    }

    pub fn width(&self) -> usize { self.width }
    pub fn height(&self) -> usize { self.height }
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
        let w = pkt.width as usize;
        let h = pkt.height as usize;
        if self.width != w || self.height != h || self.frame.len() != w * h * 4 {
            self.frame = vec![0u8; w * h * 4];
            self.width = w;
            self.height = h;
        }
        decode_frame(&pkt.data, &mut self.frame, w, h)?;
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

fn encode_frame(
    rgba: &[u8],
    prev: &[u8],
    width: usize,
    height: usize,
    frame_id: u32,
) -> (Vec<u8>, FrameStats) {
    let tiles_x = tiles_in_dim(width);
    let tiles_y = tiles_in_dim(height);
    let tile_count = tiles_x * tiles_y;

    // Determine dirty tiles in one pass.
    let mut dirty = vec![false; tile_count];
    let mut dirty_count = 0u32;
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            if tile_is_dirty(rgba, prev, width, height, tx, ty) {
                dirty[ty * tiles_x + tx] = true;
                dirty_count += 1;
            }
        }
    }

    // Build the tile dirty-map bitfield.
    let map_bytes = (tile_count + 7) / 8;
    let mut tile_map = vec![0u8; map_bytes];
    for (i, &d) in dirty.iter().enumerate() {
        if d { tile_map[i / 8] |= 1 << (i % 8); }
    }

    // Header: magic(4) + version(1) + flags(1) + frame_id(4) + w(4) + h(4) + map_bytes(2)
    let mut out = Vec::with_capacity(20 + map_bytes + (dirty_count as usize) * 64);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(0u8);
    out.extend_from_slice(&frame_id.to_le_bytes());
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    out.extend_from_slice(&(map_bytes as u16).to_le_bytes());
    out.extend_from_slice(&tile_map);

    let mut solid_count = 0u32;
    let mut delta_count = 0u32;

    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            if !dirty[ty * tiles_x + tx] { continue; }
            let mode = encode_tile(&mut out, rgba, prev, width, height, tx, ty);
            match mode {
                MODE_SOLID => solid_count += 1,
                MODE_DELTA => delta_count += 1,
                _ => {}
            }
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
fn tiles_in_dim(px: usize) -> usize {
    (px + TILE_SIZE - 1) / TILE_SIZE
}

fn tile_is_dirty(
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
    for y in y0..y1 {
        let base = (y * width + x0) * 4;
        let end = base + (x1 - x0) * 4;
        if rgba[base..end] != prev[base..end] {
            return true;
        }
    }
    false
}

/// Encode one dirty tile into `out`. Returns the chosen mode byte.
fn encode_tile(
    out: &mut Vec<u8>,
    rgba: &[u8],
    prev: &[u8],
    width: usize,
    height: usize,
    tx: usize,
    ty: usize,
) -> u8 {
    let x0 = tx * TILE_SIZE;
    let y0 = ty * TILE_SIZE;
    let x1 = (x0 + TILE_SIZE).min(width);
    let y1 = (y0 + TILE_SIZE).min(height);
    let tw = x1 - x0;
    let th = y1 - y0;
    let pixel_bytes = tw * th * 4;

    // Gather tile pixels into a flat buffer.
    let mut tile = Vec::with_capacity(pixel_bytes);
    let mut tile_prev = Vec::with_capacity(pixel_bytes);
    for y in y0..y1 {
        let base = (y * width + x0) * 4;
        let end = base + tw * 4;
        tile.extend_from_slice(&rgba[base..end]);
        tile_prev.extend_from_slice(&prev[base..end]);
    }

    // Fast path: entire tile is a single color.
    if let Some(color) = try_solid(&tile) {
        out.push(MODE_SOLID);
        out.extend_from_slice(&color);
        return MODE_SOLID;
    }

    // XOR delta then ZRLE compress.
    let mut delta = vec![0u8; pixel_bytes];
    for i in 0..pixel_bytes {
        delta[i] = tile[i] ^ tile_prev[i];
    }
    let compressed = zrle_encode(&delta);

    out.push(MODE_DELTA);
    out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&compressed);
    MODE_DELTA
}

fn try_solid(tile: &[u8]) -> Option<[u8; 4]> {
    let mut chunks = tile.chunks_exact(4);
    let first = chunks.next()?;
    let color = [first[0], first[1], first[2], first[3]];
    for chunk in chunks {
        if chunk != color { return None; }
    }
    Some(color)
}

// ── Core: decode ─────────────────────────────────────────────────────────────

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
    let _flags = read_bytes!(1)[0];
    let _frame_id = read_u32!();
    let w = read_u32!() as usize;
    let h = read_u32!() as usize;
    if w != width || h != height {
        return Err(EvrtckError::DimensionMismatch {
            expected: (width as u32, height as u32),
            got: (w as u32, h as u32),
        });
    }

    let map_bytes = read_u16!() as usize;
    let tile_map = read_bytes!(map_bytes).to_vec();

    let tiles_x = tiles_in_dim(width);
    let tiles_y = tiles_in_dim(height);

    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let idx = ty * tiles_x + tx;
            let dirty = (tile_map.get(idx / 8).copied().unwrap_or(0) >> (idx % 8)) & 1 == 1;
            if !dirty { continue; }

            need!(1);
            let mode = data[pos];
            pos += 1;

            let x0 = tx * TILE_SIZE;
            let y0 = ty * TILE_SIZE;
            let x1 = (x0 + TILE_SIZE).min(width);
            let y1 = (y0 + TILE_SIZE).min(height);

            match mode {
                MODE_SOLID => {
                    let color = read_bytes!(4);
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let off = (y * width + x) * 4;
                            frame[off..off + 4].copy_from_slice(color);
                        }
                    }
                }
                MODE_DELTA => {
                    let enc_len = read_u32!() as usize;
                    let enc = read_bytes!(enc_len);
                    let delta = zrle_decode(enc).ok_or(EvrtckError::InvalidDelta)?;

                    let expected = (x1 - x0) * (y1 - y0) * 4;
                    if delta.len() < expected { return Err(EvrtckError::InvalidDelta); }

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
                m => return Err(EvrtckError::InvalidTileMode(m)),
            }
        }
    }

    Ok(())
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
        // Count consecutive zeros.
        let z_start = i;
        while i < src.len() && src[i] == 0 { i += 1; }
        let zeros = i - z_start;

        if zeros >= 4 || (zeros > 0 && i == src.len()) {
            let mut rem = zeros;
            while rem > 0 {
                let n = rem.min(65535) as u16;
                out.push(0x00);
                out.extend_from_slice(&n.to_le_bytes());
                rem -= n as usize;
            }
            continue;
        }

        // Roll back: short zero run gets absorbed into a literal.
        i = z_start;
        let lit_start = i;
        loop {
            if i >= src.len() { break; }
            if src[i] == 0 {
                // How long is this zero run?
                let mut z = 0;
                while i + z < src.len() && src[i + z] == 0 { z += 1; }
                if z >= 4 { break; }
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
    let mut out = Vec::new();
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

    fn solid_frame(w: usize, h: usize, color: [u8; 4]) -> Vec<u8> {
        color.iter().cycle().take(w * h * 4).copied().collect()
    }

    fn black(w: usize, h: usize) -> Vec<u8> {
        vec![0u8; w * h * 4]
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
        let (w, h) = (32, 32);
        let frame = solid_frame(w, h, [200, 100, 50, 255]);
        let mut enc = EvrtckEncoder::new(w, h);
        let mut dec = EvrtckDecoder::new();
        let pkt = enc.encode(&frame, 1);
        assert_eq!(dec.decode(&pkt).unwrap(), frame.as_slice());
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
        let f2 = checkerboard(w, h);
        let f3 = solid_frame(w, h, [0, 0, 0, 0]);

        for (i, frame) in [&f1, &f2, &f3].iter().enumerate() {
            let pkt = enc.encode(frame, i as u32 + 1);
            let got = dec.decode(&pkt).unwrap();
            assert_eq!(got, frame.as_slice(), "frame {i} mismatch");
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
        frame[0] = 255; // change exactly one byte in tile (0,0)
        let pkt = enc.encode(&frame, 2);
        let got = dec.decode(&pkt).unwrap();
        assert_eq!(got[0], 255);
        assert_eq!(&got[1..], &frame[1..]);

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
        assert_eq!(got, frame.as_slice());
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
}
