#[cfg(target_os = "macos")]
mod macos {
    use std::{ffi::c_void, ptr, slice, sync::Mutex};

    use crate::nvenc::{NvencCodec, NvencPacket};

    type Boolean = u8;
    type CFIndex = isize;
    type CFAllocatorRef = *const c_void;
    type CFArrayRef = *const c_void;
    type CFBooleanRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFNumberRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFTypeRef = *const c_void;
    type CMBlockBufferRef = *const c_void;
    type CMFormatDescriptionRef = *const c_void;
    type CMSampleBufferRef = *const c_void;
    type CVPixelBufferRef = *mut c_void;
    type OSStatus = i32;
    type OSType = u32;
    type VTCompressionSessionRef = *mut c_void;
    type VTEncodeInfoFlags = u32;

    const K_CM_VIDEO_CODEC_TYPE_H264: OSType = 0x6176_6331; // 'avc1'
    const K_CV_PIXEL_FORMAT_TYPE_32_BGRA: OSType = 0x4247_5241; // 'BGRA'
    const K_CF_NUMBER_SINT32_TYPE: i32 = 3;
    const K_CM_TIME_FLAGS_VALID: u32 = 1;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CMTime {
        value: i64,
        timescale: i32,
        flags: u32,
        epoch: i64,
    }

    impl CMTime {
        fn invalid() -> Self {
            Self {
                value: 0,
                timescale: 0,
                flags: 0,
                epoch: 0,
            }
        }

        fn frame(index: i64, fps: u32) -> Self {
            Self {
                value: index,
                timescale: fps.max(1) as i32,
                flags: K_CM_TIME_FLAGS_VALID,
                epoch: 0,
            }
        }
    }

    type VTCompressionOutputCallback = unsafe extern "C" fn(
        output_callback_refcon: *mut c_void,
        source_frame_refcon: *mut c_void,
        status: OSStatus,
        info_flags: VTEncodeInfoFlags,
        sample_buffer: CMSampleBufferRef,
    );

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: CFBooleanRef;
        static kCFBooleanFalse: CFBooleanRef;

        fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
        fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> *const c_void;
        fn CFBooleanGetValue(boolean: CFBooleanRef) -> Boolean;
        fn CFDictionaryCreate(
            allocator: CFAllocatorRef,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: CFIndex,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> CFDictionaryRef;
        fn CFDictionaryGetValue(the_dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
        fn CFNumberCreate(
            allocator: CFAllocatorRef,
            the_type: i32,
            value_ptr: *const c_void,
        ) -> CFNumberRef;
        fn CFRelease(cf: CFTypeRef);
    }

    #[link(name = "CoreMedia", kind = "framework")]
    extern "C" {
        static kCMSampleAttachmentKey_NotSync: CFStringRef;

        fn CMBlockBufferCopyDataBytes(
            the_buffer: CMBlockBufferRef,
            offset_to_data: usize,
            data_length: usize,
            destination: *mut c_void,
        ) -> OSStatus;
        fn CMBlockBufferGetDataLength(the_buffer: CMBlockBufferRef) -> usize;
        fn CMSampleBufferDataIsReady(sbuf: CMSampleBufferRef) -> Boolean;
        fn CMSampleBufferGetDataBuffer(sbuf: CMSampleBufferRef) -> CMBlockBufferRef;
        fn CMSampleBufferGetFormatDescription(sbuf: CMSampleBufferRef) -> CMFormatDescriptionRef;
        fn CMSampleBufferGetSampleAttachmentsArray(
            sbuf: CMSampleBufferRef,
            create_if_necessary: Boolean,
        ) -> CFArrayRef;
        fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            video_desc: CMFormatDescriptionRef,
            parameter_set_index: usize,
            parameter_set_pointer_out: *mut *const u8,
            parameter_set_size_out: *mut usize,
            parameter_set_count_out: *mut usize,
            nal_unit_header_length_out: *mut i32,
        ) -> OSStatus;
    }

    #[link(name = "CoreVideo", kind = "framework")]
    extern "C" {
        fn CVPixelBufferCreate(
            allocator: CFAllocatorRef,
            width: usize,
            height: usize,
            pixel_format_type: OSType,
            pixel_buffer_attributes: CFDictionaryRef,
            pixel_buffer_out: *mut CVPixelBufferRef,
        ) -> OSStatus;
        fn CVPixelBufferGetBaseAddress(pixel_buffer: CVPixelBufferRef) -> *mut c_void;
        fn CVPixelBufferGetBytesPerRow(pixel_buffer: CVPixelBufferRef) -> usize;
        fn CVPixelBufferLockBaseAddress(
            pixel_buffer: CVPixelBufferRef,
            lock_flags: u64,
        ) -> OSStatus;
        fn CVPixelBufferUnlockBaseAddress(
            pixel_buffer: CVPixelBufferRef,
            unlock_flags: u64,
        ) -> OSStatus;
    }

    #[link(name = "VideoToolbox", kind = "framework")]
    extern "C" {
        static kVTCompressionPropertyKey_AllowFrameReordering: CFStringRef;
        static kVTCompressionPropertyKey_AverageBitRate: CFStringRef;
        static kVTCompressionPropertyKey_ExpectedFrameRate: CFStringRef;
        static kVTCompressionPropertyKey_MaxFrameDelayCount: CFStringRef;
        static kVTCompressionPropertyKey_MaxKeyFrameInterval: CFStringRef;
        static kVTCompressionPropertyKey_ProfileLevel: CFStringRef;
        static kVTCompressionPropertyKey_RealTime: CFStringRef;
        static kVTEncodeFrameOptionKey_ForceKeyFrame: CFStringRef;
        static kVTProfileLevel_H264_Baseline_AutoLevel: CFStringRef;
        static kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: CFStringRef;

        fn VTCompressionSessionCompleteFrames(
            session: VTCompressionSessionRef,
            complete_until_presentation_time_stamp: CMTime,
        ) -> OSStatus;
        fn VTCompressionSessionCreate(
            allocator: CFAllocatorRef,
            width: i32,
            height: i32,
            codec_type: OSType,
            encoder_specification: CFDictionaryRef,
            source_image_buffer_attributes: CFDictionaryRef,
            compressed_data_allocator: CFAllocatorRef,
            output_callback: Option<VTCompressionOutputCallback>,
            output_callback_refcon: *mut c_void,
            compression_session_out: *mut VTCompressionSessionRef,
        ) -> OSStatus;
        fn VTCompressionSessionEncodeFrame(
            session: VTCompressionSessionRef,
            image_buffer: CVPixelBufferRef,
            presentation_time_stamp: CMTime,
            duration: CMTime,
            frame_properties: CFDictionaryRef,
            source_frame_refcon: *mut c_void,
            info_flags_out: *mut VTEncodeInfoFlags,
        ) -> OSStatus;
        fn VTCompressionSessionInvalidate(session: VTCompressionSessionRef);
        fn VTCompressionSessionPrepareToEncodeFrames(session: VTCompressionSessionRef) -> OSStatus;
        fn VTSessionSetProperty(
            session: CFTypeRef,
            property_key: CFStringRef,
            property_value: CFTypeRef,
        ) -> OSStatus;
    }

    struct PacketSink {
        packets: Mutex<Vec<NvencPacket>>,
    }

    pub struct VideoToolboxEncoder {
        codec: NvencCodec,
        width: u32,
        height: u32,
        fps: u32,
        session: VTCompressionSessionRef,
        sink: Box<PacketSink>,
        frame_index: i64,
    }

    unsafe impl Send for VideoToolboxEncoder {}

    impl VideoToolboxEncoder {
        pub fn new(
            codec: NvencCodec,
            width: u32,
            height: u32,
            fps: u32,
            bitrate: u32,
        ) -> Result<Self, String> {
            if codec != NvencCodec::H264 {
                return Err(format!(
                    "VideoToolbox direct backend currently exposes H264 only, requested {}",
                    codec.label()
                ));
            }

            let width = width.max(2);
            let height = height.max(2);
            let fps = fps.clamp(5, 60);
            unsafe { Self::create(codec, width, height, fps, bitrate) }
        }

        pub fn matches(&self, codec: NvencCodec, width: u32, height: u32, fps: u32) -> bool {
            self.codec == codec
                && self.width == width.max(2)
                && self.height == height.max(2)
                && self.fps == fps.clamp(5, 60)
        }

        pub fn encode_bgra(
            &mut self,
            bgra: &[u8],
            force_key: bool,
        ) -> Result<Option<NvencPacket>, String> {
            let expected = (self.width as usize)
                .checked_mul(self.height as usize)
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| "VideoToolbox frame size overflow".to_owned())?;
            if bgra.len() != expected {
                return Err(format!(
                    "VideoToolbox frame size mismatch: got {}, expected {expected}",
                    bgra.len()
                ));
            }

            self.sink
                .packets
                .lock()
                .map_err(|_| "VideoToolbox packet sink poisoned".to_owned())?
                .clear();

            unsafe {
                let pixel_buffer = create_bgra_pixel_buffer(self.width, self.height, bgra)?;
                let pts = CMTime::frame(self.frame_index, self.fps);
                let duration = CMTime::frame(1, self.fps);
                let frame_properties = if force_key || self.frame_index == 0 {
                    create_force_keyframe_dictionary()
                } else {
                    ptr::null()
                };
                let mut info_flags = 0;
                let status = VTCompressionSessionEncodeFrame(
                    self.session,
                    pixel_buffer,
                    pts,
                    duration,
                    frame_properties,
                    ptr::null_mut(),
                    &mut info_flags,
                );
                if !frame_properties.is_null() {
                    CFRelease(frame_properties as CFTypeRef);
                }
                CFRelease(pixel_buffer as CFTypeRef);
                if status != 0 {
                    return Err(format!("VTCompressionSessionEncodeFrame status={status}"));
                }

                let status = VTCompressionSessionCompleteFrames(self.session, pts);
                if status != 0 {
                    return Err(format!(
                        "VTCompressionSessionCompleteFrames status={status}"
                    ));
                }
            }

            self.frame_index += 1;
            let mut packets = self
                .sink
                .packets
                .lock()
                .map_err(|_| "VideoToolbox packet sink poisoned".to_owned())?;
            if packets.len() > 1 {
                Ok(Some(packets.remove(0)))
            } else {
                Ok(packets.pop())
            }
        }

        unsafe fn create(
            codec: NvencCodec,
            width: u32,
            height: u32,
            fps: u32,
            bitrate: u32,
        ) -> Result<Self, String> {
            let mut sink = Box::new(PacketSink {
                packets: Mutex::new(Vec::new()),
            });
            let encoder_spec = create_hardware_encoder_specification();
            let mut session = ptr::null_mut();
            let status = VTCompressionSessionCreate(
                ptr::null(),
                width as i32,
                height as i32,
                K_CM_VIDEO_CODEC_TYPE_H264,
                encoder_spec,
                ptr::null(),
                ptr::null(),
                Some(compression_output_callback),
                (&mut *sink as *mut PacketSink).cast(),
                &mut session,
            );
            if !encoder_spec.is_null() {
                CFRelease(encoder_spec as CFTypeRef);
            }
            if status != 0 || session.is_null() {
                return Err(format!("VTCompressionSessionCreate status={status}"));
            }

            set_bool_property(session, kVTCompressionPropertyKey_RealTime, true);
            set_bool_property(
                session,
                kVTCompressionPropertyKey_AllowFrameReordering,
                false,
            );
            set_i32_property(
                session,
                kVTCompressionPropertyKey_AverageBitRate,
                bitrate as i32,
            );
            set_i32_property(
                session,
                kVTCompressionPropertyKey_ExpectedFrameRate,
                fps as i32,
            );
            set_i32_property(
                session,
                kVTCompressionPropertyKey_MaxKeyFrameInterval,
                (fps * 2) as i32,
            );
            set_i32_property(session, kVTCompressionPropertyKey_MaxFrameDelayCount, 1);
            let _ = VTSessionSetProperty(
                session as CFTypeRef,
                kVTCompressionPropertyKey_ProfileLevel,
                kVTProfileLevel_H264_Baseline_AutoLevel as CFTypeRef,
            );

            let status = VTCompressionSessionPrepareToEncodeFrames(session);
            if status != 0 {
                VTCompressionSessionInvalidate(session);
                CFRelease(session as CFTypeRef);
                return Err(format!(
                    "VTCompressionSessionPrepareToEncodeFrames status={status}"
                ));
            }

            Ok(Self {
                codec,
                width,
                height,
                fps,
                session,
                sink,
                frame_index: 0,
            })
        }
    }

    impl Drop for VideoToolboxEncoder {
        fn drop(&mut self) {
            unsafe {
                if !self.session.is_null() {
                    let _ = VTCompressionSessionCompleteFrames(self.session, CMTime::invalid());
                    VTCompressionSessionInvalidate(self.session);
                    CFRelease(self.session as CFTypeRef);
                    self.session = ptr::null_mut();
                }
            }
        }
    }

    pub fn videotoolbox_supported_platform() -> bool {
        true
    }

    pub fn videotoolbox_codecs() -> Vec<NvencCodec> {
        vec![NvencCodec::H264]
    }

    pub fn videotoolbox_encoder_names() -> Option<Vec<String>> {
        Some(vec!["H264 VideoToolbox".to_owned()])
    }

    unsafe fn create_bgra_pixel_buffer(
        width: u32,
        height: u32,
        bgra: &[u8],
    ) -> Result<CVPixelBufferRef, String> {
        let mut pixel_buffer = ptr::null_mut();
        let status = CVPixelBufferCreate(
            ptr::null(),
            width as usize,
            height as usize,
            K_CV_PIXEL_FORMAT_TYPE_32_BGRA,
            ptr::null(),
            &mut pixel_buffer,
        );
        if status != 0 || pixel_buffer.is_null() {
            return Err(format!("CVPixelBufferCreate status={status}"));
        }

        let status = CVPixelBufferLockBaseAddress(pixel_buffer, 0);
        if status != 0 {
            CFRelease(pixel_buffer as CFTypeRef);
            return Err(format!("CVPixelBufferLockBaseAddress status={status}"));
        }

        let base = CVPixelBufferGetBaseAddress(pixel_buffer) as *mut u8;
        let stride = CVPixelBufferGetBytesPerRow(pixel_buffer);
        let row_bytes = width as usize * 4;
        if base.is_null() || stride < row_bytes {
            let _ = CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);
            CFRelease(pixel_buffer as CFTypeRef);
            return Err("CVPixelBuffer base address unavailable".to_owned());
        }

        for y in 0..height as usize {
            let src_start = y * row_bytes;
            let src_end = src_start + row_bytes;
            let dst = base.add(y * stride);
            ptr::copy_nonoverlapping(bgra[src_start..src_end].as_ptr(), dst, row_bytes);
        }

        let status = CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);
        if status != 0 {
            CFRelease(pixel_buffer as CFTypeRef);
            return Err(format!("CVPixelBufferUnlockBaseAddress status={status}"));
        }
        Ok(pixel_buffer)
    }

    unsafe extern "C" fn compression_output_callback(
        output_callback_refcon: *mut c_void,
        _source_frame_refcon: *mut c_void,
        status: OSStatus,
        _info_flags: VTEncodeInfoFlags,
        sample_buffer: CMSampleBufferRef,
    ) {
        if status != 0 || output_callback_refcon.is_null() || sample_buffer.is_null() {
            return;
        }
        let sink = &*(output_callback_refcon as *const PacketSink);
        if let Some(packet) = h264_packet_from_sample_buffer(sample_buffer) {
            if let Ok(mut packets) = sink.packets.lock() {
                packets.push(packet);
            }
        }
    }

    unsafe fn h264_packet_from_sample_buffer(
        sample_buffer: CMSampleBufferRef,
    ) -> Option<NvencPacket> {
        if CMSampleBufferDataIsReady(sample_buffer) == 0 {
            return None;
        }
        let block = CMSampleBufferGetDataBuffer(sample_buffer);
        if block.is_null() {
            return None;
        }

        let format = CMSampleBufferGetFormatDescription(sample_buffer);
        let key = sample_buffer_is_keyframe(sample_buffer);
        let mut nal_length_size = 4usize;
        let mut bytes = Vec::new();
        if !format.is_null() {
            let (parameter_sets, length_size) = h264_parameter_sets(format);
            if length_size > 0 {
                nal_length_size = length_size;
            }
            if key {
                for set in parameter_sets {
                    append_annex_b_nal(&mut bytes, &set);
                }
            }
        }

        let data_len = CMBlockBufferGetDataLength(block);
        if data_len == 0 {
            return None;
        }
        let mut avcc = vec![0_u8; data_len];
        if CMBlockBufferCopyDataBytes(block, 0, data_len, avcc.as_mut_ptr().cast()) != 0 {
            return None;
        }
        if !append_avcc_payload_as_annex_b(&avcc, nal_length_size, &mut bytes) {
            return None;
        }
        Some(NvencPacket {
            codec: NvencCodec::H264,
            bytes,
            key,
        })
    }

    unsafe fn h264_parameter_sets(format: CMFormatDescriptionRef) -> (Vec<Vec<u8>>, usize) {
        let mut count = 0usize;
        let mut header_len = 4i32;
        let mut first_ptr = ptr::null();
        let mut first_len = 0usize;
        let status = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            format,
            0,
            &mut first_ptr,
            &mut first_len,
            &mut count,
            &mut header_len,
        );
        if status != 0 || count == 0 {
            return (Vec::new(), 4);
        }

        let mut sets = Vec::new();
        if !first_ptr.is_null() && first_len > 0 {
            sets.push(slice::from_raw_parts(first_ptr, first_len).to_vec());
        }
        for index in 1..count {
            let mut ptr_out = ptr::null();
            let mut len_out = 0usize;
            let mut ignored_count = 0usize;
            let mut ignored_header_len = header_len;
            if CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                format,
                index,
                &mut ptr_out,
                &mut len_out,
                &mut ignored_count,
                &mut ignored_header_len,
            ) == 0
                && !ptr_out.is_null()
                && len_out > 0
            {
                sets.push(slice::from_raw_parts(ptr_out, len_out).to_vec());
            }
        }

        (sets, header_len.max(1) as usize)
    }

    unsafe fn sample_buffer_is_keyframe(sample_buffer: CMSampleBufferRef) -> bool {
        let attachments = CMSampleBufferGetSampleAttachmentsArray(sample_buffer, 0);
        if attachments.is_null() || CFArrayGetCount(attachments) <= 0 {
            return true;
        }

        let attachment = CFArrayGetValueAtIndex(attachments, 0) as CFDictionaryRef;
        if attachment.is_null() {
            return true;
        }

        let not_sync =
            CFDictionaryGetValue(attachment, kCMSampleAttachmentKey_NotSync as *const c_void);
        not_sync.is_null() || CFBooleanGetValue(not_sync as CFBooleanRef) == 0
    }

    fn append_avcc_payload_as_annex_b(
        avcc: &[u8],
        nal_length_size: usize,
        out: &mut Vec<u8>,
    ) -> bool {
        let nal_length_size = nal_length_size.clamp(1, 4);
        let mut pos = 0usize;
        let before = out.len();
        while pos + nal_length_size <= avcc.len() {
            let mut len = 0usize;
            for byte in &avcc[pos..pos + nal_length_size] {
                len = (len << 8) | usize::from(*byte);
            }
            pos += nal_length_size;
            if len == 0 {
                continue;
            }
            let Some(end) = pos.checked_add(len) else {
                return false;
            };
            if end > avcc.len() {
                return false;
            }
            append_annex_b_nal(out, &avcc[pos..end]);
            pos = end;
        }
        out.len() > before
    }

    fn append_annex_b_nal(out: &mut Vec<u8>, nal: &[u8]) {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }

    unsafe fn create_hardware_encoder_specification() -> CFDictionaryRef {
        let keys =
            [kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder as *const c_void];
        let values = [kCFBooleanTrue as *const c_void];
        CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            ptr::null(),
            ptr::null(),
        )
    }

    unsafe fn create_force_keyframe_dictionary() -> CFDictionaryRef {
        let keys = [kVTEncodeFrameOptionKey_ForceKeyFrame as *const c_void];
        let values = [kCFBooleanTrue as *const c_void];
        CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            ptr::null(),
            ptr::null(),
        )
    }

    unsafe fn set_bool_property(session: VTCompressionSessionRef, key: CFStringRef, value: bool) {
        let cf_value = if value {
            kCFBooleanTrue
        } else {
            kCFBooleanFalse
        };
        let _ = VTSessionSetProperty(session as CFTypeRef, key, cf_value as CFTypeRef);
    }

    unsafe fn set_i32_property(session: VTCompressionSessionRef, key: CFStringRef, value: i32) {
        let cf_value = CFNumberCreate(
            ptr::null(),
            K_CF_NUMBER_SINT32_TYPE,
            (&value as *const i32).cast(),
        );
        if cf_value.is_null() {
            return;
        }
        let _ = VTSessionSetProperty(session as CFTypeRef, key, cf_value as CFTypeRef);
        CFRelease(cf_value as CFTypeRef);
    }
}

#[cfg(not(target_os = "macos"))]
mod fallback {
    use crate::nvenc::{NvencCodec, NvencPacket};

    pub struct VideoToolboxEncoder;

    impl VideoToolboxEncoder {
        pub fn new(
            codec: NvencCodec,
            _width: u32,
            _height: u32,
            _fps: u32,
            _bitrate: u32,
        ) -> Result<Self, String> {
            Err(format!(
                "VideoToolbox is macOS-only, requested {}",
                codec.label()
            ))
        }

        pub fn matches(&self, _codec: NvencCodec, _width: u32, _height: u32, _fps: u32) -> bool {
            false
        }

        pub fn encode_bgra(
            &mut self,
            _bgra: &[u8],
            _force_key: bool,
        ) -> Result<Option<NvencPacket>, String> {
            Err("VideoToolbox is macOS-only".to_owned())
        }
    }

    pub fn videotoolbox_supported_platform() -> bool {
        false
    }

    pub fn videotoolbox_codecs() -> Vec<NvencCodec> {
        Vec::new()
    }

    pub fn videotoolbox_encoder_names() -> Option<Vec<String>> {
        Some(Vec::new())
    }
}

#[cfg(not(target_os = "macos"))]
pub use fallback::{
    videotoolbox_codecs, videotoolbox_encoder_names, videotoolbox_supported_platform,
    VideoToolboxEncoder,
};
#[cfg(target_os = "macos")]
pub use macos::{
    videotoolbox_codecs, videotoolbox_encoder_names, videotoolbox_supported_platform,
    VideoToolboxEncoder,
};
