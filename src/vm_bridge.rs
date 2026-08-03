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
    /// Для Hyper-V — GUID, для VirtualBox — UUID (без префикса провайдера).
    attached_id: Option<String>,
    /// Провайдер активной VM: "hyperv" | "vbox".
    provider: String,
    width: u32,
    height: u32,
    /// Последний кадр VM в формате BGRA (как ждёт энкодер pipeline).
    bgra: Vec<u8>,
    have_frame: bool,
    /// Монотонный счётчик кадров VM: инкрементируется каждый раз, когда WMI
    /// прислал НОВЫЙ кадр. Video pipeline сравнивает с last_vm_seq и если
    /// изменился — немедленно отправляет (обходит change_detector).
    /// Решает проблему: символы в терминале занимают < 0.1% пикселей,
    /// ниже порога dirty_area_milli=80 → раньше никогда не отправлялись
    /// раньше IDR-таймера (1200ms).
    frame_seq: u64,
    status: String,
    /// Последняя заметка о вводе (ошибка инжекта мыши/клавы) — для диагностики.
    input_note: String,
    /// Канал команд в HyperVSession (мышь/клавиатура без overhead нового WMI connect).
    #[cfg(windows)]
    hyperv_cmd_tx: Option<std::sync::mpsc::Sender<crate::hyperv::HyperVCmd>>,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            generation: 0,
            attached_id: None,
            provider: String::new(),
            width: 0,
            height: 0,
            bgra: Vec::new(),
            have_frame: false,
            frame_seq: 0,
            status: String::new(),
            input_note: String::new(),
            #[cfg(windows)]
            hyperv_cmd_tx: None,
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
/// человекочитаемый статус. id в формате "provider:realid"
/// (provider: "hyperv" | "vbox"); без префикса — считаем hyperv.
pub fn attach(vm_id: &str) -> Result<String, String> {
    if vm_id.trim().is_empty() {
        detach();
        return Ok("Отсоединено — снова физический экран хоста".to_owned());
    }
    let (provider, real_id) = vm_id.split_once(':').unwrap_or(("hyperv", vm_id));
    match provider {
        "vbox" => attach_vbox(real_id),
        "hyperv" => attach_hyperv(real_id),
        other => Err(format!("неизвестный провайдер VM: {other}")),
    }
}

/// Отсоединиться: вернуть трансляцию физического экрана хоста.
pub fn detach() {
    if let Ok(mut s) = state().lock() {
        if s.attached_id.is_some() {
            s.generation = s.generation.wrapping_add(1);
            s.attached_id = None;
            s.provider = String::new();
            s.have_frame = false;
            s.bgra = Vec::new();
            s.width = 0;
            s.height = 0;
            s.status = "Отсоединено".to_owned();
            s.input_note = String::new();
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

/// Возвращает монотонный счётчик WMI-кадров VM.
///
/// Используется в video_pipeline для bypass change_detector:
/// если seq изменился с момента последнего вызова — WMI прислал НОВЫЙ кадр
/// → форсируем отправку, не ждём IDR-таймера (1200ms).
///
/// Возвращает `None` если VM не присоединена.
pub fn vm_frame_seq() -> Option<u64> {
    let s = state().lock().ok()?;
    if s.attached_id.is_none() || !s.have_frame {
        return None;
    }
    Some(s.frame_seq)
}

/// Роутинг мыши: если VM активна — отправляет в VM и возвращает `true`
/// (хост НЕ инжектит событие в свою ОС).
pub fn route_mouse(ev: &crate::rustdesk_proto::MouseEvent) -> bool {
    let (id, provider, w, h) = {
        let s = match state().lock() {
            Ok(s) => s,
            Err(_) => return false,
        };
        match &s.attached_id {
            Some(id) => (
                id.clone(),
                s.provider.clone(),
                s.width.max(1),
                s.height.max(1),
            ),
            None => return false,
        }
    };
    match provider.as_str() {
        "vbox" => {
            // VBoxManage не имеет публичного mouse API → мышь пока не поддержана.
            note_input("мышь VirtualBox: нет CLI API (TODO: VRDP)");
        }
        _ => dispatch_mouse(&id, ev, w, h),
    }
    true
}

/// Роутинг клавиатуры: если VM активна — отправляет в VM, возвращает `true`.
pub fn route_key(ev: &crate::rustdesk_proto::KeyEvent) -> bool {
    let (id, provider) = {
        let s = match state().lock() {
            Ok(s) => s,
            Err(_) => return false,
        };
        match &s.attached_id {
            Some(id) => (id.clone(), s.provider.clone()),
            None => return false,
        }
    };
    match provider.as_str() {
        "vbox" => dispatch_key_vbox(&id, ev),
        _ => dispatch_key(&id, ev),
    }
    true
}

// ── Внутренняя VM-запись (кросс-платформенная) ───────────────────────────────

pub struct VmEntry {
    pub id: String,
    pub name: String,
    pub state: String,
    pub connectable: bool,
}

/// Агрегированный список VM всех провайдеров. id с префиксом провайдера
/// ("hyperv:GUID" / "vbox:UUID"), чтобы attach знал куда роутить.
fn list_vms_entries() -> Vec<VmEntry> {
    let mut out = Vec::new();

    #[cfg(windows)]
    for v in hyperv::list_vms() {
        out.push(VmEntry {
            id: format!("hyperv:{}", v.id),
            name: v.name,
            state: v.state.label().to_owned(),
            connectable: v.state.is_connectable(),
        });
    }

    for v in crate::virtualbox::list_vms() {
        out.push(VmEntry {
            id: format!("vbox:{}", v.id),
            name: format!("{} · VirtualBox", v.name),
            state: if v.running { "Running" } else { "Off" }.to_owned(),
            connectable: v.running,
        });
    }

    out
}

// ── Общая установка активной VM ──────────────────────────────────────────────

/// Зафиксировать новую активную VM, вернуть её «эпоху» для pump-потока.
fn begin_attach(provider: &str, id: &str, name: &str) -> Result<u64, String> {
    let mut s = state().lock().map_err(|_| "lock".to_owned())?;
    s.generation = s.generation.wrapping_add(1);
    s.attached_id = Some(id.to_owned());
    s.provider = provider.to_owned();
    s.have_frame = false;
    s.input_note = String::new();
    s.status = format!("Подключение к VM «{name}»…");
    Ok(s.generation)
}

/// Обновить кадр активной VM (если эпоха ещё актуальна).
/// Инкрементирует frame_seq — video pipeline использует его для bypass change_detector.
fn push_frame(gen: u64, w: u32, h: u32, bgra: Vec<u8>) {
    if let Ok(mut s) = state().lock() {
        if s.generation == gen {
            s.width = w;
            s.height = h;
            s.bgra = bgra;
            s.have_frame = true;
            s.frame_seq = s.frame_seq.wrapping_add(1);
        }
    }
}

fn gen_is_current(gen: u64) -> bool {
    state().lock().map(|s| s.generation == gen).unwrap_or(false)
}

// ── VirtualBox attach (кросс-платформенно, через VBoxManage) ──────────────────

fn attach_vbox(uuid: &str) -> Result<String, String> {
    let vm = crate::virtualbox::list_vms()
        .into_iter()
        .find(|v| v.id == uuid)
        .ok_or_else(|| format!("VirtualBox VM {uuid} не найдена"))?;
    if !vm.running {
        return Err(format!(
            "VM «{}» выключена — запустите её в VirtualBox",
            vm.name
        ));
    }
    let gen = begin_attach("vbox", &vm.id, &vm.name)?;
    let name = vm.name.clone();
    let id = vm.id.clone();
    std::thread::Builder::new()
        .name("vm-bridge-vbox".into())
        .spawn(move || vbox_pump(id, gen))
        .map_err(|e| format!("spawn vbox pump: {e}"))?;
    Ok(format!("Подключение к VirtualBox VM «{name}»…"))
}

fn vbox_pump(uuid: String, gen: u64) {
    loop {
        if !gen_is_current(gen) {
            return;
        }
        match crate::virtualbox::screenshot(&uuid) {
            Some((w, h, rgba)) => push_frame(gen, w, h, rgba_to_bgra(&rgba)),
            None => {
                if let Ok(mut s) = state().lock() {
                    if s.generation == gen && !s.have_frame {
                        s.status = "VirtualBox: жду первый кадр…".to_owned();
                    }
                }
            }
        }
        std::thread::sleep(crate::virtualbox::SCREENSHOT_INTERVAL);
    }
}

/// VirtualBox клавиатура: текст через keyboardputstring, спецклавиши — TODO.
fn dispatch_key_vbox(uuid: &str, ev: &crate::rustdesk_proto::KeyEvent) {
    use crate::rustdesk_proto::key_event::Union;
    if !(ev.press || ev.down) {
        return; // putstring сам жмёт+отпускает; реагируем только на нажатие
    }
    let res = match &ev.union {
        Some(Union::Unicode(ch)) => {
            if let Some(c) = char::from_u32(*ch) {
                crate::virtualbox::put_string(uuid, &c.to_string())
            } else {
                Ok(())
            }
        }
        Some(Union::ControlKey(ck)) => {
            // Enter/Backspace/Tab/Esc через PS/2 scancode (set-1).
            match vbox_ctrl_scancodes(*ck) {
                Some((make, brk)) => crate::virtualbox::put_scancodes(uuid, &[make, brk]),
                None => Ok(()),
            }
        }
        None => Ok(()),
    };
    match res {
        Ok(()) => note_input("клава VirtualBox ок"),
        Err(e) => note_input(&format!("клава VirtualBox: {e}")),
    }
}

/// PS/2 set-1 (make, break) для частых спецклавиш VirtualBox.
fn vbox_ctrl_scancodes(ck: i32) -> Option<(u8, u8)> {
    // ControlKey enum: Return=27, Backspace=2, Tab=31, Escape=8, Space=30.
    Some(match ck {
        27 => (0x1C, 0x9C), // Enter
        2 => (0x0E, 0x8E),  // Backspace
        31 => (0x0F, 0x8F), // Tab
        8 => (0x01, 0x81),  // Escape
        30 => (0x39, 0xB9), // Space
        _ => return None,
    })
}

// ── Hyper-V attach (Windows) ─────────────────────────────────────────────────

#[cfg(windows)]
fn attach_hyperv(vm_id: &str) -> Result<String, String> {
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

    let gen = begin_attach("hyperv", &vm.id, &vm.name)?;
    let vm_name = vm.name.clone();
    std::thread::Builder::new()
        .name("vm-bridge-pump".into())
        .spawn(move || pump_loop(vm, gen))
        .map_err(|e| format!("spawn pump: {e}"))?;

    Ok(format!("Подключение к VM «{vm_name}»…"))
}

#[cfg(not(windows))]
fn attach_hyperv(_vm_id: &str) -> Result<String, String> {
    Err("Hyper-V доступен только на Windows-хосте".to_owned())
}

#[cfg(windows)]
fn pump_loop(vm: hyperv::VmInfo, gen: u64) {
    let session = hyperv::HyperVSession::start(vm);
    // Share the cmd channel so dispatch_mouse/dispatch_key can send input
    // without creating a new WMI connection per event.
    if let Ok(mut s) = state().lock() {
        s.hyperv_cmd_tx = Some(session.cmd_tx.clone());
    }
    loop {
        // Эпоха устарела (другой attach/detach) → гасим сессию и выходим.
        let current_gen = state().lock().map(|s| s.generation).unwrap_or(gen + 1);
        if current_gen != gen {
            if let Ok(mut s) = state().lock() {
                s.hyperv_cmd_tx = None;
            }
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
                    s.frame_seq = s.frame_seq.wrapping_add(1);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
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
    let cmd_tx = state().lock().ok().and_then(|s| s.hyperv_cmd_tx.clone());
    let Some(tx) = cmd_tx else {
        note_input("мышь: сессия не готова");
        return;
    };
    let evt_type = ev.mask & 0x7;
    let button_bits = ev.mask >> 3;
    match evt_type {
        EVT_MOVE | EVT_DOWN | EVT_UP => {
            let nx = ev.x.max(0) as u32;
            let ny = ev.y.max(0) as u32;
            let nx = nx.min(w.saturating_sub(1));
            let ny = ny.min(h.saturating_sub(1));
            let _ = tx.send(hyperv::HyperVCmd::MoveMouse(nx, ny));
            if evt_type == EVT_DOWN || evt_type == EVT_UP {
                if let Some(btn) = vm_button(button_bits) {
                    let _ = tx.send(hyperv::HyperVCmd::ClickMouse(btn, evt_type == EVT_DOWN));
                }
            }
            note_input("мышь ок");
        }
        EVT_WHEEL => {} // Hyper-V WMI не имеет прямого wheel API
        _ => {}
    }
    let _ = vm_id; // resolved via session, not needed directly
}

#[cfg(windows)]
fn dispatch_key(vm_id: &str, ev: &crate::rustdesk_proto::KeyEvent) {
    use crate::rustdesk_proto::key_event::Union;
    let cmd_tx = state().lock().ok().and_then(|s| s.hyperv_cmd_tx.clone());
    let Some(tx) = cmd_tx else {
        note_input("клава: сессия не готова");
        return;
    };

    // Send a key via the session channel — no WMI connect overhead
    let send_press = |vk: u32| {
        let _ = tx.send(hyperv::HyperVCmd::PressKey(vk));
    };
    let send_release = |vk: u32| {
        let _ = tx.send(hyperv::HyperVCmd::ReleaseKey(vk));
    };
    let send_text = |t: String| {
        let _ = tx.send(hyperv::HyperVCmd::TypeText(t));
    };

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
                    mods.iter().for_each(|m| send_press(*m));
                    send_press(scan);
                    send_release(scan);
                    mods.iter().rev().for_each(|m| send_release(*m));
                } else if ev.down {
                    send_press(scan);
                } else {
                    send_release(scan);
                }
                Ok(())
            }
            Some(Union::Unicode(ch)) => {
                if *ch == 0 {
                    return Ok(());
                }
                let Some(c) = char::from_u32(*ch) else {
                    return Ok(());
                };
                // Печатаемый символ обрабатываем только на нажатии — атомарно.
                if !(ev.press || ev.down) {
                    return Ok(());
                }

                // VK-код клавиши: ASCII-пунктуация/буквы, затем JCUKEN-кириллица.
                let vk = ascii_scancode(c).or_else(|| cyrillic_to_vk(c));

                // ДИАГНОСТИКА: показываем что реально пришло и что отправим.
                note_input(&format!(
                    "U+{:04X} '{}' press={} down={} mods={:?} shift={} vk={:?}",
                    *ch,
                    c,
                    ev.press,
                    ev.down,
                    mods,
                    needs_shift(c),
                    vk
                ));

                match vk {
                    Some(vk_code) => {
                        const VK_SHIFT: u32 = 0x10;

                        // КРИТИЧНО: Shift НЕ берём из ev.modifiers и не полагаемся на
                        // отдельные ControlKey(Shift)-события. Раньше Shift "залипал" в VM
                        // (down приходил, up терялся) → всё печаталось заглавными, а реальный
                        // Shift инвертировал в строчные. Теперь регистр определяем из САМОГО
                        // символа и сами держим Shift ровно вокруг одного символа.
                        let shift_needed = needs_shift(c);

                        // Из внешних модификаторов оставляем только Ctrl/Alt/Meta
                        // (для сочетаний Ctrl+C и т.п.), Shift игнорируем — он наш.
                        let non_shift_mods: Vec<u32> =
                            mods.iter().copied().filter(|&m| m != VK_SHIFT).collect();

                        // Атомарно: [Ctrl/Alt][Shift] key↓ key↑ [Shift][Ctrl/Alt]
                        non_shift_mods.iter().for_each(|m| send_press(*m));
                        if shift_needed {
                            send_press(VK_SHIFT);
                        }
                        send_press(vk_code);
                        send_release(vk_code);
                        if shift_needed {
                            send_release(VK_SHIFT);
                        }
                        non_shift_mods.iter().rev().for_each(|m| send_release(*m));
                    }
                    None => {
                        // Нет VK-маппинга (emoji и т.п.) — TypeText как last-resort для ASCII.
                        if c.is_ascii_graphic() {
                            send_text(c.to_string());
                        }
                    }
                }
                Ok(())
            }
            None => Ok(()),
        }
    })();

    let _ = vm_id;
    match res {
        Ok(()) => note_input("клава ок"),
        Err(e) => note_input(&format!("клава: {e}")),
    }
}

/// Символ → (VK-код физической клавиши, нужен ли Shift) на US-раскладке.
///
/// Единая точка маппинга для ОБОИХ путей ввода:
///   • сетевой (dispatch_key, Android/удалённый клиент)
///   • локальный (main.rs hyperv_ui, хост к своим же VM)
///
/// Поддерживает ASCII (буквы/цифры/пунктуацию) и кириллицу (JCUKEN→позиция).
/// Регистр (Shift) выводится из самого символа, не из внешнего состояния —
/// устраняет "залипший Shift" и двойной ввод TypeText+PressKey.
#[cfg(windows)]
pub fn char_to_vk_shift(c: char) -> Option<(u32, bool)> {
    ascii_scancode(c)
        .or_else(|| cyrillic_to_vk(c))
        .map(|vk| (vk, needs_shift(c)))
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

/// Нужен ли Shift чтобы получить данный символ на US-раскладке.
///
/// Регистр выводится из самого Unicode-символа, а не из внешних модификаторов —
/// это устраняет проблему "залипшего Shift" (всё печаталось заглавными).
///
///   'A'..'Z'  → true   (заглавные буквы)
///   'a'..'z'  → false
///   '!@#$%^&*()_+{}:"~<>?|'  → true  (Shift-символы верхнего ряда/пунктуации)
///   '1234567890-=[];'`,./\\' → false (базовые)
///   Кириллица: регистр по c.is_uppercase()
#[cfg(windows)]
fn needs_shift(c: char) -> bool {
    match c {
        'A'..='Z' => true,
        'a'..='z' => false,
        '!' | '@' | '#' | '$' | '%' | '^' | '&' | '*' | '(' | ')' | '_' | '+' | '{' | '}' | ':'
        | '"' | '~' | '<' | '>' | '?' | '|' => true,
        '0'..='9' => false,
        '-' | '=' | '[' | ']' | ';' | '\'' | '`' | ',' | '.' | '/' | '\\' | ' ' | '\t' => false,
        // Кириллица и прочее: по регистру самого символа.
        _ => c.is_uppercase(),
    }
}

/// Символ → Windows VK-код физической клавиши (US QWERTY раскладка).
///
/// Возвращает VK "базовой" клавиши без учёта Shift — модификаторы берём
/// из `ev.modifiers` (клиент сам отслеживает Shift, Ctrl, Alt).
///
/// Примеры:
///   'a'/'A' → VK_A (0x41)   — Shift в modifiers
///   '!'     → VK_1 (0x31)   — Shift в modifiers
///   '~'     → VK_OEM_3 (0xC0) — Shift в modifiers
///   '-'/'_' → VK_OEM_MINUS (0xBD)
///
/// Прежде функция возвращала None для пунктуации, и диспетчер
/// откатывался к TypeText, которая для не-ASCII символов делала мусор
/// ('с' → '~') или ничего не отправляла. Теперь все US-символы покрыты.
#[cfg(windows)]
fn ascii_scancode(c: char) -> Option<u32> {
    Some(match c {
        // Буквы: VK = код заглавной буквы (0x41–0x5A).
        'a'..='z' | 'A'..='Z' => c.to_ascii_uppercase() as u32,
        // Цифры (0x30–0x39).
        '0'..='9' => c as u32,
        ' ' => 0x20,  // VK_SPACE
        '\t' => 0x09, // VK_TAB
        // Shifted-цифры: клиент посылает Unicode символ + Shift в modifiers.
        // Возвращаем VK базовой цифровой клавиши, Shift уже есть в mods.
        '!' => 0x31, // Shift+1
        '@' => 0x32, // Shift+2
        '#' => 0x33, // Shift+3
        '$' => 0x34, // Shift+4
        '%' => 0x35, // Shift+5
        '^' => 0x36, // Shift+6
        '&' => 0x37, // Shift+7
        '*' => 0x38, // Shift+8
        '(' => 0x39, // Shift+9
        ')' => 0x30, // Shift+0
        // Пунктуация (base key без Shift / со Shift через modifiers).
        '-' | '_' => 0xBD,  // VK_OEM_MINUS
        '=' | '+' => 0xBB,  // VK_OEM_PLUS
        '[' | '{' => 0xDB,  // VK_OEM_4
        ']' | '}' => 0xDD,  // VK_OEM_6
        ';' | ':' => 0xBA,  // VK_OEM_1
        '\'' | '"' => 0xDE, // VK_OEM_7
        '`' | '~' => 0xC0,  // VK_OEM_3
        ',' | '<' => 0xBC,  // VK_OEM_COMMA
        '.' | '>' => 0xBE,  // VK_OEM_PERIOD
        '/' | '?' => 0xBF,  // VK_OEM_2
        '\\' | '|' => 0xDC, // VK_OEM_5
        _ => return None,
    })
}

/// FIX-3: Кириллица (JCUKEN) → Windows VK-код физической позиции клавиши.
///
/// Msvm_Keyboard.PressKey принимает VK-код (позицию клавиши), а не символ.
/// Если оператор набирает на русской раскладке, клиент шлёт Unicode кириллицы.
/// Без этого маппинга Ctrl+А (Select All на RU) → ничего, с ним → Ctrl+VK_F.
/// Работает только если в VM тоже стоит русская раскладка (JCUKEN, стандарт RU).
///
/// Строчные и заглавные буквы маппятся на одинаковый VK (shift обрабатывает VM).
#[cfg(windows)]
fn cyrillic_to_vk(c: char) -> Option<u32> {
    // Таблица: кириллица (lowercase) → VK латинской клавиши на той же позиции.
    // Стандарт: Windows JCUKEN (Russian QWERTY-позиционный).
    let lower = c.to_lowercase().next()?;
    Some(match lower {
        // Верхний ряд (цифры пропускаем — они совпадают с ASCII)
        'й' => 0x51, // Q
        'ц' => 0x57, // W
        'у' => 0x45, // E
        'к' => 0x52, // R
        'е' => 0x54, // T
        'н' => 0x59, // Y
        'г' => 0x55, // U
        'ш' => 0x49, // I
        'щ' => 0x4F, // O
        'з' => 0x50, // P
        'х' => 0xDB, // [ (VK_OEM_4)
        'ъ' => 0xDD, // ] (VK_OEM_6)
        // Средний ряд
        'ф' => 0x41, // A
        'ы' => 0x53, // S
        'в' => 0x44, // D
        'а' => 0x46, // F
        'п' => 0x47, // G
        'р' => 0x48, // H
        'о' => 0x4A, // J
        'л' => 0x4B, // K
        'д' => 0x4C, // L
        'ж' => 0xBA, // ; (VK_OEM_1)
        'э' => 0xDE, // ' (VK_OEM_7)
        // Нижний ряд
        'я' => 0x5A, // Z
        'ч' => 0x58, // X
        'с' => 0x43, // C
        'м' => 0x56, // V
        'и' => 0x42, // B
        'т' => 0x4E, // N
        'ь' => 0x4D, // M
        'б' => 0xBC, // , (VK_OEM_COMMA)
        'ю' => 0xBE, // . (VK_OEM_PERIOD)
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

// ── Capability graph ─────────────────────────────────────────────────────────

/// Получить capability graph для VM в JSON. Для non-Windows всегда Unsupported.
pub fn get_capability_graph(vm_id: &str) -> String {
    #[cfg(windows)]
    {
        // Найти VM в локальном инвентаре
        let (_, real_id) = vm_id.split_once(':').unwrap_or(("hyperv", vm_id));
        let vms = crate::hyperv::list_vms();
        if let Some(vm) = vms.iter().find(|v| v.id == real_id) {
            return crate::capability_engine::evaluate(vm).to_json();
        }
        // VM не найдена — Unknown
        crate::capability_engine::VmCapabilityGraph::unsupported_stub(vm_id, "VM_NOT_FOUND")
            .to_json()
    }
    #[cfg(not(windows))]
    crate::capability_engine::evaluate_stub(vm_id).to_json()
}

// ── Checkpoint operations ─────────────────────────────────────────────────────

/// Обработать запрос checkpoint операции (JSON {"vm_id","op","path"}).
/// Возвращает JSON-результат.
pub fn checkpoint_op(json: &str) -> String {
    #[cfg(windows)]
    {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(json) else {
            return r#"{"ok":false,"error":"invalid json","checkpoints":[]}"#.to_owned();
        };
        let vm_id = val.get("vm_id").and_then(|v| v.as_str()).unwrap_or("");
        let op = val.get("op").and_then(|v| v.as_str()).unwrap_or("list");
        let path = val.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let vm_path = val.get("vm_path").and_then(|v| v.as_str()).unwrap_or("");
        let (provider, real_id) = vm_id.split_once(':').unwrap_or(("hyperv", vm_id));
        match (provider, op) {
            ("hyperv", "list") => {
                let checkpoints = crate::hyperv::list_checkpoints(real_id);
                let items: Vec<String> = checkpoints
                    .iter()
                    .map(|c| {
                        format!(
                            r#"{{"name":{},"path":{},"created_time":{},"type":{}}}"#,
                            json_str(&c.name),
                            json_str(&c.wmi_path),
                            json_str(&c.created_time),
                            json_str(&c.checkpoint_type),
                        )
                    })
                    .collect();
                format!(
                    r#"{{"vm_id":{},"op":"list","ok":true,"error":"","checkpoints":[{}]}}"#,
                    json_str(vm_id),
                    items.join(",")
                )
            }
            ("hyperv", "create") => {
                let effective_path = if !vm_path.is_empty() {
                    vm_path.to_owned()
                } else {
                    format!("Msvm_ComputerSystem.CreationClassName=\"Msvm_ComputerSystem\",Name=\"{real_id}\"")
                };
                match crate::hyperv::create_checkpoint(&effective_path, None) {
                    Ok(name) => format!(
                        r#"{{"vm_id":{},"op":"create","ok":true,"error":"","name":{}}}"#,
                        json_str(vm_id),
                        json_str(&name)
                    ),
                    Err(e) => format!(
                        r#"{{"vm_id":{},"op":"create","ok":false,"error":{}}}"#,
                        json_str(vm_id),
                        json_str(&e)
                    ),
                }
            }
            ("hyperv", "apply") => match crate::hyperv::apply_checkpoint(path) {
                Ok(()) => format!(
                    r#"{{"vm_id":{},"op":"apply","ok":true,"error":""}}"#,
                    json_str(vm_id)
                ),
                Err(e) => format!(
                    r#"{{"vm_id":{},"op":"apply","ok":false,"error":{}}}"#,
                    json_str(vm_id),
                    json_str(&e)
                ),
            },
            ("hyperv", "delete") => match crate::hyperv::delete_checkpoint(path) {
                Ok(()) => format!(
                    r#"{{"vm_id":{},"op":"delete","ok":true,"error":""}}"#,
                    json_str(vm_id)
                ),
                Err(e) => format!(
                    r#"{{"vm_id":{},"op":"delete","ok":false,"error":{}}}"#,
                    json_str(vm_id),
                    json_str(&e)
                ),
            },
            _ => {
                format!(r#"{{"ok":false,"error":"unsupported: {provider}/{op}","checkpoints":[]}}"#)
            }
        }
    }
    #[cfg(not(windows))]
    r#"{"ok":false,"error":"checkpoints only on Windows host","checkpoints":[]}"#.to_owned()
}

// ── Rescue input ──────────────────────────────────────────────────────────────

/// Обработать rescue input JSON {"vm_id","input_type","text"}.
/// Выполняется на хосте с гипервизором.
pub fn rescue_input(json: &str) {
    #[cfg(windows)]
    {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(json) else {
            return;
        };
        let vm_id = val.get("vm_id").and_then(|v| v.as_str()).unwrap_or("");
        let input_type = val.get("input_type").and_then(|v| v.as_str()).unwrap_or("");
        let text = val.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let (provider, real_id) = vm_id.split_once(':').unwrap_or(("hyperv", vm_id));
        match (provider, input_type) {
            ("hyperv", "ctrl_alt_del") => {
                let _ = send_ctrl_alt_del_hyperv(real_id);
            }
            ("hyperv", "type_text") => {
                let _ = crate::hyperv::type_text(real_id, text);
                note_input(if text.is_empty() {
                    "type_text: пустая строка"
                } else {
                    "type_text: OK"
                });
            }
            ("hyperv", "press_key") => {
                if let Ok(vk) = text
                    .trim_start_matches("0x")
                    .parse::<u32>()
                    .or_else(|_| u32::from_str_radix(text.trim_start_matches("0x"), 16))
                {
                    let _ = crate::hyperv::press_key(real_id, vk);
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    let _ = crate::hyperv::release_key(real_id, vk);
                }
            }
            _ => {}
        }
    }
}

#[cfg(windows)]
fn send_ctrl_alt_del_hyperv(vm_id: &str) -> Result<(), String> {
    // Приоритет: активная HyperVSession (если vm_bridge используется через сессию)
    if let Ok(s) = state().lock() {
        if let Some(tx) = &s.hyperv_cmd_tx {
            let _ = tx.send(crate::hyperv::HyperVCmd::CtrlAltDel);
            return Ok(());
        }
    }
    // Fallback: прямой WMI вызов без сессии
    crate::hyperv::send_ctrl_alt_del(vm_id)
}
