//! Phase 3 of `TZ_HOST_SERVICE.md`: a Windows service running in Session 0
//! that launches `--host-agent` (Phase 1, `host_agent.rs`) into whichever
//! session is the active interactive console session, via
//! `WTSQueryUserToken` + `CreateProcessAsUser` — the standard pattern for
//! Session-0-isolated services that need to reach the user's desktop (this
//! is how RustDesk's own service does it).
//!
//! **UNTESTED.** Written per explicit instruction to implement this without
//! installing or running it: installing a service needs admin rights, and
//! verifying the Session-0 → user-session handoff needs a real interactive
//! logon session — neither is available in the sandboxed environment this
//! was written in. Validate on real hardware (`--install-service`, confirm
//! the service starts, confirm a `--host-agent` process appears in your
//! logon session, confirm it survives logoff/logon) before relying on it.
//!
//! Covered (see `TZ_HOST_SERVICE.md` §4 complications by number):
//! - #1 Session 0 isolation — `WTSQueryUserToken` + `CreateProcessAsUser`.
//! - #4 session changes — `SERVICE_CONTROL_SESSIONCHANGE` re-evaluates and
//!   relaunches the agent in whatever session is active now.
//! - #8 liveness — the supervisor loop relaunches the agent if its process
//!   handle becomes signaled (exited) for any reason.
//! - #9 compatibility — install/uninstall are separate from normal exe
//!   launch; running the exe directly is completely unaffected.
//!
//! **Not covered** (left for follow-up once this lands and is validated):
//! - #2 UAC/UIPI — the agent launched here runs at the logon user's normal
//!   integrity level, same as running the exe by hand today. It cannot
//!   inject input into elevated windows. Not a regression, just not yet
//!   improved by this phase.
//! - #3 secure desktop (UAC prompt / Ctrl+Alt+Del / lock screen) — not
//!   handled; the agent's capture/input will behave the same as it does
//!   today when a secure desktop is active (i.e. cannot see or reach it).
//! - #10 Linux/macOS — see `TZ_HOST_SERVICE.md` Phase 4 / task #5.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Security::{GetTokenInformation, TokenLinkedToken, TOKEN_LINKED_TOKEN};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, CreateServiceW, DeleteService, OpenSCManagerW,
    OpenServiceW, QueryServiceStatus, RegisterServiceCtrlHandlerExW, SetServiceStatus,
    StartServiceCtrlDispatcherW, StartServiceW, SC_MANAGER_ALL_ACCESS, SC_MANAGER_CONNECT,
    SERVICE_ACCEPT_SESSIONCHANGE, SERVICE_ACCEPT_STOP, SERVICE_ALL_ACCESS, SERVICE_AUTO_START,
    SERVICE_CONTROL_SESSIONCHANGE, SERVICE_CONTROL_STOP, SERVICE_ERROR_NORMAL,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS,
    SERVICE_STATUS_HANDLE, SERVICE_STOPPED, SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW,
    SERVICE_WIN32_OWN_PROCESS,
};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, TerminateProcess, WaitForSingleObject, CREATE_NEW_CONSOLE,
    CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const SERVICE_NAME: &str = "EvertyDeskLiteHostSvc";
const SERVICE_DISPLAY_NAME: &str = "EvertyDesk Lite Host Service";

struct ServiceState {
    status_handle: AtomicIsize,
    stop_requested: AtomicBool,
    session_change_seq: AtomicU32,
}

static STATE: ServiceState = ServiceState {
    status_handle: AtomicIsize::new(0),
    stop_requested: AtomicBool::new(false),
    session_change_seq: AtomicU32::new(0),
};

fn wide_null(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

// ── SCM entry point (`--winservice`) ───────────────────────────────────────

/// Blocks for the lifetime of the service. Only succeeds when actually
/// launched by the Service Control Manager — running `--winservice` by hand
/// fails immediately with `ERROR_FAILED_SERVICE_CONTROLLER_CONNECT`, which is
/// expected (there is no SCM to answer the handshake outside a real service
/// start).
pub fn run_winservice() {
    let mut name = wide_null(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(name.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR::null(),
            lpServiceProc: None,
        },
    ];
    unsafe {
        if !StartServiceCtrlDispatcherW(table.as_ptr()).as_bool() {
            eprintln!(
                "[winservice] StartServiceCtrlDispatcherW failed (not started by SCM?): {:?}",
                GetLastError()
            );
        }
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let mut name = wide_null(SERVICE_NAME);
    let handle =
        match RegisterServiceCtrlHandlerExW(PCWSTR(name.as_mut_ptr()), Some(control_handler), None)
        {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[winservice] RegisterServiceCtrlHandlerExW failed: {e}");
                return;
            }
        };
    STATE.status_handle.store(handle.0, Ordering::SeqCst);

    report_status(SERVICE_START_PENDING, 1000, 0);
    report_status(SERVICE_RUNNING, 0, 0);

    run_supervisor_loop();

    report_status(SERVICE_STOPPED, 0, 0);
}

unsafe extern "system" fn control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut core::ffi::c_void,
    _context: *mut core::ffi::c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP => {
            STATE.stop_requested.store(true, Ordering::SeqCst);
            report_status(SERVICE_STOP_PENDING, 3000, 0);
            0 // NO_ERROR
        }
        SERVICE_CONTROL_SESSIONCHANGE => {
            // event_type distinguishes LOGON/LOGOFF/LOCK/UNLOCK — we don't
            // discriminate: any session change is cheap to just re-evaluate
            // against WTSGetActiveConsoleSessionId in the supervisor loop.
            STATE.session_change_seq.fetch_add(1, Ordering::SeqCst);
            0
        }
        _ => 1, // ERROR_CALL_NOT_IMPLEMENTED
    }
}

fn report_status(
    state: windows::Win32::System::Services::SERVICE_STATUS_CURRENT_STATE,
    wait_hint_ms: u32,
    checkpoint: u32,
) {
    let raw = STATE.status_handle.load(Ordering::SeqCst);
    if raw == 0 {
        return;
    }
    let controls_accepted = if state.0 == SERVICE_RUNNING.0 {
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SESSIONCHANGE
    } else {
        0
    };
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: controls_accepted,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: checkpoint,
        dwWaitHint: wait_hint_ms,
    };
    unsafe {
        let _ = SetServiceStatus(SERVICE_STATUS_HANDLE(raw), &status);
    }
}

// ── Supervisor: keeps a --host-agent alive in the active session ──────────

fn run_supervisor_loop() {
    let mut current: Option<(u32, HANDLE)> = None;
    let mut last_seq = STATE.session_change_seq.load(Ordering::SeqCst);

    loop {
        if STATE.stop_requested.load(Ordering::SeqCst) {
            break;
        }

        let active_session = unsafe { WTSGetActiveConsoleSessionId() };
        let seq_now = STATE.session_change_seq.load(Ordering::SeqCst);
        let session_changed = seq_now != last_seq;
        last_seq = seq_now;

        let child_dead = current
            .as_ref()
            .map(|(_, h)| unsafe { WaitForSingleObject(*h, 0) } == WAIT_OBJECT_0)
            .unwrap_or(false);
        let wrong_session = current
            .as_ref()
            .map(|(s, _)| *s != active_session)
            .unwrap_or(true);

        if session_changed || child_dead || wrong_session {
            if let Some((_, h)) = current.take() {
                unsafe {
                    let _ = TerminateProcess(h, 0);
                    let _ = CloseHandle(h);
                }
            }
            // 0xFFFFFFFF (no active session, e.g. locked/no one logged on
            // yet) — nothing to launch into; wait and re-check.
            if active_session != 0xFFFF_FFFF {
                match launch_agent_in_session(active_session) {
                    Ok(h) => current = Some((active_session, h)),
                    Err(e) => {
                        eprintln!("[winservice] launch into session {active_session} failed: {e}")
                    }
                }
            }
        }

        thread::sleep(Duration::from_secs(2));
    }

    if let Some((_, h)) = current.take() {
        unsafe {
            let _ = TerminateProcess(h, 0);
            let _ = CloseHandle(h);
        }
    }
}

/// If `user_token` is a UAC "split" limited token (the normal case for an
/// administrator account with UAC enabled), returns its linked full token —
/// the one with the Administrators group actually enabled, not just present
/// disabled. `None` if there is no linked token (UAC off, standard non-admin
/// account, or this already is the full token) — caller should fall back to
/// `user_token` unchanged in that case, which is exactly today's
/// non-elevated behavior.
///
/// Why this matters: without it, `CreateProcessAsUserW` launches the agent
/// at the same reduced integrity level as an ordinary non-elevated process.
/// Windows' UIPI then silently drops any input the agent tries to inject
/// into a higher-integrity window — Task Manager (elevated by default),
/// another elevated app, a UAC prompt. That is exactly the "control is lost
/// when Task Manager is open and the program wasn't run as administrator"
/// symptom this fixes — and the same linked-token trick RustDesk's own
/// service uses for it.
unsafe fn linked_elevated_token(user_token: HANDLE) -> Option<HANDLE> {
    let mut linked = TOKEN_LINKED_TOKEN::default();
    let mut needed = 0u32;
    let ok = GetTokenInformation(
        user_token,
        TokenLinkedToken,
        Some(&mut linked as *mut _ as *mut core::ffi::c_void),
        std::mem::size_of::<TOKEN_LINKED_TOKEN>() as u32,
        &mut needed,
    );
    if ok.as_bool() && !linked.LinkedToken.is_invalid() {
        Some(linked.LinkedToken)
    } else {
        None
    }
}

fn launch_agent_in_session(session_id: u32) -> Result<HANDLE, String> {
    unsafe {
        let mut user_token = HANDLE::default();
        if !WTSQueryUserToken(session_id, &mut user_token).as_bool() {
            return Err(format!("WTSQueryUserToken: {:?}", GetLastError()));
        }

        // Prefer the linked elevated token when the account has one (see
        // `linked_elevated_token`'s doc comment) so the agent can reach
        // elevated windows. `launch_token` is what we actually launch and
        // build the environment block with; `user_token` (from
        // WTSQueryUserToken) is always closed once we're done, and
        // `launch_token` is closed too when it's the separate linked one.
        let elevated = linked_elevated_token(user_token);
        let launch_token = elevated.unwrap_or(user_token);

        let mut env_block: *mut core::ffi::c_void = std::ptr::null_mut();
        let _ = CreateEnvironmentBlock(&mut env_block, launch_token, false);

        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut cmdline = wide_null(&format!("\"{}\" --host-agent", exe.display()));
        let mut desktop = wide_null("winsta0\\default");

        let mut si = STARTUPINFOW::default();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.lpDesktop = PWSTR(desktop.as_mut_ptr());
        let mut pi = PROCESS_INFORMATION::default();

        let ok = CreateProcessAsUserW(
            launch_token,
            PCWSTR::null(),
            PWSTR(cmdline.as_mut_ptr()),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT | CREATE_NEW_CONSOLE,
            Some(env_block),
            PCWSTR::null(),
            &si,
            &mut pi,
        );

        if !env_block.is_null() {
            let _ = DestroyEnvironmentBlock(env_block);
        }
        if let Some(elevated_token) = elevated {
            let _ = CloseHandle(elevated_token);
        }
        let _ = CloseHandle(user_token);

        if !ok.as_bool() {
            return Err(format!("CreateProcessAsUserW: {:?}", GetLastError()));
        }
        let _ = CloseHandle(pi.hThread);
        Ok(pi.hProcess)
    }
}

// ── Install / uninstall (`--install-service` / `--uninstall-service`) ─────

pub fn install_service() -> Result<(), String> {
    unsafe {
        let sc_manager = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_ALL_ACCESS)
            .map_err(|e| format!("OpenSCManagerW: {e} (нужны права администратора)"))?;

        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut bin_path = wide_null(&format!("\"{}\" --winservice", exe.display()));
        let mut name = wide_null(SERVICE_NAME);
        let mut display = wide_null(SERVICE_DISPLAY_NAME);

        let result = CreateServiceW(
            sc_manager,
            PCWSTR(name.as_mut_ptr()),
            PCWSTR(display.as_mut_ptr()),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            PCWSTR(bin_path.as_mut_ptr()),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        );

        let _ = CloseServiceHandle(sc_manager);
        match result {
            Ok(svc) => {
                // Start it immediately — the main-window "install service"
                // button is meant to be one click (one UAC prompt), not
                // install-then-remember-to-separately-start.
                let start_ok = StartServiceW(svc, None).as_bool();
                let _ = CloseServiceHandle(svc);
                if !start_ok {
                    return Err(format!(
                        "Installed, but StartServiceW failed: {:?}",
                        GetLastError()
                    ));
                }
                Ok(())
            }
            Err(e) => Err(format!("CreateServiceW: {e}")),
        }
    }
}

/// Read-only, no admin rights needed — safe to call every frame from the
/// GUI to decide what to show in the "install service" hint.
pub fn is_service_installed() -> bool {
    unsafe {
        let Ok(sc_manager) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)
        else {
            return false;
        };
        let mut name = wide_null(SERVICE_NAME);
        let result = OpenServiceW(sc_manager, PCWSTR(name.as_mut_ptr()), SERVICE_QUERY_STATUS);
        let _ = CloseServiceHandle(sc_manager);
        match result {
            Ok(svc) => {
                let _ = CloseServiceHandle(svc);
                true
            }
            Err(_) => false,
        }
    }
}

/// Read-only, no admin rights needed. `false` if not installed at all.
pub fn is_service_running() -> bool {
    unsafe {
        let Ok(sc_manager) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)
        else {
            return false;
        };
        let mut name = wide_null(SERVICE_NAME);
        let svc = OpenServiceW(sc_manager, PCWSTR(name.as_mut_ptr()), SERVICE_QUERY_STATUS);
        let _ = CloseServiceHandle(sc_manager);
        let Ok(svc) = svc else { return false };
        let mut status = SERVICE_STATUS::default();
        let ok = QueryServiceStatus(svc, &mut status);
        let _ = CloseServiceHandle(svc);
        ok.as_bool() && status.dwCurrentState.0 == SERVICE_RUNNING.0
    }
}

/// Re-launches this exe with `args`, elevated via a single UAC prompt
/// (`ShellExecuteW` with `lpOperation = "runas"`), for actions that need
/// admin rights the GUI itself doesn't have — installing/uninstalling the
/// service. Fire-and-forget: returns once the elevated process has
/// *started*, not once it's done; the caller should re-poll
/// `is_service_installed()`/`is_service_running()` after a short delay
/// rather than assume immediate completion.
pub fn relaunch_elevated(args: &[&str]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut exe_wide = wide_null(&exe.display().to_string());
    let mut verb = wide_null("runas");
    let mut params = wide_null(&args.join(" "));
    unsafe {
        let result = ShellExecuteW(
            None,
            PCWSTR(verb.as_mut_ptr()),
            PCWSTR(exe_wide.as_mut_ptr()),
            PCWSTR(params.as_mut_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        // ShellExecuteW returns a pseudo-HINSTANCE; values > 32 mean success,
        // per its documented (if archaic) contract. Common failure: 1223
        // (ERROR_CANCELLED) when the user dismisses the UAC prompt.
        if result.0 > 32 {
            Ok(())
        } else {
            Err(format!("ShellExecuteW returned {}", result.0))
        }
    }
}

pub fn start_installed_service() -> Result<(), String> {
    unsafe {
        let sc_manager = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_ALL_ACCESS)
            .map_err(|e| format!("OpenSCManagerW: {e}"))?;
        let mut name = wide_null(SERVICE_NAME);
        let svc = OpenServiceW(sc_manager, PCWSTR(name.as_mut_ptr()), SERVICE_ALL_ACCESS)
            .map_err(|e| format!("OpenServiceW: {e}"));
        let _ = CloseServiceHandle(sc_manager);
        let svc = svc?;
        let ok = StartServiceW(svc, None);
        let _ = CloseServiceHandle(svc);
        if ok.as_bool() {
            Ok(())
        } else {
            Err(format!("StartServiceW: {:?}", GetLastError()))
        }
    }
}

pub fn uninstall_service() -> Result<(), String> {
    unsafe {
        let sc_manager = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_ALL_ACCESS)
            .map_err(|e| format!("OpenSCManagerW: {e} (нужны права администратора)"))?;
        let mut name = wide_null(SERVICE_NAME);
        let svc = OpenServiceW(sc_manager, PCWSTR(name.as_mut_ptr()), SERVICE_ALL_ACCESS)
            .map_err(|e| format!("OpenServiceW: {e}"));
        let svc = match svc {
            Ok(s) => s,
            Err(e) => {
                let _ = CloseServiceHandle(sc_manager);
                return Err(e);
            }
        };

        let mut status = SERVICE_STATUS::default();
        let _ = ControlService(svc, SERVICE_CONTROL_STOP, &mut status);

        let ok = DeleteService(svc);
        let _ = CloseServiceHandle(svc);
        let _ = CloseServiceHandle(sc_manager);

        if ok.as_bool() {
            Ok(())
        } else {
            Err(format!("DeleteService: {:?}", GetLastError()))
        }
    }
}
