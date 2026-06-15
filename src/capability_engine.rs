//! Capability Engine — Universal Hypervisor Access Fabric.
//!
//! Определяет, какие режимы подключения доступны для каждой VM, с каким
//! уровнем уверенности, почему недоступны остальные, и как это исправить.
//!
//! Используется:
//!  • на хосте (host-agent) — для оценки каждой VM при инвентаризации;
//!  • на клиенте (GUI) — для отображения capability badges и Explain UI.
//!
//! Принцип «Smart Backend Selection» (§38 плана): не один жёстко заданный
//! путь, а граф возможностей с чёткими reason codes и remediation steps.
//!
//! ADR-003: Every VM exposes capability graph instead of single console_mode.
//! ADR-004: Risky backends like AF_HYPERV are experimental and disabled by default.

use serde::{Deserialize, Serialize};

// ── Capability state ──────────────────────────────────────────────────────────

/// Состояние конкретной возможности для данной VM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CapabilityState {
    /// Возможность доступна и работоспособна.
    Available,
    /// Возможность доступна, но с ограничениями / нестабильно.
    Degraded,
    /// Заблокирована политикой (ACL / RBAC / JIT).
    BlockedByPolicy,
    /// Технически невозможна для данной VM/хоста.
    Unsupported,
    /// Состояние не определено — нет данных.
    Unknown,
    /// Экспериментальный backend — feature flag + POC required.
    Experimental,
}

impl CapabilityState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Available => "Available",
            Self::Degraded => "Degraded",
            Self::BlockedByPolicy => "Blocked",
            Self::Unsupported => "Unavailable",
            Self::Unknown => "Unknown",
            Self::Experimental => "Experimental",
        }
    }
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available | Self::Degraded)
    }
}

// ── Session modes ─────────────────────────────────────────────────────────────

/// Режим доступа к VM — универсальный, не привязан к Hyper-V.
/// В порядке убывания интерактивности (см. §38 плана).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionMode {
    /// VM выключена / saved — только инвентаризация и power ops.
    Offline,
    /// Live thumbnail только (1–2 FPS), без ввода.
    PreviewOnly,
    /// WMI keyboard rescue + thumbnail (Hyper-V / нет полноценного RDP).
    BasicRescue,
    /// Полноценный RDP через TCP relay (guest IP + RDP service).
    RdpRelay,
    /// VNC/noVNC console (Proxmox, libvirt/KVM, Xen).
    VncConsole,
    /// SPICE console (libvirt/KVM, Proxmox optional).
    SpiceConsole,
    /// Generic web-based console (provider noVNC embed etc).
    WebConsole,
    /// VMware WebMKS / MKS console.
    WebMksConsole,
    /// Serial console over telnet/websocket (libvirt, Proxmox).
    SerialConsole,
    /// Enhanced Session (RDP over VMBus) — лучший режим для Hyper-V.
    EnhancedSession,
    /// Provider-native console (passthrough to provider UI, fallback).
    ProviderNativeConsole,
    /// AF_HYPERV experimental (только за feature flag + POC checklist).
    ExperimentalHvSocket,
    /// Generic experimental backend (named).
    Experimental(String),
}

impl SessionMode {
    pub fn label(&self) -> &str {
        match self {
            Self::Offline => "Offline",
            Self::PreviewOnly => "Preview Only",
            Self::BasicRescue => "Basic Rescue",
            Self::RdpRelay => "RDP Relay",
            Self::VncConsole => "VNC Console",
            Self::SpiceConsole => "SPICE Console",
            Self::WebConsole => "Web Console",
            Self::WebMksConsole => "WebMKS Console",
            Self::SerialConsole => "Serial Console",
            Self::EnhancedSession => "Enhanced Session",
            Self::ProviderNativeConsole => "Provider Console",
            Self::ExperimentalHvSocket => "Experimental (HvSocket)",
            Self::Experimental(name) => name.as_str(),
        }
    }
    /// Цвет для UI-бейджа (R, G, B).
    pub fn badge_rgb(&self) -> (u8, u8, u8) {
        match self {
            Self::Offline => (0x80, 0x80, 0x80),
            Self::PreviewOnly => (0x60, 0x80, 0xA0),
            Self::BasicRescue => (0xFF, 0x99, 0x00),
            Self::RdpRelay => (0x22, 0xC5, 0x5E),
            Self::VncConsole => (0x00, 0xAF, 0xD8),
            Self::SpiceConsole => (0x8B, 0x5C, 0xF6),
            Self::WebConsole => (0x06, 0xB6, 0xD4),
            Self::WebMksConsole => (0x60, 0x99, 0xE0),
            Self::SerialConsole => (0xA0, 0xA0, 0x40),
            Self::EnhancedSession => (0x00, 0x78, 0xD4),
            Self::ProviderNativeConsole => (0x40, 0x80, 0x60),
            Self::ExperimentalHvSocket => (0xA0, 0x40, 0xD0),
            Self::Experimental(_) => (0xA0, 0x40, 0xD0),
        }
    }
    /// Base score for Smart Connect (§38.3).
    pub fn base_score(&self) -> i32 {
        match self {
            Self::ProviderNativeConsole => 100,
            Self::EnhancedSession => 95,
            Self::WebMksConsole => 95,
            Self::SpiceConsole => 90,
            Self::VncConsole => 85,
            Self::RdpRelay => 80,
            Self::WebConsole => 75,
            Self::SerialConsole => 55,
            Self::BasicRescue => 50,
            Self::PreviewOnly => 20,
            Self::Offline => 0,
            Self::ExperimentalHvSocket | Self::Experimental(_) => -50,
        }
    }
}

// ── Capability ────────────────────────────────────────────────────────────────

/// Оценка одной возможности с диагностикой.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    pub state: CapabilityState,
    /// Уровень уверенности 0–100. 100 = точно Known, 30 = эвристика.
    pub confidence: u8,
    /// Machine-readable reason code.
    pub reason_code: String,
    /// Человекочитаемое объяснение.
    pub human_reason: String,
    /// Что нужно сделать, чтобы включить возможность (если применимо).
    pub remediation: Option<String>,
}

impl Capability {
    pub fn available(reason_code: &str, human: &str) -> Self {
        Self {
            state: CapabilityState::Available,
            confidence: 95,
            reason_code: reason_code.to_owned(),
            human_reason: human.to_owned(),
            remediation: None,
        }
    }
    pub fn degraded(reason_code: &str, human: &str, fix: Option<&str>) -> Self {
        Self {
            state: CapabilityState::Degraded,
            confidence: 70,
            reason_code: reason_code.to_owned(),
            human_reason: human.to_owned(),
            remediation: fix.map(str::to_owned),
        }
    }
    pub fn unsupported(reason_code: &str, human: &str, fix: Option<&str>) -> Self {
        Self {
            state: CapabilityState::Unsupported,
            confidence: 90,
            reason_code: reason_code.to_owned(),
            human_reason: human.to_owned(),
            remediation: fix.map(str::to_owned),
        }
    }
    pub fn blocked(reason_code: &str, human: &str) -> Self {
        Self {
            state: CapabilityState::BlockedByPolicy,
            confidence: 99,
            reason_code: reason_code.to_owned(),
            human_reason: human.to_owned(),
            remediation: Some("Contact your administrator".to_owned()),
        }
    }
    pub fn unknown(reason_code: &str) -> Self {
        Self {
            state: CapabilityState::Unknown,
            confidence: 30,
            reason_code: reason_code.to_owned(),
            human_reason: "Status unknown — insufficient data".to_owned(),
            remediation: None,
        }
    }
    pub fn experimental(reason_code: &str) -> Self {
        Self {
            state: CapabilityState::Experimental,
            confidence: 20,
            reason_code: reason_code.to_owned(),
            human_reason: "Experimental backend — POC checklist not passed".to_owned(),
            remediation: Some("Enable feature flag hv_socket_backend.enabled".to_owned()),
        }
    }
}

// ── VmCapabilityGraph ─────────────────────────────────────────────────────────

/// Полный граф возможностей одной VM — центральный тип Capability Engine.
/// Универсальный: не зависит от конкретного провайдера.
/// ADR-003: Every VM exposes capability graph instead of single console_mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmCapabilityGraph {
    pub vm_id: String,
    /// Provider type — нужен для контекста, но не для разветвления логики UI.
    /// UI должен читать capability states, а не provider_type напрямую.
    pub provider_type: Option<crate::provider_api::ProviderType>,

    // ── Preview / thumbnail ───────────────────────────────────────────────────
    /// Live thumbnail через WMI (Hyper-V) или provider screenshot API.
    pub preview: Capability,

    // ── Input / rescue ────────────────────────────────────────────────────────
    /// WMI keyboard rescue (Msvm_Keyboard) — Hyper-V specific, но поле универсальное.
    pub keyboard_rescue: Capability,

    // ── Interactive console modes ─────────────────────────────────────────────
    /// RDP через TCP relay (требует guest IP + RDP service).
    pub rdp_relay: Capability,
    /// VNC/noVNC console (Proxmox, libvirt/KVM, Xen).
    pub vnc_console: Capability,
    /// SPICE console (libvirt/KVM, Proxmox optional).
    pub spice_console: Capability,
    /// Generic web-based console embed.
    pub web_console: Capability,
    /// VMware WebMKS / MKS protocol console.
    pub webmks_console: Capability,
    /// Serial console over websocket/telnet.
    pub serial_console: Capability,
    /// Enhanced Session Mode (Hyper-V: RDP over VMBus).
    pub enhanced_session: Capability,
    /// AF_HYPERV experimental transport.
    pub hv_socket: Capability,

    // ── Features ──────────────────────────────────────────────────────────────
    /// Clipboard support (mode-dependent).
    pub clipboard: Capability,
    /// File transfer support.
    pub file_transfer: Capability,
    /// Session recording availability.
    pub recording: Capability,
    /// Generic experimental / future capability slot.
    pub experimental: Capability,

    // ── Decision ─────────────────────────────────────────────────────────────
    /// Рекомендуемый режим (что открыть при нажатии Connect).
    pub recommended_mode: SessionMode,
    /// Список ограничений (shielded, policy, cluster).
    pub constraints: Vec<String>,
}

impl VmCapabilityGraph {
    /// Сериализация в JSON для передачи клиенту.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned())
    }

    /// Десериализация от хоста.
    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }

    /// Краткая строка для UI-статуса сессии (например, в toolbar).
    pub fn short_status(&self) -> String {
        format!(
            "Mode: {} | Preview:{} | Rescue:{} | RDP:{} | Enhanced:{}",
            self.recommended_mode.label(),
            self.preview.state.label(),
            self.keyboard_rescue.state.label(),
            self.rdp_relay.state.label(),
            self.enhanced_session.state.label(),
        )
    }

    /// Returns true if ANY interactive mode is available.
    pub fn any_interactive(&self) -> bool {
        [
            &self.rdp_relay,
            &self.vnc_console,
            &self.spice_console,
            &self.web_console,
            &self.webmks_console,
            &self.serial_console,
            &self.enhanced_session,
            &self.keyboard_rescue,
        ]
        .iter()
        .any(|c| c.state.is_available())
    }

    /// Helper: build a "Unsupported everywhere" stub for non-Windows hosts.
    pub fn unsupported_stub(vm_id: &str, reason: &str) -> Self {
        let u = Capability::unsupported(reason, "Not supported on this host", None);
        Self {
            vm_id: vm_id.to_owned(),
            provider_type: None,
            preview: u.clone(),
            keyboard_rescue: u.clone(),
            rdp_relay: u.clone(),
            vnc_console: u.clone(),
            spice_console: u.clone(),
            web_console: u.clone(),
            webmks_console: u.clone(),
            serial_console: u.clone(),
            enhanced_session: u.clone(),
            hv_socket: Capability::experimental("HVSOCKET_EXPERIMENTAL_DISABLED"),
            clipboard: u.clone(),
            file_transfer: u.clone(),
            recording: u.clone(),
            experimental: Capability::experimental("EXPERIMENTAL_DISABLED"),
            recommended_mode: SessionMode::Offline,
            constraints: vec![reason.to_owned()],
        }
    }
}

// ── Hyper-V evaluation ────────────────────────────────────────────────────────

/// Оценить возможности VM по данным из Hyper-V WMI инвентаризации.
#[cfg(windows)]
pub fn evaluate(vm: &crate::hyperv::VmInfo) -> VmCapabilityGraph {
    use crate::hyperv::{ConsoleMode, VmProvider, VmState};
    use crate::provider_api::ProviderType;

    let is_running = matches!(vm.state, VmState::Running | VmState::Paused);
    let is_hyperv = matches!(vm.provider, VmProvider::HyperV);

    // ── Preview ──────────────────────────────────────────────────────────────
    let preview = if is_running && is_hyperv {
        Capability::available(
            "PREVIEW_AVAILABLE",
            "WMI thumbnail активен (GetVirtualSystemThumbnailImage)",
        )
    } else if !is_running {
        Capability::unsupported(
            "VM_OFFLINE",
            "VM выключена или в состоянии Saved — thumbnail недоступен",
            Some("Запустите VM"),
        )
    } else {
        Capability::unsupported(
            "PREVIEW_NOT_SUPPORTED",
            "Preview доступен только для Hyper-V VM через WMI",
            None,
        )
    };

    // ── Keyboard rescue ───────────────────────────────────────────────────────
    let keyboard_rescue = if is_running && is_hyperv {
        Capability::available(
            "RESCUE_AVAILABLE",
            "WMI Msvm_Keyboard: TypeText, PressKey, TypeCtrlAltDel",
        )
    } else if !is_running {
        Capability::unsupported("VM_OFFLINE", "VM не запущена", Some("Запустите VM"))
    } else {
        Capability::unsupported(
            "RESCUE_NOT_HYPERV",
            "Keyboard rescue доступен только через Hyper-V WMI",
            None,
        )
    };

    // ── RDP relay ─────────────────────────────────────────────────────────────
    // Без network probe не знаем — помечаем Unknown.
    let rdp_relay = if !is_running {
        Capability::unsupported("VM_OFFLINE", "VM не запущена", None)
    } else {
        Capability::unknown("NO_GUEST_IP")
    };

    // ── VNC / SPICE / Web / WebMKS / Serial — не поддерживаются Hyper-V ──────
    let not_hyperv_console = Capability::unsupported(
        "NOT_SUPPORTED_BY_HYPERV",
        "Этот режим консоли не поддерживается Hyper-V провайдером",
        Some("Используйте RDP Relay или Enhanced Session"),
    );

    // ── Enhanced Session ──────────────────────────────────────────────────────
    let enhanced_session = match (&vm.console_mode, is_running, is_hyperv) {
        (ConsoleMode::EnhancedSession, true, true) => Capability::available(
            "ENHANCED_AVAILABLE",
            "Integration Services активны, Enhanced Session Mode поддерживается",
        ),
        (_, true, true) => Capability::unsupported(
            "ENHANCED_UNAVAILABLE_FOR_VM",
            "Integration Services не отвечают или Enhanced Session отключён на хосте",
            Some("Установите Hyper-V Integration Services в гостевой ОС"),
        ),
        (_, false, _) => Capability::unsupported("VM_OFFLINE", "VM не запущена", None),
        _ => Capability::unsupported(
            "NOT_HYPERV",
            "Enhanced Session доступен только для Hyper-V VM",
            None,
        ),
    };

    // ── AF_HYPERV experimental ────────────────────────────────────────────────
    let hv_socket = Capability::experimental("HVSOCKET_EXPERIMENTAL_DISABLED");

    // ── Clipboard ─────────────────────────────────────────────────────────────
    let clipboard = if matches!(vm.console_mode, ConsoleMode::EnhancedSession) && is_running {
        Capability::available(
            "CLIPBOARD_VIA_ENHANCED",
            "Clipboard через Enhanced Session RDP virtual channel",
        )
    } else if is_running && is_hyperv {
        Capability::degraded(
            "CLIPBOARD_PASTE_ONLY",
            "Только paste-as-keystrokes в BasicRescue",
            None,
        )
    } else {
        Capability::unsupported(
            "CLIPBOARD_UNAVAILABLE",
            "Clipboard недоступен в текущем режиме",
            None,
        )
    };

    // ── File transfer ─────────────────────────────────────────────────────────
    let file_transfer = if matches!(vm.console_mode, ConsoleMode::EnhancedSession) && is_running {
        Capability::degraded(
            "FILE_TRANSFER_VIA_ENHANCED",
            "File transfer возможен через Enhanced Session (RDP drive redirect)",
            None,
        )
    } else {
        Capability::unsupported(
            "FILE_TRANSFER_UNAVAILABLE",
            "File transfer недоступен в текущем режиме",
            None,
        )
    };

    // ── Recording ─────────────────────────────────────────────────────────────
    let recording = Capability::degraded(
        "RECORDING_METADATA_ONLY",
        "Запись событий доступна (метаданные сессии). Video recording — Phase 2",
        None,
    );

    // ── Experimental ─────────────────────────────────────────────────────────
    let experimental = Capability::experimental("EXPERIMENTAL_DISABLED");

    // ── Recommended mode ──────────────────────────────────────────────────────
    let recommended_mode = if !is_running {
        SessionMode::Offline
    } else if matches!(vm.console_mode, ConsoleMode::EnhancedSession) && is_hyperv {
        SessionMode::EnhancedSession
    } else if is_hyperv {
        SessionMode::BasicRescue
    } else {
        SessionMode::PreviewOnly
    };

    // ── Constraints ───────────────────────────────────────────────────────────
    let mut constraints = Vec::new();
    if !is_hyperv {
        constraints.push(format!(
            "Provider: {} — ограниченный доступ",
            vm.provider.label()
        ));
    }
    if matches!(vm.state, VmState::Paused) {
        constraints.push("VM на паузе — thumbnail может быть недоступен".to_owned());
    }

    VmCapabilityGraph {
        vm_id: vm.id.clone(),
        provider_type: if is_hyperv {
            Some(ProviderType::HyperV)
        } else {
            None
        },
        preview,
        keyboard_rescue,
        rdp_relay,
        vnc_console: not_hyperv_console.clone(),
        spice_console: not_hyperv_console.clone(),
        web_console: not_hyperv_console.clone(),
        webmks_console: not_hyperv_console.clone(),
        serial_console: not_hyperv_console,
        enhanced_session,
        hv_socket,
        clipboard,
        file_transfer,
        recording,
        experimental,
        recommended_mode,
        constraints,
    }
}

/// Non-Windows stub — всегда возвращает Unsupported для всего.
#[cfg(not(windows))]
pub fn evaluate_stub(vm_id: &str) -> VmCapabilityGraph {
    VmCapabilityGraph::unsupported_stub(vm_id, "HOST_NOT_WINDOWS")
}
