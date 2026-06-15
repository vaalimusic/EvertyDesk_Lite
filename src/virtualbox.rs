//! Agentless-доступ к VirtualBox VM через `VBoxManage` CLI.
//!
//! Фундамент второго провайдера (после Hyper-V). Кросс-платформенно: VBoxManage
//! есть на Windows/macOS/Linux. Доступ к гостю БЕЗ агента:
//!
//!  • enumeration: `VBoxManage list vms` + `list runningvms`
//!  • экран: `VBoxManage controlvm <uuid> screenshotpng <file>` → PNG → RGBA
//!  • клавиатура: `VBoxManage controlvm <uuid> keyboardputstring/putscancode`
//!  • мышь: VBoxManage не имеет публичного mouse API → TODO (VRDP/SDK)
//!
//! Скриншот-поллинг медленнее Hyper-V thumbnail, но не требует агента и работает
//! с любым гостём. Для интерактива позже — VRDP (встроенный RDP VirtualBox).

use std::{process::Command, sync::mpsc, thread, time::Duration};

#[derive(Debug, Clone)]
pub struct VboxVm {
    /// UUID машины (стабильный id для controlvm).
    pub id: String,
    pub name: String,
    pub running: bool,
}

/// Найти бинарь VBoxManage: PATH или типичные места установки.
fn vboxmanage() -> Option<String> {
    // 1) на PATH
    for cand in ["VBoxManage", "vboxmanage"] {
        if Command::new(cand).arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
            return Some(cand.to_owned());
        }
    }
    // 2) типичные пути
    let candidates = [
        r"C:\Program Files\Oracle\VirtualBox\VBoxManage.exe",
        r"C:\Program Files\Oracle\VirtualBox\VBoxManage",
        "/usr/bin/VBoxManage",
        "/usr/local/bin/VBoxManage",
        "/Applications/VirtualBox.app/Contents/MacOS/VBoxManage",
    ];
    for path in candidates {
        if std::path::Path::new(path).exists() {
            return Some(path.to_owned());
        }
    }
    None
}

/// Доступен ли VirtualBox на этом хосте.
pub fn is_available() -> bool {
    vboxmanage().is_some()
}

/// Список VM VirtualBox с отметкой running.
pub fn list_vms() -> Vec<VboxVm> {
    let Some(vbm) = vboxmanage() else {
        return Vec::new();
    };
    let all = run_list(&vbm, "vms");
    let running = run_list(&vbm, "runningvms")
        .into_iter()
        .map(|(_, id)| id)
        .collect::<Vec<_>>();

    all.into_iter()
        .map(|(name, id)| VboxVm {
            running: running.contains(&id),
            id,
            name,
        })
        .collect()
}

fn run_list(vbm: &str, kind: &str) -> Vec<(String, String)> {
    let Ok(out) = Command::new(vbm).args(["list", kind]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(parse_vm_line)
        .collect()
}

/// Строка вида: `"VM Name" {uuid}` → (name, uuid).
fn parse_vm_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let brace = line.rfind('{')?;
    let end = line.rfind('}')?;
    if end <= brace {
        return None;
    }
    let id = line[brace + 1..end].trim().to_owned();
    let name = line[..brace].trim().trim_matches('"').to_owned();
    if id.is_empty() {
        None
    } else {
        Some((name, id))
    }
}

/// Снять скриншот VM → (width, height, RGBA). None при ошибке/выключенной VM.
pub fn screenshot(uuid: &str) -> Option<(u32, u32, Vec<u8>)> {
    let vbm = vboxmanage()?;
    let mut path = std::env::temp_dir();
    path.push(format!("evd_vbox_{}.png", sanitize(uuid)));

    let out = Command::new(&vbm)
        .args(["controlvm", uuid, "screenshotpng"])
        .arg(&path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);

    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (w, h) = (img.width(), img.height());
    Some((w, h, img.into_raw()))
}

/// Напечатать строку в гостя (VBoxManage keyboardputstring).
pub fn put_string(uuid: &str, text: &str) -> Result<(), String> {
    let vbm = vboxmanage().ok_or_else(|| "VBoxManage не найден".to_owned())?;
    let out = Command::new(&vbm)
        .args(["controlvm", uuid, "keyboardputstring", text])
        .output()
        .map_err(|e| format!("keyboardputstring: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "keyboardputstring rc={}",
            out.status.code().unwrap_or(-1)
        ))
    }
}

/// Послать PS/2 set-1 scancodes (VBoxManage keyboardputscancode, hex-байты).
pub fn put_scancodes(uuid: &str, codes: &[u8]) -> Result<(), String> {
    if codes.is_empty() {
        return Ok(());
    }
    let vbm = vboxmanage().ok_or_else(|| "VBoxManage не найден".to_owned())?;
    let hex: Vec<String> = codes.iter().map(|b| format!("{b:02x}")).collect();
    let mut cmd = Command::new(&vbm);
    cmd.args(["controlvm", uuid, "keyboardputscancode"]);
    cmd.args(&hex);
    let out = cmd.output().map_err(|e| format!("keyboardputscancode: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "keyboardputscancode rc={}",
            out.status.code().unwrap_or(-1)
        ))
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Рекомендуемый интервал скриншот-поллинга (медленнее Hyper-V thumbnail).
pub const SCREENSHOT_INTERVAL: Duration = Duration::from_millis(200);

// ── Background session ────────────────────────────────────────────────────────

pub enum VboxCmd {
    PutString(String),
    /// PS/2 Set-1 make/break scancodes for one special key: &[make_bytes, break_bytes]
    PutScancodes(Vec<u8>, Vec<u8>),
    MoveMouse(i32, i32),
    Stop,
}

pub struct VboxSession {
    cmd_tx: mpsc::SyncSender<VboxCmd>,
    pub frame_rx: mpsc::Receiver<(u32, u32, Vec<u8>)>,
    pub status_rx: mpsc::Receiver<String>,
}

impl VboxSession {
    pub fn start(uuid: String) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<VboxCmd>(32);
        let (frame_tx, frame_rx) = mpsc::sync_channel::<(u32, u32, Vec<u8>)>(2);
        let (status_tx, status_rx) = mpsc::sync_channel::<String>(8);

        thread::spawn(move || {
            let _ = status_tx.send("Захват экрана VirtualBox...".to_owned());
            loop {
                // Drain commands first
                loop {
                    match cmd_rx.try_recv() {
                        Ok(VboxCmd::Stop) => return,
                        Ok(VboxCmd::PutString(text)) => {
                            if let Err(e) = put_string(&uuid, &text) {
                                let _ = status_tx.send(format!("Ввод: {e}"));
                            }
                        }
                        Ok(VboxCmd::PutScancodes(make, brk)) => {
                            let _ = put_scancodes(&uuid, &make);
                            thread::sleep(Duration::from_millis(10));
                            let _ = put_scancodes(&uuid, &brk);
                        }
                        Ok(VboxCmd::MoveMouse(dx, dy)) => {
                            let _ = vbox_mousemove(&uuid, dx, dy);
                        }
                        Err(_) => break,
                    }
                }
                // Capture frame
                match screenshot(&uuid) {
                    Some((w, h, rgba)) => {
                        let _ = frame_tx.try_send((w, h, rgba));
                    }
                    None => {
                        let _ = status_tx.send("Не удалось получить экран VM".to_owned());
                    }
                }
                thread::sleep(SCREENSHOT_INTERVAL);
            }
        });

        VboxSession { cmd_tx, frame_rx, status_rx }
    }

    pub fn send(&self, cmd: VboxCmd) {
        let _ = self.cmd_tx.try_send(cmd);
    }

    pub fn stop(self) {
        let _ = self.cmd_tx.try_send(VboxCmd::Stop);
    }

    /// (width, height, rgba)
    pub fn try_recv_frame(&self) -> Option<(u32, u32, Vec<u8>)> {
        self.frame_rx.try_recv().ok()
    }

    pub fn try_recv_status(&self) -> Option<String> {
        self.status_rx.try_recv().ok()
    }
}

fn vbox_mousemove(uuid: &str, dx: i32, dy: i32) -> Result<(), String> {
    let vbm = vboxmanage().ok_or_else(|| "VBoxManage не найден".to_owned())?;
    let out = Command::new(&vbm)
        .args(["controlvm", uuid, "mousemove", &dx.to_string(), &dy.to_string()])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err("mousemove failed".to_owned()) }
}

/// PS/2 Set-1 scancode pair for a special key (make, break). Abstracted over
/// the actual key type so this module stays free of egui/eframe dependencies.
/// Returns None for printable/unmapped keys (caller uses put_string instead).
pub fn special_key_to_scancodes(key_id: u8) -> Option<(Vec<u8>, Vec<u8>)> {
    fn simple(code: u8) -> (Vec<u8>, Vec<u8>) { (vec![code], vec![code | 0x80]) }
    fn ext(code: u8) -> (Vec<u8>, Vec<u8>) { (vec![0xE0, code], vec![0xE0, code | 0x80]) }
    Some(match key_id {
        0x01 => simple(0x01), // Escape
        0x0E => simple(0x0E), // Backspace
        0x0F => simple(0x0F), // Tab
        0x1C => simple(0x1C), // Enter
        0x52 => ext(0x52),    // Insert
        0x53 => ext(0x53),    // Delete
        0x47 => ext(0x47),    // Home
        0x4F => ext(0x4F),    // End
        0x49 => ext(0x49),    // PageUp
        0x51 => ext(0x51),    // PageDown
        0x4B => ext(0x4B),    // ArrowLeft
        0x4D => ext(0x4D),    // ArrowRight
        0x48 => ext(0x48),    // ArrowUp
        0x50 => ext(0x50),    // ArrowDown
        0x3B => simple(0x3B), // F1
        0x3C => simple(0x3C), // F2
        0x3D => simple(0x3D), // F3
        0x3E => simple(0x3E), // F4
        0x3F => simple(0x3F), // F5
        0x40 => simple(0x40), // F6
        0x41 => simple(0x41), // F7
        0x42 => simple(0x42), // F8
        0x43 => simple(0x43), // F9
        0x44 => simple(0x44), // F10
        0x57 => simple(0x57), // F11
        0x58 => simple(0x58), // F12
        _ => return None,
    })
}
