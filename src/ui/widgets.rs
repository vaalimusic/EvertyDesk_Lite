use eframe::egui;

/// Outer workspace container for the main screen content.
pub(crate) fn workspace_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::NONE)
        .corner_radius(egui::CornerRadius::same(18))
        .inner_margin(egui::Margin::same(0))
}

pub(crate) fn card_frame() -> egui::Frame {
    // EvertyGUI: единый источник правды — theme::card().
    crate::theme::card()
}

pub(crate) fn settings_section(
    ui: &mut egui::Ui,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::NONE
        .fill(crate::theme::palette().surface)
        .stroke(egui::Stroke::new(1.0, crate::theme::palette().border))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(title)
                    .size(15.0)
                    .strong()
                    .color(crate::theme::palette().text),
            );
            ui.add_space(10.0);
            add_contents(ui);
        });
}

pub(crate) fn settings_text_row(ui: &mut egui::Ui, label: &str, value: &mut String) {
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
                    egui::TextEdit::singleline(value).font(egui::TextStyle::Button),
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
            egui::TextEdit::singleline(value).font(egui::TextStyle::Button),
        );
    }
    ui.add_space(6.0);
}

#[allow(dead_code)]
pub(crate) fn danger_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(13.0)
                .color(crate::theme::palette().danger),
        )
        .min_size(egui::vec2(ui.available_width(), 40.0))
        .fill(crate::theme::tint(crate::theme::palette().danger, 0.10))
        .stroke(egui::Stroke::new(
            1.0,
            crate::theme::tint(crate::theme::palette().danger, 0.40),
        ))
        .corner_radius(egui::CornerRadius::same(10)),
    )
}

pub(crate) fn primary_connect_button(ui: &mut egui::Ui, text: &str, icon: &str) -> egui::Response {
    let t = crate::theme::palette();
    // Резервируем чуть больше высоты, чем сама кнопка: сверху/снизу остаётся
    // воздух, кнопка не «прилипает» к соседям, и снизу есть место под свечение.
    const BTN_H: f32 = 46.0;
    const PAD_V: f32 = 7.0; // вертикальный воздух
    let full = egui::vec2(ui.available_width(), BTN_H + PAD_V * 2.0);
    let (outer, response) = ui.allocate_exact_size(full, egui::Sense::click());
    // Кнопка по центру зарезервированной области.
    let rect = egui::Rect::from_center_size(outer.center(), egui::vec2(outer.width(), BTN_H));

    let radius = egui::CornerRadius::same(crate::theme::radius::MD);
    let fill = if response.is_pointer_button_down_on() {
        t.accent_active
    } else if response.hovered() {
        t.accent_hover
    } else {
        t.accent
    };

    // Мягкое «свечение» под кнопкой — приподнятость без тяжёлой тени.
    // Несколько полупрозрачных слоёв со смещением вниз и расширением.
    if response.hovered() {
        for i in 1..=3 {
            let grow = i as f32 * 2.0;
            let glow = rect.translate(egui::vec2(0.0, 3.0)).expand(grow);
            ui.painter().rect_filled(
                glow,
                egui::CornerRadius::same(crate::theme::radius::MD + i as u8),
                crate::theme::tint(t.accent, 0.05),
            );
        }
    }

    ui.painter().rect_filled(rect, radius, fill);
    // Тонкий верхний блик + нижняя кромка темнее для объёма.
    ui.painter().rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0, crate::theme::mix(fill, egui::Color32::BLACK, 0.12)),
        egui::StrokeKind::Inside,
    );

    let fg = t.accent_fg;
    let icon_rect = egui::Rect::from_min_size(
        rect.min + egui::vec2(18.0, (BTN_H - 20.0) / 2.0),
        egui::vec2(20.0, 20.0),
    );
    draw_line_icon(ui.painter(), icon_rect, icon, fg);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(16.0),
        fg,
    );

    // Шеврон-стрелка справа.
    let x = rect.max.x - 24.0;
    let y = rect.center().y;
    ui.painter().line_segment(
        [egui::pos2(x - 7.0, y), egui::pos2(x + 4.0, y)],
        egui::Stroke::new(1.8, fg),
    );
    ui.painter().line_segment(
        [egui::pos2(x, y - 5.0), egui::pos2(x + 5.0, y)],
        egui::Stroke::new(1.8, fg),
    );
    ui.painter().line_segment(
        [egui::pos2(x + 5.0, y), egui::pos2(x, y + 5.0)],
        egui::Stroke::new(1.8, fg),
    );
    response
}

pub(crate) fn mode_segment_button(
    ui: &mut egui::Ui,
    text: &str,
    icon: &str,
    active: bool,
    width: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 38.0), egui::Sense::click());
    let fill = if active {
        crate::theme::accent_tint(&crate::theme::palette(), 0.16)
    } else if response.hovered() {
        crate::theme::palette().surface_raised
    } else {
        crate::theme::palette().surface
    };
    let stroke = if active {
        egui::Stroke::new(1.2, crate::theme::palette().accent)
    } else {
        egui::Stroke::new(1.0, crate::theme::palette().border)
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(10), fill);
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(10),
        stroke,
        egui::StrokeKind::Inside,
    );
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.min.x + 24.0, rect.center().y),
        egui::vec2(18.0, 18.0),
    );
    let color = if active {
        crate::theme::palette().accent
    } else {
        crate::theme::palette().text_weak
    };
    draw_line_icon(ui.painter(), icon_rect, icon, color);
    ui.painter().text(
        egui::pos2(rect.min.x + 46.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(14.0),
        if active {
            crate::theme::palette().text
        } else {
            crate::theme::palette().text_weak
        },
    );
    response
}

pub(crate) fn status_pill(ui: &mut egui::Ui, label: &str, dot: egui::Color32) {
    egui::Frame::NONE
        .fill(crate::theme::palette().surface)
        .stroke(egui::Stroke::new(1.0, crate::theme::palette().border))
        .corner_radius(egui::CornerRadius::same(20))
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 5.0, dot);
            ui.add_space(7.0);
            ui.label(
                egui::RichText::new(label)
                    .size(14.0)
                    .color(crate::theme::palette().text),
            );
        });
}

pub(crate) fn compact_text_input(
    ui: &mut egui::Ui,
    value: &mut String,
    hint: &str,
    password: bool,
    enabled: bool,
    font_size: Option<f32>,
) -> egui::Response {
    let desired_size = egui::vec2(ui.available_width(), 44.0);
    let (rect, mut response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    let text_rect = rect.shrink2(egui::vec2(12.0, 5.0));
    let text_id = ui.make_persistent_id(("compact_text_input", hint));

    let is_focused = ui.memory(|memory| memory.has_focus(text_id));
    let fill = if !enabled {
        crate::theme::palette().surface_raised
    } else if is_focused {
        crate::theme::palette().surface
    } else if response.hovered() {
        crate::theme::palette().surface_raised
    } else {
        crate::theme::palette().surface
    };
    let stroke = if is_focused {
        egui::Stroke::new(1.8, crate::theme::palette().accent)
    } else if response.hovered() {
        egui::Stroke::new(1.6, crate::theme::palette().border_strong)
    } else {
        egui::Stroke::new(1.5, crate::theme::palette().border)
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(10), fill);
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(10),
        stroke,
        egui::StrokeKind::Inside,
    );

    let inner = ui.scope_builder(egui::UiBuilder::new().max_rect(text_rect), |ui| {
        let edit = egui::TextEdit::singleline(value)
            .id(text_id)
            .hint_text(hint)
            .password(password)
            .desired_width(f32::INFINITY)
            .text_color(crate::theme::palette().text)
            .font(
                font_size
                    .map(egui::FontId::proportional)
                    .unwrap_or_else(|| egui::FontId::proportional(16.0)),
            )
            .frame(egui::Frame::NONE);
        ui.add_enabled(enabled, edit)
    });
    response |= inner.inner;
    response.context_menu(|ui| {
        if ui.button("Копировать").clicked() {
            ui.ctx().copy_text(value.clone());
            ui.close();
        }
        if ui.button("Вставить").clicked() {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                if let Ok(text) = clipboard.get_text() {
                    *value = text;
                }
            }
            ui.close();
        }
        if ui.button("Очистить").clicked() {
            value.clear();
            ui.close();
        }
    });
    response
}

pub(crate) fn icon_button(ui: &mut egui::Ui, icon: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::click());
    let fill = if response.hovered() {
        crate::theme::palette().surface_raised
    } else {
        crate::theme::palette().surface
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(10), fill);
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(10),
        egui::Stroke::new(1.0, crate::theme::palette().border),
        egui::StrokeKind::Inside,
    );
    draw_line_icon(
        ui.painter(),
        rect.shrink(10.0),
        icon,
        crate::theme::palette().text,
    );
    response
}

pub(crate) fn language_button(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let fill = if active {
        crate::theme::palette().surface
    } else {
        crate::theme::palette().surface_raised
    };
    let stroke = if active {
        crate::theme::palette().text_muted
    } else {
        crate::theme::palette().border
    };
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(13.0)
                .strong()
                .color(crate::theme::palette().text),
        )
        .min_size(egui::vec2(56.0, 34.0))
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(egui::CornerRadius::same(9)),
    )
}

pub(crate) fn nav_icon_button(
    ui: &mut egui::Ui,
    label: &str,
    icon: &str,
    active: bool,
    chevron: bool,
) -> egui::Response {
    let desired = egui::vec2(ui.available_width(), 44.0);
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());
    let t = crate::theme::palette();

    let fill = if active {
        crate::theme::accent_tint(&t, if t.mode.is_dark() { 0.12 } else { 0.08 })
    } else if response.hovered() {
        t.surface_raised
    } else {
        egui::Color32::TRANSPARENT
    };
    let border_stroke = if active {
        egui::Stroke::new(1.0, crate::theme::accent_tint(&t, 0.35))
    } else if response.hovered() {
        egui::Stroke::new(1.0, t.border_strong)
    } else {
        egui::Stroke::NONE
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(12), fill);
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(12),
        border_stroke,
        egui::StrokeKind::Inside,
    );

    // Акцентная полоса слева для активного пункта
    if active {
        let bar = egui::Rect::from_min_size(
            rect.min + egui::vec2(0.0, 8.0),
            egui::vec2(3.0, rect.height() - 16.0),
        );
        ui.painter()
            .rect_filled(bar, egui::CornerRadius::same(2), t.accent);
    }

    let icon_rect =
        egui::Rect::from_min_size(rect.min + egui::vec2(14.0, 12.0), egui::vec2(20.0, 20.0));
    let icon_color = if active { t.accent } else { t.text_weak };
    draw_line_icon(ui.painter(), icon_rect, icon, icon_color);
    ui.painter().text(
        egui::pos2(rect.min.x + 46.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        if active { t.text } else { t.text_weak },
    );
    if chevron {
        let x = rect.max.x - 18.0;
        let y = rect.center().y;
        let col = t.text_weak;
        ui.painter().line_segment(
            [egui::pos2(x - 3.0, y - 6.0), egui::pos2(x + 3.0, y)],
            egui::Stroke::new(1.6, col),
        );
        ui.painter().line_segment(
            [egui::pos2(x + 3.0, y), egui::pos2(x - 3.0, y + 6.0)],
            egui::Stroke::new(1.6, col),
        );
    }
    response
}

fn draw_line_icon(p: &egui::Painter, rect: egui::Rect, icon: &str, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.8, color);
    let c = rect.center();
    match icon {
        "monitor" => {
            let screen =
                egui::Rect::from_center_size(c + egui::vec2(0.0, -2.0), egui::vec2(18.0, 13.0));
            p.rect_stroke(
                screen,
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            p.line_segment(
                [
                    egui::pos2(c.x, screen.max.y),
                    egui::pos2(c.x, screen.max.y + 5.0),
                ],
                stroke,
            );
            p.line_segment(
                [
                    egui::pos2(c.x - 6.0, screen.max.y + 5.0),
                    egui::pos2(c.x + 6.0, screen.max.y + 5.0),
                ],
                stroke,
            );
        }
        "settings" => {
            p.circle_stroke(c, 7.0, stroke);
            p.circle_stroke(c, 2.4, stroke);
            for a in [0.0_f32, 1.57, 3.14, 4.71] {
                let dir = egui::vec2(a.cos(), a.sin());
                p.line_segment([c + dir * 9.0, c + dir * 11.0], stroke);
            }
        }
        "copy" => {
            let back =
                egui::Rect::from_min_size(rect.min + egui::vec2(3.0, 1.0), egui::vec2(11.0, 13.0));
            let front =
                egui::Rect::from_min_size(rect.min + egui::vec2(7.0, 5.0), egui::vec2(11.0, 13.0));
            p.rect_stroke(
                back,
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                front,
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        "refresh" => {
            p.circle_stroke(c, 7.0, stroke);
            p.line_segment(
                [c + egui::vec2(5.0, -7.0), c + egui::vec2(9.0, -7.0)],
                stroke,
            );
            p.line_segment(
                [c + egui::vec2(9.0, -7.0), c + egui::vec2(9.0, -3.0)],
                stroke,
            );
        }
        "connect" => {
            let left = c + egui::vec2(-7.0, 0.0);
            let right = c + egui::vec2(7.0, 0.0);
            p.circle_stroke(left, 4.0, stroke);
            p.circle_stroke(right, 4.0, stroke);
            p.line_segment(
                [left + egui::vec2(4.0, 0.0), right + egui::vec2(-4.0, 0.0)],
                stroke,
            );
        }
        "console" => {
            let screen = egui::Rect::from_center_size(c, egui::vec2(19.0, 15.0));
            p.rect_stroke(
                screen,
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            p.line_segment(
                [
                    screen.min + egui::vec2(4.0, 5.0),
                    screen.min + egui::vec2(7.5, 7.5),
                ],
                stroke,
            );
            p.line_segment(
                [
                    screen.min + egui::vec2(7.5, 7.5),
                    screen.min + egui::vec2(4.0, 10.0),
                ],
                stroke,
            );
            p.line_segment(
                [
                    screen.min + egui::vec2(10.5, 10.0),
                    screen.min + egui::vec2(15.0, 10.0),
                ],
                stroke,
            );
        }
        "history" => {
            p.circle_stroke(c, 8.0, stroke);
            p.line_segment([c, c + egui::vec2(0.0, -5.0)], stroke);
            p.line_segment([c, c + egui::vec2(5.0, 2.0)], stroke);
        }
        "contacts" => {
            p.circle_stroke(c + egui::vec2(0.0, -5.0), 4.0, stroke);
            let body = [
                c + egui::vec2(-8.0, 9.0),
                c + egui::vec2(-5.0, 3.0),
                c + egui::vec2(0.0, 1.0),
                c + egui::vec2(5.0, 3.0),
                c + egui::vec2(8.0, 9.0),
            ];
            for pair in body.windows(2) {
                p.line_segment([pair[0], pair[1]], stroke);
            }
        }
        "game-controller" => {
            let body = egui::Rect::from_center_size(c, egui::vec2(18.0, 11.0));
            p.rect_stroke(
                body,
                egui::CornerRadius::same(4),
                stroke,
                egui::StrokeKind::Inside,
            );
            let lx = c.x - 5.5;
            p.line_segment(
                [egui::pos2(lx, c.y - 3.5), egui::pos2(lx, c.y + 3.5)],
                stroke,
            );
            p.line_segment(
                [egui::pos2(lx - 3.5, c.y), egui::pos2(lx + 3.5, c.y)],
                stroke,
            );
            p.circle_stroke(c + egui::vec2(5.5, 0.0), 2.0, stroke);
        }
        "server" => {
            // Сервер: три горизонтальные «полки»
            for dy in [-5.0_f32, 0.0, 5.0] {
                let r =
                    egui::Rect::from_center_size(c + egui::vec2(0.0, dy), egui::vec2(18.0, 3.5));
                p.rect_stroke(
                    r,
                    egui::CornerRadius::same(2),
                    stroke,
                    egui::StrokeKind::Inside,
                );
                p.circle_stroke(c + egui::vec2(6.5, dy), 0.9, stroke);
            }
        }
        _ => {
            let points = [
                c + egui::vec2(0.0, -9.0),
                c + egui::vec2(9.0, 0.0),
                c + egui::vec2(0.0, 9.0),
                c + egui::vec2(-9.0, 0.0),
            ];
            p.line_segment([points[0], points[1]], stroke);
            p.line_segment([points[1], points[2]], stroke);
            p.line_segment([points[2], points[3]], stroke);
            p.line_segment([points[3], points[0]], stroke);
            p.circle_stroke(c, 3.0, stroke);
        }
    }
}

/// Кликабельный чип (для недавних ID). Возвращает Response.
pub(crate) fn recent_chip(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let pad_x = 12.0;
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(13.0),
        crate::theme::palette().text,
    );
    let w = galley.size().x + pad_x * 2.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(w, 30.0), egui::Sense::click());
    let fill = if response.hovered() {
        crate::theme::accent_tint(&crate::theme::palette(), 0.16) // зеленоватый hover
    } else {
        crate::theme::palette().surface_raised
    };
    let stroke = if response.hovered() {
        egui::Stroke::new(1.0, crate::theme::palette().accent)
    } else {
        egui::Stroke::new(1.0, crate::theme::palette().border)
    };
    let p = ui.painter();
    p.rect_filled(rect, egui::CornerRadius::same(15), fill);
    p.rect_stroke(
        rect,
        egui::CornerRadius::same(15),
        stroke,
        egui::StrokeKind::Inside,
    );
    p.galley(
        egui::pos2(rect.left() + pad_x, rect.center().y - galley.size().y / 2.0),
        galley,
        egui::Color32::PLACEHOLDER,
    );
    response
}

/// Компактная «пилюля» статуса: цветная точка + label + значение.
/// Используется в нижней панели стрима вместо простыни текста.
pub(crate) fn stat_pill(
    ui: &mut egui::Ui,
    dot: Option<egui::Color32>,
    text: &str,
    value_color: egui::Color32,
) -> egui::Response {
    let font = egui::FontId::proportional(11.5);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, value_color);
    let dot_w = if dot.is_some() { 14.0 } else { 0.0 };
    let pad = 9.0;
    let w = galley.size().x + dot_w + pad * 2.0;
    let h = 22.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(
        rect,
        egui::CornerRadius::same(11),
        egui::Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 16),
    );
    let mut x = rect.left() + pad;
    if let Some(c) = dot {
        p.circle_filled(egui::pos2(x + 3.0, rect.center().y), 3.5, c);
        x += dot_w;
    }
    p.galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        egui::Color32::PLACEHOLDER,
    );
    response
}

/// Заголовок + крупное значение для карточки деталей.
pub(crate) fn info_metric(ui: &mut egui::Ui, label: &str, value: &str, color: egui::Color32) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(label)
                .size(11.0)
                .color(crate::theme::palette().text_muted),
        );
        ui.label(egui::RichText::new(value).size(17.0).strong().color(color));
    });
}
