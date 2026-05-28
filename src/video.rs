#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveVideoMode {
    ScreenshotOnly,
    H264,
}

#[allow(dead_code)]
impl LiveVideoMode {
    pub fn current() -> Self {
        if cfg!(feature = "live-h264") {
            Self::H264
        } else {
            Self::ScreenshotOnly
        }
    }

    pub fn h264_enabled(self) -> bool {
        matches!(self, Self::H264)
    }
}
