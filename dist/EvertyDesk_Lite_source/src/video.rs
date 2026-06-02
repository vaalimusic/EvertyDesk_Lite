#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveVideoMode {
    ScreenshotOnly,
    H264,
    H264Vpx,
}

#[allow(dead_code)]
impl LiveVideoMode {
    pub fn current() -> Self {
        if cfg!(feature = "live-h264") && cfg!(feature = "live-vpx") {
            Self::H264Vpx
        } else if cfg!(feature = "live-h264") {
            Self::H264
        } else {
            Self::ScreenshotOnly
        }
    }

    pub fn h264_enabled(self) -> bool {
        matches!(self, Self::H264 | Self::H264Vpx)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ScreenshotOnly => "PNG only",
            Self::H264 => "H264",
            Self::H264Vpx => "H264 + VP8/VP9",
        }
    }
}

pub fn build_codec_label() -> &'static str {
    LiveVideoMode::current().label()
}
