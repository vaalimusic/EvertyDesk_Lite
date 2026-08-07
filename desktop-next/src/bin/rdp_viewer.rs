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

use std::io::{self, BufReader};
use std::sync::Arc;
use std::time::{Duration, Instant};

use evertydesk_desktop_next::frame_renderer::FrameRenderer;
use evertydesk_desktop_next::ipc::{read_bounded_line, MAX_IPC_LINE_BYTES};
use evertydesk_desktop_next::protocol::{RdpBootstrap, RdpTarget};
use evertydesk_desktop_next::startup_log::install_process_diagnostics;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

const DEFAULT_WIDTH: u16 = 1280;
const DEFAULT_HEIGHT: u16 = 800;
/// How often to poll the session thread's frame/status channels.
const POLL_INTERVAL: Duration = Duration::from_millis(8);

fn main() {
    install_process_diagnostics("rdp-viewer");

    let bootstrap = match read_bootstrap() {
        Ok(bootstrap) => bootstrap,
        Err(error) => {
            eprintln!("[rdp-viewer] {error}");
            std::process::exit(1);
        }
    };

    let event_loop = match EventLoop::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            eprintln!("[rdp-viewer] create event loop failed: {error}");
            std::process::exit(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new(bootstrap);
    if let Err(error) = event_loop.run_app(&mut app) {
        eprintln!("[rdp-viewer] event loop error: {error}");
        std::process::exit(1);
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
type RdpSessionHandle = evertydesk_core::hyperv_rdp::RdpSession;

#[cfg(not(windows))]
struct RdpSessionHandle;

#[cfg(not(windows))]
impl RdpSessionHandle {
    fn poll_frame(&self) -> evertydesk_core::vbox_rdp::Poll<(u32, u32, Vec<u8>)> {
        evertydesk_core::vbox_rdp::Poll::Dead
    }
    fn poll_status(&self) -> evertydesk_core::vbox_rdp::Poll<String> {
        evertydesk_core::vbox_rdp::Poll::Dead
    }
    fn send(&self, _cmd: evertydesk_core::vbox_rdp::VrdeCmd) {}
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
}

impl App {
    fn new(bootstrap: RdpBootstrap) -> Self {
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
            last_poll: Instant::now(),
        }
    }

    fn target_label(&self) -> String {
        match &self.bootstrap.target {
            RdpTarget::HyperV { vm_guid } => format!("Hyper-V {vm_guid}"),
        }
    }

    #[cfg(windows)]
    fn connect(&mut self) {
        let RdpTarget::HyperV { vm_guid } = &self.bootstrap.target;
        let creds = evertydesk_core::hyperv_rdp::RdpCredentials {
            username: self.bootstrap.username.clone(),
            password: self.bootstrap.password.clone(),
            domain: self.bootstrap.domain.clone(),
        };
        match evertydesk_core::hyperv_rdp::RdpSession::connect(
            vm_guid,
            creds,
            (DEFAULT_WIDTH, DEFAULT_HEIGHT),
        ) {
            Ok(session) => self.session = Some(session),
            Err(error) => {
                self.status = format!("Ошибка подключения: {error}");
                self.set_window_title();
            }
        }
    }

    #[cfg(not(windows))]
    fn connect(&mut self) {
        self.status =
            "RDP-подключение к ВМ поддерживается только на Windows (Hyper-V)".to_owned();
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
                session.send(evertydesk_core::vbox_rdp::VrdeCmd::MouseButton {
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
                evertydesk_core::vbox_rdp::Poll::Item(message) => {
                    self.status = message;
                    self.set_window_title();
                }
                evertydesk_core::vbox_rdp::Poll::Empty => break,
                evertydesk_core::vbox_rdp::Poll::Dead => {
                    self.status = "Сессия завершена".to_owned();
                    self.set_window_title();
                    self.session = None;
                    return;
                }
            }
        }
        loop {
            let Some(session) = self.session.as_ref() else {
                return;
            };
            let outcome = session.poll_frame();
            match outcome {
                evertydesk_core::vbox_rdp::Poll::Item((width, height, rgba)) => {
                    self.present_frame(width, height, rgba);
                }
                evertydesk_core::vbox_rdp::Poll::Empty => break,
                evertydesk_core::vbox_rdp::Poll::Dead => {
                    self.status = "Сессия завершена".to_owned();
                    self.set_window_title();
                    self.session = None;
                    return;
                }
            }
        }
    }

    fn present_frame(&mut self, width: u32, height: u32, rgba: Vec<u8>) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
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
        let renderer = match FrameRenderer::new(
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
                    session.send(evertydesk_core::vbox_rdp::VrdeCmd::Stop);
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
                if let Some(session) = &self.session {
                    session.send(evertydesk_core::vbox_rdp::VrdeCmd::MouseMove { x, y });
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
                if let Some(session) = &self.session {
                    session.send(evertydesk_core::vbox_rdp::VrdeCmd::MouseButton {
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
                if let Some(session) = &self.session {
                    session.send(evertydesk_core::vbox_rdp::VrdeCmd::MouseWheel { delta });
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

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if self.last_poll.elapsed() >= POLL_INTERVAL {
            self.last_poll = Instant::now();
            self.poll_session();
        }
    }
}

impl App {
    fn send_keyboard_input(&mut self, event: KeyEvent) {
        let Some(session) = &self.session else {
            return;
        };
        let PhysicalKey::Code(code) = event.physical_key else {
            return;
        };
        let pressed = event.state == ElementState::Pressed;
        let combo = self.modifiers.control_key() || self.modifiers.alt_key() || self.modifiers.super_key();

        // Printable characters go through Unicode keyboard events (matches
        // the egui client's `egui_key_is_plain_text` gate) unless a modifier
        // combo is held, so Ctrl+C etc. still reach the scancode path below.
        if !combo && !pressed && rdp_key_is_plain_text(code) {
            return;
        }
        if !combo && pressed && rdp_key_is_plain_text(code) {
            if let Some(text) = event.text.as_ref().filter(|text| !text.is_empty()) {
                session.send(evertydesk_core::vbox_rdp::VrdeCmd::Text(text.to_string()));
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
            for (m_scancode, m_extended) in &mods {
                session.send(evertydesk_core::vbox_rdp::VrdeCmd::KeyDown {
                    scancode: *m_scancode,
                    extended: *m_extended,
                });
            }
            session.send(evertydesk_core::vbox_rdp::VrdeCmd::KeyDown { scancode, extended });
        } else {
            session.send(evertydesk_core::vbox_rdp::VrdeCmd::KeyUp { scancode, extended });
            for (m_scancode, m_extended) in mods.iter().rev() {
                session.send(evertydesk_core::vbox_rdp::VrdeCmd::KeyUp {
                    scancode: *m_scancode,
                    extended: *m_extended,
                });
            }
        }
    }
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
