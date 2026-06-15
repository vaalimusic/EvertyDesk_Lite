//! Capability Engine — Zero-Trust Hyper-V Access Fabric.
//!
//! Определяет, какие режимы подключения доступны для каждой VM, с каким
//! уровнем уверенности, почему недоступны остальные, и как это исправить.
//!
//! Используется:
//!  • на хосте (host-agent) — для оценки каждой VM при инвентаризации;
//!  • на клиенте (GUI) — для отображения capability badges и Explain UI.
//!
//! Принцип «Smart Backend Selection» (§5.3 плана): не один жёстко заданный
//! путь, а граф возможностей с чёткими reason codes и remediation steps.

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

/// Режим доступа к VM (в порядке убывания интерактивности).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionMode {
    /// VM выключена / saved — только инвентаризация и power ops.
    Offline,
    /// Live thumbnail только (1–2 FPS), без ввода.
    PreviewOnly,
    /// WMI keyboard rescue + thumbnail (нет полноценного RDP).
    BasicRescue,
    /// Полноценный RDP через TCP relay (guest IP + RDP service).
    RdpRelay,
    /// Enhanced Session (RDP over VMBus) — лучший режим.
    EnhancedSession,
    /// AF_HYPERV experimental (только за feature flag + POC checklist).
    ExperimentalHvSocket,
}

impl SessionMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Offline => "Offline",
            Self::PreviewOnly => "Preview Only",
            Self::BasicRescue => "Basic Rescue",
            Self::RdpRelay => "RDP Relay",
            Self::EnhancedSession => "Enhanced Session",
            Self::ExperimentalHvSocket => "Experimental (HvSocket)",
        }
    }
    /// Цвет для UI-бейджа.
    pub fn badge_rgb(&self) -> (u8, u8, u8) {
        match self {
            Self::Offline => (0x80, 0x80, 0x80),
            Self::PreviewOnly => (0x60, 0x80, 0xA0),
            Self::BasicRescue => (0xFF, 0x99, 0x00),
            Self::RdpRelay => (0x22, 0xC5, 0x5E),
            Self::EnhancedSession => (0x00, 0x78, 0xD4),
            Self::ExperimentalHvSocket => (0xA0, 0x40, 0xD0),
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
    /// Machine-readable reason code (см. §7.5 плана).
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmCapabilityGraph {
    pub vm_id: String,
    /// Live thumbnail через WMI GetVirtualSystemThumbnailImage.
    pub preview: Capability,
    /// WMI keyboard rescue (Msvm_Keyboard.TypeText / PressKey).
    pub keyboard_rescue: Capability,
    /// RDP через TCP relay (требует guest IP + RDP service).
    pub rdp_relay: Capability,
    /// Enhanced Session (RDP over VMBus через AF_HYPERV/VSOCK ESM).
    pub enhanced_session: Capability,
    /// AF_HYPERV experimental transport.
    pub hv_socket: Capability,
    /// Clipboard (зависит от режима).
    pub clipboard: Capability,
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
            "Mode: {} | Preview:{} | Rescue:{} | Enhanced:{}",
            self.recommended_mode.label(),
            self.preview.state.label(),
            self.keyboard_rescue.state.label(),
            self.enhanced_session.state.label(),
        )
    }
}

// ── Evaluation ────────────────────────────────────────────────────────────────

/// Оценить возможности VM по данным из Hyper-V WMI инвентаризации.
#[cfg(windows)]
pub fn evaluate(vm: &crate::hyperv::VmInfo) -> VmCapabilityGraph {
    use crate::hyperv::{ConsoleMode, VmProvider, VmState};

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
        constraints.push(format!("Provider: {} — ограниченный доступ", vm.provider.label()));
    }
    if matches!(vm.state, VmState::Paused) {
        constraints.push("VM на паузе — thumbnail может быть недоступен".to_owned());
    }

    VmCapabilityGraph {
        vm_id: vm.id.clone(),
        preview,
        keyboard_rescue,
        rdp_relay,
        enhanced_session,
        hv_socket,
        clipboard,
        recommended_mode,
        constraints,
    }
}

/// Non-Windows stub — всегда возвращает Unsupported для всего.
#[cfg(not(windows))]
pub fn evaluate_stub(vm_id: &str) -> VmCapabilityGraph {
    let u = Capability::unsupported("HOST_NOT_WINDOWS", "Hyper-V доступен только на Windows", None);
    VmCapabilityGraph {
        vm_id: vm_id.to_owned(),
        preview: u.clone(),
        keyboard_rescue: u.clone(),
        rdp_relay: u.clone(),
        enhanced_session: u.clone(),
        hv_socket: Capability::experimental("HVSOCKET_EXPERIMENTAL_DISABLED"),
        clipboard: u,
        recommended_mode: SessionMode::Offline,
        constraints: vec!["Host is not Windows — Hyper-V unavailable".to_owned()],
    }
}
