// =============================================================================
// EVRT Protocol — разработан Артуром Валиевым (Artur Valiev)
// Оригинальная реализация: EvertyGame (C#, https://github.com/djvaliev)
// Rust-порт для EvertyDesk Lite выполнен на основе оригинальных алгоритмов.
// =============================================================================

//! EVRT UDP сессия — отправка готовых кадров клиенту.
//!
//! ## Принципиальное отличие от старой реализации
//!
//! Старая версия дублировала захват + кодирование внутри себя.
//! Теперь сессия **принимает уже закодированные кадры** из единого
//! `video_pipeline` и только отвечает за:
//!
//! - UDP пакетизацию (EVRT framing)
//! - Feedback loop (ReceiverFeedback → AdaptiveRelief)
//! - SessionConfig keepalive
//! - Windows performance hints
//! - Аудио захват (WASAPI loopback)

use std::{
    net::{SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{Receiver, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    evrt::{self, ControlMessage, ReceiverFeedback, SessionConfig},
    frame_queue::AdaptiveRelief,
    host::HostEvent,
    settings::AppConfig,
    video_pipeline::EncodedFrame,
};

// ─── константы ────────────────────────────────────────────────────────────────

const IDLE_TIMEOUT: Duration = Duration::from_secs(8);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(2);
const IDR_MIN_INTERVAL: Duration = Duration::from_secs(2);
const SPIN_THRESHOLD: Duration = Duration::from_micros(1_500);

// ─── публичный интерфейс ──────────────────────────────────────────────────────

/// Параметры EVRT UDP сессии.
///
/// Сессия не захватывает экран и не кодирует — всё это делает `video_pipeline`.
/// Здесь только UDP-доставка + feedback.
pub struct EvrtSessionParams {
    /// Адрес клиента (после punch-hole).
    pub peer_addr: SocketAddr,
    /// UDP сокет (тот же что зарегистрирован на hbbs).
    pub socket: Arc<UdpSocket>,
    /// Конфиг приложения (для SessionConfig).
    pub config: AppConfig,
    /// ID пира для логов.
    pub peer_id: String,
    /// События → UI.
    pub events: Sender<HostEvent>,
    /// Сигнал остановки (shared с pipeline).
    pub stop: Arc<AtomicBool>,
    /// ★ Готовые кадры из video_pipeline — основная инновация.
    ///   Нет дублирующего захвата/кодирования.
    pub frame_rx: Receiver<EncodedFrame>,
    /// Актуальный FPS (для отчёта в SessionConfig keepalive).
    pub target_fps: Arc<AtomicU32>,
    /// Актуальный quality milli.
    pub quality_milli: Arc<AtomicU32>,
    /// Масштаб bitrate от EVRT feedback: 1000 = полный bitrate, ниже = relief.
    pub bitrate_scale_milli: Arc<AtomicU32>,
}

/// Запустить EVRT сессию. Блокирует до завершения.
pub fn run_evrt_session(params: EvrtSessionParams) -> Result<(), String> {
    let EvrtSessionParams {
        peer_addr,
        socket,
        config,
        peer_id,
        events,
        stop,
        frame_rx,
        target_fps,
        quality_milli,
        bitrate_scale_milli,
    } = params;

    // ── Windows performance hints ─────────────────────────────────────────────
    let _perf = WindowsPerfHints::enable(&events);

    evrt_log(&events, format!("EVRT session → {peer_addr}"));
    bitrate_scale_milli.store(1_000, Ordering::Relaxed);

    // ── SessionConfig ─────────────────────────────────────────────────────────
    let (screen_w, screen_h) = crate::capture::screen_size().unwrap_or((1920, 1080));
    let fps = target_fps.load(Ordering::Relaxed).clamp(5, 60);
    let bitrate = crate::host::h264_target_bitrate_bps_pub(
        screen_w,
        screen_h,
        fps,
        quality_milli.load(Ordering::Relaxed),
    );

    let session_cfg = SessionConfig {
        codec: choose_codec(&config),
        preset: if config.display.fsr_quality.is_enabled() {
            "GAME".into()
        } else {
            "MEDIA".into()
        },
        width: screen_w,
        height: screen_h,
        fps,
        bitrate,
        stream_mode: "single".into(),
        adaptation_mode: "GAME".into(),
    };

    // SessionConfig ×2 против потери первого UDP
    let cfg_pkt = evrt::build_session_config(&session_cfg.to_json());
    send_udp(&socket, &cfg_pkt, peer_addr)?;
    thread::sleep(Duration::from_millis(5));
    send_udp(&socket, &cfg_pkt, peer_addr)?;

    evrt_log(
        &events,
        format!(
            "EVRT SessionConfig: {}×{}@{} {:.1}Mbps {}",
            screen_w,
            screen_h,
            fps,
            bitrate as f64 / 1_000_000.0,
            session_cfg.codec,
        ),
    );

    // ── Ожидаем RequestKeyFrame от клиента ────────────────────────────────────
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .ok();
    let mut buf = vec![0u8; evrt::MAX_PACKET_SIZE + 64];
    let deadline = Instant::now() + Duration::from_secs(5);

    loop {
        if stop.load(Ordering::Relaxed) {
            return Err("stopped before client connected".into());
        }
        if Instant::now() > deadline {
            return Err(format!("client {peer_addr} did not respond"));
        }
        match socket.recv_from(&mut buf) {
            Ok((_, src)) if src == peer_addr => {
                evrt_log(&events, "EVRT: client confirmed".into());
                break;
            }
            Ok(_) | Err(_) => {
                send_udp(&socket, &cfg_pkt, peer_addr)?; // retry
            }
        }
    }

    // ── Feedback + keyframe request channels ──────────────────────────────────
    let (fb_tx, fb_rx) = std::sync::mpsc::channel::<ReceiverFeedback>();
    let (kf_tx, kf_rx) = std::sync::mpsc::channel::<()>();

    // ── Receive loop: feedback/control от клиента ─────────────────────────────
    {
        let recv_sock = socket.clone();
        let recv_stop = stop.clone();
        let recv_events = events.clone();

        thread::spawn(move || {
            let mut buf = vec![0u8; evrt::MAX_PACKET_SIZE + 64];
            let mut last_pkt = Instant::now();
            recv_sock
                .set_read_timeout(Some(Duration::from_millis(500)))
                .ok();

            while !recv_stop.load(Ordering::Relaxed) {
                match recv_sock.recv_from(&mut buf) {
                    Ok((len, src)) if src == peer_addr => {
                        last_pkt = Instant::now();
                        if let Some(pkt) = evrt::parse(&buf, len) {
                            if pkt.packet_type == evrt::TYPE_CONTROL {
                                match evrt::parse_control(&pkt.payload) {
                                    Some(ControlMessage::RequestKeyFrame) => {
                                        let _ = kf_tx.send(());
                                    }
                                    Some(ControlMessage::ReceiverFeedback(fb)) => {
                                        let _ = fb_tx.send(fb);
                                    }
                                    None => {}
                                }
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(ref e) if is_would_block(e) => {}
                    Err(_) => break,
                }

                if last_pkt.elapsed() > IDLE_TIMEOUT {
                    evrt_log(&recv_events, "EVRT: idle timeout".into());
                    recv_stop.store(true, Ordering::Relaxed);
                    break;
                }
            }
        });
    }

    // ── Аудио (WASAPI loopback) ───────────────────────────────────────────────
    {
        let audio_sock = socket.clone();
        let audio_stop = stop.clone();
        thread::spawn(move || {
            crate::evrt_audio::run_audio_capture(audio_sock, peer_addr, audio_stop);
        });
    }

    // ── Главный цикл: берём кадры из pipeline → пакетизируем → UDP ───────────
    let mut relief = AdaptiveRelief::new(true);
    let mut last_idr = Instant::now();
    let mut last_keepalive = Instant::now();

    // HNS PTS (как в EvertyGame _sampleTimeHns)
    let mut sample_hns: u64 = 0;
    let hns_per_frame = |fps: u32| 10_000_000u64 / fps.max(1) as u64;

    evrt_log(&events, "EVRT: main loop started".into());

    while !stop.load(Ordering::Relaxed) {
        // ── Feedback от клиента ───────────────────────────────────────────────
        while let Ok(fb) = fb_rx.try_recv() {
            let cur_fps = target_fps.load(Ordering::Relaxed);
            if let Some(step) = relief.on_feedback(&fb, cur_fps) {
                let scale_milli = relief
                    .apply_pending_milli()
                    .unwrap_or_else(|| AdaptiveRelief::bitrate_scale_milli(step));
                bitrate_scale_milli.store(scale_milli, Ordering::Relaxed);
                evrt_log(
                    &events,
                    format!(
                        "EVRT adaptive relief step={} scale={}pct pressure={}",
                        relief.current_step(),
                        scale_milli / 10,
                        fb.pressure.as_str(),
                    ),
                );
            }
        }

        // ── Keyframe request ──────────────────────────────────────────────────
        let kf_requested = kf_rx.try_recv().is_ok();

        // ── Keepalive SessionConfig ───────────────────────────────────────────
        if last_keepalive.elapsed() > KEEPALIVE_INTERVAL {
            let cur_fps = target_fps.load(Ordering::Relaxed).clamp(5, 60);
            let cur_bps = crate::host::h264_target_bitrate_bps_pub(
                screen_w,
                screen_h,
                cur_fps,
                quality_milli.load(Ordering::Relaxed),
            );
            let cur_bps = scale_bitrate_bps(cur_bps, bitrate_scale_milli.load(Ordering::Relaxed));
            let upd = SessionConfig {
                fps: cur_fps,
                bitrate: cur_bps,
                ..session_cfg.clone()
            };
            let _ = send_udp(
                &socket,
                &evrt::build_session_config(&upd.to_json()),
                peer_addr,
            );
            last_keepalive = Instant::now();
        }

        // ── Получить кадр из pipeline ─────────────────────────────────────────
        let frame = match frame_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(f) => f,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break, // pipeline завершился
        };

        // Если клиент запросил IDR или пора по таймеру — сигнализируем pipeline
        // через следующий цикл (IDR приходит сам по себе из EncoderThread)
        if kf_requested || last_idr.elapsed() > IDR_MIN_INTERVAL {
            // IDR запрошен, но мы не можем форсировать энкодер отсюда —
            // pipeline сам генерирует IDR по своему таймеру.
            // Просто пропускаем до следующего IDR если нужен.
            if kf_requested && !frame.is_idr {
                continue; // дропаем не-IDR кадры пока ждём IDR
            }
        }

        if frame.is_idr {
            last_idr = Instant::now();
            // CodecConfig (SPS/PPS) перед IDR
            if let Some(ref sps_pps) = frame.sps_pps {
                let _ = send_udp(&socket, &evrt::build_codec_config(sps_pps), peer_addr);
            }
        }

        // ── ROI перед кадром ─────────────────────────────────────────────────
        let roi = evrt::RoiRect {
            frame_id: frame.frame_id,
            ..frame.roi
        };
        let roi_pkt = evrt::build_roi_metadata(roi);
        if !roi_pkt.is_empty() {
            let _ = send_udp(&socket, &roi_pkt, peer_addr);
        }

        // ── PTS ───────────────────────────────────────────────────────────────
        let cur_fps = target_fps.load(Ordering::Relaxed).clamp(5, 60);
        let pts_us = sample_hns / 10;
        sample_hns = sample_hns.wrapping_add(hns_per_frame(cur_fps));

        // ── Пакетизация и отправка ────────────────────────────────────────────
        let pkts = evrt::packetize_video_frame(frame.frame_id, pts_us, frame.is_idr, &frame.bytes);
        for pkt in &pkts {
            if send_udp(&socket, pkt, peer_addr).is_err() {
                stop.store(true, Ordering::Relaxed);
                break;
            }
        }
    }

    let _ = events.send(HostEvent::SessionEnded {
        peer_id: peer_id.clone(),
        reason: "EVRT".into(),
    });
    let _ = events.send(HostEvent::StateChanged(crate::host::HostState::Ready));

    evrt_log(&events, format!("EVRT: сессия с {peer_id} завершена"));
    bitrate_scale_milli.store(1_000, Ordering::Relaxed);
    crate::host::release_stuck_input_pub();
    Ok(())
}

// ─── Windows performance hints ────────────────────────────────────────────────

struct WindowsPerfHints {
    #[cfg(target_os = "windows")]
    timer_raised: bool,
    #[cfg(target_os = "windows")]
    original_priority: Option<u32>,
}

impl WindowsPerfHints {
    fn enable(events: &Sender<HostEvent>) -> Self {
        #[cfg(target_os = "windows")]
        {
            use std::os::raw::c_uint;
            #[link(name = "winmm")]
            extern "system" {
                fn timeBeginPeriod(uPeriod: c_uint) -> c_uint;
            }
            let timer_raised = unsafe { timeBeginPeriod(1) } == 0;
            if timer_raised {
                evrt_log(events, "EVRT perf: timer → 1 ms ✓".into());
            }
            #[link(name = "kernel32")]
            extern "system" {
                fn GetCurrentProcess() -> *mut std::ffi::c_void;
                fn SetPriorityClass(h: *mut std::ffi::c_void, c: c_uint) -> i32;
                fn GetPriorityClass(h: *mut std::ffi::c_void) -> c_uint;
            }
            let proc = unsafe { GetCurrentProcess() };
            let orig = unsafe { GetPriorityClass(proc) };
            let ok = unsafe { SetPriorityClass(proc, 0x80) }; // HIGH
            if ok != 0 {
                evrt_log(events, "EVRT perf: priority → High ✓".into());
            }
            Self {
                timer_raised,
                original_priority: Some(orig),
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = events;
            Self {}
        }
    }
}

impl Drop for WindowsPerfHints {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        {
            use std::os::raw::c_uint;
            if self.timer_raised {
                #[link(name = "winmm")]
                extern "system" {
                    fn timeEndPeriod(uPeriod: c_uint) -> c_uint;
                }
                unsafe {
                    timeEndPeriod(1);
                }
            }
            if let Some(orig) = self.original_priority {
                #[link(name = "kernel32")]
                extern "system" {
                    fn GetCurrentProcess() -> *mut std::ffi::c_void;
                    fn SetPriorityClass(h: *mut std::ffi::c_void, c: c_uint) -> i32;
                }
                unsafe {
                    SetPriorityClass(GetCurrentProcess(), orig);
                }
            }
        }
    }
}

// ─── helpers ──────────────────────────────────────────────────────────────────

fn send_udp(socket: &UdpSocket, data: &[u8], addr: SocketAddr) -> Result<(), String> {
    socket
        .send_to(data, addr)
        .map_err(|e| format!("UDP send: {e}"))?;
    Ok(())
}

fn is_would_block(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut
}

fn choose_codec(config: &AppConfig) -> String {
    match config.display.codec {
        crate::settings::CodecPreference::H265 => "H265",
        _ => "H264",
    }
    .to_owned()
}

fn scale_bitrate_bps(base_bps: u32, scale_milli: u32) -> u32 {
    let scale_milli = scale_milli.clamp(1, 1_000);
    ((u64::from(base_bps) * u64::from(scale_milli)) / 1_000) as u32
}

fn evrt_log(events: &Sender<HostEvent>, msg: String) {
    eprintln!("[evrt] {msg}");
    let _ = events.send(HostEvent::Log(msg));
}

/// Декодировать `socket_addr` из RustDesk protobuf в `SocketAddr`.
pub fn decode_punch_addr(bytes: &[u8]) -> Option<SocketAddr> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    match bytes.len() {
        6 => {
            let ip = Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]);
            let port = u16::from_be_bytes([bytes[4], bytes[5]]);
            Some(SocketAddr::new(IpAddr::V4(ip), port))
        }
        18 => {
            let ip_bytes: [u8; 16] = bytes[..16].try_into().ok()?;
            let port = u16::from_be_bytes([bytes[16], bytes[17]]);
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(ip_bytes)), port))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_v4() {
        let bytes = [10u8, 0, 0, 1, 0x1F, 0x90];
        let addr = decode_punch_addr(&bytes).unwrap();
        assert_eq!(addr.port(), 8080);
        assert_eq!(addr.ip().to_string(), "10.0.0.1");
    }

    #[test]
    fn decode_invalid() {
        assert!(decode_punch_addr(&[]).is_none());
        assert!(decode_punch_addr(&[1, 2]).is_none());
    }
}
