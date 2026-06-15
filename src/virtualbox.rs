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

use std::process::Command;
use std::time::Duration;

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
