use crate::nvenc::{NvencCodec, NvencPacket};

pub struct VideoToolboxEncoder {
    codec: NvencCodec,
    width: u32,
    height: u32,
    fps: u32,
}

impl VideoToolboxEncoder {
    pub fn new(
        codec: NvencCodec,
        width: u32,
        height: u32,
        fps: u32,
        _bitrate: u32,
    ) -> Result<Self, String> {
        if !matches!(codec, NvencCodec::H264 | NvencCodec::H265) {
            return Err(format!(
                "VideoToolbox does not expose {} encoder",
                codec.label()
            ));
        }

        let _ = (width, height, fps);
        Err("Direct VideoToolbox encoder backend is not implemented yet".to_owned())
    }

    pub fn matches(&self, codec: NvencCodec, width: u32, height: u32, fps: u32) -> bool {
        self.codec == codec
            && self.width == width.max(2)
            && self.height == height.max(2)
            && self.fps == fps.clamp(5, 60)
    }

    pub fn encode_bgra(&mut self, _bgra: &[u8]) -> Result<Option<NvencPacket>, String> {
        Err("Direct VideoToolbox encoder backend is not implemented yet".to_owned())
    }
}

pub fn videotoolbox_supported_platform() -> bool {
    cfg!(target_os = "macos")
}

pub fn videotoolbox_codecs() -> Vec<NvencCodec> {
    Vec::new()
}

pub fn videotoolbox_encoder_names() -> Option<Vec<String>> {
    Some(Vec::new())
}
