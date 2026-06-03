use std::{
    io::{Read, Write},
    net::TcpStream,
    net::ToSocketAddrs,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
#[cfg(feature = "live-h264")]
use openh264::formats::YUVSource;
use sha2::{Digest, Sha256};
#[cfg(feature = "live-vpx")]
use shiguredo_libvpx::{Decoder as VpxDecoder, DecoderCodec, DecoderConfig};

use crate::{
    rustdesk_proto::{
        decode_message, decode_peer_message, encode_message, encode_peer_message, misc,
        peer_message, rendezvous_message, video_frame, Chroma, CodecAbility, ConnType, ControlKey,
        CursorData, EncodedVideoFrames, KeyEvent, KeyboardMode, LoginRequest, Misc, MouseEvent,
        NatType, OnlineRequest, OptionMessage, PeerMessage, PreferCodec, PublicKey,
        PunchHoleFailure, PunchHoleRequest, RendezvousMessage, RequestRelay, ScreenshotRequest,
        ShellMessage, ShellMessageKind, SupportedDecoding, SwitchDisplay,
    },
    settings::{CodecPreference, DisplayConfig, ServerConfig},
};

const RENDEZVOUS_PORT: u16 = 21116;
const ONLINE_PORT: u16 = RENDEZVOUS_PORT - 1;
const RELAY_PORT: u16 = 21117;
const SESSION_TICK_MS: u64 = 16; // ~60 fps poll; keeps command latency ≤16 ms
const RELAY_STREAM_ATTEMPTS: u8 = 3;
const RELAY_BOOTSTRAP_WAIT_SECS: u64 = 12;
const RELAY_RESPONSE_BOOTSTRAP_WAIT_SECS: u64 = 30;
const RELAY_AUTH_WAIT_SECS: u64 = 120;
const RELAY_HANDSHAKE_POLL_MS: u64 = 500;

#[derive(Clone, Debug)]
pub struct ConnectionRequest {
    pub remote_id: String,
    pub password: String,
    pub server: ServerConfig,
    pub display: DisplayConfig,
}

#[derive(Clone, Debug)]
pub enum ConnectionState {
    Idle,
    RelayReady { remote_id: String },
    Failed(String),
}

#[derive(Clone, Debug)]
pub enum SessionEvent {
    Progress(u8, String),
    Connected(String),
    Frame {
        sid: String,
        codec: String,
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },
    ScreenshotStats {
        received: u64,
        pending: bool,
    },
    FrameMetrics {
        bytes: usize,
        queue_ms: u64,
        decode_ms: u64,
        dropped: usize,
    },
    VideoPacketMetrics {
        input_fps: f32,
        input_kbps: u64,
    },
    Displays(Vec<RemoteDisplay>),
    Info(String),
    Failed(String),
    Closed,
    /// New cursor shape (RGBA, decompressed). Client should cache by id.
    CursorData {
        id: u64,
        hotx: i32,
        hoty: i32,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    /// Server is reusing a previously sent cursor shape.
    CursorId {
        id: u64,
    },
    /// Cursor moved to this remote-screen position.
    CursorPosition {
        x: i32,
        y: i32,
    },
    /// Round-trip latency measured by the peer's TestDelay heartbeat (milliseconds).
    Latency(u32),
    ShellOutput(String),
    ShellClosed,
    ShellError(String),
}

#[derive(Clone, Debug)]
pub struct RemoteDisplay {
    pub index: i32,
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub x: i32,
    pub y: i32,
    pub cursor_embedded: bool,
}

#[allow(dead_code)] // some variants constructed conditionally or reserved for future use
#[derive(Clone, Debug)]
pub enum SessionCommand {
    MouseMove {
        x: i32,
        y: i32,
    },
    MouseDown {
        x: i32,
        y: i32,
    },
    MouseUp {
        x: i32,
        y: i32,
    },
    MouseRightDown {
        x: i32,
        y: i32,
    },
    MouseRightUp {
        x: i32,
        y: i32,
    },
    MouseMiddleDown {
        x: i32,
        y: i32,
    },
    MouseMiddleUp {
        x: i32,
        y: i32,
    },
    MouseWheel {
        x: i32,
        y: i32,
    },
    KeyText(String),
    KeyControl(ControlKey),
    KeyControlState {
        key: ControlKey,
        down: bool,
    },
    KeyTextWithModifiers {
        text: String,
        modifiers: Vec<ControlKey>,
    },
    KeyControlWithModifiers {
        key: ControlKey,
        modifiers: Vec<ControlKey>,
    },
    KeyEnter,
    Screenshot,
    SetDisplay(RemoteDisplay),
    SetAutoRefresh {
        enabled: bool,
        millis: u64,
    },
    RefreshVideo,
    SetVideoFps {
        fps: i32,
    },
    SetVideoProfile {
        fps: i32,
        codec: CodecPreference,
    },
    ShellStart,
    ShellInput(String),
    ShellStop,
    Close,
}

#[allow(dead_code)] // Vp8/Vp9 variants unused when live-vpx feature is off
enum DecoderInput {
    Png {
        sid: String,
        png: Vec<u8>,
        queued_at: Instant,
        bytes: usize,
    },
    H264 {
        sid: String,
        frames: EncodedVideoFrames,
        queued_at: Instant,
        bytes: usize,
    },
    Vp8 {
        sid: String,
        frames: EncodedVideoFrames,
        queued_at: Instant,
        bytes: usize,
    },
    Vp9 {
        sid: String,
        frames: EncodedVideoFrames,
        queued_at: Instant,
        bytes: usize,
    },
    H265 {
        sid: String,
        frames: EncodedVideoFrames,
        queued_at: Instant,
        bytes: usize,
        width: u32,
        height: u32,
    },
    Av1 {
        sid: String,
        frames: EncodedVideoFrames,
        queued_at: Instant,
        bytes: usize,
        width: u32,
        height: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // SkippedVideo is only used when live-vpx is disabled.
enum FrameSource {
    Screenshot,
    Video {
        bytes: usize,
    },
    /// Video frame arrived but we cannot decode it (e.g. VP9/VP8 without live-vpx).
    /// Counted separately so the session loop can warn the user.
    SkippedVideo {
        codec: &'static str,
    },
}

enum DecoderFeedback {
    BacklogTrimmed {
        dropped: usize,
    },
    DecodeFailed {
        codec: &'static str,
    },
    FrameDecoded {
        codec: &'static str,
        queue_ms: u64,
        decode_ms: u64,
    },
}

impl ConnectionState {
    pub fn as_text(&self) -> String {
        match self {
            Self::Idle => "idle".to_owned(),
            Self::RelayReady { remote_id } => {
                format!("relay session bootstrap complete for {remote_id}")
            }
            Self::Failed(err) => format!("error: {err}"),
        }
    }
}

pub struct TransportClient;

impl TransportClient {
    pub fn check_id_server(server: &ServerConfig) -> Result<(), String> {
        connect_tcp(&server.id_server, RENDEZVOUS_PORT).map(|_| ())
    }

    pub fn query_peer_online(
        server: &ServerConfig,
        local_id: &str,
        remote_id: &str,
    ) -> Result<bool, String> {
        let mut socket = connect_tcp(&server.id_server, ONLINE_PORT)?;
        socket
            .set_read_timeout(Some(Duration::from_secs(4)))
            .map_err(|err| format!("Failed to set online read timeout: {err}"))?;
        let request = RendezvousMessage {
            union: Some(rendezvous_message::Union::OnlineRequest(OnlineRequest {
                id: local_id.to_owned(),
                peers: vec![remote_id.to_owned()],
            })),
        };
        send_framed(&mut socket, &encode_message(&request))?;
        let response = decode_message(&read_framed(&mut socket)?)
            .map_err(|err| format!("Online response decode failed: {err}"))?;
        match response.union {
            Some(rendezvous_message::Union::OnlineResponse(response)) => Ok(response
                .states
                .first()
                .is_some_and(|byte| byte & 0x80 == 0x80)),
            _ => Err("Unexpected online response".to_owned()),
        }
    }

    pub fn connect_with_progress(
        request: ConnectionRequest,
        mut progress: impl FnMut(u8, String),
    ) -> Result<ConnectionState, String> {
        let (_relay_stream, peer_stage, _displays) =
            establish_session(request.clone(), &mut progress)?;

        progress(99, format!("Login stage: {peer_stage}"));
        Ok(ConnectionState::RelayReady {
            remote_id: request.remote_id,
        })
    }

    pub fn run_session(
        request: ConnectionRequest,
        commands: Receiver<SessionCommand>,
        events: Sender<SessionEvent>,
    ) {
        let display_config = request.display.clone();
        let mut codec_preference = display_config.codec;
        let initial_video_fps = display_config.target_fps.clamp(5, 60) as i32;
        let adaptive_quality = display_config.adaptive_quality;
        let min_video_fps = display_config
            .min_fps
            .clamp(5, display_config.target_fps.clamp(5, 60)) as i32;
        let mut emit_progress = |pct, message: String| {
            let _ = events.send(SessionEvent::Progress(pct, message));
        };

        let (mut relay, peer_stage, displays) = match establish_session(request, &mut emit_progress)
        {
            Ok(session) => session,
            Err(err) => {
                let _ = events.send(SessionEvent::Failed(err));
                return;
            }
        };

        eprintln!("[session] Connected: {peer_stage}");
        eprintln!("[session] Displays from login: {}", displays.len());
        let _ = events.send(SessionEvent::Connected(peer_stage));
        let mut known_displays = displays;
        if !known_displays.is_empty() {
            let _ = events.send(SessionEvent::Displays(known_displays.clone()));
        }
        let (frame_tx, frame_rx) = mpsc::channel::<DecoderInput>();
        let (decoder_feedback_tx, decoder_feedback_rx) = mpsc::channel::<DecoderFeedback>();
        let frame_events = events.clone();
        thread::spawn(move || decode_frame_loop(frame_rx, frame_events, decoder_feedback_tx));

        let _ = relay.set_read_timeout(Some(Duration::from_millis(SESSION_TICK_MS)));
        let mut screenshot_id = 0_u64;
        let mut total_reads = 0_u64;
        let mut total_timeouts = 0_u64;
        let mut current_display = 0_i32;
        let mut auto_refresh = true;
        let mut auto_refresh_millis = 80_u64;
        let mut screenshot_pending: bool;
        let mut screenshots_received = 0_u64;
        let mut peer_messages_seen = 0_u32;
        let mut live_video_seen = false;
        let mut last_frame_received = Instant::now();
        let mut video_metric_packets = 0_u64;
        let mut video_metric_bytes = 0_u64;
        let mut last_video_packet_metrics = Instant::now();
        let mut target_video_fps = initial_video_fps;
        let mut last_decoder_recovery: Option<Instant> = None;
        let mut last_adaptive_raise = Instant::now();
        let mut stable_decoded_frames = 0_u32;
        let mut h264_decode_failures = 0_u32;
        let mut vp9_decode_failures = 0_u32;
        let mut h265_decode_failures = 0_u32;
        let mut av1_decode_failures = 0_u32;
        let mut last_live_bootstrap = Instant::now();
        // Codec telemetry — reported to UI once on first encounter.
        let mut first_live_video_seen = false;
        let mut skipped_video_count = 0_u64;
        let mut skipped_video_last_log = 0_u64; // skipped_video_count at last log
                                                // Tracks when we last sent a screenshot request — drives time-based refresh.
        let mut last_screenshot_sent: Option<Instant> = None;
        // Subscribe to display 0 (SwitchDisplay) then trigger video start.
        // SwitchDisplay must come first — it's the one-time subscription trigger.
        let _ = send_switch_display_subscribe(&mut relay, current_display);
        let _ = send_video_start_messages(
            &mut relay,
            current_display,
            true,
            target_video_fps,
            codec_preference,
        );
        let _ = send_video_received(&mut relay);
        let _ = events.send(SessionEvent::Info(
            "Display subscribed; waiting for first frame".to_owned(),
        ));
        screenshot_pending = false;
        let _ = events.send(SessionEvent::ScreenshotStats {
            received: screenshots_received,
            pending: screenshot_pending,
        });

        loop {
            while let Ok(feedback) = decoder_feedback_rx.try_recv() {
                match feedback {
                    DecoderFeedback::BacklogTrimmed { dropped } => {
                        stable_decoded_frames = 0;
                        if adaptive_quality {
                            let cooldown_ready = last_decoder_recovery
                                .map(|instant| instant.elapsed() >= Duration::from_secs(4))
                                .unwrap_or(true);
                            if cooldown_ready {
                                let next_fps = lower_adaptive_fps(
                                    target_video_fps,
                                    min_video_fps,
                                    dropped >= 16,
                                );
                                if next_fps < target_video_fps {
                                    target_video_fps = next_fps;
                                    last_decoder_recovery = Some(Instant::now());
                                    last_adaptive_raise = Instant::now();
                                    let _ = events.send(SessionEvent::Info(format!(
                                        "Decoder backlog trimmed ({dropped}); lowering stream to {target_video_fps} fps"
                                    )));
                                    let _ = send_video_start_messages(
                                        &mut relay,
                                        current_display,
                                        false,
                                        target_video_fps,
                                        codec_preference,
                                    );
                                    last_live_bootstrap = Instant::now();
                                }
                            }
                        }
                    }
                    DecoderFeedback::DecodeFailed { codec } => {
                        stable_decoded_frames = 0;
                        match codec {
                            "H264" => h264_decode_failures += 1,
                            "VP9" => vp9_decode_failures += 1,
                            "H265" => h265_decode_failures += 1,
                            "AV1" => av1_decode_failures += 1,
                            _ => {}
                        }
                        let codec_switched = if codec == "VP9"
                            && vp9_decode_failures >= 3
                            && crate::video::h264_available()
                            && codec_preference != CodecPreference::H264
                        {
                            codec_preference = CodecPreference::H264;
                            vp9_decode_failures = 0;
                            let _ = events.send(SessionEvent::Info(
                                "VP9 decode is unstable; switching stream preference to H264"
                                    .to_owned(),
                            ));
                            true
                        } else if codec == "H264"
                            && h264_decode_failures >= 3
                            && crate::video::vp9_available()
                            && codec_preference != CodecPreference::Vp9
                        {
                            codec_preference = CodecPreference::Vp9;
                            h264_decode_failures = 0;
                            let _ = events.send(SessionEvent::Info(
                                "H264 decode is unstable; switching stream preference to VP9"
                                    .to_owned(),
                            ));
                            true
                        } else if codec == "AV1"
                            && av1_decode_failures >= 2
                            && codec_preference != fallback_codec_preference()
                        {
                            codec_preference = fallback_codec_preference();
                            av1_decode_failures = 0;
                            let _ = events.send(SessionEvent::Info(format!(
                                "AV1 decode is unstable; switching stream preference to {}",
                                codec_preference.label()
                            )));
                            true
                        } else if codec == "H265"
                            && h265_decode_failures >= 2
                            && codec_preference != fallback_codec_preference()
                        {
                            codec_preference = fallback_codec_preference();
                            h265_decode_failures = 0;
                            let _ = events.send(SessionEvent::Info(format!(
                                "H265 decode is unstable; switching stream preference to {}",
                                codec_preference.label()
                            )));
                            true
                        } else {
                            false
                        };

                        if codec_switched {
                            target_video_fps = target_video_fps.min(30).max(min_video_fps);
                            last_decoder_recovery = Some(Instant::now());
                            let _ = send_video_start_messages(
                                &mut relay,
                                current_display,
                                true,
                                target_video_fps,
                                codec_preference,
                            );
                            last_live_bootstrap = Instant::now();
                        } else if adaptive_quality {
                            let cooldown_ready = last_decoder_recovery
                                .map(|instant| instant.elapsed() >= Duration::from_secs(4))
                                .unwrap_or(true);
                            if cooldown_ready {
                                let next_fps =
                                    lower_adaptive_fps(target_video_fps, min_video_fps, false);
                                if next_fps < target_video_fps {
                                    target_video_fps = next_fps;
                                    last_decoder_recovery = Some(Instant::now());
                                    last_adaptive_raise = Instant::now();
                                    let _ = events.send(SessionEvent::Info(format!(
                                        "{codec} decode failed; lowering stream to {target_video_fps} fps"
                                    )));
                                    let _ = send_video_start_messages(
                                        &mut relay,
                                        current_display,
                                        false,
                                        target_video_fps,
                                        codec_preference,
                                    );
                                    last_live_bootstrap = Instant::now();
                                }
                            }
                        }
                    }
                    DecoderFeedback::FrameDecoded {
                        codec,
                        queue_ms,
                        decode_ms,
                    } => {
                        match codec {
                            "H264" => h264_decode_failures = 0,
                            "VP9" => vp9_decode_failures = 0,
                            "H265" => h265_decode_failures = 0,
                            "AV1" => av1_decode_failures = 0,
                            _ => {}
                        }
                        if adaptive_quality
                            && target_video_fps < initial_video_fps
                            && queue_ms <= 120
                            && decode_ms <= 45
                        {
                            stable_decoded_frames = stable_decoded_frames.saturating_add(1);
                            let needed = (target_video_fps.max(5) as u32).saturating_mul(8);
                            if stable_decoded_frames >= needed
                                && last_adaptive_raise.elapsed() >= Duration::from_secs(8)
                            {
                                let next_fps =
                                    raise_adaptive_fps(target_video_fps, initial_video_fps);
                                if next_fps > target_video_fps {
                                    target_video_fps = next_fps;
                                    stable_decoded_frames = 0;
                                    last_adaptive_raise = Instant::now();
                                    let _ = events.send(SessionEvent::Info(format!(
                                        "Video decode is stable; raising stream to {target_video_fps} fps"
                                    )));
                                    let _ = send_video_start_messages(
                                        &mut relay,
                                        current_display,
                                        false,
                                        target_video_fps,
                                        codec_preference,
                                    );
                                    last_live_bootstrap = Instant::now();
                                }
                            }
                        } else {
                            stable_decoded_frames = 0;
                        }
                    }
                }
            }

            let mut pending_mouse_move = None;
            while let Ok(command) = commands.try_recv() {
                match command {
                    SessionCommand::MouseMove { x, y } => {
                        pending_mouse_move = Some((x, y));
                    }
                    SessionCommand::MouseDown { x, y } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ =
                            send_mouse(&mut relay, MOUSE_BUTTON_LEFT << 3 | MOUSE_TYPE_DOWN, x, y);
                    }
                    SessionCommand::MouseUp { x, y } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ =
                            send_mouse(&mut relay, MOUSE_BUTTON_LEFT << 3 | MOUSE_TYPE_UP, x, y);
                    }
                    SessionCommand::MouseRightDown { x, y } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ =
                            send_mouse(&mut relay, MOUSE_BUTTON_RIGHT << 3 | MOUSE_TYPE_DOWN, x, y);
                    }
                    SessionCommand::MouseRightUp { x, y } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ =
                            send_mouse(&mut relay, MOUSE_BUTTON_RIGHT << 3 | MOUSE_TYPE_UP, x, y);
                    }
                    SessionCommand::MouseMiddleDown { x, y } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ =
                            send_mouse(&mut relay, MOUSE_BUTTON_WHEEL << 3 | MOUSE_TYPE_DOWN, x, y);
                    }
                    SessionCommand::MouseMiddleUp { x, y } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ =
                            send_mouse(&mut relay, MOUSE_BUTTON_WHEEL << 3 | MOUSE_TYPE_UP, x, y);
                    }
                    SessionCommand::MouseWheel { x, y } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ = send_mouse(&mut relay, MOUSE_TYPE_WHEEL, x, y);
                    }
                    SessionCommand::KeyText(text) => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ = send_text(&mut relay, &text);
                    }
                    SessionCommand::KeyControl(key) => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ = send_control_key(&mut relay, key);
                    }
                    SessionCommand::KeyControlState { key, down } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ = send_control_key_state(&mut relay, key, down);
                    }
                    SessionCommand::KeyTextWithModifiers { text, modifiers } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ = send_text_with_modifiers(&mut relay, &text, &modifiers);
                    }
                    SessionCommand::KeyControlWithModifiers { key, modifiers } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ = send_control_key_with_modifiers(&mut relay, key, &modifiers);
                    }
                    SessionCommand::KeyEnter => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ = send_control_key(&mut relay, ControlKey::Return);
                    }
                    SessionCommand::Screenshot => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        request_screenshot_once(
                            &mut relay,
                            &mut screenshot_id,
                            current_display,
                            &events,
                        );
                        last_screenshot_sent = Some(Instant::now());
                        screenshot_pending = true;
                        let _ = events.send(SessionEvent::ScreenshotStats {
                            received: screenshots_received,
                            pending: screenshot_pending,
                        });
                    }
                    SessionCommand::SetDisplay(display) => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        current_display = display.index.max(0);
                        live_video_seen = false;
                        let _ = send_switch_display(&mut relay, current_display, Some(&display));
                        let _ = send_video_start_messages(
                            &mut relay,
                            current_display,
                            false,
                            target_video_fps,
                            codec_preference,
                        );
                        last_live_bootstrap = Instant::now();
                        request_screenshot_once(
                            &mut relay,
                            &mut screenshot_id,
                            current_display,
                            &events,
                        );
                        last_screenshot_sent = Some(Instant::now());
                        screenshot_pending = true;
                        let _ = events.send(SessionEvent::ScreenshotStats {
                            received: screenshots_received,
                            pending: screenshot_pending,
                        });
                    }
                    SessionCommand::SetAutoRefresh { enabled, millis } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        auto_refresh = enabled;
                        auto_refresh_millis = millis.max(50);
                        let _ = send_video_start_messages(
                            &mut relay,
                            current_display,
                            false,
                            target_video_fps,
                            codec_preference,
                        );
                        last_live_bootstrap = Instant::now();
                    }
                    SessionCommand::RefreshVideo => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        live_video_seen = false;
                        last_decoder_recovery = Some(Instant::now());
                        let _ = events.send(SessionEvent::Info(format!(
                            "Fresh video requested at {target_video_fps} fps"
                        )));
                        let _ = send_video_start_messages(
                            &mut relay,
                            current_display,
                            false,
                            target_video_fps,
                            codec_preference,
                        );
                        last_live_bootstrap = Instant::now();
                    }
                    SessionCommand::SetVideoFps { fps } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        target_video_fps = fps.clamp(5, 60);
                        last_decoder_recovery = Some(Instant::now());
                        let _ = events.send(SessionEvent::Info(format!(
                            "Video fps set to {target_video_fps}"
                        )));
                        let _ = send_video_start_messages(
                            &mut relay,
                            current_display,
                            false,
                            target_video_fps,
                            codec_preference,
                        );
                        last_live_bootstrap = Instant::now();
                    }
                    SessionCommand::SetVideoProfile { fps, codec } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        target_video_fps = fps.clamp(5, 60);
                        codec_preference = codec;
                        live_video_seen = false;
                        last_decoder_recovery = Some(Instant::now());
                        let _ = events.send(SessionEvent::Info(format!(
                            "Video profile set to {} at {target_video_fps} fps",
                            codec_preference.label()
                        )));
                        let _ = send_video_start_messages(
                            &mut relay,
                            current_display,
                            true,
                            target_video_fps,
                            codec_preference,
                        );
                        last_live_bootstrap = Instant::now();
                    }
                    SessionCommand::ShellStart => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ = send_shell_message(&mut relay, ShellMessageKind::Start, "");
                    }
                    SessionCommand::ShellInput(input) => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ = send_shell_message(&mut relay, ShellMessageKind::Input, &input);
                    }
                    SessionCommand::ShellStop => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ = send_shell_message(&mut relay, ShellMessageKind::Stop, "");
                    }
                    SessionCommand::Close => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ = events.send(SessionEvent::Closed);
                        return;
                    }
                }
            }
            flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);

            total_reads += 1;
            let frame_source = match read_framed(&mut relay) {
                Ok(payload) => match decode_peer_message(&payload) {
                    Ok(message) => {
                        peer_messages_seen += 1;
                        if peer_messages_seen <= 20 {
                            let desc = describe_peer_message(&message);
                            eprintln!("[session] Peer msg #{peer_messages_seen}: {desc}");
                            let _ = events.send(SessionEvent::Info(format!(
                                "Peer msg #{peer_messages_seen}: {desc}"
                            )));
                        }
                        handle_session_message(
                            message,
                            &mut relay,
                            &events,
                            &frame_tx,
                            &mut known_displays,
                            current_display,
                            target_video_fps,
                            codec_preference,
                        )
                    }
                    Err(err) => {
                        eprintln!("[session] Peer decode FAILED: {err}");
                        None
                    }
                },
                Err(err) if is_timeout_error(&err) => {
                    total_timeouts += 1;
                    if total_timeouts % 200 == 0 {
                        eprintln!(
                            "[session] idle: reads={total_reads} timeouts={total_timeouts} screenshots={screenshots_received}"
                        );
                    }
                    None
                }
                Err(err) => {
                    eprintln!("[session] READ ERROR: {err}");
                    let _ = events.send(SessionEvent::Failed(err));
                    return;
                }
            };

            match frame_source {
                Some(FrameSource::SkippedVideo { codec }) => {
                    // Server is still sending a codec we cannot decode.
                    // Do NOT set live_video_seen — screenshots must keep firing.
                    skipped_video_count += 1;
                    // On first unsupported frame: aggressively re-negotiate codec.
                    // Some servers ignore the LoginRequest codec preference and need
                    // an explicit OptionMessage nudge after they've already started streaming.
                    if skipped_video_count == 1 {
                        let fallback = fallback_codec_preference();
                        let _ = send_codec_sync_options(&mut relay, target_video_fps, fallback);
                        eprintln!(
                            "[session] Server chose unsupported {codec}; re-sending {} request",
                            fallback.label()
                        );
                    }
                    // Log first occurrence and every 300 after that.
                    if skipped_video_count - skipped_video_last_log >= 300
                        || (skipped_video_count == 1 && skipped_video_last_log == 0)
                    {
                        skipped_video_last_log = skipped_video_count;
                        let _ = events.send(SessionEvent::Info(format!(
                            "Сервер шлёт {codec} (ignored: {skipped_video_count}). \
                             Запрошен поддерживаемый fallback, пока работаем в режиме скриншотов."
                        )));
                    }
                }
                Some(source) => {
                    screenshots_received += 1;
                    screenshot_pending = false;
                    last_frame_received = Instant::now();
                    if let FrameSource::Video { bytes } = source {
                        video_metric_packets = video_metric_packets.saturating_add(1);
                        video_metric_bytes = video_metric_bytes.saturating_add(bytes as u64);
                        let metric_elapsed = last_video_packet_metrics.elapsed();
                        if metric_elapsed >= Duration::from_millis(750) {
                            let secs = metric_elapsed.as_secs_f32().max(0.001);
                            let input_fps = video_metric_packets as f32 / secs;
                            let input_kbps =
                                ((video_metric_bytes as f32 * 8.0) / secs / 1000.0).round() as u64;
                            let _ = events.send(SessionEvent::VideoPacketMetrics {
                                input_fps,
                                input_kbps,
                            });
                            video_metric_packets = 0;
                            video_metric_bytes = 0;
                            last_video_packet_metrics = Instant::now();
                        }
                        if !first_live_video_seen {
                            first_live_video_seen = true;
                            let _ = events.send(SessionEvent::Info(
                                "Live video stream active; using low-latency frame path".to_owned(),
                            ));
                        }
                        live_video_seen = true;
                    }
                    let _ = events.send(SessionEvent::ScreenshotStats {
                        received: screenshots_received,
                        pending: screenshot_pending,
                    });
                }
                None => {}
            }

            // Time-based auto-refresh. Keep this to one display and avoid piling up PNG
            // screenshot requests; otherwise the UI shows old frames from the relay backlog.
            if auto_refresh {
                let elapsed =
                    last_screenshot_sent.map_or(Duration::from_secs(999), |t| t.elapsed());
                let request_expired =
                    elapsed >= Duration::from_millis((auto_refresh_millis * 8).clamp(700, 2000));
                let video_is_fresh =
                    live_video_seen && last_frame_received.elapsed() < Duration::from_millis(1200);
                let live_bootstrap_grace =
                    !live_video_seen && last_live_bootstrap.elapsed() < Duration::from_millis(2500);
                if !live_video_seen && last_live_bootstrap.elapsed() >= Duration::from_secs(3) {
                    let _ = send_video_start_messages(
                        &mut relay,
                        current_display,
                        true,
                        target_video_fps,
                        codec_preference,
                    );
                    let _ = send_video_received(&mut relay);
                    last_live_bootstrap = Instant::now();
                    let _ = events.send(SessionEvent::Info(
                        "Retrying live video before PNG fallback".to_owned(),
                    ));
                }
                if elapsed >= Duration::from_millis(auto_refresh_millis)
                    && !video_is_fresh
                    && !live_bootstrap_grace
                    && (!screenshot_pending || request_expired)
                {
                    if request_expired && screenshot_pending {
                        eprintln!("[screenshot] request expired; asking active display again");
                    }
                    last_screenshot_sent = Some(Instant::now());
                    screenshot_pending = true;
                    request_screenshot_once(
                        &mut relay,
                        &mut screenshot_id,
                        current_display,
                        &events,
                    );
                    let _ = events.send(SessionEvent::ScreenshotStats {
                        received: screenshots_received,
                        pending: screenshot_pending,
                    });
                }
            }
        }
    }
}

const MOUSE_TYPE_MOVE: i32 = 0;
const MOUSE_TYPE_DOWN: i32 = 1;
const MOUSE_TYPE_UP: i32 = 2;
const MOUSE_TYPE_WHEEL: i32 = 3;
const MOUSE_BUTTON_LEFT: i32 = 1;
const MOUSE_BUTTON_RIGHT: i32 = 2;
const MOUSE_BUTTON_WHEEL: i32 = 4;

fn flush_pending_mouse_move(relay: &mut TcpStream, pending: &mut Option<(i32, i32)>) {
    if let Some((x, y)) = pending.take() {
        let _ = send_mouse(relay, MOUSE_TYPE_MOVE, x, y);
    }
}

fn lower_adaptive_fps(current: i32, min_fps: i32, severe: bool) -> i32 {
    let ladder = [60, 30, 20, 15, 10, 5];
    let mut next = current.clamp(5, 60);
    let steps = if severe { 2 } else { 1 };
    for _ in 0..steps {
        if let Some(candidate) = ladder
            .iter()
            .copied()
            .filter(|fps| *fps < next)
            .find(|fps| *fps >= min_fps)
        {
            next = candidate;
        }
    }
    next.max(min_fps.clamp(5, 60))
}

fn raise_adaptive_fps(current: i32, max_fps: i32) -> i32 {
    let ladder = [5, 10, 15, 20, 30, 60];
    ladder
        .iter()
        .copied()
        .find(|fps| *fps > current && *fps <= max_fps)
        .unwrap_or(current)
}

fn establish_session(
    request: ConnectionRequest,
    progress: &mut impl FnMut(u8, String),
) -> Result<(TcpStream, String, Vec<RemoteDisplay>), String> {
    progress(5, "Validating input".to_owned());
    if request.remote_id.is_empty() {
        return Err("Enter remote ID".to_owned());
    }
    if false && request.password.is_empty() {
        return Err("Enter remote password".to_owned());
    }

    progress(15, "Validating server public key".to_owned());
    validate_public_key(&request.server.public_key)?;

    progress(30, "Connecting to ID server".to_owned());
    let mut rendezvous = connect_tcp(&request.server.id_server, RENDEZVOUS_PORT)?;

    progress(45, "Connecting to Relay server".to_owned());
    let _relay = connect_tcp(&request.server.relay_server, RELAY_PORT)?;

    progress(60, "Sending RustDesk PunchHoleRequest protobuf".to_owned());
    let message = RendezvousMessage {
        union: Some(rendezvous_message::Union::PunchHoleRequest(
            PunchHoleRequest {
                id: request.remote_id.clone(),
                nat_type: NatType::UnknownNat as i32,
                licence_key: request.server.public_key.clone(),
                conn_type: ConnType::DefaultConn as i32,
                token: String::new(),
                version: "1.4.6".to_owned(),
                udp_port: 0,
                force_relay: true,
                upnp_port: 0,
                socket_addr_v6: Vec::new(),
            },
        )),
    };
    send_framed(&mut rendezvous, &encode_message(&message))?;

    progress(80, "Waiting for rendezvous response".to_owned());
    rendezvous
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|err| format!("Failed to set read timeout: {err}"))?;
    let response = read_framed(&mut rendezvous)?;
    let decoded = decode_message(&response).map_err(|err| format!("Decode failed: {err}"))?;
    let rendezvous = describe_rendezvous_response(&decoded)?;

    progress(85, "Rendezvous protobuf response decoded".to_owned());
    let relay_uuid_from_rendezvous = rendezvous.relay_uuid.clone();
    let relay_server = rendezvous
        .relay_server
        .unwrap_or_else(|| request.server.relay_server.clone());
    let secure_relay = rendezvous.has_signed_pk;
    let initial_video_fps = request.display.target_fps.clamp(5, 60) as i32;
    let codec_preference = request.display.codec;

    // Relay connection: retry because the peer may not have joined the relay
    // stream yet when the operator side opens it.
    let mut last_err = String::new();
    let max_retries = RELAY_STREAM_ATTEMPTS.saturating_sub(1);
    for attempt in 0..RELAY_STREAM_ATTEMPTS {
        let use_rendezvous_uuid = attempt == 0 && relay_uuid_from_rendezvous.is_some();
        if use_rendezvous_uuid {
            progress(
                88,
                "Using relay reservation from rendezvous response".to_owned(),
            );
        } else if attempt > 0 {
            progress(
                88 + attempt * 2,
                format!("Relay retry {attempt}/{max_retries} (peer not ready yet): {last_err}"),
            );
        } else {
            progress(88, "Requesting relay reservation".to_owned());
        }

        let relay_uuid = if use_rendezvous_uuid {
            relay_uuid_from_rendezvous
                .clone()
                .expect("checked by use_rendezvous_uuid")
        } else {
            request_relay_reservation(
                &request.server.id_server,
                &request.remote_id,
                &relay_server,
                &request.server.public_key,
                secure_relay,
            )?
        };
        let bootstrap_wait_secs = if use_rendezvous_uuid {
            RELAY_RESPONSE_BOOTSTRAP_WAIT_SECS
        } else {
            RELAY_BOOTSTRAP_WAIT_SECS
        };

        progress(92, "Opening relay stream".to_owned());
        let mut relay_stream = open_relay_stream(
            &relay_server,
            &request.remote_id,
            &relay_uuid,
            &request.server.public_key,
            secure_relay,
        )?;

        progress(96, "Waiting for peer secure/login response".to_owned());
        match read_initial_peer_stage(
            &mut relay_stream,
            &request.password,
            &request.remote_id,
            initial_video_fps,
            codec_preference,
            bootstrap_wait_secs,
            progress,
        ) {
            Ok((peer_stage, displays)) => return Ok((relay_stream, peer_stage, displays)),
            Err(err) => last_err = err,
        }
    }
    Err(last_err)
}

fn validate_public_key(public_key: &str) -> Result<(), String> {
    let decoded = STANDARD
        .decode(public_key)
        .map_err(|err| format!("Invalid public key base64: {err}"))?;
    if decoded.len() != 32 {
        return Err(format!(
            "Invalid public key length: expected 32 bytes, got {}",
            decoded.len()
        ));
    }
    Ok(())
}

pub fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, String> {
    let mut last_error = None;
    let (host, port) = split_host_port(host, port);
    let addrs = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|err| format!("{host}:{port}: DNS error: {err}"))?;

    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
            Ok(stream) => {
                configure_tcp_stream(&stream);
                return Ok(stream);
            }
            Err(err) => last_error = Some(err),
        }
    }

    Err(format!(
        "{host}:{port} unreachable: {}",
        last_error
            .map(|err| err.to_string())
            .unwrap_or_else(|| "no resolved addresses".to_owned())
    ))
}

fn configure_tcp_stream(stream: &TcpStream) {
    // Remote-control traffic is dominated by small control packets and framed
    // video chunks. Disabling Nagle keeps cursor/key events from waiting behind
    // the TCP coalescing timer on relay connections.
    let _ = stream.set_nodelay(true);
}

fn split_host_port(host: &str, default_port: u16) -> (String, u16) {
    let trimmed = host.trim();
    if let Some((name, port)) = trimmed.rsplit_once(':') {
        if !name.is_empty() && !name.contains(']') {
            if let Ok(port) = port.parse::<u16>() {
                return (name.to_owned(), port);
            }
        }
    }
    (trimmed.to_owned(), default_port)
}

pub fn send_framed(stream: &mut TcpStream, payload: &[u8]) -> Result<(), String> {
    let header = encode_frame_len(payload.len())?;
    stream
        .write_all(&header)
        .and_then(|_| stream.write_all(payload))
        .map_err(|err| format!("TCP write failed: {err}"))
}

pub fn read_framed(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut first = [0_u8; 1];
    stream
        .read_exact(&mut first)
        .map_err(|err| format!("TCP read header failed: {err}"))?;

    // First byte arrived — a message is in flight. Extend timeout to 5 s so
    // large video frames don't get cut by the short SESSION_TICK_MS poll timeout.
    // Save caller's timeout and restore it when done so this function is reusable
    // in both the handshake phase (2 s) and the session loop (SESSION_TICK_MS).
    let prev_timeout = stream.read_timeout().ok().flatten();
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

    let head_len = ((first[0] & 0x3) + 1) as usize;
    let mut header = vec![0_u8; head_len];
    header[0] = first[0];
    if head_len > 1 {
        stream
            .read_exact(&mut header[1..])
            .map_err(|err| format!("TCP read header failed: {err}"))?;
    }

    let mut len = header[0] as usize;
    if head_len > 1 {
        len |= (header[1] as usize) << 8;
    }
    if head_len > 2 {
        len |= (header[2] as usize) << 16;
    }
    if head_len > 3 {
        len |= (header[3] as usize) << 24;
    }
    len >>= 2;

    let mut payload = vec![0_u8; len];
    let read_result = stream
        .read_exact(&mut payload)
        .map_err(|err| format!("TCP read payload failed: {err}"));

    // Restore caller's timeout regardless of success/failure.
    let _ = stream.set_read_timeout(prev_timeout);

    read_result?;
    Ok(payload)
}

pub fn encode_frame_len(len: usize) -> Result<Vec<u8>, String> {
    if len <= 0x3f {
        Ok(vec![(len << 2) as u8])
    } else if len <= 0x3fff {
        Ok(((len << 2) as u16 | 0x1).to_le_bytes().to_vec())
    } else if len <= 0x3fffff {
        let header = (len << 2) as u32 | 0x2;
        Ok(vec![
            (header & 0xff) as u8,
            ((header >> 8) & 0xff) as u8,
            ((header >> 16) & 0xff) as u8,
        ])
    } else if len <= 0x3fffffff {
        Ok(((len << 2) as u32 | 0x3).to_le_bytes().to_vec())
    } else {
        Err("Frame too large".to_owned())
    }
}

fn request_relay_reservation(
    rendezvous_server: &str,
    remote_id: &str,
    relay_server: &str,
    public_key: &str,
    secure: bool,
) -> Result<String, String> {
    let mut socket = connect_tcp(rendezvous_server, RENDEZVOUS_PORT)?;
    let uuid = uuid::Uuid::new_v4().to_string();
    let request = RendezvousMessage {
        union: Some(rendezvous_message::Union::RequestRelay(RequestRelay {
            id: remote_id.to_owned(),
            uuid: uuid.clone(),
            socket_addr: Vec::new(),
            relay_server: relay_server.to_owned(),
            secure,
            licence_key: public_key.to_owned(),
            conn_type: ConnType::DefaultConn as i32,
            token: String::new(),
        })),
    };
    send_framed(&mut socket, &encode_message(&request))?;
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|err| format!("Failed to set read timeout: {err}"))?;
    let response = decode_message(&read_framed(&mut socket)?)
        .map_err(|err| format!("Relay reservation decode failed: {err}"))?;
    match response.union {
        Some(rendezvous_message::Union::RelayResponse(response)) => {
            if response.refuse_reason.is_empty() {
                Ok(uuid)
            } else {
                Err(response.refuse_reason)
            }
        }
        _ => Err("Unexpected relay reservation response".to_owned()),
    }
}

fn open_relay_stream(
    relay_server: &str,
    remote_id: &str,
    relay_uuid: &str,
    public_key: &str,
    secure: bool,
) -> Result<TcpStream, String> {
    let mut relay = connect_tcp(relay_server, RELAY_PORT)?;
    let request = RendezvousMessage {
        union: Some(rendezvous_message::Union::RequestRelay(RequestRelay {
            id: remote_id.to_owned(),
            uuid: relay_uuid.to_owned(),
            socket_addr: Vec::new(),
            relay_server: String::new(),
            secure,
            licence_key: public_key.to_owned(),
            conn_type: ConnType::DefaultConn as i32,
            token: String::new(),
        })),
    };
    send_framed(&mut relay, &encode_message(&request))?;
    Ok(relay)
}

fn read_initial_peer_stage(
    relay: &mut TcpStream,
    password: &str,
    remote_id: &str,
    fps: i32,
    codec_preference: CodecPreference,
    bootstrap_wait_secs: u64,
    progress: &mut impl FnMut(u8, String),
) -> Result<(String, Vec<RemoteDisplay>), String> {
    relay
        .set_read_timeout(Some(Duration::from_millis(RELAY_HANDSHAKE_POLL_MS)))
        .map_err(|err| format!("Failed to set relay read timeout: {err}"))?;
    let mut sent_login = false;
    let mut seen_peer_message = false;
    let wait_remote_accept = password.is_empty();
    let started = Instant::now();
    let bootstrap_deadline = started + Duration::from_secs(bootstrap_wait_secs);
    let auth_deadline = started + Duration::from_secs(RELAY_AUTH_WAIT_SECS);
    let mut last_wait_progress = started;
    loop {
        let payload = match read_framed(relay) {
            Ok(payload) => payload,
            Err(err) if is_timeout_error(&err) => {
                let now = Instant::now();
                if now.duration_since(last_wait_progress) >= Duration::from_secs(3) {
                    if seen_peer_message {
                        let stage = if wait_remote_accept && sent_login {
                            "Waiting for remote approval/login response"
                        } else if sent_login {
                            "Waiting for login response"
                        } else {
                            "Waiting for peer secure/login response"
                        };
                        progress(
                            96,
                            format!(
                                "{stage} ({}s/{}s)",
                                now.duration_since(started).as_secs(),
                                RELAY_AUTH_WAIT_SECS
                            ),
                        );
                    } else {
                        progress(
                            96,
                            format!(
                                "Waiting for host to join relay ({}s/{}s)",
                                now.duration_since(started).as_secs(),
                                bootstrap_wait_secs
                            ),
                        );
                    }
                    last_wait_progress = now;
                }
                if !seen_peer_message {
                    if now < bootstrap_deadline {
                        continue;
                    }
                    return Err(format!(
                        "Relay opened, but peer did not join within {bootstrap_wait_secs}s: {err}"
                    ));
                }
                if now < auth_deadline {
                    continue;
                }
                let stage = if wait_remote_accept && sent_login {
                    "remote approval/login response"
                } else if sent_login {
                    "login response"
                } else {
                    "peer secure/login response"
                };
                return Err(format!(
                    "Timed out waiting for {stage} after {RELAY_AUTH_WAIT_SECS}s: {err}"
                ));
            }
            Err(err) => {
                return Err(format!(
                    "Relay opened, but no peer secure/login message arrived: {err}"
                ));
            }
        };
        seen_peer_message = true;
        let message = decode_peer_message(&payload)
            .map_err(|err| format!("Peer message decode failed: {err}"))?;

        match message.union {
            Some(peer_message::Union::SignedId(_)) => {
                let fallback = PeerMessage {
                    union: Some(peer_message::Union::PublicKey(PublicKey {
                        asymmetric_value: Vec::new(),
                        symmetric_value: Vec::new(),
                    })),
                };
                send_framed(relay, &encode_peer_message(&fallback))?;
            }
            Some(peer_message::Union::Hash(hash)) => {
                let login = build_login_request(
                    password,
                    &hash.salt,
                    &hash.challenge,
                    remote_id,
                    fps,
                    codec_preference,
                );
                send_framed(relay, &encode_peer_message(&login))?;
                sent_login = true;
            }
            Some(peer_message::Union::LoginResponse(response)) => {
                if login_response_is_remote_accept_wait(&response) {
                    continue;
                }
                send_selected_windows_session(relay, &response)?;
                let displays = displays_from_login_response(&response);
                let login = describe_login_response(response, sent_login)?;
                send_switch_display_subscribe(relay, 0)?;
                send_video_start_messages(relay, 0, true, fps, codec_preference)?;
                return Ok((
                    format!("{login}; screenshot/control channel ready"),
                    displays,
                ));
            }
            Some(peer_message::Union::PeerInfo(info)) => {
                send_selected_windows_session_from_peer_info(relay, &info)?;
                let login = format!(
                    "authorized; peer info received: {} {} {}",
                    info.hostname, info.platform, info.version
                );
                let displays = displays_from_peer_info(&info);
                send_switch_display_subscribe(relay, 0)?;
                send_video_start_messages(relay, 0, true, fps, codec_preference)?;
                return Ok((
                    format!("{login}; screenshot/control channel ready"),
                    displays,
                ));
            }
            Some(peer_message::Union::PublicKey(_)) => {}
            Some(peer_message::Union::TestDelay(delay)) => {
                echo_test_delay(relay, delay)?;
            }
            Some(peer_message::Union::Misc(_)) => {}
            Some(peer_message::Union::Shell(_)) => {}
            Some(peer_message::Union::MouseEvent(_))
            | Some(peer_message::Union::KeyEvent(_))
            | Some(peer_message::Union::ScreenshotRequest(_))
            | Some(peer_message::Union::ScreenshotResponse(_))
            | Some(peer_message::Union::CursorData(_))
            | Some(peer_message::Union::CursorId(_))
            | Some(peer_message::Union::CursorPosition(_)) => {}
            Some(peer_message::Union::VideoFrame(frame)) => {
                return Ok((
                    format!(
                        "video before login response: {}",
                        describe_video_frame(&frame)
                    ),
                    Vec::new(),
                ));
            }
            Some(peer_message::Union::LoginRequest(_)) => {
                return Err("Unexpected login-request received from peer".to_owned());
            }
            None => {
                // RustDesk can send an empty message while falling back from secure
                // negotiation. Answer with an empty public-key fallback and keep
                // reading until the login/hash/video stage appears.
                let fallback = PeerMessage {
                    union: Some(peer_message::Union::PublicKey(PublicKey {
                        asymmetric_value: Vec::new(),
                        symmetric_value: Vec::new(),
                    })),
                };
                send_framed(relay, &encode_peer_message(&fallback))?;
            }
        }
    }
}

fn send_video_start_messages(
    relay: &mut TcpStream,
    display: i32,
    refresh_all: bool,
    fps: i32,
    codec_preference: CodecPreference,
) -> Result<(), String> {
    send_codec_sync_options(relay, fps, codec_preference)?;

    if refresh_all {
        let refresh_all_msg = PeerMessage {
            union: Some(peer_message::Union::Misc(Misc {
                union: Some(misc::Union::RefreshVideo(true)),
            })),
        };
        send_framed(relay, &encode_peer_message(&refresh_all_msg))?;
    }

    // RefreshVideoDisplay nudges the server to restart capture for this display.
    let refresh_display = PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::RefreshVideoDisplay(display.max(0))),
        })),
    };
    send_framed(relay, &encode_peer_message(&refresh_display))
}

/// Send SwitchDisplay — the one-time trigger that tells the server "start capturing
/// display N for this session". Call only at connection startup or explicit display
/// switch, NOT in the periodic refresh loop (would create a SwitchDisplay feedback loop).
fn send_switch_display_subscribe(relay: &mut TcpStream, display: i32) -> Result<(), String> {
    let switch_msg = PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::SwitchDisplay(SwitchDisplay {
                display: display.max(0),
                ..Default::default()
            })),
        })),
    };
    send_framed(relay, &encode_peer_message(&switch_msg))
}

fn send_codec_sync_options(
    relay: &mut TcpStream,
    fps: i32,
    codec_preference: CodecPreference,
) -> Result<(), String> {
    let message = PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::Option(OptionMessage {
                supported_decoding: Some(supported_decoding(codec_preference)),
                custom_fps: fps.clamp(5, 60),
            })),
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn supported_decoding(codec_preference: CodecPreference) -> SupportedDecoding {
    let vp9_capable = crate::video::vp9_available();
    let h264_capable = crate::video::h264_available();
    let h265_capable = crate::video::h265_available();
    let av1_capable = crate::video::av1_available();
    let prefer = preferred_codec(
        codec_preference,
        h264_capable,
        h265_capable,
        av1_capable,
        vp9_capable,
    );
    SupportedDecoding {
        ability_vp9: i32::from(vp9_capable),
        ability_h264: i32::from(h264_capable),
        ability_h265: i32::from(h265_capable),
        prefer: prefer as i32,
        ability_vp8: i32::from(crate::video::vp8_available()),
        ability_av1: i32::from(av1_capable),
        i444: Some(CodecAbility {
            vp8: false,
            vp9: cfg!(any(feature = "live-vpx", feature = "live-vpx-system")),
            av1: av1_capable,
            h264: false,
            h265: h265_capable,
        }),
        prefer_chroma: Chroma::I420 as i32,
    }
}

fn preferred_codec(
    codec_preference: CodecPreference,
    h264_capable: bool,
    h265_capable: bool,
    av1_capable: bool,
    vp9_capable: bool,
) -> PreferCodec {
    match codec_preference {
        CodecPreference::Av1 if av1_capable => PreferCodec::Av1,
        CodecPreference::H265 if h265_capable => PreferCodec::H265,
        CodecPreference::H264 if h264_capable => PreferCodec::H264,
        CodecPreference::Vp9 if vp9_capable => PreferCodec::Vp9,
        CodecPreference::Auto if av1_capable => PreferCodec::Av1,
        CodecPreference::Auto if h265_capable => PreferCodec::H265,
        CodecPreference::Auto if h264_capable => PreferCodec::H264,
        CodecPreference::Auto if vp9_capable => PreferCodec::Vp9,
        _ if av1_capable => PreferCodec::Av1,
        _ if h265_capable => PreferCodec::H265,
        _ if h264_capable => PreferCodec::H264,
        _ if vp9_capable => PreferCodec::Vp9,
        _ => PreferCodec::Auto,
    }
}

fn fallback_codec_preference() -> CodecPreference {
    if crate::video::h264_available() {
        CodecPreference::H264
    } else if crate::video::vp9_available() {
        CodecPreference::Vp9
    } else if crate::video::h265_available() {
        CodecPreference::H265
    } else if crate::video::av1_available() {
        CodecPreference::Av1
    } else {
        CodecPreference::Auto
    }
}

fn send_selected_windows_session(
    relay: &mut TcpStream,
    response: &crate::rustdesk_proto::LoginResponse,
) -> Result<(), String> {
    if let Some(crate::rustdesk_proto::login_response::Union::PeerInfo(info)) = &response.union {
        send_selected_windows_session_from_peer_info(relay, info)?;
    }
    Ok(())
}

fn send_selected_windows_session_from_peer_info(
    relay: &mut TcpStream,
    info: &crate::rustdesk_proto::PeerInfo,
) -> Result<(), String> {
    let Some(windows_sessions) = &info.windows_sessions else {
        return Ok(());
    };
    if windows_sessions.sessions.is_empty() || windows_sessions.current_sid == 0 {
        return Ok(());
    }
    let message = PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::SelectedSid(windows_sessions.current_sid)),
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

#[allow(dead_code)]
fn wait_for_video_probe(relay: &mut TcpStream) -> Result<String, String> {
    relay
        .set_read_timeout(Some(Duration::from_secs(8)))
        .map_err(|err| format!("Failed to set relay read timeout: {err}"))?;

    for attempt in 0..24 {
        let payload = match read_framed(relay) {
            Ok(payload) => payload,
            Err(_) if attempt < 23 => {
                send_video_start_messages(relay, 0, true, 30, CodecPreference::Auto)?;
                continue;
            }
            Err(err) => {
                return Err(format!(
                    "Authorized, but no video/control message arrived: {err}"
                ));
            }
        };
        let message = decode_peer_message(&payload)
            .map_err(|err| format!("Post-login message decode failed: {err}"))?;
        match message.union {
            Some(peer_message::Union::VideoFrame(frame)) => {
                send_video_received(relay)?;
                return Ok(format!(
                    "first video frame: {}",
                    describe_video_frame(&frame)
                ));
            }
            Some(peer_message::Union::PeerInfo(info)) => {
                let displays = info
                    .displays
                    .iter()
                    .map(|d| format!("{}x{}", d.width, d.height))
                    .collect::<Vec<_>>()
                    .join(", ");
                if !displays.is_empty() {
                    return Ok(format!("peer displays: {displays}; waiting for video next"));
                }
            }
            Some(peer_message::Union::LoginResponse(response)) => {
                send_selected_windows_session(relay, &response)?;
                let _ = describe_login_response(response, true)?;
            }
            Some(peer_message::Union::TestDelay(delay)) => {
                echo_test_delay(relay, delay)?;
            }
            Some(peer_message::Union::Misc(_)) => {}
            Some(peer_message::Union::Hash(_))
            | Some(peer_message::Union::SignedId(_))
            | Some(peer_message::Union::PublicKey(_))
            | Some(peer_message::Union::LoginRequest(_))
            | Some(peer_message::Union::Shell(_))
            | Some(peer_message::Union::MouseEvent(_))
            | Some(peer_message::Union::KeyEvent(_))
            | Some(peer_message::Union::ScreenshotRequest(_))
            | Some(peer_message::Union::ScreenshotResponse(_))
            | Some(peer_message::Union::CursorData(_))
            | Some(peer_message::Union::CursorId(_))
            | Some(peer_message::Union::CursorPosition(_))
            | None => {}
        }
    }

    Ok("authorized; no video frame received during probe window".to_owned())
}

fn echo_test_delay(
    relay: &mut TcpStream,
    mut delay: crate::rustdesk_proto::TestDelay,
) -> Result<(), String> {
    if delay.from_client {
        return Ok(());
    }
    delay.from_client = true;
    let message = PeerMessage {
        union: Some(peer_message::Union::TestDelay(delay)),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn send_video_received(relay: &mut TcpStream) -> Result<(), String> {
    let message = PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::VideoReceived(true)),
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn send_shell_message(
    relay: &mut TcpStream,
    kind: ShellMessageKind,
    data: &str,
) -> Result<(), String> {
    let message = PeerMessage {
        union: Some(peer_message::Union::Shell(ShellMessage {
            kind: kind as i32,
            data: data.to_owned(),
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn handle_session_message(
    message: PeerMessage,
    relay: &mut TcpStream,
    events: &Sender<SessionEvent>,
    frame_tx: &Sender<DecoderInput>,
    known_displays: &mut Vec<RemoteDisplay>,
    current_display: i32,
    target_video_fps: i32,
    codec_preference: CodecPreference,
) -> Option<FrameSource> {
    match message.union {
        Some(peer_message::Union::ScreenshotResponse(response)) => {
            if response.msg.is_empty() && !response.data.is_empty() {
                match frame_tx.send(DecoderInput::Png {
                    sid: response.sid,
                    bytes: response.data.len(),
                    png: response.data,
                    queued_at: Instant::now(),
                }) {
                    Ok(()) => {}
                    Err(_) => {
                        let _ = events.send(SessionEvent::Info(
                            "Frame decoder stopped unexpectedly".to_owned(),
                        ));
                    }
                };
                Some(FrameSource::Screenshot)
            } else if !response.msg.is_empty() {
                let _ = events.send(SessionEvent::Info(format!(
                    "Screenshot failed: {}",
                    response.msg
                )));
                Some(FrameSource::Screenshot)
            } else {
                Some(FrameSource::Screenshot)
            }
        }
        Some(peer_message::Union::VideoFrame(frame)) => {
            let _ = send_video_received(relay);
            let description = describe_video_frame(&frame);
            match frame.union {
                Some(video_frame::Union::H264s(frames)) => {
                    let bytes = encoded_frame_bytes(&frames);
                    let sid = encoded_sid("h264", &frames);
                    queue_encoded_frame(
                        frame_tx,
                        DecoderInput::H264 {
                            sid,
                            bytes,
                            queued_at: Instant::now(),
                            frames,
                        },
                        events,
                    );
                    Some(FrameSource::Video { bytes })
                }
                Some(video_frame::Union::Vp8s(frames)) => {
                    // Only count VP8 as live video if we can actually decode it.
                    // Without live-vpx, VP8 frames arrive but produce nothing on screen;
                    // counting them as "live" would suppress screenshot refresh forever.
                    #[cfg(feature = "live-vpx")]
                    {
                        let bytes = encoded_frame_bytes(&frames);
                        let sid = encoded_sid("vp8", &frames);
                        queue_encoded_frame(
                            frame_tx,
                            DecoderInput::Vp8 {
                                sid,
                                bytes,
                                queued_at: Instant::now(),
                                frames,
                            },
                            events,
                        );
                        Some(FrameSource::Video { bytes })
                    }
                    #[cfg(not(feature = "live-vpx"))]
                    {
                        let _ = (frame_tx, frames, events);
                        Some(FrameSource::SkippedVideo { codec: "VP8" }) // VP8 arrived but undecoded
                    }
                }
                Some(video_frame::Union::Vp9s(frames)) => {
                    // Queue for decoding if any VP9 decoder is available:
                    //   • live-vpx         → libvpx (built from source)
                    //   • live-vpx-system  → system libvpx (apt: libvpx-dev)
                    //   • live-vp9-mf      → Windows Media Foundation (Win10 1803+)
                    #[cfg(any(
                        feature = "live-vpx",
                        feature = "live-vpx-system",
                        all(feature = "live-vp9-mf", target_os = "windows")
                    ))]
                    {
                        let bytes = encoded_frame_bytes(&frames);
                        let sid = encoded_sid("vp9", &frames);
                        queue_encoded_frame(
                            frame_tx,
                            DecoderInput::Vp9 {
                                sid,
                                bytes,
                                queued_at: Instant::now(),
                                frames,
                            },
                            events,
                        );
                        Some(FrameSource::Video { bytes })
                    }
                    #[cfg(not(any(
                        feature = "live-vpx",
                        feature = "live-vpx-system",
                        all(feature = "live-vp9-mf", target_os = "windows")
                    )))]
                    {
                        let _ = (frame_tx, frames, events);
                        Some(FrameSource::SkippedVideo { codec: "VP9" }) // VP9 arrived but no decoder available
                    }
                }
                Some(video_frame::Union::H265s(frames)) => {
                    if crate::video::h265_available() {
                        if let Some((width, height)) =
                            decoder_dimensions(known_displays, current_display)
                        {
                            let bytes = encoded_frame_bytes(&frames);
                            let sid = encoded_sid("h265", &frames);
                            queue_encoded_frame(
                                frame_tx,
                                DecoderInput::H265 {
                                    sid,
                                    bytes,
                                    queued_at: Instant::now(),
                                    frames,
                                    width,
                                    height,
                                },
                                events,
                            );
                            Some(FrameSource::Video { bytes })
                        } else {
                            let _ = (frame_tx, frames);
                            let _ = events.send(SessionEvent::Info(
                                "Server sent H265 before display size was known; requesting fallback"
                                    .to_owned(),
                            ));
                            Some(FrameSource::SkippedVideo { codec: "H265" })
                        }
                    } else {
                        let _ = (frame_tx, frames);
                        let _ = events.send(SessionEvent::Info(
                            "Server sent H265, but hardware H265 decode is unavailable; requesting fallback"
                                .to_owned(),
                        ));
                        Some(FrameSource::SkippedVideo { codec: "H265" })
                    }
                }
                Some(video_frame::Union::Av1s(frames)) => {
                    if crate::video::av1_available() {
                        if let Some((width, height)) =
                            decoder_dimensions(known_displays, current_display)
                        {
                            let bytes = encoded_frame_bytes(&frames);
                            let sid = encoded_sid("av1", &frames);
                            queue_encoded_frame(
                                frame_tx,
                                DecoderInput::Av1 {
                                    sid,
                                    bytes,
                                    queued_at: Instant::now(),
                                    frames,
                                    width,
                                    height,
                                },
                                events,
                            );
                            Some(FrameSource::Video { bytes })
                        } else {
                            let _ = (frame_tx, frames);
                            let _ = events.send(SessionEvent::Info(
                                "Server sent AV1 before display size was known; requesting fallback"
                                    .to_owned(),
                            ));
                            Some(FrameSource::SkippedVideo { codec: "AV1" })
                        }
                    } else {
                        let _ = (frame_tx, frames);
                        let _ = events.send(SessionEvent::Info(
                            "Server sent AV1, but hardware AV1 decode is unavailable; requesting fallback"
                                .to_owned(),
                        ));
                        Some(FrameSource::SkippedVideo { codec: "AV1" })
                    }
                }
                _ => {
                    let _ = events.send(SessionEvent::Info(format!(
                        "Unsupported live video frame: {description}"
                    )));
                    None
                }
            }
        }
        Some(peer_message::Union::TestDelay(delay)) => {
            // last_delay is the RTT (ms) the server measured for the previous round-trip.
            let rtt = delay.last_delay;
            let _ = echo_test_delay(relay, delay);
            if rtt > 0 {
                let _ = events.send(SessionEvent::Latency(rtt));
            }
            None
        }
        Some(peer_message::Union::LoginResponse(response)) => {
            update_displays_from_login_response(&response, known_displays, events);
            let _ = send_selected_windows_session(relay, &response);
            let _ = send_video_start_messages(
                relay,
                current_display,
                false,
                target_video_fps,
                codec_preference,
            );
            if login_response_is_remote_accept_wait(&response) {
                let _ = events.send(SessionEvent::Info("Waiting for remote accept".to_owned()));
            }
            None
        }
        Some(peer_message::Union::PeerInfo(info)) => {
            update_displays_from_peer_info(&info, known_displays, events);
            let _ = send_selected_windows_session_from_peer_info(relay, &info);
            let _ = send_video_start_messages(
                relay,
                current_display,
                false,
                target_video_fps,
                codec_preference,
            );
            None
        }
        Some(peer_message::Union::CursorData(cd)) => {
            handle_cursor_data(cd, events);
            None
        }
        Some(peer_message::Union::CursorId(id)) => {
            let _ = events.send(SessionEvent::CursorId { id });
            None
        }
        Some(peer_message::Union::CursorPosition(cp)) => {
            let _ = events.send(SessionEvent::CursorPosition { x: cp.x, y: cp.y });
            None
        }
        Some(peer_message::Union::Misc(m)) => {
            // Log what the server tells us — most importantly the codec it selected.
            match &m.union {
                Some(misc::Union::Option(opt)) => {
                    if let Some(dec) = &opt.supported_decoding {
                        eprintln!(
                            "[session] Server codec reply: prefer={} h264={} h265={} av1={} vp9={} vp8={} fps={}",
                            dec.prefer,
                            dec.ability_h264,
                            dec.ability_h265,
                            dec.ability_av1,
                            dec.ability_vp9,
                            dec.ability_vp8,
                            opt.custom_fps
                        );
                    } else {
                        eprintln!("[session] Server Misc::Option (no decoding info)");
                    }
                }
                Some(misc::Union::SwitchDisplay(sd)) => {
                    eprintln!(
                        "[session] Server SwitchDisplay confirmed: display={} {}x{}",
                        sd.display, sd.width, sd.height
                    );
                    // No response needed — just log. Sending SwitchDisplay back
                    // would create an infinite SwitchDisplay ↔ SwitchDisplay loop.
                }
                Some(misc::Union::RefreshVideo(_)) => {
                    eprintln!("[session] Server requests RefreshVideo");
                }
                other => {
                    eprintln!(
                        "[session] Server Misc: {:?}",
                        other.as_ref().map(|_| "variant")
                    );
                }
            }
            None
        }
        Some(peer_message::Union::Shell(shell)) => {
            match ShellMessageKind::try_from(shell.kind).unwrap_or(ShellMessageKind::Output) {
                ShellMessageKind::Output => {
                    let _ = events.send(SessionEvent::ShellOutput(shell.data));
                }
                ShellMessageKind::Closed => {
                    let _ = events.send(SessionEvent::ShellClosed);
                }
                ShellMessageKind::Error => {
                    let _ = events.send(SessionEvent::ShellError(shell.data));
                }
                _ => {}
            }
            None
        }
        Some(peer_message::Union::Hash(_))
        | Some(peer_message::Union::SignedId(_))
        | Some(peer_message::Union::PublicKey(_))
        | Some(peer_message::Union::LoginRequest(_))
        | Some(peer_message::Union::MouseEvent(_))
        | Some(peer_message::Union::KeyEvent(_))
        | Some(peer_message::Union::ScreenshotRequest(_))
        | None => None,
    }
}

fn encoded_sid(prefix: &str, frames: &EncodedVideoFrames) -> String {
    frames
        .frames
        .last()
        .map(|frame| format!("{prefix}-{}", frame.pts))
        .unwrap_or_else(|| prefix.to_owned())
}

fn encoded_frame_bytes(frames: &EncodedVideoFrames) -> usize {
    frames.frames.iter().map(|frame| frame.data.len()).sum()
}

fn queue_encoded_frame(
    frame_tx: &Sender<DecoderInput>,
    input: DecoderInput,
    events: &Sender<SessionEvent>,
) {
    match frame_tx.send(input) {
        Ok(()) => {}
        Err(_) => {
            let _ = events.send(SessionEvent::Info(
                "Frame decoder stopped unexpectedly".to_owned(),
            ));
        }
    }
}

fn handle_cursor_data(cd: CursorData, events: &Sender<SessionEvent>) {
    if cd.width <= 0 || cd.height <= 0 {
        return;
    }
    let rgba = decompress_zstd(&cd.colors);
    let expected = (cd.width as usize) * (cd.height as usize) * 4;
    if rgba.len() == expected {
        let _ = events.send(SessionEvent::CursorData {
            id: cd.id,
            hotx: cd.hotx,
            hoty: cd.hoty,
            width: cd.width as u32,
            height: cd.height as u32,
            rgba,
        });
    } else if !rgba.is_empty() {
        eprintln!(
            "[cursor] size mismatch: got {} bytes, expected {} ({}x{}x4)",
            rgba.len(),
            expected,
            cd.width,
            cd.height
        );
    }
}

fn decompress_zstd(data: &[u8]) -> Vec<u8> {
    zstd::decode_all(data).unwrap_or_default()
}

fn request_screenshot_once(
    relay: &mut TcpStream,
    counter: &mut u64,
    display: i32,
    events: &Sender<SessionEvent>,
) {
    let display = display.max(0);
    match request_screenshot(relay, counter, display) {
        Ok(()) => {
            let _ = display;
        }
        Err(err) => {
            let _ = events.send(SessionEvent::Info(format!(
                "Screenshot request failed for display {display}: {err}"
            )));
        }
    }
}

fn update_displays_from_login_response(
    response: &crate::rustdesk_proto::LoginResponse,
    known_displays: &mut Vec<RemoteDisplay>,
    events: &Sender<SessionEvent>,
) {
    let displays = displays_from_login_response(response);
    if !displays.is_empty() {
        *known_displays = displays.clone();
        let _ = events.send(SessionEvent::Displays(displays));
    }
}

fn update_displays_from_peer_info(
    info: &crate::rustdesk_proto::PeerInfo,
    known_displays: &mut Vec<RemoteDisplay>,
    events: &Sender<SessionEvent>,
) {
    let displays = displays_from_peer_info(info);
    if !displays.is_empty() {
        *known_displays = displays.clone();
        let _ = events.send(SessionEvent::Displays(displays));
    }
}

fn displays_from_login_response(
    response: &crate::rustdesk_proto::LoginResponse,
) -> Vec<RemoteDisplay> {
    if let Some(crate::rustdesk_proto::login_response::Union::PeerInfo(info)) = &response.union {
        return displays_from_peer_info(info);
    }
    Vec::new()
}

fn displays_from_peer_info(info: &crate::rustdesk_proto::PeerInfo) -> Vec<RemoteDisplay> {
    let displays = info
        .displays
        .iter()
        .enumerate()
        .map(|(index, display)| RemoteDisplay {
            index: index as i32,
            name: if display.name.is_empty() {
                format!("Display {}", index + 1)
            } else {
                display.name.clone()
            },
            width: display.width,
            height: display.height,
            x: display.x,
            y: display.y,
            cursor_embedded: display.cursor_embedded,
        })
        .collect::<Vec<_>>();
    displays
}

fn decoder_dimensions(displays: &[RemoteDisplay], current_display: i32) -> Option<(u32, u32)> {
    displays
        .iter()
        .find(|display| display.index == current_display)
        .or_else(|| displays.first())
        .and_then(|display| {
            let width = u32::try_from(display.width).ok()?;
            let height = u32::try_from(display.height).ok()?;
            if width == 0 || height == 0 {
                None
            } else {
                Some((width, height))
            }
        })
}

fn request_screenshot(
    relay: &mut TcpStream,
    counter: &mut u64,
    display: i32,
) -> Result<(), String> {
    *counter += 1;
    let message = PeerMessage {
        union: Some(peer_message::Union::ScreenshotRequest(ScreenshotRequest {
            display,
            sid: format!("evertydesk-lite-d{}-{counter}", display.max(0)),
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn decode_frame_loop(
    frame_rx: Receiver<DecoderInput>,
    events: Sender<SessionEvent>,
    feedback: Sender<DecoderFeedback>,
) {
    #[cfg(feature = "live-h264")]
    let mut h264 = openh264::decoder::Decoder::new().ok();
    #[cfg(feature = "live-h264")]
    eprintln!(
        "[decoder] OpenH264 decoder {}",
        if h264.is_some() {
            "ready"
        } else {
            "unavailable"
        }
    );
    #[cfg(not(feature = "live-h264"))]
    let mut h264 = ();
    let mut h264_vt = if crate::videotoolbox::videotoolbox_h264_decoder_available() {
        eprintln!("[decoder] macOS VideoToolbox H264 decoder ready");
        Some(crate::videotoolbox::VideoToolboxH264Decoder::new())
    } else {
        None
    };

    #[cfg(feature = "live-vpx")]
    let mut vp8 = VpxDecoder::new(DecoderConfig::new(DecoderCodec::Vp8)).ok();
    #[cfg(not(feature = "live-vpx"))]
    let mut vp8 = ();
    #[cfg(feature = "live-vpx")]
    let mut vp9 = VpxDecoder::new(DecoderConfig::new(DecoderCodec::Vp9)).ok();
    #[cfg(feature = "live-vpx")]
    eprintln!(
        "[decoder] libvpx decoders: VP8={} VP9={}",
        if vp8.is_some() {
            "ready"
        } else {
            "unavailable"
        },
        if vp9.is_some() {
            "ready"
        } else {
            "unavailable"
        }
    );
    #[cfg(not(feature = "live-vpx"))]
    let mut vp9 = ();

    #[cfg(feature = "live-vpx-system")]
    let mut vp9_sys = crate::vpx_system::Vp9Decoder::new();
    #[cfg(feature = "live-vpx-system")]
    eprintln!(
        "[decoder] system libvpx VP9 decoder {}",
        if vp9_sys.is_some() {
            "ready"
        } else {
            "unavailable"
        }
    );
    #[cfg(not(feature = "live-vpx-system"))]
    let mut vp9_sys = ();

    // Windows Media Foundation VP9 decoder (no external libs, Win10 1803+).
    // Returns None on platforms/builds where MFT is unavailable.
    let mut vp9_mf = crate::vp9_mf::Vp9MfDecoder::new();
    if vp9_mf.is_some() {
        eprintln!("[decoder] Windows MF VP9 decoder ready");
    }

    let mut h265_mf: Option<crate::mf_video::MfVideoDecoder> = None;
    let mut av1_mf: Option<crate::mf_video::MfVideoDecoder> = None;
    let mf_status = crate::mf_video::mf_video_decode_status();
    if mf_status.h265 || mf_status.av1 {
        eprintln!("[decoder] {}", mf_status.label());
    }

    while let Ok(first) = frame_rx.recv() {
        let mut batch = vec![first];
        while let Ok(next) = frame_rx.try_recv() {
            batch.push(next);
        }
        let mut dropped_frames = 0_usize;
        let has_video = batch.iter().any(DecoderInput::is_video);
        if has_video {
            batch.retain(DecoderInput::is_video);
            let dropped = trim_video_backlog_to_keyframe(&mut batch);
            if dropped > 0 {
                let _ = feedback.send(DecoderFeedback::BacklogTrimmed { dropped });
                dropped_frames = dropped;
            }
        } else if batch.len() > 1 {
            let latest = batch.pop().expect("batch is not empty");
            dropped_frames = batch.len();
            batch.clear();
            batch.push(latest);
        }

        let mut latest_event = None;
        for frame in batch {
            let codec = frame.codec_name();
            let bytes = frame.byte_len();
            let queue_ms = frame
                .queued_at()
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            let started = Instant::now();
            match decode_one_frame(
                frame,
                &mut h264_vt,
                &mut h264,
                &mut vp8,
                &mut vp9,
                &mut vp9_sys,
                &mut vp9_mf,
                &mut h265_mf,
                &mut av1_mf,
            ) {
                Ok(Some(event)) => {
                    let decode_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                    let _ = events.send(SessionEvent::FrameMetrics {
                        bytes,
                        queue_ms,
                        decode_ms,
                        dropped: dropped_frames,
                    });
                    let _ = feedback.send(DecoderFeedback::FrameDecoded {
                        codec,
                        queue_ms,
                        decode_ms,
                    });
                    dropped_frames = 0;
                    latest_event = Some(event);
                }
                Ok(None) => {}
                Err(err) if decoder_needs_more_packets(&err) => {}
                Err(err) => {
                    let _ = feedback.send(DecoderFeedback::DecodeFailed { codec });
                    let _ = events.send(SessionEvent::Info(format!("Frame decode failed: {err}")));
                }
            }
        }
        if let Some(event) = latest_event {
            let _ = events.send(event);
        }
    }
}

impl DecoderInput {
    fn is_video(&self) -> bool {
        matches!(
            self,
            Self::H264 { .. }
                | Self::Vp8 { .. }
                | Self::Vp9 { .. }
                | Self::H265 { .. }
                | Self::Av1 { .. }
        )
    }

    fn has_keyframe(&self) -> bool {
        match self {
            Self::H264 { frames, .. }
            | Self::Vp8 { frames, .. }
            | Self::Vp9 { frames, .. }
            | Self::H265 { frames, .. }
            | Self::Av1 { frames, .. } => frames.frames.iter().any(|frame| frame.key),
            Self::Png { .. } => false,
        }
    }

    fn queued_at(&self) -> Instant {
        match self {
            Self::Png { queued_at, .. }
            | Self::H264 { queued_at, .. }
            | Self::Vp8 { queued_at, .. }
            | Self::Vp9 { queued_at, .. }
            | Self::H265 { queued_at, .. }
            | Self::Av1 { queued_at, .. } => *queued_at,
        }
    }

    fn byte_len(&self) -> usize {
        match self {
            Self::Png { bytes, .. }
            | Self::H264 { bytes, .. }
            | Self::Vp8 { bytes, .. }
            | Self::Vp9 { bytes, .. }
            | Self::H265 { bytes, .. }
            | Self::Av1 { bytes, .. } => *bytes,
        }
    }

    fn codec_name(&self) -> &'static str {
        match self {
            Self::Png { .. } => "PNG",
            Self::H264 { .. } => "H264",
            Self::Vp8 { .. } => "VP8",
            Self::Vp9 { .. } => "VP9",
            Self::H265 { .. } => "H265",
            Self::Av1 { .. } => "AV1",
        }
    }
}

fn trim_video_backlog_to_keyframe(batch: &mut Vec<DecoderInput>) -> usize {
    const MAX_VIDEO_BACKLOG: usize = 8;
    if batch.len() <= MAX_VIDEO_BACKLOG {
        return 0;
    }
    let before = batch.len();
    if let Some(index) = batch.iter().rposition(DecoderInput::has_keyframe) {
        if index > 0 {
            batch.drain(..index);
        }
    } else {
        let keep_from = batch.len().saturating_sub(MAX_VIDEO_BACKLOG);
        if keep_from > 0 {
            batch.drain(..keep_from);
        }
    }
    before.saturating_sub(batch.len())
}

fn decoder_needs_more_packets(err: &str) -> bool {
    err.contains("decoder needs more packets")
}

fn decode_one_frame(
    frame: DecoderInput,
    h264_vt: &mut Option<crate::videotoolbox::VideoToolboxH264Decoder>,
    #[cfg(feature = "live-h264")] h264: &mut Option<openh264::decoder::Decoder>,
    #[cfg(not(feature = "live-h264"))] _h264: &mut (),
    #[cfg(feature = "live-vpx")] vp8: &mut Option<VpxDecoder>,
    #[cfg(not(feature = "live-vpx"))] _vp8: &mut (),
    #[cfg(feature = "live-vpx")] vp9: &mut Option<VpxDecoder>,
    #[cfg(not(feature = "live-vpx"))] _vp9: &mut (),
    #[cfg(feature = "live-vpx-system")] vp9_sys: &mut Option<crate::vpx_system::Vp9Decoder>,
    #[cfg(not(feature = "live-vpx-system"))] _vp9_sys: &mut (),
    vp9_mf: &mut Option<crate::vp9_mf::Vp9MfDecoder>,
    h265_mf: &mut Option<crate::mf_video::MfVideoDecoder>,
    av1_mf: &mut Option<crate::mf_video::MfVideoDecoder>,
) -> Result<Option<SessionEvent>, String> {
    // Suppress unused-variable warning when live-vpx handles VP9 instead of MF.
    #[cfg(feature = "live-vpx")]
    let _ = &vp9_mf;
    #[cfg(all(feature = "live-vpx", feature = "live-vpx-system"))]
    let _ = &vp9_sys;

    match frame {
        DecoderInput::Png { sid, png, .. } => decode_png_rgba(&png).map(|(width, height, rgba)| {
            Some(SessionEvent::Frame {
                sid,
                codec: "PNG".to_owned(),
                width,
                height,
                rgba,
            })
        }),
        DecoderInput::H264 { sid, frames, .. } => {
            let vt_error = if let Some(decoder) = h264_vt.as_mut() {
                match decoder.decode_packets(frames.frames.iter().map(|frame| frame.data.clone())) {
                    Ok(Some((width, height, rgba))) => {
                        return Ok(Some(SessionEvent::Frame {
                            sid,
                            codec: "H264".to_owned(),
                            width,
                            height,
                            rgba,
                        }));
                    }
                    Ok(None) => None,
                    Err(err) if decoder_needs_more_packets(&err) => None,
                    Err(err) => Some(err),
                }
            } else {
                None
            };
            if let Some(err) = vt_error.as_ref() {
                eprintln!("[decoder] VideoToolbox H264 failed, falling back: {err}");
                *h264_vt = None;
            }

            #[cfg(feature = "live-h264")]
            {
                decode_h264_rgba(h264.as_mut(), frames).map(|(width, height, rgba)| {
                    Some(SessionEvent::Frame {
                        sid,
                        codec: "H264".to_owned(),
                        width,
                        height,
                        rgba,
                    })
                })
            }
            #[cfg(not(feature = "live-h264"))]
            {
                let _ = sid;
                let _ = frames;
                if let Some(err) = vt_error {
                    Err(format!(
                        "H264 frame received, but VideoToolbox failed and this build was compiled without live-h264: {err}"
                    ))
                } else {
                    Ok(None)
                }
            }
        }
        DecoderInput::Vp8 { sid, frames, .. } => {
            #[cfg(feature = "live-vpx")]
            {
                decode_vpx_rgba(vp8.as_mut(), frames).map(|(width, height, rgba)| {
                    Some(SessionEvent::Frame {
                        sid,
                        codec: "VP8".to_owned(),
                        width,
                        height,
                        rgba,
                    })
                })
            }
            #[cfg(not(feature = "live-vpx"))]
            {
                let _ = sid;
                let _ = frames;
                Err("VP8 frame received, but this build was compiled without live-vpx".to_owned())
            }
        }
        DecoderInput::Vp9 { sid, frames, .. } => {
            #[cfg(feature = "live-vpx")]
            {
                decode_vpx_rgba(vp9.as_mut(), frames).map(|(width, height, rgba)| {
                    Some(SessionEvent::Frame {
                        sid,
                        codec: "VP9".to_owned(),
                        width,
                        height,
                        rgba,
                    })
                })
            }
            // System libvpx (apt: libvpx-dev) — used on Astra etc. where the
            // from-source libvpx build fails.
            #[cfg(all(feature = "live-vpx-system", not(feature = "live-vpx")))]
            {
                let dec = vp9_sys
                    .as_mut()
                    .ok_or_else(|| "VP9: system libvpx decoder unavailable".to_owned())?;
                let mut last: Option<(usize, usize, Vec<u8>)> = None;
                for pkt in frames.frames {
                    if let Some(f) = dec.decode(&pkt.data) {
                        last = Some(f);
                    }
                }
                Ok(last.map(|(width, height, rgba)| SessionEvent::Frame {
                    sid,
                    codec: "VP9".to_owned(),
                    width,
                    height,
                    rgba,
                }))
            }
            #[cfg(not(any(feature = "live-vpx", feature = "live-vpx-system")))]
            {
                // Decode via Windows Media Foundation (live-vp9-mf feature, Win10 1803+).
                // We only queue VP9 when MF is available, so reaching here without an MF
                // decoder is an internal error (MFT not present on this specific Windows build).
                let mf = vp9_mf.as_mut().ok_or_else(|| {
                    "VP9: Windows Media Foundation decoder unavailable (Win10 1803+ required)"
                        .to_owned()
                })?;
                let mut last_frame: Option<(usize, usize, Vec<u8>)> = None;
                for pkt in frames.frames {
                    if pkt.data.is_empty() {
                        continue;
                    }
                    match mf.decode(&pkt.data) {
                        Ok(Some((w, h, rgba))) => last_frame = Some((w, h, rgba)),
                        Ok(None) => {} // decoder buffering — needs more input
                        Err(e) => return Err(e),
                    }
                }
                Ok(last_frame.map(|(width, height, rgba)| SessionEvent::Frame {
                    sid,
                    codec: "VP9".to_owned(),
                    width,
                    height,
                    rgba,
                }))
            }
        }
        DecoderInput::H265 {
            sid,
            frames,
            width,
            height,
            ..
        } => decode_mf_video_rgba(
            h265_mf,
            crate::mf_video::MfVideoCodec::H265,
            width,
            height,
            frames,
        )
        .map(|decoded| {
            decoded.map(|(width, height, rgba)| SessionEvent::Frame {
                sid,
                codec: "H265".to_owned(),
                width,
                height,
                rgba,
            })
        }),
        DecoderInput::Av1 {
            sid,
            frames,
            width,
            height,
            ..
        } => decode_mf_video_rgba(
            av1_mf,
            crate::mf_video::MfVideoCodec::Av1,
            width,
            height,
            frames,
        )
        .map(|decoded| {
            decoded.map(|(width, height, rgba)| SessionEvent::Frame {
                sid,
                codec: "AV1".to_owned(),
                width,
                height,
                rgba,
            })
        }),
    }
}

fn decode_mf_video_rgba(
    decoder: &mut Option<crate::mf_video::MfVideoDecoder>,
    codec: crate::mf_video::MfVideoCodec,
    width: u32,
    height: u32,
    frames: EncodedVideoFrames,
) -> Result<Option<(usize, usize, Vec<u8>)>, String> {
    let recreate = decoder
        .as_ref()
        .map(|decoder| !decoder.matches(codec, width, height))
        .unwrap_or(true);
    if recreate {
        *decoder = Some(crate::mf_video::MfVideoDecoder::new(codec, width, height)?);
        eprintln!(
            "[decoder] Media Foundation {} decoder started at {}x{}",
            codec.label(),
            width,
            height
        );
    }

    let decoder = decoder
        .as_mut()
        .ok_or_else(|| format!("{} decoder unavailable", codec.label()))?;
    decoder.decode_packets(frames.frames.into_iter().map(|frame| frame.data))
}

#[cfg(feature = "live-h264")]
fn decode_h264_rgba(
    decoder: Option<&mut openh264::decoder::Decoder>,
    frames: EncodedVideoFrames,
) -> Result<(usize, usize, Vec<u8>), String> {
    let decoder = decoder.ok_or_else(|| "OpenH264 decoder init failed".to_owned())?;
    let mut decoded = None;
    for frame in frames.frames {
        if frame.data.is_empty() {
            continue;
        }
        decoded = decoder
            .decode(&frame.data)
            .map_err(|err| err.to_string())?
            .map(|yuv| {
                let (width, height) = yuv.dimensions();
                let mut rgba = vec![0; width * height * 4];
                yuv.write_rgba8(&mut rgba);
                (width, height, rgba)
            })
            .or(decoded);
    }
    decoded.ok_or_else(|| "H264 decoder needs more packets".to_owned())
}

#[cfg(feature = "live-vpx")]
fn decode_vpx_rgba(
    decoder: Option<&mut VpxDecoder>,
    frames: EncodedVideoFrames,
) -> Result<(usize, usize, Vec<u8>), String> {
    let decoder = decoder.ok_or_else(|| "VPX decoder init failed".to_owned())?;
    let mut decoded = None;
    for frame in frames.frames {
        if frame.data.is_empty() {
            continue;
        }
        decoder.decode(&frame.data).map_err(|err| err.to_string())?;
        while let Some(vpx_frame) = decoder.next_frame().map_err(|err| err.to_string())? {
            decoded = Some(i420_to_rgba(
                vpx_frame.width(),
                vpx_frame.height(),
                vpx_frame.y_plane(),
                vpx_frame.u_plane(),
                vpx_frame.v_plane(),
                vpx_frame.y_stride(),
                vpx_frame.u_stride(),
                vpx_frame.v_stride(),
            ));
        }
    }
    decoded.ok_or_else(|| "VPX decoder needs more packets".to_owned())
}

#[cfg(feature = "live-vpx")]
struct YuvRgbTables {
    u_b: [i32; 256],
    u_g: [i32; 256],
    v_g: [i32; 256],
    v_r: [i32; 256],
}

#[cfg(feature = "live-vpx")]
fn yuv_rgb_tables() -> &'static YuvRgbTables {
    static TABLES: std::sync::OnceLock<YuvRgbTables> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| {
        let mut tables = YuvRgbTables {
            u_b: [0; 256],
            u_g: [0; 256],
            v_g: [0; 256],
            v_r: [0; 256],
        };
        for i in 0..256 {
            let c = i as i32 - 128;
            tables.u_b[i] = (1815 * c) >> 10;
            tables.u_g[i] = (352 * c) >> 10;
            tables.v_g[i] = (731 * c) >> 10;
            tables.v_r[i] = (1436 * c) >> 10;
        }
        tables
    })
}

#[cfg(feature = "live-vpx")]
fn i420_to_rgba(
    width: usize,
    height: usize,
    y_plane: &[u8],
    u_plane: &[u8],
    v_plane: &[u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
) -> (usize, usize, Vec<u8>) {
    let tables = yuv_rgb_tables();

    let mut rgba = vec![0_u8; width * height * 4];
    let even_width = width & !1;
    let even_height = height & !1;
    for y in (0..even_height).step_by(2) {
        let y0 = y * y_stride;
        let y1 = (y + 1) * y_stride;
        let rgba0 = y * width * 4;
        let rgba1 = (y + 1) * width * 4;
        let chroma_y = (y / 2) * u_stride;
        let chroma_v_y = (y / 2) * v_stride;
        for x in (0..even_width).step_by(2) {
            let ui = u_plane[chroma_y + x / 2] as usize;
            let vi = v_plane[chroma_v_y + x / 2] as usize;
            let add_b = tables.u_b[ui];
            let sub_g = tables.u_g[ui] + tables.v_g[vi];
            let add_r = tables.v_r[vi];

            write_yuv_pixel(
                &mut rgba,
                rgba0 + x * 4,
                y_plane[y0 + x],
                add_r,
                sub_g,
                add_b,
            );
            write_yuv_pixel(
                &mut rgba,
                rgba0 + (x + 1) * 4,
                y_plane[y0 + x + 1],
                add_r,
                sub_g,
                add_b,
            );
            write_yuv_pixel(
                &mut rgba,
                rgba1 + x * 4,
                y_plane[y1 + x],
                add_r,
                sub_g,
                add_b,
            );
            write_yuv_pixel(
                &mut rgba,
                rgba1 + (x + 1) * 4,
                y_plane[y1 + x + 1],
                add_r,
                sub_g,
                add_b,
            );
        }
    }

    if even_width != width || even_height != height {
        for y in 0..height {
            for x in 0..width {
                if x < even_width && y < even_height {
                    continue;
                }
                let ui = u_plane[(y / 2) * u_stride + (x / 2)] as usize;
                let vi = v_plane[(y / 2) * v_stride + (x / 2)] as usize;
                let offset = (y * width + x) * 4;
                write_yuv_pixel(
                    &mut rgba,
                    offset,
                    y_plane[y * y_stride + x],
                    tables.v_r[vi],
                    tables.u_g[ui] + tables.v_g[vi],
                    tables.u_b[ui],
                );
            }
        }
    }
    (width, height, rgba)
}

#[cfg(feature = "live-vpx")]
fn write_yuv_pixel(rgba: &mut [u8], offset: usize, y: u8, add_r: i32, sub_g: i32, add_b: i32) {
    let yy = y as i32;
    rgba[offset] = (yy + add_r).clamp(0, 255) as u8;
    rgba[offset + 1] = (yy - sub_g).clamp(0, 255) as u8;
    rgba[offset + 2] = (yy + add_b).clamp(0, 255) as u8;
    rgba[offset + 3] = 255;
}

fn decode_png_rgba(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    let image = image::load_from_memory(bytes)
        .map_err(|err| err.to_string())?
        .to_rgba8();
    Ok((
        image.width() as usize,
        image.height() as usize,
        image.into_raw(),
    ))
}

fn send_switch_display(
    relay: &mut TcpStream,
    display: i32,
    info: Option<&RemoteDisplay>,
) -> Result<(), String> {
    let switch_display = SwitchDisplay {
        display,
        x: info.map(|d| d.x).unwrap_or_default(),
        y: info.map(|d| d.y).unwrap_or_default(),
        width: info.map(|d| d.width).unwrap_or_default(),
        height: info.map(|d| d.height).unwrap_or_default(),
        cursor_embedded: info.map(|d| d.cursor_embedded).unwrap_or_default(),
    };
    let message = PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::SwitchDisplay(switch_display)),
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn send_mouse(relay: &mut TcpStream, mask: i32, x: i32, y: i32) -> Result<(), String> {
    let message = PeerMessage {
        union: Some(peer_message::Union::MouseEvent(MouseEvent {
            mask,
            x,
            y,
            modifiers: Vec::new(),
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn send_text(relay: &mut TcpStream, text: &str) -> Result<(), String> {
    send_text_with_modifiers(relay, text, &[])
}

fn send_text_with_modifiers(
    relay: &mut TcpStream,
    text: &str,
    modifiers: &[ControlKey],
) -> Result<(), String> {
    for ch in text.chars() {
        send_key(
            relay,
            crate::rustdesk_proto::key_event::Union::Unicode(ch as u32),
            modifiers,
        )?;
    }
    Ok(())
}

fn send_control_key(relay: &mut TcpStream, key: ControlKey) -> Result<(), String> {
    send_control_key_with_modifiers(relay, key, &[])
}

fn send_control_key_state(
    relay: &mut TcpStream,
    key: ControlKey,
    down: bool,
) -> Result<(), String> {
    let message = PeerMessage {
        union: Some(peer_message::Union::KeyEvent(KeyEvent {
            down,
            press: false,
            union: Some(crate::rustdesk_proto::key_event::Union::ControlKey(
                key as i32,
            )),
            modifiers: Vec::new(),
            mode: KeyboardMode::Legacy as i32,
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn send_control_key_with_modifiers(
    relay: &mut TcpStream,
    key: ControlKey,
    modifiers: &[ControlKey],
) -> Result<(), String> {
    send_key(
        relay,
        crate::rustdesk_proto::key_event::Union::ControlKey(key as i32),
        modifiers,
    )
}

fn send_key(
    relay: &mut TcpStream,
    union: crate::rustdesk_proto::key_event::Union,
    modifiers: &[ControlKey],
) -> Result<(), String> {
    let message = PeerMessage {
        union: Some(peer_message::Union::KeyEvent(KeyEvent {
            down: false,
            press: true,
            union: Some(union),
            modifiers: modifiers.iter().map(|key| *key as i32).collect(),
            mode: KeyboardMode::Legacy as i32,
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn is_timeout_error(err: &str) -> bool {
    err.contains("timed out")
        || err.contains("would block")
        || err.contains("WouldBlock")
        || err.contains("Resource temporarily unavailable")
        || err.contains("os error 11")
        || err.contains("os error 35")
        || err.contains("10060")
        || err.contains("Попытка установить соединение")
}

fn describe_peer_message(message: &PeerMessage) -> String {
    match &message.union {
        Some(peer_message::Union::VideoFrame(frame)) => {
            format!("VideoFrame {}", describe_video_frame(frame))
        }
        Some(peer_message::Union::ScreenshotResponse(response)) => format!(
            "ScreenshotResponse sid={} bytes={} msg={}",
            response.sid,
            response.data.len(),
            response.msg
        ),
        Some(peer_message::Union::LoginResponse(response)) => match &response.union {
            Some(crate::rustdesk_proto::login_response::Union::PeerInfo(info)) => {
                format!("LoginResponse PeerInfo {} {}", info.hostname, info.version)
            }
            Some(crate::rustdesk_proto::login_response::Union::Error(err)) => {
                format!("LoginResponse Error {err}")
            }
            None => "LoginResponse empty".to_owned(),
        },
        Some(peer_message::Union::PeerInfo(info)) => {
            format!("PeerInfo {} {}", info.hostname, info.version)
        }
        Some(peer_message::Union::Hash(_)) => "Hash".to_owned(),
        Some(peer_message::Union::SignedId(_)) => "SignedId".to_owned(),
        Some(peer_message::Union::PublicKey(_)) => "PublicKey".to_owned(),
        Some(peer_message::Union::TestDelay(_)) => "TestDelay".to_owned(),
        Some(peer_message::Union::Misc(_)) => "Misc".to_owned(),
        Some(peer_message::Union::Shell(shell)) => {
            format!("Shell kind={} bytes={}", shell.kind, shell.data.len())
        }
        Some(peer_message::Union::MouseEvent(_)) => "MouseEvent".to_owned(),
        Some(peer_message::Union::KeyEvent(_)) => "KeyEvent".to_owned(),
        Some(peer_message::Union::LoginRequest(_)) => "LoginRequest".to_owned(),
        Some(peer_message::Union::ScreenshotRequest(_)) => "ScreenshotRequest".to_owned(),
        Some(peer_message::Union::CursorData(cd)) => {
            format!(
                "CursorData id={} {}x{} hotspot=({},{})",
                cd.id, cd.width, cd.height, cd.hotx, cd.hoty
            )
        }
        Some(peer_message::Union::CursorId(id)) => format!("CursorId {id}"),
        Some(peer_message::Union::CursorPosition(cp)) => {
            format!("CursorPosition ({},{})", cp.x, cp.y)
        }
        None => "Empty".to_owned(),
    }
}

fn describe_video_frame(frame: &crate::rustdesk_proto::VideoFrame) -> String {
    match &frame.union {
        Some(video_frame::Union::Rgb(rgb)) => {
            format!("display {} RGB compress={}", frame.display, rgb.compress)
        }
        Some(video_frame::Union::Yuv(yuv)) => format!(
            "display {} YUV compress={} stride={}",
            frame.display, yuv.compress, yuv.stride
        ),
        Some(video_frame::Union::Vp8s(frames)) => {
            format!(
                "display {} VP8 frames={}",
                frame.display,
                frames.frames.len()
            )
        }
        Some(video_frame::Union::Vp9s(frames)) => {
            format!(
                "display {} VP9 frames={}",
                frame.display,
                frames.frames.len()
            )
        }
        Some(video_frame::Union::H264s(frames)) => {
            format!(
                "display {} H264 frames={}",
                frame.display,
                frames.frames.len()
            )
        }
        Some(video_frame::Union::H265s(frames)) => {
            format!(
                "display {} H265 frames={}",
                frame.display,
                frames.frames.len()
            )
        }
        Some(video_frame::Union::Av1s(frames)) => {
            format!(
                "display {} AV1 frames={}",
                frame.display,
                frames.frames.len()
            )
        }
        None => format!("display {} empty video frame", frame.display),
    }
}

fn describe_login_response(
    response: crate::rustdesk_proto::LoginResponse,
    sent_login: bool,
) -> Result<String, String> {
    match response.union {
        Some(crate::rustdesk_proto::login_response::Union::Error(err)) => {
            Err(format!("Login refused: {}", describe_login_error(&err)))
        }
        Some(crate::rustdesk_proto::login_response::Union::PeerInfo(info)) => {
            let prefix = if sent_login {
                "authorized"
            } else {
                "peer accepted without password hash"
            };
            Ok(format!(
                "{prefix}; peer info: hostname={}, platform={}, version={}",
                info.hostname, info.platform, info.version
            ))
        }
        None => Ok("empty login response".to_owned()),
    }
}

fn login_response_is_remote_accept_wait(response: &crate::rustdesk_proto::LoginResponse) -> bool {
    matches!(
        &response.union,
        Some(crate::rustdesk_proto::login_response::Union::Error(err))
            if err == "No Password Access"
    )
}

fn describe_login_error(error: &str) -> String {
    if error.to_ascii_lowercase().contains("wrong password") {
        "Wrong Password".to_owned()
    } else {
        error.to_owned()
    }
}

fn build_login_request(
    password: &str,
    salt: &str,
    challenge: &str,
    remote_id: &str,
    fps: i32,
    codec_preference: CodecPreference,
) -> PeerMessage {
    let password_hash = if password.is_empty() {
        Vec::new()
    } else {
        let mut h1 = Sha256::new();
        h1.update(password.as_bytes());
        h1.update(salt.as_bytes());
        let h1 = h1.finalize();

        let mut h2 = Sha256::new();
        h2.update(h1);
        h2.update(challenge.as_bytes());
        h2.finalize().to_vec()
    };

    PeerMessage {
        union: Some(peer_message::Union::LoginRequest(LoginRequest {
            username: remote_id.to_owned(),
            password: password_hash,
            my_id: "evertydesk-lite".to_owned(),
            my_name: "EvertyDesk Lite".to_owned(),
            option: Some(OptionMessage {
                supported_decoding: Some(supported_decoding(codec_preference)),
                custom_fps: fps.clamp(5, 60),
            }),
            video_ack_required: false,
            version: "1.4.6".to_owned(),
            my_platform: std::env::consts::OS.to_owned(),
        })),
    }
}

struct RendezvousInfo {
    relay_server: Option<String>,
    relay_uuid: Option<String>,
    has_signed_pk: bool,
}

fn describe_rendezvous_response(message: &RendezvousMessage) -> Result<RendezvousInfo, String> {
    match &message.union {
        Some(rendezvous_message::Union::PunchHoleResponse(response)) => {
            if !response.other_failure.is_empty() {
                return Err(response.other_failure.clone());
            }
            if response.socket_addr.is_empty() && response.relay_server.is_empty() {
                let failure = PunchHoleFailure::try_from(response.failure)
                    .map(describe_punch_hole_failure)
                    .unwrap_or_else(|_| format!("unknown failure {}", response.failure));
                return Err(format!("Rendezvous refused: {failure}"));
            }
            Ok(RendezvousInfo {
                relay_server: (!response.relay_server.is_empty())
                    .then(|| response.relay_server.clone()),
                relay_uuid: None,
                has_signed_pk: !response.pk.is_empty(),
            })
        }
        Some(rendezvous_message::Union::RelayResponse(response)) => {
            if !response.refuse_reason.is_empty() {
                Err(response.refuse_reason.clone())
            } else {
                Ok(RendezvousInfo {
                    relay_server: (!response.relay_server.is_empty())
                        .then(|| response.relay_server.clone()),
                    relay_uuid: (!response.uuid.is_empty()).then(|| response.uuid.clone()),
                    has_signed_pk: false,
                })
            }
        }
        Some(rendezvous_message::Union::PunchHoleRequest(_)) => {
            Err("Unexpected PunchHoleRequest response".to_owned())
        }
        Some(rendezvous_message::Union::RequestRelay(_)) => {
            Err("Unexpected RequestRelay response".to_owned())
        }
        Some(rendezvous_message::Union::OnlineRequest(_)) => {
            Err("Unexpected OnlineRequest response".to_owned())
        }
        Some(rendezvous_message::Union::OnlineResponse(_)) => {
            Err("Unexpected OnlineResponse response".to_owned())
        }
        Some(rendezvous_message::Union::RegisterPeer(_)) => {
            Err("Unexpected RegisterPeer response".to_owned())
        }
        Some(rendezvous_message::Union::RegisterPeerResponse(_)) => {
            Err("Unexpected RegisterPeerResponse response".to_owned())
        }
        Some(rendezvous_message::Union::RegisterPk(_)) => {
            Err("Unexpected RegisterPk response".to_owned())
        }
        Some(rendezvous_message::Union::RegisterPkResponse(_)) => {
            Err("Unexpected RegisterPkResponse response".to_owned())
        }
        Some(rendezvous_message::Union::KeyExchange(_)) => {
            Err("Unexpected KeyExchange response".to_owned())
        }
        Some(rendezvous_message::Union::PunchHole(_)) => {
            Err("Unexpected PunchHole response".to_owned())
        }
        Some(rendezvous_message::Union::FetchLocalAddr(_)) => {
            Err("Unexpected FetchLocalAddr response".to_owned())
        }
        None => Err("Empty rendezvous response".to_owned()),
    }
}

fn describe_punch_hole_failure(failure: PunchHoleFailure) -> String {
    match failure {
        PunchHoleFailure::IdNotExist => "ID does not exist on ID server".to_owned(),
        PunchHoleFailure::Offline => {
            "Offline: remote ID is not connected to this ID server now".to_owned()
        }
        PunchHoleFailure::LicenseMismatch => "License/public key mismatch".to_owned(),
        PunchHoleFailure::LicenseOveruse => "License/session limit exceeded".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_len_matches_rustdesk_codec_short_packet() {
        assert_eq!(encode_frame_len(0).unwrap(), vec![0]);
        assert_eq!(encode_frame_len(1).unwrap(), vec![4]);
        assert_eq!(encode_frame_len(0x3f).unwrap(), vec![0xfc]);
    }

    #[test]
    fn frame_len_matches_rustdesk_codec_medium_packet() {
        assert_eq!(encode_frame_len(0x40).unwrap(), vec![0x01, 0x01]);
        assert_eq!(encode_frame_len(0x3fff).unwrap(), vec![0xfd, 0xff]);
    }

    #[test]
    fn everty_public_key_is_valid_ed25519_size() {
        validate_public_key("MrGdbay3g8Qr84YYnxr4qLjw5zLWM1oAOdfehbBnlRs=").unwrap();
    }

    #[test]
    fn split_host_port_accepts_explicit_port() {
        assert_eq!(
            split_host_port("edesk.server1.everty.ru:21117", 21117),
            ("edesk.server1.everty.ru".to_owned(), 21117)
        );
    }

    #[test]
    fn split_host_port_uses_default_when_missing() {
        assert_eq!(
            split_host_port("edesk.server1.everty.ru", 21117),
            ("edesk.server1.everty.ru".to_owned(), 21117)
        );
    }

    #[test]
    fn login_request_uses_32_byte_password_hash() {
        let message = build_login_request(
            "secret",
            "salt",
            "challenge",
            "123",
            60,
            CodecPreference::Auto,
        );
        let Some(peer_message::Union::LoginRequest(login)) = message.union else {
            panic!("expected login request");
        };
        assert_eq!(login.password.len(), 32);
        assert_eq!(login.username, "123");
        assert_eq!(login.option.unwrap().custom_fps, 60);
    }

    #[test]
    fn login_request_uses_empty_password_for_remote_approval() {
        let message =
            build_login_request("", "salt", "challenge", "123", 30, CodecPreference::Auto);
        let Some(peer_message::Union::LoginRequest(login)) = message.union else {
            panic!("expected login request");
        };
        assert!(login.password.is_empty());
        assert_eq!(login.username, "123");
    }

    #[test]
    fn auto_codec_prefers_low_latency_h264_when_available() {
        assert_eq!(
            preferred_codec(CodecPreference::Auto, true, false, false, true) as i32,
            PreferCodec::H264 as i32
        );
        assert_eq!(
            preferred_codec(CodecPreference::Auto, false, false, false, true) as i32,
            PreferCodec::Vp9 as i32
        );
    }

    #[test]
    fn auto_codec_prefers_modern_codecs_when_available() {
        assert_eq!(
            preferred_codec(CodecPreference::Auto, true, true, false, true) as i32,
            PreferCodec::H265 as i32
        );
        assert_eq!(
            preferred_codec(CodecPreference::Auto, true, true, true, true) as i32,
            PreferCodec::Av1 as i32
        );
    }

    #[test]
    fn explicit_codec_preference_falls_back_to_supported_decoder() {
        assert_eq!(
            preferred_codec(CodecPreference::H265, true, false, false, true) as i32,
            PreferCodec::H264 as i32
        );
        assert_eq!(
            preferred_codec(CodecPreference::Av1, true, true, false, true) as i32,
            PreferCodec::H265 as i32
        );
        assert_eq!(
            preferred_codec(CodecPreference::Vp9, true, false, false, false) as i32,
            PreferCodec::H264 as i32
        );
    }

    #[test]
    fn decoder_dimensions_use_active_display_with_first_display_fallback() {
        let displays = vec![
            RemoteDisplay {
                index: 0,
                name: "Built-in".to_owned(),
                width: 1366,
                height: 768,
                x: 0,
                y: 0,
                cursor_embedded: false,
            },
            RemoteDisplay {
                index: 2,
                name: "External".to_owned(),
                width: 1920,
                height: 1080,
                x: 1366,
                y: 0,
                cursor_embedded: false,
            },
        ];

        assert_eq!(decoder_dimensions(&displays, 2), Some((1920, 1080)));
        assert_eq!(decoder_dimensions(&displays, 9), Some((1366, 768)));
        assert_eq!(decoder_dimensions(&[], 0), None);
    }

    #[test]
    fn login_response_peer_info_is_success() {
        let response = crate::rustdesk_proto::LoginResponse {
            union: Some(crate::rustdesk_proto::login_response::Union::PeerInfo(
                crate::rustdesk_proto::PeerInfo {
                    username: "user".to_owned(),
                    hostname: "host".to_owned(),
                    platform: "windows".to_owned(),
                    displays: Vec::new(),
                    current_display: 0,
                    version: "1.4.6".to_owned(),
                    windows_sessions: None,
                },
            )),
        };
        let text = describe_login_response(response, true).unwrap();
        assert!(text.contains("authorized"));
        assert!(text.contains("host"));
    }

    #[test]
    fn login_response_error_is_failure() {
        let response = crate::rustdesk_proto::LoginResponse {
            union: Some(crate::rustdesk_proto::login_response::Union::Error(
                "Wrong Password".to_owned(),
            )),
        };
        assert!(describe_login_response(response, true)
            .unwrap_err()
            .contains("Wrong Password"));
    }

    #[test]
    fn video_frame_description_reports_codec() {
        let frame = crate::rustdesk_proto::VideoFrame {
            display: 0,
            union: Some(crate::rustdesk_proto::video_frame::Union::H264s(
                crate::rustdesk_proto::EncodedVideoFrames {
                    frames: vec![crate::rustdesk_proto::EncodedVideoFrame {
                        data: vec![1, 2, 3],
                        key: true,
                        pts: 42,
                    }],
                },
            )),
        };
        assert!(describe_video_frame(&frame).contains("H264"));
    }

    #[test]
    fn timeout_detection_handles_macos_would_block() {
        assert!(is_timeout_error(
            "TCP read header failed: Resource temporarily unavailable (os error 35)"
        ));
    }

    #[test]
    fn relay_response_keeps_server_uuid() {
        let message = RendezvousMessage {
            union: Some(rendezvous_message::Union::RelayResponse(
                crate::rustdesk_proto::RelayResponse {
                    uuid: "relay-uuid".to_owned(),
                    relay_server: "relay.example.test".to_owned(),
                    ..Default::default()
                },
            )),
        };
        let info = describe_rendezvous_response(&message).unwrap();
        assert_eq!(info.relay_uuid.as_deref(), Some("relay-uuid"));
        assert_eq!(info.relay_server.as_deref(), Some("relay.example.test"));
    }

    #[test]
    fn decoder_buffering_is_not_decode_failure() {
        assert!(decoder_needs_more_packets(
            "H264 decoder needs more packets"
        ));
        assert!(decoder_needs_more_packets("VPX decoder needs more packets"));
    }
}
