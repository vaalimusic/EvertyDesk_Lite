/// Windows Media Foundation video decoder for compressed live-video packets.
///
/// This is the first native backend that does not spawn an external video tool. It uses
/// system MFT decoders directly and converts decoded NV12/I420 frames to RGBA
/// for the existing egui texture path.
#[cfg(all(feature = "live-vp9-mf", target_os = "windows"))]
mod inner {
    use std::sync::OnceLock;

    use windows::{
        core::{Result as WResult, GUID},
        Win32::{
            Foundation::E_FAIL,
            Media::MediaFoundation::{
                IMFActivate, IMFSample, IMFTransform, MFCreateMediaType, MFCreateMemoryBuffer,
                MFCreateSample, MFMediaType_Video, MFStartup, MFTEnumEx, MFVideoFormat_AV1,
                MFVideoFormat_H264, MFVideoFormat_H264_ES, MFVideoFormat_H265, MFVideoFormat_HEVC,
                MFVideoFormat_HEVC_ES, MFVideoFormat_I420, MFVideoFormat_IYUV, MFVideoFormat_NV12,
                MFVideoFormat_VP90, MFSTARTUP_NOSOCKET, MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG,
                MFT_ENUM_FLAG_SYNCMFT, MFT_OUTPUT_DATA_BUFFER, MFT_REGISTER_TYPE_INFO,
                MF_E_TRANSFORM_NEED_MORE_INPUT, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
                MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_VERSION,
            },
            System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED},
        },
    };

    const MF_E_TRANSFORM_STREAM_CHANGE: i32 = 0xC00D6D61u32 as i32;
    const MFT_OUTPUT_STREAM_PROVIDES_SAMPLES_FLAG: u32 = 0x100;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum MfVideoCodec {
        Vp9,
        H264,
        H265,
        Av1,
    }

    impl MfVideoCodec {
        pub fn label(self) -> &'static str {
            match self {
                Self::Vp9 => "VP9",
                Self::H264 => "H264",
                Self::H265 => "H265",
                Self::Av1 => "AV1",
            }
        }

        fn input_subtypes(self) -> &'static [GUID] {
            match self {
                Self::Vp9 => &[MFVideoFormat_VP90],
                Self::H264 => &[MFVideoFormat_H264, MFVideoFormat_H264_ES],
                Self::H265 => &[
                    MFVideoFormat_HEVC,
                    MFVideoFormat_H265,
                    MFVideoFormat_HEVC_ES,
                ],
                Self::Av1 => &[MFVideoFormat_AV1],
            }
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct MfVideoDecodeStatus {
        pub vp9: bool,
        pub h264: bool,
        pub h265: bool,
        pub av1: bool,
    }

    impl MfVideoDecodeStatus {
        pub fn label(&self) -> String {
            let mut codecs = Vec::new();
            if self.vp9 {
                codecs.push("VP9");
            }
            if self.h264 {
                codecs.push("H264");
            }
            if self.h265 {
                codecs.push("H265");
            }
            if self.av1 {
                codecs.push("AV1");
            }
            if codecs.is_empty() {
                "Media Foundation decode: unavailable".to_owned()
            } else {
                format!("Media Foundation decode: {}", codecs.join(", "))
            }
        }
    }

    pub fn mf_video_decode_status() -> &'static MfVideoDecodeStatus {
        static STATUS: OnceLock<MfVideoDecodeStatus> = OnceLock::new();
        STATUS.get_or_init(|| MfVideoDecodeStatus {
            vp9: decoder_available(MfVideoCodec::Vp9),
            h264: decoder_available(MfVideoCodec::H264),
            h265: decoder_available(MfVideoCodec::H265),
            av1: decoder_available(MfVideoCodec::Av1),
        })
    }

    pub fn h264_decode_available() -> bool {
        mf_video_decode_status().h264
    }

    pub fn h265_decode_available() -> bool {
        mf_video_decode_status().h265
    }

    pub fn av1_decode_available() -> bool {
        // av1decodermft_store.dll is present on many Windows machines but crashes
        // with a null pointer (0xC0000005) when actually instantiated — same bug
        // as HEVCDECODER_STORE.dll. Disable until a stable AV1 decode path exists.
        false
    }

    fn decoder_available(codec: MfVideoCodec) -> bool {
        unsafe { first_decoder_subtype(codec).is_some() }
    }

    unsafe fn first_decoder_subtype(codec: MfVideoCodec) -> Option<GUID> {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);

        for subtype in codec.input_subtypes() {
            let input = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: *subtype,
            };
            let mut count = 0u32;
            let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
            let found = MFTEnumEx(
                MFT_CATEGORY_VIDEO_DECODER,
                MFT_ENUM_FLAG(MFT_ENUM_FLAG_SYNCMFT.0),
                Some(&input),
                None,
                &mut activates,
                &mut count,
            )
            .is_ok()
                && count > 0
                && !activates.is_null();

            if !activates.is_null() {
                CoTaskMemFree(Some(activates as *const _));
            }
            if found {
                return Some(*subtype);
            }
        }
        None
    }

    pub struct MfVideoDecoder {
        codec: MfVideoCodec,
        input_subtype: GUID,
        transform: IMFTransform,
        width: u32,
        height: u32,
        configured: bool,
        provides_samples: bool,
        output_buf_size: u32,
        frame_count: u64,
    }

    unsafe impl Send for MfVideoDecoder {}

    impl MfVideoDecoder {
        pub fn new(codec: MfVideoCodec, width: u32, height: u32) -> Result<Self, String> {
            unsafe {
                Self::create(codec, width, height)
                    .map_err(|err| format!("MF {}: {err}", codec.label()))
            }
        }

        pub fn matches(&self, codec: MfVideoCodec, width: u32, height: u32) -> bool {
            self.codec == codec && self.width == width.max(2) && self.height == height.max(2)
        }

        pub fn decode_packets<I>(
            &mut self,
            packets: I,
        ) -> Result<Option<(usize, usize, Vec<u8>)>, String>
        where
            I: IntoIterator,
            I::Item: AsRef<[u8]>,
        {
            let mut latest = None;
            for packet in packets {
                let bytes = packet.as_ref();
                if bytes.is_empty() {
                    continue;
                }
                if let Some(frame) = unsafe {
                    self.run(bytes)
                        .map_err(|err| format!("MF {}: {err}", self.codec.label()))?
                } {
                    latest = Some(frame);
                }
            }
            Ok(latest)
        }

        unsafe fn create(codec: MfVideoCodec, width: u32, height: u32) -> WResult<Self> {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);

            for subtype in codec.input_subtypes() {
                let input = MFT_REGISTER_TYPE_INFO {
                    guidMajorType: MFMediaType_Video,
                    guidSubtype: *subtype,
                };
                let mut count = 0u32;
                let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
                MFTEnumEx(
                    MFT_CATEGORY_VIDEO_DECODER,
                    MFT_ENUM_FLAG(MFT_ENUM_FLAG_SYNCMFT.0),
                    Some(&input),
                    None,
                    &mut activates,
                    &mut count,
                )?;

                if count == 0 || activates.is_null() {
                    continue;
                }

                let slice = std::slice::from_raw_parts(activates, count as usize);
                let Some(activate) = slice.first().and_then(Option::as_ref) else {
                    CoTaskMemFree(Some(activates as *const _));
                    continue;
                };
                let transform: IMFTransform = activate.ActivateObject()?;
                CoTaskMemFree(Some(activates as *const _));

                eprintln!(
                    "[mf-video] {} decoder created at requested {}x{}",
                    codec.label(),
                    width,
                    height
                );
                return Ok(Self {
                    codec,
                    input_subtype: *subtype,
                    transform,
                    width: width.max(2),
                    height: height.max(2),
                    configured: false,
                    provides_samples: false,
                    output_buf_size: 0,
                    frame_count: 0,
                });
            }

            Err(windows::core::Error::from(E_FAIL))
        }

        unsafe fn configure(&mut self) -> WResult<()> {
            let in_type = MFCreateMediaType()?;
            in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            in_type.SetGUID(&MF_MT_SUBTYPE, &self.input_subtype)?;
            in_type.SetUINT64(
                &MF_MT_FRAME_SIZE,
                (u64::from(self.width) << 32) | u64::from(self.height),
            )?;
            in_type.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1)?;
            in_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
            self.transform.SetInputType(0, &in_type, 0)?;

            self.refresh_output_stream_info();
            self.select_output_type()?;
            self.configured = true;
            Ok(())
        }

        unsafe fn refresh_output_stream_info(&mut self) {
            if let Ok(info) = self.transform.GetOutputStreamInfo(0) {
                self.provides_samples =
                    (info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES_FLAG) != 0;
                self.output_buf_size = info.cbSize;
            } else {
                self.provides_samples = false;
                self.output_buf_size = self.width.saturating_mul(self.height).saturating_mul(3) / 2;
            }
        }

        unsafe fn select_output_type(&mut self) -> WResult<()> {
            let preferred = [MFVideoFormat_NV12, MFVideoFormat_I420, MFVideoFormat_IYUV];
            let mut idx = 0u32;
            while let Ok(out_type) = self.transform.GetOutputAvailableType(0, idx) {
                let sub = out_type.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
                if preferred.contains(&sub) {
                    self.transform.SetOutputType(0, &out_type, 0)?;
                    return Ok(());
                }
                idx += 1;
                if idx > 32 {
                    break;
                }
            }

            let out_type = MFCreateMediaType()?;
            out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            let _ = self.transform.SetOutputType(0, &out_type, 0);
            Ok(())
        }

        unsafe fn run(&mut self, data: &[u8]) -> WResult<Option<(usize, usize, Vec<u8>)>> {
            if !self.configured {
                self.configure()?;
            }

            let mem_buf = MFCreateMemoryBuffer(data.len() as u32)?;
            {
                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut max_len = 0u32;
                let mut cur_len = 0u32;
                mem_buf.Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))?;
                std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
                mem_buf.Unlock()?;
                mem_buf.SetCurrentLength(data.len() as u32)?;
            }
            let in_sample = MFCreateSample()?;
            in_sample.AddBuffer(&mem_buf)?;

            self.transform.ProcessInput(0, &in_sample, 0)?;
            self.frame_count = self.frame_count.saturating_add(1);

            let mut retries = 0u32;
            loop {
                let pre_sample: Option<IMFSample> = if self.provides_samples {
                    None
                } else {
                    let sz = self
                        .output_buf_size
                        .max(self.width.saturating_mul(self.height).saturating_mul(3) / 2);
                    let buf = MFCreateMemoryBuffer(sz)?;
                    let sample = MFCreateSample()?;
                    sample.AddBuffer(&buf)?;
                    Some(sample)
                };

                let mut out_buf = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(pre_sample),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(None),
                };
                let mut flags = 0u32;
                let result =
                    self.transform
                        .ProcessOutput(0, std::slice::from_mut(&mut out_buf), &mut flags);

                std::mem::ManuallyDrop::drop(&mut out_buf.pEvents);

                match result {
                    Ok(()) => {
                        let out_sample = match std::mem::ManuallyDrop::into_inner(out_buf.pSample) {
                            Some(sample) => sample,
                            None => return Ok(None),
                        };
                        return self.extract_rgba(out_sample);
                    }
                    Err(err) if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                        std::mem::ManuallyDrop::drop(&mut out_buf.pSample);
                        return Ok(None);
                    }
                    Err(err) if err.code().0 == MF_E_TRANSFORM_STREAM_CHANGE => {
                        std::mem::ManuallyDrop::drop(&mut out_buf.pSample);
                        self.renegotiate_output()?;
                        retries += 1;
                        if retries > 5 {
                            return Err(windows::core::Error::from(E_FAIL));
                        }
                    }
                    Err(err) => {
                        std::mem::ManuallyDrop::drop(&mut out_buf.pSample);
                        return Err(err);
                    }
                }
            }
        }

        unsafe fn renegotiate_output(&mut self) -> WResult<()> {
            self.select_output_type()?;
            self.refresh_output_stream_info();
            Ok(())
        }

        unsafe fn extract_rgba(
            &self,
            out_sample: IMFSample,
        ) -> WResult<Option<(usize, usize, Vec<u8>)>> {
            let out_type = self.transform.GetOutputCurrentType(0)?;
            let subtype = out_type.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
            let (width, height) = out_type
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .map(|packed| ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32))
                .unwrap_or((self.width, self.height));
            if width == 0 || height == 0 {
                return Ok(None);
            }

            let pixel_buf = out_sample.GetBufferByIndex(0)?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut cur_len = 0u32;
            pixel_buf.Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))?;
            let expected = (width * height * 3 / 2) as usize;
            let available = cur_len as usize;

            let result = if available >= expected {
                let pixels = std::slice::from_raw_parts(ptr, expected);
                let rgba = if subtype == MFVideoFormat_NV12 {
                    nv12_to_rgba(pixels, width as usize, height as usize)
                } else if subtype == MFVideoFormat_I420 || subtype == MFVideoFormat_IYUV {
                    i420_to_rgba(pixels, width as usize, height as usize)
                } else {
                    Vec::new()
                };
                if rgba.is_empty() {
                    None
                } else {
                    Some((width as usize, height as usize, rgba))
                }
            } else {
                None
            };

            pixel_buf.Unlock()?;
            Ok(result)
        }
    }

    fn nv12_to_rgba(nv12: &[u8], w: usize, h: usize) -> Vec<u8> {
        let mut out = vec![0u8; w * h * 4];
        let y_plane = &nv12[..w * h];
        let uv_plane = &nv12[w * h..];
        for row in 0..h {
            for col in 0..w {
                let y = y_plane[row * w + col] as i32 - 16;
                let uv_row = row / 2;
                let uv_col = col & !1;
                let u = uv_plane[uv_row * w + uv_col] as i32 - 128;
                let v = uv_plane[uv_row * w + uv_col + 1] as i32 - 128;
                write_rgba_pixel(&mut out, row, col, w, y, u, v);
            }
        }
        out
    }

    fn i420_to_rgba(i420: &[u8], w: usize, h: usize) -> Vec<u8> {
        let mut out = vec![0u8; w * h * 4];
        let y_len = w * h;
        let uv_w = w / 2;
        let uv_len = uv_w * (h / 2);
        let y_plane = &i420[..y_len];
        let u_plane = &i420[y_len..y_len + uv_len];
        let v_plane = &i420[y_len + uv_len..y_len + uv_len * 2];
        for row in 0..h {
            for col in 0..w {
                let y = y_plane[row * w + col] as i32 - 16;
                let uv_idx = (row / 2) * uv_w + (col / 2);
                let u = u_plane[uv_idx] as i32 - 128;
                let v = v_plane[uv_idx] as i32 - 128;
                write_rgba_pixel(&mut out, row, col, w, y, u, v);
            }
        }
        out
    }

    fn write_rgba_pixel(out: &mut [u8], row: usize, col: usize, w: usize, y: i32, u: i32, v: i32) {
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn codec_labels_are_stable() {
            assert_eq!(MfVideoCodec::Vp9.label(), "VP9");
            assert_eq!(MfVideoCodec::H264.label(), "H264");
            assert_eq!(MfVideoCodec::H265.label(), "H265");
            assert_eq!(MfVideoCodec::Av1.label(), "AV1");
        }
    }
}

#[cfg(all(feature = "live-vp9-mf", target_os = "windows"))]
#[allow(unused_imports)]
pub use inner::{
    av1_decode_available, h264_decode_available, h265_decode_available, mf_video_decode_status,
    MfVideoCodec, MfVideoDecodeStatus, MfVideoDecoder,
};

#[cfg(not(all(feature = "live-vp9-mf", target_os = "windows")))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MfVideoCodec {
    H264,
    H265,
    Av1,
}

#[cfg(not(all(feature = "live-vp9-mf", target_os = "windows")))]
impl MfVideoCodec {
    pub fn label(self) -> &'static str {
        match self {
            Self::H264 => "H264",
            Self::H265 => "H265",
            Self::Av1 => "AV1",
        }
    }
}

#[cfg(not(all(feature = "live-vp9-mf", target_os = "windows")))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MfVideoDecodeStatus {
    pub vp9: bool,
    pub h264: bool,
    pub h265: bool,
    pub av1: bool,
}

#[cfg(not(all(feature = "live-vp9-mf", target_os = "windows")))]
impl MfVideoDecodeStatus {
    pub fn label(&self) -> String {
        "Media Foundation decode: Windows only".to_owned()
    }
}

#[cfg(not(all(feature = "live-vp9-mf", target_os = "windows")))]
pub fn mf_video_decode_status() -> &'static MfVideoDecodeStatus {
    static STATUS: MfVideoDecodeStatus = MfVideoDecodeStatus {
        vp9: false,
        h264: false,
        h265: false,
        av1: false,
    };
    &STATUS
}

#[cfg(not(all(feature = "live-vp9-mf", target_os = "windows")))]
pub fn h264_decode_available() -> bool {
    false
}

#[cfg(not(all(feature = "live-vp9-mf", target_os = "windows")))]
pub fn h265_decode_available() -> bool {
    false
}

#[cfg(not(all(feature = "live-vp9-mf", target_os = "windows")))]
pub fn av1_decode_available() -> bool {
    false
}

#[cfg(not(all(feature = "live-vp9-mf", target_os = "windows")))]
pub struct MfVideoDecoder;

#[cfg(not(all(feature = "live-vp9-mf", target_os = "windows")))]
impl MfVideoDecoder {
    pub fn new(_codec: MfVideoCodec, _width: u32, _height: u32) -> Result<Self, String> {
        Err("Media Foundation video decoder is available only on Windows".to_owned())
    }

    pub fn matches(&self, _codec: MfVideoCodec, _width: u32, _height: u32) -> bool {
        false
    }

    pub fn decode_packets<I>(
        &mut self,
        _packets: I,
    ) -> Result<Option<(usize, usize, Vec<u8>)>, String>
    where
        I: IntoIterator,
        I::Item: AsRef<[u8]>,
    {
        Err("Media Foundation video decoder is available only on Windows".to_owned())
    }
}
