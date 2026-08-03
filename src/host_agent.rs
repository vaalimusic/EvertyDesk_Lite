//! Phase 1 of `TZ_HOST_SERVICE.md`: run the host in its own OS process
//! instead of a thread inside the GUI process.
//!
//! Today `HostService::start` spawns `run_host_loop` as a thread in the same
//! process as the egui window. That thread competes with the GUI for CPU and
//! dies the instant the GUI process exits — closing or even just minimizing
//! the window can starve or kill the stream (see `TZ_HOST_SERVICE.md` §1-2).
//!
//! This module lets the host run as a *separate* process (`--host-agent`)
//! that the GUI talks to over a loopback TCP socket instead of a Rust mpsc
//! channel. A loopback socket was chosen over a platform IPC primitive
//! (named pipes / unix sockets) because it needs zero new dependencies, no
//! `unsafe` FFI, and is identical code on Windows/Linux/macOS — exactly what
//! Phase 4 (Linux/macOS) will also want. `HostService`'s public API is
//! unchanged either way (see the `Backend` enum in `host.rs`), so call sites
//! elsewhere in the app don't need to know which mode is active.
//!
//! Wire format: one JSON object per line (`serde_json`), `HostEvent` flowing
//! agent → GUI and `HostCommand` flowing GUI → agent, each direction using
//! its own half of the socket so a slow reader on one side can't stall
//! writes on the other. A random token (written to `host_agent.json` next to
//! `config.json`) gates the first line of every new connection — anyone who
//! can already read that per-user config directory could read the token
//! anyway, so this matches the existing trust boundary rather than adding a
//! new one.
//!
//! Lifecycle: the agent process outlives any single GUI connection. Closing
//! the GUI just drops the socket (`HostService::detach`); the agent keeps
//! hosting and the next GUI launch reattaches to it instead of spawning a
//! duplicate. Only an explicit "stop hosting" (`HostService::stop`, which
//! sends `HostCommand::Stop` down the wire) ends the agent process itself.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::host::{HostCommand, HostEvent, HostService};
use crate::settings::AppConfig;

/// Master switch for Phase 1. Off by default: this changes the host's
/// process model, and that needs to be validated on real hardware (the
/// dev-PC ↔ Intel UHD 610 stand from `TZ_HOST_SERVICE.md` §7) before it
/// becomes the default. `EVERTYDESK_HOST_AGENT=1` opts in.
pub fn enabled() -> bool {
    std::env::var("EVERTYDESK_HOST_AGENT")
        .map(|v| v == "1")
        .unwrap_or(false)
}

#[derive(Serialize, Deserialize)]
struct AgentInfo {
    port: u16,
    token: String,
    pid: u32,
}

fn agent_info_path() -> PathBuf {
    crate::settings::config_path()
        .parent()
        .map(|p| p.join("host_agent.json"))
        .unwrap_or_else(|| PathBuf::from("host_agent.json"))
}

/// Binds the loopback listener and writes `host_agent.json` (port + random
/// token) so any process holding the config directory can find and
/// authenticate to it. Shared by `run_host_agent` (Phase 1's full listener,
/// which also forwards HostEvents to an attached GUI) and
/// `spawn_command_listener` (the in-process backend's lighter one, see its
/// doc comment) — both need the exact same discovery mechanism so
/// `--approval-prompt` (Phase 2) can reach whichever one is actually running
/// without caring which.
fn bind_control_endpoint() -> Option<(TcpListener, String)> {
    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let port = listener.local_addr().ok()?.port();
    let token = format!(
        "{:016x}{:016x}",
        std::process::id() as u64,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    );
    let info = AgentInfo {
        port,
        token: token.clone(),
        pid: std::process::id(),
    };
    if let Some(parent) = agent_info_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string(&info) {
        Ok(json) => {
            if let Err(e) = std::fs::write(agent_info_path(), json) {
                eprintln!(
                    "[host-control] failed to write {}: {e}",
                    agent_info_path().display()
                );
                return None;
            }
        }
        Err(_) => return None,
    }
    Some((listener, token))
}

/// Lightweight counterpart to `run_host_agent`'s listener, for the
/// in-process backend (`HostService::start()`'s fallback in `host.rs`).
/// Lets external helper windows — currently just `--approval-prompt`
/// (Phase 2, `approval_prompt.rs`) — always reach whichever process is
/// actually hosting, regardless of whether Phase 1 (`--host-agent`) is
/// enabled. Unlike the agent's listener this never forwards `HostEvent`s
/// anywhere (the in-process GUI already has direct access to them via
/// `HostService::event_rx`) and doesn't own the log file or exit the
/// process on `Stop` — it only relays incoming commands into `command_tx`.
/// Best-effort: if binding fails, external helper windows simply can't
/// reach this process, but hosting itself is unaffected.
pub(crate) fn spawn_command_listener(command_tx: mpsc::Sender<HostCommand>) {
    let Some((listener, token)) = bind_control_endpoint() else {
        return;
    };
    thread::Builder::new()
        .name("host-command-listener".into())
        .spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let Ok(reader_half) = conn.try_clone() else {
                    continue;
                };
                let mut reader = BufReader::new(reader_half);
                let mut first_line = String::new();
                if reader.read_line(&mut first_line).is_err() || first_line.trim() != token {
                    continue;
                }
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            if let Ok(cmd) = serde_json::from_str::<HostCommand>(line.trim()) {
                                let _ = command_tx.send(cmd);
                            }
                        }
                    }
                }
            }
        })
        .ok();
}

fn timestamp_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

// ── GUI side: attach to (or spawn) the agent process ──────────────────────

/// Tries an existing agent first; spawns a fresh `--host-agent` child if none
/// answers. Returns `None` on any failure so the caller can fall back to the
/// proven in-process path — Phase 1 must never be a hard dependency for
/// hosting to work at all.
pub fn connect_or_spawn(config: AppConfig) -> Option<HostService> {
    let stream = try_connect_existing().or_else(|| spawn_and_connect(&config))?;
    let read_half = stream.try_clone().ok()?;
    let write_half = stream.try_clone().ok()?;

    let (event_tx, event_rx) = mpsc::channel::<HostEvent>();
    let (command_tx, command_rx) = mpsc::channel::<HostCommand>();

    spawn_reader(read_half, event_tx);
    spawn_writer(write_half, command_rx);

    Some(HostService::from_agent_link(event_rx, command_tx, stream))
}

/// Connects to an already-running agent and completes the token handshake.
/// `pub(crate)` so `approval_prompt.rs` (Phase 2) can send a one-shot
/// `HostCommand::ApproveIncoming` without going through the full
/// `HostService` reader/writer thread machinery — it just needs to write one
/// line and exit.
pub(crate) fn try_connect_existing() -> Option<TcpStream> {
    let raw = std::fs::read_to_string(agent_info_path()).ok()?;
    let info: AgentInfo = serde_json::from_str(&raw).ok()?;
    let addr: std::net::SocketAddr = format!("127.0.0.1:{}", info.port).parse().ok()?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_millis(400)).ok()?;
    let mut writer = stream.try_clone().ok()?;
    writeln!(writer, "{}", info.token).ok()?;
    Some(stream)
}

fn spawn_and_connect(_config: &AppConfig) -> Option<TcpStream> {
    let exe = std::env::current_exe().ok()?;
    spawn_detached(&exe, &["--host-agent"]).ok()?;

    // Poll for host_agent.json + a successful connect — the agent needs a
    // moment to bind its listener and write the info file.
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(150));
        if let Some(stream) = try_connect_existing() {
            return Some(stream);
        }
    }
    None
}

/// Spawns `exe args…` detached from this process's console/process group
/// (Windows) or session (Unix), so the child survives this process exiting
/// or being killed. Used both for `--host-agent` itself (Phase 1's whole
/// point — closing the GUI must not kill the stream, `TZ_HOST_SERVICE.md`
/// §1-2) and for `--approval-prompt` (Phase 2, `approval_prompt.rs`), which
/// the agent raises on its own and which must equally outlive whatever
/// spawned it.
///
/// A plain `Command::spawn()` is not enough: live-confirmed on this stand, a
/// child spawned that way was torn down together with its parent (killed by
/// the same Ctrl-C/console-close signal, or reaped by a Windows Job Object
/// the parent belongs to — common under terminals and IDE-launched processes
/// that auto-kill their whole descendant tree).
#[cfg(windows)]
pub(crate) fn spawn_detached(
    exe: &std::path::Path,
    args: &[&str],
) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    // DETACHED_PROCESS: no inherited console.
    // CREATE_NEW_PROCESS_GROUP: doesn't receive Ctrl+C/Ctrl+Break aimed at
    // the parent's group.
    // CREATE_BREAKAWAY_FROM_JOB: escapes the parent's Job Object, if any.
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    // CreateProcess fails outright if CREATE_BREAKAWAY_FROM_JOB is set but
    // the parent's job doesn't allow breakaway — retry without it so a
    // restrictive job can't prevent the process from spawning at all (it'll
    // just stay tied to that job in that case, same as before this fix).
    std::process::Command::new(exe)
        .args(args)
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
        .or_else(|_| {
            std::process::Command::new(exe)
                .args(args)
                .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
                .spawn()
        })
}

#[cfg(unix)]
pub(crate) fn spawn_detached(
    exe: &std::path::Path,
    args: &[&str],
) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    // setsid(): new session, detached from the parent's controlling terminal
    // so the child doesn't receive SIGHUP when that terminal/session closes.
    unsafe {
        std::process::Command::new(exe)
            .args(args)
            .pre_exec(|| {
                libc::setsid();
                Ok(())
            })
            .spawn()
    }
}

fn spawn_reader(read_half: TcpStream, event_tx: mpsc::Sender<HostEvent>) {
    thread::Builder::new()
        .name("host-agent-reader".into())
        .spawn(move || {
            let mut lines = BufReader::new(read_half).lines();
            while let Some(Ok(line)) = lines.next() {
                if line.is_empty() {
                    continue;
                }
                if let Ok(event) = serde_json::from_str::<HostEvent>(&line) {
                    if event_tx.send(event).is_err() {
                        return;
                    }
                }
            }
            // Connection lost (agent died, or `detach()` shut it down from
            // this side) — surface it once so the UI doesn't sit on a stale
            // "Ready" state forever.
            let _ = event_tx.send(HostEvent::StateChanged(crate::host::HostState::Idle));
        })
        .ok();
}

fn spawn_writer(mut write_half: TcpStream, command_rx: mpsc::Receiver<HostCommand>) {
    thread::Builder::new()
        .name("host-agent-writer".into())
        .spawn(move || {
            while let Ok(cmd) = command_rx.recv() {
                let Ok(line) = serde_json::to_string(&cmd) else {
                    continue;
                };
                if writeln!(write_half, "{line}").is_err() {
                    return;
                }
                let _ = write_half.flush();
            }
        })
        .ok();
}

// ── Agent side: the `--host-agent` process itself ─────────────────────────

/// Entry point for `--host-agent`. Runs the same in-process host loop
/// (`HostService::start_in_process`) `run_headless_host()` already uses —
/// this process's whole job is to host that loop and bridge it to whichever
/// GUI process is currently attached, forwarding events out and commands in.
pub fn run_host_agent() {
    let config = AppConfig::load_or_create();

    let Some((listener, token)) = bind_control_endpoint() else {
        eprintln!("[host-agent] failed to bind loopback listener / write host_agent.json");
        std::process::exit(1);
    };
    eprintln!(
        "[host-agent] listening on 127.0.0.1:{} pid={}",
        listener.local_addr().map(|a| a.port()).unwrap_or(0),
        std::process::id()
    );

    let svc = HostService::start_in_process(config);
    let command_tx = svc.command_sender();

    // Truncated fresh each run, same convention as the GUI's own log file —
    // see TZ_HOST_SERVICE.md complication #7 ("logs of three processes must
    // fold back into one file"). This process now owns that file exclusively;
    // the GUI, when agent-mode is active, no longer opens it itself (see
    // `host_agent::enabled()` check in `main.rs`) so two processes never
    // write it concurrently.
    let log_file: Arc<Mutex<Option<std::fs::File>>> = Arc::new(Mutex::new(
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open("evertydesk_host_log.txt")
            .ok(),
    ));

    let current_writer: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));

    // Forwarder: every HostEvent goes to the log file (always) and to
    // whichever GUI is currently attached (if any). Detaching a GUI just
    // means `current_writer` goes back to `None` — this thread and the host
    // loop underneath it keep running regardless.
    {
        let current_writer = current_writer.clone();
        let log_file = log_file.clone();
        thread::Builder::new()
            .name("host-agent-forward".into())
            .spawn(move || {
                while let Ok(event) = svc.event_rx.recv() {
                    if let HostEvent::Log(ref msg) = event {
                        if let Ok(mut f) = log_file.lock() {
                            if let Some(file) = f.as_mut() {
                                let _ = writeln!(file, "[{}] {msg}", timestamp_hms());
                                let _ = file.flush();
                            }
                        }
                    }
                    if let HostEvent::ApprovalRequested { ref peer_id, .. } = event {
                        // Phase 2: always a separate OS window (RustDesk/
                        // AnyDesk-style), not just when no GUI is attached —
                        // an in-app modal is invisible whenever the main
                        // window is minimized or not focused, which defeats
                        // the point for the one notification that most needs
                        // to interrupt the user. The attached GUI (if any)
                        // still gets this event forwarded below for its log,
                        // but does not raise its own prompt for it — see
                        // `HostEvent::ApprovalRequested` in main.rs.
                        let ok = raise_approval_prompt(peer_id);
                        if let Ok(mut f) = log_file.lock() {
                            if let Some(file) = f.as_mut() {
                                let _ = writeln!(
                                    file,
                                    "[{}] [host-agent] approval-prompt window for {peer_id}: {}",
                                    timestamp_hms(),
                                    if ok { "raised" } else { "FAILED to raise" }
                                );
                                let _ = file.flush();
                            }
                        }
                    }
                    if let HostEvent::SessionStarted { ref peer_id, .. } = event {
                        // Same reasoning as ApprovalRequested above: always a
                        // separate window, regardless of GUI attachment.
                        let ok = raise_session_toolbar(peer_id);
                        if let Ok(mut f) = log_file.lock() {
                            if let Some(file) = f.as_mut() {
                                let _ = writeln!(
                                    file,
                                    "[{}] [host-agent] session toolbar for {peer_id}: {}",
                                    timestamp_hms(),
                                    if ok { "raised" } else { "FAILED to raise" }
                                );
                                let _ = file.flush();
                            }
                        }
                    }
                    if let Ok(line) = serde_json::to_string(&event) {
                        let mut slot = current_writer.lock().unwrap();
                        if let Some(stream) = slot.as_mut() {
                            if writeln!(stream, "{line}").is_err() || stream.flush().is_err() {
                                *slot = None;
                            }
                        }
                    }
                }
                // The host loop thread ended (`HostCommand::Stop` was
                // processed) — nothing left to serve, exit the process so a
                // future GUI launch spawns a fresh agent instead of finding
                // this now-empty shell.
                std::process::exit(0);
            })
            .ok();
    }

    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let Ok(reader_half) = conn.try_clone() else {
            continue;
        };
        let mut reader = BufReader::new(reader_half);

        let mut first_line = String::new();
        if reader.read_line(&mut first_line).is_err() || first_line.trim() != token {
            continue; // wrong/missing token — drop this connection
        }

        if let (Ok(mut slot), Ok(writer_half)) = (current_writer.lock(), conn.try_clone()) {
            *slot = Some(writer_half);
        }

        // Blocking: serve this GUI's commands until it disconnects, then
        // loop back to `accept()` for the next one (or the same one, on
        // reconnect after the GUI relaunches).
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break, // EOF or read error — GUI disconnected
                Ok(_) => {
                    let Ok(cmd) = serde_json::from_str::<HostCommand>(line.trim()) else {
                        continue;
                    };
                    let is_stop = matches!(cmd, HostCommand::Stop);
                    let _ = command_tx.send(cmd);
                    if is_stop {
                        // Give the forwarder thread a moment to flush the
                        // resulting StateChanged(Idle) to this connection
                        // before the process exits (see the forwarder above).
                        thread::sleep(Duration::from_millis(300));
                    }
                }
            }
        }
    }
}

/// Phase 2: raises a standalone confirmation window for an incoming
/// connection when no GUI is currently attached to answer it. See
/// `approval_prompt.rs`. Detached the same way `--host-agent` itself is —
/// it must outlive whatever triggered it.
fn raise_approval_prompt(peer_id: &str) -> bool {
    #[cfg(feature = "desktop-gui")]
    {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        spawn_detached(&exe, &["--approval-prompt", peer_id]).is_ok()
    }
    #[cfg(not(feature = "desktop-gui"))]
    {
        eprintln!(
            "[host-agent] Approval requested for {peer_id} but this build has no GUI \
             (desktop-gui feature off) — the request will time out."
        );
        false
    }
}

/// Raises the standalone AnyDesk/RustDesk-style session toolbar (see
/// `session_toolbar.rs`) for a session that just started. Same detached
/// spawn as `raise_approval_prompt` — must outlive whatever triggered it.
fn raise_session_toolbar(peer_id: &str) -> bool {
    #[cfg(feature = "desktop-gui")]
    {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        spawn_detached(&exe, &["--session-toolbar", peer_id]).is_ok()
    }
    #[cfg(not(feature = "desktop-gui"))]
    {
        let _ = peer_id;
        false
    }
}
