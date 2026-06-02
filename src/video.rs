#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveVideoMode {
    ScreenshotOnly,
    H264Only,
    Vp8Vp9Only,
    Vp9Only,
    H264Vp8Vp9,
    H264Vp9,
}

#[allow(dead_code)]
impl LiveVideoMode {
    pub fn current() -> Self {
        match (h264_available(), vp8_available(), vp9_available()) {
            (true, true, true) => Self::H264Vp8Vp9,
            (true, false, true) => Self::H264Vp9,
            (true, _, false) => Self::H264Only,
            (false, true, true) => Self::Vp8Vp9Only,
            (false, false, true) => Self::Vp9Only,
            (false, _, false) => Self::ScreenshotOnly,
        }
    }

    pub fn h264_enabled(self) -> bool {
        matches!(self, Self::H264Only | Self::H264Vp8Vp9 | Self::H264Vp9)
    }

    pub fn vp9_enabled(self) -> bool {
        matches!(
            self,
            Self::Vp8Vp9Only | Self::Vp9Only | Self::H264Vp8Vp9 | Self::H264Vp9
        )
    }

    pub fn label(self) -> String {
        match self {
            Self::ScreenshotOnly => "PNG only".to_owned(),
            Self::H264Only => "H264".to_owned(),
            Self::Vp8Vp9Only => format!("VP8/VP9 ({})", vp9_backend_label()),
            Self::Vp9Only => format!("VP9 ({})", vp9_backend_label()),
            Self::H264Vp8Vp9 => format!("H264 + VP8/VP9 ({})", vp9_backend_label()),
            Self::H264Vp9 => format!("H264 + VP9 ({})", vp9_backend_label()),
        }
    }
}

pub fn h264_available() -> bool {
    cfg!(feature = "live-h264")
}

pub fn vp8_available() -> bool {
    cfg!(feature = "live-vpx")
}

pub fn vp9_available() -> bool {
    cfg!(any(
        feature = "live-vpx",
        feature = "live-vpx-system",
        all(feature = "live-vp9-mf", target_os = "windows")
    ))
}

pub fn vp9_backend_label() -> &'static str {
    if cfg!(feature = "live-vpx") {
        "libvpx"
    } else if cfg!(feature = "live-vpx-system") {
        "system libvpx"
    } else if cfg!(all(feature = "live-vp9-mf", target_os = "windows")) {
        "Media Foundation"
    } else {
        "not compiled"
    }
}

pub fn build_codec_label() -> String {
    LiveVideoMode::current().label()
}
