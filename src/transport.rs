use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
#[cfg(feature = "live-h264")]
use openh264::formats::YUVSource;
use sha2::{Digest, Sha256};
#[cfg(feature = "live-vpx")]
use shiguredo_libvpx::{Decoder as VpxDecoder, DecoderCodec, DecoderConfig};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};

use crate::{
    rustdesk_proto::{
        decode_message, decode_peer_message, encode_message, encode_peer_message, misc,
        peer_message, rendezvous_message, video_frame, CaptureDisplays, Chroma, Clipboard,
        ClipboardFormat, CodecAbility, ConnType, ControlKey, CursorData, EncodedVideoFrames,
        ImageQuality, KeyEvent, KeyboardMode, LoginRequest, MessageQuery, Misc, MouseEvent,
        NatType, OnlineRequest, OptionMessage, PeerMessage, PreferCodec, PublicKey,
        PunchHoleFailure, PunchHoleRequest, RendezvousMessage, RequestRelay, ScreenshotRequest,
        ShellMessage, ShellMessageKind, SupportedDecoding, SwitchDisplay, TestDelay,
        TestNatRequest,
    },
    settings::{CodecPreference, DisplayConfig, ServerConfig},
};

const RENDEZVOUS_PORT: u16 = 21116;
const ONLINE_PORT: u16 = RENDEZVOUS_PORT - 1;
const RELAY_PORT: u16 = 21117;
const SESSION_TICK_MS: u64 = 16; // ~60 fps poll; keeps command latency ≤16 ms
const RELAY_STREAM_ATTEMPTS: u8 = 3;
const RELAY_BOOTSTRAP_WAIT_SECS: u64 = 12;
const RELAY_RESPONSE_BOOTSTRAP_WAIT_SECS: u64 = 12;
const RELAY_AUTH_WAIT_SECS: u64 = 120;
const RELAY_HANDSHAKE_POLL_MS: u64 = 150;
const DIRECT_TCP_CONNECT_TIMEOUT_SECS: u64 = 2;
const DIRECT_TCP_BOOTSTRAP_WAIT_SECS: u64 = 10;

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}

fn relay_first_fast_path() -> bool {
    env_flag("EVERTYDESK_CONNECT_RELAY_FIRST", true)
}

fn blocking_udp_nat_probe_enabled() -> bool {
    env_flag("EVERTYDESK_CONNECT_UDP_PROBE", false)
}

fn direct_tcp_probe_enabled() -> bool {
    env_flag("EVERTYDESK_TRY_DIRECT_TCP", false)
}

fn elapsed_ms(started: &Instant) -> u128 {
    started.elapsed().as_millis()
}

#[derive(Clone, Debug)]
pub struct ConnectionRequest {
    pub remote_id: String,
    pub password: String,
    pub server: ServerConfig,
    pub display: DisplayConfig,
    pub control_only: bool,
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
    ClipboardText(String),
    ShellOutput(String),
    ShellClosed,
    ShellError(String),
    /// EVRT прямое UDP соединение установлено / разорвано.
    EvrtStatus {
        active: bool,
        host_addr: String,
        port: u16,
    },
    /// EVRT метрики в реальном времени.
    EvrtMetrics {
        pressure: String, // "normal" / "high" / "critical"
        arrival_delta_ms: i32,
        assembly_delay_ms: i32,
        decode_delta_ms: i32,
        jitter_ms: u32,
        bitrate_mbps: f32,
        fps: u32,
        packets_received: u64,
        frames_assembled: u64,
        reassembly_drops: u64,
        queue_drops: u64,
    },
    /// Agentless VM: список VM от хоста (JSON `[{"id","name","state","connectable"}]`).
    VmList(String),
    /// Agentless VM: статус VM-сессии от хоста.
    VmStatus(String),
    /// Agentless VM: результат power operation.
    VmPowerResult(String),
    /// Agentless VM: capability graph JSON.
    VmCapabilities(String),
    /// Agentless VM: список чекпоинтов (JSON).
    VmCheckpoints(String),
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
    SetClipboardText(String),
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
    /// Agentless VM: запросить у хоста список VM на гипервизоре.
    ListVms,
    /// Agentless VM: прикрепиться к VM по id (пустая строка = отсоединиться).
    AttachVm(String),
    /// Agentless VM: power action. JSON: {"vm_id","vm_path","action"}
    VmPowerOp(String),
    /// Agentless VM: запросить capability graph для vm_id.
    VmCapabilityRequest(String),
    /// Agentless VM: операция с чекпоинтом. JSON: {"vm_id","op","path"}
    VmCheckpointOp(String),
    /// Agentless VM: rescue input. JSON: {"vm_id","input_type","text"}
    VmRescueInput(String),
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
        let (
            _relay_stream,
            peer_stage,
            _displays,
            _evrt_addr,
            _evrt_base,
            _early_evrt_candidates,
            _evrt_token,
        ) = establish_session(request.clone(), &mut progress)?;

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
        let control_only = request.control_only;
        let mut codec_preference = display_config.codec;
        let initial_video_fps = display_config.target_fps.clamp(5, 60) as i32;
        let adaptive_quality = display_config.adaptive_quality;
        let min_video_fps = display_config
            .min_fps
            .clamp(5, display_config.target_fps.clamp(5, 60)) as i32;
        let backlog_min_video_fps = backlog_recovery_min_fps(initial_video_fps, min_video_fps);
        let mut emit_progress = |pct, message: String| {
            let _ = events.send(SessionEvent::Progress(pct, message));
        };

        let (
            mut relay,
            peer_stage,
            displays,
            evrt_host_addr,
            evrt_host_base,
            early_evrt_candidates,
            mut evrt_token,
        ) = match establish_session(request.clone(), &mut emit_progress) {
            Ok(session) => session,
            Err(err) => {
                let _ = events.send(SessionEvent::Failed(err));
                return;
            }
        };

        eprintln!("[session] Connected: {peer_stage}");
        eprintln!("[session] Displays: {}", displays.len());
        if let Some(ref addr) = evrt_host_addr {
            eprintln!("[session] EVRT host addr (один запрос hbbs): {addr}");
        }
        let _ = events.send(SessionEvent::Connected(peer_stage));
        let mut known_displays = displays;
        if !known_displays.is_empty() {
            let _ = events.send(SessionEvent::Displays(known_displays.clone()));
        } else {
            let _ = events.send(SessionEvent::Info(
                "PeerInfo displays empty; manual monitor selector is enabled".to_owned(),
            ));
        }
        if control_only {
            let _ = events.send(SessionEvent::Info(
                "Control-only session: video and screenshots disabled".to_owned(),
            ));
        }

        // ★ EVRT: адрес хоста получен за ОДИН запрос к hbbs.
        // IP из PunchHoleResponse.socket_addr, порт из Misc{EvrtUdpPort}.
        // Порт может прийти в handshake (evrt_host_addr) ИЛИ позже в session loop
        // (evrt_host_base + late Misc). Флаг не даёт запустить дважды.
        // Единый stop-сигнал для всех EVRT-потоков этой сессии.
        // nativeStop / TCP-ошибка устанавливают его в true, чтобы потоки не
        // продолжали слать UDP хосту и не мешали новому подключению.
        let evrt_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        let mut evrt_started = false;
        if !control_only {
            if let Some(host_addr) = evrt_host_addr {
            evrt_started = true;
            let evrt_events = events.clone();
            let evrt_stop_clone = evrt_stop.clone();
            let evrt_ull = display_config.target_fps >= 60;
            let evrt_token_for_thread = evrt_token.clone();

            eprintln!("[evrt-client] прямой UDP → {host_addr}");

            thread::spawn(move || {
                if let Ok(udp) = std::net::UdpSocket::bind("0.0.0.0:0") {
                    let udp = std::sync::Arc::new(udp);
                    crate::evrt_client::try_evrt_before_relay(
                        &udp,
                        host_addr,
                        evrt_token_for_thread.clone(),
                        &evrt_events,
                        evrt_stop_clone,
                        evrt_ull,
                    );
                }
            });
            }
        }
        if !control_only && !evrt_started && !early_evrt_candidates.is_empty() {
            evrt_started = true;
            let evrt_events = events.clone();
            let evrt_ull = display_config.target_fps >= 60;
            let evrt_token_for_thread = evrt_token.clone();
            let evrt_stop_clone = evrt_stop.clone();
            eprintln!(
                "[evrt-client] ранние EVRT кандидаты: {:?}",
                early_evrt_candidates
            );
            thread::spawn(move || {
                crate::evrt_client::try_evrt_candidates(
                    early_evrt_candidates,
                    evrt_token_for_thread.clone(),
                    &evrt_events,
                    evrt_ull,
                    evrt_stop_clone,
                );
            });
        }
        let (frame_tx, frame_rx) = mpsc::channel::<DecoderInput>();
        let (decoder_feedback_tx, decoder_feedback_rx) = mpsc::channel::<DecoderFeedback>();
        if control_only {
            drop(frame_rx);
            drop(decoder_feedback_tx);
        } else {
            let frame_events = events.clone();
            thread::spawn(move || decode_frame_loop(frame_rx, frame_events, decoder_feedback_tx));
        }

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
        let mut latest_input_fps = 0.0_f32;
        let mut target_video_fps = initial_video_fps;
        let mut last_decoder_recovery: Option<Instant> = None;
        let mut last_adaptive_raise = Instant::now();
        let mut stable_decoded_frames = 0_u32;
        // 60 fps sessions are interactive first: start with RustDesk's speed
        // quality, then raise only after the incoming stream proves it is fast.
        let mut current_quality = initial_stream_quality(initial_video_fps);
        let mut last_quality_change = Instant::now();
        let mut low_input_quality_windows = 0_u32;
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
        let mut last_qos_feedback_sent: Option<Instant> = None;
        let mut qos_feedback_sent = 0_u64;
        let mut qos_feedback_failures = 0_u64;
        // Subscribe to display 0 (SwitchDisplay) then trigger video start.
        // SwitchDisplay must come first — it's the one-time subscription trigger.
        if control_only {
            screenshot_pending = false;
        } else {
            let _ = send_switch_display_subscribe(&mut relay, current_display);
            let _ = send_video_start_messages_with_quality(
                &mut relay,
                current_display,
                true,
                target_video_fps,
                codec_preference,
                current_quality,
            );
            let _ = send_video_received(&mut relay);
            request_screenshot_once(&mut relay, &mut screenshot_id, current_display, &events);
            last_screenshot_sent = Some(Instant::now());
            let _ = events.send(SessionEvent::Info(
                "Display subscribed; initial screenshot requested while live video starts"
                    .to_owned(),
            ));
            screenshot_pending = true;
        }
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
                                    backlog_min_video_fps,
                                    false,
                                );
                                if next_fps < target_video_fps {
                                    target_video_fps = next_fps;
                                    current_quality = initial_stream_quality(target_video_fps);
                                    last_decoder_recovery = Some(Instant::now());
                                    last_adaptive_raise = Instant::now();
                                    last_quality_change = Instant::now();
                                    low_input_quality_windows = 0;
                                    let _ = events.send(SessionEvent::Info(format!(
                                        "Decoder backlog trimmed ({dropped}); lowering stream to {target_video_fps} fps"
                                    )));
                                    let _ = send_video_start_messages_with_quality(
                                        &mut relay,
                                        current_display,
                                        false,
                                        target_video_fps,
                                        codec_preference,
                                        current_quality,
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
                            current_quality = initial_stream_quality(target_video_fps);
                            last_decoder_recovery = Some(Instant::now());
                            last_quality_change = Instant::now();
                            low_input_quality_windows = 0;
                            let _ = send_video_start_messages_with_quality(
                                &mut relay,
                                current_display,
                                true,
                                target_video_fps,
                                codec_preference,
                                current_quality,
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
                                    current_quality = initial_stream_quality(target_video_fps);
                                    last_decoder_recovery = Some(Instant::now());
                                    last_adaptive_raise = Instant::now();
                                    last_quality_change = Instant::now();
                                    low_input_quality_windows = 0;
                                    let _ = events.send(SessionEvent::Info(format!(
                                        "{codec} decode failed; lowering stream to {target_video_fps} fps"
                                    )));
                                    let _ = send_video_start_messages_with_quality(
                                        &mut relay,
                                        current_display,
                                        false,
                                        target_video_fps,
                                        codec_preference,
                                        current_quality,
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
                        if live_video_seen
                            && queue_ms <= 200
                            && decode_ms <= 60
                            && last_quality_change.elapsed() >= Duration::from_secs(8)
                        {
                            if let Some(next_quality) = next_quality_after_stability(
                                current_quality,
                                target_video_fps,
                                latest_input_fps,
                            ) {
                                let bootstrap_wait = quality_raise_bootstrap_wait(next_quality);
                                if last_live_bootstrap.elapsed() >= bootstrap_wait {
                                    current_quality = next_quality;
                                    last_quality_change = Instant::now();
                                    low_input_quality_windows = 0;
                                    let _ = send_video_start_messages_with_quality(
                                        &mut relay,
                                        current_display,
                                        false,
                                        target_video_fps,
                                        codec_preference,
                                        current_quality,
                                    );
                                    last_live_bootstrap = Instant::now();
                                    let _ = events.send(SessionEvent::Info(format!(
                                        "Stream stable at {latest_input_fps:.1} fps — raised quality to {}",
                                        image_quality_label(current_quality)
                                    )));
                                }
                            }
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
                                    current_quality = initial_stream_quality(target_video_fps);
                                    stable_decoded_frames = 0;
                                    last_adaptive_raise = Instant::now();
                                    last_quality_change = Instant::now();
                                    low_input_quality_windows = 0;
                                    let _ = events.send(SessionEvent::Info(format!(
                                        "Video decode is stable; raising stream to {target_video_fps} fps"
                                    )));
                                    let _ = send_video_start_messages_with_quality(
                                        &mut relay,
                                        current_display,
                                        false,
                                        target_video_fps,
                                        codec_preference,
                                        current_quality,
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
                    SessionCommand::SetClipboardText(text) => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let _ = send_clipboard_text(&mut relay, &text);
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
                        if control_only {
                            continue;
                        }
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
                        if control_only {
                            continue;
                        }
                        live_video_seen = false;
                        let _ = send_switch_display(&mut relay, current_display);
                        let _ = send_video_start_messages_with_quality(
                            &mut relay,
                            current_display,
                            false,
                            target_video_fps,
                            codec_preference,
                            current_quality,
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
                        if control_only {
                            continue;
                        }
                        let _ = send_video_start_messages_with_quality(
                            &mut relay,
                            current_display,
                            false,
                            target_video_fps,
                            codec_preference,
                            current_quality,
                        );
                        last_live_bootstrap = Instant::now();
                    }
                    SessionCommand::RefreshVideo => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        if control_only {
                            continue;
                        }
                        live_video_seen = false;
                        last_decoder_recovery = Some(Instant::now());
                        let _ = events.send(SessionEvent::Info(format!(
                            "Fresh video requested at {target_video_fps} fps"
                        )));
                        let _ = send_video_start_messages_with_quality(
                            &mut relay,
                            current_display,
                            false,
                            target_video_fps,
                            codec_preference,
                            current_quality,
                        );
                        last_live_bootstrap = Instant::now();
                    }
                    SessionCommand::SetVideoFps { fps } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        target_video_fps = fps.clamp(5, 60);
                        current_quality = initial_stream_quality(target_video_fps);
                        if control_only {
                            continue;
                        }
                        last_decoder_recovery = Some(Instant::now());
                        last_quality_change = Instant::now();
                        low_input_quality_windows = 0;
                        let _ = events.send(SessionEvent::Info(format!(
                            "Video fps set to {target_video_fps}"
                        )));
                        let _ = send_video_start_messages_with_quality(
                            &mut relay,
                            current_display,
                            false,
                            target_video_fps,
                            codec_preference,
                            current_quality,
                        );
                        last_live_bootstrap = Instant::now();
                    }
                    SessionCommand::SetVideoProfile { fps, codec } => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        target_video_fps = fps.clamp(5, 60);
                        codec_preference = codec;
                        current_quality = initial_stream_quality(target_video_fps);
                        if control_only {
                            continue;
                        }
                        live_video_seen = false;
                        last_decoder_recovery = Some(Instant::now());
                        last_quality_change = Instant::now();
                        low_input_quality_windows = 0;
                        let _ = events.send(SessionEvent::Info(format!(
                            "Video profile set to {} at {target_video_fps} fps",
                            codec_preference.label()
                        )));
                        let _ = send_video_start_messages_with_quality(
                            &mut relay,
                            current_display,
                            true,
                            target_video_fps,
                            codec_preference,
                            current_quality,
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
                    SessionCommand::ListVms => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let msg = PeerMessage {
                            union: Some(peer_message::Union::Misc(Misc {
                                union: Some(misc::Union::VmListRequest(true)),
                            })),
                        };
                        let _ = send_framed(&mut relay, &encode_peer_message(&msg));
                    }
                    SessionCommand::AttachVm(vm_id) => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let msg = PeerMessage {
                            union: Some(peer_message::Union::Misc(Misc {
                                union: Some(misc::Union::VmAttach(vm_id)),
                            })),
                        };
                        let _ = send_framed(&mut relay, &encode_peer_message(&msg));
                        // Свежее видео после переключения источника (VM ↔ экран).
                        live_video_seen = false;
                        last_decoder_recovery = Some(Instant::now());
                    }
                    SessionCommand::VmPowerOp(json) => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let msg = PeerMessage {
                            union: Some(peer_message::Union::Misc(Misc {
                                union: Some(misc::Union::VmPowerOp(json)),
                            })),
                        };
                        let _ = send_framed(&mut relay, &encode_peer_message(&msg));
                    }
                    SessionCommand::VmCapabilityRequest(vm_id) => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let msg = PeerMessage {
                            union: Some(peer_message::Union::Misc(Misc {
                                union: Some(misc::Union::VmCapabilityRequest(vm_id)),
                            })),
                        };
                        let _ = send_framed(&mut relay, &encode_peer_message(&msg));
                    }
                    SessionCommand::VmCheckpointOp(json) => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let msg = PeerMessage {
                            union: Some(peer_message::Union::Misc(Misc {
                                union: Some(misc::Union::VmCheckpointOp(json)),
                            })),
                        };
                        let _ = send_framed(&mut relay, &encode_peer_message(&msg));
                    }
                    SessionCommand::VmRescueInput(json) => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        let msg = PeerMessage {
                            union: Some(peer_message::Union::Misc(Misc {
                                union: Some(misc::Union::VmRescueInput(json)),
                            })),
                        };
                        let _ = send_framed(&mut relay, &encode_peer_message(&msg));
                    }
                    SessionCommand::Close => {
                        flush_pending_mouse_move(&mut relay, &mut pending_mouse_move);
                        evrt_stop.store(true, std::sync::atomic::Ordering::Relaxed);
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
                        let desc = describe_peer_message(&message);
                        if peer_messages_seen <= 20 || should_log_peer_message(&message) {
                            eprintln!("[session] Peer msg #{peer_messages_seen}: {desc}");
                            let _ = events.send(SessionEvent::Info(format!(
                                "Peer msg #{peer_messages_seen}: {desc}"
                            )));
                        }
                        let mut evrt_port_seen: Option<u16> = None;
                        let mut evrt_candidates: Vec<std::net::SocketAddr> = Vec::new();
                        let fs = handle_session_message(
                            message,
                            &mut relay,
                            &events,
                            &frame_tx,
                            &mut known_displays,
                            current_display,
                            target_video_fps,
                            codec_preference,
                            control_only,
                            &mut evrt_port_seen,
                            &mut evrt_candidates,
                            &mut evrt_token,
                        );
                        // ★ EVRT (после LoginResponse). Собираем кандидаты:
                        //   1) список EvrtEndpoints (LAN+VPN) — основной путь
                        //   2) host IP от hbbs punch-hole + EvrtUdpPort — запасной
                        if !control_only && !evrt_started {
                            let mut candidates = evrt_candidates;
                            if let (Some(port), Some(mut base)) = (evrt_port_seen, evrt_host_base) {
                                base.set_port(port);
                                if !candidates.contains(&base) {
                                    candidates.push(base);
                                }
                            }
                            if !candidates.is_empty() {
                                evrt_started = true;
                                let evrt_events = events.clone();
                                let evrt_ull = initial_video_fps >= 60;
                                let evrt_token_for_thread = evrt_token.clone();
                                eprintln!(
                                    "[evrt-client] пробуем {} кандидат(ов): {:?}",
                                    candidates.len(),
                                    candidates,
                                );
                                let evrt_stop_clone = evrt_stop.clone();
                                thread::spawn(move || {
                                    crate::evrt_client::try_evrt_candidates(
                                        candidates,
                                        evrt_token_for_thread.clone(),
                                        &evrt_events,
                                        evrt_ull,
                                        evrt_stop_clone,
                                    );
                                });
                            }
                        }
                        fs
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
                    evrt_stop.store(true, std::sync::atomic::Ordering::Relaxed);
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
                        let _ = send_video_received(&mut relay);
                        video_metric_packets = video_metric_packets.saturating_add(1);
                        video_metric_bytes = video_metric_bytes.saturating_add(bytes as u64);
                        let metric_elapsed = last_video_packet_metrics.elapsed();
                        if metric_elapsed >= Duration::from_millis(750) {
                            let secs = metric_elapsed.as_secs_f32().max(0.001);
                            let input_fps = video_metric_packets as f32 / secs;
                            let input_kbps =
                                ((video_metric_bytes as f32 * 8.0) / secs / 1000.0).round() as u64;
                            latest_input_fps = input_fps;
                            let _ = events.send(SessionEvent::VideoPacketMetrics {
                                input_fps,
                                input_kbps,
                            });
                            if adaptive_quality && live_video_seen {
                                if let Some(next_quality) = downgrade_quality_for_low_input(
                                    current_quality,
                                    target_video_fps,
                                    input_fps,
                                ) {
                                    let increment =
                                        if quality_drop_is_severe(target_video_fps, input_fps) {
                                            2
                                        } else {
                                            1
                                        };
                                    low_input_quality_windows =
                                        low_input_quality_windows.saturating_add(increment);
                                    if low_input_quality_windows >= 2
                                        && last_quality_change.elapsed() >= Duration::from_secs(2)
                                    {
                                        current_quality = next_quality;
                                        last_quality_change = Instant::now();
                                        low_input_quality_windows = 0;
                                        let _ = send_video_start_messages_with_quality(
                                            &mut relay,
                                            current_display,
                                            false,
                                            target_video_fps,
                                            codec_preference,
                                            current_quality,
                                        );
                                        last_live_bootstrap = Instant::now();
                                        let _ = events.send(SessionEvent::Info(format!(
                                            "Input stream dropped to {input_fps:.1} fps; downgraded quality to {}",
                                            image_quality_label(current_quality)
                                        )));
                                    }
                                } else {
                                    low_input_quality_windows = 0;
                                }
                            }
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

            let qos_feedback_due = !control_only
                && last_qos_feedback_sent
                .map(|instant| instant.elapsed() >= Duration::from_secs(1))
                .unwrap_or(true);
            if qos_feedback_due {
                if let Err(err) = send_stream_qos_feedback(&mut relay, target_video_fps) {
                    qos_feedback_failures = qos_feedback_failures.saturating_add(1);
                    if qos_feedback_failures == 1 || qos_feedback_failures % 10 == 0 {
                        let _ = events.send(SessionEvent::Info(format!(
                            "Video QoS feedback failed: {err}"
                        )));
                    }
                }
                last_qos_feedback_sent = Some(Instant::now());
                qos_feedback_sent = qos_feedback_sent.saturating_add(1);
                if qos_feedback_sent == 1 || qos_feedback_sent % 10 == 0 {
                    let _ = events.send(SessionEvent::Info(format!(
                        "Video QoS feedback sent: {target_video_fps} fps"
                    )));
                }
            }

            // Time-based auto-refresh. Keep this to one display and avoid piling up PNG
            // screenshot requests; otherwise the UI shows old frames from the relay backlog.
            if auto_refresh && !control_only {
                let elapsed =
                    last_screenshot_sent.map_or(Duration::from_secs(999), |t| t.elapsed());
                let request_expired =
                    elapsed >= Duration::from_millis((auto_refresh_millis * 8).clamp(700, 2000));
                let video_is_fresh =
                    live_video_seen && last_frame_received.elapsed() < Duration::from_millis(1200);
                let live_bootstrap_grace =
                    !live_video_seen && last_live_bootstrap.elapsed() < Duration::from_millis(2500);
                if !live_video_seen && last_live_bootstrap.elapsed() >= Duration::from_secs(3) {
                    let _ = send_video_start_messages_with_quality(
                        &mut relay,
                        current_display,
                        true,
                        target_video_fps,
                        codec_preference,
                        current_quality,
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

fn backlog_recovery_min_fps(initial_fps: i32, min_fps: i32) -> i32 {
    let initial = initial_fps.clamp(5, 60);
    let min = min_fps.clamp(5, initial);
    if initial >= 45 {
        min.max(30).min(initial)
    } else {
        min
    }
}

fn raise_adaptive_fps(current: i32, max_fps: i32) -> i32 {
    let ladder = [5, 10, 15, 20, 30, 60];
    ladder
        .iter()
        .copied()
        .find(|fps| *fps > current && *fps <= max_fps)
        .unwrap_or(current)
}

fn best_quality_min_input_fps(target_fps: i32) -> f32 {
    let target = target_fps.clamp(5, 60);
    if target >= 45 {
        target as f32 * 0.85
    } else if target >= 30 {
        target as f32 * 0.8
    } else {
        (target as f32 * 0.75).max(10.0)
    }
}

fn balanced_quality_min_input_fps(target_fps: i32) -> f32 {
    let target = target_fps.clamp(5, 60);
    if target >= 45 {
        30.0
    } else if target >= 30 {
        18.0
    } else {
        10.0
    }
}

fn next_quality_after_stability(
    current: ImageQuality,
    target_fps: i32,
    input_fps: f32,
) -> Option<ImageQuality> {
    match current {
        ImageQuality::Low if input_fps >= balanced_quality_min_input_fps(target_fps) => {
            Some(ImageQuality::Balanced)
        }
        ImageQuality::Balanced if input_fps >= best_quality_min_input_fps(target_fps) => {
            Some(ImageQuality::Best)
        }
        _ => None,
    }
}

fn quality_raise_bootstrap_wait(next_quality: ImageQuality) -> Duration {
    match next_quality {
        ImageQuality::Best => Duration::from_secs(12),
        _ => Duration::from_secs(6),
    }
}

fn quality_drop_is_severe(target_fps: i32, input_fps: f32) -> bool {
    let target = target_fps.clamp(5, 60) as f32;
    input_fps < (target * 0.15).max(6.0)
}

fn downgrade_quality_for_low_input(
    current: ImageQuality,
    target_fps: i32,
    input_fps: f32,
) -> Option<ImageQuality> {
    let target = target_fps.clamp(5, 60) as f32;
    match current {
        ImageQuality::Best if input_fps < (target * 0.35).max(10.0) => Some(ImageQuality::Balanced),
        ImageQuality::Balanced if input_fps < (target * 0.25).max(8.0) => Some(ImageQuality::Low),
        _ => None,
    }
}

fn image_quality_label(quality: ImageQuality) -> &'static str {
    match quality {
        ImageQuality::Low => "Low",
        ImageQuality::Balanced => "Balanced",
        ImageQuality::Best => "Best",
        ImageQuality::NotSet => "NotSet",
    }
}

fn establish_session(
    request: ConnectionRequest,
    progress: &mut impl FnMut(u8, String),
) -> Result<
    (
        TcpStream,
        String,
        Vec<RemoteDisplay>,
        Option<std::net::SocketAddr>, // готовый EVRT addr (если порт в handshake)
        Option<std::net::SocketAddr>, // базовый host UDP IP (для позднего порта)
        Vec<std::net::SocketAddr>,    // EVRT endpoints, если пришли до session loop
        Option<String>,
    ),
    String,
> {
    let session_started = Instant::now();
    let relay_first = relay_first_fast_path();
    let udp_probe = blocking_udp_nat_probe_enabled();
    let direct_tcp_probe = direct_tcp_probe_enabled();
    let control_only = request.control_only;

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
    let id_connect_started = Instant::now();
    let mut rendezvous_stream = connect_tcp(&request.server.id_server, RENDEZVOUS_PORT)?;
    let direct_local_addr = rendezvous_stream.local_addr().ok();
    progress(
        35,
        format!(
            "ID server connected in {} ms",
            elapsed_ms(&id_connect_started)
        ),
    );

    if relay_first {
        progress(
            45,
            "Fast connect: relay-first; direct probes are opt-in".to_owned(),
        );
    } else {
        progress(
            45,
            "Fast connect disabled; direct probes may run".to_owned(),
        );
    }

    let udp_nat = if udp_probe {
        probe_udp_nat_port(&request.server.id_server)
    } else {
        UdpNatProbe {
            port: 0,
            detail: "skipped by fast connect; set EVERTYDESK_CONNECT_UDP_PROBE=1 to enable"
                .to_owned(),
        }
    };
    if udp_nat.port > 0 {
        progress(
            52,
            format!(
                "UDP NAT probe ok; mapped port={} ({})",
                udp_nat.port, udp_nat.detail
            ),
        );
    } else {
        progress(
            52,
            format!("UDP NAT probe unavailable ({})", udp_nat.detail),
        );
    }

    // ── Один запрос к hbbs с force_relay=false ───────────────────────────────
    // Это даёт нам сразу:
    //   • peer_udp_addr — внешний UDP-адрес хоста (для EVRT punch-hole)
    //   • relay_server / relay_uuid — для TCP relay fallback
    // Один RTT к hbbs вместо двух.
    progress(
        60,
        "Sending RustDesk PunchHoleRequest (EVRT probe)".to_owned(),
    );
    let message = RendezvousMessage {
        union: Some(rendezvous_message::Union::PunchHoleRequest(
            PunchHoleRequest {
                id: request.remote_id.clone(),
                nat_type: NatType::UnknownNat as i32,
                licence_key: request.server.public_key.clone(),
                conn_type: ConnType::DefaultConn as i32,
                token: String::new(),
                version: "1.4.6".to_owned(),
                udp_port: udp_nat.port as i32,
                force_relay: false, // ← false: hbbs отдаёт peer_udp_addr
                upnp_port: 0,
                socket_addr_v6: Vec::new(),
            },
        )),
    };
    send_framed(&mut rendezvous_stream, &encode_message(&message))?;

    progress(80, "Waiting for rendezvous response".to_owned());
    let rendezvous_wait_started = Instant::now();
    rendezvous_stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|err| format!("Failed to set read timeout: {err}"))?;
    let response = read_framed(&mut rendezvous_stream)?;
    let decoded = decode_message(&response).map_err(|err| format!("Decode failed: {err}"))?;
    let rendezvous_info = describe_rendezvous_response(&decoded);
    drop(rendezvous_stream);

    // Если force_relay=false не сработал (некоторые hbbs конфиги) → ретрай с force_relay=true
    let rendezvous = match rendezvous_info {
        Ok(info) => info,
        Err(_) => {
            progress(
                82,
                "EVRT probe failed, retrying with force_relay=true".to_owned(),
            );
            let mut rendezvous2 = connect_tcp(&request.server.id_server, RENDEZVOUS_PORT)?;
            let msg2 = RendezvousMessage {
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
            send_framed(&mut rendezvous2, &encode_message(&msg2))?;
            rendezvous2
                .set_read_timeout(Some(Duration::from_secs(5)))
                .ok();
            let resp2 = read_framed(&mut rendezvous2)?;
            let dec2 = decode_message(&resp2).map_err(|e| format!("Decode failed: {e}"))?;
            describe_rendezvous_response(&dec2)?
        }
    };

    progress(
        85,
        format!(
            "Rendezvous response decoded in {} ms (total {} ms)",
            elapsed_ms(&rendezvous_wait_started),
            elapsed_ms(&session_started)
        ),
    );
    let relay_uuid_from_rendezvous = rendezvous.relay_uuid.clone();
    let relay_server = rendezvous
        .relay_server
        .unwrap_or_else(|| request.server.relay_server.clone());
    let secure_relay = rendezvous.has_signed_pk;
    let initial_video_fps = request.display.target_fps.clamp(5, 60) as i32;
    let codec_preference = request.display.codec;
    let host_udp_base = rendezvous
        .peer_udp_addr
        .as_ref()
        .and_then(|b| crate::evrt_session::decode_punch_addr(b));

    if let Some(peer_addr) =
        host_udp_base.filter(|_| !rendezvous.peer_is_udp && direct_tcp_probe && !relay_first)
    {
        progress(86, format!("Trying direct TCP punch → {peer_addr}"));
        match open_direct_tcp_session(
            peer_addr,
            direct_local_addr,
            &request.password,
            &request.remote_id,
                initial_video_fps,
                codec_preference,
                control_only,
                progress,
            ) {
            Ok((
                direct_stream,
                peer_stage,
                displays,
                evrt_port_from_misc,
                early_evrt_candidates,
                evrt_token,
            )) => {
                let evrt_host_addr = evrt_port_from_misc.map(|port| {
                    let mut addr = peer_addr;
                    addr.set_port(port);
                    addr
                });
                return Ok((
                    direct_stream,
                    format!("{peer_stage}; transport=direct-tcp"),
                    displays,
                    evrt_host_addr,
                    Some(peer_addr),
                    early_evrt_candidates,
                    evrt_token,
                ));
            }
            Err(err) => {
                progress(
                    88,
                    format!("Direct TCP failed; falling back to relay: {err}"),
                );
            }
        }
    } else if let Some(peer_addr) = host_udp_base {
        if !rendezvous.peer_is_udp && relay_first {
            progress(
                86,
                format!("Direct TCP candidate {peer_addr}; using relay-first fast path"),
            );
        } else if !rendezvous.peer_is_udp {
            progress(
                86,
                format!(
                    "Direct TCP candidate {peer_addr}; disabled, set EVERTYDESK_TRY_DIRECT_TCP=1 to test it"
                ),
            );
        } else {
            progress(
                86,
                format!(
                    "Rendezvous returned UDP/KCP direct candidate -> {peer_addr}; KCP backend pending, using relay fallback"
                ),
            );
        }
    } else if relay_uuid_from_rendezvous.is_some() {
        progress(
            86,
            "Rendezvous selected relay; no direct candidate returned".to_owned(),
        );
    }

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
        let relay_open_started = Instant::now();
        let mut relay_stream = open_relay_stream(
            &relay_server,
            &request.remote_id,
            &relay_uuid,
            &request.server.public_key,
            secure_relay,
        )?;
        progress(
            94,
            format!(
                "Relay stream opened in {} ms",
                elapsed_ms(&relay_open_started)
            ),
        );

        progress(96, "Waiting for peer secure/login response".to_owned());
        let peer_login_started = Instant::now();
        match read_initial_peer_stage(
            &mut relay_stream,
            &request.password,
            &request.remote_id,
            initial_video_fps,
            codec_preference,
            bootstrap_wait_secs,
            control_only,
            progress,
        ) {
            Ok((
                peer_stage,
                displays,
                evrt_port_from_misc,
                early_evrt_candidates,
                evrt_token,
            )) => {
                progress(
                    98,
                    format!(
                        "Peer login ready in {} ms (total {} ms)",
                        elapsed_ms(&peer_login_started),
                        elapsed_ms(&session_started)
                    ),
                );
                // Базовый UDP-адрес хоста (IP) от hbbs. Порт может прийти позже
                // в Misc{EvrtUdpPort} уже в session loop.
                // Если порт уже пришёл в handshake — готовый адрес.
                let evrt_host_addr = evrt_port_from_misc.and_then(|port| {
                    host_udp_base.map(|mut addr| {
                        addr.set_port(port);
                        addr
                    })
                });
                return Ok((
                    relay_stream,
                    peer_stage,
                    displays,
                    evrt_host_addr,
                    host_udp_base,
                    early_evrt_candidates,
                    evrt_token,
                ));
            }
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

struct UdpNatProbe {
    port: u16,
    detail: String,
}

fn probe_udp_nat_port(id_server: &str) -> UdpNatProbe {
    let (host, port) = split_host_port(id_server, RENDEZVOUS_PORT);
    let Ok(addrs) = (host.as_str(), port).to_socket_addrs() else {
        return UdpNatProbe {
            port: 0,
            detail: format!("{host}:{port} DNS failed"),
        };
    };
    let request = RendezvousMessage {
        union: Some(rendezvous_message::Union::TestNatRequest(TestNatRequest {
            serial: 0,
        })),
    };
    let payload = encode_message(&request);
    let mut last_detail = "no resolved UDP addresses".to_owned();

    for server_addr in addrs {
        let bind_addr = if server_addr.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let Ok(socket) = UdpSocket::bind(bind_addr) else {
            last_detail = format!("bind {bind_addr} failed");
            continue;
        };
        if let Err(err) = socket.connect(server_addr) {
            last_detail = format!("connect {server_addr} failed: {err}");
            continue;
        }
        let _ = socket.set_read_timeout(Some(Duration::from_millis(400)));
        let _ = socket.set_write_timeout(Some(Duration::from_millis(400)));
        let mut buf = [0_u8; 1500];
        let mut retry_sleep = Duration::from_millis(20);

        for attempt in 1..=12 {
            if let Err(err) = socket.send(&payload) {
                last_detail = format!("{server_addr} attempt {attempt}: send failed: {err}");
                continue;
            }
            match socket.recv(&mut buf) {
                Ok(len) => match decode_message(&buf[..len]) {
                    Ok(message) => {
                        if let Some(rendezvous_message::Union::TestNatResponse(response)) =
                            message.union
                        {
                            if response.port > 0 && response.port <= u16::MAX as i32 {
                                return UdpNatProbe {
                                    port: response.port as u16,
                                    detail: format!("{server_addr}, attempt {attempt}"),
                                };
                            }
                            last_detail = format!(
                                "{server_addr} attempt {attempt}: invalid port {}",
                                response.port
                            );
                        } else {
                            last_detail =
                                format!("{server_addr} attempt {attempt}: unexpected UDP response");
                        }
                    }
                    Err(err) => {
                        last_detail =
                            format!("{server_addr} attempt {attempt}: decode failed: {err}");
                    }
                },
                Err(err) => {
                    last_detail = format!("{server_addr} attempt {attempt}: recv failed: {err}");
                }
            }
            thread::sleep(retry_sleep);
            retry_sleep =
                Duration::from_millis((retry_sleep.as_millis() as u64 * 3 / 2).clamp(20, 200));
        }
    }

    UdpNatProbe {
        port: 0,
        detail: last_detail,
    }
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

fn open_direct_tcp_session(
    peer_addr: SocketAddr,
    local_addr: Option<SocketAddr>,
    password: &str,
    remote_id: &str,
    fps: i32,
    codec_preference: CodecPreference,
    control_only: bool,
    progress: &mut impl FnMut(u8, String),
) -> Result<
    (
        TcpStream,
        String,
        Vec<RemoteDisplay>,
        Option<u16>,
        Vec<SocketAddr>,
        Option<String>,
    ),
    String,
> {
    let bind_addr = local_addr.filter(|addr| addr.is_ipv4() == peer_addr.is_ipv4());
    let mut stream = match connect_tcp_addr_bound(
        peer_addr,
        bind_addr,
        Duration::from_secs(DIRECT_TCP_CONNECT_TIMEOUT_SECS),
    ) {
        Ok(stream) => stream,
        Err(bind_err) if bind_addr.is_some() && bind_err.contains("bind") => {
            connect_tcp_addr_bound(
                peer_addr,
                None,
                Duration::from_secs(DIRECT_TCP_CONNECT_TIMEOUT_SECS),
            )
            .map_err(|retry_err| {
                format!("{bind_err}; direct TCP unbound retry failed: {retry_err}")
            })?
        }
        Err(err) => return Err(err),
    };

    progress(
        90,
        format!(
            "Direct TCP connected{}; waiting for peer login",
            bind_addr
                .map(|addr| format!(" from {addr}"))
                .unwrap_or_default()
        ),
    );

    read_initial_peer_stage(
        &mut stream,
        password,
        remote_id,
        fps,
        codec_preference,
        DIRECT_TCP_BOOTSTRAP_WAIT_SECS,
        control_only,
        progress,
    )
    .map(|(peer_stage, displays, evrt_port, evrt_candidates, evrt_token)| {
        (
            stream,
            peer_stage,
            displays,
            evrt_port,
            evrt_candidates,
            evrt_token,
        )
    })
}

fn connect_tcp_addr_bound(
    peer_addr: SocketAddr,
    local_addr: Option<SocketAddr>,
    timeout: Duration,
) -> Result<TcpStream, String> {
    let socket = Socket::new(
        Domain::for_address(peer_addr),
        Type::STREAM,
        Some(Protocol::TCP),
    )
    .map_err(|err| format!("Direct TCP socket create failed: {err}"))?;
    let _ = socket.set_reuse_address(true);
    if let Some(local_addr) = local_addr {
        socket
            .bind(&SockAddr::from(local_addr))
            .map_err(|err| format!("Direct TCP bind {local_addr} failed: {err}"))?;
    }
    socket
        .connect_timeout(&SockAddr::from(peer_addr), timeout)
        .map_err(|err| format!("Direct TCP connect {peer_addr} failed: {err}"))?;

    let stream: TcpStream = socket.into();
    configure_tcp_stream(&stream);
    Ok(stream)
}

fn read_initial_peer_stage(
    relay: &mut TcpStream,
    password: &str,
    remote_id: &str,
    fps: i32,
    codec_preference: CodecPreference,
    bootstrap_wait_secs: u64,
    control_only: bool,
    progress: &mut impl FnMut(u8, String),
) -> Result<
    (
        String,
        Vec<RemoteDisplay>,
        Option<u16>,
        Vec<SocketAddr>,
        Option<String>,
    ),
    String,
> {
    relay
        .set_read_timeout(Some(Duration::from_millis(RELAY_HANDSHAKE_POLL_MS)))
        .map_err(|err| format!("Failed to set relay read timeout: {err}"))?;
    let mut sent_login = false;
    let mut seen_peer_message = false;
    let wait_remote_accept = password.is_empty();
    let mut evrt_port: Option<u16> = None;
    let mut evrt_candidates: Vec<SocketAddr> = Vec::new();
    let mut evrt_token: Option<String> = None;
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
                if !control_only {
                    send_switch_display_subscribe(relay, 0)?;
                    send_video_start_messages(relay, 0, true, fps, codec_preference)?;
                }
                let channel = if control_only {
                    "control channel ready"
                } else {
                    "screenshot/control channel ready"
                };
                return Ok((
                    format!("{login}; {channel}"),
                    displays,
                    evrt_port,
                    evrt_candidates,
                    evrt_token,
                ));
            }
            Some(peer_message::Union::PeerInfo(info)) => {
                send_selected_windows_session_from_peer_info(relay, &info)?;
                let login = format!(
                    "authorized; peer info received: {} {} {}",
                    info.hostname, info.platform, info.version
                );
                let displays = displays_from_peer_info(&info);
                if !control_only {
                    send_switch_display_subscribe(relay, 0)?;
                    send_video_start_messages(relay, 0, true, fps, codec_preference)?;
                }
                let channel = if control_only {
                    "control channel ready"
                } else {
                    "screenshot/control channel ready"
                };
                return Ok((
                    format!("{login}; {channel}"),
                    displays,
                    evrt_port,
                    evrt_candidates,
                    evrt_token,
                ));
            }
            Some(peer_message::Union::PublicKey(_)) => {}
            Some(peer_message::Union::TestDelay(delay)) => {
                echo_test_delay(relay, delay)?;
            }
            Some(peer_message::Union::Misc(misc_msg)) => {
                // ★ EVRT: хост сообщает свой UDP порт для прямого стриминга
                match misc_msg.union {
                    Some(misc::Union::EvrtUdpPort(port)) => {
                        if port > 0 && port <= 65535 {
                            evrt_port = Some(port as u16);
                            eprintln!("[evrt-client] EvrtUdpPort={port} получен");
                        }
                    }
                    Some(misc::Union::EvrtEndpoints(list)) => {
                        if let Some(token) = parse_evrt_token(&list) {
                            evrt_token = Some(token);
                            eprintln!("[evrt-client] ранний EVRT session token получен");
                        }
                        let parsed = parse_evrt_endpoints(&list);
                        if !parsed.is_empty() {
                            eprintln!("[evrt-client] ранние EvrtEndpoints: [{list}]");
                        }
                        for addr in parsed {
                            if !evrt_candidates.contains(&addr) {
                                evrt_candidates.push(addr);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some(peer_message::Union::Shell(_)) => {}
            Some(peer_message::Union::MouseEvent(_))
            | Some(peer_message::Union::KeyEvent(_))
            | Some(peer_message::Union::Clipboard(_))
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
                    evrt_port,
                    evrt_candidates,
                    evrt_token,
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
    send_video_start_messages_with_quality(
        relay,
        display,
        refresh_all,
        fps,
        codec_preference,
        initial_stream_quality(fps),
    )
}

fn send_video_start_messages_with_quality(
    relay: &mut TcpStream,
    display: i32,
    refresh_all: bool,
    fps: i32,
    codec_preference: CodecPreference,
    quality: ImageQuality,
) -> Result<(), String> {
    send_codec_sync_options_quality(relay, fps, codec_preference, quality)?;

    if refresh_all {
        let refresh_all_msg = PeerMessage {
            union: Some(peer_message::Union::Misc(Misc {
                union: Some(misc::Union::RefreshVideo(true)),
            })),
        };
        send_framed(relay, &encode_peer_message(&refresh_all_msg))?;
    }

    send_refresh_video_display(relay, display)
}

fn send_refresh_video_display(relay: &mut TcpStream, display: i32) -> Result<(), String> {
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
    let display = display.max(0);
    send_framed(
        relay,
        &encode_peer_message(&switch_display_message(display)),
    )?;
    send_capture_displays_set(relay, display)?;
    send_message_query_switch_display(relay, display)
}

fn send_codec_sync_options(
    relay: &mut TcpStream,
    fps: i32,
    codec_preference: CodecPreference,
) -> Result<(), String> {
    send_codec_sync_options_quality(relay, fps, codec_preference, initial_stream_quality(fps))
}

fn send_codec_sync_options_quality(
    relay: &mut TcpStream,
    fps: i32,
    codec_preference: CodecPreference,
    quality: ImageQuality,
) -> Result<(), String> {
    let message = PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::Option(video_option_message_quality(
                fps,
                codec_preference,
                quality,
            ))),
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn video_option_message(fps: i32, codec_preference: CodecPreference) -> OptionMessage {
    video_option_message_quality(fps, codec_preference, initial_stream_quality(fps))
}

fn video_option_message_quality(
    fps: i32,
    codec_preference: CodecPreference,
    quality: ImageQuality,
) -> OptionMessage {
    OptionMessage {
        image_quality: quality as i32,
        custom_image_quality: 0,
        supported_decoding: Some(supported_decoding(codec_preference)),
        custom_fps: fps.clamp(5, 60),
    }
}

fn initial_stream_quality(fps: i32) -> ImageQuality {
    if fps >= 45 {
        ImageQuality::Low
    } else {
        ImageQuality::Balanced
    }
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
        // Explicit client choice → advertise that concrete codec (the host
        // honours it as a strong preference over raw quality ranking).
        CodecPreference::Av1 if av1_capable => PreferCodec::Av1,
        CodecPreference::H265 if h265_capable => PreferCodec::H265,
        CodecPreference::H264 if h264_capable => PreferCodec::H264,
        CodecPreference::Vp9 if vp9_capable => PreferCodec::Vp9,
        // Auto → advertise Auto so the host's capability-aware brain picks the
        // best codec both ends can hardware-handle. Advertising a concrete codec
        // here would override that and pin the session to it.
        CodecPreference::Auto => PreferCodec::Auto,
        // Explicit choice not decodable on this machine → fall back to whatever
        // we *can* decode, best-first.
        _ if h265_capable => PreferCodec::H265,
        _ if h264_capable => PreferCodec::H264,
        _ if av1_capable => PreferCodec::Av1,
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
            | Some(peer_message::Union::Clipboard(_))
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
    delay: crate::rustdesk_proto::TestDelay,
) -> Result<(), String> {
    let Some(message) = test_delay_echo_message(delay) else {
        return Ok(());
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn test_delay_echo_message(delay: TestDelay) -> Option<PeerMessage> {
    if delay.from_client {
        return None;
    }
    Some(PeerMessage {
        union: Some(peer_message::Union::TestDelay(delay)),
    })
}

fn send_stream_qos_feedback(relay: &mut TcpStream, fps: i32) -> Result<(), String> {
    send_framed(relay, &encode_peer_message(&custom_fps_option_message(fps)))?;
    send_framed(relay, &encode_peer_message(&auto_adjust_fps_message(fps)))?;
    send_video_received(relay)?;
    send_framed(
        relay,
        &encode_peer_message(&server_test_delay_ack_message()),
    )?;
    send_framed(
        relay,
        &encode_peer_message(&client_test_delay_message(current_time_millis())),
    )
}

fn custom_fps_option_message(fps: i32) -> PeerMessage {
    PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::Option(OptionMessage {
                image_quality: ImageQuality::NotSet as i32,
                custom_image_quality: 0,
                supported_decoding: None,
                custom_fps: fps.clamp(5, 60),
            })),
        })),
    }
}

fn auto_adjust_fps_message(fps: i32) -> PeerMessage {
    PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::AutoAdjustFps(fps.clamp(5, 60) as u32)),
        })),
    }
}

fn client_test_delay_message(now_ms: i64) -> PeerMessage {
    PeerMessage {
        union: Some(peer_message::Union::TestDelay(TestDelay {
            time: now_ms,
            from_client: true,
            last_delay: 0,
            target_bitrate: 0,
        })),
    }
}

fn server_test_delay_ack_message() -> PeerMessage {
    PeerMessage {
        union: Some(peer_message::Union::TestDelay(TestDelay {
            time: 0,
            from_client: false,
            last_delay: 0,
            target_bitrate: 0,
        })),
    }
}

fn current_time_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
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

fn parse_evrt_endpoints(list: &str) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    for part in list.split(',') {
        if let Ok(addr) = part.trim().parse::<SocketAddr>() {
            if !out.contains(&addr) {
                out.push(addr);
            }
        }
    }
    out
}

fn parse_evrt_token(list: &str) -> Option<String> {
    for part in list.split(',') {
        let part = part.trim();
        let token = part
            .strip_prefix("token=")
            .or_else(|| part.strip_prefix("sessionToken="));
        if let Some(token) = token {
            if crate::evrt::valid_session_token(token) {
                return Some(token.to_owned());
            }
        }
    }
    None
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
    control_only: bool,
    evrt_port_out: &mut Option<u16>,
    evrt_candidates_out: &mut Vec<std::net::SocketAddr>,
    evrt_token_out: &mut Option<String>,
) -> Option<FrameSource> {
    match message.union {
        Some(peer_message::Union::ScreenshotResponse(response)) => {
            if control_only {
                return None;
            }
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
            if control_only {
                return None;
            }
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
            let rtt = if delay.from_client && delay.time > 0 {
                let client_rtt = current_time_millis()
                    .saturating_sub(delay.time)
                    .clamp(0, u32::MAX as i64) as u32;
                let _ = events.send(SessionEvent::Info(format!(
                    "TestDelay echo received: {client_rtt} ms"
                )));
                client_rtt
            } else {
                // last_delay is the RTT (ms) the server measured for the previous round-trip.
                let server_rtt = delay.last_delay;
                let _ = events.send(SessionEvent::Info(format!(
                    "TestDelay probe received: last_delay={server_rtt} ms, target_bitrate={} kbps",
                    delay.target_bitrate
                )));
                let _ = echo_test_delay(relay, delay);
                server_rtt
            };
            if rtt > 0 {
                let _ = events.send(SessionEvent::Latency(rtt));
            }
            None
        }
        Some(peer_message::Union::LoginResponse(response)) => {
            update_displays_from_login_response(&response, known_displays, events);
            let _ = send_selected_windows_session(relay, &response);
            if !control_only {
                let _ = send_video_start_messages(
                    relay,
                    current_display,
                    false,
                    target_video_fps,
                    codec_preference,
                );
            }
            if login_response_is_remote_accept_wait(&response) {
                let _ = events.send(SessionEvent::Info("Waiting for remote accept".to_owned()));
            }
            None
        }
        Some(peer_message::Union::PeerInfo(info)) => {
            update_displays_from_peer_info(&info, known_displays, events);
            let _ = send_selected_windows_session_from_peer_info(relay, &info);
            if !control_only {
                let _ = send_video_start_messages(
                    relay,
                    current_display,
                    false,
                    target_video_fps,
                    codec_preference,
                );
            }
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
                            "[session] Server codec reply: prefer={} h264={} h265={} av1={} vp9={} vp8={} quality={} custom_quality={} fps={}",
                            dec.prefer,
                            dec.ability_h264,
                            dec.ability_h265,
                            dec.ability_av1,
                            dec.ability_vp9,
                            dec.ability_vp8,
                            opt.image_quality,
                            opt.custom_image_quality,
                            opt.custom_fps
                        );
                    } else {
                        eprintln!(
                            "[session] Server Misc::Option (quality={} custom_quality={} fps={}, no decoding info)",
                            opt.image_quality, opt.custom_image_quality, opt.custom_fps
                        );
                    }
                }
                Some(misc::Union::SwitchDisplay(sd)) => {
                    eprintln!(
                        "[session] Server SwitchDisplay confirmed: display={} {}x{} @ {},{}",
                        sd.display, sd.width, sd.height, sd.x, sd.y
                    );
                    update_display_from_switch(&sd, known_displays, events);
                    // No response needed — just log. Sending SwitchDisplay back
                    // would create an infinite SwitchDisplay ↔ SwitchDisplay loop.
                }
                Some(misc::Union::RefreshVideo(_)) => {
                    eprintln!("[session] Server requests RefreshVideo");
                }
                Some(misc::Union::CaptureDisplays(displays)) => {
                    eprintln!(
                        "[session] Server CaptureDisplays add={:?} sub={:?} set={:?}",
                        displays.add, displays.sub, displays.set
                    );
                }
                Some(misc::Union::MessageQuery(query)) => {
                    eprintln!(
                        "[session] Server MessageQuery switch_display={}",
                        query.switch_display
                    );
                }
                Some(misc::Union::EvrtUdpPort(port)) => {
                    // Старый путь: только порт (нужен IP от hbbs punch-hole).
                    let p = (*port).min(65535) as u16;
                    if p > 0 {
                        eprintln!("[session] EvrtUdpPort={p} получен (session loop)");
                        *evrt_port_out = Some(p);
                    }
                }
                Some(misc::Union::EvrtEndpoints(list)) => {
                    // ★ Новый путь: список IP:порт кандидатов (LAN+VPN). mini-ICE.
                    eprintln!("[session] ★ EvrtEndpoints получены: [{list}]");
                    if let Some(token) = parse_evrt_token(list) {
                        *evrt_token_out = Some(token);
                        eprintln!("[session] EVRT session token received");
                    }
                    for addr in parse_evrt_endpoints(list) {
                        if !evrt_candidates_out.contains(&addr) {
                            evrt_candidates_out.push(addr);
                        }
                    }
                }
                Some(misc::Union::HostTelemetry(info)) => {
                    // ★ Телеметрия хоста — пробрасываем в --diagnose отчёт.
                    eprintln!("[session] ★ Хост-энкодер: {info}");
                    let _ = events.send(SessionEvent::Info(format!("★ Хост-энкодер: {info}")));
                }
                Some(misc::Union::VmList(json)) => {
                    eprintln!("[session] VM list received");
                    let _ = events.send(SessionEvent::VmList(json.clone()));
                }
                Some(misc::Union::VmStatus(status)) => {
                    eprintln!("[session] VM status: {status}");
                    let _ = events.send(SessionEvent::VmStatus(status.clone()));
                }
                Some(misc::Union::VmPowerResult(json)) => {
                    eprintln!("[session] VM power result: {json}");
                    let _ = events.send(SessionEvent::VmPowerResult(json.clone()));
                }
                Some(misc::Union::VmCapabilityGraph(json)) => {
                    eprintln!("[session] VM capabilities received");
                    let _ = events.send(SessionEvent::VmCapabilities(json.clone()));
                }
                Some(misc::Union::VmCheckpoints(json)) => {
                    eprintln!("[session] VM checkpoints received");
                    let _ = events.send(SessionEvent::VmCheckpoints(json.clone()));
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
        Some(peer_message::Union::Clipboard(clipboard)) => {
            match clipboard_text_from_message(clipboard) {
                Ok(Some(text)) => {
                    let _ = events.send(SessionEvent::ClipboardText(text));
                }
                Ok(None) => {
                    let _ = events.send(SessionEvent::Info(
                        "Clipboard: unsupported format".to_owned(),
                    ));
                }
                Err(err) => {
                    let _ = events.send(SessionEvent::Info(format!(
                        "Clipboard decode failed: {err}"
                    )));
                }
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
    if let Some(crate::rustdesk_proto::login_response::Union::PeerInfo(info)) = &response.union {
        update_displays_from_peer_info(info, known_displays, events);
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
    } else {
        let _ = events.send(SessionEvent::Info(format!(
            "PeerInfo displays empty: {}",
            peer_info_context(info)
        )));
    }
}

fn update_display_from_switch(
    display: &SwitchDisplay,
    known_displays: &mut Vec<RemoteDisplay>,
    events: &Sender<SessionEvent>,
) {
    let index = display.display.max(0);
    if display.width <= 0 || display.height <= 0 {
        let _ = events.send(SessionEvent::Info(format!(
            "SwitchDisplay confirmed display {}, but geometry is empty",
            index.saturating_add(1)
        )));
        return;
    }

    let updated = RemoteDisplay {
        index,
        name: known_displays
            .iter()
            .find(|known| known.index == index)
            .map(|known| known.name.clone())
            .unwrap_or_else(|| format!("Display {}", index.saturating_add(1))),
        width: display.width,
        height: display.height,
        x: display.x,
        y: display.y,
        cursor_embedded: display.cursor_embedded,
    };

    if let Some(existing) = known_displays
        .iter_mut()
        .find(|known| known.index == updated.index)
    {
        if display_geometry_matches(existing, &updated) {
            return;
        }
        *existing = updated;
    } else {
        known_displays.push(updated);
        known_displays.sort_by_key(|display| display.index);
    }

    let _ = events.send(SessionEvent::Info(format!(
        "SwitchDisplay geometry received: display {} {}x{} @ {},{}",
        index.saturating_add(1),
        display.width,
        display.height,
        display.x,
        display.y
    )));
    let _ = events.send(SessionEvent::Displays(known_displays.clone()));
}

fn display_geometry_matches(a: &RemoteDisplay, b: &RemoteDisplay) -> bool {
    a.index == b.index
        && a.name == b.name
        && a.width == b.width
        && a.height == b.height
        && a.x == b.x
        && a.y == b.y
        && a.cursor_embedded == b.cursor_embedded
}

fn peer_info_context(info: &crate::rustdesk_proto::PeerInfo) -> String {
    format!(
        "hostname={}, platform={}, version={}, current_display={}, displays={}",
        info.hostname,
        info.platform,
        info.version,
        info.current_display,
        peer_info_display_summary(info)
    )
}

fn peer_info_display_summary(info: &crate::rustdesk_proto::PeerInfo) -> String {
    if info.displays.is_empty() {
        return "0".to_owned();
    }

    info.displays
        .iter()
        .enumerate()
        .map(|(index, display)| {
            let name = if display.name.is_empty() {
                format!("Display {}", index + 1)
            } else {
                display.name.clone()
            };
            format!(
                "#{index} {name}: {}x{} @ {},{} online={} cursor={} scale={:.2}",
                display.width,
                display.height,
                display.x,
                display.y,
                display.online,
                display.cursor_embedded,
                display.scale
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
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
    // How many consecutive frames VT has returned "needs more packets".
    // VT is allowed a few startup frames; after VT_FAIL_LIMIT it is disabled.
    let mut h264_vt_fail_streak: u32 = 0;
    const VT_FAIL_LIMIT: u32 = 5;

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

    // macOS: hardware HEVC decode via VideoToolbox. Without this an H265 stream
    // from a Windows/NVENC host (no MF on macOS) cannot be decoded at all.
    let mut h265_vt = if crate::videotoolbox::videotoolbox_h264_decoder_available() {
        eprintln!("[decoder] macOS VideoToolbox H265 decoder ready");
        Some(crate::videotoolbox::VideoToolboxH264Decoder::new_hevc())
    } else {
        None
    };

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
            // If VT has failed too many times in a row, disable it so we don't
            // keep attempting hardware decode and silently falling through to
            // the slow OpenH264 software path on every single frame.
            if h264_vt_fail_streak >= VT_FAIL_LIMIT {
                if h264_vt.is_some() {
                    eprintln!("[decoder] VT failed {VT_FAIL_LIMIT} frames in a row — disabling, using OpenH264");
                    h264_vt = None;
                }
            }
            match decode_one_frame(
                frame,
                &mut h264_vt,
                &mut h265_vt,
                &mut h264,
                &mut vp8,
                &mut vp9,
                &mut vp9_sys,
                &mut vp9_mf,
                &mut h265_mf,
                &mut av1_mf,
            ) {
                Ok(Some(event)) => {
                    h264_vt_fail_streak = 0;
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
                Ok(None) => {
                    // VT returned no frame — may be startup buffering.
                    if codec == "H264" && h264_vt.is_some() {
                        h264_vt_fail_streak = h264_vt_fail_streak.saturating_add(1);
                    }
                }
                Err(err) if decoder_needs_more_packets(&err) => {
                    if codec == "H264" && h264_vt.is_some() {
                        h264_vt_fail_streak = h264_vt_fail_streak.saturating_add(1);
                    }
                }
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
    // Keep at most 3 frames per batch. Remote desktop latency matters more
    // than smooth playback: always show the most recent frame quickly.
    const MAX_VIDEO_BACKLOG: usize = 3;
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
    h265_vt: &mut Option<crate::videotoolbox::VideoToolboxH264Decoder>,
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
                    // VT alive but produced no output yet (startup buffering).
                    // Return Ok(None) — do NOT fall through to OpenH264.
                    Ok(None) => return Ok(None),
                    Err(err) if decoder_needs_more_packets(&err) => return Ok(None),
                    Err(err) => Some(err),
                }
            } else {
                None
            };

            // Only reach here when VT is disabled (None) or returned a real error.
            if let Some(err) = vt_error.as_ref() {
                eprintln!("[decoder] VideoToolbox H264 error, disabling: {err}");
                *h264_vt = None;
            }

            // OpenH264 software fallback — only used when VT is None.
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
                        "H264 frame received but VideoToolbox failed and live-h264 is not compiled: {err}"
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
        } => {
            // macOS VideoToolbox hardware HEVC first (the only HEVC decoder on
            // macOS); Media Foundation is the Windows fallback.
            if let Some(decoder) = h265_vt.as_mut() {
                match decoder.decode_packets(frames.frames.iter().map(|frame| frame.data.clone())) {
                    Ok(Some((width, height, rgba))) => {
                        return Ok(Some(SessionEvent::Frame {
                            sid,
                            codec: "H265".to_owned(),
                            width,
                            height,
                            rgba,
                        }));
                    }
                    Ok(None) => return Ok(None),
                    Err(err) if decoder_needs_more_packets(&err) => return Ok(None),
                    Err(err) => {
                        eprintln!("[decoder] VideoToolbox H265 error, disabling: {err}");
                        *h265_vt = None;
                    }
                }
            }

            decode_mf_video_rgba(
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
            })
        }
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

fn send_switch_display(relay: &mut TcpStream, display: i32) -> Result<(), String> {
    let display = display.max(0);
    send_framed(
        relay,
        &encode_peer_message(&switch_display_message(display)),
    )?;
    send_capture_displays_set(relay, display)?;
    send_message_query_switch_display(relay, display)
}

fn switch_display_message(display: i32) -> PeerMessage {
    let switch_display = SwitchDisplay {
        display: display.max(0),
        ..Default::default()
    };
    PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::SwitchDisplay(switch_display)),
        })),
    }
}

fn send_capture_displays_set(relay: &mut TcpStream, display: i32) -> Result<(), String> {
    send_framed(
        relay,
        &encode_peer_message(&capture_displays_set_message(display)),
    )
}

fn send_message_query_switch_display(relay: &mut TcpStream, display: i32) -> Result<(), String> {
    send_framed(
        relay,
        &encode_peer_message(&message_query_switch_display_message(display)),
    )
}

fn message_query_switch_display_message(display: i32) -> PeerMessage {
    PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::MessageQuery(MessageQuery {
                switch_display: display.max(0),
            })),
        })),
    }
}

fn capture_displays_set_message(display: i32) -> PeerMessage {
    PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::CaptureDisplays(CaptureDisplays {
                add: Vec::new(),
                sub: Vec::new(),
                set: vec![display.max(0)],
            })),
        })),
    }
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

fn send_clipboard_text(relay: &mut TcpStream, text: &str) -> Result<(), String> {
    send_framed(relay, &encode_peer_message(&clipboard_text_message(text)))
}

fn clipboard_text_message(text: &str) -> PeerMessage {
    PeerMessage {
        union: Some(peer_message::Union::Clipboard(Clipboard {
            compress: false,
            content: text.as_bytes().to_vec(),
            width: 0,
            height: 0,
            format: ClipboardFormat::Text as i32,
            special_name: String::new(),
        })),
    }
}

pub(crate) fn clipboard_text_from_message(clipboard: Clipboard) -> Result<Option<String>, String> {
    if ClipboardFormat::try_from(clipboard.format) != Ok(ClipboardFormat::Text) {
        return Ok(None);
    }

    let content = if clipboard.compress {
        zstd::decode_all(clipboard.content.as_slice())
            .map_err(|err| format!("zstd clipboard decode failed: {err}"))?
    } else {
        clipboard.content
    };
    String::from_utf8(content)
        .map(Some)
        .map_err(|err| format!("clipboard text is not UTF-8: {err}"))
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
                format!("LoginResponse PeerInfo {}", peer_info_context(info))
            }
            Some(crate::rustdesk_proto::login_response::Union::Error(err)) => {
                format!("LoginResponse Error {err}")
            }
            None => "LoginResponse empty".to_owned(),
        },
        Some(peer_message::Union::PeerInfo(info)) => {
            format!("PeerInfo {}", peer_info_context(info))
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
        Some(peer_message::Union::Clipboard(clipboard)) => {
            format!(
                "Clipboard format={} compressed={} bytes={}",
                clipboard.format,
                clipboard.compress,
                clipboard.content.len()
            )
        }
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

fn should_log_peer_message(message: &PeerMessage) -> bool {
    !matches!(message.union, Some(peer_message::Union::VideoFrame(_)))
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
            Ok(format!("{prefix}; peer info: {}", peer_info_context(&info)))
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
            option: Some(video_option_message(fps, codec_preference)),
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
    /// Внешний UDP-адрес хоста из `PunchHoleResponse.socket_addr` —
    /// используется для попытки прямого EVRT-соединения.
    peer_udp_addr: Option<Vec<u8>>,
    peer_is_udp: bool,
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
                peer_udp_addr: (!response.socket_addr.is_empty())
                    .then(|| response.socket_addr.clone()),
                peer_is_udp: response.is_udp,
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
                    peer_udp_addr: None,
                    peer_is_udp: false,
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
        Some(rendezvous_message::Union::TestNatRequest(_)) => {
            Err("Unexpected TestNatRequest response".to_owned())
        }
        Some(rendezvous_message::Union::TestNatResponse(_)) => {
            Err("Unexpected TestNatResponse response".to_owned())
        }
        Some(rendezvous_message::Union::PunchHole(_)) => {
            Err("Unexpected PunchHole response".to_owned())
        }
        Some(rendezvous_message::Union::FetchLocalAddr(_)) => {
            Err("Unexpected FetchLocalAddr response".to_owned())
        }
        Some(rendezvous_message::Union::PeerDiscovery(_)) => {
            Err("Unexpected PeerDiscovery response".to_owned())
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
    fn evrt_endpoint_parser_ignores_session_token_part() {
        let endpoints = parse_evrt_endpoints(
            "192.168.1.10:40000,token=0123456789abcdef0123456789abcdef,10.0.0.2:40000",
        );
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].to_string(), "192.168.1.10:40000");
        assert_eq!(endpoints[1].to_string(), "10.0.0.2:40000");
    }

    #[test]
    fn evrt_token_parser_extracts_valid_hex_token() {
        assert_eq!(
            parse_evrt_token(
                "192.168.1.10:40000,token=0123456789abcdef0123456789abcdef"
            )
            .as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert!(parse_evrt_token("192.168.1.10:40000,token=short").is_none());
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
        let option = login.option.unwrap();
        assert_eq!(option.custom_fps, 60);
        assert_eq!(option.image_quality, ImageQuality::Low as i32);
        assert_eq!(option.custom_image_quality, 0);
        assert!(option.supported_decoding.is_some());
        assert!(!login.video_ack_required);
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
    fn test_delay_echo_preserves_server_probe_flag() {
        let message = test_delay_echo_message(TestDelay {
            time: 42,
            from_client: false,
            last_delay: 17,
            target_bitrate: 2_000,
        })
        .expect("server TestDelay probes must be echoed");

        let Some(peer_message::Union::TestDelay(delay)) = message.union else {
            panic!("expected TestDelay echo");
        };
        assert!(!delay.from_client);
        assert_eq!(delay.last_delay, 17);
        assert_eq!(delay.target_bitrate, 2_000);
    }

    #[test]
    fn test_delay_echo_ignores_client_originated_probe() {
        assert!(test_delay_echo_message(TestDelay {
            time: 42,
            from_client: true,
            last_delay: 17,
            target_bitrate: 2_000,
        })
        .is_none());
    }

    #[test]
    fn custom_fps_feedback_does_not_reset_quality_or_codec() {
        let message = custom_fps_option_message(120);
        let Some(peer_message::Union::Misc(misc)) = message.union else {
            panic!("expected Misc message");
        };
        let Some(misc::Union::Option(option)) = misc.union else {
            panic!("expected Option message");
        };
        assert_eq!(option.custom_fps, 60);
        assert_eq!(option.image_quality, ImageQuality::NotSet as i32);
        assert_eq!(option.custom_image_quality, 0);
        assert!(option.supported_decoding.is_none());
    }

    #[test]
    fn auto_adjust_fps_feedback_uses_rustdesk_misc_field() {
        let message = auto_adjust_fps_message(60);
        let Some(peer_message::Union::Misc(misc)) = message.union else {
            panic!("expected Misc message");
        };
        let Some(misc::Union::AutoAdjustFps(fps)) = misc.union else {
            panic!("expected AutoAdjustFps message");
        };
        assert_eq!(fps, 60);
    }

    #[test]
    fn capture_displays_set_selects_single_display() {
        let message = capture_displays_set_message(2);
        let Some(peer_message::Union::Misc(misc)) = message.union else {
            panic!("expected Misc message");
        };
        let Some(misc::Union::CaptureDisplays(displays)) = misc.union else {
            panic!("expected CaptureDisplays message");
        };
        assert!(displays.add.is_empty());
        assert!(displays.sub.is_empty());
        assert_eq!(displays.set, vec![2]);
    }

    #[test]
    fn capture_displays_set_clamps_negative_display() {
        let message = capture_displays_set_message(-5);
        let Some(peer_message::Union::Misc(misc)) = message.union else {
            panic!("expected Misc message");
        };
        let Some(misc::Union::CaptureDisplays(displays)) = misc.union else {
            panic!("expected CaptureDisplays message");
        };
        assert_eq!(displays.set, vec![0]);
    }

    #[test]
    fn message_query_requests_switch_display_geometry() {
        let message = message_query_switch_display_message(2);
        let Some(peer_message::Union::Misc(misc)) = message.union else {
            panic!("expected Misc message");
        };
        let Some(misc::Union::MessageQuery(query)) = misc.union else {
            panic!("expected MessageQuery message");
        };
        assert_eq!(query.switch_display, 2);
    }

    #[test]
    fn switch_display_response_updates_known_displays() {
        let (tx, rx) = mpsc::channel();
        let mut known = Vec::new();

        update_display_from_switch(
            &SwitchDisplay {
                display: 1,
                x: 1920,
                y: 0,
                width: 2560,
                height: 1440,
                cursor_embedded: true,
            },
            &mut known,
            &tx,
        );

        assert_eq!(known.len(), 1);
        assert_eq!(known[0].index, 1);
        assert_eq!(known[0].width, 2560);
        assert_eq!(known[0].height, 1440);
        assert_eq!(known[0].x, 1920);
        assert!(known[0].cursor_embedded);
        assert!(rx
            .try_iter()
            .any(|event| matches!(event, SessionEvent::Displays(displays) if displays.len() == 1 && displays[0].index == 1)));
    }

    #[test]
    fn switch_display_message_does_not_request_resolution_change() {
        let message = switch_display_message(2);
        let Some(peer_message::Union::Misc(misc)) = message.union else {
            panic!("expected Misc message");
        };
        let Some(misc::Union::SwitchDisplay(display)) = misc.union else {
            panic!("expected SwitchDisplay message");
        };
        assert_eq!(display.display, 2);
        assert_eq!(display.width, 0);
        assert_eq!(display.height, 0);
        assert_eq!(display.x, 0);
        assert_eq!(display.y, 0);
    }

    #[test]
    fn clipboard_text_message_uses_rustdesk_clipboard_field() {
        let message = clipboard_text_message("привет");
        let Some(peer_message::Union::Clipboard(clipboard)) = message.union else {
            panic!("expected Clipboard message");
        };
        assert!(!clipboard.compress);
        assert_eq!(clipboard.format, ClipboardFormat::Text as i32);
        assert_eq!(clipboard.content, "привет".as_bytes());
    }

    #[test]
    fn clipboard_text_from_message_decodes_zstd_text() {
        let compressed = zstd::encode_all("hello".as_bytes(), 0).unwrap();
        let clipboard = Clipboard {
            compress: true,
            content: compressed,
            width: 0,
            height: 0,
            format: ClipboardFormat::Text as i32,
            special_name: String::new(),
        };
        assert_eq!(
            clipboard_text_from_message(clipboard).unwrap(),
            Some("hello".to_owned())
        );
    }

    #[test]
    fn client_test_delay_marks_probe_as_client_originated() {
        let message = client_test_delay_message(12345);
        let Some(peer_message::Union::TestDelay(delay)) = message.union else {
            panic!("expected TestDelay message");
        };
        assert!(delay.from_client);
        assert_eq!(delay.time, 12345);
        assert_eq!(delay.last_delay, 0);
    }

    #[test]
    fn server_test_delay_ack_looks_like_rustdesk_echo() {
        let message = server_test_delay_ack_message();
        let Some(peer_message::Union::TestDelay(delay)) = message.union else {
            panic!("expected TestDelay message");
        };
        assert!(!delay.from_client);
        assert_eq!(delay.last_delay, 0);
    }

    #[test]
    fn initial_quality_prefers_speed_for_high_fps() {
        assert_eq!(initial_stream_quality(60), ImageQuality::Low);
        assert_eq!(initial_stream_quality(45), ImageQuality::Low);
        assert_eq!(initial_stream_quality(30), ImageQuality::Balanced);
    }

    #[test]
    fn auto_codec_defers_to_host_capability_brain() {
        // On Auto the client advertises Auto regardless of its own decode mix —
        // the host's capability-aware negotiation picks the best codec both ends
        // can hardware-handle. Advertising a concrete codec here would pin it.
        for (h264, h265, av1, vp9) in [
            (true, false, false, true),
            (true, true, false, true),
            (true, true, true, true),
            (false, true, true, true),
            (false, false, true, true),
        ] {
            assert_eq!(
                preferred_codec(CodecPreference::Auto, h264, h265, av1, vp9) as i32,
                PreferCodec::Auto as i32,
            );
        }
    }

    #[test]
    fn explicit_codec_preference_falls_back_to_best_supported() {
        // H265 requested but undecodable here → fall back to H264 (only option).
        assert_eq!(
            preferred_codec(CodecPreference::H265, true, false, false, true) as i32,
            PreferCodec::H264 as i32
        );
        // AV1 requested but undecodable → fall back to the *best* available
        // decoder, which is H265 (new "latch onto the best" philosophy).
        assert_eq!(
            preferred_codec(CodecPreference::Av1, true, true, false, true) as i32,
            PreferCodec::H265 as i32
        );
        // VP9 requested but undecodable, only H264 available → H264.
        assert_eq!(
            preferred_codec(CodecPreference::Vp9, true, false, false, false) as i32,
            PreferCodec::H264 as i32
        );
    }

    #[test]
    fn backlog_recovery_keeps_interactive_floor() {
        assert_eq!(backlog_recovery_min_fps(60, 5), 30);
        assert_eq!(
            lower_adaptive_fps(60, backlog_recovery_min_fps(60, 5), false),
            30
        );
        assert_eq!(
            lower_adaptive_fps(30, backlog_recovery_min_fps(60, 5), false),
            30
        );
    }

    #[test]
    fn backlog_recovery_respects_low_target_profiles() {
        assert_eq!(backlog_recovery_min_fps(30, 10), 10);
        assert_eq!(
            lower_adaptive_fps(30, backlog_recovery_min_fps(30, 10), false),
            20
        );
    }

    #[test]
    fn best_quality_requires_real_incoming_fps() {
        assert_eq!(best_quality_min_input_fps(60), 51.0);
        assert_eq!(best_quality_min_input_fps(45), 38.25);
        assert_eq!(best_quality_min_input_fps(30), 24.0);
        assert_eq!(best_quality_min_input_fps(20), 15.0);
    }

    #[test]
    fn high_fps_quality_raises_in_steps() {
        assert_eq!(
            next_quality_after_stability(ImageQuality::Low, 60, 40.0),
            Some(ImageQuality::Balanced)
        );
        assert_eq!(
            next_quality_after_stability(ImageQuality::Balanced, 60, 39.0),
            None
        );
        assert_eq!(
            next_quality_after_stability(ImageQuality::Balanced, 60, 52.0),
            Some(ImageQuality::Best)
        );
    }

    #[test]
    fn low_input_downgrades_quality_after_best_collapse() {
        assert_eq!(
            downgrade_quality_for_low_input(ImageQuality::Best, 60, 2.3),
            Some(ImageQuality::Balanced)
        );
        assert_eq!(
            downgrade_quality_for_low_input(ImageQuality::Balanced, 60, 6.0),
            Some(ImageQuality::Low)
        );
        assert_eq!(
            downgrade_quality_for_low_input(ImageQuality::Low, 60, 2.0),
            None
        );
        assert!(quality_drop_is_severe(60, 2.3));
        assert!(!quality_drop_is_severe(60, 10.4));
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

// ─── Pub helpers for video_pipeline ──────────────────────────────────────────

/// Публичный алиас encode_peer_message для video_pipeline.
pub fn encode_peer_message_raw(msg: &crate::rustdesk_proto::PeerMessage) -> Vec<u8> {
    crate::rustdesk_proto::encode_peer_message(msg)
}

/// Публичный алиас send_framed для video_pipeline.
pub fn send_framed_raw(stream: &mut std::net::TcpStream, payload: &[u8]) -> Result<(), String> {
    send_framed(stream, payload)
}
