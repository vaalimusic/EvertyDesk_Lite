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

// ── VM provider / state ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum VmProvider {
    HyperV,
    VirtualBox,
    VMware,
}

impl VmProvider {
    pub fn label(&self) -> &str {
        match self {
            Self::HyperV => "Hyper-V",
            Self::VirtualBox => "VirtualBox",
            Self::VMware => "VMware",
        }
    }
    /// True if we can open an in-app console for this provider.
    pub fn has_console(&self) -> bool {
        matches!(self, Self::HyperV)
    }
}

/// Console capability of a VM — determines which interactive modes are available.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsoleMode {
    /// Integration Services installed + Enhanced Session enabled → RDP over VMBus (best)
    EnhancedSession,
    /// No IS / Enhanced Session disabled → WMI thumbnail preview only (limited)
    ThumbnailOnly,
    /// Not a Hyper-V VM (VirtualBox, VMware, etc.)
    Other,
}

impl ConsoleMode {
    pub fn label(&self) -> &str {
        match self {
            Self::EnhancedSession => "Enhanced Session",
            Self::ThumbnailOnly => "Превью только",
            Self::Other => "",
        }
    }
    pub fn supports_rdp(&self) -> bool {
        matches!(self, Self::EnhancedSession)
    }
}

#[derive(Debug, Clone)]
pub struct VmInfo {
    pub name: String,
    /// GUID used as SystemName for keyboard/mouse WMI objects (Hyper-V) or VM path (others)
    pub id: String,
    /// Full WMI object path for method calls
    pub wmi_path: String,
    pub state: VmState,
    pub provider: VmProvider,
    pub console_mode: ConsoleMode,
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
    /// Returns output properties map. Тонкая Option-обёртка для вызовов,
    /// которым не нужна детальная ошибка (ввод).
    fn exec_method(
        &self,
        obj_path: &str,
        method: &str,
        in_params: &[(&str, WmiVal)],
    ) -> Option<HashMap<String, WmiVal>> {
        self.exec_method_result(obj_path, method, in_params).ok()
    }

    /// Детальная версия: возвращает конкретную ошибку с HRESULT на каждом шаге,
    /// чтобы было видно ЧТО именно упало (GetObject/GetMethod/Put/ExecMethod).
    fn exec_method_result(
        &self,
        obj_path: &str,
        method: &str,
        in_params: &[(&str, WmiVal)],
    ) -> Result<HashMap<String, WmiVal>, String> {
        unsafe {
            // Get the class to obtain the method input signature
            let class_name = obj_path_class(obj_path);
            let mut class_obj: Option<IWbemClassObject> = None;
            self.svc
                .GetObject(&BSTR::from(class_name), 0, None, Some(&mut class_obj), None)
                .map_err(|e| format!("GetObject({class_name}) {}", hr(&e)))?;
            let class_obj = class_obj.ok_or_else(|| format!("GetObject({class_name}) пусто"))?;

            // Get method in-param signature and spawn an instance
            let mut in_sig: Option<IWbemClassObject> = None;
            let mut _out_sig: Option<IWbemClassObject> = None;
            let method_w = wide_null(method);
            class_obj
                .GetMethod(PCWSTR(method_w.as_ptr()), 0, &mut in_sig, &mut _out_sig)
                .map_err(|e| format!("GetMethod({method}) {}", hr(&e)))?;

            let in_instance = in_sig
                .ok_or_else(|| format!("GetMethod({method}) нет in-сигнатуры"))?
                .SpawnInstance(0)
                .map_err(|e| format!("SpawnInstance {}", hr(&e)))?;

            // Fill in-params
            for (name, val) in in_params {
                let name_w = wide_null(name);
                let var = wmi_val_to_variant(val);
                in_instance
                    .Put(PCWSTR(name_w.as_ptr()), 0, &var, 0)
                    .map_err(|e| format!("Put({name}) {}", hr(&e)))?;
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
                .map_err(|e| format!("ExecMethod({method}) {}", hr(&e)))?;

            let out = out_obj.ok_or_else(|| format!("ExecMethod({method}) пустой результат"))?;
            Ok(read_all_properties(&out))
        }
    }
}

/// Краткий HRESULT в hex + сообщение.
fn hr(e: &windows::core::Error) -> String {
    format!("hr=0x{:08X}", e.code().0 as u32)
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
        match self {
            Self::U32(v) => Some(*v),
            Self::U16(v) => Some(*v as u32),
            // WMI часто отдаёт CIM uint16/uint32 (EnabledState, разрешение видео)
            // как VARIANT VT_I4 → WmiVal::I32. Без этого state = Other(0),
            // connectable = false, кнопка «Подключиться» неактивна.
            Self::I32(v) => u32::try_from(*v).ok(),
            _ => None,
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

/// Extract the class name from a WMI object path.
///
/// Handles both absolute and relative paths:
///   Absolute: `\\SERVER\ROOT\virtualization\v2:Msvm_Keyboard.CreationClassName=...`
///             → find `:`, then take up to first `.`  → `Msvm_Keyboard`
///   Relative: `Msvm_Keyboard.CreationClassName="Msvm_Keyboard",DeviceID="Microsoft:abc"`
///             → DeviceID contains `:` — DO NOT use find(':') on relative path!
///             → Just take up to first `.`            → `Msvm_Keyboard`
///
/// BUG HISTORY: The old code always did find(':') first. On relative paths this found
/// the `:` INSIDE `"Microsoft:GUID"` in the DeviceID value, producing garbage as the
/// class name → GetObject failed → ALL keyboard/mouse WMI calls silently dropped.
fn obj_path_class(path: &str) -> &str {
    if path.starts_with("\\\\") {
        // Absolute path: \\SERVER\namespace:ClassName.key=val
        // The ONE legitimate colon is the namespace separator before the class name.
        let after_colon = path.find(':').map(|i| &path[i + 1..]).unwrap_or(path);
        after_colon.find('.').map(|i| &after_colon[..i]).unwrap_or(after_colon)
    } else {
        // Relative path: ClassName.key="val",key2="Microsoft:something"
        // Class name is everything before the first dot.
        path.find('.').map(|i| &path[..i]).unwrap_or(path)
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

// ── Public API ────────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub fn is_available() -> bool {
    Wmi::connect().is_some()
}

/// Discover VMs from all available hypervisors on this machine.
pub fn list_vms() -> Vec<VmInfo> {
    let mut all: Vec<VmInfo> = Vec::new();

    // Hyper-V (WMI root\virtualization\v2)
    let hv = {
        let mut v = list_vms_com();
        if v.is_empty() {
            v = list_vms_powershell();
        }
        v
    };
    all.extend(hv);

    // VirtualBox
    all.extend(list_virtualbox_vms());

    // VMware Workstation / Player
    all.extend(list_vmware_vms());

    all
}

fn list_virtualbox_vms() -> Vec<VmInfo> {
    let Some(vboxmanage) = find_vboxmanage() else {
        return Vec::new();
    };
    let Ok(all_out) = std::process::Command::new(&vboxmanage)
        .args(["list", "vms"])
        .output()
    else {
        return Vec::new();
    };
    let running_ids: std::collections::HashSet<String> = std::process::Command::new(&vboxmanage)
        .args(["list", "runningvms"])
        .output()
        .map(|o| parse_vbox_list(&o.stdout).into_iter().map(|(_, id)| id).collect())
        .unwrap_or_default();

    parse_vbox_list(&all_out.stdout)
        .into_iter()
        .map(|(name, id)| {
            let state = if running_ids.contains(&id) {
                VmState::Running
            } else {
                VmState::Off
            };
            VmInfo {
                name,
                wmi_path: id.clone(),
                id,
                state,
                provider: VmProvider::VirtualBox,
                console_mode: ConsoleMode::Other,
            }
        })
        .collect()
}

fn parse_vbox_list(output: &[u8]) -> Vec<(String, String)> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            // Format: "VM Name" {uuid}
            let (name_part, id_part) = line.rsplit_once(' ')?;
            let name = name_part.trim().trim_matches('"').to_owned();
            let id = id_part
                .trim()
                .trim_matches(|c| c == '{' || c == '}')
                .to_owned();
            if id.is_empty() || name.is_empty() {
                return None;
            }
            Some((name, id))
        })
        .collect()
}

fn find_vboxmanage() -> Option<String> {
    if std::process::Command::new("VBoxManage")
        .arg("--version")
        .output()
        .is_ok()
    {
        return Some("VBoxManage".to_owned());
    }
    for path in [
        r"C:\Program Files\Oracle\VirtualBox\VBoxManage.exe",
        r"C:\Program Files (x86)\Oracle\VirtualBox\VBoxManage.exe",
    ] {
        if std::path::Path::new(path).exists() {
            return Some(path.to_owned());
        }
    }
    None
}

fn list_vmware_vms() -> Vec<VmInfo> {
    let Some(vmrun) = find_vmrun() else {
        return Vec::new();
    };
    let Ok(out) = std::process::Command::new(&vmrun)
        .arg("list")
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| line.trim_end().ends_with(".vmx"))
        .map(|path| {
            let path = path.trim().to_owned();
            let name = std::path::Path::new(&path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&path)
                .to_owned();
            VmInfo {
                name,
                wmi_path: path.clone(),
                id: path,
                state: VmState::Running, // vmrun list only shows running VMs
                provider: VmProvider::VMware,
                console_mode: ConsoleMode::Other,
            }
        })
        .collect()
}

fn find_vmrun() -> Option<String> {
    if std::process::Command::new("vmrun")
        .arg("list")
        .output()
        .is_ok()
    {
        return Some("vmrun".to_owned());
    }
    for path in [
        r"C:\Program Files (x86)\VMware\VMware Workstation\vmrun.exe",
        r"C:\Program Files\VMware\VMware Workstation\vmrun.exe",
        r"C:\Program Files (x86)\VMware\VMware Player\vmrun.exe",
    ] {
        if std::path::Path::new(path).exists() {
            return Some(path.to_owned());
        }
    }
    None
}

/// Query Msvm_SummaryInformation for heartbeat status of all VMs.
/// Heartbeat == 2 means Integration Services are running → RDP over VMBus available.
fn query_heartbeat_map(wmi: &Wmi) -> HashMap<String, ConsoleMode> {
    let rows = wmi.query("SELECT Name, Heartbeat FROM Msvm_SummaryInformation");
    rows.into_iter()
        .filter_map(|mut row| {
            let name = row.remove("Name")?.as_str()?.to_owned();
            let heartbeat = row.remove("Heartbeat").and_then(|v| v.as_u32()).unwrap_or(0);
            // 2 = Ok (IS running), 6 = Error, 12 = NoContact, 0 = Unknown/not available
            let mode = if heartbeat == 2 {
                ConsoleMode::EnhancedSession
            } else {
                ConsoleMode::ThumbnailOnly
            };
            Some((name, mode))
        })
        .collect()
}

fn list_vms_com() -> Vec<VmInfo> {
    let Some(wmi) = Wmi::connect() else { return vec![] };
    // НЕ проецируем __PATH в SELECT: у virtualization-провайдера это может
    // вернуть 0 строк. read_all_properties дочитывает __PATH через obj.Get.
    let console_modes = query_heartbeat_map(&wmi);
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
            let console_mode = console_modes.get(&id).cloned().unwrap_or(ConsoleMode::ThumbnailOnly);
            Some(VmInfo {
                name: row.remove("ElementName")?.as_str()?.to_owned(),
                id,
                wmi_path,
                state: VmState::from_u32(
                    row.remove("EnabledState").and_then(|v| v.as_u32()).unwrap_or(0),
                ),
                provider: VmProvider::HyperV,
                console_mode,
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
                provider: VmProvider::HyperV,
                console_mode: ConsoleMode::ThumbnailOnly,
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

/// WMI RequestedState values for Msvm_ComputerSystem.RequestStateChange.
#[derive(Debug, Clone, PartialEq)]
pub enum VmPowerAction {
    /// Start (Enabled) — RequestedState=2
    Start,
    /// Hard power-off — RequestedState=3
    Stop,
    /// Graceful shutdown (via Integration Services) — RequestedState=4
    Shutdown,
    /// Reboot (stop then start) — RequestedState=11 or stop+start
    Restart,
    /// Pause — RequestedState=9
    Pause,
    /// Resume from pause — RequestedState=2 (same as Start)
    Resume,
    /// Save state — RequestedState=32770
    Save,
}

impl VmPowerAction {
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "start"    => Self::Start,
            "stop"     => Self::Stop,
            "shutdown" => Self::Shutdown,
            "restart"  => Self::Restart,
            "pause"    => Self::Pause,
            "resume"   => Self::Resume,
            "save"     => Self::Save,
            _ => return None,
        })
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::Start    => "Start",
            Self::Stop     => "Force Off",
            Self::Shutdown => "Shutdown",
            Self::Restart  => "Restart",
            Self::Pause    => "Pause",
            Self::Resume   => "Resume",
            Self::Save     => "Save",
        }
    }
}

/// Send a power action to a Hyper-V VM. Non-blocking — fires and forgets.
pub fn request_power_action(vm_wmi_path: &str, action: VmPowerAction) {
    let path = vm_wmi_path.to_owned();
    std::thread::spawn(move || {
        let Some(wmi) = Wmi::connect() else { return; };
        match action {
            VmPowerAction::Start    => { let _ = wmi.exec_method_result(&path, "RequestStateChange", &[("RequestedState", WmiVal::I32(2))]); }
            VmPowerAction::Stop     => { let _ = wmi.exec_method_result(&path, "RequestStateChange", &[("RequestedState", WmiVal::I32(3))]); }
            VmPowerAction::Shutdown => { let _ = wmi.exec_method_result(&path, "RequestStateChange", &[("RequestedState", WmiVal::I32(4))]); }
            VmPowerAction::Pause    => { let _ = wmi.exec_method_result(&path, "RequestStateChange", &[("RequestedState", WmiVal::I32(9))]); }
            VmPowerAction::Resume   => { let _ = wmi.exec_method_result(&path, "RequestStateChange", &[("RequestedState", WmiVal::I32(2))]); }
            VmPowerAction::Save     => { let _ = wmi.exec_method_result(&path, "RequestStateChange", &[("RequestedState", WmiVal::I32(32770))]); }
            VmPowerAction::Restart  => {
                // Stop then start — both async, give stop time to complete
                let _ = wmi.exec_method_result(&path, "RequestStateChange", &[("RequestedState", WmiVal::I32(3))]);
                std::thread::sleep(Duration::from_secs(4));
                let _ = wmi.exec_method_result(&path, "RequestStateChange", &[("RequestedState", WmiVal::I32(2))]);
            }
        }
    });
}

// ── Checkpoints (Snapshots) ───────────────────────────────────────────────────

/// Информация о чекпоинте (снапшоте) VM.
#[derive(Debug, Clone)]
pub struct CheckpointInfo {
    /// Имя чекпоинта (ElementName).
    pub name: String,
    /// WMI-путь объекта Msvm_VirtualSystemSettingData (для Apply/Delete).
    pub wmi_path: String,
    /// Время создания (строка из WMI — ISO или Windows format).
    pub created_time: String,
    /// Тип: "Standard" | "Production" | "ProductionFallback" | "Unknown".
    pub checkpoint_type: String,
    /// InstanceID (стабильный идентификатор).
    pub instance_id: String,
}

/// Получить список чекпоинтов VM.
/// Возвращает пустой вектор если VM не найдена, WMI недоступна, или чекпоинтов нет.
pub fn list_checkpoints(vm_id: &str) -> Vec<CheckpointInfo> {
    let Some(wmi) = Wmi::connect() else { return Vec::new() };
    // Чекпоинты — Msvm_VirtualSystemSettingData с VirtualSystemIdentifier = vm_guid
    // И VirtualSystemType != 'Microsoft:Hyper-V:System:Realized' (это основная конфигурация).
    // Snapshot types: 'Microsoft:Hyper-V:Snapshot:Realized', 'Microsoft:Hyper-V:Snapshot:Recovery'
    let q = format!(
        "SELECT * FROM Msvm_VirtualSystemSettingData \
         WHERE VirtualSystemIdentifier = '{}' \
         AND VirtualSystemType <> 'Microsoft:Hyper-V:System:Realized'",
        escape_wmi_value(vm_id)
    );
    let rows = wmi.query(&q);
    rows.into_iter()
        .filter_map(|mut row| {
            let name = row
                .remove("ElementName")
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "Checkpoint".to_owned());
            let path = take_non_empty_string(&mut row, "__PATH")
                .or_else(|| take_non_empty_string(&mut row, "__RELPATH"))?;
            let created_time = row
                .remove("CreationTime")
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            let instance_id = row
                .remove("InstanceID")
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            let vm_type = row
                .remove("VirtualSystemType")
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            let checkpoint_type = classify_checkpoint_type(&vm_type);
            Some(CheckpointInfo {
                name,
                wmi_path: path,
                created_time,
                checkpoint_type,
                instance_id,
            })
        })
        .collect()
}

fn classify_checkpoint_type(vm_type: &str) -> String {
    if vm_type.contains("Snapshot:Realized") {
        "Standard".to_owned()
    } else if vm_type.contains("Snapshot:Recovery") {
        "Production".to_owned()
    } else if vm_type.contains("Snapshot") {
        "Snapshot".to_owned()
    } else {
        "Unknown".to_owned()
    }
}

/// Получить путь сервиса снапшотов (Msvm_VirtualSystemSnapshotService).
fn snapshot_service_path(wmi: &Wmi) -> Option<String> {
    if let Some(mut row) = wmi
        .query("SELECT * FROM Msvm_VirtualSystemSnapshotService")
        .into_iter()
        .next()
    {
        if let Some(path) = take_non_empty_string(&mut row, "__PATH")
            .or_else(|| take_non_empty_string(&mut row, "__RELPATH"))
        {
            return Some(path);
        }
    }
    // Fallback: singleton с Name="vsss"
    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".to_owned());
    Some(format!(
        "Msvm_VirtualSystemSnapshotService.CreationClassName=\"Msvm_VirtualSystemSnapshotService\",\
Name=\"vsss\",SystemCreationClassName=\"Msvm_ComputerSystem\",SystemName=\"{}\"",
        escape_wmi_key(&host)
    ))
}

/// Создать стандартный чекпоинт для VM.
/// vm_wmi_path — WMI-путь Msvm_ComputerSystem VM.
/// name — имя чекпоинта (если None, используется дата/время).
/// Возвращает имя созданного чекпоинта или ошибку.
pub fn create_checkpoint(vm_wmi_path: &str, name: Option<&str>) -> Result<String, String> {
    let wmi = Wmi::connect().ok_or_else(|| "WMI connect failed".to_owned())?;
    let svc_path = snapshot_service_path(&wmi)
        .ok_or_else(|| "Msvm_VirtualSystemSnapshotService not found".to_owned())?;

    // SnapshotType: 2 = Standard, 3 = Production
    let in_params: Vec<(&str, WmiVal)> = vec![
        ("AffectedSystem", WmiVal::Str(vm_wmi_path.to_owned())),
        ("SnapshotType", WmiVal::I32(2)),
    ];

    let _out = wmi
        .exec_method_result(&svc_path, "CreateSnapshot", &in_params)
        .map_err(|e| format!("CreateSnapshot: {e}"))?;

    let display_name = name
        .unwrap_or("Checkpoint")
        .to_owned();
    Ok(display_name)
}

/// Применить (восстановить) чекпоинт.
/// snapshot_wmi_path — WMI-путь Msvm_VirtualSystemSettingData (из CheckpointInfo.wmi_path).
pub fn apply_checkpoint(snapshot_wmi_path: &str) -> Result<(), String> {
    let wmi = Wmi::connect().ok_or_else(|| "WMI connect failed".to_owned())?;
    let svc_path = snapshot_service_path(&wmi)
        .ok_or_else(|| "Msvm_VirtualSystemSnapshotService not found".to_owned())?;
    wmi.exec_method_result(
        &svc_path,
        "ApplySnapshot",
        &[("Snapshot", WmiVal::Str(snapshot_wmi_path.to_owned()))],
    )
    .map(|_| ())
    .map_err(|e| format!("ApplySnapshot: {e}"))
}

/// Удалить чекпоинт и его дерево (если есть дочерние снапшоты).
pub fn delete_checkpoint(snapshot_wmi_path: &str) -> Result<(), String> {
    let wmi = Wmi::connect().ok_or_else(|| "WMI connect failed".to_owned())?;
    let svc_path = snapshot_service_path(&wmi)
        .ok_or_else(|| "Msvm_VirtualSystemSnapshotService not found".to_owned())?;
    wmi.exec_method_result(
        &svc_path,
        "DestroySnapshot",
        &[("AffectedSnapshot", WmiVal::Str(snapshot_wmi_path.to_owned()))],
    )
    .map(|_| ())
    .map_err(|e| format!("DestroySnapshot: {e}"))
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

    let out = wmi
        .exec_method_result(
            &svc_path,
            "GetVirtualSystemThumbnailImage",
            // WidthPixels/HeightPixels — CIM uint16, но этот провайдер требует
            // VARIANT VT_I4 (VT_UI2 → WBEM_E_TYPE_MISMATCH 0x80041005).
            &[
                ("TargetSystem", WmiVal::Str(setting_path.clone())),
                ("WidthPixels", WmiVal::I32(width as i32)),
                ("HeightPixels", WmiVal::I32(height as i32)),
            ],
        )
        .map_err(|e| format!("GetThumbnail [{e}]; svc={svc_path}; target={setting_path}"))?;

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
    // Сначала — «realized» настройка по фильтру (GetThumbnail требует именно её,
    // а не snapshot-настройку, которую ASSOCIATORS может вернуть первой).
    let q = format!(
        "SELECT * FROM Msvm_VirtualSystemSettingData \
         WHERE VirtualSystemIdentifier = '{}' \
         AND VirtualSystemType = 'Microsoft:Hyper-V:System:Realized'",
        escape_wmi_value(vm_id)
    );
    if let Some(mut row) = wmi.query(&q).into_iter().next() {
        if let Some(path) = take_non_empty_string(&mut row, "__PATH")
            .or_else(|| take_non_empty_string(&mut row, "__RELPATH"))
            .or_else(|| {
                take_non_empty_string(&mut row, "InstanceID")
                    .map(|instance_id| virtual_system_setting_path(&instance_id))
            })
        {
            return Some(path);
        }
    }

    // Fallback: первая ассоциированная настройка VM.
    let q = format!(
        "ASSOCIATORS OF {{{vm_wmi_path}}} WHERE ResultClass = Msvm_VirtualSystemSettingData"
    );
    let mut row = wmi.query(&q).into_iter().next()?;
    take_non_empty_string(&mut row, "__PATH")
        .or_else(|| take_non_empty_string(&mut row, "__RELPATH"))
}

/// Ограничить разрешение thumbnail рамкой max_w×max_h с сохранением пропорций.
/// Чётные размеры (требование энкодеров H264). Если уже меньше — без изменений.
fn cap_resolution(w: u16, h: u16, max_w: u16, max_h: u16) -> (u16, u16) {
    if w == 0 || h == 0 {
        return (max_w, max_h);
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

/// Отправить Ctrl+Alt+Del в VM (через WMI Msvm_Keyboard.TypeCtrlAltDel).
pub fn send_ctrl_alt_del(vm_id: &str) -> Result<(), String> {
    let wmi = Wmi::connect().ok_or_else(|| "WMI connect failed".to_owned())?;
    let path = find_device_path(&wmi, "Msvm_Keyboard", vm_id)
        .ok_or_else(|| "Msvm_Keyboard не найден".to_owned())?;
    wmi.exec_method_result(&path, "TypeCtrlAltDel", &[])
        .map(|_| ())
}

pub fn press_key(vm_id: &str, scan_code: u32) -> Result<(), String> {
    kbd_exec(vm_id, "PressKey", scan_code)
}

pub fn release_key(vm_id: &str, scan_code: u32) -> Result<(), String> {
    kbd_exec(vm_id, "ReleaseKey", scan_code)
}

pub fn type_text(vm_id: &str, text: &str) -> Result<(), String> {
    let wmi = Wmi::connect().ok_or_else(|| "WMI connect failed".to_owned())?;
    let path = find_device_path(&wmi, "Msvm_Keyboard", vm_id)
        .ok_or_else(|| "Msvm_Keyboard не найден".to_owned())?;
    wmi.exec_method_result(&path, "TypeText", &[("asciiText", WmiVal::Str(text.to_owned()))])
        .map(|_| ())
}

fn kbd_exec(vm_id: &str, method: &str, scan_code: u32) -> Result<(), String> {
    let wmi = Wmi::connect().ok_or_else(|| "WMI connect failed".to_owned())?;
    let path = find_device_path(&wmi, "Msvm_Keyboard", vm_id)
        .ok_or_else(|| "Msvm_Keyboard не найден".to_owned())?;
    // VT_I4: провайдер виртуализации отвергает VT_UI4 (WBEM_E_TYPE_MISMATCH).
    // Msvm_Keyboard::PressKey/ReleaseKey parameter is named "keyCode" (NOT "scanCode")
    wmi.exec_method_result(&path, method, &[("keyCode", WmiVal::I32(scan_code as i32))])
        .map(|_| ())
}

/// x, y in 0–65535 (normalized to VM display).
pub fn move_mouse(vm_id: &str, x: u32, y: u32) -> Result<(), String> {
    let wmi = Wmi::connect().ok_or_else(|| "WMI connect failed".to_owned())?;
    let path = find_device_path(&wmi, "Msvm_Mouse", vm_id)
        .ok_or_else(|| "Msvm_Mouse не найден".to_owned())?;
    wmi.exec_method_result(
        &path,
        "SetAbsolutePosition",
        &[
            ("horizontalPosition", WmiVal::I32(x as i32)),
            ("verticalPosition", WmiVal::I32(y as i32)),
        ],
    )
    .map(|_| ())
}

/// button: 1=left, 2=right, 3=middle.
pub fn click_mouse(vm_id: &str, button: u32, press: bool) -> Result<(), String> {
    let wmi = Wmi::connect().ok_or_else(|| "WMI connect failed".to_owned())?;
    let path = find_device_path(&wmi, "Msvm_Mouse", vm_id)
        .ok_or_else(|| "Msvm_Mouse не найден".to_owned())?;
    let method = if press { "ClickButton" } else { "ReleaseButton" };
    wmi.exec_method_result(&path, method, &[("buttonIndex", WmiVal::I32(button as i32))])
        .map(|_| ())
}

fn find_device_path(wmi: &Wmi, class: &str, vm_id: &str) -> Option<String> {
    // Try WHERE first, then enumerate all and filter (some WMI providers ignore WHERE)
    let candidates = [
        format!("SELECT * FROM {class} WHERE SystemName = '{vm_id}'"),
        format!("SELECT * FROM {class}"),
    ];
    for q in &candidates {
        let rows = wmi.query(q);
        for mut row in rows {
            // Filter by SystemName when scanning all
            if q.contains("SELECT * FROM") && !q.contains("WHERE") {
                let sn = row.get("SystemName").and_then(|v| v.as_str().map(|s| s.to_owned()));
                if sn.as_deref() != Some(vm_id) {
                    continue;
                }
            }
            // Prefer __PATH, fall back to __RELPATH, else build from DeviceID
            if let Some(p) = row.get("__PATH").and_then(|v| v.as_str().map(|s| s.to_owned()))
                .filter(|s| !s.is_empty())
            {
                return Some(p);
            }
            if let Some(p) = row.get("__RELPATH").and_then(|v| v.as_str().map(|s| s.to_owned()))
                .filter(|s| !s.is_empty())
            {
                return Some(p);
            }
            // Build path manually from key properties
            if let Some(dev_id) = row.get("DeviceID").and_then(|v| v.as_str().map(|s| s.to_owned())) {
                let path = format!(
                    "{class}.CreationClassName=\"{class}\",DeviceID=\"{}\",\
                     SystemCreationClassName=\"Msvm_ComputerSystem\",SystemName=\"{vm_id}\"",
                    dev_id.replace('"', "\"\"")
                );
                return Some(path);
            }
        }
        // If WHERE-filtered query returned results, stop here regardless
        if q.contains("WHERE") && !wmi.query(q).is_empty() {
            break;
        }
    }
    None
}

/// Pre-resolve keyboard/mouse WMI paths at session start (once per session).
struct DevicePaths {
    keyboard: Option<String>,
    mouse: Option<String>,
}

impl DevicePaths {
    fn resolve(vm_id: &str) -> (Self, Option<String>) {
        let Some(wmi) = Wmi::connect() else {
            return (Self { keyboard: None, mouse: None }, Some("WMI connect failed".into()));
        };
        let keyboard = find_device_path(&wmi, "Msvm_Keyboard", vm_id);
        let mouse = find_device_path(&wmi, "Msvm_Mouse", vm_id);
        let warn = if mouse.is_none() {
            Some("Msvm_Mouse не найден — управление мышью недоступно".into())
        } else {
            None
        };
        (Self { keyboard, mouse }, warn)
    }
}

// ── Background session ────────────────────────────────────────────────────────

pub enum HyperVCmd {
    PressKey(u32),
    ReleaseKey(u32),
    TypeText(String),
    CtrlAltDel,
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
    /// Shared stop flag — set to true when session ends; capture-thread checks this each cycle.
    stop_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl HyperVSession {
    /// Запускает две нити:
    ///
    /// • **input-thread** — принимает команды из `cmd_rx` и немедленно
    ///   отправляет WMI-методы клавиатуры/мыши. Никогда не блокируется
    ///   на захвате кадра. Задержка ввода = только время WMI round-trip
    ///   (~20–80 ms), а не WMI-capture + sleep.
    ///
    /// • **capture-thread** — с целевым интервалом 50 ms захватывает кадры
    ///   через GetVirtualSystemThumbnailImage. Адаптивно снижает разрешение
    ///   при медленном WMI. Публикует кадры в `frame_tx`.
    ///
    /// Разделение устраняет главную причину задержек: ранее `PressKey` ждал
    /// завершения `capture_screen()` (50–500 ms), теперь — нет.
    pub fn start(vm: VmInfo) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<HyperVCmd>(64);
        let (frame_tx, frame_rx) = mpsc::sync_channel::<Frame>(2);
        let (status_tx, status_rx) = mpsc::sync_channel::<String>(16);
        let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_flag_cap = stop_flag.clone();

        // ── Input thread ─────────────────────────────────────────────────────
        // Выделенный поток только для WMI-ввода — не блокируется захватом кадров.
        //
        // FIX-1: Retry keyboard path — Msvm_Keyboard может быть недоступен
        //   при старте VM (Integration Services ещё не готов). Повторяем до 5 раз.
        //
        // FIX-2: Батчевый TypeText — сначала блокируемся на recv(), потом drain
        //   все уже ожидающие TypeText в одну строку → один WMI-вызов вместо N.
        //   Для быстрой печати это снижает задержку с N×80ms до 1×80ms.
        //
        // FIX-3: Modifier + кириллица — если ascii_scancode не знает символ,
        //   пробуем cyrillic_to_vk (позиционная таблица RU-JCUKEN→EN).
        //   Ctrl+А → PressKey(VK_CONTROL)+PressKey(VK_F) вместо TypeText("а").
        let vm_id_input = vm.id.clone();
        let status_tx_input = status_tx.clone();
        std::thread::Builder::new()
            .name(format!("hyperv-input-{}", vm.name))
            .spawn(move || {
                // FIX-1: Retry until Msvm_Keyboard path is resolved.
                let wmi = Wmi::connect();
                let mut paths = {
                    let (p, warn) = DevicePaths::resolve(&vm_id_input);
                    if let Some(w) = warn { let _ = status_tx_input.try_send(w); }
                    p
                };
                if paths.keyboard.is_none() {
                    let _ = status_tx_input.try_send(
                        "⚠ Msvm_Keyboard не найден, жду Integration Services…".into()
                    );
                    for attempt in 1u32..=5 {
                        std::thread::sleep(Duration::from_secs(3));
                        let (p2, warn2) = DevicePaths::resolve(&vm_id_input);
                        if let Some(w) = warn2 { let _ = status_tx_input.try_send(w); }
                        if p2.keyboard.is_some() {
                            paths = p2;
                            let _ = status_tx_input.try_send(format!(
                                "✓ Msvm_Keyboard найден (попытка {}) — ввод активен", attempt + 1
                            ));
                            break;
                        }
                        if attempt == 5 {
                            let _ = status_tx_input.try_send(
                                "✗ Msvm_Keyboard недоступен — клавиатура WMI не работает. \
                                 Попробуйте Enhanced Session или переключитесь в BasicRescue позже.".into()
                            );
                        }
                    }
                }

                // Helper: flush батч TypeText за один WMI-вызов.
                // Логирует WMI ошибку чтобы было видно ПОЧЕМУ символы не вводятся.
                let flush_text = |batch: &mut String, wmi: &Option<Wmi>, kbd: &Option<String>| {
                    if batch.is_empty() { return; }
                    if let (Some(wmi), Some(p)) = (wmi, kbd) {
                        if let Err(e) = wmi.exec_method_result(
                            p, "TypeText",
                            &[("asciiText", WmiVal::Str(batch.clone()))],
                        ) {
                            let _ = status_tx_input.try_send(
                                format!("⚠ TypeText WMI err: {e}")
                            );
                        }
                    } else if kbd.is_none() {
                        let _ = status_tx_input.try_send(
                            "⚠ ввод: Msvm_Keyboard path = None, символы теряются".into()
                        );
                    }
                    batch.clear();
                };

                loop {
                    // Ждём хотя бы одну команду (не спим попусту).
                    let first = match cmd_rx.recv() {
                        Ok(c) => c,
                        Err(_) => return,
                    };

                    // FIX-2: Накапливаем все уже лежащие команды в буфер.
                    let mut pending = vec![first];
                    loop {
                        match cmd_rx.try_recv() {
                            Ok(c) => pending.push(c),
                            Err(_) => break,
                        }
                    }

                    // Обрабатываем пачку: TypeText-ы батчуем, остальное — сразу.
                    let mut text_batch = String::new();
                    let mut stop = false;

                    for cmd in pending {
                        match cmd {
                            HyperVCmd::Stop => { stop = true; break; }

                            // FIX-2: Не отправляем сразу — копим в text_batch.
                            HyperVCmd::TypeText(t) => {
                                text_batch.push_str(&t);
                            }

                            // Перед любым non-text событием сначала сбрасываем текст.
                            HyperVCmd::PressKey(sc) => {
                                flush_text(&mut text_batch, &wmi, &paths.keyboard);
                                if let (Some(wmi), Some(p)) = (&wmi, &paths.keyboard) {
                                    if let Err(e) = wmi.exec_method_result(
                                        p, "PressKey",
                                        &[("keyCode", WmiVal::I32(sc as i32))],
                                    ) {
                                        let _ = status_tx_input.try_send(
                                            format!("⚠ PressKey(0x{sc:02X}) WMI err: {e}")
                                        );
                                    }
                                }
                            }
                            HyperVCmd::ReleaseKey(sc) => {
                                flush_text(&mut text_batch, &wmi, &paths.keyboard);
                                if let (Some(wmi), Some(p)) = (&wmi, &paths.keyboard) {
                                    if let Err(e) = wmi.exec_method_result(
                                        p, "ReleaseKey",
                                        &[("keyCode", WmiVal::I32(sc as i32))],
                                    ) {
                                        let _ = status_tx_input.try_send(
                                            format!("⚠ ReleaseKey(0x{sc:02X}) WMI err: {e}")
                                        );
                                    }
                                }
                            }
                            HyperVCmd::CtrlAltDel => {
                                flush_text(&mut text_batch, &wmi, &paths.keyboard);
                                if let (Some(wmi), Some(p)) = (&wmi, &paths.keyboard) {
                                    let _ = wmi.exec_method_result(p, "TypeCtrlAltDel", &[]);
                                }
                            }
                            HyperVCmd::MoveMouse(x, y) => {
                                flush_text(&mut text_batch, &wmi, &paths.keyboard);
                                if let (Some(wmi), Some(p)) = (&wmi, &paths.mouse) {
                                    let _ = wmi.exec_method_result(
                                        p, "SetAbsolutePosition",
                                        &[
                                            ("horizontalPosition", WmiVal::I32(x as i32)),
                                            ("verticalPosition",   WmiVal::I32(y as i32)),
                                        ],
                                    );
                                }
                            }
                            HyperVCmd::ClickMouse(b, press) => {
                                flush_text(&mut text_batch, &wmi, &paths.keyboard);
                                if let (Some(wmi), Some(p)) = (&wmi, &paths.mouse) {
                                    let method = if press { "ClickButton" } else { "ReleaseButton" };
                                    let _ = wmi.exec_method_result(
                                        p, method,
                                        &[("buttonIndex", WmiVal::I32(b as i32))],
                                    );
                                }
                            }
                        }
                    }

                    // Сбрасываем оставшийся текст-батч.
                    flush_text(&mut text_batch, &wmi, &paths.keyboard);
                    if stop { return; }
                }
            })
            .expect("hyperv-input thread");

        // ── Capture thread ───────────────────────────────────────────────────
        // Handles only GetVirtualSystemThumbnailImage; never touches input.
        let vm_clone = vm;
        std::thread::Builder::new()
            .name(format!("hyperv-cap-{}", vm_clone.name))
            .spawn(move || {
                let native = video_resolution(&vm_clone.id).unwrap_or((1280, 720));
                // 1280×720 даёт читаемый шрифт; адаптивный даунскейл снизит
                // разрешение автоматически если WMI будет медленнее 500 ms.
                let (width, height) = cap_resolution(native.0, native.1, 1280, 720);
                let _ = status_tx.try_send(format!(
                    "Hyper-V capture started: {}×{} (native {}×{})",
                    width, height, native.0, native.1
                ));

                const FRAME_TARGET_MS: u64 = 50;
                let mut cur_w = width;
                let mut cur_h = height;
                let mut capture_ema_ms: f32 = 100.0;
                let mut frame_count: u32 = 0;
                let mut fps_window_start = std::time::Instant::now();

                loop {
                    // Capture thread exits when stop_flag is set (by HyperVSession::stop).
                    if stop_flag_cap.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }

                    let t0 = std::time::Instant::now();

                    let cap_t0 = std::time::Instant::now();
                    match capture_screen(&vm_clone.id, &vm_clone.wmi_path, cur_w, cur_h) {
                        Ok(rgba) => {
                            let cap_ms = cap_t0.elapsed().as_millis() as f32;
                            capture_ema_ms = capture_ema_ms * 0.75 + cap_ms * 0.25;

                            // Adaptive resolution
                            if capture_ema_ms > 500.0 && cur_w > 320 {
                                cur_w = (cur_w / 2).max(320);
                                cur_h = (cur_h / 2).max(240);
                                let _ = status_tx.try_send(format!(
                                    "WMI медленно ({:.0}ms) — снижаю разрешение до {}×{}",
                                    capture_ema_ms, cur_w, cur_h
                                ));
                                capture_ema_ms = 300.0;
                            } else if capture_ema_ms < 200.0 && cur_w < width {
                                cur_w = (cur_w * 3 / 2).min(width);
                                cur_h = (cur_h * 3 / 2).min(height);
                            }

                            // FPS counter every 5s
                            frame_count += 1;
                            let fps_elapsed = fps_window_start.elapsed();
                            if fps_elapsed >= Duration::from_secs(5) {
                                let fps = frame_count as f32 / fps_elapsed.as_secs_f32();
                                let _ = status_tx.try_send(format!(
                                    "● VM активна | {:.1} fps | {}×{} | WMI {:.0}ms",
                                    fps, cur_w, cur_h, capture_ema_ms
                                ));
                                frame_count = 0;
                                fps_window_start = std::time::Instant::now();
                            }

                            let _ = frame_tx.try_send(Frame {
                                rgba,
                                width: cur_w as u32,
                                height: cur_h as u32,
                            });
                        }
                        Err(err) => {
                            let _ = status_tx.try_send(err);
                        }
                    }

                    let elapsed = t0.elapsed();
                    let target = Duration::from_millis(FRAME_TARGET_MS);
                    if elapsed < target {
                        std::thread::sleep(target - elapsed);
                    }
                }
            })
            .expect("hyperv-cap thread");

        HyperVSession {
            cmd_tx,
            frame_rx,
            status_rx,
            stop_flag,
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
        // Signal capture-thread to exit on its next loop iteration.
        self.stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        // Signal input-thread to exit (unblocks recv()).
        let _ = self.cmd_tx.try_send(HyperVCmd::Stop);
        // Dropping cmd_tx also causes input-thread recv() to return Err → exit.
    }
}

// ── HyperVProvider — VirtualizationProvider wrapper ──────────────────────────
//
// Wraps the existing hyperv.rs functions as a VirtualizationProvider so that
// Broker / ProviderRegistry can treat Hyper-V the same as VMware or Proxmox.
// ADR-001: Core works with Provider API, not hypervisor-specific APIs.
// §47.4–47.5: Wrap existing code, do not rewrite from scratch.

/// Provider ID used when registering in ProviderRegistry.
pub const HYPERV_PROVIDER_ID: &str = "local-hyperv";

pub struct HyperVProvider {
    provider_id: String,
}

impl HyperVProvider {
    pub fn new() -> Self {
        Self {
            provider_id: HYPERV_PROVIDER_ID.to_owned(),
        }
    }
    pub fn with_id(id: &str) -> Self {
        Self { provider_id: id.to_owned() }
    }

    /// Convert hyperv::VmInfo to universal provider_api::VmInfo.
    fn to_universal_vm(vm: &VmInfo) -> crate::provider_api::VmInfo {
        use crate::provider_api::{PowerState, ProviderType, ToolsStatus, VmInfo as UVmInfo};
        let power_state = match vm.state {
            VmState::Running => PowerState::Running,
            VmState::Off => PowerState::Stopped,
            VmState::Paused => PowerState::Paused,
            VmState::Saved => PowerState::Saved,
            VmState::Other(_) => PowerState::Unknown,
        };
        let provider_type = match vm.provider {
            VmProvider::HyperV => ProviderType::HyperV,
            VmProvider::VMware => ProviderType::VMware,
            VmProvider::VirtualBox => ProviderType::Custom("VirtualBox".to_owned()),
        };
        let tools_status = if matches!(vm.console_mode, ConsoleMode::EnhancedSession) {
            ToolsStatus::Running
        } else if matches!(vm.provider, VmProvider::HyperV) {
            ToolsStatus::Unknown
        } else {
            ToolsStatus::NotApplicable
        };
        let enhanced_state = match vm.console_mode {
            ConsoleMode::EnhancedSession => "available",
            ConsoleMode::ThumbnailOnly => "unavailable",
            ConsoleMode::Other => "not_applicable",
        };
        UVmInfo {
            vm_id: format!("hyperv:{}", vm.id),
            provider_native_id: vm.id.clone(),
            name: vm.name.clone(),
            provider_type,
            host_id: "local".to_owned(),
            cluster: None,
            power_state,
            guest_ips: vec![],
            tools_status,
            tags: vec![],
            provider_metadata: serde_json::json!({
                "hyperv": {
                    "wmi_path": vm.wmi_path,
                    "console_mode": vm.console_mode.label(),
                    "enhanced_session_state": enhanced_state,
                }
            }),
        }
    }

    /// Convert universal SnapshotInfo from a CheckpointInfo.
    fn checkpoint_to_snapshot(
        cp: &CheckpointInfo,
        vm_id: &str,
    ) -> crate::provider_api::SnapshotInfo {
        crate::provider_api::SnapshotInfo {
            snapshot_id: format!("hyperv:{}", cp.instance_id),
            provider_native_id: cp.instance_id.clone(),
            vm_id: vm_id.to_owned(),
            name: cp.name.clone(),
            description: None,
            created_at: cp.created_time.clone(),
            is_current: false,
            snapshot_type: cp.checkpoint_type.clone(),
            provider_metadata: serde_json::json!({
                "hyperv": { "wmi_path": cp.wmi_path }
            }),
        }
    }
}

impl Default for HyperVProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::provider_api::VirtualizationProvider for HyperVProvider {
    fn provider_type(&self) -> crate::provider_api::ProviderType {
        crate::provider_api::ProviderType::HyperV
    }

    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    fn list_hosts(&self) -> crate::provider_api::ProviderResult<Vec<crate::provider_api::HostInfo>> {
        use crate::provider_api::{HostInfo, ProviderType};
        // For local Hyper-V we return a single host entry for the local machine.
        let hostname = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "localhost".to_owned());
        Ok(vec![HostInfo {
            host_id: "local".to_owned(),
            hostname,
            provider_type: ProviderType::HyperV,
            cluster: None,
            os_version: None,
            provider_metadata: serde_json::json!({
                "hyperv": { "wmi_namespace": "root\\\\virtualization\\\\v2" }
            }),
        }])
    }

    fn list_vms(
        &self,
        _host_id: &str,
    ) -> crate::provider_api::ProviderResult<Vec<crate::provider_api::VmInfo>> {
        let vms = list_vms();
        Ok(vms
            .iter()
            .filter(|v| matches!(v.provider, VmProvider::HyperV))
            .map(Self::to_universal_vm)
            .collect())
    }

    fn get_vm(&self, vm_id: &str) -> crate::provider_api::ProviderResult<crate::provider_api::VmInfo> {
        let real_id = vm_id.strip_prefix("hyperv:").unwrap_or(vm_id);
        let vms = list_vms();
        vms.iter()
            .find(|v| v.id == real_id)
            .map(Self::to_universal_vm)
            .ok_or_else(|| crate::provider_api::ProviderError::VmNotFound(vm_id.to_owned()))
    }

    fn get_capabilities(
        &self,
        vm_id: &str,
    ) -> crate::provider_api::ProviderResult<crate::capability_engine::VmCapabilityGraph> {
        let real_id = vm_id.strip_prefix("hyperv:").unwrap_or(vm_id);
        let vms = list_vms();
        let vm = vms
            .iter()
            .find(|v| v.id == real_id)
            .ok_or_else(|| crate::provider_api::ProviderError::VmNotFound(vm_id.to_owned()))?;
        Ok(crate::capability_engine::evaluate(vm))
    }

    fn power_action(
        &self,
        vm_id: &str,
        action: &str,
    ) -> crate::provider_api::ProviderResult<()> {
        let real_id = vm_id.strip_prefix("hyperv:").unwrap_or(vm_id);
        let action_enum = VmPowerAction::from_str(action)
            .ok_or_else(|| crate::provider_api::ProviderError::NotSupported(
                format!("Unknown power action: {action}"),
            ))?;
        let vm_path = format!(
            "Msvm_ComputerSystem.CreationClassName=\"Msvm_ComputerSystem\",Name=\"{real_id}\""
        );
        request_power_action(&vm_path, action_enum);
        Ok(())
    }

    fn list_snapshots(
        &self,
        vm_id: &str,
    ) -> crate::provider_api::ProviderResult<Vec<crate::provider_api::SnapshotInfo>> {
        let real_id = vm_id.strip_prefix("hyperv:").unwrap_or(vm_id);
        let checkpoints = list_checkpoints(real_id);
        Ok(checkpoints
            .iter()
            .map(|cp| Self::checkpoint_to_snapshot(cp, vm_id))
            .collect())
    }

    fn create_snapshot(
        &self,
        vm_id: &str,
        name: Option<&str>,
    ) -> crate::provider_api::ProviderResult<crate::provider_api::SnapshotInfo> {
        let real_id = vm_id.strip_prefix("hyperv:").unwrap_or(vm_id);
        let vm_path = format!(
            "Msvm_ComputerSystem.CreationClassName=\"Msvm_ComputerSystem\",Name=\"{real_id}\""
        );
        let snap_name = create_checkpoint(&vm_path, name)
            .map_err(|e| crate::provider_api::ProviderError::NativeError {
                provider: "hyperv".to_owned(),
                code: "CREATE_CHECKPOINT_FAILED".to_owned(),
                message: e,
            })?;
        Ok(crate::provider_api::SnapshotInfo {
            snapshot_id: format!("hyperv:new:{snap_name}"),
            provider_native_id: snap_name.clone(),
            vm_id: vm_id.to_owned(),
            name: snap_name,
            description: None,
            created_at: chrono_now_iso(),
            is_current: false,
            snapshot_type: "Standard".to_owned(),
            provider_metadata: serde_json::Value::Null,
        })
    }

    fn apply_snapshot(
        &self,
        _vm_id: &str,
        snapshot_id: &str,
    ) -> crate::provider_api::ProviderResult<()> {
        // snapshot_id is the WMI path (stored in provider_native_id)
        let wmi_path = snapshot_id.strip_prefix("hyperv:").unwrap_or(snapshot_id);
        apply_checkpoint(wmi_path)
            .map_err(|e| crate::provider_api::ProviderError::NativeError {
                provider: "hyperv".to_owned(),
                code: "APPLY_CHECKPOINT_FAILED".to_owned(),
                message: e,
            })
    }

    fn delete_snapshot(
        &self,
        _vm_id: &str,
        snapshot_id: &str,
    ) -> crate::provider_api::ProviderResult<()> {
        let wmi_path = snapshot_id.strip_prefix("hyperv:").unwrap_or(snapshot_id);
        delete_checkpoint(wmi_path)
            .map_err(|e| crate::provider_api::ProviderError::NativeError {
                provider: "hyperv".to_owned(),
                code: "DELETE_CHECKPOINT_FAILED".to_owned(),
                message: e,
            })
    }

    fn get_preview_frame(
        &self,
        vm_id: &str,
        width: u32,
        height: u32,
    ) -> crate::provider_api::ProviderResult<Vec<u8>> {
        let real_id = vm_id.strip_prefix("hyperv:").unwrap_or(vm_id);
        let vm_path = format!(
            "Msvm_ComputerSystem.CreationClassName=\"Msvm_ComputerSystem\",Name=\"{real_id}\""
        );
        let rgba = capture_screen(real_id, &vm_path, width as u16, height as u16)
            .map_err(|e| crate::provider_api::ProviderError::NativeError {
                provider: "hyperv".to_owned(),
                code: "PREVIEW_CAPTURE_FAILED".to_owned(),
                message: e,
            })?;
        // Convert RGBA → BGRA as required by pipeline
        let bgra: Vec<u8> = rgba
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect();
        Ok(bgra)
    }

    fn manifest(&self) -> serde_json::Value {
        serde_json::json!({
            "provider_type": "hyperv",
            "provider_id": self.provider_id,
            "features": [
                "inventory", "preview", "power_control", "checkpoints",
                "keyboard_rescue", "rdp_relay", "enhanced_session_detection"
            ],
            "experimental": ["af_hyperv_socket"],
            "wmi_namespace": "root\\virtualization\\v2"
        })
    }
}

/// Returns current time as ISO 8601 string (no external dependency).
fn chrono_now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Format as yyyy-mm-ddTHH:MM:SSZ (UTC, simplified)
    let s = secs;
    let (y, mo, d, h, mi, sec) = {
        let s2 = s % 60;
        let mi2 = (s / 60) % 60;
        let h2 = (s / 3600) % 24;
        let days = s / 86400;
        // Simplified Gregorian (good until ~2100)
        let y2 = 1970 + days / 365;
        let rem = days % 365;
        let mo2 = rem / 30 + 1;
        let d2 = rem % 30 + 1;
        (y2, mo2, d2, h2, mi2, s2)
    };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{sec:02}Z")
}
