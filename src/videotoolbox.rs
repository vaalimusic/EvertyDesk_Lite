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
    type CMItemCount = isize;
    type CMSampleBufferRef = *const c_void;
    type CVPixelBufferRef = *mut c_void;
    type OSStatus = i32;
    type OSType = u32;
    type VTCompressionSessionRef = *mut c_void;
    type VTEncodeInfoFlags = u32;
    type VTDecodeFrameFlags = u32;
    type VTDecodeInfoFlags = u32;
    type VTDecompressionSessionRef = *mut c_void;

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

    type VTDecompressionOutputCallback = unsafe extern "C" fn(
        decompression_output_refcon: *mut c_void,
        source_frame_refcon: *mut c_void,
        status: OSStatus,
        info_flags: VTDecodeInfoFlags,
        image_buffer: CVPixelBufferRef,
        presentation_time_stamp: CMTime,
        presentation_duration: CMTime,
    );

    #[repr(C)]
    struct VTDecompressionOutputCallbackRecord {
        decompression_output_callback: Option<VTDecompressionOutputCallback>,
        decompression_output_refcon: *mut c_void,
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFAllocatorNull: CFAllocatorRef;
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

        fn CMBlockBufferCreateWithMemoryBlock(
            structure_allocator: CFAllocatorRef,
            memory_block: *mut c_void,
            block_length: usize,
            block_allocator: CFAllocatorRef,
            custom_block_source: *const c_void,
            offset_to_data: usize,
            data_length: usize,
            flags: u32,
            block_buffer_out: *mut CMBlockBufferRef,
        ) -> OSStatus;
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
        fn CMSampleBufferCreateReady(
            allocator: CFAllocatorRef,
            data_buffer: CMBlockBufferRef,
            format_description: CMFormatDescriptionRef,
            num_samples: CMItemCount,
            num_sample_timing_entries: CMItemCount,
            sample_timing_array: *const c_void,
            num_sample_size_entries: CMItemCount,
            sample_size_array: *const usize,
            sample_buffer_out: *mut CMSampleBufferRef,
        ) -> OSStatus;
        fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
            allocator: CFAllocatorRef,
            parameter_set_count: usize,
            parameter_set_pointers: *const *const u8,
            parameter_set_sizes: *const usize,
            nal_unit_header_length: i32,
            format_description_out: *mut CMFormatDescriptionRef,
        ) -> OSStatus;
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
        static kCVPixelBufferPixelFormatTypeKey: CFStringRef;

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
        fn CVPixelBufferGetHeight(pixel_buffer: CVPixelBufferRef) -> usize;
        fn CVPixelBufferGetPixelFormatType(pixel_buffer: CVPixelBufferRef) -> OSType;
        fn CVPixelBufferGetWidth(pixel_buffer: CVPixelBufferRef) -> usize;
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
        static kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder: CFStringRef;

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
        fn VTDecompressionSessionCreate(
            allocator: CFAllocatorRef,
            video_format_description: CMFormatDescriptionRef,
            video_decoder_specification: CFDictionaryRef,
            destination_image_buffer_attributes: CFDictionaryRef,
            output_callback: *const VTDecompressionOutputCallbackRecord,
            decompression_session_out: *mut VTDecompressionSessionRef,
        ) -> OSStatus;
        fn VTDecompressionSessionDecodeFrame(
            session: VTDecompressionSessionRef,
            sample_buffer: CMSampleBufferRef,
            decode_flags: VTDecodeFrameFlags,
            source_frame_refcon: *mut c_void,
            info_flags_out: *mut VTDecodeInfoFlags,
        ) -> OSStatus;
        fn VTDecompressionSessionInvalidate(session: VTDecompressionSessionRef);
        fn VTDecompressionSessionWaitForAsynchronousFrames(
            session: VTDecompressionSessionRef,
        ) -> OSStatus;
    }

    struct PacketSink {
        packets: Mutex<Vec<NvencPacket>>,
    }

    struct DecodeSink {
        frame: Mutex<Option<(usize, usize, Vec<u8>)>>,
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

    pub fn videotoolbox_h264_decoder_available() -> bool {
        true
    }

    pub struct VideoToolboxH264Decoder {
        session: VTDecompressionSessionRef,
        sink: Box<DecodeSink>,
        sps: Vec<u8>,
        pps: Vec<u8>,
    }

    unsafe impl Send for VideoToolboxH264Decoder {}

    impl VideoToolboxH264Decoder {
        pub fn new() -> Self {
            Self {
                session: ptr::null_mut(),
                sink: Box::new(DecodeSink {
                    frame: Mutex::new(None),
                }),
                sps: Vec::new(),
                pps: Vec::new(),
            }
        }

        pub fn decode_packets<I>(
            &mut self,
            packets: I,
        ) -> Result<Option<(usize, usize, Vec<u8>)>, String>
        where
            I: IntoIterator<Item = Vec<u8>>,
        {
            let mut decoded = None;
            for packet in packets {
                if packet.is_empty() {
                    continue;
                }
                if let Some(frame) = self.decode_packet(&packet)? {
                    decoded = Some(frame);
                }
            }
            Ok(decoded)
        }

        fn decode_packet(
            &mut self,
            packet: &[u8],
        ) -> Result<Option<(usize, usize, Vec<u8>)>, String> {
            let nals = h264_nals(packet);
            if nals.is_empty() {
                return Ok(None);
            }

            let mut sample = Vec::new();
            let mut parameter_sets_changed = false;
            for nal in nals {
                if nal.is_empty() {
                    continue;
                }
                match nal[0] & 0x1f {
                    7 => {
                        if self.sps.as_slice() != nal {
                            self.sps.clear();
                            self.sps.extend_from_slice(nal);
                            parameter_sets_changed = true;
                        }
                    }
                    8 => {
                        if self.pps.as_slice() != nal {
                            self.pps.clear();
                            self.pps.extend_from_slice(nal);
                            parameter_sets_changed = true;
                        }
                    }
                    9 => {}
                    _ => append_avcc_nal(&mut sample, nal)?,
                }
            }

            if sample.is_empty() {
                return Ok(None);
            }
            if self.sps.is_empty() || self.pps.is_empty() {
                return Err("VideoToolbox H264 decoder needs more packets".to_owned());
            }
            if self.session.is_null() || parameter_sets_changed {
                unsafe {
                    self.recreate_session()?;
                }
            }

            self.sink
                .frame
                .lock()
                .map_err(|_| "VideoToolbox decode sink poisoned".to_owned())?
                .take();

            unsafe {
                let sample_buffer = create_h264_sample_buffer(&sample, &self.sps, &self.pps)?;
                let mut info_flags = 0;
                let status = VTDecompressionSessionDecodeFrame(
                    self.session,
                    sample_buffer,
                    0,
                    ptr::null_mut(),
                    &mut info_flags,
                );
                CFRelease(sample_buffer as CFTypeRef);
                if status != 0 {
                    return Err(format!("VTDecompressionSessionDecodeFrame status={status}"));
                }
                let status = VTDecompressionSessionWaitForAsynchronousFrames(self.session);
                if status != 0 {
                    return Err(format!(
                        "VTDecompressionSessionWaitForAsynchronousFrames status={status}"
                    ));
                }
            }

            self.sink
                .frame
                .lock()
                .map_err(|_| "VideoToolbox decode sink poisoned".to_owned())?
                .take()
                .ok_or_else(|| "VideoToolbox H264 decoder needs more packets".to_owned())
                .map(Some)
        }

        unsafe fn recreate_session(&mut self) -> Result<(), String> {
            if !self.session.is_null() {
                VTDecompressionSessionInvalidate(self.session);
                CFRelease(self.session as CFTypeRef);
                self.session = ptr::null_mut();
            }

            let format = create_h264_format_description(&self.sps, &self.pps)?;
            let decoder_spec = create_hardware_decoder_specification();
            let (attrs, attrs_value) = create_bgra_pixel_buffer_attributes();
            let callback = VTDecompressionOutputCallbackRecord {
                decompression_output_callback: Some(decompression_output_callback),
                decompression_output_refcon: (&mut *self.sink as *mut DecodeSink).cast(),
            };
            let status = VTDecompressionSessionCreate(
                ptr::null(),
                format,
                decoder_spec,
                attrs,
                &callback,
                &mut self.session,
            );
            if !attrs.is_null() {
                CFRelease(attrs as CFTypeRef);
            }
            if !attrs_value.is_null() {
                CFRelease(attrs_value as CFTypeRef);
            }
            if !decoder_spec.is_null() {
                CFRelease(decoder_spec as CFTypeRef);
            }
            CFRelease(format as CFTypeRef);

            if status != 0 || self.session.is_null() {
                self.session = ptr::null_mut();
                return Err(format!("VTDecompressionSessionCreate status={status}"));
            }
            Ok(())
        }
    }

    impl Drop for VideoToolboxH264Decoder {
        fn drop(&mut self) {
            unsafe {
                if !self.session.is_null() {
                    VTDecompressionSessionInvalidate(self.session);
                    CFRelease(self.session as CFTypeRef);
                    self.session = ptr::null_mut();
                }
            }
        }
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

    unsafe fn create_h264_format_description(
        sps: &[u8],
        pps: &[u8],
    ) -> Result<CMFormatDescriptionRef, String> {
        let parameter_sets = [sps.as_ptr(), pps.as_ptr()];
        let parameter_set_sizes = [sps.len(), pps.len()];
        let mut format = ptr::null();
        let status = CMVideoFormatDescriptionCreateFromH264ParameterSets(
            ptr::null(),
            parameter_sets.len(),
            parameter_sets.as_ptr(),
            parameter_set_sizes.as_ptr(),
            4,
            &mut format,
        );
        if status != 0 || format.is_null() {
            return Err(format!(
                "CMVideoFormatDescriptionCreateFromH264ParameterSets status={status}"
            ));
        }
        Ok(format)
    }

    unsafe fn create_h264_sample_buffer(
        avcc_sample: &[u8],
        sps: &[u8],
        pps: &[u8],
    ) -> Result<CMSampleBufferRef, String> {
        let format = create_h264_format_description(sps, pps)?;
        let mut block = ptr::null();
        let status = CMBlockBufferCreateWithMemoryBlock(
            ptr::null(),
            avcc_sample.as_ptr() as *mut c_void,
            avcc_sample.len(),
            kCFAllocatorNull,
            ptr::null(),
            0,
            avcc_sample.len(),
            0,
            &mut block,
        );
        if status != 0 || block.is_null() {
            CFRelease(format as CFTypeRef);
            return Err(format!(
                "CMBlockBufferCreateWithMemoryBlock status={status}"
            ));
        }

        let sample_size = avcc_sample.len();
        let mut sample_buffer = ptr::null();
        let status = CMSampleBufferCreateReady(
            ptr::null(),
            block,
            format,
            1,
            0,
            ptr::null(),
            1,
            &sample_size,
            &mut sample_buffer,
        );
        CFRelease(block as CFTypeRef);
        CFRelease(format as CFTypeRef);
        if status != 0 || sample_buffer.is_null() {
            return Err(format!("CMSampleBufferCreateReady status={status}"));
        }
        Ok(sample_buffer)
    }

    unsafe fn create_bgra_pixel_buffer_attributes() -> (CFDictionaryRef, CFNumberRef) {
        let pixel_format = K_CV_PIXEL_FORMAT_TYPE_32_BGRA as i32;
        let value = CFNumberCreate(
            ptr::null(),
            K_CF_NUMBER_SINT32_TYPE,
            (&pixel_format as *const i32).cast(),
        );
        if value.is_null() {
            return (ptr::null(), ptr::null());
        }
        let keys = [kCVPixelBufferPixelFormatTypeKey as *const c_void];
        let values = [value as *const c_void];
        let dict = CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            ptr::null(),
            ptr::null(),
        );
        (dict, value)
    }

    unsafe fn create_hardware_decoder_specification() -> CFDictionaryRef {
        let keys =
            [kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder as *const c_void];
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

    unsafe extern "C" fn decompression_output_callback(
        decompression_output_refcon: *mut c_void,
        _source_frame_refcon: *mut c_void,
        status: OSStatus,
        _info_flags: VTDecodeInfoFlags,
        image_buffer: CVPixelBufferRef,
        _presentation_time_stamp: CMTime,
        _presentation_duration: CMTime,
    ) {
        if status != 0 || decompression_output_refcon.is_null() || image_buffer.is_null() {
            return;
        }
        let sink = &*(decompression_output_refcon as *const DecodeSink);
        if let Some(frame) = rgba_from_bgra_pixel_buffer(image_buffer) {
            if let Ok(mut slot) = sink.frame.lock() {
                *slot = Some(frame);
            }
        }
    }

    unsafe fn rgba_from_bgra_pixel_buffer(
        pixel_buffer: CVPixelBufferRef,
    ) -> Option<(usize, usize, Vec<u8>)> {
        if CVPixelBufferGetPixelFormatType(pixel_buffer) != K_CV_PIXEL_FORMAT_TYPE_32_BGRA {
            return None;
        }
        let width = CVPixelBufferGetWidth(pixel_buffer);
        let height = CVPixelBufferGetHeight(pixel_buffer);
        if width == 0 || height == 0 {
            return None;
        }
        if CVPixelBufferLockBaseAddress(pixel_buffer, 0) != 0 {
            return None;
        }
        let base = CVPixelBufferGetBaseAddress(pixel_buffer) as *const u8;
        let stride = CVPixelBufferGetBytesPerRow(pixel_buffer);
        let row_bytes = width.checked_mul(4)?;
        let frame_bytes = row_bytes.checked_mul(height)?;
        let mut rgba = vec![0_u8; frame_bytes];
        if !base.is_null() && stride >= row_bytes {
            for y in 0..height {
                let src = slice::from_raw_parts(base.add(y * stride), row_bytes);
                let dst = &mut rgba[y * row_bytes..(y + 1) * row_bytes];
                for (bgra, rgba) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                    rgba[0] = bgra[2];
                    rgba[1] = bgra[1];
                    rgba[2] = bgra[0];
                    rgba[3] = bgra[3];
                }
            }
        }
        let _ = CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);
        if base.is_null() || stride < row_bytes {
            None
        } else {
            Some((width, height, rgba))
        }
    }

    fn h264_nals(packet: &[u8]) -> Vec<&[u8]> {
        let starts = annex_b_start_codes(packet);
        if starts.is_empty() {
            return avcc_nals(packet);
        }

        let mut nals = Vec::new();
        for (idx, (start, code_len)) in starts.iter().copied().enumerate() {
            let nal_start = start + code_len;
            let nal_end = starts
                .get(idx + 1)
                .map(|(next, _)| *next)
                .unwrap_or(packet.len());
            if nal_start < nal_end {
                nals.push(trim_zero_padding(&packet[nal_start..nal_end]));
            }
        }
        nals.retain(|nal| !nal.is_empty());
        nals
    }

    fn annex_b_start_codes(packet: &[u8]) -> Vec<(usize, usize)> {
        let mut starts = Vec::new();
        let mut i = 0usize;
        while i + 3 <= packet.len() {
            if packet[i] == 0 && packet[i + 1] == 0 && packet[i + 2] == 1 {
                starts.push((i, 3));
                i += 3;
            } else if i + 4 <= packet.len()
                && packet[i] == 0
                && packet[i + 1] == 0
                && packet[i + 2] == 0
                && packet[i + 3] == 1
            {
                starts.push((i, 4));
                i += 4;
            } else {
                i += 1;
            }
        }
        starts
    }

    fn avcc_nals(packet: &[u8]) -> Vec<&[u8]> {
        let mut nals = Vec::new();
        let mut pos = 0usize;
        while pos + 4 <= packet.len() {
            let len = u32::from_be_bytes([
                packet[pos],
                packet[pos + 1],
                packet[pos + 2],
                packet[pos + 3],
            ]) as usize;
            pos += 4;
            let Some(end) = pos.checked_add(len) else {
                return Vec::new();
            };
            if len == 0 || end > packet.len() {
                return Vec::new();
            }
            nals.push(&packet[pos..end]);
            pos = end;
        }
        if nals.is_empty() && !packet.is_empty() {
            nals.push(packet);
        }
        nals
    }

    fn trim_zero_padding(mut nal: &[u8]) -> &[u8] {
        while nal.last() == Some(&0) {
            nal = &nal[..nal.len() - 1];
        }
        nal
    }

    fn append_avcc_nal(out: &mut Vec<u8>, nal: &[u8]) -> Result<(), String> {
        let len =
            u32::try_from(nal.len()).map_err(|_| "VideoToolbox H264 NAL too large".to_owned())?;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(nal);
        Ok(())
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
    pub struct VideoToolboxH264Decoder;

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

    impl VideoToolboxH264Decoder {
        pub fn new() -> Self {
            Self
        }

        pub fn decode_packets<I>(
            &mut self,
            _packets: I,
        ) -> Result<Option<(usize, usize, Vec<u8>)>, String>
        where
            I: IntoIterator<Item = Vec<u8>>,
        {
            Err("VideoToolbox H264 decoder is macOS-only".to_owned())
        }
    }

    pub fn videotoolbox_supported_platform() -> bool {
        false
    }

    pub fn videotoolbox_h264_decoder_available() -> bool {
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
    videotoolbox_codecs, videotoolbox_encoder_names, videotoolbox_h264_decoder_available,
    videotoolbox_supported_platform, VideoToolboxEncoder, VideoToolboxH264Decoder,
};
#[cfg(target_os = "macos")]
pub use macos::{
    videotoolbox_codecs, videotoolbox_encoder_names, videotoolbox_h264_decoder_available,
    videotoolbox_supported_platform, VideoToolboxEncoder, VideoToolboxH264Decoder,
};
