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

#[cfg(feature = "live-vpx-system")]
pub mod vpx_system;

// ── Android JNI-мост ──────────────────────────────────────────────────────────
#[cfg(all(target_os = "android", feature = "android-client"))]
pub mod android_ffi;
#[cfg(all(target_os = "android", feature = "android-client"))]
pub mod android_input;
#[cfg(all(target_os = "android", feature = "android-client"))]
pub mod android_video;
