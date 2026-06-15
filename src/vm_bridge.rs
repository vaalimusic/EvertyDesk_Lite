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
    /// Последняя заметка о вводе (ошибка инжекта мыши/клавы) — для диагностики.
    input_note: String,
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
            input_note: String::new(),
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

/// Текущий статус VM-сессии (для отчёта клиенту). Включает заметку о вводе,
/// если был сбой инжекта (видно в логе клиента).
pub fn status() -> String {
    state()
        .lock()
        .map(|s| {
            if s.input_note.is_empty() {
                s.status.clone()
            } else {
                format!("{} | ввод: {}", s.status, s.input_note)
            }
        })
        .unwrap_or_default()
}

/// Записать заметку о вводе (ошибку/успех инжекта). Обновляет только при смене,
/// чтобы не молотить лог.
#[cfg(windows)]
fn note_input(msg: &str) {
    if let Ok(mut s) = state().lock() {
        if s.input_note != msg {
            s.input_note = msg.to_owned();
        }
    }
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

#[cfg(windows)]
const EVT_MOVE: i32 = 0;
#[cfg(windows)]
const EVT_DOWN: i32 = 1;
#[cfg(windows)]
const EVT_UP: i32 = 2;
#[cfg(windows)]
const EVT_WHEEL: i32 = 3;

#[cfg(windows)]
fn dispatch_mouse(vm_id: &str, ev: &crate::rustdesk_proto::MouseEvent, w: u32, h: u32) {
    let evt_type = ev.mask & 0x7;
    let button_bits = ev.mask >> 3;
    let res = match evt_type {
        EVT_MOVE | EVT_DOWN | EVT_UP => {
            // Координаты экрана VM → 0–65535 (SetAbsolutePosition).
            let nx = norm(ev.x, w);
            let ny = norm(ev.y, h);
            let mut r = hyperv::move_mouse(vm_id, nx, ny);
            if r.is_ok() && (evt_type == EVT_DOWN || evt_type == EVT_UP) {
                if let Some(btn) = vm_button(button_bits) {
                    r = hyperv::click_mouse(vm_id, btn, evt_type == EVT_DOWN);
                }
            }
            r
        }
        EVT_WHEEL => Ok(()), // Hyper-V WMI не имеет прямого wheel API — пропускаем
        _ => Ok(()),
    };
    match res {
        Ok(()) => note_input("мышь ок"),
        Err(e) => note_input(&format!("мышь: {e}")),
    }
}

#[cfg(windows)]
fn dispatch_key(vm_id: &str, ev: &crate::rustdesk_proto::KeyEvent) {
    use crate::rustdesk_proto::key_event::Union;
    // Модификаторы (Shift/Ctrl/Alt) — жмём перед, отпускаем после.
    let mods: Vec<u32> = ev
        .modifiers
        .iter()
        .filter_map(|m| control_key_scancode(*m))
        .collect();

    let res: Result<(), String> = (|| {
        match &ev.union {
            Some(Union::ControlKey(ck)) => {
                let scan = control_key_scancode(*ck)
                    .ok_or_else(|| format!("нет scancode для ControlKey={ck}"))?;
                if ev.press {
                    for m in &mods {
                        hyperv::press_key(vm_id, *m)?;
                    }
                    hyperv::press_key(vm_id, scan)?;
                    hyperv::release_key(vm_id, scan)?;
                    for m in mods.iter().rev() {
                        hyperv::release_key(vm_id, *m)?;
                    }
                } else if ev.down {
                    hyperv::press_key(vm_id, scan)?;
                } else {
                    hyperv::release_key(vm_id, scan)?;
                }
                Ok(())
            }
            Some(Union::Unicode(ch)) => {
                if (ev.press || ev.down) && *ch != 0 {
                    if let Some(c) = char::from_u32(*ch) {
                        // С модификаторами (Ctrl+C и т.п.) TypeText не годится —
                        // если есть мод-клавиши, шлём через scancode.
                        if !mods.is_empty() {
                            if let Some(scan) = ascii_scancode(c) {
                                for m in &mods {
                                    hyperv::press_key(vm_id, *m)?;
                                }
                                hyperv::press_key(vm_id, scan)?;
                                hyperv::release_key(vm_id, scan)?;
                                for m in mods.iter().rev() {
                                    hyperv::release_key(vm_id, *m)?;
                                }
                                return Ok(());
                            }
                        }
                        hyperv::type_text(vm_id, &c.to_string())?;
                    }
                }
                Ok(())
            }
            None => Ok(()),
        }
    })();

    match res {
        Ok(()) => note_input("клава ок"),
        Err(e) => note_input(&format!("клава: {e}")),
    }
}

/// Windows Virtual-Key код (VK_*) для управляющей клавиши.
/// ВАЖНО: `Msvm_Keyboard.PressKey` ожидает VK-код, НЕ PS/2 scancode.
/// `ck` — значение enum ControlKey из RustDesk message.proto (реальные номера).
#[cfg(windows)]
fn control_key_scancode(ck: i32) -> Option<u32> {
    Some(match ck {
        1 => 0x12,  // Alt        → VK_MENU
        2 => 0x08,  // Backspace  → VK_BACK
        3 => 0x14,  // CapsLock   → VK_CAPITAL
        4 => 0x11,  // Control    → VK_CONTROL
        5 => 0x2E,  // Delete     → VK_DELETE
        6 => 0x28,  // DownArrow  → VK_DOWN
        7 => 0x23,  // End        → VK_END
        8 => 0x1B,  // Escape     → VK_ESCAPE
        9 => 0x70,  // F1         → VK_F1
        10 => 0x79, // F10
        11 => 0x7A, // F11
        12 => 0x7B, // F12
        13 => 0x71, // F2
        14 => 0x72, // F3
        15 => 0x73, // F4
        16 => 0x74, // F5
        17 => 0x75, // F6
        18 => 0x76, // F7
        19 => 0x77, // F8
        20 => 0x78, // F9
        21 => 0x24, // Home       → VK_HOME
        22 => 0x25, // LeftArrow  → VK_LEFT
        23 => 0x5B, // Meta       → VK_LWIN
        25 => 0x22, // PageDown   → VK_NEXT
        26 => 0x21, // PageUp     → VK_PRIOR
        27 => 0x0D, // Return     → VK_RETURN
        28 => 0x27, // RightArrow → VK_RIGHT
        29 => 0x10, // Shift      → VK_SHIFT
        30 => 0x20, // Space      → VK_SPACE
        31 => 0x09, // Tab        → VK_TAB
        32 => 0x26, // UpArrow    → VK_UP
        58 => 0x2D, // Insert     → VK_INSERT
        72 => 0x0D, // NumpadEnter→ VK_RETURN
        _ => return None,
    })
}

/// ASCII-символ → Windows VK-код базовой клавиши (US-раскладка).
/// Нужно для сочетаний с модификаторами (Ctrl+C, Ctrl+V и т.п.).
/// Для букв VK = код заглавной ('A'=0x41), цифры VK = ASCII ('0'=0x30).
#[cfg(windows)]
fn ascii_scancode(c: char) -> Option<u32> {
    if c.is_ascii_alphabetic() {
        Some(c.to_ascii_uppercase() as u32)
    } else if c.is_ascii_digit() {
        Some(c as u32)
    } else if c == ' ' {
        Some(0x20)
    } else {
        None
    }
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

#[cfg(windows)]
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
