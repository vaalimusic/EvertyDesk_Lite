use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Color32, Event, ImageData, Key, Modifiers, Pos2, RawInput, Rect, TextureId,
    TexturesDelta, Vec2,
};
use minifb::{
    InputCallback, Key as MiniKey, KeyRepeat, MouseButton, MouseMode, Window, WindowOptions,
};

use crate::{configure_style, video, EvertyDeskApp, APP_NAME};

const WIDTH: usize = 1100;
const HEIGHT: usize = 760;

type Chars = Rc<RefCell<Vec<u32>>>;

struct CharInput {
    chars: Chars,
}

impl InputCallback for CharInput {
    fn add_char(&mut self, uni_char: u32) {
        self.chars.borrow_mut().push(uni_char);
    }
}

pub fn run_software_ui() -> Result<(), String> {
    std::env::set_var("EVERTYDESK_EGUI_SOFTWARE", "1");

    let mut window = Window::new(
        APP_NAME,
        WIDTH,
        HEIGHT,
        WindowOptions {
            resize: true,
            ..WindowOptions::default()
        },
    )
    .map_err(|err| format!("open CPU egui window failed: {err}"))?;
    window.set_target_fps(60);

    eprintln!(
        "[EvertyDesk] egui CPU software backend opened. Build codecs: {}",
        video::build_codec_label()
    );

    let chars = Chars::new(RefCell::new(Vec::new()));
    window.set_input_callback(Box::new(CharInput {
        chars: chars.clone(),
    }));

    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(1.0);
    configure_style(&ctx);

    let mut app = EvertyDeskApp::new();
    let mut painter = SoftwarePainter::default();
    let mut pixels = vec![0_u32; WIDTH * HEIGHT];
    let started = Instant::now();
    let mut pointer_buttons_down = [false; 3];

    while window.is_open() && !window.is_key_down(MiniKey::Escape) {
        let (width, height) = window.get_size();
        if pixels.len() != width * height {
            pixels.resize(width * height, 0);
        }

        let raw_input = collect_input(
            &window,
            &chars,
            width,
            height,
            started.elapsed(),
            &mut pointer_buttons_down,
        );

        let output = ctx.run(raw_input, |ctx| {
            app.update_egui(ctx);
        });
        let repaint_delay = output
            .viewport_output
            .values()
            .map(|viewport| viewport.repaint_delay)
            .min()
            .unwrap_or(Duration::from_millis(33));
        tune_frame_pacing(&mut window, repaint_delay);
        handle_platform_output(&output.platform_output);

        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        painter.apply_textures(output.textures_delta);
        pixels.fill(0x14181c);
        painter.paint(&mut pixels, width, height, &primitives);

        window
            .update_with_buffer(&pixels, width, height)
            .map_err(|err| format!("CPU egui window update failed: {err}"))?;
    }

    app.shutdown();
    eprintln!("[EvertyDesk] egui CPU software backend closed.");
    Ok(())
}

fn tune_frame_pacing(window: &mut Window, repaint_after: Duration) {
    let fps = if repaint_after <= Duration::from_millis(18) {
        60
    } else if repaint_after <= Duration::from_millis(40) {
        30
    } else if repaint_after <= Duration::from_millis(100) {
        15
    } else {
        8
    };
    window.set_target_fps(fps);
}

fn collect_input(
    window: &Window,
    chars: &Chars,
    width: usize,
    height: usize,
    elapsed: Duration,
    pointer_buttons_down: &mut [bool; 3],
) -> RawInput {
    let mut input = RawInput::default();
    input.screen_rect = Some(Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(width as f32, height as f32),
    ));
    input.time = Some(elapsed.as_secs_f64());
    input.modifiers = current_modifiers(window);

    if let Some((x, y)) = window.get_mouse_pos(MouseMode::Pass) {
        let pos = Pos2::new(x, y);
        input.events.push(Event::PointerMoved(pos));
        collect_pointer_button(
            window,
            &mut input,
            pos,
            pointer_buttons_down,
            0,
            MouseButton::Left,
            egui::PointerButton::Primary,
        );
        collect_pointer_button(
            window,
            &mut input,
            pos,
            pointer_buttons_down,
            1,
            MouseButton::Right,
            egui::PointerButton::Secondary,
        );
        collect_pointer_button(
            window,
            &mut input,
            pos,
            pointer_buttons_down,
            2,
            MouseButton::Middle,
            egui::PointerButton::Middle,
        );
    }

    if let Some((x, y)) = window.get_scroll_wheel() {
        if x != 0.0 || y != 0.0 {
            input
                .events
                .push(Event::Scroll(Vec2::new(x * 32.0, y * 32.0)));
        }
    }

    for key in window.get_keys_pressed(KeyRepeat::Yes) {
        if let Some(key) = map_key(key) {
            input.events.push(Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: input.modifiers,
            });
        }
    }
    for key in window.get_keys_released() {
        if let Some(key) = map_key(key) {
            input.events.push(Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: input.modifiers,
            });
        }
    }

    let mut typed = chars.borrow_mut();
    for ch in typed.drain(..) {
        if let Some(ch) = char::from_u32(ch) {
            if !ch.is_control() {
                input.events.push(Event::Text(ch.to_string()));
            }
        }
    }

    input
}

fn collect_pointer_button(
    window: &Window,
    input: &mut RawInput,
    pos: Pos2,
    pointer_buttons_down: &mut [bool; 3],
    index: usize,
    mini_button: MouseButton,
    egui_button: egui::PointerButton,
) {
    let down = window.get_mouse_down(mini_button);
    if down != pointer_buttons_down[index] {
        input.events.push(Event::PointerButton {
            pos,
            button: egui_button,
            pressed: down,
            modifiers: input.modifiers,
        });
        pointer_buttons_down[index] = down;
    }
}

fn current_modifiers(window: &Window) -> Modifiers {
    Modifiers {
        alt: window.is_key_down(MiniKey::LeftAlt) || window.is_key_down(MiniKey::RightAlt),
        ctrl: window.is_key_down(MiniKey::LeftCtrl) || window.is_key_down(MiniKey::RightCtrl),
        shift: window.is_key_down(MiniKey::LeftShift) || window.is_key_down(MiniKey::RightShift),
        mac_cmd: false,
        command: window.is_key_down(MiniKey::LeftCtrl) || window.is_key_down(MiniKey::RightCtrl),
    }
}

fn map_key(key: MiniKey) -> Option<Key> {
    Some(match key {
        MiniKey::Down => Key::ArrowDown,
        MiniKey::Left => Key::ArrowLeft,
        MiniKey::Right => Key::ArrowRight,
        MiniKey::Up => Key::ArrowUp,
        MiniKey::Escape => Key::Escape,
        MiniKey::Tab => Key::Tab,
        MiniKey::Backspace => Key::Backspace,
        MiniKey::Enter => Key::Enter,
        MiniKey::Space => Key::Space,
        MiniKey::Insert => Key::Insert,
        MiniKey::Delete => Key::Delete,
        MiniKey::Home => Key::Home,
        MiniKey::End => Key::End,
        MiniKey::PageUp => Key::PageUp,
        MiniKey::PageDown => Key::PageDown,
        MiniKey::Key0 => Key::Num0,
        MiniKey::Key1 => Key::Num1,
        MiniKey::Key2 => Key::Num2,
        MiniKey::Key3 => Key::Num3,
        MiniKey::Key4 => Key::Num4,
        MiniKey::Key5 => Key::Num5,
        MiniKey::Key6 => Key::Num6,
        MiniKey::Key7 => Key::Num7,
        MiniKey::Key8 => Key::Num8,
        MiniKey::Key9 => Key::Num9,
        MiniKey::A => Key::A,
        MiniKey::B => Key::B,
        MiniKey::C => Key::C,
        MiniKey::D => Key::D,
        MiniKey::E => Key::E,
        MiniKey::F => Key::F,
        MiniKey::G => Key::G,
        MiniKey::H => Key::H,
        MiniKey::I => Key::I,
        MiniKey::J => Key::J,
        MiniKey::K => Key::K,
        MiniKey::L => Key::L,
        MiniKey::M => Key::M,
        MiniKey::N => Key::N,
        MiniKey::O => Key::O,
        MiniKey::P => Key::P,
        MiniKey::Q => Key::Q,
        MiniKey::R => Key::R,
        MiniKey::S => Key::S,
        MiniKey::T => Key::T,
        MiniKey::U => Key::U,
        MiniKey::V => Key::V,
        MiniKey::W => Key::W,
        MiniKey::X => Key::X,
        MiniKey::Y => Key::Y,
        MiniKey::Z => Key::Z,
        MiniKey::F1 => Key::F1,
        MiniKey::F2 => Key::F2,
        MiniKey::F3 => Key::F3,
        MiniKey::F4 => Key::F4,
        MiniKey::F5 => Key::F5,
        MiniKey::F6 => Key::F6,
        MiniKey::F7 => Key::F7,
        MiniKey::F8 => Key::F8,
        MiniKey::F9 => Key::F9,
        MiniKey::F10 => Key::F10,
        MiniKey::F11 => Key::F11,
        MiniKey::F12 => Key::F12,
        _ => return None,
    })
}

fn handle_platform_output(output: &egui::PlatformOutput) {
    if !output.copied_text.is_empty() {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(output.copied_text.clone());
        }
    }
}

#[derive(Default)]
struct SoftwarePainter {
    textures: HashMap<TextureId, Texture>,
}

struct Texture {
    width: usize,
    height: usize,
    pixels: Vec<Color32>,
    rgb: Vec<u32>,
    opaque: bool,
}

impl SoftwarePainter {
    fn apply_textures(&mut self, delta: TexturesDelta) {
        for (id, image_delta) in delta.set {
            let image = image_delta.image;
            let (size, pixels) = match image {
                ImageData::Color(image) => (image.size, image.pixels.clone()),
                ImageData::Font(image) => {
                    let pixels: Vec<Color32> = image.srgba_pixels(None).collect();
                    (image.size, pixels)
                }
            };
            if let Some(pos) = image_delta.pos {
                if let Some(texture) = self.textures.get_mut(&id) {
                    patch_texture(texture, pos, size, &pixels);
                    continue;
                }
            }
            self.textures
                .insert(id, Texture::new(size[0], size[1], pixels));
        }
        for id in delta.free {
            self.textures.remove(&id);
        }
    }

    fn paint(
        &self,
        buffer: &mut [u32],
        width: usize,
        height: usize,
        primitives: &[egui::ClippedPrimitive],
    ) {
        for primitive in primitives {
            let clip = primitive.clip_rect;
            let clip_min_x = clip.min.x.floor().max(0.0) as i32;
            let clip_min_y = clip.min.y.floor().max(0.0) as i32;
            let clip_max_x = clip.max.x.ceil().min(width as f32) as i32;
            let clip_max_y = clip.max.y.ceil().min(height as f32) as i32;
            if clip_min_x >= clip_max_x || clip_min_y >= clip_max_y {
                continue;
            }
            if let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive {
                let texture = self.textures.get(&mesh.texture_id);
                let mut index = 0;
                while index < mesh.indices.len() {
                    if index + 6 <= mesh.indices.len()
                        && draw_textured_quad_fast(
                            buffer, width, height, texture, clip_min_x, clip_min_y, clip_max_x,
                            clip_max_y, mesh, index,
                        )
                    {
                        index += 6;
                        continue;
                    }
                    if index + 3 <= mesh.indices.len() {
                        let triangle = &mesh.indices[index..index + 3];
                        let v0 = mesh.vertices[triangle[0] as usize];
                        let v1 = mesh.vertices[triangle[1] as usize];
                        let v2 = mesh.vertices[triangle[2] as usize];
                        draw_triangle(
                            buffer, width, height, texture, clip_min_x, clip_min_y, clip_max_x,
                            clip_max_y, v0, v1, v2,
                        );
                    }
                    index += 3;
                }
            }
        }
    }
}

fn patch_texture(texture: &mut Texture, pos: [usize; 2], size: [usize; 2], pixels: &[Color32]) {
    for y in 0..size[1] {
        for x in 0..size[0] {
            let dst_x = pos[0] + x;
            let dst_y = pos[1] + y;
            if dst_x < texture.width && dst_y < texture.height {
                let src = pixels[y * size[0] + x];
                let index = dst_y * texture.width + dst_x;
                texture.pixels[index] = src;
                texture.rgb[index] = color_to_rgb(src);
                if src.a() != 255 {
                    texture.opaque = false;
                }
            }
        }
    }
}

impl Texture {
    fn new(width: usize, height: usize, pixels: Vec<Color32>) -> Self {
        let opaque = pixels.iter().all(|pixel| pixel.a() == 255);
        let rgb = pixels.iter().copied().map(color_to_rgb).collect();
        Self {
            width,
            height,
            pixels,
            rgb,
            opaque,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_textured_quad_fast(
    buffer: &mut [u32],
    width: usize,
    height: usize,
    texture: Option<&Texture>,
    clip_min_x: i32,
    clip_min_y: i32,
    clip_max_x: i32,
    clip_max_y: i32,
    mesh: &egui::epaint::Mesh,
    index: usize,
) -> bool {
    let Some(texture) = texture else {
        return false;
    };
    if texture.width == 0 || texture.height == 0 {
        return false;
    }

    let idx = &mesh.indices[index..index + 6];
    let verts = [
        mesh.vertices[idx[0] as usize],
        mesh.vertices[idx[1] as usize],
        mesh.vertices[idx[2] as usize],
        mesh.vertices[idx[3] as usize],
        mesh.vertices[idx[4] as usize],
        mesh.vertices[idx[5] as usize],
    ];
    if !verts.iter().all(|v| v.color == Color32::WHITE) {
        return false;
    }

    let min_x = verts.iter().map(|v| v.pos.x).fold(f32::INFINITY, f32::min);
    let max_x = verts
        .iter()
        .map(|v| v.pos.x)
        .fold(f32::NEG_INFINITY, f32::max);
    let min_y = verts.iter().map(|v| v.pos.y).fold(f32::INFINITY, f32::min);
    let max_y = verts
        .iter()
        .map(|v| v.pos.y)
        .fold(f32::NEG_INFINITY, f32::max);
    if max_x - min_x < 1.0 || max_y - min_y < 1.0 {
        return false;
    }

    let mut tl = None;
    let mut tr = None;
    let mut bl = None;
    let mut br = None;
    for vertex in &verts {
        let x_min = nearly_equal(vertex.pos.x, min_x);
        let x_max = nearly_equal(vertex.pos.x, max_x);
        let y_min = nearly_equal(vertex.pos.y, min_y);
        let y_max = nearly_equal(vertex.pos.y, max_y);
        match (x_min, x_max, y_min, y_max) {
            (true, false, true, false) => tl = Some(vertex.uv),
            (false, true, true, false) => tr = Some(vertex.uv),
            (true, false, false, true) => bl = Some(vertex.uv),
            (false, true, false, true) => br = Some(vertex.uv),
            _ => return false,
        }
    }

    let (Some(tl), Some(tr), Some(bl), Some(br)) = (tl, tr, bl, br) else {
        return false;
    };
    if !nearly_equal(tl.y, tr.y)
        || !nearly_equal(bl.y, br.y)
        || !nearly_equal(tl.x, bl.x)
        || !nearly_equal(tr.x, br.x)
    {
        return false;
    }

    let dst_min_x = (min_x.floor() as i32).max(clip_min_x).max(0);
    let dst_max_x = (max_x.ceil() as i32).min(clip_max_x).min(width as i32);
    let dst_min_y = (min_y.floor() as i32).max(clip_min_y).max(0);
    let dst_max_y = (max_y.ceil() as i32).min(clip_max_y).min(height as i32);
    if dst_min_x >= dst_max_x || dst_min_y >= dst_max_y {
        return true;
    }

    let dst_w = (max_x - min_x).max(1.0);
    let dst_h = (max_y - min_y).max(1.0);
    let u0 = tl.x;
    let u1 = tr.x;
    let v0 = tl.y;
    let v1 = bl.y;

    for y in dst_min_y..dst_max_y {
        let fy = ((y as f32 + 0.5 - min_y) / dst_h).clamp(0.0, 1.0);
        let tex_y = ((v0 + (v1 - v0) * fy) * texture.height as f32)
            .floor()
            .clamp(0.0, (texture.height - 1) as f32) as usize;
        let dst_row = y as usize * width;
        let tex_row = tex_y * texture.width;
        for x in dst_min_x..dst_max_x {
            let fx = ((x as f32 + 0.5 - min_x) / dst_w).clamp(0.0, 1.0);
            let tex_x = ((u0 + (u1 - u0) * fx) * texture.width as f32)
                .floor()
                .clamp(0.0, (texture.width - 1) as f32) as usize;
            let idx = dst_row + x as usize;
            let tex_idx = tex_row + tex_x;
            if texture.opaque {
                buffer[idx] = texture.rgb[tex_idx];
            } else {
                buffer[idx] = blend_over(buffer[idx], texture.pixels[tex_idx]);
            }
        }
    }
    true
}

fn nearly_equal(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.5
}

#[allow(clippy::too_many_arguments)]
fn draw_triangle(
    buffer: &mut [u32],
    width: usize,
    height: usize,
    texture: Option<&Texture>,
    clip_min_x: i32,
    clip_min_y: i32,
    clip_max_x: i32,
    clip_max_y: i32,
    v0: egui::epaint::Vertex,
    v1: egui::epaint::Vertex,
    v2: egui::epaint::Vertex,
) {
    let min_x = v0
        .pos
        .x
        .min(v1.pos.x)
        .min(v2.pos.x)
        .floor()
        .max(clip_min_x as f32)
        .max(0.0) as i32;
    let max_x = v0
        .pos
        .x
        .max(v1.pos.x)
        .max(v2.pos.x)
        .ceil()
        .min(clip_max_x as f32)
        .min(width as f32) as i32;
    let min_y = v0
        .pos
        .y
        .min(v1.pos.y)
        .min(v2.pos.y)
        .floor()
        .max(clip_min_y as f32)
        .max(0.0) as i32;
    let max_y = v0
        .pos
        .y
        .max(v1.pos.y)
        .max(v2.pos.y)
        .ceil()
        .min(clip_max_y as f32)
        .min(height as f32) as i32;
    if min_x >= max_x || min_y >= max_y {
        return;
    }

    let area = edge(v0.pos, v1.pos, v2.pos);
    if area.abs() < f32::EPSILON {
        return;
    }

    for y in min_y..max_y {
        for x in min_x..max_x {
            let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge(v1.pos, v2.pos, p) / area;
            let w1 = edge(v2.pos, v0.pos, p) / area;
            let w2 = edge(v0.pos, v1.pos, p) / area;
            if w0 < -0.001 || w1 < -0.001 || w2 < -0.001 {
                continue;
            }

            let vertex_color = mix_color(v0.color, v1.color, v2.color, w0, w1, w2);
            let tex_color = if let Some(texture) = texture {
                let u = v0.uv.x * w0 + v1.uv.x * w1 + v2.uv.x * w2;
                let v = v0.uv.y * w0 + v1.uv.y * w1 + v2.uv.y * w2;
                sample_texture(texture, u, v)
            } else {
                Color32::WHITE
            };
            let color = multiply_color(vertex_color, tex_color);
            let idx = y as usize * width + x as usize;
            buffer[idx] = blend_over(buffer[idx], color);
        }
    }
}

fn edge(a: Pos2, b: Pos2, c: Pos2) -> f32 {
    (c.x - a.x) * (b.y - a.y) - (c.y - a.y) * (b.x - a.x)
}

fn mix_color(c0: Color32, c1: Color32, c2: Color32, w0: f32, w1: f32, w2: f32) -> Color32 {
    let r = c0.r() as f32 * w0 + c1.r() as f32 * w1 + c2.r() as f32 * w2;
    let g = c0.g() as f32 * w0 + c1.g() as f32 * w1 + c2.g() as f32 * w2;
    let b = c0.b() as f32 * w0 + c1.b() as f32 * w1 + c2.b() as f32 * w2;
    let a = c0.a() as f32 * w0 + c1.a() as f32 * w1 + c2.a() as f32 * w2;
    Color32::from_rgba_unmultiplied(r as u8, g as u8, b as u8, a as u8)
}

fn sample_texture(texture: &Texture, u: f32, v: f32) -> Color32 {
    if texture.width == 0 || texture.height == 0 {
        return Color32::WHITE;
    }
    let x = (u * texture.width as f32)
        .floor()
        .clamp(0.0, (texture.width - 1) as f32) as usize;
    let y = (v * texture.height as f32)
        .floor()
        .clamp(0.0, (texture.height - 1) as f32) as usize;
    texture.pixels[y * texture.width + x]
}

fn multiply_color(a: Color32, b: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        ((a.r() as u16 * b.r() as u16) / 255) as u8,
        ((a.g() as u16 * b.g() as u16) / 255) as u8,
        ((a.b() as u16 * b.b() as u16) / 255) as u8,
        ((a.a() as u16 * b.a() as u16) / 255) as u8,
    )
}

fn color_to_rgb(color: Color32) -> u32 {
    ((color.r() as u32) << 16) | ((color.g() as u32) << 8) | color.b() as u32
}

fn blend_over(dst: u32, src: Color32) -> u32 {
    let sa = src.a() as u32;
    if sa == 255 {
        return color_to_rgb(src);
    }
    if sa == 0 {
        return dst;
    }
    let dr = (dst >> 16) & 0xff;
    let dg = (dst >> 8) & 0xff;
    let db = dst & 0xff;
    let inv = 255 - sa;
    let r = (src.r() as u32 * sa + dr * inv) / 255;
    let g = (src.g() as u32 * sa + dg * inv) / 255;
    let b = (src.b() as u32 * sa + db * inv) / 255;
    (r << 16) | (g << 8) | b
}
