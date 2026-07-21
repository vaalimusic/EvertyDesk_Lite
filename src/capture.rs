//! Cross-platform screen capture.
//!
//! Windows: tries Desktop Duplication first, then falls back to GDI BitBlt.
//! macOS: uses CoreGraphics display capture.
//! Other OS: returns `None` — to be implemented in a future session.
//!
//! Returns `(width, height, bgra_pixels)`.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureDisplay {
    pub index: i32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub name: String,
}

#[cfg(any(target_os = "linux", test))]
fn sort_and_reindex_displays(mut displays: Vec<CaptureDisplay>) -> Vec<CaptureDisplay> {
    displays.sort_by_key(|display| (display.x, display.y, display.index));
    for (index, display) in displays.iter_mut().enumerate() {
        display.index = index as i32;
    }
    displays
}

#[cfg(any(target_os = "linux", test))]
fn select_capture_display(displays: &[CaptureDisplay], display: i32) -> Option<CaptureDisplay> {
    let target = display.max(0);
    displays
        .iter()
        .find(|info| info.index == target)
        .cloned()
        .or_else(|| displays.first().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(index: i32, x: i32, y: i32, width: i32, height: i32, name: &str) -> CaptureDisplay {
        CaptureDisplay {
            index,
            x,
            y,
            width,
            height,
            name: name.to_owned(),
        }
    }

    #[test]
    fn linux_style_display_order_is_stable_and_reindexed() {
        let displays = sort_and_reindex_displays(vec![
            display(7, 1920, 0, 1280, 720, "HDMI-1"),
            display(2, 0, 0, 1920, 1080, "DP-1"),
            display(9, 0, 1080, 1024, 768, "VGA-1"),
        ]);

        assert_eq!(displays[0].name, "DP-1");
        assert_eq!(displays[0].index, 0);
        assert_eq!(displays[1].name, "VGA-1");
        assert_eq!(displays[1].index, 1);
        assert_eq!(displays[2].name, "HDMI-1");
        assert_eq!(displays[2].index, 2);
    }

    #[test]
    fn display_selection_falls_back_to_first_display() {
        let displays = vec![
            display(0, 0, 0, 1920, 1080, "DP-1"),
            display(1, 1920, 0, 1280, 720, "HDMI-1"),
        ];

        assert_eq!(select_capture_display(&displays, 1).unwrap().name, "HDMI-1");
        assert_eq!(select_capture_display(&displays, 9).unwrap().name, "DP-1");
        assert_eq!(select_capture_display(&displays, -1).unwrap().name, "DP-1");
    }
}

#[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
mod win {
    use std::{cell::RefCell, mem::size_of};

    use windows::core::{ComInterface, Error as WinError, HRESULT, PCWSTR};
    use windows::Win32::{
        Foundation::{HMODULE, HWND, RECT},
        Graphics::Gdi::{
            BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreatedHDC, DeleteDC, DeleteObject,
            EnumDisplayDevicesW, GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO,
            BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, DISPLAY_DEVICEW,
            DISPLAY_DEVICE_ACTIVE, DISPLAY_DEVICE_MIRRORING_DRIVER, HBITMAP, HDC, HGDIOBJ, RGBQUAD,
            SRCCOPY,
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

    use super::CaptureDisplay;

    thread_local! {
        static DXGI_CAPTURE: RefCell<Option<DxgiCapture>> = const { RefCell::new(None) };
        static GDI_CAPTURE: RefCell<Option<GdiCapture>> = const { RefCell::new(None) };
        static DISPLAY_INFO_CACHE: RefCell<Vec<CaptureDisplay>> = const { RefCell::new(Vec::new()) };
    }

    pub fn capture_into(out: &mut Vec<u8>) -> Option<(u32, u32)> {
        capture_display_into(0, out)
    }

    pub fn capture_display_into(display: i32, out: &mut Vec<u8>) -> Option<(u32, u32)> {
        unsafe { capture_into_inner(display, out) }
    }

    /// Leak the DXGI D3D11 device held by this thread's capture state.
    /// Called from the capture thread just before it exits so that the D3D11
    /// device destructor never runs concurrently with WGPU/WGL — both paths
    /// acquire the NVIDIA KMD lock and deadlock if they overlap.
    pub fn leak_capture_resources() {
        DXGI_CAPTURE.with(|cell| {
            if let Some(cap) = cell.borrow_mut().take() {
                std::mem::forget(cap);
            }
        });
    }

    unsafe fn capture_into_inner(display: i32, out: &mut Vec<u8>) -> Option<(u32, u32)> {
        let info = cached_display_info(display)?;
        if info.width <= 0 || info.height <= 0 {
            invalidate_display_info_cache();
            return None;
        }

        if let Some(size) =
            capture_dxgi_into(out, info.index, info.width as u32, info.height as u32)
        {
            return Some(size);
        }

        let result = capture_gdi_into(out, &info);
        if result.is_none() {
            invalidate_display_info_cache();
        }
        result
    }

    fn cached_display_info(display: i32) -> Option<CaptureDisplay> {
        let target = display.max(0);
        DISPLAY_INFO_CACHE.with(|cell| {
            {
                let cached = cell.borrow();
                if let Some(info) = cached.iter().find(|info| info.index == target).cloned() {
                    return Some(info);
                }
                if target == 0 {
                    if let Some(info) = cached.first().cloned() {
                        return Some(info);
                    }
                }
            }

            let infos = display_infos();
            let selected = infos
                .iter()
                .find(|info| info.index == target)
                .cloned()
                .or_else(|| infos.first().cloned());
            *cell.borrow_mut() = infos;
            selected
        })
    }

    fn invalidate_display_info_cache() {
        DISPLAY_INFO_CACHE.with(|cell| cell.borrow_mut().clear());
    }

    unsafe fn capture_dxgi_into(
        out: &mut Vec<u8>,
        display: i32,
        width: u32,
        height: u32,
    ) -> Option<(u32, u32)> {
        DXGI_CAPTURE.with(|cell| {
            let recreate = cell
                .borrow()
                .as_ref()
                .map(|capture| !capture.matches(display, width, height))
                .unwrap_or(true);
            if recreate {
                *cell.borrow_mut() = DxgiCapture::new(display, width, height).ok();
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

    unsafe fn capture_gdi_into(out: &mut Vec<u8>, info: &CaptureDisplay) -> Option<(u32, u32)> {
        GDI_CAPTURE.with(|cell| {
            let recreate = cell
                .borrow()
                .as_ref()
                .map(|capture| !capture.matches(info.x, info.y, info.width, info.height))
                .unwrap_or(true);
            if recreate {
                *cell.borrow_mut() = GdiCapture::new(info.x, info.y, info.width, info.height);
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
        display: i32,
        width: u32,
        height: u32,
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        duplication: IDXGIOutputDuplication,
        staging: Option<ID3D11Texture2D>,
    }

    impl DxgiCapture {
        unsafe fn new(display: i32, screen_w: u32, screen_h: u32) -> Result<Self, WinError> {
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
            let output = adapter.EnumOutputs(display.max(0) as u32)?;
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
                display,
                width,
                height,
                device,
                context,
                duplication,
                staging: None,
            })
        }

        fn matches(&self, display: i32, width: u32, height: u32) -> bool {
            self.display == display && self.width == width && self.height == height
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
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        hdc_screen: HDC,
        hdc_mem: CreatedHDC,
        hbmp: HBITMAP,
        old_bitmap: HGDIOBJ,
        bitmap_info: BITMAPINFO,
    }

    impl GdiCapture {
        unsafe fn new(x: i32, y: i32, width: i32, height: i32) -> Option<Self> {
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
                x,
                y,
                width,
                height,
                hdc_screen,
                hdc_mem,
                hbmp,
                old_bitmap,
                bitmap_info,
            })
        }

        fn matches(&self, x: i32, y: i32, width: i32, height: i32) -> bool {
            self.x == x && self.y == y && self.width == width && self.height == height
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
                self.x,
                self.y,
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

    pub fn display_infos() -> Vec<CaptureDisplay> {
        unsafe {
            dxgi_display_infos()
                .map(reorder_dxgi_by_gdi)
                .unwrap_or_else(fallback_display_infos)
        }
    }

    unsafe fn dxgi_display_infos() -> Option<Vec<CaptureDisplay>> {
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
        )
        .ok()?;
        let device: ID3D11Device = device?;
        let dxgi_device: IDXGIDevice = device.cast().ok()?;
        let adapter = dxgi_device.GetAdapter().ok()?;

        let mut displays = Vec::new();
        for index in 0..16_u32 {
            let Ok(output) = adapter.EnumOutputs(index) else {
                break;
            };
            let mut desc = Default::default();
            output.GetDesc(&mut desc).ok()?;
            let RECT {
                left,
                top,
                right,
                bottom,
            } = desc.DesktopCoordinates;
            let width = right - left;
            let height = bottom - top;
            if width <= 0 || height <= 0 {
                continue;
            }
            let name = utf16_z_to_string(&desc.DeviceName)
                .unwrap_or_else(|| format!("Display {}", index + 1));
            displays.push(CaptureDisplay {
                index: index as i32,
                x: left,
                y: top,
                width,
                height,
                name,
            });
        }
        if displays.is_empty() {
            None
        } else {
            Some(displays)
        }
    }

    // RustDesk-derived display ordering idea: keep Windows GDI monitor order,
    // then match DXGI outputs by device name so announced indexes equal capture indexes.
    fn reorder_dxgi_by_gdi(dxgi: Vec<CaptureDisplay>) -> Vec<CaptureDisplay> {
        let gdi = unsafe { gdi_display_names() };
        reorder_dxgi_by_names(dxgi, &gdi)
    }

    fn reorder_dxgi_by_names(dxgi: Vec<CaptureDisplay>, gdi: &[String]) -> Vec<CaptureDisplay> {
        if gdi.is_empty() {
            return dxgi;
        }

        let mut ordered = Vec::with_capacity(dxgi.len());
        let mut used = vec![false; dxgi.len()];
        for name in gdi {
            if let Some((pos, display)) = dxgi
                .iter()
                .enumerate()
                .find(|(idx, display)| !used[*idx] && display.name.eq_ignore_ascii_case(name))
            {
                used[pos] = true;
                ordered.push(display.clone());
            }
        }
        for (idx, display) in dxgi.iter().enumerate() {
            if !used[idx] {
                ordered.push(display.clone());
            }
        }
        for (idx, display) in ordered.iter_mut().enumerate() {
            display.index = idx as i32;
        }
        ordered
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn gdi_order_reindexes_dxgi_outputs() {
            let dxgi = vec![
                CaptureDisplay {
                    index: 0,
                    x: 2560,
                    y: 0,
                    width: 1920,
                    height: 1080,
                    name: "\\\\.\\DISPLAY2".to_owned(),
                },
                CaptureDisplay {
                    index: 1,
                    x: 0,
                    y: 0,
                    width: 2560,
                    height: 1440,
                    name: "\\\\.\\DISPLAY1".to_owned(),
                },
            ];

            let ordered = reorder_dxgi_by_names(
                dxgi,
                &["\\\\.\\DISPLAY1".to_owned(), "\\\\.\\DISPLAY2".to_owned()],
            );

            assert_eq!(ordered[0].name, "\\\\.\\DISPLAY1");
            assert_eq!(ordered[0].index, 0);
            assert_eq!(ordered[1].name, "\\\\.\\DISPLAY2");
            assert_eq!(ordered[1].index, 1);
        }
    }

    unsafe fn gdi_display_names() -> Vec<String> {
        let mut displays = Vec::new();
        for device_index in 0..32_u32 {
            let mut device = DISPLAY_DEVICEW::default();
            device.cb = size_of::<DISPLAY_DEVICEW>() as u32;
            if !EnumDisplayDevicesW(PCWSTR::null(), device_index, &mut device, 0).as_bool() {
                break;
            }
            if device.StateFlags & DISPLAY_DEVICE_ACTIVE == 0 {
                continue;
            }
            if device.StateFlags & DISPLAY_DEVICE_MIRRORING_DRIVER != 0 {
                continue;
            }
            if let Some(name) = utf16_z_to_string(&device.DeviceName) {
                displays.push(name);
            }
        }
        displays
    }

    fn fallback_display_infos() -> Vec<CaptureDisplay> {
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        if w <= 0 || h <= 0 {
            Vec::new()
        } else {
            vec![CaptureDisplay {
                index: 0,
                x: 0,
                y: 0,
                width: w,
                height: h,
                name: "Display 1".to_owned(),
            }]
        }
    }

    fn utf16_z_to_string(buf: &[u16]) -> Option<String> {
        let len = buf.iter().position(|ch| *ch == 0).unwrap_or(buf.len());
        if len == 0 {
            None
        } else {
            Some(String::from_utf16_lossy(&buf[..len]))
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
        return capture_display_into(0, pixels);
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

/// Capture a specific display by the index announced in `display_infos`.
#[allow(unused)]
pub fn leak_capture_resources() {
    #[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
    win::leak_capture_resources();
}

#[allow(unused)]
pub fn capture_display_into(display: i32, pixels: &mut Vec<u8>) -> Option<(u32, u32)> {
    #[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
    return win::capture_display_into(display, pixels);

    #[cfg(target_os = "linux")]
    {
        if let Some(size) = linux_x11_shm::capture_display_into(display, pixels) {
            return Some(size);
        }
        let (width, height, data) = linux_x11::capture_display(display)?;
        *pixels = data;
        return Some((width, height));
    }

    #[cfg(not(any(
        all(target_os = "windows", feature = "live-vp9-mf"),
        target_os = "linux"
    )))]
    {
        let _ = display;
        capture_screen_into(pixels)
    }
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

#[allow(unused)]
pub fn display_infos() -> Vec<CaptureDisplay> {
    #[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
    return win::display_infos();

    #[cfg(target_os = "linux")]
    return linux_x11::display_infos();

    #[cfg(not(any(
        all(target_os = "windows", feature = "live-vp9-mf"),
        target_os = "linux"
    )))]
    screen_size()
        .map(|(width, height)| {
            vec![CaptureDisplay {
                index: 0,
                x: 0,
                y: 0,
                width: width as i32,
                height: height as i32,
                name: "Display 1".to_owned(),
            }]
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
mod linux_x11 {
    use std::{
        cell::RefCell,
        process::{Command, Stdio},
        sync::OnceLock,
    };

    use super::{select_capture_display, sort_and_reindex_displays, CaptureDisplay};

    use x11rb::{
        connection::Connection,
        protocol::{
            randr::{self, ConnectionExt as RandrExt},
            xproto::{ConnectionExt as XprotoExt, ImageFormat},
        },
        rust_connection::RustConnection,
    };

    thread_local! {
        static X11_CAPTURE: RefCell<Option<X11Capture>> = const { RefCell::new(None) };
    }

    struct X11Capture {
        conn: RustConnection,
        root: u32,
        display: CaptureDisplay,
    }

    impl X11Capture {
        fn connect(display: i32) -> Option<Self> {
            let (conn, screen_num) = x11rb::connect(None).ok()?;
            let screen = &conn.setup().roots[screen_num];
            let selected = select_display_from_conn(&conn, screen.root, display)
                .or_else(|| root_display_from_screen(screen))?;
            if selected.width <= 0 || selected.height <= 0 {
                return None;
            }
            Some(Self {
                root: screen.root,
                display: selected,
                conn,
            })
        }

        fn capture(&self) -> Option<(u32, u32, Vec<u8>)> {
            let (x, y, width, height) = x11_region(&self.display)?;
            let reply = self
                .conn
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    self.root,
                    x,
                    y,
                    width,
                    height,
                    u32::MAX,
                )
                .ok()?
                .reply()
                .ok()?;

            bgra_from_ximage(width as u32, height as u32, reply.data)
        }

        fn matches(&self, display: &CaptureDisplay) -> bool {
            self.display.index == display.index
                && self.display.x == display.x
                && self.display.y == display.y
                && self.display.width == display.width
                && self.display.height == display.height
        }
    }

    pub fn capture_display(display: i32) -> Option<(u32, u32, Vec<u8>)> {
        if let Some(frame) = capture_x11_cached(display) {
            return Some(frame);
        }
        capture_grim_ppm()
    }

    pub fn screen_size() -> Option<(u32, u32)> {
        if let Some(size) = select_display(0).and_then(|display| {
            Some((
                u32::try_from(display.width).ok()?,
                u32::try_from(display.height).ok()?,
            ))
        }) {
            return Some(size);
        }
        capture_grim_ppm().map(|(w, h, _)| (w, h))
    }

    pub fn display_infos() -> Vec<CaptureDisplay> {
        if let Some(displays) = randr_display_infos().filter(|displays| !displays.is_empty()) {
            return displays;
        }
        if let Some(display) = root_display() {
            return vec![display];
        }
        capture_grim_ppm()
            .map(|(width, height, _)| {
                vec![CaptureDisplay {
                    index: 0,
                    x: 0,
                    y: 0,
                    width: width as i32,
                    height: height as i32,
                    name: "Display 1".to_owned(),
                }]
            })
            .unwrap_or_default()
    }

    pub(super) fn select_display(display: i32) -> Option<CaptureDisplay> {
        let displays = display_infos();
        select_capture_display(&displays, display)
    }

    fn capture_x11_cached(display: i32) -> Option<(u32, u32, Vec<u8>)> {
        X11_CAPTURE.with(|cell| {
            let selected = select_display(display)?;
            if cell.borrow().is_none() {
                *cell.borrow_mut() = X11Capture::connect(selected.index);
            }
            let recreate = cell
                .borrow()
                .as_ref()
                .map(|capture| !capture.matches(&selected))
                .unwrap_or(true);
            if recreate {
                *cell.borrow_mut() = X11Capture::connect(selected.index);
            }
            let frame = cell.borrow().as_ref().and_then(X11Capture::capture);
            if frame.is_none() {
                *cell.borrow_mut() = None;
            }
            frame
        })
    }

    fn randr_display_infos() -> Option<Vec<CaptureDisplay>> {
        if std::env::var_os("DISPLAY").is_none() {
            return None;
        }
        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let reply = conn.randr_get_monitors(root, true).ok()?.reply().ok()?;
        let mut displays = reply
            .monitors
            .iter()
            .filter(|monitor| monitor.width > 0 && monitor.height > 0)
            .enumerate()
            .map(|(index, monitor)| CaptureDisplay {
                index: index as i32,
                x: monitor.x as i32,
                y: monitor.y as i32,
                width: monitor.width as i32,
                height: monitor.height as i32,
                name: monitor_name(&conn, monitor, index),
            })
            .collect::<Vec<_>>();
        if displays.is_empty() {
            return None;
        }
        Some(sort_and_reindex_displays(displays))
    }

    fn select_display_from_conn(
        conn: &RustConnection,
        root: u32,
        display: i32,
    ) -> Option<CaptureDisplay> {
        let reply = conn.randr_get_monitors(root, true).ok()?.reply().ok()?;
        let mut displays = reply
            .monitors
            .iter()
            .filter(|monitor| monitor.width > 0 && monitor.height > 0)
            .enumerate()
            .map(|(index, monitor)| CaptureDisplay {
                index: index as i32,
                x: monitor.x as i32,
                y: monitor.y as i32,
                width: monitor.width as i32,
                height: monitor.height as i32,
                name: monitor_name(conn, monitor, index),
            })
            .collect::<Vec<_>>();
        let displays = sort_and_reindex_displays(displays);
        select_capture_display(&displays, display)
    }

    fn monitor_name(
        conn: &RustConnection,
        monitor: &randr::MonitorInfo,
        fallback_index: usize,
    ) -> String {
        if monitor.name != 0 {
            if let Ok(cookie) = conn.get_atom_name(monitor.name) {
                if let Ok(reply) = cookie.reply() {
                    let name = String::from_utf8_lossy(&reply.name).trim().to_owned();
                    if !name.is_empty() {
                        return name;
                    }
                }
            }
        }
        for output in &monitor.outputs {
            if *output == 0 {
                continue;
            }
            if let Ok(cookie) = conn.randr_get_output_info(*output, 0) {
                if let Ok(reply) = cookie.reply() {
                    let name = String::from_utf8_lossy(&reply.name).trim().to_owned();
                    if !name.is_empty() {
                        return name;
                    }
                }
            }
        }
        if monitor.primary {
            format!("Primary display {}", fallback_index + 1)
        } else {
            format!("Display {}", fallback_index + 1)
        }
    }

    fn root_display() -> Option<CaptureDisplay> {
        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let screen = &conn.setup().roots[screen_num];
        root_display_from_screen(screen)
    }

    fn root_display_from_screen(
        screen: &x11rb::protocol::xproto::Screen,
    ) -> Option<CaptureDisplay> {
        if screen.width_in_pixels == 0 || screen.height_in_pixels == 0 {
            None
        } else {
            Some(CaptureDisplay {
                index: 0,
                x: 0,
                y: 0,
                width: screen.width_in_pixels as i32,
                height: screen.height_in_pixels as i32,
                name: "Display 1".to_owned(),
            })
        }
    }

    fn x11_region(display: &CaptureDisplay) -> Option<(i16, i16, u16, u16)> {
        let x = i16::try_from(display.x).ok()?;
        let y = i16::try_from(display.y).ok()?;
        let width = u16::try_from(display.width).ok()?;
        let height = u16::try_from(display.height).ok()?;
        if width == 0 || height == 0 {
            None
        } else {
            Some((x, y, width, height))
        }
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

    use super::{linux_x11, CaptureDisplay};

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
        display: CaptureDisplay,
        width: u16,
        height: u16,
        seg: shm::Seg,
        shm_id: libc::c_int,
        shm_ptr: *mut libc::c_void,
        byte_len: usize,
    }

    impl ShmCapture {
        fn connect(display: i32) -> Option<Self> {
            // Only attempt on X11 sessions.
            if std::env::var_os("DISPLAY").is_none() {
                return None;
            }

            let (conn, screen_num) = x11rb::connect(None).ok()?;
            let screen = &conn.setup().roots[screen_num];
            let selected = linux_x11::select_display(display)?;
            let width = u16::try_from(selected.width).ok()?;
            let height = u16::try_from(selected.height).ok()?;
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
            let shm_id =
                unsafe { libc::shmget(libc::IPC_PRIVATE, byte_len, libc::IPC_CREAT | 0o600) };
            if shm_id < 0 {
                return None;
            }
            let shm_ptr = unsafe { libc::shmat(shm_id, std::ptr::null(), 0) };
            if shm_ptr as isize == -1 {
                unsafe { libc::shmctl(shm_id, libc::IPC_RMID, std::ptr::null_mut()) };
                return None;
            }
            // Mark segment for deletion as soon as the last process detaches.
            unsafe { libc::shmctl(shm_id, libc::IPC_RMID, std::ptr::null_mut()) };

            // Register with the X server.
            let seg = conn.generate_id().ok()?;
            conn.shm_attach(seg, shm_id as u32, false)
                .ok()?
                .check()
                .ok()?;

            Some(Self {
                conn,
                root,
                display: selected,
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
            let (x, y) = self.origin()?;

            // Ask the server to fill our shared-memory segment.
            self.conn
                .shm_get_image(
                    self.root,
                    x,
                    y,
                    self.width,
                    self.height,
                    !0u32, // plane_mask = all planes
                    2u8,   // XCB_IMAGE_FORMAT_Z_PIXMAP
                    self.seg,
                    0, // offset in segment
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
        fn matches(&self, display: &CaptureDisplay) -> bool {
            self.display.index == display.index
                && self.display.x == display.x
                && self.display.y == display.y
                && self.display.width == display.width
                && self.display.height == display.height
        }

        fn origin(&self) -> Option<(i16, i16)> {
            Some((
                i16::try_from(self.display.x).ok()?,
                i16::try_from(self.display.y).ok()?,
            ))
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
    pub fn capture_display_into(display: i32, out: &mut Vec<u8>) -> Option<(u32, u32)> {
        SHM.with(|cell| {
            let selected = linux_x11::select_display(display)?;
            // Lazily initialise on first call.
            if cell.borrow().is_none() {
                *cell.borrow_mut() = ShmCapture::connect(selected.index);
            }

            // Detect resolution change and recreate.
            let needs_recreate = cell
                .borrow()
                .as_ref()
                .map(|cap| !cap.matches(&selected))
                .unwrap_or(true);
            if needs_recreate {
                *cell.borrow_mut() = ShmCapture::connect(selected.index);
            }

            let result = cell.borrow().as_ref().and_then(|cap| cap.capture_into(out));

            if result.is_none() {
                // Invalidate on any failure so the next call retries.
                *cell.borrow_mut() = None;
            }
            result
        })
    }
}
