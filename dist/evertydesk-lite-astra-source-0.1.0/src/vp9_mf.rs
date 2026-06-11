/// VP9 decoder backed by Windows Media Foundation.
///
/// Available on Windows 10 build 1803+ and Windows 11 — the VP9 Video Extension
/// is bundled with the OS. No external DLLs or packages required.
///
/// Activated when the `live-vp9-mf` feature is enabled (default on Windows).
#[cfg(all(feature = "live-vp9-mf", target_os = "windows"))]
mod inner {
    use windows::{
        core::Result as WResult,
        Win32::{
            Foundation::E_FAIL,
            Media::MediaFoundation::{
                IMFActivate, IMFSample, IMFTransform, MFCreateMediaType, MFCreateMemoryBuffer,
                MFCreateSample, MFMediaType_Video, MFStartup, MFTEnumEx, MFVideoFormat_I420,
                MFVideoFormat_IYUV, MFVideoFormat_NV12, MFVideoFormat_VP90, MFSTARTUP_NOSOCKET,
                MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_SYNCMFT,
                MFT_OUTPUT_DATA_BUFFER, MFT_REGISTER_TYPE_INFO, MF_E_TRANSFORM_NEED_MORE_INPUT,
                MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO,
                MF_MT_SUBTYPE, MF_VERSION,
            },
            System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED},
        },
    };

    // MF_E_TRANSFORM_STREAM_CHANGE (0xC00D6D61): output format changed mid-stream.
    // Returned by ProcessOutput when the decoder determines actual resolution from
    // the bitstream. We must re-negotiate the output type and retry.
    const MF_E_TRANSFORM_STREAM_CHANGE: i32 = 0xC00D6D61u32 as i32;

    // MFT_OUTPUT_STREAM_PROVIDES_SAMPLES flag: set when MFT allocates its own
    // output samples. When not set, the caller must allocate the output buffer.
    const MFT_OUTPUT_STREAM_PROVIDES_SAMPLES_FLAG: u32 = 0x100;

    pub struct Vp9MfDecoder {
        transform: IMFTransform,
        configured: bool,
        /// True if the MFT allocates its own output samples.
        provides_samples: bool,
        /// Required output buffer size when `!provides_samples`.
        output_buf_size: u32,
        /// Frame counter for debug throttling.
        frame_count: u64,
    }

    // IMFTransform is a COM interface — it's safe to Send across threads in our usage
    // (single-threaded decoder loop).
    unsafe impl Send for Vp9MfDecoder {}

    impl Vp9MfDecoder {
        /// Try to create a decoder. Returns `None` if VP9 MFT is not available
        /// (Windows < 10 1803, or VP9 extensions not installed).
        pub fn new() -> Option<Self> {
            unsafe { Self::create().ok() }
        }

        unsafe fn create() -> WResult<Self> {
            // COM must be initialized on this thread before calling MFStartup.
            // Returns S_FALSE if already initialized — both outcomes are fine.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            // Idempotent — fine to call multiple times.
            let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);

            let input = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: MFVideoFormat_VP90,
            };

            let mut count = 0u32;
            let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();

            // Request only SYNC MFTs. Async (hardware) MFTs require an event pump
            // which we don't implement. The built-in software VP9 decoder is sync.
            MFTEnumEx(
                MFT_CATEGORY_VIDEO_DECODER,
                MFT_ENUM_FLAG(MFT_ENUM_FLAG_SYNCMFT.0),
                Some(&input),
                None,
                &mut activates,
                &mut count,
            )?;

            if count == 0 || activates.is_null() {
                eprintln!("[vp9-mf] No VP9 MFT found on this system");
                return Err(windows::core::Error::from(E_FAIL));
            }

            eprintln!("[vp9-mf] Found {} VP9 MFT candidate(s)", count);

            let slice = std::slice::from_raw_parts(activates, count as usize);
            let activate = slice[0]
                .as_ref()
                .ok_or_else(|| windows::core::Error::from(E_FAIL))?;

            let transform: IMFTransform = activate.ActivateObject()?;
            CoTaskMemFree(Some(activates as *const _));

            eprintln!("[vp9-mf] Windows Media Foundation VP9 decoder created OK");
            Ok(Self {
                transform,
                configured: false,
                provides_samples: false,
                output_buf_size: 0,
                frame_count: 0,
            })
        }

        /// Decode one VP9 bitstream packet.  Returns `Ok(None)` when the decoder
        /// needs more data before it can produce a frame (normal on first call).
        pub fn decode(&mut self, data: &[u8]) -> Result<Option<(usize, usize, Vec<u8>)>, String> {
            unsafe { self.run(data).map_err(|e| format!("MF VP9: {e}")) }
        }

        unsafe fn configure(&mut self) -> WResult<()> {
            eprintln!("[vp9-mf] configure() start");

            // ── Input type: raw VP9 bitstream ─────────────────────────────────────
            // Some VP9 MFTs require MF_MT_FRAME_SIZE + MF_MT_FRAME_RATE even though
            // the actual resolution is carried in the bitstream.  We set a common
            // default (1920×1080 @ 30 fps) and rely on MF_E_TRANSFORM_STREAM_CHANGE
            // to correct it once the decoder reads the real resolution.
            let in_type = MFCreateMediaType()?;
            in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_VP90)?;
            // MF_MT_FRAME_SIZE:  high 32 bits = width, low 32 bits = height
            in_type.SetUINT64(&MF_MT_FRAME_SIZE, (1920u64 << 32) | 1080)?;
            // MF_MT_FRAME_RATE:  high 32 bits = numerator, low = denominator
            in_type.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1)?;
            // MF_MT_PIXEL_ASPECT_RATIO: square pixels
            in_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;

            match self.transform.SetInputType(0, &in_type, 0) {
                Ok(()) => eprintln!("[vp9-mf] SetInputType(VP90 1920x1080) OK"),
                Err(e) => {
                    eprintln!(
                        "[vp9-mf] SetInputType FAILED 0x{:08X}: {e:?}",
                        e.code().0 as u32
                    );
                    return Err(e);
                }
            }

            // ── Check whether MFT provides its own output samples ─────────────────
            if let Ok(info) = self.transform.GetOutputStreamInfo(0) {
                self.provides_samples =
                    (info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES_FLAG) != 0;
                self.output_buf_size = info.cbSize;
                eprintln!(
                    "[vp9-mf] output stream: provides_samples={} cbSize={}",
                    self.provides_samples, self.output_buf_size
                );
            } else {
                // If we can't query, assume we need to provide samples.
                self.provides_samples = false;
                self.output_buf_size = 1920 * 1080 * 3 / 2; // safe over-estimate
                eprintln!("[vp9-mf] GetOutputStreamInfo failed — assuming caller-provided samples");
            }

            // ── Output type: prefer NV12 / I420 / IYUV ───────────────────────────
            let preferred = [MFVideoFormat_NV12, MFVideoFormat_I420, MFVideoFormat_IYUV];
            let mut found = false;
            let mut idx = 0u32;
            while let Ok(out_type) = self.transform.GetOutputAvailableType(0, idx) {
                let sub = out_type.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
                eprintln!("[vp9-mf] available output type {idx}: {sub:?}");
                if preferred.contains(&sub) {
                    self.transform.SetOutputType(0, &out_type, 0)?;
                    eprintln!("[vp9-mf] output format selected: {sub:?}");
                    found = true;
                    break;
                }
                idx += 1;
                if idx > 32 {
                    break; // safety cap
                }
            }
            if !found {
                eprintln!("[vp9-mf] no preferred output type found — trying explicit NV12");
                let out_type = MFCreateMediaType()?;
                out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
                match self.transform.SetOutputType(0, &out_type, 0) {
                    Ok(()) => eprintln!("[vp9-mf] NV12 fallback SetOutputType OK"),
                    Err(e) => {
                        eprintln!("[vp9-mf] NV12 fallback SetOutputType FAILED: {e:?}");
                        eprintln!("[vp9-mf] will let MFT set output type after first frame");
                        // Some MFTs refuse SetOutputType before seeing data.
                        // Proceed — after ProcessInput, GetOutputCurrentType will work.
                    }
                }
            }

            self.configured = true;
            eprintln!("[vp9-mf] configure() done");
            Ok(())
        }

        unsafe fn run(&mut self, data: &[u8]) -> WResult<Option<(usize, usize, Vec<u8>)>> {
            if !self.configured {
                self.configure()?;
            }

            // ── Build input sample ──────────────────────────────────────────────
            let mem_buf = MFCreateMemoryBuffer(data.len() as u32)?;
            {
                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut _max = 0u32;
                let mut _cur = 0u32;
                mem_buf.Lock(&mut ptr, Some(&mut _max), Some(&mut _cur))?;
                std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
                mem_buf.Unlock()?;
                mem_buf.SetCurrentLength(data.len() as u32)?;
            }
            let in_sample: IMFSample = MFCreateSample()?;
            in_sample.AddBuffer(&mem_buf)?;

            if self.frame_count < 10 || self.frame_count % 100 == 0 {
                eprintln!(
                    "[vp9-mf] ProcessInput #{} ({} bytes)",
                    self.frame_count,
                    data.len()
                );
            }
            self.frame_count += 1;

            match self.transform.ProcessInput(0, &in_sample, 0) {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("[vp9-mf] ProcessInput FAILED: {e:?}");
                    return Err(e);
                }
            }

            // ── Collect output (retry loop handles MF_E_TRANSFORM_STREAM_CHANGE) ─
            let mut retries = 0u32;
            loop {
                // Pre-allocate output sample if MFT won't do it itself.
                let pre_sample: Option<IMFSample> = if self.provides_samples {
                    None
                } else {
                    let sz = self.output_buf_size.max(1920 * 1080 * 3 / 2);
                    let buf = MFCreateMemoryBuffer(sz)?;
                    let s = MFCreateSample()?;
                    s.AddBuffer(&buf)?;
                    Some(s)
                };

                let mut out_buf = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(pre_sample),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(None),
                };
                let mut flags = 0u32;

                let hr =
                    self.transform
                        .ProcessOutput(0, std::slice::from_mut(&mut out_buf), &mut flags);

                // Always drop pEvents to avoid COM leak.
                std::mem::ManuallyDrop::drop(&mut out_buf.pEvents);

                match hr {
                    Ok(()) => {
                        // Successfully got output.
                        let out_sample = match std::mem::ManuallyDrop::into_inner(out_buf.pSample) {
                            Some(s) => s,
                            None => {
                                eprintln!("[vp9-mf] ProcessOutput OK but pSample is None");
                                return Ok(None);
                            }
                        };
                        return self.extract_rgba(out_sample);
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                        // Drop the unused pre-allocated sample.
                        std::mem::ManuallyDrop::drop(&mut out_buf.pSample);
                        if self.frame_count <= 11 || self.frame_count % 100 == 0 {
                            eprintln!(
                                "[vp9-mf] ProcessOutput: NEED_MORE_INPUT (frame #{})",
                                self.frame_count
                            );
                        }
                        return Ok(None);
                    }
                    Err(e) if e.code().0 == MF_E_TRANSFORM_STREAM_CHANGE => {
                        // Output format changed — re-negotiate, then retry ProcessOutput.
                        std::mem::ManuallyDrop::drop(&mut out_buf.pSample);
                        eprintln!("[vp9-mf] ProcessOutput: stream format changed, re-negotiating");
                        self.renegotiate_output()?;
                        retries += 1;
                        if retries > 5 {
                            return Err(windows::core::Error::from(E_FAIL));
                        }
                        continue;
                    }
                    Err(e) => {
                        std::mem::ManuallyDrop::drop(&mut out_buf.pSample);
                        eprintln!(
                            "[vp9-mf] ProcessOutput FAILED 0x{:08X}: {e:?}",
                            e.code().0 as u32
                        );
                        return Err(e);
                    }
                }
            }
        }

        /// After MF_E_TRANSFORM_STREAM_CHANGE, accept the new output type.
        unsafe fn renegotiate_output(&mut self) -> WResult<()> {
            let preferred = [MFVideoFormat_NV12, MFVideoFormat_I420, MFVideoFormat_IYUV];
            let mut found = false;
            let mut idx = 0u32;
            while let Ok(t) = self.transform.GetOutputAvailableType(0, idx) {
                let sub = t.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
                if preferred.contains(&sub) {
                    self.transform.SetOutputType(0, &t, 0)?;
                    eprintln!("[vp9-mf] renegotiate: new output type {sub:?}");
                    found = true;
                    break;
                }
                idx += 1;
            }
            if !found {
                // Accept whatever the current type is.
                if let Ok(t) = self.transform.GetOutputCurrentType(0) {
                    self.transform.SetOutputType(0, &t, 0)?;
                    eprintln!("[vp9-mf] renegotiate: accepted current output type");
                }
            }
            // Refresh stream info (buffer size may have changed).
            if let Ok(info) = self.transform.GetOutputStreamInfo(0) {
                self.provides_samples =
                    (info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES_FLAG) != 0;
                self.output_buf_size = info.cbSize;
                eprintln!(
                    "[vp9-mf] renegotiate: provides_samples={} cbSize={}",
                    self.provides_samples, self.output_buf_size
                );
            }
            Ok(())
        }

        /// Read pixels from a decoded output sample and convert to RGBA.
        unsafe fn extract_rgba(
            &self,
            out_sample: IMFSample,
        ) -> WResult<Option<(usize, usize, Vec<u8>)>> {
            // Dimensions from current output type.
            // MF_MT_FRAME_SIZE packs width (high 32 bits) and height (low 32 bits) into a UINT64.
            let (width, height) = self
                .transform
                .GetOutputCurrentType(0)
                .and_then(|t| t.GetUINT64(&MF_MT_FRAME_SIZE))
                .map(|packed| ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32))
                .unwrap_or((0, 0));

            if width == 0 || height == 0 {
                eprintln!("[vp9-mf] extract_rgba: dimensions unknown (0x0)");
                return Ok(None);
            }

            let pixel_buf = out_sample.GetBufferByIndex(0)?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut _max = 0u32;
            let mut cur_len = 0u32;
            pixel_buf.Lock(&mut ptr, Some(&mut _max), Some(&mut cur_len))?;

            let expected_nv12 = (width * height * 3 / 2) as usize;
            let available = cur_len as usize;

            let result = if available >= expected_nv12 {
                if self.frame_count <= 11 {
                    eprintln!(
                        "[vp9-mf] frame #{}: {}x{} NV12 {} bytes → RGBA",
                        self.frame_count, width, height, available
                    );
                }
                let nv12 = std::slice::from_raw_parts(ptr, expected_nv12);
                Some((
                    width as usize,
                    height as usize,
                    nv12_to_rgba(nv12, width as usize, height as usize),
                ))
            } else {
                eprintln!(
                    "[vp9-mf] buffer too small: {available} < {expected_nv12} for {width}x{height}"
                );
                None
            };

            pixel_buf.Unlock()?;
            Ok(result)
        }
    }

    /// NV12 (Y plane + interleaved UV) → RGBA using BT.601 limited-range coefficients.
    fn nv12_to_rgba(nv12: &[u8], w: usize, h: usize) -> Vec<u8> {
        let mut out = vec![0u8; w * h * 4];
        let y_plane = &nv12[..w * h];
        let uv_plane = &nv12[w * h..];
        for row in 0..h {
            for col in 0..w {
                let y = y_plane[row * w + col] as i32 - 16;
                let uv_row = row / 2;
                let uv_col = col & !1; // round down to even column
                let u = uv_plane[uv_row * w + uv_col] as i32 - 128;
                let v = uv_plane[uv_row * w + uv_col + 1] as i32 - 128;
                let c = 298 * y;
                let r = ((c + 409 * v + 128) >> 8).clamp(0, 255) as u8;
                let g = ((c - 100 * u - 208 * v + 128) >> 8).clamp(0, 255) as u8;
                let b = ((c + 516 * u + 128) >> 8).clamp(0, 255) as u8;
                let off = (row * w + col) * 4;
                out[off] = r;
                out[off + 1] = g;
                out[off + 2] = b;
                out[off + 3] = 255;
            }
        }
        out
    }
}

// ── Public re-export ──────────────────────────────────────────────────────────

#[cfg(all(feature = "live-vp9-mf", target_os = "windows"))]
pub use inner::Vp9MfDecoder;

/// Stub for non-Windows or when feature is disabled.
#[cfg(not(all(feature = "live-vp9-mf", target_os = "windows")))]
pub struct Vp9MfDecoder;

#[cfg(not(all(feature = "live-vp9-mf", target_os = "windows")))]
impl Vp9MfDecoder {
    pub fn new() -> Option<Self> {
        None
    }
    pub fn decode(&mut self, _data: &[u8]) -> Result<Option<(usize, usize, Vec<u8>)>, String> {
        Err("VP9 MF decoder not available on this platform".to_owned())
    }
}
