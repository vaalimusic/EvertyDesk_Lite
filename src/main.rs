#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod address_book;
mod capture;
mod crypto;
mod host;
mod llm;
mod mf_encode;
mod mf_video;
mod nvenc;
mod rustdesk_proto;
mod settings;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod software_ui;
mod transport;
mod ui;
mod video;
mod videotoolbox;
mod vp9_mf;
#[cfg(feature = "live-vpx-system")]
mod vpx_system;

use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use eframe::egui::{self, ColorImage, TextureHandle};
use host::{HostEvent, HostService, HostState};
use rustdesk_proto::ControlKey;
use settings as settings_mod;
use settings_mod::{
    generate_numeric_token, AppConfig, ConnectionHistoryEntry, ContactEntry, CoordinateMode,
};
use transport::{
    ConnectionRequest, ConnectionState, RemoteDisplay, SessionCommand, SessionEvent,
    TransportClient,
};
use ui::widgets::*;

const APP_NAME: &str = "EvertyDesk Lite";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppMode {
    Connect,
    Host,
    History,
    Contacts,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UiLang {
    Ru,
    En,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectKind {
    Screen,
    Shell,
}

fn tr(lang: UiLang, ru: &'static str, en: &'static str) -> &'static str {
    match lang {
        UiLang::Ru => ru,
        UiLang::En => en,
    }
}

fn main() -> eframe::Result<()> {
    if let Some(exit_code) = run_cli_connect() {
        std::process::exit(exit_code);
    }

    let renderer_mode = std::env::var("EVERTYDESK_RENDERER")
        .unwrap_or_else(|_| "auto".to_owned())
        .to_ascii_lowercase();

    #[cfg(target_os = "linux")]
    if renderer_mode == "auto" && std::env::var_os("EVERTYDESK_LINUX_AUTOSTART_CHILD").is_none() {
        return run_linux_auto_gui();
    }

    match renderer_mode.as_str() {
        "glow" | "opengl" => return run_gui(eframe::Renderer::Glow),
        "wgpu" | "vulkan" => return run_gui(eframe::Renderer::Wgpu),
        // Pure-CPU framebuffer UI (minifb) — no OpenGL/GLX/Vulkan at all.
        // The reliable choice for VMs where GLX is broken (e.g. Astra Linux
        // on SVGA3D: "GLXBadContextTag"). Use: EVERTYDESK_RENDERER=software
        "software" | "minifb" | "cpu" | "softbuffer" => {
            run_software_ui_or_headless();
            return Ok(());
            #[cfg(all(target_os = "linux", any()))]
            {
                eprintln!("[EvertyDesk] Software (CPU) UI backend — no OpenGL.");
                if let Err(err) = software_ui::run_software_ui() {
                    eprintln!("[EvertyDesk] Software UI failed: {err}");
                    run_headless_host();
                }
                return Ok(());
            }
            #[cfg(all(not(target_os = "linux"), any()))]
            {
                eprintln!("[EvertyDesk] Software UI backend is Linux-only; using WGPU.");
                return run_gui(eframe::Renderer::Wgpu);
            }
        }
        "host" | "headless" => {
            run_headless_host();
            return Ok(());
        }
        _ => {}
    }

    match run_gui(eframe::Renderer::Wgpu) {
        Ok(()) => Ok(()),
        Err(wgpu_error) => {
            eprintln!("[EvertyDesk] WGPU renderer failed: {wgpu_error:?}");
            #[cfg(target_os = "linux")]
            {
                eprintln!(
                    "[EvertyDesk] On Linux, OpenGL fallback must be launched by the safe wrapper."
                );
                eprintln!(
                    "[EvertyDesk] Run ./scripts/run-linux-safe.sh or install with ./scripts/install-linux-user.sh.\n"
                );
                run_headless_host();
                Ok(())
            }
            #[cfg(not(target_os = "linux"))]
            {
                eprintln!("[EvertyDesk] Trying OpenGL/Glow renderer...");
                match run_gui(eframe::Renderer::Glow) {
                    Ok(()) => Ok(()),
                    Err(glow_error) => {
                        eprintln!("[EvertyDesk] Glow renderer failed: {glow_error:?}");
                        let msg = format!("{wgpu_error:?}\n{glow_error:?}");
                        if msg.contains("NoSuitableAdapterFound")
                            || msg.contains("NoSuitable")
                            || msg.contains("glutin")
                            || msg.contains("OpenGL")
                        {
                            eprintln!("\n[EvertyDesk] Нет подходящего графического режима.");
                            eprintln!(
                            "[EvertyDesk] Запускаю в режиме без GUI (--host). Для окна нужен X11/Wayland + OpenGL/Vulkan.\n"
                        );
                            run_software_ui_or_headless();
                            Ok(())
                        } else {
                            Err(glow_error)
                        }
                    }
                }
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run_software_ui_or_headless() {
    eprintln!("[EvertyDesk] Trying CPU software UI backend (no OpenGL/Vulkan)...");
    match software_ui::run_software_ui() {
        Ok(()) => {}
        Err(err) => {
            eprintln!("[EvertyDesk] Software UI failed: {err}");
            eprintln!("[EvertyDesk] Starting headless host mode.");
            run_headless_host();
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn run_software_ui_or_headless() {
    eprintln!("[EvertyDesk] Software UI backend is unavailable on this platform.");
    run_headless_host();
}

#[cfg(target_os = "linux")]
fn run_linux_auto_gui() -> eframe::Result<()> {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            eprintln!("[EvertyDesk] Cannot locate current executable: {err}");
            run_headless_host();
            return Ok(());
        }
    };

    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut attempts: Vec<LinuxGuiAttempt> = Vec::new();

    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        attempts.push(LinuxGuiAttempt {
            title: "Wayland OpenGL software",
            renderer: "glow",
            envs: &[
                ("WINIT_UNIX_BACKEND", "wayland"),
                ("EVERTYDESK_SOFTWARE", "1"),
                ("LIBGL_ALWAYS_SOFTWARE", "1"),
                ("MESA_LOADER_DRIVER_OVERRIDE", "llvmpipe"),
            ],
        });
    }

    if std::env::var_os("DISPLAY").is_some() {
        attempts.push(LinuxGuiAttempt {
            title: "X11 OpenGL default",
            renderer: "glow",
            envs: &[("WINIT_UNIX_BACKEND", "x11"), ("LIBGL_DRI3_DISABLE", "1")],
        });
        attempts.push(LinuxGuiAttempt {
            title: "X11 OpenGL software",
            renderer: "glow",
            envs: &[
                ("WINIT_UNIX_BACKEND", "x11"),
                ("EVERTYDESK_SOFTWARE", "1"),
                ("LIBGL_ALWAYS_SOFTWARE", "1"),
                ("LIBGL_DRI3_DISABLE", "1"),
                ("MESA_LOADER_DRIVER_OVERRIDE", "llvmpipe"),
                ("GALLIUM_DRIVER", "llvmpipe"),
                ("MESA_GL_VERSION_OVERRIDE", "3.3"),
                ("MESA_GLSL_VERSION_OVERRIDE", "330"),
            ],
        });
        attempts.push(LinuxGuiAttempt {
            title: "X11 indirect GLX",
            renderer: "glow",
            envs: &[
                ("WINIT_UNIX_BACKEND", "x11"),
                ("LIBGL_ALWAYS_INDIRECT", "1"),
                ("LIBGL_DRI3_DISABLE", "1"),
            ],
        });
    }

    attempts.push(LinuxGuiAttempt {
        title: "WGPU auto",
        renderer: "wgpu",
        envs: &[],
    });

    // Pure-CPU framebuffer (minifb) — no OpenGL/GLX/Vulkan. Guaranteed to work
    // on VMs with broken GLX (Astra/SVGA3D "GLXBadContextTag"). Tried as a
    // child so a crash in earlier GL attempts can't take us down with it.
    attempts.push(LinuxGuiAttempt {
        title: "CPU software framebuffer (minifb)",
        renderer: "software",
        envs: &[],
    });

    eprintln!("[EvertyDesk] Linux GUI autostart: checking available renderer...");
    for attempt in attempts {
        eprintln!("[EvertyDesk] Trying {}...", attempt.title);
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(&args)
            .env("EVERTYDESK_LINUX_AUTOSTART_CHILD", "1")
            .env("EVERTYDESK_RENDERER", attempt.renderer)
            .env("RUST_BACKTRACE", "0")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        for (name, value) in attempt.envs {
            cmd.env(name, value);
        }

        match cmd.status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => {
                eprintln!("[EvertyDesk] {} failed: {status}", attempt.title);
            }
            Err(err) => {
                eprintln!("[EvertyDesk] {} failed to start: {err}", attempt.title);
            }
        }
    }

    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("[EvertyDesk] No desktop session detected. Starting headless host mode.");
        run_headless_host();
        return Ok(());
    }

    eprintln!("[EvertyDesk] No GUI renderer worked on this Linux desktop.");
    eprintln!("[EvertyDesk] This system rejected both OpenGL/GLX and WGPU/Vulkan.");
    eprintln!("[EvertyDesk] Starting CPU software UI backend...");
    if let Err(err) = software_ui::run_software_ui() {
        eprintln!("[EvertyDesk] Software UI failed: {err}");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
struct LinuxGuiAttempt {
    title: &'static str,
    renderer: &'static str,
    envs: &'static [(&'static str, &'static str)],
}

/// Decode the embedded EvertyDesk logo (`edesk_lite_logo.png`) into a window
/// icon. Returns `None` if the image can't be decoded.
fn load_app_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/edesk_lite_logo.png"));
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

fn run_gui(renderer: eframe::Renderer) -> eframe::Result<()> {
    let wgpu_opts = eframe::egui_wgpu::WgpuConfiguration::default();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_inner_size([1180.0, 740.0])
        .with_min_inner_size([920.0, 600.0]);
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }

    let options = eframe::NativeOptions {
        viewport,
        hardware_acceleration: if std::env::var_os("EVERTYDESK_SOFTWARE").is_some() {
            eframe::HardwareAcceleration::Off
        } else {
            eframe::HardwareAcceleration::Preferred
        },
        wgpu_options: wgpu_opts,
        renderer,
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| {
            configure_ui_scale(&cc.egui_ctx);
            configure_style(&cc.egui_ctx);
            eprintln!(
                "[EvertyDesk] Build codecs: {}",
                crate::video::build_codec_label()
            );
            Ok(Box::new(EvertyDeskApp::new()))
        }),
    )
}

/// Headless host loop — same as `--host` CLI mode, used as auto-fallback
/// when the GUI cannot initialize (no GPU adapter on the server).
fn run_headless_host() {
    let config = AppConfig::load_or_create();
    eprintln!(
        "[cli] Headless host mode. local_id={} server={}",
        config.local_id, config.server.id_server
    );
    let svc = host::HostService::start(config);
    loop {
        thread::sleep(Duration::from_millis(100));
        while let Some(ev) = svc.try_recv() {
            match &ev {
                host::HostEvent::Log(msg) => eprintln!("[host] {msg}"),
                host::HostEvent::StateChanged(s) => eprintln!("[state] {s:?}"),
                host::HostEvent::Registered { request_pk } => {
                    eprintln!("[host] Registered request_pk={request_pk}")
                }
                host::HostEvent::IncomingRequest { peer_id, .. } => {
                    eprintln!("[host] Incoming from {peer_id}")
                }
                host::HostEvent::ApprovalRequested { peer_id } => {
                    eprintln!(
                        "[host] Approval requested from {peer_id}; GUI confirmation is required"
                    )
                }
                host::HostEvent::SessionStarted { peer_id } => {
                    eprintln!("[host] Session started: {peer_id}")
                }
                host::HostEvent::SessionEnded { peer_id, reason } => {
                    eprintln!("[host] Session ended: {peer_id} {reason}")
                }
                host::HostEvent::VideoTelemetry {
                    summary,
                    fallback_reason,
                } => {
                    if let Some(reason) = fallback_reason {
                        eprintln!("[host-video] {summary}; fallback={reason}");
                    } else {
                        eprintln!("[host-video] {summary}");
                    }
                }
            }
        }
    }
}

fn egui_software_backend_active() -> bool {
    std::env::var_os("EVERTYDESK_EGUI_SOFTWARE").is_some()
}

fn commercial_ui_enabled() -> bool {
    std::env::var_os("EVERTYDESK_CLASSIC_UI").is_none()
}

fn run_cli_connect() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    let command = args.next()?;
    if command == "--online" {
        let remote_id = normalize_remote_id(&args.next().unwrap_or_default());
        if remote_id.is_empty() {
            eprintln!("Usage: evertydesk-lite --online <remote-id>");
            return Some(2);
        }
        let config = AppConfig::load_or_create();
        return match TransportClient::query_peer_online(
            &config.server,
            &config.local_id,
            &remote_id,
        ) {
            Ok(true) => {
                println!("{remote_id}: online");
                Some(0)
            }
            Ok(false) => {
                println!("{remote_id}: offline");
                Some(3)
            }
            Err(err) => {
                eprintln!("Error: {err}");
                Some(1)
            }
        };
    }

    if command == "--host" {
        // Headless host mode: start the host service, stream all events to
        // stderr.  Useful for diagnosing registration issues without the GUI.
        //
        // Optional flags (after --host):
        //   --bind-port PORT   Bind UDP socket to PORT instead of random.
        //                      Use the port the EvertyDesk service was using
        //                      (e.g. 63624) after stopping the service.
        //   --use-everty-keys  Read the installed EvertyDesk's Ed25519 key
        //                      pair and use it for hbbs RegisterPk.
        let mut config = AppConfig::load_or_create();
        // Parse extra flags
        let extra_args: Vec<String> = args.collect();
        for extra_arg in &extra_args {
            match extra_arg.as_str() {
                "--use-everty-keys" => {
                    if let Some(pk) = load_everty_public_key() {
                        eprintln!("[cli] Loaded real Ed25519 public key from EvertyDesk config");
                        config.host_pk = pk;
                    } else {
                        eprintln!("[cli] Warning: could not load EvertyDesk key pair");
                    }
                }
                s if s.starts_with("--bind-port") => {
                    // --bind-port PORT  or  --bind-port=PORT
                    let port_str = if let Some(p) = s.strip_prefix("--bind-port=") {
                        p.to_owned()
                    } else {
                        // try next element in extra_args (not supported easily, use = form)
                        String::new()
                    };
                    if let Ok(p) = port_str.parse::<u16>() {
                        eprintln!("[cli] UDP bind port: {p}");
                        config.udp_bind_port = p;
                    }
                }
                _ => {}
            }
        }
        let config = config; // freeze
        eprintln!("[cli] Starting host service (headless). Press Ctrl-C to stop.");
        eprintln!(
            "[cli] local_id={} server={}",
            config.local_id, config.server.id_server
        );
        let svc = host::HostService::start(config);
        loop {
            std::thread::sleep(Duration::from_millis(100));
            while let Some(ev) = svc.try_recv() {
                match &ev {
                    host::HostEvent::Log(msg) => eprintln!("[host] {msg}"),
                    host::HostEvent::StateChanged(s) => eprintln!("[state] {:?}", s),
                    host::HostEvent::Registered { request_pk } => {
                        eprintln!("[host] Registered request_pk={request_pk}")
                    }
                    host::HostEvent::IncomingRequest { peer_id, .. } => {
                        eprintln!("[host] Incoming from {peer_id}")
                    }
                    host::HostEvent::ApprovalRequested { peer_id } => {
                        eprintln!("[host] Approval requested from {peer_id}; GUI confirmation is required")
                    }
                    host::HostEvent::SessionStarted { peer_id } => {
                        eprintln!("[host] Session started: {peer_id}")
                    }
                    host::HostEvent::SessionEnded { peer_id, reason } => {
                        eprintln!("[host] Session ended: {peer_id} {reason}")
                    }
                    host::HostEvent::VideoTelemetry {
                        summary,
                        fallback_reason,
                    } => {
                        if let Some(reason) = fallback_reason {
                            eprintln!("[host-video] {summary}; fallback={reason}");
                        } else {
                            eprintln!("[host-video] {summary}");
                        }
                    }
                }
            }
        }
    }

    if command != "--connect" {
        return None;
    }

    let remote_id = normalize_remote_id(&args.next().unwrap_or_default());
    let password = args.next().unwrap_or_default();
    if remote_id.is_empty() {
        eprintln!("Usage: evertydesk-lite --connect <remote-id> [password]");
        return Some(2);
    }

    let config = AppConfig::load_or_create();
    if is_own_remote_id(&remote_id, &config.local_id) {
        eprintln!("Error: remote ID equals this computer ID ({remote_id})");
        return Some(2);
    }
    let request = ConnectionRequest {
        remote_id,
        password,
        server: config.server,
        display: config.display,
    };

    match TransportClient::connect_with_progress(request, |pct, message| {
        println!("{pct}% - {message}");
    }) {
        Ok(state) => {
            println!("OK: {}", state.as_text());
            Some(0)
        }
        Err(err) => {
            eprintln!("Error: {err}");
            Some(1)
        }
    }
}

struct EvertyDeskApp {
    config: AppConfig,
    remote_id: String,
    password: String,
    show_password: bool,
    show_host_password: bool,
    connect_kind: ConnectKind,
    shell_window_open: bool,
    shell_output: String,
    shell_input: String,
    shell_history: Vec<String>,
    shell_history_pos: Option<usize>,
    shell_last_command: String,
    terminal_goal: String,
    terminal_ai_answer: String,
    terminal_ai_status: Option<String>,
    terminal_ai_rx: Option<Receiver<Result<String, String>>>,
    terminal_auto_pending: bool,
    terminal_auto_request_at: Option<Instant>,
    mode: AppMode,
    ui_lang: UiLang,
    new_contact_name: String,
    new_contact_id: String,
    new_contact_note: String,
    contact_search: String,
    address_book_status: Option<String>,
    show_address_book_auth: bool,
    show_new_contact_dialog: bool,
    selected_contact_idx: Option<usize>,
    contact_details_draft: Option<ContactEntry>,
    service_status: Option<String>,
    status: String,
    host_status: String,
    host_video_status: Option<String>,
    last_error: Option<String>,
    connection_state: ConnectionState,
    worker: Option<Receiver<WorkerEvent>>,
    session_tx: Option<mpsc::Sender<SessionCommand>>,
    busy: bool,
    host_check_busy: bool,
    remote_check_busy: bool,
    connected: bool,
    remote_viewer_open: bool,
    remote_fullscreen: bool,
    progress: u8,
    events: Vec<String>,
    session_log: Vec<String>,
    remote_texture: Option<TextureHandle>,
    app_logo_texture: Option<TextureHandle>,
    pending_image: Option<ColorImage>,
    last_frame_rgba: Vec<u8>,
    remote_size: [usize; 2],
    text_to_send: String,
    clipboard_status: Option<String>,
    screenshot_status: Option<String>,
    log_status: Option<String>,
    report_status: Option<String>,
    remote_input_focused: bool,
    remote_modifiers_down: RemoteModifierState,
    last_mouse_pos: Option<(i32, i32)>,
    remote_displays: Vec<RemoteDisplay>,
    selected_display: i32,
    auto_refresh: bool,
    refresh_millis: u64,
    video_fps: i32,
    fit_to_window: bool,
    coordinate_mode: CoordinateMode,
    screenshot_count: u64,
    live_frame_count: u64,
    screenshot_frame_count: u64,
    screenshot_pending: bool,
    last_screenshot_at: Option<Instant>,
    last_screenshot_sid: String,
    last_frame_codec: String,
    input_events_sent: u64,
    last_move_refresh_at: Option<Instant>,
    fps_last_at: Instant,
    fps_last_count: u64,
    display_fps: f32,
    stream_input_fps: f32,
    stream_input_kbps: u64,
    frame_bytes: usize,
    frame_queue_ms: u64,
    frame_decode_ms: u64,
    frame_dropped: usize,
    last_stream_tune_at: Option<Instant>,
    png_fallback_started_at: Option<Instant>,
    /// When the last live (VP9/H264) frame was received.
    /// Used to suppress PNG screenshot frames while live video is active.
    last_live_frame_at: Option<Instant>,
    stream_health: String,
    wheel_accum: egui::Vec2,
    /// Cache of remote cursor images by cursor ID (RGBA + hotspot).
    cursor_cache: HashMap<u64, CursorCacheEntry>,
    /// Pending cursor image to be loaded into a texture in the next frame.
    pending_cursor: Option<(u64, egui::ColorImage, i32, i32)>,
    /// Currently active cursor texture + hotspot offset.
    cursor_texture: Option<(egui::TextureHandle, i32, i32)>,
    /// Last known cursor position in remote-screen coordinates.
    cursor_pos: Option<egui::Pos2>,
    /// Last measured round-trip latency (ms) from TestDelay.
    latency_ms: Option<u32>,

    // ── Host service ──────────────────────────────────────────────────────────
    /// Background host service (None = stopped).
    host_service: Option<HostService>,
    /// Last known host-service state.
    host_state: HostState,
    /// Log lines received from the host service.
    host_log: Vec<String>,
    /// Pending incoming connection (peer_id) waiting for Accept/Reject.
    host_pending_peer: Option<String>,

    // ── Settings window ───────────────────────────────────────────────────────
    /// Whether the settings panel is visible.
    show_settings: bool,
    /// Editable copy of config while the settings window is open.
    settings_draft: Option<AppConfig>,
}

struct CursorCacheEntry {
    image: Option<egui::ColorImage>,
    texture: Option<egui::TextureHandle>,
    hotx: i32,
    hoty: i32,
}

enum WorkerEvent {
    Session(SessionEvent),
    HostServerCheck(Result<(), String>),
    RemoteOnlineCheck {
        remote_id: String,
        result: Result<bool, String>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RemoteModifierState {
    alt: bool,
    shift: bool,
    ctrl: bool,
    meta: bool,
}

impl RemoteModifierState {
    fn from_egui(modifiers: egui::Modifiers) -> Self {
        Self {
            alt: modifiers.alt,
            shift: modifiers.shift,
            ctrl: modifiers.ctrl,
            meta: modifiers.mac_cmd,
        }
    }

    fn for_each(self, mut f: impl FnMut(ControlKey, bool)) {
        f(ControlKey::Control, self.ctrl);
        f(ControlKey::Alt, self.alt);
        f(ControlKey::Shift, self.shift);
        f(ControlKey::Meta, self.meta);
    }
}

fn forward_session_events(
    session_rx: Receiver<SessionEvent>,
    ui_events: mpsc::Sender<WorkerEvent>,
) {
    while let Ok(first) = session_rx.recv() {
        let mut latest_frame = None;
        let mut terminal = false;
        forward_or_coalesce_session_event(first, &ui_events, &mut latest_frame, &mut terminal);

        loop {
            match session_rx.try_recv() {
                Ok(event) => {
                    forward_or_coalesce_session_event(
                        event,
                        &ui_events,
                        &mut latest_frame,
                        &mut terminal,
                    );
                    if terminal {
                        break;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    terminal = true;
                    break;
                }
            }
        }

        if let Some(frame) = latest_frame {
            if !terminal {
                let _ = ui_events.send(WorkerEvent::Session(frame));
            }
        }
        if terminal {
            break;
        }
    }
}

fn forward_or_coalesce_session_event(
    event: SessionEvent,
    ui_events: &mpsc::Sender<WorkerEvent>,
    latest_frame: &mut Option<SessionEvent>,
    terminal: &mut bool,
) {
    if matches!(event, SessionEvent::Frame { .. }) {
        *latest_frame = Some(event);
        return;
    }
    *terminal = matches!(event, SessionEvent::Closed | SessionEvent::Failed(_));
    let _ = ui_events.send(WorkerEvent::Session(event));
}

impl EvertyDeskApp {
    fn new() -> Self {
        let config = AppConfig::load_or_create();
        let remote_id = config.ui.last_remote_id.clone();
        let auto_refresh = config.ui.auto_refresh;
        let refresh_millis = config.ui.refresh_millis.clamp(50, 2000).min(80);
        let fit_to_window = config.ui.fit_to_window;
        let coordinate_mode = config.ui.coordinate_mode;
        let video_fps = config.display.target_fps.clamp(5, 60) as i32;
        let host_service = Some(HostService::start(config.clone()));
        Self {
            config,
            remote_id,
            password: String::new(),
            show_password: false,
            show_host_password: false,
            connect_kind: ConnectKind::Screen,
            shell_window_open: false,
            shell_output: String::new(),
            shell_input: String::new(),
            shell_history: Vec::new(),
            shell_history_pos: None,
            shell_last_command: String::new(),
            terminal_goal: String::new(),
            terminal_ai_answer: String::new(),
            terminal_ai_status: None,
            terminal_ai_rx: None,
            terminal_auto_pending: false,
            terminal_auto_request_at: None,
            mode: AppMode::Connect,
            ui_lang: UiLang::Ru,
            new_contact_name: String::new(),
            new_contact_id: String::new(),
            new_contact_note: String::new(),
            contact_search: String::new(),
            address_book_status: None,
            show_address_book_auth: false,
            show_new_contact_dialog: false,
            selected_contact_idx: None,
            contact_details_draft: None,
            service_status: None,
            host_status: "Доступ запускается автоматически.".to_owned(),
            host_video_status: None,
            status: "Готово".to_owned(),
            last_error: None,
            connection_state: ConnectionState::Idle,
            worker: None,
            session_tx: None,
            busy: false,
            host_check_busy: false,
            remote_check_busy: false,
            connected: false,
            remote_viewer_open: false,
            remote_fullscreen: false,
            progress: 0,
            events: vec!["App started".to_owned()],
            session_log: vec!["App started".to_owned()],
            remote_texture: None,
            app_logo_texture: None,
            pending_image: None,
            last_frame_rgba: Vec::new(),
            remote_size: [0, 0],
            text_to_send: String::new(),
            clipboard_status: None,
            screenshot_status: None,
            log_status: None,
            report_status: None,
            remote_input_focused: false,
            remote_modifiers_down: RemoteModifierState::default(),
            last_mouse_pos: None,
            remote_displays: Vec::new(),
            selected_display: 0,
            auto_refresh,
            refresh_millis,
            video_fps,
            fit_to_window,
            coordinate_mode,
            screenshot_count: 0,
            live_frame_count: 0,
            screenshot_frame_count: 0,
            screenshot_pending: false,
            last_screenshot_at: None,
            last_screenshot_sid: String::new(),
            last_frame_codec: "none".to_owned(),
            input_events_sent: 0,
            last_move_refresh_at: None,
            fps_last_at: Instant::now(),
            fps_last_count: 0,
            display_fps: 0.0,
            stream_input_fps: 0.0,
            stream_input_kbps: 0,
            frame_bytes: 0,
            frame_queue_ms: 0,
            frame_decode_ms: 0,
            frame_dropped: 0,
            last_stream_tune_at: None,
            png_fallback_started_at: None,
            last_live_frame_at: None,
            stream_health: "ожидание кадра".to_owned(),
            wheel_accum: egui::Vec2::ZERO,
            cursor_cache: HashMap::new(),
            pending_cursor: None,
            cursor_texture: None,
            cursor_pos: None,
            latency_ms: None,
            host_service,
            host_state: HostState::Connecting,
            host_log: vec![format!("[{}] Автозапуск доступа...", timestamp_hms())],
            host_pending_peer: None,
            show_settings: false,
            settings_draft: None,
        }
    }

    fn connect(&mut self) {
        let normalized_remote_id = normalize_remote_id(&self.remote_id);
        if is_own_remote_id(&normalized_remote_id, &self.config.local_id) {
            self.set_error("Нельзя подключиться к своему же ID. Откройте EvertyDesk на другом ПК и введите его ID.");
            return;
        }
        let request = ConnectionRequest {
            remote_id: normalized_remote_id.clone(),
            password: self.password.clone(),
            server: self.config.server.clone(),
            display: self.config.display.clone(),
        };

        if request.remote_id.is_empty() {
            self.set_error("Введите ID удаленного ПК");
            return;
        }
        if false && request.password.is_empty() {
            self.set_error("Введите пароль");
            return;
        }

        self.last_error = None;
        self.busy = true;
        self.connected = false;
        self.remote_viewer_open = false;
        self.shell_window_open = false;
        self.remote_fullscreen = false;
        self.remote_id = normalized_remote_id;
        self.save_ui_config();
        self.remote_texture = None;
        self.remote_size = [0, 0];
        self.last_frame_rgba.clear();
        self.clipboard_status = None;
        self.screenshot_status = None;
        self.log_status = None;
        self.report_status = None;
        self.session_log.clear();
        self.log(format!("Session started for {}", self.remote_id));
        self.remote_displays.clear();
        self.screenshot_count = 0;
        self.live_frame_count = 0;
        self.screenshot_frame_count = 0;
        self.screenshot_pending = false;
        self.last_screenshot_at = None;
        self.last_screenshot_sid.clear();
        self.last_frame_codec = "none".to_owned();
        self.video_fps = self.config.display.target_fps.clamp(5, 60) as i32;
        self.frame_bytes = 0;
        self.frame_queue_ms = 0;
        self.frame_decode_ms = 0;
        self.frame_dropped = 0;
        self.last_stream_tune_at = None;
        self.png_fallback_started_at = None;
        self.last_live_frame_at = None;
        self.stream_health = "ожидание кадра".to_owned();
        self.input_events_sent = 0;
        self.last_move_refresh_at = None;
        self.fps_last_at = Instant::now();
        self.fps_last_count = 0;
        self.display_fps = 0.0;
        self.stream_input_fps = 0.0;
        self.stream_input_kbps = 0;
        self.wheel_accum = egui::Vec2::ZERO;
        self.selected_display = 0;
        self.remote_input_focused = false;
        self.remote_modifiers_down = RemoteModifierState::default();
        self.progress = 1;
        self.status = format!("Подключение к {}", request.remote_id);
        self.log(self.status.clone());

        let (ui_tx, rx) = mpsc::channel();
        let (command_tx, command_rx) = mpsc::channel();
        self.session_tx = Some(command_tx);
        thread::spawn(move || {
            let (session_tx, session_rx) = mpsc::channel();
            let ui_events = ui_tx.clone();
            thread::spawn(move || {
                TransportClient::run_session(request, command_rx, session_tx);
            });
            forward_session_events(session_rx, ui_events);
        });
        self.worker = Some(rx);
    }

    fn poll_worker(&mut self) {
        let Some(rx) = self.worker.take() else {
            return;
        };

        let mut latest_frame = None;
        loop {
            match rx.try_recv() {
                Ok(WorkerEvent::Session(event)) => {
                    if matches!(event, SessionEvent::Frame { .. }) {
                        latest_frame = Some(event);
                        continue;
                    }
                    let terminal = matches!(event, SessionEvent::Failed(_) | SessionEvent::Closed);
                    self.handle_session_event(event);
                    if terminal {
                        return;
                    }
                }
                Ok(WorkerEvent::HostServerCheck(result)) => {
                    self.host_check_busy = false;
                    match result {
                        Ok(()) => {
                            self.host_status = "ID server доступен. Следующий этап: регистрация этого ПК и прием relay-сессии.".to_owned();
                            self.log("Host check: ID server reachable".to_owned());
                        }
                        Err(err) => {
                            self.host_status = format!("ID server недоступен: {err}");
                            self.log(format!("Host check failed: {err}"));
                        }
                    }
                }
                Ok(WorkerEvent::RemoteOnlineCheck { remote_id, result }) => {
                    self.remote_check_busy = false;
                    match result {
                        Ok(true) => {
                            self.progress = 100;
                            self.status = format!("{remote_id}: онлайн");
                            self.last_error = None;
                            self.log(format!("Online check: {remote_id} is online"));
                        }
                        Ok(false) => {
                            self.progress = 0;
                            self.status = format!("{remote_id}: не в сети на этом ID server");
                            self.last_error = Some(self.status.clone());
                            self.log(format!("Online check: {remote_id} is offline"));
                        }
                        Err(err) => {
                            self.progress = 0;
                            self.status = format!("Проверка ID не удалась: {err}");
                            self.last_error = Some(self.status.clone());
                            self.log(format!("Online check failed: {err}"));
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if let Some(frame) = latest_frame {
                        self.handle_session_event(frame);
                    }
                    self.worker = Some(rx);
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    if let Some(frame) = latest_frame {
                        self.handle_session_event(frame);
                    }
                    self.busy = false;
                    self.host_check_busy = false;
                    self.remote_check_busy = false;
                    if self.connected {
                        self.set_error("Background task stopped unexpectedly");
                    } else {
                        self.worker = None;
                    }
                    return;
                }
            }
        }
    }

    fn handle_session_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Progress(pct, message) => {
                self.progress = pct;
                self.status = format!("{pct}% - {message}");
                self.log(self.status.clone());
            }
            SessionEvent::Connected(info) => {
                self.busy = false;
                self.connected = true;
                self.remote_viewer_open = self.connect_kind == ConnectKind::Screen;
                self.shell_window_open = self.connect_kind == ConnectKind::Shell;
                self.progress = 100;
                self.connection_state = ConnectionState::RelayReady {
                    remote_id: self.remote_id.clone(),
                };
                self.status = "Подключено".to_owned();
                self.log(format!("Connected: {info}"));
                if self.connect_kind == ConnectKind::Shell {
                    self.shell_output.clear();
                    self.send_command(SessionCommand::ShellStart);
                } else {
                    self.send_command(SessionCommand::SetAutoRefresh {
                        enabled: self.auto_refresh,
                        millis: self.refresh_millis,
                    });
                }
            }
            SessionEvent::Frame {
                sid,
                codec,
                width,
                height,
                rgba,
            } => {
                if codec == "PNG" {
                    // Discard PNG screenshot frames while live video (VP9/H264) is
                    // actively streaming — they only cause codec badge flicker and
                    // would replace a fresher VP9 frame with an older screenshot.
                    // RustDesk never mixes live video with screenshot mode.
                    let live_active = self
                        .last_live_frame_at
                        .map(|t| t.elapsed() < Duration::from_secs(2))
                        .unwrap_or(false);
                    if live_active {
                        return;
                    }
                    self.screenshot_frame_count += 1;
                    if self.png_fallback_started_at.is_none() {
                        self.png_fallback_started_at = Some(Instant::now());
                    }
                } else {
                    self.live_frame_count += 1;
                    self.last_live_frame_at = Some(Instant::now());
                    self.png_fallback_started_at = None;
                }
                self.update_render_fps();
                self.last_frame_rgba = rgba;
                let image =
                    ColorImage::from_rgba_unmultiplied([width, height], &self.last_frame_rgba);
                self.remote_size = image.size;
                self.pending_image = Some(image);
                self.last_screenshot_sid = sid;
                self.last_frame_codec = codec;
                self.last_screenshot_at = Some(Instant::now());
                if self.screenshot_count <= 1 || self.screenshot_count % 20 == 0 {
                    self.log(format!(
                        "Frame received: {} ({})",
                        self.last_screenshot_sid, self.last_frame_codec
                    ));
                }
            }
            SessionEvent::ScreenshotStats { received, pending } => {
                self.screenshot_count = received;
                self.screenshot_pending = pending;
            }
            SessionEvent::FrameMetrics {
                bytes,
                queue_ms,
                decode_ms,
                dropped,
            } => {
                self.frame_bytes = bytes;
                self.frame_queue_ms = queue_ms;
                self.frame_decode_ms = decode_ms;
                self.frame_dropped = dropped;
                self.auto_tune_stream();
            }
            SessionEvent::VideoPacketMetrics {
                input_fps,
                input_kbps,
            } => {
                self.stream_input_fps = input_fps;
                self.stream_input_kbps = input_kbps;
                if self.last_frame_codec != "PNG" && input_fps > 0.1 {
                    if input_fps < 8.0 {
                        self.stream_health = "низкий входящий поток: хост/сеть".to_owned();
                    } else if self.display_fps + 3.0 < input_fps * 0.6 {
                        self.stream_health = "декодер/отрисовка отстаёт".to_owned();
                    }
                }
            }
            SessionEvent::Displays(displays) => {
                self.remote_displays = displays;
                if !self
                    .remote_displays
                    .iter()
                    .any(|display| display.index == self.selected_display)
                {
                    self.selected_display = self
                        .remote_displays
                        .first()
                        .map(|display| display.index)
                        .unwrap_or_default();
                }
                if self.connected {
                    if let Some(display) = self
                        .remote_displays
                        .iter()
                        .find(|display| display.index == self.selected_display)
                        .cloned()
                    {
                        self.send_command(SessionCommand::SetDisplay(display));
                    }
                }
                self.log(format!("Displays detected: {}", self.remote_displays.len()));
            }
            SessionEvent::CursorData {
                id,
                hotx,
                hoty,
                width,
                height,
                rgba,
            } => {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [width as usize, height as usize],
                    &rgba,
                );
                self.cursor_cache.insert(
                    id,
                    CursorCacheEntry {
                        image: Some(image.clone()),
                        texture: None,
                        hotx,
                        hoty,
                    },
                );
                self.pending_cursor = Some((id, image, hotx, hoty));
            }
            SessionEvent::CursorId { id } => {
                if let Some(entry) = self.cursor_cache.get(&id) {
                    if let Some(texture) = entry.texture.clone() {
                        self.cursor_texture = Some((texture, entry.hotx, entry.hoty));
                    } else if let Some(image) = entry.image.clone() {
                        self.pending_cursor = Some((id, image, entry.hotx, entry.hoty));
                    }
                }
            }
            SessionEvent::CursorPosition { x, y } => {
                self.cursor_pos = Some(egui::pos2(x as f32, y as f32));
            }
            SessionEvent::Latency(ms) => {
                self.latency_ms = Some(ms);
            }
            SessionEvent::ShellOutput(text) => {
                self.shell_output.push_str(&text);
                self.trim_shell_output();
                if self.terminal_auto_pending {
                    self.terminal_auto_request_at = Some(Instant::now());
                }
            }
            SessionEvent::ShellClosed => {
                self.shell_output.push_str("\r\n[console closed]\r\n");
            }
            SessionEvent::ShellError(err) => {
                self.shell_output
                    .push_str(&format!("\r\n[console error] {err}\r\n"));
            }
            SessionEvent::Info(message) => self.log(message),
            SessionEvent::Failed(err) => {
                self.busy = false;
                self.connected = false;
                self.remote_viewer_open = false;
                self.remote_fullscreen = false;
                self.session_tx = None;
                self.connection_state = ConnectionState::Failed(err.clone());
                self.remote_modifiers_down = RemoteModifierState::default();
                self.last_error = Some(err.clone());
                self.status = friendly_error(&err);
                self.log(format!("Error: {err}"));
            }
            SessionEvent::Closed => {
                self.busy = false;
                self.connected = false;
                self.remote_viewer_open = false;
                self.remote_fullscreen = false;
                self.session_tx = None;
                self.remote_input_focused = false;
                self.remote_modifiers_down = RemoteModifierState::default();
                self.screenshot_pending = false;
                self.status = "Отключено".to_owned();
                self.log(self.status.clone());
            }
        }
    }

    fn send_command(&mut self, command: SessionCommand) {
        if let Some(tx) = &self.session_tx {
            let is_input = command_is_input(&command);
            if tx.send(command).is_err() {
                self.set_error("Session command channel is closed");
            } else if is_input {
                self.input_events_sent += 1;
            }
        }
    }

    fn disconnect_session(&mut self, status: &str) {
        self.release_remote_modifiers();
        if let Some(tx) = self.session_tx.take() {
            let _ = tx.send(SessionCommand::Close);
        }
        self.busy = false;
        self.connected = false;
        self.remote_viewer_open = false;
        self.remote_fullscreen = false;
        self.remote_texture = None;
        self.pending_image = None;
        self.last_frame_rgba.clear();
        self.remote_input_focused = false;
        self.remote_modifiers_down = RemoteModifierState::default();
        self.screenshot_pending = false;
        self.last_mouse_pos = None;
        self.last_move_refresh_at = None;
        self.wheel_accum = egui::Vec2::ZERO;
        self.progress = 0;
        self.live_frame_count = 0;
        self.screenshot_frame_count = 0;
        self.last_frame_codec = "none".to_owned();
        self.frame_bytes = 0;
        self.frame_queue_ms = 0;
        self.frame_decode_ms = 0;
        self.frame_dropped = 0;
        self.fps_last_at = Instant::now();
        self.fps_last_count = 0;
        self.display_fps = 0.0;
        self.stream_input_fps = 0.0;
        self.stream_input_kbps = 0;
        self.last_stream_tune_at = None;
        self.png_fallback_started_at = None;
        self.last_live_frame_at = None;
        self.stream_health = "отключено".to_owned();
        self.screenshot_status = None;
        self.log_status = None;
        self.report_status = None;
        self.cursor_cache.clear();
        self.pending_cursor = None;
        self.cursor_texture = None;
        self.cursor_pos = None;
        self.latency_ms = None;
        self.status = status.to_owned();
        self.log(status.to_owned());
    }

    fn visible_status(&self) -> String {
        if self.busy {
            return "Подключение...".to_owned();
        }
        if let Some(error) = &self.last_error {
            return friendly_error(error);
        }
        if self.connected {
            return "Подключено".to_owned();
        }
        self.status.clone()
    }

    fn update_render_fps(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.fps_last_at);
        if elapsed >= Duration::from_millis(750) {
            let received = self.live_frame_count + self.screenshot_frame_count;
            let frames = received.saturating_sub(self.fps_last_count);
            self.display_fps = frames as f32 / elapsed.as_secs_f32();
            self.fps_last_at = now;
            self.fps_last_count = received;
        }
    }

    fn auto_tune_stream(&mut self) {
        let cooldown_ready = self
            .last_stream_tune_at
            .map(|instant| instant.elapsed() >= Duration::from_secs(3))
            .unwrap_or(true);

        if self.last_frame_codec == "PNG" {
            let fallback_ms = self
                .png_fallback_started_at
                .map(|instant| instant.elapsed().as_millis() as u64)
                .unwrap_or_default();
            self.stream_health = if fallback_ms >= 2_000 {
                "PNG fallback: запрашиваем live video".to_owned()
            } else {
                "PNG fallback".to_owned()
            };
            if fallback_ms >= 2_000 && cooldown_ready {
                self.last_stream_tune_at = Some(Instant::now());
                self.send_command(SessionCommand::RefreshVideo);
                self.log("Auto tune: PNG fallback persists; requested live video".to_owned());
            }
            return;
        }

        if self.last_frame_codec == "none" {
            self.stream_health = "ожидание кадра".to_owned();
            return;
        }

        let queue_lag = self.frame_queue_ms >= 450 || self.frame_dropped >= 3;
        let decode_lag = self.frame_decode_ms >= 45;
        if queue_lag || decode_lag {
            self.stream_health = if queue_lag {
                "очередь кадров: догоняем поток".to_owned()
            } else {
                "декодер перегружен".to_owned()
            };
            if cooldown_ready {
                self.last_stream_tune_at = Some(Instant::now());
                let next_fps = match self.video_fps {
                    fps if fps > 20 => 20,
                    fps if fps > 15 => 15,
                    fps => fps,
                };
                if next_fps != self.video_fps {
                    self.video_fps = next_fps;
                    self.send_command(SessionCommand::SetVideoFps { fps: next_fps });
                    self.log(format!("Auto tune: video fps lowered to {next_fps}"));
                } else {
                    self.send_command(SessionCommand::RefreshVideo);
                    self.log("Auto tune: requested fresh live video stream".to_owned());
                }
            }
            return;
        }

        self.stream_health = "live поток стабилен".to_owned();
    }

    fn save_ui_config(&mut self) {
        self.config.ui.last_remote_id = self.remote_id.clone();
        remember_remote_id(&mut self.config.ui.recent_remote_ids, &self.remote_id);
        remember_history(&mut self.config.ui.history, &self.remote_id);
        self.config.ui.auto_refresh = self.auto_refresh;
        self.config.ui.refresh_millis = self.refresh_millis;
        self.config.ui.fit_to_window = self.fit_to_window;
        self.config.ui.coordinate_mode = self.coordinate_mode;
        self.config.save();
    }

    fn set_error(&mut self, message: &str) {
        self.last_error = Some(message.to_owned());
        self.status = friendly_error(message);
        self.connection_state = ConnectionState::Failed(message.to_owned());
        self.log(format!("Error: {message}"));
    }

    fn log(&mut self, message: String) {
        let line = format!("{}  {message}", unix_timestamp_secs());
        self.session_log.push(line.clone());
        self.events.push(line);
        if self.events.len() > 80 {
            self.events.remove(0);
        }
    }
}

impl eframe::App for EvertyDeskApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.update_egui(ui.ctx());
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shutdown();
    }
}

impl EvertyDeskApp {
    #[allow(deprecated)]
    fn update_egui(&mut self, ctx: &egui::Context) {
        self.poll_worker();
        self.poll_terminal_ai();
        self.maybe_request_terminal_auto_ai();
        self.poll_host_service();
        if let Some(image) = self.pending_image.take() {
            if let Some(texture) = self.remote_texture.as_mut() {
                texture.set(image, remote_texture_options());
            } else {
                self.remote_texture =
                    Some(ctx.load_texture("remote-screen", image, remote_texture_options()));
            }
            ctx.request_repaint();
        }
        if let Some((id, image, hotx, hoty)) = self.pending_cursor.take() {
            let texture = ctx.load_texture(
                format!("remote-cursor-{id}"),
                image,
                egui::TextureOptions::NEAREST,
            );
            self.cursor_texture = Some((texture.clone(), hotx, hoty));
            if let Some(entry) = self.cursor_cache.get_mut(&id) {
                entry.texture = Some(texture);
                entry.image = None;
            }
        }
        if self.busy
            || self.connected
            || self.host_check_busy
            || self.remote_check_busy
            || self.terminal_ai_rx.is_some()
            || self.host_state.is_online()
        {
            ctx.request_repaint_after(Duration::from_millis(33));
        }

        let software_backend = egui_software_backend_active();
        if software_backend && self.connected && self.remote_viewer_open {
            self.remote_viewer_inline(ctx);
            if self.show_settings {
                self.settings_window(ctx);
            }
            return;
        }

        if self.connected && self.remote_viewer_open {
            self.remote_viewer_window(ctx);
        }
        if self.connected && self.shell_window_open {
            self.shell_window(ctx);
        }
        if self.host_pending_peer.is_some() {
            self.incoming_approval_window(ctx);
        }

        let screen_rect = ctx.content_rect();
        ctx.layer_painter(egui::LayerId::background()).rect_filled(
            screen_rect,
            egui::CornerRadius::ZERO,
            egui::Color32::from_rgb(0xFB, 0xFC, 0xFE),
        );

        // ── Left sidebar: logo · navigation · settings ───────────────────────
        egui::Panel::left("everty_sidebar")
            .resizable(false)
            .exact_size(220.0)
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(0xF7, 0xF8, 0xFA))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgb(0xEA, 0xEC, 0xF0),
                    ))
                    .corner_radius(egui::CornerRadius::ZERO)
                    .inner_margin(egui::Margin::symmetric(18, 20))
                    .outer_margin(egui::Margin::ZERO),
            )
            .show(ctx, |ui| self.sidebar(ui));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(0xFB, 0xFC, 0xFE))
                    .inner_margin(egui::Margin {
                        left: 26,
                        right: 24,
                        top: 28,
                        bottom: 24,
                    }),
            )
            .show(ctx, |ui| match self.mode {
                AppMode::Connect => self.connect_ui(ui),
                AppMode::Host => self.host_ui(ui),
                AppMode::History => self.history_ui(ui),
                AppMode::Contacts => self.contacts_ui(ui),
                AppMode::Settings => self.settings_ui(ui),
            });
        if self.mode == AppMode::Connect
            && !self.busy
            && !self.connected
            && ctx.input(|input| input.key_pressed(egui::Key::Enter))
        {
            self.connect();
        }
    }

    fn shutdown(&mut self) {
        if let Some(svc) = &self.host_service {
            svc.stop();
        }
        if self.connected || self.busy {
            self.disconnect_session("Application closed");
        }
    }

    fn text(&self, ru: &'static str, en: &'static str) -> &'static str {
        tr(self.ui_lang, ru, en)
    }

    fn host_state_text(&self) -> &'static str {
        match (&self.host_state, self.ui_lang) {
            (HostState::Idle, UiLang::Ru) => "Остановлен",
            (HostState::Idle, UiLang::En) => "Stopped",
            (HostState::Connecting, UiLang::Ru) => "Подключение...",
            (HostState::Connecting, UiLang::En) => "Connecting...",
            (HostState::Ready, UiLang::Ru) => "Готов к подключению",
            (HostState::Ready, UiLang::En) => "Ready to connect",
            (HostState::Accepting(_), UiLang::Ru) => "Сессия активна",
            (HostState::Accepting(_), UiLang::En) => "Session active",
            (HostState::Error(_), UiLang::Ru) => "Ошибка",
            (HostState::Error(_), UiLang::En) => "Error",
        }
    }

    fn ensure_app_logo_texture(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        if self.app_logo_texture.is_none() {
            let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/edesk_lite_logo.png"));
            if let Ok(img) = image::load_from_memory(bytes) {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let image = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                self.app_logo_texture =
                    Some(ctx.load_texture("evertydesk-logo", image, egui::TextureOptions::LINEAR));
            }
        }
        self.app_logo_texture.as_ref()
    }

    fn connect_ui(&mut self, ui: &mut egui::Ui) {
        if commercial_ui_enabled() {
            self.connect_ui_commercial(ui);
            return;
        }
        ui.add_space(6.0);
        ui.label("ID удаленного ПК");
        let remote_id_response = ui.add_enabled(
            !self.connected && !self.busy,
            egui::TextEdit::singleline(&mut self.remote_id).desired_width(f32::INFINITY),
        );
        ui.add_space(8.0);
        ui.label("Пароль");
        let password_response = ui.add_enabled(
            !self.connected && !self.busy,
            egui::TextEdit::singleline(&mut self.password)
                .password(!self.show_password)
                .desired_width(f32::INFINITY),
        );
        ui.small(
            "Можно оставить пустым, если удаленный RustDesk разрешает подтверждение без пароля.",
        );
        ui.checkbox(&mut self.show_password, "Показать пароль");
        if remote_id_response.changed() || password_response.changed() {
            self.last_error = None;
            if !self.connected && !self.busy {
                self.status = "Готово".to_owned();
                self.progress = 0;
            }
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.busy && !self.connected,
                    egui::Button::new("Подключиться").min_size(egui::vec2(150.0, 32.0)),
                )
                .clicked()
            {
                self.connect();
            }
            if ui
                .add_enabled(
                    !self.busy && !self.connected && !self.remote_check_busy,
                    egui::Button::new("Проверить ID").min_size(egui::vec2(120.0, 32.0)),
                )
                .clicked()
            {
                self.check_remote_online();
            }
            if ui
                .add_enabled(
                    self.connected || self.busy,
                    egui::Button::new("Отключиться").min_size(egui::vec2(140.0, 32.0)),
                )
                .clicked()
            {
                self.disconnect_session("Отключено");
            }
            if ui
                .add_enabled(
                    self.connected && !self.remote_viewer_open,
                    egui::Button::new("Экран").min_size(egui::vec2(90.0, 32.0)),
                )
                .clicked()
            {
                self.remote_viewer_open = true;
                self.status = "Экран открыт".to_owned();
                self.send_command(SessionCommand::SetAutoRefresh {
                    enabled: self.auto_refresh,
                    millis: self.refresh_millis,
                });
                self.send_command(SessionCommand::Screenshot);
            }
        });
        ui.add_space(10.0);
        if self.progress > 0 || self.busy || self.connected || self.remote_check_busy {
            ui.add(
                egui::ProgressBar::new(self.progress as f32 / 100.0)
                    .text(format!("{}%", self.progress)),
            );
            ui.add_space(6.0);
        }
        if self.last_error.is_some() {
            ui.colored_label(
                egui::Color32::from_rgb(240, 120, 120),
                self.visible_status(),
            );
        } else {
            ui.label(self.visible_status());
        }
        if self.connected && !self.remote_viewer_open {
            ui.label("Окно экрана закрыто. Нажмите Экран, чтобы открыть его снова.");
        }

        if !self.events.is_empty() {
            ui.add_space(8.0);
            ui.separator();
            ui.collapsing("Лог событий", |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button("📋 Копировать лог")
                        .on_hover_text("Скопировать весь лог в буфер обмена")
                        .clicked()
                    {
                        let all = self.events.join("\n");
                        ui.ctx().copy_text(all);
                        self.log_status = Some("Лог скопирован в буфер обмена".to_owned());
                    }
                    if ui
                        .button("🗑 Очистить")
                        .on_hover_text("Очистить лог")
                        .clicked()
                    {
                        self.events.clear();
                    }
                });
                if let Some(status) = &self.log_status {
                    ui.label(
                        egui::RichText::new(status)
                            .size(10.5)
                            .color(egui::Color32::from_rgb(0x43, 0xA8, 0x47)),
                    );
                }
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for event in self.events.iter().rev().take(30) {
                            ui.label(egui::RichText::new(event).monospace().size(10.5));
                        }
                    });
            });
        }
    }

    // ── Host UI ───────────────────────────────────────────────────────────────

    /// Left sidebar: logo, app name, version, navigation, and Settings pinned
    /// to the bottom (per UI spec v1 — Arc/Linear-style rail).
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.set_width(ui.available_width());
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
            let p = ui.painter();
            p.rect_filled(
                rect,
                egui::CornerRadius::same(12),
                egui::Color32::from_rgb(0xFC, 0xFD, 0xFF),
            );
            p.rect_stroke(
                rect,
                egui::CornerRadius::same(12),
                egui::Stroke::new(1.0, egui::Color32::from_rgb(0xE5, 0xE8, 0xEF)),
                egui::StrokeKind::Inside,
            );
            if let Some(texture) = self.ensure_app_logo_texture(ui.ctx()) {
                let image_rect = rect.shrink(6.0);
                p.image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                p.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "E",
                    egui::FontId::proportional(26.0),
                    egui::Color32::from_rgb(0x16, 0x18, 0x20),
                );
            }
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.set_max_width(118.0);
                ui.add_space(3.0);
                ui.label(
                    egui::RichText::new(APP_NAME)
                        .size(14.0)
                        .strong()
                        .color(egui::Color32::from_rgb(0x17, 0x1A, 0x22)),
                );
                ui.label(
                    egui::RichText::new(format!("v{APP_VERSION}"))
                        .size(12.0)
                        .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                );
            });
        });

        ui.add_space(18.0);
        let connect_label = self.text("Подключиться", "Connect");
        let host_label = self.text("Этот компьютер", "This computer");
        let history_label = self.text("История", "History");
        let contacts_label = self.text("Контакты", "Contacts");
        self.nav_item(ui, AppMode::Connect, connect_label, "connect");
        ui.add_space(8.0);
        self.nav_item(ui, AppMode::Host, host_label, "monitor");
        ui.add_space(8.0);
        self.nav_item(ui, AppMode::History, history_label, "history");
        ui.add_space(8.0);
        self.nav_item(ui, AppMode::Contacts, contacts_label, "contacts");

        let spacer = (ui.available_height() - 50.0).max(10.0);
        ui.add_space(spacer);
        let settings_label = self.text("Настройки", "Settings");
        if nav_icon_button(
            ui,
            settings_label,
            "settings",
            self.mode == AppMode::Settings,
            true,
        )
        .clicked()
        {
            if self.settings_draft.is_none() {
                self.settings_draft = Some(self.config.clone());
            }
            self.mode = AppMode::Settings;
        }
    }

    fn nav_item(&mut self, ui: &mut egui::Ui, mode: AppMode, label: &str, icon: &str) {
        let active = self.mode == mode;
        if nav_icon_button(ui, label, icon, active, false).clicked() {
            self.mode = mode;
        }
    }

    /// Monochrome connection-status indicator (`● Ready` / `Connecting` …).
    /// Drawn inside a right-to-left layout, so the label is added first
    /// (rightmost) and the 8 px dot to its left. Connecting pulses softly.
    /// Connection-status pill (`╭ ○ Ready ╮`) — content-sized capsule that sits
    /// inside the Remote Control header, below the subtitle. The ring pulses
    /// softly while connecting.
    fn status_capsule(&self, ui: &mut egui::Ui) {
        use egui::Color32;
        let (label, base, pulse) = if self.last_error.is_some() {
            (
                self.text("Ошибка", "Error"),
                Color32::from_rgb(0xFF, 0x55, 0x55),
                false,
            )
        } else if self.connected {
            (
                self.text("Подключено", "Connected"),
                Color32::from_rgb(0x12, 0xC9, 0x72),
                false,
            )
        } else if self.busy {
            (
                self.text("Подключение", "Connecting"),
                Color32::from_rgb(0xE5, 0xA1, 0x00),
                true,
            )
        } else {
            (
                self.text("Готово", "Ready"),
                Color32::from_rgb(0x12, 0xC9, 0x72),
                false,
            )
        };
        // Left-align the (content-sized) pill.
        ui.horizontal(|ui| {
            egui::Frame::NONE
                .fill(Color32::from_rgb(0xFF, 0xFF, 0xFF))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(0xE3, 0xE6, 0xEC)))
                .corner_radius(egui::CornerRadius::same(20))
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    let col = if pulse {
                        let t = ui.input(|i| i.time);
                        let a = 0.55 + 0.45 * (t * std::f64::consts::TAU / 0.9).sin();
                        ui.ctx().request_repaint();
                        Color32::from_rgba_unmultiplied(
                            base.r(),
                            base.g(),
                            base.b(),
                            (a * 255.0) as u8,
                        )
                    } else {
                        base
                    };
                    ui.painter().circle_filled(rect.center(), 5.0, col);
                    ui.add_space(7.0);
                    ui.label(
                        egui::RichText::new(label)
                            .size(13.0)
                            .color(Color32::from_rgb(0x20, 0x24, 0x2D)),
                    );
                });
        });
    }

    #[allow(dead_code)]
    fn status_indicator(&self, ui: &mut egui::Ui) {
        use egui::Color32;
        let (label, base, pulse) = if self.last_error.is_some() {
            ("Error", Color32::from_rgb(0xFF, 0x55, 0x55), false)
        } else if self.connected {
            ("Connected", Color32::WHITE, false)
        } else if self.busy {
            ("Connecting", Color32::WHITE, true)
        } else {
            ("Ready", Color32::WHITE, false)
        };
        ui.label(
            egui::RichText::new(label)
                .size(14.0)
                .color(Color32::from_rgb(0xC8, 0xC8, 0xC8)),
        );
        ui.add_space(8.0);
        let dot = if pulse {
            let t = ui.input(|i| i.time);
            let a = 0.5 + 0.5 * (t * std::f64::consts::TAU / 0.8).sin();
            let alpha = (110.0 + 145.0 * a) as u8;
            ui.ctx().request_repaint();
            Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), alpha)
        } else {
            base
        };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, dot);
    }

    /// Right-hand "This computer" card: your ID, password, and host status.
    fn this_computer_card(&mut self, ui: &mut egui::Ui, _min_h: f32) {
        let gray = egui::Color32::from_rgb(0x50, 0x58, 0x68);
        card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                egui::RichText::new(self.text("Этот компьютер", "This computer"))
                    .size(18.0)
                    .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
            );
            ui.add_space(16.0);

            ui.label(
                egui::RichText::new(self.text("Ваш ID", "Your ID"))
                    .size(12.0)
                    .color(gray),
            );
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format_peer_id(&self.config.local_id))
                        .size(26.0)
                        .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, "copy")
                        .on_hover_text(self.text("Скопировать ID", "Copy ID"))
                        .clicked()
                    {
                        ui.ctx().copy_text(self.config.local_id.clone());
                    }
                });
            });

            ui.add_space(12.0);
            let show_host_label = self.text("показать", "show");
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(self.text("Пароль", "Password"))
                        .size(12.0)
                        .color(gray),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.show_host_password, show_host_label);
                });
            });
            ui.horizontal(|ui| {
                let pw_text = if self.show_host_password {
                    self.config.local_password.clone()
                } else {
                    "•".repeat(self.config.local_password.len().max(8))
                };
                ui.label(
                    egui::RichText::new(pw_text)
                        .size(22.0)
                        .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, "copy")
                        .on_hover_text(self.text("Скопировать пароль", "Copy password"))
                        .clicked()
                    {
                        ui.ctx().copy_text(self.config.local_password.clone());
                    }
                    if icon_button(ui, "refresh")
                        .on_hover_text(self.text("Новый пароль", "New password"))
                        .clicked()
                    {
                        self.config.local_password = generate_numeric_token(6);
                        self.config.save();
                    }
                });
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(12.0);

            ui.label(
                egui::RichText::new(self.text("Статус", "Status"))
                    .size(12.0)
                    .color(gray),
            );
            ui.add_space(6.0);
            let online = self.host_state.is_online();
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                let base = if online {
                    egui::Color32::from_rgb(0x12, 0xC9, 0x72)
                } else {
                    egui::Color32::from_rgb(0x12, 0xC9, 0x72)
                };
                ui.painter().circle_filled(rect.center(), 5.0, base);
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new(if online {
                        self.text("В сети", "Online")
                    } else {
                        self.text("Готов к подключению", "Ready to connect")
                    })
                    .size(16.0)
                    .color(egui::Color32::from_rgb(0x20, 0x24, 0x2D)),
                );
            });
        });
    }

    fn connect_ui_commercial(&mut self, ui: &mut egui::Ui) {
        ui.add_space(0.0);
        workspace_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let title = self.text("Удаленное управление", "Remote control");
            let subtitle = self.text(
                "Быстрый RustDesk-совместимый доступ через desk.everty.ru",
                "Fast RustDesk-compatible access through desk.everty.ru",
            );
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(title)
                            .size(28.0)
                            .strong()
                            .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                    );
                    ui.add_space(3.0);
                    ui.label(
                        egui::RichText::new(subtitle)
                            .size(13.0)
                            .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.status_capsule(ui);
                });
            });

            ui.add_space(16.0);
            let two_gap = 18.0;
            let right_width = 340.0_f32.min((ui.available_width() - two_gap) * 0.42);
            let two_left = (ui.available_width() - two_gap - right_width).clamp(320.0, 430.0);
            // ui.vertical columns auto-size to their content height, so the
            // workspace wraps tightly instead of stretching down the window.
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_min_width(two_left);
                    ui.set_max_width(two_left);
                    card_frame().show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.set_min_height(214.0);
                        ui.horizontal(|ui| {
                            let segment_width = (ui.available_width() - 8.0) / 2.0;
                            if mode_segment_button(
                                ui,
                                self.text("Экран", "Screen"),
                                "monitor",
                                self.connect_kind == ConnectKind::Screen,
                                segment_width,
                            )
                            .clicked()
                            {
                                self.connect_kind = ConnectKind::Screen;
                            }
                            ui.add_space(8.0);
                            if mode_segment_button(
                                ui,
                                self.text("Консоль", "Console"),
                                "console",
                                self.connect_kind == ConnectKind::Shell,
                                segment_width,
                            )
                            .clicked()
                            {
                                self.connect_kind = ConnectKind::Shell;
                            }
                        });
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(self.text("ID партнера", "Partner ID"))
                                .size(14.0)
                                .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
                        );
                        let remote_id_response = compact_text_input(
                            ui,
                            &mut self.remote_id,
                            "123 456 789",
                            false,
                            !self.connected && !self.busy,
                            Some(22.0),
                        );
                        ui.add_space(10.0);
                        let password_label = self.text("Пароль", "Password");
                        let show_label = self.text("показать", "show");
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(password_label)
                                    .size(14.0)
                                    .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.checkbox(&mut self.show_password, show_label);
                                },
                            );
                        });
                        let password_hint = self.text(
                            "Необязательно, если доступ подтверждают вручную",
                            "Optional for remote approval",
                        );
                        let password_response = compact_text_input(
                            ui,
                            &mut self.password,
                            password_hint,
                            !self.show_password,
                            !self.connected && !self.busy,
                            Some(16.0),
                        );
                        if remote_id_response.changed() || password_response.changed() {
                            self.last_error = None;
                            if !self.connected && !self.busy {
                                self.status = self.text("Готово", "Ready").to_owned();
                                self.progress = 0;
                            }
                        }

                        ui.add_space(14.0);
                        let connect_label = if self.busy {
                            self.text("Подключение...", "Connecting...")
                        } else {
                            self.text("Подключиться", "Connect")
                        };
                        if ui
                            .add_enabled_ui(!self.busy && !self.connected, |ui| {
                                primary_connect_button(
                                    ui,
                                    connect_label,
                                    if self.connect_kind == ConnectKind::Shell {
                                        "console"
                                    } else {
                                        "connect"
                                    },
                                )
                            })
                            .inner
                            .clicked()
                        {
                            self.connect();
                        }

                        ui.add_space(10.0);
                        let check_label = self.text("Проверить ID", "Check ID");
                        ui.horizontal(|ui| {
                            if secondary_button(ui, check_label)
                                .on_hover_text(self.text(
                                    "Проверить, онлайн ли этот ID",
                                    "Check whether this ID is online",
                                ))
                                .clicked()
                                && !self.busy
                                && !self.connected
                                && !self.remote_check_busy
                            {
                                self.check_remote_online();
                            }
                        });
                    });
                });
                ui.add_space(two_gap);
                ui.vertical(|ui| {
                    ui.set_min_width(right_width);
                    ui.set_max_width(right_width);
                    self.this_computer_card(ui, 0.0);
                });
            });
        }); // ── close workspace container ───────────────────────────────────

        ui.add_space(6.0);
        if self.progress > 0 || self.busy || self.connected || self.remote_check_busy {
            ui.add(
                egui::ProgressBar::new(self.progress as f32 / 100.0)
                    .desired_width(f32::INFINITY)
                    .text(format!("{}%", self.progress)),
            );
            ui.add_space(4.0);
        }
        if self.last_error.is_some() || self.busy || self.connected || self.remote_check_busy {
            let status_color = if self.last_error.is_some() {
                egui::Color32::from_rgb(238, 95, 95)
            } else if self.connected {
                egui::Color32::from_rgb(66, 190, 112)
            } else {
                egui::Color32::from_rgb(100, 112, 128)
            };
            ui.label(
                egui::RichText::new(self.visible_status())
                    .size(12.0)
                    .color(status_color),
            );
        }

        if self.config.ui.show_connection_details {
            ui.add_space(16.0);
            card_frame().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                egui::CollapsingHeader::new(self.text(
                    "Детали подключения - нажмите, чтобы раскрыть",
                    "Connection details - click to expand",
                ))
                .default_open(false)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .button(self.text("Скопировать лог", "Copy log"))
                            .clicked()
                        {
                            ui.ctx().copy_text(self.events.join("\n"));
                            self.log_status = Some("Log copied".to_owned());
                        }
                        if ui.button(self.text("Очистить", "Clear")).clicked() {
                            self.events.clear();
                        }
                    });
                    egui::ScrollArea::vertical()
                        .max_height(130.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for event in self.events.iter().rev().take(30) {
                                ui.label(
                                    egui::RichText::new(event)
                                        .monospace()
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(150, 158, 168)),
                                );
                            }
                        });
                });
            });
        }
    }

    fn host_ui(&mut self, ui: &mut egui::Ui) {
        if commercial_ui_enabled() {
            self.host_ui_commercial(ui);
            return;
        }
        ui.add_space(8.0);

        // ── ID + Password block ───────────────────────────────────────────────
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                // "Ваш ID"
                ui.label(
                    egui::RichText::new("Ваш ID")
                        .size(11.5)
                        .color(egui::Color32::GRAY),
                );
                ui.horizontal(|ui| {
                    // Large monospace ID — like RustDesk
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format_peer_id(&self.config.local_id))
                                .monospace()
                                .size(26.0)
                                .strong(),
                        )
                        .truncate(),
                    );
                    if ui.button("⎘").on_hover_text("Копировать ID").clicked() {
                        ui.ctx().copy_text(self.config.local_id.clone());
                    }
                });

                ui.add_space(8.0);

                // Password
                ui.label(
                    egui::RichText::new("Пароль")
                        .size(11.5)
                        .color(egui::Color32::GRAY),
                );
                ui.horizontal(|ui| {
                    let pw_text = if self.show_host_password {
                        self.config.local_password.clone()
                    } else {
                        "•".repeat(self.config.local_password.len())
                    };
                    ui.add(
                        egui::Label::new(egui::RichText::new(pw_text).monospace().size(20.0))
                            .truncate(),
                    );
                    if ui.button("⎘").on_hover_text("Копировать пароль").clicked()
                    {
                        ui.ctx().copy_text(self.config.local_password.clone());
                    }
                    if ui.button("↺").on_hover_text("Новый пароль").clicked() {
                        self.config.local_password = generate_numeric_token(6);
                        self.config.save();
                    }
                    ui.checkbox(&mut self.show_host_password, "Показать");
                });
            });

        ui.add_space(10.0);

        // ── Status + Start/Stop ───────────────────────────────────────────────
        ui.horizontal(|ui| {
            // Coloured status indicator
            let (r, g, b) = self.host_state.color();
            let dot_color = egui::Color32::from_rgb(r, g, b);
            ui.colored_label(dot_color, "●");
            ui.label(self.host_state.label());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let service_running = self.host_service.is_some();
                if service_running {
                    if ui
                        .add(egui::Button::new("⏹ Остановить").min_size(egui::vec2(130.0, 28.0)))
                        .clicked()
                    {
                        self.stop_host_service();
                    }
                } else {
                    if ui
                        .add(egui::Button::new("▶ Запустить").min_size(egui::vec2(130.0, 28.0)))
                        .clicked()
                    {
                        self.start_host_service();
                    }
                }
            });
        });
        if let Some(video_status) = &self.host_video_status {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("Видео: {video_status}"))
                    .monospace()
                    .size(10.5)
                    .color(egui::Color32::from_rgb(120, 130, 145)),
            );
        }

        // Pending incoming connection
        if let Some(peer_id) = self.host_pending_peer.clone() {
            ui.add_space(8.0);
            egui::Frame::group(ui.style())
                .fill(egui::Color32::from_rgba_premultiplied(60, 110, 180, 40))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!("📡 Входящий запрос от: {peer_id}")).strong(),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new("✓ Принять")
                                    .min_size(egui::vec2(100.0, 28.0))
                                    .fill(egui::Color32::from_rgb(40, 160, 80)),
                            )
                            .clicked()
                        {
                            // TODO: full relay session acceptance
                            self.host_pending_peer = None;
                        }
                        if ui
                            .add(
                                egui::Button::new("✗ Отклонить")
                                    .min_size(egui::vec2(100.0, 28.0))
                                    .fill(egui::Color32::from_rgb(200, 60, 60)),
                            )
                            .clicked()
                        {
                            self.host_pending_peer = None;
                        }
                    });
                });
        }

        // Host log
        if !self.host_log.is_empty() {
            ui.add_space(8.0);
            ui.separator();
            ui.collapsing("Лог хост-сервиса", |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button("📋 Копировать лог")
                        .on_hover_text("Скопировать весь лог хоста в буфер обмена")
                        .clicked()
                    {
                        let all = self.host_log.join("\n");
                        ui.ctx().copy_text(all);
                        self.host_status = "Лог хоста скопирован в буфер обмена".to_owned();
                    }
                    if ui
                        .button("🗑 Очистить")
                        .on_hover_text("Очистить лог хоста")
                        .clicked()
                    {
                        self.host_log.clear();
                    }
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in self.host_log.iter().rev().take(25) {
                            ui.label(egui::RichText::new(line).monospace().size(10.5));
                        }
                    });
            });
        }
    }

    fn history_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new(self.text("История подключений", "Connection history"))
                .size(28.0)
                .strong()
                .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(self.text(
                "Последние ID сохраняются локально. Можно добавить заметку.",
                "Recent IDs are stored locally. You can add a note.",
            ))
            .size(13.0)
            .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
        );
        ui.add_space(16.0);

        if self.config.ui.history.is_empty() {
            card_frame().show(ui, |ui| {
                ui.label(self.text("История пока пустая.", "History is empty."));
            });
            return;
        }

        let mut connect_to: Option<String> = None;
        let mut remove_id: Option<String> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for entry in &mut self.config.ui.history {
                card_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(format_peer_id(&entry.remote_id))
                                    .size(22.0)
                                    .strong()
                                    .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}: {}",
                                    tr(self.ui_lang, "Подключений", "Connections"),
                                    entry.connect_count
                                ))
                                .size(12.0)
                                .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(tr(self.ui_lang, "Удалить", "Remove")).clicked() {
                                remove_id = Some(entry.remote_id.clone());
                            }
                            if ui
                                .button(tr(self.ui_lang, "Подключиться", "Connect"))
                                .clicked()
                            {
                                connect_to = Some(entry.remote_id.clone());
                            }
                        });
                    });
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(tr(self.ui_lang, "Заметка", "Note"))
                            .size(12.0)
                            .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
                    );
                    ui.add_sized(
                        egui::vec2(ui.available_width(), 40.0),
                        egui::TextEdit::singleline(&mut entry.note).hint_text(tr(
                            self.ui_lang,
                            "Например: бухгалтерия, ноутбук Ивана...",
                            "Example: accounting, Ivan's laptop...",
                        )),
                    );
                });
                ui.add_space(8.0);
            }
        });

        if let Some(id) = remove_id {
            self.config.ui.history.retain(|entry| entry.remote_id != id);
            self.config.save();
        } else {
            self.config.save();
        }
        if let Some(id) = connect_to {
            self.remote_id = id;
            self.mode = AppMode::Connect;
        }
    }

    fn incoming_approval_window(&mut self, ctx: &egui::Context) {
        let Some(peer_id) = self.host_pending_peer.clone() else {
            return;
        };
        egui::Window::new(self.text("Входящее подключение", "Incoming connection"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.label(
                    egui::RichText::new(self.text(
                        "Удаленный пользователь хочет подключиться без пароля.",
                        "A remote user wants to connect without a password.",
                    ))
                    .size(15.0)
                    .color(egui::Color32::from_rgb(0x20, 0x24, 0x2D)),
                );
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(format!("ID: {}", format_peer_id(&peer_id)))
                        .size(22.0)
                        .strong()
                        .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(self.text("Разрешить", "Allow"))
                                .min_size(egui::vec2(132.0, 40.0))
                                .fill(egui::Color32::from_rgb(0x12, 0xC9, 0x72)),
                        )
                        .clicked()
                    {
                        if let Some(svc) = &self.host_service {
                            svc.approve_incoming(&peer_id, true);
                        }
                        self.host_pending_peer = None;
                    }
                    if ui
                        .add(
                            egui::Button::new(self.text("Отклонить", "Reject"))
                                .min_size(egui::vec2(132.0, 40.0)),
                        )
                        .clicked()
                    {
                        if let Some(svc) = &self.host_service {
                            svc.approve_incoming(&peer_id, false);
                        }
                        self.host_pending_peer = None;
                    }
                });
            });
    }

    fn host_ui_commercial(&mut self, ui: &mut egui::Ui) {
        ui.add_space(0.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(self.text("Этот компьютер", "This computer"))
                        .size(34.0)
                        .strong()
                        .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(self.text(
                        "Покажите этот ID и пароль для входящего подключения",
                        "Share this ID and password for direct unattended access",
                    ))
                    .size(15.0)
                    .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (r, g, b) = self.host_state.color();
                status_pill(ui, self.host_state_text(), egui::Color32::from_rgb(r, g, b));
            });
        });

        ui.add_space(24.0);

        card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                egui::RichText::new(self.text("Данные доступа", "Access credentials"))
                    .size(18.0)
                    .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
            );
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(self.text("Ваш ID", "Your ID"))
                            .size(12.0)
                            .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
                    );
                    ui.label(
                        egui::RichText::new(format_peer_id(&self.config.local_id))
                            .size(28.0)
                            .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, "copy")
                        .on_hover_text(self.text("Скопировать ID", "Copy ID"))
                        .clicked()
                    {
                        ui.ctx().copy_text(self.config.local_id.clone());
                    }
                });
            });

            ui.add_space(18.0);
            ui.separator();
            ui.add_space(18.0);

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(self.text("Пароль доступа", "Access password"))
                            .size(12.0)
                            .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
                    );
                    let pw_text = if self.show_host_password {
                        self.config.local_password.clone()
                    } else {
                        "•".repeat(self.config.local_password.len())
                    };
                    ui.label(
                        egui::RichText::new(pw_text)
                            .size(24.0)
                            .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                    );
                });
                let show_label = self.text("показать", "show");
                let copy_pw_tip = self.text("Скопировать пароль", "Copy password");
                let new_pw_tip = self.text("Новый пароль", "New password");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.show_host_password, show_label);
                    if icon_button(ui, "copy").on_hover_text(copy_pw_tip).clicked() {
                        ui.ctx().copy_text(self.config.local_password.clone());
                    }
                    if icon_button(ui, "refresh")
                        .on_hover_text(new_pw_tip)
                        .clicked()
                    {
                        self.config.local_password = generate_numeric_token(6);
                        self.config.save();
                    }
                });
            });
        });

        ui.add_space(24.0);
        card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                egui::RichText::new(self.text("Доступ", "Sharing"))
                    .size(18.0)
                    .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
            );
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                let service_running = self.host_service.is_some();
                let label = if service_running {
                    self.text("Остановить доступ", "Stop sharing")
                } else {
                    self.text("Запустить доступ", "Start sharing")
                };
                let button = egui::Button::new(
                    egui::RichText::new(label)
                        .size(16.0)
                        .strong()
                        .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                )
                .min_size(egui::vec2(180.0, 54.0))
                .fill(egui::Color32::from_rgb(0xFF, 0xFF, 0xFF))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgb(0xE3, 0xE6, 0xEC),
                ))
                .corner_radius(egui::CornerRadius::same(10));
                if ui.add(button).clicked() {
                    if service_running {
                        self.stop_host_service();
                    } else {
                        self.start_host_service();
                    }
                }
            });
            ui.add_space(18.0);
            ui.separator();
            ui.add_space(18.0);
            ui.label(
                egui::RichText::new(&self.host_status)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
            );
            if let Some(video_status) = &self.host_video_status {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("Video: {video_status}"))
                        .monospace()
                        .size(10.5)
                        .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                );
            }
        });

        ui.add_space(24.0);
        card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            egui::CollapsingHeader::new(self.text("Диагностика хоста", "Host diagnostics"))
                .default_open(false)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .button(self.text("Скопировать лог", "Copy log"))
                            .clicked()
                        {
                            ui.ctx().copy_text(self.host_log.join("\n"));
                            self.host_status = "Host log copied".to_owned();
                        }
                        if ui.button(self.text("Очистить", "Clear")).clicked() {
                            self.host_log.clear();
                        }
                    });
                    egui::ScrollArea::vertical()
                        .max_height(140.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for line in self.host_log.iter().rev().take(30) {
                                ui.label(
                                    egui::RichText::new(line)
                                        .monospace()
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(150, 158, 168)),
                                );
                            }
                        });
                });
        });
    }

    fn start_host_service(&mut self) {
        if self.host_service.is_some() {
            return;
        }
        self.host_log
            .push(format!("[{}] Запуск хост-сервиса...", timestamp_hms()));
        self.host_state = HostState::Connecting;
        self.host_video_status = None;
        self.host_service = Some(HostService::start(self.config.clone()));
    }

    fn stop_host_service(&mut self) {
        if let Some(svc) = self.host_service.take() {
            svc.stop();
            self.host_log
                .push(format!("[{}] Хост-сервис остановлен.", timestamp_hms()));
        }
        self.host_state = HostState::Idle;
        self.host_pending_peer = None;
        self.host_video_status = None;
    }

    fn poll_host_service(&mut self) {
        let Some(svc) = &self.host_service else {
            return;
        };
        // Drain all events from the host background thread.
        let mut events_buf: Vec<HostEvent> = Vec::new();
        while let Some(ev) = svc.try_recv() {
            events_buf.push(ev);
        }
        for ev in events_buf {
            self.handle_host_event(ev);
        }
    }

    fn handle_host_event(&mut self, ev: HostEvent) {
        match ev {
            HostEvent::StateChanged(state) => {
                self.host_state = state;
            }
            HostEvent::Registered { .. } => {
                self.host_log.push(format!(
                    "[{}] Зарегистрировано на ID сервере ✓",
                    timestamp_hms()
                ));
            }
            HostEvent::IncomingRequest { peer_id, .. } => {
                self.host_log.push(format!(
                    "[{}] Входящий запрос от {peer_id}",
                    timestamp_hms()
                ));
                self.host_status = format!("Incoming connection: authenticating {peer_id}");
            }
            HostEvent::ApprovalRequested { peer_id } => {
                self.host_log.push(format!(
                    "[{}] Запрос подтверждения от {peer_id}",
                    timestamp_hms()
                ));
                self.host_pending_peer = Some(peer_id.clone());
                self.host_status = format!("Требуется подтверждение: {peer_id}");
            }
            HostEvent::SessionStarted { peer_id } => {
                self.host_log
                    .push(format!("[{}] Сессия с {peer_id} начата", timestamp_hms()));
                self.host_pending_peer = None;
                self.host_status = format!("Connected: {peer_id}");
                self.host_video_status = None;
            }
            HostEvent::SessionEnded { peer_id, reason } => {
                self.host_log.push(format!(
                    "[{}] Сессия с {peer_id} завершена {reason}",
                    timestamp_hms(),
                ));
                self.host_video_status = None;
            }
            HostEvent::VideoTelemetry {
                summary,
                fallback_reason,
            } => {
                self.host_video_status = Some(compact_host_video_status(&summary));
                if let Some(reason) = fallback_reason {
                    self.host_status = format!("Video fallback: {reason}");
                }
            }
            HostEvent::Log(msg) => {
                self.host_log.push(format!("[{}] {msg}", timestamp_hms()));
                if self.host_log.len() > 200 {
                    self.host_log.drain(0..50);
                }
            }
        }
    }

    // ── Check host server (legacy) ────────────────────────────────────────────
    #[allow(dead_code)]
    fn check_host_server(&mut self) {
        if self.host_check_busy || self.busy {
            return;
        }
        self.host_check_busy = true;
        self.host_status = "Проверяем ID server...".to_owned();
        self.log("Host check: checking ID server".to_owned());
        let server = self.config.server.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = TransportClient::check_id_server(&server);
            let _ = tx.send(WorkerEvent::HostServerCheck(result));
        });
        self.worker = Some(rx);
    }

    // ── Settings window ───────────────────────────────────────────────────────

    fn check_remote_online(&mut self) {
        if self.remote_check_busy || self.busy || self.connected {
            return;
        }
        let remote_id = normalize_remote_id(&self.remote_id);
        if remote_id.is_empty() {
            self.set_error("Введите ID удаленного ПК");
            return;
        }
        if is_own_remote_id(&remote_id, &self.config.local_id) {
            self.set_error("Это ID этого компьютера. Для подключения нужен ID другого ПК.");
            return;
        }
        self.remote_id = remote_id.clone();
        self.remote_check_busy = true;
        self.progress = 10;
        self.last_error = None;
        self.status = format!("Проверяем ID {remote_id}...");
        self.log(self.status.clone());
        let server = self.config.server.clone();
        let local_id = self.config.local_id.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = TransportClient::query_peer_online(&server, &local_id, &remote_id);
            let _ = tx.send(WorkerEvent::RemoteOnlineCheck { remote_id, result });
        });
        self.worker = Some(rx);
    }

    #[allow(deprecated)]
    fn remote_viewer_inline(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.remote_input_focused = false;
            self.release_remote_modifiers();
            self.last_mouse_pos = None;
            self.wheel_accum = egui::Vec2::ZERO;
        }

        egui::Panel::top("software-remote-toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("Back")
                    .on_hover_text("Close remote screen")
                    .clicked()
                {
                    self.remote_viewer_open = false;
                    self.remote_input_focused = false;
                    self.release_remote_modifiers();
                    self.last_mouse_pos = None;
                    self.wheel_accum = egui::Vec2::ZERO;
                    self.status = "Remote screen closed".to_owned();
                    self.send_command(SessionCommand::SetAutoRefresh {
                        enabled: false,
                        millis: self.refresh_millis,
                    });
                }

                if ui.button("Disconnect").clicked() {
                    self.disconnect_session("Disconnected");
                }

                if !self.remote_displays.is_empty() {
                    ui.separator();
                    let selected_text = self
                        .remote_displays
                        .iter()
                        .find(|d| d.index == self.selected_display)
                        .map(display_label)
                        .unwrap_or_else(|| format!("Display {}", self.selected_display + 1));
                    egui::ComboBox::from_id_salt("software-remote-display")
                        .selected_text(selected_text)
                        .show_ui(ui, |ui| {
                            let displays = self.remote_displays.clone();
                            for display in displays {
                                let label = display_label(&display);
                                if ui
                                    .selectable_value(
                                        &mut self.selected_display,
                                        display.index,
                                        label,
                                    )
                                    .clicked()
                                {
                                    self.remote_texture = None;
                                    self.remote_size = [0, 0];
                                    self.send_command(SessionCommand::SetDisplay(display));
                                }
                            }
                        });
                }

                ui.separator();
                ui.add(
                    egui::TextEdit::singleline(&mut self.text_to_send)
                        .hint_text("Text -> Enter")
                        .desired_width(180.0),
                );
                if ui
                    .add_enabled(!self.text_to_send.is_empty(), egui::Button::new("Send"))
                    .clicked()
                    || (ui.ctx().input(|i| i.key_pressed(egui::Key::Enter))
                        && !self.remote_input_focused
                        && !self.text_to_send.is_empty())
                {
                    let text = std::mem::take(&mut self.text_to_send);
                    self.send_command(SessionCommand::KeyText(text));
                    self.request_visual_refresh_after_input();
                }

                if ui.button("Paste").clicked() {
                    self.paste_local_clipboard_to_remote();
                }
                if ui
                    .button("PNG")
                    .on_hover_text("Save current frame")
                    .clicked()
                {
                    self.save_current_frame_png();
                }
                if ui.button("Refresh").clicked() {
                    self.send_command(SessionCommand::RefreshVideo);
                    self.send_command(SessionCommand::Screenshot);
                }
                if ui.checkbox(&mut self.fit_to_window, "Fit").changed() {
                    self.save_ui_config();
                }

                ui.menu_button("More", |ui| {
                    if ui
                        .checkbox(&mut self.auto_refresh, "Auto refresh")
                        .changed()
                    {
                        self.save_ui_config();
                        self.send_command(SessionCommand::SetAutoRefresh {
                            enabled: self.auto_refresh,
                            millis: self.refresh_millis,
                        });
                    }
                    let mut refresh_ms = self.refresh_millis as f32;
                    if ui
                        .add(
                            egui::Slider::new(&mut refresh_ms, 50.0..=2000.0)
                                .text("ms/frame")
                                .clamping(egui::SliderClamping::Always),
                        )
                        .changed()
                    {
                        self.refresh_millis = refresh_ms.round() as u64;
                        self.save_ui_config();
                        self.send_command(SessionCommand::SetAutoRefresh {
                            enabled: self.auto_refresh,
                            millis: self.refresh_millis,
                        });
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Video fps");
                        for fps in [15, 20, 30, 60] {
                            if ui
                                .selectable_label(self.video_fps == fps, fps.to_string())
                                .clicked()
                            {
                                self.video_fps = fps;
                                self.send_command(SessionCommand::SetVideoFps { fps });
                            }
                        }
                    });
                    if ui.button("Ctrl+Alt+Del").clicked() {
                        self.send_command(SessionCommand::KeyControl(ControlKey::CtrlAltDel));
                        self.request_visual_refresh_after_input();
                        ui.close();
                    }
                    if ui.button("Lock screen").clicked() {
                        self.send_command(SessionCommand::KeyControl(ControlKey::LockScreen));
                        self.request_visual_refresh_after_input();
                        ui.close();
                    }
                    if ui.button("Save log").clicked() {
                        self.save_session_log_file();
                        ui.close();
                    }
                    if ui.button("Support report").clicked() {
                        self.save_support_report();
                        ui.close();
                    }
                });
            });
        });

        egui::Panel::bottom("software-remote-statusbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if self.remote_size[0] > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "{}x{}",
                            self.remote_size[0], self.remote_size[1]
                        ))
                        .monospace()
                        .size(11.0),
                    );
                    ui.separator();
                }

                let fps_color = if self.display_fps >= 20.0 {
                    egui::Color32::from_rgb(80, 200, 100)
                } else if self.display_fps >= 8.0 {
                    egui::Color32::from_rgb(220, 180, 60)
                } else {
                    egui::Color32::from_rgb(220, 80, 80)
                };
                ui.label(
                    egui::RichText::new(format!("{:.1} fps", self.display_fps))
                        .monospace()
                        .size(11.0)
                        .color(fps_color),
                );
                ui.separator();
                let input_color = if self.stream_input_fps >= 20.0 {
                    egui::Color32::from_rgb(80, 200, 100)
                } else if self.stream_input_fps >= 8.0 {
                    egui::Color32::from_rgb(220, 180, 60)
                } else {
                    egui::Color32::from_rgb(220, 80, 80)
                };
                ui.label(
                    egui::RichText::new(format!(
                        "in {:.1}/s {} kbps",
                        self.stream_input_fps, self.stream_input_kbps
                    ))
                    .monospace()
                    .size(11.0)
                    .color(input_color),
                )
                .on_hover_text("Входящие live-video пакеты до декодера");
                ui.separator();

                let (codec_label, codec_color) = match self.last_frame_codec.as_str() {
                    "H264" | "H265" | "AV1" | "VP9" => (
                        self.last_frame_codec.as_str(),
                        egui::Color32::from_rgb(80, 220, 110),
                    ),
                    "PNG" => ("PNG", egui::Color32::from_rgb(220, 180, 60)),
                    _ => ("no frame", egui::Color32::GRAY),
                };
                ui.label(
                    egui::RichText::new(codec_label)
                        .strong()
                        .size(12.5)
                        .color(codec_color),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(crate::video::build_codec_label())
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(145, 160, 175)),
                );

                if let Some(ms) = self.latency_ms {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("{ms} ms"))
                            .monospace()
                            .size(11.0),
                    );
                }
                if self.frame_bytes > 0 {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!(
                            "{} KB q:{}ms dec:{}ms",
                            self.frame_bytes / 1024,
                            self.frame_queue_ms,
                            self.frame_decode_ms
                        ))
                        .monospace()
                        .size(11.0),
                    );
                }
                if self.frame_dropped > 0 {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("drop {}", self.frame_dropped))
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(220, 110, 80)),
                    );
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(&self.stream_health)
                        .size(11.0)
                        .color(stream_health_color(&self.stream_health)),
                );

                for status in [
                    self.clipboard_status.as_deref(),
                    self.screenshot_status.as_deref(),
                    self.log_status.as_deref(),
                    self.report_status.as_deref(),
                ]
                .into_iter()
                .flatten()
                {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(status)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(150, 160, 170)),
                    );
                }

                ui.separator();
                if self.remote_input_focused {
                    ui.label(
                        egui::RichText::new("input captured [Esc]")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(45, 160, 230)),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("hover/click remote screen for input")
                            .size(11.0)
                            .color(egui::Color32::GRAY),
                    );
                }
                if self.screenshot_pending {
                    ui.spinner();
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.remote_screen_ui(ui);
        });
    }

    #[allow(deprecated)]
    fn remote_viewer_window(&mut self, ctx: &egui::Context) {
        let title = {
            let id = if self.remote_id.trim().is_empty() {
                "удалённый ПК".to_owned()
            } else {
                self.remote_id.trim().to_owned()
            };
            let fps_part = if self.display_fps > 0.1 {
                format!("  {:.0} fps", self.display_fps)
            } else {
                String::new()
            };
            let lat_part = self
                .latency_ms
                .map(|ms| format!("  {ms}ms"))
                .unwrap_or_default();
            format!("EvertyDesk — {id}{fps_part}{lat_part}")
        };
        let viewport_id = egui::ViewportId::from_hash_of("evertydesk-lite-remote-viewer");
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_resizable(true)
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([720.0, 480.0]);

        ctx.show_viewport_immediate(viewport_id, builder, |ctx, _class| {
            if ctx.input(|input| input.viewport().close_requested()) {
                self.remote_viewer_open = false;
                self.remote_input_focused = false;
                self.release_remote_modifiers();
                self.last_mouse_pos = None;
                self.wheel_accum = egui::Vec2::ZERO;
                self.disconnect_session("Окно управления закрыто");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            if ctx.input(|input| input.key_pressed(egui::Key::F11)) {
                self.remote_fullscreen = !self.remote_fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.remote_fullscreen));
            }

            // ── Toolbar ────────────────────────────────────────────────────────────
            egui::Panel::top("remote-toolbar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Display selector
                    if !self.remote_displays.is_empty() {
                        let selected_text = self
                            .remote_displays
                            .iter()
                            .find(|d| d.index == self.selected_display)
                            .map(display_label)
                            .unwrap_or_else(|| format!("Дисплей {}", self.selected_display + 1));
                        egui::ComboBox::from_id_salt("remote-display")
                            .selected_text(selected_text)
                            .show_ui(ui, |ui| {
                                let displays = self.remote_displays.clone();
                                for display in displays {
                                    let label = display_label(&display);
                                    if ui
                                        .selectable_value(
                                            &mut self.selected_display,
                                            display.index,
                                            label,
                                        )
                                        .clicked()
                                    {
                                        self.remote_texture = None;
                                        self.remote_size = [0, 0];
                                        self.send_command(SessionCommand::SetDisplay(display));
                                    }
                                }
                            });
                    }

                    ui.separator();

                    // Quick text send
                    ui.add(
                        egui::TextEdit::singleline(&mut self.text_to_send)
                            .hint_text("Текст → Enter")
                            .desired_width(160.0),
                    );
                    let send_text =
                        ui.add_enabled(!self.text_to_send.is_empty(), egui::Button::new("⏎"));
                    if send_text.clicked()
                        || (ui.ctx().input(|i| i.key_pressed(egui::Key::Enter))
                            && !self.remote_input_focused
                            && !self.text_to_send.is_empty())
                    {
                        let text = std::mem::take(&mut self.text_to_send);
                        self.send_command(SessionCommand::KeyText(text));
                        self.request_visual_refresh_after_input();
                    }
                    if ui.button("⧉").on_hover_text("Вставить из буфера").clicked()
                    {
                        self.paste_local_clipboard_to_remote();
                    }
                    if ui.button("▣").on_hover_text("Сохранить кадр PNG").clicked() {
                        self.save_current_frame_png();
                    }

                    ui.separator();

                    if ui.button("🗗").on_hover_text("Полный экран (F11)").clicked() {
                        self.remote_fullscreen = !self.remote_fullscreen;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                            self.remote_fullscreen,
                        ));
                    }
                    if ui.checkbox(&mut self.fit_to_window, "Вписать").changed() {
                        self.save_ui_config();
                    }

                    ui.separator();

                    // "More" menu — advanced / rarely used settings
                    ui.menu_button("⋯", |ui| {
                        if ui
                            .checkbox(&mut self.auto_refresh, "Авто-обновление")
                            .changed()
                        {
                            self.save_ui_config();
                            self.send_command(SessionCommand::SetAutoRefresh {
                                enabled: self.auto_refresh,
                                millis: self.refresh_millis,
                            });
                        }
                        let mut refresh_ms = self.refresh_millis as f32;
                        if ui
                            .add(
                                egui::Slider::new(&mut refresh_ms, 50.0..=2000.0)
                                    .text("мс / кадр")
                                    .clamping(egui::SliderClamping::Always),
                            )
                            .changed()
                        {
                            self.refresh_millis = refresh_ms.round() as u64;
                            self.save_ui_config();
                            self.send_command(SessionCommand::SetAutoRefresh {
                                enabled: self.auto_refresh,
                                millis: self.refresh_millis,
                            });
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label("Video fps");
                            for fps in [15, 20, 30, 60] {
                                if ui
                                    .selectable_label(self.video_fps == fps, fps.to_string())
                                    .clicked()
                                {
                                    self.video_fps = fps;
                                    self.send_command(SessionCommand::SetVideoFps { fps });
                                }
                            }
                        });
                        if ui.button("↺ Обновить поток").clicked() {
                            self.send_command(SessionCommand::RefreshVideo);
                            ui.close();
                        }
                        if ui.button("Вставить буфер").clicked() {
                            self.paste_local_clipboard_to_remote();
                            ui.close();
                        }
                        if ui.button("Сохранить кадр PNG").clicked() {
                            self.save_current_frame_png();
                            ui.close();
                        }
                        if ui.button("Сохранить лог").clicked() {
                            self.save_session_log_file();
                            ui.close();
                        }
                        if ui.button("Собрать отчёт").clicked() {
                            self.save_support_report();
                            ui.close();
                        }
                        ui.separator();
                        egui::ComboBox::from_id_salt("coordinate-mode")
                            .selected_text(coordinate_mode_label(self.coordinate_mode))
                            .show_ui(ui, |ui| {
                                for mode in [
                                    CoordinateMode::Auto,
                                    CoordinateMode::Absolute,
                                    CoordinateMode::Local,
                                ] {
                                    if ui
                                        .selectable_value(
                                            &mut self.coordinate_mode,
                                            mode,
                                            coordinate_mode_label(mode),
                                        )
                                        .clicked()
                                    {
                                        self.save_ui_config();
                                        self.last_mouse_pos = None;
                                    }
                                }
                            });
                        ui.separator();
                        if ui.button("Ctrl+Alt+Del").clicked() {
                            self.send_command(SessionCommand::KeyControl(ControlKey::CtrlAltDel));
                            self.request_visual_refresh_after_input();
                            ui.close();
                        }
                        if ui.button("🔒 Заблокировать экран").clicked() {
                            self.send_command(SessionCommand::KeyControl(ControlKey::LockScreen));
                            self.request_visual_refresh_after_input();
                            ui.close();
                        }
                        ui.separator();
                        if let Some((x, y)) = self.last_mouse_pos {
                            ui.label(format!("Мышь: {x}, {y}"));
                        }
                        if let Some(age) = self.last_screenshot_age_ms() {
                            ui.label(format!("Возраст кадра: {age} мс"));
                        }
                        ui.label(format!("Отправлено событий: {}", self.input_events_sent));
                    });
                });
            });

            // ── Status bar (bottom) ─────────────────────────────────────────────────
            egui::Panel::bottom("remote-statusbar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Resolution
                    if self.remote_size[0] > 0 {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}×{}",
                                self.remote_size[0], self.remote_size[1]
                            ))
                            .monospace()
                            .size(11.0),
                        );
                        ui.separator();
                    }
                    // FPS
                    let fps_color = if self.display_fps >= 20.0 {
                        egui::Color32::from_rgb(80, 200, 100)
                    } else if self.display_fps >= 8.0 {
                        egui::Color32::from_rgb(220, 180, 60)
                    } else {
                        egui::Color32::from_rgb(220, 80, 80)
                    };
                    ui.label(
                        egui::RichText::new(format!("{:.1} fps", self.display_fps))
                            .monospace()
                            .size(11.0)
                            .color(fps_color),
                    );
                    ui.separator();
                    let input_color = if self.stream_input_fps >= 20.0 {
                        egui::Color32::from_rgb(80, 200, 100)
                    } else if self.stream_input_fps >= 8.0 {
                        egui::Color32::from_rgb(220, 180, 60)
                    } else {
                        egui::Color32::from_rgb(220, 80, 80)
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "in {:.1}/s {} kbps",
                            self.stream_input_fps, self.stream_input_kbps
                        ))
                        .monospace()
                        .size(11.0)
                        .color(input_color),
                    )
                    .on_hover_text("Входящие live-video пакеты до декодера");
                    ui.separator();
                    // Codec badge: color shows quality:
                    //   H264 green  = live video, low latency
                    //   PNG  amber  = screenshot mode, higher latency
                    //   none gray   = no frames yet
                    let (codec_label, codec_color, codec_tip) = match self.last_frame_codec.as_str()
                    {
                        "H264" => (
                            "H264",
                            egui::Color32::from_rgb(80, 220, 110),
                            "Live H264 video, lowest latency",
                        ),
                        "H265" | "AV1" | "VP9" => (
                            self.last_frame_codec.as_str(),
                            egui::Color32::from_rgb(80, 220, 110),
                            "Live video",
                        ),
                        "PNG" => (
                            "PNG",
                            egui::Color32::from_rgb(220, 180, 60),
                            "Screenshot fallback, higher latency",
                        ),
                        _ => ("no frame", egui::Color32::GRAY, "No frames received yet"),
                    };
                    ui.label(
                        egui::RichText::new(codec_label)
                            .strong()
                            .size(12.5)
                            .color(codec_color),
                    )
                    .on_hover_text(codec_tip);
                    ui.separator();
                    ui.label(
                        egui::RichText::new(crate::video::build_codec_label())
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(145, 160, 175)),
                    )
                    .on_hover_text("Codecs compiled into this EvertyDesk Lite build");
                    ui.separator();
                    // Latency
                    if let Some(ms) = self.latency_ms {
                        let lat_color = if ms < 50 {
                            egui::Color32::from_rgb(80, 200, 100)
                        } else if ms < 150 {
                            egui::Color32::from_rgb(220, 180, 60)
                        } else {
                            egui::Color32::from_rgb(220, 80, 80)
                        };
                        ui.label(
                            egui::RichText::new(format!("{ms} ms"))
                                .monospace()
                                .size(11.0)
                                .color(lat_color),
                        );
                        ui.separator();
                    }
                    if self.frame_bytes > 0 {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} KB  q:{}ms  dec:{}ms",
                                self.frame_bytes / 1024,
                                self.frame_queue_ms,
                                self.frame_decode_ms
                            ))
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(150, 160, 170)),
                        )
                        .on_hover_text("Размер кадра, ожидание в очереди и время декодирования");
                        ui.separator();
                    }
                    if self.frame_dropped > 0 {
                        ui.label(
                            egui::RichText::new(format!("drop {}", self.frame_dropped))
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(220, 110, 80)),
                        )
                        .on_hover_text("Сколько старых кадров было сброшено, чтобы догнать поток");
                        ui.separator();
                    }
                    ui.label(
                        egui::RichText::new(&self.stream_health)
                            .size(11.0)
                            .color(stream_health_color(&self.stream_health)),
                    )
                    .on_hover_text("Автоматическая оценка состояния видеопотока");
                    ui.separator();
                    if let Some(status) = &self.clipboard_status {
                        ui.label(
                            egui::RichText::new(status)
                                .size(11.0)
                                .color(egui::Color32::from_rgb(150, 160, 170)),
                        );
                        ui.separator();
                    }
                    if let Some(status) = &self.screenshot_status {
                        ui.label(
                            egui::RichText::new(status)
                                .size(11.0)
                                .color(egui::Color32::from_rgb(150, 160, 170)),
                        );
                        ui.separator();
                    }
                    if let Some(status) = &self.log_status {
                        ui.label(
                            egui::RichText::new(status)
                                .size(11.0)
                                .color(egui::Color32::from_rgb(150, 160, 170)),
                        );
                        ui.separator();
                    }
                    if let Some(status) = &self.report_status {
                        ui.label(
                            egui::RichText::new(status)
                                .size(11.0)
                                .color(egui::Color32::from_rgb(150, 160, 170)),
                        );
                        ui.separator();
                    }
                    // Input focus indicator
                    if self.remote_input_focused {
                        ui.label(
                            egui::RichText::new("⌨ ввод захвачен  [Esc = отпустить]")
                                .size(11.0)
                                .color(egui::Color32::from_rgb(45, 160, 230)),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("наведите мышь → ввод")
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        );
                    }
                    // Pending indicator
                    if self.screenshot_pending {
                        ui.spinner();
                    }
                });
            });

            egui::CentralPanel::default().show(ctx, |ui| {
                self.remote_screen_ui(ui);
            });
        });
    }

    fn remote_screen_ui(&mut self, ui: &mut egui::Ui) {
        let available_size = ui.available_size_before_wrap();
        let available_width = available_size.x.max(1.0);
        let Some(texture) = self.remote_texture.clone() else {
            ui.allocate_ui(
                egui::vec2(available_width, available_size.y.max(360.0)),
                |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.spinner();
                        ui.label("Ожидание первого кадра…");
                    });
                },
            );
            return;
        };

        let [w, h] = self.remote_size;
        if w == 0 || h == 0 {
            return;
        }
        let max_height = available_size.y.max(360.0);
        let scale = if self.fit_to_window {
            (available_width / w as f32)
                .min(max_height / h as f32)
                .clamp(0.05, 4.0)
        } else {
            1.0
        };
        let size = egui::vec2(w as f32 * scale, h as f32 * scale);

        // Auto-capture keyboard when pointer hovers inside the remote screen.
        // Release when pointer leaves the screen area.
        let pointer_in_screen = ui
            .ctx()
            .input(|i| i.pointer.hover_pos())
            .map(|p| {
                // Approximate rect check: allocate enough to know where the image will land.
                // We'll refine after the response is known.
                let _ = p;
                true
            })
            .unwrap_or(false);
        let _ = pointer_in_screen; // refined below after response

        // During live VP9 streaming the Windows VP9 encoder (Desktop Duplication)
        // embeds cursor pixels directly in the captured frame, so the separate
        // cursor overlay is redundant.  Additionally, CursorId switch messages
        // currently fail to parse (proto wire-type mismatch in RustDesk 1.4.6),
        // which leaves the overlay stuck on the first shape (I-beam).
        // → Hide the overlay while VP9 is active; show the normal OS cursor.
        let live_vp9_active = self
            .last_live_frame_at
            .map(|t| t.elapsed() < Duration::from_secs(2))
            .unwrap_or(false);

        let hover_cursor = if self.cursor_texture.is_some() && !live_vp9_active {
            egui::CursorIcon::None
        } else {
            egui::CursorIcon::Default
        };
        let response = ui
            .add(
                egui::Image::new(&texture)
                    .fit_to_exact_size(size)
                    .sense(egui::Sense::click_and_drag()),
            )
            .on_hover_cursor(hover_cursor);

        // Auto-focus keyboard input when pointer is inside remote screen.
        if self.connected {
            let hovering = response.hovered()
                || response
                    .ctx
                    .input(|i| i.pointer.hover_pos())
                    .map(|p| response.rect.contains(p))
                    .unwrap_or(false);
            if hovering && !self.remote_input_focused {
                self.remote_input_focused = true;
            }
            // Release keyboard focus when pointer leaves the screen entirely.
            if !hovering && !response.is_pointer_button_down_on() {
                // Only release if we're not in the middle of a drag
                if !ui.ctx().input(|i| i.pointer.any_down()) {
                    self.remote_input_focused = false;
                }
            }
        }
        if !self.remote_input_focused {
            self.release_remote_modifiers();
        }

        // Draw a colored focus border around the remote screen when keyboard is captured.
        if self.remote_input_focused {
            let border_color = egui::Color32::from_rgb(45, 160, 230);
            ui.painter().rect_stroke(
                response.rect,
                0.0,
                egui::Stroke::new(2.0, border_color),
                egui::StrokeKind::Inside,
            );
        }

        // Draw remote cursor overlay on top of the video (PNG / screenshot mode only).
        // Skipped during live VP9 — see live_vp9_active comment above.
        if !live_vp9_active {
            if let Some((cursor_tex, hotx, hoty)) = &self.cursor_texture {
                let cursor_px = cursor_tex.size_vec2();
                let draw_pos = if let Some(rpos) = self.cursor_pos {
                    egui::pos2(
                        response.rect.min.x + (rpos.x / w as f32) * size.x - *hotx as f32 * scale,
                        response.rect.min.y + (rpos.y / h as f32) * size.y - *hoty as f32 * scale,
                    )
                } else if let Some(local) = response.hover_pos() {
                    egui::pos2(
                        local.x - *hotx as f32 * scale,
                        local.y - *hoty as f32 * scale,
                    )
                } else {
                    return;
                };
                let cursor_rect = egui::Rect::from_min_size(draw_pos, cursor_px * scale);
                ui.painter().image(
                    cursor_tex.id(),
                    cursor_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        } // end !live_vp9_active

        if self.connected {
            // Mouse movement
            let pointer_pos = response
                .interact_pointer_pos()
                .or_else(|| response.hover_pos());
            if let Some(pos) = pointer_pos.filter(|pos| response.rect.contains(*pos)) {
                let local = pos - response.rect.min;
                let (x, y) = self.remote_point_from_local(local.x / scale, local.y / scale);
                if self.last_mouse_pos != Some((x, y)) {
                    self.last_mouse_pos = Some((x, y));
                    self.send_command(SessionCommand::MouseMove { x, y });
                    if self.should_refresh_after_move() {
                        self.send_command(SessionCommand::Screenshot);
                    }
                }
            }

            // Mouse clicks & scroll
            {
                let events = ui.input(|input| input.events.clone());
                for event in &events {
                    match event {
                        egui::Event::PointerButton {
                            pos,
                            button,
                            pressed,
                            ..
                        } => {
                            let inside = response.rect.contains(*pos);
                            if *pressed && !inside {
                                continue;
                            }
                            let (x, y) = if inside {
                                let local = *pos - response.rect.min;
                                self.remote_point_from_local(local.x / scale, local.y / scale)
                            } else {
                                self.last_mouse_pos.unwrap_or((0, 0))
                            };
                            match (button, pressed) {
                                (egui::PointerButton::Primary, true) => {
                                    self.send_command(SessionCommand::MouseDown { x, y });
                                    self.request_visual_refresh_after_input();
                                }
                                (egui::PointerButton::Primary, false) => {
                                    self.send_command(SessionCommand::MouseUp { x, y });
                                    self.request_visual_refresh_after_input();
                                }
                                (egui::PointerButton::Secondary, true) => {
                                    self.send_command(SessionCommand::MouseRightDown { x, y });
                                    self.request_visual_refresh_after_input();
                                }
                                (egui::PointerButton::Secondary, false) => {
                                    self.send_command(SessionCommand::MouseRightUp { x, y });
                                    self.request_visual_refresh_after_input();
                                }
                                (egui::PointerButton::Middle, true) => {
                                    self.send_command(SessionCommand::MouseMiddleDown { x, y });
                                    self.request_visual_refresh_after_input();
                                }
                                (egui::PointerButton::Middle, false) => {
                                    self.send_command(SessionCommand::MouseMiddleUp { x, y });
                                    self.request_visual_refresh_after_input();
                                }
                                _ => {}
                            }
                        }
                        egui::Event::MouseWheel { unit, delta, .. } => {
                            if let Some((x, y)) = self.wheel_delta(*unit, *delta) {
                                self.send_command(SessionCommand::MouseWheel { x, y });
                                self.request_visual_refresh_after_input();
                            }
                        }
                        _ => {}
                    }
                }
            }

            // Keyboard (active when pointer is over screen, no UI widget has focus)
            if self.remote_input_focused && !ui.ctx().egui_wants_keyboard_input() {
                self.handle_remote_keyboard(ui.ctx());
            }
        }
    }

    fn handle_remote_keyboard(&mut self, ctx: &egui::Context) {
        let current_modifiers = ctx.input(|input| input.modifiers);
        self.sync_remote_modifiers(current_modifiers);

        let events = ctx.input(|input| input.events.clone());
        for event in events {
            match event {
                // Paste from local clipboard → send to remote as text
                egui::Event::Paste(text) if !text.is_empty() => {
                    self.send_command(SessionCommand::KeyText(text));
                    self.request_visual_refresh_after_input();
                }
                // Printable characters from the OS input method (handles all layouts/IME)
                egui::Event::Text(text) if !text.is_empty() => {
                    self.send_command(SessionCommand::KeyText(text));
                    self.request_visual_refresh_after_input();
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } => {
                    // Ctrl+Escape releases keyboard capture
                    if key == egui::Key::Escape && modifiers.ctrl {
                        self.remote_input_focused = false;
                        self.release_remote_modifiers();
                        continue;
                    }
                    // Escape alone also releases (press Ctrl to send real Escape to remote)
                    if key == egui::Key::Escape && !has_command_modifier(modifiers) {
                        self.remote_input_focused = false;
                        self.release_remote_modifiers();
                        continue;
                    }
                    // Allow key-repeat only for navigation/edit keys (arrows, backspace, delete,
                    // page-up/down, home, end). Other keys skip repeat to avoid duplicates.
                    if repeat && !key_allows_repeat(key) {
                        continue;
                    }
                    if has_command_modifier(modifiers) && egui_key_to_text(key).is_some() {
                        let text = egui_key_to_text(key).unwrap();
                        self.send_command(SessionCommand::KeyText(text));
                        self.request_visual_refresh_after_input();
                    } else if let Some(control_key) = egui_key_to_control_key(key) {
                        self.send_command(SessionCommand::KeyControl(control_key));
                        self.request_visual_refresh_after_input();
                    }
                }
                _ => {}
            }
        }
    }

    fn sync_remote_modifiers(&mut self, modifiers: egui::Modifiers) {
        let next = RemoteModifierState::from_egui(modifiers);
        if next == self.remote_modifiers_down {
            return;
        }

        let previous = self.remote_modifiers_down;
        previous.for_each(|key, was_down| {
            let now_down = remote_modifier_is_down(next, key);
            if was_down && !now_down {
                self.send_command(SessionCommand::KeyControlState { key, down: false });
            }
        });
        next.for_each(|key, now_down| {
            let was_down = remote_modifier_is_down(previous, key);
            if now_down && !was_down {
                self.send_command(SessionCommand::KeyControlState { key, down: true });
            }
        });
        self.remote_modifiers_down = next;
    }

    fn release_remote_modifiers(&mut self) {
        let previous = self.remote_modifiers_down;
        if previous == RemoteModifierState::default() {
            return;
        }
        previous.for_each(|key, was_down| {
            if was_down {
                self.send_command(SessionCommand::KeyControlState { key, down: false });
            }
        });
        self.remote_modifiers_down = RemoteModifierState::default();
    }

    fn wheel_delta(&mut self, unit: egui::MouseWheelUnit, delta: egui::Vec2) -> Option<(i32, i32)> {
        let scaled = match unit {
            egui::MouseWheelUnit::Point => delta / 40.0,
            egui::MouseWheelUnit::Line => delta,
            egui::MouseWheelUnit::Page => delta * 8.0,
        };
        self.wheel_accum += scaled;
        let x = self.wheel_accum.x.trunc() as i32;
        let y = self.wheel_accum.y.trunc() as i32;
        self.wheel_accum.x -= x as f32;
        self.wheel_accum.y -= y as f32;
        if x == 0 && y == 0 {
            None
        } else {
            Some((x, y))
        }
    }

    fn last_screenshot_age_ms(&self) -> Option<u128> {
        self.last_screenshot_at
            .map(|instant| instant.elapsed().as_millis())
    }

    fn should_refresh_after_move(&mut self) -> bool {
        if self.live_video_active() {
            return false;
        }
        // Throttle move-triggered refreshes to once per 60 ms regardless of auto_refresh.
        // auto_refresh already fires every ~80 ms, so this adds at most one extra request
        // between auto-refresh ticks, keeping the view fresh while dragging.
        if self.screenshot_pending {
            return false;
        }
        let now = Instant::now();
        let should_refresh = self
            .last_move_refresh_at
            .map(|last| now.duration_since(last) >= Duration::from_millis(60))
            .unwrap_or(true);
        if should_refresh {
            self.last_move_refresh_at = Some(now);
        }
        should_refresh
    }

    fn request_visual_refresh_after_input(&mut self) {
        // Always request a fresh screenshot after user input so the screen reflects
        // the action immediately — don't wait for the periodic auto-refresh timer.
        // Skip only if a request is already in flight to avoid flooding the server.
        if !self.screenshot_pending && !self.live_video_active() {
            self.send_command(SessionCommand::Screenshot);
        }
    }

    fn live_video_active(&self) -> bool {
        self.last_frame_codec != "PNG"
            && self
                .last_live_frame_at
                .map(|instant| instant.elapsed() < Duration::from_secs(2))
                .unwrap_or(false)
    }

    fn paste_local_clipboard_to_remote(&mut self) {
        match read_local_clipboard_text() {
            Ok(text) if text.trim().is_empty() => {
                self.clipboard_status = Some("буфер пуст".to_owned());
            }
            Ok(text) => {
                let chars = text.chars().count();
                self.send_command(SessionCommand::KeyText(text));
                self.request_visual_refresh_after_input();
                self.clipboard_status = Some(format!("буфер отправлен: {chars} симв."));
                self.log(format!("Clipboard sent to remote: {chars} chars"));
            }
            Err(err) => {
                self.clipboard_status = Some("буфер недоступен".to_owned());
                self.log(format!("Clipboard read failed: {err}"));
            }
        }
    }

    fn save_current_frame_png(&mut self) {
        if self.last_frame_rgba.is_empty() || self.remote_size[0] == 0 || self.remote_size[1] == 0 {
            self.screenshot_status = Some("нет кадра для сохранения".to_owned());
            return;
        }

        match save_rgba_png(
            &self.remote_id,
            self.remote_size[0],
            self.remote_size[1],
            &self.last_frame_rgba,
        ) {
            Ok(path) => {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("screenshot.png");
                self.screenshot_status = Some(format!("сохранено: {name}"));
                self.log(format!("Screenshot saved: {}", path.display()));
            }
            Err(err) => {
                self.screenshot_status = Some("сохранение не удалось".to_owned());
                self.log(format!("Screenshot save failed: {err}"));
            }
        }
    }

    fn save_session_log_file(&mut self) {
        match save_session_log(&self.remote_id, &self.session_log) {
            Ok(path) => {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("session.log");
                self.log_status = Some(format!("лог сохранён: {name}"));
                self.log(format!("Session log saved: {}", path.display()));
            }
            Err(err) => {
                self.log_status = Some("лог не сохранён".to_owned());
                self.log(format!("Session log save failed: {err}"));
            }
        }
    }

    fn save_support_report(&mut self) {
        let report = SupportReport {
            remote_id: self.remote_id.clone(),
            connected: self.connected,
            codec: self.last_frame_codec.clone(),
            fps: self.display_fps,
            input_fps: self.stream_input_fps,
            input_kbps: self.stream_input_kbps,
            latency_ms: self.latency_ms,
            frame_size: self.remote_size,
            frame_bytes: self.frame_bytes,
            queue_ms: self.frame_queue_ms,
            decode_ms: self.frame_decode_ms,
            dropped: self.frame_dropped,
            stream_health: self.stream_health.clone(),
            screenshot_count: self.screenshot_count,
            live_frame_count: self.live_frame_count,
            screenshot_frame_count: self.screenshot_frame_count,
            input_events_sent: self.input_events_sent,
            last_frame_rgba: self.last_frame_rgba.clone(),
            session_log: self.session_log.clone(),
        };

        match save_support_report_bundle(report) {
            Ok(dir) => {
                let name = dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("report");
                self.report_status = Some(format!("отчёт собран: {name}"));
                self.log(format!("Support report saved: {}", dir.display()));
            }
            Err(err) => {
                self.report_status = Some("отчёт не собран".to_owned());
                self.log(format!("Support report save failed: {err}"));
            }
        }
    }

    fn remote_point_from_local(&self, x: f32, y: f32) -> (i32, i32) {
        let max_x = self.remote_size[0].saturating_sub(1) as f32;
        let max_y = self.remote_size[1].saturating_sub(1) as f32;
        let x = x.clamp(0.0, max_x).round() as i32;
        let y = y.clamp(0.0, max_y).round() as i32;
        let (offset_x, offset_y) = self.coordinate_offset();
        (offset_x + x, offset_y + y)
    }

    fn coordinate_offset(&self) -> (i32, i32) {
        let Some(display) = self
            .remote_displays
            .iter()
            .find(|display| display.index == self.selected_display)
        else {
            return (0, 0);
        };

        match self.coordinate_mode {
            CoordinateMode::Local => (0, 0),
            CoordinateMode::Absolute => (display.x, display.y),
            CoordinateMode::Auto => {
                if self.remote_displays.len() > 1 || display.x != 0 || display.y != 0 {
                    (display.x, display.y)
                } else {
                    (0, 0)
                }
            }
        }
    }
}

fn has_command_modifier(modifiers: egui::Modifiers) -> bool {
    modifiers.ctrl || modifiers.alt || modifiers.mac_cmd || modifiers.command
}

fn remote_modifier_is_down(state: RemoteModifierState, key: ControlKey) -> bool {
    match key {
        ControlKey::Alt => state.alt,
        ControlKey::Shift => state.shift,
        ControlKey::Control => state.ctrl,
        ControlKey::Meta => state.meta,
        _ => false,
    }
}

fn command_is_input(command: &SessionCommand) -> bool {
    matches!(
        command,
        SessionCommand::MouseMove { .. }
            | SessionCommand::MouseDown { .. }
            | SessionCommand::MouseUp { .. }
            | SessionCommand::MouseRightDown { .. }
            | SessionCommand::MouseRightUp { .. }
            | SessionCommand::MouseMiddleDown { .. }
            | SessionCommand::MouseMiddleUp { .. }
            | SessionCommand::MouseWheel { .. }
            | SessionCommand::KeyText(_)
            | SessionCommand::KeyControl(_)
            | SessionCommand::KeyControlState { .. }
            | SessionCommand::KeyTextWithModifiers { .. }
            | SessionCommand::KeyControlWithModifiers { .. }
            | SessionCommand::KeyEnter
            | SessionCommand::ShellInput(_)
    )
}

fn display_label(display: &RemoteDisplay) -> String {
    format!(
        "{}: {}x{} @ {},{}",
        display.name,
        display.width.max(0),
        display.height.max(0),
        display.x,
        display.y
    )
}

fn coordinate_mode_label(mode: CoordinateMode) -> &'static str {
    match mode {
        CoordinateMode::Auto => "Coord: Auto",
        CoordinateMode::Absolute => "Coord: Absolute",
        CoordinateMode::Local => "Coord: Local",
    }
}

fn stream_health_color(text: &str) -> egui::Color32 {
    if text.contains("стабилен") {
        egui::Color32::from_rgb(80, 200, 100)
    } else if text.contains("ожидание") || text.contains("PNG fallback") {
        egui::Color32::from_rgb(220, 180, 60)
    } else {
        egui::Color32::from_rgb(220, 110, 80)
    }
}

fn remote_texture_options() -> egui::TextureOptions {
    egui::TextureOptions {
        magnification: egui::TextureFilter::Nearest,
        minification: egui::TextureFilter::Linear,
        wrap_mode: egui::TextureWrapMode::ClampToEdge,
        mipmap_mode: None,
    }
}

struct SupportReport {
    remote_id: String,
    connected: bool,
    codec: String,
    fps: f32,
    input_fps: f32,
    input_kbps: u64,
    latency_ms: Option<u32>,
    frame_size: [usize; 2],
    frame_bytes: usize,
    queue_ms: u64,
    decode_ms: u64,
    dropped: usize,
    stream_health: String,
    screenshot_count: u64,
    live_frame_count: u64,
    screenshot_frame_count: u64,
    input_events_sent: u64,
    last_frame_rgba: Vec<u8>,
    session_log: Vec<String>,
}

fn read_local_clipboard_text() -> Result<String, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|err| format!("Clipboard init failed: {err}"))?;
    clipboard
        .get_text()
        .map_err(|err| format!("Clipboard text read failed: {err}"))
}

fn save_rgba_png(
    remote_id: &str,
    width: usize,
    height: usize,
    rgba: &[u8],
) -> Result<PathBuf, String> {
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "Frame size overflow".to_owned())?;
    if rgba.len() != expected {
        return Err(format!(
            "Frame buffer size mismatch: got {}, expected {expected}",
            rgba.len()
        ));
    }

    let dir = PathBuf::from("screenshots");
    fs::create_dir_all(&dir).map_err(|err| format!("Create screenshots dir failed: {err}"))?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("System time error: {err}"))?
        .as_secs();
    let id = sanitize_filename(remote_id);
    let path = dir.join(format!("evertydesk-{id}-{timestamp}.png"));
    image::save_buffer_with_format(
        &path,
        rgba,
        width as u32,
        height as u32,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|err| format!("PNG save failed: {err}"))?;
    Ok(path)
}

fn save_session_log(remote_id: &str, lines: &[String]) -> Result<PathBuf, String> {
    let dir = PathBuf::from("logs");
    fs::create_dir_all(&dir).map_err(|err| format!("Create logs dir failed: {err}"))?;
    let timestamp = unix_timestamp_secs();
    let id = sanitize_filename(remote_id);
    let path = dir.join(format!("evertydesk-{id}-{timestamp}.log"));
    let mut body = String::new();
    body.push_str("EvertyDesk Lite session log\n");
    body.push_str(&format!("remote_id={remote_id}\n"));
    body.push_str(&format!("created_at_unix={timestamp}\n\n"));
    for line in lines {
        body.push_str(line);
        body.push('\n');
    }
    fs::write(&path, body).map_err(|err| format!("Write log failed: {err}"))?;
    Ok(path)
}

fn save_support_report_bundle(report: SupportReport) -> Result<PathBuf, String> {
    let timestamp = unix_timestamp_secs();
    let id = sanitize_filename(&report.remote_id);
    let dir = PathBuf::from("reports").join(format!("evertydesk-{id}-{timestamp}"));
    fs::create_dir_all(&dir).map_err(|err| format!("Create report dir failed: {err}"))?;

    let summary_path = dir.join("summary.txt");
    fs::write(&summary_path, support_report_summary(&report, timestamp))
        .map_err(|err| format!("Write report summary failed: {err}"))?;

    let log_path = dir.join("session.log");
    let mut log_body = String::new();
    log_body.push_str("EvertyDesk Lite session log\n\n");
    for line in &report.session_log {
        log_body.push_str(line);
        log_body.push('\n');
    }
    fs::write(&log_path, log_body).map_err(|err| format!("Write report log failed: {err}"))?;

    if !report.last_frame_rgba.is_empty() && report.frame_size[0] > 0 && report.frame_size[1] > 0 {
        let shot_path = dir.join("screen.png");
        image::save_buffer_with_format(
            &shot_path,
            &report.last_frame_rgba,
            report.frame_size[0] as u32,
            report.frame_size[1] as u32,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|err| format!("Write report screen failed: {err}"))?;
    }

    Ok(dir)
}

fn support_report_summary(report: &SupportReport, timestamp: u64) -> String {
    let latency = report
        .latency_ms
        .map(|ms| ms.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    format!(
        "EvertyDesk Lite support report\n\
         created_at_unix={timestamp}\n\
         remote_id={}\n\
         connected={}\n\
         codec={}\n\
         fps={:.1}\n\
         input_fps={:.1}\n\
         input_kbps={}\n\
         latency_ms={latency}\n\
         frame={}x{}\n\
         frame_bytes={}\n\
         queue_ms={}\n\
         decode_ms={}\n\
         dropped={}\n\
         stream_health={}\n\
         screenshot_count={}\n\
         live_frame_count={}\n\
         screenshot_frame_count={}\n\
         input_events_sent={}\n",
        report.remote_id,
        report.connected,
        report.codec,
        report.fps,
        report.input_fps,
        report.input_kbps,
        report.frame_size[0],
        report.frame_size[1],
        report.frame_bytes,
        report.queue_ms,
        report.decode_ms,
        report.dropped,
        report.stream_health,
        report.screenshot_count,
        report.live_frame_count,
        report.screenshot_frame_count,
        report.input_events_sent
    )
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// "HH:MM:SS" wall-clock for log lines.
fn timestamp_hms() -> String {
    let secs = unix_timestamp_secs();
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

fn compact_host_video_status(summary: &str) -> String {
    let mut parts = Vec::new();
    for key in [
        "active_backend",
        "codec",
        "size",
        "fps",
        "bitrate",
        "packets",
        "avg_packet",
        "capture_avg",
        "capture_max",
        "change_avg",
        "encode_avg",
        "encode_max",
        "send_avg",
        "fallbacks",
        "reason",
    ] {
        if let Some(value) = telemetry_value(summary, key) {
            parts.push(format!("{key}={value}"));
        }
    }
    if parts.is_empty() {
        summary.chars().take(160).collect()
    } else {
        parts.join(" ")
    }
}

fn telemetry_value<'a>(summary: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=");
    let start = summary.find(&needle)? + needle.len();
    let rest = &summary[start..];
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(&stripped[..end])
    } else {
        let end = rest.find(' ').unwrap_or(rest.len());
        Some(&rest[..end])
    }
}

/// Format 9-digit peer ID as "XXX XXX XXX" (like RustDesk).
fn format_peer_id(id: &str) -> String {
    let digits: String = id.chars().filter(|c| c.is_ascii_digit()).collect();
    match digits.len() {
        9 => format!("{} {} {}", &digits[0..3], &digits[3..6], &digits[6..9]),
        _ => id.to_owned(),
    }
}

fn sanitize_filename(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "remote".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn egui_key_to_control_key(key: egui::Key) -> Option<ControlKey> {
    Some(match key {
        egui::Key::ArrowDown => ControlKey::DownArrow,
        egui::Key::ArrowLeft => ControlKey::LeftArrow,
        egui::Key::ArrowRight => ControlKey::RightArrow,
        egui::Key::ArrowUp => ControlKey::UpArrow,
        egui::Key::Escape => ControlKey::Escape,
        egui::Key::Tab => ControlKey::Tab,
        egui::Key::Backspace => ControlKey::Backspace,
        egui::Key::Enter => ControlKey::Return,
        egui::Key::Space => ControlKey::Space,
        egui::Key::Insert => ControlKey::Insert,
        egui::Key::Delete => ControlKey::Delete,
        egui::Key::Home => ControlKey::Home,
        egui::Key::End => ControlKey::End,
        egui::Key::PageUp => ControlKey::PageUp,
        egui::Key::PageDown => ControlKey::PageDown,
        egui::Key::F1 => ControlKey::F1,
        egui::Key::F2 => ControlKey::F2,
        egui::Key::F3 => ControlKey::F3,
        egui::Key::F4 => ControlKey::F4,
        egui::Key::F5 => ControlKey::F5,
        egui::Key::F6 => ControlKey::F6,
        egui::Key::F7 => ControlKey::F7,
        egui::Key::F8 => ControlKey::F8,
        egui::Key::F9 => ControlKey::F9,
        egui::Key::F10 => ControlKey::F10,
        egui::Key::F11 => ControlKey::F11,
        egui::Key::F12 => ControlKey::F12,
        _ => return None,
    })
}

/// Keys that should fire repeatedly when held down (navigation, delete, etc.)
fn key_allows_repeat(key: egui::Key) -> bool {
    matches!(
        key,
        egui::Key::ArrowDown
            | egui::Key::ArrowLeft
            | egui::Key::ArrowRight
            | egui::Key::ArrowUp
            | egui::Key::Backspace
            | egui::Key::Delete
            | egui::Key::PageDown
            | egui::Key::PageUp
            | egui::Key::Home
            | egui::Key::End
            | egui::Key::Tab
    )
}

fn egui_key_to_text(key: egui::Key) -> Option<String> {
    let ch = match key {
        egui::Key::A => 'a',
        egui::Key::B => 'b',
        egui::Key::C => 'c',
        egui::Key::D => 'd',
        egui::Key::E => 'e',
        egui::Key::F => 'f',
        egui::Key::G => 'g',
        egui::Key::H => 'h',
        egui::Key::I => 'i',
        egui::Key::J => 'j',
        egui::Key::K => 'k',
        egui::Key::L => 'l',
        egui::Key::M => 'm',
        egui::Key::N => 'n',
        egui::Key::O => 'o',
        egui::Key::P => 'p',
        egui::Key::Q => 'q',
        egui::Key::R => 'r',
        egui::Key::S => 's',
        egui::Key::T => 't',
        egui::Key::U => 'u',
        egui::Key::V => 'v',
        egui::Key::W => 'w',
        egui::Key::X => 'x',
        egui::Key::Y => 'y',
        egui::Key::Z => 'z',
        egui::Key::Num0 => '0',
        egui::Key::Num1 => '1',
        egui::Key::Num2 => '2',
        egui::Key::Num3 => '3',
        egui::Key::Num4 => '4',
        egui::Key::Num5 => '5',
        egui::Key::Num6 => '6',
        egui::Key::Num7 => '7',
        egui::Key::Num8 => '8',
        egui::Key::Num9 => '9',
        _ => return None,
    };
    Some(ch.to_string())
}

fn configure_ui_scale(ctx: &egui::Context) {
    let default_zoom = if cfg!(target_os = "macos") { 1.08 } else { 1.0 };
    let zoom = std::env::var("EVERTYDESK_UI_SCALE")
        .ok()
        .and_then(|value| value.trim().parse::<f32>().ok())
        .unwrap_or(default_zoom)
        .clamp(0.75, 1.75);
    ctx.set_zoom_factor(zoom);
}

fn configure_style(ctx: &egui::Context) {
    use egui::{Color32, CornerRadius, Stroke};

    let bg = Color32::from_rgb(0xFB, 0xFC, 0xFE);
    let panel = Color32::from_rgb(0xFF, 0xFF, 0xFF);
    let card = Color32::from_rgb(0xFF, 0xFF, 0xFF);
    let input = Color32::from_rgb(0xFF, 0xFF, 0xFF);
    let border = Color32::from_rgb(0xE3, 0xE6, 0xEC);
    let border_hover = Color32::from_rgb(0xD8, 0xDC, 0xE4);
    let border_focus = Color32::from_rgb(0xC7, 0xCD, 0xD8);
    let text = Color32::from_rgb(0x13, 0x17, 0x21);
    let text_weak = Color32::from_rgb(0x67, 0x70, 0x80);
    let text_strong = Color32::from_rgb(0x22, 0x27, 0x32);

    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = bg;
    visuals.window_fill = panel;
    visuals.extreme_bg_color = input;
    visuals.faint_bg_color = card;
    visuals.window_stroke = Stroke::new(1.0, border);

    visuals.selection.bg_fill = Color32::from_rgb(0xB9, 0xD7, 0xFF);
    visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(0x3D, 0x73, 0xB8));

    let rounding = CornerRadius::same(12);
    visuals.widgets.noninteractive.bg_fill = card;
    visuals.widgets.noninteractive.weak_bg_fill = card;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text_weak);
    visuals.widgets.noninteractive.corner_radius = rounding;

    visuals.widgets.inactive.bg_fill = input;
    visuals.widgets.inactive.weak_bg_fill = input;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, border);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text_strong);
    visuals.widgets.inactive.corner_radius = rounding;

    visuals.widgets.hovered.bg_fill = Color32::from_rgb(0xFA, 0xFB, 0xFD);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(0xFA, 0xFB, 0xFD);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, border_hover);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, text);
    visuals.widgets.hovered.corner_radius = rounding;

    visuals.widgets.active.bg_fill = Color32::from_rgb(0xF3, 0xF5, 0xF8);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(0xF3, 0xF5, 0xF8);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, border_focus);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, text);
    visuals.widgets.active.corner_radius = rounding;

    visuals.widgets.open.bg_fill = card;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, border);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, text);
    visuals.widgets.open.corner_radius = rounding;

    ctx.set_visuals(visuals);

    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(12.0, 9.0);
    style.spacing.button_padding = egui::vec2(16.0, 9.0);
    style.spacing.menu_margin = egui::Margin::same(8);
    style.spacing.window_margin = egui::Margin::same(16);
    style.spacing.interact_size.y = 34.0;
    ctx.set_global_style(style);
}

#[allow(dead_code)]
const SERVICE_NAME: &str = "EvertyDeskLite";

fn install_host_service() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|err| format!("current_exe: {err}"))?;
    #[cfg(target_os = "windows")]
    {
        let bin = format!("\"{}\" --host", exe.display());
        let status = std::process::Command::new("sc.exe")
            .args(["create", SERVICE_NAME, "binPath=", &bin, "start=", "auto"])
            .status()
            .map_err(|err| format!("sc create failed: {err}"))?;
        return if status.success() {
            Ok("Windows service installed. Admin rights may be required.".to_owned())
        } else {
            Err(format!("sc create exited with {status}"))
        };
    }
    #[cfg(target_os = "linux")]
    {
        let dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_owned())?
            .join(".config/systemd/user");
        fs::create_dir_all(&dir).map_err(|err| format!("create systemd dir: {err}"))?;
        let unit = dir.join("evertydesk-lite.service");
        let body = format!(
            "[Unit]\nDescription=EvertyDesk Lite host service\nAfter=network-online.target\n\n\
             [Service]\nExecStart={} --host\nRestart=always\nRestartSec=5\n\n\
             [Install]\nWantedBy=default.target\n",
            exe.display()
        );
        fs::write(&unit, body).map_err(|err| format!("write service file: {err}"))?;
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "enable", "evertydesk-lite.service"])
            .status();
        return Ok(format!(
            "User systemd service installed: {}",
            unit.display()
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_owned())?
            .join("Library/LaunchAgents");
        fs::create_dir_all(&dir).map_err(|err| format!("create LaunchAgents dir: {err}"))?;
        let plist = dir.join("ru.everty.evertydesk-lite.plist");
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>ru.everty.evertydesk-lite</string>
<key>ProgramArguments</key><array><string>{}</string><string>--host</string></array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
</dict></plist>"#,
            exe.display()
        );
        fs::write(&plist, body).map_err(|err| format!("write plist: {err}"))?;
        return Ok(format!("LaunchAgent installed: {}", plist.display()));
    }
    #[allow(unreachable_code)]
    Err("Service install is not implemented for this OS".to_owned())
}

fn start_installed_service() -> Result<String, String> {
    run_service_command("start")
}

fn stop_installed_service() -> Result<String, String> {
    run_service_command("stop")
}

fn uninstall_host_service() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("sc.exe")
            .args(["stop", SERVICE_NAME])
            .status();
        let status = std::process::Command::new("sc.exe")
            .args(["delete", SERVICE_NAME])
            .status()
            .map_err(|err| format!("sc delete failed: {err}"))?;
        return if status.success() {
            Ok("Windows service removed.".to_owned())
        } else {
            Err(format!("sc delete exited with {status}"))
        };
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "disable", "--now", "evertydesk-lite.service"])
            .status();
        let unit = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_owned())?
            .join(".config/systemd/user/evertydesk-lite.service");
        let _ = fs::remove_file(&unit);
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        return Ok("User systemd service removed.".to_owned());
    }
    #[cfg(target_os = "macos")]
    {
        let plist = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_owned())?
            .join("Library/LaunchAgents/ru.everty.evertydesk-lite.plist");
        let _ = std::process::Command::new("launchctl")
            .args(["unload", plist.to_string_lossy().as_ref()])
            .status();
        let _ = fs::remove_file(&plist);
        return Ok("LaunchAgent removed.".to_owned());
    }
    #[allow(unreachable_code)]
    Err("Service uninstall is not implemented for this OS".to_owned())
}

fn run_service_command(action: &str) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        let status = std::process::Command::new("sc.exe")
            .args([action, SERVICE_NAME])
            .status()
            .map_err(|err| format!("sc {action} failed: {err}"))?;
        return if status.success() {
            Ok(format!("Windows service {action} requested."))
        } else {
            Err(format!("sc {action} exited with {status}"))
        };
    }
    #[cfg(target_os = "linux")]
    {
        let status = std::process::Command::new("systemctl")
            .args(["--user", action, "evertydesk-lite.service"])
            .status()
            .map_err(|err| format!("systemctl {action} failed: {err}"))?;
        return if status.success() {
            Ok(format!("systemd user service {action} requested."))
        } else {
            Err(format!("systemctl exited with {status}"))
        };
    }
    #[cfg(target_os = "macos")]
    {
        let plist = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_owned())?
            .join("Library/LaunchAgents/ru.everty.evertydesk-lite.plist");
        let cmd = if action == "start" { "load" } else { "unload" };
        let status = std::process::Command::new("launchctl")
            .args([cmd, plist.to_string_lossy().as_ref()])
            .status()
            .map_err(|err| format!("launchctl {cmd} failed: {err}"))?;
        return if status.success() {
            Ok(format!("LaunchAgent {cmd} requested."))
        } else {
            Err(format!("launchctl exited with {status}"))
        };
    }
    #[allow(unreachable_code)]
    Err("Service control is not implemented for this OS".to_owned())
}

/// A `label … value` row for the This Computer info block.
#[allow(dead_code)]
fn info_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .size(13.0)
                .color(egui::Color32::from_rgb(0x80, 0x80, 0x80)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(value)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(0xCC, 0xCC, 0xCC)),
            );
        });
    });
    ui.add_space(9.0);
}

/// Try to read the 32-byte Ed25519 public key from the installed EvertyDesk
/// (or RustDesk) client config.  Returns `None` if the config is not found
/// or cannot be parsed.
///
/// Priority:
/// 1. `%APPDATA%\EvertyDesk\config\Evertydesk.toml`
/// 2. `%APPDATA%\RustDesk\config\RustDesk.toml`
fn load_everty_public_key() -> Option<Vec<u8>> {
    let appdata = std::env::var_os("APPDATA")?;
    let appdata = std::path::PathBuf::from(appdata);

    let candidates = [
        appdata
            .join("EvertyDesk")
            .join("config")
            .join("Evertydesk.toml"),
        appdata
            .join("RustDesk")
            .join("config")
            .join("RustDesk.toml"),
    ];

    for path in &candidates {
        if let Ok(raw) = std::fs::read_to_string(path) {
            // key_pair in TOML is two arrays of integers.
            // key_pair[1] is the 32-byte public key.
            if let Some(pk) = parse_key_pair_public_key(&raw) {
                eprintln!("[cli] Loaded key from {}", path.display());
                return Some(pk);
            }
        }
    }
    None
}

/// Parses the `key_pair` from a RustDesk/EvertyDesk TOML config string and
/// returns the 32-byte public key (the second inner array).
fn parse_key_pair_public_key(toml_text: &str) -> Option<Vec<u8>> {
    // The key_pair field is written as two arrays-of-integers.
    // We use a simple regex-free approach: find each bracketed number list.
    let kp_start = toml_text.find("key_pair")?;
    let text = &toml_text[kp_start..];

    // Find all outer `[` brackets (each sub-array)
    let mut arrays: Vec<Vec<u8>> = Vec::new();
    let mut depth = 0i32;
    let mut cur: Vec<u8> = Vec::new();
    let mut num_buf = String::new();

    for ch in text.chars() {
        match ch {
            '[' => {
                depth += 1;
                if depth == 2 {
                    cur.clear();
                }
            }
            ']' => {
                if depth == 2 {
                    // flush last number
                    if !num_buf.is_empty() {
                        if let Ok(n) = num_buf.trim().parse::<u8>() {
                            cur.push(n);
                        }
                        num_buf.clear();
                    }
                    arrays.push(cur.clone());
                }
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            ',' if depth == 2 => {
                if !num_buf.is_empty() {
                    if let Ok(n) = num_buf.trim().parse::<u8>() {
                        cur.push(n);
                    }
                    num_buf.clear();
                }
            }
            c if (c.is_ascii_digit() || c == '-') && depth == 2 => {
                num_buf.push(c);
            }
            _ if depth == 2 => {
                // whitespace/newline — ignore
                if !num_buf.is_empty() {
                    if let Ok(n) = num_buf.trim().parse::<u8>() {
                        cur.push(n);
                    }
                    num_buf.clear();
                }
            }
            _ => {}
        }
    }

    // key_pair = [ [64 bytes private+pub], [32 bytes public] ]
    // We want the second array (index 1), length must be 32.
    arrays.into_iter().find(|a| a.len() == 32)
}

fn normalize_remote_id(id: &str) -> String {
    id.chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-' && *ch != '_')
        .collect()
}

fn is_own_remote_id(remote_id: &str, local_id: &str) -> bool {
    let remote_id = normalize_remote_id(remote_id);
    let local_id = normalize_remote_id(local_id);
    !remote_id.is_empty() && remote_id == local_id
}

fn remember_remote_id(recent: &mut Vec<String>, id: &str) {
    let id = normalize_remote_id(id);
    if id.is_empty() {
        return;
    }
    recent.retain(|existing| existing != &id);
    recent.insert(0, id);
    recent.truncate(8);
}

fn remember_history(history: &mut Vec<ConnectionHistoryEntry>, id: &str) {
    let id = normalize_remote_id(id);
    if id.is_empty() {
        return;
    }
    let now = unix_timestamp_secs();
    if let Some(entry) = history.iter_mut().find(|entry| entry.remote_id == id) {
        entry.last_connected_unix = now;
        entry.connect_count = entry.connect_count.saturating_add(1);
    } else {
        history.insert(
            0,
            ConnectionHistoryEntry {
                remote_id: id,
                note: String::new(),
                last_connected_unix: now,
                connect_count: 1,
            },
        );
    }
    history.sort_by(|a, b| b.last_connected_unix.cmp(&a.last_connected_unix));
    history.truncate(20);
}

fn friendly_error(error: &str) -> String {
    if error.contains("Wrong Password") {
        "Неверный пароль. Проверьте пароль на удаленном ПК.".to_owned()
    } else if error.contains("Offline:") || error.contains("Rendezvous refused: Offline") {
        "Удаленный ID сейчас не в сети.".to_owned()
    } else if error.contains("ID does not exist") {
        "Такой ID не найден на сервере.".to_owned()
    } else if error.contains("Введите ID") || error.contains("Введите пароль") {
        error.to_owned()
    } else if error.contains("Background task stopped unexpectedly") {
        "Соединение неожиданно остановилось.".to_owned()
    } else {
        error.to_owned()
    }
}
