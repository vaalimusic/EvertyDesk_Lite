//! AnyDesk/RustDesk-style persistent session toolbar, raised for every
//! active host session regardless of whether the main GUI is open,
//! minimized, or (in agent mode) not running at all — see
//! `host_agent.rs::raise_session_toolbar` and the `SessionStarted` handler
//! in `main.rs`. Complements `approval_prompt.rs` (which only handles the
//! initial accept/reject); this one stays up for the session's whole
//! lifetime and lets the host operator take control back from the remote
//! client or disconnect them, the same two actions the in-app floating
//! badge in `main.rs` already offered — just no longer tied to that
//! window's own visibility.
//!
//! Talks to whichever process is actually hosting over the same loopback
//! control endpoint as `approval_prompt.rs`
//! (`host_agent::try_connect_existing`), but — unlike that one-shot sender —
//! keeps the connection open so it can react to `HostEvent::SessionEnded`
//! and close itself automatically when the session ends.

use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::host::{HostCommand, HostEvent};

pub fn run(peer_id: String) -> eframe::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title("EvertyDesk Lite")
        .with_inner_size([300.0, 150.0])
        .with_min_inner_size([300.0, 150.0])
        .with_resizable(false)
        .with_always_on_top();

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "EvertyDesk Lite — активная сессия",
        options,
        Box::new(move |_cc| Ok(Box::new(ToolbarApp::new(peer_id)))),
    )
}

struct ToolbarApp {
    peer_id: String,
    started_at: Instant,
    blocked: bool,
    should_close: bool,
    event_rx: Option<mpsc::Receiver<HostEvent>>,
    command_tx: Option<mpsc::Sender<HostCommand>>,
}

impl ToolbarApp {
    fn new(peer_id: String) -> Self {
        let mut app = Self {
            peer_id,
            started_at: Instant::now(),
            blocked: false,
            should_close: false,
            event_rx: None,
            command_tx: None,
        };
        app.connect();
        app
    }

    /// Best-effort: if this fails, the toolbar still shows (so the operator
    /// at least sees the session is active) but its buttons become no-ops
    /// and it never auto-closes — matching `approval_prompt.rs`'s stance
    /// that a broken link should degrade, not crash.
    fn connect(&mut self) {
        let Some(stream) = crate::host_agent::try_connect_existing() else {
            return;
        };
        let Ok(read_half) = stream.try_clone() else {
            return;
        };
        let write_half = stream;

        let (event_tx, event_rx) = mpsc::channel::<HostEvent>();
        thread::Builder::new()
            .name("session-toolbar-reader".into())
            .spawn(move || {
                let mut lines = BufReader::new(read_half).lines();
                while let Some(Ok(line)) = lines.next() {
                    if let Ok(ev) = serde_json::from_str::<HostEvent>(&line) {
                        if event_tx.send(ev).is_err() {
                            return;
                        }
                    }
                }
                // EOF or read error — the agent process died or dropped us.
                // Dropping `event_tx` here (loop exit) makes the UI's
                // `event_rx.try_recv()` start returning `Disconnected`,
                // which it treats as "close the window" — no point leaving
                // a toolbar open for a link that can no longer do anything.
            })
            .ok();

        let (command_tx, command_rx) = mpsc::channel::<HostCommand>();
        thread::Builder::new()
            .name("session-toolbar-writer".into())
            .spawn(move || {
                let mut w = write_half;
                while let Ok(cmd) = command_rx.recv() {
                    let Ok(line) = serde_json::to_string(&cmd) else {
                        continue;
                    };
                    if writeln!(w, "{line}").is_err() {
                        return;
                    }
                    let _ = w.flush();
                }
            })
            .ok();

        self.event_rx = Some(event_rx);
        self.command_tx = Some(command_tx);
    }

    fn send(&self, cmd: HostCommand) {
        if let Some(tx) = &self.command_tx {
            let _ = tx.send(cmd);
        }
    }
}

impl eframe::App for ToolbarApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if let Some(rx) = &self.event_rx {
            loop {
                match rx.try_recv() {
                    Ok(HostEvent::SessionEnded { peer_id, .. }) if peer_id == self.peer_id => {
                        self.should_close = true;
                    }
                    Ok(_) => {}
                    Err(mpsc::TryRecvError::Empty) => break,
                    // Reader thread ended (agent died / connection dropped) —
                    // nothing useful this toolbar can still do.
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.should_close = true;
                        break;
                    }
                }
            }
        }
        if self.should_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("📡").size(20.0));
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("К вам подключены")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(120, 120, 120)),
                    );
                    ui.label(egui::RichText::new(&self.peer_id).size(15.0).strong());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    let secs = self.started_at.elapsed().as_secs();
                    ui.label(
                        egui::RichText::new(format!("{:02}:{:02}", secs / 60, secs % 60))
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(120, 120, 120)),
                    );
                });
            });

            ui.add_space(10.0);

            let (take_label, take_fill) = if self.blocked {
                (
                    "🔓 Вернуть управление клиенту",
                    egui::Color32::from_rgb(180, 70, 70),
                )
            } else {
                (
                    "🔒 Взять управление себе",
                    egui::Color32::from_rgb(30, 120, 50),
                )
            };
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new(take_label)
                            .size(12.0)
                            .color(egui::Color32::WHITE),
                    )
                    .fill(take_fill)
                    .min_size(egui::vec2(ui.available_width(), 28.0))
                    .corner_radius(egui::CornerRadius::same(6)),
                )
                .clicked()
            {
                self.blocked = !self.blocked;
                self.send(HostCommand::SetInputBlocked(self.blocked));
            }

            ui.add_space(6.0);

            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("⛔ Отключить клиента")
                            .size(12.0)
                            .color(egui::Color32::WHITE),
                    )
                    .fill(egui::Color32::from_rgb(150, 40, 40))
                    .min_size(egui::vec2(ui.available_width(), 28.0))
                    .corner_radius(egui::CornerRadius::same(6)),
                )
                .clicked()
            {
                self.send(HostCommand::KickActiveSession);
            }
        });

        ctx.request_repaint_after(Duration::from_millis(500));
    }
}
