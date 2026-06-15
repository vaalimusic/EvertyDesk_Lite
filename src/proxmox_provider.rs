// =============================================================================
// Proxmox VE Provider — Universal Hypervisor Access Fabric
//
// Skeleton Proxmox VE provider via Proxmox API v2.
// Implements VirtualizationProvider for multi-provider dashboard.
//
// Phase:  Release 1.3 (§50.3 plan).  Current state: skeleton + API stubs.
// Real implementation uses Proxmox API token auth + VNC/noVNC ticket flow.
//
// Connection modes supported in full implementation:
//   • VNC console / noVNC (VncConsole) — primary interactive mode
//   • SPICE console (SpiceConsole) — optional, if configured
//   • Serial console (SerialConsole) — for Linux VMs
//   • RDP relay (RdpRelay) — if guest runs RDP and has IP
//   • Power management (start/stop/reset/suspend/resume)
//   • Snapshots (QEMU snapshots via API)
// =============================================================================

use crate::{
    capability_engine::{Capability, SessionMode, VmCapabilityGraph},
    provider_api::{
        HostInfo, PowerState, ProviderError, ProviderResult, ProviderType, SnapshotInfo,
        ToolsStatus, VirtualizationProvider, VmInfo,
    },
};

// ── Proxmox config ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ProxmoxConfig {
    /// Proxmox API URL, e.g. "https://pve.example.local:8006"
    pub api_url: String,
    /// Auth mode: "api_token" | "username_password"
    pub auth_mode: String,
    /// API token ID: "user@realm!tokenid"
    pub token_id: String,
    /// API token secret (stored encrypted — see §43)
    pub token_secret: String,
    /// Node name to connect to (empty = all nodes)
    pub node: String,
    /// Enable VNC/noVNC console
    pub vnc_enabled: bool,
    /// Enable SPICE console (optional)
    pub spice_enabled: bool,
    /// Enable serial console where available
    pub serial_enabled: bool,
    /// Enable RDP relay when guest IP available
    pub rdp_relay_enabled: bool,
}

impl Default for ProxmoxConfig {
    fn default() -> Self {
        Self {
            api_url: String::new(),
            auth_mode: "api_token".to_owned(),
            token_id: String::new(),
            token_secret: String::new(),
            node: String::new(),
            vnc_enabled: true,
            spice_enabled: false,
            serial_enabled: true,
            rdp_relay_enabled: true,
        }
    }
}

// ── Proxmox Provider ──────────────────────────────────────────────────────────

pub struct ProxmoxProvider {
    provider_id: String,
    config: ProxmoxConfig,
}

impl ProxmoxProvider {
    pub fn new(provider_id: &str, config: ProxmoxConfig) -> Self {
        Self {
            provider_id: provider_id.to_owned(),
            config,
        }
    }

    fn is_reachable(&self) -> bool {
        !self.config.api_url.is_empty()
    }
}

impl VirtualizationProvider for ProxmoxProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Proxmox
    }

    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    fn list_hosts(&self) -> ProviderResult<Vec<HostInfo>> {
        if !self.is_reachable() {
            return Err(ProviderError::Unavailable(
                "Proxmox API URL not configured".to_owned(),
            ));
        }
        // TODO Phase 1.3: GET /api2/json/nodes
        Ok(vec![HostInfo {
            host_id: format!(
                "{}-node-01",
                self.provider_id
            ),
            hostname: if self.config.node.is_empty() {
                self.config
                    .api_url
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_end_matches(":8006")
                    .to_owned()
            } else {
                self.config.node.clone()
            },
            provider_type: ProviderType::Proxmox,
            cluster: None,
            os_version: Some("Proxmox VE (version pending inventory)".to_owned()),
            provider_metadata: serde_json::json!({
                "proxmox": {
                    "api_url": self.config.api_url,
                    "node": self.config.node,
                }
            }),
        }])
    }

    fn list_vms(&self, _host_id: &str) -> ProviderResult<Vec<VmInfo>> {
        if !self.is_reachable() {
            return Err(ProviderError::Unavailable(
                "Proxmox API URL not configured".to_owned(),
            ));
        }
        // TODO Phase 1.3: GET /api2/json/nodes/{node}/qemu
        Ok(vec![])
    }

    fn get_vm(&self, vm_id: &str) -> ProviderResult<VmInfo> {
        // TODO Phase 1.3: GET /api2/json/nodes/{node}/qemu/{vmid}/status/current
        Err(ProviderError::VmNotFound(vm_id.to_owned()))
    }

    fn get_capabilities(&self, vm_id: &str) -> ProviderResult<VmCapabilityGraph> {
        let vnc = if self.config.vnc_enabled {
            Capability::degraded(
                "VNC_NOT_PROBED",
                "VNC capability not yet probed — inventory needed",
                Some("Connect Proxmox API to enable full probe"),
            )
        } else {
            Capability::unsupported("VNC_DISABLED_BY_CONFIG", "VNC disabled in provider config", None)
        };
        let spice = if self.config.spice_enabled {
            Capability::degraded(
                "SPICE_NOT_PROBED",
                "SPICE capability not yet probed — inventory needed",
                None,
            )
        } else {
            Capability::unsupported("SPICE_DISABLED", "SPICE not configured for this provider", None)
        };
        let serial = if self.config.serial_enabled {
            Capability::degraded(
                "SERIAL_NOT_PROBED",
                "Serial console not yet probed — VM config needed",
                None,
            )
        } else {
            Capability::unsupported("SERIAL_DISABLED", "Serial console disabled in config", None)
        };
        let rdp = if self.config.rdp_relay_enabled {
            Capability::unknown("NO_GUEST_IP_YET")
        } else {
            Capability::unsupported("RDP_DISABLED_BY_CONFIG", "RDP relay disabled", None)
        };
        Ok(VmCapabilityGraph {
            vm_id: vm_id.to_owned(),
            provider_type: Some(ProviderType::Proxmox),
            preview: Capability::unsupported(
                "PREVIEW_REQUIRES_INVENTORY",
                "Preview requires live Proxmox API inventory",
                Some("Connect to Proxmox API"),
            ),
            keyboard_rescue: Capability::unsupported(
                "RESCUE_NOT_SUPPORTED_PROXMOX",
                "WMI keyboard rescue is Hyper-V only",
                None,
            ),
            rdp_relay: rdp,
            vnc_console: vnc,
            spice_console: spice,
            web_console: Capability::degraded(
                "WEB_CONSOLE_VIA_NOVNC",
                "noVNC web console available via Proxmox API ticket (Phase 1.3)",
                None,
            ),
            webmks_console: Capability::unsupported(
                "WEBMKS_VMWARE_ONLY",
                "WebMKS console is VMware only",
                None,
            ),
            serial_console: serial,
            enhanced_session: Capability::unsupported(
                "ENHANCED_SESSION_HYPERV_ONLY",
                "Enhanced Session Mode is Hyper-V only",
                None,
            ),
            hv_socket: Capability::experimental("HVSOCKET_EXPERIMENTAL_DISABLED"),
            clipboard: Capability::unknown("CLIPBOARD_DEPENDS_ON_SESSION"),
            file_transfer: Capability::unsupported(
                "FILE_TRANSFER_NOT_SUPPORTED",
                "File transfer not implemented for Proxmox provider",
                None,
            ),
            recording: Capability::degraded(
                "RECORDING_METADATA_ONLY",
                "Event metadata recording available. Video recording — Phase 2",
                None,
            ),
            experimental: Capability::experimental("EXPERIMENTAL_DISABLED"),
            recommended_mode: SessionMode::VncConsole,
            constraints: vec!["Proxmox provider: inventory not yet connected".to_owned()],
        })
    }

    fn power_action(&self, vm_id: &str, action: &str) -> ProviderResult<()> {
        if !self.is_reachable() {
            return Err(ProviderError::Unavailable("Proxmox API unreachable".to_owned()));
        }
        // TODO Phase 1.3: POST /api2/json/nodes/{node}/qemu/{vmid}/status/{action}
        let _ = (vm_id, action);
        Err(ProviderError::NotSupported(
            "Proxmox power actions: API inventory required (Phase 1.3)".to_owned(),
        ))
    }

    fn list_snapshots(&self, vm_id: &str) -> ProviderResult<Vec<SnapshotInfo>> {
        // TODO Phase 1.3: GET /api2/json/nodes/{node}/qemu/{vmid}/snapshot
        let _ = vm_id;
        Err(ProviderError::NotSupported("Proxmox snapshots: Phase 1.3".to_owned()))
    }

    fn manifest(&self) -> serde_json::Value {
        serde_json::json!({
            "provider_type": "proxmox",
            "provider_id": self.provider_id,
            "api_url": self.config.api_url,
            "node": self.config.node,
            "features_planned": [
                "inventory", "power_control", "snapshots",
                "vnc_console", "novnc_web_console", "serial_console",
                "spice_console", "rdp_relay"
            ],
            "release": "1.3"
        })
    }
}
