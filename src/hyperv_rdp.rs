//! RDP over VMBus (Enhanced Session Mode) — Hyper-V host-side AF_HYPERV socket transport.
//!
//! # How Enhanced Session Mode works
//!
//! When Integration Services are installed and the host has Enhanced Session Mode enabled,
//! the VM guest exports an RDP listener over VMBus (not over TCP). The host can connect to
//! it directly using the Windows Hyper-V Socket API (AF_HYPERV = 34).
//!
//! Connection parameters:
//!   - `VmId`      = the VM's GUID (Msvm_ComputerSystem.Name)
//!   - `ServiceId` = {00000883-FACB-11E6-BD58-64006A7986D3}
//!                   (VSock template with port 2179 = 0x883 encoded in Data1 LE)
//!
//! Once connected, the socket carries a standard RDP byte stream — identical to
//! the TCP RDP stream on port 3389, but through VMBus instead.
//!
//! # Current state
//!
//! This module implements the transport layer (AF_HYPERV socket open + connect + I/O).
//! The RDP protocol decode layer (to extract bitmap updates) is a TODO — it requires
//! either `ironrdp` (pure Rust) or FreeRDP (C library via FFI).
//!
//! In the meantime, `RdpProxy` raw-forwards the VMBus byte stream over the EVRT
//! channel to the GUI client, which is expected to decode it.

#![cfg(windows)]

use std::{
    io,
    mem,
    sync::mpsc,
    thread,
    time::Duration,
};

// ── AF_HYPERV constants (from Windows SDK hvsocket.h) ─────────────────────────

/// Address family for Hyper-V sockets (not in the `windows` crate enum).
pub const AF_HYPERV: u16 = 34;

/// Protocol identifier for raw Hyper-V sockets.
pub const HV_PROTOCOL_RAW: i32 = 1;

/// Enhanced Session Mode RDP service GUID.
/// Derived from the VSock port template: port 2179 (0x0883) → Data1.
/// {00000883-FACB-11E6-BD58-64006A7986D3}
pub const ENHANCED_SESSION_SERVICE_ID: GuidBytes = guid_bytes(
    0x0000_0883,
    0xFACB,
    0x11E6,
    [0xBD, 0x58, 0x64, 0x00, 0x6A, 0x79, 0x86, 0xD3],
);

/// GUID as a 16-byte array (Windows mixed-endian layout):
/// Data1 LE (4) + Data2 LE (2) + Data3 LE (2) + Data4 BE (8)
pub type GuidBytes = [u8; 16];

/// Build a GuidBytes array from GUID component form at compile time.
const fn guid_bytes(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> GuidBytes {
    [
        (d1 & 0xFF) as u8,
        ((d1 >> 8) & 0xFF) as u8,
        ((d1 >> 16) & 0xFF) as u8,
        ((d1 >> 24) & 0xFF) as u8,
        (d2 & 0xFF) as u8,
        ((d2 >> 8) & 0xFF) as u8,
        (d3 & 0xFF) as u8,
        ((d3 >> 8) & 0xFF) as u8,
        d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7],
    ]
}

/// Parse a VM GUID string (with or without braces) into GuidBytes.
/// Format: `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}` or without braces.
pub fn parse_vm_guid(s: &str) -> Result<GuidBytes, String> {
    let s = s.trim().trim_matches(|c| c == '{' || c == '}');
    let parts: Vec<&str> = s.splitn(5, '-').collect();
    if parts.len() != 5 {
        return Err(format!("invalid GUID format: {s}"));
    }
    let d1 = u32::from_str_radix(parts[0], 16).map_err(|e| format!("d1: {e}"))?;
    let d2 = u16::from_str_radix(parts[1], 16).map_err(|e| format!("d2: {e}"))?;
    let d3 = u16::from_str_radix(parts[2], 16).map_err(|e| format!("d3: {e}"))?;
    let tail = parts[3].to_owned() + parts[4];
    if tail.len() != 16 {
        return Err(format!("GUID d4 length wrong: {}", tail.len()));
    }
    let mut d4 = [0u8; 8];
    for (i, d4_byte) in d4.iter_mut().enumerate() {
        *d4_byte = u8::from_str_radix(&tail[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("d4[{i}]: {e}"))?;
    }
    Ok(guid_bytes(d1, d2, d3, d4))
}

// ── SOCKADDR_HV raw layout ────────────────────────────────────────────────────

/// Raw bytes of SOCKADDR_HV (36 bytes total):
///   offset  0: sa_family  (u16 LE) = AF_HYPERV = 34
///   offset  2: reserved   (u16)    = 0
///   offset  4: VmId       (16 bytes GUID)
///   offset 20: ServiceId  (16 bytes GUID)
#[repr(C)]
struct SockAddrHv {
    family:     u16,
    reserved:   u16,
    vm_id:      GuidBytes,
    service_id: GuidBytes,
}

impl SockAddrHv {
    fn new(vm_id: GuidBytes, service_id: GuidBytes) -> Self {
        SockAddrHv {
            family: AF_HYPERV,
            reserved: 0,
            vm_id,
            service_id,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                self as *const _ as *const u8,
                mem::size_of::<SockAddrHv>(),
            )
        }
    }
}

// ── Raw WinSock API via windows crate ─────────────────────────────────────────

use windows::Win32::Networking::WinSock::{
    WSAGetLastError, WSAStartup,
    INVALID_SOCKET, SOCKET_ERROR, SOCK_STREAM, WSADATA,
    SEND_RECV_FLAGS,
    closesocket, connect, recv, send,
};
// Re-export the raw SOCKET type.
use windows::Win32::Networking::WinSock::SOCKET as RawSocket;

fn wsa_init() -> io::Result<()> {
    unsafe {
        let mut data = mem::zeroed::<WSADATA>();
        let result = WSAStartup(0x0202, &mut data);
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result));
        }
    }
    Ok(())
}

/// Open a raw AF_HYPERV SOCK_STREAM socket.
fn open_hv_socket() -> io::Result<RawSocket> {
    use windows::Win32::Networking::WinSock::socket;
    let sock = unsafe {
        socket(
            AF_HYPERV as i32,
            SOCK_STREAM,
            HV_PROTOCOL_RAW,
        )
    };
    if sock == INVALID_SOCKET {
        let err = unsafe { WSAGetLastError() };
        return Err(io::Error::from_raw_os_error(err.0));
    }
    Ok(sock)
}

/// Connect to a VM's Enhanced Session RDP endpoint.
fn connect_hv(sock: RawSocket, addr: &SockAddrHv) -> io::Result<()> {
    use windows::Win32::Networking::WinSock::SOCKADDR;
    let bytes = addr.as_bytes();
    let result = unsafe {
        connect(
            sock,
            bytes.as_ptr() as *const SOCKADDR,
            bytes.len() as i32,
        )
    };
    if result == SOCKET_ERROR {
        let err = unsafe { WSAGetLastError() };
        return Err(io::Error::from_raw_os_error(err.0));
    }
    Ok(())
}

// ── Public session API ────────────────────────────────────────────────────────

/// Commands sent from the GUI to the RDP proxy session.
pub enum RdpCmd {
    /// Raw bytes to send into the RDP stream (keyboard/mouse encoded by client).
    Write(Vec<u8>),
    /// Close the session.
    Stop,
}

/// RDP proxy session handle.
/// Raw-forwards the VMBus RDP byte stream in both directions.
pub struct RdpSession {
    /// Send raw bytes into the RDP stream (from client's RDP encoder).
    pub write_tx: mpsc::SyncSender<RdpCmd>,
    /// Receive raw bytes from the RDP stream (bitmap updates for client to decode).
    pub read_rx: mpsc::Receiver<Vec<u8>>,
    /// Human-readable status updates from the session thread.
    pub status_rx: mpsc::Receiver<String>,
}

impl RdpSession {
    /// Open an Enhanced Session connection to the given VM.
    /// `vm_guid` — the VM's identifier (Msvm_ComputerSystem.Name), e.g. `"abcdef01-..."`
    pub fn connect(vm_guid: &str) -> Result<Self, String> {
        let vm_id = parse_vm_guid(vm_guid)?;

        let (write_tx, write_rx) = mpsc::sync_channel::<RdpCmd>(64);
        let (read_tx, read_rx) = mpsc::sync_channel::<Vec<u8>>(32);
        let (status_tx, status_rx) = mpsc::sync_channel::<String>(16);

        let vm_guid_str = vm_guid.to_owned();
        thread::Builder::new()
            .name(format!("hyperv-rdp-{}", &vm_guid_str[..8]))
            .spawn(move || {
                rdp_session_thread(vm_id, write_rx, read_tx, status_tx);
            })
            .map_err(|e| format!("spawn RDP thread: {e}"))?;

        Ok(RdpSession {
            write_tx,
            read_rx,
            status_rx,
        })
    }

    pub fn write(&self, data: Vec<u8>) {
        let _ = self.write_tx.try_send(RdpCmd::Write(data));
    }

    pub fn stop(self) {
        let _ = self.write_tx.try_send(RdpCmd::Stop);
    }

    pub fn try_recv_data(&self) -> Option<Vec<u8>> {
        self.read_rx.try_recv().ok()
    }

    pub fn try_recv_status(&self) -> Option<String> {
        self.status_rx.try_recv().ok()
    }
}

// ── Session thread ────────────────────────────────────────────────────────────

fn rdp_session_thread(
    vm_id: GuidBytes,
    write_rx: mpsc::Receiver<RdpCmd>,
    read_tx: mpsc::SyncSender<Vec<u8>>,
    status_tx: mpsc::SyncSender<String>,
) {
    macro_rules! status {
        ($($arg:tt)*) => {
            let _ = status_tx.try_send(format!($($arg)*));
        };
    }

    if let Err(e) = wsa_init() {
        status!("WSAStartup: {e}");
        return;
    }

    let sock = match open_hv_socket() {
        Ok(s) => s,
        Err(e) => {
            status!("AF_HYPERV socket: {e}");
            return;
        }
    };

    let addr = SockAddrHv::new(vm_id, ENHANCED_SESSION_SERVICE_ID);
    status!("Подключение к Enhanced Session RDP (VMBus)…");
    if let Err(e) = connect_hv(sock, &addr) {
        status!("VMBus connect: {e}");
        unsafe { closesocket(sock) };
        return;
    }
    status!("Enhanced Session: соединение установлено");

    // Set non-blocking mode so we can poll write_rx and recv interleaved.
    // Uses ioctlsocket(FIONBIO, 1).
    use windows::Win32::Networking::WinSock::ioctlsocket;
    let mut nonblock: u32 = 1;
    unsafe {
        ioctlsocket(sock, 0x8004667E_u32 as i32 /* FIONBIO */, &mut nonblock);
    }

    const READ_BUF: usize = 65536;
    let mut read_buf = vec![0u8; READ_BUF];
    let running = true;

    while running {
        // Drain write commands
        loop {
            match write_rx.try_recv() {
                Ok(RdpCmd::Stop) => {
                    status!("RDP сессия закрыта");
                    unsafe { closesocket(sock) };
                    return;
                }
                Ok(RdpCmd::Write(data)) => {
                    let mut sent = 0;
                    while sent < data.len() {
                        let result = unsafe {
                            send(sock, &data[sent..], SEND_RECV_FLAGS(0))
                        };
                        if result == SOCKET_ERROR {
                            let err = unsafe { WSAGetLastError() };
                            // WSAEWOULDBLOCK (10035) — socket not ready, retry
                            if err.0 == 10035 {
                                thread::sleep(Duration::from_millis(1));
                                continue;
                            }
                            status!("RDP send error: {}", err.0);
                            unsafe { closesocket(sock) };
                            return;
                        }
                        sent += result as usize;
                    }
                }
                Err(_) => break,
            }
        }

        // Read available data from VMBus RDP stream
        let n = unsafe {
            recv(sock, &mut read_buf, SEND_RECV_FLAGS(0))
        };
        if n > 0 {
            let _ = read_tx.try_send(read_buf[..n as usize].to_vec());
        } else if n == SOCKET_ERROR {
            let err = unsafe { WSAGetLastError() };
            if err.0 != 10035 {
                // Fatal error (not WSAEWOULDBLOCK)
                status!("RDP recv error: {}", err.0);
                unsafe { closesocket(sock) };
                return;
            }
        } else if n == 0 {
            // Connection closed by peer (VM shut down / IS stopped)
            status!("VMBus: соединение закрыто гостем");
            unsafe { closesocket(sock) };
            return;
        }

        thread::sleep(Duration::from_millis(1));
    }

    unsafe { closesocket(sock) };
}

// ── Availability check ────────────────────────────────────────────────────────

/// Quick probe: can we open an AF_HYPERV socket at all?
/// Returns false if Hyper-V is not the running hypervisor or the OS is too old.
pub fn is_hv_socket_available() -> bool {
    if wsa_init().is_err() {
        return false;
    }
    match open_hv_socket() {
        Ok(sock) => {
            unsafe { closesocket(sock) };
            true
        }
        Err(_) => false,
    }
}
