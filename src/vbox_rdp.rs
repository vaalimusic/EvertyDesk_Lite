//! VirtualBox VRDE embedded RDP client — 30fps без внешних программ.
//!
//! Подключается к встроенному RDP-серверу VirtualBox (VRDE) по TCP,
//! декодирует bitmap-апдейты через ironrdp и доставляет RGBA-кадры в UI.
//!
//! Использование:
//! 1. Включить VRDE: `virtualbox::enable_vrde(uuid, port, running)`
//! 2. Создать сессию: `VrdeSession::connect("127.0.0.1", port)`
//! 3. Получать кадры: `session.try_recv_frame()` → Option<(w, h, Vec<u8>)>
//! 4. Отправлять ввод: `session.send(VrdeCmd::...)`

use std::{
    fmt,
    fs::{File, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use native_tls::TlsConnector;

// ── Spy stream: логирует первые READ_LOG_BYTES байтов приходящих от сервера ───

struct SpyStream<S> {
    inner: S,
    log: Arc<Mutex<Vec<u8>>>,
    limit: usize,
}

impl<S: Read> Read for SpyStream<S> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        let mut log = self.log.lock().unwrap();
        if log.len() < self.limit {
            let take = (self.limit - log.len()).min(n);
            log.extend_from_slice(&buf[..take]);
        }
        Ok(n)
    }
}

impl<S: Write> Write for SpyStream<S> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

use ironrdp_blocking::{connect_begin, mark_as_upgraded, single_sequence_step, Framed, Upgraded};
use ironrdp_connector::{
    connection_activation::{ConnectionActivationSequence, ConnectionActivationState},
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
        scan_code::KeyboardFlags as SlowKeyboardFlags,
        unicode::KeyboardFlags as SlowUnicodeFlags,
        InputEvent, InputEventPdu, ScanCodePdu, UnicodePdu,
    },
    rdp::{capability_sets::MajorPlatformType, headers::ShareDataPdu},
};
use ironrdp_session::{image::DecodedImage, ActiveStage, ActiveStageOutput};

// ── Команды в сессию ─────────────────────────────────────────────────────────

pub enum VrdeCmd {
    /// Движение мыши (координаты в пикселях экрана гостя).
    MouseMove {
        x: u16,
        y: u16,
    },
    /// Кнопка мыши: 0=левая, 1=правая, 2=средняя; down=true/false.
    MouseButton {
        button: u8,
        down: bool,
    },
    /// Вертикальная прокрутка. Положительное = вверх, по аналогии с Windows
    /// WHEEL_DELTA (120 на одно "деление" колеса).
    MouseWheel {
        delta: i16,
    },
    /// Нажатие клавиши (скан-код PS/2 Set-1, совместимый с Windows RDP).
    KeyDown {
        scancode: u8,
        extended: bool,
    },
    /// Отпускание клавиши.
    KeyUp {
        scancode: u8,
        extended: bool,
    },
    /// Печатный текст через RDP Unicode keyboard events.
    Text(String),
    Resize {
        width: u16,
        height: u16,
    },
    /// Закрыть сессию.
    Stop,
}

/// User-adjustable connection parameters, exposed via the settings gear on
/// the VM console page rather than hardcoded — both affect how much data
/// flows over the wire and therefore how often the bulk-compression path is
/// exercised, which is where most of the VirtualBox-specific instability we
/// found actually lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VrdeSettings {
    /// 16 or 32. VirtualBox's own documentation recommends 16bpp for RDP
    /// viewers ("we recommend [16bpp]... for best performance"); half the
    /// bytes per pixel means half the data the bulk compressor has to keep
    /// up with.
    pub color_depth: u32,
    pub compression: CompressionChoice,
}

impl Default for VrdeSettings {
    fn default() -> Self {
        Self {
            color_depth: 32,
            compression: CompressionChoice::K64,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressionChoice {
    /// MPPC, 8 KB history.
    K8,
    /// MPPC, 64 KB history — the default; a larger window means the ring
    /// wraps less often.
    K64,
}

impl CompressionChoice {
    pub fn label(&self) -> &'static str {
        match self {
            Self::K8 => "K8 (MPPC, 8 КБ)",
            Self::K64 => "K64 (MPPC, 64 КБ)",
        }
    }

    fn to_ironrdp(self) -> ironrdp_pdu::rdp::client_info::CompressionType {
        match self {
            Self::K8 => ironrdp_pdu::rdp::client_info::CompressionType::K8,
            Self::K64 => ironrdp_pdu::rdp::client_info::CompressionType::K64,
        }
    }
}

// ── Хэндл сессии ─────────────────────────────────────────────────────────────

pub struct VrdeSession {
    cmd_tx: mpsc::Sender<VrdeCmd>,
    /// (width, height, RGBA)
    pub frame_rx: mpsc::Receiver<(u32, u32, Vec<u8>)>,
    pub status_rx: mpsc::Receiver<String>,
}

impl VrdeSession {
    /// Подключиться к VRDE-серверу VirtualBox на `host:port`.
    pub fn connect(
        host: &str,
        port: u16,
        desktop_size: (u16, u16),
        settings: VrdeSettings,
    ) -> Self {
        install_panic_logging_hook();
        let (cmd_tx, cmd_rx) = mpsc::channel::<VrdeCmd>();
        let (frame_tx, frame_rx) = mpsc::sync_channel::<(u32, u32, Vec<u8>)>(2);
        let (status_tx, status_rx) = mpsc::sync_channel::<String>(128);

        let host = host.to_owned();
        thread::Builder::new()
            .name(format!("vbox-vrde-{port}"))
            .spawn(move || {
                // Every intentional exit from vrde_thread logs a reason via
                // status!()/diag!() before returning — but a *panic* unwinds
                // straight past all of that and just drops the channels,
                // which is exactly what `Poll::Dead` on the main.rs side
                // detects. Without this wrapper, "the channel disconnected"
                // and "why" were two separate, disconnected facts: the
                // disconnect was visible, but a panic payload normally only
                // goes to stderr (easy to miss for a windowed app with no
                // attached console), so every log we had ended right before
                // the actual cause. Catching it here and writing it to the
                // SAME log file closes that gap.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    vrde_thread(
                        host,
                        port,
                        desktop_size,
                        settings,
                        cmd_rx,
                        frame_tx,
                        status_tx,
                    );
                }));
                if let Err(payload) = result {
                    let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                        (*s).to_owned()
                    } else if let Some(s) = payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "non-string panic payload".to_owned()
                    };
                    log_from_ui(&format!("VRDE: !!! session thread PANICKED: {msg}"));
                }
            })
            .expect("spawn vbox-vrde thread");

        VrdeSession {
            cmd_tx,
            frame_rx,
            status_rx,
        }
    }

    pub fn send(&self, cmd: VrdeCmd) {
        let _ = self.cmd_tx.send(cmd);
    }

    pub fn stop(self) {
        let _ = self.cmd_tx.send(VrdeCmd::Stop);
    }

    /// Poll for a frame without losing information: every caller gets
    /// exactly one outcome — a frame, "nothing yet", or "the thread is
    /// gone" — from the same single channel read. An earlier version had a
    /// separate `is_dead()` that did its own `try_recv()` purely to inspect
    /// whether the channel had disconnected; if an actual frame happened to
    /// be queued at that exact moment, `try_recv()` would dequeue and
    /// discard it right there; `is_dead()` only checked the result against
    /// `Disconnected`, so a perfectly good frame would just vanish. Folding
    /// "what came back" and "is it dead" into one `Poll` return removes that
    /// failure mode structurally — there is no longer a code path that can
    /// read a message and not hand it to the caller.
    pub fn poll_frame(&self) -> Poll<(u32, u32, Vec<u8>)> {
        Poll::from(self.frame_rx.try_recv())
    }

    /// Same contract as `poll_frame`, for the status-text channel.
    pub fn poll_status(&self) -> Poll<String> {
        Poll::from(self.status_rx.try_recv())
    }
}

/// Outcome of polling one of `VrdeSession`'s channels: a new item, nothing
/// pending right now, or the producer thread has exited and the channel will
/// never yield anything again. Modeling all three as one enum (rather than
/// `Option<T>` plus a side `is_dead()` boolean check) is what makes it
/// impossible to observe "dead" without also being handed whatever the last
/// real message was — there is exactly one read of the channel, and exactly
/// one place that consumes its result.
pub enum Poll<T> {
    Item(T),
    Empty,
    Dead,
}

impl<T> From<Result<T, mpsc::TryRecvError>> for Poll<T> {
    fn from(r: Result<T, mpsc::TryRecvError>) -> Self {
        match r {
            Ok(v) => Poll::Item(v),
            Err(mpsc::TryRecvError::Empty) => Poll::Empty,
            Err(mpsc::TryRecvError::Disconnected) => Poll::Dead,
        }
    }
}

// ── VirtualBox-compatible connect_finalize ────────────────────────────────────
//
// VirtualBox VRDE ends connection finalization with a Font Map PDU whose body
// is 0 bytes long.  ironrdp-pdu's FontPdu::decode unconditionally requires 8 bytes,
// so the standard connect_finalize fails.  This wrapper runs the same step loop
// but treats the specific "FontPdu / 0 bytes" decode error as a successful finish,
// reconstructing ConnectionResult from the state snapshotted just before the step.
fn vbox_connect_finalize<S: Read + Write>(
    _: Upgraded,
    mut connector: ClientConnector,
    framed: &mut Framed<S>,
) -> ironrdp_connector::ConnectorResult<ConnectionResult> {
    use std::error::Error as StdError;
    let mut buf = WriteBuf::new();
    // Snapshot of the finalization-phase IDs captured before each step so we
    // can build ConnectionResult if ironrdp rejects VirtualBox's empty FontMap.
    let mut saved: Option<(u16, u16, DesktopSize, u32)> = None;

    loop {
        // Save finalization state before the step that might fail.
        if let ClientConnectorState::ConnectionFinalization {
            ref connection_activation,
            ..
        } = connector.state
        {
            if let ConnectionActivationState::ConnectionFinalization {
                io_channel_id,
                user_channel_id,
                desktop_size,
                share_id,
                ..
            } = connection_activation.connection_activation_state()
            {
                saved = Some((io_channel_id, user_channel_id, desktop_size, share_id));
            }
        }

        match single_sequence_step(framed, &mut connector, &mut buf) {
            Ok(()) => {}
            Err(e) => {
                // Detect VirtualBox's empty FontMap: ironrdp reports "FontPdu … 0 bytes".
                let is_empty_fontmap = {
                    let mut found = false;
                    let mut cur: Option<&dyn StdError> = Some(&e);
                    while let Some(next) = cur {
                        if next.to_string().contains("FontPdu") {
                            found = true;
                            break;
                        }
                        cur = next.source();
                    }
                    found
                };

                if is_empty_fontmap {
                    if let Some((io_channel_id, user_channel_id, desktop_size, share_id)) = saved {
                        return Ok(ConnectionResult {
                            io_channel_id,
                            user_channel_id,
                            share_id,
                            static_channels: std::mem::take(&mut connector.static_channels),
                            desktop_size,
                            enable_server_pointer: connector.config.enable_server_pointer,
                            pointer_software_rendering: connector.config.pointer_software_rendering,
                            connection_activation: ConnectionActivationSequence::new(
                                connector.config.clone(),
                                io_channel_id,
                                user_channel_id,
                            ),
                            compression_type: connector.config.compression_type,
                        });
                    }
                }
                return Err(e);
            }
        }

        if matches!(connector.state, ClientConnectorState::Connected { .. }) {
            break;
        }
    }

    match connector.state {
        ClientConnectorState::Connected { result, .. } => Ok(result),
        _ => unreachable!("vbox_connect_finalize: connector not Connected after loop"),
    }
}

// ── Поток сессии ─────────────────────────────────────────────────────────────

fn vrde_thread(
    host: String,
    port: u16,
    desktop_size: (u16, u16),
    settings: VrdeSettings,
    cmd_rx: mpsc::Receiver<VrdeCmd>,
    frame_tx: mpsc::SyncSender<(u32, u32, Vec<u8>)>,
    status_tx: mpsc::SyncSender<String>,
) {
    let mut log_file = open_vrde_log();
    vrde_log(
        &mut log_file,
        format_args!(
            "--- VRDE session start host={host} port={port} color_depth={} compression={:?} ---",
            settings.color_depth, settings.compression
        ),
    );

    macro_rules! status {
        ($($t:tt)*) => {{
            let msg = format!($($t)*);
            vrde_log(&mut log_file, format_args!("{}", msg));
            let _ = status_tx.try_send(msg);
        }};
    }
    macro_rules! diag {
        ($($t:tt)*) => {{
            vrde_log(&mut log_file, format_args!($($t)*));
        }};
    }

    status!("VRDE: подключение к {}:{}…", host, port);
    if let Some(path) = vrde_log_path() {
        status!("VRDE: лог {}", path.display());
    }

    // ── Build connector config ────────────────────────────────────────────────
    let (desktop_width, desktop_height) = sanitize_desktop_size(desktop_size.0, desktop_size.1);

    let config = Config {
        desktop_size: DesktopSize {
            width: desktop_width,
            height: desktop_height,
        },
        desktop_scale_factor: 0,
        // VirtualBox VRDE требует TLS; plain PROTOCOL_RDP отклоняется.
        enable_tls: true,
        enable_credssp: false,
        credentials: Credentials::UsernamePassword {
            username: String::new(),
            password: String::new(),
        },
        domain: None,
        client_build: 0x0A28_0000,
        client_name: "EvertyDesk".to_owned(),
        keyboard_type: KeyboardType::IbmPcAt,
        keyboard_subtype: 0,
        keyboard_functional_keys_count: 12,
        keyboard_layout: 0x0409,
        ime_file_name: String::new(),
        bitmap: Some(BitmapConfig {
            lossy_compression: false,
            // User-adjustable via the settings gear on the VM console page
            // (VirtualBox's own docs recommend 16bpp for RDP viewers).
            color_depth: settings.color_depth,
            codecs: Default::default(),
        }),
        dig_product_id: String::new(),
        client_dir: String::new(),
        alternate_shell: String::new(),
        work_dir: String::new(),
        platform: MajorPlatformType::UNSPECIFIED,
        hardware_id: None,
        request_data: None,
        autologon: true,
        enable_audio_playback: false,
        performance_flags: Default::default(),
        license_cache: None,
        timezone_info: Default::default(),
        // VirtualBox VRDE sends bulk-compressed Fast-Path graphics updates
        // regardless of what we negotiate. With compression_type: None,
        // ironrdp_session never builds a BulkCompressor (active_stage.rs:
        // `bulk_decompressor = connection_result.compression_type.and_then(...)`),
        // so every compressed update silently falls through as raw bytes fed
        // to the wrong decoder — no error, no pixels: the exact "black screen
        // with gfx counter climbing" symptom we hit before fixing the ring
        // buffer in vendor/ironrdp-bulk. K8 vs K64 is now just a history-window
        // size trade-off (both work correctly), exposed via the settings gear.
        // Rdp61 (XCRUSH) was tried and desynced on the very next packet after
        // the first one — not offered as an option.
        compression_type: Some(settings.compression.to_ironrdp()),
        enable_server_pointer: true,
        pointer_software_rendering: false,
        multitransport_flags: None,
    };

    // ── TCP connect ───────────────────────────────────────────────────────────
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .unwrap_or_else(|_| format!("127.0.0.1:{port}").parse().unwrap());

    let tcp = match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
        Ok(s) => s,
        Err(e) => {
            status!("VRDE: TCP ошибка: {e}");
            return;
        }
    };
    let _ = tcp.set_nodelay(true);

    let client_addr: SocketAddr = tcp
        .local_addr()
        .unwrap_or_else(|_| "127.0.0.1:0".parse().unwrap());

    let mut connector = ClientConnector::new(config, client_addr);
    let mut framed = Framed::new(tcp);

    // ── Phase 1: X.224 negotiation (plain TCP) ────────────────────────────────
    status!("VRDE: X.224 переговоры…");
    let should_upgrade = match connect_begin(&mut framed, &mut connector) {
        Ok(u) => u,
        Err(e) => {
            status!("VRDE: ошибка X.224: {e}");
            return;
        }
    };
    // connector.should_perform_security_upgrade() == true здесь (PROTOCOL_SSL выбран)

    // ── Phase 2: TLS handshake ────────────────────────────────────────────────
    status!("VRDE: TLS рукопожатие…");
    // Leftover-байты из TCP X.224 фазы отбрасываем — они относятся к plain-TCP
    // обмену и не должны попасть в TLS поток как новые PDU.
    let (tcp_raw, _leftover) = framed.into_inner();

    let tls_connector = match TlsConnector::builder()
        // VirtualBox VRDE использует самоподписанный сертификат
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            status!("VRDE: TLS build ошибка: {e}");
            return;
        }
    };

    let tls_stream = match tls_connector.connect(&host, tcp_raw) {
        Ok(s) => s,
        Err(e) => {
            status!("VRDE: TLS ошибка: {e}");
            return;
        }
    };

    // Spy wrapper для диагностики — логирует первые 4096 байтов от сервера
    let spy_log: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let spy = SpyStream {
        inner: tls_stream,
        log: Arc::clone(&spy_log),
        limit: 4096,
    };
    // Начинаем с пустым буфером — TLS поток начинается заново
    let mut tls_framed = Framed::new(spy);

    // ── Phase 3: finalize RDP connection over TLS ─────────────────────────────
    status!("VRDE: RDP инициализация через TLS…");
    let upgraded = mark_as_upgraded(should_upgrade, &mut connector);

    let connection_result = match vbox_connect_finalize(upgraded, connector, &mut tls_framed) {
        Ok(r) => r,
        Err(e) => {
            // Build full error chain: e → e.source() → ...
            use std::error::Error as StdError;
            let mut chain = format!("{e}");
            let mut src: Option<&dyn StdError> = e.source();
            while let Some(next) = src {
                chain.push_str(&format!(": {next}"));
                src = next.source();
            }

            let log = spy_log.lock().unwrap();
            let hex_info = if log.is_empty() {
                " [нет байтов от VRDE до ошибки]".to_owned()
            } else {
                let hex: String = log
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!(" [{}б: {}]", log.len(), hex)
            };
            drop(log);
            status!("VRDE ошибка: {chain}{hex_info}");
            return;
        }
    };

    let width = connection_result.desktop_size.width;
    let height = connection_result.desktop_size.height;
    status!("VRDE: подключено {}×{}", width, height);

    // ── Active session ────────────────────────────────────────────────────────
    let mut active = ActiveStage::new(connection_result);
    let mut image = DecodedImage::new(PixelFormat::RgbA32, width, height);

    // 8 ms read timeout keeps the input-command channel responsive.
    // Framed::read_pdu buffers partial reads internally, so a mid-frame
    // timeout just causes a continue on the outer loop and retry.
    //
    // Write timeout too: every `write_all`/`flush` call in this module
    // (input events, RDP response frames, resize requests) had no timeout at
    // all — a single stalled write (full TCP send buffer, half-dead
    // connection not yet noticed by the OS) blocks this thread forever, with
    // no detector able to react since the thread isn't running any code to
    // check them. That's a real, previously-unguarded hang path distinct
    // from everything fixed so far (MPPC ring buffer, malformed-PDU
    // tolerance, bounded reactivation reads).
    {
        let (spy, _) = tls_framed.get_inner();
        let _ = spy
            .inner
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(8)));
        let _ = spy
            .inner
            .get_ref()
            .set_write_timeout(Some(Duration::from_secs(5)));
    }

    let mut cur_mouse_x: u16 = 0;
    let mut cur_mouse_y: u16 = 0;
    // Current cursor shape received via PointerBitmap PDU; None = no cursor yet.
    let mut cursor_shape: Option<Arc<DecodedPointer>> = None;
    // True whenever the display needs a new frame (bitmap update OR cursor move).
    let mut had_update = false;
    let mut last_diag_at = Instant::now();
    let mut cmd_count = 0u64;
    let mut move_count = 0u64;
    let mut button_count = 0u64;
    let mut key_count = 0u64;
    let mut text_count = 0u64;
    let mut resize_count = 0u64;
    let mut write_ok = 0u64;
    let mut write_err = 0u64;
    let mut transient_reads = 0u64;
    let mut read_errors = 0u64;
    let mut pdu_errors = 0u64;
    let mut pdu_ignored = 0u64;
    let mut frames_sent = 0u64;
    let mut frame_drops = 0u64;
    // Distinguishes real bitmap decode events from cursor-only repaints — a
    // black canvas with frames_sent climbing but graphics_updates stuck at 0
    // means the RDP bitmap stream never actually painted anything.
    let mut graphics_updates = 0u64;
    let mut first_paint_logged = false;
    let mut send_diag_logged = false;
    let mut desync_signaled = false;
    let mut content_baseline_seen = false;
    macro_rules! emit_diag_if_due {
        ($image:expr) => {
            if last_diag_at.elapsed() >= Duration::from_secs(1) {
                let data = $image.data();
                let nonzero_count = data.iter().filter(|&&b| b != 0).count();
                let pct = 100.0 * nonzero_count as f64 / data.len().max(1) as f64;
                let iw = $image.width() as usize;
                let ih = $image.height() as usize;
                let pixel_at = |px: usize, py: usize| -> String {
                    let off = (py * iw + px) * 4;
                    if off + 4 <= data.len() {
                        format!("{:02x}{:02x}{:02x}", data[off], data[off + 1], data[off + 2])
                    } else {
                        "??".to_owned()
                    }
                };
                status!(
                    "VRDE diag: cmd={} move={} btn={} key={} text={} resize={} wr_ok={} wr_err={} tr={} rerr={} pdu={} ign={} frames={} fdrops={} gfx={} nonzero={:.1}% tl={} center={} br={}",
                    cmd_count,
                    move_count,
                    button_count,
                    key_count,
                    text_count,
                    resize_count,
                    write_ok,
                    write_err,
                    transient_reads,
                    read_errors,
                    pdu_errors,
                    pdu_ignored,
                    frames_sent,
                    frame_drops,
                    graphics_updates,
                    pct,
                    pixel_at(0, 0),
                    pixel_at(iw / 2, ih / 2),
                    pixel_at(iw.saturating_sub(1), ih.saturating_sub(1)),
                );
                last_diag_at = Instant::now();
            }
        };
    }

    loop {
        // ── Drain incoming commands ───────────────────────────────────────────
        loop {
            match cmd_rx.try_recv() {
                Ok(VrdeCmd::Stop) => {
                    status!("VRDE: сессия закрыта");
                    return;
                }
                Ok(VrdeCmd::MouseMove { x, y }) => {
                    cmd_count += 1;
                    if x == cur_mouse_x && y == cur_mouse_y {
                        continue;
                    }
                    move_count += 1;
                    cur_mouse_x = x;
                    cur_mouse_y = y;
                    // Slow-path input (X.224/MCS), not Fast-Path: VirtualBox VRDE's
                    // FastPath support appears output-only — client-to-server
                    // FastPath input PDUs were accepted (no write error) but never
                    // affected the guest at all. Slow-path Input Event PDUs are the
                    // original, universally-implemented mechanism.
                    match emit_mouse_event(
                        &active,
                        &mut tls_framed,
                        MousePdu {
                            flags: PointerFlags::MOVE,
                            number_of_wheel_rotation_units: 0,
                            x_position: x,
                            y_position: y,
                        },
                    ) {
                        Ok(()) => write_ok += 1,
                        Err(e) => {
                            write_err += 1;
                            diag!("VRDE input write error mouse_move: {e}");
                        }
                    }
                    // If cursor shape is known, emit a frame so the cursor appears
                    // at the new position immediately (no bitmap PDU needed).
                    if cursor_shape.is_some() {
                        had_update = true;
                    }
                }
                Ok(VrdeCmd::MouseButton { button, down }) => {
                    cmd_count += 1;
                    button_count += 1;
                    let btn_flag = match button {
                        0 => PointerFlags::LEFT_BUTTON,
                        1 => PointerFlags::RIGHT_BUTTON,
                        _ => PointerFlags::MIDDLE_BUTTON_OR_WHEEL,
                    };
                    let flags = if down {
                        btn_flag | PointerFlags::DOWN
                    } else {
                        btn_flag
                    };
                    match emit_mouse_event(
                        &active,
                        &mut tls_framed,
                        MousePdu {
                            flags,
                            number_of_wheel_rotation_units: 0,
                            x_position: cur_mouse_x,
                            y_position: cur_mouse_y,
                        },
                    ) {
                        Ok(()) => write_ok += 1,
                        Err(e) => {
                            write_err += 1;
                            diag!("VRDE input write error mouse_button button={button} down={down}: {e}");
                        }
                    }
                }
                Ok(VrdeCmd::MouseWheel { delta }) => {
                    cmd_count += 1;
                    button_count += 1;
                    match emit_mouse_event(
                        &active,
                        &mut tls_framed,
                        MousePdu {
                            flags: PointerFlags::VERTICAL_WHEEL,
                            number_of_wheel_rotation_units: delta,
                            x_position: cur_mouse_x,
                            y_position: cur_mouse_y,
                        },
                    ) {
                        Ok(()) => write_ok += 1,
                        Err(e) => {
                            write_err += 1;
                            diag!("VRDE input write error mouse_wheel delta={delta}: {e}");
                        }
                    }
                }
                Ok(VrdeCmd::KeyDown { scancode, extended }) => {
                    cmd_count += 1;
                    key_count += 1;
                    match emit_key_event(&active, &mut tls_framed, scancode, extended, false) {
                        Ok(()) => write_ok += 1,
                        Err(e) => {
                            write_err += 1;
                            diag!("VRDE input write error key_down sc={scancode:#x} ext={extended}: {e}");
                        }
                    }
                }
                Ok(VrdeCmd::KeyUp { scancode, extended }) => {
                    cmd_count += 1;
                    key_count += 1;
                    match emit_key_event(&active, &mut tls_framed, scancode, extended, true) {
                        Ok(()) => write_ok += 1,
                        Err(e) => {
                            write_err += 1;
                            diag!("VRDE input write error key_up sc={scancode:#x} ext={extended}: {e}");
                        }
                    }
                }
                Ok(VrdeCmd::Text(text)) => {
                    cmd_count += 1;
                    text_count += text.chars().count() as u64;
                    for ch in text.chars() {
                        let mut units = [0u16; 2];
                        for unit in ch.encode_utf16(&mut units).iter().copied() {
                            match emit_unicode_event(&active, &mut tls_framed, unit, false) {
                                Ok(()) => write_ok += 1,
                                Err(e) => {
                                    write_err += 1;
                                    diag!("VRDE input write error text_down unit={unit:#x}: {e}");
                                }
                            }
                            match emit_unicode_event(&active, &mut tls_framed, unit, true) {
                                Ok(()) => {
                                    write_ok += 1;
                                }
                                Err(e) => {
                                    write_err += 1;
                                    diag!("VRDE input write error text_up unit={unit:#x}: {e}");
                                }
                            }
                        }
                    }
                }
                Ok(VrdeCmd::Resize { width, height }) => {
                    cmd_count += 1;
                    resize_count += 1;
                    let (width, height) = sanitize_desktop_size(width, height);
                    if width == image.width() && height == image.height() {
                        continue;
                    }
                    match active.encode_resize(width as u32, height as u32, Some(100), None) {
                        Some(Ok(bytes)) => {
                            if tls_framed.write_all(&bytes).is_ok() {
                                let (stream, _) = tls_framed.get_inner_mut();
                                let _ = stream.flush();
                                write_ok += 1;
                                status!("VRDE: resize requested {}x{}", width, height);
                            } else {
                                write_err += 1;
                                diag!("VRDE resize write error {}x{}", width, height);
                            }
                        }
                        Some(Err(e)) => {
                            write_err += 1;
                            status!("VRDE: resize rejected: {e}");
                        }
                        None => {}
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        // ── Read one complete PDU via framed reader ───────────────────────────
        // read_pdu() uses ironrdp_pdu::find_size to detect FastPath vs X224
        // and accumulates bytes until exactly one PDU is available.
        let (action, frame) = match tls_framed.read_pdu() {
            Ok(f) => f,
            Err(ref e) if is_transient_read_error(e) => {
                transient_reads += 1;
                // Flush a cursor-only frame when the mouse moved but no bitmap PDU arrived.
                if had_update {
                    if send_frame(
                        &image,
                        &cursor_shape,
                        cur_mouse_x,
                        cur_mouse_y,
                        &frame_tx,
                        &mut log_file,
                        &mut send_diag_logged,
                    ) {
                        frames_sent += 1;
                    } else {
                        frame_drops += 1;
                    }
                    had_update = false;
                }
                thread::sleep(Duration::from_millis(1));
                emit_diag_if_due!(image);
                continue;
            }
            Err(e) => {
                read_errors += 1;
                diag!("VRDE read error fatal rerr={read_errors}: {e}");
                status!("VRDE: ошибка чтения: {e}");
                return;
            }
        };

        let outputs = match active.process(&mut image, action, &frame) {
            Ok(out) => out,
            Err(e) => {
                if is_ignorable_pdu_error(&e) {
                    pdu_ignored += 1;
                    // VirtualBox occasionally sends a single malformed/truncated
                    // PDU (confirmed independently against the official
                    // ironrdp-viewer reference client — not specific to our
                    // code). Unlike the old MPPC ring-buffer overflow (now fixed
                    // at the source — see the ironrdp-bulk patch), a single bad
                    // PDU doesn't corrupt any persistent decoder state, so it's
                    // safe to just skip it and keep going. No reconnect signal
                    // here; the content-collapse detector below is what catches
                    // genuine corruption (based on actual pixel evidence, not
                    // error-text pattern matching).
                    if pdu_ignored <= 5 || pdu_ignored % 50 == 0 {
                        use std::error::Error as StdError;
                        let mut chain = format!("{e}");
                        let mut src: Option<&dyn StdError> = StdError::source(&e);
                        while let Some(next) = src {
                            chain.push_str(&format!(" <- {next}"));
                            src = next.source();
                        }
                        diag!("VRDE: ignored malformed PDU #{pdu_ignored}: {chain}");
                    }
                    emit_diag_if_due!(image);
                    continue;
                }
                pdu_errors += 1;
                diag!("VRDE PDU error: {e}");
                status!("VRDE: ошибка PDU: {e}");
                continue;
            }
        };

        for output in outputs {
            match output {
                ActiveStageOutput::ResponseFrame(bytes) => match tls_framed.write_all(&bytes) {
                    Ok(()) => write_ok += 1,
                    Err(e) => {
                        write_err += 1;
                        diag!("VRDE response write error: {e}");
                    }
                },
                ActiveStageOutput::GraphicsUpdate(_) => {
                    had_update = true;
                    graphics_updates += 1;
                    if !first_paint_logged {
                        first_paint_logged = true;
                        let data = image.data();
                        let nonzero = data.iter().any(|&b| b != 0);
                        diag!(
                            "VRDE: первое GraphicsUpdate, image {}x{}, nonzero_bytes={}",
                            image.width(),
                            image.height(),
                            nonzero
                        );
                    }
                }
                // Cursor shape received from VirtualBox: store it and redraw.
                ActiveStageOutput::PointerBitmap(ptr) => {
                    cursor_shape = Some(ptr);
                    had_update = true;
                }
                // Cursor hidden / reset to default arrow.
                ActiveStageOutput::PointerHidden | ActiveStageOutput::PointerDefault => {
                    cursor_shape = None;
                    had_update = true;
                }
                // Server-side cursor position (e.g. animated cursor): just redraw.
                ActiveStageOutput::PointerPosition { .. } => {
                    had_update = true;
                }
                ActiveStageOutput::Terminate(reason) => {
                    status!("VRDE: отключение: {reason}");
                    return;
                }
                ActiveStageOutput::DeactivateAll(mut cas) => {
                    // VirtualBox sends ServerDeactivateAll after every new client
                    // connection. The returned ConnectionActivationSequence (already
                    // reset to CapabilitiesExchange) must be driven to completion
                    // before the server will send any graphics.
                    status!("VRDE: пересогласование параметров…");
                    {
                        let (spy, _) = tls_framed.get_inner();
                        // NOT `None` (infinite block): if VirtualBox sends a
                        // malformed control PDU whose declared header size
                        // exceeds what it actually transmits (a confirmed,
                        // separate VirtualBox quirk — see the malformed-PDU
                        // tolerance in vbox_reactivate below), `read_exact`
                        // loops forever waiting for bytes that will never
                        // arrive, freezing this entire worker thread — no
                        // timer or detector in main.rs can react, because the
                        // thread isn't running any code to check them. That
                        // was almost certainly the real freeze (not just the
                        // visible flash around it): a bounded timeout here
                        // turns "hang forever" into "fail after a few seconds
                        // and let the normal reconnect path recover", which is
                        // strictly better even though reactivation normally
                        // completes in well under 100ms.
                        let _ = spy
                            .inner
                            .get_ref()
                            .set_read_timeout(Some(Duration::from_millis(100)));
                    }
                    match vbox_reactivate(&mut *cas, &mut tls_framed) {
                        Ok(new_share_id) => {
                            active.set_share_id(new_share_id);
                            status!("VRDE: реактивация OK (share_id={})", new_share_id);
                        }
                        Err(msg) => {
                            status!("VRDE: реактивация: {msg}");
                            return;
                        }
                    }
                    {
                        let (spy, _) = tls_framed.get_inner();
                        let _ = spy
                            .inner
                            .get_ref()
                            .set_read_timeout(Some(Duration::from_millis(8)));
                    }
                }
                ActiveStageOutput::MultitransportRequest(_) | ActiveStageOutput::AutoDetect(_) => {}
            }
        }

        if had_update {
            // Content-based desync detector: some decoder corruption never
            // surfaces as a PDU error (active.process() returns Ok with a
            // GraphicsUpdate, just over garbage/zeroed history-buffer data) —
            // the precise VRDE_DESYNC signal above only catches the cases that
            // DO error. Sample a sparse grid of pixels; once the image has shown
            // real content, a sudden collapse to near-all-black is corruption,
            // not legitimate guest content (a real desktop practically never
            // goes from "had visible content" to "99%+ black" in one update).
            let data = image.data();
            let pixel_count = data.len() / 4;
            if pixel_count > 0 {
                const SAMPLES: usize = 512;
                let step = (pixel_count / SAMPLES).max(1);
                let mut nonzero = 0usize;
                let mut checked = 0usize;
                let mut px = 0usize;
                while px < pixel_count {
                    let off = px * 4;
                    if data[off] != 0 || data[off + 1] != 0 || data[off + 2] != 0 {
                        nonzero += 1;
                    }
                    checked += 1;
                    px += step;
                }
                let sample_pct = nonzero as f64 / checked.max(1) as f64;
                if sample_pct > 0.10 {
                    content_baseline_seen = true;
                } else if content_baseline_seen && !desync_signaled {
                    desync_signaled = true;
                    diag!(
                        "VRDE: content collapsed to near-black (sample={:.1}%) — silent decoder corruption",
                        sample_pct * 100.0
                    );
                    let _ = status_tx.try_send("VRDE_DESYNC".to_owned());
                }
            }

            if send_frame(
                &image,
                &cursor_shape,
                cur_mouse_x,
                cur_mouse_y,
                &frame_tx,
                &mut log_file,
                &mut send_diag_logged,
            ) {
                frames_sent += 1;
            } else {
                frame_drops += 1;
            }
            had_update = false;
        }

        emit_diag_if_due!(image);
    }
}

fn vrde_log_path() -> Option<PathBuf> {
    Some(std::env::temp_dir().join("evertydesk-vrde.log"))
}

fn open_vrde_log() -> Option<File> {
    let path = vrde_log_path()?;
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// Append a timestamped line to the same VRDE log file the session thread
/// writes to, callable from main.rs's own reconnect-decision code (which
/// runs on the UI thread, not inside `vrde_thread`) — needed to get a single
/// unified timeline of "why did this reconnect happen" across both sides.
pub fn log_from_ui(msg: &str) {
    let mut file = open_vrde_log();
    vrde_log(&mut file, format_args!("{msg}"));
}

/// Installs a panic hook (once, process-wide) that writes the full panic
/// message *and* source location to the VRDE log before chaining to
/// whatever hook was previously installed (so default stderr output, if a
/// console is attached, still happens too). `catch_unwind`'s payload alone
/// only carries the formatted message — `std::panic::Location` is only
/// available from inside a hook — so this is what actually gives a
/// `file:line` to go with "the session thread panicked".
fn install_panic_logging_hook() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "unknown location".to_owned());
            log_from_ui(&format!("VRDE: !!! PANIC at {location}: {info}"));
            previous(info);
        }));
    });
}

fn vrde_log(file: &mut Option<File>, args: fmt::Arguments<'_>) {
    let Some(file) = file.as_mut() else {
        return;
    };
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let _ = writeln!(file, "[{ts_ms}] {args}");
    let _ = file.flush();
}

pub(crate) fn is_transient_read_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) || (err.kind() == std::io::ErrorKind::UnexpectedEof
        && err.to_string().contains("not enough bytes"))
}

pub(crate) fn sanitize_desktop_size(width: u16, height: u16) -> (u16, u16) {
    let mut width = width.clamp(200, 8192);
    if width % 2 != 0 {
        width = width.saturating_sub(1).max(200);
    }
    let height = height.clamp(200, 8192);
    (width, height)
}

pub(crate) fn is_ignorable_pdu_error(err: &ironrdp_session::SessionError) -> bool {
    err.to_string().contains("custom error")
}

pub(crate) fn send_fastpath_input<S: Read + Write>(
    active: &mut ActiveStage,
    image: &mut DecodedImage,
    framed: &mut Framed<S>,
    events: &[FastPathInputEvent],
) -> Result<bool, String> {
    if events.is_empty() {
        return Ok(false);
    }

    let outputs = active
        .process_fastpath_input(image, events)
        .map_err(|e| format!("fastpath input: {e}"))?;

    let mut visual_update = false;
    for output in outputs {
        match output {
            ActiveStageOutput::ResponseFrame(bytes) => {
                framed
                    .write_all(&bytes)
                    .map_err(|e| format!("write fastpath input: {e}"))?;
                let (stream, _) = framed.get_inner_mut();
                stream
                    .flush()
                    .map_err(|e| format!("flush fastpath input: {e}"))?;
            }
            ActiveStageOutput::GraphicsUpdate(_) => {
                visual_update = true;
            }
            _ => {}
        }
    }

    Ok(visual_update)
}

fn emit_fastpath_key_event<S: Read + Write>(
    active: &mut ActiveStage,
    image: &mut DecodedImage,
    framed: &mut Framed<S>,
    scancode: u8,
    extended: bool,
    release: bool,
) -> Result<bool, String> {
    let mut flags = if release {
        FastKeyboardFlags::RELEASE
    } else {
        FastKeyboardFlags::empty()
    };
    if extended {
        flags |= FastKeyboardFlags::EXTENDED;
    }

    send_fastpath_input(
        active,
        image,
        framed,
        &[FastPathInputEvent::KeyboardEvent(flags, scancode)],
    )
}

fn emit_fastpath_unicode_event<S: Read + Write>(
    active: &mut ActiveStage,
    image: &mut DecodedImage,
    framed: &mut Framed<S>,
    unit: u16,
    release: bool,
) -> Result<bool, String> {
    let flags = if release {
        FastKeyboardFlags::RELEASE
    } else {
        FastKeyboardFlags::empty()
    };

    send_fastpath_input(
        active,
        image,
        framed,
        &[FastPathInputEvent::UnicodeKeyboardEvent(flags, unit)],
    )
}

fn send_slow_input<S: Read + Write>(
    active: &ActiveStage,
    framed: &mut Framed<S>,
    events: Vec<InputEvent>,
) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }

    let mut frame = WriteBuf::new();
    active
        .encode_static(&mut frame, ShareDataPdu::Input(InputEventPdu(events)))
        .map_err(|e| format!("encode input: {e}"))?;

    framed
        .write_all(frame.filled())
        .map_err(|e| format!("write input: {e}"))?;
    let (stream, _) = framed.get_inner_mut();
    stream.flush().map_err(|e| format!("flush input: {e}"))?;
    Ok(())
}

pub(crate) fn emit_mouse_event<S: Read + Write>(
    active: &ActiveStage,
    framed: &mut Framed<S>,
    pdu: MousePdu,
) -> Result<(), String> {
    send_slow_input(active, framed, vec![InputEvent::Mouse(pdu)])
}

pub(crate) fn emit_key_event<S: Read + Write>(
    active: &ActiveStage,
    framed: &mut Framed<S>,
    scancode: u8,
    extended: bool,
    release: bool,
) -> Result<(), String> {
    let mut flags = if release {
        SlowKeyboardFlags::RELEASE
    } else {
        SlowKeyboardFlags::DOWN
    };
    if extended {
        flags |= SlowKeyboardFlags::EXTENDED;
    }

    send_slow_input(
        active,
        framed,
        vec![InputEvent::ScanCode(ScanCodePdu {
            flags,
            key_code: u16::from(scancode),
        })],
    )
}

pub(crate) fn emit_unicode_event<S: Read + Write>(
    active: &ActiveStage,
    framed: &mut Framed<S>,
    unit: u16,
    release: bool,
) -> Result<(), String> {
    let flags = if release {
        SlowUnicodeFlags::RELEASE
    } else {
        SlowUnicodeFlags::empty()
    };
    send_slow_input(
        active,
        framed,
        vec![InputEvent::Unicode(UnicodePdu {
            flags,
            unicode_code: unit,
        })],
    )
}

/// ASCII char → (scancode, shift, extended). VirtualBox VRDE does not appear to
/// implement Unicode keyboard input PDUs at all (same FastPath-input-only
/// limitation as mouse) — every printable ASCII key must go over the scancode
/// path that arrow keys/shortcuts already use successfully.
/// Lowercase Cyrillic letter → physical QWERTY-position scancode, per the
/// standard Russian ЙЦУКЕН keyboard layout (e.g. 'й' sits where 'q' is on a
/// US keyboard). VirtualBox doesn't implement RDP Unicode keyboard input
/// PDUs at all (confirmed: ASCII via Unicode events silently did nothing,
/// scancode-based input worked) — sending the scancode for the position
/// only produces the expected Cyrillic letter when the GUEST's active input
/// locale is actually Russian; this can't query or change that from the
/// client side, it's relying on the same assumption every scancode-only
/// remote-input tool makes.
fn cyrillic_lower_to_scancode(ch: char) -> Option<u8> {
    Some(match ch {
        'й' => 0x10,
        'ц' => 0x11,
        'у' => 0x12,
        'к' => 0x13,
        'е' => 0x14,
        'н' => 0x15,
        'г' => 0x16,
        'ш' => 0x17,
        'щ' => 0x18,
        'з' => 0x19,
        'х' => 0x1A,
        'ъ' => 0x1B,
        'ф' => 0x1E,
        'ы' => 0x1F,
        'в' => 0x20,
        'а' => 0x21,
        'п' => 0x22,
        'р' => 0x23,
        'о' => 0x24,
        'л' => 0x25,
        'д' => 0x26,
        'ж' => 0x27,
        'э' => 0x28,
        'ё' => 0x29,
        'я' => 0x2C,
        'ч' => 0x2D,
        'с' => 0x2E,
        'м' => 0x2F,
        'и' => 0x30,
        'т' => 0x31,
        'ь' => 0x32,
        'б' => 0x33,
        'ю' => 0x34,
        _ => return None,
    })
}

pub fn char_to_rdp_scancode(ch: char) -> Option<(u8, bool, bool)> {
    if ch.is_alphabetic() && !ch.is_ascii() {
        // `to_lowercase()` is Unicode-aware (needed for Cyrillic, unlike
        // `to_ascii_lowercase()` which only handles ASCII and would leave
        // these chars untouched).
        let shifted = ch != ch.to_lowercase().next().unwrap_or(ch);
        let lower = ch.to_lowercase().next().unwrap_or(ch);
        let scancode = cyrillic_lower_to_scancode(lower)?;
        return Some((scancode, shifted, false));
    }
    let shifted = ch.is_ascii_uppercase();
    let lower = ch.to_ascii_lowercase();
    let (scancode, shift) = match lower {
        'a' => (0x1E, shifted),
        'b' => (0x30, shifted),
        'c' => (0x2E, shifted),
        'd' => (0x20, shifted),
        'e' => (0x12, shifted),
        'f' => (0x21, shifted),
        'g' => (0x22, shifted),
        'h' => (0x23, shifted),
        'i' => (0x17, shifted),
        'j' => (0x24, shifted),
        'k' => (0x25, shifted),
        'l' => (0x26, shifted),
        'm' => (0x32, shifted),
        'n' => (0x31, shifted),
        'o' => (0x18, shifted),
        'p' => (0x19, shifted),
        'q' => (0x10, shifted),
        'r' => (0x13, shifted),
        's' => (0x1F, shifted),
        't' => (0x14, shifted),
        'u' => (0x16, shifted),
        'v' => (0x2F, shifted),
        'w' => (0x11, shifted),
        'x' => (0x2D, shifted),
        'y' => (0x15, shifted),
        'z' => (0x2C, shifted),
        '1' => (0x02, false),
        '2' => (0x03, false),
        '3' => (0x04, false),
        '4' => (0x05, false),
        '5' => (0x06, false),
        '6' => (0x07, false),
        '7' => (0x08, false),
        '8' => (0x09, false),
        '9' => (0x0A, false),
        '0' => (0x0B, false),
        '!' => (0x02, true),
        '@' => (0x03, true),
        '#' => (0x04, true),
        '$' => (0x05, true),
        '%' => (0x06, true),
        '^' => (0x07, true),
        '&' => (0x08, true),
        '*' => (0x09, true),
        '(' => (0x0A, true),
        ')' => (0x0B, true),
        '-' => (0x0C, false),
        '_' => (0x0C, true),
        '=' => (0x0D, false),
        '+' => (0x0D, true),
        '[' => (0x1A, false),
        '{' => (0x1A, true),
        ']' => (0x1B, false),
        '}' => (0x1B, true),
        '\\' => (0x2B, false),
        '|' => (0x2B, true),
        ';' => (0x27, false),
        ':' => (0x27, true),
        '\'' => (0x28, false),
        '"' => (0x28, true),
        '`' => (0x29, false),
        '~' => (0x29, true),
        ',' => (0x33, false),
        '<' => (0x33, true),
        '.' => (0x34, false),
        '>' => (0x34, true),
        '/' => (0x35, false),
        '?' => (0x35, true),
        ' ' => (0x39, false),
        '\t' => (0x0F, false),
        '\n' | '\r' => (0x1C, false),
        _ => return None,
    };
    Some((scancode, shift, false))
}

// Composite cursor shape onto `rgba` at (mouse_x, mouse_y) using the cursor's
// hotspot to align the tip. VirtualBox cursor bitmaps use non-premultiplied RGBA
// (PointerBitmapTarget::Accelerated).
pub(crate) fn composite_cursor(
    rgba: &mut [u8],
    img_w: usize,
    img_h: usize,
    cursor: &DecodedPointer,
    mouse_x: u16,
    mouse_y: u16,
) {
    let cw = cursor.width as i32;
    let ch = cursor.height as i32;
    if cw == 0 || ch == 0 || cursor.bitmap_data.is_empty() {
        return;
    }
    let ox = mouse_x as i32 - cursor.hotspot_x as i32;
    let oy = mouse_y as i32 - cursor.hotspot_y as i32;
    for py in 0..ch {
        for px in 0..cw {
            let dx = ox + px;
            let dy = oy + py;
            if dx < 0 || dy < 0 || dx >= img_w as i32 || dy >= img_h as i32 {
                continue;
            }
            let si = ((py * cw + px) * 4) as usize;
            if si + 3 >= cursor.bitmap_data.len() {
                break;
            }
            let ca = cursor.bitmap_data[si + 3];
            if ca == 0 {
                continue;
            }
            let di = (dy as usize * img_w + dx as usize) * 4;
            if di + 2 >= rgba.len() {
                break;
            }
            let (cr, cg, cb) = (
                cursor.bitmap_data[si],
                cursor.bitmap_data[si + 1],
                cursor.bitmap_data[si + 2],
            );
            if ca == 255 {
                rgba[di] = cr;
                rgba[di + 1] = cg;
                rgba[di + 2] = cb;
                rgba[di + 3] = 255;
            } else {
                let a = ca as u16;
                let ia = 255 - a;
                rgba[di] = ((cr as u16 * a + rgba[di] as u16 * ia) / 255) as u8;
                rgba[di + 1] = ((cg as u16 * a + rgba[di + 1] as u16 * ia) / 255) as u8;
                rgba[di + 2] = ((cb as u16 * a + rgba[di + 2] as u16 * ia) / 255) as u8;
                rgba[di + 3] = 255;
            }
        }
    }
}

// Build and send one frame: clone the decoded image, composite the cursor on top,
// then try_send to the UI channel.
fn send_frame(
    image: &DecodedImage,
    cursor_shape: &Option<Arc<DecodedPointer>>,
    mouse_x: u16,
    mouse_y: u16,
    frame_tx: &mpsc::SyncSender<(u32, u32, Vec<u8>)>,
    log_file: &mut Option<File>,
    diag_logged: &mut bool,
) -> bool {
    let w = image.width() as usize;
    let h = image.height() as usize;
    let mut rgba = image.data().to_vec();
    if !*diag_logged {
        *diag_logged = true;
        let expected_len = w * h * 4;
        // Sample one pixel (4 bytes) from each corner and the center to localize
        // how much of the desktop actually got painted vs. left at the initial
        // zeroed DecodedImage buffer.
        let pixel_at = |px: usize, py: usize| -> String {
            let off = (py * w + px) * 4;
            if off + 4 <= rgba.len() {
                format!(
                    "{:02x}{:02x}{:02x}{:02x}",
                    rgba[off],
                    rgba[off + 1],
                    rgba[off + 2],
                    rgba[off + 3]
                )
            } else {
                "??".to_owned()
            }
        };
        let nonzero_count = rgba.iter().filter(|&&b| b != 0).count();
        let pct = 100.0 * nonzero_count as f64 / rgba.len().max(1) as f64;
        vrde_log(
            log_file,
            format_args!(
                "VRDE: send_frame диагностика w={w} h={h} len={} expected={} nonzero={}/{} ({:.2}%) \
                 tl={} tr={} center={} bl={} br={}",
                rgba.len(),
                expected_len,
                nonzero_count,
                rgba.len(),
                pct,
                pixel_at(0, 0),
                pixel_at(w.saturating_sub(1), 0),
                pixel_at(w / 2, h / 2),
                pixel_at(0, h.saturating_sub(1)),
                pixel_at(w.saturating_sub(1), h.saturating_sub(1)),
            ),
        );
    }
    // RDP desktop bitmaps carry no alpha channel; ironrdp's decoder leaves the
    // alpha byte at whatever DecodedImage was initialized with (0), so the
    // egui texture built from this buffer renders fully transparent — visible
    // RGB content, but invisible (shows the dark panel background = "black
    // screen") until alpha is forced opaque here.
    for px in rgba.chunks_exact_mut(4) {
        px[3] = 255;
    }
    if let Some(ref c) = cursor_shape {
        composite_cursor(&mut rgba, w, h, c, mouse_x, mouse_y);
    }
    frame_tx.try_send((w as u32, h as u32, rgba)).is_ok()
}

// Drives a reset ConnectionActivationSequence (CapabilitiesExchange →
// ConnectionFinalization) after a ServerDeactivateAll PDU, applying the same
// VirtualBox empty-FontMap tolerance as vbox_connect_finalize.
// Returns the new share_id on success.
//
// `single_sequence_step` is typed to `&mut ClientConnector` so we replicate
// its inner loop here for `ConnectionActivationSequence` via the `Sequence` trait.
fn vbox_reactivate<S: Read + Write>(
    cas: &mut ConnectionActivationSequence,
    framed: &mut Framed<S>,
) -> Result<u32, String> {
    use ironrdp_connector::connection_activation::ConnectionActivationState;
    use ironrdp_connector::Sequence;
    use std::error::Error as StdError;

    let mut buf = WriteBuf::new();
    let mut saved_share_id: Option<u32> = None;
    // VirtualBox VRDE sends truncated/malformed control PDUs during
    // reactivation often enough that we need bounded tolerance, not just a
    // single special-cased pattern (FontMap). Cap retries so a genuinely
    // broken stream still surfaces as an error instead of looping forever.
    let mut malformed_pdu_retries = 0u32;
    const MAX_MALFORMED_PDU_RETRIES: u32 = 8;

    loop {
        // Snapshot share_id before each step (needed if FontMap fails below).
        if let ConnectionActivationState::ConnectionFinalization { share_id, .. } =
            cas.connection_activation_state()
        {
            saved_share_id = Some(share_id);
        }

        buf.clear();
        let written = if let Some(hint) = cas.next_pdu_hint() {
            // Poll in short increments rather than relying on one long socket
            // timeout: most reactivations complete in well under 100ms, so
            // this returns as fast as the server responds instead of always
            // waiting out a fixed window, while still bounding the total
            // wait if the server genuinely goes quiet.
            let deadline = Instant::now() + Duration::from_secs(3);
            let pdu = loop {
                match framed.read_by_hint(hint) {
                    Ok(pdu) => break pdu,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) && Instant::now() < deadline =>
                    {
                        continue;
                    }
                    Err(e) => return Err(format!("read: {e}")),
                }
            };
            match cas.step(&pdu, &mut buf) {
                Ok(w) => w,
                Err(e) => {
                    let mut is_fontmap = false;
                    let mut is_truncated = false;
                    {
                        let mut cur: Option<&dyn StdError> = Some(&e);
                        while let Some(next) = cur {
                            let text = next.to_string();
                            if text.contains("FontPdu") {
                                is_fontmap = true;
                            }
                            if text.contains("not enough bytes") {
                                is_truncated = true;
                            }
                            cur = next.source();
                        }
                    }
                    // VirtualBox VRDE empty FontMap — treat as successful finish.
                    if is_fontmap {
                        return Ok(saved_share_id.unwrap_or(0));
                    }
                    // VirtualBox VRDE sometimes sends a control PDU shorter than
                    // its own declared header size (e.g. ShareControlHeader
                    // claiming 48 bytes but only 24 actually sent) — a genuine
                    // malformed packet, not something we can fix by reading
                    // differently. Skip it and keep going rather than failing
                    // the whole reactivation (and therefore the whole session)
                    // over one bad PDU from a server that's already known to be
                    // non-compliant in several other ways (empty FontMap,
                    // wrong grant_id/control_id — confirmed independently
                    // against the official ironrdp-viewer reference client).
                    if is_truncated && malformed_pdu_retries < MAX_MALFORMED_PDU_RETRIES {
                        malformed_pdu_retries += 1;
                        continue;
                    }
                    return Err(format!("{e}"));
                }
            }
        } else {
            cas.step_no_input(&mut buf).map_err(|e| format!("{e}"))?
        };

        // Forward any response bytes to the server.
        if let Some(len) = written.size() {
            framed
                .write_all(&buf[..len])
                .map_err(|e| format!("write: {e}"))?;
        }

        // Done when state is Finalized.
        if matches!(
            cas.connection_activation_state(),
            ConnectionActivationState::Finalized { .. }
        ) {
            break;
        }
    }

    Ok(match cas.connection_activation_state() {
        ConnectionActivationState::Finalized { share_id, .. } => share_id,
        _ => 0,
    })
}
