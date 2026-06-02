//! Cross-platform screen capture.
//!
//! Windows: uses GDI BitBlt (no external deps, already have the `windows` crate).
//! Other OS: returns `None` — to be implemented in a future session.
//!
//! Returns `(width, height, bgra_pixels)`.

#[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
mod win {
    use std::{cell::RefCell, mem::size_of};

    use windows::Win32::{
        Foundation::HWND,
        Graphics::Gdi::{
            BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
            GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
            DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, RGBQUAD, SRCCOPY,
        },
        UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN},
    };

    thread_local! {
        static GDI_CAPTURE: RefCell<Option<GdiCapture>> = const { RefCell::new(None) };
    }

    pub fn capture_into(out: &mut Vec<u8>) -> Option<(u32, u32)> {
        unsafe { capture_into_inner(out) }
    }

    unsafe fn capture_into_inner(out: &mut Vec<u8>) -> Option<(u32, u32)> {
        let width = GetSystemMetrics(SM_CXSCREEN);
        let height = GetSystemMetrics(SM_CYSCREEN);
        if width <= 0 || height <= 0 {
            return None;
        }

        GDI_CAPTURE.with(|cell| {
            let recreate = cell
                .borrow()
                .as_ref()
                .map(|capture| !capture.matches(width, height))
                .unwrap_or(true);
            if recreate {
                *cell.borrow_mut() = GdiCapture::new(width, height);
            }

            let result = cell
                .borrow_mut()
                .as_mut()
                .and_then(|capture| capture.capture_into(out));
            if result.is_none() {
                *cell.borrow_mut() = None;
            }
            result
        })
    }

    struct GdiCapture {
        width: i32,
        height: i32,
        hdc_screen: HDC,
        hdc_mem: HDC,
        hbmp: HBITMAP,
        old_bitmap: HGDIOBJ,
        bitmap_info: BITMAPINFO,
    }

    impl GdiCapture {
        unsafe fn new(width: i32, height: i32) -> Option<Self> {
            let hdc_screen = GetDC(HWND(0));
            if hdc_screen.is_invalid() {
                return None;
            }

            let hdc_mem = CreateCompatibleDC(hdc_screen);
            if hdc_mem.is_invalid() {
                ReleaseDC(HWND(0), hdc_screen);
                return None;
            }

            let hbmp = CreateCompatibleBitmap(hdc_screen, width, height);
            if hbmp.is_invalid() {
                let _ = DeleteDC(hdc_mem);
                ReleaseDC(HWND(0), hdc_screen);
                return None;
            }

            let old_bitmap = SelectObject(hdc_mem, hbmp);
            let bitmap_info = BITMAPINFO {
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

            Some(Self {
                width,
                height,
                hdc_screen,
                hdc_mem,
                hbmp,
                old_bitmap,
                bitmap_info,
            })
        }

        fn matches(&self, width: i32, height: i32) -> bool {
            self.width == width && self.height == height
        }

        unsafe fn capture_into(&mut self, out: &mut Vec<u8>) -> Option<(u32, u32)> {
            let w = self.width as u32;
            let h = self.height as u32;
            let byte_len = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
            if out.len() != byte_len {
                out.resize(byte_len, 0);
            }

            if !BitBlt(
                self.hdc_mem,
                0,
                0,
                self.width,
                self.height,
                self.hdc_screen,
                0,
                0,
                SRCCOPY | CAPTUREBLT,
            )
            .as_bool()
            {
                return None;
            }

            let lines = GetDIBits(
                self.hdc_mem,
                self.hbmp,
                0,
                h,
                Some(out.as_mut_ptr() as *mut _),
                &mut self.bitmap_info,
                DIB_RGB_COLORS,
            );
            if lines == 0 {
                return None;
            }

            // GDI gives us BGRA; that is already the native pixel order for our
            // pipeline.
            Some((w, h))
        }
    }

    impl Drop for GdiCapture {
        fn drop(&mut self) {
            unsafe {
                SelectObject(self.hdc_mem, self.old_bitmap);
                let _ = DeleteObject(self.hbmp);
                let _ = DeleteDC(self.hdc_mem);
                ReleaseDC(HWND(0), self.hdc_screen);
            }
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Capture the primary display.  Returns `(width, height, bgra_pixels)`.
#[allow(unused)]
pub fn capture_screen() -> Option<(u32, u32, Vec<u8>)> {
    let mut pixels = Vec::new();
    let (width, height) = capture_screen_into(&mut pixels)?;
    Some((width, height, pixels))
}

/// Capture the primary display into a caller-owned BGRA buffer.
#[allow(unused)]
pub fn capture_screen_into(pixels: &mut Vec<u8>) -> Option<(u32, u32)> {
    #[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
    return win::capture_into(pixels);

    #[cfg(target_os = "linux")]
    {
        let (width, height, data) = linux_x11::capture()?;
        *pixels = data;
        return Some((width, height));
    }

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
    use std::{
        cell::RefCell,
        process::{Command, Stdio},
        sync::OnceLock,
    };

    use x11rb::{
        connection::Connection,
        protocol::xproto::{ConnectionExt, ImageFormat},
        rust_connection::RustConnection,
    };

    thread_local! {
        static X11_CAPTURE: RefCell<Option<X11Capture>> = const { RefCell::new(None) };
    }

    struct X11Capture {
        conn: RustConnection,
        root: u32,
        width: u16,
        height: u16,
    }

    impl X11Capture {
        fn connect() -> Option<Self> {
            let (conn, screen_num) = x11rb::connect(None).ok()?;
            let screen = &conn.setup().roots[screen_num];
            if screen.width_in_pixels == 0 || screen.height_in_pixels == 0 {
                return None;
            }
            Some(Self {
                root: screen.root,
                width: screen.width_in_pixels,
                height: screen.height_in_pixels,
                conn,
            })
        }

        fn capture(&self) -> Option<(u32, u32, Vec<u8>)> {
            let reply = self
                .conn
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    self.root,
                    0,
                    0,
                    self.width,
                    self.height,
                    u32::MAX,
                )
                .ok()?
                .reply()
                .ok()?;

            bgra_from_ximage(self.width as u32, self.height as u32, reply.data)
        }
    }

    pub fn capture() -> Option<(u32, u32, Vec<u8>)> {
        if let Some(frame) = capture_x11_cached() {
            return Some(frame);
        }
        capture_grim_ppm()
    }

    pub fn screen_size() -> Option<(u32, u32)> {
        if let Some(size) = X11_CAPTURE.with(|cell| {
            if cell.borrow().is_none() {
                *cell.borrow_mut() = X11Capture::connect();
            }
            cell.borrow()
                .as_ref()
                .map(|x11| (x11.width as u32, x11.height as u32))
        }) {
            return Some(size);
        }
        capture_grim_ppm().map(|(w, h, _)| (w, h))
    }

    fn capture_x11_cached() -> Option<(u32, u32, Vec<u8>)> {
        X11_CAPTURE.with(|cell| {
            if cell.borrow().is_none() {
                *cell.borrow_mut() = X11Capture::connect();
            }
            let frame = cell.borrow().as_ref().and_then(X11Capture::capture);
            if frame.is_none() {
                *cell.borrow_mut() = None;
            }
            frame
        })
    }

    fn bgra_from_ximage(width: u32, height: u32, mut data: Vec<u8>) -> Option<(u32, u32, Vec<u8>)> {
        let pixels = width as usize * height as usize;
        if data.len() >= pixels * 4 {
            data.truncate(pixels * 4);
            for alpha in data[3..].iter_mut().step_by(4) {
                *alpha = 255;
            }
            return Some((width, height, data));
        }
        if data.len() >= pixels * 3 {
            let mut bgra = Vec::with_capacity(pixels * 4);
            for px in data.chunks_exact(3).take(pixels) {
                bgra.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            return Some((width, height, bgra));
        }
        None
    }

    fn capture_grim_ppm() -> Option<(u32, u32, Vec<u8>)> {
        static GRIM_AVAILABLE: OnceLock<bool> = OnceLock::new();
        if !*GRIM_AVAILABLE.get_or_init(|| command_exists("grim")) {
            return None;
        }
        let output = Command::new("grim")
            .args(["-t", "ppm", "-"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        parse_ppm_rgb(&output.stdout)
    }

    fn parse_ppm_rgb(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
        let mut index = 0;
        let magic = next_ppm_token(bytes, &mut index)?;
        if magic != b"P6" {
            return None;
        }
        let width = parse_ascii_u32(next_ppm_token(bytes, &mut index)?)?;
        let height = parse_ascii_u32(next_ppm_token(bytes, &mut index)?)?;
        let max = parse_ascii_u32(next_ppm_token(bytes, &mut index)?)?;
        if width == 0 || height == 0 || max != 255 {
            return None;
        }
        if bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        let pixels = width as usize * height as usize;
        let rgb_len = pixels.checked_mul(3)?;
        let rgb = bytes.get(index..index + rgb_len)?;
        let mut bgra = Vec::with_capacity(pixels * 4);
        for px in rgb.chunks_exact(3) {
            bgra.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        Some((width, height, bgra))
    }

    fn next_ppm_token<'a>(bytes: &'a [u8], index: &mut usize) -> Option<&'a [u8]> {
        loop {
            while bytes.get(*index).is_some_and(u8::is_ascii_whitespace) {
                *index += 1;
            }
            if bytes.get(*index) != Some(&b'#') {
                break;
            }
            while bytes.get(*index).is_some_and(|byte| *byte != b'\n') {
                *index += 1;
            }
        }
        let start = *index;
        while bytes
            .get(*index)
            .is_some_and(|byte| !byte.is_ascii_whitespace())
        {
            *index += 1;
        }
        (start < *index).then_some(&bytes[start..*index])
    }

    fn parse_ascii_u32(bytes: &[u8]) -> Option<u32> {
        std::str::from_utf8(bytes).ok()?.parse().ok()
    }

    fn command_exists(name: &str) -> bool {
        let path = match std::env::var_os("PATH") {
            Some(path) => path,
            None => return false,
        };
        for dir in std::env::split_paths(&path) {
            if dir.join(name).is_file() {
                return true;
            }
        }
        false
    }
}
