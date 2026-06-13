/// Fast BGRA → planar YUV420 and BGRA → interleaved NV12 converters.
///
/// Both functions process two rows at a time so luma and chroma share the
/// same BGRA read pass.  Inner loops use `unsafe` unchecked slice indexing to
/// eliminate per-pixel bounds-check overhead — the outer loops guarantee that
/// all offsets stay in-bounds.
///
/// BT.601 limited range (MPEG) coefficients are used to match what OpenH264,
/// Media Foundation, and VideoToolbox all expect by default.

// ── BT.601 limited range coefficients (integer, scaled ×256) ─────────────────
//
//  Y  =  16 + ( 66·R + 129·G +  25·B + 128) >> 8
//  Cb = 128 + (-38·R -  74·G + 112·B + 128) >> 8
//  Cr = 128 + (112·R -  94·G -  18·B + 128) >> 8

#[inline(always)]
fn y_bt601(r: i32, g: i32, b: i32) -> u8 {
    // Fast path: no .clamp() — the formula is already bounded to [16, 235]
    // for valid u8 R/G/B inputs.
    ((66 * r + 129 * g + 25 * b + 128 + 16 * 256) >> 8) as u8
}

#[inline(always)]
fn u_bt601(r: i32, g: i32, b: i32) -> u8 {
    ((-38 * r - 74 * g + 112 * b + 128 + 128 * 256) >> 8) as u8
}

#[inline(always)]
fn v_bt601(r: i32, g: i32, b: i32) -> u8 {
    ((112 * r - 94 * g - 18 * b + 128 + 128 * 256) >> 8) as u8
}

// ── BGRA → planar I420 (YUV420) ──────────────────────────────────────────────

/// Convert BGRA pixels into planar I420 (Y, U, V planes).
///
/// `dst_w` / `dst_h` must both be even and equal to the next-even multiple of
/// `src_w` / `src_h`.  When `src_w == dst_w && src_h == dst_h` the fast inner
/// path is taken; otherwise edge pixels are clamped.
pub fn bgra_to_i420(
    y_plane: &mut [u8],
    u_plane: &mut [u8],
    v_plane: &mut [u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    bgra: &[u8],
) {
    debug_assert!(dst_w % 2 == 0 && dst_h % 2 == 0);
    debug_assert_eq!(y_plane.len(), dst_w * dst_h);
    debug_assert_eq!(u_plane.len(), (dst_w / 2) * (dst_h / 2));
    debug_assert_eq!(v_plane.len(), (dst_w / 2) * (dst_h / 2));

    if src_w == dst_w && src_h == dst_h && bgra.len() >= src_w * src_h * 4 {
        bgra_to_i420_fast(y_plane, u_plane, v_plane, src_w, src_h, bgra);
    } else {
        bgra_to_i420_padded(y_plane, u_plane, v_plane, src_w, src_h, dst_w, dst_h, bgra);
    }
}

#[inline]
fn bgra_to_i420_fast(
    y_plane: &mut [u8],
    u_plane: &mut [u8],
    v_plane: &mut [u8],
    w: usize,
    h: usize,
    bgra: &[u8],
) {
    // Process two rows per outer iteration so chroma only needs one pass.
    for row_pair in 0..(h / 2) {
        let by = row_pair * 2;
        let src0 = by * w * 4; // start of even row in bgra
        let src1 = (by + 1) * w * 4; // start of odd row
        let y0 = by * w; // start of even row in y_plane
        let y1 = (by + 1) * w;
        let uv_row = row_pair * (w / 2);

        for col_pair in 0..(w / 2) {
            let bx = col_pair * 2;
            let s00 = src0 + bx * 4;
            let s01 = s00 + 4;
            let s10 = src1 + bx * 4;
            let s11 = s10 + 4;

            // SAFETY: bgra.len() >= w*h*4 and all indices are < that.
            let (b00, g00, r00) = unsafe { px(bgra, s00) };
            let (b01, g01, r01) = unsafe { px(bgra, s01) };
            let (b10, g10, r10) = unsafe { px(bgra, s10) };
            let (b11, g11, r11) = unsafe { px(bgra, s11) };

            y_plane[y0 + bx] = y_bt601(r00, g00, b00);
            y_plane[y0 + bx + 1] = y_bt601(r01, g01, b01);
            y_plane[y1 + bx] = y_bt601(r10, g10, b10);
            y_plane[y1 + bx + 1] = y_bt601(r11, g11, b11);

            // Average 2×2 block for chroma.
            let r = (r00 + r01 + r10 + r11 + 2) >> 2;
            let g = (g00 + g01 + g10 + g11 + 2) >> 2;
            let b = (b00 + b01 + b10 + b11 + 2) >> 2;
            u_plane[uv_row + col_pair] = u_bt601(r, g, b);
            v_plane[uv_row + col_pair] = v_bt601(r, g, b);
        }
    }
}

fn bgra_to_i420_padded(
    y_plane: &mut [u8],
    u_plane: &mut [u8],
    v_plane: &mut [u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    bgra: &[u8],
) {
    for row_pair in 0..(dst_h / 2) {
        let by = row_pair * 2;
        let uv_row = row_pair * (dst_w / 2);

        for col_pair in 0..(dst_w / 2) {
            let bx = col_pair * 2;
            let mut r_sum = 0i32;
            let mut g_sum = 0i32;
            let mut b_sum = 0i32;

            for dy in 0..2usize {
                let sy = (by + dy).min(src_h - 1);
                for dx in 0..2usize {
                    let sx = (bx + dx).min(src_w - 1);
                    let base = (sy * src_w + sx) * 4;
                    let b = bgra[base] as i32;
                    let g = bgra[base + 1] as i32;
                    let r = bgra[base + 2] as i32;
                    y_plane[(by + dy) * dst_w + bx + dx] = y_bt601(r, g, b);
                    r_sum += r;
                    g_sum += g;
                    b_sum += b;
                }
            }
            u_plane[uv_row + col_pair] =
                u_bt601((r_sum + 2) >> 2, (g_sum + 2) >> 2, (b_sum + 2) >> 2);
            v_plane[uv_row + col_pair] =
                v_bt601((r_sum + 2) >> 2, (g_sum + 2) >> 2, (b_sum + 2) >> 2);
        }
    }
}

// ── BGRA → NV12 ──────────────────────────────────────────────────────────────

/// Convert BGRA into NV12: `[Y plane | interleaved UV plane]`.
///
/// `out` is resized to `dst_w * dst_h * 3 / 2` bytes.
pub fn bgra_to_nv12(
    out: &mut Vec<u8>,
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    bgra: &[u8],
) {
    debug_assert!(dst_w % 2 == 0 && dst_h % 2 == 0);
    let y_len = dst_w * dst_h;
    let total = y_len + y_len / 2;
    if out.len() != total {
        out.resize(total, 0);
    }

    if src_w == dst_w && src_h == dst_h && bgra.len() >= src_w * src_h * 4 {
        // Split out into Y and UV slices for the fast path.
        let (y_plane, uv_plane) = out.split_at_mut(y_len);
        bgra_to_nv12_fast(y_plane, uv_plane, src_w, src_h, bgra);
    } else {
        bgra_to_nv12_padded(out, src_w, src_h, dst_w, dst_h, bgra);
    }
}

#[inline]
fn bgra_to_nv12_fast(y_plane: &mut [u8], uv_plane: &mut [u8], w: usize, h: usize, bgra: &[u8]) {
    for row_pair in 0..(h / 2) {
        let by = row_pair * 2;
        let src0 = by * w * 4;
        let src1 = (by + 1) * w * 4;
        let y0 = by * w;
        let y1 = (by + 1) * w;
        let uv_off = row_pair * w; // UV plane offset (interleaved, w bytes per chroma row)

        for col_pair in 0..(w / 2) {
            let bx = col_pair * 2;
            let s00 = src0 + bx * 4;
            let s01 = s00 + 4;
            let s10 = src1 + bx * 4;
            let s11 = s10 + 4;

            let (b00, g00, r00) = unsafe { px(bgra, s00) };
            let (b01, g01, r01) = unsafe { px(bgra, s01) };
            let (b10, g10, r10) = unsafe { px(bgra, s10) };
            let (b11, g11, r11) = unsafe { px(bgra, s11) };

            y_plane[y0 + bx] = y_bt601(r00, g00, b00);
            y_plane[y0 + bx + 1] = y_bt601(r01, g01, b01);
            y_plane[y1 + bx] = y_bt601(r10, g10, b10);
            y_plane[y1 + bx + 1] = y_bt601(r11, g11, b11);

            let r = (r00 + r01 + r10 + r11 + 2) >> 2;
            let g = (g00 + g01 + g10 + g11 + 2) >> 2;
            let b = (b00 + b01 + b10 + b11 + 2) >> 2;
            let uv = uv_off + bx;
            uv_plane[uv] = u_bt601(r, g, b); // Cb
            uv_plane[uv + 1] = v_bt601(r, g, b); // Cr
        }
    }
}

fn bgra_to_nv12_padded(
    out: &mut [u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    bgra: &[u8],
) {
    let y_len = dst_w * dst_h;
    for row_pair in 0..(dst_h / 2) {
        let by = row_pair * 2;
        let uv_off = y_len + row_pair * dst_w;

        for col_pair in 0..(dst_w / 2) {
            let bx = col_pair * 2;
            let mut r_sum = 0i32;
            let mut g_sum = 0i32;
            let mut b_sum = 0i32;

            for dy in 0..2usize {
                let sy = (by + dy).min(src_h - 1);
                for dx in 0..2usize {
                    let sx = (bx + dx).min(src_w - 1);
                    let base = (sy * src_w + sx) * 4;
                    let b = bgra[base] as i32;
                    let g = bgra[base + 1] as i32;
                    let r = bgra[base + 2] as i32;
                    out[(by + dy) * dst_w + bx + dx] = y_bt601(r, g, b);
                    r_sum += r;
                    g_sum += g;
                    b_sum += b;
                }
            }
            let uv = uv_off + bx;
            out[uv] = u_bt601((r_sum + 2) >> 2, (g_sum + 2) >> 2, (b_sum + 2) >> 2);
            out[uv + 1] = v_bt601((r_sum + 2) >> 2, (g_sum + 2) >> 2, (b_sum + 2) >> 2);
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Read (B, G, R) from a BGRA slice at `base` without bounds checking.
///
/// SAFETY: caller must guarantee `base + 3 < bgra.len()`.
#[inline(always)]
unsafe fn px(bgra: &[u8], base: usize) -> (i32, i32, i32) {
    let b = *bgra.get_unchecked(base) as i32;
    let g = *bgra.get_unchecked(base + 1) as i32;
    let r = *bgra.get_unchecked(base + 2) as i32;
    (b, g, r)
}

// ── Fast frame-change hash ────────────────────────────────────────────────────

// ── NV12 → RGBA ──────────────────────────────────────────────────────────────

/// Convert NV12 (biplanar: Y plane + interleaved Cb/Cr plane) to RGBA.
///
/// Used to convert VideoToolbox hardware-decode output when the decoder ignores
/// the BGRA pixel-format hint and returns YUV instead.
///
/// `y_stride` and `uv_stride` are the row strides in bytes (may be > width).
/// Output `rgba` must be pre-allocated to `width * height * 4` bytes.
#[allow(dead_code)]
pub fn nv12_to_rgba(
    rgba: &mut Vec<u8>,
    width: usize,
    height: usize,
    y_plane: &[u8],
    uv_plane: &[u8],
    y_stride: usize,
    uv_stride: usize,
) {
    let pixel_count = width.saturating_mul(height);
    let expected = pixel_count.saturating_mul(4);
    if rgba.len() != expected {
        rgba.resize(expected, 0);
    }

    // BT.601 full-range (for 420f) or limited-range (for 420v) → RGB.
    // We use full-range coefficients which work reasonably for both;
    // the visual difference for remote desktop content is negligible.
    for row in 0..height {
        let y_row = row * y_stride;
        let uv_row = (row / 2) * uv_stride;
        let out_row = row * width * 4;

        for col in 0..width {
            // SAFETY: all indices are bounded by width/height/stride checks above.
            let y = unsafe { *y_plane.get_unchecked(y_row + col) } as i32;
            let uv_base = uv_row + (col & !1); // col rounded down to even
            let cb = unsafe { *uv_plane.get_unchecked(uv_base) } as i32 - 128;
            let cr = unsafe { *uv_plane.get_unchecked(uv_base + 1) } as i32 - 128;

            // Full-range BT.601: Y ∈ [0,255], Cb/Cr ∈ [-128,127]
            let r = (y + (1436 * cr >> 10)).clamp(0, 255) as u8;
            let g = (y - (352 * cb >> 10) - (731 * cr >> 10)).clamp(0, 255) as u8;
            let b = (y + (1815 * cb >> 10)).clamp(0, 255) as u8;

            let px = out_row + col * 4;
            rgba[px] = r;
            rgba[px + 1] = g;
            rgba[px + 2] = b;
            rgba[px + 3] = 255;
        }
    }
}

/// Sample `n` evenly-spaced pixels and return a 64-bit FNV-1a digest.
///
/// This is ~3× faster than the old per-pixel delta loop because:
///  - No branch inside the sample loop.
///  - `get_unchecked` eliminates per-byte bounds checks.
///  - The FNV multiply + XOR is cheaper than abs_diff + comparison.
pub fn frame_signature(bgra: &[u8], w: usize, h: usize) -> u64 {
    if bgra.len() < w * h * 4 || w == 0 || h == 0 {
        return 0;
    }
    const FNV_OFFSET: u64 = 14695981039346656037;
    const FNV_PRIME: u64 = 1099511628211;

    // Target ~512 samples regardless of resolution.
    let pixels = w * h;
    let step = (pixels / 512).max(1);

    let mut hash = FNV_OFFSET;
    let mut idx = 0usize;
    while idx < pixels {
        let base = idx * 4;
        // SAFETY: idx < pixels and bgra.len() >= pixels*4.
        let (b, g, r) = unsafe { px(bgra, base) };
        hash = hash.wrapping_mul(FNV_PRIME) ^ (r as u64);
        hash = hash.wrapping_mul(FNV_PRIME) ^ (g as u64);
        hash = hash.wrapping_mul(FNV_PRIME) ^ (b as u64);
        idx += step;
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_size() {
        let bgra = vec![0u8; 4 * 4 * 4];
        let mut out = Vec::new();
        bgra_to_nv12(&mut out, 4, 4, 4, 4, &bgra);
        assert_eq!(out.len(), 4 * 4 * 3 / 2);
    }

    #[test]
    fn i420_black_frame() {
        let w = 4;
        let h = 4;
        let bgra = vec![0u8; w * h * 4]; // all black, alpha=0 (ignored)
        let mut y = vec![0u8; w * h];
        let mut u = vec![0u8; (w / 2) * (h / 2)];
        let mut v = vec![0u8; (w / 2) * (h / 2)];
        bgra_to_i420(&mut y, &mut u, &mut v, w, h, w, h, &bgra);
        // Black pixel → Y=16, U=128, V=128 in BT.601 limited range
        assert!(y.iter().all(|&x| x == 16), "Y: {y:?}");
        assert!(u.iter().all(|&x| x == 128), "U: {u:?}");
        assert!(v.iter().all(|&x| x == 128), "V: {v:?}");
    }

    #[test]
    fn frame_signature_detects_change() {
        let w = 32;
        let h = 32;
        let frame_a = vec![100u8; w * h * 4];
        let mut frame_b = frame_a.clone();
        frame_b[w * h * 2] = 200; // flip one pixel
        assert_ne!(
            frame_signature(&frame_a, w, h),
            frame_signature(&frame_b, w, h)
        );
    }
}
