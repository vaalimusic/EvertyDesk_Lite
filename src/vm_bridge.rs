//! Agentless VM bridge — «киллер-фича» EvertyDesk Lite.
//!
//! Позволяет удалённому клиенту, подключённому к хосту-гипервизору, видеть и
//! управлять виртуальными машинами **без установки агента в гостя**. Доступ к
//! консоли VM идёт через API гипервизора (Hyper-V WMI: `src/hyperv.rs`).
//!
//! Принцип работы (минимально инвазивный):
//!  • Когда клиент «прикрепляется» к VM, здесь поднимается захват консоли VM.
//!  • `video_pipeline` на каждом кадре спрашивает [`active_frame`] — если VM
//!    активна, кодируется кадр VM вместо физического экрана хоста.
//!  • Ввод (мышь/клавиатура) от клиента сначала отдаётся в [`route_mouse`] /
//!    [`route_key`]; если VM активна — событие уходит в VM, а не в ОС хоста.
//!
//! Так клиент не требует изменений видеотракта: он просто «видит экран» и
//! «двигает мышь», а хост прозрачно подменяет цель.

use std::sync::{Mutex, OnceLock};

#[cfg(windows)]
use crate::hyperv;

// ── Общее состояние активной VM-сессии ──────────────────────────────────────

struct Shared {
    /// Монотонная «эпоха»: каждый attach/detach инкрементит. Pump-поток
    /// сравнивает свою эпоху и завершается, если она устарела.
    generation: u64,
    /// id прикреплённой VM (None = транслируем физический экран хоста).
    attached_id: Option<String>,
    width: u32,
    height: u32,
    /// Последний кадр VM в формате BGRA (как ждёт энкодер pipeline).
    bgra: Vec<u8>,
    have_frame: bool,
    status: String,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            generation: 0,
            attached_id: None,
            width: 0,
            height: 0,
            bgra: Vec::new(),
            have_frame: false,
            status: String::new(),
        }
    }
}

fn state() -> &'static Mutex<Shared> {
    static STATE: OnceLock<Mutex<Shared>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(Shared::default()))
}

// ── Публичное API ────────────────────────────────────────────────────────────

/// Активна ли сейчас VM-сессия.
pub fn is_attached() -> bool {
    state()
        .lock()
        .map(|s| s.attached_id.is_some())
        .unwrap_or(false)
}

/// Текущий статус VM-сессии (для отчёта клиенту).
pub fn status() -> String {
    state().lock().map(|s| s.status.clone()).unwrap_or_default()
}

/// JSON-список VM гипервизора: `[{"id","name","state","connectable"}]`.
pub fn list_json() -> String {
    let vms = list_vms_entries();
    let items: Vec<String> = vms
        .iter()
        .map(|v| {
            format!(
                "{{\"id\":{},\"name\":{},\"state\":{},\"connectable\":{}}}",
                json_str(&v.id),
                json_str(&v.name),
                json_str(&v.state),
                v.connectable
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// Прикрепиться к VM по id (пустая строка = отсоединиться). Возвращает
/// человекочитаемый статус.
pub fn attach(vm_id: &str) -> Result<String, String> {
    if vm_id.trim().is_empty() {
        detach();
        return Ok("Отсоединено — снова физический экран хоста".to_owned());
    }
    attach_impl(vm_id)
}

/// Отсоединиться: вернуть трансляцию физического экрана хоста.
pub fn detach() {
    if let Ok(mut s) = state().lock() {
        if s.attached_id.is_some() {
            s.generation = s.generation.wrapping_add(1);
            s.attached_id = None;
            s.have_frame = false;
            s.bgra = Vec::new();
            s.width = 0;
            s.height = 0;
            s.status = "Отсоединено".to_owned();
        }
    }
}

/// Если VM активна и есть кадр — копирует BGRA-кадр в `out` и возвращает
/// `(width, height)`. Иначе `None` (pipeline захватывает физический экран).
pub fn active_frame(out: &mut Vec<u8>) -> Option<(u32, u32)> {
    let s = state().lock().ok()?;
    if s.attached_id.is_none() || !s.have_frame || s.bgra.is_empty() {
        return None;
    }
    out.clear();
    out.extend_from_slice(&s.bgra);
    Some((s.width, s.height))
}

/// Роутинг мыши: если VM активна — отправляет в VM и возвращает `true`
/// (хост НЕ инжектит событие в свою ОС).
pub fn route_mouse(ev: &crate::rustdesk_proto::MouseEvent) -> bool {
    let (id, w, h) = {
        let s = match state().lock() {
            Ok(s) => s,
            Err(_) => return false,
        };
        match &s.attached_id {
            Some(id) => (id.clone(), s.width.max(1), s.height.max(1)),
            None => return false,
        }
    };
    dispatch_mouse(&id, ev, w, h);
    true
}

/// Роутинг клавиатуры: если VM активна — отправляет в VM, возвращает `true`.
pub fn route_key(ev: &crate::rustdesk_proto::KeyEvent) -> bool {
    let id = {
        let s = match state().lock() {
            Ok(s) => s,
            Err(_) => return false,
        };
        match &s.attached_id {
            Some(id) => id.clone(),
            None => return false,
        }
    };
    dispatch_key(&id, ev);
    true
}

// ── Внутренняя VM-запись (кросс-платформенная) ───────────────────────────────

pub struct VmEntry {
    pub id: String,
    pub name: String,
    pub state: String,
    pub connectable: bool,
}

#[cfg(windows)]
fn list_vms_entries() -> Vec<VmEntry> {
    hyperv::list_vms()
        .into_iter()
        .map(|v| VmEntry {
            id: v.id,
            name: v.name,
            state: v.state.label().to_owned(),
            connectable: v.state.is_connectable(),
        })
        .collect()
}

#[cfg(not(windows))]
fn list_vms_entries() -> Vec<VmEntry> {
    Vec::new()
}

// ── Windows: реальный attach через Hyper-V ───────────────────────────────────

#[cfg(windows)]
fn attach_impl(vm_id: &str) -> Result<String, String> {
    let vm = hyperv::list_vms()
        .into_iter()
        .find(|v| v.id == vm_id)
        .ok_or_else(|| format!("VM {vm_id} не найдена на гипервизоре"))?;

    if !vm.state.is_connectable() {
        return Err(format!(
            "VM «{}» в состоянии {} — нельзя подключиться",
            vm.name,
            vm.state.label()
        ));
    }

    let gen = {
        let mut s = state().lock().map_err(|_| "lock".to_owned())?;
        s.generation = s.generation.wrapping_add(1);
        s.attached_id = Some(vm.id.clone());
        s.have_frame = false;
        s.status = format!("Подключение к VM «{}»…", vm.name);
        s.generation
    };

    let vm_name = vm.name.clone();
    std::thread::Builder::new()
        .name("vm-bridge-pump".into())
        .spawn(move || pump_loop(vm, gen))
        .map_err(|e| format!("spawn pump: {e}"))?;

    Ok(format!("Подключение к VM «{vm_name}»…"))
}

#[cfg(windows)]
fn pump_loop(vm: hyperv::VmInfo, gen: u64) {
    let session = hyperv::HyperVSession::start(vm);
    loop {
        // Эпоха устарела (другой attach/detach) → гасим сессию и выходим.
        let current_gen = state().lock().map(|s| s.generation).unwrap_or(gen + 1);
        if current_gen != gen {
            session.stop();
            return;
        }
        while let Some(msg) = session.try_recv_status() {
            if let Ok(mut s) = state().lock() {
                if s.generation == gen {
                    s.status = msg;
                }
            }
        }
        if let Some(frame) = session.try_recv_frame() {
            let bgra = rgba_to_bgra(&frame.rgba);
            if let Ok(mut s) = state().lock() {
                if s.generation == gen {
                    s.width = frame.width;
                    s.height = frame.height;
                    s.bgra = bgra;
                    s.have_frame = true;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}

#[cfg(not(windows))]
fn attach_impl(_vm_id: &str) -> Result<String, String> {
    Err("Agentless-доступ к VM доступен только на Windows-хосте (Hyper-V)".to_owned())
}

// ── Диспетчеризация ввода в VM ───────────────────────────────────────────────

const EVT_MOVE: i32 = 0;
const EVT_DOWN: i32 = 1;
const EVT_UP: i32 = 2;
const EVT_WHEEL: i32 = 3;

#[cfg(windows)]
fn dispatch_mouse(vm_id: &str, ev: &crate::rustdesk_proto::MouseEvent, w: u32, h: u32) {
    let evt_type = ev.mask & 0x7;
    let button_bits = ev.mask >> 3;
    match evt_type {
        EVT_MOVE | EVT_DOWN | EVT_UP => {
            // Координаты экрана VM → 0–65535 (SetAbsolutePosition).
            let nx = norm(ev.x, w);
            let ny = norm(ev.y, h);
            hyperv::move_mouse(vm_id, nx, ny);
            if evt_type == EVT_DOWN || evt_type == EVT_UP {
                if let Some(btn) = vm_button(button_bits) {
                    hyperv::click_mouse(vm_id, btn, evt_type == EVT_DOWN);
                }
            }
        }
        EVT_WHEEL => { /* Hyper-V WMI не имеет прямого wheel API — пропускаем */ }
        _ => {}
    }
}

#[cfg(windows)]
fn dispatch_key(vm_id: &str, ev: &crate::rustdesk_proto::KeyEvent) {
    use crate::rustdesk_proto::key_event::Union;
    match &ev.union {
        Some(Union::ControlKey(ck)) => {
            if let Some(scan) = control_key_scancode(*ck) {
                if ev.press {
                    hyperv::press_key(vm_id, scan);
                    hyperv::release_key(vm_id, scan);
                } else if ev.down {
                    hyperv::press_key(vm_id, scan);
                } else {
                    hyperv::release_key(vm_id, scan);
                }
            }
        }
        Some(Union::Unicode(ch)) => {
            // Печатаем символ только на нажатие (TypeText сам жмёт/отпускает).
            if (ev.press || ev.down) && *ch != 0 {
                if let Some(c) = char::from_u32(*ch) {
                    hyperv::type_text(vm_id, &c.to_string());
                }
            }
        }
        None => {}
    }
}

/// PS/2 set-1 scan-коды для частых управляющих клавиш. ControlKey — enum
/// RustDesk (i32). Возвращаем None для незамапленных (их можно добавлять).
#[cfg(windows)]
fn control_key_scancode(ck: i32) -> Option<u32> {
    // Значения ControlKey из rustdesk_proto (RustDesk message.proto).
    Some(match ck {
        3 => 0x1C,  // Return / Enter
        4 => 0x01,  // Escape
        5 => 0x0E,  // Backspace
        6 => 0x0F,  // Tab
        7 => 0x39,  // Space
        24 => 0x48, // UpArrow
        25 => 0x50, // DownArrow
        26 => 0x4B, // LeftArrow
        27 => 0x4D, // RightArrow
        29 => 0x53, // Delete
        30 => 0x47, // Home
        31 => 0x4F, // End
        _ => return None,
    })
}

#[cfg(not(windows))]
fn dispatch_mouse(_vm_id: &str, _ev: &crate::rustdesk_proto::MouseEvent, _w: u32, _h: u32) {}

#[cfg(not(windows))]
fn dispatch_key(_vm_id: &str, _ev: &crate::rustdesk_proto::KeyEvent) {}

// ── helpers ──────────────────────────────────────────────────────────────────

#[cfg(windows)]
fn vm_button(bits: i32) -> Option<u32> {
    // RustDesk button bits после >>3: 1=left, 2=right, иначе middle.
    match bits {
        1 => Some(1),
        2 => Some(2),
        0 => None,
        _ => Some(3),
    }
}

#[cfg(windows)]
fn norm(v: i32, span: u32) -> u32 {
    let span = span.max(1) as i64;
    let v = v.max(0) as i64;
    ((v * 65535) / span).clamp(0, 65535) as u32
}

fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        out.push(px[2]); // B
        out.push(px[1]); // G
        out.push(px[0]); // R
        out.push(px[3]); // A
    }
    out
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
