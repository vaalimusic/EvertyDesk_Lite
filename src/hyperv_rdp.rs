//! Hyper-V Enhanced Session Mode — RDP via the host's VM-connection broker,
//! decoded in-process with the same `ironrdp` engine used for VirtualBox VRDE.
//!
//! # How it actually works (the `vmconnect.exe` mechanism)
//!
//! Enhanced Session is NOT a direct hv_sock RDP listener in the guest (an
//! earlier version of this file tried `AF_HYPERV` to the guest and timed out —
//! os error 10060). Instead, the Hyper-V host runs a VM-connection broker
//! (`vmms`) listening on TCP **127.0.0.1:2179**. A client:
//!   1. opens a plain TCP connection to that port,
//!   2. sends an `RDP_PRECONNECTION_PDU_V2` whose string payload is the target
//!      VM's GUID, as the very first bytes — this tells the broker which VM to
//!      route to,
//!   3. then performs a completely standard RDP handshake; the broker proxies
//!      it over VMBus into the guest's RDP server.
//!
//! So the transport is ordinary TCP+TLS (same as VRDE), the only Hyper-V-
//! specific step is the preconnection blob. Everything after that reuses the
//! shared `ironrdp` helpers from `vbox_rdp`.

#![cfg(windows)]

use std::{
    fmt,
    fs::{File, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use native_tls::TlsConnector;

use ironrdp_blocking::{connect_begin, mark_as_upgraded, single_sequence_step, Framed};
use ironrdp_connector::{
    BitmapConfig, ClientConnector, ClientConnectorState, Config, ConnectionResult, Credentials,
    DesktopSize,
};
use ironrdp_core::WriteBuf;
use ironrdp_graphics::{image_processing::PixelFormat, pointer::DecodedPointer};
use ironrdp_pdu::{
    gcc::KeyboardType,
    input::{
        fast_path::{FastPathInputEvent, KeyboardFlags as FastKeyboardFlags},
        mouse::{MousePdu, PointerFlags},
    },
    pcb::{PcbVersion, PreconnectionBlob},
    rdp::capability_sets::MajorPlatformType,
};
use ironrdp_session::{image::DecodedImage, ActiveStage, ActiveStageOutput};

// Reuse the generic, transport-agnostic RDP plumbing from the VirtualBox path.
use crate::vbox_rdp::{
    char_to_rdp_scancode, composite_cursor, is_ignorable_pdu_error, is_transient_read_error,
    sanitize_desktop_size, send_fastpath_input, Poll, VrdeCmd,
};

/// Hyper-V VM-connection broker port on the host.
const HYPERV_VMCONNECT_PORT: u16 = 2179;

// ── Settings / handle ──────────────────────────────────────────────────────────

/// Guest credentials for Enhanced Session. Empty = try a graphical-login
/// connection first; the connect log reveals whether the broker/guest demands
/// NLA/CredSSP.
#[derive(Clone, Debug, Default)]
pub struct RdpCredentials {
    pub username: String,
    pub password: String,
    pub domain: String,
}

pub struct RdpSession {
    cmd_tx: mpsc::Sender<VrdeCmd>,
    frame_rx: mpsc::Receiver<(u32, u32, Vec<u8>)>,
    status_rx: mpsc::Receiver<String>,
}

impl RdpSession {
    /// Open an Enhanced Session connection to `vm_guid` (Msvm_ComputerSystem.Name).
    pub fn connect(
        vm_guid: &str,
        creds: RdpCredentials,
        desktop_size: (u16, u16),
    ) -> Result<Self, String> {
        // Normalize to a bare lowercase GUID (no braces) for the preconnection
        // blob — that's the form vmconnect/FreeRDP send.
        let vm_guid = vm_guid
            .trim()
            .trim_matches(|c| c == '{' || c == '}')
            .to_lowercase();
        if vm_guid.split('-').count() != 5 {
            return Err(format!("invalid VM GUID: {vm_guid}"));
        }

        let (cmd_tx, cmd_rx) = mpsc::channel::<VrdeCmd>();
        let (frame_tx, frame_rx) = mpsc::sync_channel::<(u32, u32, Vec<u8>)>(2);
        let (status_tx, status_rx) = mpsc::sync_channel::<String>(128);

        let short = vm_guid.chars().take(8).collect::<String>();
        thread::Builder::new()
            .name(format!("hyperv-rdp-{short}"))
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    hv_rdp_thread(vm_guid, creds, desktop_size, cmd_rx, frame_tx, status_tx);
                }));
                if let Err(payload) = result {
                    let msg = payload
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_owned())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "non-string panic payload".to_owned());
                    hv_log_line(&format!("HV-RDP: !!! session thread PANICKED: {msg}"));
                }
            })
            .map_err(|e| format!("spawn HV-RDP thread: {e}"))?;

        Ok(RdpSession {
            cmd_tx,
            frame_rx,
            status_rx,
        })
    }

    pub fn send(&self, cmd: VrdeCmd) {
        let _ = self.cmd_tx.send(cmd);
    }
    pub fn stop(self) {
        let _ = self.cmd_tx.send(VrdeCmd::Stop);
    }
    pub fn poll_frame(&self) -> Poll<(u32, u32, Vec<u8>)> {
        Poll::from(self.frame_rx.try_recv())
    }
    pub fn poll_status(&self) -> Poll<String> {
        Poll::from(self.status_rx.try_recv())
    }
}

// ── Logging (separate file from the VRDE log) ───────────────────────────────────

fn hv_log_path() -> Option<PathBuf> {
    Some(std::env::temp_dir().join("evertydesk-hvrdp.log"))
}

fn open_hv_log() -> Option<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(hv_log_path()?)
        .ok()
}

fn hv_log(file: &mut Option<File>, args: fmt::Arguments<'_>) {
    if let Some(f) = file.as_mut() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let _ = writeln!(f, "[{ts}] {args}");
        let _ = f.flush();
    }
}

fn hv_log_line(msg: &str) {
    let mut f = open_hv_log();
    hv_log(&mut f, format_args!("{msg}"));
}

// ── Session thread ──────────────────────────────────────────────────────────────

fn hv_rdp_thread(
    vm_guid: String,
    creds: RdpCredentials,
    desktop_size: (u16, u16),
    cmd_rx: mpsc::Receiver<VrdeCmd>,
    frame_tx: mpsc::SyncSender<(u32, u32, Vec<u8>)>,
    status_tx: mpsc::SyncSender<String>,
) {
    let mut log_file = open_hv_log();
    hv_log(
        &mut log_file,
        format_args!("--- HV-RDP session start vm={vm_guid} ---"),
    );

    macro_rules! status {
        ($($t:tt)*) => {{
            let msg = format!($($t)*);
            hv_log(&mut log_file, format_args!("{}", msg));
            let _ = status_tx.try_send(msg);
        }};
    }
    macro_rules! diag {
        ($($t:tt)*) => {{ hv_log(&mut log_file, format_args!($($t)*)); }};
    }

    // ── TCP to the host VM-connection broker ─────────────────────────────────
    let addr: SocketAddr = format!("127.0.0.1:{HYPERV_VMCONNECT_PORT}")
        .parse()
        .unwrap();
    status!("HV-RDP: подключение к брокеру 127.0.0.1:{HYPERV_VMCONNECT_PORT}…");
    let mut tcp = match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
        Ok(s) => s,
        Err(e) => {
            status!("HV-RDP: TCP {HYPERV_VMCONNECT_PORT}: {e} (Hyper-V роль не установлена?)");
            return;
        }
    };
    let _ = tcp.set_nodelay(true);

    // ── Preconnection Blob V2: tells the broker which VM to route to ─────────
    let pcb = PreconnectionBlob {
        version: PcbVersion::V2,
        id: 0,
        v2_payload: Some(vm_guid.clone()),
    };
    let pcb_bytes = match ironrdp_core::encode_vec(&pcb) {
        Ok(b) => b,
        Err(e) => {
            status!("HV-RDP: encode preconnection blob: {e}");
            return;
        }
    };
    if let Err(e) = tcp.write_all(&pcb_bytes) {
        status!("HV-RDP: send preconnection blob: {e}");
        return;
    }
    diag!(
        "HV-RDP: preconnection blob отправлен ({} байт, VmId={vm_guid})",
        pcb_bytes.len()
    );

    let (desktop_width, desktop_height) = sanitize_desktop_size(desktop_size.0, desktop_size.1);

    let config = Config {
        desktop_size: DesktopSize {
            width: desktop_width,
            height: desktop_height,
        },
        desktop_scale_factor: 0,
        enable_tls: true,
        // Start without CredSSP/NLA; the X.224 negotiation log shows whether the
        // guest insists on HYBRID, at which point we add it.
        enable_credssp: false,
        credentials: Credentials::UsernamePassword {
            username: creds.username.clone(),
            password: creds.password.clone(),
        },
        domain: if creds.domain.is_empty() {
            None
        } else {
            Some(creds.domain.clone())
        },
        client_build: 0x0A28_0000,
        client_name: "EvertyDesk".to_owned(),
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_functional_keys_count: 12,
        keyboard_layout: 0x0409,
        ime_file_name: String::new(),
        bitmap: Some(BitmapConfig {
            lossy_compression: false,
            color_depth: 32,
            codecs: Default::default(),
        }),
        dig_product_id: String::new(),
        client_dir: String::new(),
        alternate_shell: String::new(),
        work_dir: String::new(),
        platform: MajorPlatformType::WINDOWS,
        hardware_id: None,
        request_data: None,
        autologon: !creds.username.is_empty(),
        enable_audio_playback: false,
        performance_flags: Default::default(),
        license_cache: None,
        timezone_info: Default::default(),
        compression_type: None,
        enable_server_pointer: true,
        pointer_software_rendering: false,
        multitransport_flags: None,
    };

    let client_addr: SocketAddr = tcp
        .local_addr()
        .unwrap_or_else(|_| "127.0.0.1:0".parse().unwrap());
    let mut connector = ClientConnector::new(config, client_addr);
    let mut framed = Framed::new(tcp);

    // ── X.224 negotiation ────────────────────────────────────────────────────
    status!("HV-RDP: X.224 переговоры…");
    let should_upgrade = match connect_begin(&mut framed, &mut connector) {
        Ok(u) => u,
        Err(e) => {
            use std::error::Error as StdError;
            let mut chain = format!("{e}");
            let mut src: Option<&dyn StdError> = e.source();
            while let Some(next) = src {
                chain.push_str(&format!(" <- {next}"));
                src = next.source();
            }
            status!("HV-RDP: ошибка X.224: {chain}");
            return;
        }
    };

    let needs_tls = connector.should_perform_security_upgrade();
    diag!("HV-RDP: should_perform_security_upgrade={needs_tls}");

    let (raw, _leftover) = framed.into_inner();
    if needs_tls {
        status!("HV-RDP: TLS рукопожатие…");
        let tls_connector = match TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                status!("HV-RDP: TLS build: {e}");
                return;
            }
        };
        let tls_stream = match tls_connector.connect("hyperv-vm", raw) {
            Ok(s) => s,
            Err(e) => {
                status!("HV-RDP: TLS ошибка: {e}");
                return;
            }
        };
        let mut tls_framed = Framed::new(tls_stream);
        let upgraded = mark_as_upgraded(should_upgrade, &mut connector);
        match hv_connect_finalize(upgraded, connector, &mut tls_framed) {
            Ok(r) => run_active_session(r, tls_framed, cmd_rx, frame_tx, status_tx, log_file),
            Err(e) => {
                let mut f = log_file;
                hv_log(&mut f, format_args!("HV-RDP: финализация (TLS): {e}"));
                let _ = status_tx.try_send(format!("HV-RDP: финализация: {e}"));
            }
        }
    } else {
        let mut plain_framed = Framed::new(raw);
        let upgraded = mark_as_upgraded(should_upgrade, &mut connector);
        match hv_connect_finalize(upgraded, connector, &mut plain_framed) {
            Ok(r) => run_active_session(r, plain_framed, cmd_rx, frame_tx, status_tx, log_file),
            Err(e) => {
                let mut f = log_file;
                hv_log(&mut f, format_args!("HV-RDP: финализация (plain): {e}"));
                let _ = status_tx.try_send(format!("HV-RDP: финализация: {e}"));
            }
        }
    }
}

/// Standard finalize loop (no VirtualBox empty-FontMap tolerance — a Windows
/// guest's RDP server is spec-compliant). Returns the full error chain so the
/// first real connect is debuggable from the log.
fn hv_connect_finalize<S: Read + Write>(
    _: ironrdp_blocking::Upgraded,
    mut connector: ClientConnector,
    framed: &mut Framed<S>,
) -> Result<ConnectionResult, String> {
    use std::error::Error as StdError;
    let mut buf = WriteBuf::new();
    loop {
        if let Err(e) = single_sequence_step(framed, &mut connector, &mut buf) {
            let mut chain = format!("{e}");
            let mut src: Option<&dyn StdError> = e.source();
            while let Some(next) = src {
                chain.push_str(&format!(" <- {next}"));
                src = next.source();
            }
            return Err(chain);
        }
        if let ClientConnectorState::Connected { result } = connector.state {
            return Ok(result);
        }
    }
}

fn run_active_session<S: Read + Write>(
    connection_result: ConnectionResult,
    mut framed: Framed<S>,
    cmd_rx: mpsc::Receiver<VrdeCmd>,
    frame_tx: mpsc::SyncSender<(u32, u32, Vec<u8>)>,
    status_tx: mpsc::SyncSender<String>,
    mut log_file: Option<File>,
) {
    macro_rules! status {
        ($($t:tt)*) => {{
            let msg = format!($($t)*);
            hv_log(&mut log_file, format_args!("{}", msg));
            let _ = status_tx.try_send(msg);
        }};
    }
    macro_rules! diag {
        ($($t:tt)*) => {{ hv_log(&mut log_file, format_args!($($t)*)); }};
    }

    let width = connection_result.desktop_size.width;
    let height = connection_result.desktop_size.height;
    status!("HV-RDP: подключено {}×{}", width, height);

    let mut active = ActiveStage::new(connection_result);
    let mut image = DecodedImage::new(PixelFormat::RgbA32, width, height);

    let mut cur_x: u16 = 0;
    let mut cur_y: u16 = 0;
    let mut cursor_shape: Option<std::sync::Arc<DecodedPointer>> = None;
    let mut had_update = false;

    loop {
        loop {
            match cmd_rx.try_recv() {
                Ok(VrdeCmd::Stop) => {
                    status!("HV-RDP: сессия закрыта");
                    return;
                }
                Ok(VrdeCmd::MouseMove { x, y }) => {
                    cur_x = x;
                    cur_y = y;
                    match send_fastpath_input(
                        &mut active,
                        &mut image,
                        &mut framed,
                        &[FastPathInputEvent::MouseEvent(MousePdu {
                            flags: PointerFlags::MOVE,
                            number_of_wheel_rotation_units: 0,
                            x_position: x,
                            y_position: y,
                        })],
                    ) {
                        Ok(update) => had_update |= update,
                        Err(e) => diag!("HV-RDP input mouse move error: {e}"),
                    }
                    if cursor_shape.is_some() {
                        had_update = true;
                    }
                }
                Ok(VrdeCmd::MouseButton { button, down }) => {
                    let btn = match button {
                        0 => PointerFlags::LEFT_BUTTON,
                        1 => PointerFlags::RIGHT_BUTTON,
                        _ => PointerFlags::MIDDLE_BUTTON_OR_WHEEL,
                    };
                    let flags = if down { btn | PointerFlags::DOWN } else { btn };
                    match send_fastpath_input(
                        &mut active,
                        &mut image,
                        &mut framed,
                        &[FastPathInputEvent::MouseEvent(MousePdu {
                            flags,
                            number_of_wheel_rotation_units: 0,
                            x_position: cur_x,
                            y_position: cur_y,
                        })],
                    ) {
                        Ok(update) => had_update |= update,
                        Err(e) => diag!("HV-RDP input mouse button error: {e}"),
                    }
                }
                Ok(VrdeCmd::MouseWheel { delta }) => {
                    match send_fastpath_input(
                        &mut active,
                        &mut image,
                        &mut framed,
                        &[FastPathInputEvent::MouseEvent(MousePdu {
                            flags: PointerFlags::VERTICAL_WHEEL,
                            number_of_wheel_rotation_units: delta,
                            x_position: cur_x,
                            y_position: cur_y,
                        })],
                    ) {
                        Ok(update) => had_update |= update,
                        Err(e) => diag!("HV-RDP input wheel error: {e}"),
                    }
                }
                Ok(VrdeCmd::KeyDown { scancode, extended }) => {
                    match send_fastpath_input(
                        &mut active,
                        &mut image,
                        &mut framed,
                        &[fast_key_event(scancode, extended, false)],
                    ) {
                        Ok(update) => had_update |= update,
                        Err(e) => diag!("HV-RDP input key down error: {e}"),
                    }
                }
                Ok(VrdeCmd::KeyUp { scancode, extended }) => {
                    match send_fastpath_input(
                        &mut active,
                        &mut image,
                        &mut framed,
                        &[fast_key_event(scancode, extended, true)],
                    ) {
                        Ok(update) => had_update |= update,
                        Err(e) => diag!("HV-RDP input key up error: {e}"),
                    }
                }
                Ok(VrdeCmd::Text(text)) => {
                    for ch in text.chars() {
                        if let Some((scancode, shift, extended)) = char_to_rdp_scancode(ch) {
                            let mut events = Vec::with_capacity(4);
                            if shift {
                                events.push(fast_key_event(0x2A, false, false));
                            }
                            events.push(fast_key_event(scancode, extended, false));
                            events.push(fast_key_event(scancode, extended, true));
                            if shift {
                                events.push(fast_key_event(0x2A, false, true));
                            }
                            match send_fastpath_input(&mut active, &mut image, &mut framed, &events)
                            {
                                Ok(update) => had_update |= update,
                                Err(e) => diag!("HV-RDP input text scancode error: {e}"),
                            }
                        } else {
                            let mut units = [0u16; 2];
                            for unit in ch.encode_utf16(&mut units).iter().copied() {
                                match send_fastpath_input(
                                    &mut active,
                                    &mut image,
                                    &mut framed,
                                    &[
                                        fast_unicode_event(unit, false),
                                        fast_unicode_event(unit, true),
                                    ],
                                ) {
                                    Ok(update) => had_update |= update,
                                    Err(e) => diag!("HV-RDP input unicode error: {e}"),
                                }
                            }
                        }
                    }
                }
                Ok(VrdeCmd::Resize { .. }) => {}
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        let (action, frame) = match framed.read_pdu() {
            Ok(f) => f,
            Err(ref e) if is_transient_read_error(e) => {
                if had_update {
                    let _ = send_frame(&image, &cursor_shape, cur_x, cur_y, &frame_tx);
                    had_update = false;
                }
                continue;
            }
            Err(e) => {
                status!("HV-RDP: ошибка чтения: {e}");
                return;
            }
        };

        let outputs = match active.process(&mut image, action, &frame) {
            Ok(o) => o,
            Err(e) => {
                if is_ignorable_pdu_error(&e) {
                    continue;
                }
                diag!("HV-RDP PDU error: {e}");
                continue;
            }
        };

        for output in outputs {
            match output {
                ActiveStageOutput::ResponseFrame(bytes) => {
                    let _ = framed.write_all(&bytes);
                }
                ActiveStageOutput::GraphicsUpdate(_) => had_update = true,
                ActiveStageOutput::PointerBitmap(p) => {
                    cursor_shape = Some(p);
                    had_update = true;
                }
                ActiveStageOutput::PointerHidden | ActiveStageOutput::PointerDefault => {
                    cursor_shape = None;
                    had_update = true;
                }
                ActiveStageOutput::PointerPosition { .. } => had_update = true,
                ActiveStageOutput::Terminate(reason) => {
                    status!("HV-RDP: отключение: {reason}");
                    return;
                }
                ActiveStageOutput::DeactivateAll(_) => {
                    diag!("HV-RDP: DeactivateAll (reactivation not yet driven)");
                }
                _ => {}
            }
        }

        if had_update {
            let _ = send_frame(&image, &cursor_shape, cur_x, cur_y, &frame_tx);
            had_update = false;
        }
    }
}

fn fast_key_event(scancode: u8, extended: bool, release: bool) -> FastPathInputEvent {
    let mut flags = if release {
        FastKeyboardFlags::RELEASE
    } else {
        FastKeyboardFlags::empty()
    };
    if extended {
        flags |= FastKeyboardFlags::EXTENDED;
    }
    FastPathInputEvent::KeyboardEvent(flags, scancode)
}

fn fast_unicode_event(unit: u16, release: bool) -> FastPathInputEvent {
    let flags = if release {
        FastKeyboardFlags::RELEASE
    } else {
        FastKeyboardFlags::empty()
    };
    FastPathInputEvent::UnicodeKeyboardEvent(flags, unit)
}

fn send_frame(
    image: &DecodedImage,
    cursor_shape: &Option<std::sync::Arc<DecodedPointer>>,
    mouse_x: u16,
    mouse_y: u16,
    frame_tx: &mpsc::SyncSender<(u32, u32, Vec<u8>)>,
) -> bool {
    let w = image.width() as usize;
    let h = image.height() as usize;
    let mut rgba = image.data().to_vec();
    for px in rgba.chunks_exact_mut(4) {
        px[3] = 255;
    }
    if let Some(c) = cursor_shape {
        composite_cursor(&mut rgba, w, h, c, mouse_x, mouse_y);
    }
    frame_tx.try_send((w as u32, h as u32, rgba)).is_ok()
}
