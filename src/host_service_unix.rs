//! Phase 4 of `TZ_HOST_SERVICE.md`: Linux (systemd user service) and macOS
//! (launchd agent) equivalents of the Windows service in `winservice.rs`.
//!
//! Neither platform has a Session-0-style isolation problem — a systemd
//! user service already runs inside the user's own login session (with
//! access to its D-Bus session bus, and on a `systemd --user` instance that
//! starts at login, its desktop too, given `XDG_SESSION_TYPE`/`DISPLAY`/
//! `WAYLAND_DISPLAY` are inherited or looked up), and a launchd
//! `LaunchAgent` (as opposed to a system-wide `LaunchDaemon`) is likewise
//! already scoped to one user's GUI session. So Phase 3's whole
//! Session-0-supervisor design has no equivalent here: both of these just
//! run `--host-agent` (Phase 1) directly, under the OS's own
//! restart-on-failure supervision instead of a hand-rolled one.
//!
//! **Written but not compiled**: this dev environment is Windows, so this
//! file has never been through `cargo check` on its actual target. Validate
//! the exact unit/plist syntax and `systemctl --user` / `launchctl`
//! invocations on real Linux/macOS hardware before relying on it.

use std::path::PathBuf;
use std::process::Command;

#[cfg(target_os = "linux")]
const SERVICE_NAME: &str = "evertydesk-lite-host.service";
#[cfg(target_os = "macos")]
const LAUNCHD_LABEL: &str = "ru.everty.desklite.host";

#[cfg(target_os = "linux")]
fn systemd_user_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/systemd/user"))
}

#[cfg(target_os = "linux")]
fn unit_path() -> Option<PathBuf> {
    systemd_user_dir().map(|d| d.join(SERVICE_NAME))
}

#[cfg(target_os = "macos")]
fn launch_agents_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/LaunchAgents"))
}

#[cfg(target_os = "macos")]
fn plist_path() -> Option<PathBuf> {
    launch_agents_dir().map(|d| d.join(format!("{LAUNCHD_LABEL}.plist")))
}

#[cfg(target_os = "linux")]
pub fn install_service() -> Result<(), String> {
    let dir = systemd_user_dir().ok_or("HOME not set")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = unit_path().ok_or("HOME not set")?;

    let unit = format!(
        "[Unit]\n\
         Description=EvertyDesk Lite Host Agent\n\
         After=graphical-session.target\n\
         \n\
         [Service]\n\
         ExecStart=\"{}\" --host-agent\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exe.display()
    );
    std::fs::write(&path, unit).map_err(|e| format!("write {}: {e}", path.display()))?;

    run_ok(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
    run_ok(Command::new("systemctl").args(["--user", "enable", SERVICE_NAME]))?;
    // One click installs *and* starts — matches the Windows path
    // (winservice::install_service also starts immediately after creating).
    start_installed_service()
}

#[cfg(target_os = "linux")]
pub fn start_installed_service() -> Result<(), String> {
    run_ok(Command::new("systemctl").args(["--user", "start", SERVICE_NAME]))
}

/// Read-only — safe to call from the GUI to decide what hint to show.
#[cfg(target_os = "linux")]
pub fn is_service_installed() -> bool {
    unit_path().map(|p| p.exists()).unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub fn is_service_running() -> bool {
    Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", SERVICE_NAME])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub fn uninstall_service() -> Result<(), String> {
    // Best-effort: keep going even if the service was already stopped/absent.
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", SERVICE_NAME])
        .status();
    if let Some(path) = unit_path() {
        let _ = std::fs::remove_file(&path);
    }
    run_ok(Command::new("systemctl").args(["--user", "daemon-reload"]))
}

#[cfg(target_os = "macos")]
pub fn install_service() -> Result<(), String> {
    let dir = launch_agents_dir().ok_or("HOME not set")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = plist_path().ok_or("HOME not set")?;

    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key><string>{LAUNCHD_LABEL}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>{}</string>\n\
         \t\t<string>--host-agent</string>\n\
         \t</array>\n\
         \t<key>RunAtLoad</key><true/>\n\
         \t<key>KeepAlive</key><true/>\n\
         \t<key>ProcessType</key><string>Interactive</string>\n\
         </dict>\n\
         </plist>\n",
        exe.display()
    );
    std::fs::write(&path, plist).map_err(|e| format!("write {}: {e}", path.display()))?;

    run_ok(Command::new("launchctl").args(["load", "-w", &path.display().to_string()]))
}

#[cfg(target_os = "macos")]
pub fn start_installed_service() -> Result<(), String> {
    run_ok(Command::new("launchctl").args(["start", LAUNCHD_LABEL]))
}

#[cfg(target_os = "macos")]
pub fn uninstall_service() -> Result<(), String> {
    if let Some(path) = plist_path() {
        let _ = Command::new("launchctl")
            .args(["unload", "-w", &path.display().to_string()])
            .status();
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

/// Read-only — safe to call from the GUI to decide what hint to show.
#[cfg(target_os = "macos")]
pub fn is_service_installed() -> bool {
    plist_path().map(|p| p.exists()).unwrap_or(false)
}

#[cfg(target_os = "macos")]
pub fn is_service_running() -> bool {
    Command::new("launchctl")
        .args(["list", LAUNCHD_LABEL])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_ok(cmd: &mut Command) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("{cmd:?}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{cmd:?} exited with {status}"))
    }
}
