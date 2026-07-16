#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod address_book;
mod capability_engine;
mod theme;
mod libvirt_provider;
mod proxmox_provider;
mod provider_api;
mod session_backend;
mod smart_connect;
mod vmware_provider;
mod capture;
mod colorconv;
mod crypto;
mod diagnostics;
mod evrt;
mod evrt_audio;
mod evrtck;
#[cfg(feature = "gpu-accel")]
mod evrtck_wgpu;
mod evrt_client;
mod evrt_session;
mod frame_queue;
mod fsr;
mod host;
#[cfg(windows)]
mod hyperv;
#[cfg(windows)]
mod hyperv_rdp;
mod lan_discovery;
mod hotfix;
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
mod vbox_rdp;
mod virtualbox;
mod vm_bridge;
mod vp9_mf;
#[cfg(feature = "live-vpx-system")]
mod vpx_system;

use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
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
    HyperV,
    History,
    Contacts,
    Settings,
}

/// VM на удалённом хосте-гипервизоре (получена через agentless control-plane).
#[derive(Clone, Debug)]
struct RemoteVmEntry {
    id: String,
    name: String,
    state: String,
    connectable: bool,
    /// Capability graph received from host (None until requested).
    capability_graph: Option<capability_engine::VmCapabilityGraph>,
    /// Raw checkpoints JSON from last list operation.
    checkpoints_json: Option<String>,
}

// ── Кэш снимка дашборда провайдеров ──────────────────────────────────────────
// Раньше дашборд вызывал list_hosts/list_vms/get_capabilities КАЖДЫЙ кадр —
// для Hyper-V это синхронный WMI-скан 60 раз/сек → лютый фриз. Теперь снимок
// собирается раз в несколько секунд и рендерится из кэша.
struct DashVm {
    name: String,
    power_state: provider_api::PowerState,
    /// (ярлык режима, цвет) — из recommended_mode capability-графа.
    badge: Option<(String, (u8, u8, u8))>,
    ip: Option<String>,
}
struct DashProvider {
    ptype: provider_api::ProviderType,
    pid: String,
    reachable: bool,
    vms: Vec<DashVm>,
}
struct DashSnapshot {
    at: std::time::Instant,
    providers: Vec<DashProvider>,
    total_vms: usize,
}

/// Метка+цвет чипа провайдера по префиксу id ("hyperv:" / "vbox:").
fn vm_provider_badge(id: &str) -> (&'static str, egui::Color32) {
    if id.starts_with("vbox:") {
        ("VIRTUALBOX", egui::Color32::from_rgb(0xE8, 0x8A, 0x2E))
    } else {
        ("HYPER-V", crate::theme::palette().info)
    }
}

/// Распарсить JSON-список VM от хоста: `[{"id","name","state","connectable"}]`.
fn parse_remote_vms(json: &str) -> Vec<RemoteVmEntry> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(arr) = value.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|row| {
            Some(RemoteVmEntry {
                id: row.get("id")?.as_str()?.to_owned(),
                name: row
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("VM")
                    .to_owned(),
                state: row
                    .get("state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned(),
                connectable: row
                    .get("connectable")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                capability_graph: None,
                checkpoints_json: None,
            })
        })
        .collect()
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

/// Глобальный логгер паник: пишет место + сообщение + бэктрейс в файл рядом
/// с конфигом (evertydesk_panic.log) и в stderr. Сохраняет родной хук, чтобы
/// стандартный вывод тоже остался. Помогает диагностировать «выкидывает» без
/// отладочной сборки у пользователя.
fn install_panic_logger() {
    // Load config once so the hotfix report has correct api_key / device_id.
    let hotfix_cfg = AppConfig::load_or_create();

    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<неизвестно>".to_owned());
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_owned()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<непечатаемая paника>".to_owned()
        };
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_owned();
        let backtrace = std::backtrace::Backtrace::force_capture();
        let record = format!(
            "\n===== PANIC =====\nthread: {thread}\nat: {location}\nmessage: {msg}\nbacktrace:\n{backtrace}\n=================\n"
        );
        eprintln!("{record}");
        // Пишем рядом с конфигом, чтобы пользователь легко нашёл файл.
        let path = settings::config_path()
            .parent()
            .map(|p| p.join("evertydesk_panic.log"))
            .unwrap_or_else(|| std::path::PathBuf::from("evertydesk_panic.log"));
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{record}");
        }

        // Синхронная отправка — блокируемся максимум 5 сек прямо здесь,
        // чтобы успеть отправить POST до завершения процесса.
        hotfix::submit_crash_sync(
            format!("{}:{}", location, &msg[..msg.len().min(120)]),
            "core".to_owned(),
            "PANIC".to_owned(),
            msg.clone(),
            format!("{backtrace}"),
            &hotfix_cfg.hotfix,
            &hotfix_cfg,
        );

        prev(info);
    }));
}

fn main() -> eframe::Result<()> {
    install_panic_logger();
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
            configure_icon_fonts(&cc.egui_ctx);
            // Тема из конфига (тёмная по умолчанию).
            let theme_mode = settings::AppConfig::load_or_create().ui.theme_mode;
            theme::apply(&cc.egui_ctx, theme_mode);
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
    /// When the last error was set — used to auto-clear it after 8s.
    last_error_at: Option<Instant>,
    /// Auto-reconnect after a dropped (not auth-failure) session.
    auto_reconnect: bool,
    /// Scheduled instant to call connect() again (None = no pending reconnect).
    reconnect_after: Option<Instant>,
    /// When the toolbar was last hovered/triggered (for fullscreen auto-hide).
    toolbar_last_active: Instant,
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
    /// When true, all input (mouse+keyboard) is suppressed — view-only mode.
    view_only: bool,
    /// Shows the keyboard shortcut reference popup.
    show_shortcuts_help: bool,
    remote_modifiers_down: RemoteModifierState,
    last_mouse_pos: Option<(i32, i32)>,
    /// True between a mouse-press that landed *inside* the remote screen and its
    /// release. Mouse events are read from the global egui event stream, so a
    /// click on a toolbar/status-bar button also shows up here; without this
    /// gate a release outside the screen would still fire a MouseUp into the
    /// guest at the last in-screen position (which manifested as "clicking
    /// Детали presses Start in the guest"). Press outside → ignored; release is
    /// only forwarded if the press was armed inside, which still lets a real
    /// drag that ends off-screen complete correctly.
    remote_pointer_armed: bool,
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
    /// Stop signal for the LAN discovery thread (None = not running).
    lan_discovery_stop: Option<Arc<AtomicBool>>,

    // ── Hyper-V console ───────────────────────────────────────────────────────
    #[cfg(windows)]
    hyperv_vms: Vec<hyperv::VmInfo>,
    #[cfg(windows)]
    hyperv_session: Option<hyperv::HyperVSession>,
    #[cfg(windows)]
    vbox_session: Option<virtualbox::VboxSession>,
    /// Поисковый запрос в списке VM.
    #[cfg(windows)]
    vm_search: String,
    /// Режим полного экрана консоли (скрывает сайдбар).
    #[cfg(windows)]
    vm_console_fullscreen: bool,
    /// VM console receives pointer/keyboard only after an explicit click inside its canvas.
    #[cfg(windows)]
    vm_console_input_active: bool,
    #[cfg(windows)]
    vm_console_last_pointer_pos: Option<(u16, u16)>,
    #[cfg(windows)]
    hyperv_texture: Option<TextureHandle>,
    #[cfg(windows)]
    hyperv_guest_size: Option<(u32, u32)>,
    #[cfg(windows)]
    hyperv_status: String,
    /// Which VM is currently open in the console view (index into hyperv_vms)
    #[cfg(windows)]
    hyperv_console_vm: Option<usize>,
    /// Background VM list load channel
    #[cfg(windows)]
    hyperv_load_rx: Option<mpsc::Receiver<Vec<hyperv::VmInfo>>>,
    #[cfg(windows)]
    hyperv_loading: bool,
    /// True after the first scan finished (even if empty) — prevents infinite retry
    #[cfg(windows)]
    hyperv_checked: bool,
    /// #1 FPS counter: frames received in current 1-second window
    #[cfg(windows)]
    hyperv_frame_count: u32,
    #[cfg(windows)]
    hyperv_fps_window: std::time::Instant,
    /// Smoothed fps shown in UI
    #[cfg(windows)]
    hyperv_fps_display: f32,
    /// #7 Auto-reconnect: instant of last received frame
    #[cfg(windows)]
    hyperv_last_frame: std::time::Instant,
    /// #10 Last VM list refresh timestamp
    #[cfg(windows)]
    hyperv_last_refresh: Option<std::time::Instant>,
    /// Active Enhanced Session RDP-over-VMBus connection (for VMs with IS running).
    #[cfg(windows)]
    hyperv_rdp_session: Option<hyperv_rdp::RdpSession>,
    /// Guest credentials for Hyper-V Enhanced Session (editable via the gear).
    #[cfg(windows)]
    hyperv_rdp_creds: hyperv_rdp::RdpCredentials,
    /// VRDE-порты VirtualBox VM: uuid → активный порт. Populated when user enables VRDE.
    #[cfg(windows)]
    vbox_vrde_ports: std::collections::HashMap<String, u16>,
    /// Embedded VRDE RDP session (VirtualBox 30fps in-app).
    #[cfg(windows)]
    vbox_vrde_session: Option<vbox_rdp::VrdeSession>,
    #[cfg(windows)]
    vbox_vrde_last_desktop_size: Option<(u16, u16)>,
    /// Pending async VRDE enable request: (vm_uuid, port, enabled).
    #[cfg(windows)]
    vbox_vrde_enable_rx: Option<mpsc::Receiver<(String, u16, bool)>>,
    /// Async pointing-device (PS/2 vs USB Tablet) advisory — populated separately
    /// so the `showvminfo` probe never delays the actual VRDE connect (a blocking
    /// pre-check here previously added enough latency that users re-clicked
    /// "connect" before the first session settled, triggering VirtualBox's
    /// one-client-at-a-time VRDE policy to kick the prior session repeatedly —
    /// the perceived "black screen").
    #[cfg(windows)]
    vbox_mouse_warning_rx: Option<mpsc::Receiver<String>>,
    /// Last surfaced mouse-device advisory for the active VirtualBox console.
    #[cfg(windows)]
    vbox_mouse_warning: Option<String>,
    /// Port of the currently-live `vbox_vrde_session`, if any. Several UI buttons
    /// can all trigger "(re)connect to this VM's VRDE" — without this guard each
    /// click tore down a working session and opened a new one, and since
    /// VirtualBox VRDE only accepts one client at a time, the brand-new
    /// connection kicked the still-settling previous one off mid-handshake,
    /// turning repeated clicks into a permanent black screen.
    #[cfg(windows)]
    vbox_vrde_active_port: Option<u16>,
    /// User-adjustable connection settings (color depth, compression),
    /// shown via the gear button on the VM console toolbar. Applied on the
    /// *next* connect — changing them mid-session doesn't retroactively
    /// alter an already-negotiated connection.
    #[cfg(windows)]
    vbox_vrde_settings: vbox_rdp::VrdeSettings,
    /// Whether the settings popover is currently open.
    #[cfg(windows)]
    vbox_vrde_settings_open: bool,
    /// Instant of the last frame actually applied to the texture. VirtualBox's
    /// VRDE bulk-compression decoder occasionally desyncs mid-session (server
    /// keeps the TCP connection open but stops sending anything new) — auto-
    /// reconnect is the practical mitigation since this is a third-party
    /// (ironrdp + VirtualBox VRDE) protocol-compatibility quirk, not something
    /// fixable from the client side alone.
    #[cfg(windows)]
    vbox_vrde_last_frame: std::time::Instant,
    /// Instant of the last input command actually sent into the VRDE session.
    /// Used to distinguish "legitimately idle, nothing to update" (no frame
    /// AND no input) from "stuck" (user is actively moving the mouse/typing
    /// but no frame ever comes back) — only the latter should auto-reconnect.
    #[cfg(windows)]
    vbox_vrde_last_input: Option<std::time::Instant>,
    /// Background scan of already-enabled VirtualBox VRDE ports.
    #[cfg(windows)]
    vbox_vrde_scan_rx: Option<mpsc::Receiver<std::collections::HashMap<String, u16>>>,

    // ── Remote agentless VM (через подключённый хост-гипервизор) ───────────────
    /// Список VM, полученный от удалённого хоста (киллер-фича).
    remote_vms: Vec<RemoteVmEntry>,
    /// id VM, к которой сейчас прикреплён удалённый сеанс (пусто = экран хоста).
    remote_attached_vm: String,
    /// Статус удалённой VM-сессии (от хоста).
    remote_vm_status: String,
    /// Открыта ли панель VM в окне удалённого сеанса.
    remote_vm_panel_open: bool,

    // ── Universal Provider Registry ───────────────────────────────────────────
    /// Runtime registry of all connected hypervisor providers.
    /// Hyper-V (local) + FakeProviders registered at startup for multi-provider demo.
    provider_registry: std::sync::Arc<provider_api::ProviderRegistry>,
    /// Кэш снимка дашборда — чтобы не вызывать провайдеры (WMI/VBoxManage)
    /// на каждом кадре. Пересобирается раз в несколько секунд в фоновом потоке.
    dashboard_snapshot: Option<DashSnapshot>,
    /// Канал от фонового rebuild дашборда — None когда rebuild не запущен.
    #[cfg(windows)]
    dashboard_load_rx: Option<mpsc::Receiver<DashSnapshot>>,

    // ── Remote VM control panels ──────────────────────────────────────────────
    /// VM id для которой открыта панель управления (power / checkpoint / rescue).
    remote_ctrl_vm_id: String,
    /// Открыта ли панель power operations.
    remote_power_panel_open: bool,
    /// Открыта ли панель checkpoints.
    remote_checkpoint_panel_open: bool,
    /// Открыта ли панель rescue input.
    remote_rescue_panel_open: bool,
    /// Буфер ввода текста для TypeText в BasicRescue.
    remote_rescue_text: String,
    /// Ожидающее подтверждения опасное действие питания VM:
    /// (vm_id, action, vm_name, человекочитаемое описание). Показывает диалог.
    pending_vm_action: Option<(String, String, String, String)>,

    // ── Settings window ───────────────────────────────────────────────────────
    /// Whether the settings panel is visible.
    show_settings: bool,
    /// Editable copy of config while the settings window is open.
    settings_draft: Option<AppConfig>,
    /// Temporary buffer for custom network inputs (never pre-filled with default server values).
    settings_custom_server: crate::settings::ServerConfig,

    // ── FSR (клиентская сторона) ──────────────────────────────────────────────
    /// Адаптер FSR — апскейлит входящий видео-поток перед отображением.
    /// `None` = FSR выключен (нативное разрешение от хоста).
    fsr_viewer: Option<crate::fsr::FsrAdapter>,
    /// Нативное разрешение хоста (объявляется в PeerInfo / SessionEvent::Displays).
    /// FSR апскейлит каждый входящий кадр до этого разрешения.
    fsr_native_size: Option<(u32, u32)>,

    // ── AI Hotfix ─────────────────────────────────────────────────────────────
    hotfix_state: Arc<Mutex<hotfix::HotfixState>>,
    /// Pending consent dialog from hotfix tick (shown as egui window).
    hotfix_consent: Option<hotfix::ConsentRequest>,
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
            last_error_at: None,
            auto_reconnect: true,
            reconnect_after: None,
            toolbar_last_active: Instant::now(),
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
            view_only: false,
            show_shortcuts_help: false,
            remote_modifiers_down: RemoteModifierState::default(),
            last_mouse_pos: None,
            remote_pointer_armed: false,
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
            lan_discovery_stop: None,
            #[cfg(windows)]
            hyperv_vms: Vec::new(),
            #[cfg(windows)]
            hyperv_session: None,
            #[cfg(windows)]
            vbox_session: None,
            #[cfg(windows)]
            vm_search: String::new(),
            #[cfg(windows)]
            vm_console_fullscreen: false,
            #[cfg(windows)]
            vm_console_input_active: false,
            #[cfg(windows)]
            vm_console_last_pointer_pos: None,
            #[cfg(windows)]
            hyperv_texture: None,
            #[cfg(windows)]
            hyperv_guest_size: None,
            #[cfg(windows)]
            hyperv_status: String::new(),
            #[cfg(windows)]
            hyperv_console_vm: None,
            #[cfg(windows)]
            hyperv_load_rx: None,
            #[cfg(windows)]
            hyperv_loading: false,
            #[cfg(windows)]
            hyperv_checked: false,
            #[cfg(windows)]
            hyperv_frame_count: 0,
            #[cfg(windows)]
            hyperv_fps_window: std::time::Instant::now(),
            #[cfg(windows)]
            hyperv_fps_display: 0.0,
            #[cfg(windows)]
            hyperv_last_frame: std::time::Instant::now(),
            #[cfg(windows)]
            hyperv_last_refresh: None,
            #[cfg(windows)]
            hyperv_rdp_session: None,
            #[cfg(windows)]
            hyperv_rdp_creds: hyperv_rdp::RdpCredentials::default(),
            #[cfg(windows)]
            vbox_vrde_ports: std::collections::HashMap::new(),
            #[cfg(windows)]
            vbox_vrde_session: None,
            #[cfg(windows)]
            vbox_vrde_last_desktop_size: None,
            #[cfg(windows)]
            vbox_vrde_enable_rx: None,
            #[cfg(windows)]
            vbox_mouse_warning_rx: None,
            #[cfg(windows)]
            vbox_mouse_warning: None,
            #[cfg(windows)]
            vbox_vrde_active_port: None,
            #[cfg(windows)]
            vbox_vrde_settings: vbox_rdp::VrdeSettings::default(),
            #[cfg(windows)]
            vbox_vrde_settings_open: false,
            #[cfg(windows)]
            vbox_vrde_last_frame: std::time::Instant::now(),
            #[cfg(windows)]
            vbox_vrde_last_input: None,
            #[cfg(windows)]
            vbox_vrde_scan_rx: None,
            remote_vms: Vec::new(),
            remote_attached_vm: String::new(),
            remote_vm_status: String::new(),
            remote_vm_panel_open: false,
            provider_registry: {
                use std::sync::Arc;
                use provider_api::ProviderRegistry;
                // Только реальные провайдеры. Hyper-V регистрируется в рантайме
                // на Windows после успешного WMI-скана; VirtualBox — при наличии
                // VBoxManage. Никаких фейковых демо-провайдеров в продакшене.
                Arc::new(ProviderRegistry::new())
            },
            dashboard_snapshot: None,
            #[cfg(windows)]
            dashboard_load_rx: None,
            remote_ctrl_vm_id: String::new(),
            remote_power_panel_open: false,
            remote_checkpoint_panel_open: false,
            remote_rescue_panel_open: false,
            remote_rescue_text: String::new(),
            pending_vm_action: None,
            show_settings: false,
            settings_draft: None,
            settings_custom_server: crate::settings::ServerConfig {
                id_server: String::new(),
                relay_server: String::new(),
                api_url: String::new(),
                public_key: String::new(),
            },

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
            hotfix_state: Arc::new(Mutex::new(hotfix::HotfixState::default())),
            hotfix_consent: None,
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
        self.last_error_at = None;
        self.reconnect_after = None;
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
                // Adaptive view by default: the remote desktop always scales to
                // fit the session window (the user can still switch to 1:1 with
                // the fit toggle during the session). Without this a saved 1:1
                // preference renders the desktop at native size, overflowing the
                // window and breaking maximize on macOS.
                self.fit_to_window = true;
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
            SessionEvent::VmList(json) => {
                self.remote_vms = parse_remote_vms(&json);
                self.log(format!("Хост VM: получено {} машин", self.remote_vms.len()));
            }
            SessionEvent::VmStatus(status) => {
                self.log(format!("Хост VM: {status}"));
                self.remote_vm_status = status;
            }
            SessionEvent::VmPowerResult(json) => {
                // {"vm_id":"…","action":"…","ok":true/false,"error":"…"}
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                    let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
                    let action = v.get("action").and_then(|x| x.as_str()).unwrap_or("?");
                    let err = v.get("error").and_then(|x| x.as_str()).unwrap_or("");
                    if ok {
                        self.log(format!("Power action '{action}' — OK"));
                    } else {
                        self.log(format!("Power action '{action}' — ERROR: {err}"));
                    }
                }
            }
            SessionEvent::VmCapabilities(json) => {
                // Обновляем capability graph в записи remote_vms
                if let Some(graph) = capability_engine::VmCapabilityGraph::from_json(&json) {
                    let vm_id = graph.vm_id.clone();
                    for vm in &mut self.remote_vms {
                        if vm.id == vm_id {
                            vm.capability_graph = Some(graph.clone());
                            break;
                        }
                    }
                    self.log(format!("Capability graph: {} — {}", vm_id, graph.recommended_mode.label()));
                }
            }
            SessionEvent::VmCheckpoints(json) => {
                // {"vm_id":"…","op":"list","ok":true,"checkpoints":[…]}
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                    let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
                    let op = v.get("op").and_then(|x| x.as_str()).unwrap_or("?");
                    let vm_id = v.get("vm_id").and_then(|x| x.as_str()).unwrap_or("?");
                    if ok {
                        let count = v.get("checkpoints")
                            .and_then(|x| x.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0);
                        self.log(format!("Checkpoint '{op}' for {vm_id}: {count} checkpoints"));
                        // Обновить checkpoint list в remote_vms
                        if op == "list" {
                            for vm in &mut self.remote_vms {
                                if vm.id == vm_id {
                                    vm.checkpoints_json = Some(json.clone());
                                    break;
                                }
                            }
                        }
                    } else {
                        let err = v.get("error").and_then(|x| x.as_str()).unwrap_or("unknown");
                        self.log(format!("Checkpoint '{op}' error: {err}"));
                    }
                }
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
                self.last_error_at = Some(Instant::now());
                self.status = friendly_error(&err, self.ui_lang);
                self.log(format!("Error: {err}"));
                // Auto-reconnect unless this is an auth or "ID not found" error
                // (those require user action before a retry will succeed).
                let is_permanent = err.contains("Wrong Password")
                    || err.contains("ID does not exist")
                    || err.contains("Введите")
                    || err.contains("Enter ");
                if self.auto_reconnect && !is_permanent && !self.remote_id.is_empty() {
                    self.reconnect_after =
                        Some(Instant::now() + std::time::Duration::from_secs(10));
                }
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
        self.reconnect_after = None;
        self.status = status.to_owned();
        self.log(status.to_owned());
    }

    fn visible_status(&self) -> String {
        if self.busy {
            return "Подключение...".to_owned();
        }
        if let Some(error) = &self.last_error {
            return friendly_error(error, self.ui_lang);
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
        self.last_error_at = Some(Instant::now());
        self.status = friendly_error(message, self.ui_lang);
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
                    // WM_NULL with a 3-second timeout. Two consecutive misses
                    // before killing: one is enough to confirm a GPU deadlock,
                    // two guards against a transient stall during DXGI teardown.
                    let mut result = 0usize;
                    let ok = SendMessageTimeoutA(
                        hwnd,
                        WM_NULL,
                        WPARAM(0),
                        LPARAM(0),
                        SMTO_ABORTIFHUNG,
                        3000,
                        Some(&mut result),
                    );
                    if ok.0 != 0 {
                        misses = 0;
                        continue;
                    }
                    misses += 1;
                    if misses >= 2 {
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
        // Update window title to reflect active session host.
        {
            let title = if self.connected && !self.remote_id.is_empty() {
                format!("{} — {APP_NAME}", self.remote_id)
            } else {
                APP_NAME.to_owned()
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }

        // Apply user-configured UI zoom (0.0 = platform default).
        {
            let scale = self.config.ui.ui_scale;
            if scale > 0.0 {
                ctx.set_zoom_factor(scale.clamp(0.75, 2.0));
            }
        }

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

        // Auto-clear stale errors after 8 seconds.
        if let Some(at) = self.last_error_at {
            if at.elapsed().as_secs() >= 8 && !self.connected && !self.busy {
                self.last_error = None;
                self.last_error_at = None;
            }
        }

        // Auto-reconnect: attempt to reconnect after the scheduled delay.
        if let Some(at) = self.reconnect_after {
            if at <= Instant::now() && !self.busy && !self.connected {
                self.reconnect_after = None;
                self.connect();
            } else {
                // Keep repainting so the countdown seconds update in the UI.
                ctx.request_repaint_after(std::time::Duration::from_secs(1));
            }
        }

        self.poll_worker();
        self.poll_terminal_ai();
        self.maybe_request_terminal_auto_ai();
        self.poll_host_service();
        self.poll_hotfix(ctx);
        #[cfg(windows)]
        self.poll_hyperv_session(ctx);
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
        if self.connected && self.remote_vm_panel_open {
            self.remote_vm_window(ctx);
        }
        if self.pending_vm_action.is_some() {
            self.vm_action_confirm_window(ctx);
        }
        if self.host_pending_peer.is_some() {
            self.incoming_approval_window(ctx);
        }

        // Floating connection badge — visible on every tab when host session is active.
        if let HostState::Accepting(ref peer_id) = self.host_state.clone() {
            egui::Window::new("__active_session_badge")
                .title_bar(false)
                .resizable(false)
                .movable(false)
                .collapsible(false)
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 16.0))
                .frame(
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(18, 95, 170))
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Margin::symmetric(14, 10)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("📡").size(20.0));
                        ui.add_space(6.0);
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("К вам подключены")
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(140, 190, 255)),
                            );
                            ui.label(
                                egui::RichText::new(peer_id.as_str())
                                    .size(14.0)
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            );
                        });
                    });
                });
        }

        let screen_rect = ctx.content_rect();
        ctx.layer_painter(egui::LayerId::background()).rect_filled(
            screen_rect,
            egui::CornerRadius::ZERO,
            crate::theme::palette().bg,
        );

        // ── Left sidebar: logo · navigation · settings ───────────────────────
        egui::Panel::left("everty_sidebar")
            .resizable(false)
            .exact_size(220.0)
            .frame(
                egui::Frame::NONE
                    .fill(crate::theme::palette().surface)
                    .stroke(egui::Stroke::new(
                        1.0,
                        crate::theme::palette().border,
                    ))
                    .corner_radius(egui::CornerRadius::ZERO)
                    .inner_margin(egui::Margin::symmetric(18, 20))
                    .outer_margin(egui::Margin::ZERO),
            )
            .show(ctx, |ui| self.sidebar(ui));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(crate::theme::palette().bg)
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
                #[cfg(windows)]
                AppMode::HyperV => self.hyperv_ui(ui),
                #[cfg(not(windows))]
                AppMode::HyperV => self.hyperv_unavailable_ui(ui),
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

    fn poll_hotfix(&mut self, ctx: &egui::Context) {
        // Tick TTL-rollbacks and pick up any pending consent requests.
        let state = Arc::clone(&self.hotfix_state);
        if let Some(req) = hotfix::tick(&state, &mut self.config) {
            if self.hotfix_consent.is_none() {
                self.hotfix_consent = Some(req);
            }
        }

        // Show consent dialog if needed.
        if let Some(req) = self.hotfix_consent.clone() {
            let mut open = true;
            egui::Window::new("AI Hotfix — требуется подтверждение")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(egui::RichText::new(&req.summary).size(13.0));
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(format!("Риск: {}", req.risk_level))
                        .size(12.0)
                        .color(egui::Color32::YELLOW));
                    ui.add_space(4.0);
                    for action in &req.actions_human {
                        ui.label(format!("• {action}"));
                    }
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(
                        "Изменения будут автоматически отменены через некоторое время если станет хуже."
                    ).size(11.0).color(egui::Color32::GRAY));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("✓ Применить").clicked() {
                            hotfix::confirm_consent(&state, &mut self.config);
                            self.hotfix_consent = None;
                        }
                        if ui.button("✗ Отказаться").clicked() {
                            hotfix::deny_consent(&state);
                            self.hotfix_consent = None;
                        }
                    });
                });
            if !open {
                hotfix::deny_consent(&state);
                self.hotfix_consent = None;
            }
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
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .button(tr(self.ui_lang, "🔄 Повторить", "🔄 Retry"))
                    .clicked()
                {
                    self.reconnect_after = None;
                    self.connect();
                }
                if let Some(at) = self.reconnect_after {
                    let secs = at
                        .checked_duration_since(Instant::now())
                        .map(|d| d.as_secs() + 1)
                        .unwrap_or(0);
                    let countdown_text = match self.ui_lang {
                        UiLang::Ru => format!("Авто-переподключение через {secs}с…"),
                        UiLang::En => format!("Auto-reconnect in {secs}s…"),
                    };
                    ui.label(countdown_text);
                    if ui
                        .small_button(tr(self.ui_lang, "Отмена", "Cancel"))
                        .clicked()
                    {
                        self.reconnect_after = None;
                    }
                }
            });
        } else {
            ui.label(self.visible_status());
        }
        if self.connected && !self.remote_viewer_open {
            ui.label(tr(
                self.ui_lang,
                "Окно экрана закрыто. Нажмите Экран, чтобы открыть его снова.",
                "Screen window is closed. Click Screen to open it again.",
            ));
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
                crate::theme::palette().surface_raised,
            );
            p.rect_stroke(
                rect,
                egui::CornerRadius::same(12),
                egui::Stroke::new(1.0, crate::theme::palette().border),
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
                    crate::theme::palette().accent,
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
                        .color(crate::theme::palette().text),
                );
                ui.label(
                    egui::RichText::new(format!("v{APP_VERSION}"))
                        .size(12.0)
                        .color(crate::theme::palette().text_weak),
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
        #[cfg(windows)]
        {
            let hv_label = self.text("VM", "VM");
            self.nav_item(ui, AppMode::HyperV, hv_label, "server");
            ui.add_space(8.0);
        }
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
        let gray = crate::theme::palette().text_weak;
        card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                egui::RichText::new(self.text("Этот компьютер", "This computer"))
                    .size(18.0)
                    .color(crate::theme::palette().text),
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
                        .color(crate::theme::palette().text),
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
                        .color(crate::theme::palette().text),
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
                    crate::theme::palette().accent
                } else {
                    crate::theme::palette().accent
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
                    .color(crate::theme::palette().text),
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
            crate::theme::palette().warning
        };
        let cap_label = if online {
            tr(lang, "В сети", "Online")
        } else {
            &host_label
        };
        let cap_galley = ui.painter().layout_no_wrap(
            cap_label.to_owned(),
            egui::FontId::proportional(12.5),
            crate::theme::palette().accent_fg,
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
                .color(crate::theme::palette().text_muted),
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
                                .color(crate::theme::palette().text_weak),
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
                                    .color(crate::theme::palette().text_weak),
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

                        ui.add_space(theme::space::SM);
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
        if self.last_error.is_some() {
            ui.horizontal(|ui| {
                if ui
                    .small_button(self.text("🔄 Повторить", "🔄 Retry"))
                    .clicked()
                {
                    self.reconnect_after = None;
                    self.connect();
                }
                if let Some(at) = self.reconnect_after {
                    let secs = at
                        .checked_duration_since(Instant::now())
                        .map(|d| d.as_secs() + 1)
                        .unwrap_or(0);
                    let msg = match self.ui_lang {
                        UiLang::Ru => format!("авто-переподключение через {secs}с"),
                        UiLang::En => format!("auto-reconnect in {secs}s"),
                    };
                    ui.label(
                        egui::RichText::new(msg)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(130, 140, 155)),
                    );
                    if ui.small_button(self.text("✕", "✕")).clicked() {
                        self.reconnect_after = None;
                    }
                }
            });
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
                        .color(crate::theme::palette().text_muted),
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
                        .color(crate::theme::palette().text_muted),
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
            paint_status_dot(ui, dot_color);
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
            let t = crate::theme::palette();
            ui.add_space(theme::space::SM);
            egui::Frame::group(ui.style())
                .fill(crate::theme::accent_tint(&t, 0.12))
                .stroke(egui::Stroke::new(1.0, crate::theme::tint(t.accent, 0.45)))
                .corner_radius(egui::CornerRadius::same(theme::radius::LG))
                .inner_margin(egui::Margin::same(theme::space::MD as i8))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!("📡 Входящий запрос от: {peer_id}"))
                            .strong()
                            .color(t.text),
                    );
                    ui.add_space(theme::space::SM);
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("✓ Принять").color(t.accent_fg),
                                )
                                .min_size(egui::vec2(100.0, 28.0))
                                .fill(t.accent),
                            )
                            .clicked()
                        {
                            // TODO: full relay session acceptance
                            self.host_pending_peer = None;
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("✗ Отклонить").color(t.danger),
                                )
                                .min_size(egui::vec2(100.0, 28.0))
                                .fill(crate::theme::tint(t.danger, 0.14)),
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

    // ── Hyper-V UI ────────────────────────────────────────────────────────────

    #[cfg(not(windows))]
    fn hyperv_unavailable_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading(egui::RichText::new("Hyper-V VMs").size(20.0).strong());
        ui.add_space(12.0);
        ui.label("Локальный доступ к Hyper-V доступен только на Windows-хосте.");
        ui.add_space(6.0);
        ui.label(
            "Подключитесь к удалённому Windows-хосту-гипервизору — список его VM \
             появится в окне сеанса (кнопка «VM хоста»).",
        );
    }

    #[cfg(windows)]
    /// Launch the native Hyper-V Virtual Machine Connection (vmconnect.exe) for a VM.
    /// vmconnect.exe is the official Microsoft tool — full keyboard/mouse/clipboard support.
    #[cfg(windows)]
    fn launch_vmconnect(vm_id: &str, vm_name: &str) {
        // vmconnect.exe <server> <vm-name> -G {guid}
        // Pass "." as server = local machine
        let _ = std::process::Command::new("vmconnect.exe")
            .args([".", vm_name, "-G", vm_id])
            .spawn();
    }

    #[cfg(windows)]
    fn hyperv_start_load(&mut self) {
        if self.hyperv_loading || self.hyperv_load_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.hyperv_load_rx = Some(rx);
        self.hyperv_loading = true;
        self.hyperv_last_refresh = None; // loading in progress
        thread::spawn(move || {
            let _ = tx.send(hyperv::list_vms());
        });
    }

    #[cfg(windows)]
    fn hyperv_ui(&mut self, ui: &mut egui::Ui) {
        // ── Auto-scan on first visit ─────────────────────────────────────────
        if self.hyperv_vms.is_empty()
            && !self.hyperv_loading
            && self.hyperv_load_rx.is_none()
            && !self.hyperv_checked
        {
            self.hyperv_start_load();
        }

        // ── Layout: SidePanel + console ───────────────────────────────────────
        const SIDEBAR_W: f32 = 276.0;
        let fullscreen = self.vm_console_fullscreen;

        if fullscreen {
            self.vm_console_panel_inline(ui, true);
            return;
        }

        // Use egui SidePanel so sidebar closure ends before we call self methods
        egui::SidePanel::left("vm_sidebar_panel")
            .default_width(SIDEBAR_W)
            .width_range(220.0..=460.0)
            .resizable(true)
            .show_inside(ui, |ui| {
                ui.set_clip_rect(ui.max_rect());

                // Sidebar header row
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Виртуальные машины")
                            .size(13.0)
                            .strong()
                            .color(crate::theme::palette().text),
                    );
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            // FPS badge
                            if self.hyperv_fps_display > 0.0 {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{:.1}fps",
                                        self.hyperv_fps_display
                                    ))
                                    .size(10.5)
                                    .color(if self.hyperv_fps_display >= 1.5 {
                                        crate::theme::palette().success
                                    } else {
                                        crate::theme::palette().warning
                                    }),
                                );
                            }
                            // Disconnect button
                            let any_sess = self.hyperv_session.is_some()
                                || self.vbox_session.is_some()
                                || self.vbox_vrde_session.is_some()
                                || self.hyperv_rdp_session.is_some();
                            if any_sess
                                && ui
                                    .add(egui::Button::new(egui::RichText::new(egui_phosphor::regular::X).size(15.0)).min_size(egui::vec2(26.0, 24.0)))
                                    .on_hover_text("Отключиться от консоли VM")
                                    .clicked()
                            {
                                self.stop_vm_console_sessions();
                                self.hyperv_console_vm = None;
                                self.hyperv_texture = None;
                                self.hyperv_status.clear();
                                self.hyperv_fps_display = 0.0;
                            }
                            // Refresh button
                            if ui
                                .add_enabled(
                                    !self.hyperv_loading,
                                    egui::Button::new(egui::RichText::new(egui_phosphor::regular::ARROWS_CLOCKWISE).size(15.0)).min_size(egui::vec2(26.0, 24.0)),
                                )
                                .on_hover_text("Обновить список VM")
                                .clicked()
                            {
                                self.hyperv_vms.clear();
                                self.hyperv_load_rx = None;
                                self.vbox_vrde_scan_rx = None;
                                self.hyperv_loading = false;
                                self.hyperv_checked = false;
                                if self.hyperv_load_rx.is_none() {
                                    let (tx, rx) = mpsc::channel();
                                    self.hyperv_load_rx = Some(rx);
                                    self.hyperv_loading = true;
                                    self.hyperv_last_refresh = None;
                                    thread::spawn(move || {
                                        let _ = tx.send(hyperv::list_vms());
                                    });
                                }
                            }
                            // Last refresh age
                            if let Some(t) = self.hyperv_last_refresh {
                                let s = t.elapsed().as_secs();
                                let age = if s < 60 {
                                    format!("{}с", s)
                                } else {
                                    format!("{}м", s / 60)
                                };
                                ui.label(
                                    egui::RichText::new(age)
                                        .size(10.0)
                                        .color(crate::theme::palette().text_muted),
                                );
                            }
                        },
                    );
                });

                // Search box
                ui.add_space(5.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.vm_search)
                        .hint_text("Поиск VM...")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(5.0);
                ui.separator();
                ui.add_space(2.0);

                // Loading / empty states inside sidebar
                if self.hyperv_loading && self.hyperv_vms.is_empty() {
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new("Поиск гипервизоров...")
                            .size(12.0)
                            .color(crate::theme::palette().text_muted),
                    );
                    ui.ctx().request_repaint_after(Duration::from_millis(300));
                } else if self.hyperv_checked && self.hyperv_vms.is_empty() {
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new("Гипервизоры не найдены.\nНажмите R для повторного поиска.")
                            .size(11.0)
                            .color(crate::theme::palette().text_muted),
                    );
                }

                // VM list (grouped by provider)
                egui::ScrollArea::vertical()
                    .id_salt("vm_sidebar_list")
                    .show(ui, |ui| {
                        ui.set_width((ui.available_width() - 8.0).max(200.0));
                        let search_lc = self.vm_search.to_lowercase();

                        let provider_groups = [
                            (hyperv::VmProvider::HyperV, "HYPER-V", egui::Color32::from_rgb(0x00, 0x78, 0xD4)),
                            (hyperv::VmProvider::VirtualBox, "VIRTUALBOX", egui::Color32::from_rgb(0x18, 0x3A, 0x5C)),
                            (hyperv::VmProvider::VMware, "VMWARE", egui::Color32::from_rgb(0x60, 0x7D, 0x8B)),
                        ];

                        for (provider_kind, group_label, _badge_color) in &provider_groups {
                            let indices: Vec<usize> = (0..self.hyperv_vms.len())
                                .filter(|&i| {
                                    &self.hyperv_vms[i].provider == provider_kind
                                        && (search_lc.is_empty()
                                            || self.hyperv_vms[i]
                                                .name
                                                .to_lowercase()
                                                .contains(&search_lc))
                                })
                                .collect();

                            if indices.is_empty() {
                                continue;
                            }

                            // Provider group header
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(*group_label)
                                    .size(9.5)
                                    .color(crate::theme::palette().text_muted),
                            );
                            ui.add_space(2.0);

                            for i in indices {
                                let is_active = self.hyperv_console_vm == Some(i);
                                // Clone per-VM data so we don't hold a borrow of hyperv_vms
                                let (vm_name, vm_state, vm_id, vm_wmi_path, vm_console_mode, vm_provider) = {
                                    let vm = &self.hyperv_vms[i];
                                    (
                                        vm.name.clone(),
                                        vm.state.clone(),
                                        vm.id.clone(),
                                        vm.wmi_path.clone(),
                                        vm.console_mode.clone(),
                                        vm.provider.clone(),
                                    )
                                };

                                let state_color = match vm_state {
                                    hyperv::VmState::Running => crate::theme::palette().success,
                                    hyperv::VmState::Paused | hyperv::VmState::Saved => {
                                        crate::theme::palette().warning
                                    }
                                    _ => crate::theme::palette().text_muted,
                                };

                                let row_fill = if is_active {
                                    crate::theme::accent_tint(
                                        &crate::theme::palette(),
                                        0.18,
                                    )
                                } else {
                                    egui::Color32::TRANSPARENT
                                };

                                egui::Frame::NONE
                                    .fill(row_fill)
                                    .corner_radius(egui::CornerRadius::same(6))
                                    .inner_margin(egui::Margin::symmetric(6, 4))
                                    .show(ui, |ui| {
                                        ui.set_width((ui.available_width() - 4.0).max(180.0));

                                        // Row 1: dot + name + preview/action buttons
                                        ui.horizontal(|ui| {
                                            paint_status_dot(ui, state_color);
                                            ui.label(
                                                egui::RichText::new(&vm_name)
                                                    .strong()
                                                    .size(13.0),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(
                                                    egui::Align::Center,
                                                ),
                                                |ui| {
                                                    let can_preview = vm_state
                                                        .is_connectable()
                                                        && matches!(
                                                            vm_provider,
                                                            hyperv::VmProvider::HyperV
                                                                | hyperv::VmProvider::VirtualBox
                                                        );
                                                    let preview_tip = if !vm_state
                                                        .is_connectable()
                                                    {
                                                        "VM выключена"
                                                    } else if !can_preview {
                                                        "Недоступно для этого гипервизора"
                                                    } else {
                                                        "Открыть консоль"
                                                    };
                                                    let preview_lbl =
                                                        if is_active
                                                            && (self.hyperv_session.is_some()
                                                                || self.vbox_session.is_some()
                                                                || self.vbox_vrde_session.is_some())
                                                        {
                                                            egui_phosphor::regular::MONITOR
                                                        } else {
                                                            egui_phosphor::regular::MONITOR_PLAY
                                                        };

                                                    let pbtn = ui
                                                        .add_enabled(
                                                            can_preview && vm_state.is_connectable(),
                                                            egui::Button::new(egui::RichText::new(preview_lbl).size(15.0))
                                                                .min_size(egui::vec2(26.0, 24.0)),
                                                        )
                                                        .on_hover_text(
                                                            "Предпросмотр экрана VM",
                                                        )
                                                        .on_disabled_hover_text(preview_tip);

                                                    // Сессия для этой VM уже активна и жива — повторный клик
                                                    // не должен пересоздавать соединение: каждое новое VRDE-
                                                    // подключение выбивает предыдущее ("administrative tool"
                                                    // disconnect на стороне VirtualBox), и при частых кликах
                                                    // это превращалось в бесконечный цикл реконнектов с чёрным
                                                    // экраном, потому что кадр не успевал прийти до следующего
                                                    // обрыва.
                                                    let vrde_already_connected = matches!(vm_provider, hyperv::VmProvider::VirtualBox)
                                                        && is_active
                                                        && self.vbox_vrde_session.is_some();
                                                    let hyperv_already_connected = matches!(vm_provider, hyperv::VmProvider::HyperV)
                                                        && is_active
                                                        && self.hyperv_session.is_some();
                                                    if pbtn.clicked() && !vrde_already_connected && !hyperv_already_connected {
                                                        self.stop_vm_console_sessions();
                                                        self.hyperv_console_vm = Some(i);
                                                        self.hyperv_texture = None;
                                                        self.hyperv_status =
                                                            "Запуск захвата экрана..."
                                                                .to_owned();
                                                        self.vm_console_fullscreen = false;
                                                        match vm_provider {
                                                            hyperv::VmProvider::HyperV => {
                                                                let vm_clone = self
                                                                    .hyperv_vms[i]
                                                                    .clone();
                                                                self.hyperv_session = Some(
                                                                    hyperv::HyperVSession::start(vm_clone),
                                                                );
                                                            }
                                                            hyperv::VmProvider::VirtualBox => {
                                                                // Prefer embedded VRDE/IronRDP for interactive VirtualBox console.
                                                                if let Some(&port) = self.vbox_vrde_ports.get(&vm_id) {
                                                                    self.start_vbox_vrde_session(port);
                                                                } else {
                                                                    let port = Self::vbox_vrde_port(&vm_id);
                                                                    self.begin_vbox_vrde_connect(
                                                                        vm_id.clone(),
                                                                        port,
                                                                        matches!(vm_state, hyperv::VmState::Running),
                                                                    );
                                                                }
                                                            }
                                                            _ => {}
                                                        }
                                                    } else if pbtn.clicked() {
                                                        // Уже подключены — просто переключаем фокус на консоль.
                                                        self.hyperv_console_vm = Some(i);
                                                    }

                                                    // VMConnect (Hyper-V)
                                                    if matches!(
                                                        vm_provider,
                                                        hyperv::VmProvider::HyperV
                                                    ) && vm_state.is_connectable()
                                                    {
                                                        if ui
                                                            .add(egui::Button::new(egui::RichText::new(egui_phosphor::regular::DESKTOP).size(15.0)).min_size(egui::vec2(26.0, 24.0)))
                                                            .on_hover_text(
                                                                "vmconnect.exe — полная клавиатура и буфер обмена",
                                                            )
                                                            .clicked()
                                                        {
                                                            Self::launch_vmconnect(
                                                                &vm_id, &vm_name,
                                                            );
                                                        }
                                                    }

                                                    // RDP (Enhanced Session — Hyper-V)
                                                    if matches!(
                                                        vm_provider,
                                                        hyperv::VmProvider::HyperV
                                                    ) && matches!(
                                                        vm_console_mode,
                                                        hyperv::ConsoleMode::EnhancedSession
                                                    ) && vm_state.is_connectable()
                                                    {
                                                        let rdp_id = vm_id.clone();
                                                        if ui
                                                            .add(egui::Button::new(egui::RichText::new(egui_phosphor::regular::MONITOR_ARROW_UP).size(15.0)).min_size(egui::vec2(26.0, 24.0)))
                                                            .on_hover_text(
                                                                "RDP over VMBus — 30-60 FPS (Integration Services активны)",
                                                            )
                                                            .clicked()
                                                        {
                                                            self.stop_vm_console_sessions();
                                                            self.hyperv_console_vm =
                                                                Some(i);
                                                            self.hyperv_texture = None;
                                                            let creds = self.hyperv_rdp_creds.clone();
                                                            let size = default_vrde_desktop_size();
                                                            match hyperv_rdp::RdpSession::connect(&rdp_id, creds, size) {
                                                                Ok(sess) => {
                                                                    self.hyperv_rdp_session = Some(sess);
                                                                    self.hyperv_status = "RDP: подключение...".to_owned();
                                                                }
                                                                Err(e) => {
                                                                    self.hyperv_status = format!("RDP: {e}");
                                                                }
                                                            }
                                                        }
                                                    }

                                                    // VRDE + RDP (VirtualBox)
                                                    if matches!(
                                                        vm_provider,
                                                        hyperv::VmProvider::VirtualBox
                                                    ) && vm_state.is_connectable()
                                                    {
                                                        let vrde_port = self
                                                            .vbox_vrde_ports
                                                            .get(&vm_id)
                                                            .copied();

                                                        if let Some(port) = vrde_port {
                                                            // Embedded IronRDP client
                                                            if ui
                                                                .add(egui::Button::new(egui::RichText::new(egui_phosphor::regular::MONITOR_PLAY).size(15.0)).min_size(egui::vec2(26.0, 24.0)))
                                                                .on_hover_text(format!(
                                                                    "IronRDP: встроенная консоль localhost:{port}"
                                                                ))
                                                                .clicked()
                                                            {
                                                                self.hyperv_console_vm = Some(i);
                                                                self.vm_console_fullscreen = false;
                                                                self.start_vbox_vrde_session(port);
                                                            }
                                                            // External RDP client fallback
                                                            if ui
                                                                .add(egui::Button::new(egui::RichText::new(egui_phosphor::regular::EXPORT).size(15.0)).min_size(egui::vec2(26.0, 24.0)))
                                                                .on_hover_text(format!(
                                                                    "mstsc: внешний RDP-клиент localhost:{port}"
                                                                ))
                                                                .clicked()
                                                            {
                                                                virtualbox::launch_rdp(port);
                                                            }
                                                            // Disable VRDE
                                                            let uuid_off = vm_id.clone();
                                                            let is_run = matches!(
                                                                vm_state,
                                                                hyperv::VmState::Running
                                                            );
                                                            if ui
                                                                .add(egui::Button::new(egui::RichText::new(egui_phosphor::regular::PLUG).size(15.0)).min_size(egui::vec2(26.0, 24.0)))
                                                                .on_hover_text(format!(
                                                                    "VRDE включён (порт {port}). Нажмите для отключения"
                                                                ))
                                                                .clicked()
                                                            {
                                                                self.vbox_vrde_ports
                                                                    .remove(&uuid_off);
                                                                thread::spawn(move || {
                                                                    virtualbox::disable_vrde(
                                                                        &uuid_off, is_run,
                                                                    );
                                                                });
                                                            }
                                                        } else {
                                                            // Enable VRDE
                                                            let uuid_on = vm_id.clone();
                                                            let is_run = matches!(
                                                                vm_state,
                                                                hyperv::VmState::Running
                                                            );
                                                            if ui
                                                                .add(egui::Button::new(egui::RichText::new(egui_phosphor::regular::PLUGS_CONNECTED).size(15.0)).min_size(egui::vec2(26.0, 24.0)))
                                                                .on_hover_text(
                                                                    "Включить VRDE и сразу подключиться встроенным IronRDP.",
                                                                )
                                                                .clicked()
                                                            {
                                                                self.hyperv_console_vm = Some(i);
                                                                self.vm_console_fullscreen = false;
                                                                let port = Self::vbox_vrde_port(&uuid_on);
                                                                self.begin_vbox_vrde_connect(uuid_on, port, is_run);
                                                            }
                                                        }
                                                    }
                                                },
                                            );
                                        });

                                        // Row 2: state + badges + power controls
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(vm_state.label())
                                                    .size(10.5)
                                                    .color(crate::theme::palette().text_muted),
                                            );
                                            // Console mode badge
                                            if matches!(
                                                vm_provider,
                                                hyperv::VmProvider::HyperV
                                            ) {
                                                let cm = match vm_console_mode {
                                                    hyperv::ConsoleMode::EnhancedSession => Some(("RDP", egui::Color32::from_rgb(0x22, 0x8B, 0x22))),
                                                    hyperv::ConsoleMode::ThumbnailOnly => Some(("WMI", crate::theme::palette().text_muted)),
                                                    _ => None,
                                                };
                                                if let Some((lbl, clr)) = cm {
                                                    egui::Frame::NONE
                                                        .fill(clr)
                                                        .corner_radius(egui::CornerRadius::same(3))
                                                        .inner_margin(egui::Margin::symmetric(3, 1))
                                                        .show(ui, |ui| {
                                                            ui.label(egui::RichText::new(lbl).size(9.0).color(egui::Color32::WHITE));
                                                        });
                                                }
                                            }
                                            // VRDE badge (VirtualBox)
                                            if matches!(vm_provider, hyperv::VmProvider::VirtualBox) {
                                                if let Some(port) = self.vbox_vrde_ports.get(&vm_id) {
                                                    egui::Frame::NONE
                                                        .fill(egui::Color32::from_rgb(0x00, 0x6B, 0x3C))
                                                        .corner_radius(egui::CornerRadius::same(3))
                                                        .inner_margin(egui::Margin::symmetric(3, 1))
                                                        .show(ui, |ui| {
                                                            ui.label(egui::RichText::new(format!("VRDE:{port}")).size(9.0).color(egui::Color32::WHITE));
                                                        });
                                                }
                                            }
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if matches!(vm_provider, hyperv::VmProvider::HyperV) {
                                                        let wmi = vm_wmi_path.clone();
                                                        match vm_state {
                                                            hyperv::VmState::Off | hyperv::VmState::Saved => {
                                                                if vm_icon_button(ui, egui_phosphor::regular::POWER, "Запустить VM").clicked() {
                                                                    hyperv::request_power_action(&wmi, hyperv::VmPowerAction::Start);
                                                                }
                                                            }
                                                            hyperv::VmState::Running | hyperv::VmState::Paused => {
                                                                if vm_icon_button(ui, egui_phosphor::regular::ARROWS_CLOCKWISE, "Перезапустить VM").clicked() {
                                                                    hyperv::request_power_action(&wmi, hyperv::VmPowerAction::Restart);
                                                                }
                                                                if vm_icon_button(ui, egui_phosphor::regular::STOP, "Выключить VM").clicked() {
                                                                    hyperv::request_power_action(&wmi, hyperv::VmPowerAction::Stop);
                                                                }
                                                            }
                                                            _ => {}
                                                        }
                                                    }
                                                    if matches!(vm_provider, hyperv::VmProvider::VirtualBox) {
                                                        let uuid = vm_id.clone();
                                                        match vm_state {
                                                            hyperv::VmState::Off => {
                                                                if vm_icon_button(ui, egui_phosphor::regular::POWER, "Запустить VM (headless)").clicked() {
                                                                    virtualbox::start_vm(&uuid);
                                                                }
                                                            }
                                                            hyperv::VmState::Running => {
                                                                if vm_icon_button(ui, egui_phosphor::regular::ARROWS_CLOCKWISE, "Перезапустить VM (reset)").clicked() {
                                                                    virtualbox::reset_vm(&uuid);
                                                                }
                                                                if vm_icon_button(ui, egui_phosphor::regular::STOP, "Выключить VM (poweroff)").clicked() {
                                                                    virtualbox::stop_vm(&uuid);
                                                                }
                                                            }
                                                            _ => {}
                                                        }
                                                    }
                                                    // Hyper-V power controls
                                                    if false && matches!(vm_provider, hyperv::VmProvider::HyperV) {
                                                        let wmi = vm_wmi_path.clone();
                                                        match vm_state {
                                                            hyperv::VmState::Off | hyperv::VmState::Saved => {
                                                                if ui.small_button("Старт").on_hover_text("Запустить VM").clicked() {
                                                                    hyperv::request_power_action(&wmi, hyperv::VmPowerAction::Start);
                                                                }
                                                            }
                                                            hyperv::VmState::Running | hyperv::VmState::Paused => {
                                                                if ui.small_button("Стоп").on_hover_text("Выключить VM").clicked() {
                                                                    hyperv::request_power_action(&wmi, hyperv::VmPowerAction::Stop);
                                                                }
                                                                if ui.small_button("Ребут").on_hover_text("Перезапустить VM").clicked() {
                                                                    hyperv::request_power_action(&wmi, hyperv::VmPowerAction::Restart);
                                                                }
                                                            }
                                                            _ => {}
                                                        }
                                                    }
                                                    // VirtualBox power controls
                                                    if false && matches!(vm_provider, hyperv::VmProvider::VirtualBox) {
                                                        let uuid = vm_id.clone();
                                                        match vm_state {
                                                            hyperv::VmState::Off => {
                                                                if ui.small_button("Старт").on_hover_text("Запустить VM (headless)").clicked() {
                                                                    virtualbox::start_vm(&uuid);
                                                                }
                                                            }
                                                            hyperv::VmState::Running => {
                                                                if ui.small_button("Стоп").on_hover_text("Выключить VM (poweroff)").clicked() {
                                                                    virtualbox::stop_vm(&uuid);
                                                                }
                                                                if ui.small_button("Ребут").on_hover_text("Перезапустить VM (reset)").clicked() {
                                                                    virtualbox::reset_vm(&uuid);
                                                                }
                                                            }
                                                            _ => {}
                                                        }
                                                    }
                                                },
                                            );
                                        });
                                    });

                                ui.add_space(2.0);
                            }
                        }
                    }); // ScrollArea
            }); // SidePanel

        // After SidePanel closure ends, ui now covers the remaining (right) console area
        self.vm_console_panel_inline(ui, false);
    }

    /// Renders the console panel (toolbar + canvas + status bar) inline.
    /// Called from hyperv_ui — outside any closure, so &mut self is available.
    #[cfg(windows)]
    fn vm_console_panel_inline(&mut self, ui: &mut egui::Ui, fullscreen: bool) {
        let palette = crate::theme::palette();

        // ── Toolbar ──────────────────────────────────────────────────────────
        egui::Frame::NONE
            .fill(egui::Color32::from_rgb(0x14, 0x1B, 0x26))
            .inner_margin(egui::Margin::symmetric(8, 5))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // Back / exit fullscreen button
                    if fullscreen {
                        if ui.small_button("< Назад").on_hover_text("Выйти из полного экрана").clicked() {
                            self.vm_console_fullscreen = false;
                        }
                        ui.separator();
                    }

                    // VM name + state
                    if let Some(idx) = self.hyperv_console_vm {
                        if let Some(vm) = self.hyperv_vms.get(idx) {
                            let state_color = match vm.state {
                                hyperv::VmState::Running => palette.success,
                                hyperv::VmState::Paused | hyperv::VmState::Saved => palette.warning,
                                _ => palette.text_muted,
                            };
                            paint_status_dot(ui, state_color);
                            ui.label(egui::RichText::new(&vm.name).strong().size(13.0));
                            let provider_lbl = match vm.provider {
                                hyperv::VmProvider::HyperV => "Hyper-V",
                                hyperv::VmProvider::VirtualBox => "VirtualBox",
                                hyperv::VmProvider::VMware => "VMware",
                            };
                            ui.label(egui::RichText::new(provider_lbl).size(10.5).color(palette.text_muted));

                            ui.separator();

                            // Power controls in toolbar (for active VM)
                            let wmi_path = vm.wmi_path.clone();
                            let vm_id = vm.id.clone();
                            let vm_provider = vm.provider.clone();
                            let vm_state = vm.state.clone();

                            match vm_state {
                                hyperv::VmState::Off | hyperv::VmState::Saved => {
                                    if matches!(vm_provider, hyperv::VmProvider::HyperV) {
                                        if ui.small_button("Старт").on_hover_text("Запустить VM").clicked() {
                                            hyperv::request_power_action(&wmi_path, hyperv::VmPowerAction::Start);
                                        }
                                    } else if matches!(vm_provider, hyperv::VmProvider::VirtualBox) {
                                        if ui.small_button("Старт").on_hover_text("Запустить VM (headless)").clicked() {
                                            virtualbox::start_vm(&vm_id);
                                        }
                                    }
                                }
                                hyperv::VmState::Running | hyperv::VmState::Paused => {
                                    if matches!(vm_provider, hyperv::VmProvider::HyperV) {
                                        if ui.small_button("Стоп").on_hover_text("Выключить VM").clicked() {
                                            hyperv::request_power_action(&wmi_path, hyperv::VmPowerAction::Stop);
                                        }
                                        if ui.small_button("Ребут").on_hover_text("Перезапустить VM").clicked() {
                                            hyperv::request_power_action(&wmi_path, hyperv::VmPowerAction::Restart);
                                        }
                                    } else if matches!(vm_provider, hyperv::VmProvider::VirtualBox) {
                                        if ui.small_button("Стоп").on_hover_text("Выключить VM (poweroff)").clicked() {
                                            virtualbox::stop_vm(&vm_id);
                                        }
                                        if ui.small_button("Ребут").on_hover_text("Перезапустить VM (reset)").clicked() {
                                            virtualbox::reset_vm(&vm_id);
                                        }
                                    }
                                }
                                _ => {}
                            }
                            ui.separator();
                        }
                    }

                    // Hotkeys (only when HyperV session active)
                    if let Some(session) = &self.hyperv_session {
                        if ui.small_button("Ctrl+C").clicked() {
                            session.send(hyperv::HyperVCmd::PressKey(0x11));
                            session.send(hyperv::HyperVCmd::PressKey(0x43));
                            session.send(hyperv::HyperVCmd::ReleaseKey(0x43));
                            session.send(hyperv::HyperVCmd::ReleaseKey(0x11));
                        }
                        if ui.small_button("Ctrl+V").clicked() {
                            let text = clipboard_read_text();
                            if !text.is_empty() {
                                session.send(hyperv::HyperVCmd::TypeText(text));
                            }
                        }
                        if ui.small_button("Ctrl+A").clicked() {
                            session.send(hyperv::HyperVCmd::PressKey(0x11));
                            session.send(hyperv::HyperVCmd::PressKey(0x41));
                            session.send(hyperv::HyperVCmd::ReleaseKey(0x41));
                            session.send(hyperv::HyperVCmd::ReleaseKey(0x11));
                        }
                        if ui.small_button("Ctrl+Z").clicked() {
                            session.send(hyperv::HyperVCmd::PressKey(0x11));
                            session.send(hyperv::HyperVCmd::PressKey(0x5A));
                            session.send(hyperv::HyperVCmd::ReleaseKey(0x5A));
                            session.send(hyperv::HyperVCmd::ReleaseKey(0x11));
                        }
                        if ui.small_button("Alt+Tab").on_hover_text("Alt+Tab в гостевой ОС").clicked() {
                            session.send(hyperv::HyperVCmd::PressKey(0x12));
                            session.send(hyperv::HyperVCmd::PressKey(0x09));
                            session.send(hyperv::HyperVCmd::ReleaseKey(0x09));
                            session.send(hyperv::HyperVCmd::ReleaseKey(0x12));
                        }
                        if ui.small_button("Caps").on_hover_text("Переключить Caps Lock").clicked() {
                            session.send(hyperv::HyperVCmd::PressKey(0x14));
                            session.send(hyperv::HyperVCmd::ReleaseKey(0x14));
                        }
                        if ui.small_button("Ctrl+Alt+Del").clicked() {
                            session.send(hyperv::HyperVCmd::CtrlAltDel);
                        }
                        if ui.small_button("Win").on_hover_text("Клавиша Windows").clicked() {
                            session.send(hyperv::HyperVCmd::PressKey(0x5B));
                            session.send(hyperv::HyperVCmd::ReleaseKey(0x5B));
                        }
                        if ui.small_button("Alt+F4").on_hover_text("Закрыть активное окно в VM").clicked() {
                            session.send(hyperv::HyperVCmd::PressKey(0x12));
                            session.send(hyperv::HyperVCmd::PressKey(0x73));
                            session.send(hyperv::HyperVCmd::ReleaseKey(0x73));
                            session.send(hyperv::HyperVCmd::ReleaseKey(0x12));
                        }
                        ui.separator();
                    }

                    // VirtualBox toolbar buttons
                    if let Some(vsession) = &self.vbox_session {
                        if ui.small_button("Ctrl+V").on_hover_text("Вставить текст из буфера обмена").clicked() {
                            let text = clipboard_read_text();
                            if !text.is_empty() {
                                vsession.send(virtualbox::VboxCmd::PutString(text));
                            }
                        }
                        if ui.small_button("Ctrl+Alt+Del").on_hover_text("Ctrl+Alt+Del в VM").clicked() {
                            // PS/2: Ctrl=0x1D, Alt=0x38, Del=0xE0 0x53
                            vsession.send(virtualbox::VboxCmd::PutScancodes(vec![0x1D], vec![]));
                            vsession.send(virtualbox::VboxCmd::PutScancodes(vec![0x38], vec![]));
                            vsession.send(virtualbox::VboxCmd::PutScancodes(vec![0xE0, 0x53], vec![]));
                            vsession.send(virtualbox::VboxCmd::PutScancodes(vec![], vec![0xE0, 0xD3]));
                            vsession.send(virtualbox::VboxCmd::PutScancodes(vec![], vec![0xB8]));
                            vsession.send(virtualbox::VboxCmd::PutScancodes(vec![], vec![0x9D]));
                        }
                        // VRDE quick-launch from toolbar
                        if let Some(idx) = self.hyperv_console_vm {
                            if let Some(vm) = self.hyperv_vms.get(idx) {
                                let vm_id = vm.id.clone();
                                let is_running = matches!(vm.state, hyperv::VmState::Running);
                                if let Some(&port) = self.vbox_vrde_ports.get(&vm_id) {
                                    if ui.small_button(format!("IR:{port}"))
                                        .on_hover_text(format!("Подключиться встроенным IronRDP на порту {port}"))
                                        .clicked()
                                    {
                                        self.start_vbox_vrde_session(port);
                                    }
                                } else {
                                    if ui.small_button("IR+")
                                        .on_hover_text("Включить VRDE и подключиться встроенным IronRDP")
                                        .clicked()
                                    {
                                        let port = Self::vbox_vrde_port(&vm_id);
                                        self.begin_vbox_vrde_connect(vm_id.clone(), port, is_running);
                                    }
                                }
                            }
                        }
                        ui.separator();
                    }

                    // Fullscreen toggle (right side)
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        #[cfg(windows)]
                        if vm_icon_button(ui, egui_phosphor::regular::GEAR, "Настройки подключения (VRDE)").clicked() {
                            self.vbox_vrde_settings_open = !self.vbox_vrde_settings_open;
                        }
                        let fs_label = if fullscreen {
                            egui_phosphor::regular::CORNERS_IN
                        } else {
                            egui_phosphor::regular::CORNERS_OUT
                        };
                        let fs_tip = if fullscreen { "Выйти из полного экрана" } else { "Развернуть консоль" };
                        if vm_icon_button(ui, fs_label, fs_tip).clicked() {
                            self.vm_console_fullscreen = !self.vm_console_fullscreen;
                        }
                        // Status text (right side)
                        if !self.hyperv_status.is_empty() {
                            ui.label(
                                egui::RichText::new(&self.hyperv_status.clone())
                                    .size(10.5)
                                    .color(palette.text_muted),
                            );
                        }
                    });
                });
            });

        #[cfg(windows)]
        if self.vbox_vrde_settings_open {
            let ctx = ui.ctx().clone();
            let mut open = self.vbox_vrde_settings_open;
            egui::Window::new("Настройки VRDE")
                .id(egui::Id::new("vrde_settings_window"))
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(&ctx, |ui| {
                    ui.label(
                        egui::RichText::new("Применяется при следующем подключении")
                            .small()
                            .color(palette.text_muted),
                    );
                    ui.add_space(6.0);

                    ui.label("Глубина цвета:");
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(self.vbox_vrde_settings.color_depth == 16, "16 бит")
                            .on_hover_text("Меньше данных по сети — рекомендация самой VirtualBox для RDP-клиентов")
                            .clicked()
                        {
                            self.vbox_vrde_settings.color_depth = 16;
                        }
                        if ui
                            .selectable_label(self.vbox_vrde_settings.color_depth == 32, "32 бита")
                            .on_hover_text("Точные цвета, больше данных по сети")
                            .clicked()
                        {
                            self.vbox_vrde_settings.color_depth = 32;
                        }
                    });

                    ui.add_space(8.0);
                    ui.label("Сжатие (MPPC):");
                    ui.horizontal(|ui| {
                        for choice in [vbox_rdp::CompressionChoice::K8, vbox_rdp::CompressionChoice::K64] {
                            if ui
                                .selectable_label(self.vbox_vrde_settings.compression == choice, choice.label())
                                .clicked()
                            {
                                self.vbox_vrde_settings.compression = choice;
                            }
                        }
                    });

                    ui.add_space(10.0);
                    if let Some(port) = self.vbox_vrde_active_port {
                        if ui
                            .button("Переподключиться с новыми настройками")
                            .on_hover_text("Закроет текущую сессию и откроет новую с выбранными параметрами")
                            .clicked()
                        {
                            self.vbox_vrde_active_port = None; // bypass the no-op dedup guard
                            self.start_vbox_vrde_session(port);
                        }
                    }
                });
            self.vbox_vrde_settings_open = open;
        }

        // Pointing-device advisory (PS/2 vs USB Tablet) — separate from hyperv_status
        // so it never gets clobbered by the busy "connecting..."/fps text above.
        if let Some(warning) = self.vbox_mouse_warning.clone() {
            ui.add_space(2.0);
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(0x60, 0x40, 0x00, 0x40))
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!("⚠ {warning}"))
                            .small()
                            .color(egui::Color32::from_rgb(0xFF, 0xC1, 0x07)),
                    );
                });
        }

        // ── No session: placeholder ───────────────────────────────────────────
        if self.hyperv_session.is_none()
            && self.vbox_session.is_none()
            && self.vbox_vrde_session.is_none()
            && self.hyperv_rdp_session.is_none()
        {
            let avail = ui.available_size();
            let r = ui.allocate_rect(
                egui::Rect::from_min_size(ui.cursor().min, avail),
                egui::Sense::hover(),
            );
            ui.painter().rect_filled(
                r.rect,
                egui::CornerRadius::same(0),
                egui::Color32::from_rgb(0x0D, 0x12, 0x1C),
            );
            ui.painter().text(
                r.rect.center(),
                egui::Align2::CENTER_CENTER,
                "Выберите VM и нажмите [>] для предпросмотра",
                egui::FontId::proportional(14.0),
                palette.text_muted,
            );
            return;
        }

        // ── Canvas (texture) ──────────────────────────────────────────────────
        let avail = ui.available_size();
        if self.vbox_vrde_session.is_some() {
            let desired = vrde_desktop_size_for_canvas(avail);
            // Hysteresis: every VrdeCmd::Resize that actually changes the
            // negotiated size makes VirtualBox issue a fresh
            // ServerDeactivateAll (reactivation) — and reactivation is where
            // the truncated-PDU VirtualBox quirk lives, costing multi-second
            // recovery when it hits. `avail` jitters by a few pixels between
            // frames (font metrics, focus-border width, etc.), and at 1.25x
            // scale that was enough to toggle the request just past the 1920
            // clamp boundary and back — silently triggering a reactivation
            // cycle on nearly every frame, with no visible cause from the
            // user's side. Only resize when the request actually moved by a
            // meaningful amount.
            let changed_enough = match self.vbox_vrde_last_desktop_size {
                Some((lw, lh)) => desired.0.abs_diff(lw) > 16 || desired.1.abs_diff(lh) > 16,
                None => true,
            };
            if changed_enough {
                if let Some(vrdp) = &self.vbox_vrde_session {
                    vrdp.send(vbox_rdp::VrdeCmd::Resize {
                        width: desired.0,
                        height: desired.1,
                    });
                }
                self.vbox_vrde_last_desktop_size = Some(desired);
            }
        }
        if let Some(tex) = &self.hyperv_texture {
            let img = egui::Image::new(tex)
                .fit_to_exact_size(avail)
                .maintain_aspect_ratio(true)
                .sense(egui::Sense::click_and_drag());
            let resp = ui.add(img);

            let tex_size = tex.size();
            let rect = fitted_image_rect(resp.rect, tex_size[0] as f32, tex_size[1] as f32);
            let (vm_pointer_pressed_inside, vm_pointer_pressed_outside) = ui.input(|i| {
                let mut inside = false;
                let mut outside = false;
                for ev in &i.raw.events {
                    if let egui::Event::PointerButton {
                        pos,
                        pressed: true,
                        ..
                    } = ev
                    {
                        if rect.contains(*pos) {
                            inside = true;
                        } else {
                            outside = true;
                        }
                    }
                }
                (inside, outside)
            });

            if vm_pointer_pressed_inside {
                self.vm_console_input_active = true;
                self.vm_console_last_pointer_pos = None;
                resp.request_focus();
            } else if vm_pointer_pressed_outside {
                self.vm_console_input_active = false;
                self.vm_console_last_pointer_pos = None;
            }

            let vm_console_focused = self.vm_console_input_active || vm_pointer_pressed_inside;

            // Focus border
            {
                let border_color = if vm_console_focused {
                    palette.accent
                } else {
                    egui::Color32::from_rgba_unmultiplied(0x80, 0x80, 0x80, 0x40)
                };
                ui.painter().rect_stroke(
                    resp.rect,
                    egui::CornerRadius::same(0),
                    egui::Stroke::new(2.0, border_color),
                    egui::StrokeKind::Outside,
                );
            }

            // HyperV mouse + keyboard
            if let Some(session) = &self.hyperv_session {
                if vm_console_focused {
                    let mut mouse_cmds = Vec::new();
                    let mut last_pointer_pos = self.vm_console_last_pointer_pos;
                    let mut pending_pointer_move: Option<(u32, u32)> = None;
                    let (guest_w_px, guest_h_px) = self.hyperv_guest_size.unwrap_or((
                        tex_size[0].max(1) as u32,
                        tex_size[1].max(1) as u32,
                    ));

                    let to_hyperv_abs = |pos: egui::Pos2| -> Option<(u32, u32)> {
                        if !rect.contains(pos) || rect.width() <= 0.0 || rect.height() <= 0.0 {
                            return None;
                        }
                        let guest_w = guest_w_px.max(1) as f32;
                        let guest_h = guest_h_px.max(1) as f32;
                        let nx = ((pos.x - rect.left()) / rect.width() * guest_w)
                            .round()
                            .clamp(0.0, guest_w - 1.0) as u32;
                        let ny = ((pos.y - rect.top()) / rect.height() * guest_h)
                            .round()
                            .clamp(0.0, guest_h - 1.0) as u32;
                        Some((nx, ny))
                    };

                    let flush_move = |cmds: &mut Vec<hyperv::HyperVCmd>,
                                      last_pointer_pos: &mut Option<(u16, u16)>,
                                      pending: &mut Option<(u32, u32)>| {
                        if let Some((nx, ny)) = pending.take() {
                            let pos_key = (nx as u16, ny as u16);
                            if *last_pointer_pos != Some(pos_key) {
                                cmds.push(hyperv::HyperVCmd::MoveMouse(nx, ny));
                                *last_pointer_pos = Some(pos_key);
                            }
                        }
                    };

                    ui.input(|i| {
                        for ev in &i.raw.events {
                            match ev {
                                egui::Event::PointerMoved(pos) => {
                                    if let Some(abs) = to_hyperv_abs(*pos) {
                                        pending_pointer_move = Some(abs);
                                    }
                                }
                                egui::Event::PointerButton {
                                    pos,
                                    button,
                                    pressed,
                                    ..
                                } => {
                                    flush_move(
                                        &mut mouse_cmds,
                                        &mut last_pointer_pos,
                                        &mut pending_pointer_move,
                                    );
                                    if let Some((nx, ny)) = to_hyperv_abs(*pos) {
                                        let pos_key = (nx as u16, ny as u16);
                                        if last_pointer_pos != Some(pos_key) {
                                            mouse_cmds.push(hyperv::HyperVCmd::MoveMouse(nx, ny));
                                            last_pointer_pos = Some(pos_key);
                                        }
                                        if !pressed {
                                            let btn = match button {
                                                egui::PointerButton::Primary => Some(1),
                                                egui::PointerButton::Secondary => Some(2),
                                                egui::PointerButton::Middle => Some(3),
                                                _ => None,
                                            };
                                            if let Some(btn) = btn {
                                                mouse_cmds.push(hyperv::HyperVCmd::MouseClick(btn));
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    });

                    flush_move(
                        &mut mouse_cmds,
                        &mut last_pointer_pos,
                        &mut pending_pointer_move,
                    );

                    self.vm_console_last_pointer_pos = last_pointer_pos;
                    for cmd in mouse_cmds {
                        session.send(cmd);
                    }
                }
                if vm_console_focused {
                    // Scroll wheel
                    ui.input(|i| {
                        for ev in &i.raw.events {
                            if let egui::Event::MouseWheel { delta, .. } = ev {
                                let vk = if delta.y > 0.0 { 0x21u32 } else { 0x22u32 };
                                session.send(hyperv::HyperVCmd::PressKey(vk));
                                session.send(hyperv::HyperVCmd::ReleaseKey(vk));
                            }
                        }
                    });
                    // Keyboard
                    const VK_SHIFT: u32 = 0x10;
                    ui.input(|i| {
                        for ev in &i.raw.events {
                            match ev {
                                egui::Event::Text(t) => {
                                    for c in t.chars() {
                                        match crate::vm_bridge::char_to_vk_shift(c) {
                                            Some((vk, shift)) => {
                                                if shift { session.send(hyperv::HyperVCmd::PressKey(VK_SHIFT)); }
                                                session.send(hyperv::HyperVCmd::PressKey(vk));
                                                session.send(hyperv::HyperVCmd::ReleaseKey(vk));
                                                if shift { session.send(hyperv::HyperVCmd::ReleaseKey(VK_SHIFT)); }
                                            }
                                            None => {
                                                session.send(hyperv::HyperVCmd::TypeText(c.to_string()));
                                            }
                                        }
                                    }
                                }
                                egui::Event::Key { key, pressed, modifiers, .. } => {
                                    let has_combo = modifiers.ctrl || modifiers.alt || modifiers.command;
                                    let vk_opt = egui_key_to_vkcode(*key)
                                        .or_else(|| if has_combo { egui_letter_to_vkcode(*key) } else { None });
                                    let Some(vk) = vk_opt else { continue; };
                                    let mut mods: Vec<u32> = Vec::new();
                                    if modifiers.ctrl    { mods.push(0x11); }
                                    if modifiers.alt     { mods.push(0x12); }
                                    if modifiers.shift   { mods.push(VK_SHIFT); }
                                    if modifiers.command { mods.push(0x5B); }
                                    if *pressed {
                                        for m in &mods { session.send(hyperv::HyperVCmd::PressKey(*m)); }
                                        session.send(hyperv::HyperVCmd::PressKey(vk));
                                        session.send(hyperv::HyperVCmd::ReleaseKey(vk));
                                        for m in mods.iter().rev() { session.send(hyperv::HyperVCmd::ReleaseKey(*m)); }
                                    }
                                }
                                _ => {}
                            }
                        }
                    });
                }
            }

            // VirtualBox screenshotpng: absolute mouse + keyboard
            if let Some(vsession) = &self.vbox_session {
                if vm_console_focused {
                    if let Some(pos) = resp.hover_pos() {
                        if rect.contains(pos) {
                            let nx = ((pos.x - rect.left()) / rect.width() * 65535.0).clamp(0.0, 65535.0) as u32;
                            let ny = ((pos.y - rect.top()) / rect.height() * 65535.0).clamp(0.0, 65535.0) as u32;
                            vsession.send(virtualbox::VboxCmd::MoveMouseAbs(nx, ny));
                        }
                    }
                }
                if resp.clicked() {
                    vsession.send(virtualbox::VboxCmd::MouseButton("leftdown".to_owned()));
                    vsession.send(virtualbox::VboxCmd::MouseButton("leftup".to_owned()));
                }
                if resp.secondary_clicked() {
                    vsession.send(virtualbox::VboxCmd::MouseButton("rightdown".to_owned()));
                    vsession.send(virtualbox::VboxCmd::MouseButton("rightup".to_owned()));
                }
                if vm_console_focused {
                    ui.input(|i| {
                        for ev in &i.raw.events {
                            match ev {
                                egui::Event::Text(t) if !t.is_empty() => {
                                    vsession.send(virtualbox::VboxCmd::PutString(t.clone()));
                                }
                                egui::Event::Key { key, pressed: true, .. } => {
                                    if let Some(kid) = egui_key_to_vbox_key_id(*key) {
                                        if let Some((make, brk)) = virtualbox::special_key_to_scancodes(kid) {
                                            vsession.send(virtualbox::VboxCmd::PutScancodes(make, brk));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    });
                }
            }

            // Pixel-accurate mouse + keyboard for the ironRDP-driven consoles
            // (VirtualBox VRDE and Hyper-V Enhanced Session both speak the same
            // VrdeCmd input vocabulary, so input collection is shared and the
            // assembled commands are dispatched to whichever session is live).
            if self.vbox_vrde_session.is_some() || self.hyperv_rdp_session.is_some() {
                if vm_console_focused {
                    let mut vrde_cmds = Vec::new();
                    let mut last_pointer_pos = self.vm_console_last_pointer_pos;
                    let mut pending_pointer_move: Option<(u16, u16)> = None;

                    ui.input(|i| {
                        let has_text = i
                            .raw
                            .events
                            .iter()
                            .any(|ev| matches!(ev, egui::Event::Text(t) if !t.is_empty()));
                        for ev in &i.raw.events {
                            match ev {
                                egui::Event::PointerMoved(pos) => {
                                    if let Some((gx, gy)) = pointer_to_guest_pixel(
                                        *pos,
                                        rect,
                                        tex_size[0] as f32,
                                        tex_size[1] as f32,
                                    ) {
                                        pending_pointer_move = Some((gx, gy));
                                    }
                                }
                                egui::Event::PointerButton {
                                    pos,
                                    button,
                                    pressed,
                                    ..
                                } => {
                                    if let Some((gx, gy)) = pending_pointer_move.take() {
                                        let pos = (gx, gy);
                                        if last_pointer_pos != Some(pos) {
                                            vrde_cmds.push(vbox_rdp::VrdeCmd::MouseMove { x: gx, y: gy });
                                            last_pointer_pos = Some(pos);
                                        }
                                    }
                                    if let Some((gx, gy)) = pointer_to_guest_pixel(
                                        *pos,
                                        rect,
                                        tex_size[0] as f32,
                                        tex_size[1] as f32,
                                    ) {
                                        let Some(button) = egui_pointer_to_vrde_button(*button) else {
                                            continue;
                                        };
                                        let pos = (gx, gy);
                                        if last_pointer_pos != Some(pos) {
                                            vrde_cmds.push(vbox_rdp::VrdeCmd::MouseMove { x: gx, y: gy });
                                            last_pointer_pos = Some(pos);
                                        }
                                        vrde_cmds.push(vbox_rdp::VrdeCmd::MouseButton {
                                            button,
                                            down: *pressed,
                                        });
                                    }
                                }
                                egui::Event::Text(t) if !t.is_empty() => {
                                    if let Some((gx, gy)) = pending_pointer_move.take() {
                                        let pos = (gx, gy);
                                        if last_pointer_pos != Some(pos) {
                                            vrde_cmds.push(vbox_rdp::VrdeCmd::MouseMove { x: gx, y: gy });
                                            last_pointer_pos = Some(pos);
                                        }
                                    }
                                    for ch in t.chars() {
                                        // VirtualBox VRDE doesn't implement Unicode keyboard
                                        // input PDUs (confirmed: arrows/shortcuts work via
                                        // scancode, plain letters via Unicode silently did
                                        // nothing). Route ASCII through the scancode path that
                                        // is known to work; only non-ASCII (Cyrillic etc., no
                                        // US-layout scancode) falls back to Unicode.
                                        if let Some((scancode, shift, extended)) = vbox_rdp::char_to_rdp_scancode(ch) {
                                            if shift {
                                                vrde_cmds.push(vbox_rdp::VrdeCmd::KeyDown { scancode: 0x2A, extended: false });
                                            }
                                            vrde_cmds.push(vbox_rdp::VrdeCmd::KeyDown { scancode, extended });
                                            vrde_cmds.push(vbox_rdp::VrdeCmd::KeyUp { scancode, extended });
                                            if shift {
                                                vrde_cmds.push(vbox_rdp::VrdeCmd::KeyUp { scancode: 0x2A, extended: false });
                                            }
                                        } else {
                                            vrde_cmds.push(vbox_rdp::VrdeCmd::Text(ch.to_string()));
                                        }
                                    }
                                }
                                egui::Event::MouseWheel { unit, delta, .. } => {
                                    if let Some((gx, gy)) = pending_pointer_move.take() {
                                        let pos = (gx, gy);
                                        if last_pointer_pos != Some(pos) {
                                            vrde_cmds.push(vbox_rdp::VrdeCmd::MouseMove { x: gx, y: gy });
                                            last_pointer_pos = Some(pos);
                                        }
                                    }
                                    // RDP wheel rotation units: Windows' WHEEL_DELTA
                                    // (120) per notch is the de-facto standard scale.
                                    // egui's `Line`/`Page` units don't map to a pixel
                                    // count the way `Point` does, so just treat the
                                    // sign consistently and scale a "line" as one
                                    // notch, a "page" as several.
                                    let notches = match unit {
                                        egui::MouseWheelUnit::Point => delta.y / 40.0,
                                        egui::MouseWheelUnit::Line => delta.y,
                                        egui::MouseWheelUnit::Page => delta.y * 8.0,
                                    };
                                    if notches.abs() >= 0.01 {
                                        // The wire format packs the magnitude into a
                                        // single byte (see ironrdp_pdu::input::mouse::MousePdu::encode),
                                        // so the encodable range is -255..=255 regardless
                                        // of how large `notches` is.
                                        let wheel_delta = (notches * 120.0).clamp(-255.0, 255.0) as i16;
                                        if wheel_delta != 0 {
                                            vrde_cmds.push(vbox_rdp::VrdeCmd::MouseWheel { delta: wheel_delta });
                                        }
                                    }
                                }
                                egui::Event::Key { key, pressed, modifiers, .. } => {
                                    if let Some((gx, gy)) = pending_pointer_move.take() {
                                        let pos = (gx, gy);
                                        if last_pointer_pos != Some(pos) {
                                            vrde_cmds.push(vbox_rdp::VrdeCmd::MouseMove { x: gx, y: gy });
                                            last_pointer_pos = Some(pos);
                                        }
                                    }
                                    let combo = modifiers.ctrl || modifiers.alt || modifiers.command;
                                    if has_text && !combo && egui_key_is_plain_text(*key) {
                                        continue;
                                    }

                                    if let Some((scancode, extended)) = egui_key_to_rdp_scancode(*key) {
                                        let mut mods = Vec::new();
                                        if modifiers.ctrl {
                                            mods.push((0x1D, false));
                                        }
                                        if modifiers.alt {
                                            mods.push((0x38, false));
                                        }
                                        if modifiers.shift {
                                            mods.push((0x2A, false));
                                        }
                                        if modifiers.command {
                                            mods.push((0x5B, true));
                                        }

                                        if *pressed {
                                            for (m_scancode, m_extended) in &mods {
                                                vrde_cmds.push(vbox_rdp::VrdeCmd::KeyDown {
                                                    scancode: *m_scancode,
                                                    extended: *m_extended,
                                                });
                                            }
                                            vrde_cmds.push(vbox_rdp::VrdeCmd::KeyDown { scancode, extended });
                                        } else {
                                            vrde_cmds.push(vbox_rdp::VrdeCmd::KeyUp { scancode, extended });
                                            for (m_scancode, m_extended) in mods.iter().rev() {
                                                vrde_cmds.push(vbox_rdp::VrdeCmd::KeyUp {
                                                    scancode: *m_scancode,
                                                    extended: *m_extended,
                                                });
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    });
                    if let Some((gx, gy)) = pending_pointer_move.take() {
                        let pos = (gx, gy);
                        if last_pointer_pos != Some(pos) {
                            vrde_cmds.push(vbox_rdp::VrdeCmd::MouseMove { x: gx, y: gy });
                            last_pointer_pos = Some(pos);
                        }
                    }

                    self.vm_console_last_pointer_pos = last_pointer_pos;
                    if !vrde_cmds.is_empty() {
                        self.vbox_vrde_last_input = Some(std::time::Instant::now());
                    }
                    for cmd in vrde_cmds {
                        if let Some(vrdp) = &self.vbox_vrde_session {
                            vrdp.send(cmd);
                        } else if let Some(rdp) = &self.hyperv_rdp_session {
                            rdp.send(cmd);
                        }
                    }
                }
            }

            // Bottom status bar
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgba_unmultiplied(0x0D, 0x12, 0x1C, 0xCC))
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            if self.hyperv_fps_display > 0.0 {
                                ui.label(egui::RichText::new(format!("{:.1} fps", self.hyperv_fps_display)).size(10.5).color(
                                    if self.hyperv_fps_display >= 1.5 { palette.success } else { palette.warning }
                                ));
                                ui.separator();
                            }
                            if let Some(idx) = self.hyperv_console_vm {
                                if let Some(vm) = self.hyperv_vms.get(idx) {
                                    let p = match vm.provider {
                                        hyperv::VmProvider::HyperV => "Hyper-V",
                                        hyperv::VmProvider::VirtualBox => "VirtualBox",
                                        hyperv::VmProvider::VMware => "VMware",
                                    };
                                    ui.label(egui::RichText::new(p).size(10.5).color(palette.text_muted));
                                }
                            }
                            if let Some(tex) = &self.hyperv_texture {
                                let [w, h] = tex.size();
                                ui.label(egui::RichText::new(format!("{}x{}", w, h)).size(10.5).color(palette.text_muted));
                            }
                        });
                    });
            });
        } else {
            // Placeholder while first frame loads
            let r = ui.allocate_rect(
                egui::Rect::from_min_size(ui.cursor().min, avail),
                egui::Sense::hover(),
            );
            ui.painter().rect_filled(r.rect, egui::CornerRadius::same(0), egui::Color32::from_rgb(0x0D, 0x12, 0x1C));
            ui.painter().text(
                r.rect.center(),
                egui::Align2::CENTER_CENTER,
                "Загрузка экрана VM...",
                egui::FontId::proportional(14.0),
                palette.text_weak,
            );
        }
    }


    /// Universal Provider Dashboard — shows all registered providers and their VMs.
    /// Запустить фоновый rebuild дашборда. Вызовы провайдеров (WMI/VBoxManage)
    /// происходят в отдельном потоке — UI не блокируется.
    #[cfg(windows)]
    fn start_dashboard_rebuild(&mut self) {
        if self.dashboard_load_rx.is_some() {
            return; // уже в процессе
        }
        let registry = Arc::clone(&self.provider_registry);
        let (tx, rx) = mpsc::channel();
        self.dashboard_load_rx = Some(rx);
        thread::spawn(move || {
            let providers = registry.all();
            let mut out = Vec::with_capacity(providers.len());
            let mut total_vms = 0usize;
            for p in &providers {
                let reachable = p.list_hosts().is_ok();
                let hosts = p.list_hosts().unwrap_or_default();
                let mut vms = Vec::new();
                for host in &hosts {
                    for vm in p.list_vms(&host.host_id).unwrap_or_default() {
                        total_vms += 1;
                        let badge = p.get_capabilities(&vm.vm_id).ok().map(|g| {
                            (g.recommended_mode.label().to_owned(), g.recommended_mode.badge_rgb())
                        });
                        let ip = vm.primary_ip().map(|s| s.to_owned());
                        let name = vm.name.clone();
                        vms.push(DashVm {
                            name,
                            power_state: vm.power_state,
                            badge,
                            ip,
                        });
                    }
                }
                out.push(DashProvider {
                    ptype: p.provider_type(),
                    pid: p.provider_id().to_owned(),
                    reachable,
                    vms,
                });
            }
            let _ = tx.send(DashSnapshot {
                at: std::time::Instant::now(),
                providers: out,
                total_vms,
            });
        });
    }

    #[cfg(windows)]
    fn stop_vm_console_sessions(&mut self) {
        self.stop_vm_console_sessions_keep_focus();
        self.vm_console_input_active = false;
        self.vm_console_last_pointer_pos = None;
        self.hyperv_guest_size = None;
    }

    /// Same as `stop_vm_console_sessions` but leaves keyboard/mouse focus state
    /// untouched. Auto-reconnect (after an MPPC decoder desync) calls this:
    /// resetting `vm_console_input_active` on every reconnect made the console
    /// silently drop input focus on every blink, even though the user never
    /// clicked away — input just stopped reaching the VM until they clicked
    /// the console image again.
    #[cfg(windows)]
    fn stop_vm_console_sessions_keep_focus(&mut self) {
        if let Some(s) = self.hyperv_session.take() {
            s.stop();
        }
        if let Some(s) = self.vbox_session.take() {
            s.stop();
        }
        if let Some(s) = self.vbox_vrde_session.take() {
            s.stop();
        }
        self.vbox_vrde_last_desktop_size = None;
        if let Some(s) = self.hyperv_rdp_session.take() {
            s.stop();
        }
        self.vbox_vrde_enable_rx = None;
        self.vbox_mouse_warning_rx = None;
        self.vbox_mouse_warning = None;
        self.vbox_vrde_active_port = None;
    }

    #[cfg(windows)]
    fn vbox_vrde_port(vm_id: &str) -> u16 {
        3390 + (vm_id
            .bytes()
            .fold(0u16, |a, b| a.wrapping_add(b as u16))
            % 10)
    }

    #[cfg(windows)]
    fn start_vbox_vrde_session(&mut self, port: u16) {
        // Already connected to this exact port — re-clicking any of the several
        // "connect"/"open console" buttons must not tear down a live session.
        if self.vbox_vrde_session.is_some() && self.vbox_vrde_active_port == Some(port) {
            return;
        }
        // Deliberately NOT calling the focus-resetting stop_vm_console_sessions():
        // auto-reconnect (after a decoder desync) goes through this exact path,
        // and resetting vm_console_input_active on every blink made the console
        // silently stop accepting input until the user clicked it again — felt
        // like the whole UI lost focus, not just the image freezing.
        // Also deliberately NOT clearing hyperv_texture: that made every silent
        // reconnect flash black for the ~100-300ms TCP+TLS+RDP handshake. Keeping
        // the last good frame on screen during reconnect is strictly better —
        // callers switching to a genuinely different VM already clear the
        // texture themselves before calling this.
        self.stop_vm_console_sessions_keep_focus();
        self.hyperv_frame_count = 0;
        self.hyperv_fps_display = 0.0;
        self.hyperv_fps_window = std::time::Instant::now();
        self.hyperv_last_frame = std::time::Instant::now();
        self.hyperv_status = format!("VRDE/IronRDP: подключение к 127.0.0.1:{port}...");
        let desktop_size = default_vrde_desktop_size();
        self.vbox_vrde_last_desktop_size = Some(desktop_size);
        self.vbox_vrde_active_port = Some(port);
        self.vbox_vrde_last_frame = std::time::Instant::now();
        self.vbox_vrde_last_input = None;
        self.vbox_vrde_session = Some(vbox_rdp::VrdeSession::connect(
            "127.0.0.1",
            port,
            desktop_size,
            self.vbox_vrde_settings,
        ));
    }

    #[cfg(windows)]
    fn begin_vbox_vrde_connect(&mut self, vm_id: String, port: u16, vm_running: bool) {
        // Already connected/connecting to this VM — ignore the duplicate request
        // instead of restarting (see `vbox_vrde_active_port` doc comment).
        if self.vbox_vrde_enable_rx.is_some()
            || (self.vbox_vrde_session.is_some() && self.vbox_vrde_active_port == Some(port))
        {
            return;
        }
        self.stop_vm_console_sessions();
        self.vbox_vrde_ports.insert(vm_id.clone(), port);
        self.hyperv_texture = None;
        self.hyperv_frame_count = 0;
        self.hyperv_fps_display = 0.0;
        self.hyperv_fps_window = std::time::Instant::now();
        self.hyperv_last_frame = std::time::Instant::now();
        self.hyperv_status = format!("VRDE: включение порта {port}...");

        let (tx, rx) = mpsc::channel();
        self.vbox_vrde_enable_rx = Some(rx);
        self.vbox_mouse_warning = None;
        let mouse_vm_id = vm_id.clone();
        thread::spawn(move || {
            // enable_vrde + send must happen first and only first — no extra
            // VBoxManage round-trips on this path. Any added latency here makes
            // users re-click "connect" before the session settles, and a second
            // VRDE client connecting kicks the first one off (VirtualBox allows
            // only one VRDE connection at a time), producing an endless
            // reconnect loop that looks like a black screen.
            let ok = virtualbox::enable_vrde(&vm_id, port, vm_running);
            let _ = tx.send((vm_id, port, ok));
        });

        // Pointing-device advisory runs fully independently, on its own thread,
        // and only ever produces a side-channel warning — never gates the connect.
        let (warn_tx, warn_rx) = mpsc::channel();
        self.vbox_mouse_warning_rx = Some(warn_rx);
        thread::spawn(move || {
            let warning = match virtualbox::pointing_device_is_absolute(&mouse_vm_id) {
                Some(false) if !vm_running => {
                    match virtualbox::set_pointing_device_usbtablet(&mouse_vm_id) {
                        Ok(()) => return,
                        Err(e) => format!("Не удалось переключить мышь на USB Tablet: {e}"),
                    }
                }
                Some(false) => "VM использует PS/2-мышь (относительные координаты) — курсор \
                     не будет двигаться по RDP. Выключите VM один раз: EvertyDesk переключит \
                     на USB Tablet, дальше работает всегда."
                    .to_owned(),
                _ => return,
            };
            let _ = warn_tx.send(warning);
        });
    }

    #[cfg(windows)]
    fn start_vbox_vrde_scan(&mut self) {
        let ids: Vec<String> = self
            .hyperv_vms
            .iter()
            .filter(|vm| matches!(vm.provider, hyperv::VmProvider::VirtualBox))
            .map(|vm| vm.id.clone())
            .collect();
        if ids.is_empty() {
            self.vbox_vrde_scan_rx = None;
            return;
        }

        let (tx, rx) = mpsc::channel();
        self.vbox_vrde_scan_rx = Some(rx);
        thread::spawn(move || {
            let mut ports = std::collections::HashMap::new();
            for id in ids {
                if let Some(info) = virtualbox::get_vrde_info(&id) {
                    if info.enabled {
                        ports.insert(id, info.port);
                    }
                }
            }
            let _ = tx.send(ports);
        });
    }

    #[cfg(windows)]
    fn universal_provider_dashboard_ui(&mut self, ui: &mut egui::Ui) {
        // Собираем результат фонового rebuild если готов.
        if let Some(rx) = &self.dashboard_load_rx {
            if let Ok(snap) = rx.try_recv() {
                self.dashboard_snapshot = Some(snap);
                self.dashboard_load_rx = None;
            }
        }

        // Запускаем фоновый rebuild если снимок устарел и rebuild не идёт.
        let stale = self
            .dashboard_snapshot
            .as_ref()
            .map(|s| s.at.elapsed() > Duration::from_secs(10))
            .unwrap_or(true);
        // Обновляем дашборд только если провайдеры зарегистрированы и данные устарели.
        // Пропускаем если список VM ещё загружается — нет смысла дублировать скан.
        if stale && self.dashboard_load_rx.is_none() && !self.hyperv_loading {
            self.start_dashboard_rebuild();
        }
        let Some(snapshot) = &self.dashboard_snapshot else { return; };

        if snapshot.providers.is_empty() {
            ui.label(
                egui::RichText::new("Нет зарегистрированных провайдеров")
                    .color(crate::theme::palette().text_muted)
                    .size(12.0),
            );
            return;
        }

        let total = snapshot.providers.len();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("Провайдеры: {total}"))
                    .size(12.0)
                    .color(crate::theme::palette().text_muted),
            );
            if snapshot.total_vms > 0 {
                ui.label(
                    egui::RichText::new(format!("| VM: {}", snapshot.total_vms))
                        .size(12.0)
                        .color(crate::theme::palette().text_muted),
                );
            }
        });
        ui.add_space(6.0);

        for provider in &snapshot.providers {
            let (prov_color, prov_label) = match &provider.ptype {
                provider_api::ProviderType::HyperV => (crate::theme::palette().info, "HYPER-V"),
                provider_api::ProviderType::VMware => (crate::theme::palette().info, "VMWARE"),
                provider_api::ProviderType::Proxmox => (crate::theme::palette().warning, "PROXMOX"),
                provider_api::ProviderType::Libvirt => (crate::theme::palette().success, "LIBVIRT"),
                _ => (crate::theme::palette().text_muted, "PROVIDER"),
            };

            egui::Frame::NONE
                .fill(crate::theme::palette().surface_raised)
                .stroke(egui::Stroke::new(1.0, crate::theme::palette().border))
                .corner_radius(egui::CornerRadius::same(crate::theme::radius::MD))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        egui::Frame::NONE
                            .fill(prov_color.gamma_multiply(0.22))
                            .corner_radius(egui::CornerRadius::same(4))
                            .inner_margin(egui::Margin::symmetric(5, 2))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(prov_label)
                                        .size(10.0)
                                        .strong()
                                        .color(prov_color),
                                );
                            });
                        ui.label(
                            egui::RichText::new(&provider.pid)
                                .size(12.0)
                                .color(crate::theme::palette().text),
                        );
                        let (status_dot, status_text) = if provider.reachable {
                            (crate::theme::palette().success, "Healthy")
                        } else {
                            (crate::theme::palette().danger, "Unavailable")
                        };
                        paint_status_dot(ui, status_dot);
                        ui.colored_label(status_dot, status_text);
                    });

                    for vm in &provider.vms {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let (dot_color, state_label) = match vm.power_state {
                                provider_api::PowerState::Running =>
                                    (crate::theme::palette().success, "Running"),
                                provider_api::PowerState::Stopped =>
                                    (crate::theme::palette().text_weak, "Stopped"),
                                provider_api::PowerState::Paused =>
                                    (crate::theme::palette().warning, "Paused"),
                                _ => (crate::theme::palette().text_weak, "Unknown"),
                            };
                            paint_status_dot(ui, dot_color);
                            ui.label(
                                egui::RichText::new(&vm.name)
                                    .size(12.0)
                                    .color(crate::theme::palette().text),
                            );
                            ui.label(
                                egui::RichText::new(state_label)
                                    .size(10.5)
                                    .color(crate::theme::palette().text_muted),
                            );
                            if let Some((label, (r, g, b))) = &vm.badge {
                                let badge_color = egui::Color32::from_rgb(*r, *g, *b);
                                egui::Frame::NONE
                                    .fill(badge_color.gamma_multiply(0.20))
                                    .corner_radius(egui::CornerRadius::same(3))
                                    .inner_margin(egui::Margin::symmetric(4, 1))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(label).size(9.5).color(badge_color),
                                        );
                                    });
                            }
                            if let Some(ip) = &vm.ip {
                                ui.label(
                                    egui::RichText::new(ip)
                                        .size(10.0)
                                        .color(crate::theme::palette().info),
                                );
                            }
                        });
                    }
                    if provider.vms.is_empty() {
                        ui.label(
                            egui::RichText::new(if provider.reachable {
                                "VM не обнаружены"
                            } else {
                                "Инвентарь недоступен"
                            })
                            .size(11.0)
                            .color(crate::theme::palette().text_muted),
                        );
                    }
                });
            ui.add_space(4.0);
        }
    }

    #[cfg(windows)]
    fn poll_hyperv_session(&mut self, ctx: &egui::Context) {
        if let Some(rx) = self.vbox_mouse_warning_rx.take() {
            match rx.try_recv() {
                Ok(w) => self.vbox_mouse_warning = Some(w),
                Err(mpsc::TryRecvError::Empty) => self.vbox_mouse_warning_rx = Some(rx),
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }

        if let Some(rx) = self.vbox_vrde_enable_rx.take() {
            match rx.try_recv() {
                Ok((vm_id, port, true)) => {
                    self.vbox_vrde_ports.insert(vm_id, port);
                    self.start_vbox_vrde_session(port);
                    ctx.request_repaint();
                }
                Ok((vm_id, port, false)) => {
                    self.vbox_vrde_ports.remove(&vm_id);
                    self.hyperv_status = format!(
                        "VRDE: не удалось включить порт {port}. Проверьте VirtualBox Extension Pack и свободен ли порт."
                    );
                    ctx.request_repaint();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.vbox_vrde_enable_rx = Some(rx);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.hyperv_status = "VRDE: поток включения завершился без результата".to_owned();
                    ctx.request_repaint();
                }
            }
        }

        if let Some(rx) = self.vbox_vrde_scan_rx.take() {
            match rx.try_recv() {
                Ok(ports) => {
                    for (vm_id, port) in ports {
                        self.vbox_vrde_ports.insert(vm_id, port);
                    }
                    ctx.request_repaint();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.vbox_vrde_scan_rx = Some(rx);
                }
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }

        // Collect async VM list
        if let Some(rx) = &self.hyperv_load_rx {
            if let Ok(vms) = rx.try_recv() {
                self.hyperv_vms = vms;
                self.hyperv_loading = false;
                self.hyperv_checked = true;
                self.hyperv_load_rx = None;
                self.hyperv_last_refresh = Some(std::time::Instant::now());
                // Register local HyperV provider in the universal ProviderRegistry
                // (done here so it's only registered when WMI confirmed to work)
                {
                    use std::sync::Arc;
                    let hv = Arc::new(hyperv::HyperVProvider::new());
                    self.provider_registry.register(hv);
                }
                self.start_vbox_vrde_scan();
                ctx.request_repaint();
            }
        }

        // Poll Hyper-V session frames (WMI thumbnail — preview only)
        if let Some(session) = &self.hyperv_session {
            while let Some(msg) = session.try_recv_status() {
                self.hyperv_status = msg;
            }
            {
                if let Some(frame) = session.try_recv_frame() {
                    let w = frame.width as usize;
                    let h = frame.height as usize;
                    self.hyperv_guest_size = Some((frame.guest_width, frame.guest_height));
                    if frame.rgba.len() == w * h * 4 {
                        let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &frame.rgba);
                        self.hyperv_texture = Some(ctx.load_texture(
                            "hyperv_frame",
                            img,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    self.hyperv_frame_count += 1;
                    self.hyperv_last_frame = std::time::Instant::now();
                    let fps_elapsed = self.hyperv_fps_window.elapsed();
                    if fps_elapsed >= Duration::from_secs(1) {
                        self.hyperv_fps_display = self.hyperv_frame_count as f32 / fps_elapsed.as_secs_f32();
                        self.hyperv_frame_count = 0;
                        self.hyperv_fps_window = std::time::Instant::now();
                    }
                    ctx.request_repaint();
                }
                // #7 Auto-reconnect: if no frame for 15s while session exists, restart
                if self.hyperv_last_frame.elapsed() > Duration::from_secs(15)
                    && self.hyperv_fps_display > 0.0
                {
                    if let Some(vm_idx) = self.hyperv_console_vm {
                        if let Some(vm) = self.hyperv_vms.get(vm_idx).cloned() {
                            if let Some(s) = self.hyperv_session.take() { s.stop(); }
                            self.hyperv_fps_display = 0.0;
                            self.hyperv_last_frame = std::time::Instant::now();
                            self.hyperv_status = "Авто-реконнект...".to_owned();
                            self.hyperv_session = Some(hyperv::HyperVSession::start(vm));
                            ctx.request_repaint();
                        }
                    }
                }
            }
        }

        // Poll VirtualBox screenshotpng session frames (fallback when VRDE not enabled)
        if let Some(session) = &self.vbox_session {
            while let Some(msg) = session.try_recv_status() {
                self.hyperv_status = msg;
            }
            if let Some((w, h, rgba)) = session.try_recv_frame() {
                let (wu, hu) = (w as usize, h as usize);
                if rgba.len() == wu * hu * 4 {
                    let img = egui::ColorImage::from_rgba_unmultiplied([wu, hu], &rgba);
                    self.hyperv_texture = Some(ctx.load_texture(
                        "vbox_frame",
                        img,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                ctx.request_repaint();
            }
        }

        // Poll VirtualBox VRDE session frames (30fps embedded RDP)
        if let Some(session) = &self.vbox_vrde_session {
            // Drain every pending status message in one pass. `Poll::Dead`
            // here means the session thread has actually exited (e.g. the
            // write-timeout fix catching a stalled socket write — see
            // vbox_rdp.rs): channels only disconnect when the thread
            // function returns, so this is a 100%-reliable, instant signal,
            // unlike every elapsed-time-based "is it stuck" heuristic tried
            // before (which couldn't tell "quiet because nothing changed"
            // from "quiet because it died" without either misfiring on
            // legitimate idle or waiting out a long timer).
            let mut desync_now = false;
            let mut dead = false;
            loop {
                match session.poll_status() {
                    vbox_rdp::Poll::Item(msg) => {
                        if msg == "VRDE_DESYNC" {
                            // Decoder hit the MPPC history-buffer-overflow bug
                            // — it never recovers on its own. Reconnect
                            // immediately instead of waiting for the
                            // frame-staleness timer below.
                            desync_now = true;
                        } else {
                            self.hyperv_status = msg;
                        }
                        ctx.request_repaint();
                    }
                    vbox_rdp::Poll::Empty => break,
                    vbox_rdp::Poll::Dead => {
                        dead = true;
                        break;
                    }
                }
            }
            if dead {
                if let Some(port) = self.vbox_vrde_active_port {
                    vbox_rdp::log_from_ui("VRDE: [main] reconnect trigger = session thread exited (channel disconnected)");
                    self.hyperv_status = "VRDE: сессия завершилась, переподключаюсь...".to_owned();
                    self.vbox_vrde_active_port = None;
                    self.start_vbox_vrde_session(port);
                    self.vbox_vrde_last_frame = std::time::Instant::now();
                    ctx.request_repaint();
                }
            } else if desync_now {
                // Decoder hit the MPPC history-buffer-overflow bug — it never
                // recovers on its own. Reconnect immediately instead of waiting
                // for the frame-staleness timer below.
                if let Some(port) = self.vbox_vrde_active_port {
                    vbox_rdp::log_from_ui("VRDE: [main] reconnect trigger = content-collapse (VRDE_DESYNC status from session thread)");
                    self.hyperv_status = "VRDE: декодер застрял, переподключаюсь...".to_owned();
                    self.vbox_vrde_active_port = None;
                    self.start_vbox_vrde_session(port);
                    self.vbox_vrde_last_frame = std::time::Instant::now();
                    ctx.request_repaint();
                }
            } else {
                // Drain to the newest frame rather than processing just one
                // per UI tick: the decoder can produce frames faster than
                // egui repaints, and taking only the oldest queued frame each
                // time means we permanently lag behind during any burst of
                // activity (this is exactly what `fdrops` climbing in the
                // logs was — the channel filling up because we only ever
                // pulled one out per tick). Always showing the latest frame
                // and discarding the stale ones in between is strictly
                // smoother: the skipped frames were never going to be seen
                // anyway once a newer one exists.
                let mut latest_frame = None;
                loop {
                    match session.poll_frame() {
                        vbox_rdp::Poll::Item(frame) => latest_frame = Some(frame),
                        vbox_rdp::Poll::Empty => break,
                        vbox_rdp::Poll::Dead => {
                            dead = true;
                            break;
                        }
                    }
                }
                if dead {
                    if let Some(port) = self.vbox_vrde_active_port {
                        vbox_rdp::log_from_ui("VRDE: [main] reconnect trigger = session thread exited (channel disconnected, seen via frame channel)");
                        self.hyperv_status = "VRDE: сессия завершилась, переподключаюсь...".to_owned();
                        self.vbox_vrde_active_port = None;
                        self.start_vbox_vrde_session(port);
                        self.vbox_vrde_last_frame = std::time::Instant::now();
                        ctx.request_repaint();
                    }
                }
                if let Some((w, h, rgba)) = latest_frame {
                    let (wu, hu) = (w as usize, h as usize);
                    if rgba.len() == wu * hu * 4 {
                        // Mark the connection as alive regardless — this also
                        // covers a fresh reconnect's first frame(s), which are
                        // legitimately near-empty for the brief moment before
                        // real bitmap data streams in (the new DecodedImage
                        // starts blank). Counting that as "activity" prevents
                        // the staleness detector from immediately re-triggering
                        // another reconnect on top of the one just finished.
                        self.vbox_vrde_last_frame = std::time::Instant::now();

                        // But don't actually display it: a near-empty frame
                        // would otherwise flash black for that one frame
                        // before the next, real-content frame arrives. Sample
                        // a sparse grid and keep showing the previous texture
                        // until something substantial comes in.
                        let pixel_count = wu * hu;
                        let substantial = if pixel_count == 0 {
                            false
                        } else {
                            const SAMPLES: usize = 256;
                            let step = (pixel_count / SAMPLES).max(1);
                            let mut nonzero = 0usize;
                            let mut checked = 0usize;
                            let mut px = 0usize;
                            while px < pixel_count {
                                let off = px * 4;
                                if rgba[off] != 0 || rgba[off + 1] != 0 || rgba[off + 2] != 0 {
                                    nonzero += 1;
                                }
                                checked += 1;
                                px += step;
                            }
                            nonzero as f64 / checked.max(1) as f64 > 0.05
                        };

                        if substantial || self.hyperv_texture.is_none() {
                            let img = egui::ColorImage::from_rgba_unmultiplied([wu, hu], &rgba);
                            self.hyperv_texture = Some(ctx.load_texture(
                                "vbox_vrde_frame",
                                img,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                        // FPS counter reuse
                        self.hyperv_frame_count += 1;
                        let fps_elapsed = self.hyperv_fps_window.elapsed();
                        if fps_elapsed >= Duration::from_secs(1) {
                            self.hyperv_fps_display = self.hyperv_frame_count as f32 / fps_elapsed.as_secs_f32();
                            self.hyperv_frame_count = 0;
                            self.hyperv_fps_window = std::time::Instant::now();
                        }
                        ctx.request_repaint();
                    }
                }
                // Dropped the "stuck despite active input" heuristic, and
                // deliberately do NOT use a short timer here either —
                // measured directly (down to 300ms): even a perfectly healthy
                // session has legitimate multi-hundred-ms gaps between server
                // frames whenever the guest screen genuinely isn't changing
                // (cursor movement alone is composited client-side, no server
                // round-trip). Elapsed-time-since-last-frame cannot
                // distinguish "quiet because nothing changed" from "quiet
                // because it's broken" at ANY threshold short enough to feel
                // responsive — both produce the exact same signal. The
                // content-collapse detector above is evidence-based (it only
                // fires on an actual measured drop in real pixel data) and is
                // the correct primary signal; this is a true last-resort for
                // a socket that goes silently dead for a very long time.
                let stuck_regardless = self.vbox_vrde_last_frame.elapsed() > Duration::from_secs(60);

                if stuck_regardless {
                    if let Some(port) = self.vbox_vrde_active_port {
                        vbox_rdp::log_from_ui(&format!(
                            "VRDE: [main] reconnect trigger = stuck_regardless (60s safety net) last_frame_ms_ago={}",
                            self.vbox_vrde_last_frame.elapsed().as_millis(),
                        ));
                        self.hyperv_status = "VRDE: авто-реконнект (декодер застрял)...".to_owned();
                        self.vbox_vrde_active_port = None; // bypass the no-op dedup guard
                        self.start_vbox_vrde_session(port);
                        self.vbox_vrde_last_frame = std::time::Instant::now();
                        ctx.request_repaint();
                    }
                }
            }
        }

        // Poll Hyper-V Enhanced Session (RDP over VMBus) frames — same handling
        // as the VRDE path (it produces the same (w,h,rgba) frames + status,
        // and dies the same way via channel disconnect).
        if let Some(session) = &self.hyperv_rdp_session {
            let mut dead = false;
            loop {
                match session.poll_status() {
                    vbox_rdp::Poll::Item(msg) => {
                        self.hyperv_status = msg;
                        ctx.request_repaint();
                    }
                    vbox_rdp::Poll::Empty => break,
                    vbox_rdp::Poll::Dead => {
                        dead = true;
                        break;
                    }
                }
            }
            if dead {
                if let Some(s) = self.hyperv_rdp_session.take() {
                    s.stop();
                }
                self.hyperv_status = "RDP: сессия завершилась".to_owned();
                ctx.request_repaint();
            } else {
                let mut latest = None;
                loop {
                    match session.poll_frame() {
                        vbox_rdp::Poll::Item(f) => latest = Some(f),
                        vbox_rdp::Poll::Empty => break,
                        vbox_rdp::Poll::Dead => {
                            dead = true;
                            break;
                        }
                    }
                }
                if dead {
                    if let Some(s) = self.hyperv_rdp_session.take() {
                        s.stop();
                    }
                    self.hyperv_status = "RDP: сессия завершилась".to_owned();
                    ctx.request_repaint();
                } else if let Some((w, h, rgba)) = latest {
                    let (wu, hu) = (w as usize, h as usize);
                    if rgba.len() == wu * hu * 4 {
                        let img = egui::ColorImage::from_rgba_unmultiplied([wu, hu], &rgba);
                        self.hyperv_texture = Some(ctx.load_texture(
                            "hyperv_rdp_frame",
                            img,
                            egui::TextureOptions::LINEAR,
                        ));
                        ctx.request_repaint();
                    }
                }
            }
        }

        // Keep egui awake while any VM session is active
        if self.hyperv_session.is_some()
            || self.vbox_session.is_some()
            || self.vbox_vrde_session.is_some()
            || self.hyperv_rdp_session.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(16)); // 60fps polling
        }
    }

    fn history_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new(self.text("История подключений", "Connection history"))
                .size(28.0)
                .strong()
                .color(crate::theme::palette().text),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(self.text(
                "Последние ID сохраняются локально. Можно добавить заметку.",
                "Recent IDs are stored locally. You can add a note.",
            ))
            .size(13.0)
            .color(crate::theme::palette().text_weak),
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
                                    .color(crate::theme::palette().text),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}: {}",
                                    tr(self.ui_lang, "Подключений", "Connections"),
                                    entry.connect_count
                                ))
                                .size(12.0)
                                .color(crate::theme::palette().text_weak),
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
                            .color(crate::theme::palette().text_weak),
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

    /// Диалог подтверждения опасного действия питания VM (stop/shutdown/restart/pause).
    fn vm_action_confirm_window(&mut self, ctx: &egui::Context) {
        let Some((vm_id, action, vm_name, desc)) = self.pending_vm_action.clone() else {
            return;
        };
        let t = crate::theme::palette();
        // Цвет/значок зависят от опасности действия.
        let (accent, glyph) = match action.as_str() {
            "stop" => (t.danger, egui_phosphor::regular::STOP),
            "shutdown" => (t.warning, egui_phosphor::regular::POWER),
            "restart" => (t.info, egui_phosphor::regular::ARROWS_CLOCKWISE),
            _ => (t.text_weak, egui_phosphor::regular::PAUSE),
        };

        egui::Window::new("vm_action_confirm")
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .frame(
                egui::Frame::NONE
                    .fill(t.surface)
                    .stroke(egui::Stroke::new(1.0, t.border))
                    .corner_radius(egui::CornerRadius::same(crate::theme::radius::XL))
                    .inner_margin(egui::Margin::same(20))
                    .shadow(ctx.global_style().visuals.window_shadow),
            )
            .show(ctx, |ui| {
                ui.set_width(360.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        r,
                        egui::CornerRadius::same(crate::theme::radius::LG),
                        crate::theme::tint(accent, 0.18),
                    );
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        glyph,
                        egui::FontId::proportional(22.0),
                        accent,
                    );
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("Подтвердите действие").size(17.0).strong().color(t.text));
                        ui.label(
                            egui::RichText::new(vm_name.split(" · ").next().unwrap_or(&vm_name))
                                .size(12.0)
                                .color(t.text_weak),
                        );
                    });
                });
                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new(format!("Вы собираетесь {desc}."))
                        .size(13.5)
                        .color(t.text),
                );
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    let btn_w = (ui.available_width() - 10.0) / 2.0;
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Подтвердить").size(14.0).strong().color(t.accent_fg),
                            )
                            .fill(accent)
                            .min_size(egui::vec2(btn_w, 42.0))
                            .corner_radius(egui::CornerRadius::same(crate::theme::radius::MD)),
                        )
                        .clicked()
                    {
                        let payload =
                            serde_json::json!({ "vm_id": vm_id, "action": action }).to_string();
                        self.send_command(SessionCommand::VmPowerOp(payload));
                        self.pending_vm_action = None;
                    }
                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new("Отмена").size(14.0).color(t.text))
                                .min_size(egui::vec2(btn_w, 42.0))
                                .corner_radius(egui::CornerRadius::same(crate::theme::radius::MD)),
                        )
                        .clicked()
                    {
                        self.pending_vm_action = None;
                    }
                });
            });
    }

    fn incoming_approval_window(&mut self, ctx: &egui::Context) {
        let Some(peer_id) = self.host_pending_peer.clone() else {
            return;
        };
        let t = crate::theme::palette();

        // Кто подключается: если ID есть в адресной книге — показываем имя.
        let normalized = normalize_remote_id(&peer_id);
        let contact_name = self
            .config
            .ui
            .contacts
            .iter()
            .find(|c| normalize_remote_id(&c.remote_id) == normalized)
            .map(|c| c.name.clone());

        let title = self.text("Входящее подключение", "Incoming connection");
        let subtitle = self.text(
            "Запрос на удалённый доступ к этому компьютеру",
            "Request for remote access to this computer",
        );
        let allow_txt = self.text("Разрешить", "Allow");
        let reject_txt = self.text("Отклонить", "Reject");
        let id_caption = self.text("ID", "ID");

        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .title_bar(false) // свой заголовок — egui-titlebar выглядит чужеродно
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .frame(
                egui::Frame::NONE
                    .fill(t.surface)
                    .stroke(egui::Stroke::new(1.0, t.border))
                    .corner_radius(egui::CornerRadius::same(crate::theme::radius::XL))
                    .inner_margin(egui::Margin::same(20))
                    .shadow(ctx.global_style().visuals.window_shadow),
            )
            .show(ctx, |ui| {
                ui.set_width(360.0);

                // Шапка: иконка-аватар + заголовок.
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        r,
                        egui::CornerRadius::same(crate::theme::radius::LG),
                        crate::theme::accent_tint(&t, 0.18),
                    );
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        egui_phosphor::regular::PLUGS_CONNECTED,
                        egui::FontId::proportional(22.0),
                        t.accent,
                    );
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(title).size(17.0).strong().color(t.text));
                        ui.label(egui::RichText::new(subtitle).size(12.0).color(t.text_weak));
                    });
                });

                ui.add_space(16.0);

                // Кто: имя (если знаем) + ID на утопленной плашке.
                crate::theme::sunken().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if let Some(name) = &contact_name {
                        ui.label(egui::RichText::new(name).size(16.0).strong().color(t.text));
                        ui.add_space(2.0);
                    }
                    ui.label(egui::RichText::new(id_caption).size(11.0).color(t.text_muted));
                    ui.label(
                        egui::RichText::new(format_peer_id(&peer_id))
                            .size(22.0)
                            .strong()
                            .monospace()
                            .color(t.text),
                    );
                });

                ui.add_space(18.0);

                // Кнопки: Разрешить (акцент) + Отклонить (danger-контур).
                ui.horizontal(|ui| {
                    let btn_w = (ui.available_width() - 10.0) / 2.0;
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(allow_txt).size(14.0).strong().color(t.accent_fg),
                            )
                            .fill(t.accent)
                            .min_size(egui::vec2(btn_w, 42.0))
                            .corner_radius(egui::CornerRadius::same(crate::theme::radius::MD)),
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
                            egui::Button::new(
                                egui::RichText::new(reject_txt).size(14.0).color(t.danger),
                            )
                            .fill(crate::theme::tint(t.danger, 0.10))
                            .stroke(egui::Stroke::new(1.0, crate::theme::tint(t.danger, 0.40)))
                            .min_size(egui::vec2(btn_w, 42.0))
                            .corner_radius(egui::CornerRadius::same(crate::theme::radius::MD)),
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
                        .color(crate::theme::palette().text),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(self.text(
                        "Покажите этот ID и пароль для входящего подключения",
                        "Share this ID and password for direct unattended access",
                    ))
                    .size(15.0)
                    .color(crate::theme::palette().text_weak),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (r, g, b) = self.host_state.color();
                status_pill(ui, self.host_state_text(), egui::Color32::from_rgb(r, g, b));
            });
        });

        // macOS: surface a friendly setup card when Screen Recording or
        // Accessibility permission is missing — without them the host shows a
        // blank screen or ignores remote input. Disappears once both are on.
        #[cfg(target_os = "macos")]
        {
            let (screen_ok, ax_ok) = crate::host::macos_permission_status();
            if !(screen_ok && ax_ok) {
                ui.add_space(24.0);
                self.macos_permission_card(ui, screen_ok, ax_ok);
            }
        }

        ui.add_space(24.0);

        card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                egui::RichText::new(self.text("Данные доступа", "Access credentials"))
                    .size(18.0)
                    .color(crate::theme::palette().text),
            );
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(self.text("Ваш ID", "Your ID"))
                            .size(12.0)
                            .color(crate::theme::palette().text_weak),
                    );
                    ui.label(
                        egui::RichText::new(format_peer_id(&self.config.local_id))
                            .size(28.0)
                            .color(crate::theme::palette().text),
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
                            .color(crate::theme::palette().text_weak),
                    );
                    let pw_text = if self.show_host_password {
                        self.config.local_password.clone()
                    } else {
                        "•".repeat(self.config.local_password.len())
                    };
                    ui.label(
                        egui::RichText::new(pw_text)
                            .size(24.0)
                            .color(crate::theme::palette().text),
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
                    .color(crate::theme::palette().text),
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
                        .color(crate::theme::palette().text),
                )
                .min_size(egui::vec2(180.0, 54.0))
                .fill(crate::theme::palette().surface)
                .stroke(egui::Stroke::new(
                    1.0,
                    crate::theme::palette().border,
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
                    .color(crate::theme::palette().text_weak),
            );
            if let Some(video_status) = &self.host_video_status {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("Video: {video_status}"))
                        .monospace()
                        .size(10.5)
                        .color(crate::theme::palette().text_weak),
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

    /// macOS-only: friendly setup card guiding the user to grant the two
    /// permissions host mode needs. Each missing permission gets a one-click
    /// button that opens the exact System Settings pane.
    #[cfg(target_os = "macos")]
    fn macos_permission_card(&mut self, ui: &mut egui::Ui, screen_ok: bool, ax_ok: bool) {
        use egui_phosphor::regular as ph;
        let p = crate::theme::palette();

        let frame = egui::Frame::NONE
            .fill(p.surface)
            .stroke(egui::Stroke::new(1.0, p.warning))
            .corner_radius(egui::CornerRadius::same(14))
            .inner_margin(egui::Margin::same(20));

        frame.show(ui, |ui| {
            ui.set_min_width(ui.available_width());

            // ── Header ───────────────────────────────────────────────────────
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(ph::WARNING).size(22.0).color(p.warning));
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(self.text(
                            "Осталось выдать два разрешения",
                            "Two permissions still needed",
                        ))
                        .size(17.0)
                        .strong()
                        .color(p.text),
                    );
                    ui.label(
                        egui::RichText::new(self.text(
                            "Чтобы другие могли видеть экран этого Mac и управлять им",
                            "So others can see and control this Mac's screen",
                        ))
                        .size(13.0)
                        .color(p.text_weak),
                    );
                });
            });

            ui.add_space(16.0);

            // ── Row 1: Screen Recording ──────────────────────────────────────
            self.macos_permission_row(
                ui,
                ph::VIDEO_CAMERA,
                self.text("Запись экрана", "Screen Recording"),
                self.text(
                    "Передаёт изображение рабочего стола",
                    "Streams the desktop image to the client",
                ),
                screen_ok,
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
            );

            ui.add_space(10.0);

            // ── Row 2: Accessibility ─────────────────────────────────────────
            self.macos_permission_row(
                ui,
                ph::CURSOR_CLICK,
                self.text("Универсальный доступ", "Accessibility"),
                self.text(
                    "Позволяет управлять мышью и клавиатурой",
                    "Lets the remote control mouse & keyboard",
                ),
                ax_ok,
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            );

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(10.0);

            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(ph::ARROWS_CLOCKWISE)
                        .size(13.0)
                        .color(p.text_weak),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(self.text(
                        "Поставьте галочку напротив «EvertyDesk Lite» и перезапустите приложение.",
                        "Tick the box next to \"EvertyDesk Lite\", then restart the app.",
                    ))
                    .size(12.5)
                    .color(p.text_weak),
                );
            });
        });
    }

    /// One row of the macOS permission card: icon + title/description, and on
    /// the right either a green "Готово" badge or an "Открыть настройки" button.
    #[cfg(target_os = "macos")]
    fn macos_permission_row(
        &mut self,
        ui: &mut egui::Ui,
        icon: &str,
        title: &'static str,
        desc: &'static str,
        granted: bool,
        settings_url: &str,
    ) {
        use egui_phosphor::regular as ph;
        let p = crate::theme::palette();

        let row = egui::Frame::NONE
            .fill(p.surface_sunken)
            .stroke(egui::Stroke::new(1.0, p.border))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 12));

        row.show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(icon)
                        .size(22.0)
                        .color(if granted { p.success } else { p.accent }),
                );
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(title)
                            .size(15.0)
                            .strong()
                            .color(p.text),
                    );
                    ui.label(egui::RichText::new(desc).size(12.5).color(p.text_weak));
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if granted {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}  {}",
                                ph::CHECK_CIRCLE,
                                self.text("Готово", "Granted")
                            ))
                            .size(13.5)
                            .strong()
                            .color(p.success),
                        );
                    } else {
                        let btn = egui::Button::new(
                            egui::RichText::new(format!(
                                "{}  {}",
                                ph::ARROW_SQUARE_OUT,
                                self.text("Открыть настройки", "Open Settings")
                            ))
                            .size(13.5)
                            .strong()
                            .color(p.accent_fg),
                        )
                        .fill(p.accent)
                        .corner_radius(egui::CornerRadius::same(8))
                        .min_size(egui::vec2(0.0, 36.0));
                        if ui
                            .add(btn)
                            .on_hover_text(self.text(
                                "Откроет System Settings на нужном разделе",
                                "Opens System Settings at the right pane",
                            ))
                            .clicked()
                        {
                            let _ = std::process::Command::new("open").arg(settings_url).spawn();
                        }
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

        // Start LAN discovery so viewers on the same network can find us.
        if self.lan_discovery_stop.is_none() {
            let stop = Arc::new(AtomicBool::new(false));
            let config_arc = Arc::new(Mutex::new(self.config.clone()));
            lan_discovery::start(config_arc, stop.clone());
            self.lan_discovery_stop = Some(stop);
        }
    }

    fn stop_host_service(&mut self) {
        if let Some(svc) = self.host_service.take() {
            svc.stop();
            self.host_log
                .push(format!("[{}] Хост-сервис остановлен.", timestamp_hms()));
        }
        // Stop LAN discovery thread.
        if let Some(stop) = self.lan_discovery_stop.take() {
            stop.store(true, Ordering::Relaxed);
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
        use egui_phosphor::regular as ph;
        let t = crate::theme::palette();
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.horizontal(|ui| {
            // ── Группа: навигация / сеанс ─────────────────────────────────────
            if !detached_window
                && remote_icon_button(ui, ph::ARROW_LEFT, "Свернуть экран (сеанс активен)").clicked()
            {
                self.close_remote_viewer_panel();
                return;
            }
            if remote_icon_button_danger(ui, ph::SIGN_OUT, "Завершить удалённый сеанс").clicked() {
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

            toolbar_sep(ui);
            self.remote_display_selector_ui(
                ui,
                if detached_window { "remote-display" } else { "software-remote-display" },
            );

            // ── Группа: ввод текста / буфер ───────────────────────────────────
            toolbar_sep(ui);
            ui.add(
                egui::TextEdit::singleline(&mut self.text_to_send)
                    .hint_text("Текст в удалённый ввод")
                    .desired_width(if detached_window { 130.0 } else { 150.0 }),
            );
            let send_text = remote_icon_button_enabled(
                ui,
                !self.text_to_send.is_empty(),
                ph::PAPER_PLANE_RIGHT,
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
            if remote_icon_button(ui, ph::CLIPBOARD, "Вставить локальный буфер в удалённый").clicked() {
                self.paste_local_clipboard_to_remote();
            }

            // ── Группа: вид / захват ──────────────────────────────────────────
            toolbar_sep(ui);
            if remote_icon_button(ui, ph::ARROWS_CLOCKWISE, "Обновить live-video и контрольный кадр").clicked() {
                self.refresh_remote_screen();
            }
            if remote_icon_button(ui, ph::CAMERA, "Сохранить текущий кадр (PNG)").clicked() {
                self.save_current_frame_png();
            }
            if remote_icon_toggle(ui, ph::ARROWS_IN, self.fit_to_window, "Вписать экран в окно").clicked() {
                self.fit_to_window = !self.fit_to_window;
                self.save_ui_config();
            }
            if remote_icon_button(ui, ph::CORNERS_OUT, "Полный экран (F11)").clicked() {
                self.set_remote_fullscreen(ctx, !self.remote_fullscreen);
            }
            // Метрики/диагностика — открывает перемещаемое плавающее окно
            // поверх экрана (заменяет прежнюю нижнюю панель).
            if remote_icon_toggle(ui, ph::GAUGE, self.show_stream_info, "Метрики потока (плавающее окно)").clicked() {
                self.show_stream_info = !self.show_stream_info;
            }
            // View-only mode: disables all mouse+keyboard input forwarding.
            if remote_icon_toggle(
                ui,
                ph::EYE,
                self.view_only,
                self.text("Только просмотр (ввод заблокирован)", "View-only (input disabled)"),
            )
            .clicked()
            {
                self.view_only = !self.view_only;
                if self.view_only {
                    self.remote_input_focused = false;
                    self.release_remote_modifiers();
                }
            }

            // ── Правая группа: профиль AV, VM, меню (заполняет пространство) ──
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(egui::RichText::new(ph::DOTS_THREE).size(16.0), |ui| {
                    self.remote_more_menu_ui(ui)
                });
                if remote_icon_toggle(
                    ui,
                    ph::DESKTOP,
                    self.remote_vm_panel_open,
                    "Виртуальные машины на хосте (agentless)",
                )
                .clicked()
                {
                    self.remote_vm_panel_open = !self.remote_vm_panel_open;
                    if self.remote_vm_panel_open {
                        self.send_command(SessionCommand::ListVms);
                    }
                }
                // Профиль кодека/FPS/задержки — компактный бейдж-меню.
                let quality_badge = {
                    let codec = self.config.display.codec.label();
                    let fps = self.video_fps;
                    let lat = self.latency_ms.map(|ms| format!(" · {ms}ms")).unwrap_or_default();
                    let lat_color = self.latency_ms.map(|ms| {
                        if ms < 80 { egui::Color32::from_rgb(0x4C, 0xBF, 0x7A) }
                        else if ms < 200 { egui::Color32::from_rgb(0xFF, 0xB7, 0x47) }
                        else { egui::Color32::from_rgb(0xF0, 0x6A, 0x6A) }
                    }).unwrap_or(t.text);
                    egui::RichText::new(format!("{}  {} · {}fps{}", ph::SLIDERS_HORIZONTAL, codec, fps, lat))
                        .size(12.5)
                        .color(lat_color)
                };
                ui.menu_button(quality_badge, |ui| self.remote_video_profile_menu_ui(ui))
                    .response
                    .on_hover_text(self.text("Кодек · FPS · Задержка", "Codec · FPS · Latency"));
            });
        });
    }

    /// Панель agentless-VM: список VM удалённого хоста (Hyper-V + VirtualBox) и
    /// подключение к ним без агента в гостевой ОС.
    fn remote_vm_window(&mut self, ctx: &egui::Context) {
        let mut open = self.remote_vm_panel_open;
        egui::Window::new("🖧  Виртуальные машины хоста")
            .open(&mut open)
            .resizable(true)
            .default_width(480.0)
            .default_height(560.0)
            .min_width(380.0)
            .show(ctx, |ui| {
                // ── Шапка: счётчик + действия ──────────────────────────────────
                let total = self.remote_vms.len();
                let running = self.remote_vms.iter().filter(|v| v.connectable).count();
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{total} VM · {running} запущено"))
                            .size(13.0)
                            .strong()
                            .color(crate::theme::palette().text),
                    );
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui.button("↻").on_hover_text("Обновить список").clicked() {
                                self.send_command(SessionCommand::ListVms);
                            }
                            if !self.remote_attached_vm.is_empty()
                                && ui
                                    .button("⏏ Экран хоста")
                                    .on_hover_text("Отключиться от VM, вернуть экран хоста")
                                    .clicked()
                            {
                                self.remote_attached_vm.clear();
                                self.send_command(SessionCommand::AttachVm(String::new()));
                            }
                        },
                    );
                });

                if !self.remote_vm_status.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(&self.remote_vm_status)
                            .size(11.5)
                            .color(crate::theme::palette().success),
                    );
                }
                ui.separator();

                if self.remote_vms.is_empty() {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(
                            "Список пуст. Нажмите ↻ Обновить.\n\nДоступно, если удалённый \
                             хост — Windows с Hyper-V, либо на хосте установлен VirtualBox.",
                        )
                        .color(crate::theme::palette().text_muted),
                    );
                    return;
                }

                let vms = self.remote_vms.clone();
                let attached = self.remote_attached_vm.clone();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 7.0;
                    // Группируем по провайдеру: Hyper-V, затем VirtualBox.
                    for (prefix, title, color) in [
                        ("hyperv:", "HYPER-V", crate::theme::palette().info),
                        ("vbox:", "VIRTUALBOX", crate::theme::palette().warning),
                    ] {
                        let group: Vec<&RemoteVmEntry> =
                            vms.iter().filter(|v| v.id.starts_with(prefix)).collect();
                        if group.is_empty() {
                            continue;
                        }
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(format!("{title}  ({})", group.len()))
                                .size(11.0)
                                .strong()
                                .color(color),
                        );
                        ui.add_space(2.0);
                        for vm in group {
                            self.remote_vm_row(ui, vm, vm.id == attached);
                        }
                    }
                });
            });
        self.remote_vm_panel_open = open;
    }

    /// Одна строка-карточка VM в панели.
    fn remote_vm_row(&mut self, ui: &mut egui::Ui, vm: &RemoteVmEntry, is_attached: bool) {
        let (prov_label, prov_color) = vm_provider_badge(&vm.id);
        let dot = if vm.connectable {
            crate::theme::palette().success
        } else {
            crate::theme::palette().text_muted
        };
        let vm_id = vm.id.clone();
        let vm_name = vm.name.clone();
        let is_ctrl_open = self.remote_ctrl_vm_id == vm_id
            && (self.remote_power_panel_open
                || self.remote_checkpoint_panel_open
                || self.remote_rescue_panel_open);
        let cap_mode_label = vm
            .capability_graph
            .as_ref()
            .map(|g| g.recommended_mode.label().to_owned());
        let cap_mode_rgb = vm
            .capability_graph
            .as_ref()
            .map(|g| g.recommended_mode.badge_rgb());
        let t = crate::theme::palette();
        // Активная VM подсвечивается акцентной подложкой + рамкой; остальные —
        // на приподнятой поверхности. Адаптивно к тёмной/светлой теме.
        let (card_fill, card_stroke) = if is_attached {
            (crate::theme::accent_tint(&t, 0.14), egui::Stroke::new(1.2, t.accent))
        } else {
            (t.surface_raised, egui::Stroke::new(1.0, t.border))
        };
        egui::Frame::NONE
            .fill(card_fill)
            .stroke(card_stroke)
            .corner_radius(egui::CornerRadius::same(crate::theme::radius::LG))
            .inner_margin(egui::Margin::same(11))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // ── Top row: dot + name/state + action buttons ────────────────
                ui.horizontal(|ui| {
                    paint_status_dot(ui, dot);
                    ui.add_space(2.0);
                    ui.vertical(|ui| {
                        let name = vm_name.split(" · ").next().unwrap_or(&vm_name);
                        ui.label(
                            egui::RichText::new(name)
                                .strong()
                                .size(14.0)
                                .color(t.text),
                        );
                        ui.horizontal(|ui| {
                            // Чип провайдера
                            egui::Frame::NONE
                                .fill(prov_color.gamma_multiply(0.25))
                                .corner_radius(egui::CornerRadius::same(4))
                                .inner_margin(egui::Margin::symmetric(6, 1))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new(prov_label)
                                            .size(10.0)
                                            .strong()
                                            .color(prov_color),
                                    );
                                });
                            ui.label(
                                egui::RichText::new(&vm.state)
                                    .size(11.0)
                                    .color(crate::theme::palette().text_muted),
                            );
                            // ── Capability mode badge ─────────────────────────
                            if let (Some(label), Some((r, g, b))) = (&cap_mode_label, cap_mode_rgb) {
                                let badge_color = egui::Color32::from_rgb(r, g, b);
                                egui::Frame::NONE
                                    .fill(badge_color.gamma_multiply(0.22))
                                    .corner_radius(egui::CornerRadius::same(4))
                                    .inner_margin(egui::Margin::symmetric(5, 1))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(label.as_str())
                                                .size(9.5)
                                                .strong()
                                                .color(badge_color),
                                        );
                                    });
                            }
                        });
                    });
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if is_attached {
                                // Бейдж «подключено» с акцентной подложкой.
                                egui::Frame::NONE
                                    .fill(crate::theme::accent_tint(&t, 0.18))
                                    .corner_radius(egui::CornerRadius::same(crate::theme::radius::SM))
                                    .inner_margin(egui::Margin::symmetric(8, 3))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new("● подключено")
                                                .size(11.0)
                                                .strong()
                                                .color(t.accent),
                                        );
                                    });
                            } else if ui
                                .add_enabled(
                                    vm.connectable,
                                    egui::Button::new(
                                        egui::RichText::new("Подключиться").color(t.accent_fg),
                                    )
                                    .fill(t.accent)
                                    .corner_radius(egui::CornerRadius::same(crate::theme::radius::MD)),
                                )
                                .clicked()
                            {
                                self.remote_attached_vm = vm_id.clone();
                                self.remote_vm_status = format!("Подключение к «{vm_name}»…");
                                self.send_command(SessionCommand::AttachVm(vm_id.clone()));
                            }
                            // ── Capability / Analyse button ───────────────────
                            if ui
                                .add(egui::Button::new("🔍").small())
                                .on_hover_text("Запросить capability graph")
                                .clicked()
                            {
                                self.send_command(SessionCommand::VmCapabilityRequest(vm_id.clone()));
                            }
                        },
                    );
                });

                // ── Power + Control buttons row ───────────────────────────────
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    use egui_phosphor::regular as ph;
                    // Запуск — безопасное действие, выполняется сразу (зелёный значок).
                    if power_icon_button(ui, ph::PLAY, t.success, "Запустить VM").clicked() {
                        let payload = serde_json::json!({ "vm_id": vm_id, "action": "start" }).to_string();
                        self.send_command(SessionCommand::VmPowerOp(payload));
                    }
                    // Опасные действия — через подтверждение (ставим pending).
                    if power_icon_button(ui, ph::STOP, t.danger, "Выключить VM (hard stop)").clicked() {
                        self.pending_vm_action = Some((
                            vm_id.clone(), "stop".into(), vm_name.clone(),
                            "жёстко выключить (hard stop) — несохранённые данные в гостевой ОС будут потеряны".into(),
                        ));
                    }
                    if power_icon_button(ui, ph::POWER, t.warning, "Завершить работу (guest shutdown)").clicked() {
                        self.pending_vm_action = Some((
                            vm_id.clone(), "shutdown".into(), vm_name.clone(),
                            "корректно завершить работу гостевой ОС".into(),
                        ));
                    }
                    if power_icon_button(ui, ph::ARROWS_CLOCKWISE, t.info, "Перезапустить VM").clicked() {
                        self.pending_vm_action = Some((
                            vm_id.clone(), "restart".into(), vm_name.clone(),
                            "перезапустить виртуальную машину".into(),
                        ));
                    }
                    if power_icon_button(ui, ph::PAUSE, t.text_weak, "Пауза VM").clicked() {
                        self.pending_vm_action = Some((
                            vm_id.clone(), "pause".into(), vm_name.clone(),
                            "приостановить (pause) выполнение VM".into(),
                        ));
                    }
                    ui.separator();
                    // Checkpoint toggle
                    let chk_active = self.remote_ctrl_vm_id == vm_id
                        && self.remote_checkpoint_panel_open;
                    if ui
                        .selectable_label(chk_active, "📷 Checkpoints")
                        .clicked()
                    {
                        if self.remote_ctrl_vm_id == vm_id && self.remote_checkpoint_panel_open {
                            self.remote_checkpoint_panel_open = false;
                        } else {
                            self.remote_ctrl_vm_id = vm_id.clone();
                            self.remote_checkpoint_panel_open = true;
                            self.remote_rescue_panel_open = false;
                            // Request fresh list
                            let payload = serde_json::json!({
                                "vm_id": vm_id,
                                "op": "list"
                            })
                            .to_string();
                            self.send_command(SessionCommand::VmCheckpointOp(payload));
                        }
                    }
                    // Rescue toggle
                    let rescue_active = self.remote_ctrl_vm_id == vm_id
                        && self.remote_rescue_panel_open;
                    if ui.selectable_label(rescue_active, "🛟 Rescue").clicked() {
                        if self.remote_ctrl_vm_id == vm_id && self.remote_rescue_panel_open {
                            self.remote_rescue_panel_open = false;
                        } else {
                            self.remote_ctrl_vm_id = vm_id.clone();
                            self.remote_rescue_panel_open = true;
                            self.remote_checkpoint_panel_open = false;
                        }
                    }
                });

                // ── Checkpoint panel ──────────────────────────────────────────
                if self.remote_ctrl_vm_id == vm_id && self.remote_checkpoint_panel_open {
                    ui.add_space(6.0);
                    egui::Frame::NONE
                        .fill(crate::theme::palette().surface_sunken)
                        .stroke(egui::Stroke::new(1.0, crate::theme::palette().border))
                        .corner_radius(egui::CornerRadius::same(crate::theme::radius::MD))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Контрольные точки").strong().size(12.0));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.button("➕ Создать").clicked() {
                                        let payload = serde_json::json!({
                                            "vm_id": vm_id, "op": "create"
                                        })
                                        .to_string();
                                        self.send_command(SessionCommand::VmCheckpointOp(payload));
                                        // Refresh list after a moment
                                        let payload2 = serde_json::json!({
                                            "vm_id": vm_id, "op": "list"
                                        })
                                        .to_string();
                                        self.send_command(SessionCommand::VmCheckpointOp(payload2));
                                    }
                                });
                            });
                            ui.add_space(4.0);
                            // Show checkpoints from cached JSON
                            let checkpoints_json = vm.checkpoints_json.clone();
                            if let Some(ref json_str) = checkpoints_json {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
                                    if let Some(arr) = v.get("checkpoints").and_then(|x| x.as_array()) {
                                        if arr.is_empty() {
                                            ui.label(egui::RichText::new("Нет контрольных точек").color(crate::theme::palette().text_muted).size(11.0));
                                        }
                                        for cp in arr {
                                            let name = cp.get("name").and_then(|x| x.as_str()).unwrap_or("Без имени");
                                            let path = cp.get("path").and_then(|x| x.as_str()).unwrap_or("");
                                            let time = cp.get("created_time").and_then(|x| x.as_str()).unwrap_or("");
                                            let ctype = cp.get("type").and_then(|x| x.as_str()).unwrap_or("");
                                            ui.horizontal(|ui| {
                                                ui.label(egui::RichText::new(name).size(11.5).strong());
                                                ui.label(egui::RichText::new(format!("[{ctype}] {time}")).size(10.0).color(crate::theme::palette().text_muted));
                                                let path_str = path.to_owned();
                                                let vm_id2 = vm_id.clone();
                                                if ui.small_button("↩ Apply").clicked() {
                                                    let payload = serde_json::json!({
                                                        "vm_id": vm_id2, "op": "apply",
                                                        "path": path_str
                                                    })
                                                    .to_string();
                                                    self.send_command(SessionCommand::VmCheckpointOp(payload));
                                                }
                                                let path_str2 = path.to_owned();
                                                let vm_id3 = vm_id.clone();
                                                if ui.small_button("🗑 Delete").clicked() {
                                                    let payload = serde_json::json!({
                                                        "vm_id": vm_id3, "op": "delete",
                                                        "path": path_str2
                                                    })
                                                    .to_string();
                                                    self.send_command(SessionCommand::VmCheckpointOp(payload));
                                                }
                                            });
                                        }
                                    }
                                }
                            } else {
                                ui.label(egui::RichText::new("Загрузка…").color(crate::theme::palette().text_muted).size(11.0));
                            }
                        });
                }

                // ── Rescue input panel ────────────────────────────────────────
                if self.remote_ctrl_vm_id == vm_id && self.remote_rescue_panel_open {
                    let t = crate::theme::palette();
                    ui.add_space(6.0);
                    egui::Frame::NONE
                        .fill(crate::theme::tint(t.danger, 0.10))
                        .stroke(egui::Stroke::new(1.0, crate::theme::tint(t.danger, 0.35)))
                        .corner_radius(egui::CornerRadius::same(crate::theme::radius::MD))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(egui::RichText::new("🛟 BasicRescue — ввод").strong().size(12.0).color(t.text));
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                // Ctrl+Alt+Del
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("⌨ Ctrl+Alt+Del").color(t.accent_fg),
                                        )
                                        .fill(t.danger),
                                    )
                                    .on_hover_text("Отправить Ctrl+Alt+Del в VM")
                                    .clicked()
                                {
                                    let payload = serde_json::json!({
                                        "vm_id": vm_id,
                                        "input_type": "ctrl_alt_del",
                                        "text": ""
                                    })
                                    .to_string();
                                    self.send_command(SessionCommand::VmRescueInput(payload));
                                }
                            });
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label("Ввод текста:");
                            });
                            ui.horizontal(|ui| {
                                let text_edit = egui::TextEdit::singleline(&mut self.remote_rescue_text)
                                    .hint_text("Введите текст для отправки в VM…")
                                    .desired_width(ui.available_width() - 80.0);
                                let resp = ui.add(text_edit);
                                let send_text = resp.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                if send_text
                                    || ui
                                        .add_enabled(
                                            !self.remote_rescue_text.is_empty(),
                                            egui::Button::new("▶ Send"),
                                        )
                                        .clicked()
                                {
                                    let text = std::mem::take(&mut self.remote_rescue_text);
                                    if !text.is_empty() {
                                        let payload = serde_json::json!({
                                            "vm_id": vm_id,
                                            "input_type": "type_text",
                                            "text": text
                                        })
                                        .to_string();
                                        self.send_command(SessionCommand::VmRescueInput(payload));
                                    }
                                }
                            });
                        });
                }

                let _ = is_ctrl_open;
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
        // Show shortcut help on ? key when not capturing remote input.
        if !self.remote_input_focused
            && ctx.input(|i| i.key_pressed(egui::Key::Questionmark))
        {
            self.show_shortcuts_help = !self.show_shortcuts_help;
        }
        self.shortcuts_help_window(ctx);

        // In fullscreen, auto-hide the toolbar after 2s — reveal on mouse near top.
        let show_toolbar = !self.remote_fullscreen || {
            let near_top = ctx.input(|i| {
                i.pointer.latest_pos()
                    .map(|p| p.y < 56.0)
                    .unwrap_or(false)
            });
            if near_top { self.toolbar_last_active = Instant::now(); }
            self.toolbar_last_active.elapsed().as_secs_f32() < 2.0
        };
        if show_toolbar {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
            egui::Panel::top("software-remote-toolbar").show(ctx, |ui| {
                self.remote_session_toolbar_ui(ui, ctx, false);
            });
        }

        // ── Плавающее перемещаемое окно с метриками (вместо нижней панели) ────
        self.stream_info_window(ctx);

        // No frame margin — the remote screen + its letterbox backdrop should
        // reach the panel edges (toolbar above, status bar below) with no app
        // background showing through.
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(0x07, 0x0A, 0x0F)))
            .show(ctx, |ui| {
                self.remote_screen_ui(ui);
            });
    }

    /// Keyboard shortcuts reference popup (press ? when not capturing remote input).
    fn shortcuts_help_window(&mut self, ctx: &egui::Context) {
        if !self.show_shortcuts_help {
            return;
        }
        let mut open = self.show_shortcuts_help;
        let lang = self.ui_lang;
        egui::Window::new(tr(lang, "Горячие клавиши", "Keyboard shortcuts"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(340.0)
            .default_pos(egui::pos2(100.0, 80.0))
            .show(ctx, |ui| {
                let rows: &[(&str, &str, &str, &str)] = &[
                    ("?", "?", "Горячие клавиши", "Show shortcuts"),
                    ("Ctrl+Esc", "Ctrl+Esc", "Освободить захват ввода", "Release keyboard capture"),
                    ("F11", "F11", "Полный экран (вне захвата)", "Fullscreen (outside capture)"),
                    ("Ctrl+C/V/X/Z", "Ctrl+C/V/X/Z", "Буфер: копировать/вставить/вырезать/отмена", "Clipboard: copy/paste/cut/undo"),
                    ("Win+любая", "Win+any", "Отправить Win+клавишу удалённому ПК", "Send Win+key to remote"),
                    ("Esc", "Esc", "Отправить Esc удалённому ПК (вим/диалоги)", "Send Esc to remote (vi/dialogs)"),
                    ("Мышь внутри экрана → клик", "Click inside screen", "Захватить ввод клавиатуры", "Capture keyboard input"),
                    ("глаз ☞ тулбар", "Eye ☞ toolbar", "Режим «только просмотр»", "Toggle view-only mode"),
                    ("Камера ☞ тулбар", "Camera ☞ toolbar", "Сохранить кадр PNG", "Save frame as PNG"),
                ];
                egui::Grid::new("shortcuts-grid")
                    .num_columns(2)
                    .striped(true)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        for &(key_ru, key_en, desc_ru, desc_en) in rows {
                            let key = match lang { UiLang::Ru => key_ru, UiLang::En => key_en };
                            let desc = match lang { UiLang::Ru => desc_ru, UiLang::En => desc_en };
                            ui.label(egui::RichText::new(key).monospace().strong());
                            ui.label(desc);
                            ui.end_row();
                        }
                    });
            });
        self.show_shortcuts_help = open;
    }

    /// Красивое окно с детальной диагностикой стрима (по кнопке ℹ Детали).
    fn stream_info_window(&mut self, ctx: &egui::Context) {
        if !self.show_stream_info {
            return;
        }
        let mut open = self.show_stream_info;
        // No anchor → the window is freely draggable over the remote screen
        // (the user asked for a movable floating panel instead of a fixed
        // bottom bar). egui remembers its position by id between frames.
        egui::Window::new(self.text("Метрики потока", "Stream metrics"))
            .open(&mut open)
            .resizable(true)
            .collapsible(true)
            .default_width(300.0)
            .default_pos(egui::pos2(80.0, 90.0))
            .show(ctx, |ui| {
                let green = crate::theme::palette().accent;
                let white = egui::Color32::from_rgb(0x1A, 0x1F, 0x2A);
                let amber = crate::theme::palette().warning;

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
                                .color(crate::theme::palette().text_muted),
                        );
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let pc = match self.evrt_pressure.as_str() {
                            "critical" => crate::theme::palette().danger,
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
                        .color(crate::theme::palette().text_muted),
                    );
                } else {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(self.text(
                            "📡 TCP relay (EVRT не активен)",
                            "📡 TCP relay (EVRT inactive)",
                        ))
                        .size(11.5)
                        .color(crate::theme::palette().text_muted),
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
            .with_maximize_button(true)
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
            // Only toggle local fullscreen when keyboard is NOT captured by the remote.
            // When captured, F11 is forwarded to the remote via handle_remote_keyboard.
            if !self.remote_input_focused && ctx.input(|input| input.key_pressed(egui::Key::F11)) {
                self.remote_fullscreen = !self.remote_fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.remote_fullscreen));
            }
            if !self.remote_input_focused && ctx.input(|i| i.key_pressed(egui::Key::Questionmark)) {
                self.show_shortcuts_help = !self.show_shortcuts_help;
            }
            self.shortcuts_help_window(ctx);

            // In fullscreen, auto-hide the toolbar after 2s — reveal on mouse near top.
            let show_toolbar = !self.remote_fullscreen || {
                let near_top = ctx.input(|i| {
                    i.pointer.latest_pos()
                        .map(|p| p.y < 56.0)
                        .unwrap_or(false)
                });
                if near_top { self.toolbar_last_active = Instant::now(); }
                self.toolbar_last_active.elapsed().as_secs_f32() < 2.0
            };
            if show_toolbar {
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
                egui::Panel::top("remote-toolbar").show(ctx, |ui| {
                    self.remote_session_toolbar_ui(ui, ctx, true);
                });
            }

            // ── Плавающее перемещаемое окно с метриками (вместо нижней панели) ──
            self.stream_info_window(ctx);

            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(0x07, 0x0A, 0x0F)))
                .show(ctx, |ui| {
                    self.remote_screen_ui(ui);
                });
        });
    }

    fn remote_screen_ui(&mut self, ui: &mut egui::Ui) {
        let available_size = ui.available_size_before_wrap();
        let available_width = available_size.x.max(1.0);
        let Some(texture) = self.remote_texture.clone() else {
            let full_rect = ui.available_rect_before_wrap();
            ui.painter().rect_filled(full_rect, 0.0, egui::Color32::from_rgb(0x07, 0x0A, 0x0F));
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(full_rect), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space((full_rect.height() * 0.5 - 28.0).max(0.0));
                    ui.spinner();
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Ожидание первого кадра…")
                            .color(crate::theme::palette().text_muted),
                    );
                });
            });
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

        // Hide OS cursor when we draw our own overlay, unless VP9 is active
        // (VP9/Desktop Duplication bakes cursor into the frame already).
        let vp9_cursor_in_frame = self.last_frame_codec == "VP9";
        let hover_cursor = if self.cursor_texture.is_some() && !vp9_cursor_in_frame {
            egui::CursorIcon::None
        } else {
            egui::CursorIcon::Default
        };

        // Fill the entire viewport with a dark backdrop and center the remote
        // image inside it. Previously the image was laid out top-left and the
        // leftover area was just empty app background — at any aspect ratio
        // that doesn't match the window you'd get a bright blank strip that
        // read as "broken layout". A centered image over a flat near-black
        // backdrop reads as an intentional monitor letterbox, which is the
        // expected look for a remote-desktop viewer.
        let full_rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(
            full_rect,
            0.0,
            egui::Color32::from_rgb(0x07, 0x0A, 0x0F),
        );
        let img_min = full_rect.min
            + egui::vec2(
                ((full_rect.width() - size.x) * 0.5).max(0.0),
                ((full_rect.height() - size.y) * 0.5).max(0.0),
            );
        let img_rect = egui::Rect::from_min_size(img_min, size);
        let response = ui
            .put(
                img_rect,
                egui::Image::new(&texture).sense(egui::Sense::click_and_drag()),
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

            // Compact "input captured" pill in the top-left corner of the
            // screen — the bottom status bar that used to host this is gone.
            let painter = ui.painter();
            let pad = egui::vec2(8.0, 4.0);
            let text = tr(self.ui_lang, "⌨ ввод захвачен · Ctrl+Esc", "⌨ input captured · Ctrl+Esc");
            let font = egui::FontId::proportional(11.5);
            let galley = painter.layout_no_wrap(
                text.to_owned(),
                font,
                egui::Color32::from_rgb(0xCF, 0xE8, 0xFA),
            );
            let pill_rect = egui::Rect::from_min_size(
                response.rect.min + egui::vec2(8.0, 8.0),
                galley.size() + pad * 2.0,
            );
            painter.rect_filled(
                pill_rect,
                4.0,
                egui::Color32::from_rgba_unmultiplied(0x12, 0x3A, 0x55, 0xE0),
            );
            painter.galley(pill_rect.min + pad, galley, egui::Color32::WHITE);
        }

        // Draw remote cursor overlay on top of the video.
        // Skip for VP9: Desktop Duplication API bakes the hardware cursor into
        // the captured frame, so the overlay causes a double cursor.
        let vp9_active = self.last_frame_codec == "VP9";
        if !vp9_active {
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
        } // end cursor overlay (VP9 excluded)

        if self.connected && !self.view_only {
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
                            if *pressed {
                                // Press outside the remote screen (e.g. a click
                                // on the status-bar "Детали" button) must not
                                // touch the guest at all.
                                if !inside {
                                    continue;
                                }
                                self.remote_pointer_armed = true;
                            } else {
                                // Release: only forward if the matching press
                                // armed inside the screen (covers drag-off-screen);
                                // otherwise it's the tail of an out-of-screen UI click.
                                if !self.remote_pointer_armed {
                                    continue;
                                }
                                self.remote_pointer_armed = false;
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
                            // Only scroll the guest when the pointer is actually
                            // over the remote screen (not over the status bar).
                            let over_screen = ui
                                .ctx()
                                .input(|i| i.pointer.hover_pos())
                                .map(|p| response.rect.contains(p))
                                .unwrap_or(false);
                            if over_screen {
                                if let Some((x, y)) = self.wheel_delta(*unit, *delta) {
                                    self.send_command(SessionCommand::MouseWheel { x, y });
                                    self.request_visual_refresh_after_input();
                                }
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
        if self.view_only {
            return;
        }
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
                    // Ctrl+Escape: release keyboard capture without forwarding to remote.
                    if key == egui::Key::Escape && modifiers.ctrl {
                        self.remote_input_focused = false;
                        self.release_remote_modifiers();
                        continue;
                    }
                    // Bare Escape: forward to remote (vi, game menus, dialogs all need it).
                    // Use Ctrl+Escape to release keyboard capture.
                    // Allow key-repeat only for navigation/edit keys (arrows, backspace, delete,
                    // page-up/down, home, end). Other keys skip repeat to avoid duplicates.
                    if repeat && !key_allows_repeat(key) {
                        continue;
                    }
                    if has_command_modifier(modifiers) && egui_key_to_text(key).is_some() {
                        // Send letter keys with active modifiers explicitly so
                        // Ctrl+C/V/X/Z/A actually reach the remote as shortcuts,
                        // not as bare text characters.
                        let text = egui_key_to_text(key).unwrap();
                        let mut mods = Vec::new();
                        if modifiers.ctrl { mods.push(ControlKey::Control); }
                        // egui uses `command` for the OS command key (Win on Windows,
                        // Cmd on macOS). Forward it as Meta so Win+key reaches remote.
                        if modifiers.command { mods.push(ControlKey::Meta); }
                        if modifiers.alt { mods.push(ControlKey::Alt); }
                        if modifiers.shift { mods.push(ControlKey::Shift); }
                        self.send_command(SessionCommand::KeyTextWithModifiers { text, modifiers: mods });
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

/// Размер иконочной кнопки тулбара сессии (enterprise — компактно и ровно).
const TOOLBAR_BTN: egui::Vec2 = egui::Vec2::new(32.0, 30.0);

/// Вертикальный разделитель групп тулбара (тонкая линия в цвете границы).
fn toolbar_sep(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let h = 20.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, h), egui::Sense::hover());
    ui.painter().vline(
        rect.center().x,
        (rect.center().y - h / 2.0)..=(rect.center().y + h / 2.0),
        egui::Stroke::new(1.0, crate::theme::palette().border),
    );
    ui.add_space(4.0);
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
    let t = crate::theme::palette();
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(icon).size(16.0).color(t.text))
            .min_size(TOOLBAR_BTN)
            .corner_radius(egui::CornerRadius::same(crate::theme::radius::SM)),
    )
    .on_hover_text(tooltip)
}

/// Опасная иконочная кнопка тулбара (завершить сеанс) — danger-акцент.
fn remote_icon_button_danger(ui: &mut egui::Ui, icon: &str, tooltip: &str) -> egui::Response {
    let t = crate::theme::palette();
    ui.add(
        egui::Button::new(egui::RichText::new(icon).size(16.0).color(t.danger))
            .min_size(TOOLBAR_BTN)
            .fill(crate::theme::tint(t.danger, 0.10))
            .corner_radius(egui::CornerRadius::same(crate::theme::radius::SM)),
    )
    .on_hover_text(tooltip)
}

fn remote_icon_toggle(
    ui: &mut egui::Ui,
    icon: &str,
    selected: bool,
    tooltip: &str,
) -> egui::Response {
    let t = crate::theme::palette();
    let txt = if selected { t.accent } else { t.text };
    ui.add(
        egui::Button::new(egui::RichText::new(icon).size(16.0).color(txt))
            .selected(selected)
            .min_size(TOOLBAR_BTN)
            .corner_radius(egui::CornerRadius::same(crate::theme::radius::SM)),
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
        (crate::theme::palette().accent, true)
    } else if fps >= 8.0 {
        (crate::theme::palette().warning, false)
    } else {
        (crate::theme::palette().danger, false)
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
/// Map egui Key to Windows Virtual Key code for Hyper-V Msvm_Keyboard.
/// Only non-printable / control keys — printable chars go through TypeText.
#[cfg(windows)]
/// #3 Read plain text from the system clipboard via arboard (instant, cross-platform).
fn clipboard_read_text() -> String {
    arboard::Clipboard::new()
        .ok()
        .and_then(|mut c| c.get_text().ok())
        .unwrap_or_default()
}

fn egui_key_to_vkcode(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    Some(match key {
        Escape    => 0x1B,
        Enter     => 0x0D,
        Backspace => 0x08,
        Tab       => 0x09,
        Delete    => 0x2E,
        Insert    => 0x2D,
        Home      => 0x24,
        End       => 0x23,
        PageUp    => 0x21,
        PageDown  => 0x22,
        ArrowLeft  => 0x25,
        ArrowUp    => 0x26,
        ArrowRight => 0x27,
        ArrowDown  => 0x28,
        F1  => 0x70, F2  => 0x71, F3  => 0x72, F4  => 0x73,
        F5  => 0x74, F6  => 0x75, F7  => 0x76, F8  => 0x77,
        F9  => 0x78, F10 => 0x79, F11 => 0x7A, F12 => 0x7B,
        // Printable keys handled by TypeText — don't double-send
        _ => return None,
    })
}

/// Map letter/digit egui Keys → Windows VK code (for Ctrl+letter combos).
/// Only used when modifiers are active (printable keys otherwise go via TypeText).
fn egui_letter_to_vkcode(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    Some(match key {
        A => 0x41, B => 0x42, C => 0x43, D => 0x44, E => 0x45,
        F => 0x46, G => 0x47, H => 0x48, I => 0x49, J => 0x4A,
        K => 0x4B, L => 0x4C, M => 0x4D, N => 0x4E, O => 0x4F,
        P => 0x50, Q => 0x51, R => 0x52, S => 0x53, T => 0x54,
        U => 0x55, V => 0x56, W => 0x57, X => 0x58, Y => 0x59,
        Z => 0x5A,
        Num0 => 0x30, Num1 => 0x31, Num2 => 0x32, Num3 => 0x33,
        Num4 => 0x34, Num5 => 0x35, Num6 => 0x36, Num7 => 0x37,
        Num8 => 0x38, Num9 => 0x39,
        Space => 0x20,
        _ => return None,
    })
}

/// Map egui Key → key_id for virtualbox::special_key_to_scancodes.
/// Only non-printable keys — printable go through VboxCmd::PutString.
#[cfg(windows)]
fn egui_key_to_vbox_key_id(key: egui::Key) -> Option<u8> {
    use egui::Key::*;
    Some(match key {
        Escape    => 0x01,
        Backspace => 0x0E,
        Tab       => 0x0F,
        Enter     => 0x1C,
        Insert    => 0x52,
        Delete    => 0x53,
        Home      => 0x47,
        End       => 0x4F,
        PageUp    => 0x49,
        PageDown  => 0x51,
        ArrowLeft  => 0x4B,
        ArrowRight => 0x4D,
        ArrowUp    => 0x48,
        ArrowDown  => 0x50,
        F1  => 0x3B, F2  => 0x3C, F3  => 0x3D, F4  => 0x3E,
        F5  => 0x3F, F6  => 0x40, F7  => 0x41, F8  => 0x42,
        F9  => 0x43, F10 => 0x44, F11 => 0x57, F12 => 0x58,
        _ => return None,
    })
}

fn fitted_image_rect(outer: egui::Rect, image_w: f32, image_h: f32) -> egui::Rect {
    if image_w <= 0.0 || image_h <= 0.0 || outer.width() <= 0.0 || outer.height() <= 0.0 {
        return outer;
    }

    let image_aspect = image_w / image_h;
    let outer_aspect = outer.width() / outer.height();
    if outer_aspect > image_aspect {
        let w = outer.height() * image_aspect;
        egui::Rect::from_center_size(outer.center(), egui::vec2(w, outer.height()))
    } else {
        let h = outer.width() / image_aspect;
        egui::Rect::from_center_size(outer.center(), egui::vec2(outer.width(), h))
    }
}

fn pointer_to_guest_pixel(
    pos: egui::Pos2,
    image_rect: egui::Rect,
    guest_w: f32,
    guest_h: f32,
) -> Option<(u16, u16)> {
    if !image_rect.contains(pos)
        || image_rect.width() <= 0.0
        || image_rect.height() <= 0.0
        || guest_w <= 0.0
        || guest_h <= 0.0
    {
        return None;
    }

    let gx = ((pos.x - image_rect.left()) / image_rect.width() * guest_w)
        .clamp(0.0, guest_w - 1.0) as u16;
    let gy = ((pos.y - image_rect.top()) / image_rect.height() * guest_h)
        .clamp(0.0, guest_h - 1.0) as u16;
    Some((gx, gy))
}

fn egui_pointer_to_vrde_button(button: egui::PointerButton) -> Option<u8> {
    Some(match button {
        egui::PointerButton::Primary => 0,
        egui::PointerButton::Secondary => 1,
        egui::PointerButton::Middle => 2,
        _ => return None,
    })
}

fn egui_key_is_plain_text(key: egui::Key) -> bool {
    use egui::Key::*;
    matches!(
        key,
        A | B | C | D | E | F | G | H | I | J | K | L | M | N | O | P | Q | R | S | T | U
            | V | W | X | Y | Z
            | Num0 | Num1 | Num2 | Num3 | Num4 | Num5 | Num6 | Num7 | Num8 | Num9
            | Space
    )
}

fn egui_key_to_rdp_scancode(key: egui::Key) -> Option<(u8, bool)> {
    use egui::Key::*;
    Some(match key {
        Escape => (0x01, false),
        Backspace => (0x0E, false),
        Tab => (0x0F, false),
        Enter => (0x1C, false),
        Insert => (0x52, true),
        Delete => (0x53, true),
        Home => (0x47, true),
        End => (0x4F, true),
        PageUp => (0x49, true),
        PageDown => (0x51, true),
        ArrowLeft => (0x4B, true),
        ArrowRight => (0x4D, true),
        ArrowUp => (0x48, true),
        ArrowDown => (0x50, true),
        F1 => (0x3B, false),
        F2 => (0x3C, false),
        F3 => (0x3D, false),
        F4 => (0x3E, false),
        F5 => (0x3F, false),
        F6 => (0x40, false),
        F7 => (0x41, false),
        F8 => (0x42, false),
        F9 => (0x43, false),
        F10 => (0x44, false),
        F11 => (0x57, false),
        F12 => (0x58, false),
        A => (0x1E, false),
        B => (0x30, false),
        C => (0x2E, false),
        D => (0x20, false),
        E => (0x12, false),
        F => (0x21, false),
        G => (0x22, false),
        H => (0x23, false),
        I => (0x17, false),
        J => (0x24, false),
        K => (0x25, false),
        L => (0x26, false),
        M => (0x32, false),
        N => (0x31, false),
        O => (0x18, false),
        P => (0x19, false),
        Q => (0x10, false),
        R => (0x13, false),
        S => (0x1F, false),
        T => (0x14, false),
        U => (0x16, false),
        V => (0x2F, false),
        W => (0x11, false),
        X => (0x2D, false),
        Y => (0x15, false),
        Z => (0x2C, false),
        Num0 => (0x0B, false),
        Num1 => (0x02, false),
        Num2 => (0x03, false),
        Num3 => (0x04, false),
        Num4 => (0x05, false),
        Num5 => (0x06, false),
        Num6 => (0x07, false),
        Num7 => (0x08, false),
        Num8 => (0x09, false),
        Num9 => (0x0A, false),
        Space => (0x39, false),
        _ => return None,
    })
}

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

fn configure_icon_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}

#[cfg(windows)]
fn vm_icon_button(ui: &mut egui::Ui, icon: &str, tooltip: impl Into<egui::WidgetText>) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(icon).size(15.0))
            .min_size(egui::vec2(26.0, 24.0)),
    )
    .on_hover_text(tooltip)
}

#[cfg(windows)]
fn vm_icon_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: &str,
    tooltip: impl Into<egui::WidgetText>,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(icon).size(15.0))
            .min_size(egui::vec2(26.0, 24.0)),
    )
    .on_hover_text(tooltip)
}

#[cfg(windows)]
fn default_vrde_desktop_size() -> (u16, u16) {
    (1920, 1080)
}

#[cfg(windows)]
fn vrde_desktop_size_for_canvas(size: egui::Vec2) -> (u16, u16) {
    let scale = 1.25;
    let mut width = (size.x * scale).round().clamp(1920.0, 3840.0) as u16;
    if width % 2 != 0 {
        width = width.saturating_sub(1);
    }
    let height = (size.y * scale).round().clamp(1080.0, 2160.0) as u16;
    (width.max(200), height.max(200))
}

// Тема и стиль теперь живут в `theme.rs` (дизайн-система с токенами).
// `theme::apply(ctx, mode)` настраивает egui Visuals+Style и обновляет
// глобальную палитру, доступную через `theme::palette()`.

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

/// Рисует цветной круг-индикатор статуса (альтернатива "●" которого нет в NotoSans).
fn paint_status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// Квадратная кнопка-значок управления питанием VM: Phosphor-глиф в цвете
/// действия, подложка-tint этого же цвета. Единый стиль для play/stop/power/…
fn power_icon_button(
    ui: &mut egui::Ui,
    glyph: &str,
    color: egui::Color32,
    tooltip: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 28.0), egui::Sense::click());
    let bg = if response.hovered() {
        crate::theme::tint(color, 0.22)
    } else {
        crate::theme::tint(color, 0.12)
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(crate::theme::radius::SM), bg);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(15.0),
        color,
    );
    response.on_hover_text(tooltip)
}

/// A `label … value` row for the This Computer info block.
#[allow(dead_code)]
fn info_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .size(13.0)
                .color(crate::theme::palette().text_weak),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(value)
                    .size(13.0)
                    .color(crate::theme::palette().text_muted),
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

fn friendly_error(error: &str, lang: UiLang) -> String {
    if error.contains("Wrong Password") {
        tr(lang, "Неверный пароль. Проверьте пароль на удаленном ПК.", "Wrong password. Check the password on the remote PC.").to_owned()
    } else if error.contains("Offline:") || error.contains("Rendezvous refused: Offline") {
        tr(lang, "Удаленный ID сейчас не в сети.", "Remote ID is currently offline.").to_owned()
    } else if error.contains("ID does not exist") {
        tr(lang, "Такой ID не найден на сервере.", "This ID was not found on the server.").to_owned()
    } else if error.contains("Введите ID") || error.contains("Enter ID") {
        error.to_owned()
    } else if error.contains("Введите пароль") || error.contains("Enter password") {
        error.to_owned()
    } else if error.contains("Background task stopped unexpectedly") {
        tr(lang, "Соединение неожиданно остановилось.", "Connection stopped unexpectedly.").to_owned()
    } else if error.contains("tls") || error.contains("TLS") || error.contains("certificate") {
        tr(lang, "Ошибка TLS/сертификата. Проверьте настройки сервера.", "TLS/certificate error. Check your server settings.").to_owned()
    } else if error.contains("timed out") || error.contains("timeout") || error.contains("Connection timed out") {
        tr(lang, "Таймаут подключения. Проверьте сеть или попробуйте ещё раз.", "Connection timed out. Check your network or try again.").to_owned()
    } else if error.contains("Connection refused") || error.contains("connection refused") {
        tr(lang, "Сервер недоступен. Проверьте адрес и порт.", "Server refused the connection. Check the address and port.").to_owned()
    } else if error.contains("relay") || error.contains("Relay") {
        tr(lang, "Сервер-ретранслятор недоступен. Попробуйте позже.", "Relay server is unreachable. Try again later.").to_owned()
    } else if error.contains("Protocol") || error.contains("protocol") || error.contains("version mismatch") {
        tr(lang, "Несовместимая версия протокола.", "Protocol version mismatch.").to_owned()
    } else if error.contains("Network unreachable") || error.contains("No route to host") {
        tr(lang, "Нет сети. Проверьте подключение к интернету.", "No network. Check your internet connection.").to_owned()
    } else {
        error.to_owned()
    }
}
