use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use eframe::egui;

use crate::transport::SessionCommand;
use crate::{format_peer_id, llm, AppMode, EvertyDeskApp};

const TERMINAL_AI_EXAMPLES: [(&str, &str); 5] = [
    (
        "Ошибка",
        "Объясни последнюю ошибку в терминале и предложи безопасную команду для диагностики.",
    ),
    (
        "Сервис",
        "Проверь состояние службы, последние логи и предложи аккуратный план восстановления.",
    ),
    (
        "Сеть",
        "Помоги проверить DNS, доступность хоста, маршрут и открытые порты.",
    ),
    (
        "Диск",
        "Найди, что занимает место на диске, без удаления данных и рискованных действий.",
    ),
    (
        "Пакет",
        "Помоги установить или обновить пакет с учетом дистрибутива Linux.",
    ),
];

impl EvertyDeskApp {
    pub(crate) fn poll_terminal_ai(&mut self) {
        let Some(rx) = self.terminal_ai_rx.take() else {
            return;
        };

        match rx.try_recv() {
            Ok(Ok(answer)) => {
                self.terminal_ai_answer = answer;
                self.terminal_ai_status = Some("AI ответил".to_owned());
            }
            Ok(Err(err)) => {
                self.terminal_ai_status = Some(format!("AI ошибка: {err}"));
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.terminal_ai_rx = Some(rx);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.terminal_ai_status = Some("AI запрос прерван".to_owned());
            }
        }
    }

    pub(crate) fn maybe_request_terminal_auto_ai(&mut self) {
        if !self.terminal_auto_pending
            || !self.config.llm.enabled
            || !self.config.llm.auto_suggest
            || self.terminal_ai_rx.is_some()
        {
            return;
        }

        let due = self
            .terminal_auto_request_at
            .map(|at| at.elapsed() >= Duration::from_millis(900))
            .unwrap_or(false);
        if !due {
            return;
        }

        self.terminal_auto_pending = false;
        let goal = if self.shell_last_command.trim().is_empty() {
            "Автоматически проанализируй последний вывод терминала и предложи следующий шаг."
                .to_owned()
        } else {
            format!(
                "Автоматически проанализируй вывод после команды `{}` и предложи следующий безопасный шаг.",
                self.shell_last_command.trim()
            )
        };
        self.request_terminal_ai(goal);
    }

    pub(crate) fn trim_shell_output(&mut self) {
        const MAX_BYTES: usize = 120_000;
        if self.shell_output.len() <= MAX_BYTES {
            return;
        }
        let remove_at_least = self.shell_output.len() - MAX_BYTES;
        let split = self
            .shell_output
            .char_indices()
            .find(|(idx, _)| *idx >= remove_at_least)
            .map(|(idx, _)| idx)
            .unwrap_or(remove_at_least);
        self.shell_output.drain(..split);
    }

    #[allow(deprecated)]
    pub(crate) fn shell_window(&mut self, ctx: &egui::Context) {
        let title = format!("EvertyDesk Terminal - {}", format_peer_id(&self.remote_id));
        let viewport_id = egui::ViewportId::from_hash_of("evertydesk-lite-shell");
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([980.0, 680.0])
            .with_min_inner_size([720.0, 480.0]);

        ctx.show_viewport_immediate(viewport_id, builder, |ctx, _class| {
            if ctx.input(|input| input.viewport().close_requested()) {
                self.send_command(SessionCommand::ShellStop);
                self.disconnect_session("Консоль закрыта");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }

            egui::CentralPanel::default()
                .frame(
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(0x08, 0x0C, 0x12))
                        .inner_margin(egui::Margin::same(14)),
                )
                .show(ctx, |ui| {
                    let provider = self.config.llm.provider.label();
                    let ai_status = llm::provider_status(&self.config.llm);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("TERMINAL")
                                .size(13.0)
                                .strong()
                                .color(egui::Color32::from_rgb(0x7D, 0xF2, 0xB3)),
                        );
                        ui.add_space(8.0);
                        terminal_badge(
                            ui,
                            &format!("remote {}", format_peer_id(&self.remote_id)),
                            egui::Color32::from_rgb(0x1C, 0x27, 0x35),
                        );
                        terminal_badge(
                            ui,
                            if self.connected {
                                "connected"
                            } else {
                                "offline"
                            },
                            if self.connected {
                                egui::Color32::from_rgb(0x0F, 0x42, 0x2A)
                            } else {
                                egui::Color32::from_rgb(0x42, 0x1B, 0x1B)
                            },
                        );
                        terminal_badge(
                            ui,
                            &format!("AI {provider}: {ai_status}"),
                            egui::Color32::from_rgb(0x18, 0x22, 0x32),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button(self.text("Настройки AI", "AI settings"))
                                .clicked()
                            {
                                self.mode = AppMode::Settings;
                            }
                            if ui.small_button(self.text("Очистить", "Clear")).clicked() {
                                self.shell_output.clear();
                            }
                        });
                    });

                    ui.add_space(10.0);

                    let terminal_height = (ui.available_height() - 252.0).max(190.0);
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(0x05, 0x08, 0x0D))
                        .stroke(egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgb(0x1D, 0x2B, 0x3D),
                        ))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .stick_to_bottom(true)
                                .max_height(terminal_height)
                                .show(ui, |ui| {
                                    terminal_text_edit_scope(ui, |ui| {
                                        ui.add_sized(
                                            egui::vec2(ui.available_width(), terminal_height),
                                            egui::TextEdit::multiline(&mut self.shell_output)
                                                .font(egui::TextStyle::Monospace)
                                                .text_color(egui::Color32::from_rgb(
                                                    0xDF, 0xEA, 0xF7,
                                                ))
                                                .desired_width(f32::INFINITY)
                                                .frame(egui::Frame::NONE)
                                                .interactive(false),
                                        );
                                    });
                                });
                        });

                    ui.add_space(8.0);

                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(0x0D, 0x14, 0x1F))
                        .stroke(egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgb(0x24, 0x34, 0x49),
                        ))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("$")
                                        .monospace()
                                        .size(16.0)
                                        .color(egui::Color32::from_rgb(0x7D, 0xF2, 0xB3)),
                                );
                                let response = terminal_text_edit_scope(ui, |ui| {
                                    ui.add_sized(
                                        egui::vec2((ui.available_width() - 88.0).max(160.0), 30.0),
                                        egui::TextEdit::singleline(&mut self.shell_input)
                                            .hint_text("command")
                                            .font(egui::TextStyle::Monospace)
                                            .text_color(crate::theme::palette().surface_raised)
                                            .frame(egui::Frame::NONE),
                                    )
                                });
                                if response.has_focus()
                                    && ui.input(|input| input.key_pressed(egui::Key::ArrowUp))
                                {
                                    self.navigate_shell_history(true);
                                }
                                if response.has_focus()
                                    && ui.input(|input| input.key_pressed(egui::Key::ArrowDown))
                                {
                                    self.navigate_shell_history(false);
                                }
                                let send = response.lost_focus()
                                    && ui.input(|input| input.key_pressed(egui::Key::Enter));
                                let enter_clicked = ui
                                    .add_enabled(
                                        self.connected,
                                        egui::Button::new(self.text("Ввод", "Enter"))
                                            .min_size(egui::vec2(72.0, 30.0)),
                                    )
                                    .clicked();
                                if self.connected && (enter_clicked || send) {
                                    self.submit_shell_input();
                                    response.request_focus();
                                }
                            });
                        });

                    ui.add_space(8.0);
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(0x0D, 0x13, 0x1D))
                        .stroke(egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgb(0x22, 0x30, 0x44),
                        ))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let task_hint = self
                                    .text("что нужно сделать или исправить", "what to do or fix");
                                ui.label(
                                    egui::RichText::new(self.text("AI задача", "AI task"))
                                        .size(12.0)
                                        .strong()
                                        .color(egui::Color32::from_rgb(0x9D, 0xA8, 0xBA)),
                                );
                                let goal_width = (ui.available_width() - 246.0).max(180.0);
                                terminal_text_edit_scope(ui, |ui| {
                                    ui.add_sized(
                                        egui::vec2(goal_width, 28.0),
                                        egui::TextEdit::singleline(&mut self.terminal_goal)
                                            .hint_text(task_hint)
                                            .font(egui::TextStyle::Button)
                                            .text_color(crate::theme::palette().surface_raised)
                                            .frame(egui::Frame::NONE),
                                    );
                                });
                                let ask_clicked = ui
                                    .add_enabled(
                                        self.terminal_ai_rx.is_none(),
                                        egui::Button::new(self.text("Анализ", "Analyze"))
                                            .min_size(egui::vec2(78.0, 28.0)),
                                    )
                                    .clicked();
                                if ask_clicked {
                                    self.request_terminal_ai(self.terminal_goal.clone());
                                }
                                if self.terminal_ai_rx.is_some() {
                                    ui.spinner();
                                }
                            });

                            ui.add_space(7.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    egui::RichText::new(self.text("Примеры", "Examples"))
                                        .size(12.0)
                                        .color(egui::Color32::from_rgb(0x7F, 0x8B, 0x9E)),
                                );
                                for (label, goal) in TERMINAL_AI_EXAMPLES {
                                    if terminal_example_button(ui, label).clicked() {
                                        self.terminal_goal = goal.to_owned();
                                    }
                                }
                            });

                            if let Some(status) = &self.terminal_ai_status {
                                ui.add_space(5.0);
                                ui.label(
                                    egui::RichText::new(status)
                                        .size(12.0)
                                        .color(egui::Color32::from_rgb(0x9D, 0xA8, 0xBA)),
                                );
                            }

                            if !self.terminal_ai_answer.trim().is_empty() {
                                ui.add_space(7.0);
                                egui::ScrollArea::vertical()
                                    .max_height(82.0)
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(&self.terminal_ai_answer)
                                                .size(13.0)
                                                .color(crate::theme::palette().surface_sunken),
                                        );
                                    });
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    if ui
                                        .button(self.text("Вставить команду", "Insert command"))
                                        .clicked()
                                    {
                                        if let Some(command) = self.first_ai_command() {
                                            self.shell_input = command;
                                        }
                                    }
                                    if ui.button(self.text("Копировать", "Copy")).clicked()
                                    {
                                        ui.ctx().copy_text(self.terminal_ai_answer.clone());
                                    }
                                    if ui.button(self.text("Скрыть", "Hide")).clicked() {
                                        self.terminal_ai_answer.clear();
                                        self.terminal_ai_status = None;
                                    }
                                });
                            }
                        });
                });
        });
    }

    fn submit_shell_input(&mut self) {
        let mut input = std::mem::take(&mut self.shell_input);
        let command = input.trim_end_matches(&['\r', '\n'][..]).to_owned();
        if !input.ends_with('\n') {
            input.push('\n');
        }

        if !command.trim().is_empty() {
            if self
                .shell_history
                .last()
                .map(|last| last != &command)
                .unwrap_or(true)
            {
                self.shell_history.push(command.clone());
                if self.shell_history.len() > 100 {
                    self.shell_history.remove(0);
                }
            }
            self.shell_last_command = command.clone();
        }
        self.shell_history_pos = None;
        self.shell_output.push_str(&format!("\r\n$ {input}"));
        self.trim_shell_output();
        self.send_command(SessionCommand::ShellInput(input));

        if self.config.llm.enabled && self.config.llm.auto_suggest {
            self.terminal_auto_pending = true;
            self.terminal_auto_request_at = Some(Instant::now());
        }
    }

    fn navigate_shell_history(&mut self, older: bool) {
        if self.shell_history.is_empty() {
            return;
        }

        if older {
            let next = self
                .shell_history_pos
                .unwrap_or(self.shell_history.len())
                .saturating_sub(1);
            self.shell_history_pos = Some(next);
            self.shell_input = self.shell_history[next].clone();
            return;
        }

        if let Some(pos) = self.shell_history_pos {
            let next = pos + 1;
            if next < self.shell_history.len() {
                self.shell_history_pos = Some(next);
                self.shell_input = self.shell_history[next].clone();
            } else {
                self.shell_history_pos = None;
                self.shell_input.clear();
            }
        }
    }

    fn request_terminal_ai(&mut self, goal: String) {
        if self.terminal_ai_rx.is_some() {
            self.terminal_ai_status = Some("AI уже анализирует терминал".to_owned());
            return;
        }

        let config = self.config.llm.clone();
        let transcript = self.shell_output.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = llm::terminal_suggestion(config, transcript, goal);
            let _ = tx.send(result);
        });
        self.terminal_ai_rx = Some(rx);
        self.terminal_ai_status = Some("AI анализирует терминал...".to_owned());
    }

    fn first_ai_command(&self) -> Option<String> {
        let mut in_code = false;
        for raw in self.terminal_ai_answer.lines() {
            let line = raw.trim();
            if line.starts_with("```") {
                in_code = !in_code;
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if in_code || line.starts_with('$') || line.starts_with('>') {
                let command = line
                    .trim_start_matches('$')
                    .trim_start_matches('>')
                    .trim()
                    .to_owned();
                if !command.is_empty() {
                    return Some(command);
                }
            }
        }
        None
    }
}

fn terminal_badge(ui: &mut egui::Ui, text: &str, fill: egui::Color32) {
    egui::Frame::NONE
        .fill(fill)
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(11.0)
                    .color(egui::Color32::from_rgb(0xD8, 0xE2, 0xEF)),
            );
        });
}

fn terminal_example_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(12.0)
                .color(egui::Color32::from_rgb(0xD9, 0xE5, 0xF3)),
        )
        .min_size(egui::vec2(64.0, 24.0))
        .fill(egui::Color32::from_rgb(0x13, 0x1E, 0x2B))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(0x2B, 0x3C, 0x52),
        ))
        .corner_radius(egui::CornerRadius::same(6)),
    )
}

fn terminal_text_edit_scope<R>(
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        visuals.extreme_bg_color = egui::Color32::TRANSPARENT;
        visuals.selection.bg_fill = egui::Color32::from_rgb(0x19, 0x5B, 0x45);
        visuals.selection.stroke =
            egui::Stroke::new(1.0, egui::Color32::from_rgb(0x7D, 0xF2, 0xB3));

        visuals.widgets.noninteractive.bg_fill = egui::Color32::TRANSPARENT;
        visuals.widgets.noninteractive.weak_bg_fill = egui::Color32::TRANSPARENT;
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;

        visuals.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
        visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
        visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;

        visuals.widgets.hovered.bg_fill = egui::Color32::TRANSPARENT;
        visuals.widgets.hovered.weak_bg_fill = egui::Color32::TRANSPARENT;
        visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;

        visuals.widgets.active.bg_fill = egui::Color32::TRANSPARENT;
        visuals.widgets.active.weak_bg_fill = egui::Color32::TRANSPARENT;
        visuals.widgets.active.bg_stroke = egui::Stroke::NONE;

        add_contents(ui)
    })
    .inner
}
