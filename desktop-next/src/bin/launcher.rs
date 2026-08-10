#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use evertydesk_core::host::{HostCommand, HostEvent, HostService, HostState};
#[cfg(windows)]
use evertydesk_core::hyperv;
use evertydesk_core::settings::{
    generate_numeric_token, AppConfig, ContactEntry, FsrQualitySetting, ServerConfig, StreamingMode,
};
use evertydesk_core::virtualbox;
use evertydesk_core::vm_bridge;
use evertydesk_desktop_next::credential_store;
use evertydesk_desktop_next::i18n::{tr, TextKey, UiLanguage};
use evertydesk_desktop_next::ipc::{read_bounded_line, MAX_IPC_LINE_BYTES};
#[cfg(test)]
use evertydesk_desktop_next::launcher_store::DEFAULT_UPDATE_GITHUB_REPO;
use evertydesk_desktop_next::launcher_store::{
    normalize_contact_tags, ConnectionDirection, Contact, GameCodecPreference, LanguagePreference,
    LauncherStore, RecentConnection, UpdateChannelPreference, VmProviderPreference,
};
use evertydesk_desktop_next::protocol::{
    ConnectionQuality, RdpBootstrap, RdpTarget, ViewerBootstrap, ViewerCommand, ViewerControl,
    ViewerGameCodec, ViewerScaling, ViewerStatus,
};
use evertydesk_desktop_next::smart_agent::{
    self, AgentNotification, AgentOperator, HeartbeatRequest, SupportRequest,
};
use evertydesk_desktop_next::startup_log::install_process_diagnostics;
use evertydesk_desktop_next::updater;
use evertydesk_desktop_next::viewer_process::{spawn_viewer, ViewerProcess};
use evertydesk_desktop_next::windows_app::{
    set_current_process_app_user_model_id, WindowsAppUserModelId,
};
use iced::widget::{
    button, checkbox, column, container, row, scrollable, svg, text, text_input, tooltip, Row,
    Space,
};
use iced::{
    border, theme::Palette, Alignment, Background, Border, Color, Element, Fill, Length, Shadow,
    Size, Subscription, Task, Theme, Vector,
};
use std::collections::{hash_map::DefaultHasher, BTreeMap, BTreeSet, VecDeque};
use std::env;
#[cfg(windows)]
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::PathBuf;
#[cfg(windows)]
use std::process::Command;
use std::process::{ChildStderr, ChildStdout};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
#[cfg(windows)]
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
#[cfg(windows)]
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use zeroize::{Zeroize, Zeroizing};

const ACCENT: Color = Color::from_rgb(0.91, 0.13, 0.16);
const TEXT: Color = Color::from_rgb(0.12, 0.13, 0.15);
const MUTED: Color = Color::from_rgb(0.43, 0.46, 0.50);
const SURFACE: Color = Color::WHITE;
const CANVAS: Color = Color::from_rgb(0.965, 0.969, 0.976);
const LINE: Color = Color::from_rgb(0.88, 0.89, 0.91);
const PASSWORD_CLIPBOARD_TTL: Duration = Duration::from_secs(30);
const TEMP_PASSWORD_ROTATION_INTERVAL: Duration = Duration::from_secs(10 * 60);
const MAX_PERMANENT_PASSWORD_CHARS: usize = 128;
const APPROVAL_UI_TIMEOUT: Duration = Duration::from_secs(40);
const SINGLE_INSTANCE_PORT: u16 = 47_831;
const SINGLE_INSTANCE_REQUEST: &str = "EVERTYDESK_LAUNCHER_FOCUS_V1";
const SINGLE_INSTANCE_BACKGROUND_REQUEST: &str = "EVERTYDESK_LAUNCHER_BACKGROUND_V1";
const SINGLE_INSTANCE_RESPONSE: &str = "EVERTYDESK_LAUNCHER_PRIMARY_V1";
const SINGLE_INSTANCE_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_STARTUP_ENV_VALUE_CHARS: usize = 96;
const MAX_VIEWER_DIAGNOSTIC_BYTES: usize = 4 * 1024;
const MAX_VIEWER_DIAGNOSTIC_CHARS: usize = 320;
const MAX_VIEWER_DIAGNOSTICS: usize = 8;
const VIEWER_EXIT_STATUS_TIMEOUT: Duration = Duration::from_millis(250);
const VIEWER_STARTUP_TIMEOUT: Duration = Duration::from_secs(8);
const VIEWER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const VIEWER_CONTROL_TIMEOUT: Duration = Duration::from_secs(3);
const VIEWER_LIVENESS_TIMEOUT: Duration = Duration::from_secs(7);
const MAX_ACTIVE_VIEWERS: usize = 8;
const SMART_AGENT_API_URL: &str = "https://desk.everty.ru";
const SMART_AGENT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
const SMART_AGENT_INBOX_INTERVAL: Duration = Duration::from_secs(30);
const SMART_AGENT_BURST_INTERVAL: Duration = Duration::from_secs(5);
const SMART_AGENT_BURST_DURATION: Duration = Duration::from_secs(60);
const MAX_SMART_AGENT_NOTIFICATIONS: usize = 12;
const MAX_SUPPORT_MESSAGE_CHARS: usize = 500;
const MAIN_CONTENT_MAX_WIDTH: f32 = 1_520.0;
const MAIN_CONTENT_VERTICAL_PADDING: f32 = 28.0;
const MAIN_CONTENT_SIDE_PADDING: f32 = 28.0;

type EventBus = (
    async_channel::Sender<ProcessEvent>,
    async_channel::Receiver<ProcessEvent>,
);

static EVENT_BUS: OnceLock<EventBus> = OnceLock::new();

pub fn main() -> iced::Result {
    // Service/agent CLI modes (Phase 1/3, TZ_HOST_SERVICE.md) must be
    // handled before anything else: `winservice::install_service` points
    // the Windows service's binPath at `"<current_exe>" --winservice`, and
    // the Session-0 supervisor launches `"<current_exe>" --host-agent` into
    // the interactive session. Since this binary (not the old
    // `evertydesk-lite.exe`) is `current_exe()` when the install button is
    // clicked from here, it must understand these flags itself — there is
    // no separate binary handling them. None of these enter the iced event
    // loop or the single-instance guard; they exit directly.
    if let Some(exit_code) = handle_service_cli() {
        std::process::exit(exit_code);
    }

    // Phase 1 (TZ_HOST_SERVICE.md) defaults to on for desktop-next: hosting
    // runs in a separate `--host-agent` process (this same binary, spawned
    // detached — see `host_agent::spawn_detached`) so a launcher crash,
    // close, or kill doesn't end an active session, matching the resilience
    // the OS-service path above (B2) already offers. `HostService::start`
    // reads this once per call, so setting it here — before any window
    // opens or `start_hosting()` runs — is sufficient. An operator can still
    // force it off (e.g. while diagnosing an agent-mode-specific issue) by
    // setting EVERTYDESK_HOST_AGENT=0 before launching; only an *unset* var
    // gets this default.
    if std::env::var_os("EVERTYDESK_HOST_AGENT").is_none() {
        // SAFETY: single-threaded at this point in main(), before any
        // thread that could read the environment concurrently is spawned.
        unsafe {
            std::env::set_var("EVERTYDESK_HOST_AGENT", "1");
        }
    }

    install_process_diagnostics("launcher");
    set_current_process_app_user_model_id(WindowsAppUserModelId::Launcher);
    let start_in_background = launcher_start_in_background();
    match claim_single_instance(start_in_background) {
        Ok(true) => {}
        Ok(false) => return Ok(()),
        Err(error) => {
            eprintln!("[launcher] single-instance guard failed: {error}");
            return Ok(());
        }
    }
    log_launcher_startup();

    iced::daemon(Launcher::new, Launcher::update, Launcher::view_window)
        .title(Launcher::window_title)
        .theme(|launcher: &Launcher, _window: iced::window::Id| launcher.theme())
        .subscription(Launcher::subscription)
        .run()
}

/// Handles the service/agent CLI surface — see the comment at its call site
/// in `main()`. Returns `Some(exit_code)` if `argv[1]` was one of these
/// modes (caller must exit immediately, never enter the iced event loop),
/// `None` for a normal launcher start.
fn handle_service_cli() -> Option<i32> {
    let command = std::env::args().nth(1)?;
    match command.as_str() {
        "--host-agent" => {
            evertydesk_core::host_agent::run_host_agent();
            Some(0)
        }
        #[cfg(windows)]
        "--winservice" => {
            evertydesk_core::winservice::run_winservice();
            Some(0)
        }
        #[cfg(windows)]
        "--install-service" => Some(cli_result(evertydesk_core::winservice::install_service())),
        #[cfg(windows)]
        "--start-service" => Some(cli_result(
            evertydesk_core::winservice::start_installed_service(),
        )),
        #[cfg(windows)]
        "--uninstall-service" => Some(cli_result(evertydesk_core::winservice::uninstall_service())),
        #[cfg(unix)]
        "--install-service" => Some(cli_result(
            evertydesk_core::host_service_unix::install_service(),
        )),
        #[cfg(unix)]
        "--start-service" => Some(cli_result(
            evertydesk_core::host_service_unix::start_installed_service(),
        )),
        #[cfg(unix)]
        "--uninstall-service" => Some(cli_result(
            evertydesk_core::host_service_unix::uninstall_service(),
        )),
        _ => None,
    }
}

fn cli_result(result: Result<(), String>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("[launcher] {error}");
            1
        }
    }
}

fn launcher_start_in_background() -> bool {
    env::args()
        .skip(1)
        .any(|argument| matches!(argument.as_str(), "--background" | "--tray" | "--minimized"))
}

fn main_window_settings() -> iced::window::Settings {
    let mut window = iced::window::Settings {
        size: Size::new(920.0, 720.0),
        min_size: Some(Size::new(820.0, 640.0)),
        position: iced::window::Position::Centered,
        ..Default::default()
    };
    #[cfg(windows)]
    {
        window.exit_on_close_request = false;
        window.icon = iced::window::icon::from_rgba(tray_icon_rgba(), 32, 32).ok();
    }
    window
}

fn background_init_window_settings() -> iced::window::Settings {
    let mut window = main_window_settings();
    window.visible = false;
    window.decorations = false;
    window.min_size = None;
    window.size = Size::new(1.0, 1.0);
    window
}

fn incoming_window_settings() -> iced::window::Settings {
    let mut window = iced::window::Settings {
        size: Size::new(560.0, 450.0),
        min_size: Some(Size::new(540.0, 430.0)),
        max_size: Some(Size::new(660.0, 540.0)),
        position: iced::window::Position::Centered,
        resizable: true,
        minimizable: true,
        level: iced::window::Level::AlwaysOnTop,
        exit_on_close_request: false,
        ..Default::default()
    };
    #[cfg(windows)]
    {
        window.icon = iced::window::icon::from_rgba(tray_icon_rgba(), 32, 32).ok();
    }
    window
}

fn credential_window_settings() -> iced::window::Settings {
    let mut window = iced::window::Settings {
        size: Size::new(500.0, 380.0),
        min_size: Some(Size::new(460.0, 360.0)),
        max_size: Some(Size::new(580.0, 440.0)),
        position: iced::window::Position::Centered,
        resizable: false,
        minimizable: false,
        level: iced::window::Level::AlwaysOnTop,
        exit_on_close_request: false,
        ..Default::default()
    };
    #[cfg(windows)]
    {
        window.icon = iced::window::icon::from_rgba(tray_icon_rgba(), 32, 32).ok();
    }
    window
}

struct Launcher {
    page: Page,
    settings_section: SettingsSection,
    remote_id: String,
    password: String,
    auth_remote_id: Option<String>,
    remember_password: bool,
    auth_status: String,
    contact_name: String,
    contact_group: String,
    contact_tags: String,
    contact_note: String,
    editing_contact_id: Option<String>,
    selected_contact_id: Option<String>,
    contact_form_expanded: bool,
    device_filter: String,
    address_book_filter: AddressBookFilter,
    address_book_account: String,
    address_book_password: String,
    address_book_access_token: String,
    address_book_signed_in: bool,
    address_book_busy: bool,
    address_book_status: String,
    account_entitlements_status: String,
    account_entitlements: AccountEntitlements,
    login_options: Vec<String>,
    login_options_busy: bool,
    oidc_code: Option<String>,
    oidc_last_poll: Option<Instant>,
    oidc_deadline: Option<Instant>,
    oidc_poll_busy: bool,
    server_discovery_busy: bool,
    server_discovery_status: String,
    smart_agent_started_at: Instant,
    smart_agent_last_heartbeat: Option<Instant>,
    smart_agent_last_inbox: Option<Instant>,
    smart_agent_heartbeat_busy: bool,
    smart_agent_inbox_busy: bool,
    smart_agent_heartbeat_failures: u8,
    smart_agent_inbox_failures: u32,
    smart_agent_burst_until: Option<Instant>,
    smart_agent_status: String,
    smart_agent_notifications: VecDeque<AgentNotification>,
    smart_agent_operators: Vec<AgentOperator>,
    smart_agent_operators_busy: bool,
    support_target_machine_id: Option<String>,
    support_request_message: String,
    support_request_busy: bool,
    support_request_status: String,
    vm_bridge_status: String,
    vm_bridge_busy: bool,
    vm_inventory: Vec<VmInventoryEntry>,
    vm_inventory_filter: String,
    game_remote_id: String,
    game_password: String,
    game_remember_password: bool,
    game_connect_status: String,
    pending_connect_profile: ConnectProfile,
    status: String,
    viewers: BTreeMap<u32, ViewerEntry>,
    store: LauncherStore,
    config: AppConfig,
    host: Option<HostRuntime>,
    host_state: HostState,
    pending_approval: Option<PendingApproval>,
    incoming_accepting: Option<AcceptedIncoming>,
    incoming_session: Option<IncomingSession>,
    approval_token: u64,
    clipboard_token: u64,
    viewer_token: u64,
    password_visible: bool,
    permanent_password: String,
    permanent_password_visible: bool,
    permanent_password_status: String,
    last_temp_password_rotation: Instant,
    window_id: Option<iced::window::Id>,
    background_window_id: Option<iced::window::Id>,
    background_hide_attempts: u8,
    incoming_window_id: Option<iced::window::Id>,
    auth_window_id: Option<iced::window::Id>,
    about_open: bool,
    main_window_size: Size,
    /// One-shot guard for `exclude_main_window_from_capture` — the main
    /// window falls inside its own DXGI capture region while hosting is
    /// active, so without this, the launcher's own repaints look like
    /// "screen changed" to the change-detector and force continuous
    /// re-encode (100% CPU/GPU while the window is visible, normal while
    /// minimized). See `windows_app::exclude_window_from_capture`.
    capture_exclusion_applied: bool,
    /// OS-service (Phase 3/4, TZ_HOST_SERVICE.md) install/run state, cached
    /// and re-queried at most every few seconds — see `tick_service_hint`.
    service_hint_state: ServiceHintState,
    service_hint_next_check: Instant,
    update_state: UpdateState,
    update_next_check: Instant,
    #[cfg(windows)]
    tray: Option<TrayController>,
}

/// Auto-update flow: check -> (optionally) download+verify -> hand off to
/// the OS installer/dmg. Never applies silently — see `updater` module docs.
#[derive(Debug, Clone, Default)]
enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(updater::UpdateManifest),
    Downloading(updater::UpdateManifest),
    ReadyToInstall(PathBuf),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UpdateSource {
    ManifestUrl(String),
    GithubRelease(String),
}

/// Manifest URL for update checks. Empty by default (checks are a no-op)
/// until real hosting exists; override with `EVERTYDESK_UPDATE_URL` so this
/// doesn't need a rebuild once you have somewhere to host `latest.json`.
fn update_manifest_url() -> Option<String> {
    match env::var("EVERTYDESK_UPDATE_URL") {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

fn update_source_from_store(store: &LauncherStore) -> Option<UpdateSource> {
    match store.update_channel {
        UpdateChannelPreference::Disabled => update_manifest_url().map(UpdateSource::ManifestUrl),
        UpdateChannelPreference::ManifestUrl => {
            let url = store.update_manifest_url.trim();
            if url.is_empty() {
                None
            } else {
                Some(UpdateSource::ManifestUrl(url.to_owned()))
            }
        }
        UpdateChannelPreference::GithubRelease => {
            let repo = store.update_github_repo.trim();
            if repo.is_empty() {
                None
            } else {
                Some(UpdateSource::GithubRelease(repo.to_owned()))
            }
        }
    }
}

const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

fn update_download_dir() -> PathBuf {
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local_app_data)
            .join("EvertyDesk")
            .join("Updates");
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".cache")
            .join("evertydesk")
            .join("updates");
    }
    env::temp_dir().join("evertydesk-updates")
}

#[cfg(windows)]
fn set_launch_on_startup(enabled: bool) -> Result<(), String> {
    let run_key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let value_name = "EvertyDesk Next";
    if enabled {
        let exe = env::current_exe()
            .map_err(|error| format!("cannot resolve current executable: {error}"))?;
        let command = windows_run_command_value(&exe.to_string_lossy());
        let status = Command::new("reg")
            .args(["add", run_key, "/v", value_name, "/t", "REG_SZ", "/d"])
            .arg(command)
            .args(["/f"])
            .status()
            .map_err(|error| format!("cannot run reg.exe: {error}"))?;
        if !status.success() {
            return Err(format!("reg add failed with status {status}"));
        }
    } else {
        let query_status = Command::new("reg")
            .args(["query", run_key, "/v", value_name])
            .status()
            .map_err(|error| format!("cannot run reg.exe: {error}"))?;
        if !query_status.success() {
            return Ok(());
        }
        let status = Command::new("reg")
            .args(["delete", run_key, "/v", value_name, "/f"])
            .status()
            .map_err(|error| format!("cannot run reg.exe: {error}"))?;
        if !status.success() {
            return Err(format!("reg delete failed with status {status}"));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_launch_on_startup_enabled() -> Result<bool, String> {
    let run_key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let value_name = "EvertyDesk Next";
    let output = Command::new("reg")
        .args(["query", run_key, "/v", value_name])
        .output()
        .map_err(|error| format!("cannot run reg.exe: {error}"))?;
    Ok(output.status.success())
}

#[cfg(not(windows))]
fn is_launch_on_startup_enabled() -> Result<bool, String> {
    Ok(false)
}

#[cfg(not(windows))]
fn set_launch_on_startup(_enabled: bool) -> Result<(), String> {
    Err("system autostart is implemented for Windows only".to_owned())
}

#[cfg(windows)]
fn windows_run_command_value(executable: &str) -> String {
    format!("\"{}\" --background", executable.replace('"', "\\\""))
}

#[cfg(windows)]
fn set_start_menu_shortcut(enabled: bool) -> Result<(), String> {
    let shortcut_path = start_menu_shortcut_path()?;
    if enabled {
        let exe = env::current_exe()
            .map_err(|error| format!("cannot resolve current executable: {error}"))?;
        let shortcut_dir = shortcut_path
            .parent()
            .ok_or_else(|| "cannot resolve Start Menu shortcut directory".to_owned())?;
        fs::create_dir_all(shortcut_dir)
            .map_err(|error| format!("cannot create Start Menu directory: {error}"))?;
        let working_dir = exe
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let icon_location = format!("{},0", exe.display());
        let status = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                "$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($args[0]); $shortcut.TargetPath = $args[1]; $shortcut.WorkingDirectory = $args[2]; $shortcut.IconLocation = $args[3]; $shortcut.Save()",
            ])
            .arg(&shortcut_path)
            .arg(&exe)
            .arg(&working_dir)
            .arg(icon_location)
            .status()
            .map_err(|error| format!("cannot run PowerShell shortcut creator: {error}"))?;
        if !status.success() {
            return Err(format!(
                "PowerShell shortcut creator failed with status {status}"
            ));
        }
    } else if shortcut_path.exists() {
        fs::remove_file(&shortcut_path)
            .map_err(|error| format!("cannot remove Start Menu shortcut: {error}"))?;
    }
    Ok(())
}

#[cfg(windows)]
fn is_start_menu_shortcut_enabled() -> Result<bool, String> {
    Ok(start_menu_shortcut_path()?.exists())
}

#[cfg(not(windows))]
fn is_start_menu_shortcut_enabled() -> Result<bool, String> {
    Ok(false)
}

#[cfg(not(windows))]
fn set_start_menu_shortcut(_enabled: bool) -> Result<(), String> {
    Err("Start Menu shortcuts are implemented for Windows only".to_owned())
}

#[cfg(windows)]
fn start_menu_shortcut_path() -> Result<PathBuf, String> {
    let appdata = env::var_os("APPDATA")
        .ok_or_else(|| "APPDATA is not set; cannot resolve user Start Menu".to_owned())?;
    Ok(PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("EvertyDesk Next")
        .join("EvertyDesk.lnk"))
}

fn refresh_system_integration_state(store: &mut LauncherStore) {
    if let Ok(enabled) = is_launch_on_startup_enabled() {
        store.launch_on_startup = enabled;
    }
    if let Ok(enabled) = is_start_menu_shortcut_enabled() {
        store.show_start_menu_shortcut = enabled;
    }
}

/// Mirrors `HostState` but for the *service*, not the current session: is it
/// installed at all, and if so, running. Distinct from hosting being
/// started/stopped inside this process — the service is what keeps hosting
/// alive when this process isn't running at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceHintState {
    NotInstalled,
    InstalledNotRunning,
    Running,
    /// Install/start was just requested — waiting for the elevated relaunch
    /// (Windows) or the systemctl/launchctl call (Linux/macOS) to land.
    Installing,
}

fn query_service_hint_state() -> ServiceHintState {
    #[cfg(windows)]
    {
        if evertydesk_core::winservice::is_service_running() {
            ServiceHintState::Running
        } else if evertydesk_core::winservice::is_service_installed() {
            ServiceHintState::InstalledNotRunning
        } else {
            ServiceHintState::NotInstalled
        }
    }
    #[cfg(unix)]
    {
        if evertydesk_core::host_service_unix::is_service_running() {
            ServiceHintState::Running
        } else if evertydesk_core::host_service_unix::is_service_installed() {
            ServiceHintState::InstalledNotRunning
        } else {
            ServiceHintState::NotInstalled
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        ServiceHintState::NotInstalled
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AccountEntitlements {
    known: bool,
    smart_agent: bool,
    yandex_sso: bool,
    ldap: bool,
    client_builder: bool,
    invoice_billing: bool,
    vm: bool,
    priority_support: bool,
    audit: bool,
    branding: bool,
    vm_slots: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Page {
    #[default]
    Home,
    Devices,
    Vm,
    Game,
    Settings,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SettingsSection {
    #[default]
    Security,
    General,
    Connection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AddressBookFilter {
    All,
    Favorites,
    Recent,
    Group(String),
    Tag(String),
}

impl SettingsSection {
    const ALL: [Self; 3] = [Self::Security, Self::General, Self::Connection];

    fn icon(self) -> icondata::Icon {
        match self {
            Self::Security => icondata::LuShieldCheck,
            Self::General => icondata::LuSlidersHorizontal,
            Self::Connection => icondata::LuNetwork,
        }
    }
}

fn settings_section_label(section: SettingsSection, language: UiLanguage) -> &'static str {
    match section {
        SettingsSection::Security => tr(language, TextKey::SettingsSectionSecurity),
        SettingsSection::General => tr(language, TextKey::SettingsSectionGeneral),
        SettingsSection::Connection => tr(language, TextKey::SettingsSectionConnection),
    }
}

fn settings_section_hint(section: SettingsSection, language: UiLanguage) -> &'static str {
    match section {
        SettingsSection::Security => tr(language, TextKey::SettingsHintSecurity),
        SettingsSection::General => tr(language, TextKey::SettingsHintGeneral),
        SettingsSection::Connection => tr(language, TextKey::SettingsHintConnection),
    }
}

fn language_preference_label(preference: LanguagePreference, language: UiLanguage) -> &'static str {
    match preference {
        LanguagePreference::System => tr(language, TextKey::LanguageSystem),
        LanguagePreference::Russian => tr(language, TextKey::LanguageRussian),
        LanguagePreference::English => tr(language, TextKey::LanguageEnglish),
    }
}

fn language_preference_hint(preference: LanguagePreference, language: UiLanguage) -> &'static str {
    match preference {
        LanguagePreference::System => tr(language, TextKey::LanguageSystemHint),
        LanguagePreference::Russian => tr(language, TextKey::LanguageRussianHint),
        LanguagePreference::English => tr(language, TextKey::LanguageEnglishHint),
    }
}

fn update_channel_label(channel: UpdateChannelPreference, language: UiLanguage) -> &'static str {
    match channel {
        UpdateChannelPreference::Disabled => tr(language, TextKey::UpdateChannelDisabled),
        UpdateChannelPreference::ManifestUrl => tr(language, TextKey::UpdateChannelManifestUrl),
        UpdateChannelPreference::GithubRelease => tr(language, TextKey::UpdateChannelGithubRelease),
    }
}

fn update_channel_hint(channel: UpdateChannelPreference, language: UiLanguage) -> &'static str {
    match channel {
        UpdateChannelPreference::Disabled => tr(language, TextKey::UpdateChannelDisabledHint),
        UpdateChannelPreference::ManifestUrl => tr(language, TextKey::UpdateChannelManifestUrlHint),
        UpdateChannelPreference::GithubRelease => {
            tr(language, TextKey::UpdateChannelGithubReleaseHint)
        }
    }
}

struct HostRuntime {
    commands: mpsc::Sender<HostCommand>,
}

struct PendingApproval {
    peer_id: String,
    peer_name: String,
    platform: String,
    version: String,
    token: u64,
    expires_at: Instant,
    allow_input: bool,
    allow_clipboard: bool,
}

#[derive(Clone)]
struct AcceptedIncoming {
    peer_id: String,
    allow_input: bool,
    allow_clipboard: bool,
}

struct IncomingSession {
    session_id: u64,
    peer_id: String,
    peer_name: String,
    platform: String,
    version: String,
    input_blocked: bool,
    clipboard_allowed: bool,
    pending_input_blocked: Option<bool>,
    pending_clipboard_allowed: Option<bool>,
    started_at: Instant,
    telemetry: Option<HostVideoTelemetry>,
    fallback_reason: Option<String>,
    disconnect_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HostVideoTelemetry {
    backend: String,
    fps: u32,
    bitrate_kbps: u64,
    sent_frames: u64,
    skipped_frames: u64,
    encode_avg_ms: u64,
}

impl IncomingSession {
    fn input_action_label(&self) -> &'static str {
        if self.input_blocked {
            "Разрешить управление"
        } else {
            "Заблокировать управление"
        }
    }

    fn clipboard_action_label(&self) -> &'static str {
        if self.clipboard_allowed {
            "Запретить буфер обмена"
        } else {
            "Разрешить буфер обмена"
        }
    }
}

#[cfg(windows)]
struct TrayController {
    icon: TrayIcon,
    host_item: MenuItem,
    status_item: MenuItem,
}

struct ViewerEntry {
    remote_id: String,
    status: String,
    codec: String,
    latency_ms: Option<u32>,
    fps_times_100: u32,
    input_kbps: u64,
    dropped_frames: u64,
    session_seconds: u64,
    reconnect_count: u32,
    last_telemetry_at: Option<Instant>,
    diagnostics_expanded: bool,
    input_enabled: bool,
    audio_enabled: bool,
    game_mode: bool,
    game_codec: ViewerGameCodec,
    game_evrt2_enabled: bool,
    clipboard_enabled: bool,
    scaling: ViewerScaling,
    session_token: u64,
    ipc_ready: bool,
    heartbeat_sequence: u64,
    disconnect_requested: bool,
    closed_status_received: bool,
    pending_controls: PendingViewerControls,
    diagnostics: VecDeque<String>,
    process: ViewerProcess,
}

#[derive(Default)]
struct PendingViewerControls {
    input: Option<bool>,
    audio: Option<bool>,
    clipboard: Option<bool>,
    quality: Option<ConnectionQuality>,
    scaling: Option<ViewerScaling>,
}

impl PendingViewerControls {
    fn contains(&self, control: ViewerControl) -> bool {
        match control {
            ViewerControl::InputEnabled { enabled } => self.input == Some(enabled),
            ViewerControl::AudioEnabled { enabled } => self.audio == Some(enabled),
            ViewerControl::ClipboardEnabled { enabled } => self.clipboard == Some(enabled),
            ViewerControl::Quality { quality } => self.quality == Some(quality),
            ViewerControl::Scaling { scaling } => self.scaling == Some(scaling),
        }
    }

    fn has_kind(&self, control: ViewerControl) -> bool {
        match control {
            ViewerControl::InputEnabled { .. } => self.input.is_some(),
            ViewerControl::AudioEnabled { .. } => self.audio.is_some(),
            ViewerControl::ClipboardEnabled { .. } => self.clipboard.is_some(),
            ViewerControl::Quality { .. } => self.quality.is_some(),
            ViewerControl::Scaling { .. } => self.scaling.is_some(),
        }
    }

    fn insert(&mut self, control: ViewerControl) {
        match control {
            ViewerControl::InputEnabled { enabled } => self.input = Some(enabled),
            ViewerControl::AudioEnabled { enabled } => self.audio = Some(enabled),
            ViewerControl::ClipboardEnabled { enabled } => self.clipboard = Some(enabled),
            ViewerControl::Quality { quality } => self.quality = Some(quality),
            ViewerControl::Scaling { scaling } => self.scaling = Some(scaling),
        }
    }

    fn remove(&mut self, control: ViewerControl) -> bool {
        if !self.contains(control) {
            return false;
        }
        match control {
            ViewerControl::InputEnabled { .. } => self.input = None,
            ViewerControl::AudioEnabled { .. } => self.audio = None,
            ViewerControl::ClipboardEnabled { .. } => self.clipboard = None,
            ViewerControl::Quality { .. } => self.quality = None,
            ViewerControl::Scaling { .. } => self.scaling = None,
        }
        true
    }

    fn remove_kind(&mut self, control: ViewerControl) {
        match control {
            ViewerControl::InputEnabled { .. } => self.input = None,
            ViewerControl::AudioEnabled { .. } => self.audio = None,
            ViewerControl::ClipboardEnabled { .. } => self.clipboard = None,
            ViewerControl::Quality { .. } => self.quality = None,
            ViewerControl::Scaling { .. } => self.scaling = None,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
enum Message {
    Navigate(Page),
    SelectSettingsSection(SettingsSection),
    RemoteIdChanged(String),
    PasswordChanged(String),
    ToggleRememberPassword(bool),
    SubmitCredentials,
    CancelCredentials,
    ContactNameChanged(String),
    ContactGroupChanged(String),
    ContactTagsChanged(String),
    ContactNoteChanged(String),
    DeviceFilterChanged(String),
    SelectAddressBookFilter(AddressBookFilter),
    ClearAddressBookFilter,
    UseContactGroup(String),
    AddContactTag(String),
    SelectContact(String),
    CopyContactId(String),
    AddressBookAccountChanged(String),
    AddressBookPasswordChanged(String),
    SignInAddressBook,
    RefreshLoginOptions,
    LoginOptionsLoaded(Result<Vec<String>, String>),
    StartYandexOidc,
    CancelYandexOidc,
    YandexOidcStarted(Result<evertydesk_core::address_book::OidcAuthStart, String>),
    YandexOidcPolled(Result<evertydesk_core::address_book::OidcAuthQuery, String>),
    SyncAddressBook,
    SignOutAddressBook,
    SetQuality(ConnectionQuality),
    SetRequireConfirmation(bool),
    SetAllowKeyboardMouse(bool),
    SetAllowClipboard(bool),
    SetViewerAudioDefault(bool),
    SetLaunchOnStartup(bool),
    SetStartMenuShortcut(bool),
    SetKeepTaskbarIconOnClose(bool),
    SetLanguage(LanguagePreference),
    SetUpdateChannel(UpdateChannelPreference),
    UpdateManifestUrlChanged(String),
    UpdateGithubRepoChanged(String),
    SetStreamingMode(StreamingMode),
    SetFsrQuality(FsrQualitySetting),
    ToggleCompatibilitySettings,
    ServerApiUrlChanged(String),
    ServerIdChanged(String),
    ServerRelayChanged(String),
    ServerPublicKeyChanged(String),
    ResetServerSettings,
    DiscoverServerSettings,
    ServerDiscoveryFinished(Result<ServerConfig, String>),
    CurrentUserRefreshed(Result<(AccountEntitlements, String), String>),
    RefreshCurrentUser,
    ToggleVmSettings,
    SetVmBridgeEnabled(bool),
    SetVmProvider(VmProviderPreference),
    VmTargetChanged(String),
    SelectVmTarget(String),
    VmInventoryFilterChanged(String),
    RefreshVmBridge,
    AttachVmBridge,
    AttachVmBridgeTarget(String),
    ConnectVmRdp(String),
    DetachVmBridge,
    VmBridgeInventory(Result<Vec<VmInventoryEntry>, String>),
    VmBridgeResult(Result<String, String>),
    RunVmPowerAction {
        target: String,
        action: VmPowerAction,
    },
    VmPowerActionFinished(String),
    SetGameCodec(GameCodecPreference),
    SetGameEvrt2(bool),
    GameRemoteIdChanged(String),
    GamePasswordChanged(String),
    ToggleGameRememberPassword(bool),
    ConnectGame,
    SetSmartAgentEnabled(bool),
    SmartAgentServiceKeyChanged(String),
    AcknowledgeSmartNotification(u64),
    VoteSmartNotification(u64, String),
    CopySmartConfigField {
        label: String,
        value: String,
    },
    CopySmartNotificationLink {
        notification_id: u64,
        url: String,
    },
    RefreshSmartOperators,
    SelectSupportOperator(String),
    SupportRequestMessageChanged(String),
    RequestSmartSupport,
    RespondToSupport {
        notification_id: u64,
        request_id: u64,
        action: smart_agent::SupportAction,
        from_remote_id: String,
    },
    CopyLocalId,
    CopyLocalPassword,
    OpenMacPrivacySettings,
    TogglePasswordVisibility,
    RegeneratePassword,
    PermanentPasswordChanged(String),
    TogglePermanentPasswordVisibility,
    SavePermanentPassword,
    ClearPermanentPassword,
    StartHosting,
    StopHosting,
    InstallHostService,
    StartHostService,
    OpenAbout,
    CloseAbout,
    OpenAboutGithub,
    OpenAboutHabr,
    OpenAboutDesk,
    CopyAboutEmail,
    CheckForUpdates,
    DownloadUpdate,
    InstallUpdate,
    ApproveIncoming(bool),
    TogglePendingInput,
    TogglePendingClipboard,
    ToggleIncomingInput,
    ToggleIncomingClipboard,
    DisconnectIncoming,
    CopyIncomingPeer,
    UiTick,
    WindowOpened,
    BackgroundWindowOpened(iced::window::Id),
    BackgroundWindowHidden(bool),
    CaptureExclusionApplied(bool),
    WindowResized(iced::window::Id, Size),
    CloseRequested(iced::window::Id),
    Tray(TrayAction),
    SaveContact,
    EditContact(String),
    CancelContactEdit,
    ToggleContactForm,
    SelectRemote(String),
    ConnectRemote(String),
    ToggleFavorite(String),
    RemoveRecent(String),
    ClearRecent,
    RemoveContact(String),
    Connect,
    RefreshViewer(u32),
    ReconnectViewer(u32),
    ToggleViewerInput(u32),
    ToggleViewerAudio(u32),
    ToggleViewerClipboard(u32),
    ToggleViewerScaling(u32),
    NextViewerDisplay(u32),
    ToggleViewerDiagnostics(u32),
    CopyViewerDiagnostics(u32),
    Disconnect(u32),
    ToggleFullscreen(u32),
    ProcessEvent(ProcessEvent),
}

#[derive(Debug, Clone)]
enum ProcessEvent {
    Status {
        process_id: u32,
        status: ViewerStatus,
    },
    StreamClosed {
        process_id: u32,
    },
    Diagnostic {
        process_id: u32,
        message: String,
    },
    Host(HostEvent),
    Tray(TrayAction),
    ClipboardExpiry {
        token: u64,
        fingerprint: u64,
    },
    ApprovalExpired {
        peer_id: String,
        token: u64,
    },
    ViewerStartupExpired {
        process_id: u32,
        token: u64,
    },
    ViewerShutdownExpired {
        process_id: u32,
        token: u64,
    },
    ViewerControlExpired {
        process_id: u32,
        token: u64,
        control: ViewerControl,
    },
    ViewerLivenessExpired {
        process_id: u32,
        token: u64,
        heartbeat_sequence: u64,
    },
    AddressBook(AddressBookEvent),
    SmartAgent(SmartAgentEvent),
    Updater(UpdaterEvent),
    CurrentUserRefreshed(Result<(AccountEntitlements, String), String>),
    SecondInstance,
}

#[derive(Debug, Clone)]
enum UpdaterEvent {
    Checked(Result<Option<updater::UpdateManifest>, String>),
    Downloaded(Result<PathBuf, String>),
}

#[derive(Debug, Clone)]
enum SmartAgentEvent {
    Heartbeat(Result<(), String>),
    Inbox(Result<Vec<AgentNotification>, String>),
    Acknowledged {
        notification_id: u64,
        result: Result<(), String>,
    },
    Voted {
        notification_id: u64,
        result: Result<(), String>,
    },
    SupportResponded {
        notification_id: u64,
        action: smart_agent::SupportAction,
        from_remote_id: String,
        result: Result<(), String>,
    },
    OperatorsLoaded(Result<Vec<AgentOperator>, String>),
    SupportRequested(Result<u64, String>),
}

#[derive(Debug, Clone)]
enum AddressBookEvent {
    SignedIn {
        account: String,
        access_token: String,
        guid: String,
        contacts: Vec<ContactEntry>,
    },
    Synced {
        guid: String,
        contacts: Vec<ContactEntry>,
    },
    LoggedOut(Result<(), String>),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VmInventoryEntry {
    id: String,
    name: String,
    state: String,
    connectable: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ConnectProfile {
    #[default]
    Regular,
    Game,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VmPowerAction {
    Start,
    Stop,
    Restart,
}

impl VmPowerAction {
    fn label(self) -> &'static str {
        match self {
            Self::Start => "Старт",
            Self::Stop => "Стоп",
            Self::Restart => "Ребут",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayAction {
    Open,
    ToggleHost,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewerExitKind {
    Requested,
    Clean,
    Crashed,
    Lost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewerTimeoutKind {
    Startup,
    Shutdown,
}

impl Launcher {
    fn new() -> (Self, Task<Message>) {
        let start_in_background = launcher_start_in_background();
        let (main_window_id, open_main_window) = if start_in_background {
            let (id, open_window) = iced::window::open(background_init_window_settings());
            (Some(id), open_window.map(Message::BackgroundWindowOpened))
        } else {
            let (id, open_window) = iced::window::open(main_window_settings());
            (Some(id), open_window.map(|_| Message::WindowOpened))
        };
        let mut config = AppConfig::load_or_create();
        let (mut store, status) = match LauncherStore::load_default() {
            Ok(store) => (store, "Готов к подключению".to_owned()),
            Err(error) => (
                LauncherStore::default(),
                format!("Не удалось загрузить локальные данные: {error}"),
            ),
        };
        refresh_system_integration_state(&mut store);
        let address_book_account = store.address_book_account.clone();
        let (address_book_access_token, address_book_signed_in, token_error) =
            if address_book_account.is_empty() {
                (String::new(), false, None)
            } else {
                match credential_store::load_account_token(&address_book_account) {
                    Ok(Some(token)) if !token.trim().is_empty() => (token, true, None),
                    Ok(_) => (String::new(), false, None),
                    Err(error) => (String::new(), false, Some(error)),
                }
            };
        let (permanent_password, permanent_password_status) =
            match credential_store::load_permanent_password() {
                Ok(Some(password)) if !password.trim().is_empty() => (
                    password,
                    "Постоянный пароль загружен из системного хранилища".to_owned(),
                ),
                Ok(_) => (String::new(), "Постоянный пароль не задан".to_owned()),
                Err(error) => (
                    String::new(),
                    format!("Не удалось загрузить постоянный пароль: {error}"),
                ),
            };
        config.permanent_password = permanent_password.clone();
        let mut launcher = Self {
            page: Page::Home,
            settings_section: SettingsSection::Security,
            remote_id: String::new(),
            password: String::new(),
            auth_remote_id: None,
            remember_password: false,
            auth_status: String::new(),
            contact_name: String::new(),
            contact_group: String::new(),
            contact_tags: String::new(),
            contact_note: String::new(),
            editing_contact_id: None,
            selected_contact_id: None,
            contact_form_expanded: false,
            device_filter: String::new(),
            address_book_filter: AddressBookFilter::All,
            address_book_account,
            address_book_password: String::new(),
            address_book_access_token,
            address_book_signed_in,
            address_book_busy: false,
            address_book_status: token_error
                .map(|error| format!("Не удалось прочитать сохранённый вход: {error}"))
                .unwrap_or_default(),
            account_entitlements_status: String::new(),
            account_entitlements: AccountEntitlements::default(),
            login_options: Vec::new(),
            login_options_busy: false,
            oidc_code: None,
            oidc_last_poll: None,
            oidc_deadline: None,
            oidc_poll_busy: false,
            server_discovery_busy: false,
            server_discovery_status: String::new(),
            smart_agent_started_at: Instant::now(),
            smart_agent_last_heartbeat: None,
            smart_agent_last_inbox: None,
            smart_agent_heartbeat_busy: false,
            smart_agent_inbox_busy: false,
            smart_agent_heartbeat_failures: 0,
            smart_agent_inbox_failures: 0,
            smart_agent_burst_until: None,
            smart_agent_status: String::new(),
            smart_agent_notifications: VecDeque::new(),
            smart_agent_operators: Vec::new(),
            smart_agent_operators_busy: false,
            support_target_machine_id: None,
            support_request_message: String::new(),
            support_request_busy: false,
            support_request_status: String::new(),
            vm_bridge_status: vm_bridge::status(),
            vm_bridge_busy: false,
            vm_inventory: Vec::new(),
            vm_inventory_filter: String::new(),
            game_remote_id: String::new(),
            game_password: String::new(),
            game_remember_password: false,
            game_connect_status: String::new(),
            pending_connect_profile: ConnectProfile::Regular,
            status,
            viewers: BTreeMap::new(),
            store,
            config,
            host: None,
            host_state: HostState::Idle,
            pending_approval: None,
            incoming_accepting: None,
            incoming_session: None,
            approval_token: 0,
            clipboard_token: 0,
            viewer_token: 0,
            password_visible: false,
            permanent_password,
            permanent_password_visible: false,
            permanent_password_status,
            last_temp_password_rotation: Instant::now(),
            window_id: main_window_id,
            background_window_id: None,
            background_hide_attempts: 0,
            incoming_window_id: None,
            auth_window_id: None,
            about_open: false,
            main_window_size: Size::new(920.0, 720.0),
            capture_exclusion_applied: false,
            service_hint_state: query_service_hint_state(),
            service_hint_next_check: Instant::now() + Duration::from_secs(8),
            update_state: UpdateState::Idle,
            update_next_check: Instant::now() + Duration::from_secs(30),
            #[cfg(windows)]
            tray: None,
        };
        #[cfg(windows)]
        match TrayController::new(event_bus().0.clone()) {
            Ok(tray) => launcher.tray = Some(tray),
            Err(error) => launcher.status = format!("Системный трей недоступен: {error}"),
        }
        #[cfg(target_os = "macos")]
        {
            if launcher.store.start_host_on_launch {
                launcher.store.start_host_on_launch = false;
                let _ = launcher.store.save_default();
            }
            launcher.status = macos_startup_status();
        }
        #[cfg(not(target_os = "macos"))]
        {
            if launcher.store.start_host_on_launch {
                launcher.start_hosting();
            }
        }
        (launcher, open_main_window)
    }

    fn theme(&self) -> Theme {
        Theme::custom(
            "EvertyDesk Light",
            Palette {
                background: CANVAS,
                text: TEXT,
                primary: ACCENT,
                success: Color::from_rgb(0.12, 0.58, 0.35),
                warning: Color::from_rgb(0.91, 0.58, 0.10),
                danger: ACCENT,
            },
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            Subscription::run(process_event_stream),
            iced::window::close_requests().map(Message::CloseRequested),
            iced::window::resize_events().map(|(id, size)| Message::WindowResized(id, size)),
            iced::time::every(Duration::from_secs(1)).map(|_| Message::UiTick),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Navigate(page) => {
                self.page = page;
                if page == Page::Devices
                    && !self.address_book_signed_in
                    && self.login_options.is_empty()
                    && !self.login_options_busy
                {
                    return self.refresh_login_options();
                }
            }
            Message::SelectSettingsSection(section) => self.settings_section = section,
            Message::RemoteIdChanged(value) => self.remote_id = value,
            Message::PasswordChanged(value) => self.password = value,
            Message::ToggleRememberPassword(value) => self.remember_password = value,
            Message::SubmitCredentials => {
                self.pending_connect_profile = ConnectProfile::Regular;
                return self.submit_credentials();
            }
            Message::CancelCredentials => {
                self.cancel_credentials();
                return self.close_auth_window();
            }
            Message::ContactNameChanged(value) => self.contact_name = value,
            Message::ContactGroupChanged(value) => self.contact_group = value,
            Message::ContactTagsChanged(value) => self.contact_tags = value,
            Message::ContactNoteChanged(value) => self.contact_note = value,
            Message::DeviceFilterChanged(value) => {
                self.device_filter = value;
                self.refresh_selected_contact_visibility();
            }
            Message::SelectAddressBookFilter(filter) => {
                self.address_book_filter = filter;
                self.refresh_selected_contact_visibility();
            }
            Message::ClearAddressBookFilter => {
                self.address_book_filter = AddressBookFilter::All;
                self.refresh_selected_contact_visibility();
            }
            Message::UseContactGroup(group) => self.contact_group = group,
            Message::AddContactTag(tag) => self.add_contact_tag(&tag),
            Message::SelectContact(remote_id) => {
                self.selected_contact_id = (!remote_id.trim().is_empty()).then_some(remote_id);
            }
            Message::CopyContactId(remote_id) => {
                self.copy_to_clipboard(remote_id, "ID контакта скопирован", false);
            }
            Message::AddressBookAccountChanged(value) => {
                self.address_book_account = value;
                self.address_book_status.clear();
            }
            Message::AddressBookPasswordChanged(value) => {
                self.address_book_password = value;
                self.address_book_status.clear();
            }
            Message::SignInAddressBook => self.sign_in_address_book(),
            Message::RefreshLoginOptions => return self.refresh_login_options(),
            Message::LoginOptionsLoaded(result) => {
                self.login_options_busy = false;
                match result {
                    Ok(options) => {
                        self.login_options = options;
                        self.address_book_status =
                            if has_login_provider(&self.login_options, "yandex") {
                                "Доступен вход через Яндекс".to_owned()
                            } else {
                                "SSO-провайдеры на сервере не включены".to_owned()
                            };
                    }
                    Err(error) => {
                        self.address_book_status =
                            format!("Не удалось получить SSO: {}", bounded_text(&error, 180));
                    }
                }
            }
            Message::StartYandexOidc => return self.start_yandex_oidc(),
            Message::CancelYandexOidc => {
                self.clear_oidc_flow();
                self.address_book_busy = false;
                self.address_book_status = "Вход через Яндекс отменён".to_owned();
            }
            Message::YandexOidcStarted(result) => {
                self.address_book_busy = false;
                match result {
                    Ok(start) => {
                        self.oidc_code = Some(start.code);
                        self.oidc_last_poll = None;
                        self.oidc_deadline = Some(Instant::now() + Duration::from_secs(10 * 60));
                        self.address_book_status = match open_system_browser(&start.url) {
                            Ok(()) => "Открыл браузер для входа через Яндекс".to_owned(),
                            Err(error) => format!(
                                "Ссылка Яндекс готова, но браузер не открылся: {}",
                                bounded_text(&error, 160)
                            ),
                        };
                    }
                    Err(error) => {
                        self.clear_oidc_flow();
                        self.address_book_status = format!(
                            "Не удалось начать вход через Яндекс: {}",
                            bounded_text(&error, 180)
                        );
                    }
                }
            }
            Message::YandexOidcPolled(result) => {
                self.oidc_poll_busy = false;
                match result {
                    Ok(evertydesk_core::address_book::OidcAuthQuery::Pending) => {
                        self.address_book_status =
                            "Ожидаю подтверждение входа в браузере…".to_owned();
                    }
                    Ok(evertydesk_core::address_book::OidcAuthQuery::Authorized {
                        access_token,
                        account,
                    }) => {
                        self.clear_oidc_flow();
                        self.address_book_busy = true;
                        self.address_book_status =
                            "Яндекс подтвердил вход, загружаю адресную книгу…".to_owned();
                        if let Err(error) = spawn_address_book_token_load(
                            self.config.server.api_url.clone(),
                            account,
                            access_token,
                        ) {
                            self.address_book_busy = false;
                            self.address_book_status = error;
                        }
                    }
                    Err(error) => {
                        self.clear_oidc_flow();
                        self.address_book_status = format!(
                            "Вход через Яндекс не завершён: {}",
                            bounded_text(&error, 180)
                        );
                    }
                }
            }
            Message::SyncAddressBook => self.sync_address_book(),
            Message::SignOutAddressBook => self.sign_out_address_book(),
            Message::SetQuality(quality) => {
                if self
                    .viewers
                    .values()
                    .any(|entry| entry.pending_controls.quality.is_some())
                {
                    self.status = "Дождитесь подтверждения предыдущей смены профиля".to_owned();
                    return Task::none();
                }
                self.store.quality = quality;
                self.persist_store("Профиль качества сохранён");
                let mut failed = 0;
                for entry in self.viewers.values_mut() {
                    let control = ViewerControl::Quality { quality };
                    match entry.process.send(ViewerCommand::SetQuality { quality }) {
                        Ok(()) => {
                            entry.pending_controls.insert(control);
                            schedule_viewer_control_timeout(
                                entry.process.id(),
                                entry.session_token,
                                control,
                            );
                            entry.status =
                                format!("Ожидается подтверждение профиля «{}»", quality.label());
                        }
                        Err(error) => {
                            failed += 1;
                            entry.status = format!("Не удалось сменить профиль: {error}");
                        }
                    }
                }
                if failed > 0 {
                    self.status =
                        format!("Не удалось обновить профиль в {failed} активных сессиях");
                } else if !self.viewers.is_empty() {
                    self.status =
                        format!("Профиль «{}» отправлен активным сессиям", quality.label());
                }
            }
            Message::SetRequireConfirmation(value) => {
                self.config.security.require_confirmation = value;
                self.save_security_settings();
            }
            Message::SetAllowKeyboardMouse(value) => {
                self.config.security.allow_keyboard_mouse = value;
                self.save_security_settings();
            }
            Message::SetAllowClipboard(value) => {
                self.config.security.allow_clipboard = value;
                self.save_security_settings();
            }
            Message::SetViewerAudioDefault(value) => {
                self.store.audio_enabled = value;
                self.persist_store("Настройка звука удалённого компьютера сохранена");
            }
            Message::SetLaunchOnStartup(value) => {
                self.store.launch_on_startup = value;
                match set_launch_on_startup(value) {
                    Ok(()) => self.persist_store(if value {
                        "Автозапуск EvertyDesk включён"
                    } else {
                        "Автозапуск EvertyDesk выключен"
                    }),
                    Err(error) => {
                        self.store.launch_on_startup = !value;
                        self.status = format!("Не удалось изменить автозапуск: {error}");
                    }
                }
            }
            Message::SetStartMenuShortcut(value) => {
                self.store.show_start_menu_shortcut = value;
                match set_start_menu_shortcut(value) {
                    Ok(()) => self.persist_store(if value {
                        "Ярлык EvertyDesk добавлен в меню Пуск"
                    } else {
                        "Ярлык EvertyDesk удалён из меню Пуск"
                    }),
                    Err(error) => {
                        self.store.show_start_menu_shortcut = !value;
                        self.status = format!("Не удалось изменить ярлык в меню Пуск: {error}");
                    }
                }
            }
            Message::SetKeepTaskbarIconOnClose(value) => {
                self.store.keep_taskbar_icon_on_close = value;
                self.persist_store(if value {
                    "При закрытии окно будет сворачиваться на панель задач"
                } else {
                    "При закрытии окно будет скрываться в системный трей"
                });
            }
            Message::SetLanguage(language) => {
                self.store.language = language;
                self.persist_store(tr(self.ui_language(), TextKey::LanguageSaved));
            }
            Message::SetUpdateChannel(channel) => {
                self.store.update_channel = channel;
                self.update_state = UpdateState::Idle;
                self.persist_store(tr(self.ui_language(), TextKey::UpdateChannelSaved));
            }
            Message::UpdateManifestUrlChanged(value) => {
                self.store.update_manifest_url = value.trim().to_owned();
                self.update_state = UpdateState::Idle;
                self.persist_store(tr(self.ui_language(), TextKey::UpdateManifestUrlSaved));
            }
            Message::UpdateGithubRepoChanged(value) => {
                self.store.update_github_repo = value.trim().to_owned();
                self.update_state = UpdateState::Idle;
                self.persist_store(tr(self.ui_language(), TextKey::UpdateGithubRepoSaved));
            }
            Message::SetStreamingMode(mode) => {
                self.config.display.streaming_mode = mode;
                if mode == StreamingMode::Game {
                    self.config.display.target_fps = 60;
                    self.config.display.adaptive_quality = false;
                }
                self.save_runtime_settings("Режим трансляции сохранён");
            }
            Message::SetFsrQuality(quality) => {
                self.config.display.fsr_quality = quality;
                self.save_runtime_settings("Апскейл FSR сохранён");
            }
            Message::ToggleCompatibilitySettings => {
                self.store.compatibility_settings_expanded =
                    !self.store.compatibility_settings_expanded;
                self.persist_store("Настройки интерфейса сохранены");
            }
            Message::ServerApiUrlChanged(value) => {
                self.config.server.api_url =
                    server_field_or_default(value, ServerConfig::default().api_url);
                self.save_runtime_settings("API URL сохранён");
            }
            Message::ServerIdChanged(value) => {
                self.config.server.id_server =
                    server_field_or_default(value, ServerConfig::default().id_server);
                self.save_runtime_settings("ID server сохранён");
            }
            Message::ServerRelayChanged(value) => {
                self.config.server.relay_server =
                    server_field_or_default(value, ServerConfig::default().relay_server);
                self.save_runtime_settings("Relay server сохранён");
            }
            Message::ServerPublicKeyChanged(value) => {
                self.config.server.public_key =
                    server_field_or_default(value, ServerConfig::default().public_key);
                self.save_runtime_settings("Public key сохранён");
            }
            Message::ResetServerSettings => {
                self.config.server = ServerConfig::default();
                self.server_discovery_status.clear();
                self.save_runtime_settings("Серверы сброшены к настройкам EvertyDesk");
            }
            Message::DiscoverServerSettings => return self.discover_server_settings(),
            Message::ServerDiscoveryFinished(result) => {
                self.server_discovery_busy = false;
                match result {
                    Ok(discovered) => {
                        self.apply_discovered_server_settings(discovered);
                        self.server_discovery_status =
                            "Параметры подключения получены из API".to_owned();
                    }
                    Err(error) => {
                        self.server_discovery_status = format!(
                            "Не удалось получить параметры: {}",
                            bounded_text(&error, 180)
                        );
                    }
                }
            }
            Message::CurrentUserRefreshed(result) => match result {
                Ok((entitlements, summary)) => {
                    self.account_entitlements = entitlements;
                    self.account_entitlements_status = summary;
                }
                Err(error) => {
                    self.smart_agent_status = format!(
                        "Не удалось обновить права аккаунта: {}",
                        bounded_text(&error, 180)
                    );
                }
            },
            Message::RefreshCurrentUser => return self.refresh_current_user_entitlements(),
            Message::ToggleVmSettings => {
                self.store.vm_settings_expanded = !self.store.vm_settings_expanded;
                self.persist_store("Настройки интерфейса сохранены");
            }
            Message::SetVmBridgeEnabled(value) => {
                self.store.vm_bridge_enabled = value;
                self.persist_store("Настройка VM Bridge сохранена");
                if value {
                    self.vm_bridge_status = current_vm_status_text();
                    return Task::none();
                } else {
                    self.vm_bridge_busy = true;
                    return Task::perform(run_vm_detach(), Message::VmBridgeResult);
                }
            }
            Message::SetVmProvider(provider) => {
                self.store.vm_provider = provider;
                self.persist_store("Провайдер VM сохранён");
                return Task::none();
            }
            Message::VmTargetChanged(value) => {
                self.store.vm_target_id = sanitize_vm_target_id(&value);
                if let Some(provider) = infer_vm_provider(&self.store.vm_target_id) {
                    self.store.vm_provider = provider;
                }
                self.persist_store("VM ID сохранён");
                return Task::none();
            }
            Message::SelectVmTarget(target) => {
                self.store.vm_target_id = sanitize_vm_target_id(&target);
                if let Some(provider) = infer_vm_provider(&self.store.vm_target_id) {
                    self.store.vm_provider = provider;
                }
                self.vm_bridge_status = format!("Выбрана VM {}", self.store.vm_target_id);
                self.persist_store("VM выбрана");
                return Task::none();
            }
            Message::VmInventoryFilterChanged(value) => {
                self.vm_inventory_filter = sanitize_vm_filter(&value);
                return Task::none();
            }
            Message::RefreshVmBridge => {
                self.vm_bridge_busy = true;
                self.vm_bridge_status = "Обновляю список VM…".to_owned();
                return Task::perform(run_vm_inventory(), Message::VmBridgeInventory);
            }
            Message::AttachVmBridge => {
                let target = build_vm_target(self.store.vm_provider, &self.store.vm_target_id);
                if target.is_empty() {
                    self.vm_bridge_status = "Укажите VM ID перед подключением".to_owned();
                    return Task::none();
                } else {
                    self.vm_bridge_busy = true;
                    self.vm_bridge_status = format!("Подключаю VM {target}…");
                    return Task::perform(run_vm_attach(target), Message::VmBridgeResult);
                }
            }
            Message::AttachVmBridgeTarget(target) => {
                let target = sanitize_vm_target_id(&target);
                if target.is_empty() {
                    self.vm_bridge_status = "Укажите VM ID перед подключением".to_owned();
                    return Task::none();
                }
                self.store.vm_bridge_enabled = true;
                self.store.vm_target_id = target.clone();
                if let Some(provider) = infer_vm_provider(&target) {
                    self.store.vm_provider = provider;
                }
                self.persist_store("VM Bridge включён, VM выбрана");
                self.vm_bridge_busy = true;
                self.vm_bridge_status = format!("Подключаю VM {target}…");
                return Task::perform(run_vm_attach(target), Message::VmBridgeResult);
            }
            Message::ConnectVmRdp(target) => {
                let target = sanitize_vm_target_id(&target);
                let Some(vm_guid) = target.strip_prefix("hyperv:").map(str::to_owned) else {
                    self.vm_bridge_status =
                        "RDP-консоль пока доступна только для Hyper-V".to_owned();
                    return Task::none();
                };
                let bootstrap = RdpBootstrap {
                    target: RdpTarget::HyperV {
                        vm_guid: vm_guid.clone(),
                    },
                    username: String::new(),
                    password: String::new(),
                    domain: String::new(),
                };
                match spawn_rdp_viewer(&bootstrap) {
                    Ok(()) => {
                        self.vm_bridge_status = format!("RDP-консоль открыта для {vm_guid}");
                    }
                    Err(error) => {
                        self.vm_bridge_status = format!("Не удалось открыть RDP-консоль: {error}");
                    }
                }
            }
            Message::DetachVmBridge => {
                self.vm_bridge_busy = true;
                self.vm_bridge_status = "Отключаю VM Bridge…".to_owned();
                return Task::perform(run_vm_detach(), Message::VmBridgeResult);
            }
            Message::VmBridgeInventory(result) => {
                self.vm_bridge_busy = false;
                match result {
                    Ok(entries) => {
                        self.vm_bridge_status = format_vm_inventory_entries(&entries);
                        self.vm_inventory = entries;
                    }
                    Err(error) => {
                        self.vm_inventory.clear();
                        self.vm_bridge_status = format!("VM Bridge: {error}");
                    }
                }
                return Task::none();
            }
            Message::VmBridgeResult(result) => {
                self.vm_bridge_busy = false;
                match result {
                    Ok(status) => {
                        self.vm_bridge_status = status;
                    }
                    Err(error) => {
                        self.vm_bridge_status = format!("VM Bridge: {error}");
                    }
                }
                return Task::none();
            }
            Message::RunVmPowerAction { target, action } => {
                let target = sanitize_vm_target_id(&target);
                if target.is_empty() {
                    self.vm_bridge_status = "Выберите VM перед power action".to_owned();
                    return Task::none();
                }
                self.vm_bridge_busy = true;
                self.vm_bridge_status = format!("Выполняю {} для VM {target}…", action.label());
                return Task::perform(
                    run_vm_power_action(target, action),
                    Message::VmPowerActionFinished,
                );
            }
            Message::VmPowerActionFinished(status) => {
                self.vm_bridge_status = status;
                self.vm_bridge_busy = true;
                return Task::perform(run_vm_inventory(), Message::VmBridgeInventory);
            }
            Message::SetGameCodec(codec) => {
                self.store.game_codec = codec;
                self.persist_store("Game codec сохранён");
                return Task::none();
            }
            Message::SetGameEvrt2(value) => {
                self.store.game_evrt2_enabled = value;
                self.persist_store("EVRT2 для Game сохранён");
                return Task::none();
            }
            Message::GameRemoteIdChanged(value) => {
                self.game_remote_id = value;
                self.game_connect_status.clear();
            }
            Message::GamePasswordChanged(value) => {
                self.game_password = value;
                self.game_connect_status.clear();
            }
            Message::ToggleGameRememberPassword(value) => {
                self.game_remember_password = value;
            }
            Message::ConnectGame => {
                self.connect_game();
            }
            Message::SetSmartAgentEnabled(value) => {
                self.store.smart_agent_enabled = value;
                self.smart_agent_status = if value {
                    "Smart Agent включён; выполняется регистрация устройства".to_owned()
                } else {
                    "Smart Agent отключён".to_owned()
                };
                if value {
                    self.smart_agent_last_heartbeat = None;
                    self.smart_agent_last_inbox = None;
                    self.refresh_smart_operators();
                } else {
                    self.smart_agent_notifications.clear();
                    self.smart_agent_operators.clear();
                    self.support_target_machine_id = None;
                }
                self.persist_store("Настройка Smart Agent сохранена");
            }
            Message::SmartAgentServiceKeyChanged(value) => {
                self.store.smart_agent_service_key = value.trim().chars().take(96).collect();
                self.smart_agent_last_heartbeat = None;
                self.smart_agent_last_inbox = None;
                self.smart_agent_operators.clear();
                self.support_target_machine_id = None;
                self.support_request_status.clear();
                self.persist_store("Ключ организации Smart Agent сохранён");
            }
            Message::AcknowledgeSmartNotification(notification_id) => {
                self.acknowledge_smart_notification(notification_id);
            }
            Message::VoteSmartNotification(notification_id, vote) => {
                self.smart_agent_status = "Отправка ответа…".to_owned();
                spawn_smart_agent_vote(
                    self.config.ui.agent_machine_id.clone(),
                    notification_id,
                    vote,
                );
            }
            Message::CopySmartConfigField { label, value } => {
                self.copy_to_clipboard(value, &format!("{label} скопирован"), false);
            }
            Message::CopySmartNotificationLink {
                notification_id,
                url,
            } => {
                if is_safe_notification_link(&url) {
                    self.copy_to_clipboard(url, "Ссылка уведомления скопирована", false);
                } else {
                    self.smart_agent_status =
                        format!("Уведомление {notification_id}: ссылка отклонена");
                }
            }
            Message::RefreshSmartOperators => self.refresh_smart_operators(),
            Message::SelectSupportOperator(machine_id) => {
                self.support_target_machine_id = Some(machine_id);
                self.support_request_status.clear();
            }
            Message::SupportRequestMessageChanged(value) => {
                self.support_request_message = sanitize_support_message(&value);
                self.support_request_status.clear();
            }
            Message::RequestSmartSupport => self.request_smart_support(),
            Message::RespondToSupport {
                notification_id,
                request_id,
                action,
                from_remote_id,
            } => {
                self.smart_agent_status = "Отправка ответа на запрос поддержки…".to_owned();
                spawn_smart_agent_support_response(
                    self.config.ui.agent_machine_id.clone(),
                    self.store.smart_agent_service_key.clone(),
                    notification_id,
                    request_id,
                    action,
                    from_remote_id,
                );
            }
            Message::CopyLocalId => {
                self.copy_to_clipboard(self.config.local_id.clone(), "ID скопирован", false);
            }
            Message::CopyLocalPassword => {
                self.copy_to_clipboard(
                    self.config.local_password.clone(),
                    "Пароль скопирован на 30 секунд",
                    true,
                );
            }
            Message::OpenMacPrivacySettings => {
                self.status = open_macos_privacy_settings();
            }
            Message::TogglePasswordVisibility => {
                self.password_visible = !self.password_visible;
            }
            Message::RegeneratePassword => self.regenerate_password(),
            Message::PermanentPasswordChanged(value) => {
                self.permanent_password = sanitize_permanent_password(&value);
                self.permanent_password_status.clear();
            }
            Message::TogglePermanentPasswordVisibility => {
                self.permanent_password_visible = !self.permanent_password_visible;
            }
            Message::SavePermanentPassword => self.save_permanent_password(),
            Message::ClearPermanentPassword => self.clear_permanent_password(),
            Message::StartHosting => self.start_hosting(),
            Message::StopHosting => {
                self.stop_hosting();
                return self.close_incoming_window();
            }
            Message::InstallHostService => self.request_install_service(),
            Message::StartHostService => self.request_start_service(),
            Message::OpenAbout => {
                self.about_open = true;
            }
            Message::CloseAbout => {
                self.about_open = false;
            }
            Message::OpenAboutGithub => {
                self.status =
                    open_about_link("https://github.com/vaalimusic/EvertyDesk_Lite", "GitHub");
            }
            Message::OpenAboutHabr => {
                self.status = open_about_link("https://habr.com/ru/users/vaalimusic/", "Хабр");
            }
            Message::OpenAboutDesk => {
                self.status = open_about_link("https://desk.everty.ru", "desk.everty.ru");
            }
            Message::CopyAboutEmail => {
                self.copy_to_clipboard("info@everty.ru".to_owned(), "Email скопирован", false);
            }
            Message::CheckForUpdates => self.check_for_updates(),
            Message::DownloadUpdate => self.download_update(),
            Message::InstallUpdate => self.install_update(),
            Message::ApproveIncoming(accept) => {
                self.approve_incoming(accept);
                if !accept {
                    return self.close_incoming_window();
                }
            }
            Message::TogglePendingInput => {
                if let Some(pending) = self.pending_approval.as_mut() {
                    if self.config.security.allow_keyboard_mouse {
                        pending.allow_input = !pending.allow_input;
                    }
                }
            }
            Message::TogglePendingClipboard => {
                if let Some(pending) = self.pending_approval.as_mut() {
                    if self.config.security.allow_clipboard {
                        pending.allow_clipboard = !pending.allow_clipboard;
                    }
                }
            }
            Message::ToggleIncomingInput => self.toggle_incoming_input(),
            Message::ToggleIncomingClipboard => self.toggle_incoming_clipboard(),
            Message::DisconnectIncoming => self.disconnect_incoming(),
            Message::CopyIncomingPeer => self.copy_incoming_peer(),
            Message::UiTick => {
                self.tick_smart_agent();
                self.tick_service_hint();
                self.tick_update_check();
                self.rotate_temporary_password_if_due();
                if self.background_window_id.is_some() && self.background_hide_attempts < 5 {
                    return self.hide_background_window();
                }
                if let Some(task) = self.tick_oidc_login() {
                    return task;
                }
            }
            Message::WindowOpened => {
                if !self.capture_exclusion_applied {
                    if let Some(id) = self.window_id {
                        self.capture_exclusion_applied = true;
                        return iced::window::run(id, |handle| {
                            let hwnd = match handle.window_handle().map(|h| h.as_raw()) {
                                Ok(iced::window::raw_window_handle::RawWindowHandle::Win32(
                                    win32,
                                )) => Some(win32.hwnd.get()),
                                _ => None,
                            };
                            hwnd.is_some_and(
                                evertydesk_desktop_next::windows_app::exclude_window_from_capture,
                            )
                        })
                        .map(Message::CaptureExclusionApplied);
                    }
                }
            }
            Message::BackgroundWindowOpened(id) => {
                if self.window_id == Some(id) {
                    self.window_id = None;
                    self.capture_exclusion_applied = false;
                    self.background_window_id = Some(id);
                    self.background_hide_attempts = 0;
                    return self.hide_background_window();
                }
            }
            Message::BackgroundWindowHidden(_) => {}
            Message::CaptureExclusionApplied(ok) => {
                if !ok {
                    self.status =
                        "Не удалось исключить окно из захвата экрана (нужна Windows 10 2004+)"
                            .to_owned();
                }
            }
            Message::WindowResized(id, size) => {
                if self.window_id == Some(id) {
                    self.main_window_size = size;
                }
            }
            Message::CloseRequested(id) => {
                if self.auth_window_id == Some(id) {
                    self.cancel_credentials();
                    return self.close_auth_window();
                }
                if self.incoming_window_id == Some(id) {
                    if self.pending_approval.is_some() {
                        self.approve_incoming(false);
                        return self.close_incoming_window();
                    }
                    if self.incoming_accepting.is_some() || self.incoming_session.is_some() {
                        self.status =
                            "Входящая сессия продолжает работать в системном трее".to_owned();
                        return iced::window::minimize(id, true);
                    }
                    return self.close_incoming_window();
                }
                if self.window_id == Some(id) {
                    return self.close_main_window_to_background(id);
                }
            }
            Message::Tray(TrayAction::Open) => {
                self.status = if self.host.is_some() {
                    self.host_state.label().to_owned()
                } else {
                    "Готов к подключению".to_owned()
                };
                let mut tasks = vec![self.ensure_main_window()];
                if self.pending_approval.is_some()
                    || self.incoming_accepting.is_some()
                    || self.incoming_session.is_some()
                {
                    tasks.push(self.ensure_incoming_window());
                }
                if !tasks.is_empty() {
                    return Task::batch(tasks);
                }
            }
            Message::Tray(TrayAction::ToggleHost) => {
                if self.host.is_some() {
                    self.stop_hosting();
                } else {
                    self.start_hosting();
                }
            }
            Message::Tray(TrayAction::Quit) => {
                self.cancel_credentials();
                self.stop_hosting();
                self.begin_viewer_shutdown();
                return iced::exit();
            }
            Message::SaveContact => self.save_contact(),
            Message::EditContact(remote_id) => self.begin_contact_edit(&remote_id),
            Message::CancelContactEdit => self.clear_contact_form(),
            Message::ToggleContactForm => {
                self.contact_form_expanded = !self.contact_form_expanded;
                if !self.contact_form_expanded {
                    self.clear_contact_form();
                }
            }
            Message::SelectRemote(remote_id) => {
                self.remote_id = remote_id;
                self.selected_contact_id = self
                    .store
                    .contacts
                    .iter()
                    .find(|contact| remote_ids_match(&contact.remote_id, &self.remote_id))
                    .map(|contact| contact.remote_id.clone());
                self.status = "Адрес выбран — можно подключаться".to_owned();
            }
            Message::ConnectRemote(remote_id) => {
                self.remote_id = remote_id;
                self.pending_connect_profile = ConnectProfile::Regular;
                return self.begin_credentials();
            }
            Message::ToggleFavorite(remote_id) => {
                if let Some(favorite) = self.store.toggle_favorite(&remote_id) {
                    let status = if favorite {
                        "Устройство добавлено в избранное"
                    } else {
                        "Устройство удалено из избранного"
                    };
                    self.persist_store(status);
                }
            }
            Message::RemoveRecent(remote_id) => {
                if self.store.remove_recent(&remote_id) {
                    self.refresh_selected_contact_visibility();
                    self.persist_store("Запись удалена из истории");
                }
            }
            Message::ClearRecent => {
                if self.store.clear_recent() {
                    self.refresh_selected_contact_visibility();
                    self.persist_store("История подключений очищена");
                }
            }
            Message::RemoveContact(remote_id) => {
                if self.store.remove_contact(&remote_id) {
                    let remote_id = normalize_remote_id(&remote_id);
                    if self
                        .editing_contact_id
                        .as_deref()
                        .is_some_and(|id| remote_ids_match(id, &remote_id))
                    {
                        self.clear_contact_form();
                    }
                    if self
                        .selected_contact_id
                        .as_deref()
                        .is_some_and(|id| remote_ids_match(id, &remote_id))
                    {
                        self.selected_contact_id = None;
                    }
                    self.persist_store("Устройство удалено");
                }
            }
            Message::Connect => {
                self.pending_connect_profile = ConnectProfile::Regular;
                return self.begin_credentials();
            }
            Message::RefreshViewer(process_id) => {
                if let Some(entry) = self.viewers.get_mut(&process_id) {
                    match entry.process.send(ViewerCommand::RefreshVideo) {
                        Ok(()) => entry.status = "Запрошено обновление видео".to_owned(),
                        Err(error) => entry.status = format!("Ошибка команды: {error}"),
                    }
                }
            }
            Message::ReconnectViewer(process_id) => {
                if let Some(entry) = self.viewers.get_mut(&process_id) {
                    match entry.process.send(ViewerCommand::Reconnect) {
                        Ok(()) => entry.status = "Переподключение…".to_owned(),
                        Err(error) => entry.status = format!("Ошибка команды: {error}"),
                    }
                }
            }
            Message::ToggleViewerInput(process_id) => {
                if let Some(entry) = self.viewers.get_mut(&process_id) {
                    let enabled = !entry.input_enabled;
                    let control = ViewerControl::InputEnabled { enabled };
                    if entry.pending_controls.has_kind(control) {
                        self.status = "Viewer ещё применяет режим управления".to_owned();
                        return Task::none();
                    }
                    match entry
                        .process
                        .send(ViewerCommand::SetInputEnabled { enabled })
                    {
                        Ok(()) => {
                            entry.pending_controls.insert(control);
                            schedule_viewer_control_timeout(
                                process_id,
                                entry.session_token,
                                control,
                            );
                            entry.status = "Ожидается подтверждение режима управления…".to_owned();
                        }
                        Err(error) => entry.status = format!("Ошибка команды: {error}"),
                    }
                }
            }
            Message::ToggleViewerAudio(process_id) => {
                if let Some(entry) = self.viewers.get_mut(&process_id) {
                    let enabled = !entry.audio_enabled;
                    let control = ViewerControl::AudioEnabled { enabled };
                    if entry.pending_controls.has_kind(control) {
                        self.status = "Viewer ещё применяет настройку звука".to_owned();
                        return Task::none();
                    }
                    match entry
                        .process
                        .send(ViewerCommand::SetAudioEnabled { enabled })
                    {
                        Ok(()) => {
                            entry.pending_controls.insert(control);
                            schedule_viewer_control_timeout(
                                process_id,
                                entry.session_token,
                                control,
                            );
                            entry.status = "Ожидается подтверждение настройки звука…".to_owned();
                        }
                        Err(error) => entry.status = format!("Ошибка команды: {error}"),
                    }
                }
            }
            Message::ToggleViewerClipboard(process_id) => {
                if let Some(entry) = self.viewers.get_mut(&process_id) {
                    let enabled = !entry.clipboard_enabled;
                    let control = ViewerControl::ClipboardEnabled { enabled };
                    if entry.pending_controls.has_kind(control) {
                        self.status = "Viewer ещё применяет настройку clipboard".to_owned();
                        return Task::none();
                    }
                    match entry
                        .process
                        .send(ViewerCommand::SetClipboardEnabled { enabled })
                    {
                        Ok(()) => {
                            entry.pending_controls.insert(control);
                            schedule_viewer_control_timeout(
                                process_id,
                                entry.session_token,
                                control,
                            );
                            entry.status = "Ожидается подтверждение clipboard…".to_owned();
                        }
                        Err(error) => entry.status = format!("Ошибка команды: {error}"),
                    }
                }
            }
            Message::ToggleViewerScaling(process_id) => {
                let applied = if let Some(entry) = self.viewers.get_mut(&process_id) {
                    let scaling = entry.scaling.next();
                    let control = ViewerControl::Scaling { scaling };
                    if entry.pending_controls.has_kind(control) {
                        self.status = "Viewer ещё применяет масштабирование".to_owned();
                        return Task::none();
                    }
                    match entry.process.send(ViewerCommand::SetScaling { scaling }) {
                        Ok(()) => {
                            entry.pending_controls.insert(control);
                            schedule_viewer_control_timeout(
                                process_id,
                                entry.session_token,
                                control,
                            );
                            entry.status = "Ожидается подтверждение масштабирования…".to_owned();
                            Some(scaling)
                        }
                        Err(error) => {
                            entry.status = format!("Ошибка команды: {error}");
                            None
                        }
                    }
                } else {
                    None
                };
                if let Some(scaling) = applied {
                    self.store.scaling = scaling;
                    if let Err(error) = self.store.save_default() {
                        self.status =
                            format!("Масштаб применён, но настройка не сохранена: {error}");
                    }
                }
            }
            Message::NextViewerDisplay(process_id) => {
                if let Some(entry) = self.viewers.get_mut(&process_id) {
                    match entry
                        .process
                        .send(ViewerCommand::CycleDisplay { direction: 1 })
                    {
                        Ok(()) => entry.status = "Переключение монитора…".to_owned(),
                        Err(error) => entry.status = format!("Ошибка команды: {error}"),
                    }
                }
            }
            Message::ToggleViewerDiagnostics(process_id) => {
                if let Some(entry) = self.viewers.get_mut(&process_id) {
                    entry.diagnostics_expanded = !entry.diagnostics_expanded;
                }
            }
            Message::CopyViewerDiagnostics(process_id) => {
                if let Some(report) = self.viewers.get(&process_id).map(viewer_diagnostics_report) {
                    self.copy_to_clipboard(report, "Диагностика сессии скопирована", false);
                }
            }
            Message::Disconnect(process_id) => self.disconnect(process_id),
            Message::ToggleFullscreen(process_id) => {
                if let Some(entry) = self.viewers.get_mut(&process_id) {
                    if let Err(error) = entry.process.send(ViewerCommand::ToggleFullscreen) {
                        entry.status = format!("Ошибка команды: {error}");
                    }
                }
            }
            Message::ProcessEvent(ProcessEvent::Host(event)) => {
                return self.handle_host_event(event);
            }
            Message::ProcessEvent(ProcessEvent::Tray(action)) => {
                return self.update(Message::Tray(action));
            }
            Message::ProcessEvent(ProcessEvent::AddressBook(event)) => {
                self.handle_address_book_event(event);
            }
            Message::ProcessEvent(ProcessEvent::SmartAgent(event)) => {
                return self.handle_smart_agent_event(event);
            }
            Message::ProcessEvent(ProcessEvent::Updater(event)) => {
                self.handle_updater_event(event);
            }
            Message::ProcessEvent(ProcessEvent::CurrentUserRefreshed(result)) => {
                return self.update(Message::CurrentUserRefreshed(result));
            }
            Message::ProcessEvent(ProcessEvent::SecondInstance) => {
                self.status = "EvertyDesk уже был запущен — окно восстановлено".to_owned();
                return self.ensure_main_window();
            }
            Message::ProcessEvent(event) => {
                self.handle_process_event(event);
                if self.incoming_window_id.is_some()
                    && self.pending_approval.is_none()
                    && self.incoming_accepting.is_none()
                    && self.incoming_session.is_none()
                {
                    return self.close_incoming_window();
                }
            }
        }

        Task::none()
    }

    fn ensure_incoming_window(&mut self) -> Task<Message> {
        if let Some(id) = self.incoming_window_id {
            return Task::batch([
                iced::window::minimize(id, false),
                iced::window::gain_focus(id),
            ]);
        }
        let (id, open_window) = iced::window::open(incoming_window_settings());
        self.incoming_window_id = Some(id);
        open_window.map(|_| Message::WindowOpened)
    }

    fn close_incoming_window(&mut self) -> Task<Message> {
        self.incoming_window_id
            .take()
            .map_or_else(Task::none, iced::window::close)
    }

    fn ensure_auth_window(&mut self) -> Task<Message> {
        if let Some(id) = self.auth_window_id {
            return Task::batch([
                iced::window::minimize(id, false),
                iced::window::gain_focus(id),
            ]);
        }
        let (id, open_window) = iced::window::open(credential_window_settings());
        self.auth_window_id = Some(id);
        open_window.map(|_| Message::WindowOpened)
    }

    fn close_auth_window(&mut self) -> Task<Message> {
        self.auth_window_id
            .take()
            .map_or_else(Task::none, iced::window::close)
    }

    fn hide_background_window(&mut self) -> Task<Message> {
        const MAX_BACKGROUND_HIDE_ATTEMPTS: u8 = 5;
        let Some(id) = self.background_window_id else {
            return Task::none();
        };
        if self.background_hide_attempts >= MAX_BACKGROUND_HIDE_ATTEMPTS {
            return Task::none();
        }
        self.background_hide_attempts = self.background_hide_attempts.saturating_add(1);
        let _ =
            evertydesk_desktop_next::windows_app::hide_current_process_background_event_windows();
        iced::window::run(id, |handle| {
            let hwnd = match handle.window_handle().map(|h| h.as_raw()) {
                Ok(iced::window::raw_window_handle::RawWindowHandle::Win32(win32)) => {
                    Some(win32.hwnd.get())
                }
                _ => None,
            };
            hwnd.is_some_and(evertydesk_desktop_next::windows_app::hide_window)
        })
        .map(Message::BackgroundWindowHidden)
    }

    fn ensure_main_window(&mut self) -> Task<Message> {
        if let Some(id) = self.window_id {
            Task::batch([
                iced::window::minimize(id, false),
                iced::window::gain_focus(id),
            ])
        } else {
            let (id, open_window) = iced::window::open(main_window_settings());
            self.window_id = Some(id);
            self.capture_exclusion_applied = false;
            open_window.map(|_| Message::WindowOpened)
        }
    }

    fn close_main_window_to_background(&mut self, id: iced::window::Id) -> Task<Message> {
        self.password.zeroize();
        self.password_visible = false;
        self.status = if self.store.keep_taskbar_icon_on_close {
            "EvertyDesk свёрнут. Доступ и трей продолжают работать.".to_owned()
        } else {
            "EvertyDesk скрыт в системный трей. Доступ продолжает работать.".to_owned()
        };
        if self.store.keep_taskbar_icon_on_close {
            iced::window::minimize(id, true)
        } else {
            self.window_id = None;
            self.capture_exclusion_applied = false;
            iced::window::close(id)
        }
    }

    fn window_title(&self, id: iced::window::Id) -> String {
        if self.auth_window_id == Some(id) {
            "EvertyDesk — авторизация".to_owned()
        } else if self.incoming_window_id == Some(id) {
            if self.incoming_session.is_some() {
                "EvertyDesk — активная входящая сессия".to_owned()
            } else {
                "EvertyDesk — входящее подключение".to_owned()
            }
        } else {
            "EvertyDesk".to_owned()
        }
    }

    fn view_window(&self, id: iced::window::Id) -> Element<'_, Message> {
        if self.auth_window_id == Some(id) {
            self.credential_window_view()
        } else if self.incoming_window_id == Some(id) {
            self.incoming_window_view()
        } else {
            self.view()
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let ui_language = self.ui_language();
        let navigation = row![
            nav_button(tr(ui_language, TextKey::NavHome), Page::Home, self.page),
            nav_button(
                tr(ui_language, TextKey::NavAddressBook),
                Page::Devices,
                self.page,
            ),
            nav_icon_button(icondata::LuMonitor, "VM", Page::Vm, self.page),
            nav_icon_button(icondata::LuMousePointer2, "Game", Page::Game, self.page),
            nav_button(
                tr(ui_language, TextKey::NavSettings),
                Page::Settings,
                self.page,
            ),
        ]
        .spacing(4);

        let header = container(
            row![
                button(brand_badge(40.0, 23))
                    .on_press(Message::OpenAbout)
                    .padding(0)
                    .style(quiet_button),
                column![
                    text("EvertyDesk").size(24),
                    text("REMOTE DESKTOP").size(10).color(MUTED),
                ]
                .spacing(0),
                navigation,
                Space::new().width(Fill),
                container(
                    row![
                        text("●").size(12).color(host_state_color(&self.host_state)),
                        text(self.host_state.label()).size(13).color(MUTED),
                    ]
                    .spacing(7)
                    .align_y(Alignment::Center),
                )
                .padding([7, 11])
                .style(header_status_style),
            ]
            .align_y(Alignment::Center)
            .spacing(12),
        )
        .width(Fill)
        .padding([15, 28])
        .style(header_style);

        let page_content: Element<'_, Message> = if self.about_open {
            self.about_card()
        } else {
            match self.page {
                Page::Home => column![
                    self.local_access_card(),
                    self.connection_status_bar(),
                    self.sessions_section(),
                    self.home_recent_section(),
                ]
                .spacing(22)
                .into(),
                Page::Devices => column![
                    page_title(
                        tr(ui_language, TextKey::AddressBookTitle),
                        tr(ui_language, TextKey::AddressBookSubtitle)
                    ),
                    self.devices_section(),
                ]
                .spacing(18)
                .into(),
                Page::Vm => column![
                page_title(
                    "Виртуальные машины",
                    "Agentless VM Bridge: Hyper-V и VirtualBox через старый EvertyDesk Lite core"
                ),
                self.vm_page_section(),
            ]
                .spacing(18)
                .into(),
                Page::Game => column![
                page_title(
                    "Game режим",
                    "Минимальная задержка для динамичного изображения и интерактивного управления"
                ),
                self.game_page_section(),
            ]
                .spacing(18)
                .into(),
                Page::Settings => column![
                    page_title(
                        tr(ui_language, TextKey::SettingsTitle),
                        tr(ui_language, TextKey::SettingsSubtitle)
                    ),
                    self.settings_card(),
                ]
                .spacing(18)
                .into(),
            }
        };

        let content = column![
            page_content,
            container(
                row![
                    text("EvertyDesk · защищённое соединение")
                        .size(12)
                        .color(MUTED),
                    Space::new().width(Fill),
                    text("Пароли подключений не сохраняются")
                        .size(12)
                        .color(MUTED),
                ]
                .width(Fill),
            )
            .padding([4, 2]),
        ]
        .spacing(22)
        .width(Fill)
        .max_width(main_content_max_width(self.main_window_size.width));

        column![
            header,
            self.quick_connect_bar(),
            container(
                scrollable(
                    container(content)
                        .center_x(Fill)
                        .width(Fill)
                        .height(Length::Shrink)
                        .padding([
                            MAIN_CONTENT_VERTICAL_PADDING,
                            main_content_side_padding(self.main_window_size.width)
                        ])
                )
                .height(Fill)
                .width(Fill)
            )
            .width(Fill)
            .height(Fill),
        ]
        .height(Fill)
        .into()
    }

    fn about_card(&self) -> Element<'_, Message> {
        let ui_language = self.ui_language();
        let version = env!("CARGO_PKG_VERSION");
        let info = column![
            about_info_row(
                tr(ui_language, TextKey::AboutAuthor),
                "Артур Валиев",
                icondata::LuUser
            ),
            about_info_row(
                tr(ui_language, TextKey::AboutVersion),
                version,
                icondata::LuInfo
            ),
            about_info_row(
                tr(ui_language, TextKey::AboutContact),
                "info@everty.ru",
                icondata::LuMail
            ),
        ]
        .spacing(8);

        let links = row![
            about_action_button(
                tr(ui_language, TextKey::AboutGithub),
                icondata::LuGithub,
                Message::OpenAboutGithub
            ),
            about_action_button(
                tr(ui_language, TextKey::AboutHabr),
                icondata::LuExternalLink,
                Message::OpenAboutHabr
            ),
            about_action_button(
                tr(ui_language, TextKey::AboutDesk),
                icondata::LuBookOpen,
                Message::OpenAboutDesk
            ),
            about_action_button(
                tr(ui_language, TextKey::AboutCopyEmail),
                icondata::LuCopy,
                Message::CopyAboutEmail
            ),
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        container(
            column![
                row![
                    brand_badge(58.0, 30),
                    column![
                        text(tr(ui_language, TextKey::AboutTitle)).size(25),
                        text(tr(ui_language, TextKey::AboutSubtitle))
                            .size(12)
                            .color(MUTED),
                    ]
                    .spacing(3)
                    .width(Fill),
                    button(lucide_icon(icondata::LuX, 18.0, MUTED))
                        .on_press(Message::CloseAbout)
                        .padding(8)
                        .style(quiet_button),
                ]
                .spacing(14)
                .align_y(Alignment::Center),
                container(info).padding(14).width(Fill).style(subtle_panel),
                container(
                    column![
                        row![
                            lucide_icon(icondata::LuBookOpen, 18.0, ACCENT),
                            text(tr(ui_language, TextKey::AboutDesk)).size(17),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                        text(tr(ui_language, TextKey::AboutDeskDescription))
                            .size(12)
                            .color(MUTED),
                    ]
                    .spacing(7),
                )
                .padding(14)
                .width(Fill)
                .style(subtle_panel),
                links,
                container(
                    column![
                        row![
                            text(tr(ui_language, TextKey::UpdatesTitle)).size(17),
                            Space::new().width(Fill),
                            button(label_with_icon(
                                tr(ui_language, TextKey::AboutCheckUpdates),
                                icondata::LuRefreshCw,
                                Color::WHITE
                            ))
                            .on_press(Message::CheckForUpdates)
                            .padding([8, 12])
                            .style(accent_button),
                        ]
                        .align_y(Alignment::Center),
                        self.update_status_panel(),
                    ]
                    .spacing(10),
                )
                .padding(14)
                .width(Fill)
                .style(subtle_panel),
                row![
                    Space::new().width(Fill),
                    button(tr(ui_language, TextKey::AboutClose))
                        .on_press(Message::CloseAbout)
                        .padding([9, 16])
                        .style(quiet_button),
                ],
            ]
            .spacing(16),
        )
        .padding(22)
        .width(Fill)
        .max_width(680)
        .style(card_style)
        .into()
    }

    fn incoming_window_view(&self) -> Element<'_, Message> {
        let badge = brand_badge(46.0, 24);

        let body: Element<'_, Message> = if let Some(pending) = &self.pending_approval {
            let mut identity_block = column![
                text("Запрос удалённого доступа").size(12).color(MUTED),
                text(format_local_id(&pending.peer_id))
                    .size(24)
                    .color(ACCENT),
            ]
            .spacing(2);
            if let Some(details) =
                peer_metadata(&pending.peer_name, &pending.platform, &pending.version)
            {
                identity_block = identity_block.push(text(details).size(11).color(MUTED));
            }
            let input_label = if !self.config.security.allow_keyboard_mouse {
                "Управление запрещено настройками"
            } else if pending.allow_input {
                "Клавиатура и мышь: разрешены"
            } else {
                "Клавиатура и мышь: запрещены"
            };
            let clipboard_label = if !self.config.security.allow_clipboard {
                "Буфер запрещён настройками"
            } else if pending.allow_clipboard {
                "Буфер обмена: разрешён"
            } else {
                "Буфер обмена: запрещён"
            };
            let input_permission_content = row![
                container(lucide_icon(icondata::LuMousePointer2, 17.0, ACCENT))
                    .width(Length::Fixed(24.0)),
                column![
                    text("Управление").size(11).color(MUTED),
                    text(input_label).size(13),
                ]
                .spacing(2),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            let input_permission = if self.config.security.allow_keyboard_mouse {
                button(input_permission_content).on_press(Message::TogglePendingInput)
            } else {
                button(input_permission_content)
            }
            .height(Length::Fixed(56.0))
            .width(Fill)
            .style(quiet_button);

            let clipboard_permission_content = row![
                container(lucide_icon(icondata::LuInbox, 17.0, ACCENT)).width(Length::Fixed(24.0)),
                column![
                    text("Буфер обмена").size(11).color(MUTED),
                    text(clipboard_label).size(13),
                ]
                .spacing(2),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            let clipboard_permission = if self.config.security.allow_clipboard {
                button(clipboard_permission_content).on_press(Message::TogglePendingClipboard)
            } else {
                button(clipboard_permission_content)
            }
            .height(Length::Fixed(56.0))
            .width(Fill)
            .style(quiet_button);

            column![
                row![
                    container(lucide_icon(icondata::LuBellRing, 22.0, ACCENT))
                        .width(Length::Fixed(34.0))
                        .height(Length::Fixed(34.0))
                        .center_x(Length::Fixed(34.0))
                        .center_y(Length::Fixed(34.0))
                        .style(subtle_panel),
                    column![text("Входящее подключение").size(24), identity_block,].spacing(5),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                container(
                    row![
                        lucide_icon(icondata::LuCheck, 18.0, ACCENT),
                        column![
                            text("Перед принятием убедитесь, что вы знаете отправителя.").size(13),
                            text(format!(
                                "Автоотклонение через {} сек.",
                                approval_seconds_remaining(pending)
                            ))
                            .size(12)
                            .color(MUTED),
                        ]
                        .spacing(4)
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center)
                )
                .padding(14)
                .width(Fill)
                .style(subtle_panel),
                text("РАЗРЕШЕНИЯ ЭТОЙ СЕССИИ").size(10).color(MUTED),
                row![input_permission, clipboard_permission,]
                    .spacing(12)
                    .align_y(Alignment::Center),
                row![
                    button(row![text("Отклонить").size(14),])
                        .on_press(Message::ApproveIncoming(false))
                        .height(Length::Fixed(44.0))
                        .width(Fill)
                        .style(quiet_button),
                    button(
                        row![
                            text("Принять").color(Color::WHITE).size(14),
                            lucide_icon(icondata::LuArrowRight, 16.0, Color::WHITE),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center)
                    )
                    .on_press(Message::ApproveIncoming(true))
                    .height(Length::Fixed(44.0))
                    .width(Fill)
                    .style(accent_button),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            ]
            .spacing(14)
            .into()
        } else if let Some(session) = &self.incoming_session {
            let input_state = if session.pending_input_blocked.is_some() {
                "Применяется новое разрешение управления…"
            } else if session.input_blocked {
                "Управление заблокировано"
            } else {
                "Клавиатура и мышь разрешены"
            };
            let clipboard_state = if session.pending_clipboard_allowed.is_some() {
                "Применяется разрешение буфера обмена…"
            } else if session.clipboard_allowed {
                "Буфер обмена разрешён"
            } else {
                "Буфер обмена запрещён"
            };
            let mut session_details = column![
                text(input_state).size(14),
                text(clipboard_state).size(12).color(MUTED),
                text(format!(
                    "Сессия активна {} сек.",
                    session.started_at.elapsed().as_secs()
                ))
                .size(12)
                .color(MUTED),
            ]
            .spacing(5);
            if let Some(details) =
                peer_metadata(&session.peer_name, &session.platform, &session.version)
            {
                session_details = session_details.push(text(details).size(11).color(MUTED));
            }
            if let Some(telemetry) = &session.telemetry {
                session_details = session_details
                    .push(
                        text(format!(
                            "{} · {} FPS · {}",
                            telemetry.backend,
                            telemetry.fps,
                            format_host_bitrate(telemetry.bitrate_kbps)
                        ))
                        .size(12),
                    )
                    .push(
                        text(format!(
                            "Кадры: {} отправлено, {} пропущено · кодирование {} мс",
                            telemetry.sent_frames,
                            telemetry.skipped_frames,
                            telemetry.encode_avg_ms
                        ))
                        .size(11)
                        .color(MUTED),
                    );
            } else {
                session_details =
                    session_details.push(text("Видео: ожидание телеметрии…").size(12).color(MUTED));
            }
            if let Some(reason) = &session.fallback_reason {
                session_details = session_details.push(
                    text(format!("Резервный режим: {reason}"))
                        .size(11)
                        .color(ACCENT),
                );
            }

            let input_button =
                if session.disconnect_requested || session.pending_input_blocked.is_some() {
                    button(if session.pending_input_blocked.is_some() {
                        "Применение…"
                    } else {
                        session.input_action_label()
                    })
                } else {
                    button(session.input_action_label()).on_press(Message::ToggleIncomingInput)
                }
                .height(Length::Fixed(44.0))
                .width(Length::Fixed(220.0))
                .style(quiet_button);
            let clipboard_button = if session.disconnect_requested
                || session.pending_clipboard_allowed.is_some()
            {
                button(if session.pending_clipboard_allowed.is_some() {
                    "Применение…"
                } else {
                    session.clipboard_action_label()
                })
            } else {
                button(session.clipboard_action_label()).on_press(Message::ToggleIncomingClipboard)
            }
            .height(Length::Fixed(44.0))
            .width(Length::Fixed(220.0))
            .style(quiet_button);
            let disconnect_button = if session.disconnect_requested {
                button("Отключение…").style(quiet_button)
            } else {
                button(text("Отключить клиента").color(Color::WHITE))
                    .on_press(Message::DisconnectIncoming)
                    .style(accent_button)
            }
            .height(Length::Fixed(44.0))
            .width(Length::Fixed(190.0));

            column![
                text("Активная входящая сессия").size(25),
                row![
                    text("●").size(13).color(Color::from_rgb(0.12, 0.72, 0.38)),
                    text(format!(
                        "Подключено устройство {}",
                        format_local_id(&session.peer_id)
                    ))
                    .size(14)
                    .width(Fill),
                    button("Копировать ID")
                        .on_press(Message::CopyIncomingPeer)
                        .height(Length::Fixed(32.0))
                        .style(quiet_button),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                container(session_details)
                    .padding(14)
                    .width(Fill)
                    .style(subtle_panel),
                row![input_button, Space::new().width(Fill), clipboard_button,]
                    .align_y(Alignment::Center),
                row![Space::new().width(Fill), disconnect_button].align_y(Alignment::Center),
            ]
            .spacing(13)
            .into()
        } else {
            column![
                text("Подключение разрешено").size(25),
                text(format!(
                    "Ожидаем начала сессии с устройством {}…",
                    self.incoming_accepting
                        .as_ref()
                        .map(|incoming| incoming.peer_id.as_str())
                        .unwrap_or("—")
                ))
                .size(14)
                .color(MUTED),
                text("Окно автоматически переключится в режим управления сессией.")
                    .size(12)
                    .color(MUTED),
            ]
            .spacing(17)
            .into()
        };

        container(
            column![
                row![
                    badge,
                    column![
                        text("EvertyDesk").size(20),
                        text("REMOTE ACCESS").size(10).color(MUTED),
                    ]
                    .spacing(0),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                body,
            ]
            .spacing(22),
        )
        .padding(26)
        .width(Fill)
        .height(Fill)
        .style(card_style)
        .into()
    }

    fn credential_window_view(&self) -> Element<'_, Message> {
        let ui_language = self.ui_language();
        let remote_id = self.auth_remote_id.as_deref().unwrap_or("—");
        let status: Element<'_, Message> = if self.auth_status.is_empty() {
            Space::new().height(0).into()
        } else {
            container(text(&self.auth_status).size(12).color(ACCENT))
                .padding([8, 10])
                .width(Fill)
                .style(status_bar)
                .into()
        };

        container(
            column![
                row![
                    brand_badge(58.0, 30),
                    column![
                        text(tr(ui_language, TextKey::HomeCredentialTitle)).size(24),
                        text(format!(
                            "{} {}",
                            tr(ui_language, TextKey::HomeCredentialSubtitlePrefix),
                            format_local_id(remote_id)
                        ))
                        .size(13)
                        .color(MUTED),
                    ]
                    .spacing(3),
                ]
                .spacing(15)
                .align_y(Alignment::Center),
                text_input(
                    tr(ui_language, TextKey::HomeRemotePasswordPlaceholder),
                    &self.password
                )
                .on_input(Message::PasswordChanged)
                .secure(true)
                .on_submit(Message::SubmitCredentials)
                .padding(13)
                .size(15)
                .style(input_style)
                .width(Fill),
                checkbox(self.remember_password)
                    .label(tr(ui_language, TextKey::HomeRememberPassword))
                    .on_toggle(Message::ToggleRememberPassword)
                    .size(17),
                text(tr(ui_language, TextKey::HomeRememberPasswordHint))
                    .size(11)
                    .color(MUTED),
                status,
                row![
                    button(tr(ui_language, TextKey::HomeCancel))
                        .on_press(Message::CancelCredentials)
                        .height(Length::Fixed(42.0))
                        .width(Length::Fixed(130.0))
                        .style(quiet_button),
                    Space::new().width(Fill),
                    button(text(tr(ui_language, TextKey::HomeConnect)).color(Color::WHITE))
                        .on_press(Message::SubmitCredentials)
                        .height(Length::Fixed(42.0))
                        .width(Length::Fixed(180.0))
                        .style(accent_button),
                ]
                .align_y(Alignment::Center),
            ]
            .spacing(16),
        )
        .padding(26)
        .width(Fill)
        .height(Fill)
        .style(card_style)
        .into()
    }

    fn local_access_card(&self) -> Element<'_, Message> {
        let ui_language = self.ui_language();
        let host_online = self.host.is_some();
        let host_action = if host_online {
            button(tr(ui_language, TextKey::HomeStopReceiving))
                .on_press(Message::StopHosting)
                .style(danger_text_button)
        } else {
            button(tr(ui_language, TextKey::HomeEnableAccess))
                .on_press(Message::StartHosting)
                .style(accent_button)
        }
        .padding([10, 16]);

        let password = if self.password_visible {
            self.config.local_password.clone()
        } else {
            "•".repeat(self.config.local_password.chars().count().max(6))
        };
        let visibility_label = if self.password_visible {
            tr(ui_language, TextKey::HomeHide)
        } else {
            tr(ui_language, TextKey::HomeShow)
        };

        let mut body = column![
            row![
                text(tr(ui_language, TextKey::HomeThisWorkspace)).size(18),
                text(format_local_id(&self.config.local_id))
                    .size(34)
                    .color(ACCENT),
                button(tr(ui_language, TextKey::HomeCopy))
                    .on_press(Message::CopyLocalId)
                    .padding([8, 12])
                    .style(quiet_button),
                host_action,
            ]
            .spacing(14)
            .align_y(Alignment::Center),
            container(
                row![
                    text(tr(ui_language, TextKey::HomeOneTimePassword))
                        .size(13)
                        .color(MUTED),
                    text(password).size(19).width(Length::Fixed(120.0)),
                    button(visibility_label)
                        .on_press(Message::TogglePasswordVisibility)
                        .padding([7, 10])
                        .style(quiet_button),
                    button(tr(ui_language, TextKey::HomeCopy))
                        .on_press(Message::CopyLocalPassword)
                        .padding([7, 10])
                        .style(quiet_button),
                    button(tr(ui_language, TextKey::HomeRefreshNow))
                        .on_press(Message::RegeneratePassword)
                        .padding([7, 10])
                        .style(quiet_button),
                    Space::new().width(Fill),
                    text("●").size(11).color(host_state_color(&self.host_state)),
                    text(self.host_state.label()).size(12).color(MUTED),
                ]
                .spacing(9)
                .align_y(Alignment::Center),
            )
            .padding([10, 14])
            .width(Fill)
            .style(subtle_panel),
        ]
        .spacing(16)
        .align_x(Alignment::Center);

        #[cfg(target_os = "macos")]
        {
            let (summary, color) = macos_permission_summary();
            body = body.push(
                container(
                    row![
                        text("macOS").size(12).color(color),
                        text(summary).size(12).color(MUTED).width(Fill),
                        button("Открыть доступы")
                            .on_press(Message::OpenMacPrivacySettings)
                            .padding([7, 10])
                            .style(quiet_button),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding([9, 12])
                .width(Fill)
                .style(subtle_panel),
            );
        }

        if let Some(pending) = &self.pending_approval {
            body = body.push(
                container(
                    row![
                        column![
                            text("Входящий запрос").size(15),
                            text(format!("Устройство {} запрашивает доступ", pending.peer_id))
                                .size(12)
                                .color(MUTED),
                        ]
                        .spacing(3)
                        .width(Fill),
                        button("Отклонить")
                            .on_press(Message::ApproveIncoming(false))
                            .padding([9, 14])
                            .style(danger_text_button),
                        button("Принять")
                            .on_press(Message::ApproveIncoming(true))
                            .padding([9, 14])
                            .style(accent_button),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding(12)
                .style(status_bar),
            );
        }

        if let Some(session) = &self.incoming_session {
            body = body.push(
                container(
                    row![
                        column![
                            text("Активная входящая сессия").size(15),
                            text(format!("Подключено устройство {}", session.peer_id))
                                .size(12)
                                .color(MUTED),
                        ]
                        .spacing(3)
                        .width(Fill),
                        button(session.input_action_label())
                            .on_press(Message::ToggleIncomingInput)
                            .padding([9, 14])
                            .style(quiet_button),
                        button("Отключить клиента")
                            .on_press(Message::DisconnectIncoming)
                            .padding([9, 14])
                            .style(danger_text_button),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding(12)
                .style(status_bar),
            );
        }

        container(body)
            .padding(24)
            .width(Fill)
            .style(card_style)
            .into()
    }

    fn quick_connect_bar(&self) -> Element<'_, Message> {
        let ui_language = self.ui_language();
        let can_connect = !self.remote_id.trim().is_empty();
        let connect_content = row![
            text(tr(ui_language, TextKey::HomeConnect)).size(14),
            lucide_icon(icondata::LuArrowRight, 16.0, Color::WHITE),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        let connect_button = if can_connect {
            button(connect_content)
                .on_press(Message::Connect)
                .style(accent_button)
        } else {
            button(connect_content).style(accent_button)
        }
        .padding([10, 18])
        .width(Length::Fixed(170.0));

        container(
            row![
                text("●").size(12).color(Color::from_rgb(0.18, 0.76, 0.43)),
                text_input(
                    tr(ui_language, TextKey::HomeRemoteAddressPlaceholder),
                    &self.remote_id
                )
                .on_input(Message::RemoteIdChanged)
                .on_submit(Message::Connect)
                .padding(10)
                .size(15)
                .style(quick_input_style)
                .width(Fill),
                connect_button,
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
        .padding([9, 28])
        .width(Fill)
        .style(quick_bar_style)
        .into()
    }

    fn connection_status_bar(&self) -> Element<'_, Message> {
        container(
            row![
                text("●").size(12).color(ACCENT),
                text(&self.status).size(13).color(MUTED),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([10, 14])
        .width(Fill)
        .style(status_bar)
        .into()
    }

    fn sessions_section(&self) -> Element<'_, Message> {
        if self.viewers.is_empty() {
            return Space::new().height(0).into();
        }

        let mut sessions = column![row![
            text("Сессии").size(18),
            Space::new().width(Fill),
            container(
                text(self.viewers.len().to_string())
                    .size(12)
                    .color(Color::WHITE)
            )
            .padding([3, 9])
            .style(accent_pill),
        ]
        .align_y(Alignment::Center)]
        .spacing(10);

        for (&process_id, entry) in &self.viewers {
            let game_profile_label = viewer_game_profile_label(
                entry.game_mode,
                entry.game_codec,
                entry.game_evrt2_enabled,
            );
            let diagnostics_label = if entry.diagnostics_expanded {
                "Скрыть диагностику"
            } else {
                "Диагностика"
            };
            let diagnostics_details: Element<'_, Message> = if entry.diagnostics_expanded {
                let age = entry.last_telemetry_at.map(|updated| updated.elapsed());
                let health = viewer_connection_health(entry.latency_ms, entry.fps_times_100, age);
                let codec = if entry.codec.is_empty() {
                    "ожидание"
                } else {
                    entry.codec.as_str()
                };
                let latency = entry
                    .latency_ms
                    .map_or_else(|| "—".to_owned(), |value| format!("{value} мс"));
                let mut log =
                    column![text("Последние сообщения viewer").size(11).color(MUTED)].spacing(4);
                if entry.diagnostics.is_empty() {
                    log = log.push(text("Сообщений нет").size(11).color(MUTED));
                } else {
                    for message in entry.diagnostics.iter().rev().take(3).rev() {
                        log = log.push(text(message).size(11).color(MUTED));
                    }
                }
                container(
                    column![
                        row![
                            text(format!("Состояние: {health}")).size(12),
                            text(format!("Кодек: {codec}")).size(12),
                            text(format!(
                                "FPS: {}.{:02}",
                                entry.fps_times_100 / 100,
                                entry.fps_times_100 % 100
                            ))
                            .size(12),
                            text(format!("Битрейт: {}", format_bandwidth(entry.input_kbps)))
                                .size(12),
                            text(format!("Задержка: {latency}")).size(12),
                        ]
                        .spacing(18),
                        row![
                            text(format!(
                                "Телеметрия: {}",
                                age.map_or_else(|| "—".to_owned(), format_telemetry_age)
                            ))
                            .size(11)
                            .color(MUTED),
                            text(format!("Пропущено кадров: {}", entry.dropped_frames))
                                .size(11)
                                .color(MUTED),
                            text(format!(
                                "Сессия: {} · восстановлений {}",
                                format_duration(entry.session_seconds),
                                entry.reconnect_count
                            ))
                            .size(11)
                            .color(MUTED),
                            Space::new().width(Fill),
                            button("Копировать отчёт")
                                .on_press(Message::CopyViewerDiagnostics(process_id))
                                .padding([6, 9])
                                .style(quiet_button),
                        ]
                        .spacing(14)
                        .align_y(Alignment::Center),
                        log,
                    ]
                    .spacing(9),
                )
                .padding(11)
                .width(Fill)
                .style(subtle_panel)
                .into()
            } else {
                Space::new().height(0).into()
            };
            sessions = sessions.push(
                container(
                    column![
                        row![
                            lucide_icon(
                                icondata::LuMonitor,
                                18.0,
                                Color::from_rgb(0.12, 0.66, 0.37)
                            ),
                            column![
                                text(&entry.remote_id).size(16),
                                text(&entry.status).size(12).color(MUTED),
                            ]
                            .spacing(3)
                            .width(Fill),
                            button(diagnostics_label)
                                .on_press(Message::ToggleViewerDiagnostics(process_id))
                                .padding([8, 12])
                                .style(quiet_button),
                            button("Отключить")
                                .on_press(Message::Disconnect(process_id))
                                .padding([8, 12])
                                .style(danger_text_button),
                        ]
                        .spacing(12)
                        .align_y(Alignment::Center),
                        row![
                            text(game_profile_label).size(11).color(MUTED),
                            text(format!(
                                "FPS {}.{:02}",
                                entry.fps_times_100 / 100,
                                entry.fps_times_100 % 100
                            ))
                            .size(11)
                            .color(MUTED),
                            text(format_bandwidth(entry.input_kbps))
                                .size(11)
                                .color(MUTED),
                            Space::new().width(Fill),
                            text("Управление сессией — в окне viewer")
                                .size(11)
                                .color(MUTED),
                        ]
                        .spacing(12)
                        .align_y(Alignment::Center),
                        diagnostics_details,
                    ]
                    .spacing(10),
                )
                .padding(11)
                .style(subtle_panel),
            );
        }

        container(sessions)
            .padding(22)
            .width(Fill)
            .style(card_style)
            .into()
    }

    fn home_recent_section(&self) -> Element<'_, Message> {
        let ui_language = self.ui_language();
        let favorites: Vec<_> = self
            .store
            .contacts
            .iter()
            .filter(|contact| contact.favorite)
            .take(4)
            .collect();

        let favorites_section: Element<'_, Message> = if favorites.is_empty() {
            Space::new().height(0).into()
        } else {
            let mut favorite_cards = row![].spacing(12);
            for contact in favorites {
                favorite_cards = favorite_cards.push(
                    button(
                        row![
                            container(text(device_initial(&contact.name)).color(ACCENT))
                                .center_x(Length::Fixed(36.0))
                                .center_y(Length::Fixed(36.0))
                                .style(device_icon),
                            column![
                                text(&contact.name).size(14),
                                text(&contact.remote_id).size(11).color(MUTED),
                            ]
                            .spacing(3)
                            .width(Fill),
                            lucide_icon(icondata::LuArrowRight, 18.0, ACCENT),
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center),
                    )
                    .on_press(Message::ConnectRemote(contact.remote_id.clone()))
                    .padding(12)
                    .width(Fill)
                    .style(quiet_button),
                );
            }

            column![
                row![
                    text(tr(ui_language, TextKey::HomeFavorites)).size(18),
                    Space::new().width(Fill),
                    lucide_icon(icondata::LuStar, 16.0, ACCENT),
                ]
                .align_y(Alignment::Center),
                favorite_cards,
            ]
            .spacing(10)
            .into()
        };

        let mut recent_cards = row![].spacing(12);
        if self.store.recent.is_empty() {
            recent_cards = recent_cards.push(
                container(
                    column![
                        text(tr(ui_language, TextKey::HomeRecentEmptyTitle)).size(14),
                        text(tr(ui_language, TextKey::HomeRecentEmptyHint))
                            .size(11)
                            .color(MUTED),
                    ]
                    .spacing(4),
                )
                .padding(16)
                .width(Fill)
                .style(subtle_panel),
            );
        } else {
            for connection in self.store.recent.iter().take(4) {
                let contact =
                    self.store.contacts.iter().find(|contact| {
                        remote_ids_match(&contact.remote_id, &connection.remote_id)
                    });
                let title = contact
                    .map(|contact| contact.name.as_str())
                    .unwrap_or(tr(ui_language, TextKey::HomeRemoteDevice));
                recent_cards = recent_cards.push(
                    button(
                        column![
                            text(title).size(11).color(MUTED),
                            text(&connection.remote_id).size(16),
                            text(recent_details(connection)).size(11).color(MUTED),
                        ]
                        .spacing(5)
                        .width(Fill),
                    )
                    .on_press(Message::ConnectRemote(connection.remote_id.clone()))
                    .padding(14)
                    .width(Fill)
                    .style(quiet_button),
                );
            }
        }

        column![
            favorites_section,
            column![
                row![
                    text(tr(ui_language, TextKey::HomeRecentSessions)).size(18),
                    Space::new().width(Fill),
                    button(label_with_icon(
                        tr(ui_language, TextKey::NavAddressBook),
                        icondata::LuArrowRight,
                        ACCENT,
                    ))
                    .on_press(Message::Navigate(Page::Devices))
                    .padding([7, 10])
                    .style(danger_text_button),
                ]
                .align_y(Alignment::Center),
                recent_cards,
            ]
            .spacing(10),
        ]
        .spacing(20)
        .into()
    }

    #[allow(dead_code)]
    fn smart_agent_section(&self) -> Element<'_, Message> {
        if self.smart_agent_notifications.is_empty() {
            return Space::new().height(0).into();
        }

        let mut notifications = column![row![
            lucide_icon(icondata::LuBellRing, 19.0, ACCENT),
            text("Уведомления EvertyDesk").size(17),
            Space::new().width(Fill),
            text(self.smart_agent_notifications.len().to_string())
                .size(12)
                .color(MUTED),
        ]
        .spacing(9)
        .align_y(Alignment::Center)]
        .spacing(9);

        for notification in self.smart_agent_notifications.iter().take(3) {
            let title = if notification.title.trim().is_empty() {
                "Сообщение администратора".to_owned()
            } else {
                bounded_text(&notification.title, 100)
            };
            let notification_accent =
                smart_notification_accent(&notification.severity, &notification.kind);
            let notification_type = smart_notification_type_label(&notification.kind);
            let body = bounded_text(&notification.body, 360);
            let mut details = column![row![
                text(title).size(14),
                text(notification_type).size(11).color(notification_accent),
            ]
            .spacing(8)
            .align_y(Alignment::Center)]
            .spacing(4)
            .width(Fill);
            if notification.kind == "config_update" {
                if let Some(update) = smart_agent::parse_config_update(&notification.body) {
                    details = details.push(config_update_details(update));
                } else {
                    details = details.push(text(body).size(12).color(MUTED));
                }
            } else {
                details = details.push(text(body).size(12).color(MUTED));
            }
            if is_safe_notification_link(&notification.link) {
                let link_label = if notification.link_label.trim().is_empty() {
                    "Ссылка"
                } else {
                    notification.link_label.trim()
                };
                details = details.push(
                    row![
                        text(bounded_text(link_label, 48))
                            .size(12)
                            .color(notification_accent),
                        icon_action(
                            icondata::LuCheck,
                            "Скопировать ссылку",
                            Message::CopySmartNotificationLink {
                                notification_id: notification.id,
                                url: notification.link.clone(),
                            },
                            false,
                        ),
                    ]
                    .spacing(7)
                    .align_y(Alignment::Center),
                );
            }
            let actions: Element<'_, Message> = if notification.kind == "support_ping" {
                let support = smart_agent::parse_support_options(&notification.options);
                if let Some(request_id) = support.request_id {
                    let mut buttons = column![].spacing(5);
                    for action in support.actions {
                        buttons = buttons.push(support_action_button(
                            notification.id,
                            request_id,
                            action,
                            support.from_rustdesk_id.clone(),
                        ));
                    }
                    buttons.into()
                } else {
                    button(text("Некорректный запрос").size(11))
                        .padding([7, 10])
                        .style(quiet_button)
                        .into()
                }
            } else if notification.kind == "poll" && !notification.options.is_empty() {
                let mut choices = column![].spacing(5);
                for option in notification.options.iter().take(4) {
                    choices = choices.push(
                        button(text(bounded_text(option, 48)).size(12))
                            .on_press(Message::VoteSmartNotification(
                                notification.id,
                                option.clone(),
                            ))
                            .padding([7, 10])
                            .style(quiet_button),
                    );
                }
                choices.into()
            } else {
                icon_action(
                    icondata::LuCheck,
                    "Подтвердить и скрыть",
                    Message::AcknowledgeSmartNotification(notification.id),
                    false,
                )
            };
            notifications = notifications.push(
                container(
                    row![
                        container(lucide_icon(icondata::LuInbox, 18.0, notification_accent))
                            .center_x(Length::Fixed(36.0))
                            .center_y(Length::Fixed(36.0))
                            .style(device_icon),
                        details,
                        actions,
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding(11)
                .width(Fill)
                .style(subtle_panel),
            );
        }

        container(notifications)
            .padding(16)
            .width(Fill)
            .style(card_style)
            .into()
    }

    #[allow(dead_code)]
    fn support_request_section(&self) -> Element<'_, Message> {
        if !self.store.smart_agent_enabled {
            return Space::new().height(0).into();
        }

        let load_button: Element<'_, Message> = if self.smart_agent_operators_busy {
            button(lucide_icon(icondata::LuRefreshCw, 17.0, MUTED))
                .padding([7, 10])
                .style(quiet_button)
                .into()
        } else {
            icon_action(
                icondata::LuRefreshCw,
                "Обновить операторов",
                Message::RefreshSmartOperators,
                false,
            )
        };

        let mut operators = column![].spacing(6);
        if self.smart_agent_operators.is_empty() {
            operators = operators.push(text("Операторы ещё не загружены").size(12).color(MUTED));
        }
        for operator in self.smart_agent_operators.iter().take(5) {
            let selected =
                self.support_target_machine_id.as_deref() == Some(operator.machine_id.as_str());
            let title = if operator.hostname.trim().is_empty() {
                bounded_text(&operator.machine_id, 34)
            } else {
                bounded_text(&operator.hostname, 34)
            };
            let subtitle = if operator.rustdesk_id.trim().is_empty() {
                bounded_text(&operator.os, 42)
            } else {
                format!("ID {}", format_local_id(&operator.rustdesk_id))
            };
            let status_color = if operator.online {
                Color::from_rgb(0.18, 0.76, 0.43)
            } else {
                MUTED
            };
            let button_style = move |theme: &Theme, status| {
                if selected {
                    selected_nav_button(theme, status)
                } else {
                    quiet_button(theme, status)
                }
            };
            operators = operators.push(
                button(
                    row![
                        text("●").size(10).color(status_color),
                        column![text(title).size(13), text(subtitle).size(11).color(MUTED)]
                            .spacing(1)
                            .width(Fill),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .on_press(Message::SelectSupportOperator(operator.machine_id.clone()))
                .padding([8, 10])
                .width(Fill)
                .style(button_style),
            );
        }

        let can_send = !self.support_request_busy
            && self.support_target_machine_id.is_some()
            && !self.store.smart_agent_service_key.trim().is_empty();
        let send_label = if self.support_request_busy {
            "Отправляется"
        } else {
            "Запросить поддержку"
        };
        let send_button = if can_send {
            button(label_with_icon(
                send_label,
                icondata::LuArrowRight,
                Color::WHITE,
            ))
            .on_press(Message::RequestSmartSupport)
            .padding([10, 14])
            .style(accent_button)
        } else {
            button(label_with_icon(
                send_label,
                icondata::LuArrowRight,
                Color::WHITE,
            ))
            .padding([10, 14])
            .style(accent_button)
        };

        let request_form = column![
            text_input("Коротко опишите проблему", &self.support_request_message)
                .on_input(Message::SupportRequestMessageChanged)
                .on_submit(Message::RequestSmartSupport)
                .padding(10)
                .style(input_style),
            row![
                text(&self.support_request_status).size(12).color(MUTED),
                Space::new().width(Fill),
                text(support_message_counter(&self.support_request_message))
                    .size(11)
                    .color(
                        if self.support_request_message.chars().count() >= MAX_SUPPORT_MESSAGE_CHARS
                        {
                            ACCENT
                        } else {
                            MUTED
                        }
                    ),
                send_button,
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        ]
        .spacing(10)
        .width(Fill);

        let support_body: Element<'_, Message> =
            if use_wide_support_layout(self.main_window_size.width) {
                row![
                    container(operators).width(Length::Fixed(300.0)),
                    request_form,
                ]
                .spacing(14)
                .align_y(Alignment::Start)
                .into()
            } else {
                column![container(operators).width(Fill), request_form,]
                    .spacing(12)
                    .into()
            };

        container(
            column![
                row![
                    lucide_icon(icondata::LuBellRing, 18.0, ACCENT),
                    text("Поддержка через desk.everty.ru").size(17),
                    Space::new().width(Fill),
                    load_button,
                ]
                .spacing(9)
                .align_y(Alignment::Center),
                support_body,
            ]
            .spacing(12),
        )
        .padding(16)
        .width(Fill)
        .style(card_style)
        .into()
    }

    fn compatibility_settings_section(&self) -> Element<'_, Message> {
        let ui_language = self.ui_language();
        let expanded = self.store.compatibility_settings_expanded;
        let custom_active = server_config_is_custom(&self.config);
        let chevron = if expanded {
            icondata::LuMinus
        } else {
            icondata::LuPlus
        };
        let mut content = column![button(
            row![
                lucide_icon(chevron, 17.0, ACCENT),
                column![
                    text(tr(ui_language, TextKey::SettingsCompatibilityTitle)).size(18),
                    text(if custom_active {
                        tr(ui_language, TextKey::SettingsCompatibilityCustom)
                    } else {
                        tr(ui_language, TextKey::SettingsCompatibilityDefault)
                    })
                    .size(12)
                    .color(MUTED),
                ]
                .spacing(2)
                .width(Fill),
                text(if expanded {
                    tr(ui_language, TextKey::SettingsCompatibilityHide)
                } else {
                    tr(ui_language, TextKey::SettingsCompatibilityShow)
                })
                .size(12)
                .color(ACCENT),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .on_press(Message::ToggleCompatibilitySettings)
        .padding([8, 10])
        .width(Fill)
        .style(quiet_button)]
        .spacing(12);

        if expanded {
            let default_server = ServerConfig::default();
            let discover_button = if self.server_discovery_busy {
                button(tr(ui_language, TextKey::SettingsCompatibilityDiscovering))
                    .padding([8, 12])
                    .style(quiet_button)
            } else {
                button(tr(ui_language, TextKey::SettingsCompatibilityDiscover))
                    .on_press(Message::DiscoverServerSettings)
                    .padding([8, 12])
                    .style(quiet_button)
            };
            let fields = column![
                row![
                    discover_button,
                    text(if self.server_discovery_status.is_empty() {
                        tr(ui_language, TextKey::SettingsCompatibilityDiscoveryHint)
                    } else {
                        &self.server_discovery_status
                    })
                    .size(11)
                    .color(MUTED)
                    .width(Fill),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
                row![
                    text_input(
                        "API URL, например https://api.example.com",
                        &server_input_value(&self.config.server.api_url, &default_server.api_url)
                    )
                    .on_input(Message::ServerApiUrlChanged)
                    .padding(10)
                    .style(input_style),
                    text_input(
                        "ID server",
                        &server_input_value(
                            &self.config.server.id_server,
                            &default_server.id_server
                        )
                    )
                    .on_input(Message::ServerIdChanged)
                    .padding(10)
                    .style(input_style),
                ]
                .spacing(10),
                row![
                    text_input(
                        "Relay server",
                        &server_input_value(
                            &self.config.server.relay_server,
                            &default_server.relay_server
                        )
                    )
                    .on_input(Message::ServerRelayChanged)
                    .padding(10)
                    .style(input_style),
                    text_input(
                        "Public Key",
                        &server_input_value(
                            &self.config.server.public_key,
                            &default_server.public_key
                        )
                    )
                    .on_input(Message::ServerPublicKeyChanged)
                    .padding(10)
                    .style(input_style),
                ]
                .spacing(10),
                row![
                    text(tr(
                        ui_language,
                        TextKey::SettingsCompatibilityEmptyFieldsHint
                    ))
                    .size(11)
                    .color(MUTED),
                    Space::new().width(Fill),
                    button(tr(ui_language, TextKey::SettingsReset))
                        .on_press(Message::ResetServerSettings)
                        .padding([8, 12])
                        .style(danger_text_button),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            ]
            .spacing(10);
            content = content.push(
                container(fields)
                    .padding(12)
                    .width(Fill)
                    .style(subtle_panel),
            );
        }

        container(content)
            .padding(14)
            .width(Fill)
            .style(card_style)
            .into()
    }

    fn vm_settings_section(&self) -> Element<'_, Message> {
        let expanded = self.store.vm_settings_expanded;
        let chevron = if expanded {
            icondata::LuMinus
        } else {
            icondata::LuPlus
        };
        let active_text = if self.store.vm_bridge_enabled {
            "VM Bridge включён"
        } else {
            "VM Bridge выключен"
        };

        let entitlement_note = commercial_feature_note(
            self.account_entitlements.known,
            self.account_entitlements.vm,
            "VM",
        );
        let mut content = column![button(
            row![
                lucide_icon(chevron, 17.0, ACCENT),
                column![
                    text("VM Bridge и Game режим").size(18),
                    text(format!(
                        "{active_text} · {} · {}",
                        self.store.vm_provider.label(),
                        streaming_mode_label(self.config.display.streaming_mode)
                    ))
                    .size(12)
                    .color(MUTED),
                ]
                .spacing(2)
                .width(Fill),
                text(if expanded {
                    "Свернуть"
                } else {
                    "Развернуть"
                })
                .size(12)
                .color(ACCENT),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .on_press(Message::ToggleVmSettings)
        .padding([8, 10])
        .width(Fill)
        .style(quiet_button)]
        .spacing(12);

        if let Some(note) = entitlement_note {
            content = content.push(text(note).size(11).color(MUTED));
        }

        if expanded {
            let mut provider_buttons = row![].spacing(8);
            for provider in VmProviderPreference::ALL {
                let selected = self.store.vm_provider == provider;
                provider_buttons = provider_buttons.push(
                    button(text(provider.label()).size(13))
                        .on_press(Message::SetVmProvider(provider))
                        .padding([8, 14])
                        .style(move |theme, status| {
                            if selected {
                                selected_segment(theme, status)
                            } else {
                                quiet_button(theme, status)
                            }
                        }),
                );
            }

            let list_button = if self.vm_bridge_busy {
                button("Список VM").padding([8, 12]).style(quiet_button)
            } else {
                button("Список VM")
                    .on_press(Message::RefreshVmBridge)
                    .padding([8, 12])
                    .style(quiet_button)
            };
            let attach_button = if self.vm_bridge_busy || !self.store.vm_bridge_enabled {
                button("Подключить VM")
                    .padding([8, 12])
                    .style(accent_button)
            } else {
                button("Подключить VM")
                    .on_press(Message::AttachVmBridge)
                    .padding([8, 12])
                    .style(accent_button)
            };
            let detach_button = if self.vm_bridge_busy {
                button("Отключить VM")
                    .padding([8, 12])
                    .style(danger_text_button)
            } else {
                button("Отключить VM")
                    .on_press(Message::DetachVmBridge)
                    .padding([8, 12])
                    .style(danger_text_button)
            };

            let status_text = if self.vm_bridge_status.trim().is_empty() {
                current_vm_status_text()
            } else {
                self.vm_bridge_status.clone()
            };

            let body = column![
                checkbox(self.store.vm_bridge_enabled)
                    .label("Включить VM Bridge для входящих сессий")
                    .on_toggle(Message::SetVmBridgeEnabled)
                    .size(16),
                text("Старый EvertyDesk Lite bridge умеет подменять физический экран активной VM консолью и маршрутизировать ввод в VM.")
                    .size(11)
                    .color(MUTED),
                column![
                    text("Провайдер").size(13),
                    provider_buttons,
                    text("Auto оставляет введённый ID как есть; Hyper-V/VirtualBox добавляют префикс hyperv:/vbox:, если его нет.")
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(7),
                row![
                    text_input(
                        "VM ID, например hyperv:<id> или vbox:<uuid>",
                        &self.store.vm_target_id
                    )
                    .on_input(Message::VmTargetChanged)
                    .padding(10)
                    .style(input_style)
                    .width(Fill),
                    list_button,
                    attach_button,
                    detach_button,
                ]
                .spacing(10)
                .align_y(Alignment::Center),
                row![
                    text("Game режим включает 60 FPS и отключает adaptive quality.")
                        .size(11)
                        .color(MUTED)
                        .width(Fill),
                    button("Включить Game")
                        .on_press(Message::SetStreamingMode(StreamingMode::Game))
                        .padding([8, 12])
                        .style(quiet_button),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
                container(text(status_text).size(11).color(MUTED))
                    .padding(12)
                    .width(Fill)
                    .style(subtle_panel),
                self.vm_inventory_panel(),
            ]
            .spacing(10);

            content = content.push(container(body).padding(12).width(Fill).style(subtle_panel));
        }

        container(content)
            .padding(14)
            .width(Fill)
            .style(card_style)
            .into()
    }

    fn vm_inventory_panel(&self) -> Element<'_, Message> {
        if self.vm_inventory.is_empty() {
            return Space::new().height(Length::Fixed(0.0)).width(Fill).into();
        }

        let selected_target = sanitize_vm_target_id(&self.store.vm_target_id);
        let visible_vms = self
            .vm_inventory
            .iter()
            .filter(|vm| vm_matches_filter(vm, &self.vm_inventory_filter))
            .collect::<Vec<_>>();
        let visible_count = visible_vms.len();
        let mut groups: BTreeMap<&'static str, Vec<&VmInventoryEntry>> = BTreeMap::new();
        for vm in visible_vms {
            groups
                .entry(vm_inventory_group_key(&vm.id))
                .or_default()
                .push(vm);
        }

        let mut list = column![
            row![
                text_input(
                    "\u{41F}\u{43E}\u{438}\u{441}\u{43A}\u{20}\u{56}\u{4D}\u{3A}\u{20}\u{438}\u{43C}\u{44F}\u{2C}\u{20}\u{49}\u{44}\u{2C}\u{20}\u{441}\u{43E}\u{441}\u{442}\u{43E}\u{44F}\u{43D}\u{438}\u{435}\u{20}\u{438}\u{43B}\u{438}\u{20}\u{43F}\u{440}\u{43E}\u{432}\u{430}\u{439}\u{434}\u{435}\u{440}",
                    &self.vm_inventory_filter
                )
                .on_input(Message::VmInventoryFilterChanged)
                .padding(10)
                .style(input_style)
                .width(Fill),
                text(format!("{visible_count}/{}", self.vm_inventory.len()))
                    .size(11)
                    .color(MUTED),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            row![
                text("\u{41D}\u{430}\u{439}\u{434}\u{435}\u{43D}\u{43D}\u{44B}\u{435}\u{20}\u{56}\u{4D}").size(13).width(Fill),
                text(format!("{} \u{448}\u{442}\u{2E}", self.vm_inventory.len()))
                    .size(11)
                    .color(MUTED),
            ]
            .align_y(Alignment::Center)
        ]
        .spacing(10);

        if groups.is_empty() {
            list = list.push(empty_state(
                "VM \u{43D}\u{435}\u{20}\u{43D}\u{430}\u{439}\u{434}\u{435}\u{43D}\u{44B}\u{20}\u{43F}\u{43E}\u{20}\u{444}\u{438}\u{43B}\u{44C}\u{442}\u{440}\u{443}",
                "\u{41E}\u{447}\u{438}\u{441}\u{442}\u{438}\u{442}\u{435}\u{20}\u{43F}\u{43E}\u{438}\u{441}\u{43A}\u{20}\u{438}\u{43B}\u{438}\u{20}\u{43E}\u{431}\u{43D}\u{43E}\u{432}\u{438}\u{442}\u{435}\u{20}\u{441}\u{43F}\u{438}\u{441}\u{43E}\u{43A}\u{20}\u{56}\u{4D}\u{2E}",
            ));
        }

        for (group, vms) in groups {
            let mut group_column = column![row![
                vm_badge(vm_inventory_group_label(group), MUTED),
                text(format!("{} VM", vms.len())).size(11).color(MUTED),
            ]
            .spacing(8)
            .align_y(Alignment::Center)]
            .spacing(8);

            for vm in vms {
                let is_selected = selected_target == sanitize_vm_target_id(&vm.id);
                let state_color = vm_state_color(&vm.state, vm.connectable);
                let select_button = if is_selected {
                    button("\u{412}\u{44B}\u{431}\u{440}\u{430}\u{43D}\u{430}")
                        .padding([7, 10])
                        .style(quiet_button)
                } else {
                    button("\u{412}\u{44B}\u{431}\u{440}\u{430}\u{442}\u{44C}")
                        .on_press(Message::SelectVmTarget(vm.id.clone()))
                        .padding([7, 10])
                        .style(quiet_button)
                };
                let connect_button = if self.vm_bridge_busy || !vm.connectable {
                    button("\u{41F}\u{43E}\u{434}\u{43A}\u{43B}\u{44E}\u{447}\u{438}\u{442}\u{44C}")
                        .padding([7, 10])
                        .style(accent_button)
                } else {
                    button("\u{41F}\u{43E}\u{434}\u{43A}\u{43B}\u{44E}\u{447}\u{438}\u{442}\u{44C}")
                        .on_press(Message::AttachVmBridgeTarget(vm.id.clone()))
                        .padding([7, 10])
                        .style(accent_button)
                };
                let availability = if vm.connectable {
                    "\u{434}\u{43E}\u{441}\u{442}\u{443}\u{43F}\u{43D}\u{430}"
                } else {
                    "\u{43D}\u{435}\u{434}\u{43E}\u{441}\u{442}\u{443}\u{43F}\u{43D}\u{430}"
                };
                let power_controls =
                    vm_power_controls(&vm.id, self.vm_bridge_busy || !vm.connectable);
                // RDP console (Hyper-V Enhanced Session only for now — VirtualBox
                // VRDE needs port-discovery plumbing this doesn't have yet, see
                // rdp_viewer.rs's module doc).
                let rdp_button: Element<'_, Message> =
                    if vm.connectable && vm_inventory_group_key(&vm.id) == "1_hyperv" {
                        button("RDP")
                            .on_press(Message::ConnectVmRdp(vm.id.clone()))
                            .padding([7, 10])
                            .style(quiet_button)
                            .into()
                    } else {
                        Space::new().width(Length::Fixed(0.0)).into()
                    };
                group_column = group_column.push(
                    container(
                        row![
                            column![
                                row![
                                    text(&vm.name).size(14),
                                    if is_selected {
                                        vm_badge(
                                            "\u{432}\u{44B}\u{431}\u{440}\u{430}\u{43D}\u{430}",
                                            ACCENT,
                                        )
                                    } else {
                                        Space::new().width(Length::Fixed(0.0)).into()
                                    },
                                ]
                                .spacing(8)
                                .align_y(Alignment::Center),
                                row![
                                    text(&vm.id).size(11).color(MUTED),
                                    vm_badge(vm_provider_label_for_id(&vm.id), MUTED),
                                    vm_badge(&vm.state, state_color),
                                    vm_badge(availability, state_color),
                                ]
                                .spacing(6)
                                .align_y(Alignment::Center),
                            ]
                            .spacing(5)
                            .width(Fill),
                            power_controls,
                            rdp_button,
                            select_button,
                            connect_button,
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center),
                    )
                    .padding(10)
                    .width(Fill)
                    .style(subtle_panel),
                );
            }

            list = list.push(
                container(group_column)
                    .padding(10)
                    .width(Fill)
                    .style(subtle_panel),
            );
        }

        container(list)
            .padding(12)
            .width(Fill)
            .style(subtle_panel)
            .into()
    }

    fn vm_page_section(&self) -> Element<'_, Message> {
        let selected_vm_controls: Element<'_, Message> = if self
            .store
            .vm_target_id
            .trim()
            .is_empty()
        {
            empty_state(
                    "VM \u{43D}\u{435}\u{20}\u{432}\u{44B}\u{431}\u{440}\u{430}\u{43D}\u{430}",
                    "\u{41D}\u{430}\u{436}\u{43C}\u{438}\u{442}\u{435}\u{20}\u{AB}\u{421}\u{43F}\u{438}\u{441}\u{43E}\u{43A}\u{20}\u{56}\u{4D}\u{BB}\u{20}\u{438}\u{20}\u{432}\u{44B}\u{431}\u{435}\u{440}\u{438}\u{442}\u{435}\u{20}\u{43C}\u{430}\u{448}\u{438}\u{43D}\u{443}\u{20}\u{438}\u{43B}\u{438}\u{20}\u{432}\u{432}\u{435}\u{434}\u{438}\u{442}\u{435}\u{20}\u{56}\u{4D}\u{20}\u{49}\u{44}\u{20}\u{432}\u{440}\u{443}\u{447}\u{43D}\u{443}\u{44E}\u{2E}",
                )
        } else {
            let status_text = if self.vm_bridge_status.trim().is_empty() {
                current_vm_status_text()
            } else {
                self.vm_bridge_status.clone()
            };
            let attach_button = if self.vm_bridge_busy || !self.store.vm_bridge_enabled {
                button("\u{41F}\u{43E}\u{434}\u{43A}\u{43B}\u{44E}\u{447}\u{438}\u{442}\u{44C}\u{20}\u{56}\u{4D}")
                        .padding([8, 12])
                        .style(accent_button)
            } else {
                button("\u{41F}\u{43E}\u{434}\u{43A}\u{43B}\u{44E}\u{447}\u{438}\u{442}\u{44C}\u{20}\u{56}\u{4D}")
                        .on_press(Message::AttachVmBridge)
                        .padding([8, 12])
                        .style(accent_button)
            };
            let detach_button = if self.vm_bridge_busy {
                button("\u{41E}\u{442}\u{43A}\u{43B}\u{44E}\u{447}\u{438}\u{442}\u{44C}")
                    .padding([8, 12])
                    .style(danger_text_button)
            } else {
                button("\u{41E}\u{442}\u{43A}\u{43B}\u{44E}\u{447}\u{438}\u{442}\u{44C}")
                    .on_press(Message::DetachVmBridge)
                    .padding([8, 12])
                    .style(danger_text_button)
            };
            container(
                    row![
                        column![
                            text("\u{412}\u{44B}\u{431}\u{440}\u{430}\u{43D}\u{43D}\u{430}\u{44F}\u{20}\u{56}\u{4D}").size(13),
                            text(&self.store.vm_target_id).size(12).color(MUTED),
                            text(status_text).size(11).color(MUTED),
                        ]
                        .spacing(5)
                        .width(Fill),
                        attach_button,
                        detach_button,
                        vm_power_controls(
                            &self.store.vm_target_id,
                            self.vm_bridge_busy || !self.store.vm_bridge_enabled,
                        ),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding(14)
                .width(Fill)
                .style(card_style)
                .into()
        };
        column![
            self.vm_settings_section(),
            selected_vm_controls,
            container(
                column![
                    text("Как в EvertyDesk Lite").size(16),
                    text("VM вынесена отдельной страницей: здесь основной сценарий работы с локальными и удалёнными VM, а в настройках останутся технические параметры.")
                        .size(12)
                        .color(MUTED),
                ]
                .spacing(6),
            )
            .padding(14)
            .width(Fill)
            .style(card_style),
        ]
        .spacing(14)
        .into()
    }

    fn game_page_section(&self) -> Element<'_, Message> {
        let ui_language = self.ui_language();
        let game_selected = self.config.display.streaming_mode == StreamingMode::Game;
        let entitlement_note = commercial_feature_note(
            self.account_entitlements.known,
            self.account_entitlements.vm,
            "Game/VM",
        );
        let active_quality = format!(
            "{} FPS · adaptive quality {}",
            self.config.display.target_fps,
            if self.config.display.adaptive_quality {
                "включён"
            } else {
                "выключен"
            }
        );
        let mut quality_buttons = row![].spacing(8);
        for quality in ConnectionQuality::ALL {
            let selected = self.store.quality == quality;
            quality_buttons = quality_buttons.push(
                button(text(quality_label(quality, ui_language)).size(13))
                    .on_press(Message::SetQuality(quality))
                    .padding([8, 14])
                    .style(move |theme, status| {
                        if selected {
                            selected_segment(theme, status)
                        } else {
                            quiet_button(theme, status)
                        }
                    }),
            );
        }
        let mut codec_buttons = row![].spacing(8);
        for codec in GameCodecPreference::ALL {
            let selected = self.store.game_codec == codec;
            codec_buttons = codec_buttons.push(
                button(text(codec.label()).size(13))
                    .on_press(Message::SetGameCodec(codec))
                    .padding([8, 14])
                    .style(move |theme, status| {
                        if selected {
                            selected_segment(theme, status)
                        } else {
                            quiet_button(theme, status)
                        }
                    }),
            );
        }

        let can_connect = !normalize_remote_id(&self.game_remote_id).is_empty();
        let connect_button = if can_connect {
            button("Подключиться в Game")
                .on_press(Message::ConnectGame)
                .padding([11, 18])
                .style(accent_button)
        } else {
            button("Подключиться в Game")
                .padding([11, 18])
                .style(accent_button)
        };

        container(
            column![
                row![
                    lucide_icon(icondata::LuMousePointer2, 20.0, ACCENT),
                    column![
                        text("GAME профиль подключения").size(18),
                        text(active_quality).size(12).color(MUTED),
                    ]
                    .spacing(2)
                    .width(Fill),
                    button(if game_selected {
                        "Game включён"
                    } else {
                        "Включить Game"
                    })
                    .on_press(Message::SetStreamingMode(StreamingMode::Game))
                    .padding([9, 14])
                    .style(accent_button),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
                text(entitlement_note.unwrap_or(
                    "Game профиль использует локальные настройки качества; тарифная проверка пока мягкая.",
                ))
                .size(11)
                .color(MUTED),
                container(
                    column![
                        text("Новое Game подключение").size(13),
                        row![
                            text_input("ID удалённого устройства", &self.game_remote_id)
                                .on_input(Message::GameRemoteIdChanged)
                                .on_submit(Message::ConnectGame)
                                .padding(10)
                                .style(input_style)
                                .width(Fill),
                            text_input("Пароль", &self.game_password)
                                .secure(true)
                                .on_input(Message::GamePasswordChanged)
                                .on_submit(Message::ConnectGame)
                                .padding(10)
                                .style(input_style)
                                .width(Fill),
                            connect_button,
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center),
                        row![
                            checkbox(self.game_remember_password)
                                .label("Запомнить пароль")
                                .on_toggle(Message::ToggleGameRememberPassword)
                                .size(16),
                            Space::new().width(Fill),
                            text(&self.game_connect_status).size(11).color(MUTED),
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center),
                        text("Game подключение использует отдельные поля ID/пароля, как в EvertyDesk Lite, и перед запуском viewer включает 60 FPS + выбранный Game codec профиль.")
                            .size(11)
                            .color(MUTED),
                    ]
                    .spacing(9),
                )
                .padding(14)
                .width(Fill)
                .style(subtle_panel),
                container(
                    column![
                        text("Качество для новых viewer-сессий").size(13),
                        quality_buttons,
                        text("Для Game режима старый Lite использовал отдельный сценарий: конкретный codec/encoder, 60 FPS, без static-frame skip. Сейчас переносим это в Iced по частям.")
                            .size(11)
                            .color(MUTED),
                    ]
                    .spacing(8),
                )
                .padding(14)
                .width(Fill)
                .style(subtle_panel),
                container(
                    column![
                        text("Game codec").size(13),
                        codec_buttons,
                        text(self.store.game_codec.hint())
                            .size(11)
                            .color(MUTED),
                    ]
                    .spacing(8),
                )
                .padding(14)
                .width(Fill)
                .style(subtle_panel),
                container(
                    column![
                        checkbox(self.store.game_evrt2_enabled)
                            .label("EVRT2 поверх Game transport")
                            .on_toggle(Message::SetGameEvrt2)
                            .size(16),
                        text("В старом Lite это был экспериментальный второй поток. В desktop-next настройка уже сохраняется; подключение к handshake/transport следующим пакетом.")
                            .size(11)
                            .color(MUTED),
                    ]
                    .spacing(6),
                )
                .padding(14)
                .width(Fill)
                .style(subtle_panel),
            ]
            .spacing(12),
        )
        .padding(16)
        .width(Fill)
        .style(card_style)
        .into()
    }

    fn devices_section(&self) -> Element<'_, Message> {
        let ui_language = self.ui_language();
        let filter = self.device_filter.trim().to_lowercase();
        let recent_ids = normalized_recent_ids(&self.store.recent);
        let mut groups: BTreeMap<String, usize> = BTreeMap::new();
        let mut tags: BTreeMap<String, usize> = BTreeMap::new();
        let mut favorites = 0usize;
        for contact in &self.store.contacts {
            if contact.favorite {
                favorites += 1;
            }
            let group = if contact.group.trim().is_empty() {
                tr(ui_language, TextKey::AddressBookNoGroup).to_owned()
            } else {
                format_group_path(&contact.group)
            };
            if contact.group.trim().is_empty() {
                *groups.entry(group).or_default() += 1;
            } else {
                for group in group_path_ancestors(&contact.group) {
                    *groups.entry(group).or_default() += 1;
                }
            }
            for tag in &contact.tags {
                *tags.entry(tag.clone()).or_default() += 1;
            }
        }
        let account_panel = self.address_book_sync_banner();
        let can_save = !self.remote_id.trim().is_empty() && !self.contact_name.trim().is_empty();
        let editing = self.editing_contact_id.is_some();
        let remote_id_editor: Element<'_, Message> = if editing {
            container(
                column![
                    text("ID устройства").size(10).color(MUTED),
                    text(format_local_id(&self.remote_id)).size(14),
                ]
                .spacing(2),
            )
            .padding([8, 11])
            .width(Fill)
            .style(subtle_panel)
            .into()
        } else {
            text_input("ID устройства", &self.remote_id)
                .on_input(Message::RemoteIdChanged)
                .padding(11)
                .style(input_style)
                .width(Fill)
                .into()
        };

        let mut contacts = column![row![
            column![
                text(address_book_filter_label(
                    &self.address_book_filter,
                    ui_language
                ))
                .size(20),
                text(tr(ui_language, TextKey::AddressBookLocalCloudDevices))
                    .size(12)
                    .color(MUTED),
            ]
            .spacing(2)
            .width(Fill),
            icon_action(
                if self.contact_form_expanded {
                    icondata::LuMinus
                } else {
                    icondata::LuPlus
                },
                if self.contact_form_expanded {
                    "Скрыть форму контакта"
                } else {
                    "Добавить новый контакт"
                },
                Message::ToggleContactForm,
                false,
            ),
        ]
        .align_y(Alignment::Center),]
        .spacing(10);

        let visible_contacts: Vec<_> = self
            .store
            .contacts
            .iter()
            .filter(|contact| {
                contact_matches_address_book_filter(contact, &self.address_book_filter, &recent_ids)
                    && contact_matches_text_filter(contact, &filter)
            })
            .collect();
        let visible_contact_count = visible_contacts.len();

        if self.store.contacts.is_empty() {
            contacts = contacts.push(empty_state(
                "Нет сохранённых устройств",
                "Выберите адрес выше, задайте название и сохраните его.",
            ));
        } else if visible_contacts.is_empty() {
            contacts = contacts.push(empty_state(
                "Контакты не найдены",
                "Попробуйте изменить поисковый запрос.",
            ));
        } else {
            let mut last_group: Option<&str> = None;
            for contact in visible_contacts {
                let group = if contact.group.is_empty() {
                    tr(ui_language, TextKey::AddressBookNoGroup)
                } else {
                    contact.group.as_str()
                };
                if last_group != Some(group) {
                    let group_label = format_group_path(group);
                    contacts = contacts.push(
                        row![
                            button(text(group_label.clone()).size(12).color(ACCENT))
                                .on_press(Message::SelectAddressBookFilter(
                                    AddressBookFilter::Group(group_label)
                                ))
                                .padding([2, 0])
                                .style(quiet_button),
                            container(Space::new().height(1))
                                .width(Fill)
                                .style(separator_style),
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center),
                    );
                    last_group = Some(group);
                }
                let details = contact_details(
                    contact.remote_id.clone(),
                    contact.note.clone(),
                    contact.group.clone(),
                    contact.tags.clone(),
                );
                let selected = self
                    .selected_contact_id
                    .as_deref()
                    .is_some_and(|id| remote_ids_match(id, &contact.remote_id));
                contacts = contacts.push(
                    container(
                        row![
                            container(text(device_initial(&contact.name)).color(ACCENT))
                                .center_x(Length::Fixed(32.0))
                                .center_y(Length::Fixed(32.0))
                                .style(device_icon),
                            column![text(&contact.name).size(14), details,]
                                .spacing(2)
                                .width(Fill),
                            icon_action(
                                icondata::LuStar,
                                if contact.favorite {
                                    "Убрать из избранного"
                                } else {
                                    "Добавить в избранное"
                                },
                                Message::ToggleFavorite(contact.remote_id.clone()),
                                contact.favorite,
                            ),
                            icon_action(
                                icondata::LuInfo,
                                "Показать детали",
                                Message::SelectContact(contact.remote_id.clone()),
                                selected,
                            ),
                            icon_action(
                                icondata::LuPencil,
                                "Редактировать контакт",
                                Message::EditContact(contact.remote_id.clone()),
                                false,
                            ),
                            icon_action(
                                icondata::LuArrowRight,
                                "Подключиться",
                                Message::ConnectRemote(contact.remote_id.clone()),
                                false,
                            ),
                            icon_action(
                                icondata::LuTrash2,
                                "Удалить контакт",
                                Message::RemoveContact(contact.remote_id.clone()),
                                true,
                            ),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .padding([7, 9])
                    .style(subtle_panel),
                );
            }
        }

        let mut recent = column![row![
            column![
                text("Недавние").size(20),
                text("Последние адреса подключений").size(12).color(MUTED),
            ]
            .spacing(2)
            .width(Fill),
            if self.store.recent.is_empty() {
                button(lucide_icon(icondata::LuTrash2, 17.0, MUTED))
                    .padding([7, 10])
                    .style(quiet_button)
                    .into()
            } else {
                icon_action(
                    icondata::LuTrash2,
                    "Очистить историю",
                    Message::ClearRecent,
                    true,
                )
            }
        ]
        .align_y(Alignment::Center),]
        .spacing(10);
        let visible_recent: Vec<_> = self
            .store
            .recent
            .iter()
            .filter(|connection| {
                if connection.remote_id.to_lowercase().contains(&filter) {
                    return true;
                }
                self.store.contacts.iter().any(|contact| {
                    remote_ids_match(&contact.remote_id, &connection.remote_id)
                        && contact.name.to_lowercase().contains(&filter)
                })
            })
            .collect();
        if self.store.recent.is_empty() {
            recent = recent.push(empty_state(
                "История пока пуста",
                "Здесь появятся последние подключения.",
            ));
        } else if visible_recent.is_empty() {
            recent = recent.push(empty_state(
                "В истории ничего не найдено",
                "Попробуйте изменить поисковый запрос.",
            ));
        } else {
            for connection in visible_recent {
                recent = recent.push(
                    container(
                        row![
                            column![
                                text(&connection.remote_id).size(14),
                                text(recent_details(connection)).size(11).color(MUTED),
                            ]
                            .spacing(2)
                            .width(Fill),
                            icon_action(
                                icondata::LuMousePointer2,
                                "Подставить адрес",
                                Message::SelectRemote(connection.remote_id.clone()),
                                false,
                            ),
                            icon_action(
                                icondata::LuArrowRight,
                                "Подключиться",
                                Message::ConnectRemote(connection.remote_id.clone()),
                                false,
                            ),
                            icon_action(
                                icondata::LuTrash2,
                                "Удалить из истории",
                                Message::RemoveRecent(connection.remote_id.clone()),
                                true,
                            ),
                        ]
                        .align_y(Alignment::Center),
                    )
                    .padding([7, 9])
                    .style(subtle_panel),
                );
            }
        }

        let show_recent_panel = address_book_filter_uses_recent_panel(&self.address_book_filter);
        let lists: Element<'_, Message> =
            if show_recent_panel && use_wide_directory_layout(self.main_window_size.width) {
                row![
                    container(contacts)
                        .padding(18)
                        .width(Length::FillPortion(3))
                        .style(card_style),
                    container(recent)
                        .padding(18)
                        .width(Length::FillPortion(2))
                        .style(card_style),
                ]
                .spacing(16)
                .into()
            } else if show_recent_panel {
                column![
                    container(contacts)
                        .padding(18)
                        .width(Fill)
                        .style(card_style),
                    container(recent).padding(18).width(Fill).style(card_style),
                ]
                .spacing(14)
                .into()
            } else {
                container(contacts)
                    .padding(18)
                    .width(Fill)
                    .style(card_style)
                    .into()
            };
        let selected_contact = selected_contact_for_filter(
            &self.store.contacts,
            self.selected_contact_id.as_deref(),
            &self.address_book_filter,
            &recent_ids,
        )
        .cloned();
        let group_suggestions = address_book_group_suggestions(&self.store.contacts, 5);
        let tag_suggestions = address_book_tag_suggestions(&self.store.contacts, 8);
        let directory_body: Element<'_, Message> = if self.contact_form_expanded {
            let mut group_chips = row![].spacing(5).align_y(Alignment::Center);
            for group in group_suggestions {
                group_chips = group_chips.push(contact_filter_chip(
                    icondata::LuFolder,
                    group.clone(),
                    Message::UseContactGroup(group),
                ));
            }
            let mut tag_chips = row![].spacing(5).align_y(Alignment::Center);
            for tag in tag_suggestions {
                tag_chips = tag_chips.push(contact_filter_chip(
                    icondata::LuTag,
                    format!("#{tag}"),
                    Message::AddContactTag(tag),
                ));
            }
            let editor_panel = container(
                column![
                    row![
                        column![
                            text(if editing {
                                "Редактирование"
                            } else {
                                "Новый контакт"
                            })
                            .size(18),
                            text("Имя и ID обязательны").size(11).color(MUTED),
                        ]
                        .spacing(2)
                        .width(Fill),
                        icon_action(
                            icondata::LuX,
                            "Закрыть форму",
                            Message::CancelContactEdit,
                            true,
                        ),
                    ]
                    .align_y(Alignment::Center),
                    text_input("Название устройства", &self.contact_name)
                        .on_input(Message::ContactNameChanged)
                        .on_submit(Message::SaveContact)
                        .padding(10)
                        .style(input_style)
                        .width(Fill),
                    remote_id_editor,
                    text_input("Группа / путь", &self.contact_group)
                        .on_input(Message::ContactGroupChanged)
                        .padding(10)
                        .style(input_style)
                        .width(Fill),
                    form_suggestions("Группы", group_chips),
                    text_input("Метки через запятую", &self.contact_tags)
                        .on_input(Message::ContactTagsChanged)
                        .padding(10)
                        .style(input_style)
                        .width(Fill),
                    form_suggestions("Метки", tag_chips),
                    text_input("Заметка", &self.contact_note)
                        .on_input(Message::ContactNoteChanged)
                        .on_submit(Message::SaveContact)
                        .padding(10)
                        .style(input_style)
                        .width(Fill),
                    row![
                        button(if editing {
                            "Сохранить"
                        } else {
                            "Добавить"
                        })
                        .on_press_maybe(can_save.then_some(Message::SaveContact))
                        .padding([10, 16])
                        .style(accent_button),
                        button("Очистить")
                            .on_press(Message::CancelContactEdit)
                            .padding([10, 14])
                            .style(quiet_button),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                ]
                .spacing(10),
            )
            .padding(16)
            .width(Length::Fixed(340.0))
            .style(card_style);
            let mut side_column = column![editor_panel].spacing(14);
            if let Some(contact) = selected_contact.clone() {
                side_column = side_column.push(contact_detail_panel(contact, ui_language));
            }

            if self.main_window_size.width >= 1_180.0 {
                row![
                    container(lists).width(Fill),
                    container(side_column).width(Length::Fixed(340.0))
                ]
                .spacing(14)
                .into()
            } else {
                column![container(side_column).width(Fill), lists]
                    .spacing(14)
                    .into()
            }
        } else if let Some(contact) = selected_contact {
            let details = contact_detail_panel(contact, ui_language);
            if self.main_window_size.width >= 1_180.0 {
                row![
                    container(lists).width(Fill),
                    container(details).width(Length::Fixed(340.0))
                ]
                .spacing(14)
                .into()
            } else {
                column![details, lists].spacing(14).into()
            }
        } else {
            lists
        };

        let mut address_book_nav = column![
            text("Адресная книга").size(13).color(MUTED),
            address_book_nav_item(
                icondata::LuBookOpen,
                "Все контакты",
                self.store.contacts.len(),
                self.address_book_filter == AddressBookFilter::All,
                Message::SelectAddressBookFilter(AddressBookFilter::All),
            ),
            address_book_nav_item(
                icondata::LuStar,
                "Избранные",
                favorites,
                self.address_book_filter == AddressBookFilter::Favorites,
                Message::SelectAddressBookFilter(AddressBookFilter::Favorites),
            ),
            address_book_nav_item(
                icondata::LuClock3,
                "Недавние",
                self.store.recent.len(),
                self.address_book_filter == AddressBookFilter::Recent,
                Message::SelectAddressBookFilter(AddressBookFilter::Recent),
            ),
        ]
        .spacing(7);
        if !groups.is_empty() {
            address_book_nav = address_book_nav.push(nav_caption("Группы"));
            for (group, count) in groups {
                address_book_nav = address_book_nav.push(address_book_group_nav_item(
                    group.clone(),
                    count,
                    self.address_book_filter == AddressBookFilter::Group(group.clone()),
                    Message::SelectAddressBookFilter(AddressBookFilter::Group(group)),
                ));
            }
        }
        if !tags.is_empty() {
            address_book_nav = address_book_nav.push(nav_caption("Метки"));
            for (tag, count) in tags {
                address_book_nav = address_book_nav.push(address_book_nav_item(
                    icondata::LuTag,
                    format!("#{tag}"),
                    count,
                    self.address_book_filter == AddressBookFilter::Tag(tag.clone()),
                    Message::SelectAddressBookFilter(AddressBookFilter::Tag(tag)),
                ));
            }
        }

        let search_panel = container(
            row![
                lucide_icon(icondata::LuSearch, 20.0, MUTED),
                address_book_filter_badge(&self.address_book_filter, ui_language),
                text_input(
                    tr(ui_language, TextKey::AddressBookSearchPlaceholder),
                    &self.device_filter
                )
                .on_input(Message::DeviceFilterChanged)
                .padding(10)
                .style(input_style)
                .width(Fill),
                text(address_book_count_summary(
                    ui_language,
                    visible_contact_count,
                    self.store.contacts.len()
                ))
                .size(11)
                .color(MUTED),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding([8, 12])
        .width(Fill)
        .style(card_style);

        let right_content = column![account_panel, search_panel, directory_body].spacing(14);
        let nav_panel = container(address_book_nav)
            .padding(12)
            .width(Length::Fixed(230.0))
            .style(card_style);

        if self.main_window_size.width >= 980.0 {
            row![nav_panel, container(right_content).width(Fill)]
                .spacing(14)
                .into()
        } else {
            column![nav_panel.width(Fill), right_content]
                .spacing(14)
                .into()
        }
    }

    fn address_book_sync_banner(&self) -> Element<'_, Message> {
        let ui_language = self.ui_language();
        if self.address_book_signed_in {
            let sync_button: Element<'_, Message> = if self.address_book_busy {
                button(lucide_icon(icondata::LuRefreshCw, 17.0, MUTED))
                    .padding([7, 11])
                    .style(quiet_button)
                    .into()
            } else {
                icon_action(
                    icondata::LuRefreshCw,
                    tr(ui_language, TextKey::AddressBookSync),
                    Message::SyncAddressBook,
                    false,
                )
            };

            let status = if self.address_book_status.is_empty() {
                if self.store.address_book_last_sync_unix == 0 {
                    tr(ui_language, TextKey::AddressBookSyncRestored)
                } else {
                    tr(ui_language, TextKey::AddressBookSyncAvailable)
                }
            } else {
                &self.address_book_status
            };

            container(
                column![
                    row![
                        lucide_icon(icondata::LuCloud, 18.0, ACCENT),
                        column![
                            text(tr(ui_language, TextKey::AddressBookSyncEnabled)).size(16),
                            text(format!("Аккаунт: {}", self.address_book_account.trim()))
                                .size(11)
                                .color(MUTED),
                        ]
                        .spacing(2)
                        .width(Fill),
                        sync_button,
                        icon_action(
                            icondata::LuBadgeCheck,
                            tr(ui_language, TextKey::AddressBookRefreshEntitlements),
                            Message::RefreshCurrentUser,
                            false,
                        ),
                        icon_action(
                            icondata::LuLogOut,
                            tr(ui_language, TextKey::AddressBookSignOutCloud),
                            Message::SignOutAddressBook,
                            true,
                        ),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    account_entitlement_badges(&self.account_entitlements),
                    text(status).size(12).color(MUTED),
                ]
                .spacing(8),
            )
            .padding(12)
            .style(subtle_panel)
            .into()
        } else {
            let can_sign_in = !self.address_book_busy
                && !self.address_book_account.trim().is_empty()
                && !self.address_book_password.is_empty();
            let sign_in_button = if can_sign_in {
                button(label_with_icon(
                    tr(ui_language, TextKey::AddressBookSignIn),
                    icondata::LuArrowRight,
                    Color::WHITE,
                ))
                .on_press(Message::SignInAddressBook)
                .style(accent_button)
            } else if self.address_book_busy {
                button(tr(ui_language, TextKey::AddressBookSigningIn)).style(accent_button)
            } else {
                button(label_with_icon(
                    tr(ui_language, TextKey::AddressBookSignIn),
                    icondata::LuArrowRight,
                    Color::WHITE,
                ))
                .style(accent_button)
            };
            let sso_button: Element<'_, Message> = if self.login_options_busy {
                button("SSO…").padding([9, 12]).style(quiet_button).into()
            } else {
                button("SSO")
                    .on_press(Message::RefreshLoginOptions)
                    .padding([9, 12])
                    .style(quiet_button)
                    .into()
            };
            let yandex_button: Element<'_, Message> =
                if has_login_provider(&self.login_options, "yandex")
                    && !self.address_book_busy
                    && self.oidc_code.is_none()
                {
                    button(label_with_icon(
                        tr(ui_language, TextKey::AddressBookYandex),
                        icondata::LuExternalLink,
                        Color::WHITE,
                    ))
                    .on_press(Message::StartYandexOidc)
                    .padding([9, 14])
                    .style(accent_button)
                    .into()
                } else if self.oidc_code.is_some() {
                    button(tr(ui_language, TextKey::AddressBookWaitingYandex))
                        .padding([9, 14])
                        .style(quiet_button)
                        .into()
                } else {
                    button(tr(ui_language, TextKey::AddressBookYandex))
                        .padding([9, 14])
                        .style(quiet_button)
                        .into()
                };
            let cancel_oidc_button: Element<'_, Message> = if self.oidc_code.is_some() {
                button(tr(ui_language, TextKey::AddressBookCancel))
                    .on_press(Message::CancelYandexOidc)
                    .padding([9, 12])
                    .style(danger_text_button)
                    .into()
            } else {
                Space::new().width(Length::Shrink).into()
            };
            let status = if self.address_book_status.is_empty() {
                tr(ui_language, TextKey::AddressBookLocalWorks)
            } else {
                &self.address_book_status
            };

            let identity_row = row![
                lucide_icon(icondata::LuHardDrive, 18.0, ACCENT),
                column![
                    text(tr(ui_language, TextKey::AddressBookLocalTitle)).size(16),
                    text(status).size(11).color(MUTED),
                ]
                .spacing(2)
                .width(Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center);

            let sign_in_row = row![
                text_input(
                    tr(ui_language, TextKey::AddressBookLoginPlaceholder),
                    &self.address_book_account
                )
                .on_input(Message::AddressBookAccountChanged)
                .padding(9)
                .style(input_style)
                .width(Length::Fixed(190.0)),
                text_input(
                    tr(ui_language, TextKey::AddressBookPasswordPlaceholder),
                    &self.address_book_password
                )
                .on_input(Message::AddressBookPasswordChanged)
                .on_submit(Message::SignInAddressBook)
                .secure(true)
                .padding(9)
                .style(input_style)
                .width(Length::Fixed(190.0)),
                sign_in_button.padding([9, 14]),
                sso_button,
                yandex_button,
                cancel_oidc_button,
            ]
            .spacing(8)
            .align_y(Alignment::Center);

            let content: Element<'_, Message> = if self.main_window_size.width >= 1_320.0 {
                row![identity_row.width(Fill), sign_in_row]
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .into()
            } else {
                column![identity_row, sign_in_row].spacing(8).into()
            };

            container(content).padding(12).style(subtle_panel).into()
        }
    }

    fn settings_card(&self) -> Element<'_, Message> {
        let ui_language = self.ui_language();
        let mut quality_buttons = row![].spacing(8);
        for quality in ConnectionQuality::ALL {
            let selected = self.store.quality == quality;
            quality_buttons = quality_buttons.push(
                button(text(quality.label()).size(13))
                    .on_press(Message::SetQuality(quality))
                    .padding([8, 14])
                    .style(move |theme, status| {
                        if selected {
                            selected_segment(theme, status)
                        } else {
                            quiet_button(theme, status)
                        }
                    }),
            );
        }
        let mut mode_buttons = row![].spacing(8);
        for mode in [
            StreamingMode::Support,
            StreamingMode::Interactive,
            StreamingMode::Game,
        ] {
            let selected = self.config.display.streaming_mode == mode;
            mode_buttons = mode_buttons.push(
                button(text(streaming_mode_label(mode)).size(13))
                    .on_press(Message::SetStreamingMode(mode))
                    .padding([8, 14])
                    .style(move |theme, status| {
                        if selected {
                            selected_segment(theme, status)
                        } else {
                            quiet_button(theme, status)
                        }
                    }),
            );
        }
        let fsr_quality_row = |options: [FsrQualitySetting; 3], current: &Self| {
            let mut row = row![].spacing(8);
            for fsr_quality in options {
                let selected = current.config.display.fsr_quality == fsr_quality;
                row = row.push(
                    button(text(fsr_quality.label()).size(13))
                        .on_press(Message::SetFsrQuality(fsr_quality))
                        .padding([8, 14])
                        .style(move |theme, status| {
                            if selected {
                                selected_segment(theme, status)
                            } else {
                                quiet_button(theme, status)
                            }
                        }),
                );
            }
            row
        };
        let fsr_buttons = column![
            fsr_quality_row(
                [
                    FsrQualitySetting::Off,
                    FsrQualitySetting::Native,
                    FsrQualitySetting::UltraQuality,
                ],
                self,
            ),
            fsr_quality_row(
                [
                    FsrQualitySetting::Quality,
                    FsrQualitySetting::Balanced,
                    FsrQualitySetting::Performance,
                ],
                self,
            ),
        ]
        .spacing(8);

        let permanent_access = container(
            column![
                row![
                    lucide_icon(icondata::LuKeyRound, 16.0, ACCENT),
                    text(tr(ui_language, TextKey::SettingsPermanentPassword)).size(13),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                text(tr(
                    ui_language,
                    TextKey::SettingsPermanentPasswordDescription
                ))
                .size(11)
                .color(MUTED),
                row![
                    text_input(
                        tr(ui_language, TextKey::SettingsPermanentPasswordPlaceholder),
                        &self.permanent_password
                    )
                    .on_input(Message::PermanentPasswordChanged)
                    .on_submit(Message::SavePermanentPassword)
                    .secure(!self.permanent_password_visible)
                    .padding(10)
                    .style(input_style)
                    .width(Fill),
                    button(if self.permanent_password_visible {
                        tr(ui_language, TextKey::HomeHide)
                    } else {
                        tr(ui_language, TextKey::HomeShow)
                    })
                    .on_press(Message::TogglePermanentPasswordVisibility)
                    .padding([9, 12])
                    .style(quiet_button),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                row![
                    text(if self.permanent_password_status.is_empty() {
                        tr(ui_language, TextKey::SettingsTemporaryPasswordRotates)
                    } else {
                        &self.permanent_password_status
                    })
                    .size(11)
                    .color(MUTED)
                    .width(Fill),
                    button(tr(ui_language, TextKey::SettingsDelete))
                        .on_press(Message::ClearPermanentPassword)
                        .padding([8, 12])
                        .style(danger_text_button),
                    button(tr(ui_language, TextKey::SettingsSave))
                        .on_press(Message::SavePermanentPassword)
                        .padding([8, 14])
                        .style(accent_button),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(8),
        )
        .padding(14)
        .width(Fill)
        .style(subtle_panel);

        let incoming = column![
            text(tr(ui_language, TextKey::SettingsIncomingTitle)).size(18),
            text(tr(ui_language, TextKey::SettingsIncomingDescription))
                .size(12)
                .color(MUTED),
            permanent_access,
            container(
                column![
                    checkbox(self.config.security.require_confirmation)
                        .label(tr(ui_language, TextKey::SettingsAlwaysAskConfirmation))
                        .on_toggle(Message::SetRequireConfirmation)
                        .size(16),
                    text(tr(ui_language, TextKey::SettingsAlwaysAskConfirmationHint))
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(5),
            )
            .padding(14)
            .width(Fill)
            .style(subtle_panel),
            container(
                column![
                    text(tr(ui_language, TextKey::SettingsAccessAutoTitle)).size(13),
                    text(tr(ui_language, TextKey::SettingsAccessAutoHint))
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(5),
            )
            .padding(14)
            .width(Fill)
            .style(subtle_panel),
        ]
        .spacing(10);

        let permissions = column![
            text(tr(ui_language, TextKey::SettingsPermissionsTitle)).size(18),
            text(tr(ui_language, TextKey::SettingsPermissionsDescription))
                .size(12)
                .color(MUTED),
            container(
                column![
                    checkbox(self.config.security.allow_keyboard_mouse)
                        .label(tr(ui_language, TextKey::SettingsKeyboardMouse))
                        .on_toggle(Message::SetAllowKeyboardMouse)
                        .size(16),
                    text(tr(ui_language, TextKey::SettingsKeyboardMouseHint))
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(5),
            )
            .padding(14)
            .width(Fill)
            .style(subtle_panel),
            container(
                column![
                    checkbox(self.config.security.allow_clipboard)
                        .label(tr(ui_language, TextKey::SettingsSharedClipboard))
                        .on_toggle(Message::SetAllowClipboard)
                        .size(16),
                    text(tr(ui_language, TextKey::SettingsSharedClipboardHint))
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(5),
            )
            .padding(14)
            .width(Fill)
            .style(subtle_panel),
        ]
        .spacing(10);

        let outgoing = column![
            text(tr(ui_language, TextKey::SettingsOutgoingTitle)).size(18),
            text(tr(ui_language, TextKey::SettingsOutgoingDescription))
                .size(12)
                .color(MUTED),
            container(
                column![
                    text(tr(ui_language, TextKey::SettingsImageQuality)).size(13),
                    quality_buttons,
                    text(tr(ui_language, TextKey::SettingsQualityHint))
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(8),
            )
            .padding(14)
            .width(Fill)
            .style(subtle_panel),
            container(
                column![
                    text(tr(ui_language, TextKey::SettingsStreamingMode)).size(13),
                    mode_buttons,
                    text(streaming_mode_hint(
                        self.config.display.streaming_mode,
                        ui_language
                    ))
                    .size(11)
                    .color(MUTED),
                ]
                .spacing(8),
            )
            .padding(14)
            .width(Fill)
            .style(subtle_panel),
            container(
                column![
                    text(tr(ui_language, TextKey::SettingsFsrUpscale)).size(13),
                    fsr_buttons,
                    text(tr(ui_language, TextKey::SettingsFsrHint))
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(8),
            )
            .padding(14)
            .width(Fill)
            .style(subtle_panel),
            container(
                column![
                    checkbox(self.store.audio_enabled)
                        .label(tr(ui_language, TextKey::SettingsPlayRemoteAudio))
                        .on_toggle(Message::SetViewerAudioDefault)
                        .size(16),
                    text(tr(ui_language, TextKey::SettingsPlayRemoteAudioHint))
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(5),
            )
            .padding(14)
            .width(Fill)
            .style(subtle_panel),
        ]
        .spacing(10);

        let app_behavior = column![
            text(tr(ui_language, TextKey::SettingsAppBehaviorTitle)).size(18),
            text(tr(ui_language, TextKey::SettingsAppBehaviorDescription))
                .size(12)
                .color(MUTED),
            container(
                column![
                    checkbox(self.store.launch_on_startup)
                        .label(tr(ui_language, TextKey::SettingsLaunchOnStartup))
                        .on_toggle(Message::SetLaunchOnStartup)
                        .size(16),
                    text(tr(ui_language, TextKey::SettingsLaunchOnStartupHint))
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(5),
            )
            .padding(14)
            .width(Fill)
            .style(subtle_panel),
            container(
                column![
                    checkbox(self.store.show_start_menu_shortcut)
                        .label(tr(ui_language, TextKey::SettingsShowStartMenuShortcut))
                        .on_toggle(Message::SetStartMenuShortcut)
                        .size(16),
                    text(tr(ui_language, TextKey::SettingsShowStartMenuShortcutHint))
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(5),
            )
            .padding(14)
            .width(Fill)
            .style(subtle_panel),
            container(
                column![
                    checkbox(self.store.keep_taskbar_icon_on_close)
                        .label(tr(ui_language, TextKey::SettingsKeepTaskbarIcon))
                        .on_toggle(Message::SetKeepTaskbarIconOnClose)
                        .size(16),
                    text(if self.store.keep_taskbar_icon_on_close {
                        tr(ui_language, TextKey::SettingsKeepTaskbarIconHintOn)
                    } else {
                        tr(ui_language, TextKey::SettingsKeepTaskbarIconHintOff)
                    })
                    .size(11)
                    .color(MUTED),
                ]
                .spacing(5),
            )
            .padding(14)
            .width(Fill)
            .style(subtle_panel),
        ]
        .spacing(10);

        let mut language_buttons = row![].spacing(8).align_y(Alignment::Center);
        for language in LanguagePreference::ALL {
            let selected = self.store.language == language;
            language_buttons = language_buttons.push(
                button(text(language_preference_label(language, ui_language)).size(13))
                    .on_press(Message::SetLanguage(language))
                    .padding([8, 14])
                    .style(move |theme, status| {
                        if selected {
                            selected_segment(theme, status)
                        } else {
                            quiet_button(theme, status)
                        }
                    }),
            );
        }
        let language_settings = column![
            text(tr(ui_language, TextKey::LanguageTitle)).size(18),
            text(tr(ui_language, TextKey::LanguageDescription))
                .size(12)
                .color(MUTED),
            container(
                column![
                    language_buttons,
                    text(language_preference_hint(self.store.language, ui_language))
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(8),
            )
            .padding(14)
            .width(Fill)
            .style(subtle_panel),
        ]
        .spacing(10);

        let mut update_channel_buttons = row![].spacing(8).align_y(Alignment::Center);
        for channel in UpdateChannelPreference::ALL {
            let selected = self.store.update_channel == channel;
            update_channel_buttons = update_channel_buttons.push(
                button(text(update_channel_label(channel, ui_language)).size(13))
                    .on_press(Message::SetUpdateChannel(channel))
                    .padding([8, 14])
                    .style(move |theme, status| {
                        if selected {
                            selected_segment(theme, status)
                        } else {
                            quiet_button(theme, status)
                        }
                    }),
            );
        }
        let update_source_fields: Element<'_, Message> = match self.store.update_channel {
            UpdateChannelPreference::Disabled => {
                text(tr(ui_language, TextKey::UpdatesDisabledHint))
                    .size(11)
                    .color(MUTED)
                    .into()
            }
            UpdateChannelPreference::ManifestUrl => text_input(
                tr(ui_language, TextKey::UpdatesManifestPlaceholder),
                &self.store.update_manifest_url,
            )
            .on_input(Message::UpdateManifestUrlChanged)
            .padding(9)
            .style(input_style)
            .width(Fill)
            .into(),
            UpdateChannelPreference::GithubRelease => text_input(
                tr(ui_language, TextKey::UpdatesGithubPlaceholder),
                &self.store.update_github_repo,
            )
            .on_input(Message::UpdateGithubRepoChanged)
            .padding(9)
            .style(input_style)
            .width(Fill)
            .into(),
        };
        let update_settings = column![
            text(tr(ui_language, TextKey::UpdatesTitle)).size(18),
            text(tr(ui_language, TextKey::UpdatesDescription))
                .size(12)
                .color(MUTED),
            container(
                column![
                    update_channel_buttons,
                    text(update_channel_hint(self.store.update_channel, ui_language))
                        .size(11)
                        .color(MUTED),
                    update_source_fields,
                    self.update_status_panel(),
                ]
                .spacing(9),
            )
            .padding(14)
            .width(Fill)
            .style(subtle_panel),
        ]
        .spacing(10);

        let smart_agent_entitlement_note = commercial_feature_note(
            self.account_entitlements.known,
            self.account_entitlements.smart_agent,
            "Smart Agent",
        )
        .unwrap_or(tr(ui_language, TextKey::SettingsSmartAgentAvailable));

        let smart_agent = column![
            text(tr(ui_language, TextKey::SettingsSmartAgentTitle)).size(18),
            text(tr(ui_language, TextKey::SettingsSmartAgentDescription))
                .size(12)
                .color(MUTED),
            text(smart_agent_entitlement_note).size(11).color(MUTED),
            container(
                column![
                    checkbox(self.store.smart_agent_enabled)
                        .label(tr(ui_language, TextKey::SettingsSmartAgentEnable))
                        .on_toggle(Message::SetSmartAgentEnabled)
                        .size(16),
                    text_input(
                        tr(
                            ui_language,
                            TextKey::SettingsSmartAgentServiceKeyPlaceholder
                        ),
                        &self.store.smart_agent_service_key
                    )
                    .on_input(Message::SmartAgentServiceKeyChanged)
                    .padding(9)
                    .style(input_style),
                    text(if self.smart_agent_status.is_empty() {
                        tr(ui_language, TextKey::SettingsSmartAgentIdleHint)
                    } else {
                        &self.smart_agent_status
                    })
                    .size(11)
                    .color(MUTED),
                ]
                .spacing(5),
            )
            .padding(14)
            .width(Fill)
            .style(subtle_panel),
        ]
        .spacing(10);

        let compatibility = self.compatibility_settings_section();

        let host_status = container(
            row![
                text("\u{25CF}")
                    .size(12)
                    .color(host_state_color(&self.host_state)),
                column![
                    text("\u{0422}\u{0435}\u{043A}\u{0443}\u{0449}\u{0435}\u{0435} \u{0441}\u{043E}\u{0441}\u{0442}\u{043E}\u{044F}\u{043D}\u{0438}\u{0435}").size(12).color(MUTED),
                    text(self.host_state.label()).size(15),
                ]
                .spacing(2)
                .width(Fill),
                if self.host.is_some() {
                    button("\u{041E}\u{0441}\u{0442}\u{0430}\u{043D}\u{043E}\u{0432}\u{0438}\u{0442}\u{044C}")
                        .on_press(Message::StopHosting)
                        .padding([9, 14])
                        .style(danger_text_button)
                } else {
                    button("\u{0417}\u{0430}\u{043F}\u{0443}\u{0441}\u{0442}\u{0438}\u{0442}\u{044C}")
                        .on_press(Message::StartHosting)
                        .padding([9, 14])
                        .style(accent_button)
                },
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding(14)
        .width(Fill)
        .style(status_bar);

        // OS-service hint (Phase 3/4, TZ_HOST_SERVICE.md): hosting inside
        // this process stops the instant it exits, even minimized to tray —
        // a crash, log-off, or explicit kill still ends it. Installing the
        // service keeps access working independently of this process.
        let host_status: Element<'_, Message> = match self.service_hint_state {
            ServiceHintState::Running => host_status.into(),
            ServiceHintState::Installing => column![
                host_status,
                container(text("Установка/запуск службы...").size(12).color(MUTED))
                    .padding(10)
                    .width(Fill)
                    .style(subtle_panel),
            ]
            .spacing(8)
            .into(),
            ServiceHintState::NotInstalled | ServiceHintState::InstalledNotRunning => column![
                host_status,
                container(
                    row![
                        column![
                            text(
                                "Хост работает внутри этого процесса — закрытие/крах остановит доступ."
                            )
                            .size(12),
                            text(
                                "Установите службу, чтобы доступ работал независимо от этого окна."
                            )
                            .size(11)
                            .color(MUTED),
                        ]
                        .spacing(3)
                        .width(Fill),
                        if self.service_hint_state == ServiceHintState::NotInstalled {
                            button("Установить службу")
                                .on_press(Message::InstallHostService)
                                .padding([9, 14])
                                .style(accent_button)
                        } else {
                            button("Запустить службу")
                                .on_press(Message::StartHostService)
                                .padding([9, 14])
                                .style(accent_button)
                        },
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding(14)
                .width(Fill)
                .style(subtle_panel),
            ]
            .spacing(8)
            .into(),
        };

        let wide_settings_content = use_wide_settings_content_layout(self.main_window_size.width);
        let settings_body: Element<'_, Message> = match self.settings_section {
            SettingsSection::Security => {
                let section_content: Element<'_, Message> = if wide_settings_content {
                    row![
                        container(incoming).width(Fill),
                        container(permissions).width(Fill),
                    ]
                    .spacing(24)
                    .into()
                } else {
                    column![
                        container(incoming).width(Fill),
                        container(permissions).width(Fill),
                    ]
                    .spacing(14)
                    .into()
                };
                column![section_content, host_status].spacing(18).into()
            }
            SettingsSection::General => {
                let section_content: Element<'_, Message> = if wide_settings_content {
                    row![
                        container(outgoing).width(Fill),
                        container(column![app_behavior, language_settings].spacing(14)).width(Fill),
                        container(column![update_settings, smart_agent].spacing(14)).width(Fill),
                    ]
                    .spacing(24)
                    .into()
                } else {
                    column![
                        container(outgoing).width(Fill),
                        container(app_behavior).width(Fill),
                        container(language_settings).width(Fill),
                        container(update_settings).width(Fill),
                        container(smart_agent).width(Fill),
                    ]
                    .spacing(14)
                    .into()
                };
                column![section_content, host_status].spacing(18).into()
            }
            SettingsSection::Connection => column![compatibility, host_status].spacing(18).into(),
        };

        let mut settings_menu = column![
            text(tr(ui_language, TextKey::SettingsSectionsTitle))
                .size(12)
                .color(MUTED),
            text(settings_section_hint(self.settings_section, ui_language))
                .size(11)
                .color(MUTED),
        ]
        .spacing(8);
        for section in SettingsSection::ALL {
            let selected = self.settings_section == section;
            settings_menu = settings_menu.push(
                button(
                    row![
                        lucide_icon(section.icon(), 17.0, if selected { ACCENT } else { MUTED }),
                        text(settings_section_label(section, ui_language))
                            .size(14)
                            .width(Fill),
                    ]
                    .spacing(9)
                    .align_y(Alignment::Center),
                )
                .on_press(Message::SelectSettingsSection(section))
                .padding([10, 12])
                .width(Fill)
                .style(move |theme, status| {
                    if selected {
                        selected_nav_button(theme, status)
                    } else {
                        quiet_button(theme, status)
                    }
                }),
            );
        }

        let settings_layout: Element<'_, Message> =
            if use_wide_settings_sidebar_layout(self.main_window_size.width) {
                row![
                    container(settings_menu)
                        .padding(14)
                        .width(Length::Fixed(220.0))
                        .style(subtle_panel),
                    container(settings_body).width(Fill),
                ]
                .spacing(22)
                .align_y(Alignment::Start)
                .into()
            } else {
                column![
                    container(settings_menu)
                        .padding(14)
                        .width(Fill)
                        .style(subtle_panel),
                    container(settings_body).width(Fill),
                ]
                .spacing(16)
                .into()
            };

        container(settings_layout)
            .padding(22)
            .width(Fill)
            .style(card_style)
            .into()
    }

    fn begin_credentials(&mut self) -> Task<Message> {
        let remote_id = normalize_remote_id(&self.remote_id);
        if remote_id.is_empty() {
            self.status = "Введите адрес удалённого устройства".to_owned();
            return Task::none();
        }
        if remote_id == normalize_remote_id(&self.config.local_id) {
            self.status = "Нельзя подключиться к этому же устройству".to_owned();
            return Task::none();
        }
        if let Some(entry) = self
            .viewers
            .values_mut()
            .find(|entry| normalize_remote_id(&entry.remote_id) == remote_id)
        {
            match entry.process.send(ViewerCommand::FocusWindow) {
                Ok(()) => {
                    entry.status = "Окно сессии уже открыто".to_owned();
                    self.status = format!("Сессия {} уже активна", entry.remote_id);
                }
                Err(error) => {
                    entry.status = format!("Не удалось показать окно: {error}");
                    self.status = entry.status.clone();
                }
            }
            return Task::none();
        }
        if !can_start_viewer(self.viewers.len()) {
            self.status = format!("Достигнут лимит активных сессий ({MAX_ACTIVE_VIEWERS})");
            return Task::none();
        }

        self.password.zeroize();
        self.auth_status.clear();
        self.remember_password = false;
        match credential_store::load_password(&remote_id) {
            Ok(Some(password)) => {
                self.password = password;
                self.remember_password = true;
            }
            Ok(None) => {}
            Err(error) => {
                self.auth_status = format!("Не удалось прочитать сохранённый пароль: {error}");
            }
        }
        self.remote_id = remote_id.clone();
        self.auth_remote_id = Some(remote_id);
        self.ensure_auth_window()
    }

    fn submit_credentials(&mut self) -> Task<Message> {
        let Some(remote_id) = self.auth_remote_id.clone() else {
            return Task::none();
        };
        let credential_result = if self.remember_password {
            if self.password.is_empty() {
                credential_store::delete_password(&remote_id)
            } else {
                credential_store::store_password(&remote_id, &self.password)
            }
        } else {
            credential_store::delete_password(&remote_id)
        };
        if let Err(error) = credential_result {
            self.auth_status = format!("Не удалось обновить сохранённый пароль: {error}");
            return Task::none();
        }

        let close = self.close_auth_window();
        self.auth_remote_id = None;
        self.auth_status.clear();
        self.connect();
        close
    }

    fn cancel_credentials(&mut self) {
        self.password.zeroize();
        self.password.clear();
        self.auth_remote_id = None;
        self.auth_status.clear();
        self.remember_password = false;
    }

    fn connect_game(&mut self) {
        let remote_id = normalize_remote_id(&self.game_remote_id);
        if remote_id.is_empty() {
            self.game_connect_status = "Введите ID удалённого устройства".to_owned();
            return;
        }
        if remote_id == normalize_remote_id(&self.config.local_id) {
            self.game_connect_status = "Нельзя подключиться к этому же устройству".to_owned();
            return;
        }

        let credential_result = if self.game_remember_password {
            if self.game_password.is_empty() {
                credential_store::delete_password(&remote_id)
            } else {
                credential_store::store_password(&remote_id, &self.game_password)
            }
        } else {
            credential_store::delete_password(&remote_id)
        };
        if let Err(error) = credential_result {
            self.game_connect_status = format!("Не удалось обновить сохранённый пароль: {error}");
            return;
        }

        self.config.display.streaming_mode = StreamingMode::Game;
        self.config.display.target_fps = 60;
        self.config.display.adaptive_quality = false;
        self.pending_connect_profile = ConnectProfile::Game;
        self.remote_id = remote_id.clone();
        self.password = std::mem::take(&mut self.game_password);
        self.status = format!(
            "Game подключение: {} · {}{}",
            remote_id,
            self.store.game_codec.label(),
            if self.store.game_evrt2_enabled {
                " · EVRT2"
            } else {
                ""
            }
        );
        self.game_connect_status = "Запускаю Game viewer…".to_owned();
        self.connect();
        self.game_remote_id = remote_id;
    }

    fn connect(&mut self) {
        let remote_id = normalize_remote_id(&self.remote_id);
        if remote_id.is_empty() {
            self.password.zeroize();
            self.status = "Введите адрес удалённого устройства".to_owned();
            self.pending_connect_profile = ConnectProfile::Regular;
            return;
        }
        if normalize_remote_id(&remote_id) == normalize_remote_id(&self.config.local_id) {
            self.password.zeroize();
            self.status = "Нельзя подключиться к этому же устройству".to_owned();
            self.pending_connect_profile = ConnectProfile::Regular;
            return;
        }
        let normalized_remote = normalize_remote_id(&remote_id);
        if let Some(entry) = self
            .viewers
            .values_mut()
            .find(|entry| normalize_remote_id(&entry.remote_id) == normalized_remote)
        {
            self.password.zeroize();
            match entry.process.send(ViewerCommand::FocusWindow) {
                Ok(()) => {
                    entry.status = "Окно сессии уже открыто".to_owned();
                    self.status = format!("Сессия {} уже активна", entry.remote_id);
                }
                Err(error) => {
                    entry.status = format!("Не удалось показать окно: {error}");
                    self.status = entry.status.clone();
                }
            }
            self.pending_connect_profile = ConnectProfile::Regular;
            return;
        }
        if !can_start_viewer(self.viewers.len()) {
            self.password.zeroize();
            self.status = format!(
                "Достигнут лимит активных сессий ({MAX_ACTIVE_VIEWERS}). Закройте одну из них"
            );
            self.pending_connect_profile = ConnectProfile::Regular;
            return;
        }

        let game_mode = self.pending_connect_profile == ConnectProfile::Game;
        let game_codec = if game_mode {
            viewer_game_codec(self.store.game_codec)
        } else {
            ViewerGameCodec::Auto
        };
        let game_evrt2_enabled = game_mode && self.store.game_evrt2_enabled;
        let request = ViewerBootstrap::new(remote_id.clone(), self.password.clone())
            .with_audio(self.store.audio_enabled)
            .with_quality(self.store.quality)
            .with_scaling(self.store.scaling)
            .with_game_profile(game_mode, game_codec, game_evrt2_enabled);
        let spawn_result = spawn_viewer(&request);
        drop(request);
        self.password.zeroize();
        match spawn_result {
            Ok(mut process) => {
                let process_id = process.id();
                self.viewer_token = self.viewer_token.wrapping_add(1);
                let session_token = self.viewer_token;
                let Some(stdout) = process.take_stdout() else {
                    self.status = "Viewer запущен без канала состояния".to_owned();
                    let _ = process.disconnect();
                    self.pending_connect_profile = ConnectProfile::Regular;
                    return;
                };
                let stderr = process.take_stderr();
                let completion = Arc::new(AtomicU8::new(if stderr.is_some() { 2 } else { 1 }));

                if let Err(error) = watch_viewer(process_id, stdout, Arc::clone(&completion)) {
                    self.status = format!("Не удалось запустить монитор viewer: {error}");
                    let _ = process.disconnect();
                    self.pending_connect_profile = ConnectProfile::Regular;
                    return;
                }
                if let Some(stderr) = stderr {
                    if let Err(error) =
                        watch_viewer_diagnostics(process_id, stderr, Arc::clone(&completion))
                    {
                        self.status = format!("Viewer запущен без канала диагностики: {error}");
                        finish_viewer_stream(process_id, &completion);
                    }
                }
                self.viewers.insert(
                    process_id,
                    ViewerEntry {
                        remote_id: remote_id.clone(),
                        status: format!("Запуск · {}", self.store.quality.label()),
                        codec: String::new(),
                        latency_ms: None,
                        fps_times_100: 0,
                        input_kbps: 0,
                        dropped_frames: 0,
                        session_seconds: 0,
                        reconnect_count: 0,
                        last_telemetry_at: None,
                        diagnostics_expanded: false,
                        input_enabled: true,
                        audio_enabled: self.store.audio_enabled,
                        game_mode,
                        game_codec,
                        game_evrt2_enabled,
                        clipboard_enabled: self.config.security.allow_clipboard,
                        scaling: self.store.scaling,
                        session_token,
                        ipc_ready: false,
                        heartbeat_sequence: 0,
                        disconnect_requested: false,
                        closed_status_received: false,
                        pending_controls: PendingViewerControls::default(),
                        diagnostics: VecDeque::new(),
                        process,
                    },
                );
                if let Some(entry) = self.viewers.get_mut(&process_id) {
                    entry.status = viewer_launch_status(
                        self.store.quality,
                        game_mode,
                        game_codec,
                        game_evrt2_enabled,
                    );
                }
                schedule_viewer_timeout(
                    process_id,
                    session_token,
                    VIEWER_STARTUP_TIMEOUT,
                    ViewerTimeoutKind::Startup,
                );
                schedule_viewer_liveness_timeout(process_id, session_token, 0);
                self.store.record_recent(&remote_id);
                if let Err(error) = self.store.save_default() {
                    self.pending_connect_profile = ConnectProfile::Regular;
                    self.status = format!("Viewer запущен, но история не сохранена: {error}");
                    return;
                }
                self.status = format!("Открывается сессия для {remote_id}");
                self.pending_connect_profile = ConnectProfile::Regular;
            }
            Err(error) => {
                self.status = format!("Не удалось открыть viewer: {error}");
                self.pending_connect_profile = ConnectProfile::Regular;
            }
        }
    }

    fn start_hosting(&mut self) {
        if self.host.is_some() {
            return;
        }

        self.host_state = HostState::Connecting;
        self.status = "Запускается приём входящих подключений…".to_owned();
        let service = HostService::start(self.config.clone());
        let commands = service.command_sender();
        watch_host(service);
        self.host = Some(HostRuntime { commands });
        self.update_tray_host_item();
    }

    fn stop_hosting(&mut self) {
        if let Some(session) = &self.incoming_session {
            self.store.finish_incoming(
                &session.peer_id,
                session.started_at.elapsed().as_secs(),
                "Остановлено владельцем",
            );
            let _ = self.store.save_default();
        }
        if let Some(runtime) = self.host.take() {
            let _ = runtime.commands.send(HostCommand::SetInputBlocked(false));
            let _ = runtime.commands.send(HostCommand::SetClipboardAllowed(
                self.config.security.allow_clipboard,
            ));
            let _ = runtime.commands.send(HostCommand::Stop);
        }
        self.pending_approval = None;
        self.incoming_accepting = None;
        self.incoming_session = None;
        self.host_state = HostState::Idle;
        self.status = "Приём входящих подключений остановлен".to_owned();
        self.update_tray_host_item();
    }

    fn approve_incoming(&mut self, accept: bool) {
        let Some(pending) = self.pending_approval.take() else {
            return;
        };
        let peer_id = pending.peer_id;
        if let Some(runtime) = &self.host {
            let _ = runtime.commands.send(HostCommand::ApproveIncoming {
                peer_id: peer_id.clone(),
                accept,
            });
            self.incoming_accepting = accept.then_some(AcceptedIncoming {
                peer_id,
                allow_input: pending.allow_input,
                allow_clipboard: pending.allow_clipboard,
            });
            self.status = if accept {
                "Входящее подключение разрешено".to_owned()
            } else {
                "Входящее подключение отклонено".to_owned()
            };
        } else {
            self.incoming_accepting = None;
        }
        self.update_tray_host_item();
    }

    fn reject_incoming_peer(&self, peer_id: String) {
        if let Some(runtime) = &self.host {
            let _ = runtime.commands.send(HostCommand::ApproveIncoming {
                peer_id,
                accept: false,
            });
        }
    }

    fn toggle_incoming_input(&mut self) {
        let Some(session) = self.incoming_session.as_mut() else {
            return;
        };
        if session.disconnect_requested
            || session.pending_input_blocked.is_some()
            || session.pending_clipboard_allowed.is_some()
        {
            return;
        }
        let blocked = !session.input_blocked;
        let clipboard_allowed = session.clipboard_allowed;
        let peer_id = session.peer_id.clone();
        let Some(runtime) = &self.host else {
            self.status = "Приём подключений уже остановлен".to_owned();
            return;
        };
        match runtime.commands.send(HostCommand::SetSessionPermissions {
            peer_id: peer_id.clone(),
            session_id: session.session_id,
            input_blocked: blocked,
            clipboard_allowed,
        }) {
            Ok(()) => {
                session.pending_input_blocked = Some(blocked);
                self.status = format!("Применяются разрешения управления для {peer_id}…");
            }
            Err(error) => {
                self.status = format!("Не удалось изменить удалённое управление: {error}");
            }
        }
        self.update_tray_host_item();
    }

    fn disconnect_incoming(&mut self) {
        let Some(session) = self.incoming_session.as_mut() else {
            return;
        };
        if session.disconnect_requested {
            return;
        }
        let peer_id = session.peer_id.clone();
        let Some(runtime) = &self.host else {
            self.status = "Приём подключений уже остановлен".to_owned();
            return;
        };
        match runtime.commands.send(HostCommand::KickActiveSession) {
            Ok(()) => {
                session.disconnect_requested = true;
                self.status = format!("Отключение входящей сессии {peer_id}…");
            }
            Err(error) => {
                self.status = format!("Не удалось отключить клиента: {error}");
            }
        }
        self.update_tray_host_item();
    }

    fn toggle_incoming_clipboard(&mut self) {
        let Some(session) = self.incoming_session.as_mut() else {
            return;
        };
        if session.disconnect_requested
            || session.pending_input_blocked.is_some()
            || session.pending_clipboard_allowed.is_some()
        {
            return;
        }
        let allowed = !session.clipboard_allowed;
        let input_blocked = session.input_blocked;
        let peer_id = session.peer_id.clone();
        let Some(runtime) = &self.host else {
            self.status = "Приём подключений уже остановлен".to_owned();
            return;
        };
        match runtime.commands.send(HostCommand::SetSessionPermissions {
            peer_id: peer_id.clone(),
            session_id: session.session_id,
            input_blocked,
            clipboard_allowed: allowed,
        }) {
            Ok(()) => {
                session.pending_clipboard_allowed = Some(allowed);
                self.status = format!("Применяется разрешение буфера обмена для {peer_id}…");
            }
            Err(error) => {
                self.status = format!("Не удалось изменить доступ к буферу обмена: {error}");
            }
        }
        self.update_tray_host_item();
    }

    fn copy_incoming_peer(&mut self) {
        let peer_id = self
            .pending_approval
            .as_ref()
            .map(|pending| pending.peer_id.clone())
            .or_else(|| {
                self.incoming_session
                    .as_ref()
                    .map(|session| session.peer_id.clone())
            })
            .or_else(|| {
                self.incoming_accepting
                    .as_ref()
                    .map(|incoming| incoming.peer_id.clone())
            });
        if let Some(peer_id) = peer_id {
            self.copy_to_clipboard(peer_id, "ID удалённого устройства скопирован", false);
        }
    }

    fn regenerate_password(&mut self) {
        self.config.local_password.zeroize();
        self.config.local_password = generate_numeric_token(6);
        self.password_visible = false;
        self.last_temp_password_rotation = Instant::now();
        self.config.save();
        if let Some(runtime) = &self.host {
            let _ = runtime
                .commands
                .send(HostCommand::Reconfigure(self.config.clone()));
        }
        self.status = "Пароль доступа обновлён".to_owned();
    }

    /// Re-queries `service_hint_state` at most every few seconds — on
    /// Linux/macOS the query shells out to systemctl/launchctl, which is too
    /// slow to run on every `UiTick`.
    fn tick_service_hint(&mut self) {
        if Instant::now() < self.service_hint_next_check {
            return;
        }
        self.service_hint_state = query_service_hint_state();
        self.service_hint_next_check = Instant::now() + Duration::from_secs(8);
    }

    /// One-click install (Windows: single UAC prompt via `ShellExecuteW`
    /// runas; Linux/macOS: no elevation needed, systemd --user / launchd
    /// LaunchAgent are per-user).
    fn request_install_service(&mut self) {
        self.service_hint_state = ServiceHintState::Installing;
        #[cfg(windows)]
        {
            if let Err(error) =
                evertydesk_core::winservice::relaunch_elevated(&["--install-service"])
            {
                self.status = format!("Установка службы: {error}");
            }
        }
        #[cfg(unix)]
        {
            if let Err(error) = evertydesk_core::host_service_unix::install_service() {
                self.status = format!("Установка службы: {error}");
            }
        }
        self.service_hint_next_check = Instant::now() + Duration::from_secs(2);
    }

    fn request_start_service(&mut self) {
        self.service_hint_state = ServiceHintState::Installing;
        #[cfg(windows)]
        {
            if let Err(error) = evertydesk_core::winservice::relaunch_elevated(&["--start-service"])
            {
                self.status = format!("Запуск службы: {error}");
            }
        }
        #[cfg(unix)]
        {
            if let Err(error) = evertydesk_core::host_service_unix::start_installed_service() {
                self.status = format!("Запуск службы: {error}");
            }
        }
        self.service_hint_next_check = Instant::now() + Duration::from_secs(2);
    }

    /// Background check, at most every [`UPDATE_CHECK_INTERVAL`] — never
    /// runs at all if `EVERTYDESK_UPDATE_URL` isn't set. Does nothing while
    /// a check/download is already in flight or a result is on screen.
    fn tick_update_check(&mut self) {
        if Instant::now() < self.update_next_check {
            return;
        }
        self.update_next_check = Instant::now() + UPDATE_CHECK_INTERVAL;
        if matches!(self.update_state, UpdateState::Idle | UpdateState::UpToDate) {
            if update_source_from_store(&self.store).is_none() {
                return;
            }
            self.check_for_updates();
        }
    }

    fn check_for_updates(&mut self) {
        let Some(source) = update_source_from_store(&self.store) else {
            self.update_state = UpdateState::Error(
                tr(self.ui_language(), TextKey::UpdatesChannelNotConfigured).to_owned(),
            );
            return;
        };
        self.update_state = UpdateState::Checking;
        spawn_check_for_update(source, env!("CARGO_PKG_VERSION").to_owned());
    }

    fn download_update(&mut self) {
        let UpdateState::Available(manifest) = &self.update_state else {
            return;
        };
        let manifest = manifest.clone();
        self.update_state = UpdateState::Downloading(manifest.clone());
        spawn_download_update(manifest, update_download_dir());
    }

    fn install_update(&mut self) {
        let UpdateState::ReadyToInstall(path) = &self.update_state else {
            return;
        };
        match updater::launch_installer(path) {
            Ok(()) => self.status = "Установщик обновления запущен".to_owned(),
            Err(error) => self.status = format!("Не удалось запустить установщик: {error}"),
        }
    }

    fn ui_language(&self) -> UiLanguage {
        UiLanguage::from_preference(self.store.language)
    }

    fn update_status_panel(&self) -> Element<'_, Message> {
        let language = self.ui_language();
        let current_version = env!("CARGO_PKG_VERSION");
        let content: Element<'_, Message> = match &self.update_state {
            UpdateState::Idle => row![
                column![
                    text(tr(language, TextKey::UpdatesTitle)).size(13),
                    text(format!(
                        "{}: {current_version}",
                        tr(language, TextKey::UpdatesCurrentVersion)
                    ))
                    .size(11)
                    .color(MUTED),
                ]
                .spacing(3)
                .width(Fill),
                button(tr(language, TextKey::UpdatesCheck))
                    .on_press(Message::CheckForUpdates)
                    .padding([9, 14])
                    .style(accent_button),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
            UpdateState::Checking => text(tr(language, TextKey::UpdatesChecking))
                .size(12)
                .color(MUTED)
                .into(),
            UpdateState::UpToDate => row![
                text(format!(
                    "{} ({current_version})",
                    tr(language, TextKey::UpdatesUpToDate)
                ))
                .size(12)
                .color(MUTED)
                .width(Fill),
                button(tr(language, TextKey::UpdatesCheckAgain))
                    .on_press(Message::CheckForUpdates)
                    .padding([9, 14])
                    .style(quiet_button),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
            UpdateState::Available(manifest) => row![
                column![
                    text(format!(
                        "{}: {}",
                        tr(language, TextKey::UpdatesAvailable),
                        manifest.version
                    ))
                    .size(13),
                    if manifest.notes.is_empty() {
                        text("").size(11)
                    } else {
                        text(manifest.notes.clone()).size(11).color(MUTED)
                    },
                ]
                .spacing(3)
                .width(Fill),
                button(tr(language, TextKey::UpdatesDownloadAndVerify))
                    .on_press(Message::DownloadUpdate)
                    .padding([9, 14])
                    .style(accent_button),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
            UpdateState::Downloading(manifest) => text(format!(
                "{} {}...",
                tr(language, TextKey::UpdatesDownloading),
                manifest.version
            ))
            .size(12)
            .color(MUTED)
            .into(),
            UpdateState::ReadyToInstall(_) => row![
                text(tr(language, TextKey::UpdatesReadyToInstall))
                    .size(12)
                    .width(Fill),
                button(tr(language, TextKey::UpdatesInstall))
                    .on_press(Message::InstallUpdate)
                    .padding([9, 14])
                    .style(accent_button),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
            UpdateState::Error(error) => row![
                text(error.clone()).size(12).color(MUTED).width(Fill),
                button(tr(language, TextKey::UpdatesRetry))
                    .on_press(Message::CheckForUpdates)
                    .padding([9, 14])
                    .style(quiet_button),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
        };
        container(content)
            .padding(14)
            .width(Fill)
            .style(subtle_panel)
            .into()
    }

    fn handle_updater_event(&mut self, event: UpdaterEvent) {
        match event {
            UpdaterEvent::Checked(Ok(Some(manifest))) => {
                self.update_state = UpdateState::Available(manifest);
            }
            UpdaterEvent::Checked(Ok(None)) => {
                self.update_state = UpdateState::UpToDate;
            }
            UpdaterEvent::Checked(Err(error)) => {
                self.update_state = UpdateState::Error(error);
            }
            UpdaterEvent::Downloaded(Ok(path)) => {
                self.update_state = UpdateState::ReadyToInstall(path);
            }
            UpdaterEvent::Downloaded(Err(error)) => {
                self.update_state = UpdateState::Error(error);
            }
        }
    }

    fn rotate_temporary_password_if_due(&mut self) {
        if !should_rotate_temporary_password(
            self.last_temp_password_rotation.elapsed(),
            self.pending_approval.is_some(),
            self.incoming_accepting.is_some(),
            self.incoming_session.is_some(),
        ) {
            return;
        }
        self.config.local_password.zeroize();
        self.config.local_password = generate_numeric_token(6);
        self.password_visible = false;
        self.last_temp_password_rotation = Instant::now();
        self.config.save();
        if let Some(runtime) = &self.host {
            let _ = runtime
                .commands
                .send(HostCommand::Reconfigure(self.config.clone()));
        }
        self.status = "Одноразовый пароль автоматически обновлён".to_owned();
    }

    fn save_permanent_password(&mut self) {
        let password = sanitize_permanent_password(&self.permanent_password);
        if password.trim().is_empty() {
            self.clear_permanent_password();
            return;
        }
        match credential_store::store_permanent_password(&password) {
            Ok(()) => {
                self.config.permanent_password.zeroize();
                self.config.permanent_password = password.clone();
                self.permanent_password = password;
                self.permanent_password_visible = false;
                if let Some(runtime) = &self.host {
                    let _ = runtime
                        .commands
                        .send(HostCommand::Reconfigure(self.config.clone()));
                }
                self.permanent_password_status =
                    "Постоянный пароль сохранён в системном хранилище".to_owned();
                self.status = "Постоянный пароль для входящих подключений применён".to_owned();
            }
            Err(error) => {
                self.permanent_password_status =
                    format!("Не удалось сохранить постоянный пароль: {error}");
            }
        }
    }

    fn clear_permanent_password(&mut self) {
        let result = credential_store::delete_permanent_password();
        self.permanent_password.zeroize();
        self.permanent_password.clear();
        self.config.permanent_password.zeroize();
        self.config.permanent_password.clear();
        self.permanent_password_visible = false;
        if let Some(runtime) = &self.host {
            let _ = runtime
                .commands
                .send(HostCommand::Reconfigure(self.config.clone()));
        }
        match result {
            Ok(()) => {
                self.permanent_password_status = "Постоянный пароль удалён".to_owned();
                self.status = "Постоянный пароль отключён; остаётся одноразовый пароль".to_owned();
            }
            Err(error) => {
                self.permanent_password_status =
                    format!("Пароль отключён в приложении, но хранилище вернуло ошибку: {error}");
            }
        }
    }

    fn copy_to_clipboard(&mut self, value: String, success: &str, sensitive: bool) {
        self.clipboard_token = self.clipboard_token.wrapping_add(1);
        let token = self.clipboard_token;
        let fingerprint = clipboard_fingerprint(&value);
        self.status =
            match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(value)) {
                Ok(()) => {
                    if sensitive {
                        schedule_clipboard_expiry(token, fingerprint);
                    }
                    success.to_owned()
                }
                Err(error) => format!("Не удалось записать в буфер обмена: {error}"),
            };
    }

    fn save_contact(&mut self) {
        let tags = parse_contact_tags(&self.contact_tags);
        match self.store.upsert_contact_details_with_tags(
            &self.contact_name,
            &self.remote_id,
            &self.contact_group,
            &self.contact_note,
            &tags,
        ) {
            Ok(()) => {
                let edited = self.editing_contact_id.is_some();
                self.selected_contact_id = Some(normalize_remote_id(&self.remote_id));
                self.clear_contact_form();
                self.persist_store(if edited {
                    "Контакт обновлён"
                } else {
                    "Контакт добавлен в адресную книгу"
                });
            }
            Err(error) => self.status = error.to_string(),
        }
    }

    fn begin_contact_edit(&mut self, remote_id: &str) {
        let Some(contact) = self
            .store
            .contacts
            .iter()
            .find(|contact| remote_ids_match(&contact.remote_id, remote_id))
            .cloned()
        else {
            return;
        };
        self.remote_id = contact.remote_id.clone();
        self.contact_name = contact.name;
        self.contact_group = contact.group;
        self.contact_tags = format_contact_tags(&contact.tags);
        self.contact_note = contact.note;
        self.editing_contact_id = Some(contact.remote_id);
        self.selected_contact_id = self.editing_contact_id.clone();
        self.contact_form_expanded = true;
        self.status = "Редактирование контакта".to_owned();
    }

    fn clear_contact_form(&mut self) {
        self.contact_name.clear();
        self.contact_group.clear();
        self.contact_tags.clear();
        self.contact_note.clear();
        self.editing_contact_id = None;
        self.contact_form_expanded = false;
    }

    fn refresh_selected_contact_visibility(&mut self) {
        let recent_ids = normalized_recent_ids(&self.store.recent);
        let text_filter = self.device_filter.trim().to_lowercase();
        self.selected_contact_id = selected_contact_after_filter_change(
            &self.store.contacts,
            self.selected_contact_id.as_deref(),
            &self.address_book_filter,
            &recent_ids,
            &text_filter,
        );
    }

    fn add_contact_tag(&mut self, tag: &str) {
        let mut tags = parse_contact_tags(&self.contact_tags);
        tags.push(tag.trim().to_owned());
        self.contact_tags = format_contact_tags(&normalize_contact_tags(&tags));
    }

    fn persist_store(&mut self, success: &str) {
        self.status = match self.store.save_default() {
            Ok(()) => success.to_owned(),
            Err(error) => format!("Не удалось сохранить данные: {error}"),
        };
    }

    fn save_runtime_settings(&mut self, success: &str) {
        self.config.save();
        if let Some(runtime) = &self.host {
            let _ = runtime
                .commands
                .send(HostCommand::Reconfigure(self.config.clone()));
        }
        self.status = success.to_owned();
    }

    fn sign_in_address_book(&mut self) {
        if self.address_book_busy {
            return;
        }
        self.clear_oidc_flow();
        let account = self.address_book_account.trim().to_owned();
        if account.is_empty() || self.address_book_password.is_empty() {
            self.address_book_status = "Укажите логин и пароль или токен".to_owned();
            return;
        }
        let password = std::mem::take(&mut self.address_book_password);
        self.address_book_busy = true;
        self.address_book_status = "Вход и загрузка контактов…".to_owned();
        if let Err(error) = spawn_address_book_sign_in(
            self.config.server.api_url.clone(),
            account,
            password,
            self.config.local_id.clone(),
            self.config.ui.agent_machine_id.clone(),
        ) {
            self.address_book_busy = false;
            self.address_book_status = error;
        }
    }

    fn refresh_login_options(&mut self) -> Task<Message> {
        if self.login_options_busy {
            return Task::none();
        }
        self.login_options_busy = true;
        self.address_book_status = "Проверяю доступные способы входа…".to_owned();
        let api_url = self.config.server.api_url.clone();
        Task::perform(
            async move { evertydesk_core::address_book::login_options(&api_url) },
            Message::LoginOptionsLoaded,
        )
    }

    fn start_yandex_oidc(&mut self) -> Task<Message> {
        if self.address_book_busy || self.oidc_code.is_some() {
            return Task::none();
        }
        if !has_login_provider(&self.login_options, "yandex") {
            self.address_book_status =
                "Сначала проверьте SSO: сервер должен вернуть oidc/yandex".to_owned();
            return Task::none();
        }
        self.address_book_busy = true;
        self.address_book_status = "Запрашиваю ссылку Яндекс…".to_owned();
        let api_url = self.config.server.api_url.clone();
        let local_id = self.config.local_id.clone();
        let uuid = self.config.ui.agent_machine_id.clone();
        Task::perform(
            async move {
                evertydesk_core::address_book::oidc_auth(&api_url, "yandex", &local_id, &uuid)
            },
            Message::YandexOidcStarted,
        )
    }

    fn tick_oidc_login(&mut self) -> Option<Task<Message>> {
        let code = self.oidc_code.clone()?;
        if self.oidc_poll_busy {
            return None;
        }
        if self
            .oidc_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.clear_oidc_flow();
            self.address_book_status = "Сессия входа через Яндекс истекла".to_owned();
            return None;
        }
        if self
            .oidc_last_poll
            .is_some_and(|last_poll| last_poll.elapsed() < Duration::from_secs(1))
        {
            return None;
        }
        self.oidc_last_poll = Some(Instant::now());
        self.oidc_poll_busy = true;
        let api_url = self.config.server.api_url.clone();
        Some(Task::perform(
            async move { evertydesk_core::address_book::oidc_auth_query(&api_url, &code) },
            Message::YandexOidcPolled,
        ))
    }

    fn clear_oidc_flow(&mut self) {
        self.oidc_code = None;
        self.oidc_last_poll = None;
        self.oidc_deadline = None;
        self.oidc_poll_busy = false;
    }

    fn discover_server_settings(&mut self) -> Task<Message> {
        if self.server_discovery_busy {
            return Task::none();
        }
        self.server_discovery_busy = true;
        self.server_discovery_status = "Запрос параметров подключения…".to_owned();
        let api_url = self.config.server.api_url.clone();
        let access_token = self.address_book_access_token.clone();
        Task::perform(
            async move {
                evertydesk_core::address_book::public_connection(
                    &api_url,
                    (!access_token.trim().is_empty()).then_some(access_token.as_str()),
                )
            },
            Message::ServerDiscoveryFinished,
        )
    }

    fn apply_discovered_server_settings(&mut self, discovered: ServerConfig) {
        self.config.server.api_url =
            server_field_or_default(discovered.api_url, ServerConfig::default().api_url);
        self.config.server.id_server =
            server_field_or_default(discovered.id_server, ServerConfig::default().id_server);
        self.config.server.relay_server = server_field_or_default(
            discovered.relay_server,
            ServerConfig::default().relay_server,
        );
        if !discovered.public_key.trim().is_empty() {
            self.config.server.public_key =
                server_field_or_default(discovered.public_key, ServerConfig::default().public_key);
        }
        self.save_runtime_settings("Параметры серверов сохранены");
    }

    fn refresh_current_user_entitlements(&self) -> Task<Message> {
        if !self.address_book_signed_in || self.address_book_access_token.trim().is_empty() {
            return Task::none();
        }
        let api_url = self.config.server.api_url.clone();
        let access_token = self.address_book_access_token.clone();
        Task::perform(
            async move {
                evertydesk_core::address_book::current_user(&api_url, &access_token)
                    .map(|user| current_user_entitlements(&user))
            },
            Message::CurrentUserRefreshed,
        )
    }

    fn sync_address_book(&mut self) {
        if self.address_book_busy {
            return;
        }
        if !self.address_book_signed_in || self.address_book_access_token.trim().is_empty() {
            self.address_book_signed_in = false;
            self.address_book_status = "Войдите в аккаунт для синхронизации".to_owned();
            return;
        }
        self.address_book_busy = true;
        self.address_book_status = "Синхронизация контактов…".to_owned();
        if let Err(error) = spawn_address_book_sync(
            self.config.server.api_url.clone(),
            self.address_book_access_token.clone(),
            self.store.address_book_guid.clone(),
        ) {
            self.address_book_busy = false;
            self.address_book_status = error;
        }
    }

    fn sign_out_address_book(&mut self) {
        let account = self.address_book_account.trim().to_owned();
        let access_token = std::mem::take(&mut self.address_book_access_token);
        if !access_token.trim().is_empty() {
            let _ = spawn_address_book_logout(
                self.config.server.api_url.clone(),
                access_token,
                self.config.local_id.clone(),
            );
        }
        let delete_error = if account.is_empty() {
            None
        } else {
            credential_store::delete_account_token(&account).err()
        };
        self.address_book_password.zeroize();
        self.address_book_signed_in = false;
        self.address_book_busy = false;
        self.account_entitlements_status.clear();
        self.account_entitlements = AccountEntitlements::default();
        self.store.address_book_guid.clear();
        self.store.address_book_account = account;
        self.address_book_status = delete_error
            .map(|error| format!("Сеанс завершён, но хранилище сообщило ошибку: {error}"))
            .unwrap_or_else(|| "Вы вышли из облачной адресной книги".to_owned());
        self.persist_store("Вы вышли из облачной адресной книги");
    }

    fn handle_address_book_event(&mut self, event: AddressBookEvent) {
        self.address_book_busy = false;
        match event {
            AddressBookEvent::SignedIn {
                account,
                access_token,
                guid,
                contacts,
            } => {
                let store_warning =
                    credential_store::store_account_token(&account, &access_token).err();
                self.address_book_access_token.zeroize();
                self.address_book_access_token = access_token;
                spawn_current_user_refresh(
                    self.config.server.api_url.clone(),
                    self.address_book_access_token.clone(),
                );
                self.address_book_signed_in = true;
                self.address_book_account = account.clone();
                self.store.address_book_account = account;
                self.store.address_book_guid = guid;
                let received = contacts.len();
                let added = self
                    .store
                    .merge_cloud_contacts(cloud_contact_rows(contacts));
                self.address_book_status = match store_warning {
                    Some(error) => format!(
                        "Получено {received}, новых {added}. Не удалось безопасно сохранить вход: {error}"
                    ),
                    None => format!("Синхронизировано: {received}, новых контактов: {added}"),
                };
                self.persist_store("Облачная адресная книга синхронизирована");
            }
            AddressBookEvent::Synced { guid, contacts } => {
                self.store.address_book_guid = guid;
                let received = contacts.len();
                let added = self
                    .store
                    .merge_cloud_contacts(cloud_contact_rows(contacts));
                self.address_book_status =
                    format!("Синхронизировано: {received}, новых контактов: {added}");
                self.persist_store("Облачная адресная книга синхронизирована");
            }
            AddressBookEvent::LoggedOut(result) => {
                if let Err(error) = result {
                    self.address_book_status = format!(
                        "Локальный выход выполнен, но сервер не подтвердил отзыв токена: {}",
                        sanitize_address_book_error(&error)
                    );
                }
            }
            AddressBookEvent::Failed(error) => {
                if error.contains("HTTP 401") || error.contains("HTTP 403") {
                    let _ = credential_store::delete_account_token(&self.address_book_account);
                    self.address_book_access_token.zeroize();
                    self.address_book_signed_in = false;
                    self.store.address_book_guid.clear();
                    let _ = self.store.save_default();
                    self.address_book_status =
                        "Срок входа истёк. Введите пароль и войдите снова.".to_owned();
                } else {
                    self.address_book_status = format!(
                        "Ошибка синхронизации: {}",
                        sanitize_address_book_error(&error)
                    );
                }
            }
        }
    }

    fn save_security_settings(&mut self) {
        self.config.save();
        if let Some(runtime) = &self.host {
            let _ = runtime
                .commands
                .send(HostCommand::Reconfigure(self.config.clone()));
        }
        self.status = "Настройки безопасности применены".to_owned();
    }

    fn disconnect(&mut self, process_id: u32) {
        if let Some(entry) = self.viewers.get_mut(&process_id) {
            if entry.disconnect_requested {
                self.status = format!("Отключение {} уже выполняется…", entry.remote_id);
                return;
            }
            match entry.process.disconnect() {
                Ok(()) => {
                    entry.disconnect_requested = true;
                    entry.status = "Отключение…".to_owned();
                    self.status = format!("Отключение {}…", entry.remote_id);
                    schedule_viewer_timeout(
                        process_id,
                        entry.session_token,
                        VIEWER_SHUTDOWN_TIMEOUT,
                        ViewerTimeoutKind::Shutdown,
                    );
                }
                Err(error) => self.status = format!("Ошибка отключения: {error}"),
            }
        }
    }

    fn begin_viewer_shutdown(&mut self) {
        for entry in self.viewers.values_mut() {
            if !entry.disconnect_requested && entry.process.disconnect().is_ok() {
                entry.disconnect_requested = true;
            }
        }
    }

    fn handle_process_event(&mut self, event: ProcessEvent) {
        match event {
            ProcessEvent::Status { process_id, status } => {
                if let Some(entry) = self.viewers.get_mut(&process_id) {
                    entry.ipc_ready = true;
                }
                if let ViewerStatus::SessionSummary {
                    remote_id,
                    session_seconds,
                    reconnect_count,
                    end_reason,
                } = &status
                {
                    self.store.update_recent_summary(
                        remote_id,
                        *session_seconds,
                        *reconnect_count,
                        end_reason,
                    );
                    if let Err(error) = self.store.save_default() {
                        self.status = format!("Не удалось сохранить итоги сессии: {error}");
                    }
                }
                let ui_language = self.ui_language();
                if let Some(entry) = self.viewers.get_mut(&process_id) {
                    if !matches!(
                        &status,
                        ViewerStatus::ControlApplied { .. }
                            | ViewerStatus::ControlState { .. }
                            | ViewerStatus::Codec { .. }
                            | ViewerStatus::Heartbeat { .. }
                    ) {
                        entry.status = status_text(&status, ui_language);
                    }
                    match status {
                        ViewerStatus::Starting => reset_viewer_telemetry(entry),
                        ViewerStatus::Codec { name } => {
                            entry.codec = sanitize_diagnostic_value(&name, 48);
                        }
                        ViewerStatus::Latency { milliseconds } => {
                            entry.latency_ms = Some(milliseconds);
                        }
                        ViewerStatus::Performance {
                            fps_times_100,
                            input_kbps,
                            dropped_frames,
                            session_seconds,
                            reconnect_count,
                        } => {
                            entry.fps_times_100 = fps_times_100;
                            entry.input_kbps = input_kbps;
                            entry.dropped_frames = dropped_frames;
                            entry.session_seconds = session_seconds;
                            entry.reconnect_count = reconnect_count;
                            entry.last_telemetry_at = Some(Instant::now());
                        }
                        ViewerStatus::Heartbeat { sequence } => {
                            if sequence > entry.heartbeat_sequence {
                                entry.heartbeat_sequence = sequence;
                                schedule_viewer_liveness_timeout(
                                    process_id,
                                    entry.session_token,
                                    sequence,
                                );
                            }
                        }
                        ViewerStatus::ControlApplied { control } => {
                            if entry.pending_controls.remove(control) {
                                apply_viewer_control(entry, control);
                                entry.status = viewer_control_applied_text(control, ui_language);
                                self.status = format!("{}: {}", entry.remote_id, entry.status);
                            }
                        }
                        ViewerStatus::ControlState { control } => {
                            entry.pending_controls.remove_kind(control);
                            apply_viewer_control(entry, control);
                            entry.status = viewer_control_applied_text(control, ui_language);
                            self.status = format!("{}: {}", entry.remote_id, entry.status);
                        }
                        ViewerStatus::Failed { error } => {
                            self.status = format!("{}: {error}", entry.remote_id);
                        }
                        ViewerStatus::Closed => {
                            entry.closed_status_received = true;
                            self.status = format!("Сессия {} закрыта", entry.remote_id);
                        }
                        _ => {}
                    }
                }
            }
            ProcessEvent::StreamClosed { process_id } => {
                if let Some(mut entry) = self.viewers.remove(&process_id) {
                    let exit_status = entry
                        .process
                        .wait_for_exit(VIEWER_EXIT_STATUS_TIMEOUT)
                        .ok()
                        .flatten();
                    let exit_kind = classify_viewer_exit(
                        exit_status.as_ref().map(|status| status.success()),
                        entry.disconnect_requested,
                        entry.closed_status_received,
                    );
                    let exit = exit_status
                        .map(|status| status.to_string())
                        .unwrap_or_else(|| "завершён".to_owned());
                    self.status = match (exit_kind, entry.diagnostics.back()) {
                        (ViewerExitKind::Requested, _) => {
                            format!("Сессия {} отключена", entry.remote_id)
                        }
                        (ViewerExitKind::Clean, _) => {
                            format!("Сессия {} закрыта", entry.remote_id)
                        }
                        (ViewerExitKind::Crashed, Some(diagnostic)) => {
                            format!("Viewer {}: {exit} · {diagnostic}", entry.remote_id)
                        }
                        (ViewerExitKind::Crashed, None) => {
                            format!("Viewer {}: {exit}", entry.remote_id)
                        }
                        (ViewerExitKind::Lost, Some(diagnostic)) => {
                            format!(
                                "Viewer {} завершён неожиданно · {diagnostic}",
                                entry.remote_id
                            )
                        }
                        (ViewerExitKind::Lost, None) => {
                            format!("Viewer {} завершён неожиданно", entry.remote_id)
                        }
                    };
                }
            }
            ProcessEvent::Diagnostic {
                process_id,
                message,
            } => {
                if let Some(entry) = self.viewers.get_mut(&process_id) {
                    push_viewer_diagnostic(&mut entry.diagnostics, message);
                }
            }
            ProcessEvent::ClipboardExpiry { token, fingerprint } => {
                if token == self.clipboard_token && clear_matching_clipboard(fingerprint) {
                    self.status = "Скопированный пароль удалён из буфера обмена".to_owned();
                }
            }
            ProcessEvent::ApprovalExpired { peer_id, token } => {
                if approval_matches(self.pending_approval.as_ref(), &peer_id, token) {
                    self.approve_incoming(false);
                    self.status =
                        format!("Запрос от {peer_id} отклонён: время подтверждения истекло");
                }
            }
            ProcessEvent::ViewerStartupExpired { process_id, token } => {
                let expired = self.viewers.get(&process_id).is_some_and(|entry| {
                    viewer_watchdog_applies(entry.session_token, token, entry.ipc_ready)
                });
                if expired {
                    if let Some(entry) = self.viewers.remove(&process_id) {
                        self.status = format!(
                            "Viewer {} не ответил по IPC за {} секунд и был остановлен",
                            entry.remote_id,
                            VIEWER_STARTUP_TIMEOUT.as_secs()
                        );
                    }
                }
            }
            ProcessEvent::ViewerShutdownExpired { process_id, token } => {
                let expired = self.viewers.get(&process_id).is_some_and(|entry| {
                    viewer_watchdog_applies(entry.session_token, token, !entry.disconnect_requested)
                });
                if expired {
                    if let Some(entry) = self.viewers.remove(&process_id) {
                        self.status = format!(
                            "Зависшая сессия {} принудительно завершена",
                            entry.remote_id
                        );
                    }
                }
            }
            ProcessEvent::ViewerControlExpired {
                process_id,
                token,
                control,
            } => {
                let ui_language = self.ui_language();
                if let Some(entry) = self.viewers.get_mut(&process_id) {
                    if entry.session_token == token && entry.pending_controls.remove(control) {
                        entry.status = match ui_language {
                            UiLanguage::Russian => format!(
                                "Viewer не подтвердил настройку «{}»",
                                viewer_control_label(control, ui_language)
                            ),
                            UiLanguage::English => format!(
                                "Viewer did not confirm “{}”",
                                viewer_control_label(control, ui_language)
                            ),
                        };
                        self.status = format!("{}: {}", entry.remote_id, entry.status);
                    }
                }
            }
            ProcessEvent::ViewerLivenessExpired {
                process_id,
                token,
                heartbeat_sequence,
            } => {
                let expired = self.viewers.get(&process_id).is_some_and(|entry| {
                    viewer_liveness_expired(
                        entry.session_token,
                        token,
                        entry.heartbeat_sequence,
                        heartbeat_sequence,
                        entry.disconnect_requested,
                    )
                });
                if expired {
                    if let Some(entry) = self.viewers.remove(&process_id) {
                        self.status = format!(
                            "Viewer {} перестал отвечать и был остановлен системой контроля",
                            entry.remote_id
                        );
                    }
                }
            }
            ProcessEvent::Host(_) => {}
            ProcessEvent::Tray(_) => {}
            ProcessEvent::AddressBook(_) => {}
            ProcessEvent::SmartAgent(_) => {}
            ProcessEvent::Updater(_) => {}
            ProcessEvent::CurrentUserRefreshed(_) => {}
            ProcessEvent::SecondInstance => {}
        }
    }

    fn tick_smart_agent(&mut self) {
        if !self.store.smart_agent_enabled {
            return;
        }
        let now = Instant::now();
        let heartbeat_interval =
            smart_agent_heartbeat_interval(self.smart_agent_heartbeat_failures);
        let heartbeat_due = self
            .smart_agent_last_heartbeat
            .is_none_or(|last| now.duration_since(last) >= heartbeat_interval);
        if heartbeat_due
            && !self.smart_agent_heartbeat_busy
            && now.duration_since(self.smart_agent_started_at) >= Duration::from_secs(3)
        {
            self.smart_agent_heartbeat_busy = true;
            self.smart_agent_last_heartbeat = Some(now);
            spawn_smart_agent_heartbeat(
                self.config.ui.agent_machine_id.clone(),
                self.config.local_id.clone(),
                self.store.smart_agent_service_key.clone(),
            );
        }

        let inbox_interval = smart_agent_inbox_interval(self.smart_agent_burst_until, now);
        let inbox_due = self
            .smart_agent_last_inbox
            .is_none_or(|last| now.duration_since(last) >= inbox_interval);
        if inbox_due
            && !self.smart_agent_inbox_busy
            && now.duration_since(self.smart_agent_started_at) >= Duration::from_secs(8)
        {
            self.smart_agent_inbox_busy = true;
            self.smart_agent_last_inbox = Some(now);
            spawn_smart_agent_inbox(
                self.config.ui.agent_machine_id.clone(),
                self.store.smart_agent_service_key.clone(),
            );
        }
    }

    fn acknowledge_smart_notification(&mut self, notification_id: u64) {
        if !self
            .smart_agent_notifications
            .iter()
            .any(|notification| notification.id == notification_id)
        {
            return;
        }
        self.smart_agent_status = "Подтверждение уведомления…".to_owned();
        spawn_smart_agent_ack(self.config.ui.agent_machine_id.clone(), notification_id);
    }

    fn refresh_smart_operators(&mut self) {
        if self.smart_agent_operators_busy {
            return;
        }
        if self.store.smart_agent_service_key.trim().is_empty() {
            self.support_request_status = "Укажите service_key в настройках Smart Agent".to_owned();
            return;
        }
        self.smart_agent_operators_busy = true;
        self.support_request_status = "Загрузка операторов…".to_owned();
        spawn_smart_agent_operators(self.store.smart_agent_service_key.clone());
    }

    fn request_smart_support(&mut self) {
        if self.support_request_busy {
            return;
        }
        let Some(target_machine_id) = self.support_target_machine_id.clone() else {
            self.support_request_status = "Выберите оператора".to_owned();
            return;
        };
        let Some(operator) = self
            .smart_agent_operators
            .iter()
            .find(|operator| operator.machine_id == target_machine_id)
            .cloned()
        else {
            self.support_request_status = "Оператор больше не найден в списке".to_owned();
            return;
        };
        if self.store.smart_agent_service_key.trim().is_empty() {
            self.support_request_status = "Укажите service_key в настройках Smart Agent".to_owned();
            return;
        }
        let message = self.support_request_message.trim();
        let message = if message.is_empty() {
            "Пользователь запросил удалённую поддержку".to_owned()
        } else {
            bounded_text(message, MAX_SUPPORT_MESSAGE_CHARS)
        };
        self.support_request_busy = true;
        self.support_request_status = "Запрос поддержки отправляется…".to_owned();
        spawn_smart_agent_support_request(SupportRequest {
            machine_id: self.config.ui.agent_machine_id.clone(),
            service_key: self.store.smart_agent_service_key.clone(),
            hostname: local_hostname(),
            message,
            target_machine_id: operator.machine_id,
            target_rustdesk_id: normalize_remote_id(&operator.rustdesk_id),
            from_rustdesk_id: normalize_remote_id(&self.config.local_id),
        });
    }

    fn handle_smart_agent_event(&mut self, event: SmartAgentEvent) -> Task<Message> {
        match event {
            SmartAgentEvent::Heartbeat(result) => {
                self.smart_agent_heartbeat_busy = false;
                match result {
                    Ok(()) => {
                        self.smart_agent_heartbeat_failures = 0;
                        self.smart_agent_status = "Smart Agent подключён".to_owned();
                        if self.smart_agent_operators.is_empty()
                            && !self.smart_agent_operators_busy
                            && !self.store.smart_agent_service_key.trim().is_empty()
                        {
                            self.refresh_smart_operators();
                        }
                    }
                    Err(error) => {
                        self.smart_agent_heartbeat_failures =
                            self.smart_agent_heartbeat_failures.saturating_add(1);
                        self.smart_agent_status =
                            format!("Smart Agent: {}", bounded_text(&error, 180));
                    }
                }
            }
            SmartAgentEvent::Inbox(result) => {
                self.smart_agent_inbox_busy = false;
                match result {
                    Ok(items) => {
                        self.smart_agent_inbox_failures = 0;
                        let mut refresh_entitlements = false;
                        for notification in items {
                            if notification.kind == "entitlements_changed" {
                                refresh_entitlements = true;
                                continue;
                            }
                            if self
                                .smart_agent_notifications
                                .iter()
                                .any(|existing| existing.id == notification.id)
                            {
                                continue;
                            }
                            self.smart_agent_notifications.push_back(notification);
                        }
                        while self.smart_agent_notifications.len() > MAX_SMART_AGENT_NOTIFICATIONS {
                            self.smart_agent_notifications.pop_front();
                        }
                        if refresh_entitlements {
                            return self.refresh_current_user_entitlements();
                        }
                    }
                    Err(error) => {
                        self.smart_agent_inbox_failures =
                            self.smart_agent_inbox_failures.saturating_add(1);
                        self.smart_agent_status =
                            format!("Smart Agent inbox: {}", bounded_text(&error, 180));
                    }
                }
            }
            SmartAgentEvent::Acknowledged {
                notification_id,
                result,
            } => match result {
                Ok(()) => {
                    self.smart_agent_notifications
                        .retain(|notification| notification.id != notification_id);
                    self.smart_agent_status = "Уведомление подтверждено".to_owned();
                }
                Err(error) => {
                    self.smart_agent_status =
                        format!("Не удалось подтвердить: {}", bounded_text(&error, 180));
                }
            },
            SmartAgentEvent::Voted {
                notification_id,
                result,
            } => match result {
                Ok(()) => {
                    self.smart_agent_notifications
                        .retain(|notification| notification.id != notification_id);
                    self.smart_agent_status = "Ответ отправлен".to_owned();
                }
                Err(error) => {
                    self.smart_agent_status =
                        format!("Не удалось отправить ответ: {}", bounded_text(&error, 180));
                }
            },
            SmartAgentEvent::SupportResponded {
                notification_id,
                action,
                from_remote_id,
                result,
            } => match result {
                Ok(()) => {
                    self.smart_agent_notifications
                        .retain(|notification| notification.id != notification_id);
                    self.smart_agent_burst_until =
                        Some(Instant::now() + SMART_AGENT_BURST_DURATION);
                    self.smart_agent_last_inbox = None;
                    if action == smart_agent::SupportAction::Accept
                        && !normalize_remote_id(&from_remote_id).is_empty()
                    {
                        self.remote_id = normalize_remote_id(&from_remote_id);
                        self.status = format!("Запрос принят — подключение к {}", self.remote_id);
                        return self.begin_credentials();
                    }
                    self.smart_agent_status = support_action_result_text(action).to_owned();
                }
                Err(error) => {
                    self.smart_agent_status = format!(
                        "Не удалось ответить на запрос: {}",
                        bounded_text(&error, 180)
                    );
                }
            },
            SmartAgentEvent::OperatorsLoaded(result) => {
                self.smart_agent_operators_busy = false;
                match result {
                    Ok(operators) => {
                        self.smart_agent_operators = operators;
                        if let Some(selected) = &self.support_target_machine_id {
                            if !self
                                .smart_agent_operators
                                .iter()
                                .any(|operator| &operator.machine_id == selected)
                            {
                                self.support_target_machine_id = None;
                            }
                        }
                        self.support_request_status = if self.smart_agent_operators.is_empty() {
                            "Операторы не найдены".to_owned()
                        } else {
                            format!("Операторов: {}", self.smart_agent_operators.len())
                        };
                    }
                    Err(error) => {
                        self.support_request_status = format!(
                            "Не удалось загрузить операторов: {}",
                            bounded_text(&error, 180)
                        );
                    }
                }
            }
            SmartAgentEvent::SupportRequested(result) => {
                self.support_request_busy = false;
                match result {
                    Ok(request_id) => {
                        self.support_request_message.clear();
                        self.support_request_status =
                            format!("Запрос поддержки #{} создан", request_id);
                        self.smart_agent_burst_until =
                            Some(Instant::now() + SMART_AGENT_BURST_DURATION);
                        self.smart_agent_last_inbox = None;
                    }
                    Err(error) => {
                        self.support_request_status =
                            format!("Не удалось создать запрос: {}", bounded_text(&error, 180));
                    }
                }
            }
        }
        Task::none()
    }

    fn handle_host_event(&mut self, event: HostEvent) -> Task<Message> {
        let focus_incoming = matches!(
            &event,
            HostEvent::ApprovalRequested { .. } | HostEvent::SessionStarted { .. }
        );
        let approval_peer = match &event {
            HostEvent::ApprovalRequested { peer_id, .. } => Some(peer_id.clone()),
            _ => None,
        };
        match event {
            HostEvent::StateChanged(state) => {
                if matches!(state, HostState::Idle | HostState::Error(_)) {
                    if let Some(session) = &self.incoming_session {
                        self.store.finish_incoming(
                            &session.peer_id,
                            session.started_at.elapsed().as_secs(),
                            state.label(),
                        );
                        let _ = self.store.save_default();
                    }
                    self.pending_approval = None;
                    self.incoming_accepting = None;
                    self.incoming_session = None;
                }
                self.status = state.label().to_owned();
                self.host_state = state;
            }
            HostEvent::Registered { .. } => {
                self.host_state = HostState::Ready;
                self.status = "Устройство зарегистрировано и доступно".to_owned();
            }
            HostEvent::IncomingRequest { peer_id, .. } => {
                self.status = format!("Входящий запрос от {peer_id}");
            }
            HostEvent::ApprovalRequested {
                peer_id,
                peer_name,
                platform,
                version,
            } => {
                if self.incoming_session.is_some() || self.incoming_accepting.is_some() {
                    self.reject_incoming_peer(peer_id.clone());
                    self.status =
                        format!("Запрос от {peer_id} отклонён: другая сессия уже активна");
                } else {
                    if let Some(previous) = self.pending_approval.take() {
                        if previous.peer_id != peer_id {
                            self.reject_incoming_peer(previous.peer_id);
                        }
                    }
                    self.approval_token = self.approval_token.wrapping_add(1);
                    let token = self.approval_token;
                    schedule_approval_expiry(peer_id.clone(), token);
                    self.pending_approval = Some(PendingApproval {
                        peer_id,
                        peer_name,
                        platform,
                        version,
                        token,
                        expires_at: Instant::now() + APPROVAL_UI_TIMEOUT,
                        allow_input: self.config.security.allow_keyboard_mouse,
                        allow_clipboard: self.config.security.allow_clipboard,
                    });
                    self.status = "Требуется подтверждение входящего доступа".to_owned();
                }
            }
            HostEvent::SessionStarted {
                peer_id,
                session_id,
                peer_name,
                platform,
                version,
            } => {
                self.pending_approval = None;
                let accepted = self
                    .incoming_accepting
                    .take()
                    .filter(|incoming| incoming.peer_id == peer_id);
                let allow_input = accepted
                    .as_ref()
                    .map_or(self.config.security.allow_keyboard_mouse, |incoming| {
                        incoming.allow_input
                    });
                let allow_clipboard = accepted
                    .as_ref()
                    .map_or(self.config.security.allow_clipboard, |incoming| {
                        incoming.allow_clipboard
                    });
                let desired_input_blocked = !allow_input;
                self.store.record_incoming(&peer_id);
                let history_error = self.store.save_default().err();
                self.incoming_session = Some(IncomingSession {
                    session_id,
                    peer_id: peer_id.clone(),
                    peer_name,
                    platform,
                    version,
                    input_blocked: false,
                    clipboard_allowed: self.config.security.allow_clipboard,
                    pending_input_blocked: Some(desired_input_blocked),
                    pending_clipboard_allowed: Some(allow_clipboard),
                    started_at: Instant::now(),
                    telemetry: None,
                    fallback_reason: None,
                    disconnect_requested: false,
                });
                if let Some(runtime) = &self.host {
                    if runtime
                        .commands
                        .send(HostCommand::SetSessionPermissions {
                            peer_id: peer_id.clone(),
                            session_id,
                            input_blocked: desired_input_blocked,
                            clipboard_allowed: allow_clipboard,
                        })
                        .is_err()
                    {
                        if let Some(session) = self.incoming_session.as_mut() {
                            session.pending_input_blocked = None;
                            session.pending_clipboard_allowed = None;
                        }
                    }
                }
                self.host_state = HostState::Accepting(peer_id.clone());
                self.status = if let Some(error) = history_error {
                    format!("Входящая сессия с {peer_id} · история не сохранена: {error}")
                } else {
                    format!("Входящая сессия с {peer_id}")
                };
            }
            HostEvent::SessionEnded {
                peer_id,
                reason,
                session_id,
            } => {
                if self
                    .pending_approval
                    .as_ref()
                    .is_some_and(|pending| pending.peer_id == peer_id)
                {
                    self.pending_approval = None;
                }
                if self
                    .incoming_accepting
                    .as_ref()
                    .is_some_and(|incoming| incoming.peer_id == peer_id)
                {
                    self.incoming_accepting = None;
                }
                let active_session_matches =
                    self.incoming_session.as_ref().is_some_and(|session| {
                        session.peer_id == peer_id && session.session_id == session_id
                    });
                if self.incoming_session.is_some() && !active_session_matches {
                    self.status = format!("Устаревшее завершение сессии {peer_id} проигнорировано");
                    self.update_tray_host_item();
                    return Task::none();
                }
                if let Some(runtime) = &self.host {
                    let _ = runtime.commands.send(HostCommand::SetInputBlocked(false));
                    let _ = runtime.commands.send(HostCommand::SetClipboardAllowed(
                        self.config.security.allow_clipboard,
                    ));
                }
                let duration_seconds = self
                    .incoming_session
                    .as_ref()
                    .filter(|session| session.peer_id == peer_id)
                    .map(|session| session.started_at.elapsed().as_secs())
                    .unwrap_or_default();
                self.store
                    .finish_incoming(&peer_id, duration_seconds, &reason);
                let history_error = self.store.save_default().err();
                self.incoming_session = None;
                self.host_state = HostState::Ready;
                self.status = if let Some(error) = history_error {
                    format!("Сессия {peer_id} завершена: {reason} · история не сохранена: {error}")
                } else {
                    format!("Сессия {peer_id} завершена: {reason}")
                };
            }
            HostEvent::VideoTelemetry {
                summary,
                fallback_reason,
            } => {
                if let Some(session) = self.incoming_session.as_mut() {
                    session.telemetry = parse_host_video_telemetry(&summary);
                    session.fallback_reason = fallback_reason;
                }
            }
            HostEvent::SessionPermissionsChanged {
                peer_id,
                session_id,
                input_blocked,
                clipboard_allowed,
            } => {
                if apply_session_permissions_ack(
                    self.incoming_session.as_mut(),
                    &peer_id,
                    session_id,
                    input_blocked,
                    clipboard_allowed,
                ) {
                    self.status = format!("Разрешения сессии {peer_id} применены");
                }
            }
            HostEvent::Log(message) => {
                // '⚠' matches both the bare warning sign and the emoji-
                // presentation form ('⚠️' = U+26A0 U+FE0F) since the base
                // codepoint is present in either. Host-side warnings the
                // user actually needs to act on all use this convention:
                // the elevation-needed hint, missing macOS Screen
                // Recording/Accessibility permission, the firewall-rule
                // failure, and the "hardware codec unavailable" notice.
                // Without this, they were silently dropped — visible only
                // in `error`/`Ошибка` messages, none of which these are.
                if message.contains("error") || message.contains("Ошибка") || message.contains('⚠')
                {
                    self.status = message;
                }
            }
        }
        self.update_tray_host_item();

        let should_focus = approval_peer.is_some_and(|peer_id| {
            self.pending_approval
                .as_ref()
                .is_some_and(|pending| pending.peer_id == peer_id)
        });
        let needs_incoming_window = self.pending_approval.is_some()
            || self.incoming_accepting.is_some()
            || self.incoming_session.is_some();
        if needs_incoming_window
            && (focus_incoming || should_focus || self.incoming_window_id.is_none())
        {
            return self.ensure_incoming_window();
        }
        if !needs_incoming_window && self.incoming_window_id.is_some() {
            return self.close_incoming_window();
        }
        Task::none()
    }

    fn update_tray_host_item(&self) {
        #[cfg(windows)]
        if let Some(tray) = &self.tray {
            tray.set_state(
                self.host.is_some(),
                &tray_status_label(
                    self.host.is_some(),
                    &self.host_state,
                    self.pending_approval.as_ref(),
                    self.incoming_session.as_ref(),
                ),
            );
        }
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        if let Some(runtime) = self.host.take() {
            let _ = runtime.commands.send(HostCommand::Stop);
        }
        self.begin_viewer_shutdown();
    }
}

#[cfg(windows)]
impl TrayController {
    fn new(events: async_channel::Sender<ProcessEvent>) -> Result<Self, String> {
        let status = MenuItem::with_id("evertydesk.status", "Остановлен", false, None);
        let open = MenuItem::with_id("evertydesk.open", "Открыть EvertyDesk", true, None);
        let host = MenuItem::with_id("evertydesk.host", "Запустить приём подключений", true, None);
        let separator = PredefinedMenuItem::separator();
        let quit = MenuItem::with_id("evertydesk.quit", "Выход", true, None);
        let menu = Menu::with_items(&[&status, &open, &separator, &host, &quit])
            .map_err(|error| error.to_string())?;

        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = match event.id().as_ref() {
                "evertydesk.open" => Some(TrayAction::Open),
                "evertydesk.host" => Some(TrayAction::ToggleHost),
                "evertydesk.quit" => Some(TrayAction::Quit),
                _ => None,
            };
            if let Some(action) = action {
                let _ = events.send_blocking(ProcessEvent::Tray(action));
            }
        }));

        let icon = Icon::from_rgba(tray_icon_rgba(), 32, 32).map_err(|error| error.to_string())?;
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("EvertyDesk — удалённый доступ")
            .with_icon(icon)
            .build()
            .map_err(|error| error.to_string())?;

        Ok(Self {
            icon: tray_icon,
            host_item: host,
            status_item: status,
        })
    }

    fn set_state(&self, active: bool, status: &str) {
        self.host_item.set_text(if active {
            "Остановить приём подключений"
        } else {
            "Запустить приём подключений"
        });
        self.status_item.set_text(status);
        let _ = self
            .icon
            .set_tooltip(Some(format!("EvertyDesk — {status}")));
    }
}

#[cfg(windows)]
fn tray_icon_rgba() -> Vec<u8> {
    include_bytes!("../../assets/logo-32.rgba").to_vec()
}

fn brand_badge(size: f32, font_size: u32) -> Element<'static, Message> {
    container(text("E").size(font_size).color(Color::WHITE))
        .center_x(Length::Fixed(size))
        .center_y(Length::Fixed(size))
        .style(accent_tile)
        .into()
}

fn nav_button(label: &str, page: Page, current: Page) -> Element<'_, Message> {
    let selected = page == current;
    button(text(label).size(13))
        .on_press(Message::Navigate(page))
        .padding([8, 12])
        .style(move |theme, status| {
            if selected {
                selected_nav_button(theme, status)
            } else {
                navigation_button(theme, status)
            }
        })
        .into()
}

fn nav_icon_button(
    icon: icondata::Icon,
    label: &'static str,
    page: Page,
    current: Page,
) -> Element<'static, Message> {
    let selected = page == current;
    let icon_color = if selected { ACCENT } else { MUTED };
    let action = button(lucide_icon(icon, 16.0, icon_color))
        .on_press(Message::Navigate(page))
        .width(Length::Fixed(38.0))
        .height(Length::Fixed(34.0))
        .padding(5)
        .style(move |theme, status| {
            if selected {
                selected_nav_button(theme, status)
            } else {
                navigation_button(theme, status)
            }
        });
    tooltip(
        action,
        container(text(label).size(11).color(Color::WHITE))
            .padding([6, 9])
            .style(tooltip_panel),
        tooltip::Position::Top,
    )
    .gap(6)
    .delay(Duration::from_millis(350))
    .into()
}

fn vm_power_controls(target: &str, disabled: bool) -> Element<'static, Message> {
    let target = target.to_owned();
    let mut actions = row![].spacing(4);
    for action in [
        VmPowerAction::Start,
        VmPowerAction::Stop,
        VmPowerAction::Restart,
    ] {
        let button = button(text(action.label()).size(11)).padding([6, 8]).style(
            if action == VmPowerAction::Stop {
                danger_text_button
            } else {
                quiet_button
            },
        );
        let button = if disabled {
            button
        } else {
            button.on_press(Message::RunVmPowerAction {
                target: target.clone(),
                action,
            })
        };
        actions = actions.push(button);
    }
    actions.into()
}

fn vm_inventory_group_key(id: &str) -> &'static str {
    let provider = id
        .trim()
        .split_once(':')
        .map(|(provider, _)| provider.to_ascii_lowercase())
        .unwrap_or_else(|| "hyperv".to_owned());
    match provider.as_str() {
        "hyperv" => "1_hyperv",
        "vbox" => "2_vbox",
        "vmware" => "3_vmware",
        _ => "9_other",
    }
}

fn vm_inventory_group_label(key: &str) -> &'static str {
    match key {
        "1_hyperv" => "HYPER-V",
        "2_vbox" => "VIRTUALBOX",
        "3_vmware" => "VMWARE",
        _ => "OTHER",
    }
}

fn vm_provider_label_for_id(id: &str) -> &'static str {
    vm_inventory_group_label(vm_inventory_group_key(id))
}

fn vm_state_color(state: &str, connectable: bool) -> Color {
    let normalized = state.trim().to_ascii_lowercase();
    if connectable
        || normalized.contains("running")
        || normalized.contains("работ")
        || normalized.contains("on")
    {
        Color::from_rgb(0.12, 0.58, 0.35)
    } else if normalized.contains("paused")
        || normalized.contains("saved")
        || normalized.contains("пау")
        || normalized.contains("сохран")
    {
        Color::from_rgb(0.91, 0.58, 0.10)
    } else {
        MUTED
    }
}

fn vm_badge(label: impl Into<String>, color: Color) -> Element<'static, Message> {
    let label = label.into();
    container(text(label).size(10).color(color))
        .padding([3, 8])
        .style(move |_theme| iced::widget::container::Style {
            text_color: Some(color),
            background: Some(Color::from_rgba(color.r, color.g, color.b, 0.09).into()),
            border: Border {
                radius: 999.0.into(),
                width: 1.0,
                color: Color::from_rgba(color.r, color.g, color.b, 0.28),
            },
            ..Default::default()
        })
        .into()
}

fn dispatch_vm_power_action(target: &str, action: VmPowerAction) -> String {
    let target = sanitize_vm_target_id(target);
    let (provider, real_id) = target
        .split_once(':')
        .unwrap_or(("hyperv", target.as_str()));
    match provider {
        "vbox" => {
            match action {
                VmPowerAction::Start => virtualbox::start_vm(real_id),
                VmPowerAction::Stop => virtualbox::stop_vm(real_id),
                VmPowerAction::Restart => virtualbox::reset_vm(real_id),
            }
            format!(
                "VirtualBox: команда {} отправлена для {real_id}",
                action.label()
            )
        }
        "hyperv" => dispatch_hyperv_power_action(real_id, action),
        other => format!("Power action не поддержан для VM provider: {other}"),
    }
}

#[cfg(windows)]
fn dispatch_hyperv_power_action(real_id: &str, action: VmPowerAction) -> String {
    let hv_action = match action {
        VmPowerAction::Start => hyperv::VmPowerAction::Start,
        VmPowerAction::Stop => hyperv::VmPowerAction::Stop,
        VmPowerAction::Restart => hyperv::VmPowerAction::Restart,
    };
    hyperv::request_power_action(real_id, hv_action);
    format!(
        "Hyper-V: команда {} отправлена для {real_id}",
        action.label()
    )
}

#[cfg(not(windows))]
fn dispatch_hyperv_power_action(real_id: &str, _action: VmPowerAction) -> String {
    format!("Hyper-V недоступен на этой платформе: {real_id}")
}

fn viewer_game_codec(codec: GameCodecPreference) -> ViewerGameCodec {
    match codec {
        GameCodecPreference::Auto => ViewerGameCodec::Auto,
        GameCodecPreference::H265 => ViewerGameCodec::H265,
        GameCodecPreference::H264 => ViewerGameCodec::H264,
        GameCodecPreference::Av1 => ViewerGameCodec::Av1,
    }
}

fn viewer_game_profile_label(
    game_mode: bool,
    codec: ViewerGameCodec,
    evrt2_enabled: bool,
) -> String {
    if !game_mode {
        return "Режим: Desktop · EVRTCK".to_owned();
    }
    if evrt2_enabled {
        format!("Game {} · EVRT2", codec.label())
    } else {
        format!("Game {}", codec.label())
    }
}

fn viewer_launch_status(
    quality: ConnectionQuality,
    game_mode: bool,
    codec: ViewerGameCodec,
    evrt2_enabled: bool,
) -> String {
    if game_mode {
        format!(
            "Launch · {} · {}",
            quality.label(),
            viewer_game_profile_label(true, codec, evrt2_enabled)
        )
    } else {
        format!("Launch · {}", quality.label())
    }
}

fn icon_action<'a>(
    icon: icondata::Icon,
    label: &'a str,
    message: Message,
    danger: bool,
) -> Element<'a, Message> {
    let icon_color = if danger { ACCENT } else { MUTED };
    let action = button(lucide_icon(icon, 17.0, icon_color))
        .on_press(message)
        .width(Length::Fixed(34.0))
        .height(Length::Fixed(34.0))
        .padding(5)
        .style(if danger {
            danger_text_button
        } else {
            quiet_button
        });
    tooltip(
        action,
        container(text(label).size(11).color(Color::WHITE))
            .padding([6, 9])
            .style(tooltip_panel),
        tooltip::Position::Top,
    )
    .gap(6)
    .delay(Duration::from_millis(350))
    .into()
}

fn address_book_nav_item(
    icon: icondata::Icon,
    label: impl Into<String>,
    count: usize,
    selected: bool,
    message: Message,
) -> Element<'static, Message> {
    address_book_nav_item_with_indent(icon, label, count, selected, message, 0.0)
}

fn address_book_group_nav_item(
    group: String,
    count: usize,
    selected: bool,
    message: Message,
) -> Element<'static, Message> {
    let depth = group_path_depth(&group);
    let icon = if depth == 0 {
        icondata::LuFolder
    } else {
        icondata::LuCornerDownRight
    };
    address_book_nav_item_with_indent(
        icon,
        group_leaf_label(&group),
        count,
        selected,
        message,
        address_book_group_indent(depth),
    )
}

fn address_book_nav_item_with_indent(
    icon: icondata::Icon,
    label: impl Into<String>,
    count: usize,
    selected: bool,
    message: Message,
    indent: f32,
) -> Element<'static, Message> {
    let label = label.into();
    button(
        row![
            Space::new().width(Length::Fixed(indent)),
            lucide_icon(icon, 16.0, if selected { ACCENT } else { MUTED }),
            text(label).size(12).width(Fill),
            text(count.to_string())
                .size(11)
                .color(if selected { ACCENT } else { MUTED }),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .on_press(message)
    .padding([8, 10])
    .width(Fill)
    .style(move |theme, status| {
        if selected {
            selected_nav_button(theme, status)
        } else {
            quiet_button(theme, status)
        }
    })
    .into()
}

fn nav_caption(label: &'static str) -> Element<'static, Message> {
    row![
        text(label).size(10).color(MUTED),
        container(Space::new().height(1))
            .width(Fill)
            .style(separator_style),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn label_with_icon(
    label: &'static str,
    icon: icondata::Icon,
    color: Color,
) -> Element<'static, Message> {
    row![text(label).size(13), lucide_icon(icon, 15.0, color)]
        .spacing(7)
        .align_y(Alignment::Center)
        .into()
}

fn about_info_row(
    label: &'static str,
    value: &'static str,
    icon: icondata::Icon,
) -> Element<'static, Message> {
    row![
        lucide_icon(icon, 17.0, ACCENT),
        text(label).size(12).color(MUTED).width(Length::Fixed(84.0)),
        text(value).size(14).width(Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

fn about_action_button(
    label: &'static str,
    icon: icondata::Icon,
    message: Message,
) -> iced::widget::Button<'static, Message> {
    button(
        row![lucide_icon(icon, 16.0, ACCENT), text(label).size(13)]
            .spacing(7)
            .align_y(Alignment::Center),
    )
    .on_press(message)
    .padding([8, 12])
    .style(quiet_button)
}

fn config_update_details(update: smart_agent::ConfigUpdate) -> Element<'static, Message> {
    let mut details = column![].spacing(5);
    if !update.server.trim().is_empty() {
        details = details.push(config_update_field("server", update.server));
    }
    if !update.key.trim().is_empty() {
        details = details.push(config_update_field("key", update.key));
    }
    if !update.api_server.trim().is_empty() {
        details = details.push(config_update_field("api_server", update.api_server));
    }
    details.into()
}

fn config_update_field(label: &'static str, value: String) -> Element<'static, Message> {
    let visible_value = bounded_text(&value, 120);
    row![
        text(label).size(11).color(MUTED).width(Length::Fixed(74.0)),
        text(visible_value).size(12).width(Fill),
        icon_action(
            icondata::LuCheck,
            "Скопировать поле",
            Message::CopySmartConfigField {
                label: label.to_owned(),
                value,
            },
            false,
        ),
    ]
    .spacing(7)
    .align_y(Alignment::Center)
    .into()
}

fn support_action_button(
    notification_id: u64,
    request_id: u64,
    action: smart_agent::SupportAction,
    from_remote_id: String,
) -> Element<'static, Message> {
    let message = Message::RespondToSupport {
        notification_id,
        request_id,
        action,
        from_remote_id,
    };
    let button = button(text(support_action_label(action)).size(12))
        .on_press(message)
        .padding([7, 9])
        .width(Length::Fixed(92.0));
    match action {
        smart_agent::SupportAction::Accept => button.style(accent_button).into(),
        smart_agent::SupportAction::Decline => button.style(danger_text_button).into(),
        smart_agent::SupportAction::Defer10 | smart_agent::SupportAction::Defer60 => {
            button.style(quiet_button).into()
        }
    }
}

fn support_action_label(action: smart_agent::SupportAction) -> &'static str {
    match action {
        smart_agent::SupportAction::Accept => "Принять",
        smart_agent::SupportAction::Defer10 => "10 мин",
        smart_agent::SupportAction::Defer60 => "1 час",
        smart_agent::SupportAction::Decline => "Отклонить",
    }
}

fn support_action_result_text(action: smart_agent::SupportAction) -> &'static str {
    match action {
        smart_agent::SupportAction::Accept => "Запрос поддержки принят",
        smart_agent::SupportAction::Defer10 => "Ответ отложен на 10 минут",
        smart_agent::SupportAction::Defer60 => "Ответ отложен на один час",
        smart_agent::SupportAction::Decline => "Запрос поддержки отклонён",
    }
}

fn smart_agent_heartbeat_interval(failures: u8) -> Duration {
    if (1..=3).contains(&failures) {
        Duration::from_secs(5 * u64::from(failures))
    } else {
        SMART_AGENT_HEARTBEAT_INTERVAL
    }
}

fn smart_agent_inbox_interval(burst_until: Option<Instant>, now: Instant) -> Duration {
    if burst_until.is_some_and(|until| until > now) {
        SMART_AGENT_BURST_INTERVAL
    } else {
        SMART_AGENT_INBOX_INTERVAL
    }
}

fn lucide_icon(icon: icondata::Icon, size: f32, color: Color) -> Element<'static, Message> {
    let color = color_to_svg_hex(color);
    let source = format!(
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{}" "#,
            r#"fill="{}" stroke="{}" stroke-width="{}" "#,
            r#"stroke-linecap="{}" stroke-linejoin="{}">{}</svg>"#
        ),
        icon.view_box.unwrap_or("0 0 24 24"),
        icon.fill.unwrap_or("none"),
        color,
        icon.stroke_width.unwrap_or("2"),
        icon.stroke_linecap.unwrap_or("round"),
        icon.stroke_linejoin.unwrap_or("round"),
        icon.data,
    );

    svg(svg::Handle::from_memory(source.into_bytes()))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
}

fn color_to_svg_hex(color: Color) -> String {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(color.r),
        channel(color.g),
        channel(color.b)
    )
}

fn use_wide_directory_layout(width: f32) -> bool {
    width >= 1_320.0
}

fn use_wide_support_layout(width: f32) -> bool {
    width >= 880.0
}

fn use_wide_settings_sidebar_layout(width: f32) -> bool {
    width >= 1_060.0
}

fn use_wide_settings_content_layout(width: f32) -> bool {
    width >= 1_260.0
}

fn streaming_mode_label(mode: StreamingMode) -> &'static str {
    match mode {
        StreamingMode::Support => "Support",
        StreamingMode::Interactive => "Interactive",
        StreamingMode::Game => "Game",
    }
}

fn quality_label(quality: ConnectionQuality, language: UiLanguage) -> &'static str {
    match quality {
        ConnectionQuality::Smooth => tr(language, TextKey::QualitySmooth),
        ConnectionQuality::Balanced => tr(language, TextKey::QualityBalanced),
        ConnectionQuality::Sharp => tr(language, TextKey::QualitySharp),
    }
}

fn streaming_mode_hint(mode: StreamingMode, language: UiLanguage) -> &'static str {
    match mode {
        StreamingMode::Support => tr(language, TextKey::StreamingModeSupportHint),
        StreamingMode::Interactive => tr(language, TextKey::StreamingModeInteractiveHint),
        StreamingMode::Game => tr(language, TextKey::StreamingModeGameHint),
    }
}

fn server_config_is_custom(config: &AppConfig) -> bool {
    config.server != ServerConfig::default()
}

fn server_input_value(current: &str, default: &str) -> String {
    if current == default {
        String::new()
    } else {
        current.to_owned()
    }
}

fn server_field_or_default(value: String, default: String) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        default
    } else {
        trimmed.chars().take(512).collect()
    }
}

fn sanitize_permanent_password(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_PERMANENT_PASSWORD_CHARS)
        .collect()
}

fn should_rotate_temporary_password(
    elapsed: Duration,
    has_pending_approval: bool,
    has_accepting_session: bool,
    has_active_session: bool,
) -> bool {
    elapsed >= TEMP_PASSWORD_ROTATION_INTERVAL
        && !has_pending_approval
        && !has_accepting_session
        && !has_active_session
}

fn build_vm_target(provider: VmProviderPreference, value: &str) -> String {
    let trimmed = sanitize_vm_target_id(value);
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.contains(':') {
        return trimmed;
    }
    match provider.prefix() {
        Some(prefix) => format!("{prefix}:{trimmed}"),
        None => trimmed,
    }
}

fn sanitize_vm_target_id(value: &str) -> String {
    value.trim().chars().take(160).collect()
}

fn sanitize_vm_filter(value: &str) -> String {
    value.trim_start().chars().take(96).collect()
}

fn vm_matches_filter(vm: &VmInventoryEntry, filter: &str) -> bool {
    let filter = filter.trim().to_ascii_lowercase();
    if filter.is_empty() {
        return true;
    }
    vm.id.to_ascii_lowercase().contains(&filter)
        || vm.name.to_ascii_lowercase().contains(&filter)
        || vm.state.to_ascii_lowercase().contains(&filter)
        || vm_provider_label_for_id(&vm.id)
            .to_ascii_lowercase()
            .contains(&filter)
}

fn infer_vm_provider(value: &str) -> Option<VmProviderPreference> {
    let prefix = value.trim().split_once(':')?.0.to_ascii_lowercase();
    match prefix.as_str() {
        "hyperv" => Some(VmProviderPreference::HyperV),
        "vbox" => Some(VmProviderPreference::VirtualBox),
        _ => None,
    }
}

fn current_vm_status_text() -> String {
    let status = vm_bridge::status();
    if status.trim().is_empty() {
        "VM Bridge готов. Нажмите «Список VM» или укажите VM ID для подключения.".to_owned()
    } else {
        status
    }
}

#[cfg(target_os = "macos")]
fn macos_startup_status() -> String {
    let (screen_recording, accessibility) = evertydesk_core::host::macos_permission_status();
    match (screen_recording, accessibility) {
        (true, true) => {
            "Готов к подключению. Входящий доступ на macOS включается вручную.".to_owned()
        }
        (false, true) => {
            "Нужен доступ Screen Recording для входящих подключений на macOS".to_owned()
        }
        (true, false) => {
            "Нужен доступ Accessibility для управления мышью и клавиатурой на macOS".to_owned()
        }
        (false, false) => {
            "Нужны доступы Screen Recording и Accessibility для входящих подключений на macOS"
                .to_owned()
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_permission_summary() -> (&'static str, Color) {
    let (screen_recording, accessibility) = evertydesk_core::host::macos_permission_status();
    match (screen_recording, accessibility) {
        (true, true) => (
            "Screen Recording и Accessibility разрешены",
            Color::from_rgb(0.12, 0.58, 0.35),
        ),
        (false, true) => (
            "Разреши Screen Recording в Privacy & Security",
            Color::from_rgb(0.91, 0.58, 0.10),
        ),
        (true, false) => (
            "Разреши Accessibility в Privacy & Security",
            Color::from_rgb(0.91, 0.58, 0.10),
        ),
        (false, false) => (
            "Разреши Screen Recording и Accessibility в Privacy & Security",
            Color::from_rgb(0.91, 0.58, 0.10),
        ),
    }
}

#[cfg(target_os = "macos")]
fn open_macos_privacy_settings() -> String {
    let urls = [
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
    ];
    let mut opened = false;
    for url in urls {
        if std::process::Command::new("open")
            .arg(url)
            .status()
            .is_ok_and(|status| status.success())
        {
            opened = true;
        }
    }
    if opened {
        "Открыл Privacy & Security. Разреши Accessibility и Screen Recording для EvertyDesk Next, затем перезапусти входящий доступ.".to_owned()
    } else {
        "Не удалось открыть Privacy & Security автоматически".to_owned()
    }
}

#[cfg(not(target_os = "macos"))]
fn open_macos_privacy_settings() -> String {
    "Настройки Privacy & Security доступны только на macOS".to_owned()
}

async fn run_vm_inventory() -> Result<Vec<VmInventoryEntry>, String> {
    let raw = vm_bridge::list_json();
    parse_vm_inventory(&raw)
}

async fn run_vm_attach(target: String) -> Result<String, String> {
    vm_bridge::attach(&target).map(|status| {
        if status.trim().is_empty() {
            current_vm_status_text()
        } else {
            status
        }
    })
}

async fn run_vm_detach() -> Result<String, String> {
    vm_bridge::attach("").map(|status| {
        if status.trim().is_empty() {
            "VM Bridge отключён. Транслируется физический экран хоста.".to_owned()
        } else {
            status
        }
    })
}

async fn run_vm_power_action(target: String, action: VmPowerAction) -> String {
    dispatch_vm_power_action(&target, action)
}

/// Opens an RDP console window for `bootstrap`, fire-and-forget — unlike
/// `spawn_viewer` (evertydesk-viewer.exe), this doesn't keep a handle or a
/// control channel open; the console window manages its own lifetime
/// independently once launched, the same way `vmconnect.exe`/`mstsc`
/// external launches work in the egui client.
fn spawn_rdp_viewer(bootstrap: &RdpBootstrap) -> io::Result<()> {
    let current = std::env::current_exe()?;
    let directory = current.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "launcher executable has no parent directory",
        )
    })?;
    let mut executable = directory.join("evertydesk-rdp-viewer");
    executable.set_extension(std::env::consts::EXE_EXTENSION);

    let encoded = zeroize::Zeroizing::new(serde_json::to_vec(bootstrap).map_err(io::Error::other)?);

    let mut child = std::process::Command::new(&executable)
        .arg("--bootstrap-stdin")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("could not start {}: {error}", executable.display()),
            )
        })?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "rdp-viewer stdin unavailable"))?;
    let write_result = stdin
        .write_all(&encoded)
        .and_then(|()| stdin.write_all(b"\n"))
        .and_then(|()| stdin.flush());
    drop(stdin);

    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }

    Ok(())
}

fn parse_vm_inventory(raw: &str) -> Result<Vec<VmInventoryEntry>, String> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|error| format!("не удалось прочитать список VM: {error}"))?;
    let items = value
        .as_array()
        .ok_or_else(|| "список VM вернул неожиданный формат".to_owned())?;
    let mut entries = Vec::with_capacity(items.len());
    for item in items {
        let id = item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let name = item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("без имени")
            .to_owned();
        let state = item
            .get("state")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let connectable = item
            .get("connectable")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        entries.push(VmInventoryEntry {
            id,
            name,
            state,
            connectable,
        });
    }
    Ok(entries)
}

fn format_vm_inventory_entries(items: &[VmInventoryEntry]) -> String {
    if items.is_empty() {
        return "VM не найдены. Проверьте Hyper-V/VirtualBox и права запуска.".to_owned();
    }
    let mut lines = Vec::with_capacity(items.len().min(8) + 1);
    lines.push(format!("Найдено VM: {}", items.len()));
    for item in items.iter().take(8) {
        let suffix = if item.connectable {
            "доступна"
        } else {
            "недоступна"
        };
        lines.push(format!(
            "{} · {} · {} · {suffix}",
            item.id, item.name, item.state
        ));
    }
    if items.len() > 8 {
        lines.push(format!("…ещё {}", items.len() - 8));
    }
    lines.join("\n")
}

fn main_content_max_width(width: f32) -> f32 {
    if width >= 1_600.0 {
        MAIN_CONTENT_MAX_WIDTH
    } else {
        width.max(640.0)
    }
}

fn main_content_side_padding(width: f32) -> f32 {
    if width >= 820.0 {
        MAIN_CONTENT_SIDE_PADDING
    } else {
        16.0
    }
}

fn smart_notification_accent(severity: &str, kind: &str) -> Color {
    match severity.trim().to_ascii_lowercase().as_str() {
        "error" => ACCENT,
        "warning" => Color::from_rgb(0.91, 0.58, 0.10),
        "success" => Color::from_rgb(0.12, 0.58, 0.35),
        _ => match kind.trim() {
            "support_ping" => Color::from_rgb(0.12, 0.58, 0.35),
            "poll" => Color::from_rgb(0.20, 0.42, 0.78),
            "config_update" => Color::from_rgb(0.91, 0.58, 0.10),
            _ => ACCENT,
        },
    }
}

fn smart_notification_type_label(kind: &str) -> &'static str {
    match kind.trim() {
        "support_ping" => "поддержка",
        "poll" => "опрос",
        "config_update" => "конфигурация",
        "banner" => "сообщение",
        _ => "уведомление",
    }
}

fn page_title<'a>(title: &'a str, subtitle: &'a str) -> Element<'a, Message> {
    column![text(title).size(30), text(subtitle).size(14).color(MUTED),]
        .spacing(4)
        .into()
}

fn empty_state<'a>(title: &'a str, subtitle: &'a str) -> Element<'a, Message> {
    container(column![text(title).size(14), text(subtitle).size(11).color(MUTED),].spacing(3))
        .padding(14)
        .width(Fill)
        .style(subtle_panel)
        .into()
}

fn device_initial(name: &str) -> String {
    name.chars()
        .next()
        .map(|character| character.to_uppercase().collect())
        .unwrap_or_else(|| "•".to_owned())
}

fn format_local_id(id: &str) -> String {
    let compact: String = id
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    compact
        .as_bytes()
        .chunks(3)
        .map(|chunk| String::from_utf8_lossy(chunk))
        .collect::<Vec<_>>()
        .join(" ")
}

fn peer_metadata(name: &str, platform: &str, version: &str) -> Option<String> {
    let mut parts = Vec::new();
    let name = name.trim();
    let platform = platform.trim();
    let version = version.trim();
    if !name.is_empty() && !name.eq_ignore_ascii_case("EvertyDesk Lite") {
        parts.push(name.to_owned());
    }
    if !platform.is_empty() {
        parts.push(platform.to_owned());
    }
    if !version.is_empty() {
        parts.push(format!("версия {version}"));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn normalize_remote_id(id: &str) -> String {
    id.chars()
        .filter(|character| !character.is_whitespace() && *character != '-')
        .collect::<String>()
        .to_lowercase()
}

fn remote_ids_match(left: &str, right: &str) -> bool {
    let left = normalize_remote_id(left);
    !left.is_empty() && left == normalize_remote_id(right)
}

fn host_state_color(state: &HostState) -> Color {
    match state {
        HostState::Ready => Color::from_rgb(0.12, 0.66, 0.37),
        HostState::Connecting => Color::from_rgb(0.91, 0.58, 0.10),
        HostState::Accepting(_) => Color::from_rgb(0.12, 0.48, 0.82),
        HostState::Error(_) => ACCENT,
        HostState::Idle => MUTED,
    }
}

fn recent_time(timestamp: u64) -> &'static str {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    match now.saturating_sub(timestamp) {
        0..=59 => "только что",
        60..=3_599 => "менее часа назад",
        3_600..=86_399 => "сегодня",
        86_400..=604_799 => "на этой неделе",
        _ => "ранее",
    }
}

fn recent_details(connection: &RecentConnection) -> String {
    let direction = match connection.direction {
        ConnectionDirection::Outgoing => "Исходящая",
        ConnectionDirection::Incoming => "Входящая",
    };
    let mut details = format!("{direction} · {}", recent_time(connection.last_used_unix));
    if connection.duration_seconds > 0 {
        details.push_str(" · ");
        details.push_str(&format_duration(connection.duration_seconds));
    }
    if connection.reconnect_count > 0 {
        details.push_str(&format!(" · восстановлений {}", connection.reconnect_count));
    }
    if !connection.last_end_reason.is_empty() {
        details.push_str(" · ");
        details.push_str(&connection.last_end_reason);
    }
    details
}

fn header_style(_theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(SURFACE.into()),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 0.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.05),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        ..Default::default()
    }
}

fn header_status_style(_theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(CANVAS.into()),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 20.0.into(),
        },
        ..Default::default()
    }
}

fn tooltip_panel(_theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Color::from_rgb(0.12, 0.125, 0.14).into()),
        border: Border {
            color: Color::from_rgb(0.28, 0.29, 0.32),
            width: 1.0,
            radius: 6.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.22),
            offset: Vector::new(0.0, 3.0),
            blur_radius: 10.0,
        },
        ..Default::default()
    }
}

fn quick_bar_style(_theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Color::from_rgb(0.10, 0.105, 0.115).into()),
        border: Border {
            color: Color::from_rgb(0.20, 0.21, 0.23),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

fn card_style(_theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(SURFACE.into()),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 12.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.06),
            offset: Vector::new(0.0, 3.0),
            blur_radius: 12.0,
        },
        ..Default::default()
    }
}

fn subtle_panel(_theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(CANVAS.into()),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

fn separator_style(_theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(LINE.into()),
        ..Default::default()
    }
}

fn status_bar(_theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(CANVAS.into()),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

fn accent_tile(_theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(ACCENT.into()),
        border: border::rounded(9),
        ..Default::default()
    }
}

fn accent_pill(_theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(ACCENT.into()),
        border: border::rounded(20),
        ..Default::default()
    }
}

fn device_icon(_theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Color::from_rgb(1.0, 0.93, 0.94).into()),
        border: border::rounded(9),
        ..Default::default()
    }
}

fn accent_button(
    theme: &Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    let mut style = iced::widget::button::primary(theme, status);
    style.text_color = Color::WHITE;
    style.border.radius = 8.0.into();
    style
}

fn selected_segment(
    theme: &Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    accent_button(theme, status)
}

fn quiet_button(
    _theme: &Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    let base = iced::widget::button::Style {
        background: Some(SURFACE.into()),
        text_color: TEXT,
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 7.0.into(),
        },
        ..Default::default()
    };
    match status {
        iced::widget::button::Status::Active => base,
        iced::widget::button::Status::Hovered => iced::widget::button::Style {
            background: Some(Color::from_rgb(1.0, 0.95, 0.955).into()),
            text_color: ACCENT,
            border: Border {
                color: ACCENT,
                ..base.border
            },
            ..base
        },
        iced::widget::button::Status::Pressed => iced::widget::button::Style {
            background: Some(Color::from_rgb(1.0, 0.90, 0.91).into()),
            text_color: ACCENT,
            ..base
        },
        iced::widget::button::Status::Disabled => iced::widget::button::Style {
            background: Some(CANVAS.into()),
            text_color: MUTED.scale_alpha(0.55),
            ..base
        },
    }
}

fn input_style(
    _theme: &Theme,
    status: iced::widget::text_input::Status,
) -> iced::widget::text_input::Style {
    let base = iced::widget::text_input::Style {
        background: Background::Color(SURFACE),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 7.0.into(),
        },
        icon: MUTED,
        placeholder: Color::from_rgb(0.58, 0.61, 0.67),
        value: TEXT,
        selection: Color::from_rgba(0.91, 0.13, 0.16, 0.22),
    };
    match status {
        iced::widget::text_input::Status::Active => base,
        iced::widget::text_input::Status::Hovered => iced::widget::text_input::Style {
            border: Border {
                color: Color::from_rgb(0.68, 0.70, 0.74),
                ..base.border
            },
            ..base
        },
        iced::widget::text_input::Status::Focused { .. } => iced::widget::text_input::Style {
            border: Border {
                color: ACCENT,
                width: 1.5,
                ..base.border
            },
            ..base
        },
        iced::widget::text_input::Status::Disabled => iced::widget::text_input::Style {
            background: Background::Color(CANVAS),
            value: MUTED,
            ..base
        },
    }
}

fn quick_input_style(
    _theme: &Theme,
    status: iced::widget::text_input::Status,
) -> iced::widget::text_input::Style {
    let base = iced::widget::text_input::Style {
        background: Background::Color(Color::from_rgb(0.13, 0.135, 0.145)),
        border: Border {
            color: Color::from_rgb(0.28, 0.29, 0.31),
            width: 1.0,
            radius: 6.0.into(),
        },
        icon: Color::from_rgb(0.72, 0.74, 0.77),
        placeholder: Color::from_rgb(0.58, 0.61, 0.66),
        value: Color::WHITE,
        selection: Color::from_rgba(0.91, 0.13, 0.16, 0.38),
    };
    match status {
        iced::widget::text_input::Status::Active => base,
        iced::widget::text_input::Status::Hovered => iced::widget::text_input::Style {
            border: Border {
                color: Color::from_rgb(0.46, 0.47, 0.50),
                ..base.border
            },
            ..base
        },
        iced::widget::text_input::Status::Focused { .. } => iced::widget::text_input::Style {
            border: Border {
                color: ACCENT,
                width: 1.5,
                ..base.border
            },
            ..base
        },
        iced::widget::text_input::Status::Disabled => iced::widget::text_input::Style {
            value: Color::from_rgb(0.55, 0.57, 0.60),
            ..base
        },
    }
}

fn danger_text_button(
    theme: &Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    let mut style = iced::widget::button::text(theme, status);
    style.text_color = ACCENT;
    style.border.radius = 7.0.into();
    style
}

fn selected_nav_button(
    theme: &Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    let mut style = iced::widget::button::text(theme, status);
    style.text_color = ACCENT;
    style.background = Some(Color::from_rgb(1.0, 0.93, 0.94).into());
    style.border.radius = 7.0.into();
    style
}

fn navigation_button(
    theme: &Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    let mut style = iced::widget::button::text(theme, status);
    style.text_color = MUTED;
    style.border.radius = 7.0.into();
    style
}

fn cloud_contact_rows(
    contacts: Vec<ContactEntry>,
) -> impl Iterator<Item = (String, String, String, Vec<String>)> {
    contacts
        .into_iter()
        .map(|contact| (contact.name, contact.remote_id, contact.note, contact.tags))
}

fn parse_contact_tags(value: &str) -> Vec<String> {
    let raw_tags: Vec<String> = value
        .split([',', ';'])
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    normalize_contact_tags(&raw_tags)
}

fn format_contact_tags(tags: &[String]) -> String {
    tags.join(", ")
}

fn format_group_path(group: &str) -> String {
    group_path_segments(group).join(" / ")
}

fn group_path_segments(group: &str) -> Vec<String> {
    group
        .split(['/', '\\', '>'])
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn group_path_ancestors(group: &str) -> Vec<String> {
    let segments = group_path_segments(group);
    let mut ancestors = Vec::with_capacity(segments.len());
    for index in 0..segments.len() {
        ancestors.push(segments[..=index].join(" / "));
    }
    ancestors
}

fn group_path_depth(group: &str) -> usize {
    group_path_segments(group).len().saturating_sub(1)
}

fn group_leaf_label(group: &str) -> String {
    group_path_segments(group)
        .pop()
        .unwrap_or_else(|| format_group_path("Без группы"))
}

fn address_book_group_indent(depth: usize) -> f32 {
    (depth.min(4) as f32) * 14.0
}

fn contact_group_matches_filter(contact_group: &str, selected_group: &str) -> bool {
    let contact_group = format_group_path(contact_group);
    let selected_group = format_group_path(selected_group);
    if contact_group.is_empty() || selected_group.is_empty() {
        return contact_group == selected_group;
    }
    contact_group == selected_group
        || contact_group
            .strip_prefix(&selected_group)
            .is_some_and(|tail| tail.starts_with(" / "))
}

fn contact_details(
    remote_id: String,
    note: String,
    group: String,
    tags: Vec<String>,
) -> Element<'static, Message> {
    let mut details = column![text(remote_id).size(12).color(MUTED)].spacing(2);
    if !note.is_empty() {
        details = details.push(text(note).size(11).color(MUTED));
    }
    let group_label = if group.trim().is_empty() {
        String::new()
    } else {
        format_group_path(&group)
    };
    let mut chips = row![].spacing(5).align_y(Alignment::Center);
    if !group_label.is_empty() {
        chips = chips.push(contact_filter_chip(
            icondata::LuFolder,
            group_label.clone(),
            Message::SelectAddressBookFilter(AddressBookFilter::Group(group_label)),
        ));
    }
    if !tags.is_empty() {
        for tag in tags.into_iter().take(4) {
            chips = chips.push(contact_filter_chip(
                icondata::LuTag,
                format!("#{tag}"),
                Message::SelectAddressBookFilter(AddressBookFilter::Tag(tag)),
            ));
        }
    }
    details = details.push(chips);
    details.into()
}

fn contact_filter_chip(
    icon: icondata::Icon,
    label: String,
    message: Message,
) -> Element<'static, Message> {
    button(
        row![
            lucide_icon(icon, 11.0, ACCENT),
            text(label).size(10).color(ACCENT)
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .on_press(message)
    .padding([2, 6])
    .style(move |_theme, _status| iced::widget::button::Style {
        text_color: ACCENT,
        background: Some(Color::from_rgba(ACCENT.r, ACCENT.g, ACCENT.b, 0.07).into()),
        border: Border {
            radius: 999.0.into(),
            width: 1.0,
            color: Color::from_rgba(ACCENT.r, ACCENT.g, ACCENT.b, 0.16),
        },
        ..Default::default()
    })
    .into()
}

fn contact_detail_panel(contact: Contact, language: UiLanguage) -> Element<'static, Message> {
    let mut metadata = column![row![
        container(text(device_initial(&contact.name)).color(ACCENT))
            .center_x(Length::Fixed(40.0))
            .center_y(Length::Fixed(40.0))
            .style(device_icon),
        column![
            text(contact.name.clone()).size(17),
            text(format_local_id(&contact.remote_id))
                .size(12)
                .color(MUTED),
        ]
        .spacing(2)
        .width(Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center),]
    .spacing(10);
    if !contact.note.trim().is_empty() {
        metadata = metadata.push(text(contact.note.clone()).size(12).color(MUTED));
    }
    if !contact.group.trim().is_empty() {
        let group_label = format_group_path(&contact.group);
        metadata = metadata.push(contact_filter_chip(
            icondata::LuFolder,
            group_label.clone(),
            Message::SelectAddressBookFilter(AddressBookFilter::Group(group_label)),
        ));
    }
    if !contact.tags.is_empty() {
        let mut tag_row = row![].spacing(5).align_y(Alignment::Center);
        for tag in contact.tags.iter().take(5) {
            tag_row = tag_row.push(contact_filter_chip(
                icondata::LuTag,
                format!("#{tag}"),
                Message::SelectAddressBookFilter(AddressBookFilter::Tag(tag.clone())),
            ));
        }
        metadata = metadata.push(tag_row);
    }

    container(
        column![
            row![
                column![
                    text(tr(language, TextKey::AddressBookContactDetails)).size(18),
                    text(tr(language, TextKey::AddressBookQuickActions))
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(2)
                .width(Fill),
                icon_action(
                    icondata::LuX,
                    tr(language, TextKey::AddressBookHideDetails),
                    Message::SelectContact(String::new()),
                    true,
                ),
            ]
            .align_y(Alignment::Center),
            metadata,
            row![
                icon_action(
                    icondata::LuCopy,
                    tr(language, TextKey::AddressBookCopyId),
                    Message::CopyContactId(contact.remote_id.clone()),
                    false,
                ),
                icon_action(
                    icondata::LuMousePointer2,
                    tr(language, TextKey::AddressBookUseAddress),
                    Message::SelectRemote(contact.remote_id.clone()),
                    false,
                ),
                icon_action(
                    icondata::LuArrowRight,
                    tr(language, TextKey::AddressBookConnect),
                    Message::ConnectRemote(contact.remote_id.clone()),
                    false,
                ),
                icon_action(
                    icondata::LuPencil,
                    tr(language, TextKey::AddressBookEditContact),
                    Message::EditContact(contact.remote_id.clone()),
                    false,
                ),
                icon_action(
                    icondata::LuStar,
                    if contact.favorite {
                        tr(language, TextKey::AddressBookRemoveFromFavorites)
                    } else {
                        tr(language, TextKey::AddressBookAddToFavorites)
                    },
                    Message::ToggleFavorite(contact.remote_id.clone()),
                    contact.favorite,
                ),
                icon_action(
                    icondata::LuTrash2,
                    tr(language, TextKey::AddressBookDeleteContact),
                    Message::RemoveContact(contact.remote_id),
                    true,
                ),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        ]
        .spacing(12),
    )
    .padding(16)
    .width(Fill)
    .style(card_style)
    .into()
}

fn form_suggestions(
    label: &'static str,
    chips: Row<'static, Message>,
) -> Element<'static, Message> {
    row![
        text(label).size(10).color(MUTED).width(Length::Fixed(48.0)),
        chips
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn address_book_filter_badge(
    filter: &AddressBookFilter,
    language: UiLanguage,
) -> Element<'static, Message> {
    if matches!(filter, AddressBookFilter::All) {
        return Space::new().width(Length::Shrink).into();
    }
    let label = address_book_filter_display(filter, language);
    row![
        contact_filter_chip(
            icondata::LuListFilter,
            label,
            Message::ClearAddressBookFilter
        ),
        icon_action(
            icondata::LuX,
            tr(language, TextKey::AddressBookResetFilter),
            Message::ClearAddressBookFilter,
            true,
        ),
    ]
    .spacing(3)
    .align_y(Alignment::Center)
    .into()
}

fn address_book_filter_uses_recent_panel(filter: &AddressBookFilter) -> bool {
    matches!(filter, AddressBookFilter::All)
}

fn has_login_provider(options: &[String], provider: &str) -> bool {
    let expected = format!("oidc/{}", provider.trim().to_lowercase());
    options
        .iter()
        .any(|option| option.trim().eq_ignore_ascii_case(&expected))
}

fn current_user_entitlements(user: &serde_json::Value) -> (AccountEntitlements, String) {
    let Some(entitlements) = user
        .get("entitlements")
        .and_then(serde_json::Value::as_object)
    else {
        return (
            AccountEntitlements::default(),
            "Права аккаунта: entitlements не получены".to_owned(),
        );
    };
    if entitlements.is_empty() {
        return (
            AccountEntitlements {
                known: true,
                ..AccountEntitlements::default()
            },
            "Права аккаунта: активные тарифные фичи не найдены".to_owned(),
        );
    }
    let parsed = AccountEntitlements {
        known: true,
        smart_agent: entitlement_is_enabled(entitlements.get("has_smart_agent")),
        yandex_sso: entitlement_is_enabled(entitlements.get("has_yandex_sso")),
        ldap: entitlement_is_enabled(entitlements.get("has_ldap")),
        client_builder: entitlement_is_enabled(entitlements.get("has_client_builder")),
        invoice_billing: entitlement_is_enabled(entitlements.get("has_invoice_billing")),
        vm: entitlement_is_enabled(entitlements.get("vm_mode")),
        priority_support: entitlement_is_enabled(entitlements.get("has_priority_support")),
        audit: entitlement_is_enabled(entitlements.get("has_audit")),
        branding: entitlement_is_enabled(entitlements.get("has_branded_client")),
        vm_slots: entitlements.get("max_vm_slots").and_then(entitlement_u32),
    };
    (parsed.clone(), account_entitlements_summary(&parsed))
}

#[cfg(test)]
fn current_user_entitlements_summary(user: &serde_json::Value) -> String {
    current_user_entitlements(user).1
}

fn account_entitlements_summary(entitlements: &AccountEntitlements) -> String {
    if !entitlements.known {
        return "Права аккаунта: entitlements не получены".to_owned();
    }
    let labels = [
        (entitlements.smart_agent, "Smart Agent"),
        (entitlements.yandex_sso, "Yandex SSO"),
        (entitlements.ldap, "LDAP"),
        (entitlements.client_builder, "Client builder"),
        (entitlements.invoice_billing, "Invoice billing"),
        (entitlements.vm, "VM"),
        (entitlements.priority_support, "Priority support"),
        (entitlements.audit, "Audit"),
        (entitlements.branding, "Branding"),
    ];
    let mut active: Vec<String> = Vec::new();
    for (enabled, label) in labels {
        if enabled {
            active.push(label.to_owned());
        }
    }
    if let Some(slots) = entitlements.vm_slots {
        active.push(format!("VM slots {slots}"));
    }
    if active.is_empty() {
        "Права аккаунта: тарифные фичи выключены".to_owned()
    } else {
        format!("Права аккаунта: {}", active.join(", "))
    }
}

fn entitlement_u32(value: &serde_json::Value) -> Option<u32> {
    match value {
        serde_json::Value::Number(value) => {
            value.as_u64().and_then(|value| u32::try_from(value).ok())
        }
        serde_json::Value::String(value) => value.trim().parse::<u32>().ok(),
        _ => None,
    }
}

fn account_entitlement_badges(entitlements: &AccountEntitlements) -> Element<'static, Message> {
    let mut badges = row![].spacing(5).align_y(Alignment::Center);
    if !entitlements.known {
        return badges.push(vm_badge("Права неизвестны", MUTED)).into();
    }

    let active = [
        (entitlements.smart_agent, "Smart Agent"),
        (entitlements.yandex_sso, "Yandex SSO"),
        (entitlements.ldap, "LDAP"),
        (entitlements.client_builder, "Builder"),
        (entitlements.invoice_billing, "Billing"),
        (entitlements.vm, "VM"),
        (entitlements.priority_support, "Support"),
        (entitlements.audit, "Audit"),
        (entitlements.branding, "Branding"),
    ];
    let mut has_any = false;
    for (enabled, label) in active {
        if enabled {
            has_any = true;
            badges = badges.push(vm_badge(label, ACCENT));
        }
    }
    if let Some(slots) = entitlements.vm_slots {
        has_any = true;
        badges = badges.push(vm_badge(format!("VM slots {slots}"), ACCENT));
    }
    if !has_any {
        badges = badges.push(vm_badge("Тарифные фичи выключены", MUTED));
    }
    badges.into()
}

fn entitlement_is_enabled(value: Option<&serde_json::Value>) -> bool {
    match value {
        Some(serde_json::Value::Bool(value)) => *value,
        Some(serde_json::Value::String(value)) => {
            matches!(
                value.trim().to_lowercase().as_str(),
                "true" | "1" | "yes" | "on"
            )
        }
        Some(serde_json::Value::Number(value)) => value.as_i64().is_some_and(|value| value > 0),
        _ => false,
    }
}

fn commercial_feature_note(
    known: bool,
    enabled: bool,
    label: &'static str,
) -> Option<&'static str> {
    if !known {
        Some("Войдите в аккаунт, чтобы EvertyDesk показал тарифные права для этой функции.")
    } else if enabled {
        None
    } else {
        match label {
            "VM" => Some(
                "VM не отмечен в entitlements аккаунта; пока режим доступен как локальная функция.",
            ),
            "Game/VM" => {
                Some("Game/VM не отмечен в entitlements аккаунта; пока подключение не блокируется.")
            }
            "Smart Agent" => Some(
                "Smart Agent не отмечен в entitlements аккаунта; проверьте тариф перед production.",
            ),
            _ => Some("Функция не отмечена в entitlements аккаунта; пока ограничение мягкое."),
        }
    }
}

fn address_book_filter_label(filter: &AddressBookFilter, language: UiLanguage) -> &'static str {
    match filter {
        AddressBookFilter::All => tr(language, TextKey::AddressBookAllContacts),
        AddressBookFilter::Favorites => tr(language, TextKey::AddressBookFavorites),
        AddressBookFilter::Recent => tr(language, TextKey::AddressBookRecentContacts),
        AddressBookFilter::Group(_) => tr(language, TextKey::AddressBookGroupContacts),
        AddressBookFilter::Tag(_) => tr(language, TextKey::AddressBookTaggedContacts),
    }
}

fn address_book_filter_display(filter: &AddressBookFilter, language: UiLanguage) -> String {
    match filter {
        AddressBookFilter::All => tr(language, TextKey::AddressBookAllShort).to_owned(),
        AddressBookFilter::Favorites => tr(language, TextKey::AddressBookFavorites).to_owned(),
        AddressBookFilter::Recent => tr(language, TextKey::AddressBookRecent).to_owned(),
        AddressBookFilter::Group(group) => group.clone(),
        AddressBookFilter::Tag(tag) => format!("#{tag}"),
    }
}

fn address_book_count_summary(language: UiLanguage, visible: usize, total: usize) -> String {
    match language {
        UiLanguage::Russian => format!("{visible} показано · {total} всего"),
        UiLanguage::English => format!("{visible} shown · {total} total"),
    }
}

fn address_book_group_suggestions(contacts: &[Contact], limit: usize) -> Vec<String> {
    contacts
        .iter()
        .flat_map(|contact| group_path_ancestors(&contact.group))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(limit)
        .collect()
}

fn address_book_tag_suggestions(contacts: &[Contact], limit: usize) -> Vec<String> {
    contacts
        .iter()
        .flat_map(|contact| contact.tags.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(limit)
        .collect()
}

fn normalized_recent_ids(recent: &[RecentConnection]) -> BTreeSet<String> {
    recent
        .iter()
        .map(|connection| normalize_remote_id(&connection.remote_id))
        .filter(|remote_id| !remote_id.is_empty())
        .collect()
}

fn selected_contact_for_filter<'a>(
    contacts: &'a [Contact],
    selected_id: Option<&str>,
    filter: &AddressBookFilter,
    recent_ids: &BTreeSet<String>,
) -> Option<&'a Contact> {
    let selected_id = selected_id?.trim();
    if selected_id.is_empty() {
        return None;
    }
    contacts.iter().find(|contact| {
        remote_ids_match(&contact.remote_id, selected_id)
            && contact_matches_address_book_filter(contact, filter, recent_ids)
    })
}

fn selected_contact_after_filter_change(
    contacts: &[Contact],
    selected_id: Option<&str>,
    filter: &AddressBookFilter,
    recent_ids: &BTreeSet<String>,
    text_filter: &str,
) -> Option<String> {
    selected_contact_for_filter(contacts, selected_id, filter, recent_ids)
        .filter(|contact| contact_matches_text_filter(contact, text_filter))
        .map(|contact| contact.remote_id.clone())
}

fn contact_matches_address_book_filter(
    contact: &Contact,
    filter: &AddressBookFilter,
    recent_ids: &BTreeSet<String>,
) -> bool {
    match filter {
        AddressBookFilter::All => true,
        AddressBookFilter::Favorites => contact.favorite,
        AddressBookFilter::Recent => recent_ids.contains(&normalize_remote_id(&contact.remote_id)),
        AddressBookFilter::Group(group) => {
            if contact.group.trim().is_empty() {
                format_group_path(group) == format_group_path("Без группы")
            } else {
                contact_group_matches_filter(&contact.group, group)
            }
        }
        AddressBookFilter::Tag(tag) => contact
            .tags
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(tag)),
    }
}

fn contact_matches_text_filter(contact: &Contact, filter: &str) -> bool {
    filter.is_empty()
        || contact.name.to_lowercase().contains(filter)
        || contact.remote_id.to_lowercase().contains(filter)
        || contact.group.to_lowercase().contains(filter)
        || format_group_path(&contact.group)
            .to_lowercase()
            .contains(filter)
        || contact
            .tags
            .iter()
            .any(|tag| tag.to_lowercase().contains(filter))
        || contact.note.to_lowercase().contains(filter)
}

fn spawn_check_for_update(source: UpdateSource, current_version: String) {
    let events = event_bus().0.clone();
    thread::spawn(move || {
        let result = match source {
            UpdateSource::ManifestUrl(manifest_url) => {
                updater::check_for_update(&manifest_url, &current_version)
            }
            UpdateSource::GithubRelease(owner_repo) => {
                updater::check_github_release_for_update(&owner_repo, &current_version)
            }
        };
        let _ = events.send_blocking(ProcessEvent::Updater(UpdaterEvent::Checked(result)));
    });
}

fn spawn_download_update(manifest: updater::UpdateManifest, destination_dir: PathBuf) {
    let events = event_bus().0.clone();
    thread::spawn(move || {
        let result = updater::download_and_verify(&manifest, &destination_dir);
        let _ = events.send_blocking(ProcessEvent::Updater(UpdaterEvent::Downloaded(result)));
    });
}

fn spawn_smart_agent_heartbeat(machine_id: String, local_id: String, service_key: String) {
    let events = event_bus().0.clone();
    thread::spawn(move || {
        let request = HeartbeatRequest {
            machine_id,
            service_key,
            hostname: local_hostname(),
            os: std::env::consts::OS.to_owned(),
            os_version: std::env::var("OS").unwrap_or_else(|_| std::env::consts::OS.to_owned()),
            rustdesk_id: normalize_remote_id(&local_id),
        };
        let result = smart_agent::heartbeat(SMART_AGENT_API_URL, &request);
        let _ = events.send_blocking(ProcessEvent::SmartAgent(SmartAgentEvent::Heartbeat(result)));
    });
}

fn spawn_smart_agent_inbox(machine_id: String, service_key: String) {
    let events = event_bus().0.clone();
    thread::spawn(move || {
        let result = smart_agent::inbox(SMART_AGENT_API_URL, &machine_id, &service_key);
        let _ = events.send_blocking(ProcessEvent::SmartAgent(SmartAgentEvent::Inbox(result)));
    });
}

fn spawn_smart_agent_ack(machine_id: String, notification_id: u64) {
    let events = event_bus().0.clone();
    thread::spawn(move || {
        let result = smart_agent::acknowledge(SMART_AGENT_API_URL, &machine_id, notification_id);
        let _ = events.send_blocking(ProcessEvent::SmartAgent(SmartAgentEvent::Acknowledged {
            notification_id,
            result,
        }));
    });
}

fn spawn_smart_agent_vote(machine_id: String, notification_id: u64, vote: String) {
    let events = event_bus().0.clone();
    thread::spawn(move || {
        let result = smart_agent::vote(SMART_AGENT_API_URL, &machine_id, notification_id, &vote);
        let _ = events.send_blocking(ProcessEvent::SmartAgent(SmartAgentEvent::Voted {
            notification_id,
            result,
        }));
    });
}

fn spawn_smart_agent_operators(service_key: String) {
    let events = event_bus().0.clone();
    thread::spawn(move || {
        let result = smart_agent::operators(SMART_AGENT_API_URL, &service_key);
        let _ = events.send_blocking(ProcessEvent::SmartAgent(SmartAgentEvent::OperatorsLoaded(
            result,
        )));
    });
}

fn spawn_smart_agent_support_request(request: SupportRequest) {
    let events = event_bus().0.clone();
    thread::spawn(move || {
        let result = smart_agent::request_support(SMART_AGENT_API_URL, &request);
        let _ = events.send_blocking(ProcessEvent::SmartAgent(SmartAgentEvent::SupportRequested(
            result,
        )));
    });
}

fn spawn_smart_agent_support_response(
    machine_id: String,
    service_key: String,
    notification_id: u64,
    request_id: u64,
    action: smart_agent::SupportAction,
    from_remote_id: String,
) {
    let events = event_bus().0.clone();
    thread::spawn(move || {
        let result = smart_agent::respond_to_support(
            SMART_AGENT_API_URL,
            &machine_id,
            &service_key,
            request_id,
            action,
            "",
        )
        .and_then(|()| smart_agent::acknowledge(SMART_AGENT_API_URL, &machine_id, notification_id));
        let _ = events.send_blocking(ProcessEvent::SmartAgent(
            SmartAgentEvent::SupportResponded {
                notification_id,
                action,
                from_remote_id,
                result,
            },
        ));
    });
}

fn local_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "EvertyDesk device".to_owned())
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    let sanitized: String = value
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .take(max_chars)
        .collect();
    if value.chars().count() > max_chars {
        format!("{sanitized}…")
    } else {
        sanitized
    }
}

fn is_safe_notification_link(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("https://") || trimmed.starts_with("http://")
}

fn open_system_browser(url: &str) -> Result<(), String> {
    if !is_safe_notification_link(url) {
        return Err("небезопасная ссылка".to_owned());
    }
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler").arg(url);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn open_about_link(url: &'static str, label: &'static str) -> String {
    match open_system_browser(url) {
        Ok(()) => format!("{label}: ссылка открыта"),
        Err(error) => format!("{label}: не удалось открыть ссылку: {error}"),
    }
}

fn sanitize_support_message(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\t'))
        .take(MAX_SUPPORT_MESSAGE_CHARS)
        .collect()
}

fn support_message_counter(value: &str) -> String {
    format!("{}/{}", value.chars().count(), MAX_SUPPORT_MESSAGE_CHARS)
}

fn spawn_address_book_sign_in(
    api_url: String,
    account: String,
    password: String,
    local_id: String,
    machine_id: String,
) -> Result<(), String> {
    let events = event_bus().0.clone();
    let password = Zeroizing::new(password);
    thread::Builder::new()
        .name("address-book-login".to_owned())
        .spawn(move || {
            let result =
                evertydesk_core::address_book::login(
                    &api_url,
                    &account,
                    &password,
                    &local_id,
                    &machine_id,
                )
                .and_then(|access_token| {
                    evertydesk_core::address_book::personal_ab_guid(&api_url, &access_token)
                        .and_then(|guid| {
                            evertydesk_core::address_book::peers(&api_url, &access_token, &guid)
                                .map(|contacts| AddressBookEvent::SignedIn {
                                    account,
                                    access_token,
                                    guid,
                                    contacts,
                                })
                        })
                });
            let event = result.unwrap_or_else(AddressBookEvent::Failed);
            let _ = events.send_blocking(ProcessEvent::AddressBook(event));
        })
        .map(|_| ())
        .map_err(|error| format!("Не удалось запустить авторизацию: {error}"))
}

fn spawn_address_book_sync(
    api_url: String,
    access_token: String,
    cached_guid: String,
) -> Result<(), String> {
    let events = event_bus().0.clone();
    let access_token = Zeroizing::new(access_token);
    thread::Builder::new()
        .name("address-book-sync".to_owned())
        .spawn(move || {
            let result = if cached_guid.trim().is_empty() {
                evertydesk_core::address_book::personal_ab_guid(&api_url, &access_token)
            } else {
                Ok(cached_guid)
            }
            .and_then(|guid| {
                evertydesk_core::address_book::peers(&api_url, &access_token, &guid)
                    .map(|contacts| AddressBookEvent::Synced { guid, contacts })
            });
            let event = result.unwrap_or_else(AddressBookEvent::Failed);
            let _ = events.send_blocking(ProcessEvent::AddressBook(event));
        })
        .map(|_| ())
        .map_err(|error| format!("Не удалось запустить синхронизацию: {error}"))
}

fn spawn_address_book_token_load(
    api_url: String,
    account: String,
    access_token: String,
) -> Result<(), String> {
    let events = event_bus().0.clone();
    let access_token = Zeroizing::new(access_token);
    thread::Builder::new()
        .name("address-book-oidc-load".to_owned())
        .spawn(move || {
            let result = evertydesk_core::address_book::personal_ab_guid(&api_url, &access_token)
                .and_then(|guid| {
                    evertydesk_core::address_book::peers(&api_url, &access_token, &guid).map(
                        |contacts| AddressBookEvent::SignedIn {
                            account,
                            access_token: access_token.to_string(),
                            guid,
                            contacts,
                        },
                    )
                });
            let event = result.unwrap_or_else(AddressBookEvent::Failed);
            let _ = events.send_blocking(ProcessEvent::AddressBook(event));
        })
        .map(|_| ())
        .map_err(|error| format!("Не удалось запустить загрузку адресной книги: {error}"))
}

fn spawn_current_user_refresh(api_url: String, access_token: String) {
    if access_token.trim().is_empty() {
        return;
    }
    let events = event_bus().0.clone();
    thread::spawn(move || {
        let access_token = Zeroizing::new(access_token);
        let result = evertydesk_core::address_book::current_user(&api_url, &access_token)
            .map(|user| current_user_entitlements(&user));
        let _ = events.send_blocking(ProcessEvent::CurrentUserRefreshed(result));
    });
}

fn spawn_address_book_logout(
    api_url: String,
    access_token: String,
    local_id: String,
) -> Result<(), String> {
    let events = event_bus().0.clone();
    let access_token = Zeroizing::new(access_token);
    thread::Builder::new()
        .name("address-book-logout".to_owned())
        .spawn(move || {
            let result = evertydesk_core::address_book::logout(&api_url, &access_token, &local_id);
            let _ = events.send_blocking(ProcessEvent::AddressBook(AddressBookEvent::LoggedOut(
                result,
            )));
        })
        .map(|_| ())
        .map_err(|error| format!("Не удалось запустить выход из аккаунта: {error}"))
}

fn sanitize_address_book_error(error: &str) -> String {
    error
        .chars()
        .filter(|character| !character.is_control())
        .take(500)
        .collect()
}

fn event_bus() -> &'static EventBus {
    EVENT_BUS.get_or_init(async_channel::unbounded)
}

fn claim_single_instance(start_in_background: bool) -> io::Result<bool> {
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, SINGLE_INSTANCE_PORT));
    match TcpListener::bind(address) {
        Ok(listener) => {
            start_single_instance_listener(listener)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
            let notify_result = notify_primary_instance(address, start_in_background);
            if should_start_after_single_instance_notify(&notify_result) {
                if let Err(error) = notify_result {
                    eprintln!(
                        "[launcher] stale single-instance port; starting without focus listener: {error}"
                    );
                }
                Ok(true)
            } else {
                Ok(false)
            }
        }
        Err(error) => Err(error),
    }
}

fn start_single_instance_listener(listener: TcpListener) -> io::Result<()> {
    let events = event_bus().0.clone();
    thread::Builder::new()
        .name("evertydesk-single-instance".to_owned())
        .spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut stream) = connection else {
                    continue;
                };
                let _ = stream.set_read_timeout(Some(SINGLE_INSTANCE_TIMEOUT));
                let _ = stream.set_write_timeout(Some(SINGLE_INSTANCE_TIMEOUT));
                let request = {
                    let mut reader = BufReader::new(&mut stream);
                    read_bounded_line(&mut reader, 64).ok().flatten()
                };
                if request.as_deref().is_some_and(is_single_instance_request) {
                    let response = format!("{SINGLE_INSTANCE_RESPONSE}\n");
                    if stream.write_all(response.as_bytes()).is_ok() {
                        let _ = stream.flush();
                        if request.as_deref().is_some_and(is_focus_request) {
                            let _ = events.send_blocking(ProcessEvent::SecondInstance);
                        }
                    }
                }
            }
        })?;
    Ok(())
}

fn notify_primary_instance(address: SocketAddr, start_in_background: bool) -> io::Result<()> {
    let mut stream = TcpStream::connect_timeout(&address, SINGLE_INSTANCE_TIMEOUT)?;
    stream.set_read_timeout(Some(SINGLE_INSTANCE_TIMEOUT))?;
    stream.set_write_timeout(Some(SINGLE_INSTANCE_TIMEOUT))?;
    let request = if start_in_background {
        SINGLE_INSTANCE_BACKGROUND_REQUEST
    } else {
        SINGLE_INSTANCE_REQUEST
    };
    stream.write_all(format!("{request}\n").as_bytes())?;
    stream.flush()?;
    let response = {
        let mut reader = BufReader::new(&mut stream);
        read_bounded_line(&mut reader, 64)?
    };
    if response.as_deref() == Some(SINGLE_INSTANCE_RESPONSE) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "single-instance port is occupied by an unknown process",
        ))
    }
}

fn should_start_after_single_instance_notify(result: &io::Result<()>) -> bool {
    result.is_err()
}

fn log_launcher_startup() {
    eprintln!(
        "[launcher] startup pid={} os={} arch={} WGPU_BACKEND={} ICED_BACKEND={} RUST_BACKTRACE={}",
        std::process::id(),
        env::consts::OS,
        env::consts::ARCH,
        env_value_for_log("WGPU_BACKEND"),
        env_value_for_log("ICED_BACKEND"),
        env_value_for_log("RUST_BACKTRACE")
    );
}

fn env_value_for_log(key: &str) -> String {
    env::var(key)
        .map(|value| sanitize_startup_env_value(&value))
        .unwrap_or_else(|_| "-".to_owned())
}

fn sanitize_startup_env_value(value: &str) -> String {
    let mut sanitized = String::new();
    let mut truncated = false;
    for character in value.chars() {
        if sanitized.chars().count() >= MAX_STARTUP_ENV_VALUE_CHARS {
            truncated = true;
            break;
        }
        if character.is_control() {
            sanitized.push(' ');
        } else {
            sanitized.push(character);
        }
    }
    if truncated {
        sanitized.push('…');
    }
    sanitized.trim().to_owned()
}

fn is_focus_request(request: &str) -> bool {
    request == SINGLE_INSTANCE_REQUEST
}

fn is_single_instance_request(request: &str) -> bool {
    request == SINGLE_INSTANCE_REQUEST || request == SINGLE_INSTANCE_BACKGROUND_REQUEST
}

fn clipboard_fingerprint(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.len().hash(&mut hasher);
    value.hash(&mut hasher);
    hasher.finish()
}

fn clear_matching_clipboard(expected_fingerprint: u64) -> bool {
    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        return false;
    };
    let Ok(mut current) = clipboard.get_text() else {
        return false;
    };
    let matches = clipboard_fingerprint(&current) == expected_fingerprint;
    let cleared = matches && clipboard.set_text(String::new()).is_ok();
    current.zeroize();
    cleared
}

fn approval_matches(pending: Option<&PendingApproval>, peer_id: &str, token: u64) -> bool {
    pending.is_some_and(|pending| pending.token == token && pending.peer_id == peer_id)
}

fn apply_session_permissions_ack(
    session: Option<&mut IncomingSession>,
    peer_id: &str,
    session_id: u64,
    input_blocked: bool,
    clipboard_allowed: bool,
) -> bool {
    let Some(session) =
        session.filter(|session| session.peer_id == peer_id && session.session_id == session_id)
    else {
        return false;
    };
    session.input_blocked = input_blocked;
    session.clipboard_allowed = clipboard_allowed;
    session.pending_input_blocked = None;
    session.pending_clipboard_allowed = None;
    true
}

fn approval_seconds_remaining(pending: &PendingApproval) -> u64 {
    let remaining = pending.expires_at.saturating_duration_since(Instant::now());
    remaining
        .as_secs()
        .saturating_add(u64::from(remaining.subsec_nanos() > 0))
}

fn telemetry_value<'a>(summary: &'a str, key: &str) -> Option<&'a str> {
    summary.split_whitespace().find_map(|part| {
        part.split_once('=')
            .filter(|(candidate, _)| *candidate == key)
            .map(|(_, value)| value)
    })
}

fn telemetry_u64(summary: &str, key: &str, suffix: &str) -> Option<u64> {
    telemetry_value(summary, key)?
        .strip_suffix(suffix)
        .unwrap_or(telemetry_value(summary, key)?)
        .parse()
        .ok()
}

fn parse_host_video_telemetry(summary: &str) -> Option<HostVideoTelemetry> {
    let backend = telemetry_value(summary, "backend")
        .unwrap_or_default()
        .to_owned();
    let fps = telemetry_u64(summary, "fps", "")
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_default();
    let bitrate_kbps = telemetry_u64(summary, "actual", "kbps")
        .or_else(|| telemetry_u64(summary, "bitrate", "kbps"))
        .unwrap_or_default();
    let sent_frames = telemetry_u64(summary, "sent", "").unwrap_or_default();
    let skipped_frames = telemetry_u64(summary, "skipped_static", "").unwrap_or_default();
    let encode_avg_ms = telemetry_u64(summary, "encode_avg", "ms").unwrap_or_default();

    (!backend.is_empty()
        || fps > 0
        || bitrate_kbps > 0
        || sent_frames > 0
        || skipped_frames > 0
        || encode_avg_ms > 0)
        .then_some(HostVideoTelemetry {
            backend,
            fps,
            bitrate_kbps,
            sent_frames,
            skipped_frames,
            encode_avg_ms,
        })
}

fn format_host_bitrate(kbps: u64) -> String {
    if kbps < 1_000 {
        format!("{kbps} Кбит/с")
    } else {
        format!("{:.1} Мбит/с", kbps as f64 / 1_000.0)
    }
}

fn tray_status_label(
    hosting: bool,
    host_state: &HostState,
    pending: Option<&PendingApproval>,
    session: Option<&IncomingSession>,
) -> String {
    if let Some(pending) = pending {
        return format!(
            "Требуется подтверждение: {}",
            compact_peer_id(&pending.peer_id)
        );
    }
    if let Some(session) = session {
        let mode = if session.input_blocked {
            "управление заблокировано"
        } else {
            "сессия активна"
        };
        return format!("{} — {mode}", compact_peer_id(&session.peer_id));
    }
    if !hosting {
        return "Остановлен".to_owned();
    }
    match host_state {
        HostState::Connecting => "Подключение…".to_owned(),
        HostState::Error(_) => "Ошибка host".to_owned(),
        HostState::Accepting(peer_id) => {
            format!("{} — сессия активна", compact_peer_id(peer_id))
        }
        HostState::Idle | HostState::Ready => "Готов к подключениям".to_owned(),
    }
}

fn compact_peer_id(peer_id: &str) -> String {
    const MAX_CHARS: usize = 32;
    let mut compact: String = peer_id.chars().take(MAX_CHARS).collect();
    if peer_id.chars().count() > MAX_CHARS {
        compact.push('…');
    }
    compact
}

fn schedule_clipboard_expiry(token: u64, fingerprint: u64) {
    let events = event_bus().0.clone();
    let _ = thread::Builder::new()
        .name("evertydesk-clipboard-expiry".to_owned())
        .spawn(move || {
            thread::sleep(PASSWORD_CLIPBOARD_TTL);
            let _ = events.send_blocking(ProcessEvent::ClipboardExpiry { token, fingerprint });
        });
}

fn schedule_approval_expiry(peer_id: String, token: u64) {
    let events = event_bus().0.clone();
    let _ = thread::Builder::new()
        .name("evertydesk-approval-expiry".to_owned())
        .spawn(move || {
            thread::sleep(APPROVAL_UI_TIMEOUT);
            let _ = events.send_blocking(ProcessEvent::ApprovalExpired { peer_id, token });
        });
}

fn schedule_viewer_timeout(
    process_id: u32,
    token: u64,
    timeout: Duration,
    kind: ViewerTimeoutKind,
) {
    let events = event_bus().0.clone();
    let _ = thread::Builder::new()
        .name(format!(
            "viewer-{}-timeout-{process_id}",
            match kind {
                ViewerTimeoutKind::Startup => "startup",
                ViewerTimeoutKind::Shutdown => "shutdown",
            }
        ))
        .spawn(move || {
            thread::sleep(timeout);
            let event = match kind {
                ViewerTimeoutKind::Startup => {
                    ProcessEvent::ViewerStartupExpired { process_id, token }
                }
                ViewerTimeoutKind::Shutdown => {
                    ProcessEvent::ViewerShutdownExpired { process_id, token }
                }
            };
            let _ = events.send_blocking(event);
        });
}

fn schedule_viewer_control_timeout(process_id: u32, token: u64, control: ViewerControl) {
    let events = event_bus().0.clone();
    let _ = thread::Builder::new()
        .name(format!("viewer-control-timeout-{process_id}"))
        .spawn(move || {
            thread::sleep(VIEWER_CONTROL_TIMEOUT);
            let _ = events.send_blocking(ProcessEvent::ViewerControlExpired {
                process_id,
                token,
                control,
            });
        });
}

fn schedule_viewer_liveness_timeout(process_id: u32, token: u64, heartbeat_sequence: u64) {
    let events = event_bus().0.clone();
    let _ = thread::Builder::new()
        .name(format!("viewer-liveness-timeout-{process_id}"))
        .spawn(move || {
            thread::sleep(VIEWER_LIVENESS_TIMEOUT);
            let _ = events.send_blocking(ProcessEvent::ViewerLivenessExpired {
                process_id,
                token,
                heartbeat_sequence,
            });
        });
}

fn process_event_stream() -> impl iced::futures::Stream<Item = Message> {
    iced::futures::stream::unfold((), |()| async {
        event_bus()
            .1
            .recv()
            .await
            .ok()
            .map(|event| (Message::ProcessEvent(event), ()))
    })
}

fn watch_viewer(process_id: u32, stdout: ChildStdout, completion: Arc<AtomicU8>) -> io::Result<()> {
    let events = event_bus().0.clone();
    thread::Builder::new()
        .name(format!("viewer-status-{process_id}"))
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let line = match read_bounded_line(&mut reader, MAX_IPC_LINE_BYTES) {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(error) => {
                        let _ = events.send_blocking(ProcessEvent::Status {
                            process_id,
                            status: ViewerStatus::Failed {
                                error: format!("IPC viewer → launcher: {error}"),
                            },
                        });
                        break;
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<ViewerStatus>(&line) {
                    Ok(status) => {
                        if events
                            .send_blocking(ProcessEvent::Status { process_id, status })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = events.send_blocking(ProcessEvent::Status {
                            process_id,
                            status: ViewerStatus::Failed {
                                error: format!(
                                    "Некорректный IPC-статус viewer → launcher: {error}"
                                ),
                            },
                        });
                        break;
                    }
                }
            }
            finish_viewer_stream(process_id, &completion);
        })?;
    Ok(())
}

fn watch_viewer_diagnostics(
    process_id: u32,
    stderr: ChildStderr,
    completion: Arc<AtomicU8>,
) -> io::Result<()> {
    let events = event_bus().0.clone();
    thread::Builder::new()
        .name(format!("viewer-diagnostics-{process_id}"))
        .spawn(move || {
            let mut reader = BufReader::new(stderr);
            loop {
                let line = match read_bounded_line(&mut reader, MAX_VIEWER_DIAGNOSTIC_BYTES) {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(error) => {
                        let _ = events.send_blocking(ProcessEvent::Diagnostic {
                            process_id,
                            message: format!("stderr viewer отклонён: {error}"),
                        });
                        break;
                    }
                };
                let message = sanitize_viewer_diagnostic(&line);
                if message.is_empty() {
                    continue;
                }
                if events
                    .send_blocking(ProcessEvent::Diagnostic {
                        process_id,
                        message,
                    })
                    .is_err()
                {
                    break;
                }
            }
            finish_viewer_stream(process_id, &completion);
        })?;
    Ok(())
}

fn finish_viewer_stream(process_id: u32, completion: &AtomicU8) {
    if completion.fetch_sub(1, Ordering::AcqRel) == 1 {
        let _ = event_bus()
            .0
            .send_blocking(ProcessEvent::StreamClosed { process_id });
    }
}

fn sanitize_viewer_diagnostic(message: &str) -> String {
    let mut sanitized = String::new();
    let mut count = 0;
    for character in message.trim().chars() {
        if character.is_control() && character != '\t' {
            continue;
        }
        if count == MAX_VIEWER_DIAGNOSTIC_CHARS {
            sanitized.push('…');
            break;
        }
        sanitized.push(if character == '\t' { ' ' } else { character });
        count += 1;
    }
    sanitized
}

fn sanitize_diagnostic_value(value: &str, max_chars: usize) -> String {
    value
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(max_chars)
        .collect()
}

fn push_viewer_diagnostic(diagnostics: &mut VecDeque<String>, message: String) {
    if message.is_empty() {
        return;
    }
    if diagnostics.len() == MAX_VIEWER_DIAGNOSTICS {
        diagnostics.pop_front();
    }
    diagnostics.push_back(message);
}

fn classify_viewer_exit(
    success: Option<bool>,
    disconnect_requested: bool,
    closed_status_received: bool,
) -> ViewerExitKind {
    if success == Some(false) {
        ViewerExitKind::Crashed
    } else if disconnect_requested {
        ViewerExitKind::Requested
    } else if success == Some(true) || closed_status_received {
        ViewerExitKind::Clean
    } else {
        ViewerExitKind::Lost
    }
}

fn can_start_viewer(active_viewers: usize) -> bool {
    active_viewers < MAX_ACTIVE_VIEWERS
}

fn viewer_watchdog_applies(current_token: u64, event_token: u64, completed: bool) -> bool {
    current_token == event_token && !completed
}

fn viewer_liveness_expired(
    current_token: u64,
    event_token: u64,
    current_sequence: u64,
    event_sequence: u64,
    disconnect_requested: bool,
) -> bool {
    current_token == event_token && current_sequence == event_sequence && !disconnect_requested
}

fn apply_viewer_control(entry: &mut ViewerEntry, control: ViewerControl) {
    match control {
        ViewerControl::InputEnabled { enabled } => entry.input_enabled = enabled,
        ViewerControl::AudioEnabled { enabled } => entry.audio_enabled = enabled,
        ViewerControl::ClipboardEnabled { enabled } => entry.clipboard_enabled = enabled,
        ViewerControl::Quality { .. } => {}
        ViewerControl::Scaling { scaling } => entry.scaling = scaling,
    }
}

fn viewer_control_label(control: ViewerControl, language: UiLanguage) -> &'static str {
    match (language, control) {
        (_, ViewerControl::ClipboardEnabled { .. }) => "clipboard",
        (UiLanguage::Russian, ViewerControl::InputEnabled { .. }) => "управление",
        (UiLanguage::Russian, ViewerControl::AudioEnabled { .. }) => "звук",
        (UiLanguage::Russian, ViewerControl::Quality { .. }) => "качество",
        (UiLanguage::Russian, ViewerControl::Scaling { .. }) => "масштабирование",
        (UiLanguage::English, ViewerControl::InputEnabled { .. }) => "input",
        (UiLanguage::English, ViewerControl::AudioEnabled { .. }) => "audio",
        (UiLanguage::English, ViewerControl::Quality { .. }) => "quality",
        (UiLanguage::English, ViewerControl::Scaling { .. }) => "scaling",
    }
}

fn viewer_control_applied_text(control: ViewerControl, language: UiLanguage) -> String {
    match (language, control) {
        (UiLanguage::Russian, ViewerControl::InputEnabled { enabled: true }) => {
            "Управление включено".to_owned()
        }
        (UiLanguage::Russian, ViewerControl::InputEnabled { enabled: false }) => {
            "Режим «только просмотр»".to_owned()
        }
        (UiLanguage::Russian, ViewerControl::AudioEnabled { enabled: true }) => {
            "Звук включён".to_owned()
        }
        (UiLanguage::Russian, ViewerControl::AudioEnabled { enabled: false }) => {
            "Звук отключён".to_owned()
        }
        (UiLanguage::Russian, ViewerControl::ClipboardEnabled { enabled: true }) => {
            "Clipboard включён".to_owned()
        }
        (UiLanguage::Russian, ViewerControl::ClipboardEnabled { enabled: false }) => {
            "Clipboard отключён".to_owned()
        }
        (UiLanguage::Russian, ViewerControl::Quality { quality }) => {
            format!("Профиль «{}» подтверждён", quality_label(quality, language))
        }
        (UiLanguage::Russian, ViewerControl::Scaling { scaling }) => scaling.label().to_owned(),
        (UiLanguage::English, ViewerControl::InputEnabled { enabled: true }) => {
            "Input enabled".to_owned()
        }
        (UiLanguage::English, ViewerControl::InputEnabled { enabled: false }) => {
            "View-only mode".to_owned()
        }
        (UiLanguage::English, ViewerControl::AudioEnabled { enabled: true }) => {
            "Audio enabled".to_owned()
        }
        (UiLanguage::English, ViewerControl::AudioEnabled { enabled: false }) => {
            "Audio disabled".to_owned()
        }
        (UiLanguage::English, ViewerControl::ClipboardEnabled { enabled: true }) => {
            "Clipboard enabled".to_owned()
        }
        (UiLanguage::English, ViewerControl::ClipboardEnabled { enabled: false }) => {
            "Clipboard disabled".to_owned()
        }
        (UiLanguage::English, ViewerControl::Quality { quality }) => {
            format!("Profile “{}” confirmed", quality_label(quality, language))
        }
        (UiLanguage::English, ViewerControl::Scaling { scaling }) => match scaling {
            ViewerScaling::SmoothFit => "Scaling: smooth".to_owned(),
            ViewerScaling::PixelPerfect => "Scaling: 1:1".to_owned(),
        },
    }
}

fn watch_host(service: HostService) {
    let events = event_bus().0.clone();
    let _ = thread::Builder::new()
        .name("evertydesk-host-events".to_owned())
        .spawn(move || {
            while let Ok(event) = service.event_rx.recv() {
                if events.send_blocking(ProcessEvent::Host(event)).is_err() {
                    break;
                }
            }
        });
}

fn status_text(status: &ViewerStatus, language: UiLanguage) -> String {
    match (language, status) {
        (UiLanguage::Russian, ViewerStatus::Starting) => "Запуск viewer…".to_owned(),
        (UiLanguage::English, ViewerStatus::Starting) => "Starting viewer…".to_owned(),
        (_, ViewerStatus::Progress { percent, message }) => format!("{percent}% — {message}"),
        (_, ViewerStatus::Info { message }) => message.clone(),
        (UiLanguage::Russian, ViewerStatus::Connected { peer }) => format!("Подключено: {peer}"),
        (UiLanguage::English, ViewerStatus::Connected { peer }) => format!("Connected: {peer}"),
        (UiLanguage::Russian, ViewerStatus::Latency { milliseconds }) => {
            format!("Подключено · {milliseconds} мс")
        }
        (UiLanguage::English, ViewerStatus::Latency { milliseconds }) => {
            format!("Connected · {milliseconds} ms")
        }
        (UiLanguage::Russian, ViewerStatus::Codec { name }) => format!("Кодек: {name}"),
        (UiLanguage::English, ViewerStatus::Codec { name }) => format!("Codec: {name}"),
        (
            _,
            ViewerStatus::Performance {
                fps_times_100,
                input_kbps,
                dropped_frames,
                session_seconds,
                reconnect_count,
            },
        ) => format_performance(
            *fps_times_100,
            *input_kbps,
            *dropped_frames,
            *session_seconds,
            *reconnect_count,
            language,
        ),
        (UiLanguage::Russian, ViewerStatus::Recovery { reason }) => {
            format!("Восстановление видео · {reason}")
        }
        (UiLanguage::English, ViewerStatus::Recovery { reason }) => {
            format!("Video recovery · {reason}")
        }
        (UiLanguage::Russian, ViewerStatus::ScreenshotSaved { path }) => {
            format!("Снимок сохранён: {path}")
        }
        (UiLanguage::English, ViewerStatus::ScreenshotSaved { path }) => {
            format!("Screenshot saved: {path}")
        }
        (
            language,
            ViewerStatus::SessionSummary {
                session_seconds,
                reconnect_count,
                end_reason,
                ..
            },
        ) => {
            let reason = if end_reason.is_empty() {
                match language {
                    UiLanguage::Russian => "Сессия завершена",
                    UiLanguage::English => "Session ended",
                }
            } else {
                end_reason
            };
            match language {
                UiLanguage::Russian => format!(
                    "{reason} · {} · восстановлений {}",
                    format_duration(*session_seconds),
                    reconnect_count
                ),
                UiLanguage::English => format!(
                    "{reason} · {} · reconnects {}",
                    format_duration(*session_seconds),
                    reconnect_count
                ),
            }
        }
        (
            UiLanguage::Russian,
            ViewerStatus::Reconnecting {
                attempt,
                delay_seconds,
            },
        ) => format!("Попытка {attempt} · переподключение через {delay_seconds} с"),
        (
            UiLanguage::English,
            ViewerStatus::Reconnecting {
                attempt,
                delay_seconds,
            },
        ) => format!("Attempt {attempt} · reconnecting in {delay_seconds}s"),
        (_, ViewerStatus::Heartbeat { sequence }) => format!("Heartbeat {sequence}"),
        (_, ViewerStatus::ControlApplied { control }) => {
            viewer_control_applied_text(*control, language)
        }
        (_, ViewerStatus::ControlState { control }) => {
            viewer_control_applied_text(*control, language)
        }
        (UiLanguage::Russian, ViewerStatus::Failed { error }) => format!("Ошибка: {error}"),
        (UiLanguage::English, ViewerStatus::Failed { error }) => format!("Error: {error}"),
        (UiLanguage::Russian, ViewerStatus::Closed) => "Закрыто".to_owned(),
        (UiLanguage::English, ViewerStatus::Closed) => "Closed".to_owned(),
    }
}

fn format_performance(
    fps_times_100: u32,
    input_kbps: u64,
    dropped_frames: u64,
    session_seconds: u64,
    reconnect_count: u32,
    language: UiLanguage,
) -> String {
    let whole_fps = fps_times_100 / 100;
    let fractional_fps = fps_times_100 % 100;
    let bandwidth = format_bandwidth(input_kbps);
    let reconnects = if reconnect_count == 0 {
        String::new()
    } else {
        match language {
            UiLanguage::Russian => format!(" · восстановлений {reconnect_count}"),
            UiLanguage::English => format!(" · reconnects {reconnect_count}"),
        }
    };
    let dropped = if dropped_frames == 0 {
        String::new()
    } else {
        match language {
            UiLanguage::Russian => format!(" · пропущено {dropped_frames}"),
            UiLanguage::English => format!(" · dropped {dropped_frames}"),
        }
    };
    match language {
        UiLanguage::Russian => format!(
            "Подключено {} · {whole_fps}.{fractional_fps:02} FPS · {bandwidth}{dropped}{reconnects}",
            format_duration(session_seconds)
        ),
        UiLanguage::English => format!(
            "Connected {} · {whole_fps}.{fractional_fps:02} FPS · {bandwidth}{dropped}{reconnects}",
            format_duration(session_seconds)
        ),
    }
}

fn format_bandwidth(input_kbps: u64) -> String {
    if input_kbps >= 1_000 {
        format!("{:.1} Мбит/с", input_kbps as f64 / 1_000.0)
    } else {
        format!("{input_kbps} Кбит/с")
    }
}

fn format_telemetry_age(age: Duration) -> String {
    if age < Duration::from_secs(1) {
        format!("{} мс назад", age.as_millis())
    } else {
        format!("{:.1} с назад", age.as_secs_f32())
    }
}

fn viewer_connection_health(
    latency_ms: Option<u32>,
    fps_times_100: u32,
    telemetry_age: Option<Duration>,
) -> &'static str {
    let Some(age) = telemetry_age else {
        return "ожидание";
    };
    if age > Duration::from_secs(5) {
        return "нет свежих данных";
    }
    if latency_ms.is_some_and(|latency| latency > 200) || fps_times_100 < 1_500 {
        return "плохое";
    }
    if latency_ms.is_some_and(|latency| latency > 100) || fps_times_100 < 3_000 {
        return "среднее";
    }
    "хорошее"
}

fn reset_viewer_telemetry(entry: &mut ViewerEntry) {
    entry.codec.clear();
    entry.latency_ms = None;
    entry.fps_times_100 = 0;
    entry.input_kbps = 0;
    entry.dropped_frames = 0;
    entry.session_seconds = 0;
    entry.last_telemetry_at = None;
}

fn viewer_diagnostics_report(entry: &ViewerEntry) -> String {
    let age = entry.last_telemetry_at.map(|updated| updated.elapsed());
    let codec = if entry.codec.is_empty() {
        "ожидание"
    } else {
        entry.codec.as_str()
    };
    let latency = entry
        .latency_ms
        .map_or_else(|| "—".to_owned(), |value| format!("{value} мс"));
    let mut lines = vec![
        format!("EvertyDesk — диагностика сессии {}", entry.remote_id),
        format!(
            "Состояние: {}",
            viewer_connection_health(entry.latency_ms, entry.fps_times_100, age)
        ),
        format!("Статус: {}", entry.status),
        format!("Кодек: {codec}"),
        format!(
            "FPS: {}.{:02}",
            entry.fps_times_100 / 100,
            entry.fps_times_100 % 100
        ),
        format!("Битрейт: {}", format_bandwidth(entry.input_kbps)),
        format!("Задержка: {latency}"),
        format!("Пропущено кадров: {}", entry.dropped_frames),
        format!("Длительность: {}", format_duration(entry.session_seconds)),
        format!("Восстановлений: {}", entry.reconnect_count),
        format!(
            "Телеметрия: {}",
            age.map_or_else(|| "—".to_owned(), format_telemetry_age)
        ),
        format!(
            "Разрешения: управление={}, звук={}, clipboard={}",
            entry.input_enabled, entry.audio_enabled, entry.clipboard_enabled
        ),
        format!("Масштабирование: {}", entry.scaling.label()),
        format!(
            "Профиль: {}",
            viewer_game_profile_label(entry.game_mode, entry.game_codec, entry.game_evrt2_enabled)
        ),
    ];
    if !entry.diagnostics.is_empty() {
        lines.push("Последние сообщения viewer:".to_owned());
        lines.extend(entry.diagnostics.iter().cloned());
    }
    lines.join("\n")
}

fn format_duration(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}с"),
        60..=3_599 => format!("{}м {:02}с", seconds / 60, seconds % 60),
        _ => format!("{}ч {:02}м", seconds / 3_600, (seconds % 3_600) / 60),
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn clipboard_fingerprint_changes_with_content_and_length() {
        assert_eq!(
            clipboard_fingerprint("secret"),
            clipboard_fingerprint("secret")
        );
        assert_ne!(
            clipboard_fingerprint("secret"),
            clipboard_fingerprint("secret!")
        );
    }

    #[test]
    fn permanent_password_sanitizer_removes_control_chars_and_limits_length() {
        let raw = format!(
            "abc\n{}\tdef",
            "x".repeat(MAX_PERMANENT_PASSWORD_CHARS + 20)
        );
        let sanitized = sanitize_permanent_password(&raw);
        assert!(!sanitized.contains('\n'));
        assert!(!sanitized.contains('\t'));
        assert_eq!(sanitized.chars().count(), MAX_PERMANENT_PASSWORD_CHARS);
    }

    #[test]
    fn temporary_password_rotation_waits_for_idle_incoming_state() {
        assert!(!should_rotate_temporary_password(
            TEMP_PASSWORD_ROTATION_INTERVAL - Duration::from_secs(1),
            false,
            false,
            false
        ));
        assert!(should_rotate_temporary_password(
            TEMP_PASSWORD_ROTATION_INTERVAL,
            false,
            false,
            false
        ));
        assert!(!should_rotate_temporary_password(
            TEMP_PASSWORD_ROTATION_INTERVAL,
            true,
            false,
            false
        ));
        assert!(!should_rotate_temporary_password(
            TEMP_PASSWORD_ROTATION_INTERVAL,
            false,
            true,
            false
        ));
        assert!(!should_rotate_temporary_password(
            TEMP_PASSWORD_ROTATION_INTERVAL,
            false,
            false,
            true
        ));
    }

    #[test]
    fn approval_timeout_requires_the_current_peer_and_token() {
        let pending = PendingApproval {
            peer_id: "123".to_owned(),
            peer_name: "Office PC".to_owned(),
            platform: "windows".to_owned(),
            version: "1.4.6".to_owned(),
            token: 7,
            expires_at: Instant::now() + Duration::from_secs(40),
            allow_input: true,
            allow_clipboard: true,
        };
        assert!(approval_matches(Some(&pending), "123", 7));
        assert!(!approval_matches(Some(&pending), "123", 6));
        assert!(!approval_matches(Some(&pending), "456", 7));
        assert!(!approval_matches(None, "123", 7));
    }

    #[test]
    fn approval_countdown_is_rounded_up_and_stops_at_zero() {
        let pending = PendingApproval {
            peer_id: "123".to_owned(),
            peer_name: String::new(),
            platform: String::new(),
            version: String::new(),
            token: 1,
            expires_at: Instant::now() + Duration::from_millis(1_500),
            allow_input: true,
            allow_clipboard: false,
        };
        assert!(matches!(approval_seconds_remaining(&pending), 1 | 2));

        let expired = PendingApproval {
            expires_at: Instant::now() - Duration::from_secs(1),
            ..pending
        };
        assert_eq!(approval_seconds_remaining(&expired), 0);
    }

    #[test]
    fn incoming_session_action_reflects_runtime_input_lock() {
        let mut session = IncomingSession {
            session_id: 10,
            peer_id: "123".to_owned(),
            peer_name: "Office PC".to_owned(),
            platform: "windows".to_owned(),
            version: "1.4.6".to_owned(),
            input_blocked: false,
            clipboard_allowed: true,
            pending_input_blocked: None,
            pending_clipboard_allowed: None,
            started_at: Instant::now(),
            telemetry: None,
            fallback_reason: None,
            disconnect_requested: false,
        };
        assert_eq!(session.input_action_label(), "Заблокировать управление");
        session.input_blocked = true;
        assert_eq!(session.input_action_label(), "Разрешить управление");
        assert_eq!(session.clipboard_action_label(), "Запретить буфер обмена");
        session.clipboard_allowed = false;
        assert_eq!(session.clipboard_action_label(), "Разрешить буфер обмена");
    }

    #[test]
    fn permission_ack_only_updates_the_matching_active_session() {
        let mut session = IncomingSession {
            session_id: 10,
            peer_id: "123".to_owned(),
            peer_name: "Office PC".to_owned(),
            platform: "windows".to_owned(),
            version: "1.4.6".to_owned(),
            input_blocked: false,
            clipboard_allowed: true,
            pending_input_blocked: Some(true),
            pending_clipboard_allowed: Some(false),
            started_at: Instant::now(),
            telemetry: None,
            fallback_reason: None,
            disconnect_requested: false,
        };
        assert!(!apply_session_permissions_ack(
            Some(&mut session),
            "old-peer",
            10,
            true,
            false
        ));
        assert!(!session.input_blocked);
        assert_eq!(session.pending_input_blocked, Some(true));
        assert!(!apply_session_permissions_ack(
            Some(&mut session),
            "123",
            9,
            true,
            false
        ));

        assert!(apply_session_permissions_ack(
            Some(&mut session),
            "123",
            10,
            true,
            false
        ));
        assert!(session.input_blocked);
        assert!(!session.clipboard_allowed);
        assert_eq!(session.pending_input_blocked, None);
        assert_eq!(session.pending_clipboard_allowed, None);
    }

    #[test]
    fn tray_status_prioritizes_approval_and_active_session() {
        assert_eq!(
            tray_status_label(false, &HostState::Idle, None, None),
            "Остановлен"
        );
        assert_eq!(
            tray_status_label(true, &HostState::Ready, None, None),
            "Готов к подключениям"
        );

        let pending = PendingApproval {
            peer_id: "123".to_owned(),
            peer_name: String::new(),
            platform: String::new(),
            version: String::new(),
            token: 1,
            expires_at: Instant::now() + Duration::from_secs(40),
            allow_input: true,
            allow_clipboard: true,
        };
        let session = IncomingSession {
            session_id: 11,
            peer_id: "456".to_owned(),
            peer_name: String::new(),
            platform: String::new(),
            version: String::new(),
            input_blocked: true,
            clipboard_allowed: false,
            pending_input_blocked: None,
            pending_clipboard_allowed: None,
            started_at: Instant::now(),
            telemetry: None,
            fallback_reason: None,
            disconnect_requested: false,
        };
        assert_eq!(
            tray_status_label(
                true,
                &HostState::Accepting("456".to_owned()),
                Some(&pending),
                Some(&session)
            ),
            "Требуется подтверждение: 123"
        );
        assert_eq!(
            tray_status_label(
                true,
                &HostState::Accepting("456".to_owned()),
                None,
                Some(&session)
            ),
            "456 — управление заблокировано"
        );
        assert!(compact_peer_id(&"x".repeat(100)).chars().count() <= 33);
    }

    #[test]
    fn host_video_telemetry_uses_actual_bitrate_and_parses_pipeline_metrics() {
        let telemetry = parse_host_video_telemetry(
            "backend=EVRTCK fps=44 sent=120 skipped_static=8 bitrate=2400kbps \
             actual=2100kbps encode_avg=3ms",
        )
        .unwrap();
        assert_eq!(
            telemetry,
            HostVideoTelemetry {
                backend: "EVRTCK".to_owned(),
                fps: 44,
                bitrate_kbps: 2_100,
                sent_frames: 120,
                skipped_frames: 8,
                encode_avg_ms: 3,
            }
        );
        assert_eq!(format_host_bitrate(2_100), "2.1 Мбит/с");
        assert_eq!(format_host_bitrate(850), "850 Кбит/с");
    }

    #[test]
    fn malformed_host_video_telemetry_is_ignored() {
        assert_eq!(parse_host_video_telemetry("not telemetry"), None);
    }

    #[test]
    fn single_instance_handshake_is_exact_and_round_trips_on_loopback() {
        assert!(is_focus_request(SINGLE_INSTANCE_REQUEST));
        assert!(!is_focus_request(SINGLE_INSTANCE_BACKGROUND_REQUEST));
        assert!(is_single_instance_request(SINGLE_INSTANCE_REQUEST));
        assert!(is_single_instance_request(
            SINGLE_INSTANCE_BACKGROUND_REQUEST
        ));
        assert!(!is_focus_request("EVERTYDESK_LAUNCHER_FOCUS_V2"));

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = {
                let mut reader = BufReader::new(&mut stream);
                read_bounded_line(&mut reader, 64).unwrap().unwrap()
            };
            assert!(is_focus_request(&request));
            stream
                .write_all(format!("{SINGLE_INSTANCE_RESPONSE}\n").as_bytes())
                .unwrap();
            stream.flush().unwrap();
        });

        notify_primary_instance(address, false).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn background_single_instance_handshake_does_not_request_focus() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = {
                let mut reader = BufReader::new(&mut stream);
                read_bounded_line(&mut reader, 64).unwrap().unwrap()
            };
            assert_eq!(request, SINGLE_INSTANCE_BACKGROUND_REQUEST);
            assert!(!is_focus_request(&request));
            assert!(is_single_instance_request(&request));
            stream
                .write_all(format!("{SINGLE_INSTANCE_RESPONSE}\n").as_bytes())
                .unwrap();
            stream.flush().unwrap();
        });

        notify_primary_instance(address, true).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn stale_single_instance_port_allows_new_launcher_start() {
        assert!(!should_start_after_single_instance_notify(&Ok(())));
        assert!(should_start_after_single_instance_notify(&Err(
            io::Error::new(io::ErrorKind::TimedOut, "stale primary")
        )));
    }

    #[test]
    fn startup_env_diagnostics_are_sanitized_and_bounded() {
        let input = format!("  dx12\0\n{}\t", "x".repeat(160));
        let sanitized = sanitize_startup_env_value(&input);

        assert!(!sanitized.contains('\0'));
        assert!(!sanitized.contains('\n'));
        assert!(!sanitized.contains('\t'));
        assert!(sanitized.starts_with("dx12"));
        assert!(sanitized.ends_with('…'));
        assert!(sanitized.chars().count() <= MAX_STARTUP_ENV_VALUE_CHARS + 1);
    }

    #[test]
    fn viewer_diagnostics_are_sanitized_and_bounded() {
        let input = format!("failure\0\t{}\r\n", "x".repeat(400));
        let diagnostic = sanitize_viewer_diagnostic(&input);

        assert!(!diagnostic.contains('\0'));
        assert!(!diagnostic.contains('\t'));
        assert!(!diagnostic.contains('\r'));
        assert!(!diagnostic.contains('\n'));
        assert!(diagnostic.starts_with("failure "));
        assert_eq!(diagnostic.chars().count(), MAX_VIEWER_DIAGNOSTIC_CHARS + 1);
        assert!(diagnostic.ends_with('…'));
    }

    #[test]
    fn structured_diagnostics_sanitize_codec_and_classify_connection_health() {
        assert_eq!(
            sanitize_diagnostic_value(" H264\r\nunsafe\0 ", 8),
            "H264unsa"
        );
        assert_eq!(viewer_connection_health(None, 0, None), "ожидание");
        assert_eq!(
            viewer_connection_health(Some(40), 6_000, Some(Duration::from_secs(6))),
            "нет свежих данных"
        );
        assert_eq!(
            viewer_connection_health(Some(240), 6_000, Some(Duration::from_millis(50))),
            "плохое"
        );
        assert_eq!(
            viewer_connection_health(Some(80), 2_500, Some(Duration::from_millis(50))),
            "среднее"
        );
        assert_eq!(
            viewer_connection_health(Some(40), 6_000, Some(Duration::from_millis(50))),
            "хорошее"
        );
    }

    #[test]
    fn viewer_diagnostic_ring_keeps_only_the_latest_messages() {
        let mut diagnostics = VecDeque::new();
        for index in 0..12 {
            push_viewer_diagnostic(&mut diagnostics, index.to_string());
        }

        assert_eq!(diagnostics.len(), MAX_VIEWER_DIAGNOSTICS);
        assert_eq!(diagnostics.front().map(String::as_str), Some("4"));
        assert_eq!(diagnostics.back().map(String::as_str), Some("11"));
    }

    #[test]
    fn viewer_exit_classification_preserves_user_intent_and_crashes() {
        assert_eq!(
            classify_viewer_exit(Some(true), true, true),
            ViewerExitKind::Requested
        );
        assert_eq!(
            classify_viewer_exit(Some(true), false, true),
            ViewerExitKind::Clean
        );
        assert_eq!(
            classify_viewer_exit(None, false, true),
            ViewerExitKind::Clean
        );
        assert_eq!(
            classify_viewer_exit(Some(false), true, true),
            ViewerExitKind::Crashed
        );
        assert_eq!(
            classify_viewer_exit(None, false, false),
            ViewerExitKind::Lost
        );
    }

    #[test]
    fn viewer_capacity_is_bounded() {
        assert!(can_start_viewer(MAX_ACTIVE_VIEWERS - 1));
        assert!(!can_start_viewer(MAX_ACTIVE_VIEWERS));
        assert!(!can_start_viewer(MAX_ACTIVE_VIEWERS + 1));
    }

    #[test]
    fn viewer_watchdogs_ignore_completed_and_stale_sessions() {
        assert!(viewer_watchdog_applies(7, 7, false));
        assert!(!viewer_watchdog_applies(7, 7, true));
        assert!(!viewer_watchdog_applies(8, 7, false));
    }

    #[test]
    fn pending_viewer_controls_require_an_exact_acknowledgement() {
        let mut pending = PendingViewerControls::default();
        let requested = ViewerControl::InputEnabled { enabled: false };

        pending.insert(requested);
        assert!(pending.has_kind(ViewerControl::InputEnabled { enabled: true }));
        assert!(!pending.remove(ViewerControl::InputEnabled { enabled: true }));
        assert!(pending.contains(requested));
        assert!(pending.remove(requested));
        assert!(!pending.has_kind(requested));

        let audio = ViewerControl::AudioEnabled { enabled: false };
        pending.insert(audio);
        assert!(pending.contains(audio));
        pending.remove_kind(ViewerControl::AudioEnabled { enabled: true });
        assert!(!pending.has_kind(audio));
    }

    #[test]
    fn liveness_watchdog_ignores_new_heartbeats_stale_tokens_and_shutdown() {
        assert!(viewer_liveness_expired(7, 7, 3, 3, false));
        assert!(!viewer_liveness_expired(7, 7, 4, 3, false));
        assert!(!viewer_liveness_expired(8, 7, 3, 3, false));
        assert!(!viewer_liveness_expired(7, 7, 3, 3, true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_run_command_value_quotes_executable_path() {
        assert_eq!(
            windows_run_command_value(r"C:\Program Files\EvertyDesk\evertydesk-launcher.exe"),
            r#""C:\Program Files\EvertyDesk\evertydesk-launcher.exe" --background"#
        );
    }

    #[test]
    fn settings_sidebar_sections_are_stable_and_ordered() {
        assert_eq!(
            SettingsSection::ALL
                .map(|section| settings_section_label(section, UiLanguage::Russian)),
            ["Безопасность", "Общее", "Подключение"]
        );
        assert!(
            settings_section_hint(SettingsSection::Security, UiLanguage::Russian)
                .contains("Пароли")
        );
        assert!(
            settings_section_hint(SettingsSection::Connection, UiLanguage::Russian)
                .contains("серверы")
        );
    }

    #[test]
    fn local_id_is_grouped_for_readability() {
        assert_eq!(format_local_id("123456789"), "123 456 789");
        assert_eq!(format_local_id("123 45"), "123 45");
        assert_eq!(normalize_remote_id("123 456-789"), "123456789");
        assert_eq!(normalize_remote_id(" 123\t456 \r\n789 "), "123456789");
        assert!(remote_ids_match("123 456-789", "123456789"));
        assert!(remote_ids_match("ABC-123", "abc 123"));
        assert!(!remote_ids_match(" \t ", "123"));
    }

    #[test]
    fn address_book_layout_stacks_before_cards_become_too_narrow() {
        assert!(!use_wide_directory_layout(1_136.0));
        assert!(!use_wide_directory_layout(1_319.0));
        assert!(use_wide_directory_layout(1_320.0));
        assert!(use_wide_directory_layout(2_048.0));
    }

    #[test]
    fn address_book_filters_match_groups_tags_favorites_and_recent() {
        let contact = Contact {
            name: "Касса".to_owned(),
            remote_id: "123".to_owned(),
            favorite: true,
            group: "Офис/Касса".to_owned(),
            tags: vec!["prod".to_owned(), "vip".to_owned()],
            note: "front desk".to_owned(),
        };
        let recent_ids = BTreeSet::from(["123".to_owned()]);

        assert!(contact_matches_address_book_filter(
            &contact,
            &AddressBookFilter::Favorites,
            &recent_ids
        ));
        assert!(contact_matches_address_book_filter(
            &contact,
            &AddressBookFilter::Recent,
            &recent_ids
        ));
        assert!(contact_matches_address_book_filter(
            &contact,
            &AddressBookFilter::Group("Офис / Касса".to_owned()),
            &recent_ids
        ));
        assert!(contact_matches_address_book_filter(
            &contact,
            &AddressBookFilter::Tag("PROD".to_owned()),
            &recent_ids
        ));
        assert!(contact_matches_text_filter(&contact, "vip"));
        assert!(contact_matches_text_filter(&contact, "кас"));
        assert!(!contact_matches_text_filter(&contact, "warehouse"));
    }

    #[test]
    fn address_book_group_filters_include_nested_children() {
        let contact = Contact {
            name: "POS".to_owned(),
            remote_id: "123".to_owned(),
            favorite: false,
            group: "Office > Cashbox / POS".to_owned(),
            tags: Vec::new(),
            note: String::new(),
        };
        let recent_ids = BTreeSet::new();

        assert_eq!(
            group_path_ancestors(&contact.group),
            vec![
                "Office".to_owned(),
                "Office / Cashbox".to_owned(),
                "Office / Cashbox / POS".to_owned()
            ]
        );
        assert!(contact_matches_address_book_filter(
            &contact,
            &AddressBookFilter::Group("Office".to_owned()),
            &recent_ids
        ));
        assert!(contact_matches_address_book_filter(
            &contact,
            &AddressBookFilter::Group("Office / Cashbox".to_owned()),
            &recent_ids
        ));
        assert!(contact_matches_address_book_filter(
            &contact,
            &AddressBookFilter::Group("Office / Cashbox / POS".to_owned()),
            &recent_ids
        ));
        assert!(!contact_matches_address_book_filter(
            &contact,
            &AddressBookFilter::Group("Warehouse".to_owned()),
            &recent_ids
        ));
        assert!(contact_matches_text_filter(&contact, "office / cash"));
    }

    #[test]
    fn address_book_group_navigation_uses_leaf_labels_and_bounded_indent() {
        assert_eq!(group_path_depth("Office"), 0);
        assert_eq!(group_path_depth("Office / Cashbox / POS"), 2);
        assert_eq!(group_leaf_label("Office / Cashbox / POS"), "POS");
        assert_eq!(group_leaf_label("  "), "Без группы");
        assert_eq!(address_book_group_indent(0), 0.0);
        assert_eq!(address_book_group_indent(2), 28.0);
        assert_eq!(address_book_group_indent(12), 56.0);
    }

    #[test]
    fn address_book_filtered_views_use_full_width_without_recent_panel() {
        assert!(address_book_filter_uses_recent_panel(
            &AddressBookFilter::All
        ));
        assert!(!address_book_filter_uses_recent_panel(
            &AddressBookFilter::Favorites
        ));
        assert!(!address_book_filter_uses_recent_panel(
            &AddressBookFilter::Group("Офис".to_owned())
        ));
        assert_eq!(
            address_book_filter_label(
                &AddressBookFilter::Tag("prod".to_owned()),
                UiLanguage::Russian
            ),
            "Контакты с меткой"
        );
        assert_eq!(
            address_book_filter_display(
                &AddressBookFilter::Group("Офис".to_owned()),
                UiLanguage::Russian
            ),
            "Офис"
        );
        assert_eq!(
            address_book_filter_display(
                &AddressBookFilter::Tag("prod".to_owned()),
                UiLanguage::Russian
            ),
            "#prod"
        );
    }

    #[test]
    fn address_book_suggestions_are_unique_sorted_and_limited() {
        let contacts = vec![
            Contact {
                name: "A".to_owned(),
                remote_id: "1".to_owned(),
                favorite: false,
                group: "Офис/Касса".to_owned(),
                tags: vec!["prod".to_owned(), "vip".to_owned()],
                note: String::new(),
            },
            Contact {
                name: "B".to_owned(),
                remote_id: "2".to_owned(),
                favorite: false,
                group: "Офис / Касса".to_owned(),
                tags: vec!["prod".to_owned(), "test".to_owned()],
                note: String::new(),
            },
        ];

        assert_eq!(
            address_book_group_suggestions(&contacts, 5),
            vec!["Офис".to_owned(), "Офис / Касса".to_owned()]
        );
        assert_eq!(
            address_book_tag_suggestions(&contacts, 2),
            vec!["prod".to_owned(), "test".to_owned()]
        );
    }

    #[test]
    fn selected_contact_is_resolved_only_inside_active_filter() {
        let contacts = vec![Contact {
            name: "Cashbox".to_owned(),
            remote_id: "123 456".to_owned(),
            favorite: false,
            group: "Office".to_owned(),
            tags: vec!["prod".to_owned()],
            note: String::new(),
        }];
        let recent = vec![RecentConnection {
            remote_id: "123-456".to_owned(),
            last_used_unix: 0,
            direction: ConnectionDirection::Outgoing,
            duration_seconds: 0,
            reconnect_count: 0,
            last_end_reason: String::new(),
        }];
        let recent_ids = normalized_recent_ids(&recent);

        assert!(selected_contact_for_filter(
            &contacts,
            Some("123456"),
            &AddressBookFilter::Recent,
            &recent_ids
        )
        .is_some());
        assert!(selected_contact_for_filter(
            &contacts,
            Some("123-456"),
            &AddressBookFilter::Recent,
            &recent_ids
        )
        .is_some());
        assert!(selected_contact_for_filter(
            &contacts,
            Some("123456"),
            &AddressBookFilter::Favorites,
            &recent_ids
        )
        .is_none());
        assert!(selected_contact_for_filter(
            &contacts,
            Some(""),
            &AddressBookFilter::All,
            &recent_ids
        )
        .is_none());
        assert!(contact_matches_address_book_filter(
            &contacts[0],
            &AddressBookFilter::Recent,
            &recent_ids
        ));
    }

    #[test]
    fn selected_contact_is_cleared_when_filter_or_search_hides_it() {
        let contacts = vec![
            Contact {
                name: "Cashbox".to_owned(),
                remote_id: "123".to_owned(),
                favorite: true,
                group: "Office / Cashbox".to_owned(),
                tags: vec!["prod".to_owned()],
                note: String::new(),
            },
            Contact {
                name: "Warehouse".to_owned(),
                remote_id: "456".to_owned(),
                favorite: false,
                group: "Warehouse".to_owned(),
                tags: vec!["stock".to_owned()],
                note: String::new(),
            },
        ];
        let recent_ids = BTreeSet::from(["123".to_owned()]);

        assert_eq!(
            selected_contact_after_filter_change(
                &contacts,
                Some("123"),
                &AddressBookFilter::Group("Office".to_owned()),
                &recent_ids,
                "cash"
            ),
            Some("123".to_owned())
        );
        assert_eq!(
            selected_contact_after_filter_change(
                &contacts,
                Some("123"),
                &AddressBookFilter::Group("Warehouse".to_owned()),
                &recent_ids,
                ""
            ),
            None
        );
        assert_eq!(
            selected_contact_after_filter_change(
                &contacts,
                Some("123"),
                &AddressBookFilter::All,
                &recent_ids,
                "warehouse"
            ),
            None
        );
    }

    #[test]
    fn settings_layout_stacks_sidebar_and_content_before_it_gets_cramped() {
        assert!(!use_wide_settings_sidebar_layout(1_000.0));
        assert!(!use_wide_settings_sidebar_layout(1_059.0));
        assert!(use_wide_settings_sidebar_layout(1_060.0));
        assert!(!use_wide_settings_content_layout(1_200.0));
        assert!(!use_wide_settings_content_layout(1_259.0));
        assert!(use_wide_settings_content_layout(1_260.0));
    }

    #[test]
    fn settings_language_and_update_labels_are_localized() {
        assert_eq!(
            settings_section_label(SettingsSection::Security, UiLanguage::English),
            "Security"
        );
        assert_eq!(
            settings_section_label(SettingsSection::Security, UiLanguage::Russian),
            "Безопасность"
        );
        assert_eq!(
            language_preference_label(LanguagePreference::System, UiLanguage::English),
            "System"
        );
        assert_eq!(
            update_channel_label(UpdateChannelPreference::Disabled, UiLanguage::English),
            "Disabled"
        );
        assert_eq!(
            update_channel_label(UpdateChannelPreference::Disabled, UiLanguage::Russian),
            "Отключено"
        );
    }

    #[test]
    fn update_source_prefers_explicit_settings() {
        let mut store = LauncherStore::default();
        assert_eq!(
            update_source_from_store(&store),
            Some(UpdateSource::GithubRelease(
                DEFAULT_UPDATE_GITHUB_REPO.to_owned()
            ))
        );

        store.update_channel = UpdateChannelPreference::ManifestUrl;
        store.update_manifest_url = " https://example.com/latest.json ".to_owned();
        assert_eq!(
            update_source_from_store(&store),
            Some(UpdateSource::ManifestUrl(
                "https://example.com/latest.json".to_owned()
            ))
        );

        store.update_channel = UpdateChannelPreference::GithubRelease;
        store.update_github_repo = "EvertyDesk/EvertyDesk_Lite".to_owned();
        assert_eq!(
            update_source_from_store(&store),
            Some(UpdateSource::GithubRelease(
                "EvertyDesk/EvertyDesk_Lite".to_owned()
            ))
        );
    }

    #[test]
    fn top_navigation_keeps_vm_and_game_as_first_class_pages() {
        assert_ne!(Page::Vm, Page::Settings);
        assert_ne!(Page::Game, Page::Settings);
        assert_ne!(Page::Vm, Page::Game);
    }

    #[test]
    fn support_request_layout_stacks_on_compact_windows() {
        assert!(!use_wide_support_layout(700.0));
        assert!(!use_wide_support_layout(879.0));
        assert!(use_wide_support_layout(880.0));
        assert!(use_wide_support_layout(1_200.0));
    }

    #[test]
    fn main_content_uses_inner_padding_so_scrollbar_stays_on_viewport_edge() {
        assert_eq!(main_content_side_padding(700.0), 16.0);
        assert_eq!(main_content_side_padding(820.0), MAIN_CONTENT_SIDE_PADDING);
        assert_eq!(main_content_max_width(900.0), 900.0);
        assert_eq!(main_content_max_width(1_920.0), MAIN_CONTENT_MAX_WIDTH);
    }

    #[test]
    fn vm_target_is_normalized_by_selected_provider() {
        assert_eq!(
            build_vm_target(VmProviderPreference::Auto, " hyperv:abc "),
            "hyperv:abc"
        );
        assert_eq!(
            build_vm_target(VmProviderPreference::HyperV, "abc"),
            "hyperv:abc"
        );
        assert_eq!(
            build_vm_target(VmProviderPreference::VirtualBox, "abc"),
            "vbox:abc"
        );
        assert_eq!(
            build_vm_target(VmProviderPreference::VirtualBox, "hyperv:abc"),
            "hyperv:abc"
        );
        assert!(build_vm_target(VmProviderPreference::HyperV, "  ").is_empty());
    }

    #[test]
    fn vm_provider_is_inferred_from_prefixed_ids() {
        assert_eq!(
            infer_vm_provider(" hyperv:abc "),
            Some(VmProviderPreference::HyperV)
        );
        assert_eq!(
            infer_vm_provider("vbox:abc"),
            Some(VmProviderPreference::VirtualBox)
        );
        assert_eq!(infer_vm_provider("abc"), None);
        assert_eq!(infer_vm_provider("proxmox:abc"), None);
    }

    #[test]
    fn vm_power_action_labels_are_short_for_card_controls() {
        assert_eq!(VmPowerAction::Start.label(), "Старт");
        assert_eq!(VmPowerAction::Stop.label(), "Стоп");
        assert_eq!(VmPowerAction::Restart.label(), "Ребут");
    }

    #[test]
    fn vm_inventory_groups_match_lite_providers() {
        assert_eq!(vm_inventory_group_key("hyperv:abc"), "1_hyperv");
        assert_eq!(vm_inventory_group_label("1_hyperv"), "HYPER-V");
        assert_eq!(vm_provider_label_for_id("vbox:abc"), "VIRTUALBOX");
        assert_eq!(vm_inventory_group_key("plain-guid"), "1_hyperv");
        assert_eq!(vm_provider_label_for_id("proxmox:abc"), "OTHER");
    }

    #[test]
    fn vm_inventory_filter_matches_name_id_state_and_provider() {
        let vm = VmInventoryEntry {
            id: "vbox:abc-123".to_owned(),
            name: "Cashbox Linux".to_owned(),
            state: "running".to_owned(),
            connectable: true,
        };
        assert!(vm_matches_filter(&vm, "cashbox"));
        assert!(vm_matches_filter(&vm, "ABC"));
        assert!(vm_matches_filter(&vm, "running"));
        assert!(vm_matches_filter(&vm, "virtualbox"));
        assert!(!vm_matches_filter(&vm, "hyper-v"));
        assert_eq!(sanitize_vm_filter("  name  "), "name  ");
        assert!(sanitize_vm_filter(&"x".repeat(120)).len() <= 96);
    }

    #[test]
    fn game_form_uses_same_remote_id_normalization_as_regular_connect() {
        assert_eq!(normalize_remote_id(" 123 456-789 "), "123456789");
        assert!(normalize_remote_id(" \t\r\n ").is_empty());
    }

    #[test]
    fn game_codec_profile_maps_to_viewer_bootstrap_codec() {
        assert_eq!(
            viewer_game_codec(GameCodecPreference::Auto),
            ViewerGameCodec::Auto
        );
        assert_eq!(
            viewer_game_codec(GameCodecPreference::H265),
            ViewerGameCodec::H265
        );
        assert_eq!(
            viewer_game_profile_label(true, ViewerGameCodec::H264, true),
            "Game H264 · EVRT2"
        );
        assert_eq!(
            viewer_game_profile_label(false, ViewerGameCodec::Av1, true),
            "Режим: Desktop · EVRTCK"
        );
    }

    #[test]
    fn viewer_launch_status_includes_game_profile_only_for_game_sessions() {
        let desktop = viewer_launch_status(
            ConnectionQuality::Balanced,
            false,
            ViewerGameCodec::Av1,
            true,
        );
        assert!(desktop.contains(ConnectionQuality::Balanced.label()));
        assert!(!desktop.contains("Game"));
        assert!(!desktop.contains("AV1"));

        let game =
            viewer_launch_status(ConnectionQuality::Smooth, true, ViewerGameCodec::H265, true);
        assert!(game.contains(ConnectionQuality::Smooth.label()));
        assert!(game.contains("Game H265"));
        assert!(game.contains("EVRT2"));
    }

    #[test]
    fn vm_inventory_json_is_rendered_as_compact_status() {
        let entries = parse_vm_inventory(
            r#"[{"id":"hyperv:1","name":"Win 11","state":"running","connectable":true}]"#,
        )
        .unwrap();
        let status = format_vm_inventory_entries(&entries);
        assert!(status.contains("Найдено VM: 1"));
        assert!(status.contains("hyperv:1 · Win 11 · running · доступна"));
        let empty = parse_vm_inventory("[]").unwrap();
        assert!(format_vm_inventory_entries(&empty).contains("VM не найдены"));
        assert!(parse_vm_inventory("{}").is_err());
    }

    #[test]
    fn smart_notification_links_allow_only_http_urls() {
        assert!(is_safe_notification_link("https://desk.everty.ru/help"));
        assert!(is_safe_notification_link(" http://desk.everty.ru/help "));
        assert!(!is_safe_notification_link(""));
        assert!(!is_safe_notification_link(
            "file:///C:/Windows/System32/calc.exe"
        ));
        assert!(!is_safe_notification_link("javascript:alert(1)"));
    }

    #[test]
    fn smart_notification_labels_and_colors_are_stable() {
        assert_eq!(smart_notification_type_label("support_ping"), "поддержка");
        assert_eq!(smart_notification_type_label("poll"), "опрос");
        assert_eq!(
            smart_notification_type_label("config_update"),
            "конфигурация"
        );
        assert_eq!(smart_notification_type_label("unknown"), "уведомление");
        assert_eq!(smart_notification_accent("error", "poll"), ACCENT);
        assert_eq!(
            smart_notification_accent("", "support_ping"),
            Color::from_rgb(0.12, 0.58, 0.35)
        );
    }

    #[test]
    fn support_message_is_sanitized_and_counted() {
        assert_eq!(sanitize_support_message("abc\u{0}def"), "abcdef");
        let long = "x".repeat(MAX_SUPPORT_MESSAGE_CHARS + 20);
        let sanitized = sanitize_support_message(&long);
        assert_eq!(sanitized.chars().count(), MAX_SUPPORT_MESSAGE_CHARS);
        assert_eq!(
            support_message_counter("abc"),
            format!("3/{MAX_SUPPORT_MESSAGE_CHARS}")
        );
    }

    #[test]
    fn default_server_values_are_hidden_and_empty_fields_restore_defaults() {
        let defaults = ServerConfig::default();
        assert_eq!(
            server_input_value(&defaults.id_server, &defaults.id_server),
            ""
        );
        assert_eq!(
            server_input_value("custom.example.com", &defaults.id_server),
            "custom.example.com"
        );
        assert_eq!(
            server_field_or_default("  ".to_owned(), defaults.relay_server.clone()),
            defaults.relay_server
        );
        assert_eq!(
            server_field_or_default(" relay.example.com ".to_owned(), String::new()),
            "relay.example.com"
        );
    }

    #[test]
    fn login_options_detect_oidc_provider_without_hardcoding_button() {
        let options = vec![" oidc/yandex ".to_owned(), "oidc/other".to_owned()];
        assert!(has_login_provider(&options, "yandex"));
        assert!(has_login_provider(&options, "YANDEX"));
        assert!(!has_login_provider(&options, "google"));
        assert!(!has_login_provider(&["yandex".to_owned()], "yandex"));
    }

    #[test]
    fn current_user_entitlements_handles_boolean_string_and_numeric_flags() {
        let (entitlements, summary) = current_user_entitlements(&serde_json::json!({
            "entitlements": {
                "has_smart_agent": true,
                "has_yandex_sso": "true",
                "has_ldap": 1,
                "has_client_builder": "on",
                "has_invoice_billing": "yes",
                "vm_mode": "false",
                "max_vm_slots": "5"
            }
        }));
        assert!(entitlements.known);
        assert!(entitlements.smart_agent);
        assert!(entitlements.yandex_sso);
        assert!(entitlements.ldap);
        assert!(entitlements.client_builder);
        assert!(entitlements.invoice_billing);
        assert!(!entitlements.vm);
        assert_eq!(entitlements.vm_slots, Some(5));
        assert!(summary.contains("Smart Agent"));
        assert!(summary.contains("Yandex SSO"));
        assert!(summary.contains("LDAP"));
        assert!(summary.contains("Client builder"));
        assert!(summary.contains("Invoice billing"));
        assert!(summary.contains("VM slots 5"));
        assert!(!summary.contains("VM,"));

        assert_eq!(
            current_user_entitlements_summary(&serde_json::json!({"entitlements": {}})),
            "Права аккаунта: активные тарифные фичи не найдены"
        );
    }

    #[test]
    fn peer_metadata_omits_generic_names_and_keeps_safe_diagnostics() {
        assert_eq!(
            peer_metadata("Office PC", "windows", "1.4.6").as_deref(),
            Some("Office PC · windows · версия 1.4.6")
        );
        assert_eq!(
            peer_metadata("EvertyDesk Lite", "windows", "").as_deref(),
            Some("windows")
        );
        assert_eq!(peer_metadata("", "", ""), None);
    }

    #[test]
    fn host_state_colors_cover_every_state() {
        let states = [
            HostState::Idle,
            HostState::Connecting,
            HostState::Ready,
            HostState::Accepting("peer".to_owned()),
            HostState::Error("offline".to_owned()),
        ];
        for state in states {
            assert!(host_state_color(&state).a > 0.0);
        }
    }

    #[cfg(windows)]
    #[test]
    fn tray_icon_has_valid_rgba_dimensions_and_transparent_corners() {
        let icon = tray_icon_rgba();
        assert_eq!(icon.len(), 32 * 32 * 4);
        assert_eq!(&icon[0..4], &[0, 0, 0, 0]);
        let center = (16 * 32 + 16) * 4;
        assert_eq!(icon[center + 3], 255);
    }

    #[test]
    fn performance_status_formats_lan_and_wan_rates() {
        assert_eq!(
            format_performance(5_998, 850, 0, 9, 0, UiLanguage::Russian),
            "Подключено 9с · 59.98 FPS · 850 Кбит/с"
        );
        assert_eq!(
            format_performance(3_000, 2_500, 4, 3_661, 2, UiLanguage::Russian),
            "Подключено 1ч 01м · 30.00 FPS · 2.5 Мбит/с · пропущено 4 · восстановлений 2"
        );
        assert_eq!(format_duration(125), "2м 05с");
    }

    #[test]
    fn recent_details_include_completed_session_metrics() {
        let connection = RecentConnection {
            remote_id: "123".to_owned(),
            last_used_unix: 0,
            direction: ConnectionDirection::Incoming,
            duration_seconds: 125,
            reconnect_count: 2,
            last_end_reason: "Завершено владельцем".to_owned(),
        };
        let details = recent_details(&connection);
        assert!(details.contains("Входящая"));
        assert!(details.contains("2м 05с"));
        assert!(details.contains("восстановлений 2"));
        assert!(details.contains("Завершено владельцем"));
    }

    #[test]
    fn red_primary_buttons_always_use_white_text() {
        let style = accent_button(&Theme::Light, iced::widget::button::Status::Active);
        assert_eq!(style.text_color, Color::WHITE);
        let disabled = accent_button(&Theme::Light, iced::widget::button::Status::Disabled);
        assert_eq!(disabled.text_color, Color::WHITE);
    }

    #[test]
    fn smart_agent_heartbeat_uses_three_bounded_fast_retries() {
        assert_eq!(smart_agent_heartbeat_interval(0), Duration::from_secs(60));
        assert_eq!(smart_agent_heartbeat_interval(1), Duration::from_secs(5));
        assert_eq!(smart_agent_heartbeat_interval(2), Duration::from_secs(10));
        assert_eq!(smart_agent_heartbeat_interval(3), Duration::from_secs(15));
        assert_eq!(smart_agent_heartbeat_interval(4), Duration::from_secs(60));
    }

    #[test]
    fn smart_agent_inbox_burst_expires_and_server_text_is_bounded() {
        let now = Instant::now();
        assert_eq!(
            smart_agent_inbox_interval(Some(now + Duration::from_secs(60)), now),
            Duration::from_secs(5)
        );
        assert_eq!(
            smart_agent_inbox_interval(Some(now), now),
            Duration::from_secs(30)
        );
        assert_eq!(bounded_text("abc\u{0}def", 5), "abcde…");
    }
}
