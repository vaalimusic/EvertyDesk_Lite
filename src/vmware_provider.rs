// =============================================================================
// VMware Provider — Universal Hypervisor Access Fabric
//
// Skeleton VMware vSphere / ESXi provider.
// Implements VirtualizationProvider so that Broker/Client can treat VMware
// VMs identically to Hyper-V VMs via the universal CapabilityGraph.
//
// Phase:  Release 1.2 (§50.2 plan).  Current state: skeleton + HTTP API stub.
// Real implementation requires vSphere REST/SOAP + WebMKS ticket flow.
//
// Connection modes supported in full implementation:
//   • WebMKS / MKS console (WebMksConsole)
//   • RDP Relay (guest IP detected via VMware Tools)
//   • Power management (start/shutdown/reset/suspend)
//   • Snapshots (vSphere snapshot tree)
//   • Preview (screenshot via vSphere API when Tools running)
// =============================================================================

use crate::{
    capability_engine::{Capability, SessionMode, VmCapabilityGraph},
    provider_api::{
        HostInfo, ProviderError, ProviderResult, ProviderType, SnapshotInfo,
        VirtualizationProvider, VmInfo,
    },
};

// ── VMware Provider config ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct VMwareConfig {
    /// vCenter or ESXi URL, e.g. "https://vcenter.example.local"
    pub vcenter_url: String,
    /// Authentication mode: "service_account" | "session_token"
    pub auth_mode: String,
    /// Username for service_account auth
    pub username: String,
    /// Password / token (stored encrypted at rest — see §43)
    pub credential: String,
    /// Enable WebMKS console ticket flow
    pub webmks_enabled: bool,
    /// Enable RDP relay when guest IP available
    pub rdp_relay_enabled: bool,
}

impl Default for VMwareConfig {
    fn default() -> Self {
        Self {
            vcenter_url: String::new(),
            auth_mode: "service_account".to_owned(),
            username: String::new(),
            credential: String::new(),
            webmks_enabled: true,
            rdp_relay_enabled: true,
        }
    }
}

// ── VMware Provider ───────────────────────────────────────────────────────────

pub struct VMwareProvider {
    provider_id: String,
    config: VMwareConfig,
}

impl VMwareProvider {
    pub fn new(provider_id: &str, config: VMwareConfig) -> Self {
        Self {
            provider_id: provider_id.to_owned(),
            config,
        }
    }

    /// Check whether the vCenter API is reachable.
    /// TODO Phase 1.2: implement HTTP ping to vcenter_url/api/about
    fn is_reachable(&self) -> bool {
        !self.config.vcenter_url.is_empty()
    }
}

impl VirtualizationProvider for VMwareProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::VMware
    }

    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    fn list_hosts(&self) -> ProviderResult<Vec<HostInfo>> {
        if !self.is_reachable() {
            return Err(ProviderError::Unavailable(
                "VMware vCenter URL not configured".to_owned(),
            ));
        }
        // TODO Phase 1.2: GET /api/vcenter/host
        // For now return a placeholder so the registry shows the provider
        Ok(vec![HostInfo {
            host_id: format!("{}-esxi-01", self.provider_id),
            hostname: format!(
                "{}",
                self.config
                    .vcenter_url
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
            ),
            provider_type: ProviderType::VMware,
            cluster: None,
            os_version: Some("VMware ESXi (version pending inventory)".to_owned()),
            provider_metadata: serde_json::json!({
                "vmware": { "vcenter_url": self.config.vcenter_url }
            }),
        }])
    }

    fn list_vms(&self, _host_id: &str) -> ProviderResult<Vec<VmInfo>> {
        if !self.is_reachable() {
            return Err(ProviderError::Unavailable(
                "VMware vCenter URL not configured".to_owned(),
            ));
        }
        // TODO Phase 1.2: GET /api/vcenter/vm
        Ok(vec![])
    }

    fn get_vm(&self, vm_id: &str) -> ProviderResult<VmInfo> {
        // TODO Phase 1.2: GET /api/vcenter/vm/{vm_id}
        Err(ProviderError::VmNotFound(vm_id.to_owned()))
    }

    fn get_capabilities(&self, vm_id: &str) -> ProviderResult<VmCapabilityGraph> {
        // Without live inventory we can return a skeleton capability graph
        // showing what VMware would support in principle.
        let webmks = if self.config.webmks_enabled {
            Capability::degraded(
                "WEBMKS_NOT_PROBED",
                "WebMKS capability not yet probed — inventory needed",
                Some("Connect vCenter to enable full probe"),
            )
        } else {
            Capability::unsupported(
                "WEBMKS_DISABLED_BY_CONFIG",
                "WebMKS disabled in provider config",
                None,
            )
        };
        let rdp = if self.config.rdp_relay_enabled {
            Capability::unknown("NO_GUEST_IP_YET")
        } else {
            Capability::unsupported(
                "RDP_DISABLED_BY_CONFIG",
                "RDP relay disabled in provider config",
                None,
            )
        };
        Ok(VmCapabilityGraph {
            vm_id: vm_id.to_owned(),
            provider_type: Some(ProviderType::VMware),
            preview: Capability::unsupported(
                "PREVIEW_REQUIRES_INVENTORY",
                "Preview requires live vCenter inventory",
                Some("Connect to vCenter"),
            ),
            keyboard_rescue: Capability::unsupported(
                "RESCUE_NOT_SUPPORTED_VMWARE",
                "WMI keyboard rescue is Hyper-V only",
                None,
            ),
            rdp_relay: rdp,
            vnc_console: Capability::unsupported(
                "VNC_NOT_SUPPORTED_VMWARE",
                "VMware uses WebMKS/MKS, not VNC",
                None,
            ),
            spice_console: Capability::unsupported(
                "SPICE_NOT_SUPPORTED_VMWARE",
                "VMware does not support SPICE",
                None,
            ),
            web_console: Capability::unsupported(
                "WEB_CONSOLE_NOT_CONFIGURED",
                "Not configured",
                None,
            ),
            webmks_console: webmks,
            serial_console: Capability::unsupported(
                "SERIAL_NOT_SUPPORTED_VMWARE",
                "Serial console not available via this provider",
                None,
            ),
            enhanced_session: Capability::unsupported(
                "ENHANCED_SESSION_HYPERV_ONLY",
                "Enhanced Session Mode is Hyper-V only",
                None,
            ),
            hv_socket: Capability::experimental("HVSOCKET_EXPERIMENTAL_DISABLED"),
            clipboard: Capability::unknown("CLIPBOARD_DEPENDS_ON_SESSION"),
            file_transfer: Capability::unsupported(
                "FILE_TRANSFER_NOT_SUPPORTED",
                "Not implemented for VMware provider",
                None,
            ),
            recording: Capability::degraded(
                "RECORDING_METADATA_ONLY",
                "Event metadata recording available. Video recording — Phase 2",
                None,
            ),
            experimental: Capability::experimental("EXPERIMENTAL_DISABLED"),
            recommended_mode: SessionMode::WebMksConsole,
            constraints: vec!["VMware provider: inventory not yet connected".to_owned()],
        })
    }

    fn power_action(&self, vm_id: &str, action: &str) -> ProviderResult<()> {
        if !self.is_reachable() {
            return Err(ProviderError::Unavailable("vCenter unreachable".to_owned()));
        }
        // TODO Phase 1.2: POST /api/vcenter/vm/{vm_id}/power?action=<action>
        let _ = (vm_id, action);
        Err(ProviderError::NotSupported(
            "VMware power actions: vCenter inventory required (Phase 1.2)".to_owned(),
        ))
    }

    fn list_snapshots(&self, vm_id: &str) -> ProviderResult<Vec<SnapshotInfo>> {
        // TODO Phase 1.2: GET /api/vcenter/vm/{vm_id}/snapshots
        let _ = vm_id;
        Err(ProviderError::NotSupported(
            "VMware snapshots: Phase 1.2".to_owned(),
        ))
    }

    fn manifest(&self) -> serde_json::Value {
        serde_json::json!({
            "provider_type": "vmware",
            "provider_id": self.provider_id,
            "vcenter_url": self.config.vcenter_url,
            "features_planned": [
                "inventory", "power_control", "snapshots",
                "webmks_console", "rdp_relay", "preview_via_tools"
            ],
            "release": "1.2"
        })
    }
}
