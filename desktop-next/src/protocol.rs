use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const IPC_PROTOCOL_VERSION: u16 = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ViewerStatus {
    Starting,
    Progress {
        percent: u8,
        message: String,
    },
    Info {
        message: String,
    },
    Connected {
        peer: String,
    },
    Latency {
        milliseconds: u32,
    },
    Codec {
        name: String,
    },
    Performance {
        fps_times_100: u32,
        input_kbps: u64,
        dropped_frames: u64,
        session_seconds: u64,
        reconnect_count: u32,
    },
    Recovery {
        reason: String,
    },
    ScreenshotSaved {
        path: String,
    },
    SessionSummary {
        remote_id: String,
        session_seconds: u64,
        reconnect_count: u32,
        #[serde(default)]
        end_reason: String,
    },
    Reconnecting {
        attempt: u32,
        delay_seconds: u64,
    },
    Heartbeat {
        sequence: u64,
    },
    ControlApplied {
        control: ViewerControl,
    },
    ControlState {
        control: ViewerControl,
    },
    Failed {
        error: String,
    },
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ViewerCommand {
    Disconnect,
    ToggleFullscreen,
    Reconnect,
    RefreshVideo,
    FocusWindow,
    SetInputEnabled { enabled: bool },
    SetAudioEnabled { enabled: bool },
    SetClipboardEnabled { enabled: bool },
    CycleDisplay { direction: i8 },
    SetQuality { quality: ConnectionQuality },
    SetScaling { scaling: ViewerScaling },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "control", rename_all = "snake_case")]
pub enum ViewerControl {
    InputEnabled { enabled: bool },
    AudioEnabled { enabled: bool },
    ClipboardEnabled { enabled: bool },
    Quality { quality: ConnectionQuality },
    Scaling { scaling: ViewerScaling },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionQuality {
    Smooth,
    #[default]
    Balanced,
    Sharp,
}

impl ConnectionQuality {
    pub const ALL: [Self; 3] = [Self::Smooth, Self::Balanced, Self::Sharp];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Smooth => "Плавность",
            Self::Balanced => "Баланс",
            Self::Sharp => "Качество",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Smooth => Self::Balanced,
            Self::Balanced => Self::Sharp,
            Self::Sharp => Self::Smooth,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewerScaling {
    #[default]
    SmoothFit,
    PixelPerfect,
}

impl ViewerScaling {
    pub const fn label(self) -> &'static str {
        match self {
            Self::SmoothFit => "Масштаб: плавно",
            Self::PixelPerfect => "Масштаб: 1:1",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::SmoothFit => Self::PixelPerfect,
            Self::PixelPerfect => Self::SmoothFit,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewerGameCodec {
    #[default]
    Auto,
    H265,
    H264,
    Av1,
}

impl ViewerGameCodec {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::H265 => "H265",
            Self::H264 => "H264",
            Self::Av1 => "AV1",
        }
    }
}

/// One-shot bootstrap message sent over the viewer's stdin.
///
/// Stdin is used instead of command-line arguments so credentials are not
/// exposed in process listings. Long-lived session events will use a named
/// pipe on Windows and a Unix-domain socket on Unix.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct ViewerBootstrap {
    pub protocol_version: u16,
    pub remote_id: String,
    pub password: String,
    #[serde(default = "default_true")]
    #[zeroize(skip)]
    pub audio_enabled: bool,
    #[serde(default)]
    #[zeroize(skip)]
    pub quality: ConnectionQuality,
    #[serde(default)]
    #[zeroize(skip)]
    pub scaling: ViewerScaling,
    #[serde(default)]
    #[zeroize(skip)]
    pub game_mode: bool,
    #[serde(default)]
    #[zeroize(skip)]
    pub game_codec: ViewerGameCodec,
    #[serde(default)]
    #[zeroize(skip)]
    pub game_evrt2_enabled: bool,
}

// Manual `Debug` (not derived) so a stray `{:?}` — including into the
// persistent `desktop-next.log` via `startup_log::append_log_line` — can
// never print the plaintext password.
impl std::fmt::Debug for ViewerBootstrap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewerBootstrap")
            .field("protocol_version", &self.protocol_version)
            .field("remote_id", &self.remote_id)
            .field("password", &"<redacted>")
            .field("audio_enabled", &self.audio_enabled)
            .field("quality", &self.quality)
            .field("scaling", &self.scaling)
            .field("game_mode", &self.game_mode)
            .field("game_codec", &self.game_codec)
            .field("game_evrt2_enabled", &self.game_evrt2_enabled)
            .finish()
    }
}

impl ViewerBootstrap {
    pub fn new(remote_id: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            protocol_version: IPC_PROTOCOL_VERSION,
            remote_id: normalize_bootstrap_remote_id(&remote_id.into()),
            password: password.into(),
            audio_enabled: true,
            quality: ConnectionQuality::default(),
            scaling: ViewerScaling::default(),
            game_mode: false,
            game_codec: ViewerGameCodec::default(),
            game_evrt2_enabled: false,
        }
    }

    pub fn with_quality(mut self, quality: ConnectionQuality) -> Self {
        self.quality = quality;
        self
    }

    pub fn with_audio(mut self, enabled: bool) -> Self {
        self.audio_enabled = enabled;
        self
    }

    pub fn with_scaling(mut self, scaling: ViewerScaling) -> Self {
        self.scaling = scaling;
        self
    }

    pub fn with_game_profile(
        mut self,
        game_mode: bool,
        codec: ViewerGameCodec,
        evrt2_enabled: bool,
    ) -> Self {
        self.game_mode = game_mode;
        self.game_codec = codec;
        self.game_evrt2_enabled = evrt2_enabled;
        self
    }

    pub fn validate(&self) -> Result<(), BootstrapError> {
        if self.protocol_version != IPC_PROTOCOL_VERSION {
            return Err(BootstrapError::UnsupportedProtocol {
                received: self.protocol_version,
                supported: IPC_PROTOCOL_VERSION,
            });
        }

        let remote_id = self.remote_id.trim();
        if remote_id.is_empty() {
            return Err(BootstrapError::MissingRemoteId);
        }
        if remote_id.len() > 128 {
            return Err(BootstrapError::RemoteIdTooLong);
        }
        if remote_id.chars().any(char::is_control) {
            return Err(BootstrapError::InvalidRemoteId);
        }

        Ok(())
    }
}

fn normalize_bootstrap_remote_id(remote_id: &str) -> String {
    remote_id
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-')
        .collect()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapError {
    UnsupportedProtocol { received: u16, supported: u16 },
    MissingRemoteId,
    RemoteIdTooLong,
    InvalidRemoteId,
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedProtocol {
                received,
                supported,
            } => write!(
                formatter,
                "unsupported IPC protocol {received}; this viewer supports {supported}"
            ),
            Self::MissingRemoteId => formatter.write_str("remote ID is required"),
            Self::RemoteIdTooLong => formatter.write_str("remote ID is too long"),
            Self::InvalidRemoteId => formatter.write_str("remote ID contains control characters"),
        }
    }
}

impl std::error::Error for BootstrapError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_round_trip_does_not_lose_credentials() {
        let request = ViewerBootstrap::new("123 456 789", "p@ss word");
        let encoded = serde_json::to_vec(&request).expect("serialize bootstrap");
        let decoded: ViewerBootstrap =
            serde_json::from_slice(&encoded).expect("deserialize bootstrap");

        assert_eq!(decoded, request);
        assert!(decoded.audio_enabled);
        assert_eq!(decoded.quality, ConnectionQuality::Balanced);
        assert_eq!(decoded.scaling, ViewerScaling::SmoothFit);
        assert_eq!(decoded.remote_id, "123456789");
        assert!(!decoded.game_mode);
        assert_eq!(decoded.game_codec, ViewerGameCodec::Auto);
        assert!(!decoded.game_evrt2_enabled);
        assert_eq!(decoded.validate(), Ok(()));
    }

    #[test]
    fn bootstrap_normalizes_remote_id_spacing_and_dashes() {
        let request = ViewerBootstrap::new(" 123 456-789 ", "");
        assert_eq!(request.remote_id, "123456789");
        assert_eq!(request.validate(), Ok(()));
    }

    #[test]
    fn bootstrap_still_rejects_non_spacing_control_characters() {
        let request = ViewerBootstrap::new("123\u{0000}456", "");
        assert!(matches!(
            request.validate(),
            Err(BootstrapError::InvalidRemoteId)
        ));
    }

    #[test]
    fn bootstrap_rejects_incompatible_protocols() {
        let mut request = ViewerBootstrap::new("123", "");
        request.protocol_version += 1;

        assert!(matches!(
            request.validate(),
            Err(BootstrapError::UnsupportedProtocol { .. })
        ));
    }

    #[test]
    fn old_bootstrap_without_audio_keeps_audio_enabled() {
        let encoded = format!(
            r#"{{"protocol_version":{},"remote_id":"123","password":""}}"#,
            IPC_PROTOCOL_VERSION
        );
        let decoded: ViewerBootstrap = serde_json::from_str(&encoded).unwrap();

        assert!(decoded.audio_enabled);
        assert!(!decoded.game_mode);
        assert_eq!(decoded.game_codec, ViewerGameCodec::Auto);
        assert!(!decoded.game_evrt2_enabled);
        assert_eq!(decoded.validate(), Ok(()));
    }

    #[test]
    fn bootstrap_carries_game_profile_without_breaking_credentials() {
        let request = ViewerBootstrap::new("123", "secret").with_game_profile(
            true,
            ViewerGameCodec::H264,
            true,
        );
        let encoded = serde_json::to_string(&request).expect("serialize bootstrap");
        assert!(encoded.contains(r#""game_mode":true"#));
        assert!(encoded.contains(r#""game_codec":"h264""#));
        assert!(encoded.contains(r#""game_evrt2_enabled":true"#));
        let decoded: ViewerBootstrap = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.remote_id, "123");
        assert_eq!(decoded.password, "secret");
        assert!(decoded.game_mode);
        assert_eq!(decoded.game_codec.label(), "H264");
        assert!(decoded.game_evrt2_enabled);
    }

    #[test]
    fn status_and_command_use_stable_tagged_json() {
        let status = ViewerStatus::Latency { milliseconds: 42 };
        let encoded = serde_json::to_string(&status).expect("serialize status");
        assert_eq!(encoded, r#"{"event":"latency","milliseconds":42}"#);

        assert_eq!(
            serde_json::to_string(&ViewerStatus::Codec {
                name: "H264".to_owned(),
            })
            .unwrap(),
            r#"{"event":"codec","name":"H264"}"#
        );

        let reconnecting = ViewerStatus::Reconnecting {
            attempt: 2,
            delay_seconds: 10,
        };
        assert_eq!(
            serde_json::to_string(&reconnecting).unwrap(),
            r#"{"event":"reconnecting","attempt":2,"delay_seconds":10}"#
        );

        let command = ViewerCommand::Disconnect;
        let encoded = serde_json::to_string(&command).expect("serialize command");
        assert_eq!(encoded, r#"{"command":"disconnect"}"#);

        let command = ViewerCommand::SetQuality {
            quality: ConnectionQuality::Sharp,
        };
        assert_eq!(
            serde_json::to_string(&command).unwrap(),
            r#"{"command":"set_quality","quality":"sharp"}"#
        );

        assert_eq!(
            serde_json::to_string(&ViewerCommand::Reconnect).unwrap(),
            r#"{"command":"reconnect"}"#
        );
        assert_eq!(
            serde_json::to_string(&ViewerCommand::RefreshVideo).unwrap(),
            r#"{"command":"refresh_video"}"#
        );
        assert_eq!(
            serde_json::to_string(&ViewerCommand::FocusWindow).unwrap(),
            r#"{"command":"focus_window"}"#
        );
        assert_eq!(
            serde_json::to_string(&ViewerCommand::SetInputEnabled { enabled: false }).unwrap(),
            r#"{"command":"set_input_enabled","enabled":false}"#
        );
        assert_eq!(
            serde_json::to_string(&ViewerCommand::SetAudioEnabled { enabled: false }).unwrap(),
            r#"{"command":"set_audio_enabled","enabled":false}"#
        );
        assert_eq!(
            serde_json::to_string(&ViewerCommand::CycleDisplay { direction: 1 }).unwrap(),
            r#"{"command":"cycle_display","direction":1}"#
        );
        assert_eq!(
            serde_json::to_string(&ViewerCommand::SetScaling {
                scaling: ViewerScaling::PixelPerfect,
            })
            .unwrap(),
            r#"{"command":"set_scaling","scaling":"pixel_perfect"}"#
        );

        assert_eq!(
            serde_json::to_string(&ViewerStatus::ControlApplied {
                control: ViewerControl::ClipboardEnabled { enabled: false },
            })
            .unwrap(),
            r#"{"event":"control_applied","control":{"control":"clipboard_enabled","enabled":false}}"#
        );
        assert_eq!(
            serde_json::to_string(&ViewerStatus::ControlApplied {
                control: ViewerControl::AudioEnabled { enabled: true },
            })
            .unwrap(),
            r#"{"event":"control_applied","control":{"control":"audio_enabled","enabled":true}}"#
        );
        assert_eq!(
            serde_json::to_string(&ViewerStatus::Heartbeat { sequence: 9 }).unwrap(),
            r#"{"event":"heartbeat","sequence":9}"#
        );
        assert_eq!(
            serde_json::to_string(&ViewerStatus::ControlState {
                control: ViewerControl::Scaling {
                    scaling: ViewerScaling::PixelPerfect,
                },
            })
            .unwrap(),
            r#"{"event":"control_state","control":{"control":"scaling","scaling":"pixel_perfect"}}"#
        );
    }

    #[test]
    fn quality_profiles_cycle_and_old_session_summaries_remain_compatible() {
        assert_eq!(
            ConnectionQuality::Smooth.next(),
            ConnectionQuality::Balanced
        );
        assert_eq!(ConnectionQuality::Balanced.next(), ConnectionQuality::Sharp);
        assert_eq!(ConnectionQuality::Sharp.next(), ConnectionQuality::Smooth);

        let summary: ViewerStatus = serde_json::from_str(
            r#"{"event":"session_summary","remote_id":"123","session_seconds":5,"reconnect_count":1}"#,
        )
        .unwrap();
        assert!(matches!(
            summary,
            ViewerStatus::SessionSummary { end_reason, .. } if end_reason.is_empty()
        ));
    }
}
