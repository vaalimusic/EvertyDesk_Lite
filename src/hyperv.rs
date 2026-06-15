//! Hyper-V VM console access via raw WMI COM (root\virtualization\v2).
//!
//! No extra crates — uses the `windows` crate already in the project.
//! Screen: Hyper-V thumbnail snapshots, converted from RGB565 to RGBA.
//! Input:  Msvm_Keyboard / Msvm_Mouse WMI methods.

use std::{collections::HashMap, mem::ManuallyDrop, sync::mpsc, time::Duration};

use windows::{
    core::{BSTR, PCWSTR},
    Win32::{
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, CLSCTX_INPROC_SERVER,
                COINIT_MULTITHREADED, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE, SAFEARRAY, VARIANT, VT_ARRAY, VT_BSTR, VT_I4,
                VT_UI1, VT_UI2, VT_UI4,
            },
            Ole::{
                SafeArrayAccessData, SafeArrayGetLBound, SafeArrayGetUBound,
                SafeArrayUnaccessData,
            },
            Wmi::{
                IEnumWbemClassObject, IWbemClassObject, IWbemLocator, IWbemServices,
                WbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
                WBEM_GENERIC_FLAG_TYPE,
            },
        },
    },
};

const RPC_C_AUTHN_WINNT_VALUE: u32 = 10;
const RPC_C_AUTHZ_NONE_VALUE: u32 = 0;

// ── VM state ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct VmInfo {
    pub name: String,
    /// GUID used as SystemName for keyboard/mouse WMI objects
    pub id: String,
    /// Full WMI object path for method calls
    pub wmi_path: String,
    pub state: VmState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VmState {
    Running,
    Off,
    Paused,
    Saved,
    Other(u32),
}

impl VmState {
    fn from_u32(v: u32) -> Self {
        match v {
            2 => Self::Running,
            3 => Self::Off,
            32768 => Self::Paused,
            32769 | 32770 => Self::Saved,
            x => Self::Other(x),
        }
    }
    pub fn label(&self) -> &str {
        match self {
            Self::Running => "Running",
            Self::Off => "Off",
            Self::Paused => "Paused",
            Self::Saved => "Saved",
            Self::Other(_) => "Unknown",
        }
    }
    pub fn is_connectable(&self) -> bool {
        matches!(self, Self::Running | Self::Paused)
    }
}

// ── WMI session wrapper ───────────────────────────────────────────────────────

struct Wmi {
    svc: IWbemServices,
}

impl Wmi {
    fn connect() -> Option<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let loc: IWbemLocator =
                CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;

            let ns = BSTR::from("ROOT\\virtualization\\v2");
            let svc = loc
                .ConnectServer(&ns, None, None, &BSTR::default(), 0, None, None)
                .ok()?;

            CoSetProxyBlanket(
                &svc,
                RPC_C_AUTHN_WINNT_VALUE,
                RPC_C_AUTHZ_NONE_VALUE,
                PCWSTR::null(),
                RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
            )
            .ok()?;

            Some(Wmi { svc })
        }
    }

    /// Run WQL query, return list of property maps.
    fn query(&self, wql: &str) -> Vec<HashMap<String, WmiVal>> {
        unsafe {
            let query = BSTR::from(wql);
            let wql_str = BSTR::from("WQL");
            let flags = WBEM_GENERIC_FLAG_TYPE(
                WBEM_FLAG_FORWARD_ONLY.0 | WBEM_FLAG_RETURN_IMMEDIATELY.0,
            );

            let Ok(enumerator) =
                self.svc
                    .ExecQuery(&wql_str, &query, flags, None)
            else {
                return vec![];
            };

            collect_rows(enumerator)
        }
    }

    /// Call a WMI method. in_params: list of (name, value) to set on the input object.
    /// Returns output properties map.
    fn exec_method(
        &self,
        obj_path: &str,
        method: &str,
        in_params: &[(&str, WmiVal)],
    ) -> Option<HashMap<String, WmiVal>> {
        unsafe {
            // Get the class to obtain the method input signature
            let class_name = obj_path_class(obj_path);
            let mut class_obj: Option<IWbemClassObject> = None;
            self.svc
                .GetObject(
                    &BSTR::from(class_name),
                    0,
                    None,
                    Some(&mut class_obj),
                    None,
                )
                .ok()?;
            let class_obj = class_obj?;

            // Get method in-param signature and spawn an instance
            let mut in_sig: Option<IWbemClassObject> = None;
            let mut _out_sig: Option<IWbemClassObject> = None;
            let method_w = wide_null(method);
            class_obj
                .GetMethod(PCWSTR(method_w.as_ptr()), 0, &mut in_sig, &mut _out_sig)
                .ok()?;

            let in_instance = in_sig?.SpawnInstance(0).ok()?;

            // Fill in-params
            for (name, val) in in_params {
                let name_w = wide_null(name);
                let var = wmi_val_to_variant(val);
                in_instance
                    .Put(PCWSTR(name_w.as_ptr()), 0, &var, 0)
                    .ok()?;
            }

            // Execute method
            let mut out_obj: Option<IWbemClassObject> = None;
            self.svc
                .ExecMethod(
                    &BSTR::from(obj_path),
                    &BSTR::from(method),
                    0,
                    None,
                    &in_instance,
                    Some(&mut out_obj as *mut _),
                    None,
                )
                .ok()?;

            let out = out_obj?;
            Some(read_all_properties(&out))
        }
    }
}

// ── WMI value type ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum WmiVal {
    Str(String),
    U16(u16),
    U32(u32),
    I32(i32),
    Bytes(Vec<u8>),
}

impl WmiVal {
    fn as_str(&self) -> Option<&str> {
        if let Self::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }
    fn as_u32(&self) -> Option<u32> {
        if let Self::U32(v) = self {
            Some(*v)
        } else if let Self::U16(v) = self {
            Some(*v as u32)
        } else {
            None
        }
    }
    fn as_bytes(&self) -> Option<&[u8]> {
        if let Self::Bytes(b) = self {
            Some(b)
        } else {
            None
        }
    }
}

// ── COM ↔ WmiVal helpers ──────────────────────────────────────────────────────

unsafe fn wmi_val_to_variant(val: &WmiVal) -> VARIANT {
    let mut var = VARIANT::default();
    let inner = &mut var.Anonymous.Anonymous;
    match val {
        WmiVal::Str(s) => {
            inner.vt = VT_BSTR;
            inner.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(s.as_str()));
        }
        WmiVal::U16(v) => {
            inner.vt = VT_UI2;
            inner.Anonymous.uiVal = *v;
        }
        WmiVal::U32(v) => {
            inner.vt = VT_UI4;
            inner.Anonymous.ulVal = *v;
        }
        WmiVal::I32(v) => {
            inner.vt = VT_I4;
            inner.Anonymous.lVal = *v;
        }
        WmiVal::Bytes(_) => {} // can't put byte arrays as in-params
    }
    var
}

unsafe fn variant_to_wmi_val(var: &VARIANT) -> Option<WmiVal> {
    let inner = &var.Anonymous.Anonymous;
    let vt = inner.vt.0;
    match vt {
        v if v == VT_BSTR.0 => {
            let s = inner.Anonymous.bstrVal.to_string();
            Some(WmiVal::Str(s))
        }
        8 => {
            // VT_BSTR = 8 (same as above, belt-and-suspenders)
            let s = inner.Anonymous.bstrVal.to_string();
            Some(WmiVal::Str(s))
        }
        3 => Some(WmiVal::I32(inner.Anonymous.lVal)),  // VT_I4
        19 => Some(WmiVal::U32(inner.Anonymous.ulVal)), // VT_UI4
        18 => Some(WmiVal::U16(inner.Anonymous.uiVal)), // VT_UI2
        v if (v & VT_ARRAY.0 != 0) && (v & 0x00FF == VT_UI1.0) => {
            // VT_ARRAY | VT_UI1
            let psa: *const SAFEARRAY = inner.Anonymous.parray;
            if psa.is_null() {
                return None;
            }
            let lb = SafeArrayGetLBound(psa, 1).ok()?;
            let ub = SafeArrayGetUBound(psa, 1).ok()?;
            let len = (ub - lb + 1).max(0) as usize;
            let mut pdata: *mut std::ffi::c_void = std::ptr::null_mut();
            SafeArrayAccessData(psa, &mut pdata).ok()?;
            let bytes = std::slice::from_raw_parts(pdata as *const u8, len).to_vec();
            SafeArrayUnaccessData(psa).ok()?;
            Some(WmiVal::Bytes(bytes))
        }
        _ => None,
    }
}

unsafe fn read_all_properties(obj: &IWbemClassObject) -> HashMap<String, WmiVal> {
    let mut map = HashMap::new();

    // Begin enumeration of all properties
    let _ = obj.BeginEnumeration(0);
    loop {
        let mut name = BSTR::default();
        let mut var = VARIANT::default();
        let hr = obj.Next(
            0,
            &mut name,
            &mut var,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        if hr.is_err() {
            break;
        }
        if name.is_empty() {
            break;
        }
        if let Some(val) = variant_to_wmi_val(&var) {
            map.insert(name.to_string(), val);
        }
    }
    let _ = obj.EndEnumeration();

    for name in ["__PATH", "__RELPATH", "__CLASS"] {
        if !map.contains_key(name) {
            if let Some(val) = read_named_property(obj, name) {
                map.insert(name.to_owned(), val);
            }
        }
    }

    map
}

unsafe fn read_named_property(obj: &IWbemClassObject, name: &str) -> Option<WmiVal> {
    let name_w = wide_null(name);
    let mut var = VARIANT::default();
    obj.Get(
        PCWSTR(name_w.as_ptr()),
        0,
        &mut var,
        None,
        None,
    )
    .ok()?;
    variant_to_wmi_val(&var)
}

unsafe fn collect_rows(enumerator: IEnumWbemClassObject) -> Vec<HashMap<String, WmiVal>> {
    let mut rows = Vec::new();
    loop {
        let mut row: [Option<IWbemClassObject>; 1] = [None];
        let mut returned = 0u32;
        let hr = enumerator.Next(5000, &mut row, &mut returned);
        if hr.is_err() || returned == 0 {
            break;
        }
        if let Some(obj) = &row[0] {
            rows.push(read_all_properties(obj));
        }
    }
    rows
}

/// Extract the class name from a WMI object path like
/// `\\SERVER\ROOT\virtualization\v2:Msvm_VirtualSystemManagementService.Name="..."`
fn obj_path_class(path: &str) -> &str {
    // After the last colon, before the dot
    let after_colon = path.rfind(':').map(|i| &path[i + 1..]).unwrap_or(path);
    let before_dot = after_colon.find('.').map(|i| &after_colon[..i]).unwrap_or(after_colon);
    before_dot
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

// ── Public API ────────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub fn is_available() -> bool {
    Wmi::connect().is_some()
}

pub fn list_vms() -> Vec<VmInfo> {
    let mut vms = list_vms_com();
    if vms.is_empty() {
        vms = list_vms_powershell();
    }
    vms
}

fn list_vms_com() -> Vec<VmInfo> {
    let Some(wmi) = Wmi::connect() else { return vec![] };
    // НЕ проецируем __PATH в SELECT: у virtualization-провайдера это может
    // вернуть 0 строк. read_all_properties дочитывает __PATH через obj.Get.
    let rows = wmi.query(
        "SELECT Name, ElementName, Caption, Description, EnabledState \
         FROM Msvm_ComputerSystem",
    );
    rows.into_iter()
        .filter_map(|mut row| {
            if !is_vm_row(&row) {
                return None;
            }
            let id = row.remove("Name")?.as_str()?.to_owned();
            let wmi_path = take_non_empty_string(&mut row, "__PATH")
                .or_else(|| take_non_empty_string(&mut row, "__RELPATH"))
                .unwrap_or_else(|| vm_computer_system_path(&id));
            Some(VmInfo {
                name: row.remove("ElementName")?.as_str()?.to_owned(),
                id,
                wmi_path,
                state: VmState::from_u32(
                    row.remove("EnabledState").and_then(|v| v.as_u32()).unwrap_or(0),
                ),
            })
        })
        .collect()
}

fn list_vms_powershell() -> Vec<VmInfo> {
    let script = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Get-CimInstance -Namespace root\virtualization\v2 -ClassName Msvm_ComputerSystem |
  Where-Object {
    $_.Name -match '^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}$' -or
    $_.Caption -like '*Virtual Machine*' -or
    $_.Caption -like '*Виртуальная машина*'
  } |
  Select-Object Name, ElementName, EnabledState |
  ConvertTo-Json -Depth 2
"#;

    let Ok(output) = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .output()
    else {
        return Vec::new();
    };

    if !output.status.success() {
        return Vec::new();
    }

    parse_vm_json(&String::from_utf8_lossy(&output.stdout))
}

fn parse_vm_json(text: &str) -> Vec<VmInfo> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
        return Vec::new();
    };
    let rows: Vec<&serde_json::Value> = match &value {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(_) => vec![&value],
        _ => Vec::new(),
    };

    rows.into_iter()
        .filter_map(|row| {
            let id = row.get("Name")?.as_str()?.trim().to_owned();
            if uuid::Uuid::parse_str(&id).is_err() {
                return None;
            }
            let name = row
                .get("ElementName")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(&id)
                .to_owned();
            let state = row
                .get("EnabledState")
                .and_then(serde_json::Value::as_u64)
                .and_then(|state| u32::try_from(state).ok())
                .unwrap_or(0);
            Some(VmInfo {
                name,
                id: id.clone(),
                wmi_path: vm_computer_system_path(&id),
                state: VmState::from_u32(state),
            })
        })
        .collect()
}

fn is_vm_row(row: &HashMap<String, WmiVal>) -> bool {
    let text_says_vm = ["Caption", "Description"]
        .into_iter()
        .filter_map(|key| row.get(key).and_then(WmiVal::as_str))
        .any(|text| {
            let text = text.to_lowercase();
            text.contains("virtual machine") || text.contains("виртуальная машина")
        });

    if text_says_vm {
        return true;
    }

    row.get("Name")
        .and_then(WmiVal::as_str)
        .and_then(|name| uuid::Uuid::parse_str(name).ok())
        .is_some()
}

fn take_non_empty_string(row: &mut HashMap<String, WmiVal>, key: &str) -> Option<String> {
    row.remove(key)
        .and_then(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned))
}

fn vm_computer_system_path(vm_id: &str) -> String {
    format!(
        "Msvm_ComputerSystem.CreationClassName=\"Msvm_ComputerSystem\",Name=\"{}\"",
        escape_wmi_key(vm_id)
    )
}

fn virtual_system_setting_path(instance_id: &str) -> String {
    format!(
        "Msvm_VirtualSystemSettingData.InstanceID=\"{}\"",
        escape_wmi_key(instance_id)
    )
}

fn escape_wmi_key(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn escape_wmi_value(value: &str) -> String {
    value.replace('\'', "''")
}

/// Returns raw RGB bytes (3 bytes/pixel, width × height). None if VM is off or on error.
pub fn video_resolution(vm_id: &str) -> Option<(u16, u16)> {
    let wmi = Wmi::connect()?;
    let q = format!(
        "SELECT CurrentHorizontalResolution, CurrentVerticalResolution \
         FROM Msvm_VideoHead WHERE SystemName = '{vm_id}'",
    );
    let mut row = wmi.query(&q).into_iter().next()?;
    let width = row
        .remove("CurrentHorizontalResolution")
        .and_then(|v| v.as_u32())
        .and_then(|v| u16::try_from(v).ok())?;
    let height = row
        .remove("CurrentVerticalResolution")
        .and_then(|v| v.as_u32())
        .and_then(|v| u16::try_from(v).ok())?;
    if width == 0 || height == 0 {
        None
    } else {
        Some((width, height))
    }
}

pub fn capture_screen(vm_id: &str, vm_wmi_path: &str, width: u16, height: u16) -> Result<Vec<u8>, String> {
    let wmi = Wmi::connect().ok_or_else(|| "WMI connect failed".to_owned())?;

    // Get the management service path (singleton)
    let svc_path = management_service_path(&wmi)
        .ok_or_else(|| "Msvm_VirtualSystemManagementService path not found".to_owned())?;
    let setting_path = current_setting_path(&wmi, vm_id, vm_wmi_path)
        .ok_or_else(|| "Msvm_VirtualSystemSettingData path not found".to_owned())?;

    let out = wmi.exec_method(
        &svc_path,
        "GetVirtualSystemThumbnailImage",
        &[
            ("TargetSystem", WmiVal::Str(setting_path)),
            ("WidthPixels", WmiVal::U16(width)),
            ("HeightPixels", WmiVal::U16(height)),
        ],
    )
    .ok_or_else(|| "GetVirtualSystemThumbnailImage ExecMethod failed".to_owned())?;

    if let Some(rv) = out.get("ReturnValue").and_then(WmiVal::as_u32) {
        if rv != 0 {
            return Err(format!("GetVirtualSystemThumbnailImage ReturnValue={rv}"));
        }
    }

    let rgb565 = out
        .into_values()
        .find_map(|v| v.as_bytes().map(|b| b.to_vec()))
        .filter(|bytes| bytes.len() >= width as usize * height as usize * 2)
        .ok_or_else(|| "ImageData is empty or smaller than expected RGB565 frame".to_owned())?;

    Ok(rgb565_to_rgba(&rgb565, width as usize, height as usize))
}

fn management_service_path(wmi: &Wmi) -> Option<String> {
    // Проекция `__PATH` в SELECT отвергается некоторыми сборками virtualization
    // провайдера (запрос возвращает 0 строк). Берём `SELECT *` — read_all_properties
    // сам дочитывает __PATH через obj.Get.
    if let Some(mut row) = wmi
        .query("SELECT * FROM Msvm_VirtualSystemManagementService")
        .into_iter()
        .next()
    {
        if let Some(path) = take_non_empty_string(&mut row, "__PATH")
            .or_else(|| take_non_empty_string(&mut row, "__RELPATH"))
        {
            return Some(path);
        }
    }
    // Fallback: сервис — singleton с Name="vmms". Строим полный ключевой путь
    // (все 4 ключа), чтобы ExecMethod работал даже если запрос вернул пусто.
    Some(well_known_management_service_path())
}

/// Полный путь singleton-сервиса управления Hyper-V (ключи: CreationClassName,
/// Name="vmms", SystemCreationClassName, SystemName=имя хоста).
fn well_known_management_service_path() -> String {
    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".to_owned());
    format!(
        "Msvm_VirtualSystemManagementService.CreationClassName=\"Msvm_VirtualSystemManagementService\",\
Name=\"vmms\",SystemCreationClassName=\"Msvm_ComputerSystem\",SystemName=\"{}\"",
        escape_wmi_key(&host)
    )
}

fn current_setting_path(wmi: &Wmi, vm_id: &str, vm_wmi_path: &str) -> Option<String> {
    let q = format!(
        "ASSOCIATORS OF {{{vm_wmi_path}}} WHERE ResultClass = Msvm_VirtualSystemSettingData"
    );
    // Без `?`: если ASSOCIATORS вернул пусто — падаем в SELECT-fallback ниже,
    // а не выходим из функции.
    if let Some(mut row) = wmi.query(&q).into_iter().next() {
        if let Some(path) = take_non_empty_string(&mut row, "__PATH")
            .or_else(|| take_non_empty_string(&mut row, "__RELPATH"))
        {
            return Some(path);
        }
    }

    let q = format!(
        "SELECT * FROM Msvm_VirtualSystemSettingData \
         WHERE VirtualSystemIdentifier = '{}' \
         AND VirtualSystemType = 'Microsoft:Hyper-V:System:Realized'",
        escape_wmi_value(vm_id)
    );
    let mut row = wmi.query(&q).into_iter().next()?;
    take_non_empty_string(&mut row, "__PATH").or_else(|| {
        take_non_empty_string(&mut row, "InstanceID")
            .map(|instance_id| virtual_system_setting_path(&instance_id))
    })
}

/// Ограничить разрешение thumbnail рамкой max_w×max_h с сохранением пропорций.
/// Чётные размеры (требование энкодеров H264). Если уже меньше — без изменений.
fn cap_resolution(w: u16, h: u16, max_w: u16, max_h: u16) -> (u16, u16) {
    if w == 0 || h == 0 {
        return (1280, 720);
    }
    if w <= max_w && h <= max_h {
        return (w & !1, h & !1);
    }
    let scale = (max_w as f32 / w as f32).min(max_h as f32 / h as f32);
    let nw = ((w as f32 * scale) as u16).max(2) & !1;
    let nh = ((h as f32 * scale) as u16).max(2) & !1;
    (nw, nh)
}

fn rgb565_to_rgba(rgb565: &[u8], width: usize, height: usize) -> Vec<u8> {
    let px_count = width.saturating_mul(height);
    let mut rgba = Vec::with_capacity(px_count.saturating_mul(4));
    for px in rgb565.chunks_exact(2).take(px_count) {
        let value = u16::from_le_bytes([px[0], px[1]]);
        let r5 = ((value >> 11) & 0x1f) as u8;
        let g6 = ((value >> 5) & 0x3f) as u8;
        let b5 = (value & 0x1f) as u8;
        rgba.push((r5 << 3) | (r5 >> 2));
        rgba.push((g6 << 2) | (g6 >> 4));
        rgba.push((b5 << 3) | (b5 >> 2));
        rgba.push(255);
    }
    rgba
}

pub fn press_key(vm_id: &str, scan_code: u32) {
    kbd_exec(vm_id, "PressKey", scan_code);
}

pub fn release_key(vm_id: &str, scan_code: u32) {
    kbd_exec(vm_id, "ReleaseKey", scan_code);
}

pub fn type_text(vm_id: &str, text: &str) {
    let Some(wmi) = Wmi::connect() else { return };
    let Some(path) = find_device_path(&wmi, "Msvm_Keyboard", vm_id) else { return };
    let _ = wmi.exec_method(&path, "TypeText", &[("asciiText", WmiVal::Str(text.to_owned()))]);
}

fn kbd_exec(vm_id: &str, method: &str, scan_code: u32) {
    let Some(wmi) = Wmi::connect() else { return };
    let Some(path) = find_device_path(&wmi, "Msvm_Keyboard", vm_id) else { return };
    let _ = wmi.exec_method(&path, method, &[("scanCode", WmiVal::U32(scan_code))]);
}

/// x, y in 0–65535 (normalized to VM display).
pub fn move_mouse(vm_id: &str, x: u32, y: u32) {
    let Some(wmi) = Wmi::connect() else { return };
    let Some(path) = find_device_path(&wmi, "Msvm_Mouse", vm_id) else { return };
    let _ = wmi.exec_method(
        &path,
        "SetAbsolutePosition",
        &[
            ("horizontalPosition", WmiVal::I32(x as i32)),
            ("verticalPosition", WmiVal::I32(y as i32)),
        ],
    );
}

/// button: 1=left, 2=right, 3=middle.
pub fn click_mouse(vm_id: &str, button: u32, press: bool) {
    let Some(wmi) = Wmi::connect() else { return };
    let Some(path) = find_device_path(&wmi, "Msvm_Mouse", vm_id) else { return };
    let method = if press { "ClickButton" } else { "ReleaseButton" };
    let _ = wmi.exec_method(&path, method, &[("buttonIndex", WmiVal::U32(button))]);
}

fn find_device_path(wmi: &Wmi, class: &str, vm_id: &str) -> Option<String> {
    let q = format!("SELECT * FROM {class} WHERE SystemName = '{vm_id}'");
    wmi.query(&q)
        .into_iter()
        .next()?
        .remove("__PATH")?
        .as_str()
        .map(|s| s.to_owned())
}

// ── Background session ────────────────────────────────────────────────────────

pub enum HyperVCmd {
    PressKey(u32),
    ReleaseKey(u32),
    TypeText(String),
    MoveMouse(u32, u32),
    ClickMouse(u32, bool),
    Stop,
}

pub struct Frame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub struct HyperVSession {
    pub cmd_tx: mpsc::SyncSender<HyperVCmd>,
    pub frame_rx: mpsc::Receiver<Frame>,
    pub status_rx: mpsc::Receiver<String>,
}

impl HyperVSession {
    pub fn start(vm: VmInfo) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<HyperVCmd>(32);
        let (frame_tx, frame_rx) = mpsc::sync_channel::<Frame>(2);
        let (status_tx, status_rx) = mpsc::sync_channel::<String>(8);

        std::thread::Builder::new()
            .name(format!("hyperv-{}", vm.name))
            .spawn(move || {
                let native = video_resolution(&vm.id).unwrap_or((1280, 720));
                // Кап разрешения thumbnail: на хостах без аппаратного энкодера
                // (OpenH264-SW) 1080p даёт encode_ms 200–2500 → <1 fps. Меньший
                // thumbnail Hyper-V рендерит сам → софт-энкод тянет реалтайм.
                let (width, height) = cap_resolution(native.0, native.1, 1280, 720);
                let _ = status_tx.try_send(format!(
                    "Hyper-V capture started: {}x{} (native {}x{})",
                    width, height, native.0, native.1
                ));
                loop {
                // Drain commands
                loop {
                    match cmd_rx.try_recv() {
                        Ok(HyperVCmd::Stop) => return,
                        Ok(HyperVCmd::PressKey(sc)) => press_key(&vm.id, sc),
                        Ok(HyperVCmd::ReleaseKey(sc)) => release_key(&vm.id, sc),
                        Ok(HyperVCmd::TypeText(t)) => type_text(&vm.id, &t),
                        Ok(HyperVCmd::MoveMouse(x, y)) => move_mouse(&vm.id, x, y),
                        Ok(HyperVCmd::ClickMouse(b, p)) => click_mouse(&vm.id, b, p),
                        Err(_) => break,
                    }
                }
                // Capture frame
                match capture_screen(&vm.id, &vm.wmi_path, width, height) {
                    Ok(rgba) => {
                        let _ = frame_tx.try_send(Frame {
                            rgba,
                            width: width as u32,
                            height: height as u32,
                        });
                    }
                    Err(err) => {
                        let _ = status_tx.try_send(err);
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
                }
            })
            .expect("hyperv thread");

        HyperVSession {
            cmd_tx,
            frame_rx,
            status_rx,
        }
    }

    pub fn send(&self, cmd: HyperVCmd) {
        let _ = self.cmd_tx.try_send(cmd);
    }

    pub fn try_recv_frame(&self) -> Option<Frame> {
        self.frame_rx.try_recv().ok()
    }

    pub fn try_recv_status(&self) -> Option<String> {
        self.status_rx.try_recv().ok()
    }

    pub fn stop(self) {
        let _ = self.cmd_tx.try_send(HyperVCmd::Stop);
    }
}
