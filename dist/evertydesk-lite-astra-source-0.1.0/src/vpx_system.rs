//! Minimal FFI bindings to the **system** libvpx (Debian/Astra: `libvpx-dev`)
//! for VP9 decoding.
//!
//! Used where building libvpx from source fails — e.g. Astra Linux, whose old
//! GCC can't link libvpx's AVX-512 code (`_mm512_reduce_add_epi32`). The
//! distro-packaged libvpx was built for the local toolchain, so linking it is
//! reliable. Enabled with the `live-vpx-system` feature; `build.rs` adds
//! `-l vpx`.
//!
//! Struct layouts and the decoder ABI version match libvpx 1.7.x (the version
//! shipped by Debian buster / Astra 1.7).

#![cfg(feature = "live-vpx-system")]

use std::os::raw::{c_char, c_int, c_long, c_uint, c_void};
use std::ptr;

#[repr(C)]
struct VpxCodecCtx {
    name: *const c_char,
    iface: *mut c_void,
    err: c_int,
    err_detail: *const c_char,
    init_flags: c_long,
    config: *const c_void,
    priv_: *mut c_void,
}

#[repr(C)]
struct VpxImage {
    fmt: c_int,
    cs: c_int,
    range: c_int,
    w: c_uint,
    h: c_uint,
    bit_depth: c_uint,
    d_w: c_uint,
    d_h: c_uint,
    r_w: c_uint,
    r_h: c_uint,
    x_chroma_shift: c_uint,
    y_chroma_shift: c_uint,
    planes: [*mut u8; 4],
    stride: [c_int; 4],
    bps: c_int,
    user_priv: *mut c_void,
    img_data: *mut u8,
    img_data_owner: c_int,
    self_allocd: c_int,
    fb_priv: *mut c_void,
}

type VpxCodecIter = *const c_void;

#[link(name = "vpx")]
extern "C" {
    fn vpx_codec_vp9_dx() -> *mut c_void;
    fn vpx_codec_dec_init_ver(
        ctx: *mut VpxCodecCtx,
        iface: *mut c_void,
        cfg: *const c_void,
        flags: c_long,
        ver: c_int,
    ) -> c_int;
    fn vpx_codec_decode(
        ctx: *mut VpxCodecCtx,
        data: *const u8,
        data_sz: c_uint,
        user_priv: *mut c_void,
        deadline: c_long,
    ) -> c_int;
    fn vpx_codec_get_frame(ctx: *mut VpxCodecCtx, iter: *mut VpxCodecIter) -> *mut VpxImage;
    fn vpx_codec_destroy(ctx: *mut VpxCodecCtx) -> c_int;
}

const VPX_DECODER_ABI_VERSION: c_int = 12; // libvpx 1.7.x
const VPX_CODEC_OK: c_int = 0;

/// A VP9 decoder backed by the system libvpx.
pub struct Vp9Decoder {
    ctx: Box<VpxCodecCtx>,
}

impl Vp9Decoder {
    pub fn new() -> Option<Self> {
        let mut ctx = Box::new(unsafe { std::mem::zeroed::<VpxCodecCtx>() });
        let r = unsafe {
            vpx_codec_dec_init_ver(
                &mut *ctx,
                vpx_codec_vp9_dx(),
                ptr::null(),
                0,
                VPX_DECODER_ABI_VERSION,
            )
        };
        if r == VPX_CODEC_OK {
            Some(Self { ctx })
        } else {
            None
        }
    }

    /// Decode one VP9 frame → `(width, height, rgba)`.
    /// Returns `None` if the frame produced no output (decoder buffering / error).
    pub fn decode(&mut self, data: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
        if data.is_empty() {
            return None;
        }
        let r = unsafe {
            vpx_codec_decode(
                &mut *self.ctx,
                data.as_ptr(),
                data.len() as c_uint,
                ptr::null_mut(),
                0,
            )
        };
        if r != VPX_CODEC_OK {
            return None;
        }
        let mut iter: VpxCodecIter = ptr::null();
        let img = unsafe { vpx_codec_get_frame(&mut *self.ctx, &mut iter) };
        if img.is_null() {
            return None;
        }
        let img = unsafe { &*img };
        let w = img.d_w as usize;
        let h = img.d_h as usize;
        if w == 0 || h == 0 || img.planes[0].is_null() {
            return None;
        }
        Some((w, h, image_to_rgba(img, w, h)))
    }
}

impl Drop for Vp9Decoder {
    fn drop(&mut self) {
        unsafe {
            vpx_codec_destroy(&mut *self.ctx);
        }
    }
}

/// Convert a planar YUV `vpx_image` (I420 / I444, BT.601) to RGBA.
/// Chroma subsampling is taken from `x_chroma_shift` / `y_chroma_shift`, so it
/// works for both 4:2:0 and 4:4:4 frames.
fn image_to_rgba(img: &VpxImage, w: usize, h: usize) -> Vec<u8> {
    let y_plane = img.planes[0];
    let u_plane = img.planes[1];
    let v_plane = img.planes[2];
    let ys = img.stride[0] as usize;
    let us = img.stride[1] as usize;
    let vs = img.stride[2] as usize;
    let xcs = img.x_chroma_shift as usize;
    let ycs = img.y_chroma_shift as usize;

    let mut rgba = vec![0u8; w * h * 4];
    if u_plane.is_null() || v_plane.is_null() {
        return rgba;
    }
    unsafe {
        for j in 0..h {
            let cj = j >> ycs;
            for i in 0..w {
                let ci = i >> xcs;
                let y = *y_plane.add(j * ys + i) as i32;
                let u = *u_plane.add(cj * us + ci) as i32 - 128;
                let v = *v_plane.add(cj * vs + ci) as i32 - 128;
                let c = y - 16;
                let r = (298 * c + 409 * v + 128) >> 8;
                let g = (298 * c - 100 * u - 208 * v + 128) >> 8;
                let b = (298 * c + 516 * u + 128) >> 8;
                let o = (j * w + i) * 4;
                rgba[o] = r.clamp(0, 255) as u8;
                rgba[o + 1] = g.clamp(0, 255) as u8;
                rgba[o + 2] = b.clamp(0, 255) as u8;
                rgba[o + 3] = 255;
            }
        }
    }
    rgba
}
