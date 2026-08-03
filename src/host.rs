//! Host service — two independent subsystems:
//!
//! 1. **Registration loop** (`run_host_loop`): connects to the ID server,
//!    sends `RegisterPeer`, keeps the registration alive with heartbeats, and
//!    forwards incoming `RequestRelay` messages to the UI.
//!
//! 2. **Relay session** (`handle_relay_session`): spawned for each accepted
//!    incoming connection.  Connects to the relay server, runs the RustDesk
//!    auth handshake (Hash → LoginRequest → LoginResponse), then enters a
//!    capture / encode / stream loop while receiving mouse & keyboard events
//!    from the remote client.

use std::{
    io::{Read, Write},
    net::{TcpStream, UdpSocket},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static APPROVALS: OnceLock<Mutex<std::collections::HashMap<String, bool>>> = OnceLock::new();
static RECENT_RELAY_EVENTS: OnceLock<Mutex<std::collections::HashMap<String, Instant>>> =
    OnceLock::new();
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);
static ACTIVE_SESSION: OnceLock<Mutex<Option<(u64, String)>>> = OnceLock::new();

#[derive(Clone)]
struct RemoteIdentity {
    peer_id: String,
    peer_name: String,
    platform: String,
    version: String,
}

impl RemoteIdentity {
    fn unknown() -> Self {
        Self {
            peer_id: "Неизвестное устройство".to_owned(),
            peer_name: String::new(),
            platform: String::new(),
            version: String::new(),
        }
    }
}

fn clean_identity_field(value: &str, limit: usize) -> String {
    value
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect()
}

fn remote_identity_from_login(login: &crate::rustdesk_proto::LoginRequest) -> RemoteIdentity {
    let raw_id = clean_identity_field(&login.my_id, 128);
    let raw_name = clean_identity_field(&login.my_name, 128);
    let peer_id = if !raw_id.is_empty()
        && !raw_id.eq_ignore_ascii_case("evertydesk-lite")
        && !raw_id.eq_ignore_ascii_case("rustdesk")
    {
        raw_id
    } else if !raw_name.is_empty()
        && !raw_name.eq_ignore_ascii_case("evertydesk lite")
        && !raw_name.eq_ignore_ascii_case("rustdesk")
    {
        raw_name.clone()
    } else {
        "Неизвестное устройство".to_owned()
    };
    RemoteIdentity {
        peer_id,
        peer_name: raw_name,
        platform: clean_identity_field(&login.my_platform, 64),
        version: clean_identity_field(&login.version, 32),
    }
}

fn safe_session_end_reason(reason: &str) -> String {
    let cleaned = clean_identity_field(reason, 160);
    let lowered = cleaned.to_lowercase();
    if cleaned.is_empty() {
        "Сессия завершена".to_owned()
    } else if lowered.contains("rejected") {
        "Подключение отклонено".to_owned()
    } else if lowered.contains("approval timed out") {
        "Истекло время подтверждения".to_owned()
    } else if lowered.contains("wrong-password") || lowered.contains("wrong password") {
        "Ошибка аутентификации".to_owned()
    } else {
        cleaned
    }
}

/// When true, incoming mouse/keyboard events from the remote client are discarded.
static BLOCK_REMOTE_INPUT: AtomicBool = AtomicBool::new(false);
static ALLOW_REMOTE_CLIPBOARD: AtomicBool = AtomicBool::new(true);

pub fn set_remote_input_blocked(blocked: bool) {
    BLOCK_REMOTE_INPUT.store(blocked, Ordering::Relaxed);
}

pub fn is_remote_input_blocked() -> bool {
    BLOCK_REMOTE_INPUT.load(Ordering::Relaxed)
}

pub fn set_remote_clipboard_allowed(allowed: bool) {
    ALLOW_REMOTE_CLIPBOARD.store(allowed, Ordering::Relaxed);
}

pub fn is_remote_clipboard_allowed() -> bool {
    ALLOW_REMOTE_CLIPBOARD.load(Ordering::Relaxed)
}

fn active_session() -> &'static Mutex<Option<(u64, String)>> {
    ACTIVE_SESSION.get_or_init(|| Mutex::new(None))
}

fn set_active_session(session_id: u64, peer_id: String) {
    if let Ok(mut active) = active_session().lock() {
        *active = Some((session_id, peer_id));
    }
}

fn clear_active_session(session_id: u64) -> bool {
    if let Ok(mut active) = active_session().lock() {
        if active
            .as_ref()
            .is_some_and(|(active_id, _)| *active_id == session_id)
        {
            *active = None;
            return true;
        }
    }
    false
}

fn apply_session_permissions(
    session_id: u64,
    peer_id: &str,
    input_blocked: bool,
    clipboard_allowed: bool,
) -> bool {
    let matches_active = active_session()
        .lock()
        .ok()
        .and_then(|active| active.clone())
        .is_some_and(|(active_id, active_peer)| active_id == session_id && active_peer == peer_id);
    if !matches_active {
        return false;
    }
    set_remote_input_blocked(input_blocked);
    set_remote_clipboard_allowed(clipboard_allowed);
    true
}

/// UI-driven kill switch. When the host user presses "Отключить" on the active
/// session panel, the UI sets this to `true`; the relay input loop checks it
/// each iteration and tears the session down cleanly (same path as a normal
/// disconnect). Reset to `false` when a new session starts so a stale request
/// can't kill a fresh connection.
static KICK_ACTIVE_SESSION: AtomicBool = AtomicBool::new(false);

pub fn request_kick_active_session() {
    KICK_ACTIVE_SESSION.store(true, Ordering::Relaxed);
}

fn take_kick_request() -> bool {
    KICK_ACTIVE_SESSION.swap(false, Ordering::Relaxed)
}

fn clear_kick_request() {
    KICK_ACTIVE_SESSION.store(false, Ordering::Relaxed);
}

fn approvals() -> &'static Mutex<std::collections::HashMap<String, bool>> {
    APPROVALS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn recent_relay_events() -> &'static Mutex<std::collections::HashMap<String, Instant>> {
    RECENT_RELAY_EVENTS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn relay_event_seen_recently(key: String, window: Duration) -> bool {
    let Ok(mut events) = recent_relay_events().lock() else {
        return false;
    };
    let now = Instant::now();
    events.retain(|_, seen| now.duration_since(*seen) < Duration::from_secs(10));
    if events
        .get(&key)
        .is_some_and(|seen| now.duration_since(*seen) < window)
    {
        true
    } else {
        events.insert(key, now);
        false
    }
}

use sha2::{Digest, Sha256};

use crate::{
    crypto::{self, StreamCipher},
    rustdesk_proto::{
        decode_message, decode_peer_message, encode_message, encode_peer_message, login_response,
        misc, peer_message, rendezvous_message, DisplayInfo, Hash, IdPk, ImageQuality,
        LoginResponse, Misc, PeerInfo, PeerMessage, PreferCodec, RegisterPeer, RegisterPk,
        RelayResponse, RendezvousMessage, RequestRelay, ShellMessage, ShellMessageKind, SignedId,
        SupportedDecoding,
    },
    settings::{AppConfig, CodecPreference, EncoderPreference},
    transport::{connect_tcp, encode_frame_len, read_framed, send_framed},
};
use prost::Message as _;

const RENDEZVOUS_PORT: u16 = 21116;
const RELAY_PORT: u16 = 21117;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(28);
const READ_TIMEOUT_SHORT: Duration = Duration::from_millis(300);
const READ_TIMEOUT_AUTH: Duration = Duration::from_secs(10);
const MAX_TARGET_FPS: u32 = 60;
const DEFAULT_QUALITY_MILLI: u32 = 1_000;
const LOW_QUALITY_MILLI: u32 = 700;
const BALANCED_QUALITY_MILLI: u32 = 1_000;
const BEST_QUALITY_MILLI: u32 = 1_800;

// ── Public types ──────────────────────────────────────────────────────────────

/// Observable state of the host service.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum HostState {
    Idle,
    Connecting,
    /// Registered and waiting for incoming connections.
    Ready,
    /// Relay session in progress.
    Accepting(String),
    Error(String),
}

impl HostState {
    pub fn is_online(&self) -> bool {
        matches!(self, HostState::Ready | HostState::Accepting(_))
    }

    pub fn label(&self) -> &str {
        match self {
            HostState::Idle => "Остановлен",
            HostState::Connecting => "Подключение...",
            HostState::Ready => "Готов к подключению",
            HostState::Accepting(_) => "Сессия активна",
            HostState::Error(_) => "Ошибка",
        }
    }

    pub fn color(&self) -> EguiColor {
        match self {
            HostState::Ready => (0x43, 0xA8, 0x47),
            HostState::Connecting => (0xF5, 0xA6, 0x23),
            HostState::Accepting(_) => (0x29, 0x9D, 0xD8),
            HostState::Error(_) => (0xE0, 0x50, 0x50),
            HostState::Idle => (0x88, 0x88, 0x88),
        }
    }
}

/// RGB triple used to avoid importing egui from this module.
pub type EguiColor = (u8, u8, u8);

/// Events sent from the host background thread to the UI.
///
/// Also the wire format for the Phase 1 `--host-agent` IPC link (see
/// `host_agent.rs`): serialized as one JSON object per line over a loopback
/// TCP socket when the host runs in its own process instead of a GUI thread.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum HostEvent {
    StateChanged(HostState),
    Registered {
        #[allow(dead_code)]
        request_pk: bool,
    },
    IncomingRequest {
        peer_id: String,
        #[allow(dead_code)]
        relay_server: String,
        #[allow(dead_code)]
        uuid: String,
    },
    ApprovalRequested {
        peer_id: String,
        #[serde(default)]
        peer_name: String,
        #[serde(default)]
        platform: String,
        #[serde(default)]
        version: String,
    },
    SessionStarted {
        peer_id: String,
        #[serde(default)]
        session_id: u64,
        #[serde(default)]
        peer_name: String,
        #[serde(default)]
        platform: String,
        #[serde(default)]
        version: String,
    },
    SessionEnded {
        peer_id: String,
        reason: String,
        #[serde(default)]
        session_id: u64,
    },
    VideoTelemetry {
        summary: String,
        fallback_reason: Option<String>,
    },
    SessionPermissionsChanged {
        peer_id: String,
        session_id: u64,
        input_blocked: bool,
        clipboard_allowed: bool,
    },
    Log(String),
}

/// Commands sent from the UI to the host thread.
///
/// Also the wire format for the Phase 1 `--host-agent` IPC link — see
/// `HostEvent`'s doc comment.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum HostCommand {
    Stop,
    Reconfigure(AppConfig),
    /// Routed to the agent process's own `approvals()` map — in agent mode
    /// `HostService::approve_incoming` can no longer write that map directly
    /// because it lives in a different process's address space.
    ApproveIncoming {
        peer_id: String,
        accept: bool,
    },
    /// Take-control toggle, see HostService::set_input_blocked. Same
    /// cross-process reasoning as ApproveIncoming: BLOCK_REMOTE_INPUT lives
    /// wherever the actual relay session thread runs, which in agent mode
    /// is not this process.
    SetInputBlocked(bool),
    /// Allow or reject clipboard messages for the active incoming session.
    /// This is process-global for the same reason as `SetInputBlocked`.
    SetClipboardAllowed(bool),
    /// Atomically changes permissions only when `peer_id` is still the
    /// currently active incoming session and reports the applied state.
    SetSessionPermissions {
        peer_id: String,
        session_id: u64,
        input_blocked: bool,
        clipboard_allowed: bool,
    },
    /// Disconnect client, see HostService::kick_active_session.
    KickActiveSession,
}

// ── Service handle ────────────────────────────────────────────────────────────

/// What's on the other end of `event_rx`/`command_tx`.
///
/// `InProcess` is the original, always-available behavior: `run_host_loop`
/// on a thread in this same process, signaled directly through a shared
/// `AtomicBool`. `Agent` is Phase 1 (`TZ_HOST_SERVICE.md`) — the same loop
/// running in a separate `--host-agent` process, bridged over a loopback
/// socket by `host_agent.rs`. `HostService`'s public methods hide which one
/// is active from every other call site in the app.
enum Backend {
    InProcess(Arc<AtomicBool>),
    Agent(std::net::TcpStream),
}

pub struct HostService {
    pub event_rx: Receiver<HostEvent>,
    command_tx: Sender<HostCommand>,
    backend: Backend,
}

impl HostService {
    /// Starts hosting, using the out-of-process agent (`host_agent::enabled`)
    /// when opted in and reachable, falling back to the in-process path on
    /// any failure — Phase 1 must never be a hard dependency for hosting to
    /// work at all. Prefer this over `start_in_process` at every normal call
    /// site; the agent process itself is the one exception (see
    /// `host_agent::run_host_agent`, which must not try to spawn another
    /// agent recursively).
    pub fn start(config: AppConfig) -> Self {
        // Android has no separate-process host model (the whole app is one
        // .so loaded into a Kotlin host process) — Phase 1 is desktop-only.
        #[cfg(not(target_os = "android"))]
        {
            if crate::host_agent::enabled() {
                if let Some(svc) = crate::host_agent::connect_or_spawn(config.clone()) {
                    return svc;
                }
            }
        }
        let svc = Self::start_in_process(config);
        // Phase 2 (approval_prompt.rs) needs a way to reach whichever
        // process is actually hosting, regardless of whether Phase 1 is
        // enabled — this is the in-process half of that; `run_host_agent`
        // (Phase 1's own process) sets up its own richer version of the same
        // thing itself, so this must not run there (hence calling
        // `start_in_process` directly above, not through here).
        #[cfg(not(target_os = "android"))]
        crate::host_agent::spawn_command_listener(svc.command_sender());
        svc
    }

    /// The original behavior: `run_host_loop` on a thread in this process.
    pub fn start_in_process(config: AppConfig) -> Self {
        let (event_tx, event_rx) = mpsc::channel::<HostEvent>();
        let (command_tx, command_rx) = mpsc::channel::<HostCommand>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        thread::spawn(move || run_host_loop(config, event_tx, command_rx, stop_thread));
        Self {
            event_rx,
            command_tx,
            backend: Backend::InProcess(stop),
        }
    }

    /// Wraps an already-connected agent link (see `host_agent::connect_or_spawn`)
    /// in the same `HostService` shape every other call site already uses.
    /// `socket` is kept only so `detach()` can shut it down cleanly.
    pub(crate) fn from_agent_link(
        event_rx: Receiver<HostEvent>,
        command_tx: Sender<HostCommand>,
        socket: std::net::TcpStream,
    ) -> Self {
        Self {
            event_rx,
            command_tx,
            backend: Backend::Agent(socket),
        }
    }

    pub fn command_sender(&self) -> Sender<HostCommand> {
        self.command_tx.clone()
    }

    /// Actually stops hosting: for `Agent`, this ends the agent process too
    /// (see `host_agent::run_host_agent`'s handling of `HostCommand::Stop`).
    pub fn stop(&self) {
        if let Backend::InProcess(stop) = &self.backend {
            stop.store(true, Ordering::Relaxed);
        }
        let _ = self.command_tx.send(HostCommand::Stop);
    }

    /// Leaves hosting running and just drops this process's link to it.
    ///
    /// For `Agent`, the agent process keeps hosting after this returns — the
    /// whole point of Phase 1 is that closing/minimizing the GUI must not
    /// kill the stream (`TZ_HOST_SERVICE.md` §1). For `InProcess` there is no
    /// separate process to leave running (the host thread dies with this one
    /// regardless), so this is equivalent to `stop()`.
    pub fn detach(&self) {
        match &self.backend {
            Backend::Agent(socket) => {
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }
            Backend::InProcess(stop) => {
                stop.store(true, Ordering::Relaxed);
                let _ = self.command_tx.send(HostCommand::Stop);
            }
        }
    }

    pub fn is_agent_backed(&self) -> bool {
        matches!(self.backend, Backend::Agent(_))
    }

    pub fn reconfigure(&self, config: AppConfig) {
        let _ = self.command_tx.send(HostCommand::Reconfigure(config));
    }

    pub fn approve_incoming(&self, peer_id: &str, accept: bool) {
        match &self.backend {
            // Agent mode: `approvals()` in *this* process is a different map
            // than the one `handle_relay_session` checks in the agent
            // process — must cross the IPC link instead.
            Backend::Agent(_) => {
                let _ = self.command_tx.send(HostCommand::ApproveIncoming {
                    peer_id: peer_id.to_owned(),
                    accept,
                });
            }
            Backend::InProcess(_) => {
                if let Ok(mut approvals) = approvals().lock() {
                    approvals.insert(peer_id.to_owned(), accept);
                }
            }
        }
    }

    /// "Take control" toggle — blocks (or un-blocks) the remote client's own
    /// mouse/keyboard while a session is active. `BLOCK_REMOTE_INPUT` is a
    /// process-local static, so in agent mode this must cross the IPC link
    /// like `approve_incoming` does, not call `set_remote_input_blocked`
    /// directly (that would only touch this — irrelevant — process's copy).
    pub fn set_input_blocked(&self, blocked: bool) {
        match &self.backend {
            Backend::Agent(_) => {
                let _ = self.command_tx.send(HostCommand::SetInputBlocked(blocked));
            }
            Backend::InProcess(_) => {
                set_remote_input_blocked(blocked);
            }
        }
    }

    /// "Disconnect client" — ends the active session, same cross-process
    /// reasoning as `set_input_blocked`.
    pub fn kick_active_session(&self) {
        match &self.backend {
            Backend::Agent(_) => {
                let _ = self.command_tx.send(HostCommand::KickActiveSession);
            }
            Backend::InProcess(_) => {
                request_kick_active_session();
            }
        }
    }

    pub fn try_recv(&self) -> Option<HostEvent> {
        self.event_rx.try_recv().ok()
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Windows Firewall helpers
// ═══════════════════════════════════════════════════════════════════════════════

/// On Windows, registers an inbound UDP firewall exception for the current
/// executable so that hbbs can send `RegisterPeerResponse` / `RequestRelay`
/// packets back to us.
///
/// Uses `netsh advfirewall` (no external tools required).  If the rule already
/// exists the function returns immediately.  If `netsh` fails due to missing
/// admin rights, it re-tries via `powershell Start-Process -Verb RunAs`
/// (triggers a one-time UAC elevation dialog).
///
/// On non-Windows platforms this is a no-op.
fn setup_udp_firewall(events: &Sender<HostEvent>) {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;

        const RULE: &str = "EvertyDesk-Lite-UDP";

        let exe = match std::env::current_exe() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(_) => return,
        };

        // ── Check whether the rule already exists ─────────────────────────
        let already = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "show",
                "rule",
                &format!("name={RULE}"),
            ])
            .output()
            .map(|o| {
                // Rule name appears in output if it exists (byte-safe, ASCII)
                o.stdout.windows(RULE.len()).any(|w| w == RULE.as_bytes())
            })
            .unwrap_or(false);

        if already {
            host_log(events, "Firewall UDP rule already present ✓".to_owned());
            return;
        }

        host_log(
            events,
            "Добавление правила Windows Firewall для входящего UDP…".to_owned(),
        );

        let netsh_args = [
            "advfirewall",
            "firewall",
            "add",
            "rule",
            &format!("name={RULE}"),
            "dir=in",
            "action=allow",
            "protocol=UDP",
            &format!("program={exe}"),
            "enable=yes",
        ];

        // ── First attempt: run directly (works when already admin) ────────
        let direct_ok = Command::new("netsh")
            .args(&netsh_args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if direct_ok {
            host_log(events, "Firewall UDP rule added ✓".to_owned());
            return;
        }

        // ── Second attempt: request elevation via PowerShell UAC ──────────
        // Builds: powershell -NoProfile -Command
        //         Start-Process netsh -Verb RunAs -Wait -ArgumentList '...'
        let joined_args = netsh_args.join(" ");
        let ps_cmd = format!("Start-Process netsh -Verb RunAs -Wait -ArgumentList '{joined_args}'");

        host_log(
            events,
            "Запрос прав администратора для правила файрвола (UAC)…".to_owned(),
        );

        let elevated_ok = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if elevated_ok {
            host_log(events, "Firewall UDP rule added via UAC ✓".to_owned());
        } else {
            host_log(
                events,
                format!(
                    "⚠ Не удалось добавить правило файрвола автоматически.\n\
                 Запусти ОДИН РАЗ от Администратора:\n\
                 netsh advfirewall firewall add rule \
                 name=\"{RULE}\" dir=in action=allow protocol=UDP \
                 program=\"{exe}\" enable=yes"
                ),
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    let _ = events; // suppress unused warning
}

// ═══════════════════════════════════════════════════════════════════════════════
// Registration loop
// ═══════════════════════════════════════════════════════════════════════════════

fn run_host_loop(
    mut config: AppConfig,
    events: Sender<HostEvent>,
    commands: Receiver<HostCommand>,
    stop: Arc<AtomicBool>,
) {
    // ★ Штамп сборки запущенного бинаря — видно в консоли хоста, свежий ли он.
    host_log(
        &events,
        format!(
            "🔧 EvertyDesk Lite ХОСТ запущен | build={} | telemetry+downscale+NVENC-cascade",
            binary_build_stamp(),
        ),
    );

    // On Windows, ensure an inbound UDP firewall rule exists for this binary.
    // This is needed because Windows Firewall blocks unsolicited inbound UDP
    // from the ID server (hbbs).  The rule is a no-op on subsequent runs once
    // already present.
    setup_udp_firewall(&events);

    // Логируем доступные кодеки — видно в GUI при первом запуске.
    #[cfg(all(feature = "live-vp9-mf", target_os = "windows"))]
    {
        let enc_status = crate::mf_encode::mf_encoder_status();
        let enc = enc_status.label();
        let dec = crate::mf_video::mf_video_decode_status().label();
        host_log(&events, format!("Кодеки: {enc} | {dec}"));

        // Аппаратный H264 есть, а HEVC нет вообще — это почти всегда НЕ про
        // железо. Любой GPU, который выдаёт H264(hw) через Quick Sync/NVENC/AMF,
        // умеет и HEVC в кремнии; но Microsoft вынесла HEVC из Windows в
        // отдельный пакет Store, и без него Media Foundation не показывает ни
        // одного HEVC MFT. Пишем подсказку явно — иначе «нет H265» выглядит как
        // приговор железу и уводит диагностику не туда (живо словили на
        // Intel UHD 610: H264(hw) есть, HEVC нет ни в encode, ни в decode).
        if enc_status.has_hardware_h264() && !enc_status.has_hardware_h265() {
            host_log(
                &events,
                "ℹ️ Аппаратный H265 не найден через Media Foundation, хотя аппаратный H264 \
                 работает. Это почти всегда не про железо, а про то, что Microsoft вынесла \
                 HEVC из Windows в отдельный пакет Store. Ставить его мы не требуем — \
                 см. строку D3D12 ниже: этот путь идёт напрямую в драйвер и пакета не требует."
                    .to_owned(),
            );
        }

        // D3D12 Video Encode — путь мимо Media Foundation. Логируем ВСЕГДА, в
        // том числе отрицательный результат: именно эта строка отвечает на
        // вопрос, можно ли на данной машине кодировать HEVC без пакета Store.
        host_log(&events, crate::d3d12_encode::d3d12_encode_status().label());
        // Второй путь мимо Media Foundation — рантайм Intel, приезжающий с
        // драйвером. Отвечает на вопрос, какое поколение API стоит на машине
        // (legacy MSDK 1.x или oneVPL 2.x) — от этого зависит реализация.
        host_log(&events, crate::onevpl::onevpl_status().label());
    }

    // On macOS, host mode needs Screen Recording + Accessibility permission.
    // Surface a clear hint if either is missing — otherwise capture is blank or
    // remote input silently does nothing.
    #[cfg(target_os = "macos")]
    {
        let (ok, hints) = macos_cgevent::preflight_permissions();
        if ok {
            host_log(
                &events,
                "macOS: Screen Recording + Accessibility доступ есть ✓".to_owned(),
            );
        } else {
            for hint in hints {
                host_log(&events, hint);
            }
        }
    }

    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = events.send(HostEvent::StateChanged(HostState::Idle));
            return;
        }
        let _ = events.send(HostEvent::StateChanged(HostState::Connecting));
        host_log(
            &events,
            format!("Connecting to ID server {}…", config.server.id_server),
        );

        match registration_loop(&config, &events, &commands, &stop) {
            LoopResult::Stop => {
                let _ = events.send(HostEvent::StateChanged(HostState::Idle));
                return;
            }
            LoopResult::Reconfigure(cfg) => {
                config = cfg;
                host_log(&events, "Config updated — reconnecting.".to_owned());
            }
            LoopResult::Error(e) => {
                let _ = events.send(HostEvent::StateChanged(HostState::Error(e.clone())));
                host_log(&events, format!("Error: {e}  Retrying in 10 s…"));
                let deadline = Instant::now() + Duration::from_secs(10);
                loop {
                    thread::sleep(Duration::from_millis(250));
                    if stop.load(Ordering::Relaxed) {
                        let _ = events.send(HostEvent::StateChanged(HostState::Idle));
                        return;
                    }
                    if let Ok(cmd) = commands.try_recv() {
                        match cmd {
                            HostCommand::Stop => {
                                let _ = events.send(HostEvent::StateChanged(HostState::Idle));
                                return;
                            }
                            HostCommand::Reconfigure(cfg) => {
                                config = cfg;
                                break;
                            }
                            HostCommand::ApproveIncoming { peer_id, accept } => {
                                if let Ok(mut approvals) = approvals().lock() {
                                    approvals.insert(peer_id, accept);
                                }
                            }
                            HostCommand::SetInputBlocked(blocked) => {
                                set_remote_input_blocked(blocked);
                            }
                            HostCommand::SetClipboardAllowed(allowed) => {
                                set_remote_clipboard_allowed(allowed);
                            }
                            HostCommand::SetSessionPermissions {
                                peer_id,
                                session_id,
                                input_blocked,
                                clipboard_allowed,
                            } => {
                                if apply_session_permissions(
                                    session_id,
                                    &peer_id,
                                    input_blocked,
                                    clipboard_allowed,
                                ) {
                                    let _ = events.send(HostEvent::SessionPermissionsChanged {
                                        peer_id,
                                        session_id,
                                        input_blocked,
                                        clipboard_allowed,
                                    });
                                }
                            }
                            HostCommand::KickActiveSession => {
                                request_kick_active_session();
                            }
                        }
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                }
            }
        }
    }
}

enum LoopResult {
    Stop,
    Reconfigure(AppConfig),
    Error(String),
}

fn registration_loop(
    config: &AppConfig,
    events: &Sender<HostEvent>,
    commands: &Receiver<HostCommand>,
    stop: &Arc<AtomicBool>,
) -> LoopResult {
    // Диагностические self-тесты нужны только для отладки десктоп-хоста и
    // добавляют ~10с задержки (таймауты 3с). На Android их пропускаем, чтобы
    // хост регистрировался на hbbs немедленно — иначе клиент, подключающийся
    // сразу, получает "id does not exist" пока хост ещё не дошёл до RegisterPk.
    #[cfg(not(target_os = "android"))]
    {
        // ── Self-test: UDP loopback (proves recv_from works on this machine) ─
        udp_loopback_test(events);

        // ── Self-test: external UDP (proves inbound UDP from internet works) ─
        udp_internet_test(events);

        // ── TCP probe: see what the server actually does on port 21116 ───────
        tcp_probe(
            &config.server.id_server,
            RENDEZVOUS_PORT,
            &config.local_id,
            events,
        );
    }

    // ── DNS resolution ────────────────────────────────────────────────────────
    {
        use std::net::ToSocketAddrs;
        let probe = format!("{}:{}", config.server.id_server, RENDEZVOUS_PORT);
        match probe.to_socket_addrs() {
            Ok(it) => {
                let ips: Vec<_> = it.map(|a| a.ip().to_string()).collect();
                host_log(
                    events,
                    format!("DNS {} → [{}]", config.server.id_server, ips.join(", ")),
                );
            }
            Err(e) => {
                return LoopResult::Error(format!(
                    "DNS resolve failed for {}: {e}",
                    config.server.id_server
                ));
            }
        }
    }

    // ── UDP socket ────────────────────────────────────────────────────────────
    // Use a specific bind port if configured (e.g., via --bind-port CLI arg).
    // This lets us reuse the port that was previously registered with hbbs,
    // which can help when the server tracks peers by source IP:port.
    let bind_addr = if config.udp_bind_port > 0 {
        format!("0.0.0.0:{}", config.udp_bind_port)
    } else {
        "0.0.0.0:0".to_owned()
    };
    // Оборачиваем в Arc — тот же сокет передаётся в EVRT-сессии.
    // Это критично: punch-hole работает только с портом, который зарегистрирован на hbbs.
    let socket = Arc::new(match UdpSocket::bind(&bind_addr) {
        Ok(s) => s,
        Err(e) => {
            if config.udp_bind_port > 0 {
                host_log(
                    events,
                    format!(
                        "UDP bind to port {} failed ({e}) — falling back to random port",
                        config.udp_bind_port
                    ),
                );
                match UdpSocket::bind("0.0.0.0:0") {
                    Ok(s) => s,
                    Err(e2) => return LoopResult::Error(format!("UDP bind: {e2}")),
                }
            } else {
                return LoopResult::Error(format!("UDP bind: {e}"));
            }
        }
    });
    socket.set_read_timeout(Some(READ_TIMEOUT_SHORT)).ok();

    // Log the local port so the user can verify the firewall rule covers it.
    if let Ok(local) = socket.local_addr() {
        host_log(events, format!("UDP socket local addr: {local}"));
    }

    let server_addr = format!("{}:{}", config.server.id_server, RENDEZVOUS_PORT);

    // ── Step 1: RegisterPk first (matches actual RustDesk behaviour) ─────────
    //
    // Modern hbbs ignores RegisterPeer from peers whose public key has not
    // been registered yet.  RustDesk logs say:
    //   "register_pk of edesk due to key not confirmed"
    // — it sends RegisterPk proactively WITHOUT waiting for a
    // RegisterPeerResponse(request_pk=true) prompt.
    let pk_bytes = encode_message(&{
        use sha2::{Digest, Sha256};
        // Prefer our stable Ed25519 *sign* public key (we hold the matching
        // secret, so we can complete the secure handshake). Fall back to the
        // explicit --use-everty-keys key, then to a deterministic fake.
        let pk: Vec<u8> = if config.host_sign_pk.len() == 32 {
            host_log(
                events,
                "RegisterPk: using stable Ed25519 sign key".to_owned(),
            );
            config.host_sign_pk.clone()
        } else if config.host_pk.len() == 32 {
            config.host_pk.clone()
        } else {
            let mut h = Sha256::new();
            h.update(b"evertydesk-pk:");
            h.update(config.local_id.as_bytes());
            h.finalize().to_vec()
        };
        let mut hu = Sha256::new();
        hu.update(b"evertydesk-uuid:");
        hu.update(config.local_id.as_bytes());
        let uuid_bytes: Vec<u8> = hu.finalize()[..16].to_vec();
        RendezvousMessage {
            union: Some(rendezvous_message::Union::RegisterPk(RegisterPk {
                id: config.local_id.clone(),
                uuid: uuid_bytes,
                pk,
                old_pk: Vec::new(),
            })),
        }
    });
    host_log(
        events,
        format!(
            "RegisterPk packet: {} bytes  hex={}",
            pk_bytes.len(),
            hex_short(&pk_bytes)
        ),
    );
    if let Err(e) = socket.send_to(&pk_bytes, &server_addr) {
        return LoopResult::Error(format!("RegisterPk send: UDP send_to: {e}"));
    }
    let mut send_count: u32 = 1;
    host_log(
        events,
        format!(
            "RegisterPk sent → {server_addr}  id={}  (#{send_count})",
            config.local_id
        ),
    );

    // ── Step 2: RegisterPeer immediately after ────────────────────────────────
    let reg_bytes = encode_message(&RendezvousMessage {
        union: Some(rendezvous_message::Union::RegisterPeer(RegisterPeer {
            id: config.local_id.clone(),
            serial: 0,
        })),
    });
    host_log(
        events,
        format!(
            "RegisterPeer packet: {} bytes  hex={}",
            reg_bytes.len(),
            hex_short(&reg_bytes)
        ),
    );
    if let Err(e) = socket.send_to(&reg_bytes, &server_addr) {
        return LoopResult::Error(format!("RegisterPeer send: UDP send_to: {e}"));
    }
    send_count += 1;
    host_log(
        events,
        format!(
            "RegisterPeer sent → {server_addr}  id={}  (#{send_count})",
            config.local_id
        ),
    );

    let mut last_hb = Instant::now();
    let mut last_tick = Instant::now();
    let mut buf = vec![0u8; 8192];
    // Once the server accepts our key (RegisterPkResponse result=0), we stop
    // re-sending RegisterPk on every heartbeat — the server rate-limits it
    // (TOO_FREQUENT / result=4).  Only RegisterPeer is needed to keep the
    // registration alive.  We re-send RegisterPk only if the server requests
    // it via RegisterPeerResponse(request_pk=true).
    let mut pk_confirmed = false;

    loop {
        if stop.load(Ordering::Relaxed) {
            return LoopResult::Stop;
        }
        // ── Periodic diagnostic tick ──────────────────────────────────────────
        if last_tick.elapsed() >= Duration::from_secs(5) {
            host_log(events, format!(
                "Ожидание ответа от {server_addr} … (отправлено {send_count}×, следующий heartbeat через {:.0}s)",
                HEARTBEAT_INTERVAL.saturating_sub(last_hb.elapsed()).as_secs_f32()
            ));
            last_tick = Instant::now();
        }

        // ── Commands ──────────────────────────────────────────────────────────
        while let Ok(cmd) = commands.try_recv() {
            match cmd {
                HostCommand::Stop => return LoopResult::Stop,
                HostCommand::Reconfigure(cfg) => return LoopResult::Reconfigure(cfg),
                HostCommand::ApproveIncoming { peer_id, accept } => {
                    if let Ok(mut approvals) = approvals().lock() {
                        approvals.insert(peer_id, accept);
                    }
                }
                HostCommand::SetInputBlocked(blocked) => {
                    set_remote_input_blocked(blocked);
                }
                HostCommand::SetClipboardAllowed(allowed) => {
                    set_remote_clipboard_allowed(allowed);
                }
                HostCommand::SetSessionPermissions {
                    peer_id,
                    session_id,
                    input_blocked,
                    clipboard_allowed,
                } => {
                    if apply_session_permissions(
                        session_id,
                        &peer_id,
                        input_blocked,
                        clipboard_allowed,
                    ) {
                        let _ = events.send(HostEvent::SessionPermissionsChanged {
                            peer_id,
                            session_id,
                            input_blocked,
                            clipboard_allowed,
                        });
                    }
                }
                HostCommand::KickActiveSession => {
                    request_kick_active_session();
                }
            }
        }

        // ── Heartbeat ─────────────────────────────────────────────────────────
        if last_hb.elapsed() >= HEARTBEAT_INTERVAL {
            // Only re-send RegisterPk if the server hasn't confirmed it yet.
            // After the key is confirmed (result=0), heartbeats use RegisterPeer
            // only — re-sending RegisterPk triggers TOO_FREQUENT (result=4).
            if !pk_confirmed {
                if let Err(e) = send_register_pk_udp(
                    &socket,
                    &server_addr,
                    &config.local_id,
                    &config.host_sign_pk,
                ) {
                    return LoopResult::Error(format!("Heartbeat RegisterPk: {e}"));
                }
                send_count += 1;
            }
            if let Err(e) = send_register_peer_udp(&socket, &server_addr, &config.local_id) {
                return LoopResult::Error(format!("Heartbeat RegisterPeer: {e}"));
            }
            send_count += 1;
            host_log(
                events,
                format!(
                    "Heartbeat: {} → {server_addr} (#{send_count})",
                    if pk_confirmed {
                        "RegisterPeer"
                    } else {
                        "RegisterPk+RegisterPeer"
                    }
                ),
            );
            last_hb = Instant::now();
        }

        // ── Read incoming UDP datagram from ANY source ────────────────────────
        match socket.recv_from(&mut buf) {
            Ok((len, src)) => {
                host_log(events, format!("UDP recv {len} bytes from {src}"));
                match decode_message(&buf[..len]) {
                    Ok(msg) => match msg.union {
                        Some(rendezvous_message::Union::RegisterPeerResponse(r)) => {
                            host_log(
                                events,
                                format!("RegisterPeerResponse  request_pk={}", r.request_pk),
                            );
                            if r.request_pk {
                                pk_confirmed = false;
                                if let Err(e) = send_register_pk_udp(
                                    &socket,
                                    &server_addr,
                                    &config.local_id,
                                    &config.host_sign_pk,
                                ) {
                                    return LoopResult::Error(format!("RegisterPk: {e}"));
                                }
                                host_log(events, "RegisterPk sent (server requested)".to_owned());
                            } else {
                                host_log(events, "Registered ✓ (key already on server)".to_owned());
                                let _ = events.send(HostEvent::Registered { request_pk: false });
                                let _ = events.send(HostEvent::StateChanged(HostState::Ready));
                            }
                        }
                        Some(rendezvous_message::Union::RegisterPkResponse(r)) => {
                            let meaning = match r.result {
                                0 => "OK",
                                1 => "UUID_MISMATCH",
                                2 => "NOT_SUPPORT",
                                3 => "SERVER_ERROR",
                                4 => "INVALID_ID_FORMAT / NOT_DEPLOYED",
                                _ => "unknown",
                            };
                            host_log(
                                events,
                                format!("RegisterPkResponse  result={} ({})", r.result, meaning),
                            );
                            if r.result == 0 {
                                pk_confirmed = true;
                                host_log(
                                    events,
                                    "Public key accepted — host is online ✓".to_owned(),
                                );
                                let _ = events.send(HostEvent::Registered { request_pk: true });
                                let _ = events.send(HostEvent::StateChanged(HostState::Ready));
                            } else {
                                return LoopResult::Error(format!(
                                    "RegisterPk rejected: {} ({})",
                                    meaning, r.result
                                ));
                            }
                        }
                        other => {
                            let msg2 = RendezvousMessage { union: other };
                            if let Some(r) =
                                handle_rendezvous_msg(msg2, config, events, &socket, stop)
                            {
                                return r;
                            }
                        }
                    },
                    Err(e) => host_log(events, format!("Decode error ({len}B from {src}): {e}")),
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return LoopResult::Error(format!("UDP recv_from: {e}")),
        }
    }
}

fn handle_rendezvous_msg(
    msg: RendezvousMessage,
    config: &AppConfig,
    events: &Sender<HostEvent>,
    reg_socket: &Arc<UdpSocket>,
    stop: &Arc<AtomicBool>,
) -> Option<LoopResult> {
    match msg.union {
        Some(rendezvous_message::Union::RegisterPeerResponse(r)) => {
            host_log(events, format!("Registered ✓  request_pk={}", r.request_pk));
            let _ = events.send(HostEvent::Registered {
                request_pk: r.request_pk,
            });
            let _ = events.send(HostEvent::StateChanged(HostState::Ready));
            None
        }

        // Relay-based incoming connection request.
        Some(rendezvous_message::Union::RequestRelay(req)) => {
            let dedupe_key = format!("request-relay:{}:{}", req.id, req.uuid);
            if relay_event_seen_recently(dedupe_key, Duration::from_millis(500)) {
                host_log(
                    events,
                    format!("Duplicate RequestRelay ignored uuid={}", req.uuid),
                );
                return None;
            }
            let relay = if req.relay_server.is_empty() {
                config.server.relay_server.clone()
            } else {
                req.relay_server.clone()
            };
            host_log(
                events,
                format!(
                    "RequestRelay incoming target={} relay={relay} uuid={}",
                    req.id, req.uuid
                ),
            );
            send_relay_response(
                config,
                events,
                req.socket_addr.clone(),
                relay.clone(),
                req.uuid.clone(),
            );
            let _ = events.send(HostEvent::StateChanged(HostState::Accepting(
                req.id.clone(),
            )));
            let _ = events.send(HostEvent::IncomingRequest {
                peer_id: req.id.clone(),
                relay_server: relay.clone(),
                uuid: req.uuid.clone(),
            });

            // Spawn relay session.
            let cfg = config.clone();
            let evs = events.clone();
            let peer_id = req.id.clone();
            let uuid = req.uuid.clone();
            let stop_session = stop.clone();
            thread::spawn(move || {
                handle_relay_session(cfg, evs, peer_id, relay, uuid, stop_session);
            });
            None
        }

        // ── Standard incoming connection: server tells host a peer wants in ──
        // We always go through the relay (firewall-proof), regardless of the
        // direct-punch hint.
        // Пробуем прямой UDP (EVRT) если force_relay=false и адрес декодируется.
        // При неудаче — TCP relay как раньше.
        Some(rendezvous_message::Union::PunchHole(ph)) => {
            let dedupe_key = format!("punch-hole:{:?}", ph.socket_addr);
            if relay_event_seen_recently(dedupe_key, Duration::from_millis(500)) {
                host_log(events, "Duplicate PunchHole ignored".to_owned());
                return None;
            }
            let relay_server = if ph.relay_server.is_empty() {
                config.server.relay_server.clone()
            } else {
                ph.relay_server.clone()
            };
            host_log(
                events,
                format!(
                    "PunchHole incoming  relay={relay_server} force_relay={}",
                    ph.force_relay
                ),
            );

            // PunchHole: логируем адрес клиента и переходим к TCP relay.
            // EVRT активируется из video_pipeline через evrt_send_loop —
            // там ждут punch уже на выделенном EVRT сокете (другой порт).
            if !ph.force_relay {
                if let Some(peer_addr) = crate::evrt_session::decode_punch_addr(&ph.socket_addr) {
                    host_log(
                        events,
                        format!(
                        "PunchHole: peer={peer_addr} (EVRT будет активирован через video_pipeline)",
                    ),
                    );
                    // 3× punch чтобы открыть NAT-дырку для последующего EVRT
                    for _ in 0..3 {
                        let _ = reg_socket.send_to(&[0u8], peer_addr);
                        thread::sleep(Duration::from_millis(30));
                    }
                }
            }

            // force_relay=true или адрес не декодируется → стандартный TCP relay
            create_relay(config, events, ph.socket_addr, relay_server, stop.clone());
            None
        }

        // LAN peer asking for local address — just relay it too (universal).
        Some(rendezvous_message::Union::FetchLocalAddr(fla)) => {
            let dedupe_key = format!("fetch-local:{:?}", fla.socket_addr);
            if relay_event_seen_recently(dedupe_key, Duration::from_millis(500)) {
                host_log(events, "Duplicate FetchLocalAddr ignored".to_owned());
                return None;
            }
            let relay_server = if fla.relay_server.is_empty() {
                config.server.relay_server.clone()
            } else {
                fla.relay_server.clone()
            };
            host_log(
                events,
                format!("FetchLocalAddr incoming  relay={relay_server}"),
            );
            create_relay(config, events, fla.socket_addr, relay_server, stop.clone());
            None
        }

        Some(rendezvous_message::Union::PunchHoleRequest(req)) => {
            host_log(events, format!("PunchHoleRequest id={}", req.id));
            None
        }

        _ => None,
    }
}

/// Host side of an incoming relay connection.
///
/// 1. Opens a fresh TCP connection to the rendezvous server and sends a
///    `RelayResponse` so the server can tell the peer which relay + uuid to
///    use.
/// 2. Spawns the relay session: connects to the relay server, identifies by
///    uuid, runs the auth handshake, then captures / encodes / streams.
fn send_relay_response(
    config: &AppConfig,
    events: &Sender<HostEvent>,
    peer_socket_addr: Vec<u8>,
    relay_server: String,
    uuid: String,
) {
    let id_server = config.server.id_server.clone();
    match connect_tcp(&id_server, RENDEZVOUS_PORT) {
        Ok(mut sock) => {
            let resp = RendezvousMessage {
                union: Some(rendezvous_message::Union::RelayResponse(RelayResponse {
                    socket_addr: peer_socket_addr,
                    uuid: uuid.clone(),
                    relay_server: relay_server.clone(),
                    id: config.local_id.clone(),
                    version: "1.4.6".to_owned(),
                    ..Default::default()
                })),
            };
            if let Err(e) = send_framed(&mut sock, &encode_message(&resp)) {
                host_log(events, format!("RelayResponse send failed: {e}"));
            } else {
                host_log(
                    events,
                    format!("RelayResponse sent → {id_server} (uuid={uuid})"),
                );
            }
        }
        Err(e) => {
            host_log(
                events,
                format!("RelayResponse: connect rendezvous failed: {e}"),
            );
        }
    }
}

fn create_relay(
    config: &AppConfig,
    events: &Sender<HostEvent>,
    peer_socket_addr: Vec<u8>,
    relay_server: String,
    stop: Arc<AtomicBool>,
) {
    let uuid = uuid::Uuid::new_v4().to_string();

    // ── Step 1: tell the rendezvous server about the relay reservation ───────
    let id_server = config.server.id_server.clone();
    match connect_tcp(&id_server, RENDEZVOUS_PORT) {
        Ok(mut sock) => {
            let resp = RendezvousMessage {
                union: Some(rendezvous_message::Union::RelayResponse(RelayResponse {
                    socket_addr: peer_socket_addr,
                    uuid: uuid.clone(),
                    relay_server: relay_server.clone(),
                    id: config.local_id.clone(),
                    version: "1.4.6".to_owned(),
                    ..Default::default()
                })),
            };
            if let Err(e) = send_framed(&mut sock, &encode_message(&resp)) {
                host_log(events, format!("RelayResponse send failed: {e}"));
            } else {
                host_log(
                    events,
                    format!("RelayResponse sent → {id_server} (uuid={uuid})"),
                );
            }
        }
        Err(e) => {
            host_log(
                events,
                format!("create_relay: connect rendezvous failed: {e}"),
            );
            return;
        }
    }

    // ── Step 2: spawn the host-side relay session ────────────────────────────
    let _ = events.send(HostEvent::StateChanged(HostState::Accepting(uuid.clone())));
    let cfg = config.clone();
    let evs = events.clone();
    let peer_id = "(relay)".to_owned();
    thread::spawn(move || {
        handle_relay_session(cfg, evs, peer_id, relay_server, uuid, stop);
    });
}

// ═══════════════════════════════════════════════════════════════════════════════
// Relay session — auth + capture + stream + input injection
// ═══════════════════════════════════════════════════════════════════════════════

fn handle_relay_session(
    config: AppConfig,
    events: Sender<HostEvent>,
    peer_id: String,
    relay_server: String,
    uuid: String,
    stop: Arc<AtomicBool>,
) {
    let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed).max(1);
    let identity = Arc::new(Mutex::new(RemoteIdentity::unknown()));
    let end_reason = match relay_session_inner(
        &config,
        &events,
        &peer_id,
        session_id,
        Arc::clone(&identity),
        &relay_server,
        &uuid,
        &stop,
    ) {
        Ok(reason) => {
            host_log(&events, format!("Session with {peer_id} ended normally."));
            reason
        }
        Err(e) => {
            host_log(&events, format!("Session with {peer_id} error: {e}"));
            safe_session_end_reason(&e)
        }
    };
    // A runtime input lock belongs only to the session that requested it.
    // Never let it leak into the next incoming connection.
    if clear_active_session(session_id) {
        set_remote_input_blocked(false);
    }
    let resolved_peer_id = identity
        .lock()
        .ok()
        .map(|identity| identity.peer_id.clone())
        .unwrap_or(peer_id);
    let _ = events.send(HostEvent::SessionEnded {
        peer_id: resolved_peer_id,
        reason: end_reason,
        session_id,
    });
    let _ = events.send(HostEvent::StateChanged(HostState::Ready));
}

/// Returns a short debug string describing the current Windows logon session.
///
/// Shows the session name (Console / RDP-Tcp#N) so the host user can verify
/// that EvertyDesk is injecting input into the same session they are looking at.
fn windows_session_info() -> String {
    let session_name = std::env::var("SESSIONNAME").unwrap_or_else(|_| "Console".into());
    let username = std::env::var("USERNAME").unwrap_or_else(|_| "?".into());

    #[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
    {
        use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
        use windows::Win32::System::Threading::GetCurrentProcessId;
        let mut session_id: u32 = 0;
        let our_pid = unsafe { GetCurrentProcessId() };
        let ok = unsafe { ProcessIdToSessionId(our_pid, &mut session_id) }.as_bool();
        if ok {
            return format!(
                "PID={our_pid} session_id={session_id} SESSIONNAME={session_name} USER={username}"
            );
        }
    }

    format!("SESSIONNAME={session_name} USER={username}")
}

/// Android тачпад-хост: цикл приёма ввода без видео. Клиент (телефон) шлёт
/// Mouse/KeyEvent, мы инжектим их в службу доступности (EvertyInputService).
#[cfg(target_os = "android")]
fn android_touchpad_session(
    relay: &mut TcpStream,
    cipher: &mut Option<StreamCipher>,
    events: &Sender<HostEvent>,
    peer_id: &str,
    host_stop: &Arc<AtomicBool>,
) -> Result<(), String> {
    host_log(
        events,
        format!("Android тачпад-хост: сессия с {peer_id} (ввод-только, без видео) ✓"),
    );
    // Короткий таймаут чтения — чтобы регулярно проверять host_stop и слать keepalive.
    relay
        .set_read_timeout(Some(Duration::from_millis(1000)))
        .ok();
    let mut idle_ticks: u32 = 0;
    loop {
        if host_stop.load(Ordering::Relaxed) {
            host_log(
                events,
                format!("Android тачпад-хост: остановка сессии {peer_id}"),
            );
            return Ok(());
        }
        match recv_peer_enc(relay, cipher) {
            Ok(Some(msg)) => {
                idle_ticks = 0;
                match msg.union {
                    Some(peer_message::Union::MouseEvent(ev)) => inject_mouse(ev),
                    Some(peer_message::Union::KeyEvent(ev)) => inject_key(ev),
                    Some(peer_message::Union::TestDelay(d)) => {
                        let mut d = d;
                        d.from_client = false;
                        let reply = PeerMessage {
                            union: Some(peer_message::Union::TestDelay(d)),
                        };
                        let _ = send_peer_enc(relay, cipher, &reply);
                    }
                    _ => {}
                }
            }
            Ok(None) => {
                // keepalive от клиента — отвечаем пустым фреймом.
                relay
                    .write_all(&encode_frame_len(0)?)
                    .map_err(|e| e.to_string())?;
            }
            Err(e) => {
                if is_android_timeout(&e) {
                    idle_ticks += 1;
                    let _ = relay.write_all(&encode_frame_len(0)?);
                    if idle_ticks > 180 {
                        return Err("Android тачпад-хост: таймаут простоя (~3 мин)".to_owned());
                    }
                    continue;
                }
                return Err(format!("Android тачпад-хост: relay read: {e}"));
            }
        }
    }
}

/// EVRT2-only test session (host.rs's counterpart of `android_touchpad_session`
/// above): the client requested `evrt2_only` in its LoginRequest, so we skip
/// spawning `video_pipeline::run` entirely — no capture, no NVENC/encoder
/// loop — and go straight into the experimental EVRT2 capture→encode→UDP
/// loop instead. This is what makes the two never compete for the same
/// desktop-capture source or CPU/GPU encode time; see evrt2_experiment.rs's
/// doc comment for why that mattered when both used to run side by side.
fn run_evrt2_only_session(
    relay: &mut TcpStream,
    cipher: &mut Option<StreamCipher>,
    events: &Sender<HostEvent>,
    peer_id: &str,
    host_stop: &Arc<AtomicBool>,
) -> Result<(), String> {
    host_log(
        events,
        format!("EVRT2-only сессия с {peer_id}: живой EVRT1-пайплайн пропущен целиком ✓"),
    );
    clear_kick_request();

    // ROADMAP.md Phase 5.3 — RELAY_WRAP: the host side of the relay
    // candidate. `relay_inbound_tx` feeds bytes this thread unwraps from
    // incoming `Misc::Evrt2RelayWrap` into the race; `relay_outbound_rx`
    // (returned below) is what the writer thread further down drains and
    // wraps back out.
    let (relay_inbound_tx, relay_inbound_rx) = mpsc::channel::<Vec<u8>>();
    let (port, public_addr, auth_key, evrt2_stop, relay_outbound_rx) =
        crate::evrt2_experiment::start_host_experiment_with_relay_race(
            events.clone(),
            relay_inbound_rx,
        )
        .map_err(|e| format!("EVRT2-only: failed to bind UDP socket: {e}"))?;
    // ROADMAP.md Phase 5.1: WAN candidate appended after the LAN/VPN ones —
    // same "mini-ICE" list shape the EVRT1 UDP socket already announces.
    let mut endpoints = crate::netif::candidate_endpoints(port);
    if let Some(addr) = public_addr {
        endpoints = if endpoints.is_empty() {
            addr.to_string()
        } else {
            format!("{endpoints},{addr}")
        };
    }
    // Debug-only test hook (ROADMAP.md Phase 5.3 live verification): with
    // EVRTDESK_EVRT2_FORCE_RELAY=1, don't advertise any UDP candidates at
    // all — the client then has nothing to race except the relay leg,
    // exercising the real RELAY_WRAP path end-to-end over an actual
    // connection instead of only in the loopback unit tests. Not meant to
    // stay set for a normal session; the UDP socket itself still binds
    // normally (this only trims what's ANNOUNCED), so the host-side race
    // still races both — it just can't be won by UDP.
    let force_relay_only = std::env::var("EVRTDESK_EVRT2_FORCE_RELAY")
        .map(|v| v.trim() == "1")
        .unwrap_or(false);
    if force_relay_only {
        host_log(events, "EVRT2-only: EVRTDESK_EVRT2_FORCE_RELAY=1 — UDP-кандидаты не объявляются (тест RELAY_WRAP)".to_owned());
        endpoints.clear();
    }
    host_log(
        events,
        format!("EVRT2-only: сокет поднят на порту {port}, кандидаты: [{endpoints}] (+ relay-кандидат, Phase 5.3)"),
    );
    // ROADMAP.md Phase 4.2: the AuthTag session key travels the same TCP
    // relay channel EVRT1's own `evrt_session_token` already uses — see
    // `evrt2_crypto`'s module doc for the honest caveat on what that does
    // and doesn't defend against. Sent BEFORE the cipher is split below —
    // this is the last message on this thread that still uses the combined
    // `StreamCipher`.
    let endpoints_with_key = format!(
        "{endpoints},evrt2key={}",
        crate::evrt2_crypto::key_to_hex(&auth_key)
    );
    let _ = send_peer_enc(
        relay,
        cipher,
        &vm_misc_message(misc::Union::Evrt2ExperimentEndpoints(endpoints_with_key)),
    );

    // ROADMAP.md Phase 5.3: split the cipher (same into_halves()/try_clone()
    // pattern the live EVRT1 pipeline already uses for its own input-loop/
    // TCP-sender split) so a dedicated writer thread can forward relay-wrap
    // bytes AND keepalives independently of this thread's read loop, without
    // two threads ever calling `write_all` on the same socket concurrently.
    let (send_cipher, mut recv_cipher) = match cipher.take() {
        Some(c) => {
            let (s, r) = c.into_halves();
            (Some(s), Some(r))
        }
        None => (None, None),
    };
    let write_stream = relay
        .try_clone()
        .map_err(|e| format!("EVRT2-only: try_clone relay stream: {e}"))?;

    let writer_stop = evrt2_stop.clone();
    let writer_thread = thread::Builder::new()
        .name("evrt2-relay-writer".into())
        .spawn(move || {
            let mut stream = write_stream;
            let mut cipher = send_cipher;
            while !writer_stop.load(Ordering::Relaxed) {
                match relay_outbound_rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(raw) => {
                        let msg = vm_misc_message(misc::Union::Evrt2RelayWrap(raw));
                        if send_peer_sc(&mut stream, &mut cipher, &msg).is_err() {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Same keepalive rule as before (SDUDP.md § relay
                        // keepalive doc: ≥1/s or middleboxes drop the TCP
                        // connection after ~30s idle) — now owned solely by
                        // this thread so it never races the reader's socket
                        // clone or the relay-wrap bytes above.
                        if let Ok(frame) = encode_frame_len(0) {
                            if stream.write_all(&frame).is_err() {
                                break;
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .expect("spawn evrt2-relay-writer");

    relay
        .set_read_timeout(Some(Duration::from_millis(1000)))
        .ok();
    let result = loop {
        if host_stop.load(Ordering::Relaxed) {
            break Ok(());
        }
        if take_kick_request() {
            host_log(
                events,
                format!("EVRT2-only сессия {peer_id} остановлена хостом"),
            );
            break Ok(());
        }
        match recv_peer_rc(relay, &mut recv_cipher) {
            Ok(Some(msg)) => {
                // ROADMAP.md Phase 5.3: the only control traffic this
                // session cares about — everything else (mouse/keyboard) is
                // still ignored, pure video test, no input path.
                match &msg.union {
                    Some(peer_message::Union::Misc(m)) => match &m.union {
                        Some(misc::Union::Evrt2RelayWrap(bytes)) => {
                            let send_ok = relay_inbound_tx.send(bytes.clone()).is_ok();
                            host_log(events, format!(
                                "EVRT2-only: received Evrt2RelayWrap {} bytes, forwarded_ok={send_ok}",
                                bytes.len()
                            ));
                        }
                        other => {
                            host_log(
                                events,
                                format!(
                                    "EVRT2-only: received Misc, but not Evrt2RelayWrap: {other:?}"
                                ),
                            );
                        }
                    },
                    other => {
                        host_log(
                            events,
                            format!("EVRT2-only: received PeerMessage, not Misc: {other:?}"),
                        );
                    }
                }
            }
            Ok(None) => {} // peer's own empty keepalive frame — nothing to do
            Err(ref e) if is_timeout(e) => {} // no data right now; the writer thread keeps the connection alive on its own cadence
            Err(e) => break Err(format!("EVRT2-only: relay read: {e}")),
        }
    };
    evrt2_stop.store(true, Ordering::Relaxed);
    let _ = writer_thread.join();
    result
}

#[cfg(target_os = "android")]
fn is_android_timeout(err: &str) -> bool {
    err.contains("timed out")
        || err.contains("would block")
        || err.contains("WouldBlock")
        || err.contains("os error 11")
        || err.contains("os error 35")
}

fn relay_session_inner(
    config: &AppConfig,
    events: &Sender<HostEvent>,
    peer_id: &str,
    session_id: u64,
    session_identity: Arc<Mutex<RemoteIdentity>>,
    relay_server: &str,
    uuid: &str,
    host_stop: &Arc<AtomicBool>,
) -> Result<String, String> {
    if host_stop.load(Ordering::Relaxed) {
        return Ok("Приём подключений остановлен".to_owned());
    }
    host_log(events, format!("Session debug: {}", windows_session_info()));
    // ── 1. Connect to relay ───────────────────────────────────────────────────
    host_log(events, format!("Connecting to relay {relay_server}…"));
    let mut relay =
        connect_tcp(relay_server, RELAY_PORT).map_err(|e| format!("Relay connect: {e}"))?;

    // Identify ourselves to the relay — EXACTLY as RustDesk's
    // create_relay_connection_ does: only `licence_key` (server public key)
    // and `uuid`, everything else default/empty. Extra fields (id, secure)
    // break pairing on hbbr.
    let id_msg = RendezvousMessage {
        union: Some(rendezvous_message::Union::RequestRelay(RequestRelay {
            licence_key: config.server.public_key.clone(),
            uuid: uuid.to_owned(),
            id: String::new(),
            relay_server: String::new(),
            secure: false,
            conn_type: 0,
            token: String::new(),
            socket_addr: Vec::new(),
        })),
    };
    send_framed(&mut relay, &encode_message(&id_msg))?;
    host_log(events, "Relay identified.".to_owned());

    relay
        .set_read_timeout(Some(READ_TIMEOUT_AUTH))
        .map_err(|e| format!("set_read_timeout: {e}"))?;

    // ── 2. Secure handshake (RustDesk relay connections are always secure) ────
    // Sign IdPk{ id, ephemeral_box_pk } with our stable Ed25519 key and send it
    // as SignedId. The peer verifies it with our registered public key, seals a
    // symmetric key to our box pk, and replies PublicKey. From then on the
    // stream is secretbox-encrypted.
    let mut cipher: Option<StreamCipher> = None;
    {
        let (box_pk, box_sk) = crypto::gen_box_keypair();
        let id_pk = IdPk {
            id: config.local_id.clone(),
            pk: box_pk,
        };
        let signed = crypto::sign(&id_pk.encode_to_vec(), &config.host_sign_sk)
            .ok_or_else(|| "Failed to sign IdPk (bad sign secret key)".to_owned())?;
        let signed_id = PeerMessage {
            union: Some(peer_message::Union::SignedId(SignedId { id: signed })),
        };
        send_framed(&mut relay, &encode_peer_message(&signed_id))?;
        host_log(
            events,
            "Secure handshake: SignedId sent, awaiting PublicKey…".to_owned(),
        );

        // Read the peer's PublicKey (still plaintext). Skip keepalives.
        let mut tries = 0u32;
        loop {
            let payload = read_framed(&mut relay).map_err(|e| format!("Handshake read: {e}"))?;
            if payload.is_empty() {
                let _ = relay.write_all(&encode_frame_len(0)?);
                tries += 1;
                if tries > 200 {
                    return Err("Handshake: only keepalives, no PublicKey".to_owned());
                }
                continue;
            }
            let msg = decode_peer_message(&payload).map_err(|e| format!("Peer decode: {e}"))?;
            match msg.union {
                Some(peer_message::Union::PublicKey(pk)) => {
                    if pk.asymmetric_value.is_empty() {
                        host_log(
                            events,
                            "Peer chose insecure (empty PublicKey) — plaintext".to_owned(),
                        );
                    } else if let Some(symkey) = crypto::open_symmetric_key(
                        &pk.symmetric_value,
                        &pk.asymmetric_value,
                        &box_sk,
                    ) {
                        cipher = StreamCipher::new(&symkey);
                        host_log(events, "Secure channel established ✓".to_owned());
                    } else {
                        return Err("Failed to open peer symmetric key".to_owned());
                    }
                    break;
                }
                other => {
                    host_log(
                        events,
                        format!("Handshake: unexpected {}", peer_msg_kind(&other)),
                    );
                    tries += 1;
                    if tries > 20 {
                        return Err("Handshake: no PublicKey".to_owned());
                    }
                }
            }
        }
    }

    // ── 3. Auth: send Hash (encrypted), verify LoginRequest ───────────────────
    // Clear any stale approval state for this peer from a previous session.
    approvals().lock().unwrap().remove(peer_id);

    let salt = format!("{:016x}", random_u64());
    let challenge = format!("{:016x}", random_u64());

    let hash_msg = PeerMessage {
        union: Some(peer_message::Union::Hash(Hash {
            salt: salt.clone(),
            challenge: challenge.clone(),
        })),
    };
    send_peer_enc(&mut relay, &mut cipher, &hash_msg)?;
    host_log(
        events,
        format!("Hash challenge sent to {peer_id} — awaiting LoginRequest…"),
    );

    // Wait for a LoginRequest with a valid password. On empty/wrong password
    // we send the RustDesk-standard error ("Empty Password" / "Wrong Password")
    // and KEEP the connection open so the peer prompts and retries — closing
    // here is what made the phone show "wrong password" with no input box.
    //
    // Wall-clock deadline: even if keepalives keep arriving (resetting the
    // per-read socket timeout), we abort if no LoginRequest comes within 90s.
    let login_deadline = Instant::now() + Duration::from_secs(90);
    let mut others = 0u32;
    let mut pw_attempts = 0u32;
    let login = loop {
        if Instant::now() >= login_deadline {
            return Err(format!(
                "Login timeout: no LoginRequest from {peer_id} in 90s"
            ));
        }
        let msg = match recv_peer_enc(&mut relay, &mut cipher) {
            Ok(Some(m)) => m,
            Ok(None) => continue, // keepalive
            Err(e) => {
                return Err(format!("Relay read before login: {e}"));
            }
        };
        match msg.union {
            Some(peer_message::Union::LoginRequest(lr)) => {
                let identity = remote_identity_from_login(&lr);
                let request_peer_id = identity.peer_id.clone();
                if let Ok(mut current) = session_identity.lock() {
                    *current = identity.clone();
                }
                if request_peer_id != peer_id {
                    approvals().lock().unwrap().remove(&request_peer_id);
                }
                if lr.password.is_empty() {
                    if config.security.require_confirmation || lr.password.is_empty() {
                        host_log(
                            events,
                            format!("Approval requested for {request_peer_id} (empty password)"),
                        );
                        let _ = events.send(HostEvent::ApprovalRequested {
                            peer_id: request_peer_id.clone(),
                            peer_name: identity.peer_name.clone(),
                            platform: identity.platform.clone(),
                            version: identity.version.clone(),
                        });
                        match wait_for_approval(&request_peer_id, Duration::from_secs(45)) {
                            Some(true) => {
                                host_log(
                                    events,
                                    format!("Approved incoming connection from {request_peer_id}"),
                                );
                                break lr;
                            }
                            Some(false) => {
                                send_login_error(&mut relay, &mut cipher, "Rejected")?;
                                return Err("Incoming connection rejected".to_owned());
                            }
                            None => {
                                send_login_error(&mut relay, &mut cipher, "Timeout")?;
                                return Err("Incoming approval timed out".to_owned());
                            }
                        }
                    } else {
                        host_log(
                            events,
                            "Login probe (empty password) → requesting password".to_owned(),
                        );
                        send_login_error(&mut relay, &mut cipher, "Empty Password")?;
                        pw_attempts += 1;
                    }
                } else if verify_access_password(&lr.password, &config, &salt, &challenge) {
                    // ★ Верный пароль = автоматический доступ, без подтверждения.
                    //   ID+пароль достаточно. Подтверждение остаётся только для
                    //   подключений с ПУСТЫМ паролем (unattended режим выше).
                    host_log(
                        events,
                        format!("Password OK ✓ — авто-доступ для {request_peer_id}"),
                    );
                    break lr;
                } else {
                    host_log(events, "Wrong password → asking to retry".to_owned());
                    send_login_error(&mut relay, &mut cipher, "Wrong Password")?;
                    pw_attempts += 1;
                }
                if pw_attempts > 10 {
                    return Err("Too many wrong-password attempts".to_owned());
                }
            }
            Some(peer_message::Union::TestDelay(d)) => {
                // Echo test delay back so the client's RTT probe completes.
                let mut d = d;
                d.from_client = false;
                let reply = PeerMessage {
                    union: Some(peer_message::Union::TestDelay(d)),
                };
                host_log(events, format!("TestDelay echoed for {peer_id}"));
                let _ = send_peer_enc(&mut relay, &mut cipher, &reply);
            }
            other => {
                others += 1;
                host_log(
                    events,
                    format!(
                        "Pre-login msg #{others} from {peer_id}: {}",
                        peer_msg_kind(&other)
                    ),
                );
                if others > 40 {
                    return Err("Too many non-login messages".to_owned());
                }
            }
        }
    };
    let identity = session_identity
        .lock()
        .ok()
        .map(|identity| identity.clone())
        .unwrap_or_else(RemoteIdentity::unknown);
    let peer_id = identity.peer_id.as_str();

    // ── 4. Send LoginResponse + display info ─────────────────────────────────
    let hostname = hostname();
    let capture_displays = crate::capture::display_infos();
    for display in &capture_displays {
        host_log(
            events,
            format!(
                "Display list: idx={} name={} x={} y={} width={} height={} source=gdi/dxgi",
                display.index, display.name, display.x, display.y, display.width, display.height
            ),
        );
    }
    let mut displays: Vec<DisplayInfo> = capture_displays
        .into_iter()
        .map(|display| DisplayInfo {
            x: display.x,
            y: display.y,
            width: display.width,
            height: display.height,
            name: if display.name.is_empty() {
                format!("Display {}", display.index + 1)
            } else {
                display.name
            },
            online: true,
            cursor_embedded: false,
            scale: 1.0,
        })
        .collect();
    if displays.is_empty() {
        let (screen_w, screen_h) = crate::capture::screen_size().unwrap_or((1920, 1080));
        displays.push(DisplayInfo {
            x: 0,
            y: 0,
            width: screen_w as i32,
            height: screen_h as i32,
            name: hostname.clone(),
            online: true,
            cursor_embedded: false,
            scale: 1.0,
        });
    }
    host_log(
        events,
        format!("Host displays announced: {}", displays.len()),
    );
    let peer_info = PeerInfo {
        username: String::new(),
        hostname: hostname.clone(),
        platform: std::env::consts::OS.to_owned(),
        version: "1.4.6".to_owned(),
        current_display: 0,
        displays,
        windows_sessions: None,
    };
    let login_ok = PeerMessage {
        union: Some(peer_message::Union::LoginResponse(LoginResponse {
            union: Some(login_response::Union::PeerInfo(peer_info)),
        })),
    };
    send_peer_enc(&mut relay, &mut cipher, &login_ok)?;

    // ── EVRT2-only тест: клиент попросил не поднимать живой EVRT1-пайплайн ───
    if login.evrt2_only {
        return run_evrt2_only_session(&mut relay, &mut cipher, events, peer_id, host_stop)
            .map(|()| "EVRT2-сессия завершена".to_owned());
    }

    // ── Android-хост: тачпад-режим (ввод-только, без видео) ──────────────────
    // Пользователь смотрит прямо на экран Android/ТВ, телефон — слепой трекпад.
    // Захвата/энкодера на Android нет; принимаем Mouse/KeyEvent и инжектим их в
    // службу доступности. Видео-пайплайн ниже не запускаем.
    #[cfg(target_os = "android")]
    {
        return android_touchpad_session(&mut relay, &mut cipher, events, peer_id, host_stop)
            .map(|()| "Сеанс управления завершён".to_owned());
    }

    // ── 4а. EVRT: открываем выделенный UDP сокет и сообщаем порт клиенту ─────
    //
    // Отдельный сокет — не hbbs-порт. Это решает проблему конкуренции:
    //   hbbs-сокет: только heartbeats/регистрация
    //   evrt-сокет: только видео/feedback
    //
    // Клиент получает порт → punch-hole → EVRT сессия.
    // Если UDP не поднялся за 2 сек — клиент остаётся на TCP relay.
    let evrt_socket = try_open_evrt_socket(config, events);
    let mut evrt_announce: Option<(String, u16)> = None;
    let mut evrt_token: Option<String> = None;

    if let Some((ref sock, evrt_port)) = evrt_socket {
        let token = new_evrt_session_token();
        // ★ Перечисляем ВСЕ локальные IP (LAN + VPN) как кандидаты — mini-ICE.
        //   Это решает мультихоминг: через VPN клиент достучится по VPN-IP хоста.
        let mut endpoints = crate::netif::candidate_endpoints(evrt_port);

        // ★ STUN: публичный адрес как последний кандидат — решает "клиент не в
        //   LAN хоста" (мобильный интернет, другая сеть). ОБЯЗАТЕЛЬНО через тот
        //   же сокет, что откроет EVRT-сессию — NAT-маппинг per-socket, другой
        //   сокет узнает бесполезный порт. Best-effort: symmetric NAT или
        //   недоступность STUN-серверов оставляет endpoints как раньше (только
        //   LAN/VPN), клиент падает на TCP relay — как и было до этого фикса,
        //   никакой регрессии на неудаче.
        match crate::netif::discover_public_endpoint(sock) {
            Some(public_addr) => {
                host_log(events, format!("EVRT: STUN обнаружил публичный адрес {public_addr}"));
                endpoints = if endpoints.is_empty() {
                    public_addr.to_string()
                } else {
                    format!("{endpoints},{public_addr}")
                };
            }
            None => host_log(
                events,
                "EVRT: STUN не смог определить публичный адрес (WAN-подключения вне LAN будут падать на TCP relay)".to_owned(),
            ),
        }

        let endpoints_with_token = append_evrt_session_token(&endpoints, &token);
        evrt_token = Some(token);
        evrt_announce = Some((endpoints_with_token.clone(), evrt_port));

        if !endpoints_with_token.is_empty() {
            let misc = evrt_endpoints_message(&endpoints_with_token);
            match send_peer_enc(&mut relay, &mut cipher, &misc) {
                Ok(()) => host_log(
                    events,
                    format!("EVRT: Misc{{EvrtEndpoints=[{endpoints}]}} sent → клиент"),
                ),
                Err(e) => host_log(events, format!("EVRT: Misc endpoints send failed: {e}")),
            }
        }

        // Дублируем старый EvrtUdpPort для обратной совместимости (если у клиента
        // окажется punch-hole IP от hbbs).
        let misc_port = evrt_port_message(evrt_port);
        let _ = send_peer_enc(&mut relay, &mut cipher, &misc_port);
    }

    // EVRT game mode: allow up to 60fps regardless of host's configured limit (default 30).
    let fps_limit = if evrt_socket.is_some() {
        MAX_TARGET_FPS
    } else {
        config.display.target_fps
    };
    let target_fps = negotiated_target_fps(&login, fps_limit);
    let quality_milli = negotiated_quality_milli(&login);
    let client_video = client_video_support(&login);

    host_log(
        events,
        format!(
            "Auth OK для {peer_id}. Pipeline старт: {target_fps}fps quality={}%",
            quality_milli / 10,
        ),
    );
    // Defensive reset in case a previous session ended abnormally before its
    // launcher could send the matching unblock command.
    set_remote_input_blocked(false);
    set_remote_clipboard_allowed(config.security.allow_clipboard);
    set_active_session(session_id, peer_id.to_owned());
    let _ = events.send(HostEvent::SessionStarted {
        peer_id: peer_id.to_owned(),
        session_id,
        peer_name: identity.peer_name.clone(),
        platform: identity.platform.clone(),
        version: identity.version.clone(),
    });

    // ── Единый пайплайн: один захват → один энкодер → TCP + UDP ──────────────
    //
    // Заменяет старую схему двух параллельных систем (video_loop + evrt_session).
    // Один MF энкодер, нет конкуренции, нет двойного захвата.
    let (send_cipher, mut recv_cipher) = match cipher.take() {
        Some(c) => {
            let (s, r) = c.into_halves();
            (Some(s), Some(r))
        }
        None => (None, None),
    };

    let write_stream = relay
        .try_clone()
        .map_err(|e| format!("try_clone relay stream: {e}"))?;

    // Канал команд: input loop → pipeline
    let (cmd_tx, cmd_rx) = mpsc::channel::<crate::video_pipeline::PipelineCmd>();
    // Канал исходящих PeerMessage (shell output, etc.)
    let (peer_msg_tx, peer_msg_rx) = mpsc::channel::<PeerMessage>();

    // recv_cipher НЕ идёт в pipeline — остаётся в input loop этого треда
    // для расшифровки MouseEvent/KeyEvent от клиента.
    let pipeline_cfg = crate::video_pipeline::PipelineConfig {
        app_config: config.clone(),
        peer_id: peer_id.to_owned(),
        events: events.clone(),
        client_video,
        initial_target_fps: target_fps,
        initial_quality_milli: quality_milli,
        relay_stream: write_stream,
        send_cipher,
        evrt_socket: evrt_socket.map(|(s, _)| s),
        evrt_token,
        cmd_rx,
        peer_msg_rx,
    };

    let pipeline_stop = Arc::new(AtomicBool::new(false));
    let pipeline_stop_v = pipeline_stop.clone();
    let pipeline_handle = thread::spawn(move || {
        crate::video_pipeline::run(pipeline_cfg);
        pipeline_stop_v.store(true, Ordering::Relaxed);
    });

    if let Some((endpoints, evrt_port)) = evrt_announce {
        repeat_evrt_announcement(peer_msg_tx.clone(), events.clone(), endpoints, evrt_port);
    }

    // Shared FPS/quality для input loop → pipeline commands
    let shared_target_fps = Arc::new(AtomicU32::new(target_fps));
    let shared_quality_milli = Arc::new(AtomicU32::new(quality_milli));
    let stop = pipeline_stop.clone();

    // ── Input loop (этот тред) — читает TCP, шлёт команды в pipeline ─────────
    let _ = relay.set_read_timeout(Some(Duration::from_secs(1)));
    // recv_cipher здесь — используется для расшифровки входящих
    // MouseEvent/KeyEvent от клиента. send_cipher ушёл в pipeline.
    let mut shell: Option<ShellRuntime> = None;
    let (_peer_out_tx, _peer_out_rx) = mpsc::channel::<PeerMessage>();

    // Fresh session — drop any stale "kick" request from a previous connection.
    clear_kick_request();

    let mut end_reason = None;
    while !stop.load(Ordering::Relaxed) && !host_stop.load(Ordering::Relaxed) {
        // UI pressed "Отключить" — tear the session down like a normal disconnect.
        if take_kick_request() {
            host_log(events, format!("Session with {peer_id} terminated by host"));
            end_reason = Some("Отключено владельцем устройства".to_owned());
            break;
        }
        // Every loop iteration, not just on MouseEvent: the read timeout
        // below forces at least one iteration per second regardless of
        // whether the remote client is actually moving its mouse right now,
        // so the elevation hint fires reliably as soon as an elevated
        // window (e.g. Task Manager) takes the foreground — not only after
        // someone happens to send a mouse event while it's focused.
        #[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
        elevation_hint::check(events);
        match recv_peer_rc(&mut relay, &mut recv_cipher) {
            Ok(Some(msg)) => handle_client_input_pipeline(
                msg,
                &cmd_tx,
                &peer_msg_tx,
                events,
                &mut shell,
                &shared_target_fps,
                &shared_quality_milli,
                is_remote_clipboard_allowed(),
            ),
            Ok(None) => {}
            Err(ref e) if is_timeout(e) => {}
            Err(error) => {
                end_reason = Some(format!(
                    "Соединение потеряно: {}",
                    safe_session_end_reason(&error.to_string())
                ));
                break;
            }
        }
    }
    let end_reason = end_reason.unwrap_or_else(|| {
        if host_stop.load(Ordering::Relaxed) {
            "Приём подключений остановлен".to_owned()
        } else if stop.load(Ordering::Relaxed) {
            "Видеоконвейер завершён".to_owned()
        } else {
            "Клиент отключился".to_owned()
        }
    });

    // Сигнал остановки
    let _ = cmd_tx.send(crate::video_pipeline::PipelineCmd::Stop);
    let _ = relay.shutdown(std::net::Shutdown::Both);

    if let Some(mut shell) = shell.take() {
        shell.stop();
    }

    drop(peer_msg_tx);
    drop(cmd_tx);

    // join pipeline с таймаутом 3 сек
    {
        let (done_tx, done_rx) = mpsc::channel::<()>();
        thread::spawn(move || {
            let _ = pipeline_handle.join();
            let _ = done_tx.send(());
        });
        match done_rx.recv_timeout(Duration::from_secs(15)) {
            Ok(()) => host_log(events, format!("Pipeline join complete for {peer_id}")),
            Err(_) => {
                host_log(events, format!("Pipeline join timeout for {peer_id}"));
                eprintln!("[host] pipeline join timeout — detaching");
            }
        }
    }

    // Release input out-of-band. On Windows this calls SendInput; it must not
    // keep the relay session or process shutdown on the critical path.
    let cleanup_events = events.clone();
    let cleanup_peer = peer_id.to_owned();
    thread::spawn(move || {
        release_stuck_input();
        host_log(
            &cleanup_events,
            format!("Relay session input cleanup complete for {cleanup_peer}"),
        );
    });
    host_log(
        events,
        format!("Relay session cleanup complete for {peer_id}"),
    );

    Ok(end_reason)
}

fn negotiated_target_fps(login: &crate::rustdesk_proto::LoginRequest, configured_fps: u32) -> u32 {
    let host_limit = configured_fps.clamp(5, MAX_TARGET_FPS);
    let requested = login
        .option
        .as_ref()
        .map(|option| option.custom_fps)
        .filter(|fps| *fps > 0)
        .unwrap_or(host_limit as i32)
        .clamp(5, MAX_TARGET_FPS as i32) as u32;
    requested.min(host_limit)
}

fn negotiated_quality_milli(login: &crate::rustdesk_proto::LoginRequest) -> u32 {
    login
        .option
        .as_ref()
        .and_then(option_quality_milli)
        .unwrap_or(DEFAULT_QUALITY_MILLI)
}

fn option_quality_milli(option: &crate::rustdesk_proto::OptionMessage) -> Option<u32> {
    match ImageQuality::try_from(option.image_quality).unwrap_or(ImageQuality::NotSet) {
        ImageQuality::Best => Some(BEST_QUALITY_MILLI),
        ImageQuality::Balanced => Some(BALANCED_QUALITY_MILLI),
        ImageQuality::Low => Some(LOW_QUALITY_MILLI),
        ImageQuality::NotSet if option.custom_image_quality > 0 => {
            let raw = ((option.custom_image_quality >> 8) & 0x0fff) as u32;
            (raw > 0).then(|| (raw * 20).clamp(LOW_QUALITY_MILLI, 4_000))
        }
        ImageQuality::NotSet => None,
    }
}

fn wait_for_approval(peer_id: &str, timeout: Duration) -> Option<bool> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(mut approvals) = approvals().lock() {
            if let Some(decision) = approvals.remove(peer_id) {
                return Some(decision);
            }
        }
        thread::sleep(Duration::from_millis(150));
    }
    None
}

#[allow(dead_code)] // orphaned: telemetry от старого video_loop
fn frame_budget(target_fps: u32) -> Duration {
    let fps = target_fps.clamp(5, MAX_TARGET_FPS) as u64;
    Duration::from_micros(1_000_000 / fps)
}

#[allow(dead_code)] // orphaned: telemetry от старого video_loop
fn avg_ms(total: u64, count: u64) -> u64 {
    if count == 0 {
        0
    } else {
        total / count
    }
}

#[allow(dead_code)] // orphaned: telemetry от старого video_loop
fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg_attr(not(feature = "live-h264"), allow(dead_code))]
fn h264_target_bitrate_bps(width: u32, height: u32, fps: u32, quality_milli: u32) -> u32 {
    const MIN_BPS: u64 = 800_000;
    const DEFAULT_MAX_BPS: u64 = 14_000_000;
    const PERFORMANCE_MAX_BPS: u64 = 40_000_000;
    const SCREEN_CONTENT_MILLI_BPP: u64 = 55;

    let pixels = u64::from(width.max(1)) * u64::from(height.max(1));
    let fps = u64::from(fps.clamp(5, MAX_TARGET_FPS));
    let quality = u64::from(quality_milli.clamp(LOW_QUALITY_MILLI, 4_000));
    let max_bps = if quality_milli > DEFAULT_QUALITY_MILLI {
        PERFORMANCE_MAX_BPS
    } else {
        DEFAULT_MAX_BPS
    };
    ((pixels * fps * SCREEN_CONTENT_MILLI_BPP * quality) / 1_000_000).clamp(MIN_BPS, max_bps) as u32
}

/// Детектор изменений кадра — пропускает статичные кадры (экономия трафика).
/// Используется единым video_pipeline.
#[derive(Default)]
pub struct FrameChangeDetector {
    width: u32,
    height: u32,
    /// FNV-1a hash of the last sent frame (computed by crate::colorconv::frame_signature).
    last_hash: u64,
    last_tile_cols: u32,
    last_tile_rows: u32,
    last_tile_hashes: Vec<u64>,
    pending_fingerprint: Option<FrameFingerprint>,
    last_sent_at: Option<Instant>,
    consecutive_static_skips: u32,
    sent_since_log: u64,
    skipped_static_since_log: u64,
}

pub struct FrameDecision {
    pub send: bool,
    pub force_key: bool,
    pub roi: crate::evrt::RoiRect,
}

struct FrameFingerprint {
    width: u32,
    height: u32,
    hash: u64,
    tile_cols: u32,
    tile_rows: u32,
    tile_hashes: Vec<u64>,
}

#[derive(Clone, Copy, PartialEq)]
pub struct ClientVideoSupport {
    pub h264: bool,
    pub h265: bool,
    pub av1: bool,
    pub prefer: PreferCodec,
}

impl Default for ClientVideoSupport {
    fn default() -> Self {
        Self {
            h264: true,
            h265: false,
            av1: false,
            prefer: PreferCodec::H264,
        }
    }
}

pub struct EncodedPacket {
    pub bytes: Vec<u8>,
    pub key: bool,
}

impl FrameChangeDetector {
    pub fn decide(
        &mut self,
        width: u32,
        height: u32,
        bgra: &[u8],
        periodic_key: bool,
    ) -> FrameDecision {
        const STATIC_REFRESH: Duration = Duration::from_secs(2);

        let size_changed = self.width != width || self.height != height;
        let current = FrameFingerprint::build(width, height, bgra);
        let idle_refresh = self
            .last_sent_at
            .map(|instant| instant.elapsed() >= STATIC_REFRESH)
            .unwrap_or(true);
        let changed = size_changed || self.frame_changed(&current);
        let send = changed || periodic_key || idle_refresh;
        let roi = if size_changed || self.last_hash == 0 || periodic_key || idle_refresh {
            full_screen_roi()
        } else if changed {
            self.dirty_roi(&current).unwrap_or_else(full_screen_roi)
        } else {
            full_screen_roi()
        };
        self.pending_fingerprint = Some(current);
        if send {
            FrameDecision {
                send: true,
                force_key: size_changed || periodic_key || idle_refresh,
                roi,
            }
        } else {
            self.consecutive_static_skips = self.consecutive_static_skips.saturating_add(1);
            self.skipped_static_since_log = self.skipped_static_since_log.saturating_add(1);
            FrameDecision {
                send: false,
                force_key: false,
                roi,
            }
        }
    }

    pub fn mark_sent(&mut self, width: u32, height: u32, bgra: &[u8]) {
        self.width = width;
        self.height = height;
        let fingerprint = self
            .pending_fingerprint
            .take()
            .filter(|fp| fp.width == width && fp.height == height)
            .unwrap_or_else(|| FrameFingerprint::build(width, height, bgra));
        self.last_hash = fingerprint.hash;
        self.last_tile_cols = fingerprint.tile_cols;
        self.last_tile_rows = fingerprint.tile_rows;
        self.last_tile_hashes = fingerprint.tile_hashes;
        self.last_sent_at = Some(Instant::now());
        self.consecutive_static_skips = 0;
        self.sent_since_log = self.sent_since_log.saturating_add(1);
    }

    fn frame_changed(&self, current: &FrameFingerprint) -> bool {
        if self.last_hash == 0 {
            return true;
        }
        current.hash != self.last_hash
            || current.tile_cols != self.last_tile_cols
            || current.tile_rows != self.last_tile_rows
            || current.tile_hashes != self.last_tile_hashes
    }

    fn dirty_roi(&self, current: &FrameFingerprint) -> Option<crate::evrt::RoiRect> {
        if current.width != self.width
            || current.height != self.height
            || current.tile_cols != self.last_tile_cols
            || current.tile_rows != self.last_tile_rows
            || current.tile_hashes.len() != self.last_tile_hashes.len()
        {
            return Some(full_screen_roi());
        }

        let mut min_col = current.tile_cols;
        let mut min_row = current.tile_rows;
        let mut max_col = 0_u32;
        let mut max_row = 0_u32;
        let mut any = false;

        for row in 0..current.tile_rows {
            for col in 0..current.tile_cols {
                let idx = (row * current.tile_cols + col) as usize;
                if current.tile_hashes[idx] != self.last_tile_hashes[idx] {
                    any = true;
                    min_col = min_col.min(col);
                    min_row = min_row.min(row);
                    max_col = max_col.max(col);
                    max_row = max_row.max(row);
                }
            }
        }

        if !any {
            return None;
        }

        let x = min_col * DIRTY_TILE_SIZE;
        let y = min_row * DIRTY_TILE_SIZE;
        let right = ((max_col + 1) * DIRTY_TILE_SIZE).min(current.width);
        let bottom = ((max_row + 1) * DIRTY_TILE_SIZE).min(current.height);
        Some(crate::evrt::RoiRect {
            frame_id: 0,
            x,
            y,
            w: right.saturating_sub(x),
            h: bottom.saturating_sub(y),
        })
    }

    pub fn static_backoff_delay(&self, fps: u32) -> Option<Duration> {
        let fps = fps.clamp(5, MAX_TARGET_FPS);
        if self.consecutive_static_skips < fps {
            None
        } else if self.consecutive_static_skips < fps.saturating_mul(4) {
            Some(Duration::from_millis(25))
        } else {
            Some(Duration::from_millis(50))
        }
    }
}

const DIRTY_TILE_SIZE: u32 = 32;
const TILE_HASH_TARGET_SAMPLES: usize = 16;
const FNV_OFFSET: u64 = 14695981039346656037;
const FNV_PRIME: u64 = 1099511628211;

impl FrameFingerprint {
    fn build(width: u32, height: u32, bgra: &[u8]) -> Self {
        let hash = crate::colorconv::frame_signature(bgra, width as usize, height as usize);
        if width == 0 || height == 0 || bgra.len() < width as usize * height as usize * 4 {
            return Self {
                width,
                height,
                hash,
                tile_cols: 0,
                tile_rows: 0,
                tile_hashes: Vec::new(),
            };
        }

        let tile_cols = div_ceil_u32(width, DIRTY_TILE_SIZE);
        let tile_rows = div_ceil_u32(height, DIRTY_TILE_SIZE);
        let mut tile_hashes = Vec::with_capacity(tile_cols as usize * tile_rows as usize);
        for row in 0..tile_rows {
            for col in 0..tile_cols {
                let x0 = col * DIRTY_TILE_SIZE;
                let y0 = row * DIRTY_TILE_SIZE;
                let x1 = ((col + 1) * DIRTY_TILE_SIZE).min(width);
                let y1 = ((row + 1) * DIRTY_TILE_SIZE).min(height);
                tile_hashes.push(tile_hash(width, bgra, x0, y0, x1, y1));
            }
        }

        Self {
            width,
            height,
            hash,
            tile_cols,
            tile_rows,
            tile_hashes,
        }
    }
}

fn tile_hash(width: u32, bgra: &[u8], x0: u32, y0: u32, x1: u32, y1: u32) -> u64 {
    let tile_w = x1.saturating_sub(x0).max(1);
    let tile_h = y1.saturating_sub(y0).max(1);
    let pixels = tile_w as usize * tile_h as usize;
    let step = (pixels / TILE_HASH_TARGET_SAMPLES).max(1);

    let mut hash = FNV_OFFSET;
    let mut local = 0usize;
    while local < pixels {
        let px = local as u32;
        let x = x0 + px % tile_w;
        let y = y0 + px / tile_w;
        let base = (y as usize * width as usize + x as usize) * 4;
        if base + 2 < bgra.len() {
            let b = bgra[base] as u64;
            let g = bgra[base + 1] as u64;
            let r = bgra[base + 2] as u64;
            hash = hash.wrapping_mul(FNV_PRIME) ^ r;
            hash = hash.wrapping_mul(FNV_PRIME) ^ g;
            hash = hash.wrapping_mul(FNV_PRIME) ^ b;
        }
        local += step;
    }
    hash
}

fn div_ceil_u32(value: u32, divisor: u32) -> u32 {
    if value == 0 {
        0
    } else {
        1 + (value - 1) / divisor.max(1)
    }
}

fn full_screen_roi() -> crate::evrt::RoiRect {
    crate::evrt::RoiRect {
        frame_id: 0,
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    }
}

fn client_video_support(login: &crate::rustdesk_proto::LoginRequest) -> ClientVideoSupport {
    let Some(SupportedDecoding {
        ability_h264,
        ability_h265,
        ability_av1,
        prefer,
        ..
    }) = login
        .option
        .as_ref()
        .and_then(|option| option.supported_decoding.as_ref())
    else {
        return ClientVideoSupport::default();
    };

    ClientVideoSupport {
        h264: *ability_h264 > 0,
        h265: *ability_h265 > 0,
        av1: *ability_av1 > 0,
        prefer: PreferCodec::try_from(*prefer).unwrap_or(PreferCodec::Auto),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Codec negotiation — pick the best codec both ends can actually handle
// ═══════════════════════════════════════════════════════════════════════════════
//
// Every backend (NVENC / Media Foundation / VideoToolbox) feeds its real, live
// capabilities into one ranking brain instead of each hardcoding "H264 first".
// The cascade in `MultiEncoder::encode` then tries the fastest backend first,
// and because every backend independently maximises the *same* quality-ranked,
// capability-gated score, the session latches onto the best codec available
// end-to-end — without ever risking an unusable path.

/// Compression quality at equal bitrate — higher is better. AV1 ≻ H265 ≻ H264.
/// (VP9 would slot at 2 once an encoder exists; it has no `NvencCodec` variant.)
fn codec_quality_rank(codec: crate::nvenc::NvencCodec) -> u32 {
    match codec {
        crate::nvenc::NvencCodec::Av1 => 4,
        crate::nvenc::NvencCodec::H265 => 3,
        crate::nvenc::NvencCodec::H264 => 1,
    }
}

/// Does the client's advertised `prefer` hint name this exact codec?
fn client_prefers_codec(prefer: PreferCodec, codec: crate::nvenc::NvencCodec) -> bool {
    matches!(
        (prefer, codec),
        (PreferCodec::Av1, crate::nvenc::NvencCodec::Av1)
            | (PreferCodec::H265, crate::nvenc::NvencCodec::H265)
            | (PreferCodec::H264, crate::nvenc::NvencCodec::H264)
    )
}

/// Translate the user's explicit codec choice into a target, or `None` for the
/// fully-automatic path. VP9 has no `NvencCodec` encoder, so it negotiates like
/// Auto (and the VP9 stream path is handled separately when present).
fn explicit_codec_target(codec_preference: CodecPreference) -> Option<crate::nvenc::NvencCodec> {
    match codec_preference {
        CodecPreference::H264 => Some(crate::nvenc::NvencCodec::H264),
        CodecPreference::H265 => Some(crate::nvenc::NvencCodec::H265),
        CodecPreference::Av1 => Some(crate::nvenc::NvencCodec::Av1),
        CodecPreference::Vp9 | CodecPreference::Auto | CodecPreference::Evrtck => None,
    }
}

/// Pick the codec a single backend should target, given the codecs it can
/// produce right now (`available`, each tagged `hardware`), the user's choice,
/// and what the client can decode.
///
/// The clever conditions, in order:
///   1. **Decodability** — never target a codec the client cannot decode.
///   2. **Hardware-only high-end** — H265/AV1 only when hardware-encoded;
///      software realtime HEVC/AV1 is unusable (≈200 ms/frame), so it's filtered
///      out entirely. This is what makes Auto *both* safe and optimal without a
///      blanket "H264 first" rule.
/// Authority order, highest first:
///   3. **Host user's explicit choice** — a non-Auto pick dominates when
///      achievable, with H264 kept as a universal safety net so a session
///      always streams.
///   4. **Client's `prefer` hint** — when the client names a concrete codec
///      (e.g. it *can* decode HEVC but its decoder is software and it would
///      rather receive H264), honour it over raw quality ranking.
///   5. **Quality ranking** — when both ends are on Auto, take the best codec
///      (AV1 ≻ H265 ≻ H264), tie-broken by hardware acceleration.
fn negotiate_backend_codec(
    available: &[(crate::nvenc::NvencCodec, bool)],
    encoder_preference: EncoderPreference,
    codec_preference: CodecPreference,
    client: ClientVideoSupport,
) -> Option<crate::nvenc::NvencCodec> {
    if encoder_preference == EncoderPreference::Software || available.is_empty() {
        return None;
    }
    let explicit = explicit_codec_target(codec_preference);

    available
        .iter()
        .copied()
        // 1. client must be able to decode it
        .filter(|(codec, _)| client_can_decode_hardware(client, *codec))
        // 2. high-end codecs only with hardware encode
        .filter(|(codec, hardware)| *hardware || *codec == crate::nvenc::NvencCodec::H264)
        // 3. explicit choice narrows to that codec, keeping H264 as the net
        .filter(|(codec, _)| {
            explicit.map_or(true, |want| {
                *codec == want || *codec == crate::nvenc::NvencCodec::H264
            })
        })
        .max_by_key(|(codec, hardware)| {
            let mut score = codec_quality_rank(*codec) * 1_000;
            if Some(*codec) == explicit {
                score += 1_000_000; // host's explicit pick beats everything
            }
            if client_prefers_codec(client.prefer, *codec) {
                score += 10_000; // client's concrete prefer overrides raw quality
            }
            if *hardware {
                score += 200; // tie-break toward hardware at equal codec
            }
            score
        })
        .map(|(codec, _)| codec)
}

/// Какой кодек РЕАЛЬНО будет кодировать этот хост для данного клиента.
///
/// Повторяет порядок каскада `MultiEncoder::encode` (NVENC → Media Foundation →
/// VideoToolbox), поэтому даёт тот же ответ, что и сам энкодер, но БЕЗ создания
/// энкодера — нужно до старта захвата, чтобы объявить клиенту правду в первом же
/// `SessionConfig`.
///
/// `None` означает, что аппаратного бэкенда нет вовсе и каскад упадёт на
/// программный OpenH264, то есть на проводе будет H264.
///
/// Зачем это отдельной функцией: `SessionConfig.codec` определяет, КАКОЙ декодер
/// откроет клиент ещё до первого кадра. Объявить H265 и прислать H264 — не
/// «неточность», а гарантированный жёсткий крах клиентского процесса: системный
/// `HEVCDECODER_STORE.dll` падает с 0xC0000005, получив чужой битстрим (задача
/// #39, воспроизведено дважды). Механизм `TYPE_CODEC_CONFIG` такое расхождение
/// потом исправляет, но он едет по ДРУГОМУ каналу, чем кадры, и успевает не
/// всегда.
pub fn negotiated_session_codec(
    encoder_preference: EncoderPreference,
    codec_preference: CodecPreference,
    client: ClientVideoSupport,
) -> Option<crate::nvenc::NvencCodec> {
    choose_nvenc_codec(encoder_preference, codec_preference, client)
        .or_else(|| choose_onevpl_codec(encoder_preference, codec_preference, client))
        .or_else(|| choose_mf_encoder_codec(encoder_preference, codec_preference, client))
        .or_else(|| choose_videotoolbox_codec(encoder_preference, codec_preference, client))
}

fn choose_nvenc_codec(
    encoder_preference: EncoderPreference,
    codec_preference: CodecPreference,
    client: ClientVideoSupport,
) -> Option<crate::nvenc::NvencCodec> {
    // Every NVENC-reported codec is hardware-accelerated.
    let available: Vec<_> = crate::nvenc::nvenc_encoder_codecs()
        .into_iter()
        .map(|codec| (codec, true))
        .collect();
    negotiate_backend_codec(&available, encoder_preference, codec_preference, client)
}

/// Intel oneVPL / Media SDK. Всё, что он сообщает, аппаратное по определению:
/// сессия открывается с `MFX_IMPL_HARDWARE_ANY`, программную мы намеренно не
/// запрашиваем — она была бы медленнее уже имеющегося OpenH264.
fn choose_onevpl_codec(
    encoder_preference: EncoderPreference,
    codec_preference: CodecPreference,
    client: ClientVideoSupport,
) -> Option<crate::nvenc::NvencCodec> {
    let available: Vec<_> = crate::onevpl::onevpl_encoder_codecs()
        .into_iter()
        .map(|codec| (codec, true))
        .collect();
    negotiate_backend_codec(&available, encoder_preference, codec_preference, client)
}

fn choose_mf_encoder_codec(
    encoder_preference: EncoderPreference,
    codec_preference: CodecPreference,
    client: ClientVideoSupport,
) -> Option<crate::nvenc::NvencCodec> {
    // Media Foundation may expose software-only MFTs; tag each codec with its
    // real hardware flag so gate #2 can drop slow software HEVC.
    let status = crate::mf_encode::mf_encoder_status();
    let available: Vec<_> = crate::mf_encode::mf_encoder_codecs()
        .into_iter()
        .map(|codec| {
            let hardware = match codec {
                crate::nvenc::NvencCodec::H264 => status.has_hardware_h264(),
                crate::nvenc::NvencCodec::H265 => status.has_hardware_h265(),
                crate::nvenc::NvencCodec::Av1 => status.has_hardware_av1(),
            };
            (codec, hardware)
        })
        .collect();
    negotiate_backend_codec(&available, encoder_preference, codec_preference, client)
}

fn choose_videotoolbox_codec(
    encoder_preference: EncoderPreference,
    codec_preference: CodecPreference,
    client: ClientVideoSupport,
) -> Option<crate::nvenc::NvencCodec> {
    // VideoToolbox encode is hardware-backed (Apple Silicon media engine /
    // Intel Quick Sync).
    let available: Vec<_> = crate::videotoolbox::videotoolbox_codecs()
        .into_iter()
        .map(|codec| (codec, true))
        .collect();
    negotiate_backend_codec(&available, encoder_preference, codec_preference, client)
}

fn client_can_decode_hardware(client: ClientVideoSupport, codec: crate::nvenc::NvencCodec) -> bool {
    match codec {
        crate::nvenc::NvencCodec::H264 => client.h264,
        crate::nvenc::NvencCodec::H265 => client.h265,
        crate::nvenc::NvencCodec::Av1 => client.av1,
    }
}

fn encode_mf_frame(
    encoder: &mut Option<crate::mf_encode::MfVideoEncoder>,
    codec: crate::nvenc::NvencCodec,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
    bgra: &[u8],
    force_key: bool,
) -> Result<Option<EncodedPacket>, String> {
    let fps = fps.clamp(5, MAX_TARGET_FPS);
    let recreate = encoder
        .as_ref()
        .map(|enc| !enc.matches(codec, width, height, fps))
        .unwrap_or(true);

    if recreate {
        *encoder = Some(crate::mf_encode::MfVideoEncoder::new(
            codec, width, height, fps, bitrate,
        )?);
        eprintln!(
            "[host-video] MF {} encoder started at {}x{}@{} bitrate={}",
            codec.label(),
            width,
            height,
            fps,
            bitrate
        );
    } else if let Some(enc) = encoder.as_mut() {
        // Update bitrate at runtime — avoids tearing down and restarting the
        // encoder when the operator changes quality mid-session.
        if enc.current_bitrate() != bitrate {
            if !enc.update_bitrate(bitrate) {
                eprintln!(
                    "[host-video] MF {} bitrate update failed, recreating encoder",
                    codec.label()
                );
                *encoder = Some(crate::mf_encode::MfVideoEncoder::new(
                    codec, width, height, fps, bitrate,
                )?);
            }
        }
    }

    let Some(encoder) = encoder.as_mut() else {
        return Ok(None);
    };
    encoder.encode_bgra(bgra, force_key).map(|packet| {
        packet.map(|p| EncodedPacket {
            bytes: p.bytes,
            key: p.key,
        })
    })
}

fn encode_videotoolbox_frame(
    encoder: &mut Option<crate::videotoolbox::VideoToolboxEncoder>,
    codec: crate::nvenc::NvencCodec,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
    bgra: &[u8],
    force_key: bool,
) -> Result<Option<EncodedPacket>, String> {
    let fps = fps.clamp(5, MAX_TARGET_FPS);
    let recreate = encoder
        .as_ref()
        .map(|enc| !enc.matches(codec, width, height, fps))
        .unwrap_or(true);

    if recreate {
        *encoder = Some(crate::videotoolbox::VideoToolboxEncoder::new(
            codec, width, height, fps, bitrate,
        )?);
        eprintln!(
            "[host-video] VideoToolbox {} encoder started at {}x{}@{} bitrate={}",
            codec.label(),
            width,
            height,
            fps,
            bitrate
        );
    } else if let Some(enc) = encoder.as_mut() {
        if enc.current_bitrate() != bitrate {
            if !enc.update_bitrate(bitrate) {
                eprintln!("[host-video] VideoToolbox bitrate update failed, recreating");
                *encoder = Some(crate::videotoolbox::VideoToolboxEncoder::new(
                    codec, width, height, fps, bitrate,
                )?);
            }
        }
    }

    let Some(encoder) = encoder.as_mut() else {
        return Ok(None);
    };
    encoder.encode_bgra(bgra, force_key).map(|packet| {
        packet.map(|p| EncodedPacket {
            bytes: p.bytes,
            key: p.key,
        })
    })
}

fn encode_nvenc_frame(
    encoder: &mut Option<crate::nvenc::NvencEncoder>,
    codec: crate::nvenc::NvencCodec,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
    bgra: &[u8],
    force_key: bool,
) -> Result<Option<EncodedPacket>, String> {
    let fps = fps.clamp(5, MAX_TARGET_FPS);
    let recreate = encoder
        .as_ref()
        .map(|enc| !enc.matches(codec, width, height, fps))
        .unwrap_or(true);

    if recreate {
        *encoder = Some(crate::nvenc::NvencEncoder::new(
            codec, width, height, fps, bitrate,
        )?);
        eprintln!(
            "[host-video] NVENC {} encoder started at {}x{}@{} bitrate={}",
            codec.label(),
            width,
            height,
            fps,
            bitrate
        );
    } else if let Some(enc) = encoder.as_mut() {
        if enc.current_bitrate() != bitrate {
            enc.update_bitrate(bitrate);
        }
    }

    let Some(encoder) = encoder.as_mut() else {
        return Ok(None);
    };
    encoder.encode_bgra(bgra, force_key).map(|packet| {
        packet.map(|p| EncodedPacket {
            bytes: p.bytes,
            key: p.key,
        })
    })
}

/// Read a PeerMessage using the receive-only cipher half.
fn recv_peer_rc(
    stream: &mut TcpStream,
    cipher: &mut Option<crate::crypto::RecvCipher>,
) -> Result<Option<PeerMessage>, String> {
    let payload = read_framed(stream)?;
    if payload.is_empty() {
        return Ok(None);
    }
    let plain = if let Some(c) = cipher.as_mut() {
        c.decrypt(&payload)?
    } else {
        payload
    };
    let msg = decode_peer_message(&plain).map_err(|e| format!("Peer decode: {e}"))?;
    Ok(Some(msg))
}

// ── H264 encoding ─────────────────────────────────────────────────────────────

struct H264Packet {
    bytes: Vec<u8>,
    key: bool,
}

#[cfg(feature = "live-h264")]
fn encode_h264_frame(
    encoder: Option<&mut openh264::encoder::Encoder>,
    yuv: &mut YuvFrame,
    w: u32,
    h: u32,
    bgra: &[u8],
    key: bool,
) -> Option<H264Packet> {
    let enc = encoder?;
    if key {
        enc.force_intra_frame();
    }
    bgra_to_yuv420_into(yuv, w as usize, h as usize, bgra);
    let bitstream = enc.encode(yuv).ok()?;
    let encoded_key = matches!(
        bitstream.frame_type(),
        openh264::encoder::FrameType::IDR | openh264::encoder::FrameType::I
    );
    let mut out = Vec::new();
    for i in 0..bitstream.num_layers() {
        if let Some(layer) = bitstream.layer(i) {
            for j in 0..layer.nal_count() {
                if let Some(nal) = layer.nal_unit(j) {
                    out.extend_from_slice(nal);
                }
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(H264Packet {
            bytes: out,
            key: encoded_key,
        })
    }
}

#[cfg(not(feature = "live-h264"))]
fn encode_h264_frame(
    _encoder: &mut Option<()>,
    _w: u32,
    _h: u32,
    _bgra: &[u8],
    _key: bool,
) -> Option<H264Packet> {
    None
}

// ── YUV conversion ────────────────────────────────────────────────────────────
// All pixel math lives in crate::colorconv (optimized, unchecked, row-batched).

#[cfg(feature = "live-h264")]
#[derive(Default)]
struct YuvFrame {
    width: usize,
    height: usize,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

#[cfg(feature = "live-h264")]
impl YuvFrame {
    fn resize(&mut self, width: usize, height: usize) {
        self.width = width.next_multiple_of(2);
        self.height = height.next_multiple_of(2);
        self.y.resize(self.width * self.height, 0);
        self.u.resize((self.width / 2) * (self.height / 2), 0);
        self.v.resize((self.width / 2) * (self.height / 2), 0);
    }
}

#[cfg(feature = "live-h264")]
impl openh264::formats::YUVSource for YuvFrame {
    fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }
    fn strides(&self) -> (usize, usize, usize) {
        (self.width, self.width / 2, self.width / 2)
    }
    fn y(&self) -> &[u8] {
        &self.y
    }
    fn u(&self) -> &[u8] {
        &self.u
    }
    fn v(&self) -> &[u8] {
        &self.v
    }
}

/// BGRA → planar I420.  Delegates to `crate::colorconv` for the fast path.
#[cfg(feature = "live-h264")]
fn bgra_to_yuv420_into(out: &mut YuvFrame, w: usize, h: usize, bgra: &[u8]) {
    if w == 0 || h == 0 || bgra.len() < w.saturating_mul(h).saturating_mul(4) {
        return;
    }
    out.resize(w, h);
    let dst_w = out.width;
    let dst_h = out.height;
    crate::colorconv::bgra_to_i420(&mut out.y, &mut out.u, &mut out.v, w, h, dst_w, dst_h, bgra);
}

#[cfg(test)]
mod remote_identity_tests {
    use super::*;

    #[test]
    fn login_identity_prefers_real_client_id_and_sanitizes_metadata() {
        let login = crate::rustdesk_proto::LoginRequest {
            my_id: " 987 654 321 ".to_owned(),
            my_name: "Office PC\n".to_owned(),
            my_platform: "windows".to_owned(),
            version: "1.4.6".to_owned(),
            ..Default::default()
        };
        let identity = remote_identity_from_login(&login);
        assert_eq!(identity.peer_id, "987 654 321");
        assert_eq!(identity.peer_name, "Office PC");
        assert_eq!(identity.platform, "windows");
        assert_eq!(identity.version, "1.4.6");
    }

    #[test]
    fn legacy_generic_identity_has_a_readable_fallback() {
        let login = crate::rustdesk_proto::LoginRequest {
            my_id: "evertydesk-lite".to_owned(),
            my_name: "EvertyDesk Lite".to_owned(),
            ..Default::default()
        };
        assert_eq!(
            remote_identity_from_login(&login).peer_id,
            "Неизвестное устройство"
        );
        assert_eq!(
            safe_session_end_reason("Incoming connection rejected"),
            "Подключение отклонено"
        );
    }
}

#[cfg(test)]
mod codec_negotiation_tests {
    use super::*;
    use crate::nvenc::NvencCodec;

    fn client(h264: bool, h265: bool, av1: bool, prefer: PreferCodec) -> ClientVideoSupport {
        ClientVideoSupport {
            h264,
            h265,
            av1,
            prefer,
        }
    }

    fn negotiate(
        available: &[(NvencCodec, bool)],
        codec: CodecPreference,
        client: ClientVideoSupport,
    ) -> Option<NvencCodec> {
        negotiate_backend_codec(available, EncoderPreference::Auto, codec, client)
    }

    #[test]
    fn auto_picks_hevc_when_both_have_hardware() {
        let available = [(NvencCodec::H264, true), (NvencCodec::H265, true)];
        let got = negotiate(
            &available,
            CodecPreference::Auto,
            client(true, true, false, PreferCodec::Auto),
        );
        assert_eq!(got, Some(NvencCodec::H265));
    }

    #[test]
    fn auto_picks_av1_over_hevc_when_decodable() {
        let available = [
            (NvencCodec::H264, true),
            (NvencCodec::H265, true),
            (NvencCodec::Av1, true),
        ];
        let got = negotiate(
            &available,
            CodecPreference::Auto,
            client(true, true, true, PreferCodec::Auto),
        );
        assert_eq!(got, Some(NvencCodec::Av1));
    }

    #[test]
    fn auto_falls_to_h264_when_client_cannot_decode_hevc() {
        let available = [(NvencCodec::H264, true), (NvencCodec::H265, true)];
        let got = negotiate(
            &available,
            CodecPreference::Auto,
            client(true, false, false, PreferCodec::Auto),
        );
        assert_eq!(got, Some(NvencCodec::H264));
    }

    #[test]
    fn auto_rejects_software_only_hevc() {
        // HEVC present but software-only → gate #2 drops it, H264 wins.
        let available = [(NvencCodec::H264, true), (NvencCodec::H265, false)];
        let got = negotiate(
            &available,
            CodecPreference::Auto,
            client(true, true, false, PreferCodec::Auto),
        );
        assert_eq!(got, Some(NvencCodec::H264));
    }

    #[test]
    fn client_prefer_h264_overrides_quality() {
        // Client can decode HEVC but explicitly prefers H264 (e.g. its HEVC
        // decode is software) → honour the client.
        let available = [(NvencCodec::H264, true), (NvencCodec::H265, true)];
        let got = negotiate(
            &available,
            CodecPreference::Auto,
            client(true, true, false, PreferCodec::H264),
        );
        assert_eq!(got, Some(NvencCodec::H264));
    }

    #[test]
    fn host_explicit_choice_beats_client_prefer() {
        // Host admin explicitly chose H265; client prefers H264 but can decode
        // H265 → host wins.
        let available = [(NvencCodec::H264, true), (NvencCodec::H265, true)];
        let got = negotiate(
            &available,
            CodecPreference::H265,
            client(true, true, false, PreferCodec::H264),
        );
        assert_eq!(got, Some(NvencCodec::H265));
    }

    #[test]
    fn explicit_hevc_falls_to_h264_when_client_cannot_decode() {
        // Explicit H265 but client can't decode it → universal H264 safety net.
        let available = [(NvencCodec::H264, true), (NvencCodec::H265, true)];
        let got = negotiate(
            &available,
            CodecPreference::H265,
            client(true, false, false, PreferCodec::Auto),
        );
        assert_eq!(got, Some(NvencCodec::H264));
    }

    #[test]
    fn software_preference_disables_hardware_backends() {
        let available = [(NvencCodec::H264, true), (NvencCodec::H265, true)];
        let got = negotiate_backend_codec(
            &available,
            EncoderPreference::Software,
            CodecPreference::Auto,
            client(true, true, false, PreferCodec::Auto),
        );
        assert_eq!(got, None);
    }

    #[test]
    fn empty_backend_yields_nothing() {
        let got = negotiate(
            &[],
            CodecPreference::Auto,
            client(true, true, true, PreferCodec::Auto),
        );
        assert_eq!(got, None);
    }
}

#[cfg(test)]
mod video_quality_tests {
    use super::*;

    #[test]
    fn frame_change_detector_sends_first_frame_then_skips_static() {
        let mut detector = FrameChangeDetector::default();
        let frame = vec![0_u8; 64 * 64 * 4];

        let first = detector.decide(64, 64, &frame, false);
        assert!(first.send);
        detector.mark_sent(64, 64, &frame);

        let second = detector.decide(64, 64, &frame, false);
        assert!(!second.send);
    }

    #[test]
    fn frame_change_detector_detects_large_change() {
        let mut detector = FrameChangeDetector::default();
        let frame = vec![0_u8; 64 * 64 * 4];
        detector.mark_sent(64, 64, &frame);

        let changed = vec![255_u8; 64 * 64 * 4];
        assert!(detector.decide(64, 64, &changed, false).send);
    }

    #[test]
    fn frame_change_detector_first_frame_uses_fullscreen_roi() {
        let mut detector = FrameChangeDetector::default();
        let frame = vec![0_u8; 64 * 64 * 4];
        let decision = detector.decide(64, 64, &frame, false);
        assert!(decision.send);
        assert!(decision.roi.is_full_screen());
    }

    #[test]
    fn frame_change_detector_reports_tile_dirty_roi() {
        let mut detector = FrameChangeDetector::default();
        let frame = vec![0_u8; 128 * 128 * 4];
        detector.mark_sent(128, 128, &frame);

        let mut changed = frame.clone();
        for y in 64..96 {
            for x in 64..96 {
                let base = (y * 128 + x) * 4;
                changed[base] = 255;
                changed[base + 1] = 255;
                changed[base + 2] = 255;
                changed[base + 3] = 255;
            }
        }

        let decision = detector.decide(128, 128, &changed, false);
        assert!(decision.send);
        assert_eq!(
            decision.roi,
            crate::evrt::RoiRect {
                frame_id: 0,
                x: 64,
                y: 64,
                w: 32,
                h: 32,
            }
        );
    }

    #[test]
    fn frame_change_detector_backs_off_after_static_frames() {
        let mut detector = FrameChangeDetector::default();
        let frame = vec![0_u8; 64 * 64 * 4];
        detector.mark_sent(64, 64, &frame);

        for _ in 0..59 {
            let decision = detector.decide(64, 64, &frame, false);
            assert!(!decision.send);
            assert!(detector.static_backoff_delay(60).is_none());
        }

        let decision = detector.decide(64, 64, &frame, false);
        assert!(!decision.send);
        assert_eq!(
            detector.static_backoff_delay(60),
            Some(Duration::from_millis(25))
        );
    }

    #[test]
    fn h264_bitrate_scales_with_resolution() {
        let small = h264_target_bitrate_bps(1280, 720, 30, DEFAULT_QUALITY_MILLI);
        let full_hd = h264_target_bitrate_bps(1920, 1080, 30, DEFAULT_QUALITY_MILLI);
        let ultra_hd = h264_target_bitrate_bps(3840, 2160, 60, DEFAULT_QUALITY_MILLI);

        assert!(small >= 800_000);
        assert!(full_hd > small);
        assert!(ultra_hd > full_hd);
        assert!(ultra_hd <= 14_000_000);
    }

    #[test]
    fn h264_bitrate_uses_best_quality_headroom() {
        let balanced = h264_target_bitrate_bps(1920, 1080, 60, BALANCED_QUALITY_MILLI);
        let best = h264_target_bitrate_bps(1920, 1080, 60, BEST_QUALITY_MILLI);
        let ultra_hd_best = h264_target_bitrate_bps(3840, 2160, 60, BEST_QUALITY_MILLI);

        assert!(best > balanced);
        assert!(ultra_hd_best > 14_000_000);
        assert!(ultra_hd_best <= 40_000_000);
    }

    #[cfg(feature = "live-h264")]
    #[test]
    fn yuv_conversion_pads_odd_dimensions_for_i420() {
        let mut yuv = YuvFrame::default();
        let bgra = vec![128_u8; 3 * 3 * 4];

        bgra_to_yuv420_into(&mut yuv, 3, 3, &bgra);

        assert_eq!((yuv.width, yuv.height), (4, 4));
        assert_eq!(yuv.y.len(), 16);
        assert_eq!(yuv.u.len(), 4);
        assert_eq!(yuv.v.len(), 4);
    }
}

// ── Input injection ───────────────────────────────────────────────────────────

/// Версия handle_client_input для нового pipeline — шлёт команды через канал.
fn handle_client_input_pipeline(
    msg: PeerMessage,
    cmd_tx: &mpsc::Sender<crate::video_pipeline::PipelineCmd>,
    peer_msg_tx: &mpsc::Sender<PeerMessage>,
    events: &Sender<HostEvent>,
    shell: &mut Option<ShellRuntime>,
    target_fps: &AtomicU32,
    quality_milli: &AtomicU32,
    allow_clipboard: bool,
) {
    use crate::video_pipeline::PipelineCmd;
    match msg.union {
        Some(peer_message::Union::MouseEvent(ev)) => {
            // Agentless VM: если прикреплены к VM — ввод уходит в VM, не в ОС хоста.
            if crate::vm_bridge::route_mouse(&ev) {
                return;
            }
            if BLOCK_REMOTE_INPUT.load(Ordering::Relaxed) {
                return;
            }
            if mouse_event_should_log(&ev) {
                host_log(events, format!("Host input: {}", mouse_event_summary(&ev)));
            }
            // EVRT2CKMAX-TASK-01 (experimental): track cursor position as the
            // Visible Region focus point for EVRTCK priority tile ordering.
            // Negative coordinates (e.g. multi-monitor offsets) clamp to 0 —
            // set_focus_pixel clamps into frame bounds anyway, so this only
            // affects which edge tile gets treated as "nearest," never crashes.
            let _ = cmd_tx.send(crate::video_pipeline::PipelineCmd::CursorMoved {
                x: ev.x.max(0) as u32,
                y: ev.y.max(0) as u32,
            });
            #[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
            elevation_hint::check(events);
            inject_mouse(ev);
        }
        Some(peer_message::Union::KeyEvent(ev)) => {
            if crate::vm_bridge::route_key(&ev) {
                return;
            }
            if BLOCK_REMOTE_INPUT.load(Ordering::Relaxed) {
                return;
            }
            host_log(events, format!("Host input: {}", key_event_summary(&ev)));
            inject_key(ev);
        }
        Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::Evrt2ExperimentRequest(_)),
        })) => {
            // Экспериментальная кнопка «EVRT2» в режиме игры — полностью
            // отдельный UDP-сокет + свой цикл захвата/кодирования EVRTCK,
            // не трогает video_pipeline.rs/encode_loop. См. evrt2_experiment.rs.
            match crate::evrt2_experiment::start_host_experiment(events.clone()) {
                Ok((port, public_addr, auth_key, _stop)) => {
                    // ROADMAP.md Phase 5.1: LAN/VPN candidates plus a WAN
                    // one if STUN succeeded on this socket — same "mini-ICE"
                    // shape EVRT1's own UDP announce already uses.
                    let mut endpoints = crate::netif::candidate_endpoints(port);
                    if let Some(addr) = public_addr {
                        endpoints = if endpoints.is_empty() {
                            addr.to_string()
                        } else {
                            format!("{endpoints},{addr}")
                        };
                    }
                    host_log(events, format!("EVRT2 (experimental): сокет поднят на порту {port}, кандидаты: [{endpoints}]"));
                    let endpoints_with_key = format!(
                        "{endpoints},evrt2key={}",
                        crate::evrt2_crypto::key_to_hex(&auth_key)
                    );
                    let _ = peer_msg_tx.send(vm_misc_message(
                        misc::Union::Evrt2ExperimentEndpoints(endpoints_with_key),
                    ));
                }
                Err(e) => {
                    host_log(
                        events,
                        format!("EVRT2 (experimental): не удалось поднять сокет: {e}"),
                    );
                }
            }
        }
        Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::VmListRequest(_)),
        })) => {
            let json = crate::vm_bridge::list_json();
            host_log(events, "Host VM: список VM запрошен клиентом".to_owned());
            let _ = peer_msg_tx.send(vm_misc_message(misc::Union::VmList(json)));
        }
        Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::VmAttach(vm_id)),
        })) => {
            let attaching = !vm_id.trim().is_empty();
            let status = match crate::vm_bridge::attach(&vm_id) {
                Ok(s) => s,
                Err(e) => format!("Ошибка VM: {e}"),
            };
            host_log(events, format!("Host VM: attach '{vm_id}' → {status}"));
            let _ = cmd_tx.send(PipelineCmd::RefreshDisplay(0));
            let _ = peer_msg_tx.send(vm_misc_message(misc::Union::VmStatus(status)));
            // Периодический pump статуса, пока VM прикреплена: клиент не «висит»
            // на «Подключение…», плюс всплывают ошибки ввода (диагностика).
            if attaching {
                let tx = peer_msg_tx.clone();
                thread::spawn(move || {
                    let mut last = String::new();
                    for _ in 0..150 {
                        // ~5 минут максимум
                        thread::sleep(Duration::from_millis(2000));
                        if !crate::vm_bridge::is_attached() {
                            break;
                        }
                        let live = format!("● Поток VM активен — {}", crate::vm_bridge::status());
                        if live != last {
                            let _ = tx.send(vm_misc_message(misc::Union::VmStatus(live.clone())));
                            last = live;
                        }
                    }
                });
            }
        }
        // ── VM Power Operation ──────────────────────────────────────────────────
        Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::VmPowerOp(json)),
        })) => {
            host_log(events, format!("Host VM: power op запрошен: {json}"));
            let result = handle_vm_power_op(&json);
            let _ = peer_msg_tx.send(vm_misc_message(misc::Union::VmPowerResult(result)));
        }
        // ── VM Capability Request ───────────────────────────────────────────────
        Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::VmCapabilityRequest(vm_id)),
        })) => {
            host_log(
                events,
                format!("Host VM: capability graph запрошен для {vm_id}"),
            );
            let graph_json = crate::vm_bridge::get_capability_graph(&vm_id);
            let _ = peer_msg_tx.send(vm_misc_message(misc::Union::VmCapabilityGraph(graph_json)));
        }
        // ── VM Checkpoint Operation ─────────────────────────────────────────────
        Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::VmCheckpointOp(json)),
        })) => {
            host_log(events, format!("Host VM: checkpoint op: {json}"));
            let result = crate::vm_bridge::checkpoint_op(&json);
            let _ = peer_msg_tx.send(vm_misc_message(misc::Union::VmCheckpoints(result)));
        }
        // ── VM Rescue Input ─────────────────────────────────────────────────────
        Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::VmRescueInput(json)),
        })) => {
            host_log(events, format!("Host VM: rescue input: {json}"));
            crate::vm_bridge::rescue_input(&json);
        }
        // ── Управление громкостью хоста ─────────────────────────────────────────
        Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::SetHostVolume(vol)),
        })) => {
            let vol = vol.min(100);
            match set_system_volume(vol) {
                Ok(()) => host_log(events, format!("Громкость хоста → {vol}%")),
                Err(e) => host_log(events, format!("Не удалось изменить громкость: {e}")),
            }
        }
        Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::Option(option)),
        })) => {
            if option.custom_fps > 0 {
                let fps = (option.custom_fps as u32).clamp(5, MAX_TARGET_FPS);
                target_fps.store(fps, Ordering::Relaxed);
                let _ = cmd_tx.send(PipelineCmd::SetFps(fps));
            }
            if let Some(quality) = option_quality_milli(&option) {
                quality_milli.store(quality, Ordering::Relaxed);
                let _ = cmd_tx.send(PipelineCmd::SetQuality(quality));
            }
            if let Some(sd) = &option.supported_decoding {
                let prefer = PreferCodec::try_from(sd.prefer).unwrap_or(PreferCodec::Auto);
                let new_video = ClientVideoSupport {
                    h264: sd.ability_h264 != 0,
                    h265: sd.ability_h265 != 0,
                    av1: sd.ability_av1 != 0,
                    prefer,
                };
                host_log(
                    events,
                    format!(
                        "Host: клиент запросил кодек {:?} (h264={} h265={} av1={})",
                        prefer, new_video.h264, new_video.h265, new_video.av1
                    ),
                );
                let _ = cmd_tx.send(PipelineCmd::SetClientCodec(new_video));
            }
        }
        Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::SwitchDisplay(display)),
        })) => {
            let display = display.display.max(0);
            host_log(
                events,
                format!("Host display command: SwitchDisplay {}", display + 1),
            );
            let _ = cmd_tx.send(PipelineCmd::SetDisplay(display));
        }
        Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::CaptureDisplays(displays)),
        })) => {
            if !displays.set.is_empty() {
                let set: Vec<i32> = displays
                    .set
                    .iter()
                    .map(|display| (*display).max(0))
                    .collect();
                host_log(
                    events,
                    format!("Host display command: CaptureDisplays.set {set:?}"),
                );
                let _ = cmd_tx.send(PipelineCmd::SetSubscribedDisplays(set));
            } else {
                if !displays.add.is_empty() {
                    let add: Vec<i32> = displays
                        .add
                        .iter()
                        .map(|display| (*display).max(0))
                        .collect();
                    host_log(
                        events,
                        format!("Host display command: CaptureDisplays.add {add:?}"),
                    );
                    let _ = cmd_tx.send(PipelineCmd::AddSubscribedDisplays(add));
                }
                if !displays.sub.is_empty() {
                    let sub: Vec<i32> = displays
                        .sub
                        .iter()
                        .map(|display| (*display).max(0))
                        .collect();
                    host_log(
                        events,
                        format!("Host display command: CaptureDisplays.sub {sub:?}"),
                    );
                    let _ = cmd_tx.send(PipelineCmd::RemoveSubscribedDisplays(sub));
                }
            }
        }
        Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::MessageQuery(query)),
        })) => {
            if query.switch_display >= 0 {
                let _ = cmd_tx.send(PipelineCmd::SendSwitchDisplay(query.switch_display));
            }
        }
        Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::RefreshVideoDisplay(display)),
        })) => {
            let display = display.max(0);
            host_log(
                events,
                format!("Host display command: RefreshVideoDisplay {}", display + 1),
            );
            let _ = cmd_tx.send(PipelineCmd::RefreshDisplay(display));
        }
        Some(peer_message::Union::Shell(shell_msg)) => {
            // Shell output идёт через peer_msg_tx → pipeline → TCP relay → клиент
            handle_shell_message(shell_msg, peer_msg_tx, shell);
        }
        Some(peer_message::Union::Clipboard(clipboard)) => {
            if !allow_clipboard {
                host_log(
                    events,
                    "Host clipboard ignored: disabled by security policy".to_owned(),
                );
                return;
            }

            match crate::transport::clipboard_text_from_message(clipboard) {
                Ok(Some(text)) => {
                    let chars = text.chars().count();
                    match write_host_clipboard_text(&text) {
                        Ok(()) => {
                            host_log(events, format!("Host clipboard updated: {chars} chars"))
                        }
                        Err(err) => host_log(events, format!("Host clipboard write failed: {err}")),
                    }
                }
                Ok(None) => host_log(
                    events,
                    "Host clipboard ignored: unsupported format".to_owned(),
                ),
                Err(err) => host_log(events, format!("Host clipboard decode failed: {err}")),
            }
        }
        _ => {}
    }
}

#[cfg(feature = "desktop-gui")]
fn write_host_clipboard_text(text: &str) -> Result<(), String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|err| format!("Clipboard init failed: {err}"))?;
    clipboard
        .set_text(text.to_owned())
        .map_err(|err| format!("Clipboard text write failed: {err}"))
}

#[cfg(not(feature = "desktop-gui"))]
fn write_host_clipboard_text(_text: &str) -> Result<(), String> {
    Err("Clipboard write unavailable in this build".to_owned())
}

fn mouse_event_should_log(ev: &crate::rustdesk_proto::MouseEvent) -> bool {
    (ev.mask & 0x7) != 0
}

fn mouse_event_summary(ev: &crate::rustdesk_proto::MouseEvent) -> String {
    let kind = match ev.mask & 0x7 {
        0 => "move",
        1 => "down",
        2 => "up",
        3 => "wheel",
        _ => "unknown",
    };
    let button = match ev.mask >> 3 {
        1 => "left",
        2 => "right",
        4 => "middle",
        8 => "back",
        16 => "forward",
        _ => "none",
    };
    format!(
        "mouse {kind} button={button} x={} y={} mask={}",
        ev.x, ev.y, ev.mask
    )
}

fn key_event_summary(ev: &crate::rustdesk_proto::KeyEvent) -> String {
    let key = match &ev.union {
        Some(crate::rustdesk_proto::key_event::Union::ControlKey(key)) => {
            format!("control={key}")
        }
        Some(crate::rustdesk_proto::key_event::Union::Unicode(ch)) => format!("unicode={ch}"),
        None => "none".to_owned(),
    };
    format!("key {key} down={} press={}", ev.down, ev.press)
}

struct ShellRuntime {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl ShellRuntime {
    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn handle_shell_message(
    msg: ShellMessage,
    outgoing: &Sender<PeerMessage>,
    shell: &mut Option<ShellRuntime>,
) {
    match ShellMessageKind::try_from(msg.kind).unwrap_or(ShellMessageKind::Input) {
        ShellMessageKind::Start => {
            if shell.is_none() {
                match start_shell_process(outgoing.clone()) {
                    Ok(runtime) => {
                        *shell = Some(runtime);
                        send_shell_out(outgoing, ShellMessageKind::Output, "Shell started\r\n");
                    }
                    Err(err) => send_shell_out(outgoing, ShellMessageKind::Error, &err),
                }
            }
        }
        ShellMessageKind::Input => {
            if let Some(runtime) = shell.as_mut() {
                if let Some(stdin) = runtime.stdin.as_mut() {
                    let _ = stdin.write_all(msg.data.as_bytes());
                    let _ = stdin.flush();
                }
            }
        }
        ShellMessageKind::Stop => {
            if let Some(mut runtime) = shell.take() {
                runtime.stop();
            }
            send_shell_out(outgoing, ShellMessageKind::Closed, "");
        }
        _ => {}
    }
}

fn start_shell_process(outgoing: Sender<PeerMessage>) -> Result<ShellRuntime, String> {
    #[cfg(target_os = "windows")]
    let mut command = { Command::new("cmd.exe") };
    #[cfg(not(target_os = "windows"))]
    let mut command = {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
        Command::new(shell)
    };

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Start shell failed: {err}"))?;

    if let Some(stdout) = child.stdout.take() {
        spawn_shell_reader(stdout, outgoing.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_shell_reader(stderr, outgoing.clone());
    }

    Ok(ShellRuntime {
        stdin: child.stdin.take(),
        child,
    })
}

fn spawn_shell_reader(mut reader: impl Read + Send + 'static, outgoing: Sender<PeerMessage>) {
    thread::spawn(move || {
        let mut buf = [0u8; 2048];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    send_shell_out(&outgoing, ShellMessageKind::Closed, "");
                    break;
                }
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    send_shell_out(&outgoing, ShellMessageKind::Output, &text);
                }
                Err(err) => {
                    send_shell_out(&outgoing, ShellMessageKind::Error, &format!("{err}"));
                    break;
                }
            }
        }
    });
}

fn send_shell_out(outgoing: &Sender<PeerMessage>, kind: ShellMessageKind, data: &str) {
    let _ = outgoing.send(PeerMessage {
        union: Some(peer_message::Union::Shell(ShellMessage {
            kind: kind as i32,
            data: data.to_owned(),
        })),
    });
}

/// Release mouse buttons (and common modifier keys) that may have been left

/// "down" when a session ended mid-click — otherwise the local desktop becomes
/// unusable (e.g. Start menu not clickable).
#[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
fn release_stuck_input() {
    use std::mem::size_of;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
        KEYEVENTF_KEYUP, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTUP, MOUSEINPUT,
        MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
    };
    unsafe {
        // VK_LBUTTON=1, VK_RBUTTON=2, VK_MBUTTON=4
        // GetAsyncKeyState high bit = button currently down
        let ldown = (GetAsyncKeyState(0x01) as u16) & 0x8000 != 0;
        let rdown = (GetAsyncKeyState(0x02) as u16) & 0x8000 != 0;
        let mdown = (GetAsyncKeyState(0x04) as u16) & 0x8000 != 0;

        let mut flags = MOUSE_EVENT_FLAGS(0);
        if ldown {
            flags |= MOUSEEVENTF_LEFTUP;
        }
        if rdown {
            flags |= MOUSEEVENTF_RIGHTUP;
        }
        if mdown {
            flags |= MOUSEEVENTF_MIDDLEUP;
        }

        if flags.0 != 0 {
            let mi = MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            };
            let m_input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 { mi },
            };
            SendInput(&[m_input], size_of::<INPUT>() as i32);
        }

        // Release common modifiers only if actually down.
        // Ctrl=0x11, Alt=0x12, Shift=0x10, LWin=0x5B, RWin=0x5C
        for vk in [0x11u16, 0x12, 0x10, 0x5B, 0x5C] {
            if (GetAsyncKeyState(vk as i32) as u16) & 0x8000 == 0 {
                continue;
            }
            let ki = KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            };
            let k_input = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 { ki },
            };
            SendInput(&[k_input], size_of::<INPUT>() as i32);
        }
    }
}

#[cfg(all(
    not(target_os = "linux"),
    not(target_os = "macos"),
    not(all(target_os = "windows", feature = "live-vp9-mf"))
))]
fn release_stuck_input() {}

#[cfg(target_os = "macos")]
fn release_stuck_input() {
    macos_cgevent::release_stuck_input();
}

// ── Elevation-needed detection (TZ_HOST_SERVICE.md complication #2) ───────
//
// Windows silently drops any input a lower-integrity process injects into a
// higher-integrity window (UIPI) — SendInput still returns success, so there
// is no error to catch after the fact. The only reliable signal is to check
// *before* injecting: compare our own process's integrity level against the
// foreground window's owning process. This can't fix the block (that needs
// the agent itself to run elevated — see `winservice.rs`'s linked-token
// trick, which only applies when running as the Windows service), but it
// lets us tell the person at the keyboard why control just stopped working
// instead of leaving it a silent mystery.
#[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
mod elevation_hint {
    use super::*;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
    use windows::Win32::Security::{
        GetTokenInformation, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    static LAST_CHECK_MS: AtomicU64 = AtomicU64::new(0);
    static WAS_NEEDED: AtomicBool = AtomicBool::new(false);

    unsafe fn integrity_level_of(token: HANDLE) -> Option<u32> {
        let mut needed = 0u32;
        // First call with a null buffer to learn the required size.
        let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut needed);
        if needed == 0 {
            return None;
        }
        let mut buf = vec![0u8; needed as usize];
        if !GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buf.as_mut_ptr() as *mut _),
            needed,
            &mut needed,
        )
        .as_bool()
        {
            return None;
        }
        let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        let count = *windows::Win32::Security::GetSidSubAuthorityCount(sid);
        if count == 0 {
            return None;
        }
        // The integrity level RID is always the SID's last sub-authority —
        // e.g. S-1-16-4096 (Low), -8192 (Medium), -12288 (High), -16384
        // (System).
        let rid_ptr = windows::Win32::Security::GetSidSubAuthority(sid, (count - 1) as u32);
        Some(*rid_ptr)
    }

    unsafe fn integrity_level_of_process(process: HANDLE) -> Option<u32> {
        let mut token = HANDLE::default();
        if !OpenProcessToken(process, TOKEN_QUERY, &mut token).as_bool() {
            return None;
        }
        let level = integrity_level_of(token);
        let _ = CloseHandle(token);
        level
    }

    fn foreground_needs_elevation() -> bool {
        unsafe {
            let ours = match integrity_level_of_process(GetCurrentProcess()) {
                Some(l) => l,
                None => return false,
            };

            let hwnd: HWND = GetForegroundWindow();
            if hwnd.0 == 0 {
                return false;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return false;
            }
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return false;
            };
            let theirs = integrity_level_of_process(process);
            let _ = CloseHandle(process);
            match theirs {
                Some(theirs) => theirs > ours,
                None => false,
            }
        }
    }

    /// Checks at most once every 3 seconds (cheap atomic load otherwise —
    /// safe to call from a hot input path) and logs only on a state
    /// transition, not on every check, so this can never spam the log.
    pub fn check(events: &Sender<HostEvent>) {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let last = LAST_CHECK_MS.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last) < 3000 {
            return;
        }
        LAST_CHECK_MS.store(now_ms, Ordering::Relaxed);

        let needed = foreground_needs_elevation();
        let was = WAS_NEEDED.swap(needed, Ordering::Relaxed);
        if needed && !was {
            host_log(
                events,
                "⚠ В фокусе на хосте окно с повышенными правами (например, Диспетчер задач \
                 или UAC-запрос) — ввод от клиента туда не дойдёт: так работает защита Windows \
                 (UIPI), это не баг соединения. Для полного доступа установите службу (кнопка \
                 в главном окне) — она умеет получать повышенный токен для этого; либо запустите \
                 сам хост от имени администратора."
                    .to_owned(),
            );
        } else if !needed && was {
            host_log(
                events,
                "Окно с повышенными правами больше не в фокусе — ввод снова работает как обычно."
                    .to_owned(),
            );
        }
    }
}

#[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
fn inject_mouse(ev: crate::rustdesk_proto::MouseEvent) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
        MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN,
        MOUSEEVENTF_XUP,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    // XBUTTON1 = 0x0001 (back), XBUTTON2 = 0x0002 (forward) — for mouseData.
    const XBUTTON1: i32 = 0x0001;
    const XBUTTON2: i32 = 0x0002;

    // RustDesk mouse mask encoding:  mask = (button << 3) | event_type
    //   event_type:  0 = move, 1 = button down, 2 = button up, 3 = wheel
    //   button:      1 = left, 2 = right, 4 = middle(wheel), 8 = back, 16 = forward
    const EVT_MOVE: i32 = 0;
    const EVT_DOWN: i32 = 1;
    const EVT_UP: i32 = 2;
    const EVT_WHEEL: i32 = 3;
    const BTN_LEFT: i32 = 1;
    const BTN_RIGHT: i32 = 2;
    const BTN_MIDDLE: i32 = 4;
    const BTN_BACK: i32 = 8;
    const BTN_FORWARD: i32 = 16;

    let evt_type = ev.mask & 0x7;
    let button = ev.mask >> 3;

    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1);

        // ── Wheel: ev.x / ev.y carry scroll deltas, not coordinates ──────────
        if evt_type == EVT_WHEEL {
            if ev.y != 0 {
                send_mouse_raw(MOUSEEVENTF_WHEEL, 0, 0, ev.y * 120);
            }
            if ev.x != 0 {
                send_mouse_raw(MOUSEEVENTF_HWHEEL, 0, 0, ev.x * 120);
            }
            return;
        }

        // ── Move / down / up: ev.x, ev.y are absolute screen coordinates ─────
        // Only reposition the cursor for move events, or for down/up that carry
        // real coordinates. Some clients send button events with (0,0) — moving
        // there first made every click jump to the top-left corner.
        let max_rel_x = i64::from(vw.saturating_sub(1));
        let max_rel_y = i64::from(vh.saturating_sub(1));
        let rel_x = (i64::from(ev.x) - i64::from(vx)).clamp(0, max_rel_x);
        let rel_y = (i64::from(ev.y) - i64::from(vy)).clamp(0, max_rel_y);
        let abs_x = (rel_x * 65535 / max_rel_x.max(1)).clamp(0, 65535) as i32;
        let abs_y = (rel_y * 65535 / max_rel_y.max(1)).clamp(0, 65535) as i32;
        let do_move = evt_type == EVT_MOVE || ev.x != 0 || ev.y != 0;
        let mut flags = if do_move {
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK
        } else {
            windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS::default()
        };
        let mut mouse_data: i32 = 0;

        match evt_type {
            EVT_MOVE => {}
            EVT_DOWN => match button {
                BTN_LEFT => flags |= MOUSEEVENTF_LEFTDOWN,
                BTN_RIGHT => flags |= MOUSEEVENTF_RIGHTDOWN,
                BTN_MIDDLE => flags |= MOUSEEVENTF_MIDDLEDOWN,
                BTN_BACK => {
                    flags |= MOUSEEVENTF_XDOWN;
                    mouse_data = XBUTTON1;
                }
                BTN_FORWARD => {
                    flags |= MOUSEEVENTF_XDOWN;
                    mouse_data = XBUTTON2;
                }
                _ => {}
            },
            EVT_UP => match button {
                BTN_LEFT => flags |= MOUSEEVENTF_LEFTUP,
                BTN_RIGHT => flags |= MOUSEEVENTF_RIGHTUP,
                BTN_MIDDLE => flags |= MOUSEEVENTF_MIDDLEUP,
                BTN_BACK => {
                    flags |= MOUSEEVENTF_XUP;
                    mouse_data = XBUTTON1;
                }
                BTN_FORWARD => {
                    flags |= MOUSEEVENTF_XUP;
                    mouse_data = XBUTTON2;
                }
                _ => {}
            },
            _ => {}
        }

        let (dx, dy) = if do_move { (abs_x, abs_y) } else { (0, 0) };
        send_mouse_raw(flags, dx, dy, mouse_data);
    }
}

/// Low-level SendInput wrapper for a single mouse event.
#[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
unsafe fn send_mouse_raw(
    flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
    dx: i32,
    dy: i32,
    mouse_data: i32,
) {
    use std::mem::size_of;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEINPUT,
    };
    let mi = MOUSEINPUT {
        dx,
        dy,
        mouseData: mouse_data,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi },
    };
    SendInput(&[input], size_of::<INPUT>() as i32);
}

#[cfg(target_os = "linux")]
fn release_stuck_input() {
    linux_xtest::release_stuck_input();
}

// Android-хост: маршрутизируем мышь в службу доступности через JNI.
#[cfg(all(target_os = "android", feature = "android-client"))]
fn inject_mouse(ev: crate::rustdesk_proto::MouseEvent) {
    let kind = ev.mask & 0x7; // 0=move 1=down 2=up 3=wheel
    let button = ev.mask >> 3; // 1=left 2=right 4=wheel
    crate::android_input::on_mouse(kind, button, ev.x, ev.y);
}

#[cfg(all(
    not(target_os = "linux"),
    not(target_os = "macos"),
    not(all(target_os = "windows", feature = "live-vp9-mf")),
    not(all(target_os = "android", feature = "android-client")),
))]
fn inject_mouse(_ev: crate::rustdesk_proto::MouseEvent) {}

#[cfg(target_os = "macos")]
fn inject_mouse(ev: crate::rustdesk_proto::MouseEvent) {
    macos_cgevent::inject_mouse(ev);
}

#[cfg(target_os = "macos")]
fn inject_key(ev: crate::rustdesk_proto::KeyEvent) {
    macos_cgevent::inject_key(ev);
}

#[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
fn inject_key(ev: crate::rustdesk_proto::KeyEvent) {
    use std::mem::size_of;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };

    use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_UNICODE;

    unsafe {
        match &ev.union {
            // ── Control / named keys → virtual-key code ──────────────────────
            Some(crate::rustdesk_proto::key_event::Union::ControlKey(ck)) => {
                if *ck == crate::rustdesk_proto::ControlKey::CtrlAltDel as i32 {
                    // Preserve the semantic protocol command as a real chord.
                    // Windows may still reserve the secure-attention screen
                    // unless the installed service has the required policy,
                    // but this must never degrade to a bare Delete key.
                    send_key_vk(0x11, false);
                    send_key_vk(0x12, false);
                    send_key_vk(0x2E, false);
                    send_key_vk(0x2E, true);
                    send_key_vk(0x12, true);
                    send_key_vk(0x11, true);
                    return;
                }
                let vk = control_key_to_vk(*ck);
                if vk == 0 {
                    return;
                }
                let mut flags =
                    windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS::default();
                let key_up = !ev.down && !ev.press;
                if ev.press {
                    // press = (зажать модификаторы) → down → up → (отпустить
                    // модификаторы). Без этого Alt+←/→ (навигация 3 пальцами)
                    // и любые другие ControlKey+модификатор не работали — модификатор
                    // просто игнорировался и на хост шла голая клавиша.
                    let mod_vks: Vec<u16> = ev
                        .modifiers
                        .iter()
                        .map(|m| control_key_to_vk(*m))
                        .filter(|v| *v != 0)
                        .collect();
                    for mv in &mod_vks {
                        send_key_vk(*mv, false);
                    }
                    send_key_vk(vk, false);
                    send_key_vk(vk, true);
                    for mv in mod_vks.iter().rev() {
                        send_key_vk(*mv, true);
                    }
                    return;
                }
                if key_up {
                    flags |= KEYEVENTF_KEYUP;
                }
                let ki = KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                };
                let input = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 { ki },
                };
                SendInput(&[input], size_of::<INPUT>() as i32);
            }
            // ── Unicode character → KEYEVENTF_UNICODE (layout-independent) ────
            // This types the exact character the remote user pressed regardless
            // of the local keyboard layout (handles Cyrillic, symbols, etc.).
            Some(crate::rustdesk_proto::key_event::Union::Unicode(ch)) => {
                let Some(c) = char::from_u32(*ch) else { return };
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf).iter() {
                    // down
                    let ki = KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: *unit,
                        dwFlags: KEYEVENTF_UNICODE,
                        time: 0,
                        dwExtraInfo: 0,
                    };
                    SendInput(
                        &[INPUT {
                            r#type: INPUT_KEYBOARD,
                            Anonymous: INPUT_0 { ki },
                        }],
                        size_of::<INPUT>() as i32,
                    );
                    // up
                    let ki_up = KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: *unit,
                        dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    };
                    SendInput(
                        &[INPUT {
                            r#type: INPUT_KEYBOARD,
                            Anonymous: INPUT_0 { ki: ki_up },
                        }],
                        size_of::<INPUT>() as i32,
                    );
                }
            }
            None => {}
        }
    }
}

/// Send a single virtual-key down or up event.
#[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
unsafe fn send_key_vk(vk: u16, up: bool) {
    use std::mem::size_of;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY,
    };
    let flags = if up {
        KEYEVENTF_KEYUP
    } else {
        KEYBD_EVENT_FLAGS::default()
    };
    let ki = KEYBDINPUT {
        wVk: VIRTUAL_KEY(vk),
        wScan: 0,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
    };
    SendInput(
        &[INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki },
        }],
        size_of::<INPUT>() as i32,
    );
}

#[cfg(target_os = "linux")]
fn inject_mouse(ev: crate::rustdesk_proto::MouseEvent) {
    linux_xtest::inject_mouse(ev);
}

#[cfg(target_os = "linux")]
fn inject_key(ev: crate::rustdesk_proto::KeyEvent) {
    linux_xtest::inject_key(ev);
}

// Android-хост: управляющие клавиши → глобальные действия доступности через JNI.
#[cfg(all(target_os = "android", feature = "android-client"))]
fn inject_key(ev: crate::rustdesk_proto::KeyEvent) {
    if let Some(crate::rustdesk_proto::key_event::Union::ControlKey(code)) = ev.union {
        // Реагируем только на нажатие (press), чтобы не дублировать на down+up.
        if !ev.press && !ev.down {
            return;
        }
        let alt = ev
            .modifiers
            .iter()
            .any(|m| *m == crate::rustdesk_proto::ControlKey::Alt as i32);
        crate::android_input::on_key(code, alt);
    }
}

#[cfg(all(
    not(target_os = "linux"),
    not(target_os = "macos"),
    not(all(target_os = "windows", feature = "live-vp9-mf")),
    not(all(target_os = "android", feature = "android-client")),
))]
fn inject_key(_ev: crate::rustdesk_proto::KeyEvent) {}

#[cfg(target_os = "linux")]
mod linux_xtest {
    use std::{
        cell::RefCell,
        process::{Command, Stdio},
        sync::OnceLock,
    };

    use x11rb::{
        connection::Connection,
        protocol::{
            xproto::{self, ConnectionExt as XprotoConnectionExt},
            xtest::ConnectionExt as XtestConnectionExt,
        },
        rust_connection::RustConnection,
    };

    thread_local! {
        static X11_INPUT: RefCell<Option<X11Input>> = const { RefCell::new(None) };
    }

    struct X11Input {
        conn: RustConnection,
        root: u32,
    }

    impl X11Input {
        fn connect() -> Option<Self> {
            let (conn, screen_num) = x11rb::connect(None).ok()?;
            let root = conn.setup().roots[screen_num].root;
            Some(Self { conn, root })
        }
    }

    const EVT_MOVE: i32 = 0;
    const EVT_DOWN: i32 = 1;
    const EVT_UP: i32 = 2;
    const EVT_WHEEL: i32 = 3;
    const BTN_LEFT: i32 = 1;
    const BTN_RIGHT: i32 = 2;
    const BTN_MIDDLE: i32 = 4;
    const BTN_BACK: i32 = 8;
    const BTN_FORWARD: i32 = 16;

    pub fn inject_mouse(ev: crate::rustdesk_proto::MouseEvent) {
        if !with_x11(|conn, root| {
            let evt_type = ev.mask & 0x7;
            let button = ev.mask >> 3;

            match evt_type {
                EVT_MOVE => {
                    send_motion(&conn, root, ev.x, ev.y);
                }
                EVT_DOWN | EVT_UP => {
                    if ev.x != 0 || ev.y != 0 {
                        send_motion(&conn, root, ev.x, ev.y);
                    }
                    if let Some(detail) = mouse_button_detail(button) {
                        send_button(&conn, root, detail, evt_type == EVT_DOWN, ev.x, ev.y);
                    }
                }
                EVT_WHEEL => {
                    if ev.y != 0 {
                        click_button(&conn, root, if ev.y > 0 { 4 } else { 5 }, 0, 0);
                    }
                    if ev.x != 0 {
                        click_button(&conn, root, if ev.x > 0 { 7 } else { 6 }, 0, 0);
                    }
                }
                _ => {}
            };
            conn.flush().is_ok()
        }) {
            inject_mouse_fallback(ev);
        }
    }

    pub fn inject_key(ev: crate::rustdesk_proto::KeyEvent) {
        let ev_for_fallback = ev.clone();
        if !with_x11(|conn, root| {
            let modifiers = modifier_keycodes(&ev.modifiers);
            let apply_modifiers = ev.press;
            if apply_modifiers {
                press_modifiers(&conn, root, &modifiers, true);
            }
            match ev.union {
                Some(crate::rustdesk_proto::key_event::Union::ControlKey(ck)) => {
                    if let Some(keycode) = control_key_to_x11_keycode(ck) {
                        if ev.press {
                            send_keycode(&conn, root, keycode, true);
                            send_keycode(&conn, root, keycode, false);
                        } else {
                            send_keycode(&conn, root, keycode, ev.down);
                        }
                    }
                }
                Some(crate::rustdesk_proto::key_event::Union::Unicode(ch)) => {
                    if let Some(c) = char::from_u32(ch) {
                        if c.is_ascii() {
                            send_ascii(&conn, root, c);
                        } else {
                            let _ = type_text_with_tool(&c.to_string());
                        }
                    }
                }
                None => {}
            }
            if apply_modifiers {
                press_modifiers(&conn, root, &modifiers, false);
            }
            conn.flush().is_ok()
        }) {
            inject_key_fallback(ev_for_fallback);
        }
    }

    pub fn release_stuck_input() {
        if with_x11(|conn, root| {
            for button in [1, 2, 3, 8, 9] {
                send_button(&conn, root, button, false, 0, 0);
            }
            for keycode in [37, 50, 62, 64, 108, 133, 134] {
                send_keycode(&conn, root, keycode, false);
            }
            conn.flush().is_ok()
        }) {
            return;
        }
        for button in [0x80, 0x81, 0x82, 0x85, 0x86] {
            let _ = run_ydotool(["click", &format!("0x{button:02x}")]);
        }
        let _ = run_ydotool(["key", "29:0", "42:0", "54:0", "56:0", "125:0", "126:0"]);
    }

    fn with_x11(mut f: impl FnMut(&RustConnection, u32) -> bool) -> bool {
        X11_INPUT.with(|cell| {
            if cell.borrow().is_none() {
                *cell.borrow_mut() = X11Input::connect();
            }
            let ok = {
                let guard = cell.borrow();
                let Some(input) = guard.as_ref() else {
                    return false;
                };
                f(&input.conn, input.root)
            };
            if !ok {
                *cell.borrow_mut() = None;
            }
            ok
        })
    }

    fn send_motion<C: Connection>(conn: &C, root: u32, x: i32, y: i32) {
        let _ = conn.xtest_fake_input(
            xproto::MOTION_NOTIFY_EVENT,
            0,
            0,
            root,
            clamp_i16(x),
            clamp_i16(y),
            0,
        );
    }

    fn send_button<C: Connection>(conn: &C, root: u32, detail: u8, down: bool, x: i32, y: i32) {
        let event = if down {
            xproto::BUTTON_PRESS_EVENT
        } else {
            xproto::BUTTON_RELEASE_EVENT
        };
        let _ = conn.xtest_fake_input(event, detail, 0, root, clamp_i16(x), clamp_i16(y), 0);
    }

    fn click_button<C: Connection>(conn: &C, root: u32, detail: u8, x: i32, y: i32) {
        send_button(conn, root, detail, true, x, y);
        send_button(conn, root, detail, false, x, y);
    }

    fn mouse_button_detail(button: i32) -> Option<u8> {
        match button {
            BTN_LEFT => Some(1),
            BTN_MIDDLE => Some(2),
            BTN_RIGHT => Some(3),
            BTN_BACK => Some(8),
            BTN_FORWARD => Some(9),
            _ => None,
        }
    }

    fn send_keycode<C: Connection>(conn: &C, root: u32, keycode: u8, down: bool) {
        let event = if down {
            xproto::KEY_PRESS_EVENT
        } else {
            xproto::KEY_RELEASE_EVENT
        };
        let _ = conn.xtest_fake_input(event, keycode, 0, root, 0, 0, 0);
    }

    fn send_ascii<C>(conn: &C, root: u32, c: char)
    where
        C: Connection + XprotoConnectionExt,
    {
        let Some((keycode, shift)) = keycode_for_ascii(conn, c).or_else(|| ascii_to_x11_keycode(c))
        else {
            return;
        };
        if shift {
            send_keycode(conn, root, 50, true);
        }
        send_keycode(conn, root, keycode, true);
        send_keycode(conn, root, keycode, false);
        if shift {
            send_keycode(conn, root, 50, false);
        }
    }

    fn press_modifiers<C: Connection>(conn: &C, root: u32, keycodes: &[u8], down: bool) {
        let iter: Box<dyn Iterator<Item = &u8> + '_> = if down {
            Box::new(keycodes.iter())
        } else {
            Box::new(keycodes.iter().rev())
        };
        for keycode in iter {
            send_keycode(conn, root, *keycode, down);
        }
    }

    fn modifier_keycodes(modifiers: &[i32]) -> Vec<u8> {
        let mut out = Vec::new();
        for modifier in modifiers {
            if let Some(keycode) = control_key_to_x11_keycode(*modifier) {
                if !out.contains(&keycode) {
                    out.push(keycode);
                }
            }
        }
        out
    }

    fn keycode_for_ascii<C>(conn: &C, c: char) -> Option<(u8, bool)>
    where
        C: Connection + XprotoConnectionExt,
    {
        let setup = conn.setup();
        let min = setup.min_keycode;
        let count = setup.max_keycode.saturating_sub(min).saturating_add(1);
        let reply = conn.get_keyboard_mapping(min, count).ok()?.reply().ok()?;
        let per_key = reply.keysyms_per_keycode as usize;
        if per_key == 0 {
            return None;
        }
        let target = c as u32;
        for (index, keysyms) in reply.keysyms.chunks(per_key).enumerate() {
            if keysyms.first().copied() == Some(target) {
                return Some((min.saturating_add(index as u8), false));
            }
            if keysyms.get(1).copied() == Some(target) {
                return Some((min.saturating_add(index as u8), true));
            }
        }
        None
    }

    fn type_text_with_xdotool(text: &str) -> bool {
        run_tool("xdotool", ["type", "--clearmodifiers", "--", text])
    }

    fn type_text_with_tool(text: &str) -> bool {
        type_text_with_xdotool(text) || run_ydotool(["type", text]) || run_tool("wtype", [text])
    }

    fn inject_mouse_fallback(ev: crate::rustdesk_proto::MouseEvent) {
        let evt_type = ev.mask & 0x7;
        let button = ev.mask >> 3;
        match evt_type {
            EVT_MOVE => {
                let _ = run_ydotool([
                    "mousemove",
                    "--absolute",
                    &ev.x.to_string(),
                    &ev.y.to_string(),
                ]);
            }
            EVT_DOWN | EVT_UP => {
                if ev.x != 0 || ev.y != 0 {
                    let _ = run_ydotool([
                        "mousemove",
                        "--absolute",
                        &ev.x.to_string(),
                        &ev.y.to_string(),
                    ]);
                }
                if let Some(button) = ydotool_button(button) {
                    let state = if evt_type == EVT_DOWN { 0x40 } else { 0x80 };
                    let _ = run_ydotool(["click", &format!("0x{:02x}", state | button)]);
                }
            }
            _ => {}
        }
    }

    fn inject_key_fallback(ev: crate::rustdesk_proto::KeyEvent) {
        match ev.union {
            Some(crate::rustdesk_proto::key_event::Union::ControlKey(ck)) => {
                if let Some(code) = control_key_to_evdev(ck) {
                    if ev.press {
                        send_evdev_key_press(code, &ev.modifiers);
                    } else {
                        let _ = run_ydotool(["key", &format!("{code}:{}", i32::from(ev.down))]);
                    }
                }
            }
            Some(crate::rustdesk_proto::key_event::Union::Unicode(ch)) => {
                let Some(c) = char::from_u32(ch) else {
                    return;
                };
                if ev.press && !ev.modifiers.is_empty() {
                    if let Some((code, needs_shift)) = ascii_to_evdev(c) {
                        let mut modifiers = ev.modifiers.clone();
                        if needs_shift {
                            modifiers.push(crate::rustdesk_proto::ControlKey::Shift as i32);
                        }
                        send_evdev_key_press(code, &modifiers);
                        return;
                    }
                }
                let _ = type_text_with_tool(&c.to_string());
            }
            None => {}
        }
    }

    fn send_evdev_key_press(code: u16, modifiers: &[i32]) {
        let mut args = vec!["key".to_owned()];
        let mut modifier_codes = modifiers
            .iter()
            .filter_map(|modifier| control_key_to_evdev(*modifier))
            .collect::<Vec<_>>();
        modifier_codes.sort_unstable();
        modifier_codes.dedup();
        for modifier in &modifier_codes {
            args.push(format!("{modifier}:1"));
        }
        args.push(format!("{code}:1"));
        args.push(format!("{code}:0"));
        for modifier in modifier_codes.iter().rev() {
            args.push(format!("{modifier}:0"));
        }
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        let _ = run_ydotool(refs);
    }

    fn ydotool_button(button: i32) -> Option<u8> {
        match button {
            BTN_LEFT => Some(0x00),
            BTN_RIGHT => Some(0x01),
            BTN_MIDDLE => Some(0x02),
            BTN_FORWARD => Some(0x05),
            BTN_BACK => Some(0x06),
            _ => None,
        }
    }

    fn control_key_to_evdev(ck: i32) -> Option<u16> {
        use crate::rustdesk_proto::ControlKey;
        Some(match ck {
            x if x == ControlKey::Alt as i32 => 56,
            x if x == ControlKey::Backspace as i32 => 14,
            x if x == ControlKey::CapsLock as i32 => 58,
            x if x == ControlKey::Control as i32 => 29,
            x if x == ControlKey::Delete as i32 => 111,
            x if x == ControlKey::DownArrow as i32 => 108,
            x if x == ControlKey::End as i32 => 107,
            x if x == ControlKey::Escape as i32 => 1,
            x if x == ControlKey::F1 as i32 => 59,
            x if x == ControlKey::F2 as i32 => 60,
            x if x == ControlKey::F3 as i32 => 61,
            x if x == ControlKey::F4 as i32 => 62,
            x if x == ControlKey::F5 as i32 => 63,
            x if x == ControlKey::F6 as i32 => 64,
            x if x == ControlKey::F7 as i32 => 65,
            x if x == ControlKey::F8 as i32 => 66,
            x if x == ControlKey::F9 as i32 => 67,
            x if x == ControlKey::F10 as i32 => 68,
            x if x == ControlKey::F11 as i32 => 87,
            x if x == ControlKey::F12 as i32 => 88,
            x if x == ControlKey::Home as i32 => 102,
            x if x == ControlKey::Insert as i32 => 110,
            x if x == ControlKey::LeftArrow as i32 => 105,
            x if x == ControlKey::Meta as i32 => 125,
            x if x == ControlKey::PageDown as i32 => 109,
            x if x == ControlKey::PageUp as i32 => 104,
            x if x == ControlKey::Return as i32 => 28,
            x if x == ControlKey::NumpadEnter as i32 => 96,
            x if x == ControlKey::RightArrow as i32 => 106,
            x if x == ControlKey::Shift as i32 => 42,
            x if x == ControlKey::Space as i32 => 57,
            x if x == ControlKey::Tab as i32 => 15,
            x if x == ControlKey::UpArrow as i32 => 103,
            _ => return None,
        })
    }

    fn ascii_to_evdev(c: char) -> Option<(u16, bool)> {
        Some(match c {
            'a'..='z' => (letter_evdev(c), false),
            'A'..='Z' => (letter_evdev(c.to_ascii_lowercase()), true),
            '1' => (2, false),
            '2' => (3, false),
            '3' => (4, false),
            '4' => (5, false),
            '5' => (6, false),
            '6' => (7, false),
            '7' => (8, false),
            '8' => (9, false),
            '9' => (10, false),
            '0' => (11, false),
            '!' => (2, true),
            '@' => (3, true),
            '#' => (4, true),
            '$' => (5, true),
            '%' => (6, true),
            '^' => (7, true),
            '&' => (8, true),
            '*' => (9, true),
            '(' => (10, true),
            ')' => (11, true),
            '-' => (12, false),
            '_' => (12, true),
            '=' => (13, false),
            '+' => (13, true),
            '[' => (26, false),
            '{' => (26, true),
            ']' => (27, false),
            '}' => (27, true),
            ';' => (39, false),
            ':' => (39, true),
            '\'' => (40, false),
            '"' => (40, true),
            '`' => (41, false),
            '~' => (41, true),
            '\\' => (43, false),
            '|' => (43, true),
            ',' => (51, false),
            '<' => (51, true),
            '.' => (52, false),
            '>' => (52, true),
            '/' => (53, false),
            '?' => (53, true),
            ' ' => (57, false),
            _ => return None,
        })
    }

    fn letter_evdev(c: char) -> u16 {
        match c {
            'q' => 16,
            'w' => 17,
            'e' => 18,
            'r' => 19,
            't' => 20,
            'y' => 21,
            'u' => 22,
            'i' => 23,
            'o' => 24,
            'p' => 25,
            'a' => 30,
            's' => 31,
            'd' => 32,
            'f' => 33,
            'g' => 34,
            'h' => 35,
            'j' => 36,
            'k' => 37,
            'l' => 38,
            'z' => 44,
            'x' => 45,
            'c' => 46,
            'v' => 47,
            'b' => 48,
            'n' => 49,
            'm' => 50,
            _ => 0,
        }
    }

    fn run_ydotool<I, S>(args: I) -> bool
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        run_tool("ydotool", args)
    }

    fn run_tool<I, S>(name: &str, args: I) -> bool
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if !command_exists(name) {
            return false;
        }
        let mut command = Command::new(name);
        for arg in args {
            command.arg(arg.as_ref());
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn command_exists(name: &str) -> bool {
        static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<String, bool>>> =
            OnceLock::new();
        let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
        if let Ok(cache) = cache.lock() {
            if let Some(value) = cache.get(name).copied() {
                return value;
            }
        }
        let exists = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
            .unwrap_or(false);
        if let Ok(mut cache) = cache.lock() {
            cache.insert(name.to_owned(), exists);
        }
        exists
    }

    fn control_key_to_x11_keycode(ck: i32) -> Option<u8> {
        use crate::rustdesk_proto::ControlKey;
        Some(match ck {
            x if x == ControlKey::Alt as i32 => 64,
            x if x == ControlKey::Backspace as i32 => 22,
            x if x == ControlKey::CapsLock as i32 => 66,
            x if x == ControlKey::Control as i32 => 37,
            x if x == ControlKey::Delete as i32 => 119,
            x if x == ControlKey::DownArrow as i32 => 116,
            x if x == ControlKey::End as i32 => 115,
            x if x == ControlKey::Escape as i32 => 9,
            x if x == ControlKey::F1 as i32 => 67,
            x if x == ControlKey::F2 as i32 => 68,
            x if x == ControlKey::F3 as i32 => 69,
            x if x == ControlKey::F4 as i32 => 70,
            x if x == ControlKey::F5 as i32 => 71,
            x if x == ControlKey::F6 as i32 => 72,
            x if x == ControlKey::F7 as i32 => 73,
            x if x == ControlKey::F8 as i32 => 74,
            x if x == ControlKey::F9 as i32 => 75,
            x if x == ControlKey::F10 as i32 => 76,
            x if x == ControlKey::F11 as i32 => 95,
            x if x == ControlKey::F12 as i32 => 96,
            x if x == ControlKey::Home as i32 => 110,
            x if x == ControlKey::Insert as i32 => 118,
            x if x == ControlKey::LeftArrow as i32 => 113,
            x if x == ControlKey::Meta as i32 => 133,
            x if x == ControlKey::PageDown as i32 => 117,
            x if x == ControlKey::PageUp as i32 => 112,
            x if x == ControlKey::Return as i32 => 36,
            x if x == ControlKey::NumpadEnter as i32 => 104,
            x if x == ControlKey::RightArrow as i32 => 114,
            x if x == ControlKey::Shift as i32 => 50,
            x if x == ControlKey::Space as i32 => 65,
            x if x == ControlKey::Tab as i32 => 23,
            x if x == ControlKey::UpArrow as i32 => 111,
            _ => return None,
        })
    }

    fn ascii_to_x11_keycode(c: char) -> Option<(u8, bool)> {
        Some(match c {
            'a'..='z' => ((c as u8 - b'a') + 38, false),
            'A'..='Z' => ((c as u8 - b'A') + 38, true),
            '1' => (10, false),
            '2' => (11, false),
            '3' => (12, false),
            '4' => (13, false),
            '5' => (14, false),
            '6' => (15, false),
            '7' => (16, false),
            '8' => (17, false),
            '9' => (18, false),
            '0' => (19, false),
            '!' => (10, true),
            '@' => (11, true),
            '#' => (12, true),
            '$' => (13, true),
            '%' => (14, true),
            '^' => (15, true),
            '&' => (16, true),
            '*' => (17, true),
            '(' => (18, true),
            ')' => (19, true),
            '-' => (20, false),
            '_' => (20, true),
            '=' => (21, false),
            '+' => (21, true),
            '[' => (34, false),
            '{' => (34, true),
            ']' => (35, false),
            '}' => (35, true),
            ';' => (47, false),
            ':' => (47, true),
            '\'' => (48, false),
            '"' => (48, true),
            '`' => (49, false),
            '~' => (49, true),
            '\\' => (51, false),
            '|' => (51, true),
            ',' => (59, false),
            '<' => (59, true),
            '.' => (60, false),
            '>' => (60, true),
            '/' => (61, false),
            '?' => (61, true),
            ' ' => (65, false),
            _ => return None,
        })
    }

    fn clamp_i16(value: i32) -> i16 {
        value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// macOS host input injection — Quartz Event Services (CGEvent)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Mouse and keyboard events from the remote client are replayed on the local
// Mac via CGEventPost(kCGHIDEventTap). The host process must hold Accessibility
// permission (System Settings → Privacy & Security → Accessibility); without it
// macOS silently drops the posted events. Screen capture additionally needs
// Screen Recording permission.
#[cfg(target_os = "macos")]
mod macos_cgevent {
    use std::cell::Cell;
    use std::ffi::c_void;

    type CGEventRef = *mut c_void;
    type CGEventSourceRef = *mut c_void;
    type CGDisplayModeRef = *mut c_void;
    type CGDirectDisplayID = u32;
    type CGFloat = f64;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: CGFloat,
        y: CGFloat,
    }

    // CGEventType
    const K_LEFT_MOUSE_DOWN: u32 = 1;
    const K_LEFT_MOUSE_UP: u32 = 2;
    const K_RIGHT_MOUSE_DOWN: u32 = 3;
    const K_RIGHT_MOUSE_UP: u32 = 4;
    const K_MOUSE_MOVED: u32 = 5;
    const K_LEFT_MOUSE_DRAGGED: u32 = 6;
    const K_RIGHT_MOUSE_DRAGGED: u32 = 7;
    const K_OTHER_MOUSE_DOWN: u32 = 25;
    const K_OTHER_MOUSE_UP: u32 = 26;
    const K_OTHER_MOUSE_DRAGGED: u32 = 27;

    // CGMouseButton
    const BTN_LEFT: u32 = 0;
    const BTN_RIGHT: u32 = 1;
    const BTN_CENTER: u32 = 2;

    // CGEventTapLocation
    const K_HID_EVENT_TAP: u32 = 0;

    // CGScrollEventUnit
    const K_SCROLL_UNIT_LINE: u32 = 1;

    // CGEventFlags (modifier masks)
    const FLAG_SHIFT: u64 = 0x0002_0000;
    const FLAG_CONTROL: u64 = 0x0004_0000;
    const FLAG_ALTERNATE: u64 = 0x0008_0000;
    const FLAG_COMMAND: u64 = 0x0010_0000;

    // Protocol mouse event sub-types (mask & 0x7)
    const EVT_MOVE: i32 = 0;
    const EVT_DOWN: i32 = 1;
    const EVT_UP: i32 = 2;
    const EVT_WHEEL: i32 = 3;
    // Protocol mouse buttons (mask >> 3)
    const MB_LEFT: i32 = 1;
    const MB_RIGHT: i32 = 2;
    const MB_MIDDLE: i32 = 4;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreateMouseEvent(
            source: CGEventSourceRef,
            mouse_type: u32,
            point: CGPoint,
            button: u32,
        ) -> CGEventRef;
        fn CGEventCreateKeyboardEvent(
            source: CGEventSourceRef,
            keycode: u16,
            key_down: bool,
        ) -> CGEventRef;
        fn CGEventCreateScrollWheelEvent(
            source: CGEventSourceRef,
            units: u32,
            wheel_count: u32,
            ...
        ) -> CGEventRef;
        fn CGEventKeyboardSetUnicodeString(
            event: CGEventRef,
            length: usize,
            unicode_string: *const u16,
        );
        fn CGEventSetFlags(event: CGEventRef, flags: u64);
        fn CGEventPost(tap: u32, event: CGEventRef);

        fn CGMainDisplayID() -> CGDirectDisplayID;
        fn CGDisplayCopyDisplayMode(display: CGDirectDisplayID) -> CGDisplayModeRef;
        fn CGDisplayModeGetWidth(mode: CGDisplayModeRef) -> usize;
        fn CGDisplayModeGetPixelWidth(mode: CGDisplayModeRef) -> usize;
        fn CGDisplayModeRelease(mode: CGDisplayModeRef);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        // Screen Recording permission (macOS 10.15+). Preflight reports current
        // state without prompting; Request shows the system prompt once.
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        // Accessibility permission — required for CGEventPost to take effect.
        fn AXIsProcessTrusted() -> bool;
    }

    /// Check the two macOS permissions host mode depends on and report clear,
    /// actionable hints to the host console. Triggers the Screen Recording
    /// system prompt once (harmless if already granted). Accessibility cannot be
    /// prompted without a focused app, so we only point the user at the setting.
    pub fn preflight_permissions() -> (bool, Vec<String>) {
        let mut hints = Vec::new();
        let (screen_ok, ax_ok) = unsafe {
            let screen = CGPreflightScreenCaptureAccess();
            if !screen {
                // Ask once; the prompt appears on first run.
                let _ = CGRequestScreenCaptureAccess();
            }
            (screen, AXIsProcessTrusted())
        };
        if !screen_ok {
            hints.push(
                "⚠ Нет доступа к записи экрана. System Settings → Privacy & Security → \
                 Screen Recording → включи EvertyDesk Lite, затем перезапусти."
                    .to_owned(),
            );
        }
        if !ax_ok {
            hints.push(
                "⚠ Нет доступа Accessibility — мышь/клавиатура от клиента не сработают. \
                 System Settings → Privacy & Security → Accessibility → включи \
                 EvertyDesk Lite."
                    .to_owned(),
            );
        }
        (screen_ok && ax_ok, hints)
    }

    /// Non-prompting permission status for the GUI: `(screen_recording,
    /// accessibility)`. Cheap — safe to poll every frame; never shows a dialog.
    pub fn permission_status() -> (bool, bool) {
        unsafe { (CGPreflightScreenCaptureAccess(), AXIsProcessTrusted()) }
    }

    thread_local! {
        // Last cursor position in display *points*, so button events that arrive
        // without coordinates (x=y=0) land where the cursor actually is.
        static LAST_POS: Cell<(f64, f64)> = const { Cell::new((0.0, 0.0)) };
    }

    /// Retina scale: the captured frame is in native pixels, but CGEvent works
    /// in points. `points = pixels * (mode_width / mode_pixel_width)`.
    fn pixel_to_point_scale() -> f64 {
        unsafe {
            let mode = CGDisplayCopyDisplayMode(CGMainDisplayID());
            if mode.is_null() {
                return 1.0;
            }
            let points = CGDisplayModeGetWidth(mode) as f64;
            let pixels = CGDisplayModeGetPixelWidth(mode) as f64;
            CGDisplayModeRelease(mode);
            if pixels > 0.0 && points > 0.0 {
                points / pixels
            } else {
                1.0
            }
        }
    }

    fn post(event: CGEventRef) {
        if event.is_null() {
            return;
        }
        unsafe {
            CGEventPost(K_HID_EVENT_TAP, event);
            CFRelease(event);
        }
    }

    fn post_mouse(mouse_type: u32, point: CGPoint, button: u32) {
        let ev =
            unsafe { CGEventCreateMouseEvent(std::ptr::null_mut(), mouse_type, point, button) };
        post(ev);
    }

    pub fn inject_mouse(ev: crate::rustdesk_proto::MouseEvent) {
        let evt_type = ev.mask & 0x7;
        let button = ev.mask >> 3;
        let scale = pixel_to_point_scale();

        // Resolve target point: use supplied pixel coords (scaled to points)
        // when present, otherwise fall back to the last known position.
        let point = if ev.x != 0 || ev.y != 0 {
            let p = CGPoint {
                x: ev.x as f64 * scale,
                y: ev.y as f64 * scale,
            };
            LAST_POS.with(|c| c.set((p.x, p.y)));
            p
        } else {
            let (x, y) = LAST_POS.with(|c| c.get());
            CGPoint { x, y }
        };

        match evt_type {
            EVT_MOVE => post_mouse(K_MOUSE_MOVED, point, BTN_LEFT),
            EVT_DOWN | EVT_UP => {
                let down = evt_type == EVT_DOWN;
                let (mtype, cg_btn) = match button {
                    MB_LEFT => (
                        if down {
                            K_LEFT_MOUSE_DOWN
                        } else {
                            K_LEFT_MOUSE_UP
                        },
                        BTN_LEFT,
                    ),
                    MB_RIGHT => (
                        if down {
                            K_RIGHT_MOUSE_DOWN
                        } else {
                            K_RIGHT_MOUSE_UP
                        },
                        BTN_RIGHT,
                    ),
                    MB_MIDDLE => (
                        if down {
                            K_OTHER_MOUSE_DOWN
                        } else {
                            K_OTHER_MOUSE_UP
                        },
                        BTN_CENTER,
                    ),
                    _ => return,
                };
                post_mouse(mtype, point, cg_btn);
            }
            EVT_WHEEL => {
                // Protocol wheel deltas are in client units; one notch ≈ ±1 line.
                if ev.y != 0 {
                    let delta = (ev.y.signum() * ((ev.y.abs() / 40).max(1))).clamp(-10, 10);
                    let scroll = unsafe {
                        CGEventCreateScrollWheelEvent(
                            std::ptr::null_mut(),
                            K_SCROLL_UNIT_LINE,
                            1,
                            delta,
                        )
                    };
                    post(scroll);
                }
            }
            _ => {}
        }
    }

    pub fn inject_key(ev: crate::rustdesk_proto::KeyEvent) {
        let flags = modifier_flags(&ev.modifiers);
        match ev.union {
            Some(crate::rustdesk_proto::key_event::Union::ControlKey(ck)) => {
                let Some(keycode) = control_key_to_mac_keycode(ck) else {
                    return;
                };
                if ev.press {
                    post_key(keycode, true, flags);
                    post_key(keycode, false, flags);
                } else {
                    post_key(keycode, ev.down, flags);
                }
            }
            Some(crate::rustdesk_proto::key_event::Union::Unicode(ch)) => {
                let Some(c) = char::from_u32(ch) else { return };
                // When a non-shift modifier is held (Cmd/Ctrl/Alt), route through
                // a real keycode so OS shortcuts (⌘C, ⌘V, …) fire. Otherwise type
                // the literal character — layout-independent, handles Cyrillic etc.
                let has_command_modifier = ev
                    .modifiers
                    .iter()
                    .any(|m| *m != crate::rustdesk_proto::ControlKey::Shift as i32);
                if has_command_modifier {
                    if let Some(keycode) = ascii_to_mac_keycode(c) {
                        post_key(keycode, true, flags);
                        post_key(keycode, false, flags);
                        return;
                    }
                }
                post_unicode(c);
            }
            None => {}
        }
    }

    fn post_key(keycode: u16, key_down: bool, flags: u64) {
        let ev = unsafe { CGEventCreateKeyboardEvent(std::ptr::null_mut(), keycode, key_down) };
        if ev.is_null() {
            return;
        }
        if flags != 0 {
            unsafe { CGEventSetFlags(ev, flags) };
        }
        post(ev);
    }

    fn post_unicode(c: char) {
        let mut buf = [0u16; 2];
        let utf16 = c.encode_utf16(&mut buf);
        let units: Vec<u16> = utf16.iter().copied().collect();
        unsafe {
            let down = CGEventCreateKeyboardEvent(std::ptr::null_mut(), 0, true);
            if !down.is_null() {
                CGEventKeyboardSetUnicodeString(down, units.len(), units.as_ptr());
                post(down);
            }
            let up = CGEventCreateKeyboardEvent(std::ptr::null_mut(), 0, false);
            if !up.is_null() {
                CGEventKeyboardSetUnicodeString(up, units.len(), units.as_ptr());
                post(up);
            }
        }
    }

    fn modifier_flags(modifiers: &[i32]) -> u64 {
        use crate::rustdesk_proto::ControlKey;
        let mut flags = 0u64;
        for m in modifiers {
            if *m == ControlKey::Shift as i32 {
                flags |= FLAG_SHIFT;
            } else if *m == ControlKey::Control as i32 {
                flags |= FLAG_CONTROL;
            } else if *m == ControlKey::Alt as i32 {
                flags |= FLAG_ALTERNATE;
            } else if *m == ControlKey::Meta as i32 {
                flags |= FLAG_COMMAND;
            }
        }
        flags
    }

    pub fn release_stuck_input() {
        // Release any held modifier keys and mouse buttons so a dropped session
        // never leaves the Mac with a stuck ⌘/⇧/button.
        for keycode in [55u16, 56, 58, 59, 57] {
            post_key(keycode, false, 0);
        }
        let (x, y) = LAST_POS.with(|c| c.get());
        let point = CGPoint { x, y };
        post_mouse(K_LEFT_MOUSE_UP, point, BTN_LEFT);
        post_mouse(K_RIGHT_MOUSE_UP, point, BTN_RIGHT);
        post_mouse(K_OTHER_MOUSE_UP, point, BTN_CENTER);
    }

    /// Map a RustDesk ControlKey to a macOS virtual keycode (kVK_*).
    fn control_key_to_mac_keycode(ck: i32) -> Option<u16> {
        use crate::rustdesk_proto::ControlKey;
        Some(match ck {
            x if x == ControlKey::Alt as i32 => 58,
            x if x == ControlKey::Backspace as i32 => 51,
            x if x == ControlKey::CapsLock as i32 => 57,
            x if x == ControlKey::Control as i32 => 59,
            x if x == ControlKey::Delete as i32 => 117,
            x if x == ControlKey::DownArrow as i32 => 125,
            x if x == ControlKey::End as i32 => 119,
            x if x == ControlKey::Escape as i32 => 53,
            x if x == ControlKey::F1 as i32 => 122,
            x if x == ControlKey::F2 as i32 => 120,
            x if x == ControlKey::F3 as i32 => 99,
            x if x == ControlKey::F4 as i32 => 118,
            x if x == ControlKey::F5 as i32 => 96,
            x if x == ControlKey::F6 as i32 => 97,
            x if x == ControlKey::F7 as i32 => 98,
            x if x == ControlKey::F8 as i32 => 100,
            x if x == ControlKey::F9 as i32 => 101,
            x if x == ControlKey::F10 as i32 => 109,
            x if x == ControlKey::F11 as i32 => 103,
            x if x == ControlKey::F12 as i32 => 111,
            x if x == ControlKey::Home as i32 => 115,
            x if x == ControlKey::Insert as i32 => 114,
            x if x == ControlKey::LeftArrow as i32 => 123,
            x if x == ControlKey::Meta as i32 => 55,
            x if x == ControlKey::PageDown as i32 => 121,
            x if x == ControlKey::PageUp as i32 => 116,
            x if x == ControlKey::Return as i32 => 36,
            x if x == ControlKey::NumpadEnter as i32 => 76,
            x if x == ControlKey::RightArrow as i32 => 124,
            x if x == ControlKey::Shift as i32 => 56,
            x if x == ControlKey::Space as i32 => 49,
            x if x == ControlKey::Tab as i32 => 48,
            x if x == ControlKey::UpArrow as i32 => 126,
            _ => return None,
        })
    }

    /// Map an ASCII letter/digit to its kVK_ANSI_* virtual keycode so that
    /// modifier shortcuts (⌘C, ⌃A, …) reach the right key. Layout-dependent —
    /// only used for the shortcut path; plain text goes through post_unicode.
    fn ascii_to_mac_keycode(c: char) -> Option<u16> {
        let lower = c.to_ascii_lowercase();
        Some(match lower {
            'a' => 0,
            'b' => 11,
            'c' => 8,
            'd' => 2,
            'e' => 14,
            'f' => 3,
            'g' => 5,
            'h' => 4,
            'i' => 34,
            'j' => 38,
            'k' => 40,
            'l' => 37,
            'm' => 46,
            'n' => 45,
            'o' => 31,
            'p' => 35,
            'q' => 12,
            'r' => 15,
            's' => 1,
            't' => 17,
            'u' => 32,
            'v' => 9,
            'w' => 13,
            'x' => 7,
            'y' => 16,
            'z' => 6,
            '0' => 29,
            '1' => 18,
            '2' => 19,
            '3' => 20,
            '4' => 21,
            '5' => 23,
            '6' => 22,
            '7' => 26,
            '8' => 28,
            '9' => 25,
            _ => return None,
        })
    }
}

/// Map RustDesk ControlKey enum value to Windows VK_ code.
#[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
fn control_key_to_vk(ck: i32) -> u16 {
    use crate::rustdesk_proto::ControlKey;
    // ControlKey variants are i32 from prost; cast and match.
    match ck {
        x if x == ControlKey::Alt as i32 => 0x12,
        x if x == ControlKey::Backspace as i32 => 0x08,
        x if x == ControlKey::CapsLock as i32 => 0x14,
        x if x == ControlKey::Control as i32 => 0x11,
        x if x == ControlKey::Delete as i32 => 0x2E,
        x if x == ControlKey::End as i32 => 0x23,
        x if x == ControlKey::Escape as i32 => 0x1B,
        x if x == ControlKey::Home as i32 => 0x24,
        x if x == ControlKey::LeftArrow as i32 => 0x25,
        x if x == ControlKey::UpArrow as i32 => 0x26,
        x if x == ControlKey::RightArrow as i32 => 0x27,
        x if x == ControlKey::DownArrow as i32 => 0x28,
        x if x == ControlKey::Return as i32 => 0x0D,
        x if x == ControlKey::PageUp as i32 => 0x21,
        x if x == ControlKey::PageDown as i32 => 0x22,
        x if x == ControlKey::Shift as i32 => 0x10,
        x if x == ControlKey::Space as i32 => 0x20,
        x if x == ControlKey::Tab as i32 => 0x09,
        x if x == ControlKey::F1 as i32 => 0x70,
        x if x == ControlKey::F2 as i32 => 0x71,
        x if x == ControlKey::F3 as i32 => 0x72,
        x if x == ControlKey::F4 as i32 => 0x73,
        x if x == ControlKey::F5 as i32 => 0x74,
        x if x == ControlKey::F6 as i32 => 0x75,
        x if x == ControlKey::F7 as i32 => 0x76,
        x if x == ControlKey::F8 as i32 => 0x77,
        x if x == ControlKey::F9 as i32 => 0x78,
        x if x == ControlKey::F10 as i32 => 0x79,
        x if x == ControlKey::F11 as i32 => 0x7A,
        x if x == ControlKey::F12 as i32 => 0x7B,
        x if x == ControlKey::Meta as i32 => 0x5B, // Win key
        x if x == ControlKey::CtrlAltDel as i32 => 0x2E, // just Del; real CAD needs UAC bypass
        _ => 0,
    }
}

// ── Auth helpers ──────────────────────────────────────────────────────────────

fn verify_password(received: &[u8], our_password: &str, salt: &str, challenge: &str) -> bool {
    // Empty received hash means "remote approval" mode.
    if received.is_empty() {
        return our_password.is_empty();
    }
    // Match the algorithm used by build_login_request in transport.rs:
    // h1 = sha256(password || salt)
    // h2 = sha256(h1 || challenge)
    let mut h1 = Sha256::new();
    h1.update(our_password.as_bytes());
    h1.update(salt.as_bytes());
    let h1 = h1.finalize();

    let mut h2 = Sha256::new();
    h2.update(h1.as_slice());
    h2.update(challenge.as_bytes());
    let expected = h2.finalize();

    received == expected.as_slice()
}

fn verify_access_password(
    received: &[u8],
    config: &AppConfig,
    salt: &str,
    challenge: &str,
) -> bool {
    verify_password(received, &config.local_password, salt, challenge)
        || (!config.permanent_password.trim().is_empty()
            && verify_password(received, &config.permanent_password, salt, challenge))
}

// ── Utility ───────────────────────────────────────────────────────────────────

fn send_register_peer_udp(socket: &UdpSocket, server: &str, local_id: &str) -> Result<(), String> {
    let msg = RendezvousMessage {
        union: Some(rendezvous_message::Union::RegisterPeer(RegisterPeer {
            id: local_id.to_owned(),
            serial: 0,
        })),
    };
    socket
        .send_to(&encode_message(&msg), server)
        .map(|_| ())
        .map_err(|e| format!("UDP send_to: {e}"))
}

/// Sends `RegisterPk`. `sign_pk` is our stable Ed25519 public key (32 bytes);
/// if it isn't a valid 32-byte key we fall back to a deterministic fake so the
/// call still does something sensible.
fn send_register_pk_udp(
    socket: &UdpSocket,
    server: &str,
    local_id: &str,
    sign_pk: &[u8],
) -> Result<(), String> {
    use sha2::{Digest, Sha256};

    let pk: Vec<u8> = if sign_pk.len() == 32 {
        sign_pk.to_vec()
    } else {
        let mut h = Sha256::new();
        h.update(b"evertydesk-pk:");
        h.update(local_id.as_bytes());
        h.finalize().to_vec()
    };

    let mut hu = Sha256::new();
    hu.update(b"evertydesk-uuid:");
    hu.update(local_id.as_bytes());
    let uuid_bytes: Vec<u8> = hu.finalize()[..16].to_vec();

    let msg = RendezvousMessage {
        union: Some(rendezvous_message::Union::RegisterPk(RegisterPk {
            id: local_id.to_owned(),
            uuid: uuid_bytes,
            pk,
            old_pk: Vec::new(),
        })),
    };
    socket
        .send_to(&encode_message(&msg), server)
        .map(|_| ())
        .map_err(|e| format!("UDP send_to RegisterPk: {e}"))
}

#[allow(dead_code)]
fn read_peer_msg_timeout(stream: &mut TcpStream, timeout: Duration) -> Result<PeerMessage, String> {
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| format!("set_read_timeout: {e}"))?;
    let payload = read_framed(stream)?;
    decode_peer_message(&payload).map_err(|e| format!("Peer decode: {e}"))
}

/// UDP loopback test — sends a packet from one local socket to another and
/// reads it back.  If this fails, `recv_from` is broken on this machine
/// (driver / Winsock issue).
fn udp_loopback_test(events: &Sender<HostEvent>) {
    host_log(events, "UDP loopback test…".to_owned());

    let sock_tx = match UdpSocket::bind("127.0.0.1:0") {
        Ok(s) => s,
        Err(e) => {
            host_log(events, format!("UDP loopback: bind TX: {e}"));
            return;
        }
    };
    let sock_rx = match UdpSocket::bind("127.0.0.1:0") {
        Ok(s) => s,
        Err(e) => {
            host_log(events, format!("UDP loopback: bind RX: {e}"));
            return;
        }
    };
    let addr_rx = match sock_rx.local_addr() {
        Ok(a) => a,
        Err(e) => {
            host_log(events, format!("UDP loopback: addr: {e}"));
            return;
        }
    };

    if let Err(e) = sock_tx.send_to(b"EvertyDesk-ping", addr_rx) {
        host_log(events, format!("UDP loopback: send: {e}"));
        return;
    }
    sock_rx
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok();
    let mut buf = [0u8; 32];
    match sock_rx.recv_from(&mut buf) {
        Ok((n, _)) if &buf[..n] == b"EvertyDesk-ping" => {
            host_log(events, "UDP loopback: PASS ✓ — recv_from works".to_owned())
        }
        Ok((n, src)) => host_log(
            events,
            format!("UDP loopback: unexpected data {n}B from {src}"),
        ),
        Err(e) => host_log(events, format!("UDP loopback: FAIL — recv_from: {e}")),
    }
}

/// Internet UDP test — sends a minimal DNS query to 1.1.1.1:53 and waits for
/// a response.  If the response arrives, inbound UDP from the internet works
/// (NAT + firewall OK).  If it times out, something is blocking inbound UDP.
fn udp_internet_test(events: &Sender<HostEvent>) {
    host_log(events, "UDP internet test (DNS → 1.1.1.1:53)…".to_owned());

    let sock = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            host_log(events, format!("UDP internet: bind: {e}"));
            return;
        }
    };

    // Minimal DNS query: A record for "a.com"
    //   ID=0x1234, flags=0x0100 (standard query RD), QDCOUNT=1
    //   QNAME=\x01a\x03com\x00  QTYPE=A(1)  QCLASS=IN(1)
    let dns_query: &[u8] = &[
        0x12, 0x34, // ID
        0x01, 0x00, // Flags: standard query, recursion desired
        0x00, 0x01, // QDCOUNT = 1
        0x00, 0x00, // ANCOUNT = 0
        0x00, 0x00, // NSCOUNT = 0
        0x00, 0x00, // ARCOUNT = 0
        0x01, b'a', // \x01 + "a"
        0x03, b'c', b'o', b'm', // \x03 + "com"
        0x00, // root label
        0x00, 0x01, // QTYPE = A
        0x00, 0x01, // QCLASS = IN
    ];

    if let Err(e) = sock.send_to(dns_query, "1.1.1.1:53") {
        host_log(events, format!("UDP internet: send: {e}"));
        return;
    }
    host_log(
        events,
        "UDP internet: DNS query sent → 1.1.1.1:53".to_owned(),
    );

    sock.set_read_timeout(Some(Duration::from_secs(3))).ok();
    let mut buf = [0u8; 512];
    match sock.recv_from(&mut buf) {
        Ok((n, src)) => host_log(
            events,
            format!(
                "UDP internet: PASS ✓ — got {n}B from {src} (inbound UDP from internet works!)"
            ),
        ),
        Err(ref e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
        {
            host_log(
                events,
                "UDP internet: FAIL — timeout (3s, no response from 1.1.1.1:53) \
                → inbound UDP from internet is BLOCKED"
                    .to_owned(),
            )
        }
        Err(e) => host_log(events, format!("UDP internet: FAIL — recv_from: {e}")),
    }
}

/// Returns the first ≤ 20 bytes of `bytes` as space-separated uppercase hex.
/// Appends " …(N total)" if the slice is longer than 20 bytes.
fn hex_short(bytes: &[u8]) -> String {
    let preview = bytes.len().min(20);
    let hex = bytes[..preview]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    if bytes.len() > preview {
        format!("{hex} …({} total)", bytes.len())
    } else {
        hex
    }
}

/// Diagnostic TCP probe on the ID-server port.
///
/// Connects, logs any server greeting, sends a framed `RegisterPeer`, logs the
/// raw response (before and after decode attempts).  Pure side-effect
/// (logging); does not affect the UDP registration flow.
fn tcp_probe(host: &str, port: u16, local_id: &str, events: &Sender<HostEvent>) {
    use std::io::Read as _;
    use std::net::ToSocketAddrs;

    host_log(events, format!("=== TCP probe {host}:{port} ==="));

    // ── DNS ──────────────────────────────────────────────────────────────────
    let addr_str = format!("{host}:{port}");
    let sock_addr = match addr_str.to_socket_addrs().ok().and_then(|mut i| i.next()) {
        Some(a) => {
            host_log(events, format!("TCP probe: DNS → {}", a.ip()));
            a
        }
        None => {
            host_log(events, format!("TCP probe: DNS resolve failed for {host}"));
            return;
        }
    };

    // ── Connect ──────────────────────────────────────────────────────────────
    let mut stream = match TcpStream::connect_timeout(&sock_addr, Duration::from_secs(4)) {
        Ok(s) => {
            host_log(events, "TCP probe: connected ✓".to_owned());
            s
        }
        Err(e) => {
            host_log(events, format!("TCP probe: connect failed: {e}"));
            return;
        }
    };

    // ── 1. Read any server greeting ──────────────────────────────────────────
    stream
        .set_read_timeout(Some(Duration::from_millis(700)))
        .ok();
    let mut gbuf = [0u8; 256];
    match stream.read(&mut gbuf) {
        Ok(0) => host_log(
            events,
            "TCP probe: server closed immediately (EOF before send)".to_owned(),
        ),
        Ok(n) => host_log(
            events,
            format!("TCP probe: server greeting {n}B: {}", hex_short(&gbuf[..n])),
        ),
        Err(ref e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
        {
            host_log(
                events,
                "TCP probe: no greeting (server silent after connect)".to_owned(),
            )
        }
        Err(e) => host_log(events, format!("TCP probe: greeting read err: {e}")),
    }

    // ── 2. Send framed RegisterPeer ──────────────────────────────────────────
    let payload = encode_message(&RendezvousMessage {
        union: Some(rendezvous_message::Union::RegisterPeer(RegisterPeer {
            id: local_id.to_owned(),
            serial: 0,
        })),
    });
    host_log(
        events,
        format!(
            "TCP probe: sending framed RegisterPeer {}B: {}",
            payload.len(),
            hex_short(&payload)
        ),
    );
    if let Err(e) = send_framed(&mut stream, &payload) {
        host_log(events, format!("TCP probe: send_framed err: {e}"));
        return;
    }

    // ── 3. Read server response ───────────────────────────────────────────────
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    let mut rbuf = [0u8; 1024];
    match stream.read(&mut rbuf) {
        Ok(0) => host_log(
            events,
            "TCP probe: server closed after our message (EOF) — normal for TCP 21116".to_owned(),
        ),
        Ok(n) => {
            host_log(
                events,
                format!("TCP probe: server replied {n}B: {}", hex_short(&rbuf[..n])),
            );
            // Try raw proto decode
            if let Ok(m) = decode_message(&rbuf[..n]) {
                host_log(
                    events,
                    format!(
                        "TCP probe: raw-decode → {}",
                        match &m.union {
                            Some(rendezvous_message::Union::RegisterPeerResponse(r)) =>
                                format!("RegisterPeerResponse request_pk={}", r.request_pk),
                            Some(_) => "other variant".to_owned(),
                            None => "empty union".to_owned(),
                        }
                    ),
                );
            }
            // Try framed decode (4-byte big-endian length prefix)
            if n > 4 {
                let declared = u32::from_be_bytes([rbuf[0], rbuf[1], rbuf[2], rbuf[3]]) as usize;
                host_log(events, format!("TCP probe: 4-byte prefix = {declared}"));
                if declared > 0 && 4 + declared <= n {
                    if let Ok(m) = decode_message(&rbuf[4..4 + declared]) {
                        host_log(
                            events,
                            format!(
                                "TCP probe: framed-decode → {}",
                                match &m.union {
                                    Some(rendezvous_message::Union::RegisterPeerResponse(r)) =>
                                        format!("RegisterPeerResponse request_pk={}", r.request_pk),
                                    Some(_) => "other variant".to_owned(),
                                    None => "empty union".to_owned(),
                                }
                            ),
                        );
                    }
                }
            }
        }
        Err(ref e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
        {
            host_log(
                events,
                "TCP probe: no response (timeout after 2s)".to_owned(),
            )
        }
        Err(e) => host_log(events, format!("TCP probe: response read err: {e}")),
    }

    host_log(events, "=== TCP probe done ===".to_owned());
}

/// Send a PeerMessage over the relay, encrypting it if the secure channel is up.
fn new_evrt_session_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn append_evrt_session_token(endpoints: &str, token: &str) -> String {
    if endpoints.trim().is_empty() {
        format!("token={token}")
    } else {
        format!("{endpoints},token={token}")
    }
}

/// Обработать VmPowerOp JSON и вернуть JSON-результат.
fn handle_vm_power_op(json: &str) -> String {
    #[cfg(windows)]
    {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(json) else {
            return r#"{"ok":false,"error":"invalid json"}"#.to_owned();
        };
        let vm_id = val.get("vm_id").and_then(|v| v.as_str()).unwrap_or("");
        let vm_path = val.get("vm_path").and_then(|v| v.as_str()).unwrap_or("");
        let action_str = val.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let provider = if vm_id.starts_with("hyperv:") {
            "hyperv"
        } else if vm_id.starts_with("vbox:") {
            "vbox"
        } else {
            "hyperv"
        };
        let real_id = vm_id.splitn(2, ':').nth(1).unwrap_or(vm_id);
        match provider {
            "hyperv" => {
                if let Some(action) = crate::hyperv::VmPowerAction::from_str(action_str) {
                    let path = if !vm_path.is_empty() {
                        vm_path.to_owned()
                    } else {
                        format!("Msvm_ComputerSystem.CreationClassName=\"Msvm_ComputerSystem\",Name=\"{real_id}\"")
                    };
                    crate::hyperv::request_power_action(&path, action);
                    format!(
                        r#"{{"vm_id":{vm_id_json},"action":{action_json},"ok":true,"error":""}}"#,
                        vm_id_json = serde_json::Value::String(vm_id.to_owned()),
                        action_json = serde_json::Value::String(action_str.to_owned())
                    )
                } else {
                    format!(r#"{{"ok":false,"error":"unknown action: {action_str}"}}"#)
                }
            }
            "vbox" => {
                // VBoxManage controlvm <uuid> <acpipowerbutton|poweroff|pause|resume|savestate>
                let vbox_action = match action_str {
                    "start" => "startvm",
                    "stop" => "poweroff",
                    "shutdown" => "acpipowerbutton",
                    "pause" => "pause",
                    "resume" => "resume",
                    "save" => "savestate",
                    other => {
                        return format!(r#"{{"ok":false,"error":"unknown vbox action: {other}"}}"#)
                    }
                };
                let ok = if action_str == "start" {
                    std::process::Command::new("VBoxManage")
                        .args(["startvm", real_id])
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false)
                } else {
                    std::process::Command::new("VBoxManage")
                        .args(["controlvm", real_id, vbox_action])
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false)
                };
                format!(
                    r#"{{"vm_id":{vm_id_json},"action":{action_json},"ok":{ok},"error":""}}"#,
                    vm_id_json = serde_json::Value::String(vm_id.to_owned()),
                    action_json = serde_json::Value::String(action_str.to_owned()),
                    ok = ok
                )
            }
            _ => r#"{"ok":false,"error":"unknown provider"}"#.to_owned(),
        }
    }
    #[cfg(not(windows))]
    r#"{"ok":false,"error":"power ops only available on Windows host"}"#.to_owned()
}

fn vm_misc_message(union: misc::Union) -> PeerMessage {
    PeerMessage {
        union: Some(peer_message::Union::Misc(Misc { union: Some(union) })),
    }
}

fn evrt_endpoints_message(endpoints: &str) -> PeerMessage {
    PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::EvrtEndpoints(endpoints.to_owned())),
        })),
    }
}

fn evrt_port_message(port: u16) -> PeerMessage {
    PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::EvrtUdpPort(port as u32)),
        })),
    }
}

fn repeat_evrt_announcement(
    peer_msg_tx: mpsc::Sender<PeerMessage>,
    events: Sender<HostEvent>,
    endpoints: String,
    evrt_port: u16,
) {
    thread::Builder::new()
        .name("evrt-announce".into())
        .spawn(move || {
            for attempt in 1..=6 {
                thread::sleep(Duration::from_millis(350));
                if !endpoints.is_empty() {
                    let _ = peer_msg_tx.send(evrt_endpoints_message(&endpoints));
                }
                let _ = peer_msg_tx.send(evrt_port_message(evrt_port));
                host_log(
                    &events,
                    format!("EVRT: announcement repeat #{attempt} port={evrt_port}"),
                );
            }
        })
        .ok();
}

/// Send a PeerMessage over the relay, encrypting it if the secure channel is up.
fn send_peer_enc(
    stream: &mut TcpStream,
    cipher: &mut Option<StreamCipher>,
    msg: &PeerMessage,
) -> Result<(), String> {
    let mut bytes = encode_peer_message(msg);
    if let Some(c) = cipher.as_mut() {
        bytes = c.encrypt(&bytes);
    }
    send_framed(stream, &bytes)
}

/// Send-only-cipher-half counterpart of `send_peer_enc` — for a dedicated
/// writer thread that owns a `TcpStream::try_clone()` of the relay stream
/// (`recv_peer_rc` above is the matching receive-only-half reader, already
/// used by the live pipeline's own input-loop/TCP-sender split). ROADMAP.md
/// Phase 5.3 uses this so `run_evrt2_only_session`'s relay-wrap writer
/// thread never has to touch the read-side cipher state or the read-side
/// socket clone — the two threads share nothing, so there's no lock and no
/// risk of interleaving two `write_all` calls on the same underlying fd.
fn send_peer_sc(
    stream: &mut TcpStream,
    cipher: &mut Option<crate::crypto::SendCipher>,
    msg: &PeerMessage,
) -> Result<(), String> {
    let mut bytes = encode_peer_message(msg);
    if let Some(c) = cipher.as_mut() {
        bytes = c.encrypt(&bytes);
    }
    send_framed(stream, &bytes)
}

/// Read a PeerMessage from the relay, decrypting if the secure channel is up.
/// Returns `Ok(None)` for empty keepalive frames.
fn recv_peer_enc(
    stream: &mut TcpStream,
    cipher: &mut Option<StreamCipher>,
) -> Result<Option<PeerMessage>, String> {
    let payload = read_framed(stream)?;
    if payload.is_empty() {
        return Ok(None);
    }
    let plain = if let Some(c) = cipher.as_mut() {
        c.decrypt(&payload)?
    } else {
        payload
    };
    let msg = decode_peer_message(&plain).map_err(|e| format!("Peer decode: {e}"))?;
    Ok(Some(msg))
}

/// Send a `LoginResponse{ Error }` (encrypted if the secure channel is up).
/// Use RustDesk's exact strings ("Empty Password" / "Wrong Password") so the
/// peer shows its password dialog and retries on the same connection.
fn send_login_error(
    stream: &mut TcpStream,
    cipher: &mut Option<StreamCipher>,
    message: &str,
) -> Result<(), String> {
    let msg = PeerMessage {
        union: Some(peer_message::Union::LoginResponse(LoginResponse {
            union: Some(login_response::Union::Error(message.to_owned())),
        })),
    };
    send_peer_enc(stream, cipher, &msg)
}

/// Human-readable name of a peer_message variant (for diagnostics).
fn peer_msg_kind(union: &Option<peer_message::Union>) -> &'static str {
    use peer_message::Union as U;
    match union {
        Some(U::LoginRequest(_)) => "LoginRequest",
        Some(U::LoginResponse(_)) => "LoginResponse",
        Some(U::Hash(_)) => "Hash",
        Some(U::PublicKey(_)) => "PublicKey",
        Some(U::SignedId(_)) => "SignedId",
        Some(U::TestDelay(_)) => "TestDelay",
        Some(U::VideoFrame(_)) => "VideoFrame",
        Some(U::MouseEvent(_)) => "MouseEvent",
        Some(U::KeyEvent(_)) => "KeyEvent",
        Some(U::Misc(_)) => "Misc",
        Some(_) => "other",
        None => "empty",
    }
}

fn host_log(events: &Sender<HostEvent>, msg: String) {
    // Don't eprintln! here — the GUI shows logs in the log panel, and the
    // headless --host loop already prints everything it receives from the
    // channel.  Printing here would cause every line to appear twice in
    // headless mode.
    let _ = events.send(HostEvent::Log(msg));
}

fn is_timeout(e: &str) -> bool {
    // "os error 997" is ERROR_IO_PENDING — a Windows quirk where recv() on a
    // socket using SO_RCVTIMEO occasionally fails with 997 instead of the
    // normal WSAETIMEDOUT (10060), especially under CPU load. It means "no
    // data yet", not a broken connection. Live-found: it was killing whole
    // EVRT2 sessions ~seconds after the screen went static (host busy
    // encoding at full tilt → relay poll hit 997 → treated as fatal).
    e.contains("timed out")
        || e.contains("WouldBlock")
        || e.contains("os error 10060")
        || e.contains("os error 997")
}

fn random_u64() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    // Simple deterministic mix — not crypto-quality but good enough for salt/challenge.
    let v = ns ^ (pid << 32) ^ (ns >> 17) ^ 0xDEAD_BEEF_CAFE_1234;
    v as u64
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "EvertyDesk".to_owned())
}

/// macOS host permission status for the GUI: `(screen_recording, accessibility)`.
/// Cheap, non-prompting — safe to poll each frame.
#[cfg(target_os = "macos")]
pub fn macos_permission_status() -> (bool, bool) {
    macos_cgevent::permission_status()
}

// ─── Pub-обёртки для evrt_session ─────────────────────────────────────────────

/// Публичная обёртка — нужна для `evrt_session.rs`.
pub fn h264_target_bitrate_bps_pub(w: u32, h: u32, fps: u32, quality_milli: u32) -> u32 {
    h264_target_bitrate_bps(w, h, fps, quality_milli)
}

/// Публичная обёртка release_stuck_input — нужна для `evrt_session.rs`.
pub fn release_stuck_input_pub() {
    release_stuck_input();
}

/// Возраст запущенного бинаря в секундах (по mtime exe-файла).
/// Позволяет понять, свежий ли хост запущен, без штампов сборки.
/// Возвращает "HH:MM" локального времени сборки или "?" если недоступно.
pub fn binary_build_stamp() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return "?".into();
    };
    let Ok(meta) = std::fs::metadata(&exe) else {
        return "?".into();
    };
    let Ok(mtime) = meta.modified() else {
        return "?".into();
    };
    let unix = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Локальное HH:MM:SS из unix (грубо, без таймзоны — для сравнения свежести хватает)
    let secs_of_day = unix % 86400;
    let h = (secs_of_day / 3600) % 24;
    let m = (secs_of_day % 3600) / 60;
    let s = secs_of_day % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

// ─── MultiEncoder — единый каскад энкодеров для video_pipeline ─────────────────
//
// Инкапсулирует полную цепочку fallback'ов:
//   Media Foundation (Windows) → VideoToolbox (macOS) → NVENC → OpenH264 → PNG
//
// Это восстанавливает мультибэкендность, которая была в старом video_loop.
// pipeline создаёт один MultiEncoder и вызывает encode() на каждый кадр.

/// Результат кодирования: байты + флаг IDR + опциональные SPS/PPS + кодек.
pub struct EncodedOutput {
    pub bytes: Vec<u8>,
    pub key: bool,
    pub sps_pps: Option<Vec<u8>>,
    pub codec: &'static str,
}

/// Единый энкодер с каскадом аппаратных/программных бэкендов.
pub struct MultiEncoder {
    // Выбранные кодеки для каждого бэкенда (None = бэкенд недоступен)
    desired_mf: Option<crate::nvenc::NvencCodec>,
    desired_vt: Option<crate::nvenc::NvencCodec>,
    desired_nv: Option<crate::nvenc::NvencCodec>,
    /// Intel oneVPL/Media SDK — путь напрямую в драйвер, минуя Media
    /// Foundation. Единственный способ получить HEVC на машинах без пакета
    /// «HEVC Video Extensions» из Store.
    desired_vpl: Option<crate::nvenc::NvencCodec>,

    // Состояния энкодеров (lazy-init)
    mf: Option<crate::mf_encode::MfVideoEncoder>,
    vt: Option<crate::videotoolbox::VideoToolboxEncoder>,
    nv: Option<crate::nvenc::NvencEncoder>,
    vpl: Option<crate::onevpl::OneVplEncoder>,

    // Disabled-флаги: после ошибки бэкенд выключается
    mf_disabled: bool,
    vt_disabled: bool,
    nv_disabled: bool,
    vpl_disabled: bool,

    // OpenH264 software fallback
    #[cfg(feature = "live-h264")]
    sw: Option<openh264::encoder::Encoder>,
    #[cfg(feature = "live-h264")]
    yuv: YuvFrame,

    /// Какой бэкенд реально выдал последний кадр (для диагностики).
    /// Меняется на первом успехе — видно MF это, OpenH264 или PNG.
    active_backend: &'static str,
    /// Причина почему MF отключился (для диагностики «почему софт вместо железа»).
    mf_error: Option<String>,
}

impl MultiEncoder {
    /// Создать энкодер, выбрав доступные бэкенды по preference и возможностям клиента.
    pub fn new(
        encoder_pref: EncoderPreference,
        codec_pref: CodecPreference,
        client: ClientVideoSupport,
    ) -> Self {
        let desired_nv = choose_nvenc_codec(encoder_pref, codec_pref, client);
        let desired_vpl = choose_onevpl_codec(encoder_pref, codec_pref, client);
        let desired_mf = choose_mf_encoder_codec(encoder_pref, codec_pref, client);
        let desired_vt = choose_videotoolbox_codec(encoder_pref, codec_pref, client);

        #[cfg(feature = "live-h264")]
        let sw = build_openh264_encoder();

        Self {
            desired_mf,
            desired_vt,
            desired_nv,
            desired_vpl,
            mf: None,
            vt: None,
            nv: None,
            vpl: None,
            mf_disabled: false,
            vt_disabled: false,
            nv_disabled: false,
            vpl_disabled: false,
            #[cfg(feature = "live-h264")]
            sw,
            #[cfg(feature = "live-h264")]
            yuv: YuvFrame::default(),
            active_backend: "none",
            mf_error: None,
        }
    }

    /// Реальный бэкенд который выдал последний кадр (MF/VideoToolbox/NVENC/OpenH264/PNG).
    pub fn active_backend(&self) -> &'static str {
        self.active_backend
    }

    /// Есть ли вообще аппаратный бэкенд, пригодный для этого клиента.
    ///
    /// Игровой режим осмыслен только когда он true: иначе каскад свалится в
    /// OpenH264/PNG, и «игровой режим» окажется медленнее обычного EVRTCK —
    /// про такое лучше честно сказать, чем тихо отдать софтверную картинку.
    pub fn has_hardware_backend(&self) -> bool {
        self.desired_nv.is_some()
            || self.desired_vpl.is_some()
            || self.desired_mf.is_some()
            || self.desired_vt.is_some()
    }

    /// Есть ли аппаратный бэкенд, который ЕЩЁ НЕ отключён ошибкой.
    ///
    /// В отличие от `active_backend()`, который говорит лишь про последний
    /// конкретный кадр и может прыгать туда-сюда (аппаратный MFT асинхронный:
    /// вернул `Ok(None)` — и каскад провалился в OpenH264 на этом кадре), это
    /// стабильное свойство сессии. Меняется только когда бэкенд по-настоящему
    /// выключился после ошибки. Именно на такой признак должны опираться
    /// решения вроде «включить софтверный профиль», иначе они дребезжат на
    /// каждом кадре.
    pub fn has_active_hardware_backend(&self) -> bool {
        (self.desired_nv.is_some() && !self.nv_disabled)
            || (self.desired_vpl.is_some() && !self.vpl_disabled)
            || (self.desired_mf.is_some() && !self.mf_disabled)
            || (self.desired_vt.is_some() && !self.vt_disabled)
    }

    /// Причина отключения MF (если был выбран, но упал). Для диагностики.
    pub fn take_mf_error(&mut self) -> Option<String> {
        self.mf_error.take()
    }

    /// Intentionally leak GPU resources (NVENC D3D11 device) to avoid a
    /// deadlock in nvwgf2umx.dll when WGPU's D3D12 renderer is still active.
    /// The OS reclaims VRAM on process exit.  All CPU-side state is dropped
    /// normally; only the GPU handle is forgotten.
    pub fn leak_gpu_resources(&mut self) {
        if let Some(nv) = self.nv.take() {
            std::mem::forget(nv);
        }
        if let Some(vpl) = self.vpl.take() {
            std::mem::forget(vpl);
        }
        if let Some(mf) = self.mf.take() {
            std::mem::forget(mf);
        }
        #[cfg(feature = "live-h264")]
        if let Some(sw) = self.sw.take() {
            std::mem::forget(sw);
        }
    }

    /// Краткое описание активной цепочки бэкендов (для логов).
    pub fn backend_label(&self) -> String {
        let mut parts = Vec::new();
        if let Some(c) = self.desired_nv {
            parts.push(format!("NVENC/{}", c.label()));
        }
        if let Some(c) = self.desired_vpl {
            parts.push(format!("oneVPL/{}", c.label()));
        }
        if let Some(c) = self.desired_mf {
            parts.push(format!("MF/{}", c.label()));
        }
        if let Some(c) = self.desired_vt {
            parts.push(format!("VT/{}", c.label()));
        }
        #[cfg(feature = "live-h264")]
        if self.sw.is_some() {
            parts.push("OpenH264".to_owned());
        }
        parts.push("PNG".to_owned());
        parts.join(" → ")
    }

    /// Закодировать один кадр BGRA. Проходит каскад до первого успеха.
    /// `force_key` запрашивает IDR.
    pub fn encode(
        &mut self,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
        bgra: &[u8],
        force_key: bool,
    ) -> Option<EncodedOutput> {
        // 1. Direct NVENC (Windows, NVIDIA). This is the EvertyGame-grade path:
        // D3D11 + NVENC ultra-low-latency settings, with MF only as fallback.
        if let Some(codec) = self.desired_nv.filter(|_| !self.nv_disabled) {
            match encode_nvenc_frame(
                &mut self.nv,
                codec,
                width,
                height,
                fps,
                bitrate,
                bgra,
                force_key,
            ) {
                Ok(Some(pkt)) => {
                    self.active_backend = "NVENC";
                    return Some(EncodedOutput {
                        bytes: pkt.bytes,
                        key: pkt.key,
                        sps_pps: None,
                        codec: codec_label(codec),
                    });
                }
                // Как и у oneVPL ниже: «пакета пока нет» не даёт права спускаться
                // к бэкенду с другим кодеком — это отправило бы клиенту чужой
                // битстрим под видом текущего кодека сессии.
                Ok(None) => return None,
                Err(err) => {
                    eprintln!("[host-video] NVENC disabled after error: {err}");
                    self.nv_disabled = true;
                }
            }
        }

        // 2. Intel oneVPL / Media SDK — ДО Media Foundation.
        //
        // Порядок не косметический: на Intel без пакета «HEVC Video
        // Extensions» из Store у Media Foundation вообще нет HEVC (живо
        // подтверждено на UHD 610: MFTEnumEx не находит ни одного HEVC-MFT,
        // хотя H264(hw) находит). oneVPL идёт напрямую в драйвер и HEVC там
        // есть. Пропусти мы его вперёд MF — получили бы H264 на железе,
        // которое умеет H265.
        if let Some(codec) = self.desired_vpl.filter(|_| !self.vpl_disabled) {
            let needs_new = self
                .vpl
                .as_ref()
                .map(|enc| {
                    !enc.matches(codec, width, height, fps) || enc.current_bitrate() != bitrate
                })
                .unwrap_or(true);
            if needs_new {
                match crate::onevpl::OneVplEncoder::new(codec, width, height, fps, bitrate) {
                    Ok(enc) => self.vpl = Some(enc),
                    Err(err) => {
                        eprintln!("[host-video] oneVPL disabled after init error: {err}");
                        self.vpl_disabled = true;
                        self.vpl = None;
                    }
                }
            }
            if let Some(enc) = self.vpl.as_mut() {
                match enc.encode_bgra(bgra, force_key) {
                    Ok(Some(pkt)) => {
                        self.active_backend = "oneVPL";
                        return Some(EncodedOutput {
                            bytes: pkt.bytes,
                            key: pkt.key,
                            sps_pps: None,
                            codec: codec_label(codec),
                        });
                    }
                    Ok(None) => {
                        // «Пакета пока нет» — НЕ повод спускаться ниже по
                        // каскаду. Следующий бэкенд (Media Foundation) кодирует
                        // в ДРУГОЙ кодек, и подмешать его кадр в сессию,
                        // объявленную как H265, значит отправить клиенту H264
                        // под видом HEVC. Системный декодер Windows на таком
                        // не возвращает ошибку — он роняет процесс клиента
                        // (0xC0000005 в HEVCDECODER_STORE.dll, живо поймано
                        // отладчиком). Пропускаем кадр целиком: следующий
                        // придёт через ~16мс, это несопоставимо дешевле.
                        return None;
                    }
                    Err(err) => {
                        eprintln!("[host-video] oneVPL disabled after error: {err}");
                        self.vpl_disabled = true;
                        self.vpl = None;
                    }
                }
            }
        }

        // 3. Media Foundation (Windows)
        if let Some(codec) = self.desired_mf.filter(|_| !self.mf_disabled) {
            match encode_mf_frame(
                &mut self.mf,
                codec,
                width,
                height,
                fps,
                bitrate,
                bgra,
                force_key,
            ) {
                Ok(Some(pkt)) => {
                    let sps = if pkt.key {
                        self.mf.as_ref().and_then(|e| e.codec_config())
                    } else {
                        None
                    };
                    self.active_backend = "MediaFoundation";
                    return Some(EncodedOutput {
                        bytes: pkt.bytes,
                        key: pkt.key,
                        sps_pps: sps,
                        codec: codec_label(codec),
                    });
                }
                // Тот же принцип, что у NVENC/oneVPL выше: ниже по каскаду
                // стоит OpenH264, и он всегда H264. Для сессии на H265 это
                // чужой кодек.
                Ok(None) => return None,
                Err(e) => {
                    eprintln!("[host-video] MediaFoundation disabled after error: {e}");
                    self.mf_disabled = true;
                    self.mf_error = Some(e);
                }
            }
        }

        // 3. VideoToolbox (macOS)
        if let Some(codec) = self.desired_vt.filter(|_| !self.vt_disabled) {
            match encode_videotoolbox_frame(
                &mut self.vt,
                codec,
                width,
                height,
                fps,
                bitrate,
                bgra,
                force_key,
            ) {
                Ok(Some(pkt)) => {
                    self.active_backend = "VideoToolbox";
                    return Some(EncodedOutput {
                        bytes: pkt.bytes,
                        key: pkt.key,
                        sps_pps: None,
                        codec: codec_label(codec),
                    });
                }
                Ok(None) => return None,
                Err(err) => {
                    eprintln!("[host-video] VideoToolbox disabled after error: {err}");
                    self.vt_disabled = true;
                }
            }
        }

        // 4. OpenH264 software
        #[cfg(feature = "live-h264")]
        {
            if let Some(pkt) = encode_h264_frame(
                self.sw.as_mut(),
                &mut self.yuv,
                width,
                height,
                bgra,
                force_key,
            ) {
                self.active_backend = "OpenH264-SW";
                return Some(EncodedOutput {
                    bytes: pkt.bytes,
                    key: pkt.key,
                    sps_pps: None,
                    codec: "H264",
                });
            }
        }

        // 5. PNG fallback (только на keyframe чтобы не спамить большими кадрами)
        if force_key {
            self.active_backend = "PNG";
            return Some(EncodedOutput {
                bytes: encode_png_fallback(bgra, width, height),
                key: true,
                sps_pps: None,
                codec: "PNG",
            });
        }

        None
    }
}

fn codec_label(codec: crate::nvenc::NvencCodec) -> &'static str {
    match codec {
        crate::nvenc::NvencCodec::H265 => "H265",
        crate::nvenc::NvencCodec::Av1 => "AV1",
        crate::nvenc::NvencCodec::H264 => "H264",
    }
}

#[cfg(feature = "live-h264")]
fn build_openh264_encoder() -> Option<openh264::encoder::Encoder> {
    use openh264::encoder::{
        BitRate, Complexity, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, RateControlMode,
        SpsPpsStrategy, UsageType,
    };
    // ★ Многопоточность — главный рычаг скорости софтверного H264.
    //   На слабом/VM железе без аппаратного MF это разница между 3 и 25 fps.
    let cores = std::thread::available_parallelism()
        .map(|n| n.get() as u16)
        .unwrap_or(4)
        .clamp(1, 16);

    let cfg = EncoderConfig::new()
        .usage_type(UsageType::ScreenContentRealTime)
        .rate_control_mode(RateControlMode::Bitrate)
        .bitrate(BitRate::from_bps(8_000_000))
        .max_frame_rate(FrameRate::from_hz(MAX_TARGET_FPS as f32))
        .sps_pps_strategy(SpsPpsStrategy::IncreasingId)
        .intra_frame_period(IntraFramePeriod::from_num_frames(MAX_TARGET_FPS * 2))
        // Complexity::Low — приоритет скорости над качеством (для realtime).
        .complexity(Complexity::Low)
        .num_threads(cores);
    let api = openh264::OpenH264API::from_source();
    Encoder::with_api_config(api, cfg).ok()
}

/// PNG fallback кодирование (используется MultiEncoder и pipeline).
pub fn encode_png_fallback(bgra: &[u8], w: u32, h: u32) -> Vec<u8> {
    use image::{ImageBuffer, Rgba};
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_raw(w, h, bgra.to_vec()).unwrap_or_else(|| ImageBuffer::new(w, h));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok();
    out
}

// ─── Управление системной громкостью хоста ─────────────────────────────────────

/// Установить громкость мастер-устройства воспроизведения хоста (0..100 %).
/// Windows Core Audio (IAudioEndpointVolume). На других платформах — no-op с ошибкой.
fn set_system_volume(percent: u32) -> Result<(), String> {
    #[cfg(all(target_os = "windows", feature = "live-vp9-mf"))]
    {
        use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
        use windows::Win32::Media::Audio::{
            eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
        };
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
        };

        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| format!("MMDeviceEnumerator: {e}"))?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| format!("GetDefaultAudioEndpoint: {e}"))?;
            let endpoint: IAudioEndpointVolume = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| format!("Activate IAudioEndpointVolume: {e}"))?;
            let scalar = (percent.min(100) as f32) / 100.0;
            endpoint
                .SetMasterVolumeLevelScalar(scalar, std::ptr::null())
                .map_err(|e| format!("SetMasterVolumeLevelScalar: {e}"))?;
        }
        Ok(())
    }
    #[cfg(not(all(target_os = "windows", feature = "live-vp9-mf")))]
    {
        let _ = percent;
        Err("volume control is only available on the Windows host build".to_owned())
    }
}

// ─── EVRT socket helper ───────────────────────────────────────────────────────

/// Открыть выделенный UDP сокет для EVRT-сессии.
///
/// Порт берётся из конфига (`evrt_udp_port`) или выбирается случайный.
/// Возвращает `None` если bind не удался.
pub fn try_open_evrt_socket(
    config: &AppConfig,
    events: &Sender<HostEvent>,
) -> Option<(Arc<UdpSocket>, u16)> {
    // Порт из конфига или случайный
    let bind_addr = if config.evrt_udp_port > 0 {
        format!("0.0.0.0:{}", config.evrt_udp_port)
    } else {
        "0.0.0.0:0".to_owned()
    };

    match UdpSocket::bind(&bind_addr) {
        Ok(sock) => {
            let port = sock.local_addr().map(|a| a.port()).unwrap_or(0);
            host_log(events, format!("EVRT: UDP сокет открыт на порту {port}"));
            Some((Arc::new(sock), port))
        }
        Err(e) => {
            host_log(events, format!("EVRT: не удалось открыть UDP сокет: {e}"));
            None
        }
    }
}
