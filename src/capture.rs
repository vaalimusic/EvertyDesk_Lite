//! Cross-platform screen capture.
//!
//! Windows: tries Desktop Duplication first, then falls back to GDI BitBlt.
//! macOS: uses CoreGraphics display capture.
//! Other OS: returns `None` — to be implemented in a future session.
//!
//! Returns `(width, height, bgra_pixels)`.

#[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
mod win {
    use std::{cell::RefCell, mem::size_of};

    use windows::core::{ComInterface, Error as WinError, HRESULT};
    use windows::Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::Gdi::{
            BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreatedHDC, DeleteDC, DeleteObject,
            GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
            CAPTUREBLT, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, RGBQUAD, SRCCOPY,
        },
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0},
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource,
                ID3D11Texture2D, D3D11_BIND_FLAG, D3D11_CPU_ACCESS_READ,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
                D3D11_RESOURCE_MISC_FLAG, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_STAGING,
            },
            Dxgi::{
                Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
                IDXGIAdapter, IDXGIDevice, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
                DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
            },
        },
        UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN},
    };

    thread_local! {
        static DXGI_CAPTURE: RefCell<Option<DxgiCapture>> = const { RefCell::new(None) };
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

        if let Some(size) = capture_dxgi_into(out, width as u32, height as u32) {
            return Some(size);
        }

        capture_gdi_into(out, width, height)
    }

    unsafe fn capture_dxgi_into(
        out: &mut Vec<u8>,
        screen_w: u32,
        screen_h: u32,
    ) -> Option<(u32, u32)> {
        DXGI_CAPTURE.with(|cell| {
            if cell.borrow().is_none() {
                *cell.borrow_mut() = DxgiCapture::new(screen_w, screen_h).ok();
            }

            let result = match cell.borrow_mut().as_mut() {
                Some(capture) => capture.capture_into(out),
                None => return None,
            };

            match result {
                DxgiCaptureResult::Frame { width, height } => Some((width, height)),
                DxgiCaptureResult::NoChange { width, height } => {
                    if out.len() == frame_byte_len(width, height)? {
                        Some((width, height))
                    } else {
                        None
                    }
                }
                DxgiCaptureResult::Unavailable => {
                    *cell.borrow_mut() = None;
                    None
                }
            }
        })
    }

    unsafe fn capture_gdi_into(out: &mut Vec<u8>, width: i32, height: i32) -> Option<(u32, u32)> {
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

    enum DxgiCaptureResult {
        Frame { width: u32, height: u32 },
        NoChange { width: u32, height: u32 },
        Unavailable,
    }

    struct DxgiCapture {
        width: u32,
        height: u32,
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        duplication: IDXGIOutputDuplication,
        staging: Option<ID3D11Texture2D>,
    }

    impl DxgiCapture {
        unsafe fn new(screen_w: u32, screen_h: u32) -> Result<Self, WinError> {
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                Option::<&IDXGIAdapter>::None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE(0),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device: ID3D11Device = device.ok_or_else(|| WinError::from(E_DXGI_CAPTURE_INIT))?;
            let context = context.ok_or_else(|| WinError::from(E_DXGI_CAPTURE_INIT))?;
            let dxgi_device: IDXGIDevice = device.cast()?;
            let adapter = dxgi_device.GetAdapter()?;
            let output = adapter.EnumOutputs(0)?;
            let output1: IDXGIOutput1 = output.cast()?;
            let duplication = output1.DuplicateOutput(&device)?;

            let mut desc = Default::default();
            duplication.GetDesc(&mut desc);
            let width = desc.ModeDesc.Width;
            let height = desc.ModeDesc.Height;
            if width == 0 || height == 0 || width != screen_w || height != screen_h {
                return Err(WinError::from(E_DXGI_CAPTURE_INIT));
            }

            Ok(Self {
                width,
                height,
                device,
                context,
                duplication,
                staging: None,
            })
        }

        unsafe fn capture_into(&mut self, out: &mut Vec<u8>) -> DxgiCaptureResult {
            match self.capture_frame(out) {
                Ok(true) => DxgiCaptureResult::Frame {
                    width: self.width,
                    height: self.height,
                },
                Ok(false) => DxgiCaptureResult::NoChange {
                    width: self.width,
                    height: self.height,
                },
                Err(err) if err.code() == DXGI_ERROR_WAIT_TIMEOUT => DxgiCaptureResult::NoChange {
                    width: self.width,
                    height: self.height,
                },
                Err(err) if err.code() == DXGI_ERROR_ACCESS_LOST => DxgiCaptureResult::Unavailable,
                Err(_) => DxgiCaptureResult::Unavailable,
            }
        }

        unsafe fn capture_frame(&mut self, out: &mut Vec<u8>) -> Result<bool, WinError> {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            self.duplication
                .AcquireNextFrame(0, &mut info, &mut resource)?;
            let _release = DuplicationFrameGuard(self.duplication.clone());

            let Some(resource) = resource else {
                return Ok(false);
            };
            if info.AccumulatedFrames == 0 && info.LastMouseUpdateTime == 0 {
                return Ok(false);
            }

            let texture: ID3D11Texture2D = resource.cast()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if desc.Width == 0
                || desc.Height == 0
                || desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM
                || desc.Width != self.width
                || desc.Height != self.height
            {
                return Err(WinError::from(E_DXGI_CAPTURE_INIT));
            }

            let staging = self.ensure_staging(desc)?;
            let src: ID3D11Resource = texture.cast()?;
            let dst: ID3D11Resource = staging.cast()?;
            self.context.CopyResource(&dst, &src);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&dst, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let _map = MappedResourceGuard {
                context: &self.context,
                resource: &dst,
            };

            copy_mapped_bgra(desc.Width, desc.Height, mapped, out)
                .ok_or_else(|| WinError::from(E_DXGI_CAPTURE_INIT))?;
            Ok(true)
        }

        unsafe fn ensure_staging(
            &mut self,
            src_desc: D3D11_TEXTURE2D_DESC,
        ) -> Result<ID3D11Texture2D, WinError> {
            if let Some(staging) = &self.staging {
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                staging.GetDesc(&mut desc);
                if desc.Width == src_desc.Width && desc.Height == src_desc.Height {
                    return Ok(staging.clone());
                }
            }

            let desc = D3D11_TEXTURE2D_DESC {
                Width: src_desc.Width,
                Height: src_desc.Height,
                MipLevels: 1,
                ArraySize: 1,
                Format: src_desc.Format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: D3D11_BIND_FLAG(0),
                CPUAccessFlags: D3D11_CPU_ACCESS_READ,
                MiscFlags: D3D11_RESOURCE_MISC_FLAG(0),
            };
            let mut staging = None;
            self.device
                .CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.ok_or_else(|| WinError::from(E_DXGI_CAPTURE_INIT))?;
            self.staging = Some(staging.clone());
            Ok(staging)
        }
    }

    struct DuplicationFrameGuard(IDXGIOutputDuplication);

    impl Drop for DuplicationFrameGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = self.0.ReleaseFrame();
            }
        }
    }

    struct MappedResourceGuard<'a> {
        context: &'a ID3D11DeviceContext,
        resource: &'a ID3D11Resource,
    }

    impl Drop for MappedResourceGuard<'_> {
        fn drop(&mut self) {
            unsafe {
                self.context.Unmap(self.resource, 0);
            }
        }
    }

    struct GdiCapture {
        width: i32,
        height: i32,
        hdc_screen: HDC,
        hdc_mem: CreatedHDC,
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

    const E_DXGI_CAPTURE_INIT: HRESULT = HRESULT(-2147024809);

    fn frame_byte_len(width: u32, height: u32) -> Option<usize> {
        (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)
    }

    unsafe fn copy_mapped_bgra(
        width: u32,
        height: u32,
        mapped: D3D11_MAPPED_SUBRESOURCE,
        out: &mut Vec<u8>,
    ) -> Option<()> {
        let row_bytes = (width as usize).checked_mul(4)?;
        let byte_len = row_bytes.checked_mul(height as usize)?;
        let pitch = usize::try_from(mapped.RowPitch).ok()?;
        if mapped.pData.is_null() || pitch < row_bytes {
            return None;
        }
        if out.len() != byte_len {
            out.resize(byte_len, 0);
        }

        for y in 0..height as usize {
            let src = (mapped.pData as *const u8).add(y.checked_mul(pitch)?);
            let dst_start = y.checked_mul(row_bytes)?;
            let dst_end = dst_start.checked_add(row_bytes)?;
            out[dst_start..dst_end].copy_from_slice(std::slice::from_raw_parts(src, row_bytes));
        }
        Some(())
    }
}

#[cfg(target_os = "macos")]
mod macos_coregraphics {
    use std::ffi::c_void;

    type CGDirectDisplayID = u32;
    type CGImageRef = *mut c_void;
    type CGContextRef = *mut c_void;
    type CGColorSpaceRef = *mut c_void;
    type CGFloat = f64;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: CGFloat,
        y: CGFloat,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: CGFloat,
        height: CGFloat,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    const K_CG_IMAGE_ALPHA_PREMULTIPLIED_FIRST: u32 = 2;
    const K_CG_BITMAP_BYTE_ORDER_32_LITTLE: u32 = 2 << 12;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> CGDirectDisplayID;
        fn CGDisplayPixelsWide(display: CGDirectDisplayID) -> usize;
        fn CGDisplayPixelsHigh(display: CGDirectDisplayID) -> usize;
        fn CGDisplayCreateImage(display: CGDirectDisplayID) -> CGImageRef;
        fn CGImageGetWidth(image: CGImageRef) -> usize;
        fn CGImageGetHeight(image: CGImageRef) -> usize;
        fn CGImageRelease(image: CGImageRef);

        fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
        fn CGColorSpaceRelease(space: CGColorSpaceRef);

        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: CGColorSpaceRef,
            bitmap_info: u32,
        ) -> CGContextRef;
        fn CGContextDrawImage(context: CGContextRef, rect: CGRect, image: CGImageRef);
        fn CGContextTranslateCTM(context: CGContextRef, tx: CGFloat, ty: CGFloat);
        fn CGContextScaleCTM(context: CGContextRef, sx: CGFloat, sy: CGFloat);
        fn CGContextRelease(context: CGContextRef);
    }

    pub fn capture_into(out: &mut Vec<u8>) -> Option<(u32, u32)> {
        unsafe {
            let image = CGDisplayCreateImage(CGMainDisplayID());
            if image.is_null() {
                return None;
            }

            let width = CGImageGetWidth(image);
            let height = CGImageGetHeight(image);
            let byte_len = width.checked_mul(height)?.checked_mul(4)?;
            if width == 0 || height == 0 {
                CGImageRelease(image);
                return None;
            }
            if out.len() != byte_len {
                out.resize(byte_len, 0);
            }

            let color_space = CGColorSpaceCreateDeviceRGB();
            if color_space.is_null() {
                CGImageRelease(image);
                return None;
            }

            let bitmap_info =
                K_CG_IMAGE_ALPHA_PREMULTIPLIED_FIRST | K_CG_BITMAP_BYTE_ORDER_32_LITTLE;
            let context = CGBitmapContextCreate(
                out.as_mut_ptr() as *mut c_void,
                width,
                height,
                8,
                width * 4,
                color_space,
                bitmap_info,
            );
            CGColorSpaceRelease(color_space);
            if context.is_null() {
                CGImageRelease(image);
                return None;
            }

            CGContextTranslateCTM(context, 0.0, height as CGFloat);
            CGContextScaleCTM(context, 1.0, -1.0);
            CGContextDrawImage(
                context,
                CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: width as CGFloat,
                        height: height as CGFloat,
                    },
                },
                image,
            );
            CGContextRelease(context);
            CGImageRelease(image);
            Some((width as u32, height as u32))
        }
    }

    pub fn screen_size() -> Option<(u32, u32)> {
        unsafe {
            let display = CGMainDisplayID();
            let width = CGDisplayPixelsWide(display);
            let height = CGDisplayPixelsHigh(display);
            if width == 0 || height == 0 {
                None
            } else {
                Some((width as u32, height as u32))
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
        // Fast path: MIT-SHM shared-memory capture (no X11 socket copy).
        if let Some(size) = linux_x11_shm::capture_into(pixels) {
            return Some(size);
        }
        // Fallback: standard x11rb GetImage (socket copy) + grim on Wayland.
        let (width, height, data) = linux_x11::capture()?;
        *pixels = data;
        return Some((width, height));
    }

    #[cfg(target_os = "macos")]
    return macos_coregraphics::capture_into(pixels);

    #[cfg(not(any(
        all(target_os = "windows", feature = "live-vp9-mf"),
        target_os = "macos",
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

    #[cfg(target_os = "macos")]
    return macos_coregraphics::screen_size();

    #[cfg(not(any(
        all(target_os = "windows", feature = "live-vp9-mf"),
        target_os = "macos",
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

// ── Linux MIT-SHM fast capture ────────────────────────────────────────────────
//
// XShmGetImage copies the desktop into a shared-memory segment that the X
// server maps directly, skipping the X11 socket entirely.  For 1080p this is
// ~5–10× faster than GetImage over the socket.
//
// Falls back gracefully if:
//  - The DISPLAY variable is unset (Wayland-only session).
//  - The server does not support MIT-SHM (e.g. Xvnc, Xnest, some containers).
//  - shmget / shmat fail (seccomp sandbox, container kernel limits).
#[cfg(target_os = "linux")]
mod linux_x11_shm {
    use std::cell::RefCell;

    use x11rb::{
        connection::Connection,
        protocol::{
            shm::{self, ConnectionExt as ShmExt},
            xproto,
        },
        rust_connection::RustConnection,
    };

    struct ShmCapture {
        conn: RustConnection,
        root: xproto::Window,
        width: u16,
        height: u16,
        seg: shm::Seg,
        shm_id: libc::c_int,
        shm_ptr: *mut libc::c_void,
        byte_len: usize,
    }

    impl ShmCapture {
        fn connect() -> Option<Self> {
            // Only attempt on X11 sessions.
            if std::env::var_os("DISPLAY").is_none() {
                return None;
            }

            let (conn, screen_num) = x11rb::connect(None).ok()?;
            let screen = &conn.setup().roots[screen_num];
            let width = screen.width_in_pixels;
            let height = screen.height_in_pixels;
            let root = screen.root;
            if width == 0 || height == 0 {
                return None;
            }

            // Check that the server has the MIT-SHM extension.
            let shm_info = conn.shm_query_version().ok()?.reply().ok()?;
            if !shm_info.shared_pixmaps {
                // Extension present but pixmaps disabled — GetImage still works.
                // We continue; shm::get_image doesn't require shared_pixmaps.
            }

            // Allocate shared-memory segment: width × height × 4 bytes (BGRA).
            let byte_len = (width as usize)
                .checked_mul(height as usize)?
                .checked_mul(4)?;
            let shm_id = unsafe {
                libc::shmget(libc::IPC_PRIVATE, byte_len, libc::IPC_CREAT | 0o600)
            };
            if shm_id < 0 {
                return None;
            }
            let shm_ptr =
                unsafe { libc::shmat(shm_id, std::ptr::null(), 0) };
            if shm_ptr as isize == -1 {
                unsafe { libc::shmctl(shm_id, libc::IPC_RMID, std::ptr::null_mut()) };
                return None;
            }
            // Mark segment for deletion as soon as the last process detaches.
            unsafe { libc::shmctl(shm_id, libc::IPC_RMID, std::ptr::null_mut()) };

            // Register with the X server.
            let seg = conn.generate_id().ok()?;
            conn.shm_attach(seg, shm_id as u32, false).ok()?.check().ok()?;

            Some(Self {
                conn,
                root,
                width,
                height,
                seg,
                shm_id,
                shm_ptr,
                byte_len,
            })
        }

        /// Capture into `out`, resizing it if needed.  Returns `(w, h)`.
        fn capture_into(&self, out: &mut Vec<u8>) -> Option<(u32, u32)> {
            let w = self.width as u32;
            let h = self.height as u32;

            // Ask the server to fill our shared-memory segment.
            self.conn
                .shm_get_image(
                    self.root,
                    0, 0,
                    self.width,
                    self.height,
                    !0u32,                          // plane_mask = all planes
                    2u8, // XCB_IMAGE_FORMAT_Z_PIXMAP
                    self.seg,
                    0,                              // offset in segment
                )
                .ok()?
                .reply()
                .ok()?;

            // The segment now holds raw pixels in the server's native format
            // (usually BGRX on little-endian X11).  Copy and fix alpha.
            if out.len() != self.byte_len {
                out.resize(self.byte_len, 0);
            }
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.shm_ptr as *const u8,
                    out.as_mut_ptr(),
                    self.byte_len,
                );
            }

            // X11 returns pixels as BGRX (alpha = 0).  Force alpha = 255.
            for alpha in out[3..].iter_mut().step_by(4) {
                *alpha = 255;
            }

            Some((w, h))
        }

        /// True when the screen dimensions still match what we were created for.
        fn matches(&self, w: u16, h: u16) -> bool {
            self.width == w && self.height == h
        }
    }

    impl Drop for ShmCapture {
        fn drop(&mut self) {
            let _ = self.conn.shm_detach(self.seg);
            if self.shm_ptr as isize != -1 {
                unsafe { libc::shmdt(self.shm_ptr) };
            }
        }
    }

    thread_local! {
        static SHM: RefCell<Option<ShmCapture>> = const { RefCell::new(None) };
    }

    /// Try to capture via MIT-SHM.  Returns `None` if the extension is not
    /// available or if any system call fails.
    pub fn capture_into(out: &mut Vec<u8>) -> Option<(u32, u32)> {
        SHM.with(|cell| {
            // Lazily initialise on first call.
            if cell.borrow().is_none() {
                *cell.borrow_mut() = ShmCapture::connect();
            }

            // Detect resolution change and recreate.
            let needs_recreate = cell.borrow().as_ref().map_or(false, |cap| {
                let (cur_w, cur_h) = current_screen_size().unwrap_or((0, 0));
                !cap.matches(cur_w as u16, cur_h as u16)
            });
            if needs_recreate {
                *cell.borrow_mut() = ShmCapture::connect();
            }

            let result = cell
                .borrow()
                .as_ref()
                .and_then(|cap| cap.capture_into(out));

            if result.is_none() {
                // Invalidate on any failure so the next call retries.
                *cell.borrow_mut() = None;
            }
            result
        })
    }

    fn current_screen_size() -> Option<(u32, u32)> {
        SHM.with(|cell| {
            cell.borrow().as_ref().map(|cap| {
                (cap.width as u32, cap.height as u32)
            })
        })
    }
}
