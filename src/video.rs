use std::{process::Command, sync::OnceLock};

use crate::settings::EncoderPreference;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveVideoMode {
    ScreenshotOnly,
    H264Only,
    H264H265Av1,
    Vp8Vp9Only,
    Vp9Only,
    H264Vp8Vp9,
    H264Vp9,
}

#[allow(dead_code)]
impl LiveVideoMode {
    pub fn current() -> Self {
        match (
            h264_available(),
            h265_available(),
            av1_available(),
            vp8_available(),
            vp9_available(),
        ) {
            (true, true, true, _, _) => Self::H264H265Av1,
            (true, _, _, true, true) => Self::H264Vp8Vp9,
            (true, _, _, false, true) => Self::H264Vp9,
            (true, _, _, _, false) => Self::H264Only,
            (false, _, _, true, true) => Self::Vp8Vp9Only,
            (false, _, _, false, true) => Self::Vp9Only,
            (false, _, _, _, false) => Self::ScreenshotOnly,
        }
    }

    pub fn h264_enabled(self) -> bool {
        matches!(
            self,
            Self::H264Only | Self::H264H265Av1 | Self::H264Vp8Vp9 | Self::H264Vp9
        )
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
            Self::H264H265Av1 => "H264 + H265/AV1".to_owned(),
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

pub fn h265_available() -> bool {
    // Do not advertise H265 until a real decoder backend is wired in.
    false
}

pub fn av1_available() -> bool {
    // Do not advertise AV1 until a real decoder backend is wired in.
    false
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
    let nvenc = nvenc_status().label();
    format!("{}; {}", LiveVideoMode::current().label(), nvenc)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NvencStatus {
    Available { encoders: Vec<String> },
    NvidiaGpuButNoFfmpeg,
    NotAvailable,
}

impl NvencStatus {
    pub fn available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    pub fn label(&self) -> String {
        match self {
            Self::Available { encoders } => {
                if encoders.is_empty() {
                    "NVENC available".to_owned()
                } else {
                    format!("NVENC: {}", encoders.join(", "))
                }
            }
            Self::NvidiaGpuButNoFfmpeg => "NVENC: NVIDIA detected, ffmpeg missing".to_owned(),
            Self::NotAvailable => "NVENC: not available".to_owned(),
        }
    }
}

pub fn nvenc_status() -> &'static NvencStatus {
    static STATUS: OnceLock<NvencStatus> = OnceLock::new();
    STATUS.get_or_init(detect_nvenc_status)
}

pub fn selected_encoder_label(preference: EncoderPreference) -> String {
    match preference {
        EncoderPreference::Auto if nvenc_status().available() => {
            format!("Auto -> {}", nvenc_status().label())
        }
        EncoderPreference::Auto => "Auto -> Software H264".to_owned(),
        EncoderPreference::Nvenc if nvenc_status().available() => nvenc_status().label(),
        EncoderPreference::Nvenc => "NVENC requested, unavailable -> Software H264".to_owned(),
        EncoderPreference::Software => "Software H264".to_owned(),
    }
}

fn detect_nvenc_status() -> NvencStatus {
    if let Some(encoders) = ffmpeg_nvenc_encoders() {
        if !encoders.is_empty() {
            return NvencStatus::Available { encoders };
        }
    } else if command_exists("nvidia-smi") {
        return NvencStatus::NvidiaGpuButNoFfmpeg;
    }

    NvencStatus::NotAvailable
}

fn ffmpeg_nvenc_encoders() -> Option<Vec<String>> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let mut encoders = Vec::new();
    for name in ["h264_nvenc", "hevc_nvenc", "av1_nvenc"] {
        if text.contains(name) {
            encoders.push(name.to_owned());
        }
    }
    Some(encoders)
}

fn command_exists(name: &str) -> bool {
    Command::new(name)
        .arg("--help")
        .output()
        .map(|output| {
            output.status.success() || !output.stdout.is_empty() || !output.stderr.is_empty()
        })
        .unwrap_or(false)
}
