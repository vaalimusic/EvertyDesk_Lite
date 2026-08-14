// =============================================================================
// EvertyDesk Core — библиотека ядра (транспорт, EVRT, декод).
//
// Используется:
//   • desktop-бинарём (main.rs) — опосредованно через те же модули
//   • Android-клиентом — как cdylib `.so` через JNI-мост (android_ffi)
//
// EVRT-протокол и алгоритмы — разработка Артура Валиева (см. шапки src/evrt*.rs).
// =============================================================================

// ── Core-модули (нужны клиентскому пути transport → evrt_client → декод) ──────
pub mod address_book;
pub mod capability_engine;
pub mod capture;
pub mod colorconv;
pub mod crypto;
pub mod d3d12_encode;
pub mod evrt;
pub mod evrt2_apf;
pub mod evrt2_attention;
pub mod evrt2_crypto;
pub mod evrt2_experiment;
pub mod evrt2_fec;
pub mod evrt2_jitter;
pub mod evrt2_jitter_rig;
pub mod evrt2_modes;
pub mod evrt2_packet;
pub mod evrt2_rtt;
pub mod evrt2_scheduler;
pub mod evrt2_session;
pub mod evrt_audio;
pub mod evrt_client;
pub mod evrt_session;
pub mod evrtck;
#[cfg(feature = "gpu-accel")]
pub mod evrtck_wgpu;
pub mod execution_capability;
pub mod frame_queue;
pub mod fsr;
pub mod host;
#[cfg(not(target_os = "android"))]
pub mod host_agent;
pub mod hotfix;
pub mod libvirt_provider;
pub mod mf_encode;
pub mod mf_video;
pub mod netif;
pub mod nvenc;
pub mod onevpl;
pub mod provider_api;
pub mod proxmox_provider;
pub mod rustdesk_proto;
pub mod session_backend;
pub mod settings;
pub mod smart_connect;
#[cfg(feature = "desktop-gui")]
pub mod theme;
pub mod transport;
pub mod video;
pub mod video_pipeline;
pub mod videotoolbox;
pub mod virtualbox;
pub mod vm_bridge;
pub mod vmware_provider;
pub mod vp9_mf;

// Agentless VM-доступ через гипервизор (Hyper-V) — только Windows-хост.
#[cfg(windows)]
pub mod hyperv;

// Прямой встроенный RDP-клиент к ВМ (VirtualBox VRDE / Hyper-V Enhanced
// Session), поверх ironrdp-* — отдельный путь от evrt_client/transport,
// не через host/relay. Exposed here (was main.rs-only `mod`) so desktop-next
// can offer the same VM-console connection the egui client has.
//
// Windows-only: uses native-tls for the VRDE TLS handshake, and native-tls
// itself is a [target.'cfg(windows)'.dependencies] entry in Cargo.toml (not
// available to link against on macOS/Linux at all). This was already true
// when this lived as an ungated `mod vbox_rdp;` in main.rs — it just never
// surfaced because main.rs was never actually compiled on those platforms
// either. hyperv_rdp reuses vbox_rdp's Poll/VrdeCmd types and ironrdp
// helpers, so both are Windows-only together. desktop-next/src/bin/
// rdp_viewer.rs references vbox_rdp::{Poll, VrdeCmd} too — its non-Windows
// build defines local same-named stand-ins instead of depending on this
// module, since RdpSessionHandle there is a no-op stub on non-Windows
// anyway (see the #[cfg(not(windows))] block in rdp_viewer.rs).
#[cfg(windows)]
pub mod hyperv_rdp;
#[cfg(windows)]
pub mod vbox_rdp;
#[cfg(windows)]
pub mod vm_console_runtime;

// Phase 3/4 (TZ_HOST_SERVICE.md): OS service install/query — Windows service
// (Session 0 + linked-token elevation) / systemd --user / launchd. Exposed
// from the core library, not just main.rs's own binary, so other desktop
// front-ends (e.g. desktop-next) can offer the same "install service" path
// without reimplementing it.
#[cfg(unix)]
pub mod host_service_unix;
#[cfg(windows)]
pub mod winservice;

#[cfg(feature = "live-vpx-system")]
pub mod vpx_system;

// ── Android JNI-мост ──────────────────────────────────────────────────────────
#[cfg(all(target_os = "android", feature = "android-client"))]
pub mod android_ffi;
#[cfg(all(target_os = "android", feature = "android-client"))]
pub mod android_input;
#[cfg(all(target_os = "android", feature = "android-client"))]
pub mod android_video;
