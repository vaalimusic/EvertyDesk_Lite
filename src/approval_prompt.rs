//! Phase 2 of `TZ_HOST_SERVICE.md`: a standalone "accept this connection?"
//! window, raised by the `--host-agent` process itself (see
//! `host_agent.rs::raise_approval_prompt`) when an incoming connection needs
//! confirmation and no main GUI is currently attached to show it.
//!
//! Deliberately tiny: this is not a second copy of the main window, just a
//! prompt. It talks to the agent the same way the approval decision always
//! travels — one `HostCommand::ApproveIncoming` line over the same loopback
//! socket (`host_agent::try_connect_existing`) — then exits. It does not
//! receive `HostEvent`s or stay connected; there is nothing else for it to
//! do once the user answers (or the request times out on the host side,
//! `wait_for_approval`'s own 45s deadline in `host.rs`).

use std::io::Write;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::host::HostCommand;

const TIMEOUT: Duration = Duration::from_secs(45);

/// Entry point for `--approval-prompt <peer_id>`.
pub fn run(peer_id: String) -> eframe::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title("EvertyDesk Lite")
        .with_inner_size([440.0, 190.0])
        .with_min_inner_size([440.0, 190.0])
        .with_resizable(false)
        .with_always_on_top();

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "EvertyDesk Lite — подтверждение подключения",
        options,
        Box::new(move |_cc| Ok(Box::new(ApprovalPromptApp::new(peer_id)))),
    )
}

struct ApprovalPromptApp {
    peer_id: String,
    deadline: Instant,
    decided: bool,
}

impl ApprovalPromptApp {
    fn new(peer_id: String) -> Self {
        Self {
            peer_id,
            deadline: Instant::now() + TIMEOUT,
            decided: false,
        }
    }

    fn decide(&mut self, accept: bool) {
        send_decision(&self.peer_id, accept);
        self.decided = true;
    }
}

impl eframe::App for ApprovalPromptApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if self.decided {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if Instant::now() >= self.deadline {
            // Don't send a decision — let the host side's own 45s timeout in
            // `wait_for_approval` (host.rs) handle it consistently, whether
            // this window ever ran at all or not.
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.add_space(14.0);
            ui.heading("Входящее подключение");
            ui.add_space(6.0);
            ui.label(format!(
                "{} запрашивает удалённый доступ без пароля.",
                self.peer_id
            ));
            ui.label("Разрешить подключение?");
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                if ui.button("✔  Разрешить").clicked() {
                    self.decide(true);
                }
                if ui.button("✘  Отклонить").clicked() {
                    self.decide(false);
                }
            });
        });

        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

fn send_decision(peer_id: &str, accept: bool) {
    let Some(mut stream) = crate::host_agent::try_connect_existing() else {
        return;
    };
    let cmd = HostCommand::ApproveIncoming {
        peer_id: peer_id.to_owned(),
        accept,
    };
    if let Ok(line) = serde_json::to_string(&cmd) {
        let _ = writeln!(stream, "{line}");
        let _ = stream.flush();
    }
}
