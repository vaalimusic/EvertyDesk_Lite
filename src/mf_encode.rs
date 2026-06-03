/// Windows Media Foundation video encoder backend.
///
/// This backend does not spawn any external encoder process. It feeds
/// BGRA screen frames into a local NV12 buffer, calls Media Foundation MFT
/// encoders directly, and returns elementary H.264/H.265 access units for the
/// existing RustDesk-compatible video message path.
#[cfg(all(feature = "live-vp9-mf", target_os = "windows"))]
mod inner {
    use std::{mem::ManuallyDrop, sync::OnceLock};

    use crate::nvenc::{NvencCodec, NvencPacket};
    use windows::{
        core::{ComInterface, Result as WResult, GUID},
        Win32::{
            Foundation::{E_FAIL, VARIANT_TRUE},
            Media::MediaFoundation::{
                eAVEncCommonRateControlMode_LowDelayVBR, CODECAPI_AVEncCommonLowLatency,
                CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonQualityVsSpeed,
                CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncMPVGOPSize,
                CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVLowLatencyMode, ICodecAPI,
                IMFActivate, IMFSample, IMFTransform, MFCreateMediaType, MFCreateMemoryBuffer,
                MFCreateSample, MFMediaType_Video, MFStartup, MFTEnumEx, MFVideoFormat_H264,
                MFVideoFormat_H264_ES, MFVideoFormat_H265, MFVideoFormat_HEVC,
                MFVideoFormat_HEVC_ES, MFVideoFormat_NV12, MFSTARTUP_NOSOCKET,
                MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_HARDWARE,
                MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT,
                MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
                MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_INFO, MFT_REGISTER_TYPE_INFO,
                MF_E_TRANSFORM_NEED_MORE_INPUT, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE,
                MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
                MF_TRANSFORM_ASYNC_UNLOCK, MF_VERSION,
            },
            System::Com::{
                CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED, VARIANT, VARIANT_0,
                VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_UI4,
            },
        },
    };

    const MF_E_TRANSFORM_STREAM_CHANGE: i32 = 0xC00D6D61u32 as i32;
    const MFT_OUTPUT_STREAM_PROVIDES_SAMPLES_FLAG: u32 = 0x100;
    const HNS_PER_SECOND: i64 = 10_000_000;

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct MfEncoderStatus {
        pub h264: bool,
        pub h265: bool,
        pub hardware_h264: bool,
        pub hardware_h265: bool,
    }

    impl MfEncoderStatus {
        pub fn label(&self) -> String {
            let mut codecs = Vec::new();
            if self.h264 {
                codecs.push(if self.hardware_h264 {
                    "H264(hw)"
                } else {
                    "H264"
                });
            }
            if self.h265 {
                codecs.push(if self.hardware_h265 {
                    "H265(hw)"
                } else {
                    "H265"
                });
            }
            if codecs.is_empty() {
                "Media Foundation encode: unavailable".to_owned()
            } else {
                format!("Media Foundation encode: {}", codecs.join(", "))
            }
        }
    }

    pub fn mf_encoder_status() -> &'static MfEncoderStatus {
        static STATUS: OnceLock<MfEncoderStatus> = OnceLock::new();
        STATUS.get_or_init(|| MfEncoderStatus {
            h264: encoder_available(NvencCodec::H264, false),
            h265: encoder_available(NvencCodec::H265, false),
            hardware_h264: encoder_available(NvencCodec::H264, true),
            hardware_h265: encoder_available(NvencCodec::H265, true),
        })
    }

    pub fn mf_encoder_codecs() -> Vec<NvencCodec> {
        let status = mf_encoder_status();
        let mut codecs = Vec::new();
        if status.h265 {
            codecs.push(NvencCodec::H265);
        }
        if status.h264 {
            codecs.push(NvencCodec::H264);
        }
        codecs
    }

    pub struct MfVideoEncoder {
        codec: NvencCodec,
        transform: IMFTransform,
        source_width: u32,
        source_height: u32,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
        provides_samples: bool,
        output_buf_size: u32,
        nv12: Vec<u8>,
        frame_index: u64,
        first_packet: bool,
    }

    unsafe impl Send for MfVideoEncoder {}

    impl MfVideoEncoder {
        pub fn new(
            codec: NvencCodec,
            width: u32,
            height: u32,
            fps: u32,
            bitrate: u32,
        ) -> Result<Self, String> {
            if codec == NvencCodec::Av1 {
                return Err("Media Foundation AV1 encoder is not wired yet".to_owned());
            }
            let fps = fps.clamp(5, 60);
            unsafe {
                Self::create(codec, width.max(2), height.max(2), fps, bitrate)
                    .map_err(|err| format!("MF encode {}: {err}", codec.label()))
            }
        }

        pub fn matches(&self, codec: NvencCodec, width: u32, height: u32, fps: u32) -> bool {
            self.codec == codec
                && self.source_width == width.max(2)
                && self.source_height == height.max(2)
                && self.fps == fps.clamp(5, 60)
        }

        pub fn encode_bgra(
            &mut self,
            bgra: &[u8],
            force_key: bool,
        ) -> Result<Option<NvencPacket>, String> {
            let expected = self
                .source_width
                .saturating_mul(self.source_height)
                .saturating_mul(4) as usize;
            if bgra.len() < expected {
                return Ok(None);
            }

            bgra_to_nv12(
                &mut self.nv12,
                self.width as usize,
                self.height as usize,
                self.source_width as usize,
                self.source_height as usize,
                bgra,
            );
            unsafe {
                if force_key {
                    request_keyframe(&self.transform);
                }
                self.run()
                    .map_err(|err| format!("MF encode {}: {err}", self.codec.label()))
            }
        }

        unsafe fn create(
            codec: NvencCodec,
            width: u32,
            height: u32,
            fps: u32,
            bitrate: u32,
        ) -> WResult<Self> {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);
            let source_width = width;
            let source_height = height;
            let width = width.next_multiple_of(2);
            let height = height.next_multiple_of(2);

            let mut last_error = None;
            for prefer_hardware in [true, false] {
                for output_subtype in codec_output_subtypes(codec) {
                    let input = MFT_REGISTER_TYPE_INFO {
                        guidMajorType: MFMediaType_Video,
                        guidSubtype: MFVideoFormat_NV12,
                    };
                    let output = MFT_REGISTER_TYPE_INFO {
                        guidMajorType: MFMediaType_Video,
                        guidSubtype: *output_subtype,
                    };
                    let mut count = 0u32;
                    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
                    let flags = enum_flags(prefer_hardware);
                    let enum_result = MFTEnumEx(
                        MFT_CATEGORY_VIDEO_ENCODER,
                        flags,
                        Some(&input),
                        Some(&output),
                        &mut activates,
                        &mut count,
                    );
                    if let Err(err) = enum_result {
                        last_error = Some(err);
                        continue;
                    }
                    if count == 0 || activates.is_null() {
                        if !activates.is_null() {
                            CoTaskMemFree(Some(activates as *const _));
                        }
                        continue;
                    }

                    let slice = std::slice::from_raw_parts(activates, count as usize);
                    for activate in slice.iter().filter_map(Option::as_ref) {
                        let transform: IMFTransform = match activate.ActivateObject() {
                            Ok(transform) => transform,
                            Err(err) => {
                                last_error = Some(err);
                                continue;
                            }
                        };
                        if let Ok(attrs) = transform.GetAttributes() {
                            let _ = attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1);
                        }
                        match configure_transform(
                            &transform,
                            codec,
                            *output_subtype,
                            width,
                            height,
                            fps,
                            bitrate,
                        ) {
                            Ok(info) => {
                                CoTaskMemFree(Some(activates as *const _));
                                eprintln!(
                                    "[mf-encode] {} encoder started at {}x{}@{} ({})",
                                    codec.label(),
                                    width,
                                    height,
                                    fps,
                                    if prefer_hardware {
                                        "hardware preferred"
                                    } else {
                                        "sync"
                                    }
                                );
                                return Ok(Self {
                                    codec,
                                    transform,
                                    source_width,
                                    source_height,
                                    width,
                                    height,
                                    fps,
                                    bitrate,
                                    provides_samples: info.provides_samples,
                                    output_buf_size: info.output_buf_size,
                                    nv12: Vec::new(),
                                    frame_index: 0,
                                    first_packet: true,
                                });
                            }
                            Err(err) => last_error = Some(err),
                        }
                    }
                    CoTaskMemFree(Some(activates as *const _));
                }
            }

            Err(last_error.unwrap_or_else(|| windows::core::Error::from(E_FAIL)))
        }

        unsafe fn run(&mut self) -> WResult<Option<NvencPacket>> {
            let mem_buf = MFCreateMemoryBuffer(self.nv12.len() as u32)?;
            {
                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut max_len = 0u32;
                let mut cur_len = 0u32;
                mem_buf.Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))?;
                std::ptr::copy_nonoverlapping(self.nv12.as_ptr(), ptr, self.nv12.len());
                mem_buf.Unlock()?;
                mem_buf.SetCurrentLength(self.nv12.len() as u32)?;
            }

            let sample = MFCreateSample()?;
            sample.AddBuffer(&mem_buf)?;
            let duration = HNS_PER_SECOND / i64::from(self.fps.max(1));
            sample.SetSampleTime(self.frame_index as i64 * duration)?;
            sample.SetSampleDuration(duration)?;
            self.transform.ProcessInput(0, &sample, 0)?;
            self.frame_index = self.frame_index.saturating_add(1);

            let mut latest = None;
            let mut retries = 0u32;
            loop {
                match self.process_output()? {
                    Some(packet) => latest = Some(packet),
                    None => return Ok(latest),
                }
                retries += 1;
                if retries > 8 {
                    return Ok(latest);
                }
            }
        }

        unsafe fn process_output(&mut self) -> WResult<Option<NvencPacket>> {
            let pre_sample: Option<IMFSample> = if self.provides_samples {
                None
            } else {
                let sz = self.output_buf_size.max(self.bitrate / 8).max(64 * 1024);
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
                    self.extract_packet(out_sample)
                }
                Err(err) if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                    std::mem::ManuallyDrop::drop(&mut out_buf.pSample);
                    Ok(None)
                }
                Err(err) if err.code().0 == MF_E_TRANSFORM_STREAM_CHANGE => {
                    std::mem::ManuallyDrop::drop(&mut out_buf.pSample);
                    let info = refresh_output_info(&self.transform)?;
                    self.provides_samples = info.provides_samples;
                    self.output_buf_size = info.output_buf_size;
                    Ok(None)
                }
                Err(err) => {
                    std::mem::ManuallyDrop::drop(&mut out_buf.pSample);
                    Err(err)
                }
            }
        }

        unsafe fn extract_packet(&mut self, sample: IMFSample) -> WResult<Option<NvencPacket>> {
            let buf = sample.ConvertToContiguousBuffer()?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut cur_len = 0u32;
            buf.Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))?;
            let bytes = if cur_len == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(ptr, cur_len as usize).to_vec()
            };
            buf.Unlock()?;

            if bytes.is_empty() {
                return Ok(None);
            }

            let bytes = normalize_h26x_access_unit(&bytes);
            let key = self.first_packet || h26x_is_key(self.codec, &bytes);
            self.first_packet = false;
            Ok(Some(NvencPacket {
                codec: self.codec,
                bytes,
                key,
            }))
        }
    }

    struct OutputInfo {
        provides_samples: bool,
        output_buf_size: u32,
    }

    unsafe fn configure_transform(
        transform: &IMFTransform,
        _codec: NvencCodec,
        output_subtype: GUID,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
    ) -> WResult<OutputInfo> {
        let out_type = MFCreateMediaType()?;
        out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out_type.SetGUID(&MF_MT_SUBTYPE, &output_subtype)?;
        out_type.SetUINT64(&MF_MT_FRAME_SIZE, pack_ratio(width, height))?;
        out_type.SetUINT64(&MF_MT_FRAME_RATE, pack_ratio(fps, 1))?;
        out_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_ratio(1, 1))?;
        out_type.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
        transform.SetOutputType(0, &out_type, 0)?;

        let in_type = MFCreateMediaType()?;
        in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        in_type.SetUINT64(&MF_MT_FRAME_SIZE, pack_ratio(width, height))?;
        in_type.SetUINT64(&MF_MT_FRAME_RATE, pack_ratio(fps, 1))?;
        in_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_ratio(1, 1))?;
        transform.SetInputType(0, &in_type, 0)?;

        let tuning = tune_codec_api(transform, fps, bitrate);
        if !tuning.is_empty() {
            eprintln!("[mf-encode] CodecAPI tuning: {}", tuning.join(", "));
        }

        let _ = transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
        let _ = transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
        refresh_output_info(transform)
    }

    unsafe fn refresh_output_info(transform: &IMFTransform) -> WResult<OutputInfo> {
        let info: MFT_OUTPUT_STREAM_INFO = transform.GetOutputStreamInfo(0)?;
        Ok(OutputInfo {
            provides_samples: (info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES_FLAG) != 0,
            output_buf_size: info.cbSize,
        })
    }

    fn encoder_available(codec: NvencCodec, hardware_only: bool) -> bool {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);

            let input = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: MFVideoFormat_NV12,
            };
            for output_subtype in codec_output_subtypes(codec) {
                let output = MFT_REGISTER_TYPE_INFO {
                    guidMajorType: MFMediaType_Video,
                    guidSubtype: *output_subtype,
                };
                let mut count = 0u32;
                let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
                let result = MFTEnumEx(
                    MFT_CATEGORY_VIDEO_ENCODER,
                    enum_flags(hardware_only),
                    Some(&input),
                    Some(&output),
                    &mut activates,
                    &mut count,
                );
                let found = result.is_ok() && count > 0 && !activates.is_null();
                if !activates.is_null() {
                    CoTaskMemFree(Some(activates as *const _));
                }
                if found {
                    return true;
                }
            }
            false
        }
    }

    fn enum_flags(prefer_hardware: bool) -> MFT_ENUM_FLAG {
        let mut bits = MFT_ENUM_FLAG_SORTANDFILTER.0;
        if prefer_hardware {
            bits |= MFT_ENUM_FLAG_HARDWARE.0;
        } else {
            bits |= MFT_ENUM_FLAG_SYNCMFT.0;
        }
        MFT_ENUM_FLAG(bits)
    }

    fn codec_output_subtypes(codec: NvencCodec) -> &'static [GUID] {
        match codec {
            NvencCodec::H264 => &[MFVideoFormat_H264, MFVideoFormat_H264_ES],
            NvencCodec::H265 => &[
                MFVideoFormat_HEVC,
                MFVideoFormat_H265,
                MFVideoFormat_HEVC_ES,
            ],
            NvencCodec::Av1 => &[],
        }
    }

    #[inline]
    fn pack_ratio(numerator: u32, denominator: u32) -> u64 {
        (u64::from(numerator) << 32) | u64::from(denominator)
    }

    unsafe fn tune_codec_api(
        transform: &IMFTransform,
        fps: u32,
        bitrate: u32,
    ) -> Vec<&'static str> {
        let Ok(codec_api) = transform.cast::<ICodecAPI>() else {
            return Vec::new();
        };
        let mut applied = Vec::new();

        let low_latency = set_codec_api_bool(&codec_api, &CODECAPI_AVEncCommonLowLatency, true)
            || set_codec_api_bool(&codec_api, &CODECAPI_AVLowLatencyMode, true);
        if low_latency {
            applied.push("low-latency");
        }
        if set_codec_api_u32(
            &codec_api,
            &CODECAPI_AVEncCommonRateControlMode,
            eAVEncCommonRateControlMode_LowDelayVBR.0 as u32,
        ) {
            applied.push("low-delay-vbr");
        }
        if set_codec_api_u32(&codec_api, &CODECAPI_AVEncCommonMeanBitRate, bitrate) {
            applied.push("bitrate");
        }
        if set_codec_api_u32(
            &codec_api,
            &CODECAPI_AVEncMPVGOPSize,
            fps.clamp(5, 60).saturating_mul(2),
        ) {
            applied.push("gop");
        }
        if set_codec_api_u32(&codec_api, &CODECAPI_AVEncCommonQualityVsSpeed, 100) {
            applied.push("speed");
        }

        applied
    }

    unsafe fn request_keyframe(transform: &IMFTransform) {
        if let Ok(codec_api) = transform.cast::<ICodecAPI>() {
            let _ = set_codec_api_bool(&codec_api, &CODECAPI_AVEncVideoForceKeyFrame, true);
        }
    }

    unsafe fn set_codec_api_bool(codec_api: &ICodecAPI, api: &GUID, value: bool) -> bool {
        if codec_api.IsSupported(api as *const _).is_err()
            || codec_api.IsModifiable(api as *const _).is_err()
        {
            return false;
        }
        let variant = variant_bool(value);
        codec_api
            .SetValue(api as *const _, &variant as *const _)
            .is_ok()
    }

    unsafe fn set_codec_api_u32(codec_api: &ICodecAPI, api: &GUID, value: u32) -> bool {
        if codec_api.IsSupported(api as *const _).is_err()
            || codec_api.IsModifiable(api as *const _).is_err()
        {
            return false;
        }
        let variant = variant_u32(value);
        codec_api
            .SetValue(api as *const _, &variant as *const _)
            .is_ok()
    }

    fn variant_bool(value: bool) -> VARIANT {
        VARIANT {
            Anonymous: VARIANT_0 {
                Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                    vt: VT_BOOL,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: VARIANT_0_0_0 {
                        boolVal: if value {
                            VARIANT_TRUE
                        } else {
                            Default::default()
                        },
                    },
                }),
            },
        }
    }

    fn variant_u32(value: u32) -> VARIANT {
        VARIANT {
            Anonymous: VARIANT_0 {
                Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                    vt: VT_UI4,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: VARIANT_0_0_0 { ulVal: value },
                }),
            },
        }
    }

    fn bgra_to_nv12(
        out: &mut Vec<u8>,
        dst_width: usize,
        dst_height: usize,
        src_width: usize,
        src_height: usize,
        bgra: &[u8],
    ) {
        let y_len = dst_width * dst_height;
        out.resize(y_len + y_len / 2, 0);

        for by in (0..dst_height).step_by(2) {
            for bx in (0..dst_width).step_by(2) {
                let mut r_sum = 0_i32;
                let mut g_sum = 0_i32;
                let mut b_sum = 0_i32;

                for dy in 0..2 {
                    let y = by + dy;
                    for dx in 0..2 {
                        let x = bx + dx;
                        let (r, g, b) = bgra_pixel_rgb_clamped(bgra, src_width, src_height, x, y);
                        out[y * dst_width + x] = y_from_rgb(r, g, b);
                        r_sum += r;
                        g_sum += g;
                        b_sum += b;
                    }
                }

                let r = (r_sum + 2) / 4;
                let g = (g_sum + 2) / 4;
                let b = (b_sum + 2) / 4;
                let uv = y_len + (by / 2) * dst_width + bx;
                out[uv] = u_from_rgb(r, g, b);
                out[uv + 1] = v_from_rgb(r, g, b);
            }
        }
    }

    #[inline(always)]
    fn bgra_pixel_rgb_clamped(
        bgra: &[u8],
        width: usize,
        height: usize,
        x: usize,
        y: usize,
    ) -> (i32, i32, i32) {
        let sx = x.min(width - 1);
        let sy = y.min(height - 1);
        let base = (sy * width + sx) * 4;
        (
            bgra[base + 2] as i32,
            bgra[base + 1] as i32,
            bgra[base] as i32,
        )
    }

    #[inline(always)]
    fn y_from_rgb(r: i32, g: i32, b: i32) -> u8 {
        (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(16, 235) as u8
    }

    #[inline(always)]
    fn u_from_rgb(r: i32, g: i32, b: i32) -> u8 {
        (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(16, 240) as u8
    }

    #[inline(always)]
    fn v_from_rgb(r: i32, g: i32, b: i32) -> u8 {
        (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(16, 240) as u8
    }

    fn normalize_h26x_access_unit(bytes: &[u8]) -> Vec<u8> {
        if has_start_code(bytes) {
            return bytes.to_vec();
        }
        length_prefixed_to_annex_b(bytes, 4)
            .or_else(|| length_prefixed_to_annex_b(bytes, 2))
            .unwrap_or_else(|| bytes.to_vec())
    }

    fn length_prefixed_to_annex_b(bytes: &[u8], length_size: usize) -> Option<Vec<u8>> {
        let mut pos = 0usize;
        let mut out = Vec::with_capacity(bytes.len() + 16);
        while pos + length_size <= bytes.len() {
            let len = match length_size {
                4 => u32::from_be_bytes(bytes[pos..pos + 4].try_into().ok()?) as usize,
                2 => u16::from_be_bytes(bytes[pos..pos + 2].try_into().ok()?) as usize,
                _ => return None,
            };
            pos += length_size;
            if len == 0 || pos + len > bytes.len() {
                return None;
            }
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(&bytes[pos..pos + len]);
            pos += len;
        }
        if pos == bytes.len() && !out.is_empty() {
            Some(out)
        } else {
            None
        }
    }

    fn has_start_code(bytes: &[u8]) -> bool {
        bytes.starts_with(&[0, 0, 1]) || bytes.starts_with(&[0, 0, 0, 1])
    }

    fn h26x_is_key(codec: NvencCodec, bytes: &[u8]) -> bool {
        let mut pos = 0usize;
        while let Some((nal_start, nal_end)) = next_annex_b_nal(bytes, pos) {
            let nal = &bytes[nal_start..nal_end];
            if !nal.is_empty() {
                match codec {
                    NvencCodec::H264 if nal[0] & 0x1F == 5 => return true,
                    NvencCodec::H265 => {
                        let nal_type = (nal[0] >> 1) & 0x3F;
                        if matches!(nal_type, 19 | 20 | 21) {
                            return true;
                        }
                    }
                    _ => {}
                }
            }
            pos = nal_end;
        }
        false
    }

    fn next_annex_b_nal(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
        let start = find_start_code(bytes, from)?;
        let nal_start = if bytes[start..].starts_with(&[0, 0, 0, 1]) {
            start + 4
        } else {
            start + 3
        };
        let next = find_start_code(bytes, nal_start).unwrap_or(bytes.len());
        Some((nal_start, next))
    }

    fn find_start_code(bytes: &[u8], from: usize) -> Option<usize> {
        if bytes.len() < 3 || from >= bytes.len().saturating_sub(2) {
            return None;
        }
        let mut i = from;
        while i + 3 <= bytes.len() {
            if bytes[i..].starts_with(&[0, 0, 1])
                || (i + 4 <= bytes.len() && bytes[i..].starts_with(&[0, 0, 0, 1]))
            {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn length_prefixed_h264_is_converted_to_annex_b() {
            let bytes = [0, 0, 0, 2, 0x65, 0x88, 0, 0, 0, 1, 0x06];
            let converted = normalize_h26x_access_unit(&bytes);
            assert_eq!(&converted[..4], &[0, 0, 0, 1]);
            assert!(h26x_is_key(NvencCodec::H264, &converted));
        }

        #[test]
        fn status_label_is_stable() {
            let status = MfEncoderStatus {
                h264: true,
                h265: false,
                hardware_h264: true,
                hardware_h265: false,
            };
            assert_eq!(status.label(), "Media Foundation encode: H264(hw)");
        }
    }
}

#[cfg(all(feature = "live-vp9-mf", target_os = "windows"))]
pub use inner::{mf_encoder_codecs, mf_encoder_status, MfVideoEncoder};

#[cfg(not(all(feature = "live-vp9-mf", target_os = "windows")))]
mod fallback {
    use crate::nvenc::{NvencCodec, NvencPacket};

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct MfEncoderStatus;

    impl MfEncoderStatus {
        pub fn label(&self) -> String {
            "Media Foundation encode: Windows only".to_owned()
        }
    }

    pub fn mf_encoder_status() -> &'static MfEncoderStatus {
        static STATUS: MfEncoderStatus = MfEncoderStatus;
        &STATUS
    }

    pub fn mf_encoder_codecs() -> Vec<NvencCodec> {
        Vec::new()
    }

    pub struct MfVideoEncoder;

    impl MfVideoEncoder {
        pub fn new(
            _codec: NvencCodec,
            _width: u32,
            _height: u32,
            _fps: u32,
            _bitrate: u32,
        ) -> Result<Self, String> {
            Err("Media Foundation encoder is Windows only".to_owned())
        }

        pub fn matches(&self, _codec: NvencCodec, _width: u32, _height: u32, _fps: u32) -> bool {
            false
        }

        pub fn encode_bgra(
            &mut self,
            _bgra: &[u8],
            _force_key: bool,
        ) -> Result<Option<NvencPacket>, String> {
            Err("Media Foundation encoder is Windows only".to_owned())
        }
    }
}

#[cfg(not(all(feature = "live-vp9-mf", target_os = "windows")))]
pub use fallback::{mf_encoder_codecs, mf_encoder_status, MfVideoEncoder};
