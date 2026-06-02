use eframe::egui;

use crate::settings::{
    self as settings_mod, AppConfig, CodecPreference, EncoderPreference, LlmProvider,
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
                                    for codec in codec_preference_order() {
                                        ui.selectable_value(
                                            &mut draft.display.codec,
                                            codec,
                                            codec.label(),
                                        );
                                    }
                                },
                            );
                        });
                        ui.label(
                            egui::RichText::new(codec_status_text(draft.display.codec))
                                .size(12.0)
                                .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                        );
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(tr(selected_lang, "Энкодер", "Encoder"));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    for encoder in encoder_preference_order() {
                                        ui.selectable_value(
                                            &mut draft.display.encoder,
                                            encoder,
                                            encoder.label(),
                                        );
                                    }
                                },
                            );
                        });
                        ui.label(
                            egui::RichText::new(crate::video::selected_encoder_label(
                                draft.display.encoder,
                            ))
                            .size(12.0)
                            .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                        );
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
                        ui.add_space(6.0);
                        ui.checkbox(
                            &mut draft.display.adaptive_quality,
                            tr(
                                selected_lang,
                                "Автоматически снижать FPS при перегрузке декодера",
                                "Automatically lower FPS when the decoder is overloaded",
                            ),
                        );
                        ui.horizontal(|ui| {
                            ui.label(tr(selected_lang, "Минимальный FPS", "Minimum FPS"));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    for fps in [30u32, 20, 15, 10, 5] {
                                        ui.selectable_value(
                                            &mut draft.display.min_fps,
                                            fps,
                                            fps.to_string(),
                                        );
                                    }
                                },
                            );
                        });
                    });

                    ui.add_space(8.0);

                    llm_settings_section(ui, selected_lang, draft);

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
                            || new_cfg.display.codec != self.config.display.codec
                            || new_cfg.display.encoder != self.config.display.encoder;
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
                        "Язык, серверы, безопасность, видео и AI терминал",
                        "Language, servers, security, video and AI terminal",
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
            current_config.display.encoder,
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
                        for codec in codec_preference_order() {
                            ui.selectable_value(&mut draft.display.codec, codec, codec.label());
                        }
                    });
                });
                ui.label(
                    egui::RichText::new(codec_status_text(draft.display.codec))
                        .size(12.0)
                        .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(tr(selected_lang, "Энкодер", "Encoder"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for encoder in encoder_preference_order() {
                            ui.selectable_value(
                                &mut draft.display.encoder,
                                encoder,
                                encoder.label(),
                            );
                        }
                    });
                });
                ui.label(
                    egui::RichText::new(crate::video::selected_encoder_label(
                        draft.display.encoder,
                    ))
                    .size(12.0)
                    .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                );
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
                ui.add_space(6.0);
                ui.checkbox(
                    &mut draft.display.adaptive_quality,
                    tr(
                        selected_lang,
                        "Автоматически снижать FPS при перегрузке декодера",
                        "Automatically lower FPS when the decoder is overloaded",
                    ),
                );
                ui.horizontal(|ui| {
                    ui.label(tr(selected_lang, "Минимальный FPS", "Minimum FPS"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for fps in [30u32, 20, 15, 10, 5] {
                            ui.selectable_value(&mut draft.display.min_fps, fps, fps.to_string());
                        }
                    });
                });
            });

            ui.add_space(8.0);

            llm_settings_section(ui, selected_lang, draft);

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
                    || new_cfg.display.codec != host_reconfigure_source.4
                    || new_cfg.display.encoder != host_reconfigure_source.5;
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
            ui.checkbox(
                &mut draft.llm.auto_suggest,
                tr(
                    selected_lang,
                    "Автоматически анализировать вывод после команды",
                    "Automatically analyze output after commands",
                ),
            );

            ui.add_space(8.0);
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

            ui.add_space(8.0);
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
                        .size(12.0)
                        .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                    );
                }
            }

            ui.add_space(8.0);
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
                        .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
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
                .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
        );
        ui.add_sized(
            egui::vec2(ui.available_width(), 34.0),
            egui::TextEdit::singleline(value)
                .password(true)
                .font(egui::TextStyle::Button),
        );
    }
    ui.add_space(6.0);
}

fn codec_preference_order() -> [CodecPreference; 5] {
    [
        CodecPreference::Av1,
        CodecPreference::H265,
        CodecPreference::Vp9,
        CodecPreference::H264,
        CodecPreference::Auto,
    ]
}

fn codec_status_text(codec: CodecPreference) -> String {
    match codec {
        CodecPreference::Auto => {
            "Auto: AV1/H265 используются только когда в сборке есть decoder backend".to_owned()
        }
        CodecPreference::Av1 if !crate::video::av1_available() => {
            "AV1 decoder пока не подключен: будет fallback на H264/VP9".to_owned()
        }
        CodecPreference::H265 if !crate::video::h265_available() => {
            "H265 decoder пока не подключен: будет fallback на H264/VP9".to_owned()
        }
        _ => "Кодек будет запрошен, если локальная сборка умеет его декодировать".to_owned(),
    }
}

fn encoder_preference_order() -> [EncoderPreference; 3] {
    [
        EncoderPreference::Software,
        EncoderPreference::Nvenc,
        EncoderPreference::Auto,
    ]
}

fn default_config_from(config: &AppConfig) -> AppConfig {
    AppConfig {
        server: settings_mod::ServerConfig::default(),
        security: settings_mod::SecurityConfig::default(),
        display: settings_mod::DisplayConfig::default(),
        llm: settings_mod::LlmConfig::default(),
        local_id: config.local_id.clone(),
        local_password: config.local_password.clone(),
        ui: config.ui.clone(),
        udp_bind_port: 0,
        host_pk: Vec::new(),
        host_sign_pk: config.host_sign_pk.clone(),
        host_sign_sk: config.host_sign_sk.clone(),
    }
}
