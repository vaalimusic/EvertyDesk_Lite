//! Shared VM console runtime used by desktop front-ends.
//!
//! The old egui UI proved the VirtualBox VRDE / Hyper-V RDP path in practice.
//! New front-ends should not reimplement session selection, reconnect handling
//! and text-to-scancode behavior ad hoc; this module is the common boundary
//! around the Windows-only RDP console sessions.

use crate::hyperv_rdp;
use crate::vbox_rdp::{self, Poll, VrdeCmd};

pub const DEFAULT_DESKTOP_SIZE: (u16, u16) = (1280, 800);
pub const VBOX_RECONNECT_COOLDOWN_SECS: u64 = 10;
pub const VBOX_STUCK_REGARDLESS_SECS: u64 = 60;
pub const VBOX_DESYNC_STATUS: &str = "VRDE_DESYNC";

#[derive(Clone, Debug)]
pub enum VmConsoleTarget {
    HyperV {
        vm_guid: String,
        credentials: hyperv_rdp::RdpCredentials,
    },
    VirtualBox {
        vm_uuid: String,
        port: u16,
        settings: vbox_rdp::VrdeSettings,
    },
}

impl VmConsoleTarget {
    pub fn label(&self) -> String {
        match self {
            Self::HyperV { vm_guid, .. } => format!("Hyper-V {vm_guid}"),
            Self::VirtualBox { vm_uuid, port, .. } => format!("VirtualBox {vm_uuid} :{port}"),
        }
    }

    pub fn is_virtualbox(&self) -> bool {
        matches!(self, Self::VirtualBox { .. })
    }
}

pub enum VmConsoleSession {
    HyperV(hyperv_rdp::RdpSession),
    VirtualBox(vbox_rdp::VrdeSession),
}

impl VmConsoleSession {
    pub fn connect(
        target: &VmConsoleTarget,
        desktop_size: (u16, u16),
    ) -> Result<Self, String> {
        match target {
            VmConsoleTarget::HyperV {
                vm_guid,
                credentials,
            } => hyperv_rdp::RdpSession::connect(vm_guid, credentials.clone(), desktop_size)
                .map(Self::HyperV)
                .map_err(|error| error.to_string()),
            VmConsoleTarget::VirtualBox { port, settings, .. } => {
                Ok(Self::VirtualBox(vbox_rdp::VrdeSession::connect(
                    "127.0.0.1",
                    *port,
                    desktop_size,
                    *settings,
                )))
            }
        }
    }

    pub fn poll_frame(&self) -> Poll<(u32, u32, Vec<u8>)> {
        match self {
            Self::HyperV(session) => session.poll_frame(),
            Self::VirtualBox(session) => session.poll_frame(),
        }
    }

    pub fn poll_status(&self) -> Poll<String> {
        match self {
            Self::HyperV(session) => session.poll_status(),
            Self::VirtualBox(session) => session.poll_status(),
        }
    }

    pub fn send(&self, cmd: VrdeCmd) {
        match self {
            Self::HyperV(session) => session.send(cmd),
            Self::VirtualBox(session) => session.send(cmd),
        }
    }

    pub fn stop(self) {
        self.send(VrdeCmd::Stop);
    }
}

pub fn send_text_as_lite(session: &VmConsoleSession, text: &str) {
    for ch in text.chars() {
        // Same behavior as the original egui VM console: VirtualBox VRDE is
        // unreliable with Unicode keyboard PDUs, so use scan codes whenever
        // a character can be mapped. Unicode remains only a last fallback.
        if let Some((scancode, shift, extended)) = vbox_rdp::char_to_rdp_scancode(ch) {
            if shift {
                session.send(VrdeCmd::KeyDown {
                    scancode: 0x2A,
                    extended: false,
                });
            }
            session.send(VrdeCmd::KeyDown { scancode, extended });
            session.send(VrdeCmd::KeyUp { scancode, extended });
            if shift {
                session.send(VrdeCmd::KeyUp {
                    scancode: 0x2A,
                    extended: false,
                });
            }
        } else {
            session.send(VrdeCmd::Text(ch.to_string()));
        }
    }
}

