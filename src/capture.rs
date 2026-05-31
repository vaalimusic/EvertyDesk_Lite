//! Cross-platform screen capture.
//!
//! Windows: uses GDI BitBlt (no external deps, already have the `windows` crate).
//! Other OS: returns `None` — to be implemented in a future session.
//!
//! Returns `(width, height, bgra_pixels)`.

#[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
mod win {
    use std::mem::size_of;

    use windows::Win32::{
        Foundation::HWND,
        Graphics::Gdi::{
            BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
            GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
            DIB_RGB_COLORS, RGBQUAD, SRCCOPY,
        },
        UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN},
    };

    pub fn capture() -> Option<(u32, u32, Vec<u8>)> {
        unsafe { capture_inner() }
    }

    unsafe fn capture_inner() -> Option<(u32, u32, Vec<u8>)> {
        let width = GetSystemMetrics(SM_CXSCREEN);
        let height = GetSystemMetrics(SM_CYSCREEN);
        if width <= 0 || height <= 0 {
            return None;
        }
        let (w, h) = (width as u32, height as u32);

        // Screen DC (entire virtual desktop).
        let hdc_screen = GetDC(HWND(0));
        if hdc_screen.is_invalid() {
            return None;
        }

        // Memory DC + bitmap.
        let hdc_mem = CreateCompatibleDC(hdc_screen);
        let hbmp = CreateCompatibleBitmap(hdc_screen, width, height);
        let hbmp_old = SelectObject(hdc_mem, hbmp);

        // Copy screen → memory DC.
        let _ = BitBlt(hdc_mem, 0, 0, width, height, hdc_screen, 0, 0, SRCCOPY);

        // Extract pixels as 32-bit BGRA (no palette).
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative height → top-down (row 0 = top of screen).
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0 as u32,
                ..BITMAPINFOHEADER::default()
            },
            bmiColors: [RGBQUAD::default()],
        };

        let pixel_count = (w * h) as usize;
        let mut buf = vec![0u8; pixel_count * 4];

        GetDIBits(
            hdc_mem,
            hbmp,
            0,
            h,
            Some(buf.as_mut_ptr() as *mut _),
            &mut bi,
            DIB_RGB_COLORS,
        );

        // Cleanup.
        SelectObject(hdc_mem, hbmp_old);
        let _ = DeleteObject(hbmp);
        let _ = DeleteDC(hdc_mem);
        ReleaseDC(HWND(0), hdc_screen);

        // GDI gives us BGRA; that is already the native pixel order for our
        // pipeline (we just leave it as BGRA — the H264 encoder gets YUV
        // derived from it, not raw pixels).
        Some((w, h, buf))
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Capture the primary display.  Returns `(width, height, bgra_pixels)`.
#[allow(unused)]
pub fn capture_screen() -> Option<(u32, u32, Vec<u8>)> {
    #[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
    return win::capture();

    #[cfg(target_os = "linux")]
    return linux_x11::capture();

    #[cfg(not(any(
        all(target_os = "windows", feature = "live-vp9-mf"),
        target_os = "linux"
    )))]
    None
}

/// Return the primary display size without a full capture.
#[allow(unused)]
pub fn screen_size() -> Option<(u32, u32)> {
    #[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
    {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        if w > 0 && h > 0 {
            return Some((w as u32, h as u32));
        }
        return None;
    }

    #[cfg(target_os = "linux")]
    return linux_x11::screen_size();

    #[cfg(not(any(
        all(target_os = "windows", feature = "live-vp9-mf"),
        target_os = "linux"
    )))]
    None
}

#[cfg(target_os = "linux")]
mod linux_x11 {
    use x11rb::{connection::Connection, protocol::xproto::ConnectionExt};

    pub fn capture() -> Option<(u32, u32, Vec<u8>)> {
        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let screen = &conn.setup().roots[screen_num];
        let width = screen.width_in_pixels;
        let height = screen.height_in_pixels;
        if width == 0 || height == 0 {
            return None;
        }

        let reply = conn
            .get_image(
                x11rb::protocol::xproto::ImageFormat::Z_PIXMAP,
                screen.root,
                0,
                0,
                width,
                height,
                u32::MAX,
            )
            .ok()?
            .reply()
            .ok()?;

        let pixels = width as usize * height as usize;
        if reply.data.len() < pixels * 4 {
            return None;
        }

        let mut bgra = Vec::with_capacity(pixels * 4);
        for px in reply.data.chunks_exact(4).take(pixels) {
            bgra.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
        Some((width as u32, height as u32, bgra))
    }

    pub fn screen_size() -> Option<(u32, u32)> {
        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let screen = &conn.setup().roots[screen_num];
        let width = screen.width_in_pixels;
        let height = screen.height_in_pixels;
        if width == 0 || height == 0 {
            None
        } else {
            Some((width as u32, height as u32))
        }
    }
}
