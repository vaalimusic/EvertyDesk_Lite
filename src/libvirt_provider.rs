// =============================================================================
// libvirt / KVM Provider — Universal Hypervisor Access Fabric
//
// Skeleton libvirt provider via libvirt API (qemu:///system or remote URI).
// Implements VirtualizationProvider for multi-provider dashboard.
//
// Phase:  Release 1.4 (§50.4 plan).  Current state: skeleton + API stubs.
// Real implementation uses libvirt-sys FFI or libvirt REST (virtproxyd).
//
// Connection modes supported in full implementation:
//   • VNC console (VncConsole) — via domain XML <graphics type='vnc'>
//   • SPICE console (SpiceConsole) — via domain XML <graphics type='spice'>
//   • Serial console (SerialConsole) — virsh console / websocket
//   • RDP relay (RdpRelay) — if guest has RDP + IP via QEMU guest agent
//   • Power management (start/destroy/reboot/suspend/resume)
//   • Snapshots (QEMU external/internal where supported)
//
// Architecture note:
//   libvirt domain XML determines which console types are available.
//   The full provider will parse domain XML and set capabilities accordingly.
// =============================================================================

use crate::{
    capability_engine::{Capability, SessionMode, VmCapabilityGraph},
    provider_api::{
        HostInfo, ProviderError, ProviderResult, ProviderType, SnapshotInfo,
        VirtualizationProvider, VmInfo,
    },
};

// ── libvirt config ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LibvirtConfig {
    /// libvirt connection URI.
    /// Examples:
    ///   "qemu:///system"           — local KVM via Unix socket
    ///   "qemu+ssh://host/system"   — remote via SSH
    ///   "qemu+tls://host/system"   — remote via TLS
    pub uri: String,
    /// Enable VNC console detection from domain XML
    pub vnc_enabled: bool,
    /// Enable SPICE console detection from domain XML
    pub spice_enabled: bool,
    /// Enable serial console via virtproxyd / websocket
    pub serial_enabled: bool,
    /// Enable RDP relay when guest IP available via QEMU agent
    pub rdp_relay_enabled: bool,
}

impl Default for LibvirtConfig {
    fn default() -> Self {
        Self {
            uri: "qemu:///system".to_owned(),
            vnc_enabled: true,
            spice_enabled: true,
            serial_enabled: true,
            rdp_relay_enabled: true,
        }
    }
}

// ── libvirt Provider ──────────────────────────────────────────────────────────

pub struct LibvirtProvider {
    provider_id: String,
    config: LibvirtConfig,
}

impl LibvirtProvider {
    pub fn new(provider_id: &str, config: LibvirtConfig) -> Self {
        Self {
            provider_id: provider_id.to_owned(),
            config,
        }
    }

    fn is_reachable(&self) -> bool {
        !self.config.uri.is_empty()
    }

    /// Capability for VNC given current config state.
    fn vnc_capability(&self) -> Capability {
        if self.config.vnc_enabled {
            Capability::degraded(
                "VNC_NOT_PROBED",
                "VNC availability depends on domain XML — probe needed",
                Some("Connect libvirt to probe domain graphics"),
            )
        } else {
            Capability::unsupported("VNC_DISABLED", "VNC disabled in provider config", None)
        }
    }

    /// Capability for SPICE given current config state.
    fn spice_capability(&self) -> Capability {
        if self.config.spice_enabled {
            Capability::degraded(
                "SPICE_NOT_PROBED",
                "SPICE availability depends on domain XML — probe needed",
                None,
            )
        } else {
            Capability::unsupported("SPICE_DISABLED", "SPICE disabled in provider config", None)
        }
    }

    /// Capability for serial console.
    fn serial_capability(&self) -> Capability {
        if self.config.serial_enabled {
            Capability::degraded(
                "SERIAL_NOT_PROBED",
                "Serial console depends on domain XML <console> device",
                None,
            )
        } else {
            Capability::unsupported("SERIAL_DISABLED", "Serial console disabled", None)
        }
    }
}

impl VirtualizationProvider for LibvirtProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Libvirt
    }

    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    fn list_hosts(&self) -> ProviderResult<Vec<HostInfo>> {
        if !self.is_reachable() {
            return Err(ProviderError::Unavailable(
                "libvirt URI not configured".to_owned(),
            ));
        }
        // TODO Phase 1.4: virConnectGetHostname() + virNodeGetInfo()
        let hostname = self
            .config
            .uri
            .split("//")
            .nth(1)
            .and_then(|s| s.split('/').next())
            .and_then(|s| if s.is_empty() { None } else { Some(s) })
            .unwrap_or("localhost")
            .to_owned();
        Ok(vec![HostInfo {
            host_id: format!("{}-host", self.provider_id),
            hostname,
            provider_type: ProviderType::Libvirt,
            cluster: None,
            os_version: Some("KVM/libvirt (version pending connection)".to_owned()),
            provider_metadata: serde_json::json!({
                "libvirt": { "uri": self.config.uri }
            }),
        }])
    }

    fn list_vms(&self, _host_id: &str) -> ProviderResult<Vec<VmInfo>> {
        if !self.is_reachable() {
            return Err(ProviderError::Unavailable(
                "libvirt URI not configured".to_owned(),
            ));
        }
        // TODO Phase 1.4: virConnectListAllDomains()
        // Parse each domain's XML for graphics devices, IPs via guest agent
        Ok(vec![])
    }

    fn get_vm(&self, vm_id: &str) -> ProviderResult<VmInfo> {
        // TODO Phase 1.4: virDomainLookupByName/UUID + virDomainGetXMLDesc
        Err(ProviderError::VmNotFound(vm_id.to_owned()))
    }

    fn get_capabilities(&self, vm_id: &str) -> ProviderResult<VmCapabilityGraph> {
        // Skeleton: capabilities based on config only (no live domain XML yet)
        Ok(VmCapabilityGraph {
            vm_id: vm_id.to_owned(),
            provider_type: Some(ProviderType::Libvirt),
            preview: Capability::unsupported(
                "PREVIEW_REQUIRES_INVENTORY",
                "Preview requires live libvirt connection and domain XML",
                Some("Connect libvirt to enable preview"),
            ),
            keyboard_rescue: Capability::unsupported(
                "RESCUE_NOT_SUPPORTED_LIBVIRT",
                "WMI keyboard rescue is Hyper-V only; use serial console for rescue",
                Some("Use Serial Console for guest rescue on libvirt/KVM"),
            ),
            rdp_relay: if self.config.rdp_relay_enabled {
                Capability::unknown("NO_GUEST_IP_YET")
            } else {
                Capability::unsupported("RDP_DISABLED", "RDP relay disabled in config", None)
            },
            vnc_console: self.vnc_capability(),
            spice_console: self.spice_capability(),
            web_console: Capability::unsupported(
                "WEB_CONSOLE_NOT_CONFIGURED",
                "Web console requires virtproxyd WebSocket endpoint",
                None,
            ),
            webmks_console: Capability::unsupported(
                "WEBMKS_VMWARE_ONLY",
                "WebMKS is VMware only",
                None,
            ),
            serial_console: self.serial_capability(),
            enhanced_session: Capability::unsupported(
                "ENHANCED_SESSION_HYPERV_ONLY",
                "Enhanced Session Mode is Hyper-V only",
                None,
            ),
            hv_socket: Capability::experimental("HVSOCKET_EXPERIMENTAL_DISABLED"),
            clipboard: Capability::unknown("CLIPBOARD_DEPENDS_ON_SESSION"),
            file_transfer: Capability::unsupported(
                "FILE_TRANSFER_NOT_IMPLEMENTED",
                "File transfer not implemented for libvirt provider",
                None,
            ),
            recording: Capability::degraded(
                "RECORDING_METADATA_ONLY",
                "Event metadata recording available. Video recording — Phase 2",
                None,
            ),
            experimental: Capability::experimental("EXPERIMENTAL_DISABLED"),
            // Prefer serial console for rescue, VNC for interactive
            recommended_mode: if self.config.vnc_enabled {
                SessionMode::VncConsole
            } else if self.config.serial_enabled {
                SessionMode::SerialConsole
            } else {
                SessionMode::PreviewOnly
            },
            constraints: vec!["libvirt provider: not yet connected".to_owned()],
        })
    }

    fn power_action(&self, vm_id: &str, action: &str) -> ProviderResult<()> {
        if !self.is_reachable() {
            return Err(ProviderError::Unavailable(
                "libvirt URI not configured".to_owned(),
            ));
        }
        // TODO Phase 1.4: map action → virDomain{Create,Destroy,Reboot,Suspend,Resume}
        // Action map:
        //   "start"    → virDomainCreate()
        //   "stop"     → virDomainDestroy()      (hard)
        //   "shutdown" → virDomainShutdown()     (graceful via ACPI)
        //   "restart"  → virDomainReboot()
        //   "pause"    → virDomainSuspend()
        //   "resume"   → virDomainResume()
        let _ = (vm_id, action);
        Err(ProviderError::NotSupported(
            "libvirt power actions: Phase 1.4".to_owned(),
        ))
    }

    fn list_snapshots(&self, vm_id: &str) -> ProviderResult<Vec<SnapshotInfo>> {
        // TODO Phase 1.4: virDomainListAllSnapshots()
        // Note: external snapshots on QEMU require qcow2 backing chains
        let _ = vm_id;
        Err(ProviderError::NotSupported(
            "libvirt snapshots: Phase 1.4 (qcow2 only)".to_owned(),
        ))
    }

    fn manifest(&self) -> serde_json::Value {
        serde_json::json!({
            "provider_type": "libvirt",
            "provider_id": self.provider_id,
            "uri": self.config.uri,
            "features_planned": [
                "inventory", "power_control",
                "vnc_console", "spice_console", "serial_console",
                "rdp_relay", "snapshots_qcow2"
            ],
            "release": "1.4"
        })
    }
}
