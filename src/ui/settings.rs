use base64::Engine as _;
use eframe::egui;

use crate::settings::{
    self as settings_mod, AppConfig, CodecPreference, EncoderPreference, FsrQualitySetting,
    LlmProvider, ServerConfig, StreamingMode,
};
use crate::ui::widgets::{language_button, settings_section, settings_text_row};
use crate::{
    install_host_service, start_installed_service, stop_installed_service, tr,
    uninstall_host_service, EvertyDeskApp, SessionCommand, UiLang, APP_NAME, APP_VERSION,
};

impl EvertyDeskApp {
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let mut selected_lang = self.ui_lang;
        egui::Window::new(self.text("Настройки", "Settings"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .show(ctx, |ui| {
                let draft = match self.settings_draft.as_mut() {
                    Some(d) => d,
                    None => return,
                };

                egui::ScrollArea::vertical().show(ui, |ui| {
                    settings_section(ui, tr(selected_lang, "Общие", "General"), |ui| {
                        ui.horizontal(|ui| {
                            ui.label(tr(selected_lang, "Язык интерфейса", "Interface language"));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if language_button(ui, "EN", selected_lang == UiLang::En)
                                        .clicked()
                                    {
                                        selected_lang = UiLang::En;
                                    }
                                    if language_button(ui, "RU", selected_lang == UiLang::Ru)
                                        .clicked()
                                    {
                                        selected_lang = UiLang::Ru;
                                    }
                                },
                            );
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(tr(selected_lang, "Масштаб интерфейса", "UI scale"));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if draft.ui.ui_scale == 0.0 {
                                        // Show platform default
                                        let default_scale = if cfg!(target_os = "macos") {
                                            1.08_f32
                                        } else {
                                            1.0_f32
                                        };
                                        draft.ui.ui_scale = default_scale;
                                    }
                                    let label = format!("{:.0}%", draft.ui.ui_scale * 100.0);
                                    ui.label(&label);
                                    ui.add(
                                        egui::Slider::new(&mut draft.ui.ui_scale, 0.75..=2.0)
                                            .step_by(0.05)
                                            .show_value(false)
                                            .trailing_fill(true),
                                    );
                                },
                            );
                        });
                    });

                    ui.add_space(8.0);

                    network_section(ui, selected_lang, draft, &mut self.settings_custom_server);

                    ui.add_space(8.0);

                    security_section(
                        ui,
                        selected_lang,
                        draft,
                        &mut self.show_password,
                        &mut self.whitelist_new_id,
                    );

                    ui.add_space(8.0);

                    settings_section(ui, tr(selected_lang, "Видео", "Video"), |ui| {
                        video_settings_body(ui, selected_lang, draft);
                    });
                });

                // Detect unsaved changes.
                let has_changes = self
                    .settings_draft
                    .as_ref()
                    .map(|d| {
                        serde_json::to_string(d).ok() != serde_json::to_string(&self.config).ok()
                    })
                    .unwrap_or(false);

                ui.separator();
                if has_changes {
                    ui.label(
                        egui::RichText::new(tr(
                            selected_lang,
                            "⚠ Есть несохранённые изменения",
                            "⚠ You have unsaved changes",
                        ))
                        .size(12.0)
                        .color(egui::Color32::from_rgb(0xFF, 0xB7, 0x47)),
                    );
                }
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(tr(selected_lang, "Сохранить", "Save"))
                                .min_size(egui::vec2(110.0, 32.0)),
                        )
                        .clicked()
                    {
                        let new_cfg = self.settings_draft.take().unwrap();
                        let host_reconfigure_needed = new_cfg.server.id_server
                            != self.config.server.id_server
                            || new_cfg.server.relay_server != self.config.server.relay_server
                            || new_cfg.server.public_key != self.config.server.public_key
                            || new_cfg.display.target_fps != self.config.display.target_fps
                            || new_cfg.display.codec != self.config.display.codec
                            || new_cfg.display.encoder != self.config.display.encoder
                            || new_cfg.display.streaming_mode != self.config.display.streaming_mode
                            || new_cfg.display.fsr_quality != self.config.display.fsr_quality
                            || (new_cfg.display.fsr_sharpness - self.config.display.fsr_sharpness)
                                .abs()
                                > f32::EPSILON
                            || new_cfg.local_password != self.config.local_password;
                        let next_video_fps = new_cfg.display.target_fps.clamp(5, 60) as i32;
                        if host_reconfigure_needed {
                            if let Some(svc) = &self.host_service {
                                svc.reconfigure(new_cfg.clone());
                            }
                        }
                        self.config = new_cfg;
                        self.fsr_viewer = make_fsr_adapter(&self.config.display);
                        self.fsr_native_size = None;
                        self.video_fps = next_video_fps;
                        self.config.save();
                        if self.connected {
                            self.send_command(SessionCommand::SetVideoProfile {
                                fps: self.video_fps,
                                codec: self.config.display.codec,
                            });
                        }
                        self.show_settings = false;
                    }
                    {
                        let close_label = if has_changes {
                            tr(selected_lang, "Отменить изменения", "Discard changes")
                        } else {
                            tr(selected_lang, "Закрыть", "Close")
                        };
                        let close_btn = if has_changes {
                            egui::Button::new(
                                egui::RichText::new(close_label)
                                    .color(egui::Color32::from_rgb(0xF0, 0x6A, 0x6A)),
                            )
                        } else {
                            egui::Button::new(egui::RichText::new(close_label))
                        };
                        if ui.add(close_btn).clicked() {
                            self.settings_draft = None;
                            self.show_settings = false;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(tr(selected_lang, "Сбросить", "Reset"))
                            .on_hover_text(tr(
                                selected_lang,
                                "Вернуть настройки по умолчанию",
                                "Restore defaults",
                            ))
                            .clicked()
                        {
                            *self.settings_draft.as_mut().unwrap() =
                                default_config_from(&self.config);
                        }
                    });
                });
            });
        self.ui_lang = selected_lang;
        if !open {
            self.settings_draft = None;
            self.show_settings = false;
        }
    }

    pub(crate) fn settings_ui(&mut self, ui: &mut egui::Ui) {
        if self.settings_draft.is_none() {
            self.settings_draft = Some(self.config.clone());
            // Pre-fill custom server buffer only when already using a custom server.
            let default_srv = ServerConfig::default();
            if self.config.server != default_srv {
                self.settings_custom_server = self.config.server.clone();
            }
        }

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(self.text("Настройки", "Settings"))
                        .size(28.0)
                        .strong()
                        .color(crate::theme::palette().text),
                );
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(self.text(
                        "Безопасность, видео, сеть и AI терминал",
                        "Security, video, network and AI terminal",
                    ))
                    .size(13.0)
                    .color(crate::theme::palette().text_weak),
                );
            });
        });
        ui.add_space(12.0);

        let mut selected_lang = self.ui_lang;
        let draft = match self.settings_draft.as_mut() {
            Some(d) => d,
            None => return,
        };
        let current_config = self.config.clone();
        let host_reconfigure_source = (
            current_config.server.id_server.clone(),
            current_config.server.relay_server.clone(),
            current_config.server.public_key.clone(),
            current_config.display.target_fps,
            current_config.display.codec,
            current_config.display.encoder,
            current_config.display.streaming_mode,
            current_config.display.fsr_quality,
            current_config.display.fsr_sharpness,
            current_config.local_password.clone(),
        );

        // Reserve bottom area for Save/Cancel buttons before the scroll area,
        // otherwise ScrollArea expands to fill all height and buttons go off-screen.
        let buttons_h = 58.0;
        let scroll_h = (ui.available_height() - buttons_h).max(200.0);

        egui::ScrollArea::vertical()
            .max_height(scroll_h)
            .show(ui, |ui| {
                // ── General ──────────────────────────────────────────────────────
                settings_section(ui, tr(selected_lang, "Общие", "General"), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(tr(selected_lang, "Язык интерфейса", "Interface language"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if language_button(ui, "EN", selected_lang == UiLang::En).clicked() {
                                selected_lang = UiLang::En;
                            }
                            if language_button(ui, "RU", selected_lang == UiLang::Ru).clicked() {
                                selected_lang = UiLang::Ru;
                            }
                        });
                    });
                    ui.add_space(6.0);
                    // Переключатель темы оформления — с мгновенным предпросмотром.
                    ui.horizontal(|ui| {
                        ui.label(tr(selected_lang, "Тема оформления", "Theme"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            use crate::theme::ThemeMode;
                            let dark_label = tr(selected_lang, "🌙 Тёмная", "🌙 Dark");
                            let light_label = tr(selected_lang, "☀ Светлая", "☀ Light");
                            let auto_label = tr(selected_lang, "⚙ Авто", "⚙ Auto");
                            if language_button(
                                ui,
                                light_label,
                                draft.ui.theme_mode == ThemeMode::Light,
                            )
                            .clicked()
                            {
                                draft.ui.theme_mode = ThemeMode::Light;
                                crate::theme::apply(ui.ctx(), ThemeMode::Light);
                            }
                            if language_button(
                                ui,
                                dark_label,
                                draft.ui.theme_mode == ThemeMode::Dark,
                            )
                            .clicked()
                            {
                                draft.ui.theme_mode = ThemeMode::Dark;
                                crate::theme::apply(ui.ctx(), ThemeMode::Dark);
                            }
                            if language_button(
                                ui,
                                auto_label,
                                draft.ui.theme_mode == ThemeMode::System,
                            )
                            .clicked()
                            {
                                draft.ui.theme_mode = ThemeMode::System;
                                crate::theme::apply(ui.ctx(), ThemeMode::System);
                            }
                        });
                    });
                    ui.add_space(6.0);
                    ui.checkbox(
                        &mut draft.ui.show_connection_details,
                        tr(
                            selected_lang,
                            "Показывать детали подключения на главной",
                            "Show connection details on main page",
                        ),
                    );
                    ui.add_space(6.0);
                    ui.checkbox(
                        &mut draft.ui.show_tech_info,
                        tr(
                            selected_lang,
                            "Показывать техническую информацию (fps, битрейт, кодек)",
                            "Show technical info (fps, bitrate, codec)",
                        ),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        use crate::settings::CoordinateMode;
                        ui.label(tr(
                            selected_lang,
                            "Режим координат мыши",
                            "Mouse coordinate mode",
                        ));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            for (mode, label_ru, label_en) in [
                                (CoordinateMode::Auto, "Авто", "Auto"),
                                (CoordinateMode::Absolute, "Абсолютный", "Absolute"),
                                (CoordinateMode::Local, "Локальный", "Local"),
                            ] {
                                let label = match selected_lang {
                                    crate::UiLang::Ru => label_ru,
                                    crate::UiLang::En => label_en,
                                };
                                if language_button(ui, label, draft.ui.coordinate_mode == mode)
                                    .clicked()
                                {
                                    draft.ui.coordinate_mode = mode;
                                }
                            }
                        });
                    });
                });

                ui.add_space(8.0);

                // ── Security (with password) ─────────────────────────────────────
                security_section(
                    ui,
                    selected_lang,
                    draft,
                    &mut self.show_password,
                    &mut self.whitelist_new_id,
                );

                ui.add_space(8.0);

                // ── Video ─────────────────────────────────────────────────────────
                settings_section(ui, tr(selected_lang, "Видео", "Video"), |ui| {
                    video_settings_body(ui, selected_lang, draft);
                });

                ui.add_space(8.0);

                // ── Network (collapsed by default) ───────────────────────────────
                network_section(ui, selected_lang, draft, &mut self.settings_custom_server);

                ui.add_space(8.0);

                llm_settings_section(ui, selected_lang, draft);

                ui.add_space(8.0);

                hotfix_settings_section(ui, selected_lang, draft);

                ui.add_space(8.0);

                // ── Windows service ──────────────────────────────────────────────
                settings_section(ui, tr(selected_lang, "Служба", "Service"), |ui| {
                    ui.label(
                        egui::RichText::new(tr(
                            selected_lang,
                            "Фоновый режим: тот же исполняемый файл с аргументом --host.",
                            "Background mode: same executable with the --host argument.",
                        ))
                        .size(12.0)
                        .color(crate::theme::palette().text_weak),
                    );
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .button(tr(selected_lang, "Установить службу", "Install service"))
                            .clicked()
                        {
                            self.service_status = Some(match install_host_service() {
                                Ok(msg) => msg,
                                Err(err) => err,
                            });
                        }
                        if ui.button(tr(selected_lang, "Запустить", "Start")).clicked() {
                            self.service_status = Some(match start_installed_service() {
                                Ok(msg) => msg,
                                Err(err) => err,
                            });
                        }
                        if ui.button(tr(selected_lang, "Остановить", "Stop")).clicked() {
                            self.service_status = Some(match stop_installed_service() {
                                Ok(msg) => msg,
                                Err(err) => err,
                            });
                        }
                        if ui
                            .button(tr(selected_lang, "Удалить службу", "Uninstall service"))
                            .clicked()
                        {
                            self.service_status = Some(match uninstall_host_service() {
                                Ok(msg) => msg,
                                Err(err) => err,
                            });
                        }
                    });
                    if let Some(status) = &self.service_status {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(status)
                                .size(12.0)
                                .color(crate::theme::palette().text_weak),
                        );
                    }
                });

                ui.add_space(8.0);
                settings_section(
                    ui,
                    tr(selected_lang, "О программе", "About"),
                    |ui| {
                        ui.label(
                            egui::RichText::new(format!("{APP_NAME} v{APP_VERSION}"))
                                .color(crate::theme::palette().text),
                        );
                        ui.label(format!(
                            "{}: {}",
                            tr(selected_lang, "Конфиг", "Config"),
                            settings_mod::config_path().display()
                        ));
                    },
                );
            });

        self.ui_lang = selected_lang;
        ui.add_space(10.0);
        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(tr(selected_lang, "Сохранить", "Save"))
                        .min_size(egui::vec2(112.0, 36.0)),
                )
                .clicked()
            {
                let new_cfg = self.settings_draft.clone().unwrap();
                let host_reconfigure_needed = new_cfg.server.id_server != host_reconfigure_source.0
                    || new_cfg.server.relay_server != host_reconfigure_source.1
                    || new_cfg.server.public_key != host_reconfigure_source.2
                    || new_cfg.display.target_fps != host_reconfigure_source.3
                    || new_cfg.display.codec != host_reconfigure_source.4
                    || new_cfg.display.encoder != host_reconfigure_source.5
                    || new_cfg.display.streaming_mode != host_reconfigure_source.6
                    || new_cfg.display.fsr_quality != host_reconfigure_source.7
                    || (new_cfg.display.fsr_sharpness - host_reconfigure_source.8).abs()
                        > f32::EPSILON
                    || new_cfg.local_password != host_reconfigure_source.9;
                let next_video_fps = new_cfg.display.target_fps.clamp(5, 60) as i32;
                if host_reconfigure_needed {
                    if let Some(svc) = &self.host_service {
                        svc.reconfigure(new_cfg.clone());
                    }
                }
                self.config = new_cfg.clone();
                self.fsr_viewer = make_fsr_adapter(&self.config.display);
                self.fsr_native_size = None;
                self.video_fps = next_video_fps;
                self.config.save();
                if self.connected {
                    self.send_command(SessionCommand::SetVideoProfile {
                        fps: self.video_fps,
                        codec: self.config.display.codec,
                    });
                }
                self.settings_draft = Some(new_cfg);
            }
            if ui.button(tr(selected_lang, "Отменить", "Cancel")).clicked() {
                // Откатываем тему-предпросмотр к сохранённой.
                crate::theme::apply(ui.ctx(), self.config.ui.theme_mode);
                self.settings_draft = Some(self.config.clone());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(tr(selected_lang, "Сбросить", "Reset"))
                    .on_hover_text(tr(
                        selected_lang,
                        "Вернуть настройки по умолчанию",
                        "Restore defaults",
                    ))
                    .clicked()
                {
                    self.settings_draft = Some(default_config_from(&self.config));
                }
            });
        });
    }
}

// ── Security section (shared by both window and page) ────────────────────────

fn security_section(
    ui: &mut egui::Ui,
    selected_lang: UiLang,
    draft: &mut AppConfig,
    show_password: &mut bool,
    whitelist_new_id: &mut String,
) {
    settings_section(
        ui,
        tr(selected_lang, "Безопасность", "Security"),
        |ui| {
            // ── Password row ─────────────────────────────────────────────────────
            {
                let label = tr(selected_lang, "Пароль доступа", "Access password");
                let hint = tr(
                    selected_lang,
                    "Не задан — нужно подтверждение",
                    "Not set — approval required",
                );
                let eye_icon = if *show_password { "🙈" } else { "👁" };

                ui.horizontal(|ui| {
                    ui.set_min_height(36.0);
                    ui.set_width(ui.available_width());
                    ui.add_sized(
                        egui::vec2(150.0, 24.0),
                        egui::Label::new(
                            egui::RichText::new(label)
                                .size(13.0)
                                .color(crate::theme::palette().text_weak),
                        )
                        .truncate(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("↻")
                            .on_hover_text(tr(
                                selected_lang,
                                "Сгенерировать новый пароль",
                                "Generate new password",
                            ))
                            .clicked()
                        {
                            draft.local_password = crate::settings::generate_numeric_token(6);
                        }
                        if ui
                            .small_button(eye_icon)
                            .on_hover_text(tr(
                                selected_lang,
                                "Показать / скрыть пароль",
                                "Show / hide password",
                            ))
                            .clicked()
                        {
                            *show_password = !*show_password;
                        }
                        let width = ui.available_width().min(300.0);
                        ui.add_sized(
                            egui::vec2(width, 34.0),
                            egui::TextEdit::singleline(&mut draft.local_password)
                                .hint_text(hint)
                                .password(!*show_password)
                                .font(egui::TextStyle::Monospace),
                        );
                    });
                });
                ui.add_space(2.0);
                ui.label(
                egui::RichText::new(tr(
                    selected_lang,
                    "Клиент вводит этот пароль — подключается без диалога подтверждения. Нажмите Сохранить.",
                    "Client enters this password — connects without the approval dialog. Press Save.",
                ))
                .size(11.0)
                .color(crate::theme::palette().text_muted),
            );
                ui.add_space(6.0);
            }

            ui.checkbox(
                &mut draft.security.require_confirmation,
                tr(
                    selected_lang,
                    "Подтверждать каждое входящее подключение",
                    "Confirm every incoming connection",
                ),
            );
            ui.add_space(2.0);
            ui.checkbox(
                &mut draft.security.allow_keyboard_mouse,
                tr(
                    selected_lang,
                    "Разрешить управление клавиатурой и мышью",
                    "Allow keyboard and mouse control",
                ),
            );
            ui.add_space(2.0);
            ui.checkbox(
                &mut draft.security.allow_clipboard,
                tr(
                    selected_lang,
                    "Разрешить доступ к буферу обмена",
                    "Allow clipboard access",
                ),
            );

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);

            // ── Whitelist ────────────────────────────────────────────────────────
            ui.label(
                egui::RichText::new(tr(
                    selected_lang,
                    "Белый список (авто-подключение без диалога)",
                    "Whitelist (auto-approve without dialog)",
                ))
                .size(13.0)
                .strong()
                .color(crate::theme::palette().text),
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(tr(
                    selected_lang,
                    "ID из этого списка подключаются без подтверждения",
                    "IDs in this list connect without approval prompt",
                ))
                .size(11.0)
                .color(crate::theme::palette().text_muted),
            );
            ui.add_space(6.0);

            // Existing entries
            let mut remove_idx: Option<usize> = None;
            for (i, wid) in draft.security.whitelist.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(wid)
                            .monospace()
                            .size(13.0)
                            .color(crate::theme::palette().text),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button(egui_phosphor::regular::TRASH)
                            .on_hover_text(tr(selected_lang, "Удалить", "Remove"))
                            .clicked()
                        {
                            remove_idx = Some(i);
                        }
                    });
                });
            }
            if let Some(idx) = remove_idx {
                draft.security.whitelist.remove(idx);
            }

            // Add new entry
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(whitelist_new_id)
                        .hint_text(tr(selected_lang, "Добавить ID…", "Add ID…"))
                        .desired_width(180.0)
                        .font(egui::TextStyle::Monospace),
                );
                let can_add = !whitelist_new_id.trim().is_empty()
                    && !draft
                        .security
                        .whitelist
                        .contains(&whitelist_new_id.trim().to_owned());
                if ui
                    .add_enabled(
                        can_add,
                        egui::Button::new(tr(selected_lang, "Добавить", "Add")),
                    )
                    .clicked()
                {
                    draft
                        .security
                        .whitelist
                        .push(whitelist_new_id.trim().to_owned());
                    whitelist_new_id.clear();
                }
            });
        },
    );
}

// ── Network section: hidden by default, expandable ───────────────────────────
// `custom_server` is a separate buffer that starts empty (never pre-filled with
// the default Everty server values), so the real server address is never shown.

fn network_section(
    ui: &mut egui::Ui,
    selected_lang: UiLang,
    draft: &mut AppConfig,
    custom_server: &mut ServerConfig,
) {
    let default_srv = ServerConfig::default();
    let is_custom = draft.server != default_srv;

    settings_section(ui, tr(selected_lang, "Сеть", "Network"), |ui| {
        if is_custom {
            ui.horizontal(|ui| {
                status_dot(ui, crate::theme::palette().warning);
                ui.label(
                    egui::RichText::new(tr(
                        selected_lang,
                        "Используется собственный сервер",
                        "Using custom server",
                    ))
                    .color(crate::theme::palette().text_weak),
                );
            });
            ui.add_space(6.0);
            // When custom is active, show and edit the actual custom values.
            settings_text_row(
                ui,
                tr(selected_lang, "ID сервер", "ID server"),
                &mut draft.server.id_server,
            );
            settings_text_row(
                ui,
                tr(selected_lang, "Relay сервер", "Relay server"),
                &mut draft.server.relay_server,
            );
            settings_text_row(
                ui,
                tr(selected_lang, "API URL", "API URL"),
                &mut draft.server.api_url,
            );
            settings_text_row(
                ui,
                tr(selected_lang, "Публичный ключ", "Public key"),
                &mut draft.server.public_key,
            );
            ui.add_space(4.0);
            if ui
                .button(tr(
                    selected_lang,
                    "Вернуть сервер по умолчанию",
                    "Reset to default server",
                ))
                .clicked()
            {
                draft.server = ServerConfig::default();
                *custom_server = ServerConfig {
                    id_server: String::new(),
                    relay_server: String::new(),
                    api_url: String::new(),
                    public_key: String::new(),
                };
            }
        } else {
            ui.horizontal(|ui| {
                status_dot(ui, crate::theme::palette().accent);
                ui.label(
                    egui::RichText::new(tr(
                        selected_lang,
                        "Everty Desk сервер (по умолчанию)",
                        "Everty Desk server (default)",
                    ))
                    .color(crate::theme::palette().text_weak),
                );
            });
            ui.add_space(6.0);
            // Show empty input fields. If the user fills ID + key, apply as custom.
            ui.collapsing(
                tr(
                    selected_lang,
                    "Использовать другой сервер",
                    "Use a different server",
                ),
                |ui| {
                    ui.add_space(4.0);
                    settings_text_row(
                        ui,
                        tr(selected_lang, "ID сервер", "ID server"),
                        &mut custom_server.id_server,
                    );
                    settings_text_row(
                        ui,
                        tr(selected_lang, "Relay сервер", "Relay server"),
                        &mut custom_server.relay_server,
                    );
                    settings_text_row(
                        ui,
                        tr(selected_lang, "API URL", "API URL"),
                        &mut custom_server.api_url,
                    );
                    settings_text_row(
                        ui,
                        tr(selected_lang, "Публичный ключ", "Public key"),
                        &mut custom_server.public_key,
                    );
                    ui.add_space(4.0);
                    // Apply custom server when ID server is provided.
                    let can_apply = !custom_server.id_server.trim().is_empty();
                    if ui
                        .add_enabled(
                            can_apply,
                            egui::Button::new(tr(
                                selected_lang,
                                "Применить собственный сервер",
                                "Apply custom server",
                            )),
                        )
                        .on_disabled_hover_text(tr(
                            selected_lang,
                            "Введите адрес ID сервера",
                            "Enter an ID server address",
                        ))
                        .clicked()
                    {
                        // Fill missing fields with the custom ID server as fallback.
                        if custom_server.relay_server.trim().is_empty() {
                            custom_server.relay_server = custom_server.id_server.clone();
                        }
                        draft.server = custom_server.clone();
                    }
                },
            );
        }
    });
}

// ── Video settings body (shared) ─────────────────────────────────────────────

fn video_settings_body(ui: &mut egui::Ui, selected_lang: UiLang, draft: &mut AppConfig) {
    use egui_phosphor::regular as ph;
    // Quick presets row
    ui.horizontal(|ui| {
        ui.label(tr(selected_lang, "Пресет", "Preset"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button(format!(
                    "{}  {}",
                    ph::GAME_CONTROLLER,
                    tr(selected_lang, "Игры", "Game")
                ))
                .clicked()
            {
                draft.display.streaming_mode = crate::settings::StreamingMode::Game;
                draft.display.target_fps = 60;
                draft.display.adaptive_quality = false;
            }
            if ui
                .button(format!(
                    "{}  {}",
                    ph::SCALES,
                    tr(selected_lang, "Баланс", "Balanced")
                ))
                .clicked()
            {
                draft.display.streaming_mode = crate::settings::StreamingMode::Interactive;
                draft.display.target_fps = 30;
                draft.display.adaptive_quality = true;
            }
            if ui
                .button(format!(
                    "{}  {}",
                    ph::LIFEBUOY,
                    tr(selected_lang, "Поддержка", "Support")
                ))
                .clicked()
            {
                draft.display.streaming_mode = crate::settings::StreamingMode::Support;
                draft.display.target_fps = 15;
                draft.display.adaptive_quality = true;
            }
        });
    });
    ui.label(
        egui::RichText::new(tr(
            selected_lang,
            "Пресет меняет режим, FPS и адаптацию — остальное настраивается вручную ниже.",
            "Preset changes mode, FPS and adaptation — other settings remain manual.",
        ))
        .size(11.0)
        .color(crate::theme::palette().text_muted),
    );
    ui.add_space(8.0);

    // Выбор кодека/транспорта убран из UI: он согласуется автоматически между
    // хостом и клиентом по реальным аппаратным возможностям обеих сторон
    // (EVRT UDP при прямом соединении, иначе H264/H265 по TCP relay). Ручной
    // выбор только путал пользователя. Поле draft.display.codec остаётся Auto.

    // Row: Encoder
    ui.horizontal(|ui| {
        ui.label(tr(selected_lang, "Энкодер", "Encoder"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for encoder in encoder_preference_order() {
                ui.selectable_value(&mut draft.display.encoder, encoder, encoder.label());
            }
        });
    });
    ui.label(
        egui::RichText::new(crate::video::selected_encoder_label(draft.display.encoder))
            .size(11.0)
            .color(crate::theme::palette().text_muted),
    );
    ui.add_space(4.0);

    // Row: Mode
    ui.horizontal(|ui| {
        ui.label(tr(selected_lang, "Режим", "Mode"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for mode in [
                StreamingMode::Game,
                StreamingMode::Interactive,
                StreamingMode::Support,
            ] {
                ui.selectable_value(&mut draft.display.streaming_mode, mode, mode.label());
            }
        });
    });
    ui.add_space(4.0);

    // Row: Target FPS + Min FPS on same line
    ui.horizontal(|ui| {
        ui.label(tr(selected_lang, "FPS", "FPS"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for fps in [60u32, 30, 20, 15] {
                ui.selectable_value(&mut draft.display.target_fps, fps, fps.to_string());
            }
        });
    });
    ui.add_space(2.0);
    ui.checkbox(
        &mut draft.display.adaptive_quality,
        tr(
            selected_lang,
            "Авто-снижать FPS при перегрузке декодера",
            "Auto-lower FPS when decoder is overloaded",
        ),
    );
    if draft.display.adaptive_quality {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(tr(selected_lang, "Мин. FPS", "Min FPS"))
                    .size(13.0)
                    .color(crate::theme::palette().text_weak),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for fps in [30u32, 20, 15, 10, 5] {
                    ui.selectable_value(&mut draft.display.min_fps, fps, fps.to_string());
                }
            });
        });
    }
    ui.add_space(4.0);

    // Row: FSR
    ui.horizontal(|ui| {
        ui.label("FSR");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for quality in fsr_quality_order() {
                ui.selectable_value(&mut draft.display.fsr_quality, quality, quality.label());
            }
        });
    });

    ui.add_space(4.0);
    ui.checkbox(
        &mut draft.display.nearest_neighbour,
        tr(
            selected_lang,
            "Пиксель-перфект (ближайший сосед, без размытия)",
            "Pixel-perfect (nearest-neighbour, no blur)",
        ),
    )
    .on_hover_text(tr(
        selected_lang,
        "Отключает интерполяцию при уменьшении масштаба. Чёткость без биленьшного размытия.",
        "Disables interpolation when downscaling. Sharp pixels, no bilinear blur.",
    ));

    // IDR interval (EVRTCK keyframe frequency)
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(tr(selected_lang, "IDR-интервал (EVRTCK)", "IDR interval (EVRTCK)"))
            .on_hover_text(tr(
                selected_lang,
                "Как часто отправлять полный ключевой кадр. Реже = меньше трафика, дольше восстановление после потери. 20s оптимально для LAN.",
                "How often to send a full keyframe. Less frequent = lower bandwidth, slower recovery from packet loss. 20s is optimal for LAN.",
            ));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for secs in [60u32, 30, 20, 15, 10] {
                ui.selectable_value(
                    &mut draft.display.idr_interval_secs,
                    secs,
                    format!("{secs}s"),
                );
            }
        });
    });
    ui.label(
        egui::RichText::new(tr(
            selected_lang,
            "Текущий дефолт 20s: при 3.4 Мбит/с IDR ~850КБ ≈ 2с передачи = 10% overhead",
            "Default 20s: at 3.4 Mbps IDR ~850 KB ≈ 2s transmission = 10% overhead",
        ))
        .size(11.0)
        .color(crate::theme::palette().text_muted),
    );
}

fn llm_settings_section(ui: &mut egui::Ui, selected_lang: UiLang, draft: &mut AppConfig) {
    settings_section(
        ui,
        tr(selected_lang, "AI терминал", "AI terminal"),
        |ui| {
            ui.checkbox(
                &mut draft.llm.enabled,
                tr(
                    selected_lang,
                    "Включить LLM-помощник в терминале",
                    "Enable LLM assistant in terminal",
                ),
            );
            ui.add_space(2.0);
            ui.checkbox(
                &mut draft.llm.auto_suggest,
                tr(
                    selected_lang,
                    "Автоматически анализировать вывод после команды",
                    "Automatically analyze output after commands",
                ),
            );

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(tr(selected_lang, "Провайдер", "Provider"));
                for provider in [
                    LlmProvider::Ollama,
                    LlmProvider::OpenAi,
                    LlmProvider::YandexGpt,
                ] {
                    ui.selectable_value(&mut draft.llm.provider, provider, provider.label());
                }
            });

            ui.add_space(6.0);
            match draft.llm.provider {
                LlmProvider::Ollama => {
                    settings_text_row(
                        ui,
                        tr(selected_lang, "Ollama URL", "Ollama URL"),
                        &mut draft.llm.ollama_base_url,
                    );
                    settings_text_row(
                        ui,
                        tr(selected_lang, "Модель", "Model"),
                        &mut draft.llm.ollama_model,
                    );
                }
                LlmProvider::OpenAi => {
                    settings_text_row(
                        ui,
                        tr(selected_lang, "OpenAI endpoint", "OpenAI endpoint"),
                        &mut draft.llm.openai_base_url,
                    );
                    settings_text_row(
                        ui,
                        tr(selected_lang, "Модель", "Model"),
                        &mut draft.llm.openai_model,
                    );
                    settings_secret_row(
                        ui,
                        tr(selected_lang, "API key", "API key"),
                        &mut draft.llm.openai_api_key,
                    );
                }
                LlmProvider::YandexGpt => {
                    settings_text_row(
                        ui,
                        tr(selected_lang, "Yandex endpoint", "Yandex endpoint"),
                        &mut draft.llm.yandex_base_url,
                    );
                    settings_secret_row(
                        ui,
                        tr(selected_lang, "API key / IAM", "API key / IAM"),
                        &mut draft.llm.yandex_api_key,
                    );
                    settings_text_row(
                        ui,
                        tr(selected_lang, "Folder ID", "Folder ID"),
                        &mut draft.llm.yandex_folder_id,
                    );
                    settings_text_row(
                        ui,
                        tr(selected_lang, "Model URI", "Model URI"),
                        &mut draft.llm.yandex_model_uri,
                    );
                    ui.label(
                        egui::RichText::new(tr(
                            selected_lang,
                            "Для IAM token укажите значение с префиксом Bearer, для API key можно без префикса.",
                            "For IAM token use the Bearer prefix; API key may be entered without a prefix.",
                        ))
                        .size(11.0)
                        .color(crate::theme::palette().text_muted),
                    );
                }
            }

            ui.add_space(6.0);
            settings_text_row(
                ui,
                tr(selected_lang, "Системный prompt", "System prompt"),
                &mut draft.llm.system_prompt,
            );
            ui.horizontal_wrapped(|ui| {
                ui.label(tr(selected_lang, "Лимит ответа", "Token limit"));
                ui.add(
                    egui::DragValue::new(&mut draft.llm.max_tokens)
                        .range(128..=4096)
                        .speed(32),
                );
                ui.add_space(16.0);
                ui.label("Temperature");
                ui.add(
                    egui::Slider::new(&mut draft.llm.temperature, 0.0..=2.0)
                        .show_value(true)
                        .clamping(egui::SliderClamping::Always),
                );
            });
        },
    );
}

fn settings_secret_row(ui: &mut egui::Ui, label: &str, value: &mut String) {
    let wide = ui.available_width() >= 520.0;
    if wide {
        ui.horizontal(|ui| {
            ui.set_min_height(36.0);
            ui.set_width(ui.available_width());
            ui.add_sized(
                egui::vec2(150.0, 24.0),
                egui::Label::new(
                    egui::RichText::new(label)
                        .size(13.0)
                        .color(crate::theme::palette().text_weak),
                )
                .truncate(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let width = ui.available_width().min(360.0);
                ui.add_sized(
                    egui::vec2(width, 34.0),
                    egui::TextEdit::singleline(value)
                        .password(true)
                        .font(egui::TextStyle::Button),
                );
            });
        });
    } else {
        ui.label(
            egui::RichText::new(label)
                .size(13.0)
                .color(crate::theme::palette().text_weak),
        );
        ui.add_sized(
            egui::vec2(ui.available_width(), 34.0),
            egui::TextEdit::singleline(value)
                .password(true)
                .font(egui::TextStyle::Button),
        );
    }
    ui.add_space(4.0);
}

/// Used by the per-connection codec picker in the main connect panel
/// (`EvertyDeskApp::remote_connect_ui` in main.rs) for the EVRT1 pipeline —
/// EVRT2 negotiates its own codec independently, this list doesn't apply there.
pub(crate) fn codec_preference_order() -> [CodecPreference; 6] {
    [
        CodecPreference::Evrtck,
        CodecPreference::Auto,
        CodecPreference::H264,
        CodecPreference::H265,
        CodecPreference::Vp9,
        CodecPreference::Av1,
    ]
}

pub(crate) fn codec_status_text(codec: CodecPreference) -> String {
    match codec {
        CodecPreference::Evrtck => {
            "EVRTCK: UDP-транспорт, lossless XOR-diff. Лучший выбор для LAN и прямых подключений."
                .to_owned()
        }
        CodecPreference::Auto => {
            "Авто: EVRT если доступен, иначе TCP/H264. Кодек выбирается автоматически.".to_owned()
        }
        CodecPreference::H264 => {
            "H264: TCP relay, без EVRT. Работает везде, хорошо для WAN/интернет.".to_owned()
        }
        CodecPreference::H265 if !crate::video::h265_available() => {
            "H265: TCP relay — decoder недоступен в этой сборке, будет fallback на H264.".to_owned()
        }
        CodecPreference::H265 => {
            "H265: TCP relay, без EVRT. Лучше H264 по качеству при том же битрейте.".to_owned()
        }
        CodecPreference::Vp9 => {
            "VP9: TCP relay, без EVRT. Открытый кодек, хорошее качество на низком битрейте."
                .to_owned()
        }
        CodecPreference::Av1 if !crate::video::av1_available() => {
            "AV1: TCP relay — decoder недоступен в этой сборке, будет fallback на H264.".to_owned()
        }
        CodecPreference::Av1 => {
            "AV1: TCP relay, без EVRT. Лучшее сжатие, но требует мощного CPU на хосте.".to_owned()
        }
    }
}

fn encoder_preference_order() -> [EncoderPreference; 3] {
    [
        EncoderPreference::Software,
        EncoderPreference::Nvenc,
        EncoderPreference::Auto,
    ]
}

fn fsr_quality_order() -> [FsrQualitySetting; 6] {
    [
        FsrQualitySetting::Performance,
        FsrQualitySetting::Balanced,
        FsrQualitySetting::Quality,
        FsrQualitySetting::UltraQuality,
        FsrQualitySetting::Native,
        FsrQualitySetting::Off,
    ]
}

fn make_fsr_adapter(display: &settings_mod::DisplayConfig) -> Option<crate::fsr::FsrAdapter> {
    display.fsr_quality.to_fsr_quality().map(|quality| {
        crate::fsr::FsrAdapter::new(crate::fsr::FsrConfig {
            quality,
            sharpness: display.fsr_sharpness,
        })
    })
}

fn default_config_from(config: &AppConfig) -> AppConfig {
    AppConfig {
        server: settings_mod::ServerConfig::default(),
        security: settings_mod::SecurityConfig::default(),
        network_debug: settings_mod::NetworkDebugConfig::default(),
        display: settings_mod::DisplayConfig::default(),
        llm: settings_mod::LlmConfig::default(),
        hotfix: config.hotfix.clone(),
        local_id: config.local_id.clone(),
        local_password: config.local_password.clone(),
        permanent_password: config.permanent_password.clone(),
        ui: config.ui.clone(),
        udp_bind_port: 0,
        evrt_udp_port: 0,
        host_pk: Vec::new(),
        host_sign_pk: config.host_sign_pk.clone(),
        host_sign_sk: config.host_sign_sk.clone(),
    }
}

fn hotfix_settings_section(ui: &mut egui::Ui, selected_lang: UiLang, draft: &mut AppConfig) {
    settings_section(
        ui,
        tr(
            selected_lang,
            "AI Hotfix (авто-исправления)",
            "AI Hotfix (auto-fixes)",
        ),
        |ui| {
            ui.label(
                egui::RichText::new(tr(
                    selected_lang,
                    "Автоматически отправляет краш-отчёты на сервер EvertyDesk и применяет\n\
                     рекомендованные AI-исправления настроек (FPS, энкодер и т.д.).",
                    "Automatically reports crashes to the EvertyDesk server and applies\n\
                     AI-recommended setting fixes (FPS, encoder, etc.).",
                ))
                .size(12.0)
                .color(crate::theme::palette().text_weak),
            );

            ui.add_space(6.0);

            ui.checkbox(
                &mut draft.hotfix.enabled,
                tr(
                    selected_lang,
                    "Включить AI Hotfix Pipeline",
                    "Enable AI Hotfix Pipeline",
                ),
            );

            if draft.hotfix.enabled {
                ui.add_space(4.0);

                settings_secret_row(
                    ui,
                    tr(selected_lang, "API ключ", "API key"),
                    &mut draft.hotfix.api_key,
                );

                settings_text_row(
                    ui,
                    tr(
                        selected_lang,
                        "Публичный ключ подписи (Base64)",
                        "Signing public key (Base64)",
                    ),
                    &mut draft.hotfix.signing_public_key,
                );

                ui.add_space(4.0);

                // Проверка ключа — показываем статус
                let key_status: (String, egui::Color32) =
                    if draft.hotfix.signing_public_key.trim().is_empty() {
                        (
                            tr(
                                selected_lang,
                                "Ключ подписи не задан — подпись не проверяется",
                                "Signing key not set — signatures won't be verified",
                            )
                            .to_owned(),
                            crate::theme::palette().warning,
                        )
                    } else {
                        match base64::engine::general_purpose::STANDARD
                            .decode(draft.hotfix.signing_public_key.trim())
                        {
                            Ok(b) if b.len() == 32 => (
                                tr(
                                    selected_lang,
                                    "Ключ корректен (32 байта Ed25519)",
                                    "Key valid (32-byte Ed25519)",
                                )
                                .to_owned(),
                                crate::theme::palette().success,
                            ),
                            Ok(b) => (
                                format!(
                                    "{} ({} {})",
                                    tr(selected_lang, "Неверная длина:", "Wrong length:"),
                                    b.len(),
                                    tr(selected_lang, "байт, ожидается 32", "bytes, expected 32")
                                ),
                                crate::theme::palette().danger,
                            ),
                            Err(_) => (
                                tr(
                                    selected_lang,
                                    "Ошибка декодирования Base64",
                                    "Base64 decode error",
                                )
                                .to_owned(),
                                crate::theme::palette().danger,
                            ),
                        }
                    };

                ui.horizontal(|ui| {
                    status_dot(ui, key_status.1);
                    ui.label(
                        egui::RichText::new(key_status.0)
                            .size(11.0)
                            .color(key_status.1),
                    );
                });

                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(tr(
                            selected_lang,
                            "Минимальный интервал между отчётами (сек):",
                            "Min interval between reports (sec):",
                        ))
                        .size(13.0)
                        .color(crate::theme::palette().text_weak),
                    );
                    let mut rate = draft.hotfix.rate_limit_secs as i32;
                    if ui
                        .add(egui::DragValue::new(&mut rate).range(30..=3600).speed(10.0))
                        .changed()
                    {
                        draft.hotfix.rate_limit_secs = rate as u64;
                    }
                });

                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(tr(
                        selected_lang,
                        "API ключ и публичный ключ подписи берутся в панели администратора\n\
                         EvertyDesk → AI Hotfix → Обзор → Интеграция клиента.",
                        "API key and signing public key are found in the EvertyDesk admin panel\n\
                         under AI Hotfix → Overview → Client integration.",
                    ))
                    .size(11.0)
                    .color(crate::theme::palette().text_weak),
                );

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    let btn = ui.button(tr(
                        selected_lang,
                        "Отправить тестовый краш-отчёт",
                        "Send test crash report",
                    ));
                    ui.label(
                        egui::RichText::new(tr(
                            selected_lang,
                            "← проверь в ЛК что появилась запись",
                            "← check admin panel for a new entry",
                        ))
                        .size(11.0)
                        .color(crate::theme::palette().text_weak),
                    );
                    if btn.clicked() {
                        crate::hotfix::submit_crash_sync(
                            "test_crash_manual".to_owned(),
                            "settings_ui".to_owned(),
                            "TEST".to_owned(),
                            "Тестовый краш-отчёт из настроек EvertyDesk Lite".to_owned(),
                            "no stack trace (manual test)".to_owned(),
                            &draft.hotfix,
                            draft,
                        );
                    }
                });
            }
        },
    );
}

/// Small filled circle as a status indicator — avoids Unicode symbols that
/// may not be present in the bundled font and render as tofu squares.
fn status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let size = egui::vec2(10.0, 10.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().circle_filled(rect.center(), 4.0, color);
    }
}
