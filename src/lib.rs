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
pub mod capability_engine;
pub mod theme;
pub mod libvirt_provider;
pub mod proxmox_provider;
pub mod provider_api;
pub mod session_backend;
pub mod smart_connect;
pub mod vmware_provider;
pub mod capture;
pub mod colorconv;
pub mod crypto;
pub mod evrt;
pub mod evrt_audio;
pub mod evrtck;
#[cfg(feature = "gpu-accel")]
pub mod evrtck_wgpu;
pub mod evrt_client;
pub mod evrt_session;
pub mod frame_queue;
pub mod fsr;
pub mod host;
pub mod mf_encode;
pub mod mf_video;
pub mod netif;
pub mod nvenc;
pub mod rustdesk_proto;
pub mod hotfix;
pub mod settings;
pub mod transport;
pub mod video;
pub mod video_pipeline;
pub mod videotoolbox;
pub mod virtualbox;
pub mod vm_bridge;
pub mod vp9_mf;

// Agentless VM-доступ через гипервизор (Hyper-V) — только Windows-хост.
#[cfg(windows)]
pub mod hyperv;

#[cfg(feature = "live-vpx-system")]
pub mod vpx_system;

// ── Android JNI-мост ──────────────────────────────────────────────────────────
#[cfg(all(target_os = "android", feature = "android-client"))]
pub mod android_ffi;
