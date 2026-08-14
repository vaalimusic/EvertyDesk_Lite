#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Minimal RDP-to-VM console viewer — connects straight to a VM's RDP
//! endpoint (currently: Hyper-V Enhanced Session via the host's `vmconnect`
//! broker) and never touches EvertyDesk's own host/relay/transport at all.
//!
//! Deliberately a separate, much smaller binary from `evertydesk-viewer`
//! rather than a mode bolted onto it: the two have almost nothing in
//! common (no clipboard sync, no audio, no EVRT2, no toolbar), and keeping
//! them apart means this can't regress the RustDesk/EVRT viewer.
//!
//! v1 scope: Hyper-V Enhanced Session only. VirtualBox VRDE console needs
//! its own port-discovery plumbing that doesn't exist in desktop-next yet
//! (the old egui client tracks it in a separate `vbox_vrde_ports` map) —
//! left for a follow-up rather than guessed at here.

use std::fs;
use std::io::{self, BufReader};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use evertydesk_desktop_next::frame_renderer::{FrameRenderer, ScalingMode};
use evertydesk_desktop_next::ipc::{read_bounded_line, MAX_IPC_LINE_BYTES};
use evertydesk_desktop_next::protocol::{RdpBootstrap, RdpTarget};
use evertydesk_desktop_next::startup_log::{append_log_line, install_process_diagnostics};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

const DEFAULT_WIDTH: u16 = 1280;
const DEFAULT_HEIGHT: u16 = 800;
/// How often to poll the session thread's frame/status channels.
const POLL_INTERVAL: Duration = Duration::from_millis(16);
#[cfg(windows)]
const VBOX_RECONNECT_COOLDOWN: Duration =
    Duration::from_secs(evertydesk_core::vm_console_runtime::VBOX_RECONNECT_COOLDOWN_SECS);
#[cfg(not(windows))]
const VBOX_RECONNECT_COOLDOWN: Duration = Duration::from_secs(10);
#[cfg(windows)]
const VBOX_STUCK_REGARDLESS: Duration =
    Duration::from_secs(evertydesk_core::vm_console_runtime::VBOX_STUCK_REGARDLESS_SECS);
#[cfg(not(windows))]
const VBOX_STUCK_REGARDLESS: Duration = Duration::from_secs(60);

fn main() {
    install_process_diagnostics("rdp-viewer");

    let bootstrap = match read_bootstrap() {
        Ok(bootstrap) => bootstrap,
        Err(error) => {
            append_log_line("rdp-viewer", &format!("bootstrap failed: {error}"));
            eprintln!("[rdp-viewer] {error}");
            std::process::exit(1);
        }
    };
    append_log_line(
        "rdp-viewer",
        &format!("bootstrap ok: {}", rdp_target_label(&bootstrap.target)),
    );

    let event_loop = match EventLoop::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            append_log_line("rdp-viewer", &format!("event loop create failed: {error}"));
            eprintln!("[rdp-viewer] create event loop failed: {error}");
            std::process::exit(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App::new(bootstrap);
    if let Err(error) = event_loop.run_app(&mut app) {
        append_log_line("rdp-viewer", &format!("event loop run failed: {error}"));
        eprintln!("[rdp-viewer] event loop error: {error}");
        std::process::exit(1);
    }
}

fn rdp_target_label(target: &RdpTarget) -> String {
    match target {
        RdpTarget::HyperV { vm_guid } => format!("Hyper-V {vm_guid}"),
        RdpTarget::VirtualBox { vm_uuid, port } => format!("VirtualBox {vm_uuid} :{port}"),
    }
}

fn read_bootstrap() -> Result<RdpBootstrap, String> {
    if std::env::args().nth(1).as_deref() != Some("--bootstrap-stdin") {
        return Err("rdp-viewer must be launched with --bootstrap-stdin".to_owned());
    }
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let line = read_bounded_line(&mut reader, MAX_IPC_LINE_BYTES)
        .map_err(|error| format!("failed to read bootstrap: {error}"))?
        .ok_or_else(|| "stdin closed before a bootstrap line arrived".to_owned())?;
    serde_json::from_str(&line).map_err(|error| format!("invalid bootstrap JSON: {error}"))
}

#[cfg(windows)]
use evertydesk_core::vbox_rdp::{Poll, VrdeCmd};
#[cfg(windows)]
use evertydesk_core::vm_console_runtime::{VmConsoleSession, VmConsoleTarget};

#[cfg(windows)]
type RdpSessionHandle = VmConsoleSession;

// `hyperv_rdp::RdpSession` (Windows-only, backed by real ironrdp/VRDE
// plumbing) reuses `vbox_rdp`'s `Poll`/`VrdeCmd` as its own command and
// poll-result vocabulary, and that whole module is Windows-only (it needs
// `native_tls`, a `[target.'cfg(windows)'.dependencies]`-only crate — see
// src/lib.rs). Everywhere below constructs `VrdeCmd` and matches `Poll`
// unconditionally, so non-Windows needs same-named stand-ins even though
// `self.session` is always `None` there (see the `connect()` stub above)
// and none of this ever actually runs.
#[cfg(not(windows))]
enum VrdeCmd {
    MouseMove { x: u16, y: u16 },
    MouseButton { button: u8, down: bool },
    MouseWheel { delta: i16 },
    KeyDown { scancode: u8, extended: bool },
    KeyUp { scancode: u8, extended: bool },
    Text(String),
    Stop,
}

#[cfg(not(windows))]
enum Poll<T> {
    Item(T),
    Empty,
    Dead,
}

#[cfg(not(windows))]
struct RdpSessionHandle;

#[cfg(not(windows))]
impl RdpSessionHandle {
    fn poll_frame(&self) -> Poll<(u32, u32, Vec<u8>)> {
        Poll::Dead
    }
    fn poll_status(&self) -> Poll<String> {
        Poll::Dead
    }
    fn send(&self, _cmd: VrdeCmd) {}
}

struct App {
    bootstrap: RdpBootstrap,
    window: Option<Arc<Window>>,
    renderer: Option<FrameRenderer>,
    session: Option<RdpSessionHandle>,
    desktop_size: (u32, u32),
    has_frame: bool,
    modifiers: ModifiersState,
    pressed_mouse_buttons: Vec<MouseButton>,
    cursor_position: Option<(i32, i32)>,
    status: String,
    last_poll: Instant,
    last_frame: Instant,
    last_reconnect: Instant,
    reconnect_count: u32,
    presented_frames: u64,
    dumped_debug_frames: u32,
    input_debug_events: u32,
}

impl App {
    fn new(bootstrap: RdpBootstrap) -> Self {
        let now = Instant::now();
        Self {
            bootstrap,
            window: None,
            renderer: None,
            session: None,
            desktop_size: (u32::from(DEFAULT_WIDTH), u32::from(DEFAULT_HEIGHT)),
            has_frame: false,
            modifiers: ModifiersState::empty(),
            pressed_mouse_buttons: Vec::new(),
            cursor_position: None,
            status: "Подключение...".to_owned(),
            last_poll: now,
            last_frame: now,
            last_reconnect: now - VBOX_RECONNECT_COOLDOWN,
            reconnect_count: 0,
            presented_frames: 0,
            dumped_debug_frames: 0,
            input_debug_events: 0,
        }
    }

    fn target_label(&self) -> String {
        match &self.bootstrap.target {
            RdpTarget::HyperV { vm_guid } => format!("Hyper-V {vm_guid}"),
            RdpTarget::VirtualBox { vm_uuid, port } => format!("VirtualBox {vm_uuid} :{port}"),
        }
    }

    #[cfg(windows)]
    fn vm_console_target(&self) -> VmConsoleTarget {
        match &self.bootstrap.target {
            RdpTarget::HyperV { vm_guid } => VmConsoleTarget::HyperV {
                vm_guid: vm_guid.clone(),
                credentials: evertydesk_core::hyperv_rdp::RdpCredentials {
                    username: self.bootstrap.username.clone(),
                    password: self.bootstrap.password.clone(),
                    domain: self.bootstrap.domain.clone(),
                },
            },
            RdpTarget::VirtualBox { vm_uuid, port } => VmConsoleTarget::VirtualBox {
                vm_uuid: vm_uuid.clone(),
                port: *port,
                settings: self.bootstrap.vbox_vrde_settings.into_core(),
            },
        }
    }

    #[cfg(windows)]
    fn connect(&mut self) {
        self.last_frame = Instant::now();
        let target = self.vm_console_target();
        append_log_line(
            "rdp-viewer",
            &format!("connect requested: {}", target.label()),
        );
        if let VmConsoleTarget::VirtualBox { settings, .. } = &target {
            append_log_line(
                "rdp-viewer",
                &format!(
                    "VirtualBox VRDE profile: color_depth={} compression={}",
                    settings.color_depth,
                    settings.compression.label()
                ),
            );
        }
        match VmConsoleSession::connect(&target, (DEFAULT_WIDTH, DEFAULT_HEIGHT)) {
            Ok(session) => {
                append_log_line("rdp-viewer", &format!("connect ok: {}", target.label()));
                self.session = Some(session);
            }
            Err(error) => {
                append_log_line(
                    "rdp-viewer",
                    &format!("connect failed: {}: {error}", target.label()),
                );
                self.status = format!("Ошибка подключения: {error}");
                self.set_window_title();
            }
        }
    }

    #[cfg(not(windows))]
    fn connect(&mut self) {
        append_log_line("rdp-viewer", "connect failed: RDP VM is Windows-only");
        self.status = "RDP-подключение к ВМ поддерживается только на Windows (Hyper-V)".to_owned();
        self.set_window_title();
    }

    fn set_window_title(&self) {
        if let Some(window) = &self.window {
            window.set_title(&format!("RDP — {} — {}", self.target_label(), self.status));
        }
    }

    fn release_pressed_buttons(&mut self) {
        let Some(session) = &self.session else {
            self.pressed_mouse_buttons.clear();
            return;
        };
        for button in self.pressed_mouse_buttons.drain(..) {
            if let Some(index) = mouse_button_index(button) {
                session.send(VrdeCmd::MouseButton {
                    button: index,
                    down: false,
                });
            }
        }
    }

    fn poll_session(&mut self) {
        loop {
            let Some(session) = self.session.as_ref() else {
                return;
            };
            let outcome = session.poll_status();
            match outcome {
                Poll::Item(message) => {
                    append_log_line("rdp-viewer", &format!("status: {message}"));
                    if message == evertydesk_core::vm_console_runtime::VBOX_DESYNC_STATUS {
                        self.force_reconnect_vbox("VRDE_DESYNC");
                        continue;
                    }
                    self.status = message;
                    self.set_window_title();
                }
                Poll::Empty => break,
                Poll::Dead => {
                    let reason = latest_rdp_error(&self.bootstrap.target).unwrap_or_else(|| {
                        "session channel closed without final status".to_owned()
                    });
                    append_log_line("rdp-viewer", &format!("session dead: {reason}"));
                    self.status = format!("RDP-сессия завершена: {reason}");
                    self.set_window_title();
                    self.session = None;
                    return;
                }
            }
        }
        let mut latest_frame = None;
        loop {
            let Some(session) = self.session.as_ref() else {
                return;
            };
            let outcome = session.poll_frame();
            match outcome {
                Poll::Item((width, height, rgba)) => {
                    self.last_frame = Instant::now();
                    latest_frame = Some((width, height, rgba));
                }
                Poll::Empty => break,
                Poll::Dead => {
                    let reason = latest_rdp_error(&self.bootstrap.target)
                        .unwrap_or_else(|| "frame channel closed without final status".to_owned());
                    append_log_line("rdp-viewer", &format!("frame channel dead: {reason}"));
                    self.status = format!("RDP-сессия завершена: {reason}");
                    self.set_window_title();
                    self.session = None;
                    return;
                }
            }
        }
        if let Some((width, height, rgba)) = latest_frame {
            self.present_frame(width, height, rgba);
        }
    }

    fn present_frame(&mut self, width: u32, height: u32, rgba: Vec<u8>) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        self.presented_frames = self.presented_frames.saturating_add(1);
        if matches!(self.bootstrap.target, RdpTarget::VirtualBox { .. })
            && (self.presented_frames <= 8 || self.presented_frames.is_multiple_of(120))
        {
            append_log_line(
                "rdp-viewer",
                &format!(
                    "VirtualBox VRDE presenting frame #{}: {}",
                    self.presented_frames,
                    frame_fingerprint(width, height, &rgba)
                ),
            );
        }
        if matches!(self.bootstrap.target, RdpTarget::VirtualBox { .. })
            && should_dump_vbox_frame(self.presented_frames, self.dumped_debug_frames)
        {
            match dump_vbox_debug_frame(width, height, &rgba, self.presented_frames) {
                Ok(path) => {
                    self.dumped_debug_frames = self.dumped_debug_frames.saturating_add(1);
                    append_log_line(
                        "rdp-viewer",
                        &format!("VirtualBox VRDE debug frame saved: {}", path.display()),
                    );
                }
                Err(error) => append_log_line(
                    "rdp-viewer",
                    &format!("VirtualBox VRDE debug frame save failed: {error}"),
                ),
            }
        }
        if self.desktop_size != (width, height) {
            if let Err(error) = renderer.resize_buffer(width, height) {
                eprintln!("[rdp-viewer] resize frame buffer failed: {error}");
                return;
            }
            self.desktop_size = (width, height);
        }
        let target = renderer.frame_mut();
        if target.len() == rgba.len() {
            target.copy_from_slice(&rgba);
        }
        self.has_frame = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn maybe_reconnect_stuck_vbox(&mut self) {
        if !matches!(self.bootstrap.target, RdpTarget::VirtualBox { .. }) || self.session.is_none()
        {
            return;
        }
        if self.last_reconnect.elapsed() < VBOX_RECONNECT_COOLDOWN {
            return;
        }
        if self.last_frame.elapsed() <= VBOX_STUCK_REGARDLESS {
            return;
        }

        self.force_reconnect_vbox("stuck_regardless_60s");
    }

    fn force_reconnect_vbox(&mut self, reason: &str) {
        if !matches!(self.bootstrap.target, RdpTarget::VirtualBox { .. }) || self.session.is_none()
        {
            return;
        }
        self.reconnect_count = self.reconnect_count.saturating_add(1);
        self.last_reconnect = Instant::now();
        append_log_line(
            "rdp-viewer",
            &format!(
                "VirtualBox VRDE reconnect #{} reason={reason}: last_frame_ms={}",
                self.reconnect_count,
                self.last_frame.elapsed().as_millis()
            ),
        );
        if let Some(session) = self.session.take() {
            session.send(VrdeCmd::Stop);
        }
        self.status = format!("VRDE: переподключение #{}…", self.reconnect_count);
        self.set_window_title();
        self.presented_frames = 0;
        self.has_frame = false;
        self.dumped_debug_frames = 0;
        self.connect();
    }
}

fn latest_rdp_error(target: &RdpTarget) -> Option<String> {
    let path: PathBuf = match target {
        RdpTarget::HyperV { .. } => std::env::temp_dir().join("evertydesk-hvrdp.log"),
        RdpTarget::VirtualBox { .. } => std::env::temp_dir().join("evertydesk-vrde.log"),
    };
    let content = fs::read_to_string(path).ok()?;
    content
        .lines()
        .rev()
        .find(|line| {
            let lower = line.to_lowercase();
            lower.contains("ошибка")
                || lower.contains("error")
                || lower.contains("failed")
                || lower.contains("refused")
                || lower.contains("reset")
                || lower.contains("10054")
                || lower.contains("panic")
        })
        .map(|line| line.trim().to_owned())
}

fn frame_fingerprint(width: u32, height: u32, rgba: &[u8]) -> String {
    let Ok(wu) = usize::try_from(width) else {
        return format!("bad width={width}");
    };
    let Ok(hu) = usize::try_from(height) else {
        return format!("bad height={height}");
    };
    let pixel_count = wu.saturating_mul(hu);
    if pixel_count == 0 || rgba.len() != pixel_count.saturating_mul(4) {
        return format!(
            "bad geometry {width}x{height} len={} expected={}",
            rgba.len(),
            pixel_count.saturating_mul(4)
        );
    }

    let pixel_at = |px: usize, py: usize| -> String {
        let off = (py.saturating_mul(wu).saturating_add(px)).saturating_mul(4);
        if off + 4 <= rgba.len() {
            format!(
                "{:02x}{:02x}{:02x}{:02x}",
                rgba[off],
                rgba[off + 1],
                rgba[off + 2],
                rgba[off + 3]
            )
        } else {
            "????????".to_owned()
        }
    };

    const SAMPLES: usize = 512;
    let step = (pixel_count / SAMPLES).max(1);
    let mut nonzero = 0usize;
    let mut checked = 0usize;
    let mut px = 0usize;
    while px < pixel_count {
        let off = px * 4;
        if rgba[off] != 0 || rgba[off + 1] != 0 || rgba[off + 2] != 0 {
            nonzero += 1;
        }
        checked += 1;
        px += step;
    }
    let nonzero_pct = 100.0 * nonzero as f64 / checked.max(1) as f64;
    format!(
        "{}x{} len={} sample_nonzero={:.1}% tl={} tr={} center={} bl={} br={}",
        width,
        height,
        rgba.len(),
        nonzero_pct,
        pixel_at(0, 0),
        pixel_at(wu.saturating_sub(1), 0),
        pixel_at(wu / 2, hu / 2),
        pixel_at(0, hu.saturating_sub(1)),
        pixel_at(wu.saturating_sub(1), hu.saturating_sub(1)),
    )
}

fn should_dump_vbox_frame(presented_frames: u64, dumped_debug_frames: u32) -> bool {
    std::env::var_os("EVERTYDESK_RDP_DUMP_FRAMES").is_some()
        && dumped_debug_frames < 3
        && matches!(presented_frames, 1 | 4 | 120)
}

fn dump_vbox_debug_frame(
    width: u32,
    height: u32,
    rgba: &[u8],
    frame_index: u64,
) -> Result<PathBuf, String> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| format!("frame size overflow {width}x{height}"))?;
    if rgba.len() != expected {
        return Err(format!(
            "bad frame geometry {width}x{height}: len={} expected={expected}",
            rgba.len()
        ));
    }

    let path = std::env::temp_dir().join(format!("evertydesk-vrde-frame-{frame_index}.png"));
    let file =
        fs::File::create(&path).map_err(|error| format!("create {}: {error}", path.display()))?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| format!("png header {}: {error}", path.display()))?;
    writer
        .write_image_data(rgba)
        .map_err(|error| format!("png data {}: {error}", path.display()))?;
    Ok(path)
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = WindowAttributes::default()
            .with_title(format!("RDP — {}", self.target_label()))
            .with_inner_size(LogicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT))
            .with_min_inner_size(LogicalSize::new(640, 480));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("[rdp-viewer] create window failed: {error}");
                event_loop.exit();
                return;
            }
        };
        let mut renderer = match FrameRenderer::new(
            Arc::clone(&window),
            u32::from(DEFAULT_WIDTH),
            u32::from(DEFAULT_HEIGHT),
        ) {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("[rdp-viewer] initialize wgpu surface failed: {error}");
                event_loop.exit();
                return;
            }
        };
        renderer.set_scaling_mode(ScalingMode::Fill);
        self.window = Some(window);
        self.renderer = Some(renderer);
        self.connect();
        self.set_window_title();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window.as_ref().map(|window| window.id()) != Some(id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.release_pressed_buttons();
                if let Some(session) = self.session.take() {
                    session.send(VrdeCmd::Stop);
                }
                event_loop.exit();
            }
            WindowEvent::Focused(false) => {
                self.release_pressed_buttons();
            }
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                if let Some(renderer) = self.renderer.as_mut() {
                    if let Err(error) = renderer.resize_surface(size.width, size.height) {
                        eprintln!("[rdp-viewer] resize surface failed: {error}");
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let Some(renderer) = self.renderer.as_ref() else {
                    return;
                };
                let (x, y) = renderer
                    .window_pos_to_pixel((position.x as f32, position.y as f32))
                    .unwrap_or_else(|pos| renderer.clamp_pixel_pos(pos));
                let x = x.min(u16::MAX as usize) as u16;
                let y = y.min(u16::MAX as usize) as u16;
                self.cursor_position = Some((i32::from(x), i32::from(y)));
                self.log_input_debug(format_args!("mouse_move {x},{y}"));
                if let Some(session) = &self.session {
                    session.send(VrdeCmd::MouseMove { x, y });
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some(index) = mouse_button_index(button) else {
                    return;
                };
                let down = state == ElementState::Pressed;
                if down {
                    if !self.pressed_mouse_buttons.contains(&button) {
                        self.pressed_mouse_buttons.push(button);
                    }
                } else {
                    self.pressed_mouse_buttons.retain(|held| *held != button);
                }
                let cursor_position = self.cursor_position;
                let cursor_u16 = self.cursor_position_u16();
                self.log_input_debug(format_args!(
                    "mouse_button button={index} down={down} pos={cursor_position:?}",
                ));
                if let Some(session) = &self.session {
                    if let Some((x, y)) = cursor_u16 {
                        session.send(VrdeCmd::MouseMove { x, y });
                    }
                    session.send(VrdeCmd::MouseButton {
                        button: index,
                        down,
                    });
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(delta) => (delta.y / 20.0) as f32,
                };
                if steps.abs() < f32::EPSILON {
                    return;
                }
                let delta = (steps.clamp(-10.0, 10.0) * 120.0) as i16;
                let cursor_position = self.cursor_position;
                let cursor_u16 = self.cursor_position_u16();
                self.log_input_debug(format_args!(
                    "mouse_wheel delta={delta} pos={cursor_position:?}",
                ));
                if let Some(session) = &self.session {
                    if let Some((x, y)) = cursor_u16 {
                        session.send(VrdeCmd::MouseMove { x, y });
                    }
                    session.send(VrdeCmd::MouseWheel { delta });
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.send_keyboard_input(event);
            }
            WindowEvent::RedrawRequested => {
                if let Some(renderer) = self.renderer.as_mut() {
                    if self.has_frame {
                        if let Err(error) = renderer.render() {
                            eprintln!("[rdp-viewer] render failed: {error}");
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.last_poll.elapsed() >= POLL_INTERVAL {
            self.last_poll = Instant::now();
            self.poll_session();
            self.maybe_reconnect_stuck_vbox();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.last_poll + POLL_INTERVAL));
    }
}

impl App {
    fn cursor_position_u16(&self) -> Option<(u16, u16)> {
        let (x, y) = self.cursor_position?;
        Some((
            x.clamp(0, i32::from(u16::MAX)) as u16,
            y.clamp(0, i32::from(u16::MAX)) as u16,
        ))
    }

    fn log_input_debug(&mut self, args: std::fmt::Arguments<'_>) {
        if self.input_debug_events >= 80 {
            return;
        }
        self.input_debug_events = self.input_debug_events.saturating_add(1);
        append_log_line(
            "rdp-viewer",
            &format!("VM input #{}: {args}", self.input_debug_events),
        );
    }

    fn send_keyboard_input(&mut self, event: KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else {
            return;
        };
        let pressed = event.state == ElementState::Pressed;
        if self.session.is_none() {
            return;
        }
        let combo =
            self.modifiers.control_key() || self.modifiers.alt_key() || self.modifiers.super_key();

        // Printable characters go through Unicode keyboard events (matches
        // the egui client's `egui_key_is_plain_text` gate) unless a modifier
        // combo is held, so Ctrl+C etc. still reach the scancode path below.
        if !combo && !pressed && rdp_key_is_plain_text(code) {
            return;
        }
        if !combo && pressed && rdp_key_is_plain_text(code) {
            if let Some(text) = event.text.as_ref().filter(|text| !text.is_empty()) {
                self.log_input_debug(format_args!("key_text {text:?}"));
                if let Some(session) = &self.session {
                    send_text_like_lite(session, text);
                }
                return;
            }
        }

        let Some((scancode, extended)) = winit_keycode_to_rdp_scancode(code) else {
            return;
        };

        let mut mods: Vec<(u8, bool)> = Vec::new();
        if self.modifiers.control_key() {
            mods.push((0x1D, false));
        }
        if self.modifiers.alt_key() {
            mods.push((0x38, false));
        }
        if self.modifiers.shift_key() {
            mods.push((0x2A, false));
        }
        if self.modifiers.super_key() {
            mods.push((0x5B, true));
        }

        if pressed {
            self.log_input_debug(format_args!(
                "key_down code={code:?} sc={scancode:#x} ext={extended} mods={}",
                mods.len()
            ));
            let Some(session) = &self.session else {
                return;
            };
            for (m_scancode, m_extended) in &mods {
                session.send(VrdeCmd::KeyDown {
                    scancode: *m_scancode,
                    extended: *m_extended,
                });
            }
            session.send(VrdeCmd::KeyDown { scancode, extended });
        } else {
            self.log_input_debug(format_args!(
                "key_up code={code:?} sc={scancode:#x} ext={extended} mods={}",
                mods.len()
            ));
            let Some(session) = &self.session else {
                return;
            };
            session.send(VrdeCmd::KeyUp { scancode, extended });
            for (m_scancode, m_extended) in mods.iter().rev() {
                session.send(VrdeCmd::KeyUp {
                    scancode: *m_scancode,
                    extended: *m_extended,
                });
            }
        }
    }
}

#[cfg(windows)]
fn send_text_like_lite(session: &RdpSessionHandle, text: &str) {
    evertydesk_core::vm_console_runtime::send_text_as_lite(session, text);
}

#[cfg(not(windows))]
fn send_text_like_lite(session: &RdpSessionHandle, text: &str) {
    session.send(VrdeCmd::Text(text.to_owned()));
}

fn mouse_button_index(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Right => Some(1),
        MouseButton::Middle => Some(2),
        _ => None,
    }
}

/// Physical key -> PS/2 Set-1 scancode (+ E0-extended flag). Values carried
/// over unchanged from the egui client's proven `egui_key_to_rdp_scancode`
/// table; see that function's history for why these specific bytes.
/// Deliberately narrow — printable characters go through
/// `VrdeCmd::Text` instead (see `rdp_key_is_plain_text`).
fn winit_keycode_to_rdp_scancode(code: KeyCode) -> Option<(u8, bool)> {
    Some(match code {
        KeyCode::Escape => (0x01, false),
        KeyCode::Backspace => (0x0E, false),
        KeyCode::Tab => (0x0F, false),
        KeyCode::Enter | KeyCode::NumpadEnter => (0x1C, false),
        KeyCode::Insert => (0x52, true),
        KeyCode::Delete => (0x53, true),
        KeyCode::Home => (0x47, true),
        KeyCode::End => (0x4F, true),
        KeyCode::PageUp => (0x49, true),
        KeyCode::PageDown => (0x51, true),
        KeyCode::ArrowLeft => (0x4B, true),
        KeyCode::ArrowRight => (0x4D, true),
        KeyCode::ArrowUp => (0x48, true),
        KeyCode::ArrowDown => (0x50, true),
        KeyCode::F1 => (0x3B, false),
        KeyCode::F2 => (0x3C, false),
        KeyCode::F3 => (0x3D, false),
        KeyCode::F4 => (0x3E, false),
        KeyCode::F5 => (0x3F, false),
        KeyCode::F6 => (0x40, false),
        KeyCode::F7 => (0x41, false),
        KeyCode::F8 => (0x42, false),
        KeyCode::F9 => (0x43, false),
        KeyCode::F10 => (0x44, false),
        KeyCode::F11 => (0x57, false),
        KeyCode::F12 => (0x58, false),
        KeyCode::KeyA => (0x1E, false),
        KeyCode::KeyB => (0x30, false),
        KeyCode::KeyC => (0x2E, false),
        KeyCode::KeyD => (0x20, false),
        KeyCode::KeyE => (0x12, false),
        KeyCode::KeyF => (0x21, false),
        KeyCode::KeyG => (0x22, false),
        KeyCode::KeyH => (0x23, false),
        KeyCode::KeyI => (0x17, false),
        KeyCode::KeyJ => (0x24, false),
        KeyCode::KeyK => (0x25, false),
        KeyCode::KeyL => (0x26, false),
        KeyCode::KeyM => (0x32, false),
        KeyCode::KeyN => (0x31, false),
        KeyCode::KeyO => (0x18, false),
        KeyCode::KeyP => (0x19, false),
        KeyCode::KeyQ => (0x10, false),
        KeyCode::KeyR => (0x13, false),
        KeyCode::KeyS => (0x1F, false),
        KeyCode::KeyT => (0x14, false),
        KeyCode::KeyU => (0x16, false),
        KeyCode::KeyV => (0x2F, false),
        KeyCode::KeyW => (0x11, false),
        KeyCode::KeyX => (0x2D, false),
        KeyCode::KeyY => (0x15, false),
        KeyCode::KeyZ => (0x2C, false),
        KeyCode::Digit0 => (0x0B, false),
        KeyCode::Digit1 => (0x02, false),
        KeyCode::Digit2 => (0x03, false),
        KeyCode::Digit3 => (0x04, false),
        KeyCode::Digit4 => (0x05, false),
        KeyCode::Digit5 => (0x06, false),
        KeyCode::Digit6 => (0x07, false),
        KeyCode::Digit7 => (0x08, false),
        KeyCode::Digit8 => (0x09, false),
        KeyCode::Digit9 => (0x0A, false),
        KeyCode::Space => (0x39, false),
        _ => return None,
    })
}

/// Whether typed Unicode (`VrdeCmd::Text`) should be preferred over the
/// scancode table for this key, mirroring the egui client's
/// `egui_key_is_plain_text` gate.
fn rdp_key_is_plain_text(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::KeyA
            | KeyCode::KeyB
            | KeyCode::KeyC
            | KeyCode::KeyD
            | KeyCode::KeyE
            | KeyCode::KeyF
            | KeyCode::KeyG
            | KeyCode::KeyH
            | KeyCode::KeyI
            | KeyCode::KeyJ
            | KeyCode::KeyK
            | KeyCode::KeyL
            | KeyCode::KeyM
            | KeyCode::KeyN
            | KeyCode::KeyO
            | KeyCode::KeyP
            | KeyCode::KeyQ
            | KeyCode::KeyR
            | KeyCode::KeyS
            | KeyCode::KeyT
            | KeyCode::KeyU
            | KeyCode::KeyV
            | KeyCode::KeyW
            | KeyCode::KeyX
            | KeyCode::KeyY
            | KeyCode::KeyZ
            | KeyCode::Digit0
            | KeyCode::Digit1
            | KeyCode::Digit2
            | KeyCode::Digit3
            | KeyCode::Digit4
            | KeyCode::Digit5
            | KeyCode::Digit6
            | KeyCode::Digit7
            | KeyCode::Digit8
            | KeyCode::Digit9
    )
}
