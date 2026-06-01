use eframe::egui;

use crate::settings::{self as settings_mod, AppConfig, CodecPreference};
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
                    });

                    ui.add_space(8.0);

                    settings_section(ui, tr(selected_lang, "Сеть", "Network"), |ui| {
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
                    });

                    ui.add_space(8.0);

                    settings_section(
                        ui,
                        tr(selected_lang, "Безопасность", "Security"),
                        |ui| {
                            ui.checkbox(
                                &mut draft.security.require_confirmation,
                                tr(
                                    selected_lang,
                                    "Подтверждать каждое входящее подключение",
                                    "Confirm every incoming connection",
                                ),
                            );
                            ui.checkbox(
                                &mut draft.security.allow_keyboard_mouse,
                                tr(
                                    selected_lang,
                                    "Разрешить управление клавиатурой и мышью",
                                    "Allow keyboard and mouse control",
                                ),
                            );
                            ui.checkbox(
                                &mut draft.security.allow_clipboard,
                                tr(
                                    selected_lang,
                                    "Разрешить доступ к буферу обмена",
                                    "Allow clipboard access",
                                ),
                            );
                        },
                    );

                    ui.add_space(8.0);

                    settings_section(ui, tr(selected_lang, "Видео", "Video"), |ui| {
                        ui.horizontal(|ui| {
                            ui.label(tr(selected_lang, "Кодек", "Codec"));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    for codec in [
                                        CodecPreference::Vp9,
                                        CodecPreference::H264,
                                        CodecPreference::Auto,
                                    ] {
                                        ui.selectable_value(
                                            &mut draft.display.codec,
                                            codec,
                                            codec.label(),
                                        );
                                    }
                                },
                            );
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(tr(selected_lang, "Целевой FPS", "Target FPS"));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    for fps in [60u32, 30, 20, 15] {
                                        ui.selectable_value(
                                            &mut draft.display.target_fps,
                                            fps,
                                            fps.to_string(),
                                        );
                                    }
                                },
                            );
                        });
                    });

                    ui.add_space(8.0);

                    settings_section(
                        ui,
                        tr(selected_lang, "О программе", "About"),
                        |ui| {
                            ui.label(format!("{APP_NAME} v{APP_VERSION}"));
                            ui.label(tr(
                                selected_lang,
                                "RustDesk-совместимый клиент удаленного доступа.",
                                "RustDesk-compatible remote access client.",
                            ));
                            ui.label(format!(
                                "{}: {}",
                                tr(selected_lang, "Конфиг", "Config"),
                                settings_mod::config_path().display()
                            ));
                        },
                    );
                });

                ui.separator();
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
                            || new_cfg.display.codec != self.config.display.codec;
                        let next_video_fps = new_cfg.display.target_fps.clamp(5, 60) as i32;
                        if host_reconfigure_needed {
                            if let Some(svc) = &self.host_service {
                                svc.reconfigure(new_cfg.clone());
                            }
                        }
                        self.config = new_cfg;
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
                    if ui.button(tr(selected_lang, "Закрыть", "Close")).clicked() {
                        self.settings_draft = None;
                        self.show_settings = false;
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
        }

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(self.text("Настройки", "Settings"))
                        .size(28.0)
                        .strong()
                        .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                );
                ui.add_space(3.0);
                ui.label(
                    egui::RichText::new(self.text(
                        "Язык, серверы, безопасность и параметры видео",
                        "Language, servers, security and video options",
                    ))
                    .size(13.0)
                    .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                );
            });
        });
        ui.add_space(16.0);

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
        );

        egui::ScrollArea::vertical().show(ui, |ui| {
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
                ui.add_space(8.0);
                ui.checkbox(
                    &mut draft.ui.show_connection_details,
                    tr(
                        selected_lang,
                        "Показывать детали подключения на главной",
                        "Show connection details on main page",
                    ),
                );
            });

            ui.add_space(8.0);
            settings_section(ui, tr(selected_lang, "Сеть", "Network"), |ui| {
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
            });

            ui.add_space(8.0);
            settings_section(
                ui,
                tr(selected_lang, "Безопасность", "Security"),
                |ui| {
                    ui.checkbox(
                        &mut draft.security.require_confirmation,
                        tr(
                            selected_lang,
                            "Подтверждать каждое входящее подключение",
                            "Confirm every incoming connection",
                        ),
                    );
                    ui.checkbox(
                        &mut draft.security.allow_keyboard_mouse,
                        tr(
                            selected_lang,
                            "Разрешить управление клавиатурой и мышью",
                            "Allow keyboard and mouse control",
                        ),
                    );
                    ui.checkbox(
                        &mut draft.security.allow_clipboard,
                        tr(
                            selected_lang,
                            "Разрешить доступ к буферу обмена",
                            "Allow clipboard access",
                        ),
                    );
                },
            );

            ui.add_space(8.0);
            settings_section(ui, tr(selected_lang, "Видео", "Video"), |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr(selected_lang, "Кодек", "Codec"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for codec in [
                            CodecPreference::Vp9,
                            CodecPreference::H264,
                            CodecPreference::Auto,
                        ] {
                            ui.selectable_value(&mut draft.display.codec, codec, codec.label());
                        }
                    });
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(tr(selected_lang, "Целевой FPS", "Target FPS"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for fps in [60u32, 30, 20, 15] {
                            ui.selectable_value(
                                &mut draft.display.target_fps,
                                fps,
                                fps.to_string(),
                            );
                        }
                    });
                });
            });

            ui.add_space(8.0);
            settings_section(ui, tr(selected_lang, "Служба", "Service"), |ui| {
                ui.label(tr(
                    selected_lang,
                    "Фоновый режим использует этот же исполняемый файл с аргументом --host.",
                    "Background mode uses this executable with the --host argument.",
                ));
                ui.add_space(8.0);
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
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(status)
                            .size(12.0)
                            .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                    );
                }
            });

            ui.add_space(8.0);
            settings_section(
                ui,
                tr(selected_lang, "О программе", "About"),
                |ui| {
                    ui.label(format!("{APP_NAME} v{APP_VERSION}"));
                    ui.label(tr(
                        selected_lang,
                        "RustDesk-совместимый клиент удаленного доступа.",
                        "RustDesk-compatible remote access client.",
                    ));
                    ui.label(format!(
                        "{}: {}",
                        tr(selected_lang, "Конфиг", "Config"),
                        settings_mod::config_path().display()
                    ));
                },
            );
        });

        self.ui_lang = selected_lang;
        ui.add_space(12.0);
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
                    || new_cfg.display.codec != host_reconfigure_source.4;
                let next_video_fps = new_cfg.display.target_fps.clamp(5, 60) as i32;
                if host_reconfigure_needed {
                    if let Some(svc) = &self.host_service {
                        svc.reconfigure(new_cfg.clone());
                    }
                }
                self.config = new_cfg.clone();
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

fn default_config_from(config: &AppConfig) -> AppConfig {
    AppConfig {
        server: settings_mod::ServerConfig::default(),
        security: settings_mod::SecurityConfig::default(),
        display: settings_mod::DisplayConfig::default(),
        local_id: config.local_id.clone(),
        local_password: config.local_password.clone(),
        ui: config.ui.clone(),
        udp_bind_port: 0,
        host_pk: Vec::new(),
        host_sign_pk: config.host_sign_pk.clone(),
        host_sign_sk: config.host_sign_sk.clone(),
    }
}
