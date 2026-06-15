// =============================================================================
// Universal Hypervisor Access Fabric — Provider API
//
// Provider-agnostic core types and trait definitions.
// This module must NOT import any provider-specific modules (hyperv, vmware, …).
// All provider-specific data must live in `provider_metadata: serde_json::Value`.
//
// ADR-001: Core platform works with Provider API, not with hypervisor-specific APIs.
// ADR-005: Provider-specific fields are stored in provider_metadata.
// =============================================================================

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use serde::{Deserialize, Serialize};

use crate::capability_engine::{Capability, CapabilityState, VmCapabilityGraph};

// ── Provider type ─────────────────────────────────────────────────────────────

/// Identifies which hypervisor / platform manages the VM.
/// Used in VmInfo, CapabilityGraph, audit events and protocol messages.
/// Client/Broker must never branch on ProviderType to build capabilities —
/// use the CapabilityGraph instead (ADR-003).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    HyperV,
    VMware,
    Proxmox,
    Libvirt,
    QemuStandalone,
    Xen,
    XcpNg,
    AzureLocal,
    /// For third-party or internal providers not in the enum.
    #[serde(untagged)]
    Custom(String),
}

impl ProviderType {
    pub fn label(&self) -> &str {
        match self {
            Self::HyperV => "Hyper-V",
            Self::VMware => "VMware",
            Self::Proxmox => "Proxmox VE",
            Self::Libvirt => "libvirt/KVM",
            Self::QemuStandalone => "QEMU (standalone)",
            Self::Xen => "Xen",
            Self::XcpNg => "XCP-ng",
            Self::AzureLocal => "Azure Local (Azure Stack HCI)",
            Self::Custom(s) => s.as_str(),
        }
    }
    pub fn is_windows_based(&self) -> bool {
        matches!(self, Self::HyperV | Self::AzureLocal)
    }
}

// ── Universal power state ─────────────────────────────────────────────────────

/// Normalised VM power state across all providers.
/// Provider-specific substates go in `provider_metadata`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerState {
    Running,
    Stopped,
    Paused,
    Suspended,
    Saved,
    Starting,
    Stopping,
    Resetting,
    Unknown,
}

impl PowerState {
    pub fn label(&self) -> &str {
        match self {
            Self::Running => "Running",
            Self::Stopped => "Stopped",
            Self::Paused => "Paused",
            Self::Suspended => "Suspended",
            Self::Saved => "Saved",
            Self::Starting => "Starting",
            Self::Stopping => "Stopping",
            Self::Resetting => "Resetting",
            Self::Unknown => "Unknown",
        }
    }
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
    pub fn is_operational(&self) -> bool {
        matches!(self, Self::Running | Self::Paused | Self::Suspended | Self::Saved)
    }
    /// Badge colour for the dashboard (R, G, B).
    pub fn badge_rgb(&self) -> (u8, u8, u8) {
        match self {
            Self::Running => (0x22, 0xC5, 0x5E),
            Self::Stopped => (0x80, 0x80, 0x80),
            Self::Paused => (0xFF, 0xBF, 0x00),
            Self::Suspended => (0x60, 0x80, 0xA0),
            Self::Saved => (0x60, 0x80, 0xA0),
            Self::Starting => (0x00, 0xBC, 0xFF),
            Self::Stopping => (0xFF, 0x60, 0x00),
            Self::Resetting => (0xFF, 0x60, 0x00),
            Self::Unknown => (0x60, 0x60, 0x60),
        }
    }
}

// ── Guest tools status ────────────────────────────────────────────────────────

/// Status of provider/hypervisor tools in the guest OS.
/// Examples: VMware Tools, Hyper-V Integration Services, QEMU guest agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolsStatus {
    /// Tools are running and up-to-date.
    Running,
    /// Tools are installed but not currently running.
    NotRunning,
    /// Tools are not installed.
    NotInstalled,
    /// Tools are installed but outdated.
    Outdated,
    /// Unknown / data unavailable.
    Unknown,
    /// Provider does not use a guest tools concept (N/A).
    NotApplicable,
}

impl ToolsStatus {
    pub fn label(&self) -> &str {
        match self {
            Self::Running => "Running",
            Self::NotRunning => "Not running",
            Self::NotInstalled => "Not installed",
            Self::Outdated => "Outdated",
            Self::Unknown => "Unknown",
            Self::NotApplicable => "N/A",
        }
    }
    pub fn is_healthy(&self) -> bool {
        matches!(self, Self::Running)
    }
}

// ── Universal host info ───────────────────────────────────────────────────────

/// Normalised host (hypervisor node) info across all providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInfo {
    /// Stable platform-internal ID (not display name).
    pub host_id: String,
    /// Human-readable hostname.
    pub hostname: String,
    pub provider_type: ProviderType,
    /// Cluster / datacenter / pool name, if applicable.
    pub cluster: Option<String>,
    /// Operating system of the hypervisor host.
    pub os_version: Option<String>,
    /// Provider-specific fields (e.g. ESXi build, Proxmox node ID).
    pub provider_metadata: serde_json::Value,
}

// ── Universal VM info ─────────────────────────────────────────────────────────

/// Normalised VM information across all providers.
/// Provider-specific fields must go in `provider_metadata` only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmInfo {
    /// Stable platform-internal VM ID (UUID or compound key).
    pub vm_id: String,
    /// Provider-native identifier (GUID for Hyper-V, MOID for VMware, etc.).
    pub provider_native_id: String,
    /// Human-readable VM display name.
    pub name: String,
    pub provider_type: ProviderType,
    /// ID of the host this VM is running on.
    pub host_id: String,
    /// Cluster name if available.
    pub cluster: Option<String>,
    pub power_state: PowerState,
    /// Guest IP addresses detected by the hypervisor.
    pub guest_ips: Vec<String>,
    pub tools_status: ToolsStatus,
    /// VM tags / labels for policy and filtering.
    pub tags: Vec<String>,
    /// Provider-specific raw fields (generation, enhanced_session_state, etc.).
    pub provider_metadata: serde_json::Value,
}

impl VmInfo {
    /// Returns the primary guest IP (first in list) if available.
    pub fn primary_ip(&self) -> Option<&str> {
        self.guest_ips.first().map(|s| s.as_str())
    }
    pub fn is_running(&self) -> bool {
        self.power_state.is_running()
    }
}

// ── Universal snapshot info ───────────────────────────────────────────────────

/// Normalised snapshot / checkpoint across all providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// Stable platform-internal snapshot ID.
    pub snapshot_id: String,
    /// Provider-native snapshot identifier.
    pub provider_native_id: String,
    /// VM this snapshot belongs to.
    pub vm_id: String,
    /// Human-readable snapshot name.
    pub name: String,
    /// Description / notes.
    pub description: Option<String>,
    /// ISO 8601 creation timestamp.
    pub created_at: String,
    /// Whether this is the current/active snapshot.
    pub is_current: bool,
    /// Provider-native snapshot type (e.g. "Production" for Hyper-V, "RAM" for VMware).
    pub snapshot_type: String,
    pub provider_metadata: serde_json::Value,
}

// ── Provider error ────────────────────────────────────────────────────────────

/// Normalised error from any provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProviderError {
    /// Provider API is unreachable (network, auth).
    Unavailable(String),
    /// VM not found in provider inventory.
    VmNotFound(String),
    /// Operation is not supported by this provider.
    NotSupported(String),
    /// Authentication / authorisation failure.
    AuthFailed(String),
    /// Provider-native error (pass-through with provider context).
    NativeError { provider: String, code: String, message: String },
    /// Timeout waiting for provider response.
    Timeout(String),
    /// Any other error.
    Other(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(s) => write!(f, "Provider unavailable: {s}"),
            Self::VmNotFound(s) => write!(f, "VM not found: {s}"),
            Self::NotSupported(s) => write!(f, "Not supported: {s}"),
            Self::AuthFailed(s) => write!(f, "Auth failed: {s}"),
            Self::NativeError { provider, code, message } =>
                write!(f, "[{provider}] {code}: {message}"),
            Self::Timeout(s) => write!(f, "Timeout: {s}"),
            Self::Other(s) => write!(f, "Error: {s}"),
        }
    }
}

impl std::error::Error for ProviderError {}

pub type ProviderResult<T> = Result<T, ProviderError>;

// ── Virtualization Provider trait ─────────────────────────────────────────────

/// Minimal contract that every hypervisor connector must satisfy.
/// Sync for now (each call may block a worker thread).
/// Full async is tracked for Phase 2 (gRPC streaming).
pub trait VirtualizationProvider: Send + Sync {
    fn provider_type(&self) -> ProviderType;
    fn provider_id(&self) -> &str;

    // ── Inventory ─────────────────────────────────────────────────────────────
    fn list_hosts(&self) -> ProviderResult<Vec<HostInfo>>;
    fn list_vms(&self, host_id: &str) -> ProviderResult<Vec<VmInfo>>;
    fn get_vm(&self, vm_id: &str) -> ProviderResult<VmInfo>;

    // ── Capabilities ──────────────────────────────────────────────────────────
    fn get_capabilities(&self, vm_id: &str) -> ProviderResult<VmCapabilityGraph>;

    // ── Power management ──────────────────────────────────────────────────────
    fn power_action(&self, vm_id: &str, action: &str) -> ProviderResult<()>;

    // ── Snapshots ─────────────────────────────────────────────────────────────
    fn list_snapshots(&self, vm_id: &str) -> ProviderResult<Vec<SnapshotInfo>> {
        Err(ProviderError::NotSupported(
            "Snapshots not implemented for this provider".to_owned(),
        ))
    }
    fn create_snapshot(&self, vm_id: &str, name: Option<&str>) -> ProviderResult<SnapshotInfo> {
        let _ = (vm_id, name);
        Err(ProviderError::NotSupported("create_snapshot".to_owned()))
    }
    fn apply_snapshot(&self, vm_id: &str, snapshot_id: &str) -> ProviderResult<()> {
        let _ = (vm_id, snapshot_id);
        Err(ProviderError::NotSupported("apply_snapshot".to_owned()))
    }
    fn delete_snapshot(&self, vm_id: &str, snapshot_id: &str) -> ProviderResult<()> {
        let _ = (vm_id, snapshot_id);
        Err(ProviderError::NotSupported("delete_snapshot".to_owned()))
    }

    // ── Preview ───────────────────────────────────────────────────────────────
    /// Returns a BGRA preview frame for the VM, if available.
    fn get_preview_frame(&self, vm_id: &str, width: u32, height: u32)
        -> ProviderResult<Vec<u8>>
    {
        let _ = (vm_id, width, height);
        Err(ProviderError::NotSupported("preview not implemented".to_owned()))
    }

    // ── Provider manifest ─────────────────────────────────────────────────────
    fn manifest(&self) -> serde_json::Value {
        serde_json::json!({
            "provider_type": self.provider_type(),
            "provider_id": self.provider_id(),
        })
    }
}

// ── Provider Registry ─────────────────────────────────────────────────────────

/// Runtime registry of all connected providers.
/// Core, Broker and Client use this — never import individual provider crates.
pub struct ProviderRegistry {
    providers: RwLock<HashMap<String, Arc<dyn VirtualizationProvider>>>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: RwLock::new(HashMap::new()),
        }
    }

    /// Register a provider under its `provider_id`.
    pub fn register(&self, provider: Arc<dyn VirtualizationProvider>) {
        let id = provider.provider_id().to_owned();
        if let Ok(mut map) = self.providers.write() {
            map.insert(id, provider);
        }
    }

    /// Remove a provider by ID.
    pub fn unregister(&self, provider_id: &str) {
        if let Ok(mut map) = self.providers.write() {
            map.remove(provider_id);
        }
    }

    pub fn get(&self, provider_id: &str) -> Option<Arc<dyn VirtualizationProvider>> {
        self.providers.read().ok()?.get(provider_id).cloned()
    }

    pub fn all(&self) -> Vec<Arc<dyn VirtualizationProvider>> {
        self.providers.read().ok()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn count(&self) -> usize {
        self.providers.read().ok().map(|m| m.len()).unwrap_or(0)
    }

    /// List all VMs across all providers.
    pub fn list_all_vms(&self) -> Vec<VmInfo> {
        let providers = self.all();
        let mut vms = Vec::new();
        for p in providers {
            match p.list_hosts() {
                Ok(hosts) => {
                    for h in hosts {
                        if let Ok(host_vms) = p.list_vms(&h.host_id) {
                            vms.extend(host_vms);
                        }
                    }
                }
                Err(_) => {}
            }
        }
        vms
    }
}

// ── FakeProvider — for testing multi-provider architecture ────────────────────

/// Fake provider that returns synthetic VMs for testing.
/// Demonstrates the multi-provider architecture without real hypervisor.
pub struct FakeProvider {
    pub id: String,
    pub provider_type: ProviderType,
    pub hosts: Vec<HostInfo>,
    pub vms: Vec<VmInfo>,
}

impl FakeProvider {
    /// Create a minimal fake VMware provider with 2 test VMs.
    pub fn fake_vmware(provider_id: &str) -> Self {
        let host = HostInfo {
            host_id: "fake-host-001".to_owned(),
            hostname: "esxi-fake-01.example.local".to_owned(),
            provider_type: ProviderType::VMware,
            cluster: Some("Fake-Cluster".to_owned()),
            os_version: Some("VMware ESXi 8.0.0".to_owned()),
            provider_metadata: serde_json::json!({ "build": "20513097" }),
        };
        let vms = vec![
            VmInfo {
                vm_id: "fake-vm-001".to_owned(),
                provider_native_id: "vm-1234".to_owned(),
                name: "Fake-Win11-01".to_owned(),
                provider_type: ProviderType::VMware,
                host_id: "fake-host-001".to_owned(),
                cluster: Some("Fake-Cluster".to_owned()),
                power_state: PowerState::Running,
                guest_ips: vec!["192.168.100.10".to_owned()],
                tools_status: ToolsStatus::Running,
                tags: vec!["fake".to_owned(), "test".to_owned()],
                provider_metadata: serde_json::json!({
                    "vmware": { "moid": "vm-1234", "tools_version": "12400" }
                }),
            },
            VmInfo {
                vm_id: "fake-vm-002".to_owned(),
                provider_native_id: "vm-5678".to_owned(),
                name: "Fake-Ubuntu-01".to_owned(),
                provider_type: ProviderType::VMware,
                host_id: "fake-host-001".to_owned(),
                cluster: Some("Fake-Cluster".to_owned()),
                power_state: PowerState::Stopped,
                guest_ips: vec![],
                tools_status: ToolsStatus::NotRunning,
                tags: vec!["fake".to_owned()],
                provider_metadata: serde_json::json!({
                    "vmware": { "moid": "vm-5678", "tools_version": "12300" }
                }),
            },
        ];
        Self {
            id: provider_id.to_owned(),
            provider_type: ProviderType::VMware,
            hosts: vec![host],
            vms,
        }
    }

    /// Create a minimal fake Proxmox provider with 2 test VMs.
    pub fn fake_proxmox(provider_id: &str) -> Self {
        let host = HostInfo {
            host_id: "fake-pve-node-001".to_owned(),
            hostname: "pve-fake-01.example.local".to_owned(),
            provider_type: ProviderType::Proxmox,
            cluster: Some("Fake-PVE-Cluster".to_owned()),
            os_version: Some("Proxmox VE 8.2".to_owned()),
            provider_metadata: serde_json::json!({ "pve_version": "8.2.2" }),
        };
        let vms = vec![
            VmInfo {
                vm_id: "fake-pve-vm-100".to_owned(),
                provider_native_id: "100".to_owned(),
                name: "fake-debian-01".to_owned(),
                provider_type: ProviderType::Proxmox,
                host_id: "fake-pve-node-001".to_owned(),
                cluster: Some("Fake-PVE-Cluster".to_owned()),
                power_state: PowerState::Running,
                guest_ips: vec!["10.10.10.100".to_owned()],
                tools_status: ToolsStatus::NotApplicable,
                tags: vec!["fake".to_owned()],
                provider_metadata: serde_json::json!({
                    "proxmox": { "vmid": 100, "type": "qemu", "node": "pve-fake-01" }
                }),
            },
        ];
        Self {
            id: provider_id.to_owned(),
            provider_type: ProviderType::Proxmox,
            hosts: vec![host],
            vms,
        }
    }
}

impl VirtualizationProvider for FakeProvider {
    fn provider_type(&self) -> ProviderType {
        self.provider_type.clone()
    }
    fn provider_id(&self) -> &str {
        &self.id
    }
    fn list_hosts(&self) -> ProviderResult<Vec<HostInfo>> {
        Ok(self.hosts.clone())
    }
    fn list_vms(&self, host_id: &str) -> ProviderResult<Vec<VmInfo>> {
        Ok(self.vms.iter()
            .filter(|v| v.host_id == host_id)
            .cloned()
            .collect())
    }
    fn get_vm(&self, vm_id: &str) -> ProviderResult<VmInfo> {
        self.vms.iter().find(|v| v.vm_id == vm_id).cloned()
            .ok_or_else(|| ProviderError::VmNotFound(vm_id.to_owned()))
    }
    fn get_capabilities(&self, vm_id: &str) -> ProviderResult<VmCapabilityGraph> {
        let vm = self.get_vm(vm_id)?;
        let is_running = vm.is_running();
        let preview = if is_running {
            Capability::unsupported(
                "PREVIEW_FAKE",
                "Fake provider does not supply real preview frames",
                None,
            )
        } else {
            Capability::unsupported("VM_OFFLINE", "VM is not running", None)
        };
        let rdp = if vm.primary_ip().is_some() && is_running {
            Capability::available("RDP_GUEST_IP_KNOWN", "Guest IP detected — RDP Relay possible")
        } else {
            Capability::unknown("NO_GUEST_IP")
        };
        use crate::capability_engine::SessionMode;
        let recommended_mode = if is_running && vm.primary_ip().is_some() {
            SessionMode::RdpRelay
        } else if is_running {
            SessionMode::PreviewOnly
        } else {
            SessionMode::Offline
        };
        Ok(VmCapabilityGraph {
            vm_id: vm_id.to_owned(),
            provider_type: Some(vm.provider_type.clone()),
            preview,
            keyboard_rescue: Capability::unsupported(
                "RESCUE_NOT_SUPPORTED_BY_FAKE",
                "Fake provider does not implement keyboard rescue",
                None,
            ),
            rdp_relay: rdp,
            enhanced_session: Capability::unsupported(
                "ENHANCED_NOT_SUPPORTED",
                "Enhanced Session is Hyper-V only",
                None,
            ),
            hv_socket: Capability::experimental("HVSOCKET_EXPERIMENTAL_DISABLED"),
            vnc_console: if matches!(vm.provider_type, ProviderType::Proxmox | ProviderType::Libvirt)
                && is_running
            {
                Capability::available("VNC_AVAILABLE", "VNC console available via provider")
            } else {
                Capability::unsupported("VNC_NOT_SUPPORTED", "VNC not available for this provider", None)
            },
            spice_console: Capability::unsupported("SPICE_NOT_SUPPORTED", "SPICE not configured", None),
            web_console: Capability::unsupported("WEB_CONSOLE_NOT_SUPPORTED", "Web console not configured", None),
            webmks_console: if matches!(vm.provider_type, ProviderType::VMware) && is_running {
                Capability::available("WEBMKS_AVAILABLE", "WebMKS ticket available via vSphere")
            } else {
                Capability::unsupported("WEBMKS_NOT_SUPPORTED", "WebMKS is VMware only", None)
            },
            serial_console: Capability::unsupported("SERIAL_NOT_SUPPORTED", "Serial console not configured", None),
            clipboard: Capability::unsupported("CLIPBOARD_DEPENDS_ON_SESSION", "Clipboard depends on session mode", None),
            file_transfer: Capability::unsupported("FILE_TRANSFER_NOT_SUPPORTED", "File transfer not implemented", None),
            recording: Capability::unsupported("RECORDING_NOT_CONFIGURED", "Session recording not configured", None),
            experimental: Capability::experimental("EXPERIMENTAL_DISABLED"),
            recommended_mode,
            constraints: vec!["Fake provider — synthetic data only".to_owned()],
        })
    }
    fn power_action(&self, _vm_id: &str, _action: &str) -> ProviderResult<()> {
        // Fake provider silently accepts power actions (no state change)
        Ok(())
    }
}

// ── Smart Connect scoring ─────────────────────────────────────────────────────

/// Score assigned to a session mode candidate during Smart Connect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendScore {
    pub mode: crate::capability_engine::SessionMode,
    pub available: bool,
    pub score: i32,
    pub reason: String,
}

/// Final Smart Connect decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendDecision {
    pub selected_mode: crate::capability_engine::SessionMode,
    pub fallback_modes: Vec<crate::capability_engine::SessionMode>,
    pub reason: String,
    pub warnings: Vec<String>,
}

impl BackendDecision {
    pub fn offline() -> Self {
        Self {
            selected_mode: crate::capability_engine::SessionMode::Offline,
            fallback_modes: vec![],
            reason: "VM is offline".to_owned(),
            warnings: vec![],
        }
    }
}

/// Choose the best available session backend from a capability graph.
/// Does not apply policy — caller is responsible for policy filtering.
pub fn choose_backend(graph: &VmCapabilityGraph) -> BackendDecision {
    use crate::capability_engine::SessionMode;
    use CapabilityState::*;

    // Base scores from the plan §38.3
    let candidates: Vec<(SessionMode, &Capability, i32)> = vec![
        (SessionMode::EnhancedSession, &graph.enhanced_session, 95),
        (SessionMode::WebMksConsole, &graph.webmks_console, 95),
        (SessionMode::SpiceConsole, &graph.spice_console, 90),
        (SessionMode::VncConsole, &graph.vnc_console, 85),
        (SessionMode::RdpRelay, &graph.rdp_relay, 80),
        (SessionMode::BasicRescue, &graph.keyboard_rescue, 50),
        (SessionMode::PreviewOnly, &graph.preview, 20),
    ];

    let mut scored: Vec<BackendScore> = candidates
        .into_iter()
        .filter_map(|(mode, cap, base)| {
            match cap.state {
                Unsupported | BlockedByPolicy => None,
                _ => {
                    let mut s = base;
                    if cap.state == Degraded { s -= 20; }
                    if cap.state == Unknown { s -= 10; }
                    if cap.state == Experimental { s -= 50; }
                    Some(BackendScore {
                        mode,
                        available: cap.state.is_available(),
                        score: s,
                        reason: cap.human_reason.clone(),
                    })
                }
            }
        })
        .collect();

    if scored.is_empty() {
        return BackendDecision::offline();
    }

    scored.sort_by(|a, b| b.score.cmp(&a.score));

    let best = scored.remove(0);
    let fallbacks = scored.into_iter().map(|s| s.mode).collect();

    BackendDecision {
        selected_mode: best.mode,
        fallback_modes: fallbacks,
        reason: best.reason,
        warnings: vec![],
    }
}

// ── Policy types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardMode {
    Disabled,
    TextOnly,
    TextWithConfirmation,
    Full,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    Disabled,
    EventTimeline,
    VideoRequired,
}

/// Evaluated policy result for a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyResult {
    pub allowed: bool,
    pub require_mfa: bool,
    pub require_approval: bool,
    pub allowed_modes: Vec<crate::capability_engine::SessionMode>,
    pub clipboard_mode: ClipboardMode,
    pub recording_mode: RecordingMode,
    pub max_duration_seconds: u32,
    pub restrictions: Vec<String>,
    pub reason: Option<String>,
}

impl PolicyResult {
    /// Permissive default — allow everything.  Used for testing and dev.
    pub fn allow_all() -> Self {
        use crate::capability_engine::SessionMode;
        Self {
            allowed: true,
            require_mfa: false,
            require_approval: false,
            allowed_modes: vec![
                SessionMode::PreviewOnly,
                SessionMode::BasicRescue,
                SessionMode::RdpRelay,
                SessionMode::VncConsole,
                SessionMode::SpiceConsole,
                SessionMode::WebConsole,
                SessionMode::WebMksConsole,
                SessionMode::SerialConsole,
                SessionMode::EnhancedSession,
                SessionMode::ProviderNativeConsole,
            ],
            clipboard_mode: ClipboardMode::TextOnly,
            recording_mode: RecordingMode::EventTimeline,
            max_duration_seconds: 86400,
            restrictions: vec![],
            reason: None,
        }
    }
}
