use eframe::egui;

/// Outer workspace container for the main screen content.
pub(crate) fn workspace_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::NONE)
        .rounding(egui::Rounding::same(18))
        .inner_margin(egui::Margin::same(0))
}

pub(crate) fn card_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(egui::Color32::from_rgb(0xFF, 0xFF, 0xFF))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(0xE3, 0xE6, 0xEC),
        ))
        .rounding(egui::Rounding::same(14))
        .inner_margin(egui::Margin::same(16))
}

pub(crate) fn settings_section(
    ui: &mut egui::Ui,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::none()
        .fill(egui::Color32::from_rgb(0xFF, 0xFF, 0xFF))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(0xE3, 0xE6, 0xEC),
        ))
        .rounding(egui::Rounding::same(12))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(title)
                    .size(15.0)
                    .strong()
                    .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
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
                        .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
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
                .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
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
                .color(egui::Color32::from_rgb(0xE5, 0x18, 0x2E)),
        )
        .min_size(egui::vec2(ui.available_width(), 40.0))
        .fill(egui::Color32::from_rgb(0xFF, 0xFA, 0xFA))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(0xF4, 0xB8, 0xBE),
        ))
        .rounding(egui::Rounding::same(10)),
    )
}

pub(crate) fn primary_connect_button(ui: &mut egui::Ui, text: &str, icon: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 46.0), egui::Sense::click());
    let fill = if response.hovered() {
        egui::Color32::from_rgb(0x0B, 0xB8, 0x68)
    } else {
        egui::Color32::from_rgb(0x12, 0xC9, 0x72)
    };
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(11), fill);
    ui.painter().rect_stroke(
        rect,
        egui::Rounding::same(11),
        egui::Stroke::new(1.0, egui::Color32::from_rgb(0x0A, 0xA8, 0x5E)),
        egui::StrokeKind::Inside,
    );

    let icon_rect =
        egui::Rect::from_min_size(rect.min + egui::vec2(18.0, 13.0), egui::vec2(20.0, 20.0));
    draw_line_icon(ui.painter(), icon_rect, icon, egui::Color32::WHITE);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(16.0),
        egui::Color32::WHITE,
    );

    let x = rect.max.x - 24.0;
    let y = rect.center().y;
    ui.painter().line_segment(
        [egui::pos2(x - 7.0, y), egui::pos2(x + 4.0, y)],
        egui::Stroke::new(1.8, egui::Color32::WHITE),
    );
    ui.painter().line_segment(
        [egui::pos2(x, y - 5.0), egui::pos2(x + 5.0, y)],
        egui::Stroke::new(1.8, egui::Color32::WHITE),
    );
    ui.painter().line_segment(
        [egui::pos2(x + 5.0, y), egui::pos2(x, y + 5.0)],
        egui::Stroke::new(1.8, egui::Color32::WHITE),
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
        egui::Color32::from_rgb(0xEC, 0xF8, 0xF2)
    } else if response.hovered() {
        egui::Color32::from_rgb(0xF8, 0xFB, 0xFF)
    } else {
        egui::Color32::from_rgb(0xFF, 0xFF, 0xFF)
    };
    let stroke = if active {
        egui::Stroke::new(1.2, egui::Color32::from_rgb(0x12, 0xC9, 0x72))
    } else {
        egui::Stroke::new(1.0, egui::Color32::from_rgb(0xDF, 0xE5, 0xEE))
    };
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(10), fill);
    ui.painter().rect_stroke(
        rect,
        egui::Rounding::same(10),
        stroke,
        egui::StrokeKind::Inside,
    );
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.min.x + 24.0, rect.center().y),
        egui::vec2(18.0, 18.0),
    );
    let color = if active {
        egui::Color32::from_rgb(0x0C, 0xA8, 0x60)
    } else {
        egui::Color32::from_rgb(0x4E, 0x58, 0x68)
    };
    draw_line_icon(ui.painter(), icon_rect, icon, color);
    ui.painter().text(
        egui::pos2(rect.min.x + 46.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(14.0),
        if active {
            egui::Color32::from_rgb(0x12, 0x17, 0x20)
        } else {
            egui::Color32::from_rgb(0x4F, 0x58, 0x68)
        },
    );
    response
}

pub(crate) fn status_pill(ui: &mut egui::Ui, label: &str, dot: egui::Color32) {
    egui::Frame::none()
        .fill(egui::Color32::from_rgb(0xFF, 0xFF, 0xFF))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(0xE3, 0xE6, 0xEC),
        ))
        .rounding(egui::Rounding::same(20))
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 5.0, dot);
            ui.add_space(7.0);
            ui.label(
                egui::RichText::new(label)
                    .size(14.0)
                    .color(egui::Color32::from_rgb(0x20, 0x24, 0x2D)),
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
        egui::Color32::from_rgb(0xF4, 0xF6, 0xF9)
    } else if is_focused {
        egui::Color32::from_rgb(0xFF, 0xFF, 0xFF)
    } else if response.hovered() {
        egui::Color32::from_rgb(0xF8, 0xFB, 0xFF)
    } else {
        egui::Color32::from_rgb(0xFF, 0xFF, 0xFF)
    };
    let stroke = if is_focused {
        egui::Stroke::new(1.8, egui::Color32::from_rgb(0x5F, 0x86, 0xB8))
    } else if response.hovered() {
        egui::Stroke::new(1.6, egui::Color32::from_rgb(0x7F, 0x98, 0xB8))
    } else {
        egui::Stroke::new(1.5, egui::Color32::from_rgb(0x9E, 0xAC, 0xBF))
    };
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(10), fill);
    ui.painter().rect_stroke(
        rect,
        egui::Rounding::same(10),
        stroke,
        egui::StrokeKind::Inside,
    );

    let inner = ui.allocate_ui_at_rect(text_rect, |ui| {
        let edit = egui::TextEdit::singleline(value)
            .id(text_id)
            .hint_text(hint)
            .password(password)
            .desired_width(f32::INFINITY)
            .text_color(egui::Color32::from_rgb(0x0B, 0x10, 0x1A))
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
            ui.close_menu();
        }
        if ui.button("Вставить").clicked() {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                if let Ok(text) = clipboard.get_text() {
                    *value = text;
                }
            }
            ui.close_menu();
        }
        if ui.button("Очистить").clicked() {
            value.clear();
            ui.close_menu();
        }
    });
    response
}

pub(crate) fn icon_button(ui: &mut egui::Ui, icon: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::click());
    let fill = if response.hovered() {
        egui::Color32::from_rgb(0xF8, 0xFA, 0xFC)
    } else {
        egui::Color32::from_rgb(0xFF, 0xFF, 0xFF)
    };
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(10), fill);
    ui.painter().rect_stroke(
        rect,
        egui::Rounding::same(10),
        egui::Stroke::new(1.0, egui::Color32::from_rgb(0xE3, 0xE6, 0xEC)),
        egui::StrokeKind::Inside,
    );
    draw_line_icon(
        ui.painter(),
        rect.shrink(10.0),
        icon,
        egui::Color32::from_rgb(0x20, 0x24, 0x2D),
    );
    response
}

pub(crate) fn language_button(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let fill = if active {
        egui::Color32::from_rgb(0xFF, 0xFF, 0xFF)
    } else {
        egui::Color32::from_rgb(0xF0, 0xF2, 0xF5)
    };
    let stroke = if active {
        egui::Color32::from_rgb(0xD0, 0xD6, 0xE0)
    } else {
        egui::Color32::from_rgb(0xE3, 0xE6, 0xEC)
    };
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(13.0)
                .strong()
                .color(egui::Color32::from_rgb(0x20, 0x24, 0x2D)),
        )
        .min_size(egui::vec2(56.0, 34.0))
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .rounding(egui::Rounding::same(9)),
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
    let fill = if active || response.hovered() {
        egui::Color32::from_rgb(0xFF, 0xFF, 0xFF)
    } else {
        egui::Color32::TRANSPARENT
    };
    let stroke = if active || response.hovered() {
        egui::Stroke::new(1.0, egui::Color32::from_rgb(0xD9, 0xDD, 0xE5))
    } else {
        egui::Stroke::NONE
    };
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(12), fill);
    ui.painter().rect_stroke(
        rect,
        egui::Rounding::same(12),
        stroke,
        egui::StrokeKind::Inside,
    );

    let icon_rect =
        egui::Rect::from_min_size(rect.min + egui::vec2(13.0, 12.0), egui::vec2(20.0, 20.0));
    draw_line_icon(
        ui.painter(),
        icon_rect,
        icon,
        egui::Color32::from_rgb(0x3F, 0x48, 0x58),
    );
    ui.painter().text(
        egui::pos2(rect.min.x + 44.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        if active {
            egui::Color32::from_rgb(0x16, 0x18, 0x20)
        } else {
            egui::Color32::from_rgb(0x57, 0x60, 0x70)
        },
    );
    if chevron {
        let x = rect.max.x - 18.0;
        let y = rect.center().y;
        let col = egui::Color32::from_rgb(0x57, 0x60, 0x70);
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
                egui::Rounding::same(2),
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
                egui::Rounding::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                front,
                egui::Rounding::same(2),
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
                egui::Rounding::same(2),
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

pub(crate) fn secondary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let width = ui.available_width().min(156.0);
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(13.0)
                .color(egui::Color32::from_rgb(0x20, 0x24, 0x2D)),
        )
        .min_size(egui::vec2(width, 40.0))
        .fill(egui::Color32::from_rgb(0xFF, 0xFF, 0xFF))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(0xE3, 0xE6, 0xEC),
        ))
        .rounding(egui::Rounding::same(10)),
    )
}
