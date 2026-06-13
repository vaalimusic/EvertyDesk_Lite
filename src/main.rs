#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod address_book;
mod capture;
mod colorconv;
mod crypto;
mod diagnostics;
mod evrt;
mod evrt_audio;
mod evrt_client;
mod evrt_session;
mod frame_queue;
mod fsr;
mod host;
mod llm;
mod mf_encode;
mod mf_video;
mod netif;
mod nvenc;
mod rustdesk_proto;
mod settings;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod software_ui;
mod transport;
mod ui;
mod video;
mod video_pipeline;
mod videotoolbox;
mod vp9_mf;
#[cfg(feature = "live-vpx-system")]
mod vpx_system;

use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use eframe::egui::{self, ColorImage, TextureHandle};
use host::{HostEvent, HostService, HostState};
use rustdesk_proto::ControlKey;
use settings as settings_mod;
use settings_mod::{
    generate_numeric_token, AppConfig, CodecPreference, ConnectionHistoryEntry, ContactEntry,
    CoordinateMode,
};
use transport::{
    ConnectionRequest, ConnectionState, RemoteDisplay, SessionCommand, SessionEvent,
    TransportClient,
};
use ui::widgets::*;

const APP_NAME: &str = "EvertyDesk Lite";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_SESSION_LOG_LINES: usize = 4_000;
const SESSION_LOG_TRIM_LINES: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppMode {
    Connect,
    Host,
    History,
    Contacts,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UiLang {
    Ru,
    En,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectKind {
    Screen,
    Shell,
}

fn tr(lang: UiLang, ru: &'static str, en: &'static str) -> &'static str {
    match lang {
        UiLang::Ru => ru,
        UiLang::En => en,
    }
}

fn main() -> eframe::Result<()> {
    let cleanup = diagnostics::cleanup_default_artifacts();
    if cleanup.removed_total() > 0 || cleanup.errors > 0 {
        eprintln!(
            "[diagnostics] cleanup: removed_files={} removed_dirs={} errors={}",
            cleanup.removed_files, cleanup.removed_dirs, cleanup.errors
        );
    }
    if let Some(exit_code) = run_cli_connect() {
        std::process::exit(exit_code);
    }
    start_hung_window_guardian();

    let renderer_mode = std::env::var("EVERTYDESK_RENDERER")
        .unwrap_or_else(|_| "auto".to_owned())
        .to_ascii_lowercase();

    if renderer_mode != "host" && renderer_mode != "headless" && is_headless_graphics_session() {
        eprintln!(
            "[EvertyDesk] No interactive graphics session detected. Starting headless host mode."
        );
        run_headless_host();
        return Ok(());
    }
    if renderer_mode == "auto" {
        if let Some(reason) = server_basic_display_warning() {
            eprintln!("[EvertyDesk] {reason}");
            eprintln!("[EvertyDesk] GUI will still be attempted. Use --host for headless mode.");
        }
    }

    #[cfg(target_os = "linux")]
    if renderer_mode == "auto" && std::env::var_os("EVERTYDESK_LINUX_AUTOSTART_CHILD").is_none() {
        return run_linux_auto_gui();
    }

    match renderer_mode.as_str() {
        "glow" | "opengl" => return run_gui(eframe::Renderer::Glow),
        "wgpu" | "vulkan" => return run_gui(eframe::Renderer::Wgpu),
        // Pure-CPU framebuffer UI (minifb) — no OpenGL/GLX/Vulkan at all.
        // The reliable choice for VMs where GLX is broken (e.g. Astra Linux
        // on SVGA3D: "GLXBadContextTag"). Use: EVERTYDESK_RENDERER=software
        "software" | "minifb" | "cpu" | "softbuffer" => {
            run_software_ui_or_headless();
            return Ok(());
            #[cfg(all(target_os = "linux", any()))]
            {
                eprintln!("[EvertyDesk] Software (CPU) UI backend — no OpenGL.");
                if let Err(err) = software_ui::run_software_ui() {
                    eprintln!("[EvertyDesk] Software UI failed: {err}");
                    run_headless_host();
                }
                return Ok(());
            }
            #[cfg(all(not(target_os = "linux"), any()))]
            {
                eprintln!("[EvertyDesk] Software UI backend is Linux-only; using WGPU.");
                return run_gui(eframe::Renderer::Wgpu);
            }
        }
        "host" | "headless" => {
            run_headless_host();
            return Ok(());
        }
        _ => {}
    }

    match run_gui(eframe::Renderer::Wgpu) {
        Ok(()) => Ok(()),
        Err(wgpu_error) => {
            eprintln!("[EvertyDesk] WGPU renderer failed: {wgpu_error:?}");
            #[cfg(target_os = "linux")]
            {
                eprintln!(
                    "[EvertyDesk] On Linux, OpenGL fallback must be launched by the safe wrapper."
                );
                eprintln!(
                    "[EvertyDesk] Run ./scripts/run-linux-safe.sh or install with ./scripts/install-linux-user.sh.\n"
                );
                run_headless_host();
                Ok(())
            }
            #[cfg(not(target_os = "linux"))]
            {
                eprintln!("[EvertyDesk] Trying OpenGL/Glow renderer...");
                match run_gui(eframe::Renderer::Glow) {
                    Ok(()) => Ok(()),
                    Err(glow_error) => {
                        eprintln!("[EvertyDesk] Glow renderer failed: {glow_error:?}");
                        let msg = format!("{wgpu_error:?}\n{glow_error:?}");
                        if msg.contains("NoSuitableAdapterFound")
                            || msg.contains("NoSuitable")
                            || msg.contains("glutin")
                            || msg.contains("OpenGL")
                        {
                            eprintln!("\n[EvertyDesk] Нет подходящего графического режима.");
                            eprintln!(
                                "[EvertyDesk] GUI renderer failed. For a GUI window this system needs working OpenGL/Vulkan."
                            );
                            eprintln!(
                                "[EvertyDesk] CPU software UI is available only when explicitly requested with EVERTYDESK_RENDERER=software."
                            );
                            Err(glow_error)
                        } else {
                            Err(glow_error)
                        }
                    }
                }
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run_software_ui_or_headless() {
    if is_headless_graphics_session() {
        eprintln!(
            "[EvertyDesk] No graphics session for CPU software UI; using headless host mode."
        );
        run_headless_host();
        return;
    }
    eprintln!("[EvertyDesk] Trying CPU software UI backend (no OpenGL/Vulkan)...");
    match software_ui::run_software_ui() {
        Ok(()) => {}
        Err(err) => {
            eprintln!("[EvertyDesk] Software UI failed: {err}");
            eprintln!("[EvertyDesk] Starting headless host mode.");
            run_headless_host();
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn run_software_ui_or_headless() {
    eprintln!("[EvertyDesk] Software UI backend is unavailable on this platform.");
    run_headless_host();
}

#[cfg(target_os = "linux")]
fn run_linux_auto_gui() -> eframe::Result<()> {
    if is_headless_graphics_session() {
        eprintln!("[EvertyDesk] No desktop session detected. Starting headless host mode.");
        run_headless_host();
        return Ok(());
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            eprintln!("[EvertyDesk] Cannot locate current executable: {err}");
            run_headless_host();
            return Ok(());
        }
    };

    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut attempts: Vec<LinuxGuiAttempt> = Vec::new();
    let prefer_cpu = linux_prefers_cpu_renderer();

    if prefer_cpu {
        eprintln!("[EvertyDesk] Linux CPU-first renderer policy is active.");
        attempts.push(linux_cpu_gui_attempt());
    }

    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        attempts.push(LinuxGuiAttempt {
            title: "Wayland OpenGL software",
            renderer: "glow",
            envs: &[
                ("WINIT_UNIX_BACKEND", "wayland"),
                ("EVERTYDESK_SOFTWARE", "1"),
                ("LIBGL_ALWAYS_SOFTWARE", "1"),
                ("MESA_LOADER_DRIVER_OVERRIDE", "llvmpipe"),
            ],
        });
    }

    if std::env::var_os("DISPLAY").is_some() {
        attempts.push(LinuxGuiAttempt {
            title: "X11 OpenGL default",
            renderer: "glow",
            envs: &[("WINIT_UNIX_BACKEND", "x11"), ("LIBGL_DRI3_DISABLE", "1")],
        });
        attempts.push(LinuxGuiAttempt {
            title: "X11 OpenGL software",
            renderer: "glow",
            envs: &[
                ("WINIT_UNIX_BACKEND", "x11"),
                ("EVERTYDESK_SOFTWARE", "1"),
                ("LIBGL_ALWAYS_SOFTWARE", "1"),
                ("LIBGL_DRI3_DISABLE", "1"),
                ("MESA_LOADER_DRIVER_OVERRIDE", "llvmpipe"),
                ("GALLIUM_DRIVER", "llvmpipe"),
                ("MESA_GL_VERSION_OVERRIDE", "3.3"),
                ("MESA_GLSL_VERSION_OVERRIDE", "330"),
            ],
        });
        attempts.push(LinuxGuiAttempt {
            title: "X11 indirect GLX",
            renderer: "glow",
            envs: &[
                ("WINIT_UNIX_BACKEND", "x11"),
                ("LIBGL_ALWAYS_INDIRECT", "1"),
                ("LIBGL_DRI3_DISABLE", "1"),
            ],
        });
    }

    // Pure-CPU framebuffer (minifb) — no OpenGL/GLX/Vulkan. Guaranteed to work
    // on VMs with broken GLX (Astra/SVGA3D "GLXBadContextTag"). Tried as a
    // child so a crash in earlier GL attempts can't take us down with it.
    if !prefer_cpu {
        attempts.push(linux_cpu_gui_attempt());
    }

    if linux_env_truthy("EVERTYDESK_LINUX_AUTO_WGPU") {
        attempts.push(LinuxGuiAttempt {
            title: "WGPU auto",
            renderer: "wgpu",
            envs: &[],
        });
    }

    eprintln!("[EvertyDesk] Linux GUI autostart: checking available renderer...");
    let stable_after = linux_gui_child_stable_after();
    for attempt in attempts {
        eprintln!("[EvertyDesk] Trying {}...", attempt.title);
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(&args)
            .env("EVERTYDESK_LINUX_AUTOSTART_CHILD", "1")
            .env("EVERTYDESK_RENDERER", attempt.renderer)
            .env("RUST_BACKTRACE", "0")
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
        for (name, value) in attempt.envs {
            cmd.env(name, value);
        }

        match cmd.spawn() {
            Ok(mut child) => {
                let started = Instant::now();
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) if status.success() => return Ok(()),
                        Ok(Some(status)) => {
                            eprintln!("[EvertyDesk] {} failed: {status}", attempt.title);
                            break;
                        }
                        Ok(None) if started.elapsed() >= stable_after => {
                            eprintln!(
                                "[EvertyDesk] {} is still running after {:?}; using it.",
                                attempt.title, stable_after
                            );
                            return Ok(());
                        }
                        Ok(None) => thread::sleep(Duration::from_millis(100)),
                        Err(err) => {
                            eprintln!("[EvertyDesk] {} status failed: {err}", attempt.title);
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                eprintln!("[EvertyDesk] {} failed to start: {err}", attempt.title);
            }
        }
    }

    eprintln!("[EvertyDesk] No GUI renderer worked on this Linux desktop.");
    eprintln!("[EvertyDesk] This system rejected the automatic Linux GUI attempts.");
    eprintln!("[EvertyDesk] Starting CPU software UI backend...");
    if let Err(err) = software_ui::run_software_ui() {
        eprintln!("[EvertyDesk] Software UI failed: {err}");
    }
    Ok(())
}

fn is_headless_graphics_session() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

fn server_basic_display_warning() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        windows_basic_display_warning()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[cfg(target_os = "windows")]
fn windows_basic_display_warning() -> Option<String> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_DESC1};

    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1().ok()? };
    let mut hardware = Vec::new();
    let mut software = Vec::new();

    for index in 0..16_u32 {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        let mut desc = DXGI_ADAPTER_DESC1::default();
        if unsafe { adapter.GetDesc1(&mut desc) }.is_err() {
            continue;
        }
        let name = utf16_z_to_string(&desc.Description).unwrap_or_else(|| "unknown".to_owned());
        let software_adapter = (desc.Flags & 0x2) != 0;
        let info = WindowsDisplayAdapter {
            name,
            vendor_id: desc.VendorId,
            software: software_adapter,
        };
        if software_adapter {
            software.push(info);
        } else {
            hardware.push(info);
        }
    }

    if hardware.is_empty() && !software.is_empty() {
        let names = software
            .iter()
            .map(|adapter| adapter.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Some(format!(
            "Only software DXGI adapters detected ({names}); GUI may be slow on this server."
        ));
    }

    if !hardware.is_empty() && hardware.iter().all(WindowsDisplayAdapter::is_server_basic) {
        let names = hardware
            .iter()
            .map(|adapter| format!("{} vendor=0x{:04x}", adapter.name, adapter.vendor_id))
            .collect::<Vec<_>>()
            .join(", ");
        return Some(format!(
            "Server/basic display adapter detected ({names}); GUI may be slow on this server."
        ));
    }

    None
}

#[cfg(target_os = "windows")]
struct WindowsDisplayAdapter {
    name: String,
    vendor_id: u32,
    software: bool,
}

#[cfg(target_os = "windows")]
impl WindowsDisplayAdapter {
    fn is_server_basic(&self) -> bool {
        self.software
            || matches!(
                self.vendor_id,
                0x1414 // Microsoft Basic Render Driver / WARP
                    | 0x1a03 // ASPEED BMC adapters common on Supermicro servers
                    | 0x102b // Matrox server/BMC adapters
            )
            || {
                let name = self.name.to_ascii_lowercase();
                name.contains("microsoft basic")
                    || name.contains("aspeed")
                    || name.contains("matrox")
                    || name.contains("basic render")
                    || name.contains("basic display")
            }
    }
}

#[cfg(target_os = "windows")]
fn utf16_z_to_string(buf: &[u16]) -> Option<String> {
    let len = buf.iter().position(|ch| *ch == 0).unwrap_or(buf.len());
    let text = String::from_utf16_lossy(&buf[..len]).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(target_os = "linux")]
struct LinuxGuiAttempt {
    title: &'static str,
    renderer: &'static str,
    envs: &'static [(&'static str, &'static str)],
}

#[cfg(target_os = "linux")]
fn linux_cpu_gui_attempt() -> LinuxGuiAttempt {
    LinuxGuiAttempt {
        title: "CPU software framebuffer (minifb)",
        renderer: "software",
        envs: &[],
    }
}

#[cfg(target_os = "linux")]
fn linux_prefers_cpu_renderer() -> bool {
    if linux_env_truthy("EVERTYDESK_LINUX_GL_AUTO") {
        return false;
    }
    if linux_env_truthy("EVERTYDESK_LINUX_PREFER_CPU") {
        return true;
    }
    fs::read_to_string("/etc/os-release")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .contains("astra")
}

#[cfg(target_os = "linux")]
fn linux_gui_child_stable_after() -> Duration {
    const DEFAULT_MS: u64 = 3_000;
    let ms = std::env::var("EVERTYDESK_LINUX_GUI_STABLE_AFTER_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MS)
        .clamp(500, 30_000);
    Duration::from_millis(ms)
}

#[cfg(target_os = "linux")]
fn linux_env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Decode the embedded EvertyDesk logo (`edesk_lite_logo.png`) into a window
/// icon. Returns `None` if the image can't be decoded.
fn load_app_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/edesk_lite_logo.png"));
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

fn run_gui(renderer: eframe::Renderer) -> eframe::Result<()> {
    let wgpu_opts = eframe::egui_wgpu::WgpuConfiguration::default();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_inner_size([1180.0, 740.0])
        .with_min_inner_size([920.0, 600.0]);
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }

    let options = eframe::NativeOptions {
        viewport,
        hardware_acceleration: if std::env::var_os("EVERTYDESK_SOFTWARE").is_some() {
            eframe::HardwareAcceleration::Off
        } else {
            eframe::HardwareAcceleration::Preferred
        },
        wgpu_options: wgpu_opts,
        renderer,
        ..Default::default()
    };

    let result = eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| {
            configure_ui_scale(&cc.egui_ctx);
            configure_style(&cc.egui_ctx);
            eprintln!(
                "[EvertyDesk] Build codecs: {}",
                crate::video::build_codec_label()
            );
            Ok(Box::new(EvertyDeskApp::new()))
        }),
    );
    if result.is_ok() {
        std::process::exit(0);
    }
    result
}

/// Headless host loop — same as `--host` CLI mode, used as auto-fallback
/// when the GUI cannot initialize (no GPU adapter on the server).
fn run_headless_host() {
    let config = AppConfig::load_or_create();
    eprintln!(
        "[cli] Headless host mode. local_id={} server={}",
        config.local_id, config.server.id_server
    );
    let svc = host::HostService::start(config);
    loop {
        thread::sleep(Duration::from_millis(100));
        while let Some(ev) = svc.try_recv() {
            match &ev {
                host::HostEvent::Log(msg) => eprintln!("[host] {msg}"),
                host::HostEvent::StateChanged(s) => eprintln!("[state] {s:?}"),
                host::HostEvent::Registered { request_pk } => {
                    eprintln!("[host] Registered request_pk={request_pk}")
                }
                host::HostEvent::IncomingRequest { peer_id, .. } => {
                    eprintln!("[host] Incoming from {peer_id}")
                }
                host::HostEvent::ApprovalRequested { peer_id } => {
                    eprintln!(
                        "[host] Approval requested from {peer_id}; GUI confirmation is required"
                    )
                }
                host::HostEvent::SessionStarted { peer_id } => {
                    eprintln!("[host] Session started: {peer_id}")
                }
                host::HostEvent::SessionEnded { peer_id, reason } => {
                    eprintln!("[host] Session ended: {peer_id} {reason}")
                }
                host::HostEvent::VideoTelemetry {
                    summary,
                    fallback_reason,
                } => {
                    if let Some(reason) = fallback_reason {
                        eprintln!("[host-video] {summary}; fallback={reason}");
                    } else {
                        eprintln!("[host-video] {summary}");
                    }
                }
            }
        }
    }
}

fn egui_software_backend_active() -> bool {
    std::env::var_os("EVERTYDESK_EGUI_SOFTWARE").is_some()
}

fn commercial_ui_enabled() -> bool {
    std::env::var_os("EVERTYDESK_CLASSIC_UI").is_none()
}

fn run_cli_connect() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    let command = args.next()?;
    if command == "--online" {
        let remote_id = normalize_remote_id(&args.next().unwrap_or_default());
        if remote_id.is_empty() {
            eprintln!("Usage: evertydesk-lite --online <remote-id>");
            return Some(2);
        }
        let config = AppConfig::load_or_create();
        return match TransportClient::query_peer_online(
            &config.server,
            &config.local_id,
            &remote_id,
        ) {
            Ok(true) => {
                println!("{remote_id}: online");
                Some(0)
            }
            Ok(false) => {
                println!("{remote_id}: offline");
                Some(3)
            }
            Err(err) => {
                eprintln!("Error: {err}");
                Some(1)
            }
        };
    }

    if command == "--host" {
        // Headless host mode: start the host service, stream all events to
        // stderr.  Useful for diagnosing registration issues without the GUI.
        //
        // Optional flags (after --host):
        //   --bind-port PORT   Bind UDP socket to PORT instead of random.
        //                      Use the port the EvertyDesk service was using
        //                      (e.g. 63624) after stopping the service.
        //   --use-everty-keys  Read the installed EvertyDesk's Ed25519 key
        //                      pair and use it for hbbs RegisterPk.
        let mut config = AppConfig::load_or_create();
        // Parse extra flags
        let extra_args: Vec<String> = args.collect();
        for extra_arg in &extra_args {
            match extra_arg.as_str() {
                "--use-everty-keys" => {
                    if let Some(pk) = load_everty_public_key() {
                        eprintln!("[cli] Loaded real Ed25519 public key from EvertyDesk config");
                        config.host_pk = pk;
                    } else {
                        eprintln!("[cli] Warning: could not load EvertyDesk key pair");
                    }
                }
                s if s.starts_with("--bind-port") => {
                    // --bind-port PORT  or  --bind-port=PORT
                    let port_str = if let Some(p) = s.strip_prefix("--bind-port=") {
                        p.to_owned()
                    } else {
                        // try next element in extra_args (not supported easily, use = form)
                        String::new()
                    };
                    if let Ok(p) = port_str.parse::<u16>() {
                        eprintln!("[cli] UDP bind port: {p}");
                        config.udp_bind_port = p;
                    }
                }
                _ => {}
            }
        }
        let config = config; // freeze
        eprintln!("[cli] Starting host service (headless). Press Ctrl-C to stop.");
        eprintln!(
            "[cli] local_id={} server={}",
            config.local_id, config.server.id_server
        );
        let svc = host::HostService::start(config);
        loop {
            std::thread::sleep(Duration::from_millis(100));
            while let Some(ev) = svc.try_recv() {
                match &ev {
                    host::HostEvent::Log(msg) => eprintln!("[host] {msg}"),
                    host::HostEvent::StateChanged(s) => eprintln!("[state] {:?}", s),
                    host::HostEvent::Registered { request_pk } => {
                        eprintln!("[host] Registered request_pk={request_pk}")
                    }
                    host::HostEvent::IncomingRequest { peer_id, .. } => {
                        eprintln!("[host] Incoming from {peer_id}")
                    }
                    host::HostEvent::ApprovalRequested { peer_id } => {
                        eprintln!("[host] Approval requested from {peer_id}; GUI confirmation is required")
                    }
                    host::HostEvent::SessionStarted { peer_id } => {
                        eprintln!("[host] Session started: {peer_id}")
                    }
                    host::HostEvent::SessionEnded { peer_id, reason } => {
                        eprintln!("[host] Session ended: {peer_id} {reason}")
                    }
                    host::HostEvent::VideoTelemetry {
                        summary,
                        fallback_reason,
                    } => {
                        if let Some(reason) = fallback_reason {
                            eprintln!("[host-video] {summary}; fallback={reason}");
                        } else {
                            eprintln!("[host-video] {summary}");
                        }
                    }
                }
            }
        }
    }

    // ── Автоматическая диагностика ────────────────────────────────────────────
    // evertydesk-lite --diagnose <remote-id> [password] [--secs N] [--out DIR]
    // Подключается, гоняет полную сессию N секунд, собирает телеметрию,
    // пишет структурированный отчёт (md + json). Заменяет ручной разбор логов.
    if command == "--diagnose" {
        let remote_id = normalize_remote_id(&args.next().unwrap_or_default());
        if remote_id.is_empty() {
            eprintln!(
                "Usage: evertydesk-lite --diagnose <remote-id> [password] [--secs N] [--out DIR]"
            );
            return Some(2);
        }
        let rest: Vec<String> = args.collect();
        // password — первый позиционный аргумент, не начинающийся с --
        let password = rest
            .iter()
            .find(|a| !a.starts_with("--"))
            .cloned()
            .unwrap_or_default();
        let mut secs = 20u64;
        let mut out_dir = "diagnostics".to_owned();
        let mut it = rest.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--secs" => {
                    if let Some(v) = it.next() {
                        secs = v.parse().unwrap_or(20);
                    }
                }
                "--out" => {
                    if let Some(v) = it.next() {
                        out_dir = v.clone();
                    }
                }
                _ => {}
            }
        }
        return Some(crate::diagnostics::run_diagnose(
            &remote_id, &password, secs, &out_dir,
        ));
    }

    if command != "--connect" {
        return None;
    }

    let remote_id = normalize_remote_id(&args.next().unwrap_or_default());
    let password = args.next().unwrap_or_default();
    if remote_id.is_empty() {
        eprintln!("Usage: evertydesk-lite --connect <remote-id> [password]");
        return Some(2);
    }

    let config = AppConfig::load_or_create();
    if is_own_remote_id(&remote_id, &config.local_id) {
        eprintln!("Error: remote ID equals this computer ID ({remote_id})");
        return Some(2);
    }
    let request = ConnectionRequest {
        remote_id,
        password,
        server: config.server,
        display: config.display,
        control_only: false,
    };

    match TransportClient::connect_with_progress(request, |pct, message| {
        println!("{pct}% - {message}");
    }) {
        Ok(state) => {
            println!("OK: {}", state.as_text());
            Some(0)
        }
        Err(err) => {
            eprintln!("Error: {err}");
            Some(1)
        }
    }
}

struct EvertyDeskApp {
    config: AppConfig,
    remote_id: String,
    password: String,
    show_password: bool,
    show_host_password: bool,
    connect_kind: ConnectKind,
    shell_window_open: bool,
    shell_output: String,
    shell_input: String,
    shell_history: Vec<String>,
    shell_history_pos: Option<usize>,
    shell_last_command: String,
    terminal_goal: String,
    terminal_ai_answer: String,
    terminal_ai_status: Option<String>,
    terminal_ai_rx: Option<Receiver<Result<String, String>>>,
    terminal_auto_pending: bool,
    terminal_auto_request_at: Option<Instant>,
    mode: AppMode,
    ui_lang: UiLang,
    new_contact_name: String,
    new_contact_id: String,
    new_contact_note: String,
    contact_search: String,
    address_book_status: Option<String>,
    show_address_book_auth: bool,
    show_new_contact_dialog: bool,
    selected_contact_idx: Option<usize>,
    contact_details_draft: Option<ContactEntry>,
    service_status: Option<String>,
    status: String,
    host_status: String,
    host_video_status: Option<String>,
    last_error: Option<String>,
    connection_state: ConnectionState,
    worker: Option<Receiver<WorkerEvent>>,
    session_tx: Option<mpsc::Sender<SessionCommand>>,
    busy: bool,
    host_check_busy: bool,
    connected: bool,
    remote_viewer_open: bool,
    remote_viewer_window_spawned: bool,
    remote_fullscreen: bool,
    progress: u8,
    events: Vec<String>,
    session_log: Vec<String>,
    remote_texture: Option<TextureHandle>,
    app_logo_texture: Option<TextureHandle>,
    pending_image: Option<ColorImage>,
    last_frame_rgba: Vec<u8>,
    remote_size: [usize; 2],
    text_to_send: String,
    clipboard_status: Option<String>,
    screenshot_status: Option<String>,
    log_status: Option<String>,
    report_status: Option<String>,
    remote_input_focused: bool,
    remote_modifiers_down: RemoteModifierState,
    last_mouse_pos: Option<(i32, i32)>,
    remote_displays: Vec<RemoteDisplay>,
    selected_display: i32,
    auto_refresh: bool,
    refresh_millis: u64,
    video_fps: i32,
    fit_to_window: bool,
    coordinate_mode: CoordinateMode,
    screenshot_count: u64,
    live_frame_count: u64,
    screenshot_frame_count: u64,
    screenshot_pending: bool,
    last_screenshot_at: Option<Instant>,
    last_screenshot_sid: String,
    last_frame_codec: String,
    input_events_sent: u64,
    last_move_refresh_at: Option<Instant>,
    fps_last_at: Instant,
    fps_last_count: u64,
    display_fps: f32,
    stream_input_fps: f32,
    stream_input_kbps: u64,
    frame_bytes: usize,
    frame_queue_ms: u64,
    frame_decode_ms: u64,
    frame_dropped: usize,
    last_stream_tune_at: Option<Instant>,
    last_video_metric_log_at: Option<Instant>,
    png_fallback_started_at: Option<Instant>,
    /// When the last live (VP9/H264) frame was received.
    /// Used to suppress PNG screenshot frames while live video is active.
    last_live_frame_at: Option<Instant>,
    stream_health: String,
    wheel_accum: egui::Vec2,

    // ── EVRT статус ───────────────────────────────────────────────────────────
    /// EVRT прямой UDP активен
    evrt_active: bool,
    /// Адрес хоста для EVRT
    evrt_host_addr: String,
    /// Давление (normal/high/critical)
    evrt_pressure: String,
    /// Задержка прибытия пакетов (мс)
    evrt_arrival_delta_ms: i32,
    evrt_assembly_delay_ms: i32,
    /// Задержка декодирования (мс)
    evrt_decode_delta_ms: i32,
    /// Jitter буфер (мс)
    evrt_jitter_ms: u32,
    /// FPS EVRT
    evrt_fps: u32,
    evrt_packets_received: u64,
    evrt_frames_assembled: u64,
    evrt_reassembly_drops: u64,
    evrt_queue_drops: u64,
    /// Показывать окно с детальной диагностикой стрима.
    show_stream_info: bool,
    /// Cache of remote cursor images by cursor ID (RGBA + hotspot).
    cursor_cache: HashMap<u64, CursorCacheEntry>,
    /// Pending cursor image to be loaded into a texture in the next frame.
    pending_cursor: Option<(u64, egui::ColorImage, i32, i32)>,
    /// Currently active cursor texture + hotspot offset.
    cursor_texture: Option<(egui::TextureHandle, i32, i32)>,
    /// Last known cursor position in remote-screen coordinates.
    cursor_pos: Option<egui::Pos2>,
    /// Last measured round-trip latency (ms) from TestDelay.
    latency_ms: Option<u32>,

    // ── Host service ──────────────────────────────────────────────────────────
    /// Background host service (None = stopped).
    host_service: Option<HostService>,
    /// Last known host-service state.
    host_state: HostState,
    /// Log lines received from the host service.
    host_log: Vec<String>,
    /// Pending incoming connection (peer_id) waiting for Accept/Reject.
    host_pending_peer: Option<String>,

    // ── Settings window ───────────────────────────────────────────────────────
    /// Whether the settings panel is visible.
    show_settings: bool,
    /// Editable copy of config while the settings window is open.
    settings_draft: Option<AppConfig>,

    // ── FSR (клиентская сторона) ──────────────────────────────────────────────
    /// Адаптер FSR — апскейлит входящий видео-поток перед отображением.
    /// `None` = FSR выключен (нативное разрешение от хоста).
    fsr_viewer: Option<crate::fsr::FsrAdapter>,
    /// Нативное разрешение хоста (объявляется в PeerInfo / SessionEvent::Displays).
    /// FSR апскейлит каждый входящий кадр до этого разрешения.
    fsr_native_size: Option<(u32, u32)>,
    // ── System tray ───────────────────────────────────────────────────────────
}

struct CursorCacheEntry {
    image: Option<egui::ColorImage>,
    texture: Option<egui::TextureHandle>,
    hotx: i32,
    hoty: i32,
}

enum WorkerEvent {
    Session(SessionEvent),
    HostServerCheck(Result<(), String>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RemoteModifierState {
    alt: bool,
    shift: bool,
    ctrl: bool,
    meta: bool,
}

impl RemoteModifierState {
    fn from_egui(modifiers: egui::Modifiers) -> Self {
        Self {
            alt: modifiers.alt,
            shift: modifiers.shift,
            ctrl: modifiers.ctrl,
            meta: modifiers.mac_cmd,
        }
    }

    fn for_each(self, mut f: impl FnMut(ControlKey, bool)) {
        f(ControlKey::Control, self.ctrl);
        f(ControlKey::Alt, self.alt);
        f(ControlKey::Shift, self.shift);
        f(ControlKey::Meta, self.meta);
    }
}

fn forward_session_events(
    session_rx: Receiver<SessionEvent>,
    ui_events: mpsc::Sender<WorkerEvent>,
) {
    while let Ok(first) = session_rx.recv() {
        let mut latest_frame = None;
        let mut terminal = false;
        forward_or_coalesce_session_event(first, &ui_events, &mut latest_frame, &mut terminal);

        loop {
            match session_rx.try_recv() {
                Ok(event) => {
                    forward_or_coalesce_session_event(
                        event,
                        &ui_events,
                        &mut latest_frame,
                        &mut terminal,
                    );
                    if terminal {
                        break;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    terminal = true;
                    break;
                }
            }
        }

        if let Some(frame) = latest_frame {
            if !terminal {
                let _ = ui_events.send(WorkerEvent::Session(frame));
            }
        }
        if terminal {
            break;
        }
    }
}

fn forward_or_coalesce_session_event(
    event: SessionEvent,
    ui_events: &mpsc::Sender<WorkerEvent>,
    latest_frame: &mut Option<SessionEvent>,
    terminal: &mut bool,
) {
    if matches!(event, SessionEvent::Frame { .. }) {
        *latest_frame = Some(event);
        return;
    }
    *terminal = matches!(event, SessionEvent::Closed | SessionEvent::Failed(_));
    let _ = ui_events.send(WorkerEvent::Session(event));
}

impl EvertyDeskApp {
    fn new() -> Self {
        let config = AppConfig::load_or_create();
        let remote_id = config.ui.last_remote_id.clone();
        let auto_refresh = config.ui.auto_refresh;
        let refresh_millis = config.ui.refresh_millis.clamp(50, 2000).min(80);
        let fit_to_window = config.ui.fit_to_window;
        let coordinate_mode = config.ui.coordinate_mode;
        let video_fps = config.display.target_fps.clamp(5, 60) as i32;
        let host_service = Some(HostService::start(config.clone()));
        Self {
            config,
            remote_id,
            password: String::new(),
            show_password: false,
            show_host_password: false,
            connect_kind: ConnectKind::Screen,
            shell_window_open: false,
            shell_output: String::new(),
            shell_input: String::new(),
            shell_history: Vec::new(),
            shell_history_pos: None,
            shell_last_command: String::new(),
            terminal_goal: String::new(),
            terminal_ai_answer: String::new(),
            terminal_ai_status: None,
            terminal_ai_rx: None,
            terminal_auto_pending: false,
            terminal_auto_request_at: None,
            mode: AppMode::Connect,
            ui_lang: UiLang::Ru,
            new_contact_name: String::new(),
            new_contact_id: String::new(),
            new_contact_note: String::new(),
            contact_search: String::new(),
            address_book_status: None,
            show_address_book_auth: false,
            show_new_contact_dialog: false,
            selected_contact_idx: None,
            contact_details_draft: None,
            service_status: None,
            host_status: "Доступ запускается автоматически.".to_owned(),
            host_video_status: None,
            status: "Готово".to_owned(),
            last_error: None,
            connection_state: ConnectionState::Idle,
            worker: None,
            session_tx: None,
            busy: false,
            host_check_busy: false,
            connected: false,
            remote_viewer_open: false,
            remote_viewer_window_spawned: false,
            remote_fullscreen: false,
            progress: 0,
            events: vec!["App started".to_owned()],
            session_log: vec!["App started".to_owned()],
            remote_texture: None,
            app_logo_texture: None,
            pending_image: None,
            last_frame_rgba: Vec::new(),
            remote_size: [0, 0],
            text_to_send: String::new(),
            clipboard_status: None,
            screenshot_status: None,
            log_status: None,
            report_status: None,
            remote_input_focused: false,
            remote_modifiers_down: RemoteModifierState::default(),
            last_mouse_pos: None,
            remote_displays: Vec::new(),
            selected_display: 0,
            auto_refresh,
            refresh_millis,
            video_fps,
            fit_to_window,
            coordinate_mode,
            screenshot_count: 0,
            live_frame_count: 0,
            screenshot_frame_count: 0,
            screenshot_pending: false,
            last_screenshot_at: None,
            last_screenshot_sid: String::new(),
            last_frame_codec: "none".to_owned(),
            input_events_sent: 0,
            last_move_refresh_at: None,
            fps_last_at: Instant::now(),
            fps_last_count: 0,
            display_fps: 0.0,
            stream_input_fps: 0.0,
            stream_input_kbps: 0,
            frame_bytes: 0,
            frame_queue_ms: 0,
            frame_decode_ms: 0,
            frame_dropped: 0,
            last_stream_tune_at: None,
            last_video_metric_log_at: None,
            png_fallback_started_at: None,
            last_live_frame_at: None,
            stream_health: "ожидание кадра".to_owned(),
            wheel_accum: egui::Vec2::ZERO,
            evrt_active: false,
            evrt_host_addr: String::new(),
            evrt_pressure: "normal".to_owned(),
            evrt_arrival_delta_ms: -1,
            evrt_assembly_delay_ms: -1,
            evrt_decode_delta_ms: -1,
            evrt_jitter_ms: 0,
            evrt_fps: 0,
            evrt_packets_received: 0,
            evrt_frames_assembled: 0,
            evrt_reassembly_drops: 0,
            evrt_queue_drops: 0,
            show_stream_info: false,
            cursor_cache: HashMap::new(),
            pending_cursor: None,
            cursor_texture: None,
            cursor_pos: None,
            latency_ms: None,
            host_service,
            host_state: HostState::Connecting,
            host_log: vec![format!("[{}] Автозапуск доступа...", timestamp_hms())],
            host_pending_peer: None,
            show_settings: false,
            settings_draft: None,

            // FSR: включается из config.display.fsr_quality
            fsr_viewer: {
                let cfg = AppConfig::load_or_create();
                cfg.display.fsr_quality.to_fsr_quality().map(|q| {
                    crate::fsr::FsrAdapter::new(crate::fsr::FsrConfig {
                        quality: q,
                        sharpness: cfg.display.fsr_sharpness,
                    })
                })
            },
            fsr_native_size: None,
        }
    }

    fn connect(&mut self) {
        let normalized_remote_id = normalize_remote_id(&self.remote_id);
        if is_own_remote_id(&normalized_remote_id, &self.config.local_id) {
            self.set_error("Нельзя подключиться к своему же ID. Откройте EvertyDesk на другом ПК и введите его ID.");
            return;
        }
        let request = ConnectionRequest {
            remote_id: normalized_remote_id.clone(),
            password: self.password.clone(),
            server: self.config.server.clone(),
            display: self.config.display.clone(),
            control_only: false,
        };

        if request.remote_id.is_empty() {
            self.set_error("Введите ID удаленного ПК");
            return;
        }
        if false && request.password.is_empty() {
            self.set_error("Введите пароль");
            return;
        }

        self.last_error = None;
        self.busy = true;
        self.connected = false;
        self.remote_viewer_open = false;
        self.remote_viewer_window_spawned = false;
        self.shell_window_open = false;
        self.remote_fullscreen = false;
        self.remote_id = normalized_remote_id;
        self.save_ui_config();
        self.remote_texture = None;
        self.remote_size = [0, 0];
        self.last_frame_rgba.clear();
        self.fsr_native_size = None; // сбросить при новом подключении
        self.clipboard_status = None;
        self.screenshot_status = None;
        self.log_status = None;
        self.report_status = None;
        self.session_log.clear();
        self.log(format!("Session started for {}", self.remote_id));
        self.remote_displays.clear();
        self.screenshot_count = 0;
        self.live_frame_count = 0;
        self.screenshot_frame_count = 0;
        self.screenshot_pending = false;
        self.last_screenshot_at = None;
        self.last_screenshot_sid.clear();
        self.last_frame_codec = "none".to_owned();
        self.video_fps = self.config.display.target_fps.clamp(5, 60) as i32;
        self.frame_bytes = 0;
        self.frame_queue_ms = 0;
        self.frame_decode_ms = 0;
        self.frame_dropped = 0;
        self.last_stream_tune_at = None;
        self.last_video_metric_log_at = None;
        self.png_fallback_started_at = None;
        self.last_live_frame_at = None;
        self.stream_health = "ожидание кадра".to_owned();
        self.input_events_sent = 0;
        self.last_move_refresh_at = None;
        self.fps_last_at = Instant::now();
        self.fps_last_count = 0;
        self.display_fps = 0.0;
        self.stream_input_fps = 0.0;
        self.stream_input_kbps = 0;
        self.wheel_accum = egui::Vec2::ZERO;
        self.selected_display = 0;
        self.remote_input_focused = false;
        self.remote_modifiers_down = RemoteModifierState::default();
        self.progress = 1;
        self.status = format!("Подключение к {}", request.remote_id);
        self.log(self.status.clone());

        let (ui_tx, rx) = mpsc::channel();
        let (command_tx, command_rx) = mpsc::channel();
        self.session_tx = Some(command_tx);
        thread::spawn(move || {
            let (session_tx, session_rx) = mpsc::channel();
            let ui_events = ui_tx.clone();
            thread::spawn(move || {
                TransportClient::run_session(request, command_rx, session_tx);
            });
            forward_session_events(session_rx, ui_events);
        });
        self.worker = Some(rx);
    }

    fn poll_worker(&mut self) {
        let Some(rx) = self.worker.take() else {
            return;
        };

        let mut latest_frame = None;
        loop {
            match rx.try_recv() {
                Ok(WorkerEvent::Session(event)) => {
                    if matches!(event, SessionEvent::Frame { .. }) {
                        latest_frame = Some(event);
                        continue;
                    }
                    let terminal = matches!(event, SessionEvent::Failed(_) | SessionEvent::Closed);
                    self.handle_session_event(event);
                    if terminal {
                        return;
                    }
                }
                Ok(WorkerEvent::HostServerCheck(result)) => {
                    self.host_check_busy = false;
                    match result {
                        Ok(()) => {
                            self.host_status = "ID server доступен. Следующий этап: регистрация этого ПК и прием relay-сессии.".to_owned();
                            self.log("Host check: ID server reachable".to_owned());
                        }
                        Err(err) => {
                            self.host_status = format!("ID server недоступен: {err}");
                            self.log(format!("Host check failed: {err}"));
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if let Some(frame) = latest_frame {
                        self.handle_session_event(frame);
                    }
                    self.worker = Some(rx);
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    if let Some(frame) = latest_frame {
                        self.handle_session_event(frame);
                    }
                    self.busy = false;
                    self.host_check_busy = false;
                    if self.connected {
                        self.set_error("Background task stopped unexpectedly");
                    } else {
                        self.worker = None;
                    }
                    return;
                }
            }
        }
    }

    fn handle_session_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Progress(pct, message) => {
                self.progress = pct;
                self.status = format!("{pct}% - {message}");
                self.log(self.status.clone());
            }
            SessionEvent::Connected(info) => {
                self.busy = false;
                self.connected = true;
                self.remote_viewer_open = self.connect_kind == ConnectKind::Screen;
                self.remote_viewer_window_spawned = false;
                self.shell_window_open = self.connect_kind == ConnectKind::Shell;
                self.progress = 100;
                self.connection_state = ConnectionState::RelayReady {
                    remote_id: self.remote_id.clone(),
                };
                self.status = "Подключено".to_owned();
                self.log(format!("Connected: {info}"));
                if self.connect_kind == ConnectKind::Shell {
                    self.shell_output.clear();
                    self.send_command(SessionCommand::ShellStart);
                } else {
                    self.send_command(SessionCommand::SetAutoRefresh {
                        enabled: self.auto_refresh,
                        millis: self.refresh_millis,
                    });
                }
            }
            SessionEvent::Frame {
                sid,
                codec,
                width,
                height,
                rgba,
            } => {
                let expected_len = width.saturating_mul(height).saturating_mul(4);
                if width == 0 || height == 0 || rgba.len() != expected_len {
                    self.stream_health = format!(
                        "bad frame geometry: {}x{}, rgba={} expected={}",
                        width,
                        height,
                        rgba.len(),
                        expected_len
                    );
                    self.log(format!("Dropped frame: {}", self.stream_health));
                    return;
                }
                if codec == "PNG" {
                    // Discard PNG screenshot frames while live video (VP9/H264) is
                    // actively streaming — they only cause codec badge flicker and
                    // would replace a fresher VP9 frame with an older screenshot.
                    // RustDesk never mixes live video with screenshot mode.
                    let live_active = self
                        .last_live_frame_at
                        .map(|t| t.elapsed() < Duration::from_secs(2))
                        .unwrap_or(false);
                    if live_active {
                        return;
                    }
                    self.screenshot_frame_count += 1;
                    if self.png_fallback_started_at.is_none() {
                        self.png_fallback_started_at = Some(Instant::now());
                    }
                } else {
                    self.live_frame_count += 1;
                    self.last_live_frame_at = Some(Instant::now());
                    self.png_fallback_started_at = None;
                }
                self.update_render_fps();

                // ── FSR на стороне клиента ─────────────────────────────────
                // Если FSR включён и хост шлёт кадры в пониженном разрешении,
                // апскейлим обратно до нативного (fsr_native_size) перед
                // созданием текстуры — никаких изменений в протоколе не нужно.
                let (final_rgba, final_w, final_h) = if let Some(ref mut fsr) = self.fsr_viewer {
                    let (native_w, native_h) = self
                        .fsr_native_size
                        .unwrap_or((width as u32, height as u32));

                    // Конвертируем RGBA → BGRA для FSR (FSR работает с BGRA)
                    let mut bgra = vec![0u8; rgba.len()];
                    for i in (0..rgba.len()).step_by(4) {
                        bgra[i] = rgba[i + 2]; // B
                        bgra[i + 1] = rgba[i + 1]; // G
                        bgra[i + 2] = rgba[i]; // R
                        bgra[i + 3] = rgba[i + 3]; // A
                    }

                    let upscaled_bgra = fsr
                        .process_bgra(&bgra, width as u32, height as u32, native_w, native_h)
                        .to_owned();

                    // BGRA → RGBA обратно
                    let mut out_rgba = vec![0u8; upscaled_bgra.len()];
                    for i in (0..upscaled_bgra.len()).step_by(4) {
                        out_rgba[i] = upscaled_bgra[i + 2]; // R
                        out_rgba[i + 1] = upscaled_bgra[i + 1]; // G
                        out_rgba[i + 2] = upscaled_bgra[i]; // B
                        out_rgba[i + 3] = 255;
                    }

                    (out_rgba, native_w as usize, native_h as usize)
                } else {
                    (rgba, width, height)
                };

                self.last_frame_rgba = final_rgba;
                let image =
                    ColorImage::from_rgba_unmultiplied([final_w, final_h], &self.last_frame_rgba);
                self.remote_size = image.size;
                self.pending_image = Some(image);
                self.last_screenshot_sid = sid;
                self.last_frame_codec = codec;
                self.last_screenshot_at = Some(Instant::now());
                if self.screenshot_count <= 1 || self.screenshot_count % 20 == 0 {
                    self.log(format!(
                        "Frame received: {} ({})",
                        self.last_screenshot_sid, self.last_frame_codec
                    ));
                }
            }
            SessionEvent::ScreenshotStats { received, pending } => {
                self.screenshot_count = received;
                self.screenshot_pending = pending;
            }
            SessionEvent::FrameMetrics {
                bytes,
                queue_ms,
                decode_ms,
                dropped,
            } => {
                self.frame_bytes = bytes;
                self.frame_queue_ms = queue_ms;
                self.frame_decode_ms = decode_ms;
                self.frame_dropped = dropped;
                self.auto_tune_stream();
            }
            SessionEvent::VideoPacketMetrics {
                input_fps,
                input_kbps,
            } => {
                self.stream_input_fps = input_fps;
                self.stream_input_kbps = input_kbps;
                if self.last_frame_codec != "PNG" && input_fps > 0.1 {
                    if input_fps < 8.0 {
                        self.stream_health = "низкий входящий поток: хост/сеть".to_owned();
                    } else if self.display_fps + 3.0 < input_fps * 0.6 {
                        self.stream_health = "декодер/отрисовка отстаёт".to_owned();
                    }
                }
                if self
                    .last_video_metric_log_at
                    .map(|instant| instant.elapsed() >= Duration::from_secs(3))
                    .unwrap_or(true)
                {
                    self.last_video_metric_log_at = Some(Instant::now());
                    let latency = self
                        .latency_ms
                        .map(|ms| format!("{ms} ms"))
                        .unwrap_or_else(|| "n/a".to_owned());
                    self.log(format!(
                        "Video telemetry: in={input_fps:.1} fps / {input_kbps} kbps, render={:.1} fps, codec={}, packet={} KB, queue={} ms, decode={} ms, drop={}, latency={}, health={}",
                        self.display_fps,
                        self.last_frame_codec,
                        self.frame_bytes / 1024,
                        self.frame_queue_ms,
                        self.frame_decode_ms,
                        self.frame_dropped,
                        latency,
                        self.stream_health
                    ));
                }
            }
            SessionEvent::Displays(displays) => {
                let previous_selected_display = self.selected_display;
                // Запоминаем нативное разрешение хоста для FSR апскейла.
                // Используем первый (primary) дисплей.
                if let Some(primary) = displays.first() {
                    if primary.width > 0 && primary.height > 0 {
                        self.fsr_native_size = Some((primary.width as u32, primary.height as u32));
                    }
                }
                self.remote_displays = displays;
                self.remote_texture = None;
                self.pending_image = None;
                self.last_frame_rgba.clear();
                self.remote_size = [0, 0];
                if !self
                    .remote_displays
                    .iter()
                    .any(|display| display.index == self.selected_display)
                {
                    self.selected_display = self
                        .remote_displays
                        .first()
                        .map(|display| display.index)
                        .unwrap_or_default();
                }
                if self.connected && self.selected_display != previous_selected_display {
                    if let Some(display) = self
                        .remote_displays
                        .iter()
                        .find(|display| display.index == self.selected_display)
                        .cloned()
                    {
                        self.send_command(SessionCommand::SetDisplay(display));
                    }
                }
                let display_list = self
                    .remote_displays
                    .iter()
                    .map(display_label)
                    .collect::<Vec<_>>()
                    .join(" | ");
                self.log(format!(
                    "Displays detected: {}{}",
                    self.remote_displays.len(),
                    if display_list.is_empty() {
                        String::new()
                    } else {
                        format!(" [{display_list}]")
                    }
                ));
            }
            SessionEvent::CursorData {
                id,
                hotx,
                hoty,
                width,
                height,
                rgba,
            } => {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [width as usize, height as usize],
                    &rgba,
                );
                self.cursor_cache.insert(
                    id,
                    CursorCacheEntry {
                        image: Some(image.clone()),
                        texture: None,
                        hotx,
                        hoty,
                    },
                );
                self.pending_cursor = Some((id, image, hotx, hoty));
            }
            SessionEvent::CursorId { id } => {
                if let Some(entry) = self.cursor_cache.get(&id) {
                    if let Some(texture) = entry.texture.clone() {
                        self.cursor_texture = Some((texture, entry.hotx, entry.hoty));
                    } else if let Some(image) = entry.image.clone() {
                        self.pending_cursor = Some((id, image, entry.hotx, entry.hoty));
                    }
                }
            }
            SessionEvent::CursorPosition { x, y } => {
                // When the local pointer is actively over the remote screen we
                // already update cursor_pos from the local mouse position in
                // real-time (zero latency). Applying the host's echo of that
                // same position (with RTT latency) would cause the cursor to
                // stutter. Only accept remote position when we're not hovering.
                if !self.remote_input_focused {
                    self.cursor_pos = Some(egui::pos2(x as f32, y as f32));
                }
            }
            SessionEvent::Latency(ms) => {
                self.latency_ms = Some(ms);
            }
            SessionEvent::ShellOutput(text) => {
                self.shell_output.push_str(&text);
                self.trim_shell_output();
                if self.terminal_auto_pending {
                    self.terminal_auto_request_at = Some(Instant::now());
                }
            }
            SessionEvent::ShellClosed => {
                self.shell_output.push_str("\r\n[console closed]\r\n");
            }
            SessionEvent::ShellError(err) => {
                self.shell_output
                    .push_str(&format!("\r\n[console error] {err}\r\n"));
            }
            SessionEvent::ClipboardText(text) => {
                if !self.config.security.allow_clipboard {
                    self.clipboard_status = Some("буфер отключен в настройках".to_owned());
                    self.log("Clipboard ignored: disabled by local security policy".to_owned());
                } else {
                    let chars = text.chars().count();
                    match write_local_clipboard_text(&text) {
                        Ok(()) => {
                            self.clipboard_status = Some(format!("буфер получен: {chars} симв."));
                            self.log(format!("Clipboard received from remote: {chars} chars"));
                        }
                        Err(err) => {
                            self.clipboard_status = Some("буфер принять не удалось".to_owned());
                            self.log(format!("Clipboard write failed: {err}"));
                        }
                    }
                }
            }
            SessionEvent::Info(message) => self.log(message),
            SessionEvent::EvrtStatus {
                active,
                host_addr,
                port,
            } => {
                self.evrt_active = active;
                self.evrt_host_addr = if active {
                    format!("{host_addr}:{port}")
                } else {
                    String::new()
                };
                if active {
                    self.stream_health = format!("EVRT UDP прямой → {host_addr}:{port}");
                    self.log(format!("✓ EVRT прямое UDP соединение: {host_addr}:{port}"));
                } else {
                    self.evrt_pressure = "normal".to_owned();
                }
            }
            SessionEvent::EvrtMetrics {
                pressure,
                arrival_delta_ms,
                assembly_delay_ms,
                decode_delta_ms,
                jitter_ms,
                fps,
                packets_received,
                frames_assembled,
                reassembly_drops,
                queue_drops,
                ..
            } => {
                self.evrt_pressure = pressure;
                self.evrt_arrival_delta_ms = arrival_delta_ms;
                self.evrt_assembly_delay_ms = assembly_delay_ms;
                self.evrt_decode_delta_ms = decode_delta_ms;
                self.evrt_jitter_ms = jitter_ms;
                self.evrt_fps = fps;
                self.evrt_packets_received = packets_received;
                self.evrt_frames_assembled = frames_assembled;
                self.evrt_reassembly_drops = reassembly_drops;
                self.evrt_queue_drops = queue_drops;
            }
            SessionEvent::Failed(err) => {
                self.busy = false;
                self.connected = false;
                self.remote_viewer_open = false;
                self.remote_viewer_window_spawned = false;
                self.remote_fullscreen = false;
                self.session_tx = None;
                self.connection_state = ConnectionState::Failed(err.clone());
                self.remote_modifiers_down = RemoteModifierState::default();
                self.last_error = Some(err.clone());
                self.status = friendly_error(&err);
                self.log(format!("Error: {err}"));
            }
            SessionEvent::Closed => {
                self.busy = false;
                self.connected = false;
                self.remote_viewer_open = false;
                self.remote_viewer_window_spawned = false;
                self.remote_fullscreen = false;
                self.session_tx = None;
                self.remote_input_focused = false;
                self.remote_modifiers_down = RemoteModifierState::default();
                self.screenshot_pending = false;
                self.status = "Отключено".to_owned();
                self.log(self.status.clone());
            }
        }
    }

    fn send_command(&mut self, command: SessionCommand) {
        if let Some(tx) = &self.session_tx {
            let is_input = command_is_input(&command);
            if tx.send(command).is_err() {
                self.set_error("Session command channel is closed");
            } else if is_input {
                self.input_events_sent += 1;
            }
        }
    }

    fn disconnect_session(&mut self, status: &str) {
        self.release_remote_modifiers();
        if let Some(tx) = self.session_tx.take() {
            let _ = tx.send(SessionCommand::Close);
        }
        self.busy = false;
        self.connected = false;
        self.remote_viewer_open = false;
        self.remote_viewer_window_spawned = false;
        self.remote_fullscreen = false;
        self.remote_texture = None;
        self.pending_image = None;
        self.last_frame_rgba.clear();
        self.remote_input_focused = false;
        self.remote_modifiers_down = RemoteModifierState::default();
        self.screenshot_pending = false;
        self.last_mouse_pos = None;
        self.last_move_refresh_at = None;
        self.wheel_accum = egui::Vec2::ZERO;
        self.progress = 0;
        self.live_frame_count = 0;
        self.screenshot_frame_count = 0;
        self.last_frame_codec = "none".to_owned();
        self.frame_bytes = 0;
        self.frame_queue_ms = 0;
        self.frame_decode_ms = 0;
        self.frame_dropped = 0;
        self.fps_last_at = Instant::now();
        self.fps_last_count = 0;
        self.display_fps = 0.0;
        self.stream_input_fps = 0.0;
        self.stream_input_kbps = 0;
        self.last_stream_tune_at = None;
        self.last_video_metric_log_at = None;
        self.png_fallback_started_at = None;
        self.last_live_frame_at = None;
        self.stream_health = "отключено".to_owned();
        self.screenshot_status = None;
        self.log_status = None;
        self.report_status = None;
        self.cursor_cache.clear();
        self.pending_cursor = None;
        self.cursor_texture = None;
        self.cursor_pos = None;
        self.latency_ms = None;
        self.status = status.to_owned();
        self.log(status.to_owned());
    }

    fn visible_status(&self) -> String {
        if self.busy {
            return "Подключение...".to_owned();
        }
        if let Some(error) = &self.last_error {
            return friendly_error(error);
        }
        if self.connected {
            return "Подключено".to_owned();
        }
        self.status.clone()
    }

    fn update_render_fps(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.fps_last_at);
        if elapsed >= Duration::from_millis(750) {
            let received = self.live_frame_count + self.screenshot_frame_count;
            let frames = received.saturating_sub(self.fps_last_count);
            self.display_fps = frames as f32 / elapsed.as_secs_f32();
            self.fps_last_at = now;
            self.fps_last_count = received;
        }
    }

    fn auto_tune_stream(&mut self) {
        let cooldown_ready = self
            .last_stream_tune_at
            .map(|instant| instant.elapsed() >= Duration::from_secs(3))
            .unwrap_or(true);

        if self.last_frame_codec == "PNG" {
            let fallback_ms = self
                .png_fallback_started_at
                .map(|instant| instant.elapsed().as_millis() as u64)
                .unwrap_or_default();
            self.stream_health = if fallback_ms >= 2_000 {
                "PNG fallback: запрашиваем live video".to_owned()
            } else {
                "PNG fallback".to_owned()
            };
            if fallback_ms >= 2_000 && cooldown_ready {
                self.last_stream_tune_at = Some(Instant::now());
                self.send_command(SessionCommand::RefreshVideo);
                self.log("Auto tune: PNG fallback persists; requested live video".to_owned());
            }
            return;
        }

        if self.last_frame_codec == "none" {
            self.stream_health = "ожидание кадра".to_owned();
            return;
        }

        let stale_queue_lag = self.frame_queue_ms >= 450;
        let dropped_backlog_lag = self.frame_dropped >= 12 && self.frame_queue_ms >= 180;
        let queue_lag = stale_queue_lag || dropped_backlog_lag;
        let decode_lag = self.frame_decode_ms >= 60;
        if queue_lag || decode_lag {
            self.stream_health = if queue_lag {
                "очередь кадров: догоняем поток".to_owned()
            } else {
                "декодер перегружен".to_owned()
            };
            if cooldown_ready {
                self.last_stream_tune_at = Some(Instant::now());
                let interactive_floor = if self.config.display.target_fps >= 45 {
                    30
                } else {
                    self.config
                        .display
                        .min_fps
                        .clamp(5, self.config.display.target_fps) as i32
                };
                let next_fps = if decode_lag {
                    match self.video_fps {
                        fps if fps > 30 => 30,
                        fps if fps > 20 => 20,
                        fps if fps > 15 => 15,
                        fps => fps,
                    }
                } else if self.video_fps > interactive_floor {
                    interactive_floor
                } else {
                    self.video_fps
                };
                if next_fps != self.video_fps {
                    self.video_fps = next_fps;
                    self.send_command(SessionCommand::SetVideoFps { fps: next_fps });
                    self.log(format!("Auto tune: video fps lowered to {next_fps}"));
                } else {
                    self.send_command(SessionCommand::RefreshVideo);
                    self.log("Auto tune: requested fresh live video stream".to_owned());
                }
            }
            return;
        }

        self.stream_health = "live поток стабилен".to_owned();
    }

    fn save_ui_config(&mut self) {
        self.config.ui.last_remote_id = self.remote_id.clone();
        remember_remote_id(&mut self.config.ui.recent_remote_ids, &self.remote_id);
        remember_history(&mut self.config.ui.history, &self.remote_id);
        self.config.ui.auto_refresh = self.auto_refresh;
        self.config.ui.refresh_millis = self.refresh_millis;
        self.config.ui.fit_to_window = self.fit_to_window;
        self.config.ui.coordinate_mode = self.coordinate_mode;
        self.config.save();
    }

    fn set_error(&mut self, message: &str) {
        self.last_error = Some(message.to_owned());
        self.status = friendly_error(message);
        self.connection_state = ConnectionState::Failed(message.to_owned());
        self.log(format!("Error: {message}"));
    }

    fn log(&mut self, message: String) {
        let line = format!("{}  {message}", unix_timestamp_secs());
        self.session_log.push(line.clone());
        if self.session_log.len() > MAX_SESSION_LOG_LINES {
            let trim = SESSION_LOG_TRIM_LINES.min(self.session_log.len());
            self.session_log.drain(..trim);
        }
        self.events.push(line);
        if self.events.len() > 80 {
            self.events.remove(0);
        }
    }
}

impl eframe::App for EvertyDeskApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.update_egui(ui.ctx());
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shutdown();
        arm_shutdown_watchdog();
    }
}

fn arm_shutdown_watchdog() {
    thread::Builder::new()
        .name("shutdown-watchdog".into())
        .spawn(|| {
            thread::sleep(Duration::from_millis(2000));
            // ExitProcess runs NVIDIA DLL destructors which deadlock with
            // in-flight GPU work. TerminateProcess skips all cleanup and
            // exits instantly — the OS reclaims all GPU resources.
            force_terminate();
        })
        .ok();
}

fn force_terminate() -> ! {
    #[cfg(windows)]
    unsafe {
        windows::Win32::System::Threading::TerminateProcess(
            windows::Win32::System::Threading::GetCurrentProcess(),
            0,
        );
        unreachable!()
    }
    #[cfg(not(windows))]
    std::process::exit(0);
}

/// Background thread: if the main window becomes unresponsive (render thread
/// stuck in GPU driver — common with OBS hooks or NVIDIA D3D deadlocks),
/// IsHungAppWindow returns true and we terminate the process forcefully.
/// This is the only reliable exit path when the Win32 message pump is blocked.
fn start_hung_window_guardian() {
    #[cfg(windows)]
    thread::Builder::new()
        .name("hung-guardian".into())
        .spawn(|| {
            use windows::core::s;
            use windows::Win32::Foundation::{LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{
                FindWindowA, SendMessageTimeoutA, ShowWindow, SMTO_ABORTIFHUNG, SW_HIDE, WM_NULL,
            };

            // Wait for the window and GPU backend to fully initialize.
            thread::sleep(Duration::from_secs(10));
            let mut misses = 0_u32;

            loop {
                thread::sleep(Duration::from_secs(2));
                unsafe {
                    let hwnd = FindWindowA(None, s!("EvertyDesk Lite"));
                    if hwnd.0 == 0 {
                        misses = 0;
                        continue;
                    }
                    // WM_NULL with a long timeout. A single miss can happen
                    // during GPU/DXGI teardown, so require several misses.
                    let mut result = 0usize;
                    let ok = SendMessageTimeoutA(
                        hwnd,
                        WM_NULL,
                        WPARAM(0),
                        LPARAM(0),
                        SMTO_ABORTIFHUNG,
                        8000,
                        Some(&mut result),
                    );
                    if ok.0 != 0 {
                        misses = 0;
                        continue;
                    }
                    misses += 1;
                    if misses >= 3 {
                        eprintln!("[guardian] Render thread stuck — hiding window");
                        ShowWindow(hwnd, SW_HIDE);
                        thread::sleep(Duration::from_millis(200));
                        eprintln!("[guardian] Terminating process");
                        force_terminate();
                    }
                }
            }
        })
        .ok();

    #[cfg(not(windows))]
    let _ = ();
}

impl EvertyDeskApp {
    #[allow(deprecated)]
    fn update_egui(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) {
            self.shutdown();
            // Start the watchdog NOW, before eframe begins WGPU/D3D teardown.
            // on_exit() may never be reached if the GPU driver stalls during
            // swap-chain destruction (common with NVIDIA on Windows).
            // The watchdog kills the process from a separate thread after 3 s,
            // which first terminates the stuck GPU thread, then runs DLL cleanup
            // without contention — no deadlock.
            arm_shutdown_watchdog();
        }

        self.poll_worker();
        self.poll_terminal_ai();
        self.maybe_request_terminal_auto_ai();
        self.poll_host_service();
        if let Some(image) = self.pending_image.take() {
            if let Some(texture) = self.remote_texture.as_mut() {
                texture.set(image, remote_texture_options());
            } else {
                self.remote_texture =
                    Some(ctx.load_texture("remote-screen", image, remote_texture_options()));
            }
            ctx.request_repaint();
        }
        if let Some((id, image, hotx, hoty)) = self.pending_cursor.take() {
            let texture = ctx.load_texture(
                format!("remote-cursor-{id}"),
                image,
                egui::TextureOptions::NEAREST,
            );
            self.cursor_texture = Some((texture.clone(), hotx, hoty));
            if let Some(entry) = self.cursor_cache.get_mut(&id) {
                entry.texture = Some(texture);
                entry.image = None;
            }
        }
        if self.busy
            || self.connected
            || self.host_check_busy
            || self.terminal_ai_rx.is_some()
            || self.host_pending_peer.is_some()
            || self.host_state.is_online()
        {
            let repaint_ms = if self.connected
                && self
                    .last_live_frame_at
                    .map(|t| t.elapsed() < Duration::from_secs(3))
                    .unwrap_or(false)
            {
                16 // ~60fps poll when stream is active.
            } else if self.connected
                || self.busy
                || self.host_check_busy
                || self.terminal_ai_rx.is_some()
                || self.host_pending_peer.is_some()
            {
                33 // ~30fps while an interactive operation is pending.
            } else {
                1000 // Idle registered host: do not burn CPU on weak server GPUs.
            };
            ctx.request_repaint_after(Duration::from_millis(repaint_ms));
        }

        let software_backend = egui_software_backend_active();
        if software_backend && self.connected && self.remote_viewer_open {
            self.remote_viewer_inline(ctx);
            if self.show_settings {
                self.settings_window(ctx);
            }
            return;
        }

        if self.connected && self.remote_viewer_open {
            self.remote_viewer_window(ctx);
        }
        if self.connected && self.shell_window_open {
            self.shell_window(ctx);
        }
        if self.host_pending_peer.is_some() {
            self.incoming_approval_window(ctx);
        }

        let screen_rect = ctx.content_rect();
        ctx.layer_painter(egui::LayerId::background()).rect_filled(
            screen_rect,
            egui::CornerRadius::ZERO,
            egui::Color32::from_rgb(0xFB, 0xFC, 0xFE),
        );

        // ── Left sidebar: logo · navigation · settings ───────────────────────
        egui::Panel::left("everty_sidebar")
            .resizable(false)
            .exact_size(220.0)
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(0xF7, 0xF8, 0xFA))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgb(0xEA, 0xEC, 0xF0),
                    ))
                    .corner_radius(egui::CornerRadius::ZERO)
                    .inner_margin(egui::Margin::symmetric(18, 20))
                    .outer_margin(egui::Margin::ZERO),
            )
            .show(ctx, |ui| self.sidebar(ui));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(0xFB, 0xFC, 0xFE))
                    .inner_margin(egui::Margin {
                        left: 26,
                        right: 24,
                        top: 28,
                        bottom: 24,
                    }),
            )
            .show(ctx, |ui| match self.mode {
                AppMode::Connect => self.connect_ui(ui),
                AppMode::Host => self.host_ui(ui),
                AppMode::History => self.history_ui(ui),
                AppMode::Contacts => self.contacts_ui(ui),
                AppMode::Settings => self.settings_ui(ui),
            });
        if self.mode == AppMode::Connect
            && !self.busy
            && !self.connected
            && ctx.input(|input| input.key_pressed(egui::Key::Enter))
        {
            self.connect();
        }
    }

    fn shutdown(&mut self) {
        if let Some(svc) = self.host_service.take() {
            svc.stop();
        }
        if self.connected || self.busy {
            self.disconnect_session("Application closed");
        }
    }

    fn text(&self, ru: &'static str, en: &'static str) -> &'static str {
        tr(self.ui_lang, ru, en)
    }

    fn host_state_text(&self) -> &'static str {
        match (&self.host_state, self.ui_lang) {
            (HostState::Idle, UiLang::Ru) => "Остановлен",
            (HostState::Idle, UiLang::En) => "Stopped",
            (HostState::Connecting, UiLang::Ru) => "Подключение...",
            (HostState::Connecting, UiLang::En) => "Connecting...",
            (HostState::Ready, UiLang::Ru) => "Готов к подключению",
            (HostState::Ready, UiLang::En) => "Ready to connect",
            (HostState::Accepting(_), UiLang::Ru) => "Сессия активна",
            (HostState::Accepting(_), UiLang::En) => "Session active",
            (HostState::Error(_), UiLang::Ru) => "Ошибка",
            (HostState::Error(_), UiLang::En) => "Error",
        }
    }

    fn ensure_app_logo_texture(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        if self.app_logo_texture.is_none() {
            let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/edesk_lite_logo.png"));
            if let Ok(img) = image::load_from_memory(bytes) {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let image = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                self.app_logo_texture =
                    Some(ctx.load_texture("evertydesk-logo", image, egui::TextureOptions::LINEAR));
            }
        }
        self.app_logo_texture.as_ref()
    }

    fn connect_ui(&mut self, ui: &mut egui::Ui) {
        if commercial_ui_enabled() {
            self.connect_ui_commercial(ui);
            return;
        }
        ui.add_space(6.0);
        ui.label("ID удаленного ПК");
        let remote_id_response = ui.add_enabled(
            !self.connected && !self.busy,
            egui::TextEdit::singleline(&mut self.remote_id).desired_width(f32::INFINITY),
        );
        ui.add_space(8.0);
        ui.label("Пароль");
        let password_response = ui.add_enabled(
            !self.connected && !self.busy,
            egui::TextEdit::singleline(&mut self.password)
                .password(!self.show_password)
                .desired_width(f32::INFINITY),
        );
        ui.small(
            "Можно оставить пустым, если удаленный RustDesk разрешает подтверждение без пароля.",
        );
        ui.checkbox(&mut self.show_password, "Показать пароль");
        if remote_id_response.changed() || password_response.changed() {
            self.last_error = None;
            if !self.connected && !self.busy {
                self.status = "Готово".to_owned();
                self.progress = 0;
            }
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.busy && !self.connected,
                    egui::Button::new("Подключиться").min_size(egui::vec2(150.0, 32.0)),
                )
                .clicked()
            {
                self.connect();
            }
            if ui
                .add_enabled(
                    self.connected || self.busy,
                    egui::Button::new("Отключиться").min_size(egui::vec2(140.0, 32.0)),
                )
                .clicked()
            {
                self.disconnect_session("Отключено");
            }
            if ui
                .add_enabled(
                    self.connected && !self.remote_viewer_open,
                    egui::Button::new("Экран").min_size(egui::vec2(90.0, 32.0)),
                )
                .clicked()
            {
                self.remote_viewer_open = true;
                self.remote_viewer_window_spawned = false;
                self.status = "Экран открыт".to_owned();
                self.send_command(SessionCommand::SetAutoRefresh {
                    enabled: self.auto_refresh,
                    millis: self.refresh_millis,
                });
                self.send_command(SessionCommand::Screenshot);
            }
        });
        ui.add_space(10.0);
        if self.progress > 0 || self.busy || self.connected {
            ui.add(
                egui::ProgressBar::new(self.progress as f32 / 100.0)
                    .text(format!("{}%", self.progress)),
            );
            ui.add_space(6.0);
        }
        if self.last_error.is_some() {
            ui.colored_label(
                egui::Color32::from_rgb(240, 120, 120),
                self.visible_status(),
            );
        } else {
            ui.label(self.visible_status());
        }
        if self.connected && !self.remote_viewer_open {
            ui.label("Окно экрана закрыто. Нажмите Экран, чтобы открыть его снова.");
        }

        if !self.events.is_empty() {
            ui.add_space(8.0);
            ui.separator();
            ui.collapsing("Лог событий", |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button("📋 Копировать лог")
                        .on_hover_text("Скопировать весь лог в буфер обмена")
                        .clicked()
                    {
                        let all = self.events.join("\n");
                        ui.ctx().copy_text(all);
                        self.log_status = Some("Лог скопирован в буфер обмена".to_owned());
                    }
                    if ui
                        .button("🗑 Очистить")
                        .on_hover_text("Очистить лог")
                        .clicked()
                    {
                        self.events.clear();
                    }
                });
                if let Some(status) = &self.log_status {
                    ui.label(
                        egui::RichText::new(status)
                            .size(10.5)
                            .color(egui::Color32::from_rgb(0x43, 0xA8, 0x47)),
                    );
                }
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for event in self.events.iter().rev().take(30) {
                            ui.label(egui::RichText::new(event).monospace().size(10.5));
                        }
                    });
            });
        }
    }

    // ── Host UI ───────────────────────────────────────────────────────────────

    /// Left sidebar: logo, app name, version, navigation, and Settings pinned
    /// to the bottom (per UI spec v1 — Arc/Linear-style rail).
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.set_width(ui.available_width());
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
            let p = ui.painter();
            p.rect_filled(
                rect,
                egui::CornerRadius::same(12),
                egui::Color32::from_rgb(0xFC, 0xFD, 0xFF),
            );
            p.rect_stroke(
                rect,
                egui::CornerRadius::same(12),
                egui::Stroke::new(1.0, egui::Color32::from_rgb(0xE5, 0xE8, 0xEF)),
                egui::StrokeKind::Inside,
            );
            if let Some(texture) = self.ensure_app_logo_texture(ui.ctx()) {
                let image_rect = rect.shrink(6.0);
                p.image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                p.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "E",
                    egui::FontId::proportional(26.0),
                    egui::Color32::from_rgb(0x16, 0x18, 0x20),
                );
            }
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.set_max_width(118.0);
                ui.add_space(3.0);
                ui.label(
                    egui::RichText::new(APP_NAME)
                        .size(14.0)
                        .strong()
                        .color(egui::Color32::from_rgb(0x17, 0x1A, 0x22)),
                );
                ui.label(
                    egui::RichText::new(format!("v{APP_VERSION}"))
                        .size(12.0)
                        .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                );
            });
        });

        ui.add_space(18.0);
        let connect_label = self.text("Подключиться", "Connect");
        let host_label = self.text("Этот компьютер", "This computer");
        let history_label = self.text("История", "History");
        let contacts_label = self.text("Контакты", "Contacts");
        self.nav_item(ui, AppMode::Connect, connect_label, "connect");
        ui.add_space(8.0);
        self.nav_item(ui, AppMode::Host, host_label, "monitor");
        ui.add_space(8.0);
        self.nav_item(ui, AppMode::History, history_label, "history");
        ui.add_space(8.0);
        self.nav_item(ui, AppMode::Contacts, contacts_label, "contacts");

        let spacer = (ui.available_height() - 50.0).max(10.0);
        ui.add_space(spacer);
        let settings_label = self.text("Настройки", "Settings");
        if nav_icon_button(
            ui,
            settings_label,
            "settings",
            self.mode == AppMode::Settings,
            true,
        )
        .clicked()
        {
            if self.settings_draft.is_none() {
                self.settings_draft = Some(self.config.clone());
            }
            self.mode = AppMode::Settings;
        }
    }

    fn nav_item(&mut self, ui: &mut egui::Ui, mode: AppMode, label: &str, icon: &str) {
        let active = self.mode == mode;
        if nav_icon_button(ui, label, icon, active, false).clicked() {
            self.mode = mode;
        }
    }

    #[allow(dead_code)]
    fn status_indicator(&self, ui: &mut egui::Ui) {
        use egui::Color32;
        let (label, base, pulse) = if self.last_error.is_some() {
            ("Error", Color32::from_rgb(0xFF, 0x55, 0x55), false)
        } else if self.connected {
            ("Connected", Color32::WHITE, false)
        } else if self.busy {
            ("Connecting", Color32::WHITE, true)
        } else {
            ("Ready", Color32::WHITE, false)
        };
        ui.label(
            egui::RichText::new(label)
                .size(14.0)
                .color(Color32::from_rgb(0xC8, 0xC8, 0xC8)),
        );
        ui.add_space(8.0);
        let dot = if pulse {
            let t = ui.input(|i| i.time);
            let a = 0.5 + 0.5 * (t * std::f64::consts::TAU / 0.8).sin();
            let alpha = (110.0 + 145.0 * a) as u8;
            ui.ctx().request_repaint();
            Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), alpha)
        } else {
            base
        };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, dot);
    }

    /// Right-hand "This computer" card: your ID, password, and host status.
    fn this_computer_card(&mut self, ui: &mut egui::Ui, _min_h: f32) {
        let gray = egui::Color32::from_rgb(0x50, 0x58, 0x68);
        card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                egui::RichText::new(self.text("Этот компьютер", "This computer"))
                    .size(18.0)
                    .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
            );
            ui.add_space(16.0);

            ui.label(
                egui::RichText::new(self.text("Ваш ID", "Your ID"))
                    .size(12.0)
                    .color(gray),
            );
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format_peer_id(&self.config.local_id))
                        .size(26.0)
                        .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, "copy")
                        .on_hover_text(self.text("Скопировать ID", "Copy ID"))
                        .clicked()
                    {
                        ui.ctx().copy_text(self.config.local_id.clone());
                    }
                });
            });

            ui.add_space(12.0);
            let show_host_label = self.text("показать", "show");
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(self.text("Пароль", "Password"))
                        .size(12.0)
                        .color(gray),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.show_host_password, show_host_label);
                });
            });
            ui.horizontal(|ui| {
                let pw_text = if self.show_host_password {
                    self.config.local_password.clone()
                } else {
                    "•".repeat(self.config.local_password.len().max(8))
                };
                ui.label(
                    egui::RichText::new(pw_text)
                        .size(22.0)
                        .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, "copy")
                        .on_hover_text(self.text("Скопировать пароль", "Copy password"))
                        .clicked()
                    {
                        ui.ctx().copy_text(self.config.local_password.clone());
                    }
                    if icon_button(ui, "refresh")
                        .on_hover_text(self.text("Новый пароль", "New password"))
                        .clicked()
                    {
                        self.config.local_password = generate_numeric_token(6);
                        self.config.save();
                    }
                });
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(12.0);

            ui.label(
                egui::RichText::new(self.text("Статус", "Status"))
                    .size(12.0)
                    .color(gray),
            );
            ui.add_space(6.0);
            let online = self.host_state.is_online();
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                let base = if online {
                    egui::Color32::from_rgb(0x12, 0xC9, 0x72)
                } else {
                    egui::Color32::from_rgb(0x12, 0xC9, 0x72)
                };
                ui.painter().circle_filled(rect.center(), 5.0, base);
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new(if online {
                        self.text("В сети", "Online")
                    } else {
                        self.text("Готов к подключению", "Ready to connect")
                    })
                    .size(16.0)
                    .color(egui::Color32::from_rgb(0x20, 0x24, 0x2D)),
                );
            });
        });
    }

    /// HERO banner for the connection page: clean green header, product title,
    /// subtitle, and a compact status label.
    fn connect_hero(&mut self, ui: &mut egui::Ui) {
        let lang = self.ui_lang;
        let online = self.host_state.is_online();
        let host_label = self.host_state.label().to_owned();

        let banner_h = 78.0;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), banner_h),
            egui::Sense::hover(),
        );

        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(14),
            egui::Color32::from_rgb(0x12, 0x8C, 0x55),
        );

        let pad = 22.0;
        let text_x = rect.left() + pad;
        let title = tr(lang, "EvertyDesk Lite", "EvertyDesk Lite");
        ui.painter().text(
            egui::pos2(text_x, rect.center().y - 16.0),
            egui::Align2::LEFT_TOP,
            title,
            egui::FontId::proportional(24.0),
            egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
        );
        let subtitle = tr(
            lang,
            "Быстрый защищённый удалённый доступ",
            "Fast secure remote access",
        );
        ui.painter().text(
            egui::pos2(text_x, rect.center().y + 12.0),
            egui::Align2::LEFT_TOP,
            subtitle,
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgb(0xA7, 0xC9, 0xBE),
        );

        let dot_color = if online {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_rgb(0xFF, 0xD1, 0x6A)
        };
        let cap_label = if online {
            tr(lang, "В сети", "Online")
        } else {
            &host_label
        };
        let cap_galley = ui.painter().layout_no_wrap(
            cap_label.to_owned(),
            egui::FontId::proportional(12.5),
            egui::Color32::from_rgb(0xE8, 0xF5, 0xEF),
        );
        let cap_right = rect.right() - pad;
        let dot_x = cap_right - cap_galley.size().x - 13.0;
        ui.painter()
            .circle_filled(egui::pos2(dot_x, rect.center().y), 4.5, dot_color);
        ui.painter().galley(
            egui::pos2(dot_x + 12.0, rect.center().y - cap_galley.size().y / 2.0),
            cap_galley,
            egui::Color32::PLACEHOLDER,
        );
    }

    /// Чипы недавних подключений — клик подставляет ID в поле.
    fn recent_chips(&mut self, ui: &mut egui::Ui) {
        let recents: Vec<String> = self
            .config
            .ui
            .recent_remote_ids
            .iter()
            .filter(|id| !id.is_empty())
            .take(5)
            .cloned()
            .collect();
        if recents.is_empty() {
            return;
        }
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(self.text("Недавние", "Recent"))
                .size(12.0)
                .color(egui::Color32::from_rgb(0x8A, 0x93, 0xA3)),
        );
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for id in recents {
                let shown = format_peer_id(&id);
                if recent_chip(ui, &shown).clicked() {
                    self.remote_id = id.clone();
                    self.last_error = None;
                }
                ui.add_space(6.0);
            }
        });
    }

    fn connect_ui_commercial(&mut self, ui: &mut egui::Ui) {
        ui.add_space(0.0);
        // ── HERO-баннер: градиент + логотип + название + статус ───────────────
        self.connect_hero(ui);
        ui.add_space(14.0);

        workspace_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.add_space(2.0);
            let two_gap = 18.0;
            let right_width = 340.0_f32.min((ui.available_width() - two_gap) * 0.42);
            let two_left = (ui.available_width() - two_gap - right_width).clamp(320.0, 430.0);
            // ui.vertical columns auto-size to their content height, so the
            // workspace wraps tightly instead of stretching down the window.
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_min_width(two_left);
                    ui.set_max_width(two_left);
                    card_frame().show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.set_min_height(196.0);
                        ui.horizontal(|ui| {
                            let segment_width = (ui.available_width() - 8.0) / 2.0;
                            if mode_segment_button(
                                ui,
                                self.text("Экран", "Screen"),
                                "monitor",
                                self.connect_kind == ConnectKind::Screen,
                                segment_width,
                            )
                            .clicked()
                            {
                                self.connect_kind = ConnectKind::Screen;
                            }
                            ui.add_space(8.0);
                            if mode_segment_button(
                                ui,
                                self.text("Консоль", "Console"),
                                "console",
                                self.connect_kind == ConnectKind::Shell,
                                segment_width,
                            )
                            .clicked()
                            {
                                self.connect_kind = ConnectKind::Shell;
                            }
                        });
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(self.text("ID партнера", "Partner ID"))
                                .size(14.0)
                                .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
                        );
                        let remote_id_response = compact_text_input(
                            ui,
                            &mut self.remote_id,
                            "123 456 789",
                            false,
                            !self.connected && !self.busy,
                            Some(22.0),
                        );
                        ui.add_space(10.0);
                        let password_label = self.text("Пароль", "Password");
                        let show_label = self.text("показать", "show");
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(password_label)
                                    .size(14.0)
                                    .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.checkbox(&mut self.show_password, show_label);
                                },
                            );
                        });
                        let password_hint = self.text(
                            "Необязательно, если доступ подтверждают вручную",
                            "Optional for remote approval",
                        );
                        let password_response = compact_text_input(
                            ui,
                            &mut self.password,
                            password_hint,
                            !self.show_password,
                            !self.connected && !self.busy,
                            Some(16.0),
                        );
                        if remote_id_response.changed() || password_response.changed() {
                            self.last_error = None;
                            if !self.connected && !self.busy {
                                self.status = self.text("Готово", "Ready").to_owned();
                                self.progress = 0;
                            }
                        }

                        ui.add_space(14.0);
                        let connect_label = if self.busy {
                            self.text("Подключение...", "Connecting...")
                        } else {
                            self.text("Подключиться", "Connect")
                        };
                        if ui
                            .add_enabled_ui(!self.busy && !self.connected, |ui| {
                                primary_connect_button(
                                    ui,
                                    connect_label,
                                    if self.connect_kind == ConnectKind::Shell {
                                        "console"
                                    } else {
                                        "connect"
                                    },
                                )
                            })
                            .inner
                            .clicked()
                        {
                            self.connect();
                        }

                        // ── Чипы недавних подключений (быстрый коннект) ───────
                        self.recent_chips(ui);
                    });
                });
                ui.add_space(two_gap);
                ui.vertical(|ui| {
                    ui.set_min_width(right_width);
                    ui.set_max_width(right_width);
                    self.this_computer_card(ui, 0.0);
                });
            });
        }); // ── close workspace container ───────────────────────────────────

        ui.add_space(6.0);
        if self.progress > 0 || self.busy || self.connected {
            ui.add(
                egui::ProgressBar::new(self.progress as f32 / 100.0)
                    .desired_width(f32::INFINITY)
                    .text(format!("{}%", self.progress)),
            );
            ui.add_space(4.0);
        }
        if self.last_error.is_some() || self.busy || self.connected {
            let status_color = if self.last_error.is_some() {
                egui::Color32::from_rgb(238, 95, 95)
            } else if self.connected {
                egui::Color32::from_rgb(66, 190, 112)
            } else {
                egui::Color32::from_rgb(100, 112, 128)
            };
            ui.label(
                egui::RichText::new(self.visible_status())
                    .size(12.0)
                    .color(status_color),
            );
        }

        if self.config.ui.show_connection_details {
            ui.add_space(16.0);
            card_frame().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                egui::CollapsingHeader::new(self.text(
                    "Детали подключения - нажмите, чтобы раскрыть",
                    "Connection details - click to expand",
                ))
                .default_open(false)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .button(self.text("Скопировать лог", "Copy log"))
                            .clicked()
                        {
                            ui.ctx().copy_text(self.events.join("\n"));
                            self.log_status = Some("Log copied".to_owned());
                        }
                        if ui.button(self.text("Очистить", "Clear")).clicked() {
                            self.events.clear();
                        }
                    });
                    egui::ScrollArea::vertical()
                        .max_height(130.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for event in self.events.iter().rev().take(30) {
                                ui.label(
                                    egui::RichText::new(event)
                                        .monospace()
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(150, 158, 168)),
                                );
                            }
                        });
                });
            });
        }
    }

    fn host_ui(&mut self, ui: &mut egui::Ui) {
        if commercial_ui_enabled() {
            self.host_ui_commercial(ui);
            return;
        }
        ui.add_space(8.0);

        // ── ID + Password block ───────────────────────────────────────────────
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                // "Ваш ID"
                ui.label(
                    egui::RichText::new("Ваш ID")
                        .size(11.5)
                        .color(egui::Color32::GRAY),
                );
                ui.horizontal(|ui| {
                    // Large monospace ID — like RustDesk
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format_peer_id(&self.config.local_id))
                                .monospace()
                                .size(26.0)
                                .strong(),
                        )
                        .truncate(),
                    );
                    if ui.button("⎘").on_hover_text("Копировать ID").clicked() {
                        ui.ctx().copy_text(self.config.local_id.clone());
                    }
                });

                ui.add_space(8.0);

                // Password
                ui.label(
                    egui::RichText::new("Пароль")
                        .size(11.5)
                        .color(egui::Color32::GRAY),
                );
                ui.horizontal(|ui| {
                    let pw_text = if self.show_host_password {
                        self.config.local_password.clone()
                    } else {
                        "•".repeat(self.config.local_password.len())
                    };
                    ui.add(
                        egui::Label::new(egui::RichText::new(pw_text).monospace().size(20.0))
                            .truncate(),
                    );
                    if ui.button("⎘").on_hover_text("Копировать пароль").clicked()
                    {
                        ui.ctx().copy_text(self.config.local_password.clone());
                    }
                    if ui.button("↺").on_hover_text("Новый пароль").clicked() {
                        self.config.local_password = generate_numeric_token(6);
                        self.config.save();
                    }
                    ui.checkbox(&mut self.show_host_password, "Показать");
                });
            });

        ui.add_space(10.0);

        // ── Status + Start/Stop ───────────────────────────────────────────────
        ui.horizontal(|ui| {
            // Coloured status indicator
            let (r, g, b) = self.host_state.color();
            let dot_color = egui::Color32::from_rgb(r, g, b);
            ui.colored_label(dot_color, "●");
            ui.label(self.host_state.label());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let service_running = self.host_service.is_some();
                if service_running {
                    if ui
                        .add(egui::Button::new("⏹ Остановить").min_size(egui::vec2(130.0, 28.0)))
                        .clicked()
                    {
                        self.stop_host_service();
                    }
                } else {
                    if ui
                        .add(egui::Button::new("▶ Запустить").min_size(egui::vec2(130.0, 28.0)))
                        .clicked()
                    {
                        self.start_host_service();
                    }
                }
            });
        });
        if let Some(video_status) = &self.host_video_status {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("Видео: {video_status}"))
                    .monospace()
                    .size(10.5)
                    .color(egui::Color32::from_rgb(120, 130, 145)),
            );
        }

        // Pending incoming connection
        if let Some(peer_id) = self.host_pending_peer.clone() {
            ui.add_space(8.0);
            egui::Frame::group(ui.style())
                .fill(egui::Color32::from_rgba_premultiplied(60, 110, 180, 40))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!("📡 Входящий запрос от: {peer_id}")).strong(),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new("✓ Принять")
                                    .min_size(egui::vec2(100.0, 28.0))
                                    .fill(egui::Color32::from_rgb(40, 160, 80)),
                            )
                            .clicked()
                        {
                            // TODO: full relay session acceptance
                            self.host_pending_peer = None;
                        }
                        if ui
                            .add(
                                egui::Button::new("✗ Отклонить")
                                    .min_size(egui::vec2(100.0, 28.0))
                                    .fill(egui::Color32::from_rgb(200, 60, 60)),
                            )
                            .clicked()
                        {
                            self.host_pending_peer = None;
                        }
                    });
                });
        }

        // Host log
        if !self.host_log.is_empty() {
            ui.add_space(8.0);
            ui.separator();
            ui.collapsing("Лог хост-сервиса", |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button("📋 Копировать лог")
                        .on_hover_text("Скопировать весь лог хоста в буфер обмена")
                        .clicked()
                    {
                        let all = self.host_log.join("\n");
                        ui.ctx().copy_text(all);
                        self.host_status = "Лог хоста скопирован в буфер обмена".to_owned();
                    }
                    if ui
                        .button("🗑 Очистить")
                        .on_hover_text("Очистить лог хоста")
                        .clicked()
                    {
                        self.host_log.clear();
                    }
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in self.host_log.iter().rev().take(25) {
                            ui.label(egui::RichText::new(line).monospace().size(10.5));
                        }
                    });
            });
        }
    }

    fn history_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new(self.text("История подключений", "Connection history"))
                .size(28.0)
                .strong()
                .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(self.text(
                "Последние ID сохраняются локально. Можно добавить заметку.",
                "Recent IDs are stored locally. You can add a note.",
            ))
            .size(13.0)
            .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
        );
        ui.add_space(16.0);

        if self.config.ui.history.is_empty() {
            card_frame().show(ui, |ui| {
                ui.label(self.text("История пока пустая.", "History is empty."));
            });
            return;
        }

        let mut connect_to: Option<String> = None;
        let mut remove_id: Option<String> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for entry in &mut self.config.ui.history {
                card_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(format_peer_id(&entry.remote_id))
                                    .size(22.0)
                                    .strong()
                                    .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}: {}",
                                    tr(self.ui_lang, "Подключений", "Connections"),
                                    entry.connect_count
                                ))
                                .size(12.0)
                                .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(tr(self.ui_lang, "Удалить", "Remove")).clicked() {
                                remove_id = Some(entry.remote_id.clone());
                            }
                            if ui
                                .button(tr(self.ui_lang, "Подключиться", "Connect"))
                                .clicked()
                            {
                                connect_to = Some(entry.remote_id.clone());
                            }
                        });
                    });
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(tr(self.ui_lang, "Заметка", "Note"))
                            .size(12.0)
                            .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
                    );
                    ui.add_sized(
                        egui::vec2(ui.available_width(), 40.0),
                        egui::TextEdit::singleline(&mut entry.note).hint_text(tr(
                            self.ui_lang,
                            "Например: бухгалтерия, ноутбук Ивана...",
                            "Example: accounting, Ivan's laptop...",
                        )),
                    );
                });
                ui.add_space(8.0);
            }
        });

        if let Some(id) = remove_id {
            self.config.ui.history.retain(|entry| entry.remote_id != id);
            self.config.save();
        } else {
            self.config.save();
        }
        if let Some(id) = connect_to {
            self.remote_id = id;
            self.mode = AppMode::Connect;
        }
    }

    fn incoming_approval_window(&mut self, ctx: &egui::Context) {
        let Some(peer_id) = self.host_pending_peer.clone() else {
            return;
        };
        egui::Window::new(self.text("Входящее подключение", "Incoming connection"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.label(
                    egui::RichText::new(self.text(
                        "Удаленный пользователь хочет подключиться без пароля.",
                        "A remote user wants to connect without a password.",
                    ))
                    .size(15.0)
                    .color(egui::Color32::from_rgb(0x20, 0x24, 0x2D)),
                );
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(format!("ID: {}", format_peer_id(&peer_id)))
                        .size(22.0)
                        .strong()
                        .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(self.text("Разрешить", "Allow"))
                                .min_size(egui::vec2(132.0, 40.0))
                                .fill(egui::Color32::from_rgb(0x12, 0xC9, 0x72)),
                        )
                        .clicked()
                    {
                        if let Some(svc) = &self.host_service {
                            svc.approve_incoming(&peer_id, true);
                        }
                        self.host_pending_peer = None;
                    }
                    if ui
                        .add(
                            egui::Button::new(self.text("Отклонить", "Reject"))
                                .min_size(egui::vec2(132.0, 40.0)),
                        )
                        .clicked()
                    {
                        if let Some(svc) = &self.host_service {
                            svc.approve_incoming(&peer_id, false);
                        }
                        self.host_pending_peer = None;
                    }
                });
            });
    }

    fn host_ui_commercial(&mut self, ui: &mut egui::Ui) {
        ui.add_space(0.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(self.text("Этот компьютер", "This computer"))
                        .size(34.0)
                        .strong()
                        .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(self.text(
                        "Покажите этот ID и пароль для входящего подключения",
                        "Share this ID and password for direct unattended access",
                    ))
                    .size(15.0)
                    .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (r, g, b) = self.host_state.color();
                status_pill(ui, self.host_state_text(), egui::Color32::from_rgb(r, g, b));
            });
        });

        ui.add_space(24.0);

        card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                egui::RichText::new(self.text("Данные доступа", "Access credentials"))
                    .size(18.0)
                    .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
            );
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(self.text("Ваш ID", "Your ID"))
                            .size(12.0)
                            .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
                    );
                    ui.label(
                        egui::RichText::new(format_peer_id(&self.config.local_id))
                            .size(28.0)
                            .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, "copy")
                        .on_hover_text(self.text("Скопировать ID", "Copy ID"))
                        .clicked()
                    {
                        ui.ctx().copy_text(self.config.local_id.clone());
                    }
                });
            });

            ui.add_space(18.0);
            ui.separator();
            ui.add_space(18.0);

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(self.text("Пароль доступа", "Access password"))
                            .size(12.0)
                            .color(egui::Color32::from_rgb(0x50, 0x58, 0x68)),
                    );
                    let pw_text = if self.show_host_password {
                        self.config.local_password.clone()
                    } else {
                        "•".repeat(self.config.local_password.len())
                    };
                    ui.label(
                        egui::RichText::new(pw_text)
                            .size(24.0)
                            .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                    );
                });
                let show_label = self.text("показать", "show");
                let copy_pw_tip = self.text("Скопировать пароль", "Copy password");
                let new_pw_tip = self.text("Новый пароль", "New password");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.show_host_password, show_label);
                    if icon_button(ui, "copy").on_hover_text(copy_pw_tip).clicked() {
                        ui.ctx().copy_text(self.config.local_password.clone());
                    }
                    if icon_button(ui, "refresh")
                        .on_hover_text(new_pw_tip)
                        .clicked()
                    {
                        self.config.local_password = generate_numeric_token(6);
                        self.config.save();
                    }
                });
            });
        });

        ui.add_space(24.0);
        card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                egui::RichText::new(self.text("Доступ", "Sharing"))
                    .size(18.0)
                    .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
            );
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                let service_running = self.host_service.is_some();
                let label = if service_running {
                    self.text("Остановить доступ", "Stop sharing")
                } else {
                    self.text("Запустить доступ", "Start sharing")
                };
                let button = egui::Button::new(
                    egui::RichText::new(label)
                        .size(16.0)
                        .strong()
                        .color(egui::Color32::from_rgb(0x13, 0x17, 0x21)),
                )
                .min_size(egui::vec2(180.0, 54.0))
                .fill(egui::Color32::from_rgb(0xFF, 0xFF, 0xFF))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgb(0xE3, 0xE6, 0xEC),
                ))
                .corner_radius(egui::CornerRadius::same(10));
                if ui.add(button).clicked() {
                    if service_running {
                        self.stop_host_service();
                    } else {
                        self.start_host_service();
                    }
                }
            });
            ui.add_space(18.0);
            ui.separator();
            ui.add_space(18.0);
            ui.label(
                egui::RichText::new(&self.host_status)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
            );
            if let Some(video_status) = &self.host_video_status {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("Video: {video_status}"))
                        .monospace()
                        .size(10.5)
                        .color(egui::Color32::from_rgb(0x67, 0x70, 0x80)),
                );
            }
        });

        ui.add_space(24.0);
        card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            egui::CollapsingHeader::new(self.text("Диагностика хоста", "Host diagnostics"))
                .default_open(false)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .button(self.text("Скопировать лог", "Copy log"))
                            .clicked()
                        {
                            ui.ctx().copy_text(self.host_log.join("\n"));
                            self.host_status = "Host log copied".to_owned();
                        }
                        if ui.button(self.text("Очистить", "Clear")).clicked() {
                            self.host_log.clear();
                        }
                    });
                    egui::ScrollArea::vertical()
                        .max_height(140.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for line in self.host_log.iter().rev().take(30) {
                                ui.label(
                                    egui::RichText::new(line)
                                        .monospace()
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(150, 158, 168)),
                                );
                            }
                        });
                });
        });
    }

    fn start_host_service(&mut self) {
        if self.host_service.is_some() {
            return;
        }
        self.host_log
            .push(format!("[{}] Запуск хост-сервиса...", timestamp_hms()));
        self.host_state = HostState::Connecting;
        self.host_video_status = None;
        self.host_service = Some(HostService::start(self.config.clone()));
    }

    fn stop_host_service(&mut self) {
        if let Some(svc) = self.host_service.take() {
            svc.stop();
            self.host_log
                .push(format!("[{}] Хост-сервис остановлен.", timestamp_hms()));
        }
        self.host_state = HostState::Idle;
        self.host_pending_peer = None;
        self.host_video_status = None;
    }

    fn poll_host_service(&mut self) {
        let Some(svc) = &self.host_service else {
            return;
        };
        // Drain all events from the host background thread.
        let mut events_buf: Vec<HostEvent> = Vec::new();
        while let Some(ev) = svc.try_recv() {
            events_buf.push(ev);
        }
        for ev in events_buf {
            self.handle_host_event(ev);
        }
    }

    fn handle_host_event(&mut self, ev: HostEvent) {
        match ev {
            HostEvent::StateChanged(state) => {
                self.host_state = state;
            }
            HostEvent::Registered { .. } => {
                self.host_log.push(format!(
                    "[{}] Зарегистрировано на ID сервере ✓",
                    timestamp_hms()
                ));
            }
            HostEvent::IncomingRequest { peer_id, .. } => {
                self.host_log.push(format!(
                    "[{}] Входящий запрос от {peer_id}",
                    timestamp_hms()
                ));
                self.host_status = format!("Incoming connection: authenticating {peer_id}");
            }
            HostEvent::ApprovalRequested { peer_id } => {
                self.host_log.push(format!(
                    "[{}] Запрос подтверждения от {peer_id}",
                    timestamp_hms()
                ));
                self.host_pending_peer = Some(peer_id.clone());
                self.host_status = format!("Требуется подтверждение: {peer_id}");
            }
            HostEvent::SessionStarted { peer_id } => {
                self.host_log
                    .push(format!("[{}] Сессия с {peer_id} начата", timestamp_hms()));
                self.host_pending_peer = None;
                self.host_status = format!("Connected: {peer_id}");
                self.host_video_status = None;
            }
            HostEvent::SessionEnded { peer_id, reason } => {
                self.host_log.push(format!(
                    "[{}] Сессия с {peer_id} завершена {reason}",
                    timestamp_hms(),
                ));
                self.host_video_status = None;
            }
            HostEvent::VideoTelemetry {
                summary,
                fallback_reason,
            } => {
                self.host_video_status = Some(compact_host_video_status(&summary));
                if let Some(reason) = fallback_reason {
                    self.host_status = format!("Video fallback: {reason}");
                }
            }
            HostEvent::Log(msg) => {
                self.host_log.push(format!("[{}] {msg}", timestamp_hms()));
                if self.host_log.len() > 200 {
                    self.host_log.drain(0..50);
                }
            }
        }
    }

    // ── Check host server (legacy) ────────────────────────────────────────────
    #[allow(dead_code)]
    fn check_host_server(&mut self) {
        if self.host_check_busy || self.busy {
            return;
        }
        self.host_check_busy = true;
        self.host_status = "Проверяем ID server...".to_owned();
        self.log("Host check: checking ID server".to_owned());
        let server = self.config.server.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = TransportClient::check_id_server(&server);
            let _ = tx.send(WorkerEvent::HostServerCheck(result));
        });
        self.worker = Some(rx);
    }

    // ── Settings window ───────────────────────────────────────────────────────

    fn close_remote_viewer_panel(&mut self) {
        self.remote_viewer_open = false;
        self.remote_viewer_window_spawned = false;
        self.remote_input_focused = false;
        self.release_remote_modifiers();
        self.last_mouse_pos = None;
        self.wheel_accum = egui::Vec2::ZERO;
        self.status = "Remote screen closed".to_owned();
        self.send_command(SessionCommand::SetAutoRefresh {
            enabled: false,
            millis: self.refresh_millis,
        });
    }

    fn switch_remote_display(&mut self, display: RemoteDisplay) {
        let label = display_label(&display);
        self.selected_display = display.index;
        self.remote_texture = None;
        self.remote_size = [0, 0];
        self.last_mouse_pos = None;
        self.cursor_pos = None;
        self.screenshot_pending = false;
        self.status = format!("Переключаем монитор: {label}");
        self.log(self.status.clone());
        self.send_command(SessionCommand::SetDisplay(display));
    }

    fn switch_remote_display_index(&mut self, index: i32) {
        let index = index.max(0);
        if let Some(display) = self
            .remote_displays
            .iter()
            .find(|display| display.index == index)
            .cloned()
        {
            self.switch_remote_display(display);
            return;
        }

        self.switch_remote_display(self.manual_remote_display(index));
    }

    fn manual_remote_display(&self, index: i32) -> RemoteDisplay {
        let index = index.max(0);
        RemoteDisplay {
            index,
            name: format!("Дисплей {} (ручной)", index.saturating_add(1)),
            width: i32::try_from(self.remote_size[0]).unwrap_or_default(),
            height: i32::try_from(self.remote_size[1]).unwrap_or_default(),
            x: 0,
            y: 0,
            cursor_embedded: false,
        }
    }

    fn set_remote_fullscreen(&mut self, ctx: &egui::Context, fullscreen: bool) {
        self.remote_fullscreen = fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fullscreen));
    }

    fn set_remote_video_profile(&mut self, fps: i32, codec: CodecPreference) {
        let fps = fps.clamp(5, 60);
        self.video_fps = fps;
        self.config.display.target_fps = fps as u32;
        self.config.display.codec = codec;
        self.config.save();
        self.send_command(SessionCommand::SetVideoProfile { fps, codec });
    }

    fn remote_display_selector_ui(&mut self, ui: &mut egui::Ui, id: &'static str) {
        let display_count = self.remote_displays.len();
        let selected_pos = self
            .remote_displays
            .iter()
            .position(|display| display.index == self.selected_display)
            .map(|pos| pos + 1)
            .unwrap_or(0);
        let selected_display_number = self.selected_display.max(0).saturating_add(1);
        let button_text = if display_count == 0 {
            format!("▣ {selected_display_number}/?")
        } else if selected_pos == 0 {
            format!("▣ {selected_display_number}/{display_count}")
        } else {
            format!("▣ {selected_pos}/{display_count}")
        };

        ui.push_id(id, |ui| {
            ui.menu_button(button_text, |ui| {
                ui.label("Мониторы хоста");
                ui.separator();

                if self.remote_displays.is_empty() {
                    ui.label("Хост пока не прислал список экранов.");
                } else {
                    let displays = self.remote_displays.clone();
                    for display in displays {
                        let selected = display.index == self.selected_display;
                        if ui
                            .selectable_label(selected, display_label(&display))
                            .clicked()
                        {
                            self.switch_remote_display(display);
                            ui.close();
                        }
                    }
                }

                ui.separator();
                ui.label("Ручной выбор");
                for index in [0_i32, 1, 2, 3] {
                    let known = self
                        .remote_displays
                        .iter()
                        .any(|display| display.index == index);
                    let label = if known {
                        format!("Дисплей {} (повторить)", index.saturating_add(1))
                    } else {
                        format!("Дисплей {}", index.saturating_add(1))
                    };
                    if ui
                        .selectable_label(self.selected_display == index, label)
                        .clicked()
                    {
                        self.switch_remote_display_index(index);
                        ui.close();
                    }
                }

                ui.separator();
                if ui.button("Перезапросить экран").clicked() {
                    self.refresh_remote_screen();
                    ui.close();
                }
                ui.label(format!("Получено от хоста: {display_count}"));
            })
            .response
            .on_hover_text("Выбор монитора удаленной машины");
        });
    }

    fn remote_video_profile_menu_ui(&mut self, ui: &mut egui::Ui) {
        ui.label("Частота кадров");
        ui.horizontal_wrapped(|ui| {
            for fps in [15, 20, 30, 60] {
                if ui
                    .selectable_label(self.video_fps == fps, format!("{fps}"))
                    .clicked()
                {
                    self.set_remote_video_profile(fps, self.config.display.codec);
                    ui.close();
                }
            }
        });

        ui.separator();
        ui.label("Кодек");
        for codec in [
            CodecPreference::Auto,
            CodecPreference::H264,
            CodecPreference::H265,
            CodecPreference::Av1,
            CodecPreference::Vp9,
        ] {
            let label = match codec {
                CodecPreference::Av1 if !crate::video::av1_available() => "AV1 (эксп.)",
                CodecPreference::H265 if !crate::video::h265_available() => "H265 (если доступен)",
                _ => codec.label(),
            };
            if ui
                .selectable_label(self.config.display.codec == codec, label)
                .clicked()
            {
                self.set_remote_video_profile(self.video_fps, codec);
                ui.close();
            }
        }
    }

    fn remote_more_menu_ui(&mut self, ui: &mut egui::Ui) {
        if ui
            .checkbox(&mut self.auto_refresh, "Авто-обновление PNG")
            .changed()
        {
            self.save_ui_config();
            self.send_command(SessionCommand::SetAutoRefresh {
                enabled: self.auto_refresh,
                millis: self.refresh_millis,
            });
        }
        let mut refresh_ms = self.refresh_millis as f32;
        if ui
            .add(
                egui::Slider::new(&mut refresh_ms, 50.0..=2000.0)
                    .text("мс / кадр")
                    .clamping(egui::SliderClamping::Always),
            )
            .changed()
        {
            self.refresh_millis = refresh_ms.round() as u64;
            self.save_ui_config();
            self.send_command(SessionCommand::SetAutoRefresh {
                enabled: self.auto_refresh,
                millis: self.refresh_millis,
            });
        }

        ui.separator();
        egui::ComboBox::from_id_salt("coordinate-mode")
            .selected_text(coordinate_mode_label(self.coordinate_mode))
            .show_ui(ui, |ui| {
                for mode in [
                    CoordinateMode::Auto,
                    CoordinateMode::Absolute,
                    CoordinateMode::Local,
                ] {
                    if ui
                        .selectable_value(
                            &mut self.coordinate_mode,
                            mode,
                            coordinate_mode_label(mode),
                        )
                        .clicked()
                    {
                        self.save_ui_config();
                        self.last_mouse_pos = None;
                    }
                }
            });

        ui.separator();
        if ui.button("Ctrl+Alt+Del").clicked() {
            self.send_command(SessionCommand::KeyControl(ControlKey::CtrlAltDel));
            self.request_visual_refresh_after_input();
            ui.close();
        }
        if ui.button("Заблокировать экран").clicked() {
            self.send_command(SessionCommand::KeyControl(ControlKey::LockScreen));
            self.request_visual_refresh_after_input();
            ui.close();
        }

        ui.separator();
        if ui.button("Сохранить лог").clicked() {
            self.save_session_log_file();
            ui.close();
        }
        if ui.button("Собрать отчёт").clicked() {
            self.save_support_report();
            ui.close();
        }

        ui.separator();
        if let Some((x, y)) = self.last_mouse_pos {
            ui.label(format!("Мышь: {x}, {y}"));
        }
        if let Some(age) = self.last_screenshot_age_ms() {
            ui.label(format!("Возраст кадра: {age} мс"));
        }
        ui.label(format!("Событий ввода: {}", self.input_events_sent));
    }

    fn refresh_remote_screen(&mut self) {
        let display = self
            .remote_displays
            .iter()
            .find(|display| display.index == self.selected_display)
            .cloned()
            .unwrap_or_else(|| self.manual_remote_display(self.selected_display));
        self.send_command(SessionCommand::SetDisplay(display));
        self.log(format!(
            "Remote screen refresh requested; selected display: {}, host displays known: {}",
            self.selected_display.max(0).saturating_add(1),
            self.remote_displays.len(),
        ));
    }

    fn remote_session_toolbar_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        detached_window: bool,
    ) {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.horizontal_wrapped(|ui| {
            if !detached_window
                && remote_icon_button(ui, "←", "Закрыть экран без отключения сеанса").clicked()
            {
                self.close_remote_viewer_panel();
                return;
            }

            if remote_icon_button(ui, "⏻", "Завершить удаленный сеанс").clicked()
            {
                self.remote_viewer_open = false;
                self.remote_input_focused = false;
                self.release_remote_modifiers();
                self.last_mouse_pos = None;
                self.wheel_accum = egui::Vec2::ZERO;
                self.disconnect_session("Отключено");
                if detached_window {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                return;
            }

            self.remote_display_selector_ui(
                ui,
                if detached_window {
                    "remote-display"
                } else {
                    "software-remote-display"
                },
            );

            ui.add(
                egui::TextEdit::singleline(&mut self.text_to_send)
                    .hint_text("Текст")
                    .desired_width(if detached_window { 140.0 } else { 160.0 }),
            );
            let send_text = remote_icon_button_enabled(
                ui,
                !self.text_to_send.is_empty(),
                "↵",
                "Отправить текст",
            );
            if send_text.clicked()
                || (ui.ctx().input(|i| i.key_pressed(egui::Key::Enter))
                    && !self.remote_input_focused
                    && !self.text_to_send.is_empty())
            {
                let text = std::mem::take(&mut self.text_to_send);
                self.send_command(SessionCommand::KeyText(text));
                self.request_visual_refresh_after_input();
            }
            if remote_icon_button(ui, "⧉", "Вставить локальный буфер").clicked()
            {
                self.paste_local_clipboard_to_remote();
            }

            if remote_icon_button(ui, "↻", "Перезапросить live-video и контрольный PNG-кадр")
                .clicked()
            {
                self.refresh_remote_screen();
            }
            if remote_icon_button(ui, "PNG", "Сохранить текущий кадр").clicked()
            {
                self.save_current_frame_png();
            }
            if remote_icon_toggle(ui, "⇱", self.fit_to_window, "Масштабировать экран под окно")
                .clicked()
            {
                self.fit_to_window = !self.fit_to_window;
                self.save_ui_config();
            }
            if remote_icon_button(
                ui,
                if self.remote_fullscreen { "□" } else { "⛶" },
                "Полный экран (F11)",
            )
            .clicked()
            {
                self.set_remote_fullscreen(ctx, !self.remote_fullscreen);
            }

            ui.menu_button(
                format!(
                    "AV {} {}",
                    self.config.display.codec.label(),
                    self.video_fps
                ),
                |ui| self.remote_video_profile_menu_ui(ui),
            );
            ui.menu_button("⋯", |ui| self.remote_more_menu_ui(ui));
        });
    }

    #[allow(deprecated)]
    fn remote_viewer_inline(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.remote_input_focused = false;
            self.release_remote_modifiers();
            self.last_mouse_pos = None;
            self.wheel_accum = egui::Vec2::ZERO;
        }

        egui::Panel::top("software-remote-toolbar").show(ctx, |ui| {
            self.remote_session_toolbar_ui(ui, ctx, false);
        });

        egui::Panel::bottom("software-remote-statusbar").show(ctx, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                // Разрешение
                if self.remote_size[0] > 0 {
                    let _ = stat_pill(
                        ui,
                        None,
                        &format!("{}×{}", self.remote_size[0], self.remote_size[1]),
                        egui::Color32::from_rgb(0xC8, 0xD0, 0xDD),
                    );
                    ui.add_space(6.0);
                }
                // FPS
                let (fps_dot, _) = health_dot(self.display_fps);
                let _ = stat_pill(
                    ui,
                    Some(fps_dot),
                    &format!("{:.0} fps", self.display_fps),
                    egui::Color32::from_rgb(0xE8, 0xEE, 0xF6),
                );
                ui.add_space(6.0);
                // Кодек
                let (codec_label, codec_color) = match self.last_frame_codec.as_str() {
                    "H264" | "H265" | "AV1" | "VP9" => (
                        self.last_frame_codec.as_str(),
                        egui::Color32::from_rgb(0x5A, 0xE0, 0x9A),
                    ),
                    "PNG" => ("PNG", egui::Color32::from_rgb(0xF0, 0xC0, 0x50)),
                    _ => ("—", egui::Color32::GRAY),
                };
                let _ = stat_pill(ui, None, codec_label, codec_color);
                ui.add_space(6.0);
                // Задержка
                if let Some(ms) = self.latency_ms {
                    let lat_color = if ms <= 40 {
                        egui::Color32::from_rgb(0x5A, 0xE0, 0x9A)
                    } else if ms <= 90 {
                        egui::Color32::from_rgb(0xF0, 0xC0, 0x50)
                    } else {
                        egui::Color32::from_rgb(0xE0, 0x70, 0x70)
                    };
                    let _ = stat_pill(ui, None, &format!("{ms} ms"), lat_color);
                    ui.add_space(6.0);
                }
                // EVRT бейдж
                if self.evrt_active {
                    let pressure_color = match self.evrt_pressure.as_str() {
                        "critical" => egui::Color32::from_rgb(0xE0, 0x60, 0x60),
                        "high" => egui::Color32::from_rgb(0xF0, 0xC0, 0x50),
                        _ => egui::Color32::from_rgb(0x32, 0xD8, 0x8C),
                    };
                    let _ = stat_pill(ui, Some(pressure_color), "⚡ EVRT", pressure_color);
                    ui.add_space(6.0);
                }

                // Справа: кнопка деталей + индикатор ввода
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(4.0);
                    if self.screenshot_pending {
                        ui.spinner();
                        ui.add_space(6.0);
                    }
                    let info_btn = ui.add(
                        egui::Button::new(egui::RichText::new("ℹ Детали").size(11.5).color(
                            if self.show_stream_info {
                                egui::Color32::from_rgb(0x12, 0xC9, 0x72)
                            } else {
                                egui::Color32::from_rgb(0xB0, 0xB8, 0xC4)
                            },
                        ))
                        .frame(false),
                    );
                    if info_btn.clicked() {
                        self.show_stream_info = !self.show_stream_info;
                    }
                    ui.add_space(8.0);
                    // Краткое здоровье потока
                    let _ = stat_pill(
                        ui,
                        Some(stream_health_color(&self.stream_health)),
                        &self.stream_health,
                        egui::Color32::from_rgb(0xB8, 0xC2, 0xD0),
                    );
                    if self.remote_input_focused {
                        ui.add_space(6.0);
                        let _ = stat_pill(
                            ui,
                            Some(egui::Color32::from_rgb(0x2D, 0xA0, 0xE6)),
                            "ввод захвачен [Esc]",
                            egui::Color32::from_rgb(0x7A, 0xC0, 0xF0),
                        );
                    }
                });
            });
            ui.add_space(3.0);
        });

        // ── Окно детальной диагностики ────────────────────────────────────────
        self.stream_info_window(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            self.remote_screen_ui(ui);
        });
    }

    /// Красивое окно с детальной диагностикой стрима (по кнопке ℹ Детали).
    fn stream_info_window(&mut self, ctx: &egui::Context) {
        if !self.show_stream_info {
            return;
        }
        let mut open = self.show_stream_info;
        egui::Window::new(self.text("Диагностика потока", "Stream diagnostics"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(380.0)
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -56.0])
            .show(ctx, |ui| {
                let green = egui::Color32::from_rgb(0x12, 0xC9, 0x72);
                let white = egui::Color32::from_rgb(0x1A, 0x1F, 0x2A);
                let amber = egui::Color32::from_rgb(0xE0, 0xA0, 0x30);

                // ── Видео ──────────────────────────────────────────────────────
                ui.label(
                    egui::RichText::new(self.text("ВИДЕО", "VIDEO"))
                        .size(11.0)
                        .strong()
                        .color(green),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    info_metric(
                        ui,
                        self.text("Разрешение", "Resolution"),
                        &format!("{}×{}", self.remote_size[0], self.remote_size[1]),
                        white,
                    );
                    ui.add_space(24.0);
                    info_metric(ui, "FPS", &format!("{:.1}", self.display_fps), white);
                    ui.add_space(24.0);
                    info_metric(
                        ui,
                        self.text("Кодек", "Codec"),
                        &self.last_frame_codec,
                        white,
                    );
                });
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(12.0);

                // ── Сеть ───────────────────────────────────────────────────────
                ui.label(
                    egui::RichText::new(self.text("СЕТЬ", "NETWORK"))
                        .size(11.0)
                        .strong()
                        .color(green),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    info_metric(
                        ui,
                        self.text("Входящий", "Inbound"),
                        &format!("{:.1}/s", self.stream_input_fps),
                        white,
                    );
                    ui.add_space(20.0);
                    info_metric(
                        ui,
                        self.text("Битрейт", "Bitrate"),
                        &format!("{} kbps", self.stream_input_kbps),
                        white,
                    );
                    ui.add_space(20.0);
                    info_metric(
                        ui,
                        self.text("Пинг", "Ping"),
                        &self
                            .latency_ms
                            .map(|m| format!("{m} ms"))
                            .unwrap_or_else(|| "—".into()),
                        white,
                    );
                });
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(12.0);

                // ── Декод ──────────────────────────────────────────────────────
                ui.label(
                    egui::RichText::new(self.text("ДЕКОД", "DECODE"))
                        .size(11.0)
                        .strong()
                        .color(green),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    info_metric(
                        ui,
                        self.text("Кадр", "Frame"),
                        &format!("{} KB", self.frame_bytes / 1024),
                        white,
                    );
                    ui.add_space(20.0);
                    info_metric(
                        ui,
                        self.text("Очередь", "Queue"),
                        &format!("{} ms", self.frame_queue_ms),
                        white,
                    );
                    ui.add_space(20.0);
                    info_metric(
                        ui,
                        self.text("Декод", "Decode"),
                        &format!("{} ms", self.frame_decode_ms),
                        white,
                    );
                    ui.add_space(20.0);
                    info_metric(
                        ui,
                        self.text("Дропы", "Drops"),
                        &self.frame_dropped.to_string(),
                        if self.frame_dropped > 0 { amber } else { white },
                    );
                });

                // ── EVRT (если активен) ────────────────────────────────────────
                if self.evrt_active {
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("⚡ EVRT")
                                .size(11.0)
                                .strong()
                                .color(green),
                        );
                        ui.label(
                            egui::RichText::new(format!("→ {}", self.evrt_host_addr))
                                .size(11.0)
                                .color(egui::Color32::from_rgb(0x8A, 0x93, 0xA3)),
                        );
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let pc = match self.evrt_pressure.as_str() {
                            "critical" => egui::Color32::from_rgb(0xE0, 0x60, 0x60),
                            "high" => amber,
                            _ => green,
                        };
                        info_metric(
                            ui,
                            self.text("Давление", "Pressure"),
                            &self.evrt_pressure,
                            pc,
                        );
                        ui.add_space(16.0);
                        info_metric(
                            ui,
                            "Δ arrive",
                            &format!("{} ms", self.evrt_arrival_delta_ms),
                            white,
                        );
                        ui.add_space(16.0);
                        info_metric(
                            ui,
                            "Assemble",
                            &format!("{} ms", self.evrt_assembly_delay_ms),
                            white,
                        );
                        ui.add_space(16.0);
                        info_metric(ui, "Jitter", &format!("{} ms", self.evrt_jitter_ms), white);
                        ui.add_space(16.0);
                        info_metric(ui, "FPS", &self.evrt_fps.to_string(), white);
                    });
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{}: сборка {} / очередь {}  •  пакетов {}",
                            self.text("Потери", "Loss"),
                            self.evrt_reassembly_drops,
                            self.evrt_queue_drops,
                            self.evrt_packets_received,
                        ))
                        .size(11.0)
                        .color(egui::Color32::from_rgb(0x8A, 0x93, 0xA3)),
                    );
                } else {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(self.text(
                            "📡 TCP relay (EVRT не активен)",
                            "📡 TCP relay (EVRT inactive)",
                        ))
                        .size(11.5)
                        .color(egui::Color32::from_rgb(0x8A, 0x93, 0xA3)),
                    );
                }
            });
        self.show_stream_info = open;
    }

    #[allow(deprecated)]
    fn remote_viewer_window(&mut self, ctx: &egui::Context) {
        let title = {
            let id = if self.remote_id.trim().is_empty() {
                "удалённый ПК".to_owned()
            } else {
                self.remote_id.trim().to_owned()
            };
            let fps_part = if self.display_fps > 0.1 {
                format!("  {:.0} fps", self.display_fps)
            } else {
                String::new()
            };
            let lat_part = self
                .latency_ms
                .map(|ms| format!("  {ms}ms"))
                .unwrap_or_default();
            format!("EvertyDesk — {id}{fps_part}{lat_part}")
        };
        let viewport_id = egui::ViewportId::from_hash_of("evertydesk-lite-remote-viewer");
        let mut builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_resizable(true)
            .with_min_inner_size([720.0, 480.0]);
        if !self.remote_viewer_window_spawned {
            builder = builder.with_inner_size(remote_viewer_initial_size(self.remote_size));
            self.remote_viewer_window_spawned = true;
        }

        ctx.show_viewport_immediate(viewport_id, builder, |ctx, _class| {
            if ctx.input(|input| input.viewport().close_requested()) {
                self.remote_viewer_open = false;
                self.remote_viewer_window_spawned = false;
                self.remote_input_focused = false;
                self.release_remote_modifiers();
                self.last_mouse_pos = None;
                self.wheel_accum = egui::Vec2::ZERO;
                self.disconnect_session("Окно управления закрыто");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            if ctx.input(|input| input.key_pressed(egui::Key::F11)) {
                self.remote_fullscreen = !self.remote_fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.remote_fullscreen));
            }

            egui::Panel::top("remote-toolbar").show(ctx, |ui| {
                self.remote_session_toolbar_ui(ui, ctx, true);
            });

            // ── Status bar (bottom) ─────────────────────────────────────────────────
            egui::Panel::bottom("remote-statusbar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Resolution
                    if self.remote_size[0] > 0 {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}×{}",
                                self.remote_size[0], self.remote_size[1]
                            ))
                            .monospace()
                            .size(11.0),
                        );
                        ui.separator();
                    }
                    // FPS
                    let fps_color = if self.display_fps >= 20.0 {
                        egui::Color32::from_rgb(80, 200, 100)
                    } else if self.display_fps >= 8.0 {
                        egui::Color32::from_rgb(220, 180, 60)
                    } else {
                        egui::Color32::from_rgb(220, 80, 80)
                    };
                    ui.label(
                        egui::RichText::new(format!("{:.1} fps", self.display_fps))
                            .monospace()
                            .size(11.0)
                            .color(fps_color),
                    );
                    ui.separator();
                    let input_color = if self.stream_input_fps >= 20.0 {
                        egui::Color32::from_rgb(80, 200, 100)
                    } else if self.stream_input_fps >= 8.0 {
                        egui::Color32::from_rgb(220, 180, 60)
                    } else {
                        egui::Color32::from_rgb(220, 80, 80)
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "in {:.1}/s {} kbps",
                            self.stream_input_fps, self.stream_input_kbps
                        ))
                        .monospace()
                        .size(11.0)
                        .color(input_color),
                    )
                    .on_hover_text("Входящие live-video пакеты до декодера");
                    ui.separator();
                    // Codec badge: color shows quality:
                    //   H264 green  = live video, low latency
                    //   PNG  amber  = screenshot mode, higher latency
                    //   none gray   = no frames yet
                    let (codec_label, codec_color, codec_tip) = match self.last_frame_codec.as_str()
                    {
                        "H264" => (
                            "H264",
                            egui::Color32::from_rgb(80, 220, 110),
                            "Live H264 video, lowest latency",
                        ),
                        "H265" | "AV1" | "VP9" => (
                            self.last_frame_codec.as_str(),
                            egui::Color32::from_rgb(80, 220, 110),
                            "Live video",
                        ),
                        "PNG" => (
                            "PNG",
                            egui::Color32::from_rgb(220, 180, 60),
                            "Screenshot fallback, higher latency",
                        ),
                        _ => ("no frame", egui::Color32::GRAY, "No frames received yet"),
                    };
                    ui.label(
                        egui::RichText::new(codec_label)
                            .strong()
                            .size(12.5)
                            .color(codec_color),
                    )
                    .on_hover_text(codec_tip);
                    ui.separator();
                    ui.label(
                        egui::RichText::new(crate::video::build_codec_label())
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(145, 160, 175)),
                    )
                    .on_hover_text("Codecs compiled into this EvertyDesk Lite build");
                    ui.separator();
                    // Latency
                    if let Some(ms) = self.latency_ms {
                        let lat_color = if ms < 50 {
                            egui::Color32::from_rgb(80, 200, 100)
                        } else if ms < 150 {
                            egui::Color32::from_rgb(220, 180, 60)
                        } else {
                            egui::Color32::from_rgb(220, 80, 80)
                        };
                        ui.label(
                            egui::RichText::new(format!("{ms} ms"))
                                .monospace()
                                .size(11.0)
                                .color(lat_color),
                        );
                        ui.separator();
                    }
                    if self.frame_bytes > 0 {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} KB  q:{}ms  dec:{}ms",
                                self.frame_bytes / 1024,
                                self.frame_queue_ms,
                                self.frame_decode_ms
                            ))
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(150, 160, 170)),
                        )
                        .on_hover_text("Размер кадра, ожидание в очереди и время декодирования");
                        ui.separator();
                    }
                    if self.frame_dropped > 0 {
                        ui.label(
                            egui::RichText::new(format!("drop {}", self.frame_dropped))
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(220, 110, 80)),
                        )
                        .on_hover_text("Сколько старых кадров было сброшено, чтобы догнать поток");
                        ui.separator();
                    }
                    ui.label(
                        egui::RichText::new(&self.stream_health)
                            .size(11.0)
                            .color(stream_health_color(&self.stream_health)),
                    )
                    .on_hover_text("Автоматическая оценка состояния видеопотока");
                    ui.separator();
                    if let Some(status) = &self.clipboard_status {
                        ui.label(
                            egui::RichText::new(status)
                                .size(11.0)
                                .color(egui::Color32::from_rgb(150, 160, 170)),
                        );
                        ui.separator();
                    }
                    if let Some(status) = &self.screenshot_status {
                        ui.label(
                            egui::RichText::new(status)
                                .size(11.0)
                                .color(egui::Color32::from_rgb(150, 160, 170)),
                        );
                        ui.separator();
                    }
                    if let Some(status) = &self.log_status {
                        ui.label(
                            egui::RichText::new(status)
                                .size(11.0)
                                .color(egui::Color32::from_rgb(150, 160, 170)),
                        );
                        ui.separator();
                    }
                    if let Some(status) = &self.report_status {
                        ui.label(
                            egui::RichText::new(status)
                                .size(11.0)
                                .color(egui::Color32::from_rgb(150, 160, 170)),
                        );
                        ui.separator();
                    }
                    // Input focus indicator
                    if self.remote_input_focused {
                        ui.label(
                            egui::RichText::new("⌨ ввод захвачен  [Esc = отпустить]")
                                .size(11.0)
                                .color(egui::Color32::from_rgb(45, 160, 230)),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("наведите мышь → ввод")
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        );
                    }
                    // Pending indicator
                    if self.screenshot_pending {
                        ui.spinner();
                    }
                });
            });

            egui::CentralPanel::default().show(ctx, |ui| {
                self.remote_screen_ui(ui);
            });
        });
    }

    fn remote_screen_ui(&mut self, ui: &mut egui::Ui) {
        let available_size = ui.available_size_before_wrap();
        let available_width = available_size.x.max(1.0);
        let Some(texture) = self.remote_texture.clone() else {
            ui.allocate_ui(
                egui::vec2(available_width, available_size.y.max(360.0)),
                |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.spinner();
                        ui.label("Ожидание первого кадра…");
                    });
                },
            );
            return;
        };

        let [w, h] = self.remote_size;
        if w == 0 || h == 0 {
            return;
        }
        let max_height = available_size.y.max(360.0);
        let scale = if self.fit_to_window {
            (available_width / w as f32)
                .min(max_height / h as f32)
                .clamp(0.05, 4.0)
        } else {
            1.0
        };
        let size = egui::vec2(w as f32 * scale, h as f32 * scale);

        // Auto-capture keyboard when pointer hovers inside the remote screen.
        // Release when pointer leaves the screen area.
        let pointer_in_screen = ui
            .ctx()
            .input(|i| i.pointer.hover_pos())
            .map(|p| {
                // Approximate rect check: allocate enough to know where the image will land.
                // We'll refine after the response is known.
                let _ = p;
                true
            })
            .unwrap_or(false);
        let _ = pointer_in_screen; // refined below after response

        // During live VP9 streaming the Windows VP9 encoder (Desktop Duplication)
        // embeds cursor pixels directly in the captured frame, so the separate
        // cursor overlay is redundant.  Additionally, CursorId switch messages
        // currently fail to parse (proto wire-type mismatch in RustDesk 1.4.6),
        // Hide the OS cursor and draw the remote cursor overlay whenever we have
        // a remote cursor image, regardless of codec (H264, VP9, PNG).
        // Previously this only worked in non-VP9 mode; the variable was
        // mis-named and caused the cursor to be invisible during H264 streaming.
        let live_video_active = self
            .last_live_frame_at
            .map(|t| t.elapsed() < Duration::from_secs(2))
            .unwrap_or(false);
        let _ = live_video_active; // kept for future use

        let hover_cursor = if self.cursor_texture.is_some() {
            egui::CursorIcon::None
        } else {
            egui::CursorIcon::Default
        };
        let response = ui
            .add(
                egui::Image::new(&texture)
                    .fit_to_exact_size(size)
                    .sense(egui::Sense::click_and_drag()),
            )
            .on_hover_cursor(hover_cursor);

        // Auto-focus keyboard input when pointer is inside remote screen.
        if self.connected {
            let hovering = response.hovered()
                || response
                    .ctx
                    .input(|i| i.pointer.hover_pos())
                    .map(|p| response.rect.contains(p))
                    .unwrap_or(false);
            if hovering && !self.remote_input_focused {
                self.remote_input_focused = true;
            }
            // Release keyboard focus when pointer leaves the screen entirely.
            if !hovering && !response.is_pointer_button_down_on() {
                // Only release if we're not in the middle of a drag
                if !ui.ctx().input(|i| i.pointer.any_down()) {
                    self.remote_input_focused = false;
                }
            }
        }
        if !self.remote_input_focused {
            self.release_remote_modifiers();
        }

        // Draw a colored focus border around the remote screen when keyboard is captured.
        if self.remote_input_focused {
            let border_color = egui::Color32::from_rgb(45, 160, 230);
            ui.painter().rect_stroke(
                response.rect,
                0.0,
                egui::Stroke::new(2.0, border_color),
                egui::StrokeKind::Inside,
            );
        }

        // Draw remote cursor overlay on top of the video for all codecs.
        {
            if let Some((cursor_tex, hotx, hoty)) = &self.cursor_texture {
                let cursor_px = cursor_tex.size_vec2();
                let draw_pos = if let Some(rpos) = self.cursor_pos {
                    egui::pos2(
                        response.rect.min.x + (rpos.x / w as f32) * size.x - *hotx as f32 * scale,
                        response.rect.min.y + (rpos.y / h as f32) * size.y - *hoty as f32 * scale,
                    )
                } else if let Some(local) = response.hover_pos() {
                    egui::pos2(
                        local.x - *hotx as f32 * scale,
                        local.y - *hoty as f32 * scale,
                    )
                } else {
                    return;
                };
                let cursor_rect = egui::Rect::from_min_size(draw_pos, cursor_px * scale);
                ui.painter().image(
                    cursor_tex.id(),
                    cursor_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        } // end cursor overlay

        if self.connected {
            // Mouse movement
            let pointer_pos = response
                .interact_pointer_pos()
                .or_else(|| response.hover_pos());
            if let Some(pos) = pointer_pos.filter(|pos| response.rect.contains(*pos)) {
                let local = pos - response.rect.min;
                let (x, y) = self.remote_point_from_local(local.x / scale, local.y / scale);
                // Update cursor overlay position immediately from local input.
                // This makes the cursor feel responsive even at low video fps.
                self.cursor_pos = Some(egui::pos2(x as f32, y as f32));
                if self.last_mouse_pos != Some((x, y)) {
                    self.last_mouse_pos = Some((x, y));
                    self.send_command(SessionCommand::MouseMove { x, y });
                    if self.should_refresh_after_move() {
                        self.send_command(SessionCommand::Screenshot);
                    }
                }
            }

            // Mouse clicks & scroll
            {
                let events = ui.input(|input| input.events.clone());
                for event in &events {
                    match event {
                        egui::Event::PointerButton {
                            pos,
                            button,
                            pressed,
                            ..
                        } => {
                            let inside = response.rect.contains(*pos);
                            if *pressed && !inside {
                                continue;
                            }
                            let (x, y) = if inside {
                                let local = *pos - response.rect.min;
                                self.remote_point_from_local(local.x / scale, local.y / scale)
                            } else {
                                self.last_mouse_pos.unwrap_or((0, 0))
                            };
                            match (button, pressed) {
                                (egui::PointerButton::Primary, true) => {
                                    self.send_command(SessionCommand::MouseDown { x, y });
                                    self.request_visual_refresh_after_input();
                                }
                                (egui::PointerButton::Primary, false) => {
                                    self.send_command(SessionCommand::MouseUp { x, y });
                                    self.request_visual_refresh_after_input();
                                }
                                (egui::PointerButton::Secondary, true) => {
                                    self.send_command(SessionCommand::MouseRightDown { x, y });
                                    self.request_visual_refresh_after_input();
                                }
                                (egui::PointerButton::Secondary, false) => {
                                    self.send_command(SessionCommand::MouseRightUp { x, y });
                                    self.request_visual_refresh_after_input();
                                }
                                (egui::PointerButton::Middle, true) => {
                                    self.send_command(SessionCommand::MouseMiddleDown { x, y });
                                    self.request_visual_refresh_after_input();
                                }
                                (egui::PointerButton::Middle, false) => {
                                    self.send_command(SessionCommand::MouseMiddleUp { x, y });
                                    self.request_visual_refresh_after_input();
                                }
                                _ => {}
                            }
                        }
                        egui::Event::MouseWheel { unit, delta, .. } => {
                            if let Some((x, y)) = self.wheel_delta(*unit, *delta) {
                                self.send_command(SessionCommand::MouseWheel { x, y });
                                self.request_visual_refresh_after_input();
                            }
                        }
                        _ => {}
                    }
                }
            }

            // Keyboard (active when pointer is over screen, no UI widget has focus)
            if self.remote_input_focused && !ui.ctx().egui_wants_keyboard_input() {
                self.handle_remote_keyboard(ui.ctx());
            }
        }
    }

    fn handle_remote_keyboard(&mut self, ctx: &egui::Context) {
        let current_modifiers = ctx.input(|input| input.modifiers);
        self.sync_remote_modifiers(current_modifiers);

        let events = ctx.input(|input| input.events.clone());
        for event in events {
            match event {
                // Paste from local clipboard → send to remote as text
                egui::Event::Paste(text) if !text.is_empty() => {
                    self.send_command(SessionCommand::KeyText(text));
                    self.request_visual_refresh_after_input();
                }
                // Printable characters from the OS input method (handles all layouts/IME)
                egui::Event::Text(text) if !text.is_empty() => {
                    self.send_command(SessionCommand::KeyText(text));
                    self.request_visual_refresh_after_input();
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } => {
                    // Ctrl+Escape releases keyboard capture
                    if key == egui::Key::Escape && modifiers.ctrl {
                        self.remote_input_focused = false;
                        self.release_remote_modifiers();
                        continue;
                    }
                    // Escape alone also releases (press Ctrl to send real Escape to remote)
                    if key == egui::Key::Escape && !has_command_modifier(modifiers) {
                        self.remote_input_focused = false;
                        self.release_remote_modifiers();
                        continue;
                    }
                    // Allow key-repeat only for navigation/edit keys (arrows, backspace, delete,
                    // page-up/down, home, end). Other keys skip repeat to avoid duplicates.
                    if repeat && !key_allows_repeat(key) {
                        continue;
                    }
                    if has_command_modifier(modifiers) && egui_key_to_text(key).is_some() {
                        let text = egui_key_to_text(key).unwrap();
                        self.send_command(SessionCommand::KeyText(text));
                        self.request_visual_refresh_after_input();
                    } else if let Some(control_key) = egui_key_to_control_key(key) {
                        self.send_command(SessionCommand::KeyControl(control_key));
                        self.request_visual_refresh_after_input();
                    }
                }
                _ => {}
            }
        }
    }

    fn sync_remote_modifiers(&mut self, modifiers: egui::Modifiers) {
        let next = RemoteModifierState::from_egui(modifiers);
        if next == self.remote_modifiers_down {
            return;
        }

        let previous = self.remote_modifiers_down;
        previous.for_each(|key, was_down| {
            let now_down = remote_modifier_is_down(next, key);
            if was_down && !now_down {
                self.send_command(SessionCommand::KeyControlState { key, down: false });
            }
        });
        next.for_each(|key, now_down| {
            let was_down = remote_modifier_is_down(previous, key);
            if now_down && !was_down {
                self.send_command(SessionCommand::KeyControlState { key, down: true });
            }
        });
        self.remote_modifiers_down = next;
    }

    fn release_remote_modifiers(&mut self) {
        let previous = self.remote_modifiers_down;
        if previous == RemoteModifierState::default() {
            return;
        }
        previous.for_each(|key, was_down| {
            if was_down {
                self.send_command(SessionCommand::KeyControlState { key, down: false });
            }
        });
        self.remote_modifiers_down = RemoteModifierState::default();
    }

    fn wheel_delta(&mut self, unit: egui::MouseWheelUnit, delta: egui::Vec2) -> Option<(i32, i32)> {
        let scaled = match unit {
            egui::MouseWheelUnit::Point => delta / 40.0,
            egui::MouseWheelUnit::Line => delta,
            egui::MouseWheelUnit::Page => delta * 8.0,
        };
        self.wheel_accum += scaled;
        let x = self.wheel_accum.x.trunc() as i32;
        let y = self.wheel_accum.y.trunc() as i32;
        self.wheel_accum.x -= x as f32;
        self.wheel_accum.y -= y as f32;
        if x == 0 && y == 0 {
            None
        } else {
            Some((x, y))
        }
    }

    fn last_screenshot_age_ms(&self) -> Option<u128> {
        self.last_screenshot_at
            .map(|instant| instant.elapsed().as_millis())
    }

    fn should_refresh_after_move(&mut self) -> bool {
        if self.live_video_active() {
            return false;
        }
        // Throttle move-triggered refreshes to once per 60 ms regardless of auto_refresh.
        // auto_refresh already fires every ~80 ms, so this adds at most one extra request
        // between auto-refresh ticks, keeping the view fresh while dragging.
        if self.screenshot_pending {
            return false;
        }
        let now = Instant::now();
        let should_refresh = self
            .last_move_refresh_at
            .map(|last| now.duration_since(last) >= Duration::from_millis(60))
            .unwrap_or(true);
        if should_refresh {
            self.last_move_refresh_at = Some(now);
        }
        should_refresh
    }

    fn request_visual_refresh_after_input(&mut self) {
        // Always request a fresh screenshot after user input so the screen reflects
        // the action immediately — don't wait for the periodic auto-refresh timer.
        // Skip only if a request is already in flight to avoid flooding the server.
        if !self.screenshot_pending && !self.live_video_active() {
            self.send_command(SessionCommand::Screenshot);
        }
    }

    fn live_video_active(&self) -> bool {
        self.last_frame_codec != "PNG"
            && self
                .last_live_frame_at
                .map(|instant| instant.elapsed() < Duration::from_secs(5))
                .unwrap_or(false)
    }

    fn paste_local_clipboard_to_remote(&mut self) {
        if !self.config.security.allow_clipboard {
            self.clipboard_status = Some("буфер отключен в настройках".to_owned());
            self.log("Clipboard paste blocked: disabled by local security policy".to_owned());
            return;
        }

        match read_local_clipboard_text() {
            Ok(text) if text.trim().is_empty() => {
                self.clipboard_status = Some("буфер пуст".to_owned());
            }
            Ok(text) => {
                let chars = text.chars().count();
                self.send_command(SessionCommand::SetClipboardText(text));
                self.clipboard_status = Some(format!("буфер синхр.: {chars} симв."));
                self.log(format!("Clipboard synchronized to remote: {chars} chars"));
            }
            Err(err) => {
                self.clipboard_status = Some("буфер недоступен".to_owned());
                self.log(format!("Clipboard read failed: {err}"));
            }
        }
    }

    fn save_current_frame_png(&mut self) {
        if self.last_frame_rgba.is_empty() || self.remote_size[0] == 0 || self.remote_size[1] == 0 {
            self.screenshot_status = Some("нет кадра для сохранения".to_owned());
            return;
        }

        match save_rgba_png(
            &self.remote_id,
            self.remote_size[0],
            self.remote_size[1],
            &self.last_frame_rgba,
        ) {
            Ok(path) => {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("screenshot.png");
                self.screenshot_status = Some(format!("сохранено: {name}"));
                self.log(format!("Screenshot saved: {}", path.display()));
            }
            Err(err) => {
                self.screenshot_status = Some("сохранение не удалось".to_owned());
                self.log(format!("Screenshot save failed: {err}"));
            }
        }
    }

    fn save_session_log_file(&mut self) {
        match save_session_log(&self.remote_id, &self.session_log) {
            Ok(path) => {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("session.log");
                self.log_status = Some(format!("лог сохранён: {name}"));
                self.log(format!("Session log saved: {}", path.display()));
            }
            Err(err) => {
                self.log_status = Some("лог не сохранён".to_owned());
                self.log(format!("Session log save failed: {err}"));
            }
        }
    }

    fn save_support_report(&mut self) {
        let report = SupportReport {
            remote_id: self.remote_id.clone(),
            connected: self.connected,
            codec: self.last_frame_codec.clone(),
            fps: self.display_fps,
            input_fps: self.stream_input_fps,
            input_kbps: self.stream_input_kbps,
            latency_ms: self.latency_ms,
            frame_size: self.remote_size,
            frame_bytes: self.frame_bytes,
            queue_ms: self.frame_queue_ms,
            decode_ms: self.frame_decode_ms,
            dropped: self.frame_dropped,
            stream_health: self.stream_health.clone(),
            screenshot_count: self.screenshot_count,
            live_frame_count: self.live_frame_count,
            screenshot_frame_count: self.screenshot_frame_count,
            input_events_sent: self.input_events_sent,
            last_frame_rgba: self.last_frame_rgba.clone(),
            session_log: self.session_log.clone(),
        };

        match save_support_report_bundle(report) {
            Ok(dir) => {
                let name = dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("report");
                self.report_status = Some(format!("отчёт собран: {name}"));
                self.log(format!("Support report saved: {}", dir.display()));
            }
            Err(err) => {
                self.report_status = Some("отчёт не собран".to_owned());
                self.log(format!("Support report save failed: {err}"));
            }
        }
    }

    fn remote_point_from_local(&self, x: f32, y: f32) -> (i32, i32) {
        let frame_w = self.remote_size[0].max(1) as f32;
        let frame_h = self.remote_size[1].max(1) as f32;
        let (target_w, target_h) = self
            .remote_displays
            .iter()
            .find(|display| display.index == self.selected_display)
            .map(|display| (display.width.max(1) as f32, display.height.max(1) as f32))
            .unwrap_or((frame_w, frame_h));
        let x = (x.clamp(0.0, frame_w - 1.0) * (target_w / frame_w))
            .clamp(0.0, target_w - 1.0)
            .round() as i32;
        let y = (y.clamp(0.0, frame_h - 1.0) * (target_h / frame_h))
            .clamp(0.0, target_h - 1.0)
            .round() as i32;
        let (offset_x, offset_y) = self.coordinate_offset();
        (offset_x + x, offset_y + y)
    }

    fn coordinate_offset(&self) -> (i32, i32) {
        let Some(display) = self
            .remote_displays
            .iter()
            .find(|display| display.index == self.selected_display)
        else {
            return (0, 0);
        };

        match self.coordinate_mode {
            CoordinateMode::Local => (0, 0),
            CoordinateMode::Absolute => (display.x, display.y),
            CoordinateMode::Auto => {
                if self.remote_displays.len() > 1 || display.x != 0 || display.y != 0 {
                    (display.x, display.y)
                } else {
                    (0, 0)
                }
            }
        }
    }
}

fn has_command_modifier(modifiers: egui::Modifiers) -> bool {
    modifiers.ctrl || modifiers.alt || modifiers.mac_cmd || modifiers.command
}

fn remote_modifier_is_down(state: RemoteModifierState, key: ControlKey) -> bool {
    match key {
        ControlKey::Alt => state.alt,
        ControlKey::Shift => state.shift,
        ControlKey::Control => state.ctrl,
        ControlKey::Meta => state.meta,
        _ => false,
    }
}

fn command_is_input(command: &SessionCommand) -> bool {
    matches!(
        command,
        SessionCommand::MouseMove { .. }
            | SessionCommand::MouseDown { .. }
            | SessionCommand::MouseUp { .. }
            | SessionCommand::MouseRightDown { .. }
            | SessionCommand::MouseRightUp { .. }
            | SessionCommand::MouseMiddleDown { .. }
            | SessionCommand::MouseMiddleUp { .. }
            | SessionCommand::MouseWheel { .. }
            | SessionCommand::KeyText(_)
            | SessionCommand::KeyControl(_)
            | SessionCommand::KeyControlState { .. }
            | SessionCommand::KeyTextWithModifiers { .. }
            | SessionCommand::KeyControlWithModifiers { .. }
            | SessionCommand::KeyEnter
            | SessionCommand::ShellInput(_)
    )
}

fn display_label(display: &RemoteDisplay) -> String {
    format!(
        "{}: {}x{} @ {},{}",
        display.name,
        display.width.max(0),
        display.height.max(0),
        display.x,
        display.y
    )
}

fn remote_viewer_initial_size(remote_size: [usize; 2]) -> [f32; 2] {
    let [width, height] = remote_size;
    if width == 0 || height == 0 {
        return [1100.0, 760.0];
    }

    let toolbar_and_status = 104.0;
    let max_content = egui::vec2(1280.0, 820.0 - toolbar_and_status);
    let scale = (max_content.x / width as f32)
        .min(max_content.y / height as f32)
        .clamp(0.25, 1.0);
    [
        (width as f32 * scale).clamp(900.0, 1280.0),
        (height as f32 * scale + toolbar_and_status).clamp(620.0, 860.0),
    ]
}

fn remote_icon_button(ui: &mut egui::Ui, icon: &str, tooltip: &str) -> egui::Response {
    remote_icon_button_enabled(ui, true, icon, tooltip)
}

fn remote_icon_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: &str,
    tooltip: &str,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(icon).size(14.0)).min_size(egui::vec2(34.0, 30.0)),
    )
    .on_hover_text(tooltip)
}

fn remote_icon_toggle(
    ui: &mut egui::Ui,
    icon: &str,
    selected: bool,
    tooltip: &str,
) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(icon).size(14.0))
            .selected(selected)
            .min_size(egui::vec2(34.0, 30.0)),
    )
    .on_hover_text(tooltip)
}

fn coordinate_mode_label(mode: CoordinateMode) -> &'static str {
    match mode {
        CoordinateMode::Auto => "Coord: Auto",
        CoordinateMode::Absolute => "Coord: Absolute",
        CoordinateMode::Local => "Coord: Local",
    }
}

fn stream_health_color(text: &str) -> egui::Color32 {
    if text.contains("стабилен") {
        egui::Color32::from_rgb(80, 200, 100)
    } else if text.contains("ожидание") || text.contains("PNG fallback") {
        egui::Color32::from_rgb(220, 180, 60)
    } else {
        egui::Color32::from_rgb(220, 110, 80)
    }
}

/// Цвет точки и порог здоровья по FPS: зелёный/жёлтый/красный.
fn health_dot(fps: f32) -> (egui::Color32, bool) {
    if fps >= 20.0 {
        (egui::Color32::from_rgb(0x32, 0xD8, 0x8C), true)
    } else if fps >= 8.0 {
        (egui::Color32::from_rgb(0xF0, 0xC0, 0x50), false)
    } else {
        (egui::Color32::from_rgb(0xE0, 0x70, 0x70), false)
    }
}

fn remote_texture_options() -> egui::TextureOptions {
    egui::TextureOptions {
        magnification: egui::TextureFilter::Nearest,
        minification: egui::TextureFilter::Linear,
        wrap_mode: egui::TextureWrapMode::ClampToEdge,
        mipmap_mode: None,
    }
}

struct SupportReport {
    remote_id: String,
    connected: bool,
    codec: String,
    fps: f32,
    input_fps: f32,
    input_kbps: u64,
    latency_ms: Option<u32>,
    frame_size: [usize; 2],
    frame_bytes: usize,
    queue_ms: u64,
    decode_ms: u64,
    dropped: usize,
    stream_health: String,
    screenshot_count: u64,
    live_frame_count: u64,
    screenshot_frame_count: u64,
    input_events_sent: u64,
    last_frame_rgba: Vec<u8>,
    session_log: Vec<String>,
}

fn read_local_clipboard_text() -> Result<String, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|err| format!("Clipboard init failed: {err}"))?;
    clipboard
        .get_text()
        .map_err(|err| format!("Clipboard text read failed: {err}"))
}

fn write_local_clipboard_text(text: &str) -> Result<(), String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|err| format!("Clipboard init failed: {err}"))?;
    clipboard
        .set_text(text.to_owned())
        .map_err(|err| format!("Clipboard text write failed: {err}"))
}

fn save_rgba_png(
    remote_id: &str,
    width: usize,
    height: usize,
    rgba: &[u8],
) -> Result<PathBuf, String> {
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "Frame size overflow".to_owned())?;
    if rgba.len() != expected {
        return Err(format!(
            "Frame buffer size mismatch: got {}, expected {expected}",
            rgba.len()
        ));
    }

    let dir = PathBuf::from("screenshots");
    fs::create_dir_all(&dir).map_err(|err| format!("Create screenshots dir failed: {err}"))?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("System time error: {err}"))?
        .as_secs();
    let id = sanitize_filename(remote_id);
    let path = dir.join(format!("evertydesk-{id}-{timestamp}.png"));
    image::save_buffer_with_format(
        &path,
        rgba,
        width as u32,
        height as u32,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|err| format!("PNG save failed: {err}"))?;
    Ok(path)
}

fn save_session_log(remote_id: &str, lines: &[String]) -> Result<PathBuf, String> {
    let _ = diagnostics::cleanup_session_logs();
    let dir = PathBuf::from("logs");
    fs::create_dir_all(&dir).map_err(|err| format!("Create logs dir failed: {err}"))?;
    let timestamp = unix_timestamp_secs();
    let id = sanitize_filename(remote_id);
    let path = dir.join(format!("evertydesk-{id}-{timestamp}.log"));
    let mut body = String::new();
    body.push_str("EvertyDesk Lite session log\n");
    body.push_str(&format!("remote_id={remote_id}\n"));
    body.push_str(&format!("created_at_unix={timestamp}\n\n"));
    for line in lines {
        body.push_str(line);
        body.push('\n');
    }
    fs::write(&path, body).map_err(|err| format!("Write log failed: {err}"))?;
    let _ = diagnostics::cleanup_session_logs();
    Ok(path)
}

fn save_support_report_bundle(report: SupportReport) -> Result<PathBuf, String> {
    let _ = diagnostics::cleanup_support_reports();
    let timestamp = unix_timestamp_secs();
    let id = sanitize_filename(&report.remote_id);
    let dir = PathBuf::from("reports").join(format!("evertydesk-{id}-{timestamp}"));
    fs::create_dir_all(&dir).map_err(|err| format!("Create report dir failed: {err}"))?;

    let summary_path = dir.join("summary.txt");
    fs::write(&summary_path, support_report_summary(&report, timestamp))
        .map_err(|err| format!("Write report summary failed: {err}"))?;

    let log_path = dir.join("session.log");
    let mut log_body = String::new();
    log_body.push_str("EvertyDesk Lite session log\n\n");
    for line in &report.session_log {
        log_body.push_str(line);
        log_body.push('\n');
    }
    fs::write(&log_path, log_body).map_err(|err| format!("Write report log failed: {err}"))?;

    if !report.last_frame_rgba.is_empty() && report.frame_size[0] > 0 && report.frame_size[1] > 0 {
        let shot_path = dir.join("screen.png");
        image::save_buffer_with_format(
            &shot_path,
            &report.last_frame_rgba,
            report.frame_size[0] as u32,
            report.frame_size[1] as u32,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|err| format!("Write report screen failed: {err}"))?;
    }

    let _ = diagnostics::cleanup_support_reports();
    Ok(dir)
}

fn support_report_summary(report: &SupportReport, timestamp: u64) -> String {
    let latency = report
        .latency_ms
        .map(|ms| ms.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    format!(
        "EvertyDesk Lite support report\n\
         created_at_unix={timestamp}\n\
         remote_id={}\n\
         connected={}\n\
         codec={}\n\
         fps={:.1}\n\
         input_fps={:.1}\n\
         input_kbps={}\n\
         latency_ms={latency}\n\
         frame={}x{}\n\
         frame_bytes={}\n\
         queue_ms={}\n\
         decode_ms={}\n\
         dropped={}\n\
         stream_health={}\n\
         screenshot_count={}\n\
         live_frame_count={}\n\
         screenshot_frame_count={}\n\
         input_events_sent={}\n",
        report.remote_id,
        report.connected,
        report.codec,
        report.fps,
        report.input_fps,
        report.input_kbps,
        report.frame_size[0],
        report.frame_size[1],
        report.frame_bytes,
        report.queue_ms,
        report.decode_ms,
        report.dropped,
        report.stream_health,
        report.screenshot_count,
        report.live_frame_count,
        report.screenshot_frame_count,
        report.input_events_sent
    )
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// "HH:MM:SS" wall-clock for log lines.
fn timestamp_hms() -> String {
    let secs = unix_timestamp_secs();
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

fn compact_host_video_status(summary: &str) -> String {
    let mut parts = Vec::new();
    for key in [
        "active_backend",
        "codec",
        "size",
        "fps",
        "bitrate",
        "bitrate_min",
        "bitrate_max",
        "roi_avg",
        "roi_max",
        "relief",
        "relief_min",
        "packets",
        "avg_packet",
        "capture_avg",
        "capture_max",
        "change_avg",
        "encode_avg",
        "encode_max",
        "send_avg",
        "fallbacks",
        "reason",
    ] {
        if let Some(value) = telemetry_value(summary, key) {
            parts.push(format!("{key}={value}"));
        }
    }
    if parts.is_empty() {
        summary.chars().take(160).collect()
    } else {
        parts.join(" ")
    }
}

fn telemetry_value<'a>(summary: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=");
    let start = summary.find(&needle)? + needle.len();
    let rest = &summary[start..];
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(&stripped[..end])
    } else {
        let end = rest.find(' ').unwrap_or(rest.len());
        Some(&rest[..end])
    }
}

/// Format 9-digit peer ID as "XXX XXX XXX" (like RustDesk).
fn format_peer_id(id: &str) -> String {
    let digits: String = id.chars().filter(|c| c.is_ascii_digit()).collect();
    match digits.len() {
        9 => format!("{} {} {}", &digits[0..3], &digits[3..6], &digits[6..9]),
        _ => id.to_owned(),
    }
}

fn sanitize_filename(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "remote".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn egui_key_to_control_key(key: egui::Key) -> Option<ControlKey> {
    Some(match key {
        egui::Key::ArrowDown => ControlKey::DownArrow,
        egui::Key::ArrowLeft => ControlKey::LeftArrow,
        egui::Key::ArrowRight => ControlKey::RightArrow,
        egui::Key::ArrowUp => ControlKey::UpArrow,
        egui::Key::Escape => ControlKey::Escape,
        egui::Key::Tab => ControlKey::Tab,
        egui::Key::Backspace => ControlKey::Backspace,
        egui::Key::Enter => ControlKey::Return,
        egui::Key::Space => ControlKey::Space,
        egui::Key::Insert => ControlKey::Insert,
        egui::Key::Delete => ControlKey::Delete,
        egui::Key::Home => ControlKey::Home,
        egui::Key::End => ControlKey::End,
        egui::Key::PageUp => ControlKey::PageUp,
        egui::Key::PageDown => ControlKey::PageDown,
        egui::Key::F1 => ControlKey::F1,
        egui::Key::F2 => ControlKey::F2,
        egui::Key::F3 => ControlKey::F3,
        egui::Key::F4 => ControlKey::F4,
        egui::Key::F5 => ControlKey::F5,
        egui::Key::F6 => ControlKey::F6,
        egui::Key::F7 => ControlKey::F7,
        egui::Key::F8 => ControlKey::F8,
        egui::Key::F9 => ControlKey::F9,
        egui::Key::F10 => ControlKey::F10,
        egui::Key::F11 => ControlKey::F11,
        egui::Key::F12 => ControlKey::F12,
        _ => return None,
    })
}

/// Keys that should fire repeatedly when held down (navigation, delete, etc.)
fn key_allows_repeat(key: egui::Key) -> bool {
    matches!(
        key,
        egui::Key::ArrowDown
            | egui::Key::ArrowLeft
            | egui::Key::ArrowRight
            | egui::Key::ArrowUp
            | egui::Key::Backspace
            | egui::Key::Delete
            | egui::Key::PageDown
            | egui::Key::PageUp
            | egui::Key::Home
            | egui::Key::End
            | egui::Key::Tab
    )
}

fn egui_key_to_text(key: egui::Key) -> Option<String> {
    let ch = match key {
        egui::Key::A => 'a',
        egui::Key::B => 'b',
        egui::Key::C => 'c',
        egui::Key::D => 'd',
        egui::Key::E => 'e',
        egui::Key::F => 'f',
        egui::Key::G => 'g',
        egui::Key::H => 'h',
        egui::Key::I => 'i',
        egui::Key::J => 'j',
        egui::Key::K => 'k',
        egui::Key::L => 'l',
        egui::Key::M => 'm',
        egui::Key::N => 'n',
        egui::Key::O => 'o',
        egui::Key::P => 'p',
        egui::Key::Q => 'q',
        egui::Key::R => 'r',
        egui::Key::S => 's',
        egui::Key::T => 't',
        egui::Key::U => 'u',
        egui::Key::V => 'v',
        egui::Key::W => 'w',
        egui::Key::X => 'x',
        egui::Key::Y => 'y',
        egui::Key::Z => 'z',
        egui::Key::Num0 => '0',
        egui::Key::Num1 => '1',
        egui::Key::Num2 => '2',
        egui::Key::Num3 => '3',
        egui::Key::Num4 => '4',
        egui::Key::Num5 => '5',
        egui::Key::Num6 => '6',
        egui::Key::Num7 => '7',
        egui::Key::Num8 => '8',
        egui::Key::Num9 => '9',
        _ => return None,
    };
    Some(ch.to_string())
}

fn configure_ui_scale(ctx: &egui::Context) {
    let default_zoom = if cfg!(target_os = "macos") { 1.08 } else { 1.0 };
    let zoom = std::env::var("EVERTYDESK_UI_SCALE")
        .ok()
        .and_then(|value| value.trim().parse::<f32>().ok())
        .unwrap_or(default_zoom)
        .clamp(0.75, 1.75);
    ctx.set_zoom_factor(zoom);
}

fn configure_style(ctx: &egui::Context) {
    use egui::{Color32, CornerRadius, Stroke};

    let bg = Color32::from_rgb(0xFB, 0xFC, 0xFE);
    let panel = Color32::from_rgb(0xFF, 0xFF, 0xFF);
    let card = Color32::from_rgb(0xFF, 0xFF, 0xFF);
    let input = Color32::from_rgb(0xFF, 0xFF, 0xFF);
    let border = Color32::from_rgb(0xE3, 0xE6, 0xEC);
    let border_hover = Color32::from_rgb(0xD8, 0xDC, 0xE4);
    let border_focus = Color32::from_rgb(0xC7, 0xCD, 0xD8);
    let text = Color32::from_rgb(0x13, 0x17, 0x21);
    let text_weak = Color32::from_rgb(0x67, 0x70, 0x80);
    let text_strong = Color32::from_rgb(0x22, 0x27, 0x32);

    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = bg;
    visuals.window_fill = panel;
    visuals.extreme_bg_color = input;
    visuals.faint_bg_color = card;
    visuals.window_stroke = Stroke::new(1.0, border);

    visuals.selection.bg_fill = Color32::from_rgb(0xB9, 0xD7, 0xFF);
    visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(0x3D, 0x73, 0xB8));

    let rounding = CornerRadius::same(12);
    visuals.widgets.noninteractive.bg_fill = card;
    visuals.widgets.noninteractive.weak_bg_fill = card;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text_weak);
    visuals.widgets.noninteractive.corner_radius = rounding;

    visuals.widgets.inactive.bg_fill = input;
    visuals.widgets.inactive.weak_bg_fill = input;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, border);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text_strong);
    visuals.widgets.inactive.corner_radius = rounding;

    visuals.widgets.hovered.bg_fill = Color32::from_rgb(0xFA, 0xFB, 0xFD);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(0xFA, 0xFB, 0xFD);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, border_hover);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, text);
    visuals.widgets.hovered.corner_radius = rounding;

    visuals.widgets.active.bg_fill = Color32::from_rgb(0xF3, 0xF5, 0xF8);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(0xF3, 0xF5, 0xF8);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, border_focus);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, text);
    visuals.widgets.active.corner_radius = rounding;

    visuals.widgets.open.bg_fill = card;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, border);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, text);
    visuals.widgets.open.corner_radius = rounding;

    ctx.set_visuals(visuals);

    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(12.0, 9.0);
    style.spacing.button_padding = egui::vec2(16.0, 9.0);
    style.spacing.menu_margin = egui::Margin::same(8);
    style.spacing.window_margin = egui::Margin::same(16);
    style.spacing.interact_size.y = 34.0;
    ctx.set_global_style(style);
}

#[allow(dead_code)]
const SERVICE_NAME: &str = "EvertyDeskLite";

fn install_host_service() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|err| format!("current_exe: {err}"))?;
    #[cfg(target_os = "windows")]
    {
        let bin = format!("\"{}\" --host", exe.display());
        let status = std::process::Command::new("sc.exe")
            .args(["create", SERVICE_NAME, "binPath=", &bin, "start=", "auto"])
            .status()
            .map_err(|err| format!("sc create failed: {err}"))?;
        return if status.success() {
            Ok("Windows service installed. Admin rights may be required.".to_owned())
        } else {
            Err(format!("sc create exited with {status}"))
        };
    }
    #[cfg(target_os = "linux")]
    {
        let dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_owned())?
            .join(".config/systemd/user");
        fs::create_dir_all(&dir).map_err(|err| format!("create systemd dir: {err}"))?;
        let unit = dir.join("evertydesk-lite.service");
        let body = format!(
            "[Unit]\nDescription=EvertyDesk Lite host service\nAfter=network-online.target\n\n\
             [Service]\nExecStart={} --host\nRestart=always\nRestartSec=5\n\n\
             [Install]\nWantedBy=default.target\n",
            exe.display()
        );
        fs::write(&unit, body).map_err(|err| format!("write service file: {err}"))?;
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "enable", "evertydesk-lite.service"])
            .status();
        return Ok(format!(
            "User systemd service installed: {}",
            unit.display()
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_owned())?
            .join("Library/LaunchAgents");
        fs::create_dir_all(&dir).map_err(|err| format!("create LaunchAgents dir: {err}"))?;
        let plist = dir.join("ru.everty.evertydesk-lite.plist");
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>ru.everty.evertydesk-lite</string>
<key>ProgramArguments</key><array><string>{}</string><string>--host</string></array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
</dict></plist>"#,
            exe.display()
        );
        fs::write(&plist, body).map_err(|err| format!("write plist: {err}"))?;
        return Ok(format!("LaunchAgent installed: {}", plist.display()));
    }
    #[allow(unreachable_code)]
    Err("Service install is not implemented for this OS".to_owned())
}

fn start_installed_service() -> Result<String, String> {
    run_service_command("start")
}

fn stop_installed_service() -> Result<String, String> {
    run_service_command("stop")
}

fn uninstall_host_service() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("sc.exe")
            .args(["stop", SERVICE_NAME])
            .status();
        let status = std::process::Command::new("sc.exe")
            .args(["delete", SERVICE_NAME])
            .status()
            .map_err(|err| format!("sc delete failed: {err}"))?;
        return if status.success() {
            Ok("Windows service removed.".to_owned())
        } else {
            Err(format!("sc delete exited with {status}"))
        };
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "disable", "--now", "evertydesk-lite.service"])
            .status();
        let unit = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_owned())?
            .join(".config/systemd/user/evertydesk-lite.service");
        let _ = fs::remove_file(&unit);
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        return Ok("User systemd service removed.".to_owned());
    }
    #[cfg(target_os = "macos")]
    {
        let plist = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_owned())?
            .join("Library/LaunchAgents/ru.everty.evertydesk-lite.plist");
        let _ = std::process::Command::new("launchctl")
            .args(["unload", plist.to_string_lossy().as_ref()])
            .status();
        let _ = fs::remove_file(&plist);
        return Ok("LaunchAgent removed.".to_owned());
    }
    #[allow(unreachable_code)]
    Err("Service uninstall is not implemented for this OS".to_owned())
}

fn run_service_command(action: &str) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        let status = std::process::Command::new("sc.exe")
            .args([action, SERVICE_NAME])
            .status()
            .map_err(|err| format!("sc {action} failed: {err}"))?;
        return if status.success() {
            Ok(format!("Windows service {action} requested."))
        } else {
            Err(format!("sc {action} exited with {status}"))
        };
    }
    #[cfg(target_os = "linux")]
    {
        let status = std::process::Command::new("systemctl")
            .args(["--user", action, "evertydesk-lite.service"])
            .status()
            .map_err(|err| format!("systemctl {action} failed: {err}"))?;
        return if status.success() {
            Ok(format!("systemd user service {action} requested."))
        } else {
            Err(format!("systemctl exited with {status}"))
        };
    }
    #[cfg(target_os = "macos")]
    {
        let plist = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_owned())?
            .join("Library/LaunchAgents/ru.everty.evertydesk-lite.plist");
        let cmd = if action == "start" { "load" } else { "unload" };
        let status = std::process::Command::new("launchctl")
            .args([cmd, plist.to_string_lossy().as_ref()])
            .status()
            .map_err(|err| format!("launchctl {cmd} failed: {err}"))?;
        return if status.success() {
            Ok(format!("LaunchAgent {cmd} requested."))
        } else {
            Err(format!("launchctl exited with {status}"))
        };
    }
    #[allow(unreachable_code)]
    Err("Service control is not implemented for this OS".to_owned())
}

/// A `label … value` row for the This Computer info block.
#[allow(dead_code)]
fn info_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .size(13.0)
                .color(egui::Color32::from_rgb(0x80, 0x80, 0x80)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(value)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(0xCC, 0xCC, 0xCC)),
            );
        });
    });
    ui.add_space(9.0);
}

/// Try to read the 32-byte Ed25519 public key from the installed EvertyDesk
/// (or RustDesk) client config.  Returns `None` if the config is not found
/// or cannot be parsed.
///
/// Priority:
/// 1. `%APPDATA%\EvertyDesk\config\Evertydesk.toml`
/// 2. `%APPDATA%\RustDesk\config\RustDesk.toml`
fn load_everty_public_key() -> Option<Vec<u8>> {
    let appdata = std::env::var_os("APPDATA")?;
    let appdata = std::path::PathBuf::from(appdata);

    let candidates = [
        appdata
            .join("EvertyDesk")
            .join("config")
            .join("Evertydesk.toml"),
        appdata
            .join("RustDesk")
            .join("config")
            .join("RustDesk.toml"),
    ];

    for path in &candidates {
        if let Ok(raw) = std::fs::read_to_string(path) {
            // key_pair in TOML is two arrays of integers.
            // key_pair[1] is the 32-byte public key.
            if let Some(pk) = parse_key_pair_public_key(&raw) {
                eprintln!("[cli] Loaded key from {}", path.display());
                return Some(pk);
            }
        }
    }
    None
}

/// Parses the `key_pair` from a RustDesk/EvertyDesk TOML config string and
/// returns the 32-byte public key (the second inner array).
fn parse_key_pair_public_key(toml_text: &str) -> Option<Vec<u8>> {
    // The key_pair field is written as two arrays-of-integers.
    // We use a simple regex-free approach: find each bracketed number list.
    let kp_start = toml_text.find("key_pair")?;
    let text = &toml_text[kp_start..];

    // Find all outer `[` brackets (each sub-array)
    let mut arrays: Vec<Vec<u8>> = Vec::new();
    let mut depth = 0i32;
    let mut cur: Vec<u8> = Vec::new();
    let mut num_buf = String::new();

    for ch in text.chars() {
        match ch {
            '[' => {
                depth += 1;
                if depth == 2 {
                    cur.clear();
                }
            }
            ']' => {
                if depth == 2 {
                    // flush last number
                    if !num_buf.is_empty() {
                        if let Ok(n) = num_buf.trim().parse::<u8>() {
                            cur.push(n);
                        }
                        num_buf.clear();
                    }
                    arrays.push(cur.clone());
                }
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            ',' if depth == 2 => {
                if !num_buf.is_empty() {
                    if let Ok(n) = num_buf.trim().parse::<u8>() {
                        cur.push(n);
                    }
                    num_buf.clear();
                }
            }
            c if (c.is_ascii_digit() || c == '-') && depth == 2 => {
                num_buf.push(c);
            }
            _ if depth == 2 => {
                // whitespace/newline — ignore
                if !num_buf.is_empty() {
                    if let Ok(n) = num_buf.trim().parse::<u8>() {
                        cur.push(n);
                    }
                    num_buf.clear();
                }
            }
            _ => {}
        }
    }

    // key_pair = [ [64 bytes private+pub], [32 bytes public] ]
    // We want the second array (index 1), length must be 32.
    arrays.into_iter().find(|a| a.len() == 32)
}

fn normalize_remote_id(id: &str) -> String {
    id.chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-' && *ch != '_')
        .collect()
}

fn is_own_remote_id(remote_id: &str, local_id: &str) -> bool {
    let remote_id = normalize_remote_id(remote_id);
    let local_id = normalize_remote_id(local_id);
    !remote_id.is_empty() && remote_id == local_id
}

fn remember_remote_id(recent: &mut Vec<String>, id: &str) {
    let id = normalize_remote_id(id);
    if id.is_empty() {
        return;
    }
    recent.retain(|existing| existing != &id);
    recent.insert(0, id);
    recent.truncate(8);
}

fn remember_history(history: &mut Vec<ConnectionHistoryEntry>, id: &str) {
    let id = normalize_remote_id(id);
    if id.is_empty() {
        return;
    }
    let now = unix_timestamp_secs();
    if let Some(entry) = history.iter_mut().find(|entry| entry.remote_id == id) {
        entry.last_connected_unix = now;
        entry.connect_count = entry.connect_count.saturating_add(1);
    } else {
        history.insert(
            0,
            ConnectionHistoryEntry {
                remote_id: id,
                note: String::new(),
                last_connected_unix: now,
                connect_count: 1,
            },
        );
    }
    history.sort_by(|a, b| b.last_connected_unix.cmp(&a.last_connected_unix));
    history.truncate(20);
}

fn friendly_error(error: &str) -> String {
    if error.contains("Wrong Password") {
        "Неверный пароль. Проверьте пароль на удаленном ПК.".to_owned()
    } else if error.contains("Offline:") || error.contains("Rendezvous refused: Offline") {
        "Удаленный ID сейчас не в сети.".to_owned()
    } else if error.contains("ID does not exist") {
        "Такой ID не найден на сервере.".to_owned()
    } else if error.contains("Введите ID") || error.contains("Введите пароль") {
        error.to_owned()
    } else if error.contains("Background task stopped unexpectedly") {
        "Соединение неожиданно остановилось.".to_owned()
    } else {
        error.to_owned()
    }
}
