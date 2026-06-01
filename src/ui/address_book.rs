use eframe::egui;

use crate::settings::ContactEntry;
use crate::ui::widgets::status_pill;
use crate::{
    address_book, format_peer_id, normalize_remote_id, tr, AppMode, EvertyDeskApp, UiLang,
};

impl EvertyDeskApp {
    pub(crate) fn contacts_ui(&mut self, ui: &mut egui::Ui) {
        self.contacts_ui_compact(ui);
    }

    fn contacts_ui_compact(&mut self, ui: &mut egui::Ui) {
        let lang = self.ui_lang;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(self.text("Контакты", "Contacts"))
                        .size(26.0)
                        .strong()
                        .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "{}: {}",
                        tr(lang, "Всего", "Total"),
                        self.config.ui.contacts.len()
                    ))
                    .size(12.5)
                    .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                status_pill(
                    ui,
                    if self.config.ui.address_book_signed_in {
                        tr(lang, "В сети", "Online")
                    } else {
                        tr(lang, "Вход нужен", "Sign in")
                    },
                    if self.config.ui.address_book_signed_in {
                        egui::Color32::from_rgb(0x12, 0xC9, 0x72)
                    } else {
                        egui::Color32::from_rgb(0xA8, 0xB0, 0xBE)
                    },
                );
            });
        });
        ui.add_space(12.0);

        compact_panel_frame().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let action_width = if ui.available_width() >= 760.0 {
                    390.0
                } else {
                    0.0
                };
                let search_width = (ui.available_width() - action_width).max(220.0).min(460.0);
                ui.add_sized(
                    egui::vec2(search_width, 34.0),
                    egui::TextEdit::singleline(&mut self.contact_search).hint_text(tr(
                        lang,
                        "Поиск: имя, ID, заметка",
                        "Search: name, ID, note",
                    )),
                );
                if !self.contact_search.is_empty()
                    && ui
                        .add(
                            egui::Button::new(tr(lang, "Очистить", "Clear"))
                                .min_size(egui::vec2(82.0, 34.0)),
                        )
                        .clicked()
                {
                    self.contact_search.clear();
                }
                if ui
                    .add_enabled(
                        self.config.ui.address_book_signed_in,
                        egui::Button::new(tr(lang, "+ Контакт", "+ Contact"))
                            .min_size(egui::vec2(104.0, 34.0)),
                    )
                    .on_hover_text(tr(
                        lang,
                        "Сначала войдите в адресную книгу",
                        "Sign in to the address book first",
                    ))
                    .clicked()
                {
                    self.show_new_contact_dialog = true;
                }
                if ui
                    .add(
                        egui::Button::new(if self.config.ui.address_book_signed_in {
                            tr(lang, "Аккаунт", "Account")
                        } else {
                            tr(lang, "Войти", "Sign in")
                        })
                        .min_size(egui::vec2(96.0, 34.0)),
                    )
                    .clicked()
                {
                    self.show_address_book_auth = true;
                }
                if ui
                    .add_enabled(
                        self.config.ui.address_book_signed_in,
                        egui::Button::new(tr(lang, "Обновить", "Refresh"))
                            .min_size(egui::vec2(96.0, 34.0)),
                    )
                    .clicked()
                {
                    match self.sync_address_book() {
                        Ok(()) => {
                            self.address_book_status = Some(
                                tr(lang, "Контакты обновлены", "Contacts refreshed").to_owned(),
                            );
                            self.config.save();
                        }
                        Err(err) => self.address_book_status = Some(err),
                    }
                }
            });
            if let Some(status) = &self.address_book_status {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(status)
                        .size(12.0)
                        .color(egui::Color32::from_rgb(0x57, 0x60, 0x70)),
                );
            }
        });

        ui.add_space(12.0);
        let query = self.contact_search.trim().to_lowercase();
        let visible_indices: Vec<usize> = self
            .config
            .ui
            .contacts
            .iter()
            .enumerate()
            .filter_map(|(idx, contact)| {
                let haystack = format!(
                    "{} {} {} {}",
                    contact.name, contact.remote_id, contact.note, contact.os
                )
                .to_lowercase();
                if query.is_empty() || haystack.contains(&query) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect();

        if self.config.ui.address_book_signed_in && self.config.ui.contacts.is_empty() {
            compact_panel_frame().show(ui, |ui| {
                ui.label(
                    egui::RichText::new(tr(
                        lang,
                        "В адресной книге пока нет контактов. Добавьте первый через кнопку сверху.",
                        "The address book is empty. Add the first contact from the button above.",
                    ))
                    .size(13.0)
                    .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                );
            });
        } else if visible_indices.is_empty() {
            compact_panel_frame().show(ui, |ui| {
                ui.label(
                    egui::RichText::new(tr(lang, "Ничего не найдено.", "Nothing found."))
                        .size(13.0)
                        .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                );
            });
        } else {
            let mut connect_to: Option<String> = None;
            let mut remove_idx: Option<usize> = None;
            let mut open_details: Option<usize> = None;
            let scroll_height = ui.available_height().max(180.0);
            egui::ScrollArea::vertical()
                .max_height(scroll_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let available = ui.available_width().max(1.0);
                    let columns = contact_grid_columns(available);
                    let gap = 10.0;
                    let tile_width = contact_tile_width(available, columns, gap);
                    let tile_height = 138.0;

                    egui::Grid::new("contacts_tile_grid")
                        .num_columns(columns)
                        .spacing(egui::vec2(gap, gap))
                        .min_col_width(tile_width)
                        .show(ui, |ui| {
                            for (position, &idx) in visible_indices.iter().enumerate() {
                                let contact = self.config.ui.contacts[idx].clone();
                                ui.allocate_ui_with_layout(
                                    egui::vec2(tile_width, tile_height),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        draw_contact_tile(
                                            ui,
                                            &contact,
                                            tile_width,
                                            tile_height,
                                            lang,
                                            || open_details = Some(idx),
                                            || connect_to = Some(contact.remote_id.clone()),
                                            || remove_idx = Some(idx),
                                        );
                                    },
                                );
                                if (position + 1) % columns == 0 {
                                    ui.end_row();
                                }
                            }

                            let remainder = visible_indices.len() % columns;
                            if remainder != 0 {
                                for _ in remainder..columns {
                                    ui.allocate_exact_size(
                                        egui::vec2(tile_width, tile_height),
                                        egui::Sense::hover(),
                                    );
                                }
                                ui.end_row();
                            }
                        });
                });

            if let Some(idx) = open_details {
                self.selected_contact_idx = Some(idx);
            }
            if let Some(idx) = remove_idx {
                if idx < self.config.ui.contacts.len() {
                    let contact = self.config.ui.contacts[idx].clone();
                    match self.delete_address_book_contact(&contact.remote_id) {
                        Ok(()) => {
                            self.config.ui.contacts.remove(idx);
                            self.config.save();
                            self.address_book_status =
                                Some(tr(lang, "Контакт удален", "Contact removed").to_owned());
                            if self.selected_contact_idx == Some(idx) {
                                self.selected_contact_idx = None;
                                self.contact_details_draft = None;
                            }
                        }
                        Err(err) => self.address_book_status = Some(err),
                    }
                }
            }
            if let Some(id) = connect_to {
                self.remote_id = id;
                self.mode = AppMode::Connect;
            }
        }

        let ctx = ui.ctx().clone();
        self.address_book_auth_window(&ctx);
        self.new_contact_window(&ctx);
        self.contact_details_window(&ctx);
    }

    fn address_book_auth_window(&mut self, ctx: &egui::Context) {
        if !self.show_address_book_auth {
            return;
        }
        let lang = self.ui_lang;
        let mut open = self.show_address_book_auth;
        let mut close_window = false;
        egui::Window::new(tr(lang, "Адресная книга", "Address book"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.set_min_width(340.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(if self.config.ui.address_book_signed_in {
                            tr(lang, "Аккаунт подключен", "Account connected")
                        } else {
                            tr(lang, "Войдите в аккаунт", "Sign in")
                        })
                        .size(17.0)
                        .strong()
                        .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        status_dot(
                            ui,
                            if self.config.ui.address_book_signed_in {
                                tr(lang, "online", "online")
                            } else {
                                tr(lang, "offline", "offline")
                            },
                            if self.config.ui.address_book_signed_in {
                                egui::Color32::from_rgb(0x12, 0xC9, 0x72)
                            } else {
                                egui::Color32::from_rgb(0xA8, 0xB0, 0xBE)
                            },
                        );
                    });
                });
                ui.add_space(10.0);
                compact_labeled_text_input(
                    ui,
                    tr(lang, "Логин", "Login"),
                    &mut self.config.ui.address_book_account,
                    "user@example.com",
                    false,
                );
                compact_labeled_text_input(
                    ui,
                    tr(lang, "Пароль или токен", "Password or token"),
                    &mut self.config.ui.address_book_token,
                    tr(lang, "пароль", "password"),
                    true,
                );
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    let can_sign_in = !self.config.ui.address_book_account.trim().is_empty()
                        && !self.config.ui.address_book_token.trim().is_empty();
                    if ui
                        .add_enabled(
                            can_sign_in,
                            egui::Button::new(tr(lang, "Войти", "Sign in"))
                                .min_size(egui::vec2(92.0, 34.0)),
                        )
                        .clicked()
                    {
                        match self.sync_address_book() {
                            Ok(()) => {
                                self.address_book_status = Some(
                                    tr(lang, "Адресная книга загружена", "Address book loaded")
                                        .to_owned(),
                                );
                                self.config.save();
                                close_window = true;
                            }
                            Err(err) => self.address_book_status = Some(err),
                        }
                    }
                    if ui
                        .add_enabled(
                            self.config.ui.address_book_signed_in,
                            egui::Button::new(tr(lang, "Обновить", "Refresh"))
                                .min_size(egui::vec2(92.0, 34.0)),
                        )
                        .clicked()
                    {
                        match self.sync_address_book() {
                            Ok(()) => {
                                self.address_book_status = Some(
                                    tr(lang, "Контакты обновлены", "Contacts refreshed").to_owned(),
                                );
                                self.config.save();
                            }
                            Err(err) => self.address_book_status = Some(err),
                        }
                    }
                    if ui
                        .add_enabled(
                            self.config.ui.address_book_signed_in,
                            egui::Button::new(tr(lang, "Выйти", "Sign out"))
                                .min_size(egui::vec2(82.0, 34.0)),
                        )
                        .clicked()
                    {
                        self.config.ui.address_book_signed_in = false;
                        self.config.ui.address_book_guid.clear();
                        self.config.ui.address_book_access_token.clear();
                        self.config.save();
                        self.address_book_status =
                            Some(tr(lang, "Аккаунт отключен", "Account signed out").to_owned());
                    }
                });
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("API: {}", self.config.server.api_url))
                        .size(11.5)
                        .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                );
                if let Some(status) = &self.address_book_status {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(status)
                            .size(12.0)
                            .color(egui::Color32::from_rgb(0x57, 0x60, 0x70)),
                    );
                }
            });
        self.show_address_book_auth = open && !close_window;
    }

    fn new_contact_window(&mut self, ctx: &egui::Context) {
        if !self.show_new_contact_dialog {
            return;
        }
        let lang = self.ui_lang;
        let mut open = self.show_new_contact_dialog;
        let mut close_window = false;
        egui::Window::new(tr(lang, "Новый контакт", "New contact"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.set_min_width(340.0);
                compact_labeled_text_input(
                    ui,
                    tr(lang, "Имя", "Name"),
                    &mut self.new_contact_name,
                    tr(lang, "например: офис", "example: office"),
                    false,
                );
                compact_labeled_text_input(
                    ui,
                    "ID",
                    &mut self.new_contact_id,
                    "123 456 789",
                    false,
                );
                compact_labeled_text_input(
                    ui,
                    tr(lang, "Заметка", "Note"),
                    &mut self.new_contact_note,
                    tr(lang, "компьютер, отдел, владелец", "computer, team, owner"),
                    false,
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let id = normalize_remote_id(&self.new_contact_id);
                    let can_add = self.config.ui.address_book_signed_in && !id.is_empty();
                    if ui
                        .add_enabled(
                            can_add,
                            egui::Button::new(tr(lang, "Добавить", "Add"))
                                .min_size(egui::vec2(110.0, 34.0)),
                        )
                        .clicked()
                    {
                        let contact = ContactEntry {
                            name: self.new_contact_name.trim().to_owned(),
                            remote_id: id,
                            note: self.new_contact_note.trim().to_owned(),
                            machine_id: String::new(),
                            os: address_book::platform().to_owned(),
                            last_seen: String::new(),
                            online: false,
                        };
                        match self.add_address_book_contact(&contact) {
                            Ok(()) => {
                                self.new_contact_name.clear();
                                self.new_contact_id.clear();
                                self.new_contact_note.clear();
                                let _ = self.sync_address_book();
                                self.config.save();
                                self.address_book_status =
                                    Some(tr(lang, "Контакт добавлен", "Contact added").to_owned());
                                close_window = true;
                            }
                            Err(err) => self.address_book_status = Some(err),
                        }
                    }
                    if ui.button(tr(lang, "Закрыть", "Close")).clicked() {
                        close_window = true;
                    }
                });
                if !self.config.ui.address_book_signed_in {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(tr(
                            lang,
                            "Сначала войдите в адресную книгу.",
                            "Sign in to the address book first.",
                        ))
                        .size(12.0)
                        .color(egui::Color32::from_rgb(0xA0, 0x5A, 0x1B)),
                    );
                }
            });
        self.show_new_contact_dialog = open && !close_window;
    }

    fn contact_details_window(&mut self, ctx: &egui::Context) {
        let Some(idx) = self.selected_contact_idx else {
            return;
        };
        if idx >= self.config.ui.contacts.len() {
            self.selected_contact_idx = None;
            self.contact_details_draft = None;
            return;
        }

        let lang = self.ui_lang;
        let source_contact = self.config.ui.contacts[idx].clone();
        let draft_remote_id = self
            .contact_details_draft
            .as_ref()
            .map(|contact| contact.remote_id.as_str());
        if draft_remote_id != Some(source_contact.remote_id.as_str()) {
            self.contact_details_draft = Some(source_contact);
        }

        let mut open = true;
        let mut close_window = false;
        let mut update_contact: Option<ContactEntry> = None;
        let mut remove_remote_id: Option<String> = None;
        let mut connect_to: Option<String> = None;

        egui::Window::new(tr(lang, "Информация о контакте", "Contact details"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                let Some(contact) = self.contact_details_draft.as_mut() else {
                    return;
                };
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(if contact.name.trim().is_empty() {
                                format_peer_id(&contact.remote_id)
                            } else {
                                contact.name.clone()
                            })
                            .size(18.0)
                            .strong()
                            .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                        );
                        ui.label(
                            egui::RichText::new(format_peer_id(&contact.remote_id))
                                .size(13.0)
                                .monospace()
                                .color(egui::Color32::from_rgb(0x57, 0x60, 0x70)),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        status_dot(
                            ui,
                            if contact.online {
                                tr(lang, "online", "online")
                            } else {
                                tr(lang, "offline", "offline")
                            },
                            if contact.online {
                                egui::Color32::from_rgb(0x12, 0xC9, 0x72)
                            } else {
                                egui::Color32::from_rgb(0xA8, 0xB0, 0xBE)
                            },
                        );
                    });
                });
                ui.add_space(12.0);
                compact_info_row(ui, "ID", &format_peer_id(&contact.remote_id));
                if !contact.machine_id.trim().is_empty() {
                    compact_info_row(ui, tr(lang, "Машина", "Machine"), &contact.machine_id);
                }
                if !contact.os.trim().is_empty() {
                    compact_info_row(ui, tr(lang, "Система", "System"), &contact.os);
                }
                if !contact.last_seen.trim().is_empty() {
                    compact_info_row(ui, tr(lang, "Был в сети", "Last seen"), &contact.last_seen);
                }
                compact_labeled_text_input(
                    ui,
                    tr(lang, "Имя", "Name"),
                    &mut contact.name,
                    tr(lang, "Псевдоним", "Alias"),
                    false,
                );
                compact_labeled_text_input(
                    ui,
                    tr(lang, "Заметка", "Note"),
                    &mut contact.note,
                    tr(lang, "владелец, отдел, назначение", "owner, team, purpose"),
                    false,
                );
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add(
                            egui::Button::new(tr(lang, "Подключиться", "Connect"))
                                .min_size(egui::vec2(118.0, 34.0)),
                        )
                        .clicked()
                    {
                        connect_to = Some(contact.remote_id.clone());
                        close_window = true;
                    }
                    if ui
                        .add(
                            egui::Button::new(tr(lang, "Сохранить", "Save"))
                                .min_size(egui::vec2(104.0, 34.0)),
                        )
                        .clicked()
                    {
                        update_contact = Some(contact.clone());
                    }
                    if ui
                        .add(
                            egui::Button::new(tr(lang, "Удалить", "Remove"))
                                .min_size(egui::vec2(92.0, 34.0)),
                        )
                        .clicked()
                    {
                        remove_remote_id = Some(contact.remote_id.clone());
                        close_window = true;
                    }
                });
            });

        if let Some(contact) = update_contact {
            match self.update_address_book_contact(&contact) {
                Ok(()) => {
                    if idx < self.config.ui.contacts.len() {
                        self.config.ui.contacts[idx] = contact;
                    }
                    self.address_book_status =
                        Some(tr(lang, "Контакт сохранен", "Contact saved").to_owned());
                    self.config.save();
                }
                Err(err) => self.address_book_status = Some(err),
            }
        }
        if let Some(remote_id) = remove_remote_id {
            match self.delete_address_book_contact(&remote_id) {
                Ok(()) => {
                    if idx < self.config.ui.contacts.len()
                        && self.config.ui.contacts[idx].remote_id == remote_id
                    {
                        self.config.ui.contacts.remove(idx);
                    } else {
                        self.config
                            .ui
                            .contacts
                            .retain(|contact| contact.remote_id != remote_id);
                    }
                    self.config.save();
                    self.address_book_status =
                        Some(tr(lang, "Контакт удален", "Contact removed").to_owned());
                }
                Err(err) => self.address_book_status = Some(err),
            }
        }
        if let Some(id) = connect_to {
            self.remote_id = id;
            self.mode = AppMode::Connect;
        }
        if !open || close_window {
            self.selected_contact_idx = None;
            self.contact_details_draft = None;
        }
    }

    fn sync_address_book(&mut self) -> Result<(), String> {
        let mut token = self.ensure_address_book_token()?;
        let mut guid = match address_book::personal_ab_guid(&self.config.server.api_url, &token) {
            Ok(guid) => guid,
            Err(err) if err.contains("401") || err.contains("403") => {
                self.config.ui.address_book_signed_in = false;
                self.config.ui.address_book_access_token.clear();
                token = self.ensure_address_book_token()?;
                address_book::personal_ab_guid(&self.config.server.api_url, &token)?
            }
            Err(err) => return Err(err),
        };
        let contacts = match address_book::peers(&self.config.server.api_url, &token, &guid) {
            Ok(contacts) => contacts,
            Err(err) if err.contains("401") || err.contains("403") => {
                self.config.ui.address_book_signed_in = false;
                self.config.ui.address_book_access_token.clear();
                token = self.ensure_address_book_token()?;
                guid = address_book::personal_ab_guid(&self.config.server.api_url, &token)?;
                address_book::peers(&self.config.server.api_url, &token, &guid)?
            }
            Err(err) => return Err(err),
        };

        self.config.ui.address_book_guid = guid;
        self.config.ui.contacts = contacts;
        self.config.ui.address_book_signed_in = true;
        self.config.save();
        Ok(())
    }

    fn ensure_address_book_token(&mut self) -> Result<String, String> {
        if self.config.ui.address_book_signed_in
            && !self.config.ui.address_book_access_token.trim().is_empty()
        {
            return Ok(self.config.ui.address_book_access_token.clone());
        }

        let credential = self.config.ui.address_book_token.trim();
        if credential.is_empty() {
            return Err(self
                .text("Укажите пароль или токен", "Enter password or token")
                .to_owned());
        }

        let token = address_book::login(
            &self.config.server.api_url,
            self.config.ui.address_book_account.trim(),
            credential,
            &self.config.local_id,
            &self.config.ui.agent_machine_id,
        )?;
        self.config.ui.address_book_access_token = token.clone();
        self.config.ui.address_book_signed_in = true;
        self.config.save();
        Ok(token)
    }

    fn add_address_book_contact(&mut self, contact: &ContactEntry) -> Result<(), String> {
        let guid = self.ensure_address_book_guid()?;
        let token = self.ensure_address_book_token()?;
        address_book::add_peer(&self.config.server.api_url, &token, &guid, contact)
    }

    fn update_address_book_contact(&mut self, contact: &ContactEntry) -> Result<(), String> {
        let guid = self.ensure_address_book_guid()?;
        let token = self.ensure_address_book_token()?;
        address_book::update_peer(&self.config.server.api_url, &token, &guid, contact)
    }

    fn delete_address_book_contact(&mut self, remote_id: &str) -> Result<(), String> {
        let guid = self.ensure_address_book_guid()?;
        let token = self.ensure_address_book_token()?;
        address_book::delete_peer(&self.config.server.api_url, &token, &guid, remote_id)
    }

    fn ensure_address_book_guid(&mut self) -> Result<String, String> {
        if self.config.ui.address_book_guid.trim().is_empty() {
            self.sync_address_book()?;
        }
        Ok(self.config.ui.address_book_guid.clone())
    }
}

fn compact_panel_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(egui::Color32::from_rgb(0xFF, 0xFF, 0xFF))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(0xE2, 0xE6, 0xEE),
        ))
        .rounding(egui::Rounding::same(8.0))
        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
}

fn contact_grid_columns(available: f32) -> usize {
    if available >= 920.0 {
        4
    } else if available >= 680.0 {
        3
    } else if available >= 430.0 {
        2
    } else {
        1
    }
}

fn contact_tile_width(available: f32, columns: usize, gap: f32) -> f32 {
    ((available - gap * columns.saturating_sub(1) as f32) / columns as f32)
        .max(150.0)
        .min(available)
}

fn draw_contact_tile(
    ui: &mut egui::Ui,
    contact: &ContactEntry,
    tile_width: f32,
    tile_height: f32,
    lang: UiLang,
    mut open_details: impl FnMut(),
    mut connect: impl FnMut(),
    mut remove: impl FnMut(),
) {
    contact_tile_frame(contact.online).show(ui, |ui| {
        let inner_width = (tile_width - 22.0).max(120.0);
        let inner_height = (tile_height - 22.0).max(100.0);
        ui.set_min_size(egui::vec2(inner_width, inner_height));
        ui.set_max_width(inner_width);

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_max_width((inner_width - 52.0).max(88.0));
                ui.add_sized(
                    egui::vec2(ui.available_width(), 20.0),
                    egui::Label::new(
                        egui::RichText::new(if contact.name.trim().is_empty() {
                            format_peer_id(&contact.remote_id)
                        } else {
                            contact.name.clone()
                        })
                        .size(14.0)
                        .strong()
                        .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                    )
                    .wrap(false),
                );
                ui.add_sized(
                    egui::vec2(ui.available_width(), 18.0),
                    egui::Label::new(
                        egui::RichText::new(format_peer_id(&contact.remote_id))
                            .size(12.5)
                            .monospace()
                            .color(egui::Color32::from_rgb(0x57, 0x60, 0x70)),
                    )
                    .wrap(false),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let menu = ui.menu_button("☰", |ui| {
                    if ui.button(tr(lang, "Информация", "Details")).clicked() {
                        open_details();
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, "Подключиться", "Connect")).clicked() {
                        connect();
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, "Удалить", "Remove")).clicked() {
                        remove();
                        ui.close_menu();
                    }
                });
                menu.response.on_hover_text(tr(lang, "Действия", "Actions"));
            });
        });

        ui.add_space(8.0);
        let note = if contact.note.trim().is_empty() {
            if contact.os.trim().is_empty() {
                tr(lang, "Без заметки", "No note").to_owned()
            } else {
                contact.os.trim().to_owned()
            }
        } else {
            contact.note.trim().to_owned()
        };
        ui.add_sized(
            egui::vec2(ui.available_width(), 18.0),
            egui::Label::new(
                egui::RichText::new(note)
                    .size(12.0)
                    .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
            )
            .wrap(false),
        );

        ui.add_space((inner_height - 98.0).max(0.0));
        ui.horizontal(|ui| {
            status_dot(
                ui,
                if contact.online {
                    tr(lang, "online", "online")
                } else {
                    tr(lang, "offline", "offline")
                },
                if contact.online {
                    egui::Color32::from_rgb(0x12, 0xC9, 0x72)
                } else {
                    egui::Color32::from_rgb(0xA8, 0xB0, 0xBE)
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(tr(lang, "Подключить", "Connect"))
                            .min_size(egui::vec2(92.0, 28.0)),
                    )
                    .clicked()
                {
                    connect();
                }
            });
        });
    });
}

fn contact_tile_frame(online: bool) -> egui::Frame {
    egui::Frame::none()
        .fill(if online {
            egui::Color32::from_rgb(0xFA, 0xFF, 0xFC)
        } else {
            egui::Color32::from_rgb(0xFF, 0xFF, 0xFF)
        })
        .stroke(egui::Stroke::new(
            1.0,
            if online {
                egui::Color32::from_rgb(0xB8, 0xE8, 0xCE)
            } else {
                egui::Color32::from_rgb(0xE2, 0xE6, 0xEE)
            },
        ))
        .rounding(egui::Rounding::same(8.0))
        .inner_margin(egui::Margin::same(10.0))
}

fn status_dot(ui: &mut egui::Ui, label: &str, dot: egui::Color32) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, dot);
        ui.label(
            egui::RichText::new(label)
                .size(11.5)
                .color(egui::Color32::from_rgb(0x57, 0x60, 0x70)),
        );
    });
}

fn compact_labeled_text_input(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    hint: &str,
    password: bool,
) {
    ui.label(
        egui::RichText::new(label)
            .size(12.5)
            .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
    );
    ui.add_sized(
        egui::vec2(ui.available_width(), 34.0),
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .password(password)
            .font(egui::TextStyle::Button),
    );
    ui.add_space(8.0);
}

fn compact_info_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(96.0, 20.0),
            egui::Label::new(
                egui::RichText::new(label)
                    .size(12.0)
                    .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
            )
            .wrap(false),
        );
        ui.add_sized(
            egui::vec2((ui.available_width()).max(80.0), 20.0),
            egui::Label::new(
                egui::RichText::new(value)
                    .size(12.5)
                    .color(egui::Color32::from_rgb(0x20, 0x24, 0x2D)),
            )
            .wrap(false),
        );
    });
    ui.add_space(4.0);
}
