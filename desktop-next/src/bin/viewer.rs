#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use evertydesk_core::rustdesk_proto::ControlKey;
use evertydesk_core::settings::{AppConfig, CodecPreference, DisplayConfig, StreamingMode};
use evertydesk_core::transport::{
    ConnectionRequest, RemoteDisplay, SessionCommand, SessionEvent, TransportClient,
};
use evertydesk_desktop_next::frame_renderer::{FrameRenderer, FrameRendererError, ScalingMode};
use evertydesk_desktop_next::ipc::{read_bounded_line, MAX_IPC_LINE_BYTES};
use evertydesk_desktop_next::protocol::{
    ConnectionQuality, ViewerBootstrap, ViewerCommand, ViewerControl, ViewerGameCodec,
    ViewerScaling, ViewerStatus,
};
use evertydesk_desktop_next::startup_log::install_process_diagnostics;
use evertydesk_desktop_next::windows_app::{
    set_current_process_app_user_model_id, WindowsAppUserModelId,
};
use font8x8::{UnicodeFonts, BASIC_FONTS};
use std::collections::{hash_map::DefaultHasher, HashMap};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{self, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CustomCursor, Fullscreen, Icon, Window, WindowAttributes, WindowId};
use zeroize::Zeroize;

const FRAME_WIDTH: u32 = 960;
const FRAME_HEIGHT: u32 = 540;
const STATUS_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
const STATUS_QUEUE_CAPACITY: usize = 64;
const FINAL_STATUS_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_FRAME_DIMENSION: u32 = 16_384;
const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;
const MAX_CURSOR_DIMENSION: u32 = 512;
const MAX_CURSOR_BYTES: usize = 1024 * 1024;
const MAX_CACHED_CURSORS: usize = 256;
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
const MAX_AUDIO_FRAME_BYTES: usize = 64 * 1024;
const CLIPBOARD_POLL_INTERVAL: Duration = Duration::from_millis(500);
const UI_TICK_INTERVAL: Duration = Duration::from_millis(250);
const TOOLBAR_HIDE_DELAY: Duration = Duration::from_secs(3);
const TOOLBAR_HEIGHT: i32 = 42;
const TOOLBAR_BUTTON_WIDTH: i32 = 46;
const TOOLBAR_HANDLE_WIDTH: i32 = 128;
const TOOLBAR_HANDLE_HEIGHT: i32 = 20;
const TOOLTIP_HEIGHT: i32 = 24;

static STATUS_WRITER: OnceLock<StatusWriter> = OnceLock::new();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    install_process_diagnostics("viewer");
    set_current_process_app_user_model_id(WindowsAppUserModelId::Viewer);
    let (request, control_reader) = read_bootstrap()?;
    request.validate()?;
    start_status_writer()?;
    emit_status(&ViewerStatus::Starting);

    let event_loop = EventLoop::<ViewerEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    start_launcher_command_reader(control_reader, proxy.clone())?;
    let frame_mailbox = Arc::new(FrameMailbox::default());
    let viewer_visible = Arc::new(AtomicBool::new(true));
    let session_connected = Arc::new(AtomicBool::new(false));
    start_frame_watchdog(
        proxy.clone(),
        Arc::clone(&frame_mailbox),
        Arc::clone(&viewer_visible),
        Arc::clone(&session_connected),
    )?;
    let remote_id = request.remote_id.clone();
    let session = SessionControl::start(request, proxy.clone(), Arc::clone(&frame_mailbox))?;
    let clipboard_enabled = Arc::new(AtomicBool::new(session.allow_clipboard));
    let clipboard_watcher_stop = Arc::new(AtomicBool::new(false));
    start_local_clipboard_watcher(
        proxy.clone(),
        Arc::clone(&clipboard_enabled),
        Arc::clone(&session_connected),
        Arc::clone(&clipboard_watcher_stop),
    )?;
    start_ui_tick(proxy.clone(), Arc::clone(&clipboard_watcher_stop))?;
    start_status_heartbeat(proxy, Arc::clone(&clipboard_watcher_stop))?;
    let mut app = Viewer::new(
        remote_id,
        frame_mailbox,
        session,
        viewer_visible,
        session_connected,
        clipboard_enabled,
        clipboard_watcher_stop,
    );
    event_loop.run_app(&mut app)?;
    app.clipboard_watcher_stop.store(true, Ordering::Release);
    emit_final_status(&ViewerStatus::SessionSummary {
        remote_id: app.remote_id.trim().to_owned(),
        session_seconds: app
            .session_started
            .map(|started| started.elapsed().as_secs())
            .unwrap_or(0),
        reconnect_count: app.reconnect_count,
        end_reason: app.session_end_reason.clone(),
    });
    emit_final_status(&ViewerStatus::Closed);
    let _ = flush_status_writer(FINAL_STATUS_TIMEOUT);
    Ok(())
}

fn read_bootstrap() -> Result<(ViewerBootstrap, BufReader<io::Stdin>), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() != Some("--bootstrap-stdin") {
        return Err("viewer must be launched with --bootstrap-stdin".into());
    }

    let mut reader = BufReader::new(io::stdin());
    let encoded = read_bounded_line(&mut reader, MAX_IPC_LINE_BYTES)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "viewer bootstrap missing"))?;
    Ok((serde_json::from_str(&encoded)?, reader))
}

fn start_launcher_command_reader(
    reader: BufReader<io::Stdin>,
    proxy: winit::event_loop::EventLoopProxy<ViewerEvent>,
) -> io::Result<()> {
    thread::Builder::new()
        .name("evertydesk-launcher-commands".to_owned())
        .spawn(move || {
            let mut reader = reader;
            loop {
                let line = match read_bounded_line(&mut reader, MAX_IPC_LINE_BYTES) {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(error) => {
                        emit_status(&ViewerStatus::Failed {
                            error: format!("IPC launcher → viewer: {error}"),
                        });
                        break;
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<ViewerCommand>(&line) {
                    Ok(command) => {
                        if proxy
                            .send_event(ViewerEvent::LauncherCommand(command))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        emit_status(&ViewerStatus::Failed {
                            error: format!("Некорректная IPC-команда launcher → viewer: {error}"),
                        });
                        break;
                    }
                }
            }
            let _ = proxy.send_event(ViewerEvent::LauncherCommand(ViewerCommand::Disconnect));
        })?;
    Ok(())
}

enum StatusOutput {
    Line(Vec<u8>),
    Flush(mpsc::Sender<io::Result<()>>),
}

struct StatusWriter {
    commands: SyncSender<StatusOutput>,
    open: Arc<AtomicBool>,
}

fn start_status_writer() -> io::Result<()> {
    let (commands, receiver) = mpsc::sync_channel::<StatusOutput>(STATUS_QUEUE_CAPACITY);
    let open = Arc::new(AtomicBool::new(true));
    let writer_open = Arc::clone(&open);
    thread::Builder::new()
        .name("evertydesk-status-writer".to_owned())
        .spawn(move || {
            let stdout = io::stdout();
            let mut stdout = stdout.lock();
            while let Ok(output) = receiver.recv() {
                let failed = match output {
                    StatusOutput::Line(encoded) => stdout
                        .write_all(&encoded)
                        .and_then(|()| stdout.write_all(b"\n"))
                        .and_then(|()| stdout.flush())
                        .is_err(),
                    StatusOutput::Flush(completed) => {
                        let result = stdout.flush();
                        let failed = result.is_err();
                        let _ = completed.send(result);
                        failed
                    }
                };
                if failed {
                    break;
                }
            }
            writer_open.store(false, Ordering::Release);
        })?;
    STATUS_WRITER
        .set(StatusWriter { commands, open })
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "status writer already started",
            )
        })
}

fn emit_status(status: &ViewerStatus) {
    let encoded = encode_status_or_failure(status);
    if let Some(writer) = STATUS_WRITER.get() {
        let _ = enqueue_status_output(writer, StatusOutput::Line(encoded), None);
    }
}

fn emit_final_status(status: &ViewerStatus) {
    let encoded = encode_status_or_failure(status);
    if let Some(writer) = STATUS_WRITER.get() {
        let _ = enqueue_status_output(
            writer,
            StatusOutput::Line(encoded),
            Some(FINAL_STATUS_TIMEOUT),
        );
    }
}

fn encode_status_or_failure(status: &ViewerStatus) -> Vec<u8> {
    let encoded = encode_status(status).unwrap_or_else(|error| {
        serde_json::to_vec(&ViewerStatus::Failed {
            error: format!("Не удалось отправить IPC-статус: {error}"),
        })
        .unwrap_or_else(|_| {
            b"{\"event\":\"failed\",\"error\":\"IPC serialization failed\"}".to_vec()
        })
    });
    encoded
}

fn enqueue_status_output(
    writer: &StatusWriter,
    mut output: StatusOutput,
    timeout: Option<Duration>,
) -> io::Result<()> {
    let deadline = timeout.map(|duration| Instant::now() + duration);
    loop {
        if !writer.open.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "viewer status channel is closed",
            ));
        }
        match writer.commands.try_send(output) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "viewer status writer is unavailable",
                ));
            }
            Err(TrySendError::Full(returned)) => {
                output = returned;
                if deadline.is_none_or(|deadline| Instant::now() >= deadline) {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "viewer status queue is full",
                    ));
                }
                thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

fn flush_status_writer(timeout: Duration) -> io::Result<()> {
    let writer = STATUS_WRITER
        .get()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "status writer unavailable"))?;
    let (completed, completion) = mpsc::channel();
    enqueue_status_output(writer, StatusOutput::Flush(completed), Some(timeout))?;
    completion
        .recv_timeout(timeout)
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => {
                io::Error::new(io::ErrorKind::TimedOut, "status writer flush timed out")
            }
            mpsc::RecvTimeoutError::Disconnected => {
                io::Error::new(io::ErrorKind::BrokenPipe, "status writer stopped")
            }
        })?
}

fn encode_status(status: &ViewerStatus) -> io::Result<Vec<u8>> {
    let encoded = serde_json::to_vec(status).map_err(io::Error::other)?;
    if encoded.len() > MAX_IPC_LINE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "viewer status is {} bytes; limit is {MAX_IPC_LINE_BYTES}",
                encoded.len()
            ),
        ));
    }
    Ok(encoded)
}

fn start_frame_watchdog(
    proxy: winit::event_loop::EventLoopProxy<ViewerEvent>,
    frame_mailbox: Arc<FrameMailbox>,
    viewer_visible: Arc<AtomicBool>,
    session_connected: Arc<AtomicBool>,
) -> io::Result<()> {
    thread::Builder::new()
        .name("evertydesk-frame-watchdog".to_owned())
        .spawn(move || {
            let mut last_sequence = frame_mailbox.frame_sequence.load(Ordering::Acquire);
            let mut unchanged_intervals = 0_u8;
            loop {
                thread::sleep(Duration::from_secs(5));
                let sequence = frame_mailbox.frame_sequence.load(Ordering::Acquire);
                let watching = viewer_visible.load(Ordering::Acquire)
                    && session_connected.load(Ordering::Acquire);
                let (next_intervals, should_refresh) =
                    watchdog_step(watching, sequence == last_sequence, unchanged_intervals);
                unchanged_intervals = next_intervals;
                last_sequence = sequence;

                if should_refresh && proxy.send_event(ViewerEvent::WatchdogStalled).is_err() {
                    break;
                }
            }
        })?;
    Ok(())
}

fn start_local_clipboard_watcher(
    proxy: winit::event_loop::EventLoopProxy<ViewerEvent>,
    enabled: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) -> io::Result<()> {
    thread::Builder::new()
        .name("evertydesk-clipboard-watcher".to_owned())
        .spawn(move || {
            let mut last_observed = None;
            while !stop.load(Ordering::Acquire) {
                let active = enabled.load(Ordering::Acquire) && connected.load(Ordering::Acquire);
                if !active {
                    last_observed = None;
                    thread::sleep(CLIPBOARD_POLL_INTERVAL);
                    continue;
                }

                if let Ok(text) =
                    arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text())
                {
                    if clipboard_observation_changed(&mut last_observed, &text) {
                        let event = if text.len() <= MAX_CLIPBOARD_BYTES {
                            ViewerEvent::LocalClipboardText(text)
                        } else {
                            ViewerEvent::RejectedPayload {
                                reason: format!(
                                    "Локальный буфер обмена превышает лимит {}",
                                    format_byte_limit(MAX_CLIPBOARD_BYTES)
                                ),
                                refresh_video: false,
                            }
                        };
                        if proxy.send_event(event).is_err() {
                            break;
                        }
                    }
                }
                thread::sleep(CLIPBOARD_POLL_INTERVAL);
            }
        })?;
    Ok(())
}

fn start_status_heartbeat(
    proxy: winit::event_loop::EventLoopProxy<ViewerEvent>,
    stop: Arc<AtomicBool>,
) -> io::Result<()> {
    thread::Builder::new()
        .name("evertydesk-status-heartbeat".to_owned())
        .spawn(move || {
            let mut sequence = 0_u64;
            while !stop.load(Ordering::Acquire) {
                thread::sleep(STATUS_HEARTBEAT_INTERVAL);
                if stop.load(Ordering::Acquire) {
                    break;
                }
                sequence = sequence.wrapping_add(1);
                if proxy.send_event(ViewerEvent::Heartbeat(sequence)).is_err() {
                    break;
                }
            }
        })?;
    Ok(())
}

fn start_ui_tick(
    proxy: winit::event_loop::EventLoopProxy<ViewerEvent>,
    stop: Arc<AtomicBool>,
) -> io::Result<()> {
    thread::Builder::new()
        .name("evertydesk-viewer-ui-tick".to_owned())
        .spawn(move || {
            while !stop.load(Ordering::Acquire) {
                thread::sleep(UI_TICK_INTERVAL);
                if stop.load(Ordering::Acquire) || proxy.send_event(ViewerEvent::UiTick).is_err() {
                    break;
                }
            }
        })?;
    Ok(())
}

#[derive(Debug)]
enum ViewerEvent {
    FrameReady,
    UiTick,
    Heartbeat(u64),
    Status(String),
    Latency(u32),
    Codec(String),
    EvrtStatus {
        active: bool,
        endpoint: String,
    },
    EvrtMetrics {
        pressure: String,
        jitter_ms: u32,
        fps: u32,
        reassembly_drops: u64,
        queue_drops: u64,
    },
    Displays(Vec<RemoteDisplay>),
    CursorData {
        id: u64,
        hotx: i32,
        hoty: i32,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    CursorId(u64),
    ClipboardText(String),
    LocalClipboardText(String),
    LauncherCommand(ViewerCommand),
    Connected(String),
    Failed {
        generation: u64,
        error: String,
    },
    Closed {
        generation: u64,
    },
    Reconnect {
        generation: u64,
    },
    Performance {
        fps_times_100: u32,
        input_kbps: u64,
        dropped_frames: u64,
    },
    RejectedPayload {
        reason: String,
        refresh_video: bool,
    },
    WatchdogStalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolbarAction {
    Fullscreen,
    Display,
    Scaling,
    Quality,
    PauseVideo,
    Refresh,
    Reconnect,
    Audio,
    Input,
    Clipboard,
    Screenshot,
    CtrlAltDelete,
    Diagnostics,
    Disconnect,
}

const TOOLBAR_ACTIONS: [ToolbarAction; 14] = [
    ToolbarAction::Fullscreen,
    ToolbarAction::Display,
    ToolbarAction::Scaling,
    ToolbarAction::Quality,
    ToolbarAction::PauseVideo,
    ToolbarAction::Refresh,
    ToolbarAction::Reconnect,
    ToolbarAction::Audio,
    ToolbarAction::Input,
    ToolbarAction::Clipboard,
    ToolbarAction::Screenshot,
    ToolbarAction::CtrlAltDelete,
    ToolbarAction::Diagnostics,
    ToolbarAction::Disconnect,
];
const TOOLBAR_ACTION_COUNT: i32 = TOOLBAR_ACTIONS.len() as i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolbarChrome {
    Action(ToolbarAction),
    Handle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewerShortcut {
    ToggleFullscreen,
    PreviousDisplay,
    NextDisplay,
    NextScaling,
    NextQuality,
    ToggleVideoPause,
    Reconnect,
    ToggleAudio,
    ToggleInput,
    ToggleClipboard,
    ToggleToolbar,
    Screenshot,
    ToggleDiagnostics,
    RefreshVideo,
    CtrlAltDelete,
}

#[derive(Default)]
struct ViewerDiagnostics {
    codec: String,
    fps_times_100: u32,
    input_kbps: u64,
    dropped_frames: u64,
    latency_ms: Option<u32>,
    evrt_active: bool,
    evrt_endpoint: String,
    evrt_pressure: String,
    evrt_jitter_ms: Option<u32>,
    evrt_fps: Option<u32>,
    evrt_reassembly_drops: u64,
    evrt_queue_drops: u64,
    last_performance_at: Option<Instant>,
}

#[derive(Default)]
struct FrameMailbox {
    latest: Mutex<Option<RemoteFrame>>,
    wake_pending: AtomicBool,
    frame_sequence: AtomicU64,
}

struct RemoteFrame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

struct SessionControl {
    commands: mpsc::Sender<SessionCommand>,
    stop: Arc<AtomicBool>,
    allow_clipboard: bool,
    request: ConnectionRequest,
    proxy: winit::event_loop::EventLoopProxy<ViewerEvent>,
    frame_mailbox: Arc<FrameMailbox>,
    generation: u64,
    video_visible: bool,
    video_paused: bool,
    scaling: ViewerScaling,
    quality: ConnectionQuality,
    audio_enabled: Arc<AtomicBool>,
    request_evrt2_experiment: bool,
    transport_profile_label: String,
}

impl SessionControl {
    fn start(
        mut bootstrap: ViewerBootstrap,
        proxy: winit::event_loop::EventLoopProxy<ViewerEvent>,
        frame_mailbox: Arc<FrameMailbox>,
    ) -> io::Result<Self> {
        let mut config = AppConfig::load_or_create();
        apply_quality_profile(&mut config.display, bootstrap.quality);
        apply_transport_profile(
            &mut config.display,
            bootstrap.game_mode,
            bootstrap.game_codec,
        );
        if bootstrap.game_mode {
            emit_status(&ViewerStatus::Progress {
                percent: 8,
                message: format!(
                    "Game profile: {}{}",
                    bootstrap.game_codec.label(),
                    if bootstrap.game_evrt2_enabled {
                        " + EVRT2"
                    } else {
                        ""
                    }
                ),
            });
        }
        let scaling = bootstrap.scaling;
        let quality = bootstrap.quality;
        let request_evrt2_experiment =
            should_request_evrt2_experiment(bootstrap.game_mode, bootstrap.game_evrt2_enabled);
        let transport_profile_label =
            viewer_transport_profile_label(bootstrap.game_mode, bootstrap.game_codec);
        let allow_clipboard = config.security.allow_clipboard;
        let audio_enabled = Arc::new(AtomicBool::new(bootstrap.audio_enabled));
        let request = ConnectionRequest {
            remote_id: bootstrap.remote_id.trim().to_owned(),
            password: std::mem::take(&mut bootstrap.password),
            client_id: config.local_id.clone(),
            client_name: local_device_name(),
            server: config.server,
            display: config.display,
            control_only: false,
            audio_enabled: Arc::clone(&audio_enabled),
            evrt2_only: false,
        };

        let generation = 1;
        let (commands, stop) = spawn_session(
            request.clone(),
            proxy.clone(),
            Arc::clone(&frame_mailbox),
            generation,
            request_evrt2_experiment,
        )?;

        Ok(Self {
            commands,
            stop,
            allow_clipboard,
            request,
            proxy,
            frame_mailbox,
            generation,
            video_visible: true,
            video_paused: false,
            scaling,
            quality,
            audio_enabled,
            request_evrt2_experiment,
            transport_profile_label,
        })
    }

    fn restart(&mut self) -> io::Result<()> {
        self.stop.store(true, Ordering::Release);
        let _ = self.commands.send(SessionCommand::Close);
        self.generation = self.generation.wrapping_add(1);
        if let Ok(mut latest) = self.frame_mailbox.latest.lock() {
            *latest = None;
        }
        self.frame_mailbox
            .wake_pending
            .store(false, Ordering::Release);
        let mut request = self.request.clone();
        if self.video_paused || !self.video_visible {
            let fps = desired_video_fps(
                self.video_paused,
                self.video_visible,
                request.display.target_fps,
            );
            request.display.target_fps = fps as u32;
            request.display.min_fps = fps as u32;
        }
        let (commands, stop) = spawn_session(
            request,
            self.proxy.clone(),
            Arc::clone(&self.frame_mailbox),
            self.generation,
            self.request_evrt2_experiment,
        )?;
        self.commands = commands;
        self.stop = stop;
        Ok(())
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn set_quality(&mut self, quality: ConnectionQuality) {
        self.quality = quality;
        apply_quality_profile(&mut self.request.display, quality);
        self.send(SessionCommand::SetAdaptiveQuality {
            enabled: self.request.display.adaptive_quality,
        });
        self.apply_video_fps();
    }

    fn set_video_visible(&mut self, visible: bool) {
        if self.video_visible == visible {
            return;
        }
        self.video_visible = visible;
        self.apply_video_fps();
        if visible && !self.video_paused {
            self.send(SessionCommand::RefreshVideo);
        }
    }

    fn set_video_paused(&mut self, paused: bool) {
        if self.video_paused == paused {
            return;
        }
        self.video_paused = paused;
        self.apply_video_fps();
        if !paused && self.video_visible {
            self.send(SessionCommand::RefreshVideo);
        }
    }

    fn apply_video_fps(&self) {
        let fps = desired_video_fps(
            self.video_paused,
            self.video_visible,
            self.request.display.target_fps,
        );
        self.send(SessionCommand::SetVideoFps { fps });
    }

    fn send(&self, command: SessionCommand) {
        let _ = self.commands.send(command);
    }
}

fn spawn_session(
    request: ConnectionRequest,
    proxy: winit::event_loop::EventLoopProxy<ViewerEvent>,
    frame_mailbox: Arc<FrameMailbox>,
    generation: u64,
    request_evrt2_experiment: bool,
) -> io::Result<(mpsc::Sender<SessionCommand>, Arc<AtomicBool>)> {
    let (commands, command_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let session_stop = Arc::clone(&stop);
    let audio_enabled = Arc::clone(&request.audio_enabled);
    let event_commands = commands.clone();

    thread::Builder::new()
        .name(format!("evertydesk-session-{generation}"))
        .spawn(move || {
            TransportClient::run_session(request, command_rx, event_tx, session_stop);
        })?;

    thread::Builder::new()
        .name(format!("evertydesk-viewer-events-{generation}"))
        .spawn(move || {
            let mut dropped_frames = 0_u64;
            let mut last_codec = String::new();
            let mut audio_player = evertydesk_core::evrt_audio::AudioPlayer::new();
            let mut audio_was_enabled = true;
            while let Ok(event) = event_rx.recv() {
                let audio_is_enabled = audio_enabled.load(Ordering::Acquire);
                if audio_was_enabled && !audio_is_enabled {
                    audio_player.clear_buffer();
                }
                audio_was_enabled = audio_is_enabled;
                if audio_is_enabled {
                    audio_player.tick();
                }
                match event {
                    SessionEvent::Frame {
                        sid: _,
                        codec,
                        width,
                        height,
                        rgba,
                    } => {
                        if last_codec != codec {
                            last_codec.clone_from(&codec);
                            emit_status(&ViewerStatus::Codec {
                                name: codec.clone(),
                            });
                            let _ = proxy.send_event(ViewerEvent::Codec(codec));
                        }
                        let Ok(width) = u32::try_from(width) else {
                            continue;
                        };
                        let Ok(height) = u32::try_from(height) else {
                            continue;
                        };
                        if width == 0 || height == 0 {
                            continue;
                        }
                        if let Err(reason) = validate_rgba_payload(
                            width,
                            height,
                            rgba.len(),
                            MAX_FRAME_DIMENSION,
                            MAX_FRAME_BYTES,
                        ) {
                            let _ = proxy.send_event(ViewerEvent::RejectedPayload {
                                reason: format!("Отклонён удалённый кадр: {reason}"),
                                refresh_video: true,
                            });
                            continue;
                        }

                        if let Ok(mut latest) = frame_mailbox.latest.lock() {
                            *latest = Some(RemoteFrame {
                                width,
                                height,
                                rgba,
                            });
                        }
                        frame_mailbox.frame_sequence.fetch_add(1, Ordering::Release);
                        if !frame_mailbox.wake_pending.swap(true, Ordering::AcqRel) {
                            let _ = proxy.send_event(ViewerEvent::FrameReady);
                        }
                    }
                    SessionEvent::Progress(percent, message) => {
                        emit_status(&ViewerStatus::Progress {
                            percent,
                            message: message.clone(),
                        });
                        let _ = proxy
                            .send_event(ViewerEvent::Status(format!("{percent}% — {message}")));
                    }
                    SessionEvent::Info(message) => {
                        emit_status(&ViewerStatus::Info {
                            message: message.clone(),
                        });
                        let _ = proxy.send_event(ViewerEvent::Status(message));
                    }
                    SessionEvent::Connected(peer) => {
                        emit_status(&ViewerStatus::Connected { peer: peer.clone() });
                        let _ = proxy.send_event(ViewerEvent::Connected(peer));
                        if request_evrt2_experiment {
                            emit_status(&ViewerStatus::Progress {
                                percent: 96,
                                message: "EVRT2 experiment requested over Game session".to_owned(),
                            });
                            let _ = proxy.send_event(ViewerEvent::Status(
                                "Game EVRT2: запрошен экспериментальный поток".to_owned(),
                            ));
                            let _ = event_commands.send(SessionCommand::StartEvrt2Experiment);
                        }
                    }
                    SessionEvent::Latency(milliseconds) => {
                        emit_status(&ViewerStatus::Latency { milliseconds });
                        let _ = proxy.send_event(ViewerEvent::Latency(milliseconds));
                        let _ = proxy.send_event(ViewerEvent::Status(format!(
                            "Подключено — {milliseconds} мс"
                        )));
                    }
                    SessionEvent::EvrtStatus {
                        active,
                        host_addr,
                        port,
                    } => {
                        let endpoint = format!("{host_addr}:{port}");
                        let status = if active {
                            format!("EVRT UDP active - {endpoint}")
                        } else {
                            "EVRT UDP stopped - TCP fallback".to_owned()
                        };
                        let _ = proxy.send_event(ViewerEvent::EvrtStatus { active, endpoint });
                        let _ = proxy.send_event(ViewerEvent::Status(status));
                    }
                    SessionEvent::EvrtMetrics {
                        pressure,
                        jitter_ms,
                        fps,
                        reassembly_drops,
                        queue_drops,
                        ..
                    } => {
                        let _ = proxy.send_event(ViewerEvent::EvrtMetrics {
                            pressure,
                            jitter_ms,
                            fps,
                            reassembly_drops,
                            queue_drops,
                        });
                    }
                    SessionEvent::FrameMetrics { dropped, .. } => {
                        dropped_frames = dropped_frames
                            .saturating_add(u64::try_from(dropped).unwrap_or(u64::MAX));
                    }
                    SessionEvent::VideoPacketMetrics {
                        input_fps,
                        input_kbps,
                    } => {
                        let _ = proxy.send_event(ViewerEvent::Performance {
                            fps_times_100: fps_times_100(input_fps),
                            input_kbps,
                            dropped_frames,
                        });
                    }
                    SessionEvent::Displays(displays) => {
                        let _ = proxy.send_event(ViewerEvent::Displays(displays));
                    }
                    SessionEvent::CursorData {
                        id,
                        hotx,
                        hoty,
                        width,
                        height,
                        rgba,
                    } => {
                        if let Err(reason) = validate_rgba_payload(
                            width,
                            height,
                            rgba.len(),
                            MAX_CURSOR_DIMENSION,
                            MAX_CURSOR_BYTES,
                        ) {
                            let _ = proxy.send_event(ViewerEvent::RejectedPayload {
                                reason: format!("Отклонён удалённый курсор: {reason}"),
                                refresh_video: false,
                            });
                            continue;
                        }
                        let _ = proxy.send_event(ViewerEvent::CursorData {
                            id,
                            hotx,
                            hoty,
                            width,
                            height,
                            rgba,
                        });
                    }
                    SessionEvent::CursorId { id } => {
                        let _ = proxy.send_event(ViewerEvent::CursorId(id));
                    }
                    SessionEvent::ClipboardText(text) => {
                        if text.len() > MAX_CLIPBOARD_BYTES {
                            let _ = proxy.send_event(ViewerEvent::RejectedPayload {
                                reason: format!(
                                    "Удалённый буфер обмена превышает лимит {}",
                                    format_byte_limit(MAX_CLIPBOARD_BYTES)
                                ),
                                refresh_video: false,
                            });
                            continue;
                        }
                        let _ = proxy.send_event(ViewerEvent::ClipboardText(text));
                    }
                    SessionEvent::AudioFrame(pcm) => {
                        if audio_frame_is_audible(audio_enabled.as_ref(), &pcm) {
                            audio_player.play(&pcm);
                        }
                    }
                    SessionEvent::Failed(error) => {
                        emit_status(&ViewerStatus::Failed {
                            error: error.clone(),
                        });
                        let _ = proxy.send_event(ViewerEvent::Failed { generation, error });
                        break;
                    }
                    SessionEvent::Closed => {
                        let _ = proxy.send_event(ViewerEvent::Closed { generation });
                        break;
                    }
                    _ => {}
                }
            }
        })?;

    Ok((commands, stop))
}

fn apply_quality_profile(display: &mut DisplayConfig, quality: ConnectionQuality) {
    let (target_fps, min_fps, adaptive_quality) = match quality {
        ConnectionQuality::Smooth => (60, 30, true),
        ConnectionQuality::Balanced => (45, 20, true),
        ConnectionQuality::Sharp => (30, 15, false),
    };
    display.target_fps = target_fps;
    display.min_fps = min_fps;
    display.adaptive_quality = adaptive_quality;
}

fn apply_transport_profile(display: &mut DisplayConfig, game_mode: bool, codec: ViewerGameCodec) {
    if game_mode {
        display.streaming_mode = StreamingMode::Game;
        display.target_fps = 60;
        display.min_fps = display.min_fps.max(30);
        display.adaptive_quality = false;
        display.codec = match codec {
            ViewerGameCodec::Auto => CodecPreference::H264,
            ViewerGameCodec::H265 => CodecPreference::H265,
            ViewerGameCodec::H264 => CodecPreference::H264,
            ViewerGameCodec::Av1 => CodecPreference::Av1,
        };
    } else {
        display.streaming_mode = StreamingMode::Support;
        display.codec = CodecPreference::Evrtck;
    }
}

fn should_request_evrt2_experiment(game_mode: bool, evrt2_enabled: bool) -> bool {
    game_mode && evrt2_enabled
}

fn viewer_transport_profile_label(game_mode: bool, codec: ViewerGameCodec) -> String {
    if game_mode {
        format!("Game {}", codec.label())
    } else {
        "Desktop EVRTCK".to_owned()
    }
}

fn desired_video_fps(paused: bool, visible: bool, target_fps: u32) -> i32 {
    if paused {
        1
    } else if visible {
        target_fps.max(1) as i32
    } else {
        5
    }
}

fn pixels_scaling_mode(scaling: ViewerScaling) -> ScalingMode {
    match scaling {
        ViewerScaling::SmoothFit => ScalingMode::Fill,
        ViewerScaling::PixelPerfect => ScalingMode::PixelPerfect,
    }
}

fn toolbar_hit_test(x: i32, y: i32, frame_width: i32) -> Option<ToolbarAction> {
    if !(0..TOOLBAR_HEIGHT).contains(&y) || !(0..frame_width).contains(&x) {
        return None;
    }
    let (start_x, button_width, total_width) = toolbar_geometry(frame_width);
    if x < start_x || x >= start_x + total_width {
        return None;
    }
    toolbar_action_at_index((x - start_x) / button_width)
}

fn toolbar_action_at_index(index: i32) -> Option<ToolbarAction> {
    usize::try_from(index)
        .ok()
        .and_then(|index| TOOLBAR_ACTIONS.get(index).copied())
}

fn toolbar_handle_hit_test(x: i32, y: i32, frame_width: i32, toolbar_visible: bool) -> bool {
    if !(0..frame_width).contains(&x) {
        return false;
    }
    let handle_x = ((frame_width - TOOLBAR_HANDLE_WIDTH) / 2).max(0);
    let handle_y = if toolbar_visible { TOOLBAR_HEIGHT } else { 0 };
    (handle_x..handle_x + TOOLBAR_HANDLE_WIDTH).contains(&x)
        && (handle_y..handle_y + TOOLBAR_HANDLE_HEIGHT).contains(&y)
}

fn toolbar_chrome_hit_test(
    x: i32,
    y: i32,
    frame_width: i32,
    toolbar_visible: bool,
) -> Option<ToolbarChrome> {
    if toolbar_handle_hit_test(x, y, frame_width, toolbar_visible) {
        return Some(ToolbarChrome::Handle);
    }
    if toolbar_visible {
        toolbar_hit_test(x, y, frame_width).map(ToolbarChrome::Action)
    } else {
        None
    }
}

fn toolbar_chrome_consumes_input(chrome: Option<ToolbarChrome>) -> bool {
    chrome.is_some()
}

fn toolbar_chrome_refreshes_activity(chrome: Option<ToolbarChrome>, toolbar_visible: bool) -> bool {
    toolbar_visible && chrome.is_some()
}

fn toolbar_overlay_top(toolbar_visible: bool, margin: i32) -> i32 {
    if toolbar_visible {
        TOOLBAR_HEIGHT + TOOLBAR_HANDLE_HEIGHT + margin
    } else {
        TOOLBAR_HANDLE_HEIGHT + margin
    }
}

fn toolbar_geometry(frame_width: i32) -> (i32, i32, i32) {
    let available = frame_width.max(TOOLBAR_ACTION_COUNT);
    let button_width = if available >= TOOLBAR_BUTTON_WIDTH * TOOLBAR_ACTION_COUNT {
        TOOLBAR_BUTTON_WIDTH
    } else {
        (available / TOOLBAR_ACTION_COUNT).max(1)
    };
    let total_width = (button_width * TOOLBAR_ACTION_COUNT).min(available);
    let start_x = (frame_width - total_width).max(0) / 2;
    (start_x, button_width, total_width)
}

#[allow(clippy::too_many_arguments)]
fn draw_toolbar(
    frame: &mut [u8],
    width: u32,
    height: u32,
    hovered: Option<ToolbarAction>,
    handle_hovered: bool,
    toolbar_visible: bool,
    input_enabled: bool,
    audio_enabled: bool,
    clipboard_enabled: bool,
    video_paused: bool,
    diagnostics_visible: bool,
    diagnostics: &ViewerDiagnostics,
    session_seconds: u64,
    reconnect_count: u32,
    scaling: ViewerScaling,
    quality: ConnectionQuality,
    transport_profile: &str,
    evrt2_experiment: bool,
    selected_display: Option<i32>,
    display_count: usize,
    connection_notice: Option<&str>,
) {
    let Ok(width) = i32::try_from(width) else {
        return;
    };
    let Ok(height) = i32::try_from(height) else {
        return;
    };
    let (start_x, button_width, total_width) = toolbar_geometry(width);
    if toolbar_visible {
        fill_rgba_rect(
            frame,
            width,
            height,
            start_x,
            0,
            total_width.min(width),
            TOOLBAR_HEIGHT.min(height),
            [26, 28, 32, 255],
        );

        for (index, action) in TOOLBAR_ACTIONS.into_iter().enumerate() {
            let x = start_x + i32::try_from(index).unwrap_or(0) * button_width;
            let background = if action == ToolbarAction::Disconnect {
                [204, 42, 50, 255]
            } else if hovered == Some(action) {
                [65, 68, 75, 255]
            } else {
                [36, 39, 44, 255]
            };
            fill_rgba_rect(
                frame,
                width,
                height,
                x + 1,
                1,
                button_width - 2,
                TOOLBAR_HEIGHT.min(height) - 2,
                background,
            );
            let icon = match action {
                ToolbarAction::Input if !input_enabled => [238, 75, 82, 255],
                ToolbarAction::Audio if !audio_enabled => [238, 75, 82, 255],
                ToolbarAction::Clipboard if !clipboard_enabled => [238, 75, 82, 255],
                ToolbarAction::PauseVideo if video_paused => [238, 75, 82, 255],
                ToolbarAction::Diagnostics if diagnostics_visible => [238, 75, 82, 255],
                _ => [238, 240, 244, 255],
            };
            draw_toolbar_icon(frame, width, height, x + button_width / 2, action, icon);
        }
        if let Some(action) = hovered {
            draw_toolbar_tooltip(frame, width, height, start_x, button_width, action, true);
        }
    }
    draw_toolbar_handle(frame, width, height, toolbar_visible, handle_hovered);
    if diagnostics_visible {
        draw_diagnostics_panel(
            frame,
            width,
            height,
            diagnostics,
            session_seconds,
            reconnect_count,
            scaling,
            quality,
            transport_profile,
            evrt2_experiment,
            width,
            height,
            selected_display,
            display_count,
            video_paused,
            audio_enabled,
            input_enabled,
            clipboard_enabled,
        );
    }
    if let Some(notice) = connection_notice {
        draw_connection_notice(frame, width, height, notice, toolbar_visible);
    }
}

fn draw_toolbar_handle(
    frame: &mut [u8],
    width: i32,
    height: i32,
    toolbar_visible: bool,
    hovered: bool,
) {
    let x = ((width - TOOLBAR_HANDLE_WIDTH) / 2).max(0);
    let y = if toolbar_visible { TOOLBAR_HEIGHT } else { 0 };
    let background = if hovered {
        [65, 68, 75, 245]
    } else {
        [26, 28, 32, 235]
    };
    fill_rgba_rect(
        frame,
        width,
        height,
        x,
        y,
        TOOLBAR_HANDLE_WIDTH.min(width),
        TOOLBAR_HANDLE_HEIGHT.min(height),
        background,
    );
    fill_rgba_rect(
        frame,
        width,
        height,
        x + 1,
        y,
        TOOLBAR_HANDLE_WIDTH.saturating_sub(2),
        1,
        [65, 68, 75, 255],
    );
    let center_x = x + 17;
    let center_y = y + TOOLBAR_HANDLE_HEIGHT / 2;
    let color = [238, 240, 244, 255];
    if toolbar_visible {
        draw_rgba_line(
            frame,
            width,
            height,
            center_x - 8,
            center_y + 3,
            center_x,
            center_y - 4,
            color,
        );
        draw_rgba_line(
            frame,
            width,
            height,
            center_x,
            center_y - 4,
            center_x + 8,
            center_y + 3,
            color,
        );
    } else {
        draw_rgba_line(
            frame,
            width,
            height,
            center_x - 8,
            center_y - 3,
            center_x,
            center_y + 4,
            color,
        );
        draw_rgba_line(
            frame,
            width,
            height,
            center_x,
            center_y + 4,
            center_x + 8,
            center_y - 3,
            color,
        );
    }
    let label = if toolbar_visible {
        "Hide controls"
    } else {
        "Show controls"
    };
    draw_ascii_text(
        frame,
        width,
        height,
        x + 34,
        y + 7,
        label,
        [238, 240, 244, 255],
        1,
    );
}

fn draw_toolbar_icon(
    frame: &mut [u8],
    width: i32,
    height: i32,
    center_x: i32,
    action: ToolbarAction,
    color: [u8; 4],
) {
    let center_y = TOOLBAR_HEIGHT / 2;
    match action {
        ToolbarAction::Fullscreen => {
            for (x, y) in [
                (center_x - 9, center_y - 8),
                (center_x + 5, center_y - 8),
                (center_x - 9, center_y + 5),
                (center_x + 5, center_y + 5),
            ] {
                fill_rgba_rect(frame, width, height, x, y, 4, 4, color);
            }
        }
        ToolbarAction::Display => {
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 9,
                center_y - 7,
                18,
                12,
                color,
            );
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 3,
                center_y + 6,
                6,
                3,
                color,
            );
        }
        ToolbarAction::Scaling => {
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 9,
                center_y - 1,
                18,
                3,
                color,
            );
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 1,
                center_y - 9,
                3,
                18,
                color,
            );
        }
        ToolbarAction::Quality => {
            draw_ascii_text(
                frame,
                width,
                height,
                center_x - 4,
                center_y - 4,
                "Q",
                color,
                1,
            );
        }
        ToolbarAction::PauseVideo => {
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 7,
                center_y - 8,
                5,
                16,
                color,
            );
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x + 2,
                center_y - 8,
                5,
                16,
                color,
            );
        }
        ToolbarAction::Refresh => {
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 8,
                center_y - 7,
                16,
                3,
                color,
            );
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x + 5,
                center_y - 7,
                3,
                9,
                color,
            );
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 8,
                center_y + 5,
                16,
                3,
                color,
            );
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 8,
                center_y - 1,
                3,
                9,
                color,
            );
        }
        ToolbarAction::Reconnect => {
            draw_ascii_text(
                frame,
                width,
                height,
                center_x - 4,
                center_y - 4,
                "R",
                color,
                1,
            );
        }
        ToolbarAction::Audio => {
            draw_ascii_text(
                frame,
                width,
                height,
                center_x - 4,
                center_y - 4,
                "A",
                color,
                1,
            );
        }
        ToolbarAction::Input => {
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 7,
                center_y - 8,
                4,
                16,
                color,
            );
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 3,
                center_y + 3,
                10,
                4,
                color,
            );
        }
        ToolbarAction::Clipboard => {
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 7,
                center_y - 7,
                14,
                16,
                color,
            );
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 3,
                center_y - 10,
                6,
                4,
                color,
            );
        }
        ToolbarAction::Screenshot => {
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 9,
                center_y - 6,
                18,
                13,
                color,
            );
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 3,
                center_y - 2,
                6,
                6,
                [36, 39, 44, 255],
            );
        }
        ToolbarAction::CtrlAltDelete => {
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 9,
                center_y - 7,
                18,
                14,
                color,
            );
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 5,
                center_y - 3,
                10,
                6,
                [36, 39, 44, 255],
            );
        }
        ToolbarAction::Diagnostics => {
            fill_rgba_rect(
                frame,
                width,
                height,
                center_x - 2,
                center_y - 7,
                4,
                4,
                color,
            );
            fill_rgba_rect(frame, width, height, center_x - 2, center_y, 4, 10, color);
        }
        ToolbarAction::Disconnect => {
            for offset in -7..=7 {
                fill_rgba_rect(
                    frame,
                    width,
                    height,
                    center_x + offset,
                    center_y + offset,
                    2,
                    2,
                    color,
                );
                fill_rgba_rect(
                    frame,
                    width,
                    height,
                    center_x + offset,
                    center_y - offset,
                    2,
                    2,
                    color,
                );
            }
        }
    }
}

fn toolbar_action_index(action: ToolbarAction) -> i32 {
    TOOLBAR_ACTIONS
        .iter()
        .position(|candidate| *candidate == action)
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(0)
}

fn toolbar_action_label(action: ToolbarAction) -> &'static str {
    match action {
        ToolbarAction::Fullscreen => "Fullscreen (Alt+Enter) - Hide controls: Ctrl+Alt+H",
        ToolbarAction::Display => "Monitor (Ctrl+Alt+Left/Right)",
        ToolbarAction::Scaling => "Scaling (Ctrl+Alt+M)",
        ToolbarAction::Quality => "Quality profile (Ctrl+Alt+Q)",
        ToolbarAction::PauseVideo => "Pause video (Ctrl+Alt+P)",
        ToolbarAction::Refresh => "Refresh video (Ctrl+Alt+R)",
        ToolbarAction::Reconnect => "Reconnect now (Ctrl+Alt+K)",
        ToolbarAction::Audio => "Remote audio (Ctrl+Alt+A)",
        ToolbarAction::Input => "Keyboard and mouse (Ctrl+Alt+I)",
        ToolbarAction::Clipboard => "Clipboard (Ctrl+Alt+C)",
        ToolbarAction::Screenshot => "Screenshot (Ctrl+Alt+S)",
        ToolbarAction::CtrlAltDelete => "Send Ctrl+Alt+Del (Ctrl+Alt+End)",
        ToolbarAction::Diagnostics => "Connection info (Ctrl+Alt+D)",
        ToolbarAction::Disconnect => "Disconnect",
    }
}

fn draw_toolbar_tooltip(
    frame: &mut [u8],
    width: i32,
    height: i32,
    toolbar_start_x: i32,
    button_width: i32,
    action: ToolbarAction,
    toolbar_visible: bool,
) {
    let label = toolbar_tooltip_text(toolbar_action_label(action), width);
    let tooltip_width = toolbar_text_width(&label) + 16;
    let button_center =
        toolbar_start_x + toolbar_action_index(action) * button_width + button_width / 2;
    let x = (button_center - tooltip_width / 2)
        .max(4)
        .min((width - tooltip_width - 4).max(4));
    let y = toolbar_overlay_top(toolbar_visible, 5);
    fill_rgba_rect(
        frame,
        width,
        height,
        x,
        y,
        tooltip_width,
        TOOLTIP_HEIGHT,
        [18, 20, 24, 242],
    );
    draw_ascii_text(
        frame,
        width,
        height,
        x + 8,
        y + 8,
        &label,
        [245, 246, 248, 255],
        1,
    );
}

fn toolbar_text_width(label: &str) -> i32 {
    i32::try_from(label.chars().count())
        .unwrap_or(0)
        .saturating_mul(8)
}

fn toolbar_tooltip_text(label: &str, frame_width: i32) -> String {
    let max_chars = ((frame_width.saturating_sub(24)) / 8).max(8) as usize;
    if label.chars().count() <= max_chars {
        return label.to_owned();
    }
    let mut clipped: String = label.chars().take(max_chars.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

fn draw_connection_notice(
    frame: &mut [u8],
    width: i32,
    height: i32,
    notice: &str,
    toolbar_visible: bool,
) {
    let text_width = i32::try_from(notice.chars().count())
        .unwrap_or_default()
        .saturating_mul(8);
    let banner_width = (text_width + 32).min(width.saturating_sub(8)).max(80);
    let x = (width - banner_width).max(0) / 2;
    let y = toolbar_overlay_top(toolbar_visible, 8);
    fill_rgba_rect(
        frame,
        width,
        height,
        x,
        y,
        banner_width,
        30,
        [18, 20, 24, 235],
    );
    fill_rgba_rect(frame, width, height, x, y, 4, 30, [238, 75, 82, 255]);
    draw_ascii_text(
        frame,
        width,
        height,
        x + 16,
        y + 11,
        notice,
        [245, 246, 248, 255],
        1,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_diagnostics_panel(
    frame: &mut [u8],
    width: i32,
    height: i32,
    diagnostics: &ViewerDiagnostics,
    session_seconds: u64,
    reconnect_count: u32,
    scaling: ViewerScaling,
    quality: ConnectionQuality,
    transport_profile: &str,
    evrt2_experiment: bool,
    frame_width: i32,
    frame_height: i32,
    selected_display: Option<i32>,
    display_count: usize,
    video_paused: bool,
    audio_enabled: bool,
    input_enabled: bool,
    clipboard_enabled: bool,
) {
    const PANEL_WIDTH: i32 = 272;
    const PANEL_HEIGHT: i32 = 454;
    let x = (width - PANEL_WIDTH - 14).max(4);
    let y = toolbar_overlay_top(true, 10);
    fill_rgba_rect(
        frame,
        width,
        height,
        x,
        y,
        PANEL_WIDTH.min(width - x),
        PANEL_HEIGHT.min(height - y),
        [18, 20, 24, 242],
    );
    fill_rgba_rect(
        frame,
        width,
        height,
        x,
        y,
        4,
        PANEL_HEIGHT.min(height - y),
        [232, 34, 42, 255],
    );

    let codec = if diagnostics.codec.is_empty() {
        "Waiting"
    } else {
        diagnostics.codec.as_str()
    };
    let latency = diagnostics
        .latency_ms
        .map_or_else(|| "--".to_owned(), |value| value.to_string());
    let telemetry_age = diagnostics
        .last_performance_at
        .map(|updated| updated.elapsed())
        .map_or_else(|| "--".to_owned(), format_telemetry_age);
    let health = diagnostics_health(
        diagnostics.latency_ms,
        diagnostics.fps_times_100,
        diagnostics
            .last_performance_at
            .map(|updated| updated.elapsed()),
    );
    let transport = if diagnostics.evrt_active {
        "EVRT UDP"
    } else {
        "TCP fallback"
    };
    let evrt_fps = diagnostics
        .evrt_fps
        .map_or_else(|| "--".to_owned(), |value| value.to_string());
    let evrt_jitter = diagnostics
        .evrt_jitter_ms
        .map_or_else(|| "--".to_owned(), |value| value.to_string());
    let evrt_pressure = if diagnostics.evrt_pressure.is_empty() {
        "--"
    } else {
        diagnostics.evrt_pressure.as_str()
    };
    let lines = [
        "CONNECTION INFO".to_owned(),
        format!("Health      {health}"),
        format!("Codec       {codec}"),
        format!("Transport   {transport}"),
        format!("EVRT FPS    {evrt_fps}"),
        format!("EVRT jitter {evrt_jitter} ms"),
        format!("EVRT press  {evrt_pressure}"),
        format!(
            "EVRT drops  {}/{}",
            diagnostics.evrt_reassembly_drops, diagnostics.evrt_queue_drops
        ),
        format!(
            "FPS         {}.{:02}",
            diagnostics.fps_times_100 / 100,
            diagnostics.fps_times_100 % 100
        ),
        format!("Bitrate     {} kbps", diagnostics.input_kbps),
        format!("Latency     {latency} ms"),
        format!("Telemetry   {telemetry_age}"),
        format!("Dropped     {}", diagnostics.dropped_frames),
        format!("Session     {}", format_duration(session_seconds)),
        format!("Reconnects  {reconnect_count}"),
        format!("Resolution  {frame_width}x{frame_height}"),
        format!(
            "Monitor     {} / {}",
            selected_display.map_or_else(|| "--".to_owned(), |value| (value + 1).to_string()),
            display_count.max(1)
        ),
        format!(
            "Video       {}",
            if video_paused { "Paused" } else { "Active" }
        ),
        format!("Quality     {}", diagnostics_quality_label(quality)),
        format!("Scaling     {}", diagnostics_scaling_label(scaling)),
        format!("Profile     {transport_profile}"),
        format!("EVRT2       {}", enabled_label(evrt2_experiment)),
        format!("Audio       {}", enabled_label(audio_enabled)),
        format!("Input       {}", enabled_label(input_enabled)),
        format!("Clipboard   {}", enabled_label(clipboard_enabled)),
    ];
    for (index, line) in lines.iter().enumerate() {
        let color = if index == 0 {
            [238, 75, 82, 255]
        } else {
            [235, 237, 241, 255]
        };
        draw_ascii_text(
            frame,
            width,
            height,
            x + 16,
            y + 13 + i32::try_from(index).unwrap_or(0) * 19,
            line,
            color,
            1,
        );
    }
}

fn enabled_label(enabled: bool) -> &'static str {
    if enabled {
        "Enabled"
    } else {
        "Disabled"
    }
}

fn viewer_window_icon() -> Option<Icon> {
    Icon::from_rgba(
        include_bytes!("../../assets/viewer-logo-32.rgba").to_vec(),
        32,
        32,
    )
    .ok()
}

fn local_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|name| name.trim().chars().take(64).collect::<String>())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "EvertyDesk".to_owned())
}

fn diagnostics_scaling_label(scaling: ViewerScaling) -> &'static str {
    match scaling {
        ViewerScaling::SmoothFit => "Smooth fit",
        ViewerScaling::PixelPerfect => "Pixel perfect",
    }
}

fn diagnostics_quality_label(quality: ConnectionQuality) -> &'static str {
    match quality {
        ConnectionQuality::Smooth => "Smooth",
        ConnectionQuality::Balanced => "Balanced",
        ConnectionQuality::Sharp => "Sharp",
    }
}

fn resolved_display_index(displays: &[RemoteDisplay], previous: Option<i32>) -> Option<i32> {
    previous
        .filter(|selected| displays.iter().any(|display| display.index == *selected))
        .or_else(|| displays.first().map(|display| display.index))
}

fn map_frame_position_to_display(
    frame_x: i32,
    frame_y: i32,
    frame_size: (u32, u32),
    display: &RemoteDisplay,
) -> (i32, i32) {
    let frame_width = i64::from(frame_size.0.max(1));
    let frame_height = i64::from(frame_size.1.max(1));
    let display_width = i64::from(display.width.max(1));
    let display_height = i64::from(display.height.max(1));

    let local_x = i64::from(frame_x.max(0));
    let local_y = i64::from(frame_y.max(0));
    let scaled_x = (local_x * display_width / frame_width).clamp(0, display_width - 1);
    let scaled_y = (local_y * display_height / frame_height).clamp(0, display_height - 1);

    (
        i32::try_from(i64::from(display.x) + scaled_x).unwrap_or_else(|_| {
            if display.x.is_negative() {
                i32::MIN
            } else {
                i32::MAX
            }
        }),
        i32::try_from(i64::from(display.y) + scaled_y).unwrap_or_else(|_| {
            if display.y.is_negative() {
                i32::MIN
            } else {
                i32::MAX
            }
        }),
    )
}

fn diagnostics_health(
    latency_ms: Option<u32>,
    fps_times_100: u32,
    telemetry_age: Option<Duration>,
) -> &'static str {
    let Some(age) = telemetry_age else {
        return "Waiting";
    };
    if age > Duration::from_secs(5) {
        return "Stale";
    }
    if latency_ms.is_some_and(|latency| latency > 200) || fps_times_100 < 1_500 {
        return "Poor";
    }
    if latency_ms.is_some_and(|latency| latency > 100) || fps_times_100 < 3_000 {
        return "Fair";
    }
    "Good"
}

fn format_telemetry_age(age: Duration) -> String {
    if age < Duration::from_secs(1) {
        format!("{} ms", age.as_millis())
    } else {
        format!("{:.1} s", age.as_secs_f32())
    }
}

fn format_duration(seconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_ascii_text(
    frame: &mut [u8],
    width: i32,
    height: i32,
    x: i32,
    y: i32,
    text: &str,
    color: [u8; 4],
    scale: i32,
) {
    for (char_index, character) in text.chars().enumerate() {
        let Some(glyph) = BASIC_FONTS.get(character) else {
            continue;
        };
        let char_x = x + i32::try_from(char_index).unwrap_or(0) * 8 * scale;
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0_u8..8 {
                if bits & (1_u8 << column) != 0 {
                    fill_rgba_rect(
                        frame,
                        width,
                        height,
                        char_x + i32::from(column) * scale,
                        y + i32::try_from(row).unwrap_or(0) * scale,
                        scale,
                        scale,
                        color,
                    );
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_rgba_rect(
    frame: &mut [u8],
    width: i32,
    height: i32,
    x: i32,
    y: i32,
    rect_width: i32,
    rect_height: i32,
    color: [u8; 4],
) {
    let start_x = x.max(0).min(width);
    let start_y = y.max(0).min(height);
    let end_x = x.saturating_add(rect_width).max(0).min(width);
    let end_y = y.saturating_add(rect_height).max(0).min(height);
    for pixel_y in start_y..end_y {
        for pixel_x in start_x..end_x {
            let Some(index) = pixel_y
                .checked_mul(width)
                .and_then(|row| row.checked_add(pixel_x))
                .and_then(|pixel| pixel.checked_mul(4))
                .and_then(|index| usize::try_from(index).ok())
            else {
                continue;
            };
            if index + 4 <= frame.len() {
                frame[index..index + 4].copy_from_slice(&color);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_rgba_line(
    frame: &mut [u8],
    width: i32,
    height: i32,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    color: [u8; 4],
) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut error = dx + dy;

    loop {
        fill_rgba_rect(frame, width, height, x0, y0, 2, 2, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let double_error = error.saturating_mul(2);
        if double_error >= dy {
            error += dy;
            x0 += sx;
        }
        if double_error <= dx {
            error += dx;
            y0 += sy;
        }
    }
}

impl Drop for SessionControl {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.commands.send(SessionCommand::Close);
        self.request.password.zeroize();
    }
}

struct Viewer {
    remote_id: String,
    window: Option<Arc<Window>>,
    // Field kept as `pixels` (not the `pixels` crate) to minimize the diff against
    // the call sites below; `FrameRenderer` exposes the same method names.
    pixels: Option<FrameRenderer>,
    frame_mailbox: Arc<FrameMailbox>,
    viewer_visible: Arc<AtomicBool>,
    session_connected: Arc<AtomicBool>,
    clipboard_enabled: Arc<AtomicBool>,
    clipboard_watcher_stop: Arc<AtomicBool>,
    frame_size: (u32, u32),
    base_frame: Vec<u8>,
    has_frame: bool,
    session: SessionControl,
    cursor_position: Option<(i32, i32)>,
    toolbar_hover: Option<ToolbarAction>,
    toolbar_handle_hover: bool,
    toolbar_visible: bool,
    toolbar_collapsed_by_user: bool,
    toolbar_last_activity: Instant,
    diagnostics_visible: bool,
    connection_notice: Option<String>,
    notice_expires_at: Option<Instant>,
    diagnostics: ViewerDiagnostics,
    modifiers: ModifiersState,
    displays: Vec<RemoteDisplay>,
    selected_display: Option<i32>,
    cursors: HashMap<u64, CustomCursor>,
    pressed_mouse_buttons: HashMap<MouseButton, (i32, i32)>,
    focused: bool,
    input_enabled: bool,
    audio_enabled: bool,
    video_paused: bool,
    scaling: ViewerScaling,
    quality: ConnectionQuality,
    last_remote_clipboard: Option<String>,
    last_sent_clipboard: Option<String>,
    reconnect_attempts: u32,
    reconnect_count: u32,
    reconnect_scheduled_for: Option<u64>,
    session_started: Option<Instant>,
    session_end_reason: String,
}

impl Viewer {
    fn new(
        remote_id: String,
        frame_mailbox: Arc<FrameMailbox>,
        session: SessionControl,
        viewer_visible: Arc<AtomicBool>,
        session_connected: Arc<AtomicBool>,
        clipboard_enabled: Arc<AtomicBool>,
        clipboard_watcher_stop: Arc<AtomicBool>,
    ) -> Self {
        let scaling = session.scaling;
        let quality = session.quality;
        let audio_enabled = session.audio_enabled.load(Ordering::Acquire);
        Self {
            remote_id,
            window: None,
            pixels: None,
            frame_mailbox,
            viewer_visible,
            session_connected,
            clipboard_enabled,
            clipboard_watcher_stop,
            frame_size: (FRAME_WIDTH, FRAME_HEIGHT),
            base_frame: Vec::new(),
            has_frame: false,
            session,
            cursor_position: None,
            toolbar_hover: None,
            toolbar_handle_hover: false,
            toolbar_visible: true,
            toolbar_collapsed_by_user: false,
            toolbar_last_activity: Instant::now(),
            diagnostics_visible: false,
            connection_notice: Some("CONNECTING...".to_owned()),
            notice_expires_at: None,
            diagnostics: ViewerDiagnostics::default(),
            modifiers: ModifiersState::empty(),
            displays: Vec::new(),
            selected_display: None,
            cursors: HashMap::new(),
            pressed_mouse_buttons: HashMap::new(),
            focused: false,
            input_enabled: true,
            audio_enabled,
            video_paused: false,
            scaling,
            quality,
            last_remote_clipboard: None,
            last_sent_clipboard: None,
            reconnect_attempts: 0,
            reconnect_count: 0,
            reconnect_scheduled_for: None,
            session_started: None,
            session_end_reason: "Сессия завершена".to_owned(),
        }
    }

    fn render(&mut self) -> Result<(), FrameRendererError> {
        let Some(pixels) = self.pixels.as_mut() else {
            return Ok(());
        };
        pixels.render()
    }

    fn present_latest_frame(&mut self) {
        self.frame_mailbox
            .wake_pending
            .store(false, Ordering::Release);
        let frame = self
            .frame_mailbox
            .latest
            .lock()
            .ok()
            .and_then(|mut latest| latest.take());
        let Some(frame) = frame else {
            return;
        };

        let expected_len = usize::try_from(frame.width)
            .ok()
            .and_then(|width| {
                usize::try_from(frame.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(4));
        if expected_len != Some(frame.rgba.len()) {
            eprintln!(
                "[viewer] rejected malformed frame {}x{} with {} bytes",
                frame.width,
                frame.height,
                frame.rgba.len()
            );
            return;
        }
        if self
            .connection_notice
            .as_deref()
            .is_some_and(|notice| notice.starts_with("VIDEO STALLED"))
        {
            self.connection_notice = None;
            self.notice_expires_at = None;
        }

        let session_seconds = self.session_seconds();
        let Some(pixels) = self.pixels.as_mut() else {
            return;
        };
        if self.frame_size != (frame.width, frame.height) {
            if let Err(error) = pixels.resize_buffer(frame.width, frame.height) {
                eprintln!("[viewer] resize frame buffer failed: {error}");
                return;
            }
            self.frame_size = (frame.width, frame.height);
        }
        self.base_frame = frame.rgba;
        let target = pixels.frame_mut();
        target.copy_from_slice(&self.base_frame);
        draw_toolbar(
            target,
            frame.width,
            frame.height,
            self.toolbar_hover,
            self.toolbar_handle_hover,
            self.toolbar_visible,
            self.input_enabled,
            self.audio_enabled,
            self.session.allow_clipboard,
            self.video_paused,
            self.diagnostics_visible,
            &self.diagnostics,
            session_seconds,
            self.reconnect_count,
            self.scaling,
            self.quality,
            &self.session.transport_profile_label,
            self.session.request_evrt2_experiment,
            self.selected_display,
            self.displays.len(),
            self.connection_notice.as_deref(),
        );
        self.has_frame = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn remote_position(&self, physical_x: f64, physical_y: f64) -> Option<(i32, i32)> {
        let pixels = self.pixels.as_ref()?;
        let (x, y) = pixels
            .window_pos_to_pixel((physical_x as f32, physical_y as f32))
            .ok()?;
        Some((i32::try_from(x).ok()?, i32::try_from(y).ok()?))
    }

    fn remote_input_position(&self, frame_x: i32, frame_y: i32) -> (i32, i32) {
        let Some(display) = self.selected_display.and_then(|selected| {
            self.displays
                .iter()
                .find(|display| display.index == selected)
        }) else {
            return (frame_x, frame_y);
        };

        map_frame_position_to_display(frame_x, frame_y, self.frame_size, display)
    }

    fn update_modifiers(&mut self, next: ModifiersState) {
        for (key, was_down, now_down) in [
            (
                ControlKey::Control,
                self.modifiers.control_key(),
                next.control_key(),
            ),
            (ControlKey::Alt, self.modifiers.alt_key(), next.alt_key()),
            (
                ControlKey::Shift,
                self.modifiers.shift_key(),
                next.shift_key(),
            ),
            (
                ControlKey::Meta,
                self.modifiers.super_key(),
                next.super_key(),
            ),
        ] {
            if was_down != now_down {
                self.session.send(SessionCommand::KeyControlState {
                    key,
                    down: now_down,
                });
            }
        }
        self.modifiers = next;
    }

    fn send_keyboard_input(&mut self, event: winit::event::KeyEvent) {
        if !self.focused || event.state != ElementState::Pressed {
            return;
        }

        match event.logical_key {
            Key::Named(named) => {
                if let Some(shortcut) = named_viewer_shortcut(self.modifiers, named) {
                    self.release_remote_inputs();
                    self.activate_shortcut(shortcut);
                    return;
                }
                if !self.input_enabled {
                    return;
                }
                if let Some(key) = named_key_to_control_key(named) {
                    self.session.send(SessionCommand::KeyControl(key));
                }
            }
            Key::Character(text) if !text.is_empty() => {
                if let Some(shortcut) = character_viewer_shortcut(self.modifiers, &text) {
                    self.release_remote_inputs();
                    self.activate_shortcut(shortcut);
                    return;
                }
                if self.modifiers.control_key() && text.eq_ignore_ascii_case("v") {
                    self.sync_local_clipboard_to_remote();
                }
                if !self.input_enabled {
                    return;
                }
                let modifiers = active_control_modifiers(self.modifiers);
                if modifiers.is_empty() {
                    self.session.send(SessionCommand::KeyText(text.to_string()));
                } else {
                    self.session.send(SessionCommand::KeyTextWithModifiers {
                        text: text.to_string(),
                        modifiers,
                    });
                }
            }
            _ => {}
        }
    }

    fn activate_shortcut(&mut self, shortcut: ViewerShortcut) {
        match shortcut {
            ViewerShortcut::ToggleFullscreen => self.toggle_fullscreen(),
            ViewerShortcut::PreviousDisplay => self.cycle_display(-1),
            ViewerShortcut::NextDisplay => self.cycle_display(1),
            ViewerShortcut::NextScaling => {
                let scaling = self.scaling.next();
                self.set_scaling(scaling);
                emit_status(&ViewerStatus::ControlState {
                    control: ViewerControl::Scaling { scaling },
                });
            }
            ViewerShortcut::NextQuality => {
                self.cycle_quality();
            }
            ViewerShortcut::ToggleVideoPause => self.toggle_video_pause(),
            ViewerShortcut::Reconnect => {
                self.reconnect_attempts = 0;
                self.restart_session();
            }
            ViewerShortcut::ToggleAudio => self.toggle_audio(),
            ViewerShortcut::ToggleInput => {
                let enabled = !self.input_enabled;
                if !enabled {
                    self.release_remote_inputs();
                }
                self.input_enabled = enabled;
                emit_status(&ViewerStatus::ControlState {
                    control: ViewerControl::InputEnabled { enabled },
                });
                self.redraw_toolbar();
            }
            ViewerShortcut::ToggleToolbar => self.toggle_toolbar_collapsed(),
            ViewerShortcut::ToggleClipboard => {
                let enabled = !self.session.allow_clipboard;
                self.set_clipboard_enabled(enabled);
                emit_status(&ViewerStatus::ControlState {
                    control: ViewerControl::ClipboardEnabled { enabled },
                });
                self.redraw_toolbar();
            }
            ViewerShortcut::Screenshot => self.save_screenshot(),
            ViewerShortcut::ToggleDiagnostics => {
                self.diagnostics_visible = !self.diagnostics_visible;
                self.show_toolbar();
                self.redraw_toolbar();
            }
            ViewerShortcut::RefreshVideo => {
                self.set_transient_notice("REQUESTING FRESH VIDEO FRAME...");
                self.session.send(SessionCommand::RefreshVideo);
                self.redraw_toolbar();
            }
            ViewerShortcut::CtrlAltDelete => self.send_ctrl_alt_delete(),
        }
    }

    fn sync_local_clipboard_to_remote(&mut self) {
        if !self.session.allow_clipboard {
            return;
        }
        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
            Ok(text) if !text.is_empty() => {
                self.forward_local_clipboard(text);
            }
            Ok(_) => {}
            Err(error) => eprintln!("[viewer] read local clipboard failed: {error}"),
        }
    }

    fn forward_local_clipboard(&mut self, text: String) {
        if !self.session.allow_clipboard || text.is_empty() {
            return;
        }
        if text.len() > MAX_CLIPBOARD_BYTES {
            self.report_rejected_payload(
                format!(
                    "Локальный буфер обмена превышает лимит {}",
                    format_byte_limit(MAX_CLIPBOARD_BYTES)
                ),
                false,
            );
            return;
        }
        if !should_forward_clipboard(
            &text,
            self.last_sent_clipboard.as_deref(),
            self.last_remote_clipboard.as_deref(),
        ) {
            return;
        }
        self.last_sent_clipboard = Some(text.clone());
        self.session.send(SessionCommand::SetClipboardText(text));
    }

    fn receive_remote_clipboard(&mut self, text: String) {
        if !self.session.allow_clipboard {
            return;
        }
        if text.len() > MAX_CLIPBOARD_BYTES {
            self.report_rejected_payload(
                format!(
                    "Удалённый буфер обмена превышает лимит {}",
                    format_byte_limit(MAX_CLIPBOARD_BYTES)
                ),
                false,
            );
            return;
        }
        if self.last_remote_clipboard.as_deref() == Some(text.as_str()) {
            return;
        }
        if let Err(error) =
            arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text.clone()))
        {
            eprintln!("[viewer] write local clipboard failed: {error}");
        } else {
            self.last_remote_clipboard = Some(text);
        }
    }

    fn report_rejected_payload(&self, reason: String, refresh_video: bool) {
        if refresh_video {
            self.session.send(SessionCommand::RefreshVideo);
        }
        emit_status(&ViewerStatus::Recovery {
            reason: reason.clone(),
        });
        if let Some(window) = &self.window {
            window.set_title(&format!("EvertyDesk Viewer — {reason}"));
        }
    }

    fn save_screenshot(&mut self) {
        match self.write_screenshot() {
            Ok(path) => {
                let path = path.to_string_lossy().into_owned();
                emit_status(&ViewerStatus::ScreenshotSaved { path: path.clone() });
                if let Some(window) = &self.window {
                    window.set_title(&format!("EvertyDesk Viewer — снимок сохранён: {path}"));
                }
            }
            Err(error) => {
                if let Some(window) = &self.window {
                    window.set_title(&format!("EvertyDesk Viewer — снимок не сохранён: {error}"));
                }
            }
        }
    }

    fn write_screenshot(&self) -> Result<PathBuf, String> {
        if !self.has_frame {
            return Err("кадр ещё не получен".to_owned());
        }
        let pixels = self
            .pixels
            .as_ref()
            .ok_or_else(|| "рендерер ещё не готов".to_owned())?;
        let directory = screenshot_directory().map_err(|error| error.to_string())?;
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let path = directory.join(format!(
            "EvertyDesk-{}-{timestamp}.png",
            safe_filename_component(&self.remote_id)
        ));
        let file = File::create(&path).map_err(|error| error.to_string())?;
        write_rgba_png(file, self.frame_size.0, self.frame_size.1, pixels.frame())
            .map_err(|error| error.to_string())?;
        Ok(path)
    }

    fn toggle_fullscreen(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        if window.fullscreen().is_some() {
            window.set_fullscreen(None);
            self.toolbar_visible = true;
            self.toolbar_collapsed_by_user = false;
        } else {
            window.set_fullscreen(Some(Fullscreen::Borderless(None)));
            self.show_toolbar();
        }
        self.redraw_toolbar();
    }

    fn show_toolbar(&mut self) {
        self.toolbar_visible = true;
        self.toolbar_collapsed_by_user = false;
        self.clear_toolbar_hover();
        self.toolbar_last_activity = Instant::now();
    }

    fn toggle_toolbar_collapsed(&mut self) {
        if self.toolbar_visible {
            self.toolbar_visible = false;
            self.toolbar_collapsed_by_user = true;
            self.clear_toolbar_hover();
            self.diagnostics_visible = false;
            self.set_transient_notice("CONTROLS HIDDEN - CTRL+ALT+H TO SHOW");
        } else {
            self.toolbar_visible = true;
            self.toolbar_collapsed_by_user = false;
            self.clear_toolbar_hover();
            self.toolbar_last_activity = Instant::now();
            self.set_transient_notice("CONTROLS SHOWN");
        }
        self.redraw_toolbar();
    }

    fn clear_toolbar_hover(&mut self) {
        self.toolbar_hover = None;
        self.toolbar_handle_hover = false;
    }

    fn update_toolbar_visibility(&mut self) {
        if self
            .notice_expires_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.connection_notice = None;
            self.notice_expires_at = None;
            self.redraw_toolbar();
        }
        let fullscreen = self
            .window
            .as_ref()
            .is_some_and(|window| window.fullscreen().is_some());
        if !fullscreen {
            if !self.toolbar_visible && !self.toolbar_collapsed_by_user {
                self.toolbar_visible = true;
                self.redraw_toolbar();
            }
            return;
        }
        if self.toolbar_visible
            && self.toolbar_hover.is_none()
            && !self.toolbar_handle_hover
            && !self.diagnostics_visible
            && !self.toolbar_collapsed_by_user
            && self.toolbar_last_activity.elapsed() >= TOOLBAR_HIDE_DELAY
        {
            self.toolbar_visible = false;
            self.redraw_toolbar();
        }
    }

    fn send_ctrl_alt_delete(&mut self) {
        if !self.input_enabled {
            self.set_transient_notice("INPUT IS DISABLED");
        } else {
            self.session
                .send(SessionCommand::KeyControl(ControlKey::CtrlAltDel));
            self.set_transient_notice("CTRL+ALT+DEL SENT");
        }
        self.redraw_toolbar();
    }

    fn set_transient_notice(&mut self, notice: &str) {
        self.connection_notice = Some(notice.to_owned());
        self.notice_expires_at = Some(Instant::now() + Duration::from_secs(2));
    }

    fn set_scaling(&mut self, scaling: ViewerScaling) {
        self.scaling = scaling;
        if let Some(pixels) = self.pixels.as_mut() {
            pixels.set_scaling_mode(pixels_scaling_mode(scaling));
        }
        if let Some(window) = &self.window {
            window.set_title(&format!(
                "EvertyDesk Viewer — {} — {}",
                self.remote_id.trim(),
                scaling.label()
            ));
            window.request_redraw();
        }
    }

    fn redraw_toolbar(&mut self) {
        let session_seconds = self.session_seconds();
        if let Some(pixels) = self.pixels.as_mut() {
            let target = pixels.frame_mut();
            if target.len() == self.base_frame.len() {
                target.copy_from_slice(&self.base_frame);
            }
            draw_toolbar(
                target,
                self.frame_size.0,
                self.frame_size.1,
                self.toolbar_hover,
                self.toolbar_handle_hover,
                self.toolbar_visible,
                self.input_enabled,
                self.audio_enabled,
                self.session.allow_clipboard,
                self.video_paused,
                self.diagnostics_visible,
                &self.diagnostics,
                session_seconds,
                self.reconnect_count,
                self.scaling,
                self.quality,
                &self.session.transport_profile_label,
                self.session.request_evrt2_experiment,
                self.selected_display,
                self.displays.len(),
                self.connection_notice.as_deref(),
            );
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn session_seconds(&self) -> u64 {
        self.session_started
            .map(|started| started.elapsed().as_secs())
            .unwrap_or(0)
    }

    fn set_clipboard_enabled(&mut self, enabled: bool) {
        self.session.allow_clipboard = enabled;
        self.clipboard_enabled.store(enabled, Ordering::Release);
        if !enabled {
            if let Some(text) = self.last_remote_clipboard.as_mut() {
                text.zeroize();
            }
            if let Some(text) = self.last_sent_clipboard.as_mut() {
                text.zeroize();
            }
            self.last_remote_clipboard = None;
            self.last_sent_clipboard = None;
        }
    }

    fn activate_toolbar_action(&mut self, action: ToolbarAction, event_loop: &ActiveEventLoop) {
        match action {
            ToolbarAction::Fullscreen => self.toggle_fullscreen(),
            ToolbarAction::Display => self.cycle_display(1),
            ToolbarAction::Scaling => {
                let scaling = self.scaling.next();
                self.set_scaling(scaling);
                emit_status(&ViewerStatus::ControlState {
                    control: ViewerControl::Scaling { scaling },
                });
            }
            ToolbarAction::Quality => self.cycle_quality(),
            ToolbarAction::PauseVideo => self.toggle_video_pause(),
            ToolbarAction::Refresh => {
                self.set_transient_notice("REQUESTING FRESH VIDEO FRAME...");
                self.session.send(SessionCommand::RefreshVideo);
            }
            ToolbarAction::Reconnect => {
                self.reconnect_attempts = 0;
                self.restart_session();
            }
            ToolbarAction::Audio => self.toggle_audio(),
            ToolbarAction::Input => {
                let enabled = !self.input_enabled;
                if !enabled {
                    self.release_remote_inputs();
                }
                self.input_enabled = enabled;
                emit_status(&ViewerStatus::ControlState {
                    control: ViewerControl::InputEnabled { enabled },
                });
            }
            ToolbarAction::Clipboard => {
                let enabled = !self.session.allow_clipboard;
                self.set_clipboard_enabled(enabled);
                emit_status(&ViewerStatus::ControlState {
                    control: ViewerControl::ClipboardEnabled { enabled },
                });
            }
            ToolbarAction::Screenshot => self.save_screenshot(),
            ToolbarAction::CtrlAltDelete => self.send_ctrl_alt_delete(),
            ToolbarAction::Diagnostics => {
                self.diagnostics_visible = !self.diagnostics_visible;
            }
            ToolbarAction::Disconnect => {
                self.release_remote_inputs();
                self.session_end_reason = "Отключено пользователем".to_owned();
                self.session.send(SessionCommand::Close);
                event_loop.exit();
            }
        }
        self.toolbar_last_activity = Instant::now();
        self.redraw_toolbar();
    }

    fn cycle_display(&mut self, direction: isize) {
        if self.displays.len() < 2 {
            return;
        }
        let current = self
            .selected_display
            .and_then(|selected| {
                self.displays
                    .iter()
                    .position(|display| display.index == selected)
            })
            .unwrap_or(0);
        let len = self.displays.len() as isize;
        let next = (current as isize + direction).rem_euclid(len) as usize;
        let display = self.displays[next].clone();
        self.selected_display = Some(display.index);
        self.apply_cursor_visibility(display.cursor_embedded);
        self.session.send(SessionCommand::SetDisplay(display));
    }

    fn cycle_quality(&mut self) {
        let quality = self.quality.next();
        self.quality = quality;
        self.session.set_quality(quality);
        self.set_transient_notice(match quality {
            ConnectionQuality::Smooth => "QUALITY: SMOOTH",
            ConnectionQuality::Balanced => "QUALITY: BALANCED",
            ConnectionQuality::Sharp => "QUALITY: SHARP",
        });
        emit_status(&ViewerStatus::ControlState {
            control: ViewerControl::Quality { quality },
        });
        self.redraw_toolbar();
    }

    fn toggle_video_pause(&mut self) {
        self.video_paused = !self.video_paused;
        self.session.set_video_paused(self.video_paused);
        self.set_transient_notice(if self.video_paused {
            "VIDEO PAUSED"
        } else {
            "VIDEO RESUMED"
        });
        self.redraw_toolbar();
    }

    fn toggle_audio(&mut self) {
        self.set_audio_enabled(!self.audio_enabled);
        emit_status(&ViewerStatus::ControlState {
            control: ViewerControl::AudioEnabled {
                enabled: self.audio_enabled,
            },
        });
    }

    fn set_audio_enabled(&mut self, enabled: bool) {
        self.audio_enabled = enabled;
        self.session
            .audio_enabled
            .store(self.audio_enabled, Ordering::Release);
        self.set_transient_notice(if self.audio_enabled {
            "REMOTE AUDIO ENABLED"
        } else {
            "REMOTE AUDIO MUTED"
        });
        self.redraw_toolbar();
    }

    fn emit_control_snapshot(&self) {
        for control in [
            ViewerControl::InputEnabled {
                enabled: self.input_enabled,
            },
            ViewerControl::AudioEnabled {
                enabled: self.audio_enabled,
            },
            ViewerControl::ClipboardEnabled {
                enabled: self.session.allow_clipboard,
            },
            ViewerControl::Quality {
                quality: self.quality,
            },
            ViewerControl::Scaling {
                scaling: self.scaling,
            },
        ] {
            emit_status(&ViewerStatus::ControlState { control });
        }
    }

    fn apply_cursor_visibility(&self, cursor_embedded: bool) {
        if let Some(window) = &self.window {
            window.set_cursor_visible(!cursor_embedded);
        }
    }

    fn apply_cursor_id(&mut self, id: u64) {
        if let (Some(window), Some(cursor)) = (&self.window, self.cursors.get(&id)) {
            window.set_cursor(cursor.clone());
        }
    }

    fn release_remote_inputs(&mut self) {
        self.update_modifiers(ModifiersState::empty());
        let fallback_position = self.cursor_position;
        for (button, pressed_at) in self.pressed_mouse_buttons.drain() {
            let (x, y) = fallback_position.unwrap_or(pressed_at);
            if let Some(command) = mouse_button_command(button, ElementState::Released, x, y) {
                self.session.send(command);
            }
        }
    }

    fn schedule_reconnect(&mut self) {
        let generation = self.session.generation();
        if !should_schedule_reconnect(self.reconnect_scheduled_for, generation) {
            return;
        }
        self.reconnect_scheduled_for = Some(generation);
        self.reconnect_attempts = self.reconnect_attempts.saturating_add(1);
        let delay_seconds = reconnect_delay_seconds(self.reconnect_attempts);
        self.connection_notice = Some(format!(
            "RECONNECTING IN {delay_seconds}S · ATTEMPT {}",
            self.reconnect_attempts
        ));
        self.notice_expires_at = None;
        self.redraw_toolbar();
        emit_status(&ViewerStatus::Reconnecting {
            attempt: self.reconnect_attempts,
            delay_seconds,
        });
        if let Some(window) = &self.window {
            window.set_title(&format!(
                "EvertyDesk Viewer — {} — переподключение через {delay_seconds} с",
                self.remote_id.trim()
            ));
        }

        let proxy = self.session.proxy.clone();
        let _ = thread::Builder::new()
            .name("evertydesk-reconnect-delay".to_owned())
            .spawn(move || {
                thread::sleep(std::time::Duration::from_secs(delay_seconds));
                let _ = proxy.send_event(ViewerEvent::Reconnect { generation });
            });
    }

    fn restart_session(&mut self) {
        self.reconnect_scheduled_for = None;
        self.release_remote_inputs();
        self.session_connected.store(false, Ordering::Release);
        self.reconnect_count = self.reconnect_count.saturating_add(1);
        self.displays.clear();
        self.cursors.clear();
        self.cursor_position = None;
        self.diagnostics = ViewerDiagnostics::default();
        self.pressed_mouse_buttons.clear();
        self.modifiers = ModifiersState::empty();
        self.last_sent_clipboard = None;
        self.connection_notice = Some("CONNECTING...".to_owned());
        self.notice_expires_at = None;
        self.redraw_toolbar();
        if let Err(error) = self.session.restart() {
            eprintln!("[viewer] restart session failed: {error}");
            self.schedule_reconnect();
        } else if let Some(window) = &self.window {
            emit_status(&ViewerStatus::Starting);
            window.set_title(&format!(
                "EvertyDesk Viewer — {} — подключение…",
                self.remote_id.trim()
            ));
        }
    }
}

impl ApplicationHandler<ViewerEvent> for Viewer {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let title = format!("EvertyDesk Viewer — {}", self.remote_id.trim());
        let attributes = WindowAttributes::default()
            .with_title(title)
            .with_window_icon(viewer_window_icon())
            .with_inner_size(LogicalSize::new(FRAME_WIDTH, FRAME_HEIGHT))
            .with_min_inner_size(LogicalSize::new(640, 360));

        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("[viewer] create window failed: {error}");
                event_loop.exit();
                return;
            }
        };
        let mut pixels = match FrameRenderer::new(Arc::clone(&window), FRAME_WIDTH, FRAME_HEIGHT) {
            Ok(pixels) => pixels,
            Err(error) => {
                eprintln!("[viewer] initialize wgpu surface failed: {error}");
                event_loop.exit();
                return;
            }
        };
        pixels.set_scaling_mode(pixels_scaling_mode(self.scaling));

        self.window = Some(window);
        self.pixels = Some(pixels);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(current_window_id) = self.window.as_ref().map(|window| window.id()) else {
            return;
        };
        if current_window_id != window_id {
            return;
        }

        match event {
            WindowEvent::CloseRequested => {
                self.release_remote_inputs();
                if self.session_end_reason == "Сессия завершена" {
                    self.session_end_reason = "Окно viewer закрыто пользователем".to_owned();
                }
                event_loop.exit();
            }
            WindowEvent::Focused(true) => {
                self.focused = true;
            }
            WindowEvent::Focused(false) => {
                self.focused = false;
                self.clear_toolbar_hover();
                self.release_remote_inputs();
                self.redraw_toolbar();
            }
            WindowEvent::Occluded(occluded) => {
                let visible = !occluded;
                self.viewer_visible.store(visible, Ordering::Release);
                self.session.set_video_visible(visible);
            }
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                if let Some(pixels) = self.pixels.as_mut() {
                    if let Err(error) = pixels.resize_surface(size.width, size.height) {
                        eprintln!("[viewer] resize surface failed: {error}");
                        self.session_end_reason =
                            safe_viewer_end_reason(&format!("Ошибка поверхности: {error}"));
                        event_loop.exit();
                    } else {
                        self.clear_toolbar_hover();
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some((frame_x, frame_y)) = self.remote_position(position.x, position.y) {
                    let frame_width = i32::try_from(self.frame_size.0).unwrap_or(i32::MAX);
                    let next_chrome = toolbar_chrome_hit_test(
                        frame_x,
                        frame_y,
                        frame_width,
                        self.toolbar_visible,
                    );
                    let next_toolbar = match next_chrome {
                        Some(ToolbarChrome::Action(action)) => Some(action),
                        _ => None,
                    };
                    let next_handle = matches!(next_chrome, Some(ToolbarChrome::Handle));
                    if next_toolbar != self.toolbar_hover
                        || next_handle != self.toolbar_handle_hover
                    {
                        if next_chrome.is_some()
                            && self.toolbar_hover.is_none()
                            && !self.toolbar_handle_hover
                        {
                            self.release_remote_inputs();
                        }
                        self.toolbar_hover = next_toolbar;
                        self.toolbar_handle_hover = next_handle;
                        self.redraw_toolbar();
                    }
                    if toolbar_chrome_refreshes_activity(next_chrome, self.toolbar_visible) {
                        self.toolbar_last_activity = Instant::now();
                    }
                    if toolbar_chrome_consumes_input(next_chrome) {
                        self.cursor_position = None;
                        return;
                    }
                    let (x, y) = self.remote_input_position(frame_x, frame_y);
                    self.cursor_position = Some((x, y));
                    if self.focused && self.input_enabled {
                        self.session.send(SessionCommand::MouseMove { x, y });
                    }
                } else {
                    let had_chrome_hover = self.toolbar_hover.take().is_some()
                        || std::mem::take(&mut self.toolbar_handle_hover);
                    if had_chrome_hover {
                        self.redraw_toolbar();
                    }
                    self.cursor_position = None;
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.toolbar_handle_hover {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        self.toggle_toolbar_collapsed();
                    }
                    return;
                }
                if let Some(action) = self.toolbar_hover {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        self.activate_toolbar_action(action, event_loop);
                    }
                    return;
                }
                if !self.focused || !self.input_enabled {
                    return;
                }
                let position = match (state, self.cursor_position) {
                    (ElementState::Pressed, Some(position)) => {
                        self.pressed_mouse_buttons.insert(button, position);
                        Some(position)
                    }
                    (ElementState::Released, Some(position)) => {
                        self.pressed_mouse_buttons.remove(&button);
                        Some(position)
                    }
                    (ElementState::Released, None) => self.pressed_mouse_buttons.remove(&button),
                    (ElementState::Pressed, None) => None,
                };
                let Some((x, y)) = position else {
                    return;
                };
                if let Some(command) = mouse_button_command(button, state, x, y) {
                    self.session.send(command);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.toolbar_hover.is_some() || self.toolbar_handle_hover {
                    return;
                }
                if !self.focused || !self.input_enabled {
                    return;
                }
                let (x, y) = normalized_wheel_delta(delta);
                if x != 0 || y != 0 {
                    self.session.send(SessionCommand::MouseWheel { x, y });
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                if self.focused && self.input_enabled {
                    self.update_modifiers(modifiers.state());
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.send_keyboard_input(event);
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.render() {
                    eprintln!("[viewer] render failed: {error}");
                    self.session_end_reason =
                        safe_viewer_end_reason(&format!("Ошибка отображения: {error}"));
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: ViewerEvent) {
        match event {
            ViewerEvent::FrameReady => self.present_latest_frame(),
            ViewerEvent::UiTick => self.update_toolbar_visibility(),
            ViewerEvent::Heartbeat(sequence) => {
                emit_status(&ViewerStatus::Heartbeat { sequence });
            }
            ViewerEvent::Status(status) => {
                if let Some(window) = &self.window {
                    window.set_title(&format!(
                        "EvertyDesk Viewer — {} — {status}",
                        self.remote_id.trim()
                    ));
                }
            }
            ViewerEvent::Latency(milliseconds) => {
                self.diagnostics.latency_ms = Some(milliseconds);
                if self.diagnostics_visible {
                    self.redraw_toolbar();
                }
            }
            ViewerEvent::Codec(codec) => {
                if self.diagnostics.codec != codec {
                    self.diagnostics.codec = codec;
                    if self.diagnostics_visible {
                        self.redraw_toolbar();
                    }
                }
            }
            ViewerEvent::EvrtStatus { active, endpoint } => {
                self.diagnostics.evrt_active = active;
                self.diagnostics.evrt_endpoint = endpoint;
                if !active {
                    self.diagnostics.evrt_pressure.clear();
                    self.diagnostics.evrt_jitter_ms = None;
                    self.diagnostics.evrt_fps = None;
                }
                if self.diagnostics_visible {
                    self.redraw_toolbar();
                }
            }
            ViewerEvent::EvrtMetrics {
                pressure,
                jitter_ms,
                fps,
                reassembly_drops,
                queue_drops,
            } => {
                self.diagnostics.evrt_pressure = pressure;
                self.diagnostics.evrt_jitter_ms = Some(jitter_ms);
                self.diagnostics.evrt_fps = Some(fps);
                self.diagnostics.evrt_reassembly_drops = reassembly_drops;
                self.diagnostics.evrt_queue_drops = queue_drops;
                if self.diagnostics_visible {
                    self.redraw_toolbar();
                }
            }
            ViewerEvent::Displays(displays) => {
                let previous_selection = self.selected_display;
                self.displays = displays;
                self.selected_display = resolved_display_index(&self.displays, previous_selection);
                if let Some(display) = self.selected_display.and_then(|selected| {
                    self.displays
                        .iter()
                        .find(|display| display.index == selected)
                        .cloned()
                }) {
                    self.apply_cursor_visibility(display.cursor_embedded);
                    if previous_selection == Some(display.index)
                        && self.displays.first().map(|candidate| candidate.index)
                            != Some(display.index)
                    {
                        self.session.send(SessionCommand::SetDisplay(display));
                    }
                }
            }
            ViewerEvent::CursorData {
                id,
                hotx,
                hoty,
                width,
                height,
                rgba,
            } => {
                let dimensions = (
                    u16::try_from(width),
                    u16::try_from(height),
                    u16::try_from(hotx),
                    u16::try_from(hoty),
                );
                if let (Ok(width), Ok(height), Ok(hotx), Ok(hoty)) = dimensions {
                    match CustomCursor::from_rgba(rgba, width, height, hotx, hoty) {
                        Ok(source) => {
                            if !self.cursors.contains_key(&id)
                                && self.cursors.len() >= MAX_CACHED_CURSORS
                            {
                                self.cursors.clear();
                            }
                            let cursor = event_loop.create_custom_cursor(source);
                            self.cursors.insert(id, cursor);
                            self.apply_cursor_id(id);
                        }
                        Err(error) => {
                            eprintln!("[viewer] rejected remote cursor {id}: {error}");
                        }
                    }
                }
            }
            ViewerEvent::CursorId(id) => self.apply_cursor_id(id),
            ViewerEvent::ClipboardText(text) => self.receive_remote_clipboard(text),
            ViewerEvent::LocalClipboardText(text) => self.forward_local_clipboard(text),
            ViewerEvent::LauncherCommand(ViewerCommand::Disconnect) => {
                self.release_remote_inputs();
                self.session_end_reason = "Отключено пользователем".to_owned();
                self.session.send(SessionCommand::Close);
                event_loop.exit();
            }
            ViewerEvent::LauncherCommand(ViewerCommand::ToggleFullscreen) => {
                self.toggle_fullscreen();
            }
            ViewerEvent::LauncherCommand(ViewerCommand::Reconnect) => {
                self.reconnect_attempts = 0;
                self.restart_session();
            }
            ViewerEvent::LauncherCommand(ViewerCommand::RefreshVideo) => {
                self.session.send(SessionCommand::RefreshVideo);
                if let Some(window) = &self.window {
                    window.set_title(&format!(
                        "EvertyDesk Viewer — {} — обновление видео…",
                        self.remote_id.trim()
                    ));
                }
            }
            ViewerEvent::LauncherCommand(ViewerCommand::FocusWindow) => {
                if let Some(window) = &self.window {
                    window.set_minimized(false);
                    window.focus_window();
                }
            }
            ViewerEvent::LauncherCommand(ViewerCommand::SetInputEnabled { enabled }) => {
                if !enabled {
                    self.release_remote_inputs();
                }
                self.input_enabled = enabled;
                self.redraw_toolbar();
                if let Some(window) = &self.window {
                    let mode = if enabled {
                        "управление включено"
                    } else {
                        "только просмотр"
                    };
                    window.set_title(&format!(
                        "EvertyDesk Viewer — {} — {mode}",
                        self.remote_id.trim()
                    ));
                }
                emit_status(&ViewerStatus::ControlApplied {
                    control: ViewerControl::InputEnabled { enabled },
                });
            }
            ViewerEvent::LauncherCommand(ViewerCommand::SetAudioEnabled { enabled }) => {
                self.set_audio_enabled(enabled);
                emit_status(&ViewerStatus::ControlApplied {
                    control: ViewerControl::AudioEnabled { enabled },
                });
            }
            ViewerEvent::LauncherCommand(ViewerCommand::SetClipboardEnabled { enabled }) => {
                self.set_clipboard_enabled(enabled);
                self.redraw_toolbar();
                emit_status(&ViewerStatus::ControlApplied {
                    control: ViewerControl::ClipboardEnabled { enabled },
                });
            }
            ViewerEvent::LauncherCommand(ViewerCommand::CycleDisplay { direction }) => {
                let direction = direction.signum();
                if direction != 0 {
                    self.cycle_display(direction as isize);
                }
            }
            ViewerEvent::LauncherCommand(ViewerCommand::SetQuality { quality }) => {
                self.quality = quality;
                self.session.set_quality(quality);
                self.redraw_toolbar();
                if let Some(window) = &self.window {
                    window.set_title(&format!(
                        "EvertyDesk Viewer — {} — профиль: {}",
                        self.remote_id.trim(),
                        quality.label()
                    ));
                }
                emit_status(&ViewerStatus::ControlApplied {
                    control: ViewerControl::Quality { quality },
                });
            }
            ViewerEvent::LauncherCommand(ViewerCommand::SetScaling { scaling }) => {
                self.set_scaling(scaling);
                emit_status(&ViewerStatus::ControlApplied {
                    control: ViewerControl::Scaling { scaling },
                });
            }
            ViewerEvent::Connected(peer) => {
                self.reconnect_scheduled_for = None;
                self.reconnect_attempts = 0;
                self.session_connected.store(true, Ordering::Release);
                self.session_started.get_or_insert_with(Instant::now);
                self.emit_control_snapshot();
                self.connection_notice = None;
                self.notice_expires_at = None;
                self.redraw_toolbar();
                if let Some(window) = &self.window {
                    window.set_title(&format!(
                        "EvertyDesk Viewer — {} — подключено: {peer}",
                        self.remote_id.trim()
                    ));
                }
            }
            ViewerEvent::Failed { generation, error } => {
                if generation != self.session.generation() {
                    return;
                }
                self.session_connected.store(false, Ordering::Release);
                eprintln!("[viewer] session failed: {error}");
                if let Some(window) = &self.window {
                    window.set_title(&format!("EvertyDesk Viewer — ошибка: {error}"));
                }
                if !is_permanent_connection_error(&error) {
                    self.schedule_reconnect();
                } else {
                    self.session_end_reason = safe_viewer_end_reason(&error);
                    self.connection_notice = Some("CONNECTION FAILED".to_owned());
                    self.notice_expires_at = None;
                    self.redraw_toolbar();
                }
            }
            ViewerEvent::Closed { generation } => {
                if generation == self.session.generation() {
                    self.session_connected.store(false, Ordering::Release);
                    self.schedule_reconnect();
                }
            }
            ViewerEvent::Reconnect { generation } => {
                if generation == self.session.generation() {
                    self.restart_session();
                }
            }
            ViewerEvent::Performance {
                fps_times_100,
                input_kbps,
                dropped_frames,
            } => {
                self.diagnostics.fps_times_100 = fps_times_100;
                self.diagnostics.input_kbps = input_kbps;
                self.diagnostics.dropped_frames = dropped_frames;
                self.diagnostics.last_performance_at = Some(Instant::now());
                emit_status(&ViewerStatus::Performance {
                    fps_times_100,
                    input_kbps,
                    dropped_frames,
                    session_seconds: self
                        .session_started
                        .map(|started| started.elapsed().as_secs())
                        .unwrap_or(0),
                    reconnect_count: self.reconnect_count,
                });
                if self.diagnostics_visible {
                    self.redraw_toolbar();
                }
            }
            ViewerEvent::RejectedPayload {
                reason,
                refresh_video,
            } => self.report_rejected_payload(reason, refresh_video),
            ViewerEvent::WatchdogStalled => {
                if self.viewer_visible.load(Ordering::Acquire)
                    && self.session_connected.load(Ordering::Acquire)
                    && !self.video_paused
                {
                    self.connection_notice = Some("VIDEO STALLED · REQUESTING KEYFRAME".to_owned());
                    self.notice_expires_at = None;
                    self.session.send(SessionCommand::RefreshVideo);
                    emit_status(&ViewerStatus::Recovery {
                        reason: "Видеопоток не обновлялся 15 секунд — запрошен свежий кадр"
                            .to_owned(),
                    });
                    self.redraw_toolbar();
                }
            }
        }
    }
}

fn is_permanent_connection_error(error: &str) -> bool {
    let error = error.to_lowercase();
    error.contains("wrong password")
        || error.contains("неверн")
        || error.contains("введите")
        || error.contains("enter password")
        || error.contains("id does not exist")
        || error.contains("id не существует")
}

fn safe_viewer_end_reason(reason: &str) -> String {
    let reason: String = reason
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(120)
        .collect();
    if reason.is_empty() {
        "Сессия завершена".to_owned()
    } else {
        reason
    }
}

fn normalized_wheel_delta(delta: MouseScrollDelta) -> (i32, i32) {
    const MAX_WHEEL_STEPS: f64 = 20.0;
    let (x, y) = match delta {
        MouseScrollDelta::LineDelta(x, y) => (f64::from(x), f64::from(y)),
        MouseScrollDelta::PixelDelta(delta) => (delta.x / 40.0, delta.y / 40.0),
    };
    let normalize = |value: f64| {
        if value.is_finite() {
            value.round().clamp(-MAX_WHEEL_STEPS, MAX_WHEEL_STEPS) as i32
        } else {
            0
        }
    };
    (normalize(x), normalize(y))
}

fn audio_frame_is_audible(enabled: &AtomicBool, pcm: &[u8]) -> bool {
    !pcm.is_empty() && pcm.len() <= MAX_AUDIO_FRAME_BYTES && enabled.load(Ordering::Acquire)
}

fn mouse_button_command(
    button: MouseButton,
    state: ElementState,
    x: i32,
    y: i32,
) -> Option<SessionCommand> {
    Some(match (button, state) {
        (MouseButton::Left, ElementState::Pressed) => SessionCommand::MouseDown { x, y },
        (MouseButton::Left, ElementState::Released) => SessionCommand::MouseUp { x, y },
        (MouseButton::Right, ElementState::Pressed) => SessionCommand::MouseRightDown { x, y },
        (MouseButton::Right, ElementState::Released) => SessionCommand::MouseRightUp { x, y },
        (MouseButton::Middle, ElementState::Pressed) => SessionCommand::MouseMiddleDown { x, y },
        (MouseButton::Middle, ElementState::Released) => SessionCommand::MouseMiddleUp { x, y },
        _ => return None,
    })
}

fn reconnect_delay_seconds(attempt: u32) -> u64 {
    (5_u64 * (1_u64 << attempt.saturating_sub(1).min(3))).min(40)
}

fn should_schedule_reconnect(pending_generation: Option<u64>, generation: u64) -> bool {
    pending_generation != Some(generation)
}

fn fps_times_100(input_fps: f32) -> u32 {
    if !input_fps.is_finite() || input_fps <= 0.0 {
        return 0;
    }
    (input_fps * 100.0).round().clamp(0.0, u32::MAX as f32) as u32
}

fn screenshot_directory() -> io::Result<PathBuf> {
    if let Some(profile) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        return Ok(PathBuf::from(profile).join("Pictures").join("EvertyDesk"));
    }
    Ok(std::env::current_dir()?.join("EvertyDesk Screenshots"))
}

fn safe_filename_component(value: &str) -> String {
    let safe: String = value
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    if safe.is_empty() {
        "remote".to_owned()
    } else {
        safe
    }
}

fn should_forward_clipboard(
    text: &str,
    last_sent: Option<&str>,
    last_remote: Option<&str>,
) -> bool {
    !text.is_empty() && last_sent != Some(text) && last_remote != Some(text)
}

fn clipboard_text_fingerprint(text: &str) -> (usize, u64) {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    (text.len(), hasher.finish())
}

fn clipboard_observation_changed(last_observed: &mut Option<(usize, u64)>, text: &str) -> bool {
    let fingerprint = clipboard_text_fingerprint(text);
    if *last_observed == Some(fingerprint) {
        return false;
    }
    *last_observed = Some(fingerprint);
    true
}

fn validate_rgba_payload(
    width: u32,
    height: u32,
    actual_bytes: usize,
    max_dimension: u32,
    max_bytes: usize,
) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("нулевой размер".to_owned());
    }
    if width > max_dimension || height > max_dimension {
        return Err(format!(
            "размер {width}×{height} превышает предел {max_dimension}×{max_dimension}"
        ));
    }
    let expected_bytes = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "переполнение размера RGBA".to_owned())?;
    if expected_bytes > max_bytes {
        return Err(format!(
            "RGBA занимает {}, предел {}",
            format_byte_limit(expected_bytes),
            format_byte_limit(max_bytes)
        ));
    }
    if actual_bytes != expected_bytes {
        return Err(format!(
            "ожидалось {expected_bytes} байт RGBA, получено {actual_bytes}"
        ));
    }
    Ok(())
}

fn format_byte_limit(bytes: usize) -> String {
    if bytes >= 1024 * 1024 && bytes.is_multiple_of(1024 * 1024) {
        format!("{} МиБ", bytes / (1024 * 1024))
    } else if bytes >= 1024 && bytes.is_multiple_of(1024) {
        format!("{} КиБ", bytes / 1024)
    } else {
        format!("{bytes} байт")
    }
}

fn write_rgba_png<W: Write>(
    output: W,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(), png::EncodingError> {
    let mut encoder = png::Encoder::new(output, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    writer.finish()
}

fn watchdog_step(watching: bool, sequence_unchanged: bool, intervals: u8) -> (u8, bool) {
    if !watching || !sequence_unchanged {
        return (0, false);
    }
    let intervals = intervals.saturating_add(1);
    if intervals >= 3 {
        (0, true)
    } else {
        (intervals, false)
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.clipboard_watcher_stop.store(true, Ordering::Release);
        if let Some(text) = self.last_remote_clipboard.as_mut() {
            text.zeroize();
        }
        if let Some(text) = self.last_sent_clipboard.as_mut() {
            text.zeroize();
        }
    }
}

fn named_viewer_shortcut(modifiers: ModifiersState, key: NamedKey) -> Option<ViewerShortcut> {
    if modifiers.alt_key() && key == NamedKey::Enter {
        return Some(ViewerShortcut::ToggleFullscreen);
    }
    if !(modifiers.control_key() && modifiers.alt_key()) {
        return None;
    }
    match key {
        NamedKey::ArrowLeft => Some(ViewerShortcut::PreviousDisplay),
        NamedKey::ArrowRight => Some(ViewerShortcut::NextDisplay),
        NamedKey::End => Some(ViewerShortcut::CtrlAltDelete),
        _ => None,
    }
}

fn character_viewer_shortcut(modifiers: ModifiersState, text: &str) -> Option<ViewerShortcut> {
    if !(modifiers.control_key() && modifiers.alt_key()) {
        return None;
    }
    if text.eq_ignore_ascii_case("f") {
        Some(ViewerShortcut::ToggleFullscreen)
    } else if text.eq_ignore_ascii_case("m") {
        Some(ViewerShortcut::NextScaling)
    } else if text.eq_ignore_ascii_case("q") {
        Some(ViewerShortcut::NextQuality)
    } else if text.eq_ignore_ascii_case("p") {
        Some(ViewerShortcut::ToggleVideoPause)
    } else if text.eq_ignore_ascii_case("k") {
        Some(ViewerShortcut::Reconnect)
    } else if text.eq_ignore_ascii_case("a") {
        Some(ViewerShortcut::ToggleAudio)
    } else if text.eq_ignore_ascii_case("i") {
        Some(ViewerShortcut::ToggleInput)
    } else if text.eq_ignore_ascii_case("h") {
        Some(ViewerShortcut::ToggleToolbar)
    } else if text.eq_ignore_ascii_case("c") {
        Some(ViewerShortcut::ToggleClipboard)
    } else if text.eq_ignore_ascii_case("s") {
        Some(ViewerShortcut::Screenshot)
    } else if text.eq_ignore_ascii_case("d") {
        Some(ViewerShortcut::ToggleDiagnostics)
    } else if text.eq_ignore_ascii_case("r") {
        Some(ViewerShortcut::RefreshVideo)
    } else {
        None
    }
}

fn active_control_modifiers(modifiers: ModifiersState) -> Vec<ControlKey> {
    let mut active = Vec::with_capacity(4);
    if modifiers.control_key() {
        active.push(ControlKey::Control);
    }
    if modifiers.alt_key() {
        active.push(ControlKey::Alt);
    }
    if modifiers.shift_key() {
        active.push(ControlKey::Shift);
    }
    if modifiers.super_key() {
        active.push(ControlKey::Meta);
    }
    active
}

fn named_key_to_control_key(key: NamedKey) -> Option<ControlKey> {
    Some(match key {
        NamedKey::ArrowDown => ControlKey::DownArrow,
        NamedKey::ArrowLeft => ControlKey::LeftArrow,
        NamedKey::ArrowRight => ControlKey::RightArrow,
        NamedKey::ArrowUp => ControlKey::UpArrow,
        NamedKey::Escape => ControlKey::Escape,
        NamedKey::Tab => ControlKey::Tab,
        NamedKey::Backspace => ControlKey::Backspace,
        NamedKey::Enter => ControlKey::Return,
        NamedKey::Space => ControlKey::Space,
        NamedKey::Insert => ControlKey::Insert,
        NamedKey::Delete => ControlKey::Delete,
        NamedKey::Home => ControlKey::Home,
        NamedKey::End => ControlKey::End,
        NamedKey::PageUp => ControlKey::PageUp,
        NamedKey::PageDown => ControlKey::PageDown,
        NamedKey::F1 => ControlKey::F1,
        NamedKey::F2 => ControlKey::F2,
        NamedKey::F3 => ControlKey::F3,
        NamedKey::F4 => ControlKey::F4,
        NamedKey::F5 => ControlKey::F5,
        NamedKey::F6 => ControlKey::F6,
        NamedKey::F7 => ControlKey::F7,
        NamedKey::F8 => ControlKey::F8,
        NamedKey::F9 => ControlKey::F9,
        NamedKey::F10 => ControlKey::F10,
        NamedKey::F11 => ControlKey::F11,
        NamedKey::F12 => ControlKey::F12,
        _ => return None,
    })
}

#[cfg(test)]
mod quality_tests {
    use super::*;

    #[test]
    fn quality_profiles_change_transport_display_settings() {
        let mut display = DisplayConfig::default();
        apply_quality_profile(&mut display, ConnectionQuality::Smooth);
        assert_eq!(display.target_fps, 60);
        assert!(display.adaptive_quality);

        apply_quality_profile(&mut display, ConnectionQuality::Sharp);
        assert_eq!(display.target_fps, 30);
        assert_eq!(display.min_fps, 15);
        assert!(!display.adaptive_quality);
    }

    #[test]
    fn game_profile_forces_concrete_codec_and_low_latency_settings() {
        let mut display = DisplayConfig::default();
        apply_quality_profile(&mut display, ConnectionQuality::Sharp);
        apply_transport_profile(&mut display, true, ViewerGameCodec::H265);

        assert_eq!(display.streaming_mode, StreamingMode::Game);
        assert_eq!(display.target_fps, 60);
        assert!(display.min_fps >= 30);
        assert!(!display.adaptive_quality);
        assert_eq!(display.codec, CodecPreference::H265);

        apply_transport_profile(&mut display, true, ViewerGameCodec::Auto);
        assert_eq!(display.codec, CodecPreference::H264);
    }

    #[test]
    fn desktop_profile_forces_evrtck_even_after_saved_game_settings() {
        let mut display = DisplayConfig {
            streaming_mode: StreamingMode::Game,
            codec: CodecPreference::H265,
            ..DisplayConfig::default()
        };

        apply_transport_profile(&mut display, false, ViewerGameCodec::Av1);

        assert_eq!(display.streaming_mode, StreamingMode::Support);
        assert_eq!(display.codec, CodecPreference::Evrtck);
    }

    #[test]
    fn evrt2_experiment_is_requested_only_for_game_opt_in() {
        assert!(should_request_evrt2_experiment(true, true));
        assert!(!should_request_evrt2_experiment(true, false));
        assert!(!should_request_evrt2_experiment(false, true));
        assert!(!should_request_evrt2_experiment(false, false));
    }

    #[test]
    fn transport_profile_label_distinguishes_desktop_and_game() {
        assert_eq!(
            viewer_transport_profile_label(false, ViewerGameCodec::Av1),
            "Desktop EVRTCK"
        );
        assert_eq!(
            viewer_transport_profile_label(true, ViewerGameCodec::H265),
            "Game H265"
        );
    }

    #[test]
    fn viewer_scaling_maps_to_the_expected_pixels_pipeline() {
        assert!(matches!(
            pixels_scaling_mode(ViewerScaling::SmoothFit),
            ScalingMode::Fill
        ));
        assert!(matches!(
            pixels_scaling_mode(ViewerScaling::PixelPerfect),
            ScalingMode::PixelPerfect
        ));
        assert_eq!(ViewerScaling::SmoothFit.next(), ViewerScaling::PixelPerfect);
    }
}

#[cfg(test)]
mod reconnect_tests {
    use super::*;

    #[test]
    fn reconnect_backoff_is_bounded() {
        assert_eq!(reconnect_delay_seconds(1), 5);
        assert_eq!(reconnect_delay_seconds(2), 10);
        assert_eq!(reconnect_delay_seconds(3), 20);
        assert_eq!(reconnect_delay_seconds(4), 40);
        assert_eq!(reconnect_delay_seconds(20), 40);
    }

    #[test]
    fn duplicate_failures_do_not_schedule_two_reconnects_for_one_generation() {
        assert!(should_schedule_reconnect(None, 7));
        assert!(!should_schedule_reconnect(Some(7), 7));
        assert!(should_schedule_reconnect(Some(7), 8));
    }

    #[test]
    fn authentication_and_unknown_id_errors_are_permanent() {
        assert!(is_permanent_connection_error("Wrong Password"));
        assert!(is_permanent_connection_error("ID does not exist"));
        assert!(is_permanent_connection_error("Введите пароль"));
        assert!(!is_permanent_connection_error("relay connection timed out"));
        assert!(!is_permanent_connection_error("network unreachable"));
    }
}

#[cfg(test)]
mod input_safety_tests {
    use super::*;

    #[test]
    fn supported_mouse_buttons_have_matching_release_commands() {
        assert!(matches!(
            mouse_button_command(MouseButton::Left, ElementState::Released, 10, 20),
            Some(SessionCommand::MouseUp { x: 10, y: 20 })
        ));
        assert!(matches!(
            mouse_button_command(MouseButton::Right, ElementState::Released, 10, 20),
            Some(SessionCommand::MouseRightUp { x: 10, y: 20 })
        ));
        assert!(matches!(
            mouse_button_command(MouseButton::Middle, ElementState::Released, 10, 20),
            Some(SessionCommand::MouseMiddleUp { x: 10, y: 20 })
        ));
        assert!(mouse_button_command(MouseButton::Back, ElementState::Pressed, 10, 20).is_none());
    }

    #[test]
    fn toolbar_hit_zones_are_bounded_and_map_every_action() {
        let width = 960;
        let (start, button_width, _) = toolbar_geometry(width);
        assert_eq!(TOOLBAR_ACTIONS.len(), TOOLBAR_ACTION_COUNT as usize);
        for (index, action) in TOOLBAR_ACTIONS.into_iter().enumerate() {
            let x = start + i32::try_from(index).unwrap() * button_width + 2;
            assert_eq!(toolbar_hit_test(x, 5, width), Some(action));
            assert_eq!(
                toolbar_action_at_index(i32::try_from(index).unwrap()),
                Some(action)
            );
            assert_eq!(toolbar_action_index(action), i32::try_from(index).unwrap());
        }
        assert_eq!(toolbar_action_at_index(-1), None);
        assert_eq!(toolbar_action_at_index(TOOLBAR_ACTION_COUNT), None);
        assert_eq!(toolbar_hit_test(start - 1, 5, width), None);
        assert_eq!(toolbar_hit_test(start, TOOLBAR_HEIGHT, width), None);
    }

    #[test]
    fn toolbar_actions_have_unique_non_empty_labels() {
        let mut labels = std::collections::HashSet::new();
        for action in TOOLBAR_ACTIONS {
            let label = toolbar_action_label(action);
            assert!(!label.trim().is_empty());
            assert!(labels.insert(label));
            assert_eq!(
                toolbar_action_at_index(toolbar_action_index(action)),
                Some(action)
            );
        }
    }

    #[test]
    fn toolbar_keeps_every_action_reachable_on_narrow_frames() {
        let width = 320;
        let (start, button_width, total_width) = toolbar_geometry(width);
        assert!(start >= 0);
        assert!(start + total_width <= width);
        assert!(button_width < TOOLBAR_BUTTON_WIDTH);
        assert_eq!(total_width, button_width * TOOLBAR_ACTION_COUNT);
        let disconnect_x = start
            + toolbar_action_index(ToolbarAction::Disconnect) * button_width
            + button_width / 2;
        assert_eq!(
            toolbar_hit_test(disconnect_x, 5, width),
            Some(ToolbarAction::Disconnect)
        );
    }

    #[test]
    fn toolbar_handle_hit_zone_toggles_above_and_below_toolbar() {
        let width = 960;
        let handle_x = width / 2;
        assert_eq!(
            toolbar_chrome_hit_test(handle_x, TOOLBAR_HEIGHT + 4, width, true),
            Some(ToolbarChrome::Handle)
        );
        assert_eq!(
            toolbar_chrome_hit_test(handle_x, 4, width, false),
            Some(ToolbarChrome::Handle)
        );
        assert_eq!(
            toolbar_chrome_hit_test(
                handle_x,
                TOOLBAR_HEIGHT + TOOLBAR_HANDLE_HEIGHT,
                width,
                true
            ),
            None
        );
        assert_eq!(
            toolbar_chrome_hit_test(handle_x, TOOLBAR_HANDLE_HEIGHT, width, false),
            None
        );
    }

    #[test]
    fn toolbar_chrome_consumes_input_without_refreshing_hidden_toolbar_activity() {
        let width = 960;
        let handle_x = width / 2;
        let visible_action = toolbar_chrome_hit_test(260, 5, width, true);
        assert!(matches!(visible_action, Some(ToolbarChrome::Action(_))));
        assert!(toolbar_chrome_consumes_input(visible_action));
        assert!(toolbar_chrome_refreshes_activity(visible_action, true));

        let hidden_handle = toolbar_chrome_hit_test(handle_x, 4, width, false);
        assert_eq!(hidden_handle, Some(ToolbarChrome::Handle));
        assert!(toolbar_chrome_consumes_input(hidden_handle));
        assert!(!toolbar_chrome_refreshes_activity(hidden_handle, false));
    }

    #[test]
    fn toolbar_secondary_overlays_are_below_the_handle() {
        assert_eq!(
            toolbar_overlay_top(true, 0),
            TOOLBAR_HEIGHT + TOOLBAR_HANDLE_HEIGHT
        );
        assert_eq!(toolbar_overlay_top(false, 0), TOOLBAR_HANDLE_HEIGHT);
        assert!(toolbar_overlay_top(true, 5) > TOOLBAR_HEIGHT + TOOLBAR_HANDLE_HEIGHT);
        assert!(toolbar_overlay_top(false, 5) > TOOLBAR_HANDLE_HEIGHT);
    }

    #[test]
    fn toolbar_tooltips_are_clipped_to_the_frame_width() {
        let clipped =
            toolbar_tooltip_text("Fullscreen (Alt+Enter) - Hide controls: Ctrl+Alt+H", 160);
        assert!(clipped.ends_with('…'));
        assert!(clipped.chars().count() <= 17);

        let short = toolbar_tooltip_text("Disconnect", 320);
        assert_eq!(short, "Disconnect");
        assert_eq!(toolbar_text_width("Diagnostics"), 88);
    }

    #[test]
    fn toolbar_renderer_stays_inside_the_rgba_buffer() {
        let mut frame = vec![0; 360 * 260 * 4];
        draw_toolbar(
            &mut frame,
            360,
            260,
            Some(ToolbarAction::Screenshot),
            false,
            true,
            false,
            false,
            false,
            true,
            true,
            &ViewerDiagnostics {
                codec: "H264".to_owned(),
                fps_times_100: 5_998,
                input_kbps: 2_400,
                dropped_frames: 3,
                latency_ms: Some(42),
                last_performance_at: Some(Instant::now()),
                ..ViewerDiagnostics::default()
            },
            65,
            1,
            ViewerScaling::SmoothFit,
            ConnectionQuality::Balanced,
            "Game H264",
            true,
            Some(0),
            2,
            Some("RECONNECTING"),
        );
        assert!(frame.chunks_exact(4).any(|pixel| pixel[3] == 255));
    }

    #[test]
    fn local_shortcuts_cover_session_toolbar_actions() {
        let control_alt = ModifiersState::CONTROL | ModifiersState::ALT;
        assert_eq!(
            named_viewer_shortcut(control_alt, NamedKey::End),
            Some(ViewerShortcut::CtrlAltDelete)
        );
        assert_eq!(
            named_viewer_shortcut(control_alt, NamedKey::ArrowRight),
            Some(ViewerShortcut::NextDisplay)
        );
        assert_eq!(
            character_viewer_shortcut(control_alt, "d"),
            Some(ViewerShortcut::ToggleDiagnostics)
        );
        assert_eq!(
            character_viewer_shortcut(control_alt, "R"),
            Some(ViewerShortcut::RefreshVideo)
        );
        assert_eq!(
            character_viewer_shortcut(control_alt, "q"),
            Some(ViewerShortcut::NextQuality)
        );
        assert_eq!(
            character_viewer_shortcut(control_alt, "p"),
            Some(ViewerShortcut::ToggleVideoPause)
        );
        assert_eq!(
            character_viewer_shortcut(control_alt, "K"),
            Some(ViewerShortcut::Reconnect)
        );
        assert_eq!(
            character_viewer_shortcut(control_alt, "a"),
            Some(ViewerShortcut::ToggleAudio)
        );
        assert_eq!(
            character_viewer_shortcut(control_alt, "h"),
            Some(ViewerShortcut::ToggleToolbar)
        );
        assert_eq!(
            character_viewer_shortcut(ModifiersState::CONTROL, "d"),
            None
        );
    }

    #[test]
    fn end_reasons_are_sanitized_and_bounded() {
        let reason = safe_viewer_end_reason(&format!("lost\0\r\n{}", "x".repeat(300)));
        assert!(!reason.chars().any(char::is_control));
        assert!(reason.chars().count() <= 120);
        assert_eq!(safe_viewer_end_reason(" \r\n "), "Сессия завершена");
    }

    #[test]
    fn paused_and_background_video_use_bounded_frame_rates() {
        assert_eq!(desired_video_fps(false, true, 60), 60);
        assert_eq!(desired_video_fps(false, false, 60), 5);
        assert_eq!(desired_video_fps(true, true, 60), 1);
        assert_eq!(desired_video_fps(true, false, 60), 1);
    }

    #[test]
    fn diagnostics_health_detects_stale_slow_and_healthy_sessions() {
        assert_eq!(diagnostics_health(None, 0, None), "Waiting");
        assert_eq!(
            diagnostics_health(Some(35), 6_000, Some(Duration::from_secs(6))),
            "Stale"
        );
        assert_eq!(
            diagnostics_health(Some(220), 6_000, Some(Duration::from_millis(100))),
            "Poor"
        );
        assert_eq!(
            diagnostics_health(Some(80), 2_500, Some(Duration::from_millis(100))),
            "Fair"
        );
        assert_eq!(
            diagnostics_health(Some(35), 6_000, Some(Duration::from_millis(100))),
            "Good"
        );
    }

    #[test]
    fn reconnect_keeps_the_selected_monitor_when_it_still_exists() {
        let displays = [
            RemoteDisplay {
                index: 2,
                name: "Primary".to_owned(),
                width: 1920,
                height: 1080,
                x: 0,
                y: 0,
                cursor_embedded: false,
            },
            RemoteDisplay {
                index: 7,
                name: "Secondary".to_owned(),
                width: 2560,
                height: 1440,
                x: 1920,
                y: 0,
                cursor_embedded: true,
            },
        ];

        assert_eq!(resolved_display_index(&displays, Some(7)), Some(7));
        assert_eq!(resolved_display_index(&displays, Some(99)), Some(2));
        assert_eq!(resolved_display_index(&[], Some(7)), None);
    }

    #[test]
    fn frame_input_coordinates_are_mapped_to_selected_display_origin() {
        let display = RemoteDisplay {
            index: 7,
            name: "Secondary".to_owned(),
            width: 2560,
            height: 1440,
            x: 1920,
            y: -180,
            cursor_embedded: false,
        };

        assert_eq!(
            map_frame_position_to_display(0, 0, (2560, 1440), &display),
            (1920, -180)
        );
        assert_eq!(
            map_frame_position_to_display(2559, 1439, (2560, 1440), &display),
            (4479, 1259)
        );
    }

    #[test]
    fn downscaled_frame_input_coordinates_expand_to_display_pixels() {
        let display = RemoteDisplay {
            index: 0,
            name: "Main".to_owned(),
            width: 3840,
            height: 2160,
            x: 0,
            y: 0,
            cursor_embedded: false,
        };

        assert_eq!(
            map_frame_position_to_display(960, 540, (1920, 1080), &display),
            (1920, 1080)
        );
    }

    #[test]
    fn wheel_input_is_finite_rounded_and_clamped() {
        assert_eq!(
            normalized_wheel_delta(MouseScrollDelta::LineDelta(1.4, -2.6)),
            (1, -3)
        );
        assert_eq!(
            normalized_wheel_delta(MouseScrollDelta::PixelDelta(
                winit::dpi::PhysicalPosition::new(4_000.0, -4_000.0)
            )),
            (20, -20)
        );
        assert_eq!(
            normalized_wheel_delta(MouseScrollDelta::PixelDelta(
                winit::dpi::PhysicalPosition::new(f64::NAN, f64::INFINITY)
            )),
            (0, 0)
        );
    }

    #[test]
    fn audio_mute_is_isolated_per_viewer_session() {
        let first = AtomicBool::new(true);
        let second = AtomicBool::new(true);
        assert!(audio_frame_is_audible(&first, &[1, 2]));
        first.store(false, Ordering::Release);
        assert!(!audio_frame_is_audible(&first, &[1, 2]));
        assert!(audio_frame_is_audible(&second, &[1, 2]));
        assert!(!audio_frame_is_audible(&second, &[]));
        assert!(!audio_frame_is_audible(
            &second,
            &vec![0; MAX_AUDIO_FRAME_BYTES + 1]
        ));
    }
}

#[cfg(test)]
mod telemetry_tests {
    use super::*;

    #[test]
    fn fps_is_encoded_without_floats_in_ipc() {
        assert_eq!(fps_times_100(59.976), 5_998);
        assert_eq!(fps_times_100(0.0), 0);
        assert_eq!(fps_times_100(f32::NAN), 0);
        assert_eq!(fps_times_100(f32::INFINITY), 0);
    }

    #[test]
    fn screenshot_names_cannot_escape_the_picture_directory() {
        assert_eq!(safe_filename_component("123 456/../PC"), "123_456____PC");
        assert_eq!(safe_filename_component("\n\r"), "remote");
        assert!(safe_filename_component(&"a".repeat(100)).len() <= 48);
    }

    #[test]
    fn clipboard_echoes_and_duplicates_are_suppressed() {
        assert!(should_forward_clipboard("new", Some("old"), Some("remote")));
        assert!(!should_forward_clipboard("same", Some("same"), None));
        assert!(!should_forward_clipboard("same", None, Some("same")));
        assert!(!should_forward_clipboard("", None, None));
        assert_eq!(
            clipboard_text_fingerprint("same"),
            clipboard_text_fingerprint("same")
        );
        assert_ne!(
            clipboard_text_fingerprint("same"),
            clipboard_text_fingerprint("changed")
        );

        let mut observed = None;
        assert!(clipboard_observation_changed(&mut observed, "first"));
        assert!(!clipboard_observation_changed(&mut observed, "first"));
        assert!(clipboard_observation_changed(&mut observed, "second"));
    }

    #[test]
    fn screenshot_encoder_writes_a_complete_png() {
        let mut encoded = Vec::new();
        write_rgba_png(&mut encoded, 1, 1, &[255, 0, 0, 255]).unwrap();
        assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");
        assert!(encoded.windows(4).any(|chunk| chunk == b"IEND"));
    }

    #[test]
    fn watchdog_only_refreshes_visible_connected_stalls() {
        assert_eq!(watchdog_step(false, true, 2), (0, false));
        assert_eq!(watchdog_step(true, false, 2), (0, false));
        assert_eq!(watchdog_step(true, true, 0), (1, false));
        assert_eq!(watchdog_step(true, true, 2), (0, true));
    }

    #[test]
    fn outgoing_viewer_status_respects_the_ipc_limit() {
        assert!(encode_status(&ViewerStatus::Connected {
            peer: "remote".to_owned(),
        })
        .is_ok());
        let error = encode_status(&ViewerStatus::Recovery {
            reason: "x".repeat(MAX_IPC_LINE_BYTES),
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn status_queue_reports_backpressure_without_blocking() {
        let (commands, receiver) = mpsc::sync_channel(1);
        let writer = StatusWriter {
            commands,
            open: Arc::new(AtomicBool::new(true)),
        };

        enqueue_status_output(&writer, StatusOutput::Line(b"first".to_vec()), None).unwrap();
        let error = enqueue_status_output(&writer, StatusOutput::Line(b"second".to_vec()), None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);

        drop(receiver);
        let error =
            enqueue_status_output(&writer, StatusOutput::Line(Vec::new()), None).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn final_status_can_wait_boundedly_for_queue_capacity() {
        let (commands, receiver) = mpsc::sync_channel(1);
        let writer = StatusWriter {
            commands,
            open: Arc::new(AtomicBool::new(true)),
        };
        enqueue_status_output(&writer, StatusOutput::Line(b"first".to_vec()), None).unwrap();
        let consumer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            let _ = receiver.recv();
            let _ = receiver.recv();
        });

        enqueue_status_output(
            &writer,
            StatusOutput::Line(b"final".to_vec()),
            Some(Duration::from_secs(1)),
        )
        .unwrap();
        consumer.join().unwrap();
    }

    #[test]
    fn rgba_payload_validation_rejects_dimensions_lengths_and_memory_excess() {
        assert!(validate_rgba_payload(2, 2, 16, 64, 1_024).is_ok());
        assert!(validate_rgba_payload(0, 2, 0, 64, 1_024).is_err());
        assert!(validate_rgba_payload(65, 2, 520, 64, 1_024).is_err());
        assert!(validate_rgba_payload(16, 16, 1_023, 64, 1_024).is_err());
        assert!(validate_rgba_payload(16, 16, 1_024, 64, 1_023).is_err());
    }

    #[test]
    fn payload_limits_have_readable_binary_units() {
        assert_eq!(format_byte_limit(512), "512 байт");
        assert_eq!(format_byte_limit(64 * 1024), "64 КиБ");
        assert_eq!(format_byte_limit(1024 * 1024), "1 МиБ");
    }
}
