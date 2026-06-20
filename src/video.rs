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
    cfg!(feature = "live-h264") || crate::videotoolbox::videotoolbox_h264_decoder_available()
}

pub fn h265_available() -> bool {
    // Windows Media Foundation HEVC, or macOS VideoToolbox HEVC (same gate as
    // the VT H264 decoder — true only on macOS).
    crate::mf_video::h265_decode_available()
        || crate::videotoolbox::videotoolbox_h264_decoder_available()
}

pub fn av1_available() -> bool {
    crate::mf_video::av1_decode_available()
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
    let mf_encoder = crate::mf_encode::mf_encoder_status().label();
    let nvenc = nvenc_status().label();
    let videotoolbox = videotoolbox_status().label();
    format!(
        "{}; {}; {}; {}; {}; {}",
        LiveVideoMode::current().label(),
        crate::mf_video::mf_video_decode_status().label(),
        mf_encoder,
        videotoolbox,
        nv_codec_sdk_label(),
        nvenc
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NvencStatus {
    Available {
        encoders: Vec<String>,
        api_ready: bool,
    },
    ApiReadyNoDirectEncoder,
    NvidiaGpuNoDirectEncoder,
    UnsupportedPlatform,
    NotAvailable,
}

impl NvencStatus {
    pub fn available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    pub fn label(&self) -> String {
        match self {
            Self::Available {
                encoders,
                api_ready,
            } => {
                let api = if *api_ready {
                    crate::nvenc::nvencode_api_probe().label()
                } else {
                    "NvEncodeAPI FFI not ready".to_owned()
                };
                if encoders.is_empty() {
                    format!("NVENC available; {api}")
                } else {
                    format!("NVENC: {}; {api}", encoders.join(", "))
                }
            }
            Self::ApiReadyNoDirectEncoder => {
                "NVENC API ready, direct encoder backend in progress".to_owned()
            }
            Self::NvidiaGpuNoDirectEncoder => {
                "NVENC: NVIDIA detected, direct encoder backend in progress".to_owned()
            }
            Self::UnsupportedPlatform => "NVENC: not supported on macOS".to_owned(),
            Self::NotAvailable => "NVENC: not available".to_owned(),
        }
    }
}

pub fn nvenc_status() -> &'static NvencStatus {
    static STATUS: OnceLock<NvencStatus> = OnceLock::new();
    STATUS.get_or_init(detect_nvenc_status)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoToolboxStatus {
    Available { encoders: Vec<String> },
    DirectEncoderUnavailable,
    UnsupportedPlatform,
}

impl VideoToolboxStatus {
    pub fn available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    pub fn label(&self) -> String {
        match self {
            Self::Available { encoders } if encoders.is_empty() => {
                "VideoToolbox available".to_owned()
            }
            Self::Available { encoders } => {
                format!("VideoToolbox: {}", encoders.join(", "))
            }
            Self::DirectEncoderUnavailable => {
                "VideoToolbox: direct encoder backend in progress".to_owned()
            }
            Self::UnsupportedPlatform => "VideoToolbox: macOS only".to_owned(),
        }
    }
}

pub fn videotoolbox_status() -> &'static VideoToolboxStatus {
    static STATUS: OnceLock<VideoToolboxStatus> = OnceLock::new();
    STATUS.get_or_init(detect_videotoolbox_status)
}

pub fn selected_encoder_label(preference: EncoderPreference) -> String {
    match preference {
        EncoderPreference::Auto if !crate::mf_encode::mf_encoder_codecs().is_empty() => {
            format!("Auto -> {}", crate::mf_encode::mf_encoder_status().label())
        }
        EncoderPreference::Auto if videotoolbox_status().available() => {
            format!("Auto -> {}", videotoolbox_status().label())
        }
        EncoderPreference::Auto if nvenc_status().available() => {
            format!(
                "Auto -> {}; {}",
                nvenc_status().label(),
                nv_codec_sdk_label()
            )
        }
        EncoderPreference::Auto if nv_codec_sdk_present() => {
            format!("Auto -> Software H264; {}", nv_codec_sdk_label())
        }
        EncoderPreference::Auto => "Auto -> Software H264".to_owned(),
        EncoderPreference::Nvenc
            if cfg!(target_os = "macos") && videotoolbox_status().available() =>
        {
            videotoolbox_status().label()
        }
        EncoderPreference::Nvenc if nvenc_status().available() => {
            format!("{}; {}", nvenc_status().label(), nv_codec_sdk_label())
        }
        EncoderPreference::Nvenc if !crate::mf_encode::mf_encoder_codecs().is_empty() => {
            format!(
                "Native hardware requested -> {}",
                crate::mf_encode::mf_encoder_status().label()
            )
        }
        EncoderPreference::Nvenc if cfg!(target_os = "macos") => {
            format!(
                "VideoToolbox requested, runtime unavailable -> Software H264 ({})",
                videotoolbox_status().label()
            )
        }
        EncoderPreference::Nvenc if nv_codec_sdk_present() => {
            format!(
                "NVENC SDK ready, runtime unavailable -> Software H264; {}",
                nv_codec_sdk_label()
            )
        }
        EncoderPreference::Nvenc => {
            "NVENC requested, SDK/runtime unavailable -> Software H264".to_owned()
        }
        EncoderPreference::Software => "Software H264".to_owned(),
    }
}

pub fn nv_codec_sdk_present() -> bool {
    option_env!("EVERTYDESK_NV_CODEC_SDK_PATH").is_some()
}

pub fn nv_codec_sdk_label() -> String {
    match (
        option_env!("EVERTYDESK_NV_CODEC_SDK_VERSION"),
        option_env!("EVERTYDESK_NV_CODEC_SDK_PATH"),
    ) {
        (Some(version), Some(path)) => format!("NV Codec SDK {version}: {path}"),
        (None, Some(path)) => format!("NV Codec SDK: {path}"),
        _ => "NV Codec SDK: not found".to_owned(),
    }
}

fn detect_nvenc_status() -> NvencStatus {
    if !crate::nvenc::nvenc_supported_platform() {
        return NvencStatus::UnsupportedPlatform;
    }
    let api_ready = crate::nvenc::nvencode_api_available();
    let encoders = crate::nvenc::nvenc_encoder_names().unwrap_or_default();
    if !encoders.is_empty() {
        return NvencStatus::Available {
            encoders,
            api_ready,
        };
    }
    if api_ready {
        return NvencStatus::ApiReadyNoDirectEncoder;
    }
    if command_exists("nvidia-smi") {
        return NvencStatus::NvidiaGpuNoDirectEncoder;
    }

    NvencStatus::NotAvailable
}

fn detect_videotoolbox_status() -> VideoToolboxStatus {
    if !crate::videotoolbox::videotoolbox_supported_platform() {
        return VideoToolboxStatus::UnsupportedPlatform;
    }
    let Some(encoders) = crate::videotoolbox::videotoolbox_encoder_names() else {
        return VideoToolboxStatus::DirectEncoderUnavailable;
    };
    if encoders.is_empty() {
        VideoToolboxStatus::DirectEncoderUnavailable
    } else {
        VideoToolboxStatus::Available { encoders }
    }
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
