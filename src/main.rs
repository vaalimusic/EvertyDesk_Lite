#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod rustdesk_proto;
mod settings;
mod transport;
mod video;

use std::{
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use eframe::egui::{self, ColorImage, TextureHandle};
use rustdesk_proto::ControlKey;
use settings::{generate_numeric_token, AppConfig, CoordinateMode};
use transport::{
    ConnectionRequest, ConnectionState, RemoteDisplay, SessionCommand, SessionEvent,
    TransportClient,
};

const APP_NAME: &str = "EvertyDesk Lite";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppMode {
    Connect,
    Host,
}

fn main() -> eframe::Result<()> {
    if let Some(exit_code) = run_cli_connect() {
        std::process::exit(exit_code);
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_inner_size([500.0, 350.0])
            .with_min_inner_size([420.0, 300.0]),
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_pixels_per_point(1.0);
            configure_style(&cc.egui_ctx);
            Box::new(EvertyDeskApp::new())
        }),
    )
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
    let request = ConnectionRequest {
        remote_id,
        password,
        server: config.server,
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
    mode: AppMode,
    status: String,
    host_status: String,
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
    remote_texture: Option<TextureHandle>,
    pending_image: Option<ColorImage>,
    remote_size: [usize; 2],
    text_to_send: String,
    remote_input_focused: bool,
    last_mouse_pos: Option<(i32, i32)>,
    remote_displays: Vec<RemoteDisplay>,
    selected_display: i32,
    auto_refresh: bool,
    refresh_millis: u64,
    fit_to_window: bool,
    coordinate_mode: CoordinateMode,
    screenshot_count: u64,
    screenshot_pending: bool,
    last_screenshot_at: Option<Instant>,
    last_screenshot_sid: String,
    input_events_sent: u64,
    last_move_refresh_at: Option<Instant>,
    fps_last_at: Instant,
    fps_last_count: u64,
    display_fps: f32,
    wheel_accum: egui::Vec2,
}

enum WorkerEvent {
    Session(SessionEvent),
    HostServerCheck(Result<(), String>),
    RemoteOnlineCheck {
        remote_id: String,
        result: Result<bool, String>,
    },
}

impl EvertyDeskApp {
    fn new() -> Self {
        let config = AppConfig::load_or_create();
        let remote_id = config.ui.last_remote_id.clone();
        let auto_refresh = config.ui.auto_refresh;
        let refresh_millis = config.ui.refresh_millis.clamp(50, 2000).min(80);
        let fit_to_window = config.ui.fit_to_window;
        let coordinate_mode = config.ui.coordinate_mode;
        Self {
            config,
            remote_id,
            password: String::new(),
            show_password: false,
            show_host_password: false,
            mode: AppMode::Connect,
            host_status: "Хост-режим: входящие подключения пока в разработке.".to_owned(),
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
            remote_texture: None,
            pending_image: None,
            remote_size: [0, 0],
            text_to_send: String::new(),
            remote_input_focused: false,
            last_mouse_pos: None,
            remote_displays: Vec::new(),
            selected_display: 0,
            auto_refresh,
            refresh_millis,
            fit_to_window,
            coordinate_mode,
            screenshot_count: 0,
            screenshot_pending: false,
            last_screenshot_at: None,
            last_screenshot_sid: String::new(),
            input_events_sent: 0,
            last_move_refresh_at: None,
            fps_last_at: Instant::now(),
            fps_last_count: 0,
            display_fps: 0.0,
            wheel_accum: egui::Vec2::ZERO,
        }
    }

    fn connect(&mut self) {
        let normalized_remote_id = normalize_remote_id(&self.remote_id);
        let request = ConnectionRequest {
            remote_id: normalized_remote_id.clone(),
            password: self.password.clone(),
            server: self.config.server.clone(),
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
        self.remote_fullscreen = false;
        self.remote_id = normalized_remote_id;
        self.save_ui_config();
        self.remote_texture = None;
        self.remote_size = [0, 0];
        self.remote_displays.clear();
        self.screenshot_count = 0;
        self.screenshot_pending = false;
        self.last_screenshot_at = None;
        self.last_screenshot_sid.clear();
        self.input_events_sent = 0;
        self.last_move_refresh_at = None;
        self.fps_last_at = Instant::now();
        self.fps_last_count = 0;
        self.display_fps = 0.0;
        self.wheel_accum = egui::Vec2::ZERO;
        self.selected_display = 0;
        self.remote_input_focused = false;
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
            while let Ok(event) = session_rx.recv() {
                let terminal = matches!(event, SessionEvent::Closed | SessionEvent::Failed(_));
                let _ = ui_events.send(WorkerEvent::Session(event));
                if terminal {
                    break;
                }
            }
        });
        self.worker = Some(rx);
    }

    fn poll_worker(&mut self) {
        let Some(rx) = self.worker.take() else {
            return;
        };

        loop {
            match rx.try_recv() {
                Ok(WorkerEvent::Session(event)) => {
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
                    self.worker = Some(rx);
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
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
                self.remote_viewer_open = true;
                self.progress = 100;
                self.connection_state = ConnectionState::RelayReady {
                    remote_id: self.remote_id.clone(),
                };
                self.status = "Подключено".to_owned();
                self.log(format!("Connected: {info}"));
                self.send_command(SessionCommand::SetAutoRefresh {
                    enabled: self.auto_refresh,
                    millis: self.refresh_millis,
                });
            }
            SessionEvent::Frame {
                sid,
                width,
                height,
                rgba,
            } => {
                let image = ColorImage::from_rgba_unmultiplied([width, height], &rgba);
                self.remote_size = image.size;
                self.pending_image = Some(image);
                self.last_screenshot_sid = sid;
                self.last_screenshot_at = Some(Instant::now());
                if self.screenshot_count <= 1 || self.screenshot_count % 20 == 0 {
                    self.log(format!("Frame received: {}", self.last_screenshot_sid));
                }
            }
            SessionEvent::ScreenshotStats { received, pending } => {
                self.screenshot_count = received;
                self.screenshot_pending = pending;
                self.update_fps(received);
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
            SessionEvent::Info(message) => self.log(message),
            SessionEvent::Failed(err) => {
                self.busy = false;
                self.connected = false;
                self.remote_viewer_open = false;
                self.remote_fullscreen = false;
                self.session_tx = None;
                self.connection_state = ConnectionState::Failed(err.clone());
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
        if let Some(tx) = self.session_tx.take() {
            let _ = tx.send(SessionCommand::Close);
        }
        self.busy = false;
        self.connected = false;
        self.remote_viewer_open = false;
        self.remote_fullscreen = false;
        self.remote_input_focused = false;
        self.screenshot_pending = false;
        self.last_mouse_pos = None;
        self.last_move_refresh_at = None;
        self.wheel_accum = egui::Vec2::ZERO;
        self.progress = 0;
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

    fn update_fps(&mut self, received: u64) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.fps_last_at);
        if elapsed >= Duration::from_millis(750) {
            let frames = received.saturating_sub(self.fps_last_count);
            self.display_fps = frames as f32 / elapsed.as_secs_f32();
            self.fps_last_at = now;
            self.fps_last_count = received;
        }
    }

    fn save_ui_config(&mut self) {
        self.config.ui.last_remote_id = self.remote_id.clone();
        remember_remote_id(&mut self.config.ui.recent_remote_ids, &self.remote_id);
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
        self.events.push(message);
        if self.events.len() > 80 {
            self.events.remove(0);
        }
    }
}

impl eframe::App for EvertyDeskApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker();
        if let Some(image) = self.pending_image.take() {
            if let Some(texture) = self.remote_texture.as_mut() {
                texture.set(image, egui::TextureOptions::NEAREST);
            } else {
                self.remote_texture =
                    Some(ctx.load_texture("remote-screen", image, egui::TextureOptions::NEAREST));
            }
            ctx.request_repaint();
        }
        if self.busy || self.connected || self.host_check_busy || self.remote_check_busy {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
        if self.connected && self.remote_viewer_open {
            self.remote_viewer_window(ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.heading(APP_NAME);
                ui.label(format!("v{APP_VERSION}"));
            });
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.mode, AppMode::Connect, "Подключиться");
                ui.selectable_value(&mut self.mode, AppMode::Host, "Этот компьютер");
            });
            ui.separator();
            match self.mode {
                AppMode::Connect => self.connect_ui(ui),
                AppMode::Host => self.host_ui(ui),
            }
        });
        if self.mode == AppMode::Connect
            && !self.busy
            && !self.connected
            && ctx.input(|input| input.key_pressed(egui::Key::Enter))
        {
            self.connect();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.connected || self.busy {
            self.disconnect_session("Application closed");
        }
    }
}

impl EvertyDeskApp {
    fn connect_ui(&mut self, ui: &mut egui::Ui) {
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
    }

    fn host_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label("Ваш ID");
        ui.horizontal(|ui| {
            ui.monospace(&self.config.local_id);
            if ui.button("Копировать").clicked() {
                ui.output_mut(|output| output.copied_text = self.config.local_id.clone());
            }
        });

        ui.add_space(8.0);
        ui.label("Пароль для входящего подключения");
        ui.horizontal(|ui| {
            let password_text = if self.show_host_password {
                self.config.local_password.clone()
            } else {
                "•".repeat(self.config.local_password.len())
            };
            ui.monospace(password_text);
            if ui.button("Копировать").clicked() {
                ui.output_mut(|output| output.copied_text = self.config.local_password.clone());
            }
            if ui.button("Обновить").clicked() {
                self.config.local_password = generate_numeric_token(6);
                self.config.save();
            }
        });
        ui.checkbox(&mut self.show_host_password, "Показать пароль");

        ui.add_space(10.0);
        ui.colored_label(
            egui::Color32::from_rgb(230, 190, 95),
            "Host Mode: входящие подключения пока в разработке.",
        );
        ui.label("Следующий этап: регистрация этого ПК на ID server и прием relay-сессии.");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.host_check_busy && !self.busy && !self.connected,
                    egui::Button::new("Проверить сервер").min_size(egui::vec2(150.0, 30.0)),
                )
                .clicked()
            {
                self.check_host_server();
            }
            if self.host_check_busy {
                ui.spinner();
                ui.label("Проверка...");
            }
        });
        ui.colored_label(egui::Color32::from_rgb(230, 190, 95), &self.host_status);
    }

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

    fn check_remote_online(&mut self) {
        if self.remote_check_busy || self.busy || self.connected {
            return;
        }
        let remote_id = normalize_remote_id(&self.remote_id);
        if remote_id.is_empty() {
            self.set_error("Введите ID удаленного ПК");
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

    fn remote_viewer_window(&mut self, ctx: &egui::Context) {
        let title = if self.remote_id.trim().is_empty() {
            "EvertyDesk Lite - Remote desktop".to_owned()
        } else {
            format!("EvertyDesk Lite - {}", self.remote_id.trim())
        };
        let viewport_id = egui::ViewportId::from_hash_of("evertydesk-lite-remote-viewer");
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([720.0, 480.0]);

        ctx.show_viewport_immediate(viewport_id, builder, |ctx, _class| {
            if ctx.input(|input| input.viewport().close_requested()) {
                self.remote_viewer_open = false;
                self.remote_input_focused = false;
                self.last_mouse_pos = None;
                self.wheel_accum = egui::Vec2::ZERO;
                self.status = "Окно экрана закрыто".to_owned();
                self.send_command(SessionCommand::SetAutoRefresh {
                    enabled: false,
                    millis: self.refresh_millis,
                });
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            if ctx.input(|input| input.key_pressed(egui::Key::F11)) {
                self.remote_fullscreen = !self.remote_fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.remote_fullscreen));
            }

            egui::TopBottomPanel::top("remote-toolbar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Обновить").clicked() {
                        self.send_command(SessionCommand::Screenshot);
                    }
                    if ui.checkbox(&mut self.fit_to_window, "По размеру").changed() {
                        self.save_ui_config();
                    }
                    let fullscreen_label = if self.remote_fullscreen {
                        "РћРєРЅРѕ"
                    } else {
                        "Р’Рѕ РІРµСЃСЊ СЌРєСЂР°РЅ"
                    };
                    if ui.button(fullscreen_label).clicked() {
                        self.remote_fullscreen = !self.remote_fullscreen;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                            self.remote_fullscreen,
                        ));
                    }
                    if !self.remote_displays.is_empty() {
                        let selected_text = self
                            .remote_displays
                            .iter()
                            .find(|display| display.index == self.selected_display)
                            .map(display_label)
                            .unwrap_or_else(|| format!("Display {}", self.selected_display + 1));
                        egui::ComboBox::from_id_source("remote-display")
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
                    if ui.button("Enter").clicked() {
                        self.send_command(SessionCommand::KeyEnter);
                        self.send_command(SessionCommand::Screenshot);
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.text_to_send)
                            .hint_text("Текст")
                            .desired_width(180.0),
                    );
                    if ui
                        .add_enabled(
                            !self.text_to_send.is_empty(),
                            egui::Button::new("Отправить"),
                        )
                        .clicked()
                    {
                        let text = std::mem::take(&mut self.text_to_send);
                        self.send_command(SessionCommand::KeyText(text));
                        self.send_command(SessionCommand::Screenshot);
                    }
                    if ui
                        .add_enabled(self.remote_input_focused, egui::Button::new("Отпустить"))
                        .clicked()
                    {
                        self.remote_input_focused = false;
                        self.last_mouse_pos = None;
                        self.wheel_accum = egui::Vec2::ZERO;
                    }
                    ui.separator();
                    ui.label(remote_status_text(self));
                    ui.menu_button("Еще", |ui| {
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
                                    .text("refresh ms")
                                    .clamp_to_range(true),
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
                        egui::ComboBox::from_id_source("coordinate-mode")
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
                        if let Some((x, y)) = self.last_mouse_pos {
                            ui.label(format!("Mouse: {x},{y}"));
                        }
                        ui.label(format!("Input events: {}", self.input_events_sent));
                        if let Some(age) = self.last_screenshot_age_ms() {
                            ui.label(format!("Frame age: {age} ms"));
                        }
                        ui.separator();
                        if ui.button("Ctrl+Alt+Del").clicked() {
                            self.send_command(SessionCommand::KeyControl(ControlKey::CtrlAltDel));
                            self.send_command(SessionCommand::Screenshot);
                            ui.close_menu();
                        }
                        if ui.button("Lock remote").clicked() {
                            self.send_command(SessionCommand::KeyControl(ControlKey::LockScreen));
                            self.send_command(SessionCommand::Screenshot);
                            ui.close_menu();
                        }
                    });
                });
            });

            egui::CentralPanel::default().show(ctx, |ui| {
                self.remote_screen_ui(ui);
            });
        });
    }

    fn remote_screen_ui(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_width();
        let Some(texture) = self.remote_texture.clone() else {
            ui.allocate_ui(
                egui::vec2(available, ui.available_height().max(360.0)),
                |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.label("Waiting for remote screenshot");
                    });
                },
            );
            return;
        };

        let [w, h] = self.remote_size;
        if w == 0 || h == 0 {
            return;
        }
        let max_height = ui.available_height().max(360.0);
        let scale = if self.fit_to_window {
            (available / w as f32).min(max_height / h as f32).min(1.0)
        } else {
            1.0
        };
        let size = egui::vec2(w as f32 * scale, h as f32 * scale);
        let response = ui
            .add(
                egui::Image::new(&texture)
                    .fit_to_exact_size(size)
                    .sense(egui::Sense::click_and_drag()),
            )
            .on_hover_cursor(egui::CursorIcon::Crosshair);
        if response.double_clicked() {
            self.remote_fullscreen = !self.remote_fullscreen;
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.remote_fullscreen));
        }
        if self.connected {
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

            if response.hovered() || self.remote_input_focused {
                let events = ui.input(|input| input.events.clone());
                for event in events {
                    match event {
                        egui::Event::PointerButton {
                            pos,
                            button,
                            pressed,
                            ..
                        } => {
                            let inside = response.rect.contains(pos);
                            if pressed && !inside {
                                continue;
                            }
                            let (x, y) = if inside {
                                let local = pos - response.rect.min;
                                self.remote_point_from_local(local.x / scale, local.y / scale)
                            } else {
                                self.last_mouse_pos.unwrap_or((0, 0))
                            };
                            if inside {
                                self.remote_input_focused = true;
                            }
                            match (button, pressed) {
                                (egui::PointerButton::Primary, true) => {
                                    self.send_command(SessionCommand::MouseDown { x, y });
                                    self.send_command(SessionCommand::Screenshot);
                                }
                                (egui::PointerButton::Primary, false) => {
                                    self.send_command(SessionCommand::MouseUp { x, y });
                                    self.send_command(SessionCommand::Screenshot);
                                }
                                (egui::PointerButton::Secondary, true) => {
                                    self.send_command(SessionCommand::MouseRightDown { x, y });
                                    self.send_command(SessionCommand::Screenshot);
                                }
                                (egui::PointerButton::Secondary, false) => {
                                    self.send_command(SessionCommand::MouseRightUp { x, y });
                                    self.send_command(SessionCommand::Screenshot);
                                }
                                (egui::PointerButton::Middle, true) => {
                                    self.send_command(SessionCommand::MouseMiddleDown { x, y });
                                    self.send_command(SessionCommand::Screenshot);
                                }
                                (egui::PointerButton::Middle, false) => {
                                    self.send_command(SessionCommand::MouseMiddleUp { x, y });
                                    self.send_command(SessionCommand::Screenshot);
                                }
                                _ => {}
                            }
                        }
                        egui::Event::MouseWheel { unit, delta, .. } => {
                            if let Some((x, y)) = self.wheel_delta(unit, delta) {
                                self.send_command(SessionCommand::MouseWheel { x, y });
                                self.send_command(SessionCommand::Screenshot);
                            }
                        }
                        _ => {}
                    }
                }
            }

            if self.remote_input_focused && !ui.ctx().wants_keyboard_input() {
                self.handle_remote_keyboard(ui.ctx());
            }
        }
    }

    fn handle_remote_keyboard(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|input| input.events.clone());
        for event in events {
            match event {
                egui::Event::Text(text) if !text.is_empty() => {
                    self.send_command(SessionCommand::KeyText(text));
                    self.send_command(SessionCommand::Screenshot);
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } => {
                    if key == egui::Key::Escape && modifiers.ctrl {
                        self.remote_input_focused = false;
                        continue;
                    }
                    let remote_modifiers = egui_modifiers_to_control_keys(modifiers);
                    if has_command_modifier(modifiers) && egui_key_to_text(key).is_some() {
                        let text = egui_key_to_text(key).unwrap();
                        self.send_command(SessionCommand::KeyTextWithModifiers {
                            text,
                            modifiers: remote_modifiers,
                        });
                        self.send_command(SessionCommand::Screenshot);
                    } else if let Some(control_key) = egui_key_to_control_key(key) {
                        self.send_command(SessionCommand::KeyControlWithModifiers {
                            key: control_key,
                            modifiers: remote_modifiers,
                        });
                        self.send_command(SessionCommand::Screenshot);
                    }
                }
                _ => {}
            }
        }
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
            | SessionCommand::KeyTextWithModifiers { .. }
            | SessionCommand::KeyControlWithModifiers { .. }
            | SessionCommand::KeyEnter
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

fn remote_status_text(app: &EvertyDeskApp) -> String {
    let input = if app.remote_input_focused {
        "ввод"
    } else {
        "кликните экран"
    };
    let pending = if app.screenshot_pending { " ..." } else { "" };
    format!(
        "{}x{} {:.1} fps frame {} {input}{pending}",
        app.remote_size[0], app.remote_size[1], app.display_fps, app.screenshot_count
    )
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

fn egui_modifiers_to_control_keys(modifiers: egui::Modifiers) -> Vec<ControlKey> {
    let mut keys = Vec::new();
    if modifiers.alt {
        keys.push(ControlKey::Alt);
    }
    if modifiers.shift {
        keys.push(ControlKey::Shift);
    }
    if modifiers.ctrl {
        keys.push(ControlKey::Control);
    }
    if modifiers.mac_cmd || modifiers.command {
        keys.push(ControlKey::Meta);
    }
    keys
}

fn configure_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(20, 24, 28);
    visuals.window_fill = egui::Color32::from_rgb(18, 22, 26);
    visuals.extreme_bg_color = egui::Color32::from_rgb(12, 14, 16);
    visuals.selection.bg_fill = egui::Color32::from_rgb(45, 120, 170);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(34, 40, 46);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(46, 55, 62);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(54, 92, 118);
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.visuals.widgets.noninteractive.rounding = egui::Rounding::same(6.0);
    style.visuals.widgets.inactive.rounding = egui::Rounding::same(6.0);
    style.visuals.widgets.hovered.rounding = egui::Rounding::same(6.0);
    style.visuals.widgets.active.rounding = egui::Rounding::same(6.0);
    ctx.set_style(style);
}

fn normalize_remote_id(id: &str) -> String {
    id.chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-' && *ch != '_')
        .collect()
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
