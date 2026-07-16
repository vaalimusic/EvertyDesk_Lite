// =============================================================================
// EvertyDesk — Design System / Theme
//
// Единая дизайн-система: семантические цвет-токены вместо хардкод-RGB по всему
// UI. Две темы (тёмная Pro + светлая премиум), единый фирменный зелёный акцент.
//
// Использование в UI-коде:
//     let t = crate::theme::palette();
//     ui.label(egui::RichText::new("…").color(t.text));
//     button.fill(t.accent);
//
// Тема применяется через `apply(ctx, mode)` — настраивает egui Visuals+Style
// и обновляет глобальную палитру, доступную через `palette()`.
// =============================================================================

use eframe::egui::{self, Color32};
use std::sync::{OnceLock, RwLock};

/// Режим темы — сохраняется в конфиге.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Dark,
    Light,
    /// Follow the OS / system preference (updated live, no restart needed).
    System,
}

impl Default for ThemeMode {
    fn default() -> Self {
        ThemeMode::Dark
    }
}

impl ThemeMode {
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Dark => "Тёмная",
            ThemeMode::Light => "Светлая",
            ThemeMode::System => "Авто",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::System => ThemeMode::Dark,
        }
    }

    pub fn is_dark(self) -> bool {
        matches!(self, ThemeMode::Dark)
    }

    /// Resolve to a concrete Dark or Light mode.
    /// When `System`, falls back to Dark if OS preference is unavailable.
    pub fn resolved(self, system: Option<egui::Theme>) -> Self {
        match self {
            ThemeMode::System => match system {
                Some(egui::Theme::Light) => ThemeMode::Light,
                _ => ThemeMode::Dark,
            },
            other => other,
        }
    }
}

/// Семантические цвет-токены. Copy — берётся из UI без аллокаций.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub mode: ThemeMode,

    // Поверхности (от самой глубокой к приподнятой)
    pub bg: Color32,             // фон приложения
    pub surface: Color32,        // карты, окна
    pub surface_raised: Color32, // приподнятые элементы, hover
    pub surface_sunken: Color32, // утопленные: поля ввода, рельсы

    // Границы
    pub border: Color32,
    pub border_strong: Color32,

    // Текст
    pub text: Color32,
    pub text_weak: Color32,
    pub text_muted: Color32,

    // Фирменный акцент (зелёный)
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_active: Color32,
    pub accent_fg: Color32, // текст/иконка поверх акцента

    // Семантические статусы
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub info: Color32,

    // Выделение
    pub selection_bg: Color32,
    pub selection_stroke: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

impl Palette {
    /// Тёмная тема Pro — глубокий фон, контрастный текст, яркий зелёный акцент.
    pub const fn dark() -> Self {
        Palette {
            mode: ThemeMode::Dark,

            bg: rgb(0x0E, 0x11, 0x16),
            surface: rgb(0x16, 0x1A, 0x22),
            surface_raised: rgb(0x1D, 0x22, 0x2C),
            surface_sunken: rgb(0x0A, 0x0D, 0x12),

            border: rgb(0x27, 0x2D, 0x38),
            border_strong: rgb(0x39, 0x41, 0x50),

            text: rgb(0xE6, 0xE9, 0xEF),
            text_weak: rgb(0x9B, 0xA4, 0xB2),
            text_muted: rgb(0x6B, 0x74, 0x84),

            accent: rgb(0x1F, 0xB8, 0x66),
            accent_hover: rgb(0x29, 0xC9, 0x72),
            accent_active: rgb(0x17, 0xA0, 0x58),
            accent_fg: rgb(0x06, 0x1A, 0x0E),

            success: rgb(0x22, 0xC5, 0x5E),
            warning: rgb(0xF0, 0xB4, 0x29),
            danger: rgb(0xF0, 0x5A, 0x52),
            info: rgb(0x3B, 0x9E, 0xE8),

            selection_bg: rgb(0x1B, 0x3A, 0x2C),
            selection_stroke: rgb(0x1F, 0xB8, 0x66),
        }
    }

    /// Светлая премиум-тема — тёплый фон, мягкие границы, насыщенный акцент.
    pub const fn light() -> Self {
        Palette {
            mode: ThemeMode::Light,

            bg: rgb(0xF5, 0xF7, 0xFB),
            surface: rgb(0xFF, 0xFF, 0xFF),
            surface_raised: rgb(0xFF, 0xFF, 0xFF),
            surface_sunken: rgb(0xEC, 0xEF, 0xF4),

            border: rgb(0xE1, 0xE5, 0xEC),
            border_strong: rgb(0xC9, 0xD0, 0xDB),

            text: rgb(0x13, 0x17, 0x21),
            text_weak: rgb(0x5B, 0x64, 0x72),
            text_muted: rgb(0x8A, 0x93, 0xA3),

            accent: rgb(0x0F, 0xA8, 0x5C),
            accent_hover: rgb(0x12, 0xB8, 0x66),
            accent_active: rgb(0x0C, 0x90, 0x4E),
            accent_fg: rgb(0xFF, 0xFF, 0xFF),

            success: rgb(0x16, 0xA3, 0x4A),
            warning: rgb(0xC2, 0x8A, 0x09),
            danger: rgb(0xDC, 0x49, 0x46),
            info: rgb(0x2B, 0x7F, 0xD4),

            selection_bg: rgb(0xC7, 0xEC, 0xD7),
            selection_stroke: rgb(0x0F, 0xA8, 0x5C),
        }
    }

    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::dark(),
            ThemeMode::Light => Self::light(),
            ThemeMode::System => Self::dark(), // caller should resolve System first
        }
    }
}

// ── Глобальная текущая палитра ───────────────────────────────────────────────

fn current() -> &'static RwLock<Palette> {
    static CURRENT: OnceLock<RwLock<Palette>> = OnceLock::new();
    CURRENT.get_or_init(|| RwLock::new(Palette::dark()))
}

/// Текущая активная палитра. Дёшево (Copy). Используется по всему UI.
pub fn palette() -> Palette {
    *current().read().unwrap_or_else(|e| e.into_inner())
}

/// Применить тему: настроить egui Visuals+Style и обновить глобальную палитру.
pub fn apply(ctx: &egui::Context, mode: ThemeMode) {
    let resolved = mode.resolved(ctx.system_theme());
    let p = Palette::for_mode(resolved);
    if let Ok(mut w) = current().write() {
        *w = p;
    }
    apply_palette(ctx, &p);
}

/// Сконфигурировать egui под конкретную палитру.
pub fn apply_palette(ctx: &egui::Context, p: &Palette) {
    use egui::{CornerRadius, Margin, Stroke};

    let mut v = if p.mode.is_dark() {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.extreme_bg_color = p.surface_sunken;
    v.faint_bg_color = p.surface_raised;
    v.window_stroke = Stroke::new(1.0, p.border);
    // НЕ ставим override_text_color: он перебивает «слабый» цвет, из-за чего
    // плейсхолдеры (hint_text) становятся невидимыми. Базовый цвет текста идёт
    // из widgets.noninteractive.fg_stroke ниже, weak/hint — производный от него.
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.danger;

    v.selection.bg_fill = p.selection_bg;
    v.selection.stroke = Stroke::new(1.0, p.selection_stroke);

    // Глубина: мягкие тени окон и всплывашек.
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: shadow_color(p, 0.35),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: shadow_color(p, 0.30),
    };

    let radius = CornerRadius::same(10);
    let radius_lg = CornerRadius::same(12);

    // noninteractive — карты, лейблы, контейнеры.
    // fg_stroke = основной цвет текста по умолчанию (ui.label без явного цвета).
    // egui выводит из него и weak_text_color (плейсхолдеры) — поэтому здесь
    // именно `text`, а не `text_weak`, иначе весь текст станет блеклым.
    v.widgets.noninteractive.bg_fill = p.surface;
    v.widgets.noninteractive.weak_bg_fill = p.surface;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.noninteractive.corner_radius = radius_lg;

    // inactive — кнопки/поля в покое
    v.widgets.inactive.bg_fill = p.surface_raised;
    v.widgets.inactive.weak_bg_fill = p.surface_sunken;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.inactive.corner_radius = radius;

    // hovered
    v.widgets.hovered.bg_fill = mix(p.surface_raised, p.accent, if p.mode.is_dark() { 0.10 } else { 0.06 });
    v.widgets.hovered.weak_bg_fill = p.surface_raised;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.border_strong);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.hovered.corner_radius = radius;
    v.widgets.hovered.expansion = 1.0;

    // active — нажато
    v.widgets.active.bg_fill = mix(p.surface_raised, p.accent, if p.mode.is_dark() { 0.18 } else { 0.12 });
    v.widgets.active.weak_bg_fill = p.surface_raised;
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.active.corner_radius = radius;

    // open — раскрытые меню/комбо
    v.widgets.open.bg_fill = p.surface_raised;
    v.widgets.open.weak_bg_fill = p.surface_raised;
    v.widgets.open.bg_stroke = Stroke::new(1.0, p.border_strong);
    v.widgets.open.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.open.corner_radius = radius;

    ctx.set_visuals(v);

    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(12.0, 9.0);
    style.spacing.button_padding = egui::vec2(16.0, 9.0);
    style.spacing.menu_margin = Margin::same(8);
    style.spacing.window_margin = Margin::same(16);
    style.spacing.interact_size.y = 34.0;
    ctx.set_global_style(style);
}

/// Цвет тени с учётом темы.
fn shadow_color(p: &Palette, alpha: f32) -> Color32 {
    let a = (alpha * 255.0) as u8;
    if p.mode.is_dark() {
        Color32::from_black_alpha(a)
    } else {
        // На светлой тени мягче и холоднее.
        Color32::from_rgba_unmultiplied(0x1A, 0x22, 0x30, a / 2)
    }
}

/// Линейная смесь двух цветов (t=0 → a, t=1 → b).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    Color32::from_rgb(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
    )
}

/// Полупрозрачный оттенок акцента — для бейджей/подложек.
pub fn accent_tint(p: &Palette, alpha: f32) -> Color32 {
    let a = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
    Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), a)
}

/// Полупрозрачный оттенок произвольного статус-цвета — для бейджей.
pub fn tint(c: Color32, alpha: f32) -> Color32 {
    let a = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

// =============================================================================
// EvertyGUI — design tokens (стандарт). Подробности: docs/EVERTYGUI.md
//
// ПРАВИЛО №1: никогда не хардкодить RGB в UI-коде. Всегда брать цвет из
// `palette()`, размер из `font`, отступ из `space`, радиус из `radius`.
// =============================================================================

/// Шкала размеров шрифта (pt). 8 ступеней — этого достаточно для всего UI.
pub mod font {
    pub const DISPLAY: f32 = 28.0; // крупный заголовок экрана / hero
    pub const H1: f32 = 22.0; // заголовок раздела
    pub const H2: f32 = 18.0; // заголовок карты
    pub const H3: f32 = 15.0; // подзаголовок / strong-лейбл
    pub const BODY: f32 = 14.0; // основной текст, кнопки
    pub const SMALL: f32 = 13.0; // вторичный текст
    pub const CAPTION: f32 = 12.0; // подписи, метки полей
    pub const MICRO: f32 = 11.0; // бейджи, счётчики
}

/// Шкала отступов (px). Кратна 2/4 — единый ритм по всему UI.
pub mod space {
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
}

/// Шкала радиусов скругления (px).
pub mod radius {
    pub const SM: u8 = 8; // мелкие элементы: бейджи, чипы
    pub const MD: u8 = 10; // кнопки, поля
    pub const LG: u8 = 12; // карты, секции
    pub const XL: u8 = 14; // крупные карты, окна
}

// ── Текстовые хелперы (типографика стандарта) ────────────────────────────────

use egui::RichText;

/// Заголовок раздела (H1, strong, основной текст).
pub fn h1(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(font::H1).strong().color(palette().text)
}

/// Заголовок карты (H2, strong).
pub fn h2(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(font::H2).strong().color(palette().text)
}

/// Подзаголовок / сильный лейбл (H3, strong).
pub fn h3(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(font::H3).strong().color(palette().text)
}

/// Основной текст.
pub fn body(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(font::BODY).color(palette().text)
}

/// Вторичный / приглушённый текст.
pub fn weak(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(font::SMALL).color(palette().text_weak)
}

/// Подпись / метка поля (CAPTION, muted).
pub fn caption(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(font::CAPTION).color(palette().text_muted)
}

// ── Компонент-хелперы ────────────────────────────────────────────────────────

/// Frame карты по стандарту EvertyGUI: surface + border + LG-радиус + LG-паддинг.
pub fn card() -> egui::Frame {
    let t = palette();
    egui::Frame::NONE
        .fill(t.surface)
        .stroke(egui::Stroke::new(1.0, t.border))
        .corner_radius(egui::CornerRadius::same(radius::XL))
        .inner_margin(egui::Margin::same(space::LG as i8))
}

/// Frame утопленной панели (поля ввода, контейнеры кода/лога).
pub fn sunken() -> egui::Frame {
    let t = palette();
    egui::Frame::NONE
        .fill(t.surface_sunken)
        .stroke(egui::Stroke::new(1.0, t.border))
        .corner_radius(egui::CornerRadius::same(radius::MD))
        .inner_margin(egui::Margin::same(space::MD as i8))
}

/// Нарисовать пилюлю-бейдж со статус-цветом (полупрозрачная подложка + текст).
/// Возвращает Response для hover/click.
pub fn badge(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(font::MICRO),
        color,
    );
    let pad = egui::vec2(space::SM, space::XS);
    let size = galley.size() + pad * 2.0;
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::same(radius::SM),
        tint(color, 0.16),
    );
    ui.painter().galley(rect.min + pad, galley, color);
    resp
}
